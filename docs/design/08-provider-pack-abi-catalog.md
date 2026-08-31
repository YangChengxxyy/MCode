# ProviderPack ABI: catalog and selection

> 返回 [08-provider-pack-abi.md](08-provider-pack-abi.md)。本文件保留原 authority 的章节编号与规范效力。

## 3. Catalog source, metadata, cache and refresh

### 3.1 Catalog source view

`descriptor-request` 与 `catalog-request` 都带同一 manifest-fixed `catalog-source-view`：`embedded` 或 `verified(catalog-metadata-view)`。manifest 要求 `verified` 而 Host 无法取得已验证 metadata 时，activation 直接失败，不改用 embedded。offline verified flow 只能使用 attempt 前已通过 current schema、digest、revision 与 binding validation 的 cache；没有该 cache 就失败。`embedded` 从 descriptor 到 pagination 全程 zero-network，不发起 fetch。这个 view 是 bounded data，不是 fetch authority；Pack 不提供 URL/template/path。

`descriptor` 返回一个 route 的 final `catalog-digest` 与 `model-count`。每个 `catalog` page 必须使用相同 source revision/digest。descriptor 与 auth-interaction 永远 zero network；catalog 的 network/cache flow 只由 Host 执行。

`catalog-metadata-view` 的 exact fields 是 `revision:catalog-revision` 与 `entries:list<catalog-metadata-entry>`，entries `0..=4,096`，按 selection 唯一排序。`revision` 必须是 Host 从已验证 network response 或 current cache 构造并 sealed 的 exact closed record，Pack 不能改写。`catalog-metadata-entry` 的字段为：

| field | exact type and meaning |
| --- | --- |
| `selection` | `model-selection`；metadata key |
| `display-name` | `option<string>` |
| `input-modalities` | `list<input-modality>` |
| `tool-capability` | `tool-capability` |
| `reasoning-capability` | `reasoning-capability` |
| `context-tokens` | `option<u64>` |
| `max-output-tokens` | `option<u64>` |

metadata 是该 revision 对这些 typed fields 的 complete replacement；缺失就是 `None` 或 `unknown`，不会回退 embedded value。Host reject selection 不在 signed Pack snapshot、count mismatch，以及任何 provider/current-model/operation/endpoint/auth/adapter/header field。没有 generic metadata map。

`provider-descriptor` 固定包含 provider/route comparison values、final `catalog-digest` 与 exact `model-count`：embedded source 的 `source-revision` 必须为 `None`；verified descriptor 与每个 verified `catalog-page` 必须回显 `Some`，且逐 field 等于 sealed `catalog-metadata-view.revision` record。`catalog-entry` 使用第 2.3 节的 exact field order，metadata 不得提供其中的 `current-model` 或 `completion-operation`。`catalog-page` 固定回显 provider、route、source revision、digest、declared count 与 offset，再返回 entries 和 `next-offset`；这些 comparison values 必须与 descriptor 和 sealed snapshot 完全一致。capability 使用 `capability-support` records，不能改回 list、flag 或 open metadata。

### 3.2 Host-only catalog fetch and cache rules

Host 唯一允许的 Pi catalog fetch 是：在 provider/route/Pack/generation/catalog-source binding 完成后，原始 `catalog` control operation 下，精确 `GET https://pi.dev/api/models/providers/{ProviderId}`；无 query、credential、compression、redirect；Pack 不产生 URL。此 fetch 只适用于 verified flow；embedded flow 禁止 network。每个 verified network response 必须有且仅有一个 current `Last-Modified` source：单一 field value 必须 strict-parse RFC 9110 IMF-fixdate，拒绝 duplicate/comma-combined value、OWS variant 与全部 obsolete date form；Host 将有效日期规范化为 Unix seconds `u64`，只接受 `1..=i64::MAX`。Host 在读取或分配 body 前先处理唯一 canonical `Content-Length`：值必须逐 byte 匹配 exact ASCII grammar `0|[1-9][0-9]*`，不允许 OWS、sign 或 leading zero；重复、grammar failure、overflow 或声明值 `>2 MiB` 立即拒绝，missing length 才允许 bounded streaming。之后仍将 response 以每 chunk `<=64 KiB` 流式计入 raw aggregate `2 MiB` cap，在每次 aggregate allocation 前拒绝 streamed 2 MiB+1；present length 与最终 streamed byte count 不等也拒绝。任何 `content-encoding` reject。

