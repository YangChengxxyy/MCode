# T7 FeaturePack ABI authority

> 本文冻结 `mcode:feature-pack@0.0.1` 的唯一 current、first developer preview 目标契约，不声称 T7 或任一 Pack runtime 已实现。本文是仓库内可审查的 FeaturePack authority；紧随 T7 交付的 parseable WIT source、current LF golden 与 semantic JSONL golden 必须是其 machine-verifiable projection。
>
> 所有 schema/type/field/variant/function 名称使用英文；说明使用中文。本文只声明 Manager、全部 FeaturePack world/interface 与 Provider reference 的 sole-current `0.0.1` ABI 和 typed surface；不存在旧版本共存，不保留任何旧版本文件、`abi_v1.json`、historical golden、compatibility parser/adapter、ABI alias、dual-read 或 fallback，也不提供通用 payload、shared DTO、public `Value`、map 或 `metadata/extensions` 字段。目标契约、machine-verifiable artifact 与后续 runtime 分阶段验证。

## 1. Current topology and artifact boundary

T7 只冻结以下 13 个 current world：

| package | world | exact boundary |
| --- | --- | --- |
| `mcode:plugin@0.0.1` | `mcode:plugin/manager@0.0.1` | import `mcode:plugin/feature-service@0.0.1`; export `mcode:plugin/manager-lifecycle@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/session@0.0.1` | import `mcode:feature-pack/session-host@0.0.1`; export `mcode:feature-pack/session-pack@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/compaction@0.0.1` | import `mcode:feature-pack/compaction-host@0.0.1`; export `mcode:feature-pack/compaction-pack@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/resources@0.0.1` | no imports; export `mcode:feature-pack/resources-pack@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/ask@0.0.1` | import `mcode:feature-pack/ask-host@0.0.1`; export `mcode:feature-pack/ask-pack@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/todo@0.0.1` | import `mcode:feature-pack/todo-host@0.0.1`; export `mcode:feature-pack/todo-pack@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/web@0.0.1` | import `mcode:feature-pack/web-host@0.0.1`; export `mcode:feature-pack/web-pack@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/mcp@0.0.1` | import `mcode:feature-pack/mcp-host@0.0.1`; export `mcode:feature-pack/mcp-pack@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/usage@0.0.1` | import `mcode:feature-pack/usage-host@0.0.1`; export `mcode:feature-pack/usage-pack@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/subagents@0.0.1` | import `mcode:feature-pack/subagents-host@0.0.1`; export `mcode:feature-pack/subagents-pack@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/workspace@0.0.1` | import `mcode:feature-pack/workspace-host@0.0.1`; export `mcode:feature-pack/workspace-pack@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/ui@0.0.1` | no imports; export `mcode:feature-pack/ui-pack@0.0.1` |
| `mcode:provider-pack@0.0.1` | `mcode:provider-pack/provider@0.0.1` | zero imports; export `mcode:provider-pack/provider-api@0.0.1`; 详见 [08-provider-pack-abi.md](08-provider-pack-abi.md) |

Manager、11 个 FeaturePack family world/interface 与 Provider reference 都只存在表中 exact `0.0.1` first developer preview；其他历史 package/world/interface 文件、adapter、alias 与并行 current surface 均不存在。`mcode:feature-pack@0.0.1` 是一个 package，包含 11 个物理独立的 family world。每个 world 独立声明自己的 request、progress、result、error、Host interface 与嵌套类型；不能通过 `use` 跨 family 复用类型。Manager 的 import/export 方向只能是 Manager import `feature-service`、export `manager-lifecycle`，不能反向。

Main repository 只拥有 ABI/WIT/current goldens、binary static preflight、纯 semantic validator、通用 T8 component/resource runtime 和 family Host service/effect substrate；第一方 Manager/Pack source、build 与 release artifact 只在 `https://github.com/MCapricorns/MCode_plugins`。Parseable WIT source、resolved-world golden 和每-world semantic JSONL golden 是紧随本文的 T7 artifact slice；该 slice 缺失或未通过 parser 时，T7 不得标记通过。

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

## 3. `session` world

### 3.1 Exact signatures and local fields

- Imported interface: `mcode:feature-pack/session-host@0.0.1`。
- Exported interface: `mcode:feature-pack/session-pack@0.0.1`。
- Host signatures: `load-ledger(request: ledger-read) -> result<ledger-page,session-host-error>`；`commit-ledger(mutation: ledger-mutation) -> result<ledger-commit,session-host-error>`。
- Pack signatures: `invoke(request: session-request) -> result<own<session-operation>,session-error>`；`session-operation.pull() -> session-pull`。

| local type | exact fields/variants |
| --- | --- |
| `session-request` | `create(create-request) \| open(open-request) \| append(append-request) \| read(read-request) \| fork(fork-request) \| rewind(rewind-request)` |
| `create-request` | `session-id:string, root-branch-id:string` |
| `open-request` | `session-id:string` |
| `append-request` | `session-id:string, branch-id:string, expected-head:head-stamp, reservation:event-reservation-view` |
| `read-request` | `session-id:string, branch-id:string, snapshot-head:head-stamp, after:option<string>, limit:u16` |
| `fork-request` | `session-id:string, from-branch-id:string, at-event-id:string, new-branch-id:string, reservation:branch-reservation-view` |
| `rewind-request` | `session-id:string, branch-id:string, to-event-id:string, new-branch-id:string, reservation:branch-reservation-view` |
| `session-progress` | `recovering \| replaying \| committing` |
| `session-pull` | `pending \| progress(session-progress) \| complete(session-result) \| failed(session-error)` |
| `session-result` | `created(created-result) \| opened(opened-result) \| appended(appended-result) \| events(events-result) \| branched(branched-result)` |
| `session-error` | `invalid-argument \| not-found \| conflict(conflict-result) \| corrupt \| limit \| cancelled \| unavailable` |
| `created-result` | `session-id:string, branch-id:string, head:head-stamp` |
| `opened-result` | `heads:list<branch-head>` |
| `appended-result` | `head:head-stamp` |
| `events-result` | `items:list<session-event>, next:option<string>` |
| `branched-result` | `branch-id:string, head:head-stamp` |
| `branch-head` | `branch-id:string, head:head-stamp` |
| `session-event` | `event-id:string, digest:string, bytes:u64, kind:event-kind, call-id:option<string>` |
| `event-kind` | `message \| tool-call \| tool-result \| usage` |
| `head-stamp` | `empty \| event(string)` |
| `event-reservation-view` | `event-id:string, payload-digest:string, branch-id:string, expected-head:head-stamp` |
| `branch-mutation-kind` | `fork \| rewind` |
| `branch-reservation-view` | `reservation-id:string, kind:branch-mutation-kind, source-branch-id:string, source-head:head-stamp, target-event-id:string, new-branch-id:string, mutation-digest:string` |
| `ledger-read` | `open(open-ledger-read) \| append(append-ledger-read) \| events(events-ledger-read) \| fork(fork-ledger-read) \| rewind(rewind-ledger-read)` |
| `open-ledger-read` | `session-id:string` |
| `append-ledger-read` | `session-id:string, branch-id:string, expected-head:head-stamp, reservation:event-reservation-view` |
| `events-ledger-read` | `session-id:string, branch-id:string, snapshot-head:head-stamp, after:option<string>, limit:u16` |
| `fork-ledger-read` | `session-id:string, from-branch-id:string, at-event-id:string, new-branch-id:string, reservation:branch-reservation-view` |
| `rewind-ledger-read` | `session-id:string, branch-id:string, to-event-id:string, new-branch-id:string, reservation:branch-reservation-view` |
| `ledger-page` | `opened(opened-ledger-page) \| append(append-ledger-view) \| events(events-ledger-page) \| fork(fork-ledger-view) \| rewind(rewind-ledger-view)` |
| `opened-ledger-page` | `heads:list<branch-head>` |
| `append-ledger-view` | `branch-id:string, actual-head:head-stamp, reservation:event-reservation-view` |
| `events-ledger-page` | `items:list<session-event>, next:option<string>` |
| `fork-ledger-view` | `from-branch-id:string, source-head:head-stamp, at-event-id:string, new-branch-id:string, reservation:branch-reservation-view` |
| `rewind-ledger-view` | `branch-id:string, source-head:head-stamp, to-event-id:string, new-branch-id:string, reservation:branch-reservation-view` |
| `ledger-mutation` | `create(create-ledger-mutation) \| append(append-ledger-mutation) \| fork(fork-ledger-mutation) \| rewind(rewind-ledger-mutation)` |
| `create-ledger-mutation` | `session-id:string, root-branch-id:string` |
| `append-ledger-mutation` | `session-id:string, branch-id:string, expected-head:head-stamp, reservation:event-reservation-view` |
| `fork-ledger-mutation` | `session-id:string, from-branch-id:string, at-event-id:string, new-branch-id:string, reservation:branch-reservation-view` |
| `rewind-ledger-mutation` | `session-id:string, branch-id:string, to-event-id:string, new-branch-id:string, reservation:branch-reservation-view` |
| `ledger-commit` | `created(created-ledger-commit) \| appended(appended-ledger-commit) \| branched(branched-ledger-commit)` |
| `created-ledger-commit` | `session-id:string, branch-id:string, head:head-stamp` |
| `appended-ledger-commit` | `head:head-stamp` |
| `branched-ledger-commit` | `branch-id:string, head:head-stamp` |
| `session-host-error` | `not-found \| conflict(conflict-result) \| corrupt \| limit \| unavailable` |
| `conflict-result` | `actual:head-stamp` |

### 3.2 Session semantics and stage

Session grammar 固定为 session `ses1-[0-9a-f]{32}`、branch `br1-[0-9a-f]{32}`、event `evt1-[0-9a-f]{32}`、call `call1-[0-9a-f]{32}`、digest lowercase `sha256:` 加 64 lowercase hex；reservation event ID 也服从 event grammar。`create` 的 session/root branch 在 Pack 前由 Host mint 并共同 pre-reserve，guest 不能创建；accepted create commit 必须 exact-return 两 ID且 `head=empty`，Pack 的 `created` terminal 再 exact copy同一 session ID、root branch ID与 `empty` head。accepted append commit 与 `appended` terminal 的 head 都必须精确为 `event(reservation.event-id)`。accepted fork commit/terminal 的 branch ID 都必须精确为 `new-branch-id`、head 都必须精确为 `event(at-event-id)`；accepted rewind commit/terminal 同样使用 exact `new-branch-id`，head 精确为 `event(to-event-id)`。每条写路径都先由 Host validation 接受 exact commit case/branch/head，再要求 Pack terminal exact typed structural copy；crossed commit/terminal branch 或 head 立即关闭且不允许第二次 commit。fixtures 分别 crossed create/append/fork/rewind 的 branch/head、覆盖 commit-to-terminal copy，并证明 mismatch 后 extra commit x0。branch/head/event/call/digest/reservation cross-fields 必须解析到同一 session、branch lineage、generation、outer task 与 expected head。

`load-ledger.open|append|events|fork|rewind` 分别且只服务 outer `open|append|read|fork|rewind`；read fields 与 outer request 做 exact typed structural equality；append/fork/rewind view 全部字段来自同一 immutable Host row；commit case 必须匹配 outer case。branch 最多 64；open 返回全部 heads，按 branch ID byte order。read snapshot immutable；`after=None` 从首项开始，`Some(evt)` 必须是同 branch snapshot ancestor且 exclusive。items 按 ledger order且 `<=limit`（`1..=256`）；非 EOF `next=Some(last-returned-event-id)` 并严格前进，下一页以其为 exclusive after；EOF 才为 None。空 snapshot 可空+None，非 EOF 不得空页。cursor row 绑定 after/branch/snapshot-head；stale、rewound、forked、foreign或 replay continuation 在 guest 前拒绝。

Session append 的 single-use event reservation 保持不变，只在 payload durability 与 call/result ordering check 成功后创建；`commit-ledger` 在 expected-head CAS 下消费它，防止 tool call 与 result 之间插入事件。fork/rewind 则各使用 Host-issued `sbr1-[0-9a-f]{32}` single-use `branch-reservation-view`，并在同一 outer task 上把 kind、source branch/head、target event、new branch 和 mutation digest 绑定为一行。digest 是 SHA-256 over ASCII `mcode-session-branch-mutation-v1\0`，随后按顺序编码 `session-id,reservation-id,kind,source-branch-id,source-head,target-event-id,new-branch-id`；string=`u32be byte-length || UTF-8`，kind 为 zero-based u8，head 为 zero-based u8 tag，`event` tag 后接 framed event ID，所有转换 checked。outer request、ledger read、ledger view、ledger mutation 与 reservation row 必须 exact typed structural equality；commit 在 bound source-head CAS 下 single-use 消费。crossed/missing/replayed reservation、wrong kind/target/head/digest 或 CAS loss 均 zero commit，且绝不退回无 reservation 的 branch mutation。event payload bytes `1..=8 MiB`，usage event `1..=64 KiB`；event digest 是 lowercase `sha256:` digest。`call-id` 只在 `tool-call|tool-result` 时为 `Some`，其他 kind 必须为 `None`；Host 在 event reservation 前执行完整 call/result ordering check。reservation 成功或失败后都不可再次使用。T9 负责 durable storage/effect；T7 只做 in-memory table/reducer validation。

