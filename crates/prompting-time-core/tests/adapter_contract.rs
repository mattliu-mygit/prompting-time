use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use prompting_time_core::domain::ConversationId;
use prompting_time_core::providers::codex::CodexAdapter;
use prompting_time_core::providers::{
    ApprovalRequestDetails, ApprovalResponse, FileChangeApprovalDetail, FileChangeKind,
    NativeAgentStatus, NativeChildStatus, NativeSubAgentActivityKind, ProviderAdapter,
    ProviderCapabilities, ProviderError, ProviderEvent, ProviderHealth, ProviderId,
    ProviderSession, ProviderTurn, ProviderTurnOwner, RequestedNetworkPermissions,
    RequestedPermissionProfile, ResumeSession, StartSession, TurnRequest, UserInputOption,
    UserInputQuestion,
};
use prompting_time_core::router::ProviderCapability;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;
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
            native_group_id: None,
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
            native_group_id: None,
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
                            details: None,
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
                details: None,
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

fn fake_codex(script: &str) -> (TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let binary = directory.path().join("codex-fixture");
    fs::write(&binary, format!("#!/bin/sh\nset -eu\n{script}\n")).unwrap();
    let mut permissions = fs::metadata(&binary).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&binary, permissions).unwrap();
    (directory, binary)
}

fn response_id_shell(variable: &str) -> String {
    format!("{variable}=$(printf '%s' \"${{line}}\" | sed -E 's/.*\\\"id\\\":([0-9]+).*/\\1/')")
}

#[tokio::test]
async fn codex_adapter_handshakes_and_uses_explicit_safe_session_policy() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
[ "$1" = "app-server" ]
IFS= read -r line
printf '%s' "$line" | grep -q '"method":"initialize"'
printf '%s' "$line" | grep -q '"name":"prompting_time"'
printf '%s' "$line" | grep -q '"title":"Prompting Time"'
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test","platformFamily":"unix","platformOs":"macos","codexHome":"/invented/codex-home"}}}}\n' "$request_id"
IFS= read -r line
printf '%s' "$line" | grep -q '"method":"initialized"'
IFS= read -r line
printf '%s' "$line" | grep -q '"method":"thread/start"'
printf '%s' "$line" | grep -q '"cwd":"/tmp/invented-project"'
printf '%s' "$line" | grep -q '"approvalPolicy":"on-request"'
printf '%s' "$line" | grep -q '"sandbox":"workspace-write"'
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-7","sessionId":"session-3"}}}}}}\n' "$request_id"
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);

    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();

    assert_eq!(session.provider, ProviderId::Codex);
    assert_eq!(session.native_id, "thread-7");
    assert_eq!(session.native_group_id.as_deref(), Some("session-3"));
}

#[tokio::test]
async fn codex_adapter_correlates_out_of_order_responses_by_request_id() {
    let extract_first = response_id_shell("first_id");
    let extract_second = response_id_shell("second_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_first}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test","platformFamily":"unix","platformOs":"macos","codexHome":"/invented/codex-home"}}}}\n' "$first_id"
IFS= read -r line
IFS= read -r line
{extract_first}
IFS= read -r line
{extract_second}
[ "$second_id" -gt "$first_id" ]
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-second","sessionId":"session-second"}}}}}}\n' "$second_id"
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-first","sessionId":"session-first"}}}}}}\n' "$first_id"
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();

    let (first, second) = tokio::join!(
        adapter.start_session(start_request()),
        adapter.start_session(start_request())
    );

    assert!(first.unwrap().native_id.contains("thread-first"));
    assert!(second.unwrap().native_id.contains("thread-second"));
}

#[tokio::test]
async fn abandoned_codex_requests_release_pending_capacity_and_late_replies_are_harmless() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test","platformFamily":"unix","platformOs":"macos","codexHome":"/invented/codex-home"}}}}\n' "$request_id"
IFS= read -r line
i=1
while [ "$i" -le 130 ]; do
  IFS= read -r line
  i=$((i + 1))
done
i=1
while [ "$i" -le 130 ]; do
  printf '{{"id":%s,"result":{{"thread":{{"id":"thread-late","sessionId":"session-late"}}}}}}\n' "$i"
  i=$((i + 1))
done
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-responsive","sessionId":"session-responsive"}}}}}}\n' "$request_id"
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();

    for _ in 0..130 {
        let abandoned_adapter = adapter.clone();
        let request =
            tokio::spawn(async move { abandoned_adapter.start_session(start_request()).await });
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        request.abort();
        let _ = request.await;
    }

    let session = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        adapter.start_session(start_request()),
    )
    .await
    .expect("abandoned requests permanently consumed pending capacity")
    .unwrap();
    assert_eq!(session.native_id, "thread-responsive");
}

#[tokio::test]
async fn numeric_and_string_codex_server_request_ids_remain_distinct() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test","platformFamily":"unix","platformOs":"macos","codexHome":"/invented/codex-home"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-7","sessionId":"session-3"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-7","turn":{{"id":"turn-9","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-9","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"method":"item/commandExecution/requestApproval","id":5,"params":{{"threadId":"thread-7","turnId":"turn-9","itemId":"command-5","startedAtMs":1,"reason":"numeric"}}}}\n'
printf '{{"method":"item/commandExecution/requestApproval","id":"number:5","params":{{"threadId":"thread-7","turnId":"turn-9","itemId":"command-string-5","startedAtMs":2,"reason":"string"}}}}\n'
IFS= read -r line
IFS= read -r line
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("request colliding ids"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    let first = turn.recv().await.unwrap().unwrap();
    let second = turn.recv().await.unwrap().unwrap();
    let ProviderEvent::ApprovalRequested {
        request_id: first_id,
        ..
    } = first
    else {
        panic!("expected first approval");
    };
    let ProviderEvent::ApprovalRequested {
        request_id: second_id,
        ..
    } = second
    else {
        panic!("expected second approval");
    };
    assert_ne!(first_id, second_id);
    adapter
        .respond(&session, &first_id, ApprovalResponse::Denied)
        .await
        .unwrap();
    adapter
        .respond(&session, &second_id, ApprovalResponse::Denied)
        .await
        .unwrap();
}

#[tokio::test]
async fn the_same_raw_id_is_independent_in_client_and_server_directions() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-opposite-id","sessionId":"session-opposite-id"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '%s' "$request_id" | grep -q '^2$'
printf '{{"method":"turn/started","params":{{"threadId":"thread-opposite-id","turn":{{"id":"turn-opposite-id","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":2,"method":"item/commandExecution/requestApproval","params":{{"threadId":"thread-opposite-id","turnId":"turn-opposite-id","itemId":"command-opposite-id","startedAtMs":1,"command":"printf invented","cwd":"/invented/project"}}}}\n'
printf '{{"id":2,"result":{{"turn":{{"id":"turn-opposite-id","items":[],"status":"inProgress"}}}}}}\n'
IFS= read -r line
printf '%s' "$line" | grep -q '"id":2'
printf '%s' "$line" | grep -q '"decision":"decline"'
printf '{{"method":"turn/completed","params":{{"threadId":"thread-opposite-id","turn":{{"id":"turn-opposite-id","items":[],"status":"completed"}}}}}}\n'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("same ID in opposite directions"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    let request_id = match turn.recv().await.unwrap().unwrap() {
        ProviderEvent::ApprovalRequested { request_id, .. } => request_id,
        event => panic!("expected approval, got {event:?}"),
    };
    assert_eq!(request_id, "number:2");
    adapter
        .respond(&session, &request_id, ApprovalResponse::Denied)
        .await
        .unwrap();
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnCompleted
    );
}

#[tokio::test]
async fn malformed_client_response_id_is_connection_fatal() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
printf '{{"id":null,"result":{{"thread":{{"id":"thread-invalid-response"}}}}}}\n'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();

    assert!(adapter.start_session(start_request()).await.is_err());
    wait_for_unavailable(&adapter).await;
}

#[tokio::test]
async fn codex_approval_response_must_match_its_active_session_and_turn() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test","platformFamily":"unix","platformOs":"macos","codexHome":"/invented/codex-home"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-a","sessionId":"session-a"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-a","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"method":"item/commandExecution/requestApproval","id":"approval-a","params":{{"threadId":"thread-a","turnId":"turn-a","itemId":"command-a","startedAtMs":1}}}}\n'
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-b","sessionId":"session-b"}}}}}}\n' "$request_id"
IFS= read -r line
printf '%s' "$line" | grep -q '"id":"approval-a"'
printf '%s' "$line" | grep -q '"decision":"accept"'
printf '{{"method":"turn/completed","params":{{"threadId":"thread-a","turn":{{"id":"turn-a","items":[],"status":"completed"}}}}}}\n'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session_a = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session_a, TurnRequest::new("request invented approval"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    let request_id = match turn.recv().await.unwrap().unwrap() {
        ProviderEvent::ApprovalRequested { request_id, .. } => request_id,
        event => panic!("expected approval, got {event:?}"),
    };
    let session_b = adapter.start_session(start_request()).await.unwrap();
    assert!(matches!(
        adapter
            .respond(&session_b, &request_id, ApprovalResponse::Approved)
            .await,
        Err(ProviderError::Protocol { category }) if category == "server-request-owner-mismatch"
    ));
    adapter
        .respond(&session_a, &request_id, ApprovalResponse::Approved)
        .await
        .unwrap();
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnCompleted
    );
    turn.shutdown().await.unwrap();
}

