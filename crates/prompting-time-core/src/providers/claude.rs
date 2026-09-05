//! Claude's stdio protocol, with one owned process per active turn.
//!
//! Bindings are retained for this adapter's lifetime (at most 4096). After restart the runtime's
//! persisted binding is trusted; a missing native transcript fails closed and requires a new
//! conversation. In particular, initialize without a first prompt does not create a transcript.
//! Claude exposes session and message IDs, but not a turn ID: TurnStarted uses our generation UUID.

mod protocol;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout_at};
use uuid::Uuid;

use super::process::{
    EVENT_CHANNEL_CAPACITY, JsonLineProcess, JsonLineSender, JsonLineShutdown, MAX_LINE_BYTES,
};
use super::{
    ApprovalResponse, ProviderAdapter, ProviderCapabilities, ProviderCapability, ProviderError,
    ProviderErrorCategory, ProviderEvent, ProviderHealth, ProviderId, ProviderSession,
    ProviderTurn, ProviderTurnOwner, ResumeSession, StartSession, TurnRequest,
};
use crate::domain::ConversationId;
use protocol::{PendingControl, Protocol};

const MAX_SESSION_BINDINGS: usize = 4096;
// Each retained input is limited to 64 KiB by protocol.rs: at most 8 MiB pending per turn.
// Answered entries retain only identities; original payloads are released at response reservation.
const MAX_CONTROLS: usize = 128;
const OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
const INTERRUPT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct ClaudeAdapter {
    inner: Arc<AdapterInner>,
}

struct AdapterInner {
    binary: PathBuf,
    sessions: Mutex<HashMap<String, Arc<Binding>>>,
    stopped: AtomicBool,
}

struct Binding {
    conversation: ConversationId,
    workspace: PathBuf,
    dispatched: Arc<AtomicBool>,
    active: Mutex<Option<Arc<ActiveTurn>>>,
}

struct ActiveTurn {
    shared: Arc<TurnState>,
    worker: AsyncMutex<Option<JoinHandle<Result<(), ProviderError>>>>,
}

// The worker holds this state, not the ActiveTurn that owns its join handle.
struct TurnState {
    generation: String,
    sender: JsonLineSender,
    process_shutdown: JsonLineShutdown,
    stop: watch::Sender<bool>,
    terminal: watch::Sender<Option<Result<(), ProviderError>>>,
    controls: Mutex<HashMap<String, PendingControl>>,
    interrupt_sent: AtomicBool,
}

impl TurnState {
    fn stop(&self) {
        self.stop.send_replace(true);
        self.process_shutdown.request();
    }
}

struct CancelOnDrop(Option<Arc<TurnState>>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(state) = &self.0 {
            state.stop();
        }
    }
}

impl ActiveTurn {
    async fn stop(&self) -> Result<(), ProviderError> {
        self.shared.stop();
        let mut slot = self.worker.lock().await;
        let result = match slot.as_mut() {
            Some(worker) => worker.await.map_err(|_| protocol_error("worker-stopped"))?,
            None => return Ok(()),
        };
        slot.take();
        result
    }
}

struct TurnOwner(Arc<ActiveTurn>);

impl Drop for TurnOwner {
    fn drop(&mut self) {
        self.0.shared.stop();
    }
}

#[async_trait]
impl ProviderTurnOwner for TurnOwner {
    async fn shutdown(self: Box<Self>) -> Result<(), ProviderError> {
        self.0.stop().await
    }
}

impl Drop for AdapterInner {
    fn drop(&mut self) {
        for binding in self.sessions.lock().unwrap().values() {
            if let Some(active) = binding.active.lock().unwrap().as_ref() {
                active.shared.stop();
            }
        }
    }
}

impl ClaudeAdapter {
    pub fn new(binary: PathBuf) -> Self {
        Self {
            inner: Arc::new(AdapterInner {
                binary,
                sessions: Mutex::new(HashMap::new()),
                stopped: AtomicBool::new(false),
            }),
        }
    }

