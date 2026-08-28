# 冻结架构落地路线图

> 本页记录架构级实施顺序；详细验收以执行计划和实际代码为准。目标描述不把迁移前实现变成 compatibility 承诺。

## 当前状态

T0–T5 已完成。T5 已删除旧 Core compaction pipeline、direct MCP runtime 与仅供它使用的 vendored `rmcp`；CLI 的旧 Provider/Session assembly、product flags、Tokio runtime 与 headless renderer 也已删除，`run`/`resume` 当前只保留 fail-closed clap 骨架。旧 `mcode-session` crate、global JSONL/store/path、legacy Session config scope 及 Core Session product public API/runtime 已删除，不存在 global Session store、JSONL 或 compatibility fallback。中性的 `AgentEvent`/`MessageDelta`/`TurnOutcome` 是现行最小 Agent loop 协议，Core ids 只保留 `CallId`。旧 `mcode-llm` crate 及其 profile、catalog、identity、header、wire、HTTP、SSE、registry、fallback、旧 Provider/stream/error 实现和全部专属 tests/fixtures/live ignored tests 已整体删除，且未迁移或保留 stub、legacy namespace、adapter、compatibility、fake 或 unavailable Provider。独立的 `mcode-provider-api` 仍只提供 provider-neutral Agent↔Host Rust port，`mcode-agent` 已迁到该边界；该 port 不是 `mcode:provider-pack@0.1.0` world、产品 extension surface 或 Provider 实现，也不代表 Host adapter 或 T11 Provider 能力已交付。仓库级审计确认 Core 的 provider wire-only state 是 T5 唯一 blocker；replay、assistant phase、thinking replay 与 tool-call item id 现已连同 rich/object wire shape 完整删除，Text/Thinking 只接受 plain string，Core DTO 在 provider-neutral serde 边界递归 fail closed。Core 的 direct `url` import/dependency 与 workspace direct declaration 也已删除，未保留 alias、deprecation、adapter、compatibility 或 fallback。下一步是 T6；Plugin ABI v1 的替换与拒绝仍属于 T7，不是 fallback。

## T5 — 最小 Core 与旧 pipeline 删除

- 保留 `read`、`write`、`edit`、`find`、`grep`、`exec`、`shell` 及 [02-tools-permissions.md](02-tools-permissions.md) 的安全契约。
- T5 必须删除旧 `mcode-llm`、`mcode-session`、`mcode-mcp`、Core compaction pipeline、仅供旧 MCP 使用的 vendored runtime，以及产品 FakeProvider/本地 profile 入口；不得等待 replacement Pack。当前旧 Core compaction pipeline、`mcode-mcp`、vendored `rmcp`、CLI 的 Provider/Session 产品入口、`mcode-session` crate、legacy Session config scope 与 Core Session product API/runtime 已删除。旧 `mcode-llm` crate 连同 profile/catalog/identity/header/wire/HTTP/SSE/registry/fallback、旧 Provider/stream/error 实现及全部专属 tests/fixtures/live ignored tests 也已删除，没有迁移或保留任何 stub、legacy namespace、adapter、compatibility、fake 或 unavailable Provider。`AgentEvent`/`MessageDelta`/`TurnOutcome` 是保留的 provider-neutral 最小 Agent loop 协议；独立 `mcode-provider-api` 仍仅承接 provider-neutral Agent↔Host Rust port，且 `mcode-agent` 已完成迁移。该 port 不属于 T7 ProviderPack world，也不提供 T11 产品 Provider 能力。仓库级审计确认 Core provider wire-only state 是唯一剩余 blocker；replay、assistant phase、thinking replay、tool-call item id、对应 rich/object wire shape 以及 Core 的 direct `url` dependency/workspace declaration 现已全部删除，provider-neutral serde DTO 递归 fail closed，且未保留 compatibility 路径。T5 已完成，下一步是 T6。
- 旧 `--provider`、`--profile`、`--model`、`--fake` 产品 flags 已从 CLI 删除并作为 unknown argument 拒绝；`MCODE_FAKE` 不再影响产品行为。replacement Manager/Pack 未交付时，`run`/`resume` 必须在解析后立即 fail closed，同时显示 `com.mcode.providers` + signed Provider Pack 与 `com.mcode.session` + signed Session Pack 的安装/激活指引，不读取 cwd、filesystem、network、environment、auth 或 state。
- 历史 `abi_v1` golden 只可作为冻结测试资料；loader/runtime 必须拒绝 v1 artifact，它不是 fallback、adapter 或 compatibility 路径。

## T6 — 目录、安装权威性与空 auth store

