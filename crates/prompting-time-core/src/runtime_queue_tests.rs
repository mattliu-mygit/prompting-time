use super::*;
use crate::domain::{ApprovalStatus, UserInputQuestion};
use crate::providers::{ProviderCapabilities, ProviderHealth, ProviderTurnOwner};
use crate::store::NewConversation;
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, AtomicUsize};

struct QueueAdapter {
    receiver: Mutex<Option<mpsc::Receiver<Result<ProviderEvent, ProviderError>>>>,
    responses: Mutex<Vec<String>>,
    reject_response: AtomicBool,
    shutdowns: Arc<AtomicUsize>,
}

struct QueueOwner(Arc<AtomicUsize>);

#[async_trait]
impl ProviderTurnOwner for QueueOwner {
    async fn shutdown(self: Box<Self>) -> Result<(), ProviderError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[async_trait]
impl ProviderAdapter for QueueAdapter {
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
            native_id: "queue-session".into(),
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
            QueueOwner(Arc::clone(&self.shutdowns)),
        ))
    }
    async fn steer(&self, _: &ProviderSession, _: &str, _: &str) -> Result<(), ProviderError> {
        unreachable!()
    }
    async fn respond(
        &self,
        _: &ProviderSession,
        request_id: &str,
        _: ApprovalResponse,
    ) -> Result<(), ProviderError> {
        self.responses.lock().unwrap().push(request_id.to_owned());
        if self.reject_response.load(Ordering::SeqCst) {
            return Err(ProviderError::NotDispatched {
                category: ProviderErrorCategory::Protocol,
            });
        }
        Ok(())
    }
    async fn interrupt(&self, _: &ProviderSession, _: &str) -> Result<(), ProviderError> {
        Ok(())
    }
}

struct Fixture {
    store: Store,
    conversation_id: ConversationId,
    supervisor: Arc<RunSupervisor>,
    handle: RunHandle,
    sender: mpsc::Sender<Result<ProviderEvent, ProviderError>>,
    adapter: Arc<QueueAdapter>,
}

fn approval(id: &str) -> ProviderEvent {
    ProviderEvent::ApprovalRequested {
        request_id: id.into(),
        operation: "write".into(),
        scope: "fixture.txt".into(),
        details: None,
    }
}

fn question(id: &str) -> ProviderEvent {
    ProviderEvent::UserInputRequested {
        request_id: id.into(),
        questions: vec![UserInputQuestion {
            id: "question".into(),
            header: "Choice".into(),
            question: "Choose a value".into(),
            options: None,
            is_other: false,
            is_secret: false,
        }],
        auto_resolution_ms: None,
    }
}

impl Fixture {
    async fn new(ack: Option<Arc<ResponseAcknowledgementBarrier>>) -> Self {
        Self::with_barriers(ack, None).await
    }

    async fn with_barriers(
        ack: Option<Arc<ResponseAcknowledgementBarrier>>,
        terminal: Option<Arc<TerminalReceiptBarrier>>,
    ) -> Self {
        let store = Store::open_in_memory().await.unwrap();
        let conversation_id = store
            .create_conversation(NewConversation::projectless("permission queue"))
            .await
            .unwrap()
            .id;
        let (sender, receiver) = mpsc::channel(4);
        let adapter = Arc::new(QueueAdapter {
            receiver: Mutex::new(Some(receiver)),
            responses: Mutex::new(Vec::new()),
            reject_response: AtomicBool::new(false),
            shutdowns: Arc::new(AtomicUsize::new(0)),
        });
        let mut supervisor = RunSupervisor::new(store.clone(), vec![adapter.clone()]).unwrap();
        if let Some(ack) = ack {
            supervisor.set_response_acknowledgement_barrier(ack);
        }
        if let Some(terminal) = terminal {
            supervisor.set_terminal_receipt_barrier(terminal);
        }
        let supervisor = Arc::new(supervisor);
        let handle = supervisor
            .submit(RunRequest::new(
                conversation_id,
                PathBuf::from("/tmp/permission-queue"),
                ProviderId::Codex,
                TurnRequest::new("fixture"),
            ))
            .await
            .unwrap();
        sender
            .send(Ok(ProviderEvent::TurnStarted {
                native_turn_id: "queue-turn".into(),
            }))
            .await
            .unwrap();
        sender.send(Ok(approval("a"))).await.unwrap();
        handle.wait_for(RunStatus::Waiting).await.unwrap();
        Self {
            store,
            conversation_id,
            supervisor,
            handle,
            sender,
            adapter,
        }
    }

