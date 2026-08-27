# 插件系统

> 对应 crate:`mcode-plugin-api` / `mcode-plugin-host`
> 参考:pi ExtensionAPI(~35 事件、三种调度语义、热重载)、grok-build plugin.json(manifest 资源包、TrustStore、marketplace、hooks 门)

## 1. 三层插件形态

| 层 | 形态 | 能做什么 | 场景 |
| --- | --- | --- | --- |
| **Tier 1 清单式** | `plugin.toml` + 资源目录,零代码 | skills/prompts/agents/themes/命令钩子/MCP 配置 | 团队规范包、提示词集、模型配置 |
| **Tier 2 WASM(默认代码插件)** | wasm component(WIT 契约) | 全生命周期钩子、注册工具/命令/渲染、Provider | 所有需要逻辑的扩展 |
| **Tier 3 进程/MCP** | stdio JSON-RPC(MCP 兼容) | 工具、资源、prompt | 生态兼容、语言无关、重隔离 |

原则:**能力递进,体验一致**。三层都产出同一种运行时对象(`ToolDyn` / hook handler / command),loop 不知道来源。

## 2. Tier 1:清单式插件包

```toml
# plugin.toml
name = "team-toolkit"
version = "0.1.0"

skills   = ["skills/review.md"]
prompts  = ["prompts/commit.txt"]
agents   = ["agents/reviewer.toml"]
themes   = ["themes/dark.json"]

[[hooks]]                        # shell 命令钩子(无需代码运行时)
event = "session_start"
command = "./scripts/init-env.sh"
kind = "notify"                  # notify | gate

[mcp_servers.linear]
command = "npx"
args = ["-y", "@linear/mcp-server"]
```

发现路径:`<project>/.mcode/plugins/`(需 trust)→ `~/.mcode/plugins/` → marketplace 安装目录 → `--plugin-dir`。技能命名空间 `plugin:skill`(grok-build 前缀规则)。

## 3. Tier 2:WASM 代码插件(核心)

### 3.1 技术选型

- 运行时:**wasmtime + Component Model**(wasm32-wasip2)
- 契约:**WIT**,宿主定义一次,各语言生成 binding
- 沙箱默认项:fuel 限制、无网络(需声明 capability)、fs 白名单(cwd 内)、内存上限
- 热重载:重新 instantiate 组件,先跑 `session_shutdown` 钩子再卸载(pi 语义)

### 3.2 WIT 契约(草案)

```wit
package mcode:plugin@0.1.0;

interface types {
    record event { name: string, payload: string }   // payload = JSON
    variant hook-result {
        pass,
        block(string),                                // Gate:阻断 + 原因
        transformed(string),                          // Transform:改写后的 JSON
    }
    record tool-spec { name: string, description: string, params-schema: string }
}

interface host-api {
    register-tool: func(spec: tool-spec);
    register-command: func(name: string, description: string);
    on: func(event: string);                          // 订阅
    log: func(level: string, msg: string);
    emit-ui: func(renderable-json: string);           // 渲染描述(02 文档 §4)
}

world plugin {
    include wasi:cli/imports;
    import host-api;
    export on-event: func(e: event) -> hook-result;
    export call-tool: func(name: string, args-json: string, call-id: string) -> result<string, string>;
    export call-command: func(name: string, args: string) -> result<string, string>;
}
```

### 3.3 插件开发体验

```rust
// Rust 插件(cargo-component 模板)
use mcode_plugin::prelude::*;

mcode_plugin::plugin!(|api| {
    api.on("tool_call", |ev| {
        if ev.tool == "bash" && ev.args["command"].contains("rm -rf /") {
            return HookResult::block("危险命令");
        }
        HookResult::pass()
    });
    api.register_tool("todo", todo_tool);
});
```

```ts
// TS 插件(javy / componentize-js,体验对齐 pi)
import { on, registerTool } from "@mcode/plugin-sdk";

on("session_start", () => log.info("hello"));
registerTool({ name: "todo", description: "…", params: {…}, execute: async (args) => {…} });
```

语言支持优先级:**Rust(一等,cargo-component)→ TS(javy,M3 后接)→ Go(TinyGo，按需)**。宿主 WIT 不变，只加 SDK/模板。

### 3.4 失败隔离(两边项目的共同教训)

- 加载错误按插件收集到 `errors[]`,不阻塞启动，状态页可查看
- 钩子 trap/panic → 捕获 → 记事件 → **本次会话禁用该插件**(pi 的 stale-context 失效语义)
- WASM trap 天然进程隔离；内嵌 runner 用 `catch_unwind` + panic hook

## 4. 钩子系统(核心中的核心)

