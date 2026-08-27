// Rust guideline compliant 2026-08-27.

use super::*;
use crate::builtin::test_support::{ctx_at, run_dyn, text_of};
use crate::ctx::ToolCtx;
use crate::tool::ToolDyn;
use mcode_core::ids::{CallId, SessionId};
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn assert_truncated_counts(result: &ToolResult, stdout_bytes: u64, stderr_bytes: u64) {
    assert!(!result.is_error, "{}", text_of(result));
    let text = text_of(result);
    let total = stdout_bytes.saturating_add(stderr_bytes);
    assert!(
        text.contains(&format!(
            "[output truncated: showed first {MAX_OUTPUT_BYTES} of {total} bytes]"
        )),
        "{text}"
    );
    let details = result.details.as_ref().unwrap();
    assert_eq!(details["truncated"], true);
    assert_eq!(details["stdout_bytes"], stdout_bytes);
    assert_eq!(details["stderr_bytes"], stderr_bytes);
}

fn repeated_encoded_ascii(prefix: &[u8], encoded_x: &[u8]) -> Vec<u8> {
    let count = MAX_OUTPUT_BYTES + 1;
    let mut output = Vec::with_capacity(prefix.len() + encoded_x.len() * count);
    output.extend_from_slice(prefix);
    for _ in 0..count {
        output.extend_from_slice(encoded_x);
    }
    output
}

async fn assert_encoded_ascii_truncates(output: Vec<u8>) {
    let total_bytes = u64::try_from(output.len()).unwrap();
    let mut pipe = Some(std::io::Cursor::new(output));
    let mut stdout = CapturedStream::new();

    read_bounded(&mut pipe, &mut stdout).await.unwrap();

    assert!(pipe.is_none());
    let result = format_result(
        None::<std::process::ExitStatus>,
        "fixture",
        "fixture",
        stdout,
        CapturedStream::default(),
        0,
        false,
        None,
    );
    assert_truncated_counts(&result, total_bytes, 0);
    let text = text_of(&result);
    assert!(text.starts_with(&"x".repeat(MAX_OUTPUT_BYTES)), "{text}");
    assert!(!text.contains('\u{fffd}'), "{text}");
}

#[tokio::test]
async fn bounded_reader_drains_all_bytes_and_retains_only_render_prefix() {
    let produced = 5 * 1024 * 1024;
    let mut pipe = Some(std::io::Cursor::new(vec![b'x'; produced]));
    let mut captured = CapturedStream::new();

    read_bounded(&mut pipe, &mut captured).await.unwrap();

    assert!(pipe.is_none());
    assert_eq!(captured.total_bytes, produced as u64);
    assert_eq!(captured.retained.len(), MAX_RETAINED_OUTPUT_BYTES);
    assert!(captured.retained.iter().all(|byte| *byte == b'x'));
}

#[tokio::test]
async fn bounded_reader_preserves_utf8_bom_past_render_limit() {
    let output = repeated_encoded_ascii(&[0xef, 0xbb, 0xbf], b"x");
    assert_encoded_ascii_truncates(output).await;
}

#[tokio::test]
async fn bounded_reader_preserves_utf16_prefix_past_render_limit() {
    for (bom, encoded_x) in [([0xff, 0xfe], [b'x', 0]), ([0xfe, 0xff], [0, b'x'])] {
        let output = repeated_encoded_ascii(&bom, &encoded_x);
        assert_encoded_ascii_truncates(output).await;
    }
}

#[test]
fn collection_error_reports_teardown_failure() {
    let collection = std::io::Error::other("pipe read failed");
    let teardown = std::io::Error::other("kill denied");
    let combined = collection_error(&collection, Some(&teardown)).to_string();
    assert!(combined.contains("pipe read failed"), "{combined}");
    assert!(
        combined.contains("termination failed: kill denied"),
        "{combined}"
    );

    let collection_only = collection_error(&collection, None).to_string();
    assert!(
        collection_only.contains("pipe read failed"),
        "{collection_only}"
    );
    assert!(
        !collection_only.contains("termination failed"),
        "{collection_only}"
    );
}

#[test]
fn timeout_notice_does_not_claim_kill_when_teardown_failed() {
    let failed = timeout_notice(
        Duration::from_secs(1),
        Some(&std::io::Error::other("job terminate denied")),
    );
    assert!(failed.contains("timed out after 1s"), "{failed}");
    assert!(failed.contains("termination failed"), "{failed}");
    assert!(failed.contains("job terminate denied"), "{failed}");
    assert!(!failed.contains("was killed"), "{failed}");

    let killed = timeout_notice(Duration::from_secs(2), None);
    assert_eq!(killed, "[command timed out after 2s and was killed]");
}