#[tokio::test]
async fn terminal_codex_turn_rejects_and_forgets_every_outstanding_request() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test","platformFamily":"unix","platformOs":"macos","codexHome":"/invented/codex-home"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-terminal","sessionId":"session-terminal"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-terminal","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"method":"item/commandExecution/requestApproval","id":"approval-terminal","params":{{"threadId":"thread-terminal","turnId":"turn-terminal","itemId":"command-terminal","startedAtMs":1000}}}}\n'
printf '{{"method":"item/fileChange/patchUpdated","params":{{"threadId":"thread-terminal","turnId":"turn-terminal","itemId":"file-terminal","changes":[{{"path":"/invented/project/file.txt","kind":{{"type":"add"}},"diff":""}}]}}}}\n'
printf '{{"method":"item/fileChange/requestApproval","id":"file-terminal","params":{{"threadId":"thread-terminal","turnId":"turn-terminal","itemId":"file-terminal","startedAtMs":1002}}}}\n'
printf '{{"method":"turn/completed","params":{{"threadId":"thread-terminal","turn":{{"id":"turn-terminal","items":[],"status":"completed"}}}}}}\n'
IFS= read -r line
IFS= read -r second
printf '%s\n%s' "$line" "$second" | grep -q '"id":"approval-terminal"'
printf '%s\n%s' "$line" "$second" | grep -q '"id":"file-terminal"'
printf '%s\n%s' "$line" "$second" | grep -q '"code":-32004'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("finish before approval"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    let mut request_ids = Vec::new();
    for _ in 0..2 {
        match turn.recv().await.unwrap().unwrap() {
            ProviderEvent::ApprovalRequested { request_id, .. } => request_ids.push(request_id),
            event => panic!("expected approval, got {event:?}"),
        }
    }
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnCompleted
    );
    for request_id in request_ids {
        assert!(matches!(
            adapter
                .respond(&session, &request_id, ApprovalResponse::Approved)
                .await,
            Err(ProviderError::Protocol { category }) if category == "unknown-server-request"
        ));
    }
    turn.shutdown().await.unwrap();
}

#[tokio::test]
async fn delayed_completion_from_a_prior_turn_cannot_complete_the_current_turn() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test","platformFamily":"unix","platformOs":"macos","codexHome":"/invented/codex-home"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-7","sessionId":"session-3"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-7","turn":{{"id":"turn-old","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-old","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"method":"turn/completed","params":{{"threadId":"thread-7","turn":{{"id":"turn-old","items":[],"status":"completed"}}}}}}\n'
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-7","turn":{{"id":"turn-new","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-new","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"method":"turn/completed","params":{{"threadId":"thread-7","turn":{{"id":"turn-old","items":[],"status":"completed"}}}}}}\n'
printf '{{"method":"item/commandExecution/requestApproval","id":"approval-old","params":{{"threadId":"thread-7","turnId":"turn-old","itemId":"command-old"}}}}\n'
IFS= read -r line
printf '%s' "$line" | grep -q '"id":"approval-old"'
printf '%s' "$line" | grep -q '"code":-32003'
printf '{{"method":"item/agentMessage/delta","params":{{"threadId":"thread-7","turnId":"turn-new","itemId":"message-new","delta":"NEW"}}}}\n'
printf '{{"method":"turn/completed","params":{{"threadId":"thread-7","turn":{{"id":"turn-new","items":[],"status":"completed"}}}}}}\n'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut first = adapter
        .start_turn(&session, TurnRequest::new("first"))
        .await
        .unwrap();
    assert!(matches!(
        first.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    assert_eq!(
        first.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnCompleted
    );
    first.shutdown().await.unwrap();

    let mut second = adapter
        .start_turn(&session, TurnRequest::new("second"))
        .await
        .unwrap();
    assert!(
        matches!(second.recv().await.unwrap().unwrap(), ProviderEvent::TurnStarted { native_turn_id } if native_turn_id == "turn-new")
    );
    assert!(
        matches!(second.recv().await.unwrap().unwrap(), ProviderEvent::AssistantMessageDelta { content, .. } if content == "NEW")
    );
    assert_eq!(
        second.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnCompleted
    );
}

#[tokio::test]
async fn codex_health_becomes_unavailable_when_the_dispatcher_process_exits() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test","platformFamily":"unix","platformOs":"macos","codexHome":"/invented/codex-home"}}}}\n' "$request_id"
IFS= read -r line
exit 0
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();

    let health = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let health = adapter.health().await.unwrap();
            if matches!(health, ProviderHealth::Unavailable { .. }) {
                break health;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("dead Codex app-server remained healthy");
    assert!(matches!(health, ProviderHealth::Unavailable { .. }));
}

#[tokio::test]
async fn codex_adapter_streams_schema_valid_fixture_events() {
    let fixture = include_str!("fixtures/codex/session.jsonl");
    let mut script = String::new();
    for line in fixture.lines() {
        let envelope: serde_json::Value = serde_json::from_str(line).unwrap();
        if envelope["direction"] == "client" {
            script.push_str("IFS= read -r line || exit 1\n");
        } else {
            let message = serde_json::to_string(&envelope["message"]).unwrap();
            assert!(!message.contains('\''));
            script.push_str(&format!("printf '%s\\n' '{message}'\n"));
        }
    }
    script.push_str("sleep 30\n");
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    assert_eq!(session.native_id, "thread-invented-1");
    assert_eq!(
        session.native_group_id.as_deref(),
        Some("session-invented-1")
    );
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("Inspect the invented fixture."))
        .await
        .unwrap();
    let mut saw_message = false;
    let mut saw_child = false;
    let mut saw_subagent = false;
    loop {
        match turn.recv().await.unwrap().unwrap() {
            ProviderEvent::AssistantMessageDelta { content, .. } => {
                saw_message |= content == "READY"
            }
            ProviderEvent::ChildAgentActivity { .. } => saw_child = true,
            ProviderEvent::SubAgentActivity { .. } => saw_subagent = true,
            ProviderEvent::ApprovalRequested { request_id, .. } => {
                adapter
                    .respond(&session, &request_id, ApprovalResponse::Approved)
                    .await
                    .unwrap();
            }
            ProviderEvent::UserInputRequested { request_id, .. } => {
                adapter
                    .respond(
                        &session,
                        &request_id,
                        ApprovalResponse::Answers(BTreeMap::from([
                            ("question-a".to_owned(), vec!["alpha".to_owned()]),
                            ("question-b".to_owned(), vec!["invented".to_owned()]),
                        ])),
                    )
                    .await
                    .unwrap();
            }
            ProviderEvent::TurnCompleted => break,
            _ => {}
        }
    }
    assert!(saw_message && saw_child && saw_subagent);
    turn.shutdown().await.unwrap();
    assert!(fixture.contains("\"decision\":\"accept\""));
    assert!(!fixture.contains("/Users/"));
}

#[tokio::test]
async fn codex_adapter_normalizes_streamed_native_items_children_and_approval() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test","platformFamily":"unix","platformOs":"macos","codexHome":"/invented/codex-home"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-7","sessionId":"session-3"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-7","turn":{{"id":"turn-9","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-9","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"method":"item/agentMessage/delta","params":{{"threadId":"thread-7","turnId":"turn-9","itemId":"message-item-2","delta":"READY"}}}}\n'
printf '{{"method":"item/started","params":{{"threadId":"thread-7","turnId":"turn-9","startedAtMs":1000,"item":{{"type":"commandExecution","id":"command-item-3","command":"printf READY","commandActions":[],"cwd":"/invented/project","status":"inProgress"}}}}}}\n'
printf '{{"method":"item/started","params":{{"threadId":"thread-7","turnId":"turn-9","startedAtMs":1001,"item":{{"type":"collabAgentToolCall","id":"child-item-4","tool":"spawnAgent","status":"inProgress","senderThreadId":"thread-7","receiverThreadIds":["thread-child-8"],"agentsStates":{{"thread-child-8":{{"status":"running"}}}}}}}}}}\n'
printf '{{"method":"item/started","params":{{"threadId":"thread-7","turnId":"turn-9","startedAtMs":1001,"item":{{"type":"subAgentActivity","id":"subagent-item-5","agentThreadId":"thread-child-8","agentPath":"researcher","kind":"interacted"}}}}}}\n'
printf '{{"method":"item/commandExecution/requestApproval","id":"approval-5","params":{{"threadId":"thread-7","turnId":"turn-9","itemId":"command-item-3","startedAtMs":1002,"command":"printf READY","cwd":"/invented/project","reason":"Run the invented command."}}}}\n'
printf '{{"method":"future/inventedNotification","params":{{"threadId":"thread-7","turnId":"turn-9"}}}}\n'
IFS= read -r line
printf '%s' "$line" | grep -q '"id":"approval-5"'
printf '%s' "$line" | grep -q '"decision":"accept"'
printf '{{"method":"item/tool/requestUserInput","id":"input-6","params":{{"threadId":"thread-7","turnId":"turn-9","itemId":"input-item-6","autoResolutionMs":5000,"questions":[{{"id":"question-a","header":"Choice","question":"Which invented option?","options":[{{"label":"Alpha","description":"First invented option"}}],"isOther":true,"isSecret":false}},{{"id":"question-b","header":"Token","question":"Supply an invented token?","options":null,"isOther":false,"isSecret":true}}]}}}}\n'
IFS= read -r line
printf '%s' "$line" | grep -q '"id":"input-6"'
printf '%s' "$line" | grep -q '"question-a":{{"answers":\["alpha"\]}}'
printf '%s' "$line" | grep -q '"question-b":{{"answers":\["invented-token","second"\]}}'
printf '{{"method":"turn/completed","params":{{"threadId":"thread-7","turn":{{"id":"turn-9","items":[],"status":"completed"}}}}}}\n'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("inspect invented data"))
        .await
        .unwrap();

    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted {
            native_turn_id: "turn-9".to_owned(),
        }
    );
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::AssistantMessageDelta {
            native_item_id: "message-item-2".to_owned(),
            content: "READY".to_owned(),
        }
    );
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::NativeItemActivity { native_item_id, mutation: prompting_time_core::domain::MutationState::Unknown, .. }
            if native_item_id == "command-item-3"
    ));
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::ChildAgentActivity { native_item_id, parent_native_thread_id, child_native_thread_ids, child_statuses, .. }
            if native_item_id == "child-item-4"
                && parent_native_thread_id == "thread-7"
                && child_native_thread_ids == vec!["thread-child-8"]
                && child_statuses == vec![NativeChildStatus {
                    native_thread_id: "thread-child-8".to_owned(),
                    status: NativeAgentStatus::Running,
                }]
    ));
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::SubAgentActivity {
            native_item_id: "subagent-item-5".to_owned(),
            agent_thread_id: "thread-child-8".to_owned(),
            agent_path: "researcher".to_owned(),
            activity: NativeSubAgentActivityKind::Interacted,
        }
    );
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::ApprovalRequested { request_id, details, .. }
            if request_id == "string:approval-5"
                && details == Some(ApprovalRequestDetails::CommandExecution {
                    command: Some("printf READY".to_owned()),
                    cwd: Some("/invented/project".to_owned()),
                })
    ));
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::Unrecognized {
            method: "future/inventedNotification".to_owned(),
        }
    );

    adapter
        .respond(&session, "string:approval-5", ApprovalResponse::Approved)
        .await
        .unwrap();
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::UserInputRequested {
            request_id: "string:input-6".to_owned(),
            questions: vec![
                UserInputQuestion {
                    id: "question-a".to_owned(),
                    header: "Choice".to_owned(),
                    question: "Which invented option?".to_owned(),
                    options: Some(vec![UserInputOption {
                        label: "Alpha".to_owned(),
                        description: "First invented option".to_owned(),
                    }]),
                    is_other: true,
                    is_secret: false,
                },
                UserInputQuestion {
                    id: "question-b".to_owned(),
                    header: "Token".to_owned(),
                    question: "Supply an invented token?".to_owned(),
                    options: None,
                    is_other: false,
                    is_secret: true,
                },
            ],
            auto_resolution_ms: Some(5000),
        }
    );
    assert!(matches!(
        adapter
            .respond(
                &session,
                "string:input-6",
                ApprovalResponse::Answer("must not be copied".to_owned()),
            )
            .await,
        Err(ProviderError::Protocol { category }) if category == "response-kind-mismatch"
    ));
    assert!(matches!(
        adapter
            .respond(
                &session,
                "string:input-6",
                ApprovalResponse::Answers(BTreeMap::from([(
                    "question-a".to_owned(),
                    vec!["alpha".to_owned()],
                )])),
            )
            .await,
        Err(ProviderError::Protocol { category }) if category == "user-input-answer-shape"
    ));
    adapter
        .respond(
            &session,
            "string:input-6",
            ApprovalResponse::Answers(BTreeMap::from([
                ("question-a".to_owned(), vec!["alpha".to_owned()]),
                (
                    "question-b".to_owned(),
                    vec!["invented-token".to_owned(), "second".to_owned()],
                ),
            ])),
        )
        .await
        .unwrap();
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnCompleted
    );
    turn.shutdown().await.unwrap();
}