    fn bind(
        &self,
        native_id: String,
        request: StartSession,
        dispatched: bool,
    ) -> Result<ProviderSession, ProviderError> {
        if Uuid::parse_str(&native_id).is_err() {
            return Err(rejected());
        }
        let workspace = request
            .working_directory
            .canonicalize()
            .map_err(|_| rejected())?;
        if !workspace.is_dir() {
            return Err(rejected());
        }
        let mut sessions = self.inner.sessions.lock().unwrap();
        if self.inner.stopped.load(Ordering::Acquire) {
            return Err(rejected());
        }
        if let Some(binding) = sessions.get(&native_id) {
            if binding.conversation != request.conversation_id || binding.workspace != workspace {
                return Err(rejected());
            }
        } else {
            if sessions.len() >= MAX_SESSION_BINDINGS {
                return Err(protocol_error(
                    "session-binding-capacity-create-new-adapter",
                ));
            }
            sessions.insert(
                native_id.clone(),
                Arc::new(Binding {
                    conversation: request.conversation_id,
                    workspace,
                    dispatched: Arc::new(AtomicBool::new(dispatched)),
                    active: Mutex::new(None),
                }),
            );
        }
        Ok(ProviderSession {
            provider: ProviderId::Claude,
            native_id,
            native_group_id: None,
        })
    }

    fn binding(&self, session: &ProviderSession) -> Result<Arc<Binding>, ProviderError> {
        if session.provider != ProviderId::Claude
            || session.native_group_id.is_some()
            || self.inner.stopped.load(Ordering::Acquire)
        {
            return Err(rejected());
        }
        self.inner
            .sessions
            .lock()
            .unwrap()
            .get(&session.native_id)
            .cloned()
            .ok_or_else(rejected)
    }

    fn active(&self, session: &ProviderSession) -> Result<Arc<ActiveTurn>, ProviderError> {
        let binding = self.binding(session)?;
        let active = binding
            .active
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(rejected)?;
        if *active.shared.stop.borrow() || active.shared.terminal.borrow().is_some() {
            return Err(rejected());
        }
        Ok(active)
    }

    fn active_turns(&self) -> Vec<Arc<ActiveTurn>> {
        self.inner
            .sessions
            .lock()
            .unwrap()
            .values()
            .filter_map(|binding| binding.active.lock().unwrap().clone())
            .collect()
    }
}

#[async_trait]
impl ProviderAdapter for ClaudeAdapter {
    fn id(&self) -> ProviderId {
        ProviderId::Claude
    }

    fn capabilities(&self) -> ProviderCapabilities {
        [
            ProviderCapability::Streaming,
            ProviderCapability::DeferredApproval,
            ProviderCapability::Interruption,
            ProviderCapability::Resume,
            ProviderCapability::ChildAgents,
        ]
        .into()
    }

