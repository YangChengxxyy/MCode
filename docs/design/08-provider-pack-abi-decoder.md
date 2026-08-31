# ProviderPack ABI: decoder, limits and artifact gates

> 返回 [08-provider-pack-abi.md](08-provider-pack-abi.md)。本文件保留原 authority 的章节编号与规范效力。

## 10. Response decoder automaton and normalized events

### 10.1 Frame and pull types

| local type | exact fields/variants |
| --- | --- |
| `response-frame` | `head(response-head) \| data(list<u8>) \| end` |
| `response-head` | `status:u16, media:response-media` |
| `response-media` | `json \| event-stream` |
| `decoder-pull` | `events(list<normalized-event>) \| need-frame` |
| `normalized-event` | `text-delta(text-delta) \| reasoning-delta(reasoning-delta) \| reasoning-proof(reasoning-proof) \| tool-call-start(tool-call-start) \| tool-arguments-delta(tool-arguments-delta) \| tool-call-end(tool-call-end) \| completed(completion-terminal) \| failed(provider-error)` |
| `text-delta` | `content-index:u8, text:string` |
| `reasoning-delta` | `content-index:u8, kind:reasoning-kind, text:string` |
| `reasoning-proof` | `content-index:u8, kind:reasoning-kind, proof:list<u8>` |
| `tool-call-start` | `content-index:u8, call-id:string, name:string` |
| `tool-arguments-delta` | `content-index:u8, call-id:string, delta:string` |
| `tool-call-end` | `content-index:u8, call-id:string` |
| `completion-terminal` | `reason:completion-reason, reported-model:option<model-id>, usage:usage` |
| `completion-reason` | `stop \| tool-use \| length` |
| `usage` | `input-tokens:option<u64>, output-tokens:option<u64>, cache-read-tokens:option<u64>, cache-write-tokens:option<u64>` |

Host decoder reducer 的完整 state set 是 `initial-pull | need-head | draining-body | need-body | draining-after-end | closed`；`closed` 另存唯一 close cause 与 outer terminal。method `Ok` transition 如下，表外 call/result/frame 一律 protocol close：

| current state | legal Host call | required guest result/input | next state and Host action |
| --- | --- | --- | --- |
| `initial-pull` | `pull(limit)`，`limit=1..=16` | exactly `Ok(need-frame)` | `need-head`；这是唯一 initial transition |
| `need-head` | `push(head)` | status `200..=599`、media `json\|event-stream`，guest returns `Ok(accepted)` | `draining-body`；seal the sole head |
| `draining-body` | `pull(limit)`，`limit=1..=free-capacity` | `Ok(events(batch))`，batch `1..=limit`、nonterminal | 保持 `draining-body`，enqueue 后等 capacity |
| `draining-body` | `pull(limit)` | `Ok(need-frame)` 且无 decoder-pending event | `need-body`；仅 output queue empty 后可读 network |
| `need-body` | `push(data)` | data `1..=64 KiB`，guest returns `Ok(accepted)` | `draining-body`；计入 frame/byte caps |
| `need-body` | `push(end)` | guest returns `Ok(accepted)` | `draining-after-end`；seal the sole end，不再读 network |
| `draining-after-end` | `pull(limit)` | `Ok(events(batch))`，batch `1..=limit`、无 terminal | 保持 `draining-after-end`，enqueue 后继续 drain |
| `draining-after-end` | `pull(limit)` | `Ok(events(batch))`，exactly one terminal 且为 batch last item | receipt 时 CAS `closed`，撤销 leases，发布该 sole terminal |
| any nonclosed | active method returns `Err(provider-error)` | closed stable error only | method receipt 时 CAS `closed`；该 frame/batch 不被接受，合成 exactly one stable failed outer terminal |
| any nonclosed | cancel/deadline/trap/cap breach/protocol violation | no further guest result trusted | winner CAS `closed`，关闭 transport、撤销 leases、合成 one stable outer terminal |
| `need-head\|need-body` | transport EOF/cancellation before required frame/end | no decoder call | winner CAS `closed`，关闭 transport并合成 one stable outer terminal |
| `closed` | no `pull`/`push` permitted | n/a | zero post-close decoder guest calls；CAS loser只观察 closed，不发布第二 terminal |

`push` 只在 prior `need-frame` 所形成的 `need-head|need-body` 合法；`pull` 只在 table 所列三个 draining/initial state 合法。`need-frame` 在 `draining-after-end` 非法；events empty、limit+1、terminal before end、terminal nonfinal/repeated、head/data/end 顺序错误都触发 protocol close。Host 在 push 前完成 transport decompression；恰一个 head 先于 data，恰一个 end 后不再有 frame。最多 1,024 data frames；2xx cumulative data `<=16 MiB`，non-2xx cumulative error data `<=64 KiB`，这些不能扩大 Host retained-data budget。

