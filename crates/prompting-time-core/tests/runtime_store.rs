use prompting_time_core::domain::{
    ApprovalResolution, ApprovalResponseIntentStatus, ApprovalStatus, MutationState, RunStatus,
    TimelineEventKind,
};
use prompting_time_core::providers::{DispatchCertainty, ProviderErrorCategory, ProviderId};
use prompting_time_core::store::{
    MAX_STAGED_EVENT_BYTES, MAX_STAGED_EVENT_ROWS, NewConversation, ProviderEventRecord,
    StageWaitingEventOutcome, Store, StoreError,
};
use tempfile::TempDir;

#[tokio::test]
async fn normalized_tool_event_updates_mutation_and_payload_atomically() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("runtime store"))
        .await
        .unwrap();
    let (run, root) = store
        .create_run(conversation.id, ProviderId::Codex)
        .await
        .unwrap();
    store
        .bind_native_session(run.id, "native-session-1")
        .await
        .unwrap();
    let started = store
        .append_run_event(
            run.id,
            root.id,
            ProviderEventRecord::started_with_native_id("native-turn-2"),
        )
        .await
        .unwrap();

    let event = store
        .append_run_event(
            run.id,
            root.id,
            ProviderEventRecord::tool("wrote fixture.txt", MutationState::Observed),
        )
        .await
        .unwrap();

    assert_eq!(event.kind, TimelineEventKind::Tool);
    let stored = store.load_run(run.id).await.unwrap();
    assert_eq!(
        stored.native_session_id.as_deref(),
        Some("native-session-1")
    );
    assert_eq!(stored.mutation_state, MutationState::Observed);
    assert_eq!(
        store.load_event_payload(started.id).await.unwrap().unwrap()["nativeTurnId"],
        "native-turn-2"
    );
}

#[tokio::test]
async fn waiting_events_stage_and_drain_once_in_receipt_order() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("staged waiting output"))
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
                "native-approval",
                "write",
                "fixture.txt",
            ),
        )
        .await
        .unwrap();

    for record in [
        ProviderEventRecord::message("buffered message"),
        ProviderEventRecord::progress("buffered progress"),
        ProviderEventRecord::tool("buffered write", MutationState::Observed),
    ] {
        store
            .stage_waiting_event(run.id, root.id, record)
            .await
            .unwrap();
    }

    let waiting = store.load_run(run.id).await.unwrap();
    assert_eq!(waiting.status, RunStatus::Waiting);
    assert_eq!(waiting.mutation_state, MutationState::Observed);
    let hidden = store
        .load_timeline(conversation.id, None, 20)
        .await
        .unwrap();
    assert_eq!(hidden.items.len(), 2);

    store
        .record_response_intent(
            run.id,
            root.id,
            "native-approval",
            ApprovalResolution::Approved,
        )
        .await
        .unwrap();
    store
        .acknowledge_response_intent(run.id, root.id, "native-approval")
        .await
        .unwrap();
    assert!(matches!(
        store
            .acknowledge_response_intent(run.id, root.id, "native-approval")
            .await,
        Err(StoreError::ApprovalResponseAlreadyAcknowledged)
    ));
    assert_eq!(
        store
            .load_timeline(conversation.id, None, 20)
            .await
            .unwrap()
            .items
            .into_iter()
            .map(|event| event.content)
            .collect::<Vec<_>>(),
        [
            "Provider run started",
            "write",
            "Provider run resumed",
            "buffered message",
            "buffered progress",
            "buffered write",
        ]
    );
}