    async fn health(&self) -> Result<ProviderHealth, ProviderError> {
        let unavailable = |category: &str| {
            Ok(ProviderHealth::Unavailable {
                category: category.into(),
            })
        };
        let version = match inspect(&self.inner.binary, &["--version"]).await {
            Ok((bytes, true)) => String::from_utf8_lossy(&bytes)
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_owned(),
            _ => return unavailable("claude-not-installed-or-inspection-failed"),
        };
        let parts: Vec<_> = version.split('.').map(str::parse::<u32>).collect();
        if !matches!(parts.as_slice(), [Ok(2), Ok(minor), Ok(patch)] if (*minor, *patch) >= (1, 205))
        {
            return unavailable("claude-requires-major-2-version-2.1.205-or-newer");
        }
        let auth = match inspect(&self.inner.binary, &["auth", "status", "--json"]).await {
            Ok((bytes, success)) => serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|value| value.get("loggedIn").and_then(Value::as_bool))
                .filter(|logged_in| success || !logged_in),
            Err(_) => None,
        };
        match auth {
            Some(true) => Ok(ProviderHealth::Healthy { version }),
            Some(false) => unavailable("claude-login-required-run-claude-auth-login"),
            None => unavailable("claude-auth-status-unavailable-run-claude-auth-login"),
        }
    }

    async fn start_session(&self, request: StartSession) -> Result<ProviderSession, ProviderError> {
        self.bind(Uuid::now_v7().to_string(), request, false)
    }

    async fn resume_session(
        &self,
        native_id: &str,
        request: ResumeSession,
    ) -> Result<ProviderSession, ProviderError> {
        self.bind(
            native_id.into(),
            StartSession {
                conversation_id: request.conversation_id,
                working_directory: request.working_directory,
            },
            true,
        )
    }

    async fn start_turn(
        &self,
        session: &ProviderSession,
        request: TurnRequest,
    ) -> Result<ProviderTurn, ProviderError> {
        let prompt = json!({"type":"user", "message":{"role":"user", "content":request.prompt},
            "parent_tool_use_id":null, "session_id":session.native_id});
        if serde_json::to_vec(&prompt).map_err(|_| rejected())?.len() > MAX_LINE_BYTES {
            return Err(rejected());
        }
        let binding = self.binding(session)?;
        let previous = binding.active.lock().unwrap().clone();
        if let Some(previous) = previous {
            if !*previous.shared.stop.borrow() && previous.shared.terminal.borrow().is_none() {
                return Err(rejected());
            }
            previous.stop().await?;
        }
        let (events, receiver) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
        let (ready, started) = oneshot::channel();
        let active = {
            let mut slot = binding.active.lock().unwrap();
            if self.inner.stopped.load(Ordering::Acquire) {
                return Err(rejected());
            }
            if let Some(active) = slot.as_ref()
                && !*active.shared.stop.borrow()
                && active.shared.terminal.borrow().is_none()
            {
                return Err(rejected());
            }
            let mut command = Command::new(&self.inner.binary);
            command
                .args([
                    "--print",
                    "--input-format",
                    "stream-json",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--include-partial-messages",
                    "--no-chrome",
                    "--permission-mode",
                    "default",
                    "--permission-prompt-tool",
                    "stdio",
                    "--setting-sources=",
                    "--strict-mcp-config",
                    "--mcp-config",
                    r#"{"mcpServers":{}}"#,
                ])
                .arg(format!(
                    "--{}={}",
                    if binding.dispatched.load(Ordering::Acquire) {
                        "resume"
                    } else {
                        "session-id"
                    },
                    session.native_id
                ))
                .current_dir(&binding.workspace)
                .env_remove("CLAUDECODE")
                .env("CLAUDE_CODE_ENTRYPOINT", "prompting-time");
            let process = JsonLineProcess::spawn(command)?;
            let (stop, _) = watch::channel(false);
            let (terminal, _) = watch::channel(None);
            let shared = Arc::new(TurnState {
                generation: Uuid::now_v7().to_string(),
                sender: process.sender(),
                process_shutdown: process.shutdown_handle(),
                stop,
                terminal,
                controls: Mutex::new(HashMap::new()),
                interrupt_sent: AtomicBool::new(false),
            });
            let worker = tokio::spawn(run_turn(
                process,
                Arc::clone(&shared),
                session.native_id.clone(),
                Arc::clone(&binding.dispatched),
                prompt,
                events,
                ready,
            ));
            let active = Arc::new(ActiveTurn {
                shared,
                worker: AsyncMutex::new(Some(worker)),
            });
            *slot = Some(Arc::clone(&active));
            active
        };
        let mut guard = CancelOnDrop(Some(Arc::clone(&active.shared)));
        match started
            .await
            .map_err(|_| protocol_error("start-worker-stopped"))?
        {
            Ok(()) => {
                guard.0.take();
                Ok(ProviderTurn::new(receiver, TurnOwner(active)))
            }
            Err(error) => {
                active.stop().await?;
                Err(error)
            }
        }
    }

    async fn steer(&self, _: &ProviderSession, _: &str, _: &str) -> Result<(), ProviderError> {
        Err(rejected())
    }

    async fn respond(
        &self,
        session: &ProviderSession,
        request_id: &str,
        response: ApprovalResponse,
    ) -> Result<(), ProviderError> {
        let active = self.active(session)?;
        let value = {
            let mut controls = active.shared.controls.lock().unwrap();
            let pending = controls.get_mut(request_id).ok_or_else(rejected)?;
            if pending.claimed {
                return Err(rejected());
            }
            let value = pending.response(response)?;
            // Reserve before any enqueue. Cancellation after this point never restores pending.
            pending.claimed = true;
            pending.release_input();
            value
        };
        let mut guard = CancelOnDrop(Some(Arc::clone(&active.shared)));
        write(
            &active.shared.sender,
            &value,
            Instant::now() + OPERATION_TIMEOUT,
        )
        .await?;
        if let Some(control) = active.shared.controls.lock().unwrap().get_mut(request_id) {
            control.written = true;
        }
        guard.0.take();
        Ok(())
    }

    async fn interrupt(
        &self,
        session: &ProviderSession,
        active_turn: &str,
    ) -> Result<(), ProviderError> {
        let active = self.active(session)?;
        if active.shared.generation != active_turn
            || active.shared.interrupt_sent.swap(true, Ordering::AcqRel)
        {
            return Err(rejected());
        }
        let mut guard = CancelOnDrop(Some(Arc::clone(&active.shared)));
        let deadline = Instant::now() + INTERRUPT_TIMEOUT;
        let mut terminal = active.shared.terminal.subscribe();
        let result = async {
            write(&active.shared.sender, &json!({"type":"control_request", "request_id":format!("interrupt:{active_turn}"), "request":{"subtype":"interrupt"}}), deadline).await?;
            loop {
                if let Some(result) = terminal.borrow().clone() { return result; }
                timeout_at(deadline, terminal.changed()).await.map_err(|_| protocol_error("interrupt-terminal-timeout"))?.map_err(|_| protocol_error("interrupt-owner-stopped"))?;
            }
        }.await;
        active.stop().await?;
        guard.0.take();
        result
    }

    async fn shutdown(&self) -> Result<(), ProviderError> {
        self.inner.stopped.store(true, Ordering::Release);
        let active = self.active_turns();
        for turn in &active {
            turn.shared.stop();
        }
        let mut result = Ok(());
        for turn in active {
            if let Err(error) = turn.stop().await {
                result = Err(error);
            }
        }
        result
    }

    fn force_shutdown(&self) {
        self.inner.stopped.store(true, Ordering::Release);
        for active in self.active_turns() {
            active.shared.stop();
        }
    }
}

