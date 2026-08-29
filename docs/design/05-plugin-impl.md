# Plugin v2 实施契约

> 本文约束目标 runtime；没有 direct Web/MCP/AgentRun capability、通用 JSON 逃生舱或旧 ABI compatibility 路径。

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

## 2. 三个独立 ABI

| world | boundary | 要求 |
| --- | --- | --- |
| Manager Plugin `mcode:plugin@0.2.0` | `start-task` / `poll-task` / `cancel-task` JSON envelope 的唯一 `FeatureService` kind | Host 先绑定 caller identity 与 strict family，再 family-specific typed decode |
| FeaturePack `mcode:feature-pack@0.1.0` | family-specific typed `invoke` / `pull` | 不与 Manager gateway 混用 |
| ProviderPack `mcode:provider-pack@0.1.0` | bounded descriptor、prepared request、response frame、normalized stream event | Host 独占 transport/auth |

每个 world 独立 version、binding 与 golden，且 no-WASI。JSON 只是有界 envelope；不得使用 `PackOperation`、`serde_json::Value`、无界 map、opaque blob 或延迟解释字段。Feature 使用独立 tagged DTO/golden。Provider `toolChoice` 固定为 `Unset|Auto|None|Specific`，每种 wire 冻结 omitted 语义。

## 3. Auth 与 transport service

Provider/Web/Usage Pack 的签名 manifest 必须包含 canonical service/account/issuer/auth schema、trusted signer/source、credential-contract version、operation、exact method/origin/path 与 auth slot。安装/activation 审批这些精确 authority。Host 对每个 active Pack 自动匹配 `plugins/.host/auth.json` 的 canonical account；匹配全部 contract 字段时重用一份 secret，不需要重复 key 或 per-Pack login。mismatch/new authority 拒绝并要求 rebind。

Host 从 consumer family、Manager/Pack identity/version/hash/generation、account/version、provider/source、signer/source、contract、operation、request target 与 auth destination 导出单次 generation-bound injection lease。Host 独占 HTTP/TLS/DNS/proxy、same-origin redirect、timeout/retry/cancel/backpressure、credential lookup/refresh/insertion、reserved header、redaction、allowlist、generation 与审计。Pack 不得看见 secret/grant、设置 `Authorization`、`Proxy-Authorization`、`Cookie`、`x-api-key`、`api-key`、`cf-aig-authorization`、`Host` 或 `Content-Length`，也不得运行时扩展 endpoint/auth destination。

## 4. Route、Usage 与 UI

Host 在已验证 route/request/terminal 边界盖章 Manager/Pack identity/version/hash/generation、provider、request/turn ID、requested model/alias、endpoint/auth fingerprint，并生成 immutable `ModelRouteLease`、`UsageContextSnapshot`、`UsageSample`。缺失信息保持 `None`；route generation 更新后拒绝旧事件、重复 terminal 与未绑定事件。

Usage 与 Provider 独立。Usage Pack 只处理自己的 canonical source，不能查询 Provider 或推测模型；Usage Manager 依根配置顺序，将 bounded normalized row/card 组合到固定 `status.trailing/usage.summary` 和 `panel/usage.details`。Pack 不抢 slot 或 custom draw。UiPack 负责布局，Theme Pack 只提供 tokens；所有跨 family 调用只经 typed Host service。

## 5. 必需验证

验证至少覆盖：12 Manager registry 与 third-party nested-only 拒绝、Pack authority、三个 world/no-WASI/golden/交叉拒绝、family binding 早于 decode、generation/cancel/quiescence、Provider/Web/Usage contract exact match/rebind、reserved headers/redaction、route/source/singleton collision、immutable usage inputs 与固定 slots、UTF-8-safe terminal chunking。
