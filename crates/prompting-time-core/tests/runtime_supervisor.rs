use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use prompting_time_core::domain::{
    ApprovalResolution, ApprovalStatus, ConversationId, MutationState, RunStatus, TimelineEventKind,
};
use prompting_time_core::providers::process::JsonLineProcess;
use prompting_time_core::providers::{
    ApprovalResponse, ProviderAdapter, ProviderCapabilities, ProviderError, ProviderErrorCategory,
    ProviderEvent, ProviderHealth, ProviderId, ProviderSession, ProviderTurn, ProviderTurnOwner,
    ResumeSession, StartSession, TurnRequest,
};
use prompting_time_core::router::ProviderCapability;
use prompting_time_core::runtime::{
    MAX_CONCURRENT_ROOT_RUNS, MAX_QUEUED_ROOT_RUNS, RunRequest, RunSupervisor, RuntimeError,
};
use prompting_time_core::store::{NewConversation, ProviderEventRecord, Store};
use tokio::process::Command;
use tokio::sync::mpsc;

#[derive(Clone)]
enum Plan {
    Blocking,
    Approval,
    Complete,
    DoubleTerminal,
    Crash { mutation: MutationState },
    StreamError(ProviderError),
    StreamClosed,
    DelayedDuplicateTerminal,
    DelayedPostTerminalMutation,
    ApprovalCrash,
    ApprovalClosed,
    ApprovalWithMutation(MutationState),
    OwnedChild,
    TrackedOwner,
    StartTurnError(ProviderError),
    Panic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HangPoint {
    StartSession,
    ResumeSession,
    StartTurn,
    Steer,
    Respond,
    Interrupt,
}

struct ScriptedAdapter {
    id: ProviderId,
    plans: Mutex<VecDeque<Plan>>,
    open_turns: Mutex<Vec<mpsc::Sender<Result<ProviderEvent, ProviderError>>>>,
    started: AtomicUsize,
    interrupted: Mutex<Vec<String>>,
    steers: Mutex<Vec<String>>,
    responses: Mutex<Vec<String>>,
    hang: Option<HangPoint>,
    response_error: bool,
    immediate_response_output: bool,
    response_output_count: usize,
    response_events: Vec<ProviderEvent>,
    delayed_response_tail: bool,
    owned_pid: AtomicUsize,
    control_started: AtomicUsize,
    owner_shutdowns: Arc<AtomicUsize>,
}

struct NoopTurnOwner;

#[async_trait]
impl ProviderTurnOwner for NoopTurnOwner {
    async fn shutdown(self: Box<Self>) -> Result<(), ProviderError> {
        Ok(())
    }
}

struct ProcessTurnOwner(Option<JsonLineProcess>);

#[async_trait]
impl ProviderTurnOwner for ProcessTurnOwner {
    async fn shutdown(mut self: Box<Self>) -> Result<(), ProviderError> {
        match self.0.take() {
            Some(process) => process.shutdown().await,
            None => Ok(()),
        }
    }
}

struct TrackingTurnOwner(Arc<AtomicUsize>);

#[async_trait]
impl ProviderTurnOwner for TrackingTurnOwner {
    async fn shutdown(self: Box<Self>) -> Result<(), ProviderError> {
        tokio::task::yield_now().await;
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl ScriptedAdapter {
    fn new(id: ProviderId, plans: impl IntoIterator<Item = Plan>) -> Self {
        Self {
            id,
            plans: Mutex::new(plans.into_iter().collect()),
            open_turns: Mutex::new(Vec::new()),
            started: AtomicUsize::new(0),
            interrupted: Mutex::new(Vec::new()),
            steers: Mutex::new(Vec::new()),
            responses: Mutex::new(Vec::new()),
            hang: None,
            response_error: false,
            immediate_response_output: false,
            response_output_count: 0,
            response_events: Vec::new(),
            delayed_response_tail: false,
            owned_pid: AtomicUsize::new(0),
            control_started: AtomicUsize::new(0),
            owner_shutdowns: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn hanging(id: ProviderId, plan: Plan, hang: HangPoint) -> Self {
        let mut adapter = Self::new(id, [plan]);
        adapter.hang = Some(hang);
        adapter
    }

    fn with_response_behavior(mut self, immediate_output: bool, reject: bool) -> Self {
        self.immediate_response_output = immediate_output;
        self.response_error = reject;
        self
    }

    fn with_response_flood(mut self, count: usize) -> Self {
        self.response_output_count = count;
        self
    }

    fn with_response_events(mut self, events: Vec<ProviderEvent>, delayed_tail: bool) -> Self {
        self.response_events = events;
        self.delayed_response_tail = delayed_tail;
        self
    }

    fn send_one(&self, event: Result<ProviderEvent, ProviderError>) {
        self.open_turns.lock().unwrap()[0].try_send(event).unwrap();
    }

    fn close_one(&self) {
        self.open_turns.lock().unwrap().remove(0);
    }

    fn complete_one(&self) {
        self.open_turns
            .lock()
            .unwrap()
            .remove(0)
            .try_send(Ok(ProviderEvent::TurnCompleted))
            .unwrap();
    }

    fn complete_all(&self) {
        for sender in self.open_turns.lock().unwrap().drain(..) {
            let _ = sender.try_send(Ok(ProviderEvent::TurnCompleted));
        }
    }
}

#[async_trait]
impl ProviderAdapter for ScriptedAdapter {
    fn id(&self) -> ProviderId {
        self.id
    }

    fn capabilities(&self) -> ProviderCapabilities {
        [
            ProviderCapability::Streaming,
            ProviderCapability::DeferredApproval,
            ProviderCapability::Interruption,
        ]
        .into()
    }

    async fn health(&self) -> Result<ProviderHealth, ProviderError> {
        Ok(ProviderHealth::Healthy {
            version: "fixture".to_owned(),
        })
    }

    async fn start_session(
        &self,
        _request: StartSession,
    ) -> Result<ProviderSession, ProviderError> {
        if self.hang == Some(HangPoint::StartSession) {
            std::future::pending().await
        }
        Ok(ProviderSession {
            provider: self.id,
            native_id: format!("{:?}-session", self.id).to_lowercase(),
        })
    }

    async fn resume_session(
        &self,
        native_id: &str,
        _request: ResumeSession,
    ) -> Result<ProviderSession, ProviderError> {
        if self.hang == Some(HangPoint::ResumeSession) {
            std::future::pending().await
        }
        Ok(ProviderSession {
            provider: self.id,
            native_id: native_id.to_owned(),
        })
    }

    async fn start_turn(
        &self,
        _session: &ProviderSession,
        _request: TurnRequest,
    ) -> Result<ProviderTurn, ProviderError> {
        if self.hang == Some(HangPoint::StartTurn) {
            std::future::pending().await
        }
        let number = self.started.fetch_add(1, Ordering::SeqCst) + 1;
        let plan = self
            .plans
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Plan::Blocking);
        let plan = match plan {
            Plan::StartTurnError(error) => return Err(error),
            plan => plan,
        };
        let (sender, receiver) = mpsc::channel(16);
        sender
            .try_send(Ok(ProviderEvent::TurnStarted {
                native_turn_id: format!("turn-{number}"),
            }))
            .unwrap();
        match plan {
            Plan::Blocking => self.open_turns.lock().unwrap().push(sender),
            Plan::Approval => {
                sender
                    .try_send(Ok(ProviderEvent::ApprovalRequested {
                        request_id: "approval-1".to_owned(),
                        operation: "write".to_owned(),
                        scope: "fixture.txt".to_owned(),
                    }))
                    .unwrap();
                self.open_turns.lock().unwrap().push(sender);
            }
            Plan::Complete => {
                sender
                    .try_send(Ok(ProviderEvent::AssistantMessage {
                        content: "done".to_owned(),
                    }))
                    .unwrap();
                sender.try_send(Ok(ProviderEvent::TurnCompleted)).unwrap();
            }
            Plan::DoubleTerminal => {
                sender.try_send(Ok(ProviderEvent::TurnCompleted)).unwrap();
                sender.try_send(Ok(ProviderEvent::TurnCompleted)).unwrap();
            }
            Plan::Crash { mutation } => {
                if mutation != MutationState::NoneObserved {
                    sender
                        .try_send(Ok(ProviderEvent::ToolActivity {
                            description: "tool ran".to_owned(),
                            mutation,
                        }))
                        .unwrap();
                }
                sender
                    .try_send(Err(ProviderError::Protocol {
                        category: "fixture-crash".to_owned(),
                    }))
                    .unwrap();
            }
            Plan::StreamError(error) => sender.try_send(Err(error)).unwrap(),
            Plan::StreamClosed => {}
            Plan::DelayedDuplicateTerminal => {
                tokio::spawn(async move {
                    sender.send(Ok(ProviderEvent::TurnCompleted)).await.unwrap();
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    let _ = sender.send(Ok(ProviderEvent::TurnCompleted)).await;
                });
                return Ok(ProviderTurn::new(receiver, NoopTurnOwner));
            }
            Plan::DelayedPostTerminalMutation => {
                tokio::spawn(async move {
                    sender.send(Ok(ProviderEvent::TurnCompleted)).await.unwrap();
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    let _ = sender
                        .send(Ok(ProviderEvent::ToolActivity {
                            description: "late write".to_owned(),
                            mutation: MutationState::Observed,
                        }))
                        .await;
                });
                return Ok(ProviderTurn::new(receiver, NoopTurnOwner));
            }
            Plan::ApprovalCrash => {
                sender
                    .try_send(Ok(ProviderEvent::ApprovalRequested {
                        request_id: "approval-1".to_owned(),
                        operation: "write".to_owned(),
                        scope: "fixture.txt".to_owned(),
                    }))
                    .unwrap();
                sender.try_send(Err(ProviderError::ProcessExited)).unwrap();
            }
            Plan::ApprovalClosed => {
                sender
                    .try_send(Ok(ProviderEvent::ApprovalRequested {
                        request_id: "approval-1".to_owned(),
                        operation: "write".to_owned(),
                        scope: "fixture.txt".to_owned(),
                    }))
                    .unwrap();
            }
            Plan::ApprovalWithMutation(mutation) => {
                sender
                    .try_send(Ok(ProviderEvent::ApprovalRequested {
                        request_id: "approval-1".to_owned(),
                        operation: "write".to_owned(),
                        scope: "fixture.txt".to_owned(),
                    }))
                    .unwrap();
                sender
                    .try_send(Ok(ProviderEvent::ToolActivity {
                        description: "buffered mutation".to_owned(),
                        mutation,
                    }))
                    .unwrap();
                self.open_turns.lock().unwrap().push(sender);
            }
            Plan::OwnedChild => {
                let mut command = Command::new("/bin/sh");
                command.arg("-c").arg("sleep 30");
                let process = JsonLineProcess::spawn(command).unwrap();
                self.owned_pid
                    .store(process.id() as usize, Ordering::SeqCst);
                self.open_turns.lock().unwrap().push(sender);
                return Ok(ProviderTurn::new(receiver, ProcessTurnOwner(Some(process))));
            }
            Plan::TrackedOwner => {
                self.open_turns.lock().unwrap().push(sender);
                return Ok(ProviderTurn::new(
                    receiver,
                    TrackingTurnOwner(Arc::clone(&self.owner_shutdowns)),
                ));
            }
            Plan::StartTurnError(_) => unreachable!("handled before stream creation"),
            Plan::Panic => panic!("fixture provider panic"),
        }
        Ok(ProviderTurn::new(receiver, NoopTurnOwner))
    }

    async fn steer(
        &self,
        _session: &ProviderSession,
        active_turn: &str,
        text: &str,
    ) -> Result<(), ProviderError> {
        if self.hang == Some(HangPoint::Steer) {
            self.control_started.fetch_add(1, Ordering::SeqCst);
            std::future::pending().await
        }
        self.steers
            .lock()
            .unwrap()
            .push(format!("{active_turn}:{text}"));
        Ok(())
    }

    async fn respond(
        &self,
        _session: &ProviderSession,
        request_id: &str,
        response: ApprovalResponse,
    ) -> Result<(), ProviderError> {
        if self.hang == Some(HangPoint::Respond) {
            self.control_started.fetch_add(1, Ordering::SeqCst);
            std::future::pending().await
        }
        self.responses
            .lock()
            .unwrap()
            .push(format!("{request_id}:{response:?}"));
        if self.response_error {
            return Err(ProviderError::Protocol {
                category: "fixture-response-rejected".to_owned(),
            });
        }
        if self.immediate_response_output {
            self.open_turns.lock().unwrap()[0]
                .try_send(Ok(ProviderEvent::Progress {
                    content: "immediate output".to_owned(),
                }))
                .unwrap();
        }
        if self.response_output_count > 0 {
            let sender = self.open_turns.lock().unwrap()[0].clone();
            for index in 0..self.response_output_count {
                sender
                    .send(Ok(ProviderEvent::Progress {
                        content: format!("buffered-{index}"),
                    }))
                    .await
                    .unwrap();
            }
            while sender.capacity() != sender.max_capacity() {
                tokio::task::yield_now().await;
            }
        } else if !self.response_events.is_empty() {
            let sender = self.open_turns.lock().unwrap().remove(0);
            let events = self.response_events.clone();
            if self.delayed_response_tail {
                sender.send(Ok(events[0].clone())).await.unwrap();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    for event in events.into_iter().skip(1) {
                        let _ = sender.send(Ok(event)).await;
                    }
                });
            } else {
                for event in events {
                    sender.send(Ok(event)).await.unwrap();
                }
                while sender.capacity() != sender.max_capacity() {
                    tokio::task::yield_now().await;
                }
            }
        } else {
            self.complete_one();
        }
        Ok(())
    }

    async fn interrupt(
        &self,
        _session: &ProviderSession,
        active_turn: &str,
    ) -> Result<(), ProviderError> {
        if self.hang == Some(HangPoint::Interrupt) {
            std::future::pending().await
        }
        self.interrupted
            .lock()
            .unwrap()
            .push(active_turn.to_owned());
        Ok(())
    }
}

async fn fixture() -> (Store, ConversationId) {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store
        .create_conversation(NewConversation::projectless("runtime fixture"))
        .await
        .unwrap();
    (store, conversation.id)
}

fn request(conversation_id: ConversationId, provider: ProviderId) -> RunRequest {
    RunRequest::new(
        conversation_id,
        PathBuf::from("/tmp/invented-project"),
        provider,
        TurnRequest::new("run the fixture"),
    )
}

async fn eventually(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("condition should become true");
}

#[tokio::test]
async fn supervisor_runs_four_roots_and_leaves_the_fifth_queued() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(ScriptedAdapter::new(
        ProviderId::Codex,
        std::iter::repeat_n(Plan::Blocking, 5),
    ));
    let supervisor = RunSupervisor::new(store.clone(), vec![adapter.clone()]).unwrap();
    let mut handles = Vec::new();
    for _ in 0..5 {
        handles.push(
            supervisor
                .submit(request(conversation_id, ProviderId::Codex))
                .await
                .unwrap(),
        );
    }

