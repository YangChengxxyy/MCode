//! M1 acceptance e2e: the real `mcode` binary driven through a JSON
//! [`mcode_llm::ProviderProfile`] against a localhost chat-completions
//! mock. `$MCODE_HOME` points at a temp directory so tests never touch
//! `~/.mcode`.
//!
//! 1. `mcode run` — a multi-turn tool-calling session (text →
//!    `read` tool call → closing text): stdout carries the streamed
//!    text plus the `==> tool` / `<== ok` status lines, exit code 0.
//! 2. `mcode resume latest "…"` — the same session directory is
//!    resumed and the *same file* is appended to.
//! 3. The session file exists and its first line is the
//!    `"format_version":2` header.
//! 4. A `shell` tool call executes without a Core permission prompt and
//!    captures real shell output. On Windows that assertion requires a
//!    usable `pwsh.exe` on `PATH` and never provisions it.
//! 5. `resume` without a prompt is a usage error (M1 keeps resume
//!    minimal); an unknown session spec fails with a clear message.

// Rust guideline compliant 2026-08-26.

use assert_cmd::Command;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use tempfile::TempDir;

/// The `mcode` binary under test.
fn mcode() -> Command {
    Command::cargo_bin("mcode").unwrap()
}

#[cfg(windows)]
fn path_pwsh_candidate(path_var: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    for entry in std::env::split_paths(path_var.unwrap_or_default()) {
        if !entry.is_absolute() {
            continue;
        }
        let candidate = entry.join("pwsh.exe");
        match fs::metadata(&candidate) {
            Ok(metadata) if metadata.is_file() => return Some(candidate),
            Ok(_) => return None,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
    }
    None
}

#[cfg(windows)]
fn path_pwsh_is_usable() -> bool {
    let Some(candidate) = path_pwsh_candidate(std::env::var_os("PATH").as_deref()) else {
        return false;
    };
    std::process::Command::new(candidate)
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

/// One localhost chat-completions server with a queued response body.
struct LocalLlm {
    addr: SocketAddr,
    _accept: JoinHandle<()>,
}

impl LocalLlm {
    fn spawn(bodies: Vec<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local mock");
        let addr = listener.local_addr().expect("addr");
        let queue = Arc::new(Mutex::new(VecDeque::from(bodies)));
        let accept = std::thread::spawn(move || {
            for incoming in listener.incoming() {
                let Ok(mut stream) = incoming else {
                    continue;
                };
                if drain_http_request(&mut stream).is_none() {
                    continue;
                }
                let body = queue.lock().expect("mock queue").pop_front();
                let response = match body {
                    Some(bytes) => bytes,
                    None => http_response(
                        "500 Internal Server Error",
                        "application/json",
                        br#"{"error":"script exhausted"}"#,
                    ),
                };
                let _ = stream.write_all(&response);
                let _ = stream.flush();
            }
        });
        Self {
            addr,
            _accept: accept,
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }
}

fn drain_http_request(stream: &mut TcpStream) -> Option<()> {
    let mut buf = Vec::new();
    loop {
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let mut content_length = 0usize;
    for line in head.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let header_end = buf
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .unwrap_or(buf.len());
    let mut have = buf.len().saturating_sub(header_end);
    while have < content_length {
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        have += n;
    }
    Some(())
}

fn http_response(status: &str, content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}

fn sse_http(events: &[Value]) -> Vec<u8> {
    let mut body = String::new();
    for event in events {
        body.push_str("data: ");
        body.push_str(&event.to_string());
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    http_response("200 OK", "text/event-stream", body.as_bytes())
}

fn text_turn(text: &str) -> Vec<u8> {
    sse_http(&[
        json!({"choices":[{"index":0,"delta":{"content":text},"finish_reason":null}]}),
        json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
    ])
}

fn tool_turn(text: &str, id: &str, name: &str, arguments: Value) -> Vec<u8> {
    sse_http(&[
        json!({"choices":[{"index":0,"delta":{"content":text},"finish_reason":null}]}),
        json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}}]},"finish_reason":null}]}),
        json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
    ])
}

/// Isolated home, project cwd, JSON profile, and localhost mock.
struct Sandbox {
    home: TempDir,
    project: TempDir,
    profile: PathBuf,
    _llm: LocalLlm,
}