## 4. `compaction` world

### 4.1 Exact signatures and local fields

- Imported interface: `mcode:feature-pack/compaction-host@0.0.1`。
- Exported interface: `mcode:feature-pack/compaction-pack@0.0.1`。
- Host signature: `summarize(request: summary-request) -> result<summary-output,compaction-host-error>`。
- Pack signature: `invoke(request: compaction-request) -> result<own<compaction-operation>,compaction-error>`；`compaction-operation.pull() -> compaction-pull`。

| local type | exact fields/variants |
| --- | --- |
| `compaction-request` | `assess(assess-request) \| summarize(summarize-request)` |
| `assess-request` | `session-id:string, branch-id:string, head:head-stamp, input-tokens:u64, context-limit:u64, reserve-output:u64` |
| `summarize-request` | `session-id:string, branch-id:string, head:head-stamp, items:list<summary-item>, target-tokens:u32` |
| `compaction-progress` | `assessing \| validating \| summarizing(summarizing-progress)` |
| `compaction-pull` | `pending \| progress(compaction-progress) \| complete(compaction-result) \| failed(compaction-error)` |
| `summarizing-progress` | `completed:u16, total:u16` |
| `compaction-result` | `assessment(assessment-result) \| summary(summary-result)` |
| `assessment-result` | `needed:bool, target-tokens:u32` |
| `summary-result` | `text:string, covered-through:string, input-tokens:option<u64>, output-tokens:option<u64>` |
| `compaction-error` | `invalid-argument \| stale-head \| provider-unavailable \| invalid-terminal(invalid-terminal-reason) \| limit \| cancelled \| unavailable` |
| `invalid-terminal-reason` | `length \| error \| cancel \| tool-call` |
| `summary-item` | `event-id:string, kind:item-kind, text:string, call-id:option<string>` |
| `item-kind` | `system \| user \| assistant \| tool-call \| tool-result` |
| `head-stamp` | `empty \| event(string)` |
| `summary-request` | `session-id:string, branch-id:string, head:head-stamp, items:list<summary-item>, target-tokens:u32` |
| `summary-output` | `text:string, covered-through:string, input-tokens:option<u64>, output-tokens:option<u64>` |
| `compaction-host-error` | `stale-head \| provider-unavailable \| invalid-terminal(invalid-terminal-reason) \| limit \| cancelled \| unavailable` |

### 4.2 Semantics and stage

本 world 独立关闭其 scalar grammar：session ID 精确匹配 `ses1-[0-9a-f]{32}`（37 UTF-8 bytes），branch ID 精确匹配 `br1-[0-9a-f]{32}`（36 bytes），每个 item event ID 与 `covered-through` 精确匹配 `evt1-[0-9a-f]{32}`（37 bytes），call ID 精确匹配 `call1-[0-9a-f]{32}`（38 bytes）；`head-stamp` 只能是 `empty` 或 `event(payload)`，且 event payload 也精确匹配该 37-byte event grammar。session、branch、item event、covered-through、call 与 head event payload 各自都是 Compaction-family-local string projection；相同文本只按下述 explicit relation 比较，不能据此把不同 family、field 或 Host table row 的 authority 互换。Host 必须在任何 Pack/route/effect 前，对 request 侧的 session、branch、item event、call 与 head event payload 按 decoded valid UTF-8 exact byte length 做 checked validation，并将它们解析到同一 Session ledger、同一 branch lineage 与 request head：items 按该 branch ledger order且 event-id unique，head event payload也必须属于同一 ledger/branch。

`covered-through` 只在 `summarize` Host output 返回后存在；Host 必须在接受、保留或发布该 output/terminal 前验证其 exact 37-byte event grammar，并要求它等于 request 最后一个 item ID且是 request head 的同-branch ancestor。request-side golden 对每种 projection 的 malformed grammar、exact byte length 与 crossed relation gate 独立覆盖 0/1/N/N+1：含 empty/zero、一个 exact-valid value、各自 exact N-byte boundary、N+1 byte、错误 prefix/case/hex/UTF-8，以及 crossed session/branch/event/head/call/family/table-row，全部在 Pack/route/effect 前 zero side effect。output-side `covered-through` golden 独立覆盖同一 grammar boundary、非最后 item、非 ancestor 与 crossed event/family/table-row；拒绝时不得接受、保留或发布该 summary output，published state 不变，且在已经发生的 `summarize` effect 后不得产生追加 Pack/route/effect，不声称该次 `summarize` 为 zero effect。

所有 counter checked。items `1..=2,048`；tool-call 后紧邻同 call-id tool-result，不得 orphan/cross/duplicate，其他 kind call-id=None。每项 text `1..=1 MiB` Safe，总 text及完整 request aggregate分别 `<=8 MiB`；字段乘积再大也不放宽 aggregate。context-limit `1..=i64::MAX`，reserve-output `0..=context-limit`。assessment `needed` 精确等于 checked `input-tokens+reserve-output>context-limit`；不需要时 target=0，需要时 target=`min(1,048,576,context-limit-reserve-output)` checked u32，差为0/不可表示则 limit。summarize target `1..=1,048,576`。progress completed<=total，total `1..=2,048`。summary text `0..=1 MiB` Safe；Host output 与 terminal exact typed structural equality。counter 为 None 或 `Some(0..=i64::MAX)`。

`summarize` 没有 network/credential grant 参数；Host 在 current route lease 下解析 route，再启动 Provider completion，不能 mint Compaction network grant。T21 负责真实 effect、cancel/drain 和 generation；T7 只验证 reducer 和 typed boundary。

## 5. `resources` world

### 5.1 Exact signatures and local fields

- No Host import。
- Exported interface: `mcode:feature-pack/resources-pack@0.0.1`。
- Pack signature: `invoke(request: resources-request) -> result<own<resources-operation>,resources-error>`；`resources-operation.pull() -> resources-pull`。

| local type | exact fields/variants |
| --- | --- |
| `resources-request` | `catalog(catalog-request) \| read(read-request) \| render-prompt(render-prompt-request) \| contributions` |
| `catalog-request` | `offset:u32, limit:u16` |
| `read-request` | `id:string, offset:u64, max-bytes:u32` |
| `render-prompt-request` | `id:string, args:list<prompt-arg>` |
| `resources-progress` | `loading \| rendering` |
| `resources-pull` | `pending \| progress(resources-progress) \| complete(resources-result) \| failed(resources-error)` |
| `resources-result` | `catalog(catalog-result) \| read(read-result) \| prompt(prompt-result) \| contributions(contributions-result)` |
| `catalog-result` | `items:list<catalog-entry>, next-offset:option<u32>` |
| `catalog-entry` | `%resource(resource-entry) \| prompt(prompt-entry)` |
| `resource-entry` | `id:string, title:string, media:resource-media, size-hint:option<u64>` |
| `resource-media` | `text \| markdown` |
| `prompt-entry` | `id:string, title:string, params:list<prompt-param>` |
| `prompt-param` | `name:string, label:string, required:bool` |
| `prompt-arg` | `name:string, value:string` |
| `read-result` | `text:string, next-offset:option<u64>` |
| `prompt-result` | `id:string, messages:list<prompt-message>` |
| `prompt-message` | `role:message-role, text:string` |
| `message-role` | `system \| user \| assistant` |
| `contributions-result` | `items:list<contribution>` |
| `contribution` | `id:string, kind:contribution-kind` |
| `contribution-kind` | `status \| panel` |
| `resources-error` | `invalid-argument \| not-found \| limit \| unavailable \| cancelled` |

### 5.2 Semantics and stage

Resources Pack 始终 zero-import；数据随 Pack generation immutable embedded。T14 连接 real guest/Host integration，但不得增加 import/fetch。resource、prompt、contribution ID 为 `LocalId(128)`；resource/prompt ID 跨 kind global unique。read 只接受 `%resource` kind，render-prompt 只接受 prompt kind，cross-kind lookup 为 invalid-argument。title `Label(256)`；size-hint `0..=i64::MAX`。catalog generation-bound，total `<=8,192`，按 `(kind-tag %resource=0|prompt=1,id-bytes)`；limit `1..=128`。初始 offset=0，next=`offset+items.len` checked exact successor，Some iff 小于 sealed total；EOF None，empty+Some/skip/replay/stale generation reject。

每个 request/result aggregate `<=1 MiB`。prompt params `0..=64`，name `LocalId(64)`、label `Label(128)`，name byte-sort unique。args `0..=64`且 byte-sort unique，无 unknown name；required exactly once，optional至多一次，value `0..=64 KiB` Safe。prompt result ID等于 request；messages `0..=16`，严格保持 embedded template declaration order，每条 `<=64 KiB` Safe、总 text `<=256 KiB`。contributions `0..=64`，ID unique byte-sorted、总 charge `<=256 KiB`。

read max-bytes `4..=65,536`；offset 是 canonical UTF-8 scalar boundary。非 EOF 至少返回一 scalar、不拆 scalar且 `<=max-bytes`，next=checked `offset+returned UTF-8 bytes`；EOF iff None，只有 EOF 可空。每个 bound 有 0/1/N/N+1 golden。

## 6. `ask` world

### 6.1 Exact signatures and local fields

- Imported interface: `mcode:feature-pack/ask-host@0.0.1`。
- Exported interface: `mcode:feature-pack/ask-pack@0.0.1`。
- Host signature: `present(request: interaction-request) -> result<interaction-output,ask-host-error>`。
- Pack signature: `invoke(request: ask-request) -> result<own<ask-operation>,ask-error>`；`ask-operation.pull() -> ask-pull`。

| local type | exact fields/variants |
| --- | --- |
| `ask-request` | `present(present-request)` |
| `present-request` | `title:option<string>, questions:list<question>` |
| `question` | `id:string, header:string, question:string, kind:question-kind` |
| `question-kind` | `confirm \| text(text-params) \| single-choice(choice-params) \| multi-choice(choice-params)` |
| `text-params` | `max-bytes:u16, multiline:bool` |
| `choice-params` | `choices:list<choice>` |
| `choice` | `id:string, label:string, description:string, preview:option<string>` |
| `ask-progress` | `waiting(waiting-progress)` |
| `ask-pull` | `pending \| progress(ask-progress) \| complete(ask-result) \| failed(ask-error)` |
| `waiting-progress` | `index:u8, total:u8` |
| `ask-result` | `answered(answers) \| abandoned` |
| `answers` | `items:list<answer>` |
| `answer` | `question-id:string, value:answer-value` |
| `answer-value` | `confirmed(bool) \| text(string) \| choice(string) \| choices(list<string>)` |
| `ask-error` | `invalid-argument \| invalid-answer \| interaction-unavailable \| limit \| cancelled` |
| `interaction-request` | `title:option<string>, questions:list<question>` |
| `interaction-output` | `answered(answers) \| abandoned` |
| `ask-host-error` | `invalid-answer \| interaction-unavailable \| limit \| cancelled` |

### 6.2 Semantics and stage

questions `1..=4`；question ID 为 `LocalId(128)` 且 unique，header 为 `Label(64)`，question 为 `Safe+(1 KiB)`。text `max-bytes=1..=8,192`；answer text 必须 `0..=max-bytes` Safe，multiline=false 时不得含 TAB/LF。choice 数量 `2..=4`；choice ID 是 `LocalId(128)` 且在该 question 内 unique并保持声明顺序，label 是 `Label(60)`，description `0..=1 KiB` Safe，preview 为 None 或 `0..=16 KiB` Safe。waiting 首次若出现必须 index=0；total 固定等于 question count，index `0..=total-1`。后续只可重复 exact current pair或递增恰好1，不得 skip/rollback/change total。terminal 可在任一 current index 后出现且之后 zero pull；重复 waiting 不代表重复 present（import 恰一次）。

`answers.items` 长度必须等于 question count 且按 question declaration order；每个 question ID 恰好出现一次：confirm 只能用 confirmed，text 只能用满足该 question 参数的 text，single-choice 的 string 必须等于恰好一个 declared choice ID，multi-choice list `0..=4`、按 declaration order unique且每项都存在。`abandoned` 不携带 partial answers。title 为 None 或 `Label(128)`；Host `interaction-output` 必须原样满足同一 request schema，Pack terminal 只能复制 `answered|abandoned` 对应 case。每个 question/choice/answer string、list 与 max-bytes 分别覆盖 0/1/N/N+1。Ask 不是 authorization、grant、secret 或 credential answer API；T15 负责 Host interaction，T7 只做 answer cardinality/type validation。

