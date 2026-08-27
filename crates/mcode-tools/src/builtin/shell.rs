//! Platform-shell selection and process construction for the public `bash` tool.
//!
//! The public tool name is intentionally stable. This module chooses the
//! native execution backend and owns the platform-specific process-containment
//! lifecycle without leaking it into the tool API.

// Rust guideline compliant 2026-08-27.

use std::path::Path;
use std::process::Stdio;

#[cfg(any(windows, test))]
use base64::Engine as _;
#[cfg(any(windows, test))]
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use tokio::process::{Child, Command};

use crate::tool::ToolError;

/// Maximum `CreateProcessW` command-line length, including its terminator.
#[cfg(any(windows, test))]
const WINDOWS_COMMAND_LINE_LIMIT_UTF16_UNITS: usize = 32_767;

/// PowerShell arguments placed before the directly encoded user script.
#[cfg(any(windows, test))]
const POWERSHELL_ARGUMENTS: &[&str] = &[
    "-NoLogo",
    "-NoProfile",
    "-NonInteractive",
    "-ExecutionPolicy",
    "Bypass",
    "-EncodedCommand",
];

#[cfg(windows)]
const WINDOWS_SHELL_EXECUTABLE: &str = "pwsh.exe";

#[cfg(not(windows))]
#[derive(Debug, Clone, Copy)]
struct ShellCandidate {
    executable: &'static str,
}

#[cfg(not(windows))]
const SHELL_CANDIDATES: &[ShellCandidate] = &[
    ShellCandidate {
        executable: "/bin/bash",
    },
    ShellCandidate { executable: "bash" },
    ShellCandidate { executable: "sh" },
];

/// Returns the identifier used before shell selection finishes.
pub(crate) fn preferred_identifier() -> &'static str {
    #[cfg(windows)]
    return WINDOWS_SHELL_EXECUTABLE;
    #[cfg(not(windows))]
    SHELL_CANDIDATES[0].executable
}

/// A spawned native shell and its process-containment ownership.
pub(crate) struct SpawnedShell {
    pub(crate) child: Child,
    pub(crate) identifier: &'static str,
    pub(crate) process_tree: ProcessTree,
}

/// Platform teardown state kept alive until the shell is reaped.
pub(crate) struct ProcessTree {
    #[cfg(unix)]
    group: UnixProcessGroupId,
    #[cfg(windows)]
    job: WindowsJob,
}

