# MCode 交付计划

只记录未完成工作、依赖和必须保持的边界。当前交付树为 `D:/my_private_pro/MCode`，分支为 `main`。

## 当前检查点：T8

- [ ] 完成 Resources runtime sentinel：exact DTO/validation、Pack worker、Host task table、deadline/cancel/stale/terminal、generation replacement 与真实 guest 集成测试。
- [ ] 完成其余 family-specific Pack invoke/pull 接线，并统一复用 task/lifecycle 边界。
- [ ] 完成 T8 integration audit 与相关门禁，随后进入 T9。

T8 本次 sentinel 只验证通用 runtime/ownership/task 机制。Resources 跨页一致性、真实资源 UTF-8/EOF、prompt 参数关联等完整 reducer 留在 T14。

## 激活数量

| 类型 | 同时生效数量 |
| --- | ---: |
| Provider Packs | N |
| MCP Packs | N；server/tool identity 全局唯一 |
| Usage Packs | N；按 source identity 隔离 |
| UI runtime | 0..1 |
| Theme / Wallpaper | 0..1；可安装多个候选 |
| Session / Compaction / Resources / Ask / Todo / Web / Subagents / Workspace | 0..1 |

所有激活都绑定 exact Manager generation、configured revision 与 canonical digest。替换必须先停止准入、取消任务并排空旧 generation；trap、timeout 或 future-drop 后不得复用失效实例。

## 插件实现来源与通用边界

- 实现每个插件前，先审读本仓库现有代码、WIT/goldens/design docs，以及用户 GitHub 中对应实现和历史；列出可复用行为、边界与测试，再开始编码。明确参考源包括 `MCode`、`MCode_plugins`、`pi-subagents`、`dsh-web-querit`、`pi-querit-search`、`pi-web-access`；已有逻辑迁移到 canonical Manager/Pack/Host 分层，不能凭空重写或只照第三方 README 猜行为。`MCode_plugins` 当前为空，旧 TypeScript 只作为行为与测试参考，不视为现成 Wasm Pack。
- 第一方插件源码与发布目标是 `MCode_plugins/plugins/<family>/{manager/,packs/<pack-id>/}`；第一方和第三方 Pack 使用同一签名、安装、generation、限额和故障隔离路径，无内置捷径。
- 顶层只允许固定 12 个 MCode-owned Manager。Host 不扫描 Pack；Manager 独占本 family 的 discovery、选择和配置，Host 独占 secure loading、typed service、secret、网络、进程与文件系统 authority。
- Manager/Pack 无 WASI、任意 filesystem/network/process/socket/credential/raw Host handle。所有输入、输出、队列、并发、fuel、deadline 和重试有界；流严格 `pending/progress -> exactly one terminal`。
- Manager gateway 只接受绑定 caller family/generation 的 declared operation；Pack 使用 family-specific typed `invoke/pull`。cancel、reload、timeout、trap、future-drop 必须回收任务并使损坏实例不可复用。
- Provider/Web/Usage 的 endpoint、method、origin、path、auth slot 和 credential contract 来自 signed binding；Host 独占 HTTP/TLS/DNS/proxy、redirect、retry、timeout、credential 注入、reserved headers、redaction 与 provenance。

## 未完成插件要求

### Session、安装链与 Provider

- **T9 Session**：event-sourced branch/resume/rewind、ledger、replay/recovery 由 Session Pack 拥有；Host 只提供 identity-isolated durable storage/WAL、bounds、backpressure 与 generation fence。tool result 先 durable，再进入消息流；失败不得回退 Core memory/JSONL。
- **T10 安装链**：Manager 与 Pack 使用独立 namespace/pointer，共用 signed bundle、source trust、高水位和 crash-safe WAL；支持原子安装、更新、回滚和恢复，不在用户机器执行 bundle 内 build/npm/Git hook。
- **T11 Providers**：先实现 Providers Manager、Pi Pack、Synthetic Pack。Provider 可用性必须同时满足 Manager active、signed Pack active、valid snapshot/cache、Host adapter supported 和 credential binding matched。
- **Pi Pack**：从用户现有代码/锁定 snapshot 生成可重复 importer、alignment、manifest、machine-readable diff 与 goldens；覆盖 10 种 wire、40 个 text Provider、模型 catalog、header/body/stream/error、tool choice、reasoning 和 fragmented tool-call。未知上游变化 fail closed，不能静默 fallback。
- **Synthetic Provider**：固定 `POST https://api.synthetic.new/v1/chat/completions` + Bearer；只暴露锁定的四个 text/vision alias，保留 requested/returned model provenance，vision 只接收 Host 验证且总量有界的图片 bytes。

