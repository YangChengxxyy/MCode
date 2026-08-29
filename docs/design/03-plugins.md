# Manager、Pack 与 Host substrate

> 状态：**冻结目标**。本页的 Manager、Service、world 与目录规则未因文档存在而声称已落地。

## 1. 组成与权限边界

每个产品 Feature 只有一个 Manager 和一个 Host-owned typed Pack Service。Service 按该 family 明确冻结 singleton、single-active 或无冲突 multi-active/routing 基数；不得靠名称、加载顺序或隐式 priority 决胜。所有能力首先是 `plugins/<feature>/manager/` 中的顶层 Manager Plugin，实际工作的 Pack 只物理嵌套在同一容器的 `packs/<pack-id>/`。Core/Host 只 discovery、验证和装载这一顶层 Manager；已装载的 Manager 选择期望的嵌套 Pack，并经 gateway 请求激活。Pack 不是顶层 plugin entry，也永不进入根 `plugins.json`；Core loader 不独立 discovery、选择或装载 Pack。

Manager 定义 feature 编排、Pack 选择策略和激活请求，但 guest Manager 没有 filesystem 或 raw handle，不能读取 installation state/payload，也不能执行验签、trust 或其他安全验证。只有对应 Host-owned typed Pack Service 能在 caller capability 与 family 均已绑定、请求已授权后，打开 installation state/payload，验证 source binding、signature、trust、version、hash、world 与 golden，实例化 Pack runtime、绑定 generation/leases，并返回有界状态。Host 还提供安装事务、资源限制和 OS substrate；Pack 定义 feature 语义。Host 不携带产品策略，first-party 与 third-party 均无私有路径。Compaction 额外固定为 Host-wide singleton。Manager 不得直接访问 filesystem、network、secrets、MCP、Subagents，也不得发现或装载其他顶层 Plugin；其嵌套 Pack 生命周期不授予任何 guest raw capability。所有三类 guest world 均不获得 WASI、filesystem、process、socket、terminal、credential 或 raw handle。Manager/Pack 缺失、签名或 trust 不匹配、版本不兼容、DTO 越界或 generation 过期时必须 fail closed，并返回安装指引。

## 2. 第一方 Manager 与源目录

第一方源层级固定为 `MCode_plugins/plugins/<feature>/manager/` 与同一 feature 下的 `MCode_plugins/plugins/<feature>/packs/<pack-id>/`；运行时使用相同的顶层 Plugin 容器与嵌套 Pack 形状。保留的顶层 Plugin ID/feature 目录**恰好**是 `providers`、`session`、`compaction`、`resources`、`ask`、`todo`、`web`、`mcp`、`usage`、`subagents`、`workspace`、`ui`。

| Feature / top-level Plugin ID | Manager | Manager 源目录 | first-party Pack 源目录 |
| --- | --- | --- | --- |
| Providers / `providers` | `com.mcode.providers` | `MCode_plugins/plugins/providers/manager/` | `MCode_plugins/plugins/providers/packs/pi/` |
| Session / `session` | `com.mcode.session` | `MCode_plugins/plugins/session/manager/` | `MCode_plugins/plugins/session/packs/mcode/` |
| Compaction / `compaction` | `com.mcode.compaction` | `MCode_plugins/plugins/compaction/manager/` | `MCode_plugins/plugins/compaction/packs/adaptive/` |
| Resources / `resources` | `com.mcode.resources` | `MCode_plugins/plugins/resources/manager/` | `MCode_plugins/plugins/resources/packs/mcode/` |
| Ask / `ask` | `com.mcode.ask` | `MCode_plugins/plugins/ask/manager/` | `MCode_plugins/plugins/ask/packs/mcode/` |
| Todo / `todo` | `com.mcode.todo` | `MCode_plugins/plugins/todo/manager/` | `MCode_plugins/plugins/todo/packs/mcode/` |
| Web / `web` | `com.mcode.web` | `MCode_plugins/plugins/web/manager/` | `MCode_plugins/plugins/web/packs/mcode/` |
| MCP / `mcp` | `com.mcode.mcp` | `MCode_plugins/plugins/mcp/manager/` | `MCode_plugins/plugins/mcp/packs/mcode/` |
| Usage / `usage` | `com.mcode.usage` | `MCode_plugins/plugins/usage/manager/` | `MCode_plugins/plugins/usage/packs/mcode/` |
| Subagents / `subagents` | `com.mcode.subagents` | `MCode_plugins/plugins/subagents/manager/` | `MCode_plugins/plugins/subagents/packs/mcode/` |
| Workspace / `workspace` | `com.mcode.workspace` | `MCode_plugins/plugins/workspace/manager/` | `MCode_plugins/plugins/workspace/packs/mcode/` |
| UI / `ui` | `com.mcode.ui` | `MCode_plugins/plugins/ui/manager/` | `MCode_plugins/plugins/ui/packs/mcode/` |

