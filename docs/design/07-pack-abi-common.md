# FeaturePack ABI: common exact surface

> 返回 [07-pack-abi.md](07-pack-abi.md)。本文件保留原 authority 的章节编号与规范效力。

## 2. Common exact surface, ownership and authority

### 2.1 Exported operation signature

本文所有 type table 使用同一冻结记法，且每行只属于该节的 family interface：逗号分隔的 `name:type` 是 record；variant 中含 payload 的 case 写 `case(type)`，payload-free case 写 bare `case`；仅 bare cases 的 `a \| b` 是 enum；`flags ...` 是 flags；`alias of ...` 是 alias。不存在隐式 structural type 或跨 family name resolution。WIT 关键字作为 case identifier 时使用 lexical escape：Resources catalog 的 semantic case `resource` 写 `%resource`，Todo request/task-read 的 semantic case `list` 写 `%list`，MCP JSON/schema 与 Usage wire/schema 的 semantic case `string` 写 `%string`；`%` 只属于 WIT 词法转义，semantic case name 仍分别是 `resource`、`list`、`string`。canonical Todo `OperationId` `list` 保持不变；`list<T>`、`string` 等 built-in type use 绝不转义。

11个 exported resources逐一为：`resource session-operation { pull: func() -> session-pull; }`、`resource compaction-operation { pull: func() -> compaction-pull; }`、`resource resources-operation { pull: func() -> resources-pull; }`、`resource ask-operation { pull: func() -> ask-pull; }`、`resource todo-operation { pull: func() -> todo-pull; }`、`resource web-operation { pull: func() -> web-pull; }`、`resource mcp-operation { pull: func() -> mcp-pull; }`、`resource usage-operation { pull: func() -> usage-pull; }`、`resource subagents-operation { pull: func() -> subagents-pull; }`、`resource workspace-operation { pull: func() -> workspace-pull; }`、`resource ui-operation { pull: func() -> ui-pull; }`。各interface只export其local `invoke: func(request: family-request)->result<own<family-operation>,family-error>`；第3–13节冻结exact names/signatures。exported `own<family-operation>` 严格由 Pack 创建并转给 Host；pull variant恰有pending/progress/complete/failed且无第二channel。operation resource 是唯一允许由 Pack export 的业务 resource。Host 关闭后不再调用 guest；稳定 ID/cursor/revision/reservation/result 都是 scalar/value projection。除此之外，`own` 只用于 imported Web/MCP/Usage exchange，严格由 Host 创建并转给 guest。destructor 是 bounded best effort，绝不授予/维持 authority 或承担 correctness；trap/hang 时 Host dispose Store。

### 2.2 Exact Host imports and ownership

| world | exact imported methods | method result and ownership |
| --- | --- | --- |
| `session` | `load-ledger(request: ledger-read)`；`commit-ledger(mutation: ledger-mutation)` | `result<ledger-page,session-host-error>`；`result<ledger-commit,session-host-error>`；均为 value result |
| `compaction` | `summarize(request: summary-request)` | `result<summary-output,compaction-host-error>`；value result |
| `resources` | none | Pack 不请求 Host effect |
| `ask` | `present(request: interaction-request)` | `result<interaction-output,ask-host-error>`；value result |
| `todo` | `load-tasks(request: task-read)`；`commit-task-event(mutation: task-mutation)` | `result<task-page,todo-host-error>`；`result<task-commit,todo-host-error>`；value result |
| `web` | `start-search(request: typed-search)`；`start-fetch(request: typed-fetch)` | `result<own<web-exchange>,web-host-error>`；exchange ownership 转给 guest，取消/authority 仍由 Host 保有 |
| `mcp` | `start-invoke(request: typed-invocation)` | `result<own<mcp-exchange>,mcp-host-error>`；exchange ownership 转给 guest，snapshot/transport authority 仍由 Host 保有 |
| `usage` | `start-refresh()` | `result<own<usage-exchange>,usage-host-error>`；source/transport authority 仍由 Host 保有 |
| `subagents` | `run-step(request: step-request)`；`recover-step(request: recovery-request)` | `result<step-output,subagents-host-error>`；`result<recovery-output,subagents-host-error>`；value result |
| `workspace` | `scan(request: scan-request)`；`apply-rollback(request: rollback-request)` | `result<scan-page,workspace-host-error>`；`result<rollback-output,workspace-host-error>`；value result |
| `ui` | none | Pack 不请求 Host effect |