    eventually(|| adapter.started.load(Ordering::SeqCst) == 4).await;
    assert_eq!(
        store.load_run(handles[4].run_id()).await.unwrap().status,
        RunStatus::Queued
    );

    adapter.complete_one();
    eventually(|| adapter.started.load(Ordering::SeqCst) == 5).await;
    adapter.complete_all();
    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn root_admission_is_bounded_before_persistence_and_shutdown_reconciles_every_handle() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(ScriptedAdapter::new(ProviderId::Codex, []));
    let supervisor = RunSupervisor::new(store.clone(), vec![adapter.clone()]).unwrap();
    let admission_limit = MAX_CONCURRENT_ROOT_RUNS + MAX_QUEUED_ROOT_RUNS;
    let mut handles = Vec::with_capacity(admission_limit);
    for _ in 0..admission_limit {
        handles.push(
            supervisor
                .submit(request(conversation_id, ProviderId::Codex))
                .await
                .unwrap(),
        );
    }

    eventually(|| adapter.started.load(Ordering::SeqCst) == MAX_CONCURRENT_ROOT_RUNS).await;
    let before = store.pending_recovery().await.unwrap();
    assert_eq!(before.len(), admission_limit);
    assert!(matches!(
        supervisor
            .submit(request(conversation_id, ProviderId::Codex))
            .await,
        Err(RuntimeError::RunQueueFull { limit }) if limit == MAX_QUEUED_ROOT_RUNS
    ));
    assert_eq!(
        store.pending_recovery().await.unwrap().len(),
        admission_limit
    );
    assert_eq!(
        adapter.started.load(Ordering::SeqCst),
        MAX_CONCURRENT_ROOT_RUNS
    );

