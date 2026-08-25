//! The command-line surface of `mcode` (M1 T6): clap definitions for
//! `mcode run` / `mcode resume` plus the global flags.
//!
//! ```text
//! mcode [--model <id>] [--cwd <path>] [--fake <script.json>] [--yolo]
//!       run "<prompt>"
//!       resume <session-id | latest | file.jsonl> "<prompt>"
//! ```

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Model id used when `--model` is absent.
pub const DEFAULT_MODEL: &str = "gpt-4o-mini";

/// The system prompt M1 sessions run with (a single part).
pub const SYSTEM_PROMPT: &str = "You are MCode, a terminal coding agent. Use the \
     available tools to accomplish the user's task; when the task is done, reply \
     with a concise summary and stop.";

/// Parsed command line.
#[derive(Debug, Parser)]
#[command(
    name = "mcode",
    version,
    about = "MCode headless coding agent (M1: run one turn sequence, resume sessions)"
)]
pub struct Cli {
    /// Model id handed to the provider.
    #[arg(long, global = true, default_value = DEFAULT_MODEL)]
    pub model: String,

    /// Working directory of the session: tools resolve relative paths
    /// against it and it selects the session directory
    /// (`~/.mcode/sessions/<cwd-slug>/`). Defaults to the process cwd.
    #[arg(long, global = true, value_name = "PATH")]
    pub cwd: Option<PathBuf>,

    /// Drive the model from a scripted `FakeProvider` JSON file instead
    /// of a real provider — the foundation of every e2e test (never
    /// removed). Also settable as `$MCODE_FAKE`; the flag wins.
    #[arg(long, global = true, env = "MCODE_FAKE", value_name = "SCRIPT.json")]
    pub fake: Option<PathBuf>,

    /// Answer every permission request with "allow" (skip the stdin
    /// prompt entirely).
    #[arg(long, global = true)]
    pub yolo: bool,

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
            "--model",
            "gpt-5",
            "--cwd",
            "/tmp/x",
            "--fake",
            "demo.json",
            "--yolo",
            "run",
            "hello",
        ])
        .unwrap();
        assert_eq!(cli.model, "gpt-5");
        assert_eq!(cli.cwd, Some(PathBuf::from("/tmp/x")));
        assert_eq!(cli.fake, Some(PathBuf::from("demo.json")));
        assert!(cli.yolo);
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
        assert_eq!(cli.model, DEFAULT_MODEL);
        assert!(!cli.yolo);
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
        let cli = Cli::try_parse_from(["mcode", "run", "hi", "--yolo"]).unwrap();
        assert!(cli.yolo);
    }

    #[test]
    fn run_without_prompt_is_a_usage_error() {
        assert!(Cli::try_parse_from(["mcode", "run"]).is_err());
    }

    #[test]
    fn resume_without_prompt_is_a_usage_error() {
        // M1 keeps resume minimal: a prompt is mandatory.
        assert!(Cli::try_parse_from(["mcode", "resume", "latest"]).is_err());
    }
}
