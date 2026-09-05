use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use prompting_time_core::domain::{ConversationId, MutationState};
use prompting_time_core::providers::claude::ClaudeAdapter;
use prompting_time_core::providers::{
    ApprovalResponse, NativeAgentStatus, ProviderAdapter, ProviderCapability, ProviderError,
    ProviderEvent, ProviderHealth, ProviderSession, ProviderTurn, ResumeSession, StartSession,
    TurnRequest,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::time::timeout;

// Protocol fixtures contain only invented data. The executable exercises the real owned transport.
struct Fixture {
    directory: TempDir,
    adapter: ClaudeAdapter,
    conversation: ConversationId,
}

impl Fixture {
    fn new(body: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("claude-fixture");
        fs::write(&binary, format!("{PRELUDE}\n{body}\n")).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            directory,
            adapter: ClaudeAdapter::new(binary),
            conversation: ConversationId::new(),
        }
    }

    async fn session(&self) -> ProviderSession {
        self.adapter
            .start_session(StartSession {
                conversation_id: self.conversation,
                working_directory: self.directory.path().to_path_buf(),
            })
            .await
            .unwrap()
    }

    async fn resume(&self, session: &ProviderSession) -> ProviderSession {
        self.adapter
            .resume_session(
                &session.native_id,
                ResumeSession {
                    conversation_id: self.conversation,
                    working_directory: self.directory.path().to_path_buf(),
                },
            )
            .await
            .unwrap()
    }

    fn read(&self, name: &str) -> Value {
        serde_json::from_slice(&fs::read(self.directory.path().join(name)).unwrap()).unwrap()
    }
}

const PRELUDE: &str = r#"#!/usr/bin/env python3
import json, os, pathlib, sys, time
root = pathlib.Path(__file__).parent
def emit(value):
    print(json.dumps(value), flush=True)
def receive():
    return json.loads(sys.stdin.readline())
def record(name, value):
    (root / name).write_text(json.dumps(value))
def barrier(name):
    record(name + '.ready', True)
    while not (root / (name + '.release')).exists():
        time.sleep(0.005)
def result(**extra):
    emit(dict(type='result', session_id=session, subtype='success', is_error=False, **extra))
def assistant(content, parent=None, mid='message-1'):
    emit(dict(type='assistant', session_id=session, parent_tool_use_id=parent,
              message=dict(id=mid, content=content)))
def task(kind, tid, tool, **extra):
    emit(dict(type='system', subtype=kind, session_id=session, task_id=tid, tool_use_id=tool, **extra))
def permission(rid='request-1', tool='Write', data=None):
    emit(dict(type='control_request', request_id=rid, request=dict(subtype='can_use_tool',
         tool_name=tool, tool_use_id='tool-' + rid, input=data or {'file_path':'invented.txt','content':'INVENTED'})))
if '--version' in sys.argv:
    print((root / 'version').read_text() if (root / 'version').exists() else '2.1.205 (Claude Code)')
    sys.exit(1 if (root / 'inspection-failure').exists() else 0)
if 'auth' in sys.argv:
    emit(json.loads((root / 'auth').read_text()) if (root / 'auth').exists() else {'loggedIn':True,'account':'MUST-NOT-RETAIN'})
    sys.exit(0)
session = next(arg.split('=',1)[1] for arg in sys.argv if arg.startswith(('--session-id=', '--resume=')))
record('args', sys.argv[1:])
record('pid', os.getpid())
if (root / 'missing-resume').exists() and any(arg.startswith('--resume=') for arg in sys.argv):
    print('No conversation found with session ID: invented', file=sys.stderr, flush=True)
    sys.exit(1)
initialize = receive()
record('initialize', initialize)
if (root / 'hold-initialize').exists():
    barrier('initialize')
emit({'type':'control_response','response':{'subtype':'success','request_id':initialize['request_id'],'response':{}}})
if (root / 'hold-prompt-read').exists():
    sys.stdin.read(1)
    barrier('prompt-read')
prompt = receive()
record('prompt', prompt)
"#;

async fn event(turn: &mut ProviderTurn) -> Result<ProviderEvent, ProviderError> {
    timeout(Duration::from_secs(5), turn.recv())
        .await
        .expect("bounded event wait")
        .expect("event")
}

async fn collect(turn: &mut ProviderTurn) -> Vec<Result<ProviderEvent, ProviderError>> {
    let mut events = Vec::new();
    while let Some(value) = timeout(Duration::from_secs(5), turn.recv()).await.unwrap() {
        events.push(value);
    }
    turn.shutdown().await.unwrap();
    events
}

