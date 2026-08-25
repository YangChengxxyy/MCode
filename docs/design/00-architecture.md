# MCode 总体架构

> 状态:草案 v0.1 · 决策待评审
> 参考实现:pi(`~/projects/pi`,TS 单进程插件架构)、grok-build(`~/projects/grok-build`,Rust ~95 crate 工作区)

## 1. 设计原则

MCode 是 **pi 的 Rust 重实现**:功能面对齐 pi(coding agent harness + 插件生态),TUI 的模块/进程架构对齐 grok-build pager(详见 06-tui.md)。架构吸取两个参考项目的经验:

**从 pi 继承**

- 插件体验优先:全生命周期钩子、`on()` 订阅 + `register_*()` 注册的统一 API、热重载
- Agent loop 是纯逻辑,不含 UI,不含会话持久化
- 会话 = JSONL append-only 日志,`id/parent_id` 构成树,支持 fork/分支

**从 grok-build 继承**

- Rust 工程化:actor + tokio 通道驱动会话,`Tool` trait + 类型擦除分发,`schemars` 单源 schema
- 安全分层:TrustStore + 权限规则引擎(allow/ask/deny)+ UI 确认,而不是"插件全权限"
- 引擎与 UI 边界协议化(pager ↔ shell 走 ACP);MCode 用"UI 中立渲染描述"达成同类解耦

**明确不抄**

- 不抄 pi 的无沙箱扩展模型 —— WASM 插件默认沙箱
- 不抄 grok-build 的 crate 粒度 —— 控制在 ~10 个 crate,先聚合,有证据再拆
- M1 不做 ACP 服务端;渲染走协议中立描述,ACP 适配器后补

## 2. Crate 布局

```
mcode/
├── crates/
│   ├── mcode-core          # 消息/事件/错误类型;零业务依赖的叶子
│   ├── mcode-llm           # Provider 抽象:StreamFn、模型注册表、auth
│   ├── mcode-agent         # AgentLoop:双循环、steer/followUp 队列、compaction 策略
│   ├── mcode-tools         # Tool trait、ToolDyn 擦除、Registry、PermissionEngine、内建工具
│   ├── mcode-session       # 会话 actor:JSONL 存储、事件广播、fork/resume/rewind
│   ├── mcode-plugin-api    # 插件契约:WIT 定义、事件类型、Host API DTO
│   ├── mcode-plugin-host   # 三种加载器(manifest / WASM / MCP)、HookRunner、TrustStore
│   ├── mcode-render        # UI 中立渲染描述(Renderable)定义
│   ├── mcode-tui           # ratatui TUI:AppView/actions/effects/scrollback/consent(06-tui.md)
│   ├── mcode-cli           # clap CLI;非 TTY 时走 headless 输出适配器
│   └── mcode               # 主二进制(composition root)
└── docs/design/
```

依赖方向(单向,不可成环):

```
mcode → mcode-cli → mcode-tui(render 适配)─┐
                 ↘ mcode-plugin-host → mcode-plugin-api
                    mcode-session → mcode-agent → mcode-tools → mcode-llm → mcode-core
```

- `mcode-agent`、`mcode-llm` **不知道 UI 存在**(pi 的 `runAgentLoop` 是纯函数,grok 的 shell 与 pager 分离,同一原则的两种做法)。收益:可测试、subagent 复用 loop、headless 免费。
- 内建工具与插件工具进同一个 `ToolRegistry`,无二等公民。

## 3. 关键横切决策

| 决策 | 选择 | 理由 |
| --- | --- | --- |
| 异步运行时 | tokio(current_thread + spawn_local 用于会话 actor) | 对订阅模型下 V8/TUI 友好;grok-build 同选择 |
| 插件运行时 | 三层:manifest(零代码)→ WASM(默认代码插件)→ MCP/进程(生态) | 见 03-plugins.md |
| 工具 schema | `schemars::JsonSchema` 派生,单源:运行时校验 + 发 LLM | grok-build 验证过的模式 |
| 会话存储 | JSONL 树 + `format_version` 头,目录 `~/.mcode/sessions/<cwd-slug>/` | pi v3 模式;迁移简单,grep 友好 |
| 权限 | 规则引擎(模式匹配)→ 钩子门 → UI ask,三级顺序求值 | grok-build 三级模型 |
| UI | ratatui;工具/插件返回**渲染描述**而非直接操作终端 | pi 的 renderCall/renderResult 思想,协议中立化 |
| 版本化 | 所有持久化格式带 version 字段,写入即迁移 | pi sessions v3 的教训 |

## 4. 目录约定(用户侧)

```
~/.mcode/               # 或 $MCODE_HOME
├── settings.toml
├── auth.toml
├── plugins/            # 用户级插件
├── sessions/<cwd-slug>/*.jsonl
├── skills/ prompts/ themes/
└── trust.toml          # TrustStore

<project>/.mcode/       # 项目级,加载前需 trust
├── plugins/  skills/  prompts/
└── settings.toml
```

加载顺序(后者覆盖):内置 < marketplace 安装 < 用户级 < 项目级 < CLI `--plugin-dir`。与 pi/grok 一致。
