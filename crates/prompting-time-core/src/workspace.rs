use std::{
    collections::HashMap,
    io::Read,
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex, OnceLock, Weak},
};

use rustix::fs::{FileType, Mode, OFlags, fstat, openat};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::{RwLock, oneshot};

use crate::domain::{ConversationId, Workspace, WorkspaceId};

mod owned_fs;

use owned_fs::{
    IndexedWorktreeEntry, IndexedWorktreeState, ValidatedOwnedDirectory, WorktreeMetadataTarget,
    create_scratch_workspace_nofollow, create_worktree_parent_nofollow,
    open_directory_path_nofollow, open_owned_worktree_target, quarantine_owned_tree,
    remove_worktree_metadata_nofollow, rustix_io_error,
};

#[derive(Clone, Debug)]
pub enum WorkspaceRequest {
    Projectless {
        conversation_id: ConversationId,
    },
    Isolated {
        conversation_id: ConversationId,
        selected_path: PathBuf,
    },
    Direct {
        conversation_id: ConversationId,
        selected_path: PathBuf,
    },
}

impl WorkspaceRequest {
    pub fn projectless(conversation_id: ConversationId) -> Self {
        Self::Projectless { conversation_id }
    }

    pub fn isolated(selected_path: impl Into<PathBuf>) -> Self {
        Self::isolated_for(ConversationId::new(), selected_path)
    }

    pub fn isolated_for(
        conversation_id: ConversationId,
        selected_path: impl Into<PathBuf>,
    ) -> Self {
        Self::Isolated {
            conversation_id,
            selected_path: selected_path.into(),
        }
    }

    pub fn direct(selected_path: impl Into<PathBuf>) -> Self {
        Self::direct_for(ConversationId::new(), selected_path)
    }

