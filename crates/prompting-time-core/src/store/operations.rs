use super::*;
use crate::tool_operation::ToolOperationStatus;

impl Store {
    /// A discarded display observation still contributes mutation evidence.
    pub(crate) async fn retain_discarded_operation_mutation(
        &self,
        run_id: RunId,
        expected_owner: &str,
        mutation: MutationState,
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        require_dispatch_owner(&mut transaction, run_id, expected_owner).await?;
        let current: String =
            sqlx::query_scalar("SELECT mutation_state FROM provider_runs WHERE id = ?")
                .bind(run_id.to_string())
                .fetch_one(&mut *transaction)
                .await?;
        let next = merge_mutation_state(parse_mutation_state(&current)?, mutation);
        sqlx::query("UPDATE provider_runs SET mutation_state = ?, updated_at = ? WHERE id = ?")
            .bind(mutation_state_label(next))
            .bind(now_millis())
            .bind(run_id.to_string())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(())
    }
}

pub(super) fn normalize(record: ProviderEventRecord) -> Result<ProviderEventRecord, StoreError> {
    if let ProviderEventRecord::NativeItem {
        native_item_id,
        native_turn_id,
        native_agent_id,
        operation,
        content,
        mutation,
    } = record
    {
        validate_native_agent_id(&native_item_id)?;
        for id in [native_turn_id.as_deref(), native_agent_id.as_deref()]
            .into_iter()
            .flatten()
        {
            validate_native_agent_id(id)?;
        }
        let operation = operation.map(ToolOperation::bounded);
        let content = operation
            .as_ref()
            .map_or(content, |operation| operation.title.clone());
        return Ok(ProviderEventRecord::NativeItem {
            native_item_id,
            native_turn_id,
            native_agent_id,
            operation,
            content,
            mutation,
        });
    }
    Ok(record)
}

/// Resolve only materialized child identities; a missing child never becomes root activity.
pub(super) async fn owner(
    transaction: &mut Transaction<'_, Sqlite>,
    run: &ProviderRun,
    fallback: AgentId,
    record: &ProviderEventRecord,
) -> Result<AgentId, StoreError> {
    let ProviderEventRecord::NativeItem {
        native_agent_id: Some(id),
        native_turn_id,
        ..
    } = record
    else {
        return Ok(fallback);
    };
    let agent = load_agent_by_native_id(transaction, run.id, id)
        .await?
        .ok_or(StoreError::NativeAgentIdentityConflict)?;
    if agent.parent_id.is_none()
        || agent.provider != run.provider
        || !matches!(agent.status, AgentStatus::Running | AgentStatus::Waiting)
    {
        return Err(StoreError::NativeAgentIdentityConflict);
    }
    if run.provider == ProviderId::Codex {
        let active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM native_child_turns WHERE agent_id = ? AND native_turn_id = ? AND ended = 0)")
            .bind(agent.id.to_string()).bind(native_turn_id).fetch_one(&mut **transaction).await?;
        if !active {
            return Err(StoreError::NativeAgentIdentityConflict);
        }
    } else if native_turn_id.is_some() {
        return Err(StoreError::NativeAgentIdentityConflict);
    }
    Ok(agent.id)
}

