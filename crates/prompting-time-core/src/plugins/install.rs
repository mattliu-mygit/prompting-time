//! Installing a plugin: download a release archive with curl, check its SHA-256, unpack it with
//! tar, check its manifest, move it into the plugins folder, and record where it came from. Each
//! plugin `<name>` keeps its files in `<plugins folder>/<name>/` (a folder install keeps them where
//! they are) and PT's record of it in `<plugins folder>/<name>.json`.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::process::Command;
use tokio::sync::watch;

use super::manifest::{self, Manifest};
use super::run::run_to_end;
use super::{PluginError, PluginHostConfig, PluginSource};

/// Downloads and unpacked archives wait here until they are installed; PT empties it when it opens.
pub(crate) const STAGING_DIR: &str = ".staging";
/// The largest release archive PT downloads. Whooper's is about 4 MB.
const MAX_DOWNLOAD_BYTES: &str = "268435456";

/// One installed plugin as PT recorded and read it.
#[derive(Clone, Debug)]
pub(crate) struct Installed {
    pub(crate) source: PluginSource,
    pub(crate) folder: PathBuf,
    pub(crate) manifest: Manifest,
    pub(crate) settings_schema: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    source: PluginSource,
    folder: PathBuf,
}

/// A plugin checked and ready to install: unpacked in the staging folder, or a folder install
/// checked in place.
pub(crate) struct Prepared {
    pub(crate) installed: Installed,
    /// The staging folder that holds the download and the unpacked files, deleted when dropped.
    _work: Option<WorkFolder>,
}

struct WorkFolder(PathBuf);

impl Drop for WorkFolder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Every plugin whose record and files PT can read. A plugin whose files no longer fit, such as a
/// development folder with a broken manifest, is left out until it is installed again.
pub(crate) fn load_records(plugins_dir: &Path) -> Vec<Installed> {
    let Ok(entries) = fs::read_dir(plugins_dir) else {
        return Vec::new();
    };
    let mut installed = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".json"))
        else {
            continue;
        };
        let Some(record) = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Record>(&bytes).ok())
        else {
            continue;
        };
        let Ok((manifest, settings_schema)) = manifest::load(&record.folder) else {
            continue;
        };
        if manifest.name == name {
            installed.push(Installed {
                source: record.source,
                folder: record.folder,
                manifest,
                settings_schema,
            });
        }
    }
    installed
}

/// Fetches, checks, and unpacks `source` without touching what is installed.
pub(crate) async fn prepare(
    config: &PluginHostConfig,
    source: PluginSource,
    quitting: watch::Receiver<bool>,
) -> Result<Prepared, PluginError> {
    let staging = config.plugins_dir.join(STAGING_DIR);
    match source {
        PluginSource::Release { url, sha256 } => {
            let sha256 = checked_sha256(&sha256)?;
            if !url.starts_with("https://") {
                return Err(PluginError::InvalidRequest(
                    "A release archive's URL must start with https://.".to_owned(),
                ));
            }
            let work = work_folder(&staging)?;
            let archive = work.0.join("archive");
            download(
                &config.downloader,
                &url,
                &archive,
                &staging,
                quitting.clone(),
            )
            .await?;
            let (folder, manifest, settings_schema) =
                unpack(&archive, &sha256, &work.0, &staging, quitting).await?;
            Ok(Prepared {
                installed: Installed {
                    source: PluginSource::Release { url, sha256 },
                    folder,
                    manifest,
                    settings_schema,
                },
                _work: Some(work),
            })
        }
        PluginSource::Archive { path, sha256 } => {
            let sha256 = checked_sha256(&sha256)?;
            absolute(&path)?;
            let work = work_folder(&staging)?;
            let (folder, manifest, settings_schema) =
                unpack(&path, &sha256, &work.0, &staging, quitting).await?;
            Ok(Prepared {
                installed: Installed {
                    source: PluginSource::Archive { path, sha256 },
                    folder,
                    manifest,
                    settings_schema,
                },
                _work: Some(work),
            })
        }
        PluginSource::Folder { path } => {
            absolute(&path)?;
            let folder = path.canonicalize().map_err(|source| PluginError::Io {
                context: format!("could not open {}", path.display()),
                source,
            })?;
            // PT replaces and deletes folders under its plugins folder, so a development folder
            // must live elsewhere.
            if config
                .plugins_dir
                .canonicalize()
                .is_ok_and(|plugins_dir| folder.starts_with(plugins_dir))
            {
                return Err(PluginError::InvalidRequest(
                    "A plugin folder to run in place must be outside Prompting Time's plugins folder."
                        .to_owned(),
                ));
            }
            let (manifest, settings_schema) = manifest::load(&folder)?;
            Ok(Prepared {
                installed: Installed {
                    source: PluginSource::Folder { path },
                    folder,
                    manifest,
                    settings_schema,
                },
                _work: None,
            })
        }
    }
}

/// Installs a prepared plugin in place of any plugin of the same name, whose process the caller
/// has stopped.
pub(crate) fn commit(plugins_dir: &Path, prepared: Prepared) -> Result<Installed, PluginError> {
    // The work folder holds what is being installed, so it is deleted only after the move below.
    let Prepared {
        mut installed,
        _work: _work_folder,
    } = prepared;
    let name = &installed.manifest.name;
    let owned = plugins_dir.join(name);
    remove_owned_folder(plugins_dir, &owned)?;
    if !matches!(installed.source, PluginSource::Folder { .. }) {
        fs::rename(&installed.folder, &owned).map_err(|source| PluginError::Io {
            context: format!("could not move {name} into {}", plugins_dir.display()),
            source,
        })?;
        installed.folder = owned;
    }
    let record = serde_json::to_vec_pretty(&Record {
        source: installed.source.clone(),
        folder: installed.folder.clone(),
    })
    .expect("a plugin record serializes");
    let record_path = plugins_dir.join(format!("{name}.json"));
    let temporary = plugins_dir.join(format!("{name}.json.tmp"));
    fs::write(&temporary, record)
        .and_then(|()| fs::rename(&temporary, &record_path))
        .map_err(|source| PluginError::Io {
            context: format!("could not record {name}'s install"),
            source,
        })?;
    Ok(installed)
}

