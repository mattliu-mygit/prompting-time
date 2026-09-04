use super::*;

impl From<&StateProviderDiagnostic> for ProviderInstallation {
    fn from(value: &StateProviderDiagnostic) -> Self {
        let diagnostic = match (&value.diagnostic, &value.action) {
            (Some(diagnostic), Some(action)) => Some(format!("{diagnostic} {action}")),
            (Some(diagnostic), None) => Some(diagnostic.clone()),
            (None, Some(action)) => Some(action.clone()),
            (None, None) => None,
        };
        Self {
            id: value.id.into(),
            installed: value.installed,
            available: value.available,
            version: value.version.clone(),
            diagnostic,
            capabilities: provider_capabilities(&value.capabilities),
        }
    }
}

fn provider_capabilities(
    capabilities: &prompting_time_core::providers::ProviderCapabilities,
) -> Vec<ProviderCapability> {
    [
        CoreProviderCapability::Streaming,
        CoreProviderCapability::Steering,
        CoreProviderCapability::DeferredApproval,
        CoreProviderCapability::Interruption,
        CoreProviderCapability::Resume,
        CoreProviderCapability::ChildAgents,
    ]
    .into_iter()
    .filter(|capability| capabilities.supports(*capability))
    .map(Into::into)
    .collect()
}

impl From<CoreProviderId> for ProviderId {
    fn from(value: CoreProviderId) -> Self {
        match value {
            CoreProviderId::Codex => Self::Codex,
            CoreProviderId::Claude => Self::Claude,
        }
    }
}

impl From<ProviderId> for CoreProviderId {
    fn from(value: ProviderId) -> Self {
        match value {
            ProviderId::Codex => Self::Codex,
            ProviderId::Claude => Self::Claude,
        }
    }
}

impl From<RoutingProfile> for CoreRoutingProfile {
    fn from(value: RoutingProfile) -> Self {
        match value {
            RoutingProfile::Balanced => Self::Balanced,
            RoutingProfile::BestFit => Self::BestFit,
            RoutingProfile::UsageBalance => Self::UsageBalance,
        }
    }
}

impl From<CreateConversationRequest> for CoreConversationRequest {
    fn from(value: CreateConversationRequest) -> Self {
        Self {
            title: value.title,
            objective: value.objective,
            constraints: value.constraints,
            workspace: match value.workspace {
                ConversationWorkspaceRequest::Projectless => ConversationWorkspace::Projectless,
                ConversationWorkspaceRequest::Isolated { path } => {
                    ConversationWorkspace::Isolated(PathBuf::from(path))
                }
                ConversationWorkspaceRequest::Direct { path } => {
                    ConversationWorkspace::Direct(PathBuf::from(path))
                }
            },
            routing_profile: value.routing_profile.into(),
        }
    }
}

impl From<ConversationOverview> for ConversationSummary {
    fn from(value: ConversationOverview) -> Self {
        Self {
            id: value.conversation.id.to_string(),
            title: value.conversation.title,
            routing_profile: match value.routing_profile {
                CoreRoutingProfile::Balanced => RoutingProfile::Balanced,
                CoreRoutingProfile::BestFit => RoutingProfile::BestFit,
                CoreRoutingProfile::UsageBalance => RoutingProfile::UsageBalance,
            },
            workspace_id: value.conversation.workspace_id.map(|id| id.to_string()),
            archived: value.conversation.archived,
            project_root: value
                .project_root
                .map(|path| path.to_string_lossy().into_owned()),
            current_run_id: value.run.as_ref().map(|run| run.id.to_string()),
            provider: value.run.as_ref().map(|run| run.provider.into()),
            run_status: value.run.as_ref().map(|run| run.status.into()),
            rollup_status: value.rollup_status.map(Into::into),
            agents: value.agents.into_iter().map(Into::into).collect(),
            agents_truncated: value.agents_truncated,
        }
    }
}

impl From<Page<ConversationOverview>> for ConversationPage {
    fn from(value: Page<ConversationOverview>) -> Self {
        Self {
            items: value.items.into_iter().map(Into::into).collect(),
            next_cursor: value.next_cursor,
        }
    }
}

impl From<AgentNode> for AgentSnapshot {
    fn from(value: AgentNode) -> Self {
        Self {
            id: value.id.to_string(),
            parent_id: value.parent_id.map(|id| id.to_string()),
            provider: value.provider.into(),
            label: value.label,
            summary: value.summary,
            status: value.status.into(),
        }
    }
}

