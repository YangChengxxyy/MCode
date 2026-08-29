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
   ├─ .staging/<transaction-id>/
   └─ <feature>/
      ├─ manager/{config.json,installation.json,data/,versions/<semver>/}
      └─ packs/<pack-id>/{installation.json,data/,versions/<pack-version>/}
```

eager 仅创建 `~/.mcode/` 与 `~/.mcode/plugins/`。`.host/`、`auth.json`、`.staging/`、feature、manager、packs、data 和 versions 均由可信操作 lazy 创建。`.host` 是保留 Host-only namespace，不是第 13 个 family；`.staging` 是 no-follow、owned、同卷事务目录，恢复后删除，永不 discovery/export。

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