    adapter.complete_one();
    let completed = handles.remove(0);
    assert_eq!(completed.wait().await.unwrap().status, RunStatus::Completed);
    handles.push(
        supervisor
            .submit(request(conversation_id, ProviderId::Codex))
            .await
            .expect("a completed root must release one admission slot"),
    );

    tokio::time::timeout(Duration::from_secs(5), supervisor.shutdown())
        .await
        .expect("shutdown must drain a full admitted queue")
        .unwrap();
    for handle in handles {
        assert_eq!(handle.wait().await.unwrap().status, RunStatus::Interrupted);
    }
    assert!(store.pending_recovery().await.unwrap().is_empty());
}

#[tokio::test]
async fn interrupt_before_queued_task_subscribes_is_not_lost() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(ScriptedAdapter::new(
        ProviderId::Codex,
        std::iter::repeat_n(Plan::Blocking, 5),
    ));
    let supervisor = RunSupervisor::new(store.clone(), vec![adapter.clone()]).unwrap();
    let mut running = Vec::new();
    for _ in 0..4 {
        let handle = supervisor
            .submit(request(conversation_id, ProviderId::Codex))
            .await
            .unwrap();
        handle.wait_for(RunStatus::Running).await.unwrap();
        running.push(handle);
    }

    let queued = supervisor
        .submit(request(conversation_id, ProviderId::Codex))
        .await
        .unwrap();
    supervisor.interrupt(queued.run_id()).await.unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(2), queued.wait())
        .await
        .expect("a queued interrupt must survive task subscription")
        .unwrap();
    assert_eq!(outcome.status, RunStatus::Interrupted);
    assert_eq!(
        store.load_run(queued.run_id()).await.unwrap().status,
        RunStatus::Interrupted
    );
    assert_eq!(adapter.started.load(Ordering::SeqCst), 4);

    adapter.complete_all();
    for handle in running {
        assert_eq!(handle.wait().await.unwrap().status, RunStatus::Completed);
    }
    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn interrupt_flood_is_coalesced_and_cannot_delay_shutdown() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(ScriptedAdapter::new(ProviderId::Codex, [Plan::Blocking]));
    let supervisor = Arc::new(RunSupervisor::new(store, vec![adapter]).unwrap());
    let handle = supervisor
        .submit(request(conversation_id, ProviderId::Codex))
        .await
        .unwrap();
    handle.wait_for(RunStatus::Running).await.unwrap();
    let barrier = Arc::new(tokio::sync::Barrier::new(257));
    let mut interrupts = Vec::new();
    for _ in 0..256 {
        let supervisor = Arc::clone(&supervisor);
        let barrier = Arc::clone(&barrier);
        let run_id = handle.run_id();
        interrupts.push(tokio::spawn(async move {
            barrier.wait().await;
            supervisor.interrupt(run_id).await
        }));
    }
    barrier.wait().await;
    tokio::time::timeout(Duration::from_secs(2), supervisor.shutdown())
        .await
        .expect("dedicated shutdown control must bypass interrupt pressure")
        .unwrap();
    for interrupt in interrupts {
        assert!(matches!(
            interrupt.await.unwrap(),
            Ok(()) | Err(RuntimeError::UnknownRun(_))
        ));
    }
    assert_eq!(handle.wait().await.unwrap().status, RunStatus::Interrupted);
}