impl From<prompting_time_core::store::AgentPage> for AgentTreePage {
    fn from(value: prompting_time_core::store::AgentPage) -> Self {
        Self {
            run_id: value.run_id.map(|id| id.to_string()),
            items: value
                .items
                .into_iter()
                .map(|record| AgentTreeItem {
                    agent: record.agent.into(),
                    depth: record.depth,
                })
                .collect(),
            next_cursor: value.next_cursor,
        }
    }
}

impl From<CoreAgentStatus> for AgentStatus {
    fn from(value: CoreAgentStatus) -> Self {
        match value {
            CoreAgentStatus::Queued => Self::Queued,
            CoreAgentStatus::Running => Self::Running,
            CoreAgentStatus::Waiting => Self::Waiting,
            CoreAgentStatus::Completed => Self::Completed,
            CoreAgentStatus::Interrupted => Self::Interrupted,
            CoreAgentStatus::Failed => Self::Failed,
        }
    }
}

impl From<CoreRollupStatus> for RollupStatus {
    fn from(value: CoreRollupStatus) -> Self {
        match value {
            CoreRollupStatus::NeedsAttention => Self::NeedsAttention,
            CoreRollupStatus::Active => Self::Active,
            CoreRollupStatus::Failed => Self::Failed,
            CoreRollupStatus::Interrupted => Self::Interrupted,
            CoreRollupStatus::Completed => Self::Completed,
        }
    }
}

impl From<CoreTimelineEventKind> for TimelineItemKind {
    fn from(value: CoreTimelineEventKind) -> Self {
        match value {
            CoreTimelineEventKind::Message => Self::Message,
            CoreTimelineEventKind::Tool => Self::Tool,
            CoreTimelineEventKind::Progress => Self::Progress,
            CoreTimelineEventKind::Diagnostic => Self::Diagnostic,
            CoreTimelineEventKind::Lifecycle => Self::Lifecycle,
        }
    }
}

impl From<CoreMessageRole> for MessageRole {
    fn from(value: CoreMessageRole) -> Self {
        match value {
            CoreMessageRole::User => Self::User,
            CoreMessageRole::Assistant => Self::Assistant,
        }
    }
}

impl From<TimelineRecord> for TimelineItem {
    fn from(value: TimelineRecord) -> Self {
        Self {
            id: value.event.id.to_string(),
            conversation_id: value.event.conversation_id.to_string(),
            run_id: value.event.run_id.to_string(),
            agent_id: value.event.agent_id.to_string(),
            sequence: value.event.sequence.to_string(),
            kind: value.event.kind.into(),
            role: value.event.role.map(Into::into),
            content: value.event.content,
            content_bytes: value.content_bytes.to_string(),
            truncated: value.content_truncated,
            provider: value.provider.into(),
        }
    }
}

impl From<CoreTimelineSnapshot> for TimelinePage {
    fn from(value: CoreTimelineSnapshot) -> Self {
        Self {
            items: value.events.items.into_iter().map(Into::into).collect(),
            next_cursor: value.events.next_cursor,
            approvals: value.approvals.items.into_iter().map(Into::into).collect(),
            approvals_truncated: value.approvals.truncated,
            approvals_next_cursor: value.approvals.next_cursor,
        }
    }
}

impl From<CoreEventDetail> for EventDetailSnapshot {
    fn from(value: CoreEventDetail) -> Self {
        Self {
            id: value.id.to_string(),
            content: value.content,
            content_bytes: value.content_bytes.to_string(),
            truncated: value.truncated,
        }
    }
}

impl From<CoreRunStatus> for RunStatus {
    fn from(value: CoreRunStatus) -> Self {
        match value {
            CoreRunStatus::Queued => Self::Queued,
            CoreRunStatus::Running => Self::Running,
            CoreRunStatus::Waiting => Self::Waiting,
            CoreRunStatus::Completed => Self::Completed,
            CoreRunStatus::Interrupted => Self::Interrupted,
            CoreRunStatus::Failed => Self::Failed,
        }
    }
}

impl From<ApprovalResponse> for CoreApprovalResponse {
    fn from(value: ApprovalResponse) -> Self {
        match value {
            ApprovalResponse::Approved => Self::Approved,
            ApprovalResponse::Denied => Self::Denied,
            ApprovalResponse::Answer(answer) => Self::Answer(answer),
            ApprovalResponse::Answers(answers) => Self::Answers(answers),
        }
    }
}

