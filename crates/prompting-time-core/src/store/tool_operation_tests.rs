use super::*;
use crate::tool_operation::{ToolOperation, ToolOperationStatus};

#[tokio::test]
async fn operation_from_ended_child_turn_cannot_revive_when_agent_runs_again() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("child turn"))
        .await
        .unwrap();
    let (run, root) = store
        .create_run(conversation.id, ProviderId::Codex)
        .await
        .unwrap();
    store.bind_native_session(run.id, "session").await.unwrap();
    store
        .claim_provider_dispatch(run.id, "owner", Duration::from_secs(60))
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
                "session",
                vec!["child".into()],
                vec![],
                "spawn",
                "running",
            ),
        )
        .await
        .unwrap();
    let native = NativeChildTurn {
        native_thread_id: "child".into(),
        native_turn_id: "first".into(),
    };
    store
        .append_owned_native_child_event(
            run.id,
            &native,
            "owner",
            ProviderEventRecord::started_with_native_id("first"),
        )
        .await
        .unwrap();
    let mut record = observation("item", Some("first"), ToolOperationStatus::Running);
    if let ProviderEventRecord::NativeItem {
        native_agent_id, ..
    } = &mut record
    {
        *native_agent_id = Some("child".into());
    }
    let operation = store
        .append_owned_run_event(run.id, root.id, "owner", record.clone())
        .await
        .unwrap();
    if let ProviderEventRecord::NativeItem { native_turn_id, .. } = &mut record {
        *native_turn_id = Some("wrong".into());
    }
    assert!(matches!(
        store
            .append_owned_run_event(run.id, root.id, "owner", record)
            .await,
        Err(StoreError::NativeAgentIdentityConflict)
    ));
    store
        .append_owned_native_child_event(run.id, &native, "owner", ProviderEventRecord::Completed)
        .await
        .unwrap();
    let next = NativeChildTurn {
        native_thread_id: "child".into(),
        native_turn_id: "second".into(),
    };
    store
        .append_owned_native_child_event(
            run.id,
            &next,
            "owner",
            ProviderEventRecord::started_with_native_id("second"),
        )
        .await
        .unwrap();
    let child_id = store
        .list_child_conversations(conversation.id, None, 1)
        .await
        .unwrap()
        .items[0]
        .id;
    let row = store
        .load_recent_timeline(child_id, None, 100)
        .await
        .unwrap()
        .items
        .into_iter()
        .find(|row| row.event.id == operation.id)
        .unwrap();
    assert_eq!(row.operation.unwrap().status, ToolOperationStatus::Unknown);
    assert_eq!(
        store
            .load_event_detail(operation.id)
            .await
            .unwrap()
            .operation
            .unwrap()
            .operation
            .status,
        ToolOperationStatus::Unknown
    );
}

#[tokio::test]
async fn unified_conversation_child_operation_refresh_preserves_sequence_and_detail_revision() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("child operation"))
        .await
        .unwrap();
    let (run, root) = store
        .create_run(conversation.id, ProviderId::Claude)
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
            ProviderEventRecord::child_agent(
                "spawn",
                "session",
                vec!["child".into()],
                vec![],
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
                "start",
                "child",
                "root/child",
                NativeSubAgentActivityKind::Started,
            ),
        )
        .await
        .unwrap();
    let child = store
        .list_child_conversations(conversation.id, None, 1)
        .await
        .unwrap()
        .items[0]
        .id;
    let mut record = observation("operation", None, ToolOperationStatus::Running);
    if let ProviderEventRecord::NativeItem {
        native_agent_id, ..
    } = &mut record
    {
        *native_agent_id = Some("child".into());
    }
    let event = store
        .append_run_event(run.id, root.id, record.clone())
        .await
        .unwrap();
    let before = store
        .load_recent_timeline(child, None, 80)
        .await
        .unwrap()
        .items
        .into_iter()
        .find(|row| row.event.id == event.id)
        .unwrap();
    if let ProviderEventRecord::NativeItem {
        operation: Some(operation),
        ..
    } = &mut record
    {
        operation.status = ToolOperationStatus::Succeeded;
        operation.output = Some("captured result".into());
    }
    let updated = store
        .append_run_event(run.id, root.id, record)
        .await
        .unwrap();
    let after = store
        .load_recent_timeline(child, None, 80)
        .await
        .unwrap()
        .items
        .into_iter()
        .find(|row| row.event.id == event.id)
        .unwrap();
    assert_eq!(updated.id, event.id);
    assert_eq!(before.event.sequence, after.event.sequence);
    assert_eq!(after.event.conversation_id, child);
    assert!(
        after.operation.as_ref().unwrap().detail_revision
            > before.operation.unwrap().detail_revision
    );
    assert_eq!(
        after.operation.unwrap().status,
        ToolOperationStatus::Succeeded
    );
    assert_eq!(
        store
            .load_event_detail(event.id)
            .await
            .unwrap()
            .operation
            .unwrap()
            .operation
            .output
            .as_deref(),
        Some("captured result")
    );
    assert!(
        !store
            .load_recent_timeline(conversation.id, None, 80)
            .await
            .unwrap()
            .items
            .iter()
            .any(|row| row.event.id == event.id)
    );
}

