//! Prompting Time's host for background-agent plugins, such as Whooper. A plugin is a folder with a
//! manifest (`manifest.rs`) and the commands it names. PT installs it from a release archive pinned
//! by its SHA-256, or, for development, from a local archive or folder; checks the programs it
//! needs; starts its start command when PT opens, unless a copy already holds the plugin's run lock;
//! runs its actions and returns their output and exit code; serves its page's files; watches its
//! state folder; stops and starts the start command on demand; reads the end of its output; and
//! stops the process group it started when PT quits, leaving a copy started elsewhere running.
//!
//! Beyond the manifest, PT relies on conventions that Whooper follows:
//! - A plugin keeps its state in `$XDG_STATE_HOME/<name>` (default `~/.local/state/<name>`), and PT
//!   passes the XDG_STATE_HOME it watches to every plugin command.
//! - A running start command holds `<state folder>/run.lock/pid`, holding its process ID.
//! - An action named `get-settings` prints the settings as JSON, and one named `save-settings`
//!   saves the settings given as one JSON argument.
//! - An action with `confirm: true` takes `--print`, which changes nothing outside this Mac, saves
//!   the request it would run in a file in the state folder, names that file by its absolute path
//!   in its output, and prints `fingerprint: <hex>`, the file's SHA-256, on its last line; and
//!   `--expect <hex>`, which refuses a request whose fingerprint changed since.
//! - An action the manifest's `status.item_actions` names takes `--item <id>` after its fixed
//!   arguments, naming one item of the `status` action's format-2 output; the window passes it as an
//!   ordinary argument, so a confirm action still gets `--print` or `--expect` after it.

mod install;
pub mod manifest;
mod page;
mod prerequisites;
mod run;
mod watcher;

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::owned_process::OwnedProcess;
use crate::providers::child_path;
use install::{Installed, STAGING_DIR};
pub use manifest::{GET_SETTINGS_ACTION, Manifest, ManifestError, SAVE_SETTINGS_ACTION};
pub use page::{PAGE_FILE_LIMIT, PageFileError};
pub use prerequisites::PrerequisiteStatus;
pub use run::ACTION_OUTPUT_LIMIT;
use watcher::StateWatcher;

const CHANGE_CHANNEL_CAPACITY: usize = 64;
/// How long PT waits for get-settings when it checks prerequisites. The first run of Whooper's
/// scripts has uv download Python and packages, which can take a minute.
const SETTINGS_TIMEOUT: Duration = Duration::from_secs(120);
/// The largest request file PT reads for a confirmation. Whooper's review requests are a few
/// kilobytes; a request past this cannot be confirmed.
const PREVIEW_LIMIT: u64 = 1024 * 1024;
/// The most of a start command's log PT returns for its Logs tab. Whooper writes a few lines per
/// round, so this holds days of rounds; the whole file stays on disk for an editor.
const LOG_READ_LIMIT: u64 = 256 * 1024;
/// The most bytes of arguments PT adds after an action's command, such as the settings
/// save-settings gets as one JSON argument. macOS refuses to start a command whose arguments and
/// environment hold more than 1 MiB, so PT refuses well below that with a message saying why.
const ARGUMENTS_LIMIT: usize = 256 * 1024;
/// The flags with which PT previews a confirm action and runs what the user confirmed.
const CONFIRM_FLAGS: [&str; 2] = ["--print", "--expect"];

pub struct PluginHostConfig {
    /// Where PT keeps installed plugins and its records of them, such as
    /// `<Application Support>/plugins`.
    pub plugins_dir: PathBuf,
    /// The XDG state home under which each plugin keeps its state folder.
    pub state_home: PathBuf,
    /// The program that downloads release archives: `/usr/bin/curl`, or a fake in tests.
    pub downloader: PathBuf,
}