#[tokio::test]
async fn acknowledgement_validates_native_request_and_survives_restart_atomically() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("atomic-acknowledgement.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("atomic acknowledgement"))
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
                "exact-native-request",
                "write",
                "fixture.txt",
            ),
        )
        .await
        .unwrap();
    store
        .stage_waiting_event(
            run.id,
            root.id,
            ProviderEventRecord::message("durable buffered output"),
        )
        .await
        .unwrap();
    store
        .record_response_intent(
            run.id,
            root.id,
            "exact-native-request",
            ApprovalResolution::Approved,
        )
        .await
        .unwrap();

    assert!(matches!(
        store
            .acknowledge_response_intent(run.id, root.id, "different-native-request")
            .await,
        Err(StoreError::NotFound {
            entity: "approval",
            ..
        })
    ));
    let waiting = store
        .pending_recovery()
        .await
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.run.id == run.id)
        .unwrap();
    assert_eq!(waiting.run.status, RunStatus::Waiting);
    assert_eq!(waiting.staged_events.len(), 1);

    store
        .acknowledge_response_intent(run.id, root.id, "exact-native-request")
        .await
        .unwrap();
    store.close().await;

    let reopened = Store::open(&path).await.unwrap();
    assert!(matches!(
        reopened
            .acknowledge_response_intent(run.id, root.id, "exact-native-request")
            .await,
        Err(StoreError::ApprovalResponseAlreadyAcknowledged)
    ));
    let recovered = reopened
        .pending_recovery()
        .await
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.run.id == run.id)
        .unwrap();
    assert_eq!(recovered.run.status, RunStatus::Running);
    assert!(recovered.staged_events.is_empty());
    assert_eq!(
        reopened
            .load_timeline(conversation.id, None, 20)
            .await
            .unwrap()
            .items
            .into_iter()
            .map(|event| event.content)
            .collect::<Vec<_>>(),
        [
            "Provider run started",
            "write",
            "Provider run resumed",
            "durable buffered output",
        ]
    );
}

#[tokio::test]
async fn staged_queue_replaces_first_row_or_byte_overflow_with_one_compact_marker() {
    for overflow_by_bytes in [false, true] {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("bounded staged output"))
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

        if overflow_by_bytes {
            let outcome = store
                .stage_waiting_event(
                    run.id,
                    root.id,
                    ProviderEventRecord::message("preserved in full"),
                )
                .await
                .unwrap();
            assert!(matches!(outcome, StageWaitingEventOutcome::Staged(_)));
            let outcome = store
                .stage_waiting_event(
                    run.id,
                    root.id,
                    ProviderEventRecord::tool(
                        "x".repeat(MAX_STAGED_EVENT_BYTES + 1),
                        MutationState::Observed,
                    ),
                )
                .await
                .unwrap();
            assert!(matches!(outcome, StageWaitingEventOutcome::Overflowed(_)));
        } else {
            for index in 0..MAX_STAGED_EVENT_ROWS - 1 {
                store
                    .stage_waiting_event(
                        run.id,
                        root.id,
                        ProviderEventRecord::progress(format!("event-{index}")),
                    )
                    .await
                    .unwrap();
            }
            let outcome = store
                .stage_waiting_event(
                    run.id,
                    root.id,
                    ProviderEventRecord::message("one row too many"),
                )
                .await
                .unwrap();
            assert!(matches!(outcome, StageWaitingEventOutcome::Overflowed(_)));
        }

        assert!(matches!(
            store
                .stage_waiting_event(
                    run.id,
                    root.id,
                    ProviderEventRecord::progress("rejected after marker"),
                )
                .await,
            Err(StoreError::StagedEventOverflowed)
        ));
        let recovery = store
            .pending_recovery()
            .await
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.run.id == run.id)
            .unwrap();
        assert!(recovery.staged_events_overflowed);
        assert!(!recovery.staged_events_truncated);
        assert!(recovery.staged_events.len() <= MAX_STAGED_EVENT_ROWS);
        let marker = recovery.staged_events.last().unwrap();
        assert_eq!(marker.kind, TimelineEventKind::Diagnostic);
        assert_eq!(marker.mutation_state, Some(MutationState::Unknown));
        assert_eq!(
            marker.overflowed_kind,
            Some(if overflow_by_bytes {
                TimelineEventKind::Tool
            } else {
                TimelineEventKind::Message
            })
        );
        assert!(marker.content.len() < 256);
        assert_eq!(recovery.run.mutation_state, MutationState::Unknown);
        if overflow_by_bytes {
            assert_eq!(recovery.staged_events[0].content, "preserved in full");
            assert_eq!(recovery.staged_events.len(), 2);
        } else {
            assert_eq!(recovery.staged_events.len(), MAX_STAGED_EVENT_ROWS);
        }
    }
}

