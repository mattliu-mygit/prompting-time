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
    cleanup_deadline: Option<tokio::time::Instant>,
    #[cfg(all(test, target_os = "macos"))]
    signal_permission_failures: usize,
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
            cleanup_deadline: None,
            #[cfg(all(test, target_os = "macos"))]
            signal_permission_failures: 0,
        })
    }

    pub(crate) fn id(&self) -> Option<u32> {
        self.child.id()
    }

    // A running leader returns None immediately. An observed exit awaits bounded group
    // cleanup, so callers never mistake pending cleanup for a still-running process.
    pub(crate) async fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(group) = self.group {
            match waitid(
                WaitId::Pid(group),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            ) {
                Ok(None) => return Ok(None),
                Ok(Some(_)) => {
                    self.cleanup_group().await?;
                }
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
            if let Some(status) = self.try_wait().await? {
                return Ok(status);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub(crate) async fn terminate(&mut self) -> io::Result<ExitStatus> {
        let deadline = self.cleanup_group().await?;
        tokio::time::timeout_at(deadline, self.child.wait())
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "owned process reaping timed out")
            })?
    }

    async fn cleanup_group(&mut self) -> io::Result<tokio::time::Instant> {
        // Store the deadline on the owner: select! may cancel and recreate a natural
        // wait, or switch to termination. Neither should restart the cleanup allowance.
        let deadline = *self
            .cleanup_deadline
            .get_or_insert_with(|| tokio::time::Instant::now() + Duration::from_secs(2));
        loop {
            match self.signal_group() {
                // Darwin can exclude an exiting leader from killpg before waitid reports it
                // waitable, and additional zombies can briefly remain after natural exit.
                // Allow either transition to settle; persistent errors remain errors.
                #[cfg(target_os = "macos")]
                Err(error)
                    if error.kind() == io::ErrorKind::PermissionDenied
                        && tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                result => {
                    result?;
                    return Ok(deadline);
                }
            }
        }
    }

    fn signal_group(&mut self) -> io::Result<()> {
        // Simulate the guarded OS signaling boundary: Darwin's zombie-group lifetime
        // depends on its external reaper and cannot be scheduled deterministically.
        #[cfg(all(test, target_os = "macos"))]
        if self.group.is_some() && self.signal_permission_failures > 0 {
            self.signal_permission_failures -= 1;
            return Err(rustix::io::Errno::PERM.into());
        }
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

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    async fn exited_process() -> OwnedProcess {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 0"]);
        let process = OwnedProcess::spawn(&mut command).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if waitid(
                    WaitId::Pid(process.group.unwrap()),
                    WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
                )
                .unwrap()
                .is_some()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("finite child should exit");
        process
    }

    #[tokio::test]
    async fn natural_exit_retries_transient_group_permission_failures() {
        let mut process = exited_process().await;
        process.signal_permission_failures = 3;
        let status = tokio::time::timeout(Duration::from_secs(3), process.try_wait())
            .await
            .expect("cleanup must be bounded")
            .expect("transient zombie-group cleanup must preserve successful exit")
            .expect("an observed exit must not become a still-running result");
        assert!(status.success());
        assert!(process.group.is_none());
        assert!(process.id().is_none(), "leader must be reaped");
    }

    #[tokio::test]
    async fn natural_exit_with_exiting_background_child_succeeds() {
        for _ in 0..32 {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", "(exit 0) & exit 0"]);
            let mut process = OwnedProcess::spawn(&mut command).unwrap();
            let status = tokio::time::timeout(Duration::from_secs(3), process.wait())
                .await
                .expect("cleanup must be bounded")
                .expect("exiting background child must not fail a successful command");
            assert!(status.success());
            assert!(process.id().is_none());
        }
    }

    #[tokio::test]
    async fn natural_exit_reports_persistent_group_permission_failure() {
        let mut process = exited_process().await;
        process.signal_permission_failures = usize::MAX;
        let started = tokio::time::Instant::now();
        let error = tokio::time::timeout(Duration::from_secs(3), process.wait())
            .await
            .expect("persistent failure must be bounded")
            .expect_err("permission failure must remain observable");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(started.elapsed() >= Duration::from_secs(2));
        assert!(
            process.group.is_some(),
            "retain the reserved group on failure"
        );
        assert!(
            process.id().is_some(),
            "do not reap before cleanup succeeds"
        );
        process.signal_permission_failures = 0;
        assert!(process.terminate().await.unwrap().success());
    }

    #[tokio::test]
    async fn canceled_natural_exit_keeps_cleanup_deadline_for_termination() {
        let mut process = exited_process().await;
        process.signal_permission_failures = usize::MAX;
        let started = tokio::time::Instant::now();
        assert!(
            tokio::time::timeout(Duration::from_millis(500), process.wait())
                .await
                .is_err(),
            "natural exit should keep retrying until cancellation"
        );
        let error =
            tokio::time::timeout_at(started + Duration::from_millis(2300), process.terminate())
                .await
                .expect("termination must share the original cleanup deadline")
                .expect_err("persistent permission failure must remain observable");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        process.signal_permission_failures = 0;
        assert!(process.terminate().await.unwrap().success());
    }
}
