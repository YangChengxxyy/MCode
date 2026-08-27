//! Windows Job Object enrollment for suspended children.
//!
//! A child is created with `CREATE_SUSPENDED`, assigned to a dedicated
//! kill-on-close Job, and only then resumed. Direct `Command` `NotFound`
//! errors keep their original kind so callers can distinguish missing
//! executables from containment failures.

// Rust guideline compliant 2026-08-27.

use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use tokio::process::{Child, Command};

use super::ProcessTree;

/// RAII owner for a Windows Job Object configured to kill all members when its
/// last handle closes. `OwnedHandle` provides the matching `CloseHandle`.
pub(super) struct WindowsJob {
    handle: OwnedHandle,
}

/// Spawns a Windows child, enrolls it while suspended, then resumes it.
///
/// The builder receives whether it must request breakaway from a parent Job.
/// The child never executes user code before dedicated Job enrollment.
///
/// # Errors
///
/// Returns an error when host Job inspection, spawn, enrollment, or resume
/// fails. A direct spawn `NotFound` error retains its original error kind.
pub(crate) fn spawn_windows_enrolled(
    mut build: impl FnMut(bool) -> Command,
) -> std::io::Result<(Child, ProcessTree)> {
    let parent_job = current_process_is_in_job()
        .map_err(|err| wrap_other("failed to query the host's Job membership", err))?;
    let job = WindowsJob::new()
        .map_err(|err| wrap_other("failed to create the child Job Object", err))?;

    // Windows 8+ supports nested Jobs. First inherit any host Job, then add the
    // still-suspended child to our dedicated Job. This keeps ordinary CI host
    // limits while adding a narrower descendant-containment boundary.
    let mut child = build(false).spawn()?;
    let enrollment = job.assign(&child);
    if let Err(nested_error) = enrollment {
        if !parent_job {
            let err = wrap_other(
                "failed to enroll the suspended child in its dedicated Job Object",
                nested_error,
            );
            let _ = child.start_kill();
            return Err(err);
        }

        // Older or specially configured parent Jobs can reject nesting. The
        // first child is still suspended and is terminated through its owned
        // process handle before the explicit-breakaway fallback is attempted.
        let _ = child.start_kill();
        child = build(true).spawn().map_err(|breakaway_error| {
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
            let err = wrap_other(
                "CREATE_BREAKAWAY_FROM_JOB succeeded but dedicated Job enrollment failed",
                assign_error,
            );
            let _ = child.start_kill();
            return Err(err);
        }
    }

    if let Err(err) = resume_windows_child(&child) {
        let err = wrap_other("failed to resume the enrolled child", err);
        let _ = job.terminate();
        let _ = child.start_kill();
        return Err(err);
    }

    Ok((child, ProcessTree { job }))
}

impl WindowsJob {
    fn new() -> std::io::Result<Self> {
        use std::mem::size_of;
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
        let handle = unsafe { OwnedHandle::from_raw_handle(raw_job) };
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
            .ok_or_else(|| std::io::Error::other("child exited before Job Object enrollment"))?;
        // SAFETY: both are borrowed, live kernel handles for the duration of
        // this call. The Job owns neither the tokio Child handle nor vice versa.
        if unsafe { AssignProcessToJobObject(self.raw_handle(), process_handle) } != 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    pub(super) fn terminate(&self) -> std::io::Result<()> {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        // SAFETY: the owned Job handle remains live for this call.
        if unsafe { TerminateJobObject(self.raw_handle(), 1) } != 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    fn raw_handle(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.handle.as_raw_handle()
    }
}

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

fn wrap_other(context: &str, err: std::io::Error) -> std::io::Error {
    // Contextual setup failures are not executable lookup failures. Keeping
    // them `Other` ensures only Command::spawn's bare NotFound result can
    // trigger managed PowerShell provisioning.
    std::io::Error::other(format!("{context}: {err}"))
}

/// Resume the only thread of a newly created, suspended Windows child.
fn resume_windows_child(child: &Child) -> std::io::Result<()> {
    use std::mem::size_of;
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
        .ok_or_else(|| std::io::Error::other("suspended child has no process handle"))?;
    let pid = child
        .id()
        .ok_or_else(|| std::io::Error::other("suspended child has no process id"))?;
    // SAFETY: the Tokio Child retains this process HANDLE and no wait or kill
    // has occurred. GetProcessId borrows it only for this call.
    let handle_pid = unsafe { GetProcessId(process_handle) };
    if handle_pid == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if handle_pid != pid {
        return Err(std::io::Error::other(format!(
            "suspended child process handle identifies {handle_pid}, but Child::id returned {pid}"
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
            "suspended child terminated before its initial thread could be resumed",
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
                    "suspended child had unexpected suspend count {previous_count}"
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
                    format!("no thread found for suspended child process {pid}"),
                ));
            }
            return Err(err);
        }
    }
}
