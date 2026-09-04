use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    io::Read,
    os::fd::OwnedFd,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use super::{WorkspaceError, canonicalize};
use rustix::fs::{
    AtFlags, Dir, FileType, Mode, OFlags, RenameFlags, fstat, mkdirat, open, openat, readlinkat,
    renameat_with, statat, unlinkat,
};
use sha1::{Digest as Sha1Digest, Sha1};
use sha2::{Digest as Sha2Digest, Sha256};

pub(super) struct WorktreeMetadataTarget {
    pub(super) parent: OwnedFd,
    pub(super) directory: OwnedFd,
    pub(super) name: OsString,
    pub(super) common_directory: OwnedFd,
    pub(super) common_path: PathBuf,
    pub(super) worktree_git_file: OwnedFd,
}

pub(super) struct ValidatedOwnedDirectory {
    #[cfg(test)]
    app_data_path: PathBuf,
    app_data: OwnedFd,
    worktrees: OwnedFd,
    repository: OwnedFd,
    repository_name: OsString,
    directory: OwnedFd,
    name: OsString,
}

pub(super) struct QuarantinedOwnedDirectory {
    original: ValidatedOwnedDirectory,
    quarantine: OwnedFd,
    directory: OwnedFd,
    name: OsString,
    #[cfg(test)]
    path: PathBuf,
}

pub(super) struct IndexedWorktreeEntry {
    pub(super) path: PathBuf,
    pub(super) mode: u32,
    pub(super) object_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum IndexedWorktreeState {
    Clean,
    ModifiedTracked,
    Untracked,
}

impl ValidatedOwnedDirectory {
    pub(super) fn descriptor(&self) -> &OwnedFd {
        &self.directory
    }
}

pub(super) fn create_worktree_parent_nofollow(
    app_data_dir: &Path,
    repository_id: &str,
) -> Result<(), WorkspaceError> {
    const OPERATION: &str = "create worktree parent without following links";
    let flags = directory_flags();
    let app_data = open(app_data_dir, flags, Mode::empty())
        .map_err(|source| rustix_io_error(OPERATION, source))?;
    match mkdirat(&app_data, "worktrees", Mode::RWXU) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
        Err(source) => return Err(rustix_io_error(OPERATION, source)),
    }
    let worktrees = openat(&app_data, "worktrees", flags, Mode::empty())
        .map_err(|source| rustix_io_error(OPERATION, source))?;
    match mkdirat(&worktrees, repository_id, Mode::RWXU) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
        Err(source) => return Err(rustix_io_error(OPERATION, source)),
    }
    openat(&worktrees, repository_id, flags, Mode::empty())
        .map(|_| ())
        .map_err(|source| rustix_io_error(OPERATION, source))
}

pub(super) fn create_scratch_workspace_nofollow(
    app_data_dir: &Path,
    conversation_id: &str,
) -> Result<PathBuf, WorkspaceError> {
    const OPERATION: &str = "create projectless workspace without following links";
    let app_data_path = canonicalize(app_data_dir, "resolve application data directory")?;
    let flags = directory_flags();
    let app_data = open(&app_data_path, flags, Mode::empty())
        .map_err(|source| rustix_io_error(OPERATION, source))?;
    match mkdirat(&app_data, "scratch", Mode::RWXU) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
        Err(source) => return Err(rustix_io_error(OPERATION, source)),
    }
    let scratch = openat(&app_data, "scratch", flags, Mode::empty())
        .map_err(|source| rustix_io_error(OPERATION, source))?;
    match mkdirat(&scratch, conversation_id, Mode::RWXU) {
        Ok(()) => {}
        Err(rustix::io::Errno::EXIST) => {
            return Err(WorkspaceError::OwnershipConflict {
                operation: OPERATION,
            });
        }
        Err(source) => return Err(rustix_io_error(OPERATION, source)),
    }
    let directory = openat(&scratch, conversation_id, flags, Mode::empty())
        .map_err(|source| rustix_io_error(OPERATION, source))?;
    let current_scratch = openat(&app_data, "scratch", flags, Mode::empty())
        .map_err(|source| rustix_io_error(OPERATION, source))?;
    let current_directory = openat(&current_scratch, conversation_id, flags, Mode::empty())
        .map_err(|source| rustix_io_error(OPERATION, source))?;
    if !same_directory_identity(&scratch, &current_scratch, OPERATION)?
        || !same_directory_identity(&directory, &current_directory, OPERATION)?
    {
        return Err(WorkspaceError::OwnershipConflict {
            operation: OPERATION,
        });
    }
    let path = app_data_path.join("scratch").join(conversation_id);
    if canonicalize(&path, "resolve projectless workspace")? != path {
        return Err(WorkspaceError::OwnershipConflict {
            operation: OPERATION,
        });
    }
    Ok(path)
}

