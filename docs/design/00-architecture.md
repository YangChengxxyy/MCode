# MCode 冻结架构

> 状态：**冻结目标**。本文定义后续实现的边界；当前 `main` 未实现的 Manager、Pack 或 Service 不因本文而变为已落地能力。

## 1. 三层边界

| 层 | 职责 | 不负责 |
| --- | --- | --- |
| Agent Core | 最小 Agent loop 与七个 builtin 工具；消费 Host 提供的已验证动态工具表面 | Session、Provider、Compaction、UI、Host adapter、产品策略或 Pack 生命周期 |
| Host substrate | 三个 ABI runtime、签名与安装验证、caller/family 绑定、typed Service、OS 安全原语 | 选择或解释产品 feature 的语义、默认行为或授权策略 |
| 产品 Feature | 唯一 Manager 与已签名 Pack 的 feature 语义 | 绕过 Host 取得 OS、网络、secret 或 raw handle |

Agent Core 的唯一 builtin 名称是 `read`、`write`、`edit`、`find`、`grep`、`exec`、`shell`。它们的 Structured Exec、文件和搜索安全契约见 [02-tools-permissions.md](02-tools-permissions.md)。没有公开 `bash`、`PermissionEngine`、Core Ask、grant、`--yolo` 或按工具名授予特权。

builtin 名称不可覆盖。激活的 Manager+Pack 可以提交有界、typed、namespaced 的工具、命令、UI 与 feature contribution；Host 验证 provenance、family、schema、预算和名称后，才用 Host adapter 将其接入 Agent。Pack 不能直接注册或替换工具，也不能把通用 JSON 当成扩展逃生舱。

```text
Caller ─► Host 绑定 caller capability + feature family
                  │
                  ▼
Manager Plugin ── authorized request ─► Host-owned typed Pack Service ─► FeaturePack / ProviderPack
                  │                                      │
                  └─ bounded contributions ───────────────┴─► Host substrate adapter ─► Agent Core

Agent Core ─► seven canonical tools ─► Host OS safety primitives
```

每项产品能力首先是 `plugins/<feature>/manager/` 中的顶层 Manager Plugin，实际工作的 Pack 物理嵌套在同一 `plugins/<feature>/packs/<pack-id>/`。Core/Host 只装载顶层 Manager Plugin，不把 Pack 当作顶层 plugin entry；已装载的 Manager 只选择期望的嵌套 Pack 并请求激活。Manager guest 没有 filesystem 或 raw handle，不读取 installation state/payload，也不执行验签、trust 或兼容性判断。只有对应的 Host-owned typed Pack Service 能在已授权且 family-bound 的 Manager 请求下打开 installation state/payload，验证 source binding、signature、trust、version、hash、world 与 golden，实例化 Pack runtime、绑定 generation/leases，并向 Manager 返回有界状态。Core loader 不独立 discovery、选择或装载 Pack。该 Pack 生命周期不授予 guest filesystem、network、secrets、MCP 或 Subagents 的直接访问。Manager Plugin、FeaturePack 与 ProviderPack 都不获得 WASI、filesystem、process、socket、terminal、credential 或 raw handle。

## 2. 产品 Feature 注册表

除最小 Agent 和七个 builtin 外，产品能力必须经过唯一 Manager、Host-owned typed Service 和 signed Pack。下列 first-party family 保留 `com.mcode.*`，且每个 family 只能有一个 Manager：

- `com.mcode.providers`、`com.mcode.session`、`com.mcode.compaction`
- `com.mcode.resources`、`com.mcode.ask`、`com.mcode.todo`
- `com.mcode.web`、`com.mcode.mcp`、`com.mcode.usage`
- `com.mcode.subagents`、`com.mcode.workspace`、`com.mcode.ui`

第三方可为全新 feature 安装自己的唯一顶层 Plugin ID/Manager，但不得占用 `com.mcode.*`、保留的 built-in Plugin ID 或复制既有 family。first-party Pack 没有私有捷径。缺少、验签失败、trust 不匹配或版本不兼容的 Manager/Pack 必须 fail closed 并给出安装指引；Host 和 Core 不得代替实现。

