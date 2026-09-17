use prompting_time_core::{
    domain::ConversationId,
    providers::{
        ProviderAdapter, ProviderError, ProviderErrorCategory, ResumeSession, StartSession,
        TurnRequest, codex::CodexAdapter,
    },
    thinking::{ThinkingDecision, ThinkingPreference},
};
use std::{fs, os::unix::fs::PermissionsExt};

fn fixture(mode: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("codex-thinking");
    let script = include_str!("fixtures/codex_thinking.py").replace("FIXTURE_MODE", mode);
    fs::write(&binary, script).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    (dir, binary)
}

fn request(preference: ThinkingPreference) -> TurnRequest {
    TurnRequest::new("debug invented issue")
        .with_thinking(ThinkingDecision::resolve(preference, "debug invented issue").unwrap())
}

#[tokio::test]
async fn codex_thinking_bounds_session_cache_and_resume_repopulates_evicted_metadata() {
    let (_dir, binary) = fixture("many-sessions");
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let mut sessions = Vec::new();
    for _ in 0..257 {
        sessions.push(
            adapter
                .start_session(StartSession {
                    context_budget:
                        prompting_time_core::context_budget::ContextBudget::provider_default(),
                    conversation_id: ConversationId::new(),
                    working_directory: "/tmp/invented-project".into(),
                })
                .await
                .unwrap(),
        );
    }
    let result = adapter
        .start_turn(&sessions[0], request(ThinkingPreference::ProviderDefault))
        .await;
    assert!(
        matches!(result, Err(ProviderError::NotDispatched { .. })),
        "old session metadata must be evicted"
    );
    let resumed = adapter
        .resume_session(
            &sessions[0].native_id,
            ResumeSession {
                context_budget:
                    prompting_time_core::context_budget::ContextBudget::provider_default(),
                conversation_id: ConversationId::new(),
                working_directory: "/tmp/invented-project".into(),
            },
        )
        .await
        .unwrap();
    for session in [&sessions[256], &resumed] {
        let mut turn = adapter
            .start_turn(session, request(ThinkingPreference::ProviderDefault))
            .await
            .unwrap();
        assert_eq!(
            turn.thinking().unwrap().configured_effort.as_deref(),
            Some("low")
        );
        while let Some(event) = turn.recv().await {
            if event.unwrap().is_terminal() {
                break;
            }
        }
        turn.shutdown().await.unwrap();
    }
    adapter.shutdown().await.unwrap();
}

#[tokio::test]
async fn codex_thinking_restores_workspace_default_after_manual_and_resume() {
    let (dir, binary) = fixture("normal");
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter
        .start_session(StartSession {
            context_budget: prompting_time_core::context_budget::ContextBudget::provider_default(),
            conversation_id: ConversationId::new(),
            working_directory: "/tmp/invented-project".into(),
        })
        .await
        .unwrap();
    for preference in [
        ThinkingPreference::Manual {
            level: "extra_native".into(),
        },
        ThinkingPreference::ProviderDefault,
        ThinkingPreference::Auto,
    ] {
        let mut turn = adapter
            .start_turn(&session, request(preference))
            .await
            .unwrap();
        let configuration = turn.thinking().unwrap();
        assert_eq!(configuration.model, "actual-hidden");
        assert_eq!(
            configuration.supported_efforts,
            ["low", "medium", "extra_native"]
        );
        while let Some(event) = turn.recv().await {
            if event.unwrap().is_terminal() {
                break;
            }
        }
        turn.shutdown().await.unwrap();
    }
    let resumed = adapter
        .resume_session(
            &session.native_id,
            ResumeSession {
                context_budget:
                    prompting_time_core::context_budget::ContextBudget::provider_default(),
                conversation_id: ConversationId::new(),
                working_directory: "/tmp/invented-project".into(),
            },
        )
        .await
        .unwrap();
    let mut turn = adapter
        .start_turn(&resumed, request(ThinkingPreference::ProviderDefault))
        .await
        .unwrap();
    assert_eq!(
        turn.thinking().unwrap().configured_effort.as_deref(),
        Some("low")
    );
    while let Some(event) = turn.recv().await {
        if event.unwrap().is_terminal() {
            break;
        }
    }
    turn.shutdown().await.unwrap();
    adapter.shutdown().await.unwrap();
    let log = fs::read_to_string(dir.path().join("requests.jsonl")).unwrap();
    let messages: Vec<serde_json::Value> = log
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let efforts: Vec<_> = messages
        .iter()
        .filter(|m| m["method"] == "turn/start")
        .map(|m| m["params"]["effort"].as_str().unwrap())
        .collect();
    assert_eq!(efforts, ["extra_native", "low", "medium", "low"]);
    assert!(
        messages
            .iter()
            .filter(|m| m["method"] == "model/list")
            .all(|m| m["params"]["includeHidden"] == true)
    );
    assert!(
        messages
            .iter()
            .filter(|m| m["method"] == "config/read")
            .all(|m| m["params"]["cwd"] == "/tmp/invented-project"
                && m["params"]["includeLayers"] == false)
    );
}