impl ProcessTree {
    /// Terminate the containment boundary, then kill and reap the shell.
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
            // termination. Enrollment completed before the shell was resumed.
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

/// Spawn PowerShell 7 from `PATH` or MCode's verified managed cache.
///
/// # Errors
///
/// Returns an error if `pwsh.exe` cannot be spawned, secure provisioning fails,
/// the command line is too long, or process containment cannot be established.
#[cfg(windows)]
pub(crate) async fn spawn(command: &str, cwd: &Path) -> Result<SpawnedShell, ToolError> {
    require_session_cwd(cwd)?;

    let path_candidate = Path::new(WINDOWS_SHELL_EXECUTABLE);
    let encoded_command = encode_powershell_command(command, path_candidate)?;
    match spawn_windows_candidate(path_candidate, &encoded_command, cwd) {
        Ok((child, job)) => Ok(SpawnedShell {
            child,
            identifier: WINDOWS_SHELL_EXECUTABLE,
            process_tree: ProcessTree { job },
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let managed = crate::builtin::powershell::ensure_pwsh().await?;
            let managed_command = encode_powershell_command(command, &managed)?;
            let (child, job) = spawn_windows_candidate(&managed, &managed_command, cwd).map_err(
                |managed_error| {
                    ToolError::Execution(format!(
                        "failed to spawn managed PowerShell 7 from the managed pwsh cache: {managed_error}"
                    ))
                },
            )?;
            Ok(SpawnedShell {
                child,
                identifier: WINDOWS_SHELL_EXECUTABLE,
                process_tree: ProcessTree { job },
            })
        }
        Err(err) => Err(ToolError::Execution(format!(
            "failed to spawn PowerShell 7 ({WINDOWS_SHELL_EXECUTABLE}): {err}"
        ))),
    }
}

/// Spawn the first available POSIX shell in platform preference order.
///
/// # Errors
///
/// Returns an error when no candidate can be spawned or process-group
/// containment cannot be established.
#[cfg(not(windows))]
pub(crate) async fn spawn(command: &str, cwd: &Path) -> Result<SpawnedShell, ToolError> {
    require_session_cwd(cwd)?;
    let mut failures = Vec::with_capacity(SHELL_CANDIDATES.len());
    for candidate in SHELL_CANDIDATES {
        #[cfg(unix)]
        let spawned = spawn_posix_candidate(candidate.executable, command, cwd).and_then(|child| {
            let group = UnixProcessGroupId::for_child(&child)?;
            Ok((child, ProcessTree { group }))
        });
        #[cfg(not(unix))]
        let spawned = spawn_posix_candidate(candidate.executable, command, cwd)
            .map(|child| (child, ProcessTree {}));

        match spawned {
            Ok((child, process_tree)) => {
                return Ok(SpawnedShell {
                    child,
                    identifier: candidate.executable,
                    process_tree,
                });
            }
            Err(err) => failures.push(format!("{}: {err}", candidate.executable)),
        }
    }

    Err(ToolError::Execution(format!(
        "failed to spawn a platform shell (tried {}): {}",
        SHELL_CANDIDATES
            .iter()
            .map(|candidate| candidate.executable)
            .collect::<Vec<_>>()
            .join(", "),
        failures.join("; ")
    )))
}

#[cfg(not(windows))]
fn spawn_posix_candidate(executable: &str, command: &str, cwd: &Path) -> std::io::Result<Child> {
    let mut process = Command::new(executable);
    process.arg("-c").arg(command);
    configure_common(&mut process, cwd);

    // The leader pid becomes a scoped group id validated immediately after
    // spawn, before it can ever reach killpg.
    #[cfg(unix)]
    process.process_group(0);

    process.spawn()
}

#[cfg(windows)]
fn spawn_windows_candidate(
    executable: &Path,
    encoded_command: &str,
    cwd: &Path,
) -> std::io::Result<(Child, WindowsJob)> {
    let parent_job = current_process_is_in_job()
        .map_err(|err| windows_spawn_error("failed to query the host's Job membership", err))?;
    let job = WindowsJob::new()
        .map_err(|err| windows_spawn_error("failed to create the shell Job Object", err))?;

    // Windows 8+ supports nested Jobs. First inherit any host Job, then add the
    // still-suspended child to our dedicated Job. This keeps ordinary CI host
    // limits while adding a narrower descendant-containment boundary.
    let mut child = build_windows_command(executable, encoded_command, cwd, false).spawn()?;
    let enrollment = job.assign(&child);
    if let Err(nested_error) = enrollment {
        if !parent_job {
            let err = windows_spawn_error(
                "failed to enroll the suspended shell in its dedicated Job Object",
                nested_error,
            );
            let _ = child.start_kill();
            return Err(err);
        }

        // Older or specially configured parent Jobs can reject nesting. The
        // first child is still suspended and is terminated through its owned
        // process handle before the explicit-breakaway fallback is attempted.
        let _ = child.start_kill();
        child = build_windows_command(executable, encoded_command, cwd, true)
            .spawn()
            .map_err(|breakaway_error| {
                let kind = breakaway_error.kind();
                std::io::Error::new(
                    kind,
                    format!(
                        "parent Job rejected nested dedicated Job enrollment ({nested_error}); \
                         CREATE_BREAKAWAY_FROM_JOB fallback failed ({breakaway_error}); refusing \
                         to run without enforceable descendant containment"
                    ),
                )
            })?;
        if let Err(assign_error) = job.assign(&child) {
            let err = windows_spawn_error(
                "CREATE_BREAKAWAY_FROM_JOB succeeded but dedicated Job enrollment failed",
                assign_error,
            );
            let _ = child.start_kill();
            return Err(err);
        }
    }

    if let Err(err) = resume_windows_child(&child) {
        let err = windows_spawn_error("failed to resume the enrolled shell", err);
        let _ = job.terminate();
        let _ = child.start_kill();
        return Err(err);
    }

    Ok((child, job))
}

#[cfg(windows)]
fn build_windows_command(
    executable: &Path,
    encoded_command: &str,
    cwd: &Path,
    breakaway: bool,
) -> Command {
    use windows_sys::Win32::System::Threading::{
        CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_SUSPENDED,
    };

    debug_assert!(
        powershell_command_line_units(executable, encoded_command.len())
            .is_some_and(|units| units <= WINDOWS_COMMAND_LINE_LIMIT_UTF16_UNITS)
    );

    let mut process = Command::new(executable);
    process.args(POWERSHELL_ARGUMENTS).arg(encoded_command);
    configure_common(&mut process, cwd);

    // CREATE_SUSPENDED closes the spawn-to-enrollment race: no user code or
    // descendant can execute until dedicated Job assignment succeeds. Avoid
    // CREATE_NO_WINDOW because PowerShell then emits redirected text in the
    // legacy system code page instead of the inherited console's encoding.
    let mut flags = CREATE_NEW_PROCESS_GROUP | CREATE_SUSPENDED;
    if breakaway {
        flags |= CREATE_BREAKAWAY_FROM_JOB;
    }
    process.creation_flags(flags);
    process
}

// Session cwd is always the tool working directory; model-visible errors use
// `.` instead of the absolute host path.
fn require_session_cwd(cwd: &Path) -> Result<(), ToolError> {
    #[cfg(windows)]
    let context = "failed to spawn PowerShell 7";
    #[cfg(not(windows))]
    let context = "failed to spawn a platform shell";
    let metadata = std::fs::metadata(cwd).map_err(|err| {
        ToolError::Execution(format!(
            "{context}: working directory . is unavailable: {err}"
        ))
    })?;
    if metadata.is_dir() {
        Ok(())
    } else {
        Err(ToolError::Execution(format!(
            "{context}: working directory . is not a directory"
        )))
    }
}

fn configure_common(process: &mut Command, cwd: &Path) {
    process
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
}

#[cfg(windows)]
fn current_process_is_in_job() -> std::io::Result<bool> {
    use windows_sys::Win32::System::JobObjects::IsProcessInJob;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut in_job = 0;
    // SAFETY: GetCurrentProcess returns a borrowed pseudo-handle. A null Job
    // handle asks about any containing Job, and `in_job` is valid output storage.
    let queried =
        unsafe { IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &raw mut in_job) };
    if queried == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(in_job != 0)
    }
}

