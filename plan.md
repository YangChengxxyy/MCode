# MCode 交付计划

> 这是 MCode 首个开发者预览的依赖有序规范性执行入口；所有实现必须满足相应依赖、验收条件与发布门禁。公开的 MCode product/workspace release、所有第一方 Manager/Pack package/release，以及 MCode-owned Manager、FeaturePack、ProviderPack 的 package/world/interface 均为 sole-current `0.0.1`，不保留历史 artifact、alias、dual-read、fallback 或 coexistence。
>
> 内部 schema format tag（包括 `formatVersion=1` 与 `AdapterContractV1.version=1`）及外部 dependency/WASI version 不是 MCode release version，保持不变。

## 0. 当前执行检查点（2026-08-31）

- 唯一交付树是 `D:/my_private_pro/MCode`，唯一交付分支是 `main`；不存在待恢复的独立交付树或临时候选。
- T7 已完成。T8 已落地 runtime/preflight、Manager loading/lifecycle/generation/current dispatch，以及 generation-bound bounded Pack candidate loading；T8 仍未整体完成。
- [x] **TODO(T8.1)**：production current-generation dispatch 已在一次权威选择中绑定 Director identity、family、record、generation 与 admission；公开 API 只接受 opaque expected tag，不暴露 test-only entry 或 Store ownership。
- [x] **TODO(T8.2)**：FeaturePack/ProviderPack 使用 canonical inventory digest 与 exact current Manager generation 完成 bounded single-candidate loading；Host 不扫描 Pack，不暴露 Store/instance/raw component。
- [ ] **TODO(T8.3)**：把 `start-task/poll-task/cancel-task` 接到真实 current Pack execution，完成 generation-bound cancel、RAII waiting、quiescence、stale rejection 与 shutdown/drop 端到端门禁。
- [ ] **TODO(T8.4)**：完成 T8 integration audit 与相关门禁，确认后解锁 T9/T10。
- [ ] **TODO(T9)**：交付 Session Manager/Pack 与 Host durable service。
- [ ] **TODO(T10)**：交付 Manager/Pack 签名安装、更新、回滚和 crash-safe WAL。
- [ ] **TODO(T11)**：交付 Providers Manager、Pi Pack 与 Synthetic Provider Pack。
- [ ] **TODO(T12)**：交付 TUI、UI Manager/Pack 与 generic login flow。
- [ ] **TODO(T13)**：交付 Workspace Manager/Pack、checkpoint 与 rollback。
- [ ] **TODO(T14)**：交付 Resources Manager/Pack。
- [ ] **TODO(T15)**：交付 Ask Manager/Pack。
- [ ] **TODO(T16)**：交付 Todo Manager/Pack。
- [ ] **TODO(T17)**：交付 Web Manager、Querit Pack 与 Synthetic Web Pack。
- [ ] **TODO(T18)**：交付 MCP Manager/Pack。
- [ ] **TODO(T19)**：交付 Usage Manager、Host accounting 与 provider-specific Usage Packs/widgets。
- [ ] **TODO(T20)**：交付 Subagents Manager/Pack。
- [ ] **TODO(T21)**：交付 singleton Compaction Manager/Pack。
- [ ] **TODO(T22)**：交付产品 export/import。
- [ ] **TODO(T23)**：交付 Core 自动更新。
- [ ] **TODO(T24)**：交付最终产品组合与 headless CLI。
- [ ] **TODO(T25)**：删除旧路径的全部可执行识别、读取与兼容代码。
- [ ] **TODO(T26)**：完成最终文档。
- [ ] **TODO(T27)**：完成 Windows/Linux/macOS 安全、发布和 e2e 门禁。
- [ ] **TODO(final)**：完成全项目 audit/cleanup、插件指南、release review，并在所有门禁通过后发布 `v0.0.1`。
- `minimax.txt` 永远不得读取、打印、复制、修改或 stage。

## 1. 路线图与依赖

- T0 Structured Exec（Windows-first）
- T1 Unified Shell
- T2 动态小型 system prompt
- T3 搜索/文件工具 Phase 1
- T4 Windows-first 工具契约
- T5 最小 Core + 删除旧 pipelines
- T6 `.mcode`、配置与凭据布局
- T7 sole-current `0.0.1` Manager/FeaturePack/ProviderPack ABIs + 11-family DTO/goldens + all-world no-WASI；FeaturePack 详见 [docs/design/07-pack-abi.md](docs/design/07-pack-abi.md)，ProviderPack 详见 [docs/design/08-provider-pack-abi.md](docs/design/08-provider-pack-abi.md)
- T8 MCode 插件生命周期
- T9 Session Manager/Pack + Host durable service
- T10 Manager + Feature/Provider Pack 安装更新链
- T11 Providers Manager + Pi/Synthetic Provider Packs
- T12 TUI + UI Manager/Pack
- T13 Workspace Manager/Pack + checkpoint/rollback
- T14 Resources Manager/Pack
- T15 Ask Manager/Pack
- T16 Todo Manager/Pack
- T17 Web Manager + Querit/Synthetic Web Packs
- T18 MCP Manager/Pack
- T19 Usage Manager + provider-specific Usage Packs/widgets
- T20 Subagents Manager/Pack
- T21 Compaction Manager/Pack（singleton）
- T22 产品 export/import
- T23 Core 自动更新
- T24 产品组合与 CLI
- T25 删除旧路径可执行代码
- T26 最终文档
- T27 三平台安全/发布门禁

