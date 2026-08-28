# MCode 设计文档

> 本目录记录冻结 target 与已核对的当前实现状态。目标描述不代表 Manager、Pack、world 或 Service 已实现；迁移前 direct runtime 不是 compatibility 或 fallback。

## 阅读顺序

| 文档 | 内容 |
| --- | --- |
| [00-architecture.md](00-architecture.md) | 最小 Agent Core、七个 builtin、动态 Host adapter 与产品边界 |
| [01-agent-core.md](01-agent-core.md) | Agent loop，以及 Session/Workspace/Provider/Compaction/auth 边界 |
| [02-tools-permissions.md](02-tools-permissions.md) | canonical tools 安全契约与受限 namespaced 动态贡献 |
| [03-plugins.md](03-plugins.md) | 唯一 Manager、三个 world、严格源/用户目录、安装权威性与 Host adapter |
| [04-roadmap.md](04-roadmap.md) | T5旧pipeline删除、T6/T11 auth、T7三ABI、T9 SessionPack、T12 interactive UI、T22–T27收口 |
| [05-plugin-impl.md](05-plugin-impl.md) | Manager gateway、FeaturePack invoke/pull、ProviderPack、Session durable bytes 与验证 |
| [06-tui.md](06-tui.md) | interactive UiPack、终端 Host substrate 与独立 typed headless CLI |

## 冻结结论

- Agent Core 只有最小 loop 和 `read`、`write`、`edit`、`find`、`grep`、`exec`、`shell` 七个不可覆盖的 builtin。
- Manager+Pack 可经 bounded typed contribution 和 Host adapter 增加 namespaced 工具、命令、UI 与 feature；不能直接注册、覆盖或使用 generic JSON escape hatch。
- 上述 12 个 first-party family 保留各自唯一的 `com.mcode.*` Manager/Service/Pack 链路；第三方可为全新 feature 安装自己的唯一 Manager，但不得占用 `com.mcode.*` 或复制既有 family。
- ABI 独立为 Manager Plugin `mcode:plugin@0.2.0`、FeaturePack `mcode:feature-pack@0.1.0`、ProviderPack `mcode:provider-pack@0.1.0`；各自 version/golden/no-WASI。
- Session durable bytes 只由 SessionPack Service 写入 Pack 隔离数据区；不存在全局 Session tree。
- 没有公开 `bash`、`PermissionEngine`、Core Ask、grant、`--yolo` 或按名称特权。

具体目录与权威性以 [03-plugins.md](03-plugins.md) 为准；未落地项和 fail-closed 时序以 [04-roadmap.md](04-roadmap.md) 为准。
