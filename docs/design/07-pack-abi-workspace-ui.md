# FeaturePack ABI: workspace and UI

> 返回 [07-pack-abi.md](07-pack-abi.md)。本文件保留原 authority 的章节编号与规范效力。

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