async fn wait_file(path: &Path) {
    timeout(Duration::from_secs(5), async {
        while !path.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

async fn reaped(pid: u64) {
    timeout(Duration::from_secs(2), async {
        loop {
            let running = std::process::Command::new("/bin/kill")
                .args(["-0", &pid.to_string()])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success();
            if !running {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("owned process must be reaped");
}

#[tokio::test]
async fn fresh_then_resume_uses_exact_flags_and_preserves_context_boundary() {
    let fixture = Fixture::new(
        r#"
if any(arg.startswith('--resume=') for arg in sys.argv):
    assistant([{'type':'text','text':(root / 'context').read_text()}])
else:
    (root / 'context').write_text(prompt['message']['content'])
    assistant([{'type':'text','text':'FIRST'}])
result()
barrier('terminal')
"#,
    );
    let session = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(&session, TurnRequest::new("INVENTED CONTEXT"))
        .await
        .unwrap();
    let events = collect(&mut turn).await;
    assert!(matches!(
        events.first(),
        Some(Ok(ProviderEvent::TurnStarted { .. }))
    ));
    assert!(matches!(
        events.last(),
        Some(Ok(ProviderEvent::TurnCompleted))
    ));
    let args = fixture.read("args");
    let args = args.as_array().unwrap();
    assert!(args.contains(&json!(format!("--session-id={}", session.native_id))));
    for pair in [
        ["--permission-mode", "default"],
        ["--permission-prompt-tool", "stdio"],
        ["--input-format", "stream-json"],
    ] {
        assert!(
            args.windows(2)
                .any(|args| args == [json!(pair[0]), json!(pair[1])])
        );
    }
    for required in [
        "--setting-sources=",
        "--strict-mcp-config",
        "--no-chrome",
        "--include-partial-messages",
    ] {
        assert!(args.contains(&json!(required)));
    }
    assert!(
        !args
            .iter()
            .any(|arg| arg.as_str().unwrap().contains("bypass") || arg == "--max-budget-usd")
    );
    assert_eq!(
        fixture.read("initialize")["request"]["forwardSubagentText"],
        true
    );
    let resumed = fixture.resume(&session).await;
    let mut turn = fixture
        .adapter
        .start_turn(&resumed, TurnRequest::new("Recall"))
        .await
        .unwrap();
    let events = collect(&mut turn).await;
    assert!(events.iter().any(|event| matches!(event, Ok(ProviderEvent::AssistantMessageDelta {content, ..}) if content == "INVENTED CONTEXT")));
    assert!(
        fixture
            .read("args")
            .as_array()
            .unwrap()
            .contains(&json!(format!("--resume={}", session.native_id)))
    );
}

#[tokio::test]
async fn health_checks_version_and_only_authenticated_availability() {
    let fixture = Fixture::new("result()");
    assert_eq!(
        fixture.adapter.health().await.unwrap(),
        ProviderHealth::Healthy {
            version: "2.1.205".into()
        }
    );
    fs::write(
        fixture.directory.path().join("auth"),
        r#"{"loggedIn":false,"account":"PRIVATE"}"#,
    )
    .unwrap();
    assert!(
        matches!(fixture.adapter.health().await.unwrap(), ProviderHealth::Unavailable {category} if category.contains("login") && !category.contains("PRIVATE"))
    );
    for version in ["2.1.204", "3.0.0", "invalid"] {
        fs::write(fixture.directory.path().join("version"), version).unwrap();
        assert!(matches!(
            fixture.adapter.health().await.unwrap(),
            ProviderHealth::Unavailable { .. }
        ));
    }
    assert!(
        !fixture
            .adapter
            .capabilities()
            .supports(ProviderCapability::Steering)
    );
}

#[tokio::test]
async fn failed_version_inspection_cannot_report_healthy() {
    let fixture = Fixture::new("result()");
    fs::write(fixture.directory.path().join("inspection-failure"), "").unwrap();
    assert!(matches!(
        fixture.adapter.health().await.unwrap(),
        ProviderHealth::Unavailable { .. }
    ));
}

#[tokio::test]
async fn adapter_restart_with_missing_native_session_fails_closed_without_fresh_replay() {
    let fixture = Fixture::new("result()");
    let session = fixture.session().await;
    fs::write(fixture.directory.path().join("missing-resume"), "").unwrap();
    let adapter = ClaudeAdapter::new(fixture.directory.path().join("claude-fixture"));
    let resumed = adapter
        .resume_session(
            &session.native_id,
            ResumeSession {
                conversation_id: fixture.conversation,
                working_directory: fixture.directory.path().into(),
            },
        )
        .await
        .unwrap();
    let error = adapter
        .start_turn(&resumed, TurnRequest::new("followup"))
        .await
        .unwrap_err();
    assert!(
        matches!(error, ProviderError::Protocol { category } if category.contains("native-session-missing-start-new-conversation"))
    );
    assert!(!fixture.directory.path().join("prompt").exists());
    assert!(
        fixture
            .read("args")
            .as_array()
            .unwrap()
            .contains(&json!(format!("--resume={}", session.native_id)))
    );
}

#[tokio::test]
async fn malformed_questions_and_conflicting_native_identity_fail_closed() {
    for body in [
        "permission(tool='AskUserQuestion',data={'questions':[{'question':'Same','multiSelect':False},{'question':'Same','multiSelect':False}]})",
        "permission(tool='AskUserQuestion',data={'questions':[{'question':'Choice','multiSelect':False,'options':[{'label':'A'},{'label':'A'}]}]})",
        "permission(tool='AskUserQuestion',data={'questions':[{'question':'Choice','multiSelect':'yes'}]})",
        "assistant([{'type':'tool_use','id':'a','name':'Agent','input':{}}]); task('task_started','task-a','a'); task('task_started','task-a','different')",
        "assistant([{'type':'tool_use','id':'a','name':'Agent','input':{}}]); assistant([{'type':'tool_use','id':'a','name':'Agent','input':{}}],parent='different')",
        "permission(); permission()",
        "sys.stdout.write('x'* (9*1024*1024) + '\\n'); sys.stdout.flush()",
    ] {
        let fixture = Fixture::new(body);
        let session = fixture.session().await;
        let mut turn = fixture
            .adapter
            .start_turn(&session, TurnRequest::new("invalid"))
            .await
            .unwrap();
        let events = collect(&mut turn).await;
        assert!(events.iter().any(Result::is_err), "{body}: {events:?}");
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Ok(ProviderEvent::TurnCompleted)))
        );
    }
}

#[tokio::test]
async fn rate_limit_events_are_advisory_until_terminal_result() {
    for status in ["allowed", "allowed_warning", "rejected"] {
        let fixture = Fixture::new(&format!(
            "emit(dict(type='rate_limit_event',session_id=session,uuid='quota-1',rate_limit_info={{'status':'{status}','overageStatus':'rejected'}}))\nassistant([{{'type':'text','text':'READY'}}])\nbarrier('before-result')\nresult()"
        ));
        let session = fixture.session().await;
        let mut turn = fixture
            .adapter
            .start_turn(&session, TurnRequest::new("invented"))
            .await
            .unwrap();
        assert!(matches!(
            event(&mut turn).await.unwrap(),
            ProviderEvent::TurnStarted { .. }
        ));
        assert!(
            matches!(event(&mut turn).await.unwrap(), ProviderEvent::AssistantMessageDelta { content, .. } if content == "READY")
        );
        wait_file(&fixture.directory.path().join("before-result.ready")).await;
        fs::write(fixture.directory.path().join("before-result.release"), "").unwrap();
        let events = collect(&mut turn).await;
        assert!(matches!(
            events.as_slice(),
            [Ok(ProviderEvent::TurnCompleted)]
        ));
        reaped(fixture.read("pid").as_u64().unwrap()).await;
    }
}

#[tokio::test]
async fn rate_limit_events_do_not_hide_failure_or_invalid_envelopes() {
    for body in [
        "emit(dict(type='rate_limit_event',session_id=session,uuid='quota-1',rate_limit_info={'status':'rejected'}))",
        "emit(dict(type='rate_limit_event',session_id=session,uuid='quota-1',rate_limit_info={'status':'rejected'})); emit(dict(type='result',session_id=session,subtype='error_during_execution',is_error=True))",
        "emit(dict(type='rate_limit_event',session_id='wrong-session',uuid='quota-1',rate_limit_info={'status':'allowed'})); result()",
        "emit(dict(type='rate_limit_event',session_id=session,uuid='quota-1',rate_limit_info={'status':'unknown'})); result()",
        "emit(dict(type='rate_limit_event',session_id=session,uuid='quota-1')); result()",
        "emit(dict(type='rate_limit_event',uuid='quota-1',rate_limit_info={'status':'allowed'})); result()",
        "emit(dict(type='rate_limit_event',session_id=session,rate_limit_info={'status':'allowed'})); result()",
        "emit(dict(type='unknown_event',session_id=session)); result()",
    ] {
        let fixture = Fixture::new(body);
        let session = fixture.session().await;
        let mut turn = fixture
            .adapter
            .start_turn(&session, TurnRequest::new("invented"))
            .await
            .unwrap();
        let events = collect(&mut turn).await;
        assert!(events.iter().any(Result::is_err), "{events:?}");
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Ok(ProviderEvent::TurnCompleted))),
            "{events:?}"
        );
        reaped(fixture.read("pid").as_u64().unwrap()).await;
    }
}

#[tokio::test]
async fn thinking_token_estimates_do_not_become_text_or_terminal_evidence() {
    for terminal in [false, true] {
        let fixture = Fixture::new(&format!(
            "emit(dict(type='system',subtype='thinking_tokens',session_id=session,uuid='estimate-1',estimated_tokens=12,estimated_tokens_delta=12))\n{}",
            if terminal {
                "assistant([{'type':'text','text':'DONE'}]); result()"
            } else {
                ""
            }
        ));
        let session = fixture.session().await;
        let mut turn = fixture
            .adapter
            .start_turn(&session, TurnRequest::new("invented"))
            .await
            .unwrap();
        let events = collect(&mut turn).await;
        if terminal {
            assert!(
                matches!(events.as_slice(), [Ok(ProviderEvent::TurnStarted { .. }), Ok(ProviderEvent::AssistantMessageDelta { content, .. }), Ok(ProviderEvent::TurnCompleted)] if content == "DONE"),
                "{events:?}"
            );
        } else {
            assert!(
                matches!(
                    events.as_slice(),
                    [
                        Ok(ProviderEvent::TurnStarted { .. }),
                        Err(ProviderError::StreamClosed)
                    ]
                ),
                "{events:?}"
            );
        }
        reaped(fixture.read("pid").as_u64().unwrap()).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "uses the installed Claude account; coordinator runs explicitly"]
async fn live_adapter_streams_and_resumes_context() {
    assert_eq!(
        std::env::var("PROMPTING_TIME_LIVE_CLAUDE").as_deref(),
        Ok("1")
    );
    let workspace = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let adapter = ClaudeAdapter::new("claude".into());
    let conversation_id = ConversationId::new();
    let result = timeout(Duration::from_secs(120), async {
        let session = adapter.start_session(StartSession {conversation_id, working_directory:workspace.path().into()}).await?;
        let marker = format!("INVENTED-{}", uuid::Uuid::now_v7());
        for (prompt, expected) in [(format!("Remember this invented marker: {marker}. Reply only STORED. Do not use tools."), "STORED"), ("What was the invented marker? Reply with only that exact marker. Do not use tools.".into(), marker.as_str())] {
            let session = adapter.resume_session(&session.native_id, ResumeSession {conversation_id, working_directory:workspace.path().into()}).await?;
            let mut turn = adapter.start_turn(&session, TurnRequest::new(prompt)).await?;
            let mut text = String::new();
            let mut completed = false;
            while let Some(event) = turn.recv().await {
                match event? {
                    ProviderEvent::AssistantMessageDelta {content, ..} => text.push_str(&content),
                    ProviderEvent::TurnCompleted => completed = true,
                    ProviderEvent::ApprovalRequested {request_id, ..} | ProviderEvent::UserInputRequested {request_id, ..} => adapter.respond(&session, &request_id, ApprovalResponse::Denied).await?,
                    _ => {},
                }
            }
            turn.shutdown().await?;
            assert!(completed);
            assert_eq!(text.trim(), expected);
        }
        Ok::<_, ProviderError>(())
    }).await;
    adapter.shutdown().await.unwrap();
    result.expect("live smoke deadline").unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "uses the installed Claude account; coordinator runs explicitly"]
async fn live_adapter_denies_and_allows_invented_write() {
    assert_eq!(
        std::env::var("PROMPTING_TIME_LIVE_CLAUDE").as_deref(),
        Ok("1")
    );
    for approve in [false, true] {
        let workspace = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let target = workspace.path().join("adapter-probe.txt");
        let adapter = ClaudeAdapter::new("claude".into());
        let result = timeout(Duration::from_secs(120), async {
            let session = adapter.start_session(StartSession {conversation_id:ConversationId::new(), working_directory:workspace.path().into()}).await?;
            let mut turn = adapter.start_turn(&session, TurnRequest::new(format!("Use the Write tool exactly once to write the exact text ADAPTER-PROBE to {}. Use no other tools. If permission is denied, do not retry. Then reply DONE.", target.display()))).await?;
            let mut requested = false;
            let mut completed = false;
            while let Some(event) = turn.recv().await {
                match event? {
                    ProviderEvent::ApprovalRequested {request_id, operation, scope, ..} => {
                        let exact = operation == "Write" && Path::new(&scope) == target && !requested;
                        requested |= exact;
                        adapter.respond(&session, &request_id, if approve && exact {ApprovalResponse::Approved} else {ApprovalResponse::Denied}).await?;
                    }
                    ProviderEvent::UserInputRequested {request_id, ..} => adapter.respond(&session, &request_id, ApprovalResponse::Denied).await?,
                    ProviderEvent::TurnCompleted => completed = true,
                    _ => {},
                }
            }
            turn.shutdown().await?;
            assert!(requested && completed);
            if approve { assert_eq!(fs::read_to_string(&target).unwrap().trim(), "ADAPTER-PROBE"); }
            else { assert!(!target.exists()); }
            Ok::<_, ProviderError>(())
        }).await;
        adapter.shutdown().await.unwrap();
        result.expect("live smoke deadline").unwrap();
    }
}

#[tokio::test]
async fn cancelled_pre_prompt_initialization_stays_fresh_and_reaps_owner() {
    let fixture = Fixture::new("result()");
    fs::write(fixture.directory.path().join("hold-initialize"), "").unwrap();
    let session = fixture.session().await;
    let initialized = fixture.directory.path().join("initialize.ready");
    {
        let start = fixture
            .adapter
            .start_turn(&session, TurnRequest::new("never dispatched"));
        tokio::pin!(start);
        tokio::select! {
            _ = &mut start => panic!("initialization barrier must hold start"),
            _ = wait_file(&initialized) => {}
        }
    }
    reaped(fixture.read("pid").as_u64().unwrap()).await;
    fs::remove_file(fixture.directory.path().join("hold-initialize")).unwrap();
    let resumed = fixture.resume(&session).await;
    let mut turn = fixture
        .adapter
        .start_turn(&resumed, TurnRequest::new("first actual prompt"))
        .await
        .unwrap();
    collect(&mut turn).await;
    assert!(
        fixture
            .read("args")
            .as_array()
            .unwrap()
            .contains(&json!(format!("--session-id={}", session.native_id)))
    );
}

#[tokio::test]
async fn streaming_deduplicates_full_messages_and_omits_child_text() {
    let fixture = Fixture::new(
        r#"
def stream(event):
    emit(dict(type='stream_event', session_id=session, parent_tool_use_id=None, event=event))
stream({'type':'message_start','message':{'id':'message-1'}})
stream({'type':'content_block_start','index':0,'content_block':{'type':'text','text':''}})
stream({'type':'content_block_delta','index':0,'delta':{'type':'text_delta','text':'HEL'}})
assistant([{'type':'text','text':'HELLO'}])
assistant([{'type':'text','text':'HELLO'}])
assistant([{'type':'text','text':'CHILD SECRET'}], parent='agent-a', mid='child')
result(result='HELLO')
"#,
    );
    let session = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(&session, TurnRequest::new("stream"))
        .await
        .unwrap();
    let events = collect(&mut turn).await;
    let text: String = events
        .iter()
        .filter_map(|event| match event {
            Ok(ProviderEvent::AssistantMessageDelta { content, .. }) => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "HELLO");
}

#[tokio::test]
async fn fragmented_assistant_frames_keep_native_text_block_identity() {
    let fixture = Fixture::new(
        r#"
def stream(event):
    emit(dict(type='stream_event', session_id=session, parent_tool_use_id=None, event=event))
def full(fid, text):
    emit(dict(type='assistant', uuid=fid, session_id=session, parent_tool_use_id=None,
              message=dict(id='message-1', content=[dict(type='text', text=text)])))
stream({'type':'message_start','message':{'id':'message-1'}})
stream({'type':'content_block_start','index':0,'content_block':{'type':'thinking','thinking':''}})
assistant([{'type':'thinking','thinking':'INVENTED'}])
stream({'type':'content_block_stop','index':0})
stream({'type':'content_block_start','index':1,'content_block':{'type':'text','text':''}})
stream({'type':'content_block_delta','index':1,'delta':{'type':'text_delta','text':'HEL'}})
# Installed CLI emits the completed one-block assistant before the raw stop event.
full('frame-1', 'HELLO')
stream({'type':'content_block_stop','index':1})
stream({'type':'content_block_start','index':2,'content_block':{'type':'text','text':''}})
stream({'type':'content_block_delta','index':2,'delta':{'type':'text_delta','text':'WOR'}})
# A repeated earlier frame must not attach to the currently active block.
full('frame-1', 'HELLO')
full('frame-2', 'WORLD')
stream({'type':'content_block_stop','index':2})
stream({'type':'content_block_start','index':3,'content_block':{'type':'text','text':''}})
stream({'type':'content_block_delta','index':3,'delta':{'type':'text_delta','text':'HELLO'}})
full('frame-3', 'HELLO')
stream({'type':'content_block_stop','index':3})
stream({'type':'message_stop'})
full('frame-1', 'HELLO')
full('frame-2', 'WORLD')
full('frame-3', 'HELLO')
result()
"#,
    );
    let session = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(&session, TurnRequest::new("fragmented stream"))
        .await
        .unwrap();
    let events = collect(&mut turn).await;
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    let text: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Ok(ProviderEvent::AssistantMessageDelta {
                native_item_id,
                content,
            }) => Some((native_item_id.as_str(), content.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        text,
        [
            ("message-1:1", "HEL"),
            ("message-1:1", "LO"),
            ("message-1:2", "WOR"),
            ("message-1:2", "LD"),
            ("message-1:3", "HELLO"),
        ]
    );
}

#[tokio::test]
async fn full_only_assistant_fragments_preserve_distinct_identical_blocks() {
    let fixture = Fixture::new(
        r#"
def full(fid, blocks):
    emit(dict(type='assistant', uuid=fid, session_id=session, parent_tool_use_id=None,
              message=dict(id='message-1', content=blocks)))
full('frame-1', [dict(type='text', text='HELLO')])
full('frame-2', [dict(type='text', text='HELLO')])
full('frame-3', [dict(type='text', text='A'), dict(type='text', text='B')])
full('frame-1', [dict(type='text', text='HELLO')])
full('frame-3', [dict(type='text', text='A'), dict(type='text', text='B')])
result()
"#,
    );
    let session = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(&session, TurnRequest::new("full fragments"))
        .await
        .unwrap();
    let events = collect(&mut turn).await;
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    let mut text = BTreeMap::new();
    for event in events {
        if let Ok(ProviderEvent::AssistantMessageDelta {
            native_item_id,
            content,
        }) = event
        {
            text.entry(native_item_id)
                .or_insert_with(String::new)
                .push_str(&content);
        }
    }
    assert_eq!(text.len(), 4);
    assert_eq!(
        text.values().map(String::as_str).collect::<Vec<_>>(),
        ["HELLO", "HELLO", "A", "B"]
    );
}

#[tokio::test]
async fn assistant_frame_identity_conflicts_and_unmapped_stream_frames_fail_closed() {
    for body in [
        r#"
for mid in ['message-1', 'message-2']:
    emit(dict(type='assistant', uuid='frame-1', session_id=session,
              message=dict(id=mid, content=[dict(type='text', text='A')])))
result()
"#,
        r#"
for blocks in [[dict(type='text', text='A')], [dict(type='text', text='A'), dict(type='text', text='B')]]:
    emit(dict(type='assistant', uuid='frame-1', session_id=session,
              message=dict(id='message-1', content=blocks)))
result()
"#,
        r#"
for ev in [dict(type='message_start', message=dict(id='message-1')),
           dict(type='content_block_start', index=1, content_block=dict(type='text', text='A')),
           dict(type='content_block_stop', index=1), dict(type='message_stop')]:
    emit(dict(type='stream_event', session_id=session, event=ev))
assistant([dict(type='text', text='A')])
result()
"#,
        r#"
emit(dict(type='stream_event', session_id=session, event=dict(type='message_start', message=dict(id='message-1'))))
emit(dict(type='stream_event', session_id=session, event=dict(type='content_block_start', index=0, content_block=dict(type='text', text=''))))
for index in range(1025):
    emit(dict(type='assistant', uuid='frame-' + str(index), session_id=session,
              message=dict(id='message-1', content=[dict(type='text', text='')])))
result()
"#,
    ] {
        let fixture = Fixture::new(body);
        let session = fixture.session().await;
        let mut turn = fixture
            .adapter
            .start_turn(&session, TurnRequest::new("invalid frame identity"))
            .await
            .unwrap();
        let events = collect(&mut turn).await;
        assert!(events.iter().any(Result::is_err), "{events:?}");
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Ok(ProviderEvent::TurnCompleted)))
        );
    }
}

#[tokio::test]
async fn recursive_lifecycle_waits_for_late_identity_and_actual_terminals() {
    let fixture = Fixture::new(
        r#"
assistant([{'type':'tool_use','id':'agent-a','name':'Agent','input':{}}])
task('task_started','task-a','agent-a')
task('task_started','task-b','agent-b')
task('task_notification','task-b','agent-b',status='failed')
result()
assistant([{'type':'tool_use','id':'agent-b','name':'Agent','input':{}}], parent='agent-a',mid='child')
task('task_started','task-b','agent-b')
task('task_notification','task-a','agent-a',status='completed')
"#,
    );
    let session = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(&session, TurnRequest::new("tree"))
        .await
        .unwrap();
    let events = collect(&mut turn).await;
    let children: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Ok(ProviderEvent::ChildAgentActivity {
                parent_native_thread_id,
                child_statuses,
                ..
            }) => Some((parent_native_thread_id, child_statuses)),
            _ => None,
        })
        .collect();
    assert!(children.iter().any(|(parent, statuses)| {
        *parent == "task-a"
            && statuses.iter().any(|child| {
                child.native_thread_id == "task-b" && child.status == NativeAgentStatus::Errored
            })
    }));
    assert_eq!(
        children
            .iter()
            .filter(|(_, statuses)| statuses
                .iter()
                .any(|child| child.native_thread_id == "task-b"
                    && child.status == NativeAgentStatus::Running))
            .count(),
        1
    );
    assert!(matches!(
        events.last(),
        Some(Ok(ProviderEvent::TurnCompleted))
    ));
}