#[tokio::test]
async fn terminal_failure_flushes_staged_events_before_its_diagnostic() {
    for reconcile in [false, true] {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("terminal staged output"))
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
            .append_run_event(run.id, root.id, ProviderEventRecord::waiting())
            .await
            .unwrap();
        store
            .stage_waiting_event(
                run.id,
                root.id,
                ProviderEventRecord::message("last buffered message"),
            )
            .await
            .unwrap();
        store
            .stage_waiting_event(
                run.id,
                root.id,
                ProviderEventRecord::tool("last buffered write", MutationState::Observed),
            )
            .await
            .unwrap();

        if reconcile {
            assert!(
                store
                    .fail_run_if_active(
                        run.id,
                        root.id,
                        ProviderErrorCategory::ContractViolation,
                        MutationState::Unknown,
                        DispatchCertainty::MayHaveDispatched,
                    )
                    .await
                    .unwrap()
            );
        } else {
            store
                .append_run_event(
                    run.id,
                    root.id,
                    ProviderEventRecord::provider_failed(
                        ProviderErrorCategory::StreamClosed,
                        MutationState::Unknown,
                        DispatchCertainty::MayHaveDispatched,
                    ),
                )
                .await
                .unwrap();
        }

        let timeline = store
            .load_timeline(conversation.id, None, 20)
            .await
            .unwrap();
        assert_eq!(
            timeline
                .items
                .iter()
                .map(|event| event.content.as_str())
                .collect::<Vec<_>>(),
            [
                "Provider run started",
                "Provider run is waiting",
                "last buffered message",
                "last buffered write",
                if reconcile {
                    "Provider failed: contract violation"
                } else {
                    "Provider failed: stream closed"
                },
            ]
        );
        assert_eq!(
            store.load_run(run.id).await.unwrap().mutation_state,
            MutationState::Unknown
        );
    }
}