`web-exchange`、`mcp-exchange`、`usage-exchange` 是同 family 的 bounded pull/backpressure resource。其 `pull()` 直接返回 family-local frame variant，不使用第二个 error channel；guest drop 仅是 best-effort signal，Host owns closure。仅 matching sole start method 成功一次后才分别授权 `web-exchange.pull`、`mcp-exchange.pull`、`usage-exchange.pull`；start 前、wrong start/case、duplicate start 或 foreign exchange 的 pull 均在 guest/transport 前拒绝。三者的 DTO 独立，但 reducer 同为 closed：success 是 `head -> bounded payload -> end`，`failed` 可替换尚未 terminal 的 suffix；`end|failed` 立即关闭，之后的 pending/pull/frame 都是 protocol failure。exchange `pending` 只允许出现在 first non-pending frame 之前；head 已接受后，async pull 必须等待下一 frame 或 `failed`，不能再返回 pending。duplicate/missing head/end、extra payload、cap/snapshot/schema/source mismatch 或 deadline 会关闭 Host work 并合成 exactly one stable outer terminal；one-frame buffering 和 no-read-before-pull backpressure 不因 family 改变。任何 stable ID、cursor、revision、checkpoint、sample 或 result 都是 record/value projection，不能用 resource borrow 作为持久 ID 或 result；这些 resource 不能是 OS/raw handle、可序列化 token、cross-world token、URL、socket 或 credential。

### 2.3 Operation authority and method allowlist

`OperationId` 是唯一 declarative operation authority key；`taskId` 只承担 runtime correlation。body 不携带 Pack ID。Host 在 family-body decode 之前复用 Manager gateway 的最多 128 项 declaration validator，绑定 canonical caller family、Manager identity、active generation、`OperationId`、expected request case 和 allowlist。strict decode 后，Host 才解析 selector/view，创建 immutable `ResolvedOperationBinding`，固定 Pack ID/source/version/hash/generation 与 singleton 或 multi-active role。Providers 只能从 Host route/catalog view resolve；Usage 只能从 bound canonical source/sample stamp resolve；UI 的 `render-runtime|handle-action` 只能 resolve sole runtime role，`resolve-theme` 只能 resolve Host-selected theme ID among N active themes；其余 family 只能 resolve sole active Pack。same-shaped guest selector 不能选择 Pack。

| family | request case -> canonical `OperationId` -> allowed Host method(s) |
| --- | --- |
| `session` | `create` -> `create` -> `commit-ledger`; `open` -> `open` -> `load-ledger`; `append` -> `append` -> `load-ledger+commit-ledger`; `read` -> `read` -> `load-ledger`; `fork` -> `fork` -> `load-ledger+commit-ledger`; `rewind` -> `rewind` -> `load-ledger+commit-ledger` |
| `compaction` | `assess` -> `assess` -> `none`; `summarize` -> `summarize` -> `summarize` |
| `resources` | `catalog` -> `catalog` -> `none`; `read` -> `read` -> `none`; `render-prompt` -> `render-prompt` -> `none`; `contributions` -> `contributions` -> `none` |
| `ask` | `present` -> `present` -> `present` |
| `todo` | `create` -> `create` -> `load-tasks+commit-task-event`; `get` -> `get` -> `load-tasks`; `list` -> `list` -> `load-tasks`; `set-status` -> `set-status` -> `load-tasks+commit-task-event`; `set-subject` -> `set-subject` -> `load-tasks+commit-task-event`; `set-description` -> `set-description` -> `load-tasks+commit-task-event`; `replace-dependencies` -> `replace-dependencies` -> `load-tasks+commit-task-event`; `set-owner` -> `set-owner` -> `load-tasks+commit-task-event`; `delete` -> `delete` -> `load-tasks+commit-task-event` |
| `web` | `search` -> `search` -> `start-search`; `fetch` -> `fetch` -> `start-fetch` |
| `mcp` | `servers` -> `servers` -> `none`; `tools` -> `tools` -> `none`; `invoke` -> `invoke` -> `start-invoke` |
| `usage` | `ingest` -> `ingest` -> `none`; `render-summary` -> `render-summary` -> `none`; `render-details` -> `render-details` -> `none`; `refresh` -> `refresh` -> `start-refresh` |
| `subagents` | `roles` -> `roles` -> `none`; `enqueue` -> `enqueue` -> `run-step`; `recover` -> `recover` -> `recover-step` |
| `workspace` | `checkpoint` -> `checkpoint` -> `scan`; `inspect` -> `inspect` -> `scan`; `rollback` -> `rollback` -> `scan+apply-rollback` |
| `ui` | `render-runtime` -> `render-runtime` -> `none`; `handle-action` -> `handle-action` -> `none`; `resolve-theme` -> `resolve-theme` -> `none` |