Host 仅接受 final 2xx JSON；raw network response 使用下述 locked `pi-provider-response-v3` parser，已存在 cache 使用独立的 closed cache-envelope parser。两个 parser 共用 lexical/structural gates：UTF-8、depth `<=32`、node `<=32,768`、bounded Safe strings、duplicate key、trailing content、unknown/missing field、错误 JSON type 与任何 string/number/bool/null coercion 全部 reject，但不能把 Host cache envelope 当作 network wire schema。cache input 也必须在 aggregate allocation 前独立执行 `<=2 MiB` byte cap；不读取其他 schema 或 alternate source。cache path 固定为 `plugins/providers/packs/pi/data/models-store.json`，使用 owned no-follow path、锁和 durable atomic replace；不存原始 date text。无有效 cache 的 offline 状态是明确 failure；network failure/invalid data 只能使用 attempt 前选定的 already-valid current cache，不可使用 partial response 或 alternate source。

### 3.2.1 Raw Pi schema, Host projection and closed cache envelope

Network body 不是 Host envelope；`GET /api/models/providers/{ProviderId}` 的 raw top-level shape 是以 model ID 为 member name、provider model record 为 value 的 JSON object。其 sole-current grammar 是由锁定的 `@earendil-works/pi-coding-agent`/`pi-ai` schema `3` importer 生成的 `pi-provider-response-v3.schema.json`；该 artifact 的 SHA-256 必须进入 signed Pi snapshot，T11 必须把 schema、生成器与一份从冻结 endpoint 捕获的 exact response fixture 一同提交。verified flow 在 artifact 缺失、digest mismatch 或 parser/generator 不一致时不可激活。该 generated closed schema 冻结每个 required/optional member、nested shape、JSON type 与 bound；raw missing/unknown member、错误 type/coercion 都 reject。top-level key 必须逐 byte 等于 record 的 model ID，所有 records 的 provider、wire/API、base URL、header/compat/auth-related fields 只作 signed-snapshot comparison，任一 mutation 都 reject，绝不投影成 metadata 或 authority。

Host 通过与 generated schema 一同签名并计入 digest 的 closed projection table，将每个 raw record 唯一映射到第 3.1 节允许的七个 metadata fields；table 不能由 response、Pack DTO 或 runtime map 扩展。selection 由 top-level model ID 和 signed snapshot 的 exact selection mapping 得出，`providerId` 只取 pre-bound provider，`modelCount` 只取 validated raw map cardinality，`canonicalContentDigest` 只由 Host 在 projection 后计算；network body 不携带或覆盖这四个值，也没有 Host cache revision/time field。projection 前必须证明 raw selection set/count 与 signed snapshot 的 expected metadata set/count 完全一致。

Cache envelope 是 closed JSON object，字段恰为 `formatVersion`、`kind`、`providerId`、`lastModified`、`canonicalContentDigest`、`modelCount`、`entries`，没有其他字段；每个字段都 required。`formatVersion` 必须是 JSON integer exact `1`，`kind` 必须是 JSON string exact `mcode-provider-catalog-cache`。cache schema 没有 `schemaVersion`、HTTP date text 或其他 revision field。

Host projection 与 cache envelope 的 `entries` 共用唯一 closed entry representation；每个 entry 恰有以下 required JSON members，不能省略 nullable member：

