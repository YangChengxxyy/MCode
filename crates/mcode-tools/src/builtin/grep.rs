//! `grep` — in-process content search over the session cwd (regex or
//! literal), with include/exclude globs and a result cap. Implemented
//! with `regex` + the `ignore` walker (respects `.gitignore`, skips
//! hidden files); never shells out to `rg`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use globset::GlobMatcher;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::ctx::ToolCtx;
use crate::stream::ToolStream;
use crate::tool::{Tool, ToolError, ToolResult};

/// Default cap on reported matching lines.
pub const MAX_MATCHES: usize = 200;

/// The `grep` builtin.
pub struct GrepTool;

/// Arguments for [`GrepTool`].
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GrepArgs {
    /// Pattern to search for: literal text by default, or a regular
    /// expression when `is_regex` is set.
    pub pattern: String,
    /// Interpret `pattern` as a regular expression.
    #[serde(default)]
    pub is_regex: bool,
    /// File or directory to search (relative → session cwd). Defaults to
    /// the whole cwd.
    pub path: Option<String>,
    /// Only search files whose path (relative to the search root)
    /// matches this glob, e.g. `"*.rs"` (matches nested paths).
    pub include: Option<String>,
    /// Skip files whose path matches this glob.
    pub exclude: Option<String>,
    /// Maximum number of matching lines to report (default 200).
    pub max_results: Option<usize>,
}

/// One matching line, in output order (sorted by path, then line).
struct LineMatch {
    rel_path: String,
    line_no: usize,
    line: String,
}

#[async_trait]
impl Tool for GrepTool {
    type Args = GrepArgs;
    type Output = ();

    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents under a directory (or the session cwd). Literal \
         text by default, regex with is_regex; optional include/exclude globs. \
         Reports up to 200 matching lines as `path:line:text`, with a notice \
         when more matches exist. Hidden and gitignored files are skipped."
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some("grep: search file contents (pattern, optional is_regex/path/include/exclude).")
    }

    async fn execute(
        &self,
        args: Self::Args,
        ctx: &ToolCtx,
        _out: &mut ToolStream,
    ) -> Result<ToolResult, ToolError> {
        // Literal patterns are escaped into a regex so one matcher drives
        // both modes.
        let pattern = if args.is_regex {
            args.pattern.clone()
        } else {
            regex::escape(&args.pattern)
        };
        let re = regex::Regex::new(&pattern)
            .map_err(|err| ToolError::InvalidArgs(format!("invalid regex: {err}")))?;

        let include = compile_glob(args.include.as_deref(), "include")?;
        let exclude = compile_glob(args.exclude.as_deref(), "exclude")?;

        let root = match &args.path {
            Some(p) => ctx.resolve(p),
            None => ctx.cwd.clone(),
        };
        // A nonexistent root must be an error, not an empty success —
        // otherwise the model can't distinguish a typo'd path from a
        // search that matched nothing (`read` errors on missing files
        // too). Covers both directory and single-file targets.
        if !root.exists() {
            return Err(ToolError::Execution(format!(
                "search path does not exist: {}",
                root.display()
            )));
        }
        let cap = args.max_results.unwrap_or(MAX_MATCHES);

        let files: Vec<PathBuf> = if root.is_file() {
            vec![root.clone()]
        } else {
            // Single-threaded deterministic walk; hidden files and
            // gitignored paths are skipped by the ignore crate.
            ignore::Walk::new(&root)
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.file_type().is_some_and(|ft| ft.is_file()))
                .map(|entry| entry.into_path())
                .collect()
        };

        let mut matches: Vec<LineMatch> = Vec::new();
        let mut total_matches = 0usize;
        let mut files_searched = 0usize;

        for file in &files {
            let rel_path = match file.strip_prefix(&root) {
                Ok(rel) if !rel.as_os_str().is_empty() => stable_path(rel),
                _ => stable_path(file),
            };
            if let Some(inc) = &include
                && !inc.is_match(&rel_path)
            {
                continue;
            }
            if let Some(exc) = &exclude
                && exc.is_match(&rel_path)
            {
                continue;
            }

            // Non-UTF-8 files are treated as binary and skipped.
            let Ok(content) = tokio::fs::read_to_string(file).await else {
                continue;
            };
            files_searched += 1;

            for (idx, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    total_matches += 1;
                    if matches.len() < cap {
                        matches.push(LineMatch {
                            rel_path: rel_path.clone(),
                            line_no: idx + 1,
                            line: line.to_owned(),
                        });
                    }
                }
            }
        }

        matches.sort_by(|a, b| a.rel_path.cmp(&b.rel_path).then(a.line_no.cmp(&b.line_no)));
        let truncated = total_matches > matches.len();

        let mut text = matches
            .iter()
            .map(|m| format!("{}:{}:{}", m.rel_path, m.line_no, m.line))
            .collect::<Vec<_>>()
            .join("\n");
        if truncated {
            text.push_str(&format!(
                "\n[showing first {} of {total_matches} matching lines; narrow the pattern or raise max_results]",
                matches.len()
            ));
        }

        Ok(ToolResult::text(text).with_details(json!({
            "root": root.display().to_string(),
            "files_searched": files_searched,
            "matches": total_matches,
            "shown": matches.len(),
            "truncated": truncated,
        })))
    }
}

