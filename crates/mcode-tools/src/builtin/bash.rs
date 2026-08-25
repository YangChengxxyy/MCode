//! `bash` — run a shell command via `bash -c` in the session cwd, with a
//! configurable timeout, captured stdout/stderr, and output truncation.
//! On Unix the shell runs as its own process-group leader and
//! timeout/cancel kills the whole group, so grandchildren (dev servers,
//! watchers, …) cannot outlive the call holding the output pipes; on
//! other platforms only the shell itself is killed.

use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tokio::io::AsyncReadExt;

use crate::ctx::ToolCtx;
use crate::stream::ToolStream;
use crate::tool::{Concurrency, Tool, ToolError, ToolResult};

/// Default command timeout (seconds).
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;
/// Combined stdout+stderr cap per call (~50 KiB, then a notice).
pub const MAX_OUTPUT_BYTES: usize = 50 * 1024;

/// The `bash` builtin.
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
    /// Shell command to execute (run via `bash -c`, inheriting the
    /// environment, with the session cwd as working directory).
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

#[async_trait]
impl Tool for BashTool {
    type Args = BashArgs;
    type Output = ();

    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a shell command via bash and return its captured stdout/stderr \
         (truncated beyond 50 KiB). A non-zero exit is reported as an error \
         result, not a tool failure. Default timeout: 120 s; on timeout the \
         whole process group (child processes included) is killed on Unix \
         (just the shell elsewhere)."
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some("bash: run a shell command (command, optional timeout_secs).")
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

        let mut command = tokio::process::Command::new("bash");
        command
            .arg("-c")
            .arg(&args.command)
            .current_dir(&ctx.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Make the shell its own process-group leader so a timeout or
        // cancel can kill the whole tree: `bash -c "sleep 300"` timing
        // out must not leave the orphaned `sleep` alive holding the
        // inherited stdout/stderr pipe ends.
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|err| ToolError::Execution(format!("failed to spawn bash: {err}")))?;

        let mut stdout_pipe = child.stdout.take().expect("stdout was piped");
        let mut stderr_pipe = child.stderr.take().expect("stderr was piped");
        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();

        // Race the process collection against the deadline and the
        // session cancellation token. The three reads run concurrently;
        // on timeout/cancel the buffers keep whatever arrived so far.
        let outcome = tokio::select! {
            _ = ctx.cancel.cancelled() => Outcome::Cancelled,
            collected = tokio::time::timeout(timeout, async {
                let read_out = stdout_pipe.read_to_end(&mut stdout_buf);
                let read_err = stderr_pipe.read_to_end(&mut stderr_buf);
                let status = child.wait();
                let (status, out, err) = tokio::join!(status, read_out, read_err);
                // Flatten: any read failure OR wait failure is one io::Error.
                out.and(err).and(status)
            }) => match collected {
                Ok(status) => Outcome::Done(status),
                Err(_) => Outcome::Timeout,
            },
        };

        let duration_ms = started.elapsed().as_millis() as u64;
        match outcome {
            Outcome::Done(Ok(status)) => Ok(format_result(
                status,
                &args.command,
                stdout_buf,
                stderr_buf,
                duration_ms,
                false,
                None,
            )),
            Outcome::Done(Err(err)) => Err(ToolError::Execution(format!(
                "failed to collect command output: {err}"
            ))),
            Outcome::Timeout => {
                // select! drops the collection future before this arm
                // runs, so the child handle is free again; reap it. The
                // timeout banner is folded into the single text block.
                kill_process_tree(&mut child);
                let _ = child.wait().await;
                let notice = format!(
                    "[command timed out after {}s and was killed]",
                    timeout.as_secs()
                );
                let mut result = format_result(
                    /* status */ None,
                    &args.command,
                    stdout_buf,
                    stderr_buf,
                    duration_ms,
                    true,
                    Some(&notice),
                );
                result.details.as_mut().unwrap()["timed_out"] = json!(true);
                Ok(result)
            }
            Outcome::Cancelled => {
                kill_process_tree(&mut child);
                let _ = child.wait().await;
                Err(ToolError::Execution(
                    "command cancelled before completion".into(),
                ))
            }
        }
    }
}