async fn inspect(binary: &PathBuf, args: &[&str]) -> Result<(Vec<u8>, bool), ProviderError> {
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| protocol_error("inspection-unavailable"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| protocol_error("inspection-stdout"))?
        .take(16 * 1024 + 1);
    let operation = async {
        let mut bytes = Vec::new();
        stdout
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| protocol_error("inspection-read"))?;
        if bytes.len() > 16 * 1024 {
            return Err(protocol_error("inspection-output-limit"));
        }
        // auth status may return nonzero for a valid loggedIn=false response.
        let status = child
            .wait()
            .await
            .map_err(|_| protocol_error("inspection-exit"))?;
        Ok((bytes, status.success()))
    };
    let result = timeout_at(Instant::now() + Duration::from_secs(5), operation)
        .await
        .unwrap_or_else(|_| Err(protocol_error("inspection-timeout")));
    if result.is_err() {
        let _ = child.kill().await;
    }
    result
}

async fn write(
    sender: &JsonLineSender,
    value: &Value,
    deadline: Instant,
) -> Result<(), ProviderError> {
    timeout_at(deadline, sender.send(value))
        .await
        .map_err(|_| protocol_error("control-write-timeout"))?
}

fn protocol_error(category: &str) -> ProviderError {
    ProviderError::Protocol {
        category: format!("claude-{category}"),
    }
}
fn rejected() -> ProviderError {
    ProviderError::NotDispatched {
        category: ProviderErrorCategory::Rejected,
    }
}