`none` 表示该 operation 不能调用 Host method，不表示可调用任意服务。Provider completion operation 必须另遵守 [08](08-provider-pack-abi.md) 的 grant/authority digest 规则。Web search/fetch、Usage refresh 是各自 outer binding；MCP invoke 只使用 signed MCP binding；Compaction `summarize` 不 mint network grant，而是在当前 route lease 与 completion grant 下启动 child completion。

### 2.4 Scalar/table authority

Host mint/reserve 所有 `ses1/br1/evt1/todo1/sub1/cp1` ID。WIT 中的 ID、cursor、revision、digest、reservation、fence 和 sample 只是 immutable typed projection。每次 guest 返回的同形 scalar 在下一次 import/effect 前再次 table-resolve；foreign、stale、missing、duplicate、wrong-generation、wrong-operation、wrong-reservation、cross-family 值在 guest/import/effect 前拒绝。reservation/sample private row 至少绑定 caller family、Manager/Pack identity、generation、declarative `OperationId`、同一 outer task 与 reservation/fence。cursor row 改为绑定 caller family、Manager/Pack identity、generation、declarative `OperationId`、完整 canonical query、snapshot/revision/head/digest 与 last key/offset；cursor 可由之后创建的 page operation 兑换，且该 operation 的 task ID 不要求等于签发 cursor 的 task ID。

| projection | Host table authority | guest 可见含义 |
| --- | --- | --- |
| stable ID | family + Manager/Pack + generation + owner row | 不可伪造的有界 ID 字符串；字符串本身不授予 authority |
| cursor/offset | caller/family/Pack/generation + declarative operation + complete query + immutable snapshot/revision + last position | later page operation 可 single-use redeem；严格前进；EOF 为 `None` |
| revision/head/fingerprint | source row + generation + expected-current fence | immutable comparison value；不能单独执行写操作 |
| reservation/sample | same outer task + payload digest + terminal/order/source stamp | single-use Host decision；guest 不能创建、re-seal 或复制 |

### 2.5 Checked logical size and pre-allocation order

语义 charge 使用 checked `u64`：`bool/u8/s8=1`，`u16/s16=2`，`u32/s32/char=4`，`u64/s64=8`，`string=4+UTF-8 bytes`，`list=4+sum(elements)`，record/tuple 为字段和，enum/variant/option/result 为 `4+active payload`，flags 只允许最多 32 cases 且 charge 为 4，`own|borrow` reference 各为 4。该 logical charge 只是 Host semantic policy，不声称等于 Wasmtime allocation。所需 ABI 是 memory32；memory64、f32/f64、future、stream、error-context 和未列类型 static reject。Overflow 是 `limit`，无副作用。

Host-to-guest 值在 lowering 前检查 raw byte/count、logical charge、component/memory/table/resource admission；guest export result 和 Host import argument 先经过 Wasmtime canonical lift fuel，再在 lift 后立即检查 length/order/unique/cross-field/table binding/logical charge，之后才能保留 state、chain、transport、diagnostic 或 credential flow。每个 Store 使用 `Store::set_hostcall_fuel(16 MiB)` 而不是默认 `128 MiB`；retained guest-derived data 为每 operation `8 MiB`、每 Pack instance `64 MiB`。