- 固定 [03-plugins.md](03-plugins.md) 的 `config.json`、`plugins.json`、Manager/Pack 目录、installation authority、portable ID、no-follow owner validation 和 Host-only staging。
- 项目 `.mcode` 不参与 discovery，也不能覆盖 trust、source、Pack routing、Provider endpoint/auth destination 或 credential。
- `provider_plugins/auth.json` 本阶段只提供 strict 空 store、schema、CAS、ACL/mode、durability 和 redaction 机械；不得创建/注入 credential entry、迁移旧 secret 或删除旧 source。

## T7 — 三个 ABI world

- 分别冻结 Manager Plugin `mcode:plugin@0.2.0`、FeaturePack `mcode:feature-pack@0.1.0`、ProviderPack `mcode:provider-pack@0.1.0` 的 version、binding、golden 和 no-WASI gate。
- Manager guest 只经 `start-task` / `poll-task` / `cancel-task` gateway 的唯一 `FeatureService` operation；Host 先绑定 caller capability/family，再做 family-specific typed decode。
- FeaturePack 的 typed `invoke` / `pull` 边界独立；删除 Web/MCP/AgentRun/Subagents direct kind/capability，不定义通用 JSON escape hatch 或共享可增长 `PackOperation`。

## T8–T10 — 生命周期、Session Pack 与交付链

- T8 实现 Manager discovery、签名 preflight、generation、quiescence 与 fail-closed lifecycle；Pack 不进入顶层 plugin registry。
- T9 发布 `com.mcode.session`、`SessionPackService` 与 `session_plugins/mcode`。Session persistence/resume/branch/rewind/rollback 全由 Pack 定义，durable bytes 只进 Pack identity 隔离的 `data/`。
- T10 实现 Manager 与 Pack 分离的 signed install/update/rollback chain、source-bound trust、高水位和 crash-safe activation。

T9 与 T10 都依赖 T7+T8，可以并行推进；二者都不得使用 first-party 私有安装或装载路径。

## T11 — Providers Manager、Pi Pack 与 credential binding

发布 `com.mcode.providers`、`ProviderPackService` 与 `provider_plugins/pi`。Host 独占 auth store、HTTP/TLS/DNS/proxy、reserved headers 和 credential injection。只有签名 Pack/provider/endpoint identity 已验证后，Host 才可创建或注入 auth entry；旧 secret 必须在新 binding 原子写入并验证成功后才删除。

## T12 — interactive TUI 与 UI Pack

T12只交付interactive TUI、`com.mcode.ui`、`UiPackService`与`ui_plugins/mcode`。UiPack缺失时interactive TUI不可用并给出安装指引；本阶段不交付headless login/logout、provider/model管理或非交互run/resume。

## T13–T21 — 其余产品 Feature

依赖相关 substrate 后依次交付：

- T13 Workspace checkpoint/rollback；
- T14 Resources；T15 Ask；T16 Todo；
- T17 Web；T18 MCP（`com.mcode.mcp` + `McpPackService` + `mcp_plugins/mcode`）；T19 Usage；
- T20 AgentRun/Subagents；
- T21 Host-wide singleton Compaction。

每项都必须包含唯一第一方 Manager、Host-owned typed Service、first-party Pack 和同等 third-party Pack contract。动态 namespaced tool/command/UI contribution 只经 bounded typed descriptor 与 Host adapter；canonical builtin 不可覆盖。

## T22–T27 — 产品收口

- T22 export/import：Session 只能走带 provenance/generation 验证的 typed SessionPack transaction，不复制或解释 Pack data 目录。
- T23 Core updater 与 Manager/Pack updater 分离，并保持 signed artifact、trust high-water 和 crash-safe switch。
- T24 增加基于 Providers/Session Manager-bound typed Service 的 headless login/logout、provider/model 管理及非交互 run/resume；不恢复旧 global flags、本地 Provider 文件或 FakeProvider。
- T25 删除旧路径、旧格式和临时架构产物，不复活 T5 已删除 pipeline。
- T26 记录最终实际行为与验证事实。
- T27 在 Windows、Linux、macOS 原生 runner 完成 fmt、strict Clippy、full tests、Manager/Pack install/update/rollback/e2e、安全与独立 Reviewer 门禁。

## 完成条件

Agent Core 仍只有最小 loop 与七个 builtin；所有产品行为可追溯到唯一 Manager、独立 ABI world、Host Service、Pack source/hash/trust 与 generation。first-party 和 third-party 走同一路径，且不存在旧 pipeline、direct privileged capability、compatibility adapter 或 unavailable fallback。