#[tokio::test]
async fn invalid_results_and_unresolved_lifecycle_fail_closed() {
    for body in [
        "emit({'type':'result','session_id':session,'subtype':'success'})",
        "emit({'type':'result','session_id':'wrong','subtype':'success','is_error':False})",
        "emit({'type':'result','session_id':session,'subtype':'error_during_execution','is_error':True})",
        "result(stop_reason='tool_deferred', deferred_tool_use={'id':'pending','name':'Write','input':{}})",
        "result(stop_reason='tool_deferred')",
        "task('task_started','task-b','agent-b'); result()",
        "emit({'type':'system','subtype':'task_updated','session_id':session,'task_id':'task-a','patch':{'status':'killed'}})",
        "print('not-json', flush=True)",
        "sys.exit(0)",
    ] {
        let fixture = Fixture::new(body);
        let session = fixture.session().await;
        let mut turn = fixture
            .adapter
            .start_turn(&session, TurnRequest::new("invalid"))
            .await
            .unwrap();
        let events = collect(&mut turn).await;
        assert!(events.iter().any(Result::is_err), "{body}: {events:?}");
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Ok(ProviderEvent::TurnCompleted)))
        );
    }
}

#[tokio::test]
async fn approval_preserves_input_denies_without_permissions_and_rejects_duplicates() {
    for decision in [ApprovalResponse::Approved, ApprovalResponse::Denied] {
        let fixture = Fixture::new(
            "permission(); record('response', receive()); barrier('response'); result()",
        );
        let session = fixture.session().await;
        let mut turn = fixture
            .adapter
            .start_turn(&session, TurnRequest::new("permission"))
            .await
            .unwrap();
        assert!(matches!(
            event(&mut turn).await.unwrap(),
            ProviderEvent::TurnStarted { .. }
        ));
        let ProviderEvent::ApprovalRequested {
            request_id,
            details,
            ..
        } = event(&mut turn).await.unwrap()
        else {
            panic!("approval")
        };
        assert!(details.is_some());
        fixture
            .adapter
            .respond(&session, &request_id, decision.clone())
            .await
            .unwrap();
        assert!(
            fixture
                .adapter
                .respond(&session, &request_id, decision.clone())
                .await
                .is_err()
        );
        wait_file(&fixture.directory.path().join("response.ready")).await;
        let response = fixture.read("response");
        let response = &response["response"]["response"];
        if decision == ApprovalResponse::Approved {
            assert_eq!(
                response["updatedInput"],
                json!({"file_path":"invented.txt","content":"INVENTED"})
            );
        } else {
            assert_eq!(response["behavior"], "deny");
            assert!(response.get("updatedInput").is_none());
        }
        assert!(response.get("updatedPermissions").is_none());
        fs::write(fixture.directory.path().join("response.release"), "").unwrap();
        assert!(matches!(
            collect(&mut turn).await.last(),
            Some(Ok(ProviderEvent::TurnCompleted))
        ));
    }
}

