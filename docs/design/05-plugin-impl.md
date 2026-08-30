# Plugin 实施契约

> 本文约束首个开发者预览的目标 runtime；公开的 MCode product/workspace release、所有第一方 Manager/Pack package/release，以及 MCode-owned Manager、FeaturePack、ProviderPack 的 package/world/interface 均为 sole-current `0.0.1`。Manager 只有 `mcode:plugin@0.0.1`，只保留 current golden、parser 和 binding；不保留 `abi_v1.json`、historical golden、compatibility parser/adapter、ABI alias、dual-read 或 fallback。已删除旧 manifest/capability/contribution/state/UI/event/provenance 与 generic guest runtime。没有 direct Web/MCP/AgentRun capability 或通用 JSON 逃生舱。
>
> 截至本文修订时，仓库已包含三套 sole-current ABI artifacts：Manager current WIT/golden 与 strict task-wire contract，以及 FeaturePack/ProviderPack parser-checked WIT/current goldens 和 semantic JSONL goldens；Provider 的 crate-private pure DTO validator foundation 亦已落地。T7 仍缺 closed `AdapterContractV1`、static dummy context counter、完整 pure decoder reducers、13 candidate worlds × 13 validators rejection matrix 与完整 binary static preflight；Store/instantiate、discovery、装载与 lifecycle runtime 属于后续 T8，T7 未完成前不得进入。

## 1. 选择、装载与生命周期

Core/Host 只对 `plugins.json` 的 12 个 Manager 进行 discovery、验证和装载。Manager 是其 nested `packs/<pack-id>/` 的唯一 discovery、验证、选择、加载、配置、状态与 UI 请求方；Host 不扫描或直接加载 Pack。Manager 仅经对应 typed Host Pack Service 激活 Pack，且不读取 installation state/payload 或执行安全验证。

```text
Manager enabled/source/active hash/trust
  -> verify Manager identity + Manager world
  -> establish matching typed Host Pack Service
  -> Manager submits family-bound Pack activation
  -> Service opens authoritative Pack state/payload
  -> verify source/signature/trust/version/hash/world/golden
  -> instantiate Pack; bind generation/leases; return bounded state
```

trap、timeout、cancel、stale generation、unknown/跨 family/未声明/oversized request 都 fail closed。reload 取消 pending UI/service operation；Host 回收 Pack task、stream、interaction、singleton lease。阻塞 interaction 使用 generation-bound RAII lease，waiting start/end 在异常、取消、reload、drop 时严格 exactly once 配对。

## 2. 三个 ABI packages / 十三个 worlds

| package / world group | boundary | 要求 |
| --- | --- | --- |
| Manager Plugin `mcode:plugin@0.0.1` / `mcode:plugin/manager@0.0.1`（1 world） | 唯一 import 是 `mcode:plugin/feature-service@0.0.1` 的 `start-task` / `poll-task` / `cancel-task` string transport；唯一 export 是 `mcode:plugin/manager-lifecycle@0.0.1` 的 typed `initialize` / `poll` / `shutdown` lifecycle | Host 先绑定 canonical family、Manager ID 与 generation，将 `1..=9007199254740991` 的 generation 放入 typed `initialization-context` 调用 `initialize`，再选择 family-specific typed decode |
| FeaturePack `mcode:feature-pack@0.0.1`（11 worlds；全部 MCode-owned world/interface 均为 `@0.0.1`） | `session`、`compaction`、`resources`、`ask`、`todo`、`web`、`mcp`、`usage`、`subagents`、`workspace`、`ui` 各自独立 world 和 typed `invoke` / `pull`，见 [07-pack-abi.md](07-pack-abi.md) | 不与 Manager gateway 混用；每个 pull 有 `pending\|progress\|complete\|failed`，无第二 error channel |
| ProviderPack `mcode:provider-pack@0.0.1` / `mcode:provider-pack/provider@0.0.1`（1 world；export `mcode:provider-pack/provider-api@0.0.1`） | zero-import descriptor/catalog/auth-interaction/prepare-request 与 response decoder，见 [08-provider-pack-abi.md](08-provider-pack-abi.md) | Host 独占 route、catalog、transport、credential/auth |