#[tokio::test]
async fn codex_adapter_resumes_steers_and_interrupts_with_native_ids() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test","platformFamily":"unix","platformOs":"macos","codexHome":"/invented/codex-home"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
printf '%s' "$line" | grep -q '"method":"thread/resume"'
printf '%s' "$line" | grep -q '"threadId":"thread-existing"'
printf '%s' "$line" | grep -q '"approvalPolicy":"on-request"'
printf '%s' "$line" | grep -q '"sandbox":"workspace-write"'
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-existing","sessionId":"session-existing"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-existing","turn":{{"id":"turn-active","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-active","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
IFS= read -r line
printf '%s' "$line" | grep -q '"method":"turn/steer"'
printf '%s' "$line" | grep -q '"expectedTurnId":"turn-active"'
{extract_id}
printf '{{"id":%s,"result":{{"turnId":"turn-active"}}}}\n' "$request_id"
IFS= read -r line
printf '%s' "$line" | grep -q '"method":"turn/interrupt"'
printf '%s' "$line" | grep -q '"turnId":"turn-active"'
{extract_id}
printf '{{"id":%s,"result":{{}}}}\n' "$request_id"
printf '{{"method":"turn/completed","params":{{"threadId":"thread-existing","turn":{{"id":"turn-active","items":[],"status":"interrupted"}}}}}}\n'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter
        .resume_session(
            "thread-existing",
            ResumeSession {
                conversation_id: ConversationId::new(),
                working_directory: PathBuf::from("/tmp/invented-project"),
            },
        )
        .await
        .unwrap();
    assert_eq!(session.native_group_id.as_deref(), Some("session-existing"));
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("begin"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { native_turn_id } if native_turn_id == "turn-active"
    ));

    adapter
        .steer(&session, "turn-active", "add this")
        .await
        .unwrap();
    adapter.interrupt(&session, "turn-active").await.unwrap();
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::Interrupted
    );
    turn.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelling_codex_turn_start_interrupts_the_native_turn() {
    let marker_directory = tempfile::tempdir().unwrap();
    let marker = marker_directory.path().join("interrupted");
    let marker_text = marker.to_string_lossy();
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test","platformFamily":"unix","platformOs":"macos","codexHome":"/invented/codex-home"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-7","sessionId":"session-3"}}}}}}\n' "$request_id"
IFS= read -r line
printf '{{"method":"turn/started","params":{{"threadId":"thread-7","turn":{{"id":"turn-cancelled","items":[],"status":"inProgress"}}}}}}\n'
IFS= read -r line
printf '%s' "$line" | grep -q '"method":"turn/interrupt"'
printf '%s' "$line" | grep -q '"turnId":"turn-cancelled"'
{extract_id}
: > '{marker_text}'
printf '{{"id":%s,"result":{{}}}}\n' "$request_id"
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let adapter_for_turn = adapter.clone();
    let turn = tokio::spawn(async move {
        adapter_for_turn
            .start_turn(&session, TurnRequest::new("cancel this start"))
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    turn.abort();

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !marker.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cancelled turn start did not interrupt the native turn");
}

#[tokio::test]
async fn cancelling_turn_start_before_announcement_interrupts_when_id_arrives() {
    let marker_directory = tempfile::tempdir().unwrap();
    let marker = marker_directory
        .path()
        .join("interrupted-after-announcement");
    let marker_text = marker.to_string_lossy();
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-late-id","sessionId":"session-late-id"}}}}}}\n' "$request_id"
IFS= read -r line
sleep 1
printf '{{"method":"turn/started","params":{{"threadId":"thread-late-id","turn":{{"id":"turn-late-id","items":[],"status":"inProgress"}}}}}}\n'
IFS= read -r line
printf '%s' "$line" | grep -q '"method":"turn/interrupt"'
printf '%s' "$line" | grep -q '"turnId":"turn-late-id"'
{extract_id}
: > '{marker_text}'
printf '{{"id":%s,"result":{{}}}}\n' "$request_id"
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let turn_adapter = adapter.clone();
    let start = tokio::spawn(async move {
        turn_adapter
            .start_turn(&session, TurnRequest::new("cancel before ID"))
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    start.abort();
    let _ = start.await;

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !marker.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cancelled provisional turn was never interrupted");
}

#[tokio::test]
async fn cancelling_turn_start_with_only_a_late_response_terminates_the_connection() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-response-only","sessionId":"session-response-only"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
sleep 1
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-response-only","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let turn_adapter = adapter.clone();
    let start = tokio::spawn(async move {
        turn_adapter
            .start_turn(&session, TurnRequest::new("cancel before response"))
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    start.abort();
    let _ = start.await;

    wait_for_unavailable(&adapter).await;
}

#[tokio::test]
async fn unconfirmed_cleanup_interrupt_terminates_the_connection() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-no-ack","sessionId":"session-no-ack"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-no-ack","turn":{{"id":"turn-no-ack","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-no-ack","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
IFS= read -r line
printf '%s' "$line" | grep -q '"method":"turn/interrupt"'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let turn = adapter
        .start_turn(&session, TurnRequest::new("drop without ack"))
        .await
        .unwrap();
    drop(turn);

    wait_for_unavailable(&adapter).await;
}

#[tokio::test]
async fn failed_turn_completion_closes_the_turn_without_interrupting_it() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-failed","sessionId":"session-failed"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-failed","turn":{{"id":"turn-failed","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-failed","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"method":"turn/completed","params":{{"threadId":"thread-failed","turn":{{"id":"turn-failed","items":[],"status":"failed"}}}}}}\n'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("fail terminally"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    assert!(
        matches!(turn.recv().await.unwrap(), Err(ProviderError::Protocol { category }) if category == "turn-failed")
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), turn.recv())
            .await
            .expect("failed terminal retained its turn sink")
            .is_none()
    );
    turn.shutdown().await.unwrap();
}

#[tokio::test]
async fn recognized_notification_without_turn_identity_is_connection_fatal() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-bad-note","sessionId":"session-bad-note"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-bad-note","turn":{{"id":"turn-bad-note","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-bad-note","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"method":"item/agentMessage/delta","params":{{"threadId":"thread-bad-note","itemId":"message-bad-note","delta":"lost"}}}}\n'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("malformed notification"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(1), turn.recv())
            .await
            .expect("malformed recognized notification was silently dropped")
            .unwrap(),
        Err(ProviderError::Protocol { .. })
    ));
    wait_for_unavailable(&adapter).await;
}

#[tokio::test]
async fn dropping_last_codex_adapter_reaps_its_app_server_process() {
    let process_directory = tempfile::tempdir().unwrap();
    let pid_file = process_directory.path().join("pid");
    let pid_text = pid_file.to_string_lossy();
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
printf '%s' "$$" > '{pid_text}'
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test","platformFamily":"unix","platformOs":"macos","codexHome":"/invented/codex-home"}}}}\n' "$request_id"
IFS= read -r line
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let pid = fs::read_to_string(&pid_file).unwrap();

    drop(adapter);

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let alive = std::process::Command::new("/bin/kill")
                .args(["-0", &pid])
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if !alive {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("dropping the adapter left the app-server process alive");
}

#[tokio::test]
async fn dropping_codex_adapter_reaps_a_child_blocked_on_a_large_stdin_write() {
    let process_directory = tempfile::tempdir().unwrap();
    let pid_file = process_directory.path().join("pid");
    let pid_text = pid_file.to_string_lossy();
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
printf '%s' "$$" > '{pid_text}'
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test","platformFamily":"unix","platformOs":"macos","codexHome":"/invented/codex-home"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-blocked","sessionId":"session-blocked"}}}}}}\n' "$request_id"
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let writer_adapter = adapter.clone();
    let writer = tokio::spawn(async move {
        writer_adapter
            .steer(&session, "turn-blocked", &"x".repeat(4 * 1024 * 1024))
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    writer.abort();
    let _ = writer.await;
    drop(adapter);

    let pid = fs::read_to_string(&pid_file).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let alive = std::process::Command::new("/bin/kill")
                .args(["-0", &pid])
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if !alive {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("dropping the adapter did not reap the stdin-blocked app-server");
}

async fn wait_for_unavailable(adapter: &CodexAdapter) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if matches!(
                adapter.health().await.unwrap(),
                ProviderHealth::Unavailable { .. }
            ) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Codex adapter remained healthy after a fatal connection error");
}

#[tokio::test]
async fn codex_buffers_turn_events_and_requests_until_turn_start_is_correlated() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-early","sessionId":"session-early"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-early","turn":{{"id":"turn-early","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"method":"item/agentMessage/delta","params":{{"threadId":"thread-early","turnId":"turn-early","itemId":"message-early","delta":"EARLY"}}}}\n'
printf '{{"id":"approval-early","method":"item/commandExecution/requestApproval","params":{{"threadId":"thread-early","turnId":"turn-early","itemId":"command-early","startedAtMs":1,"reason":"fixture"}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-early","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
IFS= read -r line
printf '%s' "$line" | grep -q '"id":"approval-early"'
printf '%s' "$line" | grep -q '"decision":"accept"'
printf '{{"method":"turn/completed","params":{{"threadId":"thread-early","turn":{{"id":"turn-early","items":[],"status":"completed"}}}}}}\n'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("begin"))
        .await
        .unwrap();

    assert!(
        matches!(turn.recv().await.unwrap().unwrap(), ProviderEvent::TurnStarted { native_turn_id } if native_turn_id == "turn-early")
    );
    assert!(
        matches!(turn.recv().await.unwrap().unwrap(), ProviderEvent::AssistantMessageDelta { native_item_id, content } if native_item_id == "message-early" && content == "EARLY")
    );
    let request_id = match turn.recv().await.unwrap().unwrap() {
        ProviderEvent::ApprovalRequested { request_id, .. } => request_id,
        other => panic!("expected buffered approval request, got {other:?}"),
    };
    adapter
        .respond(&session, &request_id, ApprovalResponse::Approved)
        .await
        .unwrap();
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnCompleted
    );
}