/// Renders paths with stable separators for matching and model-visible output.
fn stable_path(path: &Path) -> String {
    let rendered = path.to_string_lossy().into_owned();
    #[cfg(windows)]
    {
        rendered.replace('\\', "/")
    }
    #[cfg(not(windows))]
    {
        rendered
    }
}

/// Compile an optional glob filter; malformed globs fail fast (unlike
/// permission-rule patterns, these come straight from the model and the
/// feedback loop helps).
fn compile_glob(glob: Option<&str>, what: &str) -> Result<Option<GlobMatcher>, ToolError> {
    match glob {
        None => Ok(None),
        Some(pattern) => globset::Glob::new(pattern)
            .map(|g| Some(g.compile_matcher()))
            .map_err(|err| {
                ToolError::InvalidArgs(format!("invalid {what} glob `{pattern}`: {err}"))
            }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::test_support::{ctx_at, run_dyn, text_of};
    use serde_json::json;

    fn fixture(dir: &Path) {
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {\n    // hello\n}\n").unwrap();
        std::fs::write(
            dir.join("src/util.rs"),
            "// hello from util\npub fn x() {}\n",
        )
        .unwrap();
        std::fs::write(dir.join("docs/notes.md"), "# notes\nhello world\n").unwrap();
        std::fs::write(dir.join("binary.bin"), b"\xff\xfe\x00hello").unwrap();
    }

    #[tokio::test]
    async fn literal_search_finds_matches_with_rel_paths() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        let ctx = ctx_at(dir.path());

        let result = run_dyn(&GrepTool, json!({"pattern": "hello"}), &ctx)
            .await
            .unwrap();
        let text = text_of(&result);
        assert!(text.contains("docs/notes.md:2:hello world"), "{text}");
        assert!(text.contains("src/main.rs:2:    // hello"), "{text}");
        assert!(text.contains("src/util.rs:1:// hello from util"), "{text}");
        // The binary file is skipped silently.
        assert!(!text.contains("binary.bin"), "{text}");
        assert!(!result.is_error);

        let details = result.details.unwrap();
        assert_eq!(details["matches"], 3);
        assert_eq!(details["files_searched"], 3);
    }

    #[tokio::test]
    async fn literal_patterns_do_not_act_as_regex() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        let ctx = ctx_at(dir.path());

        // "h.llo" as a literal must not match "hello".
        let result = run_dyn(&GrepTool, json!({"pattern": "h.llo"}), &ctx)
            .await
            .unwrap();
        assert_eq!(text_of(&result), "");
        assert_eq!(result.details.unwrap()["matches"], 0);
    }

    #[tokio::test]
    async fn regex_mode_matches_patterns() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        let ctx = ctx_at(dir.path());

        let result = run_dyn(
            &GrepTool,
            json!({"pattern": "hello (world|from)", "is_regex": true}),
            &ctx,
        )
        .await
        .unwrap();
        let text = text_of(&result);
        assert!(text.contains("hello world"), "{text}");
        assert!(text.contains("hello from"), "{text}");
    }

    #[tokio::test]
    async fn invalid_regex_is_invalid_args() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());

        let err = run_dyn(
            &GrepTool,
            json!({"pattern": "(unclosed", "is_regex": true}),
            &ctx,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn include_glob_filters_to_matching_files() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        let ctx = ctx_at(dir.path());

        let result = run_dyn(
            &GrepTool,
            json!({"pattern": "hello", "include": "*.rs"}),
            &ctx,
        )
        .await
        .unwrap();
        let text = text_of(&result);
        // "*.rs" matches nested paths too (globset `*` crosses `/`).
        assert!(text.contains("src/main.rs"), "{text}");
        assert!(text.contains("src/util.rs"), "{text}");
        assert!(!text.contains("notes.md"), "{text}");
    }

    #[tokio::test]
    async fn exclude_glob_skips_matching_files() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        let ctx = ctx_at(dir.path());

        let result = run_dyn(
            &GrepTool,
            json!({"pattern": "hello", "exclude": "*.md"}),
            &ctx,
        )
        .await
        .unwrap();
        let text = text_of(&result);
        assert!(!text.contains("notes.md"), "{text}");
        assert!(text.contains("src/util.rs"), "{text}");
    }

    #[tokio::test]
    async fn result_cap_reports_total_with_notice() {
        let dir = tempfile::tempdir().unwrap();
        let many: Vec<String> = (1..=205).map(|i| format!("hit {i}")).collect();
        std::fs::write(dir.path().join("many.txt"), many.join("\n")).unwrap();
        let ctx = ctx_at(dir.path());

        // Default cap.
        let result = run_dyn(&GrepTool, json!({"pattern": "hit"}), &ctx)
            .await
            .unwrap();
        let text = text_of(&result);
        assert!(
            text.contains("[showing first 200 of 205 matching lines"),
            "{text}"
        );
        let details = result.details.unwrap();
        assert_eq!(details["matches"], 205);
        assert_eq!(details["shown"], 200);
        assert_eq!(details["truncated"], true);

        // Custom cap via max_results.
        let result = run_dyn(&GrepTool, json!({"pattern": "hit", "max_results": 5}), &ctx)
            .await
            .unwrap();
        let text = text_of(&result);
        assert!(text.contains("[showing first 5 of 205"), "{text}");
        assert_eq!(text.lines().filter(|l| !l.starts_with('[')).count(), 5);
    }

    #[tokio::test]
    async fn path_can_target_a_single_file() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        let ctx = ctx_at(dir.path());

        let result = run_dyn(
            &GrepTool,
            json!({"pattern": "hello", "path": "src/util.rs"}),
            &ctx,
        )
        .await
        .unwrap();
        let text = text_of(&result);
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains("hello from util"), "{text}");
        assert_eq!(result.details.unwrap()["files_searched"], 1);
    }

    #[tokio::test]
    async fn path_can_target_a_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        let ctx = ctx_at(dir.path());

        let result = run_dyn(&GrepTool, json!({"pattern": "hello", "path": "docs"}), &ctx)
            .await
            .unwrap();
        let text = text_of(&result);
        assert!(text.contains("notes.md:2:hello world"), "{text}");
        assert!(!text.contains("main.rs"), "{text}");
    }

    #[tokio::test]
    async fn nonexistent_path_is_an_execution_error() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());

        // Missing directory...
        let err = run_dyn(
            &GrepTool,
            json!({"pattern": "x", "path": "no/such/dir"}),
            &ctx,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::Execution(_)), "{err}");

        // ...and missing single-file target.
        let err = run_dyn(
            &GrepTool,
            json!({"pattern": "x", "path": "no-such-file.txt"}),
            &ctx,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::Execution(_)), "{err}");
    }

    #[tokio::test]
    async fn no_matches_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        let ctx = ctx_at(dir.path());

        let result = run_dyn(&GrepTool, json!({"pattern": "zzz-nothing"}), &ctx)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(text_of(&result), "");
        assert_eq!(result.details.unwrap()["matches"], 0);
    }

    #[tokio::test]
    async fn malformed_include_glob_is_invalid_args() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());

        let err = run_dyn(
            &GrepTool,
            json!({"pattern": "x", "include": "[unclosed"}),
            &ctx,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgs(_)));
    }
}
