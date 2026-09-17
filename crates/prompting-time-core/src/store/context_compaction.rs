use super::*;
use crate::context_compaction::{CompactionActivity, CompactionObservation, CompactionPhase};

pub(super) fn event_fields(phase: &CompactionPhase) -> (TimelineEventKind, &'static str) {
    match phase {
        CompactionPhase::Completed { .. } => (TimelineEventKind::Lifecycle, "Context compacted"),
        CompactionPhase::Started => (TimelineEventKind::Diagnostic, "Context compaction started"),
        CompactionPhase::Cleared => (TimelineEventKind::Diagnostic, "Context compaction cleared"),
        CompactionPhase::Failed => (TimelineEventKind::Diagnostic, "Context compaction failed"),
    }
}

pub(super) fn from_payload(payload: &str) -> Result<Option<CompactionObservation>, StoreError> {
    let value: serde_json::Value =
        serde_json::from_str(payload).map_err(invalid_data("compaction"))?;
    value
        .get("compaction")
        .map(|value| serde_json::from_value(value.clone()).map_err(invalid_data("compaction")))
        .transpose()
}

pub(super) async fn owner(
    transaction: &mut Transaction<'_, Sqlite>,
    run: &ProviderRun,
    fallback: AgentId,
    record: &ProviderEventRecord,
) -> Result<AgentId, StoreError> {
    let ProviderEventRecord::Compaction { observation } = record else {
        return Ok(fallback);
    };
    observation
        .validate()
        .map_err(|error| StoreError::InvalidData {
            entity: "compaction",
            detail: error.to_string(),
        })?;
    let agent = if let Some(native) = &observation.native_agent_id {
        let agent = load_agent_by_native_id(transaction, run.id, native)
            .await?
            .ok_or(StoreError::NativeAgentIdentityConflict)?;
        if agent.parent_id.is_none()
            || agent.provider != run.provider
            || observation.native_session_id != *native
        {
            return Err(StoreError::NativeAgentIdentityConflict);
        }
        agent
    } else {
        let agent = sqlx::query_as::<_, AgentNodeRow>("SELECT id, run_id, parent_id, provider, provider_native_id, provider_native_path, label, summary, status, created_at FROM agent_nodes WHERE id = ? AND run_id = ?")
            .bind(fallback.to_string()).bind(run.id.to_string()).fetch_optional(&mut **transaction).await?
            .ok_or(StoreError::NativeAgentIdentityConflict)?.into_domain()?;
        if agent.parent_id.is_some()
            || run.native_session_id.as_deref() != Some(observation.native_session_id.as_str())
        {
            return Err(StoreError::NativeAgentIdentityConflict);
        }
        agent
    };
    if run.provider == ProviderId::Codex {
        let current = if agent.parent_id.is_some() {
            sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM native_child_turns WHERE agent_id = ? AND native_turn_id = ? AND ended = 0)")
                .bind(agent.id.to_string()).bind(&observation.native_turn_id).fetch_one(&mut **transaction).await?
        } else {
            let turn: Option<String> = sqlx::query_scalar("SELECT json_extract(payload_json, '$.nativeTurnId') FROM events WHERE run_id = ? AND agent_id = ? AND kind = 'lifecycle' AND json_valid(payload_json) AND json_type(payload_json, '$.nativeTurnId') = 'text' ORDER BY sequence DESC LIMIT 1")
                .bind(run.id.to_string()).bind(agent.id.to_string()).fetch_optional(&mut **transaction).await?;
            turn.is_some() && turn == observation.native_turn_id
        };
        if !current {
            return Err(StoreError::NativeAgentIdentityConflict);
        }
    } else if observation.native_turn_id.is_some() {
        return Err(StoreError::NativeAgentIdentityConflict);
    }
    if !matches!(agent.status, AgentStatus::Running | AgentStatus::Waiting)
        || !matches!(run.status, RunStatus::Running | RunStatus::Waiting)
    {
        return Err(StoreError::NativeAgentIdentityConflict);
    }
    Ok(agent.id)
}