#[tokio::test]
async fn codex_preserves_fast_completion_before_turn_start_response() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-fast","sessionId":"session-fast"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-fast","turn":{{"id":"turn-fast","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"method":"item/agentMessage/delta","params":{{"threadId":"thread-fast","turnId":"turn-fast","itemId":"message-fast","delta":"DONE"}}}}\n'
printf '{{"method":"turn/completed","params":{{"threadId":"thread-fast","turn":{{"id":"turn-fast","items":[],"status":"completed"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-fast","items":[],"status":"completed"}}}}}}\n' "$request_id"
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("finish quickly"))
        .await
        .unwrap();

    assert!(
        matches!(turn.recv().await.unwrap().unwrap(), ProviderEvent::TurnStarted { native_turn_id } if native_turn_id == "turn-fast")
    );
    assert!(
        matches!(turn.recv().await.unwrap().unwrap(), ProviderEvent::AssistantMessageDelta { content, .. } if content == "DONE")
    );
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnCompleted
    );
}

#[tokio::test]
async fn provisional_terminal_never_replays_an_already_rejected_request() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-early-terminal","sessionId":"session-early-terminal"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-early-terminal","turn":{{"id":"turn-early-terminal","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":"approval-early-terminal","method":"item/commandExecution/requestApproval","params":{{"threadId":"thread-early-terminal","turnId":"turn-early-terminal","itemId":"command-early-terminal","startedAtMs":1,"reason":"fixture"}}}}\n'
printf '{{"method":"turn/completed","params":{{"threadId":"thread-early-terminal","turn":{{"id":"turn-early-terminal","items":[],"status":"completed"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-early-terminal","items":[],"status":"completed"}}}}}}\n' "$request_id"
IFS= read -r line
printf '%s' "$line" | grep -q '"id":"approval-early-terminal"'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("terminal before response"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnCompleted
    );
    assert!(turn.recv().await.is_none());
}

#[tokio::test]
async fn cancelling_a_queued_request_does_not_kill_an_acceptably_delayed_connection() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-delay","sessionId":"session-delay"}}}}}}\n' "$request_id"
sleep 1
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-after-delay","sessionId":"session-after-delay"}}}}}}\n' "$request_id"
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let writer_adapter = adapter.clone();
    let writer = tokio::spawn(async move {
        writer_adapter
            .steer(&session, "turn-delay", &"x".repeat(4 * 1024 * 1024))
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let queued_adapter = adapter.clone();
    let queued = tokio::spawn(async move { queued_adapter.start_session(start_request()).await });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    queued.abort();
    let _ = queued.await;

    tokio::time::timeout(std::time::Duration::from_secs(2), writer)
        .await
        .expect("acceptably delayed write never recovered")
        .unwrap()
        .unwrap();
    assert!(matches!(
        adapter.health().await.unwrap(),
        ProviderHealth::Healthy { .. }
    ));
    let session = adapter.start_session(start_request()).await.unwrap();
    assert_eq!(session.native_id, "thread-after-delay");
}

#[tokio::test]
async fn abandoning_a_request_blocked_on_stdin_terminates_the_connection() {
    let marker_directory = tempfile::tempdir().unwrap();
    let marker = marker_directory.path().join("stopped-reading");
    let marker_text = marker.to_string_lossy();
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-blocked","sessionId":"session-blocked"}}}}}}\n' "$request_id"
: > '{marker_text}'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !marker.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fake Codex process did not stop reading stdin");
    let writer_adapter = adapter.clone();
    let writer = tokio::spawn(async move {
        writer_adapter
            .steer(&session, "turn-blocked", &"x".repeat(4 * 1024 * 1024))
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    writer.abort();
    let _ = writer.await;

    wait_for_unavailable(&adapter).await;
    assert!(adapter.start_session(start_request()).await.is_err());
}

#[tokio::test]
async fn failed_owner_interrupt_terminates_connection_without_claiming_completion() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-owner","sessionId":"session-owner"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-owner","turn":{{"id":"turn-owner","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-owner","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"error":{{"code":-32000,"message":"interrupt failed"}}}}\n' "$request_id"
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("keep mutating"))
        .await
        .unwrap();

    assert!(turn.shutdown().await.is_err());
    wait_for_unavailable(&adapter).await;
}

#[tokio::test]
async fn malformed_or_mismatched_turn_start_response_is_connection_fatal() {
    for result in [
        r#"{"turn":{"items":[],"status":"inProgress"}}"#,
        r#"{"turn":{"id":"turn-other","items":[],"status":"inProgress"}}"#,
    ] {
        let extract_id = response_id_shell("request_id");
        let script = format!(
            r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-malformed","sessionId":"session-malformed"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-malformed","turn":{{"id":"turn-announced","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{result}}}\n' "$request_id"
sleep 30
"#
        );
        let (_directory, binary) = fake_codex(&script);
        let adapter = CodexAdapter::connect(binary).await.unwrap();
        let session = adapter.start_session(start_request()).await.unwrap();

        assert!(
            adapter
                .start_turn(&session, TurnRequest::new("malformed"))
                .await
                .is_err()
        );
        wait_for_unavailable(&adapter).await;
    }
}

#[tokio::test]
async fn failed_server_request_rejection_is_connection_fatal_before_completion() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-reject","sessionId":"session-reject"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-reject","turn":{{"id":"turn-reject","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-reject","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"id":"approval-reject","method":"item/commandExecution/requestApproval","params":{{"threadId":"thread-reject","turnId":"turn-reject","itemId":"command-reject","startedAtMs":1,"reason":"fixture"}}}}\n'
sleep 1
exec 0<&-
printf '{{"method":"turn/completed","params":{{"threadId":"thread-reject","turn":{{"id":"turn-reject","items":[],"status":"completed"}}}}}}\n'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("request approval"))
        .await
        .unwrap();

    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::ApprovalRequested { .. }
    ));
    assert!(matches!(
        turn.recv().await.unwrap(),
        Err(ProviderError::Transport { .. })
    ));
    wait_for_unavailable(&adapter).await;
}

#[tokio::test]
async fn codex_adapter_errors_preserve_code_and_message_but_drop_data_payload() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test","platformFamily":"unix","platformOs":"macos","codexHome":"/invented/codex-home"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"error":{{"code":-32042,"message":"invented rejection","data":{{"secret":"must-not-leak"}}}}}}\n' "$request_id"
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();

    let error = adapter.start_session(start_request()).await.unwrap_err();
    let diagnostic = error.to_string();
    assert!(diagnostic.contains("-32042"));
    assert!(diagnostic.contains("invented rejection"));
    assert!(!diagnostic.contains("must-not-leak"));
}

#[tokio::test]
async fn codex_permission_responses_never_broaden_the_requested_profile() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test","platformFamily":"unix","platformOs":"macos","codexHome":"/invented/codex-home"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-7","sessionId":"session-3"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-7","turn":{{"id":"turn-9","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-9","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"method":"item/permissions/requestApproval","id":"permission-6","params":{{"threadId":"thread-7","turnId":"turn-9","itemId":"permission-item-1","startedAtMs":1000,"cwd":"/invented/project","reason":"Allow one host","permissions":{{"network":{{"enabled":true}}}}}}}}\n'
IFS= read -r line
printf '%s' "$line" | grep -q '"id":"permission-6"'
printf '%s' "$line" | grep -q '"permissions":{{"network":{{"enabled":true}}}}'
if printf '%s' "$line" | grep -q 'danger'; then exit 71; fi
printf '{{"method":"item/permissions/requestApproval","id":"permission-7","params":{{"threadId":"thread-7","turnId":"turn-9","itemId":"permission-item-2","startedAtMs":1001,"cwd":"/invented/project","reason":"Allow one host again","permissions":{{"network":{{"enabled":true}}}}}}}}\n'
IFS= read -r line
printf '%s' "$line" | grep -q '"id":"permission-7"'
printf '%s' "$line" | grep -q '"permissions":{{}}'
if printf '%s' "$line" | grep -q 'enabled'; then exit 72; fi
printf '{{"method":"turn/completed","params":{{"threadId":"thread-7","turn":{{"id":"turn-9","items":[],"status":"completed"}}}}}}\n'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("request one permission"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::ApprovalRequested { request_id, operation, details, .. }
            if request_id == "string:permission-6"
                && operation == "permission request"
                && details == Some(ApprovalRequestDetails::PermissionProfile {
                    cwd: "/invented/project".to_owned(),
                    profile: RequestedPermissionProfile {
                        file_system: None,
                        network: Some(RequestedNetworkPermissions { enabled: Some(true) }),
                    },
                })
    ));

    adapter
        .respond(&session, "string:permission-6", ApprovalResponse::Approved)
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::ApprovalRequested { request_id, operation, .. }
            if request_id == "string:permission-7" && operation == "permission request"
    ));
    adapter
        .respond(&session, "string:permission-7", ApprovalResponse::Denied)
        .await
        .unwrap();
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnCompleted
    );
}

