//! `write` — write a full file, creating parent directories.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::ctx::ToolCtx;
use crate::stream::ToolStream;
use crate::tool::{Tool, ToolError, ToolResult};

/// The `write` builtin.
pub struct WriteTool;

/// Arguments for [`WriteTool`].
#[derive(Debug, Deserialize, JsonSchema)]
pub struct WriteArgs {
    /// Path of the file to write. Relative paths resolve against the
    /// session cwd; missing parent directories are created.
    pub path: String,
    /// The complete new content of the file (replaces any existing
    /// content).
    pub content: String,
}

#[async_trait]
impl Tool for WriteTool {
    type Args = WriteArgs;
    type Output = ();

    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write a text file to the local filesystem, creating parent \
         directories as needed. Replaces the file's entire content."
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
        let path = ctx.resolve(&args.path);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await.map_err(|err| {
                    ToolError::Execution(format!(
                        "failed to create directory {}: {err}",
                        parent.display()
                    ))
                })?;
            }
        }

        let bytes = args.content.len();
        tokio::fs::write(&path, &args.content)
            .await
            .map_err(|err| {
                ToolError::Execution(format!("failed to write {}: {err}", path.display()))
            })?;

        Ok(
            ToolResult::text(format!("Wrote {bytes} bytes to {}", path.display())).with_details(
                json!({
                    "path": path.display().to_string(),
                    "bytes_written": bytes,
                }),
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::test_support::{ctx_at, run_dyn, text_of};
    use crate::tool::{Concurrency, ToolDyn};
    use serde_json::json;

    #[tokio::test]
    async fn writes_file_and_creates_missing_parents() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());

        let result = run_dyn(
            &WriteTool,
            json!({"path": "deep/nested/new.txt", "content": "hello mcode"}),
            &ctx,
        )
        .await
        .unwrap();
        assert!(!result.is_error);

        let on_disk = std::fs::read_to_string(dir.path().join("deep/nested/new.txt")).unwrap();
        assert_eq!(on_disk, "hello mcode");
        assert!(
            text_of(&result).starts_with("Wrote 11 bytes to "),
            "{}",
            text_of(&result)
        );
        assert_eq!(
            result.details.unwrap()["bytes_written"],
            "hello mcode".len()
        );
    }

    #[tokio::test]
    async fn overwrites_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("old.txt"), "previous content").unwrap();
        let ctx = ctx_at(dir.path());

        run_dyn(
            &WriteTool,
            json!({"path": "old.txt", "content": "new"}),
            &ctx,
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("old.txt")).unwrap(),
            "new"
        );
    }

    #[tokio::test]
    async fn empty_content_writes_zero_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());

        let result = run_dyn(
            &WriteTool,
            json!({"path": "empty.txt", "content": ""}),
            &ctx,
        )
        .await
        .unwrap();
        assert_eq!(result.details.unwrap()["bytes_written"], 0);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("empty.txt")).unwrap(),
            ""
        );
    }

    #[tokio::test]
    async fn parent_is_a_regular_file_fails() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("blocker"), "file").unwrap();
        let ctx = ctx_at(dir.path());

        // create_dir_all("blocker/sub") must fail since blocker is a file.
        let err = run_dyn(
            &WriteTool,
            json!({"path": "blocker/sub/x.txt", "content": "x"}),
            &ctx,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::Execution(_)));
    }

    #[test]
    fn capability_markers() {
        let tool: &dyn ToolDyn = &WriteTool;
        assert!(tool.mutates_fs());
        assert_eq!(tool.concurrency(), Concurrency::Parallel);
    }
}