    pub fn direct_for(conversation_id: ConversationId, selected_path: impl Into<PathBuf>) -> Self {
        Self::Direct {
            conversation_id,
            selected_path: selected_path.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct WorkspaceLease {
    pub conversation_id: ConversationId,
    pub project_root: Option<PathBuf>,
    pub path: PathBuf,
    pub owned_worktree: bool,
    ownership: Option<OwnedWorktree>,
}

impl WorkspaceLease {
    pub fn workspace(&self, id: WorkspaceId) -> Workspace {
        Workspace {
            id,
            conversation_id: self.conversation_id,
            project_root: self.project_root.clone(),
            execution_path: self.path.clone(),
            owned_worktree: self.owned_worktree,
            worktree_base_commit: self
                .ownership
                .as_ref()
                .map(|ownership| ownership.base_commit.clone()),
        }
    }
}

#[derive(Clone, Debug)]
struct OwnedWorktree {
    project_root: PathBuf,
    path: PathBuf,
    base_commit: String,
    base_revision: String,
    branch: String,
}

struct WorktreeRemovalTarget {
    worktree: ValidatedOwnedDirectory,
    metadata: WorktreeMetadataTarget,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceBlocker {
    NotOwned,
    MissingWorktree,
    ModifiedTrackedFiles,
    UntrackedFiles,
    UniqueCommits,
    ActiveProcess,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupEligibility {
    Eligible,
    Blocked(WorkspaceBlocker),
}

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("workspace operation `{operation}` failed")]
    Io {
        operation: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("git operation `{operation}` failed with status {status:?}")]
    GitCommandFailed {
        operation: &'static str,
        status: Option<i32>,
    },
    #[error("git operation `{operation}` returned invalid output")]
    InvalidGitOutput { operation: &'static str },
    #[error("owned worktree removal blocked: {blocker:?}")]
    RemovalBlocked { blocker: WorkspaceBlocker },
    #[error("owned worktree removal task failed")]
    RemovalTaskFailed,
    #[error("workspace preparation task failed")]
    PreparationTaskFailed,
    #[error("workspace ownership conflict during `{operation}`")]
    OwnershipConflict { operation: &'static str },
    #[error("owned worktree retained in quarantine during `{operation}`")]
    QuarantineRetained { operation: &'static str },
    #[error("quarantined owned worktree restoration failed during `{operation}`")]
    QuarantineRestorationFailed { operation: &'static str },
}

#[derive(Clone)]
pub struct WorkspaceManager {
    app_data_dir: PathBuf,
    owned_processes: OwnedProcessRegistry,
    #[cfg(test)]
    removal_coordination: Option<Arc<RemovalCoordination>>,
}

#[cfg(test)]
struct RemovalCoordination {
    after_validation: tokio::sync::Barrier,
    before_deletion: tokio::sync::Barrier,
}

#[cfg(test)]
fn install_removal_coordination(_path: &Path) -> Arc<RemovalCoordination> {
    Arc::new(RemovalCoordination {
        after_validation: tokio::sync::Barrier::new(2),
        before_deletion: tokio::sync::Barrier::new(2),
    })
}

#[cfg(test)]
struct PostQuarantineCoordination {
    quarantined: tokio::sync::Barrier,
    continue_checks: tokio::sync::Barrier,
}

#[cfg(test)]
fn post_quarantine_coordinations()
-> &'static Mutex<HashMap<PathBuf, Arc<PostQuarantineCoordination>>> {
    static COORDINATIONS: OnceLock<Mutex<HashMap<PathBuf, Arc<PostQuarantineCoordination>>>> =
        OnceLock::new();
    COORDINATIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
fn install_post_quarantine_coordination(path: &Path) -> Arc<PostQuarantineCoordination> {
    let coordination = Arc::new(PostQuarantineCoordination {
        quarantined: tokio::sync::Barrier::new(2),
        continue_checks: tokio::sync::Barrier::new(2),
    });
    post_quarantine_coordinations()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(path.to_owned(), Arc::clone(&coordination));
    coordination
}

#[cfg(test)]
async fn coordinate_post_quarantine(path: &Path) {
    let coordination = post_quarantine_coordinations()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(path);
    if let Some(coordination) = coordination {
        coordination.quarantined.wait().await;
        coordination.continue_checks.wait().await;
    }
}

#[cfg(test)]
struct PostRollbackRemovalCoordination {
    worktree_removed: tokio::sync::Barrier,
    continue_ref_cleanup: tokio::sync::Barrier,
}

#[cfg(test)]
fn post_rollback_removal_coordinations()
-> &'static Mutex<HashMap<PathBuf, Arc<PostRollbackRemovalCoordination>>> {
    static COORDINATIONS: OnceLock<Mutex<HashMap<PathBuf, Arc<PostRollbackRemovalCoordination>>>> =
        OnceLock::new();
    COORDINATIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
fn install_post_rollback_removal_coordination(path: &Path) -> Arc<PostRollbackRemovalCoordination> {
    let coordination = Arc::new(PostRollbackRemovalCoordination {
        worktree_removed: tokio::sync::Barrier::new(2),
        continue_ref_cleanup: tokio::sync::Barrier::new(2),
    });
    post_rollback_removal_coordinations()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(path.to_owned(), Arc::clone(&coordination));
    coordination
}

#[cfg(test)]
async fn coordinate_post_rollback_removal(path: &Path) {
    let coordination = post_rollback_removal_coordinations()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(path);
    if let Some(coordination) = coordination {
        coordination.worktree_removed.wait().await;
        coordination.continue_ref_cleanup.wait().await;
    }
}

#[derive(Clone, Default)]
pub struct OwnedProcessRegistry {
    state: Arc<ProcessState>,
}

#[derive(Default)]
struct ProcessState {
    active_paths: Arc<Mutex<HashMap<PathBuf, usize>>>,
    removal_gate: Arc<RwLock<()>>,
}

pub struct OwnedProcessRegistration {
    path: PathBuf,
    state: Arc<ProcessState>,
}

impl OwnedProcessRegistry {
    /// Records a process launched by this application for an execution directory.
    /// The registration remains active until the returned guard is dropped.
    pub async fn register(
        &self,
        execution_path: impl AsRef<Path>,
    ) -> Result<OwnedProcessRegistration, WorkspaceError> {
        let _launch_reservation = self.state.removal_gate.read().await;
        let path = canonicalize(execution_path.as_ref(), "resolve owned process directory")?;
        let mut active_paths = self
            .state
            .active_paths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *active_paths.entry(path.clone()).or_default() += 1;
        drop(active_paths);
        Ok(OwnedProcessRegistration {
            path,
            state: Arc::clone(&self.state),
        })
    }

    fn has_active_process(&self, execution_path: &Path) -> bool {
        self.state
            .active_paths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(execution_path)
            .is_some_and(|count| *count > 0)
    }
}

impl Drop for OwnedProcessRegistration {
    fn drop(&mut self) {
        let mut active_paths = self
            .state
            .active_paths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(count) = active_paths.get_mut(&self.path) {
            *count -= 1;
            if *count == 0 {
                active_paths.remove(&self.path);
            }
        }
    }
}

impl WorkspaceManager {
    pub fn new(app_data_dir: impl Into<PathBuf>) -> Self {
        let app_data_dir = app_data_dir.into();
        Self {
            owned_processes: process_registry(&app_data_dir),
            app_data_dir,
            #[cfg(test)]
            removal_coordination: None,
        }
    }

    pub fn owned_processes(&self) -> OwnedProcessRegistry {
        self.owned_processes.clone()
    }

    pub async fn lease(&self, workspace: &Workspace) -> Result<WorkspaceLease, WorkspaceError> {
        let _lifecycle_reservation = Arc::clone(&self.owned_processes.state.removal_gate)
            .read_owned()
            .await;
        let ownership = match (
            &workspace.project_root,
            workspace.owned_worktree,
            &workspace.worktree_base_commit,
        ) {
            (Some(project_root), true, Some(base_commit)) => Some(OwnedWorktree {
                project_root: project_root.clone(),
                path: workspace.execution_path.clone(),
                base_commit: base_commit.clone(),
                base_revision: base_reference(workspace.conversation_id),
                branch: worktree_branch(workspace.conversation_id),
            }),
            _ => None,
        };
        let lease = WorkspaceLease {
            conversation_id: workspace.conversation_id,
            project_root: workspace.project_root.clone(),
            path: workspace.execution_path.clone(),
            owned_worktree: workspace.owned_worktree,
            ownership,
        };
        if workspace.owned_worktree {
            if lease.ownership.is_none() {
                return Err(WorkspaceError::OwnershipConflict {
                    operation: "recover durable workspace",
                });
            }
            self.removal_target(&lease).await?;
        }
        Ok(lease)
    }

    pub async fn prepare(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceLease, WorkspaceError> {
        let lifecycle_reservation = Arc::clone(&self.owned_processes.state.removal_gate)
            .write_owned()
            .await;
        match request {
            WorkspaceRequest::Projectless { conversation_id } => {
                create_dir_all(&self.app_data_dir, "create application data directory")?;
                let path = create_scratch_workspace_nofollow(
                    &self.app_data_dir,
                    &conversation_id.to_string(),
                )?;

                Ok(WorkspaceLease {
                    conversation_id,
                    project_root: None,
                    path,
                    owned_worktree: false,
                    ownership: None,
                })
            }
            WorkspaceRequest::Isolated {
                conversation_id,
                selected_path,
            } => {
                create_dir_all(&self.app_data_dir, "create application data directory")?;
                let app_data_dir =
                    canonicalize(&self.app_data_dir, "resolve application data directory")?;
                let selected_path = canonicalize(&selected_path, "resolve selected project")?;
                let project_root = git_path(
                    &selected_path,
                    &["rev-parse", "--show-toplevel"],
                    "resolve Git project root",
                )
                .await?;
                let project_root = canonicalize(&project_root, "resolve Git project root")?;
                let current_head = git_text(
                    &project_root,
                    &["rev-parse", "HEAD"],
                    "resolve worktree base",
                )
                .await?;
                let branch = worktree_branch(conversation_id);
                let base_revision = base_reference(conversation_id);
                let repository_id = repository_id(&project_root);
                let path = app_data_dir
                    .join("worktrees")
                    .join(&repository_id)
                    .join(conversation_id.to_string());
                create_worktree_parent_nofollow(&app_data_dir, &repository_id)?;

                let (base_commit, create_base_reference) = if let Some(recorded_base) =
                    git_reference_value(&project_root, &base_revision).await?
                {
                    let worktrees = git_text(
                        &project_root,
                        &["worktree", "list", "--porcelain"],
                        "inventory Git worktrees",
                    )
                    .await?;
                    match worktree_registration(&worktrees, &path, &branch) {
                        WorktreeRegistration::Owned => {
                            let path = owned_path_for_lease(
                                &app_data_dir,
                                &path,
                                "recover isolated worktree",
                            )?;
                            return Ok(owned_lease(
                                conversation_id,
                                project_root,
                                path,
                                recorded_base,
                                base_revision,
                                branch,
                            ));
                        }
                        WorktreeRegistration::Missing
                            if !worktree_path_is_listed(&worktrees, &path)
                                && matches!(
                                    std::fs::symlink_metadata(&path),
                                    Err(error) if error.kind() == std::io::ErrorKind::NotFound
                                )
                                && git_reference_value(
                                    &project_root,
                                    &format!("refs/heads/{branch}"),
                                )
                                .await?
                                .is_none() =>
                        {
                            (recorded_base, false)
                        }
                        _ => {
                            return Err(WorkspaceError::OwnershipConflict {
                                operation: "recover isolated worktree",
                            });
                        }
                    }
                } else {
                    (current_head, true)
                };

                let (result_sender, result_receiver) = oneshot::channel();
                let (acknowledgment_sender, acknowledgment_receiver) = oneshot::channel();
                let task_app_data_dir = app_data_dir.clone();
                tokio::spawn(async move {
                    let _lifecycle_reservation = lifecycle_reservation;
                    let result = create_owned_worktree(
                        task_app_data_dir.clone(),
                        project_root,
                        path,
                        base_commit,
                        base_revision,
                        branch,
                        create_base_reference,
                    )
                    .await;
                    match result {
                        Ok(ownership) => {
                            let _ = deliver_prepared_worktree(
                                task_app_data_dir,
                                ownership,
                                result_sender,
                                acknowledgment_receiver,
                            )
                            .await;
                        }
                        Err(error) => {
                            let _ = result_sender.send(Err(error));
                        }
                    }
                });
                let ownership = result_receiver
                    .await
                    .map_err(|_| WorkspaceError::PreparationTaskFailed)??;
                let _ = acknowledgment_sender.send(());
                Ok(owned_lease(
                    conversation_id,
                    ownership.project_root,
                    ownership.path,
                    ownership.base_commit,
                    ownership.base_revision,
                    ownership.branch,
                ))
            }
            WorkspaceRequest::Direct {
                conversation_id,
                selected_path,
            } => {
                let selected_path = canonicalize(&selected_path, "resolve selected project")?;
                let project_root = match detect_git_root(&selected_path).await? {
                    Some(project_root) => canonicalize(&project_root, "resolve Git project root")?,
                    None => selected_path.clone(),
                };

                Ok(WorkspaceLease {
                    conversation_id,
                    project_root: Some(project_root),
                    path: selected_path,
                    owned_worktree: false,
                    ownership: None,
                })
            }
        }
    }

    pub async fn cleanup_eligibility(
        &self,
        lease: &WorkspaceLease,
    ) -> Result<CleanupEligibility, WorkspaceError> {
        let Some(ownership) = lease.ownership.as_ref() else {
            return Ok(CleanupEligibility::Blocked(WorkspaceBlocker::NotOwned));
        };
        if !lease.owned_worktree
            || lease.path != ownership.path
            || lease.project_root.as_ref() != Some(&ownership.project_root)
        {
            return Ok(CleanupEligibility::Blocked(WorkspaceBlocker::NotOwned));
        }

        let resolved_path = match validate_owned_path(&self.app_data_dir, &ownership.path)? {
            OwnedPathValidation::Valid(path) => path,
            OwnedPathValidation::Blocked(blocker) => {
                return Ok(CleanupEligibility::Blocked(blocker));
            }
        };

        let worktrees = git_text(
            &ownership.project_root,
            &["worktree", "list", "--porcelain"],
            "inventory Git worktrees",
        )
        .await?;
        match worktree_registration(&worktrees, &resolved_path, &ownership.branch) {
            WorktreeRegistration::Owned => {}
            WorktreeRegistration::Missing => {
                return Ok(CleanupEligibility::Blocked(
                    WorkspaceBlocker::MissingWorktree,
                ));
            }
            WorktreeRegistration::DifferentBranch => {
                return Ok(CleanupEligibility::Blocked(WorkspaceBlocker::NotOwned));
            }
        }

        let status = git_text(
            &resolved_path,
            &[
                "status",
                "--porcelain=v2",
                "--untracked-files=all",
                "--ignored=matching",
            ],
            "inspect worktree status",
        )
        .await?;
        let index_flags = git_text(
            &resolved_path,
            &["ls-files", "-v", "-z"],
            "inspect worktree index flags",
        )
        .await?;
        mutable_cleanup_eligibility(
            ownership,
            &status,
            &index_flags,
            true,
            self.owned_processes.has_active_process(&resolved_path),
        )
        .await
    }

    pub async fn remove_owned(&self, lease: &WorkspaceLease) -> Result<(), WorkspaceError> {
        let removal_reservation = Arc::clone(&self.owned_processes.state.removal_gate)
            .write_owned()
            .await;
        let manager = self.clone();
        let lease = lease.clone();
        tokio::spawn(async move {
            let _removal_reservation = removal_reservation;
            let target = manager.removal_target(&lease).await?;
            #[cfg(test)]
            if let Some(coordination) = &manager.removal_coordination {
                coordination.after_validation.wait().await;
                coordination.before_deletion.wait().await;
            }
            let ownership = lease
                .ownership
                .as_ref()
                .ok_or(WorkspaceError::RemovalBlocked {
                    blocker: WorkspaceBlocker::NotOwned,
                })?;
            let quarantined = quarantine_owned_tree(target.worktree)?;
            #[cfg(test)]
            coordinate_post_quarantine(&ownership.path).await;
            let final_check = async {
                let index = git_metadata_text(
                    &target.metadata,
                    &["ls-files", "--stage", "-v", "-z"],
                    "inspect quarantined worktree index",
                )
                .await?;
                let index = parse_index_snapshot(&index)?;
                let worktree_state = quarantined
                    .inspect_indexed_contents(&index.entries, &target.metadata.worktree_git_file)?;
                let head = git_metadata_text(
                    &target.metadata,
                    &["symbolic-ref", "HEAD"],
                    "inspect quarantined worktree branch",
                )
                .await?;
                let index_matches_head = git_metadata_index_matches_head(&target.metadata).await?;
                descriptor_bound_cleanup_eligibility(
                    ownership,
                    &target.metadata,
                    worktree_state,
                    index.suppresses_worktree_check,
                    index_matches_head,
                    head == format!("refs/heads/{}", ownership.branch),
                    manager.owned_processes.has_active_process(&lease.path),
                )
                .await
            }
            .await;
            let metadata_binding = target
                .metadata
                .namespace_bindings_intact("revalidate worktree metadata before removal");
            if !matches!(metadata_binding, Ok(true)) {
                quarantined.restore()?;
                return Err(metadata_binding
                    .err()
                    .unwrap_or(WorkspaceError::OwnershipConflict {
                        operation: "revalidate worktree metadata before removal",
                    }));
            }
            match final_check {
                Ok(CleanupEligibility::Eligible) => {}
                Ok(CleanupEligibility::Blocked(blocker)) => {
                    quarantined.restore()?;
                    return Err(WorkspaceError::RemovalBlocked { blocker });
                }
                Err(error) => {
                    quarantined.restore()?;
                    return Err(error);
                }
            }
            quarantined.remove(&target.metadata.worktree_git_file)?;
            remove_worktree_metadata_nofollow(&target.metadata)
        })
        .await
        .map_err(|_| WorkspaceError::RemovalTaskFailed)?
    }

    async fn removal_target(
        &self,
        lease: &WorkspaceLease,
    ) -> Result<WorktreeRemovalTarget, WorkspaceError> {
        let ownership = lease
            .ownership
            .as_ref()
            .ok_or(WorkspaceError::RemovalBlocked {
                blocker: WorkspaceBlocker::NotOwned,
            })?;
        let resolved_path = match validate_owned_path(&self.app_data_dir, &ownership.path)? {
            OwnedPathValidation::Valid(path) => path,
            OwnedPathValidation::Blocked(blocker) => {
                return Err(WorkspaceError::RemovalBlocked { blocker });
            }
        };
        let worktrees = git_text(
            &ownership.project_root,
            &["worktree", "list", "--porcelain"],
            "revalidate Git worktree ownership",
        )
        .await?;
        if worktree_registration(&worktrees, &resolved_path, &ownership.branch)
            != WorktreeRegistration::Owned
            || git_reference_value(&ownership.project_root, &ownership.base_revision)
                .await?
                .as_deref()
                != Some(&ownership.base_commit)
        {
            return Err(WorkspaceError::RemovalBlocked {
                blocker: WorkspaceBlocker::NotOwned,
            });
        }
        let worktree = open_owned_worktree_target(&self.app_data_dir, &resolved_path)?;
        let metadata = worktree_metadata_target(
            &ownership.project_root,
            &resolved_path,
            worktree.descriptor(),
        )
        .await?;
        Ok(WorktreeRemovalTarget { worktree, metadata })
    }
}

fn owned_lease(
    conversation_id: ConversationId,
    project_root: PathBuf,
    path: PathBuf,
    base_commit: String,
    base_revision: String,
    branch: String,
) -> WorkspaceLease {
    WorkspaceLease {
        conversation_id,
        project_root: Some(project_root.clone()),
        path: path.clone(),
        owned_worktree: true,
        ownership: Some(OwnedWorktree {
            project_root,
            path,
            base_commit,
            base_revision,
            branch,
        }),
    }
}

async fn mutable_cleanup_eligibility(
    ownership: &OwnedWorktree,
    status: &str,
    index_flags: &str,
    branch_matches: bool,
    active_process: bool,
) -> Result<CleanupEligibility, WorkspaceError> {
    if !branch_matches {
        return Ok(CleanupEligibility::Blocked(WorkspaceBlocker::NotOwned));
    }
    if status
        .lines()
        .any(|line| !line.is_empty() && !line.starts_with("? ") && !line.starts_with("! "))
    {
        return Ok(CleanupEligibility::Blocked(
            WorkspaceBlocker::ModifiedTrackedFiles,
        ));
    }
    if index_flags_hide_worktree_changes(index_flags) {
        return Ok(CleanupEligibility::Blocked(
            WorkspaceBlocker::ModifiedTrackedFiles,
        ));
    }
    if status
        .lines()
        .any(|line| line.starts_with("? ") || line.starts_with("! "))
    {
        return Ok(CleanupEligibility::Blocked(
            WorkspaceBlocker::UntrackedFiles,
        ));
    }
    if git_reference_value(&ownership.project_root, &ownership.base_revision)
        .await?
        .as_deref()
        != Some(&ownership.base_commit)
    {
        return Ok(CleanupEligibility::Blocked(WorkspaceBlocker::NotOwned));
    }
    let revision_range = format!("{}..{}", ownership.base_commit, ownership.branch);
    let unique_commits = git_text(
        &ownership.project_root,
        &["log", &revision_range, "--oneline"],
        "inspect unique worktree commits",
    )
    .await?;
    if !unique_commits.is_empty() {
        return Ok(CleanupEligibility::Blocked(WorkspaceBlocker::UniqueCommits));
    }
    if active_process {
        return Ok(CleanupEligibility::Blocked(WorkspaceBlocker::ActiveProcess));
    }
    Ok(CleanupEligibility::Eligible)
}

async fn descriptor_bound_cleanup_eligibility(
    ownership: &OwnedWorktree,
    metadata: &WorktreeMetadataTarget,
    worktree_state: IndexedWorktreeState,
    suppresses_worktree_check: bool,
    index_matches_head: bool,
    branch_matches: bool,
    active_process: bool,
) -> Result<CleanupEligibility, WorkspaceError> {
    if !branch_matches {
        return Ok(CleanupEligibility::Blocked(WorkspaceBlocker::NotOwned));
    }
    if worktree_state == IndexedWorktreeState::ModifiedTracked
        || suppresses_worktree_check
        || !index_matches_head
    {
        return Ok(CleanupEligibility::Blocked(
            WorkspaceBlocker::ModifiedTrackedFiles,
        ));
    }
    if worktree_state == IndexedWorktreeState::Untracked {
        return Ok(CleanupEligibility::Blocked(
            WorkspaceBlocker::UntrackedFiles,
        ));
    }
    if git_common_reference_value(metadata, &ownership.base_revision)
        .await?
        .as_deref()
        != Some(&ownership.base_commit)
    {
        return Ok(CleanupEligibility::Blocked(WorkspaceBlocker::NotOwned));
    }
    let revision_range = format!("{}..{}", ownership.base_commit, ownership.branch);
    let unique_commits = git_common_text(
        metadata,
        &["log", &revision_range, "--oneline"],
        "inspect quarantined unique worktree commits",
    )
    .await?;
    if !unique_commits.is_empty() {
        return Ok(CleanupEligibility::Blocked(WorkspaceBlocker::UniqueCommits));
    }
    if active_process {
        return Ok(CleanupEligibility::Blocked(WorkspaceBlocker::ActiveProcess));
    }
    Ok(CleanupEligibility::Eligible)
}

struct IndexSnapshot {
    entries: Vec<IndexedWorktreeEntry>,
    suppresses_worktree_check: bool,
}

fn parse_index_snapshot(output: &str) -> Result<IndexSnapshot, WorkspaceError> {
    const OPERATION: &str = "parse quarantined worktree index";
    let mut entries = Vec::new();
    let mut suppresses_worktree_check = false;
    for record in output.split('\0').filter(|record| !record.is_empty()) {
        let (metadata, path) = record
            .split_once('\t')
            .ok_or(WorkspaceError::InvalidGitOutput {
                operation: OPERATION,
            })?;
        let mut fields = metadata.split_whitespace();
        let tag = fields
            .next()
            .and_then(|field| field.as_bytes().first().copied())
            .ok_or(WorkspaceError::InvalidGitOutput {
                operation: OPERATION,
            })?;
        let mode = u32::from_str_radix(
            fields.next().ok_or(WorkspaceError::InvalidGitOutput {
                operation: OPERATION,
            })?,
            8,
        )
        .map_err(|_| WorkspaceError::InvalidGitOutput {
            operation: OPERATION,
        })?;
        let object_id = fields
            .next()
            .ok_or(WorkspaceError::InvalidGitOutput {
                operation: OPERATION,
            })?
            .to_owned();
        let stage = fields.next().ok_or(WorkspaceError::InvalidGitOutput {
            operation: OPERATION,
        })?;
        if fields.next().is_some() || stage != "0" {
            return Err(WorkspaceError::InvalidGitOutput {
                operation: OPERATION,
            });
        }
        let path = PathBuf::from(path);
        if path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(WorkspaceError::InvalidGitOutput {
                operation: OPERATION,
            });
        }
        suppresses_worktree_check |= tag.is_ascii_lowercase() || tag == b'S';
        entries.push(IndexedWorktreeEntry {
            path,
            mode,
            object_id,
        });
    }
    Ok(IndexSnapshot {
        entries,
        suppresses_worktree_check,
    })
}

fn index_flags_hide_worktree_changes(index_flags: &str) -> bool {
    index_flags.split('\0').any(|entry| {
        entry
            .as_bytes()
            .first()
            .is_some_and(|tag| tag.is_ascii_lowercase() || *tag == b'S')
    })
}

async fn create_owned_worktree(
    app_data_dir: PathBuf,
    project_root: PathBuf,
    path: PathBuf,
    base_commit: String,
    base_revision: String,
    branch: String,
    create_base_reference: bool,
) -> Result<OwnedWorktree, WorkspaceError> {
    if create_base_reference {
        run_git(
            &project_root,
            &["update-ref", &base_revision, &base_commit, ""],
            "record worktree base",
        )
        .await?;
    }
    let ownership = OwnedWorktree {
        project_root,
        path,
        base_commit,
        base_revision,
        branch,
    };
    let path_argument = ownership.path.to_string_lossy().into_owned();
    if let Err(error) = run_git(
        &ownership.project_root,
        &[
            "worktree",
            "add",
            "-b",
            &ownership.branch,
            &path_argument,
            &ownership.base_commit,
        ],
        "create isolated worktree",
    )
    .await
    {
        rollback_cancelled_worktree(&app_data_dir, &ownership).await?;
        return Err(error);
    }
    let path = match owned_path_for_lease(
        &app_data_dir,
        &ownership.path,
        "validate created isolated worktree",
    ) {
        Ok(path) => path,
        Err(error) => {
            rollback_cancelled_worktree(&app_data_dir, &ownership).await?;
            return Err(error);
        }
    };
    Ok(OwnedWorktree { path, ..ownership })
}

async fn deliver_prepared_worktree(
    app_data_dir: PathBuf,
    ownership: OwnedWorktree,
    result_sender: oneshot::Sender<Result<OwnedWorktree, WorkspaceError>>,
    acknowledgment_receiver: oneshot::Receiver<()>,
) -> Result<(), WorkspaceError> {
    if result_sender.send(Ok(ownership.clone())).is_err() || acknowledgment_receiver.await.is_err()
    {
        rollback_cancelled_worktree(&app_data_dir, &ownership).await?;
    }
    Ok(())
}

async fn rollback_cancelled_worktree(
    app_data_dir: &Path,
    ownership: &OwnedWorktree,
) -> Result<(), WorkspaceError> {
    let worktrees = git_text(
        &ownership.project_root,
        &["worktree", "list", "--porcelain"],
        "inventory cancelled worktree",
    )
    .await?;
    let remove_path = match validate_owned_path(app_data_dir, &ownership.path) {
        Ok(OwnedPathValidation::Valid(path))
            if worktree_registration(&worktrees, &path, &ownership.branch)
                == WorktreeRegistration::Owned
                && git_reference_value(&ownership.project_root, &ownership.base_revision)
                    .await
                    .ok()
                    .flatten()
                    .as_deref()
                    == Some(&ownership.base_commit) =>
        {
            Some(Some(path))
        }
        Ok(OwnedPathValidation::Blocked(WorkspaceBlocker::MissingWorktree))
            if !worktree_path_is_listed(&worktrees, &ownership.path) =>
        {
            Some(None)
        }
        _ => None,
    };
    let Some(remove_path) = remove_path else {
        return Ok(());
    };
    let retained_metadata = if let Some(path) = remove_path {
        let worktree = open_owned_worktree_target(app_data_dir, &path)?;
        let metadata =
            worktree_metadata_target(&ownership.project_root, &path, worktree.descriptor()).await?;
        let quarantined = quarantine_owned_tree(worktree)?;
        #[cfg(test)]
        coordinate_post_quarantine(&ownership.path).await;
        let final_check = async {
            let index = git_metadata_text(
                &metadata,
                &["ls-files", "--stage", "-v", "-z"],
                "inspect quarantined cancelled worktree index",
            )
            .await?;
            let index = parse_index_snapshot(&index)?;
            let worktree_state = quarantined
                .inspect_indexed_contents(&index.entries, &metadata.worktree_git_file)?;
            let head = git_metadata_text(
                &metadata,
                &["symbolic-ref", "HEAD"],
                "inspect quarantined cancelled worktree branch",
            )
            .await?;
            let index_matches_head = git_metadata_index_matches_head(&metadata).await?;
            descriptor_bound_cleanup_eligibility(
                ownership,
                &metadata,
                worktree_state,
                index.suppresses_worktree_check,
                index_matches_head,
                head == format!("refs/heads/{}", ownership.branch),
                false,
            )
            .await
        }
        .await;
        let metadata_binding = metadata
            .namespace_bindings_intact("revalidate cancelled worktree metadata before removal");
        if !matches!(metadata_binding, Ok(true)) {
            quarantined.restore()?;
            return Err(metadata_binding
                .err()
                .unwrap_or(WorkspaceError::OwnershipConflict {
                    operation: "revalidate cancelled worktree metadata before removal",
                }));
        }
        match final_check {
            Ok(CleanupEligibility::Eligible) => {}
            Ok(CleanupEligibility::Blocked(_)) => {
                quarantined.restore()?;
                return Ok(());
            }
            Err(error) => {
                quarantined.restore()?;
                return Err(error);
            }
        }
        quarantined.remove(&metadata.worktree_git_file)?;
        remove_worktree_metadata_nofollow(&metadata)?;
        Some(metadata)
    } else {
        None
    };
    #[cfg(test)]
    coordinate_post_rollback_removal(&ownership.path).await;
    let branch_reference = format!("refs/heads/{}", ownership.branch);
    let branch_value = if let Some(metadata) = &retained_metadata {
        git_common_reference_value(metadata, &branch_reference).await
    } else {
        git_reference_value(&ownership.project_root, &branch_reference).await
    };
    let branch_exists = match branch_value {
        Ok(Some(branch_commit)) if branch_commit == ownership.base_commit => true,
        Ok(None) => false,
        _ => return Ok(()),
    };
    if let Some(metadata) = &retained_metadata {
        delete_cancelled_references_in_common(
            metadata,
            &branch_reference,
            &ownership.base_revision,
            &ownership.base_commit,
            branch_exists,
        )
        .await
    } else {
        delete_cancelled_references(
            &ownership.project_root,
            &branch_reference,
            &ownership.base_revision,
            &ownership.base_commit,
            branch_exists,
        )
        .await
    }
}

async fn delete_cancelled_references(
    project_root: &Path,
    branch_reference: &str,
    base_reference: &str,
    base_commit: &str,
    branch_exists: bool,
) -> Result<(), WorkspaceError> {
    let transaction = cancelled_reference_transaction(
        branch_reference,
        base_reference,
        base_commit,
        branch_exists,
    );
    run_git_with_stdin(
        project_root,
        &["update-ref", "--stdin"],
        transaction.as_bytes(),
        "discard cancelled worktree ownership",
    )
    .await
}

async fn delete_cancelled_references_in_common(
    metadata: &WorktreeMetadataTarget,
    branch_reference: &str,
    base_reference: &str,
    base_commit: &str,
    branch_exists: bool,
) -> Result<(), WorkspaceError> {
    let transaction = cancelled_reference_transaction(
        branch_reference,
        base_reference,
        base_commit,
        branch_exists,
    );
    run_git_descriptor_with_stdin(
        metadata.common_descriptor(),
        &["update-ref", "--stdin"],
        transaction.as_bytes(),
        "discard cancelled worktree ownership",
    )
    .await
}

fn cancelled_reference_transaction(
    branch_reference: &str,
    base_reference: &str,
    base_commit: &str,
    branch_exists: bool,
) -> String {
    let branch_update = if branch_exists {
        format!("delete {branch_reference} {base_commit}\n")
    } else {
        format!("verify {branch_reference}\n")
    };
    format!("start\n{branch_update}delete {base_reference} {base_commit}\nprepare\ncommit\n")
}

fn owned_path_for_lease(
    app_data_dir: &Path,
    expected_path: &Path,
    operation: &'static str,
) -> Result<PathBuf, WorkspaceError> {
    match validate_owned_path(app_data_dir, expected_path)? {
        OwnedPathValidation::Valid(path) => Ok(path),
        OwnedPathValidation::Blocked(_) => Err(WorkspaceError::OwnershipConflict { operation }),
    }
}

async fn worktree_metadata_target(
    project_root: &Path,
    worktree_path: &Path,
    worktree: &OwnedFd,
) -> Result<WorktreeMetadataTarget, WorkspaceError> {
    const OPERATION: &str = "validate worktree metadata ownership";
    let directory_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    let git_file = openat(
        worktree,
        ".git",
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|source| rustix_io_error(OPERATION, source))?;
    let git_file_stat = fstat(&git_file).map_err(|source| rustix_io_error(OPERATION, source))?;
    if FileType::from_raw_mode(git_file_stat.st_mode) != FileType::RegularFile {
        return Err(WorkspaceError::OwnershipConflict {
            operation: OPERATION,
        });
    }
    let mut git_file_text = String::new();
    std::fs::File::from(git_file.try_clone().map_err(|source| WorkspaceError::Io {
        operation: OPERATION,
        source,
    })?)
    .read_to_string(&mut git_file_text)
    .map_err(|source| WorkspaceError::Io {
        operation: OPERATION,
        source,
    })?;
    let worktree_git_dir = git_file_text
        .trim()
        .strip_prefix("gitdir: ")
        .map(PathBuf::from)
        .ok_or(WorkspaceError::OwnershipConflict {
            operation: OPERATION,
        })?;
    let common_git_dir = git_path(
        project_root,
        &["rev-parse", "--git-common-dir"],
        "resolve common Git metadata",
    )
    .await?;
    let worktree_git_dir = canonicalize(
        &resolve_git_path(worktree_path, worktree_git_dir),
        "resolve worktree metadata",
    )?;
    let common_git_dir = canonicalize(
        &resolve_git_path(project_root, common_git_dir),
        "resolve common Git metadata",
    )?;
    let metadata_root = canonicalize(
        &common_git_dir.join("worktrees"),
        "resolve linked worktree metadata root",
    )?;
    let relative = worktree_git_dir.strip_prefix(&metadata_root).map_err(|_| {
        WorkspaceError::OwnershipConflict {
            operation: OPERATION,
        }
    })?;
    let name = match (relative.components().next(), relative.components().nth(1)) {
        (Some(std::path::Component::Normal(name)), None) => name.to_owned(),
        _ => {
            return Err(WorkspaceError::OwnershipConflict {
                operation: OPERATION,
            });
        }
    };
    let common_git = open_directory_path_nofollow(&common_git_dir, OPERATION)?;
    let parent = openat(&common_git, "worktrees", directory_flags, Mode::empty())
        .map_err(|source| rustix_io_error(OPERATION, source))?;
    let directory = openat(&parent, &name, directory_flags, Mode::empty())
        .map_err(|source| rustix_io_error(OPERATION, source))?;
    let backpointer = openat(
        &directory,
        "gitdir",
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|source| rustix_io_error(OPERATION, source))?;
    let mut backpointer_text = String::new();
    std::fs::File::from(backpointer)
        .read_to_string(&mut backpointer_text)
        .map_err(|source| WorkspaceError::Io {
            operation: OPERATION,
            source,
        })?;
    let recorded_git_file = canonicalize(
        &resolve_git_path(&worktree_git_dir, PathBuf::from(backpointer_text.trim())),
        OPERATION,
    )?;
    let expected_git_file = worktree_path.join(".git");
    if recorded_git_file != expected_git_file {
        return Err(WorkspaceError::OwnershipConflict {
            operation: OPERATION,
        });
    }
    Ok(WorktreeMetadataTarget {
        parent,
        directory,
        name,
        common_directory: common_git,
        common_path: common_git_dir,
        worktree_git_file: git_file,
    })
}

fn resolve_git_path(directory: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        directory.join(path)
    }
}

fn create_dir_all(path: &Path, operation: &'static str) -> Result<(), WorkspaceError> {
    std::fs::create_dir_all(path).map_err(|source| WorkspaceError::Io { operation, source })
}

fn canonicalize(path: &Path, operation: &'static str) -> Result<PathBuf, WorkspaceError> {
    std::fs::canonicalize(path).map_err(|source| WorkspaceError::Io { operation, source })
}

fn repository_id(project_root: &Path) -> String {
    format!(
        "{:x}",
        Sha256::digest(project_root.as_os_str().as_encoded_bytes())
    )
}

enum OwnedPathValidation {
    Valid(PathBuf),
    Blocked(WorkspaceBlocker),
}

fn validate_owned_path(
    app_data_dir: &Path,
    expected_path: &Path,
) -> Result<OwnedPathValidation, WorkspaceError> {
    let app_data_dir = match std::fs::canonicalize(app_data_dir) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(OwnedPathValidation::Blocked(WorkspaceBlocker::NotOwned));
        }
        Err(source) => {
            return Err(WorkspaceError::Io {
                operation: "resolve application data directory",
                source,
            });
        }
    };
    let worktrees_root = app_data_dir.join("worktrees");
    let root_metadata = match std::fs::symlink_metadata(&worktrees_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(OwnedPathValidation::Blocked(WorkspaceBlocker::NotOwned));
        }
        Err(source) => {
            return Err(WorkspaceError::Io {
                operation: "inspect worktree root",
                source,
            });
        }
    };
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Ok(OwnedPathValidation::Blocked(WorkspaceBlocker::NotOwned));
    }
    let relative = match expected_path.strip_prefix(&worktrees_root) {
        Ok(relative) if !relative.as_os_str().is_empty() => relative,
        _ => return Ok(OwnedPathValidation::Blocked(WorkspaceBlocker::NotOwned)),
    };
    let mut current = worktrees_root.clone();
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Ok(OwnedPathValidation::Blocked(WorkspaceBlocker::NotOwned));
        }
        current.push(component);
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(OwnedPathValidation::Blocked(
                    WorkspaceBlocker::MissingWorktree,
                ));
            }
            Err(source) => {
                return Err(WorkspaceError::Io {
                    operation: "inspect owned worktree path",
                    source,
                });
            }
        };
        if metadata.file_type().is_symlink() {
            return Ok(OwnedPathValidation::Blocked(WorkspaceBlocker::NotOwned));
        }
    }
    let resolved = canonicalize(&current, "resolve owned worktree path")?;
    if resolved != current || !resolved.starts_with(&worktrees_root) || resolved == worktrees_root {
        return Ok(OwnedPathValidation::Blocked(WorkspaceBlocker::NotOwned));
    }
    Ok(OwnedPathValidation::Valid(resolved))
}

