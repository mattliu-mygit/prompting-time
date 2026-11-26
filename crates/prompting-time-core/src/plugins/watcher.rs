//! Watching a plugin's state folder, so PT can tell the UI which of the plugin's files changed.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use super::PluginChange;

/// Changes that arrive within this long of the first one go out as one event. A review appends to
/// its transcript many times a second, and the UI needs to hear about that at most a few times.
const DEBOUNCE: Duration = Duration::from_millis(250);
/// The most files one event lists; past it, the event says that anything may have changed.
const MAX_CHANGED_FILES: usize = 100;

/// Watches one state folder while it lives. Dropping it stops the watch.
pub(crate) struct StateWatcher {
    _watcher: notify::RecommendedWatcher,
    forward: JoinHandle<()>,
}

impl StateWatcher {
    /// Starts watching `state_dir`, creating it when the plugin has not yet, and sends one
    /// `PluginChange` per burst of changes.
    pub(crate) fn start(
        plugin: String,
        state_dir: &Path,
        changes: broadcast::Sender<PluginChange>,
    ) -> notify::Result<Self> {
        fs::create_dir_all(state_dir)?;
        // FSEvents reports resolved paths, such as /private/var for /var.
        let root = state_dir.canonicalize()?;
        let (paths, received) = mpsc::unbounded_channel();
        // notify calls this on its own thread, so it only hands paths over. None stands for "anything
        // may have changed": a watch error, or FSEvents dropping events and asking for a rescan.
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
                Ok(event) if event.need_rescan() => {
                    let _ = paths.send(None);
                }
                Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                    for path in event.paths {
                        let _ = paths.send(Some(path));
                    }
                }
                Ok(_) => {}
                Err(_) => {
                    let _ = paths.send(None);
                }
            })?;
        watcher.watch(&root, RecursiveMode::Recursive)?;
        let forward = tokio::spawn(forward(plugin, root, received, changes));
        Ok(Self {
            _watcher: watcher,
            forward,
        })
    }
}

impl Drop for StateWatcher {
    fn drop(&mut self) {
        self.forward.abort();
    }
}

async fn forward(
    plugin: String,
    root: PathBuf,
    mut paths: mpsc::UnboundedReceiver<Option<PathBuf>>,
    changes: broadcast::Sender<PluginChange>,
) {
    while let Some(first) = paths.recv().await {
        let mut files = BTreeSet::new();
        let mut anything = false;
        collect(&root, first, &mut files, &mut anything);
        let deadline = tokio::time::Instant::now() + DEBOUNCE;
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => break,
                next = paths.recv() => match next {
                    Some(path) => collect(&root, path, &mut files, &mut anything),
                    None => break,
                },
            }
        }
        if files.is_empty() && !anything {
            continue;
        }
        let files =
            (!anything && files.len() <= MAX_CHANGED_FILES).then(|| files.into_iter().collect());
        let _ = changes.send(PluginChange {
            plugin: plugin.clone(),
            files,
        });
    }
}

fn collect(root: &Path, path: Option<PathBuf>, files: &mut BTreeSet<String>, anything: &mut bool) {
    let Some(path) = path else {
        *anything = true;
        return;
    };
    if let Ok(relative) = path.strip_prefix(root)
        && !relative.as_os_str().is_empty()
    {
        files.insert(relative.to_string_lossy().into_owned());
    }
}
