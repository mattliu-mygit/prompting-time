use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use prompting_time_core::app::{
    AppError, ApprovalDetail as CoreApprovalDetail, ConversationOverview,
    ConversationRequest as CoreConversationRequest, ConversationWorkspace,
    InspectorSnapshot as CoreInspectorSnapshot, SubmitRequest as CoreSubmitRequest,
    TimelineSnapshot as CoreTimelineSnapshot,
};
use prompting_time_core::domain::{
    AgentNode, AgentStatus as CoreAgentStatus, ApprovalId,
    ApprovalRequestDetails as CoreApprovalRequestDetails, ApprovalStatus as CoreApprovalStatus,
    ConversationId, FileChangeKind as CoreFileChangeKind, MessageRole as CoreMessageRole,
    RequestedFileSystemAccess as CoreFileSystemAccess,
    RequestedFileSystemPath as CoreFileSystemPath, RequestedSpecialPath as CoreSpecialPath,
    RollupStatus as CoreRollupStatus, RunId, RunStatus as CoreRunStatus, TimelineEventId,
    TimelineEventKind as CoreTimelineEventKind,
};
use prompting_time_core::handoff::HandoffError;
use prompting_time_core::providers::{
    ApprovalResponse as CoreApprovalResponse, ProviderId as CoreProviderId,
};
use prompting_time_core::router::{
    ProviderCapability as CoreProviderCapability, ProviderEvaluation as CoreProviderEvaluation,
    ProviderRank as CoreProviderRank, ProviderUnavailability as CoreProviderUnavailability,
    RoutingBlocker as CoreRoutingBlocker, RoutingCriterion as CoreRoutingCriterion, RoutingError,
    RoutingProfile as CoreRoutingProfile, RoutingReason as CoreRoutingReason,
    TaskKind as CoreTaskKind,
};
use prompting_time_core::runtime::RuntimeError;
use prompting_time_core::store::{
    EventDetail as CoreEventDetail, Page, StoreError, TimelineRecord,
};
use prompting_time_core::workspace::{
    CleanupEligibility as CoreCleanupEligibility, WorkspaceBlocker as CoreWorkspaceBlocker,
    WorkspaceChangeKind as CoreWorkspaceChangeKind, WorkspaceError,
    WorkspaceMode as CoreWorkspaceMode, WorkspaceSnapshot as CoreWorkspaceSnapshot,
};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;
use uuid::Uuid;

use crate::state::{AppState, ProviderDiagnostic as StateProviderDiagnostic, StateError};

mod conversions;
mod dto;

pub use dto::*;

pub const APP_EVENT_NAME: &str = "prompting-time://app-event";

#[tauri::command]
#[specta::specta]
pub async fn bootstrap(state: State<'_, Arc<AppState>>) -> Result<BootstrapSnapshot, CommandError> {
    Ok(BootstrapSnapshot {
        providers: state.providers().iter().map(Into::into).collect(),
        startup_diagnostic: state.startup_diagnostic().map(|diagnostic| CommandError {
            code: diagnostic.code,
            message: diagnostic.message.clone(),
            action: diagnostic.action.clone(),
        }),
    })
}

#[tauri::command]
#[specta::specta]
pub async fn list_conversations(
    state: State<'_, Arc<AppState>>,
    request: ListConversationsRequest,
) -> Result<ConversationPage, CommandError> {
    let page = state
        .service()?
        .list_conversation_overviews(request.cursor, request.limit)
        .await?;
    Ok(page.into())
}

#[tauri::command]
#[specta::specta]
pub async fn load_conversation(
    state: State<'_, Arc<AppState>>,
    request: LoadConversationRequest,
) -> Result<ConversationSummary, CommandError> {
    Ok(state
        .service()?
        .load_conversation_overview(parse_conversation_id(&request.conversation_id)?)
        .await?
        .into())
}

#[tauri::command]
#[specta::specta]
pub async fn load_timeline(
    state: State<'_, Arc<AppState>>,
    request: LoadTimelineRequest,
) -> Result<TimelinePage, CommandError> {
    let conversation_id = parse_conversation_id(&request.conversation_id)?;
    let page = state
        .service()?
        .load_timeline_snapshot(conversation_id, request.cursor, request.limit)
        .await?;
    Ok(page.into())
}

