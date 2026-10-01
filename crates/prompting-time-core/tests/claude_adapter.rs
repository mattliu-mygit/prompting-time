use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use prompting_time_core::app::{ConversationRequest, PromptingTime, SubmitRequest};
use prompting_time_core::domain::{AgentStatus, ConversationId, MutationState, RunStatus};
use prompting_time_core::providers::claude::ClaudeAdapter;
use prompting_time_core::providers::{
    ApprovalResponse, NativeAgentStatus, ProviderAdapter, ProviderCapability, ProviderError,
    ProviderEvent, ProviderHealth, ProviderId, ProviderSession, ProviderTurn, ResumeSession,
    StartSession, TurnRequest,
};
use prompting_time_core::router::Router;
use prompting_time_core::store::Store;
use prompting_time_core::thinking::{ThinkingDecision, ThinkingPreference};
use prompting_time_core::workspace::WorkspaceManager;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::time::timeout;

#[path = "support/conversation_approvals.rs"]
mod conversation_approvals;
use conversation_approvals::load_subtree_approvals;

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
                context_budget:
                    prompting_time_core::context_budget::ContextBudget::provider_default(),
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
                    context_budget:
                        prompting_time_core::context_budget::ContextBudget::provider_default(),
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

    fn app(&self, store: Store) -> PromptingTime {
        PromptingTime::new(
            store,
            Router::default(),
            WorkspaceManager::new(self.directory.path()),
            vec![Arc::new(ClaudeAdapter::new(
                self.directory.path().join("claude-fixture"),
            ))],
        )
        .unwrap()
    }
}

fn app_request(
    conversation_id: ConversationId,
    content: &str,
    provider: Option<ProviderId>,
) -> SubmitRequest {
    SubmitRequest {
        context_budget: None,
        thinking: None,
        command_id: uuid::Uuid::now_v7().to_string(),
        conversation_id,
        content: content.into(),
        provider_override: provider,
    }
}