```text
T4 -> T5 -> T6 -> T7 -> T8
T7 + T8 -> T9
T6 + T7 + T8 -> T10
T7 + T8 + T10 -> T11
T9 + T11 -> T12
T7 + T8 + T9 + T10 -> T13
T7 + T8 + T9 + T10 -> T14/T17/T18/T19
T7 + T10 + T12 -> T15
T7 + T9 + T10 + T12 -> T16
T7 + T8 + T9 + T10 + T11 + T13 -> T20
T7 + T8 + T9 + T10 + T11 -> T21
T6 + T9 + T10 + T11 + T12 + T13 + T14..T21 -> T22 -> T23
T11..T23 -> T24 -> T25 -> T26 -> T27
```

T9 与 T10 可在 T7+T8 后独立推进；T13 不阻塞优先交付 T12。T6/T7 及其余真实依赖不得跳过。

## 2. 不可变产品架构

### 2.1 Core、Manager 与 Pack

- 模型可见 Core 工具仅为 `read/write/edit/find/grep/exec/shell`，无 public `bash` alias。Core 只保留最小 Agent loop、canonical tools，以及插件化所需的 ABI/runtime、受限 Host services、签名加载和 OS 安全原语；产品 feature policy 全在插件。
- Structured Exec 保持环境 allowlist、identity/digest、cancel/timeout、process-tree 回收、bounded output 和 typed/redacted error；child 不继承 ambient/provider/plugin/MCP secret 或 loader/interpreter variables。PATH `pwsh.exe` 仅视为 same-account host input，且只有 typed `NotFound` 可进入 authenticated managed-cache fallback。
- Core 不得恢复 PermissionEngine、Core Ask/grant、`--yolo`、name-based privilege、provider-specific policy、旧 llm/profile/wire/HTTP/SSE/compaction/MCP/session pipeline、FakeProvider 产品入口或 unavailable fake fallback。工具流固定 `Progress* -> exactly one Terminal`，consumer drop/cancel 必须监督回收 producer/child。
- 顶层 Manager **只有且永远只有 12 个**：`providers`、`session`、`compaction`、`resources`、`ask`、`todo`、`web`、`mcp`、`usage`、`subagents`、`workspace`、`ui`。严格 enum 拒绝 unknown/第三方顶层 ID、Manager、family、Host service 与 `com.mcode.*` identity。
- `plugins.json` 只注册这 12 个 `plugins/<feature>/manager/`；Host 不扫描或直接加载 Pack。每个 Manager 是其 `plugins/<feature>/packs/<pack-id>/` 的唯一 discovery、验证、选择、加载、配置、状态与 UI 请求方，并且只能调用对应 typed Host Pack Service。
- 第三方只能实现已发布的 MCode Pack API；新 family 必须由 MCode 新版本先定义 Manager、typed service、ABI/golden 和保留 ID。Manager/Pack 均无 WASI、任意 filesystem/network/process/terminal/socket/credential/raw Host handle。
- 第一方 Manager/Pack 的唯一源码、构建、发布仓库是 `https://github.com/MCapricorns/MCode_plugins`，固定路径 `plugins/<feature>/{manager/,packs/<pack-id>/}`。MCode 主仓仅保留 Core/Host ABI、typed services、安全安装更新 substrate 与集成契约；第一方与第三方走完全相同的签名、source trust、安装、更新、generation、provenance、限额和故障隔离路径，无私有捷径、legacy namespace 或 hidden fallback。
- active 基数：Providers 为 `N` 个；Usage 为 canonical source key 不冲突的 `N` 个；UI 为恰好一个 product runtime Pack 加 `N` 个 declarative Theme Pack；其余 family 均为 Host-wide `0..1` singleton。Pack identity 同时最多一个 generation；ID/source/route/auth slot/source key 冲突、role 不符或旧 generation 未排空均 fail closed，不按名称、安装/加载顺序或隐式 priority 决胜。
- 缺失 Manager/Pack 时只显示准确安装指引，不得回落到 Core。内置 canonical tools 仍是 Agent 必需能力；额外 tool/command/UI 只能经受限 ABI 提供。

### 2.2 用户目录与权威文件

```text
~/.mcode/
├─ config.json
├─ plugins.json
└─ plugins/
   ├─ .host/auth.json
   ├─ .staging.lock
   ├─ .staging/
   │  └─ tx1-<32 lowercase hex>/
   │     ├─ transaction.lock
   │     ├─ journal.json
   │     └─ payload/
   └─ <feature>/
      ├─ manager/{config.json,installation.json,data/,versions/<canonical-semver>/component.wasm}
      └─ packs/<pack-id>/{installation.json,data/,versions/<pack-version>/}
```

