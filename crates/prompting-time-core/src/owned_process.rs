use std::{io, process::ExitStatus, time::Duration};

use rustix::process::{Pid, Signal, WaitId, WaitIdOptions, kill_process_group, waitid};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

/// Owns one isolated process group, exposing only its pipes to callers.
/// Observing exit with WNOWAIT keeps the leader's PID reserved until the group is signaled,
/// including when descendants outlive the leader. No group ID is used after reaping.
pub(crate) struct OwnedProcess {
    child: Child,
    pub(crate) stdin: Option<ChildStdin>,
    pub(crate) stdout: Option<ChildStdout>,
    pub(crate) stderr: Option<ChildStderr>,
    group: Option<Pid>,
}

impl OwnedProcess {
    pub(crate) fn spawn(command: &mut Command) -> io::Result<Self> {
        command.process_group(0).kill_on_drop(true);
        let mut child = command.spawn()?;
        let group = child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .filter(|id| *id > 1)
            .and_then(Pid::from_raw)
            .ok_or_else(|| io::Error::other("missing owned process group"))?;
        Ok(Self {
            stdin: child.stdin.take(),
            stdout: child.stdout.take(),
            stderr: child.stderr.take(),
            child,
            group: Some(group),
        })
    }

    pub(crate) fn id(&self) -> Option<u32> {
        self.child.id()
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(group) = self.group {
            match waitid(
                WaitId::Pid(group),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            ) {
                Ok(None) => return Ok(None),
                Ok(Some(_)) => self.signal_group()?,
                Err(rustix::io::Errno::INTR) => return Ok(None),
                Err(error) => {
                    // If another reaper consumed our child, the saved ID is no longer safe.
                    if error == rustix::io::Errno::CHILD {
                        self.group = None;
                    }
                    return Err(error.into());
                }
            }
        }
        self.child.try_wait()
    }

    pub(crate) async fn wait(&mut self) -> io::Result<ExitStatus> {
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(status);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub(crate) async fn terminate(&mut self) -> io::Result<ExitStatus> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            match self.signal_group() {
                // Darwin can exclude an exiting leader from killpg before waitid reports it
                // waitable. Allow that transition to settle; persistent errors remain errors.
                #[cfg(target_os = "macos")]
                Err(error)
                    if error.kind() == io::ErrorKind::PermissionDenied
                        && tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                result => {
                    result?;
                    break;
                }
            }
        }
        tokio::time::timeout_at(deadline, self.child.wait())
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "owned process reaping timed out")
            })?
    }

    fn signal_group(&mut self) -> io::Result<()> {
        if let Some(group) = self.group {
            match kill_process_group(group, Signal::KILL) {
                Ok(()) | Err(rustix::io::Errno::SRCH) => self.group = None,
                #[cfg(target_os = "macos")]
                Err(rustix::io::Errno::PERM) if only_zombie_leader_remains(group) => {
                    self.group = None;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn only_zombie_leader_remains(group: Pid) -> bool {
    // Darwin's killpg skips zombies and returns EPERM for a zombie-only group. Do not
    // suppress permission failures unless the complete group is our reserved, exited leader.
    let observed = waitid(
        WaitId::Pid(group),
        WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
    );
    if !matches!(observed, Ok(Some(_))) {
        return false;
    }
    unsafe extern "C" {
        fn proc_listpids(kind: u32, group: u32, buffer: *mut std::ffi::c_void, size: i32) -> i32;
    }
    const PROC_PGRP_ONLY: u32 = 2;
    // Two slots distinguish exactly one member from a truncated or larger group. Query
    // only our group; any other member or API failure preserves the original EPERM.
    let mut members = [0_i32; 2];
    // SAFETY: libproc receives a valid writable buffer with its exact byte size.
    let bytes = unsafe {
        proc_listpids(
            PROC_PGRP_ONLY,
            group.as_raw_nonzero().get() as u32,
            members.as_mut_ptr().cast(),
            std::mem::size_of_val(&members) as i32,
        )
    };
    bytes == std::mem::size_of::<i32>() as i32 && members[0] == group.as_raw_nonzero().get()
}

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        // Cancellation cannot await or report errors. Signal before Child's drop can reap the
        // leader; kill_on_drop provides Tokio's eventual direct-child reaping fallback.
        let _ = self.signal_group();
    }
}