`pull` 没有 `finished` variant。每个 events batch logical charge `<=1 MiB`。每次 successful push 后 Host 按表 drain 到 `need-frame` 或 terminal batch；terminal receipt 后绝不再次调用 decoder。method Err 本身是唯一 guest failure receipt，不转换成 decoder event；若 output queue 已满，Host 只在 private closed state 保留 synthesized terminal，等已接受 items 被消费后发布，不再调用 guest。close/lease revocation 由单一 atomic CAS 线性化，cancel、EOF、trap、deadline、protocol、method Err 与 terminal 的 loser 都不得产生 effect 或 guest call；destructor 仅 bounded best effort，不计入 decoder protocol call。

### 10.2 Event reducer and backpressure

non-2xx 可消费 bounded error data，但不输出 text/reasoning/tool event，end 后只输出一个 stable failed terminal；status/header/error body 不进入 normalized output 或 diagnostics。每个 normalized event `<=128 KiB`；text/reasoning/tool-argument delta 必须 nonempty `Safe+(64 KiB)`；proof `1..=64 KiB`，所有 proofs `<=256 KiB`；所有 events 共用 count `<=65,536`、charge `<=8 MiB`。content-index 是 `0..=63`，最多 64 个，必须 contiguous/nondecreasing；每 index 只能绑定一种 text/reasoning/tool kind，advance 后不可重现。text/reasoning index 保持既有 nondecreasing reducer；一个 tool content-index 则精确表示 one and only one call。

Tool reducer 对 next contiguous index 的 state 是 `unused | open(call-id,name,delta-count,bytes) | sealed`。`tool-call-start` 只能在 `unused` 以 next index 打开 sole call，call ID 是 `1..=128` tracking string且 request-wide unique，name 按 sealed registry 校验。`open` 期间唯一合法的 content event 是同一 content-index、同一 call-id 的 nonempty `tool-arguments-delta`；禁止 second start（无论 ID 是否相同）、text/reasoning/proof event、另一 call interleave、index advance 或 terminal。必须收到 `1..=16,384` 个 delta 后，exact same index/ID 的 `tool-call-end` 才把该 index `sealed`；zero delta reject，文本 `{}` 是一个 nonempty delta并可形成最小 object。seal 后所有 subsequent content events 必须使用 next contiguous index，sealed index 永不重现；late delta/end 都 reject。多个 calls 因而各占 distinct contiguous content-index，不能让第二 call 复用 prior index。decoder 可以在自己的 bounded private state 中 buffer/reorder fragmented wire chunks 来产生此 normalized order，但发布给 Host 的 events 不得暴露 interleaving 或违反 reducer。

每 call cumulative delta `<=1 MiB`，all calls `<=2 MiB`。end 时 strict-parse 一个 duplicate-free、depth `<=64`、nodes `<=16,384`、logical charge `<=1 MiB` 的 canonical JSON object。proof 只能在当前 reasoning index advance 前出现一次，且要求 catalog proof capability；orphan/cross-index reject。所有这些 scalar/list/count/aggregate 分别覆盖 0/1/N/N+1。tool fixtures 另覆盖 second call same index、cross-call interleave、open 时 advance、sealed index 的 late delta 与 late end。

Host normalized-output queue capacity exactly 16。Host 每次只用 `limit=free-capacity` 调 decoder；capacity zero 时不 pull、不读 network、不 push frame。enqueue 一个 batch 后等待 consumer capacity；即便 decoder 已返回 `need-frame`，下一次 network read 也只能在 output queue empty 后进行。batch 0/limit/limit+1、queue 17、terminal 非 final、free capacity 0/1/16、head-then-disconnect 和 byte-at-a-time framing 都是 fixture。

### 10.3 Protocol failures and cleanup

head 前 data、head 重复、end 重复、data/end after closure、无 prior `need-frame` push、missing head/end、terminal before end、terminal not last、repeated terminal、data cap N+1、late event、pull after Host close 都是 protocol failure。cancel、EOF、trap、deadline、cap breach 或 protocol failure 按 10.1 table 的 atomic close 规则关闭 transport并合成一个 stable outer terminal；decoder destructor 不负责 correctness。只有该 table 明列的 call/state/result transition 合法。

## 11. Logical limits, redaction and T7 gates