#[tokio::test]
async fn codex_file_approval_retains_paths_but_not_patch_contents() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-file","sessionId":"session-file"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-file","turn":{{"id":"turn-file","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-file","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"method":"item/started","params":{{"threadId":"thread-file","turnId":"turn-file","startedAtMs":1000,"item":{{"type":"fileChange","id":"file-item","status":"inProgress","changes":[{{"path":"/invented/project/old.txt","kind":{{"type":"update","move_path":"/invented/project/new.txt"}},"diff":"PRIVATE PATCH BODY"}}]}}}}}}\n'
printf '{{"method":"item/fileChange/requestApproval","id":"file-approval","params":{{"threadId":"thread-file","turnId":"turn-file","itemId":"file-item","startedAtMs":1001,"grantRoot":"/invented/project","reason":"Move the invented file"}}}}\n'
IFS= read -r line
printf '%s' "$line" | grep -q '"id":"file-approval"'
printf '{{"method":"turn/completed","params":{{"threadId":"thread-file","turn":{{"id":"turn-file","items":[],"status":"completed"}}}}}}\n'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("request a file move"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::NativeItemActivity { native_item_id, .. } if native_item_id == "file-item"
    ));
    let approval = turn.recv().await.unwrap().unwrap();
    assert!(matches!(
        approval,
        ProviderEvent::ApprovalRequested { ref details, .. }
            if details == &Some(ApprovalRequestDetails::FileChange {
                changes: vec![FileChangeApprovalDetail {
                    path: "/invented/project/old.txt".to_owned(),
                    change: FileChangeKind::Update {
                        move_path: Some("/invented/project/new.txt".to_owned()),
                    },
                }],
                grant_root: Some("/invented/project".to_owned()),
                reason: Some("Move the invented file".to_owned()),
            })
    ));
    assert!(!format!("{approval:?}").contains("PRIVATE PATCH BODY"));

    adapter
        .respond(&session, "string:file-approval", ApprovalResponse::Approved)
        .await
        .unwrap();
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnCompleted
    );
}

#[tokio::test]
async fn codex_rejects_a_stale_explicit_interrupt_without_mutating_the_active_turn() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-stale-interrupt","sessionId":"session-stale-interrupt"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-stale-interrupt","turn":{{"id":"turn-current","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-current","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
IFS= read -r line
printf '%s' "$line" | grep -q '"method":"turn/interrupt"'
printf '%s' "$line" | grep -q '"turnId":"turn-current"'
{extract_id}
printf '{{"id":%s,"result":{{}}}}\n' "$request_id"
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("keep the active turn intact"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { native_turn_id } if native_turn_id == "turn-current"
    ));

    assert!(adapter.interrupt(&session, "turn-stale").await.is_err());
    adapter.interrupt(&session, "turn-current").await.unwrap();
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::Interrupted
    );
}

#[tokio::test]
async fn provisional_terminal_seals_the_turn_against_later_activity_and_requests() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-sealed","sessionId":"session-sealed"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-sealed","turn":{{"id":"turn-sealed","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"method":"turn/completed","params":{{"threadId":"thread-sealed","turn":{{"id":"turn-sealed","items":[],"status":"completed"}}}}}}\n'
printf '{{"method":"item/agentMessage/delta","params":{{"threadId":"thread-sealed","turnId":"turn-sealed","itemId":"message-too-late","delta":"MUST NOT REPLAY"}}}}\n'
printf '{{"id":"approval-too-late","method":"item/fileChange/requestApproval","params":{{"threadId":"thread-sealed","turnId":"turn-sealed","itemId":"file-too-late","startedAtMs":1000,"grantRoot":"/invented/too-late"}}}}\n'
printf '{{"method":"turn/completed","params":{{"threadId":"thread-sealed","turn":{{"id":"turn-sealed","items":[],"status":"completed"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-sealed","items":[],"status":"completed"}}}}}}\n' "$request_id"
IFS= read -r line
printf '%s' "$line" | grep -q '"id":"approval-too-late"'
printf '%s' "$line" | grep -q '"error"'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("seal at first terminal"))
        .await
        .unwrap();

    let mut events = Vec::new();
    while let Some(event) = turn.recv().await {
        events.push(event.unwrap());
    }
    assert_eq!(
        events,
        vec![
            ProviderEvent::TurnStarted {
                native_turn_id: "turn-sealed".to_owned(),
            },
            ProviderEvent::TurnCompleted,
        ]
    );
}

#[tokio::test]
async fn terminal_event_uses_reserved_capacity_when_the_consumer_is_slow() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-full","sessionId":"session-full"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-full","turn":{{"id":"turn-full","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-full","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
i=0
while [ "$i" -lt 255 ]; do
  printf '{{"method":"item/agentMessage/delta","params":{{"threadId":"thread-full","turnId":"turn-full","itemId":"message-full","delta":"x"}}}}\n'
  i=$((i + 1))
done
printf '{{"method":"turn/completed","params":{{"threadId":"thread-full","turn":{{"id":"turn-full","items":[],"status":"completed"}}}}}}\n'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("fill the stream before reading"))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let mut terminal_count = 0;
    while let Some(event) = turn.recv().await {
        if event.unwrap().is_terminal() {
            terminal_count += 1;
        }
    }
    assert_eq!(terminal_count, 1);
}

#[tokio::test]
async fn interrupt_rejects_existing_and_new_requests_before_completing_the_turn() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-cancel-requests","sessionId":"session-cancel-requests"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-cancel-requests","turn":{{"id":"turn-cancel-requests","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-cancel-requests","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"id":"approval-before","method":"item/commandExecution/requestApproval","params":{{"threadId":"thread-cancel-requests","turnId":"turn-cancel-requests","itemId":"command-before","startedAtMs":1000,"command":"printf before","cwd":"/invented/project"}}}}\n'
IFS= read -r line
printf '%s' "$line" | grep -q '"method":"turn/interrupt"'
{extract_id}
printf '{{"id":"approval-after","method":"item/commandExecution/requestApproval","params":{{"threadId":"thread-cancel-requests","turnId":"turn-cancel-requests","itemId":"command-after","startedAtMs":1001,"command":"printf after","cwd":"/invented/project"}}}}\n'
IFS= read -r line
printf '%s' "$line" | grep -q '"id":"approval-after"'
printf '%s' "$line" | grep -q '"error"'
printf '{{"id":%s,"result":{{}}}}\n' "$request_id"
IFS= read -r line
printf '%s' "$line" | grep -q '"id":"approval-before"'
printf '%s' "$line" | grep -q '"error"'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("interrupt approvals"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::ApprovalRequested { request_id, .. } if request_id == "string:approval-before"
    ));

    adapter
        .interrupt(&session, "turn-cancel-requests")
        .await
        .unwrap();
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::Interrupted
    );
    assert!(turn.recv().await.is_none());
    assert!(
        adapter
            .respond(&session, "string:approval-before", ApprovalResponse::Denied,)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn concurrent_interrupt_callers_share_one_native_confirmation() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-coalesce","sessionId":"session-coalesce"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-coalesce","turn":{{"id":"turn-coalesce","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-coalesce","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
IFS= read -r line
printf '%s' "$line" | grep -q '"method":"turn/interrupt"'
{extract_id}
sleep 0.2
printf '{{"id":%s,"result":{{}}}}\n' "$request_id"
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("coalesce interruption"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));

    let (first, second) = tokio::join!(
        adapter.interrupt(&session, "turn-coalesce"),
        adapter.interrupt(&session, "turn-coalesce"),
    );
    assert_eq!(first, second);
    assert!(first.is_ok());
    assert!(matches!(
        adapter.health().await.unwrap(),
        ProviderHealth::Healthy { .. }
    ));
}

#[tokio::test]
async fn turn_completion_confirms_an_interrupt_before_its_late_rpc_response() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-complete-race","sessionId":"session-complete-race"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-complete-race","turn":{{"id":"turn-complete-race","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-complete-race","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
IFS= read -r line
printf '%s' "$line" | grep -q '"method":"turn/interrupt"'
{extract_id}
printf '{{"method":"turn/completed","params":{{"threadId":"thread-complete-race","turn":{{"id":"turn-complete-race","items":[],"status":"completed"}}}}}}\n'
sleep 0.05
printf '{{"id":%s,"result":{{}}}}\n' "$request_id"
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("completion interrupt race"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));

    adapter
        .interrupt(&session, "turn-complete-race")
        .await
        .unwrap();
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnCompleted
    );
    tokio::time::sleep(std::time::Duration::from_millis(75)).await;
    assert!(matches!(
        adapter.health().await.unwrap(),
        ProviderHealth::Healthy { .. }
    ));
}