fn worktree_branch(conversation_id: ConversationId) -> String {
    format!("prompting-time/{conversation_id}")
}

fn base_reference(conversation_id: ConversationId) -> String {
    format!("refs/prompting-time/bases/{conversation_id}")
}

fn process_registry(app_data_dir: &Path) -> OwnedProcessRegistry {
    static REGISTRIES: OnceLock<Mutex<HashMap<PathBuf, Weak<ProcessState>>>> = OnceLock::new();

    let key = path_identity(app_data_dir);
    let mut registries = REGISTRIES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let state = registries
        .get(&key)
        .and_then(Weak::upgrade)
        .unwrap_or_else(|| {
            let state = Arc::new(ProcessState::default());
            registries.insert(key, Arc::downgrade(&state));
            state
        });
    OwnedProcessRegistry { state }
}

fn path_identity(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .unwrap_or_else(|_| path.to_owned())
    };
    let absolute = normalize_lexical_path(&absolute);
    let mut existing = absolute.as_path();
    let mut missing = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            return absolute;
        };
        missing.push(name.to_owned());
        let Some(parent) = existing.parent() else {
            return absolute;
        };
        existing = parent;
    }
    let mut identity = std::fs::canonicalize(existing).unwrap_or_else(|_| existing.to_owned());
    for component in missing.iter().rev() {
        identity.push(component);
    }
    identity
}