#[tokio::test]
async fn questions_map_canonical_answers_to_original_text_and_decline_multiselect() {
    for multiple in [false, true] {
        let fixture = Fixture::new(&format!(
            r#"
permission(tool='AskUserQuestion',data={{'questions':[{{'question':'Exact question?', 'header':'Choice','multiSelect':{},'options':[{{'label':'BLUE','description':'Blue choice'}}]}}]}})
record('response', receive())
result()
"#,
            if multiple { "True" } else { "False" }
        ));
        let session = fixture.session().await;
        let mut turn = fixture
            .adapter
            .start_turn(&session, TurnRequest::new("question"))
            .await
            .unwrap();
        event(&mut turn).await.unwrap();
        if !multiple {
            let ProviderEvent::UserInputRequested {
                request_id,
                questions,
                ..
            } = event(&mut turn).await.unwrap()
            else {
                panic!("question")
            };
            let mut answers = BTreeMap::new();
            answers.insert(questions[0].id.clone(), vec!["BLUE".into()]);
            fixture
                .adapter
                .respond(&session, &request_id, ApprovalResponse::Answers(answers))
                .await
                .unwrap();
        }
        let events = collect(&mut turn).await;
        assert!(matches!(
            events.last(),
            Some(Ok(ProviderEvent::TurnCompleted))
        ));
        let response = fixture.read("response");
        let response = &response["response"]["response"];
        if multiple {
            assert_eq!(response["behavior"], "deny");
            assert!(
                response["message"]
                    .as_str()
                    .unwrap()
                    .contains("single-select")
            );
        } else {
            assert_eq!(
                response["updatedInput"]["answers"],
                json!({"Exact question?":"BLUE"})
            );
        }
    }
}

