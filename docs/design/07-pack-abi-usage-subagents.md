# FeaturePack ABI: usage and subagents

> 返回 [07-pack-abi.md](07-pack-abi.md)。本文件保留原 authority 的章节编号与规范效力。

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