### 4.1 三种调度语义(pi 模型)

```rust
pub enum DispatchKind {
    Notify,     // 广播,返回值忽略;明确标注的会话事件可 { cancel }
    Transform,  // 中间件链:handler 返回值是下一个 handler 的输入,最后回宿主校验
    Gate,       // 可变事件 + 可阻断:handler 原地改事件,返回 pass/block
}

pub struct HookRunner { /* 按 load 顺序遍历插件 */ }
impl HookRunner {
    pub async fn notify(&self, ev: &Event);
    pub async fn transform<T>(&self, ev: &Event, value: T) -> T;
    pub async fn gate(&self, ev: &mut Event) -> GateResult; // Pass | Block(reason)
}
```

### 4.2 事件表(v0.1,精选自 pi 的 ~35 个)

| 事件 | 语义 | 触发点 |
| --- | --- | --- |
| `project_trust` | Notify | 项目 trust 解析后 |
| `resources_discover` | Transform | 资源扫描后(插件可注入 skill/prompt 路径) |
| `session_start` / `session_shutdown` | Notify | 会话生命周期 |
| `session_before_fork` | Notify(cancelable) | fork 前 |
| `user_prompt` | Transform | 用户输入进上下文前 |
| `before_provider_request` | Transform | 普通 AgentLoop 的 LLM 请求前(可改 system/messages/tools) |
| `message_start` / `message_end` | Notify / Transform | 流边界;end 可改整条 assistant 消息 |
| `context` | Transform | 组装上下文后(注入/裁剪) |
| `tool_call` | **Gate** | dispatch 前:改写参数 / 阻断 |
| `tool_result` | Transform | 回填上下文前(脱敏、摘要、截断) |
| `turn_start` / `turn_end` | Notify | 回合边界 |
| `stop_gate` | Gate | agent 本要停止时(可注入 followUp 继续) |
| `subagent_start` / `subagent_end` | Notify | 子代理生命周期 |

`tool_call` Gate 在 capability 绑定前运行;通过 Gate 后,声明 `search_access` / `file_access` 的工具只按最终参数绑定一次 `PreparedSearch` / `PreparedFile`,不能使用改写前的路径或句柄。Core 不再做规则求值或 Ask 提示。

新增规则：事件表进 `mcode-plugin-api` 语义化版本；新增事件向后兼容，改语义要 major。

**Compaction 是闭合核心例外**:`mcode-compaction` 不触发 `session_before_compact`/`after_compact`，其私有 provider request、transcript、模型输出和候选重建也不经过 `before_provider_request`、`context` 或 message hooks。插件不能观察、取消或改写压缩；会话 actor 只消费闭合核心返回的已验证结果。

### 4.3 命令与 CLI flag

`register_command("review", handler)` → `/review`;同名加数字后缀解析(pi 的 `/command1`)。`register_flag` 允许插件贡献 CLI flag(M3)。

## 5. Tier 3:进程插件 / MCP

- MCP client 内建于 `mcode-plugin-host`;server 配置来自 Tier 1 manifest 的 `[mcp_servers]` 或 settings
- MCP 工具包装成 `ToolDyn` 进 Registry，名称 `mcp:<server>:<tool>`，与内建工具同一 dispatch
- 后续可加自定义 JSON-RPC 协议支持非 MCP 的 daemon，但 v1 只吃 MCP 生态

## 6. 治理：Trust / 安装 / 状态

```toml
# ~/.mcode/trust.toml
trusted_dirs = ["~/work/team-plugins"]
trusted_plugins = ["team-toolkit@0.1.0"]

# ~/.mcode/settings.toml
[plugins]
enabled = ["team-toolkit"]
disabled = ["experimental-x"]
```

- **trust 门控**：项目级(`.mcode/plugins/`)插件加载前要求目录在 TrustStore(grok-build TrustStore 模式);`mcode trust` 管理。
- **marketplace**:`mcode plugin install <git-url|name>` → git clone 到 `~/.mcode/plugins/marketplace/<name>`;`mcode plugin list/enable/disable/update`(grok-build marketplace 目录 + git 安装模式)。
- 每次会话启动输出已加载插件清单 + 失败原因(pi 的 `LoadExtensionsResult`)。

## 7. 待决策

- [ ] WIT 版本策略：`mcode:plugin@0.x` 接口演进时多版本并存还是强升级？
- [ ] WASM 插件的 fs 白名单粒度：cwd 整树 vs 显式声明路径
- [ ] TS SDK 走 javy(嵌 QuickJS)还是 componentize-js(SpiderMonkey)?体积 vs 兼容性
- [ ] Tier 1 的 shell-command 钩子安全边界(是否也走 trust)