pub(super) fn open_owned_worktree_target(
    app_data_dir: &Path,
    expected_path: &Path,
) -> Result<ValidatedOwnedDirectory, WorkspaceError> {
    const OPERATION: &str = "open owned worktree without following links";
    let app_data_dir = canonicalize(app_data_dir, "resolve application data directory")?;
    let relative = expected_path
        .strip_prefix(app_data_dir.join("worktrees"))
        .map_err(|_| WorkspaceError::OwnershipConflict {
            operation: OPERATION,
        })?;
    let components: Vec<OsString> = relative
        .components()
        .map(|component| match component {
            std::path::Component::Normal(name) => Ok(name.to_owned()),
            _ => Err(WorkspaceError::OwnershipConflict {
                operation: OPERATION,
            }),
        })
        .collect::<Result<_, _>>()?;
    if components.len() != 2 {
        return Err(WorkspaceError::OwnershipConflict {
            operation: OPERATION,
        });
    }

    let flags = directory_flags();
    let app_data = open(&app_data_dir, flags, Mode::empty())
        .map_err(|source| rustix_io_error(OPERATION, source))?;
    let worktrees = openat(&app_data, "worktrees", flags, Mode::empty())
        .map_err(|source| rustix_io_error(OPERATION, source))?;
    let repository = openat(&worktrees, &components[0], flags, Mode::empty())
        .map_err(|source| rustix_io_error(OPERATION, source))?;
    let directory = openat(&repository, &components[1], flags, Mode::empty())
        .map_err(|source| rustix_io_error(OPERATION, source))?;
    Ok(ValidatedOwnedDirectory {
        #[cfg(test)]
        app_data_path: app_data_dir,
        app_data,
        worktrees,
        repository,
        repository_name: components[0].clone(),
        directory,
        name: components[1].clone(),
    })
}

pub(super) fn quarantine_owned_tree(
    target: ValidatedOwnedDirectory,
) -> Result<QuarantinedOwnedDirectory, WorkspaceError> {
    const OPERATION: &str = "quarantine owned worktree";
    const QUARANTINE: &str = "worktree-quarantine";
    let flags = directory_flags();

    let current_repository = openat(
        &target.worktrees,
        &target.repository_name,
        flags,
        Mode::empty(),
    )
    .map_err(|source| rustix_io_error(OPERATION, source))?;
    if !same_directory_identity(&target.repository, &current_repository, OPERATION)? {
        return Err(WorkspaceError::OwnershipConflict {
            operation: OPERATION,
        });
    }

    match mkdirat(&target.app_data, QUARANTINE, Mode::RWXU) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
        Err(source) => return Err(rustix_io_error(OPERATION, source)),
    }
    let quarantine = openat(&target.app_data, QUARANTINE, flags, Mode::empty())
        .map_err(|source| rustix_io_error(OPERATION, source))?;
    let quarantine_name = OsString::from(format!(
        "owned-{}--{}",
        target.repository_name.to_string_lossy(),
        target.name.to_string_lossy()
    ));
    renameat_with(
        &target.repository,
        &target.name,
        &quarantine,
        &quarantine_name,
        RenameFlags::NOREPLACE,
    )
    .map_err(|source| rustix_io_error(OPERATION, source))?;

    let quarantined = match openat(&quarantine, &quarantine_name, flags, Mode::empty()) {
        Ok(directory) => directory,
        Err(source) => {
            return match restore_quarantined_entry(&quarantine, &quarantine_name, &target) {
                Ok(()) => Err(rustix_io_error(OPERATION, source)),
                Err(error) => Err(error),
            };
        }
    };
    match same_directory_identity(&target.directory, &quarantined, OPERATION) {
        Ok(true) => {}
        Ok(false) => {
            return match restore_quarantined_entry(&quarantine, &quarantine_name, &target) {
                Ok(()) => Err(WorkspaceError::OwnershipConflict {
                    operation: OPERATION,
                }),
                Err(error) => Err(error),
            };
        }
        Err(error) => {
            return match restore_quarantined_entry(&quarantine, &quarantine_name, &target) {
                Ok(()) => Err(error),
                Err(restore_error) => Err(restore_error),
            };
        }
    }
    #[cfg(test)]
    let path = target.app_data_path.join(QUARANTINE).join(&quarantine_name);
    Ok(QuarantinedOwnedDirectory {
        original: target,
        quarantine,
        directory: quarantined,
        name: quarantine_name,
        #[cfg(test)]
        path,
    })
}

