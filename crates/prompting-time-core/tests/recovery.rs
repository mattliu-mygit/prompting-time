use std::collections::{BTreeSet, HashSet};

use prompting_time_core::domain::{
    AgentId, AgentStatus, MutationState, RunId, RunStatus, TimelineEventKind,
};
use prompting_time_core::providers::ProviderId;
use prompting_time_core::store::{NewConversation, ProviderEventRecord, Store, StoreError};
use tempfile::TempDir;

#[tokio::test]
async fn event_and_run_state_commit_atomically() {
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

    let recovered = store.pending_recovery().await.unwrap();
    assert_eq!(recovered[0].run.status, RunStatus::Running);
    assert_eq!(recovered[0].agents[0].status, AgentStatus::Running);
    assert_eq!(recovered[0].events.len(), 1);
}

#[tokio::test]
async fn restart_recovers_staged_waiting_events_without_publishing_them() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("staged-recovery.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("staged recovery"))
        .await
        .unwrap();
    let (run, root) = store
        .create_run(conversation.id, ProviderId::Claude)
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
                ProviderId::Claude,
                "native-question",
                "question",
                "deployment target",
            ),
        )
        .await
        .unwrap();
    store
        .stage_waiting_event(
            run.id,
            root.id,
            ProviderEventRecord::progress("still considering"),
        )
        .await
        .unwrap();
    store
        .stage_waiting_event(
            run.id,
            root.id,
            ProviderEventRecord::tool("possibly wrote state", MutationState::Unknown),
        )
        .await
        .unwrap();
    store.close().await;

    let reopened = Store::open(&path).await.unwrap();
    let recovery = reopened
        .pending_recovery()
        .await
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.run.id == run.id)
        .unwrap();
    assert_eq!(recovery.run.status, RunStatus::Waiting);
    assert_eq!(recovery.run.mutation_state, MutationState::Unknown);
    assert!(!recovery.staged_events_overflowed);
    assert!(!recovery.staged_events_truncated);
    assert_eq!(
        recovery
            .staged_events
            .iter()
            .map(|event| (event.kind, event.content.as_str(), event.mutation_state))
            .collect::<Vec<_>>(),
        [
            (TimelineEventKind::Progress, "still considering", None,),
            (
                TimelineEventKind::Tool,
                "possibly wrote state",
                Some(MutationState::Unknown),
            ),
        ]
    );
    let timeline = reopened
        .load_timeline(conversation.id, None, 20)
        .await
        .unwrap();
    assert_eq!(timeline.items.len(), 2);
    assert!(
        timeline
            .items
            .iter()
            .all(|event| !event.content.starts_with("still")
                && !event.content.starts_with("possibly"))
    );
}

#[tokio::test]
async fn timeline_uses_stable_cursor_pagination() {
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
    for index in 0..124 {
        store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::progress(format!("event {index}")),
            )
            .await
            .unwrap();
    }

    let first = store
        .load_timeline(conversation.id, None, 50)
        .await
        .unwrap();
    store
        .append_run_event(run.id, root.id, ProviderEventRecord::progress("event 125"))
        .await
        .unwrap();
    let second = store
        .load_timeline(conversation.id, first.next_cursor, 50)
        .await
        .unwrap();
    let third = store
        .load_timeline(conversation.id, second.next_cursor.clone(), 50)
        .await
        .unwrap();
    assert_eq!(first.items.len(), 50);
    assert_eq!(second.items.len(), 50);
    assert_eq!(third.items.len(), 26);
    assert!(first.items.last().unwrap().sequence < second.items.first().unwrap().sequence);
    assert!(second.items.last().unwrap().sequence < third.items.first().unwrap().sequence);
    assert!(third.next_cursor.is_none());
}

#[tokio::test]
async fn invalid_transition_rolls_back_its_event() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("Test"))
        .await
        .unwrap();
    let (run, root) = store
        .create_run(conversation.id, ProviderId::Codex)
        .await
        .unwrap();

    let error = store
        .append_run_event(run.id, root.id, ProviderEventRecord::completed())
        .await
        .unwrap_err();

    assert!(matches!(error, StoreError::Domain(_)));
    let timeline = store
        .load_timeline(conversation.id, None, 50)
        .await
        .unwrap();
    assert!(timeline.items.is_empty());
    let recovered = store.pending_recovery().await.unwrap();
    assert_eq!(recovered[0].run.status, RunStatus::Queued);
}