fn checked_sha256(sha256: &str) -> Result<String, PluginError> {
    let sha256 = sha256.trim().to_ascii_lowercase();
    if super::is_sha256_hex(&sha256) {
        Ok(sha256)
    } else {
        Err(PluginError::InvalidRequest(
            "The SHA-256 must be 64 hexadecimal digits.".to_owned(),
        ))
    }
}

fn absolute(path: &Path) -> Result<(), PluginError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(PluginError::InvalidRequest(format!(
            "{} is not an absolute path.",
            path.display()
        )))
    }
}

fn work_folder(staging: &Path) -> Result<WorkFolder, PluginError> {
    let path = staging.join(uuid::Uuid::now_v7().to_string());
    fs::create_dir(&path).map_err(|source| PluginError::Io {
        context: format!("could not create {}", path.display()),
        source,
    })?;
    Ok(WorkFolder(path))
}

async fn download(
    downloader: &Path,
    url: &str,
    destination: &Path,
    staging: &Path,
    quitting: watch::Receiver<bool>,
) -> Result<(), PluginError> {
    let mut command = Command::new(downloader);
    command
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "30",
            "--max-time",
            "600",
            "--max-filesize",
            MAX_DOWNLOAD_BYTES,
            "--output",
        ])
        .arg(destination)
        .arg(url);
    let finished = run_to_end(command, staging, quitting).await?;
    if finished.status.success() {
        Ok(())
    } else {
        Err(PluginError::Download(failure_text(
            &finished.stderr,
            finished.status,
        )))
    }
}

/// Checks the archive's SHA-256, unpacks it into `work`, and checks the plugin it holds.
async fn unpack(
    archive: &Path,
    expected: &str,
    work: &Path,
    staging: &Path,
    quitting: watch::Receiver<bool>,
) -> Result<(PathBuf, Manifest, String), PluginError> {
    let hashed = archive.to_path_buf();
    let actual = tokio::task::spawn_blocking(move || sha256_file(&hashed))
        .await
        .map_err(|_| PluginError::Io {
            context: format!("could not read {}", archive.display()),
            source: io::Error::other("the hashing task stopped"),
        })?
        .map_err(|source| PluginError::Io {
            context: format!("could not read {}", archive.display()),
            source,
        })?;
    if actual != expected {
        return Err(PluginError::ChecksumMismatch {
            expected: expected.to_owned(),
            actual,
        });
    }
    let target = work.join("unpacked");
    fs::create_dir(&target).map_err(|source| PluginError::Io {
        context: format!("could not create {}", target.display()),
        source,
    })?;
    // macOS's bsdtar refuses entries with absolute paths or `..`, and writing through symlinks,
    // unless it is given -P.
    let mut command = Command::new("/usr/bin/tar");
    command.arg("-xf").arg(archive).arg("-C").arg(&target);
    let finished = run_to_end(command, staging, quitting).await?;
    if !finished.status.success() {
        return Err(PluginError::Archive(failure_text(
            &finished.stderr,
            finished.status,
        )));
    }
    let folder = single_folder(&target)?;
    let (manifest, settings_schema) = manifest::load(&folder)?;
    Ok((folder, manifest, settings_schema))
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    io::copy(&mut file, &mut digest)?;
    Ok(format!("{:x}", digest.finalize()))
}

/// A release archive holds one folder, such as `whooper-0.1.0/`, with the plugin's files.
fn single_folder(target: &Path) -> Result<PathBuf, PluginError> {
    let entries: Vec<_> = fs::read_dir(target)
        .map_err(|source| PluginError::Io {
            context: format!("could not read {}", target.display()),
            source,
        })?
        .collect::<Result<_, _>>()
        .map_err(|source| PluginError::Io {
            context: format!("could not read {}", target.display()),
            source,
        })?;
    match entries.as_slice() {
        [entry] if entry.file_type().is_ok_and(|kind| kind.is_dir()) => Ok(entry.path()),
        _ => Err(PluginError::Archive(
            "the archive must hold exactly one folder, with the plugin's files in it".to_owned(),
        )),
    }
}

/// Deletes `<plugins folder>/<name>`, the copy of an archive install, which only PT writes to.
fn remove_owned_folder(plugins_dir: &Path, owned: &Path) -> Result<(), PluginError> {
    if !owned.exists() {
        return Ok(());
    }
    // Moving it aside first means a delete that fails partway leaves nothing half deleted at the
    // plugin's path.
    let aside = plugins_dir
        .join(STAGING_DIR)
        .join(format!("replaced-{}", uuid::Uuid::now_v7()));
    fs::rename(owned, &aside).map_err(|source| PluginError::Io {
        context: format!("could not replace {}", owned.display()),
        source,
    })?;
    let _ = fs::remove_dir_all(&aside);
    Ok(())
}

fn failure_text(stderr: &[u8], status: std::process::ExitStatus) -> String {
    let text = String::from_utf8_lossy(stderr).trim().to_owned();
    if text.is_empty() {
        match status.code() {
            Some(code) => format!("exited with code {code}"),
            None => "stopped by a signal".to_owned(),
        }
    } else {
        text
    }
}