#[tauri::command]
#[specta::specta]
pub async fn load_agent_tree(
    state: State<'_, Arc<AppState>>,
    request: LoadAgentTreeRequest,
) -> Result<AgentTreePage, CommandError> {
    Ok(state
        .service()?
        .load_agent_page(
            parse_conversation_id(&request.conversation_id)?,
            request.cursor,
            request.limit,
        )
        .await?
        .into())
}

#[tauri::command]
#[specta::specta]
pub async fn load_event_detail(
    state: State<'_, Arc<AppState>>,
    request: LoadEventDetailRequest,
) -> Result<EventDetailSnapshot, CommandError> {
    Ok(state
        .service()?
        .load_event_detail(parse_timeline_event_id(&request.event_id)?)
        .await?
        .into())
}

#[tauri::command]
#[specta::specta]
pub async fn load_approvals(
    state: State<'_, Arc<AppState>>,
    request: LoadApprovalsRequest,
) -> Result<ApprovalPage, CommandError> {
    Ok(state
        .service()?
        .load_approvals(
            parse_conversation_id(&request.conversation_id)?,
            request.cursor,
            matches!(request.kind, ApprovalListKind::Pending),
            request.limit,
        )
        .await?
        .into())
}

#[tauri::command]
#[specta::specta]
pub async fn load_approval_detail(
    state: State<'_, Arc<AppState>>,
    request: LoadApprovalDetailRequest,
) -> Result<ApprovalDetailSnapshot, CommandError> {
    Ok(ApprovalDetailSnapshot::from_core(
        state
            .service()?
            .load_approval_detail(parse_approval_id(&request.approval_id)?)
            .await?,
    ))
}

#[tauri::command]
#[specta::specta]
pub async fn load_approval_questions(
    state: State<'_, Arc<AppState>>,
    request: LoadApprovalQuestionsRequest,
) -> Result<ApprovalQuestionPage, CommandError> {
    Ok(state
        .service()?
        .load_approval_questions(
            parse_approval_id(&request.approval_id)?,
            request.cursor,
            request.limit,
        )
        .await?
        .into())
}

#[tauri::command]
#[specta::specta]
pub async fn create_conversation(
    state: State<'_, Arc<AppState>>,
    request: CreateConversationRequest,
) -> Result<ConversationSummary, CommandError> {
    let conversation = state
        .service()?
        .create_conversation_overview(request.into())
        .await?;
    Ok(conversation.into())
}

#[tauri::command]
#[specta::specta]
pub async fn submit_message(
    state: State<'_, Arc<AppState>>,
    request: SubmitMessageRequest,
) -> Result<SubmissionSnapshot, CommandError> {
    let conversation_id = parse_conversation_id(&request.conversation_id)?;
    let submission = state
        .service()?
        .submit(CoreSubmitRequest {
            command_id: request.command_id,
            conversation_id,
            content: request.text,
            provider_override: request.provider_override.map(Into::into),
        })
        .await?;
    let snapshot = SubmissionSnapshot {
        run_id: submission.handle.run_id().to_string(),
        status: submission.handle.status().into(),
        provider: submission.decision.provider.into(),
        duplicate: submission.duplicate,
        routing_explanation: submission.decision.explanation,
    };
    Ok(snapshot)
}