#[cfg(windows)]
fn windows_spawn_error(context: &str, err: std::io::Error) -> std::io::Error {
    // Contextual setup failures are not executable lookup failures. Keeping
    // them `Other` ensures only Command::spawn's bare NotFound result can
    // trigger managed PowerShell provisioning.
    std::io::Error::other(format!("{context}: {err}"))
}

/// Resume the only thread of a newly created, suspended Windows child.
#[cfg(windows)]
fn resume_windows_child(child: &Child) -> std::io::Result<()> {
    use std::mem::size_of;
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
    use windows_sys::Win32::Foundation::{
        ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessId, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME, WaitForSingleObject,
    };

    let process_handle = child
        .raw_handle()
        .ok_or_else(|| std::io::Error::other("suspended shell has no process handle"))?;
    let pid = child
        .id()
        .ok_or_else(|| std::io::Error::other("suspended shell has no process id"))?;
    // SAFETY: the Tokio Child retains this process HANDLE and no wait or kill
    // has occurred. GetProcessId borrows it only for this call.
    let handle_pid = unsafe { GetProcessId(process_handle) };
    if handle_pid == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if handle_pid != pid {
        return Err(std::io::Error::other(format!(
            "suspended shell process handle identifies {handle_pid}, but Child::id returned {pid}"
        )));
    }
    // CREATE_SUSPENDED prevents self-termination or additional threads. The
    // stable Child-owned process HANDLE is checked immediately before the PID
    // is used solely to find the initial thread, which is resumed immediately.
    // No timeout/cancellation cleanup path ever uses this PID.
    // SAFETY: `process_handle` is live and SYNCHRONIZE-capable; a zero timeout
    // only observes whether the represented process object has terminated.
    let wait_status = unsafe { WaitForSingleObject(process_handle, 0) };
    if wait_status == WAIT_FAILED {
        return Err(std::io::Error::last_os_error());
    }
    if wait_status != WAIT_TIMEOUT {
        return Err(std::io::Error::other(
            "suspended shell terminated before its initial thread could be resumed",
        ));
    }

    // SAFETY: TH32CS_SNAPTHREAD requests a system-wide, read-only snapshot;
    // the process-id argument is documented as ignored for thread snapshots.
    let raw_snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if raw_snapshot == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: CreateToolhelp32Snapshot returned a fresh owned HANDLE. The
    // OwnedHandle closes it exactly once on every return path.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw_snapshot) };

    let entry_len = u32::try_from(size_of::<THREADENTRY32>())
        .expect("THREADENTRY32 size fits in a Windows DWORD");
    let mut entry = THREADENTRY32 {
        dwSize: entry_len,
        ..THREADENTRY32::default()
    };
    // SAFETY: the snapshot handle is live and `entry` has the required size;
    // the API writes only within that structure for the duration of the call.
    if unsafe { Thread32First(snapshot.as_raw_handle(), &raw mut entry) } == 0 {
        return Err(std::io::Error::last_os_error());
    }

    loop {
        if entry.th32OwnerProcessID == pid {
            // CREATE_SUSPENDED prevents the primary thread from creating any
            // additional threads, so the matching entry is the initial thread.
            // SAFETY: the id came from the live snapshot; the requested access
            // is limited to changing this thread's suspend count.
            let raw_thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if raw_thread.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: OpenThread returned a fresh owned HANDLE. OwnedHandle
            // provides the matching CloseHandle on every return path.
            let thread = unsafe { OwnedHandle::from_raw_handle(raw_thread) };
            // SAFETY: `thread` is a live thread handle with suspend/resume
            // access. ResumeThread borrows it only for this call.
            let previous_count = unsafe { ResumeThread(thread.as_raw_handle()) };
            if previous_count == u32::MAX {
                return Err(std::io::Error::last_os_error());
            }
            if previous_count != 1 {
                return Err(std::io::Error::other(format!(
                    "suspended shell had unexpected suspend count {previous_count}"
                )));
            }
            return Ok(());
        }

        // SAFETY: the snapshot and initialized output structure remain live;
        // Thread32Next has the same buffer contract as Thread32First.
        if unsafe { Thread32Next(snapshot.as_raw_handle(), &raw mut entry) } == 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("no thread found for suspended shell process {pid}"),
                ));
            }
            return Err(err);
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
            .ok_or_else(|| std::io::Error::other("shell exited before process-group enrollment"))?;
        // Command::process_group(0) performs setpgid(0, 0) before exec and
        // makes spawn fail if that setup fails. Do not require the shell to
        // remain in the group here: a script can deliberately escape after
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

