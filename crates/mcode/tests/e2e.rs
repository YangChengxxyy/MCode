//! M1 acceptance e2e (`07-m1-plan.md` §T6 + §里程碑验收脚本): the
//! real `mcode` binary driven end-to-end through the scripted
//! `FakeProvider` (`--fake` / `$MCODE_FAKE`), with `$MCODE_HOME`
//! pointed at a temp directory so tests never touch `~/.mcode`.
//!
//! 1. `mcode run` — a multi-turn tool-calling session (text →
//!    `read` tool call → closing text): stdout carries the streamed
//!    text plus the `==> tool` / `<== ok` status lines, exit code 0.
//! 2. `mcode resume latest "…"` — the same session directory is
//!    resumed and the *same file* is appended to.
//! 3. The session file exists and its first line is the
//!    `"format_version":1` header.
//! 4. Non-TTY stdin denies `Ask` permissions (bash), and `--yolo`
//!    allows them — the session still completes either way.
//! 5. `resume` without a prompt is a usage error (M1 keeps resume
//!    minimal); an unknown session spec fails with a clear message.

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// The checked-in fixture directory next to this test file.
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The `mcode` binary under test.
fn mcode() -> Command {
    Command::cargo_bin("mcode").unwrap()
}

/// An isolated environment: `$MCODE_HOME` temp dir plus a project
/// temp dir (`--cwd`) holding a small `Cargo.toml` for the `read`
/// tool to chew on.
struct Sandbox {
    home: TempDir,
    project: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        fs::write(
            project.path().join("Cargo.toml"),
            "[workspace]\nresolver = \"3\"\nmembers = []\n",
        )
        .unwrap();
        Self { home, project }
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    fn project(&self) -> &Path {
        self.project.path()
    }

    /// A raw configured command: binary + isolated home + project cwd
    /// + fake provider script; callers add flags (`--yolo`) and the
    /// subcommand.
    fn command(&self, script: impl AsRef<Path>) -> Command {
        let mut cmd = mcode();
        cmd.env("MCODE_HOME", self.home())
            .arg("--cwd")
            .arg(self.project())
            .arg("--fake")
            .arg(script.as_ref());
        cmd
    }

    /// [`Sandbox::command`] plus `--yolo`.
    fn yolo_command(&self, script: impl AsRef<Path>) -> Command {
        let mut cmd = self.command(script);
        cmd.arg("--yolo");
        cmd
    }

    /// Write a fake-provider script into the sandbox home.
    fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.home().join(name);
        fs::write(&path, body).unwrap();
        path
    }
}

/// A fake script whose bash call is subject to the `bash(*) → Ask`
/// default rule: turn 1 calls bash, turn 2 answers regardless.
fn bash_script(final_text: &str) -> String {
    r#"[{"text": "Running the build check.", "tool_calls": [{"id": "call_bash", "name": "bash", "arguments": {"command": "echo hello"}}]}, {"text": "{FINAL}", "stop_reason": "Stop"}]"#
        .replace("{FINAL}", final_text)
}

