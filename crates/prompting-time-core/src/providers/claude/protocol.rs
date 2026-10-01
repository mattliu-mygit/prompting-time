use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Value, json};

use super::{protocol_error, rejected};
use crate::context_compaction::{CompactionObservation, CompactionPhase};
use crate::domain::MutationState;
use crate::tool_operation::{MAX_OPERATION_TITLE_BYTES, ToolOperation};
#[path = "operations.rs"]
mod operations;
use crate::providers::{
    ApprovalRequestDetails, ApprovalResponse, FileChangeApprovalDetail, FileChangeKind,
    NativeAgentStatus, NativeChildStatus, ProviderError, ProviderEvent, UserInputOption,
    UserInputQuestion,
};

const MAX_IDENTITIES: usize = 1024;
const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;
const MAX_INPUT_BYTES: usize = 64 * 1024;

pub(super) fn required_id<'a>(value: &'a Value, key: &str) -> Result<&'a str, ProviderError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control))
        .ok_or_else(|| protocol_error("invalid-identity"))
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, ProviderError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty() && text.len() <= MAX_INPUT_BYTES)
        .ok_or_else(|| protocol_error("invalid-required-text"))
}

pub(super) fn validate_session(value: &Value, session: &str) -> Result<(), ProviderError> {
    if let Some(actual) = value.get("session_id")
        && actual.as_str() != Some(session)
    {
        return Err(protocol_error("session-mismatch"));
    }
    Ok(())
}

fn validate_system_advisory(value: &Value, session: &str) -> Result<(), ProviderError> {
    if required_id(value, "session_id")? != session {
        return Err(protocol_error("session-mismatch"));
    }
    required_id(value, "uuid")?;
    // Follow the 2.1.205 schemas, not UI assumptions. Retry counters can repeat or
    // exceed the maximum, delays can be fractional, and display strings can be empty.
    let valid = match value["subtype"].as_str() {
        Some("api_retry") => {
            ["attempt", "max_retries", "retry_delay_ms"]
                .iter()
                .all(|key| value[key].is_number())
                && value
                    .get("error_status")
                    .is_some_and(|status| status.is_null() || status.is_number())
                && matches!(
                    value["error"].as_str(),
                    Some(
                        "authentication_failed"
                            | "oauth_org_not_allowed"
                            | "billing_error"
                            | "rate_limit"
                            | "overloaded"
                            | "invalid_request"
                            | "model_not_found"
                            | "server_error"
                            | "unknown"
                            | "max_output_tokens"
                    )
                )
        }
        Some("notification") => {
            value["key"].is_string()
                && value["text"].is_string()
                && matches!(
                    value["priority"].as_str(),
                    Some("low" | "medium" | "high" | "immediate")
                )
                && value.get("color").is_none_or(Value::is_string)
                && value.get("timeout_ms").is_none_or(Value::is_number)
        }
        Some("memory_recall") => {
            matches!(value["mode"].as_str(), Some("select" | "synthesize"))
                && value["memories"].as_array().is_some_and(|memories| {
                    memories.iter().all(|memory| {
                        memory["path"].is_string()
                            && matches!(
                                memory["scope"].as_str(),
                                Some("personal" | "team" | "organization")
                            )
                            && memory.get("content").is_none_or(Value::is_string)
                    })
                })
        }
        _ => false,
    };
    if !valid {
        return Err(protocol_error("invalid-system-advisory"));
    }
    Ok(())
}

struct Tool {
    name: String,
    identified: bool,
    order: usize,
    parent: Option<String>,
    completed: bool,
    operation: ToolOperation,
    dirty: bool,
    mutation: MutationState,
}

struct Task {
    tool: String,
    status: NativeAgentStatus,
    emitted: Option<NativeAgentStatus>,
    update_terminal: Option<NativeAgentStatus>,
}

struct TextFrame {
    message: String,
    block_count: usize,
    native_block: Option<usize>,
}

pub(super) struct Protocol {
    session: String,
    stream_message: Option<String>,
    stream_text_blocks: HashMap<String, Option<usize>>,
    text_frames: HashMap<String, TextFrame>,
    texts: HashMap<String, String>,
    text_bytes: usize,
    tools: HashMap<String, Tool>,
    tasks: BTreeMap<String, Task>,
    root_success: bool,
    operation_bytes: usize,
}

impl Protocol {
    pub(super) fn new(session: String) -> Self {
        Self {
            session,
            stream_message: None,
            stream_text_blocks: HashMap::new(),
            text_frames: HashMap::new(),
            texts: HashMap::new(),
            text_bytes: 0,
            tools: HashMap::new(),
            tasks: BTreeMap::new(),
            root_success: false,
            operation_bytes: 0,
        }
    }

