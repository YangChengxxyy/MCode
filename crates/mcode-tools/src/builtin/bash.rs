//! `bash` — the stable public name for running a platform-native shell command
//! in the session cwd, with a configurable timeout, captured stdout/stderr,
//! and output truncation. macOS/Linux use a POSIX shell; Windows uses
//! PowerShell 7. Timeout/cancellation tears down the platform containment
//! boundary; descendants that deliberately escape it are outside the guarantee.

// Rust guideline compliant 2026-08-26.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tokio::io::AsyncReadExt;

use crate::builtin::shell;
use crate::ctx::ToolCtx;
use crate::stream::ToolStream;
use crate::tool::{Concurrency, Tool, ToolError, ToolResult};

/// Default command timeout (seconds).
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;
/// Combined stdout+stderr cap per call (~50 KiB, then a notice).
pub const MAX_OUTPUT_BYTES: usize = 50 * 1024;

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

        let mut stdout_pipe = child.stdout.take().expect("stdout was piped");
        let mut stderr_pipe = child.stderr.take().expect("stderr was piped");
        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();

        // Race collection against cancellation and the deadline. Collection
        // drains both pipes before it ever polls Child::wait. Therefore an
        // exited leader remains unreaped while any descendant (including one
        // outside the process group) keeps a pipe open, pinning the PID/PGID
        // identity until timeout cleanup validates and signals it.
        let outcome = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => Outcome::Cancelled,
            _ = &mut deadline => Outcome::Timeout,
            status = collect_shell_output(
                &mut child,
                &mut stdout_pipe,
                &mut stderr_pipe,
                &mut stdout_buf,
                &mut stderr_buf,
            ) => Outcome::Done(status),
        };

        let duration_ms = started.elapsed().as_millis() as u64;
        match outcome {
            Outcome::Done(Ok(status)) => Ok(format_result(
                status,
                &args.command,
                shell_identifier,
                stdout_buf,
                stderr_buf,
                duration_ms,
                false,
                None,
            )),
            Outcome::Done(Err(err)) => {
                process_tree.kill_and_reap(&mut child).await;
                drop(stdout_pipe);
                drop(stderr_pipe);
                Err(ToolError::Execution(format!(
                    "failed to collect command output: {err}"
                )))
            }
            Outcome::Timeout => {
                // Keep the leader and its pipe handles alive while the
                // platform backend tears down its containment boundary, then
                // close our read ends so escaped descendants cannot block us.
                process_tree.kill_and_reap(&mut child).await;
                drop(stdout_pipe);
                drop(stderr_pipe);
                Ok(timed_out_result(
                    &args.command,
                    shell_identifier,
                    stdout_buf,
                    stderr_buf,
                    duration_ms,
                    timeout,
                ))
            }
            Outcome::Cancelled => {
                process_tree.kill_and_reap(&mut child).await;
                drop(stdout_pipe);
                drop(stderr_pipe);
                Err(command_cancelled_error())
            }
        }
    }
}

fn command_cancelled_error() -> ToolError {
    ToolError::Execution("command cancelled before completion".into())
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
        Vec::new(),
        Vec::new(),
        duration_ms,
        true,
        Some(&notice),
    );
    mark_timed_out(result)
}

fn timed_out_result(
    command: &str,
    shell_identifier: &str,
    stdout_buf: Vec<u8>,
    stderr_buf: Vec<u8>,
    duration_ms: u64,
    timeout: Duration,
) -> ToolResult {
    let notice = format!(
        "[command timed out after {}s and was killed]",
        timeout.as_secs()
    );
    let result = format_result(
        /* status */ None,
        command,
        shell_identifier,
        stdout_buf,
        stderr_buf,
        duration_ms,
        true,
        Some(&notice),
    );
    mark_timed_out(result)
}

fn mark_timed_out(mut result: ToolResult) -> ToolResult {
    result.details.as_mut().expect("details were populated")["timed_out"] = json!(true);
    result
}

