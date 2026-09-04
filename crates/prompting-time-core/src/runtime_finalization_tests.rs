use super::*;
use crate::domain::{AgentStatus, ApprovalStatus, TimelineEvent};
use crate::providers::{
    NativeAgentStatus, NativeChildStatus, ProviderCapabilities, ProviderHealth, ProviderTurnOwner,
};
use crate::store::{AgentPageRecord, NewConversation};
use async_trait::async_trait;

#[derive(Default)]
struct ShutdownControl {
    entered: Notify,
    release: Notify,
    fail: bool,
}

struct FinalizationAdapter {
    receiver: Mutex<Option<mpsc::Receiver<Result<ProviderEvent, ProviderError>>>>,
    shutdown: Arc<ShutdownControl>,
}

struct FinalizationOwner(Arc<ShutdownControl>);

#[async_trait]
impl ProviderTurnOwner for FinalizationOwner {
    async fn shutdown(self: Box<Self>) -> Result<(), ProviderError> {
        self.0.entered.notify_one();
        self.0.release.notified().await;
        if self.0.fail {
            return Err(ProviderError::Transport {
                category: "fixture_shutdown".into(),
            });
        }
        Ok(())
    }
}

#[async_trait]
impl ProviderAdapter for FinalizationAdapter {
    fn id(&self) -> ProviderId {
        ProviderId::Codex
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::default()
    }
    async fn health(&self) -> Result<ProviderHealth, ProviderError> {
        Ok(ProviderHealth::Healthy {
            version: "fixture".into(),
        })
    }
    async fn start_session(&self, _: StartSession) -> Result<ProviderSession, ProviderError> {
        Ok(ProviderSession {
            provider: self.id(),
            native_id: "recursive-session".into(),
            native_group_id: None,
        })
    }
    async fn resume_session(
        &self,
        _: &str,
        _: ResumeSession,
    ) -> Result<ProviderSession, ProviderError> {
        unreachable!()
    }
    async fn start_turn(
        &self,
        _: &ProviderSession,
        _: TurnRequest,
    ) -> Result<ProviderTurn, ProviderError> {
        Ok(ProviderTurn::new(
            self.receiver.lock().unwrap().take().unwrap(),
            FinalizationOwner(self.shutdown.clone()),
        ))
    }
    async fn steer(&self, _: &ProviderSession, _: &str, _: &str) -> Result<(), ProviderError> {
        unreachable!()
    }
    async fn respond(
        &self,
        _: &ProviderSession,
        _: &str,
        _: ApprovalResponse,
    ) -> Result<(), ProviderError> {
        unreachable!()
    }
    async fn interrupt(&self, _: &ProviderSession, _: &str) -> Result<(), ProviderError> {
        unreachable!()
    }
}

struct Fixture {
    store: Store,
    conversation_id: ConversationId,
    supervisor: RunSupervisor,
    handle: RunHandle,
    sender: Option<mpsc::Sender<Result<ProviderEvent, ProviderError>>>,
    shutdown: Arc<ShutdownControl>,
}

impl Fixture {
    async fn new(shutdown_error: bool) -> Self {
        let store = Store::open_in_memory().await.unwrap();
        let conversation_id = store
            .create_conversation(NewConversation::projectless("recursive finalization"))
            .await
            .unwrap()
            .id;
        let (sender, receiver) = mpsc::channel(4);
        let shutdown = Arc::new(ShutdownControl {
            fail: shutdown_error,
            ..Default::default()
        });
        let adapter = Arc::new(FinalizationAdapter {
            receiver: Mutex::new(Some(receiver)),
            shutdown: shutdown.clone(),
        });
        let supervisor = RunSupervisor::new(store.clone(), vec![adapter]).unwrap();
        let handle = supervisor
            .submit(RunRequest::new(
                conversation_id,
                PathBuf::from("/tmp/recursive-finalization"),
                ProviderId::Codex,
                TurnRequest::new("fixture"),
            ))
            .await
            .unwrap();
        sender
            .send(Ok(ProviderEvent::TurnStarted {
                native_turn_id: "recursive-turn".into(),
            }))
            .await
            .unwrap();
        handle.wait_for(RunStatus::Running).await.unwrap();
        Self {
            store,
            conversation_id,
            supervisor,
            handle,
            sender: Some(sender),
            shutdown,
        }
    }

    async fn send(&self, event: ProviderEvent) {
        self.sender.as_ref().unwrap().send(Ok(event)).await.unwrap();
    }

    async fn agents(&self) -> Vec<AgentPageRecord> {
        let mut agents = Vec::new();
        let mut cursor = None;
        loop {
            let page = self
                .store
                .load_agent_page(self.conversation_id, cursor, 200)
                .await
                .unwrap();
            agents.extend(page.items);
            cursor = page.next_cursor;
            if cursor.is_none() {
                return agents;
            }
        }
    }