fn normalize_lexical_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(Path::new("/")),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::Normal(name) => normalized.push(name),
        }
    }
    normalized
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorktreeRegistration {
    Owned,
    Missing,
    DifferentBranch,
}

fn worktree_registration(
    inventory: &str,
    expected_path: &Path,
    expected_branch: &str,
) -> WorktreeRegistration {
    for entry in inventory.split("\n\n") {
        let mut path_matches = false;
        let mut branch_matches = false;
        let mut prunable = false;
        for line in entry.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                path_matches = Path::new(path) == expected_path;
            }
            if let Some(branch) = line.strip_prefix("branch refs/heads/") {
                branch_matches = branch == expected_branch;
            }
            prunable |= line.starts_with("prunable ");
        }
        if path_matches {
            return if prunable || !expected_path.exists() {
                WorktreeRegistration::Missing
            } else if branch_matches {
                WorktreeRegistration::Owned
            } else {
                WorktreeRegistration::DifferentBranch
            };
        }
    }
    WorktreeRegistration::Missing
}

fn worktree_path_is_listed(inventory: &str, expected_path: &Path) -> bool {
    inventory.lines().any(|line| {
        line.strip_prefix("worktree ")
            .is_some_and(|path| Path::new(path) == expected_path)
    })
}

async fn git_reference_value(
    project_root: &Path,
    reference: &str,
) -> Result<Option<String>, WorkspaceError> {
    let operation = "read worktree ownership marker";
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--verify", "--quiet", reference])
        .output()
        .await
        .map_err(|source| WorkspaceError::Io { operation, source })?;
    match output.status.code() {
        Some(0) => String::from_utf8(output.stdout)
            .map(|value| Some(value.trim().to_owned()))
            .map_err(|_| WorkspaceError::InvalidGitOutput { operation }),
        Some(1) => Ok(None),
        status => Err(WorkspaceError::GitCommandFailed { operation, status }),
    }
}

async fn git_path(
    directory: &Path,
    args: &[&str],
    operation: &'static str,
) -> Result<PathBuf, WorkspaceError> {
    git_text(directory, args, operation)
        .await
        .map(PathBuf::from)
}

async fn detect_git_root(directory: &Path) -> Result<Option<PathBuf>, WorkspaceError> {
    let operation = "detect Git project";
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .await
        .map_err(|source| WorkspaceError::Io { operation, source })?;
    if !output.status.success() {
        if directory
            .ancestors()
            .any(|ancestor| std::fs::symlink_metadata(ancestor.join(".git")).is_ok())
        {
            return Err(WorkspaceError::GitCommandFailed {
                operation,
                status: output.status.code(),
            });
        }
        return Ok(None);
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| WorkspaceError::InvalidGitOutput { operation })?;
    Ok(Some(PathBuf::from(stdout.trim())))
}

async fn git_text(
    directory: &Path,
    args: &[&str],
    operation: &'static str,
) -> Result<String, WorkspaceError> {
    let output = git_output(directory, args, operation).await?;
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| WorkspaceError::InvalidGitOutput { operation })?;
    Ok(stdout.trim().to_owned())
}

async fn git_metadata_text(
    metadata: &WorktreeMetadataTarget,
    args: &[&str],
    operation: &'static str,
) -> Result<String, WorkspaceError> {
    git_descriptor_text(metadata.descriptor(), args, operation).await
}

async fn git_metadata_index_matches_head(
    metadata: &WorktreeMetadataTarget,
) -> Result<bool, WorkspaceError> {
    let operation = "compare quarantined worktree index with HEAD";
    let output = git_descriptor_output(
        metadata.descriptor(),
        &["diff-index", "--cached", "--quiet", "HEAD", "--"],
        operation,
    )
    .await?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        status => Err(WorkspaceError::GitCommandFailed { operation, status }),
    }
}

async fn git_common_text(
    metadata: &WorktreeMetadataTarget,
    args: &[&str],
    operation: &'static str,
) -> Result<String, WorkspaceError> {
    git_descriptor_text(metadata.common_descriptor(), args, operation).await
}

async fn git_descriptor_text(
    directory: &OwnedFd,
    args: &[&str],
    operation: &'static str,
) -> Result<String, WorkspaceError> {
    let output = git_descriptor_output(directory, args, operation).await?;
    if !output.status.success() {
        return Err(WorkspaceError::GitCommandFailed {
            operation,
            status: output.status.code(),
        });
    }
    String::from_utf8(output.stdout)
        .map(|stdout| stdout.trim().to_owned())
        .map_err(|_| WorkspaceError::InvalidGitOutput { operation })
}

async fn git_common_reference_value(
    metadata: &WorktreeMetadataTarget,
    reference: &str,
) -> Result<Option<String>, WorkspaceError> {
    let operation = "read quarantined worktree ownership marker";
    let output = git_descriptor_output(
        metadata.common_descriptor(),
        &["rev-parse", "--verify", "--quiet", reference],
        operation,
    )
    .await?;
    match output.status.code() {
        Some(0) => String::from_utf8(output.stdout)
            .map(|stdout| Some(stdout.trim().to_owned()))
            .map_err(|_| WorkspaceError::InvalidGitOutput { operation }),
        Some(1) => Ok(None),
        status => Err(WorkspaceError::GitCommandFailed { operation, status }),
    }
}

async fn git_descriptor_output(
    directory: &OwnedFd,
    args: &[&str],
    operation: &'static str,
) -> Result<std::process::Output, WorkspaceError> {
    let directory = directory
        .try_clone()
        .map_err(|source| WorkspaceError::Io { operation, source })?;
    let mut command = Command::new("git");
    command.arg("--git-dir=.").args(args);
    // SAFETY: `fchdir` is an async-signal-safe syscall, and the closure only
    // accesses the owned directory descriptor captured before spawn.
    unsafe {
        command.pre_exec(move || rustix::process::fchdir(&directory).map_err(std::io::Error::from));
    }
    command
        .output()
        .await
        .map_err(|source| WorkspaceError::Io { operation, source })
}

async fn run_git(
    directory: &Path,
    args: &[&str],
    operation: &'static str,
) -> Result<(), WorkspaceError> {
    git_output(directory, args, operation).await.map(|_| ())
}

async fn run_git_with_stdin(
    directory: &Path,
    args: &[&str],
    input: &[u8],
    operation: &'static str,
) -> Result<(), WorkspaceError> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| WorkspaceError::Io { operation, source })?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or(WorkspaceError::InvalidGitOutput { operation })?;
    let write_result = stdin.write_all(input).await;
    drop(stdin);
    let status = child
        .wait()
        .await
        .map_err(|source| WorkspaceError::Io { operation, source })?;
    write_result.map_err(|source| WorkspaceError::Io { operation, source })?;
    if !status.success() {
        return Err(WorkspaceError::GitCommandFailed {
            operation,
            status: status.code(),
        });
    }
    Ok(())
}

async fn run_git_descriptor_with_stdin(
    directory: &OwnedFd,
    args: &[&str],
    input: &[u8],
    operation: &'static str,
) -> Result<(), WorkspaceError> {
    let directory = directory
        .try_clone()
        .map_err(|source| WorkspaceError::Io { operation, source })?;
    let mut command = Command::new("git");
    command
        .arg("--git-dir=.")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: `fchdir` is an async-signal-safe syscall, and the closure only
    // accesses the owned directory descriptor captured before spawn.
    unsafe {
        command.pre_exec(move || rustix::process::fchdir(&directory).map_err(std::io::Error::from));
    }
    let mut child = command
        .spawn()
        .map_err(|source| WorkspaceError::Io { operation, source })?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or(WorkspaceError::InvalidGitOutput { operation })?;
    let write_result = stdin.write_all(input).await;
    drop(stdin);
    let status = child
        .wait()
        .await
        .map_err(|source| WorkspaceError::Io { operation, source })?;
    write_result.map_err(|source| WorkspaceError::Io { operation, source })?;
    if !status.success() {
        return Err(WorkspaceError::GitCommandFailed {
            operation,
            status: status.code(),
        });
    }
    Ok(())
}