#[tokio::test]
async fn operation_queue_counts_serialized_detail_and_overflow_keeps_mutation() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("escaped output"))
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
        .append_run_event(run.id, root.id, ProviderEventRecord::Waiting)
        .await
        .unwrap();
    let mut overflowed = false;
    for index in 0..100 {
        let mut record = observation(&index.to_string(), None, ToolOperationStatus::Running);
        if let ProviderEventRecord::NativeItem {
            operation: Some(operation),
            mutation,
            ..
        } = &mut record
        {
            operation.output = Some("\0".repeat(300_000));
            *mutation = MutationState::Observed;
        }
        if matches!(
            store
                .stage_waiting_event(run.id, root.id, record)
                .await
                .unwrap(),
            StageWaitingEventOutcome::Overflowed(_)
        ) {
            overflowed = true;
            break;
        }
    }
    assert!(overflowed);
    let bytes: i64 = sqlx::query_scalar("SELECT SUM(length(CAST(content AS BLOB)) + coalesce(length(CAST(payload_json AS BLOB)), 0)) FROM staged_provider_events WHERE run_id = ?").bind(run.id.to_string()).fetch_one(&store.pool).await.unwrap();
    assert!(bytes <= MAX_STAGED_EVENT_BYTES as i64);
    assert_eq!(
        store.load_run(run.id).await.unwrap().mutation_state,
        MutationState::Unknown
    );
}

#[tokio::test]
async fn claude_operation_owner_is_materialized_child_and_never_root_fallback() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("owner"))
        .await
        .unwrap();
    let (run, root) = store
        .create_run(conversation.id, ProviderId::Claude)
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
            ProviderEventRecord::child_agent(
                "spawn",
                "session",
                vec!["child".into()],
                vec![],
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
                "task",
                "child",
                "root/child",
                NativeSubAgentActivityKind::Started,
            ),
        )
        .await
        .unwrap();
    let mut child_record = observation("same", None, ToolOperationStatus::Running);
    if let ProviderEventRecord::NativeItem {
        native_agent_id, ..
    } = &mut child_record
    {
        *native_agent_id = Some("child".into());
    }
    let child = store
        .append_run_event(run.id, root.id, child_record.clone())
        .await
        .unwrap();
    let root_event = store
        .append_run_event(
            run.id,
            root.id,
            observation("same", None, ToolOperationStatus::Running),
        )
        .await
        .unwrap();
    assert_ne!(child.agent_id, root.id);
    assert_ne!(child.id, root_event.id);
    if let ProviderEventRecord::NativeItem {
        native_agent_id, ..
    } = &mut child_record
    {
        *native_agent_id = Some("missing".into());
    }
    assert!(matches!(
        store.append_run_event(run.id, root.id, child_record).await,
        Err(StoreError::NativeAgentIdentityConflict)
    ));
}

fn observation(item: &str, turn: Option<&str>, status: ToolOperationStatus) -> ProviderEventRecord {
    let mut operation = ToolOperation::new("Run invented-check", status);
    if status == ToolOperationStatus::Succeeded {
        operation.output = Some("READY\n".into());
    }
    ProviderEventRecord::NativeItem {
        native_item_id: item.into(),
        native_agent_id: None,
        native_turn_id: turn.map(str::to_owned),
        operation: Some(operation),
        content: "legacy label".into(),
        mutation: MutationState::NoneObserved,
    }
}

#[tokio::test]
async fn operation_updates_remain_visible_in_an_older_bounded_page() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("older operations"))
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
    let operation = store
        .append_run_event(
            run.id,
            root.id,
            observation("old", None, ToolOperationStatus::Running),
        )
        .await
        .unwrap();
    for _ in 0..4 {
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::progress("later"))
            .await
            .unwrap();
    }
    let latest = store
        .load_recent_timeline(conversation.id, None, 4)
        .await
        .unwrap();
    assert!(!latest.items.iter().any(|row| row.operation.is_some()));
    let before = store
        .load_recent_timeline(conversation.id, latest.next_cursor.clone(), 4)
        .await
        .unwrap();
    let updated = store
        .append_run_event(
            run.id,
            root.id,
            observation("old", None, ToolOperationStatus::Succeeded),
        )
        .await
        .unwrap();
    let after = store
        .load_recent_timeline(conversation.id, latest.next_cursor, 4)
        .await
        .unwrap();
    assert_eq!(operation.id, updated.id);
    assert_eq!(
        before.items.last().unwrap().event.id,
        after.items.last().unwrap().event.id
    );
    assert_eq!(
        after
            .items
            .last()
            .unwrap()
            .operation
            .as_ref()
            .unwrap()
            .status,
        ToolOperationStatus::Succeeded
    );
}

