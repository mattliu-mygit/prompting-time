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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub workspace_id: Option<String>,
    pub archived: bool,
    pub project_root: Option<String>,
    pub current_run_id: Option<String>,
    pub provider: Option<ProviderId>,
    pub run_status: Option<RunStatus>,
    pub rollup_status: Option<RollupStatus>,
    pub agents: Vec<AgentSnapshot>,
    pub agents_truncated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum AgentStatus {
    Queued,
    Running,
    Waiting,
    Completed,
    Interrupted,
    Failed,
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
pub struct AgentSnapshot {
    pub id: String,
    pub parent_id: Option<String>,
    pub provider: ProviderId,
    pub label: String,
    pub summary: Option<String>,
    pub status: AgentStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct LoadAgentTreeRequest {
    pub conversation_id: String,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentTreeItem {
    pub agent: AgentSnapshot,
    pub depth: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentTreePage {
    pub run_id: Option<String>,
    pub items: Vec<AgentTreeItem>,
    pub next_cursor: Option<String>,
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
pub enum MessageRole {
    User,
    Assistant,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TimelineItem {
    pub id: String,
    pub conversation_id: String,
    pub run_id: String,
    pub agent_id: String,
    pub sequence: String,
    pub kind: TimelineItemKind,
    pub role: Option<MessageRole>,
    pub content: String,
    pub content_bytes: String,
    pub truncated: bool,
    pub provider: ProviderId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TimelinePage {
    pub items: Vec<TimelineItem>,
    pub next_cursor: Option<String>,
    pub approvals: Vec<ApprovalSnapshot>,
    pub approvals_truncated: bool,
    pub approvals_next_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct LoadEventDetailRequest {
    pub event_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EventDetailSnapshot {
    pub id: String,
    pub content: String,
    pub content_bytes: String,
    pub truncated: bool,
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
pub struct SubmitMessageRequest {
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
    pub run_id: String,
    pub status: RunStatus,
    pub provider: ProviderId,
    pub duplicate: bool,
    pub routing_explanation: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "camelCase")]
pub struct SteerRunRequest {
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
    pub cleanup: CleanupSnapshot,
    pub current_run: Option<CurrentRunSnapshot>,
    pub routing: Option<RoutingSnapshot>,
    pub handoff: Option<String>,
    pub active_descendant_count: u32,
    pub agents_truncated: bool,
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
        conversation_id: String,
        run_id: String,
    },
    ReloadRequired {
        sequence: String,
    },
}
