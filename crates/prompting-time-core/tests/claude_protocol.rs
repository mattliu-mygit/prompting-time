use std::collections::VecDeque;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use prompting_time_core::providers::process::JsonLineProcess;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::process::Command;
use tokio::time::{Instant, timeout, timeout_at};
use uuid::Uuid;

const OPERATION_TIMEOUT: Duration = Duration::from_secs(120);
const PROCESS_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

type ProbeResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PermissionMode {
    DontAsk,
    Manual,
}

impl PermissionMode {
    fn as_arg(self) -> &'static str {
        match self {
            Self::DontAsk => "dontAsk",
            Self::Manual => "manual",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToolProfile {
    None,
    Mutation,
    ChildAgent,
}

impl ToolProfile {
    fn tools_arg(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Mutation => "Write",
            Self::ChildAgent => "Agent",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ApprovalHookState {
    Allow,
    Defer,
    Deny,
}

impl ApprovalHookState {
    fn defer(&mut self) {
        *self = Self::Defer;
    }

    fn deny(&mut self) {
        *self = Self::Deny;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ChildAgentEvidence {
    parent_tool_use_id: String,
    native_task_id: String,
    child_origin_events: usize,
    completed: bool,
}

#[derive(Debug)]
struct ProbeTurn {
    session_id: String,
    subtype: String,
    stop_reason: Option<String>,
    terminal_reason: Option<String>,
    is_error: bool,
    final_text: String,
    pending_approval: Option<Value>,
    events: Vec<Value>,
}

impl ProbeTurn {
    fn from_result(result: Value, events: Vec<Value>) -> ProbeResult<Self> {
        if result.get("type").and_then(Value::as_str) != Some("result") {
            return Err("expected a Claude result event".into());
        }
        let session_id = required_string(&result, "session_id")?.to_owned();
        let subtype = required_string(&result, "subtype")?.to_owned();
        let is_error = result
            .get("is_error")
            .and_then(Value::as_bool)
            .ok_or("Claude result is missing required is_error")?;
        let stop_reason = result
            .get("stop_reason")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let terminal_reason = result
            .get("terminal_reason")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let final_text = result
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let pending_approval = result.get("deferred_tool_use").cloned();
        Ok(Self {
            session_id,
            subtype,
            stop_reason,
            terminal_reason,
            is_error,
            final_text,
            pending_approval,
            events,
        })
    }

    fn synthetic(events: Vec<Value>) -> Self {
        Self {
            session_id: "synthetic-session".to_owned(),
            subtype: "success".to_owned(),
            stop_reason: Some("end_turn".to_owned()),
            terminal_reason: Some("completed".to_owned()),
            is_error: false,
            final_text: "synthetic".to_owned(),
            pending_approval: None,
            events,
        }
    }

    fn require_success(&self) -> ProbeResult<()> {
        if self.subtype != "success" || self.is_error {
            return Err(format!(
                "Claude turn was not successful: subtype={},is_error={}",
                self.subtype, self.is_error
            )
            .into());
        }
        Ok(())
    }

    fn deferred_write(&self, workspace: &Path) -> ProbeResult<&Value> {
        self.require_success()?;
        if self.stop_reason.as_deref() != Some("tool_deferred") {
            return Err("Claude result did not stop for a deferred tool".into());
        }
        let pending = self
            .pending_approval
            .as_ref()
            .ok_or("Claude result omitted deferred_tool_use")?;
        required_string(pending, "id")?;
        if required_string(pending, "name")? != "Write" {
            return Err("deferred tool was not Write".into());
        }
        let input = pending
            .get("input")
            .and_then(Value::as_object)
            .ok_or("deferred Write omitted its input")?;
        let file_path = input
            .get("file_path")
            .and_then(Value::as_str)
            .ok_or("deferred Write omitted file_path")?;
        if Path::new(file_path) != workspace.join("approval-probe.txt") {
            return Err("deferred Write targeted an unexpected path".into());
        }
        let content = input
            .get("content")
            .and_then(Value::as_str)
            .ok_or("deferred Write omitted content")?;
        if content.trim() != "PROBE" {
            return Err("deferred Write content was not PROBE".into());
        }
        Ok(pending)
    }

    fn require_denied_continuation(&self) -> ProbeResult<()> {
        self.require_success()?;
        if self.stop_reason.as_deref() == Some("tool_deferred")
            || self.terminal_reason.as_deref() == Some("tool_deferred")
            || self.pending_approval.is_some()
            || self.final_text.trim().is_empty()
        {
            return Err("denied tool did not continue to a completed assistant result".into());
        }
        Ok(())
    }

    fn require_interrupted(&self) -> ProbeResult<()> {
        let interrupted = matches!(
            self.terminal_reason.as_deref(),
            Some("aborted_streaming" | "aborted_tools")
        ) || self
            .stop_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("interrupt"));
        if !interrupted {
            return Err(format!(
                "interrupt ended with stop_reason={:?},terminal_reason={:?}",
                self.stop_reason, self.terminal_reason
            )
            .into());
        }
        Ok(())
    }

    fn child_agents(&self) -> ProbeResult<Vec<ChildAgentEvidence>> {
        self.require_success()?;
        let starts = self
            .events
            .iter()
            .filter(|event| event.get("type").and_then(Value::as_str) == Some("assistant"))
            .filter(|event| {
                event.get("session_id").and_then(Value::as_str) == Some(self.session_id.as_str())
            })
            .filter(|event| event.get("parent_tool_use_id").is_none_or(Value::is_null))
            .filter_map(|event| event.pointer("/message/content").and_then(Value::as_array))
            .flat_map(|content| content.iter())
            .filter(|block| {
                block.get("type").and_then(Value::as_str) == Some("tool_use")
                    && block.get("name").and_then(Value::as_str) == Some("Agent")
            })
            .filter_map(|block| block.get("id").and_then(Value::as_str))
            .collect::<Vec<_>>();

        let mut agents = Vec::with_capacity(starts.len());
        for parent_tool_use_id in starts {
            let origin_events = self
                .events
                .iter()
                .filter(|event| {
                    matches!(
                        event.get("type").and_then(Value::as_str),
                        Some("assistant" | "stream_event")
                    ) && event.get("parent_tool_use_id").and_then(Value::as_str)
                        == Some(parent_tool_use_id)
                })
                .collect::<Vec<_>>();
            if origin_events.is_empty() {
                return Err(format!(
                    "Agent tool {parent_tool_use_id} emitted no child-origin event"
                )
                .into());
            }
            for event in &origin_events {
                if required_string(event, "session_id")? != self.session_id {
                    return Err("child-origin event belongs to another session".into());
                }
            }
            let lifecycle_starts = self
                .events
                .iter()
                .filter(|event| {
                    event["type"] == "system"
                        && event["subtype"] == "task_started"
                        && event["tool_use_id"] == parent_tool_use_id
                })
                .collect::<Vec<_>>();
            if lifecycle_starts.len() != 1 {
                return Err("Agent requires exactly one correlated task_started event".into());
            }
            let start = lifecycle_starts[0];
            let task_id = required_string(start, "task_id")?;
            if required_string(start, "session_id")? != self.session_id {
                return Err("child start belongs to another session".into());
            }
            let mut completed = false;
            for event in self.events.iter().filter(|event| {
                event["type"] == "system"
                    && event["subtype"] == "task_notification"
                    && (event["task_id"] == task_id || event["tool_use_id"] == parent_tool_use_id)
            }) {
                if required_string(event, "session_id")? != self.session_id
                    || required_string(event, "task_id")? != task_id
                    || event
                        .get("tool_use_id")
                        .is_some_and(|id| !id.is_null() && id != parent_tool_use_id)
                {
                    return Err("child termination identity does not match its start".into());
                }
                if required_string(event, "status")? != "completed" {
                    return Err("child task did not complete successfully".into());
                }
                completed = true;
            }
            if !completed {
                return Err("child task has no successful lifecycle termination".into());
            }
            agents.push(ChildAgentEvidence {
                parent_tool_use_id: parent_tool_use_id.to_owned(),
                native_task_id: task_id.to_owned(),
                child_origin_events: origin_events.len(),
                completed,
            });
        }
        if agents.is_empty() {
            return Err("Claude emitted no Agent tool use".into());
        }
        Ok(agents)
    }
}

#[derive(Debug)]
struct ActiveTurn {
    session_id: String,
    deadline: Instant,
}

struct LiveClaudeProbe {
    binary: PathBuf,
    workspace: TempDir,
    runtime: TempDir,
    permission_mode: PermissionMode,
    tool_profile: ToolProfile,
    session_id: String,
    process: Option<JsonLineProcess>,
    events: Vec<Value>,
    pending_events: VecDeque<Value>,
    request_sequence: u64,
    approval_hook: ApprovalHookState,
}

impl LiveClaudeProbe {
    async fn spawn(permission_mode: PermissionMode) -> ProbeResult<Self> {
        let tool_profile = match permission_mode {
            PermissionMode::DontAsk => ToolProfile::None,
            PermissionMode::Manual => ToolProfile::Mutation,
        };
        Self::spawn_with_tools(permission_mode, tool_profile).await
    }

    async fn spawn_with_tools(
        permission_mode: PermissionMode,
        tool_profile: ToolProfile,
    ) -> ProbeResult<Self> {
        if std::env::var("PROMPTING_TIME_LIVE_CLAUDE").as_deref() != Ok("1") {
            return Err("set PROMPTING_TIME_LIVE_CLAUDE=1 to run live Claude probes".into());
        }

        let workspace = tempfile::tempdir()?;
        fs::set_permissions(workspace.path(), fs::Permissions::from_mode(0o700))?;
        let runtime = tempfile::tempdir()?;
        fs::set_permissions(runtime.path(), fs::Permissions::from_mode(0o700))?;
        let session_id = Uuid::now_v7().to_string();
        let mut probe = Self {
            binary: PathBuf::from("claude"),
            workspace,
            runtime,
            permission_mode,
            tool_profile,
            session_id,
            process: None,
            events: Vec::new(),
            pending_events: VecDeque::new(),
            request_sequence: 0,
            approval_hook: ApprovalHookState::Allow,
        };
        let deadline = operation_deadline();
        probe.start_process(None, deadline).await?;
        Ok(probe)
    }

    fn cwd(&self) -> &Path {
        self.workspace.path()
    }

    fn defer_next_approval(&mut self) {
        self.approval_hook.defer();
        self.write_hook()
            .expect("live probe hook must remain writable");
    }

    async fn send(&mut self, prompt: &str) -> ProbeResult<ProbeTurn> {
        let deadline = operation_deadline();
        self.send_prompt(prompt, deadline).await?;
        self.receive_turn(deadline).await
    }

    async fn begin(&mut self, prompt: &str) -> ProbeResult<ActiveTurn> {
        let deadline = operation_deadline();
        self.send_prompt(prompt, deadline).await?;
        Ok(ActiveTurn {
            session_id: self.session_id.clone(),
            deadline,
        })
    }

    async fn wait_for_assistant_delta(&mut self, active: &ActiveTurn) -> ProbeResult<()> {
        if active.session_id != self.session_id {
            return Err("active turn belongs to another session".into());
        }
        loop {
            let event = self.next_event(active.deadline).await?;
            if event.get("session_id").and_then(Value::as_str) == Some(&self.session_id)
                && event.get("type").and_then(Value::as_str) == Some("stream_event")
                && event.pointer("/event/type").and_then(Value::as_str)
                    == Some("content_block_delta")
            {
                return Ok(());
            }
        }
    }

    async fn interrupt(&mut self, active: ActiveTurn) -> ProbeResult<String> {
        if active.session_id != self.session_id {
            return Err("active turn belongs to another session".into());
        }
        let response = self
            .control(json!({"subtype": "interrupt"}), active.deadline)
            .await?;
        if response
            .pointer("/response/subtype")
            .and_then(Value::as_str)
            != Some("success")
        {
            return Err(format!("interrupt was not acknowledged: {}", summarize(&response)).into());
        }

        loop {
            let event = self.next_event(active.deadline).await?;
            if event.get("type").and_then(Value::as_str) == Some("result") {
                let interrupted = ProbeTurn::from_result(event, Vec::new())?;
                if interrupted.session_id != self.session_id {
                    return Err("interrupt result belongs to another session".into());
                }
                interrupted.require_interrupted()?;
                break;
            }
        }
        self.stop_process().await?;
        Ok(self.session_id.clone())
    }

    async fn resume(&mut self, session_id: &str, prompt: &str) -> ProbeResult<ProbeTurn> {
        if session_id != self.session_id {
            return Err("cannot resume a different session".into());
        }
        self.stop_process().await?;
        let deadline = operation_deadline();
        self.start_process(Some(session_id), deadline).await?;
        self.send_prompt(prompt, deadline).await?;
        self.receive_turn(deadline).await
    }

    async fn deny_deferred(&mut self, pending: &Value) -> ProbeResult<ProbeTurn> {
        let pending_id = pending
            .get("id")
            .and_then(Value::as_str)
            .ok_or("deferred tool use is missing its id")?;
        if pending_id.is_empty() {
            return Err("deferred tool use id is empty".into());
        }
        self.approval_hook.deny();
        self.write_hook()?;
        let session_id = self.session_id.clone();
        self.stop_process().await?;
        let deadline = operation_deadline();
        self.start_process(Some(&session_id), deadline).await?;
        self.receive_turn(deadline).await
    }

    async fn finish(mut self) -> ProbeResult<()> {
        self.stop_process().await
    }

    async fn start_process(&mut self, resume: Option<&str>, deadline: Instant) -> ProbeResult<()> {
        if self.process.is_some() {
            return Err("probe process is already running".into());
        }

        let settings = if matches!(self.permission_mode, PermissionMode::Manual) {
            self.write_hook()?;
            Some(self.write_settings()?)
        } else {
            None
        };

        let mut command = Command::new(&self.binary);
        command
            .args(process_args(
                self.tool_profile,
                self.permission_mode,
                resume,
            ))
            .current_dir(self.workspace.path())
            .env_remove("CLAUDECODE")
            .env("CLAUDE_CODE_ENTRYPOINT", "prompting-time-live-probe");
        if matches!(self.permission_mode, PermissionMode::DontAsk) {
            command.arg("--safe-mode");
        }
        if let Some(settings) = settings {
            command.arg("--settings").arg(settings);
        }
        if resume.is_none() {
            command.arg(format!("--session-id={}", self.session_id));
        }

        self.process = Some(JsonLineProcess::spawn(command)?);
        let response = self
            .control(
                json!({
                    "subtype": "initialize",
                    "hooks": null,
                    "forwardSubagentText": true
                }),
                deadline,
            )
            .await?;
        if response
            .pointer("/response/subtype")
            .and_then(Value::as_str)
            != Some("success")
        {
            return Err(
                format!("initialize was not acknowledged: {}", summarize(&response)).into(),
            );
        }
        Ok(())
    }

    fn write_settings(&self) -> ProbeResult<PathBuf> {
        let settings_path = self.runtime.path().join("settings.json");
        let hook_path = self.runtime.path().join("permission-hook.sh");
        let settings = json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Write|Edit|Bash|AskUserQuestion",
                    "hooks": [{
                        "type": "command",
                        "command": hook_path,
                        "timeout": 30
                    }]
                }]
            }
        });
        fs::write(&settings_path, serde_json::to_vec(&settings)?)?;
        fs::set_permissions(&settings_path, fs::Permissions::from_mode(0o600))?;
        Ok(settings_path)
    }

