//! `read` — read a UTF-8 text file with optional line windowing and
//! output truncation (pi-style: ~2000 lines / 50 KiB with a notice).

use std::path::Path;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::ctx::ToolCtx;
use crate::stream::ToolStream;
use crate::tool::{Tool, ToolError, ToolResult};

/// Maximum lines returned in one call.
pub const MAX_LINES: usize = 2000;
/// Maximum bytes returned in one call.
pub const MAX_BYTES: usize = 50 * 1024;

/// The `read` builtin.
pub struct ReadTool;

/// Arguments for [`ReadTool`].
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadArgs {
    /// Path of the file to read. Relative paths resolve against the
    /// session cwd.
    pub path: String,
    /// 1-based line number to start reading from (default: the first
    /// line).
    pub offset: Option<usize>,
    /// Maximum number of lines to return (default: all, up to the
    /// tool's output cap).
    pub limit: Option<usize>,
}

#[async_trait]
impl Tool for ReadTool {
    type Args = ReadArgs;
    type Output = ();

    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read a UTF-8 text file from the local filesystem. Use offset/limit to \
         window into large files; output is truncated with a notice beyond \
         2000 lines or 50 KiB per call."
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some("read: fetch file contents (path, optional 1-based offset / limit).")
    }

    async fn execute(
        &self,
        args: Self::Args,
        ctx: &ToolCtx,
        _out: &mut ToolStream,
    ) -> Result<ToolResult, ToolError> {
        let path = ctx.resolve(&args.path);
        let content = tokio::fs::read_to_string(&path).await.map_err(|err| {
            ToolError::Execution(format!("failed to read {}: {err}", path.display()))
        })?;

        let total_lines = content.lines().count();
        let start = args.offset.unwrap_or(1).saturating_sub(1).min(total_lines);
        let end = match args.limit {
            Some(limit) => (start + limit).min(total_lines),
            None => total_lines,
        };

        // Tool-level caps (distinct from the user-requested window):
        // at most MAX_LINES lines and MAX_BYTES bytes per call, with a
        // notice telling the model how to read the rest.
        let capped_end = end.min(start + MAX_LINES);
        let selected: Vec<&str> = content
            .lines()
            .skip(start)
            .take(capped_end - start)
            .collect();
        let text = selected.join("\n");
        let (mut text, byte_truncated) = crate::builtin::truncate_bytes(&text, MAX_BYTES);
        // Tool caps only: a user-requested offset/limit window is *not*
        // truncation — the model asked for exactly those lines.
        let truncated = byte_truncated || capped_end < end;

        if truncated {
            text.push_str(&format!(
                "\n[output truncated: showing lines {}-{} of {total_lines}; re-invoke with offset/limit to read more]",
                start + 1,
                start + selected.len(),
            ));
        }

        Ok(ToolResult::text(text).with_details(json!({
            "path": display(&path),
            "total_lines": total_lines,
            "returned_lines": selected.len(),
            "truncated": truncated,
        })))
    }
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::test_support::{ctx_at, run_dyn, text_of};
    use serde_json::json;

    #[tokio::test]
    async fn reads_small_file_completely() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let ctx = ctx_at(dir.path());

        let result = run_dyn(&ReadTool, json!({"path": "notes.txt"}), &ctx)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(text_of(&result), "alpha\nbeta\ngamma\n".trim_end());
        assert_eq!(result.details.unwrap()["total_lines"], 3);
    }

    #[tokio::test]
    async fn missing_file_is_an_execution_error() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());

        let err = run_dyn(&ReadTool, json!({"path": "nope.txt"}), &ctx)
            .await
            .unwrap_err();
        match err {
            ToolError::Execution(msg) => {
                assert!(msg.contains("nope.txt"), "{msg}");
                assert!(msg.contains("failed to read"), "{msg}");
            }
            other => panic!("expected Execution, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn non_utf8_file_is_an_execution_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("blob.bin"), b"\xff\xfe\x00binary").unwrap();
        let ctx = ctx_at(dir.path());

        let err = run_dyn(&ReadTool, json!({"path": "blob.bin"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Execution(_)));
    }

    #[tokio::test]
    async fn offset_and_limit_window_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let body: Vec<String> = (1..=100).map(|i| format!("line-{i}")).collect();
        std::fs::write(dir.path().join("num.txt"), body.join("\n")).unwrap();
        let ctx = ctx_at(dir.path());

        let result = run_dyn(
            &ReadTool,
            json!({"path": "num.txt", "offset": 10, "limit": 3}),
            &ctx,
        )
        .await
        .unwrap();
        // A user-requested window is not "truncated" — no notice.
        assert_eq!(text_of(&result), "line-10\nline-11\nline-12");
        assert_eq!(result.details.unwrap()["truncated"], false);
    }

    #[tokio::test]
    async fn offset_beyond_eof_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tiny.txt"), "one line\n").unwrap();
        let ctx = ctx_at(dir.path());

        let result = run_dyn(&ReadTool, json!({"path": "tiny.txt", "offset": 999}), &ctx)
            .await
            .unwrap();
        assert_eq!(text_of(&result), "");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn line_cap_truncates_with_notice() {
        let dir = tempfile::tempdir().unwrap();
        let body: Vec<String> = (1..=2500).map(|i| format!("{i}")).collect();
        std::fs::write(dir.path().join("big.txt"), body.join("\n")).unwrap();
        let ctx = ctx_at(dir.path());

        let result = run_dyn(&ReadTool, json!({"path": "big.txt"}), &ctx)
            .await
            .unwrap();
        let text = text_of(&result);
        let returned = text.lines().count();
        assert_eq!(returned, MAX_LINES + 1, "content + notice line");
        assert!(
            text.contains("[output truncated: showing lines 1-2000 of 2500"),
            "{text}"
        );

        let details = result.details.unwrap();
        assert_eq!(details["truncated"], true);
        assert_eq!(details["total_lines"], 2500);
        assert_eq!(details["returned_lines"], MAX_LINES);

        // Following the notice's advice yields the next window.
        let result = run_dyn(
            &ReadTool,
            json!({"path": "big.txt", "offset": 2001, "limit": 2}),
            &ctx,
        )
        .await
        .unwrap();
        assert_eq!(text_of(&result), "2001\n2002");
    }

    #[tokio::test]
    async fn byte_cap_truncates_single_huge_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("wide.txt"), "x".repeat(60 * 1024)).unwrap();
        let ctx = ctx_at(dir.path());

        let result = run_dyn(&ReadTool, json!({"path": "wide.txt"}), &ctx)
            .await
            .unwrap();
        let text = text_of(&result);
        assert!(text.len() > MAX_BYTES, "notice is appended");
        assert!(text.starts_with(&"x".repeat(MAX_BYTES)));
        assert!(text.contains("[output truncated"));
        assert_eq!(result.details.unwrap()["truncated"], true);
    }

    #[tokio::test]
    async fn empty_file_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("empty.txt"), "").unwrap();
        let ctx = ctx_at(dir.path());

        let result = run_dyn(&ReadTool, json!({"path": "empty.txt"}), &ctx)
            .await
            .unwrap();
        assert_eq!(text_of(&result), "");
        assert_eq!(result.details.unwrap()["total_lines"], 0);
    }
}
