//! `bash` — the stable public name for running a platform-native shell command
//! in the session cwd, with a configurable timeout, captured stdout/stderr,
//! and output truncation. macOS/Linux use a POSIX shell; Windows uses
//! PowerShell 7. Timeout/cancellation tears down the platform containment
//! boundary; descendants that deliberately escape it are outside the guarantee.

// Rust guideline compliant 2026-08-27.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::builtin::shell;
use crate::ctx::ToolCtx;
use crate::stream::ToolStream;
use crate::tool::{Concurrency, Tool, ToolError, ToolResult};

/// Default command timeout (seconds).
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;
/// Combined stdout+stderr cap per call (~50 KiB, then a notice).
pub const MAX_OUTPUT_BYTES: usize = 50 * 1024;

// A fixed scratch buffer keeps discarded output from growing user-space memory.
const OUTPUT_READ_CHUNK_BYTES: usize = 16 * 1024;
// BOM-marked UTF-16 ASCII uses two raw bytes per rendered UTF-8 byte. Retain
// one rendered byte beyond the output cap so truncation remains observable.
const MAX_RETAINED_OUTPUT_BYTES: usize = 2 + 2 * (MAX_OUTPUT_BYTES + 1);

/// The `bash` builtin.
#[derive(Debug)]
pub struct BashTool {
    default_timeout: Duration,
}

impl BashTool {
    /// A bash tool with the default 120 s timeout.
    pub fn new() -> Self {
        Self {
            default_timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        }
    }

    /// A bash tool with a custom default timeout; per-call `timeout_secs`
    /// arguments still take precedence.
    pub fn with_default_timeout(secs: u64) -> Self {
        Self {
            default_timeout: Duration::from_secs(secs),
        }
    }
}

impl Default for BashTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Arguments for [`BashTool`].
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BashArgs {
    /// Command to execute with the platform shell (POSIX shell on
    /// macOS/Linux, PowerShell 7 on Windows), inheriting the environment and
    /// using the session cwd as its working directory.
    pub command: String,
    /// Timeout in seconds for this command (default: 120).
    pub timeout_secs: Option<u64>,
}

/// How the wait ended.
enum Outcome {
    Done(std::io::Result<std::process::ExitStatus>),
    Timeout,
    Cancelled,
}

#[derive(Debug, Default)]
struct CapturedStream {
    retained: Vec<u8>,
    total_bytes: u64,
}

impl CapturedStream {
    fn new() -> Self {
        Self {
            retained: Vec::with_capacity(MAX_RETAINED_OUTPUT_BYTES),
            total_bytes: 0,
        }
    }
}

#[async_trait]
impl Tool for BashTool {
    type Args = BashArgs;
    type Output = ();

    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a command with the platform shell (a POSIX shell on \
         macOS/Linux, PowerShell 7 on Windows) and return captured stdout/stderr \
         truncated beyond 50 KiB. A non-zero exit is an error result, not a \
         tool failure. Default timeout: 120 s; timeout or cancellation tears \
         down the shell's platform process-containment boundary."
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(
            "bash: run a command with the platform shell (PowerShell 7 on Windows; \
             command, optional timeout_secs).",
        )
    }

    fn concurrency(&self) -> Concurrency {
        Concurrency::Exclusive
    }

    fn mutates_fs(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        args: Self::Args,
        ctx: &ToolCtx,
        _out: &mut ToolStream,
    ) -> Result<ToolResult, ToolError> {
        let timeout = Duration::from_secs(
            args.timeout_secs
                .unwrap_or(self.default_timeout.as_secs())
                .max(1),
        );
        let started = Instant::now();
        if ctx.cancel.is_cancelled() {
            return Err(command_cancelled_error());
        }

        // One deadline covers both managed-shell provisioning and execution.
        // Cancellation is polled first so a pre-cancelled call cannot spawn a
        // shell or begin configuring the managed Windows runtime.
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        let spawned = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => return Err(command_cancelled_error()),
            _ = &mut deadline => {
                return Ok(timed_out_before_spawn_result(
                    &args.command,
                    shell::preferred_identifier(),
                    started.elapsed().as_millis() as u64,
                    timeout,
                ));
            }
            spawned = shell::spawn(&args.command, &ctx.cwd) => spawned?,
        };
        let shell::SpawnedShell {
            mut child,
            identifier: shell_identifier,
            process_tree,
        } = spawned;

        let mut stdout_pipe = Some(child.stdout.take().expect("stdout was piped"));
        let mut stderr_pipe = Some(child.stderr.take().expect("stderr was piped"));
        let mut stdout = CapturedStream::new();
        let mut stderr = CapturedStream::new();

        // Race collection against cancellation and the deadline. Collection
        // drains both pipes concurrently while retaining bounded prefixes
        // before it ever polls Child::wait. An exited leader therefore stays
        // unreaped while a descendant (including one outside the process
        // group) keeps a pipe open, pinning the PID/PGID until timeout cleanup.
        let outcome = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => Outcome::Cancelled,
            _ = &mut deadline => Outcome::Timeout,
            status = collect_shell_output(
                &mut child,
                &mut stdout_pipe,
                &mut stderr_pipe,
                &mut stdout,
                &mut stderr,
            ) => Outcome::Done(status),
        };

        let duration_ms = started.elapsed().as_millis() as u64;
        match outcome {
            Outcome::Done(Ok(status)) => Ok(format_result(
                status,
                &args.command,
                shell_identifier,
                stdout,
                stderr,
                duration_ms,
                false,
                None,
            )),
            Outcome::Done(Err(err)) => {
                let teardown = process_tree.kill_and_reap(&mut child).await;
                drop(stdout_pipe);
                drop(stderr_pipe);
                Err(collection_error(&err, teardown.as_ref().err()))
            }
            Outcome::Timeout => {
                // Keep the leader and any still-open pipe handles alive while
                // the platform backend tears down its containment boundary,
                // then close our read ends so escaped descendants cannot block
                // us.
                let teardown = process_tree.kill_and_reap(&mut child).await;
                drop(stdout_pipe);
                drop(stderr_pipe);
                Ok(timed_out_result(
                    &args.command,
                    shell_identifier,
                    stdout,
                    stderr,
                    duration_ms,
                    timeout,
                    teardown,
                ))
            }
            Outcome::Cancelled => {
                let teardown = process_tree.kill_and_reap(&mut child).await;
                drop(stdout_pipe);
                drop(stderr_pipe);
                Err(command_cancelled_error_from(teardown.err()))
            }
        }
    }
}