    fn write_hook(&self) -> ProbeResult<()> {
        let output = hook_output(self.approval_hook);
        let hook_path = self.runtime.path().join("permission-hook.sh");
        let script = format!("#!/bin/sh\nprintf '%s\\n' '{}'\n", output);
        fs::write(&hook_path, script)?;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }

    async fn send_prompt(&self, prompt: &str, deadline: Instant) -> ProbeResult<()> {
        let process = self
            .process
            .as_ref()
            .ok_or("probe process is not running")?;
        timeout_at(
            deadline,
            process.send(&json!({
                "type": "user",
                "message": {"role": "user", "content": prompt},
                "parent_tool_use_id": null,
                "session_id": self.session_id
            })),
        )
        .await
        .map_err(|_| "Claude prompt write exceeded its operation deadline")??;
        Ok(())
    }

    async fn receive_turn(&mut self, deadline: Instant) -> ProbeResult<ProbeTurn> {
        let mut events = Vec::new();
        loop {
            let event = self.next_event(deadline).await?;
            events.push(event.clone());
            if event.get("type").and_then(Value::as_str) == Some("result") {
                let turn = ProbeTurn::from_result(event, events)?;
                if turn.session_id != self.session_id {
                    return Err("Claude result belongs to another session".into());
                }
                return Ok(turn);
            }
        }
    }

