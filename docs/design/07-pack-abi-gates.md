# FeaturePack ABI: safety and artifact gates

> 返回 [07-pack-abi.md](07-pack-abi.md)。本文件保留原 authority 的章节编号与规范效力。

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