| JSON member | exact JSON representation and mapping |
| --- | --- |
| `selection` | closed object `{"tag":"exact","payload":<model-id>}` 或 `{"tag":"alias","payload":<model-alias>}`；object 恰有 `tag,payload`，映射 `model-selection` |
| `displayName` | JSON string `Label(256)` 或 JSON `null`，分别映射 `Some`/`None` |
| `inputModalities` | `1..=3` 个 JSON enum string 的 array，映射 declaration-order `list<input-modality>` |
| `toolCapabilities` | closed object，恰有 `tools,autoChoice,noneChoice,specificChoice` 四个 capability enum string |
| `reasoningCapabilities` | closed object，恰有 `reasoning,effort,budget,proof` 四个 capability enum string |
| `contextTokens` | JSON integer 或 JSON `null`，映射 `option<u64>` |
| `maxOutputTokens` | JSON integer 或 JSON `null`，映射 `option<u64>` |

capability enum string 恰为 `unknown|unsupported|supported`；modality enum string 恰为 `unknown|text|image`。modality 按 WIT declaration order 严格递增且 unique，`unknown` 出现时必须是 sole item。所有 projected/cache JSON unsigned integer token 必须逐 byte 匹配 `0|[1-9][0-9]*`，不接受 sign、fraction、exponent、leading zero 或 quoted number；`modelCount` 范围 `0..=4,096`，non-null context/output 范围 `1..=u64::MAX`，cache `lastModified` 范围 `1..=i64::MAX`。raw record 中不参与 projection 的 number 仍按 generated schema 的 exact grammar/type 验证，不能经 float conversion。`providerId`、selection payload 和 digest 分别使用第 2.3 节的 exact scalar grammar。entries 按 `(selection-tag,payload UTF-8 bytes)` 严格递增，tag `exact=0,alias=1`，selection unique，payload across both tags 也 unique；因此 duplicate、out-of-order、`exact("x")` 与 `alias("x")` coexist 均 reject。array/object 外形、member spelling/case、enum spelling/case 与 null-vs-present 必须 exact。

Raw network identity/authority fields 必须先逐 field 等于 pre-bound provider 与 signed snapshot；projection 的 count 必须同时等于 canonical entries length 与 signed snapshot 的 expected metadata count。cache 的 `providerId` 必须等于同一 pre-bound provider，cache 的 `modelCount` 也必须同时等于 `entries.len` 与 signed count。`canonicalContentDigest` 是 Host-local lowercase `sha256:` 加 SHA-256 over domain-separated typed preimage：exact ASCII `mcode-provider-catalog-content-v1\0`，随后是 bound `provider-id` 与 canonical entries。string 为 `u32be byte-length || UTF-8`，unsigned integer 为 fixed-width big-endian，bool 为 `00|01`，option 为 `00|01` 后接 payload，variant/enum 为本文 fixed zero-based `u8` ordinal，list 为 `u32be count` 加 elements；entry 按上表映射到第 3.1 节 WIT field order 编码。计算明确排除 raw nonprojected fields、cache 自己的 `canonicalContentDigest`、format/kind、`modelCount`、`lastModified` 与 HTTP date text；count 已在 hash 前强制等于 canonical entries length。cache stored digest 必须 byte-equal computed digest，否则 schema validation 失败。

Network response 的 sole revision time 来自已 normalized 的唯一 `Last-Modified` header；raw body 不可能携带或覆盖 time/digest。Host 从 pre-bound provider、validated projection/count、locally computed digest 与 header seconds 构造唯一 in-memory candidate：`catalog-metadata-view{revision:{last-modified:<header-seconds>,canonical-content-digest:<computed>},entries:<mapped entries>}`。接受 candidate 时，Host 用 fixed `formatVersion=1`/`kind`、同一 provider/count/digest/entries 与 header seconds 作为 `lastModified` 构造且只构造上述 cache envelope。Cache 的 sole revision time 来自 `lastModified`，并映射相同 view/record；cache 不从 header 取 revision。raw comparison/projection 与 cache envelope 都逐 field 证明 provider、count 与 computed digest 后才可构造 sealed `catalog-metadata-view`，而 `catalog-revision` 恰为该 view 的 `{last-modified,canonical-content-digest}`，没有第三 revision source。