/// Kill everything the command spawned, not just the `bash` leader.
///
/// The shell runs as its own process-group leader (`process_group(0)`),
/// so its pid names the group; grandchildren inherit both the group
/// membership and the output pipe ends, so a leader-only kill would
/// leave them running. `start_kill` remains as the non-Unix fallback
/// and belt-and-braces reap of the leader itself.
fn kill_process_tree(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // SAFETY: killpg(2) targeting the child's own process group;
            // failure (group already gone) falls through to start_kill.
            unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL) };
        }
    }
    let _ = child.start_kill();
}

/// Assemble the tool result from collected output.
///
/// `status == None` marks a command that did not finish (timeout path);
/// such results are always `is_error`.
#[allow(clippy::too_many_arguments)]
fn format_result(
    status: impl Into<Option<std::process::ExitStatus>>,
    command: &str,
    stdout_buf: Vec<u8>,
    stderr_buf: Vec<u8>,
    duration_ms: u64,
    forced_error: bool,
    notice: Option<&str>,
) -> ToolResult {
    let status = status.into();
    let stdout = String::from_utf8_lossy(&stdout_buf);
    let stderr = String::from_utf8_lossy(&stderr_buf);

    let mut text = String::new();
    if !stdout.trim().is_empty() {
        text.push_str(&stdout);
    }
    if !stderr.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str("[stderr]\n");
        text.push_str(&stderr);
    }

    let (text, truncated) = crate::builtin::truncate_bytes(&text, MAX_OUTPUT_BYTES);
    let mut text = text;
    if truncated {
        let total = stdout_buf.len() + stderr_buf.len();
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
        "stdout_bytes": stdout_buf.len(),
        "stderr_bytes": stderr_buf.len(),
        "duration_ms": duration_ms,
        "truncated": truncated,
    });
    if let Some(s) = status {
        details["exit_code"] = json!(display_exit(&s));
    }

    ToolResult {
        content: vec![mcode_core::message::ContentBlock::Text(text)],
        is_error,
        details: Some(details),
    }
}

