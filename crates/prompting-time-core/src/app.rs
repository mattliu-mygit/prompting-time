use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::domain::{
    AgentStatus, ApprovalStatus, Conversation, ConversationId, MessageRole, RunId, Workspace,
    WorkspaceId,
};
use crate::handoff::{
    ChildAgentOutcome, ChildAgentStatus, DurableDecision, HandoffBuilder, HandoffCapsule,
    HandoffError, HandoffInput, HandoffMessage, UnresolvedFailure,
};
use crate::providers::{
    ApprovalResponse, ProviderAdapter, ProviderErrorCategory, ProviderHealth, ProviderId,
};
use crate::router::{
    ProviderRoutingState, ProviderUnavailability, RouteRequest, Router, RoutingCriterion,
    RoutingDecision, RoutingError, RoutingProfile, RoutingReason,
};
use crate::runtime::{
    FallbackRequest, PreparedRunHandle, RunHandle, RunRequest, RunSupervisor, RuntimeError,
};
use crate::store::{
    ConversationSettings, MAX_CANONICAL_MESSAGE_BYTES, NewSubmission, Store, StoreError,
    validate_conversation_settings,
};
use crate::workspace::{WorkspaceError, WorkspaceManager, WorkspaceRequest};

const HANDOFF_BUDGET_CHARS: usize = 32_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConversationWorkspace {
    Projectless,
    Isolated(PathBuf),
    Direct(PathBuf),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationRequest {
    pub title: String,
    pub objective: String,
    pub constraints: Vec<String>,
    pub workspace: ConversationWorkspace,
    pub routing_profile: RoutingProfile,
}

impl ConversationRequest {
    pub fn projectless(title: impl Into<String>) -> Self {
        let title = title.into();
        Self {
            objective: title.clone(),
            title,
            constraints: Vec::new(),
            workspace: ConversationWorkspace::Projectless,
            routing_profile: RoutingProfile::Balanced,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubmitRequest {
    pub command_id: String,
    pub conversation_id: ConversationId,
    pub content: String,
    pub provider_override: Option<ProviderId>,
}

pub struct Submission {
    pub handle: RunHandle,
    pub decision: RoutingDecision,
    pub duplicate: bool,
}

pub struct PromptingTime {
    store: Store,
    router: Router,
    workspace_manager: WorkspaceManager,
    providers: HashMap<ProviderId, Arc<dyn ProviderAdapter>>,
    supervisor: RunSupervisor,
    submissions: Mutex<()>,
}

impl PromptingTime {
    pub fn new(
        store: Store,
        router: Router,
        workspace_manager: WorkspaceManager,
        providers: Vec<Arc<dyn ProviderAdapter>>,
    ) -> Result<Self, AppError> {
        let supervisor = RunSupervisor::new(store.clone(), providers.clone())?;
        let providers = providers
            .into_iter()
            .map(|provider| (provider.id(), provider))
            .collect();
        Ok(Self {
            store,
            router,
            workspace_manager,
            providers,
            supervisor,
            submissions: Mutex::new(()),
        })
    }

    pub async fn create_conversation(
        &self,
        request: ConversationRequest,
    ) -> Result<Conversation, AppError> {
        let settings = ConversationSettings {
            objective: request.objective,
            constraints: request.constraints,
            routing_profile: request.routing_profile,
        };
        validate_conversation_settings(&settings)?;
        let conversation_id = ConversationId::new();
        let workspace_request = match request.workspace {
            ConversationWorkspace::Projectless => WorkspaceRequest::projectless(conversation_id),
            ConversationWorkspace::Isolated(path) => {
                WorkspaceRequest::isolated_for(conversation_id, path)
            }
            ConversationWorkspace::Direct(path) => {
                WorkspaceRequest::direct_for(conversation_id, path)
            }
        };
        let prepared = self
            .workspace_manager
            .prepare_for_persistence(workspace_request)
            .await?;
        let workspace = prepared.workspace(WorkspaceId::new());
        match self
            .store
            .create_configured_conversation(conversation_id, request.title, &workspace, &settings)
            .await
        {
            Ok(conversation) => {
                prepared.commit();
                Ok(conversation)
            }
            Err(source) => {
                let cleanup_error = prepared.rollback().await.err();
                Err(AppError::ConversationPersistence {
                    source,
                    cleanup_error,
                })
            }
        }
    }

    pub async fn submit(&self, request: SubmitRequest) -> Result<Submission, AppError> {
        validate_submit(&request)?;
        let _submission = self.submissions.lock().await;
        let request_hash = submission_hash(&request);
        if let Some(existing) = self.store.load_submission(&request.command_id).await? {
            if existing.run.conversation_id != request.conversation_id
                || existing.request_hash != request_hash
            {
                return Err(StoreError::CommandConflict {
                    command_id: request.command_id,
                }
                .into());
            }
            return Ok(Submission {
                handle: self
                    .supervisor
                    .existing_handle(existing.run, existing.fallback_run),
                decision: existing.routing_decision,
                duplicate: true,
            });
        }
        let conversation = self
            .store
            .load_conversation(request.conversation_id)
            .await?;
        if conversation.archived {
            return Err(StoreError::ConversationArchived(conversation.id).into());
        }
        let settings = self
            .store
            .load_conversation_settings(request.conversation_id)
            .await?;
        let workspace = self.store.load_workspace(request.conversation_id).await?;
        let routing_states = self.routing_states().await;
        let mut route = RouteRequest::builder(&request.content)
            .eligible(routing_states)
            .usage(self.store.provider_usage().await?)
            .profile(settings.routing_profile);
        if let Some(provider) = self.store.latest_provider(request.conversation_id).await? {
            route = route.current_provider(provider);
        }
        if let Some(provider) = request.provider_override {
            route = route.override_provider(provider);
        }
        let decision = self.router.route(route.build())?;
        let native_session = self
            .store
            .load_provider_session(request.conversation_id, decision.provider)
            .await?;
        let switched = self
            .store
            .latest_provider(request.conversation_id)
            .await?
            .is_some_and(|provider| provider != decision.provider);
        let handoff = if native_session.is_none() || switched {
            Some(
                self.build_handoff(
                    request.conversation_id,
                    decision.provider,
                    &request.content,
                    &settings,
                    &workspace,
                    None,
                )
                .await?,
            )
        } else {
            None
        };
        let fallback = if request.provider_override.is_none() {
            decision
                .eligible_providers
                .iter()
                .copied()
                .find(|provider| *provider != decision.provider)
        } else {
            None
        };
        let fallback = match fallback {
            Some(provider) => {
                let session = self
                    .store
                    .load_provider_session(request.conversation_id, provider)
                    .await?;
                let capsule = self
                    .build_handoff(
                        request.conversation_id,
                        provider,
                        &request.content,
                        &settings,
                        &workspace,
                        Some(UnresolvedFailure::ProviderRejectedBeforeDispatch),
                    )
                    .await?;
                Some(FallbackRequest {
                    provider,
                    native_session_id: session.map(|session| session.native_id),
                    turn: crate::providers::TurnRequest::new(&capsule.rendered),
                    handoff_rendered: Some(capsule.rendered),
                    handoff_hash: Some(capsule.content_hash),
                    routing_decision: Some(Box::new(RoutingDecision {
                        provider,
                        reason: RoutingReason::SafeFallback,
                        override_provider: None,
                        rationale: vec![
                            RoutingCriterion::EligibleProviders {
                                providers: decision.eligible_providers.clone(),
                            },
                            RoutingCriterion::SafeFallback {
                                from: decision.provider,
                                to: provider,
                            },
                        ],
                        explanation: format!(
                            "Selected {provider:?} because the first provider rejected the request before any mutation"
                        ),
                        ..decision.clone()
                    })),
                })
            }
            None => None,
        };
        let prompt = match &handoff {
            Some(capsule) => capsule.rendered.clone(),
            None => request.content.clone(),
        };
        let turn_prompt = prompt.clone();
        let mut run_request = RunRequest::new(
            request.conversation_id,
            workspace.execution_path,
            decision.provider,
            crate::providers::TurnRequest::new(prompt),
        );
        if let Some(session) = native_session {
            run_request = run_request.resume(session.native_id);
        }
        if let Some(fallback) = fallback {
            run_request = run_request.with_fallback_request(fallback);
        }
        let PreparedRunHandle { handle, duplicate } = self
            .supervisor
            .submit_persisted(
                run_request,
                NewSubmission {
                    command_id: request.command_id,
                    request_hash,
                    conversation_id: request.conversation_id,
                    provider: decision.provider,
                    content: request.content,
                    routing_decision: decision.clone(),
                    handoff_rendered: handoff.as_ref().map(|capsule| capsule.rendered.clone()),
                    handoff_hash: handoff.map(|capsule| capsule.content_hash),
                    turn_prompt,
                },
            )
            .await?;
        Ok(Submission {
            handle,
            decision,
            duplicate,
        })
    }

    pub async fn steer(&self, run_id: RunId, text: &str) -> Result<(), AppError> {
        self.supervisor
            .steer(run_id, text)
            .await
            .map_err(Into::into)
    }

    pub async fn respond_to_approval(
        &self,
        run_id: RunId,
        provider_request_id: &str,
        response: ApprovalResponse,
    ) -> Result<(), AppError> {
        let approval = self
            .store
            .load_approval(run_id, provider_request_id)
            .await?;
        if approval.status != ApprovalStatus::Pending {
            return Err(AppError::StaleApproval {
                run_id,
                request_id: provider_request_id.to_owned(),
            });
        }
        self.supervisor
            .respond(run_id, provider_request_id, response)
            .await
            .map_err(Into::into)
    }

    pub async fn interrupt(&self, run_id: RunId) -> Result<(), AppError> {
        self.supervisor.interrupt(run_id).await.map_err(Into::into)
    }

    pub async fn archive(&self, conversation_id: ConversationId) -> Result<(), AppError> {
        self.store
            .archive_conversation(conversation_id)
            .await
            .map_err(Into::into)
    }

    pub async fn shutdown(&self) -> Result<(), AppError> {
        let runtime_result = self.supervisor.shutdown().await;
        self.workspace_manager.wait_for_pending_preparations().await;
        runtime_result.map_err(Into::into)
    }

    async fn routing_states(&self) -> Vec<ProviderRoutingState> {
        let mut providers = self.providers.values().collect::<Vec<_>>();
        providers.sort_by_key(|provider| match provider.id() {
            ProviderId::Codex => 0,
            ProviderId::Claude => 1,
        });
        let mut states = Vec::with_capacity(providers.len());
        for provider in providers {
            let state = match provider.health().await {
                Ok(ProviderHealth::Healthy { .. }) => {
                    ProviderRoutingState::available(provider.id(), provider.capabilities())
                }
                Ok(ProviderHealth::Unavailable { category }) => ProviderRoutingState::unavailable(
                    provider.id(),
                    provider.capabilities(),
                    unavailable_category(&category),
                ),
                Err(error) => ProviderRoutingState::unavailable(
                    provider.id(),
                    provider.capabilities(),
                    unavailable_error(error.category()),
                ),
            };
            states.push(state);
        }
        states
    }

    async fn build_handoff(
        &self,
        conversation_id: ConversationId,
        provider: ProviderId,
        current_request: &str,
        settings: &ConversationSettings,
        workspace: &Workspace,
        unresolved_failure: Option<UnresolvedFailure>,
    ) -> Result<HandoffCapsule, AppError> {
        let boundary = self
            .store
            .provider_context_boundary(conversation_id, provider)
            .await?;
        let messages = self
            .store
            .load_messages_after(conversation_id, boundary, HANDOFF_BUDGET_CHARS)
            .await?
            .into_iter()
            .map(|message| match message.role {
                MessageRole::User => HandoffMessage::user(message.content),
                MessageRole::Assistant => HandoffMessage::assistant(message.content),
            })
            .collect();
        let decisions = self
            .store
            .load_routing_decisions(conversation_id)
            .await?
            .into_iter()
            .map(|decision| DurableDecision {
                provider: decision.provider,
                reason: decision.reason,
                task_kind: decision.task_kind,
            })
            .collect();
        let child_agent_outcomes = self
            .store
            .load_child_agent_outcomes(conversation_id)
            .await?
            .into_iter()
            .map(|outcome| ChildAgentOutcome {
                provider: outcome.provider,
                provider_native_id: outcome.provider_native_id,
                summary: outcome.summary,
                status: match outcome.status {
                    AgentStatus::Queued => ChildAgentStatus::Pending,
                    AgentStatus::Running => ChildAgentStatus::Running,
                    AgentStatus::Waiting => ChildAgentStatus::Waiting,
                    AgentStatus::Interrupted => ChildAgentStatus::Interrupted,
                    AgentStatus::Completed => ChildAgentStatus::Completed,
                    AgentStatus::Failed => ChildAgentStatus::Errored,
                },
            })
            .collect();
        let workspace_state = Some(self.workspace_manager.snapshot(workspace).await?);
        HandoffBuilder::new(HANDOFF_BUDGET_CHARS)
            .build(HandoffInput {
                objective: settings.objective.clone(),
                current_request: current_request.to_owned(),
                constraints: settings.constraints.clone(),
                decisions,
                child_agent_outcomes,
                workspace_state,
                messages,
                unresolved_failure,
            })
            .map_err(Into::into)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Routing(#[from] RoutingError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error(transparent)]
    Handoff(#[from] HandoffError),
    #[error("conversation persistence failed after workspace preparation")]
    ConversationPersistence {
        #[source]
        source: StoreError,
        cleanup_error: Option<WorkspaceError>,
    },
    #[error("command id and message content must not be empty")]
    EmptySubmission,
    #[error("message exceeds the {limit}-byte limit")]
    MessageTooLarge { limit: usize },
    #[error("approval request {request_id} for run {run_id} is no longer pending")]
    StaleApproval { run_id: RunId, request_id: String },
}

fn validate_submit(request: &SubmitRequest) -> Result<(), AppError> {
    if request.command_id.trim().is_empty() || request.content.trim().is_empty() {
        return Err(AppError::EmptySubmission);
    }
    if request.content.len() > MAX_CANONICAL_MESSAGE_BYTES {
        return Err(AppError::MessageTooLarge {
            limit: MAX_CANONICAL_MESSAGE_BYTES,
        });
    }
    Ok(())
}

fn submission_hash(request: &SubmitRequest) -> String {
    let mut digest = Sha256::new();
    digest.update(request.conversation_id.to_string());
    digest.update([0]);
    digest.update(request.content.as_bytes());
    digest.update([0]);
    digest.update(match request.provider_override {
        Some(ProviderId::Codex) => b"codex".as_slice(),
        Some(ProviderId::Claude) => b"claude".as_slice(),
        None => b"auto".as_slice(),
    });
    format!("{:x}", digest.finalize())
}

fn unavailable_category(category: &str) -> ProviderUnavailability {
    let category = category.to_ascii_lowercase();
    if category.contains("not-installed") || category.contains("not installed") {
        ProviderUnavailability::NotInstalled
    } else if category.contains("unsupported-version") || category.contains("unsupported version") {
        ProviderUnavailability::UnsupportedVersion
    } else if category.contains("unauthenticated")
        || category.contains("not-authenticated")
        || category.contains("login-required")
    {
        ProviderUnavailability::Unauthenticated
    } else if category.contains("quota") || category.contains("rate-limit") {
        ProviderUnavailability::QuotaBlocked
    } else {
        ProviderUnavailability::Unhealthy
    }
}

fn unavailable_error(category: ProviderErrorCategory) -> ProviderUnavailability {
    match category {
        ProviderErrorCategory::NotInstalled => ProviderUnavailability::NotInstalled,
        ProviderErrorCategory::TimedOut
        | ProviderErrorCategory::InspectionFailed
        | ProviderErrorCategory::Rejected
        | ProviderErrorCategory::Protocol
        | ProviderErrorCategory::Transport
        | ProviderErrorCategory::MalformedJson
        | ProviderErrorCategory::OversizedFrame
        | ProviderErrorCategory::ProcessExited
        | ProviderErrorCategory::StreamClosed
        | ProviderErrorCategory::ContractViolation => ProviderUnavailability::Unhealthy,
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use tempfile::tempdir;

    use crate::store::{MAX_OBJECTIVE_BYTES, install_conversation_persistence_barrier};

    use super::*;

    #[test]
    fn provider_health_categories_preserve_actionable_unavailability() {
        assert_eq!(
            unavailable_category("login-required"),
            ProviderUnavailability::Unauthenticated
        );
        assert_eq!(
            unavailable_category("quota-exhausted"),
            ProviderUnavailability::QuotaBlocked
        );
        assert_eq!(
            unavailable_error(ProviderErrorCategory::NotInstalled),
            ProviderUnavailability::NotInstalled
        );
    }

    #[tokio::test]
    async fn oversized_required_context_is_rejected_before_workspace_preparation() {
        let temp = tempdir().unwrap();
        let app_data = temp.path().join("app-data");
        let app = PromptingTime::new(
            Store::open_in_memory().await.unwrap(),
            Router::default(),
            WorkspaceManager::new(&app_data),
            Vec::<Arc<dyn ProviderAdapter>>::new(),
        )
        .unwrap();

        let error = app
            .create_conversation(ConversationRequest {
                title: "oversized objective".to_owned(),
                objective: "x".repeat(MAX_OBJECTIVE_BYTES + 1),
                constraints: Vec::new(),
                workspace: ConversationWorkspace::Projectless,
                routing_profile: RoutingProfile::Balanced,
            })
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            AppError::Store(StoreError::InvalidData { .. })
        ));
        assert!(!app_data.exists());
    }

    #[tokio::test]
    async fn aborting_conversation_persistence_rolls_back_owned_isolated_state() {
        let temp = tempdir().unwrap();
        let repository = temp.path().join("repository");
        std::fs::create_dir(&repository).unwrap();
        run_git(&repository, &["init", "--initial-branch=main"]);
        run_git(&repository, &["config", "user.name", "Prompting Time Test"]);
        run_git(
            &repository,
            &["config", "user.email", "prompting-time@example.test"],
        );
        std::fs::write(repository.join("tracked.txt"), "initial\n").unwrap();
        run_git(&repository, &["add", "tracked.txt"]);
        run_git(&repository, &["commit", "-m", "initial"]);
        let store = Store::open_in_memory().await.unwrap();
        let app_data = temp.path().join("app-data");
        let app = Arc::new(
            PromptingTime::new(
                store,
                Router::default(),
                WorkspaceManager::new(&app_data),
                Vec::<Arc<dyn ProviderAdapter>>::new(),
            )
            .unwrap(),
        );
        let barrier = install_conversation_persistence_barrier();
        let task_app = Arc::clone(&app);
        let task_repository = repository.clone();
        let create = tokio::spawn(async move {
            task_app
                .create_conversation(ConversationRequest {
                    title: "cancel persistence".to_owned(),
                    objective: "verify exact rollback".to_owned(),
                    constraints: Vec::new(),
                    workspace: ConversationWorkspace::Isolated(task_repository),
                    routing_profile: RoutingProfile::Balanced,
                })
                .await
        });

        barrier.transaction_started.wait().await;
        create.abort();
        assert!(create.await.unwrap_err().is_cancelled());
        app.shutdown().await.unwrap();

        let worktrees = git_output(&repository, &["worktree", "list", "--porcelain"]);
        assert!(!worktrees.contains(&app_data.display().to_string()));
        let refs = git_output(
            &repository,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/heads/prompting-time",
                "refs/prompting-time",
            ],
        );
        assert!(refs.trim().is_empty());
    }

    fn run_git(directory: &std::path::Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git command failed: {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_output(directory: &std::path::Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "git command failed: {args:?}");
        String::from_utf8(output.stdout).unwrap()
    }
}