pub(super) fn remove_worktree_metadata_nofollow(
    target: &WorktreeMetadataTarget,
) -> Result<(), WorkspaceError> {
    const OPERATION: &str = "remove owned worktree metadata without following links";
    if !target.namespace_bindings_intact(OPERATION)? {
        return Err(WorkspaceError::OwnershipConflict {
            operation: OPERATION,
        });
    }
    let root_device = fstat(&target.directory)
        .map_err(|source| rustix_io_error(OPERATION, source))?
        .st_dev as u64;
    remove_directory_contents_nofollow(&target.directory, root_device, OPERATION)?;
    if !target.namespace_bindings_intact(OPERATION)? {
        return Err(WorkspaceError::OwnershipConflict {
            operation: OPERATION,
        });
    }
    unlinkat(&target.parent, &target.name, AtFlags::REMOVEDIR)
        .map_err(|source| rustix_io_error(OPERATION, source))
}

impl WorktreeMetadataTarget {
    pub(super) fn descriptor(&self) -> &OwnedFd {
        &self.directory
    }

    pub(super) fn common_descriptor(&self) -> &OwnedFd {
        &self.common_directory
    }

    pub(super) fn namespace_bindings_intact(
        &self,
        operation: &'static str,
    ) -> Result<bool, WorkspaceError> {
        let common = open_directory_path_nofollow(&self.common_path, operation)?;
        if !same_directory_identity(&self.common_directory, &common, operation)? {
            return Ok(false);
        }
        let parent = openat(&common, "worktrees", directory_flags(), Mode::empty())
            .map_err(|source| rustix_io_error(operation, source))?;
        if !same_directory_identity(&self.parent, &parent, operation)? {
            return Ok(false);
        }
        let directory = openat(&parent, &self.name, directory_flags(), Mode::empty())
            .map_err(|source| rustix_io_error(operation, source))?;
        same_directory_identity(&self.directory, &directory, operation)
    }
}

fn restore_quarantined_entry(
    quarantine: &OwnedFd,
    quarantine_name: &std::ffi::OsStr,
    target: &ValidatedOwnedDirectory,
) -> Result<(), WorkspaceError> {
    const OPERATION: &str = "restore quarantined owned worktree";
    let current_repository = openat(
        &target.worktrees,
        &target.repository_name,
        directory_flags(),
        Mode::empty(),
    )
    .map_err(|_| WorkspaceError::QuarantineRetained {
        operation: OPERATION,
    })?;
    match same_directory_identity(&target.repository, &current_repository, OPERATION) {
        Ok(true) => {}
        Ok(false) | Err(_) => {
            return Err(WorkspaceError::QuarantineRetained {
                operation: OPERATION,
            });
        }
    }
    renameat_with(
        quarantine,
        quarantine_name,
        &target.repository,
        &target.name,
        RenameFlags::NOREPLACE,
    )
    .map_err(|_| WorkspaceError::QuarantineRetained {
        operation: OPERATION,
    })?;
    let restored = openat(
        &target.repository,
        &target.name,
        directory_flags(),
        Mode::empty(),
    )
    .map_err(|_| WorkspaceError::QuarantineRestorationFailed {
        operation: OPERATION,
    })?;
    match same_directory_identity(&target.directory, &restored, OPERATION) {
        Ok(true) => Ok(()),
        Ok(false) | Err(_) => Err(WorkspaceError::QuarantineRestorationFailed {
            operation: OPERATION,
        }),
    }
}