- eager **仅**创建 `~/.mcode/` 与 `~/.mcode/plugins/`；`.host/`、`auth.json`、`.staging.lock`、`.staging/`、所有 feature/manager/packs/data/versions 均由可信操作 lazy 创建。不存在 Host 全局 `sessions/` 或 `ensure_sessions_dir`；Session bytes 只能写入所选 `plugins/session/packs/<pack-id>/data/`。
- 根 `config.json` 仅保存 Host composition：默认 provider/model、Providers/Usage 有序 active sets、一个 UI runtime、Theme set 及其余 singleton。未知 family/role、重复 Pack ID、隐式 default 和 singleton 多选由 root parser 拒绝；激活在解析 signed contracts 后另行拒绝 active Usage Packs 的 canonical source-key collision。Usage 顺序只决定 widget row/card composition，不参与 source/Pack binding。
- `plugins.json` 是 12 个 Manager 的 enablement/source/active version+hash/trust high-water 唯一权威；Pack 永不进入其中。Manager 是 `plugins/<family>/manager/versions/<canonical-semver>/component.wasm` 单文件 artifact，`active.digest` 是该 `component.wasm` 精确 bytes 的 SHA-256。Manager `installation.json` 只是 Host 生成的 receipt；Pack `installation.json` 是其 source、selected version+hash、trust high-water、inventory 的唯一权威。Manager `config.json` 只含 bounded 非敏感偏好。
- `.host` 是保留 Host namespace，不是第 13 个 family；`.staging` 是 Host-only、no-follow、owned、同卷的未信任 payload substrate，永不 discovery/export，也不得保存 credential。`TransactionId` 只能由 Host 的 OS CSPRNG 生成 128 bits，并精确编码为 `tx1-[0-9a-f]{32}`；公开 API 不接受任意字符串 transaction ID，恢复所需 parser 保持 crate-private。
- `journal.json` 上限 `1 KiB`，canonical writer 只输出紧凑 UTF-8 JSON 加一个 LF，例如 `{"formatVersion":1,"kind":"mcode-staging-transaction","transactionId":"tx1-0123456789abcdef0123456789abcdef","state":"writing"}`。v1 恰好四个字段，`state` 精确为 `writing|staged|committing|committed` 之一，并拒绝 duplicate/unknown/missing、ID 与目录名不一致、非 UTF-8、trailing content、错误类型和未知 state。T6 只写 `writing` 并在全部 payload durable 后原子改为 `staged`；`committing|committed` 仅由 T10 写，T6 只识别并保留。journal 不含 target、action、digest、signature、trust、rollback 或 redo/undo 数据，不是 WAL。
- 固定 staging 上限为：`.staging/` 最多 `1024` 个直属条目；writing payload 允许 `0..4096` 个、staged payload 要求 `1..4096` 个 regular file，目录最多 `4096` 个，file+directory 合计最多 `8192` 个条目；单文件最多 `256 MiB`，逻辑总字节最多 `512 MiB`，均以 checked `u64` 累加；relative path 最多 `512` bytes/`128` components，每个 component 最多 `128` bytes，并复用 `BundlePath` lowercase portable grammar。payload 只允许 owner-private、同卷、link-count-one regular file 与目录；link/reparse/hardlink alias/mount/cross-device/special file 一律拒绝。
- 锁序固定为 blocking exclusive `plugins/.staging.lock` → nonblocking exclusive `transaction.lock`；创建方依次 durable 创建 `.staging/`、transaction、lock 与 payload，按同一 journal publication 流程发布 `writing` 后才释放 global lock，并让 `StagedTransaction` guard 全生命周期持有 transaction lock。payload 文件使用 handle-relative exclusive create/no-follow，逐文件 flush 后自底向上 flush 目录。每次 journal publication 都必须写 canonical private same-directory temp、flush temp、atomic replace、验证 published identity/access，再 flush transaction directory；crash 残留 temp 是未知 entry，恢复必须保留。只有 payload 与 `staged` journal 的全部 post-replace barrier 成功后才能返回 `StagedTransaction`。仅持有 transaction ID 不授予存活 lease；guard drop/crash 表示 abandonment，除非 T10 已在同一锁下完成 durable claim。持有 transaction、WAL 或 authority lock 时禁止反向获取 global lock；T10 后续锁序为 transaction → coordinator/WAL → 按 canonical path bytes 排序的 authority locks。
- 恢复在 global lock 下使用原生 handle-relative bounded enumeration；`.staging` 不存在时不创建 `.staging` 或 `.staging.lock`，存在 `.staging` 却缺失或无法验证既存 `.staging.lock` 时整次恢复零修改。每个 canonical transaction 先验证 owner/ACL 或 mode、same-volume、既存 lock、strict journal 与整棵树的类型/identity/size/count/path bounds，再尝试删除；transaction lock busy 时跳过。只有 inactive、精确 v1 `writing|staged`、根目录恰好为 `transaction.lock,journal.json,payload/` 且完整预检通过的事务，才可原生自底向上删除并对每个修改过的父目录执行 durability barrier。扫描超过 `1024` 个直属条目时整次恢复零删除；missing/malformed/future、`committing|committed`、未知额外 entry（包括 crash 残留 temp 或 T10 `commit/`）、special/cross-device/over-limit/preflight identity race 或 I/O failure 均不 quarantine、不修复、不降级，且预检失败前不修改 names/content/permissions。删除开始后的 native delete 或 barrier failure 必须返回 indeterminate failure、停止整次恢复、保留任何仍存在的 residue，且不得伪报 clean recovery；最终 transaction name 已删除后的 parent barrier failure 可以没有可见 residue。Windows 使用 rooted native enumeration/deletion，Unix 使用逐 handle no-follow 与 `st_dev`；禁止 `read_dir`/`remove_dir_all` 或 path-based recursion。
- `staged` 只证明当前写入的未信任 bytes 已 durable、私有、同卷且结构有界，不证明 signed inventory completeness、digest、signature、source、trust 或 activation。T10 独占 `commit/wal.json`、durable claim、验签、trust/high-water、安装、激活、回滚以及 `committing|committed` 恢复；T10 在 durable WAL 后、state 更新前留下的 `commit/` 也必须阻止 T6 删除。
- Pack ID 使用 portable lowercase ASCII grammar，拒绝 traversal、分隔符、DOS 保留名、大小写 alias、空/尾点/尾空格和 namespace collision；manifest 必须绑定已知 family、对应 ABI、publisher/source。
- 不创建或读取顶层 `auth.json`、`credentials.json`、`models.json`、`settings.json` 或 `--profile` Provider 定义。项目 `.mcode` 仅在 trusted 后作为 bounded config layer，不能 discovery/install 插件或覆盖 enablement/source/trust、Pack selection/routing、endpoint/auth destination、credential。冻结旧路径不迁移、不兼容读取、不回退；只删除代码库中的可执行识别、读取与兼容路径。磁盘上既存的旧 artifact 位于产品边界之外，永不读取、迁移或删除；禁止递归清理旧根，且不触碰 legacy secret、未知用户数据或当前插件状态。

