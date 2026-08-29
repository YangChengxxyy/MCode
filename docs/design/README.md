# MCode 设计文档

> 本目录定义冻结的产品目标；目标不是已实现能力，也不提供旧路径的兼容或回退承诺。

## 阅读顺序

| 文档 | 内容 |
| --- | --- |
| [00-architecture.md](00-architecture.md) | Core、12 个 Manager family、Pack、Host 与 credential 边界 |
| [01-agent-core.md](01-agent-core.md) | Agent loop、Session、Provider、Usage 与 Compaction 边界 |
| [02-tools-permissions.md](02-tools-permissions.md) | canonical tools 安全契约与受限动态贡献 |
| [03-plugins.md](03-plugins.md) | nested Pack、目录、安装权威性、cardinality 与 auth contract |
| [04-roadmap.md](04-roadmap.md) | T6–T27 的依赖、交付顺序和验收 |
| [05-plugin-impl.md](05-plugin-impl.md) | 三个 ABI、生命周期、Host Service、route/usage 契约 |
| [06-tui.md](06-tui.md) | UiPack、Theme Pack、终端 Host substrate 与 typed headless CLI |

## 冻结结论

- Agent Core 只有最小 loop 和不可覆盖的 `read`、`write`、`edit`、`find`、`grep`、`exec`、`shell`；没有公开 `bash`、`PermissionEngine`、Core Ask、grant 或 `--yolo`。
- 顶层 Manager family **恰好 12 个**：`providers`、`session`、`compaction`、`resources`、`ask`、`todo`、`web`、`mcp`、`usage`、`subagents`、`workspace`、`ui`。未知、第三方顶层 ID、Manager、family、Host service 与 `com.mcode.*` identity 一律拒绝。
- 第三方只能实现已发布 family 的签名 nested Pack；新 family 必须由 MCode 版本先定义 Manager、typed Service、ABI/golden 与保留 ID。第一方 Manager/Pack 的唯一源码、构建与发布仓库是 `https://github.com/MCapricorns/MCode_plugins`。
- Core/Host 只装载 Manager；Manager 只能经对应 typed Host Pack Service 选择和激活自身 nested Pack。所有 guest 无 WASI、OS、filesystem、network、process、terminal、socket、credential 或 raw Host handle。
- Providers 为 `N`；Usage 为 canonical source key 唯一的 `N`；UI 为一个 product UiPack 加 `N` 个 Theme-role Pack；包括 Web 在内的其余 family 均为 Host-wide `0..1` singleton。
- 唯一 credential authority 是 lazy 的 `~/.mcode/plugins/.host/auth.json`。Host 对所有 active Provider/Web/Usage Pack 自动按精确签名 contract 匹配一份 canonical service/account secret；不匹配或新增 authority 必须 fail closed/rebind。
- Usage 与 Provider 独立。Usage 只消费 Host-stamped immutable `ModelRouteLease`、`UsageContextSnapshot`、`UsageSample`，并只贡献 `status.trailing/usage.summary` 和 `panel/usage.details`；UI 不得直接跨 family 调用或 raw draw，Theme 只提供 style tokens。

目录、权威文件和凭据契约见 [03-plugins.md](03-plugins.md)；ABI 与数据流见 [05-plugin-impl.md](05-plugin-impl.md)。