所有 current Manager/Feature/Provider component 编译前 `<=4 MiB`，超限 artifact 在 measured Host policy change 前不可执行。T7 binary-only preflight 直接使用 locked `wasmparser` 检查 exact component types、core memory/table min/max/count、memory64/shared/atomics/threads，并在 `Component::from_binary` 前拒绝；`resources_required()` 不能替代这些 checks；不创建 Store。T8 只用 Wasmtime `async`、`consume_fuel` 和 epoch interruption；instantiate、guest call 与 waiting Host function 只走 async API。T8 每个 Pack instance 独占一个 Store，并在 instantiate 前安装一个 composite limiter：每 memory `64 MiB`、每 table `65,536` elements、最多 2 memories、4 tables、64 core instances；wrapper 保证 aggregate memory `128 MiB`、table `65,536`，正常拒绝不保留 reservation，已批准增长后的分配失败则保守保留 reservation、毒化并 dispose Store。Host resource slots 在首次 push 前 `ResourceTable::set_max_capacity(4,096)`；另有 admission ledger，最多 4,096 live Host-visible resources、1,024 open operations，并在 terminal/cancel/close 时释放 admission；任何 limiter、aggregate reservation、resource-table cap 或 admission mechanism 缺失时 Pack execution 必须 disabled。

默认每个完整 request/result record（含所有 nested fields/lists）的 aggregate charge `<=1 MiB`；session、compaction、Provider input aggregate `<=8 MiB`；每个 progress `<=64 KiB`。各 field 的 individual bound 或其数学乘积即使更大也绝不放宽 family aggregate；先 checked 求每项、再 checked sum，overflow/aggregate N+1均整体 `limit` 且零副作用；Manager JSON envelope `<=64 KiB` 且不携带大型 Pack DTO。每一 field/list 做 `0/1/N/N+1` 和 checked overflow。每个 operation 只有 `100,000,000` fuel units 的 deterministic 总预算，不把它解释为 instruction count；initial guest call 与每次 awaited Host exchange 后的 resume 分别形成一个 guest-active segment，共享该 operation 的剩余 fuel，Host call、`pending` 或 resume 都不补充或重置 fuel。segment supervisor 为每个 guest-active segment 启动独立 monotonic `<=2s` deadline；该时钟从进入 guest 到 guest yield/return 在 segment 内绝不重置。guest yield 后 awaited Host exchange 的等待时间不计入 guest-active segment；resume 获得新的 `<=2s` segment deadline，但只能使用剩余 fuel、pull cap 与 family-specific monotonic end-to-end operation deadline。end-to-end clock 在 operation allocation 前记录 `started-at` 并生成 immutable absolute `deadline-at`，跨越全部 guest segment、`pending`、Host exchange wait 与 resume 且绝不重置；missing、zero、infinite、overflow、wrong-family 或 crossed identity/generation deadline binding 在 `invoke` 前拒绝。epoch ticker target `<=10ms`，实际 interruption 发生在下一次 epoch check，不承诺 `2.010s` 或 scheduler-independent wall-time SLA。任一 guest-segment、Web page 或 end-to-end deadline 到达都竞争同一 close CAS；winner 立即关闭 operation/exchange，deadline 后的 frame、resume、import、effect 或 terminal 均无 late effect。limit/shape/invariant 映射为 family `limit` 或 `invalid-argument`；trap/deadline/cleanup 只映射为 stable outer `cancelled`、`unavailable` 或 `failed`，不回传 source text。

非 Web family 的 exact duration 来自 T8 Host 启动时必需且进程内 immutable 的 closed `FeatureDeadlinePolicyV1` snapshot；其 exact record 为 `{session-ms:u32, compaction-ms:u32, resources-ms:u32, ask-ms:u32, todo-ms:u32, mcp-ms:u32, usage-ms:u32, subagents-ms:u32, workspace-ms:u32, ui-ms:u32}`，每个字段是 `1..=u32::MAX` milliseconds，无 default、extra field、fallback 或 runtime extension。operation row 绑定 family、declarative `OperationId`、Manager/Pack identity+generation、同一 immutable policy snapshot、`started-at` 与 checked `deadline-at=started-at+duration`。Web 不读取该 snapshot，其 duration 只来自本 operation 已接受且 identity/generation/digest matched 的 `WebAuthorityBindingV1.deadline.total-ms`。