impl From<prompting_time_core::store::ApprovalSummary> for ApprovalSnapshot {
    fn from(value: prompting_time_core::store::ApprovalSummary) -> Self {
        Self {
            id: value.id.to_string(),
            run_id: value.run_id.to_string(),
            agent_id: value.agent_id.to_string(),
            provider: value.provider.into(),
            operation: truncate_utf8(value.operation, 256),
            scope: truncate_utf8(value.scope, 512),
            status: value.status.into(),
            response_pending: value.response_pending,
            agent_path: value.agent_path,
            agent_path_truncated: value.agent_path_truncated,
        }
    }
}

impl From<prompting_time_core::store::ApprovalPage> for ApprovalPage {
    fn from(value: prompting_time_core::store::ApprovalPage) -> Self {
        Self {
            items: value.items.into_iter().map(Into::into).collect(),
            next_cursor: value.next_cursor,
        }
    }
}

impl ApprovalDetailSnapshot {
    pub(crate) fn from_core(value: CoreApprovalDetail) -> Self {
        const MAX_DETAIL_BYTES: usize = 256 * 1024;
        let mut detail = Self {
            id: value.id.to_string(),
            status: value.status.into(),
            response_pending: value.response_pending,
            agent_path: value.agent_path,
            agent_path_truncated: value.agent_path_truncated,
            operation: value.operation,
            scope: value.scope,
            input: value.input.map(|input| UserInputRequest {
                questions: input
                    .questions
                    .into_iter()
                    .map(|question| UserInputQuestion {
                        id: question.id,
                        header: question.header,
                        question: question.question,
                        options: question.options.map(|options| {
                            options
                                .into_iter()
                                .map(|option| UserInputOption {
                                    label: option.label,
                                    description: option.description,
                                })
                                .collect()
                        }),
                        is_other: question.is_other,
                        is_secret: question.is_secret,
                    })
                    .collect(),
                auto_resolution_ms: input.auto_resolution_ms.map(|value| value.to_string()),
            }),
            details: value.details.map(Into::into),
            question_count: value.question_count,
            truncated: value.truncated,
        };
        if serde_json::to_vec(&detail).is_ok_and(|encoded| encoded.len() > MAX_DETAIL_BYTES) {
            detail.operation = truncate_utf8(detail.operation, 256);
            detail.scope = truncate_utf8(detail.scope, 512);
            detail.input = None;
            detail.details = None;
            detail.truncated = true;
        }
        detail
    }
}

impl From<prompting_time_core::store::ApprovalQuestionPreview> for ApprovalQuestionPreview {
    fn from(value: prompting_time_core::store::ApprovalQuestionPreview) -> Self {
        Self {
            id: value.id,
            header: value.header,
            question: value.question,
            options: value.options.map(|options| {
                options
                    .into_iter()
                    .map(|option| UserInputOption {
                        label: option.label,
                        description: option.description,
                    })
                    .collect()
            }),
            is_other: value.is_other,
            is_secret: value.is_secret,
            truncated: value.truncated,
        }
    }
}

impl From<prompting_time_core::store::ApprovalQuestionPage> for ApprovalQuestionPage {
    fn from(value: prompting_time_core::store::ApprovalQuestionPage) -> Self {
        Self {
            items: value.items.into_iter().map(Into::into).collect(),
            total_count: value.total_count,
            next_cursor: value.next_cursor,
        }
    }
}

fn truncate_utf8(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value
}

impl From<CoreApprovalStatus> for ApprovalStatus {
    fn from(value: CoreApprovalStatus) -> Self {
        match value {
            CoreApprovalStatus::Pending => Self::Pending,
            CoreApprovalStatus::Approved => Self::Approved,
            CoreApprovalStatus::Denied => Self::Denied,
            CoreApprovalStatus::Answered => Self::Answered,
            CoreApprovalStatus::Cancelled => Self::Cancelled,
            CoreApprovalStatus::Failed => Self::Failed,
        }
    }
}

