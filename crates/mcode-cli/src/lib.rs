//! `mcode-cli` — the headless CLI of MCode (M1 T6): clap parsing,
//! session assembly, and the stdout rendering loop that closes the
//! M1 milestone ("complete a multi-turn tool-calling session in a
//! TUI-less terminal and resume it").
//!
//! # Assembly
//!
//! ```text
//! --fake <script.json> ─► FakeProvider ─┐
//! otherwise OpenAiProvider::from_env() ─┴► SessionEnv
//!                                        ├─ ToolRegistry (5 builtins)
//!                                        ├─ PermissionEngine (default rules: bash → Ask)
//!                                        ├─ permission prompt: StdinPermissionPrompt | AllowAll (--yolo)
//!                                        └─ cwd (--cwd, default: process cwd)
//!                     SessionHandle::new / resume_path(latest|id|path)
//!                     Prompt(UserMessage) ──► SessionEvent stream ──► HeadlessRenderer
//!                     TurnEnded ──► wait until persisted ──► exit 0
//! ```
//!
//! Exit codes: `0` when the turn completed (or was steered to an end),
//! `1` when it aborted/errored or setup failed; clap usage errors keep
//! clap's `2`.

pub mod cli;
pub mod permission;
pub mod render;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use mcode_agent::{AgentConfig, AllowAll, PermissionPrompt};
use mcode_core::events::{SessionEvent, TurnOutcome};
use mcode_core::message::{Message, UserMessage};
use mcode_llm::{FakeProvider, OpenAiProvider, Provider};
use mcode_session::{SessionHandle, default_agent_factory, paths};
use mcode_tools::ToolRegistry;
use mcode_tools::builtin::register_builtins;
use tokio::sync::broadcast;

pub use cli::{Cli, Command, SYSTEM_PROMPT};
pub use permission::StdinPermissionPrompt;
pub use render::HeadlessRenderer;

/// Exit code of a successfully completed turn.
pub const EXIT_OK: u8 = 0;
/// Exit code for aborted/errored turns and setup failures.
pub const EXIT_FAILURE_U8: u8 = 1;

