use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use prompting_time_core::domain::ConversationId;
use prompting_time_core::providers::{
    ApprovalResponse, ProviderAdapter, ProviderCapabilities, ProviderError, ProviderEvent,
    ProviderHealth, ProviderId, ProviderSession, ProviderTurn, ProviderTurnOwner, ResumeSession,
    StartSession, TurnRequest,
};
use prompting_time_core::router::ProviderCapability;
use tokio::sync::mpsc;

#[derive(Clone, Copy)]
enum Script {
    Approval,
    Interrupted,
    Error,
}

struct FakeAdapter {
    script: Script,
    calls: Arc<Mutex<Vec<String>>>,
    owner_shutdowns: Arc<AtomicUsize>,
}

struct TrackingTurnOwner(Arc<AtomicUsize>);

#[async_trait]
impl ProviderTurnOwner for TrackingTurnOwner {
    async fn shutdown(self: Box<Self>) -> Result<(), ProviderError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl FakeAdapter {
    fn new(script: Script) -> Self {
        Self {
            script,
            calls: Arc::new(Mutex::new(Vec::new())),
            owner_shutdowns: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl ProviderAdapter for FakeAdapter {
    fn id(&self) -> ProviderId {
        ProviderId::Codex
    }

    fn capabilities(&self) -> ProviderCapabilities {
        [
            ProviderCapability::Streaming,
            ProviderCapability::DeferredApproval,
            ProviderCapability::Interruption,
            ProviderCapability::Resume,
        ]
        .into()
    }

    async fn health(&self) -> Result<ProviderHealth, ProviderError> {
        Ok(ProviderHealth::Healthy {
            version: "test-1".to_owned(),
        })
    }

    async fn start_session(
        &self,
        _request: StartSession,
    ) -> Result<ProviderSession, ProviderError> {
        Ok(ProviderSession {
            provider: ProviderId::Codex,
            native_id: "native-session-7".to_owned(),
        })
    }

    async fn resume_session(
        &self,
        native_id: &str,
        _request: ResumeSession,
    ) -> Result<ProviderSession, ProviderError> {
        Ok(ProviderSession {
            provider: ProviderId::Codex,
            native_id: native_id.to_owned(),
        })
    }

    async fn start_turn(
        &self,
        _session: &ProviderSession,
        _request: TurnRequest,
    ) -> Result<ProviderTurn, ProviderError> {
        let (sender, receiver) = mpsc::channel(8);
        let script = self.script;
        tokio::spawn(async move {
            sender
                .send(Ok(ProviderEvent::TurnStarted {
                    native_turn_id: "native-turn-9".to_owned(),
                }))
                .await
                .unwrap();
            match script {
                Script::Approval => {
                    sender
                        .send(Ok(ProviderEvent::ApprovalRequested {
                            request_id: "approval-3".to_owned(),
                            operation: "write".to_owned(),
                            scope: "fixture.txt".to_owned(),
                        }))
                        .await
                        .unwrap();
                    sender.send(Ok(ProviderEvent::TurnCompleted)).await.unwrap();
                }
                Script::Interrupted => {
                    sender.send(Ok(ProviderEvent::Interrupted)).await.unwrap();
                }
                Script::Error => {
                    sender
                        .send(Err(ProviderError::Protocol {
                            category: "fixture".to_owned(),
                        }))
                        .await
                        .unwrap();
                }
            }
        });
        Ok(ProviderTurn::new(
            receiver,
            TrackingTurnOwner(Arc::clone(&self.owner_shutdowns)),
        ))
    }

    async fn steer(
        &self,
        _session: &ProviderSession,
        active_turn: &str,
        text: &str,
    ) -> Result<(), ProviderError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("steer:{active_turn}:{text}"));
        Ok(())
    }

    async fn respond(
        &self,
        _session: &ProviderSession,
        request_id: &str,
        response: ApprovalResponse,
    ) -> Result<(), ProviderError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("respond:{request_id}:{response:?}"));
        Ok(())
    }

    async fn interrupt(
        &self,
        _session: &ProviderSession,
        active_turn: &str,
    ) -> Result<(), ProviderError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("interrupt:{active_turn}"));
        Ok(())
    }
}

fn start_request() -> StartSession {
    StartSession {
        conversation_id: ConversationId::new(),
        working_directory: PathBuf::from("/tmp/invented-project"),
    }
}

#[tokio::test]
async fn adapter_retains_native_ids_and_orders_one_terminal_event() {
    let adapter = FakeAdapter::new(Script::Approval);
    let session = adapter.start_session(start_request()).await.unwrap();
    assert_eq!(session.native_id, "native-session-7");

    let mut stream = adapter
        .start_turn(&session, TurnRequest::new("inspect the fixture"))
        .await
        .unwrap();
    let mut events = Vec::new();
    while let Some(event) = stream.recv().await {
        events.push(event.unwrap());
    }

    assert_eq!(
        events,
        vec![
            ProviderEvent::TurnStarted {
                native_turn_id: "native-turn-9".to_owned()
            },
            ProviderEvent::ApprovalRequested {
                request_id: "approval-3".to_owned(),
                operation: "write".to_owned(),
                scope: "fixture.txt".to_owned(),
            },
            ProviderEvent::TurnCompleted,
        ]
    );
    assert_eq!(events.iter().filter(|event| event.is_terminal()).count(), 1);
}

#[tokio::test]
async fn adapter_contract_supports_approval_resume_and_interruption() {
    let approval = FakeAdapter::new(Script::Approval);
    let session = approval.start_session(start_request()).await.unwrap();
    approval
        .respond(&session, "approval-3", ApprovalResponse::Approved)
        .await
        .unwrap();
    assert_eq!(approval.calls(), vec!["respond:approval-3:Approved"]);

    let interrupted = FakeAdapter::new(Script::Interrupted);
    let session = interrupted.start_session(start_request()).await.unwrap();
    interrupted
        .interrupt(&session, "native-turn-9")
        .await
        .unwrap();
    assert_eq!(interrupted.calls(), vec!["interrupt:native-turn-9"]);
}

#[tokio::test]
async fn adapter_stream_errors_remain_typed() {
    let adapter = FakeAdapter::new(Script::Error);
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut stream = adapter
        .start_turn(&session, TurnRequest::new("fail predictably"))
        .await
        .unwrap();

    assert!(matches!(
        stream.recv().await.unwrap(),
        Ok(ProviderEvent::TurnStarted { .. })
    ));
    assert!(matches!(
        stream.recv().await.unwrap(),
        Err(ProviderError::Protocol { category }) if category == "fixture"
    ));
}

#[tokio::test]
async fn adapter_turn_shutdown_awaits_its_owner_once() {
    let adapter = FakeAdapter::new(Script::Interrupted);
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("shutdown fixture"))
        .await
        .unwrap();

    turn.shutdown().await.unwrap();
    turn.shutdown().await.unwrap();

    assert_eq!(adapter.owner_shutdowns.load(Ordering::SeqCst), 1);
}