impl From<CoreApprovalRequestDetails> for ApprovalDetails {
    fn from(value: CoreApprovalRequestDetails) -> Self {
        match value {
            CoreApprovalRequestDetails::CommandExecution { command, cwd } => {
                Self::CommandExecution { command, cwd }
            }
            CoreApprovalRequestDetails::FileChange {
                changes,
                grant_root,
                reason,
            } => Self::FileChange {
                changes: changes
                    .into_iter()
                    .map(|change| FileChange {
                        path: change.path,
                        change: change.change.into(),
                    })
                    .collect(),
                grant_root,
                reason,
            },
            CoreApprovalRequestDetails::PermissionProfile { cwd, profile } => {
                let file_system = profile.file_system;
                Self::PermissionProfile {
                    cwd,
                    profile: PermissionProfile {
                        entries: file_system.as_ref().and_then(|permissions| {
                            permissions.entries.as_ref().map(|entries| {
                                entries
                                    .iter()
                                    .cloned()
                                    .map(|entry| PermissionEntry {
                                        access: entry.access.into(),
                                        path: entry.path.into(),
                                    })
                                    .collect()
                            })
                        }),
                        glob_scan_max_depth: file_system
                            .as_ref()
                            .and_then(|permissions| permissions.glob_scan_max_depth)
                            .map(|value| value.to_string()),
                        read: file_system
                            .as_ref()
                            .and_then(|permissions| permissions.read.clone()),
                        write: file_system
                            .as_ref()
                            .and_then(|permissions| permissions.write.clone()),
                        network_enabled: profile.network.and_then(|network| network.enabled),
                    },
                }
            }
        }
    }
}

impl From<CoreFileChangeKind> for FileChangeKind {
    fn from(value: CoreFileChangeKind) -> Self {
        match value {
            CoreFileChangeKind::Add => Self::Add,
            CoreFileChangeKind::Delete => Self::Delete,
            CoreFileChangeKind::Update { move_path } => Self::Update { move_path },
        }
    }
}

impl From<CoreFileSystemAccess> for FileSystemAccess {
    fn from(value: CoreFileSystemAccess) -> Self {
        match value {
            CoreFileSystemAccess::Read => Self::Read,
            CoreFileSystemAccess::Write => Self::Write,
            CoreFileSystemAccess::Deny => Self::Deny,
        }
    }
}

impl From<CoreFileSystemPath> for FileSystemPath {
    fn from(value: CoreFileSystemPath) -> Self {
        match value {
            CoreFileSystemPath::Path { path } => Self::Path { path },
            CoreFileSystemPath::GlobPattern { pattern } => Self::GlobPattern { pattern },
            CoreFileSystemPath::Special { value } => Self::Special {
                value: value.into(),
            },
        }
    }
}

impl From<CoreSpecialPath> for SpecialPath {
    fn from(value: CoreSpecialPath) -> Self {
        match value {
            CoreSpecialPath::Root => Self::Root,
            CoreSpecialPath::Minimal => Self::Minimal,
            CoreSpecialPath::ProjectRoots { subpath } => Self::ProjectRoots { subpath },
            CoreSpecialPath::Tmpdir => Self::Tmpdir,
            CoreSpecialPath::SlashTmp => Self::SlashTmp,
            CoreSpecialPath::Unknown { path, subpath } => Self::Unknown { path, subpath },
        }
    }
}

impl From<CoreWorkspaceSnapshot> for WorkspaceSnapshot {
    fn from(value: CoreWorkspaceSnapshot) -> Self {
        Self {
            mode: match value.mode {
                CoreWorkspaceMode::Projectless => WorkspaceMode::Projectless,
                CoreWorkspaceMode::Direct => WorkspaceMode::Direct,
                CoreWorkspaceMode::Isolated => WorkspaceMode::Isolated,
            },
            changes: value
                .changes
                .into_iter()
                .map(|change| WorkspaceChange {
                    kind: match change.kind {
                        CoreWorkspaceChangeKind::Present => WorkspaceChangeKind::Present,
                        CoreWorkspaceChangeKind::Added => WorkspaceChangeKind::Added,
                        CoreWorkspaceChangeKind::Modified => WorkspaceChangeKind::Modified,
                        CoreWorkspaceChangeKind::Deleted => WorkspaceChangeKind::Deleted,
                        CoreWorkspaceChangeKind::Renamed => WorkspaceChangeKind::Renamed,
                        CoreWorkspaceChangeKind::Copied => WorkspaceChangeKind::Copied,
                        CoreWorkspaceChangeKind::Untracked => WorkspaceChangeKind::Untracked,
                        CoreWorkspaceChangeKind::Conflicted => WorkspaceChangeKind::Conflicted,
                    },
                    relative_path: change.relative_path,
                })
                .collect(),
            truncated: value.truncated,
        }
    }
}

