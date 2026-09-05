use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::domain::MutationState;

use super::process::{EVENT_CHANNEL_CAPACITY, JsonLineProcess, JsonLineSender, JsonLineShutdown};
use super::provider_command;
use super::{
    ApprovalRequestDetails, ApprovalResponse, FileChangeApprovalDetail, FileChangeKind,
    NativeAgentStatus, NativeChildStatus, NativeSubAgentActivityKind, ProviderAdapter,
    ProviderCapabilities, ProviderError, ProviderEvent, ProviderHealth, ProviderId,
    ProviderSession, ProviderTurn, ProviderTurnOwner, RequestedPermissionProfile, ResumeSession,
    StartSession, TurnRequest, UserInputOption, UserInputQuestion,
};
use crate::router::ProviderCapability;

const COMMAND_CAPACITY: usize = 128;
const MAX_PENDING_REQUESTS: usize = 128;
const MAX_SERVER_REQUESTS: usize = 128;
const MAX_FILE_CHANGE_ITEMS: usize = 128;
const MAX_INTERRUPT_WAITERS: usize = 128;
const CANCELLATION_CAPACITY: usize = 512;
const TOMBSTONE_CAPACITY: usize = 256;
const PROVISIONAL_EVENT_CAPACITY: usize = 128;
const REQUEST_QUEUED: u8 = 0;
const REQUEST_WRITING: u8 = 1;
const REQUEST_AWAITING_RESPONSE: u8 = 2;
const REQUEST_FINISHED: u8 = 3;
const REQUEST_CANCELLED: u8 = 4;
const FATAL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(1);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct CodexAdapter {
    inner: Arc<AdapterInner>,
}

struct AdapterInner {
    client: Client,
    shutdown: watch::Sender<bool>,
    process_shutdown: JsonLineShutdown,
    dispatcher: AsyncMutex<Option<JoinHandle<Result<(), ProviderError>>>>,
    version: Mutex<String>,
    alive: Arc<AtomicBool>,
}

impl Drop for AdapterInner {
    fn drop(&mut self) {
        self.shutdown.send_replace(true);
        self.process_shutdown.request();
    }
}

#[derive(Clone)]
struct Client {
    commands: mpsc::Sender<ClientCommand>,
    cancellations: mpsc::Sender<Cancellation>,
    next_request_key: Arc<AtomicU64>,
    process_shutdown: JsonLineShutdown,
}

struct CancelTurn {
    thread_id: String,
    turn_id: Option<String>,
    registration_id: Option<u64>,
}

enum Cancellation {
    Request(u64),
    Turn(CancelTurn),
    UndispatchedTurn {
        thread_id: String,
        registration_id: u64,
    },
}

enum ClientCommand {
    Request {
        request_key: u64,
        phase: Arc<AtomicU8>,
        kind: RequestKind,
        method: &'static str,
        params: Value,
        response: oneshot::Sender<Result<Value, ProviderError>>,
    },
    Notify {
        method: &'static str,
        params: Value,
        response: oneshot::Sender<Result<(), ProviderError>>,
    },
    RegisterTurn {
        thread_id: String,
        registration_id: u64,
        events: mpsc::Sender<Result<ProviderEvent, ProviderError>>,
        completed: Arc<AtomicBool>,
        response: oneshot::Sender<Result<(), ProviderError>>,
    },
    CancelTurn {
        thread_id: String,
        turn_id: Option<String>,
        registration_id: Option<u64>,
        response: Option<oneshot::Sender<Result<(), ProviderError>>>,
    },
    Respond {
        thread_id: String,
        request_id: String,
        response_value: ApprovalResponse,
        response: oneshot::Sender<Result<(), ProviderError>>,
    },
}

enum PendingResponse {
    Deliver {
        request_key: Option<u64>,
        kind: RequestKind,
        response: oneshot::Sender<Result<Value, ProviderError>>,
    },
    Fatal {
        confirmed: Arc<AtomicBool>,
        thread_id: String,
        turn_id: String,
    },
}

enum RequestKind {
    Ordinary,
    TurnStart {
        thread_id: String,
        registration_id: u64,
    },
    Interrupt {
        thread_id: String,
        turn_id: String,
        completed: Arc<AtomicBool>,
        confirmed: Arc<AtomicBool>,
    },
}

struct OutboundRequest {
    request_key: Option<u64>,
    phase: Arc<AtomicU8>,
    kind: RequestKind,
    method: &'static str,
    params: Value,
    response: oneshot::Sender<Result<Value, ProviderError>>,
}

struct TurnSink {
    registration_id: u64,
    events: mpsc::Sender<Result<ProviderEvent, ProviderError>>,
    completed: Arc<AtomicBool>,
    native_turn_id: Option<String>,
    announced_turn_id: Option<String>,
    provisional_events: VecDeque<Result<ProviderEvent, ProviderError>>,
    provisional_terminal: bool,
    cancelled: bool,
    cancellation_resolved: Arc<AtomicBool>,
    interrupt_pending: bool,
    interrupt_waiters: Vec<oneshot::Sender<Result<(), ProviderError>>>,
    file_changes: HashMap<String, Vec<FileChangeApprovalDetail>>,
}

enum ServerRequestKind {
    Approval,
    UserInput {
        question_ids: Vec<String>,
    },
    Permissions {
        requested: RequestedPermissionProfile,
    },
}

struct ServerRequest {
    id: RpcId,
    thread_id: String,
    turn_id: String,
    kind: ServerRequestKind,
}

struct DispatcherState {
    next_id: i64,
    pending: HashMap<RpcId, PendingResponse>,
    turns: HashMap<String, TurnSink>,
    server_requests: HashMap<String, ServerRequest>,
    client_response_tombstones: VecDeque<RpcId>,
    server_request_tombstones: VecDeque<RpcId>,
    confirmed_interrupts: VecDeque<(String, String, u64)>,
    process_shutdown: JsonLineShutdown,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(untagged)]
enum RpcId {
    String(String),
    Number(i64),
}

impl RpcId {
    fn external(&self) -> String {
        match self {
            Self::String(value) => format!("string:{value}"),
            Self::Number(value) => format!("number:{value}"),
        }
    }
}

impl CodexAdapter {
    pub async fn connect(binary: PathBuf) -> Result<Self, ProviderError> {
        Self::connect_with_initialization_timeout(binary, REQUEST_TIMEOUT).await
    }

    /// Starts and initializes Codex while retaining ownership through timeout cleanup.
    pub async fn connect_with_initialization_timeout(
        binary: PathBuf,
        initialization_timeout: Duration,
    ) -> Result<Self, ProviderError> {
        let mut command = provider_command(&binary);
        command.arg("app-server");
        let process = JsonLineProcess::spawn(command)?;
        let process_shutdown = process.shutdown_handle();
        let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (cancellations, cancellation_receiver) = mpsc::channel(CANCELLATION_CAPACITY);
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let client = Client {
            commands,
            cancellations,
            next_request_key: Arc::new(AtomicU64::new(0)),
            process_shutdown: process_shutdown.clone(),
        };
        let alive = Arc::new(AtomicBool::new(true));
        let dispatcher_alive = Arc::clone(&alive);
        let dispatcher = tokio::spawn(async move {
            let result = run_dispatcher(
                process,
                command_receiver,
                cancellation_receiver,
                shutdown_receiver,
            )
            .await;
            dispatcher_alive.store(false, Ordering::Release);
            result
        });
        let provisional = Self {
            inner: Arc::new(AdapterInner {
                client: client.clone(),
                shutdown,
                process_shutdown,
                dispatcher: AsyncMutex::new(Some(dispatcher)),
                version: Mutex::new(String::new()),
                alive,
            }),
        };

        let handshake = async {
            let initialized = client
                .request(
                    "initialize",
                    json!({
                        "clientInfo": {
                            "name": "prompting_time",
                            "title": "Prompting Time",
                            "version": env!("CARGO_PKG_VERSION"),
                        }
                    }),
                )
                .await?;
            client.notify("initialized", json!({})).await?;
            Ok::<Value, ProviderError>(initialized)
        };
        let initialized = match tokio::time::timeout(initialization_timeout, handshake).await {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => {
                let _ = provisional.stop().await;
                return Err(error);
            }
            Err(_) => {
                let _ = provisional.stop().await;
                return Err(ProviderError::Transport {
                    category: "codex-initialization-timeout".to_owned(),
                });
            }
        };
        let version = initialized
            .get("userAgent")
            .and_then(Value::as_str)
            .unwrap_or("codex app-server")
            .to_owned();
        *provisional
            .inner
            .version
            .lock()
            .expect("version mutex must not be poisoned") = version;
        Ok(provisional)
    }

    async fn stop(&self) -> Result<(), ProviderError> {
        self.inner.shutdown.send_replace(true);
        self.inner.process_shutdown.request();
        let mut dispatcher = self.inner.dispatcher.lock().await;
        let Some(task) = dispatcher.as_mut() else {
            return Ok(());
        };
        let result = task
            .await
            .map_err(|_| ProviderError::Transport {
                category: "codex-dispatcher-task".to_owned(),
            })
            .and_then(|result| result);
        dispatcher.take();
        result
    }

    /// Archive a session created only for an opt-in live integration probe.
    pub async fn archive_session(&self, session: &ProviderSession) -> Result<(), ProviderError> {
        require_codex_session(session)?;
        self.inner
            .client
            .request("thread/archive", json!({"threadId": session.native_id}))
            .await?;
        Ok(())
    }
}

impl Client {
    async fn request(&self, method: &'static str, params: Value) -> Result<Value, ProviderError> {
        self.request_with_kind(RequestKind::Ordinary, method, params)
            .await
    }

    async fn start_turn(
        &self,
        request_key: u64,
        phase: Arc<AtomicU8>,
        thread_id: String,
        params: Value,
    ) -> Result<Value, ProviderError> {
        self.request_prepared(
            request_key,
            phase,
            RequestKind::TurnStart {
                thread_id: thread_id.clone(),
                registration_id: request_key,
            },
            "turn/start",
            params,
        )
        .await
    }

    async fn request_with_kind(
        &self,
        kind: RequestKind,
        method: &'static str,
        params: Value,
    ) -> Result<Value, ProviderError> {
        let request_key = self.next_request_key.fetch_add(1, Ordering::Relaxed);
        let phase = Arc::new(AtomicU8::new(REQUEST_QUEUED));
        let mut guard = RequestCancellationGuard {
            client: self.clone(),
            request_key,
            phase: Arc::clone(&phase),
            armed: true,
        };
        let result = self
            .request_prepared(request_key, phase, kind, method, params)
            .await;
        guard.armed = guard.phase.load(Ordering::Acquire) != REQUEST_FINISHED;
        result
    }

    async fn request_prepared(
        &self,
        request_key: u64,
        phase: Arc<AtomicU8>,
        kind: RequestKind,
        method: &'static str,
        params: Value,
    ) -> Result<Value, ProviderError> {
        let (response, receiver) = oneshot::channel();
        self.commands
            .send(ClientCommand::Request {
                request_key,
                phase: Arc::clone(&phase),
                kind,
                method,
                params,
                response,
            })
            .await
            .map_err(|_| closed_transport())?;
        match tokio::time::timeout(REQUEST_TIMEOUT, receiver).await {
            Ok(result) => {
                phase.store(REQUEST_FINISHED, Ordering::Release);
                result.map_err(|_| closed_transport())?
            }
            Err(_) => Err(ProviderError::Transport {
                category: "request-timeout".to_owned(),
            }),
        }
    }

    async fn notify(&self, method: &'static str, params: Value) -> Result<(), ProviderError> {
        let (response, receiver) = oneshot::channel();
        self.commands
            .send(ClientCommand::Notify {
                method,
                params,
                response,
            })
            .await
            .map_err(|_| closed_transport())?;
        receiver.await.map_err(|_| closed_transport())?
    }

    async fn register_turn(
        &self,
        thread_id: String,
        registration_id: u64,
        events: mpsc::Sender<Result<ProviderEvent, ProviderError>>,
        completed: Arc<AtomicBool>,
    ) -> Result<(), ProviderError> {
        let (response, receiver) = oneshot::channel();
        self.commands
            .send(ClientCommand::RegisterTurn {
                thread_id,
                registration_id,
                events,
                completed,
                response,
            })
            .await
            .map_err(|_| closed_transport())?;
        receiver.await.map_err(|_| closed_transport())?
    }