    async fn wait_for_agents(&self, count: usize) {
        let mut changes = self.store.subscribe_changes();
        tokio::time::timeout(Duration::from_secs(3), async {
            while self.agents().await.len() != count {
                changes.recv().await.unwrap();
            }
        })
        .await
        .expect("child declarations must become durable");
    }

    async fn tree(&self) {
        self.send(children(
            "recursive-session",
            &[
                ("child", NativeAgentStatus::Running),
                ("completed", NativeAgentStatus::Completed),
            ],
        ))
        .await;
        self.send(children(
            "child",
            &[("grandchild", NativeAgentStatus::Running)],
        ))
        .await;
        self.wait_for_agents(4).await;
    }

    async fn timeline(&self) -> Vec<TimelineEvent> {
        self.store
            .load_timeline(self.conversation_id, None, 200)
            .await
            .unwrap()
            .items
    }

    async fn release_and_expect(&self, status: RunStatus) {
        self.shutdown.entered.notified().await;
        assert!(self.agents().await.iter().all(|entry| !matches!(
            entry.agent.status,
            AgentStatus::Interrupted | AgentStatus::Failed
        )));
        self.shutdown.release.notify_one();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), self.handle.wait())
                .await
                .expect("finalization must terminate")
                .unwrap()
                .status,
            status
        );
        assert_eq!(
            self.store
                .load_run(self.handle.run_id())
                .await
                .unwrap()
                .status,
            status
        );
    }

    async fn expect_tree(&self, status: AgentStatus, category: Option<ProviderErrorCategory>) {
        let agents = self.agents().await;
        assert_eq!(
            agents
                .iter()
                .filter(|entry| entry.agent.status == AgentStatus::Completed)
                .count(),
            1
        );
        assert_eq!(
            agents
                .iter()
                .filter(|entry| entry.agent.status == status)
                .count(),
            3
        );
        assert_eq!(
            self.store
                .load_run(self.handle.run_id())
                .await
                .unwrap()
                .mutation_state,
            MutationState::Unknown
        );
        let timeline = self.timeline().await;
        let mut terminals = Vec::new();
        for event in &timeline {
            if let Some(payload) = self.store.load_event_payload(event.id).await.unwrap()
                && payload.get("mutation") == Some(&serde_json::json!(MutationState::Unknown))
            {
                if let Some(category) = category {
                    assert_eq!(payload["errorCategory"], serde_json::json!(category));
                } else {
                    assert!(payload.get("errorCategory").is_none());
                }
                terminals.push(event);
            }
        }
        let depths = terminals
            .iter()
            .map(|event| {
                agents
                    .iter()
                    .find(|entry| entry.agent.id == event.agent_id)
                    .unwrap()
                    .depth
            })
            .collect::<Vec<_>>();
        // Root cancellation can omit its own mutation payload; descendants never do.
        assert!(
            depths == [2, 1, 0] || (category.is_none() && depths == [2, 1]),
            "terminal order: {depths:?}"
        );
    }
}

fn children(parent: &str, children: &[(&str, NativeAgentStatus)]) -> ProviderEvent {
    ProviderEvent::ChildAgentActivity {
        native_item_id: format!("spawn-{parent}"),
        parent_native_thread_id: parent.into(),
        child_native_thread_ids: children.iter().map(|(id, _)| (*id).into()).collect(),
        child_statuses: children
            .iter()
            .map(|(id, status)| NativeChildStatus {
                native_thread_id: (*id).into(),
                status: status.clone(),
            })
            .collect(),
        operation: format!("spawn-{parent}"),
        status: "inProgress".into(),
    }
}