## 3. 凭据与网络安全（最新决策）

- 唯一 credential authority 是按首次登录 lazy 创建、仅 Rust Host 可访问的 `~/.mcode/plugins/.host/auth.json`。它以严格 envelope 保存 `formatVersion`、`kind=mcode-host-auth`、document revision、credentials 与 grants；每个 canonical service/account secret 只存一次，并有独立 CAS `credentialVersion`。Manager/Pack 不拥有 credential。
- 每个签名 Provider/Web/Usage Pack 必须声明 canonical credential account key，以及精确 `operation + method + origin + path + auth slot`、canonical service/account/issuer/auth schema、trusted signer/source 和 signed credential-contract version。Pack 安装/激活时展示并批准这份精确网络 authority。
- Host 自动为**任何 active Provider/Web/Usage Pack**匹配 vault account；仅当 canonical service/account/issuer/auth schema、trusted signer/source、signed credential-contract version 与批准契约全部精确一致时复用。不限于 Synthetic；成功后不重复输入 key，也不要求逐 Pack login。
- Host 从 account/version、consumer family、Manager/Pack ID/version/hash/generation、canonical provider/source、signer/source、credential-contract version、operation、exact method/origin/path、auth adapter/reserved-header destination 推导单次且 generation-bound 的 injection lease。Pack 不能查看 secret、grant、其他 Pack/account，不能借用其他 operation，也不能设置 reserved auth headers。
- 新的或不匹配的 signer、credential-contract version、origin、auth scheme、destination 一律 fail closed，并要求显式 rebind/approval；只要 canonical account 未变，不必重新输入 secret。rotation 原子替换一份 secret；revoke/rebind 仅影响目标 consumer。仅签名契约允许且 destination 不变的 metadata 更新可保留绑定。
- 登录由 generic hidden-secret interaction 完成：T12 为 TUI，T24 为等价 typed stdin/anonymous pipe；secret 不进 argv、environment、terminal echo、guest DTO。一次事务可保存 secret 并批准多个已安装 consumer；新增 authority 只需 rebind。
- vault 使用跨进程 exclusive lock、credential/grant-aware CAS merge、atomic replace、file/parent durability barrier、no-follow 与 current-owner validation；Windows DACL 仅当前 SID+SYSTEM，Unix 目录 `0700`、文件 `0600`。内存 secret 使用 zeroizing/redacted 类型；日志、错误、Session、event、provenance、WASM DTO、测试诊断均不得含原值。
- Provider/Web/Usage Pack 只能构造 bounded 非敏感请求并解析 bounded 响应；Host 独占 HTTP/TLS/DNS/proxy、same-origin redirect、timeout/retry/cancel/backpressure、response bounds、credential lookup/refresh/insertion、privileged auth adapters、reserved-header policy、redaction、allowlist、generation 与审计。
- Pack 不得设置 `Authorization`、`Proxy-Authorization`、`Cookie`、`x-api-key`、`api-key`、`cf-aig-authorization`、`Host`、`Content-Length`，不得运行时扩展签名外 endpoint/auth destination，不得泄漏 payload/secret 到数据或诊断面。

