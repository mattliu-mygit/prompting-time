use std::sync::Arc;
use std::time::Duration;

use prompting_time_core::app::{ConversationRequest, PromptingTime, SubmitRequest};
use prompting_time_core::domain::{AgentStatus, RunStatus};
use prompting_time_core::providers::codex::CodexAdapter;
use prompting_time_core::providers::{ApprovalResponse, ProviderId};
use prompting_time_core::router::Router;
use prompting_time_core::store::Store;
use prompting_time_core::workspace::WorkspaceManager;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "uses the installed Codex CLI and authenticated account"]
async fn live_codex_app_native_child_approval_and_denial() {
    use prompting_time_core::domain::{
        ApprovalRequestDetails, ApprovalResponseIntentStatus, ApprovalStatus,
    };
    assert_eq!(
        std::env::var("PROMPTING_TIME_LIVE_CODEX").as_deref(),
        Ok("1")
    );
    let directory = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = Store::open(&directory.path().join("controls.sqlite"))
        .await
        .unwrap();
    let codex =
        CodexAdapter::connect_with_initialization_timeout("codex".into(), Duration::from_secs(10))
            .await
            .unwrap();
    let app = PromptingTime::new(
        store.clone(),
        Router::default(),
        WorkspaceManager::new(directory.path()),
        vec![Arc::new(codex)],
    )
    .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(300), async {
        let mut evidence = Vec::new();
        for allow in [false, true] {
            let target = directory.path().join(if allow { "allow.txt" } else { "deny.txt" });
            let command = format!("/usr/bin/printf SYNTHETIC_CHILD_CONTROL > {}", target.display());
            let expected = [command.clone(), format!("/bin/sh -c '{command}'"), format!("/bin/zsh -lc '{command}'"), format!("/bin/bash -lc '{command}'")];
            let conversation = app.create_conversation(ConversationRequest::projectless("Invented child control gate")).await?;
            let mut changes = app.subscribe_changes();
            let submission = app.submit(SubmitRequest {
                command_id: uuid::Uuid::now_v7().to_string(), conversation_id:conversation.id, provider_override:Some(ProviderId::Codex),
                content:format!("Use the subagent tool exactly once to create one child. Give the child this exact instruction: Use exec_command exactly once to run `{command}` with shell /bin/sh, login=false, sandbox_permissions=require_escalated, justification='Synthetic child approval test'. Request approval explicitly even if the command could run in the sandbox. This is a test of a single permission request; if denied, stop and report DENIED, never retry or use another tool. If allowed, report DONE. Do no other filesystem, shell, network, or tool operations. Root: wait for this child using only agent tools, then report ROOT. Do not run the command yourself or create more children."),
            }).await?;
            let mut answered = std::collections::HashSet::new();
            let mut exact = Vec::new();
            let outcome = loop {
                for approval in app.load_approvals(conversation.id,None,true,20).await?.items {
                    if !answered.insert(approval.id) { continue; }
                    let detail = app.load_approval_detail(approval.id).await?;
                    let tree = app.load_agent_page(conversation.id,None,20).await?;
                    let child = tree.items.iter().find(|node| node.agent.id == approval.agent_id && node.depth == 1);
                    let matches = child.is_some() && !detail.truncated && matches!(&detail.details,
                        Some(ApprovalRequestDetails::CommandExecution { command:Some(value), .. }) if expected.contains(value));
                    let durable = store.load_approval_by_id(approval.id).await?;
                    let owner = match durable.provider_request_id.as_deref() {
                        Some(request) => store.load_native_child_control_owner(approval.run_id,request).await?, None => None,
                    };
                    let owner_matches = owner.as_ref().is_some_and(|(agent, native)| *agent == approval.agent_id && !native.native_thread_id.is_empty() && !native.native_turn_id.is_empty());
                    let decision = allow && matches && owner_matches && exact.is_empty();
                    app.respond_to_approval_id(approval.id, if decision { ApprovalResponse::Approved } else { ApprovalResponse::Denied }).await?;
                    if matches && owner_matches { exact.push(approval.id); }
                }
                tokio::select! {
                    outcome = submission.handle.wait() => break outcome?,
                    changed = changes.recv() => { changed?; },
                }
            };
            let tree = app.load_agent_page(conversation.id,None,20).await?;
            let approvals = app.load_approvals(conversation.id,None,false,20).await?;
            let acknowledged = if let Some(id) = exact.first() {
                let approval = store.load_approval_by_id(*id).await?;
                approval.status == if allow { ApprovalStatus::Approved } else { ApprovalStatus::Denied }
                    && approval.response_intent.is_some_and(|intent| intent.status == ApprovalResponseIntentStatus::Acknowledged)
            } else { false };
            evidence.push((allow, outcome.status, exact.len(), answered.len(), approvals.items.len(), acknowledged, tree.items.iter().map(|node| (node.depth,node.agent.status)).collect::<Vec<_>>(), std::fs::read_to_string(&target).ok()));
        }
        Ok::<_,Box<dyn std::error::Error>>(evidence)
    }).await;
    let shutdown = app.shutdown_with_grace(Duration::from_secs(8)).await;
    shutdown.unwrap();
    for (allow, status, exact, answered, total, acknowledged, tree, contents) in
        result.expect("bounded child-control gate").unwrap()
    {
        println!(
            "allow={allow}; root_status={status:?}; exact_controls={exact}; controls={answered}/{total}; acknowledged={acknowledged}; canonical_depth_status={tree:?}; target_present={}",
            contents.is_some()
        );
        assert_eq!(status, RunStatus::Completed);
        assert_eq!(exact, 1);
        assert_eq!(answered, 1);
        assert_eq!(total, 1);
        assert!(acknowledged);
        assert_eq!(
            tree,
            vec![(0, AgentStatus::Completed), (1, AgentStatus::Completed)]
        );
        assert_eq!(
            contents,
            allow.then(|| "SYNTHETIC_CHILD_CONTROL".to_owned())
        );
    }
}