#[tokio::test]
async fn recursive_interrupt_closes_descendants_deepest_first_after_owned_shutdown() {
    for app_shutdown in [false, true] {
        let fixture = Fixture::new(false).await;
        fixture.tree().await;
        if app_shutdown {
            let ((), result) = tokio::join!(
                fixture.release_and_expect(RunStatus::Interrupted),
                fixture.supervisor.shutdown(),
            );
            result.unwrap();
        } else {
            fixture
                .supervisor
                .interrupt(fixture.handle.run_id())
                .await
                .unwrap();
            fixture.release_and_expect(RunStatus::Interrupted).await;
        }
        fixture.expect_tree(AgentStatus::Interrupted, None).await;
        fixture.supervisor.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn recursive_failures_preserve_the_original_category() {
    for error in [
        Some(ProviderError::Protocol {
            category: "fixture_protocol".into(),
        }),
        None,
    ] {
        let mut fixture = Fixture::new(false).await;
        fixture.tree().await;
        let category = error
            .as_ref()
            .map_or(ProviderErrorCategory::StreamClosed, ProviderError::category);
        if let Some(error) = error {
            fixture
                .sender
                .as_ref()
                .unwrap()
                .send(Err(error))
                .await
                .unwrap();
        }
        fixture.sender.take();
        fixture.release_and_expect(RunStatus::Failed).await;
        fixture
            .expect_tree(AgentStatus::Failed, Some(category))
            .await;
        fixture.supervisor.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn recursive_shutdown_error_fails_instead_of_claiming_interruption() {
    let fixture = Fixture::new(true).await;
    fixture.tree().await;
    fixture
        .supervisor
        .interrupt(fixture.handle.run_id())
        .await
        .unwrap();
    fixture.release_and_expect(RunStatus::Failed).await;
    fixture
        .expect_tree(AgentStatus::Failed, Some(ProviderErrorCategory::Transport))
        .await;
    fixture.supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn recursive_completion_with_active_descendants_fails_closed() {
    let mut fixture = Fixture::new(false).await;
    fixture.tree().await;
    fixture.send(ProviderEvent::TurnCompleted).await;
    fixture.sender.take();
    fixture.release_and_expect(RunStatus::Failed).await;
    fixture
        .expect_tree(
            AgentStatus::Failed,
            Some(ProviderErrorCategory::ContractViolation),
        )
        .await;
    fixture.supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn recursive_interrupt_pages_past_two_hundred_children_and_excludes_root() {
    let fixture = Fixture::new(false).await;
    for index in 0..205 {
        fixture
            .send(children(
                "recursive-session",
                &[(&format!("child-{index}"), NativeAgentStatus::Running)],
            ))
            .await;
    }
    fixture.wait_for_agents(206).await;
    fixture
        .supervisor
        .interrupt(fixture.handle.run_id())
        .await
        .unwrap();
    fixture.release_and_expect(RunStatus::Interrupted).await;
    assert!(
        fixture
            .agents()
            .await
            .iter()
            .all(|entry| entry.agent.status == AgentStatus::Interrupted)
    );
    fixture.supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn recursive_owner_transfer_during_shutdown_fences_stale_cleanup() {
    let fixture = Fixture::new(false).await;
    fixture.tree().await;
    fixture
        .supervisor
        .interrupt(fixture.handle.run_id())
        .await
        .unwrap();
    fixture.shutdown.entered.notified().await;
    let before = fixture.timeline().await;
    fixture
        .store
        .replace_dispatch_owner_for_test(fixture.handle.run_id(), "winning-owner")
        .await
        .unwrap();
    fixture.shutdown.release.notify_one();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), fixture.handle.wait())
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(fixture.timeline().await, before);
    assert_eq!(
        fixture
            .store
            .load_run(fixture.handle.run_id())
            .await
            .unwrap()
            .status,
        RunStatus::Running
    );
    assert_eq!(
        fixture
            .agents()
            .await
            .iter()
            .filter(|entry| entry.agent.status == AgentStatus::Running)
            .count(),
        3
    );
    assert!(fixture.supervisor.shutdown().await.is_err());
}

#[tokio::test]
async fn recursive_interrupt_preserves_staged_child_audit_and_cancels_approval() {
    let fixture = Fixture::new(false).await;
    fixture
        .send(ProviderEvent::ApprovalRequested {
            request_id: "permission".into(),
            operation: "write".into(),
            scope: "fixture.txt".into(),
            details: None,
        })
        .await;
    fixture.handle.wait_for(RunStatus::Waiting).await.unwrap();
    fixture
        .send(children(
            "recursive-session",
            &[("staged-child", NativeAgentStatus::Running)],
        ))
        .await;
    fixture.wait_for_agents(2).await;
    assert!(
        !fixture
            .timeline()
            .await
            .iter()
            .any(|event| event.content == "spawn-recursive-session")
    );
    fixture
        .supervisor
        .interrupt(fixture.handle.run_id())
        .await
        .unwrap();
    fixture.release_and_expect(RunStatus::Interrupted).await;
    assert!(
        fixture
            .agents()
            .await
            .iter()
            .all(|entry| entry.agent.status == AgentStatus::Interrupted)
    );
    assert_eq!(
        fixture
            .store
            .load_approval(fixture.handle.run_id(), "permission")
            .await
            .unwrap()
            .status,
        ApprovalStatus::Cancelled
    );
    assert_eq!(
        fixture
            .timeline()
            .await
            .iter()
            .filter(|event| event.content == "spawn-recursive-session")
            .count(),
        1
    );
    assert!(fixture.store.pending_recovery().await.unwrap().is_empty());
    fixture.supervisor.shutdown().await.unwrap();
}