## 4. ABI、生命周期与跨 Pack 数据流

- Manager task wire 保留 `start-task/poll-task/cancel-task` JSON 网关，但删除 direct `Web/Mcp/AgentRun` task/capability，只增加唯一 `FeatureService` kind；其公开 `abiVersion` 是精确 string `"0.0.1"`。Host 由 caller Manager identity 绑定 strict family，再进入 family-specific typed decoder。`operationId` 是与 vault operation authority 共用唯一无分配 validator 的 `1..=128` bytes declarative canonical key；Host 在 body decode、Pack 与 transport 前，以最多 128 项的已绑定声明集 fail closed，`taskId` 仍是 Host-issued task instance identity。`start-task` 的 pre-allocation rejection 只含 `abiVersion/kind/state/error`；带 `operationId/taskId/generation` 的 assigned error 只属于 task 的 poll/cancel 生命周期。未知、跨 family、未声明或 oversized 请求 fail closed；不是万能 JSON 通道。
- T7 已冻结三个 ABI packages、共 13 个独立 no-WASI current worlds/goldens：sole-current Manager `mcode:plugin@0.0.1` 的一个 world、共享同一 package version 的 FeaturePack `mcode:feature-pack@0.0.1` 十一个 worlds、ProviderPack `mcode:provider-pack@0.0.1` 的一个 world，且各自的 MCode-owned world/interface 同为 `0.0.1`；并完成 11 个 family-specific tagged DTO/goldens、closed `AdapterContractV1`、static trusted dummy context counter、pure decoder reducers、13 × 13 交叉拒绝与 scanner-first binary preflight。三套 ABI 均只保留 current surface，不保留 `abi_v1.json`、historical golden、compatibility parser/adapter、ABI alias、dual-read 或 fallback，也不提供 generic JSON、secret、socket、URL authority、reserved header 或 raw handle。FeaturePack authority 见 [docs/design/07-pack-abi.md](docs/design/07-pack-abi.md)，ProviderPack authority 见 [docs/design/08-provider-pack-abi.md](docs/design/08-provider-pack-abi.md)。Host 通过 typed `initialization-context` 向 Manager 提供 active generation；Manager task envelope 不含 caller-supplied family；不建共享且持续膨胀的 `PackOperation` enum。
- Manager lifecycle 的 PREPARING 与 CURRENT admission 严格分离：`initialize` 和显式一步式 `poll_preparation` 不能进入 FeatureService；原子发布后，上层 coordinator 必须只用 Director 产出的 opaque current-generation view 作为 expected tag 显式驱动一次 `poll_current`，不得 timer/busy poll。Director 在同一锁内重选 exact family/record/generation、校验 tag并取得 activity，随后才等待 owner；stale wake 在 guest 前拒绝且不能改投 replacement。revision 只作 selection observation，不参与 generation identity；revision-only advance 后 exact live generation 的旧 tag 仍可用，返回值与 post-selection error以实际 selection revision 盖章。CURRENT `Ready|Pending` 保持 current；`Stopping|Stopped`、guest rejection、trap、fuel/deadline 或 Store failure只 compare-remove exact generation，再 cancel/drain、作一次 bounded shutdown并 dispose。每次 lifecycle call 使用独立 bounded fuel lease；authority revision/target/high-water 不因 runtime terminal 回退，重试必须由显式 reconcile 消耗 fresh generation。
- ProviderPack Service **不提供** signed endpoint、auth slot 或 adapter ID；它只暴露 bounded descriptor/catalog/auth-presentation comparison DTO、prepared typed request、decoder frames 与 normalized events。signed endpoint、auth slot、adapter、transport、credential 及 provider ID/route/auth slot uniqueness 由 Host pre-bound context 独占。`toolChoice` 明确为 `Unset|Auto|None|Specific`，每个 wire 单独冻结 omitted 语义。
- trap/timeout/cancel/stale generation 均 fail closed。Manager reload 取消 pending UI/service operation；Host 回收 Pack task/stream/interaction/singleton lease。阻塞 interaction 使用 generation-bound RAII lease，最外层 waiting-start/end 严格配对且异常、取消、reload、drop 时 exactly once 结束。
- Provider 与 Usage 实现保持独立。Usage **只能**从 Host-stamped immutable `ModelRouteLease`、`UsageContextSnapshot`、`UsageSample` 得知 current/requested/resolved model；绝不查询 Provider，也不从字符串、Session、widget 或 quota 猜测。
- Host 在已验证 route/request/terminal 边界盖章 Manager/Pack identity、version/hash/generation、provider、request/turn ID、requested model/alias、endpoint/auth fingerprint，以及可选 resolved model/token/cache counters。缺失保持 `None`；route generation 更新后拒绝旧事件、重复 terminal 和未绑定事件。
- Usage 支持 `N` 个 source Pack。每个 Pack 只处理自己的 canonical source，返回 bounded normalized row/card；Usage Manager 按根配置顺序组合固定 `status.trailing/usage.summary` 与 `panel/usage.details` semantic slots。Pack 不能抢 slot/custom draw；UI runtime 负责布局，Theme 只提供 tokens。

