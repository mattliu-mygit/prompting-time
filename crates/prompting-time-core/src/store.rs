use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{FromRow, QueryBuilder, Sqlite, SqlitePool, Transaction};
use time::OffsetDateTime;
use tokio::sync::broadcast;
use uuid::Uuid;

#[cfg(test)]
use std::sync::{Arc, Mutex, OnceLock};

use crate::domain::{
    AgentId, AgentNode, AgentStatus, Approval, ApprovalId, ApprovalRequestDetails,
    ApprovalResolution, ApprovalResponseIntent, ApprovalResponseIntentStatus, ApprovalStatus,
    Conversation, ConversationId, DomainError, Message, MessageId, MessageRole, MutationState,
    ProviderRun, RunId, RunStatus, TimelineEvent, TimelineEventId, TimelineEventKind, Workspace,
};
use crate::providers::{
    DispatchCertainty, NativeAgentStatus, NativeChildStatus, NativeSubAgentActivityKind,
    ProviderErrorCategory, ProviderId, ProviderSession, UserInputQuestion, UserInputRequest,
};
use crate::router::{RoutingDecision, RoutingProfile, RoutingReason, TaskKind};

const MAX_PAGE_SIZE: u32 = 200;
const MAX_POOL_CONNECTIONS: u32 = 8;
const STORE_CHANGE_CHANNEL_CAPACITY: usize = 256;
const RECOVERY_BATCH_SIZE: i64 = 200;
pub const MAX_TIMELINE_PREVIEW_BYTES: usize = 1_024;
pub const MAX_TIMELINE_PAGE_CONTENT_BYTES: usize =
    MAX_TIMELINE_PREVIEW_BYTES * MAX_PAGE_SIZE as usize;
pub const MAX_EVENT_DETAIL_BYTES: usize = 256 * 1_024;
pub const MAX_CONVERSATION_TITLE_BYTES: usize = 256;
const UNTITLED_CONVERSATION_TITLE: &str = "Untitled conversation";
const MAX_APPROVAL_DETAIL_SOURCE_BYTES: i64 = 256 * 1_024;
const MAX_APPROVAL_QUESTION_PAGE_SIZE: u32 = 50;
const MAX_APPROVAL_QUESTION_SOURCE_BYTES: i64 = 4 * 1_024;
const MAX_APPROVAL_QUESTION_HEADER_BYTES: usize = 256;
const MAX_APPROVAL_QUESTION_TEXT_BYTES: usize = 2 * 1_024;
const MAX_APPROVAL_AGENT_PATH_NODES: usize = 256;
const MAX_RUN_AUDIT_HANDOFF_BYTES: i64 = 256 * 1_024;
const MAX_RUN_AUDIT_ROUTING_BYTES: i64 = 64 * 1_024;
const MAX_AGENT_LABEL_PREVIEW_BYTES: usize = 256;
const MAX_AGENT_SUMMARY_PREVIEW_BYTES: usize = 2_048;
/// Physical queue bound: 256 complete provider events plus one reserved overflow marker.
pub const MAX_STAGED_EVENT_ROWS: usize = 257;
pub const MAX_STAGED_EVENT_BYTES: usize = 8 * 1024 * 1024;
const STAGED_OVERFLOW_CONTENT: &str = "Provider output omitted: staged queue limit exceeded";
const MAX_NATIVE_AGENT_ID_BYTES: usize = 256;
const MAX_NATIVE_AGENT_PATH_BYTES: usize = 1_024;
const MAX_CHILDREN_PER_EVENT: usize = 64;
const MAX_HANDOFF_DECISIONS: i64 = 32;
const MAX_HANDOFF_DECISION_REASON_BYTES: i64 = 64;
const MAX_HANDOFF_TASK_KIND_BYTES: i64 = 32;
const MAX_HANDOFF_CHILDREN: i64 = 32;
const MAX_HANDOFF_CHILD_SUMMARY_BYTES: i64 = 2_048;
const MAX_HANDOFF_MESSAGE_ROWS: i64 = 2_048;
pub const MAX_CANONICAL_MESSAGE_BYTES: usize = 64 * 1024;
pub(crate) const MAX_OBJECTIVE_BYTES: usize = 8 * 1024;
pub(crate) const MAX_CONSTRAINTS: usize = 32;
pub(crate) const MAX_CONSTRAINT_BYTES: usize = 2 * 1024;
pub(crate) const MAX_CONSTRAINT_BYTES_TOTAL: usize = 16 * 1024;
const MAX_CONSTRAINTS_JSON_BYTES: usize = 24 * 1024;
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[cfg(test)]
pub(crate) struct ConversationPersistenceBarrier {
    pub(crate) transaction_started: tokio::sync::Barrier,
    pub(crate) continue_persistence: tokio::sync::Barrier,
}

#[cfg(test)]
fn conversation_persistence_barrier() -> &'static Mutex<Option<Arc<ConversationPersistenceBarrier>>>
{
    static BARRIER: OnceLock<Mutex<Option<Arc<ConversationPersistenceBarrier>>>> = OnceLock::new();
    BARRIER.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
pub(crate) fn install_conversation_persistence_barrier() -> Arc<ConversationPersistenceBarrier> {
    let barrier = Arc::new(ConversationPersistenceBarrier {
        transaction_started: tokio::sync::Barrier::new(2),
        continue_persistence: tokio::sync::Barrier::new(2),
    });
    *conversation_persistence_barrier()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::clone(&barrier));
    barrier
}

#[cfg(test)]
async fn coordinate_conversation_persistence() {
    let barrier = conversation_persistence_barrier()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(barrier) = barrier {
        barrier.transaction_started.wait().await;
        barrier.continue_persistence.wait().await;
    }
}

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
    changes: broadcast::Sender<StoreChange>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoreChange {
    pub conversation_id: ConversationId,
    pub run_id: Option<RunId>,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("could not create SQLite parent directory {path}")]
    CreateParent {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("SQLite operation failed")]
    Database(#[from] sqlx::Error),
    #[error("SQLite migration failed")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error("page limit must be between 1 and {MAX_PAGE_SIZE}, got {0}")]
    InvalidPageLimit(u32),
    #[error("cursor is invalid")]
    InvalidCursor,
    #[error("cannot append {event} event while run is {status}")]
    InvalidEventState {
        event: &'static str,
        status: &'static str,
    },
    #[error("provider run cannot become terminal while a descendant is nonterminal")]
    ActiveDescendants,
    #[error("provider run cannot complete while an approval is pending")]
    PendingApproval,
    #[error("fallback provider must differ from the primary provider")]
    SameFallbackProvider,
    #[error("fallback requires a failed primary run with no observed mutation")]
    UnsafeFallbackState,
    #[error("primary run already has a fallback attempt")]
    FallbackAlreadyExists,
    #[error("existing fallback attempt belongs to different prepared intent")]
    FallbackIntentConflict,
    #[error("only approved, denied, or answer are valid provider approval responses")]
    InvalidApprovalResolution,
    #[error("approval already has an unresolved response intent")]
    ApprovalResponseIntentExists,
    #[error("approval does not have a recorded response intent in the required state")]
    InvalidApprovalResponseIntentState,
    #[error("only message, progress, or tool events can be staged while waiting")]
    InvalidStagedEvent,
    #[error("staged provider event queue has already overflowed")]
    StagedEventOverflowed,
    #[error("stored staged provider event queue exceeds its durable bounds")]
    CorruptStagedEventQueue,
    #[error("approval response intent was already acknowledged")]
    ApprovalResponseAlreadyAcknowledged,
    #[error("stored {entity} contains invalid data: {detail}")]
    InvalidData {
        entity: &'static str,
        detail: String,
    },
    #[error("{entity} {id} was not found")]
    NotFound { entity: &'static str, id: String },
    #[error("conversation {0} is archived")]
    ConversationArchived(ConversationId),
    #[error("conversation {0} already has an active turn")]
    ConversationBusy(ConversationId),
    #[error("command id {command_id} already belongs to a different request")]
    CommandConflict { command_id: String },
    #[error("provider-native agent identity conflicts with its recorded parent")]
    NativeAgentIdentityConflict,
    #[error("canonical message exceeds the {limit}-byte limit")]
    MessageTooLarge { limit: usize },
    #[error("provider run {0} is no longer owned by this supervisor")]
    DispatchOwnerMismatch(RunId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewConversation {
    title: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationSettings {
    pub objective: String,
    pub constraints: Vec<String>,
    pub routing_profile: RoutingProfile,
}

#[derive(Clone, Debug)]
pub(crate) struct NewSubmission {
    pub command_id: String,
    pub request_hash: String,
    pub conversation_id: ConversationId,
    pub provider: ProviderId,
    pub content: String,
    pub routing_decision: RoutingDecision,
    pub handoff_rendered: Option<String>,
    pub handoff_hash: Option<String>,
    pub turn_prompt: String,
}

#[derive(Clone, Debug)]
pub(crate) enum PreparedSubmission {
    Created { run: ProviderRun, root: AgentNode },
    Duplicate(ProviderRun),
}

#[derive(Clone, Debug)]
pub(crate) struct StoredSubmission {
    pub request_hash: String,
    pub run: ProviderRun,
    pub fallback_run: Option<ProviderRun>,
    pub routing_decision: RoutingDecision,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StoredChildAgentOutcome {
    pub provider: ProviderId,
    pub provider_native_id: String,
    pub summary: Option<String>,
    pub status: AgentStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StoredRoutingDecision {
    pub provider: ProviderId,
    pub reason: RoutingReason,
    pub task_kind: TaskKind,
}

#[derive(Clone, Debug)]
pub(crate) struct NewFallbackAttempt {
    pub provider: ProviderId,
    pub native_session_id: Option<String>,
    pub turn_prompt: String,
    pub handoff_rendered: Option<String>,
    pub handoff_hash: Option<String>,
    pub routing_decision: Option<Box<RoutingDecision>>,
}

impl NewConversation {
    pub fn projectless(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderEventRecord {
    Started {
        native_turn_id: Option<String>,
    },
    Message(String),
    NativeMessage {
        content: String,
        native_item_id: String,
    },
    Progress(String),
    Tool {
        content: String,
        mutation: MutationState,
    },
    NativeItem {
        native_item_id: String,
        content: String,
        mutation: MutationState,
    },
    ChildAgent {
        native_item_id: String,
        parent_native_thread_id: String,
        child_native_thread_ids: Vec<String>,
        child_statuses: Vec<NativeChildStatus>,
        operation: String,
        status: String,
    },
    SubAgent {
        native_item_id: String,
        agent_thread_id: String,
        agent_path: String,
        activity: NativeSubAgentActivityKind,
    },
    Unrecognized {
        method: String,
    },
    Diagnostic(String),
    ApprovalRequested {
        provider: ProviderId,
        request_id: String,
        operation: String,
        scope: String,
        details: Option<ApprovalRequestDetails>,
    },
    UserInputRequested {
        provider: ProviderId,
        request_id: String,
        questions: Vec<UserInputQuestion>,
        auto_resolution_ms: Option<u64>,
    },
    Waiting,
    Resumed,
    Completed,
    Interrupted,
    InterruptedWithMutation(MutationState),
    Failed(String),
    FailedWithMutation {
        diagnostic: String,
        mutation: MutationState,
    },
    ProviderFailed {
        category: ProviderErrorCategory,
        mutation: MutationState,
        dispatch_certainty: DispatchCertainty,
    },
}

impl ProviderEventRecord {
    pub fn started() -> Self {
        Self::Started {
            native_turn_id: None,
        }
    }

    pub fn started_with_native_id(native_turn_id: impl Into<String>) -> Self {
        Self::Started {
            native_turn_id: Some(native_turn_id.into()),
        }
    }

    pub fn progress(content: impl Into<String>) -> Self {
        Self::Progress(content.into())
    }

    pub fn message(content: impl Into<String>) -> Self {
        Self::Message(content.into())
    }

    pub fn native_message(content: impl Into<String>, native_item_id: impl Into<String>) -> Self {
        Self::NativeMessage {
            content: content.into(),
            native_item_id: native_item_id.into(),
        }
    }

    pub fn tool(content: impl Into<String>, mutation: MutationState) -> Self {
        Self::Tool {
            content: content.into(),
            mutation,
        }
    }

    pub fn native_item(
        native_item_id: impl Into<String>,
        content: impl Into<String>,
        mutation: MutationState,
    ) -> Self {
        Self::NativeItem {
            native_item_id: native_item_id.into(),
            content: content.into(),
            mutation,
        }
    }

    pub fn child_agent(
        native_item_id: impl Into<String>,
        parent_native_thread_id: impl Into<String>,
        child_native_thread_ids: Vec<String>,
        child_statuses: Vec<NativeChildStatus>,
        operation: impl Into<String>,
        status: impl Into<String>,
    ) -> Self {
        Self::ChildAgent {
            native_item_id: native_item_id.into(),
            parent_native_thread_id: parent_native_thread_id.into(),
            child_native_thread_ids,
            child_statuses,
            operation: operation.into(),
            status: status.into(),
        }
    }

    pub fn sub_agent(
        native_item_id: impl Into<String>,
        agent_thread_id: impl Into<String>,
        agent_path: impl Into<String>,
        activity: NativeSubAgentActivityKind,
    ) -> Self {
        Self::SubAgent {
            native_item_id: native_item_id.into(),
            agent_thread_id: agent_thread_id.into(),
            agent_path: agent_path.into(),
            activity,
        }
    }

    pub fn unrecognized(method: impl Into<String>) -> Self {
        Self::Unrecognized {
            method: method.into(),
        }
    }

    pub fn diagnostic(content: impl Into<String>) -> Self {
        Self::Diagnostic(content.into())
    }

    pub fn approval_requested(
        provider: ProviderId,
        request_id: impl Into<String>,
        operation: impl Into<String>,
        scope: impl Into<String>,
    ) -> Self {
        Self::ApprovalRequested {
            provider,
            request_id: request_id.into(),
            operation: operation.into(),
            scope: scope.into(),
            details: None,
        }
    }

    pub fn approval_requested_with_details(
        provider: ProviderId,
        request_id: impl Into<String>,
        operation: impl Into<String>,
        scope: impl Into<String>,
        details: Option<ApprovalRequestDetails>,
    ) -> Self {
        Self::ApprovalRequested {
            provider,
            request_id: request_id.into(),
            operation: operation.into(),
            scope: scope.into(),
            details,
        }
    }

    pub fn user_input_requested(
        provider: ProviderId,
        request_id: impl Into<String>,
        questions: Vec<UserInputQuestion>,
        auto_resolution_ms: Option<u64>,
    ) -> Self {
        Self::UserInputRequested {
            provider,
            request_id: request_id.into(),
            questions,
            auto_resolution_ms,
        }
    }

    pub fn waiting() -> Self {
        Self::Waiting
    }

    pub fn resumed() -> Self {
        Self::Resumed
    }

    pub fn completed() -> Self {
        Self::Completed
    }

    pub fn interrupted() -> Self {
        Self::Interrupted
    }

    pub fn interrupted_with_mutation(mutation: MutationState) -> Self {
        Self::InterruptedWithMutation(mutation)
    }

    pub fn failed(diagnostic: impl Into<String>) -> Self {
        Self::Failed(diagnostic.into())
    }

    pub fn failed_with_mutation(diagnostic: impl Into<String>, mutation: MutationState) -> Self {
        Self::FailedWithMutation {
            diagnostic: diagnostic.into(),
            mutation,
        }
    }

    pub fn provider_failed(
        category: ProviderErrorCategory,
        mutation: MutationState,
        dispatch_certainty: DispatchCertainty,
    ) -> Self {
        Self::ProviderFailed {
            category,
            mutation,
            dispatch_certainty,
        }
    }

    fn event_fields(&self, is_root: bool) -> (TimelineEventKind, &str) {
        match (self, is_root) {
            (Self::Started { .. }, true) => (TimelineEventKind::Lifecycle, "Provider run started"),
            (Self::Started { .. }, false) => (TimelineEventKind::Lifecycle, "Agent started"),
            (Self::Message(content) | Self::NativeMessage { content, .. }, _) => {
                (TimelineEventKind::Message, content)
            }
            (Self::Progress(content), _) => (TimelineEventKind::Progress, content),
            (Self::Tool { content, .. } | Self::NativeItem { content, .. }, _) => {
                (TimelineEventKind::Tool, content)
            }
            (Self::ChildAgent { operation, .. }, _) => (TimelineEventKind::Progress, operation),
            (Self::SubAgent { .. }, _) => (TimelineEventKind::Progress, "Agent activity"),
            (Self::Unrecognized { method }, _) => (TimelineEventKind::Diagnostic, method),
            (Self::Diagnostic(content), _) => (TimelineEventKind::Diagnostic, content),
            (Self::ApprovalRequested { operation, .. }, _) => {
                (TimelineEventKind::Lifecycle, operation)
            }
            (Self::UserInputRequested { .. }, _) => (TimelineEventKind::Lifecycle, "user input"),
            (Self::Waiting, true) => (TimelineEventKind::Lifecycle, "Provider run is waiting"),
            (Self::Waiting, false) => (TimelineEventKind::Lifecycle, "Agent is waiting"),
            (Self::Resumed, true) => (TimelineEventKind::Lifecycle, "Provider run resumed"),
            (Self::Resumed, false) => (TimelineEventKind::Lifecycle, "Agent resumed"),
            (Self::Completed, true) => (TimelineEventKind::Lifecycle, "Provider run completed"),
            (Self::Completed, false) => (TimelineEventKind::Lifecycle, "Agent completed"),
            (Self::Interrupted, true) => (TimelineEventKind::Lifecycle, "Provider run interrupted"),
            (Self::Interrupted, false) => (TimelineEventKind::Lifecycle, "Agent interrupted"),
            (Self::InterruptedWithMutation(_), true) => {
                (TimelineEventKind::Lifecycle, "Provider run interrupted")
            }
            (Self::InterruptedWithMutation(_), false) => {
                (TimelineEventKind::Lifecycle, "Agent interrupted")
            }
            (Self::Failed(diagnostic), _) => (TimelineEventKind::Diagnostic, diagnostic),
            (Self::FailedWithMutation { diagnostic, .. }, _) => {
                (TimelineEventKind::Diagnostic, diagnostic)
            }
            (Self::ProviderFailed { category, .. }, _) => (
                TimelineEventKind::Diagnostic,
                provider_error_content(*category),
            ),
        }
    }

    fn transition(&self) -> Option<(RunStatus, AgentStatus)> {
        match self {
            Self::Started { .. } | Self::Resumed => {
                Some((RunStatus::Running, AgentStatus::Running))
            }
            Self::Waiting | Self::ApprovalRequested { .. } | Self::UserInputRequested { .. } => {
                Some((RunStatus::Waiting, AgentStatus::Waiting))
            }
            Self::Completed => Some((RunStatus::Completed, AgentStatus::Completed)),
            Self::Interrupted | Self::InterruptedWithMutation(_) => {
                Some((RunStatus::Interrupted, AgentStatus::Interrupted))
            }
            Self::Failed(_) | Self::FailedWithMutation { .. } | Self::ProviderFailed { .. } => {
                Some((RunStatus::Failed, AgentStatus::Failed))
            }
            Self::Message(_)
            | Self::NativeMessage { .. }
            | Self::Progress(_)
            | Self::Tool { .. }
            | Self::NativeItem { .. }
            | Self::ChildAgent { .. }
            | Self::SubAgent { .. }
            | Self::Unrecognized { .. }
            | Self::Diagnostic(_) => None,
        }
    }

    fn payload_json(&self) -> Option<String> {
        match self {
            Self::Started {
                native_turn_id: Some(native_turn_id),
            } => Some(serde_json::json!({ "nativeTurnId": native_turn_id }).to_string()),
            Self::Tool { mutation, .. } => {
                Some(serde_json::json!({ "mutation": mutation }).to_string())
            }
            Self::NativeMessage { native_item_id, .. } => {
                Some(serde_json::json!({ "nativeItemId": native_item_id }).to_string())
            }
            Self::NativeItem {
                native_item_id,
                mutation,
                ..
            } => Some(
                serde_json::json!({
                    "nativeItemId": native_item_id,
                    "mutation": mutation,
                })
                .to_string(),
            ),
            Self::ChildAgent {
                native_item_id,
                parent_native_thread_id,
                child_native_thread_ids,
                child_statuses,
                operation,
                status,
            } => Some(
                serde_json::json!({
                    "recordType": "childAgent",
                    "nativeItemId": native_item_id,
                    "parentNativeThreadId": parent_native_thread_id,
                    "childNativeThreadIds": child_native_thread_ids,
                    "childStatuses": child_statuses,
                    "operation": operation,
                    "status": status,
                })
                .to_string(),
            ),
            Self::SubAgent {
                native_item_id,
                agent_thread_id,
                agent_path,
                activity,
            } => Some(
                serde_json::json!({
                    "nativeItemId": native_item_id,
                    "agentThreadId": agent_thread_id,
                    "agentPath": agent_path,
                    "activity": activity,
                })
                .to_string(),
            ),
            Self::Unrecognized { method } => {
                Some(serde_json::json!({ "method": method }).to_string())
            }
            Self::Diagnostic(_) => None,
            Self::ApprovalRequested { request_id, .. } => {
                Some(serde_json::json!({ "requestId": request_id }).to_string())
            }
            Self::UserInputRequested {
                request_id,
                questions,
                auto_resolution_ms,
                ..
            } => Some(
                serde_json::json!({
                    "requestId": request_id,
                    "questions": questions,
                    "autoResolutionMs": auto_resolution_ms,
                })
                .to_string(),
            ),
            Self::InterruptedWithMutation(mutation) => {
                Some(serde_json::json!({ "mutation": mutation }).to_string())
            }
            Self::ProviderFailed {
                category,
                mutation,
                dispatch_certainty,
            } => Some(
                serde_json::json!({
                    "errorCategory": category,
                    "mutation": mutation,
                    "dispatchCertainty": dispatch_certainty,
                })
                .to_string(),
            ),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SidebarDetails {
    pub conversation_id: ConversationId,
    pub routing_profile: RoutingProfile,
    pub project_root: Option<PathBuf>,
    pub run: Option<ProviderRun>,
    pub rollup_status: Option<crate::domain::RollupStatus>,
    pub active_descendant_count: usize,
    pub agents: Vec<AgentNode>,
    pub agents_truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimelineRecord {
    pub event: TimelineEvent,
    pub provider: ProviderId,
    pub content_bytes: usize,
    pub content_truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventDetail {
    pub id: TimelineEventId,
    pub content: String,
    pub content_bytes: usize,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentPageRecord {
    pub agent: AgentNode,
    pub depth: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentPage {
    pub run_id: Option<RunId>,
    pub items: Vec<AgentPageRecord>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecoveryAgent {
    pub run_id: RunId,
    pub agent_id: AgentId,
    pub mutation_state: MutationState,
    pub is_root: bool,
    pub depth: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalPage {
    pub items: Vec<ApprovalSummary>,
    pub truncated: bool,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalSummary {
    pub id: ApprovalId,
    pub run_id: RunId,
    pub agent_id: AgentId,
    pub provider: ProviderId,
    pub operation: String,
    pub scope: String,
    pub status: ApprovalStatus,
    pub response_pending: bool,
    pub agent_path: Vec<String>,
    pub agent_path_truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalDetailRecord {
    pub id: ApprovalId,
    pub status: ApprovalStatus,
    pub response_pending: bool,
    pub agent_path: Vec<String>,
    pub agent_path_truncated: bool,
    pub operation: String,
    pub scope: String,
    pub input: Option<UserInputRequest>,
    pub details: Option<ApprovalRequestDetails>,
    pub question_count: u32,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunAuditSummary {
    pub id: RunId,
    pub provider: ProviderId,
    pub status: RunStatus,
    pub reason: Option<RoutingReason>,
    pub routing_truncated: bool,
    pub has_handoff: bool,
    pub created_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunAuditPage {
    pub items: Vec<RunAuditSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunAuditDetailRecord {
    pub id: RunId,
    pub provider: ProviderId,
    pub status: RunStatus,
    pub routing: Option<RoutingDecision>,
    pub reason: Option<RoutingReason>,
    pub routing_truncated: bool,
    pub handoff: Option<String>,
    pub handoff_truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ApprovalQuestionPreview {
    pub id: String,
    pub header: String,
    pub question: String,
    pub options: Option<Vec<crate::domain::UserInputOption>>,
    pub is_other: bool,
    pub is_secret: bool,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalQuestionPage {
    pub items: Vec<ApprovalQuestionPreview>,
    pub total_count: u32,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryRun {
    pub run: ProviderRun,
    pub attempt_intent: Option<RecoveryAttemptIntent>,
    pub agents: Vec<AgentNode>,
    pub approvals: Vec<Approval>,
    pub staged_events: Vec<StagedProviderEvent>,
    pub staged_events_overflowed: bool,
    pub staged_events_truncated: bool,
    pub events: Vec<TimelineEvent>,
    pub events_truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryAttemptIntent {
    pub turn_prompt: String,
    pub handoff_rendered: Option<String>,
    pub handoff_hash: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QueuedRecovery {
    pub run: ProviderRun,
    pub attempt_intent: Option<RecoveryAttemptIntent>,
    pub roots: Vec<AgentNode>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedProviderEvent {
    pub id: TimelineEventId,
    pub conversation_id: ConversationId,
    pub run_id: RunId,
    pub agent_id: AgentId,
    pub sequence: u64,
    pub kind: TimelineEventKind,
    pub content: String,
    pub native_item_id: Option<String>,
    pub payload_json: Option<String>,
    pub mutation_state: Option<MutationState>,
    pub overflowed_kind: Option<TimelineEventKind>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StageWaitingEventOutcome {
    Staged(StagedProviderEvent),
    Overflowed(StagedProviderEvent),
}

impl Store {
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|source| StoreError::CreateParent {
                path: parent.to_owned(),
                source,
            })?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        Self::connect(options, MAX_POOL_CONNECTIONS).await
    }

    pub async fn open_in_memory() -> Result<Self, StoreError> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")?
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        Self::connect(options, 1).await
    }

    #[cfg(test)]
    pub(crate) async fn reject_dispatch_claims_for_test(&self) -> Result<(), StoreError> {
        sqlx::query(
            "CREATE TRIGGER reject_dispatch_claim_for_test \
             BEFORE UPDATE OF dispatch_certainty ON provider_runs \
             WHEN NEW.dispatch_certainty = 'may_have_dispatched' \
             BEGIN SELECT RAISE(ABORT, 'dispatch claim rejected by test'); END",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn allow_dispatch_claims_for_test(&self) -> Result<(), StoreError> {
        sqlx::query("DROP TRIGGER reject_dispatch_claim_for_test")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn reject_recovery_events_for_test(&self) -> Result<(), StoreError> {
        sqlx::query(
            "CREATE TRIGGER reject_recovery_event_for_test \
             BEFORE INSERT ON events \
             BEGIN SELECT RAISE(ABORT, 'recovery event rejected by test'); END",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn allow_recovery_events_for_test(&self) -> Result<(), StoreError> {
        sqlx::query("DROP TRIGGER reject_recovery_event_for_test")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn expire_dispatch_lease_for_test(
        &self,
        run_id: RunId,
    ) -> Result<(), StoreError> {
        sqlx::query("UPDATE provider_runs SET dispatch_lease_expires_at = 0 WHERE id = ?")
            .bind(run_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn delay_dispatch_lease_for_test(
        &self,
        run_id: RunId,
    ) -> Result<(), StoreError> {
        sqlx::query("UPDATE provider_runs SET dispatch_lease_expires_at = ? WHERE id = ?")
            .bind(now_millis().saturating_sub(10))
            .bind(run_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn replace_dispatch_owner_for_test(
        &self,
        run_id: RunId,
        owner_id: &str,
    ) -> Result<(), StoreError> {
        sqlx::query("UPDATE provider_runs SET dispatch_owner_id = ? WHERE id = ?")
            .bind(owner_id)
            .bind(run_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn connect(
        options: SqliteConnectOptions,
        max_connections: u32,
    ) -> Result<Self, StoreError> {
        let pool = SqlitePoolOptions::new()
            .max_connections(max_connections)
            .connect_with(options)
            .await?;
        MIGRATOR.run(&pool).await?;
        let (changes, _) = broadcast::channel(STORE_CHANGE_CHANNEL_CAPACITY);
        Ok(Self { pool, changes })
    }

    pub fn subscribe_changes(&self) -> broadcast::Receiver<StoreChange> {
        self.changes.subscribe()
    }

    fn notify_change(&self, conversation_id: ConversationId, run_id: RunId) {
        let _ = self.changes.send(StoreChange {
            conversation_id,
            run_id: Some(run_id),
        });
    }

    fn notify_conversation_change(&self, conversation_id: ConversationId) {
        let _ = self.changes.send(StoreChange {
            conversation_id,
            run_id: None,
        });
    }

    pub async fn close(self) {
        self.pool.close().await;
    }

    pub async fn create_conversation(
        &self,
        new_conversation: NewConversation,
    ) -> Result<Conversation, StoreError> {
        self.create_conversation_with_id(ConversationId::new(), new_conversation)
            .await
    }

    pub(crate) async fn create_conversation_with_id(
        &self,
        conversation_id: ConversationId,
        new_conversation: NewConversation,
    ) -> Result<Conversation, StoreError> {
        let title = normalize_conversation_title(new_conversation.title)?;
        let conversation = Conversation {
            id: conversation_id,
            title,
            workspace_id: None,
            archived: false,
        };
        let now = now_millis();

        sqlx::query(
            "INSERT INTO conversations (id, title, workspace_id, status, created_at, updated_at) \
             VALUES (?, ?, NULL, 'active', ?, ?)",
        )
        .bind(conversation.id.to_string())
        .bind(&conversation.title)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;

        self.notify_conversation_change(conversation.id);
        Ok(conversation)
    }

    pub(crate) async fn create_configured_conversation(
        &self,
        conversation_id: ConversationId,
        title: String,
        workspace: &Workspace,
        settings: &ConversationSettings,
    ) -> Result<Conversation, StoreError> {
        let title = normalize_conversation_title(title)?;
        validate_conversation_settings(settings)?;
        if workspace.conversation_id != conversation_id {
            return Err(StoreError::InvalidData {
                entity: "workspace",
                detail: "conversation ownership mismatch".to_owned(),
            });
        }
        let now = now_millis();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            "INSERT INTO conversations (id, title, workspace_id, status, created_at, updated_at) \
             VALUES (?, ?, NULL, 'active', ?, ?)",
        )
        .bind(conversation_id.to_string())
        .bind(&title)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        #[cfg(test)]
        coordinate_conversation_persistence().await;
        sqlx::query(
            "INSERT INTO workspaces \
             (id, conversation_id, project_root, execution_path, owned_worktree, \
              worktree_base_commit, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(workspace.id.to_string())
        .bind(conversation_id.to_string())
        .bind(
            workspace
                .project_root
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        )
        .bind(workspace.execution_path.to_string_lossy().into_owned())
        .bind(workspace.owned_worktree)
        .bind(&workspace.worktree_base_commit)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO conversation_settings \
             (conversation_id, objective, constraints_json, routing_profile) VALUES (?, ?, ?, ?)",
        )
        .bind(conversation_id.to_string())
        .bind(&settings.objective)
        .bind(serde_json::to_string(&settings.constraints).map_err(invalid_json("constraints"))?)
        .bind(routing_profile_label(settings.routing_profile))
        .execute(&mut *transaction)
        .await?;
        sqlx::query("UPDATE conversations SET workspace_id = ? WHERE id = ?")
            .bind(workspace.id.to_string())
            .bind(conversation_id.to_string())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        self.notify_conversation_change(conversation_id);
        Ok(Conversation {
            id: conversation_id,
            title,
            workspace_id: Some(workspace.id),
            archived: false,
        })
    }

    pub async fn load_conversation(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Conversation, StoreError> {
        sqlx::query_as::<_, ConversationRow>(
            "SELECT id, substr(title, 1, 256) AS title, workspace_id, status, updated_at \
             FROM conversations WHERE id = ?",
        )
        .bind(conversation_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            entity: "conversation",
            id: conversation_id.to_string(),
        })?
        .into_record()
        .map(|record| record.conversation)
    }

    pub async fn load_conversation_settings(
        &self,
        conversation_id: ConversationId,
    ) -> Result<ConversationSettings, StoreError> {
        let (objective, constraints_json, routing_profile): (Option<String>, Option<String>, String) =
            sqlx::query_as(
                "SELECT \
                    CASE WHEN length(CAST(objective AS BLOB)) <= ? THEN objective END, \
                    CASE WHEN length(CAST(constraints_json AS BLOB)) <= ? THEN constraints_json END, \
                    routing_profile \
                 FROM conversation_settings WHERE conversation_id = ?",
            )
            .bind(i64::try_from(MAX_OBJECTIVE_BYTES).unwrap())
            .bind(i64::try_from(MAX_CONSTRAINTS_JSON_BYTES).unwrap())
            .bind(conversation_id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound {
                entity: "conversation settings",
                id: conversation_id.to_string(),
            })?;
        let settings = ConversationSettings {
            objective: objective.ok_or_else(|| StoreError::InvalidData {
                entity: "conversation settings",
                detail: "objective exceeds the durable byte bound".to_owned(),
            })?,
            constraints: serde_json::from_str(&constraints_json.ok_or_else(|| {
                StoreError::InvalidData {
                    entity: "conversation settings",
                    detail: "constraints exceed the durable byte bound".to_owned(),
                }
            })?)
            .map_err(invalid_data("conversation constraints"))?,
            routing_profile: parse_routing_profile(&routing_profile)?,
        };
        validate_conversation_settings(&settings)?;
        Ok(settings)
    }

    pub async fn load_workspace(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Workspace, StoreError> {
        let row: (String, Option<String>, String, bool, Option<String>) = sqlx::query_as(
            "SELECT id, project_root, execution_path, owned_worktree, worktree_base_commit \
             FROM workspaces WHERE conversation_id = ?",
        )
        .bind(conversation_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            entity: "workspace",
            id: conversation_id.to_string(),
        })?;
        Ok(Workspace {
            id: parse_uuid("workspace", &row.0)?.into(),
            conversation_id,
            project_root: row.1.map(PathBuf::from),
            execution_path: PathBuf::from(row.2),
            owned_worktree: row.3,
            worktree_base_commit: row.4,
        })
    }

    #[cfg(test)]
    pub(crate) async fn prepare_submission(
        &self,
        submission: NewSubmission,
    ) -> Result<PreparedSubmission, StoreError> {
        self.prepare_submission_inner(submission, None).await
    }

    pub(crate) async fn prepare_claimed_submission(
        &self,
        submission: NewSubmission,
        owner_id: &str,
        lease_duration: Duration,
    ) -> Result<PreparedSubmission, StoreError> {
        self.prepare_submission_inner(submission, Some((owner_id, lease_duration)))
            .await
    }

    async fn prepare_submission_inner(
        &self,
        submission: NewSubmission,
        dispatch_claim: Option<(&str, Duration)>,
    ) -> Result<PreparedSubmission, StoreError> {
        validate_message_size(&submission.content)?;
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let duplicate = sqlx::query_as::<_, SubmittedRunRow>(
            "SELECT submitted_commands.request_hash, provider_runs.id, \
             provider_runs.conversation_id, provider_runs.provider, \
             provider_runs.fallback_from_run_id, provider_runs.native_session_id, \
             provider_runs.status, provider_runs.mutation_state, \
             provider_runs.dispatch_certainty, provider_runs.created_at \
             FROM submitted_commands JOIN provider_runs ON provider_runs.id = submitted_commands.run_id \
             WHERE submitted_commands.command_id = ?",
        )
        .bind(&submission.command_id)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(row) = duplicate {
            let (request_hash, run) = row.into_parts()?;
            if run.conversation_id != submission.conversation_id
                || request_hash != submission.request_hash
            {
                return Err(StoreError::CommandConflict {
                    command_id: submission.command_id,
                });
            }
            transaction.commit().await?;
            return Ok(PreparedSubmission::Duplicate(run));
        }

        let status: String = sqlx::query_scalar("SELECT status FROM conversations WHERE id = ?")
            .bind(submission.conversation_id.to_string())
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or_else(|| StoreError::NotFound {
                entity: "conversation",
                id: submission.conversation_id.to_string(),
            })?;
        if status == "archived" {
            return Err(StoreError::ConversationArchived(submission.conversation_id));
        }
        let active: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM provider_runs WHERE conversation_id = ? \
             AND status IN ('queued', 'running', 'waiting'))",
        )
        .bind(submission.conversation_id.to_string())
        .fetch_one(&mut *transaction)
        .await?;
        if active {
            return Err(StoreError::ConversationBusy(submission.conversation_id));
        }

        let run = ProviderRun::new(submission.conversation_id, submission.provider);
        let root = AgentNode::root(run.id, submission.provider, "orchestrator");
        let now = now_millis();
        sqlx::query(
            "INSERT INTO provider_runs \
             (id, conversation_id, provider, native_session_id, status, mutation_state, \
              handoff_rendered, handoff_hash, application_managed, turn_prompt, created_at, updated_at) \
             VALUES (?, ?, ?, NULL, ?, ?, ?, ?, 1, ?, ?, ?)",
        )
        .bind(run.id.to_string())
        .bind(run.conversation_id.to_string())
        .bind(provider_label(run.provider))
        .bind(run_status_label(run.status))
        .bind(mutation_state_label(run.mutation_state))
        .bind(&submission.handoff_rendered)
        .bind(&submission.handoff_hash)
        .bind(&submission.turn_prompt)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO agent_nodes \
             (id, run_id, parent_id, provider, label, status, created_at, updated_at) \
             VALUES (?, ?, NULL, ?, ?, ?, ?, ?)",
        )
        .bind(root.id.to_string())
        .bind(run.id.to_string())
        .bind(provider_label(root.provider))
        .bind(&root.label)
        .bind(agent_status_label(root.status))
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        let message_id = MessageId::new();
        let result = sqlx::query(
            "INSERT INTO messages (id, conversation_id, run_id, role, content, created_at) \
             VALUES (?, ?, ?, 'user', ?, ?)",
        )
        .bind(message_id.to_string())
        .bind(run.conversation_id.to_string())
        .bind(run.id.to_string())
        .bind(&submission.content)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        let event_result = sqlx::query(
            "INSERT INTO events \
             (id, conversation_id, run_id, agent_id, kind, role, content, created_at) \
             VALUES (?, ?, ?, ?, 'message', 'user', ?, ?)",
        )
        .bind(message_id.to_string())
        .bind(run.conversation_id.to_string())
        .bind(run.id.to_string())
        .bind(root.id.to_string())
        .bind(&submission.content)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        let _event_sequence = u64::try_from(event_result.last_insert_rowid()).map_err(|_| {
            StoreError::InvalidData {
                entity: "timeline event",
                detail: "negative sequence".to_owned(),
            }
        })?;
        let message_sequence =
            u64::try_from(result.last_insert_rowid()).map_err(|_| StoreError::InvalidData {
                entity: "message sequence",
                detail: "negative sequence".to_owned(),
            })?;
        sqlx::query("UPDATE provider_runs SET context_through_sequence = ? WHERE id = ?")
            .bind(
                i64::try_from(message_sequence).map_err(|_| StoreError::InvalidData {
                    entity: "message sequence",
                    detail: "sequence exceeds SQLite range".to_owned(),
                })?,
            )
            .bind(run.id.to_string())
            .execute(&mut *transaction)
            .await?;
        let decision = serde_json::to_string(&submission.routing_decision)
            .map_err(invalid_json("routing decision"))?;
        sqlx::query(
            "INSERT INTO routing_decisions \
             (id, run_id, chosen_provider, details_json, reason, task_kind, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::now_v7().to_string())
        .bind(run.id.to_string())
        .bind(provider_label(run.provider))
        .bind(decision)
        .bind(routing_reason_label(submission.routing_decision.reason))
        .bind(task_kind_label(submission.routing_decision.task_kind))
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO submitted_commands \
             (command_id, request_hash, conversation_id, run_id, created_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&submission.command_id)
        .bind(&submission.request_hash)
        .bind(run.conversation_id.to_string())
        .bind(run.id.to_string())
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("UPDATE conversations SET updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(run.conversation_id.to_string())
            .execute(&mut *transaction)
            .await?;
        if let Some((owner_id, lease_duration)) = dispatch_claim {
            sqlx::query(
                "UPDATE provider_runs \
                 SET dispatch_certainty = 'may_have_dispatched', dispatch_owner_id = ?, \
                     dispatch_lease_expires_at = ?, updated_at = ? WHERE id = ?",
            )
            .bind(owner_id)
            .bind(lease_expires_at(lease_duration))
            .bind(now_millis())
            .bind(run.id.to_string())
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        self.notify_change(run.conversation_id, run.id);
        Ok(PreparedSubmission::Created { run, root })
    }

    pub(crate) async fn load_submission(
        &self,
        command_id: &str,
    ) -> Result<Option<StoredSubmission>, StoreError> {
        let row = sqlx::query_as::<_, SubmittedRunRow>(
            "SELECT submitted_commands.request_hash, provider_runs.id, \
             provider_runs.conversation_id, provider_runs.provider, \
             provider_runs.fallback_from_run_id, provider_runs.native_session_id, \
             provider_runs.status, provider_runs.mutation_state, \
             provider_runs.dispatch_certainty, provider_runs.created_at \
             FROM submitted_commands JOIN provider_runs ON provider_runs.id = submitted_commands.run_id \
             WHERE submitted_commands.command_id = ?",
        )
        .bind(command_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let (request_hash, run) = row.into_parts()?;
        let routing_decision = self.load_routing_decision(run.id).await?;
        let fallback_run = sqlx::query_as::<_, ProviderRunRow>(
            "SELECT id, conversation_id, provider, fallback_from_run_id, native_session_id, \
             status, mutation_state, dispatch_certainty, created_at FROM provider_runs \
             WHERE fallback_from_run_id = ?",
        )
        .bind(run.id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .map(ProviderRunRow::into_domain)
        .transpose()?;
        Ok(Some(StoredSubmission {
            request_hash,
            run,
            fallback_run,
            routing_decision,
        }))
    }

    pub async fn create_run(
        &self,
        conversation_id: ConversationId,
        provider: ProviderId,
    ) -> Result<(ProviderRun, AgentNode), StoreError> {
        let run = ProviderRun::new(conversation_id, provider);
        let root = AgentNode::root(run.id, provider, "orchestrator");
        let now = now_millis();
        let mut transaction = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO provider_runs \
             (id, conversation_id, provider, native_session_id, status, mutation_state, created_at, updated_at) \
             VALUES (?, ?, ?, NULL, ?, ?, ?, ?)",
        )
        .bind(run.id.to_string())
        .bind(conversation_id.to_string())
        .bind(provider_label(run.provider))
        .bind(run_status_label(run.status))
        .bind(mutation_state_label(run.mutation_state))
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "INSERT INTO agent_nodes \
             (id, run_id, parent_id, provider, label, status, created_at, updated_at) \
             VALUES (?, ?, NULL, ?, ?, ?, ?, ?)",
        )
        .bind(root.id.to_string())
        .bind(run.id.to_string())
        .bind(provider_label(root.provider))
        .bind(&root.label)
        .bind(agent_status_label(root.status))
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;

        sqlx::query("UPDATE conversations SET updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(conversation_id.to_string())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        self.notify_change(conversation_id, run.id);
        Ok((run, root))
    }

    pub async fn create_fallback_run(
        &self,
        primary_run_id: RunId,
        provider: ProviderId,
    ) -> Result<(ProviderRun, AgentNode), StoreError> {
        self.create_fallback_run_with_handoff(primary_run_id, provider, None, None, None)
            .await
    }

    pub(crate) async fn create_fallback_run_with_handoff(
        &self,
        primary_run_id: RunId,
        provider: ProviderId,
        handoff_rendered: Option<&str>,
        handoff_hash: Option<&str>,
        routing_decision: Option<&RoutingDecision>,
    ) -> Result<(ProviderRun, AgentNode), StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let primary = sqlx::query_as::<_, ProviderRunRow>(
            "SELECT id, conversation_id, provider, fallback_from_run_id, native_session_id, status, mutation_state, dispatch_certainty, created_at \
             FROM provider_runs WHERE id = ?",
        )
        .bind(primary_run_id.to_string())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            entity: "provider run",
            id: primary_run_id.to_string(),
        })?
        .into_domain()?;
        if primary.provider == provider {
            return Err(StoreError::SameFallbackProvider);
        }
        if primary.fallback_from_run_id.is_some()
            || primary.status != RunStatus::Failed
            || primary.mutation_state != MutationState::NoneObserved
            || primary.dispatch_certainty != Some(DispatchCertainty::NotDispatched)
        {
            return Err(StoreError::UnsafeFallbackState);
        }
        let fallback_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM provider_runs WHERE fallback_from_run_id = ?)",
        )
        .bind(primary_run_id.to_string())
        .fetch_one(&mut *transaction)
        .await?;
        if fallback_exists {
            return Err(StoreError::FallbackAlreadyExists);
        }
        let (conversation_status, application_managed): (String, bool) = sqlx::query_as(
            "SELECT conversations.status, provider_runs.application_managed FROM provider_runs \
                 JOIN conversations ON conversations.id = provider_runs.conversation_id \
                 WHERE provider_runs.id = ?",
        )
        .bind(primary_run_id.to_string())
        .fetch_one(&mut *transaction)
        .await?;
        if conversation_status == "archived" {
            return Err(StoreError::ConversationArchived(primary.conversation_id));
        }
        if application_managed {
            return Err(StoreError::UnsafeFallbackState);
        }
        let mut run = ProviderRun::new(primary.conversation_id, provider);
        run.fallback_from_run_id = Some(primary_run_id);
        let root = AgentNode::root(run.id, provider, "orchestrator");
        let now = now_millis();
        sqlx::query(
            "INSERT INTO provider_runs \
             (id, conversation_id, provider, fallback_from_run_id, native_session_id, status, \
              mutation_state, handoff_rendered, handoff_hash, context_through_sequence, \
              application_managed, created_at, updated_at) \
             VALUES (?, ?, ?, ?, NULL, ?, ?, ?, ?, \
                     (SELECT context_through_sequence FROM provider_runs WHERE id = ?), \
                     (SELECT application_managed FROM provider_runs WHERE id = ?), ?, ?)",
        )
        .bind(run.id.to_string())
        .bind(run.conversation_id.to_string())
        .bind(provider_label(run.provider))
        .bind(primary_run_id.to_string())
        .bind(run_status_label(run.status))
        .bind(mutation_state_label(run.mutation_state))
        .bind(handoff_rendered)
        .bind(handoff_hash)
        .bind(primary_run_id.to_string())
        .bind(primary_run_id.to_string())
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        if let Some(decision) = routing_decision {
            sqlx::query(
                "INSERT INTO routing_decisions \
                 (id, run_id, chosen_provider, details_json, reason, task_kind, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(Uuid::now_v7().to_string())
            .bind(run.id.to_string())
            .bind(provider_label(provider))
            .bind(serde_json::to_string(decision).map_err(invalid_json("routing decision"))?)
            .bind(routing_reason_label(decision.reason))
            .bind(task_kind_label(decision.task_kind))
            .bind(now)
            .execute(&mut *transaction)
            .await?;
        }
        sqlx::query(
            "INSERT INTO agent_nodes \
             (id, run_id, parent_id, provider, label, status, created_at, updated_at) \
             VALUES (?, ?, NULL, ?, ?, ?, ?, ?)",
        )
        .bind(root.id.to_string())
        .bind(run.id.to_string())
        .bind(provider_label(root.provider))
        .bind(&root.label)
        .bind(agent_status_label(root.status))
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("UPDATE conversations SET updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(run.conversation_id.to_string())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        self.notify_change(run.conversation_id, run.id);
        Ok((run, root))
    }

    pub(crate) async fn fail_and_create_owned_fallback(
        &self,
        primary_run_id: RunId,
        primary_root_id: AgentId,
        expected_owner_id: &str,
        lease_duration: Duration,
        category: ProviderErrorCategory,
        fallback: NewFallbackAttempt,
    ) -> Result<(ProviderRun, AgentNode), StoreError> {
        let (_, created) = self
            .append_run_event_inner(
                primary_run_id,
                primary_root_id,
                ProviderEventRecord::provider_failed(
                    category,
                    MutationState::NoneObserved,
                    DispatchCertainty::NotDispatched,
                ),
                Some(expected_owner_id),
                Some((fallback, Some((expected_owner_id, lease_duration)))),
            )
            .await?;
        created.ok_or(StoreError::UnsafeFallbackState)
    }

    #[cfg(test)]
    pub(crate) async fn fail_and_create_fallback(
        &self,
        primary_run_id: RunId,
        primary_root_id: AgentId,
        category: ProviderErrorCategory,
        fallback: NewFallbackAttempt,
    ) -> Result<(ProviderRun, AgentNode), StoreError> {
        let (_, created) = self
            .append_run_event_inner(
                primary_run_id,
                primary_root_id,
                ProviderEventRecord::provider_failed(
                    category,
                    MutationState::NoneObserved,
                    DispatchCertainty::NotDispatched,
                ),
                None,
                Some((fallback, None)),
            )
            .await?;
        created.ok_or(StoreError::UnsafeFallbackState)
    }

    pub async fn load_run(&self, run_id: RunId) -> Result<ProviderRun, StoreError> {
        sqlx::query_as::<_, ProviderRunRow>(
            "SELECT id, conversation_id, provider, fallback_from_run_id, native_session_id, status, mutation_state, dispatch_certainty, created_at \
             FROM provider_runs WHERE id = ?",
        )
        .bind(run_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            entity: "provider run",
            id: run_id.to_string(),
        })?
        .into_domain()
    }

    pub(crate) async fn is_owned_fallback_transition(
        &self,
        run_id: RunId,
        owner_id: &str,
    ) -> Result<bool, StoreError> {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM provider_runs AS primary_run \
             WHERE primary_run.id = ? AND primary_run.dispatch_owner_id = ? \
             AND primary_run.status IN ('completed', 'failed', 'interrupted') \
             AND EXISTS(SELECT 1 FROM provider_runs AS fallback_run \
                 WHERE fallback_run.fallback_from_run_id = primary_run.id \
                 AND fallback_run.dispatch_owner_id = primary_run.dispatch_owner_id \
                 AND fallback_run.status IN ('queued', 'running', 'waiting')))",
        )
        .bind(run_id.to_string())
        .bind(owner_id)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn bind_native_session(
        &self,
        run_id: RunId,
        native_session_id: &str,
    ) -> Result<(), StoreError> {
        self.bind_native_session_with_group(run_id, native_session_id, None)
            .await
    }

    pub async fn bind_native_session_with_group(
        &self,
        run_id: RunId,
        native_session_id: &str,
        native_group_id: Option<&str>,
    ) -> Result<(), StoreError> {
        self.bind_native_session_with_group_inner(run_id, native_session_id, native_group_id, None)
            .await
    }

    pub(crate) async fn bind_owned_native_session_with_group(
        &self,
        run_id: RunId,
        native_session_id: &str,
        native_group_id: Option<&str>,
        expected_owner_id: &str,
    ) -> Result<(), StoreError> {
        self.bind_native_session_with_group_inner(
            run_id,
            native_session_id,
            native_group_id,
            Some(expected_owner_id),
        )
        .await
    }

    async fn bind_native_session_with_group_inner(
        &self,
        run_id: RunId,
        native_session_id: &str,
        native_group_id: Option<&str>,
        expected_owner_id: Option<&str>,
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(expected_owner_id) = expected_owner_id {
            require_dispatch_owner(&mut transaction, run_id, expected_owner_id).await?;
        }
        let run = sqlx::query_as::<_, ProviderRunRow>(
            "SELECT id, conversation_id, provider, fallback_from_run_id, native_session_id, status, mutation_state, dispatch_certainty, created_at \
             FROM provider_runs WHERE id = ?",
        )
        .bind(run_id.to_string())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            entity: "provider run",
            id: run_id.to_string(),
        })?
        .into_domain()?;
        if run.status != RunStatus::Queued {
            return Err(StoreError::InvalidEventState {
                event: "bind native session",
                status: run_status_label(run.status),
            });
        }
        let now = now_millis();
        sqlx::query(
            "INSERT INTO provider_sessions \
             (id, conversation_id, provider, native_session_id, native_group_id, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(conversation_id, provider) DO UPDATE SET \
             native_session_id = excluded.native_session_id, \
             native_group_id = excluded.native_group_id, updated_at = excluded.updated_at",
        )
        .bind(Uuid::now_v7().to_string())
        .bind(run.conversation_id.to_string())
        .bind(provider_label(run.provider))
        .bind(native_session_id)
        .bind(native_group_id)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        let provider_session_id: String = sqlx::query_scalar(
            "SELECT id FROM provider_sessions WHERE conversation_id = ? AND provider = ?",
        )
        .bind(run.conversation_id.to_string())
        .bind(provider_label(run.provider))
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE provider_runs SET provider_session_id = ?, native_session_id = ?, updated_at = ? \
             WHERE id = ?",
        )
        .bind(provider_session_id)
        .bind(native_session_id)
        .bind(now)
        .bind(run_id.to_string())
        .execute(&mut *transaction)
        .await?;
        let root_update = sqlx::query(
            "UPDATE agent_nodes SET provider_native_id = ?, updated_at = ? \
             WHERE run_id = ? AND parent_id IS NULL \
               AND (provider_native_id IS NULL OR provider_native_id = ?)",
        )
        .bind(native_session_id)
        .bind(now)
        .bind(run_id.to_string())
        .bind(native_session_id)
        .execute(&mut *transaction)
        .await?;
        if root_update.rows_affected() != 1 {
            return Err(StoreError::NativeAgentIdentityConflict);
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn load_provider_session(
        &self,
        conversation_id: ConversationId,
        provider: ProviderId,
    ) -> Result<Option<ProviderSession>, StoreError> {
        let row = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT native_session_id, native_group_id FROM provider_sessions \
             WHERE conversation_id = ? AND provider = ? AND native_session_id IS NOT NULL",
        )
        .bind(conversation_id.to_string())
        .bind(provider_label(provider))
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(native_id, native_group_id)| ProviderSession {
            provider,
            native_id,
            native_group_id,
        }))
    }

    pub async fn provider_context_boundary(
        &self,
        conversation_id: ConversationId,
        provider: ProviderId,
    ) -> Result<u64, StoreError> {
        let boundary: Option<i64> = sqlx::query_scalar(
            "SELECT context_through_sequence FROM provider_sessions \
             WHERE conversation_id = ? AND provider = ?",
        )
        .bind(conversation_id.to_string())
        .bind(provider_label(provider))
        .fetch_optional(&self.pool)
        .await?;
        boundary
            .map(|value| {
                u64::try_from(value).map_err(|_| StoreError::InvalidData {
                    entity: "provider context boundary",
                    detail: "negative sequence".to_owned(),
                })
            })
            .transpose()
            .map(|value| value.unwrap_or(0))
    }

    pub(crate) async fn load_messages_after(
        &self,
        conversation_id: ConversationId,
        after_sequence: u64,
        context_budget_chars: usize,
    ) -> Result<Vec<Message>, StoreError> {
        let after_sequence =
            i64::try_from(after_sequence).map_err(|_| StoreError::InvalidData {
                entity: "message sequence",
                detail: "sequence exceeds SQLite range".to_owned(),
            })?;
        let context_budget_chars =
            i64::try_from(context_budget_chars).map_err(|_| StoreError::InvalidData {
                entity: "message context budget",
                detail: "budget exceeds SQLite range".to_owned(),
            })?;
        let rows: Vec<(i64, String, Option<String>, String, String)> = sqlx::query_as(
            "WITH candidates AS ( \
                 SELECT sequence, id, run_id, role, \
                        length(content) AS content_chars, \
                        length(CAST(content AS BLOB)) AS content_bytes \
                 FROM messages \
                 WHERE conversation_id = ? AND sequence > ? \
                 ORDER BY sequence DESC LIMIT ? \
             ), newest AS ( \
                 SELECT sequence, id, run_id, role, content_bytes, \
                        SUM(CASE WHEN content_bytes > ? THEN ? + 1 \
                                 ELSE content_chars + 13 END) \
                            OVER (ORDER BY sequence DESC) AS used_chars \
                 FROM candidates \
             ) \
             SELECT newest.sequence, newest.id, newest.run_id, newest.role, messages.content \
             FROM newest JOIN messages ON messages.sequence = newest.sequence \
             WHERE newest.used_chars <= ? AND newest.content_bytes <= ? \
             ORDER BY newest.sequence",
        )
        .bind(conversation_id.to_string())
        .bind(after_sequence)
        .bind(MAX_HANDOFF_MESSAGE_ROWS)
        .bind(i64::try_from(MAX_CANONICAL_MESSAGE_BYTES).unwrap())
        .bind(context_budget_chars)
        .bind(context_budget_chars)
        .bind(i64::try_from(MAX_CANONICAL_MESSAGE_BYTES).unwrap())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(sequence, id, run_id, role, content)| {
                Ok(Message {
                    id: parse_uuid("message", &id)?.into(),
                    conversation_id,
                    run_id: run_id
                        .map(|id| parse_uuid("provider run", &id).map(Into::into))
                        .transpose()?,
                    sequence: u64::try_from(sequence).map_err(|_| StoreError::InvalidData {
                        entity: "message sequence",
                        detail: "negative sequence".to_owned(),
                    })?,
                    role: match role.as_str() {
                        "user" => MessageRole::User,
                        "assistant" => MessageRole::Assistant,
                        value => {
                            return Err(StoreError::InvalidData {
                                entity: "message role",
                                detail: value.to_owned(),
                            });
                        }
                    },
                    content,
                })
            })
            .collect()
    }

    pub async fn latest_provider(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Option<ProviderId>, StoreError> {
        let provider: Option<String> = sqlx::query_scalar(
            "SELECT provider FROM provider_runs WHERE conversation_id = ? \
             ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(conversation_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        provider.map(|value| parse_provider(&value)).transpose()
    }

    pub async fn provider_usage(&self) -> Result<Vec<(ProviderId, u64)>, StoreError> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT provider, COUNT(*) FROM provider_runs GROUP BY provider ORDER BY provider",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(provider, count)| {
                Ok((
                    parse_provider(&provider)?,
                    u64::try_from(count).map_err(|_| StoreError::InvalidData {
                        entity: "provider usage",
                        detail: "negative count".to_owned(),
                    })?,
                ))
            })
            .collect()
    }

    pub async fn advance_provider_context(&self, run_id: RunId) -> Result<(), StoreError> {
        self.advance_provider_context_inner(run_id, None).await
    }

    pub(crate) async fn advance_owned_provider_context(
        &self,
        run_id: RunId,
        expected_owner_id: &str,
    ) -> Result<(), StoreError> {
        self.advance_provider_context_inner(run_id, Some(expected_owner_id))
            .await
    }

    async fn advance_provider_context_inner(
        &self,
        run_id: RunId,
        expected_owner_id: Option<&str>,
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(expected_owner_id) = expected_owner_id {
            require_dispatch_owner(&mut transaction, run_id, expected_owner_id).await?;
        }
        let changed = sqlx::query(
            "UPDATE provider_sessions SET context_through_sequence = \
             max(context_through_sequence, coalesce((SELECT context_through_sequence \
                 FROM provider_runs WHERE id = ?), 0)), updated_at = ? \
             WHERE conversation_id = (SELECT conversation_id FROM provider_runs WHERE id = ?) \
             AND provider = (SELECT provider FROM provider_runs WHERE id = ?)",
        )
        .bind(run_id.to_string())
        .bind(now_millis())
        .bind(run_id.to_string())
        .bind(run_id.to_string())
        .execute(&mut *transaction)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(StoreError::NotFound {
                entity: "provider session",
                id: run_id.to_string(),
            });
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn load_handoff(
        &self,
        run_id: RunId,
    ) -> Result<Option<(String, String)>, StoreError> {
        let row: Option<(Option<String>, Option<String>)> =
            sqlx::query_as("SELECT handoff_rendered, handoff_hash FROM provider_runs WHERE id = ?")
                .bind(run_id.to_string())
                .fetch_optional(&self.pool)
                .await?;
        match row {
            Some((Some(rendered), Some(hash))) => Ok(Some((rendered, hash))),
            Some((None, None)) => Ok(None),
            Some(_) => Err(StoreError::InvalidData {
                entity: "provider run handoff",
                detail: "rendered capsule and hash must both be present".to_owned(),
            }),
            None => Err(StoreError::NotFound {
                entity: "provider run",
                id: run_id.to_string(),
            }),
        }
    }

    pub async fn load_routing_decision(
        &self,
        run_id: RunId,
    ) -> Result<RoutingDecision, StoreError> {
        let details: String =
            sqlx::query_scalar("SELECT details_json FROM routing_decisions WHERE run_id = ?")
                .bind(run_id.to_string())
                .fetch_optional(&self.pool)
                .await?
                .ok_or_else(|| StoreError::NotFound {
                    entity: "routing decision",
                    id: run_id.to_string(),
                })?;
        serde_json::from_str(&details).map_err(invalid_data("routing decision"))
    }

    pub(crate) async fn load_routing_decisions(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<StoredRoutingDecision>, StoreError> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT chosen_provider, reason, task_kind FROM ( \
                 SELECT routing_decisions.chosen_provider, routing_decisions.reason, \
                        routing_decisions.task_kind, routing_decisions.created_at, \
                        routing_decisions.id \
                 FROM routing_decisions \
                 JOIN provider_runs ON provider_runs.id = routing_decisions.run_id \
                 WHERE provider_runs.conversation_id = ? \
                 AND routing_decisions.reason IS NOT NULL \
                 AND routing_decisions.task_kind IS NOT NULL \
                 AND length(routing_decisions.reason) <= ? \
                 AND length(routing_decisions.task_kind) <= ? \
                 ORDER BY routing_decisions.created_at DESC, routing_decisions.id DESC LIMIT ? \
             ) ORDER BY created_at, id",
        )
        .bind(conversation_id.to_string())
        .bind(MAX_HANDOFF_DECISION_REASON_BYTES)
        .bind(MAX_HANDOFF_TASK_KIND_BYTES)
        .bind(MAX_HANDOFF_DECISIONS)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(provider, reason, task_kind)| {
                Ok(StoredRoutingDecision {
                    provider: parse_provider(&provider)?,
                    reason: parse_routing_reason(&reason)?,
                    task_kind: parse_task_kind(&task_kind)?,
                })
            })
            .collect()
    }

    pub(crate) async fn load_child_agent_outcomes(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<StoredChildAgentOutcome>, StoreError> {
        let rows: Vec<(String, String, Option<String>, String)> = sqlx::query_as(
            "SELECT provider, provider_native_id, summary, status FROM ( \
                 SELECT agent_nodes.provider, agent_nodes.provider_native_id, \
                        agent_nodes.summary, agent_nodes.status, agent_nodes.created_at, \
                        agent_nodes.id \
                 FROM agent_nodes \
                 JOIN provider_runs ON provider_runs.id = agent_nodes.run_id \
                 WHERE provider_runs.conversation_id = ? \
                 AND agent_nodes.parent_id IS NOT NULL \
                 AND agent_nodes.provider_native_id IS NOT NULL \
                 AND length(agent_nodes.provider_native_id) <= ? \
                 AND length(coalesce(agent_nodes.summary, '')) <= ? \
                 ORDER BY agent_nodes.created_at DESC, agent_nodes.id DESC LIMIT ? \
             ) ORDER BY created_at, id",
        )
        .bind(conversation_id.to_string())
        .bind(i64::try_from(MAX_NATIVE_AGENT_ID_BYTES).unwrap())
        .bind(MAX_HANDOFF_CHILD_SUMMARY_BYTES)
        .bind(MAX_HANDOFF_CHILDREN)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(provider, provider_native_id, summary, status)| {
                Ok(StoredChildAgentOutcome {
                    provider: parse_provider(&provider)?,
                    provider_native_id,
                    summary,
                    status: parse_agent_status(&status)?,
                })
            })
            .collect()
    }

    pub async fn archive_conversation(
        &self,
        conversation_id: ConversationId,
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let active: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM provider_runs WHERE conversation_id = ? \
             AND status IN ('queued', 'running', 'waiting'))",
        )
        .bind(conversation_id.to_string())
        .fetch_one(&mut *transaction)
        .await?;
        if active {
            return Err(StoreError::ConversationBusy(conversation_id));
        }
        let changed = sqlx::query(
            "UPDATE conversations SET status = 'archived', updated_at = ? WHERE id = ?",
        )
        .bind(now_millis())
        .bind(conversation_id.to_string())
        .execute(&mut *transaction)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(StoreError::NotFound {
                entity: "conversation",
                id: conversation_id.to_string(),
            });
        }
        transaction.commit().await?;
        self.notify_conversation_change(conversation_id);
        Ok(())
    }

    pub async fn append_run_event(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        record: ProviderEventRecord,
    ) -> Result<TimelineEvent, StoreError> {
        self.append_run_event_inner(run_id, agent_id, record, None, None)
            .await
            .map(|(event, _)| event)
    }

    pub(crate) async fn append_owned_run_event(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        expected_owner_id: &str,
        record: ProviderEventRecord,
    ) -> Result<TimelineEvent, StoreError> {
        self.append_run_event_inner(run_id, agent_id, record, Some(expected_owner_id), None)
            .await
            .map(|(event, _)| event)
    }

    async fn append_run_event_inner(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        record: ProviderEventRecord,
        expected_owner_id: Option<&str>,
        fallback: Option<(NewFallbackAttempt, Option<(&str, Duration)>)>,
    ) -> Result<(TimelineEvent, Option<(ProviderRun, AgentNode)>), StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(expected_owner_id) = expected_owner_id {
            require_dispatch_owner(&mut transaction, run_id, expected_owner_id).await?;
        }
        let run_row = sqlx::query_as::<_, ProviderRunRow>(
            "SELECT id, conversation_id, provider, fallback_from_run_id, native_session_id, status, mutation_state, dispatch_certainty, created_at \
             FROM provider_runs WHERE id = ?",
        )
        .bind(run_id.to_string())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            entity: "provider run",
            id: run_id.to_string(),
        })?;
        let mut run = run_row.into_domain()?;
        let agent_row = sqlx::query_as::<_, AgentNodeRow>(
            "SELECT id, run_id, parent_id, provider, provider_native_id, provider_native_path, label, summary, status, created_at \
             FROM agent_nodes WHERE id = ? AND run_id = ?",
        )
        .bind(agent_id.to_string())
        .bind(run_id.to_string())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            entity: "agent node",
            id: agent_id.to_string(),
        })?;
        let agent = agent_row.into_domain()?;
        let is_root = agent.parent_id.is_none();
        if run.status == RunStatus::Failed
            && let Some((fallback, _)) = fallback.as_ref()
            && let Some(existing) =
                load_existing_fallback(&mut transaction, &run, fallback, expected_owner_id).await?
        {
            let event = sqlx::query_as::<_, TimelineEventRow>(
                "SELECT id, conversation_id, run_id, agent_id, sequence, kind, role, content \
                 FROM events WHERE run_id = ? AND agent_id = ? \
                 ORDER BY sequence DESC LIMIT 1",
            )
            .bind(run_id.to_string())
            .bind(agent_id.to_string())
            .fetch_one(&mut *transaction)
            .await?
            .into_domain()?;
            transaction.commit().await?;
            return Ok((event, Some(existing)));
        }
        validate_event_state(&record, &run, &agent)?;
        if fallback.as_ref().is_some_and(|(fallback, _)| {
            !is_root
                || fallback.provider == run.provider
                || !matches!(
                    &record,
                    ProviderEventRecord::ProviderFailed {
                        mutation: MutationState::NoneObserved,
                        dispatch_certainty: DispatchCertainty::NotDispatched,
                        ..
                    }
                )
        }) {
            return Err(StoreError::UnsafeFallbackState);
        }
        let event_agent_id = match &record {
            ProviderEventRecord::ChildAgent {
                parent_native_thread_id,
                child_native_thread_ids,
                child_statuses,
                ..
            } => {
                materialize_child_agents(
                    &mut transaction,
                    &run,
                    &agent,
                    parent_native_thread_id,
                    child_native_thread_ids,
                    child_statuses,
                    now_millis(),
                )
                .await?
            }
            ProviderEventRecord::SubAgent {
                agent_thread_id,
                agent_path,
                activity,
                ..
            } => {
                update_sub_agent(
                    &mut transaction,
                    &run,
                    &agent,
                    agent_thread_id,
                    agent_path,
                    *activity,
                    now_millis(),
                )
                .await?
            }
            _ => agent_id,
        };
        if is_root
            && record
                .transition()
                .is_some_and(|(status, _)| is_terminal_run_status(status))
        {
            let active_descendants: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM agent_nodes \
                 WHERE run_id = ? AND parent_id IS NOT NULL \
                 AND status IN ('queued', 'running', 'waiting'))",
            )
            .bind(run_id.to_string())
            .fetch_one(&mut *transaction)
            .await?;
            if active_descendants {
                return Err(StoreError::ActiveDescendants);
            }
        }
        let terminal_approval_resolution = match &record {
            ProviderEventRecord::Completed => {
                let has_pending: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM approvals \
                     WHERE run_id = ? AND agent_id = ? AND status = 'pending')",
                )
                .bind(run_id.to_string())
                .bind(agent_id.to_string())
                .fetch_one(&mut *transaction)
                .await?;
                if has_pending {
                    return Err(StoreError::PendingApproval);
                }
                None
            }
            ProviderEventRecord::Interrupted | ProviderEventRecord::InterruptedWithMutation(_) => {
                Some(ApprovalResolution::Cancelled)
            }
            ProviderEventRecord::Failed(_)
            | ProviderEventRecord::FailedWithMutation { .. }
            | ProviderEventRecord::ProviderFailed { .. } => Some(ApprovalResolution::Failed),
            _ => None,
        };
        if record
            .transition()
            .is_some_and(|(status, _)| is_terminal_run_status(status))
        {
            drain_staged_events_in_transaction(
                &mut transaction,
                run_id,
                (!is_root).then_some(agent_id),
            )
            .await?;
        }
        let (kind, content) = record.event_fields(is_root);
        let payload_json = record.payload_json();
        let native_item_id = match &record {
            ProviderEventRecord::NativeMessage { native_item_id, .. } => {
                Some(native_item_id.as_str())
            }
            _ => None,
        };
        let event_id = TimelineEventId::new();
        let now = now_millis();

        if let Some(native_item_id) = native_item_id
            && let Some((existing_id, sequence, existing_content)) =
                sqlx::query_as::<_, (String, i64, String)>(
                    "SELECT id, sequence, content FROM events \
                     WHERE run_id = ? AND agent_id = ? AND kind = 'message' \
                     AND native_item_id = ?",
                )
                .bind(run_id.to_string())
                .bind(agent_id.to_string())
                .bind(native_item_id)
                .fetch_optional(&mut *transaction)
                .await?
        {
            sqlx::query("UPDATE events SET content = content || ? WHERE id = ?")
                .bind(content)
                .bind(&existing_id)
                .execute(&mut *transaction)
                .await?;
            if is_root {
                persist_assistant_message_in_transaction(
                    &mut transaction,
                    run.id,
                    run.conversation_id,
                    run.provider,
                    Some(native_item_id),
                    content,
                    now,
                )
                .await?;
            }
            sqlx::query("UPDATE conversations SET updated_at = ? WHERE id = ?")
                .bind(now)
                .bind(run.conversation_id.to_string())
                .execute(&mut *transaction)
                .await?;
            transaction.commit().await?;
            self.notify_change(run.conversation_id, run_id);
            return Ok((
                TimelineEvent {
                    id: parse_uuid("timeline event", &existing_id)?.into(),
                    conversation_id: run.conversation_id,
                    run_id,
                    agent_id,
                    sequence: u64::try_from(sequence).map_err(|_| StoreError::InvalidData {
                        entity: "timeline event",
                        detail: "negative sequence".to_owned(),
                    })?,
                    kind,
                    role: (kind == TimelineEventKind::Message).then_some(MessageRole::Assistant),
                    content: existing_content + content,
                },
                None,
            ));
        }

        let approval_fields = match &record {
            ProviderEventRecord::ApprovalRequested {
                provider,
                request_id,
                operation,
                scope,
                details,
            } => Some((
                *provider,
                request_id,
                operation.as_str(),
                scope.as_str(),
                None,
                details
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|error| StoreError::InvalidData {
                        entity: "approval request details",
                        detail: error.to_string(),
                    })?,
            )),
            ProviderEventRecord::UserInputRequested {
                provider,
                request_id,
                questions,
                auto_resolution_ms,
                ..
            } => Some((
                *provider,
                request_id,
                "user input",
                "structured questions",
                Some(
                    serde_json::to_string(&UserInputRequest {
                        questions: questions.clone(),
                        auto_resolution_ms: *auto_resolution_ms,
                    })
                    .map_err(|error| StoreError::InvalidData {
                        entity: "user input request",
                        detail: error.to_string(),
                    })?,
                ),
                None,
            )),
            _ => None,
        };
        if let Some((provider, request_id, operation, scope, request_json, details_json)) =
            approval_fields
        {
            if provider != run.provider {
                return Err(StoreError::InvalidData {
                    entity: "approval provider",
                    detail: "does not match provider run".to_owned(),
                });
            }
            let approval_id = ApprovalId::new();
            let question_count = match &record {
                ProviderEventRecord::UserInputRequested { questions, .. } => questions.len(),
                _ => 0,
            };
            sqlx::query(
                "INSERT INTO approvals \
                 (id, conversation_id, run_id, agent_id, provider, provider_request_id, operation, scope, request_json, details_json, question_count, status, resolution_json, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', NULL, ?, ?)",
            )
            .bind(approval_id.to_string())
            .bind(run.conversation_id.to_string())
            .bind(run_id.to_string())
            .bind(agent_id.to_string())
            .bind(provider_label(provider))
            .bind(request_id)
            .bind(operation)
            .bind(scope)
            .bind(request_json)
            .bind(details_json)
            .bind(i64::try_from(question_count).map_err(|_| StoreError::InvalidData {
                entity: "approval question count",
                detail: "count exceeds the supported range".to_owned(),
            })?)
            .bind(now)
            .bind(now)
            .execute(&mut *transaction)
            .await?;
            if let ProviderEventRecord::UserInputRequested { questions, .. } = &record {
                persist_approval_questions(&mut transaction, approval_id, questions).await?;
            }
        }

        if let Some(resolution) = terminal_approval_resolution {
            sqlx::query(
                "UPDATE approvals SET status = ?, resolution_json = ?, \
                 response_intent_status = CASE \
                    WHEN response_intent_status = 'recorded' THEN 'dispatch_unknown' \
                    ELSE response_intent_status END, updated_at = ? \
                 WHERE run_id = ? AND agent_id = ? AND status = 'pending'",
            )
            .bind(approval_resolution_label(&resolution))
            .bind(serialize_approval_resolution(&resolution)?)
            .bind(now)
            .bind(run_id.to_string())
            .bind(agent_id.to_string())
            .execute(&mut *transaction)
            .await?;
        }

        let result = sqlx::query(
            "INSERT INTO events \
             (id, conversation_id, run_id, agent_id, kind, role, content, payload_json, native_item_id, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(event_id.to_string())
        .bind(run.conversation_id.to_string())
        .bind(run_id.to_string())
        .bind(event_agent_id.to_string())
        .bind(event_kind_label(kind))
        .bind((kind == TimelineEventKind::Message).then_some("assistant"))
        .bind(content)
        .bind(payload_json)
        .bind(native_item_id)
        .bind(now)
        .execute(&mut *transaction)
        .await?;

        if is_root {
            match &record {
                ProviderEventRecord::Message(content) => {
                    persist_assistant_message_in_transaction(
                        &mut transaction,
                        run.id,
                        run.conversation_id,
                        run.provider,
                        None,
                        content,
                        now,
                    )
                    .await?;
                }
                ProviderEventRecord::NativeMessage {
                    content,
                    native_item_id,
                } => {
                    persist_assistant_message_in_transaction(
                        &mut transaction,
                        run.id,
                        run.conversation_id,
                        run.provider,
                        Some(native_item_id),
                        content,
                        now,
                    )
                    .await?;
                }
                _ => {}
            }
        }

        if let Some((next_run_status, next_agent_status)) = record.transition() {
            if is_root {
                run.transition(next_run_status)?;
            }
            validate_agent_transition(agent.status, next_agent_status)?;

            if is_root {
                sqlx::query("UPDATE provider_runs SET status = ?, updated_at = ? WHERE id = ?")
                    .bind(run_status_label(run.status))
                    .bind(now)
                    .bind(run_id.to_string())
                    .execute(&mut *transaction)
                    .await?;
            }
            sqlx::query("UPDATE agent_nodes SET status = ?, updated_at = ? WHERE id = ?")
                .bind(agent_status_label(next_agent_status))
                .bind(now)
                .bind(agent_id.to_string())
                .execute(&mut *transaction)
                .await?;
        }

        let event_mutation = match &record {
            ProviderEventRecord::Tool { mutation, .. }
            | ProviderEventRecord::NativeItem { mutation, .. }
            | ProviderEventRecord::FailedWithMutation { mutation, .. }
            | ProviderEventRecord::InterruptedWithMutation(mutation)
            | ProviderEventRecord::ProviderFailed { mutation, .. } => Some(*mutation),
            _ => None,
        };
        if let Some(mutation) = event_mutation {
            let next = merge_mutation_state(run.mutation_state, mutation);
            run.mutation_state = next;
            sqlx::query("UPDATE provider_runs SET mutation_state = ?, updated_at = ? WHERE id = ?")
                .bind(mutation_state_label(next))
                .bind(now)
                .bind(run_id.to_string())
                .execute(&mut *transaction)
                .await?;
        }
        if let ProviderEventRecord::ProviderFailed {
            dispatch_certainty, ..
        } = record
        {
            run.dispatch_certainty = Some(dispatch_certainty);
            sqlx::query(
                "UPDATE provider_runs SET dispatch_certainty = ?, updated_at = ? WHERE id = ?",
            )
            .bind(dispatch_certainty_label(dispatch_certainty))
            .bind(now)
            .bind(run_id.to_string())
            .execute(&mut *transaction)
            .await?;
        }

        let fallback = match fallback {
            Some((fallback, dispatch_claim)) => Some(
                insert_atomic_fallback(&mut transaction, &run, fallback, dispatch_claim, now)
                    .await?,
            ),
            None => None,
        };

        sqlx::query("UPDATE conversations SET updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(run.conversation_id.to_string())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        self.notify_change(run.conversation_id, run_id);
        if let Some((fallback_run, _)) = &fallback {
            self.notify_change(run.conversation_id, fallback_run.id);
        }

        Ok((
            TimelineEvent {
                id: event_id,
                conversation_id: run.conversation_id,
                run_id,
                agent_id: event_agent_id,
                sequence: u64::try_from(result.last_insert_rowid()).map_err(|_| {
                    StoreError::InvalidData {
                        entity: "timeline event",
                        detail: "negative sequence".to_owned(),
                    }
                })?,
                kind,
                role: (kind == TimelineEventKind::Message).then_some(MessageRole::Assistant),
                content: content.to_owned(),
            },
            fallback,
        ))
    }

    pub async fn stage_waiting_event(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        record: ProviderEventRecord,
    ) -> Result<StageWaitingEventOutcome, StoreError> {
        self.stage_waiting_event_inner(run_id, agent_id, record, None)
            .await
    }

    pub(crate) async fn stage_owned_waiting_event(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        expected_owner_id: &str,
        record: ProviderEventRecord,
    ) -> Result<StageWaitingEventOutcome, StoreError> {
        self.stage_waiting_event_inner(run_id, agent_id, record, Some(expected_owner_id))
            .await
    }

    async fn stage_waiting_event_inner(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        record: ProviderEventRecord,
        expected_owner_id: Option<&str>,
    ) -> Result<StageWaitingEventOutcome, StoreError> {
        let canonical_record = record.clone();
        let native_item_id = match &record {
            ProviderEventRecord::NativeMessage { native_item_id, .. } => {
                Some(native_item_id.clone())
            }
            _ => None,
        };
        let payload_json = record.payload_json();
        let (kind, content, mutation_state) = match record {
            ProviderEventRecord::Message(content) => (TimelineEventKind::Message, content, None),
            ProviderEventRecord::NativeMessage { content, .. } => {
                (TimelineEventKind::Message, content, None)
            }
            ProviderEventRecord::Progress(content) => (TimelineEventKind::Progress, content, None),
            ProviderEventRecord::Tool { content, mutation } => {
                (TimelineEventKind::Tool, content, Some(mutation))
            }
            ProviderEventRecord::NativeItem {
                content, mutation, ..
            } => (TimelineEventKind::Tool, content, Some(mutation)),
            ProviderEventRecord::ChildAgent { operation, .. } => {
                (TimelineEventKind::Progress, operation, None)
            }
            ProviderEventRecord::SubAgent { .. } => (
                TimelineEventKind::Progress,
                "Agent activity".to_owned(),
                None,
            ),
            ProviderEventRecord::Unrecognized { method } => {
                (TimelineEventKind::Diagnostic, method, None)
            }
            _ => return Err(StoreError::InvalidStagedEvent),
        };
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(expected_owner_id) = expected_owner_id {
            require_dispatch_owner(&mut transaction, run_id, expected_owner_id).await?;
        }
        let run = sqlx::query_as::<_, ProviderRunRow>(
            "SELECT id, conversation_id, provider, fallback_from_run_id, native_session_id, status, mutation_state, dispatch_certainty, created_at \
             FROM provider_runs WHERE id = ?",
        )
        .bind(run_id.to_string())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            entity: "provider run",
            id: run_id.to_string(),
        })?
        .into_domain()?;
        let agent = sqlx::query_as::<_, AgentNodeRow>(
            "SELECT id, run_id, parent_id, provider, provider_native_id, provider_native_path, label, summary, status, created_at \
             FROM agent_nodes WHERE id = ? AND run_id = ?",
        )
        .bind(agent_id.to_string())
        .bind(run_id.to_string())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            entity: "agent node",
            id: agent_id.to_string(),
        })?
        .into_domain()?;
        if run.status != RunStatus::Waiting || agent.status != AgentStatus::Waiting {
            return Err(StoreError::InvalidEventState {
                event: "stage waiting event",
                status: if run.status != RunStatus::Waiting {
                    run_status_label(run.status)
                } else {
                    agent_status_label(agent.status)
                },
            });
        }
        let event_agent_id = match &canonical_record {
            ProviderEventRecord::ChildAgent {
                parent_native_thread_id,
                child_native_thread_ids,
                child_statuses,
                ..
            } => {
                materialize_child_agents(
                    &mut transaction,
                    &run,
                    &agent,
                    parent_native_thread_id,
                    child_native_thread_ids,
                    child_statuses,
                    now_millis(),
                )
                .await?
            }
            ProviderEventRecord::SubAgent {
                agent_thread_id,
                agent_path,
                activity,
                ..
            } => {
                update_sub_agent(
                    &mut transaction,
                    &run,
                    &agent,
                    agent_thread_id,
                    agent_path,
                    *activity,
                    now_millis(),
                )
                .await?
            }
            _ => agent_id,
        };

        let existing_message = if let Some(native_item_id) = native_item_id.as_deref() {
            sqlx::query_as::<_, (String, i64, String, Option<String>)>(
                "SELECT id, sequence, content, payload_json FROM staged_provider_events \
                 WHERE run_id = ? AND agent_id = ? AND kind = 'message' AND native_item_id = ?",
            )
            .bind(run_id.to_string())
            .bind(event_agent_id.to_string())
            .bind(native_item_id)
            .fetch_optional(&mut *transaction)
            .await?
        } else {
            None
        };

        let (staged_count, staged_bytes, overflowed): (i64, i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(content_bytes), 0), COALESCE(MAX(overflowed), 0) \
             FROM ( \
                 SELECT length(CAST(content AS BLOB)) + \
                        COALESCE(length(CAST(payload_json AS BLOB)), 0) AS content_bytes, \
                        CASE WHEN overflowed_kind IS NULL THEN 0 ELSE 1 END AS overflowed \
                 FROM staged_provider_events WHERE run_id = ? \
                 ORDER BY sequence, id LIMIT ? \
             )",
        )
        .bind(run_id.to_string())
        .bind(i64::try_from(MAX_STAGED_EVENT_ROWS + 1).unwrap())
        .fetch_one(&mut *transaction)
        .await?;
        if staged_count > i64::try_from(MAX_STAGED_EVENT_ROWS).unwrap()
            || staged_bytes > i64::try_from(MAX_STAGED_EVENT_BYTES).unwrap()
        {
            return Err(StoreError::CorruptStagedEventQueue);
        }
        if overflowed != 0 {
            return Err(StoreError::StagedEventOverflowed);
        }

        let incoming_bytes = content.len().saturating_add(if existing_message.is_some() {
            0
        } else {
            payload_json.as_ref().map_or(0, String::len)
        });
        let normal_row_limit = MAX_STAGED_EVENT_ROWS - 1;
        let normal_byte_limit = MAX_STAGED_EVENT_BYTES - STAGED_OVERFLOW_CONTENT.len();
        if usize::try_from(staged_count).unwrap() > normal_row_limit
            || usize::try_from(staged_bytes).unwrap() > normal_byte_limit
        {
            return Err(StoreError::CorruptStagedEventQueue);
        }
        let exceeds_limit = (existing_message.is_none()
            && usize::try_from(staged_count).unwrap() >= normal_row_limit)
            || usize::try_from(staged_bytes)
                .unwrap()
                .saturating_add(incoming_bytes)
                > normal_byte_limit;
        if !exceeds_limit
            && let Some((existing_id, sequence, existing_content, existing_payload)) =
                existing_message
        {
            sqlx::query("UPDATE staged_provider_events SET content = content || ? WHERE id = ?")
                .bind(&content)
                .bind(&existing_id)
                .execute(&mut *transaction)
                .await?;
            let now = now_millis();
            sqlx::query("UPDATE conversations SET updated_at = ? WHERE id = ?")
                .bind(now)
                .bind(run.conversation_id.to_string())
                .execute(&mut *transaction)
                .await?;
            transaction.commit().await?;
            self.notify_change(run.conversation_id, run_id);
            return Ok(StageWaitingEventOutcome::Staged(StagedProviderEvent {
                id: parse_uuid("staged provider event", &existing_id)?.into(),
                conversation_id: run.conversation_id,
                run_id,
                agent_id: event_agent_id,
                sequence: u64::try_from(sequence).map_err(|_| StoreError::InvalidData {
                    entity: "staged provider event",
                    detail: "negative sequence".to_owned(),
                })?,
                kind,
                content: existing_content + &content,
                native_item_id,
                payload_json: existing_payload,
                mutation_state: None,
                overflowed_kind: None,
            }));
        }
        let (stored_kind, stored_content, stored_mutation, stored_payload, overflowed_kind) =
            if exceeds_limit {
                (
                    TimelineEventKind::Diagnostic,
                    STAGED_OVERFLOW_CONTENT.to_owned(),
                    Some(MutationState::Unknown),
                    None,
                    Some(kind),
                )
            } else {
                (kind, content, mutation_state, payload_json, None)
            };

        let id = TimelineEventId::new();
        let now = now_millis();
        let inserted = sqlx::query(
            "INSERT INTO staged_provider_events \
             (id, conversation_id, run_id, agent_id, kind, content, payload_json, native_item_id, \
              mutation_state, overflowed_kind, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(run.conversation_id.to_string())
        .bind(run_id.to_string())
        .bind(event_agent_id.to_string())
        .bind(event_kind_label(stored_kind))
        .bind(&stored_content)
        .bind(&stored_payload)
        .bind(
            (overflowed_kind.is_none())
                .then_some(native_item_id.as_deref())
                .flatten(),
        )
        .bind(stored_mutation.map(mutation_state_label))
        .bind(overflowed_kind.map(event_kind_label))
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        if let Some(mutation) = stored_mutation {
            let next = merge_mutation_state(run.mutation_state, mutation);
            sqlx::query("UPDATE provider_runs SET mutation_state = ?, updated_at = ? WHERE id = ?")
                .bind(mutation_state_label(next))
                .bind(now)
                .bind(run_id.to_string())
                .execute(&mut *transaction)
                .await?;
        }
        sqlx::query("UPDATE conversations SET updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(run.conversation_id.to_string())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        self.notify_change(run.conversation_id, run_id);

        let staged = StagedProviderEvent {
            id,
            conversation_id: run.conversation_id,
            run_id,
            agent_id: event_agent_id,
            sequence: u64::try_from(inserted.last_insert_rowid()).map_err(|_| {
                StoreError::InvalidData {
                    entity: "staged provider event",
                    detail: "negative sequence".to_owned(),
                }
            })?,
            kind: stored_kind,
            content: stored_content,
            native_item_id: (overflowed_kind.is_none())
                .then_some(native_item_id)
                .flatten(),
            payload_json: stored_payload,
            mutation_state: stored_mutation,
            overflowed_kind,
        };
        Ok(if exceeds_limit {
            StageWaitingEventOutcome::Overflowed(staged)
        } else {
            StageWaitingEventOutcome::Staged(staged)
        })
    }

    pub async fn record_response_intent(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        provider_request_id: &str,
        resolution: ApprovalResolution,
    ) -> Result<Approval, StoreError> {
        self.record_response_intent_inner(run_id, agent_id, provider_request_id, resolution, None)
            .await
    }

    pub(crate) async fn record_owned_response_intent(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        provider_request_id: &str,
        resolution: ApprovalResolution,
        expected_owner_id: &str,
    ) -> Result<Approval, StoreError> {
        self.record_response_intent_inner(
            run_id,
            agent_id,
            provider_request_id,
            resolution,
            Some(expected_owner_id),
        )
        .await
    }

    async fn record_response_intent_inner(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        provider_request_id: &str,
        resolution: ApprovalResolution,
        expected_owner_id: Option<&str>,
    ) -> Result<Approval, StoreError> {
        if matches!(
            resolution,
            ApprovalResolution::Cancelled | ApprovalResolution::Failed
        ) {
            return Err(StoreError::InvalidApprovalResolution);
        }
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(expected_owner_id) = expected_owner_id {
            require_dispatch_owner(&mut transaction, run_id, expected_owner_id).await?;
        }
        let (run_status, conversation_id): (String, String) =
            sqlx::query_as("SELECT status, conversation_id FROM provider_runs WHERE id = ?")
                .bind(run_id.to_string())
                .fetch_optional(&mut *transaction)
                .await?
                .ok_or_else(|| StoreError::NotFound {
                    entity: "provider run",
                    id: run_id.to_string(),
                })?;
        let agent_status: String =
            sqlx::query_scalar("SELECT status FROM agent_nodes WHERE id = ? AND run_id = ?")
                .bind(agent_id.to_string())
                .bind(run_id.to_string())
                .fetch_optional(&mut *transaction)
                .await?
                .ok_or_else(|| StoreError::NotFound {
                    entity: "agent node",
                    id: agent_id.to_string(),
                })?;
        if run_status != "waiting" || agent_status != "waiting" {
            return Err(StoreError::InvalidEventState {
                event: "approval response intent",
                status: run_status_label(parse_run_status(&run_status)?),
            });
        }
        let mut approval = sqlx::query_as::<_, ApprovalRow>(
            "SELECT id, run_id, agent_id, provider, provider_request_id, operation, scope, request_json, details_json, status, resolution_json, \
             response_intent_json, response_intent_status \
             FROM approvals WHERE run_id = ? AND agent_id = ? AND provider_request_id = ?",
        )
        .bind(run_id.to_string())
        .bind(agent_id.to_string())
        .bind(provider_request_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            entity: "approval",
            id: provider_request_id.to_owned(),
        })?
        .into_domain()?;
        if approval.status != ApprovalStatus::Pending {
            return Err(StoreError::NotFound {
                entity: "pending approval",
                id: provider_request_id.to_owned(),
            });
        }
        if approval
            .response_intent
            .as_ref()
            .is_some_and(|intent| intent.status != ApprovalResponseIntentStatus::Rejected)
        {
            return Err(StoreError::ApprovalResponseIntentExists);
        }
        let now = now_millis();
        let result = sqlx::query(
            "UPDATE approvals SET response_intent_json = ?, response_intent_status = 'recorded', \
             updated_at = ? WHERE run_id = ? AND agent_id = ? AND provider_request_id = ? \
             AND status = 'pending' AND \
             (response_intent_status IS NULL OR response_intent_status = 'rejected')",
        )
        .bind(serialize_approval_resolution(&resolution)?)
        .bind(now)
        .bind(run_id.to_string())
        .bind(agent_id.to_string())
        .bind(provider_request_id)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StoreError::ApprovalResponseIntentExists);
        }
        approval.response_intent = Some(ApprovalResponseIntent {
            resolution,
            status: ApprovalResponseIntentStatus::Recorded,
        });
        transaction.commit().await?;
        self.notify_change(parse_uuid("conversation", &conversation_id)?.into(), run_id);
        Ok(approval)
    }

    pub async fn reject_response_intent(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        provider_request_id: &str,
        dispatch_certainty: DispatchCertainty,
    ) -> Result<Approval, StoreError> {
        self.reject_response_intent_inner(
            run_id,
            agent_id,
            provider_request_id,
            dispatch_certainty,
            None,
        )
        .await
    }

    pub(crate) async fn reject_owned_response_intent(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        provider_request_id: &str,
        dispatch_certainty: DispatchCertainty,
        expected_owner_id: &str,
    ) -> Result<Approval, StoreError> {
        self.reject_response_intent_inner(
            run_id,
            agent_id,
            provider_request_id,
            dispatch_certainty,
            Some(expected_owner_id),
        )
        .await
    }

    async fn reject_response_intent_inner(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        provider_request_id: &str,
        dispatch_certainty: DispatchCertainty,
        expected_owner_id: Option<&str>,
    ) -> Result<Approval, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(expected_owner_id) = expected_owner_id {
            require_dispatch_owner(&mut transaction, run_id, expected_owner_id).await?;
        }
        let conversation_id: String =
            sqlx::query_scalar("SELECT conversation_id FROM provider_runs WHERE id = ?")
                .bind(run_id.to_string())
                .fetch_one(&mut *transaction)
                .await?;
        let mut approval = sqlx::query_as::<_, ApprovalRow>(
            "SELECT id, run_id, agent_id, provider, provider_request_id, operation, scope, request_json, details_json, status, resolution_json, \
             response_intent_json, response_intent_status \
             FROM approvals WHERE run_id = ? AND agent_id = ? AND provider_request_id = ?",
        )
        .bind(run_id.to_string())
        .bind(agent_id.to_string())
        .bind(provider_request_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            entity: "approval",
            id: provider_request_id.to_owned(),
        })?
        .into_domain()?;
        if approval.status != ApprovalStatus::Pending
            || approval
                .response_intent
                .as_ref()
                .is_none_or(|intent| intent.status != ApprovalResponseIntentStatus::Recorded)
        {
            return Err(StoreError::InvalidApprovalResponseIntentState);
        }
        let intent_status = match dispatch_certainty {
            DispatchCertainty::NotDispatched => ApprovalResponseIntentStatus::Rejected,
            DispatchCertainty::MayHaveDispatched => ApprovalResponseIntentStatus::DispatchUnknown,
        };
        let result = sqlx::query(
            "UPDATE approvals SET response_intent_status = ?, updated_at = ? \
             WHERE run_id = ? AND agent_id = ? AND provider_request_id = ? \
             AND status = 'pending' AND response_intent_status = 'recorded'",
        )
        .bind(approval_response_intent_status_label(intent_status))
        .bind(now_millis())
        .bind(run_id.to_string())
        .bind(agent_id.to_string())
        .bind(provider_request_id)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StoreError::InvalidApprovalResponseIntentState);
        }
        approval.response_intent.as_mut().unwrap().status = intent_status;
        transaction.commit().await?;
        self.notify_change(parse_uuid("conversation", &conversation_id)?.into(), run_id);
        Ok(approval)
    }

    pub async fn acknowledge_response_intent(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        provider_request_id: &str,
    ) -> Result<TimelineEvent, StoreError> {
        self.acknowledge_response_intent_inner(run_id, agent_id, provider_request_id, None)
            .await
    }

    pub(crate) async fn acknowledge_owned_response_intent(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        provider_request_id: &str,
        expected_owner_id: &str,
    ) -> Result<TimelineEvent, StoreError> {
        self.acknowledge_response_intent_inner(
            run_id,
            agent_id,
            provider_request_id,
            Some(expected_owner_id),
        )
        .await
    }

    async fn acknowledge_response_intent_inner(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        provider_request_id: &str,
        expected_owner_id: Option<&str>,
    ) -> Result<TimelineEvent, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(expected_owner_id) = expected_owner_id {
            require_dispatch_owner(&mut transaction, run_id, expected_owner_id).await?;
        }
        let run_row = sqlx::query_as::<_, ProviderRunRow>(
            "SELECT id, conversation_id, provider, fallback_from_run_id, native_session_id, status, mutation_state, dispatch_certainty, created_at \
             FROM provider_runs WHERE id = ?",
        )
        .bind(run_id.to_string())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            entity: "provider run",
            id: run_id.to_string(),
        })?;
        let mut run = run_row.into_domain()?;
        let agent_row = sqlx::query_as::<_, AgentNodeRow>(
            "SELECT id, run_id, parent_id, provider, provider_native_id, provider_native_path, label, summary, status, created_at \
             FROM agent_nodes WHERE id = ? AND run_id = ?",
        )
        .bind(agent_id.to_string())
        .bind(run_id.to_string())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            entity: "agent node",
            id: agent_id.to_string(),
        })?;
        let agent = agent_row.into_domain()?;
        let approval = sqlx::query_as::<_, ApprovalRow>(
            "SELECT id, run_id, agent_id, provider, provider_request_id, operation, scope, request_json, details_json, status, resolution_json, \
             response_intent_json, response_intent_status \
             FROM approvals WHERE run_id = ? AND agent_id = ? AND provider_request_id = ?",
        )
        .bind(run_id.to_string())
        .bind(agent_id.to_string())
        .bind(provider_request_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            entity: "approval",
            id: provider_request_id.to_owned(),
        })?
        .into_domain()?;
        if approval
            .response_intent
            .as_ref()
            .is_some_and(|intent| intent.status == ApprovalResponseIntentStatus::Acknowledged)
        {
            return Err(StoreError::ApprovalResponseAlreadyAcknowledged);
        }
        let resolution = match approval.response_intent {
            Some(ApprovalResponseIntent {
                resolution,
                status: ApprovalResponseIntentStatus::Recorded,
            }) if approval.status == ApprovalStatus::Pending => resolution,
            _ => return Err(StoreError::InvalidApprovalResponseIntentState),
        };
        if run.status != RunStatus::Waiting || agent.status != AgentStatus::Waiting {
            return Err(StoreError::InvalidEventState {
                event: "approval response acknowledgement",
                status: if run.status != RunStatus::Waiting {
                    run_status_label(run.status)
                } else {
                    agent_status_label(agent.status)
                },
            });
        }
        let now = now_millis();
        let result = sqlx::query(
            "UPDATE approvals SET status = ?, resolution_json = response_intent_json, \
             response_intent_status = 'acknowledged', updated_at = ? \
             WHERE run_id = ? AND agent_id = ? AND provider_request_id = ? \
             AND status = 'pending' AND response_intent_status = 'recorded'",
        )
        .bind(approval_resolution_label(&resolution))
        .bind(now)
        .bind(run_id.to_string())
        .bind(agent_id.to_string())
        .bind(provider_request_id)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StoreError::InvalidApprovalResponseIntentState);
        }
        run.transition(RunStatus::Running)?;
        validate_agent_transition(agent.status, AgentStatus::Running)?;
        let event_id = TimelineEventId::new();
        let content = "Provider run resumed";
        let inserted = sqlx::query(
            "INSERT INTO events \
             (id, conversation_id, run_id, agent_id, kind, content, payload_json, created_at) \
             VALUES (?, ?, ?, ?, 'lifecycle', ?, ?, ?)",
        )
        .bind(event_id.to_string())
        .bind(run.conversation_id.to_string())
        .bind(run_id.to_string())
        .bind(agent_id.to_string())
        .bind(content)
        .bind(
            serde_json::json!({
                "requestId": provider_request_id,
                "resolution": resolution,
            })
            .to_string(),
        )
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("UPDATE provider_runs SET status = 'running', updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(run_id.to_string())
            .execute(&mut *transaction)
            .await?;
        sqlx::query("UPDATE agent_nodes SET status = 'running', updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(agent_id.to_string())
            .execute(&mut *transaction)
            .await?;
        drain_staged_events_in_transaction(&mut transaction, run_id, Some(agent_id)).await?;
        sqlx::query("UPDATE conversations SET updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(run.conversation_id.to_string())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        self.notify_change(run.conversation_id, run_id);
        Ok(TimelineEvent {
            id: event_id,
            conversation_id: run.conversation_id,
            run_id,
            agent_id,
            sequence: u64::try_from(inserted.last_insert_rowid()).map_err(|_| {
                StoreError::InvalidData {
                    entity: "timeline event",
                    detail: "negative sequence".to_owned(),
                }
            })?,
            kind: TimelineEventKind::Lifecycle,
            role: None,
            content: content.to_owned(),
        })
    }

    pub async fn load_approval(
        &self,
        run_id: RunId,
        provider_request_id: &str,
    ) -> Result<Approval, StoreError> {
        sqlx::query_as::<_, ApprovalRow>(
            "SELECT id, run_id, agent_id, provider, provider_request_id, operation, scope, request_json, details_json, status, resolution_json, \
             response_intent_json, response_intent_status \
             FROM approvals WHERE run_id = ? AND provider_request_id = ?",
        )
        .bind(run_id.to_string())
        .bind(provider_request_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            entity: "approval",
            id: provider_request_id.to_owned(),
        })?
        .into_domain()
    }

    pub async fn load_approval_by_id(
        &self,
        approval_id: ApprovalId,
    ) -> Result<Approval, StoreError> {
        sqlx::query_as::<_, ApprovalRow>(
            "SELECT id, run_id, agent_id, provider, provider_request_id, operation, scope, \
                    request_json, details_json, status, resolution_json, response_intent_json, \
                    response_intent_status FROM approvals WHERE id = ?",
        )
        .bind(approval_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            entity: "approval",
            id: approval_id.to_string(),
        })?
        .into_domain()
    }

    pub async fn load_approval_detail(
        &self,
        approval_id: ApprovalId,
    ) -> Result<ApprovalDetailRecord, StoreError> {
        let mut detail = sqlx::query_as::<_, ApprovalDetailRow>(
            "SELECT id, question_count, status, response_intent_status, \
                    CASE WHEN length(CAST(operation AS BLOB)) + length(CAST(scope AS BLOB)) + \
                                   COALESCE(length(CAST(request_json AS BLOB)), 0) + \
                                   COALESCE(length(CAST(details_json AS BLOB)), 0) <= ? \
                         THEN operation ELSE substr(operation, 1, 256) END AS operation, \
                    CASE WHEN length(CAST(operation AS BLOB)) + length(CAST(scope AS BLOB)) + \
                                   COALESCE(length(CAST(request_json AS BLOB)), 0) + \
                                   COALESCE(length(CAST(details_json AS BLOB)), 0) <= ? \
                         THEN scope ELSE substr(scope, 1, 512) END AS scope, \
                    CASE WHEN length(CAST(operation AS BLOB)) + length(CAST(scope AS BLOB)) + \
                                   COALESCE(length(CAST(request_json AS BLOB)), 0) + \
                                   COALESCE(length(CAST(details_json AS BLOB)), 0) <= ? \
                         THEN request_json END AS request_json, \
                    CASE WHEN length(CAST(operation AS BLOB)) + length(CAST(scope AS BLOB)) + \
                                   COALESCE(length(CAST(request_json AS BLOB)), 0) + \
                                   COALESCE(length(CAST(details_json AS BLOB)), 0) <= ? \
                         THEN details_json END AS details_json, \
                    (length(CAST(operation AS BLOB)) + length(CAST(scope AS BLOB)) + \
                     COALESCE(length(CAST(request_json AS BLOB)), 0) + \
                     COALESCE(length(CAST(details_json AS BLOB)), 0) > ?) AS truncated \
             FROM approvals WHERE id = ?",
        )
        .bind(MAX_APPROVAL_DETAIL_SOURCE_BYTES)
        .bind(MAX_APPROVAL_DETAIL_SOURCE_BYTES)
        .bind(MAX_APPROVAL_DETAIL_SOURCE_BYTES)
        .bind(MAX_APPROVAL_DETAIL_SOURCE_BYTES)
        .bind(MAX_APPROVAL_DETAIL_SOURCE_BYTES)
        .bind(approval_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            entity: "approval",
            id: approval_id.to_string(),
        })?
        .into_record()?;

        let (agent_path, agent_path_truncated) = self.load_approval_agent_path(approval_id).await?;
        detail.agent_path = agent_path;
        detail.agent_path_truncated = agent_path_truncated;
        Ok(detail)
    }

    async fn load_approval_agent_path(
        &self,
        approval_id: ApprovalId,
    ) -> Result<(Vec<String>, bool), StoreError> {
        let mut leaf_to_root = sqlx::query_scalar::<_, String>(
            "WITH RECURSIVE ancestry(id, parent_id, label, depth) AS (\
                 SELECT agent_nodes.id, agent_nodes.parent_id, agent_nodes.label, 0 \
                 FROM agent_nodes \
                 JOIN approvals ON approvals.agent_id = agent_nodes.id \
                                  AND approvals.run_id = agent_nodes.run_id \
                 WHERE approvals.id = ? \
                 UNION ALL \
                 SELECT parent.id, parent.parent_id, parent.label, ancestry.depth + 1 \
                 FROM agent_nodes AS parent \
                 JOIN ancestry ON ancestry.parent_id = parent.id \
                 WHERE ancestry.depth < ?\
             ) \
             SELECT substr(label, 1, ?) FROM ancestry ORDER BY depth ASC LIMIT ?",
        )
        .bind(approval_id.to_string())
        .bind(i64::try_from(MAX_APPROVAL_AGENT_PATH_NODES).expect("path limit fits i64"))
        .bind(i64::try_from(MAX_AGENT_LABEL_PREVIEW_BYTES).expect("label limit fits i64"))
        .bind(i64::try_from(MAX_APPROVAL_AGENT_PATH_NODES + 1).expect("path limit fits i64"))
        .fetch_all(&self.pool)
        .await?;
        let truncated = leaf_to_root.len() > MAX_APPROVAL_AGENT_PATH_NODES;
        leaf_to_root.truncate(MAX_APPROVAL_AGENT_PATH_NODES);
        leaf_to_root.iter_mut().for_each(|label| {
            *label = truncate_utf8(std::mem::take(label), MAX_AGENT_LABEL_PREVIEW_BYTES);
        });
        leaf_to_root.reverse();
        Ok((leaf_to_root, truncated))
    }

    pub async fn load_approval_questions(
        &self,
        approval_id: ApprovalId,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<ApprovalQuestionPage, StoreError> {
        if !(1..=MAX_APPROVAL_QUESTION_PAGE_SIZE).contains(&limit) {
            return Err(StoreError::InvalidPageLimit(limit));
        }
        let offset = match cursor {
            Some(cursor) => {
                let cursor = decode_question_cursor(&cursor)?;
                if cursor.approval_id != approval_id {
                    return Err(StoreError::InvalidCursor);
                }
                cursor.offset
            }
            None => 0,
        };
        let total_count: i64 =
            sqlx::query_scalar("SELECT question_count FROM approvals WHERE id = ?")
                .bind(approval_id.to_string())
                .fetch_optional(&self.pool)
                .await?
                .ok_or_else(|| StoreError::NotFound {
                    entity: "approval",
                    id: approval_id.to_string(),
                })?;
        let rows = sqlx::query_as::<_, ApprovalQuestionRow>(
            "SELECT ordinal, header, question, options_json, is_other, is_secret, \
                    source_bytes, header_bytes, question_bytes \
             FROM approval_questions WHERE approval_id = ? AND ordinal >= ? \
             ORDER BY ordinal LIMIT ?",
        )
        .bind(approval_id.to_string())
        .bind(i64::from(offset))
        .bind(i64::from(limit) + 1)
        .fetch_all(&self.pool)
        .await?;
        let has_more = rows.len() > limit as usize;
        let items = rows
            .into_iter()
            .take(limit as usize)
            .map(ApprovalQuestionRow::into_preview)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if has_more {
            let next_offset = offset.checked_add(limit).ok_or(StoreError::InvalidCursor)?;
            Some(encode_question_cursor(QuestionCursor {
                approval_id,
                offset: next_offset,
            })?)
        } else {
            None
        };
        Ok(ApprovalQuestionPage {
            items,
            total_count: u32::try_from(total_count).map_err(|_| StoreError::InvalidData {
                entity: "approval question count",
                detail: "count exceeds the supported range".to_owned(),
            })?,
            next_cursor,
        })
    }

    /// Atomically records a sanitized provider failure unless the run already reached a terminal
    /// state. This is used by the supervisor to reconcile tasks that fail outside their normal
    /// event loop, including panics.
    pub async fn fail_run_if_active(
        &self,
        run_id: RunId,
        root_id: AgentId,
        category: ProviderErrorCategory,
        mutation: MutationState,
        dispatch_certainty: DispatchCertainty,
    ) -> Result<bool, StoreError> {
        self.fail_run_if_active_inner(
            run_id,
            root_id,
            category,
            mutation,
            dispatch_certainty,
            None,
        )
        .await
    }

    pub(crate) async fn fail_owned_run_if_active(
        &self,
        run_id: RunId,
        root_id: AgentId,
        category: ProviderErrorCategory,
        mutation: MutationState,
        dispatch_certainty: DispatchCertainty,
        expected_owner_id: &str,
    ) -> Result<bool, StoreError> {
        self.fail_run_if_active_inner(
            run_id,
            root_id,
            category,
            mutation,
            dispatch_certainty,
            Some(expected_owner_id),
        )
        .await
    }

    async fn fail_run_if_active_inner(
        &self,
        run_id: RunId,
        root_id: AgentId,
        category: ProviderErrorCategory,
        mutation: MutationState,
        dispatch_certainty: DispatchCertainty,
        expected_owner_id: Option<&str>,
    ) -> Result<bool, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(expected_owner_id) = expected_owner_id {
            require_dispatch_owner(&mut transaction, run_id, expected_owner_id).await?;
        }
        let run = sqlx::query_as::<_, ProviderRunRow>(
            "SELECT id, conversation_id, provider, fallback_from_run_id, native_session_id, status, mutation_state, dispatch_certainty, created_at \
             FROM provider_runs WHERE id = ?",
        )
        .bind(run_id.to_string())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            entity: "provider run",
            id: run_id.to_string(),
        })?
        .into_domain()?;
        if is_terminal_run_status(run.status) {
            transaction.rollback().await?;
            return Ok(false);
        }
        let root_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM agent_nodes \
             WHERE id = ? AND run_id = ? AND parent_id IS NULL)",
        )
        .bind(root_id.to_string())
        .bind(run_id.to_string())
        .fetch_one(&mut *transaction)
        .await?;
        if !root_exists {
            return Err(StoreError::NotFound {
                entity: "root agent node",
                id: root_id.to_string(),
            });
        }

        drain_staged_events_in_transaction(&mut transaction, run_id, None).await?;

        let now = now_millis();
        let resolution = ApprovalResolution::Failed;
        sqlx::query(
            "UPDATE approvals SET status = 'failed', resolution_json = ?, \
             response_intent_status = CASE \
                WHEN response_intent_status = 'recorded' THEN 'dispatch_unknown' \
                ELSE response_intent_status END, updated_at = ? \
             WHERE run_id = ? AND status = 'pending'",
        )
        .bind(serialize_approval_resolution(&resolution)?)
        .bind(now)
        .bind(run_id.to_string())
        .execute(&mut *transaction)
        .await?;
        let event_id = TimelineEventId::new();
        let next_mutation = merge_mutation_state(run.mutation_state, mutation);
        let inserted = sqlx::query(
            "INSERT INTO events \
             (id, conversation_id, run_id, agent_id, kind, content, payload_json, created_at) \
             VALUES (?, ?, ?, ?, 'diagnostic', ?, ?, ?)",
        )
        .bind(event_id.to_string())
        .bind(run.conversation_id.to_string())
        .bind(run_id.to_string())
        .bind(root_id.to_string())
        .bind(provider_error_content(category))
        .bind(
            serde_json::json!({
                "errorCategory": category,
                "mutation": mutation,
                "dispatchCertainty": dispatch_certainty,
            })
            .to_string(),
        )
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        debug_assert!(inserted.last_insert_rowid() > 0);
        sqlx::query(
            "UPDATE provider_runs SET status = 'failed', mutation_state = ?, \
             dispatch_certainty = ?, updated_at = ? \
             WHERE id = ?",
        )
        .bind(mutation_state_label(next_mutation))
        .bind(dispatch_certainty_label(dispatch_certainty))
        .bind(now)
        .bind(run_id.to_string())
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE agent_nodes SET status = 'failed', updated_at = ? \
             WHERE run_id = ? AND status IN ('queued', 'running', 'waiting')",
        )
        .bind(now)
        .bind(run_id.to_string())
        .execute(&mut *transaction)
        .await?;
        sqlx::query("UPDATE conversations SET updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(run.conversation_id.to_string())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        self.notify_change(run.conversation_id, run_id);
        Ok(true)
    }

    /// Returns a page from the current `updated_at` ordering.
    ///
    /// This is a live refresh view, not a snapshot: activity can move a conversation ahead of an
    /// existing cursor. Consumers that need a refreshed sidebar should restart at the first page.
    pub async fn list_conversations(
        &self,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<Page<Conversation>, StoreError> {
        self.list_conversations_filtered(cursor, limit, false).await
    }

    pub async fn list_active_conversations(
        &self,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<Page<Conversation>, StoreError> {
        self.list_conversations_filtered(cursor, limit, true).await
    }

    async fn list_conversations_filtered(
        &self,
        cursor: Option<String>,
        limit: u32,
        active_only: bool,
    ) -> Result<Page<Conversation>, StoreError> {
        validate_page_limit(limit)?;
        let cursor = cursor.map(|value| decode_cursor(&value)).transpose()?;
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT id, substr(title, 1, 256) AS title, workspace_id, status, updated_at \
             FROM conversations WHERE 1 = 1",
        );
        if active_only {
            query.push(" AND status <> 'archived'");
        }
        if let Some(cursor) = cursor {
            let sequence = cursor_sequence_i64(&cursor)?;
            query.push(" AND (updated_at < ");
            query.push_bind(sequence);
            query.push(" OR (updated_at = ");
            query.push_bind(sequence);
            query.push(" AND id < ");
            query.push_bind(cursor.id.to_string());
            query.push("))");
        }
        query.push(" ORDER BY updated_at DESC, id DESC LIMIT ");
        query.push_bind(i64::from(limit) + 1);
        let rows = query
            .build_query_as::<ConversationRow>()
            .fetch_all(&self.pool)
            .await?;

        let mut records = rows
            .into_iter()
            .map(ConversationRow::into_record)
            .collect::<Result<Vec<_>, _>>()?;
        let has_more = records.len() > limit as usize;
        if has_more {
            records.truncate(limit as usize);
        }
        let next_cursor = if has_more {
            let last = records.last().expect("a page with more rows is non-empty");
            Some(encode_cursor(Cursor {
                sequence: u64::try_from(last.updated_at).map_err(|_| StoreError::InvalidData {
                    entity: "conversation",
                    detail: "negative updated_at".to_owned(),
                })?,
                id: last.conversation.id.as_uuid(),
            })?)
        } else {
            None
        };

        Ok(Page {
            items: records
                .into_iter()
                .map(|record| record.conversation)
                .collect(),
            next_cursor,
        })
    }

    pub async fn load_sidebar_details(
        &self,
        conversation_ids: &[ConversationId],
    ) -> Result<Vec<SidebarDetails>, StoreError> {
        if conversation_ids.is_empty() {
            return Ok(Vec::new());
        }
        if conversation_ids.len() > MAX_PAGE_SIZE as usize {
            return Err(StoreError::InvalidPageLimit(
                u32::try_from(conversation_ids.len()).unwrap_or(u32::MAX),
            ));
        }

        let mut query = QueryBuilder::<Sqlite>::new(
            "WITH selected(id) AS (SELECT id FROM conversations WHERE id IN (",
        );
        {
            let mut separated = query.separated(", ");
            for id in conversation_ids {
                separated.push_bind(id.to_string());
            }
        }
        query.push(
            ")) \
             SELECT selected.id AS conversation_id, \
                    COALESCE(conversation_settings.routing_profile, 'balanced') AS routing_profile, \
                    workspaces.project_root, \
                    latest_runs.id AS run_id, latest_runs.provider, \
                    latest_runs.fallback_from_run_id, latest_runs.native_session_id, \
                    latest_runs.status AS run_status, latest_runs.mutation_state, \
                    latest_runs.dispatch_certainty, latest_runs.created_at AS run_created_at, \
                    CASE WHEN latest_runs.waiting_agent_count > 0 THEN 'needs_attention' \
                         WHEN latest_runs.active_agent_count > 0 THEN 'active' \
                         WHEN latest_runs.failed_agent_count > 0 THEN 'failed' \
                         WHEN latest_runs.interrupted_agent_count > 0 THEN 'interrupted' \
                         WHEN latest_runs.id IS NOT NULL THEN 'completed' END AS rollup_status, \
                    latest_runs.active_descendant_count, \
                    latest_runs.agent_total_count AS total_agent_count, sidebar_agents.id AS agent_id, \
                    sidebar_agents.parent_id, sidebar_agents.provider AS agent_provider, \
                    NULL AS provider_native_id, NULL AS provider_native_path, \
                    substr(sidebar_agents.label, 1, 256) AS label, \
                    substr(sidebar_agents.summary, 1, 2048) AS summary, \
                    sidebar_agents.status AS agent_status, \
                    sidebar_agents.created_at AS agent_created_at \
             FROM selected \
             LEFT JOIN conversations ON conversations.id = selected.id \
             LEFT JOIN conversation_settings \
                    ON conversation_settings.conversation_id = conversations.id \
             LEFT JOIN workspaces ON workspaces.id = conversations.workspace_id \
             LEFT JOIN provider_runs AS latest_runs ON latest_runs.id = ( \
                 SELECT candidate.id FROM provider_runs AS candidate \
                 WHERE candidate.conversation_id = selected.id \
                 ORDER BY candidate.created_at DESC, candidate.id DESC LIMIT 1 \
             ) \
             LEFT JOIN agent_nodes AS sidebar_agents \
                    ON sidebar_agents.run_id = latest_runs.id AND sidebar_agents.parent_id IS NULL \
             ORDER BY selected.id",
        );
        let rows = query
            .build_query_as::<SidebarDetailRow>()
            .fetch_all(&self.pool)
            .await?;

        let mut by_id = std::collections::HashMap::new();
        for id in conversation_ids {
            by_id.insert(
                *id,
                SidebarDetails {
                    conversation_id: *id,
                    routing_profile: RoutingProfile::Balanced,
                    project_root: None,
                    run: None,
                    rollup_status: None,
                    active_descendant_count: 0,
                    agents: Vec::new(),
                    agents_truncated: false,
                },
            );
        }
        for row in rows {
            let conversation_id: ConversationId =
                parse_uuid("conversation", &row.conversation_id)?.into();
            let details =
                by_id
                    .get_mut(&conversation_id)
                    .ok_or_else(|| StoreError::InvalidData {
                        entity: "sidebar conversation",
                        detail: "query returned an unrequested conversation".to_owned(),
                    })?;
            details.project_root = row.project_root.clone().map(PathBuf::from);
            details.routing_profile = parse_routing_profile(&row.routing_profile)?;
            if details.run.is_none() {
                details.run = row.provider_run()?;
                details.rollup_status = row
                    .rollup_status
                    .as_deref()
                    .map(parse_rollup_status)
                    .transpose()?;
                details.active_descendant_count =
                    usize::try_from(row.active_descendant_count.unwrap_or(0)).map_err(|_| {
                        StoreError::InvalidData {
                            entity: "sidebar active descendant count",
                            detail: "negative count".to_owned(),
                        }
                    })?;
                details.agents_truncated = row.total_agent_count.unwrap_or(0) > 1;
            }
            if let Some(agent) = row.agent()? {
                details.agents.push(agent);
            }
        }
        conversation_ids
            .iter()
            .map(|id| {
                by_id.remove(id).ok_or_else(|| StoreError::InvalidData {
                    entity: "sidebar conversation",
                    detail: "requested conversation was omitted".to_owned(),
                })
            })
            .collect()
    }

    /// Returns a chronological page ending at the newest visible event. `next_cursor` requests
    /// the immediately older page, so the desktop can prepend history without hydrating it all.
    pub async fn load_recent_timeline(
        &self,
        conversation_id: ConversationId,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<Page<TimelineRecord>, StoreError> {
        validate_page_limit(limit)?;
        let cursor = cursor.map(|value| decode_cursor(&value)).transpose()?;
        let rows = if let Some(cursor) = cursor {
            sqlx::query_as::<_, TimelineRecordRow>(
                "SELECT events.id, events.conversation_id, events.run_id, events.agent_id, \
                        events.sequence, events.kind, events.role, substr(events.content, 1, 1024) AS content, \
                        length(CAST(events.content AS BLOB)) AS content_bytes, provider_runs.provider \
                 FROM events JOIN provider_runs ON provider_runs.id = events.run_id \
                 WHERE events.conversation_id = ? AND \
                       (events.sequence < ? OR (events.sequence = ? AND events.id < ?)) \
                 ORDER BY events.sequence DESC, events.id DESC LIMIT ?",
            )
            .bind(conversation_id.to_string())
            .bind(cursor_sequence_i64(&cursor)?)
            .bind(cursor_sequence_i64(&cursor)?)
            .bind(cursor.id.to_string())
            .bind(i64::from(limit) + 1)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, TimelineRecordRow>(
                "SELECT events.id, events.conversation_id, events.run_id, events.agent_id, \
                        events.sequence, events.kind, events.role, substr(events.content, 1, 1024) AS content, \
                        length(CAST(events.content AS BLOB)) AS content_bytes, provider_runs.provider \
                 FROM events JOIN provider_runs ON provider_runs.id = events.run_id \
                 WHERE events.conversation_id = ? \
                 ORDER BY events.sequence DESC, events.id DESC LIMIT ?",
            )
            .bind(conversation_id.to_string())
            .bind(i64::from(limit) + 1)
            .fetch_all(&self.pool)
            .await?
        };
        let has_more = rows.len() > limit as usize;
        let mut items = rows
            .into_iter()
            .take(limit as usize)
            .map(TimelineRecordRow::into_record)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if has_more {
            let oldest = items.last().expect("a page with more rows is non-empty");
            Some(encode_cursor(Cursor {
                sequence: oldest.event.sequence,
                id: oldest.event.id.as_uuid(),
            })?)
        } else {
            None
        };
        items.reverse();
        Ok(Page { items, next_cursor })
    }

    pub async fn load_recent_approvals(
        &self,
        conversation_id: ConversationId,
        limit: u32,
    ) -> Result<ApprovalPage, StoreError> {
        self.load_approvals(conversation_id, None, true, limit)
            .await
    }

    /// Pages pending approvals independently from history so every actionable request remains
    /// discoverable even when a conversation has a large approval history.
    pub async fn load_approvals(
        &self,
        conversation_id: ConversationId,
        cursor: Option<String>,
        pending: bool,
        limit: u32,
    ) -> Result<ApprovalPage, StoreError> {
        validate_page_limit(limit)?;
        let cursor = cursor.map(|value| decode_cursor(&value)).transpose()?;
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT approvals.id, approvals.run_id, approvals.agent_id, approvals.provider, \
                    substr(approvals.operation, 1, 256) AS operation, \
                    substr(approvals.scope, 1, 512) AS scope, approvals.status, \
                    approvals.response_intent_status, \
                    approvals.created_at \
             FROM approvals WHERE approvals.conversation_id = ",
        );
        query.push_bind(conversation_id.to_string());
        if pending {
            query.push(" AND approvals.status = 'pending'");
        } else {
            query.push(" AND approvals.status <> 'pending'");
        }
        if let Some(cursor) = cursor {
            let created_at = cursor_sequence_i64(&cursor)?;
            query.push(" AND (approvals.created_at < ");
            query.push_bind(created_at);
            query.push(" OR (approvals.created_at = ");
            query.push_bind(created_at);
            query.push(" AND approvals.id < ");
            query.push_bind(cursor.id.to_string());
            query.push("))");
        }
        query.push(" ORDER BY approvals.created_at DESC, approvals.id DESC LIMIT ");
        query.push_bind(i64::from(limit) + 1);
        let rows = query
            .build_query_as::<ApprovalListRow>()
            .fetch_all(&self.pool)
            .await?;
        let truncated = rows.len() > limit as usize;
        let rows = rows.into_iter().take(limit as usize).collect::<Vec<_>>();
        let next_cursor = if truncated {
            let last = rows.last().expect("a truncated approval page is non-empty");
            Some(encode_cursor(Cursor {
                sequence: u64::try_from(last.created_at).map_err(|_| StoreError::InvalidData {
                    entity: "approval",
                    detail: "negative creation timestamp".to_owned(),
                })?,
                id: parse_uuid("approval", &last.id)?,
            })?)
        } else {
            None
        };
        let mut items = rows
            .into_iter()
            .map(ApprovalListRow::into_summary)
            .collect::<Result<Vec<_>, _>>()?;
        for item in &mut items {
            let (agent_path, truncated) = self.load_approval_agent_path(item.id).await?;
            item.agent_path = agent_path;
            item.agent_path_truncated = truncated;
        }
        Ok(ApprovalPage {
            items,
            truncated,
            next_cursor,
        })
    }

    pub async fn latest_run_for_conversation(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Option<ProviderRun>, StoreError> {
        sqlx::query_as::<_, ProviderRunRow>(
            "SELECT id, conversation_id, provider, fallback_from_run_id, native_session_id, \
                    status, mutation_state, dispatch_certainty, created_at \
             FROM provider_runs WHERE conversation_id = ? \
             ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(conversation_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .map(ProviderRunRow::into_domain)
        .transpose()
    }

    pub async fn load_run_audits(
        &self,
        conversation_id: ConversationId,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<RunAuditPage, StoreError> {
        validate_page_limit(limit)?;
        let cursor = cursor.map(|value| decode_cursor(&value)).transpose()?;
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT provider_runs.id, provider_runs.provider, provider_runs.status, \
                    provider_runs.created_at, \
                    routing_decisions.reason AS routing_reason, \
                    COALESCE(length(CAST(routing_decisions.details_json AS BLOB)) > ",
        );
        query.push_bind(MAX_RUN_AUDIT_ROUTING_BYTES);
        query.push(
            ", FALSE) AS routing_truncated, \
                    provider_runs.handoff_rendered IS NOT NULL AS has_handoff \
             FROM provider_runs \
             LEFT JOIN routing_decisions ON routing_decisions.run_id = provider_runs.id \
             WHERE provider_runs.conversation_id = ",
        );
        query.push_bind(conversation_id.to_string());
        if let Some(cursor) = cursor {
            let created_at = cursor_sequence_i64(&cursor)?;
            query.push(" AND (provider_runs.created_at < ");
            query.push_bind(created_at);
            query.push(" OR (provider_runs.created_at = ");
            query.push_bind(created_at);
            query.push(" AND provider_runs.id < ");
            query.push_bind(cursor.id.to_string());
            query.push("))");
        }
        query.push(" ORDER BY provider_runs.created_at DESC, provider_runs.id DESC LIMIT ");
        query.push_bind(i64::from(limit) + 1);
        let rows = query
            .build_query_as::<RunAuditRow>()
            .fetch_all(&self.pool)
            .await?;
        let has_more = rows.len() > limit as usize;
        let rows = rows.into_iter().take(limit as usize).collect::<Vec<_>>();
        let next_cursor = if has_more {
            let last = rows.last().expect("a paged run audit has a last row");
            Some(encode_cursor(Cursor {
                sequence: u64::try_from(last.created_at).map_err(|_| StoreError::InvalidData {
                    entity: "provider run audit",
                    detail: "negative creation timestamp".to_owned(),
                })?,
                id: parse_uuid("provider run", &last.id)?,
            })?)
        } else {
            None
        };
        Ok(RunAuditPage {
            items: rows
                .into_iter()
                .map(RunAuditRow::into_summary)
                .collect::<Result<Vec<_>, _>>()?,
            next_cursor,
        })
    }

    pub async fn load_run_audit(
        &self,
        conversation_id: ConversationId,
        run_id: RunId,
    ) -> Result<RunAuditDetailRecord, StoreError> {
        sqlx::query_as::<_, RunAuditDetailRow>(
            "SELECT provider_runs.id, provider_runs.provider, provider_runs.status, \
                    CASE WHEN length(CAST(routing_decisions.details_json AS BLOB)) <= ? \
                         THEN routing_decisions.details_json END AS routing_json, \
                    routing_decisions.reason AS routing_reason, \
                    COALESCE(length(CAST(routing_decisions.details_json AS BLOB)), 0) > ? AS routing_truncated, \
                    CASE WHEN length(CAST(provider_runs.handoff_rendered AS BLOB)) <= ? \
                         THEN provider_runs.handoff_rendered \
                         ELSE substr(provider_runs.handoff_rendered, 1, ?) END AS handoff, \
                    COALESCE(length(CAST(provider_runs.handoff_rendered AS BLOB)), 0) > ? AS handoff_truncated \
             FROM provider_runs \
             LEFT JOIN routing_decisions ON routing_decisions.run_id = provider_runs.id \
             WHERE provider_runs.conversation_id = ? AND provider_runs.id = ?",
        )
        .bind(MAX_RUN_AUDIT_ROUTING_BYTES)
        .bind(MAX_RUN_AUDIT_ROUTING_BYTES)
        .bind(MAX_RUN_AUDIT_HANDOFF_BYTES)
        .bind(MAX_RUN_AUDIT_HANDOFF_BYTES)
        .bind(MAX_RUN_AUDIT_HANDOFF_BYTES)
        .bind(conversation_id.to_string())
        .bind(run_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            entity: "provider run audit",
            id: run_id.to_string(),
        })?
        .into_detail()
    }

    pub async fn load_timeline(
        &self,
        conversation_id: ConversationId,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<Page<TimelineEvent>, StoreError> {
        validate_page_limit(limit)?;
        let cursor = cursor.map(|value| decode_cursor(&value)).transpose()?;
        let rows = if let Some(cursor) = cursor {
            sqlx::query_as::<_, TimelineEventRow>(
                "SELECT id, conversation_id, run_id, agent_id, sequence, kind, role, content \
                 FROM events WHERE conversation_id = ? \
                 AND (sequence > ? OR (sequence = ? AND id > ?)) \
                 ORDER BY sequence, id LIMIT ?",
            )
            .bind(conversation_id.to_string())
            .bind(cursor_sequence_i64(&cursor)?)
            .bind(cursor_sequence_i64(&cursor)?)
            .bind(cursor.id.to_string())
            .bind(i64::from(limit) + 1)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, TimelineEventRow>(
                "SELECT id, conversation_id, run_id, agent_id, sequence, kind, role, content \
                 FROM events WHERE conversation_id = ? ORDER BY sequence, id LIMIT ?",
            )
            .bind(conversation_id.to_string())
            .bind(i64::from(limit) + 1)
            .fetch_all(&self.pool)
            .await?
        };

        let mut items = rows
            .into_iter()
            .map(TimelineEventRow::into_domain)
            .collect::<Result<Vec<_>, _>>()?;
        let has_more = items.len() > limit as usize;
        if has_more {
            items.truncate(limit as usize);
        }
        let next_cursor = if has_more {
            let last = items.last().expect("a page with more rows is non-empty");
            Some(encode_cursor(Cursor {
                sequence: last.sequence,
                id: last.id.as_uuid(),
            })?)
        } else {
            None
        };

        Ok(Page { items, next_cursor })
    }

    pub async fn load_event_payload(
        &self,
        event_id: TimelineEventId,
    ) -> Result<Option<serde_json::Value>, StoreError> {
        let payload: Option<String> =
            sqlx::query_scalar("SELECT payload_json FROM events WHERE id = ?")
                .bind(event_id.to_string())
                .fetch_optional(&self.pool)
                .await?
                .ok_or_else(|| StoreError::NotFound {
                    entity: "timeline event",
                    id: event_id.to_string(),
                })?;
        payload
            .map(|payload| {
                serde_json::from_str(&payload).map_err(|error| StoreError::InvalidData {
                    entity: "timeline event payload",
                    detail: error.to_string(),
                })
            })
            .transpose()
    }

    pub async fn load_agent_page(
        &self,
        conversation_id: ConversationId,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<AgentPage, StoreError> {
        validate_page_limit(limit)?;
        let cursor = cursor
            .map(|value| decode_agent_cursor(&value))
            .transpose()?;
        let run = if let Some(cursor) = &cursor {
            let run = self.load_run(cursor.run_id).await?;
            if run.conversation_id != conversation_id {
                return Err(StoreError::InvalidCursor);
            }
            Some(run)
        } else {
            self.latest_run_for_conversation(conversation_id).await?
        };
        let Some(run) = run else {
            return Ok(AgentPage {
                run_id: None,
                items: Vec::new(),
                next_cursor: None,
            });
        };
        let rows = if let Some(cursor) = cursor {
            sqlx::query_as::<_, AgentPageRow>(
                "SELECT id, run_id, parent_id, provider, NULL AS provider_native_id, \
                        NULL AS provider_native_path, substr(label, 1, 256) AS label, \
                        substr(summary, 1, 2048) AS summary, status, created_at, depth \
                 FROM agent_nodes WHERE run_id = ? AND \
                      (created_at > ? OR (created_at = ? AND id > ?)) \
                 ORDER BY created_at, id LIMIT ?",
            )
            .bind(run.id.to_string())
            .bind(agent_cursor_created_at_i64(&cursor)?)
            .bind(agent_cursor_created_at_i64(&cursor)?)
            .bind(cursor.id.to_string())
            .bind(i64::from(limit) + 1)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, AgentPageRow>(
                "SELECT id, run_id, parent_id, provider, NULL AS provider_native_id, \
                        NULL AS provider_native_path, substr(label, 1, 256) AS label, \
                        substr(summary, 1, 2048) AS summary, status, created_at, depth \
                 FROM agent_nodes WHERE run_id = ? ORDER BY created_at, id LIMIT ?",
            )
            .bind(run.id.to_string())
            .bind(i64::from(limit) + 1)
            .fetch_all(&self.pool)
            .await?
        };
        let has_more = rows.len() > limit as usize;
        let rows = rows.into_iter().take(limit as usize).collect::<Vec<_>>();
        let next_cursor = if has_more {
            let last = rows.last().expect("a truncated agent page is non-empty");
            Some(encode_agent_cursor(AgentCursor {
                created_at: u64::try_from(last.created_at).map_err(|_| {
                    StoreError::InvalidData {
                        entity: "agent node",
                        detail: "negative creation timestamp".to_owned(),
                    }
                })?,
                id: parse_uuid("agent node", &last.id)?,
                run_id: run.id,
            })?)
        } else {
            None
        };
        Ok(AgentPage {
            run_id: Some(run.id),
            items: rows
                .into_iter()
                .map(AgentPageRow::into_record)
                .collect::<Result<Vec<_>, _>>()?,
            next_cursor,
        })
    }

    /// Loads bounded display content only. Provider-native payloads stay inside the Store.
    pub async fn load_event_detail(
        &self,
        event_id: TimelineEventId,
    ) -> Result<EventDetail, StoreError> {
        let (content, content_bytes): (String, i64) = sqlx::query_as(
            "SELECT substr(content, 1, 262144), length(CAST(content AS BLOB)) \
             FROM events WHERE id = ?",
        )
        .bind(event_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StoreError::NotFound {
            entity: "timeline event",
            id: event_id.to_string(),
        })?;
        let content_bytes =
            usize::try_from(content_bytes).map_err(|_| StoreError::InvalidData {
                entity: "timeline event",
                detail: "negative content length".to_owned(),
            })?;
        let content = truncate_utf8(content, MAX_EVENT_DETAIL_BYTES);
        Ok(EventDetail {
            id: event_id,
            truncated: content_bytes > content.len(),
            content,
            content_bytes,
        })
    }

    pub async fn pending_recovery(&self) -> Result<Vec<RecoveryRun>, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let mut recovery = Vec::new();
        let mut run_cursor: Option<(String, i64, String)> = None;

        loop {
            let run_rows = if let Some((status, created_at, id)) = &run_cursor {
                sqlx::query_as::<_, ProviderRunRow>(
                    "SELECT id, conversation_id, provider, fallback_from_run_id, native_session_id, status, mutation_state, dispatch_certainty, created_at \
                     FROM provider_runs WHERE status IN ('queued', 'running', 'waiting') AND \
                     (status > ? OR (status = ? AND \
                       (created_at > ? OR (created_at = ? AND id > ?)))) \
                     ORDER BY status, created_at, id LIMIT ?",
                )
                .bind(status)
                .bind(status)
                .bind(created_at)
                .bind(created_at)
                .bind(id)
                .bind(RECOVERY_BATCH_SIZE)
                .fetch_all(&mut *transaction)
                .await?
            } else {
                sqlx::query_as::<_, ProviderRunRow>(
                    "SELECT id, conversation_id, provider, fallback_from_run_id, native_session_id, status, mutation_state, dispatch_certainty, created_at \
                     FROM provider_runs WHERE status IN ('queued', 'running', 'waiting') \
                     ORDER BY status, created_at, id LIMIT ?",
                )
                .bind(RECOVERY_BATCH_SIZE)
                .fetch_all(&mut *transaction)
                .await?
            };
            if run_rows.is_empty() {
                break;
            }
            let batch_complete = run_rows.len() < RECOVERY_BATCH_SIZE as usize;
            run_cursor = run_rows
                .last()
                .map(|row| (row.status.clone(), row.created_at, row.id.clone()));

            for row in run_rows {
                let run = row.into_domain()?;
                let (turn_prompt, handoff_rendered, handoff_hash): (
                    Option<String>,
                    Option<String>,
                    Option<String>,
                ) = sqlx::query_as(
                    "SELECT turn_prompt, handoff_rendered, handoff_hash \
                     FROM provider_runs WHERE id = ?",
                )
                .bind(run.id.to_string())
                .fetch_one(&mut *transaction)
                .await?;
                if handoff_rendered.is_some() != handoff_hash.is_some() {
                    return Err(StoreError::InvalidData {
                        entity: "provider run recovery intent",
                        detail: "rendered capsule and hash must both be present".to_owned(),
                    });
                }
                let attempt_intent = turn_prompt.map(|turn_prompt| RecoveryAttemptIntent {
                    turn_prompt,
                    handoff_rendered,
                    handoff_hash,
                });
                let mut agents = Vec::new();
                let mut agent_cursor: Option<(i64, String)> = None;
                loop {
                    let agent_rows = if let Some((created_at, id)) = &agent_cursor {
                        sqlx::query_as::<_, AgentNodeRow>(
                            "SELECT id, run_id, parent_id, provider, provider_native_id, provider_native_path, label, summary, status, created_at \
                             FROM agent_nodes WHERE run_id = ? AND \
                             (created_at > ? OR (created_at = ? AND id > ?)) \
                             ORDER BY created_at, id LIMIT ?",
                        )
                        .bind(run.id.to_string())
                        .bind(created_at)
                        .bind(created_at)
                        .bind(id)
                        .bind(RECOVERY_BATCH_SIZE)
                        .fetch_all(&mut *transaction)
                        .await?
                    } else {
                        sqlx::query_as::<_, AgentNodeRow>(
                            "SELECT id, run_id, parent_id, provider, provider_native_id, provider_native_path, label, summary, status, created_at \
                             FROM agent_nodes WHERE run_id = ? ORDER BY created_at, id LIMIT ?",
                        )
                        .bind(run.id.to_string())
                        .bind(RECOVERY_BATCH_SIZE)
                        .fetch_all(&mut *transaction)
                        .await?
                    };
                    if agent_rows.is_empty() {
                        break;
                    }
                    let agent_batch_complete = agent_rows.len() < RECOVERY_BATCH_SIZE as usize;
                    agent_cursor = agent_rows
                        .last()
                        .map(|row| (row.created_at, row.id.clone()));
                    agents.extend(
                        agent_rows
                            .into_iter()
                            .map(AgentNodeRow::into_domain)
                            .collect::<Result<Vec<_>, _>>()?,
                    );
                    if agent_batch_complete {
                        break;
                    }
                }

                let approvals = sqlx::query_as::<_, ApprovalRow>(
                    "SELECT id, run_id, agent_id, provider, provider_request_id, operation, scope, request_json, details_json, status, \
                     resolution_json, response_intent_json, response_intent_status \
                     FROM approvals WHERE run_id = ? AND status = 'pending' \
                     ORDER BY created_at, id",
                )
                .bind(run.id.to_string())
                .fetch_all(&mut *transaction)
                .await?
                .into_iter()
                .map(ApprovalRow::into_domain)
                .collect::<Result<Vec<_>, _>>()?;

                let (staged_count, staged_bytes): (i64, i64) = sqlx::query_as(
                    "SELECT COUNT(*), COALESCE(SUM(content_bytes), 0) FROM ( \
                         SELECT length(CAST(content AS BLOB)) + \
                                COALESCE(length(CAST(payload_json AS BLOB)), 0) AS content_bytes \
                         FROM staged_provider_events WHERE run_id = ? \
                         ORDER BY sequence, id LIMIT ? \
                     )",
                )
                .bind(run.id.to_string())
                .bind(i64::try_from(MAX_STAGED_EVENT_ROWS + 1).unwrap())
                .fetch_one(&mut *transaction)
                .await?;
                let staged_rows = sqlx::query_as::<_, StagedProviderEventRow>(
                    "WITH prefix AS ( \
                         SELECT id, conversation_id, run_id, agent_id, sequence, kind, content, \
                                native_item_id, payload_json, mutation_state, overflowed_kind \
                         FROM staged_provider_events WHERE run_id = ? \
                         ORDER BY sequence, id LIMIT ? \
                     ), bounded AS ( \
                         SELECT id, conversation_id, run_id, agent_id, sequence, kind, content, \
                                native_item_id, payload_json, mutation_state, overflowed_kind, \
                                SUM(length(CAST(content AS BLOB)) + \
                                    COALESCE(length(CAST(payload_json AS BLOB)), 0)) OVER ( \
                                    ORDER BY sequence, id \
                                ) AS cumulative_bytes \
                         FROM prefix \
                     ) \
                     SELECT id, conversation_id, run_id, agent_id, sequence, kind, content, \
                            native_item_id, payload_json, mutation_state, overflowed_kind \
                     FROM bounded WHERE cumulative_bytes <= ? ORDER BY sequence, id LIMIT ?",
                )
                .bind(run.id.to_string())
                .bind(i64::try_from(MAX_STAGED_EVENT_ROWS + 1).unwrap())
                .bind(i64::try_from(MAX_STAGED_EVENT_BYTES).unwrap())
                .bind(i64::try_from(MAX_STAGED_EVENT_ROWS).unwrap())
                .fetch_all(&mut *transaction)
                .await?;
                let staged_events_truncated = staged_count
                    > i64::try_from(MAX_STAGED_EVENT_ROWS).unwrap()
                    || staged_bytes > i64::try_from(MAX_STAGED_EVENT_BYTES).unwrap();
                let staged_events = staged_rows
                    .into_iter()
                    .map(StagedProviderEventRow::into_domain)
                    .collect::<Result<Vec<_>, _>>()?;
                let staged_events_overflowed = staged_events
                    .iter()
                    .any(|event| event.overflowed_kind.is_some());

                let event_rows = sqlx::query_as::<_, TimelineEventRow>(
                    "SELECT id, conversation_id, run_id, agent_id, sequence, kind, role, content \
                 FROM events WHERE run_id = ? ORDER BY sequence DESC, id DESC LIMIT ?",
                )
                .bind(run.id.to_string())
                .bind(RECOVERY_BATCH_SIZE + 1)
                .fetch_all(&mut *transaction)
                .await?;
                let events_truncated = event_rows.len() > RECOVERY_BATCH_SIZE as usize;
                let mut events = event_rows
                    .into_iter()
                    .take(RECOVERY_BATCH_SIZE as usize)
                    .map(TimelineEventRow::into_domain)
                    .collect::<Result<Vec<_>, _>>()?;
                events.reverse();
                recovery.push(RecoveryRun {
                    run,
                    attempt_intent,
                    agents,
                    approvals,
                    staged_events,
                    staged_events_overflowed,
                    staged_events_truncated,
                    events,
                    events_truncated,
                });
            }

            if batch_complete {
                break;
            }
        }

        transaction.commit().await?;
        Ok(recovery)
    }

    /// Atomically records that this process owns the next provider dispatch for a queued run.
    /// A false result means another process claimed it or the run is no longer recoverable.
    pub(crate) async fn claim_provider_dispatch(
        &self,
        run_id: RunId,
        owner_id: &str,
        lease_duration: Duration,
    ) -> Result<bool, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let expires_at = lease_expires_at(lease_duration);
        let claimed = sqlx::query(
            "UPDATE provider_runs \
             SET dispatch_certainty = 'may_have_dispatched', dispatch_owner_id = ?, \
                 dispatch_lease_expires_at = ?, updated_at = ? \
             WHERE id = ? AND status = 'queued' AND mutation_state = 'none_observed' \
             AND dispatch_owner_id IS NULL \
             AND (dispatch_certainty IS NULL OR dispatch_certainty = 'not_dispatched')",
        )
        .bind(owner_id)
        .bind(expires_at)
        .bind(now_millis())
        .bind(run_id.to_string())
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            == 1;
        transaction.commit().await?;
        Ok(claimed)
    }

    pub(crate) async fn refresh_provider_dispatch_lease(
        &self,
        run_id: RunId,
        owner_id: &str,
        lease_duration: Duration,
        stale_grace: Duration,
    ) -> Result<bool, StoreError> {
        let now = now_millis();
        let refreshed = sqlx::query(
            "UPDATE provider_runs SET dispatch_lease_expires_at = ?, updated_at = ? \
             WHERE id = ? AND dispatch_owner_id = ? \
             AND dispatch_lease_expires_at >= ? \
             AND status IN ('queued', 'running', 'waiting')",
        )
        .bind(lease_expires_at(lease_duration))
        .bind(now)
        .bind(run_id.to_string())
        .bind(owner_id)
        .bind(now.saturating_sub(duration_millis(stale_grace)))
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1;
        Ok(refreshed)
    }

    /// Atomically transfers an abandoned dispatch lease to a recovery owner.
    /// A successful transfer fences the prior owner before recovery interrupts the run.
    pub(crate) async fn claim_stale_provider_dispatch(
        &self,
        run_id: RunId,
        recovery_owner_id: &str,
        excluded_owner_id: &str,
        lease_duration: Duration,
        stale_grace: Duration,
    ) -> Result<bool, StoreError> {
        let now = now_millis();
        let claimed = sqlx::query(
            "UPDATE provider_runs \
             SET dispatch_owner_id = ?, dispatch_lease_expires_at = ?, updated_at = ? \
             WHERE id = ? AND dispatch_owner_id IS NOT NULL \
             AND dispatch_owner_id != ? AND dispatch_lease_expires_at < ? \
             AND status IN ('queued', 'running', 'waiting')",
        )
        .bind(recovery_owner_id)
        .bind(lease_expires_at(lease_duration))
        .bind(now)
        .bind(run_id.to_string())
        .bind(excluded_owner_id)
        .bind(now.saturating_sub(duration_millis(stale_grace)))
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1;
        Ok(claimed)
    }

    pub(crate) async fn claim_unowned_provider_dispatch_recovery(
        &self,
        run_id: RunId,
        recovery_owner_id: &str,
        lease_duration: Duration,
    ) -> Result<bool, StoreError> {
        let claimed = sqlx::query(
            "UPDATE provider_runs \
             SET dispatch_owner_id = ?, dispatch_lease_expires_at = ?, updated_at = ? \
             WHERE id = ? AND dispatch_owner_id IS NULL \
             AND status IN ('queued', 'running', 'waiting')",
        )
        .bind(recovery_owner_id)
        .bind(lease_expires_at(lease_duration))
        .bind(now_millis())
        .bind(run_id.to_string())
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1;
        Ok(claimed)
    }

    pub(crate) async fn promote_recovery_dispatch_claim(
        &self,
        run_id: RunId,
        recovery_owner_id: &str,
        supervisor_owner_id: &str,
        lease_duration: Duration,
    ) -> Result<bool, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let promoted = sqlx::query(
            "UPDATE provider_runs \
             SET dispatch_certainty = 'may_have_dispatched', dispatch_owner_id = ?, \
                 dispatch_lease_expires_at = ?, updated_at = ? \
             WHERE id = ? AND dispatch_owner_id = ? \
             AND status = 'queued' AND mutation_state = 'none_observed' \
             AND (dispatch_certainty IS NULL OR dispatch_certainty = 'not_dispatched') \
             AND native_session_id IS NULL",
        )
        .bind(supervisor_owner_id)
        .bind(lease_expires_at(lease_duration))
        .bind(now_millis())
        .bind(run_id.to_string())
        .bind(recovery_owner_id)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            == 1;
        transaction.commit().await?;
        Ok(promoted)
    }

    #[cfg(test)]
    pub(crate) async fn has_protected_provider_dispatch_lease(
        &self,
        run_id: RunId,
        stale_grace: Duration,
    ) -> Result<bool, StoreError> {
        let lease = sqlx::query_as::<_, (Option<String>, Option<i64>)>(
            "SELECT dispatch_owner_id, dispatch_lease_expires_at \
             FROM provider_runs WHERE id = ?",
        )
        .bind(run_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound {
            entity: "provider run",
            id: run_id.to_string(),
        })?;
        Ok(dispatch_lease_is_protected(
            lease.0.as_deref(),
            lease.1,
            now_millis(),
            stale_grace,
        ))
    }

    pub(crate) async fn load_ambiguous_recovery_run_ids(
        &self,
        after_run_id: Option<RunId>,
        limit: u32,
    ) -> Result<Vec<RunId>, StoreError> {
        validate_page_limit(limit)?;
        let rows = match after_run_id {
            Some(after_run_id) => {
                sqlx::query_scalar::<_, String>(
                    "SELECT id FROM provider_runs \
                     WHERE status IN ('running', 'waiting') AND id > ? \
                     ORDER BY id LIMIT ?",
                )
                .bind(after_run_id.to_string())
                .bind(i64::from(limit))
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_scalar::<_, String>(
                    "SELECT id FROM provider_runs WHERE status IN ('running', 'waiting') \
                     ORDER BY id LIMIT ?",
                )
                .bind(i64::from(limit))
                .fetch_all(&self.pool)
                .await?
            }
        };
        rows.into_iter()
            .map(|id| parse_uuid("provider run", &id).map(Into::into))
            .collect()
    }

    pub(crate) async fn load_queued_recovery_batch(
        &self,
        after_run_id: Option<RunId>,
        limit: u32,
    ) -> Result<Vec<QueuedRecovery>, StoreError> {
        validate_page_limit(limit)?;
        let rows = match after_run_id {
            Some(after_run_id) => {
                sqlx::query_as::<_, ProviderRunRow>(
                    "SELECT id, conversation_id, provider, fallback_from_run_id, native_session_id, status, mutation_state, dispatch_certainty, created_at \
                     FROM provider_runs WHERE status = 'queued' AND id > ? ORDER BY id LIMIT ?",
                )
                .bind(after_run_id.to_string())
                .bind(i64::from(limit))
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as::<_, ProviderRunRow>(
                    "SELECT id, conversation_id, provider, fallback_from_run_id, native_session_id, status, mutation_state, dispatch_certainty, created_at \
                     FROM provider_runs WHERE status = 'queued' ORDER BY id LIMIT ?",
                )
                .bind(i64::from(limit))
                .fetch_all(&self.pool)
                .await?
            }
        };
        let mut recovery = Vec::with_capacity(rows.len());
        for row in rows {
            let run = row.into_domain()?;
            let (turn_prompt, handoff_rendered, handoff_hash): (
                Option<String>,
                Option<String>,
                Option<String>,
            ) = sqlx::query_as(
                "SELECT turn_prompt, handoff_rendered, handoff_hash FROM provider_runs WHERE id = ?",
            )
            .bind(run.id.to_string())
            .fetch_one(&self.pool)
            .await?;
            if handoff_rendered.is_some() != handoff_hash.is_some() {
                return Err(StoreError::InvalidData {
                    entity: "provider run recovery intent",
                    detail: "rendered capsule and hash must both be present".to_owned(),
                });
            }
            let attempt_intent = turn_prompt.map(|turn_prompt| RecoveryAttemptIntent {
                turn_prompt,
                handoff_rendered,
                handoff_hash,
            });
            let roots = sqlx::query_as::<_, AgentNodeRow>(
                "SELECT id, run_id, parent_id, provider, provider_native_id, provider_native_path, label, summary, status, created_at \
                 FROM agent_nodes WHERE run_id = ? AND parent_id IS NULL ORDER BY id LIMIT 2",
            )
            .bind(run.id.to_string())
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(AgentNodeRow::into_domain)
            .collect::<Result<Vec<_>, _>>()?;
            recovery.push(QueuedRecovery {
                run,
                attempt_intent,
                roots,
            });
        }
        Ok(recovery)
    }

    /// Returns the deepest active agents first. Persisted depth plus the recovery index makes
    /// repeated batches linear while preserving child-before-parent interruption semantics.
    pub async fn load_recovery_agent_batch(
        &self,
        limit: u32,
    ) -> Result<Vec<RecoveryAgent>, StoreError> {
        validate_page_limit(limit)?;
        let rows = sqlx::query_as::<_, RecoveryAgentRow>(
            "SELECT agent_nodes.run_id, agent_nodes.id AS agent_id, \
                    provider_runs.mutation_state, agent_nodes.parent_id IS NULL AS is_root, \
                    agent_nodes.depth \
             FROM agent_nodes INDEXED BY idx_agents_recovery_depth \
             CROSS JOIN provider_runs ON provider_runs.id = agent_nodes.run_id \
             WHERE agent_nodes.status IN ('queued', 'running', 'waiting') \
               AND provider_runs.status IN ('queued', 'running', 'waiting') \
             ORDER BY agent_nodes.depth DESC, agent_nodes.created_at, agent_nodes.id LIMIT ?",
        )
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(RecoveryAgentRow::into_domain)
            .collect()
    }

    pub(crate) async fn load_recovery_agent_batch_for_run(
        &self,
        run_id: RunId,
        limit: u32,
    ) -> Result<Vec<RecoveryAgent>, StoreError> {
        validate_page_limit(limit)?;
        let rows = sqlx::query_as::<_, RecoveryAgentRow>(
            "SELECT agent_nodes.run_id, agent_nodes.id AS agent_id, \
                    provider_runs.mutation_state, agent_nodes.parent_id IS NULL AS is_root, \
                    agent_nodes.depth \
             FROM agent_nodes INDEXED BY idx_agents_recovery_depth \
             CROSS JOIN provider_runs ON provider_runs.id = agent_nodes.run_id \
             WHERE agent_nodes.run_id = ? \
               AND agent_nodes.status IN ('queued', 'running', 'waiting') \
               AND provider_runs.status IN ('queued', 'running', 'waiting') \
             ORDER BY agent_nodes.depth DESC, agent_nodes.created_at, agent_nodes.id LIMIT ?",
        )
        .bind(run_id.to_string())
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(RecoveryAgentRow::into_domain)
            .collect()
    }
}

async fn materialize_child_agents(
    transaction: &mut Transaction<'_, Sqlite>,
    run: &ProviderRun,
    anchor: &AgentNode,
    parent_native_id: &str,
    child_native_ids: &[String],
    child_statuses: &[NativeChildStatus],
    now: i64,
) -> Result<AgentId, StoreError> {
    validate_native_agent_id(parent_native_id)?;
    if anchor.parent_id.is_some() || anchor.provider != run.provider {
        return Err(StoreError::NativeAgentIdentityConflict);
    }
    if child_native_ids.len() > MAX_CHILDREN_PER_EVENT
        || child_statuses.len() > MAX_CHILDREN_PER_EVENT
    {
        return Err(StoreError::InvalidData {
            entity: "child agent event",
            detail: "child count exceeds the durable bound".to_owned(),
        });
    }
    let declared = child_native_ids.iter().collect::<HashSet<_>>();
    let reported = child_statuses
        .iter()
        .map(|child| &child.native_thread_id)
        .collect::<HashSet<_>>();
    if declared.len() != child_native_ids.len()
        || reported.len() != child_statuses.len()
        || !reported.is_subset(&declared)
        || declared.iter().any(|id| id.as_str() == parent_native_id)
    {
        return Err(StoreError::NativeAgentIdentityConflict);
    }
    for child_native_id in child_native_ids {
        validate_native_agent_id(child_native_id)?;
    }

    let parent = load_agent_by_native_id(transaction, run.id, parent_native_id)
        .await?
        .ok_or(StoreError::NativeAgentIdentityConflict)?;

    for child_native_id in child_native_ids {
        let reported_status = child_statuses
            .iter()
            .find(|child| child.native_thread_id == *child_native_id)
            .map(|child| native_agent_status(&child.status));
        if let Some(existing) =
            load_agent_by_native_id(transaction, run.id, child_native_id).await?
        {
            if existing.parent_id != Some(parent.id) || existing.provider != run.provider {
                return Err(StoreError::NativeAgentIdentityConflict);
            }
            if let Some(next) = reported_status {
                validate_native_agent_update(existing.status, next)?;
                sqlx::query("UPDATE agent_nodes SET status = ?, updated_at = ? WHERE id = ?")
                    .bind(agent_status_label(next))
                    .bind(now)
                    .bind(existing.id.to_string())
                    .execute(&mut **transaction)
                    .await?;
            }
        } else {
            let child_id = AgentId::new();
            let parent_depth: i64 =
                sqlx::query_scalar("SELECT depth FROM agent_nodes WHERE id = ?")
                    .bind(parent.id.to_string())
                    .fetch_one(&mut **transaction)
                    .await?;
            let sibling_ordinal: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) + 1 FROM agent_nodes WHERE run_id = ? AND parent_id = ?",
            )
            .bind(run.id.to_string())
            .bind(parent.id.to_string())
            .fetch_one(&mut **transaction)
            .await?;
            let label = if parent.parent_id.is_none() {
                format!("Agent {sibling_ordinal}")
            } else {
                format!("{}.{}", parent.label, sibling_ordinal)
            };
            sqlx::query(
                "INSERT INTO agent_nodes \
                 (id, run_id, parent_id, provider, provider_native_id, label, status, depth, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(child_id.to_string())
            .bind(run.id.to_string())
            .bind(parent.id.to_string())
            .bind(provider_label(run.provider))
            .bind(child_native_id)
            .bind(label)
            .bind(agent_status_label(
                reported_status.unwrap_or(AgentStatus::Queued),
            ))
            .bind(parent_depth + 1)
            .bind(now)
            .bind(now)
            .execute(&mut **transaction)
            .await?;
        }
    }
    Ok(parent.id)
}

async fn update_sub_agent(
    transaction: &mut Transaction<'_, Sqlite>,
    run: &ProviderRun,
    anchor: &AgentNode,
    native_id: &str,
    native_path: &str,
    activity: NativeSubAgentActivityKind,
    now: i64,
) -> Result<AgentId, StoreError> {
    validate_native_agent_id(native_id)?;
    if native_path.is_empty()
        || native_path.len() > MAX_NATIVE_AGENT_PATH_BYTES
        || anchor.parent_id.is_some()
    {
        return Err(StoreError::NativeAgentIdentityConflict);
    }
    let existing = load_agent_by_native_id(transaction, run.id, native_id)
        .await?
        .ok_or(StoreError::NativeAgentIdentityConflict)?;
    if existing.provider != run.provider
        || existing
            .provider_native_path
            .as_deref()
            .is_some_and(|path| path != native_path)
    {
        return Err(StoreError::NativeAgentIdentityConflict);
    }
    let next = match activity {
        NativeSubAgentActivityKind::Started | NativeSubAgentActivityKind::Interacted => {
            AgentStatus::Running
        }
        NativeSubAgentActivityKind::Interrupted => AgentStatus::Interrupted,
    };
    validate_native_agent_update(existing.status, next)?;
    sqlx::query(
        "UPDATE agent_nodes SET provider_native_path = coalesce(provider_native_path, ?), \
         status = ?, updated_at = ? WHERE id = ?",
    )
    .bind(native_path)
    .bind(agent_status_label(next))
    .bind(now)
    .bind(existing.id.to_string())
    .execute(&mut **transaction)
    .await?;
    Ok(existing.id)
}

async fn load_agent_by_native_id(
    transaction: &mut Transaction<'_, Sqlite>,
    run_id: RunId,
    native_id: &str,
) -> Result<Option<AgentNode>, StoreError> {
    sqlx::query_as::<_, AgentNodeRow>(
        "SELECT id, run_id, parent_id, provider, provider_native_id, provider_native_path, \
                label, summary, status, created_at \
         FROM agent_nodes WHERE run_id = ? AND provider_native_id = ?",
    )
    .bind(run_id.to_string())
    .bind(native_id)
    .fetch_optional(&mut **transaction)
    .await?
    .map(AgentNodeRow::into_domain)
    .transpose()
}

fn validate_native_agent_id(native_id: &str) -> Result<(), StoreError> {
    if native_id.is_empty() || native_id.len() > MAX_NATIVE_AGENT_ID_BYTES {
        return Err(StoreError::InvalidData {
            entity: "provider-native agent identity",
            detail: "identity is empty or exceeds the durable bound".to_owned(),
        });
    }
    Ok(())
}

fn native_agent_status(status: &NativeAgentStatus) -> AgentStatus {
    match status {
        NativeAgentStatus::PendingInit => AgentStatus::Queued,
        NativeAgentStatus::Running => AgentStatus::Running,
        NativeAgentStatus::Interrupted | NativeAgentStatus::Shutdown => AgentStatus::Interrupted,
        NativeAgentStatus::Completed => AgentStatus::Completed,
        NativeAgentStatus::Errored
        | NativeAgentStatus::NotFound
        | NativeAgentStatus::Unrecognized(_) => AgentStatus::Failed,
    }
}

fn validate_native_agent_update(from: AgentStatus, to: AgentStatus) -> Result<(), StoreError> {
    let valid = from == to
        || matches!(
            (from, to),
            (
                AgentStatus::Queued,
                AgentStatus::Running
                    | AgentStatus::Completed
                    | AgentStatus::Interrupted
                    | AgentStatus::Failed
            ) | (
                AgentStatus::Running,
                AgentStatus::Waiting
                    | AgentStatus::Completed
                    | AgentStatus::Interrupted
                    | AgentStatus::Failed
            ) | (
                AgentStatus::Waiting,
                AgentStatus::Running
                    | AgentStatus::Completed
                    | AgentStatus::Interrupted
                    | AgentStatus::Failed
            )
        );
    if valid {
        Ok(())
    } else {
        Err(StoreError::NativeAgentIdentityConflict)
    }
}

async fn insert_atomic_fallback(
    transaction: &mut Transaction<'_, Sqlite>,
    primary: &ProviderRun,
    fallback: NewFallbackAttempt,
    dispatch_claim: Option<(&str, Duration)>,
    now: i64,
) -> Result<(ProviderRun, AgentNode), StoreError> {
    if primary.fallback_from_run_id.is_some()
        || primary.status != RunStatus::Failed
        || primary.mutation_state != MutationState::NoneObserved
        || primary.dispatch_certainty != Some(DispatchCertainty::NotDispatched)
    {
        return Err(StoreError::UnsafeFallbackState);
    }
    if primary.provider == fallback.provider {
        return Err(StoreError::SameFallbackProvider);
    }
    if fallback
        .routing_decision
        .as_ref()
        .is_some_and(|decision| decision.provider != fallback.provider)
        || fallback.handoff_rendered.is_some() != fallback.handoff_hash.is_some()
    {
        return Err(StoreError::UnsafeFallbackState);
    }
    let fallback_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM provider_runs WHERE fallback_from_run_id = ?)",
    )
    .bind(primary.id.to_string())
    .fetch_one(&mut **transaction)
    .await?;
    if fallback_exists {
        return Err(StoreError::FallbackAlreadyExists);
    }
    let (conversation_status, application_managed, context_through_sequence): (
        String,
        bool,
        Option<i64>,
    ) = sqlx::query_as(
        "SELECT conversations.status, provider_runs.application_managed, \
                provider_runs.context_through_sequence \
         FROM provider_runs \
         JOIN conversations ON conversations.id = provider_runs.conversation_id \
         WHERE provider_runs.id = ?",
    )
    .bind(primary.id.to_string())
    .fetch_one(&mut **transaction)
    .await?;
    if conversation_status == "archived" {
        return Err(StoreError::ConversationArchived(primary.conversation_id));
    }
    let provider_session_id = match fallback.native_session_id.as_deref() {
        Some(native_session_id) => Some(
            sqlx::query_scalar::<_, String>(
                "SELECT id FROM provider_sessions WHERE conversation_id = ? \
                 AND provider = ? AND native_session_id = ?",
            )
            .bind(primary.conversation_id.to_string())
            .bind(provider_label(fallback.provider))
            .bind(native_session_id)
            .fetch_optional(&mut **transaction)
            .await?
            .ok_or_else(|| StoreError::InvalidData {
                entity: "fallback native session",
                detail: "does not belong to the fallback provider and conversation".to_owned(),
            })?,
        ),
        None => None,
    };
    let routing_decision = fallback
        .routing_decision
        .as_deref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(invalid_json("routing decision"))?;
    let routing_fields = fallback
        .routing_decision
        .as_ref()
        .map(|decision| (decision.reason, decision.task_kind));
    let mut run = ProviderRun::new(primary.conversation_id, fallback.provider);
    run.fallback_from_run_id = Some(primary.id);
    run.native_session_id = fallback.native_session_id.clone();
    let root = AgentNode::root(run.id, fallback.provider, "orchestrator");
    let (dispatch_certainty, dispatch_owner_id, dispatch_lease_expires_at) = dispatch_claim
        .map(|(owner_id, lease_duration)| {
            (
                Some("may_have_dispatched"),
                Some(owner_id),
                Some(lease_expires_at(lease_duration)),
            )
        })
        .unwrap_or((None, None, None));
    sqlx::query(
        "INSERT INTO provider_runs \
         (id, conversation_id, provider_session_id, provider, fallback_from_run_id, \
          native_session_id, status, mutation_state, handoff_rendered, handoff_hash, \
          context_through_sequence, application_managed, turn_prompt, dispatch_certainty, \
          dispatch_owner_id, dispatch_lease_expires_at, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, 'queued', 'none_observed', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(run.id.to_string())
    .bind(run.conversation_id.to_string())
    .bind(provider_session_id)
    .bind(provider_label(run.provider))
    .bind(primary.id.to_string())
    .bind(&fallback.native_session_id)
    .bind(&fallback.handoff_rendered)
    .bind(&fallback.handoff_hash)
    .bind(context_through_sequence)
    .bind(application_managed)
    .bind(&fallback.turn_prompt)
    .bind(dispatch_certainty)
    .bind(dispatch_owner_id)
    .bind(dispatch_lease_expires_at)
    .bind(now)
    .bind(now)
    .execute(&mut **transaction)
    .await?;
    if let Some(routing_decision) = routing_decision {
        let (reason, task_kind) = routing_fields.expect("serialized decision has typed fields");
        sqlx::query(
            "INSERT INTO routing_decisions \
             (id, run_id, chosen_provider, details_json, reason, task_kind, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::now_v7().to_string())
        .bind(run.id.to_string())
        .bind(provider_label(run.provider))
        .bind(routing_decision)
        .bind(routing_reason_label(reason))
        .bind(task_kind_label(task_kind))
        .bind(now)
        .execute(&mut **transaction)
        .await?;
    }
    sqlx::query(
        "INSERT INTO agent_nodes \
         (id, run_id, parent_id, provider, label, status, created_at, updated_at) \
         VALUES (?, ?, NULL, ?, ?, 'queued', ?, ?)",
    )
    .bind(root.id.to_string())
    .bind(run.id.to_string())
    .bind(provider_label(run.provider))
    .bind(&root.label)
    .bind(now)
    .bind(now)
    .execute(&mut **transaction)
    .await?;
    Ok((run, root))
}

async fn require_dispatch_owner(
    transaction: &mut Transaction<'_, Sqlite>,
    run_id: RunId,
    expected_owner_id: &str,
) -> Result<(), StoreError> {
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM provider_runs \
         WHERE id = ? AND dispatch_owner_id = ?)",
    )
    .bind(run_id.to_string())
    .bind(expected_owner_id)
    .fetch_one(&mut **transaction)
    .await?;
    if owned {
        Ok(())
    } else {
        Err(StoreError::DispatchOwnerMismatch(run_id))
    }
}

async fn load_existing_fallback(
    transaction: &mut Transaction<'_, Sqlite>,
    primary: &ProviderRun,
    expected: &NewFallbackAttempt,
    expected_owner_id: Option<&str>,
) -> Result<Option<(ProviderRun, AgentNode)>, StoreError> {
    type ExistingFallbackRow = (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let row: Option<ExistingFallbackRow> = sqlx::query_as(
        "SELECT id, provider, native_session_id, handoff_hash, turn_prompt, dispatch_owner_id \
             FROM provider_runs WHERE fallback_from_run_id = ?",
    )
    .bind(primary.id.to_string())
    .fetch_optional(&mut **transaction)
    .await?;
    let Some((id, provider, native_session_id, handoff_hash, turn_prompt, owner_id)) = row else {
        return Ok(None);
    };
    let fallback_run_id = parse_uuid("provider run", &id)?.into();
    if expected_owner_id
        .is_some_and(|expected_owner_id| owner_id.as_deref() != Some(expected_owner_id))
    {
        return Err(StoreError::DispatchOwnerMismatch(fallback_run_id));
    }
    if parse_provider(&provider)? != expected.provider
        || native_session_id != expected.native_session_id
        || handoff_hash != expected.handoff_hash
        || turn_prompt.as_deref() != Some(expected.turn_prompt.as_str())
    {
        return Err(StoreError::FallbackIntentConflict);
    }
    let run = sqlx::query_as::<_, ProviderRunRow>(
        "SELECT id, conversation_id, provider, fallback_from_run_id, native_session_id, status, mutation_state, dispatch_certainty, created_at \
         FROM provider_runs WHERE id = ?",
    )
    .bind(&id)
    .fetch_one(&mut **transaction)
    .await?
    .into_domain()?;
    let root = sqlx::query_as::<_, AgentNodeRow>(
        "SELECT id, run_id, parent_id, provider, provider_native_id, provider_native_path, \
                label, summary, status, created_at \
         FROM agent_nodes WHERE run_id = ? AND parent_id IS NULL",
    )
    .bind(id)
    .fetch_one(&mut **transaction)
    .await?
    .into_domain()?;
    Ok(Some((run, root)))
}

async fn persist_approval_questions(
    transaction: &mut Transaction<'_, Sqlite>,
    approval_id: ApprovalId,
    questions: &[UserInputQuestion],
) -> Result<(), StoreError> {
    for (ordinal, question) in questions.iter().enumerate() {
        let source_bytes = serde_json::to_vec(question)
            .map_err(invalid_data("approval question"))?
            .len();
        let options_json = question
            .options
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(invalid_data("approval options"))?
            .filter(|options| options.len() <= MAX_APPROVAL_QUESTION_SOURCE_BYTES as usize);
        let header_bytes =
            i64::try_from(question.header.len()).map_err(|_| StoreError::InvalidData {
                entity: "approval question",
                detail: "header size exceeds the supported range".to_owned(),
            })?;
        let question_bytes =
            i64::try_from(question.question.len()).map_err(|_| StoreError::InvalidData {
                entity: "approval question",
                detail: "question size exceeds the supported range".to_owned(),
            })?;
        sqlx::query(
            "INSERT INTO approval_questions \
             (approval_id, ordinal, header, question, options_json, is_other, is_secret, \
              source_bytes, header_bytes, question_bytes) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(approval_id.to_string())
        .bind(i64::try_from(ordinal).map_err(|_| StoreError::InvalidData {
            entity: "approval question",
            detail: "ordinal exceeds the supported range".to_owned(),
        })?)
        .bind(truncate_utf8(
            question.header.clone(),
            MAX_APPROVAL_QUESTION_HEADER_BYTES,
        ))
        .bind(truncate_utf8(
            question.question.clone(),
            MAX_APPROVAL_QUESTION_TEXT_BYTES,
        ))
        .bind(options_json)
        .bind(question.is_other)
        .bind(question.is_secret)
        .bind(
            i64::try_from(source_bytes).map_err(|_| StoreError::InvalidData {
                entity: "approval question",
                detail: "source size exceeds the supported range".to_owned(),
            })?,
        )
        .bind(header_bytes)
        .bind(question_bytes)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

async fn persist_assistant_message_in_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    run_id: RunId,
    conversation_id: ConversationId,
    provider: ProviderId,
    native_item_id: Option<&str>,
    content: &str,
    now: i64,
) -> Result<(), StoreError> {
    validate_message_size(content)?;
    let sequence = if let Some(native_item_id) = native_item_id {
        let existing: Option<(i64, i64)> = sqlx::query_as(
            "SELECT sequence, length(CAST(content AS BLOB)) FROM messages \
             WHERE run_id = ? AND role = 'assistant' \
             AND native_item_id = ?",
        )
        .bind(run_id.to_string())
        .bind(native_item_id)
        .fetch_optional(&mut **transaction)
        .await?;
        if let Some((sequence, existing_bytes)) = existing {
            if usize::try_from(existing_bytes)
                .unwrap_or(usize::MAX)
                .saturating_add(content.len())
                > MAX_CANONICAL_MESSAGE_BYTES
            {
                return Err(StoreError::MessageTooLarge {
                    limit: MAX_CANONICAL_MESSAGE_BYTES,
                });
            }
            sqlx::query("UPDATE messages SET content = content || ? WHERE sequence = ?")
                .bind(content)
                .bind(sequence)
                .execute(&mut **transaction)
                .await?;
            sequence
        } else {
            sqlx::query(
                "INSERT INTO messages \
                 (id, conversation_id, run_id, role, content, native_item_id, created_at) \
                 VALUES (?, ?, ?, 'assistant', ?, ?, ?)",
            )
            .bind(MessageId::new().to_string())
            .bind(conversation_id.to_string())
            .bind(run_id.to_string())
            .bind(content)
            .bind(native_item_id)
            .bind(now)
            .execute(&mut **transaction)
            .await?
            .last_insert_rowid()
        }
    } else {
        sqlx::query(
            "INSERT INTO messages \
             (id, conversation_id, run_id, role, content, native_item_id, created_at) \
             VALUES (?, ?, ?, 'assistant', ?, NULL, ?)",
        )
        .bind(MessageId::new().to_string())
        .bind(conversation_id.to_string())
        .bind(run_id.to_string())
        .bind(content)
        .bind(now)
        .execute(&mut **transaction)
        .await?
        .last_insert_rowid()
    };
    sqlx::query(
        "UPDATE provider_sessions SET context_through_sequence = \
         max(context_through_sequence, ?), updated_at = ? \
         WHERE conversation_id = ? AND provider = ?",
    )
    .bind(sequence)
    .bind(now)
    .bind(conversation_id.to_string())
    .bind(provider_label(provider))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn validate_message_size(content: &str) -> Result<(), StoreError> {
    if content.len() > MAX_CANONICAL_MESSAGE_BYTES {
        Err(StoreError::MessageTooLarge {
            limit: MAX_CANONICAL_MESSAGE_BYTES,
        })
    } else {
        Ok(())
    }
}

pub(crate) fn validate_conversation_settings(
    settings: &ConversationSettings,
) -> Result<(), StoreError> {
    let aggregate_constraint_bytes =
        settings
            .constraints
            .iter()
            .try_fold(0_usize, |total, constraint| {
                if constraint.len() > MAX_CONSTRAINT_BYTES {
                    return Err(StoreError::InvalidData {
                        entity: "conversation settings",
                        detail: "a constraint exceeds the durable byte bound".to_owned(),
                    });
                }
                total
                    .checked_add(constraint.len())
                    .ok_or_else(|| StoreError::InvalidData {
                        entity: "conversation settings",
                        detail: "constraint bytes overflow the durable bound".to_owned(),
                    })
            })?;
    if settings.objective.len() > MAX_OBJECTIVE_BYTES
        || settings.constraints.len() > MAX_CONSTRAINTS
        || aggregate_constraint_bytes > MAX_CONSTRAINT_BYTES_TOTAL
    {
        return Err(StoreError::InvalidData {
            entity: "conversation settings",
            detail: "required handoff context exceeds the durable bound".to_owned(),
        });
    }
    Ok(())
}

async fn drain_staged_events_in_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    run_id: RunId,
    agent_id: Option<AgentId>,
) -> Result<Vec<TimelineEvent>, StoreError> {
    let (conversation_id, provider, root_id): (String, String, String) = sqlx::query_as(
        "SELECT provider_runs.conversation_id, provider_runs.provider, agent_nodes.id \
         FROM provider_runs JOIN agent_nodes ON agent_nodes.run_id = provider_runs.id \
         WHERE provider_runs.id = ? AND agent_nodes.parent_id IS NULL",
    )
    .bind(run_id.to_string())
    .fetch_one(&mut **transaction)
    .await?;
    let conversation_id: ConversationId = parse_uuid("conversation", &conversation_id)?.into();
    let provider = parse_provider(&provider)?;
    let root_id: AgentId = parse_uuid("agent node", &root_id)?.into();
    let (staged_count, staged_bytes): (i64, i64) = if let Some(agent_id) = agent_id {
        sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(content_bytes), 0) FROM ( \
                 SELECT length(CAST(content AS BLOB)) + \
                        COALESCE(length(CAST(payload_json AS BLOB)), 0) AS content_bytes \
                 FROM staged_provider_events WHERE run_id = ? AND agent_id = ? \
                 ORDER BY sequence, id LIMIT ? \
             )",
        )
        .bind(run_id.to_string())
        .bind(agent_id.to_string())
        .bind(i64::try_from(MAX_STAGED_EVENT_ROWS + 1).unwrap())
        .fetch_one(&mut **transaction)
        .await?
    } else {
        sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(content_bytes), 0) FROM ( \
                 SELECT length(CAST(content AS BLOB)) + \
                        COALESCE(length(CAST(payload_json AS BLOB)), 0) AS content_bytes \
                 FROM staged_provider_events WHERE run_id = ? \
                 ORDER BY sequence, id LIMIT ? \
             )",
        )
        .bind(run_id.to_string())
        .bind(i64::try_from(MAX_STAGED_EVENT_ROWS + 1).unwrap())
        .fetch_one(&mut **transaction)
        .await?
    };
    if staged_count > i64::try_from(MAX_STAGED_EVENT_ROWS).unwrap()
        || staged_bytes > i64::try_from(MAX_STAGED_EVENT_BYTES).unwrap()
    {
        return Err(StoreError::CorruptStagedEventQueue);
    }
    let rows = if let Some(agent_id) = agent_id {
        sqlx::query_as::<_, StagedProviderEventRow>(
            "SELECT id, conversation_id, run_id, agent_id, sequence, kind, content, native_item_id, payload_json, \
                    mutation_state, overflowed_kind \
             FROM staged_provider_events WHERE run_id = ? AND agent_id = ? \
             ORDER BY sequence, id LIMIT ?",
        )
        .bind(run_id.to_string())
        .bind(agent_id.to_string())
        .bind(i64::try_from(MAX_STAGED_EVENT_ROWS + 1).unwrap())
        .fetch_all(&mut **transaction)
        .await?
    } else {
        sqlx::query_as::<_, StagedProviderEventRow>(
            "SELECT id, conversation_id, run_id, agent_id, sequence, kind, content, native_item_id, payload_json, \
                    mutation_state, overflowed_kind \
             FROM staged_provider_events WHERE run_id = ? ORDER BY sequence, id LIMIT ?",
        )
        .bind(run_id.to_string())
        .bind(i64::try_from(MAX_STAGED_EVENT_ROWS + 1).unwrap())
        .fetch_all(&mut **transaction)
        .await?
    };
    if rows.len() > MAX_STAGED_EVENT_ROWS {
        return Err(StoreError::CorruptStagedEventQueue);
    }
    let staged = rows
        .into_iter()
        .map(StagedProviderEventRow::into_domain)
        .collect::<Result<Vec<_>, _>>()?;
    let now = now_millis();
    let mut drained = Vec::with_capacity(staged.len());
    for event in staged {
        let canonical_assistant = event.agent_id == root_id
            && event.kind == TimelineEventKind::Message
            && event.overflowed_kind.is_none();
        if let Some(native_item_id) = event.native_item_id.as_deref()
            && let Some((existing_id, sequence, existing_content)) =
                sqlx::query_as::<_, (String, i64, String)>(
                    "SELECT id, sequence, content FROM events \
                     WHERE run_id = ? AND agent_id = ? AND kind = 'message' \
                     AND native_item_id = ?",
                )
                .bind(event.run_id.to_string())
                .bind(event.agent_id.to_string())
                .bind(native_item_id)
                .fetch_optional(&mut **transaction)
                .await?
        {
            sqlx::query("UPDATE events SET content = content || ? WHERE id = ?")
                .bind(&event.content)
                .bind(&existing_id)
                .execute(&mut **transaction)
                .await?;
            if canonical_assistant {
                persist_assistant_message_in_transaction(
                    transaction,
                    run_id,
                    conversation_id,
                    provider,
                    Some(native_item_id),
                    &event.content,
                    now,
                )
                .await?;
            }
            drained.push(TimelineEvent {
                id: parse_uuid("timeline event", &existing_id)?.into(),
                conversation_id: event.conversation_id,
                run_id: event.run_id,
                agent_id: event.agent_id,
                sequence: u64::try_from(sequence).map_err(|_| StoreError::InvalidData {
                    entity: "timeline event",
                    detail: "negative sequence".to_owned(),
                })?,
                kind: event.kind,
                role: canonical_assistant.then_some(MessageRole::Assistant),
                content: existing_content + &event.content,
            });
            continue;
        }
        let payload_json = event.payload_json.clone().or_else(|| {
            match (event.mutation_state, event.overflowed_kind) {
                (Some(mutation), Some(overflowed_kind)) => Some(
                    serde_json::json!({
                        "mutation": mutation,
                        "overflowedKind": overflowed_kind,
                    })
                    .to_string(),
                ),
                (Some(mutation), None) => {
                    Some(serde_json::json!({ "mutation": mutation }).to_string())
                }
                (None, _) => None,
            }
        });
        let inserted = sqlx::query(
            "INSERT INTO events \
             (id, conversation_id, run_id, agent_id, kind, role, content, payload_json, native_item_id, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(event.id.to_string())
        .bind(event.conversation_id.to_string())
        .bind(event.run_id.to_string())
        .bind(event.agent_id.to_string())
        .bind(event_kind_label(event.kind))
        .bind(canonical_assistant.then_some("assistant"))
        .bind(&event.content)
        .bind(payload_json)
        .bind(event.native_item_id.as_deref())
        .bind(now)
        .execute(&mut **transaction)
        .await?;
        if canonical_assistant {
            persist_assistant_message_in_transaction(
                transaction,
                run_id,
                conversation_id,
                provider,
                event.native_item_id.as_deref(),
                &event.content,
                now,
            )
            .await?;
        }
        drained.push(TimelineEvent {
            id: event.id,
            conversation_id: event.conversation_id,
            run_id: event.run_id,
            agent_id: event.agent_id,
            sequence: u64::try_from(inserted.last_insert_rowid()).map_err(|_| {
                StoreError::InvalidData {
                    entity: "timeline event",
                    detail: "negative sequence".to_owned(),
                }
            })?,
            kind: event.kind,
            role: canonical_assistant.then_some(MessageRole::Assistant),
            content: event.content,
        });
    }
    if let Some(agent_id) = agent_id {
        sqlx::query("DELETE FROM staged_provider_events WHERE run_id = ? AND agent_id = ?")
            .bind(run_id.to_string())
            .bind(agent_id.to_string())
            .execute(&mut **transaction)
            .await?;
    } else {
        sqlx::query("DELETE FROM staged_provider_events WHERE run_id = ?")
            .bind(run_id.to_string())
            .execute(&mut **transaction)
            .await?;
    }
    Ok(drained)
}

#[derive(Debug, Deserialize, Serialize)]
struct Cursor {
    sequence: u64,
    id: Uuid,
}

#[derive(Debug, Deserialize, Serialize)]
struct AgentCursor {
    created_at: u64,
    id: Uuid,
    run_id: RunId,
}

#[derive(Debug, Deserialize, Serialize)]
struct QuestionCursor {
    approval_id: ApprovalId,
    offset: u32,
}

fn validate_page_limit(limit: u32) -> Result<(), StoreError> {
    if (1..=MAX_PAGE_SIZE).contains(&limit) {
        Ok(())
    } else {
        Err(StoreError::InvalidPageLimit(limit))
    }
}

fn encode_cursor(cursor: Cursor) -> Result<String, StoreError> {
    let json = serde_json::to_vec(&cursor).map_err(|error| StoreError::InvalidData {
        entity: "cursor",
        detail: error.to_string(),
    })?;
    Ok(URL_SAFE_NO_PAD.encode(json))
}

fn decode_cursor(value: &str) -> Result<Cursor, StoreError> {
    let json = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| StoreError::InvalidCursor)?;
    serde_json::from_slice(&json).map_err(|_| StoreError::InvalidCursor)
}

fn encode_agent_cursor(cursor: AgentCursor) -> Result<String, StoreError> {
    let json = serde_json::to_vec(&cursor).map_err(|error| StoreError::InvalidData {
        entity: "agent cursor",
        detail: error.to_string(),
    })?;
    Ok(URL_SAFE_NO_PAD.encode(json))
}

fn decode_agent_cursor(value: &str) -> Result<AgentCursor, StoreError> {
    let json = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| StoreError::InvalidCursor)?;
    serde_json::from_slice(&json).map_err(|_| StoreError::InvalidCursor)
}

fn encode_question_cursor(cursor: QuestionCursor) -> Result<String, StoreError> {
    let json = serde_json::to_vec(&cursor).map_err(|error| StoreError::InvalidData {
        entity: "approval question cursor",
        detail: error.to_string(),
    })?;
    Ok(URL_SAFE_NO_PAD.encode(json))
}

fn decode_question_cursor(value: &str) -> Result<QuestionCursor, StoreError> {
    let json = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| StoreError::InvalidCursor)?;
    serde_json::from_slice(&json).map_err(|_| StoreError::InvalidCursor)
}

fn agent_cursor_created_at_i64(cursor: &AgentCursor) -> Result<i64, StoreError> {
    i64::try_from(cursor.created_at).map_err(|_| StoreError::InvalidCursor)
}

fn cursor_sequence_i64(cursor: &Cursor) -> Result<i64, StoreError> {
    i64::try_from(cursor.sequence).map_err(|_| StoreError::InvalidCursor)
}

fn now_millis() -> i64 {
    i64::try_from(OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
        .expect("the current timestamp fits in SQLite INTEGER")
}

fn lease_expires_at(duration: Duration) -> i64 {
    now_millis().saturating_add(duration_millis(duration))
}

fn duration_millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
fn dispatch_lease_is_protected(
    owner_id: Option<&str>,
    expires_at: Option<i64>,
    now: i64,
    stale_grace: Duration,
) -> bool {
    owner_id.is_some()
        && expires_at.is_some_and(|expires_at| {
            expires_at.saturating_add(duration_millis(stale_grace)) >= now
        })
}

fn validate_agent_transition(from: AgentStatus, to: AgentStatus) -> Result<(), StoreError> {
    let valid = matches!(
        (from, to),
        (
            AgentStatus::Queued,
            AgentStatus::Running | AgentStatus::Interrupted | AgentStatus::Failed
        ) | (
            AgentStatus::Running,
            AgentStatus::Waiting
                | AgentStatus::Completed
                | AgentStatus::Interrupted
                | AgentStatus::Failed
        ) | (
            AgentStatus::Waiting,
            AgentStatus::Running | AgentStatus::Interrupted | AgentStatus::Failed
        )
    );
    if valid {
        Ok(())
    } else {
        Err(DomainError::InvalidTransition {
            entity: "agent node",
            from: agent_status_label(from),
            to: agent_status_label(to),
        }
        .into())
    }
}

fn is_terminal_run_status(status: RunStatus) -> bool {
    matches!(
        status,
        RunStatus::Completed | RunStatus::Interrupted | RunStatus::Failed
    )
}

fn validate_event_state(
    record: &ProviderEventRecord,
    run: &ProviderRun,
    agent: &AgentNode,
) -> Result<(), StoreError> {
    let is_root = agent.parent_id.is_none();
    match record {
        ProviderEventRecord::Started { .. }
            if agent.status != AgentStatus::Queued
                || (is_root && run.status != RunStatus::Queued) =>
        {
            Err(StoreError::InvalidEventState {
                event: "started",
                status: agent_status_label(agent.status),
            })
        }
        ProviderEventRecord::Resumed
            if agent.status != AgentStatus::Waiting
                || (is_root && run.status != RunStatus::Waiting) =>
        {
            Err(StoreError::InvalidEventState {
                event: "resumed",
                status: agent_status_label(agent.status),
            })
        }
        ProviderEventRecord::Progress(_)
        | ProviderEventRecord::Message(_)
        | ProviderEventRecord::NativeMessage { .. }
        | ProviderEventRecord::Tool { .. }
        | ProviderEventRecord::NativeItem { .. }
        | ProviderEventRecord::ChildAgent { .. }
        | ProviderEventRecord::SubAgent { .. }
        | ProviderEventRecord::Unrecognized { .. }
            if agent.status != AgentStatus::Running
                || (is_root && run.status != RunStatus::Running) =>
        {
            Err(StoreError::InvalidEventState {
                event: "progress",
                status: agent_status_label(agent.status),
            })
        }
        ProviderEventRecord::ApprovalRequested { .. }
        | ProviderEventRecord::UserInputRequested { .. }
            if agent.status != AgentStatus::Running
                || (is_root && run.status != RunStatus::Running) =>
        {
            Err(StoreError::InvalidEventState {
                event: "approval requested",
                status: agent_status_label(agent.status),
            })
        }
        _ => Ok(()),
    }
}

fn merge_mutation_state(current: MutationState, observed: MutationState) -> MutationState {
    match (current, observed) {
        (MutationState::Unknown, _) | (_, MutationState::Unknown) => MutationState::Unknown,
        (MutationState::Observed, _) | (_, MutationState::Observed) => MutationState::Observed,
        _ => MutationState::NoneObserved,
    }
}

fn provider_label(provider: ProviderId) -> &'static str {
    match provider {
        ProviderId::Codex => "codex",
        ProviderId::Claude => "claude",
    }
}

fn routing_reason_label(reason: RoutingReason) -> &'static str {
    match reason {
        RoutingReason::ManualOverride => "manualOverride",
        RoutingReason::RequiredCapabilities => "requiredCapabilities",
        RoutingReason::Continuity => "continuity",
        RoutingReason::OnlyEligibleProvider => "onlyEligibleProvider",
        RoutingReason::LeastUsed => "leastUsed",
        RoutingReason::DeterministicTieBreak => "deterministicTieBreak",
        RoutingReason::SafeFallback => "safeFallback",
    }
}

fn parse_routing_reason(value: &str) -> Result<RoutingReason, StoreError> {
    match value {
        "manualOverride" => Ok(RoutingReason::ManualOverride),
        "requiredCapabilities" => Ok(RoutingReason::RequiredCapabilities),
        "continuity" => Ok(RoutingReason::Continuity),
        "onlyEligibleProvider" => Ok(RoutingReason::OnlyEligibleProvider),
        "leastUsed" => Ok(RoutingReason::LeastUsed),
        "deterministicTieBreak" => Ok(RoutingReason::DeterministicTieBreak),
        "safeFallback" => Ok(RoutingReason::SafeFallback),
        value => Err(StoreError::InvalidData {
            entity: "routing reason",
            detail: value.to_owned(),
        }),
    }
}

fn task_kind_label(kind: TaskKind) -> &'static str {
    match kind {
        TaskKind::Implementation => "implementation",
        TaskKind::Review => "review",
        TaskKind::Research => "research",
        TaskKind::General => "general",
    }
}

fn parse_task_kind(value: &str) -> Result<TaskKind, StoreError> {
    match value {
        "implementation" => Ok(TaskKind::Implementation),
        "review" => Ok(TaskKind::Review),
        "research" => Ok(TaskKind::Research),
        "general" => Ok(TaskKind::General),
        value => Err(StoreError::InvalidData {
            entity: "routing task kind",
            detail: value.to_owned(),
        }),
    }
}

fn routing_profile_label(profile: RoutingProfile) -> &'static str {
    match profile {
        RoutingProfile::Balanced => "balanced",
        RoutingProfile::BestFit => "best_fit",
        RoutingProfile::UsageBalance => "usage_balance",
    }
}

fn parse_routing_profile(value: &str) -> Result<RoutingProfile, StoreError> {
    match value {
        "balanced" => Ok(RoutingProfile::Balanced),
        "best_fit" => Ok(RoutingProfile::BestFit),
        "usage_balance" => Ok(RoutingProfile::UsageBalance),
        value => Err(StoreError::InvalidData {
            entity: "routing profile",
            detail: value.to_owned(),
        }),
    }
}

fn invalid_json(entity: &'static str) -> impl FnOnce(serde_json::Error) -> StoreError {
    move |error| StoreError::InvalidData {
        entity,
        detail: error.to_string(),
    }
}

fn invalid_data(entity: &'static str) -> impl FnOnce(serde_json::Error) -> StoreError {
    invalid_json(entity)
}

fn run_status_label(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Queued => "queued",
        RunStatus::Running => "running",
        RunStatus::Waiting => "waiting",
        RunStatus::Completed => "completed",
        RunStatus::Interrupted => "interrupted",
        RunStatus::Failed => "failed",
    }
}

fn agent_status_label(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Queued => "queued",
        AgentStatus::Running => "running",
        AgentStatus::Waiting => "waiting",
        AgentStatus::Completed => "completed",
        AgentStatus::Interrupted => "interrupted",
        AgentStatus::Failed => "failed",
    }
}

fn mutation_state_label(state: MutationState) -> &'static str {
    match state {
        MutationState::NoneObserved => "none_observed",
        MutationState::Observed => "observed",
        MutationState::Unknown => "unknown",
    }
}

fn dispatch_certainty_label(certainty: DispatchCertainty) -> &'static str {
    match certainty {
        DispatchCertainty::NotDispatched => "not_dispatched",
        DispatchCertainty::MayHaveDispatched => "may_have_dispatched",
    }
}

fn approval_resolution_label(resolution: &ApprovalResolution) -> &'static str {
    match resolution {
        ApprovalResolution::Approved => "approved",
        ApprovalResolution::Denied => "denied",
        ApprovalResolution::Answer(_) | ApprovalResolution::Answers(_) => "answered",
        ApprovalResolution::Cancelled => "cancelled",
        ApprovalResolution::Failed => "failed",
    }
}

fn approval_response_intent_status_label(status: ApprovalResponseIntentStatus) -> &'static str {
    match status {
        ApprovalResponseIntentStatus::Recorded => "recorded",
        ApprovalResponseIntentStatus::Acknowledged => "acknowledged",
        ApprovalResponseIntentStatus::Rejected => "rejected",
        ApprovalResponseIntentStatus::DispatchUnknown => "dispatch_unknown",
    }
}

fn serialize_approval_resolution(resolution: &ApprovalResolution) -> Result<String, StoreError> {
    serde_json::to_string(resolution).map_err(|error| StoreError::InvalidData {
        entity: "approval resolution",
        detail: error.to_string(),
    })
}

fn provider_error_content(category: ProviderErrorCategory) -> &'static str {
    match category {
        ProviderErrorCategory::NotInstalled => "Provider failed: not installed",
        ProviderErrorCategory::TimedOut => "Provider failed: timed out",
        ProviderErrorCategory::InspectionFailed => "Provider failed: inspection failed",
        ProviderErrorCategory::Rejected => "Provider failed: request rejected",
        ProviderErrorCategory::Protocol => "Provider failed: protocol error",
        ProviderErrorCategory::Transport => "Provider failed: transport error",
        ProviderErrorCategory::MalformedJson => "Provider failed: malformed JSON",
        ProviderErrorCategory::OversizedFrame => "Provider failed: oversized frame",
        ProviderErrorCategory::ProcessExited => "Provider failed: process exited",
        ProviderErrorCategory::StreamClosed => "Provider failed: stream closed",
        ProviderErrorCategory::ContractViolation => "Provider failed: contract violation",
    }
}

fn event_kind_label(kind: TimelineEventKind) -> &'static str {
    match kind {
        TimelineEventKind::Message => "message",
        TimelineEventKind::Tool => "tool",
        TimelineEventKind::Progress => "progress",
        TimelineEventKind::Diagnostic => "diagnostic",
        TimelineEventKind::Lifecycle => "lifecycle",
    }
}

fn parse_uuid(entity: &'static str, value: &str) -> Result<Uuid, StoreError> {
    Uuid::parse_str(value).map_err(|error| StoreError::InvalidData {
        entity,
        detail: error.to_string(),
    })
}

fn parse_provider(value: &str) -> Result<ProviderId, StoreError> {
    match value {
        "codex" => Ok(ProviderId::Codex),
        "claude" => Ok(ProviderId::Claude),
        value => Err(StoreError::InvalidData {
            entity: "provider",
            detail: value.to_owned(),
        }),
    }
}

fn parse_run_status(value: &str) -> Result<RunStatus, StoreError> {
    match value {
        "queued" => Ok(RunStatus::Queued),
        "running" => Ok(RunStatus::Running),
        "waiting" => Ok(RunStatus::Waiting),
        "completed" => Ok(RunStatus::Completed),
        "interrupted" => Ok(RunStatus::Interrupted),
        "failed" => Ok(RunStatus::Failed),
        value => Err(StoreError::InvalidData {
            entity: "provider run status",
            detail: value.to_owned(),
        }),
    }
}

fn parse_agent_status(value: &str) -> Result<AgentStatus, StoreError> {
    match value {
        "queued" => Ok(AgentStatus::Queued),
        "running" => Ok(AgentStatus::Running),
        "waiting" => Ok(AgentStatus::Waiting),
        "completed" => Ok(AgentStatus::Completed),
        "interrupted" => Ok(AgentStatus::Interrupted),
        "failed" => Ok(AgentStatus::Failed),
        value => Err(StoreError::InvalidData {
            entity: "agent node status",
            detail: value.to_owned(),
        }),
    }
}

fn parse_rollup_status(value: &str) -> Result<crate::domain::RollupStatus, StoreError> {
    match value {
        "needs_attention" => Ok(crate::domain::RollupStatus::NeedsAttention),
        "active" => Ok(crate::domain::RollupStatus::Active),
        "failed" => Ok(crate::domain::RollupStatus::Failed),
        "interrupted" => Ok(crate::domain::RollupStatus::Interrupted),
        "completed" => Ok(crate::domain::RollupStatus::Completed),
        value => Err(StoreError::InvalidData {
            entity: "conversation rollup status",
            detail: value.to_owned(),
        }),
    }
}

fn parse_mutation_state(value: &str) -> Result<MutationState, StoreError> {
    match value {
        "none_observed" => Ok(MutationState::NoneObserved),
        "observed" => Ok(MutationState::Observed),
        "unknown" => Ok(MutationState::Unknown),
        value => Err(StoreError::InvalidData {
            entity: "provider run mutation state",
            detail: value.to_owned(),
        }),
    }
}

fn parse_dispatch_certainty(value: &str) -> Result<DispatchCertainty, StoreError> {
    match value {
        "not_dispatched" => Ok(DispatchCertainty::NotDispatched),
        "may_have_dispatched" => Ok(DispatchCertainty::MayHaveDispatched),
        value => Err(StoreError::InvalidData {
            entity: "provider run dispatch certainty",
            detail: value.to_owned(),
        }),
    }
}

fn parse_approval_status(value: &str) -> Result<ApprovalStatus, StoreError> {
    match value {
        "pending" => Ok(ApprovalStatus::Pending),
        "approved" => Ok(ApprovalStatus::Approved),
        "denied" => Ok(ApprovalStatus::Denied),
        "answered" => Ok(ApprovalStatus::Answered),
        "cancelled" => Ok(ApprovalStatus::Cancelled),
        "failed" => Ok(ApprovalStatus::Failed),
        value => Err(StoreError::InvalidData {
            entity: "approval status",
            detail: value.to_owned(),
        }),
    }
}

fn parse_approval_response_intent_status(
    value: &str,
) -> Result<ApprovalResponseIntentStatus, StoreError> {
    match value {
        "recorded" => Ok(ApprovalResponseIntentStatus::Recorded),
        "acknowledged" => Ok(ApprovalResponseIntentStatus::Acknowledged),
        "rejected" => Ok(ApprovalResponseIntentStatus::Rejected),
        "dispatch_unknown" => Ok(ApprovalResponseIntentStatus::DispatchUnknown),
        value => Err(StoreError::InvalidData {
            entity: "approval response intent status",
            detail: value.to_owned(),
        }),
    }
}

fn parse_event_kind(value: &str) -> Result<TimelineEventKind, StoreError> {
    match value {
        "message" => Ok(TimelineEventKind::Message),
        "tool" => Ok(TimelineEventKind::Tool),
        "progress" => Ok(TimelineEventKind::Progress),
        "diagnostic" => Ok(TimelineEventKind::Diagnostic),
        "lifecycle" => Ok(TimelineEventKind::Lifecycle),
        value => Err(StoreError::InvalidData {
            entity: "timeline event kind",
            detail: value.to_owned(),
        }),
    }
}

fn parse_message_role(value: &str) -> Result<MessageRole, StoreError> {
    match value {
        "user" => Ok(MessageRole::User),
        "assistant" => Ok(MessageRole::Assistant),
        _ => Err(StoreError::InvalidData {
            entity: "timeline event role",
            detail: format!("unknown role {value}"),
        }),
    }
}

fn normalize_conversation_title(title: String) -> Result<String, StoreError> {
    let title = title.trim();
    if title.is_empty() || title.len() > MAX_CONVERSATION_TITLE_BYTES {
        return Err(StoreError::InvalidData {
            entity: "conversation title",
            detail: format!(
                "must contain 1 to {MAX_CONVERSATION_TITLE_BYTES} bytes after trimming"
            ),
        });
    }
    Ok(title.to_owned())
}

fn normalize_legacy_conversation_title(title: String) -> String {
    let title = title.trim();
    if title.is_empty() {
        return UNTITLED_CONVERSATION_TITLE.to_owned();
    }
    truncate_utf8(title.to_owned(), MAX_CONVERSATION_TITLE_BYTES)
}

fn truncate_utf8(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value
}

#[derive(FromRow)]
struct ConversationRow {
    id: String,
    title: String,
    workspace_id: Option<String>,
    status: String,
    updated_at: i64,
}

struct ConversationRecord {
    conversation: Conversation,
    updated_at: i64,
}

impl ConversationRow {
    fn into_record(self) -> Result<ConversationRecord, StoreError> {
        let archived = match self.status.as_str() {
            "active" => false,
            "archived" => true,
            value => {
                return Err(StoreError::InvalidData {
                    entity: "conversation status",
                    detail: value.to_owned(),
                });
            }
        };
        let workspace_id = self
            .workspace_id
            .map(|id| parse_uuid("workspace", &id).map(Into::into))
            .transpose()?;
        Ok(ConversationRecord {
            conversation: Conversation {
                id: parse_uuid("conversation", &self.id)?.into(),
                title: normalize_legacy_conversation_title(self.title),
                workspace_id,
                archived,
            },
            updated_at: self.updated_at,
        })
    }
}

#[derive(FromRow)]
struct ProviderRunRow {
    id: String,
    conversation_id: String,
    provider: String,
    fallback_from_run_id: Option<String>,
    native_session_id: Option<String>,
    status: String,
    mutation_state: String,
    dispatch_certainty: Option<String>,
    created_at: i64,
}

#[derive(FromRow)]
struct SubmittedRunRow {
    request_hash: String,
    id: String,
    conversation_id: String,
    provider: String,
    fallback_from_run_id: Option<String>,
    native_session_id: Option<String>,
    status: String,
    mutation_state: String,
    dispatch_certainty: Option<String>,
    created_at: i64,
}

impl SubmittedRunRow {
    fn into_parts(self) -> Result<(String, ProviderRun), StoreError> {
        let request_hash = self.request_hash;
        let run = ProviderRunRow {
            id: self.id,
            conversation_id: self.conversation_id,
            provider: self.provider,
            fallback_from_run_id: self.fallback_from_run_id,
            native_session_id: self.native_session_id,
            status: self.status,
            mutation_state: self.mutation_state,
            dispatch_certainty: self.dispatch_certainty,
            created_at: self.created_at,
        }
        .into_domain()?;
        Ok((request_hash, run))
    }
}

impl ProviderRunRow {
    fn into_domain(self) -> Result<ProviderRun, StoreError> {
        Ok(ProviderRun {
            id: parse_uuid("provider run", &self.id)?.into(),
            conversation_id: parse_uuid("conversation", &self.conversation_id)?.into(),
            provider: parse_provider(&self.provider)?,
            fallback_from_run_id: self
                .fallback_from_run_id
                .map(|id| parse_uuid("provider run", &id).map(Into::into))
                .transpose()?,
            native_session_id: self.native_session_id,
            status: parse_run_status(&self.status)?,
            mutation_state: parse_mutation_state(&self.mutation_state)?,
            dispatch_certainty: self
                .dispatch_certainty
                .map(|certainty| parse_dispatch_certainty(&certainty))
                .transpose()?,
        })
    }
}

#[derive(FromRow)]
struct ApprovalRow {
    id: String,
    run_id: String,
    agent_id: String,
    provider: String,
    provider_request_id: Option<String>,
    operation: String,
    scope: String,
    request_json: Option<String>,
    details_json: Option<String>,
    status: String,
    resolution_json: Option<String>,
    response_intent_json: Option<String>,
    response_intent_status: Option<String>,
}

#[derive(FromRow)]
struct ApprovalListRow {
    id: String,
    run_id: String,
    agent_id: String,
    provider: String,
    operation: String,
    scope: String,
    status: String,
    response_intent_status: Option<String>,
    created_at: i64,
}

#[derive(FromRow)]
struct RunAuditRow {
    id: String,
    provider: String,
    status: String,
    created_at: i64,
    routing_reason: Option<String>,
    routing_truncated: bool,
    has_handoff: bool,
}

impl RunAuditRow {
    fn into_summary(self) -> Result<RunAuditSummary, StoreError> {
        Ok(RunAuditSummary {
            id: parse_uuid("provider run", &self.id)?.into(),
            provider: parse_provider(&self.provider)?,
            status: parse_run_status(&self.status)?,
            reason: self
                .routing_reason
                .as_deref()
                .map(parse_routing_reason)
                .transpose()?,
            routing_truncated: self.routing_truncated,
            has_handoff: self.has_handoff,
            created_at: self.created_at,
        })
    }
}

#[derive(FromRow)]
struct RunAuditDetailRow {
    id: String,
    provider: String,
    status: String,
    routing_json: Option<String>,
    routing_reason: Option<String>,
    routing_truncated: bool,
    handoff: Option<String>,
    handoff_truncated: bool,
}

impl RunAuditDetailRow {
    fn into_detail(self) -> Result<RunAuditDetailRecord, StoreError> {
        Ok(RunAuditDetailRecord {
            id: parse_uuid("provider run", &self.id)?.into(),
            provider: parse_provider(&self.provider)?,
            status: parse_run_status(&self.status)?,
            routing: self
                .routing_json
                .map(|value| serde_json::from_str(&value).map_err(invalid_data("routing decision")))
                .transpose()?,
            reason: self
                .routing_reason
                .as_deref()
                .map(parse_routing_reason)
                .transpose()?,
            routing_truncated: self.routing_truncated,
            handoff: self
                .handoff
                .map(|value| truncate_utf8(value, MAX_RUN_AUDIT_HANDOFF_BYTES as usize)),
            handoff_truncated: self.handoff_truncated,
        })
    }
}

#[derive(FromRow)]
struct ApprovalDetailRow {
    id: String,
    status: String,
    response_intent_status: Option<String>,
    operation: String,
    scope: String,
    request_json: Option<String>,
    details_json: Option<String>,
    question_count: i64,
    truncated: bool,
}

impl ApprovalDetailRow {
    fn into_record(self) -> Result<ApprovalDetailRecord, StoreError> {
        let response_pending = self
            .response_intent_status
            .as_deref()
            .is_some_and(|status| matches!(status, "recorded" | "dispatch_unknown"));
        if let Some(status) = self.response_intent_status.as_deref() {
            parse_approval_response_intent_status(status)?;
        }
        Ok(ApprovalDetailRecord {
            id: parse_uuid("approval", &self.id)?.into(),
            status: parse_approval_status(&self.status)?,
            response_pending,
            agent_path: Vec::new(),
            agent_path_truncated: false,
            operation: truncate_utf8(self.operation, MAX_EVENT_DETAIL_BYTES),
            scope: truncate_utf8(self.scope, MAX_EVENT_DETAIL_BYTES),
            input: self
                .request_json
                .map(|value| {
                    serde_json::from_str(&value).map_err(invalid_data("user input request"))
                })
                .transpose()?,
            details: self
                .details_json
                .map(|value| {
                    serde_json::from_str(&value).map_err(invalid_data("approval request details"))
                })
                .transpose()?,
            question_count: u32::try_from(self.question_count).map_err(|_| {
                StoreError::InvalidData {
                    entity: "approval question count",
                    detail: "count exceeds the supported range".to_owned(),
                }
            })?,
            truncated: self.truncated,
        })
    }
}

#[derive(FromRow)]
struct ApprovalQuestionRow {
    ordinal: i64,
    header: String,
    question: String,
    options_json: Option<String>,
    is_other: bool,
    is_secret: bool,
    source_bytes: i64,
    header_bytes: i64,
    question_bytes: i64,
}

impl ApprovalQuestionRow {
    fn into_preview(self) -> Result<ApprovalQuestionPreview, StoreError> {
        let ordinal = u32::try_from(self.ordinal).map_err(|_| StoreError::InvalidData {
            entity: "approval question",
            detail: "ordinal exceeds the supported range".to_owned(),
        })?;
        let options = self
            .options_json
            .map(|value| serde_json::from_str(&value).map_err(invalid_data("approval options")))
            .transpose()?;
        Ok(ApprovalQuestionPreview {
            id: format!("question-{}", ordinal.saturating_add(1)),
            header: truncate_utf8(self.header, MAX_APPROVAL_QUESTION_HEADER_BYTES),
            question: truncate_utf8(self.question, MAX_APPROVAL_QUESTION_TEXT_BYTES),
            options,
            is_other: self.is_other,
            is_secret: self.is_secret,
            truncated: self.source_bytes > MAX_APPROVAL_QUESTION_SOURCE_BYTES
                || self.header_bytes > MAX_APPROVAL_QUESTION_HEADER_BYTES as i64
                || self.question_bytes > MAX_APPROVAL_QUESTION_TEXT_BYTES as i64,
        })
    }
}

impl ApprovalListRow {
    fn into_summary(self) -> Result<ApprovalSummary, StoreError> {
        let response_pending = self
            .response_intent_status
            .as_deref()
            .is_some_and(|status| matches!(status, "recorded" | "dispatch_unknown"));
        if let Some(status) = self.response_intent_status.as_deref() {
            parse_approval_response_intent_status(status)?;
        }
        Ok(ApprovalSummary {
            id: parse_uuid("approval", &self.id)?.into(),
            run_id: parse_uuid("provider run", &self.run_id)?.into(),
            agent_id: parse_uuid("agent node", &self.agent_id)?.into(),
            provider: parse_provider(&self.provider)?,
            operation: truncate_utf8(self.operation, 256),
            scope: truncate_utf8(self.scope, 512),
            status: parse_approval_status(&self.status)?,
            response_pending,
            agent_path: Vec::new(),
            agent_path_truncated: false,
        })
    }
}

impl ApprovalRow {
    fn into_domain(self) -> Result<Approval, StoreError> {
        let input = self
            .request_json
            .map(|value| {
                serde_json::from_str(&value).map_err(|error| StoreError::InvalidData {
                    entity: "user input request",
                    detail: error.to_string(),
                })
            })
            .transpose()?;
        let details = self
            .details_json
            .map(|value| {
                serde_json::from_str(&value).map_err(|error| StoreError::InvalidData {
                    entity: "approval request details",
                    detail: error.to_string(),
                })
            })
            .transpose()?;
        let status = parse_approval_status(&self.status)?;
        let resolution: Option<ApprovalResolution> = self
            .resolution_json
            .map(|value| {
                serde_json::from_str(&value).map_err(|error| StoreError::InvalidData {
                    entity: "approval resolution",
                    detail: error.to_string(),
                })
            })
            .transpose()?;
        let resolution_matches_status = match &resolution {
            None => status == ApprovalStatus::Pending,
            Some(resolution) => resolution.status() == status,
        };
        if !resolution_matches_status {
            return Err(StoreError::InvalidData {
                entity: "approval resolution",
                detail: "does not match approval status".to_owned(),
            });
        }
        let response_intent = match (self.response_intent_json, self.response_intent_status) {
            (None, None) => None,
            (Some(resolution), Some(status)) => Some(ApprovalResponseIntent {
                resolution: serde_json::from_str(&resolution).map_err(|error| {
                    StoreError::InvalidData {
                        entity: "approval response intent",
                        detail: error.to_string(),
                    }
                })?,
                status: parse_approval_response_intent_status(&status)?,
            }),
            _ => {
                return Err(StoreError::InvalidData {
                    entity: "approval response intent",
                    detail: "intent and status must both be present".to_owned(),
                });
            }
        };
        if response_intent.as_ref().is_some_and(|intent| {
            intent.status == ApprovalResponseIntentStatus::Acknowledged
                && resolution.as_ref() != Some(&intent.resolution)
        }) {
            return Err(StoreError::InvalidData {
                entity: "approval response intent",
                detail: "acknowledged intent does not match approval resolution".to_owned(),
            });
        }
        Ok(Approval {
            id: parse_uuid("approval", &self.id)?.into(),
            run_id: parse_uuid("provider run", &self.run_id)?.into(),
            agent_id: parse_uuid("agent node", &self.agent_id)?.into(),
            provider: parse_provider(&self.provider)?,
            provider_request_id: self.provider_request_id,
            operation: self.operation,
            scope: self.scope,
            input,
            details,
            status,
            resolution,
            response_intent,
        })
    }
}

#[derive(FromRow)]
struct AgentNodeRow {
    id: String,
    run_id: String,
    parent_id: Option<String>,
    provider: String,
    provider_native_id: Option<String>,
    provider_native_path: Option<String>,
    label: String,
    summary: Option<String>,
    status: String,
    created_at: i64,
}

impl AgentNodeRow {
    fn into_domain(self) -> Result<AgentNode, StoreError> {
        Ok(AgentNode {
            id: parse_uuid("agent node", &self.id)?.into(),
            run_id: parse_uuid("provider run", &self.run_id)?.into(),
            parent_id: self
                .parent_id
                .map(|id| parse_uuid("agent node", &id).map(Into::into))
                .transpose()?,
            provider: parse_provider(&self.provider)?,
            provider_native_id: self.provider_native_id,
            provider_native_path: self.provider_native_path,
            label: self.label,
            summary: self.summary,
            status: parse_agent_status(&self.status)?,
        })
    }
}

#[derive(FromRow)]
struct AgentPageRow {
    id: String,
    run_id: String,
    parent_id: Option<String>,
    provider: String,
    provider_native_id: Option<String>,
    provider_native_path: Option<String>,
    label: String,
    summary: Option<String>,
    status: String,
    created_at: i64,
    depth: i64,
}

#[derive(FromRow)]
struct RecoveryAgentRow {
    run_id: String,
    agent_id: String,
    mutation_state: String,
    is_root: bool,
    depth: i64,
}

impl RecoveryAgentRow {
    fn into_domain(self) -> Result<RecoveryAgent, StoreError> {
        Ok(RecoveryAgent {
            run_id: parse_uuid("provider run", &self.run_id)?.into(),
            agent_id: parse_uuid("agent node", &self.agent_id)?.into(),
            mutation_state: parse_mutation_state(&self.mutation_state)?,
            is_root: self.is_root,
            depth: u32::try_from(self.depth).map_err(|_| StoreError::InvalidData {
                entity: "agent node",
                detail: "depth exceeds the supported range".to_owned(),
            })?,
        })
    }
}

impl AgentPageRow {
    fn into_record(self) -> Result<AgentPageRecord, StoreError> {
        let depth = u32::try_from(self.depth).map_err(|_| StoreError::InvalidData {
            entity: "agent node",
            detail: "depth exceeds the supported range".to_owned(),
        })?;
        Ok(AgentPageRecord {
            agent: AgentNodeRow {
                id: self.id,
                run_id: self.run_id,
                parent_id: self.parent_id,
                provider: self.provider,
                provider_native_id: self.provider_native_id,
                provider_native_path: self.provider_native_path,
                label: truncate_utf8(self.label, MAX_AGENT_LABEL_PREVIEW_BYTES),
                summary: self
                    .summary
                    .map(|summary| truncate_utf8(summary, MAX_AGENT_SUMMARY_PREVIEW_BYTES)),
                status: self.status,
                created_at: self.created_at,
            }
            .into_domain()?,
            depth,
        })
    }
}

#[derive(FromRow)]
struct TimelineEventRow {
    id: String,
    conversation_id: String,
    run_id: String,
    agent_id: String,
    sequence: i64,
    kind: String,
    role: Option<String>,
    content: String,
}

#[derive(FromRow)]
struct TimelineRecordRow {
    id: String,
    conversation_id: String,
    run_id: String,
    agent_id: String,
    sequence: i64,
    kind: String,
    role: Option<String>,
    content: String,
    content_bytes: i64,
    provider: String,
}

impl TimelineRecordRow {
    fn into_record(self) -> Result<TimelineRecord, StoreError> {
        let provider = parse_provider(&self.provider)?;
        let content_bytes =
            usize::try_from(self.content_bytes).map_err(|_| StoreError::InvalidData {
                entity: "timeline event",
                detail: "negative content length".to_owned(),
            })?;
        let content = truncate_utf8(self.content, MAX_TIMELINE_PREVIEW_BYTES);
        let content_truncated = content_bytes > content.len();
        Ok(TimelineRecord {
            event: TimelineEventRow {
                id: self.id,
                conversation_id: self.conversation_id,
                run_id: self.run_id,
                agent_id: self.agent_id,
                sequence: self.sequence,
                kind: self.kind,
                role: self.role,
                content,
            }
            .into_domain()?,
            provider,
            content_bytes,
            content_truncated,
        })
    }
}

#[derive(FromRow)]
struct SidebarDetailRow {
    conversation_id: String,
    routing_profile: String,
    project_root: Option<String>,
    run_id: Option<String>,
    provider: Option<String>,
    fallback_from_run_id: Option<String>,
    native_session_id: Option<String>,
    run_status: Option<String>,
    mutation_state: Option<String>,
    dispatch_certainty: Option<String>,
    run_created_at: Option<i64>,
    rollup_status: Option<String>,
    active_descendant_count: Option<i64>,
    total_agent_count: Option<i64>,
    agent_id: Option<String>,
    parent_id: Option<String>,
    agent_provider: Option<String>,
    provider_native_id: Option<String>,
    provider_native_path: Option<String>,
    label: Option<String>,
    summary: Option<String>,
    agent_status: Option<String>,
    agent_created_at: Option<i64>,
}

impl SidebarDetailRow {
    fn provider_run(&self) -> Result<Option<ProviderRun>, StoreError> {
        let Some(id) = self.run_id.as_deref() else {
            return Ok(None);
        };
        ProviderRunRow {
            id: id.to_owned(),
            conversation_id: self.conversation_id.clone(),
            provider: required_sidebar(&self.provider, "provider")?,
            fallback_from_run_id: self.fallback_from_run_id.clone(),
            native_session_id: self.native_session_id.clone(),
            status: required_sidebar(&self.run_status, "run status")?,
            mutation_state: required_sidebar(&self.mutation_state, "mutation state")?,
            dispatch_certainty: self.dispatch_certainty.clone(),
            created_at: self.run_created_at.ok_or_else(|| StoreError::InvalidData {
                entity: "sidebar run",
                detail: "missing creation timestamp".to_owned(),
            })?,
        }
        .into_domain()
        .map(Some)
    }

    fn agent(&self) -> Result<Option<AgentNode>, StoreError> {
        let Some(id) = self.agent_id.as_deref() else {
            return Ok(None);
        };
        let run_id = required_sidebar(&self.run_id, "run id")?;
        let _ = self.agent_created_at;
        AgentNodeRow {
            id: id.to_owned(),
            run_id,
            parent_id: self.parent_id.clone(),
            provider: required_sidebar(&self.agent_provider, "agent provider")?,
            provider_native_id: self.provider_native_id.clone(),
            provider_native_path: self.provider_native_path.clone(),
            label: truncate_utf8(
                required_sidebar(&self.label, "agent label")?,
                MAX_AGENT_LABEL_PREVIEW_BYTES,
            ),
            summary: self
                .summary
                .clone()
                .map(|summary| truncate_utf8(summary, MAX_AGENT_SUMMARY_PREVIEW_BYTES)),
            status: required_sidebar(&self.agent_status, "agent status")?,
            created_at: self
                .agent_created_at
                .ok_or_else(|| StoreError::InvalidData {
                    entity: "sidebar agent",
                    detail: "missing creation timestamp".to_owned(),
                })?,
        }
        .into_domain()
        .map(Some)
    }
}

fn required_sidebar(value: &Option<String>, field: &'static str) -> Result<String, StoreError> {
    value.clone().ok_or_else(|| StoreError::InvalidData {
        entity: "sidebar snapshot",
        detail: format!("missing {field}"),
    })
}

#[derive(FromRow)]
struct StagedProviderEventRow {
    id: String,
    conversation_id: String,
    run_id: String,
    agent_id: String,
    sequence: i64,
    kind: String,
    content: String,
    native_item_id: Option<String>,
    payload_json: Option<String>,
    mutation_state: Option<String>,
    overflowed_kind: Option<String>,
}

impl StagedProviderEventRow {
    fn into_domain(self) -> Result<StagedProviderEvent, StoreError> {
        Ok(StagedProviderEvent {
            id: parse_uuid("staged provider event", &self.id)?.into(),
            conversation_id: parse_uuid("conversation", &self.conversation_id)?.into(),
            run_id: parse_uuid("provider run", &self.run_id)?.into(),
            agent_id: parse_uuid("agent node", &self.agent_id)?.into(),
            sequence: u64::try_from(self.sequence).map_err(|_| StoreError::InvalidData {
                entity: "staged provider event",
                detail: "negative sequence".to_owned(),
            })?,
            kind: parse_event_kind(&self.kind)?,
            content: self.content,
            native_item_id: self.native_item_id,
            payload_json: self.payload_json,
            mutation_state: self
                .mutation_state
                .map(|mutation| parse_mutation_state(&mutation))
                .transpose()?,
            overflowed_kind: self
                .overflowed_kind
                .map(|kind| parse_event_kind(&kind))
                .transpose()?,
        })
    }
}

impl TimelineEventRow {
    fn into_domain(self) -> Result<TimelineEvent, StoreError> {
        Ok(TimelineEvent {
            id: parse_uuid("timeline event", &self.id)?.into(),
            conversation_id: parse_uuid("conversation", &self.conversation_id)?.into(),
            run_id: parse_uuid("provider run", &self.run_id)?.into(),
            agent_id: parse_uuid("agent node", &self.agent_id)?.into(),
            sequence: u64::try_from(self.sequence).map_err(|_| StoreError::InvalidData {
                entity: "timeline event",
                detail: "negative sequence".to_owned(),
            })?,
            kind: parse_event_kind(&self.kind)?,
            role: self
                .role
                .map(|role| parse_message_role(&role))
                .transpose()?,
            content: self.content,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use sqlx::sqlite::SqlitePoolOptions;
    use sqlx::{AssertSqlSafe, SqlitePool};
    use tokio::sync::broadcast;

    use crate::domain::{
        AgentId, ApprovalId, ApprovalResolution, ConversationId, MessageId, MessageRole,
        MutationState, RunId, RunStatus, TimelineEventId, TimelineEventKind, UserInputQuestion,
        Workspace, WorkspaceId,
    };
    use crate::providers::{
        DispatchCertainty, NativeAgentStatus, NativeChildStatus, NativeSubAgentActivityKind,
        ProviderErrorCategory, ProviderId,
    };

    use super::{
        ConversationSettings, MAX_APPROVAL_AGENT_PATH_NODES, MAX_CANONICAL_MESSAGE_BYTES,
        MAX_NATIVE_AGENT_ID_BYTES, MAX_OBJECTIVE_BYTES, MAX_RUN_AUDIT_HANDOFF_BYTES,
        MAX_STAGED_EVENT_BYTES, MAX_STAGED_EVENT_ROWS, MIGRATOR, NewConversation,
        NewFallbackAttempt, NewSubmission, PreparedSubmission, ProviderEventRecord,
        STAGED_OVERFLOW_CONTENT, STORE_CHANGE_CHANNEL_CAPACITY, Store, StoreError,
        dispatch_lease_is_protected, provider_label,
    };

    #[test]
    fn dispatch_lease_grace_tolerates_delayed_renewal_but_eventually_expires() {
        let expiry = 1_000_000;
        let grace = Duration::from_secs(300);

        assert!(dispatch_lease_is_protected(
            Some("live-owner"),
            Some(expiry),
            expiry + 120_000,
            grace,
        ));
        assert!(!dispatch_lease_is_protected(
            Some("crashed-owner"),
            Some(expiry),
            expiry + 300_001,
            grace,
        ));
        assert!(!dispatch_lease_is_protected(
            None,
            Some(expiry),
            expiry,
            grace,
        ));
    }

    #[tokio::test]
    async fn stale_recovery_claim_atomically_fences_the_previous_owner() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("stale lease fence"))
            .await
            .unwrap();
        let (run, _) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        let lease = Duration::from_secs(120);
        let grace = Duration::from_secs(300);
        assert!(
            store
                .claim_provider_dispatch(run.id, "provider-owner", lease)
                .await
                .unwrap()
        );
        store.delay_dispatch_lease_for_test(run.id).await.unwrap();
        assert!(
            store
                .refresh_provider_dispatch_lease(run.id, "provider-owner", lease, grace)
                .await
                .unwrap()
        );
        store.expire_dispatch_lease_for_test(run.id).await.unwrap();

        let (late_refresh, recovery_claim) = tokio::join!(
            store.refresh_provider_dispatch_lease(run.id, "provider-owner", lease, grace),
            store.claim_stale_provider_dispatch(
                run.id,
                "recovery-owner",
                "current-supervisor",
                lease,
                grace,
            ),
        );

        assert!(!late_refresh.unwrap());
        assert!(recovery_claim.unwrap());
        assert!(
            store
                .refresh_provider_dispatch_lease(run.id, "recovery-owner", lease, grace)
                .await
                .unwrap()
        );
        store.expire_dispatch_lease_for_test(run.id).await.unwrap();
        assert!(
            !store
                .claim_stale_provider_dispatch(
                    run.id,
                    "next-recovery-owner",
                    "recovery-owner",
                    lease,
                    grace,
                )
                .await
                .unwrap()
        );
        assert!(
            store
                .claim_stale_provider_dispatch(
                    run.id,
                    "next-recovery-owner",
                    "current-supervisor",
                    lease,
                    grace,
                )
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn every_owned_write_rejects_a_superseded_owner_before_state_validation() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("owned write fence"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        assert!(
            store
                .claim_provider_dispatch(run.id, "former-owner", Duration::from_secs(120))
                .await
                .unwrap()
        );
        store
            .replace_dispatch_owner_for_test(run.id, "current-owner")
            .await
            .unwrap();

        macro_rules! assert_fenced {
            ($future:expr) => {
                assert!(matches!(
                    $future.await.unwrap_err(),
                    StoreError::DispatchOwnerMismatch(id) if id == run.id
                ));
            };
        }

        assert_fenced!(store.bind_owned_native_session_with_group(
            run.id,
            "native-session",
            None,
            "former-owner",
        ));
        assert_fenced!(store.advance_owned_provider_context(run.id, "former-owner"));
        assert_fenced!(store.append_owned_run_event(
            run.id,
            root.id,
            "former-owner",
            ProviderEventRecord::started(),
        ));
        assert_fenced!(store.stage_owned_waiting_event(
            run.id,
            root.id,
            "former-owner",
            ProviderEventRecord::progress("late output"),
        ));
        assert_fenced!(store.record_owned_response_intent(
            run.id,
            root.id,
            "missing-request",
            ApprovalResolution::Approved,
            "former-owner",
        ));
        assert_fenced!(store.reject_owned_response_intent(
            run.id,
            root.id,
            "missing-request",
            DispatchCertainty::MayHaveDispatched,
            "former-owner",
        ));
        assert_fenced!(store.acknowledge_owned_response_intent(
            run.id,
            root.id,
            "missing-request",
            "former-owner",
        ));
        assert_fenced!(store.fail_owned_run_if_active(
            run.id,
            root.id,
            ProviderErrorCategory::ContractViolation,
            MutationState::Unknown,
            DispatchCertainty::MayHaveDispatched,
            "former-owner",
        ));
        assert_fenced!(store.fail_and_create_owned_fallback(
            run.id,
            root.id,
            "former-owner",
            Duration::from_secs(120),
            ProviderErrorCategory::Rejected,
            NewFallbackAttempt {
                provider: ProviderId::Claude,
                native_session_id: None,
                handoff_rendered: None,
                handoff_hash: None,
                routing_decision: None,
                turn_prompt: "fallback".to_owned(),
            },
        ));

        assert_eq!(
            store.load_run(run.id).await.unwrap().status,
            RunStatus::Queued
        );
        let event_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE run_id = ?")
            .bind(run.id.to_string())
            .fetch_one(&store.pool)
            .await
            .unwrap();
        let session_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM provider_sessions WHERE conversation_id = ?")
                .bind(conversation.id.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(event_count, 0);
        assert_eq!(session_count, 0);
    }

    async fn explain_query_plan(pool: &SqlitePool, statement: &str) -> Vec<String> {
        sqlx::query_as::<_, (i64, i64, i64, String)>(AssertSqlSafe(format!(
            "EXPLAIN QUERY PLAN {statement}"
        )))
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|(_, _, _, detail)| detail)
        .collect()
    }

    #[tokio::test]
    async fn store_initializes_sqlite_and_all_owned_tables() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("nested").join("store.sqlite3");

        let store = Store::open(&path).await.unwrap();

        assert!(path.exists());
        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        let busy_timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        let table_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name IN \
             ('workspaces', 'conversations', 'provider_sessions', 'provider_runs', \
              'agent_nodes', 'messages', 'events', 'approvals', 'routing_decisions', \
              'staged_provider_events', 'approval_questions')",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();
        let index_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'index' AND name IN \
             ('idx_conversations_status_updated', 'idx_conversations_workspace_updated', \
              'idx_runs_conversation_created', 'idx_agents_run_parent', \
              'idx_events_conversation_sequence', 'idx_approvals_conversation_pending', \
              'idx_approvals_conversation_history', \
              'idx_staged_provider_events_run_sequence')",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();

        assert_eq!(foreign_keys, 1);
        assert_eq!(journal_mode, "wal");
        assert_eq!(busy_timeout, 5_000);
        assert_eq!(table_count, 11);
        assert_eq!(index_count, 8);

        let workspace_columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('workspaces') ORDER BY cid")
                .fetch_all(&store.pool)
                .await
                .unwrap();
        assert!(workspace_columns.contains(&"worktree_base_commit".to_owned()));
        let duplicate_agent_page_index: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sqlite_schema \
             WHERE type = 'index' AND name = 'idx_agents_run_page'",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert_eq!(duplicate_agent_page_index, 0);
    }

    #[tokio::test]
    async fn conversation_titles_are_normalized_and_byte_bounded() {
        let store = Store::open_in_memory().await.unwrap();

        for title in ["   ".to_owned(), "x".repeat(257), "界".repeat(86)] {
            assert!(matches!(
                store
                    .create_conversation(NewConversation::projectless(title))
                    .await,
                Err(StoreError::InvalidData {
                    entity: "conversation title",
                    ..
                })
            ));
        }
        let normalized = store
            .create_conversation(NewConversation::projectless("  useful title  "))
            .await
            .unwrap();
        assert_eq!(normalized.title, "useful title");
        let multibyte = store
            .create_conversation(NewConversation::projectless("界".repeat(85)))
            .await
            .unwrap();
        assert_eq!(multibyte.title.len(), 255);

        let legacy_id = ConversationId::new();
        sqlx::query(
            "INSERT INTO conversations \
             (id, title, workspace_id, status, created_at, updated_at) \
             VALUES (?, ?, NULL, 'active', 1, 1)",
        )
        .bind(legacy_id.to_string())
        .bind("界".repeat(1_000))
        .execute(&store.pool)
        .await
        .unwrap();
        let legacy = store.load_conversation(legacy_id).await.unwrap();
        assert!(legacy.title.len() <= 256);
        assert!(legacy.title.is_char_boundary(legacy.title.len()));

        let blank_legacy_id = ConversationId::new();
        sqlx::query(
            "INSERT INTO conversations \
             (id, title, workspace_id, status, created_at, updated_at) \
             VALUES (?, '   ', NULL, 'active', 2, 2)",
        )
        .bind(blank_legacy_id.to_string())
        .execute(&store.pool)
        .await
        .unwrap();
        assert_eq!(
            store
                .load_conversation(blank_legacy_id)
                .await
                .unwrap()
                .title,
            "Untitled conversation"
        );
        let sidebar = store.list_conversations(None, 20).await.unwrap();
        assert_eq!(
            sidebar
                .items
                .iter()
                .find(|conversation| conversation.id == blank_legacy_id)
                .unwrap()
                .title,
            "Untitled conversation"
        );
    }

    #[tokio::test]
    async fn visible_run_creation_publishes_only_after_commit() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("run changes"))
            .await
            .unwrap();
        let mut changes = store.subscribe_changes();
        let (primary, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        assert_eq!(
            changes.recv().await.unwrap(),
            super::StoreChange {
                conversation_id: conversation.id,
                run_id: Some(primary.id),
            }
        );
        store
            .append_run_event(primary.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        let _ = changes.recv().await.unwrap();
        store
            .fail_run_if_active(
                primary.id,
                root.id,
                crate::providers::ProviderErrorCategory::Rejected,
                crate::domain::MutationState::NoneObserved,
                crate::providers::DispatchCertainty::NotDispatched,
            )
            .await
            .unwrap();
        let _ = changes.recv().await.unwrap();
        let (fallback, _) = store
            .create_fallback_run(primary.id, ProviderId::Claude)
            .await
            .unwrap();
        assert_eq!(
            changes.recv().await.unwrap(),
            super::StoreChange {
                conversation_id: conversation.id,
                run_id: Some(fallback.id),
            }
        );
    }

    #[tokio::test]
    async fn fresh_database_approval_pages_are_conversation_bounded_and_presorted() {
        let store = Store::open_in_memory().await.unwrap();
        for predicate in ["status = 'pending'", "status <> 'pending'"] {
            let plan = explain_query_plan(
                &store.pool,
                &format!(
                    "SELECT id FROM approvals WHERE conversation_id = 'fixture' \
                     AND {predicate} ORDER BY created_at DESC, id DESC LIMIT 201"
                ),
            )
            .await;
            assert!(
                plan.iter()
                    .any(|detail| detail.contains("idx_approvals_conversation_"))
            );
            assert!(!plan.iter().any(|detail| detail.contains("SCAN approvals")));
            assert!(!plan.iter().any(|detail| detail.contains("TEMP B-TREE")));
        }
    }

    #[tokio::test]
    async fn approval_pages_do_not_cross_conversations_at_scale() {
        let store = Store::open_in_memory().await.unwrap();
        let [
            (target_conversation, target_run, target_root),
            (other_conversation, other_run, other_root),
        ] = two_runs(&store).await;
        for index in 0..600 {
            let (conversation_id, run_id, agent_id) = if index % 2 == 0 {
                (target_conversation, target_run, target_root)
            } else {
                (other_conversation, other_run, other_root)
            };
            let (status, resolution) = if index % 4 < 2 {
                ("pending", None)
            } else {
                ("denied", Some("{\"kind\":\"denied\"}"))
            };
            sqlx::query(
                "INSERT INTO approvals \
                 (id, conversation_id, run_id, agent_id, provider, provider_request_id, \
                  operation, scope, status, resolution_json, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, 'codex', ?, 'command execution', 'current turn', \
                         ?, ?, ?, ?)",
            )
            .bind(ApprovalId::new().to_string())
            .bind(conversation_id.to_string())
            .bind(run_id.to_string())
            .bind(agent_id.to_string())
            .bind(format!("request-{index}"))
            .bind(status)
            .bind(resolution)
            .bind(index)
            .bind(index)
            .execute(&store.pool)
            .await
            .unwrap();
        }

        for pending in [true, false] {
            let mut cursor = None;
            let mut approvals = Vec::new();
            loop {
                let page = store
                    .load_approvals(target_conversation, cursor.take(), pending, 50)
                    .await
                    .unwrap();
                approvals.extend(page.items);
                let Some(next) = page.next_cursor else { break };
                cursor = Some(next);
            }
            assert_eq!(approvals.len(), 150);
            assert!(
                approvals
                    .iter()
                    .all(|approval| approval.run_id == target_run)
            );
        }
    }

    #[tokio::test]
    async fn fresh_database_recovery_batch_is_driven_by_the_depth_index_without_sorting() {
        let store = Store::open_in_memory().await.unwrap();
        let plan = explain_query_plan(
            &store.pool,
            "SELECT agent_nodes.run_id, agent_nodes.id, provider_runs.mutation_state, \
                    agent_nodes.parent_id IS NULL, agent_nodes.depth \
             FROM agent_nodes INDEXED BY idx_agents_recovery_depth \
             CROSS JOIN provider_runs ON provider_runs.id = agent_nodes.run_id \
             WHERE agent_nodes.status IN ('queued', 'running', 'waiting') \
               AND provider_runs.status IN ('queued', 'running', 'waiting') \
             ORDER BY agent_nodes.depth DESC, agent_nodes.created_at, agent_nodes.id LIMIT 200",
        )
        .await;

        assert!(
            plan.iter()
                .any(|detail| detail.contains("idx_agents_recovery_depth"))
        );
        assert!(!plan.iter().any(|detail| detail.contains("TEMP B-TREE")));
    }

    #[tokio::test]
    async fn committed_provider_events_publish_compact_invalidations() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("fixture"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        let mut changes = store.subscribe_changes();

        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();

        assert_eq!(
            changes.recv().await.unwrap(),
            super::StoreChange {
                conversation_id: conversation.id,
                run_id: Some(run.id),
            }
        );
    }

    #[tokio::test]
    async fn durable_create_submit_and_archive_publish_store_owned_post_commit_invalidations() {
        let store = Store::open_in_memory().await.unwrap();
        let mut changes = store.subscribe_changes();
        let conversation = store
            .create_conversation(NewConversation::projectless("post commit changes"))
            .await
            .unwrap();
        assert_eq!(
            changes.recv().await.unwrap(),
            super::StoreChange {
                conversation_id: conversation.id,
                run_id: None,
            }
        );

        let decision = crate::router::Router::default()
            .route(
                crate::router::RouteRequest::builder("submit")
                    .eligible([crate::router::ProviderRoutingState::available(
                        ProviderId::Codex,
                        crate::providers::ProviderCapabilities::default(),
                    )])
                    .override_provider(ProviderId::Codex)
                    .build(),
            )
            .unwrap();
        let PreparedSubmission::Created { run, .. } = store
            .prepare_submission(NewSubmission {
                command_id: "post-commit-submit".to_owned(),
                request_hash: "post-commit-hash".to_owned(),
                conversation_id: conversation.id,
                provider: ProviderId::Codex,
                content: "submit".to_owned(),
                routing_decision: decision,
                handoff_rendered: None,
                handoff_hash: None,
                turn_prompt: "submit".to_owned(),
            })
            .await
            .unwrap()
        else {
            panic!("first submit must create a run");
        };
        assert_eq!(
            changes.recv().await.unwrap(),
            super::StoreChange {
                conversation_id: conversation.id,
                run_id: Some(run.id),
            }
        );
        sqlx::query("UPDATE provider_runs SET status = 'completed' WHERE id = ?")
            .bind(run.id.to_string())
            .execute(&store.pool)
            .await
            .unwrap();
        store.archive_conversation(conversation.id).await.unwrap();
        assert_eq!(
            changes.recv().await.unwrap(),
            super::StoreChange {
                conversation_id: conversation.id,
                run_id: None,
            }
        );
    }

    #[tokio::test]
    async fn acknowledged_approval_publishes_after_the_visible_state_commits() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("fixture"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::approval_requested(
                    ProviderId::Codex,
                    "approval-1",
                    "run command",
                    "workspace",
                ),
            )
            .await
            .unwrap();
        store
            .record_response_intent(run.id, root.id, "approval-1", ApprovalResolution::Approved)
            .await
            .unwrap();
        let mut changes = store.subscribe_changes();

        store
            .acknowledge_response_intent(run.id, root.id, "approval-1")
            .await
            .unwrap();

        assert_eq!(
            changes.recv().await.unwrap(),
            super::StoreChange {
                conversation_id: conversation.id,
                run_id: Some(run.id),
            }
        );
        assert_eq!(
            store.load_run(run.id).await.unwrap().status,
            RunStatus::Running
        );
    }

    #[tokio::test]
    async fn recent_timeline_starts_at_the_newest_page_and_cursors_older() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("recent timeline"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        for content in ["one", "two", "three", "four"] {
            store
                .append_run_event(run.id, root.id, ProviderEventRecord::progress(content))
                .await
                .unwrap();
        }

        let newest = store
            .load_recent_timeline(conversation.id, None, 2)
            .await
            .unwrap();
        assert_eq!(
            newest
                .items
                .iter()
                .map(|record| record.event.content.as_str())
                .collect::<Vec<_>>(),
            ["three", "four"]
        );
        assert!(
            newest
                .items
                .windows(2)
                .all(|pair| { pair[0].event.sequence < pair[1].event.sequence })
        );

        let older = store
            .load_recent_timeline(conversation.id, newest.next_cursor, 2)
            .await
            .unwrap();
        assert_eq!(
            older
                .items
                .iter()
                .map(|record| record.event.content.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
        assert!(older.next_cursor.is_some());
    }

    #[tokio::test]
    async fn desktop_timeline_previews_and_event_detail_are_utf8_safe_and_byte_bounded() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("bounded timeline"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        let large = "🦀".repeat(super::MAX_EVENT_DETAIL_BYTES);
        let event = store
            .append_run_event(run.id, root.id, ProviderEventRecord::progress(&large))
            .await
            .unwrap();

        let page = store
            .load_recent_timeline(conversation.id, None, 200)
            .await
            .unwrap();
        assert!(
            page.items
                .iter()
                .map(|item| item.event.content.len())
                .sum::<usize>()
                <= super::MAX_TIMELINE_PAGE_CONTENT_BYTES
        );
        let preview = page
            .items
            .iter()
            .find(|item| item.event.id == event.id)
            .unwrap();
        assert!(preview.event.content.len() <= super::MAX_TIMELINE_PREVIEW_BYTES);
        assert!(preview.content_truncated);
        assert_eq!(preview.content_bytes, large.len());

        let detail = store.load_event_detail(event.id).await.unwrap();
        assert!(detail.content.len() <= super::MAX_EVENT_DETAIL_BYTES);
        assert!(detail.truncated);
        assert_eq!(detail.content_bytes, large.len());
        assert!(std::str::from_utf8(detail.content.as_bytes()).is_ok());
    }

    #[tokio::test]
    async fn submitted_user_and_assistant_messages_share_one_ordered_timeline_without_retry_duplicates()
     {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("canonical transcript"))
            .await
            .unwrap();
        let decision = crate::router::Router::default()
            .route(
                crate::router::RouteRequest::builder("hello")
                    .eligible([crate::router::ProviderRoutingState::available(
                        ProviderId::Codex,
                        crate::providers::ProviderCapabilities::default(),
                    )])
                    .override_provider(ProviderId::Codex)
                    .build(),
            )
            .unwrap();
        let submission = NewSubmission {
            command_id: "canonical-transcript-command".to_owned(),
            request_hash: "canonical-transcript-hash".to_owned(),
            conversation_id: conversation.id,
            provider: ProviderId::Codex,
            content: "hello".to_owned(),
            routing_decision: decision,
            handoff_rendered: None,
            handoff_hash: None,
            turn_prompt: "hello".to_owned(),
        };
        let PreparedSubmission::Created { run, root } =
            store.prepare_submission(submission.clone()).await.unwrap()
        else {
            panic!("first submission must create the run");
        };
        assert!(matches!(
            store.prepare_submission(submission).await.unwrap(),
            PreparedSubmission::Duplicate(existing) if existing.id == run.id
        ));
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::message("hello back"))
            .await
            .unwrap();

        let messages = store
            .load_recent_timeline(conversation.id, None, 20)
            .await
            .unwrap()
            .items
            .into_iter()
            .filter(|record| record.event.kind == TimelineEventKind::Message)
            .collect::<Vec<_>>();

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].event.content, "hello");
        assert_eq!(messages[0].event.role, Some(MessageRole::User));
        assert_eq!(messages[0].provider, ProviderId::Codex);
        assert_eq!(messages[1].event.content, "hello back");
        assert_eq!(messages[1].event.role, Some(MessageRole::Assistant));
        assert_eq!(messages[1].provider, ProviderId::Codex);
        assert!(messages[0].event.sequence < messages[1].event.sequence);
        let durable_user_message_id: String =
            sqlx::query_scalar("SELECT id FROM messages WHERE run_id = ? AND role = 'user'")
                .bind(run.id.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(messages[0].event.id.to_string(), durable_user_message_id);
    }

    #[tokio::test]
    async fn sidebar_details_preserve_recursive_agents_and_roll_up_attention() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("agent tree"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        let child = insert_child(&store, run.id, root.id, "running", "child").await;
        let grandchild = insert_child(&store, run.id, child, "waiting", "grandchild").await;

        let details = store
            .load_sidebar_details(&[conversation.id])
            .await
            .unwrap()
            .pop()
            .unwrap();

        assert_eq!(details.run.as_ref().map(|value| value.id), Some(run.id));
        assert_eq!(
            details.rollup_status,
            Some(crate::domain::RollupStatus::NeedsAttention)
        );
        assert_eq!(details.agents.len(), 1);
        assert_eq!(details.agents[0].id, root.id);
        assert!(details.agents_truncated);
        let page = store
            .load_agent_page(conversation.id, None, 20)
            .await
            .unwrap();
        assert_eq!(
            page.items
                .iter()
                .find(|item| item.agent.id == child)
                .and_then(|item| item.agent.parent_id),
            Some(root.id)
        );
        assert_eq!(
            page.items
                .iter()
                .find(|item| item.agent.id == grandchild)
                .and_then(|item| item.agent.parent_id),
            Some(child)
        );
        assert!(details.agents_truncated);
        assert_eq!(details.active_descendant_count, 2);
    }

    #[tokio::test]
    async fn agent_tree_pages_make_every_descendant_and_deep_ancestry_discoverable() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("large agent tree"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        for index in 0..225 {
            insert_child(&store, run.id, root.id, "queued", &format!("Agent {index}")).await;
        }
        let mut parent = root.id;
        for depth in 1..=40 {
            parent = insert_child(
                &store,
                run.id,
                parent,
                "queued",
                &format!("Deep agent {depth}"),
            )
            .await;
        }

        let recovery_batch = store.load_recovery_agent_batch(200).await.unwrap();
        assert_eq!(recovery_batch.len(), 200);
        assert!(
            recovery_batch
                .windows(2)
                .all(|pair| pair[0].depth >= pair[1].depth)
        );

        let first = store
            .load_agent_page(conversation.id, None, 100)
            .await
            .unwrap();
        assert_eq!(first.run_id, Some(run.id));
        let (newer_run, _) = store
            .create_run(conversation.id, ProviderId::Claude)
            .await
            .unwrap();
        let mut cursor = first.next_cursor;
        let mut agents = first.items;
        loop {
            let page = store
                .load_agent_page(conversation.id, cursor.take(), 100)
                .await
                .unwrap();
            assert_eq!(page.run_id, Some(run.id));
            assert_ne!(page.run_id, Some(newer_run.id));
            agents.extend(page.items);
            let Some(next) = page.next_cursor else { break };
            cursor = Some(next);
        }

        assert_eq!(agents.len(), 266);
        assert_eq!(agents.iter().map(|item| item.depth).max(), Some(40));
        assert!(agents.iter().any(|item| item.agent.id == parent));
    }

    #[tokio::test]
    async fn every_pending_approval_is_cursor_discoverable_without_loading_structured_payloads() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("many approvals"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        let native_secret = "provider-native-question-secret";
        let request = serde_json::json!({
            "questions": [{
                "id": native_secret,
                "header": "Header",
                "question": "x".repeat(100_000),
                "options": null,
                "isOther": false,
                "isSecret": false
            }],
            "autoResolutionMs": null
        })
        .to_string();
        let oversized_questions = (0..75)
            .map(|index| UserInputQuestion {
                id: format!("{native_secret}-{index}"),
                header: "Header".to_owned(),
                question: "🦀".repeat(2_000),
                options: Some(vec![crate::domain::UserInputOption {
                    label: "Continue".to_owned(),
                    description: "Use the bounded preview".to_owned(),
                }]),
                is_other: false,
                is_secret: false,
            })
            .collect::<Vec<_>>();
        store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::user_input_requested(
                    ProviderId::Codex,
                    "oversized-native-request",
                    oversized_questions,
                    None,
                ),
            )
            .await
            .unwrap();
        let oversized_approval_id: String = sqlx::query_scalar(
            "SELECT id FROM approvals WHERE provider_request_id = 'oversized-native-request'",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();
        let oversized_approval_id: ApprovalId = uuid::Uuid::parse_str(&oversized_approval_id)
            .unwrap()
            .into();
        for index in 0..224 {
            sqlx::query(
                "INSERT INTO approvals \
                 (id, conversation_id, run_id, agent_id, provider, provider_request_id, operation, scope, \
                  request_json, status, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, 'codex', ?, ?, ?, ?, 'pending', ?, ?)",
            )
            .bind(ApprovalId::new().to_string())
            .bind(conversation.id.to_string())
            .bind(run.id.to_string())
            .bind(root.id.to_string())
            .bind(format!("native-request-{index}"))
            .bind("🦀".repeat(4_000))
            .bind("界".repeat(4_000))
            .bind(&request)
            .bind(index)
            .bind(index)
            .execute(&store.pool)
            .await
            .unwrap();
        }

        let first = store
            .load_approvals(conversation.id, None, true, 200)
            .await
            .unwrap();
        assert_eq!(first.items.len(), 200);
        assert!(first.next_cursor.is_some());
        assert!(first.items.iter().all(|approval| {
            approval.operation.len() <= 256
                && approval.scope.len() <= 512
                && !approval.response_pending
        }));
        let second = store
            .load_approvals(conversation.id, first.next_cursor, true, 200)
            .await
            .unwrap();
        assert_eq!(second.items.len(), 25);
        assert!(second.next_cursor.is_none());

        let detail = store
            .load_approval_detail(oversized_approval_id)
            .await
            .unwrap();
        assert!(detail.truncated);
        assert!(detail.input.is_none());
        assert!(detail.details.is_none());
        assert_eq!(detail.status, crate::domain::ApprovalStatus::Pending);
        assert!(!detail.response_pending);
        assert_eq!(detail.agent_path.len(), 1);
        assert!(!detail.agent_path_truncated);
        assert_eq!(detail.question_count, 75);
        sqlx::query(
            "UPDATE approvals SET response_intent_json = '{\"kind\":\"approved\"}', \
             response_intent_status = 'recorded' WHERE id = ?",
        )
        .bind(oversized_approval_id.to_string())
        .execute(&store.pool)
        .await
        .unwrap();
        assert!(
            store
                .load_approval_detail(oversized_approval_id)
                .await
                .unwrap()
                .response_pending
        );
        let first_questions = store
            .load_approval_questions(oversized_approval_id, None, 50)
            .await
            .unwrap();
        assert_eq!(first_questions.items.len(), 50);
        assert_eq!(first_questions.total_count, 75);
        assert!(first_questions.items.iter().all(|question| {
            question.id.starts_with("question-")
                && question.question.len() <= super::MAX_APPROVAL_QUESTION_TEXT_BYTES
                && question.truncated
        }));
        assert_eq!(
            first_questions.items[0].options.as_ref().unwrap()[0].label,
            "Continue"
        );
        assert!(
            !serde_json::to_string(&first_questions.items)
                .unwrap()
                .contains(native_secret)
        );
        let second_questions = store
            .load_approval_questions(oversized_approval_id, first_questions.next_cursor, 50)
            .await
            .unwrap();
        assert_eq!(second_questions.items.len(), 25);
        assert!(second_questions.next_cursor.is_none());
    }

    #[tokio::test]
    async fn user_input_questions_are_normalized_once_for_indexed_paging() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("normalized questions"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        let questions = (0..125)
            .map(|index| UserInputQuestion {
                id: format!("provider-native-question-{index}"),
                header: format!("Header {index}"),
                question: format!("Question {index}"),
                options: Some(vec![crate::domain::UserInputOption {
                    label: "Yes".to_owned(),
                    description: "Continue".to_owned(),
                }]),
                is_other: false,
                is_secret: false,
            })
            .collect::<Vec<_>>();
        store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::user_input_requested(
                    ProviderId::Codex,
                    "provider-native-request",
                    questions,
                    None,
                ),
            )
            .await
            .unwrap();
        let approval_id: String = sqlx::query_scalar("SELECT id FROM approvals WHERE run_id = ?")
            .bind(run.id.to_string())
            .fetch_one(&store.pool)
            .await
            .unwrap();
        let approval_id: ApprovalId = uuid::Uuid::parse_str(&approval_id).unwrap().into();

        let normalized_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM approval_questions WHERE approval_id = ?")
                .bind(approval_id.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(normalized_count, 125);
        sqlx::query("UPDATE approvals SET request_json = '{\"questions\":[]}' WHERE id = ?")
            .bind(approval_id.to_string())
            .execute(&store.pool)
            .await
            .unwrap();

        let first = store
            .load_approval_questions(approval_id, None, 50)
            .await
            .unwrap();
        let second = store
            .load_approval_questions(approval_id, first.next_cursor, 50)
            .await
            .unwrap();
        assert_eq!(first.items.len(), 50);
        assert_eq!(second.items.len(), 50);
        assert_eq!(first.total_count, 125);
        assert_eq!(first.items[0].id, "question-1");
        assert_eq!(first.items[0].options.as_ref().unwrap()[0].label, "Yes");
        assert_eq!(second.items[0].id, "question-51");
        assert!(
            !serde_json::to_string(&first.items)
                .unwrap()
                .contains("provider-native")
        );
    }

    #[tokio::test]
    async fn approval_detail_returns_a_bounded_canonical_agent_path() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("deep approval"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        let mut leaf = root.id;
        for index in 0..MAX_APPROVAL_AGENT_PATH_NODES {
            leaf = insert_child(&store, run.id, leaf, "running", &format!("child-{index}")).await;
        }
        store
            .append_run_event(
                run.id,
                leaf,
                ProviderEventRecord::approval_requested(
                    ProviderId::Codex,
                    "deep-approval",
                    "edit",
                    "one file",
                ),
            )
            .await
            .unwrap();
        let approval_id: String = sqlx::query_scalar(
            "SELECT id FROM approvals WHERE provider_request_id = 'deep-approval'",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();
        let detail = store
            .load_approval_detail(uuid::Uuid::parse_str(&approval_id).unwrap().into())
            .await
            .unwrap();

        assert_eq!(detail.agent_path.len(), MAX_APPROVAL_AGENT_PATH_NODES);
        assert!(detail.agent_path_truncated);
        assert_eq!(detail.agent_path.first().unwrap(), "child-0");
        assert_eq!(detail.agent_path.last().unwrap(), "child-255");
    }

    #[tokio::test]
    async fn provider_run_audit_is_paged_scoped_and_keeps_exact_app_owned_handoff() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("run audit"))
            .await
            .unwrap();
        let other = store
            .create_conversation(NewConversation::projectless("other"))
            .await
            .unwrap();
        let (codex, _) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        let (claude, _) = store
            .create_run(conversation.id, ProviderId::Claude)
            .await
            .unwrap();
        let (unrelated, _) = store.create_run(other.id, ProviderId::Codex).await.unwrap();
        for (run, provider) in [
            (codex.id, ProviderId::Codex),
            (claude.id, ProviderId::Claude),
        ] {
            let decision = crate::router::Router::default()
                .route(
                    crate::router::RouteRequest::builder("audit fixture")
                        .eligible([
                            crate::router::ProviderRoutingState::available(
                                ProviderId::Codex,
                                crate::providers::ProviderCapabilities::default(),
                            ),
                            crate::router::ProviderRoutingState::available(
                                ProviderId::Claude,
                                crate::providers::ProviderCapabilities::default(),
                            ),
                        ])
                        .override_provider(provider)
                        .build(),
                )
                .unwrap();
            sqlx::query(
                "INSERT INTO routing_decisions \
                 (id, run_id, chosen_provider, details_json, reason, task_kind, created_at) \
                 VALUES (?, ?, ?, ?, 'manualOverride', 'general', 5)",
            )
            .bind(uuid::Uuid::now_v7().to_string())
            .bind(run.to_string())
            .bind(provider_label(provider))
            .bind(serde_json::to_string(&decision).unwrap())
            .execute(&store.pool)
            .await
            .unwrap();
        }
        sqlx::query("UPDATE provider_runs SET handoff_rendered = 'exact claude handoff', created_at = 5 WHERE id = ?")
            .bind(claude.id.to_string()).execute(&store.pool).await.unwrap();
        sqlx::query("UPDATE provider_runs SET created_at = 5 WHERE id IN (?, ?)")
            .bind(codex.id.to_string())
            .bind(unrelated.id.to_string())
            .execute(&store.pool)
            .await
            .unwrap();

        let first = store
            .load_run_audits(conversation.id, None, 1)
            .await
            .unwrap();
        assert_eq!(first.items.len(), 1);
        assert!(first.next_cursor.is_some());
        let second = store
            .load_run_audits(conversation.id, first.next_cursor, 1)
            .await
            .unwrap();
        assert_eq!(second.items.len(), 1);
        assert_ne!(first.items[0].id, second.items[0].id);
        assert!([codex.id, claude.id].contains(&first.items[0].id));
        assert!([codex.id, claude.id].contains(&second.items[0].id));
        assert_eq!(
            first.items[0].reason,
            Some(crate::router::RoutingReason::ManualOverride)
        );
        assert_eq!(
            second.items[0].reason,
            Some(crate::router::RoutingReason::ManualOverride)
        );

        let detail = store
            .load_run_audit(conversation.id, claude.id)
            .await
            .unwrap();
        assert_eq!(detail.handoff.as_deref(), Some("exact claude handoff"));
        assert!(!detail.handoff_truncated);
        assert_eq!(detail.routing.unwrap().provider, ProviderId::Claude);

        let oversized_handoff = "🦀".repeat(100_000);
        sqlx::query("UPDATE provider_runs SET handoff_rendered = ? WHERE id = ?")
            .bind(oversized_handoff)
            .bind(claude.id.to_string())
            .execute(&store.pool)
            .await
            .unwrap();
        let bounded = store
            .load_run_audit(conversation.id, claude.id)
            .await
            .unwrap();
        assert!(bounded.handoff_truncated);
        assert!(bounded.handoff.unwrap().len() <= MAX_RUN_AUDIT_HANDOFF_BYTES as usize);

        let mut oversized_routing = crate::router::Router::default()
            .route(
                crate::router::RouteRequest::builder("audit fixture")
                    .eligible([crate::router::ProviderRoutingState::available(
                        ProviderId::Claude,
                        crate::providers::ProviderCapabilities::default(),
                    )])
                    .override_provider(ProviderId::Claude)
                    .build(),
            )
            .unwrap();
        oversized_routing.explanation = "x".repeat(100_000);
        sqlx::query("UPDATE routing_decisions SET details_json = ?, reason = 'manualOverride' WHERE run_id = ?")
            .bind(serde_json::to_string(&oversized_routing).unwrap())
            .bind(claude.id.to_string())
            .execute(&store.pool)
            .await
            .unwrap();
        let bounded_page = store
            .load_run_audits(conversation.id, None, 2)
            .await
            .unwrap();
        let bounded_summary = bounded_page
            .items
            .iter()
            .find(|run| run.id == claude.id)
            .unwrap();
        assert!(bounded_summary.routing_truncated);
        assert_eq!(
            bounded_summary.reason,
            Some(crate::router::RoutingReason::ManualOverride)
        );
        let bounded_detail = store
            .load_run_audit(conversation.id, claude.id)
            .await
            .unwrap();
        assert!(bounded_detail.routing.is_none());
        assert!(bounded_detail.routing_truncated);
        assert_eq!(
            bounded_detail.reason,
            Some(crate::router::RoutingReason::ManualOverride)
        );
        assert!(matches!(
            store.load_run_audit(other.id, claude.id).await,
            Err(StoreError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn conversation_settings_are_bounded_on_write_and_legacy_read() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation_id = ConversationId::new();
        let workspace = Workspace {
            id: WorkspaceId::new(),
            conversation_id,
            project_root: None,
            execution_path: PathBuf::from("/tmp/prompting-time-settings-fixture"),
            owned_worktree: false,
            worktree_base_commit: None,
        };
        let settings = ConversationSettings {
            objective: "x".repeat(MAX_OBJECTIVE_BYTES + 1),
            constraints: Vec::new(),
            routing_profile: crate::router::RoutingProfile::Balanced,
        };

        let write_error = store
            .create_configured_conversation(
                conversation_id,
                "oversized settings".to_owned(),
                &workspace,
                &settings,
            )
            .await
            .unwrap_err();
        assert!(matches!(write_error, StoreError::InvalidData { .. }));

        let legacy = store
            .create_conversation(NewConversation::projectless("legacy settings"))
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO conversation_settings \
             (conversation_id, objective, constraints_json, routing_profile) \
             VALUES (?, ?, '[]', 'balanced')",
        )
        .bind(legacy.id.to_string())
        .bind("x".repeat(MAX_OBJECTIVE_BYTES + 1))
        .execute(&store.pool)
        .await
        .unwrap();

        let read_error = store
            .load_conversation_settings(legacy.id)
            .await
            .unwrap_err();
        assert!(matches!(read_error, StoreError::InvalidData { .. }));
    }

    #[tokio::test]
    async fn draining_staged_native_message_deltas_persists_one_canonical_message() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("staged assistant"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store.bind_native_session(run.id, "session").await.unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::approval_requested(
                    ProviderId::Codex,
                    "approval",
                    "write",
                    "fixture",
                ),
            )
            .await
            .unwrap();
        for content in ["aggregated ", "answer"] {
            store
                .stage_waiting_event(
                    run.id,
                    root.id,
                    ProviderEventRecord::native_message(content, "message"),
                )
                .await
                .unwrap();
        }

        store
            .append_run_event(run.id, root.id, ProviderEventRecord::interrupted())
            .await
            .unwrap();

        let messages: Vec<(i64, String)> = sqlx::query_as(
            "SELECT sequence, content FROM messages WHERE run_id = ? AND role = 'assistant'",
        )
        .bind(run.id.to_string())
        .fetch_all(&store.pool)
        .await
        .unwrap();
        assert_eq!(
            messages,
            vec![(messages[0].0, "aggregated answer".to_owned())]
        );
        assert_eq!(
            store
                .provider_context_boundary(conversation.id, ProviderId::Codex)
                .await
                .unwrap(),
            u64::try_from(messages[0].0).unwrap()
        );
    }

    #[tokio::test]
    async fn fallback_insertion_rejects_an_archived_conversation() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("archived fallback"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::provider_failed(
                    crate::providers::ProviderErrorCategory::Rejected,
                    crate::domain::MutationState::NoneObserved,
                    crate::providers::DispatchCertainty::NotDispatched,
                ),
            )
            .await
            .unwrap();
        store.archive_conversation(conversation.id).await.unwrap();

        let error = store
            .create_fallback_run(run.id, ProviderId::Claude)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            StoreError::ConversationArchived(id) if id == conversation.id
        ));
    }

    #[tokio::test]
    async fn legacy_fallback_creation_rejects_application_managed_runs() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("managed fallback"))
            .await
            .unwrap();
        let decision = crate::router::Router::default()
            .route(
                crate::router::RouteRequest::builder("fixture")
                    .eligible([crate::router::ProviderRoutingState::available(
                        ProviderId::Codex,
                        crate::providers::ProviderCapabilities::default(),
                    )])
                    .override_provider(ProviderId::Codex)
                    .build(),
            )
            .unwrap();
        let PreparedSubmission::Created { run, root } = store
            .prepare_submission(NewSubmission {
                command_id: "managed-fallback".to_owned(),
                request_hash: "managed-fallback-hash".to_owned(),
                conversation_id: conversation.id,
                provider: ProviderId::Codex,
                content: "fixture".to_owned(),
                routing_decision: decision,
                handoff_rendered: None,
                handoff_hash: None,
                turn_prompt: "fixture".to_owned(),
            })
            .await
            .unwrap()
        else {
            panic!("new command must create a run");
        };
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::provider_failed(
                    crate::providers::ProviderErrorCategory::Rejected,
                    crate::domain::MutationState::NoneObserved,
                    crate::providers::DispatchCertainty::NotDispatched,
                ),
            )
            .await
            .unwrap();

        let error = store
            .create_fallback_run(run.id, ProviderId::Claude)
            .await
            .unwrap_err();

        assert!(matches!(error, StoreError::UnsafeFallbackState));
    }

    #[tokio::test]
    async fn atomic_fallback_retry_returns_the_same_durable_attempt() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("atomic fallback"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        let fallback = NewFallbackAttempt {
            provider: ProviderId::Claude,
            native_session_id: None,
            turn_prompt: "provider-specific prompt".to_owned(),
            handoff_rendered: Some("bounded handoff".to_owned()),
            handoff_hash: Some("stable-hash".to_owned()),
            routing_decision: None,
        };

        let first = store
            .fail_and_create_fallback(
                run.id,
                root.id,
                crate::providers::ProviderErrorCategory::Rejected,
                fallback.clone(),
            )
            .await
            .unwrap();
        let retry = store
            .fail_and_create_fallback(
                run.id,
                root.id,
                crate::providers::ProviderErrorCategory::Rejected,
                fallback,
            )
            .await
            .unwrap();

        assert_eq!(first.0.id, retry.0.id);
        assert_eq!(first.1.id, retry.1.id);
        assert_eq!(
            store.load_run(run.id).await.unwrap().status,
            RunStatus::Failed
        );
        assert_eq!(retry.0.status, RunStatus::Queued);
        let failure_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE run_id = ? AND kind = 'diagnostic'",
        )
        .bind(run.id.to_string())
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert_eq!(failure_events, 1);
        let prompt: String =
            sqlx::query_scalar("SELECT turn_prompt FROM provider_runs WHERE id = ?")
                .bind(first.0.id.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(prompt, "provider-specific prompt");
        let recovery = store.pending_recovery().await.unwrap();
        assert_eq!(recovery.len(), 1);
        assert_eq!(recovery[0].run.id, first.0.id);
        assert_eq!(
            recovery[0].attempt_intent.as_ref().unwrap().turn_prompt,
            "provider-specific prompt"
        );
        assert_eq!(
            recovery[0]
                .attempt_intent
                .as_ref()
                .unwrap()
                .handoff_hash
                .as_deref(),
            Some("stable-hash")
        );
    }

    #[tokio::test]
    async fn atomic_fallback_retry_rejects_a_different_prepared_intent() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("fallback conflict"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        let fallback = NewFallbackAttempt {
            provider: ProviderId::Claude,
            native_session_id: None,
            turn_prompt: "first prompt".to_owned(),
            handoff_rendered: Some("handoff".to_owned()),
            handoff_hash: Some("hash".to_owned()),
            routing_decision: None,
        };
        store
            .fail_and_create_fallback(
                run.id,
                root.id,
                crate::providers::ProviderErrorCategory::Rejected,
                fallback.clone(),
            )
            .await
            .unwrap();
        let error = store
            .fail_and_create_fallback(
                run.id,
                root.id,
                crate::providers::ProviderErrorCategory::Rejected,
                NewFallbackAttempt {
                    turn_prompt: "different prompt".to_owned(),
                    ..fallback
                },
            )
            .await
            .unwrap_err();

        assert!(matches!(error, StoreError::FallbackIntentConflict));
        let fallback_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM provider_runs WHERE fallback_from_run_id = ?")
                .bind(run.id.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(fallback_count, 1);
    }

    #[tokio::test]
    async fn provider_child_activity_materializes_a_recursive_canonical_tree() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("native child tree"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .bind_native_session(run.id, "root-native")
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();

        let root_event = store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::child_agent(
                    "spawn-a",
                    "root-native",
                    vec!["child-a".to_owned()],
                    vec![NativeChildStatus {
                        native_thread_id: "child-a".to_owned(),
                        status: NativeAgentStatus::Running,
                    }],
                    "spawn",
                    "running",
                ),
            )
            .await
            .unwrap();
        assert_eq!(root_event.agent_id, root.id);

        let nested_event = store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::child_agent(
                    "spawn-b",
                    "child-a",
                    vec!["child-b".to_owned()],
                    vec![NativeChildStatus {
                        native_thread_id: "child-b".to_owned(),
                        status: NativeAgentStatus::Running,
                    }],
                    "spawn",
                    "running",
                ),
            )
            .await
            .unwrap();
        let subagent_event = store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::sub_agent(
                    "activity-b",
                    "child-b",
                    "researcher/child-b",
                    NativeSubAgentActivityKind::Interacted,
                ),
            )
            .await
            .unwrap();

        let rows: Vec<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
            "SELECT provider_native_id, parent_id, provider_native_path, status \
             FROM agent_nodes WHERE run_id = ? ORDER BY created_at, id",
        )
        .bind(run.id.to_string())
        .fetch_all(&store.pool)
        .await
        .unwrap();
        let root_id = root.id.to_string();
        let child_a = rows.iter().find(|row| row.0 == "child-a").unwrap();
        let child_a_id: String = sqlx::query_scalar(
            "SELECT id FROM agent_nodes WHERE run_id = ? AND provider_native_id = 'child-a'",
        )
        .bind(run.id.to_string())
        .fetch_one(&store.pool)
        .await
        .unwrap();
        let child_b = rows.iter().find(|row| row.0 == "child-b").unwrap();
        assert_eq!(child_a.1.as_deref(), Some(root_id.as_str()));
        assert_eq!(child_b.1.as_deref(), Some(child_a_id.as_str()));
        assert_eq!(child_b.2.as_deref(), Some("researcher/child-b"));
        assert_eq!(nested_event.agent_id.to_string(), child_a_id);
        assert_eq!(subagent_event.agent_id.to_string(), {
            sqlx::query_scalar::<_, String>(
                "SELECT id FROM agent_nodes WHERE run_id = ? AND provider_native_id = 'child-b'",
            )
            .bind(run.id.to_string())
            .fetch_one(&store.pool)
            .await
            .unwrap()
        });
    }

    #[tokio::test]
    async fn provider_child_activity_materializes_receivers_without_reported_status() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("partial native child states"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .bind_native_session(run.id, "root-native")
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();

        store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::child_agent(
                    "spawn-partial",
                    "root-native",
                    vec!["reported".to_owned(), "pending".to_owned()],
                    vec![NativeChildStatus {
                        native_thread_id: "reported".to_owned(),
                        status: NativeAgentStatus::Running,
                    }],
                    "spawn",
                    "running",
                ),
            )
            .await
            .unwrap();

        let statuses: Vec<(String, String)> = sqlx::query_as(
            "SELECT provider_native_id, status FROM agent_nodes \
             WHERE run_id = ? AND parent_id IS NOT NULL ORDER BY provider_native_id",
        )
        .bind(run.id.to_string())
        .fetch_all(&store.pool)
        .await
        .unwrap();
        assert_eq!(
            statuses,
            vec![
                ("pending".to_owned(), "queued".to_owned()),
                ("reported".to_owned(), "running".to_owned()),
            ]
        );
    }

    #[tokio::test]
    async fn child_agent_identity_requires_a_bound_parent_and_validates_unreported_receivers() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("bound child parent"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        let unknown_parent = store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::child_agent(
                    "spawn",
                    "unbound-root",
                    vec!["child".to_owned()],
                    Vec::new(),
                    "spawn",
                    "queued",
                ),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            unknown_parent,
            StoreError::NativeAgentIdentityConflict
        ));

        let second_conversation = store
            .create_conversation(NewConversation::projectless("invalid child identity"))
            .await
            .unwrap();
        let (second_run, second_root) = store
            .create_run(second_conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .bind_native_session(second_run.id, "root")
            .await
            .unwrap();
        store
            .append_run_event(
                second_run.id,
                second_root.id,
                ProviderEventRecord::started(),
            )
            .await
            .unwrap();
        let oversized_receiver = store
            .append_run_event(
                second_run.id,
                second_root.id,
                ProviderEventRecord::child_agent(
                    "spawn",
                    "root",
                    vec!["x".repeat(MAX_NATIVE_AGENT_ID_BYTES + 1)],
                    Vec::new(),
                    "spawn",
                    "queued",
                ),
            )
            .await
            .unwrap_err();
        assert!(matches!(oversized_receiver, StoreError::InvalidData { .. }));
    }

    #[tokio::test]
    async fn binding_a_native_session_never_overwrites_root_identity() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("stable root identity"))
            .await
            .unwrap();
        let (run, _) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store.bind_native_session(run.id, "root-a").await.unwrap();

        let error = store
            .bind_native_session(run.id, "root-b")
            .await
            .unwrap_err();

        assert!(matches!(error, StoreError::NativeAgentIdentityConflict));
        assert_eq!(
            store
                .load_provider_session(conversation.id, ProviderId::Codex)
                .await
                .unwrap()
                .unwrap()
                .native_id,
            "root-a"
        );
    }

    #[tokio::test]
    async fn provider_child_identity_rejects_reparenting_path_changes_and_terminal_regression() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("native child conflicts"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .bind_native_session(run.id, "root-native")
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::child_agent(
                    "spawn",
                    "root-native",
                    vec!["child".to_owned()],
                    vec![NativeChildStatus {
                        native_thread_id: "child".to_owned(),
                        status: NativeAgentStatus::Running,
                    }],
                    "spawn",
                    "running",
                ),
            )
            .await
            .unwrap();
        store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::sub_agent(
                    "activity",
                    "child",
                    "stable/path",
                    NativeSubAgentActivityKind::Interacted,
                ),
            )
            .await
            .unwrap();

        let path_error = store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::sub_agent(
                    "activity-2",
                    "child",
                    "different/path",
                    NativeSubAgentActivityKind::Interacted,
                ),
            )
            .await
            .unwrap_err();
        let reparent_error = store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::child_agent(
                    "reparent",
                    "child",
                    vec!["root-native".to_owned()],
                    vec![NativeChildStatus {
                        native_thread_id: "root-native".to_owned(),
                        status: NativeAgentStatus::Completed,
                    }],
                    "reparent",
                    "completed",
                ),
            )
            .await
            .unwrap_err();
        store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::child_agent(
                    "complete",
                    "root-native",
                    vec!["child".to_owned()],
                    vec![NativeChildStatus {
                        native_thread_id: "child".to_owned(),
                        status: NativeAgentStatus::Completed,
                    }],
                    "complete",
                    "completed",
                ),
            )
            .await
            .unwrap();
        let regression_error = store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::child_agent(
                    "regress",
                    "root-native",
                    vec!["child".to_owned()],
                    vec![NativeChildStatus {
                        native_thread_id: "child".to_owned(),
                        status: NativeAgentStatus::Running,
                    }],
                    "regress",
                    "running",
                ),
            )
            .await
            .unwrap_err();

        assert!(matches!(
            path_error,
            StoreError::NativeAgentIdentityConflict
        ));
        assert!(matches!(
            reparent_error,
            StoreError::NativeAgentIdentityConflict
        ));
        assert!(matches!(
            regression_error,
            StoreError::NativeAgentIdentityConflict
        ));
    }

    #[tokio::test]
    async fn unseen_message_loading_is_character_budgeted_not_row_truncated() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("message budget"))
            .await
            .unwrap();
        for index in 0..250 {
            sqlx::query(
                "INSERT INTO messages \
                 (id, conversation_id, run_id, role, content, created_at) \
                 VALUES (?, ?, NULL, 'user', ?, ?)",
            )
            .bind(crate::domain::MessageId::new().to_string())
            .bind(conversation.id.to_string())
            .bind(format!("message-{index}"))
            .bind(index)
            .execute(&store.pool)
            .await
            .unwrap();
        }

        let roomy = store
            .load_messages_after(conversation.id, 0, 10_000)
            .await
            .unwrap();
        let tight = store
            .load_messages_after(conversation.id, 0, 60)
            .await
            .unwrap();

        assert_eq!(roomy.len(), 250);
        assert!(tight.len() < roomy.len());
        assert_eq!(tight.last().unwrap().content, "message-249");
    }

    #[tokio::test]
    async fn unseen_message_loading_bounds_rows_and_stops_at_an_oversized_newest_row() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("bounded message source"))
            .await
            .unwrap();
        for index in 0..2_050 {
            sqlx::query(
                "INSERT INTO messages \
                 (id, conversation_id, run_id, role, content, created_at) \
                 VALUES (?, ?, NULL, 'user', ?, ?)",
            )
            .bind(crate::domain::MessageId::new().to_string())
            .bind(conversation.id.to_string())
            .bind(format!("m{index}"))
            .bind(index)
            .execute(&store.pool)
            .await
            .unwrap();
        }

        let bounded = store
            .load_messages_after(conversation.id, 0, 100_000)
            .await
            .unwrap();
        assert_eq!(bounded.len(), 2_048);
        assert_eq!(bounded.first().unwrap().content, "m2");

        sqlx::query(
            "INSERT INTO messages \
             (id, conversation_id, run_id, role, content, created_at) \
             VALUES (?, ?, NULL, 'assistant', ?, ?)",
        )
        .bind(crate::domain::MessageId::new().to_string())
        .bind(conversation.id.to_string())
        .bind("x".repeat(200_000))
        .bind(3_000)
        .execute(&store.pool)
        .await
        .unwrap();

        let after_oversized = store
            .load_messages_after(conversation.id, 0, 100_000)
            .await
            .unwrap();
        assert!(after_oversized.is_empty());
    }

    #[tokio::test]
    async fn oversized_user_submission_is_rejected_before_persistence() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("oversized user message"))
            .await
            .unwrap();
        let decision = crate::router::Router::default()
            .route(
                crate::router::RouteRequest::builder("fixture")
                    .eligible([crate::router::ProviderRoutingState::available(
                        ProviderId::Codex,
                        crate::providers::ProviderCapabilities::default(),
                    )])
                    .override_provider(ProviderId::Codex)
                    .build(),
            )
            .unwrap();

        let error = store
            .prepare_submission(NewSubmission {
                command_id: "oversized".to_owned(),
                request_hash: "hash".to_owned(),
                conversation_id: conversation.id,
                provider: ProviderId::Codex,
                content: "x".repeat(MAX_CANONICAL_MESSAGE_BYTES + 1),
                routing_decision: decision,
                handoff_rendered: None,
                handoff_hash: None,
                turn_prompt: "fixture".to_owned(),
            })
            .await
            .unwrap_err();

        assert!(matches!(error, StoreError::MessageTooLarge { .. }));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn assistant_delta_aggregation_cannot_exceed_the_message_bound() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("oversized assistant message"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store.bind_native_session(run.id, "session").await.unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::native_message(
                    "x".repeat(MAX_CANONICAL_MESSAGE_BYTES - 1),
                    "message",
                ),
            )
            .await
            .unwrap();

        let error = store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::native_message("yz", "message"),
            )
            .await
            .unwrap_err();

        assert!(matches!(error, StoreError::MessageTooLarge { .. }));
        let length: i64 = sqlx::query_scalar(
            "SELECT length(CAST(content AS BLOB)) FROM messages WHERE run_id = ?",
        )
        .bind(run.id.to_string())
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert_eq!(
            length,
            i64::try_from(MAX_CANONICAL_MESSAGE_BYTES - 1).unwrap()
        );
    }

    #[tokio::test]
    async fn child_handoff_source_is_row_and_field_bounded() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("bounded child source"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        for index in 0..35 {
            sqlx::query(
                "INSERT INTO agent_nodes \
                 (id, run_id, parent_id, provider, provider_native_id, label, summary, status, \
                  created_at, updated_at) \
                 VALUES (?, ?, ?, 'codex', ?, 'child', ?, 'completed', ?, ?)",
            )
            .bind(AgentId::new().to_string())
            .bind(run.id.to_string())
            .bind(root.id.to_string())
            .bind(format!("child-{index}"))
            .bind(format!("summary-{index}"))
            .bind(index)
            .bind(index)
            .execute(&store.pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO agent_nodes \
             (id, run_id, parent_id, provider, provider_native_id, label, summary, status, \
              created_at, updated_at) \
             VALUES (?, ?, ?, 'codex', 'oversized', 'child', ?, 'completed', 100, 100)",
        )
        .bind(AgentId::new().to_string())
        .bind(run.id.to_string())
        .bind(root.id.to_string())
        .bind("s".repeat(10_000))
        .execute(&store.pool)
        .await
        .unwrap();

        let outcomes = store
            .load_child_agent_outcomes(conversation.id)
            .await
            .unwrap();

        assert_eq!(outcomes.len(), 32);
        assert!(
            outcomes
                .iter()
                .all(|outcome| outcome.provider_native_id != "oversized")
        );
        assert_eq!(outcomes.first().unwrap().provider_native_id, "child-3");
        assert_eq!(outcomes.last().unwrap().provider_native_id, "child-34");
    }

    #[tokio::test]
    async fn routing_handoff_source_rejects_an_oversized_typed_field() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("bounded decision source"))
            .await
            .unwrap();
        let (run, _) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO routing_decisions \
             (id, run_id, chosen_provider, details_json, reason, task_kind, created_at) \
             VALUES (?, ?, 'codex', '{}', ?, 'general', 1)",
        )
        .bind(uuid::Uuid::now_v7().to_string())
        .bind(run.id.to_string())
        .bind("r".repeat(10_000))
        .execute(&store.pool)
        .await
        .unwrap();

        assert!(
            store
                .load_routing_decisions(conversation.id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn migration_preserves_v2_approval_states_through_v3_v4_v5_and_v6() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        MIGRATOR.run_to(2, &pool).await.unwrap();
        let conversation_id = ConversationId::new();
        let run_id = RunId::new();
        let agent_id = AgentId::new();
        sqlx::query(
            "INSERT INTO conversations \
             (id, title, workspace_id, status, created_at, updated_at) \
             VALUES (?, 'migration fixture', NULL, 'active', 1, 1)",
        )
        .bind(conversation_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO provider_runs \
             (id, conversation_id, provider, native_session_id, status, mutation_state, \
              created_at, updated_at) \
             VALUES (?, ?, 'codex', NULL, 'waiting', 'none_observed', 1, 1)",
        )
        .bind(run_id.to_string())
        .bind(conversation_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agent_nodes \
             (id, run_id, parent_id, provider, label, status, created_at, updated_at) \
             VALUES (?, ?, NULL, 'codex', 'orchestrator', 'waiting', 1, 1)",
        )
        .bind(agent_id.to_string())
        .bind(run_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
        for (request_id, status, decision) in [
            ("pending-v2", "pending", None),
            ("approved-v2", "approved", Some("approved")),
            ("denied-v2", "denied", Some("denied")),
        ] {
            sqlx::query(
                "INSERT INTO approvals \
                 (id, run_id, agent_id, provider, provider_request_id, operation, scope, \
                  status, decision, created_at, updated_at) \
                 VALUES (?, ?, ?, 'codex', ?, 'write', 'fixture.txt', ?, ?, 1, 1)",
            )
            .bind(ApprovalId::new().to_string())
            .bind(run_id.to_string())
            .bind(agent_id.to_string())
            .bind(request_id)
            .bind(status)
            .bind(decision)
            .execute(&pool)
            .await
            .unwrap();
        }

        MIGRATOR.run_to(3, &pool).await.unwrap();
        MIGRATOR.run(&pool).await.unwrap();
        let (changes, _) = broadcast::channel(STORE_CHANGE_CHANNEL_CAPACITY);
        let store = Store { pool, changes };

        let pending = store.load_approval(run_id, "pending-v2").await.unwrap();
        assert_eq!(pending.status, crate::domain::ApprovalStatus::Pending);
        assert_eq!(pending.resolution, None);
        assert_eq!(pending.response_intent, None);
        for (request_id, status, resolution) in [
            (
                "approved-v2",
                crate::domain::ApprovalStatus::Approved,
                crate::domain::ApprovalResolution::Approved,
            ),
            (
                "denied-v2",
                crate::domain::ApprovalStatus::Denied,
                crate::domain::ApprovalResolution::Denied,
            ),
        ] {
            let approval = store.load_approval(run_id, request_id).await.unwrap();
            assert_eq!(approval.status, status);
            assert_eq!(approval.resolution, Some(resolution));
            assert_eq!(approval.response_intent, None);
        }
        assert_eq!(
            store.load_run(run_id).await.unwrap().dispatch_certainty,
            None
        );
        let staged_table_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_schema \
             WHERE type = 'table' AND name = 'staged_provider_events'",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert_eq!(staged_table_count, 1);
    }

    #[tokio::test]
    async fn migration_backfills_historical_user_turns_into_canonical_timeline_order() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        MIGRATOR.run_to(12, &pool).await.unwrap();
        let conversation_id = ConversationId::new();
        let run_id = RunId::new();
        let agent_id = AgentId::new();
        sqlx::query(
            "INSERT INTO conversations \
             (id, title, workspace_id, status, created_at, updated_at) \
             VALUES (?, 'upgrade fixture', NULL, 'active', 1, 3)",
        )
        .bind(conversation_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO provider_runs \
             (id, conversation_id, provider, status, mutation_state, application_managed, \
              created_at, updated_at) \
             VALUES (?, ?, 'codex', 'completed', 'none_observed', 1, 1, 3)",
        )
        .bind(run_id.to_string())
        .bind(conversation_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agent_nodes \
             (id, run_id, parent_id, provider, label, status, created_at, updated_at) \
             VALUES (?, ?, NULL, 'codex', 'orchestrator', 'completed', 1, 3)",
        )
        .bind(agent_id.to_string())
        .bind(run_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
        let user_message_id = MessageId::new();
        sqlx::query(
            "INSERT INTO messages \
             (id, conversation_id, run_id, role, content, created_at) \
             VALUES (?, ?, ?, 'user', 'historical question', 2)",
        )
        .bind(user_message_id.to_string())
        .bind(conversation_id.to_string())
        .bind(run_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO messages \
             (id, conversation_id, run_id, role, content, created_at) \
             VALUES (?, ?, ?, 'assistant', 'historical answer', 3)",
        )
        .bind(MessageId::new().to_string())
        .bind(conversation_id.to_string())
        .bind(run_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO events \
             (id, conversation_id, run_id, agent_id, kind, content, created_at) \
             VALUES (?, ?, ?, ?, 'message', 'historical answer', 3)",
        )
        .bind(TimelineEventId::new().to_string())
        .bind(conversation_id.to_string())
        .bind(run_id.to_string())
        .bind(agent_id.to_string())
        .execute(&pool)
        .await
        .unwrap();

        MIGRATOR.run_to(13, &pool).await.unwrap();
        sqlx::query(
            "INSERT INTO events \
             (id, conversation_id, run_id, agent_id, kind, role, content, created_at) \
             VALUES (?, ?, ?, ?, 'message', 'user', 'historical question', 2)",
        )
        .bind(TimelineEventId::new().to_string())
        .bind(conversation_id.to_string())
        .bind(run_id.to_string())
        .bind(agent_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
        MIGRATOR.run(&pool).await.unwrap();
        let (changes, _) = broadcast::channel(STORE_CHANGE_CHANNEL_CAPACITY);
        let store = Store { pool, changes };
        let timeline = store
            .load_recent_timeline(conversation_id, None, 20)
            .await
            .unwrap();

        assert_eq!(
            timeline
                .items
                .iter()
                .map(|item| (item.event.role, item.event.content.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (Some(MessageRole::User), "historical question"),
                (Some(MessageRole::Assistant), "historical answer"),
            ]
        );
        let user_events: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE id = ? AND role = 'user'")
                .bind(user_message_id.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(user_events, 1);
    }

    #[tokio::test]
    async fn migration_bounds_oversized_preexisting_staged_evidence() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        MIGRATOR.run_to(5, &pool).await.unwrap();
        let conversation_id = ConversationId::new();
        let run_id = RunId::new();
        let agent_id = AgentId::new();
        sqlx::query(
            "INSERT INTO conversations \
             (id, title, workspace_id, status, created_at, updated_at) \
             VALUES (?, 'bounded migration fixture', NULL, 'active', 1, 1)",
        )
        .bind(conversation_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO provider_runs \
             (id, conversation_id, provider, native_session_id, status, mutation_state, \
              dispatch_certainty, created_at, updated_at) \
             VALUES (?, ?, 'codex', NULL, 'waiting', 'none_observed', NULL, 1, 1)",
        )
        .bind(run_id.to_string())
        .bind(conversation_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agent_nodes \
             (id, run_id, parent_id, provider, label, status, created_at, updated_at) \
             VALUES (?, ?, NULL, 'codex', 'orchestrator', 'waiting', 1, 1)",
        )
        .bind(agent_id.to_string())
        .bind(run_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
        let huge = "x".repeat(MAX_STAGED_EVENT_BYTES + 1);
        for index in 0..MAX_STAGED_EVENT_ROWS + 44 {
            let content = if index == 5 {
                huge.as_str()
            } else {
                "preexisting"
            };
            sqlx::query(
                "INSERT INTO staged_provider_events \
                 (id, conversation_id, run_id, agent_id, kind, content, mutation_state, created_at) \
                 VALUES (?, ?, ?, ?, 'progress', ?, NULL, 1)",
            )
            .bind(TimelineEventId::new().to_string())
            .bind(conversation_id.to_string())
            .bind(run_id.to_string())
            .bind(agent_id.to_string())
            .bind(content)
            .execute(&pool)
            .await
            .unwrap();
        }

        MIGRATOR.run(&pool).await.unwrap();
        let (rows, bytes, markers): (i64, i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), SUM(length(CAST(content AS BLOB))), \
                    SUM(CASE WHEN overflowed_kind IS NULL THEN 0 ELSE 1 END) \
             FROM staged_provider_events WHERE run_id = ?",
        )
        .bind(run_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(rows <= i64::try_from(MAX_STAGED_EVENT_ROWS).unwrap());
        assert!(bytes <= i64::try_from(MAX_STAGED_EVENT_BYTES).unwrap());
        assert_eq!(markers, 1);
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT content FROM staged_provider_events WHERE overflowed_kind IS NOT NULL",
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            STAGED_OVERFLOW_CONTENT
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT mutation_state FROM provider_runs WHERE id = ?",
            )
            .bind(run_id.to_string())
            .fetch_one(&pool)
            .await
            .unwrap(),
            "unknown"
        );
    }

    #[tokio::test]
    async fn migration_backfills_root_native_identity_from_the_bound_run_session() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        MIGRATOR.run_to(11, &pool).await.unwrap();
        let conversation_id = ConversationId::new();
        let run_id = RunId::new();
        let agent_id = AgentId::new();
        sqlx::query(
            "INSERT INTO conversations \
             (id, title, workspace_id, status, created_at, updated_at) \
             VALUES (?, 'native identity migration', NULL, 'active', 1, 1)",
        )
        .bind(conversation_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO provider_runs \
             (id, conversation_id, provider, native_session_id, status, mutation_state, \
              application_managed, created_at, updated_at) \
             VALUES (?, ?, 'codex', 'native-root', 'queued', 'none_observed', 0, 1, 1)",
        )
        .bind(run_id.to_string())
        .bind(conversation_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agent_nodes \
             (id, run_id, parent_id, provider, label, status, created_at, updated_at) \
             VALUES (?, ?, NULL, 'codex', 'orchestrator', 'queued', 1, 1)",
        )
        .bind(agent_id.to_string())
        .bind(run_id.to_string())
        .execute(&pool)
        .await
        .unwrap();

        MIGRATOR.run(&pool).await.unwrap();

        let native_id: Option<String> =
            sqlx::query_scalar("SELECT provider_native_id FROM agent_nodes WHERE id = ?")
                .bind(agent_id.to_string())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(native_id.as_deref(), Some("native-root"));
    }

    #[tokio::test]
    async fn corrupt_staged_queue_is_recovered_bounded_and_acknowledgement_rolls_back() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("corrupt staged queue"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::approval_requested(
                    ProviderId::Codex,
                    "bounded-ack",
                    "write",
                    "fixture.txt",
                ),
            )
            .await
            .unwrap();
        store
            .record_response_intent(
                run.id,
                root.id,
                "bounded-ack",
                crate::domain::ApprovalResolution::Approved,
            )
            .await
            .unwrap();
        let huge = "x".repeat(MAX_STAGED_EVENT_BYTES + 1);
        for index in 0..MAX_STAGED_EVENT_ROWS + 1 {
            let ordinary = format!("corrupt-{index}");
            let content = if index == 5 {
                huge.as_str()
            } else {
                ordinary.as_str()
            };
            sqlx::query(
                "INSERT INTO staged_provider_events \
                 (id, conversation_id, run_id, agent_id, kind, content, mutation_state, \
                  overflowed_kind, created_at) \
                 VALUES (?, ?, ?, ?, 'progress', ?, NULL, NULL, 1)",
            )
            .bind(TimelineEventId::new().to_string())
            .bind(conversation.id.to_string())
            .bind(run.id.to_string())
            .bind(root.id.to_string())
            .bind(content)
            .execute(&store.pool)
            .await
            .unwrap();
        }

        let recovery = store
            .pending_recovery()
            .await
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.run.id == run.id)
            .unwrap();
        assert!(recovery.staged_events.len() <= MAX_STAGED_EVENT_ROWS);
        assert!(
            recovery
                .staged_events
                .iter()
                .map(|event| event.content.len())
                .sum::<usize>()
                <= MAX_STAGED_EVENT_BYTES
        );
        assert!(recovery.staged_events_truncated);
        assert!(!recovery.staged_events_overflowed);

        assert!(matches!(
            store
                .acknowledge_response_intent(run.id, root.id, "bounded-ack")
                .await,
            Err(StoreError::CorruptStagedEventQueue)
        ));
        assert_eq!(
            store.load_run(run.id).await.unwrap().status,
            RunStatus::Waiting
        );
        let approval = store.load_approval(run.id, "bounded-ack").await.unwrap();
        assert_eq!(approval.status, crate::domain::ApprovalStatus::Pending);
        assert_eq!(
            approval.response_intent.unwrap().status,
            crate::domain::ApprovalResponseIntentStatus::Recorded
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM events WHERE run_id = ? AND content = 'Provider run resumed'",
            )
            .bind(run.id.to_string())
            .fetch_one(&store.pool)
            .await
            .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM staged_provider_events WHERE run_id = ?",
            )
            .bind(run.id.to_string())
            .fetch_one(&store.pool)
            .await
            .unwrap(),
            i64::try_from(MAX_STAGED_EVENT_ROWS + 1).unwrap()
        );
    }

    #[tokio::test]
    async fn workspace_cannot_be_linked_to_a_different_conversation() {
        let store = Store::open_in_memory().await.unwrap();
        let owner = store
            .create_conversation(NewConversation::projectless("Owner"))
            .await
            .unwrap();
        let other = store
            .create_conversation(NewConversation::projectless("Other"))
            .await
            .unwrap();
        let workspace_id = WorkspaceId::new();

        sqlx::query(
            "INSERT INTO workspaces \
             (id, conversation_id, project_root, execution_path, owned_worktree, created_at, updated_at) \
             VALUES (?, ?, NULL, '/tmp/project', 0, 1, 1)",
        )
        .bind(workspace_id.to_string())
        .bind(owner.id.to_string())
        .execute(&store.pool)
        .await
        .unwrap();

        let cross_link = sqlx::query("UPDATE conversations SET workspace_id = ? WHERE id = ?")
            .bind(workspace_id.to_string())
            .bind(other.id.to_string())
            .execute(&store.pool)
            .await;

        assert!(cross_link.is_err());
        sqlx::query("UPDATE conversations SET workspace_id = ? WHERE id = ?")
            .bind(workspace_id.to_string())
            .bind(owner.id.to_string())
            .execute(&store.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM conversations WHERE id = ?")
            .bind(owner.id.to_string())
            .execute(&store.pool)
            .await
            .unwrap();
        let workspace_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspaces")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(workspace_count, 0);
    }

    #[tokio::test]
    async fn owned_workspace_requires_a_durable_base_commit() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("Owner"))
            .await
            .unwrap();

        let result = sqlx::query(
            "INSERT INTO workspaces \
             (id, conversation_id, project_root, execution_path, owned_worktree, worktree_base_commit, created_at, updated_at) \
             VALUES (?, ?, '/tmp/project', '/tmp/worktree', 1, NULL, 1, 1)",
        )
        .bind(WorkspaceId::new().to_string())
        .bind(conversation.id.to_string())
        .execute(&store.pool)
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn agent_parent_must_belong_to_the_same_run() {
        let store = Store::open_in_memory().await.unwrap();
        let [(_, _, first_root), (_, second_run, _)] = two_runs(&store).await;

        let result = sqlx::query(
            "INSERT INTO agent_nodes \
             (id, run_id, parent_id, provider, label, status, created_at, updated_at) \
             VALUES (?, ?, ?, 'codex', 'cross-run child', 'queued', 1, 1)",
        )
        .bind(AgentId::new().to_string())
        .bind(second_run.to_string())
        .bind(first_root.to_string())
        .execute(&store.pool)
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn event_conversation_must_own_its_run() {
        let store = Store::open_in_memory().await.unwrap();
        let [(first_conversation, _, _), (_, second_run, second_root)] = two_runs(&store).await;

        let result = insert_event(
            &store,
            first_conversation,
            second_run,
            second_root,
            "wrong conversation",
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn event_agent_must_belong_to_its_run() {
        let store = Store::open_in_memory().await.unwrap();
        let [(_, _, first_root), (second_conversation, second_run, _)] = two_runs(&store).await;

        let result = insert_event(
            &store,
            second_conversation,
            second_run,
            first_root,
            "wrong agent",
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn approval_agent_must_belong_to_its_run() {
        let store = Store::open_in_memory().await.unwrap();
        let [(_, _, first_root), (second_conversation, second_run, _)] = two_runs(&store).await;

        let result = sqlx::query(
            "INSERT INTO approvals \
             (id, conversation_id, run_id, agent_id, provider, operation, scope, status, resolution_json, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 'codex', 'write', 'file', 'pending', NULL, 1, 1)",
        )
        .bind(ApprovalId::new().to_string())
        .bind(second_conversation.to_string())
        .bind(second_run.to_string())
        .bind(first_root.to_string())
        .execute(&store.pool)
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn approval_conversation_must_own_its_run() {
        let store = Store::open_in_memory().await.unwrap();
        let [(first_conversation, _, _), (_, second_run, second_root)] = two_runs(&store).await;

        let result = sqlx::query(
            "INSERT INTO approvals \
             (id, conversation_id, run_id, agent_id, provider, operation, scope, status, \
              resolution_json, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 'codex', 'write', 'file', 'pending', NULL, 1, 1)",
        )
        .bind(ApprovalId::new().to_string())
        .bind(first_conversation.to_string())
        .bind(second_run.to_string())
        .bind(second_root.to_string())
        .execute(&store.pool)
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn approval_resolution_must_match_its_status() {
        let store = Store::open_in_memory().await.unwrap();
        let [(conversation, run, root), _] = two_runs(&store).await;

        let result = sqlx::query(
            "INSERT INTO approvals \
             (id, conversation_id, run_id, agent_id, provider, operation, scope, status, resolution_json, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 'codex', 'write', 'file', 'answered', '{\"kind\":\"denied\"}', 1, 1)",
        )
        .bind(ApprovalId::new().to_string())
        .bind(conversation.to_string())
        .bind(run.to_string())
        .bind(root.to_string())
        .execute(&store.pool)
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn run_session_must_belong_to_the_same_conversation() {
        let store = Store::open_in_memory().await.unwrap();
        let [first, second] = two_conversations(&store).await;
        let session = insert_provider_session(&store, first, "codex").await;

        let result = insert_provider_run(&store, second, Some(&session), "codex").await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn run_session_must_use_the_same_provider() {
        let store = Store::open_in_memory().await.unwrap();
        let [conversation, _] = two_conversations(&store).await;
        let session = insert_provider_session(&store, conversation, "codex").await;

        let result = insert_provider_run(&store, conversation, Some(&session), "claude").await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn message_run_must_belong_to_the_same_conversation() {
        let store = Store::open_in_memory().await.unwrap();
        let [(first_conversation, _, _), (_, second_run, _)] = two_runs(&store).await;

        let result = insert_message(&store, first_conversation, Some(second_run)).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn nullable_ownership_references_are_cleared_without_cross_aggregate_deletes() {
        let store = Store::open_in_memory().await.unwrap();
        let [first, second] = two_conversations(&store).await;
        let first_session = insert_provider_session(&store, first, "codex").await;
        let second_session = insert_provider_session(&store, second, "codex").await;
        let first_run = insert_provider_run(&store, first, Some(&first_session), "codex")
            .await
            .unwrap();
        let second_run = insert_provider_run(&store, second, Some(&second_session), "codex")
            .await
            .unwrap();
        insert_message(&store, first, Some(first_run))
            .await
            .unwrap();
        insert_message(&store, second, Some(second_run))
            .await
            .unwrap();

        sqlx::query("DELETE FROM provider_sessions WHERE id = ?")
            .bind(&first_session)
            .execute(&store.pool)
            .await
            .unwrap();
        let first_run_session: Option<String> =
            sqlx::query_scalar("SELECT provider_session_id FROM provider_runs WHERE id = ?")
                .bind(first_run.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        let second_run_session: Option<String> =
            sqlx::query_scalar("SELECT provider_session_id FROM provider_runs WHERE id = ?")
                .bind(second_run.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(first_run_session, None);
        assert_eq!(second_run_session.as_deref(), Some(second_session.as_str()));

        sqlx::query("DELETE FROM provider_runs WHERE id = ?")
            .bind(first_run.to_string())
            .execute(&store.pool)
            .await
            .unwrap();
        let first_message_run: Option<String> =
            sqlx::query_scalar("SELECT run_id FROM messages WHERE conversation_id = ?")
                .bind(first.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        let second_message_run: Option<String> =
            sqlx::query_scalar("SELECT run_id FROM messages WHERE conversation_id = ?")
                .bind(second.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(first_message_run, None);
        assert_eq!(
            second_message_run.as_deref(),
            Some(second_run.to_string().as_str())
        );

        sqlx::query("DELETE FROM conversations WHERE id = ?")
            .bind(first.to_string())
            .execute(&store.pool)
            .await
            .unwrap();
        let first_messages: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE conversation_id = ?")
                .bind(first.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        let second_messages: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE conversation_id = ?")
                .bind(second.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(first_messages, 0);
        assert_eq!(second_messages, 1);

        sqlx::query("DELETE FROM conversations WHERE id = ?")
            .bind(second.to_string())
            .execute(&store.pool)
            .await
            .unwrap();
        let remaining_owned_rows: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM provider_sessions) + \
                    (SELECT COUNT(*) FROM provider_runs) + \
                    (SELECT COUNT(*) FROM messages)",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert_eq!(remaining_owned_rows, 0);
    }

    #[tokio::test]
    async fn child_lifecycle_events_update_only_the_child_agent() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("Test"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        let completed = insert_child(&store, run.id, root.id, "queued", "completed").await;
        let interrupted = insert_child(&store, run.id, root.id, "queued", "interrupted").await;
        let failed = insert_child(&store, run.id, root.id, "queued", "failed").await;

        store
            .append_run_event(run.id, completed, ProviderEventRecord::started())
            .await
            .unwrap();
        store
            .append_run_event(run.id, completed, ProviderEventRecord::waiting())
            .await
            .unwrap();
        let waiting_state = store.pending_recovery().await.unwrap();
        assert_eq!(waiting_state[0].run.status, RunStatus::Running);
        assert_eq!(
            waiting_state[0]
                .agents
                .iter()
                .find(|agent| agent.id == completed)
                .unwrap()
                .status,
            crate::domain::AgentStatus::Waiting
        );
        store
            .append_run_event(run.id, completed, ProviderEventRecord::resumed())
            .await
            .unwrap();
        store
            .append_run_event(run.id, completed, ProviderEventRecord::completed())
            .await
            .unwrap();
        store
            .append_run_event(run.id, interrupted, ProviderEventRecord::interrupted())
            .await
            .unwrap();
        store
            .append_run_event(run.id, failed, ProviderEventRecord::failed("child failed"))
            .await
            .unwrap();

        let recovered = store.pending_recovery().await.unwrap();
        assert_eq!(recovered[0].run.status, RunStatus::Running);
        let statuses = recovered[0]
            .agents
            .iter()
            .map(|agent| (agent.id, agent.status))
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(statuses[&completed], crate::domain::AgentStatus::Completed);
        assert_eq!(
            statuses[&interrupted],
            crate::domain::AgentStatus::Interrupted
        );
        assert_eq!(statuses[&failed], crate::domain::AgentStatus::Failed);
        assert_eq!(recovered[0].events.len(), 7);
    }

    #[tokio::test]
    async fn child_lifecycle_timeline_content_describes_the_agent() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("Test"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        let child = insert_child(&store, run.id, root.id, "queued", "child").await;
        for record in [
            ProviderEventRecord::started(),
            ProviderEventRecord::waiting(),
            ProviderEventRecord::resumed(),
            ProviderEventRecord::completed(),
        ] {
            store.append_run_event(run.id, child, record).await.unwrap();
        }

        let timeline = store
            .load_timeline(conversation.id, None, 20)
            .await
            .unwrap();
        assert_eq!(timeline.items[0].content, "Provider run started");
        assert_eq!(
            timeline
                .items
                .iter()
                .filter(|event| event.agent_id == child)
                .map(|event| event.content.as_str())
                .collect::<Vec<_>>(),
            [
                "Agent started",
                "Agent is waiting",
                "Agent resumed",
                "Agent completed"
            ]
        );
    }

    #[tokio::test]
    async fn child_activity_continues_while_the_root_is_waiting() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("Test"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        let child = insert_child(&store, run.id, root.id, "queued", "child").await;
        store
            .append_run_event(run.id, child, ProviderEventRecord::started())
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::waiting())
            .await
            .unwrap();

        store
            .append_run_event(
                run.id,
                child,
                ProviderEventRecord::progress("still working"),
            )
            .await
            .unwrap();
        store
            .append_run_event(run.id, child, ProviderEventRecord::waiting())
            .await
            .unwrap();
        store
            .append_run_event(run.id, child, ProviderEventRecord::resumed())
            .await
            .unwrap();

        let recovered = store.pending_recovery().await.unwrap();
        assert_eq!(recovered[0].run.status, RunStatus::Waiting);
        assert_eq!(
            recovered[0]
                .agents
                .iter()
                .find(|agent| agent.id == child)
                .unwrap()
                .status,
            crate::domain::AgentStatus::Running
        );
        assert_eq!(recovered[0].events.len(), 6);
    }

    #[tokio::test]
    async fn completed_root_is_rejected_while_a_descendant_is_active() {
        assert_terminal_root_rejected(ProviderEventRecord::completed(), "queued", 1).await;
    }

    #[tokio::test]
    async fn interrupted_root_is_rejected_while_a_descendant_is_active() {
        assert_terminal_root_rejected(ProviderEventRecord::interrupted(), "running", 2).await;
    }

    #[tokio::test]
    async fn failed_root_is_rejected_while_a_descendant_is_active() {
        assert_terminal_root_rejected(ProviderEventRecord::failed("root failed"), "waiting", 3)
            .await;
    }

    #[tokio::test]
    async fn root_terminal_events_are_accepted_after_descendants_are_terminal() {
        let cases = [
            (
                ProviderEventRecord::completed(),
                ProviderEventRecord::completed(),
                "completed",
            ),
            (
                ProviderEventRecord::interrupted(),
                ProviderEventRecord::interrupted(),
                "interrupted",
            ),
            (
                ProviderEventRecord::failed("child failed"),
                ProviderEventRecord::failed("root failed"),
                "failed",
            ),
        ];

        for (child_terminal, root_terminal, expected) in cases {
            let store = Store::open_in_memory().await.unwrap();
            let conversation = store
                .create_conversation(NewConversation::projectless("Test"))
                .await
                .unwrap();
            let (run, root) = store
                .create_run(conversation.id, ProviderId::Codex)
                .await
                .unwrap();
            store
                .append_run_event(run.id, root.id, ProviderEventRecord::started())
                .await
                .unwrap();
            let child = insert_child(&store, run.id, root.id, "queued", "child").await;
            if matches!(child_terminal, ProviderEventRecord::Completed) {
                store
                    .append_run_event(run.id, child, ProviderEventRecord::started())
                    .await
                    .unwrap();
            }
            store
                .append_run_event(run.id, child, child_terminal)
                .await
                .unwrap();

            store
                .append_run_event(run.id, root.id, root_terminal)
                .await
                .unwrap();

            let statuses: (String, String) = sqlx::query_as(
                "SELECT provider_runs.status, agent_nodes.status FROM provider_runs \
                 JOIN agent_nodes ON agent_nodes.id = ? WHERE provider_runs.id = ?",
            )
            .bind(root.id.to_string())
            .bind(run.id.to_string())
            .fetch_one(&store.pool)
            .await
            .unwrap();
            assert_eq!(statuses, (expected.to_owned(), expected.to_owned()));
        }
    }

    #[tokio::test]
    async fn illegal_child_transition_rolls_back_its_event() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("Test"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        let child = insert_child(&store, run.id, root.id, "queued", "child").await;

        let result = store
            .append_run_event(run.id, child, ProviderEventRecord::completed())
            .await;

        assert!(matches!(result, Err(StoreError::Domain(_))));
        let recovered = store.pending_recovery().await.unwrap();
        assert_eq!(recovered[0].run.status, RunStatus::Running);
        assert_eq!(
            recovered[0]
                .agents
                .iter()
                .find(|agent| agent.id == child)
                .unwrap()
                .status,
            crate::domain::AgentStatus::Queued
        );
        assert_eq!(recovered[0].events.len(), 1);
    }

    #[tokio::test]
    async fn composite_ownership_edges_cascade_owned_records() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("Test"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        let child = insert_child(&store, run.id, root.id, "running", "child").await;
        insert_event(&store, conversation.id, run.id, child, "child event")
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO approvals \
             (id, conversation_id, run_id, agent_id, provider, operation, scope, status, resolution_json, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 'codex', 'write', 'file', 'pending', NULL, 1, 1)",
        )
        .bind(ApprovalId::new().to_string())
        .bind(conversation.id.to_string())
        .bind(run.id.to_string())
        .bind(child.to_string())
        .execute(&store.pool)
        .await
        .unwrap();

        sqlx::query("DELETE FROM agent_nodes WHERE id = ?")
            .bind(root.id.to_string())
            .execute(&store.pool)
            .await
            .unwrap();

        let child_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_nodes WHERE id = ?")
            .bind(child.to_string())
            .fetch_one(&store.pool)
            .await
            .unwrap();
        let child_event_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE agent_id = ?")
                .bind(child.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        let child_approval_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM approvals WHERE agent_id = ?")
                .bind(child.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(child_count, 0);
        assert_eq!(child_event_count, 0);
        assert_eq!(child_approval_count, 0);

        sqlx::query("DELETE FROM conversations WHERE id = ?")
            .bind(conversation.id.to_string())
            .execute(&store.pool)
            .await
            .unwrap();
        let run_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM provider_runs")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        let agent_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_nodes")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(run_count, 0);
        assert_eq!(agent_count, 0);
    }

    #[tokio::test]
    async fn recovery_batches_more_than_200_agents_without_omission() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("Many agents"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        let mut expected = std::collections::HashSet::from([root.id]);
        for index in 0..205 {
            expected.insert(
                insert_child(&store, run.id, root.id, "queued", &format!("child {index}")).await,
            );
        }

        let recovered = store.pending_recovery().await.unwrap();
        let actual = recovered[0]
            .agents
            .iter()
            .map(|agent| agent.id)
            .collect::<std::collections::HashSet<_>>();

        assert_eq!(recovered[0].agents.len(), 206);
        assert_eq!(actual, expected);
    }

    async fn two_runs(store: &Store) -> [(ConversationId, RunId, AgentId); 2] {
        let first = store
            .create_conversation(NewConversation::projectless("First"))
            .await
            .unwrap();
        let (first_run, first_root) = store.create_run(first.id, ProviderId::Codex).await.unwrap();
        let second = store
            .create_conversation(NewConversation::projectless("Second"))
            .await
            .unwrap();
        let (second_run, second_root) = store
            .create_run(second.id, ProviderId::Codex)
            .await
            .unwrap();
        [
            (first.id, first_run.id, first_root.id),
            (second.id, second_run.id, second_root.id),
        ]
    }

    async fn two_conversations(store: &Store) -> [ConversationId; 2] {
        let first = store
            .create_conversation(NewConversation::projectless("First"))
            .await
            .unwrap();
        let second = store
            .create_conversation(NewConversation::projectless("Second"))
            .await
            .unwrap();
        [first.id, second.id]
    }

    async fn insert_provider_session(
        store: &Store,
        conversation_id: ConversationId,
        provider: &str,
    ) -> String {
        let id = format!("session-{conversation_id}-{provider}");
        sqlx::query(
            "INSERT INTO provider_sessions \
             (id, conversation_id, provider, native_session_id, created_at, updated_at) \
             VALUES (?, ?, ?, NULL, 1, 1)",
        )
        .bind(&id)
        .bind(conversation_id.to_string())
        .bind(provider)
        .execute(&store.pool)
        .await
        .unwrap();
        id
    }

    async fn insert_provider_run(
        store: &Store,
        conversation_id: ConversationId,
        provider_session_id: Option<&str>,
        provider: &str,
    ) -> Result<RunId, sqlx::Error> {
        let id = RunId::new();
        sqlx::query(
            "INSERT INTO provider_runs \
             (id, conversation_id, provider_session_id, provider, native_session_id, status, \
              mutation_state, created_at, updated_at) \
             VALUES (?, ?, ?, ?, NULL, 'queued', 'none_observed', 1, 1)",
        )
        .bind(id.to_string())
        .bind(conversation_id.to_string())
        .bind(provider_session_id)
        .bind(provider)
        .execute(&store.pool)
        .await?;
        Ok(id)
    }

    async fn insert_message(
        store: &Store,
        conversation_id: ConversationId,
        run_id: Option<RunId>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO messages \
             (id, conversation_id, run_id, role, content, created_at) \
             VALUES (?, ?, ?, 'assistant', 'message', 1)",
        )
        .bind(TimelineEventId::new().to_string())
        .bind(conversation_id.to_string())
        .bind(run_id.map(|id| id.to_string()))
        .execute(&store.pool)
        .await?;
        Ok(())
    }

    async fn insert_event(
        store: &Store,
        conversation_id: ConversationId,
        run_id: RunId,
        agent_id: AgentId,
        content: &str,
    ) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> {
        sqlx::query(
            "INSERT INTO events \
             (id, conversation_id, run_id, agent_id, kind, content, payload_json, created_at) \
             VALUES (?, ?, ?, ?, 'progress', ?, NULL, 1)",
        )
        .bind(TimelineEventId::new().to_string())
        .bind(conversation_id.to_string())
        .bind(run_id.to_string())
        .bind(agent_id.to_string())
        .bind(content)
        .execute(&store.pool)
        .await
    }

    async fn insert_child(
        store: &Store,
        run_id: RunId,
        parent_id: AgentId,
        status: &str,
        label: &str,
    ) -> AgentId {
        let child_id = AgentId::new();
        sqlx::query(
            "INSERT INTO agent_nodes \
             (id, run_id, parent_id, provider, label, status, depth, created_at, updated_at) \
             VALUES (?, ?, ?, 'codex', ?, ?, \
                     (SELECT depth + 1 FROM agent_nodes WHERE id = ?), 1, 1)",
        )
        .bind(child_id.to_string())
        .bind(run_id.to_string())
        .bind(parent_id.to_string())
        .bind(label)
        .bind(status)
        .bind(parent_id.to_string())
        .execute(&store.pool)
        .await
        .unwrap();
        child_id
    }

    async fn assert_terminal_root_rejected(
        record: ProviderEventRecord,
        expected_child_status: &str,
        expected_event_count: i64,
    ) {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("Test"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();
        let child = insert_child(&store, run.id, root.id, "queued", "child").await;
        if matches!(expected_child_status, "running" | "waiting") {
            store
                .append_run_event(run.id, child, ProviderEventRecord::started())
                .await
                .unwrap();
        }
        if expected_child_status == "waiting" {
            store
                .append_run_event(run.id, child, ProviderEventRecord::waiting())
                .await
                .unwrap();
        }

        let result = store.append_run_event(run.id, root.id, record).await;

        assert!(result.is_err());
        let run_status: String =
            sqlx::query_scalar("SELECT status FROM provider_runs WHERE id = ?")
                .bind(run.id.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        let root_status: String = sqlx::query_scalar("SELECT status FROM agent_nodes WHERE id = ?")
            .bind(root.id.to_string())
            .fetch_one(&store.pool)
            .await
            .unwrap();
        let child_status: String =
            sqlx::query_scalar("SELECT status FROM agent_nodes WHERE id = ?")
                .bind(child.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        let event_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE run_id = ?")
            .bind(run.id.to_string())
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(run_status, "running");
        assert_eq!(root_status, "running");
        assert_eq!(child_status, expected_child_status);
        assert_eq!(event_count, expected_event_count);
    }
}