    pub(super) fn normalize(&mut self, value: Value) -> Result<Vec<ProviderEvent>, ProviderError> {
        let mut events = Vec::new();
        let parent = match value.get("parent_tool_use_id") {
            None | Some(Value::Null) => None,
            Some(Value::String(parent)) if !parent.is_empty() && parent.len() <= 256 => {
                Some(parent.as_str())
            }
            _ => return Err(protocol_error("invalid-parent-identity")),
        };
        match value["type"].as_str() {
            Some("stream_event") => {
                if parent.is_none() {
                    self.stream(&value["event"], &mut events)?;
                }
            }
            Some("assistant") => {
                let message = &value["message"];
                let message_id = required_id(message, "id")?;
                let content = message["content"]
                    .as_array()
                    .ok_or_else(|| protocol_error("assistant-content"))?;
                if content.len() > MAX_IDENTITIES {
                    return Err(protocol_error("assistant-content-capacity"));
                }
                let frame_id = value
                    .get("uuid")
                    .map(|_| required_id(&value, "uuid"))
                    .transpose()?;
                let native_block =
                    if parent.is_none() && content.iter().any(|block| block["type"] == "text") {
                        self.text_frame(message_id, frame_id, content.len())?
                    } else {
                        None
                    };
                for (index, block) in content.iter().enumerate() {
                    match block["type"].as_str() {
                        Some("text") if parent.is_none() => self.text(
                            match (native_block, frame_id) {
                                (Some(index), _) => format!("{message_id}:{index}"),
                                (None, Some(frame)) => {
                                    format!("{message_id}:frame:{frame}:{index}")
                                }
                                (None, None) => format!("{message_id}:{index}"),
                            },
                            block["text"]
                                .as_str()
                                .ok_or_else(|| protocol_error("text-block"))?,
                            false,
                            &mut events,
                        )?,
                        Some("tool_use") => {
                            self.tool(block, parent)?;
                            self.emit_operations(&mut events);
                        }
                        Some("text" | "thinking" | "redacted_thinking") => {}
                        _ => return Err(protocol_error("unsupported-assistant-block")),
                    }
                }
            }
            Some("user") => {
                if let Some(content) = value["message"]["content"].as_array() {
                    if content.len() > MAX_IDENTITIES {
                        return Err(protocol_error("tool-result-capacity"));
                    }
                    for block in content {
                        if block["type"] != "tool_result" {
                            continue;
                        }
                        let id = required_id(block, "tool_use_id")?;
                        if !self.tools.contains_key(id) {
                            self.tool(&json!({"id":id,"name":"Tool","input":{}}), parent)?;
                            self.tools.get_mut(id).unwrap().identified = false;
                        }
                        let tool = self.tools.get_mut(id).unwrap();
                        if parent.is_some() && tool.parent.as_deref() != parent {
                            return Err(protocol_error("tool-result-owner-conflict"));
                        }
                        let mutation =
                            if matches!(tool.name.as_str(), "Write" | "Edit" | "NotebookEdit")
                                && block["is_error"] == false
                            {
                                MutationState::Observed
                            } else {
                                MutationState::Unknown
                            };
                        let mutation = match (tool.mutation, mutation) {
                            (MutationState::Unknown, _) | (_, MutationState::Unknown) => {
                                MutationState::Unknown
                            }
                            (MutationState::Observed, _) | (_, MutationState::Observed) => {
                                MutationState::Observed
                            }
                            _ => MutationState::NoneObserved,
                        };
                        if !tool.completed || tool.mutation != mutation {
                            events.push(ProviderEvent::MutationEvidence { mutation });
                        }
                        tool.completed = true;
                        tool.mutation = mutation;
                        let mut operation = operations::result(&tool.name, block);
                        operation.title = tool.operation.title.clone();
                        self.update_operation(id, operation);
                        self.emit_operations(&mut events);
                    }
                }
            }
            Some("rate_limit_event") => {
                if required_id(&value, "session_id")? != self.session {
                    return Err(protocol_error("session-mismatch"));
                }
                required_id(&value, "uuid")?;
                if !matches!(
                    value["rate_limit_info"]["status"].as_str(),
                    Some("allowed" | "allowed_warning" | "rejected")
                ) {
                    return Err(protocol_error("invalid-rate-limit-event"));
                }
                // Quota state does not establish the turn's terminal outcome.
                // Only the result/lifecycle boundary can complete or fail the turn.
            }
            Some("system") => match value["subtype"].as_str() {
                Some("api_retry" | "notification" | "memory_recall") => {
                    validate_system_advisory(&value, &self.session)?;
                    // Claude owns retrying. Display-only metadata is not transcript,
                    // compaction, or terminal evidence; never retain its private payload.
                    return Ok(events);
                }
                Some("status" | "compact_boundary") => {
                    // This CLI does not establish a native owner for forwarded child compaction.
                    // Never turn an unproven child's notice into root activity.
                    if parent.is_none() {
                        let session = required_id(&value, "session_id")?;
                        if session != self.session {
                            return Err(protocol_error("session-mismatch"));
                        }
                        let uuid = required_id(&value, "uuid")?;
                        let phase = if value["subtype"] == "compact_boundary" {
                            CompactionPhase::Completed {
                                boundary_id: uuid.into(),
                            }
                        } else {
                            match value.get("status") {
                                // Partial-message streaming emits this before each model request.
                                // It is advisory, not a compaction transition or terminal result.
                                Some(Value::String(status)) if status == "requesting" => {
                                    return Ok(events);
                                }
                                Some(Value::String(status)) if status == "compacting" => {
                                    CompactionPhase::Started
                                }
                                Some(Value::Null) if value["compact_result"] == "failed" => {
                                    CompactionPhase::Failed
                                }
                                Some(Value::Null) => CompactionPhase::Cleared,
                                _ => return Err(protocol_error("invalid-compaction-status")),
                            }
                        };
                        events.push(ProviderEvent::Compaction {
                            observation: CompactionObservation {
                                native_session_id: session.into(),
                                native_turn_id: None,
                                native_agent_id: None,
                                phase,
                            },
                        });
                    }
                }
                Some("task_started" | "task_notification") => self.lifecycle(&value)?,
                Some("task_updated") => self.task_update(&value)?,
                Some(
                    "init"
                    | "thinking_tokens"
                    | "task_progress"
                    | "hook_started"
                    | "hook_progress"
                    | "hook_response"
                    | "session_state_changed",
                ) => {}
                // These known types have continuation/model/transcript semantics we do
                // not implement yet. Keep them failures, with literal, private-safe codes.
                Some("informational") => {
                    return Err(protocol_error("unsupported-system-informational"));
                }
                Some("model_fallback") => {
                    return Err(protocol_error("unsupported-system-model-fallback"));
                }
                Some("model_consent_fallback") => {
                    return Err(protocol_error("unsupported-system-model-consent-fallback"));
                }
                Some("model_refusal_fallback") => {
                    return Err(protocol_error("unsupported-system-model-refusal-fallback"));
                }
                Some("model_refusal_no_fallback") => {
                    return Err(protocol_error(
                        "unsupported-system-model-refusal-no-fallback",
                    ));
                }
                _ => return Err(protocol_error("unsupported-system-envelope")),
            },
            Some("result") => {
                if required_id(&value, "session_id")? != self.session {
                    return Err(protocol_error("session-mismatch"));
                }
                let is_error = value["is_error"]
                    .as_bool()
                    .ok_or_else(|| protocol_error("result-missing-error-status"))?;
                if matches!(
                    value["terminal_reason"].as_str(),
                    Some("aborted_streaming" | "aborted_tools")
                ) || value["stop_reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("interrupt"))
                {
                    events.push(ProviderEvent::Interrupted);
                    return Ok(events);
                }
                if value["subtype"] != "success"
                    || is_error
                    || value["stop_reason"] == "tool_deferred"
                    || value["terminal_reason"] == "tool_deferred"
                    || value
                        .get("deferred_tool_use")
                        .is_some_and(|pending| !pending.is_null())
                {
                    return Err(protocol_error("result-failed-or-deferred"));
                }
                self.root_success = true;
            }
            // No raw notification payload is retained. Unknown meaningful shapes fail closed.
            _ => return Err(protocol_error("unsupported-envelope")),
        }
        // Preserve observed tool order when its Agent block unlocks a lifecycle
        // event; newly resolved child operations follow their owner registration.
        self.emit_operations(&mut events);
        self.materialize(&mut events)?;
        self.emit_operations(&mut events);
        if self.root_success
            && self
                .tasks
                .values()
                .all(|task| terminal(&task.status) && task.emitted.as_ref() == Some(&task.status))
        {
            events.push(ProviderEvent::TurnCompleted);
        }
        Ok(events)
    }

    fn text_frame(
        &mut self,
        message: &str,
        frame: Option<&str>,
        block_count: usize,
    ) -> Result<Option<usize>, ProviderError> {
        if let Some(seen) = frame.and_then(|frame| self.text_frames.get(frame)) {
            if seen.message != message || seen.block_count != block_count {
                return Err(protocol_error("text-frame-identity-conflict"));
            }
            return Ok(seen.native_block);
        }
        // CLI 2.1.205 yields a one-block assistant frame before its raw block-stop
        // event. Its array index is local to that fragment, not the native index.
        let native_block = match self.stream_text_blocks.get(message) {
            Some(Some(index)) if block_count == 1 => Some(*index),
            Some(_) => return Err(protocol_error("unmapped-streamed-text-frame")),
            None => None,
        };
        if let Some(frame) = frame {
            if self.text_frames.len() >= MAX_IDENTITIES {
                return Err(protocol_error("text-frame-capacity"));
            }
            self.text_frames.insert(
                frame.into(),
                TextFrame {
                    message: message.into(),
                    block_count,
                    native_block,
                },
            );
        }
        Ok(native_block)
    }

    fn text(
        &mut self,
        id: String,
        text: &str,
        delta: bool,
        events: &mut Vec<ProviderEvent>,
    ) -> Result<(), ProviderError> {
        if !self.texts.contains_key(&id) && self.texts.len() >= MAX_IDENTITIES {
            return Err(protocol_error("text-identity-capacity"));
        }
        let seen = self.texts.entry(id.clone()).or_default();
        let suffix = if delta {
            text
        } else {
            text.strip_prefix(seen.as_str())
                .ok_or_else(|| protocol_error("conflicting-full-message-text"))?
        };
        if self.text_bytes + suffix.len() > MAX_TEXT_BYTES {
            return Err(protocol_error("text-capacity"));
        }
        if !suffix.is_empty() {
            self.text_bytes += suffix.len();
            seen.push_str(suffix);
            events.push(ProviderEvent::AssistantMessageDelta {
                native_item_id: id,
                content: suffix.into(),
            });
        }
        Ok(())
    }

    fn stream(
        &mut self,
        event: &Value,
        events: &mut Vec<ProviderEvent>,
    ) -> Result<(), ProviderError> {
        match event["type"].as_str() {
            Some("message_start") => {
                let message = required_id(&event["message"], "id")?;
                if !self.stream_text_blocks.contains_key(message)
                    && self.stream_text_blocks.len() >= MAX_IDENTITIES
                {
                    return Err(protocol_error("stream-message-capacity"));
                }
                self.stream_text_blocks.insert(message.into(), None);
                self.stream_message = Some(message.into());
            }
            Some("content_block_start" | "content_block_delta") => {
                let index: usize = event["index"]
                    .as_u64()
                    .and_then(|n| n.try_into().ok())
                    .filter(|n| *n < MAX_IDENTITIES)
                    .ok_or_else(|| protocol_error("block-index"))?;
                let message = self
                    .stream_message
                    .clone()
                    .ok_or_else(|| protocol_error("delta-without-message"))?;
                if event["type"] == "content_block_start" {
                    let block = &event["content_block"];
                    self.stream_text_blocks
                        .insert(message.clone(), (block["type"] == "text").then_some(index));
                    match block["type"].as_str() {
                        Some("text") => self.text(
                            format!("{message}:{index}"),
                            block["text"]
                                .as_str()
                                .ok_or_else(|| protocol_error("text-block"))?,
                            false,
                            events,
                        )?,
                        Some("tool_use") => self.tool(block, None)?,
                        Some("thinking" | "redacted_thinking") => {}
                        _ => return Err(protocol_error("unsupported-stream-block")),
                    }
                } else {
                    match event["delta"]["type"].as_str() {
                        Some("text_delta") => self.text(
                            format!("{message}:{index}"),
                            event["delta"]["text"]
                                .as_str()
                                .ok_or_else(|| protocol_error("text-delta"))?,
                            true,
                            events,
                        )?,
                        Some("input_json_delta" | "thinking_delta" | "signature_delta") => {}
                        _ => return Err(protocol_error("unsupported-stream-delta")),
                    }
                }
            }
            Some("message_delta") => {}
            Some("content_block_stop" | "message_stop") => {
                if let Some(message) = &self.stream_message {
                    self.stream_text_blocks.insert(message.clone(), None);
                }
                if event["type"] == "message_stop" {
                    self.stream_message = None;
                }
            }
            _ => return Err(protocol_error("unsupported-stream-event")),
        }
        Ok(())
    }

    fn tool(&mut self, block: &Value, parent: Option<&str>) -> Result<(), ProviderError> {
        let id = required_id(block, "id")?;
        let name = required_id(block, "name")?;
        if let Some(tool) = self.tools.get_mut(id) {
            if (tool.identified && tool.name != name) || tool.parent.as_deref() != parent {
                return Err(protocol_error("tool-identity-conflict"));
            }
            tool.name = name.into();
            tool.identified = true;
            self.update_operation(id, operations::input(name, &block["input"]));
            return Ok(());
        }
        if self.tools.len() >= MAX_IDENTITIES || parent == Some(id) {
            return Err(protocol_error("tool-capacity-or-cycle"));
        }
        // Reserve one bounded title for every remaining identity, so pressure
        // truncates optional detail without preventing later operation rows.
        let reserve = (MAX_IDENTITIES - self.tools.len() - 1) * MAX_OPERATION_TITLE_BYTES;
        let available = MAX_TEXT_BYTES.saturating_sub(self.operation_bytes + reserve);
        let operation = operations::retain(operations::input(name, &block["input"]), available);
        let bytes = self.operation_bytes + operation.display_bytes();
        self.tools.insert(
            id.into(),
            Tool {
                name: name.into(),
                identified: true,
                order: self.tools.len(),
                parent: parent.map(str::to_owned),
                completed: false,
                operation,
                dirty: true,
                mutation: MutationState::NoneObserved,
            },
        );
        let mut cursor = parent;
        let mut visited = HashSet::new();
        visited.insert(id);
        while let Some(parent) = cursor {
            if !visited.insert(parent) {
                return Err(protocol_error("tool-parent-cycle"));
            }
            cursor = self
                .tools
                .get(parent)
                .and_then(|tool| tool.parent.as_deref());
        }
        self.operation_bytes = bytes;
        Ok(())
    }

    fn update_operation(&mut self, id: &str, incoming: ToolOperation) {
        let tool = self.tools.get_mut(id).expect("known tool");
        let title = (tool.operation.input.is_none() && incoming.input.is_some())
            .then(|| incoming.title.clone());
        let mut merged = tool.operation.clone().merge(incoming);
        if let Some(title) = title {
            merged.title = title;
            merged = merged.bounded();
        }
        let prior_bytes = tool.operation.display_bytes();
        let reserve = (MAX_IDENTITIES - self.tools.len()) * MAX_OPERATION_TITLE_BYTES;
        let available = MAX_TEXT_BYTES.saturating_sub(self.operation_bytes - prior_bytes + reserve);
        let merged = operations::retain(merged, available);
        let tool = self.tools.get_mut(id).expect("known tool");
        let bytes = self.operation_bytes - prior_bytes + merged.display_bytes();
        self.operation_bytes = bytes;
        tool.dirty |= tool.operation != merged;
        tool.operation = merged;
    }

    fn emit_operations(&mut self, events: &mut Vec<ProviderEvent>) {
        let mut dirty: Vec<_> = self
            .tools
            .iter()
            .filter(|(_, tool)| tool.dirty)
            .map(|(id, tool)| (tool.order, id.clone()))
            .collect();
        dirty.sort_by_key(|(order, _)| *order);
        for (_, id) in dirty {
            let tool = self.tools.get_mut(&id).unwrap();
            let owner = match &tool.parent {
                None => None,
                Some(parent) => {
                    let Some((task_id, _)) = self
                        .tasks
                        .iter()
                        .find(|(_, task)| task.tool == *parent && task.emitted.is_some())
                    else {
                        continue;
                    };
                    Some(task_id.clone())
                }
            };
            events.push(ProviderEvent::NativeItemActivity {
                native_item_id: id.clone(),
                native_turn_id: None,
                native_agent_id: owner,
                description: tool.operation.title.clone(),
                operation: Some(tool.operation.clone()),
                mutation: tool.mutation,
            });
            tool.dirty = false;
        }
    }

    fn task_update(&mut self, value: &Value) -> Result<(), ProviderError> {
        required_id(value, "uuid")?;
        let task_id = required_id(value, "task_id")?;
        let task = self
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| protocol_error("unknown-task-update"))?;
        let patch = value["patch"]
            .as_object()
            .ok_or_else(|| protocol_error("invalid-task-patch"))?;
        for (key, field) in patch {
            let valid = match key.as_str() {
                "status" => matches!(
                    field.as_str(),
                    Some("pending" | "running" | "completed" | "failed" | "killed" | "paused")
                ),
                "description" | "error" => field.is_string(),
                "end_time" | "total_paused_ms" => field.is_number(),
                "is_backgrounded" => field.is_boolean(),
                _ => false,
            };
            if !valid {
                return Err(protocol_error("invalid-task-patch"));
            }
        }
        let status = match patch.get("status").and_then(Value::as_str) {
            Some("completed") => Some(NativeAgentStatus::Completed),
            Some("failed") => Some(NativeAgentStatus::Errored),
            Some("killed") => Some(NativeAgentStatus::Interrupted),
            _ => None,
        };
        if let Some(status) = status {
            if task
                .update_terminal
                .as_ref()
                .is_some_and(|prior| prior != &status)
                || (terminal(&task.status) && task.status != status)
            {
                return Err(protocol_error("conflicting-task-terminal"));
            }
            task.update_terminal = Some(status);
        }
        // Updates precede the notification bookend in CLI 2.1.205. Retain only
        // terminal consistency evidence; descriptions/errors never leave this frame.
        // The notification still owns child completion, including after root result.
        Ok(())
    }