## 5. 特定 Pack 冻结契约

### 5.1 Pi Provider Pack

- `plugins/providers/packs/pi` 冻结 `@earendil-works/pi-coding-agent`/`pi-ai` `0.84.4`、schema `3`、generated-at `2026-08-28T22:00:02.569Z`、structure hash `456b83c08bed3255d7e399d7927c6743e7f3568435691b3d38cc3666ffa70479`、model-set hash `72385b2a5d80d906b6fef6da6d823c63d9e68874952ed0a9d794595abac7c719`。
- 基线为 40 个 text Provider（39 static + Radius dynamic）、1,290 个 static chat/text-output models、10 wires：`anthropic-messages`、`openai-completions`、`openai-responses`、`openai-codex-responses`、`azure-openai-responses`、`google-generative-ai`、`google-vertex`、`mistral-conversations`、`bedrock-converse-stream`、`pi-messages`。OpenRouter Images 与 50 image models 不属于首版 text-output；`deepseek-v4-flash-vision-exp` 也不是 Web 搜索。
- Provider IDs：`amazon-bedrock, ant-ling, anthropic, azure-openai-responses, baseten, cerebras, cloudflare-ai-gateway, cloudflare-workers-ai, deepseek, fireworks, github-copilot, google, google-vertex, groq, huggingface, kimi-coding, minimax, minimax-cn, mistral, moonshotai, moonshotai-cn, nvidia, openai, openai-codex, opencode, opencode-go, openrouter, qwen-token-plan, qwen-token-plan-cn, qwen-token-plan-individual, radius, together, vercel-ai-gateway, xai, xiaomi, xiaomi-token-plan-ams, xiaomi-token-plan-cn, xiaomi-token-plan-sgp, zai, zai-coding-cn`。
- 无 provider-list API；只可从签名 snapshot 枚举，并从 `https://pi.dev/api/models/providers/<id>` 接收 snapshot 允许的 bounded metadata。远端不能新增 provider/endpoint/auth/wire/header；Host 按 `(lastModified, canonical-content-digest)` 处理 greater、equal+same、lower 和 equal+different 四种关系，cache 固定 `plugins/providers/packs/pi/data/models-store.json`，使用 owned no-follow path、跨进程锁和 durable atomic replace；离线无有效 cache 明确失败。
- Pi codec/header/body/parser 归 Pack；升级以可重复 importer/auditor、manifest、machine-readable diff、`docs/alignment.md` 生成 count/hash/golden，校验 exports、license/provenance、schema、endpoint/auth、wire、tool/reasoning/terminal 语义。未知或无法解释的变化 fail closed；只有通用 ABI/Host transport/auth 改变才升级 Core。
- regression 必须覆盖 `toolChoice=Unset`、reasoning text/summary 连续段与独立 encrypted signature、Mistral fragmented tool call 的 bounded index 关联；冲突/歧义 fail closed。

### 5.2 Synthetic

- Provider Pack：`POST https://api.synthetic.new/v1/chat/completions`，Bearer；不探测/fallback `/openai/v1`。仅 `syn:large:text`、`syn:small:text`、`syn:large:vision`、`syn:small:vision`；alias 可漂移，provenance 同存 requested alias 与 returned model，不固化 target/cost/context/capability。vision 只接 Host 验证且总量有界的 JPEG/PNG/GIF/WebP/TIFF bytes，不接 path/URL。
- Web Pack：`POST https://api.synthetic.new/v2/search`，仅 bounded `query`，严格解析 `results[{url,title,text,published}]`。Web family singleton；与 Querit 互斥且不拼接能力；选 Synthetic 时 `fetch_content` 明确不可用。“zero-data-retention”仅作为有来源的上游声明。
- Usage Pack：`GET https://api.synthetic.new/v2/quotas`，严格解析 `subscription{limit,requests,renewsAt}`，source key `provider:synthetic`；quota 不能覆盖 Host 当前模型。“不计 subscription limit”仅作为上游声明。
- 三个实现完全独立，但都声明同一个 canonical `synthetic/<account-id>` account；Host 按第 3 节规则自动复用一份 key，并分别批准精确 authority。默认测试用 dummy transport/credential；无 key 不伪造 live PASS。

### 5.3 Querit Web Pack

