# ProviderPack ABI: prepared request and canonical JSON

> 返回 [08-provider-pack-abi.md](08-provider-pack-abi.md)。本文件保留原 authority 的章节编号与规范效力。

## 5. Exact prepared request and completion input

### 5.1 Top-level and message fields

| local type | exact fields/variants |
| --- | --- |
| `prepare-input` | `provider-id:provider-id, route-id:provider-route-id, catalog-digest:catalog-digest, selection:model-selection, current-model:model-id, operation-id:provider-operation-id, request-id:request-id, turn-id:turn-id, system:list<string>, messages:list<message>, tools:list<tool-definition>, tool-choice:tool-choice, reasoning:reasoning, cache-retention:cache-retention, max-output-tokens:option<u64>` |
| `message` | `user(user-message) \| assistant(assistant-message) \| tool-result(tool-result-message)` |
| `user-message` / `assistant-message` | `blocks:list<user-block>` / `blocks:list<assistant-block>` |
| `tool-result-message` | `call-id:string, blocks:list<tool-result-block>, is-error:bool` |
| `user-block` | `text(text-block) \| image(image-view)` |
| `assistant-block` | `text(text-block) \| reasoning(reasoning-block) \| tool-call(tool-call-block)` |
| `tool-result-block` | `text(text-block) \| image(image-view)` |
| `text-block` | `text:string` |
| `reasoning-block` | `kind:reasoning-kind, text:string, proof:option<reasoning-proof-view>` |
| `reasoning-kind` | `thinking \| summary` |
| `reasoning-proof-view` | `stamp:proof-stamp, source-request-id:request-id, source-turn-id:turn-id, source-content-index:u8, reasoning-kind:reasoning-kind, proof:list<u8>` |
| `tool-call-block` | `call-id:string, name:string, arguments:wire-json-document` |
| `tool-definition` | `name:string, description:string, input-schema:wire-json-document` |
| `tool-choice` | `unset \| auto \| none \| specific(specific-tool-choice)` |
| `specific-tool-choice` | `name:string` |
| `reasoning` | `unset \| disabled \| enabled(enabled-reasoning)` |
| `enabled-reasoning` | `effort:option<reasoning-effort>, budget-tokens:option<u64>` |
| `reasoning-effort` | `minimal \| low \| medium \| high` |
| `cache-retention` | `unset \| none \| request \| session` |
| `image-view` | `stamp:image-stamp, media-type:image-media-type, bytes:list<u8>, metadata:image-metadata` |
| `image-metadata` | `width:u32 (1..=16,384), height:u32 (1..=16,384), frames:u32 (1..=64)` |
| `image-media-type` | `png \| jpeg \| gif \| webp \| tiff` |

`prepare-input` 的 bounds 是 system `0..=1,024`、messages `0..=4,096`、tools `0..=1,024`、每个 message blocks `1..=4,096`，且整个 input logical charge `<=8 MiB`。每个 system part 是 `Safe(64 KiB)`，每个 tool description 是 `Safe(64 KiB)`；`text-block.text` 与 `reasoning-block.text` 各自是 `Safe+(64 KiB)`；`tool-definition.name`、`specific-tool-choice.name`、`tool-call-block.name` 与 output `tool-call-start.name` 都是 `Label(128)` 并用 exact bytes 比较，不做 case/Unicode normalization；`tool-result-message.call-id`、`tool-call-block.call-id` 与三个 output tool events 的 call-id 使用 request/turn 的同一 `1..=128` ASCII tracking grammar。tool definition name 与 call ID 在各自集合全局 unique；每个 call name 必须引用 exactly one definition，但同一 definition 可被不同 call ID 引用；tool arguments 与 input schema root 必须是 object。present 的 `max-output-tokens` 与 reasoning budget 至少 1，且 max output 不得超过 catalog limit。

每个 `image-view.bytes` 为 `1..=8 MiB`；这是 typed input 的 pre-transform admission cap，不承诺其 text encoding 一定适配 8 MiB wire-body cap。`image-metadata.width` 与 `height` 各为 `1..=16,384`，`frames` 为 `1..=64`，且 selected catalog entry 的 canonical `input-modalities` 必须包含 `image`（`unknown` 不满足 image capability）。Host 在 lowering 前始终按 stamp 解析 stamped sidecar，并逐 field 验证 media、完整 bytes、width、height、frames、capability 与上述 bounds；不接受 URL/path/base64 input。metadata 三个 scalars 都只是 validation-only：各自有 zero mandatory output debt，contract 可通过既有 scalar source 对同一 image occurrence 的每一项引用零次或一次；引用时必须 `checked-u32` 且 output canonical value 与 Host-validated sidecar exact match，duplicate use reject，未引用合法，适配没有 dimension/frame wire fields 的 body。

本文每一个 raw image binding 都仅指 bytes/media payload accounting，不强迫 metadata 输出。每个 image occurrence 的 bytes 与 media 必须在 prepared tree 通过同一个 `image-data-uri` composite 或两个 separate scalar exact-once account；trusted adapter 若输出 base64，decode 后必须 byte-identical 于 Host-verified bytes，并按第 6/7/8 节在 Host allocation 前验证 checked expansion 与 remaining derived-string/wire-body budget。所有单项 bounds 仍受 containing value logical-charge cap 限制；不能 fit 的 admitted image 返回 `limit` 且 zero credential/network。

### 5.2 Message reducer and option semantics

