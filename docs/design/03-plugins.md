# Manager、Pack 与 Host substrate

> 本文冻结 Pack 的目录、权威性、cardinality 与 credential contract。Manager、Service、world 和目录规则不因本文而成为已实现能力。

## 1. 顶层 registry 与 nested Pack

`plugins.json` 只登记 **恰好 12 个** MCode-owned top-level Manager family：`providers`、`session`、`compaction`、`resources`、`ask`、`todo`、`web`、`mcp`、`usage`、`subagents`、`workspace`、`ui`。它是各 Manager 的 enablement、source、active version+hash、trust high-water 的唯一权威；Pack 永不进入其中。

未知或第三方顶层 ID、Manager、family、Host service 和 `com.mcode.*` identity 均拒绝。第三方只能实现已发布 family 的签名 nested Pack；新 family 必须先由 MCode 定义 Manager、typed service、ABI/golden 和保留 ID。第一方 Manager/Pack 的唯一源码、构建、发布仓库是 `https://github.com/MCapricorns/MCode_plugins`，路径为 `plugins/<feature>/manager/` 与 `plugins/<feature>/packs/<pack-id>/`。

Core/Host 只 discovery、验证、装载 Manager。Manager 是自身 `packs/<pack-id>/` 的唯一 discovery、验证、选择、加载、配置、状态与 UI 请求方，并只能调用对应 typed Host Pack Service；Host 不扫描或直接加载 Pack。Manager/Pack 均无 WASI、OS、filesystem、network、process、terminal、socket、credential 或 raw Host handle。

| family | active cardinality |
| --- | --- |
| `providers` | `N` |
| `usage` | canonical source key 唯一的 `N` |
| `ui` | 一个 product UiPack + `N` 个 Theme-role Pack |
| `session`、`compaction`、`resources`、`ask`、`todo`、`web`、`mcp`、`subagents`、`workspace` | Host-wide `0..1` singleton |

同一 Pack identity 同时只能有一个 generation。ID/source/route/auth slot/source key/role 冲突、旧 generation 未排空或 singleton 多选均 fail closed；不得按名称、安装/加载顺序或隐式 priority 决胜。

## 2. 用户目录与权威文件

```text
~/.mcode/
├─ config.json
├─ plugins.json
└─ plugins/
   ├─ .host/auth.json
   ├─ .staging.lock
   ├─ .staging/
   │  └─ tx1-<32 lowercase hex>/
   │     ├─ transaction.lock
   │     ├─ journal.json
   │     └─ payload/
   └─ <feature>/
      ├─ manager/{config.json,installation.json,data/,versions/<semver>/}
      └─ packs/<pack-id>/{installation.json,data/,versions/<pack-version>/}
```

eager 仅创建 `~/.mcode/` 与 `~/.mcode/plugins/`。`.host/`、`auth.json`、`.staging.lock`、`.staging/`、feature、manager、packs、data 和 versions 均由可信操作 lazy 创建。`.host` 是保留 Host-only namespace，不是第 13 个 family；`.staging` 是 Host-only、no-follow、owned、同卷的未信任 payload substrate，永不 discovery/export，也不得保存 credential。

Transaction ID 只能由 Host OS CSPRNG 生成 128 bits，并精确编码为 `tx1-[0-9a-f]{32}`；公开 API 不接受任意字符串 ID。`journal.json` 上限 `1 KiB`，canonical v1 是紧凑 UTF-8 JSON 加一个 LF，且恰好包含 `formatVersion=1`、`kind=mcode-staging-transaction`、与目录名相同的 `transactionId`、以及 `state`。T6 只写 `writing|staged`；T10 独占 `committing|committed` 与 `commit/wal.json`。journal 不是 WAL，不得携带 target、digest、signature、trust 或 rollback 数据。

writing payload 允许 `0..4096` 个、staged payload 要求 `1..4096` 个 link-count-one regular file；目录最多 `4096` 个，file+directory 合计最多 `8192` 个条目；单文件最多 `256 MiB`、总量最多 `512 MiB`，以 checked `u64` 计数；路径复用 `BundlePath` 的 `512` bytes/`128` components/每 component `128` bytes lowercase portable grammar。`.staging/` 最多扫描 `1024` 个直属条目，超过时整次恢复零删除。link/reparse/hardlink alias/mount/cross-device/special file 均拒绝。