| family | exact end-to-end duration source |
| --- | --- |
| `session` | `FeatureDeadlinePolicyV1.session-ms` |
| `compaction` | `FeatureDeadlinePolicyV1.compaction-ms` |
| `resources` | `FeatureDeadlinePolicyV1.resources-ms` |
| `ask` | `FeatureDeadlinePolicyV1.ask-ms` |
| `todo` | `FeatureDeadlinePolicyV1.todo-ms` |
| `web` | accepted `WebAuthorityBindingV1.deadline.total-ms` (`1..=70,000`) |
| `mcp` | `FeatureDeadlinePolicyV1.mcp-ms` |
| `usage` | `FeatureDeadlinePolicyV1.usage-ms` |
| `subagents` | `FeatureDeadlinePolicyV1.subagents-ms` |
| `workspace` | `FeatureDeadlinePolicyV1.workspace-ms` |
| `ui` | `FeatureDeadlinePolicyV1.ui-ms` |

### 2.6 Operation lifecycle

每个 operation 的 Host state 恰为 `open -> pulling -> terminal-received|cancelling -> terminal-published -> closed`。同一 operation 的并发 pull reject；`pending` 不带字段、不执行 effect，outer task 保持 open并结束当前 guest-active segment；它不暂停或重置 family end-to-end clock，也不补充 fuel/pull budget，下一次 resume 仅获得新的 `<=2s` guest-active segment deadline。awaited Host exchange 同样结束当前 segment，其等待时间位于 guest-active segment 之外但位于 family end-to-end clock 之内。outer-operation pull 与其最多一个 exchange 的 pull 各有独立 `65,536` counter，合计最多 `131,072`；任一 counter 的 N+1 都由 CAS 原子关闭 outer 并合成一个 stable failure。

cancel、reload、guest-segment/page/end-to-end timeout、stale generation、EOF、trap、protocol violation 与 guest terminal 竞争同一 close CAS。CAS winner 立即阻止新 guest call/resume、Host import/effect、operation/exchange pull，取消 Host work，invalidate 并 remove 全部 exchanges、views、leases、resource-table rows，再发布 exactly one terminal，最后才 bounded best-effort guest drop；destructor trap/hang 则 dispose Store。close 后的 late/queued data 不可观察且无 effect，guest drop 不承担 authority 或 cleanup correctness。clock-boundary fixtures 必须覆盖 guest segment、每个 `FeatureDeadlinePolicyV1` family field、Web operation/page/attempt clock 的 just-before/at/just-after deadline，证明 awaited exchange time不计 guest-active segment却计入 end-to-end、resume得到fresh `<=2s` segment但只剩余 fuel/pull/end-to-end budget。Web 组合 fixture 另固定小于上限的 `total-ms=5,000`、`per-page-timeout=1s`、`per-attempt-ms=200`，覆盖 attempt timeout 后 policy-permitted retry、page/operation timeout 禁止 retry，以及 retry 只刷新 attempt clock；每种 deadline/terminal CAS 竞态都必须 exactly one terminal、zero late import/effect/frame。T8 对每个 one-Store Pack 设置一个 async owner loop/mutex，序列化全部 guest entry；awaited Host import 的整个 call ownership 也不释放，因此任何时刻都没有 concurrent mutable Store access。

### 2.7 Shared pagination rule

所有 continuation 都是 family-local Host-table cursor view。除 Session event page 保持 immutable branch ledger order 外，page 必须 canonical sorted；`items.len<=limit`，continuation 严格前进，EOF 是 `None`。cursor 只可 single-use redeem；later page operation 必须与签发 row 的 caller/family/Manager/Pack/generation/`OperationId`/完整 query/snapshot binding 相同，但 task ID 可以不同。N+1、duplicate redemption、empty page plus continuation、skip、self-loop、rollback、missing page，以及跨 caller/Pack/generation/`OperationId`/query/branch/server/source/snapshot replay 都在 guest/effect 前无 mutation reject。

### 2.8 Operation-specific reducer, import cardinality and terminal case

下表是 11 个 world 各自 validator 的 normative state matrix，不是 shared DTO/implementation。`a -> b` 表示 progress 可省略但若出现只能按列出的顺序；同名 plain progress 最多一次。`pending` 可在任一 nonterminal wait 重复，但也计入每 operation `65,536` pull cap。每个 request 只能以表中 success case 或一次 `failed(family-error)` terminal 结束。