/// The direct and approval-drain paths share the same durable replay decision.
pub(super) async fn persist(
    transaction: &mut Transaction<'_, Sqlite>,
    conversation_id: ConversationId,
    run_id: RunId,
    agent_id: AgentId,
    provider: ProviderId,
    observation: &CompactionObservation,
) -> Result<TimelineEvent, StoreError> {
    observation
        .validate()
        .map_err(|error| StoreError::InvalidData {
            entity: "compaction",
            detail: error.to_string(),
        })?;
    let key = match &observation.phase {
        CompactionPhase::Completed { boundary_id } => Some(
            serde_json::json!([provider, observation.native_session_id, boundary_id]).to_string(),
        ),
        _ => None,
    };
    let existing = if let Some(key) = &key {
        sqlx::query_as::<_, TimelineEventRow>("SELECT id, conversation_id, run_id, agent_id, sequence, kind, role, content FROM events WHERE conversation_id = ? AND compaction_key = ?")
            .bind(conversation_id.to_string()).bind(key).fetch_optional(&mut **transaction).await?
    } else {
        let latest: Option<(String, String)> = sqlx::query_as("SELECT id, payload_json FROM events WHERE run_id = ? AND agent_id = ? AND json_valid(payload_json) AND json_type(payload_json, '$.compaction') = 'object' ORDER BY sequence DESC LIMIT 1")
            .bind(run_id.to_string()).bind(agent_id.to_string()).fetch_optional(&mut **transaction).await?;
        if let Some((id, payload)) = latest
            && from_payload(&payload)?.as_ref() == Some(observation)
        {
            sqlx::query_as::<_, TimelineEventRow>("SELECT id, conversation_id, run_id, agent_id, sequence, kind, role, content FROM events WHERE id = ?")
                .bind(id).fetch_optional(&mut **transaction).await?
        } else {
            None
        }
    };
    if let Some(existing) = existing {
        return existing.into_domain();
    }
    let id = TimelineEventId::new();
    let (kind, content) = event_fields(&observation.phase);
    let now = now_millis();
    let sequence = sqlx::query("INSERT INTO events (id, conversation_id, run_id, agent_id, kind, content, payload_json, native_turn_id, compaction_key, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(id.to_string()).bind(conversation_id.to_string()).bind(run_id.to_string()).bind(agent_id.to_string())
        .bind(event_kind_label(kind)).bind(content).bind(serde_json::json!({"compaction":observation}).to_string())
        .bind(&observation.native_turn_id).bind(key).bind(now).execute(&mut **transaction).await?.last_insert_rowid();
    sqlx::query("UPDATE conversations SET updated_at = ? WHERE id = ?")
        .bind(now)
        .bind(conversation_id.to_string())
        .execute(&mut **transaction)
        .await?;
    Ok(TimelineEvent {
        id,
        conversation_id,
        run_id,
        agent_id,
        sequence: u64::try_from(sequence).map_err(|_| StoreError::InvalidData {
            entity: "compaction sequence",
            detail: "negative sequence".into(),
        })?,
        kind,
        role: None,
        content: content.into(),
    })
}

impl Store {
    /// Current activity is independent of the selected transcript page.
    pub async fn load_active_compaction(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Option<CompactionActivity>, StoreError> {
        let binding = self.conversation_binding(conversation_id).await?;
        // A recovery claim fences abandoned work; it does not resume native execution.
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT e.run_id, e.agent_id, r.provider FROM events e JOIN provider_runs r ON r.id = e.run_id JOIN agent_nodes a ON a.id = e.agent_id \
             WHERE r.status IN ('running', 'waiting') AND a.status IN ('running', 'waiting') \
             AND r.dispatch_owner_id IS NOT NULL AND r.dispatch_owner_id NOT GLOB 'recovery:*' \
             AND r.dispatch_lease_expires_at >= ",
        );
        query.push_bind(now_millis());
        match binding {
            ConversationBinding::Managed => {
                query
                    .push(" AND e.conversation_id = ")
                    .push_bind(conversation_id.to_string())
                    .push(" AND a.parent_id IS NULL");
            }
            ConversationBinding::Observed {
                run_id, agent_id, ..
            } => {
                query
                    .push(" AND e.run_id = ")
                    .push_bind(run_id.to_string())
                    .push(" AND e.agent_id = ")
                    .push_bind(agent_id.to_string());
            }
        }
        query.push(" AND json_valid(e.payload_json) AND json_extract(e.payload_json, '$.compaction.phase.kind') = 'started' \
            AND NOT EXISTS (SELECT 1 FROM events later WHERE later.run_id = e.run_id AND later.agent_id = e.agent_id AND later.sequence > e.sequence AND json_valid(later.payload_json) AND json_type(later.payload_json, '$.compaction') = 'object') \
            AND ((a.parent_id IS NULL AND json_extract(e.payload_json, '$.compaction.nativeSessionId') = r.native_session_id) OR (a.parent_id IS NOT NULL AND json_extract(e.payload_json, '$.compaction.nativeSessionId') = a.provider_native_id)) \
            AND (r.provider <> 'codex' OR (a.parent_id IS NOT NULL AND EXISTS(SELECT 1 FROM native_child_turns t WHERE t.agent_id = a.id AND t.native_turn_id = e.native_turn_id AND t.ended = 0)) \
                OR (a.parent_id IS NULL AND e.native_turn_id = (SELECT json_extract(s.payload_json, '$.nativeTurnId') FROM events s WHERE s.run_id = e.run_id AND s.agent_id = e.agent_id AND s.kind = 'lifecycle' AND json_valid(s.payload_json) AND json_type(s.payload_json, '$.nativeTurnId') = 'text' ORDER BY s.sequence DESC LIMIT 1))) \
            ORDER BY e.sequence DESC LIMIT 1");
        query
            .build_query_as::<(String, String, String)>()
            .fetch_optional(&self.pool)
            .await?
            .map(|(run, agent, provider)| {
                Ok(CompactionActivity {
                    run_id: parse_uuid("run", &run)?.into(),
                    agent_id: parse_uuid("agent", &agent)?.into(),
                    provider: parse_provider(&provider)?,
                })
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::context_compaction::{CompactionObservation, CompactionPhase};

    fn record(phase: CompactionPhase) -> ProviderEventRecord {
        ProviderEventRecord::Compaction {
            observation: CompactionObservation {
                native_session_id: "session".into(),
                native_turn_id: Some("turn".into()),
                native_agent_id: None,
                phase,
            },
        }
    }

    async fn fixture() -> (Store, ConversationId, RunId, AgentId) {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("compaction"))
            .await
            .unwrap();
        let (run, agent) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store.bind_native_session(run.id, "session").await.unwrap();
        store
            .claim_provider_dispatch(run.id, "owner", Duration::from_secs(120))
            .await
            .unwrap();
        store
            .append_owned_run_event(
                run.id,
                agent.id,
                "owner",
                ProviderEventRecord::started_with_native_id("turn"),
            )
            .await
            .unwrap();
        (store, conversation.id, run.id, agent.id)
    }

    #[tokio::test]
    async fn compaction_states_preserve_history_and_duplicate_completion_is_noop() {
        let (store, conversation, run, agent) = fixture().await;
        let message = store
            .append_owned_run_event(
                run,
                agent,
                "owner",
                ProviderEventRecord::message("original transcript\nexact"),
            )
            .await
            .unwrap();
        let append = |phase| store.append_owned_run_event(run, agent, "owner", record(phase));
        let first = append(CompactionPhase::Started).await.unwrap();
        assert_eq!(append(CompactionPhase::Started).await.unwrap(), first);
        assert!(
            store
                .load_active_compaction(conversation)
                .await
                .unwrap()
                .is_some()
        );
        append(CompactionPhase::Cleared).await.unwrap();
        assert!(
            store
                .load_active_compaction(conversation)
                .await
                .unwrap()
                .is_none()
        );
        append(CompactionPhase::Started).await.unwrap();
        append(CompactionPhase::Failed).await.unwrap();
        assert!(
            store
                .load_active_compaction(conversation)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store.load_run(run).await.unwrap().status,
            RunStatus::Running
        );
        assert!(
            !store
                .load_recent_timeline(conversation, None, 200)
                .await
                .unwrap()
                .items
                .iter()
                .any(|row| row.event.content == "Context compacted")
        );
        let completion = append(CompactionPhase::Completed {
            boundary_id: "boundary".into(),
        })
        .await
        .unwrap();
        assert_eq!(completion.content, "Context compacted");
        append(CompactionPhase::Started).await.unwrap();
        assert_eq!(
            append(CompactionPhase::Completed {
                boundary_id: "boundary".into()
            })
            .await
            .unwrap(),
            completion
        );
        assert!(
            store
                .load_active_compaction(conversation)
                .await
                .unwrap()
                .is_some()
        );
        let page = store
            .load_recent_timeline(conversation, None, 200)
            .await
            .unwrap();
        assert!(page.items.iter().any(|row| row.event == message));
        assert_eq!(
            page.items
                .iter()
                .filter(|row| row.event.content == "Context compacted")
                .count(),
            1
        );
        assert!(!page.items.iter().any(|row| row.event.id == first.id));
        store
            .append_owned_run_event(run, agent, "owner", ProviderEventRecord::completed())
            .await
            .unwrap();
        assert!(
            store
                .load_active_compaction(conversation)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn compaction_rejects_wrong_session_turn_and_dispatch_owner() {
        let (store, conversation, run, agent) = fixture().await;
        for (session, turn) in [("other", "turn"), ("session", "stale")] {
            let mut observation = match record(CompactionPhase::Started) {
                ProviderEventRecord::Compaction { observation } => observation,
                _ => unreachable!(),
            };
            observation.native_session_id = session.into();
            observation.native_turn_id = Some(turn.into());
            assert!(
                store
                    .append_owned_run_event(
                        run,
                        agent,
                        "owner",
                        ProviderEventRecord::Compaction { observation }
                    )
                    .await
                    .is_err()
            );
        }
        assert!(
            store
                .append_owned_run_event(run, agent, "former", record(CompactionPhase::Started))
                .await
                .is_err()
        );
        assert!(
            store
                .load_active_compaction(conversation)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn compaction_staged_duplicates_coalesce_and_terminal_drain_retains_completion() {
        let (store, conversation, run, agent) = fixture().await;
        store
            .append_owned_run_event(run, agent, "owner", ProviderEventRecord::waiting())
            .await
            .unwrap();
        for _ in 0..300 {
            let outcome = store
                .stage_owned_waiting_event(run, agent, "owner", record(CompactionPhase::Started))
                .await
                .unwrap();
            assert!(
                matches!(outcome, StageWaitingEventOutcome::Staged(_)),
                "identical starts must coalesce before queue overflow"
            );
        }
        for _ in 0..2 {
            store
                .stage_owned_waiting_event(
                    run,
                    agent,
                    "owner",
                    record(CompactionPhase::Completed {
                        boundary_id: "staged".into(),
                    }),
                )
                .await
                .unwrap();
        }
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM staged_provider_events WHERE run_id = ?")
                .bind(run.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(count, 2);
        store
            .append_owned_run_event(run, agent, "owner", ProviderEventRecord::interrupted())
            .await
            .unwrap();
        assert!(
            store
                .load_active_compaction(conversation)
                .await
                .unwrap()
                .is_none()
        );
        let page = store
            .load_recent_timeline(conversation, None, 200)
            .await
            .unwrap();
        assert_eq!(
            page.items
                .iter()
                .filter(|row| row.event.content == "Context compacted")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn compaction_staged_replay_cannot_clear_newer_start() {
        let (store, conversation, run, agent) = fixture().await;
        let completed = || {
            record(CompactionPhase::Completed {
                boundary_id: "old".into(),
            })
        };
        let original = store
            .append_owned_run_event(run, agent, "owner", completed())
            .await
            .unwrap();
        store
            .append_owned_run_event(
                run,
                agent,
                "owner",
                ProviderEventRecord::approval_requested(
                    ProviderId::Codex,
                    "approval",
                    "operation",
                    "scope",
                ),
            )
            .await
            .unwrap();
        for event in [record(CompactionPhase::Started), completed()] {
            store
                .stage_owned_waiting_event(run, agent, "owner", event)
                .await
                .unwrap();
        }
        store
            .record_response_intent(run, agent, "approval", ApprovalResolution::Approved)
            .await
            .unwrap();
        store
            .acknowledge_response_intent(run, agent, "approval")
            .await
            .unwrap();
        assert!(
            store
                .load_active_compaction(conversation)
                .await
                .unwrap()
                .is_some()
        );
        let page = store
            .load_recent_timeline(conversation, None, 200)
            .await
            .unwrap();
        let completions = page
            .items
            .iter()
            .filter(|item| item.event.content == "Context compacted")
            .collect::<Vec<_>>();
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].event, original);
    }

    #[tokio::test]
    async fn compaction_completion_identity_survives_next_run() {
        let (store, conversation, run, agent) = fixture().await;
        let first = store
            .append_owned_run_event(
                run,
                agent,
                "owner",
                record(CompactionPhase::Completed {
                    boundary_id: "shared".into(),
                }),
            )
            .await
            .unwrap();
        store
            .append_owned_run_event(run, agent, "owner", ProviderEventRecord::completed())
            .await
            .unwrap();
        let (next, root) = store
            .create_run(conversation, ProviderId::Codex)
            .await
            .unwrap();
        store.bind_native_session(next.id, "session").await.unwrap();
        store
            .claim_provider_dispatch(next.id, "next-owner", Duration::from_secs(120))
            .await
            .unwrap();
        store
            .append_owned_run_event(
                next.id,
                root.id,
                "next-owner",
                ProviderEventRecord::started_with_native_id("turn"),
            )
            .await
            .unwrap();
        store
            .append_owned_run_event(
                next.id,
                root.id,
                "next-owner",
                record(CompactionPhase::Started),
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .append_owned_run_event(
                    next.id,
                    root.id,
                    "next-owner",
                    record(CompactionPhase::Completed {
                        boundary_id: "shared".into()
                    })
                )
                .await
                .unwrap(),
            first
        );
        assert_eq!(
            store
                .load_active_compaction(conversation)
                .await
                .unwrap()
                .unwrap()
                .run_id,
            next.id
        );
        store.expire_dispatch_lease_for_test(next.id).await.unwrap();
        assert!(
            store
                .load_active_compaction(conversation)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .claim_stale_provider_dispatch(
                    next.id,
                    "recovery:fixture",
                    "excluded",
                    Duration::from_secs(120),
                    Duration::ZERO
                )
                .await
                .unwrap()
        );
        assert!(
            store
                .load_active_compaction(conversation)
                .await
                .unwrap()
                .is_none(),
            "a recovery lease must not revive stale native compaction before interruption"
        );
        assert!(
            store
                .append_owned_run_event(
                    next.id,
                    root.id,
                    "next-owner",
                    record(CompactionPhase::Cleared)
                )
                .await
                .is_err()
        );
        store
            .append_owned_run_event(
                next.id,
                root.id,
                "recovery:fixture",
                ProviderEventRecord::interrupted(),
            )
            .await
            .unwrap();
        assert!(
            store
                .load_active_compaction(conversation)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn compaction_unowned_recovery_claim_does_not_revive_activity() {
        let (store, conversation, run, agent) = fixture().await;
        store
            .append_owned_run_event(run, agent, "owner", record(CompactionPhase::Started))
            .await
            .unwrap();
        sqlx::query("UPDATE provider_runs SET dispatch_owner_id = NULL, dispatch_lease_expires_at = NULL WHERE id = ?")
            .bind(run.to_string()).execute(&store.pool).await.unwrap();
        assert!(
            store
                .claim_unowned_provider_dispatch_recovery(
                    run,
                    "recovery:fixture",
                    Duration::from_secs(120)
                )
                .await
                .unwrap()
        );
        assert_eq!(
            store.load_run(run).await.unwrap().status,
            RunStatus::Running
        );
        assert!(
            store
                .load_active_compaction(conversation)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn compaction_child_belongs_only_to_observed_current_turn() {
        let (store, conversation, run, root) = fixture().await;
        store
            .append_owned_run_event(
                run,
                root,
                "owner",
                ProviderEventRecord::child_agent(
                    "spawn",
                    "session",
                    vec!["child".into()],
                    vec![],
                    "spawnAgent",
                    "completed",
                ),
            )
            .await
            .unwrap();
        let child: String = sqlx::query_scalar(
            "SELECT id FROM agent_nodes WHERE run_id = ? AND provider_native_id = 'child'",
        )
        .bind(run.to_string())
        .fetch_one(&store.pool)
        .await
        .unwrap();
        let child: AgentId = parse_uuid("agent", &child).unwrap().into();
        let observed: String =
            sqlx::query_scalar("SELECT id FROM conversations WHERE observed_agent_id = ?")
                .bind(child.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        let observed: ConversationId = parse_uuid("conversation", &observed).unwrap().into();
        let turn = NativeChildTurn {
            native_thread_id: "child".into(),
            native_turn_id: "child-turn".into(),
        };
        store
            .append_owned_native_child_event(
                run,
                &turn,
                "owner",
                ProviderEventRecord::started_with_native_id("child-turn"),
            )
            .await
            .unwrap();
        let observation = CompactionObservation {
            native_session_id: "child".into(),
            native_turn_id: Some("child-turn".into()),
            native_agent_id: Some("child".into()),
            phase: CompactionPhase::Started,
        };
        store
            .append_owned_run_event(
                run,
                root,
                "owner",
                ProviderEventRecord::Compaction {
                    observation: observation.clone(),
                },
            )
            .await
            .unwrap();
        assert!(
            store
                .load_active_compaction(conversation)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store
                .load_active_compaction(observed)
                .await
                .unwrap()
                .unwrap()
                .agent_id,
            child
        );
        store
            .append_owned_native_child_event(run, &turn, "owner", ProviderEventRecord::completed())
            .await
            .unwrap();
        let next = NativeChildTurn {
            native_thread_id: "child".into(),
            native_turn_id: "next".into(),
        };
        store
            .append_owned_native_child_event(
                run,
                &next,
                "owner",
                ProviderEventRecord::started_with_native_id("next"),
            )
            .await
            .unwrap();
        assert!(
            store
                .load_active_compaction(observed)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .append_owned_run_event(
                    run,
                    root,
                    "owner",
                    ProviderEventRecord::Compaction { observation }
                )
                .await
                .is_err()
        );
        let unknown = CompactionObservation {
            native_session_id: "missing".into(),
            native_turn_id: Some("next".into()),
            native_agent_id: Some("missing".into()),
            phase: CompactionPhase::Started,
        };
        assert!(
            store
                .append_owned_run_event(
                    run,
                    root,
                    "owner",
                    ProviderEventRecord::Compaction {
                        observation: unknown
                    }
                )
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn compaction_ack_drains_once_and_unpaged_activity_ignores_transcript_page() {
        let (store, conversation, run, agent) = fixture().await;
        for i in 0..4 {
            store
                .append_owned_run_event(
                    run,
                    agent,
                    "owner",
                    ProviderEventRecord::message(format!("message {i}")),
                )
                .await
                .unwrap();
        }
        store
            .append_owned_run_event(
                run,
                agent,
                "owner",
                ProviderEventRecord::approval_requested(
                    ProviderId::Codex,
                    "approval",
                    "operation",
                    "scope",
                ),
            )
            .await
            .unwrap();
        store
            .stage_owned_waiting_event(run, agent, "owner", record(CompactionPhase::Started))
            .await
            .unwrap();
        store
            .record_response_intent(run, agent, "approval", ApprovalResolution::Approved)
            .await
            .unwrap();
        store
            .acknowledge_response_intent(run, agent, "approval")
            .await
            .unwrap();
        let newest = store
            .load_recent_timeline(conversation, None, 2)
            .await
            .unwrap();
        let _older = store
            .load_recent_timeline(conversation, newest.next_cursor, 2)
            .await
            .unwrap();
        assert!(
            store
                .load_active_compaction(conversation)
                .await
                .unwrap()
                .is_some()
        );
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM staged_provider_events WHERE run_id = ?")
                .bind(run.to_string())
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn compaction_distinct_boundaries_respect_staged_queue_limit() {
        let (store, conversation, run, agent) = fixture().await;
        store
            .append_owned_run_event(run, agent, "owner", ProviderEventRecord::waiting())
            .await
            .unwrap();
        for i in 0..MAX_STAGED_EVENT_ROWS - 1 {
            assert!(matches!(
                store
                    .stage_owned_waiting_event(
                        run,
                        agent,
                        "owner",
                        record(CompactionPhase::Completed {
                            boundary_id: format!("boundary-{i}")
                        })
                    )
                    .await
                    .unwrap(),
                StageWaitingEventOutcome::Staged(_)
            ));
        }
        assert!(matches!(
            store
                .stage_owned_waiting_event(
                    run,
                    agent,
                    "owner",
                    record(CompactionPhase::Completed {
                        boundary_id: "omitted".into()
                    })
                )
                .await
                .unwrap(),
            StageWaitingEventOutcome::Overflowed(_)
        ));
        store
            .append_owned_run_event(run, agent, "owner", ProviderEventRecord::interrupted())
            .await
            .unwrap();
        let omitted: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE compaction_key LIKE '%omitted%'")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(omitted, 0);
        assert!(
            store
                .load_active_compaction(conversation)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn compaction_completion_reopen_and_provider_session_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("compaction.sqlite");
        let store = Store::open(&path).await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("reopen"))
            .await
            .unwrap()
            .id;
        let mut original = None;
        for (provider, session) in [
            (ProviderId::Codex, "session"),
            (ProviderId::Codex, "other"),
            (ProviderId::Claude, "session"),
            (ProviderId::Codex, "session"),
        ] {
            let (run, root) = store.create_run(conversation, provider).await.unwrap();
            store.bind_native_session(run.id, session).await.unwrap();
            store
                .claim_provider_dispatch(run.id, "owner", Duration::from_secs(120))
                .await
                .unwrap();
            store
                .append_owned_run_event(
                    run.id,
                    root.id,
                    "owner",
                    ProviderEventRecord::started_with_native_id("turn"),
                )
                .await
                .unwrap();
            let observation = CompactionObservation {
                native_session_id: session.into(),
                native_turn_id: (provider == ProviderId::Codex).then(|| "turn".into()),
                native_agent_id: None,
                phase: CompactionPhase::Completed {
                    boundary_id: "boundary".into(),
                },
            };
            let event = store
                .append_owned_run_event(
                    run.id,
                    root.id,
                    "owner",
                    ProviderEventRecord::Compaction { observation },
                )
                .await
                .unwrap();
            if provider == ProviderId::Codex && session == "session" {
                if let Some(previous) = original.as_ref() {
                    assert_eq!(&event, previous);
                } else {
                    original = Some(event);
                }
            }
            store
                .append_owned_run_event(run.id, root.id, "owner", ProviderEventRecord::completed())
                .await
                .unwrap();
        }
        store.pool.close().await;
        let reopened = Store::open(&path).await.unwrap();
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE compaction_key IS NOT NULL")
                .fetch_one(&reopened.pool)
                .await
                .unwrap();
        assert_eq!(count, 3);
        let (run, root) = reopened
            .create_run(conversation, ProviderId::Codex)
            .await
            .unwrap();
        reopened
            .bind_native_session(run.id, "session")
            .await
            .unwrap();
        reopened
            .claim_provider_dispatch(run.id, "owner", Duration::from_secs(120))
            .await
            .unwrap();
        reopened
            .append_owned_run_event(
                run.id,
                root.id,
                "owner",
                ProviderEventRecord::started_with_native_id("turn"),
            )
            .await
            .unwrap();
        let replay = reopened
            .append_owned_run_event(
                run.id,
                root.id,
                "owner",
                record(CompactionPhase::Completed {
                    boundary_id: "boundary".into(),
                }),
            )
            .await
            .unwrap();
        assert_eq!(Some(replay), original);
        let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as("EXPLAIN QUERY PLAN SELECT id FROM events WHERE conversation_id = ? AND compaction_key = ?")
            .bind(conversation.to_string()).bind("key").fetch_all(&reopened.pool).await.unwrap();
        assert!(
            plan.iter()
                .any(|row| row.3.contains("idx_events_compaction_key"))
        );
    }
}
