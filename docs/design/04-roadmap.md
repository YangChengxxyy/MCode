# 路线图

## M1 — 最小闭环(无插件)

目标:能跑通"prompt → LLM → 工具 → 回填"全链路,headless CLI。

- [ ] `mcode-core`:Message/ToolCall/ToolResult/Message 事件类型
- [ ] `mcode-llm`:1 个 OpenAI 兼容 Provider + API key auth,`EventStream`
- [ ] `mcode-agent`:双循环 + steer/followUp + abort(CancellationToken);FakeProvider 驱动的 loop 集成测试
- [ ] `mcode-tools`:trait + Registry(read/write/edit/bash/grep)+ PermissionEngine(规则表 only,交互后补)
- [ ] `mcode-session`:JSONL 存储(format_version=1)+ SessionHandle(broadcast 事件)+ resume
- [ ] `mcode-cli`:`mcode run "..."` + 流式打印;无 TUI

验收:在无 TUI 的终端里完成一次多轮工具调用会话并可 resume。

## M2 — Tier 1 插件 + 治理

- [ ] plugin.toml manifest 解析、发现路径、覆盖顺序
- [ ] skills(SKILL.md frontmatter)/ prompts / themes 资源接入 loop
- [ ] shell-command 钩子(notify + gate)+ TrustStore(项目级门控、`mcode trust`)
- [ ] `plugin.toml` 的 `[mcp_servers]` → MCP client 最小实现(tools/call only)
- [ ] HookRunner 三种语义骨架(此时只有 shell 钩子在链上)

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
- [ ] Compaction(LLM 摘要策略)+ rewind
- [ ] TS 插件 SDK(javy)
- [ ] ACP 服务端 / web UI(渲染描述协议复用)
- [ ] 多 Provider 注册表 + OAuth(Provider 也作为插件)

## 里程碑间的硬约束

- 每个 M 结束:cargo workspace 全量 test 绿 + `clippy -D warnings` 绿
- 公共类型(`mcode-core` / `mcode-plugin-api`)变更必须同 PR 更新 design 文档
- 事件表/WIT 只允许向后兼容演进,破坏性变更需要 ADR

## ADR 议题池(到点开 ADR)

1. WIT 多版本并存策略
2. Gate 改写参数后权限重跑的边界(03 §7)
3. steer 时 partial message 进不进上下文(01 §6)
4. MCP 工具名前缀与冲突(02 §7)