#[tokio::test]
async fn supervisor_interrupts_only_the_requested_native_turn() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(ScriptedAdapter::new(
        ProviderId::Codex,
        [Plan::Blocking, Plan::Blocking],
    ));
    let supervisor = RunSupervisor::new(store.clone(), vec![adapter.clone()]).unwrap();
    let first = supervisor
        .submit(request(conversation_id, ProviderId::Codex))
        .await
        .unwrap();
    let _second = supervisor
        .submit(request(conversation_id, ProviderId::Codex))
        .await
        .unwrap();
    eventually(|| adapter.started.load(Ordering::SeqCst) == 2).await;

    supervisor.interrupt(first.run_id()).await.unwrap();
    eventually(|| adapter.interrupted.lock().unwrap().len() == 1).await;
    assert_eq!(&*adapter.interrupted.lock().unwrap(), &["turn-1"]);
    first.wait_for(RunStatus::Interrupted).await.unwrap();
    adapter.complete_all();
    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn supervisor_steers_the_active_native_turn() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(ScriptedAdapter::new(ProviderId::Codex, [Plan::Blocking]));
    let supervisor = RunSupervisor::new(store, vec![adapter.clone()]).unwrap();
    let handle = supervisor
        .submit(request(conversation_id, ProviderId::Codex))
        .await
        .unwrap();
    handle.wait_for(RunStatus::Running).await.unwrap();

    supervisor
        .steer(handle.run_id(), "focus on the parser")
        .await
        .unwrap();

    assert_eq!(
        &*adapter.steers.lock().unwrap(),
        &["turn-1:focus on the parser"]
    );
    adapter.complete_all();
    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn supervisor_persists_native_session_and_approval_pause_resume() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(ScriptedAdapter::new(ProviderId::Codex, [Plan::Approval]));
    let supervisor = RunSupervisor::new(store.clone(), vec![adapter.clone()]).unwrap();
    let handle = supervisor
        .submit(request(conversation_id, ProviderId::Codex))
        .await
        .unwrap();

    handle.wait_for(RunStatus::Waiting).await.unwrap();
    supervisor
        .respond(handle.run_id(), "approval-1", ApprovalResponse::Approved)
        .await
        .unwrap();
    assert_eq!(
        &*adapter.responses.lock().unwrap(),
        &["approval-1:Approved"]
    );
    assert_eq!(handle.wait().await.unwrap().status, RunStatus::Completed);
    let run = store.load_run(handle.run_id()).await.unwrap();
    assert_eq!(run.native_session_id.as_deref(), Some("codex-session"));
    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn supervisor_falls_back_once_only_before_mutation() {
    let (store, conversation_id) = fixture().await;
    let primary = Arc::new(ScriptedAdapter::new(
        ProviderId::Codex,
        [Plan::StartTurnError(ProviderError::NotDispatched {
            category: prompting_time_core::providers::ProviderErrorCategory::Rejected,
        })],
    ));
    let fallback = Arc::new(ScriptedAdapter::new(ProviderId::Claude, [Plan::Complete]));
    let supervisor = RunSupervisor::new(store, vec![primary.clone(), fallback.clone()]).unwrap();

    let handle = supervisor
        .submit(request(conversation_id, ProviderId::Codex).with_fallback(ProviderId::Claude))
        .await
        .unwrap();

    assert_eq!(handle.wait().await.unwrap().status, RunStatus::Completed);
    assert_eq!(primary.started.load(Ordering::SeqCst), 1);
    assert_eq!(fallback.started.load(Ordering::SeqCst), 1);
    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn ambiguous_start_or_stream_failure_never_falls_back() {
    for (plan, category) in [
        (
            Plan::StartTurnError(ProviderError::Transport {
                category: "lost-start-response-secret".to_owned(),
            }),
            ProviderErrorCategory::Transport,
        ),
        (
            Plan::StartTurnError(ProviderError::NotInstalled {
                binary: "fixture".to_owned(),
                diagnostic: "secret".to_owned(),
            }),
            ProviderErrorCategory::NotInstalled,
        ),
        (
            Plan::StartTurnError(ProviderError::InspectionFailed {
                binary: "fixture".to_owned(),
                diagnostic: "secret".to_owned(),
            }),
            ProviderErrorCategory::InspectionFailed,
        ),
        (
            Plan::StartTurnError(ProviderError::TimedOut {
                binary: "fixture".to_owned(),
                diagnostic: "secret".to_owned(),
            }),
            ProviderErrorCategory::TimedOut,
        ),
        (
            Plan::Crash {
                mutation: MutationState::NoneObserved,
            },
            ProviderErrorCategory::Protocol,
        ),
        (
            Plan::StreamError(ProviderError::MalformedJson),
            ProviderErrorCategory::MalformedJson,
        ),
        (
            Plan::StreamError(ProviderError::ProcessExited),
            ProviderErrorCategory::ProcessExited,
        ),
        (Plan::StreamClosed, ProviderErrorCategory::StreamClosed),
    ] {
        let (store, conversation_id) = fixture().await;
        let primary = Arc::new(ScriptedAdapter::new(ProviderId::Codex, [plan]));
        let fallback = Arc::new(ScriptedAdapter::new(ProviderId::Claude, [Plan::Complete]));
        let supervisor =
            RunSupervisor::new(store.clone(), vec![primary, fallback.clone()]).unwrap();

        let handle = supervisor
            .submit(request(conversation_id, ProviderId::Codex).with_fallback(ProviderId::Claude))
            .await
            .unwrap();

        assert_eq!(handle.wait().await.unwrap().status, RunStatus::Failed);
        assert_eq!(fallback.started.load(Ordering::SeqCst), 0);
        assert_eq!(
            store
                .load_run(handle.run_id())
                .await
                .unwrap()
                .mutation_state,
            MutationState::Unknown
        );
        let timeline = store
            .load_timeline(conversation_id, None, 20)
            .await
            .unwrap();
        let failure = timeline.items.last().unwrap();
        assert!(!failure.content.contains("secret"));
        assert_eq!(
            store.load_event_payload(failure.id).await.unwrap().unwrap()["errorCategory"],
            serde_json::to_value(category).unwrap()
        );
        supervisor.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn supervisor_never_falls_back_after_observed_or_unknown_mutation() {
    for mutation in [MutationState::Observed, MutationState::Unknown] {
        let (store, conversation_id) = fixture().await;
        let primary = Arc::new(ScriptedAdapter::new(
            ProviderId::Codex,
            [Plan::Crash { mutation }],
        ));
        let fallback = Arc::new(ScriptedAdapter::new(ProviderId::Claude, [Plan::Complete]));
        let supervisor =
            RunSupervisor::new(store.clone(), vec![primary, fallback.clone()]).unwrap();

        let handle = supervisor
            .submit(request(conversation_id, ProviderId::Codex).with_fallback(ProviderId::Claude))
            .await
            .unwrap();

        assert_eq!(handle.wait().await.unwrap().status, RunStatus::Failed);
        assert_eq!(fallback.started.load(Ordering::SeqCst), 0);
        assert_eq!(
            store
                .load_run(handle.run_id())
                .await
                .unwrap()
                .mutation_state,
            MutationState::Unknown
        );
        supervisor.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn supervisor_rejects_more_than_one_terminal_event() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(ScriptedAdapter::new(
        ProviderId::Codex,
        [Plan::DoubleTerminal],
    ));
    let supervisor = RunSupervisor::new(store.clone(), vec![adapter]).unwrap();

    let handle = supervisor
        .submit(request(conversation_id, ProviderId::Codex))
        .await
        .unwrap();

    assert_eq!(handle.wait().await.unwrap().status, RunStatus::Failed);
    assert_eq!(
        store
            .load_run(handle.run_id())
            .await
            .unwrap()
            .mutation_state,
        MutationState::Unknown
    );
    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_fallback_is_not_retried() {
    let (store, conversation_id) = fixture().await;
    let primary = Arc::new(ScriptedAdapter::new(
        ProviderId::Codex,
        [Plan::StartTurnError(ProviderError::NotDispatched {
            category: prompting_time_core::providers::ProviderErrorCategory::Rejected,
        })],
    ));
    let fallback = Arc::new(ScriptedAdapter::new(
        ProviderId::Claude,
        [Plan::Crash {
            mutation: MutationState::NoneObserved,
        }],
    ));
    let supervisor = RunSupervisor::new(store, vec![primary.clone(), fallback.clone()]).unwrap();

    let handle = supervisor
        .submit(request(conversation_id, ProviderId::Codex).with_fallback(ProviderId::Claude))
        .await
        .unwrap();

    assert_eq!(handle.wait().await.unwrap().status, RunStatus::Failed);
    assert_eq!(primary.started.load(Ordering::SeqCst), 1);
    assert_eq!(fallback.started.load(Ordering::SeqCst), 1);
    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn invalid_approval_is_rejected_before_reaching_provider() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(ScriptedAdapter::new(ProviderId::Codex, [Plan::Approval]));
    let supervisor = RunSupervisor::new(store, vec![adapter.clone()]).unwrap();
    let handle = supervisor
        .submit(request(conversation_id, ProviderId::Codex))
        .await
        .unwrap();
    handle.wait_for(RunStatus::Waiting).await.unwrap();

    assert!(
        supervisor
            .respond(handle.run_id(), "wrong-request", ApprovalResponse::Denied)
            .await
            .is_err()
    );
    assert!(adapter.responses.lock().unwrap().is_empty());
    supervisor
        .respond(handle.run_id(), "approval-1", ApprovalResponse::Denied)
        .await
        .unwrap();
    assert_eq!(handle.wait().await.unwrap().status, RunStatus::Completed);
    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn synchronous_output_waits_for_exact_approval_persistence() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(
        ScriptedAdapter::new(ProviderId::Codex, [Plan::Approval])
            .with_response_behavior(true, false),
    );
    let supervisor = RunSupervisor::new(store.clone(), vec![adapter]).unwrap();
    let handle = supervisor
        .submit(request(conversation_id, ProviderId::Codex))
        .await
        .unwrap();
    handle.wait_for(RunStatus::Waiting).await.unwrap();

    supervisor
        .respond(
            handle.run_id(),
            "approval-1",
            ApprovalResponse::Answer("use the existing file".to_owned()),
        )
        .await
        .unwrap();

    assert_eq!(handle.wait().await.unwrap().status, RunStatus::Completed);
    let approval = store
        .load_approval(handle.run_id(), "approval-1")
        .await
        .unwrap();
    assert_eq!(approval.status, ApprovalStatus::Answered);
    assert_eq!(
        approval.resolution,
        Some(ApprovalResolution::Answer(
            "use the existing file".to_owned()
        ))
    );
    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn rejected_approval_response_remains_pending() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(
        ScriptedAdapter::new(ProviderId::Codex, [Plan::Approval])
            .with_response_behavior(false, true),
    );
    let supervisor = RunSupervisor::new(store.clone(), vec![adapter]).unwrap();
    let handle = supervisor
        .submit(request(conversation_id, ProviderId::Codex))
        .await
        .unwrap();
    handle.wait_for(RunStatus::Waiting).await.unwrap();

    assert!(
        supervisor
            .respond(handle.run_id(), "approval-1", ApprovalResponse::Approved,)
            .await
            .is_err()
    );
    let approval = store
        .load_approval(handle.run_id(), "approval-1")
        .await
        .unwrap();
    assert_eq!(approval.status, ApprovalStatus::Pending);
    assert_eq!(approval.resolution, None);
    supervisor.interrupt(handle.run_id()).await.unwrap();
    handle.wait_for(RunStatus::Interrupted).await.unwrap();
    assert_eq!(
        store
            .load_approval(handle.run_id(), "approval-1")
            .await
            .unwrap()
            .status,
        ApprovalStatus::Cancelled
    );
    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn fallback_outcome_names_both_durable_attempts() {
    let (store, conversation_id) = fixture().await;
    let primary = Arc::new(ScriptedAdapter::new(
        ProviderId::Codex,
        [Plan::StartTurnError(ProviderError::NotDispatched {
            category: prompting_time_core::providers::ProviderErrorCategory::Rejected,
        })],
    ));
    let fallback = Arc::new(ScriptedAdapter::new(ProviderId::Claude, [Plan::Complete]));
    let supervisor = RunSupervisor::new(store.clone(), vec![primary, fallback]).unwrap();
    let handle = supervisor
        .submit(request(conversation_id, ProviderId::Codex).with_fallback(ProviderId::Claude))
        .await
        .unwrap();

    let outcome = handle.wait().await.unwrap();
    let fallback_run_id = outcome
        .fallback_run_id
        .expect("fallback attempt is durable");
    assert_eq!(outcome.primary_run_id, handle.run_id());
    assert_eq!(outcome.terminal_run_id, fallback_run_id);
    assert_eq!(outcome.status, RunStatus::Completed);
    assert_eq!(
        store.load_run(handle.run_id()).await.unwrap().status,
        RunStatus::Failed
    );
    assert_eq!(
        store
            .load_run(fallback_run_id)
            .await
            .unwrap()
            .fallback_from_run_id,
        Some(handle.run_id())
    );
    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn same_provider_cannot_be_used_as_fallback() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(ScriptedAdapter::new(ProviderId::Codex, []));
    let supervisor = RunSupervisor::new(store.clone(), vec![adapter]).unwrap();

    assert!(matches!(
        supervisor
            .submit(request(conversation_id, ProviderId::Codex).with_fallback(ProviderId::Codex))
            .await,
        Err(RuntimeError::InvalidFallbackProvider(ProviderId::Codex))
    ));
    assert!(store.pending_recovery().await.unwrap().is_empty());
    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn panicked_provider_task_is_reconciled_durably() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(ScriptedAdapter::new(ProviderId::Codex, [Plan::Panic]));
    let supervisor = RunSupervisor::new(store.clone(), vec![adapter]).unwrap();
    let handle = supervisor
        .submit(request(conversation_id, ProviderId::Codex))
        .await
        .unwrap();

    assert_eq!(handle.wait().await.unwrap().status, RunStatus::Failed);
    assert_eq!(
        store.load_run(handle.run_id()).await.unwrap().status,
        RunStatus::Failed
    );
    assert!(matches!(
        supervisor.shutdown().await,
        Err(RuntimeError::OwnedTaskFailed)
    ));
}

#[tokio::test]
async fn shutdown_cancels_every_blocking_adapter_operation() {
    for (plan, point) in [
        (Plan::Blocking, HangPoint::StartSession),
        (Plan::Blocking, HangPoint::StartTurn),
        (Plan::Blocking, HangPoint::Interrupt),
    ] {
        let (store, conversation_id) = fixture().await;
        let adapter = Arc::new(ScriptedAdapter::hanging(ProviderId::Codex, plan, point));
        let supervisor = RunSupervisor::new(store, vec![adapter.clone()]).unwrap();
        let handle = supervisor
            .submit(request(conversation_id, ProviderId::Codex))
            .await
            .unwrap();
        if point == HangPoint::Interrupt {
            handle.wait_for(RunStatus::Running).await.unwrap();
            supervisor.interrupt(handle.run_id()).await.unwrap();
        }
        tokio::time::timeout(Duration::from_secs(2), supervisor.shutdown())
            .await
            .unwrap_or_else(|_| panic!("shutdown must preempt blocked {point:?}"))
            .unwrap();
        assert_eq!(handle.wait().await.unwrap().status, RunStatus::Interrupted);
    }
}

#[tokio::test]
async fn shutdown_cancels_blocking_session_resume() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(ScriptedAdapter::hanging(
        ProviderId::Codex,
        Plan::Blocking,
        HangPoint::ResumeSession,
    ));
    let supervisor = RunSupervisor::new(store, vec![adapter]).unwrap();
    let handle = supervisor
        .submit(request(conversation_id, ProviderId::Codex).resume("existing-session"))
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(2), supervisor.shutdown())
        .await
        .expect("shutdown must preempt blocked session resume")
        .unwrap();
    assert_eq!(handle.wait().await.unwrap().status, RunStatus::Interrupted);
}

#[tokio::test]
async fn shutdown_cancels_blocking_control_calls() {
    for (plan, point) in [
        (Plan::Blocking, HangPoint::Steer),
        (Plan::Approval, HangPoint::Respond),
    ] {
        let (store, conversation_id) = fixture().await;
        let adapter = Arc::new(ScriptedAdapter::hanging(ProviderId::Codex, plan, point));
        let supervisor = Arc::new(RunSupervisor::new(store, vec![adapter]).unwrap());
        let handle = supervisor
            .submit(request(conversation_id, ProviderId::Codex))
            .await
            .unwrap();
        let call = match point {
            HangPoint::Steer => {
                handle.wait_for(RunStatus::Running).await.unwrap();
                let supervisor = Arc::clone(&supervisor);
                let run_id = handle.run_id();
                tokio::spawn(async move { supervisor.steer(run_id, "blocked").await })
            }
            HangPoint::Respond => {
                handle.wait_for(RunStatus::Waiting).await.unwrap();
                let supervisor = Arc::clone(&supervisor);
                let run_id = handle.run_id();
                tokio::spawn(async move {
                    supervisor
                        .respond(run_id, "approval-1", ApprovalResponse::Denied)
                        .await
                })
            }
            _ => unreachable!(),
        };
        tokio::time::timeout(Duration::from_secs(2), supervisor.shutdown())
            .await
            .expect("shutdown must preempt a blocked control call")
            .unwrap();
        assert!(call.await.unwrap().is_err());
    }
}

#[tokio::test]
async fn attempt_terminalization_cancels_blocking_control_calls() {
    for (plan, point) in [
        (Plan::Blocking, HangPoint::Steer),
        (Plan::Approval, HangPoint::Respond),
    ] {
        let (store, conversation_id) = fixture().await;
        let adapter = Arc::new(ScriptedAdapter::hanging(ProviderId::Codex, plan, point));
        let supervisor = Arc::new(RunSupervisor::new(store, vec![adapter.clone()]).unwrap());
        let handle = supervisor
            .submit(request(conversation_id, ProviderId::Codex))
            .await
            .unwrap();
        let call = match point {
            HangPoint::Steer => {
                handle.wait_for(RunStatus::Running).await.unwrap();
                let supervisor = Arc::clone(&supervisor);
                let run_id = handle.run_id();
                tokio::spawn(async move { supervisor.steer(run_id, "blocked").await })
            }
            HangPoint::Respond => {
                handle.wait_for(RunStatus::Waiting).await.unwrap();
                let supervisor = Arc::clone(&supervisor);
                let run_id = handle.run_id();
                tokio::spawn(async move {
                    supervisor
                        .respond(run_id, "approval-1", ApprovalResponse::Denied)
                        .await
                })
            }
            _ => unreachable!(),
        };
        eventually(|| adapter.control_started.load(Ordering::SeqCst) == 1).await;
        adapter.send_one(Ok(ProviderEvent::TurnCompleted));
        adapter.close_one();
        let outcome = tokio::time::timeout(Duration::from_secs(2), handle.wait())
            .await
            .expect("terminal stream closure must finish the provider attempt")
            .unwrap();
        if point == HangPoint::Steer {
            assert_eq!(outcome.status, RunStatus::Completed);
        } else {
            assert_eq!(outcome.status, RunStatus::Failed);
        }
        tokio::time::timeout(Duration::from_secs(2), call)
            .await
            .expect("attempt terminalization must wake blocked control calls")
            .unwrap()
            .expect_err("a control call cannot outlive its provider attempt");
        supervisor.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn submit_after_shutdown_fails_without_persisting_a_run() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(ScriptedAdapter::new(ProviderId::Codex, []));
    let supervisor = RunSupervisor::new(store.clone(), vec![adapter]).unwrap();
    supervisor.shutdown().await.unwrap();

    assert!(matches!(
        supervisor
            .submit(request(conversation_id, ProviderId::Codex))
            .await,
        Err(RuntimeError::SupervisorClosed)
    ));
    assert!(store.pending_recovery().await.unwrap().is_empty());
}

#[tokio::test]
async fn delayed_activity_after_terminal_is_a_contract_failure() {
    for plan in [
        Plan::DelayedDuplicateTerminal,
        Plan::DelayedPostTerminalMutation,
    ] {
        let (store, conversation_id) = fixture().await;
        let adapter = Arc::new(ScriptedAdapter::new(ProviderId::Codex, [plan]));
        let supervisor = RunSupervisor::new(store.clone(), vec![adapter]).unwrap();
        let handle = supervisor
            .submit(request(conversation_id, ProviderId::Codex))
            .await
            .unwrap();

        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), handle.wait())
                .await
                .expect("terminal validation must be bounded")
                .unwrap()
                .status,
            RunStatus::Failed
        );
        assert_eq!(
            store
                .load_run(handle.run_id())
                .await
                .unwrap()
                .mutation_state,
            MutationState::Unknown
        );
        supervisor.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn buffered_activity_after_terminal_is_a_contract_failure() {
    for tail in [
        ProviderEvent::ToolActivity {
            description: "late buffered write".to_owned(),
            mutation: MutationState::Observed,
        },
        ProviderEvent::TurnCompleted,
    ] {
        for delayed in [false, true] {
            let (store, conversation_id) = fixture().await;
            let adapter = Arc::new(
                ScriptedAdapter::new(ProviderId::Codex, [Plan::Approval]).with_response_events(
                    vec![ProviderEvent::TurnCompleted, tail.clone()],
                    delayed,
                ),
            );
            let supervisor = RunSupervisor::new(store.clone(), vec![adapter]).unwrap();
            let handle = supervisor
                .submit(request(conversation_id, ProviderId::Codex))
                .await
                .unwrap();
            handle.wait_for(RunStatus::Waiting).await.unwrap();
            let response = supervisor
                .respond(handle.run_id(), "approval-1", ApprovalResponse::Approved)
                .await;
            assert!(response.is_ok() || matches!(response, Err(RuntimeError::OperationCancelled)));

            assert_eq!(handle.wait().await.unwrap().status, RunStatus::Failed);
            let run = store.load_run(handle.run_id()).await.unwrap();
            assert_eq!(run.status, RunStatus::Failed);
            assert_eq!(run.mutation_state, MutationState::Unknown);
            supervisor.shutdown().await.unwrap();
        }
    }
}

#[tokio::test]
async fn provider_failure_while_waiting_for_approval_is_terminal() {
    for plan in [Plan::ApprovalCrash, Plan::ApprovalClosed] {
        let (store, conversation_id) = fixture().await;
        let adapter = Arc::new(ScriptedAdapter::new(ProviderId::Codex, [plan]));
        let supervisor = RunSupervisor::new(store.clone(), vec![adapter]).unwrap();
        let handle = supervisor
            .submit(request(conversation_id, ProviderId::Codex))
            .await
            .unwrap();

        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), handle.wait())
                .await
                .expect("approval liveness must keep polling the provider")
                .unwrap()
                .status,
            RunStatus::Failed
        );
        assert_eq!(
            store
                .load_approval(handle.run_id(), "approval-1")
                .await
                .unwrap()
                .status,
            ApprovalStatus::Failed
        );
        supervisor.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn buffered_mutation_is_durable_before_interrupt_or_shutdown() {
    for (mutation, app_shutdown) in [
        (MutationState::Observed, false),
        (MutationState::Unknown, true),
    ] {
        let (store, conversation_id) = fixture().await;
        let adapter = Arc::new(ScriptedAdapter::new(
            ProviderId::Codex,
            [Plan::ApprovalWithMutation(mutation)],
        ));
        let supervisor = RunSupervisor::new(store.clone(), vec![adapter]).unwrap();
        let handle = supervisor
            .submit(request(conversation_id, ProviderId::Codex))
            .await
            .unwrap();
        handle.wait_for(RunStatus::Waiting).await.unwrap();

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if store
                    .load_run(handle.run_id())
                    .await
                    .unwrap()
                    .mutation_state
                    == mutation
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("received mutation evidence must become durable while approval is pending");

        if app_shutdown {
            supervisor.shutdown().await.unwrap();
        } else {
            supervisor.interrupt(handle.run_id()).await.unwrap();
            handle.wait_for(RunStatus::Interrupted).await.unwrap();
            supervisor.shutdown().await.unwrap();
        }
        assert_eq!(
            store
                .load_run(handle.run_id())
                .await
                .unwrap()
                .mutation_state,
            mutation
        );
    }
}

#[tokio::test]
async fn buffered_waiting_events_survive_interrupt_shutdown_and_crash_once_in_order() {
    #[derive(Clone, Copy)]
    enum Stop {
        Interrupt,
        Shutdown,
        Crash,
    }

    for stop in [Stop::Interrupt, Stop::Shutdown, Stop::Crash] {
        let (store, conversation_id) = fixture().await;
        let adapter = Arc::new(ScriptedAdapter::new(ProviderId::Codex, [Plan::Approval]));
        let supervisor = RunSupervisor::new(store.clone(), vec![adapter.clone()]).unwrap();
        let handle = supervisor
            .submit(request(conversation_id, ProviderId::Codex))
            .await
            .unwrap();
        handle.wait_for(RunStatus::Waiting).await.unwrap();
        for event in [
            ProviderEvent::AssistantMessage {
                content: "buffered message".to_owned(),
            },
            ProviderEvent::Progress {
                content: "buffered progress".to_owned(),
            },
            ProviderEvent::ToolActivity {
                description: "buffered write".to_owned(),
                mutation: MutationState::Observed,
            },
        ] {
            adapter.send_one(Ok(event));
        }
        while store
            .load_run(handle.run_id())
            .await
            .unwrap()
            .mutation_state
            != MutationState::Observed
        {
            tokio::task::yield_now().await;
        }

        match stop {
            Stop::Interrupt => supervisor.interrupt(handle.run_id()).await.unwrap(),
            Stop::Shutdown => supervisor.shutdown().await.unwrap(),
            Stop::Crash => adapter.send_one(Err(ProviderError::ProcessExited)),
        }
        let expected_status = if matches!(stop, Stop::Crash) {
            RunStatus::Failed
        } else {
            RunStatus::Interrupted
        };
        assert_eq!(handle.wait().await.unwrap().status, expected_status);
        let timeline = store
            .load_timeline(conversation_id, None, 20)
            .await
            .unwrap();
        let buffered = timeline
            .items
            .iter()
            .filter(|event| event.content.starts_with("buffered"))
            .map(|event| event.content.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            buffered,
            ["buffered message", "buffered progress", "buffered write"]
        );
        if !matches!(stop, Stop::Shutdown) {
            supervisor.shutdown().await.unwrap();
        }
    }
}

#[tokio::test]
async fn approval_response_can_ack_behind_a_full_provider_channel() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(
        ScriptedAdapter::new(ProviderId::Codex, [Plan::Approval]).with_response_flood(256),
    );
    let supervisor = RunSupervisor::new(store.clone(), vec![adapter.clone()]).unwrap();
    let handle = supervisor
        .submit(request(conversation_id, ProviderId::Codex))
        .await
        .unwrap();
    handle.wait_for(RunStatus::Waiting).await.unwrap();

    tokio::time::timeout(
        Duration::from_secs(2),
        supervisor.respond(handle.run_id(), "approval-1", ApprovalResponse::Approved),
    )
    .await
    .expect("stream consumption must let the provider deliver its response ack")
    .unwrap();
    adapter.complete_one();
    assert_eq!(handle.wait().await.unwrap().status, RunStatus::Completed);

    let mut cursor = None;
    let mut progress = Vec::new();
    loop {
        let page = store
            .load_timeline(conversation_id, cursor.take(), 200)
            .await
            .unwrap();
        progress.extend(
            page.items
                .iter()
                .filter(|event| event.content.starts_with("buffered-"))
                .map(|event| event.content.clone()),
        );
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(
        progress,
        (0..256)
            .map(|index| format!("buffered-{index}"))
            .collect::<Vec<_>>()
    );
    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn approval_event_buffer_overflow_fails_with_unknown_mutation() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(
        ScriptedAdapter::new(ProviderId::Codex, [Plan::Approval]).with_response_flood(257),
    );
    let supervisor = RunSupervisor::new(store.clone(), vec![adapter]).unwrap();
    let handle = supervisor
        .submit(request(conversation_id, ProviderId::Codex))
        .await
        .unwrap();
    handle.wait_for(RunStatus::Waiting).await.unwrap();

    let _ = supervisor
        .respond(handle.run_id(), "approval-1", ApprovalResponse::Approved)
        .await;
    assert_eq!(handle.wait().await.unwrap().status, RunStatus::Failed);
    assert_eq!(
        store
            .load_run(handle.run_id())
            .await
            .unwrap()
            .mutation_state,
        MutationState::Unknown
    );
    let mut cursor = None;
    let mut overflow_kinds = Vec::new();
    loop {
        let page = store
            .load_timeline(conversation_id, cursor.take(), 200)
            .await
            .unwrap();
        overflow_kinds.extend(
            page.items
                .iter()
                .filter(|event| {
                    event.content == "Provider output omitted: staged queue limit exceeded"
                })
                .map(|event| event.kind),
        );
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(overflow_kinds, [TimelineEventKind::Diagnostic]);
    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn interrupt_timeout_forces_owned_process_shutdown_without_app_shutdown() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(ScriptedAdapter::hanging(
        ProviderId::Codex,
        Plan::OwnedChild,
        HangPoint::Interrupt,
    ));
    let supervisor = RunSupervisor::new(store, vec![adapter.clone()]).unwrap();
    let handle = supervisor
        .submit(request(conversation_id, ProviderId::Codex))
        .await
        .unwrap();
    handle.wait_for(RunStatus::Running).await.unwrap();
    let pid = adapter.owned_pid.load(Ordering::SeqCst);
    assert_ne!(pid, 0);

    supervisor.interrupt(handle.run_id()).await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), handle.wait())
            .await
            .expect("normal interrupt must have its own timeout")
            .unwrap()
            .status,
        RunStatus::Interrupted
    );
    assert!(
        !std::process::Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success()),
        "owned provider child must be reaped before terminal status"
    );
    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn supervisor_shutdown_awaits_owned_process_reaping() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(ScriptedAdapter::new(ProviderId::Codex, [Plan::OwnedChild]));
    let supervisor = RunSupervisor::new(store, vec![adapter.clone()]).unwrap();
    let handle = supervisor
        .submit(request(conversation_id, ProviderId::Codex))
        .await
        .unwrap();
    handle.wait_for(RunStatus::Running).await.unwrap();
    let pid = adapter.owned_pid.load(Ordering::SeqCst);
    assert_ne!(pid, 0);

    supervisor.shutdown().await.unwrap();
    assert_eq!(handle.wait().await.unwrap().status, RunStatus::Interrupted);
    assert!(
        !std::process::Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success()),
        "supervisor shutdown must reap the owned provider child"
    );
}