/// Canonical application API, real provider, disposable workspace. This checks
/// recursive ancestry and native terminal lifecycle; approvals have a separate gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "uses the installed Codex CLI and authenticated account"]
async fn live_codex_app_recursive_ancestry_and_activity_completion() {
    assert_eq!(
        std::env::var("PROMPTING_TIME_LIVE_CODEX").as_deref(),
        Ok("1")
    );
    let directory = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = Store::open(&directory.path().join("recursion.sqlite"))
        .await
        .unwrap();
    let codex =
        CodexAdapter::connect_with_initialization_timeout("codex".into(), Duration::from_secs(10))
            .await
            .unwrap();
    let app = PromptingTime::new(
        store.clone(),
        Router::default(),
        WorkspaceManager::new(directory.path()),
        vec![Arc::new(codex)],
    )
    .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(150), async {
        let conversation = app.create_conversation(ConversationRequest::projectless("Invented Codex recursion gate")).await?;
        let mut changes = app.subscribe_changes();
        let submission = app.submit(SubmitRequest {
            command_id: uuid::Uuid::now_v7().to_string(), conversation_id: conversation.id,
            provider_override: Some(ProviderId::Codex),
            content: "Use your subagent tool exactly once to create a child orchestrator. Tell the child to create exactly one grandchild using its subagent tool and wait for it. Grandchild must reply GRANDCHILD without tools. Child waits then replies CHILD. Wait for the child before replying ROOT. Use no filesystem, shell, network, or other tools. Do not create additional agents.".to_owned(),
        }).await?;
        let mut answered = std::collections::HashSet::new();
        let outcome = loop {
            for approval in app.load_approvals(conversation.id, None, true, 20).await?.items {
                if answered.insert(approval.id) {
                    app.respond_to_approval_id(approval.id, ApprovalResponse::Denied).await?;
                }
            }
            tokio::select! {
                outcome = submission.handle.wait() => break outcome?,
                changed = changes.recv() => { changed?; },
            }
        };
        let tree = app.load_agent_page(conversation.id, None, 20).await?;
        Ok::<_, Box<dyn std::error::Error>>((outcome.status, tree))
    }).await;
    // Assertions and panic paths run only after the owned provider has shut down.
    let shutdown = app.shutdown_with_grace(Duration::from_secs(5)).await;
    shutdown.unwrap();
    let (status, tree) = result.expect("bounded recursive application gate").unwrap();
    println!(
        "root_status={status:?}; canonical_depth_status={:?}",
        tree.items
            .iter()
            .map(|item| (item.depth, item.agent.status))
            .collect::<Vec<_>>()
    );
    assert_eq!(status, RunStatus::Completed);
    assert_eq!(
        tree.items.len(),
        3,
        "exactly one root, child, and grandchild"
    );
    assert!(tree.next_cursor.is_none());
    for depth in 0..=2 {
        let node = tree
            .items
            .iter()
            .find(|item| item.depth == depth)
            .expect("complete ancestry");
        assert_eq!(node.agent.status, AgentStatus::Completed);
        if depth == 0 {
            assert!(node.agent.parent_id.is_none());
        } else {
            assert_eq!(
                node.agent.parent_id,
                Some(
                    tree.items
                        .iter()
                        .find(|item| item.depth == depth - 1)
                        .unwrap()
                        .agent
                        .id
                )
            );
        }
    }
}
