# Manager、Pack 与 Host substrate

> 状态：**冻结目标**。本页的 Manager、Service、world 与目录规则未因文档存在而声称已落地。

## 1. 组成与权限边界

每个产品 Feature 只有一个 Manager 和一个 Host-owned typed Service。Service 按该 family 明确冻结 singleton、single-active 或无冲突 multi-active/routing 基数；不得靠名称、加载顺序或隐式 priority 决胜。Manager 定义 feature 编排；Host 负责 signer、trust、安装、runtime、caller/family 绑定、资源限制和 OS substrate；Pack 定义 feature 语义。Host 不携带产品策略，first-party 与 third-party 均无私有路径。Compaction 额外固定为 Host-wide singleton。

Manager 不得直接访问 filesystem、network、secrets、MCP、Subagents 或 discovery/load Pack。所有三类 guest world 均不获得 WASI、filesystem、process、socket、terminal、credential 或 raw handle。Manager/Pack 缺失、签名或 trust 不匹配、版本不兼容、DTO 越界或 generation 过期时必须 fail closed，并返回安装指引。

## 2. 第一方 Manager 与源目录

第一方 Manager 源根固定为 `MCode_plugins/plugins/<manager>/`，下表冻结保留的 `com.mcode.*` family 与目录。第三方可以为全新 feature 安装自己的唯一 Manager，但不得占用 `com.mcode.*`、复制既有 family 的 Manager，或绕过相同签名、trust、生命周期和 Service 约束。

| Feature | Manager | Manager 源目录 | first-party Pack 源目录 |
| --- | --- | --- | --- |
| Providers | `com.mcode.providers` | `MCode_plugins/plugins/providers/` | `provider_plugins/pi` |
| Session | `com.mcode.session` | `MCode_plugins/plugins/session/` | `session_plugins/mcode` |
| Compaction | `com.mcode.compaction` | `MCode_plugins/plugins/compaction/` | `compaction_plugins/adaptive` |
| Resources | `com.mcode.resources` | `MCode_plugins/plugins/resources/` | `resource_plugins/mcode` |
| Ask | `com.mcode.ask` | `MCode_plugins/plugins/ask/` | `ask_plugins/mcode` |
| Todo | `com.mcode.todo` | `MCode_plugins/plugins/todo/` | `todo_plugins/mcode` |
| Web | `com.mcode.web` | `MCode_plugins/plugins/web/` | `web_plugins/mcode` |
| MCP | `com.mcode.mcp` | `MCode_plugins/plugins/mcp/` | `mcp_plugins/mcode` |
| Usage | `com.mcode.usage` | `MCode_plugins/plugins/usage/` | `usage_plugins/mcode` |
| Subagents | `com.mcode.subagents` | `MCode_plugins/plugins/subagents/` | `subagent_plugins/mcode` |
| Workspace | `com.mcode.workspace` | `MCode_plugins/plugins/workspace/` | `workspace_plugins/mcode` |
| UI | `com.mcode.ui` | `MCode_plugins/plugins/ui/` | `ui_plugins/mcode` |

不得以第二个 Manager、按名称特权、普通 hook、直接 transport 或 Core fallback 复制这些 product Feature。Session、Workspace、Provider 和 Compaction 边界见 [01-agent-core.md](01-agent-core.md)。

## 3. 三个独立 world

| world | guest | 唯一边界 | 独立要求 |
| --- | --- | --- | --- |
| Manager Plugin `mcode:plugin@0.2.0` | Manager | 仅通过 Plugin WIT 的 `start-task` / `poll-task` / `cancel-task` JSON gateway 发起新的唯一 FeatureService operation | 独立 version、binding、golden、no-WASI |
| FeaturePack `mcode:feature-pack@0.1.0` | 除 Provider 外的产品 Pack | FeaturePack Service 自己的 typed `invoke` / `pull` 边界 | 独立 version、binding、golden、no-WASI |
| ProviderPack `mcode:provider-pack@0.1.0` | Provider Pack | typed provider request/stream/error 边界 | 独立 version、binding、golden、no-WASI |

