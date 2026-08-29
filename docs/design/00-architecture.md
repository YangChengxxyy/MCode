# MCode 冻结架构

> 本文定义产品边界；缺失的 Manager 或 Pack 只显示准确安装指引，绝不由 Core 回退实现。

## 1. 三层边界

| 层 | 职责 | 不负责 |
| --- | --- | --- |
| Agent Core | 最小 Agent loop、七个 canonical builtin、消费已验证动态表面 | 产品 feature、选择、持久化、auth、网络或 Pack 生命周期 |
| Host substrate | ABI runtime、签名/安装验证、typed Service、credential/network/OS 安全原语 | 产品策略或隐式默认 |
| 产品 Feature | Manager 编排和签名 Pack 语义 | 获得 OS、network、secret 或 raw handle |

Core builtin 只有 `read`、`write`、`edit`、`find`、`grep`、`exec`、`shell`，名称不可覆盖。Manager+Pack 可提出有界 typed、namespaced 工具、命令、UI 与 feature contribution；Host 验证 provenance、family、schema、预算和 generation 后才创建 adapter。通用 JSON 不得成为扩展逃生舱。

```text
Caller -> Host binds caller identity + family
             -> Manager -> typed Host Pack Service -> active nested Pack
             -> Host adapter -> Agent Core
```

Host 只装载 `plugins/<feature>/manager/`；Manager 请求选择和激活 `plugins/<feature>/packs/<pack-id>/`，但不读取 payload 或 installation state。对应 typed Host Pack Service 独占打开、验签、trust、world/golden、实例化与 generation/lease 绑定。Manager/FeaturePack/ProviderPack 均无 WASI、OS、filesystem、network、process、terminal、socket、credential 或 raw Host handle。

## 2. 固定产品注册表

顶层 Manager family **只有且永远只有 12 个**：

`providers`、`session`、`compaction`、`resources`、`ask`、`todo`、`web`、`mcp`、`usage`、`subagents`、`workspace`、`ui`。

它们保留对应 `com.mcode.*` identity、feature directory 与 typed Host service。未知或第三方顶层 ID、Manager、family、Host service 和 `com.mcode.*` identity 均 fail closed。第三方只能为上述已发布 family 提供签名 nested Pack；新 family 先由 MCode 版本定义 Manager、service、ABI/golden 与保留 ID。第一方和第三方使用完全相同的签名、source trust、安装、更新、generation、provenance、限额与隔离链路；第一方唯一源码、构建、发布仓库为 `https://github.com/MCapricorns/MCode_plugins`。

active cardinality 固定如下：

- Providers：`N` 个；
- Usage：canonical source key 不重复的 `N` 个；
- UI：恰好一个 product UiPack，外加 `N` 个 Theme-role Pack；
- 其余 family（包括 Web）：Host-wide `0..1` singleton。

同一 Pack identity 同时只能有一个 generation。ID、source、route、auth slot、source key、role 或 singleton 冲突，以及未排空旧 generation，均 fail closed；不得以名称、安装顺序、加载顺序或隐式 priority 决胜。

## 3. 专属边界

- Session：`session` Manager 和 `plugins/session/packs/<pack-id>`；仅 `SessionPackService` 写入该 Pack `data/`，Host 仅提供隔离 storage/WAL/bounds/fence。
- Provider：`providers` Manager 和 ProviderPack；Host 独占 HTTP/TLS/DNS/proxy、credential 与 reserved headers。Provider Pack 只准备有界非敏感请求和解析有界响应。
- Usage：独立于 Provider；仅接受 Host-stamped immutable route/context/sample，不查询 Provider 或从字符串、Session、widget、quota 推测模型。
- Compaction：Host-wide singleton；切换前 cancel/drain，随后原子切换 generation。
- Web、MCP、Subagents：仅经各自 Manager gateway 与 typed Service；没有 direct capability。
- UI：一个 UiPack 负责布局与交互；Theme Pack 仅提供 style tokens。UI 不得直接跨 family 调用、共享对象或 raw draw。

## 4. 凭据、网络与 UI 安全

唯一 credential authority 是 Host-only `~/.mcode/plugins/.host/auth.json`。每个签名 Provider/Web/Usage Pack 声明精确 canonical service/account/issuer/auth schema、trusted signer/source、signed credential-contract version，以及 `operation + method + origin + path + auth slot`。安装或激活时批准此 authority；Host 为每个 active Pack 自动精确匹配 canonical account，因而同一服务/account 不重复输入 key 或逐 Pack 登录。任何 signer、contract、origin、scheme 或 destination 的新增或不匹配均 fail closed/rebind。

Host 独占 injection、redirect、timeout、retry、cancel、backpressure、reserved-header policy 与 redaction；Pack 不能设置 auth/reserved headers、扩展 endpoint 或 destination、读取 secret 或借用其他 operation。详见 [03-plugins.md](03-plugins.md)。

终端能力 image/true-color/hyperlinks 分别为 `Auto|ForceOn|ForceOff`，root 显式设置优先。Host 清理 control/bidi，所有输出按 UTF-8 boundary 分块且每块 `<=1 MiB`。