    async fn cancel_turn(
        &self,
        thread_id: String,
        turn_id: Option<String>,
        registration_id: Option<u64>,
    ) -> Result<(), ProviderError> {
        let (response, receiver) = oneshot::channel();
        let result = tokio::time::timeout(REQUEST_TIMEOUT, async {
            self.commands
                .send(ClientCommand::CancelTurn {
                    thread_id,
                    turn_id,
                    registration_id,
                    response: Some(response),
                })
                .await
                .map_err(|_| closed_transport())?;
            receiver.await.map_err(|_| closed_transport())?
        })
        .await
        .unwrap_or_else(|_| {
            Err(ProviderError::Transport {
                category: "request-timeout".to_owned(),
            })
        });
        if result.as_ref().is_err_and(|error| {
            error.dispatch_certainty() != super::DispatchCertainty::NotDispatched
        }) {
            self.process_shutdown.request();
        }
        result
    }

    fn cancel(&self, cancellation: Cancellation) {
        if self.cancellations.try_send(cancellation).is_err() {
            self.process_shutdown.request();
        }
    }

    fn cancel_request(&self, request_key: u64, phase: &AtomicU8) -> u8 {
        let previous = phase.swap(REQUEST_CANCELLED, Ordering::AcqRel);
        match previous {
            REQUEST_QUEUED | REQUEST_FINISHED | REQUEST_CANCELLED => {}
            REQUEST_WRITING => self.process_shutdown.request(),
            REQUEST_AWAITING_RESPONSE => self.cancel(Cancellation::Request(request_key)),
            _ => self.process_shutdown.request(),
        }
        previous
    }
}

struct RequestCancellationGuard {
    client: Client,
    request_key: u64,
    phase: Arc<AtomicU8>,
    armed: bool,
}

impl Drop for RequestCancellationGuard {
    fn drop(&mut self) {
        if self.armed {
            self.client.cancel_request(self.request_key, &self.phase);
        }
    }
}

struct TurnRegistrationGuard {
    client: Client,
    thread_id: String,
    request_key: u64,
    request_phase: Arc<AtomicU8>,
    armed: bool,
}