#[tokio::test]
async fn store_error_after_turn_start_awaits_owner_shutdown_before_reconciliation() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(ScriptedAdapter::new(
        ProviderId::Codex,
        [Plan::TrackedOwner],
    ));
    let supervisor = RunSupervisor::new(store.clone(), vec![adapter.clone()]).unwrap();
    let handle = supervisor
        .submit(request(conversation_id, ProviderId::Codex))
        .await
        .unwrap();
    handle.wait_for(RunStatus::Running).await.unwrap();
    let recovery = store
        .pending_recovery()
        .await
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.run.id == handle.run_id())
        .unwrap();
    let root = recovery
        .agents
        .into_iter()
        .find(|agent| agent.parent_id.is_none())
        .unwrap();
    store
        .append_run_event(handle.run_id(), root.id, ProviderEventRecord::completed())
        .await
        .unwrap();

    adapter.send_one(Ok(ProviderEvent::Progress {
        content: "cannot persist after external terminal".to_owned(),
    }));
    assert_eq!(handle.wait().await.unwrap().status, RunStatus::Completed);
    assert_eq!(adapter.owner_shutdowns.load(Ordering::SeqCst), 1);
    assert!(matches!(
        supervisor.shutdown().await,
        Err(RuntimeError::OwnedTaskFailed)
    ));
}