#[tokio::test]
async fn progress_events_require_a_running_run() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("Test"))
        .await
        .unwrap();
    let (run, root) = store
        .create_run(conversation.id, ProviderId::Codex)
        .await
        .unwrap();

    assert!(
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::progress("too early"))
            .await
            .is_err()
    );
    store
        .append_run_event(run.id, root.id, ProviderEventRecord::started())
        .await
        .unwrap();
    store
        .append_run_event(run.id, root.id, ProviderEventRecord::completed())
        .await
        .unwrap();
    assert!(
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::progress("too late"))
            .await
            .is_err()
    );

    let timeline = store
        .load_timeline(conversation.id, None, 50)
        .await
        .unwrap();
    assert_eq!(timeline.items.len(), 2);
}

#[tokio::test]
async fn resumed_event_is_rejected_before_a_run_has_waited() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("Test"))
        .await
        .unwrap();
    let (run, root) = store
        .create_run(conversation.id, ProviderId::Codex)
        .await
        .unwrap();

    let error = store
        .append_run_event(run.id, root.id, ProviderEventRecord::resumed())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        StoreError::InvalidEventState {
            event: "resumed",
            status: "queued"
        }
    ));
    let recovered = store.pending_recovery().await.unwrap();
    assert_eq!(recovered[0].run.status, RunStatus::Queued);
    assert_eq!(recovered[0].agents[0].status, AgentStatus::Queued);
    assert!(recovered[0].events.is_empty());
}

#[tokio::test]
async fn started_event_is_rejected_after_a_run_has_waited() {
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
    store
        .append_run_event(run.id, root.id, ProviderEventRecord::waiting())
        .await
        .unwrap();

    let error = store
        .append_run_event(run.id, root.id, ProviderEventRecord::started())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        StoreError::InvalidEventState {
            event: "started",
            status: "waiting"
        }
    ));
    let recovered = store.pending_recovery().await.unwrap();
    assert_eq!(recovered[0].run.status, RunStatus::Waiting);
    assert_eq!(recovered[0].agents[0].status, AgentStatus::Waiting);
    assert_eq!(recovered[0].events.len(), 2);
}

#[tokio::test]
async fn pagination_rejects_invalid_limits_and_cursors() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("Test"))
        .await
        .unwrap();

    assert!(matches!(
        store.load_timeline(conversation.id, None, 0).await,
        Err(StoreError::InvalidPageLimit(0))
    ));
    assert!(matches!(
        store.load_timeline(conversation.id, None, 201).await,
        Err(StoreError::InvalidPageLimit(201))
    ));
    assert!(matches!(
        store
            .load_timeline(conversation.id, Some("not-a-cursor".to_owned()), 50)
            .await,
        Err(StoreError::InvalidCursor)
    ));
}

#[tokio::test]
async fn conversation_listing_is_bounded_and_paginated() {
    let store = Store::open_in_memory().await.unwrap();
    for title in ["First", "Second", "Third"] {
        store
            .create_conversation(NewConversation::projectless(title))
            .await
            .unwrap();
    }

    let first = store.list_conversations(None, 2).await.unwrap();
    let second = store
        .list_conversations(first.next_cursor, 2)
        .await
        .unwrap();

    assert_eq!(first.items.len(), 2);
    assert_eq!(second.items.len(), 1);
    let ids = first
        .items
        .iter()
        .chain(&second.items)
        .map(|conversation| conversation.id)
        .collect::<HashSet<_>>();
    assert_eq!(ids.len(), 3);
    assert!(matches!(
        store.list_conversations(None, 201).await,
        Err(StoreError::InvalidPageLimit(201))
    ));
}