    async fn send(&self, event: ProviderEvent) {
        self.sender.send(Ok(event)).await.unwrap();
    }

    // A durable marker proves all preceding channel events have been handled.
    async fn staged(&self, marker: &str) {
        self.send(ProviderEvent::Progress {
            content: marker.into(),
        })
        .await;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                assert_eq!(self.handle.status(), RunStatus::Waiting);
                if self
                    .store
                    .pending_recovery()
                    .await
                    .unwrap()
                    .iter()
                    .any(|run| {
                        run.staged_events
                            .iter()
                            .any(|event| event.content == marker)
                    })
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("preceding controls must be queued while progress is staged");
    }

    async fn pending(&self, id: &str) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let active =
                    Arc::clone(&self.supervisor.active.lock().unwrap()[&self.handle.run_id()]);
                let matches = active
                    .attempt
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|attempt| attempt.pending_request_id.as_deref() == Some(id));
                if matches
                    && self
                        .store
                        .load_approval(self.handle.run_id(), id)
                        .await
                        .is_ok()
                {
                    break;
                }
                assert!(!matches!(
                    self.handle.status(),
                    RunStatus::Failed | RunStatus::Interrupted
                ));
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("next permission must become visible");
        let pending = self
            .store
            .load_recent_approvals(self.conversation_id, 30)
            .await
            .unwrap();
        assert_eq!(pending.items.len(), 1);
        assert_eq!(
            self.store
                .load_approval(self.handle.run_id(), id)
                .await
                .unwrap()
                .status,
            ApprovalStatus::Pending
        );
    }

    async fn respond(&self, id: &str) {
        let response = if id == "b" {
            ApprovalResponse::Answer("chosen".into())
        } else {
            ApprovalResponse::Approved
        };
        self.supervisor
            .respond(self.handle.run_id(), id, response)
            .await
            .unwrap();
    }

    fn spawn_response(&self) -> JoinHandle<Result<(), RuntimeError>> {
        let supervisor = Arc::clone(&self.supervisor);
        let run_id = self.handle.run_id();
        tokio::spawn(async move {
            supervisor
                .respond(run_id, "a", ApprovalResponse::Approved)
                .await
        })
    }

    async fn finish(self, terminal: ProviderEvent, expected: RunStatus) {
        self.sender.send(Ok(terminal)).await.unwrap();
        drop(self.sender);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), self.handle.wait())
                .await
                .unwrap()
                .unwrap()
                .status,
            expected
        );
        assert_eq!(self.adapter.shutdowns.load(Ordering::SeqCst), 1);
        self.supervisor.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn mixed_permissions_are_fifo_and_progress_survives_once() {
    let fixture = Fixture::new(None).await;
    fixture.send(question("b")).await;
    fixture.staged("first progress").await;
    fixture.send(approval("c")).await;
    fixture
        .send(ProviderEvent::ToolActivity {
            description: "queued mutation".into(),
            mutation: MutationState::Observed,
        })
        .await;
    fixture.staged("second progress").await;
    fixture.pending("a").await;
    fixture.respond("a").await;
    fixture.pending("b").await;
    fixture.respond("b").await;
    fixture.pending("c").await;
    fixture.respond("c").await;
    assert_eq!(*fixture.adapter.responses.lock().unwrap(), ["a", "b", "c"]);
    let timeline = fixture
        .store
        .load_timeline(fixture.conversation_id, None, 30)
        .await
        .unwrap();
    let progress: Vec<_> = timeline
        .items
        .iter()
        .filter(|event| event.content.ends_with(" progress"))
        .map(|event| event.content.as_str())
        .collect();
    assert_eq!(progress, ["first progress", "second progress"]);
    assert_eq!(
        fixture
            .store
            .load_run(fixture.handle.run_id())
            .await
            .unwrap()
            .mutation_state,
        MutationState::Observed
    );
    fixture
        .finish(ProviderEvent::TurnCompleted, RunStatus::Completed)
        .await;
}

#[tokio::test]
async fn received_permission_during_acknowledgement_stays_behind_queued_permission() {
    let ack = Arc::new(ResponseAcknowledgementBarrier::new());
    let fixture = Fixture::new(Some(Arc::clone(&ack))).await;
    fixture.send(question("b")).await;
    fixture.staged("before ack progress").await;
    let response = fixture.spawn_response();
    ack.committed.notified().await;
    fixture.send(approval("c")).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.sender.capacity() != fixture.sender.max_capacity() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("C must be received while acknowledgement holds the attempt gate");
    ack.release.notify_one();
    response.await.unwrap().unwrap();
    fixture.pending("b").await;
    ack.release.notify_one();
    fixture.respond("b").await;
    fixture.pending("c").await;
    ack.release.notify_one();
    fixture.respond("c").await;
    assert_eq!(*fixture.adapter.responses.lock().unwrap(), ["a", "b", "c"]);
    fixture
        .finish(ProviderEvent::TurnCompleted, RunStatus::Completed)
        .await;
}

