# mcode-compaction

Private MCode host foundation for context compaction. This crate is unpublished and is **not** an extension SDK.

## Closed-core boundary

- The host passes the currently selected `dyn Provider` and model id. The crate never selects a vendor, fails over, or switches providers.
- There is no compactor strategy trait, callback, hook, registry, interceptor, or replacement API.
- Plugin code must never receive `CompactionInput`, the serialized transcript, provider request, or candidate output.
- `Message::Custom` payloads never enter the summary request. Values before the cut are preserved verbatim when rebuilding context rather than replaced by model prose.
- `TokenEstimator` is a concrete conservative fallback, not an implementation trait. Its text estimate equals the UTF-8 byte length of the input, which is a provable upper bound for every provider tokenizer (each emitted token covers at least one input byte; byte-fallback encoders that may spend up to four tokens on one four-byte scalar are fully covered). Trusted provider-native counts can be attached to model-visible `CompactionMessage` values by the host; custom messages always count as zero LLM tokens.
- The prior summary is a separate bounded input whose tokens reduce the transcript budget. Serialization favors messages nearest the cut and records the count of wholly omitted older messages.
- A transcript message counts as included only when its segment carries meaningful body content or one complete auditable truncation marker plus the closing `<<<END MESSAGE>>>` marker; header-only segments are omitted instead of silently counted. Tool-result bodies are bounded by a writer that shares the enclosing segment budget, so the outer writer never truncates them a second time, and every `ToolResultTruncation` record (including `serialized_chars`) describes the final emitted output. Untrusted body text that spells the structural `<<<END MESSAGE>>>` line is escaped (`<<<END-MESSAGE>>>`, length-preserving), so it can neither forge the closing-marker audit nor end a segment early. Body rendering never allocates more than the enclosing writer's remaining character budget (rendered lengths are counted without allocation), so oversized message or tool bodies cannot amplify memory before truncation applies. When even the newest message cannot be serialized with a complete audit marker, serialization fails closed instead of emitting an unauditable transcript.
- Deterministic host paths persist through a tagged exact JSON representation: readable UTF-8 when possible, raw Unix OS bytes base64-encoded when not, and raw Windows UTF-16 code units (including unpaired surrogates) base64-encoded when a path is not UTF-8-representable. Round trips are exact, distinct raw paths never collide into one lossy spelling, and the emitted JSON stays valid.

## Transactional actor/session integration contract

A later session/actor integration should follow this order:

1. Snapshot the active branch as `CompactionInput`, including entry ids, token counts, the prior summary as a separate field, and deterministic host details.
2. Call `compact_context` with the session's current provider and cancellation token without changing `AgentState` or `SessionStore`.
3. On success, re-check the branch tip, source message count, and cut ids against the snapshot. Reject stale output.
4. Call `rebuild_context` to produce a candidate, but do not install it yet.
5. Inside one serialized actor critical section, append a future versioned compaction session entry containing `CompactionOutput`/`CompactionDetails`; only after that append succeeds, install the already-validated candidate and advance the branch tip. Replay should rebuild from the recorded cut and summary rather than rewriting old JSONL lines.
6. On planning, provider, validation, append, or cancellation failure, leave in-memory state and the active branch unchanged.

No session entry or `Message` variant is added in this foundation crate. Session replay must treat the typed deterministic sidecar—not model prose—as authoritative for files, commands, todos, and background operations.

## Adaptive trigger foundation

`AdaptiveTriggerPolicy` (JSON via serde, camelCase fields; no TOML surface anywhere) drives the sealed adaptive trigger:

- **Advertised vs. effective context.** The effective working context is `min(advertised, session clamp, maxWorkingTokens, 400_000)`. The 400k cap is a hard invariant: user or model settings may lower `maxWorkingTokens`, never raise it, and the runtime `TriggerInputs::session_context_cap_tokens` clamp can only lower the cap for the current session.
- **Policy fields.** `triggerRatio` (default `0.82`), `targetRatio` (default `0.55`), `maxWorkingTokens` (default and hard cap `400000`), `reserveTokens` (required baseline input for the automatic trigger, no schema default), and `minGainRatio` (default `0.2`). Validation rejects non-finite ratios, values outside `(0, 1)`, `targetRatio >= triggerRatio`, a trigger-to-target gain below `minGainRatio`, a cap above 400k, or a reserve that leaves no working room.
- **Trigger formula.** `max(1, min(floor(effective * triggerRatio), effective - adaptiveReserve))` computed with exact integer arithmetic (the decimal the JSON spells is the ratio used, so `floor(300_000 * 0.82)` is exactly `246_000`). The adaptive reserve adds bounded allowances for the requested maximum output (capped, never the model's full claimed output) and tool/schema overhead to the policy baseline. The one-token floor keeps a legal sub-token ratio product from yielding a zero threshold, which would fire at zero usage and immediately re-trigger after compacting to a zero target. The post-compaction hysteresis target is `min(floor(effective * targetRatio), threshold - ceil(threshold * minGainRatio))`: it stays strictly below the final threshold — including thresholds lowered by the reserve or the uncertainty discount — and keeps the minimum relative gain, so a compacted session never re-triggers immediately. The `minGainRatio` validation uses the same decimal-exact semantics, so boundary configurations such as `0.82/0.656/0.2` (exactly 20 percent gain) validate.
- **Provider wiring.** `TriggerInputs` is the explicit input interface: trusted `providerReportedTotalTokens` replaces the host estimate whenever present (`evaluatedUsedTokens` equals the trusted report, never a `max` of both values), and `toolOutputTokens` plus the absence of a provider report apply a bounded (≤ 10%) deterministic threshold reduction for estimation uncertainty. No model self-assessment ("the model feels dumber") participates in the decision.
- **Retry ownership.** `evaluate_trigger` is pure and stateless. A host that learns a smaller provider context length may lower the session clamp and run at most one compact-and-retry at the upper layer; this crate never retries on its own.

Reference points covered by tests: advertised 1M → effective 400k, trigger 328k; 300k → 246k; 272k → 223_040; 128k with dynamic reserve → 99_328.

## Current provider API limitation

`mcode_llm::Request` currently has no maximum-output-token field. The compactor therefore states the configured ceiling in its private user message and rejects summaries above that ceiling before returning output. Only a natural `StopReason::Stop` completion is accepted; length-limited, error, and tool-use completions are rejected. When the provider request type gains a native output limit, the host integration should wire the same `CompactionPolicy::max_summary_tokens()` value without changing compaction semantics.
