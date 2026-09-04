use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::domain::{
    AgentNode, AgentStatus, Approval, ApprovalId, ApprovalStatus, Conversation, ConversationId,
    MessageRole, ProviderRun, RollupStatus, RunId, Workspace, WorkspaceId,
};
use crate::handoff::{
    ChildAgentOutcome, ChildAgentStatus, DurableDecision, HandoffBuilder, HandoffCapsule,
    HandoffError, HandoffInput, HandoffMessage, UnresolvedFailure,
};
use crate::providers::{
    ApprovalResponse, DispatchCertainty, ProviderAdapter, ProviderErrorCategory, ProviderHealth,
    ProviderId,
};
use crate::router::{
    ProviderRoutingState, ProviderUnavailability, RouteRequest, Router, RoutingCriterion,
    RoutingDecision, RoutingError, RoutingProfile, RoutingReason,
};
#[cfg(test)]
use crate::runtime::DISPATCH_LEASE_STALE_GRACE;
use crate::runtime::{
    FallbackRequest, PreparedRunHandle, RecoveryClaim, RunHandle, RunRequest, RunSupervisor,
    RuntimeError,
};
use crate::store::{
    AgentPage, ApprovalPage, ConversationSettings, EventDetail, MAX_CANONICAL_MESSAGE_BYTES,
    NewSubmission, Page, ProviderEventRecord, SidebarDetails, Store, StoreChange, StoreError,
    TimelineRecord, validate_conversation_settings,
};
use crate::workspace::{
    CleanupEligibility, WorkspaceError, WorkspaceManager, WorkspaceRequest, WorkspaceSnapshot,
};

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationOverview {
    pub conversation: Conversation,
    pub routing_profile: RoutingProfile,
    pub project_root: Option<PathBuf>,
    pub run: Option<RunOverview>,
    pub rollup_status: Option<RollupStatus>,
    pub agents: Vec<AgentNode>,
    pub agents_truncated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunOverview {
    pub id: RunId,
    pub provider: ProviderId,
    pub status: crate::domain::RunStatus,
}

impl From<ProviderRun> for RunOverview {
    fn from(run: ProviderRun) -> Self {
        Self {
            id: run.id,
            provider: run.provider,
            status: run.status,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimelineSnapshot {
    pub events: Page<TimelineRecord>,
    pub approvals: ApprovalPage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalDetail {
    pub id: ApprovalId,
    pub status: crate::domain::ApprovalStatus,
    pub response_pending: bool,
    pub agent_path: Vec<String>,
    pub agent_path_truncated: bool,
    pub operation: String,
    pub scope: String,
    pub input: Option<crate::domain::UserInputRequest>,
    pub details: Option<crate::domain::ApprovalRequestDetails>,
    pub question_count: u32,
    pub truncated: bool,
}

impl From<crate::store::ApprovalDetailRecord> for ApprovalDetail {
    fn from(approval: crate::store::ApprovalDetailRecord) -> Self {
        Self {
            id: approval.id,
            status: approval.status,
            response_pending: approval.response_pending,
            agent_path: approval.agent_path,
            agent_path_truncated: approval.agent_path_truncated,
            operation: approval.operation,
            scope: approval.scope,
            input: approval.input.map(|mut input| {
                for (index, question) in input.questions.iter_mut().enumerate() {
                    question.id = format!("question-{}", index + 1);
                }
                input
            }),
            details: approval.details,
            question_count: approval.question_count,
            truncated: approval.truncated,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectorSnapshot {
    pub workspace: WorkspaceSnapshot,
    pub execution_path: PathBuf,
    pub owned_worktree: bool,
    pub cleanup: CleanupEligibility,
    pub run: Option<RunOverview>,
    pub routing: Option<RoutingDecision>,
    pub handoff: Option<String>,
    pub active_descendant_count: usize,
    pub agents_truncated: bool,
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
        self.create_conversation_with_workspace(request)
            .await
            .map(|(conversation, _)| conversation)
    }

    async fn create_conversation_with_workspace(
        &self,
        request: ConversationRequest,
    ) -> Result<(Conversation, Workspace), AppError> {
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
                Ok((conversation, workspace))
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

    pub async fn create_conversation_overview(
        &self,
        request: ConversationRequest,
    ) -> Result<ConversationOverview, AppError> {
        let routing_profile = request.routing_profile;
        let (conversation, workspace) = self.create_conversation_with_workspace(request).await?;
        Ok(ConversationOverview {
            conversation,
            routing_profile,
            project_root: workspace.project_root,
            run: None,
            rollup_status: None,
            agents: Vec::new(),
            agents_truncated: false,
        })
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

    pub async fn respond_to_approval_id(
        &self,
        approval_id: ApprovalId,
        response: ApprovalResponse,
    ) -> Result<(), AppError> {
        let approval = self.store.load_approval_by_id(approval_id).await?;
        if approval.status != ApprovalStatus::Pending {
            return Err(AppError::StaleApproval {
                run_id: approval.run_id,
                request_id: approval_id.to_string(),
            });
        }
        let provider_request_id =
            approval
                .provider_request_id
                .clone()
                .ok_or_else(|| StoreError::InvalidData {
                    entity: "approval",
                    detail: "pending approval is missing its provider request identifier"
                        .to_owned(),
                })?;
        let response = remap_approval_response(&approval, response)?;
        self.supervisor
            .respond(approval.run_id, &provider_request_id, response)
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

    pub async fn list_conversations(
        &self,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<Page<Conversation>, AppError> {
        self.store
            .list_conversations(cursor, limit)
            .await
            .map_err(Into::into)
    }

    pub async fn list_conversation_overviews(
        &self,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<Page<ConversationOverview>, AppError> {
        let page = self.store.list_active_conversations(cursor, limit).await?;
        let ids = page
            .items
            .iter()
            .map(|conversation| conversation.id)
            .collect::<Vec<_>>();
        let details = self.store.load_sidebar_details(&ids).await?;
        let items = page
            .items
            .into_iter()
            .zip(details)
            .map(|(conversation, details)| overview(conversation, details))
            .collect();
        Ok(Page {
            items,
            next_cursor: page.next_cursor,
        })
    }

    pub async fn load_conversation_overview(
        &self,
        conversation_id: ConversationId,
    ) -> Result<ConversationOverview, AppError> {
        let conversation = self.store.load_conversation(conversation_id).await?;
        let details = self
            .store
            .load_sidebar_details(&[conversation_id])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| StoreError::InvalidData {
                entity: "conversation overview",
                detail: "requested conversation details were omitted".to_owned(),
            })?;
        Ok(overview(conversation, details))
    }

    pub async fn load_timeline_snapshot(
        &self,
        conversation_id: ConversationId,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<TimelineSnapshot, AppError> {
        let events = self
            .store
            .load_recent_timeline(conversation_id, cursor, limit)
            .await?;
        let approvals = self
            .store
            .load_recent_approvals(conversation_id, 30)
            .await?;
        Ok(TimelineSnapshot { events, approvals })
    }

    pub async fn load_event_detail(
        &self,
        event_id: crate::domain::TimelineEventId,
    ) -> Result<EventDetail, AppError> {
        self.store
            .load_event_detail(event_id)
            .await
            .map_err(Into::into)
    }

    pub async fn load_approvals(
        &self,
        conversation_id: ConversationId,
        cursor: Option<String>,
        pending: bool,
        limit: u32,
    ) -> Result<ApprovalPage, AppError> {
        self.store
            .load_approvals(conversation_id, cursor, pending, limit)
            .await
            .map_err(Into::into)
    }

    pub async fn load_approval_detail(
        &self,
        approval_id: ApprovalId,
    ) -> Result<ApprovalDetail, AppError> {
        let approval = self.store.load_approval_detail(approval_id).await?;
        Ok(approval.into())
    }

    pub async fn load_approval_questions(
        &self,
        approval_id: ApprovalId,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<crate::store::ApprovalQuestionPage, AppError> {
        self.store
            .load_approval_questions(approval_id, cursor, limit)
            .await
            .map_err(Into::into)
    }

    pub async fn load_agent_page(
        &self,
        conversation_id: ConversationId,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<AgentPage, AppError> {
        self.store
            .load_agent_page(conversation_id, cursor, limit)
            .await
            .map_err(Into::into)
    }

    pub async fn load_run_audits(
        &self,
        conversation_id: ConversationId,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<crate::store::RunAuditPage, AppError> {
        self.store
            .load_run_audits(conversation_id, cursor, limit)
            .await
            .map_err(Into::into)
    }

    pub async fn load_run_audit(
        &self,
        conversation_id: ConversationId,
        run_id: RunId,
    ) -> Result<crate::store::RunAuditDetailRecord, AppError> {
        self.store
            .load_run_audit(conversation_id, run_id)
            .await
            .map_err(Into::into)
    }

    pub async fn inspect_workspace(
        &self,
        conversation_id: ConversationId,
    ) -> Result<WorkspaceSnapshot, AppError> {
        let workspace = self.store.load_workspace(conversation_id).await?;
        self.workspace_manager
            .snapshot(&workspace)
            .await
            .map_err(Into::into)
    }

    pub async fn is_git_project(&self, path: &std::path::Path) -> Result<bool, AppError> {
        self.workspace_manager
            .is_git_project(path)
            .await
            .map_err(Into::into)
    }

    pub async fn inspect_conversation(
        &self,
        conversation_id: ConversationId,
    ) -> Result<InspectorSnapshot, AppError> {
        let workspace = self.store.load_workspace(conversation_id).await?;
        let execution_path = workspace.execution_path.clone();
        let owned_worktree = workspace.owned_worktree;
        let snapshot = self.workspace_manager.snapshot(&workspace).await?;
        let lease = self.workspace_manager.lease(&workspace).await?;
        let cleanup = self.workspace_manager.cleanup_eligibility(&lease).await?;
        let run = self
            .store
            .latest_run_for_conversation(conversation_id)
            .await?;
        let (routing, handoff) = if let Some(run) = &run {
            let routing = match self.store.load_routing_decision(run.id).await {
                Ok(decision) => Some(decision),
                Err(StoreError::NotFound { .. }) => None,
                Err(error) => return Err(error.into()),
            };
            let handoff = self
                .store
                .load_handoff(run.id)
                .await?
                .map(|(rendered, _)| rendered);
            (routing, handoff)
        } else {
            (None, None)
        };
        let details = self
            .store
            .load_sidebar_details(&[conversation_id])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| StoreError::InvalidData {
                entity: "conversation inspector",
                detail: "sidebar details were omitted".to_owned(),
            })?;
        Ok(InspectorSnapshot {
            workspace: snapshot,
            execution_path,
            owned_worktree,
            cleanup,
            run: run.map(Into::into),
            routing,
            handoff,
            active_descendant_count: details.active_descendant_count,
            agents_truncated: details.agents_truncated,
        })
    }

    /// Re-enters turns that are provably undispatched and closes every ambiguous turn bottom-up.
    pub async fn reconcile_startup(&self) -> Result<usize, AppError> {
        self.reconcile_recoverable_runs(false).await
    }

    /// Rechecks claims after startup has given any live owner time to renew after a system stall.
    pub async fn reconcile_abandoned_dispatches(&self) -> Result<usize, AppError> {
        self.reconcile_recoverable_runs(true).await
    }

    async fn reconcile_recoverable_runs(&self, reap_stale_claims: bool) -> Result<usize, AppError> {
        let mut interrupted_runs = 0;

        // Active or waiting work may already have caused provider side effects. The current
        // durable model retains a session ID but not a provider-confirmed active-turn token, so
        // replaying its prompt is never a safe resume even when the adapter can resume sessions.
        let mut ambiguous_cursor = None;
        loop {
            let run_ids = self
                .store
                .load_ambiguous_recovery_run_ids(ambiguous_cursor, 200)
                .await?;
            if run_ids.is_empty() {
                break;
            }
            ambiguous_cursor = run_ids.last().copied();
            for run_id in run_ids {
                interrupted_runs += self
                    .interrupt_stale_recovery(
                        run_id,
                        "Prompting Time restarted while this provider turn was active. Review its output and workspace before retrying.",
                        reap_stale_claims,
                    )
                    .await?;
            }
        }

        let mut cursor = None;
        loop {
            let recoveries = self.store.load_queued_recovery_batch(cursor, 200).await?;
            if recoveries.is_empty() {
                break;
            }
            cursor = recoveries.last().map(|recovery| recovery.run.id);
            for recovery in recoveries {
                let root = (recovery.roots.len() == 1)
                    .then(|| recovery.roots.first().cloned())
                    .flatten();
                let intent = recovery.attempt_intent;
                let adapter = self.providers.get(&recovery.run.provider);
                if recovery.run.dispatch_certainty == Some(DispatchCertainty::MayHaveDispatched) {
                    interrupted_runs += self
                        .interrupt_stale_recovery(
                            recovery.run.id,
                            "This queued turn may have reached its provider before Prompting Time stopped. Review provider output and the workspace before retrying.",
                            reap_stale_claims,
                        )
                        .await?;
                    continue;
                }
                let safe_dispatch = recovery.run.mutation_state
                    == crate::domain::MutationState::NoneObserved
                    && root.as_ref().is_some_and(|root| {
                        root.status == AgentStatus::Queued && root.run_id == recovery.run.id
                    })
                    && intent.is_some()
                    && recovery.run.native_session_id.is_none()
                    && adapter.is_some();
                if !safe_dispatch {
                    interrupted_runs += self
                        .interrupt_stale_recovery(
                            recovery.run.id,
                            "This queued turn lacked enough durable state to restart safely. Review the conversation and retry the message.",
                            reap_stale_claims,
                        )
                        .await?;
                    continue;
                }

                let Some(claim) = self
                    .supervisor
                    .claim_recovery_run(recovery.run.id, reap_stale_claims)
                    .await?
                else {
                    continue;
                };
                let workspace = self
                    .store
                    .load_workspace(recovery.run.conversation_id)
                    .await?;
                let intent = intent.expect("safe recovery has a durable turn intent");
                let request = RunRequest::new(
                    recovery.run.conversation_id,
                    workspace.execution_path,
                    recovery.run.provider,
                    crate::providers::TurnRequest::new(intent.turn_prompt),
                );
                match self
                    .supervisor
                    .recover_persisted(
                        request,
                        recovery.run.clone(),
                        root.expect("safe recovery has a root agent"),
                        &claim,
                    )
                    .await
                {
                    Ok(_) => {}
                    Err(
                        RuntimeError::RunQueueFull { .. } | RuntimeError::CommandQueueFull { .. },
                    ) => {
                        interrupted_runs += self
                            .interrupt_owned_recovery(
                                recovery.run.id,
                                &claim,
                                "Startup recovery capacity was exhausted. Review the conversation and retry the message.",
                            )
                            .await?;
                    }
                    Err(RuntimeError::DispatchAlreadyClaimed(run_id)) => {
                        debug_assert_eq!(run_id, recovery.run.id);
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(interrupted_runs)
    }

    async fn interrupt_stale_recovery(
        &self,
        run_id: RunId,
        diagnostic: &'static str,
        reap_stale_claims: bool,
    ) -> Result<usize, AppError> {
        let Some(claim) = self
            .supervisor
            .claim_recovery_run(run_id, reap_stale_claims)
            .await?
        else {
            return Ok(0);
        };
        self.interrupt_owned_recovery(run_id, &claim, diagnostic)
            .await
    }

    async fn interrupt_owned_recovery(
        &self,
        run_id: RunId,
        claim: &RecoveryClaim,
        diagnostic: &'static str,
    ) -> Result<usize, AppError> {
        let mut interrupted_roots = 0;
        let mut diagnostic_written = false;
        loop {
            let batch = self
                .store
                .load_recovery_agent_batch_for_run(run_id, 200)
                .await?;
            if batch.is_empty() {
                break;
            }
            for recovery in batch {
                if recovery.is_root && !diagnostic_written {
                    self.store
                        .append_owned_run_event(
                            recovery.run_id,
                            recovery.agent_id,
                            claim.owner_id(),
                            ProviderEventRecord::diagnostic(diagnostic),
                        )
                        .await?;
                    diagnostic_written = true;
                }
                self.store
                    .append_owned_run_event(
                        recovery.run_id,
                        recovery.agent_id,
                        claim.owner_id(),
                        ProviderEventRecord::interrupted_with_mutation(recovery.mutation_state),
                    )
                    .await?;
                interrupted_roots += usize::from(recovery.is_root);
            }
        }
        Ok(interrupted_roots)
    }

    pub async fn shutdown(&self) -> Result<(), AppError> {
        let runtime_result = self.supervisor.shutdown().await;
        self.workspace_manager.wait_for_pending_preparations().await;
        runtime_result.map_err(Into::into)
    }

    pub async fn shutdown_with_grace(&self, grace: std::time::Duration) -> Result<(), AppError> {
        let runtime_result = self.supervisor.shutdown_with_grace(grace).await;
        let _ = tokio::time::timeout(
            grace,
            self.workspace_manager.wait_for_pending_preparations(),
        )
        .await;
        runtime_result.map_err(Into::into)
    }

    pub async fn force_shutdown(&self) -> Result<(), AppError> {
        self.supervisor.force_shutdown().await.map_err(Into::into)
    }

    pub fn subscribe_changes(&self) -> tokio::sync::broadcast::Receiver<StoreChange> {
        self.store.subscribe_changes()
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

fn overview(conversation: Conversation, details: SidebarDetails) -> ConversationOverview {
    debug_assert_eq!(conversation.id, details.conversation_id);
    ConversationOverview {
        conversation,
        routing_profile: details.routing_profile,
        project_root: details.project_root,
        run: details.run.map(Into::into),
        rollup_status: details.rollup_status,
        agents: details.agents,
        agents_truncated: details.agents_truncated,
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
    #[error("approval response contains an unknown question identifier")]
    InvalidApprovalQuestion,
}

fn remap_approval_response(
    approval: &Approval,
    response: ApprovalResponse,
) -> Result<ApprovalResponse, AppError> {
    let ApprovalResponse::Answers(answers) = response else {
        return Ok(response);
    };
    let questions = approval
        .input
        .as_ref()
        .ok_or(AppError::InvalidApprovalQuestion)?;
    let mut native_answers = BTreeMap::new();
    for (canonical_id, answer) in answers {
        let ordinal = canonical_id
            .strip_prefix("question-")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|ordinal| *ordinal > 0)
            .ok_or(AppError::InvalidApprovalQuestion)?;
        let native_id = questions
            .questions
            .get(ordinal - 1)
            .map(|question| question.id.clone())
            .ok_or(AppError::InvalidApprovalQuestion)?;
        native_answers.insert(native_id, answer);
    }
    Ok(ApprovalResponse::Answers(native_answers))
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
        || category.contains("not authenticated")
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use tempfile::tempdir;
    use tokio::sync::{Barrier, Semaphore, mpsc};

    use crate::domain::{AgentId, RunStatus};
    use crate::providers::{
        ProviderCapabilities, ProviderCapability, ProviderEvent, ProviderSession, ProviderTurn,
        ProviderTurnOwner, ResumeSession, StartSession, TurnRequest,
    };
    use crate::store::{
        MAX_OBJECTIVE_BYTES, NewSubmission, PreparedSubmission,
        install_conversation_persistence_barrier,
    };

    use super::*;

    struct RecoveryAdapter {
        starts: AtomicUsize,
        resumes: AtomicUsize,
        turns: AtomicUsize,
        start_gate: Option<Arc<Semaphore>>,
    }

    impl RecoveryAdapter {
        fn new() -> Self {
            Self {
                starts: AtomicUsize::new(0),
                resumes: AtomicUsize::new(0),
                turns: AtomicUsize::new(0),
                start_gate: None,
            }
        }

        fn blocked() -> (Self, Arc<Semaphore>) {
            let gate = Arc::new(Semaphore::new(0));
            (
                Self {
                    starts: AtomicUsize::new(0),
                    resumes: AtomicUsize::new(0),
                    turns: AtomicUsize::new(0),
                    start_gate: Some(Arc::clone(&gate)),
                },
                gate,
            )
        }
    }

    struct RecoveryTurnOwner;

    #[async_trait]
    impl ProviderTurnOwner for RecoveryTurnOwner {
        async fn shutdown(self: Box<Self>) -> Result<(), crate::providers::ProviderError> {
            Ok(())
        }
    }

    #[async_trait]
    impl ProviderAdapter for RecoveryAdapter {
        fn id(&self) -> ProviderId {
            ProviderId::Codex
        }

        fn capabilities(&self) -> ProviderCapabilities {
            [ProviderCapability::Streaming, ProviderCapability::Resume].into()
        }

        async fn health(&self) -> Result<ProviderHealth, crate::providers::ProviderError> {
            Ok(ProviderHealth::Healthy {
                version: "recovery-fixture".to_owned(),
            })
        }

        async fn start_session(
            &self,
            request: StartSession,
        ) -> Result<ProviderSession, crate::providers::ProviderError> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = &self.start_gate {
                gate.acquire().await.unwrap().forget();
            }
            Ok(ProviderSession {
                provider: ProviderId::Codex,
                native_id: format!("session-{}", request.conversation_id),
                native_group_id: None,
            })
        }

        async fn resume_session(
            &self,
            native_id: &str,
            _request: ResumeSession,
        ) -> Result<ProviderSession, crate::providers::ProviderError> {
            self.resumes.fetch_add(1, Ordering::SeqCst);
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
        ) -> Result<ProviderTurn, crate::providers::ProviderError> {
            self.turns.fetch_add(1, Ordering::SeqCst);
            let (sender, receiver) = mpsc::channel(4);
            sender
                .send(Ok(ProviderEvent::TurnStarted {
                    native_turn_id: "recovered-turn".to_owned(),
                }))
                .await
                .unwrap();
            sender.send(Ok(ProviderEvent::TurnCompleted)).await.unwrap();
            Ok(ProviderTurn::new(receiver, RecoveryTurnOwner))
        }

        async fn steer(
            &self,
            _session: &ProviderSession,
            _active_turn: &str,
            _text: &str,
        ) -> Result<(), crate::providers::ProviderError> {
            Ok(())
        }

        async fn respond(
            &self,
            _session: &ProviderSession,
            _request_id: &str,
            _response: ApprovalResponse,
        ) -> Result<(), crate::providers::ProviderError> {
            Ok(())
        }

        async fn interrupt(
            &self,
            _session: &ProviderSession,
            _active_turn: &str,
        ) -> Result<(), crate::providers::ProviderError> {
            Ok(())
        }
    }

    fn recovery_submission(conversation_id: ConversationId) -> NewSubmission {
        let decision = Router::default()
            .route(
                RouteRequest::builder("recover this turn")
                    .eligible([ProviderRoutingState::available(
                        ProviderId::Codex,
                        [ProviderCapability::Streaming, ProviderCapability::Resume].into(),
                    )])
                    .override_provider(ProviderId::Codex)
                    .build(),
            )
            .unwrap();
        NewSubmission {
            command_id: format!("recover-{conversation_id}"),
            request_hash: "recovery-hash".to_owned(),
            conversation_id,
            provider: ProviderId::Codex,
            content: "recover this turn".to_owned(),
            routing_decision: decision,
            handoff_rendered: None,
            handoff_hash: None,
            turn_prompt: "recover this turn".to_owned(),
        }
    }

    async fn wait_for_run_status(store: &Store, run_id: RunId, status: RunStatus) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if store.load_run(run_id).await.unwrap().status == status {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    async fn wait_for_provider_calls(calls: &AtomicUsize, expected: usize) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while calls.load(Ordering::SeqCst) < expected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[test]
    fn provider_health_categories_preserve_actionable_unavailability() {
        assert_eq!(
            unavailable_category("login-required"),
            ProviderUnavailability::Unauthenticated
        );
        assert_eq!(
            unavailable_category("Codex is not authenticated"),
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

    #[test]
    fn canonical_question_ordinals_are_remapped_to_native_keys_only_at_dispatch() {
        let mut approval = Approval::new(
            RunId::new(),
            AgentId::new(),
            ProviderId::Codex,
            "provider-secret-question-request",
            "questions",
            "user",
        );
        approval.input = Some(crate::domain::UserInputRequest {
            questions: vec![crate::domain::UserInputQuestion {
                id: "provider-secret-question-key".to_owned(),
                header: "Choice".to_owned(),
                question: "Choose".to_owned(),
                options: None,
                is_other: false,
                is_secret: false,
            }],
            auto_resolution_ms: None,
        });

        let detail: ApprovalDetail = crate::store::ApprovalDetailRecord {
            id: approval.id,
            status: approval.status,
            response_pending: false,
            agent_path: vec!["Root".to_owned()],
            agent_path_truncated: false,
            operation: approval.operation.clone(),
            scope: approval.scope.clone(),
            input: approval.input.clone(),
            details: approval.details.clone(),
            question_count: approval
                .input
                .as_ref()
                .map_or(0, |input| input.questions.len() as u32),
            truncated: false,
        }
        .into();
        assert_eq!(
            detail.input.unwrap().questions[0].id,
            "question-1".to_owned()
        );

        let response = remap_approval_response(
            &approval,
            ApprovalResponse::Answers(BTreeMap::from([(
                "question-1".to_owned(),
                vec!["yes".to_owned()],
            )])),
        )
        .unwrap();

        assert_eq!(
            response,
            ApprovalResponse::Answers(BTreeMap::from([(
                "provider-secret-question-key".to_owned(),
                vec!["yes".to_owned()],
            )]))
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
    async fn one_conversation_overview_is_addressable_by_canonical_id() {
        let temporary = tempdir().unwrap();
        let app = PromptingTime::new(
            Store::open_in_memory().await.unwrap(),
            Router::default(),
            WorkspaceManager::new(temporary.path()),
            Vec::<Arc<dyn ProviderAdapter>>::new(),
        )
        .unwrap();
        let created = app
            .create_conversation_overview(ConversationRequest {
                title: "Targeted refresh".to_owned(),
                objective: String::new(),
                constraints: Vec::new(),
                workspace: ConversationWorkspace::Projectless,
                routing_profile: RoutingProfile::BestFit,
            })
            .await
            .unwrap();

        let loaded = app
            .load_conversation_overview(created.conversation.id)
            .await
            .unwrap();

        assert_eq!(loaded, created);
        assert_eq!(loaded.routing_profile, RoutingProfile::BestFit);
    }

    #[tokio::test]
    async fn startup_recovery_batches_a_deep_large_tree_child_before_parent() {
        let temporary = tempdir().unwrap();
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(crate::store::NewConversation::projectless(
                "large recovery tree",
            ))
            .await
            .unwrap();
        let (run, root) = store
            .create_run(conversation.id, ProviderId::Codex)
            .await
            .unwrap();
        store
            .bind_native_session(run.id, "root-native")
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();

        for batch in 0..4 {
            let start = batch * 55;
            let child_ids = (start..start + 55)
                .map(|index| format!("sibling-{index}"))
                .collect::<Vec<_>>();
            let statuses = child_ids
                .iter()
                .map(|id| crate::providers::NativeChildStatus {
                    native_thread_id: id.clone(),
                    status: crate::providers::NativeAgentStatus::Running,
                })
                .collect();
            store
                .append_run_event(
                    run.id,
                    root.id,
                    ProviderEventRecord::child_agent(
                        format!("spawn-siblings-{batch}"),
                        "root-native",
                        child_ids,
                        statuses,
                        "spawn agents",
                        "running",
                    ),
                )
                .await
                .unwrap();
        }
        let mut parent_native = "sibling-0".to_owned();
        for depth in 0..25 {
            let child_native = format!("deep-{depth}");
            store
                .append_run_event(
                    run.id,
                    root.id,
                    ProviderEventRecord::child_agent(
                        format!("spawn-deep-{depth}"),
                        &parent_native,
                        vec![child_native.clone()],
                        vec![crate::providers::NativeChildStatus {
                            native_thread_id: child_native.clone(),
                            status: crate::providers::NativeAgentStatus::Running,
                        }],
                        "spawn nested agent",
                        "running",
                    ),
                )
                .await
                .unwrap();
            parent_native = child_native;
        }

        let mut agents = Vec::new();
        let mut cursor = None;
        loop {
            let page = store
                .load_agent_page(conversation.id, cursor.take(), 200)
                .await
                .unwrap();
            agents.extend(page.items.into_iter().map(|item| item.agent));
            let Some(next) = page.next_cursor else { break };
            cursor = Some(next);
        }
        assert!(agents.len() > 200);
        let app = PromptingTime::new(
            store.clone(),
            Router::default(),
            WorkspaceManager::new(temporary.path()),
            Vec::<Arc<dyn ProviderAdapter>>::new(),
        )
        .unwrap();

        assert_eq!(app.reconcile_startup().await.unwrap(), 1);
        assert!(
            store
                .load_recovery_agent_batch(200)
                .await
                .unwrap()
                .is_empty()
        );

        let mut interrupted_at = HashMap::new();
        let mut event_cursor = None;
        loop {
            let page = store
                .load_timeline(conversation.id, event_cursor.take(), 200)
                .await
                .unwrap();
            for event in page.items {
                if event.content.ends_with("interrupted") {
                    interrupted_at.insert(event.agent_id, event.sequence);
                }
            }
            let Some(next) = page.next_cursor else { break };
            event_cursor = Some(next);
        }
        assert_eq!(interrupted_at.len(), agents.len());
        for agent in agents {
            if let Some(parent_id) = agent.parent_id {
                assert!(interrupted_at[&agent.id] < interrupted_at[&parent_id]);
            }
        }
        app.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn startup_requeues_a_persisted_queued_root_without_creating_another_run() {
        let temporary = tempdir().unwrap();
        let store = Store::open_in_memory().await.unwrap();
        let adapter = Arc::new(RecoveryAdapter::new());
        let app = PromptingTime::new(
            store.clone(),
            Router::default(),
            WorkspaceManager::new(temporary.path()),
            vec![adapter.clone()],
        )
        .unwrap();
        let conversation = app
            .create_conversation(ConversationRequest::projectless("queued recovery"))
            .await
            .unwrap();
        let PreparedSubmission::Created { run, .. } = store
            .prepare_submission(recovery_submission(conversation.id))
            .await
            .unwrap()
        else {
            panic!("fixture must create a queued run");
        };

        assert_eq!(app.reconcile_startup().await.unwrap(), 0);
        wait_for_run_status(&store, run.id, RunStatus::Completed).await;

        assert_eq!(adapter.starts.load(Ordering::SeqCst), 1);
        assert_eq!(adapter.resumes.load(Ordering::SeqCst), 0);
        assert_eq!(adapter.turns.load(Ordering::SeqCst), 1);
        assert_eq!(
            store.load_run(run.id).await.unwrap().dispatch_certainty,
            Some(DispatchCertainty::MayHaveDispatched)
        );
        assert_eq!(store.pending_recovery().await.unwrap().len(), 0);
        app.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn crash_after_provider_acceptance_never_dispatches_the_turn_twice() {
        let temporary = tempdir().unwrap();
        let store = Store::open_in_memory().await.unwrap();
        let (adapter, gate) = RecoveryAdapter::blocked();
        let adapter = Arc::new(adapter);
        let first = PromptingTime::new(
            store.clone(),
            Router::default(),
            WorkspaceManager::new(temporary.path()),
            vec![adapter.clone()],
        )
        .unwrap();
        let conversation = first
            .create_conversation(ConversationRequest::projectless("crash recovery"))
            .await
            .unwrap();
        let PreparedSubmission::Created { run, .. } = store
            .prepare_submission(recovery_submission(conversation.id))
            .await
            .unwrap()
        else {
            panic!("fixture must create a queued run");
        };

        assert_eq!(first.reconcile_startup().await.unwrap(), 0);
        wait_for_provider_calls(&adapter.starts, 1).await;
        first.supervisor.crash_for_test().await;

        let second = PromptingTime::new(
            store.clone(),
            Router::default(),
            WorkspaceManager::new(temporary.path()),
            vec![adapter.clone()],
        )
        .unwrap();
        assert_eq!(second.reconcile_startup().await.unwrap(), 0);
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                wait_for_provider_calls(&adapter.starts, 2),
            )
            .await
            .is_err()
        );
        assert_eq!(adapter.starts.load(Ordering::SeqCst), 1);
        assert_eq!(
            store.load_run(run.id).await.unwrap().dispatch_certainty,
            Some(DispatchCertainty::MayHaveDispatched)
        );
        assert_eq!(
            store.load_run(run.id).await.unwrap().status,
            RunStatus::Queued
        );
        store.expire_dispatch_lease_for_test(run.id).await.unwrap();
        assert_eq!(second.reconcile_abandoned_dispatches().await.unwrap(), 1);
        assert_eq!(adapter.starts.load(Ordering::SeqCst), 1);
        assert_eq!(
            store.load_run(run.id).await.unwrap().status,
            RunStatus::Interrupted
        );
        gate.add_permits(1);
        second.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn stale_dispatch_claim_is_interrupted_without_provider_redispatch() {
        let temporary = tempdir().unwrap();
        let store = Store::open_in_memory().await.unwrap();
        let adapter = Arc::new(RecoveryAdapter::new());
        let app = PromptingTime::new(
            store.clone(),
            Router::default(),
            WorkspaceManager::new(temporary.path()),
            vec![adapter.clone()],
        )
        .unwrap();
        let conversation = app
            .create_conversation(ConversationRequest::projectless("stale claim"))
            .await
            .unwrap();
        let PreparedSubmission::Created { run, .. } = store
            .prepare_submission(recovery_submission(conversation.id))
            .await
            .unwrap()
        else {
            panic!("fixture must create a queued run");
        };
        assert!(
            store
                .claim_provider_dispatch(
                    run.id,
                    "crashed-instance",
                    std::time::Duration::from_secs(15),
                )
                .await
                .unwrap()
        );
        store.expire_dispatch_lease_for_test(run.id).await.unwrap();

        assert_eq!(app.reconcile_startup().await.unwrap(), 0);
        assert_eq!(app.reconcile_abandoned_dispatches().await.unwrap(), 1);
        assert_eq!(adapter.starts.load(Ordering::SeqCst), 0);
        assert_eq!(adapter.resumes.load(Ordering::SeqCst), 0);
        assert_eq!(adapter.turns.load(Ordering::SeqCst), 0);
        assert_eq!(
            store.load_run(run.id).await.unwrap().status,
            RunStatus::Interrupted
        );
        app.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn failed_stale_reconciliation_can_be_reclaimed_by_the_same_app() {
        let temporary = tempdir().unwrap();
        let store = Store::open_in_memory().await.unwrap();
        let adapter = Arc::new(RecoveryAdapter::new());
        let app = PromptingTime::new(
            store.clone(),
            Router::default(),
            WorkspaceManager::new(temporary.path()),
            vec![adapter.clone()],
        )
        .unwrap();
        let conversation = app
            .create_conversation(ConversationRequest::projectless("retry stale recovery"))
            .await
            .unwrap();
        let PreparedSubmission::Created { run, .. } = store
            .prepare_submission(recovery_submission(conversation.id))
            .await
            .unwrap()
        else {
            panic!("fixture must create a queued run");
        };
        assert!(
            store
                .claim_provider_dispatch(
                    run.id,
                    "crashed-instance",
                    std::time::Duration::from_secs(15),
                )
                .await
                .unwrap()
        );
        store.expire_dispatch_lease_for_test(run.id).await.unwrap();
        store.reject_recovery_events_for_test().await.unwrap();

        assert!(app.reconcile_abandoned_dispatches().await.is_err());
        assert_eq!(adapter.starts.load(Ordering::SeqCst), 0);

        store.allow_recovery_events_for_test().await.unwrap();
        store.expire_dispatch_lease_for_test(run.id).await.unwrap();
        assert_eq!(app.reconcile_abandoned_dispatches().await.unwrap(), 1);
        assert_eq!(
            store.load_run(run.id).await.unwrap().status,
            RunStatus::Interrupted
        );
        assert_eq!(adapter.starts.load(Ordering::SeqCst), 0);
        app.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn active_supervisor_renews_its_dispatch_lease() {
        let temporary = tempdir().unwrap();
        let store = Store::open_in_memory().await.unwrap();
        let (adapter, gate) = RecoveryAdapter::blocked();
        let adapter = Arc::new(adapter);
        let app = PromptingTime::new(
            store.clone(),
            Router::default(),
            WorkspaceManager::new(temporary.path()),
            vec![adapter.clone()],
        )
        .unwrap();
        let conversation = app
            .create_conversation(ConversationRequest::projectless("lease renewal"))
            .await
            .unwrap();
        let PreparedSubmission::Created { run, .. } = store
            .prepare_submission(recovery_submission(conversation.id))
            .await
            .unwrap()
        else {
            panic!("fixture must create a queued run");
        };

        assert_eq!(app.reconcile_startup().await.unwrap(), 0);
        wait_for_provider_calls(&adapter.starts, 1).await;
        store.delay_dispatch_lease_for_test(run.id).await.unwrap();
        let observer = PromptingTime::new(
            store.clone(),
            Router::default(),
            WorkspaceManager::new(temporary.path()),
            vec![adapter.clone()],
        )
        .unwrap();
        assert_eq!(observer.reconcile_startup().await.unwrap(), 0);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        assert!(
            store
                .has_protected_provider_dispatch_lease(run.id, DISPATCH_LEASE_STALE_GRACE,)
                .await
                .unwrap()
        );
        gate.add_permits(1);
        wait_for_run_status(&store, run.id, RunStatus::Completed).await;
        observer.shutdown().await.unwrap();
        app.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn concurrent_startup_reconciliation_claims_one_provider_dispatch() {
        let temporary = tempdir().unwrap();
        let store = Store::open_in_memory().await.unwrap();
        let (adapter, gate) = RecoveryAdapter::blocked();
        let adapter = Arc::new(adapter);
        let first = PromptingTime::new(
            store.clone(),
            Router::default(),
            WorkspaceManager::new(temporary.path()),
            vec![adapter.clone()],
        )
        .unwrap();
        let second = PromptingTime::new(
            store.clone(),
            Router::default(),
            WorkspaceManager::new(temporary.path()),
            vec![adapter.clone()],
        )
        .unwrap();
        let conversation = first
            .create_conversation(ConversationRequest::projectless("concurrent recovery"))
            .await
            .unwrap();
        let PreparedSubmission::Created { run, .. } = store
            .prepare_submission(recovery_submission(conversation.id))
            .await
            .unwrap()
        else {
            panic!("fixture must create a queued run");
        };
        let claim_barrier = Arc::new(Barrier::new(2));
        first
            .supervisor
            .synchronize_recovery_claim_for_test(Arc::clone(&claim_barrier));
        second
            .supervisor
            .synchronize_recovery_claim_for_test(claim_barrier);

        let (first_recovery, second_recovery) =
            tokio::join!(first.reconcile_startup(), second.reconcile_startup());
        assert_eq!(first_recovery.unwrap(), 0);
        assert_eq!(second_recovery.unwrap(), 0);
        wait_for_provider_calls(&adapter.starts, 1).await;
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                wait_for_provider_calls(&adapter.starts, 2),
            )
            .await
            .is_err()
        );
        assert_eq!(adapter.starts.load(Ordering::SeqCst), 1);

        gate.add_permits(1);
        wait_for_run_status(&store, run.id, RunStatus::Completed).await;
        first.shutdown().await.unwrap();
        second.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn dispatch_claim_transaction_failure_makes_zero_provider_calls() {
        let temporary = tempdir().unwrap();
        let store = Store::open_in_memory().await.unwrap();
        let adapter = Arc::new(RecoveryAdapter::new());
        let app = PromptingTime::new(
            store.clone(),
            Router::default(),
            WorkspaceManager::new(temporary.path()),
            vec![adapter.clone()],
        )
        .unwrap();
        let conversation = app
            .create_conversation(ConversationRequest::projectless("claim failure"))
            .await
            .unwrap();
        let PreparedSubmission::Created { .. } = store
            .prepare_submission(recovery_submission(conversation.id))
            .await
            .unwrap()
        else {
            panic!("fixture must create a queued run");
        };
        store.reject_dispatch_claims_for_test().await.unwrap();

        assert!(app.reconcile_startup().await.is_err());
        assert_eq!(adapter.starts.load(Ordering::SeqCst), 0);
        assert_eq!(adapter.resumes.load(Ordering::SeqCst), 0);
        assert_eq!(adapter.turns.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn submission_and_dispatch_claim_roll_back_together_on_transaction_failure() {
        let temporary = tempdir().unwrap();
        let store = Store::open_in_memory().await.unwrap();
        let adapter = Arc::new(RecoveryAdapter::new());
        let app = PromptingTime::new(
            store.clone(),
            Router::default(),
            WorkspaceManager::new(temporary.path()),
            vec![adapter.clone()],
        )
        .unwrap();
        let conversation = app
            .create_conversation(ConversationRequest::projectless("atomic submission claim"))
            .await
            .unwrap();
        let request = SubmitRequest {
            command_id: "atomic-claim-command".to_owned(),
            conversation_id: conversation.id,
            content: "run after the transaction succeeds".to_owned(),
            provider_override: Some(ProviderId::Codex),
        };
        store.reject_dispatch_claims_for_test().await.unwrap();

        assert!(app.submit(request.clone()).await.is_err());
        assert!(
            store
                .load_submission(&request.command_id)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(adapter.starts.load(Ordering::SeqCst), 0);

        store.allow_dispatch_claims_for_test().await.unwrap();
        let submitted = app.submit(request).await.unwrap();
        assert!(!submitted.duplicate);
        wait_for_run_status(&store, submitted.handle.run_id(), RunStatus::Completed).await;
        assert_eq!(adapter.starts.load(Ordering::SeqCst), 1);
        app.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn pending_claims_are_renewed_before_root_capacity_is_available() {
        let temporary = tempdir().unwrap();
        let store = Store::open_in_memory().await.unwrap();
        let (adapter, gate) = RecoveryAdapter::blocked();
        let adapter = Arc::new(adapter);
        let app = PromptingTime::new(
            store.clone(),
            Router::default(),
            WorkspaceManager::new(temporary.path()),
            vec![adapter.clone()],
        )
        .unwrap();
        let mut runs = Vec::new();
        for index in 0..5 {
            let conversation = app
                .create_conversation(ConversationRequest::projectless(format!(
                    "pending lease {index}"
                )))
                .await
                .unwrap();
            let PreparedSubmission::Created { run, .. } = store
                .prepare_submission(recovery_submission(conversation.id))
                .await
                .unwrap()
            else {
                panic!("fixture must create a queued run");
            };
            runs.push(run.id);
        }

        assert_eq!(app.reconcile_startup().await.unwrap(), 0);
        wait_for_provider_calls(&adapter.starts, 4).await;
        let pending = *runs.last().unwrap();
        store.delay_dispatch_lease_for_test(pending).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(
            store
                .has_protected_provider_dispatch_lease(pending, DISPATCH_LEASE_STALE_GRACE,)
                .await
                .unwrap()
        );

        gate.add_permits(5);
        for run_id in runs {
            wait_for_run_status(&store, run_id, RunStatus::Completed).await;
        }
        assert_eq!(adapter.starts.load(Ordering::SeqCst), 5);
        app.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn interrupted_pending_claim_is_fenced_before_provider_dispatch() {
        let temporary = tempdir().unwrap();
        let store = Store::open_in_memory().await.unwrap();
        let (adapter, gate) = RecoveryAdapter::blocked();
        let adapter = Arc::new(adapter);
        let app = PromptingTime::new(
            store.clone(),
            Router::default(),
            WorkspaceManager::new(temporary.path()),
            vec![adapter.clone()],
        )
        .unwrap();
        let mut runs = Vec::new();
        for index in 0..5 {
            let conversation = app
                .create_conversation(ConversationRequest::projectless(format!(
                    "pending fence {index}"
                )))
                .await
                .unwrap();
            let PreparedSubmission::Created { run, root } = store
                .prepare_submission(recovery_submission(conversation.id))
                .await
                .unwrap()
            else {
                panic!("fixture must create a queued run");
            };
            runs.push((run.id, root.id));
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }

        assert_eq!(app.reconcile_startup().await.unwrap(), 0);
        wait_for_provider_calls(&adapter.starts, 4).await;
        let (pending_run, pending_root) = *runs.last().unwrap();
        store
            .append_run_event(
                pending_run,
                pending_root,
                ProviderEventRecord::interrupted(),
            )
            .await
            .unwrap();

        gate.add_permits(4);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(adapter.starts.load(Ordering::SeqCst), 4);
        assert_eq!(
            store.load_run(pending_run).await.unwrap().status,
            RunStatus::Interrupted
        );
        let _ = app.shutdown().await;
    }

    #[tokio::test]
    async fn startup_does_not_replay_a_queued_turn_after_a_session_was_bound() {
        let temporary = tempdir().unwrap();
        let store = Store::open_in_memory().await.unwrap();
        let adapter = Arc::new(RecoveryAdapter::new());
        let app = PromptingTime::new(
            store.clone(),
            Router::default(),
            WorkspaceManager::new(temporary.path()),
            vec![adapter.clone()],
        )
        .unwrap();
        let conversation = app
            .create_conversation(ConversationRequest::projectless("queued session recovery"))
            .await
            .unwrap();
        let PreparedSubmission::Created { run, .. } = store
            .prepare_submission(recovery_submission(conversation.id))
            .await
            .unwrap()
        else {
            panic!("fixture must create a queued run");
        };
        store
            .bind_native_session(run.id, "retained-session")
            .await
            .unwrap();

        assert_eq!(app.reconcile_startup().await.unwrap(), 1);

        assert_eq!(adapter.starts.load(Ordering::SeqCst), 0);
        assert_eq!(adapter.resumes.load(Ordering::SeqCst), 0);
        assert_eq!(adapter.turns.load(Ordering::SeqCst), 0);
        assert_eq!(
            store.load_run(run.id).await.unwrap().status,
            RunStatus::Interrupted
        );
        app.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn startup_does_not_replay_an_ambiguous_running_turn() {
        let temporary = tempdir().unwrap();
        let store = Store::open_in_memory().await.unwrap();
        let adapter = Arc::new(RecoveryAdapter::new());
        let app = PromptingTime::new(
            store.clone(),
            Router::default(),
            WorkspaceManager::new(temporary.path()),
            vec![adapter.clone()],
        )
        .unwrap();
        let conversation = app
            .create_conversation(ConversationRequest::projectless("ambiguous recovery"))
            .await
            .unwrap();
        let PreparedSubmission::Created { run, root } = store
            .prepare_submission(recovery_submission(conversation.id))
            .await
            .unwrap()
        else {
            panic!("fixture must create a queued run");
        };
        store
            .bind_native_session(run.id, "retained-session")
            .await
            .unwrap();
        store
            .append_run_event(run.id, root.id, ProviderEventRecord::started())
            .await
            .unwrap();

        assert_eq!(app.reconcile_startup().await.unwrap(), 1);
        assert_eq!(
            store.load_run(run.id).await.unwrap().status,
            RunStatus::Interrupted
        );
        assert_eq!(adapter.starts.load(Ordering::SeqCst), 0);
        assert_eq!(adapter.resumes.load(Ordering::SeqCst), 0);
        assert_eq!(adapter.turns.load(Ordering::SeqCst), 0);
        assert!(
            store
                .load_timeline(conversation.id, None, 20)
                .await
                .unwrap()
                .items
                .iter()
                .any(|event| event.content.contains("Review its output and workspace"))
        );
        app.shutdown().await.unwrap();
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
