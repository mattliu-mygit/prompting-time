use prompting_time_core::{
    context_budget::ContextBudget,
    domain::ConversationId,
    providers::{
        ProviderAdapter, ProviderError, ResumeSession, StartSession, TurnRequest,
        codex::CodexAdapter,
    },
};
use serde_json::Value;
use std::{fs, os::unix::fs::PermissionsExt};

fn fixture(mode: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let user_agent = if mode == "unsupported" {
        "codex-cli 0.999.0"
    } else {
        "prompting_time/0.153.4 (Mac OS 15.0.0; arm64)"
    };
    fixture_with_user_agent(mode, Some(user_agent))
}
fn fixture_with_user_agent(
    mode: &str,
    user_agent: Option<&str>,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("codex-compaction");
    fs::write(
        dir.path().join("user_agent.json"),
        serde_json::to_vec(&user_agent).unwrap(),
    )
    .unwrap();
    fs::write(
        &binary,
        include_str!("fixtures/codex_compaction.py").replace("FIXTURE_MODE", mode),
    )
    .unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    (dir, binary)
}
fn start(dir: &std::path::Path, budget: ContextBudget) -> StartSession {
    StartSession {
        conversation_id: ConversationId::new(),
        working_directory: dir.into(),
        context_budget: budget,
    }
}
fn logs(dir: &std::path::Path) -> Vec<Value> {
    fs::read_to_string(dir.join("requests.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
const NUMERIC: ContextBudget = ContextBudget::Tokens { tokens: 300_000 };

#[tokio::test]
async fn initial_budget_and_loaded_change_reset_preserve_identity_and_other_session() {
    let (dir, binary) = fixture("normal");
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter
        .start_session(start(dir.path(), NUMERIC))
        .await
        .unwrap();
    let other = adapter
        .start_session(start(dir.path(), ContextBudget::ProviderDefault))
        .await
        .unwrap();
    for budget in [
        NUMERIC,
        ContextBudget::Tokens { tokens: 500_000 },
        ContextBudget::ProviderDefault,
    ] {
        let resumed = adapter
            .resume_session(
                &session.native_id,
                ResumeSession {
                    conversation_id: ConversationId::new(),
                    working_directory: dir.path().into(),
                    context_budget: budget,
                },
            )
            .await
            .unwrap();
        assert_eq!(resumed.native_id, session.native_id);
        let mut turn = adapter
            .start_turn(
                &resumed,
                TurnRequest::new("invented").with_context_budget(budget),
            )
            .await
            .unwrap();
        while let Some(event) = turn.recv().await {
            if event.unwrap().is_terminal() {
                break;
            }
        }
        turn.shutdown().await.unwrap();
    }
    let mut turn = adapter
        .start_turn(&other, TurnRequest::new("other"))
        .await
        .unwrap();
    while let Some(event) = turn.recv().await {
        if event.unwrap().is_terminal() {
            break;
        }
    }
    turn.shutdown().await.unwrap();
    adapter.shutdown().await.unwrap();
    let rows = logs(dir.path());
    let starts: Vec<_> = rows
        .iter()
        .filter(|r| r["method"] == "thread/start")
        .collect();
    assert_eq!(
        starts[0]["params"]["config"],
        serde_json::json!({"model_auto_compact_token_limit":300000})
    );
    assert!(starts[1]["params"].get("config").is_none());
    let reloads: Vec<_> = rows
        .iter()
        .filter(|r| r["method"] == "thread/resume" && r["params"].get("config").is_some())
        .collect();
    assert_eq!(reloads.len(), 2);
    assert_eq!(
        reloads[0]["params"]["config"],
        serde_json::json!({"model_auto_compact_token_limit":500000})
    );
    assert_eq!(reloads[1]["params"]["config"], serde_json::json!({}));
    assert!(
        reloads
            .iter()
            .all(|r| r["params"]["threadId"] == session.native_id)
    );
    assert_eq!(
        rows.iter().filter(|r| r["method"] == "turn/start").count(),
        4
    );
}

#[tokio::test]
async fn uncertain_loaded_configuration_never_dispatches_prompt() {
    for mode in ["ignore", "wrong-owner", "reject", "active"] {
        let (dir, binary) = fixture(mode);
        let adapter = CodexAdapter::connect(binary).await.unwrap();
        let session = adapter
            .start_session(start(dir.path(), ContextBudget::ProviderDefault))
            .await
            .unwrap();
        let result = adapter
            .start_turn(
                &session,
                TurnRequest::new("never").with_context_budget(NUMERIC),
            )
            .await;
        assert!(
            matches!(result, Err(ProviderError::NotDispatched { .. })),
            "{mode}"
        );
        adapter.shutdown().await.unwrap();
        assert!(
            !logs(dir.path()).iter().any(|r| r["method"] == "turn/start"),
            "{mode}"
        );
    }
}

#[tokio::test]
async fn unsupported_codex_version_rejects_number_before_session_creation() {
    let (dir, binary) = fixture("unsupported");
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    assert!(
        adapter
            .start_session(start(dir.path(), NUMERIC))
            .await
            .is_err()
    );
    adapter.shutdown().await.unwrap();
    assert!(
        !logs(dir.path())
            .iter()
            .any(|r| r["method"] == "thread/start")
    );
}

#[tokio::test]
async fn exact_initialize_user_agents_gate_numeric_budget_before_dispatch() {
    let prefixes = [
        "codex-cli ",
        "codex_cli_rs/",
        "prompting_time/",
        "Codex Desktop/",
    ];
    for prefix in prefixes {
        let user_agent = format!("{prefix}0.153.4 (Mac OS 15.0.0; arm64)");
        let (dir, binary) = fixture_with_user_agent("normal", Some(&user_agent));
        let adapter = CodexAdapter::connect(binary).await.unwrap();
        adapter
            .start_session(start(dir.path(), NUMERIC))
            .await
            .unwrap();
        adapter.shutdown().await.unwrap();
        let starts: Vec<_> = logs(dir.path())
            .into_iter()
            .filter(|row| row["method"] == "thread/start")
            .collect();
        assert_eq!(starts.len(), 1, "{user_agent}");
        assert_eq!(
            starts[0]["params"]["config"],
            serde_json::json!({"model_auto_compact_token_limit":300000}),
            "{user_agent}"
        );
    }

    let mut rejected = Vec::new();
    for prefix in prefixes {
        for version in ["0.153.3", "0.153.5", "0.153.40", "0.153.4-rc.1"] {
            rejected.push(Some(format!("{prefix}{version} (Mac OS 15.0.0; arm64)")));
        }
    }
    rejected.extend([
        Some(String::new()),
        None,
        Some("other-product/0.153.4 (Mac OS 15.0.0; arm64)".into()),
    ]);
    for user_agent in rejected {
        let (dir, binary) = fixture_with_user_agent("normal", user_agent.as_deref());
        let adapter = CodexAdapter::connect(binary).await.unwrap();
        assert!(matches!(
            adapter.start_session(start(dir.path(), NUMERIC)).await,
            Err(ProviderError::NotDispatched {
                category: prompting_time_core::providers::ProviderErrorCategory::UnsupportedContextBudget
            })
        ), "{user_agent:?}");
        assert!(
            !logs(dir.path())
                .iter()
                .any(|row| row["method"] == "thread/start" || row["method"] == "turn/start"),
            "{user_agent:?}"
        );
        adapter
            .start_session(start(dir.path(), ContextBudget::ProviderDefault))
            .await
            .unwrap();
        adapter.shutdown().await.unwrap();
        let starts: Vec<_> = logs(dir.path())
            .into_iter()
            .filter(|row| row["method"] == "thread/start")
            .collect();
        assert_eq!(starts.len(), 1, "{user_agent:?}");
        assert!(
            starts[0]["params"].get("config").is_none(),
            "{user_agent:?}"
        );
    }

    let (dir, binary) = fixture_with_user_agent("metadata-version", Some("codex-cli 0.153.3"));
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    assert!(matches!(
        adapter.start_session(start(dir.path(), NUMERIC)).await,
        Err(ProviderError::NotDispatched {
            category:
                prompting_time_core::providers::ProviderErrorCategory::UnsupportedContextBudget
        })
    ));
    adapter.shutdown().await.unwrap();
    assert!(
        !logs(dir.path())
            .iter()
            .any(|row| row["method"] == "thread/start" || row["method"] == "turn/start")
    );
}

#[tokio::test]
async fn cancelled_configuration_cannot_reuse_an_unverified_budget() {
    let (dir, binary) = fixture("cancel-once");
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter
        .start_session(start(dir.path(), ContextBudget::ProviderDefault))
        .await
        .unwrap();
    let pending_adapter = adapter.clone();
    let pending_session = session.clone();
    let pending = tokio::spawn(async move {
        pending_adapter
            .start_turn(
                &pending_session,
                TurnRequest::new("cancel").with_context_budget(NUMERIC),
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !dir.path().join("reload.ready").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        adapter
            .start_turn(
                &session,
                TurnRequest::new("overlap")
                    .with_context_budget(ContextBudget::Tokens { tokens: 500_000 })
            )
            .await,
        Err(ProviderError::NotDispatched { .. })
    ));
    pending.abort();
    let _ = pending.await;
    fs::write(dir.path().join("reload.release"), b"release").unwrap();
    let mut turn = adapter
        .start_turn(
            &session,
            TurnRequest::new("retry explicit").with_context_budget(NUMERIC),
        )
        .await
        .unwrap();
    while let Some(event) = turn.recv().await {
        if event.unwrap().is_terminal() {
            break;
        }
    }
    turn.shutdown().await.unwrap();
    adapter.shutdown().await.unwrap();
    let rows = logs(dir.path());
    assert_eq!(
        rows.iter()
            .filter(|r| r["method"] == "thread/resume" && r["params"].get("config").is_some())
            .count(),
        2
    );
    assert_eq!(
        rows.iter().filter(|r| r["method"] == "turn/start").count(),
        1
    );
}

#[tokio::test]
async fn active_root_or_live_descendant_prevents_reconfiguration() {
    for mode in ["hold-root", "live-child"] {
        let (dir, binary) = fixture(mode);
        let adapter = CodexAdapter::connect(binary).await.unwrap();
        let session = adapter
            .start_session(start(dir.path(), ContextBudget::ProviderDefault))
            .await
            .unwrap();
        let mut turn = adapter
            .start_turn(&session, TurnRequest::new("running"))
            .await
            .unwrap();
        if mode == "live-child" {
            while let Some(event) = turn.recv().await {
                if event.unwrap().is_terminal() {
                    break;
                }
            }
        }
        assert!(matches!(
            adapter
                .start_turn(
                    &session,
                    TurnRequest::new("blocked").with_context_budget(NUMERIC)
                )
                .await,
            Err(ProviderError::NotDispatched { .. })
        ));
        assert!(
            !logs(dir.path())
                .iter()
                .any(|r| r["method"] == "thread/unsubscribe")
        );
        adapter.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn rejected_initial_configuration_is_not_dispatched() {
    let (dir, binary) = fixture("reject-start");
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let result = adapter.start_session(start(dir.path(), NUMERIC)).await;
    assert!(matches!(result, Err(ProviderError::NotDispatched { .. })));
    adapter.shutdown().await.unwrap();
    assert!(!logs(dir.path()).iter().any(|r| r["method"] == "turn/start"));
}

#[tokio::test]
async fn configuring_one_thread_preserves_another_active_turn() {
    let (dir, binary) = fixture("other-active");
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter
        .start_session(start(dir.path(), ContextBudget::ProviderDefault))
        .await
        .unwrap();
    let other = adapter
        .start_session(start(dir.path(), ContextBudget::ProviderDefault))
        .await
        .unwrap();
    let _active = adapter
        .start_turn(&other, TurnRequest::new("active"))
        .await
        .unwrap();
    let mut turn = adapter
        .start_turn(
            &session,
            TurnRequest::new("target").with_context_budget(NUMERIC),
        )
        .await
        .unwrap();
    while let Some(event) = turn.recv().await {
        if event.unwrap().is_terminal() {
            break;
        }
    }
    turn.shutdown().await.unwrap();
    adapter.shutdown().await.unwrap();
    let rows = logs(dir.path());
    assert_eq!(
        rows.iter().filter(|r| r["method"] == "initialize").count(),
        1
    );
    assert_eq!(
        rows.iter().filter(|r| r["method"] == "turn/start").count(),
        2
    );
    assert!(
        !rows
            .iter()
            .any(|r| r["method"] == "thread/unsubscribe"
                && r["params"]["threadId"] == other.native_id)
    );
}

#[tokio::test]
async fn native_compaction_lifecycle_keeps_root_and_child_identity() {
    use prompting_time_core::{context_compaction::CompactionPhase, providers::ProviderEvent};
    let (dir, binary) = fixture("compaction-events");
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter
        .start_session(start(dir.path(), ContextBudget::ProviderDefault))
        .await
        .unwrap();
    let mut turn = adapter
        .start_turn(&session, TurnRequest::new("invented"))
        .await
        .unwrap();
    let mut observations = Vec::new();
    let mut deprecated_notices = 0;
    while let Some(event) = turn.recv().await {
        let event = event.unwrap();
        if matches!(&event,ProviderEvent::Unrecognized {method} if method=="thread/compacted") {
            deprecated_notices += 1;
        }
        if let ProviderEvent::Compaction { observation } = &event {
            observations.push(observation.clone());
        }
        if event.is_terminal() {
            break;
        }
    }
    turn.shutdown().await.unwrap();
    adapter.shutdown().await.unwrap();
    assert_eq!(
        deprecated_notices, 0,
        "canonical items supersede deprecated telemetry"
    );
    assert_eq!(
        observations.len(),
        4,
        "root and child start/end; stale and deprecated never complete"
    );
    assert_eq!(observations[0].native_session_id, session.native_id);
    assert!(observations[0].native_agent_id.is_none());
    assert_eq!(observations[0].phase, CompactionPhase::Started);
    assert_eq!(
        observations[1].phase,
        CompactionPhase::Completed {
            boundary_id: serde_json::to_string(&["turn-1", "boundary"]).unwrap()
        }
    );
    assert_eq!(observations[2].native_session_id, "child");
    assert_eq!(observations[2].native_agent_id.as_deref(), Some("child"));
    assert_eq!(
        observations[2].native_turn_id.as_deref(),
        Some("child-turn")
    );
}
