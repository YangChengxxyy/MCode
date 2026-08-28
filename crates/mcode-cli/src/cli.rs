//! The command-line surface of `mcode` (M1 T6): clap definitions for
//! `mcode run` / `mcode resume` plus the global flags.
//!
//! ```text
//! mcode [--provider <id>] [--profile <path.json>] [--model <id>]
//!       [--cwd <path>] run "<prompt>"
//!       resume <session-id | latest | file.jsonl> "<prompt>"
//! ```

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Model id used when `--model` is absent on OpenAI-compatible profiles.
pub const DEFAULT_MODEL: &str = "gpt-4o-mini";
/// Built-in profile used when `--provider` and `--profile` are absent.
pub const DEFAULT_PROVIDER: &str = "generic-openai";

/// Parsed command line.
#[derive(Debug, Parser)]
#[command(
    name = "mcode",
    version,
    about = "MCode headless coding agent (M1: run one turn sequence, resume sessions)"
)]
pub struct Cli {
    /// Built-in provider profile id from [`mcode_llm::ProviderRegistry`].
    #[arg(long, global = true, default_value = DEFAULT_PROVIDER, value_name = "ID")]
    pub provider: String,

    /// Strict JSON [`mcode_llm::ProviderProfile`] file. When set, this
    /// replaces the built-in `--provider` selection.
    #[arg(long, global = true, value_name = "PATH.json")]
    pub profile: Option<PathBuf>,

    /// Model id handed to the provider. When omitted, the CLI uses the
    /// selected profile's catalog default (`gpt-4o-mini`, OpenRouter
    /// `openai/gpt-4o-mini`, `claude-sonnet-4-5`, or `deepseek-chat`).
    #[arg(long, global = true, value_name = "ID")]
    pub model: Option<String>,

    /// Working directory of the session: tools resolve relative paths
    /// against it and it selects the session directory
    /// (`~/.mcode/sessions/<cwd-slug>/`). Defaults to the process cwd.
    #[arg(long, global = true, value_name = "PATH")]
    pub cwd: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

/// `mcode run` / `mcode resume` (the clap `Subcommand` derive is on the import; the enum itself is `Command` to avoid the name clash).
#[derive(Debug, PartialEq, Subcommand)]
pub enum Command {
    /// Start a new session and run one turn sequence until the agent
    /// stops.
    Run {
        /// The user prompt.
        prompt: String,
    },
    /// Resume a persisted session and continue it with a new prompt.
    Resume {
        /// Which session to resume: `latest` (most recent for the
        /// cwd), a session id, or a JSONL file path.
        session: String,
        /// The user prompt to continue with. Required in M1 — an
        /// interactive REPL lands with the TUI milestone.
        prompt: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_declares_expected_subcommands() {
        let command = Cli::command();
        let names: Vec<&str> = command.get_subcommands().map(|s| s.get_name()).collect();
        assert_eq!(names, vec!["run", "resume"]);
    }

    #[test]
    fn parses_run_with_global_flags() {
        let cli = Cli::try_parse_from([
            "mcode",
            "--provider",
            "anthropic",
            "--profile",
            "local.json",
            "--model",
            "gpt-5",
            "--cwd",
            "/tmp/x",
            "run",
            "hello",
        ])
        .unwrap();
        assert_eq!(cli.provider, "anthropic");
        assert_eq!(cli.profile, Some(PathBuf::from("local.json")));
        assert_eq!(cli.model.as_deref(), Some("gpt-5"));
        assert_eq!(cli.cwd, Some(PathBuf::from("/tmp/x")));
        assert_eq!(
            cli.command,
            Command::Run {
                prompt: "hello".into()
            }
        );
    }

    #[test]
    fn parses_resume_with_session_spec() {
        let cli = Cli::try_parse_from(["mcode", "resume", "latest", "continue"]).unwrap();
        assert_eq!(cli.provider, DEFAULT_PROVIDER);
        assert!(cli.profile.is_none());
        assert!(cli.model.is_none());
        assert_eq!(
            cli.command,
            Command::Resume {
                session: "latest".into(),
                prompt: "continue".into()
            }
        );
    }

    #[test]
    fn flags_work_after_the_subcommand_too() {
        // Global flags are accepted in either position.
        let cli = Cli::try_parse_from(["mcode", "run", "hi", "--cwd", "/tmp/x"]).unwrap();
        assert_eq!(cli.cwd, Some(PathBuf::from("/tmp/x")));
    }

    #[test]
    fn omitted_model_stays_unset_for_non_openai_providers() {
        let anthropic =
            Cli::try_parse_from(["mcode", "--provider", "anthropic", "run", "hi"]).unwrap();
        assert_eq!(anthropic.provider, "anthropic");
        assert!(anthropic.model.is_none());
        let deepseek =
            Cli::try_parse_from(["mcode", "--provider", "deepseek", "run", "hi"]).unwrap();
        assert_eq!(deepseek.provider, "deepseek");
        assert!(deepseek.model.is_none());
    }

    #[test]
    fn run_without_prompt_is_a_usage_error() {
        assert!(Cli::try_parse_from(["mcode", "run"]).is_err());
    }

    #[test]
    fn yolo_flag_is_rejected() {
        let error = Cli::try_parse_from(["mcode", "--yolo", "run", "hi"])
            .expect_err("--yolo must remain outside the CLI surface");
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn resume_without_prompt_is_a_usage_error() {
        // M1 keeps resume minimal: a prompt is mandatory.
        assert!(Cli::try_parse_from(["mcode", "resume", "latest"]).is_err());
    }
}

// Rust guideline compliant 2026-08-26
