# T7 FeaturePack ABI authority

> 本文冻结 `mcode:feature-pack@0.0.1` 的唯一 current、first developer preview 目标契约，不声称 T7 或任一 Pack runtime 已实现。本文是仓库内可审查的 FeaturePack authority；紧随 T7 交付的 parseable WIT source、current LF golden 与 semantic JSONL golden 必须是其 machine-verifiable projection。
>
> 所有 schema/type/field/variant/function 名称使用英文；说明使用中文。本文只声明 Manager、全部 FeaturePack world/interface 与 Provider reference 的 sole-current `0.0.1` ABI 和 typed surface；不存在旧版本共存，不保留任何旧版本文件、`abi_v1.json`、historical golden、compatibility parser/adapter、ABI alias、dual-read 或 fallback，也不提供通用 payload、shared DTO、public `Value`、map 或 `metadata/extensions` 字段。目标契约、machine-verifiable artifact 与后续 runtime 分阶段验证。

## Authority map

以下主题文件共同构成 FeaturePack ABI authority；章节编号在拆分后保持连续且规范效力不变。

- [Topology and artifact boundary](07-pack-abi-topology.md) — §1
- [Common exact surface, ownership and authority](07-pack-abi-common.md) — §2
- [Session, compaction and resources](07-pack-abi-session-resources.md) — §3–§5
- [Ask and todo](07-pack-abi-ask-todo.md) — §6–§7
- [Web and MCP](07-pack-abi-web-mcp.md) — §8–§9
- [Usage and subagents](07-pack-abi-usage-subagents.md) — §10–§11
- [Workspace and UI](07-pack-abi-workspace-ui.md) — §12–§13
- [Safety and artifact gates](07-pack-abi-gates.md) — §14–§15
