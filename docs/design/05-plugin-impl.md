# Plugin v2 实施契约

> 状态：**冻结实现契约，尚未落地**。本文约束目标 runtime；它不认可旧 direct runtime 或 Plugin ABI v1 (`mcode:plugin@0.1.0`) 作为兼容路径。

## 1. 选择、安装与装载

Host 是唯一可以 discovery、验证、安装和装载 Manager/Pack 的组件。目标顺序：

```text
plugins.json 的 Manager enabled/source/active hash/trust
  → 验证唯一 Manager identity 与 Manager Plugin world
  → 读取对应 Pack installation.json 的 source/selected hash/trust
  → 验签 payload、验证 world/version/golden compatibility
  → 建立有界 runtime 与新 generation
  → 注入对应 typed Service
  → Manager 只经 Service 调度 active Pack
```

`plugins/<manager-id>/installation.json` 是非权威 installation receipt（first-party `<manager-id>` 为保留的 `com.mcode.<feature>`）；它不能修改 `plugins.json` 的 Manager routing、trust 或 active version+hash。Pack `installation.json` 才是该 Pack source binding、selected version+hash、trust high-water 与安装 inventory 的权威。Manager 和 Pack 都不得自行读取安装目录、选择 publisher、下载、discovery 或 load 彼此。

first-party 与 third-party 使用相同 validation、generation、failure 和 installation flow。任一步失败都销毁未提交 generation、fail closed 并保留安装指引；不回退到直接实现。

## 2. 三个 ABI

### Manager Plugin：`mcode:plugin@0.2.0`

Manager guest 的唯一 FeatureService 入口是 Plugin WIT `start-task` / `poll-task` / `cancel-task` JSON gateway。Host 先从 caller 与路由绑定 capability/family，随后才对有界 body 做该 family 的 typed decode；JSON 因而只是受限 envelope，不能成为 Manager 到 Pack 的通用语义通道。

### FeaturePack：`mcode:feature-pack@0.1.0`

FeaturePack Service 以自己的 typed `invoke` / `pull` boundary 调用 active FeaturePack。它不接受 Manager 的 `start-task` / `poll-task` / `cancel-task`，也不与 Manager Plugin 共用 world、binding 或 Service。

### ProviderPack：`mcode:provider-pack@0.1.0`

ProviderPack 使用独立的 typed provider request/stream/error boundary。Host 独占 auth store、HTTP、TLS、DNS、proxy、reserved headers 和连接控制；ProviderPack 看不到 credential、socket 或 HTTP client。

三个 world 均要求独立 version 规则、binding、golden request/response/error fixtures 与 no-WASI runtime。不得定义跨 world 的 adapter、共享可增长 `PackOperation` enum、通用 JSON、`serde_json::Value`、无界 map、opaque blob 或延迟解释字段。一个 world 的 golden 不能被另一个接受。

Web、MCP、AgentRun/Subagents 的 direct kind/capability 必须删除。它们只能沿 Manager gateway → family typed decode → 对应 Service → Pack 的路径运行。

## 3. 动态贡献与 Agent adapter

Manager+Pack 可以提出 bounded typed tool、command、UI 或 feature contribution。Host 验证 Manager/Pack provenance、feature family、active hash、generation、namespace、schema、能力描述、配额与取消边界后，才创建 Host adapter。

adapter 是动态工具进入 Agent 的唯一入口：

- 七个 canonical builtin 仍保留且不可覆盖；
- 动态工具必须 namespaced，绑定一个活跃 Manager+Pack generation；
- Pack 不直接注册 `ToolDyn`、不修改 Registry、不持有 raw capability；
- 文件/搜索动态工具必须经过同一 no-follow/preflight/prepared-capability 边界；
- `com.mcode.mcp` 可提交 bounded MCP tool contribution，由 Host adapter 接入，而非 direct transport。

Manager gateway 的 JSON 不扩大 contribution DTO；所有 descriptor 都是闭合、有界、按 family 验证的类型。

## 4. Session durable bytes

Session 仅由 `com.mcode.session` / `session_plugins/mcode` 的 `SessionPackService` 实现。只有该 Service 能写入 `~/.mcode/session_plugins/<pack-id>/data/`；Host 在每次操作上绑定并验证 Pack ID/version/hash/generation，而不定义另一套公开的 data 子目录协议。

Host 只提供 no-follow owned storage、bounded WAL、atomic append、durability、backpressure、generation fence 与 DTO 验证。SessionPack 定义 session/event/branch/resume/rewind/rollback 语义；Host/CLI/Core 不解释 durable bytes、不重建全局 tree，也不在 Pack 缺失时恢复。

任何 import/export 都必须是 typed SessionPack transaction，绑定 Pack provenance 与 generation；不得复制 Pack data 目录或把目录内容当作会话协议。

## 5. auth 时序

T6 对 `provider_plugins/auth.json` 只提供 strict 空 store、schema、CAS 与 ACL 机械。T11 之前不得创建/注入 entry 或迁移旧 secret。仅在签名 Pack identity 已验证后，Host 才能执行这些动作；secret 始终留在 Host auth store，不对 Manager/Pack 暴露。

## 6. 必需验证

每个 target runtime/Pack 至少覆盖：

- `plugins.json` 与 Manager/Pack `installation.json` 的不同权威性；project `.mcode` 不能 discovery 或覆盖 trust/routing；
- 三个 world 各自的 version、no-WASI、golden 与交叉拒绝；
- caller capability/family 在 JSON typed decode 前绑定，错误 family/body/generation fail closed；
- FeaturePack `invoke` / `pull` 不与 Manager gateway 混用；没有 `PackOperation` 或 generic JSON escape hatch；
- canonical builtin 不可覆盖，动态 namespaced contribution 只能经 Host adapter，且保留工具安全 preflight；
- SessionPack 数据按 Pack ID隔离，操作绑定version/hash/generation，只有 SessionPack Service能写；typed import/export不复制目录；
- Web/MCP/AgentRun/Subagents 没有 direct kind/capability；
- T6 空 auth 机械和 T11 身份后 entry 生命周期；
- canonical seven tools 的 Structured Exec 与 OS 安全检查不被 Pack 路径弱化。

当前 `main` 尚未提供上述 target worlds、Services、installation flow 或 golden suites；实现顺序见 [04-roadmap.md](04-roadmap.md)。