## 7. `todo` world

### 7.1 Exact signatures and local fields

- Imported interface: `mcode:feature-pack/todo-host@0.0.1`。
- Exported interface: `mcode:feature-pack/todo-pack@0.0.1`。
- Host signatures: `load-tasks(request: task-read) -> result<task-page,todo-host-error>`；`commit-task-event(mutation: task-mutation) -> result<task-commit,todo-host-error>`。
- Pack signatures: `invoke(request: todo-request) -> result<own<todo-operation>,todo-error>`；`todo-operation.pull() -> todo-pull`。

| local type | exact fields/variants |
| --- | --- |
| `todo-request` | `create(create-request) \| get(get-request) \| %list(list-request) \| set-status(set-status-request) \| set-subject(set-subject-request) \| set-description(set-description-request) \| replace-dependencies(replace-dependencies-request) \| set-owner(set-owner-request) \| delete(delete-request)` |
| `create-request` | `todo-id:string, subject:string, description:string, blocked-by:list<string>, owner:option<string>, reservation:event-reservation-view` |
| `get-request` | `todo-id:string` |
| `list-request` | `snapshot:snapshot-revision, status:option<task-status>, after:option<string>, limit:u16` |
| `set-status-request` | `todo-id:string, expected-revision:task-revision, status:task-status, active-form:option<string>, reservation:event-reservation-view` |
| `set-subject-request` | `todo-id:string, expected-revision:task-revision, subject:string, reservation:event-reservation-view` |
| `set-description-request` | `todo-id:string, expected-revision:task-revision, description:string, reservation:event-reservation-view` |
| `replace-dependencies-request` | `todo-id:string, expected-revision:task-revision, blocked-by:list<string>, reservation:event-reservation-view` |
| `set-owner-request` | `todo-id:string, expected-revision:task-revision, owner:option<string>, reservation:event-reservation-view` |
| `delete-request` | `todo-id:string, expected-revision:task-revision, reservation:event-reservation-view` |
| `todo-progress` | `loading \| persisting` |
| `todo-pull` | `pending \| progress(todo-progress) \| complete(todo-result) \| failed(todo-error)` |
| `todo-result` | `created(task) \| current(task) \| listed(listed-result) \| updated(task) \| deleted(task)` |
| `task` | `todo-id:string, revision:task-revision, status:task-status, subject:string, description:string, active-form:option<string>, blocked-by:list<string>, owner:option<string>` |
| `listed-result` | `items:list<task>, next:option<string>` |
| `task-status` | `pending \| in-progress \| completed \| deleted` |
| `task-revision` | alias of `u64`; one task row scope |
| `snapshot-revision` | alias of `u64`; immutable list document scope |
| `task-read` | `create(create-task-read) \| get(get-task-read) \| %list(list-task-read)` |
| `create-task-read` | `todo-id:string` |
| `get-task-read` | `todo-id:string` |
| `list-task-read` | `snapshot:snapshot-revision, status:option<task-status>, after:option<string>, limit:u16` |
| `task-page` | `absent \| current(task) \| listed(listed-task-page)` |
| `listed-task-page` | `items:list<task>, next:option<string>` |
| `task-mutation` | `mutation:todo-mutation, reservation:event-reservation-view` |
| `todo-mutation` | `create(create-mutation) \| set-status(set-status-mutation) \| set-subject(set-subject-mutation) \| set-description(set-description-mutation) \| replace-dependencies(replace-dependencies-mutation) \| set-owner(set-owner-mutation) \| delete(delete-mutation)` |
| `create-mutation` | `todo-id:string, subject:string, description:string, blocked-by:list<string>, owner:option<string>` |
| `set-status-mutation` | `todo-id:string, expected-revision:task-revision, status:task-status, active-form:option<string>` |
| `set-subject-mutation` | `todo-id:string, expected-revision:task-revision, subject:string` |
| `set-description-mutation` | `todo-id:string, expected-revision:task-revision, description:string` |
| `replace-dependencies-mutation` | `todo-id:string, expected-revision:task-revision, blocked-by:list<string>` |
| `set-owner-mutation` | `todo-id:string, expected-revision:task-revision, owner:option<string>` |
| `delete-mutation` | `todo-id:string, expected-revision:task-revision` |
| `event-reservation-view` | `reservation-id:string, mutation-digest:string, expected-revision:option<task-revision>` |
| `task-commit` | `task:task` |
| `todo-error` | `invalid-argument \| already-exists \| not-found \| revision-conflict(revision-conflict-result) \| invalid-transition \| dependency-cycle \| limit \| unavailable \| cancelled` |
| `revision-conflict-result` | `actual:task-revision` |
| `todo-host-error` | `already-exists \| not-found \| revision-conflict(revision-conflict-result) \| invalid-transition \| dependency-cycle \| limit \| unavailable` |

### 7.2 Semantics and stage

Host 在 Pack 前 mint/pre-reserve `todo1-[0-9a-f]{32}` todo ID 与 `tdr1-[0-9a-f]{32}` reservation；guest string 不能 mint。task-revision 只用于单 task CAS，snapshot-revision 只用于 immutable list/cursor；即使数值相同也不可互换，均 `1..=i64::MAX`。subject `1..=256` 单行 Label，description `0..=8 KiB` Safe，owner/active-form None或 `1..=256` 单行 Label。blocked-by `0..=64`，每项匹配 todo grammar、byte-sorted unique、存在且非 deleted，不得 self/cycle；completed dependency可保留，只有全部 dependency completed 才可转 in-progress/completed。active-form=Some iff in-progress。

`create` 只可用于不存在的 ID，commit 后固定产生 revision `1`、status `pending`、`active-form=None` 的 `created(task)`；若 `load-tasks.create` 找到既有 ID，Host 只能返回 `Err(already-exists)` 并同名映射 outer `already-exists`，commit x0、reservation 不消费且零 durable mutation，不能映射 `current`、`invalid-argument` 或其他 error。`set-status` 的无重叠矩阵固定如下；表外没有“其他 same-state”规则：

| source | target | exact reducer result |
| --- | --- | --- |
| `pending` | `pending` | exact no-op：`current`、commit x0、reservation不消费、revision不变；target active-form必须None |
| `pending` | `in-progress` | allowed update；active-form必须Some |
| `pending` | `completed` | allowed update；active-form必须None |
| `pending` | `deleted` | `invalid-transition` |
| `in-progress` | `in-progress` | active-form与current相同则exact no-op/`current`；不同则allowed `updated`；两者都必须Some |
| `in-progress` | `pending` | allowed update；active-form必须None |
| `in-progress` | `completed` | allowed update；active-form必须None |
| `in-progress` | `deleted` | `invalid-transition` |
| `completed` | every target，包括 `completed` | `invalid-transition` |
| `deleted` | every target，包括 `deleted` | `invalid-transition` |

`delete` 是从 `pending|in-progress` 进入 `deleted` 的唯一 transition，commit 后清空 active-form 并返回 status `deleted` 的 `deleted(task)`；从 `completed|deleted` delete 均 `invalid-transition`、commit x0、reservation不消费、revision不变且 zero durable mutation。`completed|deleted` 的 subject/description/dependencies/owner mutation 也一律拒绝；equal-value subject/description/dependencies/owner 只在 `pending|in-progress` 是 no-op并返回 `current`、commit x0、reservation不消费、revision不变。除 create 外每次成功 commit 的 revision 必须恰为 expected revision `+1`，overflow reject；失败/no-op 不消费 revision。`set-subject|set-description|replace-dependencies|set-owner` 只在 `pending|in-progress` 上返回 `updated(task)`。reducer 与 semantic goldens 必须逐格使用上述 exact result/commit cardinality，覆盖四态笛卡尔积（尤其 completed->completed 与 deleted->deleted invalid）、两种合法 delete source、全部 terminal mutation、active-form same/changed pairing、create/result/revision 0/1/N/N+1，以及每个合法 equal-value no-op 的 `current`、zero commit、revision不变和reservation未消费。

`load-tasks.create` 只服务 outer `create`；不存在时返回 `absent`，既有时只能返回 Host `Err(already-exists)` 并 zero commit，不能返回 `current`。`load-tasks.get` 只服务 outer `get` 或 non-create mutation 的 exact target lookup并返回 `current`；`load-tasks.list` 只服务 outer `list` 并返回 `listed`。crossed `task-page` case 在 commit 前拒绝。list snapshot immutable，完整 filter 绑定 cursor；task ID byte-sorted，items `<=limit` 且 `limit=1..=256`，next 严格前进。mutation digest 是 SHA-256 over ASCII `mcode-todo-mutation-v1\0`、todo-mutation zero-based u8 tag与 table field order：string=`u32be length||UTF-8`、list=`u32be count||elements`、option=`00|01`后payload、u64=u64be、bool=`00|01`，所有转换 checked。request mutation/reservation preimage/import mutation exact typed structural equality；Todo 不引用 Session event。实际 mutation 携带 Host-issued single-use reservation；create expected=None，其余 Some(expected)，Pack import reservation exact field equal。request/result aggregate各 `<=1 MiB`。commit failure 无 partial Task/event。list cursor绑定完整 filter与snapshot，初始 after=None，Some为exclusive todo ID；非EOF next=last item且strict forward，EOF None，stale replay reject。

## 8. `web` world

### 8.1 Exact signatures and local fields

- Imported interface: `mcode:feature-pack/web-host@0.0.1`。
- Exported interface: `mcode:feature-pack/web-pack@0.0.1`。
- Host signatures: `start-search(request: typed-search) -> result<own<web-exchange>,web-host-error>`；`start-fetch(request: typed-fetch) -> result<own<web-exchange>,web-host-error>`。
- Exchange signature: `web-exchange.pull() -> web-exchange-pull`。
- Pack signatures: `invoke(request: web-request) -> result<own<web-operation>,web-error>`；`web-operation.pull() -> web-pull`。

| local type | exact fields/variants |
| --- | --- |
| `web-request` | `search(search-request) \| fetch(fetch-request)` |
| `search-request` | `query:string, count:u8, range:search-range, chunks:u8, country:option<string>, language:option<string>, domains:list<string>` |
| `search-range` | `none \| d7 \| w2 \| m3 \| y1` |
| `fetch-request` | `urls:list<string>, format:content-format, per-page-timeout:u8, metadata:bool` |
| `content-format` | `markdown \| text \| html` |
| `web-progress` | `searching \| fetching(fetch-progress)` |
| `web-pull` | `pending \| progress(web-progress) \| complete(web-result) \| failed(web-error)` |
| `fetch-progress` | `completed:u8, total:u8` |
| `web-result` | `search-results(search-results) \| fetch-results(fetch-results)` |
| `search-results` | `items:list<search-item>, truncated:bool` |
| `search-item` | `source-id:string, search-id:string, url:string, title:string, text:string, published:option<string>, truncated:bool` |
| `fetch-results` | `pages:list<fetch-page>, truncated:bool` |
| `fetch-page` | `source-id:string, search-id:option<string>, url:string, title:option<string>, text:string, published:option<string>, truncated:bool, original-bytes:option<u64>, returned-bytes:u64, returned-lines:u32` |
| `web-error` | `invalid-argument \| invalid-url \| authority-rejected \| remote-unavailable \| protocol \| limit \| cancelled` |
| `typed-search` | exactly the seven fields of `search-request`; no method/origin/path/header/credential/extension field |
| `typed-fetch` | exactly `urls,format,per-page-timeout,metadata`; one request carries the complete deduplicated URL list |
| `web-head` | `status:u16, media:web-media` |
| `web-media` | `json \| event-stream \| text \| html` |
| `web-data` | `bytes:list<u8>` |
| `web-frame` | `head(web-head) \| data(web-data) \| end` |
| `web-exchange-pull` | `pending \| frame(web-frame) \| failed(web-failure)` |
| `web-failure` | `dns \| tls \| timeout \| truncated \| transport \| cancelled` |
| `web-host-error` | `invalid-argument \| authority-rejected \| remote-unavailable \| protocol \| limit \| cancelled` |

