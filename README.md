# MCode

MCode is being rebuilt around a minimal Agent Core and signed Manager/Pack product features. The current repository provides the library foundation for the seven canonical builtins, but the product CLI does not yet assemble Provider or Session services.

## Fail-closed CLI skeleton (T5)

The `mcode` binary currently preserves only the future non-interactive command shape:

```text
mcode [--cwd <path>] run "<prompt>"
mcode [--cwd <path>] resume <session> "<prompt>"
```

Both commands parse successfully and then exit with code `1`. The deterministic setup error instructs the user to install and activate:

- the `com.mcode.providers` Manager with a signed Provider Pack; and
- the `com.mcode.session` Manager with a signed Session Pack.

Until those Manager-bound typed services exist, the CLI does not start a run, persist or resume state, access `--cwd`, create MCode home/session/state paths, read credentials or environment-based product configuration, or use the network. `--cwd` is retained only as an invocation parameter for the future implementation.

The removed product flags `--provider`, `--profile`, `--model`, `--fake`, and `--yolo` are rejected by clap with exit code `2`. `MCODE_FAKE` is not read. Non-interactive run/resume behavior will be rebuilt at T24 on Providers/Session Manager-bound typed services; the old Provider/Session assembly is not a fallback.

## Core builtin library foundation

The canonical builtin names are `read`, `write`, `edit`, `shell`, `exec`, `grep`, and `find`. They are library capabilities and are not currently reachable through the fail-closed product commands.

The public platform-shell builtin is `shell`; there is no `bash` alias. It accepts `command`/`timeout_secs` and uses `/bin/bash`, PATH `bash`, then `sh` on macOS/Linux, or PowerShell 7 (`pwsh.exe`) on Windows. Launch goes through Structured Exec with one invocation working cwd, an allowlisted environment, and a reconstructed PATH snapshot; executable identity is pinned before contained spawn. A PATH-resolved Windows `pwsh.exe` is pinned and identity-checked, but remains same-account host input rather than a sandbox boundary. After typed PATH `NotFound`, the managed cache is the authenticated Microsoft-distribution fallback. The pinned Microsoft portable artifact described by `crates/mcode-tools/assets/powershell-windows.json` is provisioned under the configured MCode home `bin/powershell/` directory; HTTPS, exact size, archive SHA-256, safe staged ZIP extraction, atomic publication, and Authenticode for the signed startup chain are verified. Cache reuse checks the complete file manifest, rehashing the required runtime and any file whose metadata changed, so missing or damaged dependencies trigger a rebuild. Managed cache provisioning runs only after typed PATH `NotFound`; a non-regular or non-PE PATH hit fails closed without downloading. This setup is lazy: an offline Windows `shell` call fails closed when `pwsh.exe` is absent from `PATH` and no valid managed cache exists. The call observes `timeout_secs` and cancellation during both setup and command execution. If either wins during blocking cache finalization, MCode returns without starting the shell; the finalizer may finish the cache in the background while retaining its staging directory and install lock.

Windows passes the user script itself as UTF-16LE Base64 to `-EncodedCommand`; no .NET launcher runs before it, so leading `using` statements and ConstrainedLanguage-permitted cmdlets remain usable. `-ExecutionPolicy Bypass` does not override WDAC/AppLocker language mode. A suspended child is assigned to a dedicated kill-on-close Job before it is resumed, preferring a nested Job under CI host Jobs and using explicit breakaway only when nesting is rejected. Unix timeout cleanup validates the still-unreaped leader and current PGID before `killpg`. Processes created outside those inherited boundaries (for example via `setsid` or an external Windows broker) are not claimed as contained.

`exec` launches one PE, ELF, or Mach-O image from an explicit `program` plus `args[]`; it never inserts a shell, follows a shebang, or falls back to an interpreter. Bare names search only absolute host `PATH` entries, including final symlink and reparse aliases, while path arguments resolve against the invocation working cwd. A registered, schema-valid builtin `exec` call executes directly after schema validation: there is no Core permission prompt and no policy hook. Preparation snapshots the cwd, sorted allowlisted runtime/locale/temp environment, and reconstructed absolute `PATH` once; the same snapshot drives bare-name resolution and every platform spawn. A versioned, length-framed SHA-256 invocation digest binds the canonical image path, native file identity, image digest, effective `argv[0]`, arguments, cwd, and effective environment. Results expose that digest and bounded length summaries, never environment values. Ambient credential and loader-injection variables are not copied.

`exec` is unsandboxed current-user execution with normal filesystem and network access. It follows the final alias, opens the regular target, and records identity from that retained handle; Linux x86_64 GNU uses `execveat` with fail-closed `close_range(CLOSE_RANGE_CLOEXEC)` so only fds 0–2 survive a successful exec (musl/BSD are unsupported), Windows enrolls a suspended child in a dedicated Job, verifies it, then resumes it, and macOS Apple Silicon verifies a suspended `/dev/fd` launch before `SIGCONT`. A process-wide lease serializes builtin `write`, `edit`, `shell`, and `exec`. Process cleanup retains the pin and lease through terminate-and-reap; dropping its calling future transfers that ownership to a supervisor. Same-account processes outside MCode remain outside this boundary.

Registered, schema-valid builtin calls execute directly. There is no Core permission prompt, `--yolo` flag, or persistent grant file. Unknown tools, invalid arguments, cancellation, and tool errors remain lifecycle errors. These safety contracts are not a claim that the current CLI can dispatch the builtins.
