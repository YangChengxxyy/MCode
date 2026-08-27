//! Shared process-containment authority for native execution builtins.
//!
//! Unix children are enrolled in a dedicated process group. Windows children
//! are enrolled in a kill-on-close Job Object before their initial thread
//! resumes. `kill_and_reap` is fallible and reports real teardown errors;
//! it does not treat an invalid Windows Job handle as evidence that members
//! exited.

// Rust guideline compliant 2026-08-27.

mod output;
#[cfg(windows)]
mod windows;

use tokio::process::Child;

pub(crate) use output::{
    CapturedStream, MAX_OUTPUT_BYTES, collect_child_output, decode_captured_text,
};

#[cfg(windows)]
pub(crate) use windows::spawn_windows_enrolled;

#[cfg(test)]
pub(crate) use output::{MAX_RETAINED_OUTPUT_BYTES, read_bounded};

/// Platform teardown state kept alive until the child is reaped.
pub(crate) struct ProcessTree {
    #[cfg(unix)]
    group: UnixProcessGroupId,
    #[cfg(windows)]
    job: windows::WindowsJob,
}

impl ProcessTree {
    /// Enrolls a Unix child whose spawn requested `process_group(0)`.
    ///
    /// # Errors
    ///
    /// Returns an error when the child has already been reaped or the
    /// resulting group id is degenerate or equals the caller's group.
    #[cfg(unix)]
    pub(crate) fn enroll_unix(child: &Child) -> std::io::Result<Self> {
        Ok(Self {
            group: UnixProcessGroupId::for_child(child)?,
        })
    }

    /// Terminate the containment boundary, then kill and reap the child.
    ///
    /// A missing Unix process group (`ESRCH`) and a leader observed as exited
    /// are successful teardown. Every Windows Job Object error is preserved:
    /// an empty Job remains a valid owned handle, so an invalid handle is not
    /// evidence that its members exited. Successful `killpg` or
    /// `TerminateJobObject` still cannot report per-member teardown, so
    /// observation after a successful containment syscall is fail-open.
    ///
    /// # Errors
    ///
    /// Returns the first real OS error from process-group `killpg` (Unix),
    /// `TerminateJobObject` (Windows), the fallback leader kill, or `wait`.
    pub(crate) async fn kill_and_reap(&self, child: &mut Child) -> std::io::Result<()> {
        #[cfg(unix)]
        let containment = ignore_missing_process_group(self.group.kill(child));

        #[cfg(windows)]
        let containment = {
            // The owned Job handle is the only authority used for descendant
            // termination. Enrollment completed before the child was resumed.
            self.job.terminate()
        };

        #[cfg(any(unix, windows))]
        let leader = if containment.is_ok() {
            reap_child(child).await
        } else {
            kill_leader_and_reap(child).await
        };

        #[cfg(not(any(unix, windows)))]
        let (containment, leader) = (Ok(()), kill_leader_and_reap(child).await);

        combine_teardown_results(containment, leader)
    }
}

#[cfg(unix)]
fn ignore_missing_process_group(result: std::io::Result<()>) -> std::io::Result<()> {
    result.or_else(|err| {
        if err.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(err)
        }
    })
}

fn combine_teardown_results(
    containment: std::io::Result<()>,
    leader: std::io::Result<()>,
) -> std::io::Result<()> {
    containment.and(leader)
}

async fn kill_leader_and_reap(child: &mut Child) -> std::io::Result<()> {
    match child.start_kill() {
        Ok(()) => reap_child(child).await,
        Err(kill_error) => match child.try_wait() {
            Ok(Some(_)) => Ok(()),
            Err(wait_error) if is_already_reaped(&wait_error) => Ok(()),
            Ok(None) | Err(_) => Err(kill_error),
        },
    }
}

fn is_already_reaped(err: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        err.raw_os_error() == Some(libc::ECHILD)
    }
    #[cfg(not(unix))]
    {
        let _ = err;
        false
    }
}