fn display_exit(status: &std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(-1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::test_support::{ctx_at, run_dyn, text_of};
    use crate::ctx::ToolCtx;
    use mcode_core::ids::{CallId, SessionId};
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn captures_stdout_and_runs_in_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());

        let result = run_dyn(&BashTool::new(), json!({"command": "echo hello"}), &ctx)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(text_of(&result), "hello\n");
    }

    #[tokio::test]
    async fn pwd_reflects_session_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());

        let result = run_dyn(&BashTool::new(), json!({"command": "pwd"}), &ctx)
            .await
            .unwrap();
        // macOS tempdirs are symlinked (/var → /private/var): compare
        // canonicalized paths.
        let printed = std::fs::canonicalize(text_of(&result).trim()).unwrap();
        let expected = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(printed, expected);
    }

    #[tokio::test]
    async fn non_zero_exit_is_error_result_not_tool_error() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());

        let result = run_dyn(
            &BashTool::new(),
            json!({"command": "echo oops; exit 3"}),
            &ctx,
        )
        .await
        .unwrap(); // Ok(...) — the failure is data
        assert!(result.is_error);
        assert!(text_of(&result).contains("oops"));
        assert!(text_of(&result).contains("[exit code: 3]"));
        assert_eq!(result.details.unwrap()["exit_code"], 3);
    }

    #[tokio::test]
    async fn stderr_is_captured_and_labelled() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());

        let result = run_dyn(
            &BashTool::new(),
            json!({"command": "echo out; echo err >&2"}),
            &ctx,
        )
        .await
        .unwrap();
        let text = text_of(&result);
        assert!(text.contains("out"));
        assert!(text.contains("[stderr]"));
        assert!(text.contains("err"));
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn timeout_kills_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());

        let started = Instant::now();
        let result = run_dyn(
            &BashTool::new(),
            json!({"command": "echo started; sleep 30", "timeout_secs": 1}),
            &ctx,
        )
        .await
        .unwrap();
        let elapsed = started.elapsed();
        assert!(result.is_error);
        assert!(text_of(&result).contains("timed out after 1s"));
        // Killed at the deadline, not after the full sleep.
        assert!(elapsed < Duration::from_secs(10), "took {elapsed:?}");
        assert_eq!(result.details.unwrap()["timed_out"], true);
    }

    #[tokio::test]
    async fn timeout_kills_grandchildren_not_just_the_shell() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());
        let log = dir.path().join("beat.log");
        let log_arg = log.display().to_string();

        // A grandchild that keeps appending to a file must stop when the
        // tool times out; a leader-only kill would leave it running.
        let result = run_dyn(
            &BashTool::new(),
            json!({
                "command": format!(
                    "echo go; while true; do echo x >> {log_arg}; sleep 0.2; done & sleep 30"
                ),
                "timeout_secs": 1,
            }),
            &ctx,
        )
        .await
        .unwrap();
        assert!(result.is_error);
        assert!(text_of(&result).contains("timed out after 1s"));

        // If any group member survived, the log keeps growing.
        tokio::time::sleep(Duration::from_millis(800)).await;
        let first = std::fs::read_to_string(&log)
            .map(|s| s.lines().count())
            .unwrap_or(0);
        tokio::time::sleep(Duration::from_millis(800)).await;
        let second = std::fs::read_to_string(&log)
            .map(|s| s.lines().count())
            .unwrap_or(0);
        assert!(first > 0, "grandchild never ran (log missing)");
        assert_eq!(first, second, "grandchild survived the group kill");
    }

    #[tokio::test]
    async fn huge_output_is_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());

        let result = run_dyn(
            &BashTool::new(),
            json!({"command": "printf 'x%.0s' {1..60000}"}),
            &ctx,
        )
        .await
        .unwrap();
        assert!(!result.is_error);
        let text = text_of(&result);
        assert!(text.contains("[output truncated: showed first 51200 of 60000 bytes]"));
        assert!(text.len() < 60000);
        assert_eq!(result.details.unwrap()["truncated"], true);
    }

    #[tokio::test]
    async fn cancellation_token_aborts_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let ctx =
            ToolCtx::new(dir.path(), SessionId::from("s"), CallId::from("c")).with_cancel(cancel);

        let err = run_dyn(&BashTool::new(), json!({"command": "sleep 30"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Execution(_)));
        assert!(err.to_string().contains("cancelled"), "{err}");
    }

    #[tokio::test]
    async fn spawn_failure_is_execution_error() {
        let ctx = ToolCtx::new(
            "/definitely/not/a/dir",
            SessionId::from("s"),
            CallId::from("c"),
        );
        let err = run_dyn(&BashTool::new(), json!({"command": "echo hi"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Execution(_)));
        assert!(err.to_string().contains("spawn"), "{}", err.to_string());
    }

    #[tokio::test]
    async fn per_call_timeout_overrides_default() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());

        // Default (2 s) would time out; per-call 30 s lets it finish.
        let result = run_dyn(
            &BashTool::with_default_timeout(2),
            json!({"command": "sleep 4; echo done", "timeout_secs": 30}),
            &ctx,
        )
        .await
        .unwrap();
        assert!(!result.is_error, "{:?}", text_of(&result));
        assert_eq!(text_of(&result).trim(), "done");

        // And the tool-level default still applies without an override.
        let result = run_dyn(
            &BashTool::with_default_timeout(1),
            json!({"command": "sleep 30"}),
            &ctx,
        )
        .await
        .unwrap();
        assert!(result.is_error);
        assert!(text_of(&result).contains("timed out after 1s"));
    }

    #[tokio::test]
    async fn empty_output_success() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());

        let result = run_dyn(&BashTool::new(), json!({"command": "true"}), &ctx)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(text_of(&result), "");
    }
}