Manager、FeaturePack 与 ProviderPack 的 sole-current ABI artifacts 已落地；FeaturePack 与 ProviderPack 已具备 parser-checked source/current goldens 和逐 world semantic JSONL goldens。三者的目标字段表、exact signatures、artifact slice 边界和 T7 gates 分别由本文、[07-pack-abi.md](07-pack-abi.md) 与 [08-provider-pack-abi.md](08-provider-pack-abi.md) 定义。这不表示 T7 已完成：artifact validators 仍缺 closed `AdapterContractV1`、static dummy context counter、完整 pure decoder reducers、13 candidate worlds × 13 validators rejection matrix 与完整 binary static preflight，完成后方可进入 T8。11 个 Feature worlds 共享唯一 package version `mcode:feature-pack@0.0.1`，但每个 world 的 binding、type namespace 与 current-only golden 物理独立；全部 MCode-owned FeaturePack interface 同为 `@0.0.1`，且不保留 `abi_v1.json`、historical golden、compatibility parser/adapter、ABI alias、dual-read 或 fallback。Manager task wire 的公开 `abiVersion` 固定为精确 string `"0.0.1"`，wire 仅有 `kind=featureService` 且不接受 caller-supplied family；Manager 只能使用 Host 在 `initialize` typed context 中提供的 active generation 构造 task wire。`operationId` 是 `1..=128` bytes 的 declarative canonical key，与 Host vault operation authority 共用唯一 validator；Host 在 body decode、Pack 与 transport 前，以最多 128 项的已绑定声明集 fail closed。`taskId` 仍是 Host-issued task instance identity。`start-task` 在分配前的拒绝统一为仅含 `abiVersion/kind/state/error` 的 `FeatureTaskRejection`；带 `operationId/taskId/generation` 的 `FeatureTaskError` 只属于已分配 task 的 poll/cancel 生命周期。JSON 只承载固定字段、有界且 family-body typed 的 task envelope，公共 API 不接受或返回 `serde_json::Value`、raw opaque body、无界 map 或延迟解释字段；不创建共享 `PackOperation`。Provider `toolChoice` 固定为 `Unset|Auto|None|Specific`，每种 wire 冻结 omitted 语义。

## 3. Auth 与 transport service

Provider/Web/Usage Pack 的签名 manifest 必须包含 canonical service/account/issuer/auth schema、trusted signer/source、credential-contract version、operation、exact method/origin/path 与 auth slot。安装/activation 审批这些精确 authority。Host 对每个 active Pack 自动匹配 `plugins/.host/auth.json` 的 canonical account；匹配全部 contract 字段时重用一份 secret，不需要重复 key 或 per-Pack login。mismatch/new authority 拒绝并要求 rebind。

Host 从 consumer family、Manager/Pack identity/version/hash/generation、account/version、provider/source、signer/source、contract、operation、request target 与 auth destination 导出单次 generation-bound injection lease。Host 独占 HTTP/TLS/DNS/proxy、same-origin redirect、timeout/retry/cancel/backpressure、credential lookup/refresh/insertion、reserved header、redaction、allowlist、generation 与审计。Pack 不得看见 secret/grant、设置 `Authorization`、`Proxy-Authorization`、`Cookie`、`x-api-key`、`api-key`、`cf-aig-authorization`、`Host` 或 `Content-Length`，也不得运行时扩展 endpoint/auth destination。

## 4. Route、Usage 与 UI

Host 在已验证 route/request/terminal 边界盖章 Manager/Pack identity/version/hash/generation、provider、request/turn ID、requested model/alias、endpoint/auth fingerprint，并生成 immutable `ModelRouteLease`、`UsageContextSnapshot`、`UsageSample`。缺失信息保持 `None`；route generation 更新后拒绝旧事件、重复 terminal 与未绑定事件。

Usage 与 Provider 独立。Usage Pack 只处理自己的 canonical source，不能查询 Provider 或推测模型；Usage Manager 依根配置顺序，将 bounded normalized row/card 组合到固定 `status.trailing/usage.summary` 和 `panel/usage.details`。Pack 不抢 slot 或 custom draw。UiPack 负责布局，Theme Pack 只提供 tokens；所有跨 family 调用只经 typed Host service。

## 5. 必需验证

验证至少覆盖：12 Manager registry 与 third-party nested-only 拒绝、Pack authority、三个 package、13 个 world 的 no-WASI/golden/交叉拒绝、family binding 早于 decode、generation/cancel/quiescence、Provider/Web/Usage contract exact match/rebind、reserved headers/redaction、route/source/singleton collision、immutable usage inputs 与固定 slots、UTF-8-safe terminal chunking。