impl PluginHostConfig {
    /// State folders under `$XDG_STATE_HOME`, else `~/.local/state`, as Whooper finds its own, and
    /// downloads with macOS's curl. None without an absolute HOME or XDG_STATE_HOME.
    pub fn from_environment(plugins_dir: PathBuf) -> Option<Self> {
        let absolute = |name| {
            env::var_os(name)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        let state_home = absolute("XDG_STATE_HOME")
            .or_else(|| absolute("HOME").map(|home| home.join(".local/state")))?;
        Some(Self {
            plugins_dir,
            state_home,
            downloader: PathBuf::from("/usr/bin/curl"),
        })
    }
}

/// Where a plugin comes from. Release and archive installs copy the plugin into PT's plugins folder;
/// a folder install runs the folder in place.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PluginSource {
    /// A release archive downloaded over HTTPS, such as the asset of a GitHub release for a version
    /// tag, refused unless its SHA-256 is `sha256`.
    Release { url: String, sha256: String },
    /// A release archive on this Mac, such as one Whooper's `scripts/build-release` built.
    Archive { path: PathBuf, sha256: String },
    /// A plugin folder on this Mac, for developing a plugin.
    Folder { path: PathBuf },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginSnapshot {
    pub manifest: Manifest,
    pub source: PluginSource,
    /// Where the plugin's files are.
    pub folder: PathBuf,
    /// The folder PT watches for the plugin's state.
    pub state_folder: PathBuf,
    /// The settings' JSON Schema, as the plugin's file holds it.
    pub settings_schema: String,
    pub prerequisites: Vec<PrerequisiteStatus>,
    /// Why PT could not read the settings that decide which prerequisites are needed. PT does not
    /// start a plugin whose settings it cannot read.
    pub settings_error: Option<String>,
    pub process: PluginProcess,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PluginProcess {
    /// PT started the start command, which runs; PT stops its process group when PT quits.
    Started { pid: u32 },
    /// Another copy holds the run lock, such as one started from a terminal; PT leaves it alone.
    RunningElsewhere { pid: u32 },
    /// Nothing runs. `exit` is how the copy PT last started ended, if it did.
    NotRunning { exit: Option<PluginExit> },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginExit {
    /// None when a signal ended the process.
    pub code: Option<i32>,
    /// The end of the start command's output.
    pub output: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionRequest {
    pub plugin: String,
    pub action: String,
    /// Arguments after the action's fixed ones, such as a run ID.
    pub args: Vec<String>,
    /// For an action with `confirm: true`: None runs it with `--print`, which changes nothing and
    /// returns the request and its fingerprint for the user to confirm; the confirmed fingerprint
    /// then runs it for real with `--expect <fingerprint>`.
    pub confirmed_fingerprint: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionOutput {
    /// None when a signal ended the command.
    pub exit_code: Option<i32>,
    /// The start of what the action printed, at most `ACTION_OUTPUT_LIMIT` bytes.
    pub stdout: String,
    /// Whether the action printed more than `ACTION_OUTPUT_LIMIT` bytes, so `stdout` is cut off.
    pub stdout_truncated: bool,
    pub stderr: String,
    /// The fingerprint a `--print` run printed, for the confirmation.
    pub fingerprint: Option<String>,
    /// What a `--print` run will do, for the confirmation: the text of the file it saved, whose
    /// SHA-256 is the fingerprint, such as Whooper's `submit-request.json`.
    pub preview: Option<String>,
}

/// Something about a plugin changed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginChange {
    pub plugin: String,
    /// The files that changed in the plugin's state folder, relative to it, such as
    /// `runs/<run>/drafts.json`. Empty when the change is PT's own: an install, a prerequisite
    /// check, or the start command starting or ending. None when more files changed than one change
    /// lists, or the watch lost track.
    pub files: Option<Vec<String>>,
}

/// The end of what a plugin's start command printed since PT last started it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginLog {
    /// `<plugins folder>/<name>.log`, which each start empties.
    pub path: PathBuf,
    /// The end of the file, from its first whole line; empty when the file does not exist.
    pub text: String,
    /// Whether the file holds more than `text`.
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("{0}")]
    InvalidRequest(String),
    #[error("the download failed: {0}")]
    Download(String),
    #[error("the archive's SHA-256 is {actual}, not the expected {expected}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("the archive could not be unpacked: {0}")]
    Archive(String),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("no plugin named {0} is installed")]
    UnknownPlugin(String),
    #[error("{plugin} has no action named {action}")]
    UnknownAction { plugin: String, action: String },
    #[error("{0} printed no fingerprint line, so there is nothing to confirm")]
    MissingFingerprint(String),
    #[error(
        "{0} named no file in its state folder whose SHA-256 is its fingerprint, so there is nothing to show for confirmation"
    )]
    MissingPreview(String),
    #[error(
        "{plugin} runs as process {pid}, which Prompting Time did not start, so it leaves that copy running"
    )]
    StartedElsewhere { plugin: String, pid: u32 },
    #[error("Prompting Time is quitting")]
    Quitting,
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },
}

/// Installs, starts, runs, watches, and stops plugins. Clones share one host.
#[derive(Clone)]
pub struct PluginHost {
    inner: Arc<Inner>,
}

struct Inner {
    config: PluginHostConfig,
    plugins: Mutex<BTreeMap<String, Plugin>>,
    /// Serializes installs, starts, and shutdown, which replace and start processes.
    operations: tokio::sync::Mutex<()>,
    /// One lock per plugin name, kept across installs: each action holds it shared while it runs,
    /// and an install holds it exclusively while it replaces the plugin's files, so no action runs
    /// from files an install deletes.
    action_locks: Mutex<BTreeMap<String, Arc<tokio::sync::RwLock<()>>>>,
    changes: broadcast::Sender<PluginChange>,
    quitting: watch::Sender<bool>,
    /// Each command PT runs holds a clone until it ends; shutdown takes this one and waits until the
    /// channel closes.
    runs: Mutex<Option<mpsc::Sender<()>>>,
    runs_finished: tokio::sync::Mutex<mpsc::Receiver<()>>,
    generations: AtomicU64,
}