#[tokio::test]
async fn dropping_a_direct_interrupt_after_write_still_confirms_the_turn() {
    let marker_directory = tempfile::tempdir().unwrap();
    let admitted = marker_directory.path().join("interrupt-admitted");
    let release = marker_directory.path().join("release-interrupt");
    let admitted_text = admitted.to_string_lossy();
    let release_text = release.to_string_lossy();
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-drop-interrupt","sessionId":"session-drop-interrupt"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-drop-interrupt","turn":{{"id":"turn-drop-interrupt","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-drop-interrupt","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
IFS= read -r line
printf '%s' "$line" | grep -q '"method":"turn/interrupt"'
{extract_id}
: > '{admitted_text}'
while [ ! -e '{release_text}' ]; do sleep 0.01; done
printf '{{"id":%s,"result":{{}}}}\n' "$request_id"
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("drop written interrupt"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));

    let interrupt_adapter = adapter.clone();
    let interrupt_session = session.clone();
    let interrupt = tokio::spawn(async move {
        interrupt_adapter
            .interrupt(&interrupt_session, "turn-drop-interrupt")
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !admitted.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    interrupt.abort();
    fs::write(release, b"release").unwrap();

    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::Interrupted
    );
    assert!(matches!(
        adapter.health().await.unwrap(),
        ProviderHealth::Healthy { .. }
    ));
}

#[tokio::test]
async fn a_direct_interrupt_retries_after_pending_capacity_is_released() {
    let marker_directory = tempfile::tempdir().unwrap();
    let full = marker_directory.path().join("pending-full");
    let full_text = full.to_string_lossy();
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-capacity-interrupt","sessionId":"session-capacity-interrupt"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-capacity-interrupt","turn":{{"id":"turn-capacity-interrupt","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-capacity-interrupt","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
i=0
while [ "$i" -lt 128 ]; do
  IFS= read -r line
  printf '%s' "$line" | grep -q '"method":"thread/start"'
  i=$((i + 1))
done
: > '{full_text}'
IFS= read -r line
printf '%s' "$line" | grep -q '"method":"turn/interrupt"'
{extract_id}
printf '{{"id":%s,"result":{{}}}}\n' "$request_id"
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("capacity then interrupt"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));

    let mut pending = Vec::new();
    for _ in 0..128 {
        let pending_adapter = adapter.clone();
        pending.push(tokio::spawn(async move {
            pending_adapter.start_session(start_request()).await
        }));
    }
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !full.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert!(matches!(
        adapter.interrupt(&session, "turn-capacity-interrupt").await,
        Err(ProviderError::NotDispatched { .. })
    ));
    pending.pop().unwrap().abort();
    let retry = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match adapter.interrupt(&session, "turn-capacity-interrupt").await {
                Ok(()) => break,
                Err(ProviderError::NotDispatched { .. }) => tokio::task::yield_now().await,
                Err(error) => panic!("unexpected retry error: {error:?}"),
            }
        }
    })
    .await;
    assert!(retry.is_ok(), "interrupt did not become retryable");
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::Interrupted
    );
    for task in pending {
        task.abort();
    }
}

#[tokio::test]
async fn dropping_a_response_ready_turn_start_interrupts_its_exact_generation() {
    let marker_directory = tempfile::tempdir().unwrap();
    let request_read = marker_directory.path().join("turn-request-read");
    let release = marker_directory.path().join("release-turn-response");
    let response_sent = marker_directory.path().join("turn-response-sent");
    let interrupted = marker_directory.path().join("turn-interrupted");
    let request_read_text = request_read.to_string_lossy();
    let release_text = release.to_string_lossy();
    let response_sent_text = response_sent.to_string_lossy();
    let interrupted_text = interrupted.to_string_lossy();
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-ready-drop","sessionId":"session-ready-drop"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
turn_request_id="$request_id"
: > '{request_read_text}'
while [ ! -e '{release_text}' ]; do sleep 0.01; done
printf '{{"method":"turn/started","params":{{"threadId":"thread-ready-drop","turn":{{"id":"turn-ready-drop","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-ready-drop","items":[],"status":"inProgress"}}}}}}\n' "$turn_request_id"
: > '{response_sent_text}'
IFS= read -r line
printf '%s' "$line" | grep -q '"method":"turn/interrupt"'
{extract_id}
printf '{{"id":%s,"result":{{}}}}\n' "$request_id"
: > '{interrupted_text}'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut pending_turn =
        Box::pin(adapter.start_turn(&session, TurnRequest::new("drop response-ready turn start")));
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !request_read.exists() {
            tokio::select! {
                result = &mut pending_turn => panic!("turn completed before gate: {result:?}"),
                _ = tokio::task::yield_now() => {}
            }
        }
    })
    .await
    .unwrap();
    fs::write(release, b"release").unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !response_sent.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    drop(pending_turn);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !interrupted.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        adapter.health().await.unwrap(),
        ProviderHealth::Healthy { .. }
    ));
}