### 3.2.2 Refresh state transitions

每次 activation/refresh 最多一次 fetch；descriptor 和 pagination 共用一份 sealed snapshot，不 refetch。状态比较必须同时包含 valid current cache revision 与 optional active route/catalog generation revision；磁盘 cache 存在不代表对应 generation 已 publication。已有 valid current cache 时，Host 只将 valid candidate response 与 cache 的 numeric `catalog-revision.last-modified` 比较；仅在 timestamp 相等时比较 `canonical-content-digest`，先得到 effective cache，再执行 generation reconciliation：

| candidate relation to valid cache | effective cache outcome |
| --- | --- |
| `response.last-modified > cache.last-modified` | 接受完整新 snapshot，先 durably persist exact cache envelope/revision，再以新 cache 为 effective cache |
| `response.last-modified == cache.last-modified` 且 content digest equal | byte-preserving cache no-op；existing cache 为 effective cache |
| network unavailable/invalid | 不写 response；attempt 前已验证的 existing cache 为 effective cache |
| `response.last-modified < cache.last-modified` | reject candidate；existing cache 为 effective cache |
| `response.last-modified == cache.last-modified` 且 content digest different | reject candidate；existing cache 为 effective cache |

Host 从第 2.2 节 current pre-bound `identity` 与 `route` 的全部 exact fields、signed catalog snapshot/schema/projection digest、adapter profile/contract/wire/header/transport policy digest，以及 exact `context-counter-ref-v1` tuple/`counter-digest` 构造 expected immutable generation binding；active generation 持有 publication 时 sealed 的同一 binding。对 effective cache 的 generation reconciliation 恰有三种分支：只有 active generation 的 sealed binding byte-equal expected binding 且其 catalog revision byte-equal effective cache revision 时，才是不调用 Pack、不改 generation 的 no-op；active generation 不存在时，从 effective cache 的 sealed snapshot 创建并 publication initial generation；任一 binding field 或 catalog revision 不同时，创建并完整验证 replacement generation，publication 成功后才原子切换并 drain old generation。generation 创建、descriptor validation 或 publication 失败时，不撤销已 durable 的 cache，不切换/销毁 old active generation，也不报告 activation 成功；下一次 activation/refresh 必须从该 cache 重试 reconciliation。因此 cold process start 的 valid offline cache、`persist cache -> crash/failure -> publication` residue，以及收到 equal+equal response 但没有 matching active generation 都必须 publication cache generation，而不是命中 no-op。

没有 current cache 且 network candidate 通过 transport、raw schema/projection、provider/count/digest 和 signed-snapshot verification 时，Host 接受该 candidate，先 durably persist exact cache envelope 及同一 revision，再按上述 `active=None` 分支创建 initial route/catalog generation；此分支没有 old generation 可 drain。只有持久化、descriptor validation 与 initial generation publication 全部成功后，Host 才从该 generation 的 sealed snapshot 提供 descriptor 与 pagination。没有 current cache 且 fetch/network failure 或 candidate invalid 时明确失败：不调用 Pack、不创建 generation、不写 partial cache，也不改用 alternate source。