严格 request decode、declarative `OperationId`、case/method allowlist、Host selector/table/generation binding 全部成功后才创建 operation。此前任何失败返回 `invoke Err`、零 import/Pack effect。创建后，Host 为每个消费型 import 在调用前原子 reserve 表中 cardinality slot；wrong case/order/duplicate/N+1 在实际 import 前拒绝，所以不会产生额外 effect。同名 `*-host-error` 映射到同名 outer family error；无同名 outer case时，authority/snapshot/schema/answer/transition/dependency rejection 映射 `invalid-argument`，transport/source/provider/interaction/isolation unavailable 映射该 family-specific stable unavailable case。Host `protocol` 在 family 有 `protocol` 时映射该 case，否则映射该 family-specific stable unavailable case（Usage 为 `source-unavailable`）。accepted Host result/exchange payload 必须按 terminal record 声明 field order、variant tag、option presence、整数和 UTF-8 string/list 内容做 exact typed structural equality 后逐字段复制，不比较 canonical ABI bytes，也不 recompute。该映射和 `failed(family-error)` 是唯一 typed error channel。任一 import Err、crossed return case 或 validation failure 关闭 reducer且禁止后续 import；commit/apply/start/run call 永不 retry。terminal/cancel/close 后 guest pull、exchange pull 与 Host import 均为 zero。

| request case | allowed progress sequence | exact Host import/resource-method sequence | allowed success `complete` case(s) |
| --- | --- | --- | --- |
| `session.create` | `committing` | `commit-ledger(create) x1` | `created` |
| `session.open` | `recovering` | `load-ledger(open) x1` | `opened` |
| `session.append` | `replaying -> committing` | `load-ledger(append) x1 -> commit-ledger(append) x1` | `appended` |
| `session.read` | `replaying` | `load-ledger(events) x1` | `events` |
| `session.fork` | `replaying -> committing` | `load-ledger(fork) x1 -> commit-ledger(fork) x1` | `branched` |
| `session.rewind` | `replaying -> committing` | `load-ledger(rewind) x1 -> commit-ledger(rewind) x1` | `branched` |
| `compaction.assess` | `assessing -> validating` | none | `assessment` |
| `compaction.summarize` | `validating -> summarizing*` | `summarize x1` | `summary` |
| `resources.catalog` | `loading` | none | `catalog` |
| `resources.read` | `loading` | none | `read` |
| `resources.render-prompt` | `rendering` | none | `prompt` |
| `resources.contributions` | `loading` | none | `contributions` |
| `ask.present` | `waiting*` | `present x1` | `answered\|abandoned` matching Host output |
| `todo.create` | `loading`; absent path then emits `persisting` | `load-tasks(create) x1 -> commit-task-event(create) x1` iff accepted `absent`；`already-exists` 或任一先行 terminal/error 均 x0 | `created` |
| `todo.get` | `loading` | `load-tasks(get) x1` | `current` |
| `todo.list` | `loading` | `load-tasks(list) x1` | `listed` |
| `todo.set-status` | `loading`; changed path may then emit `persisting` | `load-tasks(get) x1 -> commit-task-event(set-status) x1` iff accepted changed path reaches commit；exact no-op 或任一先行 terminal/error 均 x0 | `current\|updated` |
| `todo.set-subject` | `loading`; changed path may then emit `persisting` | `load-tasks(get) x1 -> commit-task-event(set-subject) x1` iff accepted changed path reaches commit；exact no-op 或任一先行 terminal/error 均 x0 | `current\|updated` |
| `todo.set-description` | `loading`; changed path may then emit `persisting` | `load-tasks(get) x1 -> commit-task-event(set-description) x1` iff accepted changed path reaches commit；exact no-op 或任一先行 terminal/error 均 x0 | `current\|updated` |
| `todo.replace-dependencies` | `loading`; changed path may then emit `persisting` | `load-tasks(get) x1 -> commit-task-event(replace-dependencies) x1` iff accepted changed path reaches commit；exact no-op 或任一先行 terminal/error 均 x0 | `current\|updated` |
| `todo.set-owner` | `loading`; changed path may then emit `persisting` | `load-tasks(get) x1 -> commit-task-event(set-owner) x1` iff accepted changed path reaches commit；exact no-op 或任一先行 terminal/error 均 x0 | `current\|updated` |
| `todo.delete` | `loading`; accepted `pending\|in-progress` path may then emit `persisting` | `load-tasks(get) x1 -> commit-task-event(delete) x1` iff current source is accepted `pending\|in-progress`；`completed\|deleted` 的 `invalid-transition` 或任一先行 terminal/error 均 commit x0 | `deleted` |
| `web.search` | `searching` | `start-search x1 -> web-exchange.pull x1..=65,536` through terminal | `search-results` |
| `web.fetch` | `fetching*` | `start-fetch x1 -> web-exchange.pull x1..=65,536` through terminal | `fetch-results` |
| `mcp.servers` | `discovering` | none | `servers` |
| `mcp.tools` | `discovering` | none | `tools` |
| `mcp.invoke` | `invoking` | `start-invoke x1 -> mcp-exchange.pull x1..=65,536` through terminal | `invoked` |
| `usage.ingest` | `normalizing` | none | `ingested` |
| `usage.render-summary` | `normalizing` | none；Host constructs exactly one bound request `state` before guest | `summary` |
| `usage.render-details` | `normalizing` | none；Host constructs exactly one bound request `state` before guest | `details` |
| `usage.refresh` | `refreshing -> normalizing` | `start-refresh x1 -> usage-exchange.pull x1..=65,536` through terminal | `refreshed` |
| `subagents.roles` | none | none | `roles` |
| `subagents.enqueue` | optional `queued`，then per-attempt optional `running(attempt) -> review-round(attempt)?` in contiguous attempt order | `run-step x1..=max-attempts` sequentially | `job` |
| `subagents.recover` | `recovering` | `recover-step x1` | `job` |
| `workspace.checkpoint` | `scanning* -> snapshotting` | `scan x1..=65,536` following cursor to EOF | `checkpoint` |
| `workspace.inspect` | `scanning` | `scan x1` | `inspected` |
| `workspace.rollback` | `scanning* -> rolling-back` | `scan x1..=65,535` following cursor to EOF -> `apply-rollback x1` | `rolled-back` |
| `ui.render-runtime` | `rendering` | none | `frame` |
| `ui.handle-action` | none | none | `action` |
| `ui.resolve-theme` | none | none | `theme` |

