# FeaturePack ABI: ask and todo

> 返回 [07-pack-abi.md](07-pack-abi.md)。本文件保留原 authority 的章节编号与规范效力。

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
