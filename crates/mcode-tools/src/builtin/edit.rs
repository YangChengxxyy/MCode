//! `edit` — replace a unique string in a file (M1 simplification of the
//! hashline-anchor design; see `07-m1-plan.md` T3: 唯一字符串, 失败要求
//! 更多上下文).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::ctx::ToolCtx;
use crate::stream::ToolStream;
use crate::tool::{Tool, ToolError, ToolResult};

/// The `edit` builtin.
pub struct EditTool;

/// Arguments for [`EditTool`].
#[derive(Debug, Deserialize, JsonSchema)]
pub struct EditArgs {
    /// Path of the file to edit. Relative paths resolve against the
    /// session cwd.
    pub path: String,
    /// Exact text to replace. Must occur **exactly once** in the file —
    /// include surrounding lines to disambiguate.
    pub old_string: String,
    /// Replacement text.
    pub new_string: String,
}

#[async_trait]
impl Tool for EditTool {
    type Args = EditArgs;
    type Output = ();

    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Replace text in a file. `old_string` must match exactly once; if it \
         is absent or ambiguous the tool errors and asks for more surrounding \
         context."
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
        if args.old_string.is_empty() {
            return Err(ToolError::InvalidArgs(
                "old_string must not be empty; provide the text to replace".into(),
            ));
        }
        let path = ctx.resolve(&args.path);
        let content = tokio::fs::read_to_string(&path).await.map_err(|err| {
            ToolError::Execution(format!("failed to read {}: {err}", path.display()))
        })?;

        let occurrences = content.matches(&args.old_string).count();
        match occurrences {
            0 => Err(ToolError::Execution(format!(
                "old_string not found in {}; re-read the file and provide the exact text",
                path.display()
            ))),
            1 => {
                let edited = content.replacen(&args.old_string, &args.new_string, 1);
                tokio::fs::write(&path, &edited).await.map_err(|err| {
                    ToolError::Execution(format!("failed to write {}: {err}", path.display()))
                })?;
                Ok(ToolResult::text(format!(
                    "Edited {}: replaced 1 occurrence ({} → {} bytes)",
                    path.display(),
                    args.old_string.len(),
                    args.new_string.len(),
                ))
                .with_details(json!({
                    "path": path.display().to_string(),
                    "replacements": 1,
                    "bytes_before": content.len(),
                    "bytes_after": edited.len(),
                })))
            }
            n => Err(ToolError::Execution(format!(
                "old_string occurs {n} times in {}; include more surrounding lines to make it unique",
                path.display()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::test_support::{ctx_at, run_dyn};
    use serde_json::json;

    #[tokio::test]
    async fn replaces_unique_string() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("code.rs");
        std::fs::write(&file, "fn main() {\n    println!(\"hello\");\n}\n").unwrap();
        let ctx = ctx_at(dir.path());

        let result = run_dyn(
            &EditTool,
            json!({
                "path": "code.rs",
                "old_string": "println!(\"hello\");",
                "new_string": "println!(\"goodbye\");",
            }),
            &ctx,
        )
        .await
        .unwrap();
        assert!(!result.is_error);
        assert!(run_dyn_txt(&result).starts_with("Edited"));

        let on_disk = std::fs::read_to_string(&file).unwrap();
        assert_eq!(on_disk, "fn main() {\n    println!(\"goodbye\");\n}\n");
    }

    #[tokio::test]
    async fn missing_string_errors_with_guidance() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "aaa\nbbb\n").unwrap();
        let ctx = ctx_at(dir.path());

        let err = run_dyn(
            &EditTool,
            json!({"path": "f.txt", "old_string": "zzz", "new_string": "y"}),
            &ctx,
        )
        .await
        .unwrap_err();
        match err {
            ToolError::Execution(msg) => {
                assert!(msg.contains("not found"), "{msg}");
                assert!(msg.contains("f.txt"), "{msg}");
            }
            other => panic!("expected Execution, got {other:?}"),
        }
        // File untouched.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("f.txt")).unwrap(),
            "aaa\nbbb\n"
        );
    }

    #[tokio::test]
    async fn ambiguous_string_errors_asking_for_context() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "x\n  item\ny\n  item\n").unwrap();
        let ctx = ctx_at(dir.path());

        let err = run_dyn(
            &EditTool,
            json!({"path": "f.txt", "old_string": "item", "new_string": "thing"}),
            &ctx,
        )
        .await
        .unwrap_err();
        match err {
            ToolError::Execution(msg) => {
                assert!(msg.contains("2 times"), "{msg}");
                assert!(msg.contains("unique"), "{msg}");
            }
            other => panic!("expected Execution, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn context_disambiguates() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "x\n  item\ny\n  item\n").unwrap();
        let ctx = ctx_at(dir.path());

        // Including the preceding line makes the match unique.
        let result = run_dyn(
            &EditTool,
            json!({
                "path": "f.txt",
                "old_string": "x\n  item",
                "new_string": "x\n  thing",
            }),
            &ctx,
        )
        .await
        .unwrap();
        assert!(!result.is_error);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("f.txt")).unwrap(),
            "x\n  thing\ny\n  item\n"
        );
    }

    #[tokio::test]
    async fn missing_file_is_an_execution_error() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());

        let err = run_dyn(
            &EditTool,
            json!({"path": "absent.txt", "old_string": "a", "new_string": "b"}),
            &ctx,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::Execution(_)));
    }

    #[tokio::test]
    async fn empty_old_string_is_invalid_args() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "content").unwrap();
        let ctx = ctx_at(dir.path());

        let err = run_dyn(
            &EditTool,
            json!({"path": "f.txt", "old_string": "", "new_string": "b"}),
            &ctx,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn multi_byte_strings_count_bytes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("utf8.txt"), "héllo wörld").unwrap();
        let ctx = ctx_at(dir.path());

        let result = run_dyn(
            &EditTool,
            json!({
                "path": "utf8.txt",
                "old_string": "héllo",
                "new_string": "hola",
            }),
            &ctx,
        )
        .await
        .unwrap();
        let details = result.details.unwrap();
        assert_eq!(details["bytes_before"], "héllo wörld".len());
        assert_eq!(details["bytes_after"], "hola wörld".len());
    }

    fn run_dyn_txt(result: &ToolResult) -> &str {
        match result.content.as_slice() {
            [mcode_core::message::ContentBlock::Text(text)] => &text.text,
            other => panic!("expected single text block, got {other:?}"),
        }
    }
}