#[tokio::test]
async fn malformed_recognized_approval_is_rejected_before_connection_teardown() {
    let marker_directory = tempfile::tempdir().unwrap();
    let marker = marker_directory.path().join("malformed-rejected");
    let marker_text = marker.to_string_lossy();
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-malformed","sessionId":"session-malformed"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-malformed","turn":{{"id":"turn-malformed","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-malformed","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"id":"approval-malformed","method":"item/commandExecution/requestApproval","params":{{"threadId":"thread-malformed","turnId":"turn-malformed","itemId":"command-malformed","startedAtMs":1000,"command":42,"cwd":"/invented/project"}}}}\n'
IFS= read -r line
printf '%s' "$line" | grep -q '"id":"approval-malformed"'
printf '%s' "$line" | grep -q '"code":-32602'
: > '{marker_text}'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("reject malformed approval"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    assert!(turn.recv().await.unwrap().is_err());
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !marker.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("malformed approval was not explicitly rejected");
}

#[tokio::test]
async fn unseen_file_change_item_cannot_produce_an_empty_approval() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-unseen-file","sessionId":"session-unseen-file"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-unseen-file","turn":{{"id":"turn-unseen-file","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-unseen-file","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"id":"file-unseen","method":"item/fileChange/requestApproval","params":{{"threadId":"thread-unseen-file","turnId":"turn-unseen-file","itemId":"never-seen","startedAtMs":1000,"grantRoot":"/invented/project"}}}}\n'
IFS= read -r line
printf '%s' "$line" | grep -q '"id":"file-unseen"'
printf '%s' "$line" | grep -q '"code":-32602'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("reject unseen file item"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    assert!(turn.recv().await.unwrap().is_err());
}

#[tokio::test]
async fn known_server_requests_with_invalid_ids_receive_one_null_id_error_each() {
    let extract_id = response_id_shell("request_id");
    let mut script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-invalid-ids","sessionId":"session-invalid-ids"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-invalid-ids","turn":{{"id":"turn-invalid-ids","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-invalid-ids","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
"#
    );
    for id_fragment in [
        "",
        ",\"id\":null",
        ",\"id\":{}",
        ",\"id\":1.5",
        ",\"id\":true",
        ",\"id\":[]",
        ",\"id\":9223372036854775808",
    ] {
        script.push_str(&format!(
            "printf '%s\\n' '{{\"method\":\"item/commandExecution/requestApproval\"{id_fragment},\"params\":{{\"threadId\":\"thread-invalid-ids\",\"turnId\":\"turn-invalid-ids\",\"itemId\":\"command-invalid\",\"startedAtMs\":1}}}}'\n"
        ));
        script.push_str("IFS= read -r line\n");
        script.push_str("printf '%s' \"$line\" | grep -q '\"id\":null'\n");
        script.push_str("printf '%s' \"$line\" | grep -q '\"code\":-32600'\n");
    }
    script.push_str("printf '%s\\n' '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"thread-invalid-ids\",\"turn\":{\"id\":\"turn-invalid-ids\",\"items\":[],\"status\":\"completed\"}}}'\n");
    script.push_str("sleep 30\n");

    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("reject invalid ids"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), turn.recv())
            .await
            .expect("invalid server request did not receive a response")
            .unwrap()
            .unwrap(),
        ProviderEvent::TurnCompleted
    );
}

#[tokio::test]
async fn permission_approval_rejects_unknown_nested_fields() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-hidden-permission","sessionId":"session-hidden-permission"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-hidden-permission","turn":{{"id":"turn-hidden-permission","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-hidden-permission","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"id":"permission-hidden","method":"item/permissions/requestApproval","params":{{"threadId":"thread-hidden-permission","turnId":"turn-hidden-permission","itemId":"permission-hidden","startedAtMs":1,"cwd":"/invented/project","permissions":{{"network":{{"enabled":true,"hidden":true}}}}}}}}\n'
IFS= read -r line
printf '%s' "$line" | grep -q '"id":"permission-hidden"'
printf '%s' "$line" | grep -q '"code":-32602'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("reject hidden permission"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    assert!(turn.recv().await.unwrap().is_err());
}

#[tokio::test]
async fn approval_requests_missing_schema_required_fields_are_rejected_before_ui_admission() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-required-fields","sessionId":"session-required-fields"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-required-fields","turn":{{"id":"turn-required-fields","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-required-fields","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"method":"item/started","params":{{"threadId":"thread-required-fields","turnId":"turn-required-fields","startedAtMs":1,"item":{{"type":"fileChange","id":"file-required","status":"inProgress","changes":[{{"path":"/invented/project/file.txt","kind":{{"type":"add"}},"diff":""}}]}}}}}}\n'
check_rejection() {{
  IFS= read -r line
  printf '%s' "$line" | grep -q "\"id\":\"$1\""
  printf '%s' "$line" | grep -q '"code":-32602'
}}
printf '{{"id":"missing-item","method":"item/commandExecution/requestApproval","params":{{"threadId":"thread-required-fields","turnId":"turn-required-fields","startedAtMs":1}}}}\n'
check_rejection missing-item
printf '{{"id":"missing-started","method":"item/commandExecution/requestApproval","params":{{"threadId":"thread-required-fields","turnId":"turn-required-fields","itemId":"command-required"}}}}\n'
check_rejection missing-started
printf '{{"id":"float-started","method":"item/commandExecution/requestApproval","params":{{"threadId":"thread-required-fields","turnId":"turn-required-fields","itemId":"command-required","startedAtMs":1.5}}}}\n'
check_rejection float-started
printf '{{"id":"range-started","method":"item/commandExecution/requestApproval","params":{{"threadId":"thread-required-fields","turnId":"turn-required-fields","itemId":"command-required","startedAtMs":9223372036854775808}}}}\n'
check_rejection range-started
printf '{{"id":"file-missing-started","method":"item/fileChange/requestApproval","params":{{"threadId":"thread-required-fields","turnId":"turn-required-fields","itemId":"file-required"}}}}\n'
check_rejection file-missing-started
printf '{{"id":"input-missing-item","method":"item/tool/requestUserInput","params":{{"threadId":"thread-required-fields","turnId":"turn-required-fields","questions":[{{"id":"question","header":"Choice","question":"Choose?"}}]}}}}\n'
check_rejection input-missing-item
printf '{{"id":"permission-missing-item","method":"item/permissions/requestApproval","params":{{"threadId":"thread-required-fields","turnId":"turn-required-fields","startedAtMs":1,"cwd":"/invented/project","permissions":{{}}}}}}\n'
check_rejection permission-missing-item
printf '{{"id":"permission-missing-started","method":"item/permissions/requestApproval","params":{{"threadId":"thread-required-fields","turnId":"turn-required-fields","itemId":"permission-required","cwd":"/invented/project","permissions":{{}}}}}}\n'
check_rejection permission-missing-started
printf '{{"id":"permission-zero-depth","method":"item/permissions/requestApproval","params":{{"threadId":"thread-required-fields","turnId":"turn-required-fields","itemId":"permission-required","startedAtMs":1,"cwd":"/invented/project","permissions":{{"fileSystem":{{"globScanMaxDepth":0}}}}}}}}\n'
check_rejection permission-zero-depth
printf '{{"method":"turn/completed","params":{{"threadId":"thread-required-fields","turn":{{"id":"turn-required-fields","items":[],"status":"completed"}}}}}}\n'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(
            &session,
            TurnRequest::new("validate required approval fields"),
        )
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::NativeItemActivity { .. }
    ));
    for _ in 0..9 {
        let event = turn.recv().await.unwrap();
        assert!(event.is_err(), "invalid request reached the UI: {event:?}");
    }
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnCompleted
    );
}

#[tokio::test]
async fn collab_activity_rejects_mismatched_parent_and_unlisted_agent_state() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-collab-owner","sessionId":"session-collab-owner"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-collab-owner","turn":{{"id":"turn-collab-owner","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-collab-owner","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"method":"item/started","params":{{"threadId":"thread-collab-owner","turnId":"turn-collab-owner","startedAtMs":1,"item":{{"type":"collabAgentToolCall","id":"collab-owner","tool":"spawnAgent","status":"inProgress","senderThreadId":"other-thread","receiverThreadIds":["child-listed"],"agentsStates":{{"child-unlisted":{{"status":"running"}}}}}}}}}}\n'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("validate collab owner"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    assert!(turn.recv().await.unwrap().is_err());
}

#[tokio::test]
async fn duplicate_pending_server_request_id_is_answered_once_and_is_connection_fatal() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-duplicate-pending","sessionId":"session-duplicate-pending"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-duplicate-pending","turn":{{"id":"turn-duplicate-pending","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-duplicate-pending","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"id":"duplicate-pending","method":"item/commandExecution/requestApproval","params":{{"threadId":"thread-duplicate-pending","turnId":"turn-duplicate-pending","itemId":"command-1","startedAtMs":1}}}}\n'
printf '{{"id":"duplicate-pending","method":"item/commandExecution/requestApproval","params":{{"threadId":"thread-duplicate-pending","turnId":"turn-duplicate-pending","itemId":"command-2","startedAtMs":2}}}}\n'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("duplicate pending"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::ApprovalRequested { .. }
    ));
    assert!(turn.recv().await.unwrap().is_err());
    wait_for_unavailable(&adapter).await;
    assert!(
        adapter
            .respond(
                &session,
                "string:duplicate-pending",
                ApprovalResponse::Denied,
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn duplicate_server_request_after_success_closes_without_a_second_response() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-duplicate-done","sessionId":"session-duplicate-done"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-duplicate-done","turn":{{"id":"turn-duplicate-done","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-duplicate-done","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"id":"duplicate-done","method":"item/commandExecution/requestApproval","params":{{"threadId":"thread-duplicate-done","turnId":"turn-duplicate-done","itemId":"command-1","startedAtMs":1}}}}\n'
IFS= read -r line
printf '%s' "$line" | grep -q '"id":"duplicate-done"'
printf '%s' "$line" | grep -q '"result"'
printf '{{"id":"duplicate-done","method":"item/commandExecution/requestApproval","params":{{"threadId":"thread-duplicate-done","turnId":"turn-duplicate-done","itemId":"command-2","startedAtMs":2}}}}\n'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("duplicate completed"))
        .await
        .unwrap();
    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    let request_id = match turn.recv().await.unwrap().unwrap() {
        ProviderEvent::ApprovalRequested { request_id, .. } => request_id,
        event => panic!("expected approval, got {event:?}"),
    };
    adapter
        .respond(&session, &request_id, ApprovalResponse::Approved)
        .await
        .unwrap();
    assert!(turn.recv().await.unwrap().is_err());
    wait_for_unavailable(&adapter).await;
}

#[tokio::test]
async fn full_provisional_buffer_rejects_a_request_without_retaining_it() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-provisional-full","sessionId":"session-provisional-full"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
turn_request_id="$request_id"
printf '{{"method":"turn/started","params":{{"threadId":"thread-provisional-full","turn":{{"id":"turn-provisional-full","items":[],"status":"inProgress"}}}}}}\n'
i=0
while [ "$i" -lt 128 ]; do
  printf '{{"method":"item/agentMessage/delta","params":{{"threadId":"thread-provisional-full","turnId":"turn-provisional-full","itemId":"message-full","delta":"x"}}}}\n'
  i=$((i + 1))
done
printf '{{"id":"approval-full","method":"item/commandExecution/requestApproval","params":{{"threadId":"thread-provisional-full","turnId":"turn-provisional-full","itemId":"command-full","startedAtMs":1}}}}\n'
IFS= read -r line
printf '%s' "$line" | grep -q '"id":"approval-full"'
printf '%s' "$line" | grep -q '"code":-32001'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-provisional-full","items":[],"status":"inProgress"}}}}}}\n' "$turn_request_id"
printf '{{"method":"turn/completed","params":{{"threadId":"thread-provisional-full","turn":{{"id":"turn-provisional-full","items":[],"status":"completed"}}}}}}\n'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("fill provisional buffer"))
        .await
        .unwrap();
    let mut deltas = 0;
    while let Some(event) = turn.recv().await {
        match event.unwrap() {
            ProviderEvent::AssistantMessageDelta { .. } => deltas += 1,
            ProviderEvent::ApprovalRequested { .. } => panic!("overflow request was retained"),
            ProviderEvent::TurnCompleted => break,
            _ => {}
        }
    }
    assert_eq!(deltas, 128);
    assert!(
        adapter
            .respond(&session, "string:approval-full", ApprovalResponse::Denied)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn duplicate_codex_response_id_does_not_poison_the_active_turn() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test","platformFamily":"unix","platformOs":"macos","codexHome":"/invented/codex-home"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-7","sessionId":"session-3"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-7","turn":{{"id":"turn-9","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-9","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-9","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"method":"item/agentMessage/delta","params":{{"threadId":"thread-7","turnId":"turn-9","itemId":"message-9","delta":"READY"}}}}\n'
printf '{{"method":"turn/completed","params":{{"threadId":"thread-7","turn":{{"id":"turn-9","items":[],"status":"completed"}}}}}}\n'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("duplicate response"))
        .await
        .unwrap();

    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::AssistantMessageDelta {
            native_item_id: "message-9".to_owned(),
            content: "READY".to_owned(),
        }
    );
    assert_eq!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnCompleted
    );
    turn.shutdown().await.unwrap();
}

#[tokio::test]
async fn unsupported_codex_server_requests_fail_explicitly() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test","platformFamily":"unix","platformOs":"macos","codexHome":"/invented/codex-home"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-7","sessionId":"session-3"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-7","turn":{{"id":"turn-9","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-9","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
printf '{{"method":"future/requestApproval","id":"unknown-request-1","params":{{"threadId":"thread-7","turnId":"turn-9"}}}}\n'
IFS= read -r line
printf '%s' "$line" | grep -q '"id":"unknown-request-1"'
printf '%s' "$line" | grep -q '"code":-32601'
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("send an unsupported request"))
        .await
        .unwrap();

    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    assert!(matches!(
        turn.recv().await.unwrap(),
        Err(ProviderError::Protocol { category }) if category == "unsupported-server-request"
    ));
}