Host-private closed `WebAuthorityBindingV1` 不是 WIT、guest DTO 或 map，其 exact record 为 `{version:u16 (=1),family:web-authority-family (=web),manager-id:string,manager-generation:u64,pack-id:string,pack-version:string,pack-hash:string,pack-generation:u64,operation:web-authority-operation,method:web-method,origin:string,path:string,query-policy:web-query-policy,service-id:string,account-id:string,auth-slot-id:string,adapter-policy-digest:string,header-policy-digest:string,redirect-policy:web-redirect-policy,retry-policy:web-retry-policy,deadline:web-deadline,authority-digest:string}`。closed local types 为 sole-case enum `web-authority-family=web`、`web-authority-operation=search|fetch`、`web-method=get|post`、`web-query-policy=none|adapter-canonical`、`web-redirect-policy=forbid|same-origin-bounded(u8)`、`web-retry-policy={max-attempts:u8,backoff-ms:u32,retry-transport:bool,statuses:list<u16>}`、`web-deadline={total-ms:u32,per-attempt-ms:u32}`；无 unknown case/field。`same-origin-bounded` count 精确为 `1..=5`；retry `max-attempts=1..=4`、`backoff-ms=0..=60,000`、`statuses` 为 `0..=16` 个 numeric ascending sorted unique `400..=599` 值，`retry-transport` 是 bool；deadline `total-ms=1..=70,000`、`per-attempt-ms=1..=total-ms`。`max-attempts=1` 时 `statuses` 必须为空且 `retry-transport=false`；`max-attempts>1` 时至少 `retry-transport=true` 或 `statuses` 非空。Host 使用 checked `u64` 计算 `max-attempts * per-attempt-ms + (max-attempts - 1) * backoff-ms`，其值必须 `<=total-ms`；overflow、超界或 policy/deadline mismatch 全部在 digest acceptance/transport 前拒绝。authority digest 是 lowercase SHA-256 over ASCII `mcode-web-authority-binding-v1\0` 加除 `authority-digest` 外上述 declaration-order fields；string/list=`u32be` byte-length/count，u16/u32/u64=fixed-width big-endian，bool=`00|01`，enum/variant=zero-based u8 tag，`web-authority-family.web` 固定编码为 `00`，record 按声明顺序且转换 checked。T7 只用 dummy binding，逐字段（含 version、family、identity/generation、operation/method/origin/path/query policy、service/account/auth slot、adapter/header、redirect/retry/deadline）制造 digest/binding mismatch 并证明 zero transport；另以一份 complete dummy binding 固定全部 canonical preimage bytes 与 expected digest golden，明确覆盖 family 的 `00` byte。policy fixtures 对 redirect count、attempt/backoff/status count/status value/total/per-attempt 分别覆盖 0/1/N/N+1，并覆盖 duplicate/unsorted statuses、single-attempt带retry条件、multi-attempt无retry条件、per-attempt>total、checked schedule>total及每个 digest/binding mismatch。T10 才把同一 frozen shape 绑定为具体 signed values，T17 才执行 transport。URL 与 query 始终只是 bounded untrusted payload，不能覆盖 binding。

### 8.2 URL, exchange and stage semantics

Canonical URL 唯一顺序：(1) 要求 `1..=4096` ASCII bytes及 strict absolute http/https；拒绝 raw control/Bidi/backslash、fragment、userinfo、malformed percent、Unicode/punycode、malformed host/port。(2) decode全部 unreserved bytes；percent-decoded非ASCII必须 valid UTF-8；拒绝 decoded C0/C1/DEL/Bidi及 authority/path encoded `/|\\`；query执行同一 percent/UTF-8/control normalization，但 query `/` 不是 path separator。(3) 拒绝 decoding 后 `.`/`..` path segment（含 `%2e` aliases）。(4) lowercase scheme/host；IPv4 canonical dotted decimal，IPv6 RFC5952 lowercase compressed，DNS用下述 grammar；port仅 decimal 1..65535且无 leading zero，移除 http:80/https:443；empty path=`/`；剩余 percent hex uppercase；无 fragment。然后 canonical-string deduplicate，fetch为1..10 unique。URL/query仅 untrusted data，不改变 signed authority；output重跑同一算法。

`search-request.query` 为 `Safe+(1,000)`，`count=1..=20`，`chunks=0..=32`，`range` 只为 `none|d7|w2|m3|y1`；`country` 为 None 或 `[a-z]{2}`。language为 None或 exact syntactic subset：primary=2–3 lowercase letters；随后依次可有一个4-letter script、一个2-letter或3-digit region、0..3个5..8 lowercase-alnum variant；variant unique，单一 `-`，无 extension/private-use/grandfathered/empty，总长2..35。domains为0..20 unique byte-sorted canonical DNS names（非IP）；总长1..253 lowercase ASCII，label 1..63，首尾 alphanumeric、内部 alnum/hyphen，无 empty/trailing dot。`typed-fetch` 的 format 为 `markdown|text|html`，每页 timeout 为 `1..=60s`，metadata 为 bool；同一 fetch 不拆成多个 exchange。

web-exchange success为 pending*->head->data*->end；failed可作为first non-pending或在head后替换未完成suffix，successful轨迹的first non-pending必须且只能head。每pull最多一个frame，head/data/end不能合帧。status 200..599；data nonempty且每frame<=64KiB。上帧被pull前不读下帧；end/failed后zero guest/transport pull，post-terminal为protocol failure且不产生第二terminal。fixture覆盖pre-head transport failure与head-then-disconnect。

| response class | data frame count | cumulative data |
| --- | ---: | ---: |
| search 2xx | `<=32` | `<=2 MiB` |
| fetch 2xx | `<=160` | `<=10 MiB` |
| any non-2xx | one frame | `<=8 KiB` |

Declared/streamed N+1 在 buffer 前 reject；non-2xx 的 status/header/error body 不进入 normalized output 或 diagnostic。Host 生成 `source-id=wsrc1-[0-9a-f]{32}` 与 `search-id=wsea1-[0-9a-f]{32}`；search result 有 `0..=request.count` items，source ID unique，search ID 必须等于本次 sealed search，item 顺序保持 Host canonical rank。每个 output URL 再跑上述 canonical validator；title 是 `0..=512` Safe，text 是 `0..=50 KiB` Safe，published 为 None 或 Host-normalized exact 20-byte UTC `YYYY-MM-DDTHH:MM:SSZ`。

fetch pages 必须与 deduplicated request URLs 数量相等并保持 request order；Host 不得因 count/aggregate/line cap 丢弃 fetch page，只能在对应 page 内截断 text suffix并设置该 page `truncated=true`。source ID unique。`search-id=Some` 只可来自 Host provenance table 中该 canonical URL 的 exact prior search，否则为 None。title/published 使用同一 bound；metadata=false 时 title、published、original-bytes 必须全为 None。`returned-bytes` 必须等于 text UTF-8 byte length且 `<=50 KiB`；`returned-lines` 必须等于 empty 时 0、否则 LF count 加“末尾不是 LF”这一项，且 `<=2,000`；`original-bytes` present 时 `0..=10 MiB`。完整 result record logical charge `<=50 KiB`、总 returned lines `<=2,000`。fetch outer truncated精确等于任一page truncated；search outer truncated精确等于任一item truncated或Host因request count/aggregate cap丢弃ranked item suffix。published仅来自Host validated metadata。每个 guest-active segment独立受该 segment supervisor 的 `<=2s` deadline。Web operation clock 使用第 2.5 节在 operation allocation 前记录的同一 `started-at`（因此必然早于 `start-search|start-fetch`），absolute deadline 精确为 `started-at + accepted WebAuthorityBindingV1.deadline.total-ms`，不能放宽到固定 70 秒。fetch 每页 page clock 在该 URL 的 first transport attempt 前开始，absolute deadline 精确为 `page-started-at + validated fetch-request.per-page-timeout * 1,000ms`，跨该页全部 retry/backoff 且绝不重置；每个 transport attempt 另以 `attempt-started-at + accepted WebAuthorityBindingV1.deadline.per-attempt-ms` 为 attempt deadline，只有 policy 允许且 page/operation clock 均未到期时 attempt timeout 才能 retry，retry 只刷新 attempt clock。每次 transport wait 都取 attempt、page（fetch only）与 operation 三个 absolute deadline 的最早者；page 或 operation deadline winner 关闭同一 CAS，attempt deadline winner若不能合法 retry也关闭该 CAS。awaited exchange 位于 segment 外但计入 operation/page clock；resume仅刷新 segment deadline，不能刷新剩余 fuel、pull、page 或 operation budget，任一 deadline 后都无 late effect。Host error同名映射outer；exchange dns/tls/timeout/truncated/transport->remote-unavailable，cancelled->cancelled，non-2xx/schema failure->protocol；无第二 channel。每个 query/language/domain/URL/output field/list/count/byte/line aggregate 分别覆盖 0/1/N/N+1；fetch aggregate N+1 fixture必须仍返回与URL等长且同序的pages并在page内截断，缺页或多页reject。fixture 另覆盖 URL `%2e%2e`、`%7e` alias、encoded control/Bidi/separator、malformed UTF-8、default port、duplicate canonical form 和 URL 10/11。T7 只用 in-memory exchange fixture；T17 接入 Querit/Synthetic 的真实 Host transport，不新增 generic HTTP 或改变 `0.0.1`。

## 9. `mcp` world

### 9.1 Exact signatures and local fields

- Imported interface: `mcode:feature-pack/mcp-host@0.0.1`。
- Exported interface: `mcode:feature-pack/mcp-pack@0.0.1`。
- Sole Host signature: `start-invoke(request: typed-invocation) -> result<own<mcp-exchange>,mcp-host-error>`。
- Exchange signature: `mcp-exchange.pull() -> mcp-exchange-pull`。
- Pack signatures: `invoke(request: mcp-request) -> result<own<mcp-operation>,mcp-error>`；`mcp-operation.pull() -> mcp-pull`。

| local type | exact fields/variants |
| --- | --- |
| `mcp-request` | `servers(servers-request) \| tools(tools-request) \| invoke(invoke-request)` |
| `servers-request` | `snapshot-digest:string, after:option<string>, limit:u16` |
| `tools-request` | `snapshot-digest:string, server-id:string, after:option<string>, limit:u16` |
| `invoke-request` | `snapshot-digest:string, schema-digest:string, server-id:string, tool-id:string, arguments:mcp-json-document` |
| `mcp-progress` | `discovering \| invoking` |
| `mcp-pull` | `pending \| progress(mcp-progress) \| complete(mcp-result) \| failed(mcp-error)` |
| `mcp-result` | `servers(server-page) \| tools(tool-page) \| invoked(mcp-output)` |
| `server-page` | `items:list<server-info>, next:option<string>` |
| `server-info` | `server-id:string, title:string` |
| `tool-page` | `server-id:string, items:list<tool-info>, next:option<string>` |
| `tool-info` | `tool-id:string, title:string, description:option<string>, schema-digest:string, schema:mcp-schema-document` |
| `mcp-output` | `text(string) \| json(mcp-json-document)` |
| `mcp-error` | `invalid-argument \| snapshot-mismatch \| schema-mismatch \| server-not-found \| tool-not-found \| protocol \| limit \| transport-unavailable \| cancelled` |
| `typed-invocation` | `snapshot-digest:string, schema-digest:string, server-id:string, tool-id:string, arguments:mcp-json-document` |
| `mcp-head` | `invocation-id:string, snapshot-digest:string, schema-digest:string` |
| `mcp-exchange-output` | `text(string) \| json(mcp-json-document)` |
| `mcp-frame` | `head(mcp-head) \| output(mcp-exchange-output) \| failed(mcp-failure) \| end` |
| `mcp-exchange-pull` | `pending \| frame(mcp-frame)` |
| `mcp-failure` | `transport \| protocol \| timeout \| cancelled` |
| `mcp-host-error` | `invalid-argument \| snapshot-mismatch \| schema-mismatch \| protocol \| limit \| transport-unavailable \| cancelled` |

### 9.2 Independent MCP AST and schema

`mcp-json-document` 是独立 family-local AST：`{root:u32,nodes:list<mcp-json-node>}`，nodes `1..=16,384`、depth `<=64`、logical charge `<=1 MiB`。

| AST type | exact fields/variants |
| --- | --- |
| `mcp-json-document` | `root:u32, nodes:list<mcp-json-node>` |
| `mcp-json-node` | `null \| boolean(bool) \| number(string) \| %string(string) \| array(mcp-json-array) \| object(mcp-json-object)` |
| `mcp-json-array` | `children:list<u32>` |
| `mcp-json-object` | `members:list<mcp-json-member>` |
| `mcp-json-member` | `key:string, value:u32` |

`mcp-json-node` 只使用上述 six cases；number/string payload 的 bounds 如下，array/object 必须使用 family-local named records，不能共享 Provider/Usage DTO。root 必须为最后 node；所有 node reachable；每个 non-root 恰好一个 parent；每个 child index 严格小于 parent index；array/object children/members 各 `0..=1,024`；arguments root 必须 object。number 是 `1..=128` ASCII bytes，exact grammar 是 `0|-?(?:[1-9][0-9]*(?:\.[0-9]*[1-9])?|0\.[0-9]*[1-9])(?:e-?[1-9][0-9]*)?`。key `Safe(256)`、UTF-8 byte-sorted 且 unique；string value `Safe(64 KiB)`。malformed index/reachability/parent/order/number/string、duplicate key 和 N+1 均在 start 前 reject。

每个 tool entry 携带 snapshot-bound、独立 `mcp-schema-document`：