fn command_cancelled_error() -> ToolError {
    command_cancelled_error_from(None)
}

fn command_cancelled_error_from(teardown: Option<std::io::Error>) -> ToolError {
    match teardown {
        Some(err) => ToolError::Execution(format!(
            "command cancelled before completion; termination failed: {err}"
        )),
        None => ToolError::Execution("command cancelled before completion".into()),
    }
}

fn collection_error(collection: &std::io::Error, teardown: Option<&std::io::Error>) -> ToolError {
    match teardown {
        Some(err) => ToolError::Execution(format!(
            "failed to collect command output: {collection}; termination failed: {err}"
        )),
        None => ToolError::Execution(format!("failed to collect command output: {collection}")),
    }
}

fn timed_out_before_spawn_result(
    command: &str,
    shell_identifier: &str,
    duration_ms: u64,
    timeout: Duration,
) -> ToolResult {
    let notice = format!(
        "[command timed out after {}s before the shell started]",
        timeout.as_secs()
    );
    let result = format_result(
        /* status */ None,
        command,
        shell_identifier,
        CapturedStream::default(),
        CapturedStream::default(),
        duration_ms,
        true,
        Some(&notice),
    );
    mark_timed_out(result)
}

fn timed_out_result(
    command: &str,
    shell_identifier: &str,
    stdout: CapturedStream,
    stderr: CapturedStream,
    duration_ms: u64,
    timeout: Duration,
    teardown: Result<(), std::io::Error>,
) -> ToolResult {
    let notice = timeout_notice(timeout, teardown.as_ref().err());
    let result = format_result(
        /* status */ None,
        command,
        shell_identifier,
        stdout,
        stderr,
        duration_ms,
        true,
        Some(&notice),
    );
    mark_timed_out(result)
}

fn timeout_notice(timeout: Duration, teardown: Option<&std::io::Error>) -> String {
    match teardown {
        Some(err) => format!(
            "[command timed out after {}s; termination failed: {err}]",
            timeout.as_secs()
        ),
        None => format!(
            "[command timed out after {}s and was killed]",
            timeout.as_secs()
        ),
    }
}

fn mark_timed_out(mut result: ToolResult) -> ToolResult {
    result.details.as_mut().expect("details were populated")["timed_out"] = json!(true);
    result
}

async fn collect_shell_output(
    child: &mut tokio::process::Child,
    stdout_pipe: &mut Option<tokio::process::ChildStdout>,
    stderr_pipe: &mut Option<tokio::process::ChildStderr>,
    stdout: &mut CapturedStream,
    stderr: &mut CapturedStream,
) -> std::io::Result<std::process::ExitStatus> {
    let (out, err) = tokio::join!(
        read_bounded(stdout_pipe, stdout),
        read_bounded(stderr_pipe, stderr),
    );
    out.and(err)?;

    // Do not move this wait above the pipe-drain barrier. On Unix, an
    // unreaped live/zombie leader reserves its PID and therefore its PGID
    // number while an escaped descendant can keep collection pending.
    child.wait().await
}

