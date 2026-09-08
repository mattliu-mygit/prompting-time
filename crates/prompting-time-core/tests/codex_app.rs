use std::sync::Arc;
use std::time::Duration;

use prompting_time_core::app::{ConversationRequest, PromptingTime, SubmitRequest};
use prompting_time_core::domain::{AgentStatus, RunStatus};
use prompting_time_core::providers::codex::CodexAdapter;
use prompting_time_core::providers::{ApprovalResponse, ProviderId};
use prompting_time_core::router::Router;
use prompting_time_core::store::Store;
use prompting_time_core::workspace::WorkspaceManager;

/// Canonical application API, real provider, disposable workspace. This checks
/// activity-driven ancestry; native descendant turns and approvals have a separate gate.
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
