use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::process::Output;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::timeout;

pub use crate::domain::{
    ApprovalRequestDetails, FileChangeApprovalDetail, FileChangeKind, RequestedFileSystemAccess,
    RequestedFileSystemEntry, RequestedFileSystemPath, RequestedFileSystemPermissions,
    RequestedNetworkPermissions, RequestedPermissionProfile, RequestedSpecialPath, UserInputOption,
    UserInputQuestion, UserInputRequest,
};
use crate::domain::{ConversationId, MutationState};

pub mod codex;
pub mod process;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Codex,
    Claude,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderCapability {
    Streaming,
    Steering,
    DeferredApproval,
    Interruption,
    Resume,
    ChildAgents,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ProviderCapabilities(Vec<ProviderCapability>);

impl ProviderCapabilities {
    pub fn supports(&self, capability: ProviderCapability) -> bool {
        self.0.contains(&capability)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = ProviderCapability> + '_ {
        self.0.iter().copied()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<const N: usize> From<[ProviderCapability; N]> for ProviderCapabilities {
    fn from(capabilities: [ProviderCapability; N]) -> Self {
        Self::from_iter(capabilities)
    }
}

impl FromIterator<ProviderCapability> for ProviderCapabilities {
    fn from_iter<T: IntoIterator<Item = ProviderCapability>>(capabilities: T) -> Self {
        let mut capabilities = capabilities.into_iter().collect::<Vec<_>>();
        capabilities.sort_unstable();
        capabilities.dedup();
        Self(capabilities)
    }
}

impl<'de> Deserialize<'de> for ProviderCapabilities {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Vec::<ProviderCapability>::deserialize(deserializer)?
            .into_iter()
            .collect())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum ProviderHealth {
    Healthy { version: String },
    Unavailable { category: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartSession {
    pub conversation_id: ConversationId,
    pub working_directory: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeSession {
    pub conversation_id: ConversationId,
    pub working_directory: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSession {
    pub provider: ProviderId,
    pub native_id: String,
    /// Provider-native identity shared by related sessions, when exposed by the provider.
    pub native_group_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnRequest {
    pub prompt: String,
}

impl TurnRequest {
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum ApprovalResponse {
    Approved,
    Denied,
    Answer(String),
    Answers(BTreeMap<String, Vec<String>>),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeSubAgentActivityKind {
    Started,
    Interacted,
    Interrupted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeChildStatus {
    pub native_thread_id: String,
    pub status: NativeAgentStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum NativeAgentStatus {
    PendingInit,
    Running,
    Interrupted,
    Completed,
    Errored,
    Shutdown,
    NotFound,
    Unrecognized(String),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ProviderEvent {
    TurnStarted {
        native_turn_id: String,
    },
    AssistantMessage {
        content: String,
    },
    AssistantMessageDelta {
        native_item_id: String,
        content: String,
    },
    Progress {
        content: String,
    },
    ToolActivity {
        description: String,
        mutation: MutationState,
    },
    NativeItemActivity {
        native_item_id: String,
        description: String,
        mutation: MutationState,
    },
    ChildAgentActivity {
        native_item_id: String,
        parent_native_thread_id: String,
        child_native_thread_ids: Vec<String>,
        child_statuses: Vec<NativeChildStatus>,
        operation: String,
        status: String,
    },
    SubAgentActivity {
        native_item_id: String,
        agent_thread_id: String,
        agent_path: String,
        activity: NativeSubAgentActivityKind,
    },
    ApprovalRequested {
        request_id: String,
        operation: String,
        scope: String,
        details: Option<ApprovalRequestDetails>,
    },
    UserInputRequested {
        request_id: String,
        questions: Vec<UserInputQuestion>,
        auto_resolution_ms: Option<u64>,
    },
    /// A forward-compatible notification retained without its potentially sensitive payload.
    Unrecognized {
        method: String,
    },
    TurnCompleted,
    Interrupted,
}

impl ProviderEvent {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::TurnCompleted | Self::Interrupted)
    }
}

#[async_trait]
pub trait ProviderTurnOwner: Send {
    async fn shutdown(self: Box<Self>) -> Result<(), ProviderError>;
}

/// An event stream coupled to the resource that owns its provider turn.
///
/// `shutdown` must stop and await every process/request owned by the turn. Dropping an owner
/// without calling `shutdown` must still initiate termination, so timeout and unwind paths cannot
/// orphan provider processes.
pub struct ProviderTurn {
    events: mpsc::Receiver<Result<ProviderEvent, ProviderError>>,
    owner: Option<Box<dyn ProviderTurnOwner>>,
}

impl ProviderTurn {
    pub fn new(
        events: mpsc::Receiver<Result<ProviderEvent, ProviderError>>,
        owner: impl ProviderTurnOwner + 'static,
    ) -> Self {
        Self {
            events,
            owner: Some(Box::new(owner)),
        }
    }

    pub async fn recv(&mut self) -> Option<Result<ProviderEvent, ProviderError>> {
        self.events.recv().await
    }

    pub async fn shutdown(&mut self) -> Result<(), ProviderError> {
        match self.owner.take() {
            Some(owner) => owner.shutdown().await,
            None => Ok(()),
        }
    }
}

impl fmt::Debug for ProviderTurn {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderTurn")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DispatchCertainty {
    NotDispatched,
    MayHaveDispatched,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderErrorCategory {
    NotInstalled,
    TimedOut,
    InspectionFailed,
    Rejected,
    Protocol,
    Transport,
    MalformedJson,
    OversizedFrame,
    ProcessExited,
    StreamClosed,
    ContractViolation,
}

/// A provider session boundary owned by the run supervisor.
///
/// Adapter futures must be cancellation-safe: dropping any operation future must release its
/// request resources, and dropping a turn-start future must stop any process/request it owns.
/// Long-lived child processes must also terminate when the adapter/session that owns them drops.
#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    fn id(&self) -> ProviderId;
    fn capabilities(&self) -> ProviderCapabilities;
    async fn health(&self) -> Result<ProviderHealth, ProviderError>;
    async fn start_session(&self, request: StartSession) -> Result<ProviderSession, ProviderError>;
    async fn resume_session(
        &self,
        native_id: &str,
        request: ResumeSession,
    ) -> Result<ProviderSession, ProviderError>;
    async fn start_turn(
        &self,
        session: &ProviderSession,
        request: TurnRequest,
    ) -> Result<ProviderTurn, ProviderError>;
    async fn steer(
        &self,
        session: &ProviderSession,
        active_turn: &str,
        text: &str,
    ) -> Result<(), ProviderError>;
    async fn respond(
        &self,
        session: &ProviderSession,
        request_id: &str,
        response: ApprovalResponse,
    ) -> Result<(), ProviderError>;
    async fn interrupt(
        &self,
        session: &ProviderSession,
        active_turn: &str,
    ) -> Result<(), ProviderError>;
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInstallation {
    pub id: ProviderId,
    pub installed: bool,
    pub version: Option<String>,
    pub diagnostic: Option<String>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ProviderError {
    #[error("{binary} is not installed")]
    NotInstalled { binary: String, diagnostic: String },
    #[error("{binary} did not return a version within five seconds")]
    TimedOut { binary: String, diagnostic: String },
    #[error("{binary} could not be inspected")]
    InspectionFailed { binary: String, diagnostic: String },
    #[error("provider protocol failed ({category})")]
    Protocol { category: String },
    #[error("provider transport failed ({category})")]
    Transport { category: String },
    #[error("provider emitted malformed JSON")]
    MalformedJson,
    #[error("provider frame exceeded the {limit} byte limit")]
    OversizedFrame { limit: usize },
    #[error("provider process exited before the stream completed")]
    ProcessExited,
    #[error("provider stream closed before completion")]
    StreamClosed,
    #[error("provider rejected the turn before dispatch ({category:?})")]
    NotDispatched { category: ProviderErrorCategory },
}

impl ProviderError {
    pub fn dispatch_certainty(&self) -> DispatchCertainty {
        match self {
            Self::NotDispatched { .. } => DispatchCertainty::NotDispatched,
            _ => DispatchCertainty::MayHaveDispatched,
        }
    }

    pub fn category(&self) -> ProviderErrorCategory {
        match self {
            Self::NotInstalled { .. } => ProviderErrorCategory::NotInstalled,
            Self::TimedOut { .. } => ProviderErrorCategory::TimedOut,
            Self::InspectionFailed { .. } => ProviderErrorCategory::InspectionFailed,
            Self::Protocol { .. } => ProviderErrorCategory::Protocol,
            Self::Transport { .. } => ProviderErrorCategory::Transport,
            Self::MalformedJson => ProviderErrorCategory::MalformedJson,
            Self::OversizedFrame { .. } => ProviderErrorCategory::OversizedFrame,
            Self::ProcessExited => ProviderErrorCategory::ProcessExited,
            Self::StreamClosed => ProviderErrorCategory::StreamClosed,
            Self::NotDispatched { category } => *category,
        }
    }

    pub fn into_installation(self, id: ProviderId) -> ProviderInstallation {
        let diagnostic = match self {
            Self::NotInstalled { diagnostic, .. }
            | Self::TimedOut { diagnostic, .. }
            | Self::InspectionFailed { diagnostic, .. } => diagnostic,
            other => other.to_string(),
        };

        ProviderInstallation {
            id,
            installed: false,
            version: None,
            diagnostic: Some(diagnostic),
        }
    }
}

pub async fn discover_provider(
    binary: &str,
    id: ProviderId,
) -> Result<ProviderInstallation, ProviderError> {
    let mut command = Command::new(binary);
    command.arg("--version").kill_on_drop(true);

    let output = timeout(Duration::from_secs(5), command.output())
        .await
        .map_err(|_| ProviderError::TimedOut {
            binary: binary.to_owned(),
            diagnostic: "The version command timed out after five seconds.".to_owned(),
        })?
        .map_err(|error| {
            let diagnostic = sanitize_diagnostic(&error.to_string());

            if error.kind() == std::io::ErrorKind::NotFound {
                ProviderError::NotInstalled {
                    binary: binary.to_owned(),
                    diagnostic,
                }
            } else {
                ProviderError::InspectionFailed {
                    binary: binary.to_owned(),
                    diagnostic,
                }
            }
        })?;

    if !output.status.success() {
        return Err(ProviderError::InspectionFailed {
            binary: binary.to_owned(),
            diagnostic: output_diagnostic(&output),
        });
    }

    let version = first_version_line(&output.stdout).or_else(|| first_version_line(&output.stderr));
    match version {
        Some(version) => Ok(ProviderInstallation {
            id,
            installed: true,
            version: Some(version),
            diagnostic: None,
        }),
        None => Err(ProviderError::InspectionFailed {
            binary: binary.to_owned(),
            diagnostic: output_diagnostic(&output),
        }),
    }
}

fn first_version_line(output: &[u8]) -> Option<String> {
    String::from_utf8_lossy(output)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.to_ascii_lowercase().starts_with("warning"))
        .map(version_from_line)
}

fn version_from_line(line: &str) -> String {
    line.split_whitespace()
        .find(|word| word.chars().any(|character| character.is_ascii_digit()))
        .unwrap_or(line)
        .trim_matches(|character: char| !character.is_ascii_alphanumeric() && character != '.')
        .to_owned()
}

fn output_diagnostic(output: &Output) -> String {
    let stderr = sanitize_diagnostic(&String::from_utf8_lossy(&output.stderr));
    if stderr.is_empty() {
        sanitize_diagnostic(&String::from_utf8_lossy(&output.stdout))
    } else {
        stderr
    }
}

fn sanitize_diagnostic(diagnostic: &str) -> String {
    diagnostic
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[tokio::test]
    async fn missing_binary_is_reported_without_panicking() {
        let result = discover_provider("definitely-not-installed", ProviderId::Codex).await;
        assert!(matches!(result, Err(ProviderError::NotInstalled { .. })));
    }

    #[tokio::test]
    async fn timed_out_provider_is_terminated() {
        let directory = std::env::temp_dir().join(format!(
            "prompting-time-provider-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after the Unix epoch")
                .as_nanos()
        ));
        let marker = directory.join("still-running");
        let binary = directory.join("hanging-provider");
        fs::create_dir_all(&directory).expect("test directory should be created");
        fs::write(
            &binary,
            format!("#!/bin/sh\nsleep 6\ntouch '{}'\n", marker.display()),
        )
        .expect("test provider should be written");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))
            .expect("test provider should be executable");

        let result = discover_provider(
            binary.to_str().expect("test path should be UTF-8"),
            ProviderId::Codex,
        )
        .await;

        assert!(matches!(result, Err(ProviderError::TimedOut { .. })));
        tokio::time::sleep(Duration::from_secs(2)).await;
        let process_continued = marker.exists();
        fs::remove_dir_all(directory).expect("test directory should be removed");
        assert!(!process_continued, "timed-out provider process continued");
    }
}