async fn run_turn(
    mut process: JsonLineProcess,
    state: Arc<TurnState>,
    session: String,
    dispatched: Arc<AtomicBool>,
    prompt: Value,
    events: mpsc::Sender<Result<ProviderEvent, ProviderError>>,
    ready: oneshot::Sender<Result<(), ProviderError>>,
) -> Result<(), ProviderError> {
    let mut stop = state.stop.subscribe();
    let mut ready = Some(ready);
    let run = async {
        let deadline = Instant::now() + OPERATION_TIMEOUT;
        let init_id = format!("initialize:{}", state.generation);
        write(&state.sender, &json!({"type":"control_request","request_id":init_id,"request":{"subtype":"initialize","hooks":null,"forwardSubagentText":true}}), deadline).await?;
        loop {
            let value = timeout_at(deadline, process.recv())
                .await
                .map_err(|_| protocol_error("initialize-timeout"))?
                .ok_or(ProviderError::StreamClosed)??;
            protocol::validate_session(&value, &session)?;
            if value["type"] == "control_response" && value["response"]["request_id"] == init_id {
                if value["response"]["subtype"] != "success" {
                    return Err(protocol_error("initialize-rejected"));
                }
                break;
            }
            if value["type"] != "system" || value["subtype"] != "init" {
                return Err(protocol_error("unexpected-initialize-envelope"));
            }
        }
        // From this point a prompt might reach the native process. Never retry it as Fresh.
        dispatched.store(true, Ordering::Release);
        write(&state.sender, &prompt, deadline).await?;
        events
            .send(Ok(ProviderEvent::TurnStarted {
                native_turn_id: state.generation.clone(),
            }))
            .await
            .map_err(|_| ProviderError::StreamClosed)?;
        if let Some(ready) = ready.take() {
            let _ = ready.send(Ok(()));
        }
        let mut protocol = Protocol::new(session.clone());
        loop {
            let value = process.recv().await.ok_or(ProviderError::StreamClosed)??;
            protocol::validate_session(&value, &session)?;
            match value["type"].as_str() {
                Some("control_request") => {
                    let native_id = protocol::required_id(&value, "request_id")?;
                    let request_id = format!("{}:{native_id}", state.generation);
                    let (mut pending, event, reply) =
                        protocol::control(&value, request_id.clone())?;
                    if pending.claimed {
                        pending.release_input();
                    }
                    {
                        let mut controls = state.controls.lock().unwrap();
                        if controls.len() >= MAX_CONTROLS
                            || controls.contains_key(&request_id)
                            || pending.tool_use_id.as_ref().is_some_and(|tool| {
                                controls
                                    .values()
                                    .any(|control| control.tool_use_id.as_ref() == Some(tool))
                            })
                        {
                            return Err(protocol_error("control-capacity-or-duplicate"));
                        }
                        controls.insert(request_id.clone(), pending);
                    }
                    if let Some(reply) = reply {
                        write(&state.sender, &reply, Instant::now() + OPERATION_TIMEOUT).await?;
                        if let Some(control) = state.controls.lock().unwrap().get_mut(&request_id) {
                            control.written = true;
                        }
                    }
                    if let Some(event) = event {
                        events
                            .send(Ok(event))
                            .await
                            .map_err(|_| ProviderError::StreamClosed)?;
                    }
                }
                Some("control_cancel_request") => {
                    let native_id = protocol::required_id(&value, "request_id")?;
                    let key = format!("{}:{native_id}", state.generation);
                    if state
                        .controls
                        .lock()
                        .unwrap()
                        .get(&key)
                        .is_some_and(|request| !request.written)
                    {
                        return Err(protocol_error("active-question-withdrawn-turn-stopped"));
                    }
                }
                Some("control_response") => {
                    if value["response"]["request_id"] != format!("interrupt:{}", state.generation)
                        || !state.interrupt_sent.load(Ordering::Acquire)
                        || value["response"]["subtype"] != "success"
                    {
                        return Err(protocol_error("unexpected-control-response"));
                    }
                }
                _ => {
                    for event in protocol.normalize(value)? {
                        let terminal = event.is_terminal();
                        if terminal {
                            if state
                                .controls
                                .lock()
                                .unwrap()
                                .values()
                                .any(|control| !control.claimed)
                                && event != ProviderEvent::Interrupted
                            {
                                return Err(protocol_error("result-with-pending-control"));
                            }
                            state.terminal.send_replace(Some(Ok(())));
                        }
                        events
                            .send(Ok(event))
                            .await
                            .map_err(|_| ProviderError::StreamClosed)?;
                        if terminal {
                            return Ok(());
                        }
                    }
                }
            }
        }
    };
    let result = tokio::select! {
        biased;
        _ = async { if !*stop.borrow() { let _ = stop.changed().await; } } => Err(ProviderError::StreamClosed),
        result = run => result,
    };
    let result = result.map_err(|error| {
        let stderr = String::from_utf8_lossy(&process.stderr_snapshot()).to_lowercase();
        if stderr.contains("no conversation found")
            || stderr.contains("no session found")
            || stderr.contains("session not found")
        {
            protocol_error("native-session-missing-start-new-conversation")
        } else {
            error
        }
    });
    if let Err(error) = &result {
        state.terminal.send_replace(Some(Err(error.clone())));
        if let Some(ready) = ready.take() {
            let _ = ready.send(Err(error.clone()));
        }
        if !*stop.borrow() {
            tokio::select! { _ = stop.changed() => {}, _ = events.send(Err(error.clone())) => {} }
        }
    }
    // The canonical stream closes before waiting for native EOF: Claude remains interactive.
    drop(events);
    state.controls.lock().unwrap().clear();
    process.shutdown().await
}
