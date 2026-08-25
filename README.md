# MCode

A composable agent coding harness with built-in subagents, configurable web search, and MCP support.

## Headless CLI (M1)

M1 ships the TUI-less `mcode` binary: it runs one multi-turn
tool-calling session, persists it as JSONL, and can resume it later.

```text
mcode [--model <model>] [--cwd <path>] [--fake <script.json>] [--yolo]
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
- `--yolo` — answer every permission request with "allow", skipping
  the prompt.

Without `--yolo`, the default rules map `bash(*)` to `Ask`: the
question is printed on stderr and one stdin line is read — `y`/`yes`
allows, anything else denies; a non-TTY stdin denies immediately; no
answer within 30 s denies.

Output contract: assistant text streams to stdout raw, with
`==> tool <name> <args>` / `<== ok|error <summary>` status lines
(args/summary truncated to 120 chars); thinking, progress, permission
decisions and errors go to stderr.

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