impl From<CoreInspectorSnapshot> for InspectorSnapshot {
    fn from(value: CoreInspectorSnapshot) -> Self {
        let cleanup = match value.cleanup {
            CoreCleanupEligibility::Eligible => CleanupSnapshot {
                eligible: true,
                blocker: None,
            },
            CoreCleanupEligibility::Blocked(blocker) => CleanupSnapshot {
                eligible: false,
                blocker: Some(blocker.into()),
            },
        };
        Self {
            workspace: value.workspace.into(),
            execution_path: value.execution_path.to_string_lossy().into_owned(),
            owned_worktree: value.owned_worktree,
            cleanup,
            current_run: value.run.map(|run| CurrentRunSnapshot {
                id: run.id.to_string(),
                provider: run.provider.into(),
                status: run.status.into(),
            }),
            routing: value.routing.map(routing_snapshot),
            handoff: value.handoff,
            active_descendant_count: u32::try_from(value.active_descendant_count)
                .unwrap_or(u32::MAX),
            agents_truncated: value.agents_truncated,
        }
    }
}

impl From<prompting_time_core::store::RunAuditPage> for RunAuditPage {
    fn from(value: prompting_time_core::store::RunAuditPage) -> Self {
        Self {
            items: value
                .items
                .into_iter()
                .map(|run| RunAuditSummarySnapshot {
                    id: run.id.to_string(),
                    provider: run.provider.into(),
                    status: run.status.into(),
                    reason: run.reason.map(Into::into),
                    routing_truncated: run.routing_truncated,
                    has_handoff: run.has_handoff,
                })
                .collect(),
            next_cursor: value.next_cursor,
        }
    }
}

impl From<prompting_time_core::store::RunAuditDetailRecord> for RunAuditDetailSnapshot {
    fn from(value: prompting_time_core::store::RunAuditDetailRecord) -> Self {
        Self {
            id: value.id.to_string(),
            provider: value.provider.into(),
            status: value.status.into(),
            routing: value.routing.map(routing_snapshot),
            reason: value.reason.map(Into::into),
            routing_truncated: value.routing_truncated,
            handoff: value.handoff,
            handoff_truncated: value.handoff_truncated,
        }
    }
}

fn routing_snapshot(routing: prompting_time_core::router::RoutingDecision) -> RoutingSnapshot {
    let capabilities = provider_capabilities(&routing.required_capabilities);
    RoutingSnapshot {
        provider: routing.provider.into(),
        profile: match routing.profile {
            CoreRoutingProfile::Balanced => RoutingProfile::Balanced,
            CoreRoutingProfile::BestFit => RoutingProfile::BestFit,
            CoreRoutingProfile::UsageBalance => RoutingProfile::UsageBalance,
        },
        reason: routing.reason.into(),
        task_kind: routing.task_kind.into(),
        override_provider: routing.override_provider.map(Into::into),
        eligible_providers: routing
            .eligible_providers
            .into_iter()
            .map(Into::into)
            .collect(),
        required_capabilities: capabilities,
        evaluations: routing.evaluations.into_iter().map(Into::into).collect(),
        rationale: routing.rationale.into_iter().map(Into::into).collect(),
        explanation: routing.explanation,
    }
}

impl From<CoreWorkspaceBlocker> for CleanupBlocker {
    fn from(value: CoreWorkspaceBlocker) -> Self {
        match value {
            CoreWorkspaceBlocker::NotOwned => Self::NotOwned,
            CoreWorkspaceBlocker::MissingWorktree => Self::MissingWorktree,
            CoreWorkspaceBlocker::ModifiedTrackedFiles => Self::ModifiedTrackedFiles,
            CoreWorkspaceBlocker::UntrackedFiles => Self::UntrackedFiles,
            CoreWorkspaceBlocker::UniqueCommits => Self::UniqueCommits,
            CoreWorkspaceBlocker::ActiveProcess => Self::ActiveProcess,
        }
    }
}