    async fn receive_child_turn(&mut self, deadline: Instant) -> ProbeResult<ProbeTurn> {
        let mut turn = self.receive_turn(deadline).await?;
        turn.require_success()?;
        // A root result can precede background child termination. Keep the same
        // absolute deadline; tool_result may merely acknowledge the spawn.
        while turn.child_agents().is_err() {
            turn.events.push(self.next_event(deadline).await?);
        }
        Ok(turn)
    }

    async fn control(&mut self, request: Value, deadline: Instant) -> ProbeResult<Value> {
        self.request_sequence += 1;
        let request_id = format!("probe-{}", self.request_sequence);
        let process = self
            .process
            .as_ref()
            .ok_or("probe process is not running")?;
        timeout_at(
            deadline,
            process.send(&json!({
                "type": "control_request",
                "request_id": request_id,
                "request": request
            })),
        )
        .await
        .map_err(|_| "Claude control write exceeded its operation deadline")??;

        loop {
            let event = self.read_process_event(deadline).await?;
            if event.get("type").and_then(Value::as_str) == Some("control_response")
                && event
                    .pointer("/response/request_id")
                    .and_then(Value::as_str)
                    == Some(request_id.as_str())
            {
                return Ok(event);
            }
            self.pending_events.push_back(event);
        }
    }