    fn lifecycle(&mut self, value: &Value) -> Result<(), ProviderError> {
        let task_id = required_id(value, "task_id")?;
        let tool_id = required_id(value, "tool_use_id")?;
        if task_id == self.session {
            return Err(protocol_error("task-session-identity-conflict"));
        }
        let status = if value["subtype"] == "task_started" {
            NativeAgentStatus::Running
        } else {
            // failed/stopped are SDK schema variants; completed is authenticated fixture evidence.
            match value["status"].as_str() {
                Some("completed") => NativeAgentStatus::Completed,
                Some("failed") => NativeAgentStatus::Errored,
                Some("stopped") => NativeAgentStatus::Interrupted,
                _ => return Err(protocol_error("unsupported-task-status")),
            }
        };
        if self
            .tasks
            .iter()
            .any(|(id, task)| id != task_id && task.tool == tool_id)
        {
            return Err(protocol_error("task-tool-identity-conflict"));
        }
        if let Some(task) = self.tasks.get_mut(task_id) {
            if task.tool != tool_id {
                return Err(protocol_error("task-tool-identity-conflict"));
            }
            if terminal(&status)
                && task
                    .update_terminal
                    .as_ref()
                    .is_some_and(|prior| prior != &status)
            {
                return Err(protocol_error("conflicting-task-terminal"));
            }
            if terminal(&task.status) {
                if terminal(&status) && status != task.status {
                    return Err(protocol_error("conflicting-task-terminal"));
                }
            } else {
                task.status = status;
            }
        } else {
            if self.tasks.len() >= MAX_IDENTITIES {
                return Err(protocol_error("task-capacity"));
            }
            self.tasks.insert(
                task_id.into(),
                Task {
                    tool: tool_id.into(),
                    status,
                    emitted: None,
                    update_terminal: None,
                },
            );
        }
        Ok(())
    }