- `plugins/web/packs/querit` 冻结 `https://api.querit.ai`、Bearer、`POST /v1/search` 与 `POST /v1/contents`；不得实现 DeepSeek-backed search。
- search：query `1..1000` UTF-8 bytes、count `1..20`。fetch：去重后 `1..10` 个无 embedded credential 的 HTTP(S) URL（各 `<=4096` bytes）、format `markdown|text|html`、每页 timeout `1..60s`、metadata flag。
- defaults 仅允许 bounded count、`d7|w2|m3|y1`、content/chunks、country/language、最多 20 个 normalized domains。search/content/error response 上限分别 `2 MiB/10 MiB/8 KiB`，总 deadline `<=70s`，model-visible 输出 `<=50 KiB/2000 lines`，保留 typed source/search-ID/truncation provenance。
- 远端内容始终标记 untrusted，并移除 terminal control/bidi；不得把截断全文写入普通 OS temp、从 environment 取 key、宽松解析配置或透传远端错误原文。

### 5.4 Minimax CN live smoke

- T11 后才允许显式 opt-in、默认 skip 且不进 CI 的 smoke；必须走 `providers Manager -> Host ProviderPack Service -> signed Pi generation -> minimax-cn -> anthropic-messages`，冻结 `https://api.minimaxi.com/anthropic/v1/messages` 与 `X-Api-Key`。若与官方 Bearer 文档冲突，只报告 mismatch，不改 header/fallback。
- 本地 `minimax.txt` **永远不得由 Agent 读取、打印、复制或暂存/stage**。仅在签名 identity/auth binding 已建立后，由用户通过 anonymous pipe/stdin 将 secret 交给 redacted Host harness，在 disposable secure Home 中走正式 auth CAS；路径与 secret 都不是产品 API。
- 使用 `MiniMax-M2.7`、`cacheRetention=None`、`maxRetries=0`、短 deadline/小 token budget，最多两次请求：bounded reasoning/text stream、forced tool call（仅验证）。只观察 method/URL、header presence、payload shape/count、按 `contentIndex` 关联的 events、provenance、exactly-one terminal、无泄漏与清理；不打印 body/response。
- 负控：未 opt-in 零网络、签名失败零注册/注入、无 credential 时 fetch count=0、缺 Host route token 的 direct adapter 调用拒绝。

## 6. 分阶段验收

### T6–T10 基础

- **T6**：实现第 2.2/3 节 strict schema/path/vault、empty vault、CAS、ACL/mode、durability、redaction、typed transaction ID，以及 Windows/Unix 原生 lazy staging/abandoned-transaction recovery。T6 只产生 `writing|staged`，只证明未信任 payload 的 durable/private/same-volume/bounded mechanical state；旧配置与 secret 均不迁移、不读取、不删除。签名、trust/high-water、WAL、安装、激活、回滚、`committing|committed` 及其恢复全部属于 T10。T11 前无签名 Pack identity，生产路径不得生成可注入 credential/grant。
- **T7（已完成）**：sole-current Manager、FeaturePack 与 ProviderPack 三个 ABI packages、13 个 current worlds/goldens、11 个 family-specific DTO/goldens、all-world no-WASI/交叉拒绝、closed adapter/context-counter/decoder validation 及 binary static preflight 已落地；继续保留 Provider route ownership 与 Host-only `ModelRouteLease/UsageSample` substrate。FeaturePack authority 见 [docs/design/07-pack-abi.md](docs/design/07-pack-abi.md)，ProviderPack authority 见 [docs/design/08-provider-pack-abi.md](docs/design/08-provider-pack-abi.md)。
- **T8（进行中）**：fixed-12 Manager loading、typed lifecycle、authority generation director、原子 publication gate、cancel/drain、cleanup worker 与 bounded Pack candidate loading 已落地；current Pack execution 及端到端门禁仍按第 0 节 TODO 完成。
- **T9**：交付 `session` Manager、SessionPack Service、`packs/mcode`。Pack 拥有 event-sourced branch/resume/rewind、ledger、replay/recovery；Host 只提供 identity-isolated durable storage/WAL、bounds、backpressure 与 fence。tool results 必须先进入 Host state 和 durable transaction，再追加 custom/plugin message；不可插入 call/result 之间。失败无 Core memory/JSONL fallback。
- **T10**：Manager 与 Pack 使用独立 namespace/pointer，共用 signed bundle、source trust、高水位与 crash-safe WAL；multi-active 分项提交、singleton 原子切换。更新不得读 vault；credential contract diff 只触发对应 rebind。用户机器不执行 bundle 内 `build.rs`、npm scripts、Git hooks、submodule 或 LFS。

### T11–T13 核心产品