#[tokio::test]
async fn codex_server_requests_without_an_active_owner_are_rejected_explicitly() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test","platformFamily":"unix","platformOs":"macos","codexHome":"/invented/codex-home"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"method":"item/commandExecution/requestApproval","id":"missing-owner","params":{{"itemId":"command-missing"}}}}\n'
IFS= read -r line
printf '%s' "$line" | grep -q '"id":"missing-owner"'
printf '%s' "$line" | grep -q '"code":-32602'
printf '{{"method":"item/fileChange/requestApproval","id":"inactive-owner","params":{{"threadId":"thread-inactive","turnId":"turn-inactive","itemId":"file-inactive"}}}}\n'
IFS= read -r line
printf '%s' "$line" | grep -q '"id":"inactive-owner"'
printf '%s' "$line" | grep -q '"code":-32003'
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-after-rejection","sessionId":"session-after-rejection"}}}}}}\n' "$request_id"
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    assert_eq!(session.native_id, "thread-after-rejection");
}

#[tokio::test]
async fn codex_process_closure_ends_each_active_turn_with_a_typed_error() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test","platformFamily":"unix","platformOs":"macos","codexHome":"/invented/codex-home"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-7","sessionId":"session-3"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-7","turn":{{"id":"turn-9","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-9","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
exit 0
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter.start_session(start_request()).await.unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("observe process closure"))
        .await
        .unwrap();

    assert!(matches!(
        turn.recv().await.unwrap().unwrap(),
        ProviderEvent::TurnStarted { .. }
    ));
    assert_eq!(turn.recv().await.unwrap(), Err(ProviderError::StreamClosed));
    assert!(turn.recv().await.is_none());
}

#[tokio::test]
async fn an_unread_turn_does_not_block_other_codex_sessions() {
    let extract_id = response_id_shell("request_id");
    let script = format!(
        r#"
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"userAgent":"codex-cli 0.test","platformFamily":"unix","platformOs":"macos","codexHome":"/invented/codex-home"}}}}\n' "$request_id"
IFS= read -r line
IFS= read -r line
{extract_id}
printf '{{"id":%s,"result":{{"thread":{{"id":"thread-busy","sessionId":"session-busy"}}}}}}\n' "$request_id"
IFS= read -r line
{extract_id}
printf '{{"method":"turn/started","params":{{"threadId":"thread-busy","turn":{{"id":"turn-busy","items":[],"status":"inProgress"}}}}}}\n'
printf '{{"id":%s,"result":{{"turn":{{"id":"turn-busy","items":[],"status":"inProgress"}}}}}}\n' "$request_id"
i=0
while [ "$i" -lt 300 ]; do
  printf '{{"method":"item/agentMessage/delta","params":{{"threadId":"thread-busy","turnId":"turn-busy","itemId":"message-busy","delta":"x"}}}}\n'
  i=$((i + 1))
done
while IFS= read -r line; do
  if printf '%s' "$line" | grep -q '"method":"thread/start"'; then
    {extract_id}
    printf '{{"id":%s,"result":{{"thread":{{"id":"thread-responsive","sessionId":"session-responsive"}}}}}}\n' "$request_id"
    break
  fi
done
sleep 30
"#
    );
    let (_directory, binary) = fake_codex(&script);
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let first_session = adapter.start_session(start_request()).await.unwrap();
    let _unread_turn = adapter
        .start_turn(&first_session, TurnRequest::new("fill the bounded stream"))
        .await
        .unwrap();

    let second_session = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        adapter.start_session(start_request()),
    )
    .await
    .expect("one unread turn blocked the shared Codex dispatcher")
    .unwrap();

    assert_eq!(second_session.native_id, "thread-responsive");
}

#[test]
fn unknown_notification_fixture_is_explicitly_separate_from_the_typed_recording() {
    let envelope: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/codex/unknown_notification.jsonl")).unwrap();
    assert_eq!(envelope["message"]["method"], "future/inventedNotification");
    assert!(!include_str!("fixtures/codex/session.jsonl").contains("future/inventedNotification"));
}

#[test]
#[ignore = "generates schemas from the installed Codex CLI 0.144.1"]
fn codex_fixture_matches_the_generated_0_144_1_protocol_schema() {
    if std::env::var("PROMPTING_TIME_CODEX_SCHEMA").as_deref() != Ok("1") {
        return;
    }
    let version = std::process::Command::new("codex")
        .arg("--version")
        .output()
        .expect("installed Codex CLI must be executable");
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        "codex-cli 0.144.1"
    );

    let schemas = tempfile::tempdir().unwrap();
    let generated = std::process::Command::new("codex")
        .args(["app-server", "generate-json-schema", "--out"])
        .arg(schemas.path())
        .output()
        .expect("Codex schema generation must run");
    assert!(
        generated.status.success(),
        "schema generation failed: {}",
        String::from_utf8_lossy(&generated.stderr)
    );

    fn validator(schema_root: &std::path::Path, relative: &str) -> jsonschema::Validator {
        let schema: serde_json::Value =
            serde_json::from_slice(&fs::read(schema_root.join(relative)).unwrap()).unwrap();
        jsonschema::draft7::new(&schema)
            .unwrap_or_else(|error| panic!("failed to compile {relative}: {error}"))
    }

    fn assert_valid(
        schema_root: &std::path::Path,
        relative: &str,
        value: &serde_json::Value,
        line: usize,
    ) {
        if let Err(error) = validator(schema_root, relative).validate(value) {
            panic!("fixture line {line} failed {relative}: {error}");
        }
    }

    let mut requests = BTreeMap::<String, String>::new();
    for (index, line) in include_str!("fixtures/codex/session.jsonl")
        .lines()
        .enumerate()
    {
        let line_number = index + 1;
        let envelope: serde_json::Value = serde_json::from_str(line).unwrap();
        let direction = envelope["direction"].as_str().unwrap();
        let message = &envelope["message"];
        assert_valid(schemas.path(), "JSONRPCMessage.json", message, line_number);
        match (message.get("method"), message.get("id")) {
            (Some(method), Some(id)) => {
                let typed = if direction == "client" {
                    "ClientRequest.json"
                } else {
                    "ServerRequest.json"
                };
                assert_valid(schemas.path(), typed, message, line_number);
                requests.insert(id.to_string(), method.as_str().unwrap().to_owned());
            }
            (Some(_), None) => {
                let typed = if direction == "client" {
                    "ClientNotification.json"
                } else {
                    "ServerNotification.json"
                };
                assert_valid(schemas.path(), typed, message, line_number);
            }
            (None, Some(id)) => {
                assert_valid(schemas.path(), "JSONRPCResponse.json", message, line_number);
                let method = requests
                    .remove(&id.to_string())
                    .unwrap_or_else(|| panic!("fixture line {line_number} has no request"));
                let response_schema = match method.as_str() {
                    "initialize" => "v1/InitializeResponse.json",
                    "thread/start" => "v2/ThreadStartResponse.json",
                    "turn/start" => "v2/TurnStartResponse.json",
                    "item/commandExecution/requestApproval" => {
                        "CommandExecutionRequestApprovalResponse.json"
                    }
                    "item/fileChange/requestApproval" => "FileChangeRequestApprovalResponse.json",
                    "item/permissions/requestApproval" => "PermissionsRequestApprovalResponse.json",
                    "item/tool/requestUserInput" => "ToolRequestUserInputResponse.json",
                    other => panic!("fixture response has unmapped method {other}"),
                };
                assert_valid(
                    schemas.path(),
                    response_schema,
                    &message["result"],
                    line_number,
                );
            }
            (None, None) => panic!("fixture line {line_number} is not JSON-RPC"),
        }
    }
    assert!(
        requests.is_empty(),
        "fixture has requests without responses"
    );

    let unknown: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/codex/unknown_notification.jsonl")).unwrap();
    assert_valid(
        schemas.path(),
        "JSONRPCMessage.json",
        &unknown["message"],
        1,
    );
    assert!(
        validator(schemas.path(), "ServerNotification.json")
            .validate(&unknown["message"])
            .is_err()
    );
}

#[tokio::test]
#[ignore = "uses the installed Codex CLI and authenticated account"]
async fn live_codex_smoke_uses_an_empty_temporary_git_repository() {
    if std::env::var("PROMPTING_TIME_LIVE_CODEX").as_deref() != Ok("1") {
        return;
    }
    let repository = tempfile::tempdir().unwrap();
    let status = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(repository.path())
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(fs::read_dir(repository.path()).unwrap().count(), 1);

    let adapter = CodexAdapter::connect(PathBuf::from("codex")).await.unwrap();
    let session = adapter
        .start_session(StartSession {
            conversation_id: ConversationId::new(),
            working_directory: repository.path().to_owned(),
        })
        .await
        .unwrap();
    let mut turn = adapter
        .start_turn(
            &session,
            TurnRequest::new("Reply READY without using tools."),
        )
        .await
        .unwrap();
    let mut assistant = String::new();
    let mut completed = false;
    tokio::time::timeout(std::time::Duration::from_secs(120), async {
        while let Some(event) = turn.recv().await {
            match event.unwrap() {
                ProviderEvent::AssistantMessage { content }
                | ProviderEvent::AssistantMessageDelta { content, .. } => {
                    assistant.push_str(&content);
                }
                ProviderEvent::TurnCompleted => {
                    completed = true;
                    break;
                }
                ProviderEvent::ApprovalRequested { request_id, .. } => {
                    adapter
                        .respond(&session, &request_id, ApprovalResponse::Denied)
                        .await
                        .unwrap();
                }
                _ => {}
            }
        }
    })
    .await
    .expect("live Codex turn timed out");
    turn.shutdown().await.unwrap();
    adapter.archive_session(&session).await.unwrap();

    assert!(assistant.contains("READY"));
    assert!(completed);
}
