//! Stage-3 permission resolution for the headless CLI: the
//! ask-the-user callback prints the question on **stderr** (assistant
//! text on stdout stays clean) and reads one answer line from stdin.
//!
//! Decision rules (M1 headless):
//!
//! * `--yolo` never reaches here — the CLI wires [`AllowAll`]
//!   ([`mcode_agent::AllowAll`]) instead.
//! * stdin is not a terminal (closed, piped, redirected) → **deny**
//!   immediately, with the reason printed on stderr.
//! * `y` / `yes` (case-insensitive, surrounding whitespace trimmed) →
//!   allow; anything else → deny.
//! * no answer within [`ANSWER_TIMEOUT`] (30 s) → **deny**.

use std::io::IsTerminal;
use std::time::Duration;

use async_trait::async_trait;
use mcode_agent::{PermissionPrompt, PermissionRequest};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::render::truncate_chars;

/// How long the prompt waits for an answer line before denying.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

/// How many characters of the call arguments the question shows.
const ARGS_WIDTH: usize = 120;

/// The stdin-based [`PermissionPrompt`] of the headless CLI.
pub struct StdinPermissionPrompt {
    timeout: Duration,
}

impl StdinPermissionPrompt {
    /// A prompt with the default [`ANSWER_TIMEOUT`].
    pub fn new() -> Self {
        Self {
            timeout: ANSWER_TIMEOUT,
        }
    }

    /// A prompt with a custom timeout (tests).
    pub fn with_timeout(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl Default for StdinPermissionPrompt {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse one answer line: only `y` / `yes` (case-insensitive) allow.
pub fn parse_answer(line: &str) -> bool {
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

#[async_trait]
impl PermissionPrompt for StdinPermissionPrompt {
    async fn prompt(&self, req: PermissionRequest) -> bool {
        let args = truncate_chars(&req.arguments.to_string(), ARGS_WIDTH);
        let question = format!("permission: allow {}({args})? [y/N] ", req.tool_name);

        // Non-TTY stdin can never carry an interactive answer — deny
        // rather than block on a dead pipe.
        if !std::io::stdin().is_terminal() {
            eprintln!("{question}— stdin is not a terminal, denying");
            return false;
        }

        eprint!("{question}");
        use std::io::Write as _;
        let _ = std::io::stderr().flush();

        let mut line = String::new();
        let mut stdin = BufReader::new(tokio::io::stdin());
        let read = tokio::select! {
            read = stdin.read_line(&mut line) => read,
            _ = tokio::time::sleep(self.timeout) => {
                eprintln!("\npermission: no answer within {} s, denying", self.timeout.as_secs());
                return false;
            }
        };
        match read {
            Ok(0) => {
                eprintln!("\npermission: stdin closed, denying");
                false
            }
            Ok(_) => parse_answer(&line),
            Err(err) => {
                eprintln!("\npermission: stdin read failed ({err}), denying");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_y_and_yes_allow() {
        assert!(parse_answer("y"));
        assert!(parse_answer("yes"));
        assert!(parse_answer(" Y "));
        assert!(parse_answer("YES\n"));
        assert!(!parse_answer(""));
        assert!(!parse_answer("\n"));
        assert!(!parse_answer("n"));
        assert!(!parse_answer("no"));
        assert!(!parse_answer("yeah"));
        assert!(!parse_answer("y es"));
        assert!(!parse_answer("allow"));
    }

    #[test]
    fn default_timeout_is_30s() {
        assert_eq!(StdinPermissionPrompt::new().timeout, ANSWER_TIMEOUT);
        assert_eq!(
            StdinPermissionPrompt::default().timeout,
            Duration::from_secs(30)
        );
    }
}
