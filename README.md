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

The built-in keeps the compatibility name `bash` and arguments
`command`/`timeout_secs`, but executes the platform shell: a POSIX shell
on macOS/Linux and PowerShell 7 (`pwsh.exe`) on Windows. It is the only
Windows backend, and result `details.shell` reports `pwsh.exe`. If it is
absent from `PATH`, MCode provisions the pinned
Microsoft portable artifact described by
`crates/mcode-tools/assets/powershell-windows.json` under
`<mcode-home>/bin/powershell/`; HTTPS, exact size, archive SHA-256,
safe staged ZIP extraction, atomic publication, and Authenticode for the
signed startup chain are verified. Cache reuse checks the complete file
manifest, rehashing the required runtime and any file whose metadata
changed, so missing or damaged dependencies trigger a rebuild. This setup
is lazy: an offline Windows `bash` call fails closed only when `pwsh.exe`
is absent from `PATH` and no valid managed cache exists. The call observes
`timeout_secs` and cancellation during both setup and command execution.
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