#[tokio::test]
async fn permission_request_is_not_mutation_and_unknown_tool_completion_is_uncertain() {
    let fixture = Fixture::new(
        r#"
assistant([{'type':'tool_use','id':'tool-1','name':'Unfamiliar','input':{}}])
emit({'type':'user','session_id':session,'message':{'content':[{'type':'tool_result','tool_use_id':'tool-1','content':'DONE'}]}})
result()
"#,
    );
    let session = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(&session, TurnRequest::new("unknown tool"))
        .await
        .unwrap();
    let events = collect(&mut turn).await;
    assert!(events.iter().any(|event| matches!(
        event,
        Ok(ProviderEvent::NativeItemActivity {
            mutation: MutationState::Unknown,
            ..
        })
    )));
}

#[tokio::test]
async fn session_bindings_reject_changed_ownership_and_bound_lifetime_admission() {
    let fixture = Fixture::new("result()");
    let session = fixture.session().await;
    assert!(
        fixture
            .adapter
            .resume_session(
                &session.native_id,
                ResumeSession {
                    conversation_id: ConversationId::new(),
                    working_directory: fixture.directory.path().into(),
                }
            )
            .await
            .is_err()
    );
    assert!(
        fixture
            .adapter
            .resume_session(
                "not-a-session",
                ResumeSession {
                    conversation_id: fixture.conversation,
                    working_directory: fixture.directory.path().into(),
                }
            )
            .await
            .is_err()
    );
    for _ in 1..4096 {
        fixture.session().await;
    }
    assert!(
        fixture
            .adapter
            .start_session(StartSession {
                conversation_id: fixture.conversation,
                working_directory: fixture.directory.path().into()
            })
            .await
            .is_err()
    );
    assert_eq!(fixture.resume(&session).await, session);
}

