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
/// The most of a command's stdout PT keeps, from its start, where JSON results begin. A page's
/// `call` gets this output back whole or not at all, and the window holds several copies of it at
/// once (the bytes, the IPC message, the JavaScript string, the parsed value, and the copy posted
/// to the page), so this bounds what one action costs the window: in Node, 8 MiB of JSON taking
/// those steps briefly used about 50 MiB of heap and 76 ms. Raise it if a plugin's page needs a
/// larger reply; a page that needs much more should read its data in parts.
pub const ACTION_OUTPUT_LIMIT: u32 = 8 * 1024 * 1024;
/// The most of a command's stderr PT keeps, from its end, which holds the reason it failed.
const STDERR_LIMIT: u64 = 1024 * 1024;
/// The end of the start command's log PT keeps when the command exits.
pub(crate) const EXIT_LOG_LIMIT: u64 = 4 * 1024;

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
    /// The start of what the command printed, at most `ACTION_OUTPUT_LIMIT` bytes.
    pub(crate) stdout: Vec<u8>,
    /// Whether the command printed more than `stdout` holds.
    pub(crate) stdout_truncated: bool,
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
    let ((stdout, stdout_truncated), stderr) = tokio::task::spawn_blocking(move || {
        Ok::<_, io::Error>((
            read_head(&mut stdout, u64::from(ACTION_OUTPUT_LIMIT))?,
            read_tail(&mut stderr, STDERR_LIMIT)?,
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
        stdout_truncated,
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

/// The last `limit` bytes of the start command's log, from its first whole line, and whether the
/// file holds more. Empty when the file cannot be read, such as before the first start.
pub(crate) fn log_tail(log: &Path, limit: u64) -> (String, bool) {
    let Ok(mut file) = File::open(log) else {
        return (String::new(), false);
    };
    let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let Ok(tail) = read_tail(&mut file, limit) else {
        return (String::new(), false);
    };
    let truncated = length > limit;
    let text = String::from_utf8_lossy(&tail);
    let whole_lines = match text.split_once('\n') {
        Some((_, rest)) if truncated => rest,
        _ => &text,
    };
    (whole_lines.trim().to_owned(), truncated)
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
    runs_program(
        String::from_utf8_lossy(&output.stdout).trim(),
        start_program,
    )
    .then_some(pid)
}

/// Whether `command`, a command line as `ps` prints it, runs `program`: whether it holds `program`
/// as a whole word or as the last component of a path, such as `/bin/sh /plugins/x/bin/whooper-run`
/// for a script whose interpreter is argv0. `ps` joins the arguments with spaces, so a path that
/// holds a space cannot be told from two words; this matches it anyway, and also matches a command
/// that only names the program's path as an argument, such as an editor open on it.
fn runs_program(command: &str, program: &str) -> bool {
    command.match_indices(program).any(|(start, _)| {
        let before = command[..start].chars().next_back();
        let after = command[start + program.len()..].chars().next();
        matches!(before, None | Some('/' | ' ')) && matches!(after, None | Some(' '))
    })
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

/// The first `limit` bytes of `file`, and whether it holds more.
fn read_head(file: &mut File, limit: u64) -> io::Result<(Vec<u8>, bool)> {
    let length = file.metadata()?.len();
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes)?;
    Ok((bytes, length > limit))
}

fn read_tail(file: &mut File, limit: u64) -> io::Result<Vec<u8>> {
    let length = file.metadata()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(limit)))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Child, Command as StdCommand};

    use super::*;

    struct Killed(Child);

    impl Drop for Killed {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn lock(state_dir: &Path, pid: u32) {
        fs::create_dir_all(state_dir.join("run.lock")).unwrap();
        fs::write(state_dir.join("run.lock/pid"), pid.to_string()).unwrap();
    }

    #[test]
    fn a_command_line_runs_a_program_only_as_a_whole_path_component() {
        for (command, runs) in [
            ("/bin/sh /plugins/fake-agent/bin/fake-run", true),
            (
                "/bin/bash -e /Library/Application Support/plugins/x/bin/fake-run --once",
                true,
            ),
            ("/plugins/fake-agent/bin/fake-run", true),
            ("fake-run", true),
            ("/bin/sh fake-run", true),
            ("/bin/sh /plugins/fake-agent/bin/fake-runner", false),
            ("/bin/sh -c sleep 30 fake-run-helper", false),
            ("/usr/bin/vim notes-fake-run", false),
            ("/usr/bin/python3 -m fake_run", false),
            ("/bin/sleep 30 --fake-run", false),
            ("", false),
        ] {
            assert_eq!(runs_program(command, "fake-run"), runs, "{command:?}");
        }
    }

    #[tokio::test]
    async fn the_run_lock_holder_is_a_live_process_running_the_start_program() {
        let root = tempfile::tempdir().unwrap();
        let state_dir = root.path().join("state");
        assert_eq!(
            run_lock_holder(&state_dir, "fake-run").await,
            None,
            "no lock"
        );

        // A script under a folder whose name holds a space, as Application Support's does.
        let script = root.path().join("Application Support/bin/fake-run");
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::write(&script, "#!/bin/sh\nsleep 30\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let holder = Killed(StdCommand::new(&script).spawn().unwrap());
        lock(&state_dir, holder.0.id());
        assert_eq!(
            run_lock_holder(&state_dir, "fake-run").await,
            Some(holder.0.id())
        );
        assert_eq!(run_lock_holder(&state_dir, "other-run").await, None);

        // A reused process ID whose command line merely contains the program's name.
        let reused = Killed(
            StdCommand::new("/bin/sh")
                .args(["-c", "sleep 30", "fake-run-helper"])
                .spawn()
                .unwrap(),
        );
        lock(&state_dir, reused.0.id());
        assert_eq!(run_lock_holder(&state_dir, "fake-run").await, None);

        for pid in ["1", "0", "not a pid"] {
            fs::write(state_dir.join("run.lock/pid"), pid).unwrap();
            assert_eq!(run_lock_holder(&state_dir, "launchd").await, None, "{pid}");
        }
    }
}