request-wide reducer 是 `idle | pending(call-id,name,sealed-definition queue)`。assistant message 中的 complete `tool-call-block` 按 declaration order 入 queue；紧随其后的 messages 必须恰好按 queue 顺序各提供一个 nonempty `tool-result-message`。queue 未空时禁止 user/assistant、extra/missing/duplicate/out-of-order result 和 request end；idle tool-result、orphan/crossed call 都 reject。每个 input call ID 全局 unique，且每个 call name 解析到唯一 sealed tool definition；arguments 已是 duplicate-free、depth-bounded、object-rooted `wire-json-document`。只有 reducer 将当前 result `call-id` exact-match 到 queued preceding call 后，Host 才为该 result 构造两个 independent derived projections：`tool-result-status` 从 validated `is-error=false|true` 唯一映射为 `success|error`，`tool-result-name` 取 matched call 的 exact name。`tool-result-name` 不是 guest field，也不再次消费 earlier `tool-call-name`；它必须同时 byte-equal queued call name 与 sealed registry definition 的 `Label(128)` bytes。crossed/orphan/unknown call 或 name mismatch 在 Adapter expansion、credential lookup 和 network 前 reject。

`tools` nonempty、任何 input tool call、任何 output tool event 或 terminal `tool-use` 都要求 selected catalog entry 的 `tools=supported`；`tool-choice` 的 `unset|auto|none|specific` 各有 adapter contract 中唯一 encoding：unset 的相关 paths 必须全部 absent，不能用 null/auto/empty tools/provider default；auto 要求 tools nonempty 且 `tools=auto-choice=supported`；none 要求 `none-choice=supported`；specific 要求 `tools=specific-choice=supported`、`tools.len=1`，且 sole input tool name 与 payload 完全一致。reasoning `unset` 使 related paths 全 absent，`disabled` 与 `enabled` 不可相互降级；任何 reasoning input/output 要求 `reasoning=supported`，present effort 另要求 `effort=supported`，present budget 另要求 `budget=supported`，且 effort/budget 精确保留。cache `unset` 是 absent，`none|request|session` 各自保留 exact mode。`max-output-tokens=None` 是 absent，`Some(0)` reject；present value 要求 catalog limit 存在并不得超过它。unknown/unsupported 均不满足任何 supported check。

Host 为 decoder 保留 sealed input tool registry、exact `tool-choice` 与 selected catalog capabilities。每个 output `tool-call-start.name` 必须 byte-identical 匹配 registry 中恰好一个 definition，并继承其 `Label(128)` bound；call ID 使用上述 tracking grammar且 request-wide unique。input tools 为空、catalog `tools!=supported` 或 choice=`none` 时任何 tool event reject；choice=`specific(name)` 时每个 start 只能使用该 sole name；`auto|unset` 也只能使用 registry member，绝不接受 provider 新名字。重复 start/call ID、unknown/crossed name、name 在 start/end 间变化、没有完整 arguments object 的 call 都在 normalized event publication 前拒绝；`tool-use` terminal 要求至少一个 complete call，`stop|length` 到达时不得有 open call。

## 6. Prepared JSON tree and canonical text

`wire-json-document` 是 family-local flat typed tree，不是 `body-json:string`：`root:u32, nodes:list<wire-json-node>`。node 只有 `null-value | boolean-value(bool) | number-value(string) | string-value(string) | array-value(wire-json-array) | object-value(wire-json-object)`；`wire-json-array{items:list<u32>}`；`wire-json-object{fields:list<wire-json-field>}`；field 为 `{key:string,value:u32}`。prepared body root 必须 object，logical charge `<=8 MiB`，nodes `1..=262,144`，depth `<=64`；root explicit 且为 final node、all reachable、non-root exactly one parent、child index lower than parent，禁止 shared child/cycle/missing node/trailing content/compression/multipart。

number text `1..=128` ASCII bytes，exact grammar 为 `0|-?(?:[1-9][0-9]*(?:\.[0-9]*[1-9])?|0\.[0-9]*[1-9])(?:e-?[1-9][0-9]*)?`。因此 negative zero、zero exponent、leading zero、plus sign、uppercase `E`、exponent zero、trailing fractional zero、NaN/infinity 都 reject。object key 是 UTF-8 byte-sorted unique `Safe(256)`。ordinary string value 是 `Safe(64 KiB)`；只有被 validated contract node 的 `base64-standard-*|data-uri|join-lf|canonical-json-string|mistral-tool-result-content` transform 绑定、并由 Host 从 exact source 重算 byte-equal 的 value 才是 `DerivedSafe(8 MiB)`。`canonical-json-string` 的 transformed value 只能是 Host 从 validated arguments tree 生成的 canonical text；guest supplied text 绝不能取得该 provenance。`DerivedSafe` 使用同一 UTF-8/control/bidi rules，但不沿用 64 KiB ordinary cap；它仍同时受 prepared tree `8 MiB` logical charge、单次 operation retained-data cap 与 serialized wire body `8 MiB` cap。未绑定 transform 的 guest string、identity/enum/constant output、Host-derived `tool-result-status|tool-result-name` 与 generic strict text input 不能声明 derived provenance。

Host emit 的 canonical JSON 保留 stored member order 和 number token，不做 float conversion，只直接写 UTF-8 scalar；只 escape quote/backslash 与允许的 TAB/LF 为 `\t|\n`，不 escape slash，不输出 `\u`。strict text input 解码后再次应用 tree/depth/node/Safe rules；测试 `\u0000`、`\r`、`\u202e`、ordinary decoded string 64 KiB+1、无合法 transform provenance 的 larger string 和 key 256+1 必须拒绝。derived value 即使 decoded bytes `<=8 MiB`，若 escaping/punctuation 后使 final sink 达到 8 MiB+1 仍为 `limit`。