    fn materialize(&mut self, events: &mut Vec<ProviderEvent>) -> Result<(), ProviderError> {
        loop {
            let mut changed = false;
            let ids: Vec<_> = self.tasks.keys().cloned().collect();
            for id in ids {
                let task = &self.tasks[&id];
                if task.emitted.as_ref() == Some(&task.status) {
                    continue;
                }
                let Some(tool) = self.tools.get(&task.tool) else {
                    continue;
                };
                if tool.name != "Agent" {
                    return Err(protocol_error("task-without-agent-tool"));
                }
                let parent =
                    match &tool.parent {
                        None => self.session.clone(),
                        Some(parent_tool) => {
                            let Some((parent, _)) = self.tasks.iter().find(|(_, task)| {
                                task.tool == *parent_tool && task.emitted.is_some()
                            }) else {
                                continue;
                            };
                            parent.clone()
                        }
                    };
                if parent == id {
                    return Err(protocol_error("task-parent-cycle"));
                }
                let task = self.tasks.get_mut(&id).unwrap();
                if task.emitted.is_none() {
                    events.push(child_event(
                        &id,
                        &parent,
                        &task.tool,
                        NativeAgentStatus::Running,
                    ));
                    task.emitted = Some(NativeAgentStatus::Running);
                }
                // Pending snapshots belong to the newly registered Running
                // owner, before a terminal-first notification closes it.
                self.emit_operations(events);
                let task = self.tasks.get_mut(&id).unwrap();
                if task.emitted.as_ref() != Some(&task.status) {
                    events.push(child_event(&id, &parent, &task.tool, task.status.clone()));
                    task.emitted = Some(task.status.clone());
                }
                changed = true;
            }
            if !changed {
                return Ok(());
            }
        }
    }
}

