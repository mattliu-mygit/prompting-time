#[cfg(test)]
mod tests {
    use crate::providers::ProviderId;

    use super::*;

    #[test]
    fn waiting_descendant_rolls_attention_to_root() {
        let run = RunId::new();
        let root = AgentNode::root(run, ProviderId::Codex, "orchestrator");
        let child = AgentNode::child(
            run,
            root.id,
            ProviderId::Claude,
            "reviewer",
            AgentStatus::Waiting,
        );
        let grandchild = AgentNode::child(
            run,
            child.id,
            ProviderId::Claude,
            "researcher",
            AgentStatus::Running,
        );

        assert_eq!(
            roll_up_status(root.id, &[root, child, grandchild]).unwrap(),
            RollupStatus::NeedsAttention
        );
    }

    #[test]
    fn completed_run_cannot_return_to_running() {
        let mut run = ProviderRun::new(ConversationId::new(), ProviderId::Codex);

        run.transition(RunStatus::Running).unwrap();
        run.transition(RunStatus::Completed).unwrap();

        assert!(matches!(
            run.transition(RunStatus::Running),
            Err(DomainError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn approval_can_be_resolved_once() {
        let mut approval = Approval::new(
            RunId::new(),
            AgentId::new(),
            ProviderId::Claude,
            "native-request-42",
            "write a file",
            "this operation",
        );

        approval
            .resolve(ApprovalResolution::Answer("only this directory".to_owned()))
            .unwrap();

        assert_eq!(approval.status, ApprovalStatus::Answered);
        assert_eq!(
            approval.resolution,
            Some(ApprovalResolution::Answer("only this directory".to_owned()))
        );
        assert!(matches!(
            approval.resolve(ApprovalResolution::Denied),
            Err(DomainError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn domain_serializes_fields_as_camel_case() {
        let agent = AgentNode::child(
            RunId::new(),
            AgentId::new(),
            ProviderId::Codex,
            "worker",
            AgentStatus::Running,
        );

        let value = serde_json::to_value(agent).unwrap();

        assert!(value.get("runId").is_some());
        assert!(value.get("parentId").is_some());
        assert!(value.get("run_id").is_none());
        assert_eq!(
            serde_json::to_string(&ProviderId::Codex).unwrap(),
            "\"codex\""
        );
        assert_eq!(
            serde_json::from_str::<ProviderId>("\"claude\"").unwrap(),
            ProviderId::Claude
        );

        let approval = Approval::new(
            RunId::new(),
            AgentId::new(),
            ProviderId::Codex,
            "native-request-42",
            "write",
            "fixture.txt",
        );
        let approval_value = serde_json::to_value(approval).unwrap();
        assert_eq!(approval_value["providerRequestId"], "native-request-42");
        assert!(approval_value.get("provider_request_id").is_none());
    }

    #[test]
    fn workspace_serializes_durable_worktree_base_commit() {
        let workspace = Workspace {
            id: WorkspaceId::new(),
            conversation_id: ConversationId::new(),
            project_root: Some(std::path::PathBuf::from("/project")),
            execution_path: std::path::PathBuf::from("/worktree"),
            owned_worktree: true,
            worktree_base_commit: Some("0123456789abcdef".to_owned()),
        };

        let value = serde_json::to_value(workspace).unwrap();

        assert_eq!(value["worktreeBaseCommit"], "0123456789abcdef");
        assert!(value.get("worktree_base_commit").is_none());
    }

    #[test]
    fn missing_parent_is_rejected() {
        let run = RunId::new();
        let root = AgentNode::root(run, ProviderId::Codex, "orchestrator");
        let child = AgentNode::child(
            run,
            AgentId::new(),
            ProviderId::Claude,
            "reviewer",
            AgentStatus::Queued,
        );

        assert!(matches!(
            roll_up_status(root.id, &[root, child]),
            Err(DomainError::MissingParent { .. })
        ));
    }

    #[test]
    fn two_node_cycle_is_rejected() {
        let run = RunId::new();
        let mut first = AgentNode::root(run, ProviderId::Codex, "first");
        let mut second = AgentNode::root(run, ProviderId::Claude, "second");
        first.parent_id = Some(second.id);
        second.parent_id = Some(first.id);

        assert!(matches!(
            roll_up_status(first.id, &[first, second]),
            Err(DomainError::AgentCycle { .. })
        ));
    }

    #[test]
    fn child_from_another_run_is_rejected() {
        let root = AgentNode::root(RunId::new(), ProviderId::Codex, "orchestrator");
        let child = AgentNode::child(
            RunId::new(),
            root.id,
            ProviderId::Claude,
            "reviewer",
            AgentStatus::Running,
        );

        assert!(matches!(
            roll_up_status(root.id, &[root, child]),
            Err(DomainError::ParentRunMismatch { .. })
        ));
    }
}
use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::providers::{DispatchCertainty, ProviderId};

pub use crate::error::DomainError;

macro_rules! define_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            pub fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

define_id!(ConversationId);
define_id!(MessageId);
define_id!(RunId);
define_id!(AgentId);
define_id!(TimelineEventId);
define_id!(ApprovalId);
define_id!(WorkspaceId);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RunStatus {
    Queued,
    Running,
    Waiting,
    Completed,
    Interrupted,
    Failed,
}

impl RunStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Completed => "completed",
            Self::Interrupted => "interrupted",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentStatus {
    Queued,
    Running,
    Waiting,
    Completed,
    Interrupted,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MutationState {
    NoneObserved,
    Observed,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RollupStatus {
    NeedsAttention,
    Active,
    Failed,
    Interrupted,
    Completed,
}

impl RollupStatus {
    fn precedence(self) -> u8 {
        match self {
            Self::NeedsAttention => 5,
            Self::Active => 4,
            Self::Failed => 3,
            Self::Interrupted => 2,
            Self::Completed => 1,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: ConversationId,
    pub title: String,
    pub workspace_id: Option<WorkspaceId>,
    pub archived: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageRole {
    User,
    Assistant,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: MessageId,
    pub conversation_id: ConversationId,
    pub run_id: Option<RunId>,
    pub sequence: u64,
    pub role: MessageRole,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRun {
    pub id: RunId,
    pub conversation_id: ConversationId,
    pub provider: ProviderId,
    pub fallback_from_run_id: Option<RunId>,
    pub native_session_id: Option<String>,
    pub status: RunStatus,
    pub mutation_state: MutationState,
    pub dispatch_certainty: Option<DispatchCertainty>,
}

impl ProviderRun {
    pub fn new(conversation_id: ConversationId, provider: ProviderId) -> Self {
        Self {
            id: RunId::new(),
            conversation_id,
            provider,
            fallback_from_run_id: None,
            native_session_id: None,
            status: RunStatus::Queued,
            mutation_state: MutationState::NoneObserved,
            dispatch_certainty: None,
        }
    }

    pub fn transition(&mut self, to: RunStatus) -> Result<(), DomainError> {
        let from = self.status;
        match (from, to) {
            (
                RunStatus::Queued,
                RunStatus::Running | RunStatus::Interrupted | RunStatus::Failed,
            )
            | (
                RunStatus::Running,
                RunStatus::Waiting
                | RunStatus::Completed
                | RunStatus::Interrupted
                | RunStatus::Failed,
            )
            | (
                RunStatus::Waiting,
                RunStatus::Running | RunStatus::Interrupted | RunStatus::Failed,
            ) => {
                self.status = to;
                Ok(())
            }
            _ => Err(DomainError::InvalidTransition {
                entity: "provider run",
                from: from.label(),
                to: to.label(),
            }),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentNode {
    pub id: AgentId,
    pub run_id: RunId,
    pub parent_id: Option<AgentId>,
    pub provider: ProviderId,
    pub label: String,
    pub status: AgentStatus,
}

impl AgentNode {
    pub fn root(run_id: RunId, provider: ProviderId, label: impl Into<String>) -> Self {
        Self {
            id: AgentId::new(),
            run_id,
            parent_id: None,
            provider,
            label: label.into(),
            status: AgentStatus::Queued,
        }
    }

    pub fn child(
        run_id: RunId,
        parent_id: AgentId,
        provider: ProviderId,
        label: impl Into<String>,
        status: AgentStatus,
    ) -> Self {
        Self {
            id: AgentId::new(),
            run_id,
            parent_id: Some(parent_id),
            provider,
            label: label.into(),
            status,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TimelineEventKind {
    Message,
    Tool,
    Progress,
    Diagnostic,
    Lifecycle,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineEvent {
    pub id: TimelineEventId,
    pub conversation_id: ConversationId,
    pub run_id: RunId,
    pub agent_id: AgentId,
    pub sequence: u64,
    pub kind: TimelineEventKind,
    pub content: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Answered,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum ApprovalResolution {
    Approved,
    Denied,
    Answer(String),
    Answers(BTreeMap<String, Vec<String>>),
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputOption {
    pub label: String,
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    pub options: Option<Vec<UserInputOption>>,
    pub is_other: bool,
    pub is_secret: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputRequest {
    pub questions: Vec<UserInputQuestion>,
    pub auto_resolution_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ApprovalRequestDetails {
    CommandExecution {
        command: Option<String>,
        cwd: Option<String>,
    },
    FileChange {
        changes: Vec<FileChangeApprovalDetail>,
        grant_root: Option<String>,
        reason: Option<String>,
    },
    PermissionProfile {
        cwd: String,
        profile: RequestedPermissionProfile,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeApprovalDetail {
    pub path: String,
    pub change: FileChangeKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum FileChangeKind {
    Add,
    Delete,
    Update { move_path: Option<String> },
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestedPermissionProfile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_system: Option<RequestedFileSystemPermissions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<RequestedNetworkPermissions>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestedFileSystemPermissions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entries: Option<Vec<RequestedFileSystemEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glob_scan_max_depth: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestedFileSystemEntry {
    pub access: RequestedFileSystemAccess,
    pub path: RequestedFileSystemPath,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestedFileSystemAccess {
    Read,
    Write,
    Deny,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum RequestedFileSystemPath {
    Path { path: String },
    GlobPattern { pattern: String },
    Special { value: RequestedSpecialPath },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum RequestedSpecialPath {
    Root,
    Minimal,
    ProjectRoots {
        #[serde(skip_serializing_if = "Option::is_none")]
        subpath: Option<String>,
    },
    Tmpdir,
    SlashTmp,
    Unknown {
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        subpath: Option<String>,
    },
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestedNetworkPermissions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalResponseIntentStatus {
    Recorded,
    Acknowledged,
    Rejected,
    DispatchUnknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalResponseIntent {
    pub resolution: ApprovalResolution,
    pub status: ApprovalResponseIntentStatus,
}

impl ApprovalResolution {
    pub(crate) fn status(&self) -> ApprovalStatus {
        match self {
            Self::Approved => ApprovalStatus::Approved,
            Self::Denied => ApprovalStatus::Denied,
            Self::Answer(_) | Self::Answers(_) => ApprovalStatus::Answered,
            Self::Cancelled => ApprovalStatus::Cancelled,
            Self::Failed => ApprovalStatus::Failed,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Answer(_) | Self::Answers(_) => "answered",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub id: ApprovalId,
    pub run_id: RunId,
    pub agent_id: AgentId,
    pub provider: ProviderId,
    pub provider_request_id: Option<String>,
    pub operation: String,
    pub scope: String,
    pub input: Option<UserInputRequest>,
    pub details: Option<ApprovalRequestDetails>,
    pub status: ApprovalStatus,
    pub resolution: Option<ApprovalResolution>,
    pub response_intent: Option<ApprovalResponseIntent>,
}

impl Approval {
    pub fn new(
        run_id: RunId,
        agent_id: AgentId,
        provider: ProviderId,
        provider_request_id: impl Into<String>,
        operation: impl Into<String>,
        scope: impl Into<String>,
    ) -> Self {
        Self {
            id: ApprovalId::new(),
            run_id,
            agent_id,
            provider,
            provider_request_id: Some(provider_request_id.into()),
            operation: operation.into(),
            scope: scope.into(),
            input: None,
            details: None,
            status: ApprovalStatus::Pending,
            resolution: None,
            response_intent: None,
        }
    }

    pub fn resolve(&mut self, resolution: ApprovalResolution) -> Result<(), DomainError> {
        match self.status {
            ApprovalStatus::Pending => {
                self.status = resolution.status();
                self.resolution = Some(resolution);
                Ok(())
            }
            from => Err(DomainError::InvalidTransition {
                entity: "approval",
                from: approval_status_label(from),
                to: resolution.label(),
            }),
        }
    }
}

fn approval_status_label(status: ApprovalStatus) -> &'static str {
    match status {
        ApprovalStatus::Pending => "pending",
        ApprovalStatus::Approved => "approved",
        ApprovalStatus::Denied => "denied",
        ApprovalStatus::Answered => "answered",
        ApprovalStatus::Cancelled => "cancelled",
        ApprovalStatus::Failed => "failed",
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: WorkspaceId,
    pub conversation_id: ConversationId,
    pub project_root: Option<PathBuf>,
    pub execution_path: PathBuf,
    pub owned_worktree: bool,
    pub worktree_base_commit: Option<String>,
}

pub fn roll_up_status(root: AgentId, agents: &[AgentNode]) -> Result<RollupStatus, DomainError> {
    let mut nodes = HashMap::with_capacity(agents.len());
    for agent in agents {
        if nodes.insert(agent.id, agent).is_some() {
            return Err(DomainError::DuplicateAgent { agent: agent.id });
        }
    }

    if !nodes.contains_key(&root) {
        return Err(DomainError::RootNotFound { root });
    }

    let mut children: HashMap<AgentId, Vec<AgentId>> = HashMap::new();
    for agent in agents {
        if let Some(parent_id) = agent.parent_id {
            let parent = nodes.get(&parent_id).ok_or(DomainError::MissingParent {
                agent: agent.id,
                parent: parent_id,
            })?;
            if parent.run_id != agent.run_id {
                return Err(DomainError::ParentRunMismatch {
                    agent: agent.id,
                    parent: parent_id,
                    run: agent.run_id,
                    parent_run: parent.run_id,
                });
            }
            children.entry(parent_id).or_default().push(agent.id);
        }
    }

    for agent in agents {
        let mut visited = HashSet::new();
        let mut current = agent;
        while let Some(parent_id) = current.parent_id {
            if !visited.insert(current.id) {
                return Err(DomainError::AgentCycle { agent: current.id });
            }
            current = nodes
                .get(&parent_id)
                .expect("parents are validated before cycle detection");
        }
    }

    let mut result = RollupStatus::Completed;
    let mut pending = vec![root];
    while let Some(agent_id) = pending.pop() {
        let agent = nodes
            .get(&agent_id)
            .expect("root and children are validated before traversal");
        let status = match agent.status {
            AgentStatus::Waiting => RollupStatus::NeedsAttention,
            AgentStatus::Queued | AgentStatus::Running => RollupStatus::Active,
            AgentStatus::Failed => RollupStatus::Failed,
            AgentStatus::Interrupted => RollupStatus::Interrupted,
            AgentStatus::Completed => RollupStatus::Completed,
        };
        if status.precedence() > result.precedence() {
            result = status;
        }
        if let Some(descendants) = children.get(&agent_id) {
            pending.extend(descendants);
        }
    }

    Ok(result)
}