锁序固定为 blocking `.staging.lock` → nonblocking `transaction.lock`；transaction guard 持锁到 staged payload 被 T10 接管或被放弃。创建方 durable 创建各级目录与 lock，并以 canonical private temp → temp flush → atomic replace → published identity/access 验证 → transaction directory flush 的固定流程发布 `writing`，之后才释放 global lock；crash temp 作为未知 entry 保留。payload 以 native handle-relative exclusive create/no-follow 写入并逐文件、逐目录 durable；`staged` journal 也完成同一 post-replace 流程后才可返回 guard。恢复同样只用原生 handle-relative bounded enumeration/deletion，先完整预检再修改。仅 inactive、精确 v1 `writing|staged`、根恰好含 `transaction.lock,journal.json,payload/` 且整棵树 owner-private、同卷、regular、有界的 transaction 可自底向上删除；busy、missing/malformed/future、`committing|committed`、未知 entry、special/cross-device/over-limit/preflight identity race 或 I/O failure 均原样保留，不 quarantine、修复或降级。删除开始后的 native delete/barrier failure 必须返回 indeterminate failure、停止整次恢复、保留任何仍存在的 residue，且不得伪报 clean；final-parent barrier failure 可以没有可见 residue。缺失 `.staging` 时恢复不得创建任何 staging 对象；存在 `.staging` 却缺失或无法验证既存 `.staging.lock` 时整次恢复零修改。禁止 `read_dir`、`remove_dir_all` 或 path-based recursion。

`staged` 只表示未信任 bytes mechanically durable/private/same-volume/bounded，不表示 signed inventory complete、digest/signature/source/trust verified 或 active。T10 独占 durable claim、验签、trust/high-water、安装、激活、回滚、WAL 与 committed recovery；T10 的 `commit/` 一旦出现，T6 即不得删除。持有 transaction/WAL/authority lock 时禁止反向获取 global lock；T10 后续按 transaction → coordinator/WAL → canonical path byte order authority locks 取锁。

Manager `installation.json` 是 Host receipt；Pack `installation.json` 才是其 source、selected version+hash、trust high-water、inventory 的唯一权威。Manager `config.json` 只保存有界非敏感偏好。根 `config.json` 只保存 Host composition：默认 provider/model、Providers/Usage 有序 active sets、一个 UI runtime、Theme set 和其余 singleton；未知 family/role、重复 ID/source、隐式 default 与 singleton 多选均拒绝。Usage 顺序就是 widget row/card 顺序。

不存在顶层 `auth.json`、`credentials.json`、`models.json`、`settings.json` 或 `--profile` Provider 定义。项目 `.mcode` 仅可在 trusted 后作为 bounded config layer，不能 discovery/install 插件或覆盖 enablement/source/trust、Pack selection/routing、endpoint/auth destination 或 credential。冻结旧路径不迁移、不兼容读取、不回退；只删除代码库中的可执行识别、读取与兼容路径。磁盘上既存的旧 artifact 位于产品边界之外，永不读取、迁移或删除；禁止递归清理旧根，且不触碰 legacy secret、未知用户数据或当前插件状态。

## 3. Credential contract 与网络 authority

唯一 credential authority 是首次登录时 lazy 创建、仅 Rust Host 可访问的 `~/.mcode/plugins/.host/auth.json`。它保存严格 envelope、credentials、grants 与每个 canonical service/account 一份 secret，并以 `credentialVersion` CAS 更新；Manager 或 Pack 不拥有 credential。

每个签名 Provider/Web/Usage Pack **必须**声明精确 canonical service/account/issuer/auth schema、trusted signer/source、signed credential-contract version 和 `operation + method + origin + path + auth slot`。安装/激活批准该精确 authority。Host 自动为每个 active Provider/Web/Usage Pack 匹配 vault account；只有全部字段与批准 contract 精确一致时复用，因而不重复输入 key 或逐 Pack login。新的或不匹配的 signer、credential-contract、origin、scheme 或 destination 一律 fail closed/rebind。

Host 以 account/version、consumer family、Manager/Pack identity/version/hash/generation、provider/source、signer/source、contract、operation、精确 request target 和 auth destination 推导单次 generation-bound injection lease。Pack 只能构造有界非敏感 request、解析有界 response，不能读取 secret/grant、借用 account/operation、设置 reserved auth headers 或扩展 endpoint/auth destination。Host 独占 HTTP/TLS/DNS/proxy、redirect、timeout/retry/cancel/backpressure、credential insertion、redaction、allowlist、generation 与审计。

## 4. 固定 Pack 契约

- Pi Provider Pack 固定 `@earendil-works/pi-coding-agent`/`pi-ai` `0.84.4`；仅从签名 snapshot 枚举 provider，bounded metadata 不得新增 provider/endpoint/auth/wire/header。
- Querit 是 `web` family singleton，固定 `https://api.querit.ai`、Bearer、`POST /v1/search` 和 `POST /v1/contents`；不得实现 DeepSeek-backed search。
- Synthetic Provider、Web、Usage 是独立 Pack，共享 canonical `synthetic/<account-id>`。Web 与 Querit 互斥；Synthetic Web 无 `fetch_content`。Synthetic Usage source key 为 `provider:synthetic`，且不覆盖 Host 当前模型。

详见 [05-plugin-impl.md](05-plugin-impl.md) 的 ABI、route/usage 和生命周期约束。