#[tokio::test]
async fn approval_request_and_response_transition_durable_state_once() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("approval store"))
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
                "request-4",
                "execute",
                "fixture command",
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        store.load_run(run.id).await.unwrap().status,
        RunStatus::Waiting
    );

    store
        .record_response_intent(
            run.id,
            root.id,
            "request-4",
            ApprovalResolution::Answer("only this directory".to_owned()),
        )
        .await
        .unwrap();
    store
        .acknowledge_response_intent(run.id, root.id, "request-4")
        .await
        .unwrap();
    assert_eq!(
        store.load_run(run.id).await.unwrap().status,
        RunStatus::Running
    );
    let approval = store.load_approval(run.id, "request-4").await.unwrap();
    assert_eq!(approval.status, ApprovalStatus::Answered);
    assert_eq!(
        approval.resolution,
        Some(ApprovalResolution::Answer("only this directory".to_owned()))
    );
    assert!(
        store
            .record_response_intent(run.id, root.id, "request-4", ApprovalResolution::Denied,)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn approval_response_intent_is_recorded_before_dispatch_and_acknowledged_exactly_once() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("approval intent"))
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
                "request-intent",
                "execute",
                "fixture command",
            ),
        )
        .await
        .unwrap();

    let intended = store
        .record_response_intent(
            run.id,
            root.id,
            "request-intent",
            ApprovalResolution::Answer("only this directory".to_owned()),
        )
        .await
        .unwrap();
    assert_eq!(intended.status, ApprovalStatus::Pending);
    let intent = intended.response_intent.unwrap();
    assert_eq!(intent.status, ApprovalResponseIntentStatus::Recorded);
    assert_eq!(
        intent.resolution,
        ApprovalResolution::Answer("only this directory".to_owned())
    );
    assert!(matches!(
        store
            .record_response_intent(
                run.id,
                root.id,
                "request-intent",
                ApprovalResolution::Denied,
            )
            .await,
        Err(StoreError::ApprovalResponseIntentExists)
    ));

    let rejected = store
        .reject_response_intent(
            run.id,
            root.id,
            "request-intent",
            DispatchCertainty::NotDispatched,
        )
        .await
        .unwrap();
    assert_eq!(rejected.status, ApprovalStatus::Pending);
    assert_eq!(
        rejected.response_intent.unwrap().status,
        ApprovalResponseIntentStatus::Rejected
    );
    assert_eq!(
        store.load_run(run.id).await.unwrap().status,
        RunStatus::Waiting
    );

    store
        .record_response_intent(
            run.id,
            root.id,
            "request-intent",
            ApprovalResolution::Denied,
        )
        .await
        .unwrap();
    store
        .acknowledge_response_intent(run.id, root.id, "request-intent")
        .await
        .unwrap();

    let approval = store.load_approval(run.id, "request-intent").await.unwrap();
    assert_eq!(approval.status, ApprovalStatus::Denied);
    assert_eq!(approval.resolution, Some(ApprovalResolution::Denied));
    assert_eq!(
        approval.response_intent.unwrap().status,
        ApprovalResponseIntentStatus::Acknowledged
    );
    assert_eq!(
        store.load_run(run.id).await.unwrap().status,
        RunStatus::Running
    );
    assert!(
        store
            .acknowledge_response_intent(run.id, root.id, "request-intent")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn ambiguous_approval_dispatch_remains_durable_and_visible_to_recovery() {
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("approval-recovery.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("approval recovery"))
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
                "request-unknown",
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
            "request-unknown",
            ApprovalResolution::Approved,
        )
        .await
        .unwrap();
    store
        .reject_response_intent(
            run.id,
            root.id,
            "request-unknown",
            DispatchCertainty::MayHaveDispatched,
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
    assert_eq!(recovery.approvals.len(), 1);
    let approval = recovery.approvals.into_iter().next().unwrap();
    assert_eq!(approval.status, ApprovalStatus::Pending);
    let intent = approval.response_intent.unwrap();
    assert_eq!(intent.resolution, ApprovalResolution::Approved);
    assert_eq!(intent.status, ApprovalResponseIntentStatus::DispatchUnknown);
}

#[tokio::test]
async fn restart_recovery_retains_pending_and_recorded_native_approval_ids() {
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("native-approval-recovery.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("native approval ids"))
        .await
        .unwrap();
    let (pending_run, pending_root) = store
        .create_run(conversation.id, ProviderId::Codex)
        .await
        .unwrap();
    let (recorded_run, recorded_root) = store
        .create_run(conversation.id, ProviderId::Claude)
        .await
        .unwrap();
    for (run_id, root_id, provider, request_id) in [
        (
            pending_run.id,
            pending_root.id,
            ProviderId::Codex,
            "codex/native/pending-7",
        ),
        (
            recorded_run.id,
            recorded_root.id,
            ProviderId::Claude,
            "claude/native/recorded-9",
        ),
    ] {
        store
            .append_run_event(run_id, root_id, ProviderEventRecord::started())
            .await
            .unwrap();
        store
            .append_run_event(
                run_id,
                root_id,
                ProviderEventRecord::approval_requested(
                    provider,
                    request_id,
                    "write",
                    "fixture.txt",
                ),
            )
            .await
            .unwrap();
    }
    store
        .record_response_intent(
            recorded_run.id,
            recorded_root.id,
            "claude/native/recorded-9",
            ApprovalResolution::Answer("exact native answer".to_owned()),
        )
        .await
        .unwrap();
    store.close().await;

    let reopened = Store::open(&path).await.unwrap();
    let recovery = reopened.pending_recovery().await.unwrap();
    let pending = recovery
        .iter()
        .find(|candidate| candidate.run.id == pending_run.id)
        .unwrap()
        .approvals
        .first()
        .unwrap();
    assert_eq!(
        pending.provider_request_id.as_deref(),
        Some("codex/native/pending-7")
    );
    assert_eq!(pending.response_intent, None);
    let recorded = recovery
        .iter()
        .find(|candidate| candidate.run.id == recorded_run.id)
        .unwrap()
        .approvals
        .first()
        .unwrap();
    assert_eq!(
        recorded.provider_request_id.as_deref(),
        Some("claude/native/recorded-9")
    );
    assert_eq!(recorded.status, ApprovalStatus::Pending);
    assert_eq!(
        recorded.response_intent.as_ref().unwrap().resolution,
        ApprovalResolution::Answer("exact native answer".to_owned())
    );
    assert_eq!(
        recorded.response_intent.as_ref().unwrap().status,
        ApprovalResponseIntentStatus::Recorded
    );
}

#[tokio::test]
async fn terminal_events_resolve_or_reject_pending_approvals_atomically() {
    for (terminal, expected_status, expected_resolution) in [
        (
            ProviderEventRecord::interrupted(),
            ApprovalStatus::Cancelled,
            ApprovalResolution::Cancelled,
        ),
        (
            ProviderEventRecord::provider_failed(
                ProviderErrorCategory::ProcessExited,
                MutationState::Unknown,
                DispatchCertainty::MayHaveDispatched,
            ),
            ApprovalStatus::Failed,
            ApprovalResolution::Failed,
        ),
    ] {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("terminal approval"))
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
                    "request-terminal",
                    "write",
                    "fixture.txt",
                ),
            )
            .await
            .unwrap();

        store
            .append_run_event(run.id, root.id, terminal)
            .await
            .unwrap();

        let approval = store
            .load_approval(run.id, "request-terminal")
            .await
            .unwrap();
        assert_eq!(approval.status, expected_status);
        assert_eq!(approval.resolution, Some(expected_resolution));
    }

    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("completion approval"))
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
                "request-pending",
                "execute",
                "command",
            ),
        )
        .await
        .unwrap();

    assert!(matches!(
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::completed())
            .await,
        Err(StoreError::PendingApproval)
    ));
    assert_eq!(
        store.load_run(run.id).await.unwrap().status,
        RunStatus::Waiting
    );
    assert_eq!(
        store
            .load_approval(run.id, "request-pending")
            .await
            .unwrap()
            .status,
        ApprovalStatus::Pending
    );
}