struct Plugin {
    installed: Installed,
    state_dir: PathBuf,
    prerequisites: Vec<PrerequisiteStatus>,
    settings_error: Option<String>,
    started: Option<Started>,
    last_exit: Option<PluginExit>,
    watcher: Option<StateWatcher>,
}

/// The start command PT started, owned by its supervising task.
struct Started {
    pid: u32,
    generation: u64,
    stop: oneshot::Sender<()>,
    supervisor: JoinHandle<()>,
}

impl PluginHost {
    /// Reads the installed plugins; nothing starts until `start_installed`.
    pub fn new(config: PluginHostConfig) -> io::Result<Self> {
        fs::create_dir_all(&config.plugins_dir)?;
        let staging = config.plugins_dir.join(STAGING_DIR);
        // Leftovers of an install PT quit in the middle of.
        match fs::remove_dir_all(&staging) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        fs::create_dir(&staging)?;
        let (changes, _) = broadcast::channel(CHANGE_CHANNEL_CAPACITY);
        let (runs, runs_finished) = mpsc::channel(1);
        let installed = install::load_records(&config.plugins_dir);
        let inner = Arc::new(Inner {
            config,
            plugins: Mutex::new(BTreeMap::new()),
            operations: tokio::sync::Mutex::new(()),
            action_locks: Mutex::new(BTreeMap::new()),
            changes,
            quitting: watch::channel(false).0,
            runs: Mutex::new(Some(runs)),
            runs_finished: tokio::sync::Mutex::new(runs_finished),
            generations: AtomicU64::new(1),
        });
        for installed in installed {
            inner.insert(installed);
        }
        Ok(Self { inner })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PluginChange> {
        self.inner.changes.subscribe()
    }

    /// What PT does for each installed plugin when it opens: watch its state folder, check its
    /// prerequisites, and start its start command unless a copy holds the run lock.
    pub async fn start_installed(&self) {
        let _operations = self.inner.operations.lock().await;
        let names: Vec<String> = self.inner.plugins().keys().cloned().collect();
        for name in names {
            if *self.inner.quitting.borrow() {
                return;
            }
            self.inner.watch(&name);
            self.inner.apply_start_rule(&name).await;
        }
    }

    /// Installs a plugin in place of any plugin of the same name, then starts it as PT does when it
    /// opens. Nothing changes when the download, checksum, archive, or manifest fails. The
    /// replacement waits until the actions running from the copy it replaces end, and actions
    /// called meanwhile wait for the new copy; an action that runs for minutes, such as a question
    /// to a coding agent, holds the install up that long.
    pub async fn install(&self, source: PluginSource) -> Result<PluginSnapshot, PluginError> {
        let _run = self.inner.begin_run()?;
        let prepared =
            install::prepare(&self.inner.config, source, self.inner.quitting.subscribe()).await?;
        let _operations = self.inner.operations.lock().await;
        let name = prepared.installed.manifest.name.clone();
        let actions = self.inner.action_lock(&name).write_owned().await;
        if *self.inner.quitting.borrow() {
            return Err(PluginError::Quitting);
        }
        // The copy being replaced stops first, so its process never runs on files that are gone.
        self.inner.remove(&name).await;
        let installed = install::commit(&self.inner.config.plugins_dir, prepared)?;
        self.inner.insert(installed);
        drop(actions);
        self.inner.watch(&name);
        self.inner.apply_start_rule(&name).await;
        self.inner
            .snapshot(&name)
            .await
            .ok_or(PluginError::UnknownPlugin(name))
    }

    pub async fn list(&self) -> Vec<PluginSnapshot> {
        let names: Vec<String> = self.inner.plugins().keys().cloned().collect();
        let mut snapshots = Vec::with_capacity(names.len());
        for name in names {
            if let Some(snapshot) = self.inner.snapshot(&name).await {
                snapshots.push(snapshot);
            }
        }
        snapshots
    }

    /// Runs one of a plugin's actions to its end and returns its output and exit code, which say
    /// how it went: Whooper's submit exits 10 to 13 for its refusals, for example.
    pub async fn run_action(&self, request: ActionRequest) -> Result<ActionOutput, PluginError> {
        self.inner.installed(&request.plugin)?;
        let _running = self.inner.action_lock(&request.plugin).read_owned().await;
        // Read again under the lock: an install may have replaced the plugin while this waited.
        let installed = self.inner.installed(&request.plugin)?;
        let action = installed
            .manifest
            .action(&request.action)
            .cloned()
            .ok_or_else(|| PluginError::UnknownAction {
                plugin: request.plugin.clone(),
                action: request.action.clone(),
            })?;
        let mut args = request.args;
        // PT adds these flags itself, so no caller, such as a page or a row's item ID, can run a
        // confirm action unconfirmed or with a fingerprint the user never saw.
        if action.confirm
            && let Some(flag) = args.iter().find(|arg| {
                CONFIRM_FLAGS.iter().any(|flag| {
                    arg.strip_prefix(flag)
                        .is_some_and(|rest| rest.is_empty() || rest.starts_with('='))
                })
            })
        {
            return Err(PluginError::InvalidRequest(format!(
                "{} asks to confirm first, so Prompting Time adds --print and --expect itself; its arguments may not hold {flag}.",
                action.name
            )));
        }
        if args.iter().map(|arg| arg.len() + 1).sum::<usize>() > ARGUMENTS_LIMIT {
            return Err(PluginError::InvalidRequest(format!(
                "The arguments for {} hold more than {} KiB, the most Prompting Time passes to a plugin's command.",
                action.name,
                ARGUMENTS_LIMIT / 1024
            )));
        }
        let preview = match (action.confirm, request.confirmed_fingerprint) {
            (false, None) => false,
            (false, Some(_)) => {
                return Err(PluginError::InvalidRequest(format!(
                    "{} runs without a confirmation, so it takes no fingerprint.",
                    action.name
                )));
            }
            (true, None) => {
                args.push("--print".to_owned());
                true
            }
            (true, Some(fingerprint)) => {
                if !is_sha256_hex(&fingerprint) {
                    return Err(PluginError::InvalidRequest(
                        "The confirmed fingerprint must be 64 lowercase hexadecimal digits."
                            .to_owned(),
                    ));
                }
                args.extend(["--expect".to_owned(), fingerprint]);
                false
            }
        };
        let finished = self
            .inner
            .run_command(&installed.folder, &action.command, args)
            .await?;
        let output = ActionOutput {
            exit_code: finished.status.code(),
            stdout: String::from_utf8_lossy(&finished.stdout).into_owned(),
            stdout_truncated: finished.stdout_truncated,
            stderr: String::from_utf8_lossy(&finished.stderr).into_owned(),
            fingerprint: None,
            preview: None,
        };
        if preview && output.exit_code == Some(0) {
            let fingerprint = printed_fingerprint(&output.stdout)
                .ok_or_else(|| PluginError::MissingFingerprint(action.name.clone()))?;
            let state_dir = self.inner.config.state_home.join(&installed.manifest.name);
            let preview = saved_preview(&output.stdout, &state_dir, &fingerprint)
                .ok_or(PluginError::MissingPreview(action.name))?;
            return Ok(ActionOutput {
                fingerprint: Some(fingerprint),
                preview: Some(preview),
                ..output
            });
        }
        Ok(output)
    }

    /// Stops the start command PT started for `name`, as quitting does: SIGTERM to its process
    /// group, then SIGKILL after the grace period, which ends a review in progress. It stays stopped
    /// until `start` or the next time PT opens. PT refuses to stop a copy started elsewhere, and
    /// stopping a plugin that does not run changes nothing.
    pub async fn stop(&self, name: &str) -> Result<PluginSnapshot, PluginError> {
        let _operations = self.inner.operations.lock().await;
        let (state_dir, start_program, started) = {
            let plugins = self.inner.plugins();
            let plugin = plugins
                .get(name)
                .ok_or_else(|| PluginError::UnknownPlugin(name.to_owned()))?;
            (
                plugin.state_dir.clone(),
                plugin.installed.manifest.start_program().to_owned(),
                plugin.started.is_some(),
            )
        };
        if !started && let Some(pid) = run::run_lock_holder(&state_dir, &start_program).await {
            return Err(PluginError::StartedElsewhere {
                plugin: name.to_owned(),
                pid,
            });
        }
        self.inner.stop_started(name).await;
        self.inner
            .snapshot(name)
            .await
            .ok_or_else(|| PluginError::UnknownPlugin(name.to_owned()))
    }

    /// Starts the start command of a plugin that is stopped or exited, as PT does when it opens:
    /// unless a needed program is missing, its settings cannot be read, or a copy holds the run
    /// lock. The snapshot says whether it started, and why not.
    pub async fn start(&self, name: &str) -> Result<PluginSnapshot, PluginError> {
        let _operations = self.inner.operations.lock().await;
        if *self.inner.quitting.borrow() {
            return Err(PluginError::Quitting);
        }
        self.inner.installed(name)?;
        self.inner.apply_start_rule(name).await;
        self.inner
            .snapshot(name)
            .await
            .ok_or_else(|| PluginError::UnknownPlugin(name.to_owned()))
    }

    /// The end of what the plugin's start command printed since PT last started it, for its Logs
    /// tab. A copy started elsewhere prints where it was started, not here.
    pub fn read_log(&self, name: &str) -> Result<PluginLog, PluginError> {
        self.inner.installed(name)?;
        let path = self.inner.log_path(name);
        let (text, truncated) = run::log_tail(&path, LOG_READ_LIMIT);
        Ok(PluginLog {
            path,
            text,
            truncated,
        })
    }

    /// Runs the plugin's save-settings action with `values`, a JSON object of the settings to
    /// change. The plugin checks and saves them; a refusal comes back as its exit code and stderr.
    /// A saved change can change which prerequisites are needed, so PT checks them again.
    pub async fn save_settings(
        &self,
        plugin: &str,
        values: &str,
    ) -> Result<ActionOutput, PluginError> {
        let values: serde_json::Map<String, Value> =
            serde_json::from_str(values).map_err(|_| {
                PluginError::InvalidRequest("The settings must be a JSON object.".to_owned())
            })?;
        let output = self
            .run_action(ActionRequest {
                plugin: plugin.to_owned(),
                action: SAVE_SETTINGS_ACTION.to_owned(),
                args: vec![Value::Object(values).to_string()],
                confirmed_fingerprint: None,
            })
            .await?;
        if output.exit_code == Some(0) {
            let installed = self.inner.installed(plugin)?;
            let (prerequisites, settings_error) = self.inner.check_prerequisites(&installed).await;
            if let Some(entry) = self.inner.plugins().get_mut(plugin) {
                entry.prerequisites = prerequisites;
                entry.settings_error = settings_error;
            }
            self.inner.notify(plugin);
        }
        Ok(output)
    }

    /// What PT does when it quits: stop every command it runs and every start command it started,
    /// each with SIGTERM to its process group and SIGKILL after a grace period, and stop watching.
    /// A copy started elsewhere keeps running.
    pub async fn shutdown(&self) {
        let inner = &self.inner;
        inner.quitting.send_replace(true);
        drop(inner.runs_sender().take());
        let _operations = inner.operations.lock().await;
        let (started, watchers): (Vec<Started>, Vec<StateWatcher>) = {
            let mut plugins = inner.plugins();
            let started = plugins
                .values_mut()
                .filter_map(|plugin| plugin.started.take())
                .collect();
            let watchers = plugins
                .values_mut()
                .filter_map(|plugin| plugin.watcher.take())
                .collect();
            (started, watchers)
        };
        drop(watchers);
        // Each supervisor stops its own process group, so the groups stop at the same time.
        let supervisors: Vec<JoinHandle<()>> = started
            .into_iter()
            .map(|started| {
                let _ = started.stop.send(());
                started.supervisor
            })
            .collect();
        for supervisor in supervisors {
            let _ = supervisor.await;
        }
        let mut runs_finished = inner.runs_finished.lock().await;
        let _ = tokio::time::timeout(
            run::STOP_GRACE + Duration::from_secs(3),
            runs_finished.recv(),
        )
        .await;
    }
}

impl Inner {
    fn plugins(&self) -> MutexGuard<'_, BTreeMap<String, Plugin>> {
        self.plugins
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn runs_sender(&self) -> MutexGuard<'_, Option<mpsc::Sender<()>>> {
        self.runs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn action_lock(&self, name: &str) -> Arc<tokio::sync::RwLock<()>> {
        let mut locks = self
            .action_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::clone(locks.entry(name.to_owned()).or_default())
    }