Provider 使用 checked-`u64` logical charge：`bool/u8/s8=1`、`u16/s16=2`、`u32/s32/char=4`、`u64/s64=8`、string `4+UTF-8 bytes`、list `4+sum`、record/tuple field sum、enum/variant/option/result `4+active payload`、flags（最多 32 cases）为 4、WIT reference 为 4。ABI 只接受 memory32；memory64、f32/f64、future、stream、error-context 和未列 type static reject。Host-to-guest lowering 前、canonical lift fuel 后和 semantic validation 后分别守 bounds；任何 overflow 无 side effect。

所有 current Manager/Feature/Provider component 编译前 `<=4 MiB`，超限 artifact 在 measured Host policy change 前不可执行。T7 binary-only scan 用 locked `wasmparser` 检查 exact nested types、core memory/table min/max/count，拒绝 memory64/shared/atomics/threads；`resources_required()` 不能替代这些 checks；不创建 Store。T8 每 Pack one Store，启用 Wasmtime `async`、`consume_fuel` 与 epoch interruption；instantiate、guest call 与 waiting Host function 只走 async API。每个 Store 使用 `Store::set_hostcall_fuel(16 MiB)` 而不是默认 `128 MiB`；fuel 在每次 Host call 重置。每 memory `64 MiB`、最多 2 memories、每 table `65,536` elements、最多 4 tables、64 core instances；aggregate memory `128 MiB`、table `65,536`；Host resource table capacity `4,096`，admission live resources `4,096`、open operations `1,024`，并在 terminal/cancel/close 时释放 admission。每 operation retained guest-derived data `8 MiB`，每 Pack `64 MiB`；每个 guest segment 为 `100,000,000` fuel units 的 deterministic budget，不把它解释为 instruction count。Host supervisor 持有 monotonic `<=2s` outer deadline；epoch ticker target `<=10ms`，实际 interruption 发生在下一次 epoch check，不承诺 `2.010s` 或 scheduler-independent wall-time SLA；Host exchange wait 有独立 finite deadline，resume 只使用剩余 segment/deadline budget。

`prepare-input` 与 prepared body 各自 charge `<=8 MiB`；单个 decoder event `<=128 KiB`，decoder batch `<=1 MiB`，headers final `<=32/16 KiB`。每个 outer operation/decoder 最多累计 65,536 次 pull；N+1 由 Host 关闭并合成一个 stable failure，之后 zero guest calls。其余 decoder limits 按本文第 10 节。Limit/shape/invariant 映射 stable `limit|invalid-argument`；trap/deadline/cleanup 映射 `cancelled|unavailable|failed`，不带 source string。error/status/header/body/credential/proof raw bytes 不进入 log/diagnostic。

T7 pure/static gates：source WIT 与 current LF golden byte-identical；world zero imports 且 exactly one exported `provider-api` interface，该 interface exactly four freestanding functions plus one decoder resource、无其他 member/export；catalog digest/paging/cache relation、alias mapping、一个 exhaustive trusted dummy AdapterContractV1/schema validator、JSON tree、message reducer、canonical arguments string、Mistral tool-result content composite、per-wire tool-result status/name projection 与 accounting、ToolChoice/reasoning/cache、image/proof/header、sole trusted dummy context-counter registry entry、decoder/backpressure 全用 dummy in-memory fixture；fixture 可用 frozen wire IDs 驱动 closed validation branches，但不调用 network、credential、signature activation、Store、real adapter、real counter 或 real wire，也不包含 ten real wire contract/golden。

## 12. Stage ownership and artifact slice

| stage | exclusive responsibility | T7 不声称 |
| --- | --- | --- |
| T7 | current artifact/parser/static preflight、pure validators、redaction和bounds | Provider runtime、signature activation、credential/network、real wire PASS |
| T8 | Store/limiter/fuel/epoch、generation、decoder resource、cancel/reload/quiescence、outer terminal races | Provider product effect PASS |
| T10 | signed manifest/source trust、authority/profile/header binding、activation；不读取或注入 vault | credential injection PASS |
| T11 | Provider runtime、signed catalog metadata/cache、route/adapter binding、ten context-counter profile refs/Host implementations、vault/grant/injection、Pi ten-wire goldens | T7 real wire/counter PASS |
| T17/T18/T19 | frozen Feature Web/MCP/Usage Host effects | Provider 代替这些 family 的 Host methods |

Parseable artifact slice 必须提供 `mcode:provider-pack/provider@0.0.1` source WIT、byte-identical current LF `provider_current.wit`、`provider_current.jsonl` semantic golden 和 exact static preflight fixture。没有该 parser-checked artifact，本文仅冻结目标 authority，不是 source/golden/runtime 完成声明。若未来发生 breaking change，必须整体发布新的 package/world/current golden；当前 surface 不加解释分支。