    async fn next_event(&mut self, deadline: Instant) -> ProbeResult<Value> {
        if let Some(event) = self.pending_events.pop_front() {
            return Ok(event);
        }
        self.read_process_event(deadline).await
    }

    async fn read_process_event(&mut self, deadline: Instant) -> ProbeResult<Value> {
        let process = self
            .process
            .as_mut()
            .ok_or("probe process is not running")?;
        let event = timeout_at(deadline, process.recv())
            .await
            .map_err(|_| "Claude operation deadline expired while waiting for a protocol event")?
            .ok_or_else(|| {
                let stderr = String::from_utf8_lossy(&process.stderr_snapshot()).to_string();
                format!(
                    "Claude stream closed; stderr category={}",
                    sanitize_stderr(&stderr)
                )
            })??;
        self.events.push(event.clone());
        Ok(event)
    }

    async fn stop_process(&mut self) -> ProbeResult<()> {
        let Some(process) = self.process.take() else {
            return Ok(());
        };
        match timeout(PROCESS_SHUTDOWN_TIMEOUT, process.shutdown()).await {
            Ok(result) => result.map_err(|error| Box::new(error) as _),
            Err(_) => Err("timed out killing and awaiting Claude process".into()),
        }
    }
}

impl Drop for LiveClaudeProbe {
    fn drop(&mut self) {
        if let Some(process) = self.process.take() {
            process.shutdown_handle().request();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                tokio::task::block_in_place(move || {
                    let _ = handle.block_on(async move {
                        timeout(PROCESS_SHUTDOWN_TIMEOUT, process.shutdown()).await
                    });
                });
            }
        }
    }
}