### UI 与 Workspace

- **T12 UI**：Host 独占 terminal safety、focus/input、paste/IME、sanitization、clipboard capability；UI Manager + runtime Pack 提供产品 UI。Theme/Wallpaper 可安装多个候选，但同时各只生效一个；不能执行代码或获取额外 authority。
- terminal image/true-color/hyperlinks 各为 `Auto|ForceOn|ForceOff`；write 分块 `<=1 MiB` 且保持 UTF-8 boundary，远端文本移除 control/bidi，诊断不记录原文。
- T12 先统一 root schema、Pack role、selection projection 与 docs 的 Theme 基数，并定义 declarative theme inventory；现有 dark/light palettes 迁入第一方 Theme Pack，不作为 Core fallback。Wallpaper 另行定义选择字段、signed image stamp/bytes、resize/crop、z-order 和 capability-off 行为；不得借此获得 filesystem/terminal authority。
- **T13 Workspace**：typed Host service 覆盖 tracked/untracked/ignored、删除、metadata、hash、限额、并发冲突与 no-follow handle。不能证明范围的 exec/shell 标为不可回滚；rollback 不覆盖并发修改。

### Resources、Ask 与 Todo

- **T14 Resources**：实现 catalog/lookup/read/render-prompt/UI contribution 的完整 stateful reducer；验证跨页 total/skip/replay/global ID、真实 UTF-8/EOF、prompt 参数声明和 generation 一致性。
- **T15 Ask**：只提供 generic Host interaction 与 typed progress/result，不恢复 Core authorization/grant。
- **T16 Todo**：stable task ID、依赖图、状态机和 Todo-local durable task event；并发更新必须 revision/CAS-bound。

### Web 搜索

- **T17 Web**：先 Querit Pack，后 Synthetic Web Pack；Web 为 singleton，二者互斥，无 cross-Pack fallback。搜索与正文抓取只经 Web Manager -> typed Web service -> selected Pack，UI/Core 不保留第二条 direct search 通道。
- **Querit**：固定 `https://api.querit.ai`、Bearer、`POST /v1/search` 与 `POST /v1/contents`，不得实现 DeepSeek-backed search。query 为 `1..1000` UTF-8 bytes，count `1..20`；fetch 对 canonical URL 去重后为 `1..10`，拒绝 embedded credential，format 仅 `markdown|text|html`，每页 timeout `1..60s`。
- Querit 只接受 bounded date/content/chunks/country/language/domain filters；search/content/error response 上限分别为 `2 MiB/10 MiB/8 KiB`，operation deadline `<=70s`，model-visible 输出 `<=50 KiB/2000 lines`。保留 source/search ID/truncation provenance、请求顺序和每页截断状态。
- 从 `dsh-web-querit`、`pi-querit-search`、`pi-web-access` 迁移 bounded reader、sanitizer、URL/SSRF 与格式测试；冻结 `chunks=0`、include/exclude domains、language/country 映射、partial contents 同序补全和 remote provenance 投影。credential/config 只在下一 operation 生效，当前 operation 使用 immutable snapshot。
- URL、redirect、DNS/IP 和 same-origin 由 Host 校验；远端内容始终标记 untrusted 并移除 terminal control/bidi。不得从 environment 取 key、把全文写普通 OS temp、透传远端原始错误或在 Pack 内自行联网。
- **Synthetic Web**：固定 `POST https://api.synthetic.new/v2/search`，仅 bounded query，严格解析 `results[{url,title,text,published}]`；`fetch_content` 明确 unavailable，不回落 Querit。

### MCP、Usage、Subagents 与 Compaction