async fn reap_child(child: &mut Child) -> std::io::Result<()> {
    loop {
        match child.wait().await {
            Ok(_) => return Ok(()),
            Err(err) if is_already_reaped(&err) => return Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
}

/// A process group tied to an unreaped child leader.
#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
struct UnixProcessGroupId {
    leader_pid: u32,
    group_id: libc::pid_t,
}

#[cfg(unix)]
impl UnixProcessGroupId {
    fn for_child(child: &Child) -> std::io::Result<Self> {
        let pid = child
            .id()
            .ok_or_else(|| std::io::Error::other("child exited before process-group enrollment"))?;
        // Command::process_group(0) performs setpgid(0, 0) before exec and
        // makes spawn fail if that setup fails. Do not require the child to
        // remain in the group here: a program can deliberately escape after
        // exec, and termination-time identity checks handle that safely.
        Self::new(pid)
    }

    fn new(pid: u32) -> std::io::Result<Self> {
        if pid <= 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("refusing degenerate process-group id {pid}"),
            ));
        }
        if pid > libc::pid_t::MAX as u32 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("process-group id {pid} exceeds pid_t::MAX"),
            ));
        }

        let group_id = pid as libc::pid_t;
        // SAFETY: getpgrp has no arguments or failure value and only reads the
        // caller's process-group id.
        let own_group = unsafe { libc::getpgrp() };
        if group_id == own_group {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("refusing the caller's own process-group id {pid}"),
            ));
        }
        Ok(Self {
            leader_pid: pid,
            group_id,
        })
    }

    fn current_leader(self, current_child_id: Option<u32>) -> std::io::Result<libc::pid_t> {
        match current_child_id {
            Some(pid) if pid == self.leader_pid => Ok(self.group_id),
            Some(pid) => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "refusing process-group signal: Child::id is {pid}, saved leader is {}",
                    self.leader_pid
                ),
            )),
            None => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "refusing process-group signal after Child::id was cleared",
            )),
        }
    }

    fn validated_group(
        self,
        observed_group: libc::pid_t,
        own_group: libc::pid_t,
    ) -> std::io::Result<libc::pid_t> {
        if observed_group != self.group_id {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "refusing process-group signal: leader now belongs to {observed_group}, \
                     saved group is {}",
                    self.group_id
                ),
            ));
        }
        if self.group_id <= 1 || self.group_id == own_group {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("refusing broadcast or caller group {}", self.group_id),
            ));
        }
        Ok(self.group_id)
    }

    fn kill(self, child: &Child) -> std::io::Result<()> {
        let leader = self.current_leader(child.id())?;
        let observed_group = get_process_group(leader)?;
        // SAFETY: getpgrp has no arguments or failure value and only reads the
        // caller's current process-group id.
        let own_group = unsafe { libc::getpgrp() };
        let target = self.validated_group(observed_group, own_group)?;

        // The collection path has not waited on the child before timeout or
        // cancellation. Its live/zombie PID therefore cannot be reused between
        // this getpgid validation and killpg, so `target` still names only the
        // original group. Any validation failure skips the group signal and
        // lets the Child-handle fallback below kill only the leader.
        // SAFETY: `target` is positive, foreign, and was just observed as the
        // matching, still-reserved child leader's process group.
        if unsafe { libc::killpg(target, libc::SIGKILL) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

#[cfg(unix)]
fn get_process_group(pid: libc::pid_t) -> std::io::Result<libc::pid_t> {
    // SAFETY: `pid` was range-checked from Child::id and getpgid only observes
    // process metadata. A return value of -1 is the documented failure value.
    let group = unsafe { libc::getpgid(pid) };
    if group == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(group)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn process_group_id_rejects_broadcast_and_wrapping_values() {
        assert!(UnixProcessGroupId::new(0).is_err());
        assert!(UnixProcessGroupId::new(1).is_err());
        assert!(UnixProcessGroupId::new(u32::MAX).is_err());

        // SAFETY: getpgrp has no arguments or failure value.
        let own = unsafe { libc::getpgrp() } as u32;
        assert!(UnixProcessGroupId::new(own).is_err());
        let foreign = if own == 2 { 3 } else { 2 };
        let group = UnixProcessGroupId::new(foreign).unwrap();
        assert_eq!(group.group_id, foreign as libc::pid_t);
    }

    #[cfg(unix)]
    #[test]
    fn process_group_signal_requires_current_child_and_observed_group() {
        // SAFETY: getpgrp has no arguments or failure value.
        let own = unsafe { libc::getpgrp() };
        let foreign = if own == 2 { 3 } else { 2 };
        let group = UnixProcessGroupId::new(foreign as u32).unwrap();

        assert!(group.current_leader(None).is_err());
        assert!(group.current_leader(Some((foreign + 1) as u32)).is_err());
        assert_eq!(group.current_leader(Some(foreign as u32)).unwrap(), foreign);
        assert!(group.validated_group(foreign + 1, own).is_err());
        assert!(group.validated_group(foreign, foreign).is_err());
        assert_eq!(group.validated_group(foreign, own).unwrap(), foreign);
    }

    #[cfg(unix)]
    #[test]
    fn teardown_ignores_only_missing_process_groups_and_reaped_children() {
        let esrch = std::io::Error::from_raw_os_error(libc::ESRCH);
        assert!(ignore_missing_process_group(Err(esrch)).is_ok());
        let echild = std::io::Error::from_raw_os_error(libc::ECHILD);
        assert!(is_already_reaped(&echild));
        let eperm = std::io::Error::from_raw_os_error(libc::EPERM);
        assert!(!is_already_reaped(&eperm));
        assert!(ignore_missing_process_group(Err(eperm)).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn teardown_preserves_invalid_job_handle_error() {
        use windows_sys::Win32::Foundation::ERROR_INVALID_HANDLE;

        let invalid_handle = std::io::Error::from_raw_os_error(ERROR_INVALID_HANDLE as i32);
        let result = combine_teardown_results(Err(invalid_handle), Ok(()));
        assert_eq!(
            result.unwrap_err().raw_os_error(),
            Some(ERROR_INVALID_HANDLE as i32)
        );
    }
}