impl From<CoreRoutingReason> for RoutingReason {
    fn from(value: CoreRoutingReason) -> Self {
        match value {
            CoreRoutingReason::ManualOverride => Self::ManualOverride,
            CoreRoutingReason::RequiredCapabilities => Self::RequiredCapabilities,
            CoreRoutingReason::Continuity => Self::Continuity,
            CoreRoutingReason::OnlyEligibleProvider => Self::OnlyEligibleProvider,
            CoreRoutingReason::LeastUsed => Self::LeastUsed,
            CoreRoutingReason::DeterministicTieBreak => Self::DeterministicTieBreak,
            CoreRoutingReason::SafeFallback => Self::SafeFallback,
        }
    }
}

impl From<CoreTaskKind> for TaskKind {
    fn from(value: CoreTaskKind) -> Self {
        match value {
            CoreTaskKind::Implementation => Self::Implementation,
            CoreTaskKind::Review => Self::Review,
            CoreTaskKind::Research => Self::Research,
            CoreTaskKind::General => Self::General,
        }
    }
}

impl From<CoreProviderCapability> for ProviderCapability {
    fn from(value: CoreProviderCapability) -> Self {
        match value {
            CoreProviderCapability::Streaming => Self::Streaming,
            CoreProviderCapability::Steering => Self::Steering,
            CoreProviderCapability::DeferredApproval => Self::DeferredApproval,
            CoreProviderCapability::Interruption => Self::Interruption,
            CoreProviderCapability::Resume => Self::Resume,
            CoreProviderCapability::ChildAgents => Self::ChildAgents,
        }
    }
}