#[tokio::test]
async fn operation_identity_merges_in_place_and_scopes_turns_and_runs() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("operations"))
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
    let started = store
        .append_run_event(
            run.id,
            root.id,
            observation("a", Some("turn"), ToolOperationStatus::Running),
        )
        .await
        .unwrap();
    let completed = store
        .append_run_event(
            run.id,
            root.id,
            observation("a", Some("turn"), ToolOperationStatus::Succeeded),
        )
        .await
        .unwrap();
    assert_eq!(completed.id, started.id);
    assert_eq!(completed.sequence, started.sequence);
    let page = store
        .load_recent_timeline(conversation.id, None, 100)
        .await
        .unwrap();
    assert_eq!(
        page.items
            .iter()
            .filter(|row| row.operation.is_some())
            .count(),
        1
    );
    let detail = store
        .load_event_detail(started.id)
        .await
        .unwrap()
        .operation
        .unwrap();
    assert_eq!(detail.operation.output.as_deref(), Some("READY\n"));
    let duplicate = store
        .append_run_event(
            run.id,
            root.id,
            observation("a", Some("turn"), ToolOperationStatus::Succeeded),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .load_event_detail(duplicate.id)
            .await
            .unwrap()
            .operation
            .unwrap()
            .revision,
        detail.revision
    );
    for (item, turn) in [("b", Some("turn")), ("a", Some("other")), ("a", None)] {
        let other = store
            .append_run_event(
                run.id,
                root.id,
                observation(item, turn, ToolOperationStatus::Running),
            )
            .await
            .unwrap();
        assert_ne!(other.id, started.id);
    }
    let (other_run, other_root) = store
        .create_run(conversation.id, ProviderId::Codex)
        .await
        .unwrap();
    store
        .append_run_event(other_run.id, other_root.id, ProviderEventRecord::started())
        .await
        .unwrap();
    let other = store
        .append_run_event(
            other_run.id,
            other_root.id,
            observation("a", Some("turn"), ToolOperationStatus::Running),
        )
        .await
        .unwrap();
    assert_ne!(other.id, started.id);
}

#[tokio::test]
async fn staged_operations_merge_with_existing_and_terminal_settles_running() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("staged operations"))
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
    let first = store
        .append_run_event(
            run.id,
            root.id,
            observation("a", None, ToolOperationStatus::Running),
        )
        .await
        .unwrap();
    store
        .append_run_event(run.id, root.id, ProviderEventRecord::Waiting)
        .await
        .unwrap();
    for (item, status) in [
        ("a", ToolOperationStatus::Succeeded),
        ("b", ToolOperationStatus::Running),
        ("b", ToolOperationStatus::Succeeded),
        ("c", ToolOperationStatus::Running),
    ] {
        store
            .stage_waiting_event(run.id, root.id, observation(item, None, status))
            .await
            .unwrap();
    }
    store
        .append_run_event(run.id, root.id, ProviderEventRecord::Interrupted)
        .await
        .unwrap();
    let page = store
        .load_recent_timeline(conversation.id, None, 100)
        .await
        .unwrap();
    let operations: Vec<_> = page
        .items
        .iter()
        .filter(|row| row.operation.is_some())
        .collect();
    assert_eq!(operations.len(), 3);
    assert!(operations.iter().any(|row| row.event.id == first.id
        && row.operation.as_ref().unwrap().status == ToolOperationStatus::Succeeded));
    assert!(
        operations
            .iter()
            .all(|row| row.operation.as_ref().unwrap().status != ToolOperationStatus::Running)
    );
}

#[tokio::test]
async fn legacy_tools_remain_distinct_and_conflicts_project_notice() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("legacy operations"))
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
    for _ in 0..2 {
        store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::native_item("old", "old output", MutationState::NoneObserved),
            )
            .await
            .unwrap();
    }
    let done = store
        .append_run_event(
            run.id,
            root.id,
            observation("new", None, ToolOperationStatus::Failed),
        )
        .await
        .unwrap();
    store
        .append_run_event(
            run.id,
            root.id,
            observation("new", None, ToolOperationStatus::Running),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .load_event_detail(done.id)
            .await
            .unwrap()
            .operation
            .unwrap()
            .operation
            .status,
        ToolOperationStatus::Failed
    );
    store
        .append_run_event(
            run.id,
            root.id,
            observation("new", None, ToolOperationStatus::Succeeded),
        )
        .await
        .unwrap();
    let page = store
        .load_recent_timeline(conversation.id, None, 100)
        .await
        .unwrap();
    assert_eq!(
        page.items
            .iter()
            .filter(|row| row.event.content == "old output")
            .count(),
        2
    );
    assert_eq!(
        page.items
            .iter()
            .find(|row| row.event.id == done.id)
            .unwrap()
            .presentation,
        TimelinePresentation::Notice
    );
}