fn terminal(status: &NativeAgentStatus) -> bool {
    matches!(
        status,
        NativeAgentStatus::Completed | NativeAgentStatus::Errored | NativeAgentStatus::Interrupted
    )
}

fn child_event(id: &str, parent: &str, tool: &str, status: NativeAgentStatus) -> ProviderEvent {
    ProviderEvent::ChildAgentActivity {
        native_item_id: tool.into(),
        parent_native_thread_id: parent.into(),
        child_native_thread_ids: vec![id.into()],
        operation: "spawn".into(),
        status: if terminal(&status) {
            "completed"
        } else {
            "inProgress"
        }
        .into(),
        child_statuses: vec![NativeChildStatus {
            native_thread_id: id.into(),
            status,
        }],
    }
}

pub(super) struct PendingControl {
    native_id: String,
    pub(super) tool_use_id: Option<String>,
    input: Value,
    questions: Option<Vec<UserInputQuestion>>,
    pub(super) claimed: bool,
    pub(super) written: bool,
}

impl PendingControl {
    pub(super) fn release_input(&mut self) {
        self.input = Value::Null;
        self.questions = None;
    }

    pub(super) fn response(&self, response: ApprovalResponse) -> Result<Value, ProviderError> {
        if response == ApprovalResponse::Denied {
            return Ok(reply(
                &self.native_id,
                json!({"behavior":"deny","message":"User denied; do not retry"}),
            ));
        }
        let mut input = self.input.clone();
        match (&self.questions, response) {
            (None, ApprovalResponse::Approved) => {}
            (Some(questions), response) => {
                let answers = match response {
                    ApprovalResponse::Answers(answers) => answers,
                    ApprovalResponse::Answer(answer) if questions.len() == 1 => {
                        BTreeMap::from([(questions[0].id.clone(), vec![answer])])
                    }
                    _ => return Err(rejected()),
                };
                if answers.len() != questions.len() {
                    return Err(rejected());
                }
                let mut native = serde_json::Map::new();
                for question in questions {
                    let answer = answers
                        .get(&question.id)
                        .filter(|answers| answers.len() == 1)
                        .and_then(|answers| answers.first())
                        .filter(|answer| {
                            !answer.trim().is_empty() && answer.len() <= MAX_INPUT_BYTES
                        })
                        .ok_or_else(rejected)?;
                    native.insert(question.question.clone(), json!(answer));
                }
                input["answers"] = Value::Object(native);
            }
            _ => return Err(rejected()),
        }
        Ok(reply(
            &self.native_id,
            json!({"behavior":"allow","updatedInput":input}),
        ))
    }
}

fn reply(id: &str, value: Value) -> Value {
    json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":value}})
}

pub(super) fn control(
    value: &Value,
    request_id: String,
) -> Result<(PendingControl, Option<ProviderEvent>, Option<Value>), ProviderError> {
    let native_id = required_id(value, "request_id")?.to_owned();
    let request = &value["request"];
    let mut pending = PendingControl {
        native_id: native_id.clone(),
        tool_use_id: None,
        input: Value::Null,
        questions: None,
        claimed: false,
        written: false,
    };
    if request["subtype"] != "can_use_tool" {
        pending.claimed = true;
        let reply = json!({"type":"control_response","response":{"subtype":"error","request_id":native_id,"error":"Unsupported control request in Prompting Time"}});
        return Ok((pending, None, Some(reply)));
    }
    pending.tool_use_id = Some(required_id(request, "tool_use_id")?.into());
    let tool = required_id(request, "tool_name")?;
    let input = &request["input"];
    if !input.is_object()
        || serde_json::to_vec(input)
            .map_err(|_| protocol_error("tool-input"))?
            .len()
            > MAX_INPUT_BYTES
    {
        return Err(protocol_error("tool-input-capacity-or-shape"));
    }
    pending.input = input.clone();
    if tool == "AskUserQuestion" {
        let questions = input["questions"]
            .as_array()
            .filter(|questions| !questions.is_empty() && questions.len() <= 8)
            .ok_or_else(|| protocol_error("question-count"))?;
        let mut normalized = Vec::new();
        let mut texts = HashSet::new();
        let mut multiple = false;
        for (index, question) in questions.iter().enumerate() {
            let text = string(question, "question")?;
            if !texts.insert(text) {
                return Err(protocol_error("duplicate-question-text"));
            }
            multiple |= question["multiSelect"]
                .as_bool()
                .ok_or_else(|| protocol_error("question-selection-shape"))?;
            let mut labels = HashSet::new();
            let options = match question.get("options") {
                None => None,
                Some(options) => {
                    let options = options
                        .as_array()
                        .filter(|options| !options.is_empty() && options.len() <= 32)
                        .ok_or_else(|| protocol_error("question-options"))?;
                    Some(
                        options
                            .iter()
                            .map(|option| {
                                let label = string(option, "label")?;
                                if !labels.insert(label) {
                                    return Err(protocol_error("duplicate-question-option"));
                                }
                                Ok(UserInputOption {
                                    label: label.into(),
                                    description: option
                                        .get("description")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .into(),
                                })
                            })
                            .collect::<Result<Vec<_>, ProviderError>>()?,
                    )
                }
            };
            normalized.push(UserInputQuestion {
                id: format!("{request_id}:{index}"),
                header: question
                    .get("header")
                    .and_then(Value::as_str)
                    .unwrap_or("Question")
                    .into(),
                question: text.into(),
                options,
                is_other: true,
                is_secret: false,
            });
        }
        if multiple {
            pending.claimed = true;
            return Ok((
                pending,
                None,
                Some(reply(
                    &native_id,
                    json!({"behavior":"deny","message":"Prompting Time supports single-select questions and free text. Please ask single-select questions."}),
                )),
            ));
        }
        pending.questions = Some(normalized.clone());
        return Ok((
            pending,
            Some(ProviderEvent::UserInputRequested {
                request_id,
                questions: normalized,
                auto_resolution_ms: None,
            }),
            None,
        ));
    }
    let details = match tool {
        "Bash" => Some(ApprovalRequestDetails::CommandExecution {
            command: Some(string(input, "command")?.into()),
            cwd: input.get("cwd").and_then(Value::as_str).map(str::to_owned),
        }),
        "Write" | "Edit" => Some(ApprovalRequestDetails::FileChange {
            changes: vec![FileChangeApprovalDetail {
                path: string(input, "file_path")?.into(),
                change: FileChangeKind::Update { move_path: None },
            }],
            grant_root: None,
            reason: None,
        }),
        _ => None,
    };
    let scope = input
        .get("file_path")
        .and_then(Value::as_str)
        .unwrap_or("Current Claude session workspace")
        .to_owned();
    Ok((
        pending,
        Some(ProviderEvent::ApprovalRequested {
            request_id,
            operation: tool.into(),
            scope,
            details,
        }),
        None,
    ))
}