#[tokio::test]
async fn concurrent_sessions_are_independent_and_same_session_start_is_excluded() {
    let fixture = Fixture::new("barrier(session); result()");
    let first = fixture.session().await;
    let second = fixture.session().await;
    let mut one = fixture
        .adapter
        .start_turn(&first, TurnRequest::new("one"))
        .await
        .unwrap();
    let mut two = fixture
        .adapter
        .start_turn(&second, TurnRequest::new("two"))
        .await
        .unwrap();
    assert!(
        fixture
            .adapter
            .start_turn(&first, TurnRequest::new("duplicate"))
            .await
            .is_err()
    );
    fs::write(
        fixture
            .directory
            .path()
            .join(format!("{}.release", second.native_id)),
        "",
    )
    .unwrap();
    assert!(matches!(
        collect(&mut two).await.last(),
        Some(Ok(ProviderEvent::TurnCompleted))
    ));
    one.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelled_blocked_prompt_write_is_uncertain_and_never_retried_fresh() {
    let fixture = Fixture::new("result()");
    fs::write(fixture.directory.path().join("hold-prompt-read"), "").unwrap();
    let session = fixture.session().await;
    let ready = fixture.directory.path().join("prompt-read.ready");
    {
        let start = fixture
            .adapter
            .start_turn(&session, TurnRequest::new("x".repeat(1024 * 1024)));
        tokio::pin!(start);
        tokio::select! {
            _ = &mut start => panic!("blocked prompt must keep start pending"),
            _ = wait_file(&ready) => {},
        }
    }
    reaped(fixture.read("pid").as_u64().unwrap()).await;
    fs::remove_file(fixture.directory.path().join("hold-prompt-read")).unwrap();
    let mut turn = fixture
        .adapter
        .start_turn(
            &fixture.resume(&session).await,
            TurnRequest::new("followup"),
        )
        .await
        .unwrap();
    collect(&mut turn).await;
    assert!(
        fixture
            .read("args")
            .as_array()
            .unwrap()
            .contains(&json!(format!("--resume={}", session.native_id)))
    );
}

#[tokio::test]
async fn turn_drop_adapter_drop_and_cancelled_shutdown_reap_owned_processes() {
    for mode in ["turn", "adapter", "cancelled-shutdown"] {
        let fixture = Fixture::new("barrier('active'); result()");
        let session = fixture.session().await;
        let turn = fixture
            .adapter
            .start_turn(&session, TurnRequest::new("wait"))
            .await
            .unwrap();
        wait_file(&fixture.directory.path().join("active.ready")).await;
        let pid = fixture.read("pid").as_u64().unwrap();
        match mode {
            "turn" => drop(turn),
            "adapter" => {
                drop(fixture.adapter);
                reaped(pid).await;
                drop(turn);
            }
            _ => {
                {
                    let shutdown = fixture.adapter.shutdown();
                    tokio::pin!(shutdown);
                    std::future::poll_fn(|cx| {
                        let _ = std::future::Future::poll(shutdown.as_mut(), cx);
                        std::task::Poll::Ready(())
                    })
                    .await;
                }
                fixture.adapter.force_shutdown();
                fixture.adapter.shutdown().await.unwrap();
                drop(turn);
            }
        }
        reaped(pid).await;
    }
}

#[tokio::test]
async fn slow_event_consumer_cannot_prevent_owned_shutdown() {
    let fixture = Fixture::new(
        r#"
for i in range(800):
    assistant([{'type':'text','text':'x'}], mid='message-' + str(i))
barrier('flooded')
"#,
    );
    let session = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(&session, TurnRequest::new("many messages"))
        .await
        .unwrap();
    wait_file(&fixture.directory.path().join("flooded.ready")).await;
    let pid = fixture.read("pid").as_u64().unwrap();
    timeout(Duration::from_secs(2), turn.shutdown())
        .await
        .unwrap()
        .unwrap();
    reaped(pid).await;
}

#[tokio::test]
async fn interrupt_requires_terminal_evidence_and_exact_generation() {
    let fixture = Fixture::new(
        r#"
interrupt = receive()
emit({'type':'control_response','response':{'subtype':'success','request_id':interrupt['request_id'],'response':{}}})
barrier('interrupt-ack')
result(terminal_reason='aborted_tools')
"#,
    );
    let session = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(&session, TurnRequest::new("wait"))
        .await
        .unwrap();
    let ProviderEvent::TurnStarted { native_turn_id } = event(&mut turn).await.unwrap() else {
        panic!("started")
    };
    assert!(
        fixture
            .adapter
            .interrupt(&session, "wrong-generation")
            .await
            .is_err()
    );
    let ack = fixture.directory.path().join("interrupt-ack.ready");
    let interrupt = fixture.adapter.interrupt(&session, &native_turn_id);
    tokio::pin!(interrupt);
    tokio::select! {
        _ = &mut interrupt => panic!("ack alone must not complete interruption"),
        _ = wait_file(&ack) => {},
    }
    fs::write(fixture.directory.path().join("interrupt-ack.release"), "").unwrap();
    interrupt.await.unwrap();
    assert!(matches!(
        event(&mut turn).await.unwrap(),
        ProviderEvent::Interrupted
    ));
    turn.shutdown().await.unwrap();
}

#[tokio::test]
async fn wrong_session_stale_generation_and_bad_answers_are_not_dispatched() {
    let fixture = Fixture::new(
        "permission(tool='AskUserQuestion',data={'questions':[{'question':'Exact?', 'multiSelect':False}]}); record('response',receive()); result()",
    );
    let session = fixture.session().await;
    let other = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(&session, TurnRequest::new("question"))
        .await
        .unwrap();
    event(&mut turn).await.unwrap();
    let ProviderEvent::UserInputRequested {
        request_id,
        questions,
        ..
    } = event(&mut turn).await.unwrap()
    else {
        panic!("question")
    };
    for (target, response) in [
        (&other, ApprovalResponse::Answer("answer".into())),
        (&session, ApprovalResponse::Approved),
        (
            &session,
            ApprovalResponse::Answers(BTreeMap::from([("wrong-id".into(), vec!["answer".into()])])),
        ),
        (
            &session,
            ApprovalResponse::Answers(BTreeMap::from([(
                questions[0].id.clone(),
                vec!["one".into(), "two".into()],
            )])),
        ),
    ] {
        let error = fixture
            .adapter
            .respond(target, &request_id, response)
            .await
            .unwrap_err();
        assert_eq!(
            error.dispatch_certainty(),
            prompting_time_core::providers::DispatchCertainty::NotDispatched
        );
    }
    fixture
        .adapter
        .respond(
            &session,
            &request_id,
            ApprovalResponse::Answer("free text".into()),
        )
        .await
        .unwrap();
    collect(&mut turn).await;
    let mut next = fixture
        .adapter
        .start_turn(&session, TurnRequest::new("again"))
        .await
        .unwrap();
    assert!(
        fixture
            .adapter
            .respond(
                &session,
                &request_id,
                ApprovalResponse::Answer("stale".into())
            )
            .await
            .is_err()
    );
    next.shutdown().await.unwrap();
}

#[tokio::test]
async fn request_payload_is_bounded_and_sequential_callbacks_release_correlation() {
    let oversized =
        Fixture::new("permission(data={'file_path':'invented.txt','content':'x'*65536}); result()");
    let session = oversized.session().await;
    let mut turn = oversized
        .adapter
        .start_turn(&session, TurnRequest::new("oversize"))
        .await
        .unwrap();
    let events = collect(&mut turn).await;
    assert!(events.iter().any(Result::is_err));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Ok(ProviderEvent::ApprovalRequested { .. })))
    );
    let fixture = Fixture::new(
        "for i in range(12):\n    permission(rid=str(i),data={'file_path':'invented.txt','content':'x'*60000})\n    receive()\nresult()",
    );
    let session = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(&session, TurnRequest::new("sequential"))
        .await
        .unwrap();
    event(&mut turn).await.unwrap();
    for _ in 0..12 {
        let ProviderEvent::ApprovalRequested { request_id, .. } = event(&mut turn).await.unwrap()
        else {
            panic!("approval")
        };
        fixture
            .adapter
            .respond(&session, &request_id, ApprovalResponse::Approved)
            .await
            .unwrap();
    }
    assert!(matches!(
        collect(&mut turn).await.last(),
        Some(Ok(ProviderEvent::TurnCompleted))
    ));
}

