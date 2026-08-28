# 路线图

## M1 — 最小闭环(无插件) · 任务拆解见 [07-m1-plan](07-m1-plan.md)

目标:能跑通"prompt → LLM → 工具 → 回填"全链路,headless CLI。

- [x] `mcode-core`:Message/ToolCall/ToolResult/Message 事件类型
- [x] `mcode-llm`:1 个 OpenAI 兼容 Provider + API key auth,`EventStream`
- [x] `mcode-agent`:双循环 + steer/followUp + abort(CancellationToken);FakeProvider 驱动的 loop 集成测试
- [x] `mcode-tools`:trait + Registry(read/write/edit/shell/exec/grep/find);已注册 schema-valid 调用直接执行
- [x] `mcode-session`:JSONL 存储(format_version=1)+ SessionHandle(broadcast 事件)+ resume
- [x] `mcode-cli`:`mcode run "..."` / `resume` + 流式打印;无 TUI

验收:在无 TUI 的终端里完成一次多轮工具调用会话并可 resume。

## M2 — Tier 1 插件 + 治理

- [ ] plugin.toml manifest 解析、发现路径、覆盖顺序
- [ ] skills(SKILL.md frontmatter)/ prompts / themes 资源接入 loop
- [ ] shell-command 钩子(notify + gate)+ TrustStore(项目级门控、`mcode trust`)
- [ ] `plugin.toml` 的 `[mcp_servers]` → MCP client 最小实现(tools/call only)
- [ ] HookRunner 三种语义骨架(此时只有 shell 钩子在链上)

## M2.5 — TUI(06-tui.md)

目标:交互式终端体验对齐 pi 核心场景。

- [x] `mcode-render`:`RenderBlock`(Text/Markdown/Diff/Table/Tree/Progress/Error/Widget)+有界 headless 纯文本降级
- [ ] `mcode-tui`:AppView/ActionRegistry/actions/effects/scrollback/input;纯状态、能力降级和 ratatui 基础适配器已落地,编辑器 + 流式渲染 + 工具块待完成
- [ ] consent 模态保持为有界、permission-shaped 的宿主交互表面(Core 不解释选项,也不接授权引擎)
- [ ] 状态栏(模型/usage/cwd)+ `/model` `/session` `/quit` 基础命令
- [ ] resume picker(JSONL 会话树浏览)

验收:tmux 里完成一次多轮会话,Esc steer 生效。

## M3 — WASM 插件(开发体验核心)

- [ ] `mcode-plugin-api`:WIT 契约 v0.1 + Rust guest SDK(cargo-component 模板)
- [ ] `mcode-plugin-host`:wasmtime 加载、燃料/内存/沙箱配置、热重载(`mcode reload`)
- [ ] 事件表 v0.1 全量接到 loop(03-plugins §4.2)
- [ ] 插件注册工具 → `ToolDyn` 包装进 Registry;`register_command` → `/cmd`
- [ ] 渲染描述 emit-ui → ratatui 适配器
- [ ] 2 个 example 插件:hello(钩子)+ todo(有状态工具,对齐 pi examples)

## M4 — 生态与完善

- [ ] marketplace:git 安装 / enable / disable / update
- [ ] subagent(并行子代理、fork 上下文)
- [ ] Compaction 的 session actor 原子接入 + rewind(`mcode-compaction` 闭合 foundation 已落地)
- [ ] TS 插件 SDK(javy)
- [ ] ACP 服务端 / web UI(渲染描述协议复用)
- [ ] 多 Provider 注册表 + OAuth(Provider 也作为插件)

## 里程碑间的硬约束

- 每个 M 结束:cargo workspace 全量 test 绿 + `clippy -D warnings` 绿
- 公共类型(`mcode-core` / `mcode-plugin-api`)变更必须同 PR 更新 design 文档
- 事件表/WIT 只允许向后兼容演进,破坏性变更需要 ADR

## ADR 议题池(到点开 ADR)

1. WIT 多版本并存策略
2. steer 时 partial message 进不进上下文(01 §6)
3. MCP 工具名前缀与冲突(02 §7)