/// Every `.jsonl` session file under `<home>/sessions/**`, sorted.
fn session_files(home: &Path) -> Vec<PathBuf> {
    let sessions = home.join("sessions");
    let mut files = Vec::new();
    for slug_dir in fs::read_dir(&sessions).unwrap().flatten() {
        for entry in fs::read_dir(slug_dir.path()).unwrap().flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// The `==> …` / `<== …` status lines of a stdout capture, in order.
fn status_lines(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .filter(|line| line.starts_with("==> ") || line.starts_with("<== "))
        .collect()
}

#[test]
fn run_completes_a_multi_turn_tool_session() {
    let sandbox = Sandbox::new();

    let output = sandbox
        .yolo_command(fixtures().join("demo.json"))
        .arg("run")
        .arg("Read Cargo.toml and summarize")
        .assert()
        .success();

    let stdout_bytes = output.get_output().stdout.clone();
    let stdout = String::from_utf8_lossy(&stdout_bytes);
    // Streamed assistant text (both turns) …
    assert!(stdout.contains("I'll read Cargo.toml first."), "{stdout}");
    assert!(
        stdout.contains("Done: it is a workspace manifest with a members list."),
        "{stdout}"
    );
    // … and the documented status-line sequence: one tool call, ok,
    // then the closing text (no further status lines).
    assert_eq!(
        status_lines(&stdout),
        vec![
            "==> tool read {\"path\":\"Cargo.toml\"}",
            "<== ok [workspace]",
        ],
        "{stdout}"
    );

    // One session file under <home>/sessions/<cwd-slug>/ with the
    // format-version header, holding the full 5-entry exchange:
    // header + user + assistant(text+call) + tool result + assistant.
    let files = session_files(sandbox.home());
    assert_eq!(files.len(), 1, "exactly one session file: {files:?}");
    let content = fs::read_to_string(&files[0]).unwrap();
    let mut lines = content.lines();
    let header = lines.next().unwrap();
    assert!(header.contains(r#""format_version":1"#), "{header}");
    assert!(header.contains(r#""type":"header""#), "{header}");
    assert_eq!(content.lines().count(), 5, "{content}");
}

#[test]
fn resume_latest_continues_the_same_session_file() {
    let sandbox = Sandbox::new();

    sandbox
        .yolo_command(fixtures().join("demo.json"))
        .arg("run")
        .arg("Read Cargo.toml and summarize")
        .assert()
        .success();

    let output = sandbox
        .yolo_command(fixtures().join("demo_resume.json"))
        .arg("resume")
        .arg("latest")
        .arg("continue")
        .assert()
        .success();

    let stdout_bytes = output.get_output().stdout.clone();
    let stdout = String::from_utf8_lossy(&stdout_bytes);
    assert!(
        stdout.contains("Resumed: the manifest still says workspace."),
        "{stdout}"
    );

    // Still exactly one file (resume appends, it does not fork a new
    // one), now grown by the resumed user + assistant pair.
    let files = session_files(sandbox.home());
    assert_eq!(files.len(), 1, "{files:?}");
    let content = fs::read_to_string(&files[0]).unwrap();
    assert_eq!(content.lines().count(), 7, "{content}");
    assert!(
        content
            .lines()
            .next()
            .unwrap()
            .contains(r#""format_version":1"#),
        "{content}"
    );
    // The resumed turn is really in the log.
    assert!(
        content.contains("Resumed: the manifest still says workspace."),
        "{content}"
    );
}

#[test]
fn non_tty_stdin_denies_ask_permissions_and_the_turn_still_completes() {
    let sandbox = Sandbox::new();
    let script = sandbox.write_script(
        "bash_ask.json",
        &bash_script("Could not run bash, but that is fine."),
    );

    // No --yolo and stdin is null: the Ask must be denied, printed,
    // and fed back as an error result without breaking the turn.
    let output = sandbox
        .command(script)
        .arg("run")
        .arg("run the check")
        .assert()
        .success();

    let stdout_bytes = output.get_output().stdout.clone();
    let stdout = String::from_utf8_lossy(&stdout_bytes);
    assert!(
        status_lines(&stdout).contains(&"<== error permission denied: the request was declined"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Could not run bash, but that is fine."),
        "{stdout}"
    );
    let stderr_bytes = output.get_output().stderr.clone();
    let stderr = String::from_utf8_lossy(&stderr_bytes);
    assert!(
        stderr.contains("stdin is not a terminal"),
        "denial reason on stderr: {stderr}"
    );
    assert!(stderr.contains("permission: denied"), "{stderr}");
}

#[test]
fn yolo_allows_ask_permissions() {
    let sandbox = Sandbox::new();
    let script = sandbox.write_script("bash_yolo.json", &bash_script("The check printed hello."));

    let output = sandbox
        .yolo_command(script)
        .arg("run")
        .arg("run the check")
        .assert()
        .success();

    let stdout_bytes = output.get_output().stdout.clone();
    let stdout = String::from_utf8_lossy(&stdout_bytes);
    assert!(status_lines(&stdout).contains(&"<== ok hello"), "{stdout}");
    assert!(stdout.contains("The check printed hello."), "{stdout}");
}

#[test]
fn mcode_fake_env_var_selects_the_provider_too() {
    // The DoD script form: `$MCODE_FAKE=… mcode run …`.
    let sandbox = Sandbox::new();
    let mut cmd = mcode();
    cmd.env("MCODE_HOME", sandbox.home())
        .env("MCODE_FAKE", fixtures().join("demo_resume.json"))
        .arg("--cwd")
        .arg(sandbox.project())
        .arg("run")
        .arg("just talk");
    let output = cmd.assert().success();
    let stdout_bytes = output.get_output().stdout.clone();
    let stdout = String::from_utf8_lossy(&stdout_bytes);
    assert!(
        stdout.contains("Resumed: the manifest still says workspace."),
        "{stdout}"
    );
}

#[test]
fn resume_without_prompt_is_a_usage_error() {
    let sandbox = Sandbox::new();
    sandbox
        .yolo_command(fixtures().join("demo.json"))
        .arg("resume")
        .arg("latest")
        .assert()
        .failure()
        .stderr(predicates::str::contains("PROMPT"));
}

#[test]
fn resume_of_unknown_session_fails_with_a_clear_message() {
    let sandbox = Sandbox::new();
    sandbox
        .yolo_command(fixtures().join("demo.json"))
        .arg("resume")
        .arg("deadbeef-id")
        .arg("continue")
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "no session found for 'deadbeef-id'",
        ));
}

#[test]
fn run_with_a_broken_fake_script_fails_fast() {
    let sandbox = Sandbox::new();
    let bad = sandbox.write_script("bad.json", "not json");

    sandbox
        .command(bad)
        .arg("run")
        .arg("hello")
        .assert()
        .failure()
        .stderr(predicates::str::contains("cannot load fake script"));
}