    fn begin_run(&self) -> Result<mpsc::Sender<()>, PluginError> {
        self.runs_sender().clone().ok_or(PluginError::Quitting)
    }

    fn staging(&self) -> PathBuf {
        self.config.plugins_dir.join(STAGING_DIR)
    }

    /// Where the start command's output goes.
    fn log_path(&self, name: &str) -> PathBuf {
        self.config.plugins_dir.join(format!("{name}.log"))
    }

    fn insert(&self, installed: Installed) {
        let name = installed.manifest.name.clone();
        let plugin = Plugin {
            state_dir: self.config.state_home.join(&name),
            // Until PT reads the settings, a program that depends on them counts as needed.
            prerequisites: prerequisites::check(
                &installed.manifest.prerequisites,
                None,
                &child_path(),
            ),
            installed,
            settings_error: None,
            started: None,
            last_exit: None,
            watcher: None,
        };
        self.plugins().insert(name, plugin);
    }

    fn installed(&self, name: &str) -> Result<Installed, PluginError> {
        self.plugins()
            .get(name)
            .map(|plugin| plugin.installed.clone())
            .ok_or_else(|| PluginError::UnknownPlugin(name.to_owned()))
    }

    fn notify(&self, name: &str) {
        let _ = self.changes.send(PluginChange {
            plugin: name.to_owned(),
            files: Some(Vec::new()),
        });
    }