impl Drop for TurnRegistrationGuard {
    fn drop(&mut self) {
        if self.armed {
            let previous = self
                .client
                .cancel_request(self.request_key, &self.request_phase);
            match turn_start_drop_action(previous) {
                TurnStartDropAction::Unregister => {
                    self.client.cancel(Cancellation::UndispatchedTurn {
                        thread_id: self.thread_id.clone(),
                        registration_id: self.request_key,
                    })
                }
                TurnStartDropAction::CancelActive => {
                    self.client.cancel(Cancellation::Turn(CancelTurn {
                        thread_id: self.thread_id.clone(),
                        turn_id: None,
                        registration_id: Some(self.request_key),
                    }));
                }
                TurnStartDropAction::Fatal => self.client.process_shutdown.request(),
                TurnStartDropAction::None => {}
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TurnStartDropAction {
    Unregister,
    CancelActive,
    Fatal,
    None,
}

fn turn_start_drop_action(previous_phase: u8) -> TurnStartDropAction {
    match previous_phase {
        REQUEST_QUEUED => TurnStartDropAction::Unregister,
        REQUEST_AWAITING_RESPONSE => TurnStartDropAction::CancelActive,
        REQUEST_WRITING => TurnStartDropAction::Fatal,
        REQUEST_FINISHED | REQUEST_CANCELLED => TurnStartDropAction::None,
        _ => TurnStartDropAction::Fatal,
    }
}

struct CodexTurnOwner {
    client: Client,
    thread_id: String,
    turn_id: String,
    registration_id: u64,
    completed: Arc<AtomicBool>,
}

struct FatalShutdownGuard {
    shutdown: JsonLineShutdown,
    armed: bool,
}

impl Drop for FatalShutdownGuard {
    fn drop(&mut self) {
        if self.armed {
            self.shutdown.request();
        }
    }
}

impl Drop for CodexTurnOwner {
    fn drop(&mut self) {
        if !self.completed.load(Ordering::Acquire) {
            self.client.cancel(Cancellation::Turn(CancelTurn {
                thread_id: self.thread_id.clone(),
                turn_id: Some(self.turn_id.clone()),
                registration_id: Some(self.registration_id),
            }));
        }
    }
}

#[async_trait]
impl ProviderTurnOwner for CodexTurnOwner {
    async fn shutdown(self: Box<Self>) -> Result<(), ProviderError> {
        if self.completed.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut failure = FatalShutdownGuard {
            shutdown: self.client.process_shutdown.clone(),
            armed: true,
        };
        let cancellation = self
            .client
            .cancel_turn(
                self.thread_id.clone(),
                Some(self.turn_id.clone()),
                Some(self.registration_id),
            )
            .await;
        match cancellation {
            Ok(()) => {}
            Err(ProviderError::NotDispatched { .. }) if self.completed.load(Ordering::Acquire) => {}
            Err(error) => return Err(error),
        }
        failure.armed = false;
        self.completed.store(true, Ordering::Release);
        Ok(())
    }
}

#[async_trait]
impl ProviderAdapter for CodexAdapter {
    fn id(&self) -> ProviderId {
        ProviderId::Codex
    }

    fn capabilities(&self) -> ProviderCapabilities {
        [
            ProviderCapability::Streaming,
            ProviderCapability::Steering,
            ProviderCapability::DeferredApproval,
            ProviderCapability::Interruption,
            ProviderCapability::Resume,
            ProviderCapability::ChildAgents,
        ]
        .into()
    }

    async fn health(&self) -> Result<ProviderHealth, ProviderError> {
        if !self.inner.alive.load(Ordering::Acquire) {
            return Ok(ProviderHealth::Unavailable {
                category: "codex-app-server-exited".to_owned(),
            });
        }
        Ok(ProviderHealth::Healthy {
            version: self
                .inner
                .version
                .lock()
                .expect("version mutex must not be poisoned")
                .clone(),
        })
    }

    async fn start_session(&self, request: StartSession) -> Result<ProviderSession, ProviderError> {
        let cwd = path_string(&request.working_directory)?;
        let result = self
            .inner
            .client
            .request(
                "thread/start",
                json!({
                    "cwd": cwd,
                    "approvalPolicy": "on-request",
                    "sandbox": "workspace-write",
                }),
            )
            .await?;
        parse_session(result)
    }

    async fn resume_session(
        &self,
        native_id: &str,
        request: ResumeSession,
    ) -> Result<ProviderSession, ProviderError> {
        let cwd = path_string(&request.working_directory)?;
        let result = self
            .inner
            .client
            .request(
                "thread/resume",
                json!({
                    "threadId": native_id,
                    "cwd": cwd,
                    "approvalPolicy": "on-request",
                    "sandbox": "workspace-write",
                }),
            )
            .await?;
        parse_session(result)
    }

    async fn start_turn(
        &self,
        session: &ProviderSession,
        request: TurnRequest,
    ) -> Result<ProviderTurn, ProviderError> {
        require_codex_session(session)?;
        let (events, receiver) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
        let completed = Arc::new(AtomicBool::new(false));
        let request_key = self
            .inner
            .client
            .next_request_key
            .fetch_add(1, Ordering::Relaxed);
        let request_phase = Arc::new(AtomicU8::new(REQUEST_QUEUED));
        let mut guard = TurnRegistrationGuard {
            client: self.inner.client.clone(),
            thread_id: session.native_id.clone(),
            request_key,
            request_phase: Arc::clone(&request_phase),
            armed: true,
        };
        self.inner
            .client
            .register_turn(
                session.native_id.clone(),
                request_key,
                events,
                Arc::clone(&completed),
            )
            .await?;
        let result = self
            .inner
            .client
            .start_turn(
                request_key,
                request_phase,
                session.native_id.clone(),
                json!({
                    "threadId": session.native_id,
                    "input": [{"type": "text", "text": request.prompt}],
                }),
            )
            .await?;
        let turn_id = required_string(&result, &["turn", "id"], "turn-start-id")?;
        guard.armed = false;
        Ok(ProviderTurn::new(
            receiver,
            CodexTurnOwner {
                client: self.inner.client.clone(),
                thread_id: session.native_id.clone(),
                turn_id,
                registration_id: request_key,
                completed,
            },
        ))
    }

    async fn steer(
        &self,
        session: &ProviderSession,
        active_turn: &str,
        text: &str,
    ) -> Result<(), ProviderError> {
        require_codex_session(session)?;
        self.inner
            .client
            .request(
                "turn/steer",
                json!({
                    "threadId": session.native_id,
                    "expectedTurnId": active_turn,
                    "input": [{"type": "text", "text": text}],
                }),
            )
            .await?;
        Ok(())
    }

    async fn respond(
        &self,
        session: &ProviderSession,
        request_id: &str,
        response_value: ApprovalResponse,
    ) -> Result<(), ProviderError> {
        require_codex_session(session)?;
        let (response, receiver) = oneshot::channel();
        self.inner
            .client
            .commands
            .send(ClientCommand::Respond {
                thread_id: session.native_id.clone(),
                request_id: request_id.to_owned(),
                response_value,
                response,
            })
            .await
            .map_err(|_| closed_transport())?;
        receiver.await.map_err(|_| closed_transport())?
    }

    async fn interrupt(
        &self,
        session: &ProviderSession,
        active_turn: &str,
    ) -> Result<(), ProviderError> {
        require_codex_session(session)?;
        self.inner
            .client
            .cancel_turn(
                session.native_id.clone(),
                Some(active_turn.to_owned()),
                None,
            )
            .await
    }

    async fn shutdown(&self) -> Result<(), ProviderError> {
        self.stop().await
    }

    fn force_shutdown(&self) {
        self.inner.shutdown.send_replace(true);
        self.inner.process_shutdown.request();
    }
}

async fn run_dispatcher(
    mut process: JsonLineProcess,
    mut commands: mpsc::Receiver<ClientCommand>,
    mut cancellations: mpsc::Receiver<Cancellation>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), ProviderError> {
    let sender = process.sender();
    let mut state = DispatcherState {
        next_id: 0,
        pending: HashMap::new(),
        turns: HashMap::new(),
        server_requests: HashMap::new(),
        client_response_tombstones: VecDeque::new(),
        server_request_tombstones: VecDeque::new(),
        confirmed_interrupts: VecDeque::new(),
        process_shutdown: process.shutdown_handle(),
    };
    let result = loop {
        tokio::select! {
            _ = shutdown.changed() => break Ok(()),
            cancellation = cancellations.recv() => {
                if let Some(cancellation) = cancellation {
                    match cancellation {
                        Cancellation::Request(request_key) => {
                            cancel_pending_request(request_key, &mut state);
                        }
                        Cancellation::Turn(cancellation) => {
                            cancel_registered_turn(
                                cancellation.thread_id,
                                cancellation.turn_id,
                                cancellation.registration_id,
                                None,
                                &sender,
                                &mut state,
                            ).await?;
                        }
                        Cancellation::UndispatchedTurn { thread_id, registration_id } => {
                            unregister_turn_registration(
                                &thread_id,
                                registration_id,
                                &mut state.turns,
                            );
                        }
                    }
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { break Ok(()); };
                if let Err(error) = handle_command(command, &sender, &mut state).await {
                    broadcast_error(&mut state.turns, error.clone()).await;
                    break Err(error);
                }
            }
            message = process.recv() => {
                match message {
                    Some(Ok(message)) => {
                        if let Err(error) = handle_server_message(message, &sender, &mut state).await {
                            broadcast_error(&mut state.turns, error.clone()).await;
                            break Err(error);
                        }
                    }
                    Some(Err(error)) => {
                        broadcast_error(&mut state.turns, error.clone()).await;
                        break Err(error);
                    }
                    None => {
                        let error = ProviderError::StreamClosed;
                        broadcast_error(&mut state.turns, error.clone()).await;
                        break Err(error);
                    }
                }
            }
        }
    };

    let closure_error = ProviderError::StreamClosed;
    for (_, pending) in state.pending {
        if let PendingResponse::Deliver { response, .. } = pending {
            let _ = response.send(Err(closure_error.clone()));
        }
    }
    let shutdown_result = process.shutdown().await;
    result.and(shutdown_result)
}

fn unregister_turn_registration(
    thread_id: &str,
    registration_id: u64,
    turns: &mut HashMap<String, TurnSink>,
) {
    let removable = turns.get(thread_id).is_some_and(|turn| {
        turn.registration_id == registration_id
            && turn.native_turn_id.is_none()
            && turn.announced_turn_id.is_none()
    });
    if removable {
        turns.remove(thread_id);
    }
}

async fn handle_command(
    command: ClientCommand,
    sender: &JsonLineSender,
    state: &mut DispatcherState,
) -> Result<(), ProviderError> {
    match command {
        ClientCommand::Request {
            request_key,
            phase,
            kind,
            method,
            params,
            response,
        } => {
            send_request(
                sender,
                state,
                OutboundRequest {
                    request_key: Some(request_key),
                    phase,
                    kind,
                    method,
                    params,
                    response,
                },
            )
            .await
        }
        ClientCommand::Notify {
            method,
            params,
            response,
        } => {
            let result = sender
                .send(&json!({"method": method, "params": params}))
                .await;
            let _ = response.send(result.clone());
            result
        }
        ClientCommand::RegisterTurn {
            thread_id,
            registration_id,
            events,
            completed,
            response,
        } => {
            let result = if response.is_closed() || events.is_closed() {
                Ok(())
            } else if state.turns.len() >= EVENT_CHANNEL_CAPACITY {
                Err(protocol("too-many-active-turns"))
            } else {
                match state.turns.entry(thread_id) {
                    std::collections::hash_map::Entry::Occupied(_) => {
                        Err(protocol("thread-already-active"))
                    }
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(TurnSink {
                            registration_id,
                            events,
                            completed,
                            native_turn_id: None,
                            announced_turn_id: None,
                            provisional_events: VecDeque::new(),
                            provisional_terminal: false,
                            cancelled: false,
                            cancellation_resolved: Arc::new(AtomicBool::new(false)),
                            interrupt_pending: false,
                            interrupt_waiters: Vec::new(),
                            file_changes: HashMap::new(),
                        });
                        Ok(())
                    }
                }
            };
            let _ = response.send(result);
            Ok(())
        }
        ClientCommand::CancelTurn {
            thread_id,
            turn_id,
            registration_id,
            response,
        } => {
            cancel_registered_turn(thread_id, turn_id, registration_id, response, sender, state)
                .await
        }
        ClientCommand::Respond {
            thread_id,
            request_id,
            response_value,
            response,
        } => {
            let result =
                respond_to_server_request(sender, &thread_id, &request_id, response_value, state)
                    .await;
            let _ = response.send(result.clone());
            result.or(Ok(()))
        }
    }
}

async fn cancel_registered_turn(
    thread_id: String,
    turn_id: Option<String>,
    registration_id: Option<u64>,
    response: Option<oneshot::Sender<Result<(), ProviderError>>>,
    sender: &JsonLineSender,
    state: &mut DispatcherState,
) -> Result<(), ProviderError> {
    if let Some(requested_turn_id) = turn_id.as_deref()
        && !state.turns.contains_key(&thread_id)
        && state.confirmed_interrupts.iter().any(
            |(confirmed_thread, confirmed_turn, confirmed_registration)| {
                confirmed_thread == &thread_id
                    && confirmed_turn == requested_turn_id
                    && registration_id.is_none_or(|id| id == *confirmed_registration)
            },
        )
    {
        if let Some(response) = response {
            let _ = response.send(Ok(()));
        }
        return Ok(());
    }
    if let Some(registration_id) = registration_id {
        let matches_registration = state
            .turns
            .get(&thread_id)
            .is_some_and(|turn| turn.registration_id == registration_id);
        if !matches_registration {
            if let Some(response) = response {
                let _ = response.send(Err(not_dispatched()));
            }
            return Ok(());
        }
    }
    if let Some(requested_turn_id) = turn_id.as_deref() {
        let matches_active = state
            .turns
            .get(&thread_id)
            .and_then(active_or_announced_turn_id)
            .is_some_and(|active_turn_id| active_turn_id == requested_turn_id);
        if !matches_active {
            if let Some(response) = response {
                let _ = response.send(Err(not_dispatched()));
            }
            return Ok(());
        }
    }
    let resolved_turn_id = turn_id.or_else(|| {
        state.turns.get(&thread_id).and_then(|turn| {
            turn.native_turn_id
                .clone()
                .or_else(|| turn.announced_turn_id.clone())
        })
    });
    if let Some(turn_id) = resolved_turn_id {
        let Some(turn) = state.turns.get_mut(&thread_id) else {
            if let Some(response) = response {
                let _ = response.send(Err(protocol("turn-not-active")));
            }
            return Ok(());
        };
        if turn.interrupt_pending {
            if let Some(response) = response {
                if turn.interrupt_waiters.len() >= MAX_INTERRUPT_WAITERS {
                    let _ = response.send(Err(not_dispatched()));
                } else {
                    turn.interrupt_waiters.push(response);
                }
            }
            return Ok(());
        }
        if response.as_ref().is_some_and(oneshot::Sender::is_closed) {
            return Ok(());
        }
        if response.is_some() && state.pending.len() >= MAX_PENDING_REQUESTS {
            if let Some(response) = response {
                let _ = response.send(Err(not_dispatched()));
            }
            return Ok(());
        }
        let completed = Arc::clone(&turn.completed);
        let confirmed = Arc::new(AtomicBool::new(false));
        let cancellation_resolved = Arc::clone(&turn.cancellation_resolved);
        if let Some(response) = response {
            send_request(
                sender,
                state,
                OutboundRequest {
                    request_key: None,
                    phase: Arc::new(AtomicU8::new(REQUEST_QUEUED)),
                    kind: RequestKind::Interrupt {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        completed,
                        confirmed,
                    },
                    method: "turn/interrupt",
                    params: json!({"threadId": thread_id, "turnId": turn_id}),
                    response: response.map(),
                },
            )
            .await?;
        } else {
            send_fatal_interrupt(sender, state, thread_id.clone(), turn_id.clone(), completed)
                .await?;
            cancellation_resolved.store(true, Ordering::Release);
        }
        let turn = state
            .turns
            .get_mut(&thread_id)
            .ok_or_else(|| protocol("turn-registration-missing"))?;
        if active_or_announced_turn_id(turn) != Some(&turn_id) {
            return Err(protocol("turn-owner-changed-during-interrupt"));
        }
        turn.cancelled = true;
        turn.interrupt_pending = true;
    } else if let Some(turn) = state.turns.get_mut(&thread_id) {
        if !turn.cancelled {
            turn.cancelled = true;
            let resolved = Arc::clone(&turn.cancellation_resolved);
            let shutdown = state.process_shutdown.clone();
            tokio::spawn(async move {
                tokio::time::sleep(FATAL_RESPONSE_TIMEOUT).await;
                if !resolved.load(Ordering::Acquire) {
                    shutdown.request();
                }
            });
        }
        if let Some(response) = response {
            let _ = response.send(Err(protocol("turn-id-not-announced")));
        }
    } else if let Some(response) = response {
        let _ = response.send(Err(protocol("turn-not-active")));
    }
    Ok(())
}

trait MapUnitResponse {
    fn map(self) -> oneshot::Sender<Result<Value, ProviderError>>;
}

impl MapUnitResponse for oneshot::Sender<Result<(), ProviderError>> {
    fn map(self) -> oneshot::Sender<Result<Value, ProviderError>> {
        let (sender, receiver) = oneshot::channel();
        tokio::spawn(async move {
            let result = receiver.await.unwrap_or_else(|_| Err(closed_transport()));
            let _ = self.send(result.map(|_| ()));
        });
        sender
    }
}

async fn send_request(
    sender: &JsonLineSender,
    state: &mut DispatcherState,
    request: OutboundRequest,
) -> Result<(), ProviderError> {
    let OutboundRequest {
        request_key,
        phase,
        kind,
        method,
        params,
        response,
    } = request;
    if request_key.is_some() && response.is_closed() {
        if let RequestKind::TurnStart {
            thread_id,
            registration_id,
        } = &kind
        {
            unregister_turn_registration(thread_id, *registration_id, &mut state.turns);
        }
        return Ok(());
    }
    if state.pending.len() >= MAX_PENDING_REQUESTS {
        reject_pending_capacity(&kind, response, &mut state.turns);
        return Ok(());
    }
    if phase
        .compare_exchange(
            REQUEST_QUEUED,
            REQUEST_WRITING,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_err()
    {
        return Ok(());
    }
    let id = RpcId::Number(state.next_id);
    state.next_id = state
        .next_id
        .checked_add(1)
        .ok_or_else(|| protocol("request-id-exhausted"))?;
    let request_message = json!({"method": method, "id": id, "params": params});
    sender.send(&request_message).await?;
    if phase
        .compare_exchange(
            REQUEST_WRITING,
            REQUEST_AWAITING_RESPONSE,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_err()
    {
        remember_client_response_tombstone(state, id);
        return Ok(());
    }
    let interrupt_deadline = match &kind {
        RequestKind::Interrupt {
            completed,
            confirmed,
            ..
        } => Some((
            Arc::clone(completed),
            Arc::clone(confirmed),
            Arc::clone(&phase),
            state.process_shutdown.clone(),
        )),
        _ => None,
    };
    state.pending.insert(
        id,
        PendingResponse::Deliver {
            request_key,
            kind,
            response,
        },
    );
    if let Some((completed, confirmed, phase, shutdown)) = interrupt_deadline {
        tokio::spawn(async move {
            tokio::time::sleep(FATAL_RESPONSE_TIMEOUT).await;
            if phase.load(Ordering::Acquire) == REQUEST_AWAITING_RESPONSE
                && !confirmed.load(Ordering::Acquire)
                && !completed.load(Ordering::Acquire)
            {
                shutdown.request();
            }
        });
    }
    Ok(())
}

fn reject_pending_capacity(
    kind: &RequestKind,
    response: oneshot::Sender<Result<Value, ProviderError>>,
    turns: &mut HashMap<String, TurnSink>,
) {
    if let RequestKind::TurnStart {
        thread_id,
        registration_id,
    } = kind
    {
        unregister_turn_registration(thread_id, *registration_id, turns);
    }
    let _ = response.send(Err(not_dispatched()));
}

async fn send_fatal_interrupt(
    sender: &JsonLineSender,
    state: &mut DispatcherState,
    thread_id: String,
    turn_id: String,
    completed: Arc<AtomicBool>,
) -> Result<(), ProviderError> {
    if state.pending.len() >= MAX_PENDING_REQUESTS {
        return Err(protocol("too-many-pending-requests"));
    }
    let id = RpcId::Number(state.next_id);
    state.next_id = state
        .next_id
        .checked_add(1)
        .ok_or_else(|| protocol("request-id-exhausted"))?;
    let confirmed = Arc::new(AtomicBool::new(false));
    state.pending.insert(
        id.clone(),
        PendingResponse::Fatal {
            confirmed: Arc::clone(&confirmed),
            thread_id: thread_id.clone(),
            turn_id: turn_id.clone(),
        },
    );
    let shutdown = state.process_shutdown.clone();
    let interrupt_sender = sender.clone();
    let interrupt_shutdown = shutdown.clone();
    let request = json!({
        "method": "turn/interrupt",
        "id": id,
        "params": {"threadId": thread_id, "turnId": turn_id},
    });
    tokio::spawn(async move {
        if interrupt_sender.send(&request).await.is_err() {
            interrupt_shutdown.request();
        }
    });
    tokio::spawn(async move {
        tokio::time::sleep(FATAL_RESPONSE_TIMEOUT).await;
        if !confirmed.load(Ordering::Acquire) && !completed.load(Ordering::Acquire) {
            shutdown.request();
        }
    });
    Ok(())
}

fn cancel_pending_request(request_key: u64, state: &mut DispatcherState) {
    let cancelled = state
        .pending
        .iter()
        .find_map(|(id, pending)| match pending {
            PendingResponse::Deliver {
                request_key: Some(candidate),
                ..
            } if *candidate == request_key => Some(id.clone()),
            _ => None,
        });
    if let Some(id) = cancelled {
        state.pending.remove(&id);
        remember_client_response_tombstone(state, id);
    }
}

fn remember_client_response_tombstone(state: &mut DispatcherState, id: RpcId) {
    if state.client_response_tombstones.len() == TOMBSTONE_CAPACITY {
        state.client_response_tombstones.pop_front();
    }
    state.client_response_tombstones.push_back(id);
}

fn remember_server_request_tombstone(state: &mut DispatcherState, id: RpcId) {
    if state.server_request_tombstones.len() == TOMBSTONE_CAPACITY {
        state.server_request_tombstones.pop_front();
    }
    state.server_request_tombstones.push_back(id);
}

async fn handle_server_message(
    message: Value,
    sender: &JsonLineSender,
    state: &mut DispatcherState,
) -> Result<(), ProviderError> {
    if let Some(raw_method) = message.get("method") {
        let Some(method) = raw_method.as_str() else {
            if message.get("id").is_some() {
                return send_invalid_request(sender, state).await;
            }
            return Err(protocol("invalid-server-method"));
        };
        let known_request = is_known_server_request(method);
        if known_request || message.get("id").is_some() {
            let Some(id) = message.get("id").and_then(parse_rpc_id) else {
                return send_invalid_request(sender, state).await;
            };
            if state
                .server_request_tombstones
                .iter()
                .any(|tombstone| tombstone == &id)
            {
                return Err(protocol("duplicate-server-request-after-response"));
            }
            return handle_server_request(message, id, sender, state).await;
        }
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        return handle_notification(method, params, sender, state).await;
    }

    let id = message
        .get("id")
        .and_then(parse_rpc_id)
        .ok_or_else(|| protocol("malformed-response-id"))?;
    handle_server_response(message, id, sender, state).await
}

fn is_known_server_request(method: &str) -> bool {
    matches!(
        method,
        "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/tool/requestUserInput"
            | "item/permissions/requestApproval"
    )
}

async fn send_invalid_request(
    sender: &JsonLineSender,
    state: &DispatcherState,
) -> Result<(), ProviderError> {
    required_write(
        sender,
        &state.process_shutdown,
        json!({
            "id": Value::Null,
            "error": {"code": -32600, "message": "Invalid request"},
        }),
    )
    .await
}

async fn handle_server_response(
    message: Value,
    id: RpcId,
    sender: &JsonLineSender,
    state: &mut DispatcherState,
) -> Result<(), ProviderError> {
    let Some(pending_response) = state.pending.remove(&id) else {
        if let Some(position) = state
            .client_response_tombstones
            .iter()
            .position(|tombstone| tombstone == &id)
        {
            state.client_response_tombstones.remove(position);
        }
        return Ok(());
    };
    let PendingResponse::Deliver {
        kind,
        response: deliver,
        ..
    } = pending_response
    else {
        let PendingResponse::Fatal {
            confirmed,
            thread_id,
            turn_id,
        } = pending_response
        else {
            unreachable!("pending response variants are exhaustive")
        };
        confirmed.store(true, Ordering::Release);
        parse_response(message)?;
        confirm_interrupted_turn(sender, state, &thread_id, &turn_id).await?;
        return Ok(());
    };
    let response = parse_response(message);
    match (kind, response) {
        (
            RequestKind::TurnStart {
                thread_id,
                registration_id,
            },
            Ok(result),
        ) => {
            if let Err(error) =
                activate_turn(&thread_id, registration_id, &result, sender, state).await
            {
                let _ = deliver.send(Err(error.clone()));
                return Err(error);
            }
            if deliver.send(Ok(result)).is_err() {
                cancel_registered_turn(thread_id, None, Some(registration_id), None, sender, state)
                    .await?;
            }
        }
        (
            RequestKind::TurnStart {
                thread_id,
                registration_id,
            },
            Err(error),
        ) => {
            let announced = state
                .turns
                .get(&thread_id)
                .is_some_and(|turn| turn.announced_turn_id.is_some());
            let _ = deliver.send(Err(error.clone()));
            if announced {
                return Err(error);
            }
            unregister_turn_registration(&thread_id, registration_id, &mut state.turns);
        }
        (
            RequestKind::Interrupt {
                thread_id,
                turn_id,
                confirmed,
                ..
            },
            Ok(result),
        ) => {
            confirmed.store(true, Ordering::Release);
            match confirm_interrupted_turn(sender, state, &thread_id, &turn_id).await {
                Ok(()) => {
                    let _ = deliver.send(Ok(result));
                }
                Err(error) => {
                    resolve_interrupt_waiters(state, &thread_id, &turn_id, Err(error.clone()));
                    let _ = deliver.send(Err(error.clone()));
                    return Err(error);
                }
            }
        }
        (
            RequestKind::Interrupt {
                thread_id, turn_id, ..
            },
            Err(error),
        ) => {
            resolve_interrupt_waiters(state, &thread_id, &turn_id, Err(error.clone()));
            let _ = deliver.send(Err(error.clone()));
            return Err(error);
        }
        (RequestKind::Ordinary, response) => {
            let _ = deliver.send(response);
        }
    }
    Ok(())
}

async fn confirm_interrupted_turn(
    sender: &JsonLineSender,
    state: &mut DispatcherState,
    thread_id: &str,
    turn_id: &str,
) -> Result<(), ProviderError> {
    let matches = state
        .turns
        .get(thread_id)
        .and_then(active_or_announced_turn_id)
        .is_some_and(|active_turn_id| active_turn_id == turn_id);
    if matches {
        reject_server_requests(sender, state, thread_id, turn_id, "Turn was cancelled").await?;
    }
    if matches && let Some(mut turn) = state.turns.remove(thread_id) {
        remember_confirmed_interrupt(state, thread_id, turn_id, turn.registration_id);
        let _ = turn.events.try_send(Ok(ProviderEvent::Interrupted));
        turn.completed.store(true, Ordering::Release);
        for waiter in turn.interrupt_waiters.drain(..) {
            let _ = waiter.send(Ok(()));
        }
    }
    Ok(())
}

fn resolve_interrupt_waiters(
    state: &mut DispatcherState,
    thread_id: &str,
    turn_id: &str,
    result: Result<(), ProviderError>,
) {
    if let Some(turn) = state.turns.get_mut(thread_id)
        && active_or_announced_turn_id(turn) == Some(turn_id)
    {
        for waiter in turn.interrupt_waiters.drain(..) {
            let _ = waiter.send(result.clone());
        }
    }
}

fn remember_confirmed_interrupt(
    state: &mut DispatcherState,
    thread_id: &str,
    turn_id: &str,
    registration_id: u64,
) {
    if state.confirmed_interrupts.len() == TOMBSTONE_CAPACITY {
        state.confirmed_interrupts.pop_front();
    }
    state.confirmed_interrupts.push_back((
        thread_id.to_owned(),
        turn_id.to_owned(),
        registration_id,
    ));
}

fn confirm_terminal_interrupt(
    state: &mut DispatcherState,
    thread_id: &str,
    turn_id: &str,
) -> Option<oneshot::Sender<Result<Value, ProviderError>>> {
    let response_id = state.pending.iter().find_map(|(id, pending)| {
        let matches = match pending {
            PendingResponse::Deliver {
                kind:
                    RequestKind::Interrupt {
                        thread_id: pending_thread,
                        turn_id: pending_turn,
                        ..
                    },
                ..
            }
            | PendingResponse::Fatal {
                thread_id: pending_thread,
                turn_id: pending_turn,
                ..
            } => pending_thread == thread_id && pending_turn == turn_id,
            _ => false,
        };
        matches.then(|| id.clone())
    });
    let interrupt_was_pending = response_id.is_some();
    let mut primary_response = None;
    if let Some(id) = response_id
        && let Some(pending) = state.pending.remove(&id)
    {
        match pending {
            PendingResponse::Deliver {
                kind: RequestKind::Interrupt { confirmed, .. },
                response,
                ..
            } => {
                confirmed.store(true, Ordering::Release);
                primary_response = Some(response);
            }
            PendingResponse::Fatal { confirmed, .. } => {
                confirmed.store(true, Ordering::Release);
            }
            PendingResponse::Deliver { .. } => {
                unreachable!("terminal interrupt lookup returned a non-interrupt request")
            }
        }
        remember_client_response_tombstone(state, id);
    }
    if interrupt_was_pending
        && let Some(registration_id) = state.turns.get(thread_id).map(|turn| turn.registration_id)
    {
        remember_confirmed_interrupt(state, thread_id, turn_id, registration_id);
    }
    primary_response
}

async fn activate_turn(
    thread_id: &str,
    registration_id: u64,
    result: &Value,
    sender: &JsonLineSender,
    state: &mut DispatcherState,
) -> Result<(), ProviderError> {
    let turn_id = required_string(result, &["turn", "id"], "turn-start-id")?;
    let (cancelled, announced_turn_id, mut buffered, provisional_terminal) = {
        let turn = state
            .turns
            .get_mut(thread_id)
            .ok_or_else(|| protocol("turn-registration-missing"))?;
        if turn.registration_id != registration_id {
            return Err(protocol("turn-registration-mismatch"));
        }
        if turn
            .announced_turn_id
            .as_deref()
            .is_some_and(|announced| announced != turn_id)
        {
            return Err(protocol("turn-start-id-mismatch"));
        }
        turn.native_turn_id = Some(turn_id.clone());
        (
            turn.cancelled,
            turn.announced_turn_id.clone(),
            std::mem::take(&mut turn.provisional_events),
            turn.provisional_terminal,
        )
    };
    if cancelled {
        cancel_registered_turn(
            thread_id.to_owned(),
            Some(turn_id),
            None,
            None,
            sender,
            state,
        )
        .await?;
        return Err(protocol("turn-start-cancelled"));
    }
    deliver_to_turn(
        state,
        thread_id,
        Ok(ProviderEvent::TurnStarted {
            native_turn_id: turn_id.clone(),
        }),
        sender,
    )
    .await?;
    if provisional_terminal {
        let rejected_ids = state
            .server_requests
            .iter()
            .filter(|(_, request)| request.thread_id == thread_id && request.turn_id == turn_id)
            .map(|(request_id, _)| request_id.clone())
            .collect::<Vec<_>>();
        reject_server_requests(
            sender,
            state,
            thread_id,
            announced_turn_id.as_deref().unwrap_or(&turn_id),
            "Turn completed before a response",
        )
        .await?;
        buffered.retain(|event| {
            let request_id = match event {
                Ok(ProviderEvent::ApprovalRequested { request_id, .. })
                | Ok(ProviderEvent::UserInputRequested { request_id, .. }) => Some(request_id),
                _ => None,
            };
            !request_id.is_some_and(|request_id| rejected_ids.contains(request_id))
        });
    }
    for event in buffered {
        deliver_to_turn(state, thread_id, event, sender).await?;
    }
    if provisional_terminal && let Some(turn) = state.turns.remove(thread_id) {
        turn.completed.store(true, Ordering::Release);
    }
    Ok(())
}

fn parse_response(message: Value) -> Result<Value, ProviderError> {
    if let Some(result) = message.get("result") {
        return Ok(result.clone());
    }
    let error = message
        .get("error")
        .ok_or_else(|| protocol("response-without-result"))?;
    let code = error
        .get("code")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("provider request failed");
    let message = message.chars().take(512).collect::<String>();
    Err(ProviderError::Protocol {
        category: format!("rpc-{code}-{message}"),
    })
}

async fn handle_server_request(
    message: Value,
    id: RpcId,
    sender: &JsonLineSender,
    state: &mut DispatcherState,
) -> Result<(), ProviderError> {
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
    let external_id = id.external();
    if state.server_requests.remove(&external_id).is_some() {
        required_write(
            sender,
            &state.process_shutdown,
            duplicate_server_request_response(id.clone()),
        )
        .await?;
        remember_server_request_tombstone(state, id);
        return Err(protocol("duplicate-server-request-id"));
    }
    if !is_known_server_request(method) {
        required_write(
            sender,
            &state.process_shutdown,
            json!({
                "id": id,
                "error": {"code": -32601, "message": "Unsupported server request"},
            }),
        )
        .await?;
        remember_server_request_tombstone(state, id.clone());
        if let (Ok(thread_id), Ok(turn_id)) = (
            required_string(&params, &["threadId"], "server-request-thread-id"),
            required_string(&params, &["turnId"], "server-request-turn-id"),
        ) && state
            .turns
            .get(&thread_id)
            .and_then(active_or_announced_turn_id)
            .is_some_and(|active_turn_id| active_turn_id == turn_id)
        {
            deliver_to_turn(
                state,
                &thread_id,
                Err(protocol("unsupported-server-request")),
                sender,
            )
            .await?;
        }
        return Ok(());
    }
    let ownership =
        required_string(&params, &["threadId"], "server-request-thread-id").and_then(|thread_id| {
            required_string(&params, &["turnId"], "server-request-turn-id")
                .map(|turn_id| (thread_id, turn_id))
        });
    let Ok((thread_id, turn_id)) = ownership else {
        required_write(
            sender,
            &state.process_shutdown,
            json!({
                "id": id,
                "error": {"code": -32602, "message": "Missing request owner"},
            }),
        )
        .await?;
        remember_server_request_tombstone(state, id);
        return Ok(());
    };
    let active_owner = state
        .turns
        .get(&thread_id)
        .and_then(active_or_announced_turn_id)
        .is_some_and(|active_turn_id| active_turn_id == turn_id);
    if !active_owner {
        required_write(
            sender,
            &state.process_shutdown,
            json!({
                "id": id,
                "error": {"code": -32003, "message": "Request owner is not active"},
            }),
        )
        .await?;
        remember_server_request_tombstone(state, id);
        return Ok(());
    }
    if state
        .turns
        .get(&thread_id)
        .is_some_and(|turn| turn.cancelled || turn.interrupt_pending)
    {
        required_write(
            sender,
            &state.process_shutdown,
            json!({
                "id": id,
                "error": {"code": -32004, "message": "Turn is being cancelled"},
            }),
        )
        .await?;
        remember_server_request_tombstone(state, id);
        return Ok(());
    }
    if state
        .turns
        .get(&thread_id)
        .is_some_and(|turn| turn.provisional_terminal)
    {
        required_write(
            sender,
            &state.process_shutdown,
            json!({
                "id": id,
                "error": {"code": -32004, "message": "Turn already completed"},
            }),
        )
        .await?;
        remember_server_request_tombstone(state, id);
        return Ok(());
    }
    if state.server_requests.len() >= MAX_SERVER_REQUESTS {
        required_write(
            sender,
            &state.process_shutdown,
            json!({
                "id": id,
                "error": {"code": -32001, "message": "Client request capacity exceeded"},
            }),
        )
        .await?;
        remember_server_request_tombstone(state, id);
        deliver_to_turn(
            state,
            &thread_id,
            Err(protocol("excess-server-request")),
            sender,
        )
        .await?;
        return Ok(());
    }
    let parsed =
        parse_supported_server_request(method, &params, &external_id, state.turns.get(&thread_id));
    let Ok((kind, event)) = parsed else {
        required_write(
            sender,
            &state.process_shutdown,
            json!({
                "id": id,
                "error": {"code": -32602, "message": "Invalid request payload"},
            }),
        )
        .await?;
        remember_server_request_tombstone(state, id);
        deliver_to_turn(
            state,
            &thread_id,
            Err(protocol("invalid-server-request-payload")),
            sender,
        )
        .await?;
        return Ok(());
    };
    if state.turns.get(&thread_id).is_some_and(|turn| {
        turn.native_turn_id.is_none() && turn.provisional_events.len() >= PROVISIONAL_EVENT_CAPACITY
    }) {
        required_write(
            sender,
            &state.process_shutdown,
            json!({
                "id": id,
                "error": {"code": -32001, "message": "Turn event capacity exceeded"},
            }),
        )
        .await?;
        remember_server_request_tombstone(state, id);
        return Ok(());
    }
    state.server_requests.insert(
        external_id.clone(),
        ServerRequest {
            id,
            thread_id: thread_id.clone(),
            turn_id,
            kind,
        },
    );
    deliver_or_buffer_turn_event(state, &thread_id, Ok(event), false, sender).await?;
    Ok(())
}

fn duplicate_server_request_response(id: RpcId) -> Value {
    json!({
        "id": id,
        "error": {"code": -32600, "message": "Duplicate server request ID"},
    })
}

fn parse_supported_server_request(
    method: &str,
    params: &Value,
    external_id: &str,
    turn: Option<&TurnSink>,
) -> Result<(ServerRequestKind, ProviderEvent), ProviderError> {
    match method {
        "item/commandExecution/requestApproval" => {
            required_string(params, &["itemId"], "command-approval-item-id")?;
            required_i64(params, "startedAtMs", "command-approval-started-at")?;
            Ok((
                ServerRequestKind::Approval,
                ProviderEvent::ApprovalRequested {
                    request_id: external_id.to_owned(),
                    operation: "command execution".to_owned(),
                    scope: approval_scope(params, "command execution"),
                    details: Some(ApprovalRequestDetails::CommandExecution {
                        command: optional_string(params, "command")?,
                        cwd: optional_string(params, "cwd")?,
                    }),
                },
            ))
        }
        "item/fileChange/requestApproval" => {
            let item_id = required_string(params, &["itemId"], "file-change-item-id")?;
            required_i64(params, "startedAtMs", "file-change-approval-started-at")?;
            let changes = turn
                .and_then(|turn| turn.file_changes.get(&item_id))
                .cloned()
                .ok_or_else(|| protocol("file-change-item-not-observed"))?;
            Ok((
                ServerRequestKind::Approval,
                ProviderEvent::ApprovalRequested {
                    request_id: external_id.to_owned(),
                    operation: "file change".to_owned(),
                    scope: approval_scope(params, "file change"),
                    details: Some(ApprovalRequestDetails::FileChange {
                        changes,
                        grant_root: optional_string(params, "grantRoot")?,
                        reason: optional_string(params, "reason")?,
                    }),
                },
            ))
        }
        "item/tool/requestUserInput" => {
            required_string(params, &["itemId"], "user-input-item-id")?;
            parse_user_input(params).map(|(questions, auto_resolution_ms)| {
                let question_ids = questions
                    .iter()
                    .map(|question| question.id.clone())
                    .collect();
                (
                    ServerRequestKind::UserInput { question_ids },
                    ProviderEvent::UserInputRequested {
                        request_id: external_id.to_owned(),
                        questions,
                        auto_resolution_ms,
                    },
                )
            })
        }
        "item/permissions/requestApproval" => {
            required_string(params, &["itemId"], "permission-item-id")?;
            required_i64(params, "startedAtMs", "permission-started-at")?;
            let raw_requested = params
                .get("permissions")
                .cloned()
                .ok_or_else(|| protocol("permission-profile"))?;
            let requested = serde_json::from_value::<RequestedPermissionProfile>(raw_requested)
                .map_err(|_| protocol("permission-profile"))?;
            if requested
                .file_system
                .as_ref()
                .and_then(|file_system| file_system.glob_scan_max_depth)
                == Some(0)
            {
                return Err(protocol("permission-glob-scan-max-depth"));
            }
            Ok((
                ServerRequestKind::Permissions {
                    requested: requested.clone(),
                },
                ProviderEvent::ApprovalRequested {
                    request_id: external_id.to_owned(),
                    operation: "permission request".to_owned(),
                    scope: approval_scope(params, "permission request"),
                    details: Some(ApprovalRequestDetails::PermissionProfile {
                        cwd: required_string(params, &["cwd"], "permission-cwd")?,
                        profile: requested,
                    }),
                },
            ))
        }
        _ => Err(protocol("unsupported-server-request")),
    }
}

fn active_or_announced_turn_id(turn: &TurnSink) -> Option<&str> {
    turn.native_turn_id
        .as_deref()
        .or(turn.announced_turn_id.as_deref())
}

async fn deliver_or_buffer_turn_event(
    state: &mut DispatcherState,
    thread_id: &str,
    event: Result<ProviderEvent, ProviderError>,
    terminal: bool,
    sender: &JsonLineSender,
) -> Result<(), ProviderError> {
    if state
        .turns
        .get(thread_id)
        .is_some_and(|turn| turn.native_turn_id.is_none())
    {
        let turn = state
            .turns
            .get_mut(thread_id)
            .expect("provisional turn was checked above");
        if turn.provisional_events.len() == PROVISIONAL_EVENT_CAPACITY {
            return Err(protocol("provisional-event-capacity-exceeded"));
        }
        turn.provisional_events.push_back(event);
        turn.provisional_terminal |= terminal;
        return Ok(());
    }
    deliver_to_turn(state, thread_id, event, sender).await
}

fn parse_user_input(
    params: &Value,
) -> Result<(Vec<UserInputQuestion>, Option<u64>), ProviderError> {
    let raw_questions = params
        .get("questions")
        .and_then(Value::as_array)
        .filter(|questions| !questions.is_empty())
        .ok_or_else(|| protocol("user-input-questions"))?;
    let mut questions = Vec::with_capacity(raw_questions.len());
    for raw in raw_questions {
        let id = required_string(raw, &["id"], "user-input-question-id")?;
        if questions
            .iter()
            .any(|question: &UserInputQuestion| question.id == id)
        {
            return Err(protocol("duplicate-user-input-question-id"));
        }
        let options = match raw.get("options") {
            None | Some(Value::Null) => None,
            Some(Value::Array(options)) => Some(
                options
                    .iter()
                    .map(|option| {
                        Ok(UserInputOption {
                            label: required_string(option, &["label"], "user-input-option-label")?,
                            description: required_string(
                                option,
                                &["description"],
                                "user-input-option-description",
                            )?,
                        })
                    })
                    .collect::<Result<Vec<_>, ProviderError>>()?,
            ),
            Some(_) => return Err(protocol("user-input-options")),
        };
        let boolean = |field, default| match raw.get(field) {
            None => Ok(default),
            Some(value) => value.as_bool().ok_or_else(|| protocol("user-input-flag")),
        };
        questions.push(UserInputQuestion {
            id,
            header: required_string(raw, &["header"], "user-input-question-header")?,
            question: required_string(raw, &["question"], "user-input-question-text")?,
            options,
            is_other: boolean("isOther", false)?,
            is_secret: boolean("isSecret", false)?,
        });
    }
    let auto_resolution_ms = match params.get("autoResolutionMs") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .ok_or_else(|| protocol("user-input-auto-resolution"))?,
        ),
    };
    Ok((questions, auto_resolution_ms))
}

async fn handle_notification(
    method: &str,
    params: Value,
    sender: &JsonLineSender,
    state: &mut DispatcherState,
) -> Result<(), ProviderError> {
    let recognized = matches!(
        method,
        "turn/started"
            | "turn/completed"
            | "item/agentMessage/delta"
            | "item/started"
            | "item/completed"
            | "item/fileChange/patchUpdated"
    );
    let thread_id = match required_string(&params, &["threadId"], "notification-thread-id") {
        Ok(thread_id) => thread_id,
        Err(error) if recognized => return Err(error),
        Err(_) => return Ok(()),
    };
    if method == "turn/started" {
        let turn_id = required_string(&params, &["turn", "id"], "turn-started-id")?;
        let mut cancelled = false;
        if let Some(turn) = state.turns.get_mut(&thread_id)
            && turn.native_turn_id.is_none()
        {
            if turn
                .announced_turn_id
                .as_deref()
                .is_some_and(|announced| announced != turn_id)
            {
                return Err(protocol("turn-started-id-mismatch"));
            }
            if turn.provisional_terminal {
                return Ok(());
            }
            turn.announced_turn_id = Some(turn_id.clone());
            cancelled = turn.cancelled;
        }
        if cancelled {
            cancel_registered_turn(thread_id, Some(turn_id), None, None, sender, state).await?;
        }
        return Ok(());
    }
    let turn_id = match method {
        "turn/completed" => required_string(&params, &["turn", "id"], "turn-completed-id"),
        _ => required_string(&params, &["turnId"], "notification-turn-id"),
    };
    let turn_id = match turn_id {
        Ok(turn_id) => turn_id,
        Err(error) if recognized => return Err(error),
        Err(_) => return Ok(()),
    };
    if !state
        .turns
        .get(&thread_id)
        .and_then(active_or_announced_turn_id)
        .is_some_and(|active_turn_id| active_turn_id == turn_id)
    {
        return Ok(());
    }
    if state
        .turns
        .get(&thread_id)
        .is_some_and(|turn| turn.provisional_terminal)
    {
        return Ok(());
    }
    if method == "item/fileChange/patchUpdated" {
        let item_id = required_string(&params, &["itemId"], "file-change-item-id")?;
        let changes = parse_file_changes(
            params
                .get("changes")
                .ok_or_else(|| protocol("file-change-details"))?,
        )?;
        record_file_changes(state, &thread_id, item_id, changes)?;
        return Ok(());
    }
    if matches!(method, "item/started" | "item/completed")
        && params
            .get("item")
            .and_then(|item| item.get("type"))
            .and_then(Value::as_str)
            == Some("fileChange")
    {
        let item = params.get("item").expect("file-change item was checked");
        let item_id = required_string(item, &["id"], "file-change-item-id")?;
        let changes = parse_file_changes(
            item.get("changes")
                .ok_or_else(|| protocol("file-change-details"))?,
        )?;
        record_file_changes(state, &thread_id, item_id, changes)?;
    }
    let event = match method {
        "item/agentMessage/delta" => {
            let content = required_string(&params, &["delta"], "agent-message-delta");
            let native_item_id = required_string(&params, &["itemId"], "agent-message-item-id");
            content.and_then(|content| {
                native_item_id.map(|native_item_id| ProviderEvent::AssistantMessageDelta {
                    native_item_id,
                    content,
                })
            })
        }
        "item/started" | "item/completed" => match normalize_item(&params) {
            Ok(Some(event)) => Ok(event),
            Ok(None) => return Ok(()),
            Err(error) => Err(error),
        },
        "turn/completed" => match params
            .get("turn")
            .and_then(|turn| turn.get("status"))
            .and_then(Value::as_str)
        {
            Some("completed") => Ok(ProviderEvent::TurnCompleted),
            Some("interrupted") => Ok(ProviderEvent::Interrupted),
            Some("failed") => Err(protocol("turn-failed")),
            _ => Err(protocol("invalid-turn-completion-status")),
        },
        _ => Ok(ProviderEvent::Unrecognized {
            method: method.to_owned(),
        }),
    };

    let terminal = method == "turn/completed";
    let provisional = state
        .turns
        .get(&thread_id)
        .is_some_and(|turn| turn.native_turn_id.is_none());
    if provisional {
        let turn = state
            .turns
            .get_mut(&thread_id)
            .expect("provisional turn was checked above");
        if turn.provisional_events.len() == PROVISIONAL_EVENT_CAPACITY {
            return Err(protocol("provisional-event-capacity-exceeded"));
        }
        turn.provisional_events.push_back(event);
        turn.provisional_terminal |= terminal;
        return Ok(());
    }
    if terminal {
        let primary_interrupt = confirm_terminal_interrupt(state, &thread_id, &turn_id);
        if let Err(error) = reject_server_requests(
            sender,
            state,
            &thread_id,
            &turn_id,
            "Turn completed before a response",
        )
        .await
        {
            resolve_interrupt_waiters(state, &thread_id, &turn_id, Err(error.clone()));
            if let Some(response) = primary_interrupt {
                let _ = response.send(Err(error.clone()));
            }
            return Err(error);
        }
        send_to_turn(&mut state.turns, &thread_id, event);
        if let Some(mut turn) = state.turns.remove(&thread_id) {
            turn.completed.store(true, Ordering::Release);
            for waiter in turn.interrupt_waiters.drain(..) {
                let _ = waiter.send(Ok(()));
            }
        }
        if let Some(response) = primary_interrupt {
            let _ = response.send(Ok(json!({})));
        }
        return Ok(());
    }
    deliver_to_turn(state, &thread_id, event, sender).await?;
    let cancelled = state
        .turns
        .get(&thread_id)
        .is_some_and(|turn| turn.cancelled);
    if cancelled
        && let Some(turn_id) = state
            .turns
            .get(&thread_id)
            .and_then(|turn| turn.native_turn_id.clone())
    {
        cancel_registered_turn(thread_id, Some(turn_id), None, None, sender, state).await?;
    }
    Ok(())
}

fn normalize_item(params: &Value) -> Result<Option<ProviderEvent>, ProviderError> {
    let item = params.get("item").ok_or_else(|| protocol("item-missing"))?;
    let native_item_id = required_string(item, &["id"], "item-id")?;
    let item_type = required_string(item, &["type"], "item-type")?;
    if item_type == "collabAgentToolCall" {
        let parent_native_thread_id =
            required_string(item, &["senderThreadId"], "child-parent-id")?;
        let outer_thread_id = required_string(params, &["threadId"], "notification-thread-id")?;
        if parent_native_thread_id != outer_thread_id {
            return Err(protocol("child-parent-owner-mismatch"));
        }
        let operation = required_string(item, &["tool"], "child-tool")?;
        let status = required_string(item, &["status"], "child-status")?;
        let raw_child_ids = item
            .get("receiverThreadIds")
            .and_then(Value::as_array)
            .ok_or_else(|| protocol("child-thread-ids"))?;
        let mut child_native_thread_ids = Vec::with_capacity(raw_child_ids.len());
        for raw_child_id in raw_child_ids {
            let child_id = raw_child_id
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| protocol("child-thread-id"))?;
            if child_native_thread_ids.iter().any(|id| id == child_id) {
                return Err(protocol("duplicate-child-thread-id"));
            }
            child_native_thread_ids.push(child_id.to_owned());
        }
        let raw_states = item
            .get("agentsStates")
            .and_then(Value::as_object)
            .ok_or_else(|| protocol("child-agent-states"))?;
        let mut child_statuses = Vec::with_capacity(raw_states.len());
        for (native_thread_id, state) in raw_states {
            if native_thread_id.is_empty() || !state.is_object() {
                return Err(protocol("child-agent-state"));
            }
            if !child_native_thread_ids
                .iter()
                .any(|receiver| receiver == native_thread_id)
            {
                return Err(protocol("child-agent-state-not-receiver"));
            }
            let status = required_string(state, &["status"], "child-agent-status")?;
            child_statuses.push(NativeChildStatus {
                native_thread_id: native_thread_id.clone(),
                status: parse_agent_status(&status),
            });
        }
        child_statuses.sort_by(|left, right| left.native_thread_id.cmp(&right.native_thread_id));
        return Ok(Some(ProviderEvent::ChildAgentActivity {
            native_item_id,
            parent_native_thread_id,
            child_native_thread_ids,
            child_statuses,
            operation,
            status,
        }));
    }
    if item_type == "subAgentActivity" {
        let activity = match required_string(item, &["kind"], "subagent-activity-kind")?.as_str() {
            "started" => NativeSubAgentActivityKind::Started,
            "interacted" => NativeSubAgentActivityKind::Interacted,
            "interrupted" => NativeSubAgentActivityKind::Interrupted,
            _ => return Err(protocol("subagent-activity-kind")),
        };
        return Ok(Some(ProviderEvent::SubAgentActivity {
            native_item_id,
            agent_thread_id: required_string(
                item,
                &["agentThreadId"],
                "subagent-activity-thread-id",
            )?,
            agent_path: required_string(item, &["agentPath"], "subagent-activity-path")?,
            activity,
        }));
    }
    if matches!(item_type.as_str(), "agentMessage" | "userMessage") {
        return Ok(None);
    }
    let mutation = match item_type.as_str() {
        "fileChange" => MutationState::Observed,
        "commandExecution" | "mcpToolCall" | "dynamicToolCall" => MutationState::Unknown,
        _ => MutationState::NoneObserved,
    };
    let status = item
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("observed");
    Ok(Some(ProviderEvent::NativeItemActivity {
        native_item_id,
        description: format!("{item_type}: {status}"),
        mutation,
    }))
}

fn send_to_turn(
    turns: &mut HashMap<String, TurnSink>,
    thread_id: &str,
    event: Result<ProviderEvent, ProviderError>,
) -> bool {
    turns.get(thread_id).is_some_and(|turn| {
        let terminal = match &event {
            Ok(event) => event.is_terminal(),
            Err(_) => true,
        };
        if !terminal && turn.events.capacity() <= 1 {
            return true;
        }
        turn.events.try_send(event).is_err()
    })
}

async fn deliver_to_turn(
    state: &mut DispatcherState,
    thread_id: &str,
    event: Result<ProviderEvent, ProviderError>,
    sender: &JsonLineSender,
) -> Result<(), ProviderError> {
    if !send_to_turn(&mut state.turns, thread_id, event) {
        return Ok(());
    }

    let request_ids = state
        .server_requests
        .iter()
        .filter(|(_, request)| request.thread_id == thread_id)
        .map(|(external_id, _)| external_id.clone())
        .collect::<Vec<_>>();
    for external_id in request_ids {
        if let Some(request) = state.server_requests.remove(&external_id) {
            required_write(
                sender,
                &state.process_shutdown,
                json!({
                    "id": request.id,
                    "error": {"code": -32002, "message": "Turn event consumer unavailable"},
                }),
            )
            .await?;
            remember_server_request_tombstone(state, request.id);
        }
    }
    let interrupt = state.turns.get_mut(thread_id).and_then(|turn| {
        if turn.interrupt_pending {
            return None;
        }
        let turn_id = turn.native_turn_id.clone()?;
        turn.cancelled = true;
        turn.interrupt_pending = true;
        Some((turn_id, Arc::clone(&turn.completed)))
    });
    if let Some((turn_id, completed)) = interrupt {
        send_fatal_interrupt(sender, state, thread_id.to_owned(), turn_id, completed).await?;
    }
    Ok(())
}

async fn reject_server_requests(
    sender: &JsonLineSender,
    state: &mut DispatcherState,
    thread_id: &str,
    turn_id: &str,
    message: &'static str,
) -> Result<(), ProviderError> {
    let request_ids = state
        .server_requests
        .iter()
        .filter(|(_, request)| request.thread_id == thread_id && request.turn_id == turn_id)
        .map(|(request_id, _)| request_id.clone())
        .collect::<Vec<_>>();
    for request_id in request_ids {
        if let Some(request) = state.server_requests.remove(&request_id) {
            required_write(
                sender,
                &state.process_shutdown,
                json!({
                    "id": request.id,
                    "error": {"code": -32004, "message": message},
                }),
            )
            .await?;
            remember_server_request_tombstone(state, request.id);
        }
    }
    Ok(())
}

async fn required_write(
    sender: &JsonLineSender,
    shutdown: &JsonLineShutdown,
    message: Value,
) -> Result<(), ProviderError> {
    let result = tokio::time::timeout(FATAL_RESPONSE_TIMEOUT, sender.send(&message)).await;
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            shutdown.request();
            Err(error)
        }
        Err(_) => {
            shutdown.request();
            Err(ProviderError::Transport {
                category: "required-response-timeout".to_owned(),
            })
        }
    }
}

async fn broadcast_error(turns: &mut HashMap<String, TurnSink>, error: ProviderError) {
    for (_, turn) in turns.drain() {
        let _ = turn.events.try_send(Err(error.clone()));
    }
}

async fn respond_to_server_request(
    sender: &JsonLineSender,
    thread_id: &str,
    request_id: &str,
    response: ApprovalResponse,
    state: &mut DispatcherState,
) -> Result<(), ProviderError> {
    let request = state
        .server_requests
        .get(request_id)
        .ok_or_else(|| protocol("unknown-server-request"))?;
    let active_owner = state.turns.get(thread_id).is_some_and(|turn| {
        turn.native_turn_id.as_deref() == Some(&request.turn_id)
            && !turn.cancelled
            && !turn.interrupt_pending
    });
    if request.thread_id != thread_id || !active_owner {
        return Err(protocol("server-request-owner-mismatch"));
    }
    let result = match (&request.kind, response) {
        (ServerRequestKind::Approval, ApprovalResponse::Approved) => json!({"decision": "accept"}),
        (ServerRequestKind::Approval, ApprovalResponse::Denied) => json!({"decision": "decline"}),
        (ServerRequestKind::UserInput { question_ids }, ApprovalResponse::Answers(answers)) => {
            if answers.len() != question_ids.len()
                || question_ids.iter().any(|id| !answers.contains_key(id))
            {
                return Err(protocol("user-input-answer-shape"));
            }
            let answers = answers
                .into_iter()
                .map(|(id, answers)| (id, json!({"answers": answers})))
                .collect::<serde_json::Map<_, _>>();
            json!({"answers": answers})
        }
        (ServerRequestKind::Permissions { requested }, ApprovalResponse::Approved) => {
            json!({"permissions": requested, "scope": "turn"})
        }
        (ServerRequestKind::Permissions { .. }, ApprovalResponse::Denied) => {
            json!({"permissions": {}, "scope": "turn"})
        }
        _ => return Err(protocol("response-kind-mismatch")),
    };
    let id = request.id.clone();
    required_write(
        sender,
        &state.process_shutdown,
        json!({"id": id, "result": result}),
    )
    .await?;
    state.server_requests.remove(request_id);
    remember_server_request_tombstone(state, id);
    Ok(())
}

fn approval_scope(params: &Value, canonical_fallback: &str) -> String {
    params
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or(canonical_fallback)
        .to_owned()
}

fn optional_string(params: &Value, field: &str) -> Result<Option<String>, ProviderError> {
    match params.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(protocol("approval-string-field")),
    }
}