#[cfg(test)]
mod advisory_tests {
    use super::*;

    fn notices() -> [Value; 3] {
        [
            json!({"type":"system", "subtype":"api_retry", "session_id":"session", "uuid":"retry",
                "attempt":1, "max_retries":0, "retry_delay_ms":123.5, "error_status":null, "error":"unknown"}),
            json!({"type":"system", "subtype":"notification", "session_id":"session", "uuid":"notification",
                "key":"", "text":"", "priority":"low"}),
            json!({"type":"system", "subtype":"memory_recall", "session_id":"session", "uuid":"memory",
                "mode":"select", "memories":[]}),
        ]
    }

    #[test]
    fn native_advisories_discard_private_fields_and_accept_native_variants() {
        let mut protocol = Protocol::new("session".into());
        for notice in notices() {
            assert!(protocol.normalize(notice).unwrap().is_empty());
        }
        for error in [
            "authentication_failed",
            "oauth_org_not_allowed",
            "billing_error",
            "rate_limit",
            "overloaded",
            "invalid_request",
            "model_not_found",
            "server_error",
            "unknown",
            "max_output_tokens",
        ] {
            let mut notice = notices()[0].clone();
            notice["error"] = json!(error);
            // Native watchdogs repeat an attempt with decreasing, fractional delays.
            // Their counter can exceed max_retries, including max_retries=0.
            for delay in [123.5, 1.5, 0.0] {
                notice["retry_delay_ms"] = json!(delay);
                notice["error_status"] = json!(503);
                assert!(protocol.normalize(notice.clone()).unwrap().is_empty());
            }
        }
        for priority in ["low", "medium", "high", "immediate"] {
            let mut notice = notices()[1].clone();
            notice["priority"] = json!(priority);
            notice["text"] = json!("PRIVATE_NOTICE");
            notice["color"] = json!("PRIVATE_COLOR");
            notice["timeout_ms"] = json!(0.5);
            assert!(protocol.normalize(notice).unwrap().is_empty());
        }
        for mode in ["select", "synthesize"] {
            let mut notice = notices()[2].clone();
            notice["mode"] = json!(mode);
            notice["parent_tool_use_id"] = json!("child-tool");
            notice["memories"] = json!([
                {"path":"PRIVATE_PATH", "scope":"personal"},
                {"path":"PRIVATE_PATH", "scope":"team", "content":"PRIVATE_CONTENT"},
                {"path":"PRIVATE_PATH", "scope":"organization", "content":"PRIVATE_CONTENT"}
            ]);
            assert!(protocol.normalize(notice).unwrap().is_empty());
        }
    }

    #[test]
    fn native_advisories_reject_missing_or_malformed_payload_fields() {
        let required = [
            vec![
                "attempt",
                "max_retries",
                "retry_delay_ms",
                "error_status",
                "error",
            ],
            vec!["key", "text", "priority"],
            vec!["mode", "memories"],
        ];
        for (notice, fields) in notices().into_iter().zip(required) {
            for field in fields {
                for missing in [false, true] {
                    let mut invalid = notice.clone();
                    if missing {
                        invalid.as_object_mut().unwrap().remove(field);
                    } else {
                        invalid[field] = json!(true);
                    }
                    assert_eq!(
                        Protocol::new("session".into())
                            .normalize(invalid)
                            .unwrap_err(),
                        protocol_error("invalid-system-advisory"),
                        "{field}, missing={missing}"
                    );
                }
            }
        }
        for (index, patch) in [
            (0, json!({"error":"unrecognized-error"})),
            (1, json!({"priority":"unrecognized-priority"})),
            (1, json!({"color":null})),
            (1, json!({"timeout_ms":"5000"})),
            (2, json!({"mode":"unrecognized-mode"})),
            (2, json!({"memories":[{}]})),
            (2, json!({"memories":[{"path":1,"scope":"personal"}]})),
            (
                2,
                json!({"memories":[{"path":"invented","scope":"unrecognized-scope"}]}),
            ),
            (
                2,
                json!({"memories":[{"path":"invented","scope":"personal","content":null}]}),
            ),
        ] {
            let mut invalid = notices()[index].clone();
            invalid
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert_eq!(
                Protocol::new("session".into())
                    .normalize(invalid)
                    .unwrap_err(),
                protocol_error("invalid-system-advisory")
            );
        }
    }
}

#[cfg(test)]
mod operation_tests {
    use super::*;
    use crate::tool_operation::ToolOperationStatus;

    fn operation(events: &[ProviderEvent]) -> &crate::tool_operation::ToolOperation {
        events
            .iter()
            .find_map(|event| match event {
                ProviderEvent::NativeItemActivity { operation, .. } => operation.as_ref(),
                _ => None,
            })
            .expect("selected tool operation")
    }