Host 先验证 provider membership、count/hash/schema，以及 metadata 不能增加 provider/endpoint/auth/wire/header。每次 refresh 的 redirect、compression、duplicate/missing/unknown/trailing/type/coercion、depth/node、declared 与 streamed 各自 2 MiB/2 MiB+1、declared/actual mismatch、cache relation 都是 T11 transport/cache tests；T7 只用 dummy bytes/parser fixture。`Last-Modified` source-count fixtures 使用 0/1/N/N+1 boundaries，覆盖 missing、sole valid 与 duplicate；strict parser fixtures 覆盖 malformed IMF-fixdate 和每种 obsolete date，normalization/cache fixtures 覆盖 zero 与 timestamp overflow (`i64::MAX+1`)。raw-schema fixtures 必须接受冻结 endpoint 的 exact captured provider response，并证明 generated schema/parser digest matching；逐项 mutation unknown/missing/type/coercion 以及 provider、wire/API、base URL、header/compat/auth-related field 都必须拒绝。projection/cache entry/digest fixtures 使用 0/1/N/N+1 并 mutation 每个 field、tag、enum、null 和 integer grammar；refresh fixtures 覆盖 greater、equal+equal、lower、equal+different、network failure/invalid with valid cache、no-cache+valid candidate initial generation、no-cache+network failure 与 no-cache+invalid candidate，以及 cache/response revision mismatch。generation reconciliation fixtures 另覆盖 offline cold start with valid cache、equal+equal without active generation、persist 后 crash/restart、persist 后 publication failure/retry、matching revision+binding active no-op、same catalog revision with changed Pack hash、Pack generation、signed snapshot/profile digest 或 context-counter binding/digest 各自强制 replacement/drain，以及 stale catalog revision active replacement/drain。

## 4. Catalog digest, selection and alias binding

`catalog-digest` 是 lowercase `sha256:` 加 SHA-256 over domain-separated canonical preimage。preimage 起始为 exact ASCII `mcode-provider-catalog-v1\0`；string 为 `u32be byte-length || UTF-8`，unsigned integer 为 fixed-width big-endian，bool 为 `00|01`，option 为 tag `00|01` 后跟 payload，variant 为 declared zero-based `u8` ordinal，list 为 `u32be count` 加 elements。provider ID、route ID、declared count、每个 model entry 都按 WIT field order 编码；model entry 按 byte-sorted selection order 编码。checked length/count overflow 或 noncanonical ordering 在 hash 前 reject。

`model-alias` 是 current catalog model data，不是 ABI alias。`model-selection` 是 `exact(model-id) | alias(model-alias)`。ordering key 是 `(selection-tag,payload-bytes)`，tag `exact=0`、`alias=1`；payload across both variants 必须 unique，所以 `exact("x")` 与 `alias("x")` 不能共存。catalog request 与 returned page 都要求 `offset<=declared-count`；page entries `<=requested limit`、selection-sorted，nonfinal page 必须 nonempty。internal computed offset 为 checked `offset+entries.len` 且不得超过 declared count；`next-offset=Some(computed)` 当且仅当 `computed<declared-count`，此时必须 strictly greater；`computed=declared-count` 的 final page 恰为 `None`。Host-private cursor/request binding 只冻结 caller identity、Manager ID、Pack ID/generation、provider、route、catalog-source variant 及其 sealed revision/canonical content digest（存在时）、catalog digest、requested limit、immutable declared count/snapshot，并保存 current expected offset。首个 validated request 的 offset 建立该 expected offset；accepted nonfinal page 只把它原子推进到 validated `next-offset`，下一页在其余 sealed tuple 不变时必须使用该新 offset。跨任何实际 tuple field、snapshot 或 expected offset 的 replay，以及已消费 offset 的重用都 reject。empty nonfinal page、empty+Some、skip、self-loop、rollback、missing page、count mismatch、digest/generation change 都 reject。

每条 entry 的 alias/current mapping 是 immutable snapshot fact：

| selection | `current-model` | route lease | `UsageContextSnapshot` | `requested-alias` |
| --- | --- | --- | --- | --- |
| `exact(x)` | `x` | `current_model=x` | `requested_model=x` | `None` |
| `alias(a)` -> `m` | `m` | `current_model=m` | `requested_model=m` | `Some(a)` |

`resolved_model` 初始为 `None`，只能由 Host 从 validated decoder value 设置一次，不覆盖 requested fields。`prepare-input` 必须携带 selection、current model、catalog digest 和 `operation-id`；Host 证明该 ID 等于 entry 的 `completion-operation`，并证明 provider、route、selection、current model、modalities、capabilities、limits、operation 来自一份 snapshot。alias target 改变会改变 digest/generation，并使旧 route/usage/proof stamps 失效；不从字符串推断 alias target、operation、capability 或 authority。