/// RAII owner for a Windows Job Object configured to kill all members when its
/// last handle closes. `OwnedHandle` provides the matching `CloseHandle`.
#[cfg(windows)]
struct WindowsJob {
    handle: std::os::windows::io::OwnedHandle,
}

#[cfg(windows)]
impl WindowsJob {
    fn new() -> std::io::Result<Self> {
        use std::mem::size_of;
        use std::os::windows::io::FromRawHandle as _;
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };

        // SAFETY: both optional pointers are null, requesting an unnamed,
        // non-inheritable Job Object. A non-null result is uniquely owned.
        let raw_job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw_job.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: CreateJobObjectW returned a fresh owned HANDLE. OwnedHandle
        // closes it exactly once with CloseHandle on every later error/drop.
        let handle = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(raw_job) };
        let job = Self { handle };

        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let info_len = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
            .expect("Job Object limit structure size fits u32");
        // SAFETY: the Job handle is live, `info` has the exact documented type
        // and byte length, and the API borrows it only for this call.
        let configured = unsafe {
            SetInformationJobObject(
                job.raw_handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast(),
                info_len,
            )
        };
        if configured == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(job)
    }

    fn assign(&self, child: &Child) -> std::io::Result<()> {
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;

        let process_handle = child
            .raw_handle()
            .ok_or_else(|| std::io::Error::other("shell exited before Job Object enrollment"))?;
        // SAFETY: both are borrowed, live kernel handles for the duration of
        // this call. The Job owns neither the tokio Child handle nor vice versa.
        if unsafe { AssignProcessToJobObject(self.raw_handle(), process_handle) } != 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    fn terminate(&self) -> std::io::Result<()> {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        // SAFETY: the owned Job handle remains live for this call.
        if unsafe { TerminateJobObject(self.raw_handle(), 1) } != 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    fn raw_handle(&self) -> windows_sys::Win32::Foundation::HANDLE {
        use std::os::windows::io::AsRawHandle as _;

        self.handle.as_raw_handle()
    }
}

/// Encode the user script itself as PowerShell's UTF-16LE Base64 transport.
///
/// No launcher script or .NET decoding API is inserted, so a leading `using`
/// statement remains the first statement and ConstrainedLanguage can execute
/// its permitted cmdlets. The exact `CreateProcessW` budget includes the quoted
/// executable, fixed arguments, encoded payload, spaces, and final UTF-16 NUL.
#[cfg(any(windows, test))]
pub(crate) fn encode_powershell_command(
    command: &str,
    executable: &Path,
) -> Result<String, ToolError> {
    let command_byte_len = command
        .encode_utf16()
        .count()
        .checked_mul(2)
        .ok_or_else(|| command_too_long(executable, None))?;
    let encoded_len =
        base64_encoded_len(command_byte_len).ok_or_else(|| command_too_long(executable, None))?;
    let command_line_units = powershell_command_line_units(executable, encoded_len)
        .ok_or_else(|| command_too_long(executable, Some(encoded_len)))?;
    if command_line_units > WINDOWS_COMMAND_LINE_LIMIT_UTF16_UNITS {
        return Err(command_too_long(executable, Some(encoded_len)));
    }

    Ok(BASE64_STANDARD.encode(utf16le_bytes(command, command_byte_len)))
}

#[cfg(any(windows, test))]
fn powershell_command_line_units(executable: &Path, encoded_len: usize) -> Option<usize> {
    // `std::process::Command` quotes argv[0] on Windows even when it contains no
    // spaces. Every fixed argument and Base64 character needs no extra quoting;
    // an empty Base64 argument is represented as `""`.
    let mut units = executable_utf16_units(executable).checked_add(2)?;
    for argument in POWERSHELL_ARGUMENTS {
        units = units
            .checked_add(1)?
            .checked_add(argument.encode_utf16().count())?;
    }
    units = units
        .checked_add(1)?
        .checked_add(if encoded_len == 0 { 2 } else { encoded_len })?;
    units.checked_add(1)
}

#[cfg(windows)]
fn executable_utf16_units(executable: &Path) -> usize {
    use std::os::windows::ffi::OsStrExt as _;

    executable.as_os_str().encode_wide().count()
}

#[cfg(all(test, not(windows)))]
fn executable_utf16_units(executable: &Path) -> usize {
    executable
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .count()
}

#[cfg(any(windows, test))]
fn maximum_encoded_command_chars(executable: &Path) -> Option<usize> {
    let one_character_line = powershell_command_line_units(executable, 1)?;
    WINDOWS_COMMAND_LINE_LIMIT_UTF16_UNITS.checked_sub(one_character_line.checked_sub(1)?)
}

#[cfg(any(windows, test))]
fn base64_encoded_len(byte_len: usize) -> Option<usize> {
    byte_len.checked_add(2)?.checked_div(3)?.checked_mul(4)
}

#[cfg(any(windows, test))]
fn utf16le_bytes(value: &str, byte_len: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(byte_len);
    for unit in value.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

#[cfg(any(windows, test))]
fn command_too_long(executable: &Path, encoded_len: Option<usize>) -> ToolError {
    let maximum = maximum_encoded_command_chars(executable)
        .map_or_else(|| "unrepresentable".to_owned(), |value| value.to_string());
    let encoded =
        encoded_len.map_or_else(|| "overflowed usize".to_owned(), |value| value.to_string());
    let executable_name = executable.file_name().map_or_else(
        || std::borrow::Cow::Borrowed("pwsh.exe"),
        |name| name.to_string_lossy(),
    );
    ToolError::InvalidArgs(format!(
        "command is too long for PowerShell 7's 32,767 UTF-16-code-unit CreateProcessW \
         command-line limit (including the terminator): encoded length is {encoded}, maximum \
         for executable {executable_name} is {maximum}"
    ))
}

#[cfg(test)]
#[path = "shell_tests.rs"]
mod tests;