#[tokio::test]
async fn concurrent_submit_and_shutdown_never_strands_an_accepted_run() {
    let (store, conversation_id) = fixture().await;
    let adapter = Arc::new(ScriptedAdapter::new(ProviderId::Codex, [Plan::Blocking]));
    let supervisor = Arc::new(RunSupervisor::new(store.clone(), vec![adapter]).unwrap());
    let barrier = Arc::new(tokio::sync::Barrier::new(3));

    let submitter = {
        let supervisor = Arc::clone(&supervisor);
        let barrier = Arc::clone(&barrier);
        tokio::spawn(async move {
            barrier.wait().await;
            supervisor
                .submit(request(conversation_id, ProviderId::Codex))
                .await
        })
    };
    let stopper = {
        let supervisor = Arc::clone(&supervisor);
        let barrier = Arc::clone(&barrier);
        tokio::spawn(async move {
            barrier.wait().await;
            supervisor.shutdown().await
        })
    };
    barrier.wait().await;

    let submitted = submitter.await.unwrap();
    stopper.await.unwrap().unwrap();
    if let Ok(handle) = submitted {
        assert_eq!(handle.wait().await.unwrap().status, RunStatus::Interrupted);
    }
    assert!(store.pending_recovery().await.unwrap().is_empty());
}