#[test]
fn cancelled_error_reports_teardown_failure() {
    let err = command_cancelled_error_from(Some(std::io::Error::other("sigkill denied")));
    let text = err.to_string();
    assert!(text.contains("cancelled before completion"), "{text}");
    assert!(text.contains("termination failed"), "{text}");
    assert!(text.contains("sigkill denied"), "{text}");
    assert!(
        !command_cancelled_error()
            .to_string()
            .contains("termination failed")
    );
}

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
    assert!(
        !err.to_string().contains("/definitely/not/a/dir"),
        "{}",
        err.to_string()
    );
}

#[tokio::test]
async fn spawn_cwd_error_uses_relative_spelling_not_host_path() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing-cwd");
    let ctx = ToolCtx::new(&missing, SessionId::from("s"), CallId::from("c"));
    let err = run_dyn(&BashTool::new(), json!({"command": "echo hi"}), &ctx)
        .await
        .unwrap_err();
    let msg = err.to_string();
    let abs = missing.to_string_lossy();
    let parent = dir.path().to_string_lossy();
    assert!(!msg.contains(abs.as_ref()), "{msg}");
    assert!(!msg.contains(parent.as_ref()), "{msg}");
    assert!(msg.contains("working directory ."), "{msg}");
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
        assert_truncated_counts(&result, 60_000, 0);
        assert!(text_of(&result).len() < 60_000);
    }

    #[tokio::test]
    async fn five_mib_stdout_and_stderr_stay_bounded_and_complete() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());
        let started = Instant::now();
        let result = run_dyn(
            &BashTool::new(),
            json!({
                "command": concat!(
                    "dd if=/dev/zero bs=65536 count=80 2>/dev/null; ",
                    "dd if=/dev/zero bs=65536 count=80 2>/dev/null | cat >&2"
                )
            }),
            &ctx,
        )
        .await
        .unwrap();
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(20),
            "bounded capture took {elapsed:?}"
        );
        assert_truncated_counts(&result, 5 * 1024 * 1024, 5 * 1024 * 1024);
    }

    #[tokio::test]
    async fn cancellation_token_aborts_before_spawning_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("cancelled-command-ran");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let ctx =
            ToolCtx::new(dir.path(), SessionId::from("s"), CallId::from("c")).with_cancel(cancel);
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
        let mut stdout_pipe = Some(child.stdout.take().unwrap());
        let mut stderr_pipe = Some(child.stderr.take().unwrap());
        let mut stdout = CapturedStream::new();
        let mut stderr = CapturedStream::new();

        let remained_pending = tokio::time::timeout(
            Duration::from_millis(300),
            collect_shell_output(
                &mut child,
                &mut stdout_pipe,
                &mut stderr_pipe,
                &mut stdout,
                &mut stderr,
            ),
        )
        .await
        .is_err();
        let unreaped_pid = child.id();

        // Release the escaped helper before assertions so even a failing
        // regression does not leave a long-lived process behind.
        std::fs::write(&release, []).unwrap();
        let _ = process_tree.kill_and_reap(&mut child).await;
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
        assert_truncated_counts(&result, 60_000, 0);
        assert!(text_of(&result).len() < 60_000);
    }

    #[tokio::test]
    async fn five_mib_stdout_and_stderr_stay_bounded_and_complete() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());
        let started = Instant::now();
        let result = require_path_pwsh!(
            run_dyn(
                &BashTool::new(),
                json!({
                    "command": concat!(
                        "[Console]::Out.Write(('x' * 5242880)); ",
                        "[Console]::Error.Write(('y' * 5242880))"
                    )
                }),
                &ctx,
            )
            .await
        )
        .unwrap();
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(20),
            "bounded capture took {elapsed:?}"
        );
        assert_truncated_counts(&result, 5 * 1024 * 1024, 5 * 1024 * 1024);
    }

    #[tokio::test]
    async fn cancellation_token_aborts_before_spawning_or_provisioning() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("cancelled-command-ran");
        let marker_arg = powershell_quote(&marker.to_string_lossy());
        let cancel = CancellationToken::new();
        cancel.cancel();
        let ctx =
            ToolCtx::new(dir.path(), SessionId::from("s"), CallId::from("c")).with_cancel(cancel);
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

        let result =
            require_path_pwsh!(run_dyn(&BashTool::new(), json!({"command": "$null"}), &ctx).await)
                .unwrap();
        assert!(!result.is_error);
        assert_eq!(text_of(&result), "");
    }
}