first-party 与 third-party 都使用这一两层路径形状。第三方可以为全新 feature 增加唯一的顶层 Plugin ID 与 Manager，但不得占用上述保留目录名、`com.mcode.*` 或既有 family，也不得绕过相同签名、trust、生命周期和 Service 约束。

不得以第二个 Manager、按名称特权、普通 hook、直接 transport 或 Core fallback 复制这些 product Feature。Session、Workspace、Provider 和 Compaction 边界见 [01-agent-core.md](01-agent-core.md)。

## 3. 三个独立 world

| world | guest | 唯一边界 | 独立要求 |
| --- | --- | --- | --- |
| Manager Plugin `mcode:plugin@0.2.0` | Manager | 仅通过 Plugin WIT 的 `start-task` / `poll-task` / `cancel-task` JSON gateway 发起新的唯一 FeatureService operation | 独立 version、binding、golden、no-WASI |
| FeaturePack `mcode:feature-pack@0.1.0` | 除 Provider 外的产品 Pack | FeaturePack Service 自己的 typed `invoke` / `pull` 边界 | 独立 version、binding、golden、no-WASI |
| ProviderPack `mcode:provider-pack@0.1.0` | Provider Pack | typed provider request/stream/error 边界 | 独立 version、binding、golden、no-WASI |

Manager gateway 的 JSON 只是有界 transport envelope，不是通用 API：Host **先**绑定 caller capability 与 feature family，随后才按该 family typed decode。Manager guest 只能经该 gateway 提交 Pack 选择与激活请求，不能直接调用 FeaturePack Service、ProviderPack Service 或 OS substrate；Host-owned typed Pack Service 只处理通过上述绑定与授权的请求。

FeaturePack 的 `invoke` / `pull` 与 Manager 的 `start-task` / `poll-task` / `cancel-task` 是不同 world、不同 Service 边界，不能混用或互相 adapter。不得定义共享、可增长的 `PackOperation` enum，也不得用通用 JSON、`serde_json::Value`、无界 map、opaque blob 或“以后解释”字段绕过 family DTO。每个 world 的 golden 只能验证本 world，交叉输入必须拒绝。

Web、MCP、AgentRun/Subagents 的 direct kind/capability 已删除；它们仅通过上表的 Manager gateway 和对应 typed Service 运行。

## 4. 动态贡献与 Host adapter

七个 canonical builtin 仍然是唯一 builtin。激活的 Manager+Pack 可以提出 bounded typed tool contribution、command、UI 或 feature contribution。Host 仅在验证 Manager/Pack provenance、family、active hash、generation、namespaced 名称、schema、能力描述和资源预算后，创建 Host adapter。

adapter 是 Agent 看到动态工具的唯一途径：Pack 不直接注册 `ToolDyn`、不修改 Registry、不能覆盖 builtin。文件或搜索动态工具使用与 builtin 相同的 Host preflight 与 prepared capability；MCP 工具也只能由 `com.mcode.mcp` Service 生成这种 namespaced adapter，不得回到 direct transport。贡献 DTO 是闭合、有界类型；Manager gateway JSON 不能扩展其语义。

## 5. 安装目录与权威性

用户目录固定为：

```text
~/.mcode/
├── config.json
├── plugins.json
└── plugins/
    ├── .staging/<transaction-id>/     # Host-only，lazy
    └── <plugin-id>/                   # first-party 为 providers/session/...
        ├── manager/
        │   ├── config.json
        │   ├── installation.json
        │   ├── data/
        │   └── versions/
        ├── packs/
        │   └── <pack-id>/
        │       ├── installation.json
        │       ├── data/
        │       └── versions/
        └── host/                      # Host-private optional state，lazy
            └── auth.json              # providers only
```