fn operation_deadline() -> Instant {
    Instant::now() + OPERATION_TIMEOUT
}

fn process_args(
    tool_profile: ToolProfile,
    permission_mode: PermissionMode,
    resume: Option<&str>,
) -> Vec<String> {
    let mut args = [
        "--print",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--include-hook-events",
        "--no-chrome",
        "--max-budget-usd",
        "0.50",
        "--tools",
        tool_profile.tools_arg(),
        "--permission-mode",
        permission_mode.as_arg(),
        "--setting-sources=",
        "--strict-mcp-config",
        "--mcp-config",
        r#"{"mcpServers":{}}"#,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    if let Some(session_id) = resume {
        args.push(format!("--resume={session_id}"));
    }
    args
}

fn contains_arg_pair(args: &[String], name: &str, value: &str) -> bool {
    args.windows(2)
        .any(|pair| pair[0] == name && pair[1] == value)
}

fn hook_output(state: ApprovalHookState) -> Value {
    match state {
        ApprovalHookState::Allow => json!({}),
        ApprovalHookState::Defer => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "defer",
                "permissionDecisionReason": "Prompting Time live probe deferred the mutation"
            }
        }),
        ApprovalHookState::Deny => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "Prompting Time live probe denied the mutation"
            }
        }),
    }
}