fn required_i64(params: &Value, field: &str, category: &str) -> Result<i64, ProviderError> {
    params
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| protocol(category))
}

fn parse_file_changes(value: &Value) -> Result<Vec<FileChangeApprovalDetail>, ProviderError> {
    value
        .as_array()
        .ok_or_else(|| protocol("file-change-details"))?
        .iter()
        .map(|raw_change| {
            let kind = raw_change
                .get("kind")
                .ok_or_else(|| protocol("file-change-kind"))?;
            let kind_name = required_string(kind, &["type"], "file-change-kind")?;
            let change = match kind_name.as_str() {
                "add" => FileChangeKind::Add,
                "delete" => FileChangeKind::Delete,
                "update" => FileChangeKind::Update {
                    move_path: optional_string(kind, "move_path")?,
                },
                _ => return Err(protocol("file-change-kind")),
            };
            Ok(FileChangeApprovalDetail {
                path: required_string(raw_change, &["path"], "file-change-path")?,
                change,
            })
        })
        .collect()
}

fn record_file_changes(
    state: &mut DispatcherState,
    thread_id: &str,
    item_id: String,
    changes: Vec<FileChangeApprovalDetail>,
) -> Result<(), ProviderError> {
    let turn = state
        .turns
        .get_mut(thread_id)
        .ok_or_else(|| protocol("turn-registration-missing"))?;
    if !turn.file_changes.contains_key(&item_id) && turn.file_changes.len() >= MAX_FILE_CHANGE_ITEMS
    {
        return Err(protocol("file-change-item-capacity-exceeded"));
    }
    turn.file_changes.insert(item_id, changes);
    Ok(())
}