#[tokio::test]
async fn unsupported_server_requests_receive_errors_and_active_withdrawal_fails_turn() {
    let fixture = Fixture::new(
        "emit({'type':'control_request','request_id':'unsupported','request':{'subtype':'unknown'}}); record('response',receive()); result()",
    );
    let session = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(&session, TurnRequest::new("unsupported"))
        .await
        .unwrap();
    collect(&mut turn).await;
    assert_eq!(fixture.read("response")["response"]["subtype"], "error");
    let fixture = Fixture::new(
        "permission(); emit({'type':'control_cancel_request','request_id':'request-1'}); barrier('cancelled')",
    );
    let session = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(&session, TurnRequest::new("withdraw"))
        .await
        .unwrap();
    let events = collect(&mut turn).await;
    assert!(events.iter().any(Result::is_err));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Ok(ProviderEvent::TurnCompleted)))
    );
}

#[tokio::test]
async fn cancelled_control_response_stops_uncertain_dispatch_and_rejects_retry() {
    let fixture = Fixture::new(
        "permission(data={'file_path':'invented.txt','content':'x'*60000}); barrier('hold-response')",
    );
    let session = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(&session, TurnRequest::new("write"))
        .await
        .unwrap();
    event(&mut turn).await.unwrap();
    let ProviderEvent::ApprovalRequested { request_id, .. } = event(&mut turn).await.unwrap()
    else {
        panic!("approval")
    };
    wait_file(&fixture.directory.path().join("hold-response.ready")).await;
    {
        let response = fixture
            .adapter
            .respond(&session, &request_id, ApprovalResponse::Approved);
        tokio::pin!(response);
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(response.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
    }
    reaped(fixture.read("pid").as_u64().unwrap()).await;
    assert!(
        fixture
            .adapter
            .respond(&session, &request_id, ApprovalResponse::Approved)
            .await
            .is_err()
    );
    turn.shutdown().await.unwrap();
}

#[tokio::test]
async fn changed_request_identity_cannot_publish_the_same_native_tool_twice() {
    let fixture = Fixture::new(
        r#"
permission()
emit({'type':'control_request','request_id':'changed-id','request':{'subtype':'can_use_tool','tool_name':'Write','tool_use_id':'tool-request-1','input':{'file_path':'invented.txt','content':'INVENTED'}}})
result()
"#,
    );
    let session = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(&session, TurnRequest::new("duplicate tool"))
        .await
        .unwrap();
    let events = collect(&mut turn).await;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Ok(ProviderEvent::ApprovalRequested { .. })))
            .count(),
        1
    );
    assert!(events.iter().any(Result::is_err));
}