impl Sandbox {
    fn with_turns(turns: Vec<Vec<u8>>) -> Self {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        fs::write(
            project.path().join("Cargo.toml"),
            "[workspace]\nresolver = \"3\"\nmembers = []\n",
        )
        .unwrap();
        let llm = LocalLlm::spawn(turns);
        let profile = home.path().join("provider.json");
        let profile_json = json!({
            "id": "e2e-local",
            "wire": "open_ai_chat_completions",
            "base_url": llm.base_url(),
            "auth": { "scheme": "none" }
        });
        fs::write(&profile, profile_json.to_string()).unwrap();
        Self {
            home,
            project,
            profile,
            _llm: llm,
        }
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    fn project(&self) -> &Path {
        self.project.path()
    }

    fn command(&self) -> Command {
        let mut cmd = mcode();
        cmd.env("MCODE_HOME", self.home())
            .arg("--cwd")
            .arg(self.project())
            .arg("--profile")
            .arg(&self.profile);
        cmd
    }
}

fn demo_turns() -> Vec<Vec<u8>> {
    vec![
        tool_turn(
            "I'll read Cargo.toml first.",
            "call_read_manifest",
            "read",
            json!({"path": "Cargo.toml"}),
        ),
        text_turn("Done: it is a workspace manifest with a members list."),
    ]
}

fn resume_turns() -> Vec<Vec<u8>> {
    let mut turns = demo_turns();
    turns.push(text_turn("Resumed: the manifest still says workspace."));
    turns
}

fn shell_turns(final_text: &str) -> Vec<Vec<u8>> {
    vec![
        tool_turn(
            "Running the build check.",
            "call_shell",
            "shell",
            json!({"command": "echo hello"}),
        ),
        text_turn(final_text),
    ]
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
    let sandbox = Sandbox::with_turns(demo_turns());

    let output = sandbox
        .command()
        .arg("run")
        .arg("Read Cargo.toml and summarize")
        .assert()
        .success();

    let stdout_bytes = output.get_output().stdout.clone();
    let stdout = String::from_utf8_lossy(&stdout_bytes);
    assert!(stdout.contains("I'll read Cargo.toml first."), "{stdout}");
    assert!(
        stdout.contains("Done: it is a workspace manifest with a members list."),
        "{stdout}"
    );
    assert_eq!(
        status_lines(&stdout),
        vec![
            "==> tool read {\"path\":\"Cargo.toml\"}",
            "<== ok [workspace]",
        ],
        "{stdout}"
    );

    let files = session_files(sandbox.home());
    assert_eq!(files.len(), 1, "exactly one session file: {files:?}");
    let content = fs::read_to_string(&files[0]).unwrap();
    let mut lines = content.lines();
    let header = lines.next().unwrap();
    assert!(header.contains(r#""format_version":2"#), "{header}");
    assert!(header.contains(r#""type":"header""#), "{header}");
    assert_eq!(content.lines().count(), 5, "{content}");
}

#[test]
fn resume_latest_continues_the_same_session_file() {
    let sandbox = Sandbox::with_turns(resume_turns());

    sandbox
        .command()
        .arg("run")
        .arg("Read Cargo.toml and summarize")
        .assert()
        .success();

    let output = sandbox
        .command()
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

    let files = session_files(sandbox.home());
    assert_eq!(files.len(), 1, "{files:?}");
    let content = fs::read_to_string(&files[0]).unwrap();
    assert_eq!(content.lines().count(), 7, "{content}");
    assert!(
        content
            .lines()
            .next()
            .unwrap()
            .contains(r#""format_version":2"#),
        "{content}"
    );
    assert!(
        content.contains("Resumed: the manifest still says workspace."),
        "{content}"
    );
}

#[test]
fn shell_executes_without_a_permission_prompt() {
    #[cfg(windows)]
    if !path_pwsh_is_usable() {
        eprintln!("skipping e2e shell test: usable pwsh.exe is not on PATH");
        return;
    }

    let sandbox = Sandbox::with_turns(shell_turns("The check printed hello."));

    let output = sandbox
        .command()
        .arg("run")
        .arg("run the check")
        .assert()
        .success();

    let stdout_bytes = output.get_output().stdout.clone();
    let stdout = String::from_utf8_lossy(&stdout_bytes);
    assert!(status_lines(&stdout).contains(&"<== ok hello"), "{stdout}");
    assert!(stdout.contains("The check printed hello."), "{stdout}");
    let stderr_bytes = output.get_output().stderr.clone();
    let stderr = String::from_utf8_lossy(&stderr_bytes);
    assert!(
        !stderr.contains("permission:"),
        "headless dispatch must not wait for a permission prompt: {stderr}"
    );
    #[cfg(windows)]
    assert!(
        !sandbox.home().join("bin").join("powershell").exists(),
        "PATH-backed shell e2e must not create a managed PowerShell cache"
    );
}

#[test]
fn resume_without_prompt_is_a_usage_error() {
    let sandbox = Sandbox::with_turns(demo_turns());
    sandbox
        .command()
        .arg("resume")
        .arg("latest")
        .assert()
        .failure()
        .stderr(predicates::str::contains("PROMPT"));
}

#[test]
fn resume_of_unknown_session_fails_with_a_clear_message() {
    let sandbox = Sandbox::with_turns(demo_turns());
    sandbox
        .command()
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
fn run_with_a_broken_profile_fails_fast() {
    let sandbox = Sandbox::with_turns(demo_turns());
    let bad = sandbox.home().join("bad.json");
    fs::write(&bad, "not json").unwrap();

    let mut cmd = mcode();
    cmd.env("MCODE_HOME", sandbox.home())
        .arg("--cwd")
        .arg(sandbox.project())
        .arg("--profile")
        .arg(&bad)
        .arg("run")
        .arg("hello")
        .assert()
        .failure()
        .stderr(predicates::str::contains("invalid provider profile JSON"));
}

// Rust guideline compliant 2026-08-26