#[tauri::command]
#[specta::specta]
pub async fn steer_run(
    state: State<'_, Arc<AppState>>,
    request: SteerRunRequest,
) -> Result<(), CommandError> {
    state
        .service()?
        .steer(parse_run_id(&request.run_id)?, &request.text)
        .await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn respond_to_approval(
    state: State<'_, Arc<AppState>>,
    request: RespondToApprovalRequest,
) -> Result<(), CommandError> {
    state
        .service()?
        .respond_to_approval_id(
            parse_approval_id(&request.approval_id)?,
            request.response.into(),
        )
        .await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn interrupt_run(
    state: State<'_, Arc<AppState>>,
    request: InterruptRunRequest,
) -> Result<(), CommandError> {
    state
        .service()?
        .interrupt(parse_run_id(&request.run_id)?)
        .await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn archive_conversation(
    state: State<'_, Arc<AppState>>,
    request: ArchiveConversationRequest,
) -> Result<(), CommandError> {
    state
        .service()?
        .archive(parse_conversation_id(&request.conversation_id)?)
        .await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn inspect_workspace(
    state: State<'_, Arc<AppState>>,
    request: InspectWorkspaceRequest,
) -> Result<InspectorSnapshot, CommandError> {
    let snapshot = state
        .service()?
        .inspect_conversation(parse_conversation_id(&request.conversation_id)?)
        .await?;
    Ok(snapshot.into())
}

pub fn binding_builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new()
        .commands(tauri_specta::collect_commands![
            bootstrap,
            list_conversations,
            load_conversation,
            load_timeline,
            load_agent_tree,
            load_event_detail,
            load_approvals,
            load_approval_detail,
            load_approval_questions,
            create_conversation,
            submit_message,
            steer_run,
            respond_to_approval,
            interrupt_run,
            archive_conversation,
            inspect_workspace,
        ])
        .typ::<AppEvent>()
}

#[derive(Debug, thiserror::Error)]
pub enum BindingExportError {
    #[error("TypeScript binding generation failed")]
    Generate(#[from] specta_typescript::Error),
    #[error("TypeScript binding output could not be normalized")]
    Io(#[from] std::io::Error),
}

pub fn export_typescript(path: &Path) -> Result<(), BindingExportError> {
    binding_builder().export(
        specta_typescript::Typescript::default()
            .header("/* eslint-disable @typescript-eslint/no-explicit-any */"),
        path,
    )?;
    let generated = std::fs::read_to_string(path)?;
    std::fs::write(path, format!("{}\n", generated.trim_end()))?;
    Ok(())
}

fn parse_conversation_id(value: &str) -> Result<ConversationId, CommandError> {
    parse_uuid(value, "conversation").map(Into::into)
}

fn parse_run_id(value: &str) -> Result<RunId, CommandError> {
    parse_uuid(value, "run").map(Into::into)
}

fn parse_approval_id(value: &str) -> Result<ApprovalId, CommandError> {
    parse_uuid(value, "approval").map(Into::into)
}

fn parse_timeline_event_id(value: &str) -> Result<TimelineEventId, CommandError> {
    parse_uuid(value, "timeline event").map(Into::into)
}

fn parse_uuid(value: &str, entity: &str) -> Result<Uuid, CommandError> {
    Uuid::parse_str(value).map_err(|_| CommandError {
        code: "invalid-request",
        message: format!("The {entity} identifier is invalid."),
        action: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn application_approval_detail(
        approval: prompting_time_core::domain::Approval,
    ) -> CoreApprovalDetail {
        let question_count = approval
            .input
            .as_ref()
            .map_or(0, |input| input.questions.len() as u32);
        prompting_time_core::store::ApprovalDetailRecord {
            id: approval.id,
            operation: approval.operation,
            scope: approval.scope,
            input: approval.input,
            details: approval.details,
            question_count,
            truncated: false,
        }
        .into()
    }

    #[test]
    fn invalid_identifiers_are_rejected_at_the_boundary() {
        let error = parse_conversation_id("not-a-uuid").unwrap_err();

        assert_eq!(error.code, "invalid-request");
        assert_eq!(error.message, "The conversation identifier is invalid.");
        assert_eq!(error.action, None);
    }

    #[test]
    fn internal_paths_are_not_exposed_in_command_errors() {
        let error = CommandError::from(AppError::Store(StoreError::CreateParent {
            path: PathBuf::from("sensitive-storage-fixture"),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "private detail"),
        }));
        let encoded = serde_json::to_string(&error).unwrap();

        assert_eq!(error.code, "storage-error");
        assert!(!encoded.contains("sensitive-storage-fixture"));
        assert!(!encoded.contains("private detail"));
    }

    #[test]
    fn not_found_errors_never_echo_internal_provider_identifiers() {
        let error = CommandError::from(AppError::Store(StoreError::NotFound {
            entity: "approval",
            id: "opaque-provider-request-secret".to_owned(),
        }));
        let encoded = serde_json::to_string(&error).unwrap();

        assert_eq!(error.code, "not-found");
        assert!(!encoded.contains("opaque-provider-request-secret"));
    }

    #[test]
    fn bootstrap_distinguishes_availability_and_exposes_adapter_capabilities() {
        let provider = ProviderInstallation::from(&StateProviderDiagnostic {
            id: CoreProviderId::Codex,
            installed: true,
            available: true,
            version: Some("fixture".to_owned()),
            diagnostic: None,
            action: None,
            capabilities: [
                CoreProviderCapability::Streaming,
                CoreProviderCapability::Steering,
            ]
            .into(),
        });

        assert!(provider.available);
        assert_eq!(
            provider.capabilities,
            [ProviderCapability::Streaming, ProviderCapability::Steering]
        );
    }

    #[test]
    fn ordinary_timeline_items_have_no_provider_native_fields() {
        let json = serde_json::to_value(TimelineItem {
            id: "event-1".to_owned(),
            conversation_id: "conversation-1".to_owned(),
            run_id: "run-1".to_owned(),
            agent_id: "agent-1".to_owned(),
            sequence: "1".to_owned(),
            kind: TimelineItemKind::Progress,
            role: None,
            content: "Working".to_owned(),
            content_bytes: "7".to_owned(),
            truncated: false,
            provider: ProviderId::Codex,
        })
        .unwrap();
        let encoded = json.to_string().to_ascii_lowercase();

        assert!(!encoded.contains("native"));
        assert!(!encoded.contains("payload"));
    }

    #[test]
    fn approval_boundary_replaces_provider_question_ids_with_canonical_ordinals() {
        let mut approval = prompting_time_core::domain::Approval::new(
            RunId::new(),
            prompting_time_core::domain::AgentId::new(),
            CoreProviderId::Codex,
            "provider-native-request-secret",
            "questions",
            "user",
        );
        approval.input = Some(prompting_time_core::domain::UserInputRequest {
            questions: vec![prompting_time_core::domain::UserInputQuestion {
                id: "provider-native-question-secret".to_owned(),
                header: "Choice".to_owned(),
                question: "Choose".to_owned(),
                options: None,
                is_other: false,
                is_secret: false,
            }],
            auto_resolution_ms: None,
        });

        let detail = ApprovalDetailSnapshot::from_core(application_approval_detail(approval));
        let detail_json = serde_json::to_string(&detail).unwrap();

        assert!(!detail_json.contains("provider-native"));
        assert_eq!(
            detail.input.unwrap().questions[0].id,
            "question-1".to_owned()
        );
    }

    #[test]
    fn reasonless_codex_approval_snapshot_is_actionable_without_native_ids() {
        let snapshot = ApprovalSnapshot::from(prompting_time_core::store::ApprovalSummary {
            id: ApprovalId::new(),
            run_id: RunId::new(),
            agent_id: prompting_time_core::domain::AgentId::new(),
            provider: CoreProviderId::Codex,
            operation: "command execution".to_owned(),
            scope: "command execution".to_owned(),
            status: prompting_time_core::domain::ApprovalStatus::Pending,
            response_pending: false,
        });
        let encoded = serde_json::to_string(&snapshot).unwrap();

        assert_eq!(snapshot.status, ApprovalStatus::Pending);
        assert_eq!(snapshot.scope, "command execution");
        assert!(!encoded.contains("provider-native-item-secret"));
        assert!(!encoded.to_ascii_lowercase().contains("native"));
    }

    #[test]
    fn oversized_approval_detail_returns_an_explicit_bounded_truncation() {
        let mut approval = prompting_time_core::domain::Approval::new(
            RunId::new(),
            prompting_time_core::domain::AgentId::new(),
            CoreProviderId::Codex,
            "opaque-request",
            "questions",
            "user",
        );
        approval.input = Some(prompting_time_core::domain::UserInputRequest {
            questions: vec![prompting_time_core::domain::UserInputQuestion {
                id: "opaque-question".to_owned(),
                header: "Large".to_owned(),
                question: "🦀".repeat(100_000),
                options: None,
                is_other: false,
                is_secret: false,
            }],
            auto_resolution_ms: None,
        });

        let detail = ApprovalDetailSnapshot::from_core(application_approval_detail(approval));
        let encoded = serde_json::to_vec(&detail).unwrap();

        assert!(detail.truncated);
        assert!(encoded.len() <= 256 * 1024);
        assert!(detail.input.is_none());
    }

    #[test]
    fn committed_typescript_binding_is_current() {
        let expected_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/bridge/types.ts");
        if std::env::var_os("PROMPTING_TIME_UPDATE_BINDINGS").is_some() {
            export_typescript(&expected_path).unwrap();
            return;
        }
        let temporary = tempfile::tempdir().unwrap();
        let generated = temporary.path().join("types.ts");
        export_typescript(&generated).unwrap();

        let expected = std::fs::read_to_string(expected_path).unwrap();
        let actual = std::fs::read_to_string(generated).unwrap();
        assert_eq!(actual, expected, "regenerate src/bridge/types.ts from Rust");
    }
}