| schema type | exact fields/variants |
| --- | --- |
| `mcp-schema-document` | `root:u32, nodes:list<mcp-schema-node>` |
| `mcp-schema-node` | `any \| null \| boolean \| number(mcp-number-schema) \| %string(mcp-string-schema) \| array(mcp-array-schema) \| object(mcp-object-schema)` |
| `mcp-number-schema` | `integer:bool, minimum:option<mcp-number-text>, maximum:option<mcp-number-text>` |
| `mcp-string-schema` | `min-bytes:u32, max-bytes:u32` |
| `mcp-array-schema` | `item:u32, min-items:u16, max-items:u16` |
| `mcp-object-schema` | `properties:list<mcp-schema-property>, additional:mcp-additional` |
| `mcp-schema-property` | `key:string, schema:u32, required:bool` |
| `mcp-additional` | `forbid \| allow-any \| schema(u32)` |
| `mcp-number-text` | alias of `string` with the canonical-number bounds below |

nodes `1..=4,096`、logical charge `<=512 KiB`、depth `<=64`、single-parent、root last、all reachable；root 为 object。number bounds 为 None 或 `1..=128` ASCII canonical number，按 exact rational comparison；min 不得大于 max，integer=true 时边界也必须为整数；string bounds 为 `0..=65,536`，array bounds 为 `0..=1,024`，各自 min 不得大于 max。property key 是 `Safe+(256)`，按 byte-sort unique，最多 `1,024` 个；所有 schema reference 都是 child index。evaluator递归精确匹配：any接受任一 node；null/boolean/string/array/object仅同名case；number仅number，integer=true要求exact rational denominator=1。min/max inclusive exact arbitrary-precision rational；string按UTF-8 bytes；array按item count并逐项验item schema；object required必须存在，declared property按schema，undeclared在forbid拒绝、allow-any接受、schema(i)逐项验证。arguments 与 JSON response 均完整消费 root，并针对同一个 tool snapshot 的同一 `mcp-schema-document` 校验；不存在独立 output schema。closed `any` node 与 `allow-any` additional 是本文明确授权的 bounded AST cases，仍受 document node/depth/charge/Safe/graph bounds，绝不是 raw `Value`、map、raw JSON 或 extensions escape。无coercion/default/union/pattern/unknown。schema-digest为lowercase sha256 over `mcode-mcp-schema-v1\0`+schema node declaration order；tool snapshot digest为sha256 over `mcode-mcp-tool-snapshot-v1\0`+当前 immutable Pack catalog的完整 server list：server按server-id byte order，每项编码 `{server-id,server-title,tools}`，包括zero-tool server；tools按tool-id byte order，每项编码 `{tool-id,title,description,schema-digest}`。两者 framing：string/list=u32be length/count，integer fixed big-endian，bool=00/01，option=00/01+payload，variant=zero-based u8，record按上述声明field order，conversion checked。digest golden必须证明修改server title、添加或删除zero-tool server、修改任一tool field均改变snapshot digest。

successful轨迹的first non-pending frame必须且只能head；success=head->恰一output->end，failed可作为first non-pending或在head后替换未完成suffix。每pull最多一frame、最多3 non-pending、one-frame buffering，consumer pull前不生产下一帧；terminal后zero pull，post-terminal protocol failure。start Host error同名映射outer；exchange transport/timeout->transport-unavailable，protocol->protocol，cancelled->cancelled；digest mismatch映射对应outer mismatch，无第二channel。fixture覆盖pre-head transport failure与head-then-disconnect。`snapshot-digest`、`schema-digest` 都是 lowercase `sha256:` 加 64 lowercase hex。server/tool ID 是 `VisibleAscii(256)`；server title、tool title 是 `Label(256)`，description 为 None 或 `0..=4 KiB` Safe。cursor 是 None 或 Host-issued `mcpc1-[0-9a-f]{32}`；server page items `0..=limit`，按 server ID byte-sort unique；tool page 的 echoed server ID 必须等于 request，items `0..=limit`、按 tool ID byte-sort unique；limit `1..=128`。cursor private row 绑定 caller/Pack/generation/`OperationId`、server、snapshot digest、完整 query 与 last ID。

typed-invocation五 fields按声明顺序与bound invoke request exact typed structural equality。Host生成 `invocation-id=mcp1-[0-9a-f]{32}`；head invocation ID必须命中本次single-use Host row，head digest等于request且一次。text Safe(1MiB)，JSON <=1MiB且重验schema；terminal invoked与accepted output exact typed structural equality。server/tool not found、malformed/foreign/crossed invocation ID、crossed digest/cursor/head/output 在 retain 前 reject；fixture逐项覆盖这些 invocation ID failures。每个 identity/title/description/cursor/page/output/AST aggregate 分别覆盖 0/1/N/N+1。MCP 不暴露 process、socket、URL、path、stdio、raw JSON-RPC 或当前 vault credential。`servers`/`tools` 使用 Pack snapshot，Host 只参与 `invoke`。T7 验证各自 AST/schema graph 和 crossed snapshot；T18 负责真实 Host transport。

## 10. `usage` world

### 10.1 Exact signatures and local fields

- Imported interface: `mcode:feature-pack/usage-host@0.0.1`。
- Exported interface: `mcode:feature-pack/usage-pack@0.0.1`。
- Sole Host signature: `start-refresh() -> result<own<usage-exchange>,usage-host-error>`，source context 在 Host binding 中预先固定。
- Exchange signature: `usage-exchange.pull() -> usage-exchange-pull`。
- Pack signatures: `invoke(request: usage-request) -> result<own<usage-operation>,usage-error>`；`usage-operation.pull() -> usage-pull`。

| local type | exact fields/variants |
| --- | --- |
| `usage-request` | `ingest(ingest-request) \| render-summary(render-summary-request) \| render-details(render-details-request) \| refresh(refresh-request)` |
| `ingest-request` | `source-view:usage-source-view, sample:usage-sample-view` |
| `render-summary-request` | `source-view:usage-source-view, state:usage-render-state-view` |
| `render-details-request` | `source-view:usage-source-view, state:usage-render-state-view` |
| `refresh-request` | `source-view:usage-source-view` |
| `usage-progress` | `normalizing \| refreshing` |
| `usage-pull` | `pending \| progress(usage-progress) \| complete(usage-result) \| failed(usage-error)` |
| `usage-result` | `ingested \| summary(summary-result) \| details(details-result) \| refreshed` |
| `summary-result` | `rows:list<usage-row>` |
| `details-result` | `cards:list<usage-card>` |
| `usage-row` | `id:string, label:string, value:string, tone:usage-tone` |
| `usage-tone` | `neutral \| info \| success \| warning \| error` |
| `usage-card` | `id:string, title:string, rows:list<usage-row>` |
| `usage-error` | `invalid-argument \| stale-stamp \| duplicate-sample \| source-unavailable \| limit \| cancelled` |
| `usage-source-view` | `source:string, source-stamp:string` |
| `usage-render-state-view` | `state-stamp:string, source:string, source-contract-digest:string, consumer-pack-generation:u64, accepted-samples:list<usage-sample-view>, latest-refresh:option<usage-wire-document>` |
| `usage-sample-view` | `source:string, sample-id:string, producer-provider:string, producer-route:string, producer-request:string, producer-turn:string, current-model:string, requested-model:string, requested-alias:option<string>, resolved-model:option<string>, counters:usage-counters, source-contract-digest:string, producer-pack-hash:string, producer-pack-generation:u64, producer-route-generation:u64, terminal:bool` |
| `usage-counters` | `input:option<u64>, output:option<u64>, cache-read:option<u64>, cache-write:option<u64>` |
| `usage-host-error` | `authority-rejected \| protocol \| source-unavailable \| limit \| cancelled` |
| `usage-head` | `status:u16, source-contract-digest:string, pack-generation:u64` |
| `usage-document-frame` | `value:usage-wire-document` |
| `usage-frame` | `head(usage-head) \| document(usage-document-frame) \| failed(usage-failure) \| end` |
| `usage-exchange-pull` | `pending \| frame(usage-frame)` |
| `usage-failure` | `transport \| protocol \| timeout \| cancelled` |

### 10.2 Independent Usage AST and `UsageSourceContractV1`

`usage-wire-document` 独立定义为 `{root:u32,nodes:list<usage-wire-node>}`，nodes `1..=16,384`、depth `<=64`、logical charge `<=1 MiB`。

| AST type | exact fields/variants |
| --- | --- |
| `usage-wire-document` | `root:u32, nodes:list<usage-wire-node>` |
| `usage-wire-node` | `null \| boolean(bool) \| number(string) \| %string(string) \| array(usage-wire-array) \| object(usage-wire-object)` |
| `usage-wire-array` | `children:list<u32>` |
| `usage-wire-object` | `members:list<usage-wire-member>` |
| `usage-wire-member` | `key:string, value:u32` |

root object 且 last，all reachable，non-root one parent，child index lower than parent。node 只使用上述 six cases；number/string bounds 如下，array/object 使用 Usage-local named records。array/object 各 `0..=1,024`；number 是 `1..=128` ASCII bytes，使用与 MCP 相同的 exact canonical grammar，但类型定义、validator、schema 和 digest 完全独立；key `Safe(256)` byte-sorted unique，string `Safe(64 KiB)`。Usage WIT 不传 raw wire bytes，exchange 传的是已建模的 AST document。

Host 绑定一个 closed `UsageSourceContractV1`：`{version=1, manager-id:string, pack-id:string, pack-version:string, pack-hash:string, publisher-source-id:string, canonical-source-key:string, operation-id=refresh, authority-digest:string, schema:usage-source-schema}`，总 logical charge `<=1 MiB`。`canonical-source-key` 必须恰为 `1..=256` lowercase ASCII bytes：首字节 `a-z`，末字节 alphanumeric，内部只能 alphanumeric 或单个 `-._:/` separator；separator 不得相邻，按 `/` 分隔的每个 segment 非空且不能为 `.` 或 `..`，验证与 digest 均使用 exact input bytes。`usage-source-view` 是 Host-issued selector projection：`source` 为该 canonical key；`source-stamp` 为 `usrc1-[0-9a-f]{32}`。private table row 同时绑定 caller、Usage Manager、Usage Pack ID/hash/generation、source contract/digest、declarative outer `OperationId` 与 active root-config position；这些 private fields 不进入 body。四个 request case 都必须携带该 view；严格 decode 后 Host 以它唯一解析 N active Usage Packs 并建立 immutable `ResolvedOperationBinding`。`ingest.sample.source`、contract digest、producer Pack/generation 与 view row 必须一致。unknown/stale/crossed source/stamp/Pack/generation 在 operation allocation、`start-refresh` 与任何 retained mutation 前拒绝。Usage schema 的 family-local types 是：

| schema type | exact fields/variants |
| --- | --- |
| `usage-source-schema` | `root:u32, nodes:list<usage-schema-node>` |
| `usage-schema-node` | `null \| boolean \| number(usage-number-schema) \| %string(usage-string-schema) \| array(usage-array-schema) \| object(usage-object-schema)` |
| `usage-number-schema` | `minimum:option<usage-number-text>, maximum:option<usage-number-text>` |
| `usage-string-schema` | `min-bytes:u32, max-bytes:u32` |
| `usage-array-schema` | `item:u32, min-items:u16, max-items:u16` |
| `usage-object-schema` | `properties:list<usage-schema-property>, additional:usage-additional` |
| `usage-schema-property` | `key:string, schema:u32, required:bool` |
| `usage-additional` | sole enum case `forbid` |
| `usage-number-text` | alias of `string` with the canonical-number bounds below |

nodes `1..=4,096`、logical charge `<=512 KiB`、depth `<=64`、root object、root last、all reachable、single-parent、child-before-parent。property key 为 `Safe+(256)`，sorted unique，最多 `1,024` 个；number 使用 exact `1..=128` canonical decimal grammar 与 exact rational comparison，string bounds `0..=65,536`，array bounds `0..=1,024`，min 不得大于 max，所有 references 是 child indices。没有 default/coercion/unknown field；每个 document node 必须被 schema 消费。authority-digest为lowercase sha256 over ASCII `mcode-usage-authority-v1\0` 后按contract declaration order编码 manager-id、pack-id、pack-version、pack-hash、publisher-source-id、canonical-source-key与固定 `operation-id=refresh`；它不包含transport target/header/credential。schema digest为lowercase sha256 over `mcode-usage-schema-v1\0`+schema declaration-order document。contract digest为lowercase sha256 over `mcode-usage-source-contract-v1\0`+version、全部identity/source fields、`operation-id`、authority-digest、schema（无self field），严格保持 `UsageSourceContractV1` declaration order。framing：string/list=u32be byte-length/count，unsigned=fixed big-endian，bool=00/01，option=00/01+payload，variant=zero-based u8，record按table field order，conversion checked；runtime另绑generation。unknown identity/schema/version、unconsumed document node、source/`operation-id`/`pack-generation`/digest mismatch 都 reject。contract parser/semantic goldens 必须只接受 declaration-order exact `operation-id=refresh`，拒绝任何 alternate field name、错位字段、其他 operation value及其 digest preimage；Usage WIT parser/semantic goldens必须只接受 exact `usage-head.pack-generation`，拒绝任何 alternate generation field name。