Repeated progress额外 invariants：compaction total=items.len固定、completed从0 contiguous到total；Ask遵守第6节 exact repeat/advance；Web fetch 的 `total` 必须精确等于 deduplicated request URL count `1..=10`，若发出任一 `fetching`，首个 `completed` 必须为1，后续每个必须精确 `+1`，每值都 `<=total`，不得 duplicate/skip/rollback/cross total；progress 整体可省略，所以最后一个 progress 可以小于 total而 terminal 仍有效。Web search 的独立 declared progress 仍仅是 `searching`，不使用 fetch counter 规则；Subagents attempt从1 contiguous，只有continue且attempt<max才能再次run-step，到上限continue明确合成outer `failed(limit)`而非job failed，其他outcome立即terminal；Workspace按同snapshot cursor至EOF。Web fetch fixtures 对 completed/total 分别覆盖 0/1/N/N+1、duplicate/skip/rollback 与 crossed-total，且覆盖无 progress、partial final progress 和 exact-total progress；search progress 继续按其 own declared cases 独立覆盖。Subagents recovery使用第11节closed receipt，无bool恢复或silent resume。

Web exchange success 固定 `pending* -> head -> data* -> end`，MCP 固定 `pending* -> head -> output -> end`，Usage 固定 `pending* -> head -> document -> end`；各自 `failed` 可替换尚未完成的 suffix。exchange terminal 后 outer result 必须复制 accepted normalized value，不能 guest-held recompute。Session/Todo/Workspace 的 read/commit return variant 必须与表中 outer case相同。fixtures 对每行覆盖 crossed complete、progress rollback/duplicate、每个 import 的 0/1/N/N+1、duplicate start/commit/apply/run 和“前一步失败后下一步 zero effect”；Todo 还逐项覆盖 create 的 `already-exists`/任一先行 error 与五个 setter 的 exact no-op/任一先行 error 均 commit x0，并覆盖 delete 的 `completed|deleted` source 为 `invalid-transition`、commit x0、reservation不消费、revision不变且 zero durable mutation。