`plugins.json` 只含顶层 Manager entry，且对 Manager 的 `enabled`、source binding、active version+hash 与 trust high-water 唯一权威。Pack 虽物理位于 `plugins/` 下，却绝不是顶层 plugin entry，永不进入根 `plugins.json`。`plugins/<plugin-id>/manager/installation.json` 只是 Host 从签名 bundle 生成的非权威 Manager installation inventory/receipt，不能改变 routing、trust 或 active pointer。例如第三方全新 Manager `org.example.diagram` 只能使用 `plugins/org.example.diagram/`，不能占用 `com.mcode.*` 或内置 Plugin ID。每个嵌套 Pack 的 `plugins/<plugin-id>/packs/<pack-id>/installation.json` 对其 source binding、selected version+hash、trust high-water 与安装 inventory 唯一权威。Manager 只提交 family-bound 的 Pack 选择与激活请求；Host-owned typed Pack Service 独占该文件和 payload 的打开与安全验证，并实例化、绑定和激活 runtime。Pack payload、版本和 data 不能由 `plugins.json` 推断或替代。

Session durable bytes 仅由 `SessionPackService` 写入 `plugins/session/packs/<pack-id>/data/`；Host 在操作边界绑定并验证 Pack ID/version/hash/generation，不把这些字段隐式编码成另一套公共目录协议。不存在独立的全局 Session durable 区、Session tree 或任何 lazy bootstrap 路径。`plugins/<plugin-id>/host/` 是 Host-private optional state；`plugins/providers/host/auth.json` 是 Provider auth 的专用位置，访问时序见 [01-agent-core.md](01-agent-core.md)。

初始化只确保 owned root 与 `plugins/`；`config.json`、`plugins.json`、每个 Plugin 容器、Manager、Packs、`host/`、`auth.json`、所有 `data/`、`versions/` 与 Host-only `plugins/.staging/<transaction-id>/` 都只在对应可信事务中 lazy 创建。`.staging` 不能进入 discovery 或 export。

根 `config.json` 只保存 Host-owned 产品组合和非敏感 Pack selection/routing。`plugins/<plugin-id>/manager/config.json` 只保存该 Manager 的有界非敏感偏好，不能保存 enablement、trust、Pack identity、Provider endpoint/auth destination 或 credential。

项目 `.mcode` 不参与 Manager/Pack discovery，不能覆盖 `plugins.json` 的 trust/source/active hash、Host Pack selection/routing、Provider endpoint/auth destination 或 credential。

## 6. 当前实现状态（非目标）

当前 `main` 尚无本页的 Manager registry、三个 target world、typed Pack Service、installation authority 或 Host adapter 路径。旧 Core compaction pipeline、direct MCP runtime 与仅供它使用的 vendored `rmcp` 已删除；旧 Session crate、global JSONL/store/path、CLI assembly、legacy Session config scope 及 Core Session product public API/runtime 也已删除。旧 `mcode-llm` crate 及其 profile、catalog、identity、header、wire、HTTP、SSE、registry、fallback、旧 Provider/stream/error 实现和全部专属 tests/fixtures/live ignored tests 已整体删除，且未迁移或保留 stub、legacy namespace、adapter、compatibility、fake 或 unavailable Provider。独立的 `mcode-provider-api` 仍只提供 provider-neutral Agent↔Host Rust port，`mcode-agent` 已迁到该边界；该 port 不是 `mcode:provider-pack@0.1.0` world、产品 extension surface 或 Provider 实现，也不表示 Host adapter 或 T11 Provider 能力已交付。仓库级审计确认 Core 的 provider wire-only state 是 T5 唯一 blocker；replay、assistant phase、thinking replay 与 tool-call item id 现已连同 rich/object wire shape 完整删除，Text/Thinking 只接受 plain string，Core DTO 在 provider-neutral serde 边界递归 fail closed。Core 的 direct `url` import/dependency 与 workspace direct declaration 也已删除，未保留 alias、deprecation、adapter、compatibility 或 fallback；T5 因此完成，下一步是 T6。现有 Plugin ABI v1 (`mcode:plugin@0.1.0`) 仍待 T7 由三个 target world 替换并由 loader/runtime 拒绝，不是 compatibility 或 fallback。实施阶段见 [04-roadmap.md](04-roadmap.md)，具体执行约束见 [05-plugin-impl.md](05-plugin-impl.md)。
