# 冻结架构落地路线图

> 实施必须遵守依赖、签名链和 fail-closed 边界；产品 feature 不以旧 runtime、fallback 或 compatibility adapter 交付。

## 依赖顺序

```text
T4 -> T5 -> T6 -> T7 -> T8
T7 + T8 -> T9
T6 + T7 + T8 -> T10
T7 + T8 + T10 -> T11
T9 + T11 -> T12
T7 + T8 + T9 + T10 -> T13/T14/T17/T18/T19
T7 + T10 + T12 -> T15
T7 + T9 + T10 + T12 -> T16
T7 + T8 + T9 + T10 + T11 + T13 -> T20
T7 + T8 + T9 + T10 + T11 -> T21
T6 + T9 + T10 + T11 + T12 + T13 + T14..T21 -> T22 -> T23
T11..T23 -> T24 -> T25 -> T26 -> T27
```

T9 与 T10 可在 T7+T8 后并行；T13 不阻塞 T12。真实依赖不得跳过。

## T6–T10：安全 substrate

- **T6**：实现 `~/.mcode` strict schema/path/vault。eager 仅 root 与 `plugins/`；唯一 credential file 为 Host-only `plugins/.host/auth.json`，支持 strict envelope、CAS、ACL/mode、durability、redaction，尚无可注入 entry。只迁移非 secret config。
- **T7**：冻结 no-WASI Manager/FeaturePack/ProviderPack 三个 ABI/golden，family DTO 与 Host-only `ModelRouteLease`/`UsageSample` substrate；无 generic JSON、secret、socket、任意 URL、reserved header 或 raw handle。
- **T8**：只加载 12 个 Manager；Pack 仅由匹配 Manager 经 typed service 激活，完成 generation、cancel、RAII waiting 和 quiescence。
- **T9**：交付 `session` Manager、SessionPack Service 与 Pack；SessionPack 定义 durable event/branch/resume/rewind/recovery，Host 只提供隔离 durable storage。
- **T10**：Manager/Pack 独立 namespace/pointer、共同 signed bundle/source trust/high-water/WAL；multi-active 分项提交、singleton 原子切换。credential contract diff 只触发目标 rebind。

## T11–T13：核心产品

- **T11**：交付 Providers Manager、Pi Pack、Synthetic Provider Pack。Pi 固定 `0.84.4`、签名 snapshot、10-wire bounded codec；接通 vault。Provider 可用性要求 Manager、签名 Pack、snapshot/cache、Host adapter 与 matched credential binding 全部有效。
- **T12**：交付 TUI、UI Manager、一个 product UiPack 和 Theme-role Packs。Host 独占 terminal safety、focus/input、paste/IME、sanitization；image/true-color/hyperlinks 支持 `Auto|ForceOn|ForceOff`，root 显式 override 优先；输出保持 UTF-8 boundary 且每块 `<=1 MiB`。
- **T13**：交付 Workspace checkpoint/rollback；Host service 覆盖 bounded workspace operation、no-follow handles、conflict 与 rollback fence。

## T14–T21：其余 family

- **T14–T16**：Resources、Ask、Todo；Ask 使用 generic Host interaction，Todo 使用 stable ID、依赖、状态机和 durable Session event。
- **T17**：Web，先 Querit 后 Synthetic；Web singleton，二者互斥，不得 cross-Pack fallback。
- **T18**：MCP；**T19**：Usage，先 Host accounting Pack，再 Synthetic Usage Pack，按 unique canonical source 的 `N` Pack 组合固定 usage slots。
- **T20**：Subagents；**T21**：Compaction singleton，先 cancel/drain 再原子切换。

## T22–T27：收口

- **T22**：export/import 包含 composition、12 Manager 与全部 Packs；vault 只能经 Broker typed flow 并重验 consumer signer/destination，Session 只能经 SessionPack typed flow，排除 cache/log/temp。
- **T23**：Core 与 Manager/Pack updater 独立，使用 signed platform artifact、channel trust、高水位与 crash-safe switch。
- **T24**：提供基于 Broker/Providers/Session typed services 的 headless account/provider/model 执行与恢复；secret 只经 stdin/anonymous pipe。
- **T25–T26**：删除旧路径和临时产物；只记录最终行为与验证事实，覆盖 12 Manager、nested Packs、credential、TUI/headless 与更新。
- **T27**：Windows/Linux/macOS 原生安全、离线、crash、redaction、singleton、install/update/rollback/e2e 门禁全部通过后才发布。

## Minimax CN smoke

T11 后才允许显式 opt-in 的本地 smoke；默认 skip，不进 CI。路径固定为 Providers Manager → Host ProviderPack Service → 签名 Pi generation → `minimax-cn` → `anthropic-messages`，endpoint 为 `https://api.minimaxi.com/anthropic/v1/messages`、auth slot 为 `X-Api-Key`。最多两次有界请求，仅观察 method/URL、header presence、payload shape/count、events、provenance、exactly-one terminal 与清理。`minimax.txt` 永远不得读取、打印、复制或 stage。