## 3. 专属边界

- Session 是 `com.mcode.session` + `plugins/session/packs/mcode`。只有 `SessionPackService` 可以将 durable bytes 写入按 Pack ID、version、hash、generation 隔离的 SessionPack 数据区；Host 仅提供 no-follow owned storage、bounded WAL、atomic append、durability、backpressure、generation fence 与 DTO 验证。session/event/branch/resume/rewind/rollback 语义属于 SessionPack。
- Workspace checkpoint/rollback 是 `com.mcode.workspace` + `plugins/workspace/packs/mcode`，不在 Core。
- Provider 是 `com.mcode.providers` + `plugins/providers/packs/pi`。Host 独占 auth store、HTTP、TLS、DNS、proxy 和 reserved headers。
- Compaction 是 Host-wide singleton `com.mcode.compaction` + `plugins/compaction/packs/adaptive`；Core 没有 compaction 实现、hook、registry 或 fallback。
- Web、MCP、AgentRun/Subagents 仅经 Manager gateway 与对应 typed Service 运行；没有 direct kind/capability 双栈。

目录、安装权威性与三 ABI 见 [03-plugins.md](03-plugins.md)；执行边界见 [05-plugin-impl.md](05-plugin-impl.md)。

## 4. 当前实现状态（非目标）

当前仓库已有最小 loop 和七个 canonical builtin 的 library 基础；旧 Core compaction pipeline、direct MCP runtime 与仅供它使用的 vendored `rmcp` 已删除。CLI 中旧 Provider/Session assembly、catalog/profile/model/session path、Tokio runtime、headless renderer 及 `--provider`、`--profile`、`--model`、`--fake` 产品表面均已删除；`run`/`resume` 只保留 clap 骨架，并在解析后立即 fail closed，要求安装并激活 `com.mcode.providers` + signed Provider Pack 和 `com.mcode.session` + signed Session Pack。旧 `mcode-session` crate、global JSONL/store/path、CLI assembly、legacy Session config scope 及 Core Session product public API/runtime 已删除；仓库没有 global Session store、JSONL 或 compatibility fallback。中性的 `AgentEvent`/`MessageDelta`/`TurnOutcome` 是现行最小 Agent loop 协议，Core ids 只保留 `CallId`。旧 `mcode-llm` crate 及其 profile、catalog、identity、header、wire、HTTP、SSE、registry、fallback、旧 Provider/stream/error 实现和全部专属 tests/fixtures/live ignored tests 已整体删除，且未迁移或保留 stub、legacy namespace、adapter、compatibility、fake 或 unavailable Provider。独立的 `mcode-provider-api` 仍只提供 provider-neutral Agent↔Host Rust port，`mcode-agent` 已迁到该边界；该 crate 不是 `mcode:provider-pack@0.1.0` world、产品 extension surface 或 Provider 实现，也不表示 Host adapter 或 T11 Provider 能力已交付。仓库级审计确认 Core 的 provider wire-only state 是 T5 唯一 blocker；`ReplayWire`/`ReplayDomain`/`ReplayState`、assistant phase、thinking replay 与 tool-call item id 现已连同 rich/object wire shape 完整删除，Text/Thinking 只接受 plain string，Core DTO 在 provider-neutral serde 边界递归 fail closed。Core 的 direct `url` import/dependency 与 workspace direct declaration 也已删除，未保留 alias、deprecation、adapter、compatibility 或 fallback；T5 因此完成，下一步是 T6。Plugin ABI v1 (`mcode:plugin@0.1.0`) 仍待 T7 由三个 target world 替换并在 loader/runtime 拒绝，不是 compatibility 或 fallback。冻结目标中的三个 ABI、Manager registry、typed Pack Service、动态 Host adapter 和产品 UI Pack 均未因此文档变为已实现能力。迁移删除时机与 fail-closed 要求见 [04-roadmap.md](04-roadmap.md)。