successful轨迹的first non-pending frame必须head，`usage-head.status` 必须精确位于 `200..=599`，且 `usage-head.pack-generation` 必须等于 signed source contract 所属 Usage Pack 的 bound generation；它绑定 signed source contract 的 Usage Pack generation，明确不同于 sample 的 producer Provider Pack generation与render-state的consumer Usage Pack generation，三者不得混用或相互比较。`failed` 可作为first non-pending或在head后替换未完成suffix。每pull最多一frame。2xx=head->document->end；non-2xx=head->failed；transport在head前为failed，在head后且document前为head->failed，在document后且end前为head->document->failed；failed后无后续document/end。最多3 non-pending、one-frame buffer，消费前不读下一帧；terminal后zero pull，post-terminal protocol failure。Host error authority-rejected->invalid-argument、protocol->source-unavailable，source-unavailable/limit/cancelled同名；exchange transport/timeout/protocol->source-unavailable，cancelled->cancelled，无第二channel。fixture覆盖 status 的 199/200/599/600、pre-head、head-then-disconnect、document-then-disconnect，以及 `pack-generation` 与 producer/consumer generation 的 crossed values。

四个operation data path固定：ingest在同一 outer task 预留恰一Host-prevalidated single-use sample view且无import，只有 accepted terminal `ingested` 才原子消费并写入 accepted state；任何 failed/cancel/protocol terminal 都回滚 reservation 与 state mutation。render-summary/details 无 Host import；Host 必须在 guest 前构造 request 内的 immutable `usage-render-state-view`，Pack 只能从该 request state 派生输出，不能读取 hidden guest history。state stamp 为 Host-issued `urst1-[0-9a-f]{32}`；accepted samples `0..=1,024`，按 sample ID bytes canonical sorted且在单份 state 内 unique；每项 `source` 与 `source-contract-digest` 必须分别等于 state 的 `source` 与 `source-contract-digest`，而 `state.consumer-pack-generation` 必须等于 private row 绑定的 consumer Usage Pack generation。sample 的 `producer-pack-generation` 始终解析 producer Provider Pack，可与 consumer generation 不同且绝不比较二者。accepted sample projection 可在多份 Host-built immutable render state 中重复 replay，不再次消费 ingest reservation；optional latest refresh document 已按同一 source schema验证。完整 state charge `<=1 MiB`，snapshot 在 operation 生命周期内 immutable。refresh alone调用start-refresh恰一次，只有 accepted terminal `refreshed` 才保留本次 document 为 latest；任何 failure 回滚 document/state mutation。outer declarative `OperationId=refresh` 只进入 outer binding，contract `operation-id=refresh` 只进入 signed source contract及其 authority/contract digest；名称相近但作用域独立，验证时不得以其中一个代替另一个。sample view只在sealed sample checks后产生且terminal=true，source/contract/producer stamps完全一致。四个 counter 独立 optional，`Some(0)` 是真实零，缺失保持 `None`。T19 负责真实 quota transport；T7 只验证 AST/schema/contract/reducer。

### 10.3 Usage output and stage

`usage-source-view.source` 与 `usage-sample-view.source` 都表示 Usage consumer 的 canonical source key；Host 在投影 sealed Provider sample 时按 signed source contract 设置它，绝不复制或猜测 Provider Pack source string。sample 的 `producer-pack-hash|producer-pack-generation` 明确指 producer Provider Pack，`producer-route-generation` 指 Provider route；private source-view row另绑consumer Usage Pack/hash/generation，二者不得混用。sample ID为Host-issued `usmp1-[0-9a-f]{32}` 并绑定source/producer/route/request/turn/terminal；single-use 只约束 `ingest-request.sample` 的一次 accepted insertion，duplicate/stale/foreign ingestion 在 Pack 前拒绝。成功进入 accepted state 后，同一 immutable sample projection 可出现在后续多次 render-summary/details state 中且不再次消费；单份 state 内仍按 sample ID unique。其余 sample/producer-*/model fields `1..=256` VisibleAscii；`source-contract-digest` 与 `producer-pack-hash` 均为 lowercase `sha256:` 加64 lowercase hex；producer generations `1..=i64::MAX`；alias/resolved model 为 None 或 `1..=256` VisibleAscii。每counter为None或Some(0..=i64::MAX)，sum/charge checked u64，overflow=limit。summary rows `0..=64`，row ID `1..=128`、label `1..=64`、value `0..=128` Safe；details cards `0..=16`，card ID与title各 `1..=128`，每card rows `0..=128`且row使用同一bounds；render result charge `<=256 KiB`，ingest/refresh request/result `<=1 MiB`。golden逐一覆盖row/card ID、label、value、title、state sample count/order/uniqueness/charge、latest document presence与两种digest grammar的0/1/N/N+1，并验证 ingest/refresh success-only retention及所有 failure rollback；另固定同一 accepted sample 依次用于 render-summary/details 均成功、duplicate ingestion 拒绝、producer/consumer generation 故意不等仍成功，以及 crossed `state.consumer-pack-generation` 拒绝。Usage 不查询 Provider、不读取 Session/widget/quota 猜模型、不选择 source key；render state 只能来自 Host 对 accepted sample/source state 的 replay，不能信任 guest-held history。Usage Manager 按 root config 顺序写入固定 `status.trailing/usage.summary` 与 `panel/usage.details`。

## 11. `subagents` world

### 11.1 Exact signatures and local fields

- Imported interface: `mcode:feature-pack/subagents-host@0.0.1`。
- Exported interface: `mcode:feature-pack/subagents-pack@0.0.1`。
- Host signatures: `run-step(request: step-request) -> result<step-output,subagents-host-error>`；`recover-step(request: recovery-request) -> result<recovery-output,subagents-host-error>`。
- Pack signatures: `invoke(request: subagents-request) -> result<own<subagents-operation>,subagents-error>`；`subagents-operation.pull() -> subagents-pull`。

| local type | exact fields/variants |
| --- | --- |
| `subagents-request` | `roles \| enqueue(enqueue-request) \| recover(recover-request)` |
| `enqueue-request` | `job-id:string, reservation:job-reservation-view, role:string, task:string, mode:job-mode, isolation:isolation-mode, retain-session:bool, review-target:option<string>, max-attempts:u8` |
| `recover-request` | `job-id:string` |
| `subagents-progress` | `queued(queued-progress) \| running(running-progress) \| review-round(review-round-progress) \| recovering` |
| `subagents-pull` | `pending \| progress(subagents-progress) \| complete(subagents-result) \| failed(subagents-error)` |
| `queued-progress` | `position:u16` |
| `running-progress` | `attempt:u8, phase:job-mode` |
| `review-round-progress` | `current:u8, total:u8` |
| `job-mode` | `run \| review \| fix` |
| `isolation-mode` | `shared \| worktree` |
| `role-info` | `id:string, title:string, modes:list<job-mode>` |
| `roles-result` | `items:list<role-info>` |
| `job-result` | `job-id:string, outcome:job-outcome, summary:string, retained-session-id:option<string>` |
| `job-outcome` | `success \| changes-requested \| failed` |
| `subagents-result` | `roles(roles-result) \| job(job-result)` |
| `step-request` | `job-id:string, attempt:u8, mode:job-mode` |
| `step-output` | `outcome:step-outcome, summary:string, retained-session-id:option<string>` |
| `step-outcome` | `continue \| success \| changes-requested \| failed` |
| `recovery-request` | `job-id:string` |
| `recovery-output` | `job-id:string, receipt:recovery-receipt` |
| `recovery-receipt` | `recovered(job-result) \| unrecoverable` |
| `job-reservation-view` | `reservation-id:string, job-id:string` |
| `subagents-error` | `invalid-argument \| role-not-found \| queue-full \| isolation-unavailable \| stale-job \| crash-unrecoverable \| limit \| unavailable \| cancelled` |
| `subagents-host-error` | `isolation-unavailable \| stale-job \| crash-unrecoverable \| limit \| unavailable \| cancelled` |

### 11.2 Semantics and stage

Host在Pack前mint并共同pre-reserve `sub1-[0-9a-f]{32}` job ID与 `sjr1-[0-9a-f]{32}` reservation；guest string不能mint，enqueue exact-return bound ID。role LocalId(128)，task Safe+(64KiB)，attempt/max-attempts 1..8。`review-target` 在 mode `run` 时必须 None，在 `review|fix` 时必须是另一条 `sub1-...` Host table job view且不能 self-reference。queue position `0..=1,024`；review current/total `1..=8` 且 current `<=total`。roles items `0..=128`、按 role ID byte-sort unique；title `Label(256)`；modes `1..=3`、unique 且按 `run,review,fix` 固定顺序。

job/step summary 是 `0..=64 KiB` Safe；retained-session-id 为 None 或 Host-issued `rs1-[0-9a-f]{32}`。所有 progress 仍可按第2.8节整体省略。queued若出现仅一次且先于任何attempt progress；某attempt若发progress，必须先且只发一次 `running{attempt,phase=fixed mode}`，review mode随后至多一次review-round(current=attempt,total=max-attempts)，不得在没有同attempt running时单独发review-round。不同attempt按 `running(1),review-round(1)?,running(2),review-round(2)?...` 交替，整个attempt的progress pair可省略。attempt从1 contiguous，无skip/repeat；mode/isolation/target/retain/max在reservation后固定。每个 step request必须等于private row contiguous next step；`step-output.continue` 必须 retained-session-id=None，其他 outcome 的 summary/session 只可复制该 step receipt。`retain-session=false` 时所有 retained ID 必须 None；true 时也不推导缺失值。`job-result.job-id` 必须等于 outer job；success/changes-requested/failed 分别复制 terminal step outcome。recovery request job ID、`recovery-output.job-id` 与 `recovered(job-result).job-id` 必须三者 exact equal。recover只为terminal crashed retained row签发single-use reservation：unrecoverable->failed(crash-unrecoverable)；`recovered(result)` 仅返回 Host-sealed 的完整 terminal `job-result`（job-id、outcome、summary、retained-session-id），不恢复/继续job，Pack 的 outer `complete(job(result))` 必须逐字段 exact copy该 receipt，不能伪造或依赖 hidden guest state。receipt消费后、double/repeat、nonterminal或其他terminal recover均stale-job且zero run-step；outcome/summary/retained ID 全部来自同一 pre-bound terminal row。fixture覆盖 crossed recovery output/receipt job ID、outcome、summary、retained-session-id、任一 terminal field mutation、两个review attempt的交替progress、整个attempt progress省略及review-round without running拒绝。每个 role/job/attempt/outcome/summary/session field与 list覆盖 0/1/N/N+1。Pack 不接收 worktree path、process ID、command、filesystem/process handle。T20 负责 isolation/queue/recovery；T7 只验证 typed step/recovery。

## 12. `workspace` world

### 12.1 Exact signatures and local fields

- Imported interface: `mcode:feature-pack/workspace-host@0.0.1`。
- Exported interface: `mcode:feature-pack/workspace-pack@0.0.1`。
- Host signatures: `scan(request: scan-request) -> result<scan-page,workspace-host-error>`；`apply-rollback(request: rollback-request) -> result<rollback-output,workspace-host-error>`。
- Pack signatures: `invoke(request: workspace-request) -> result<own<workspace-operation>,workspace-error>`；`workspace-operation.pull() -> workspace-pull`。

| local type | exact fields/variants |
| --- | --- |
| `workspace-request` | `checkpoint(checkpoint-request) \| inspect(inspect-request) \| rollback(rollback-request)` |
| `checkpoint-request` | `reservation:checkpoint-reservation-view` |
| `inspect-request` | `checkpoint-id:string, fingerprint:string, offset:u32, limit:u16` |
| `rollback-request` | `checkpoint-id:string, expected-current:string, reservation:checkpoint-reservation-view` |
| `workspace-progress` | `scanning \| snapshotting \| rolling-back` |
| `workspace-pull` | `pending \| progress(workspace-progress) \| complete(workspace-result) \| failed(workspace-error)` |
| `workspace-result` | `checkpoint(checkpoint-result) \| inspected(inspected-result) \| rolled-back(rolled-back-result)` |
| `checkpoint-result` | `checkpoint-id:string, fingerprint:string, files:u64, dirs:u64, bytes:u64` |
| `workspace-path` | alias of `string` with local canonical-path rules |
| `change` | `path:workspace-path, tracking:tracking-kind, kind:change-kind, hash:option<string>` |
| `inspected-result` | `items:list<change>, next:option<u32>` |
| `conflict-result` | `paths:list<workspace-path>, truncated:bool` |
| `rolled-back-result` | `fingerprint:string` |
| `workspace-error` | `invalid-argument \| not-found \| conflict(conflict-result) \| unrollbackable \| unsafe-entry \| limit \| unavailable \| cancelled` |
| `scan-request` | `checkpoint-id:string, fingerprint:string, offset:u32, limit:u16` |
| `scan-page` | `items:list<change>, next:option<u32>, snapshot:workspace-snapshot-view` |
| `workspace-snapshot-view` | `fingerprint:string, files:u64, dirs:u64, bytes:u64` |
| `rollback-output` | `fingerprint:string` |
| `tracking-kind` | `tracked \| untracked \| ignored` |
| `change-kind` | `added \| modified \| deleted \| metadata \| unrollbackable` |
| `checkpoint-reservation-view` | `checkpoint-id:string, reservation-id:string, expected-current:string` |
| `workspace-host-error` | `not-found \| conflict(conflict-result) \| unrollbackable \| unsafe-entry \| limit \| unavailable \| cancelled` |

