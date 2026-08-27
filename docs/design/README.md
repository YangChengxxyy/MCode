# MCode 设计文档

| 文档 | 内容 | 对应 crate |
| --- | --- | --- |
| [00-architecture](00-architecture.md) | 设计原则、crate 布局、横切决策、目录约定 | 全局 |
| [01-agent-core](01-agent-core.md) | 消息模型、Provider 抽象、AgentLoop、会话存储、Compaction | core / llm / compaction / agent / session |
| [02-tools-permissions](02-tools-permissions.md) | Tool trait、Registry、流式输出、渲染描述、直接 dispatch | tools |
| [03-plugins](03-plugins.md) | 三层插件形态(manifest / WASM / MCP)、WIT 契约、钩子事件表、治理 | plugin-api / plugin-host |
| [04-roadmap](04-roadmap.md) | 里程碑 M1–M4、硬约束、ADR 议题池 | — |
| [05-plugin-impl](05-plugin-impl.md) | WASM 插件实现:host 加载器、双适配器、guest SDK、热重载、沙箱 | plugin-host |
| [06-tui](06-tui.md) | TUI:单进程协议化边界、AppView/actions/effects 分层、功能面清单、consent、headless | tui / render |
| [07-m1-plan](07-m1-plan.md) | M1 任务级拆解:T0–T6 依赖图、文件清单、测试矩阵、DoD 验收脚本 | 全部 |

## 决策记录

已拍板:

- **插件形式**:三层混合;代码插件默认 **WASM**(沙箱),Rust 一等开发语言,TS 经 javy 后续接入(2026-03-11)
- **演进方式**:先设计文档评审,再动工 M1(2026-03-11)
- **定位与 TUI**:MCode = pi 的 Rust 重实现;TUI 功能面对齐 pi、模块架构对齐 grok-build pager;v1 单进程,UI↔引擎只过 SessionCommand/SessionEvent 通道(2026-03-11)
- **读前必读**:00 的"明确不抄"清单 —— 三个否定决策(WASM 必沙箱、crate 粒度受控、M1 无 ACP)

参考文献(本地):

- pi 扩展文档:`~/projects/pi/packages/coding-agent/docs/extensions.md`
- pi agent loop:`~/projects/pi/packages/agent/src/agent-loop.ts`
- grok-build 工具 trait:`~/projects/grok-build/crates/codegen/xai-grok-tools/`(Tool/ToolDyn/schemars)
- grok-build 插件 manifest:`~/projects/grok-build/crates/codegen/xai-grok-agent/src/plugins/`