async fn git_output(
    directory: &Path,
    args: &[&str],
    operation: &'static str,
) -> Result<std::process::Output, WorkspaceError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .await
        .map_err(|source| WorkspaceError::Io { operation, source })?;
    if !output.status.success() {
        return Err(WorkspaceError::GitCommandFailed {
            operation,
            status: output.status.code(),
        });
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::{
        os::unix::fs::{PermissionsExt, symlink},
        path::{Path, PathBuf},
        sync::Arc,
        time::Duration,
    };

    use rustix::fs::open;
    use tempfile::{TempDir, tempdir};
    use tokio::process::Command;

    use crate::domain::{ConversationId, WorkspaceId};

    use super::owned_fs::{removal_device_matches, remove_directory_contents_nofollow};
    use super::*;

    struct TestRepository {
        _temp: TempDir,
        path: PathBuf,
        app_data: PathBuf,
    }

    impl TestRepository {
        async fn new() -> Self {
            let temp = tempdir().unwrap();
            let path = temp.path().join("project");
            let app_data = temp.path().join("app-data");
            std::fs::create_dir(&path).unwrap();
            git(&path, &["init", "--initial-branch=main"]).await;
            git(&path, &["config", "user.name", "Prompting Time Test"]).await;
            git(
                &path,
                &["config", "user.email", "prompting-time@example.test"],
            )
            .await;
            std::fs::write(path.join("tracked.txt"), "initial\n").unwrap();
            git(&path, &["add", "tracked.txt"]).await;
            git(&path, &["commit", "-m", "initial"]).await;

            Self {
                _temp: temp,
                path,
                app_data,
            }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn app_data_dir(&self) -> &Path {
            &self.app_data
        }
    }

    async fn git(directory: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(args)
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    async fn git_stdout(directory: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(args)
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap()
    }

    #[tokio::test]
    async fn dirty_owned_worktree_is_not_removable() {
        let repo = TestRepository::new().await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        std::fs::write(lease.path.join("untracked.txt"), "keep me").unwrap();

        assert_eq!(
            manager.cleanup_eligibility(&lease).await.unwrap(),
            CleanupEligibility::Blocked(WorkspaceBlocker::UntrackedFiles)
        );
        assert!(matches!(
            manager.remove_owned(&lease).await,
            Err(WorkspaceError::RemovalBlocked {
                blocker: WorkspaceBlocker::UntrackedFiles
            })
        ));
        assert!(lease.path.exists());
    }

    #[tokio::test]
    async fn assume_unchanged_cannot_hide_modified_tracked_data() {
        hidden_index_flag_cannot_hide_modified_tracked_data("--assume-unchanged").await;
    }

    #[tokio::test]
    async fn skip_worktree_cannot_hide_modified_tracked_data() {
        hidden_index_flag_cannot_hide_modified_tracked_data("--skip-worktree").await;
    }

    async fn hidden_index_flag_cannot_hide_modified_tracked_data(flag: &str) {
        let repo = TestRepository::new().await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        git(&lease.path, &["update-index", flag, "tracked.txt"]).await;
        let tracked = lease.path.join("tracked.txt");
        std::fs::write(&tracked, "valuable hidden change\n").unwrap();

        assert_eq!(
            manager.cleanup_eligibility(&lease).await.unwrap(),
            CleanupEligibility::Blocked(WorkspaceBlocker::ModifiedTrackedFiles)
        );
        assert!(matches!(
            manager.remove_owned(&lease).await,
            Err(WorkspaceError::RemovalBlocked {
                blocker: WorkspaceBlocker::ModifiedTrackedFiles
            })
        ));
        assert_eq!(
            std::fs::read_to_string(&tracked).unwrap(),
            "valuable hidden change\n"
        );
        assert_ownership_evidence_remains(&repo, &lease).await;
    }

    #[derive(Clone, Copy)]
    enum StagedChange {
        Modification,
        Addition,
        Deletion,
    }

    #[tokio::test]
    async fn staged_modification_blocks_owned_removal() {
        staged_change_blocks_cleanup(StagedChange::Modification, false).await;
    }

    #[tokio::test]
    async fn staged_addition_blocks_owned_removal() {
        staged_change_blocks_cleanup(StagedChange::Addition, false).await;
    }

    #[tokio::test]
    async fn staged_deletion_blocks_owned_removal() {
        staged_change_blocks_cleanup(StagedChange::Deletion, false).await;
    }

    #[tokio::test]
    async fn staged_modification_blocks_cancelled_rollback() {
        staged_change_blocks_cleanup(StagedChange::Modification, true).await;
    }

    #[tokio::test]
    async fn staged_addition_blocks_cancelled_rollback() {
        staged_change_blocks_cleanup(StagedChange::Addition, true).await;
    }

    #[tokio::test]
    async fn staged_deletion_blocks_cancelled_rollback() {
        staged_change_blocks_cleanup(StagedChange::Deletion, true).await;
    }

    async fn staged_change_blocks_cleanup(change: StagedChange, cancelled: bool) {
        let repo = TestRepository::new().await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let (expected_status, valuable_path, expected_contents) = match change {
            StagedChange::Modification => {
                let path = lease.path.join("tracked.txt");
                std::fs::write(&path, "valuable staged modification\n").unwrap();
                git(&lease.path, &["add", "tracked.txt"]).await;
                (
                    "M\ttracked.txt",
                    Some(path),
                    Some("valuable staged modification\n"),
                )
            }
            StagedChange::Addition => {
                let path = lease.path.join("valuable-added.txt");
                std::fs::write(&path, "valuable staged addition\n").unwrap();
                git(&lease.path, &["add", "valuable-added.txt"]).await;
                (
                    "A\tvaluable-added.txt",
                    Some(path),
                    Some("valuable staged addition\n"),
                )
            }
            StagedChange::Deletion => {
                git(&lease.path, &["rm", "tracked.txt"]).await;
                ("D\ttracked.txt", None, None)
            }
        };

        assert_eq!(
            manager.cleanup_eligibility(&lease).await.unwrap(),
            CleanupEligibility::Blocked(WorkspaceBlocker::ModifiedTrackedFiles)
        );
        if cancelled {
            rollback_cancelled_worktree(repo.app_data_dir(), lease.ownership.as_ref().unwrap())
                .await
                .unwrap();
        } else {
            assert!(matches!(
                manager.remove_owned(&lease).await,
                Err(WorkspaceError::RemovalBlocked {
                    blocker: WorkspaceBlocker::ModifiedTrackedFiles
                })
            ));
        }

        assert!(lease.path.exists());
        assert_eq!(
            git_stdout(&lease.path, &["diff", "--cached", "--name-status"])
                .await
                .trim(),
            expected_status
        );
        if let (Some(path), Some(contents)) = (valuable_path, expected_contents) {
            assert_eq!(std::fs::read_to_string(path).unwrap(), contents);
        }
        assert_ownership_evidence_remains(&repo, &lease).await;
    }

    #[tokio::test]
    async fn owner_execute_removal_blocks_owned_removal() {
        owner_execute_removal_blocks_cleanup(false).await;
    }

    #[tokio::test]
    async fn owner_execute_removal_blocks_cancelled_rollback() {
        owner_execute_removal_blocks_cleanup(true).await;
    }

    async fn owner_execute_removal_blocks_cleanup(cancelled: bool) {
        let repo = TestRepository::new().await;
        let executable = repo.path().join("executable.sh");
        std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
        git(repo.path(), &["add", "executable.sh"]).await;
        git(repo.path(), &["commit", "-m", "add executable"]).await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let executable = lease.path.join("executable.sh");
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o655);
        std::fs::set_permissions(&executable, permissions).unwrap();

        assert_eq!(
            manager.cleanup_eligibility(&lease).await.unwrap(),
            CleanupEligibility::Blocked(WorkspaceBlocker::ModifiedTrackedFiles)
        );
        if cancelled {
            rollback_cancelled_worktree(repo.app_data_dir(), lease.ownership.as_ref().unwrap())
                .await
                .unwrap();
        } else {
            assert!(matches!(
                manager.remove_owned(&lease).await,
                Err(WorkspaceError::RemovalBlocked {
                    blocker: WorkspaceBlocker::ModifiedTrackedFiles
                })
            ));
        }

        assert!(lease.path.exists());
        assert_eq!(
            std::fs::metadata(&executable).unwrap().permissions().mode() & 0o777,
            0o655
        );
        assert_ownership_evidence_remains(&repo, &lease).await;
    }

    #[tokio::test]
    async fn ignored_file_blocks_owned_worktree_removal() {
        let repo = TestRepository::new().await;
        std::fs::write(repo.path().join(".gitignore"), ".env\n").unwrap();
        git(repo.path(), &["add", ".gitignore"]).await;
        git(repo.path(), &["commit", "-m", "ignore local environment"]).await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let ignored = lease.path.join(".env");
        std::fs::write(&ignored, "valuable local secret\n").unwrap();

        assert_eq!(
            manager.cleanup_eligibility(&lease).await.unwrap(),
            CleanupEligibility::Blocked(WorkspaceBlocker::UntrackedFiles)
        );
        assert!(matches!(
            manager.remove_owned(&lease).await,
            Err(WorkspaceError::RemovalBlocked {
                blocker: WorkspaceBlocker::UntrackedFiles
            })
        ));
        assert_eq!(
            std::fs::read_to_string(&ignored).unwrap(),
            "valuable local secret\n"
        );
        assert!(
            git_reference_value(repo.path(), &base_reference(lease.conversation_id))
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            git_reference_value(
                repo.path(),
                &format!("refs/heads/{}", worktree_branch(lease.conversation_id)),
            )
            .await
            .unwrap()
            .is_some()
        );
    }

    #[tokio::test]
    async fn projectless_workspace_has_no_worktree() {
        let app_data = tempdir().unwrap();
        let lease = WorkspaceManager::new(app_data.path())
            .prepare(WorkspaceRequest::projectless(ConversationId::new()))
            .await
            .unwrap();

        assert!(lease.project_root.is_none());
        assert!(!lease.owned_worktree);
        assert!(
            lease.path.starts_with(
                std::fs::canonicalize(app_data.path())
                    .unwrap()
                    .join("scratch")
            )
        );
    }

    #[tokio::test]
    async fn projectless_workspace_refuses_a_symlinked_scratch_parent() {
        let temp = tempdir().unwrap();
        let app_data = temp.path().join("app-data");
        let outside = temp.path().join("outside-scratch");
        std::fs::create_dir(&app_data).unwrap();
        std::fs::create_dir(&outside).unwrap();
        symlink(&outside, app_data.join("scratch")).unwrap();
        let conversation_id = ConversationId::new();

        let result = WorkspaceManager::new(&app_data)
            .prepare(WorkspaceRequest::projectless(conversation_id))
            .await;

        assert!(result.is_err());
        assert!(!outside.join(conversation_id.to_string()).exists());
    }

    #[tokio::test]
    async fn projectless_workspace_refuses_an_existing_conversation_directory() {
        let temp = tempdir().unwrap();
        let app_data = temp.path().join("app-data");
        let conversation_id = ConversationId::new();
        let existing = app_data.join("scratch").join(conversation_id.to_string());
        std::fs::create_dir_all(&existing).unwrap();
        let sentinel = existing.join("valuable.txt");
        std::fs::write(&sentinel, "keep me\n").unwrap();

        let result = WorkspaceManager::new(&app_data)
            .prepare(WorkspaceRequest::projectless(conversation_id))
            .await;

        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "keep me\n");
    }

    #[tokio::test]
    async fn prepared_lease_maps_to_the_canonical_workspace_model() {
        let app_data = tempdir().unwrap();
        let conversation_id = ConversationId::new();
        let lease = WorkspaceManager::new(app_data.path())
            .prepare(WorkspaceRequest::projectless(conversation_id))
            .await
            .unwrap();
        let workspace_id = WorkspaceId::new();

        let workspace = lease.workspace(workspace_id);

        assert_eq!(workspace.id, workspace_id);
        assert_eq!(workspace.conversation_id, conversation_id);
        assert_eq!(workspace.project_root, lease.project_root);
        assert_eq!(workspace.execution_path, lease.path);
        assert_eq!(workspace.owned_worktree, lease.owned_worktree);
    }

    #[tokio::test]
    async fn isolated_request_can_use_the_owning_conversation_id() {
        let repo = TestRepository::new().await;
        let conversation_id = ConversationId::new();
        let manager = WorkspaceManager::new(repo.app_data_dir());

        let lease = manager
            .prepare(WorkspaceRequest::isolated_for(conversation_id, repo.path()))
            .await
            .unwrap();

        assert_eq!(lease.conversation_id, conversation_id);
        assert_eq!(
            lease.path.file_name().unwrap(),
            conversation_id.to_string().as_str()
        );
    }

    #[tokio::test]
    async fn modified_tracked_files_block_cleanup() {
        let repo = TestRepository::new().await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        std::fs::write(lease.path.join("tracked.txt"), "changed\n").unwrap();

        assert_eq!(
            manager.cleanup_eligibility(&lease).await.unwrap(),
            CleanupEligibility::Blocked(WorkspaceBlocker::ModifiedTrackedFiles)
        );
        assert!(matches!(
            manager.remove_owned(&lease).await,
            Err(WorkspaceError::RemovalBlocked {
                blocker: WorkspaceBlocker::ModifiedTrackedFiles
            })
        ));
        assert!(lease.path.exists());
    }

    #[tokio::test]
    async fn unique_commits_block_cleanup() {
        let repo = TestRepository::new().await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        std::fs::write(lease.path.join("committed.txt"), "valuable\n").unwrap();
        git(&lease.path, &["add", "committed.txt"]).await;
        git(&lease.path, &["commit", "-m", "valuable work"]).await;

        assert_eq!(
            manager.cleanup_eligibility(&lease).await.unwrap(),
            CleanupEligibility::Blocked(WorkspaceBlocker::UniqueCommits)
        );
        assert!(matches!(
            manager.remove_owned(&lease).await,
            Err(WorkspaceError::RemovalBlocked {
                blocker: WorkspaceBlocker::UniqueCommits
            })
        ));
        assert!(lease.path.exists());
    }

    #[tokio::test]
    async fn active_owned_process_blocks_cleanup() {
        let repo = TestRepository::new().await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let _process = manager
            .owned_processes()
            .register(&lease.path)
            .await
            .unwrap();

        assert_eq!(
            manager.cleanup_eligibility(&lease).await.unwrap(),
            CleanupEligibility::Blocked(WorkspaceBlocker::ActiveProcess)
        );
        assert!(matches!(
            manager.remove_owned(&lease).await,
            Err(WorkspaceError::RemovalBlocked {
                blocker: WorkspaceBlocker::ActiveProcess
            })
        ));
        assert!(lease.path.exists());
    }

    #[tokio::test]
    async fn dropping_owned_process_registration_clears_the_blocker() {
        let repo = TestRepository::new().await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let process = manager
            .owned_processes()
            .register(&lease.path)
            .await
            .unwrap();
        drop(process);

        assert_eq!(
            manager.cleanup_eligibility(&lease).await.unwrap(),
            CleanupEligibility::Eligible
        );
    }

    #[tokio::test]
    async fn managers_for_the_same_app_data_share_owned_process_activity() {
        let repo = TestRepository::new().await;
        let owner = WorkspaceManager::new(repo.app_data_dir());
        let lease = owner
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let _process = owner.owned_processes().register(&lease.path).await.unwrap();
        drop(owner);
        let remover = WorkspaceManager::new(repo.app_data_dir());

        assert!(matches!(
            remover.remove_owned(&lease).await,
            Err(WorkspaceError::RemovalBlocked {
                blocker: WorkspaceBlocker::ActiveProcess
            })
        ));
        assert!(lease.path.exists());
    }

    #[tokio::test]
    async fn app_data_aliases_share_the_lifecycle_gate_and_process_map() {
        let repo = TestRepository::new().await;
        let alias_parent = repo._temp.path().join("missing-when-manager-created");
        let lexical_alias = alias_parent.join("..").join("app-data");
        let alias_manager = WorkspaceManager::new(&lexical_alias);
        std::fs::create_dir(&alias_parent).unwrap();
        let owner = WorkspaceManager::new(repo.app_data_dir());
        let lease = owner
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let symlink_alias = repo._temp.path().join("app-data-alias");
        symlink(repo.app_data_dir(), &symlink_alias).unwrap();
        let symlink_manager = WorkspaceManager::new(&symlink_alias);

        assert!(Arc::ptr_eq(
            &alias_manager.owned_processes.state,
            &owner.owned_processes.state
        ));
        assert!(Arc::ptr_eq(
            &symlink_manager.owned_processes.state,
            &owner.owned_processes.state
        ));
        let _process = alias_manager
            .owned_processes()
            .register(&lease.path)
            .await
            .unwrap();
        assert!(matches!(
            owner.remove_owned(&lease).await,
            Err(WorkspaceError::RemovalBlocked {
                blocker: WorkspaceBlocker::ActiveProcess
            })
        ));
        assert!(lease.path.exists());
    }

    #[tokio::test]
    async fn missing_owned_worktree_blocks_cleanup() {
        let repo = TestRepository::new().await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        git(
            repo.path(),
            &["worktree", "remove", lease.path.to_string_lossy().as_ref()],
        )
        .await;

        assert_eq!(
            manager.cleanup_eligibility(&lease).await.unwrap(),
            CleanupEligibility::Blocked(WorkspaceBlocker::MissingWorktree)
        );
    }

    #[tokio::test]
    async fn physically_missing_registered_worktree_blocks_cleanup() {
        let repo = TestRepository::new().await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let retained = repo._temp.path().join("retained-worktree");
        std::fs::rename(&lease.path, &retained).unwrap();

        assert_eq!(
            manager.cleanup_eligibility(&lease).await.unwrap(),
            CleanupEligibility::Blocked(WorkspaceBlocker::MissingWorktree)
        );
    }

    #[tokio::test]
    async fn canonical_workspace_reconstructs_durable_cleanup_ownership() {
        let repo = TestRepository::new().await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let workspace = lease.workspace(WorkspaceId::new());
        std::fs::write(lease.path.join("committed.txt"), "valuable\n").unwrap();
        git(&lease.path, &["add", "committed.txt"]).await;
        git(&lease.path, &["commit", "-m", "valuable work"]).await;
        drop(lease);
        drop(manager);

        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager.lease(&workspace).await.unwrap();

        assert_eq!(
            manager.cleanup_eligibility(&lease).await.unwrap(),
            CleanupEligibility::Blocked(WorkspaceBlocker::UniqueCommits)
        );
    }

    #[tokio::test]
    async fn failed_repeat_prepare_cannot_move_the_durable_base() {
        let repo = TestRepository::new().await;
        let conversation_id = ConversationId::new();
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated_for(conversation_id, repo.path()))
            .await
            .unwrap();
        std::fs::write(lease.path.join("committed.txt"), "valuable\n").unwrap();
        git(&lease.path, &["add", "committed.txt"]).await;
        git(&lease.path, &["commit", "-m", "valuable work"]).await;

        assert!(
            manager
                .prepare(WorkspaceRequest::isolated_for(conversation_id, &lease.path,))
                .await
                .is_err()
        );
        assert_eq!(
            manager.cleanup_eligibility(&lease).await.unwrap(),
            CleanupEligibility::Blocked(WorkspaceBlocker::UniqueCommits)
        );
    }

    #[tokio::test]
    async fn repeat_prepare_from_the_same_project_recovers_the_owned_lease() {
        let repo = TestRepository::new().await;
        let conversation_id = ConversationId::new();
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let first = manager
            .prepare(WorkspaceRequest::isolated_for(conversation_id, repo.path()))
            .await
            .unwrap();

        let recovered = manager
            .prepare(WorkspaceRequest::isolated_for(conversation_id, repo.path()))
            .await
            .unwrap();

        assert_eq!(recovered.path, first.path);
        assert_eq!(recovered.project_root, first.project_root);
        assert_eq!(
            manager.cleanup_eligibility(&recovered).await.unwrap(),
            CleanupEligibility::Eligible
        );
    }

    #[tokio::test]
    async fn repeat_prepare_recovers_after_the_primary_checkout_advances() {
        let repo = TestRepository::new().await;
        let conversation_id = ConversationId::new();
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let first = manager
            .prepare(WorkspaceRequest::isolated_for(conversation_id, repo.path()))
            .await
            .unwrap();
        std::fs::write(repo.path().join("later.txt"), "later\n").unwrap();
        git(repo.path(), &["add", "later.txt"]).await;
        git(repo.path(), &["commit", "-m", "advance main"]).await;

        let recovered = manager
            .prepare(WorkspaceRequest::isolated_for(conversation_id, repo.path()))
            .await
            .unwrap();

        assert_eq!(recovered.path, first.path);
        assert_eq!(
            recovered.workspace(WorkspaceId::new()).worktree_base_commit,
            first.workspace(WorkspaceId::new()).worktree_base_commit
        );
    }

    #[tokio::test]
    async fn direct_checkout_is_never_owned_or_removable() {
        let repo = TestRepository::new().await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let conversation_id = ConversationId::new();
        let lease = manager
            .prepare(WorkspaceRequest::direct_for(conversation_id, repo.path()))
            .await
            .unwrap();

        assert_eq!(lease.conversation_id, conversation_id);
        assert!(!lease.owned_worktree);
        assert_eq!(
            manager.cleanup_eligibility(&lease).await.unwrap(),
            CleanupEligibility::Blocked(WorkspaceBlocker::NotOwned)
        );
        assert!(matches!(
            manager.remove_owned(&lease).await,
            Err(WorkspaceError::RemovalBlocked {
                blocker: WorkspaceBlocker::NotOwned
            })
        ));
        assert!(repo.path().exists());
    }

    #[tokio::test]
    async fn non_git_directory_can_be_used_directly_without_worktree_ownership() {
        let temp = tempdir().unwrap();
        let project = temp.path().join("plain-project");
        let app_data = temp.path().join("app-data");
        std::fs::create_dir(&project).unwrap();
        let manager = WorkspaceManager::new(app_data);

        let lease = manager
            .prepare(WorkspaceRequest::direct(&project))
            .await
            .unwrap();
        let project = std::fs::canonicalize(project).unwrap();

        assert_eq!(lease.project_root.as_deref(), Some(project.as_path()));
        assert_eq!(lease.path, project);
        assert!(!lease.owned_worktree);
    }

    #[tokio::test]
    async fn invalid_git_metadata_is_not_silently_treated_as_non_git() {
        let temp = tempdir().unwrap();
        let project = temp.path().join("broken-project");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join(".git"), "not valid Git metadata").unwrap();
        let manager = WorkspaceManager::new(temp.path().join("app-data"));

        let error = manager
            .prepare(WorkspaceRequest::direct(&project))
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            WorkspaceError::GitCommandFailed {
                operation: "detect Git project",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn another_manager_cannot_remove_an_owned_worktree() {
        let repo = TestRepository::new().await;
        let owner = WorkspaceManager::new(repo.app_data_dir());
        let lease = owner
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let other_app_data = repo._temp.path().join("other-app-data");
        let other = WorkspaceManager::new(other_app_data);

        assert_eq!(
            other.cleanup_eligibility(&lease).await.unwrap(),
            CleanupEligibility::Blocked(WorkspaceBlocker::NotOwned)
        );
    }

    #[tokio::test]
    async fn worktree_with_reassigned_branch_is_not_treated_as_owned() {
        let repo = TestRepository::new().await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        git(&lease.path, &["branch", "-m", "foreign-branch"]).await;

        assert_eq!(
            manager.cleanup_eligibility(&lease).await.unwrap(),
            CleanupEligibility::Blocked(WorkspaceBlocker::NotOwned)
        );
    }

    #[tokio::test]
    async fn clean_owned_worktree_can_be_removed() {
        let repo = TestRepository::new().await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();

        assert_eq!(
            manager.cleanup_eligibility(&lease).await.unwrap(),
            CleanupEligibility::Eligible
        );
        manager.remove_owned(&lease).await.unwrap();
        assert!(!lease.path.exists());
    }

    #[tokio::test]
    async fn removing_owned_worktree_retains_unrelated_prunable_metadata() {
        let repo = TestRepository::new().await;
        let unrelated = repo._temp.path().join("unrelated-worktree");
        git(
            repo.path(),
            &[
                "worktree",
                "add",
                "-b",
                "unrelated",
                unrelated.to_string_lossy().as_ref(),
            ],
        )
        .await;
        let unrelated = std::fs::canonicalize(unrelated).unwrap();
        let moved_unrelated = repo._temp.path().join("moved-unrelated-worktree");
        std::fs::rename(&unrelated, &moved_unrelated).unwrap();
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();

        manager.remove_owned(&lease).await.unwrap();

        let inventory = git_stdout(repo.path(), &["worktree", "list", "--porcelain"]).await;
        assert!(
            worktree_path_is_listed(&inventory, &unrelated),
            "unrelated worktree missing from inventory: {inventory}"
        );
    }

    #[tokio::test]
    async fn redirected_git_file_cannot_delete_another_worktrees_metadata() {
        let repo = TestRepository::new().await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let owned = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let other = repo._temp.path().join("other-worktree");
        git(
            repo.path(),
            &[
                "worktree",
                "add",
                "-b",
                "other-worktree",
                other.to_string_lossy().as_ref(),
            ],
        )
        .await;
        let other = std::fs::canonicalize(other).unwrap();
        let other_git_file = std::fs::read(other.join(".git")).unwrap();
        std::fs::write(owned.path.join(".git"), other_git_file).unwrap();

        let result = manager.remove_owned(&owned).await;

        assert!(owned.path.exists());
        assert!(result.is_err());
        git(&other, &["status", "--porcelain=v2"]).await;
    }

    #[tokio::test]
    async fn symlinked_git_file_cannot_delete_another_worktrees_metadata() {
        let repo = TestRepository::new().await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let owned = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let other = repo._temp.path().join("other-symlink-worktree");
        git(
            repo.path(),
            &[
                "worktree",
                "add",
                "-b",
                "other-symlink-worktree",
                other.to_string_lossy().as_ref(),
            ],
        )
        .await;
        let other = std::fs::canonicalize(other).unwrap();
        std::fs::remove_file(owned.path.join(".git")).unwrap();
        symlink(other.join(".git"), owned.path.join(".git")).unwrap();

        let result = manager.remove_owned(&owned).await;

        assert!(owned.path.exists());
        assert!(result.is_err());
        git(&other, &["status", "--porcelain=v2"]).await;
    }

    #[tokio::test]
    async fn symlinked_owned_ancestor_cannot_redirect_removal_outside_app_data() {
        let repo = TestRepository::new().await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let owned_parent = lease.path.parent().unwrap();
        let outside_parent = repo._temp.path().join("outside-app-data");
        std::fs::rename(owned_parent, &outside_parent).unwrap();
        symlink(&outside_parent, owned_parent).unwrap();
        let outside_file = outside_parent
            .join(lease.conversation_id.to_string())
            .join("tracked.txt");

        let result = manager.remove_owned(&lease).await;

        assert!(outside_file.exists());
        assert!(matches!(
            result,
            Err(WorkspaceError::RemovalBlocked {
                blocker: WorkspaceBlocker::NotOwned
            })
        ));
    }

    #[tokio::test]
    async fn ancestor_swap_after_validation_never_deletes_the_outside_target() {
        let repo = TestRepository::new().await;
        let mut manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let coordination = install_removal_coordination(&lease.path);
        manager.removal_coordination = Some(coordination.clone());
        let manager = Arc::new(manager);
        let removal = tokio::spawn({
            let manager = Arc::clone(&manager);
            let lease = lease.clone();
            async move { manager.remove_owned(&lease).await }
        });
        coordination.after_validation.wait().await;
        let owned_parent = lease.path.parent().unwrap();
        let outside_parent = repo._temp.path().join("outside-after-validation");
        std::fs::rename(owned_parent, &outside_parent).unwrap();
        symlink(&outside_parent, owned_parent).unwrap();
        let outside_file = outside_parent
            .join(lease.conversation_id.to_string())
            .join("tracked.txt");
        coordination.before_deletion.wait().await;

        let result = removal.await.unwrap();
        assert!(outside_file.exists());
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn directory_swap_after_validation_never_recursively_deletes_either_directory() {
        let repo = TestRepository::new().await;
        let mut manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let coordination = install_removal_coordination(&lease.path);
        manager.removal_coordination = Some(coordination.clone());
        let manager = Arc::new(manager);
        let removal = tokio::spawn({
            let manager = Arc::clone(&manager);
            let lease = lease.clone();
            async move { manager.remove_owned(&lease).await }
        });
        coordination.after_validation.wait().await;
        let displaced = repo._temp.path().join("displaced-owned-worktree");
        std::fs::rename(&lease.path, &displaced).unwrap();
        let displaced_sentinel = displaced.join("tracked.txt");
        std::fs::create_dir(&lease.path).unwrap();
        let replacement_sentinel = lease.path.join("replacement-sentinel.txt");
        std::fs::write(&replacement_sentinel, "replacement\n").unwrap();
        coordination.before_deletion.wait().await;

        let result = removal.await.unwrap();

        assert!(result.is_err());
        assert!(displaced_sentinel.exists());
        assert_eq!(
            std::fs::read_to_string(&replacement_sentinel).unwrap(),
            "replacement\n"
        );
        assert!(
            git_reference_value(repo.path(), &base_reference(lease.conversation_id))
                .await
                .unwrap()
                .is_some()
        );
        let inventory = git_stdout(repo.path(), &["worktree", "list", "--porcelain"]).await;
        assert!(worktree_path_is_listed(&inventory, &lease.path));
    }

    #[tokio::test]
    async fn quarantine_restore_refuses_a_moved_repository_parent() {
        let repo = TestRepository::new().await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let target = open_owned_worktree_target(repo.app_data_dir(), &lease.path).unwrap();
        let quarantined = quarantine_owned_tree(target).unwrap();
        let quarantine_path = quarantined.path().to_owned();
        assert_eq!(
            quarantine_path,
            std::fs::canonicalize(repo.app_data_dir())
                .unwrap()
                .join("worktree-quarantine")
                .join(format!(
                    "owned-{}--{}",
                    repository_id(&std::fs::canonicalize(repo.path()).unwrap()),
                    lease.conversation_id
                ))
        );
        let repository_parent = lease.path.parent().unwrap();
        let displaced_parent = repo._temp.path().join("displaced-before-restore");
        std::fs::rename(repository_parent, &displaced_parent).unwrap();
        symlink(&displaced_parent, repository_parent).unwrap();

        let result = quarantined.restore();

        assert!(matches!(
            result,
            Err(WorkspaceError::QuarantineRetained { .. })
        ));
        assert!(quarantine_path.join("tracked.txt").exists());
        assert!(
            !displaced_parent
                .join(lease.conversation_id.to_string())
                .exists()
        );
    }

    #[tokio::test]
    async fn ignored_file_added_after_validation_is_preserved() {
        late_file_added_after_validation_is_preserved(true).await;
    }

    #[tokio::test]
    async fn untracked_file_added_after_validation_is_preserved() {
        late_file_added_after_validation_is_preserved(false).await;
    }

    #[tokio::test]
    async fn quarantine_namespace_swap_cannot_hide_dirty_data_from_removal() {
        let repo = TestRepository::new().await;
        let manager = Arc::new(WorkspaceManager::new(repo.app_data_dir()));
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let coordination = install_post_quarantine_coordination(&lease.path);
        let removal = tokio::spawn({
            let manager = Arc::clone(&manager);
            let lease = lease.clone();
            async move { manager.remove_owned(&lease).await }
        });
        coordination.quarantined.wait().await;
        let (displaced, replacement) =
            replace_quarantine_entry_with_clean_directory(&repo, &lease, "removal");
        let late = displaced.join("late-valuable.txt");
        std::fs::write(&late, "keep me\n").unwrap();
        coordination.continue_checks.wait().await;

        assert!(removal.await.unwrap().is_err());
        assert_eq!(std::fs::read_to_string(&late).unwrap(), "keep me\n");
        assert!(replacement.join("tracked.txt").exists());
        assert_ownership_evidence_remains(&repo, &lease).await;
    }

    #[tokio::test]
    async fn quarantine_namespace_swap_cannot_hide_dirty_data_from_cancelled_rollback() {
        let repo = TestRepository::new().await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let coordination = install_post_quarantine_coordination(&lease.path);
        let app_data = repo.app_data_dir().to_owned();
        let ownership = lease.ownership.clone().unwrap();
        let rollback =
            tokio::spawn(async move { rollback_cancelled_worktree(&app_data, &ownership).await });
        coordination.quarantined.wait().await;
        let (displaced, replacement) =
            replace_quarantine_entry_with_clean_directory(&repo, &lease, "rollback");
        let late = displaced.join("late-valuable.txt");
        std::fs::write(&late, "keep me\n").unwrap();
        coordination.continue_checks.wait().await;

        assert!(rollback.await.unwrap().is_err());
        assert_eq!(std::fs::read_to_string(&late).unwrap(), "keep me\n");
        assert!(replacement.join("tracked.txt").exists());
        assert_ownership_evidence_remains(&repo, &lease).await;
    }

    #[tokio::test]
    async fn git_file_replacement_after_quarantine_is_not_recursively_deleted() {
        let repo = TestRepository::new().await;
        let manager = Arc::new(WorkspaceManager::new(repo.app_data_dir()));
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let coordination = install_post_quarantine_coordination(&lease.path);
        let removal = tokio::spawn({
            let manager = Arc::clone(&manager);
            let lease = lease.clone();
            async move { manager.remove_owned(&lease).await }
        });
        coordination.quarantined.wait().await;
        let git_entry = quarantine_path(&repo, &lease).join(".git");
        std::fs::remove_file(&git_entry).unwrap();
        std::fs::create_dir(&git_entry).unwrap();
        let sentinel = git_entry.join("valuable.txt");
        std::fs::write(&sentinel, "keep me\n").unwrap();
        coordination.continue_checks.wait().await;

        assert!(removal.await.unwrap().is_err());
        let retained_sentinel = if sentinel.exists() {
            sentinel
        } else {
            lease.path.join(".git").join("valuable.txt")
        };
        assert_eq!(
            std::fs::read_to_string(retained_sentinel).unwrap(),
            "keep me\n"
        );
        assert_ownership_evidence_remains(&repo, &lease).await;
    }

    #[tokio::test]
    async fn admin_metadata_swap_cannot_redirect_final_removal_checks() {
        metadata_namespace_swap_preserves_worktree(false, false).await;
    }

    #[tokio::test]
    async fn admin_metadata_swap_cannot_redirect_cancelled_rollback_checks() {
        metadata_namespace_swap_preserves_worktree(true, false).await;
    }

    #[tokio::test]
    async fn repository_swap_cannot_redirect_final_removal_checks() {
        metadata_namespace_swap_preserves_worktree(false, true).await;
    }

    #[tokio::test]
    async fn repository_swap_cannot_redirect_cancelled_rollback_checks() {
        metadata_namespace_swap_preserves_worktree(true, true).await;
    }

    #[tokio::test]
    async fn repository_symlink_swap_cannot_redirect_cancelled_ref_cleanup() {
        let repo = TestRepository::new().await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let base_reference = base_reference(lease.conversation_id);
        let branch_reference = format!("refs/heads/{}", worktree_branch(lease.conversation_id));
        let coordination = install_post_rollback_removal_coordination(&lease.path);
        let app_data = repo.app_data_dir().to_owned();
        let ownership = lease.ownership.clone().unwrap();
        let rollback =
            tokio::spawn(async move { rollback_cancelled_worktree(&app_data, &ownership).await });

        coordination.worktree_removed.wait().await;
        let retained_repository = repo._temp.path().join("retained-rollback-repository");
        let outside_repository = repo._temp.path().join("outside-replacement-repository");
        std::fs::rename(repo.path(), &retained_repository).unwrap();
        copy_directory_recursively(&retained_repository, &outside_repository);
        symlink(&outside_repository, repo.path()).unwrap();
        coordination.continue_ref_cleanup.wait().await;

        rollback.await.unwrap().unwrap();
        assert!(
            git_reference_value(&outside_repository, &base_reference)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            git_reference_value(&outside_repository, &branch_reference)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            git_reference_value(&retained_repository, &base_reference)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            git_reference_value(&retained_repository, &branch_reference)
                .await
                .unwrap()
                .is_none()
        );
    }

    async fn metadata_namespace_swap_preserves_worktree(cancelled: bool, repository_swap: bool) {
        let repo = TestRepository::new().await;
        let manager = Arc::new(WorkspaceManager::new(repo.app_data_dir()));
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let admin_path = linked_admin_path(&lease);
        let coordination = install_post_quarantine_coordination(&lease.path);
        let operation = if cancelled {
            let app_data = repo.app_data_dir().to_owned();
            let ownership = lease.ownership.clone().unwrap();
            tokio::spawn(async move { rollback_cancelled_worktree(&app_data, &ownership).await })
        } else {
            let manager = Arc::clone(&manager);
            let lease = lease.clone();
            tokio::spawn(async move { manager.remove_owned(&lease).await })
        };
        coordination.quarantined.wait().await;
        if repository_swap {
            let displaced = repo._temp.path().join(if cancelled {
                "displaced-cancelled-repository"
            } else {
                "displaced-removal-repository"
            });
            std::fs::rename(repo.path(), &displaced).unwrap();
            copy_directory_recursively(&displaced, repo.path());
            git(
                &displaced,
                &["update-ref", "-d", &base_reference(lease.conversation_id)],
            )
            .await;
        } else {
            let displaced = repo._temp.path().join(if cancelled {
                "displaced-cancelled-admin"
            } else {
                "displaced-removal-admin"
            });
            std::fs::rename(&admin_path, &displaced).unwrap();
            copy_directory_recursively(&displaced, &admin_path);
            std::fs::write(
                displaced.join("HEAD"),
                "ref: refs/heads/not-the-owned-branch\n",
            )
            .unwrap();
        }
        coordination.continue_checks.wait().await;

        assert!(operation.await.unwrap().is_err());
        assert_eq!(
            std::fs::read_to_string(lease.path.join("tracked.txt")).unwrap(),
            "initial\n"
        );
        assert_ownership_evidence_remains(&repo, &lease).await;
    }

    fn linked_admin_path(lease: &WorkspaceLease) -> PathBuf {
        let git_file = std::fs::read_to_string(lease.path.join(".git")).unwrap();
        PathBuf::from(git_file.trim().strip_prefix("gitdir: ").unwrap())
    }

    fn copy_directory_recursively(source: &Path, destination: &Path) {
        std::fs::create_dir(destination).unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let target = destination.join(entry.file_name());
            let file_type = entry.file_type().unwrap();
            if file_type.is_dir() {
                copy_directory_recursively(&entry.path(), &target);
            } else if file_type.is_symlink() {
                symlink(std::fs::read_link(entry.path()).unwrap(), target).unwrap();
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    fn replace_quarantine_entry_with_clean_directory(
        repo: &TestRepository,
        lease: &WorkspaceLease,
        label: &str,
    ) -> (PathBuf, PathBuf) {
        let quarantine = quarantine_path(repo, lease);
        let displaced = repo._temp.path().join(format!("displaced-{label}"));
        std::fs::rename(&quarantine, &displaced).unwrap();
        std::fs::create_dir(&quarantine).unwrap();
        std::fs::copy(
            displaced.join("tracked.txt"),
            quarantine.join("tracked.txt"),
        )
        .unwrap();
        (displaced, quarantine)
    }

    fn quarantine_path(repo: &TestRepository, lease: &WorkspaceLease) -> PathBuf {
        let repository_id = repository_id(&std::fs::canonicalize(repo.path()).unwrap());
        std::fs::canonicalize(repo.app_data_dir())
            .unwrap()
            .join("worktree-quarantine")
            .join(format!("owned-{repository_id}--{}", lease.conversation_id))
    }

    async fn assert_ownership_evidence_remains(repo: &TestRepository, lease: &WorkspaceLease) {
        assert!(
            git_reference_value(repo.path(), &base_reference(lease.conversation_id))
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            git_reference_value(
                repo.path(),
                &format!("refs/heads/{}", worktree_branch(lease.conversation_id)),
            )
            .await
            .unwrap()
            .is_some()
        );
        let inventory = git_stdout(repo.path(), &["worktree", "list", "--porcelain"]).await;
        assert!(worktree_path_is_listed(&inventory, &lease.path));
    }

    async fn late_file_added_after_validation_is_preserved(ignored: bool) {
        let repo = TestRepository::new().await;
        let file_name = if ignored { "late.env" } else { "late.txt" };
        if ignored {
            std::fs::write(repo.path().join(".gitignore"), "*.env\n").unwrap();
            git(repo.path(), &["add", ".gitignore"]).await;
            git(repo.path(), &["commit", "-m", "ignore environment files"]).await;
        }
        let mut manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let coordination = install_removal_coordination(&lease.path);
        manager.removal_coordination = Some(coordination.clone());
        let manager = Arc::new(manager);
        let removal = tokio::spawn({
            let manager = Arc::clone(&manager);
            let lease = lease.clone();
            async move { manager.remove_owned(&lease).await }
        });
        coordination.after_validation.wait().await;
        let artifact = lease.path.join(file_name);
        std::fs::write(&artifact, "late valuable data\n").unwrap();
        coordination.before_deletion.wait().await;

        assert!(matches!(
            removal.await.unwrap(),
            Err(WorkspaceError::RemovalBlocked {
                blocker: WorkspaceBlocker::UntrackedFiles
            })
        ));
        assert_eq!(
            std::fs::read_to_string(&artifact).unwrap(),
            "late valuable data\n"
        );
        assert!(
            git_reference_value(repo.path(), &base_reference(lease.conversation_id))
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            git_reference_value(
                repo.path(),
                &format!("refs/heads/{}", worktree_branch(lease.conversation_id)),
            )
            .await
            .unwrap()
            .is_some()
        );
        let inventory = git_stdout(repo.path(), &["worktree", "list", "--porcelain"]).await;
        assert!(worktree_path_is_listed(&inventory, &lease.path));
    }

    #[test]
    fn removal_boundary_rejects_a_different_device() {
        assert!(removal_device_matches(41, 41));
        assert!(!removal_device_matches(41, 42));
    }

    #[test]
    fn recursive_removal_refuses_a_different_root_device_without_deleting_contents() {
        const OPERATION: &str = "test cross-device removal refusal";
        let temp = tempdir().unwrap();
        let directory_path = temp.path().join("owned");
        std::fs::create_dir(&directory_path).unwrap();
        let sentinel = directory_path.join("sentinel.txt");
        std::fs::write(&sentinel, "keep me\n").unwrap();
        let directory = open(
            &directory_path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .unwrap();
        let actual_device = fstat(&directory).unwrap().st_dev as u64;

        let result = remove_directory_contents_nofollow(
            &directory,
            actual_device.wrapping_add(1),
            OPERATION,
        );

        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "keep me\n");
    }

    #[tokio::test]
    async fn new_prepare_never_returns_a_worktree_through_a_symlinked_parent() {
        let repo = TestRepository::new().await;
        let project_root = std::fs::canonicalize(repo.path()).unwrap();
        let conversation_id = ConversationId::new();
        let worktrees = repo.app_data_dir().join("worktrees");
        let repository_parent = worktrees.join(repository_id(&project_root));
        let outside_parent = repo._temp.path().join("outside-new-prepare");
        std::fs::create_dir_all(&worktrees).unwrap();
        std::fs::create_dir(&outside_parent).unwrap();
        symlink(&outside_parent, &repository_parent).unwrap();

        let result = WorkspaceManager::new(repo.app_data_dir())
            .prepare(WorkspaceRequest::isolated_for(conversation_id, repo.path()))
            .await;

        assert!(result.is_err());
        assert!(!outside_parent.join(conversation_id.to_string()).exists());
        assert!(outside_parent.exists());
        assert!(
            git_reference_value(repo.path(), &base_reference(conversation_id))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn parent_swap_during_create_returns_no_outside_lease_and_preserves_evidence() {
        let repo = TestRepository::new().await;
        let hooks = repo._temp.path().join("create-swap-hooks");
        let started = repo._temp.path().join("create-swap-started");
        let release = repo._temp.path().join("create-swap-release");
        std::fs::create_dir(&hooks).unwrap();
        let hook = hooks.join("post-checkout");
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\n: > '{}'\nwhile [ ! -e '{}' ]; do sleep 0.01; done\n",
                started.display(),
                release.display()
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&hook, permissions).unwrap();
        git(
            repo.path(),
            &["config", "core.hooksPath", hooks.to_string_lossy().as_ref()],
        )
        .await;
        let project_root = std::fs::canonicalize(repo.path()).unwrap();
        let conversation_id = ConversationId::new();
        let expected_parent = repo
            .app_data_dir()
            .join("worktrees")
            .join(repository_id(&project_root));
        let manager = Arc::new(WorkspaceManager::new(repo.app_data_dir()));
        let preparation = tokio::spawn({
            let manager = Arc::clone(&manager);
            let project = repo.path().to_owned();
            async move {
                manager
                    .prepare(WorkspaceRequest::isolated_for(conversation_id, project))
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !started.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let outside_parent = repo._temp.path().join("outside-create-swap");
        std::fs::rename(&expected_parent, &outside_parent).unwrap();
        symlink(&outside_parent, &expected_parent).unwrap();
        std::fs::write(&release, "continue\n").unwrap();

        let result = preparation.await.unwrap();
        let outside_worktree = outside_parent.join(conversation_id.to_string());

        assert!(result.is_err());
        assert!(outside_worktree.join("tracked.txt").exists());
        assert!(
            git_reference_value(repo.path(), &base_reference(conversation_id))
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            git_reference_value(
                repo.path(),
                &format!("refs/heads/{}", worktree_branch(conversation_id)),
            )
            .await
            .unwrap()
            .is_some()
        );
    }

    #[tokio::test]
    async fn recovered_prepare_never_returns_a_worktree_through_a_symlinked_parent() {
        let repo = TestRepository::new().await;
        let conversation_id = ConversationId::new();
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated_for(conversation_id, repo.path()))
            .await
            .unwrap();
        let owned_parent = lease.path.parent().unwrap();
        let outside_parent = repo._temp.path().join("outside-recovery");
        std::fs::rename(owned_parent, &outside_parent).unwrap();
        symlink(&outside_parent, owned_parent).unwrap();

        let result = manager
            .prepare(WorkspaceRequest::isolated_for(conversation_id, repo.path()))
            .await;

        assert!(result.is_err());
        assert!(
            outside_parent
                .join(conversation_id.to_string())
                .join("tracked.txt")
                .exists()
        );
    }

    #[tokio::test]
    async fn mutable_base_ref_cannot_replace_the_durable_base_commit() {
        let repo = TestRepository::new().await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let workspace = lease.workspace(WorkspaceId::new());
        std::fs::write(lease.path.join("committed.txt"), "valuable\n").unwrap();
        git(&lease.path, &["add", "committed.txt"]).await;
        git(&lease.path, &["commit", "-m", "valuable work"]).await;
        let current = git_stdout(&lease.path, &["rev-parse", "HEAD"]).await;
        let base_reference = base_reference(lease.conversation_id);
        git(
            repo.path(),
            &["update-ref", &base_reference, current.trim()],
        )
        .await;
        let manager = WorkspaceManager::new(repo.app_data_dir());

        assert!(matches!(
            manager.lease(&workspace).await,
            Err(WorkspaceError::RemovalBlocked {
                blocker: WorkspaceBlocker::NotOwned
            })
        ));
    }

    #[tokio::test]
    async fn cancelled_convenience_prepare_rolls_back_created_git_ownership() {
        let repo = TestRepository::new().await;
        let hooks = repo._temp.path().join("hooks");
        let started = repo._temp.path().join("checkout-started");
        let release = repo._temp.path().join("release-checkout");
        std::fs::create_dir(&hooks).unwrap();
        let hook = hooks.join("post-checkout");
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\n: > '{}'\nwhile [ ! -e '{}' ]; do sleep 0.01; done\n",
                started.display(),
                release.display()
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&hook, permissions).unwrap();
        git(
            repo.path(),
            &["config", "core.hooksPath", hooks.to_string_lossy().as_ref()],
        )
        .await;
        let request = WorkspaceRequest::isolated(repo.path());
        let conversation_id = match &request {
            WorkspaceRequest::Isolated {
                conversation_id, ..
            } => *conversation_id,
            _ => unreachable!(),
        };
        let manager = Arc::new(WorkspaceManager::new(repo.app_data_dir()));
        let task = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.prepare(request).await }
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !started.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        std::fs::write(&release, "continue").unwrap();
        let base_reference = base_reference(conversation_id);
        let branch_reference = format!("refs/heads/{}", worktree_branch(conversation_id));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let base_exists = git_reference_value(repo.path(), &base_reference)
                    .await
                    .unwrap()
                    .is_some();
                let branch_exists = git_reference_value(repo.path(), &branch_reference)
                    .await
                    .unwrap()
                    .is_some();
                if !base_exists && !branch_exists {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn unacknowledged_preparation_delivery_rolls_back_git_ownership() {
        let repo = TestRepository::new().await;
        let conversation_id = ConversationId::new();
        let project_root = std::fs::canonicalize(repo.path()).unwrap();
        let app_data_dir = std::fs::canonicalize({
            std::fs::create_dir_all(repo.app_data_dir()).unwrap();
            repo.app_data_dir()
        })
        .unwrap();
        let base_commit = git_stdout(repo.path(), &["rev-parse", "HEAD"])
            .await
            .trim()
            .to_owned();
        let base_revision = base_reference(conversation_id);
        let branch = worktree_branch(conversation_id);
        let path = app_data_dir
            .join("worktrees")
            .join(repository_id(&project_root))
            .join(conversation_id.to_string());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let ownership = create_owned_worktree(
            app_data_dir.clone(),
            project_root,
            path.clone(),
            base_commit,
            base_revision.clone(),
            branch.clone(),
            true,
        )
        .await
        .unwrap();
        let (result_sender, result_receiver) = oneshot::channel();
        let (acknowledgment_sender, acknowledgment_receiver) = oneshot::channel();

        let delivery = tokio::spawn(deliver_prepared_worktree(
            app_data_dir,
            ownership,
            result_sender,
            acknowledgment_receiver,
        ));
        let delivered = result_receiver.await.unwrap().unwrap();
        assert_eq!(delivered.path, path);
        drop(acknowledgment_sender);
        delivery.await.unwrap().unwrap();

        assert!(!path.exists());
        assert!(
            git_reference_value(repo.path(), &base_revision)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            git_reference_value(repo.path(), &format!("refs/heads/{branch}"))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn cancelled_rollback_preserves_ignored_directory_and_ownership() {
        let repo = TestRepository::new().await;
        std::fs::write(repo.path().join(".gitignore"), "local-cache/\n").unwrap();
        git(repo.path(), &["add", ".gitignore"]).await;
        git(repo.path(), &["commit", "-m", "ignore local cache"]).await;
        let manager = WorkspaceManager::new(repo.app_data_dir());
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let ignored = lease.path.join("local-cache").join("valuable.txt");
        std::fs::create_dir(ignored.parent().unwrap()).unwrap();
        std::fs::write(&ignored, "keep me\n").unwrap();
        let ownership = lease.ownership.clone().unwrap();

        rollback_cancelled_worktree(repo.app_data_dir(), &ownership)
            .await
            .unwrap();

        assert_eq!(std::fs::read_to_string(&ignored).unwrap(), "keep me\n");
        assert!(
            git_reference_value(repo.path(), &ownership.base_revision)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            git_reference_value(repo.path(), &format!("refs/heads/{}", ownership.branch),)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn cancelled_prepare_preserves_a_branch_advanced_by_a_checkout_hook() {
        let repo = TestRepository::new().await;
        let hooks = repo._temp.path().join("hooks");
        let started = repo._temp.path().join("commit-created");
        let release = repo._temp.path().join("release-checkout");
        std::fs::create_dir(&hooks).unwrap();
        let hook = hooks.join("post-checkout");
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\nprintf 'valuable\\n' > valuable.txt\ngit add valuable.txt\ngit commit -m 'hook commit' >/dev/null\n: > '{}'\nwhile [ ! -e '{}' ]; do sleep 0.01; done\n",
                started.display(),
                release.display()
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&hook, permissions).unwrap();
        git(
            repo.path(),
            &["config", "core.hooksPath", hooks.to_string_lossy().as_ref()],
        )
        .await;
        let conversation_id = ConversationId::new();
        let manager = Arc::new(WorkspaceManager::new(repo.app_data_dir()));
        let task = tokio::spawn({
            let manager = Arc::clone(&manager);
            let project = repo.path().to_owned();
            async move {
                manager
                    .prepare(WorkspaceRequest::isolated_for(conversation_id, project))
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !started.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        std::fs::write(&release, "continue").unwrap();
        let rollback_completed = Arc::clone(&manager.owned_processes.state.removal_gate)
            .write_owned()
            .await;
        drop(rollback_completed);
        let base_revision = base_reference(conversation_id);

        assert!(
            git_reference_value(repo.path(), &base_revision)
                .await
                .unwrap()
                .is_some()
        );
        let unique = git_stdout(
            repo.path(),
            &[
                "log",
                &format!("{base_revision}..{}", worktree_branch(conversation_id)),
                "--oneline",
            ],
        )
        .await;
        assert!(!unique.trim().is_empty());
    }

    #[tokio::test]
    async fn atomic_cancelled_ref_cleanup_preserves_the_marker_when_the_branch_changed() {
        let repo = TestRepository::new().await;
        let conversation_id = ConversationId::new();
        let base_commit = git_stdout(repo.path(), &["rev-parse", "HEAD"])
            .await
            .trim()
            .to_owned();
        let base_reference = base_reference(conversation_id);
        let branch_reference = format!("refs/heads/{}", worktree_branch(conversation_id));
        git(
            repo.path(),
            &["update-ref", &base_reference, &base_commit, ""],
        )
        .await;
        git(
            repo.path(),
            &["update-ref", &branch_reference, &base_commit, ""],
        )
        .await;
        std::fs::write(repo.path().join("later.txt"), "later\n").unwrap();
        git(repo.path(), &["add", "later.txt"]).await;
        git(repo.path(), &["commit", "-m", "advance branch target"]).await;
        let changed_commit = git_stdout(repo.path(), &["rev-parse", "HEAD"])
            .await
            .trim()
            .to_owned();
        git(
            repo.path(),
            &[
                "update-ref",
                &branch_reference,
                &changed_commit,
                &base_commit,
            ],
        )
        .await;

        assert!(
            delete_cancelled_references(
                repo.path(),
                &branch_reference,
                &base_reference,
                &base_commit,
                true,
            )
            .await
            .is_err()
        );
        assert_eq!(
            git_reference_value(repo.path(), &branch_reference)
                .await
                .unwrap()
                .as_deref(),
            Some(changed_commit.as_str())
        );
        assert_eq!(
            git_reference_value(repo.path(), &base_reference)
                .await
                .unwrap()
                .as_deref(),
            Some(base_commit.as_str())
        );
    }

    #[tokio::test]
    async fn stranded_base_ref_without_branch_or_worktree_is_retryable() {
        let repo = TestRepository::new().await;
        let conversation_id = ConversationId::new();
        let base_commit = git_stdout(repo.path(), &["rev-parse", "HEAD"]).await;
        let base_reference = base_reference(conversation_id);
        git(
            repo.path(),
            &["update-ref", &base_reference, base_commit.trim(), ""],
        )
        .await;

        let lease = WorkspaceManager::new(repo.app_data_dir())
            .prepare(WorkspaceRequest::isolated_for(conversation_id, repo.path()))
            .await
            .unwrap();

        assert_eq!(lease.conversation_id, conversation_id);
        assert_eq!(
            WorkspaceManager::new(repo.app_data_dir())
                .cleanup_eligibility(&lease)
                .await
                .unwrap(),
            CleanupEligibility::Eligible
        );
    }

    #[tokio::test]
    async fn prepare_waits_for_the_shared_lifecycle_reservation() {
        let repo = TestRepository::new().await;
        let manager = Arc::new(WorkspaceManager::new(repo.app_data_dir()));
        let conversation_id = ConversationId::new();
        let reservation = Arc::clone(&manager.owned_processes.state.removal_gate)
            .write_owned()
            .await;
        let mut prepare = tokio::spawn({
            let manager = Arc::clone(&manager);
            let project = repo.path().to_owned();
            async move {
                manager
                    .prepare(WorkspaceRequest::isolated_for(conversation_id, project))
                    .await
            }
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(250), &mut prepare)
                .await
                .is_err()
        );
        drop(reservation);
        assert!(prepare.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn durable_recovery_waits_for_the_shared_lifecycle_reservation() {
        let repo = TestRepository::new().await;
        let manager = Arc::new(WorkspaceManager::new(repo.app_data_dir()));
        let lease = manager
            .prepare(WorkspaceRequest::isolated(repo.path()))
            .await
            .unwrap();
        let workspace = lease.workspace(WorkspaceId::new());
        let reservation = Arc::clone(&manager.owned_processes.state.removal_gate)
            .write_owned()
            .await;
        let mut recovery = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.lease(&workspace).await }
        });

        assert!(
            tokio::time::timeout(Duration::from_millis(250), &mut recovery)
                .await
                .is_err()
        );
        drop(reservation);
        assert!(recovery.await.unwrap().is_ok());
    }

    #[test]
    fn repository_identity_digest_has_a_stable_fixed_vector() {
        assert_eq!(
            repository_id(Path::new("/tmp/example/repository")),
            "9d1a58181c325879a120b5a4867b0bc6f85897423e5f825a2d28a271158cc1f0"
        );
    }

    #[tokio::test]
    async fn projectless_artifacts_are_retained() {
        let app_data = tempdir().unwrap();
        let manager = WorkspaceManager::new(app_data.path());
        let lease = manager
            .prepare(WorkspaceRequest::projectless(ConversationId::new()))
            .await
            .unwrap();
        std::fs::write(lease.path.join("artifact.txt"), "keep me").unwrap();

        assert!(matches!(
            manager.remove_owned(&lease).await,
            Err(WorkspaceError::RemovalBlocked {
                blocker: WorkspaceBlocker::NotOwned
            })
        ));
        assert!(lease.path.join("artifact.txt").exists());
    }
}
