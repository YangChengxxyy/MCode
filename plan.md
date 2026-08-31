# MCode 交付计划

只记录未完成工作与必须保持的边界。当前分支为 `main`。

## 当前检查点：T8 架构收缩

- [ ] 删除 12 个顶层 Manager、内置 family 的 FeaturePack/Manager JSON 路径及其重复 activation/gateway。
- [ ] 建立一套第一方 typed task runtime，统一 deadline、cancel、reload、generation、限额和回收。
- [ ] 只保留外部 Pack/asset ABI，完成 T8 integration audit 后进入 T9。

## 产品与扩展边界

第一方内置：Session、Compaction、Resources、Ask、Todo、Subagents、Workspace、默认 UI。它们与 Core/Host 同版本交付，直接使用 Rust typed API，不经过 Wasm Manager、JSON task wire、Pack discovery 或独立安装。

| 外部扩展 | 安装数 | 同时激活 | 说明 |
| --- | ---: | ---: | --- |
| Provider Packs | N | N | provider/model identity 全局唯一 |
| Web Packs | N | 0..1 | Querit 与 Synthetic Web 互斥，无 fallback |
| MCP Packs | N | N | 每个 Pack 可挂 N 个 server/tool；identity 全局唯一 |
| Usage Packs | N | N | 按 canonical source identity 隔离 |
| Theme assets | N | 0..1 | versioned declarative schema，不执行代码 |
| Wallpaper assets | N | 0..1 | signed asset/hash 与 fit/position/opacity/blur/tint |

第一方和第三方外部 Pack 使用同一签名、安装、更新、限额、generation fence 和故障隔离路径。Pack 无 WASI，不取得任意 filesystem/network/process/socket/credential authority；Host 独占 transport、secret、storage、process 和 workspace handle。

外部实现硬边界：

- Provider：Pi importer 必须可重复并对未知上游变化 fail closed；Synthetic 固定 `POST https://api.synthetic.new/v1/chat/completions`，保留 requested/returned model provenance。
- Web：Querit 固定 `/v1/search` 与 `/v1/contents`，query/count/fetch/body/deadline 全部有界；Synthetic 固定 `/v2/search` 且不提供 fetch fallback。URL、redirect、DNS/IP、credential 和 remote-text sanitization 归 Host。
- MCP：stdio/HTTP command/origin/auth/config 进入 signed binding；Host 独占进程、网络、重连、cancel/drain 和 backpressure。
- Usage：外部 quota snapshot 与 Host accounting 分离；Pack 不查询 Provider、不猜当前模型、不直接读取 credential source。
- UI/asset：Host 独占 terminal/input/IME/clipboard safety；Theme/Wallpaper 只能声明样式与签名资产，不能执行代码。

## 实现参考

实现前审读本仓库 WIT/goldens/design、用户 GitHub 与下列锁定源码；只迁移行为、状态机和测试，不迁移本机 authority、monkey patch、动态加载或配置写入。

- 内置能力行为基线：Session、Compaction、Resources、Ask、Todo、Subagents、Workspace 和默认 UI 全面对齐 Codex/Grok 的公开优秀实践；旧 MCode 产品行为不再作为参考，只复用已验证的安全、持久化和 generation primitives。
- 通用行为：durable goal、resume/fork/rewind/compact、明确 approval boundary、可观察的 streaming tool events、typed output/citation provenance，以及只对独立工作并行 fan-out；实现保持 MCode-owned。
- Ask/Todo：`juicesharp/rpiv-mono@d13677c` 的 `rpiv-ask-user-question`、`rpiv-todo`；保留 1..4 questions、typed answers、preview、abandon、可见 todo 状态、dependency/replay 语义。
- Usage：`marckrenn/pi-sub@65deb56`；复用 source/display 分层、缓存快照与 quota windows，不读取其他工具的 `auth.json` 或环境凭据。目标覆盖 DeepSeek、OpenAI、Synthetic、Kimi、Z.AI/GLM 等真实可验证 source adapter。
- UI：`pi-droid-styling@902b06e`、`pi-themes@cde2ff4`；首个 `mcode-default` 使用 Droid conversation、Gemini user zone 与 auto input，Theme/Wallpaper 保持独立 schema。
- Web：`dsh-web-querit`、`pi-querit-search`、`pi-web-access`。
- Subagents：以 Codex/Grok 的异步委派模型为主，吸收用户现有 GitHub/本地实现与 `pi-subagents` 中可证明可靠的队列、worktree 和恢复机制；父 agent 不以同步 wait 驱动正常进度。

## 后续 TODO

- [ ] T9：内置 Session，event-sourced branch/resume/rewind、durable ledger/WAL、replay/recovery。
- [ ] T10：外部 Pack 与 asset 的签名安装、更新、回滚和 crash-safe WAL；bundle 不执行 build/npm/Git hook。
- [ ] T11：Provider runtime、Pi/Synthetic Packs；补 OpenAI、DeepSeek、Kimi、Z.AI/GLM 等 canonical adapters。
- [ ] T12：默认 TUI、内置 UI runtime、generic login、Theme/Wallpaper schema 与选择。
- [ ] T13：内置 Workspace checkpoint/rollback；no-follow handle、并发冲突和不可回滚证据。
- [ ] T14：内置 Resources catalog/read/render-prompt/contributions 与 Host-owned large payload sidecar。
- [ ] T15：内置 Ask interaction、typed progress/result、cancel-safe Host wait。
- [ ] T16：内置 Todo stable ID、dependency graph、revision/CAS 与 durable task event。
- [ ] T17：Web runtime、Querit/Synthetic Packs；bounded reader、URL/SSRF、provenance 与 sanitization。
- [ ] T18：MCP multi-Pack composite catalog、owner routing、transport binding 与 replacement fence。
- [ ] T19：Usage multi-source sampling、accounting、quota/status snapshots 与 UI widgets。
- [ ] T20：内置 Subagents async fan-out、bounded queue、steer/follow-up/cancel、worktree lease 与 crash recovery。
- [ ] T21：内置 Compaction adaptive scheduling、Provider child completion 与 atomic checkpoint。
- [ ] T22：产品 export/import。
- [ ] T23：Core 自动更新。
- [ ] T24：最终产品组合与 headless CLI。
- [ ] T25：删除旧路径的识别、读取、兼容代码和 dead code。
- [ ] T26：最终文档与扩展指南。
- [ ] T27：Windows/Linux/macOS 安全、offline/crash、redaction 与 e2e 门禁。
- [ ] final：workspace 全量 audit/cleanup、三平台 CI、secret/provenance/release review，发布 `v0.0.1`。

依赖主线：`T8 -> T9 -> T12`；T10 在 T11、T17–T19 前完成。

## 开发门禁

- 一个完整 feature 一个 commit/push；`plan.md` 状态清理独立提交。
- 先 targeted，再跑相关 format/lint/build/test；提交前审阅 exact staged diff。
- `minimax.txt` 永远不得读取、打印、复制、修改或 stage。
