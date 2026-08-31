# FeaturePack ABI: session, compaction and resources

> 返回 [07-pack-abi.md](07-pack-abi.md)。本文件保留原 authority 的章节编号与规范效力。

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