### 12.2 Semantics and stage

唯一 rollback-request同时作为outer payload与Host argument。`workspace-path`是本world local typed alias和Host canonical relative Safe data：`1..=512` UTF-8 bytes、`1..=128` components；拒绝 NUL/control/Bidi、empty/`.`/`..`、backslash、absolute root、drive、duplicate separator和 trailing separator。Host 生成 `checkpoint-id=cp1-[0-9a-f]{32}` 与 `reservation-id=wsr1-[0-9a-f]{32}`；fingerprint/current fence 是 lowercase `sha256:` 加 64 lowercase hex。change hash 为 None 或同格式 digest；files/dirs/bytes `0..=i64::MAX`。

scan offset `0..=u32::MAX`、limit `1..=256`；scan-import与operation-pull各有独立65536 cap。inspect首个scan request必须与outer `checkpoint-id,fingerprint,offset,limit` exact typed structural equality。checkpoint首个scan固定为 `{checkpoint-id=reservation.checkpoint-id,fingerprint=reservation.expected-current,offset=0,limit=256}`；rollback要求outer与reservation的checkpoint-id/expected-current相等，首个scan固定为 `{checkpoint-id=outer.checkpoint-id,fingerprint=outer.expected-current,offset=0,limit=256}`。后续scan保持同checkpoint/fingerprint/limit并原样使用前页next作为offset；非EOF next=`offset+items.len` checked exact且strict forward；EOF None，empty+Some/stale/replay reject。path byte-sorted unique。每页 snapshot exact typed structural equality；完整scan receipt绑定所有pages、snapshot/fingerprint/reservation/expected-current。任一unsafe/unrollbackable/missing/crossed page/snapshot/fence使receipt invalid；因为 invalid page 只能在对应 `scan` import 已返回后被观察，该次及此前 scan 已是 observed Host effect，不能声称 workspace effect 为零。validator 必须立即停止，不再发起后续 `scan`，绝不调用 `apply-rollback`，并保持 zero durable workspace mutation；只有 pre-import request/binding rejection 才是 scan x0。checkpoint result只能复制final view。conflict paths 0..16、typed path byte-sorted unique。fixtures分别验证inspect/checkpoint/rollback首个scan exact argument，并拒绝crossed offset/limit/fingerprint；每个 invalid-page fixture 断言检测该页所需的 exact observed scan count、后续 scan x0、`apply-rollback` x0与 zero durable mutation，pre-import rejection另断言 scan x0。每个 path/ID/digest/count/page/snapshot/conflict aggregate 分别覆盖 0/1/N/N+1。T13 负责 Host scan/rollback；T7 只验证 path/cursor/fence reducer。

## 13. `ui` world

### 13.1 Exact signatures and local fields

- No Host import。
- Exported interface: `mcode:feature-pack/ui-pack@0.0.1`。
- Pack signature: `invoke(request: ui-request) -> result<own<ui-operation>,ui-error>`；`ui-operation.pull() -> ui-pull`。

| local type | exact fields/variants |
| --- | --- |
| `ui-request` | `render-runtime(render-runtime-request) \| handle-action(handle-action-request) \| resolve-theme(resolve-theme-request)` |
| `render-runtime-request` | `revision:u64, viewport:viewport, effective-capabilities:effective-capabilities, model:ui-model` |
| `handle-action-request` | `revision:u64, action:ui-action` |
| `resolve-theme-request` | `revision:u64, effective-capabilities:effective-capabilities` |
| `viewport` | `columns:u16, rows:u16` |
| `effective-capabilities` | `color:color-capability, unicode:bool, images:bool, hyperlinks:bool` |
| `color-capability` | `no-color \| basic \| ansi256 \| true-color` |
| `ui-model` | `transcript:list<transcript-line>, composer:string, status:list<status-item>, panels:list<panel>, overlay:option<overlay>, picker:option<picker-view>, notifications:list<notification-view>, images:list<image-projection>, hyperlinks:list<hyperlink-projection>` |
| `transcript-line` | `role:transcript-role, content:ui-content` |
| `transcript-role` | `user \| assistant \| tool \| system` |
| `ui-content` | `lines:list<content-line>` |
| `content-line` | `spans:list<content-span>` |
| `content-span` | `text(text-span) \| image(image-span)` |
| `text-span` | `text:string, hyperlink:option<hyperlink-stamp>` |
| `image-stamp` | alias of `string`; `uimg1-[0-9a-f]{32}` |
| `hyperlink-stamp` | alias of `string`; `ulnk1-[0-9a-f]{32}` |
| `image-span` | `image:image-stamp` |
| `status-item` | `id:string, label:string, value:string, tone:ui-tone` |
| `ui-tone` | `neutral \| info \| success \| warning \| error` |
| `panel` | `id:string, title:string, body:ui-content` |
| `overlay` | `kind:overlay-kind, title:string, body:ui-content` |
| `overlay-kind` | `dialog \| help` |
| `picker-view` | `id:string, title:string, query:string, items:list<picker-item>, selected:option<u16>` |
| `picker-item` | `id:string, label:string, detail:option<string>, disabled:bool` |
| `notification-view` | `id:string, tone:ui-tone, title:string, body:ui-content, actions:list<notification-button>` |
| `notification-button` | `id:string, label:string` |
| `image-projection` | `stamp:image-stamp, media-type:image-media-type, pixel-width:u32, pixel-height:u32, frame-count:u16, alt:string` |
| `image-media-type` | `png \| jpeg \| gif \| webp \| tiff` |
| `hyperlink-projection` | `stamp:hyperlink-stamp, label:string` |
| `ui-action` | `none \| submit-text(submit-text-action) \| focus(focus-action) \| scroll(scroll-action) \| dismiss-overlay \| picker(picker-action) \| notification(notification-action) \| activate-hyperlink(activate-hyperlink-action)` |
| `submit-text-action` | `text:string` |
| `focus-action` | `target:focus-target` |
| `focus-target` | `composer \| transcript \| panel(string) \| picker(string) \| overlay` |
| `scroll-action` | `target:scroll-target, delta:s16` |
| `scroll-target` | `transcript \| panel(string) \| picker(string) \| overlay` |
| `picker-action` | `move(picker-move) \| select(picker-select) \| cancel(picker-cancel)` |
| `picker-move` | `picker-id:string, delta:s16` |
| `picker-select` | `picker-id:string, item-id:string` |
| `picker-cancel` | `picker-id:string` |
| `notification-action` | `dismiss(notification-dismiss) \| activate(notification-activate)` |
| `notification-dismiss` | `notification-id:string` |
| `notification-activate` | `notification-id:string, action-id:string` |
| `activate-hyperlink-action` | `stamp:hyperlink-stamp` |
| `ui-progress` | `rendering` |
| `ui-pull` | `pending \| progress(ui-progress) \| complete(ui-result) \| failed(ui-error)` |
| `ui-result` | `frame(frame-result) \| action(action-result) \| theme(theme-result)` |
| `frame-result` | `revision:u64, viewport:viewport, clear:frame-clear, paints:list<paint-run>` |
| `frame-clear` | sole enum case `all` |
| `paint-run` | `row:u16, column:u16, content:paint-content, semantic-style:ui-style` |
| `paint-content` | `text(paint-text) \| image(paint-image)` |
| `paint-text` | `text:string, hyperlink:option<hyperlink-stamp>` |
| `paint-image` | `image:image-stamp, columns:u16, rows:u16` |
| `ui-style` | `foreground:theme-token-name, background:option<theme-token-name>, attributes:ui-attributes` |
| `ui-attributes` | flags `bold, dim, italic, underline, reverse, strikethrough` |
| `ui-color` | `default \| indexed(u8) \| rgb(rgb-color)` |
| `rgb-color` | `red:u8, green:u8, blue:u8` |
| `action-result` | `revision:u64, command:ui-command` |
| `ui-command` | `none \| submit-text(submit-text-command) \| focus(focus-command) \| scroll(scroll-command) \| dismiss-overlay \| picker(picker-command) \| notification(notification-command) \| open-hyperlink(open-hyperlink-command)` |
| `submit-text-command` | `text:string` |
| `focus-command` | `target:focus-target` |
| `scroll-command` | `target:scroll-target, delta:s16` |
| `picker-command` | `move(picker-move) \| select(picker-select) \| cancel(picker-cancel)` |
| `notification-command` | `dismiss(notification-dismiss) \| activate(notification-activate)` |
| `open-hyperlink-command` | `stamp:hyperlink-stamp` |
| `theme-result` | `revision:u64, tokens:list<theme-token>` |
| `theme-token` | `token:theme-token-name, color:ui-color, attributes:ui-attributes` |
| `theme-token-name` | `background \| surface \| surface-raised \| text-primary \| text-muted \| text-dim \| border \| border-focus \| accent \| accent-muted \| success \| warning \| error \| info \| selection-background \| selection-text \| input-background \| input-text \| status-background \| status-text \| tool-title \| tool-output \| markdown-heading \| markdown-link \| markdown-code \| markdown-quote \| diff-added \| diff-removed \| diff-context \| syntax-comment \| syntax-keyword \| syntax-function \| syntax-variable \| syntax-string \| syntax-number \| syntax-type \| syntax-operator \| syntax-punctuation \| progress-track \| progress-fill` |
| `ui-error` | `invalid-argument \| wrong-role \| stale-revision \| unsupported-surface \| limit \| unavailable \| cancelled` |

### 13.2 Model, projection, action and theme invariants

revision 是 Host table-validated `1..=i64::MAX` scalar；guest 不能凭 revision 改 state。viewport columns `1..=512`、rows `1..=256`。每个 request 的 capability/model/projection row 都绑定同一 caller、runtime-or-theme role、Pack/hash/generation 与 revision；stale/crossed revision 在 Pack 前 reject。capability resolution固定：ForceOff=false；Auto=Host detected；ForceOn仅在Host capability available时true，否则Pack前 unsupported-surface；color取policy与detected共同支持的最高closed level。Host在Pack前拒绝zero viewport。guest不能扩大。

model 的 transcript `0..=512`、status `0..=64`、panels `0..=16`、notifications `0..=64`、images/hyperlinks 各 `0..=256`；picker items `0..=256`，notification actions `0..=4`，完整 model logical charge `<=1 MiB`。content 每 value `0..=1,024` lines、每 line `0..=1,024` spans；text/composer `0..=64 KiB` Safe。status/panel/picker/notification/item/button ID 都是 `LocalId(128)` 且在其 own collection unique；title/label 是 `Label(256)`，status value与picker query是required `0..=1 KiB` Safe；picker detail才是None或该bound。三者单行且拒绝TAB/LF；table optionality是authority。image alt `0..=256` Safe，pixel dimensions `1..=16,384`、frames `1..=64`。

image stamp 是 Host-issued `uimg1-[0-9a-f]{32}`，hyperlink stamp 是 `ulnk1-[0-9a-f]{32}`；private row 绑定同一 revision/generation 的 bytes 或 canonical URL/target，但 Pack DTO、command 和 frame 永不含 URL、href、path、bytes、base64、terminal sequence 或 raw handle。每个 content/paint/action/command 的 image/hyperlink stamp 必须命中 model projection 中恰好一项；每个 panel/picker/notification/action ID 同样精确解析。picker selected 为 None 或小于 items.len 且 item 非 disabled；disabled item 不可 select。picker move/scroll delta `-256..=256`；notification activate 必须命中该 notification 的 button；dismiss-overlay 仅在 overlay present 时有效。所有 command 只是 Host revalidated intent，不创建 authority。