pub(super) async fn persist(
    transaction: &mut Transaction<'_, Sqlite>,
    event: TimelineEvent,
    payload: &str,
    now: i64,
) -> Result<TimelineEvent, StoreError> {
    let mut payload: serde_json::Value =
        serde_json::from_str(payload).map_err(invalid_data("tool operation"))?;
    let incoming: ToolOperation = serde_json::from_value(payload["operation"].take())
        .map_err(invalid_data("tool operation"))?;
    let item = payload["nativeItemId"]
        .as_str()
        .ok_or(StoreError::NativeAgentIdentityConflict)?;
    validate_native_agent_id(item)?;
    let turn = payload["nativeTurnId"].as_str();
    if let Some(turn) = turn {
        validate_native_agent_id(turn)?;
    }
    let existing: Option<(String, i64, String, i64)> = sqlx::query_as(
        "SELECT id, sequence, operation_json, operation_revision FROM events WHERE run_id = ? AND agent_id = ? AND coalesce(native_turn_id, '') = coalesce(?, '') AND native_item_id = ? AND operation_json IS NOT NULL")
        .bind(event.run_id.to_string()).bind(event.agent_id.to_string()).bind(turn).bind(item).fetch_optional(&mut **transaction).await?;
    let (id, sequence, operation, revision) =
        if let Some((id, sequence, stored, revision)) = existing {
            let previous: ToolOperation =
                serde_json::from_str(&stored).map_err(invalid_data("tool operation"))?;
            let merged = previous.clone().merge(incoming);
            let revision = if merged == previous {
                revision
            } else {
                revision.saturating_add(1)
            };
            (
                parse_uuid("timeline event", &id)?.into(),
                Some(sequence),
                merged,
                revision,
            )
        } else {
            (event.id, None, incoming.bounded(), 1)
        };
    let operation_json =
        serde_json::to_string(&operation).map_err(invalid_data("tool operation"))?;
    let sequence = if let Some(sequence) = sequence {
        sqlx::query("UPDATE events SET content = ?, operation_json = ?, operation_revision = ? WHERE id = ?")
            .bind(&operation.title).bind(&operation_json).bind(revision).bind(id.to_string()).execute(&mut **transaction).await?;
        sequence
    } else {
        sqlx::query("INSERT INTO events (id, conversation_id, run_id, agent_id, kind, content, payload_json, native_item_id, native_turn_id, operation_json, operation_revision, created_at) VALUES (?, ?, ?, ?, 'tool', ?, ?, ?, ?, ?, ?, ?)")
            .bind(id.to_string()).bind(event.conversation_id.to_string()).bind(event.run_id.to_string()).bind(event.agent_id.to_string())
            .bind(&operation.title).bind(payload.to_string()).bind(item).bind(turn).bind(&operation_json).bind(revision).bind(now).execute(&mut **transaction).await?.last_insert_rowid()
    };
    // Certainty is independent of display outcome, including duplicate/failed snapshots.
    let mutation: MutationState = serde_json::from_value(payload["mutation"].clone())
        .map_err(invalid_data("tool mutation"))?;
    let current: String =
        sqlx::query_scalar("SELECT mutation_state FROM provider_runs WHERE id = ?")
            .bind(event.run_id.to_string())
            .fetch_one(&mut **transaction)
            .await?;
    let mutation = merge_mutation_state(parse_mutation_state(&current)?, mutation);
    sqlx::query("UPDATE provider_runs SET mutation_state = ?, updated_at = ? WHERE id = ?")
        .bind(mutation_state_label(mutation))
        .bind(now)
        .bind(event.run_id.to_string())
        .execute(&mut **transaction)
        .await?;
    sqlx::query("UPDATE conversations SET updated_at = ? WHERE id = ?")
        .bind(now)
        .bind(event.conversation_id.to_string())
        .execute(&mut **transaction)
        .await?;
    Ok(TimelineEvent {
        id,
        sequence: u64::try_from(sequence).map_err(|_| StoreError::InvalidData {
            entity: "tool sequence",
            detail: "negative sequence".into(),
        })?,
        content: operation.title,
        ..event
    })
}

pub(super) fn projected_status(
    status: ToolOperationStatus,
    run: &str,
    agent: &str,
) -> ToolOperationStatus {
    if status != ToolOperationStatus::Running {
        return status;
    }
    if run == "interrupted" || agent == "interrupted" {
        return ToolOperationStatus::Interrupted;
    }
    if matches!(run, "completed" | "failed") || matches!(agent, "completed" | "failed") {
        return ToolOperationStatus::Unknown;
    }
    status
}

pub(super) fn detail(
    json: Option<String>,
    revision: i64,
    run: &str,
    agent: &str,
) -> Result<Option<ToolOperationDetail>, StoreError> {
    json.map(|json| {
        let mut operation: ToolOperation =
            serde_json::from_str(&json).map_err(invalid_data("tool operation"))?;
        let status = projected_status(operation.status, run, agent);
        let settled = status != operation.status;
        operation.status = status;
        Ok(ToolOperationDetail {
            operation: operation.bounded(),
            revision: u64::try_from(revision)
                .map_err(|_| StoreError::InvalidData {
                    entity: "tool revision",
                    detail: "negative revision".into(),
                })?
                .saturating_add(u64::from(settled)),
        })
    })
    .transpose()
}