- **T18 MCP**：MCP Manager 可同时激活 N 个 MCP Packs，每个 Pack 可挂载 N 个 server/tool 子项；渐进披露 catalog，server/tool identity 全局稳定且唯一，冲突 fail closed。先把 root composition、selection projection 和 docs 从 singleton 迁为 multi-Pack，再实现 composite snapshot、owner routing 与 replacement fence。
- MCP 的 stdio/HTTP 等 transport、command/origin/auth/config 必须进入 signed server binding；Host 独占启停、重连、process/network/credential、cancel/drain、backpressure 和诊断，Pack 不自行解释或取得这些 authority。
- **T19 Usage**：按 canonical source identity 激活 N 个 Packs；Host accounting 与外部 quota 独立，事件为 immutable generation-stamped samples。Pack 不查询 Provider、不猜当前模型；固定 semantic widget slots，由 UI runtime 布局。
- **Synthetic Usage**：固定 `GET https://api.synthetic.new/v2/quotas`，source key `provider:synthetic`；与 Synthetic Provider/Web 共享 canonical account credential，但独立批准 authority，quota 不覆盖 Host 当前模型。
- **T20 Subagents**：先学习用户现有 GitHub/本地 subagent 实现，再迁移 roles、异步 fan-out、bounded parallel queue、状态查询、steer/follow-up/cancel、retained session、review/fix loop 和 crash recovery。父 agent 启动子任务后继续工作，完成结果异步回流；不得靠同步 wait 驱动正常进度。
- 写任务默认使用隔离 worktree，绑定 base commit/ref、workspace lease 与 cleanup policy；隔离创建失败必须明确失败，不能静默回到共享树。队列、attempt/review rounds、输出和恢复均有界；stale job、不可恢复 crash、queue full、cancel 使用 typed terminal。
- 从 `pi-subagents` 迁移 `background/thread-lifecycle/runtime/worktree/durable/recovery` 的队列、CAS、自动唤醒、集成与恢复测试。现有 WIT 只有 `roles|enqueue|recover`，T20 必须补 typed status/steer/follow-up/resume/cancel control；completion exactly once，child 默认 leaf 且不继承 subagent 管理 authority，冲突与 cleanup failure 保留可恢复证据。
- **T21 Compaction**：singleton adaptive Pack；先 cancel/drain 再原子切换。每个 durable tool result 后、下一次 Provider 请求前重新估算；summary 只有完整成功才能 checkpoint，partial text、tool call、`length/error/cancel` 均失败。

### 收口

- **T22–T24**：完成 composition/Manager/Packs export-import、Core 与插件独立更新器、基于 Broker/Providers/Session typed services 的 headless CLI；secret 只走 Broker + stdin/anonymous pipe。
- **T25–T27**：删除旧路径可执行识别/读取/兼容代码，完成最终文档与 Windows/Linux/macOS 安全、发布、offline/crash、redaction 和 e2e 门禁。

## 后续 TODO

- [ ] T9：Session Manager/Pack 与 Host durable service。
- [ ] T10：签名安装、更新、回滚与 crash-safe WAL。
- [ ] T11：Providers Manager、Pi Pack 与 Synthetic Provider Pack。
- [ ] T12：TUI、UI Manager/Pack 与 generic login flow。
- [ ] T13：Workspace Manager/Pack、checkpoint 与 rollback。
- [ ] T14：Resources Manager/Pack 与完整 stateful reducer。
- [ ] T15：Ask Manager/Pack。
- [ ] T16：Todo Manager/Pack。
- [ ] T17：Web Manager、Querit Pack 与 Synthetic Web Pack。
- [ ] T18：MCP Manager/Pack 与多 Pack 聚合。
- [ ] T19：Usage Manager、Host accounting 与 Usage Packs/widgets。
- [ ] T20：Subagents Manager/Pack。
- [ ] T21：singleton Compaction Manager/Pack。
- [ ] T22：产品 export/import。
- [ ] T23：Core 自动更新。
- [ ] T24：最终产品组合与 headless CLI。
- [ ] T25：删除旧路径的可执行识别、读取与兼容代码。
- [ ] T26：最终文档。
- [ ] T27：Windows/Linux/macOS 安全、发布与 e2e 门禁。
- [ ] final：全项目 audit/cleanup、插件指南、release review，发布 `v0.0.1`。

依赖主线：`T8 -> T9 -> T12`；`T10` 在 T11–T21 前完成；T9–T21 完成后进入 T22–T27 与 final。

## 开发门禁

- 一个完整 feature 一个 commit/push；`plan.md` 状态清理可独立提交。
- 先跑 targeted check，再跑该 feature 相关 format/lint/build/test；提交前审阅 exact staged diff。
- final 前执行 workspace 全量门禁、dead-code/旧路径清理、三平台 CI/e2e 与 secret/provenance/release audit。
- `minimax.txt` 永远不得读取、打印、复制、修改或 stage。
