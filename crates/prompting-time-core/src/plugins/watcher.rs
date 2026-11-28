//! Watching a plugin's state folder, so PT can tell the UI which of the plugin's files changed.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
/// The most paths waiting for the forwarding task. A burst past it, such as a plugin rewriting
/// thousands of files at once, goes out as "anything may have changed" instead of using more memory.
const QUEUE_CAPACITY: usize = 1024;

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
        let (queue, received) = Queue::new(QUEUE_CAPACITY);
        // notify calls this on its own thread, so it only hands paths over. None stands for "anything
        // may have changed": a watch error, or FSEvents dropping events and asking for a rescan.
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
                Ok(event) if event.need_rescan() => queue.push(None),
                Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                    for path in event.paths {
                        queue.push(Some(path));
                    }
                }
                Ok(_) => {}
                Err(_) => queue.push(None),
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

/// The watch's side of a bounded queue of changed paths. A path that does not fit marks the queue
/// overflowed, which the forwarding task reads as "anything may have changed".
struct Queue {
    paths: mpsc::Sender<Option<PathBuf>>,
    overflowed: Arc<AtomicBool>,
}

/// The forwarding task's side of the queue.
struct Received {
    paths: mpsc::Receiver<Option<PathBuf>>,
    overflowed: Arc<AtomicBool>,
}

impl Queue {
    fn new(capacity: usize) -> (Self, Received) {
        let (paths, received) = mpsc::channel(capacity);
        let overflowed = Arc::new(AtomicBool::new(false));
        (
            Self {
                paths,
                overflowed: Arc::clone(&overflowed),
            },
            Received {
                paths: received,
                overflowed,
            },
        )
    }

    fn push(&self, path: Option<PathBuf>) {
        // A full queue holds paths, so the forwarding task wakes and reads the mark.
        if self.paths.try_send(path).is_err() {
            self.overflowed.store(true, Ordering::Release);
        }
    }
}

impl Received {
    /// The next path, or None for "anything may have changed"; the outer None once the watch ends.
    async fn recv(&mut self) -> Option<Option<PathBuf>> {
        let path = self.paths.recv().await?;
        Some(if self.overflowed.swap(false, Ordering::AcqRel) {
            None
        } else {
            path
        })
    }
}

async fn forward(
    plugin: String,
    root: PathBuf,
    mut paths: Received,
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
    if *anything {
        return;
    }
    let Some(path) = path else {
        *anything = true;
        files.clear();
        return;
    };
    if let Ok(relative) = path.strip_prefix(root)
        && !relative.as_os_str().is_empty()
    {
        files.insert(relative.to_string_lossy().into_owned());
        // Past the most one event lists, the event says anything changed, so stop keeping names.
        if files.len() > MAX_CHANGED_FILES {
            *anything = true;
            files.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn forwarded(push: impl FnOnce(&Queue), capacity: usize) -> PluginChange {
        let root = PathBuf::from("/state/fake-agent");
        let (queue, received) = Queue::new(capacity);
        let (changes, mut heard) = broadcast::channel(4);
        push(&queue);
        drop(queue);
        forward("fake-agent".to_owned(), root, received, changes).await;
        heard.try_recv().unwrap()
    }

    #[tokio::test]
    async fn paths_that_fit_the_queue_go_out_by_name() {
        let change = forwarded(
            |queue| {
                queue.push(Some(PathBuf::from("/state/fake-agent/runs/a.json")));
                queue.push(Some(PathBuf::from("/state/fake-agent/runs/a.json")));
                queue.push(Some(PathBuf::from("/state/fake-agent/status.json")));
            },
            3,
        )
        .await;
        assert_eq!(
            change.files,
            Some(vec!["runs/a.json".to_owned(), "status.json".to_owned()])
        );
    }

    #[tokio::test]
    async fn a_path_that_does_not_fit_the_queue_means_anything_may_have_changed() {
        let change = forwarded(
            |queue| {
                for index in 0..3 {
                    queue.push(Some(PathBuf::from(format!("/state/fake-agent/{index}"))));
                }
            },
            2,
        )
        .await;
        assert_eq!(change.files, None);
    }

    #[tokio::test]
    async fn more_files_than_one_event_lists_mean_anything_may_have_changed() {
        let change = forwarded(
            |queue| {
                for index in 0..=MAX_CHANGED_FILES {
                    queue.push(Some(PathBuf::from(format!("/state/fake-agent/{index}"))));
                }
            },
            MAX_CHANGED_FILES + 1,
        )
        .await;
        assert_eq!(change.files, None);
    }
}