    fn watch(&self, name: &str) {
        let mut plugins = self.plugins();
        let Some(plugin) = plugins.get_mut(name) else {
            return;
        };
        if plugin.watcher.is_none() {
            // A plugin works without the watch; the UI then hears of its files only when it asks.
            plugin.watcher =
                StateWatcher::start(name.to_owned(), &plugin.state_dir, self.changes.clone()).ok();
        }
    }

    /// Stops the start command PT started for `name`, if any, and forgets the plugin.
    async fn remove(&self, name: &str) {
        self.stop_started(name).await;
        let removed = self.plugins().remove(name);
        // Its watch stops here, outside the lock.
        drop(removed);
    }

    async fn stop_started(&self, name: &str) {
        let started = self
            .plugins()
            .get_mut(name)
            .and_then(|plugin| plugin.started.take());
        if let Some(started) = started {
            let _ = started.stop.send(());
            let _ = started.supervisor.await;
        }
    }

    /// Checks the prerequisites and starts the start command, unless one is missing, PT is
    /// quitting, or a copy already holds the run lock.
    async fn apply_start_rule(self: &Arc<Self>, name: &str) {
        let Some((installed, state_dir, running)) = self.plugins().get(name).map(|plugin| {
            (
                plugin.installed.clone(),
                plugin.state_dir.clone(),
                plugin.started.is_some(),
            )
        }) else {
            return;
        };
        if running {
            return;
        }
        let (prerequisites, settings_error) = self.check_prerequisites(&installed).await;
        let ready = settings_error.is_none()
            && prerequisites
                .iter()
                .all(|prerequisite| !prerequisite.needed || prerequisite.path.is_some());
        if let Some(plugin) = self.plugins().get_mut(name) {
            plugin.prerequisites = prerequisites;
            plugin.settings_error = settings_error;
        }
        // Read before the await below: the watch's guard must not be held across it.
        let quitting = *self.quitting.borrow();
        if ready
            && !quitting
            && run::run_lock_holder(&state_dir, installed.manifest.start_program())
                .await
                .is_none()
        {
            self.start(name, &installed);
        }
        self.notify(name);
    }

