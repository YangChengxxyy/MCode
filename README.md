# MCode

A composable agent coding harness with built-in subagents, configurable web search, and MCP support.

## Headless CLI (M1)

M1 ships the TUI-less `mcode` binary: it runs one multi-turn
tool-calling session, persists it as JSONL, and can resume it later.

```text
mcode [--model <model>] [--cwd <path>] [--fake <script.json>]
      run "<prompt>"                                        # new session
      resume <session-id | latest | file.jsonl> "<prompt>"  # continue (prompt required)
```

- `--model` — model id handed to the provider (default `gpt-4o-mini`).
- `--cwd` — session working directory; tools resolve relative paths
  against it, and it selects the session directory
  `<mcode-home>/sessions/<cwd-slug>/` (default: the process cwd).
  `resume latest` picks the newest session for that cwd.
- `--fake <script.json>` — drive the session from a scripted
  `FakeProvider` instead of a real provider; also settable as
  `$MCODE_FAKE` (the flag wins). This is the foundation of the e2e
  tests and is never removed.

The public platform-shell built-in is `shell`; there is no `bash` alias. It
accepts `command`/`timeout_secs` and uses `/bin/bash`, PATH `bash`, then `sh`
on macOS/Linux, or PowerShell 7 (`pwsh.exe`) on Windows. Launch goes through
Structured Exec with one cwd, allowlisted environment, and reconstructed PATH
snapshot; executable identity is pinned before contained spawn. A PATH-resolved
Windows `pwsh.exe` is pinned and identity-checked, but remains same-account host
input rather than a sandbox boundary. After typed PATH `NotFound`, the managed
cache is the authenticated Microsoft-distribution fallback. The pinned
Microsoft portable artifact described by
`crates/mcode-tools/assets/powershell-windows.json` is provisioned under
`<mcode-home>/bin/powershell/`; HTTPS, exact size, archive SHA-256, safe staged
ZIP extraction, atomic publication, and Authenticode for the signed startup
chain are verified. Cache reuse checks the complete file manifest, rehashing
the required runtime and any file whose metadata changed, so missing or
damaged dependencies trigger a rebuild. Managed cache provisioning runs
only after typed PATH `NotFound`; a non-regular or non-PE PATH hit fails
closed without downloading. This setup is lazy: an offline Windows `shell`
call fails closed when `pwsh.exe` is absent from `PATH` and no valid
managed cache exists. The call observes `timeout_secs` and
cancellation during both setup and command execution.
If either wins during blocking cache finalization, MCode returns without
starting the shell; the finalizer may finish the cache in the background
while retaining its staging directory and install lock.

Windows passes the user script itself as UTF-16LE Base64 to
`-EncodedCommand`; no .NET launcher runs before it, so leading `using`
statements and ConstrainedLanguage-permitted cmdlets remain usable.
`-ExecutionPolicy Bypass` does not override WDAC/AppLocker language mode.
A suspended child is assigned to a dedicated kill-on-close Job before it
is resumed, preferring a nested Job under CI host Jobs and using explicit
breakaway only when nesting is rejected. Unix timeout cleanup validates
the still-unreaped leader and current PGID before `killpg`. Processes
created outside those inherited boundaries (for example via `setsid` or
an external Windows broker) are not claimed as contained.

The seven built-ins are `read`, `write`, `edit`, `shell`, `exec`, `grep`,
and `find`. `exec` launches one PE, ELF, or Mach-O image from an explicit
`program` plus `args[]`; it never inserts a shell, follows a shebang, or
falls back to an interpreter. Bare names search only absolute host `PATH`
entries, including final symlink and reparse aliases, while path arguments
resolve against the session cwd. A registered, schema-valid builtin `exec`
call executes directly after schema validation: there is no Core permission
prompt and no policy hook. Preparation snapshots the cwd, sorted allowlisted
runtime/locale/temp environment, and reconstructed absolute `PATH` once; the
same snapshot drives bare-name resolution and every platform spawn. A
versioned, length-framed SHA-256 invocation digest binds the canonical image
path, native file identity, image digest, effective `argv[0]`, arguments, cwd,
and effective environment. Results expose that digest and bounded length
summaries, never environment values. Ambient credential and loader-injection variables are not
copied.

`exec` is unsandboxed current-user execution with normal filesystem and
network access. It follows the final alias, opens the regular target, and
records identity from that retained handle; Linux x86_64 GNU uses `execveat`
with fail-closed `close_range(CLOSE_RANGE_CLOEXEC)` so only fds 0–2 survive a
successful exec (musl/BSD are unsupported), Windows enrolls a suspended child
in a dedicated Job, verifies it, then resumes it, and macOS Apple Silicon
verifies a suspended `/dev/fd` launch before `SIGCONT`. A
process-wide lease serializes built-in `write`, `edit`, `shell`, and `exec`.
Process cleanup retains the pin and lease through terminate-and-reap; dropping
its calling future transfers that ownership to a supervisor. Same-account
processes outside MCode remain outside this boundary.

Registered, schema-valid tool calls execute directly. There is no Core
permission prompt, `--yolo` flag, or persistent grant file. Unknown
tools, invalid arguments, cancellation, and tool errors still fail as
lifecycle errors and are written back to the model as `is_error`
results.

Output contract: assistant text streams to stdout raw, with
`==> tool <name> <args>` / `<== ok|error <summary>` status lines
(args/summary truncated to 120 chars); thinking, progress, and errors
go to stderr.

Exit codes: `0` — turn completed (or steered to an end); `1` —
aborted/errored turn or setup failure; `2` — clap usage error.

Sessions are stored as JSONL (`format_version: 1` header) under
`<mcode-home>/sessions/<cwd-slug>/`, where the home is `$MCODE_HOME`
or `~/.mcode`. Without `--fake`, the OpenAI-compatible provider reads
`OPENAI_API_KEY` (optionally `OPENAI_BASE_URL`, falling back to
`~/.mcode/auth.toml`).

Example (scripted, no API key; build first with `cargo build -p mcode`):

```bash
MCODE_HOME=/tmp/mcode-demo MCODE_FAKE=crates/mcode/tests/fixtures/demo.json \
  mcode run "读取 Cargo.toml 并总结"
MCODE_HOME=/tmp/mcode-demo MCODE_FAKE=crates/mcode/tests/fixtures/demo_resume.json \
  mcode resume latest "继续"
```
