# Vendored dependencies

## rmcp 3.1.4

- **Source:** official crate `rmcp` 3.1.4 from the [Model Context Protocol Rust SDK](https://github.com/modelcontextprotocol/rust-sdk) (`crates.io` / git tag `rmcp-v3.1.4`).
- **Why vendor:** upstream SSE reconnect is session-global (`SseRetryPolicy::retry` + `tokio::time::Sleep` only). `mcode-mcp` needs per-stream wait state so concurrent common/request streams cannot steal, skip, or stack backoff.
- **Workspace:** path dependency from `[workspace.dependencies] rmcp`. It is listed in workspace `exclude` (in-tree path deps are otherwise implicit members). `cargo test --workspace` must not run upstream integration tests; several `[[test]]` targets (for example `tests/test_deserialization.rs`) read fixtures that were intentionally not vendored.

### Modified files

Relative to stock 3.1.4:

| File | Change |
| --- | --- |
| `rmcp/Cargo.toml` | Remove `resolver = "2"`; disable library unit tests and doctests; allow `dead_code`, `exhaustive_enums`, `exhaustive_structs`, `result_large_err`, `too_many_arguments`, and `question_mark` for this vendored crate. |
| `rmcp/build.rs` | Replace upstream git-hook setup with a no-op so building the path dependency cannot configure the checkout's `core.hooksPath`. |
| `rmcp/src/lib.rs` | Add crate-level `allow` attributes for `dead_code`, `result_large_err`, `too_many_arguments`, and `question_mark`, matching the vendored lint policy. |
| `rmcp/src/transport/common/client_side_sse.rs` | Add `SseStreamRetryHooks`, `SseStreamContext`, `SseRetryPolicy::stream_context`, `SseStreamReconnect::bind_stream_context`. `SseAutoReconnectStream` notes live, waits via `policy_retry_wait`, and uses `BoxFuture` so cancel can drop stream-local state. |
| `rmcp/src/transport/streamable_http_client.rs` | `StreamableHttpClientReconnect` stores `SseStreamContext`, implements `bind_stream_context`, and applies `extra_get_delay()` before each reconnect GET (covers the SDK-skipped first GET after a mid-stream error). |

No other vendored source files are intentionally patched. Do not format, clippy-fix, or rewrite `vendor/rmcp` except to re-apply these vendoring and reconnect changes.

### Reconnect hooks

`mcode-mcp` implements the hooks on `StreamRetryToken` / `ConfiguredSseRetry`:

- `stream_context()` → a fresh token per `SseAutoReconnectStream`
- `note_live()` when the inner SSE stream is connected
- `policy_retry_wait(n)` → RAII wait; dropping the future (including never-polled) clears pending on that token only
- `extra_get_delay()` → consume pending/live for that token only

### Upgrade

1. Replace `vendor/rmcp` with the new crate sources (keep `version = "=…"` in workspace `Cargo.toml` in lockstep).
2. Re-apply the vendoring integration patch: remove `resolver = "2"`, disable the library's tests/doctests, and restore the listed lint allowances in `Cargo.toml`; replace `build.rs` with the no-op; restore the crate-level allowances in `src/lib.rs`.
3. Re-apply the two-file transport hook patch above; keep the public hook names unless `mcode-mcp` is updated in the same change.
4. Keep `vendor/rmcp` in workspace `exclude` and out of `members` (upstream tests/fixtures stay out of the repo gate).
5. Run `cargo test -p mcode-mcp` and `cargo test --workspace`.
6. Update this file (source tag, modified files) if the patch surface changes.