async fn collect_shell_output(
    child: &mut tokio::process::Child,
    stdout_pipe: &mut tokio::process::ChildStdout,
    stderr_pipe: &mut tokio::process::ChildStderr,
    stdout_buf: &mut Vec<u8>,
    stderr_buf: &mut Vec<u8>,
) -> std::io::Result<std::process::ExitStatus> {
    let read_out = stdout_pipe.read_to_end(stdout_buf);
    let read_err = stderr_pipe.read_to_end(stderr_buf);
    let (out, err) = tokio::join!(read_out, read_err);
    out.and(err)?;

    // Do not move this wait above the pipe-drain barrier. On Unix, an
    // unreaped live/zombie leader reserves its PID and therefore its PGID
    // number while an escaped descendant can keep collection pending.
    child.wait().await
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
    stdout_buf: Vec<u8>,
    stderr_buf: Vec<u8>,
    duration_ms: u64,
    forced_error: bool,
    notice: Option<&str>,
) -> ToolResult {
    let status = status.into();
    let stdout = decode_captured_text(&stdout_buf);
    let stderr = decode_captured_text(&stderr_buf);

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
        "shell": shell,
        "stdout_bytes": stdout_buf.len(),
        "stderr_bytes": stderr_buf.len(),
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
    let mut chunks = payload.chunks_exact(2);
    let units = chunks
        .by_ref()
        .map(|chunk| decode_unit([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    let mut decoded = String::from_utf16_lossy(&units);
    if !chunks.remainder().is_empty() {
        decoded.push('\u{fffd}');
    }
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::test_support::{ctx_at, run_dyn, text_of};
    use crate::ctx::ToolCtx;
    use crate::tool::ToolDyn;
    use mcode_core::ids::{CallId, SessionId};
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn public_contract_keeps_bash_name_and_arguments() {
        let tool = BashTool::new();
        let dyn_tool: &dyn ToolDyn = &tool;
        let spec = dyn_tool.spec();

        assert_eq!(spec.name, "bash");
        assert!(spec.params_schema["properties"]["command"].is_object());
        assert!(spec.params_schema["properties"]["timeout_secs"].is_object());
        assert!(spec.description.contains("platform shell"));
        assert!(spec.description.contains("PowerShell 7 on Windows"));
        assert!(tool.prompt_snippet().unwrap().contains("platform shell"));
    }

    #[test]
    fn captured_text_decodes_utf8_and_bom_marked_utf16_without_ansi() {
        assert_eq!(decode_captured_text("中文".as_bytes()), "中文");

        let mut utf16le = vec![0xff, 0xfe];
        utf16le.extend("中文".encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(decode_captured_text(&utf16le), "中文");

        let mut utf16be = vec![0xfe, 0xff];
        utf16be.extend("中文".encode_utf16().flat_map(u16::to_be_bytes));
        assert_eq!(decode_captured_text(&utf16be), "中文");

        // GBK bytes for "中文" are deliberately not decoded through CP_ACP.
        let legacy_code_page = [0xd6, 0xd0, 0xce, 0xc4];
        let decoded = decode_captured_text(&legacy_code_page);
        assert_ne!(decoded, "中文");
        assert!(decoded.contains('\u{fffd}'));
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

    #[cfg(unix)]
    mod unix {
        use super::*;

        fn shell_quote(path: &std::path::Path) -> String {
            format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
        }

        #[tokio::test]
        async fn captures_stdout_and_runs_in_cwd() {
            let dir = tempfile::tempdir().unwrap();
            let ctx = ctx_at(dir.path());

            let result = run_dyn(&BashTool::new(), json!({"command": "echo hello"}), &ctx)
                .await
                .unwrap();
            assert!(!result.is_error);
            assert_eq!(text_of(&result), "hello\n");
            assert!(matches!(
                result.details.as_ref().unwrap()["shell"].as_str(),
                Some("/bin/bash" | "bash" | "sh")
            ));
        }

        #[tokio::test]
        async fn pwd_reflects_session_cwd() {
            let dir = tempfile::tempdir().unwrap();
            let ctx = ctx_at(dir.path());

            let result = run_dyn(&BashTool::new(), json!({"command": "pwd"}), &ctx)
                .await
                .unwrap();
            // macOS tempdirs are symlinked (/var → /private/var).
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
            .unwrap();
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
            assert!(elapsed < Duration::from_secs(10), "took {elapsed:?}");
            assert_eq!(result.details.unwrap()["timed_out"], true);
        }

        #[tokio::test]
        async fn timeout_kills_grandchildren_not_just_the_shell() {
            let dir = tempfile::tempdir().unwrap();
            let ctx = ctx_at(dir.path());
            let log = dir.path().join("beat.log");
            let log_arg = shell_quote(&log);

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
                json!({"command": "printf '%60000s' '' | tr ' ' x"}),
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
        async fn cancellation_token_aborts_before_spawning_the_command() {
            let dir = tempfile::tempdir().unwrap();
            let marker = dir.path().join("cancelled-command-ran");
            let cancel = CancellationToken::new();
            cancel.cancel();
            let ctx = ToolCtx::new(dir.path(), SessionId::from("s"), CallId::from("c"))
                .with_cancel(cancel);
            let command = format!("printf x > {}; sleep 30", shell_quote(&marker));

            let err = run_dyn(&BashTool::new(), json!({"command": command}), &ctx)
                .await
                .unwrap_err();
            assert!(matches!(err, ToolError::Execution(_)));
            assert!(err.to_string().contains("cancelled"), "{err}");
            assert!(!marker.exists(), "pre-cancelled command was started");
        }

        #[tokio::test]
        async fn per_call_timeout_overrides_default() {
            let dir = tempfile::tempdir().unwrap();
            let ctx = ctx_at(dir.path());

            let result = run_dyn(
                &BashTool::with_default_timeout(2),
                json!({"command": "sleep 4; echo done", "timeout_secs": 30}),
                &ctx,
            )
            .await
            .unwrap();
            assert!(!result.is_error, "{:?}", text_of(&result));
            assert_eq!(text_of(&result).trim(), "done");

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

        #[cfg(target_os = "linux")]
        #[tokio::test]
        async fn escaped_pipe_holder_keeps_group_leader_unreaped() {
            let dir = tempfile::tempdir().unwrap();
            let release = dir.path().join("release");
            let command = format!(
                "setsid sh -c 'while [ ! -e \"$1\" ]; do sleep 0.05; done' sh {} & exit 0",
                shell_quote(&release)
            );
            let shell::SpawnedShell {
                mut child,
                process_tree,
                ..
            } = shell::spawn(&command, dir.path()).await.unwrap();
            let saved_pid = child.id();
            let mut stdout_pipe = child.stdout.take().unwrap();
            let mut stderr_pipe = child.stderr.take().unwrap();
            let mut stdout_buf = Vec::new();
            let mut stderr_buf = Vec::new();

            let remained_pending = tokio::time::timeout(
                Duration::from_millis(300),
                collect_shell_output(
                    &mut child,
                    &mut stdout_pipe,
                    &mut stderr_pipe,
                    &mut stdout_buf,
                    &mut stderr_buf,
                ),
            )
            .await
            .is_err();
            let unreaped_pid = child.id();

            // Release the escaped helper before assertions so even a failing
            // regression does not leave a long-lived process behind.
            std::fs::write(&release, []).unwrap();
            process_tree.kill_and_reap(&mut child).await;
            drop(stdout_pipe);
            drop(stderr_pipe);

            assert!(
                remained_pending,
                "escaped descendant did not retain the pipes"
            );
            assert!(saved_pid.is_some());
            assert_eq!(
                unreaped_pid, saved_pid,
                "collection polled/reaped the leader before the pipe barrier"
            );
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

    #[cfg(windows)]
    mod windows {
        use super::*;

        fn path_pwsh_is_usable() -> bool {
            std::process::Command::new("pwsh.exe")
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "exit 0",
                ])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        }

        macro_rules! require_path_pwsh {
            ($result:expr) => {{
                if !path_pwsh_is_usable() {
                    eprintln!("skipping integration test: usable pwsh.exe is not on PATH");
                    return;
                }
                $result
            }};
        }

        fn powershell_quote(value: &str) -> String {
            format!("'{}'", value.replace('\'', "''"))
        }

        #[tokio::test]
        async fn captures_stdout_and_records_selected_shell() {
            let dir = tempfile::tempdir().unwrap();
            let ctx = ctx_at(dir.path());

            let result = require_path_pwsh!(
                run_dyn(
                    &BashTool::new(),
                    json!({"command": "Write-Output 'hello'"}),
                    &ctx,
                )
                .await
            )
            .unwrap();
            assert!(!result.is_error);
            assert_eq!(text_of(&result).trim(), "hello");
            assert!(matches!(
                result.details.as_ref().unwrap()["shell"].as_str(),
                Some("pwsh.exe")
            ));
        }

        #[tokio::test]
        async fn cwd_reflects_session_cwd() {
            let dir = tempfile::tempdir().unwrap();
            let ctx = ctx_at(dir.path());

            let result = require_path_pwsh!(
                run_dyn(
                    &BashTool::new(),
                    json!({"command": "Write-Output (Get-Location).Path"}),
                    &ctx,
                )
                .await
            )
            .unwrap();
            let printed = std::fs::canonicalize(text_of(&result).trim()).unwrap();
            let expected = std::fs::canonicalize(dir.path()).unwrap();
            assert_eq!(printed, expected);
        }

        #[tokio::test]
        async fn unicode_quotes_and_metacharacters_survive_without_requoting() {
            let dir = tempfile::tempdir().unwrap();
            let ctx = ctx_at(dir.path());

            let result = require_path_pwsh!(
                run_dyn(
                    &BashTool::new(),
                    json!({"command": "Write-Output '中文 ''quote'' & $()'"}),
                    &ctx,
                )
                .await
            )
            .unwrap();
            assert!(!result.is_error, "{}", text_of(&result));
            assert_eq!(text_of(&result).trim(), "中文 'quote' & $()");
        }

        #[tokio::test]
        async fn using_statement_remains_first_in_the_user_script() {
            let dir = tempfile::tempdir().unwrap();
            let ctx = ctx_at(dir.path());

            let result = require_path_pwsh!(
                run_dyn(
                    &BashTool::new(),
                    json!({
                        "command": "using namespace System.Text\nWrite-Output ([Encoding]::UTF8.WebName)"
                    }),
                    &ctx,
                )
                .await
            )
            .unwrap();
            assert!(!result.is_error, "{}", text_of(&result));
            assert_eq!(text_of(&result).trim(), "utf-8");
        }

        #[tokio::test]
        async fn constrained_language_runs_basic_cmdlets_without_a_launcher() {
            let dir = tempfile::tempdir().unwrap();
            let ctx = ctx_at(dir.path());

            let result = require_path_pwsh!(
                run_dyn(
                    &BashTool::new(),
                    json!({
                        "command": concat!(
                            "$ExecutionContext.SessionState.LanguageMode = ",
                            "'ConstrainedLanguage'\n",
                            "Write-Output $ExecutionContext.SessionState.LanguageMode\n",
                            "Get-Location | Select-Object -ExpandProperty Path\n",
                            "Write-Output 'restricted-basic-ok'"
                        )
                    }),
                    &ctx,
                )
                .await
            )
            .unwrap();
            let text = text_of(&result);
            assert!(!result.is_error, "{text}");
            assert!(text.contains("ConstrainedLanguage"), "{text}");
            assert!(text.contains("restricted-basic-ok"), "{text}");
            assert!(!text.contains("Cannot invoke method"), "{text}");
            assert!(!text.contains("Only core types are supported"), "{text}");
        }

        #[tokio::test]
        async fn non_zero_exit_is_error_result_not_tool_error() {
            let dir = tempfile::tempdir().unwrap();
            let ctx = ctx_at(dir.path());

            let result = require_path_pwsh!(
                run_dyn(
                    &BashTool::new(),
                    json!({"command": "Write-Output 'oops'; exit 3"}),
                    &ctx,
                )
                .await
            )
            .unwrap();
            assert!(result.is_error);
            assert!(text_of(&result).contains("oops"));
            assert!(text_of(&result).contains("[exit code: 3]"));
            assert_eq!(result.details.unwrap()["exit_code"], 3);
        }

        #[tokio::test]
        async fn utf8_stderr_is_captured_and_labelled() {
            let dir = tempfile::tempdir().unwrap();
            let ctx = ctx_at(dir.path());

            let result = require_path_pwsh!(
                run_dyn(
                    &BashTool::new(),
                    json!({"command": "Write-Error -Message '错误 err'; exit 0"}),
                    &ctx,
                )
                .await
            )
            .unwrap();
            let text = text_of(&result);
            assert!(text.contains("[stderr]"), "{text}");
            assert!(text.contains("错误 err"), "{text}");
            assert!(!result.is_error);
        }

        #[tokio::test]
        async fn timeout_kills_the_command() {
            let dir = tempfile::tempdir().unwrap();
            let ctx = ctx_at(dir.path());

            let started = Instant::now();
            let result = require_path_pwsh!(
                run_dyn(
                    &BashTool::new(),
                    json!({
                        "command": "Write-Output 'started'; Start-Sleep -Seconds 30",
                        "timeout_secs": 1,
                    }),
                    &ctx,
                )
                .await
            )
            .unwrap();
            let elapsed = started.elapsed();
            assert!(result.is_error);
            assert!(text_of(&result).contains("timed out after 1s"));
            assert!(elapsed < Duration::from_secs(15), "took {elapsed:?}");
            assert_eq!(result.details.unwrap()["timed_out"], true);
        }

        #[tokio::test]
        async fn timeout_terminates_job_members_after_the_shell_has_exited() {
            let dir = tempfile::tempdir().unwrap();
            let ctx = ctx_at(dir.path());
            let log = dir.path().join("detached-beat.log");
            let log_arg = powershell_quote(&log.to_string_lossy());
            let child_script = format!(
                "Stop-Process -Id ([int]$env:MCODE_TEST_PARENT_PID) -Force; \
                 for ($i = 0; $i -lt 300; $i++) {{ \
                 Add-Content -LiteralPath {log_arg} -Value x -Encoding utf8; \
                 Start-Sleep -Milliseconds 100 }}"
            );
            let child_encoded = crate::builtin::shell::encode_powershell_command(
                &child_script,
                std::path::Path::new("pwsh.exe"),
            )
            .expect("child command fits the encoded-command limit");
            let command = format!(
                "$env:MCODE_TEST_PARENT_PID = [string]$PID; \
                 & 'pwsh.exe' -NoLogo -NoProfile -NonInteractive \
                 -ExecutionPolicy Bypass -EncodedCommand '{child_encoded}'"
            );

            let started = Instant::now();
            let result = require_path_pwsh!(
                run_dyn(
                    &BashTool::new(),
                    json!({"command": command, "timeout_secs": 3}),
                    &ctx,
                )
                .await
            )
            .unwrap();
            let elapsed = started.elapsed();
            assert!(result.is_error);
            assert!(text_of(&result).contains("timed out after 3s"));
            assert!(elapsed < Duration::from_secs(12), "took {elapsed:?}");

            tokio::time::sleep(Duration::from_millis(800)).await;
            let first = std::fs::read_to_string(&log)
                .map(|s| s.lines().count())
                .unwrap_or(0);
            tokio::time::sleep(Duration::from_millis(800)).await;
            let second = std::fs::read_to_string(&log)
                .map(|s| s.lines().count())
                .unwrap_or(0);
            assert!(first > 0, "descendant never ran (log missing)");
            assert_eq!(first, second, "Job member survived after shell exit");
        }

        #[tokio::test]
        async fn timeout_kills_grandchild_process_tree() {
            let dir = tempfile::tempdir().unwrap();
            let ctx = ctx_at(dir.path());
            let log = dir.path().join("beat.log");
            let log_arg = powershell_quote(&log.to_string_lossy());
            let child_script = format!(
                "for ($i = 0; $i -lt 300; $i++) {{ \
                 Add-Content -LiteralPath {log_arg} -Value x -Encoding utf8; \
                 Start-Sleep -Milliseconds 100 }}"
            );
            let child_encoded = crate::builtin::shell::encode_powershell_command(
                &child_script,
                std::path::Path::new("pwsh.exe"),
            )
            .expect("child command fits the encoded-command limit");
            // Invoke the native child directly so it remains an ordinary
            // descendant; the outer PowerShell waits while the child writes.
            let command = format!(
                "& 'pwsh.exe' -NoLogo -NoProfile -NonInteractive \
                 -ExecutionPolicy Bypass -EncodedCommand '{child_encoded}'"
            );

            let result = require_path_pwsh!(
                run_dyn(
                    &BashTool::new(),
                    json!({"command": command, "timeout_secs": 5}),
                    &ctx,
                )
                .await
            )
            .unwrap();
            assert!(result.is_error);
            assert!(text_of(&result).contains("timed out after 5s"));

            tokio::time::sleep(Duration::from_millis(800)).await;
            let first = std::fs::read_to_string(&log)
                .map(|s| s.lines().count())
                .unwrap_or(0);
            tokio::time::sleep(Duration::from_millis(800)).await;
            let second = std::fs::read_to_string(&log)
                .map(|s| s.lines().count())
                .unwrap_or(0);
            assert!(first > 0, "grandchild never ran (log missing)");
            assert_eq!(first, second, "grandchild survived process-tree teardown");
        }

        #[tokio::test]
        async fn huge_output_is_truncated() {
            let dir = tempfile::tempdir().unwrap();
            let ctx = ctx_at(dir.path());

            let result = require_path_pwsh!(
                run_dyn(
                    &BashTool::new(),
                    json!({"command": "[Console]::Out.Write(('x' * 60000))"}),
                    &ctx,
                )
                .await
            )
            .unwrap();
            assert!(!result.is_error, "{}", text_of(&result));
            let text = text_of(&result);
            assert!(text.contains("[output truncated: showed first 51200 of 60000 bytes]"));
            assert!(text.len() < 60000);
            assert_eq!(result.details.unwrap()["truncated"], true);
        }

        #[tokio::test]
        async fn cancellation_token_aborts_before_spawning_or_provisioning() {
            let dir = tempfile::tempdir().unwrap();
            let marker = dir.path().join("cancelled-command-ran");
            let marker_arg = powershell_quote(&marker.to_string_lossy());
            let cancel = CancellationToken::new();
            cancel.cancel();
            let ctx = ToolCtx::new(dir.path(), SessionId::from("s"), CallId::from("c"))
                .with_cancel(cancel);
            let command =
                format!("Set-Content -LiteralPath {marker_arg} -Value x; Start-Sleep -Seconds 30");

            // No PATH prerequisite: cancellation must win before lookup or
            // managed PowerShell provisioning can begin.
            let err = run_dyn(&BashTool::new(), json!({"command": command}), &ctx)
                .await
                .unwrap_err();
            assert!(matches!(err, ToolError::Execution(_)));
            assert!(err.to_string().contains("cancelled"), "{err}");
            assert!(!marker.exists(), "pre-cancelled command was started");
        }

        #[tokio::test]
        async fn oversized_encoded_command_is_invalid_args() {
            let dir = tempfile::tempdir().unwrap();
            let ctx = ctx_at(dir.path());

            let err = require_path_pwsh!(
                run_dyn(
                    &BashTool::new(),
                    json!({"command": "界".repeat(20_000)}),
                    &ctx,
                )
                .await
            )
            .unwrap_err();
            assert!(matches!(err, ToolError::InvalidArgs(_)));
            assert!(err.to_string().contains("32,767 UTF-16-code-unit"), "{err}");
            assert!(err.to_string().contains("maximum for executable"), "{err}");
        }

        #[tokio::test]
        async fn empty_output_success() {
            let dir = tempfile::tempdir().unwrap();
            let ctx = ctx_at(dir.path());

            let result = require_path_pwsh!(
                run_dyn(&BashTool::new(), json!({"command": "$null"}), &ctx).await
            )
            .unwrap();
            assert!(!result.is_error);
            assert_eq!(text_of(&result), "");
        }
    }
}