#[tokio::test]
async fn rejected_response_keeps_next_permission_hidden_until_retry_acknowledges() {
    let fixture = Fixture::new(None).await;
    fixture.send(question("b")).await;
    fixture.staged("queued progress").await;
    fixture
        .adapter
        .reject_response
        .store(true, Ordering::SeqCst);
    assert!(
        fixture
            .supervisor
            .respond(fixture.handle.run_id(), "a", ApprovalResponse::Approved)
            .await
            .is_err()
    );
    fixture.pending("a").await;
    fixture.staged("after rejection progress").await;
    assert!(
        fixture
            .store
            .load_approval(fixture.handle.run_id(), "b")
            .await
            .is_err()
    );
    fixture
        .adapter
        .reject_response
        .store(false, Ordering::SeqCst);
    fixture.respond("a").await;
    fixture.pending("b").await;
    fixture.respond("b").await;
    assert_eq!(*fixture.adapter.responses.lock().unwrap(), ["a", "a", "b"]);
    fixture
        .finish(ProviderEvent::TurnCompleted, RunStatus::Completed)
        .await;
}

#[tokio::test]
async fn terminal_discards_queued_permissions() {
    let fixture = Fixture::new(None).await;
    fixture.send(question("b")).await;
    fixture.send(approval("c")).await;
    fixture.staged("terminal progress").await;
    let store = fixture.store.clone();
    let run_id = fixture.handle.run_id();
    fixture
        .finish(ProviderEvent::TurnCompleted, RunStatus::Failed)
        .await;
    assert!(store.load_approval(run_id, "b").await.is_err());
    assert!(store.load_approval(run_id, "c").await.is_err());
}

#[tokio::test]
async fn cancellation_already_set_during_acknowledgement_prevents_queue_promotion() {
    for shutdown in [false, true] {
        let ack = Arc::new(ResponseAcknowledgementBarrier::new());
        let fixture = Fixture::new(Some(Arc::clone(&ack))).await;
        fixture.send(question("b")).await;
        fixture.staged("cancel progress").await;
        let response = fixture.spawn_response();
        ack.committed.notified().await;
        let active =
            Arc::clone(&fixture.supervisor.active.lock().unwrap()[&fixture.handle.run_id()]);
        if shutdown {
            active.shutdown.send_replace(true);
        } else {
            active.cancellation.send_replace(true);
        }
        ack.release.notify_one();
        response.await.unwrap().unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), fixture.handle.wait())
                .await
                .unwrap()
                .unwrap()
                .status,
            RunStatus::Interrupted
        );
        assert!(
            fixture
                .store
                .load_approval(fixture.handle.run_id(), "b")
                .await
                .is_err()
        );
        assert_eq!(*fixture.adapter.responses.lock().unwrap(), ["a"]);
        assert_eq!(fixture.adapter.shutdowns.load(Ordering::SeqCst), 1);
        assert!(
            fixture
                .supervisor
                .respond(
                    fixture.handle.run_id(),
                    "b",
                    ApprovalResponse::Answer("stale".into())
                )
                .await
                .is_err()
        );
        fixture.supervisor.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn ownership_loss_prevents_queue_promotion() {
    let ack = Arc::new(ResponseAcknowledgementBarrier::new());
    let fixture = Fixture::new(Some(Arc::clone(&ack))).await;
    fixture.send(question("b")).await;
    fixture.staged("transfer progress").await;
    let response = fixture.spawn_response();
    ack.committed.notified().await;
    fixture
        .store
        .replace_dispatch_owner_for_test(fixture.handle.run_id(), "replacement-owner")
        .await
        .unwrap();
    ack.release.notify_one();
    response.await.unwrap().unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), fixture.handle.wait())
            .await
            .unwrap()
            .is_err()
    );
    assert!(
        fixture
            .store
            .load_approval(fixture.handle.run_id(), "b")
            .await
            .is_err()
    );
    assert_eq!(*fixture.adapter.responses.lock().unwrap(), ["a"]);
    assert_eq!(fixture.adapter.shutdowns.load(Ordering::SeqCst), 1);
    assert!(matches!(
        fixture.supervisor.shutdown().await,
        Err(RuntimeError::OwnedTaskFailed)
    ));
}

