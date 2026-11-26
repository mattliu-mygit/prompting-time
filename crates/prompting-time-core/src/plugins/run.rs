//! Running a plugin's programs: commands PT runs to their end and reads, the start command PT keeps
//! running, and the check for a copy that already holds the plugin's run lock.

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::watch;

use super::PluginError;
use crate::owned_process::OwnedProcess;
use crate::providers::child_path;

/// How long a plugin's process group gets to clean up after SIGTERM before PT kills it, when PT
/// quits or replaces the plugin. A review's teardown that takes longer is cut short; raise this if
/// plugins report half-finished cleanups.
pub(crate) const STOP_GRACE: Duration = Duration::from_secs(5);
/// The most of each output stream PT keeps: the start of stdout, which holds JSON results, and the
/// end of stderr, which holds the reason a command failed.
const OUTPUT_LIMIT: u64 = 1024 * 1024;
/// The end of the start command's log PT shows when the command exits.
const LOG_TAIL_LIMIT: u64 = 4 * 1024;

/// `program` with `args`, run in the plugin's folder, with the PATH PT gives every CLI it starts and
/// the XDG state home PT watches, so the plugin and PT agree on its state folder.
pub(crate) fn plugin_command(
    program: &Path,
    args: &[String],
    folder: &Path,
    state_home: &Path,
) -> Command {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(folder)
        .env("PATH", child_path())
        .env("XDG_STATE_HOME", state_home);
    command
}

pub(crate) struct Finished {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

/// Runs `command` in its own process group until it exits, or until PT starts quitting, which stops
/// the group. Its output goes to unlinked files in `scratch` rather than pipes, so a large output
/// never blocks it, and a descendant that keeps a stream open cannot hold up the result.
pub(crate) async fn run_to_end(
    mut command: Command,
    scratch: &Path,
    mut quitting: watch::Receiver<bool>,
) -> Result<Finished, PluginError> {
    let program = Path::new(command.as_std().get_program()).to_path_buf();
    let context = |what: &str| format!("{what} {}", program.display());
    let io_error = |what: &str| {
        let context = context(what);
        move |source| PluginError::Io { context, source }
    };
    let mut stdout = scratch_file(scratch).map_err(io_error("could not capture the output of"))?;
    let mut stderr = scratch_file(scratch).map_err(io_error("could not capture the output of"))?;
    command
        .stdin(Stdio::null())
        .stdout(
            stdout
                .try_clone()
                .map_err(io_error("could not capture the output of"))?,
        )
        .stderr(
            stderr
                .try_clone()
                .map_err(io_error("could not capture the output of"))?,
        );
    let mut process = OwnedProcess::spawn(&mut command).map_err(io_error("could not start"))?;
    drop(command);
    let status = tokio::select! {
        status = process.wait() => status.map_err(io_error("could not wait for"))?,
        // The watch's guard is dropped inside, so it is never held across the stop below.
        () = async { let _ = quitting.wait_for(|quitting| *quitting).await; } => {
            process.stop(STOP_GRACE).await.map_err(io_error("could not stop"))?;
            return Err(PluginError::Quitting);
        }
    };
    let (stdout, stderr) = tokio::task::spawn_blocking(move || {
        Ok::<_, io::Error>((
            read_head(&mut stdout, OUTPUT_LIMIT)?,
            read_tail(&mut stderr, OUTPUT_LIMIT)?,
        ))
    })
    .await
    .map_err(|_| PluginError::Io {
        context: context("could not read the output of"),
        source: io::Error::other("the reading task stopped"),
    })?
    .map_err(io_error("could not read the output of"))?;
    Ok(Finished {
        status,
        stdout,
        stderr,
    })
}

/// Starts the start command in its own process group, writing its output to `log`, which each start
/// empties.
pub(crate) fn spawn_start(mut command: Command, log: &Path) -> io::Result<OwnedProcess> {
    let file = File::create(log)?;
    command
        .stdin(Stdio::null())
        .stdout(file.try_clone()?)
        .stderr(file);
    OwnedProcess::spawn(&mut command)
}

/// The end of the start command's log, from its first whole line.
pub(crate) fn log_tail(log: &Path) -> String {
    let Ok(mut file) = File::open(log) else {
        return String::new();
    };
    let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let Ok(tail) = read_tail(&mut file, LOG_TAIL_LIMIT) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&tail);
    let whole_lines = match text.split_once('\n') {
        Some((_, rest)) if length > LOG_TAIL_LIMIT => rest,
        _ => &text,
    };
    whole_lines.trim().to_owned()
}

/// The process holding `<state folder>/run.lock/pid`, the lock a running start command holds,
/// when that process is alive and still runs `start_program`: Whooper's whooper-run applies the
/// same rule to a lock it finds, since a lock outlives a `kill -9`, and its process ID can later
/// belong to any program.
pub(crate) async fn run_lock_holder(state_dir: &Path, start_program: &str) -> Option<u32> {
    let pid: u32 = tokio::fs::read_to_string(state_dir.join("run.lock").join("pid"))
        .await
        .ok()?
        .trim()
        .parse()
        .ok()?;
    if pid <= 1 {
        return None;
    }
    let output = Command::new("/bin/ps")
        .args([OsStr::new("-o"), OsStr::new("command="), OsStr::new("-p")])
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .contains(start_program)
        .then_some(pid)
}

fn scratch_file(scratch: &Path) -> io::Result<File> {
    let path = scratch.join(format!("output-{}", uuid::Uuid::now_v7()));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    // The open file stays usable without its name, so nothing is left to clean up.
    fs::remove_file(&path)?;
    Ok(file)
}

fn read_head(file: &mut File, limit: u64) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn read_tail(file: &mut File, limit: u64) -> io::Result<Vec<u8>> {
    let length = file.metadata()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(limit)))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}