    fn start(self: &Arc<Self>, name: &str, installed: &Installed) {
        let start = &installed.manifest.start;
        let command = run::plugin_command(
            &installed.folder.join(&start[0]),
            &start[1..],
            &installed.folder,
            &self.config.state_home,
        );
        let log = self.log_path(name);
        // The lock stays held from the spawn until Started is stored, so the supervisor cannot
        // record an exit before PT records the start.
        let mut plugins = self.plugins();
        let Some(plugin) = plugins.get_mut(name) else {
            return;
        };
        match run::spawn_start(command, &log) {
            Ok(process) => {
                let generation = self.generations.fetch_add(1, Ordering::Relaxed);
                let (stop, stopped) = oneshot::channel();
                plugin.started = Some(Started {
                    pid: process.id().unwrap_or_default(),
                    generation,
                    stop,
                    supervisor: tokio::spawn(supervise(
                        Arc::downgrade(self),
                        name.to_owned(),
                        generation,
                        process,
                        stopped,
                        log,
                    )),
                });
                plugin.last_exit = None;
            }
            Err(error) => {
                plugin.last_exit = Some(PluginExit {
                    code: None,
                    output: format!("Prompting Time could not start {}: {error}", start[0]),
                });
            }
        }
    }

    /// Records how the copy PT started ended: `None` when PT stopped it on purpose.
    fn record_exit(&self, name: &str, generation: u64, exit: Option<PluginExit>) {
        let mut plugins = self.plugins();
        let Some(plugin) = plugins.get_mut(name) else {
            return;
        };
        match &plugin.started {
            // A newer copy runs, so this exit is old news.
            Some(started) if started.generation != generation => return,
            Some(_) => plugin.started = None,
            None => {}
        }
        plugin.last_exit = exit;
    }

    async fn check_prerequisites(
        &self,
        installed: &Installed,
    ) -> (Vec<PrerequisiteStatus>, Option<String>) {
        let manifest = &installed.manifest;
        let depends_on_settings = manifest
            .prerequisites
            .iter()
            .any(|prerequisite| prerequisite.only_when.is_some());
        let (settings, settings_error) = if depends_on_settings {
            match self.read_settings(installed).await {
                Ok(settings) => (Some(settings), None),
                Err(error) => (None, Some(error)),
            }
        } else {
            (None, None)
        };
        (
            prerequisites::check(&manifest.prerequisites, settings.as_ref(), &child_path()),
            settings_error,
        )
    }