- **T11**：在 `MCode_plugins` 依次交付 Providers Manager、Pi Pack、Synthetic Pack；每 Pack 独立 Reviewer/commit。Pi 提供 importer/alignment、10-wire bounded codecs、40-provider endpoint/auth/header/body/model/stream/error goldens；默认只用 dummy secret。Provider 可用性 = Manager active + signed Pack active + valid snapshot/cache + Host adapter supported + credential binding matched。此阶段才接通 vault；credential 只能通过当前 Broker flow 新建或更新，绝不读取、迁移或删除旧 secret source。
- **T12**：TUI Host 独占 terminal safety、focus/input、paste/IME、sanitization；产品 UI 来自 `ui` Manager + `packs/mcode`。generic login modal deep-link 同一 Broker。terminal capability 为 image/true-color/hyperlinks 各 `Auto|ForceOn|ForceOff`；显式 root 设置优先。clipboard 仅在 active selection 后经 Host capability。所有 write 分块 `<=1 MiB` 且保持 UTF-8 boundary；远端文本清 control/bidi；诊断不记录原文。widget 遵循第 4 节固定 slots。
- **T13**：Workspace Manager/Pack 经 bounded Host service 覆盖 tracked/untracked/ignored、删除、metadata、hash、限额、并发冲突和 no-follow handles；不可证明范围的 exec/shell 标记不可回滚，rollback 不覆盖并发修改。

### T14–T21 功能 Packs

- **T14 Resources**：bounded resource/prompt/status/UI contribution。
- **T15 Ask**：generic Host interaction；不恢复 Core authorization/grant。
- **T16 Todo**：stable task ID、依赖、状态机、Todo-local durable task event。
- **T17 Web**：先 Querit 后 Synthetic，各自 Reviewer/commit；严格执行第 5 节，Web singleton 无 cross-Pack fallback。
- **T18 MCP**：progressive disclosure、stable server/tool identity、统一 hook boundary。
- **T19 Usage**：先 Host accounting `packs/mcode`，后 Synthetic quota Pack，各自 Reviewer/commit；`N` source、immutable events、固定 widgets，严格遵循第 4 节。
- **T20 Subagents**：roles、parallel queue、worktree isolation、retained session、review/fix loop、crash recovery。
- **T21 Compaction**：Manager + `packs/adaptive`；策略可调用 Host Provider service，Host-wide 单 generation，先 cancel/drain 再原子切换。每次 tool result durable 后、下一次 Provider 请求前重新估算并必要时压缩；恢复明确 progress。summary 的 stop reason 为 `length/error/cancel` 或含 tool call 时失败，partial text 不成 checkpoint；使用 `toolChoice=Unset`。

### T22–T27 收口

- **T22**：export/import 包含 composition、12 Manager、全部 Packs；vault 只能经 Broker typed flow 导入导出并重新验证 consumer signer/destination，不展开为 Pack 文件。Session 只经 SessionPack typed flow；缺 Manager/Pack fail closed；排除 cache/log/temp。
- **T23**：Core updater 与 Manager/Pack updater 独立；signed platform artifact、channel trust、高水位、crash-safe switch。
- **T24**：增加基于 Broker/Providers/Session typed services 的 headless account/provider/model/run/resume；secret 仅 stdin/anonymous pipe。不得恢复旧 global flags、`$MCODE_FAKE`、本地 Provider 文件或 fallback。queue drain 必须 generation/CAS-bound、atomic、bounded、stable ID/type/order；并发消息不误删，abort 不隐式继续。
- **T25**：删除代码库中对旧 `.MCode`、顶层 `settings.json`/`models.json`/auth/credentials/auth-state、`plugins.lock*`、global sessions/`ensure_sessions_dir`、sibling Pack roots、profile/provider definitions、Fake/M1/TOML/Tier、legacy namespace/dual-read/ABI compatibility alias/migration/fallback 的可执行识别、读取、兼容代码及仓库内临时候选。无迁移、无兼容读取、无回退；磁盘上既存的旧 artifact 永不读取、迁移或删除，禁止递归删除旧根，且不触碰 legacy secret、未知用户数据或当前插件状态。不得复活旧 llm/compaction/MCP pipeline。
- **T26**：只记录最终验证事实，覆盖 Core、严格 12 Manager、nested Packs、credential、TUI/headless/update 与旧路径零兼容，并证明 unknown/第三方顶层 Manager 始终拒绝。
- **T27**：Windows/Linux/macOS 原生 fmt/check/strict Clippy/full tests；全部 Manager/Pack build/sign/install/update/rollback/e2e；security、offline/crash、redaction、singleton、Reviewer 及 `main == origin/main` 全通过后才发布。

## 7. 安全、审查与交付门禁

- 开发阶段以完整 TODO 为单位运行 targeted tests 与受影响的 fmt/check/Clippy；提交只包含相关路径，不跳依赖、不削弱检查。
- 始终保留 secret redaction、owned/no-follow filesystem、bounded input/output、generation/cancel/quiescence 与 exactly-one terminal 等安全边界。
- T27/final 在发布 `v0.0.1` 前统一完成 Windows/Linux/macOS full gates、全树 security/dead-code/legacy cleanup、e2e、Reviewer 与 `main == origin/main` 检查。