#[tokio::test]
async fn native_withdrawal_during_response_write_stops_the_turn() {
    let fixture = Fixture::new(
        r#"
permission(data={'file_path':'invented.txt','content':'x'*60000})
permission(rid='request-2',data={'file_path':'invented.txt','content':'x'*60000})
sys.stdin.read(1)
record('withdraw-during-write.ready', True)
emit({'type':'control_cancel_request','request_id':'request-2'})
barrier('after-withdraw')
"#,
    );
    let session = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(&session, TurnRequest::new("write"))
        .await
        .unwrap();
    event(&mut turn).await.unwrap();
    let ProviderEvent::ApprovalRequested { request_id, .. } = event(&mut turn).await.unwrap()
    else {
        panic!("approval")
    };
    let ProviderEvent::ApprovalRequested {
        request_id: second_id,
        ..
    } = event(&mut turn).await.unwrap()
    else {
        panic!("second approval")
    };
    {
        let first = fixture
            .adapter
            .respond(&session, &request_id, ApprovalResponse::Approved);
        let second = fixture
            .adapter
            .respond(&session, &second_id, ApprovalResponse::Approved);
        tokio::pin!(first, second);
        // Reserve both responses without polling either write acknowledgement. The peer only
        // reads the first byte of the first response, then withdraws the second active callback.
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(first.as_mut(), cx).is_pending());
            assert!(std::future::Future::poll(second.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        wait_file(&fixture.directory.path().join("withdraw-during-write.ready")).await;
        assert!(event(&mut turn).await.is_err());
    }
    turn.shutdown().await.unwrap();
}