    async fn read_settings(&self, installed: &Installed) -> Result<Value, String> {
        let action = installed
            .manifest
            .action(GET_SETTINGS_ACTION)
            .expect("a manifest whose prerequisites depend on settings has get-settings");
        let finished = tokio::time::timeout(
            SETTINGS_TIMEOUT,
            self.run_command(&installed.folder, &action.command, Vec::new()),
        )
        .await
        .map_err(|_| {
            format!(
                "{GET_SETTINGS_ACTION} did not finish within {} seconds",
                SETTINGS_TIMEOUT.as_secs()
            )
        })?
        .map_err(|error| error.to_string())?;
        if !finished.status.success() {
            let stderr = String::from_utf8_lossy(&finished.stderr).trim().to_owned();
            return Err(if stderr.is_empty() {
                format!("{GET_SETTINGS_ACTION} failed with {}", finished.status)
            } else {
                stderr
            });
        }
        serde_json::from_slice::<Value>(&finished.stdout)
            .ok()
            .filter(Value::is_object)
            .ok_or_else(|| format!("{GET_SETTINGS_ACTION} did not print the settings as JSON"))
    }

    /// Runs a plugin command: `command` (a path in `folder` and its fixed arguments), then `extra`.
    async fn run_command(
        &self,
        folder: &Path,
        command: &[String],
        extra: Vec<String>,
    ) -> Result<run::Finished, PluginError> {
        let _run = self.begin_run()?;
        let mut args = command[1..].to_vec();
        args.extend(extra);
        let command = run::plugin_command(
            &folder.join(&command[0]),
            &args,
            folder,
            &self.config.state_home,
        );
        run::run_to_end(command, &self.staging(), self.quitting.subscribe()).await
    }