Manager gateway 的 JSON 只是有界 transport envelope，不是通用 API：Host **先**绑定 caller capability 与 feature family，随后才按该 family typed decode。Manager guest 不能直接调用 FeaturePack Service、ProviderPack Service 或 OS substrate。

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
├── plugins/
│   └── <manager-id>/                  # first-party 为保留的 com.mcode.<feature>
│       ├── config.json
│       ├── installation.json
│       ├── data/
│       └── versions/
├── provider_plugins/
│   ├── auth.json
│   └── <pack-id>/
│       ├── installation.json
│       ├── data/
│       └── versions/
└── <feature>_plugins/
    └── <pack-id>/
        ├── installation.json
        ├── data/
        └── versions/
```

`plugins.json` 只含 Manager entry，且对 Manager 的 `enabled`、source binding、active version+hash 与 trust high-water 唯一权威。`plugins/<manager-id>/installation.json` 只是 Host 从签名 bundle 生成的非权威 installation inventory/receipt，不能改变 routing、trust 或 active pointer。例如第三方全新 Manager `org.example.diagram` 只能使用 `plugins/org.example.diagram/`，不能占用 `com.mcode.*`。每个 Pack 的 `installation.json` 对其 source binding、selected version+hash、trust high-water 与安装 inventory 唯一权威；Pack payload、版本和 data 不能由 `plugins.json` 推断或替代。

Session durable bytes 仅由 `SessionPackService` 写入 `session_plugins/<pack-id>/data/`；Host 在操作边界绑定并验证 Pack ID/version/hash/generation，不把这些字段隐式编码成另一套公共目录协议。不存在独立的全局 Session durable 区、Session tree 或任何 lazy bootstrap 路径。`provider_plugins/auth.json` 是 Provider auth 的专用位置，访问时序见 [01-agent-core.md](01-agent-core.md)。

初始化只确保root、`plugins/`和`provider_plugins/`；Manager目录、其他family root、Pack、`auth.json`和Host-only `.staging/<transaction-id>/`都只在对应可信事务中lazy创建。`.staging`不能进入discovery或export。

根 `config.json` 只保存 Host-owned 产品组合和非敏感 Pack selection/routing。`plugins/<manager-id>/config.json` 只保存该 Manager 的有界非敏感偏好，不能保存 enablement、trust、Pack identity、Provider endpoint/auth destination 或 credential。

项目 `.mcode` 不参与 Manager/Pack discovery，不能覆盖 `plugins.json` 的 trust/source/active hash、Host Pack selection/routing、Provider endpoint/auth destination 或 credential。

## 6. 当前实现状态（非目标）

当前 `main` 尚无本页的 Manager registry、三个 target world、typed Pack Service、installation authority 或 Host adapter 路径。旧 Core compaction pipeline、direct MCP runtime 与仅供它使用的 vendored `rmcp` 已删除；旧 Session crate、global JSONL/store/path、CLI assembly、legacy Session config scope 及 Core Session product public API/runtime 也已删除。旧 `mcode-llm` crate 及其 profile、catalog、identity、header、wire、HTTP、SSE、registry、fallback、旧 Provider/stream/error 实现和全部专属 tests/fixtures/live ignored tests 已整体删除，且未迁移或保留 stub、legacy namespace、adapter、compatibility、fake 或 unavailable Provider。独立的 `mcode-provider-api` 仍只提供 provider-neutral Agent↔Host Rust port，`mcode-agent` 已迁到该边界；该 port 不是 `mcode:provider-pack@0.1.0` world、产品 extension surface 或 Provider 实现，也不表示 Host adapter 或 T11 Provider 能力已交付。当前唯一明确剩余的 T5 acceptance residue 是 Core `ReplayWire`/`ReplayDomain`/`ReplayState`、`AssistantPhase`/`TextBlock.phase`、`ThinkingBlock.replay` 与 `ToolCall.item_id` 的 wire-only cleanup，T5 尚未完成。现有 Plugin ABI v1 (`mcode:plugin@0.1.0`) 仍待 T7 由三个 target world 替换并由 loader/runtime 拒绝。以上迁移都不提供 compatibility、legacy 或 fallback。实施阶段见 [04-roadmap.md](04-roadmap.md)，具体执行约束见 [05-plugin-impl.md](05-plugin-impl.md)。