fn parse_agent_status(status: &str) -> NativeAgentStatus {
    match status {
        "pendingInit" => NativeAgentStatus::PendingInit,
        "running" => NativeAgentStatus::Running,
        "interrupted" => NativeAgentStatus::Interrupted,
        "completed" => NativeAgentStatus::Completed,
        "errored" => NativeAgentStatus::Errored,
        "shutdown" => NativeAgentStatus::Shutdown,
        "notFound" => NativeAgentStatus::NotFound,
        other => NativeAgentStatus::Unrecognized(other.to_owned()),
    }
}

fn parse_rpc_id(value: &Value) -> Option<RpcId> {
    serde_json::from_value(value.clone()).ok()
}

fn parse_session(result: Value) -> Result<ProviderSession, ProviderError> {
    Ok(ProviderSession {
        provider: ProviderId::Codex,
        native_id: required_string(&result, &["thread", "id"], "thread-id")?,
        native_group_id: Some(required_string(
            &result,
            &["thread", "sessionId"],
            "thread-session-id",
        )?),
    })
}

fn require_codex_session(session: &ProviderSession) -> Result<(), ProviderError> {
    if session.provider == ProviderId::Codex {
        Ok(())
    } else {
        Err(protocol("provider-session-mismatch"))
    }
}

fn required_string(value: &Value, path: &[&str], category: &str) -> Result<String, ProviderError> {
    let mut current = value;
    for component in path {
        current = current.get(*component).ok_or_else(|| protocol(category))?;
    }
    current
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| protocol(category))
}