#[tokio::test]
async fn app_projectless_auto_routing_and_restart_resume_keep_canonical_session() {
    let fixture = Fixture::new(
        r#"
record('cwd', os.getcwd())
if any(arg.startswith('--resume=') for arg in sys.argv):
    assistant([{'type':'text','text':(root / 'context').read_text()}])
else:
    (root / 'context').write_text('INVENTED-CONTINUITY')
    assistant([{'type':'text','text':'STORED'}])
result()
"#,
    );
    let database = fixture.directory.path().join("app.sqlite");
    let store = Store::open(&database).await.unwrap();
    let app = fixture.app(store.clone());
    let conversation = app
        .create_conversation(ConversationRequest::projectless("Invented continuity"))
        .await
        .unwrap();
    let first = app
        .submit(app_request(
            conversation.id,
            "Remember INVENTED-CONTINUITY",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(first.decision.provider, ProviderId::Claude);
    assert_eq!(
        timeout(Duration::from_secs(10), first.handle.wait())
            .await
            .unwrap()
            .unwrap()
            .status,
        RunStatus::Completed
    );
    let native = store
        .load_provider_session(conversation.id, ProviderId::Claude)
        .await
        .unwrap()
        .unwrap();
    let workspace = store.load_workspace(conversation.id).await.unwrap();
    assert!(workspace.project_root.is_none());
    assert_eq!(
        Path::new(fixture.read("cwd").as_str().unwrap()),
        workspace.execution_path.canonicalize().unwrap()
    );
    app.shutdown().await.unwrap();
    drop(app);
    drop(store);

    let store = Store::open(&database).await.unwrap();
    let app = fixture.app(store.clone());
    assert_eq!(app.reconcile_startup().await.unwrap(), 0);
    let second = app
        .submit(app_request(
            conversation.id,
            "Recall",
            Some(ProviderId::Claude),
        ))
        .await
        .unwrap();
    assert_eq!(
        timeout(Duration::from_secs(10), second.handle.wait())
            .await
            .unwrap()
            .unwrap()
            .status,
        RunStatus::Completed
    );
    assert_eq!(
        store
            .load_provider_session(conversation.id, ProviderId::Claude)
            .await
            .unwrap()
            .unwrap(),
        native
    );
    assert!(
        fixture
            .read("args")
            .as_array()
            .unwrap()
            .contains(&json!(format!("--resume={}", native.native_id)))
    );
    let timeline = app
        .load_timeline_snapshot(conversation.id, None, 80)
        .await
        .unwrap();
    assert!(
        timeline
            .events
            .items
            .iter()
            .any(|record| record.event.content == "INVENTED-CONTINUITY")
    );
    assert_eq!(
        app.load_run_audits(conversation.id, None, 10)
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn app_approval_and_question_responses_use_canonical_ids_and_reject_stale_actions() {
    let fixture = Fixture::new(
        r#"
permission()
record('permission-response', receive())
permission(rid='question-request',tool='AskUserQuestion',data={'questions':[{'question':'Exact question?', 'header':'Choice','multiSelect':False,'options':[{'label':'BLUE','description':'Blue choice'}]}]})
record('question-response', receive())
result()
"#,
    );
    let app = fixture.app(Store::open_in_memory().await.unwrap());
    let conversation = app
        .create_conversation(ConversationRequest::projectless("Invented approval"))
        .await
        .unwrap();
    let submission = app
        .submit(app_request(
            conversation.id,
            "Request permission and a choice",
            Some(ProviderId::Claude),
        ))
        .await
        .unwrap();
    let work = async {
        let mut changes = app.subscribe_changes();
        let mut previous = None;
        for is_question in [false, true] {
            let approval = loop {
                let approvals = app
                    .load_approvals(conversation.id, None, true, 30)
                    .await
                    .unwrap();
                if let Some(approval) = approvals
                    .items
                    .into_iter()
                    .find(|item| Some(item.id) != previous)
                {
                    break approval;
                }
                changes.recv().await.unwrap();
            };
            assert_eq!(approval.provider, ProviderId::Claude);
            let response = if is_question {
                let questions = app
                    .load_approval_questions(approval.id, None, 30)
                    .await
                    .unwrap();
                assert_eq!(questions.items[0].question, "Exact question?");
                ApprovalResponse::Answers(BTreeMap::from([(
                    questions.items[0].id.clone(),
                    vec!["BLUE".into()],
                )]))
            } else {
                assert_eq!(approval.operation, "Write");
                ApprovalResponse::Denied
            };
            app.respond_to_approval_id(approval.id, response)
                .await
                .unwrap();
            assert!(
                app.respond_to_approval_id(approval.id, ApprovalResponse::Approved)
                    .await
                    .is_err()
            );
            previous = Some(approval.id);
        }
        assert_eq!(
            submission.handle.wait().await.unwrap().status,
            RunStatus::Completed
        );
    };
    timeout(Duration::from_secs(10), work).await.unwrap();
    assert_eq!(
        fixture.read("permission-response")["response"]["response"]["behavior"],
        "deny"
    );
    assert_eq!(
        fixture.read("question-response")["response"]["response"]["updatedInput"]["answers"],
        json!({"Exact question?":"BLUE"})
    );
    assert!(
        app.load_approvals(conversation.id, None, true, 30)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn terminal_first_child_identity_persists_pending_operation_output() {
    let fixture = Fixture::new(
        r#"
assistant([{'type':'tool_use','id':'agent','name':'Agent','input':{}}])
assistant([{'type':'tool_use','id':'child-tool','name':'Read','input':{'file_path':'invented.txt'}}],parent='agent',mid='child-message')
emit({'type':'user','session_id':session,'parent_tool_use_id':'agent','message':{'content':[{'type':'tool_result','tool_use_id':'child-tool','content':'CHILD_OUTPUT'}]}})
task('task_notification','child','agent',status='completed')
result()
"#,
    );
    let store = Store::open_in_memory().await.unwrap();
    let app = fixture.app(store.clone());
    let conversation = app
        .create_conversation(ConversationRequest::projectless("Terminal first child"))
        .await
        .unwrap();
    let submission = app
        .submit(app_request(conversation.id, "Read in child", None))
        .await
        .unwrap();
    let status = timeout(Duration::from_secs(10), submission.handle.wait())
        .await
        .unwrap()
        .unwrap()
        .status;
    let tree = store
        .load_agent_page(conversation.id, None, 20)
        .await
        .unwrap();
    let child = tree.items.iter().find(|item| item.depth == 1).unwrap();
    let child_conversation = app
        .list_child_conversation_overviews(conversation.id, None, 20)
        .await
        .unwrap()
        .items
        .remove(0)
        .conversation;
    let timeline = app
        .load_timeline_snapshot(child_conversation.id, None, 30)
        .await
        .unwrap();
    let mut outputs = Vec::new();
    for row in timeline
        .events
        .items
        .iter()
        .filter(|row| row.operation.is_some())
    {
        let detail = app.load_event_detail(row.event.id).await.unwrap();
        if detail
            .operation
            .as_ref()
            .is_some_and(|detail| detail.operation.output.as_deref() == Some("CHILD_OUTPUT"))
        {
            outputs.push(row.event.agent_id);
        }
    }
    app.shutdown().await.unwrap();
    assert_eq!(status, RunStatus::Completed);
    assert_eq!(outputs, vec![child.agent.id]);
}

#[tokio::test]
async fn app_recursive_claude_tasks_project_into_canonical_sidebar_hierarchy() {
    let fixture = Fixture::new(
        r#"
assistant([{'type':'tool_use','id':'agent-a','name':'Agent','input':{}}])
task('task_started','task-a','agent-a')
assistant([{'type':'tool_use','id':'agent-b','name':'Agent','input':{}}], parent='agent-a',mid='child')
task('task_started','task-b','agent-b')
task('task_notification','task-b','agent-b',status='completed')
task('task_notification','task-a','agent-a',status='completed')
result()
"#,
    );
    let store = Store::open_in_memory().await.unwrap();
    let app = fixture.app(store.clone());
    let conversation = app
        .create_conversation(ConversationRequest::projectless("Invented tree"))
        .await
        .unwrap();
    let submission = app
        .submit(app_request(conversation.id, "Delegate recursively", None))
        .await
        .unwrap();
    assert_eq!(
        timeout(Duration::from_secs(10), submission.handle.wait())
            .await
            .unwrap()
            .unwrap()
            .status,
        RunStatus::Completed
    );
    let tree = store
        .load_agent_page(conversation.id, None, 20)
        .await
        .unwrap();
    assert_eq!(tree.items.len(), 3);
    let child = tree.items.iter().find(|item| item.depth == 1).unwrap();
    let grandchild = tree.items.iter().find(|item| item.depth == 2).unwrap();
    assert_eq!(grandchild.agent.parent_id, Some(child.agent.id));
    assert_eq!((child.depth, grandchild.depth), (1, 2));
    let child_conversation = app
        .list_child_conversation_overviews(conversation.id, None, 20)
        .await
        .unwrap()
        .items
        .remove(0)
        .conversation;
    let grandchild_conversation = app
        .list_child_conversation_overviews(child_conversation.id, None, 20)
        .await
        .unwrap()
        .items
        .remove(0)
        .conversation;
    let path = app
        .load_conversation_path(grandchild_conversation.id)
        .await
        .unwrap();
    assert_eq!(
        path.items
            .iter()
            .map(|item| item.conversation.id)
            .collect::<Vec<_>>(),
        vec![
            conversation.id,
            child_conversation.id,
            grandchild_conversation.id
        ]
    );
    assert!(
        tree.items
            .iter()
            .all(|item| item.agent.status == AgentStatus::Completed)
    );
    assert!(
        tree.items
            .iter()
            .all(|item| item.agent.provider_native_id.is_none())
    );
    assert_eq!(
        app.load_conversation_overview(conversation.id)
            .await
            .unwrap()
            .rollup_status,
        Some(prompting_time_core::domain::RollupStatus::Completed)
    );
    app.shutdown().await.unwrap();
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
models = [{'value':'default','resolvedModel':'invented-model','supportsEffort':True,'supportedEffortLevels':['low','medium','high','xhigh','max']}]
metadata = json.loads((root / 'thinking-metadata').read_text()) if (root / 'thinking-metadata').exists() else {}
emit({'type':'control_response','response':{'subtype':'success','request_id':initialize['request_id'],'response':{'models':metadata.get('models', models), 'account':'MUST-NOT-RETAIN'}}})
message = receive()
assert message.get('request', {}).get('subtype') == 'get_settings'
record('get-settings', message)
if (root / 'hold-settings').exists():
    barrier('settings')
applied = {'model':'invented-model','effort':os.environ.get('CLAUDE_CODE_EFFORT_LEVEL', 'high')}
settings = json.loads(sys.argv[sys.argv.index('--settings')+1]) if '--settings' in sys.argv else {}
record('compaction-env', {key:os.environ.get(key) for key in ['DISABLE_COMPACT','DISABLE_AUTO_COMPACT','CLAUDE_CODE_AUTO_COMPACT_WINDOW']})
emit({'type':'control_response','response':{'subtype':'success','request_id':message['request_id'],'response':{'applied':metadata.get('applied', applied), 'effective':metadata.get('effective', settings), 'errors':metadata.get('errors', []), 'settings':{'secret':'MUST-NOT-RETAIN'}}}})
if (root / 'hold-prompt-read').exists():
    sys.stdin.read(1)
    barrier('prompt-read')
prompt = receive()
record('prompt', prompt)
record('effort-env', os.environ.get('CLAUDE_CODE_EFFORT_LEVEL'))
"#;

fn thinking_request(preference: ThinkingPreference) -> TurnRequest {
    TurnRequest::new("Review this code")
        .with_thinking(ThinkingDecision::resolve(preference, "Review this code").unwrap())
}

#[tokio::test]
async fn context_budget_launch_settings_resume_change_and_reset() {
    use prompting_time_core::context_budget::ContextBudget;
    for version in ["2.1.205 (Claude Code)", "2.1.286 (Claude Code)"] {
        let fixture = Fixture::new("result()");
        fs::write(fixture.directory.path().join("version"), version).unwrap();
        let session = fixture.session().await;
        for tokens in [
            Some(200_000),
            Some(300_000),
            Some(400_000),
            Some(500_000),
            None,
        ] {
            let budget = tokens.map_or(ContextBudget::ProviderDefault, |tokens| {
                ContextBudget::Tokens { tokens }
            });
            let mut turn = fixture
                .adapter
                .start_turn(
                    &session,
                    TurnRequest::new("invented").with_context_budget(budget),
                )
                .await
                .unwrap();
            collect(&mut turn).await;
            turn.shutdown().await.unwrap();
            let args = fixture.read("args");
            let args = args.as_array().unwrap();
            if let Some(tokens) = tokens {
                let index = args
                    .iter()
                    .position(|arg| arg == "--settings")
                    .expect("numeric budget needs inline settings");
                let settings: Value =
                    serde_json::from_str(args[index + 1].as_str().unwrap()).unwrap();
                assert_eq!(
                    settings,
                    json!({"autoCompactEnabled":true,"autoCompactWindow":tokens,"env":{"DISABLE_COMPACT":"0","DISABLE_AUTO_COMPACT":"0","CLAUDE_CODE_AUTO_COMPACT_WINDOW":""}})
                );
                assert_eq!(fixture.read("compaction-env"), settings["env"]);
            } else {
                assert!(!args.iter().any(|arg| arg == "--settings"));
            }
            assert_eq!(fixture.read("prompt")["session_id"], session.native_id);
        }
    }
}

#[tokio::test]
async fn context_budget_malformed_policy_or_validation_readback_never_dispatches() {
    use prompting_time_core::context_budget::ContextBudget;
    let valid = json!({"autoCompactEnabled":true,"autoCompactWindow":300000,"env":{"DISABLE_COMPACT":"0","DISABLE_AUTO_COMPACT":"0","CLAUDE_CODE_AUTO_COMPACT_WINDOW":""}});
    let mut cases = vec![
        json!({"effective":{}}),
        json!({"effective":{"autoCompactEnabled":false,"autoCompactWindow":300000}}),
        json!({"effective":valid,"errors":[{"message":"PRIVATE_POLICY_ERROR"}]}),
        json!({"effective":valid,"errors":"PRIVATE_ERROR"}),
    ];
    for (field, value) in [
        ("autoCompactEnabled", json!("true")),
        ("autoCompactWindow", json!(200000)),
        ("autoCompactWindow", json!("300000")),
        ("env", Value::Null),
    ] {
        let mut effective = valid.clone();
        effective[field] = value;
        cases.push(json!({"effective":effective}));
    }
    for field in [
        "DISABLE_COMPACT",
        "DISABLE_AUTO_COMPACT",
        "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
    ] {
        for value in [Value::Null, json!("1")] {
            let mut effective = valid.clone();
            effective["env"][field] = value;
            cases.push(json!({"effective":effective}));
        }
    }
    for version in ["2.1.205 (Claude Code)", "2.1.286 (Claude Code)"] {
        for metadata in &cases {
            let fixture = Fixture::new("result()");
            fs::write(fixture.directory.path().join("version"), version).unwrap();
            fs::write(
                fixture.directory.path().join("thinking-metadata"),
                metadata.to_string(),
            )
            .unwrap();
            let session = fixture.session().await;
            let result = fixture
                .adapter
                .start_turn(
                    &session,
                    TurnRequest::new("never")
                        .with_context_budget(ContextBudget::Tokens { tokens: 300000 }),
                )
                .await;
            assert!(matches!(
                result,
                Err(ProviderError::NotDispatched {
                    category:
                        prompting_time_core::providers::ProviderErrorCategory::UnsupportedContextBudget
                })
            ));
            assert!(!fixture.directory.path().join("prompt").exists());
        }
    }
}

#[tokio::test]
async fn context_budget_requires_exact_supported_version_before_launch() {
    use prompting_time_core::context_budget::ContextBudget;
    for version in [
        "2.1.204", "2.1.206", "2.1.285", "2.1.287", "2.2.0", "3.0.0", "invalid",
    ] {
        let fixture = Fixture::new("result()");
        fs::write(fixture.directory.path().join("version"), version).unwrap();
        let session = fixture.session().await;
        let result = fixture
            .adapter
            .start_turn(
                &session,
                TurnRequest::new("never")
                    .with_context_budget(ContextBudget::Tokens { tokens: 300000 }),
            )
            .await;
        assert!(matches!(
            result,
            Err(ProviderError::NotDispatched {
                category:
                    prompting_time_core::providers::ProviderErrorCategory::UnsupportedContextBudget
            })
        ));
        assert!(!fixture.directory.path().join("prompt").exists());
        assert!(!fixture.directory.path().join("initialize").exists());
        let mut turn = fixture
            .adapter
            .start_turn(&session, TurnRequest::new("default"))
            .await
            .unwrap();
        collect(&mut turn).await;
        turn.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn context_budget_inherited_environment_resets_on_resume() {
    use prompting_time_core::context_budget::ContextBudget;
    if std::env::var_os("PROMPTING_TIME_COMPACTION_ENV_FIXTURE").is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "context_budget_inherited_environment_resets_on_resume",
                "--nocapture",
            ])
            .env("PROMPTING_TIME_COMPACTION_ENV_FIXTURE", "1")
            .env("DISABLE_COMPACT", "1")
            .env("DISABLE_AUTO_COMPACT", "1")
            .env("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "170000")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let inherited = json!({"DISABLE_COMPACT":"1","DISABLE_AUTO_COMPACT":"1","CLAUDE_CODE_AUTO_COMPACT_WINDOW":"170000"});
    let fixture = Fixture::new("result()");
    let session = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(
            &session,
            TurnRequest::new("numeric")
                .with_context_budget(ContextBudget::Tokens { tokens: 300000 }),
        )
        .await
        .unwrap();
    collect(&mut turn).await;
    turn.shutdown().await.unwrap();
    assert_eq!(
        fixture.read("compaction-env"),
        json!({"DISABLE_COMPACT":"0","DISABLE_AUTO_COMPACT":"0","CLAUDE_CODE_AUTO_COMPACT_WINDOW":""})
    );
    let resumed = fixture.resume(&session).await;
    let mut turn = fixture
        .adapter
        .start_turn(&resumed, TurnRequest::new("default"))
        .await
        .unwrap();
    collect(&mut turn).await;
    turn.shutdown().await.unwrap();
    assert_eq!(fixture.read("compaction-env"), inherited);
    assert_eq!(std::env::var("DISABLE_COMPACT").unwrap(), "1");
    assert_eq!(std::env::var("DISABLE_AUTO_COMPACT").unwrap(), "1");
    assert_eq!(
        std::env::var("CLAUDE_CODE_AUTO_COMPACT_WINDOW").unwrap(),
        "170000"
    );
    assert!(
        fixture
            .read("args")
            .as_array()
            .unwrap()
            .contains(&json!(format!("--resume={}", session.native_id)))
    );
}

#[tokio::test]
async fn compaction_observations_preserve_native_boundary_and_clear_without_success() {
    use prompting_time_core::context_compaction::CompactionPhase;
    let fixture = Fixture::new(
        r#"
for event in [
    dict(subtype='compact_boundary', uuid='boundary-before-start'),
    dict(subtype='status', status='compacting', uuid='status-1'),
    dict(subtype='status', status='requesting', uuid='request-during-compaction'),
    dict(subtype='status', status='compacting', uuid='status-2'),
    dict(subtype='status', status=None, compact_result='success', uuid='status-3'),
    dict(subtype='compact_boundary', uuid='boundary-after-success'),
    dict(subtype='status', status='compacting', uuid='status-4'),
    dict(subtype='status', status=None, compact_result='failed', compact_error='PRIVATE_ERROR', uuid='status-5'),
    dict(subtype='status', status=None, uuid='status-6'),
    dict(subtype='status', status='requesting', uuid='request-after-compaction'),
    dict(subtype='compact_boundary', uuid='child-boundary', parent_tool_use_id='unproven-child')
]:
    emit(dict(type='system',session_id=session,**event))
result()
"#,
    );
    let session = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(&session, TurnRequest::new("invented"))
        .await
        .unwrap();
    let events = collect(&mut turn).await;
    assert!(events.iter().all(Result::is_ok));
    let observations: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Ok(ProviderEvent::Compaction { observation }) => Some(observation),
            _ => None,
        })
        .collect();
    assert_eq!(
        observations
            .iter()
            .map(|event| event.phase.clone())
            .collect::<Vec<_>>(),
        vec![
            CompactionPhase::Completed {
                boundary_id: "boundary-before-start".into()
            },
            CompactionPhase::Started,
            CompactionPhase::Started,
            CompactionPhase::Cleared,
            CompactionPhase::Completed {
                boundary_id: "boundary-after-success".into()
            },
            CompactionPhase::Started,
            CompactionPhase::Failed,
            CompactionPhase::Cleared,
        ]
    );
    assert!(
        observations
            .iter()
            .all(|event| event.native_session_id == session.native_id
                && event.native_turn_id.is_none()
                && event.native_agent_id.is_none())
    );
    assert!(!format!("{events:?}").contains("PRIVATE_ERROR"));
}

#[tokio::test]
async fn compaction_observations_require_bounded_matching_native_identity() {
    for patch in [
        json!({"session_id":null}),
        json!({"session_id":"wrong"}),
        json!({"uuid":null}),
        json!({"uuid":""}),
        json!({"uuid":"x".repeat(257)}),
        json!({"uuid":"bad\nidentity"}),
    ] {
        let body = format!(
            "event=dict(type='system',subtype='compact_boundary',session_id=session,uuid='boundary')\nevent.update(json.loads({:?}))\nemit(event)\nresult()",
            patch.to_string()
        );
        let fixture = Fixture::new(&body);
        let session = fixture.session().await;
        let mut turn = fixture
            .adapter
            .start_turn(&session, TurnRequest::new("invented"))
            .await
            .unwrap();
        let mut failed = false;
        while let Some(event) = turn.recv().await {
            match event {
                Err(_) => {
                    failed = true;
                    break;
                }
                Ok(ProviderEvent::Compaction { .. }) => {
                    panic!("invalid native identity must not publish compaction")
                }
                _ => {}
            }
        }
        assert!(failed, "invalid identity must fail closed: {patch}");
    }
}

#[tokio::test]
async fn thinking_environment_override_and_resume_reset() {
    // Exercise inheritance in an isolated test process; never mutate the test runner's environment.
    if std::env::var_os("PROMPTING_TIME_THINKING_ENV_FIXTURE").is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "thinking_environment_override_and_resume_reset",
                "--nocapture",
            ])
            .env("PROMPTING_TIME_THINKING_ENV_FIXTURE", "1")
            .env("CLAUDE_CODE_EFFORT_LEVEL", "high")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let fixture = Fixture::new("result()");
    let session = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(
            &session,
            thinking_request(ThinkingPreference::Manual {
                level: "low".into(),
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        turn.thinking().unwrap().configured_effort.as_deref(),
        Some("low")
    );
    collect(&mut turn).await;
    assert_eq!(fixture.read("effort-env"), "low");
    assert_eq!(std::env::var("CLAUDE_CODE_EFFORT_LEVEL").unwrap(), "high");
    let resumed = fixture.resume(&session).await;
    let mut turn = fixture
        .adapter
        .start_turn(&resumed, TurnRequest::new("Provider default again"))
        .await
        .unwrap();
    let configuration = turn.thinking().unwrap();
    assert_eq!(configuration.configured_effort.as_deref(), Some("high"));
    assert_eq!(configuration.model, "invented-model");
    assert!(
        !serde_json::to_string(configuration)
            .unwrap()
            .contains("MUST-NOT-RETAIN")
    );
    collect(&mut turn).await;
    assert_eq!(fixture.read("effort-env"), "high");
    let args = fixture.read("args");
    let args = args.as_array().unwrap();
    assert!(args.contains(&json!(format!("--resume={}", session.native_id))));
    assert!(args.contains(&json!("--setting-sources=")));
    assert!(args.contains(&json!("--strict-mcp-config")));
    assert!(
        args.windows(2)
            .any(|pair| pair == [json!("--mcp-config"), json!(r#"{"mcpServers":{}}"#)])
    );
    assert!(
        !args
            .iter()
            .any(|arg| arg.as_str().unwrap().starts_with("--effort"))
    );
}

#[tokio::test]
async fn thinking_manual_rejects_caps_unsupported_and_unknown_before_prompt() {
    for metadata in [
        json!({"applied":{"model":"invented-model","effort":"medium"}}),
        json!({"models":[{"value":"default","resolvedModel":"invented-model","supportsEffort":true,"supportedEffortLevels":["low","medium","high"]}],"applied":{"model":"invented-model","effort":"high"}}),
        json!({"models":[]}),
        json!({"applied":{"model":"different-active-model","effort":"max"}}),
        json!({"applied":{"model":"invented-model"}}),
        json!({"applied":{"model":"invented-model","effort":null}}),
        json!({"models":[{"value":"default","resolvedModel":"invented-model","supportsEffort":false}]}),
    ] {
        let fixture = Fixture::new("result()");
        fs::write(
            fixture.directory.path().join("thinking-metadata"),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
        let session = fixture.session().await;
        let result = fixture
            .adapter
            .start_turn(
                &session,
                thinking_request(ThinkingPreference::Manual {
                    level: "max".into(),
                }),
            )
            .await;
        assert!(
            matches!(
                result,
                Err(ProviderError::NotDispatched {
                    category:
                        prompting_time_core::providers::ProviderErrorCategory::UnsupportedThinking
                })
            ),
            "metadata: {metadata}"
        );
        assert!(!fixture.directory.path().join("prompt").exists());
        reaped(fixture.read("pid").as_u64().unwrap()).await;
        fs::remove_file(fixture.directory.path().join("thinking-metadata")).unwrap();
        let mut next = fixture
            .adapter
            .start_turn(&session, TurnRequest::new("first actual prompt"))
            .await
            .unwrap();
        collect(&mut next).await;
        assert!(
            fixture
                .read("args")
                .as_array()
                .unwrap()
                .contains(&json!(format!("--session-id={}", session.native_id)))
        );
    }
}

#[tokio::test]
async fn thinking_auto_discloses_cap_and_uses_active_model_metadata() {
    let fixture = Fixture::new("result()");
    fs::write(fixture.directory.path().join("thinking-metadata"), serde_json::to_vec(&json!({
        "models":[
            {"value":"default","resolvedModel":"other-default","supportsEffort":false},
            {"value":"chosen","resolvedModel":"invented-active","supportsEffort":true,"supportedEffortLevels":["low","medium","high"]},
            {"value":"invented-active","resolvedModel":"invented-active","supportsEffort":true,"supportedEffortLevels":["low","medium","high"]}
        ],
        "applied":{"model":"invented-active","effort":"medium"}
    })).unwrap()).unwrap();
    let session = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(&session, thinking_request(ThinkingPreference::Auto))
        .await
        .unwrap();
    let configuration = turn.thinking().unwrap();
    assert_eq!(configuration.model, "invented-active");
    assert_eq!(configuration.supported_efforts, ["low", "medium", "high"]);
    assert_eq!(configuration.configured_effort.as_deref(), Some("medium"));
    collect(&mut turn).await;
    assert_eq!(fixture.read("effort-env"), "high");
}

#[tokio::test]
async fn thinking_unknown_or_unbounded_configuration_fails_closed_for_defaults() {
    for metadata in [
        json!({"models":null}),
        json!({"models":[{"value":"default","resolvedModel":"invented-model","supportsEffort":true}]}),
        json!({"models":[{"resolvedModel":"invented-model","supportsEffort":true,"supportedEffortLevels":["high","high"]}]}),
        json!({"models":[{"resolvedModel":"invented-model","supportsEffort":true,"supportedEffortLevels":["high"]},{"resolvedModel":"invented-model","supportsEffort":true,"supportedEffortLevels":["low"]}]}),
        json!({"applied":{"model":"m".repeat(257),"effort":"high"}}),
        json!({"applied":{"model":"invented-model","effort":"e".repeat(65)}}),
        json!({"applied":{}}),
    ] {
        for preference in [
            ThinkingPreference::ProviderDefault,
            ThinkingPreference::Auto,
        ] {
            let fixture = Fixture::new("result()");
            fs::write(
                fixture.directory.path().join("thinking-metadata"),
                serde_json::to_vec(&metadata).unwrap(),
            )
            .unwrap();
            let session = fixture.session().await;
            assert!(matches!(
                fixture
                    .adapter
                    .start_turn(&session, thinking_request(preference))
                    .await,
                Err(ProviderError::NotDispatched {
                    category:
                        prompting_time_core::providers::ProviderErrorCategory::UnsupportedThinking
                })
            ));
            assert!(!fixture.directory.path().join("prompt").exists());
        }
    }
}

#[tokio::test]
async fn thinking_cancel_during_applied_settings_reaps_child_without_dispatch() {
    let fixture = Fixture::new("result()");
    fs::write(fixture.directory.path().join("hold-settings"), "").unwrap();
    let session = fixture.session().await;
    let ready = fixture.directory.path().join("settings.ready");
    {
        let start = fixture
            .adapter
            .start_turn(&session, thinking_request(ThinkingPreference::Auto));
        tokio::pin!(start);
        tokio::select! {
            _ = &mut start => panic!("must await native settings before prompt"),
            _ = wait_file(&ready) => {}
        }
    }
    reaped(fixture.read("pid").as_u64().unwrap()).await;
    assert!(!fixture.directory.path().join("prompt").exists());
    fs::remove_file(fixture.directory.path().join("hold-settings")).unwrap();
    let mut turn = fixture
        .adapter
        .start_turn(&session, TurnRequest::new("first actual prompt"))
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
                context_budget:
                    prompting_time_core::context_budget::ContextBudget::provider_default(),
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
async fn requesting_status_allows_streaming_without_compaction_or_early_completion() {
    let fixture = Fixture::new(
        r#"
emit(dict(type='system',subtype='status',status='requesting',session_id=session,uuid='request-1'))
def stream(event):
    emit(dict(type='stream_event',session_id=session,parent_tool_use_id=None,event=event))
stream(dict(type='message_start',message=dict(id='message-1')))
stream(dict(type='content_block_start',index=0,content_block=dict(type='text',text='')))
stream(dict(type='content_block_delta',index=0,delta=dict(type='text_delta',text='READY')))
stream(dict(type='content_block_stop',index=0))
barrier('before-result')
result()
"#,
    );
    let session = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(
            &session,
            TurnRequest::new("invented").with_context_budget(
                prompting_time_core::context_budget::ContextBudget::Tokens { tokens: 300_000 },
            ),
        )
        .await
        .unwrap();
    let events = async {
        assert!(matches!(event(&mut turn).await?, ProviderEvent::TurnStarted { .. }));
        assert!(matches!(event(&mut turn).await?, ProviderEvent::AssistantMessageDelta { content, .. } if content == "READY"));
        wait_file(&fixture.directory.path().join("before-result.ready")).await;
        fs::write(fixture.directory.path().join("before-result.release"), "").unwrap();
        Ok::<_, ProviderError>(collect(&mut turn).await)
    }
    .await;
    turn.shutdown().await.unwrap();
    assert!(matches!(
        events.unwrap().as_slice(),
        [Ok(ProviderEvent::TurnCompleted)]
    ));
    reaped(fixture.read("pid").as_u64().unwrap()).await;
}

#[tokio::test]
async fn requesting_status_does_not_hide_missing_or_failed_results() {
    for (ending, expected) in [
        ("", ProviderError::StreamClosed),
        (
            "emit(dict(type='result',session_id=session,subtype='error_during_execution',is_error=True))",
            ProviderError::Protocol {
                category: "claude-result-failed-or-deferred".into(),
            },
        ),
    ] {
        let fixture = Fixture::new(&format!(
            "emit(dict(type='system',subtype='status',status='requesting',session_id=session,uuid='request-1'))\n{ending}"
        ));
        let session = fixture.session().await;
        let mut turn = fixture
            .adapter
            .start_turn(&session, TurnRequest::new("invented"))
            .await
            .unwrap();
        let events = collect(&mut turn).await;
        turn.shutdown().await.unwrap();
        assert!(
            matches!(events.as_slice(), [Ok(ProviderEvent::TurnStarted { .. }), Err(error)] if error == &expected),
            "{events:?}"
        );
        reaped(fixture.read("pid").as_u64().unwrap()).await;
    }
}

#[tokio::test]
async fn requesting_status_keeps_identity_and_unknown_status_validation() {
    for patch in [
        json!({"session_id":"wrong-session"}),
        json!({"session_id":null}),
        json!({"uuid":""}),
        json!({"status":"unknown"}),
        json!({"status":42}),
    ] {
        let fixture = Fixture::new(&format!(
            "value=dict(type='system',subtype='status',status='requesting',session_id=session,uuid='request-1')\nvalue.update(json.loads({:?}))\nemit(value)\nresult()",
            patch.to_string()
        ));
        let session = fixture.session().await;
        let mut turn = fixture
            .adapter
            .start_turn(&session, TurnRequest::new("invented"))
            .await
            .unwrap();
        let events = collect(&mut turn).await;
        turn.shutdown().await.unwrap();
        assert!(
            matches!(
                events.as_slice(),
                [
                    Ok(ProviderEvent::TurnStarted { .. }),
                    Err(ProviderError::Protocol { .. })
                ]
            ),
            "{patch}: {events:?}"
        );
        reaped(fixture.read("pid").as_u64().unwrap()).await;
    }
}

#[tokio::test]
async fn native_system_advisories_preserve_streaming_and_wait_for_result() {
    for advisory in native_system_advisories() {
        let fixture = Fixture::new(&format!(
            r#"
notice = json.loads({advisory:?})
notice.update(type='system', session_id=session, uuid='notice-1')
emit(notice)
emit(dict(type='system',subtype='status',status='requesting',session_id=session,uuid='request-1'))
def stream(event):
    emit(dict(type='stream_event',session_id=session,parent_tool_use_id=None,event=event))
stream(dict(type='message_start',message=dict(id='message-1')))
stream(dict(type='content_block_start',index=0,content_block=dict(type='text',text='')))
stream(dict(type='content_block_delta',index=0,delta=dict(type='text_delta',text='READY')))
stream(dict(type='content_block_stop',index=0))
barrier('before-result')
result()
"#,
            advisory = advisory.to_string(),
        ));
        let session = fixture.session().await;
        let mut turn = fixture
            .adapter
            .start_turn(
                &session,
                TurnRequest::new("invented").with_context_budget(
                    prompting_time_core::context_budget::ContextBudget::Tokens { tokens: 300_000 },
                ),
            )
            .await
            .unwrap();
        let events = async {
            assert!(matches!(event(&mut turn).await?, ProviderEvent::TurnStarted { .. }));
            assert!(matches!(event(&mut turn).await?, ProviderEvent::AssistantMessageDelta { content, .. } if content == "READY"));
            wait_file(&fixture.directory.path().join("before-result.ready")).await;
            fs::write(fixture.directory.path().join("before-result.release"), "").unwrap();
            Ok::<_, ProviderError>(collect(&mut turn).await)
        }.await;
        turn.shutdown().await.unwrap();
        assert!(matches!(
            events.unwrap().as_slice(),
            [Ok(ProviderEvent::TurnCompleted)]
        ));
        reaped(fixture.read("pid").as_u64().unwrap()).await;
    }
}

// Shapes from the installed CLI's stream-json producers; no native payload is retained.
fn native_system_advisories() -> [Value; 3] {
    [
        json!({"subtype":"api_retry", "attempt":1, "max_retries":10,
            "retry_delay_ms":512.5, "error_status":null, "error":"unknown"}),
        json!({"subtype":"notification", "key":"invented-notice",
            "text":"MUST-NOT-RETAIN", "priority":"high", "color":"yellow", "timeout_ms":5000}),
        json!({"subtype":"memory_recall", "mode":"select",
            "memories":[{"path":"/private/example-memory.md", "scope":"personal"}]}),
    ]
}

#[tokio::test]
async fn native_system_advisories_do_not_hide_missing_or_failed_results() {
    for advisory in native_system_advisories() {
        for ending in [
            "",
            "emit(dict(type='result',session_id=session,subtype='error_during_execution',is_error=True))",
            "emit(dict(type='result',session_id=session,subtype='success',is_error=True))",
        ] {
            let fixture = Fixture::new(&format!(
                "notice=json.loads({:?})\nnotice.update(type='system',session_id=session,uuid='notice-1')\nemit(notice)\n{ending}",
                advisory.to_string()
            ));
            let session = fixture.session().await;
            let mut turn = fixture
                .adapter
                .start_turn(&session, TurnRequest::new("invented"))
                .await
                .unwrap();
            let events = collect(&mut turn).await;
            turn.shutdown().await.unwrap();
            let expected = if ending.is_empty() {
                ProviderError::StreamClosed
            } else {
                ProviderError::Protocol {
                    category: "claude-result-failed-or-deferred".into(),
                }
            };
            assert!(
                matches!(events.as_slice(), [Ok(ProviderEvent::TurnStarted { .. }), Err(error)] if error == &expected),
                "{advisory}: {events:?}"
            );
            reaped(fixture.read("pid").as_u64().unwrap()).await;
        }
    }
}

#[tokio::test]
async fn native_system_advisories_require_matching_identity() {
    for advisory in native_system_advisories() {
        for patch in [
            "notice.pop('session_id')",
            "notice['session_id']='wrong-session'",
            "notice['session_id']=None",
            "notice.pop('uuid')",
            "notice['uuid']=''",
            "notice['uuid']='invalid\\nidentity'",
        ] {
            let fixture = Fixture::new(&format!(
                "notice=json.loads({:?})\nnotice.update(type='system',session_id=session,uuid='notice-1')\n{patch}\nemit(notice)\nresult()",
                advisory.to_string()
            ));
            let session = fixture.session().await;
            let mut turn = fixture
                .adapter
                .start_turn(&session, TurnRequest::new("invented"))
                .await
                .unwrap();
            let events = collect(&mut turn).await;
            turn.shutdown().await.unwrap();
            assert!(
                matches!(events.as_slice(), [Ok(ProviderEvent::TurnStarted { .. }), Err(ProviderError::Protocol { category })] if category == "claude-invalid-identity" || category == "claude-session-mismatch"),
                "{advisory}, {patch}: {events:?}"
            );
            reaped(fixture.read("pid").as_u64().unwrap()).await;
        }
    }
}

#[tokio::test]
async fn unhandled_system_events_are_not_treated_as_advisories() {
    for (subtype, code) in [
        ("unknown-event", "claude-unsupported-system-envelope"),
        ("informational", "claude-unsupported-system-informational"),
        ("model_fallback", "claude-unsupported-system-model-fallback"),
        (
            "model_consent_fallback",
            "claude-unsupported-system-model-consent-fallback",
        ),
        (
            "model_refusal_fallback",
            "claude-unsupported-system-model-refusal-fallback",
        ),
        (
            "model_refusal_no_fallback",
            "claude-unsupported-system-model-refusal-no-fallback",
        ),
    ] {
        let fixture = Fixture::new(&format!(
            "emit(dict(type='system',subtype='{subtype}',session_id=session,uuid='notice-1',prevent_continuation=True))\nresult()"
        ));
        let session = fixture.session().await;
        let mut turn = fixture
            .adapter
            .start_turn(&session, TurnRequest::new("invented"))
            .await
            .unwrap();
        let events = collect(&mut turn).await;
        turn.shutdown().await.unwrap();
        assert!(
            matches!(events.as_slice(), [Ok(ProviderEvent::TurnStarted { .. }), Err(ProviderError::Protocol { category })] if category == code),
            "{subtype}: {events:?}"
        );
        let error = events.last().unwrap().as_ref().unwrap_err();
        assert_eq!(error.diagnostic_code().unwrap().as_str(), code);
        reaped(fixture.read("pid").as_u64().unwrap()).await;
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
        let session = adapter.start_session(StartSession {context_budget: prompting_time_core::context_budget::ContextBudget::provider_default(), conversation_id, working_directory:workspace.path().into()}).await?;
        let marker = format!("INVENTED-{}", uuid::Uuid::now_v7());
        for (prompt, expected) in [(format!("Remember this invented marker: {marker}. Reply only STORED. Do not use tools."), "STORED"), ("What was the invented marker? Reply with only that exact marker. Do not use tools.".into(), marker.as_str())] {
            let session = adapter.resume_session(&session.native_id, ResumeSession {context_budget: prompting_time_core::context_budget::ContextBudget::provider_default(), conversation_id, working_directory:workspace.path().into()}).await?;
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
            let session = adapter.start_session(StartSession {context_budget: prompting_time_core::context_budget::ContextBudget::provider_default(), conversation_id:ConversationId::new(), working_directory:workspace.path().into()}).await?;
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

// Exercises the same canonical APIs used by Tauri, without launching or controlling a native UI.
async fn live_app_turn(
    app: &PromptingTime,
    conversation_id: ConversationId,
    provider: ProviderId,
    prompt: &str,
    write: Option<(&Path, bool)>,
    allow_agents: bool,
) -> Result<(String, usize), Box<dyn std::error::Error>> {
    let mut changes = app.subscribe_changes();
    let submission = app
        .submit(app_request(conversation_id, prompt, Some(provider)))
        .await?;
    if submission.decision.provider != provider {
        return Err("explicit provider override was not honored".into());
    }
    let mut answered = std::collections::HashSet::new();
    let mut exact_writes = 0;
    let mut agent_approvals = 0;
    let outcome = loop {
        for approval in load_subtree_approvals(app, conversation_id, true).await? {
            if !answered.insert(approval.id) {
                continue;
            }
            let exact = if let Some((target, _)) = write {
                let detail = app.load_approval_detail(approval.id).await?;
                approval.provider == ProviderId::Claude
                    && approval.operation == "Write"
                    && Path::new(&approval.scope) == target
                    && !detail.truncated
                    && matches!(detail.details, Some(prompting_time_core::domain::ApprovalRequestDetails::FileChange { changes, .. }) if changes.len() == 1 && Path::new(&changes[0].path) == target)
            } else {
                false
            };
            let agent = allow_agents
                && approval.provider == ProviderId::Claude
                && approval.operation == "Agent"
                && agent_approvals < 2;
            agent_approvals += usize::from(agent);
            let allow =
                agent || (exact && exact_writes == 0 && write.is_some_and(|(_, allow)| allow));
            exact_writes += usize::from(exact);
            app.respond_to_approval_id(
                approval.id,
                if allow {
                    ApprovalResponse::Approved
                } else {
                    ApprovalResponse::Denied
                },
            )
            .await?;
        }
        tokio::select! {
            outcome = submission.handle.wait() => break outcome?,
            changed = changes.recv() => { changed?; },
        }
    };
    if outcome.status != RunStatus::Completed {
        let diagnostics = app
            .load_timeline_snapshot(conversation_id, None, 80)
            .await?
            .events
            .items
            .into_iter()
            .filter(|record| {
                record.event.kind == prompting_time_core::domain::TimelineEventKind::Diagnostic
            })
            .map(|record| record.event.content)
            .collect::<Vec<_>>();
        return Err(format!("app turn ended {:?}: {diagnostics:?}", outcome.status).into());
    }
    let text = app
        .load_timeline_snapshot(conversation_id, None, 80)
        .await?
        .events
        .items
        .into_iter()
        .filter(|record| {
            record.event.run_id == submission.handle.run_id()
                && record.event.role == Some(prompting_time_core::domain::MessageRole::Assistant)
        })
        .map(|record| record.event.content)
        .collect::<Vec<_>>()
        .join("");
    Ok((text, exact_writes))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "uses both installed provider accounts; coordinator runs explicitly"]
async fn live_app_projectless_repeated_turns_and_provider_switching() {
    assert_eq!(
        std::env::var("PROMPTING_TIME_LIVE_CLAUDE").as_deref(),
        Ok("1")
    );
    assert_eq!(
        std::env::var("PROMPTING_TIME_LIVE_CODEX").as_deref(),
        Ok("1")
    );
    let directory = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = Store::open(&directory.path().join("app.sqlite"))
        .await
        .unwrap();
    let codex =
        prompting_time_core::providers::codex::CodexAdapter::connect_with_initialization_timeout(
            "codex".into(),
            Duration::from_secs(10),
        )
        .await
        .unwrap();
    let app = PromptingTime::new(
        store.clone(),
        Router::default(),
        WorkspaceManager::new(directory.path()),
        vec![
            Arc::new(ClaudeAdapter::new("claude".into())),
            Arc::new(codex),
        ],
    )
    .unwrap();
    let marker = format!("INVENTED-{}", uuid::Uuid::now_v7());
    let result = timeout(Duration::from_secs(300), async {
        let conversation = app.create_conversation(ConversationRequest::projectless("Invented provider continuity")).await?;
        let mut outputs = Vec::new();
        outputs.push(live_app_turn(&app, conversation.id, ProviderId::Claude, &format!("Remember this invented marker: {marker}. Reply only STORED. Do not use tools."), None, false).await?.0);
        let original = store.load_provider_session(conversation.id, ProviderId::Claude).await?;
        for provider in [ProviderId::Claude, ProviderId::Codex, ProviderId::Claude] {
            outputs.push(live_app_turn(&app, conversation.id, provider, "What was the invented marker? Reply with only that exact marker. Use no tools.", None, false).await?.0);
        }
        let resumed = store.load_provider_session(conversation.id, ProviderId::Claude).await?;
        let audits = app.load_run_audits(conversation.id, None, 10).await?;
        Ok::<_, Box<dyn std::error::Error>>((outputs, original, resumed, audits))
    }).await;
    app.shutdown_with_grace(Duration::from_secs(5))
        .await
        .unwrap();
    let (outputs, original, resumed, audits) =
        result.expect("bounded app switching smoke").unwrap();
    assert_eq!(
        outputs.iter().map(|text| text.trim()).collect::<Vec<_>>(),
        ["STORED", &marker, &marker, &marker]
    );
    assert!(original.is_some());
    assert_eq!(original, resumed);
    assert_eq!(audits.items.len(), 4);
    assert_eq!(
        audits
            .items
            .iter()
            .filter(|run| run.provider == ProviderId::Codex)
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "uses the installed Claude account; coordinator runs explicitly"]
async fn live_app_canonical_approval_denies_and_allows_invented_write() {
    assert_eq!(
        std::env::var("PROMPTING_TIME_LIVE_CLAUDE").as_deref(),
        Ok("1")
    );
    for allow in [false, true] {
        let directory = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let store = Store::open(&directory.path().join("app.sqlite"))
            .await
            .unwrap();
        let app = PromptingTime::new(
            store.clone(),
            Router::default(),
            WorkspaceManager::new(directory.path()),
            vec![Arc::new(ClaudeAdapter::new("claude".into()))],
        )
        .unwrap();
        let result = timeout(Duration::from_secs(120), async {
            let conversation = app.create_conversation(ConversationRequest::projectless("Invented approval boundary")).await?;
            let target = store.load_workspace(conversation.id).await?.execution_path.canonicalize()?.join("app-probe.txt");
            let (_, requests) = live_app_turn(&app, conversation.id, ProviderId::Claude, &format!("Use the Write tool exactly once to write the exact text APP-PROBE to {}. Use no other tools. If permission is denied, do not retry. Then reply DONE.", target.display()), Some((&target, allow)), false).await?;
            Ok::<_, Box<dyn std::error::Error>>((target, requests))
        }).await;
        app.shutdown_with_grace(Duration::from_secs(5))
            .await
            .unwrap();
        let (target, requests) = result.expect("bounded app approval smoke").unwrap();
        assert_eq!(requests, 1);
        if allow {
            assert_eq!(fs::read_to_string(target).unwrap().trim(), "APP-PROBE");
        } else {
            assert!(!target.exists());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "uses the installed Claude account; coordinator runs explicitly"]
async fn live_app_recursive_claude_tasks_have_completed_canonical_ancestry() {
    assert_eq!(
        std::env::var("PROMPTING_TIME_LIVE_CLAUDE").as_deref(),
        Ok("1")
    );
    let directory = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = Store::open(&directory.path().join("app.sqlite"))
        .await
        .unwrap();
    let app = PromptingTime::new(
        store.clone(),
        Router::default(),
        WorkspaceManager::new(directory.path()),
        vec![Arc::new(ClaudeAdapter::new("claude".into()))],
    )
    .unwrap();
    let result = timeout(Duration::from_secs(180), async {
        let conversation = app.create_conversation(ConversationRequest::projectless("Invented recursive delegation")).await?;
        live_app_turn(&app, conversation.id, ProviderId::Claude,
            "Use Agent exactly once to launch an orchestrating child. Tell that child to use Agent exactly once to launch a grandchild which replies GRANDCHILD, then report its reply. This must be a depth-two delegation: root -> child -> grandchild. Do not launch the grandchild yourself. Use no tools other than Agent. Do not read or write files.",
            None, true).await?;
        Ok::<_, Box<dyn std::error::Error>>(store.load_agent_page(conversation.id, None, 20).await?)
    }).await;
    app.shutdown_with_grace(Duration::from_secs(5))
        .await
        .unwrap();
    let tree = result.expect("bounded recursive app smoke").unwrap();
    assert_eq!(tree.items.len(), 3);
    let root = tree.items.iter().find(|item| item.depth == 0).unwrap();
    let child = tree.items.iter().find(|item| item.depth == 1).unwrap();
    let grandchild = tree.items.iter().find(|item| item.depth == 2).unwrap();
    assert_eq!(child.agent.parent_id, Some(root.agent.id));
    assert_eq!(grandchild.agent.parent_id, Some(child.agent.id));
    assert!(
        tree.items
            .iter()
            .all(|item| item.agent.status == AgentStatus::Completed)
    );
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
async fn task_updates_wait_for_declared_identity_and_notification_terminals() {
    for tool_first in [false, true] {
        let started = "task('task_started','task-a','agent-a')";
        let patch = "emit(dict(type='system',subtype='task_updated',session_id=session,uuid='update-1',task_id='task-a',patch={'status':'completed','end_time':123}))";
        let tool = "assistant([{'type':'tool_use','id':'agent-a','name':'Agent','input':{}}])";
        let fixture = Fixture::new(&format!(
            "{started}\n{first}\n{second}\nresult()\nbarrier('before-notification')\ntask('task_notification','task-a','agent-a',status='completed')",
            first = if tool_first { tool } else { patch },
            second = if tool_first { patch } else { tool },
        ));
        let session = fixture.session().await;
        let mut turn = fixture
            .adapter
            .start_turn(&session, TurnRequest::new("invented updates"))
            .await
            .unwrap();
        assert!(matches!(
            event(&mut turn).await.unwrap(),
            ProviderEvent::TurnStarted { .. }
        ));
        assert!(matches!(
            event(&mut turn).await.unwrap(),
            ProviderEvent::NativeItemActivity { .. }
        ));
        assert!(
            matches!(event(&mut turn).await.unwrap(), ProviderEvent::ChildAgentActivity { child_statuses, .. } if child_statuses[0].status == NativeAgentStatus::Running)
        );
        wait_file(&fixture.directory.path().join("before-notification.ready")).await;
        assert!(
            timeout(Duration::from_millis(100), turn.recv())
                .await
                .is_err(),
            "patch and root result must not complete the child"
        );
        fs::write(
            fixture.directory.path().join("before-notification.release"),
            "",
        )
        .unwrap();
        let events = collect(&mut turn).await;
        assert!(
            matches!(events.as_slice(), [Ok(ProviderEvent::ChildAgentActivity { child_statuses, .. }), Ok(ProviderEvent::TurnCompleted)] if child_statuses[0].status == NativeAgentStatus::Completed),
            "{events:?}"
        );
        reaped(fixture.read("pid").as_u64().unwrap()).await;
    }
}

#[tokio::test]
async fn task_updates_validate_patches_and_never_supply_missing_terminal_evidence() {
    for (patch, finish, succeeds) in [
        (
            "{'description':'HIDDEN','error':'HIDDEN','is_backgrounded':False,'total_paused_ms':1}",
            "task('task_notification','task-a','agent-a',status='completed'); result()",
            true,
        ),
        (
            "{'status':'pending'}",
            "task('task_notification','task-a','agent-a',status='completed'); result()",
            true,
        ),
        (
            "{'status':'running'}",
            "task('task_notification','task-a','agent-a',status='completed'); result()",
            true,
        ),
        (
            "{'status':'paused'}",
            "task('task_notification','task-a','agent-a',status='completed'); result()",
            true,
        ),
        (
            "{'status':'failed'}",
            "task('task_notification','task-a','agent-a',status='failed'); result()",
            true,
        ),
        (
            "{'status':'killed'}",
            "task('task_notification','task-a','agent-a',status='stopped'); result()",
            true,
        ),
        ("{'status':'completed'}", "result()", false),
        (
            "{'status':'completed'}",
            "task('task_notification','task-a','agent-a',status='failed'); result()",
            false,
        ),
        ("{'status':'unknown'}", "result()", false),
        ("{'status':None}", "result()", false),
        ("{'new_lifecycle_field':True}", "result()", false),
        ("{'end_time':'invalid'}", "result()", false),
        ("{'description':False}", "result()", false),
        ("{'is_backgrounded':'invalid'}", "result()", false),
        ("[]", "result()", false),
    ] {
        let fixture = Fixture::new(&format!(
            "assistant([{{'type':'tool_use','id':'agent-a','name':'Agent','input':{{}}}}])\ntask('task_started','task-a','agent-a')\nemit(dict(type='system',subtype='task_updated',session_id=session,uuid='update-1',task_id='task-a',patch={patch}))\n{finish}"
        ));
        let session = fixture.session().await;
        let mut turn = fixture
            .adapter
            .start_turn(&session, TurnRequest::new("invented updates"))
            .await
            .unwrap();
        let events = collect(&mut turn).await;
        assert_eq!(
            events.iter().all(Result::is_ok),
            succeeds,
            "{patch}: {events:?}"
        );
        assert_eq!(
            matches!(events.last(), Some(Ok(ProviderEvent::TurnCompleted))),
            succeeds,
            "{events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Ok(ProviderEvent::AssistantMessageDelta { .. })))
        );
        reaped(fixture.read("pid").as_u64().unwrap()).await;
    }
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
        "emit(dict(type='system',subtype='task_updated',session_id=session,uuid='update-1',task_id='unknown-task',patch={'status':'completed'})); result()",
        "task('task_started','task-a','agent-a'); emit(dict(type='system',subtype='task_updated',session_id='wrong',uuid='update-1',task_id='task-a',patch={}))",
        "task('task_started','task-a','agent-a'); emit(dict(type='system',subtype='task_updated',session_id=session,task_id='task-a',patch={}))",
        "task('task_started','task-a','agent-a'); task('task_notification','task-a','agent-a',status='completed'); emit(dict(type='system',subtype='task_updated',session_id=session,uuid='update-1',task_id='task-a',patch={'status':'failed'}))",
        "task('task_started','task-a','agent-a'); [emit(dict(type='system',subtype='task_updated',session_id=session,uuid='update-'+status,task_id='task-a',patch={'status':status})) for status in ['completed','failed']]",
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
async fn tool_operation_stream_enrichment_survives_adapter_delivery() {
    let fixture = Fixture::new(
        r#"
emit({'type':'stream_event','session_id':session,'event':{'type':'message_start','message':{'id':'message'}}})
emit({'type':'stream_event','session_id':session,'event':{'type':'content_block_start','index':0,'content_block':{'type':'tool_use','id':'tool-1','name':'Bash','input':{}}}})
assistant([{'type':'tool_use','id':'tool-1','name':'Bash','input':{'command':'cargo test','private':'PRIVATE_INPUT'}}])
emit({'type':'user','session_id':session,'message':{'content':[{'type':'tool_result','tool_use_id':'tool-1','is_error':False,'content':'DONE'}]}})
result()
"#,
    );
    let session = fixture.session().await;
    let mut turn = fixture
        .adapter
        .start_turn(&session, TurnRequest::new("operation"))
        .await
        .unwrap();
    let events = collect(&mut turn).await;
    let snapshots: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Ok(ProviderEvent::NativeItemActivity {
                native_item_id,
                operation: Some(operation),
                ..
            }) => Some((native_item_id, operation)),
            _ => None,
        })
        .collect();
    assert_eq!(snapshots.len(), 3);
    assert!(snapshots.iter().all(|(id, _)| id.as_str() == "tool-1"));
    assert_eq!(snapshots[1].1.input.as_deref(), Some("cargo test"));
    assert_eq!(snapshots[2].1.output.as_deref(), Some("DONE"));
    assert_eq!(
        snapshots[2].1.status,
        prompting_time_core::tool_operation::ToolOperationStatus::Succeeded
    );
    assert!(matches!(
        events.last(),
        Some(Ok(ProviderEvent::TurnCompleted))
    ));
    assert!(!format!("{events:?}").contains("PRIVATE_INPUT"));
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
                    context_budget:
                        prompting_time_core::context_budget::ContextBudget::provider_default(),
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
                    context_budget:
                        prompting_time_core::context_budget::ContextBudget::provider_default(),
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
                context_budget:
                    prompting_time_core::context_budget::ContextBudget::provider_default(),
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