impl QuarantinedOwnedDirectory {
    #[cfg(test)]
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn inspect_indexed_contents(
        &self,
        entries: &[IndexedWorktreeEntry],
        expected_git_file: &OwnedFd,
    ) -> Result<IndexedWorktreeState, WorkspaceError> {
        const OPERATION: &str = "inspect quarantined worktree contents";
        if !self.git_file_binding_intact(expected_git_file, OPERATION)? {
            return Err(WorkspaceError::OwnershipConflict {
                operation: OPERATION,
            });
        }
        let expected: HashMap<&Path, &IndexedWorktreeEntry> = entries
            .iter()
            .map(|entry| (entry.path.as_path(), entry))
            .collect();
        let root_device = fstat(&self.directory)
            .map_err(|source| rustix_io_error(OPERATION, source))?
            .st_dev as u64;
        let mut seen = HashSet::new();
        let mut modified = false;
        let mut untracked = false;
        inspect_directory_contents(
            &self.directory,
            Path::new(""),
            root_device,
            &expected,
            &mut seen,
            &mut modified,
            &mut untracked,
            OPERATION,
        )?;
        modified |= expected.keys().any(|path| !seen.contains(*path));
        Ok(if modified {
            IndexedWorktreeState::ModifiedTracked
        } else if untracked {
            IndexedWorktreeState::Untracked
        } else {
            IndexedWorktreeState::Clean
        })
    }

    pub(super) fn restore(self) -> Result<(), WorkspaceError> {
        const OPERATION: &str = "restore quarantined owned worktree";
        match self.path_binds_retained_directory(OPERATION) {
            Ok(true) => {}
            Ok(false) | Err(_) => {
                return Err(WorkspaceError::QuarantineRetained {
                    operation: OPERATION,
                });
            }
        }
        restore_quarantined_entry(&self.quarantine, &self.name, &self.original)
    }

    pub(super) fn remove(self, expected_git_file: &OwnedFd) -> Result<(), WorkspaceError> {
        const OPERATION: &str = "remove quarantined owned worktree";
        let result = (|| {
            if !self.path_binds_retained_directory(OPERATION)? {
                return Err(WorkspaceError::OwnershipConflict {
                    operation: OPERATION,
                });
            }
            if !self.git_file_binding_intact(expected_git_file, OPERATION)? {
                return Err(WorkspaceError::OwnershipConflict {
                    operation: OPERATION,
                });
            }
            let root_device = fstat(&self.directory)
                .map_err(|source| rustix_io_error(OPERATION, source))?
                .st_dev as u64;
            remove_directory_contents_nofollow(&self.directory, root_device, OPERATION)?;
            if !self.path_binds_retained_directory(OPERATION)? {
                return Err(WorkspaceError::OwnershipConflict {
                    operation: OPERATION,
                });
            }
            unlinkat(&self.quarantine, &self.name, AtFlags::REMOVEDIR)
                .map_err(|source| rustix_io_error(OPERATION, source))
        })();
        result.map_err(|_| WorkspaceError::QuarantineRetained {
            operation: OPERATION,
        })
    }

    fn path_binds_retained_directory(
        &self,
        operation: &'static str,
    ) -> Result<bool, WorkspaceError> {
        let current = openat(
            &self.quarantine,
            &self.name,
            directory_flags(),
            Mode::empty(),
        )
        .map_err(|source| rustix_io_error(operation, source))?;
        same_directory_identity(&self.directory, &current, operation)
    }

    fn git_file_binding_intact(
        &self,
        expected: &OwnedFd,
        operation: &'static str,
    ) -> Result<bool, WorkspaceError> {
        let current = openat(
            &self.directory,
            ".git",
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|source| rustix_io_error(operation, source))?;
        same_regular_file_identity(expected, &current, operation)
    }
}