fn required_string<'a>(value: &'a Value, key: &str) -> ProbeResult<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Claude event is missing required {key}").into())
}

fn summarize(value: &Value) -> String {
    let message_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let subtype = value
        .get("subtype")
        .or_else(|| value.pointer("/response/subtype"))
        .and_then(Value::as_str)
        .unwrap_or("none");
    format!("type={message_type},subtype={subtype}")
}

fn sanitize_stderr(stderr: &str) -> String {
    if stderr.is_empty() {
        "empty".to_owned()
    } else if stderr.contains("permission") {
        "permission-error".to_owned()
    } else if stderr.contains("auth") || stderr.contains("login") {
        "authentication-error".to_owned()
    } else {
        "nonempty".to_owned()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "uses the installed Claude account"]
async fn live_stream_accepts_two_turns_on_one_session() {
    let mut probe = LiveClaudeProbe::spawn(PermissionMode::DontAsk)
        .await
        .unwrap();
    let first = probe
        .send("Reply with ONE and do not use tools.")
        .await
        .unwrap();
    let second = probe
        .send("Reply with TWO and do not use tools.")
        .await
        .unwrap();
    first.require_success().unwrap();
    second.require_success().unwrap();
    assert_eq!(first.session_id, second.session_id);
    assert_eq!(first.final_text.trim(), "ONE");
    assert_eq!(second.final_text.trim(), "TWO");
    probe.finish().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "uses the installed Claude account"]
async fn live_deferred_approval_can_resume() {
    let mut probe = LiveClaudeProbe::spawn(PermissionMode::Manual)
        .await
        .unwrap();
    probe.defer_next_approval();
    let deferred = probe
        .send("Create approval-probe.txt containing PROBE.")
        .await
        .unwrap();
    let pending = deferred.deferred_write(probe.cwd()).unwrap().clone();
    assert!(!probe.cwd().join("approval-probe.txt").exists());
    let resumed = probe.deny_deferred(&pending).await.unwrap();
    resumed.require_denied_continuation().unwrap();
    assert_eq!(resumed.session_id, deferred.session_id);
    assert!(!probe.cwd().join("approval-probe.txt").exists());
    probe.finish().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "uses the installed Claude account"]
async fn live_interrupt_preserves_resumable_session() {
    let mut probe = LiveClaudeProbe::spawn(PermissionMode::DontAsk)
        .await
        .unwrap();
    let active = probe
        .begin("List the integers from 1 through 200, one per line, without tools.")
        .await
        .unwrap();
    probe.wait_for_assistant_delta(&active).await.unwrap();
    let session_id = probe.interrupt(active).await.unwrap();
    let resumed = probe
        .resume(&session_id, "Reply RESUMED without tools.")
        .await
        .unwrap();
    resumed.require_success().unwrap();
    assert_eq!(resumed.session_id, session_id);
    assert_eq!(resumed.final_text.trim(), "RESUMED");
    probe.finish().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "uses the installed Claude account"]
async fn live_child_agent_events_have_stable_identity() {
    let mut probe =
        LiveClaudeProbe::spawn_with_tools(PermissionMode::DontAsk, ToolProfile::ChildAgent)
            .await
            .unwrap();
    let deadline = operation_deadline();
    probe
        .send_prompt(
            "Ask exactly one subagent to reply CHILD. Do not use other tools.",
            deadline,
        )
        .await
        .unwrap();
    let result = probe.receive_child_turn(deadline).await.unwrap();
    let agents = result.child_agents().unwrap();
    assert_eq!(agents.len(), 1);
    assert!(agents[0].child_origin_events > 0);
    assert!(agents[0].completed);
    probe.finish().await.unwrap();
}

#[test]
fn denied_hook_decision_survives_resume_preparation() {
    let mut state = ApprovalHookState::Allow;
    state.defer();
    assert_eq!(state, ApprovalHookState::Defer);
    state.deny();
    assert_eq!(state, ApprovalHookState::Deny);
    assert_eq!(
        hook_output(state)["hookSpecificOutput"]["permissionDecision"],
        "deny"
    );
}

#[test]
fn deferred_write_requires_a_non_error_tool_deferred_result() {
    let workspace = tempfile::tempdir().unwrap();
    let mut result = json!({
        "type": "result",
        "subtype": "success",
        "session_id": Uuid::now_v7().to_string(),
        "is_error": false,
        "stop_reason": "tool_deferred",
        "result": "",
        "deferred_tool_use": {
            "id": "tool-write-1",
            "name": "Write",
            "input": {
                "file_path": workspace.path().join("approval-probe.txt"),
                "content": "PROBE"
            }
        }
    });

    let turn = ProbeTurn::from_result(result.clone(), vec![]).unwrap();
    assert!(turn.deferred_write(workspace.path()).is_ok());

    result["is_error"] = json!(true);
    let turn = ProbeTurn::from_result(result, vec![]).unwrap();
    assert!(turn.deferred_write(workspace.path()).is_err());
}

#[test]
fn child_evidence_requires_child_origin_events() {
    let parent = "tool-agent-1";
    let top_level_start = json!({
        "type": "assistant",
        "parent_tool_use_id": null,
        "session_id": "synthetic-session",
        "message": {"content": [{"type": "tool_use", "id": parent, "name": "Agent"}]}
    });
    let child_message = json!({
        "type": "assistant",
        "parent_tool_use_id": parent,
        "session_id": "synthetic-session",
        "agentId": "agent-native-1",
        "message": {"content": [{"type": "text", "text": "CHILD"}]}
    });
    let top_level_stop = json!({
        "type": "user",
        "parent_tool_use_id": null,
        "message": {"content": [{"type": "tool_result", "tool_use_id": parent}]}
    });

    let without_child = ProbeTurn::synthetic(vec![top_level_start.clone(), top_level_stop.clone()]);
    assert!(without_child.child_agents().is_err());

    let mut with_child = ProbeTurn::synthetic(vec![top_level_start, child_message, top_level_stop]);
    assert!(
        with_child.child_agents().is_err(),
        "spawn acknowledgement is not completion"
    );
    with_child.events.push(json!({"type":"system", "subtype":"task_started", "session_id":"synthetic-session", "task_id":"task-1", "tool_use_id":parent}));
    assert!(
        with_child.child_agents().is_err(),
        "start is not completion"
    );
    with_child.events.push(json!({"type":"system", "subtype":"task_notification", "session_id":"synthetic-session", "task_id":"task-1", "status":"completed"}));
    let agents = with_child.child_agents().unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].parent_tool_use_id, parent);
    assert_eq!(agents[0].native_task_id, "task-1");
    for (key, value) in [
        ("status", "failed"),
        ("status", "stopped"),
        ("session_id", "other"),
        ("task_id", "other"),
        ("tool_use_id", "other"),
    ] {
        let mut invalid = with_child.events.clone();
        invalid.last_mut().unwrap()[key] = json!(value);
        assert!(
            ProbeTurn::synthetic(invalid).child_agents().is_err(),
            "accepted invalid {key}={value}"
        );
    }
}