/// Binary entry point: parse, run, map to an exit code. Usage errors
/// exit with clap's code (`2`) directly.
pub fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build the tokio runtime");
    match runtime.block_on(run(cli)) {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            eprintln!("mcode: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// Everything the CLI does after parsing, as one async function (unit
/// of the runtime above).
pub async fn run(cli: Cli) -> Result<u8> {
    let cwd = resolve_cwd(cli.cwd.as_deref())?;
    let provider: Arc<dyn Provider> = match &cli.fake {
        Some(script) => Arc::new(
            FakeProvider::from_json_file(script)
                .with_context(|| format!("cannot load fake script {}", script.display()))?,
        ),
        None => Arc::new(OpenAiProvider::from_env()?),
    };

    let tools = Arc::new(ToolRegistry::new());
    register_builtins(&tools);

    let permission_prompt: Arc<dyn PermissionPrompt> = if cli.yolo {
        Arc::new(AllowAll)
    } else {
        Arc::new(StdinPermissionPrompt::new())
    };

    let env = mcode_session::SessionEnv::new(provider, tools)
        .with_cwd(cwd.clone())
        .with_permission_prompt(permission_prompt);
    let agent_config = AgentConfig::new(cli.model.clone()).with_system_prompt(SYSTEM_PROMPT);

    let prompt = match &cli.command {
        cli::Command::Run { prompt } => prompt.clone(),
        cli::Command::Resume { prompt, .. } => prompt.clone(),
    };

    let handle = match &cli.command {
        cli::Command::Run { .. } => SessionHandle::new(env, agent_config, default_agent_factory())
            .context("cannot start a new session")?,
        cli::Command::Resume { session, .. } => {
            let path = resolve_resume_spec(session, &cwd)?;
            SessionHandle::resume_path(path, env, agent_config, default_agent_factory())
                .context("cannot resume the session")?
        }
    };
    eprintln!(
        "session {} → {}",
        handle.session_id(),
        handle.path().display()
    );

    // Subscribe before prompting so no event of the turn is missed.
    let mut events = handle.subscribe();
    handle
        .prompt(Message::User(UserMessage::text(prompt)))
        .await
        .context("cannot send the prompt to the session")?;

    let mut renderer = HeadlessRenderer::stdio();
    let mut outcome: Option<TurnOutcome> = None;
    loop {
        match events.recv().await {
            Ok(event) => {
                if let SessionEvent::TurnEnded(turn) = &event {
                    outcome = Some(*turn);
                }
                renderer
                    .render(&event)
                    .context("cannot write the session output")?;
                if outcome.is_some() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                eprintln!("mcode: rendering lagged, {skipped} events skipped");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }

    // Deterministic disk barrier: the turn is persisted before exit.
    handle
        .wait_processed(1)
        .await
        .context("the session actor stopped before persisting")?;
    if let Err(err) = handle.shutdown().await {
        eprintln!("mcode: session actor ended uncleanly: {err}");
    }

    Ok(match outcome {
        Some(TurnOutcome::Completed | TurnOutcome::Steered) => EXIT_OK,
        _ => EXIT_FAILURE_U8,
    })
}

/// Resolve the session working directory: the `--cwd` flag or the
/// process cwd, canonicalized so the session-dir slug is stable
/// across `run` and later `resume latest` invocations.
fn resolve_cwd(flag: Option<&Path>) -> Result<PathBuf> {
    let cwd = match flag {
        Some(path) => path.to_path_buf(),
        None => std::env::current_dir().context("cannot determine the current directory")?,
    };
    std::fs::canonicalize(&cwd)
        .with_context(|| format!("--cwd {} is not an accessible directory", cwd.display()))
}

/// Resolve a `resume` session specifier against the real session root:
/// `latest` → the newest session file for `cwd`; anything else is a
/// file path or a session id (see
/// [`paths::resolve_session`](mcode_session::paths::resolve_session)).
pub fn resolve_resume_spec(spec: &str, cwd: &Path) -> Result<PathBuf> {
    resolve_resume_spec_from(&paths::sessions_root(), cwd, spec)
}

/// Testable core of [`resolve_resume_spec`] with the sessions root
/// injected.
pub fn resolve_resume_spec_from(root: &Path, cwd: &Path, spec: &str) -> Result<PathBuf> {
    if spec.eq_ignore_ascii_case("latest") {
        paths::latest_session_file_from(root, cwd).with_context(|| {
            format!(
                "no session found for cwd {} under {} — run `mcode run` first",
                cwd.display(),
                root.display()
            )
        })
    } else {
        paths::resolve_session_from(root, spec).with_context(|| {
            format!(
                "no session found for '{spec}' (neither an existing file nor a known session id)"
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Create a session file under `<root>/<slug>/<name>` — the layout
    /// [`paths::latest_session_file_from`] expects (`root` is the
    /// *sessions* root, not the MCode home).
    fn touch(root: &Path, slug: &str, name: &str) -> PathBuf {
        let dir = root.join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, "").unwrap();
        path
    }

    #[test]
    fn latest_resolves_the_newest_session_file_for_the_cwd() {
        let home = TempDir::new().unwrap();
        let root = home.path().join("sessions");
        touch(&root, "Users-cc-app", "20250101T000000_a.jsonl");
        let newest = touch(&root, "Users-cc-app", "20250202T000000_b.jsonl");
        touch(&root, "Users-cc-other", "20250303T000000_c.jsonl");

        let resolved = resolve_resume_spec_from(&root, Path::new("/Users/cc/app"), "latest");
        assert_eq!(resolved.unwrap(), newest);
    }

    #[test]
    fn latest_without_sessions_is_an_error() {
        let home = TempDir::new().unwrap();
        let err = resolve_resume_spec_from(home.path(), Path::new("/nowhere"), "LATEST")
            .unwrap_err()
            .to_string();
        assert!(err.contains("no session found"), "{err}");
    }

    #[test]
    fn spec_passes_through_existing_files() {
        let home = TempDir::new().unwrap();
        let file = touch(&home.path().join("sessions"), "slug", "x.jsonl");
        let resolved = resolve_resume_spec_from(
            &home.path().join("sessions"),
            Path::new("/anywhere"),
            &file.display().to_string(),
        );
        assert_eq!(resolved.unwrap(), file);
    }

    #[test]
    fn unknown_id_is_an_error() {
        let home = TempDir::new().unwrap();
        let err = resolve_resume_spec_from(home.path(), Path::new("/anywhere"), "deadbeef")
            .unwrap_err()
            .to_string();
        assert!(err.contains("deadbeef"), "{err}");
        assert!(err.contains("no session found"), "{err}");
    }
}
