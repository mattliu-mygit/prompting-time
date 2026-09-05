use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Value, json};

use super::{protocol_error, rejected};
use crate::domain::MutationState;
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

struct Tool {
    name: String,
    parent: Option<String>,
    completed: bool,
}

struct Task {
    tool: String,
    status: NativeAgentStatus,
    emitted: Option<NativeAgentStatus>,
}

pub(super) struct Protocol {
    session: String,
    stream_message: Option<String>,
    texts: HashMap<String, String>,
    text_bytes: usize,
    tools: HashMap<String, Tool>,
    tasks: BTreeMap<String, Task>,
    root_success: bool,
}

impl Protocol {
    pub(super) fn new(session: String) -> Self {
        Self {
            session,
            stream_message: None,
            texts: HashMap::new(),
            text_bytes: 0,
            tools: HashMap::new(),
            tasks: BTreeMap::new(),
            root_success: false,
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
                for (index, block) in content.iter().enumerate() {
                    match block["type"].as_str() {
                        Some("text") if parent.is_none() => self.text(
                            message_id,
                            index,
                            block["text"]
                                .as_str()
                                .ok_or_else(|| protocol_error("text-block"))?,
                            false,
                            &mut events,
                        )?,
                        Some("tool_use") => self.tool(block, parent, &mut events)?,
                        Some("text" | "thinking" | "redacted_thinking") => {}
                        _ => return Err(protocol_error("unsupported-assistant-block")),
                    }
                }
            }
            Some("user") => {
                if let Some(content) = value["message"]["content"].as_array() {
                    for block in content {
                        if block["type"] != "tool_result" {
                            continue;
                        }
                        let id = required_id(block, "tool_use_id")?;
                        let Some(tool) = self.tools.get_mut(id) else {
                            return Err(protocol_error("unknown-tool-result"));
                        };
                        if tool.completed {
                            continue;
                        }
                        tool.completed = true;
                        let mutation =
                            if matches!(tool.name.as_str(), "Write" | "Edit" | "NotebookEdit")
                                && block["is_error"] == false
                            {
                                MutationState::Observed
                            } else {
                                MutationState::Unknown
                            };
                        events.push(ProviderEvent::NativeItemActivity {
                            native_item_id: id.into(),
                            description: format!("{} finished", tool.name),
                            mutation,
                        });
                    }
                }
            }
            Some("system") => match value["subtype"].as_str() {
                Some("task_started" | "task_notification") => self.lifecycle(&value)?,
                Some("task_updated") => {
                    return Err(protocol_error(
                        "unsupported-task-updated-lifecycle-update-adapter",
                    ));
                }
                Some(
                    "init"
                    | "status"
                    | "task_progress"
                    | "compact_boundary"
                    | "hook_started"
                    | "hook_progress"
                    | "hook_response"
                    | "session_state_changed",
                ) => {}
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
        self.materialize(&mut events)?;
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

    fn text(
        &mut self,
        message: &str,
        index: usize,
        text: &str,
        delta: bool,
        events: &mut Vec<ProviderEvent>,
    ) -> Result<(), ProviderError> {
        let id = format!("{message}:{index}");
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
                self.stream_message = Some(required_id(&event["message"], "id")?.into())
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
                    match block["type"].as_str() {
                        Some("text") => self.text(
                            &message,
                            index,
                            block["text"]
                                .as_str()
                                .ok_or_else(|| protocol_error("text-block"))?,
                            false,
                            events,
                        )?,
                        Some("tool_use") => self.tool(block, None, events)?,
                        Some("thinking" | "redacted_thinking") => {}
                        _ => return Err(protocol_error("unsupported-stream-block")),
                    }
                } else {
                    match event["delta"]["type"].as_str() {
                        Some("text_delta") => self.text(
                            &message,
                            index,
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
            Some("message_delta" | "content_block_stop") => {}
            Some("message_stop") => self.stream_message = None,
            _ => return Err(protocol_error("unsupported-stream-event")),
        }
        Ok(())
    }

    fn tool(
        &mut self,
        block: &Value,
        parent: Option<&str>,
        events: &mut Vec<ProviderEvent>,
    ) -> Result<(), ProviderError> {
        let id = required_id(block, "id")?;
        let name = required_id(block, "name")?;
        if let Some(tool) = self.tools.get(id) {
            if tool.name != name || tool.parent.as_deref() != parent {
                return Err(protocol_error("tool-identity-conflict"));
            }
            return Ok(());
        }
        if self.tools.len() >= MAX_IDENTITIES || parent == Some(id) {
            return Err(protocol_error("tool-capacity-or-cycle"));
        }
        self.tools.insert(
            id.into(),
            Tool {
                name: name.into(),
                parent: parent.map(str::to_owned),
                completed: false,
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
        events.push(ProviderEvent::NativeItemActivity {
            native_item_id: id.into(),
            description: format!("{name} requested"),
            mutation: MutationState::NoneObserved,
        });
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
