use super::*;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: &'static str,
    pub message: String,
    pub action: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Codex,
    Claude,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInstallation {
    pub id: ProviderId,
    pub installed: bool,
    pub available: bool,
    pub version: Option<String>,
    pub diagnostic: Option<String>,
    pub capabilities: Vec<ProviderCapability>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapSnapshot {
    pub providers: Vec<ProviderInstallation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_diagnostic: Option<CommandError>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct ListConversationsRequest {
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct ListChildConversationsRequest {
    pub parent_id: String,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct LoadConversationRequest {
    pub conversation_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ThinkingPreference {
    Auto,
    ProviderDefault,
    Manual { level: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingDecision {
    pub preference: ThinkingPreference,
    pub requested_effort: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingConfiguration {
    pub model: String,
    pub supported_efforts: Vec<String>,
    pub configured_effort: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct SetThinkingPreferenceRequest {
    pub conversation_id: String,
    pub preference: ThinkingPreference,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ContextBudget {
    ProviderDefault,
    Tokens { tokens: u32 },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct SetContextBudgetRequest {
    pub conversation_id: String,
    pub preference: ContextBudget,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub context_budget: ContextBudget,
    pub run_context_budget: Option<ContextBudget>,
    pub thinking_preference: ThinkingPreference,
    pub thinking_decision: Option<ThinkingDecision>,
    pub thinking_configuration: Option<ThinkingConfiguration>,
    pub id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub has_children: bool,
    pub summary: Option<String>,
    pub capabilities: ConversationCapabilities,
    pub routing_profile: RoutingProfile,
    pub workspace_id: Option<String>,
    pub archived: bool,
    pub project_root: Option<String>,
    pub current_run_id: Option<String>,
    pub provider: Option<ProviderId>,
    pub run_status: Option<RunStatus>,
    pub rollup_status: Option<RollupStatus>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ConversationCapabilities {
    pub can_send: bool,
    pub can_interrupt: bool,
    pub can_archive: bool,
    pub can_route: bool,
    pub unavailable_reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ConversationPath {
    pub items: Vec<ConversationSummary>,
    pub truncated: bool,
    pub owner_conversation_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RollupStatus {
    NeedsAttention,
    Active,
    Failed,
    Interrupted,
    Completed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ConversationPage {
    pub items: Vec<ConversationSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct LoadTimelineRequest {
    pub conversation_id: String,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum TimelineItemKind {
    Message,
    Tool,
    Progress,
    Diagnostic,
    Lifecycle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum TimelinePresentation {
    Normal,
    Notice,
    Failure,
    Telemetry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum MessageRole {
    User,
    Assistant,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TimelineItem {
    pub operation: Option<ToolOperationSummary>,
    pub id: String,
    pub conversation_id: String,
    pub run_id: String,
    pub agent_id: String,
    pub sequence: String,
    pub kind: TimelineItemKind,
    pub presentation: TimelinePresentation,
    pub role: Option<MessageRole>,
    pub content: String,
    pub content_bytes: String,
    pub truncated: bool,
    pub provider: ProviderId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CompactionActivity {
    pub run_id: String,
    pub agent_id: String,
    pub provider: ProviderId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TimelinePage {
    pub active_compaction: Option<CompactionActivity>,
    pub items: Vec<TimelineItem>,
    pub next_cursor: Option<String>,
    pub approvals: Vec<ApprovalSnapshot>,
    pub approvals_truncated: bool,
    pub approvals_next_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsPage {
    pub items: Vec<TimelineItem>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct LoadEventDetailRequest {
    pub event_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EventDetailSnapshot {
    pub operation: Option<ToolOperationDetailSnapshot>,
    pub id: String,
    pub content: String,
    pub content_bytes: String,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ToolOperationStatus {
    Running,
    Succeeded,
    Failed,
    Interrupted,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ToolOperationSummary {
    pub status: ToolOperationStatus,
    pub has_details: bool,
    pub detail_revision: String,
    pub duration_ms: Option<String>,
    pub exit_code: Option<i32>,
    pub truncated: bool,
    pub conflicted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ToolOperationDetailSnapshot {
    pub title: String,
    pub status: ToolOperationStatus,
    pub input: Option<String>,
    pub output: Option<String>,
    pub error: Option<String>,
    pub context: Option<String>,
    pub duration_ms: Option<String>,
    pub exit_code: Option<i32>,
    pub truncated: bool,
    pub conflicted: bool,
    pub detail_revision: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RoutingProfile {
    Balanced,
    BestFit,
    UsageBalance,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum ConversationWorkspaceRequest {
    Projectless,
    Isolated { path: String },
    Direct { path: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct CreateConversationRequest {
    pub title: String,
    pub objective: String,
    pub constraints: Vec<String>,
    pub workspace: ConversationWorkspaceRequest,
    pub routing_profile: RoutingProfile,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct InspectProjectRequest {
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPathSnapshot {
    pub is_git: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct SubmitMessageRequest {
    #[specta(optional)]
    pub context_budget: Option<ContextBudget>,
    #[specta(optional)]
    pub thinking: Option<ThinkingPreference>,
    pub conversation_id: String,
    pub text: String,
    pub provider_override: Option<ProviderId>,
    pub command_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RunStatus {
    Queued,
    Running,
    Waiting,
    Completed,
    Interrupted,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionSnapshot {
    pub thinking_decision: ThinkingDecision,
    pub run_id: String,
    pub status: RunStatus,
    pub provider: ProviderId,
    pub duplicate: bool,
    pub routing_explanation: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct SteerRunRequest {
    pub conversation_id: String,
    pub run_id: String,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum ApprovalResponse {
    Approved,
    Denied,
    Answer(String),
    Answers(BTreeMap<String, Vec<String>>),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct RespondToApprovalRequest {
    pub approval_id: String,
    pub response: ApprovalResponse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Answered,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UserInputOption {
    pub label: String,
    pub description: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UserInputQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    pub options: Option<Vec<UserInputOption>>,
    pub is_other: bool,
    pub is_secret: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UserInputRequest {
    pub questions: Vec<UserInputQuestion>,
    pub auto_resolution_ms: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum FileSystemAccess {
    Read,
    Write,
    Deny,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum FileSystemPath {
    Path { path: String },
    GlobPattern { pattern: String },
    Special { value: SpecialPath },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SpecialPath {
    Root,
    Minimal,
    ProjectRoots {
        subpath: Option<String>,
    },
    Tmpdir,
    SlashTmp,
    Unknown {
        path: String,
        subpath: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PermissionEntry {
    pub access: FileSystemAccess,
    pub path: FileSystemPath,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PermissionProfile {
    pub entries: Option<Vec<PermissionEntry>>,
    pub glob_scan_max_depth: Option<String>,
    pub read: Option<Vec<String>>,
    pub write: Option<Vec<String>>,
    pub network_enabled: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum ApprovalDetails {
    CommandExecution {
        command: Option<String>,
        cwd: Option<String>,
    },
    FileChange {
        changes: Vec<FileChange>,
        grant_root: Option<String>,
        reason: Option<String>,
    },
    PermissionProfile {
        cwd: String,
        profile: PermissionProfile,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    pub change: FileChangeKind,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum FileChangeKind {
    Add,
    Delete,
    Update { move_path: Option<String> },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalSnapshot {
    pub id: String,
    pub run_id: String,
    pub agent_id: String,
    pub provider: ProviderId,
    pub operation: String,
    pub scope: String,
    pub status: ApprovalStatus,
    pub response_pending: bool,
    pub agent_path: Vec<String>,
    pub agent_path_truncated: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalListKind {
    Pending,
    History,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct LoadApprovalsRequest {
    pub conversation_id: String,
    pub cursor: Option<String>,
    pub limit: u32,
    pub kind: ApprovalListKind,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalPage {
    pub items: Vec<ApprovalSnapshot>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct LoadApprovalDetailRequest {
    pub approval_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct LoadApprovalQuestionsRequest {
    pub approval_id: String,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDetailSnapshot {
    pub id: String,
    pub status: ApprovalStatus,
    pub response_pending: bool,
    pub agent_path: Vec<String>,
    pub agent_path_truncated: bool,
    pub operation: String,
    pub scope: String,
    pub input: Option<UserInputRequest>,
    pub details: Option<ApprovalDetails>,
    pub question_count: u32,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalQuestionPreview {
    pub id: String,
    pub header: String,
    pub question: String,
    pub options: Option<Vec<UserInputOption>>,
    pub is_other: bool,
    pub is_secret: bool,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalQuestionPage {
    pub items: Vec<ApprovalQuestionPreview>,
    pub total_count: u32,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct InterruptRunRequest {
    pub conversation_id: String,
    pub run_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveConversationRequest {
    pub conversation_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct InspectWorkspaceRequest {
    pub conversation_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceMode {
    Projectless,
    Direct,
    Isolated,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceChangeKind {
    Present,
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    Untracked,
    Conflicted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChange {
    pub kind: WorkspaceChangeKind,
    pub relative_path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSnapshot {
    pub mode: WorkspaceMode,
    pub changes: Vec<WorkspaceChange>,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum CleanupBlocker {
    NotOwned,
    MissingWorktree,
    ModifiedTrackedFiles,
    UntrackedFiles,
    UniqueCommits,
    ActiveProcess,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CleanupSnapshot {
    pub eligible: bool,
    pub blocker: Option<CleanupBlocker>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RoutingSnapshot {
    pub provider: ProviderId,
    pub profile: RoutingProfile,
    pub reason: RoutingReason,
    pub task_kind: TaskKind,
    pub override_provider: Option<ProviderId>,
    pub eligible_providers: Vec<ProviderId>,
    pub required_capabilities: Vec<ProviderCapability>,
    pub evaluations: Vec<ProviderEvaluation>,
    pub rationale: Vec<RoutingCriterion>,
    pub explanation: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProviderEvaluation {
    pub provider: ProviderId,
    pub eligible: bool,
    pub blockers: Vec<RoutingBlocker>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum RoutingBlocker {
    Unavailable(ProviderUnavailability),
    MissingCapability(ProviderCapability),
    NotReported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ProviderUnavailability {
    NotInstalled,
    UnsupportedVersion,
    Unauthenticated,
    Unhealthy,
    QuotaBlocked,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRank {
    pub provider: ProviderId,
    pub recent_root_runs: String,
    pub stable_order: u8,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum RoutingCriterion {
    ManualOverride {
        provider: ProviderId,
    },
    EligibleProviders {
        providers: Vec<ProviderId>,
    },
    RequiredCapabilities {
        capabilities: Vec<ProviderCapability>,
    },
    Continuity {
        provider: ProviderId,
    },
    RankedCandidates {
        candidates: Vec<ProviderRank>,
    },
    SafeFallback {
        from: ProviderId,
        to: ProviderId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RoutingReason {
    ManualOverride,
    RequiredCapabilities,
    Continuity,
    OnlyEligibleProvider,
    LeastUsed,
    DeterministicTieBreak,
    SafeFallback,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum TaskKind {
    Implementation,
    Review,
    Research,
    General,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ProviderCapability {
    Streaming,
    Steering,
    DeferredApproval,
    Interruption,
    Resume,
    ChildAgents,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CurrentRunSnapshot {
    pub id: String,
    pub provider: ProviderId,
    pub status: RunStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct InspectorSnapshot {
    pub workspace: WorkspaceSnapshot,
    pub execution_path: String,
    pub owned_worktree: bool,
    pub cleanup: CleanupSnapshot,
    pub current_run: Option<CurrentRunSnapshot>,
    pub routing: Option<RoutingSnapshot>,
    pub handoff: Option<String>,
    pub active_descendant_count: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct ListRunAuditsRequest {
    pub conversation_id: String,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct LoadRunAuditRequest {
    pub conversation_id: String,
    pub run_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RunAuditSummarySnapshot {
    pub id: String,
    pub provider: ProviderId,
    pub status: RunStatus,
    pub reason: Option<RoutingReason>,
    pub routing_truncated: bool,
    pub has_handoff: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RunAuditPage {
    pub items: Vec<RunAuditSummarySnapshot>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RunAuditDetailSnapshot {
    pub thinking_decision: ThinkingDecision,
    pub thinking_configuration: Option<ThinkingConfiguration>,
    pub id: String,
    pub provider: ProviderId,
    pub status: RunStatus,
    pub routing: Option<RoutingSnapshot>,
    pub reason: Option<RoutingReason>,
    pub routing_truncated: bool,
    pub handoff: Option<String>,
    pub handoff_truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum AppEvent {
    ConversationChanged {
        sequence: String,
        conversation_id: String,
    },
    RunChanged {
        sequence: String,
        // Original execution owner, which can differ from the displayed child conversation.
        conversation_id: String,
        run_id: String,
    },
    ReloadRequired {
        sequence: String,
    },
    PluginChanged {
        sequence: String,
        plugin: String,
        /// Paths relative to the plugin's state folder of the files that changed there, such as
        /// "runs/<run>/drafts.json". Empty when Prompting Time changed the plugin itself: installed
        /// it, checked its prerequisites, or its start command started or ended. Null when more
        /// files changed than one event lists.
        files: Option<Vec<String>>,
    },
}

/// Where a plugin comes from. A release or archive must match its SHA-256 (64 hexadecimal
/// digits); a folder runs in place, for developing a plugin. Paths are absolute.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PluginSource {
    Release { url: String, sha256: String },
    Archive { path: String, sha256: String },
    Folder { path: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct InstallPluginRequest {
    pub source: PluginSource,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PluginSnapshot {
    pub name: String,
    pub title: String,
    pub version: String,
    pub description: String,
    pub source: PluginSource,
    pub folder: String,
    pub state_folder: String,
    /// The settings' JSON Schema, as JSON text.
    pub settings_schema: String,
    /// Where the window loads the plugin's page, in a frame sandboxed to scripts only, such as
    /// "pt-plugin://whooper/index.html".
    pub page_url: String,
    pub actions: Vec<PluginAction>,
    pub prerequisites: Vec<PluginPrerequisite>,
    /// Why the settings that decide which programs are needed could not be read; the plugin
    /// does not start until they can be.
    pub settings_error: Option<String>,
    pub process: PluginProcess,
    /// The manifest's `status` key: null unless the status action prints status format 2.
    pub status: Option<PluginStatusConfig>,
}

/// The vocabulary of a plugin's status format 2, which its status action fills in.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PluginStatusConfig {
    pub format: u32,
    /// The groups items go in; the window orders, folds, and puts them under Needs you by these.
    pub groups: Vec<PluginStatusGroup>,
    /// The actions an item's row may offer, run with `--item <id>` after their own arguments.
    pub item_actions: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PluginStatusGroup {
    pub id: String,
    pub title: String,
    /// Whether the group's items wait on the user, which lists them under Needs you.
    pub needs_you: bool,
    /// Where the group lists among the plugin's groups, lowest first.
    pub order: u32,
    /// Whether the group's section starts folded.
    pub folded: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PluginAction {
    pub name: String,
    pub title: String,
    pub description: String,
    /// The user confirms what the action will do before it runs for real.
    pub confirm: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PluginPrerequisite {
    pub command: String,
    pub purpose: String,
    pub needed: bool,
    /// Where the program was found, or null when it is missing.
    pub path: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum PluginProcess {
    /// Prompting Time started it and stops it when Prompting Time quits.
    Started {
        pid: u32,
    },
    /// Another copy holds the run lock, such as one started from a terminal.
    RunningElsewhere {
        pid: u32,
    },
    NotRunning {
        exit: Option<PluginExit>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PluginExit {
    pub code: Option<i32>,
    pub output: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct PluginActionRequest {
    pub plugin: String,
    pub action: String,
    pub args: Vec<String>,
    /// For an action that needs confirmation: null runs it with --print, which changes nothing and
    /// returns the preview and fingerprint to confirm; the confirmed fingerprint then runs it for
    /// real.
    pub confirmed_fingerprint: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PluginActionResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub fingerprint: Option<String>,
    /// What a --print run will do, for the confirmation: the text of the file it saved in the
    /// plugin's state folder, whose SHA-256 is the fingerprint.
    pub preview: Option<String>,
}

/// Names the plugin to stop, start, or read the log of.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct PluginNameRequest {
    pub plugin: String,
}

/// The end of what a plugin's start command printed since Prompting Time last started it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PluginLog {
    /// The log file, which each start empties.
    pub path: String,
    /// The end of the file, from its first whole line; empty before the first start.
    pub text: String,
    /// Whether the file holds more than `text`.
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct SavePluginSettingsRequest {
    pub plugin: String,
    /// The settings to change, as JSON object text.
    pub values: String,
}

/// What the menu bar extra shows, as the window's sidebar shows it: each background agent's status
/// in the same words, with its row's buttons, and how many reviews need you.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMenuBarRequest {
    /// False removes the icon from the menu bar: the user turned it off, or no background agent is
    /// installed.
    pub visible: bool,
    pub agents: Vec<MenuBarAgent>,
    /// The reviews under Needs you.
    pub reviews_needing_you: u32,
}

/// One background agent in the menu bar extra.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct MenuBarAgent {
    /// The plugin's name, which the agent's commands carry back to the window.
    pub plugin: String,
    pub title: String,
    /// The status words of the agent's sidebar row, such as "Running · next poll 3:45 PM" or
    /// "Can't review · Docker isn't running".
    pub status: String,
    /// The ID of the cause that blocks every review, which the status words name; choosing them
    /// opens its screen.
    pub cause: Option<String>,
    /// Whether the agent fails, is blocked, or can't start, which a dot on the icon shows.
    pub alert: bool,
    /// The row's buttons in order: Pause, Resume, or Start while it is stopped, then Poll now.
    pub controls: Vec<MenuBarControl>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct MenuBarControl {
    pub action: MenuBarAction,
    /// The button's label, such as "Pause" or "Polling…".
    pub label: String,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum MenuBarAction {
    Pause,
    Resume,
    Start,
    Poll,
}

/// A choice in the menu bar extra that the window carries out as the sidebar would. Prompting Time
/// sends it as the `prompting-time://menu-bar` event, and brings the window forward first for one
/// that opens a screen.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Type)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum MenuBarCommand {
    /// Pause, Resume, Start, or Poll now.
    Agent {
        plugin: String,
        action: MenuBarAction,
    },
    /// The agent's screen.
    OpenAgent { plugin: String },
    /// The screen of the cause that blocks every review.
    OpenCause { plugin: String, cause: String },
    /// The first review under Needs you.
    OpenNeedsYou,
}