    async fn snapshot(&self, name: &str) -> Option<PluginSnapshot> {
        let (mut snapshot, started_pid, last_exit) = {
            let plugins = self.plugins();
            let plugin = plugins.get(name)?;
            (
                PluginSnapshot {
                    manifest: plugin.installed.manifest.clone(),
                    source: plugin.installed.source.clone(),
                    folder: plugin.installed.folder.clone(),
                    state_folder: plugin.state_dir.clone(),
                    settings_schema: plugin.installed.settings_schema.clone(),
                    prerequisites: plugin.prerequisites.clone(),
                    settings_error: plugin.settings_error.clone(),
                    process: PluginProcess::NotRunning { exit: None },
                },
                plugin.started.as_ref().map(|started| started.pid),
                plugin.last_exit.clone(),
            )
        };
        snapshot.process = match started_pid {
            Some(pid) => PluginProcess::Started { pid },
            None => match run::run_lock_holder(
                &snapshot.state_folder,
                snapshot.manifest.start_program(),
            )
            .await
            {
                Some(pid) => PluginProcess::RunningElsewhere { pid },
                None => PluginProcess::NotRunning { exit: last_exit },
            },
        };
        Some(snapshot)
    }
}

/// Owns the start command PT started until it exits or PT stops it, then records how it ended.
async fn supervise(
    host: Weak<Inner>,
    name: String,
    generation: u64,
    mut process: OwnedProcess,
    mut stop: oneshot::Receiver<()>,
    log: PathBuf,
) {
    let (status, stopped) = tokio::select! {
        status = process.wait() => (status, false),
        // A dropped sender stops the process too, so no copy outlives its record.
        _ = &mut stop => (process.stop(run::STOP_GRACE).await, true),
    };
    let exit = match status {
        // PT stopped it on purpose: on request, on quitting, or to replace it, so it did not fail.
        Ok(_) if stopped => None,
        Ok(status) => Some(PluginExit {
            code: status.code(),
            output: run::log_tail(&log, run::EXIT_LOG_LIMIT).0,
        }),
        Err(error) => Some(PluginExit {
            code: None,
            output: format!("Prompting Time could not wait for the process: {error}"),
        }),
    };
    if let Some(host) = host.upgrade() {
        host.record_exit(&name, generation, exit);
        host.notify(&name);
    }
}

pub(crate) fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The fingerprint on the last line a `--print` run printed, as `fingerprint: <hex>`.
fn printed_fingerprint(stdout: &str) -> Option<String> {
    let line = stdout.lines().rev().find(|line| !line.trim().is_empty())?;
    let fingerprint = line.trim().strip_prefix("fingerprint: ")?;
    is_sha256_hex(fingerprint).then(|| fingerprint.to_owned())
}

/// The text of the file a `--print` run saved its request in: a file inside the plugin's state
/// folder that the output names by its absolute path, as Whooper's submit prints `printed the
/// request to <path>; nothing posted`, and whose SHA-256 is the printed fingerprint. The checksum
/// means the confirmation shows exactly the request that `--expect <fingerprint>` runs. A path is
/// read up to the next whitespace, so a state folder whose path holds a space has no preview, and
/// its confirm actions cannot run.
fn saved_preview(stdout: &str, state_dir: &Path, fingerprint: &str) -> Option<String> {
    // The state folder and the printed path can name the same place differently, such as /var
    // and /private/var on macOS.
    let root = state_dir.canonicalize().ok()?;
    stdout
        .split_whitespace()
        .map(|word| word.trim_end_matches([';', ',', ':', '.']))
        .filter(|word| word.starts_with('/'))
        .find_map(|word| {
            let file = Path::new(word).canonicalize().ok()?;
            let metadata = fs::metadata(&file).ok()?;
            if !file.starts_with(&root) || !metadata.is_file() || metadata.len() > PREVIEW_LIMIT {
                return None;
            }
            let bytes = fs::read(&file).ok()?;
            if format!("{:x}", Sha256::digest(&bytes)) != fingerprint {
                return None;
            }
            String::from_utf8(bytes).ok()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fingerprint_comes_from_the_last_line_only() {
        let hex = "a".repeat(64);
        assert_eq!(
            printed_fingerprint(&format!(
                "printed the request to submit-request.json; nothing posted\nfingerprint: {hex}\n\n"
            )),
            Some(hex.clone())
        );
        assert_eq!(
            printed_fingerprint(&format!("fingerprint: {hex}\nmore output\n")),
            None
        );
        assert_eq!(
            printed_fingerprint(&format!("fingerprint: {}", "A".repeat(64))),
            None
        );
        assert_eq!(printed_fingerprint("fingerprint: abc"), None);
        assert_eq!(printed_fingerprint(""), None);
    }
    /// A state folder holding `request`, and the --print output that names its file.
    fn saved(request: &[u8]) -> (tempfile::TempDir, PathBuf, String) {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        fs::create_dir_all(state.join("runs/run-1")).unwrap();
        let file = state.join("runs/run-1/submit-request.json");
        fs::write(&file, request).unwrap();
        (root, file, format!("{:x}", Sha256::digest(request)))
    }

    #[test]
    fn a_preview_is_the_named_file_in_the_state_folder_whose_sha256_is_the_fingerprint() {
        let (root, file, fingerprint) = saved(b"request for run-1");
        let state = root.path().join("state");
        for stdout in [
            format!(
                "printed the request to {}; nothing posted\n",
                file.display()
            ),
            format!("saved {}.\nfingerprint: {fingerprint}\n", file.display()),
            format!("{},", file.display()),
        ] {
            assert_eq!(
                saved_preview(&stdout, &state, &fingerprint).as_deref(),
                Some("request for run-1"),
                "{stdout}"
            );
        }
        // A path that names the file another way, through `..`, is the same file.
        let roundabout = state.join("runs/../runs/run-1/submit-request.json");
        assert_eq!(
            saved_preview(&roundabout.display().to_string(), &state, &fingerprint).as_deref(),
            Some("request for run-1")
        );
        for stdout in [
            String::new(),
            "printed the request to runs/run-1/submit-request.json".to_owned(),
            format!("printed the request to {}x", file.display()),
            format!("printed the request to {}", state.display()),
        ] {
            assert_eq!(
                saved_preview(&stdout, &state, &fingerprint),
                None,
                "{stdout}"
            );
        }
        assert_eq!(
            saved_preview(&file.display().to_string(), &state, &"0".repeat(64)),
            None,
            "a file whose SHA-256 is not the fingerprint"
        );
        assert_eq!(
            saved_preview(
                &file.display().to_string(),
                &root.path().join("missing"),
                &fingerprint
            ),
            None,
            "a state folder that does not exist"
        );
    }

    #[test]
    fn a_preview_is_never_a_file_outside_the_state_folder_too_large_or_not_text() {
        let (root, _, fingerprint) = saved(b"request for run-1");
        let state = root.path().join("state");
        let outside = root.path().join("outside.json");
        fs::write(&outside, b"request for run-1").unwrap();
        let link = state.join("runs/run-1/link.json");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        for file in [&outside, &link] {
            assert_eq!(
                saved_preview(&file.display().to_string(), &state, &fingerprint),
                None,
                "{}",
                file.display()
            );
        }

        let large = vec![b'x'; usize::try_from(PREVIEW_LIMIT).unwrap() + 1];
        let (root, file, fingerprint) = saved(&large);
        assert_eq!(
            saved_preview(
                &file.display().to_string(),
                &root.path().join("state"),
                &fingerprint
            ),
            None,
            "larger than the preview limit"
        );
        let (root, file, fingerprint) = saved(&[0xff, 0xfe]);
        assert_eq!(
            saved_preview(
                &file.display().to_string(),
                &root.path().join("state"),
                &fingerprint
            ),
            None,
            "not UTF-8"
        );
    }
}