#[tokio::test]
async fn restart_recovers_only_non_terminal_runs() {
    let directory = TempDir::new().unwrap();
    let path = directory
        .path()
        .join("nested")
        .join("prompting-time.sqlite3");
    let store = Store::open(&path).await.unwrap();

    let _queued = create_run(&store, "Queued").await;
    let running = create_run(&store, "Running").await;
    append(&store, running, ProviderEventRecord::started()).await;
    let waiting = create_run(&store, "Waiting").await;
    append(&store, waiting, ProviderEventRecord::started()).await;
    append(&store, waiting, ProviderEventRecord::waiting()).await;
    let completed = create_run(&store, "Completed").await;
    append(&store, completed, ProviderEventRecord::started()).await;
    append(&store, completed, ProviderEventRecord::completed()).await;
    let interrupted = create_run(&store, "Interrupted").await;
    append(&store, interrupted, ProviderEventRecord::interrupted()).await;
    let failed = create_run(&store, "Failed").await;
    append(
        &store,
        failed,
        ProviderEventRecord::failed("provider exited"),
    )
    .await;

    store.close().await;
    let reopened = Store::open(&path).await.unwrap();
    let recovered = reopened.pending_recovery().await.unwrap();
    let statuses = recovered
        .iter()
        .map(|item| item.run.status)
        .collect::<Vec<_>>();

    assert_eq!(recovered.len(), 3);
    assert!(statuses.contains(&RunStatus::Queued));
    assert!(statuses.contains(&RunStatus::Running));
    assert!(statuses.contains(&RunStatus::Waiting));
    assert!(!statuses.iter().any(|status| matches!(
        status,
        RunStatus::Completed | RunStatus::Interrupted | RunStatus::Failed
    )));
}

#[tokio::test]
async fn recovery_bounds_event_history_and_reports_truncation() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("Many events"))
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
    for index in 0..201 {
        store
            .append_run_event(
                run.id,
                root.id,
                ProviderEventRecord::progress(format!("event {index}")),
            )
            .await
            .unwrap();
    }

    let recovered = store.pending_recovery().await.unwrap();
    assert_eq!(recovered[0].events.len(), 200);
    assert!(recovered[0].events_truncated);
}

#[tokio::test]
async fn recovery_batches_more_than_200_active_runs_without_omission() {
    let store = Store::open_in_memory().await.unwrap();
    let mut expected = HashSet::new();
    for index in 0..205 {
        let conversation = store
            .create_conversation(NewConversation::projectless(format!("Run {index}")))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        if index >= 70 {
            store
                .append_run_event(run.id, root.id, ProviderEventRecord::started())
                .await
                .unwrap();
            if index >= 140 {
                store
                    .append_run_event(run.id, root.id, ProviderEventRecord::waiting())
                    .await
                    .unwrap();
            }
        }
        expected.insert(run.id);
    }

    let recovered = store.pending_recovery().await.unwrap();
    let actual = recovered
        .iter()
        .map(|item| item.run.id)
        .collect::<HashSet<_>>();

    assert_eq!(recovered.len(), 205);
    assert_eq!(actual, expected);
    assert_eq!(
        recovered
            .iter()
            .filter(|item| item.run.status == RunStatus::Queued)
            .count(),
        70
    );
    assert_eq!(
        recovered
            .iter()
            .filter(|item| item.run.status == RunStatus::Running)
            .count(),
        70
    );
    assert_eq!(
        recovered
            .iter()
            .filter(|item| item.run.status == RunStatus::Waiting)
            .count(),
        65
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writers_allocate_each_sequence_once() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("prompting-time.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("Concurrent"))
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

    let mut writers = Vec::new();
    for index in 0..16 {
        let writer = store.clone();
        writers.push(tokio::spawn(async move {
            writer
                .append_run_event(
                    run.id,
                    root.id,
                    ProviderEventRecord::progress(format!("event {index}")),
                )
                .await
                .unwrap()
        }));
    }

    let mut written_sequences = BTreeSet::new();
    for writer in writers {
        written_sequences.insert(writer.await.unwrap().sequence);
    }
    let timeline = store
        .load_timeline(conversation.id, None, 50)
        .await
        .unwrap();
    let persisted_sequences = timeline
        .items
        .iter()
        .filter(|event| event.kind == TimelineEventKind::Progress)
        .map(|event| event.sequence)
        .collect::<BTreeSet<_>>();

    assert_eq!(written_sequences.len(), 16);
    assert_eq!(persisted_sequences, written_sequences);
    assert_eq!(persisted_sequences.len(), 16);
}

async fn create_run(store: &Store, title: &str) -> (RunId, AgentId) {
    let conversation = store
        .create_conversation(NewConversation::projectless(title))
        .await
        .unwrap();
    let (run, root) = store
        .create_run(conversation.id, ProviderId::Codex)
        .await
        .unwrap();
    (run.id, root.id)
}

async fn append(store: &Store, ids: (RunId, AgentId), event: ProviderEventRecord) {
    store.append_run_event(ids.0, ids.1, event).await.unwrap();
}
