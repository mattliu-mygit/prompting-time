use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{FromRow, SqlitePool};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::domain::{
    AgentId, AgentNode, AgentStatus, Conversation, ConversationId, DomainError, MutationState,
    ProviderRun, RunId, RunStatus, TimelineEvent, TimelineEventId, TimelineEventKind,
};
use crate::providers::ProviderId;

const MAX_PAGE_SIZE: u32 = 200;
const MAX_POOL_CONNECTIONS: u32 = 8;
const RECOVERY_BATCH_SIZE: i64 = 200;
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
    Started,
    Progress(String),
    Waiting,
    Resumed,
    Completed,
    Interrupted,
    Failed(String),
}

impl ProviderEventRecord {
    pub fn started() -> Self {
        Self::Started
    }

    pub fn progress(content: impl Into<String>) -> Self {
        Self::Progress(content.into())
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

    pub fn failed(diagnostic: impl Into<String>) -> Self {
        Self::Failed(diagnostic.into())
    }

    fn event_fields(&self, is_root: bool) -> (TimelineEventKind, &str) {
        match (self, is_root) {
            (Self::Started, true) => (TimelineEventKind::Lifecycle, "Provider run started"),
            (Self::Started, false) => (TimelineEventKind::Lifecycle, "Agent started"),
            (Self::Progress(content), _) => (TimelineEventKind::Progress, content),
            (Self::Waiting, true) => (TimelineEventKind::Lifecycle, "Provider run is waiting"),
            (Self::Waiting, false) => (TimelineEventKind::Lifecycle, "Agent is waiting"),
            (Self::Resumed, true) => (TimelineEventKind::Lifecycle, "Provider run resumed"),
            (Self::Resumed, false) => (TimelineEventKind::Lifecycle, "Agent resumed"),
            (Self::Completed, true) => (TimelineEventKind::Lifecycle, "Provider run completed"),
            (Self::Completed, false) => (TimelineEventKind::Lifecycle, "Agent completed"),
            (Self::Interrupted, true) => (TimelineEventKind::Lifecycle, "Provider run interrupted"),
            (Self::Interrupted, false) => (TimelineEventKind::Lifecycle, "Agent interrupted"),
            (Self::Failed(diagnostic), _) => (TimelineEventKind::Diagnostic, diagnostic),
        }
    }

    fn transition(&self) -> Option<(RunStatus, AgentStatus)> {
        match self {
            Self::Started | Self::Resumed => Some((RunStatus::Running, AgentStatus::Running)),
            Self::Waiting => Some((RunStatus::Waiting, AgentStatus::Waiting)),
            Self::Completed => Some((RunStatus::Completed, AgentStatus::Completed)),
            Self::Interrupted => Some((RunStatus::Interrupted, AgentStatus::Interrupted)),
            Self::Failed(_) => Some((RunStatus::Failed, AgentStatus::Failed)),
            Self::Progress(_) => None,
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
    pub events: Vec<TimelineEvent>,
    pub events_truncated: bool,
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

    pub async fn append_run_event(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        record: ProviderEventRecord,
    ) -> Result<TimelineEvent, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let run_row = sqlx::query_as::<_, ProviderRunRow>(
            "SELECT id, conversation_id, provider, native_session_id, status, mutation_state, created_at \
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
        let (kind, content) = record.event_fields(is_root);
        let event_id = TimelineEventId::new();
        let now = now_millis();

        let result = sqlx::query(
            "INSERT INTO events \
             (id, conversation_id, run_id, agent_id, kind, content, payload_json, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, NULL, ?)",
        )
        .bind(event_id.to_string())
        .bind(run.conversation_id.to_string())
        .bind(run_id.to_string())
        .bind(agent_id.to_string())
        .bind(event_kind_label(kind))
        .bind(content)
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

    pub async fn pending_recovery(&self) -> Result<Vec<RecoveryRun>, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let mut recovery = Vec::new();
        let mut run_cursor: Option<(String, i64, String)> = None;

        loop {
            let run_rows = if let Some((status, created_at, id)) = &run_cursor {
                sqlx::query_as::<_, ProviderRunRow>(
                    "SELECT id, conversation_id, provider, native_session_id, status, mutation_state, created_at \
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
                    "SELECT id, conversation_id, provider, native_session_id, status, mutation_state, created_at \
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
        ProviderEventRecord::Started
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
            if agent.status != AgentStatus::Running
                || (is_root && run.status != RunStatus::Running) =>
        {
            Err(StoreError::InvalidEventState {
                event: "progress",
                status: agent_status_label(agent.status),
            })
        }
        _ => Ok(()),
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
    native_session_id: Option<String>,
    status: String,
    mutation_state: String,
    created_at: i64,
}

impl ProviderRunRow {
    fn into_domain(self) -> Result<ProviderRun, StoreError> {
        Ok(ProviderRun {
            id: parse_uuid("provider run", &self.id)?.into(),
            conversation_id: parse_uuid("conversation", &self.conversation_id)?.into(),
            provider: parse_provider(&self.provider)?,
            native_session_id: self.native_session_id,
            status: parse_run_status(&self.status)?,
            mutation_state: parse_mutation_state(&self.mutation_state)?,
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
    use crate::domain::{
        AgentId, ApprovalId, ConversationId, RunId, RunStatus, TimelineEventId, WorkspaceId,
    };
    use crate::providers::ProviderId;

    use super::{NewConversation, ProviderEventRecord, Store, StoreError};

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
              'agent_nodes', 'messages', 'events', 'approvals', 'routing_decisions')",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();
        let index_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'index' AND name IN \
             ('idx_conversations_status_updated', 'idx_conversations_workspace_updated', \
              'idx_runs_conversation_created', 'idx_agents_run_parent', \
              'idx_events_conversation_sequence', 'idx_approvals_pending')",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();

        assert_eq!(foreign_keys, 1);
        assert_eq!(journal_mode, "wal");
        assert_eq!(busy_timeout, 5_000);
        assert_eq!(table_count, 9);
        assert_eq!(index_count, 6);

        let workspace_columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('workspaces') ORDER BY cid")
                .fetch_all(&store.pool)
                .await
                .unwrap();
        assert!(workspace_columns.contains(&"worktree_base_commit".to_owned()));
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
             (id, run_id, agent_id, provider, operation, scope, status, decision, created_at, updated_at) \
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
             (id, run_id, agent_id, provider, operation, scope, status, decision, created_at, updated_at) \
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