impl From<CoreProviderEvaluation> for ProviderEvaluation {
    fn from(value: CoreProviderEvaluation) -> Self {
        Self {
            provider: value.provider.into(),
            eligible: value.eligible,
            blockers: value.blockers.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<CoreRoutingBlocker> for RoutingBlocker {
    fn from(value: CoreRoutingBlocker) -> Self {
        match value {
            CoreRoutingBlocker::Unavailable(reason) => Self::Unavailable(reason.into()),
            CoreRoutingBlocker::MissingCapability(capability) => {
                Self::MissingCapability(capability.into())
            }
            CoreRoutingBlocker::NotReported => Self::NotReported,
        }
    }
}

impl From<CoreProviderUnavailability> for ProviderUnavailability {
    fn from(value: CoreProviderUnavailability) -> Self {
        match value {
            CoreProviderUnavailability::NotInstalled => Self::NotInstalled,
            CoreProviderUnavailability::UnsupportedVersion => Self::UnsupportedVersion,
            CoreProviderUnavailability::Unauthenticated => Self::Unauthenticated,
            CoreProviderUnavailability::Unhealthy => Self::Unhealthy,
            CoreProviderUnavailability::QuotaBlocked => Self::QuotaBlocked,
        }
    }
}

impl From<CoreProviderRank> for ProviderRank {
    fn from(value: CoreProviderRank) -> Self {
        Self {
            provider: value.provider.into(),
            recent_root_runs: value.recent_root_runs.to_string(),
            stable_order: value.stable_order,
        }
    }
}

impl From<CoreRoutingCriterion> for RoutingCriterion {
    fn from(value: CoreRoutingCriterion) -> Self {
        match value {
            CoreRoutingCriterion::ManualOverride { provider } => Self::ManualOverride {
                provider: provider.into(),
            },
            CoreRoutingCriterion::EligibleProviders { providers } => Self::EligibleProviders {
                providers: providers.into_iter().map(Into::into).collect(),
            },
            CoreRoutingCriterion::RequiredCapabilities { capabilities } => {
                Self::RequiredCapabilities {
                    capabilities: provider_capabilities(&capabilities),
                }
            }
            CoreRoutingCriterion::Continuity { provider } => Self::Continuity {
                provider: provider.into(),
            },
            CoreRoutingCriterion::RankedCandidates { candidates } => Self::RankedCandidates {
                candidates: candidates.into_iter().map(Into::into).collect(),
            },
            CoreRoutingCriterion::SafeFallback { from, to } => Self::SafeFallback {
                from: from.into(),
                to: to.into(),
            },
        }
    }
}

impl From<StateError> for CommandError {
    fn from(_: StateError) -> Self {
        Self {
            code: "startup-unavailable",
            message: "Prompting Time application services are unavailable.".to_owned(),
            action: Some("Resolve the startup diagnostic and restart Prompting Time.".to_owned()),
        }
    }
}

impl From<AppError> for CommandError {
    fn from(error: AppError) -> Self {
        match error {
            AppError::EmptySubmission => invalid_request("The message and command identifier are required."),
            AppError::MessageTooLarge { .. } => invalid_request("The message is too large."),
            AppError::InvalidApprovalQuestion => {
                invalid_request("The approval response contains an unknown question.")
            }
            AppError::StaleApproval { .. } => Self {
                code: "stale-approval",
                message: "This approval request is no longer pending.".to_owned(),
                action: Some("Refresh the conversation before responding again.".to_owned()),
            },
            AppError::Store(error) => store_error(error),
            AppError::Routing(error) => routing_error(error),
            AppError::Runtime(error) => runtime_error(error),
            AppError::Workspace(error) => workspace_error(error),
            AppError::Handoff(error) => handoff_error(error),
            AppError::ConversationPersistence { .. } => Self {
                code: "storage-error",
                message: "The conversation could not be saved; its prepared workspace was retained or safely rolled back.".to_owned(),
                action: Some("Inspect the workspace before retrying.".to_owned()),
            },
        }
    }
}

fn invalid_request(message: &str) -> CommandError {
    CommandError {
        code: "invalid-request",
        message: message.to_owned(),
        action: None,
    }
}

fn store_error(error: StoreError) -> CommandError {
    match error {
        StoreError::InvalidPageLimit(_) | StoreError::InvalidCursor => {
            invalid_request("The requested page is invalid.")
        }
        StoreError::NotFound { entity, .. } => CommandError {
            code: "not-found",
            message: format!("The requested {entity} was not found."),
            action: Some("Refresh the conversation and retry.".to_owned()),
        },
        StoreError::ConversationArchived(_) => CommandError {
            code: "conversation-archived",
            message: "The conversation is archived.".to_owned(),
            action: None,
        },
        StoreError::ConversationBusy(_) => CommandError {
            code: "conversation-busy",
            message: "The conversation already has an active turn.".to_owned(),
            action: Some("Wait for or interrupt the active turn before submitting.".to_owned()),
        },
        StoreError::CommandConflict { .. } => CommandError {
            code: "command-conflict",
            message: "That command identifier belongs to a different request.".to_owned(),
            action: Some("Retry with a new command identifier.".to_owned()),
        },
        _ => CommandError {
            code: "storage-error",
            message: "Prompting Time could not update its local conversation data.".to_owned(),
            action: Some("Retry once; if the problem persists, restart Prompting Time.".to_owned()),
        },
    }
}

fn routing_error(_: RoutingError) -> CommandError {
    CommandError {
        code: "provider-unavailable",
        message: "No eligible provider is currently available for this request.".to_owned(),
        action: Some("Check provider diagnostics or choose another provider.".to_owned()),
    }
}

fn runtime_error(error: RuntimeError) -> CommandError {
    match error {
        RuntimeError::Store(error) => store_error(error),
        RuntimeError::RunQueueFull { .. } | RuntimeError::CommandQueueFull { .. } => CommandError {
            code: "queue-full",
            message: "The local run queue is full.".to_owned(),
            action: Some("Wait for or interrupt an active run, then retry.".to_owned()),
        },
        RuntimeError::ApprovalResponseTooLarge { .. } => {
            invalid_request("The approval response is too large.")
        }
        RuntimeError::UnknownRun(_) | RuntimeError::UnknownApproval { .. } => CommandError {
            code: "not-found",
            message: "The requested active operation was not found.".to_owned(),
            action: Some("Refresh the conversation and retry.".to_owned()),
        },
        RuntimeError::MissingAdapter(_) => routing_error(RoutingError::NoEligibleProviders {
            evaluations: Vec::new(),
        }),
        _ => CommandError {
            code: "runtime-error",
            message: "The provider operation could not be completed.".to_owned(),
            action: Some("Inspect provider diagnostics and the conversation timeline.".to_owned()),
        },
    }
}

fn workspace_error(_: WorkspaceError) -> CommandError {
    CommandError {
        code: "workspace-error",
        message: "The workspace operation could not be completed safely.".to_owned(),
        action: Some(
            "Inspect the selected directory and its Git state before retrying.".to_owned(),
        ),
    }
}

fn handoff_error(_: HandoffError) -> CommandError {
    CommandError {
        code: "handoff-error",
        message: "A bounded provider handoff could not be prepared.".to_owned(),
        action: Some("Shorten the request or conversation objective, then retry.".to_owned()),
    }
}
