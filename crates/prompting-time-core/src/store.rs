use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{FromRow, Sqlite, SqlitePool, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::domain::{
    AgentId, AgentNode, AgentStatus, Approval, ApprovalId, ApprovalRequestDetails,
    ApprovalResolution, ApprovalResponseIntent, ApprovalResponseIntentStatus, ApprovalStatus,
    Conversation, ConversationId, DomainError, MutationState, ProviderRun, RunId, RunStatus,
    TimelineEvent, TimelineEventId, TimelineEventKind,
};
use crate::providers::{
    DispatchCertainty, NativeChildStatus, NativeSubAgentActivityKind, ProviderErrorCategory,
    ProviderId, ProviderSession, UserInputQuestion, UserInputRequest,
};

const MAX_PAGE_SIZE: u32 = 200;
const MAX_POOL_CONNECTIONS: u32 = 8;
const RECOVERY_BATCH_SIZE: i64 = 200;
/// Physical queue bound: 256 complete provider events plus one reserved overflow marker.
pub const MAX_STAGED_EVENT_ROWS: usize = 257;
pub const MAX_STAGED_EVENT_BYTES: usize = 8 * 1024 * 1024;
const STAGED_OVERFLOW_CONTENT: &str = "Provider output omitted: staged queue limit exceeded";
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewConversation {
    title: String,
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
            (Self::SubAgent { agent_path, .. }, _) => (TimelineEventKind::Progress, agent_path),
            (Self::Unrecognized { method }, _) => (TimelineEventKind::Diagnostic, method),
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
            | Self::Unrecognized { .. } => None,
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
pub struct RecoveryRun {
    pub run: ProviderRun,
    pub agents: Vec<AgentNode>,
    pub approvals: Vec<Approval>,
    pub staged_events: Vec<StagedProviderEvent>,
    pub staged_events_overflowed: bool,
    pub staged_events_truncated: bool,
    pub events: Vec<TimelineEvent>,
    pub events_truncated: bool,
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

    async fn connect(
        options: SqliteConnectOptions,
        max_connections: u32,
    ) -> Result<Self, StoreError> {
        let pool = SqlitePoolOptions::new()
            .max_connections(max_connections)
            .connect_with(options)
            .await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn close(self) {
        self.pool.close().await;
    }

    pub async fn create_conversation(
        &self,
        new_conversation: NewConversation,
    ) -> Result<Conversation, StoreError> {
        let conversation = Conversation {
            id: ConversationId::new(),
            title: new_conversation.title,
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

        Ok(conversation)
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

        Ok((run, root))
    }

    pub async fn create_fallback_run(
        &self,
        primary_run_id: RunId,
        provider: ProviderId,
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

        let mut run = ProviderRun::new(primary.conversation_id, provider);
        run.fallback_from_run_id = Some(primary_run_id);
        let root = AgentNode::root(run.id, provider, "orchestrator");
        let now = now_millis();
        sqlx::query(
            "INSERT INTO provider_runs \
             (id, conversation_id, provider, fallback_from_run_id, native_session_id, status, mutation_state, created_at, updated_at) \
             VALUES (?, ?, ?, ?, NULL, ?, ?, ?, ?)",
        )
        .bind(run.id.to_string())
        .bind(run.conversation_id.to_string())
        .bind(provider_label(run.provider))
        .bind(primary_run_id.to_string())
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
            .bind(run.conversation_id.to_string())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok((run, root))
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
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
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

    pub async fn append_run_event(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        record: ProviderEventRecord,
    ) -> Result<TimelineEvent, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
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
            "SELECT id, run_id, parent_id, provider, label, status, created_at \
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
        validate_event_state(&record, &run, &agent)?;
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
            sqlx::query("UPDATE conversations SET updated_at = ? WHERE id = ?")
                .bind(now)
                .bind(run.conversation_id.to_string())
                .execute(&mut *transaction)
                .await?;
            transaction.commit().await?;
            return Ok(TimelineEvent {
                id: parse_uuid("timeline event", &existing_id)?.into(),
                conversation_id: run.conversation_id,
                run_id,
                agent_id,
                sequence: u64::try_from(sequence).map_err(|_| StoreError::InvalidData {
                    entity: "timeline event",
                    detail: "negative sequence".to_owned(),
                })?,
                kind,
                content: existing_content + content,
            });
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
            sqlx::query(
                "INSERT INTO approvals \
                 (id, run_id, agent_id, provider, provider_request_id, operation, scope, request_json, details_json, status, resolution_json, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', NULL, ?, ?)",
            )
            .bind(ApprovalId::new().to_string())
            .bind(run_id.to_string())
            .bind(agent_id.to_string())
            .bind(provider_label(provider))
            .bind(request_id)
            .bind(operation)
            .bind(scope)
            .bind(request_json)
            .bind(details_json)
            .bind(now)
            .bind(now)
            .execute(&mut *transaction)
            .await?;
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
             (id, conversation_id, run_id, agent_id, kind, content, payload_json, native_item_id, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(event_id.to_string())
        .bind(run.conversation_id.to_string())
        .bind(run_id.to_string())
        .bind(agent_id.to_string())
        .bind(event_kind_label(kind))
        .bind(content)
        .bind(payload_json)
        .bind(native_item_id)
        .bind(now)
        .execute(&mut *transaction)
        .await?;

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
            sqlx::query(
                "UPDATE provider_runs SET dispatch_certainty = ?, updated_at = ? WHERE id = ?",
            )
            .bind(dispatch_certainty_label(dispatch_certainty))
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

        Ok(TimelineEvent {
            id: event_id,
            conversation_id: run.conversation_id,
            run_id,
            agent_id,
            sequence: u64::try_from(result.last_insert_rowid()).map_err(|_| {
                StoreError::InvalidData {
                    entity: "timeline event",
                    detail: "negative sequence".to_owned(),
                }
            })?,
            kind,
            content: content.to_owned(),
        })
    }

    pub async fn stage_waiting_event(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        record: ProviderEventRecord,
    ) -> Result<StageWaitingEventOutcome, StoreError> {
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
            ProviderEventRecord::SubAgent { agent_path, .. } => {
                (TimelineEventKind::Progress, agent_path, None)
            }
            ProviderEventRecord::Unrecognized { method } => {
                (TimelineEventKind::Diagnostic, method, None)
            }
            _ => return Err(StoreError::InvalidStagedEvent),
        };
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
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
            "SELECT id, run_id, parent_id, provider, label, status, created_at \
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

        let existing_message = if let Some(native_item_id) = native_item_id.as_deref() {
            sqlx::query_as::<_, (String, i64, String, Option<String>)>(
                "SELECT id, sequence, content, payload_json FROM staged_provider_events \
                 WHERE run_id = ? AND agent_id = ? AND kind = 'message' AND native_item_id = ?",
            )
            .bind(run_id.to_string())
            .bind(agent_id.to_string())
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
            return Ok(StageWaitingEventOutcome::Staged(StagedProviderEvent {
                id: parse_uuid("staged provider event", &existing_id)?.into(),
                conversation_id: run.conversation_id,
                run_id,
                agent_id,
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
        .bind(agent_id.to_string())
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

        let staged = StagedProviderEvent {
            id,
            conversation_id: run.conversation_id,
            run_id,
            agent_id,
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
        if matches!(
            resolution,
            ApprovalResolution::Cancelled | ApprovalResolution::Failed
        ) {
            return Err(StoreError::InvalidApprovalResolution);
        }
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let run_status: String =
            sqlx::query_scalar("SELECT status FROM provider_runs WHERE id = ?")
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
        Ok(approval)
    }

    pub async fn reject_response_intent(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        provider_request_id: &str,
        dispatch_certainty: DispatchCertainty,
    ) -> Result<Approval, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
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
        Ok(approval)
    }

    pub async fn acknowledge_response_intent(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        provider_request_id: &str,
    ) -> Result<TimelineEvent, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
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
            "SELECT id, run_id, parent_id, provider, label, status, created_at \
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
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
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
        validate_page_limit(limit)?;
        let cursor = cursor.map(|value| decode_cursor(&value)).transpose()?;
        let rows = if let Some(cursor) = cursor {
            sqlx::query_as::<_, ConversationRow>(
                "SELECT id, title, workspace_id, status, updated_at FROM conversations \
                 WHERE updated_at < ? OR (updated_at = ? AND id < ?) \
                 ORDER BY updated_at DESC, id DESC LIMIT ?",
            )
            .bind(cursor_sequence_i64(&cursor)?)
            .bind(cursor_sequence_i64(&cursor)?)
            .bind(cursor.id.to_string())
            .bind(i64::from(limit) + 1)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, ConversationRow>(
                "SELECT id, title, workspace_id, status, updated_at FROM conversations \
                 ORDER BY updated_at DESC, id DESC LIMIT ?",
            )
            .bind(i64::from(limit) + 1)
            .fetch_all(&self.pool)
            .await?
        };

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
                "SELECT id, conversation_id, run_id, agent_id, sequence, kind, content \
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
                "SELECT id, conversation_id, run_id, agent_id, sequence, kind, content \
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
                let mut agents = Vec::new();
                let mut agent_cursor: Option<(i64, String)> = None;
                loop {
                    let agent_rows = if let Some((created_at, id)) = &agent_cursor {
                        sqlx::query_as::<_, AgentNodeRow>(
                            "SELECT id, run_id, parent_id, provider, label, status, created_at \
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
                            "SELECT id, run_id, parent_id, provider, label, status, created_at \
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
                    "SELECT id, conversation_id, run_id, agent_id, sequence, kind, content \
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
}

async fn drain_staged_events_in_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    run_id: RunId,
    agent_id: Option<AgentId>,
) -> Result<Vec<TimelineEvent>, StoreError> {
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
                content: existing_content + &event.content,
            });
            continue;
        }
        let inserted = sqlx::query(
            "INSERT INTO events \
             (id, conversation_id, run_id, agent_id, kind, content, payload_json, native_item_id, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(event.id.to_string())
        .bind(event.conversation_id.to_string())
        .bind(event.run_id.to_string())
        .bind(event.agent_id.to_string())
        .bind(event_kind_label(event.kind))
        .bind(&event.content)
        .bind(event.payload_json.or_else(|| {
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
        }))
        .bind(event.native_item_id)
        .bind(now)
        .execute(&mut **transaction)
        .await?;
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

fn cursor_sequence_i64(cursor: &Cursor) -> Result<i64, StoreError> {
    i64::try_from(cursor.sequence).map_err(|_| StoreError::InvalidCursor)
}

fn now_millis() -> i64 {
    i64::try_from(OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
        .expect("the current timestamp fits in SQLite INTEGER")
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
                title: self.title,
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
    label: String,
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
            label: self.label,
            status: parse_agent_status(&self.status)?,
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
    content: String,
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
            content: self.content,
        })
    }
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use crate::domain::{
        AgentId, ApprovalId, ConversationId, RunId, RunStatus, TimelineEventId, WorkspaceId,
    };
    use crate::providers::ProviderId;

    use super::{
        MAX_STAGED_EVENT_BYTES, MAX_STAGED_EVENT_ROWS, MIGRATOR, NewConversation,
        ProviderEventRecord, STAGED_OVERFLOW_CONTENT, Store, StoreError,
    };

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
              'staged_provider_events')",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();
        let index_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'index' AND name IN \
             ('idx_conversations_status_updated', 'idx_conversations_workspace_updated', \
              'idx_runs_conversation_created', 'idx_agents_run_parent', \
              'idx_events_conversation_sequence', 'idx_approvals_pending', \
              'idx_staged_provider_events_run_sequence')",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();

        assert_eq!(foreign_keys, 1);
        assert_eq!(journal_mode, "wal");
        assert_eq!(busy_timeout, 5_000);
        assert_eq!(table_count, 10);
        assert_eq!(index_count, 7);

        let workspace_columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('workspaces') ORDER BY cid")
                .fetch_all(&store.pool)
                .await
                .unwrap();
        assert!(workspace_columns.contains(&"worktree_base_commit".to_owned()));
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
        let store = Store { pool };

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
        let [(_, _, first_root), (_, second_run, _)] = two_runs(&store).await;

        let result = sqlx::query(
            "INSERT INTO approvals \
             (id, run_id, agent_id, provider, operation, scope, status, resolution_json, created_at, updated_at) \
             VALUES (?, ?, ?, 'codex', 'write', 'file', 'pending', NULL, 1, 1)",
        )
        .bind(ApprovalId::new().to_string())
        .bind(second_run.to_string())
        .bind(first_root.to_string())
        .execute(&store.pool)
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn approval_resolution_must_match_its_status() {
        let store = Store::open_in_memory().await.unwrap();
        let [(_, run, root), _] = two_runs(&store).await;

        let result = sqlx::query(
            "INSERT INTO approvals \
             (id, run_id, agent_id, provider, operation, scope, status, resolution_json, created_at, updated_at) \
             VALUES (?, ?, ?, 'codex', 'write', 'file', 'answered', '{\"kind\":\"denied\"}', 1, 1)",
        )
        .bind(ApprovalId::new().to_string())
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
             (id, run_id, agent_id, provider, operation, scope, status, resolution_json, created_at, updated_at) \
             VALUES (?, ?, ?, 'codex', 'write', 'file', 'pending', NULL, 1, 1)",
        )
        .bind(ApprovalId::new().to_string())
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
             (id, run_id, parent_id, provider, label, status, created_at, updated_at) \
             VALUES (?, ?, ?, 'codex', ?, ?, 1, 1)",
        )
        .bind(child_id.to_string())
        .bind(run_id.to_string())
        .bind(parent_id.to_string())
        .bind(label)
        .bind(status)
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