#[tokio::test]
async fn child_collection_handles_completion_before_and_after_root_result() {
    for root_first in [false, true] {
        let mut events = VecDeque::from(vec![
            json!({"type":"assistant", "session_id":"synthetic-session", "message":{"content":[{"type":"tool_use", "name":"Agent", "id":"tool-1"}]}}),
            json!({"type":"system", "subtype":"task_started", "session_id":"synthetic-session", "task_id":"task-1", "tool_use_id":"tool-1"}),
            json!({"type":"assistant", "session_id":"synthetic-session", "parent_tool_use_id":"tool-1", "message":{"content":[{"type":"text", "text":"CHILD"}]}}),
        ]);
        let root = json!({"type":"result", "subtype":"success", "is_error":false, "session_id":"synthetic-session"});
        let child = json!({"type":"system", "subtype":"task_notification", "session_id":"synthetic-session", "task_id":"task-1", "status":"completed", "tool_use_id":"tool-1"});
        events.extend(if root_first {
            [root, child]
        } else {
            [child, root]
        });
        let mut probe = LiveClaudeProbe {
            binary: PathBuf::new(),
            workspace: tempfile::tempdir().unwrap(),
            runtime: tempfile::tempdir().unwrap(),
            permission_mode: PermissionMode::DontAsk,
            tool_profile: ToolProfile::ChildAgent,
            session_id: "synthetic-session".into(),
            process: None,
            events: vec![],
            pending_events: events,
            request_sequence: 0,
            approval_hook: ApprovalHookState::Allow,
        };
        let turn = probe
            .receive_child_turn(operation_deadline())
            .await
            .unwrap();
        assert_eq!(turn.child_agents().unwrap()[0].native_task_id, "task-1");
        assert!(probe.pending_events.is_empty());
    }
}

#[test]
fn scenario_arguments_are_restrictive_and_budgeted() {
    let no_tools = process_args(ToolProfile::None, PermissionMode::DontAsk, None);
    assert!(contains_arg_pair(&no_tools, "--tools", ""));
    assert!(contains_arg_pair(&no_tools, "--max-budget-usd", "0.50"));
    assert!(no_tools.iter().any(|arg| arg == "--no-chrome"));

    let mutation = process_args(ToolProfile::Mutation, PermissionMode::Manual, None);
    assert!(contains_arg_pair(&mutation, "--tools", "Write"));

    let child = process_args(ToolProfile::ChildAgent, PermissionMode::DontAsk, None);
    assert!(contains_arg_pair(&child, "--tools", "Agent"));
}