    #[test]
    fn claude_operation_enriches_streamed_input_and_result() {
        let mut protocol = Protocol::new("session".into());
        protocol.normalize(json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"message"}}})).unwrap();
        let start = protocol
            .normalize(
                json!({"type":"stream_event","event":{"type":"content_block_start","index":0,
            "content_block":{"type":"tool_use","id":"tool","name":"Bash","input":{}}}}),
            )
            .unwrap();
        assert_eq!(operation(&start).title, "Bash");
        let frame = json!({"type":"assistant","message":{"id":"message","content":[{"type":"tool_use","id":"tool","name":"Bash","input":{"command":"cargo test","private":"PRIVATE_MARKER"}}]}});
        let full = protocol.normalize(frame.clone()).unwrap();
        assert_eq!(operation(&full).title, "Run cargo test");
        assert_eq!(operation(&full).input.as_deref(), Some("cargo test"));
        assert!(protocol.normalize(frame).unwrap().is_empty());
        let result = json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tool","is_error":true,"content":""}]}});
        let end = protocol.normalize(result.clone()).unwrap();
        assert_eq!(operation(&end).status, ToolOperationStatus::Failed);
        assert_eq!(operation(&end).output.as_deref(), Some(""));
        assert!(
            !serde_json::to_string(&end)
                .unwrap()
                .contains("PRIVATE_MARKER")
        );
        assert!(protocol.normalize(result).unwrap().is_empty());
    }

    fn tool_frame(id: &str, name: &str, input: Value, parent: Option<&str>) -> Value {
        json!({"type":"assistant","parent_tool_use_id":parent,"message":{"id":"message","content":[{"type":"tool_use","id":id,"name":name,"input":input}]}})
    }

    #[test]
    fn claude_operation_selected_file_and_search_inputs() {
        for (name, input, title) in [
            (
                "Read",
                json!({"file_path":"src/main.rs"}),
                "Read src/main.rs",
            ),
            (
                "Write",
                json!({"file_path":"src/main.rs","content":"NEW"}),
                "Write src/main.rs",
            ),
            (
                "Edit",
                json!({"file_path":"src/main.rs","old_string":"OLD","new_string":"NEW"}),
                "Edit src/main.rs",
            ),
            (
                "Glob",
                json!({"pattern":"*.rs","path":"src"}),
                "Find files *.rs",
            ),
            (
                "Grep",
                json!({"pattern":"routing","path":"src"}),
                "Search routing",
            ),
            (
                "WebSearch",
                json!({"query":"rust docs"}),
                "Search rust docs",
            ),
            (
                "WebFetch",
                json!({"url":"https://example.com"}),
                "Fetch https://example.com",
            ),
            (
                "NotebookEdit",
                json!({"notebook_path":"example.ipynb"}),
                "Edit example.ipynb",
            ),
        ] {
            let mut protocol = Protocol::new("session".into());
            let events = protocol
                .normalize(tool_frame("tool", name, input, None))
                .unwrap();
            assert_eq!(operation(&events).title, title);
            if name == "Edit" {
                assert_eq!(
                    operation(&events).input.as_deref(),
                    Some("Old text:\nOLD\nNew text:\nNEW")
                );
            }
            if name == "Write" {
                assert_eq!(operation(&events).input.as_deref(), Some("NEW"));
            }
        }
    }

    #[test]
    fn claude_operation_missing_empty_and_text_block_results() {
        for (content, expected) in [
            (None, None),
            (Some(json!("")), Some("")),
            (Some(json!([])), Some("")),
            (
                Some(
                    json!([{"type":"text","text":"A"},{"type":"image","data":"PRIVATE"},{"type":"text","text":"B"}]),
                ),
                Some("A\nB"),
            ),
            (Some(json!([{"type":"image","data":"PRIVATE"}])), None),
        ] {
            let mut protocol = Protocol::new("session".into());
            protocol
                .normalize(tool_frame(
                    "tool",
                    "Unfamiliar",
                    json!({"secret":"PRIVATE"}),
                    None,
                ))
                .unwrap();
            let mut block = json!({"type":"tool_result","tool_use_id":"tool"});
            if let Some(content) = content {
                block["content"] = content;
            }
            let events = protocol
                .normalize(json!({"type":"user","message":{"content":[block]}}))
                .unwrap();
            assert_eq!(operation(&events).output.as_deref(), expected);
            assert_eq!(operation(&events).status, ToolOperationStatus::Succeeded);
            assert!(!serde_json::to_string(&events).unwrap().contains("PRIVATE"));
        }
    }

    #[test]
    fn claude_operation_completion_only_and_late_input() {
        let mut protocol = Protocol::new("session".into());
        let events = protocol.normalize(json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"missing","content":"DONE"}]}})).unwrap();
        assert_eq!(operation(&events).status, ToolOperationStatus::Succeeded);
        let events = protocol
            .normalize(tool_frame(
                "missing",
                "Bash",
                json!({"command":"cargo test"}),
                None,
            ))
            .unwrap();
        assert_eq!(operation(&events).input.as_deref(), Some("cargo test"));
        assert_eq!(operation(&events).title, "Run cargo test");
        assert_eq!(operation(&events).status, ToolOperationStatus::Succeeded);
    }

    #[test]
    fn claude_operation_child_waits_for_materialized_owner_but_keeps_mutation() {
        let mut protocol = Protocol::new("session".into());
        protocol
            .normalize(tool_frame("agent", "Agent", json!({}), None))
            .unwrap();
        let start = protocol
            .normalize(tool_frame(
                "write",
                "Write",
                json!({"file_path":"child.txt","content":"NEW"}),
                Some("agent"),
            ))
            .unwrap();
        assert!(
            start
                .iter()
                .all(|event| !matches!(event, ProviderEvent::NativeItemActivity { .. }))
        );
        let end = protocol.normalize(json!({"type":"user","parent_tool_use_id":"agent","message":{"content":[{"type":"tool_result","tool_use_id":"write","is_error":false,"content":"DONE"}]}})).unwrap();
        assert!(end.iter().any(|event| matches!(
            event,
            ProviderEvent::MutationEvidence {
                mutation: MutationState::Observed
            }
        )));
        assert!(
            end.iter()
                .all(|event| !matches!(event, ProviderEvent::NativeItemActivity { .. }))
        );
        let mapped = protocol.normalize(json!({"type":"system","subtype":"task_started","task_id":"child","tool_use_id":"agent"})).unwrap();
        let ProviderEvent::NativeItemActivity {
            native_agent_id,
            native_turn_id,
            operation: Some(op),
            ..
        } = mapped
            .iter()
            .find(|event| matches!(event, ProviderEvent::NativeItemActivity { .. }))
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(native_agent_id.as_deref(), Some("child"));
        assert!(native_turn_id.is_none());
        assert_eq!(op.output.as_deref(), Some("DONE"));
        assert!(matches!(
            mapped.first(),
            Some(ProviderEvent::ChildAgentActivity { .. })
        ));
    }

    #[test]
    fn claude_operation_bounds_retention_and_omits_thinking() {
        let mut protocol = Protocol::new("session".into());
        let private = protocol.normalize(json!({"type":"assistant","message":{"id":"message","content":[{"type":"thinking","thinking":"PRIVATE"},{"type":"redacted_thinking","data":"PRIVATE"}]}})).unwrap();
        assert!(private.is_empty());
        let long = "é".repeat(crate::tool_operation::MAX_OPERATION_BYTES);
        for index in 0..40 {
            let events = protocol
                .normalize(tool_frame(
                    &format!("tool-{index}"),
                    "Bash",
                    json!({"command":long}),
                    None,
                ))
                .unwrap();
            let op = operation(&events);
            assert!(op.truncated);
            assert!(op.display_bytes() <= crate::tool_operation::MAX_OPERATION_BYTES);
        }
        assert!(protocol.operation_bytes <= MAX_TEXT_BYTES);
    }

    #[test]
    fn claude_operation_pressure_does_not_fail_execution_or_pending_children() {
        for pending in [false, true] {
            let mut protocol = Protocol::new("session".into());
            if pending {
                protocol
                    .normalize(tool_frame("agent", "Agent", json!({}), None))
                    .unwrap();
            }
            let long = "é".repeat(crate::tool_operation::MAX_OPERATION_BYTES / 2);
            for index in 0..40 {
                let id = format!("tool-{index}");
                let parent = pending.then_some("agent");
                protocol
                    .normalize(tool_frame(&id, "Bash", json!({"command":"check"}), parent))
                    .unwrap();
                let frame = json!({"type":"user","parent_tool_use_id":parent,"message":{"content":[{"type":"tool_result","tool_use_id":id,"content":long}]}});
                let events = protocol.normalize(frame.clone()).unwrap();
                assert!(events.iter().any(|event| matches!(
                    event,
                    ProviderEvent::MutationEvidence {
                        mutation: MutationState::Unknown
                    }
                )));
                if pending {
                    assert!(
                        events.iter().all(|event| !matches!(
                            event,
                            ProviderEvent::NativeItemActivity { .. }
                        ))
                    );
                } else {
                    assert_eq!(operation(&events).status, ToolOperationStatus::Succeeded);
                }
                assert!(
                    protocol.normalize(frame).unwrap().is_empty(),
                    "duplicate pressure observations stay idempotent"
                );
                assert!(protocol.operation_bytes <= MAX_TEXT_BYTES);
                let allocated: usize = protocol
                    .tools
                    .values()
                    .map(|tool| {
                        tool.operation.title.capacity()
                            + [
                                &tool.operation.input,
                                &tool.operation.output,
                                &tool.operation.error,
                                &tool.operation.context,
                            ]
                            .into_iter()
                            .flatten()
                            .map(String::capacity)
                            .sum::<usize>()
                    })
                    .sum();
                assert!(
                    allocated <= MAX_TEXT_BYTES,
                    "truncated text must release its allocation"
                );
            }
            let conflict = protocol.normalize(json!({"type":"user","parent_tool_use_id":pending.then_some("agent"),"message":{"content":[{"type":"tool_result","tool_use_id":"tool-39","is_error":true,"content":"broken"}]}})).unwrap();
            if !pending {
                assert_eq!(operation(&conflict).status, ToolOperationStatus::Unknown);
                assert!(operation(&conflict).conflicted);
                assert_eq!(operation(&conflict).error.as_deref(), Some("broken"));
            }
            if pending {
                let events = protocol.normalize(json!({"type":"system","subtype":"task_notification","task_id":"child","tool_use_id":"agent","status":"completed"})).unwrap();
                let snapshots: Vec<_> = events
                    .iter()
                    .filter_map(|event| match event {
                        ProviderEvent::NativeItemActivity {
                            native_agent_id,
                            operation: Some(operation),
                            ..
                        } => Some((native_agent_id, operation)),
                        _ => None,
                    })
                    .collect();
                assert_eq!(snapshots.len(), 40);
                assert!(
                    snapshots
                        .iter()
                        .all(|(owner, _)| owner.as_deref() == Some("child"))
                );
                assert!(snapshots.iter().any(|(_, operation)| operation.truncated));
            }
            let end = protocol.normalize(json!({"type":"result","session_id":"session","subtype":"success","is_error":false,"result":"done"})).unwrap();
            assert!(matches!(end.last(), Some(ProviderEvent::TurnCompleted)));
        }
    }

    #[test]
    fn claude_operation_conflicting_results_keep_failure_and_same_identity() {
        let mut protocol = Protocol::new("session".into());
        protocol
            .normalize(tool_frame("tool", "Bash", json!({"command":"false"}), None))
            .unwrap();
        for (failed, output) in [(true, "broken"), (false, "ok")] {
            let events = protocol.normalize(json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tool","is_error":failed,"content":output}]}})).unwrap();
            if !failed {
                assert!(operation(&events).conflicted);
                assert_eq!(operation(&events).status, ToolOperationStatus::Unknown);
                assert_eq!(operation(&events).error.as_deref(), Some("broken"));
            }
        }
    }

    #[test]
    fn claude_operation_multi_tool_order_and_ownership_conflicts() {
        let mut protocol = Protocol::new("session".into());
        let blocks: Vec<_> = (0..30).map(|index| json!({"type":"tool_use","id":format!("tool-{index}"),"name":"Read","input":{"file_path":"same.rs"}})).collect();
        let events = protocol
            .normalize(json!({"type":"assistant","message":{"id":"message","content":blocks}}))
            .unwrap();
        let ids: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                ProviderEvent::NativeItemActivity { native_item_id, .. } => {
                    Some(native_item_id.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            ids,
            (0..30)
                .map(|index| format!("tool-{index}"))
                .collect::<Vec<_>>()
        );
        assert!(
            protocol
                .normalize(tool_frame("tool-0", "Read", json!({}), Some("unrelated")))
                .is_err()
        );
    }

    #[test]
    fn claude_operation_result_cannot_change_owner_or_invent_success() {
        let mut protocol = Protocol::new("session".into());
        protocol
            .normalize(tool_frame(
                "tool",
                "Read",
                json!({"file_path":"file"}),
                None,
            ))
            .unwrap();
        let events = protocol.normalize(json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tool","is_error":"no"}]}})).unwrap();
        assert_eq!(operation(&events).status, ToolOperationStatus::Unknown);
        assert!(protocol.normalize(json!({"type":"user","parent_tool_use_id":"different","message":{"content":[{"type":"tool_result","tool_use_id":"tool","content":"WRONG"}]}})).is_err());
    }

    #[test]
    fn claude_operation_bounds_collection_work() {
        let mut protocol = Protocol::new("session".into());
        let blocks = vec![
            json!({"type":"tool_use","id":"same","name":"Read","input":{}});
            MAX_IDENTITIES + 1
        ];
        assert!(
            protocol
                .normalize(json!({"type":"assistant","message":{"id":"message","content":blocks}}))
                .is_err()
        );
        let mut protocol = Protocol::new("session".into());
        let blocks = vec![
            json!({"type":"tool_result","tool_use_id":"same","content":""});
            MAX_IDENTITIES + 1
        ];
        assert!(
            protocol
                .normalize(json!({"type":"user","message":{"content":blocks}}))
                .is_err()
        );
    }

    #[test]
    fn claude_operation_keeps_position_between_assistant_blocks() {
        let mut protocol = Protocol::new("session".into());
        let events = protocol
            .normalize(
                json!({"type":"assistant","message":{"id":"message","content":[
                    {"type":"text","text":"Before"},
                    {"type":"tool_use","id":"tool","name":"Read","input":{"file_path":"file"}},
                    {"type":"text","text":"After"}
                ]}}),
            )
            .unwrap();
        assert!(
            matches!(&events[0],ProviderEvent::AssistantMessageDelta { content, .. } if content == "Before")
        );
        assert!(matches!(
            &events[1],
            ProviderEvent::NativeItemActivity { .. }
        ));
        assert!(
            matches!(&events[2],ProviderEvent::AssistantMessageDelta { content, .. } if content == "After")
        );
    }
}