#[tokio::test]
async fn known_eof_during_acknowledgement_prevents_queue_promotion() {
    let ack = Arc::new(ResponseAcknowledgementBarrier::new());
    let terminal = Arc::new(TerminalReceiptBarrier::new());
    let fixture = Fixture::with_barriers(Some(Arc::clone(&ack)), Some(Arc::clone(&terminal))).await;
    fixture.send(question("b")).await;
    fixture.staged("eof progress").await;
    let response = fixture.spawn_response();
    ack.committed.notified().await;
    drop(fixture.sender);
    terminal.received.notified().await;
    // Closure is already recorded; let acknowledgement clear the pending ID before
    // the receive loop resumes so promotion cannot rely only on the waiting branch.
    ack.release.notify_one();
    response.await.unwrap().unwrap();
    terminal.release.notify_one();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), fixture.handle.wait())
            .await
            .unwrap()
            .unwrap()
            .status,
        RunStatus::Failed
    );
    assert!(
        fixture
            .store
            .load_approval(fixture.handle.run_id(), "b")
            .await
            .is_err()
    );
    assert_eq!(*fixture.adapter.responses.lock().unwrap(), ["a"]);
    assert_eq!(fixture.adapter.shutdowns.load(Ordering::SeqCst), 1);
    fixture.supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn permission_queue_charges_serialized_bytes_and_releases_them_on_promotion() {
    let fixture = Fixture::new(None).await;
    let mut large = approval("large");
    let overhead = serde_json::to_vec(&large).unwrap().len() - "write".len();
    if let ProviderEvent::ApprovalRequested { operation, .. } = &mut large {
        *operation = "x".repeat(256 * 1024 - overhead);
    }
    assert_eq!(serde_json::to_vec(&large).unwrap().len(), 256 * 1024);
    fixture.send(large.clone()).await;
    fixture.staged("byte boundary progress").await;
    fixture.respond("a").await;
    fixture.pending("large").await;
    if let ProviderEvent::ApprovalRequested { request_id, .. } = &mut large {
        *request_id = "other".into();
    }
    fixture.send(large).await;
    fixture.staged("byte refill progress").await;
    fixture.respond("large").await;
    fixture.pending("other").await;
    fixture.respond("other").await;
    fixture
        .finish(ProviderEvent::TurnCompleted, RunStatus::Completed)
        .await;
}

#[tokio::test]
async fn permission_queue_accepts_sixteen_waiting_requests_and_releases_capacity() {
    let fixture = Fixture::new(None).await;
    for id in 0..16 {
        fixture.send(approval(&format!("queued-{id}"))).await;
    }
    fixture.staged("full progress").await;
    fixture.respond("a").await;
    fixture.pending("queued-0").await;
    fixture.send(approval("queued-16")).await;
    fixture.staged("refilled progress").await;
    for id in 0..17 {
        let id = format!("queued-{id}");
        fixture.pending(&id).await;
        fixture.respond(&id).await;
    }
    assert_eq!(fixture.adapter.responses.lock().unwrap().len(), 18);
    fixture
        .finish(ProviderEvent::TurnCompleted, RunStatus::Completed)
        .await;
}

#[tokio::test]
async fn permission_queue_rejects_invalid_ids_and_capacity_overflow() {
    let mut cases = vec![
        vec![question("a")],
        vec![question("b"), approval("b")],
        vec![question("")],
        vec![approval("  ")],
    ];
    cases.push(
        (0..17)
            .map(|id| approval(&format!("queued-{id}")))
            .collect(),
    );
    let mut large = approval("large");
    if let ProviderEvent::ApprovalRequested { operation, .. } = &mut large {
        // JSON escaping doubles these bytes; raw string lengths would incorrectly fit.
        *operation = "\"".repeat(70 * 1024);
    }
    let mut second_large = large.clone();
    if let ProviderEvent::ApprovalRequested { request_id, .. } = &mut second_large {
        *request_id = "second-large".into();
    }
    cases.push(vec![large, second_large]);
    for events in cases {
        let fixture = Fixture::new(None).await;
        for event in events {
            fixture.send(event).await;
        }
        let outcome = tokio::time::timeout(Duration::from_secs(3), fixture.handle.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, RunStatus::Failed);
        assert!(fixture.adapter.responses.lock().unwrap().is_empty());
        assert_eq!(fixture.adapter.shutdowns.load(Ordering::SeqCst), 1);
        fixture.supervisor.shutdown().await.unwrap();
    }
}