#[allow(clippy::too_many_arguments)]
fn inspect_directory_contents<'a>(
    directory: &OwnedFd,
    relative: &Path,
    root_device: u64,
    expected: &HashMap<&'a Path, &'a IndexedWorktreeEntry>,
    seen: &mut HashSet<PathBuf>,
    modified: &mut bool,
    untracked: &mut bool,
    operation: &'static str,
) -> Result<(), WorkspaceError> {
    let entries = Dir::read_from(directory).map_err(|source| rustix_io_error(operation, source))?;
    for entry in entries {
        let entry = entry.map_err(|source| rustix_io_error(operation, source))?;
        let name = entry.file_name();
        if matches!(name.to_bytes(), b"." | b"..")
            || (relative.as_os_str().is_empty() && name.to_bytes() == b".git")
        {
            continue;
        }
        let path = relative.join(std::ffi::OsStr::from_bytes(name.to_bytes()));
        let stat = statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|source| rustix_io_error(operation, source))?;
        if stat.st_dev as u64 != root_device {
            return Err(WorkspaceError::OwnershipConflict { operation });
        }
        match FileType::from_raw_mode(stat.st_mode) {
            FileType::Directory => {
                if expected.contains_key(path.as_path()) {
                    seen.insert(path);
                    *modified = true;
                    continue;
                }
                let child = openat(directory, name, directory_flags(), Mode::empty())
                    .map_err(|source| rustix_io_error(operation, source))?;
                inspect_directory_contents(
                    &child,
                    &path,
                    root_device,
                    expected,
                    seen,
                    modified,
                    untracked,
                    operation,
                )?;
            }
            FileType::RegularFile => {
                let Some(indexed) = expected.get(path.as_path()) else {
                    *untracked = true;
                    continue;
                };
                seen.insert(path);
                let file = openat(
                    directory,
                    name,
                    OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                    Mode::empty(),
                )
                .map_err(|source| rustix_io_error(operation, source))?;
                let executable = stat.st_mode & 0o100 != 0;
                let expected_executable = indexed.mode == 0o100755;
                if !matches!(indexed.mode, 0o100644 | 0o100755)
                    || executable != expected_executable
                    || !blob_reader_matches(
                        std::fs::File::from(file),
                        stat.st_size as u64,
                        &indexed.object_id,
                        operation,
                    )?
                {
                    *modified = true;
                }
            }
            FileType::Symlink => {
                let Some(indexed) = expected.get(path.as_path()) else {
                    *untracked = true;
                    continue;
                };
                seen.insert(path);
                let target = readlinkat(directory, name, Vec::new())
                    .map_err(|source| rustix_io_error(operation, source))?;
                if indexed.mode != 0o120000
                    || !blob_bytes_match(target.to_bytes(), &indexed.object_id)
                {
                    *modified = true;
                }
            }
            _ => {
                if expected.contains_key(path.as_path()) {
                    seen.insert(path);
                    *modified = true;
                } else {
                    *untracked = true;
                }
            }
        }
    }
    Ok(())
}

fn blob_reader_matches(
    mut reader: impl Read,
    length: u64,
    expected: &str,
    operation: &'static str,
) -> Result<bool, WorkspaceError> {
    let header = format!("blob {length}\0");
    let mut buffer = [0_u8; 16 * 1024];
    match expected.len() {
        40 => {
            let mut digest = Sha1::new();
            digest.update(header.as_bytes());
            loop {
                let read = reader
                    .read(&mut buffer)
                    .map_err(|source| WorkspaceError::Io { operation, source })?;
                if read == 0 {
                    break;
                }
                digest.update(&buffer[..read]);
            }
            Ok(hex_digest(digest.finalize()) == expected)
        }
        64 => {
            let mut digest = Sha256::new();
            digest.update(header.as_bytes());
            loop {
                let read = reader
                    .read(&mut buffer)
                    .map_err(|source| WorkspaceError::Io { operation, source })?;
                if read == 0 {
                    break;
                }
                digest.update(&buffer[..read]);
            }
            Ok(hex_digest(digest.finalize()) == expected)
        }
        _ => Ok(false),
    }
}