#[tokio::test]
async fn fallback_runs_have_distinct_durable_identity_and_alternate_provider() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("fallback"))
        .await
        .unwrap();
    let (primary, primary_root) = store
        .create_run(conversation.id, ProviderId::Codex)
        .await
        .unwrap();

    assert!(matches!(
        store
            .create_fallback_run(primary.id, ProviderId::Claude)
            .await,
        Err(StoreError::UnsafeFallbackState)
    ));
    store
        .append_run_event(
            primary.id,
            primary_root.id,
            ProviderEventRecord::provider_failed(
                ProviderErrorCategory::Rejected,
                MutationState::NoneObserved,
                DispatchCertainty::NotDispatched,
            ),
        )
        .await
        .unwrap();

    let (fallback, _) = store
        .create_fallback_run(primary.id, ProviderId::Claude)
        .await
        .unwrap();

    assert_ne!(fallback.id, primary.id);
    assert_eq!(fallback.conversation_id, primary.conversation_id);
    assert_eq!(fallback.fallback_from_run_id, Some(primary.id));
    assert!(matches!(
        store
            .create_fallback_run(primary.id, ProviderId::Claude)
            .await,
        Err(StoreError::FallbackAlreadyExists)
    ));
    assert!(matches!(
        store
            .create_fallback_run(primary.id, ProviderId::Codex)
            .await,
        Err(StoreError::SameFallbackProvider)
    ));
    let fallback_root = store
        .pending_recovery()
        .await
        .unwrap()
        .into_iter()
        .find(|recovery| recovery.run.id == fallback.id)
        .unwrap()
        .agents
        .into_iter()
        .find(|agent| agent.parent_id.is_none())
        .unwrap();
    store
        .append_run_event(
            fallback.id,
            fallback_root.id,
            ProviderEventRecord::provider_failed(
                ProviderErrorCategory::Rejected,
                MutationState::NoneObserved,
                DispatchCertainty::NotDispatched,
            ),
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .create_fallback_run(fallback.id, ProviderId::Codex)
            .await,
        Err(StoreError::UnsafeFallbackState)
    ));
}

