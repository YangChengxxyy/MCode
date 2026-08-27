//! Platform-shell selection and process construction for the public `bash` tool.
//!
//! The public tool name is intentionally stable. This module chooses the
//! native execution backend and constructs the process; containment is owned
//! by [`crate::builtin::process`].

// Rust guideline compliant 2026-08-27.

use std::path::Path;
use std::process::Stdio;

#[cfg(any(windows, test))]
use base64::Engine as _;
#[cfg(any(windows, test))]
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use tokio::process::{Child, Command};

use crate::builtin::process::ProcessTree;
#[cfg(windows)]
use crate::builtin::process::spawn_windows_enrolled;
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
        Ok((child, process_tree)) => Ok(SpawnedShell {
            child,
            identifier: WINDOWS_SHELL_EXECUTABLE,
            process_tree,
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let managed = crate::builtin::powershell::ensure_pwsh().await?;
            let managed_command = encode_powershell_command(command, &managed)?;
            let (child, process_tree) = spawn_windows_candidate(&managed, &managed_command, cwd)
                .map_err(|managed_error| {
                    ToolError::Execution(format!(
                        "failed to spawn managed PowerShell 7 from the managed pwsh cache: {managed_error}"
                    ))
                })?;
            Ok(SpawnedShell {
                child,
                identifier: WINDOWS_SHELL_EXECUTABLE,
                process_tree,
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
            let process_tree = ProcessTree::enroll_unix(&child)?;
            Ok((child, process_tree))
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
) -> std::io::Result<(Child, ProcessTree)> {
    spawn_windows_enrolled(|breakaway| {
        build_windows_command(executable, encoded_command, cwd, breakaway)
    })
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