fn blob_bytes_match(bytes: &[u8], expected: &str) -> bool {
    let header = format!("blob {}\0", bytes.len());
    match expected.len() {
        40 => {
            let mut digest = Sha1::new();
            digest.update(header.as_bytes());
            digest.update(bytes);
            hex_digest(digest.finalize()) == expected
        }
        64 => {
            let mut digest = Sha256::new();
            digest.update(header.as_bytes());
            digest.update(bytes);
            hex_digest(digest.finalize()) == expected
        }
        _ => false,
    }
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = bytes.as_ref();
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn same_directory_identity(
    expected: &OwnedFd,
    candidate: &OwnedFd,
    operation: &'static str,
) -> Result<bool, WorkspaceError> {
    let expected = fstat(expected).map_err(|source| rustix_io_error(operation, source))?;
    let candidate = fstat(candidate).map_err(|source| rustix_io_error(operation, source))?;
    Ok(
        FileType::from_raw_mode(expected.st_mode) == FileType::Directory
            && FileType::from_raw_mode(candidate.st_mode) == FileType::Directory
            && expected.st_dev == candidate.st_dev
            && expected.st_ino == candidate.st_ino,
    )
}

fn same_regular_file_identity(
    expected: &OwnedFd,
    candidate: &OwnedFd,
    operation: &'static str,
) -> Result<bool, WorkspaceError> {
    let expected = fstat(expected).map_err(|source| rustix_io_error(operation, source))?;
    let candidate = fstat(candidate).map_err(|source| rustix_io_error(operation, source))?;
    Ok(
        FileType::from_raw_mode(expected.st_mode) == FileType::RegularFile
            && FileType::from_raw_mode(candidate.st_mode) == FileType::RegularFile
            && expected.st_dev == candidate.st_dev
            && expected.st_ino == candidate.st_ino,
    )
}

pub(super) fn open_directory_path_nofollow(
    path: &Path,
    operation: &'static str,
) -> Result<OwnedFd, WorkspaceError> {
    if !path.is_absolute() {
        return Err(WorkspaceError::OwnershipConflict { operation });
    }
    let mut directory = open("/", directory_flags(), Mode::empty())
        .map_err(|source| rustix_io_error(operation, source))?;
    for component in path.components() {
        match component {
            std::path::Component::RootDir => {}
            std::path::Component::Normal(name) => {
                directory = openat(&directory, name, directory_flags(), Mode::empty())
                    .map_err(|source| rustix_io_error(operation, source))?;
            }
            _ => return Err(WorkspaceError::OwnershipConflict { operation }),
        }
    }
    Ok(directory)
}

pub(super) fn remove_directory_contents_nofollow(
    directory: &OwnedFd,
    root_device: u64,
    operation: &'static str,
) -> Result<(), WorkspaceError> {
    let directory_device = fstat(directory)
        .map_err(|source| rustix_io_error(operation, source))?
        .st_dev as u64;
    if !removal_device_matches(root_device, directory_device) {
        return Err(WorkspaceError::OwnershipConflict { operation });
    }
    let entries = Dir::read_from(directory).map_err(|source| rustix_io_error(operation, source))?;
    for entry in entries {
        let entry = entry.map_err(|source| rustix_io_error(operation, source))?;
        let name = entry.file_name();
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        match openat(directory, name, directory_flags(), Mode::empty()) {
            Ok(child) => {
                remove_directory_contents_nofollow(&child, root_device, operation)?;
                unlinkat(directory, name, AtFlags::REMOVEDIR)
                    .map_err(|source| rustix_io_error(operation, source))?;
            }
            Err(rustix::io::Errno::NOTDIR | rustix::io::Errno::LOOP) => {
                unlinkat(directory, name, AtFlags::empty())
                    .map_err(|source| rustix_io_error(operation, source))?;
            }
            Err(source) => return Err(rustix_io_error(operation, source)),
        }
    }
    Ok(())
}

pub(super) fn removal_device_matches(root_device: u64, candidate_device: u64) -> bool {
    root_device == candidate_device
}

fn directory_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW
}

pub(super) fn rustix_io_error(
    operation: &'static str,
    source: rustix::io::Errno,
) -> WorkspaceError {
    WorkspaceError::Io {
        operation,
        source: source.into(),
    }
}