fn path_string(path: &Path) -> Result<&str, ProviderError> {
    path.to_str().ok_or(ProviderError::NotDispatched {
        category: super::ProviderErrorCategory::Protocol,
    })
}

fn protocol(category: &str) -> ProviderError {
    ProviderError::Protocol {
        category: category.to_owned(),
    }
}

fn closed_transport() -> ProviderError {
    ProviderError::Transport {
        category: "codex-dispatcher-closed".to_owned(),
    }
}

fn not_dispatched() -> ProviderError {
    ProviderError::NotDispatched {
        category: super::ProviderErrorCategory::Protocol,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

    use serde_json::json;
    use tokio::process::Command;
    use tokio::sync::{mpsc, oneshot};

    use super::{
        Client, ClientCommand, CodexTurnOwner, DispatcherState, PROVISIONAL_EVENT_CAPACITY,
        PendingResponse, ProviderError, REQUEST_AWAITING_RESPONSE, REQUEST_CANCELLED,
        REQUEST_FINISHED, REQUEST_QUEUED, REQUEST_WRITING, RequestKind, RpcId, ServerRequest,
        ServerRequestKind, TurnSink, TurnStartDropAction, cancel_registered_turn,
        duplicate_server_request_response, handle_command, handle_notification,
        handle_server_request, handle_server_response, normalize_item,
        parse_supported_server_request, reject_pending_capacity, reject_server_requests,
        respond_to_server_request, turn_start_drop_action, unregister_turn_registration,
    };
    use crate::providers::process::JsonLineProcess;
    use crate::providers::{ApprovalResponse, ProviderEvent, ProviderTurnOwner};

    fn turn_sink(registration_id: u64) -> TurnSink {
        let (events, _) = mpsc::channel(PROVISIONAL_EVENT_CAPACITY);
        TurnSink {
            registration_id,
            events,
            completed: Arc::new(AtomicBool::new(false)),
            native_turn_id: None,
            announced_turn_id: None,
            provisional_events: VecDeque::new(),
            provisional_terminal: false,
            cancelled: false,
            cancellation_resolved: Arc::new(AtomicBool::new(false)),
            interrupt_pending: false,
            interrupt_waiters: Vec::new(),
            file_changes: HashMap::new(),
        }
    }

    #[test]
    fn queued_turn_start_drop_unregisters_without_killing_the_connection() {
        assert_eq!(
            turn_start_drop_action(REQUEST_QUEUED),
            TurnStartDropAction::Unregister
        );
        assert_eq!(
            turn_start_drop_action(REQUEST_WRITING),
            TurnStartDropAction::Fatal
        );
        assert_eq!(
            turn_start_drop_action(REQUEST_AWAITING_RESPONSE),
            TurnStartDropAction::CancelActive
        );
        assert_eq!(
            turn_start_drop_action(REQUEST_FINISHED),
            TurnStartDropAction::None
        );
        assert_eq!(
            turn_start_drop_action(REQUEST_CANCELLED),
            TurnStartDropAction::None
        );
    }

    #[test]
    fn stale_cleanup_cannot_unregister_a_replacement_turn() {
        let mut turns = HashMap::from([("thread-1".to_owned(), turn_sink(2))]);

        unregister_turn_registration("thread-1", 1, &mut turns);
        assert_eq!(turns.get("thread-1").unwrap().registration_id, 2);

        unregister_turn_registration("thread-1", 2, &mut turns);
        assert!(!turns.contains_key("thread-1"));
    }

    #[tokio::test]
    async fn ready_start_response_after_receivers_close_retains_the_cancelling_generation() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30"]);
        let process = JsonLineProcess::spawn(command).unwrap();
        let sender = process.sender();
        let shutdown = process.shutdown_handle();
        let (events, receiver) = mpsc::channel(PROVISIONAL_EVENT_CAPACITY);
        drop(receiver);
        let mut turn = turn_sink(7);
        turn.events = events;
        let (response, response_receiver) = oneshot::channel();
        drop(response_receiver);
        let id = RpcId::Number(1);
        let mut state = DispatcherState {
            next_id: 2,
            pending: HashMap::from([(
                id.clone(),
                PendingResponse::Deliver {
                    request_key: Some(7),
                    kind: RequestKind::TurnStart {
                        thread_id: "thread-1".to_owned(),
                        registration_id: 7,
                    },
                    response,
                },
            )]),
            turns: HashMap::from([("thread-1".to_owned(), turn)]),
            server_requests: HashMap::new(),
            client_response_tombstones: VecDeque::new(),
            server_request_tombstones: VecDeque::new(),
            confirmed_interrupts: VecDeque::new(),
            process_shutdown: shutdown,
        };

        handle_server_response(
            json!({"id": 1, "result": {"turn": {"id": "turn-1"}}}),
            id,
            &sender,
            &mut state,
        )
        .await
        .unwrap();

        let retained = state.turns.get("thread-1").expect("generation was removed");
        assert_eq!(retained.registration_id, 7);
        assert!(retained.cancelled);
        assert!(retained.interrupt_pending);

        let (replacement_events, _replacement_receiver) = mpsc::channel(1);
        let (registered, registration) = oneshot::channel();
        handle_command(
            ClientCommand::RegisterTurn {
                thread_id: "thread-1".to_owned(),
                registration_id: 8,
                events: replacement_events,
                completed: Arc::new(AtomicBool::new(false)),
                response: registered,
            },
            &sender,
            &mut state,
        )
        .await
        .unwrap();
        assert_eq!(
            registration.await.unwrap(),
            Err(super::protocol("thread-already-active"))
        );
        assert_eq!(state.turns["thread-1"].registration_id, 7);
        process.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn natural_terminal_winning_queued_owner_shutdown_is_successful() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30"]);
        let process = JsonLineProcess::spawn(command).unwrap();
        let (commands, mut command_receiver) = mpsc::channel(1);
        let (cancellations, _cancellation_receiver) = mpsc::channel(1);
        let completed = Arc::new(AtomicBool::new(false));
        let owner = CodexTurnOwner {
            client: Client {
                commands,
                cancellations,
                next_request_key: Arc::new(AtomicU64::new(1)),
                process_shutdown: process.shutdown_handle(),
            },
            thread_id: "thread-1".to_owned(),
            turn_id: "turn-1".to_owned(),
            registration_id: 7,
            completed: Arc::clone(&completed),
        };
        let shutdown = tokio::spawn(Box::new(owner).shutdown());
        let command = command_receiver.recv().await.unwrap();
        let ClientCommand::CancelTurn { response, .. } = command else {
            panic!("owner shutdown did not enqueue cancellation");
        };
        completed.store(true, Ordering::Release);
        response
            .unwrap()
            .send(Err(super::not_dispatched()))
            .unwrap();

        assert_eq!(shutdown.await.unwrap(), Ok(()));
        process.send(&json!({"probe": true})).await.unwrap();
        process.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn terminal_interrupt_waiters_fail_when_approval_cleanup_write_fails() {
        let mut command = Command::new("sh");
        command.args(["-c", "exec 0<&-; printf '{\"ready\":true}\\n'; sleep 30"]);
        let mut process = JsonLineProcess::spawn(command).unwrap();
        assert_eq!(
            process.recv().await.unwrap().unwrap(),
            json!({"ready": true})
        );
        let sender = process.sender();
        let shutdown = process.shutdown_handle();
        let completed = Arc::new(AtomicBool::new(false));
        let confirmed = Arc::new(AtomicBool::new(false));
        let (coalesced_response, coalesced_result) = oneshot::channel();
        let mut turn = turn_sink(7);
        turn.native_turn_id = Some("turn-1".to_owned());
        turn.cancelled = true;
        turn.interrupt_pending = true;
        turn.interrupt_waiters.push(coalesced_response);
        let (primary_response, primary_result) = oneshot::channel();
        let interrupt_id = RpcId::Number(9);
        let approval_id = RpcId::String("approval".to_owned());
        let mut state = DispatcherState {
            next_id: 10,
            pending: HashMap::from([(
                interrupt_id.clone(),
                PendingResponse::Deliver {
                    request_key: None,
                    kind: RequestKind::Interrupt {
                        thread_id: "thread-1".to_owned(),
                        turn_id: "turn-1".to_owned(),
                        completed: Arc::clone(&completed),
                        confirmed: Arc::clone(&confirmed),
                    },
                    response: primary_response,
                },
            )]),
            turns: HashMap::from([("thread-1".to_owned(), turn)]),
            server_requests: HashMap::from([(
                approval_id.external(),
                ServerRequest {
                    id: approval_id,
                    thread_id: "thread-1".to_owned(),
                    turn_id: "turn-1".to_owned(),
                    kind: ServerRequestKind::Approval,
                },
            )]),
            client_response_tombstones: VecDeque::new(),
            server_request_tombstones: VecDeque::new(),
            confirmed_interrupts: VecDeque::new(),
            process_shutdown: shutdown,
        };

        let result = handle_notification(
            "turn/completed",
            json!({
                "threadId": "thread-1",
                "turn": {"id": "turn-1", "status": "completed"},
            }),
            &sender,
            &mut state,
        )
        .await;

        let error = result.unwrap_err();
        assert_eq!(primary_result.await.unwrap(), Err(error.clone()));
        assert_eq!(coalesced_result.await.unwrap(), Err(error));
        assert!(confirmed.load(Ordering::Acquire));
        assert!(!completed.load(Ordering::Acquire));
        assert!(state.turns.contains_key("thread-1"));
        assert!(state.pending.is_empty());
        assert_eq!(
            state.client_response_tombstones,
            VecDeque::from([interrupt_id])
        );
        assert_eq!(
            state.confirmed_interrupts,
            VecDeque::from([("thread-1".to_owned(), "turn-1".to_owned(), 7)])
        );
        assert!(process.shutdown().await.is_err());
    }

    #[tokio::test]
    async fn duplicate_pending_id_error_is_writer_acknowledged_before_fatal_return() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30"]);
        let process = JsonLineProcess::spawn(command).unwrap();
        let sender = process.sender();
        let shutdown = process.shutdown_handle();
        let mut turn = turn_sink(7);
        turn.native_turn_id = Some("turn-1".to_owned());
        let id = RpcId::String("duplicate".to_owned());
        let mut state = DispatcherState {
            next_id: 2,
            pending: HashMap::new(),
            turns: HashMap::from([("thread-1".to_owned(), turn)]),
            server_requests: HashMap::from([(
                id.external(),
                ServerRequest {
                    id: id.clone(),
                    thread_id: "thread-1".to_owned(),
                    turn_id: "turn-1".to_owned(),
                    kind: ServerRequestKind::Approval,
                },
            )]),
            client_response_tombstones: VecDeque::new(),
            server_request_tombstones: VecDeque::new(),
            confirmed_interrupts: VecDeque::new(),
            process_shutdown: shutdown,
        };
        assert_eq!(
            duplicate_server_request_response(id.clone()),
            json!({
                "id": "duplicate",
                "error": {"code": -32600, "message": "Duplicate server request ID"},
            })
        );

        let result = handle_server_request(
            json!({
                "id": "duplicate",
                "method": "item/commandExecution/requestApproval",
                "params": {
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "itemId": "command-1",
                    "startedAtMs": 1,
                },
            }),
            id.clone(),
            &sender,
            &mut state,
        )
        .await;

        assert_eq!(result, Err(super::protocol("duplicate-server-request-id")));
        assert!(state.server_requests.is_empty());
        assert_eq!(state.server_request_tombstones, VecDeque::from([id]));
        sender.send(&json!({"probe": true})).await.unwrap();
        process.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn admitted_interrupt_closes_the_user_approval_gate_before_confirmation() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30"]);
        let process = JsonLineProcess::spawn(command).unwrap();
        let sender = process.sender();
        let shutdown = process.shutdown_handle();
        let mut turn = turn_sink(7);
        turn.native_turn_id = Some("turn-1".to_owned());
        let request_id = RpcId::String("approval".to_owned());
        let external_id = request_id.external();
        let mut state = DispatcherState {
            next_id: 2,
            pending: HashMap::new(),
            turns: HashMap::from([("thread-1".to_owned(), turn)]),
            server_requests: HashMap::from([(
                external_id.clone(),
                ServerRequest {
                    id: request_id,
                    thread_id: "thread-1".to_owned(),
                    turn_id: "turn-1".to_owned(),
                    kind: ServerRequestKind::Approval,
                },
            )]),
            client_response_tombstones: VecDeque::new(),
            server_request_tombstones: VecDeque::new(),
            confirmed_interrupts: VecDeque::new(),
            process_shutdown: shutdown,
        };
        let (interrupt_response, _interrupt_result) = oneshot::channel();
        cancel_registered_turn(
            "thread-1".to_owned(),
            Some("turn-1".to_owned()),
            Some(7),
            Some(interrupt_response),
            &sender,
            &mut state,
        )
        .await
        .unwrap();
        let turn = &state.turns["thread-1"];
        assert!(turn.cancelled);
        assert!(turn.interrupt_pending);

        assert!(
            respond_to_server_request(
                &sender,
                "thread-1",
                &external_id,
                ApprovalResponse::Approved,
                &mut state,
            )
            .await
            .is_err()
        );
        assert!(state.server_requests.contains_key(&external_id));
        reject_server_requests(
            &sender,
            &mut state,
            "thread-1",
            "turn-1",
            "Turn was cancelled",
        )
        .await
        .unwrap();
        assert!(!state.server_requests.contains_key(&external_id));
        assert_eq!(state.server_request_tombstones.len(), 1);
        process.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn duplicate_pending_id_during_cancellation_invalidates_before_cleanup() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30"]);
        let process = JsonLineProcess::spawn(command).unwrap();
        let sender = process.sender();
        let shutdown = process.shutdown_handle();
        let mut turn = turn_sink(7);
        turn.native_turn_id = Some("turn-1".to_owned());
        turn.cancelled = true;
        turn.interrupt_pending = true;
        let id = RpcId::String("duplicate-cancelling".to_owned());
        let mut state = DispatcherState {
            next_id: 2,
            pending: HashMap::new(),
            turns: HashMap::from([("thread-1".to_owned(), turn)]),
            server_requests: HashMap::from([(
                id.external(),
                ServerRequest {
                    id: id.clone(),
                    thread_id: "thread-1".to_owned(),
                    turn_id: "turn-1".to_owned(),
                    kind: ServerRequestKind::Approval,
                },
            )]),
            client_response_tombstones: VecDeque::new(),
            server_request_tombstones: VecDeque::new(),
            confirmed_interrupts: VecDeque::new(),
            process_shutdown: shutdown,
        };

        let result = handle_server_request(
            json!({
                "id": "duplicate-cancelling",
                "method": "item/commandExecution/requestApproval",
                "params": {
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "itemId": "command-2",
                    "startedAtMs": 2,
                },
            }),
            id.clone(),
            &sender,
            &mut state,
        )
        .await;

        assert_eq!(result, Err(super::protocol("duplicate-server-request-id")));
        assert!(state.server_requests.is_empty());
        assert_eq!(state.server_request_tombstones, VecDeque::from([id]));
        reject_server_requests(
            &sender,
            &mut state,
            "thread-1",
            "turn-1",
            "Turn was cancelled",
        )
        .await
        .unwrap();
        assert_eq!(state.server_request_tombstones.len(), 1);
        process.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn duplicate_pending_id_precedes_owner_and_terminal_admission_checks() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30"]);
        let process = JsonLineProcess::spawn(command).unwrap();
        let sender = process.sender();

        for (suffix, incoming_thread, terminal) in [
            ("owner", "other-thread", false),
            ("terminal", "thread-1", true),
        ] {
            let mut turn = turn_sink(7);
            turn.native_turn_id = Some("turn-1".to_owned());
            turn.provisional_terminal = terminal;
            let id = RpcId::String(format!("duplicate-{suffix}"));
            let mut state = DispatcherState {
                next_id: 2,
                pending: HashMap::new(),
                turns: HashMap::from([("thread-1".to_owned(), turn)]),
                server_requests: HashMap::from([(
                    id.external(),
                    ServerRequest {
                        id: id.clone(),
                        thread_id: "thread-1".to_owned(),
                        turn_id: "turn-1".to_owned(),
                        kind: ServerRequestKind::Approval,
                    },
                )]),
                client_response_tombstones: VecDeque::new(),
                server_request_tombstones: VecDeque::new(),
                confirmed_interrupts: VecDeque::new(),
                process_shutdown: process.shutdown_handle(),
            };

            let result = handle_server_request(
                json!({
                    "id": format!("duplicate-{suffix}"),
                    "method": "item/commandExecution/requestApproval",
                    "params": {
                        "threadId": incoming_thread,
                        "turnId": "turn-1",
                        "itemId": "command-2",
                        "startedAtMs": 2,
                    },
                }),
                id.clone(),
                &sender,
                &mut state,
            )
            .await;

            assert_eq!(result, Err(super::protocol("duplicate-server-request-id")));
            assert!(state.server_requests.is_empty());
            assert_eq!(state.server_request_tombstones, VecDeque::from([id]));
        }
        process.shutdown().await.unwrap();
    }

    #[test]
    fn pending_capacity_rejection_unregisters_the_exact_turn_as_not_dispatched() {
        let mut turns = HashMap::from([("thread-1".to_owned(), turn_sink(7))]);
        let phase = AtomicU8::new(REQUEST_QUEUED);
        let (response, mut receiver) = oneshot::channel();

        reject_pending_capacity(
            &RequestKind::TurnStart {
                thread_id: "thread-1".to_owned(),
                registration_id: 7,
            },
            response,
            &mut turns,
        );

        assert_eq!(
            phase.load(std::sync::atomic::Ordering::Acquire),
            REQUEST_QUEUED
        );
        assert!(!turns.contains_key("thread-1"));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            Err(ProviderError::NotDispatched { .. })
        ));
    }

    #[test]
    fn direct_interrupt_capacity_rejection_leaves_the_turn_retryable() {
        let mut active = turn_sink(7);
        active.native_turn_id = Some("turn-7".to_owned());
        let mut turns = HashMap::from([("thread-1".to_owned(), active)]);
        let (response, mut receiver) = oneshot::channel();

        reject_pending_capacity(
            &RequestKind::Interrupt {
                thread_id: "thread-1".to_owned(),
                turn_id: "turn-7".to_owned(),
                completed: Arc::new(AtomicBool::new(false)),
                confirmed: Arc::new(AtomicBool::new(false)),
            },
            response,
            &mut turns,
        );

        let turn = turns.get("thread-1").unwrap();
        assert!(!turn.cancelled);
        assert!(!turn.interrupt_pending);
        assert!(matches!(
            receiver.try_recv().unwrap(),
            Err(ProviderError::NotDispatched { .. })
        ));
    }

    #[test]
    fn collab_items_reject_every_malformed_receiver_and_status_entry() {
        let malformed_receiver = json!({
            "threadId": "parent",
            "item": {
                "id": "item-1",
                "type": "collabAgentToolCall",
                "senderThreadId": "parent",
                "tool": "spawnAgent",
                "status": "inProgress",
                "receiverThreadIds": ["child", 42],
                "agentsStates": {"child": {"status": "running"}}
            }
        });
        assert!(normalize_item(&malformed_receiver).is_err());

        let malformed_status = json!({
            "threadId": "parent",
            "item": {
                "id": "item-1",
                "type": "collabAgentToolCall",
                "senderThreadId": "parent",
                "tool": "spawnAgent",
                "status": "inProgress",
                "receiverThreadIds": ["child"],
                "agentsStates": {"child": {"status": 42}}
            }
        });
        assert!(normalize_item(&malformed_status).is_err());

        let receiver_without_state = json!({
            "threadId": "parent",
            "item": {
                "id": "item-1",
                "type": "collabAgentToolCall",
                "senderThreadId": "parent",
                "tool": "spawnAgent",
                "status": "futureNonemptyStatus",
                "receiverThreadIds": ["child"],
                "agentsStates": {}
            }
        });
        assert!(normalize_item(&receiver_without_state).unwrap().is_some());

        for field in ["tool", "status"] {
            let mut missing = receiver_without_state.clone();
            missing["item"].as_object_mut().unwrap().remove(field);
            assert!(
                normalize_item(&missing).is_err(),
                "accepted missing {field}"
            );
        }
    }

    #[test]
    fn reasonless_approval_uses_a_canonical_scope_instead_of_native_item_identity() {
        let (_, event) = parse_supported_server_request(
            "item/commandExecution/requestApproval",
            &json!({
                "itemId": "provider-native-item-secret",
                "startedAtMs": 1,
                "command": "cargo test"
            }),
            "string:provider-native-request-secret",
            None,
        )
        .unwrap();
        let ProviderEvent::ApprovalRequested { scope, .. } = event else {
            panic!("fixture must normalize to an approval");
        };

        assert_eq!(scope, "command execution");
        assert!(
            !serde_json::to_string(&scope)
                .unwrap()
                .contains("provider-native")
        );
    }
}