#[tokio::test]
async fn codex_thinking_rejects_unsupported_or_unknown_before_prompt() {
    for mode in [
        "normal",
        "missing-model",
        "missing-config",
        "repeat-cursor",
        "invalid-default",
        "unknown-model",
        "oversized-levels",
    ] {
        let (dir, binary) = fixture(mode);
        let adapter = CodexAdapter::connect(binary).await.unwrap();
        let session = adapter
            .start_session(StartSession {
                context_budget:
                    prompting_time_core::context_budget::ContextBudget::provider_default(),
                conversation_id: ConversationId::new(),
                working_directory: "/tmp/invented-project".into(),
            })
            .await
            .unwrap();
        let preference = if mode == "normal" {
            ThinkingPreference::Manual {
                level: "high".into(),
            }
        } else {
            ThinkingPreference::ProviderDefault
        };
        let result = adapter.start_turn(&session, request(preference)).await;
        assert!(
            matches!(
                result,
                Err(ProviderError::NotDispatched {
                    category: ProviderErrorCategory::UnsupportedThinking
                })
            ),
            "mode {mode} did not reject unsupported thinking"
        );
        adapter.shutdown().await.unwrap();
        let log = fs::read_to_string(dir.path().join("requests.jsonl")).unwrap();
        assert!(
            !log.contains("\"method\":\"turn/start\""),
            "mode {mode} dispatched prompt"
        );
    }
}

#[tokio::test]
async fn codex_thinking_null_config_uses_catalog_default() {
    let (_dir, binary) = fixture("null-config");
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter
        .start_session(StartSession {
            context_budget: prompting_time_core::context_budget::ContextBudget::provider_default(),
            conversation_id: ConversationId::new(),
            working_directory: "/tmp/invented-project".into(),
        })
        .await
        .unwrap();
    let mut turn = adapter
        .start_turn(&session, request(ThinkingPreference::ProviderDefault))
        .await
        .unwrap();
    assert_eq!(
        turn.thinking().unwrap().configured_effort.as_deref(),
        Some("medium")
    );
    while let Some(event) = turn.recv().await {
        if event.unwrap().is_terminal() {
            break;
        }
    }
    turn.shutdown().await.unwrap();
    adapter.shutdown().await.unwrap();
}

#[tokio::test]
async fn codex_thinking_schema_optional_unset_effort_uses_explicit_catalog_default() {
    let (_dir, binary) = fixture("unset-config");
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter
        .start_session(StartSession {
            context_budget: prompting_time_core::context_budget::ContextBudget::provider_default(),
            conversation_id: ConversationId::new(),
            working_directory: "/tmp/invented-project".into(),
        })
        .await
        .unwrap();
    let mut turn = adapter
        .start_turn(&session, request(ThinkingPreference::ProviderDefault))
        .await
        .unwrap();
    assert_eq!(
        turn.thinking().unwrap().configured_effort.as_deref(),
        Some("medium")
    );
    while let Some(event) = turn.recv().await {
        if event.unwrap().is_terminal() {
            break;
        }
    }
    turn.shutdown().await.unwrap();
    adapter.shutdown().await.unwrap();
}

#[tokio::test]
async fn codex_thinking_revalidates_changed_thread_model_before_manual_dispatch() {
    let (dir, binary) = fixture("changed-model");
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter
        .start_session(StartSession {
            context_budget: prompting_time_core::context_budget::ContextBudget::provider_default(),
            conversation_id: ConversationId::new(),
            working_directory: "/tmp/invented-project".into(),
        })
        .await
        .unwrap();
    let mut turn = adapter
        .start_turn(
            &session,
            request(ThinkingPreference::Manual {
                level: "extra_native".into(),
            }),
        )
        .await
        .unwrap();
    while let Some(event) = turn.recv().await {
        if event.unwrap().is_terminal() {
            break;
        }
    }
    turn.shutdown().await.unwrap();
    let result = adapter
        .start_turn(
            &session,
            request(ThinkingPreference::Manual {
                level: "extra_native".into(),
            }),
        )
        .await;
    assert!(matches!(result, Err(ProviderError::NotDispatched { .. })));
    adapter.shutdown().await.unwrap();
    let log = fs::read_to_string(dir.path().join("requests.jsonl")).unwrap();
    assert_eq!(
        log.lines()
            .filter(|line| line.contains("\"method\":\"turn/start\""))
            .count(),
        1
    );
}

#[tokio::test]
async fn codex_thinking_cancelled_discovery_never_dispatches_prompt_and_connection_reusable() {
    let (dir, binary) = fixture("stall-once");
    let adapter = CodexAdapter::connect(binary).await.unwrap();
    let session = adapter
        .start_session(StartSession {
            context_budget: prompting_time_core::context_budget::ContextBudget::provider_default(),
            conversation_id: ConversationId::new(),
            working_directory: "/tmp/invented-project".into(),
        })
        .await
        .unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        adapter.start_turn(&session, request(ThinkingPreference::Auto)),
    )
    .await;
    assert!(result.is_err());
    let mut turn = adapter
        .start_turn(&session, request(ThinkingPreference::ProviderDefault))
        .await
        .unwrap();
    while let Some(event) = turn.recv().await {
        if event.unwrap().is_terminal() {
            break;
        }
    }
    turn.shutdown().await.unwrap();
    adapter.shutdown().await.unwrap();
    let log = fs::read_to_string(dir.path().join("requests.jsonl")).unwrap();
    assert_eq!(
        log.lines()
            .filter(|line| line.contains("\"method\":\"turn/start\""))
            .count(),
        1
    );
}