case一一对应；result revision等于request，frame viewport exact typed structural equality。handle-action必须绑定same-revision accepted model row。command variant与payload exact typed structural equality；尤其 input `submit-text-action.text` 与 output `submit-text-command.text` 都必须是 `Safe+(64 KiB)`，并以 structural equality 保持完全相同的 UTF-8 bytes。仅activate-hyperlink重命名open-hyperlink但stamp相等；none只对应input none。crossed/stale/disabled在effect前reject。submit-text fixture覆盖 empty、1、N、N+1 UTF-8 bytes、control、Bidi以及任一byte mismatch。

`theme-result.tokens` 必须恰好 40 项，按上表 declaration order 各出现一次，不允许缺项、重复、额外 token 或 string extension；kebab-case wire name 按 ordinal 一一映射当前 `SemanticToken::ALL` 的 snake_case Rust name。`no-color` 只接受 `default`；`basic` 接受 `default|indexed(0..=15)`；`ansi256` 接受 `default|indexed(u8)`；`true-color` 接受全部 `ui-color` cases。runtime style 只能引用该 closed enum；Host 用已绑定 theme table 解析 foreground/background，最终 attributes 是 foreground token、可选 background token 与 paint attributes 的 set union。

### 13.3 Complete frame-to-cell-grid reducer

Host private schema固定为 `cell-grid{columns:u16,rows:u16,cells:list<cell>}`、`cell{cluster:string,style:ui-style,annotation:option<hyperlink-stamp>,owner:cell-owner}`、`cell-owner=blank|text{origin:u32,width:u8}|wide-continuation{owner:u32,offset:u8}|image{stamp:image-stamp,owner:u32,row-offset:u16,column-offset:u16}`；coordinates/owner index均0-based row-major，schema不进入public WIT。frame.clear=all。Host在scratch grid原子验证/reduce；错误丢弃scratch且published不变。Host 在保留任何 paint 前先验证全部 fields/references/charge；成功后分配 exactly `viewport.columns * viewport.rows` 个 row-major cells，并将每格重置为 one-space blank，style 固定为 `foreground=text-primary, background=Some(background), attributes=empty`，且无 annotation。每一新 frame 从空 grid 开始；empty paints 与 viewport shrink 因而清除旧 frame，绝不保留 stale cell。paints `0..=8,192` 且完整 frame logical charge `<=1 MiB`；list order 是唯一 paint order，later paint wins。

每个 paint origin 的 `row`、`column` 都是 0-based，且必须分别满足 `row < viewport.rows`、`column < viewport.columns`；任一 text/image origin 越界都拒绝整个 frame。text paint 为 `Safe+(64 KiB)`、不得含 TAB/LF。Host 使用 locked `unicode-segmentation=1.13.3` 的 extended grapheme algorithm 与 `unicode-width=0.2.0` 的 `UnicodeWidthStr::width`。text cursor 从 paint `column` 开始，row 固定为 origin row，永不 wrap 或改变 row；predecessor 初始为空且只记录本 paint 最近一次成功写入的 text lead。对每个 grapheme，width 只可 0、1、2，width >2 拒绝整个 frame。width 1/2 且全部落在 row 内时，先按 owner 规则清除其将覆盖的所有完整 owner，再写 lead（width 2 另写 continuation），把 predecessor 设为该 lead并将 cursor checked 前进对应 width。若处理任一 cluster 时 cursor 已在/越过 right edge，或 width 1/2 会跨过 edge，则在清除任何 owner 前丢弃整个 cluster，把 cursor 固定为 `viewport.columns`、清空 predecessor；此后所有 cluster（包括 width-0 combining）都不产生 write。width 0 只附着到本 paint 立即前一个成功写入且仍为 predecessor 的 lead，不移动 cursor；没有 predecessor时，仅当 `unicode=true` 且 dotted-circle 的 width 1 在当前 cursor完整可放入时，先按相同 owner 规则物化 `U+25CC` lead、cursor前进1，再把 cluster附着到它；若 dotted-circle不完整fit，则按right-edge规则饱和并停止后续write。`unicode=false` 时所有 paint text/alt 必须 ASCII。覆盖已有 wide/image owner 的任一 cell 前先把该 owner 的完整 span 重置为上述 fixed clear blank，之后再写新 owner；孤立 continuation 永远 invalid。

image paint columns/rows 各 `1..=viewport`，origin 必须在 viewport；Host将rectangle clip到viewport；每cell保留同stamp、未裁剪origin owner与从该origin起算的exact row/column offset；重叠先清完整owner，later wins，不读取bytes。`images=false` 时 image paint reject，Pack 必须自行用 projection alt 生成 text fallback；`hyperlinks=false` 时带 hyperlink annotation 的 text paint reject。capability=true 也只允许 same-revision Host stamp。T12 的最终 adapter 才把 accepted cell grid 转成 terminal escapes、resolved hyperlink 或 image protocol；T7 不接触 terminal/network/bytes。

### 13.4 UI semantic gates and stage

每个 list/string/ID/stamp/reference/coordinate/rectangle/capability 分别覆盖 0/1/N/N+1；token fixtures 覆盖 exact 40/order、missing/duplicate/41st、kebab/snake ordinal mapping。grid fixtures 覆盖 full clear、empty/shrink、paint order、right clip、wide-at-last-column、right-edge后继续text、right-edge后combining zero-write、wide/image overlap、combining attach/orphan materialization、text/image origin row或column等于viewport bound时 whole-frame reject、image rectangle clip、hyperlink/image/unicode off、repeat reduction byte identity。安全 fixture 扫描 DTO/AST 禁止 URL/image bytes/raw terminal/key/paste/IME/clipboard/handle；Host 只在相同 revision/generation row 下接受 open-hyperlink/image projection。UI 只交换 semantic model/action/projection/token/paint/cell data；Host独占editor caret/selection/editing、terminal capability、focus/input、paste/IME、clipboard、UTF-8 boundary与绘制；Pack拥有composer layout与semantic submit。T12先隔离当前generic internal `Widget`/`ReplaceBlocks`再适配ABI，不把它们变成ABI。

## 14. Text safety, redaction and family isolation

`Safe(n)` 是 decoded valid UTF-8 string，长度为 `0..=n` UTF-8 bytes（不是 Unicode scalar/grapheme count），并拒绝 CR、DEL、全部 C0（TAB/LF 例外）、全部 C1，以及 Unicode `Bidi_Control`：`U+061C`、`U+200E`、`U+200F`、`U+202A..U+202E`、`U+2066..U+2069`。`Safe+(n)` 使用完全相同的 UTF-8/exclusion rule但长度为 `1..=n` UTF-8 bytes。`Label(n)` 是 `1..=n` UTF-8 bytes 的 Safe 且不含 TAB/LF；`VisibleAscii(n)` 是 `1..=n` bytes 的可打印 ASCII；`LocalId(n)` 是 `1..=n` ASCII bytes 的 lowercase `[a-z][a-z0-9-]*`。所有 validator/golden 都按完整 decoded scalar 的 UTF-8 byte boundary构造 0/1/N/N+1 fixture，接受不超过N的完整多byte scalar，拒绝会超过N的整个scalar，绝不通过截断某个scalar来满足bound。

stable error 与 diagnostic 绝不携带 raw upstream text、raw secret、credential、stack、header 或 authority。redaction 的唯一例外仅是 typed Web success output 中已经通过本节 `Safe`/URL canonicalization、field/count/byte bounds 与 schema normalization 的 URL/title/text；Host只给该success payload附上不可由guest伪造的 `untrusted` provenance。每个 Web URL/title/text 即使已经canonicalize/normalize，也仍禁止进入任何 stable error 或 diagnostic；Host untrusted provenance不适用于error/diagnostic，且该例外不允许 raw body、header、error text、credential 或 `WebAuthorityBindingV1` 离开typed success payload。

11 个 world 的 type namespace、Host interface、validator、semantic JSONL golden 和 cursor/table key 均独立。资源 ownership 仅服务于 exported operation 以及 Web/MCP/Usage bounded exchange；任何 stable ID、cursor、revision、reservation、fence、sample、model 或 result 都不以 borrowed resource 表示。任何 cross-family selector、pack/generation mismatch、method 未列、case mismatch、credential-contract mismatch 在 allocation/Pack/effect 前失败。

## 15. T7 artifact slice and stage-owned gates

### 15.1 Artifact slice contract

紧随本文的 parseable artifact slice 必须同时提供：

1. `mcode:feature-pack@0.0.1` 的 11 个 world source WIT；每个 world 的 interface、field/variant 顺序与本文表逐项一致。
2. 11 个 resolved-world current LF WIT goldens，以及每个 world 的独立 semantic JSONL golden；每份 source WIT 只与其对应 resolved current WIT golden byte-identical，JSONL 不参与该字节比较。
3. `mcode:plugin/manager@0.0.1` 与 `mcode:provider-pack/provider@0.0.1` 的 current artifact reference；Provider surface 由 [08](08-provider-pack-abi.md) 定义。
4. 13 candidate worlds × 13 validators；只有 diagonal 通过。每次 T7 preflight 都从所调用 validator/current golden 接收 expected complete world ID，并按该 ID 对 top-level 与全部 nested shape 做 exact comparison；raw component binary 不要求揭示 source world name，validator 不得从 binary 猜测 expected world。T10 manifest 后续必须绑定同一个 complete world ID。对每个有参数的 freestanding/resource function 逐一只改一个 frozen parameter label，必须全部拒绝；same-name crossed shape、extra member、semver-compatible-but-not-exact package/interface/world name、WAT/core Wasm、`wasi:*`、ambient/raw Host、socket/filesystem/process/secret import/export 同样在 Store 前拒绝。

本文的 tables 是 review authority；artifact slice 是 parser authority。没有 parser-checked artifact 时，本文只表示目标契约，不声称 source/golden/runtime 已完成。未来 parser golden 必须逐一覆盖所有 escaped keyword `%resource`、`%list`、`%string`，验证其 parse 后 semantic case name 分别仍为 `resource`、`list`、`string`，且 canonical Todo `OperationId=list` 未改变；遗漏转义、把 built-in `list<T>|string` 误转义或引入额外 escaped case 均拒绝。T7 fixtures 必须测试 declaration `128/129`、两个 active Pack 共享一个 operation、crossed Provider/Usage/UI selector、case/role/generation mismatch，并证明这些 mismatch 在 task allocation、Pack/effect、credential 和 network 前为 zero side effect。11 个 world 的 semantic goldens 必须逐 field/list/count/byte/charge/cursor/revision 覆盖 0/1/N/N+1，并逐行覆盖第 2.8 节 request/result case、progress order、import cardinality 与 duplicate side-effect zero-extra-effect。Web/MCP/Usage 还必须各自覆盖 closed reducer、one-frame backpressure、cumulative frame/byte/node/charge limit；pre-head failure、head-then-disconnect、late pull/frame、crossed snapshot/schema/source 和 infinite-frame 都合成唯一 stable terminal。

### 15.2 T7 and later ownership

| stage | exact responsibility | T7 不声称 |
| --- | --- | --- |
| T7 | source/golden equality、binary static preflight、logical-size/shape/table/cursor/reducer/redaction pure tests，含 Web/MCP/Usage backpressure/cumulative-limit N+1；无 Store、instantiate、credential、signature activation、network、real wire | runtime PASS 或 real Host effect PASS |
| T8 | Store/limiter/fuel/epoch、one-Store async owner loop/mutex、operation/exchange resources、generation、cancel/reload/quiescence、destructor failure、exactly-one outer terminal | family product effect 已完成 |
| T9 | Session durable storage、reservation/CAS、branch/event semantics | T7 durable PASS |
| T10 | signed bundle/manifest/source trust、install/activation/rollback，并将 manifest 绑定到 T7 validator/current golden 使用的同一 complete world ID；不读取或注入 vault credential | T7 credential/transport PASS |
| T11 | Provider runtime、catalog metadata/cache、route/adapter/grant/injection integration | T7 real Provider wire PASS |
| T12 | UI runtime/terminal Host | T7 terminal PASS |
| T13 | Workspace scan/checkpoint/rollback Host | T7 filesystem effect PASS |
| T14 | Resources real guest/Host integration over immutable embedded Pack data；仍 zero-import | T7 resource product PASS |
| T15 | Ask interaction Host | T7 interaction product PASS |
| T16 | Todo durable event Host | T7 Todo product PASS |
| T17 | Web Querit/Synthetic transport using frozen `web-host` | zero-import Web 或 ABI bump |
| T18 | MCP Host transport using frozen `mcp-host` | MCP guest process/socket access |
| T19 | Usage source transport using frozen `usage-host` | Usage guest raw bytes/endpoint/header/credential |
| T20 | Subagents isolation/queue/recovery | T7 process/worktree effect PASS |
| T21 | Compaction route-bound Host effect | Compaction network grant |

任何后续 stage 都必须使用本 `0.0.1` 的已冻结 typed surface；已经列入本契约的 Web/MCP/Usage methods 不得通过 ABI bump 推迟。若未来发生 breaking change，必须整体发布新的 package/world/current golden，不在当前 surface 加解释分支。