// Drain one stream to EOF while retaining only the prefix needed for rendering.
// Continuing to read prevents capture limits from changing the child's exit
// behavior, and a fixed scratch buffer bounds memory after the prefix is full.
async fn read_bounded<R>(pipe: &mut Option<R>, captured: &mut CapturedStream) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let Some(reader) = pipe.as_mut() else {
        return Ok(());
    };
    let retained_limit = MAX_RETAINED_OUTPUT_BYTES;
    let mut chunk = [0_u8; OUTPUT_READ_CHUNK_BYTES];
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        captured.total_bytes =
            captured
                .total_bytes
                .checked_add(u64::try_from(count).map_err(|_| {
                    std::io::Error::other("shell output read length does not fit u64")
                })?)
                .ok_or_else(|| std::io::Error::other("shell output length overflowed u64"))?;
        let retained = retained_limit.saturating_sub(captured.retained.len());
        captured
            .retained
            .extend_from_slice(&chunk[..count.min(retained)]);
    }
    drop(pipe.take());
    Ok(())
}

/// Assemble the tool result from collected output.
///
/// `status == None` marks a command that did not finish (timeout path);
/// such results are always `is_error`.
#[expect(
    clippy::too_many_arguments,
    reason = "result assembly mirrors the tool's stable output fields"
)]
fn format_result(
    status: impl Into<Option<std::process::ExitStatus>>,
    command: &str,
    shell: &str,
    stdout: CapturedStream,
    stderr: CapturedStream,
    duration_ms: u64,
    forced_error: bool,
    notice: Option<&str>,
) -> ToolResult {
    let status = status.into();
    let stdout_text = decode_captured_text(&stdout.retained);
    let stderr_text = decode_captured_text(&stderr.retained);

    let mut text = String::new();
    if !stdout_text.trim().is_empty() {
        text.push_str(&stdout_text);
    }
    if !stderr_text.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str("[stderr]\n");
        text.push_str(&stderr_text);
    }

    let (text, truncated) = crate::builtin::truncate_bytes(&text, MAX_OUTPUT_BYTES);
    let mut text = text;
    if truncated {
        let total = stdout.total_bytes.saturating_add(stderr.total_bytes);
        text.push_str(&format!(
            "\n[output truncated: showed first {MAX_OUTPUT_BYTES} of {total} bytes]"
        ));
    }

    let is_error = forced_error || status.is_some_and(|s| !s.success());
    if is_error {
        match &status {
            Some(s) => text.push_str(&format!("\n[exit code: {}]", display_exit(s))),
            None => match notice {
                // A caller-provided notice (e.g. the timeout banner)
                // replaces the generic no-status line.
                Some(notice) => {
                    text.push('\n');
                    text.push_str(notice);
                }
                None => text.push_str("\n[no exit status: command did not finish]"),
            },
        }
    } else if let Some(notice) = notice {
        text.push('\n');
        text.push_str(notice);
    }

    let mut details = json!({
        "command": command,
        "shell": shell,
        "stdout_bytes": stdout.total_bytes,
        "stderr_bytes": stderr.total_bytes,
        "duration_ms": duration_ms,
        "truncated": truncated,
    });
    if let Some(s) = status {
        details["exit_code"] = json!(display_exit(&s));
    }

    ToolResult {
        content: vec![mcode_core::message::ContentBlock::Text(text.into())],
        is_error,
        details: Some(details),
    }
}

fn display_exit(status: &std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(-1)
}

/// Decode captured shell text without consulting a legacy system code page.
fn decode_captured_text(bytes: &[u8]) -> String {
    if let Some(payload) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8_lossy(payload).into_owned();
    }
    if let Some(payload) = bytes.strip_prefix(&[0xff, 0xfe]) {
        return decode_utf16(payload, u16::from_le_bytes);
    }
    if let Some(payload) = bytes.strip_prefix(&[0xfe, 0xff]) {
        return decode_utf16(payload, u16::from_be_bytes);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn decode_utf16(payload: &[u8], decode_unit: fn([u8; 2]) -> u16) -> String {
    let (chunks, remainder) = payload.as_chunks::<2>();
    let units = chunks.iter().copied().map(decode_unit).collect::<Vec<_>>();
    let mut decoded = String::from_utf16_lossy(&units);
    if !remainder.is_empty() {
        decoded.push('\u{fffd}');
    }
    decoded
}

#[cfg(test)]
#[path = "bash_tests.rs"]
mod tests;