#[tokio::test]
async fn fallback_requires_durable_not_dispatched_proof() {
    for (failure, expected_certainty) in [
        (
            ProviderEventRecord::failed_with_mutation(
                "untyped failure",
                MutationState::NoneObserved,
            ),
            None,
        ),
        (
            ProviderEventRecord::provider_failed(
                ProviderErrorCategory::Transport,
                MutationState::NoneObserved,
                DispatchCertainty::MayHaveDispatched,
            ),
            Some(DispatchCertainty::MayHaveDispatched),
        ),
    ] {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("unsafe fallback"))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, failure)
            .await
            .unwrap();

        let persisted = store.load_run(run.id).await.unwrap();
        assert_eq!(persisted.status, RunStatus::Failed);
        assert_eq!(persisted.mutation_state, MutationState::NoneObserved);
        assert_eq!(persisted.dispatch_certainty, expected_certainty);
        assert!(matches!(
            store.create_fallback_run(run.id, ProviderId::Claude).await,
            Err(StoreError::UnsafeFallbackState)
        ));
    }
}

#[tokio::test]
async fn provider_failures_persist_only_a_typed_sanitized_category() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("sanitized failure"))
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

    let event = store
        .append_run_event(
            run.id,
            root.id,
            ProviderEventRecord::provider_failed(
                ProviderErrorCategory::MalformedJson,
                MutationState::Unknown,
                DispatchCertainty::MayHaveDispatched,
            ),
        )
        .await
        .unwrap();

    assert_eq!(event.kind, TimelineEventKind::Diagnostic);
    assert_eq!(event.content, "Provider failed: malformed JSON");
    assert_eq!(
        store.load_event_payload(event.id).await.unwrap().unwrap(),
        serde_json::json!({
            "errorCategory": "malformedJson",
            "mutation": "unknown",
            "dispatchCertainty": "mayHaveDispatched",
        })
    );
    let persisted = store.load_run(run.id).await.unwrap();
    assert_eq!(persisted.mutation_state, MutationState::Unknown);
    assert_eq!(
        persisted.dispatch_certainty,
        Some(DispatchCertainty::MayHaveDispatched)
    );
}

#[tokio::test]
async fn fail_run_if_active_is_atomic_and_idempotent() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("panic recovery"))
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
                "request-panic",
                "execute",
                "command",
            ),
        )
        .await
        .unwrap();

    assert!(
        store
            .fail_run_if_active(
                run.id,
                root.id,
                ProviderErrorCategory::ContractViolation,
                MutationState::Unknown,
                DispatchCertainty::MayHaveDispatched,
            )
            .await
            .unwrap()
    );
    assert!(
        !store
            .fail_run_if_active(
                run.id,
                root.id,
                ProviderErrorCategory::ContractViolation,
                MutationState::Unknown,
                DispatchCertainty::MayHaveDispatched,
            )
            .await
            .unwrap()
    );
    assert_eq!(
        store.load_run(run.id).await.unwrap().status,
        RunStatus::Failed
    );
    assert_eq!(
        store
            .load_approval(run.id, "request-panic")
            .await
            .unwrap()
            .status,
        ApprovalStatus::Failed
    );
}
