# T7 ProviderPack ABI authority

> 本文冻结 `mcode:provider-pack/provider@0.0.1` 的 current-only 目标契约，不声称 Provider runtime、catalog network、credential binding 或真实 wire 已实现。本文是仓库内可审查的 ProviderPack authority；紧随 T7 交付的 parseable WIT source、current LF golden 与 semantic JSONL golden 必须是其 machine-verifiable projection。
>
> Provider world 只有 zero-import current surface。只解析当前 typed surface；不保留 `abi_v1.json`、historical golden、compatibility parser/adapter、ABI alias、dual-read 或 fallback；没有 guest Host call、URL/socket/credential DTO、raw handle 或 generic JSON escape hatch。所有名称使用英文，说明使用中文。

## 1. Exact zero-import surface and ownership

| declaration | exact signature | ownership and effect |
| --- | --- | --- |
| `descriptor` | `descriptor(request: descriptor-request) -> result<provider-descriptor,provider-error>` | 每个 signed provider/route 调用一次；zero network；value result |
| `catalog` | `catalog(request: catalog-request) -> result<catalog-page,provider-error>` | 使用 Host 已 sealed 的 catalog source view；Pack 不 fetch；value result |
| `auth-interaction` | `auth-interaction(request: auth-interaction-request) -> result<auth-interaction-response,provider-error>` | presentation metadata only；zero network；没有 guest auth-answer |
| `prepare-request` | `prepare-request(input: prepare-input) -> result<prepared-completion,provider-error>` | 返回 prepared typed request 与 `own<response-decoder>`；Host 串行驱动 decoder |
| `response-decoder.pull` | `response-decoder.pull(limit: u8) -> result<decoder-pull,provider-error>` | `limit=1..=16`；Host drain 到 `need-frame` 或 terminal batch |
| `response-decoder.push` | `response-decoder.push(frame: response-frame) -> result<frame-acceptance,provider-error>` | Host 仅在 decoder 要求 frame 后 push；成功值只能是 `accepted` |

`provider` world 的 import set 必须为空，export set 只有 `provider-api` interface；该 interface 只有上述 four freestanding functions 与 `response-decoder` resource。`prepared-completion.decoder` 的 ownership 从 guest 转给 Host；Host owns outer channel、route lease、credential/injection lease 与 closure。decoder drop 只触发 bounded best-effort cleanup，不能授予或撤销 authority。

### 1.1 Provider errors and top-level declarations

| local type | exact variants/fields |
| --- | --- |
| `provider-error` | `invalid-argument \| limit \| unsupported-flow(unsupported-flow) \| unavailable \| cancelled \| failed` |
| `unsupported-flow` | `catalog-source \| authentication \| model \| tools \| tool-choice \| reasoning \| cache \| image \| proof \| response-media` |
| `descriptor-request` | `provider-id:provider-id, route-id:provider-route-id, catalog-source:catalog-source-view` |
| `provider-descriptor` | `provider-id:provider-id, route-id:provider-route-id, source-revision:option<catalog-revision>, catalog-digest:catalog-digest, model-count:u32` |
| `catalog-request` | `provider-id:provider-id, route-id:provider-route-id, catalog-source:catalog-source-view, catalog-digest:catalog-digest, offset:u32, limit:u16` |
| `catalog-page` | `provider-id:provider-id, route-id:provider-route-id, source-revision:option<catalog-revision>, catalog-digest:catalog-digest, declared-count:u32, offset:u32, entries:list<catalog-entry>, next-offset:option<u32>` |
| `auth-interaction-request` | `provider-id:provider-id, route-id:provider-route-id` |
| `auth-interaction-response` | `not-required \| instructions(auth-instructions)` |
| `auth-instructions` | `title:string, steps:list<string>` |
| `catalog-source-view` | `embedded \| verified(catalog-metadata-view)` |
| `prepared-request` | `body:wire-json-document, ordinary-headers:list<ordinary-header>` |
| `prepared-completion` | `request:prepared-request, decoder:own<response-decoder>` |
| `ordinary-header` | `name:string, value:string` |
| `frame-acceptance` | `accepted` |

`provider-error`、unsupported flow 和所有 terminal failure 都不含 message、status、body、header、URL、credential、stack、raw JSON 或 untrusted text。`auth-interaction.not-required` 只是 presentation metadata，不能覆盖 signed Host auth requirement；`instructions.title` 必须是 `Label(256)`，`steps` 为 `1..=32` 个 ordered `Safe+(4,096)` string，不能接收 answer 或 secret。metadata 与 Host requirement 不一致时，在 Broker、Pack activity 和 network 前 reject。Host 只对 signed provider/route identity 调用 descriptor，不能让 guest 枚举 identity。

## 2. Host context, grant authority and operation binding

Provider guest DTO 中的 provider、route、catalog digest、selection、current model、operation、request/turn ID 都是不可信 comparison values。method、origin、path/query、auth slot、adapter profile、credential/grant/injection lease、generation、route lease、provenance、socket 和 transport URL 只存在于 Host pre-bound context。Host 在 guest invocation 前及 network 前，用一个 immutable route/catalog snapshot 重新证明这些值。

### 2.1 Completion operation and durable grant

Provider fixed control operation 是 `descriptor`、`catalog`、`auth-interaction`；三者 ID pairwise distinct，且不得作为任何 `completion-operation`。durable grant key 是 `(family,manager,pack,operation)`。一个 Pack 内一个 completion operation 必须唯一映射一个 canonical credential/transport authority digest，digest 覆盖：provider、route、method、origin/path/query、auth slot/adapter/destination、redirect/retry/deadline policy。多个 model entry 只有在该 digest byte-identical 时才能共享 operation；same operation/different authority 在 activation 前拒绝。

一次 completion 保留同一个 sole `OperationId` 为 Manager `operationId`、Provider `prepare-input.operation-id`、route lease operation、grant key 和 authority digest；catalog entry 的 `completion-operation` 必须与该 `operation-id` 相等，不能创建第二 authority ID。未知 operation、control/completion collision、consumer/Pack/generation mismatch、credential-contract mismatch 在 Broker、Pack、credential lookup、transport 前失败且 zero network/injection。Web、Usage、MCP、Compaction 的 downstream authority 不可由 Provider 代用；Compaction 复用已有 route lease/grant，不 mint Compaction grant。

### 2.2 Exact Host context fields

签名 manifest/ledger 的 immutable context 至少绑定：

| group | exact authority fields |
| --- | --- |
| identity | family=`providers`、role、Manager ID、Pack ID、source、publisher/signer、version、hash、generation、component/world/interface digest |
| route | `provider-id`、`route-id`、catalog digest/count、auth slot、route lease、catalog generation |
| model | selection tag/payload、exact current model、completion operation、input modalities、tool/reasoning capabilities、context/output limits、exact Host `context-counter-ref-v1` tuple 与 `counter-digest` |
| credential | canonical service/account/issuer/schema、credential-contract version、injection lease、authority digest |
| transport | exact method/origin/path/query、adapter profile、ordinary/reserved header policy、response media、deadline/retry/redirect policy |
| provenance | source request ID、turn ID、Pack hash/generation、route/catalog/source stamp |

这些 fields 是 Host context，不是 import、URL、socket、credential 或 authority-bearing guest constructor。

### 2.3 Provider scalar and bounded local types

Provider namespace 内的 scalar aliases 彼此独立：`provider-id` 是 `1..=64` lowercase ASCII，首字符为 letter、末字符为 alphanumeric，内部只允许 lowercase letters、digits 与 nonadjacent `-`；`provider-route-id`、`model-id`、`model-alias` 是 `1..=256` visible ASCII；`provider-operation-id` 使用与 Manager `OperationId` 相同的 `1..=128` canonical validator；`request-id` 与 `turn-id` 是 `1..=128` ASCII bytes，首尾为 alphanumeric、内部只允许 alphanumeric 或 `.`、`_`、`:`、`-`；`image-stamp` 是 `img1-[0-9a-f]{32}`，`proof-stamp` 是 `prf1-[0-9a-f]{32}`，二者均只由 Host table 生成；`catalog-digest` 与 `catalog-content-digest` 是独立 scalar aliases，exact grammar 均为 `sha256:[0-9a-f]{64}`；`catalog-revision` 是 closed record，不是 string 或 digest alias。model count 为 `0..=4,096`，catalog page limit 为 `1..=256`。所有 aliases 的 bytes/count 使用 checked conversion。

| local type | exact shape and bound |
| --- | --- |
| `catalog-content-digest` | lowercase text，exact grammar `sha256:[0-9a-f]{64}` |
| `catalog-revision` | closed record `{last-modified:u64, canonical-content-digest:catalog-content-digest}`；`last-modified` 为 normalized Unix seconds，范围 `1..=i64::MAX` |
| `capability-support` | `unknown \| unsupported \| supported` |
| `input-modality` | `unknown \| text \| image`；最多 3 个，declaration order、unique，`unknown` 出现时必须是 sole item |
| `tool-capability` | `tools:capability-support, auto-choice:capability-support, none-choice:capability-support, specific-choice:capability-support` |
| `reasoning-capability` | `reasoning:capability-support, effort:capability-support, budget:capability-support, proof:capability-support` |
| `model-selection` | `exact(model-id) \| alias(model-alias)` |
| `catalog-entry` | `selection:model-selection, current-model:model-id, display-name:option<string>, input-modalities:list<input-modality>, tool-capability:tool-capability, reasoning-capability:reasoning-capability, context-tokens:option<u64>, max-output-tokens:option<u64>, completion-operation:provider-operation-id` |
| `catalog-metadata-entry` | `selection:model-selection, display-name:option<string>, input-modalities:list<input-modality>, tool-capability:tool-capability, reasoning-capability:reasoning-capability, context-tokens:option<u64>, max-output-tokens:option<u64>` |

`Safe(n)` 是 UTF-8 `0..=n` bytes，拒绝 CR、DEL、全部 C0（TAB/LF 例外）、全部 C1，以及 Unicode `Bidi_Control` `U+061C`、`U+200E`、`U+200F`、`U+202A..U+202E`、`U+2066..U+2069`；`Safe+(n)` 为非空 Safe，`Label(n)` 为非空且无 TAB/LF 的 Safe，`VisibleAscii(n)` 为非空 ASCII `!` 到 `~`。Provider 的 every local string/list 仍分别执行 `0/1/N/N+1`。catalog selection unique；present 的 `display-name` 必须是 `Label(256)`；present 的 context/output token limit 必须至少 1；unknown/unsupported capability 不能被当作 supported。

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

## 5. Exact prepared request and completion input

### 5.1 Top-level and message fields

| local type | exact fields/variants |
| --- | --- |
| `prepare-input` | `provider-id:provider-id, route-id:provider-route-id, catalog-digest:catalog-digest, selection:model-selection, current-model:model-id, operation-id:provider-operation-id, request-id:request-id, turn-id:turn-id, system:list<string>, messages:list<message>, tools:list<tool-definition>, tool-choice:tool-choice, reasoning:reasoning, cache-retention:cache-retention, max-output-tokens:option<u64>` |
| `message` | `user(user-message) \| assistant(assistant-message) \| tool-result(tool-result-message)` |
| `user-message` / `assistant-message` | `blocks:list<user-block>` / `blocks:list<assistant-block>` |
| `tool-result-message` | `call-id:string, blocks:list<tool-result-block>, is-error:bool` |
| `user-block` | `text(text-block) \| image(image-view)` |
| `assistant-block` | `text(text-block) \| reasoning(reasoning-block) \| tool-call(tool-call-block)` |
| `tool-result-block` | `text(text-block) \| image(image-view)` |
| `text-block` | `text:string` |
| `reasoning-block` | `kind:reasoning-kind, text:string, proof:option<reasoning-proof-view>` |
| `reasoning-kind` | `thinking \| summary` |
| `reasoning-proof-view` | `stamp:proof-stamp, source-request-id:request-id, source-turn-id:turn-id, source-content-index:u8, reasoning-kind:reasoning-kind, proof:list<u8>` |
| `tool-call-block` | `call-id:string, name:string, arguments:wire-json-document` |
| `tool-definition` | `name:string, description:string, input-schema:wire-json-document` |
| `tool-choice` | `unset \| auto \| none \| specific(specific-tool-choice)` |
| `specific-tool-choice` | `name:string` |
| `reasoning` | `unset \| disabled \| enabled(enabled-reasoning)` |
| `enabled-reasoning` | `effort:option<reasoning-effort>, budget-tokens:option<u64>` |
| `reasoning-effort` | `minimal \| low \| medium \| high` |
| `cache-retention` | `unset \| none \| request \| session` |
| `image-view` | `stamp:image-stamp, media-type:image-media-type, bytes:list<u8>, metadata:image-metadata` |
| `image-metadata` | `width:u32 (1..=16,384), height:u32 (1..=16,384), frames:u32 (1..=64)` |
| `image-media-type` | `png \| jpeg \| gif \| webp \| tiff` |

`prepare-input` 的 bounds 是 system `0..=1,024`、messages `0..=4,096`、tools `0..=1,024`、每个 message blocks `1..=4,096`，且整个 input logical charge `<=8 MiB`。每个 system part 是 `Safe(64 KiB)`，每个 tool description 是 `Safe(64 KiB)`；`text-block.text` 与 `reasoning-block.text` 各自是 `Safe+(64 KiB)`；`tool-definition.name`、`specific-tool-choice.name`、`tool-call-block.name` 与 output `tool-call-start.name` 都是 `Label(128)` 并用 exact bytes 比较，不做 case/Unicode normalization；`tool-result-message.call-id`、`tool-call-block.call-id` 与三个 output tool events 的 call-id 使用 request/turn 的同一 `1..=128` ASCII tracking grammar。tool definition name 与 call ID 在各自集合全局 unique；每个 call name 必须引用 exactly one definition，但同一 definition 可被不同 call ID 引用；tool arguments 与 input schema root 必须是 object。present 的 `max-output-tokens` 与 reasoning budget 至少 1，且 max output 不得超过 catalog limit。

每个 `image-view.bytes` 为 `1..=8 MiB`；这是 typed input 的 pre-transform admission cap，不承诺其 text encoding 一定适配 8 MiB wire-body cap。`image-metadata.width` 与 `height` 各为 `1..=16,384`，`frames` 为 `1..=64`，且 selected catalog entry 的 canonical `input-modalities` 必须包含 `image`（`unknown` 不满足 image capability）。Host 在 lowering 前始终按 stamp 解析 stamped sidecar，并逐 field 验证 media、完整 bytes、width、height、frames、capability 与上述 bounds；不接受 URL/path/base64 input。metadata 三个 scalars 都只是 validation-only：各自有 zero mandatory output debt，contract 可通过既有 scalar source 对同一 image occurrence 的每一项引用零次或一次；引用时必须 `checked-u32` 且 output canonical value 与 Host-validated sidecar exact match，duplicate use reject，未引用合法，适配没有 dimension/frame wire fields 的 body。

本文每一个 raw image binding 都仅指 bytes/media payload accounting，不强迫 metadata 输出。每个 image occurrence 的 bytes 与 media 必须在 prepared tree 通过同一个 `image-data-uri` composite 或两个 separate scalar exact-once account；trusted adapter 若输出 base64，decode 后必须 byte-identical 于 Host-verified bytes，并按第 6/7/8 节在 Host allocation 前验证 checked expansion 与 remaining derived-string/wire-body budget。所有单项 bounds 仍受 containing value logical-charge cap 限制；不能 fit 的 admitted image 返回 `limit` 且 zero credential/network。

### 5.2 Message reducer and option semantics

request-wide reducer 是 `idle | pending(call-id,name,sealed-definition queue)`。assistant message 中的 complete `tool-call-block` 按 declaration order 入 queue；紧随其后的 messages 必须恰好按 queue 顺序各提供一个 nonempty `tool-result-message`。queue 未空时禁止 user/assistant、extra/missing/duplicate/out-of-order result 和 request end；idle tool-result、orphan/crossed call 都 reject。每个 input call ID 全局 unique，且每个 call name 解析到唯一 sealed tool definition；arguments 已是 duplicate-free、depth-bounded、object-rooted `wire-json-document`。只有 reducer 将当前 result `call-id` exact-match 到 queued preceding call 后，Host 才为该 result 构造两个 independent derived projections：`tool-result-status` 从 validated `is-error=false|true` 唯一映射为 `success|error`，`tool-result-name` 取 matched call 的 exact name。`tool-result-name` 不是 guest field，也不再次消费 earlier `tool-call-name`；它必须同时 byte-equal queued call name 与 sealed registry definition 的 `Label(128)` bytes。crossed/orphan/unknown call 或 name mismatch 在 Adapter expansion、credential lookup 和 network 前 reject。

`tools` nonempty、任何 input tool call、任何 output tool event 或 terminal `tool-use` 都要求 selected catalog entry 的 `tools=supported`；`tool-choice` 的 `unset|auto|none|specific` 各有 adapter contract 中唯一 encoding：unset 的相关 paths 必须全部 absent，不能用 null/auto/empty tools/provider default；auto 要求 tools nonempty 且 `tools=auto-choice=supported`；none 要求 `none-choice=supported`；specific 要求 `tools=specific-choice=supported`、`tools.len=1`，且 sole input tool name 与 payload 完全一致。reasoning `unset` 使 related paths 全 absent，`disabled` 与 `enabled` 不可相互降级；任何 reasoning input/output 要求 `reasoning=supported`，present effort 另要求 `effort=supported`，present budget 另要求 `budget=supported`，且 effort/budget 精确保留。cache `unset` 是 absent，`none|request|session` 各自保留 exact mode。`max-output-tokens=None` 是 absent，`Some(0)` reject；present value 要求 catalog limit 存在并不得超过它。unknown/unsupported 均不满足任何 supported check。

Host 为 decoder 保留 sealed input tool registry、exact `tool-choice` 与 selected catalog capabilities。每个 output `tool-call-start.name` 必须 byte-identical 匹配 registry 中恰好一个 definition，并继承其 `Label(128)` bound；call ID 使用上述 tracking grammar且 request-wide unique。input tools 为空、catalog `tools!=supported` 或 choice=`none` 时任何 tool event reject；choice=`specific(name)` 时每个 start 只能使用该 sole name；`auto|unset` 也只能使用 registry member，绝不接受 provider 新名字。重复 start/call ID、unknown/crossed name、name 在 start/end 间变化、没有完整 arguments object 的 call 都在 normalized event publication 前拒绝；`tool-use` terminal 要求至少一个 complete call，`stop|length` 到达时不得有 open call。

## 6. Prepared JSON tree and canonical text

`wire-json-document` 是 family-local flat typed tree，不是 `body-json:string`：`root:u32, nodes:list<wire-json-node>`。node 只有 `null-value | boolean-value(bool) | number-value(string) | string-value(string) | array-value(wire-json-array) | object-value(wire-json-object)`；`wire-json-array{items:list<u32>}`；`wire-json-object{fields:list<wire-json-field>}`；field 为 `{key:string,value:u32}`。prepared body root 必须 object，logical charge `<=8 MiB`，nodes `1..=262,144`，depth `<=64`；root explicit 且为 final node、all reachable、non-root exactly one parent、child index lower than parent，禁止 shared child/cycle/missing node/trailing content/compression/multipart。

number text `1..=128` ASCII bytes，exact grammar 为 `0|-?(?:[1-9][0-9]*(?:\.[0-9]*[1-9])?|0\.[0-9]*[1-9])(?:e-?[1-9][0-9]*)?`。因此 negative zero、zero exponent、leading zero、plus sign、uppercase `E`、exponent zero、trailing fractional zero、NaN/infinity 都 reject。object key 是 UTF-8 byte-sorted unique `Safe(256)`。ordinary string value 是 `Safe(64 KiB)`；只有被 validated contract node 的 `base64-standard-*|data-uri|join-lf|canonical-json-string|mistral-tool-result-content` transform 绑定、并由 Host 从 exact source 重算 byte-equal 的 value 才是 `DerivedSafe(8 MiB)`。`canonical-json-string` 的 transformed value 只能是 Host 从 validated arguments tree 生成的 canonical text；guest supplied text 绝不能取得该 provenance。`DerivedSafe` 使用同一 UTF-8/control/bidi rules，但不沿用 64 KiB ordinary cap；它仍同时受 prepared tree `8 MiB` logical charge、单次 operation retained-data cap 与 serialized wire body `8 MiB` cap。未绑定 transform 的 guest string、identity/enum/constant output、Host-derived `tool-result-status|tool-result-name` 与 generic strict text input 不能声明 derived provenance。

Host emit 的 canonical JSON 保留 stored member order 和 number token，不做 float conversion，只直接写 UTF-8 scalar；只 escape quote/backslash 与允许的 TAB/LF 为 `\t|\n`，不 escape slash，不输出 `\u`。strict text input 解码后再次应用 tree/depth/node/Safe rules；测试 `\u0000`、`\r`、`\u202e`、ordinary decoded string 64 KiB+1、无合法 transform provenance 的 larger string 和 key 256+1 必须拒绝。derived value 即使 decoded bytes `<=8 MiB`，若 escaping/punctuation 后使 final sink 达到 8 MiB+1 仍为 `limit`。

## 7. Closed `AdapterContractV1` and pure validator

T7 冻结 Host-private、closed、non-extensible `adapter-contract-v1`；它不是 guest DTO、authority map 或 metadata bag。Host 必须先从 sealed selected catalog entry 构造 immutable `validated-catalog-entry-view`；guest 不能构造或修改该 view。private validator 的 exact signature 是 `validate-adapter(contract: adapter-contract-v1, selected: validated-catalog-entry-view, original: prepare-input, prepared: wire-json-document, headers: list<ordinary-header>) -> provider-validation-result`。此 pure call 同时绑定 sealed catalog view、original、prepared tree 与 headers；T7 只维护一个 exhaustive trusted dummy contract/schema validator fixture，十个 real wire contract 与各自 golden 由 T11 实例化和验证，T7 不声称它们已落地。所有下列 type 都只存在于 Host validator namespace，不能被 manifest、Pack 或 runtime map 扩展。

| private type | exact fields/variants |
| --- | --- |
| `adapter-contract-v1` | exactly six top-level fields: `version:u8, wire-id:adapter-wire-id, model-source:adapter-model-source, tree:contract-tree, ordinary-header-rules:list<ordinary-header-rule>, decoder-kind:adapter-decoder-kind` |
| `adapter-wire-id` | `anthropic-messages \| openai-completions \| openai-responses \| openai-codex-responses \| azure-openai-responses \| google-generative-ai \| google-vertex \| mistral-conversations \| bedrock-converse-stream \| pi-messages` |
| `adapter-model-source` | `requested-selection \| current-model` |
| `contract-tree` | `root:u32, nodes:list<contract-node>, tables:list<enum-token-table>` |
| `contract-node` | `parent:option<u32>, segment:option<path-segment>, presence:adapter-presence, presence-source:option<adapter-presence-source>, body:contract-node-body` |
| `contract-node-body` | `object(contract-object) \| array(contract-array) \| switch(contract-switch) \| value(contract-value) \| constant(contract-constant)` |
| `contract-object` | `children:list<u32>` |
| `contract-array` | `collection:adapter-collection, item:u32, min:u32, max:u32` |
| `contract-switch` | `source:adapter-variant-source, cases:list<contract-case>` |
| `contract-value` | `source:adapter-scalar-source, transform:adapter-transform` |
| `contract-constant` | `value:typed-json-constant` |
| `contract-case` | `variant-ordinal:u8, node:u32` |
| `path-segment` | `key(string) \| array-item` |
| `adapter-collection` | `system \| messages \| system-messages \| blocks \| tools` |
| `adapter-variant-source` | `model-selection \| system-message-entry \| message \| user-block \| assistant-block \| tool-result-block \| tool-result-status \| tool-choice \| reasoning \| cache-retention` |
| `adapter-scalar-source` | `selected-model \| selection-kind \| system-item \| system-joined \| message-role \| block-kind \| block-text \| tool-result-call-id \| tool-result-is-error \| tool-result-status \| tool-result-name \| mistral-tool-result-content \| tool-call-id \| tool-call-name \| tool-call-arguments \| tool-name \| tool-description \| tool-schema \| reasoning-kind \| proof \| image-bytes \| image-media-type \| image-width \| image-height \| image-frames \| image-data-uri \| tool-choice-kind \| tool-choice-name \| reasoning-mode \| reasoning-effort \| reasoning-budget \| cache-retention \| max-output` |
| `adapter-transform` | `identity \| checked-u32 \| checked-u64 \| json-subtree \| canonical-json-string \| mistral-tool-result-content \| join-lf \| base64-standard-padded \| base64-standard-unpadded \| data-uri \| enum-token(u16)` |
| `adapter-presence` | `required \| omit-if-none \| omit-for-unset` |
| `adapter-presence-source` | `reasoning-proof \| reasoning-effort \| reasoning-budget \| max-output \| tool-choice \| reasoning \| cache-retention` |
| `typed-json-constant` | `null \| boolean(bool) \| number(string) \| string(string)` |
| `enum-token-table` | `source:adapter-enum-source, entries:list<enum-token-entry>` |
| `adapter-enum-source` | `selection-kind \| message-kind \| user-block-kind \| assistant-block-kind \| tool-result-block-kind \| tool-result-status \| reasoning-kind \| image-media-type \| tool-choice \| reasoning-mode \| reasoning-effort \| cache-retention` |
| `enum-token-entry` | `variant-ordinal:u8, token:string` |
| `ordinary-header-rule` | `fixed(fixed-header-rule) \| one-of(one-of-header-rule)` |
| `fixed-header-rule` | `name:string, value:string` |
| `one-of-header-rule` | `name:string, values:list<string>, required:bool` |
| `adapter-decoder-kind` | `anthropic-messages \| openai-completions \| openai-responses \| openai-codex-responses \| azure-openai-responses \| google-generative-ai \| google-vertex \| mistral-conversations \| bedrock-converse-stream \| pi-messages` |
| `validated-catalog-entry-view` | `provider-id:provider-id, route-id:provider-route-id, catalog-digest:catalog-digest, selection:model-selection, current-model:model-id, input-modalities:list<input-modality>, tool-capability:tool-capability, reasoning-capability:reasoning-capability, context-tokens:option<u64>, max-output-tokens:option<u64>, completion-operation:provider-operation-id` |
| `provider-validation-result` | `result<validated-adapter,adapter-validation-error>` |
| `validated-adapter` | `wire-id:adapter-wire-id, decoder-kind:adapter-decoder-kind, contract-digest:string, body-digest:string, ordinary-header-digest:string` |
| `adapter-validation-error` | `invalid-contract \| source-mismatch \| body-mismatch \| header-mismatch \| capability-mismatch \| limit` |
| `context-counter-ref-v1` | exact ordered fields `registry-id:string(LocalId(64)), registry-version:u16, algorithm-id:string(LocalId(64)), algorithm-version:u16, algorithm-digest:string, vocabulary-digest:string, wire-framing-id:string(LocalId(64)), wire-framing-version:u16, wire-framing-digest:string, output-reservation-id:string(LocalId(64)), output-reservation-version:u16, output-reservation-digest:string` |

`adapter-contract-v1` 顶层只允许 table 中的 exact six fields，`version` 必须 exact `1`；extra/missing/seventh field 一律 `invalid-contract`。`wire-id` 与 `decoder-kind` 只接受上述 declaration-order ordinal 的同名 1:1 pair；任何 crossed pair 都是 `invalid-contract`。contract tree 为 postorder：`root,nodes,tables` 都是 tree representation 的一部分，nodes `1..=4,096`、depth `<=32`、tree total logical charge `<=1 MiB`。root index 必须是 final node，root wrapper 必须 `parent=None,segment=None,presence=required,presence-source=None,body=object`；每个 non-root 必须 `parent=Some(higher-index)`，恰被该 parent 的 `children|item|cases` 引用一次，且 child index lower than parent。object child 的 segment 必须是 unique `key(Safe+(128))` 并按 UTF-8 bytes sorted；array item 的 segment 必须是 `array-item`；direct switch case node 的 segment 必须 `None` 并继承 switch destination；其他 non-root 的 segment 必须 `Some`。引用的 parent、segment 与 reconstructed destination path 必须一致。所有节点 reachable、每个 non-root exactly one tree parent，derived destination 为 `1..=16` segments；在每个 exhaustive active switch expansion 中，每个 materialized output path 全局 unique，scalar path 与 compound path 不能相等，且只有实际 tree ancestor 可以是 prefix。该 wrapper 使 optional compound object/array/switch 也能整棵省略：`required` 必须 `presence-source=None` 且不可省略；`omit-if-none` 必须绑定 lexical occurrence 中恰一个 `reasoning-proof|reasoning-effort|reasoning-budget|max-output` option，iff source 为 `None` 时省略；`omit-for-unset` 必须绑定 lexical occurrence 中恰一个 `tool-choice|reasoning|cache-retention` variant，iff source 为 `unset` 时省略。其他 presence/source pair reject。省略节点时其所有 descendants 都 absent，不能输出 null、空 object 或 placeholder。

array min `<=max<=262,144` 且 max 不得超过其 source collection bound。`adapter-collection` 的 legal source set 已由下表穷尽：

| collection | exact lexical source and bound |
| --- | --- |
| `system` | root `prepare-input.system` declaration-order list，`0..=1,024` |
| `messages` | root `prepare-input.messages` declaration-order list，`0..=4,096` |
| `tools` | root `prepare-input.tools` declaration-order list，`0..=1,024` |
| `blocks` | 仅在一个具体 `message` variant 的 lexical occurrence 内，绑定 exactly enclosing `user-message.blocks`、`assistant-message.blocks` 或 `tool-result-message.blocks` 的对应 typed declaration-order list，`1..=4,096` |
| `system-messages` | 仅为下一段定义的 special root sequence；两个 frozen wires 上 checked `system.len + messages.len <=5,120` |

每个 array node 是一个独立 collection occurrence：它在上述 exact lexical scope 绑定恰好一个 source collection；实际 cardinality 必须在 min/max 内，每个 source element 按 declaration order 恰由 item subtree 展开一次，不能跨 occurrence 混用、重复、跳过或由 constant 补足。不存在其他 collection source：`tool-definition.input-schema` 仅对应一个 source=`tool-schema`, transform=`json-subtree` scalar occurrence；每个 image 是 enclosing user/tool-result block list 中一个 block variant occurrence，其 bytes/media/metadata 只形成 scalar payload，不是可迭代 adapter collection。contract 使用 removed `schema-members|images` spelling/ordinal、任何 unknown collection、root 与 lexical scope 错配、错误 typed list kind 或 crossed occurrence 都是 `invalid-contract`。除下述 closed `system-messages` sequence、`system` composite alternative 与 `mistral-tool-result-content` composite 外，每个 original collection occurrence 必须被一个 array node account，除非包含它的 optional wrapper 由 exact absent condition 整棵省略。`mistral-tool-result-content` 只能替代一个 Mistral tool-result lexical `blocks` occurrence 的 normal array expansion，并一次 account 该 collection 及全部 entries；normal expansion 与该 composite 必须 exclusive and complete。未选择 `system-messages` 时，root-level `messages` occurrence 仍由 `adapter-collection.messages` account，而 root-level `system` occurrence 必须且只能选择 `adapter-collection.system` expansion 或下述 `system-joined` composite，不能两者都用或两者都不用；root-level `tools` 与每个 message 的 lexical `blocks` occurrence 始终分别 exact-once account。switch cases 按 ordinal sorted unique，并对 source current variants exhaustive；`model-selection` 的 `exact=0,alias=1` switch 使 `selection-kind` reachable。

`adapter-collection.system-messages` 是 nested tree 内唯一 closed bounded sequence primitive，不是 generic list、arbitrary concat、output-path exception 或第七个 top-level field。它只允许 `wire-id=openai-completions|mistral-conversations`，contract 中至多出现一次，且必须在 root lexical occurrence 绑定 exactly one root-level `system` occurrence 与 exactly one root-level `messages` occurrence。materialized array sequence 恰为全部 system items（original declaration order），随后为全部 messages（original declaration order）；cardinality 用 checked `u64` 计算并要求 `system.len + messages.len <= 5,120`，同时满足该 array 的 min/max。item subtree 的 root 必须是 source=`system-message-entry` 的 exhaustive switch，fixed ordinals `system=0,message=1`：`system-item` 及其 source 只在 `system` case reachable；normal source=`message` 的 exhaustive message switch 只在 `message` case reachable。每个 synthetic entry 恰展开一次，禁止 duplicate、skip、cross-case 或跨 occurrence consumption。选择该 sequence 一次 account 两个 collections 及其全部 entries，禁止另有 `adapter-collection.system`、`adapter-collection.messages` 或 `system-joined`；不选择时维持上一段既有 alternatives。该 sequence 只 materialize 一个 array destination path，仍完整遵守 active switch expansion 的全局 output-path uniqueness。

private ordinals 固定为：selection `exact=0,alias=1`；synthetic system-message entry `system=0,message=1`；message `user=0,assistant=1,tool-result=2`；user block `text=0,image=1`；assistant block `text=0,reasoning=1,tool-call=2`；tool-result block `text=0,image=1`；derived tool-result status `success=0,error=1`，分别且只从 `is-error=false,true` 投影；tool-choice `unset=0,auto=1,none=2,specific=3`；reasoning mode `unset=0,disabled=1,enabled=2`；cache `unset=0,none=1,request=2,session=3`；reasoning-kind `thinking=0,summary=1`；reasoning-effort `minimal=0,low=1,medium=2,high=3`；image media `png=0,jpeg=1,gif=2,webp=3,tiff=4`。这些 ordinals 与 current WIT declaration order 或本文 explicit derived projection 同步，不能由 parser/runtime 枚举顺序猜测。

scalar enum mapping 是 closed 且唯一：`selection-kind -> selection-kind`、`message-role -> message-kind`、`block-kind ->` 当前 enclosing switch 对应的 `user-block-kind|assistant-block-kind|tool-result-block-kind`、`tool-result-status -> tool-result-status`、`reasoning-kind -> reasoning-kind`、`image-media-type -> image-media-type`、`tool-choice-kind -> tool-choice`、`tool-choice-name` 不是 enum、`reasoning-mode -> reasoning-mode`、`reasoning-effort -> reasoning-effort`、`cache-retention -> cache-retention`。`reasoning-block.kind` 是 sole output-consumable `reasoning-kind` scalar；`reasoning-proof-view.reasoning-kind` 只用于 Host comparison/sidecar validation，具有 zero output debt，不能解析为 scalar source。除下一段冻结的 tool-result status scalar/switch alternatives 外，kind/mode scalar 只在同名 exhaustive variant-source switch case 内可达。`selected-model` 是 JSON string source，由 contract 的 `model-source` 唯一决定：`requested-selection` 取 exact/alias payload bytes 并 account 该 selection payload，`current-model` 取 input current model 并 account 该 field；未被选择的另一 model view 只做 Host comparison validation，不形成 output-consumption debt。底层 `selection-payload` 与 `current-model` 不作为 `adapter-scalar-source` 暴露，不能在 `selected-model` 之外再次引用或序列化；`selection-kind` 只取 exact/alias ordinal。其他 scalar source 不得使用 `enum-token`。`enum-token(table-index)` 必须解析到 `tree.tables` 中恰好一项，且 table source 与上述 mapping 相同。`tree.tables` `0..=64`，index `0..=63`，每个 referenced index 只引用一表且不能有未引用表；每表 entries `1..=16`，按 ordinal sorted unique、token 为 `VisibleAscii(128)`，token 也 unique，并对 source variants exhaustive。缺/extra ordinal、重复 token、crossed source/table、错误 cardinality 或 out-of-range index 都是 `invalid-contract`。

source/transform compatibility 是 executable closed matrix；transform 完成后产生的 JSON type 也固定如下，任何未列 pair、隐式 conversion 或 output type mismatch 都是 `invalid-contract`：

| exact source class | sole legal transform(s) | output JSON type |
| --- | --- | --- |
| string scalar `selected-model\|system-item\|block-text\|tool-result-call-id\|tool-result-name\|tool-call-id\|tool-call-name\|tool-name\|tool-description\|tool-choice-name` | `identity` | string；source bytes 必须满足其原 type，`selected-model` 明确是 string |
| boolean scalar `tool-result-is-error` | `identity` | boolean |
| closed enum scalar `selection-kind\|message-role\|block-kind\|tool-result-status\|reasoning-kind\|image-media-type\|tool-choice-kind\|reasoning-mode\|reasoning-effort\|cache-retention` | `enum-token(valid-table-index)` | string token |
| `image-width\|image-height\|image-frames` | `checked-u32` | canonical unsigned decimal JSON number |
| `reasoning-budget\|max-output` | `checked-u64` | canonical unsigned decimal JSON number |
| `tool-call-arguments` | `json-subtree`；或仅在 `openai-completions\|openai-responses\|openai-codex-responses\|azure-openai-responses\|mistral-conversations` 使用 `canonical-json-string` | exact validated object subtree；或包含其 canonical JSON text 的 string |
| `tool-schema` | `json-subtree` | exact validated JSON subtree；root 仍须 object |
| `proof\|image-bytes` | `base64-standard-padded` 或 `base64-standard-unpadded` | string |
| `system-joined` | `join-lf` | string |
| `image-data-uri` | `data-uri` | string |
| `mistral-tool-result-content` | `mistral-tool-result-content`，仅限 `mistral-conversations` | exact Mistral content-chunk array；derived text 后跟 zero or more image chunks |

每个 lexical tool-result occurrence 的 error/status 与 derived name accounting 按 `wire-id` closed 如下；`tool-result-status` scalar 必须使用对应 exhaustive enum table，variant source 必须使用对应 exhaustive switch，二者都投影同一个 underlying validated bool，不能创造第二份 input debt：

| wire ID | exact error/status accounting | exact derived-name accounting |
| --- | --- | --- |
| `anthropic-messages` | `tool-result-is-error` 以 boolean `identity` exactly once；forbid status scalar/switch | forbidden；zero mandatory output debt |
| `pi-messages` | `tool-result-is-error` 以 boolean `identity` exactly once；forbid status scalar/switch | `tool-result-name` 以 `identity` exactly once |
| `bedrock-converse-stream` | status scalar exactly once，且 enum-token table entries 恰为 `{variant-ordinal:0,token:"success"},{variant-ordinal:1,token:"error"}`；forbid bool/switch | forbidden；zero mandatory output debt |
| `google-generative-ai\|google-vertex` | status variant-source exactly once，且 exhaustive switch 的 ordinal `0` success case 构造 output branch、ordinal `1` error case 构造 error branch；forbid bool/status scalar | `tool-result-name` 以 `identity` exactly once |
| `openai-completions\|openai-responses\|openai-codex-responses\|azure-openai-responses` | validate original bool，但 zero mandatory output debt；forbid bool source 与 status scalar/switch | forbidden；zero mandatory output debt |
| `mistral-conversations` | `mistral-tool-result-content` composite exactly once，并由该 composite 消费 original bool；forbid bool source 与 status scalar/switch | `tool-result-name` 以 `identity` exactly once |

同一 occurrence 对 underlying bool 最多消费一次：boolean、status scalar、status switch 与 Mistral content composite 四者绝不能同时、重复或跨 occurrence 使用；有 mandatory debt 的 wire 选择 multiple/neither 都 reject，zero-debt wire 出现任一 source/composite 也 reject。`tool-result-name` 只能在上述四个 wires 的 matched tool-result lexical occurrence 使用一次；它与 earlier `tool-call-name` 是 independent ledger entry，不能省略、duplicate、跨 result 复用或在其余六个 wires 使用。

`join-lf` 只对一个 complete `system-joined` occurrence 合法，按 original declaration order 用 literal LF byte `0x0a` 连接全部 system strings；空 collection 得到 empty string，serializer 后续按第 6 节将 LF 写为 `\n`。Host 在 lift/copy 前以 checked `u64` 计算 decoded output bytes `sum(item.byte-length) + max(item-count-1,0)`，并证明它同时 fit `DerivedSafe` 与 remaining tree/wire budgets；overflow/budget+1 不分配 aggregate。选择 composite 时一次 account system collection 与其全部 item strings，禁止同一 occurrence 再出现 `adapter-collection.system`、`adapter-collection.system-messages` 或 `system-item`；选择 collection expansion 时反向禁止 `system-joined`。选择 `system-messages` 时也反向禁止 `system-joined`。适用 wire 未选择 sequence 时，该 collection-vs-composite alternative 必须 exclusive and complete；其他 wire 始终只允许该既有 alternative。

`canonical-json-string` 只对 frozen `openai-completions|openai-responses|openai-codex-responses|azure-openai-responses|mistral-conversations` 的一个 `tool-call-arguments` occurrence 合法。Host 先验证 source 是 object-rooted `wire-json-document`，并在不 emit 或分配 aggregate 的 sizing pass 中按第 6 节 serializer exact rules 用 checked `u64` 计算 canonical text byte-length `C`，以及其中 quote 或 backslash bytes 总数 `E`；canonical text 没有 raw control byte。Host 同时 checked 计算最终 enclosing JSON-string length `2 + C + E`（两枚 quotes，canonical-text 中每枚 quote/backslash 各增加 one escape byte），并证明 `C` fit `DerivedSafe(8 MiB)` transformed-value cap、prepared-tree remaining 8 MiB logical-charge budget、operation remaining 8 MiB retained-data budget，且 enclosing string 加 destination punctuation fit final 8 MiB wire-body budget。只有全部 counters fit 后才分配 bounded decoded UTF-8 text sink，并恰好运行一次第 6 节 canonical serializer into that sink，without enclosing string quotes；stored member order、number token 和 escaping 因而与第 6 节 exact 一致，独立重复执行必须 byte-equal deterministic。只有该 Host-derived text 可标记 `DerivedSafe`；overflow 或任一 N+1 都返回 `limit` 且不分配 partial value。每个 arguments occurrence 必须且只能选择 `json-subtree` 或 `canonical-json-string` 一次；both、neither、second/reused source、crossed occurrence、wrong wire 全部 reject。`tool-schema` 仍且只允许 `json-subtree`。

`mistral-tool-result-content` 只对 `mistral-conversations` 的一个 complete tool-result lexical occurrence 合法，并替代该 occurrence 的 normal `adapter-collection.blocks` expansion。令 `J` 为全部 text block values 按 declaration order 用 literal LF byte `0x0a` 连接的 string，`H` 表示是否存在 image block；Host 从 sealed selected view 得出 image modality support，且第 5.1 节已使 `H=true`、image unsupported 在 expansion 前 reject。`Trim(J)` 只删除首尾由下列 exact Unicode scalars 组成的 maximal runs：`U+0009,U+000A,U+0020,U+00A0,U+1680,U+2000..U+200A,U+2028,U+2029,U+202F,U+205F,U+3000,U+FEFF`；中间及其他 scalars byte-preserving。令 `P` 在 `is-error=true` 时为 exact `[tool error] `、否则 empty；derived text `R` 在 `Trim(J)` nonempty 时为 `P || Trim(J)`，否则在 `H=true` 时为 `P || (see attached image)`，在 `H=false` 时为 `P || (no tool output)`。materialized array 的 first item 恰为 `{"type":"text","text":R}`，随后是每个 image block 按 original declaration order 产生的 `{"type":"image_url","image_url":<exact data-uri>}`；data URI 使用下一段的 exact MIME/base64 rules。

该 composite 一次 account enclosing blocks collection、每个 block variant、全部 text payload、underlying `is-error` bool，以及每个 image 的 bytes/media；image metadata 仍遵守 validation-only zero/one rule。选择 composite 时禁止另有该 occurrence 的 blocks array、`block-text`、`tool-result-is-error|tool-result-status`、`image-bytes|image-media-type|image-data-uri` source；`tool-result-call-id` 与 derived `tool-result-name` 仍各按本表独立 account。Host 在 lift/copy 或 aggregate allocation 前以 checked `u64` 扫描 source，计算 join、trim slice、prefix/placeholder、每个 data URI、array/object punctuation、escaping 后 final wire length及全部 logical/retained charges；只有全部 fit 后才分配并构造，prepared chunks 必须与 Host 重算结果 byte-equal。derived text 与 data URI 才取得 `DerivedSafe` provenance；overflow 或任一 cap N+1 返回 `limit`，normal/composite both/neither、crossed occurrence 或 wrong wire 返回 `invalid-contract`，prepared chunk mismatch 返回 `body-mismatch`，且均不产生 partial value。

`base64-standard-padded` 的 output string byte-length 是 checked `4 * ceil(source-bytes/3)`；unpadded 是 checked `4 * floor(source-bytes/3) + (remainder 0|1|2 -> 0|2|3)`。`data-uri` 只对同一 image occurrence 的 bytes+media composite `image-data-uri` 合法，输出 exact `data:` 加 lowercase MIME `image/png|image/jpeg|image/gif|image/webp|image/tiff`、literal `;base64,` 与 RFC 4648 standard padded base64，其 byte-length 是 checked exact prefix length 加 padded length。Host 必须在 lift/copy 或 aggregate allocation 前计算这些长度并证明 value fit `DerivedSafe`、prepared-tree remaining logical charge、operation retained-data 与 final wire-body budget；任何 overflow 或 first byte over 任一 cap 都返回 `limit`，不复制 partial derived string，且 zero credential/network。

`data-uri` 只且一次 account underlying `image-bytes` 与 `image-media-type`，不消费 width、height 或 frames，并禁止该 occurrence 分别复用 bytes/media；`mistral-tool-result-content` 对其 complete blocks occurrence 中每个 image 依相同规则 exact-once account bytes/media。选择 Mistral content composite 时禁止 normal blocks expansion 或任何 separate image payload source；其他 image occurrence 的 payload 必须且只能选择 `data-uri` composite 或分别 exact-once account bytes 与 media，both/neither 均 reject。三个 metadata scalars 独立保持 validation-only zero debt：每项可省略或以 `checked-u32` exact-once 输出，duplicate metadata source reject，不能借 omission 或 metadata output 改变 mandatory bytes+media accounting。`base64-*` 只接受 proof/image raw bytes；`json-subtree` 只接受 tool arguments/schema；`checked-u32`、`checked-u64`、`enum-token` 与 `identity` 也只能使用表中 source。source 的 typed admission cap、derived decoded length 与 final serialized length 是三个独立 checked counters，不能用 8 MiB source cap 预先证明 expansion fit。

`typed-json-constant.number` 必须满足第 6 节 exact `1..=128` canonical number grammar，`string` 必须是 `Safe(64 KiB)`，`boolean` 与 `null` 只输出其 exact JSON scalar；constant 不消费任何 original source，只 account 自己唯一的 output node/path。每个 composite、ordinary scalar、variant 与 collection 的 underlying accounting ledger 是同一份，因此 duplicate consumption、separate reuse、遗漏 source 或用 constant 假装消费 input 都 reject。

ordinary header rules `0..=32`，仅按 lowercase name UTF-8 bytes canonical sort且 case-insensitive unique；name/value 使用第 8 节 bound。`fixed` 是 Host-owned：prepared guest headers 中必须 absent，Adapter validation 后由 Host 插入 exact value。`one-of.values` 是 guest-owned allowed set，含 `1..=16` 个按 value bytes sorted unique values；required=true 时 guest 恰好提供一个 declared value，false 时零或一个。除此之外 guest 没有 ordinary header allowlist。规则不得命名 permanent deny/credential/transport reserved headers；prepared extra header、fixed collision、required one-of 缺失与未消费 guest header都拒绝。

complete forbidden authority source field list 是：`prepare-input.provider-id`、`route-id`、`catalog-digest`、`operation-id`、`request-id`、`turn-id`；`image-view.stamp`；`reasoning-proof-view.stamp`、`source-request-id`、`source-turn-id`、`source-content-index`。这些 fields 不是 scalar-source，contract node 不得引用或序列化；`reasoning-proof-view.reasoning-kind` 同样是 comparison/validation-only non-source，只有 enclosing `reasoning-block.kind` 形成 `reasoning-kind` output debt。proof 为 `Some` 时，其 bytes 必须由 active exact option wrapper 内的 `proof` source exact-once account，遗漏或 duplicate 都 reject；proof 为 `None` 时，只能由对应 `omit-if-none(reasoning-proof)` wrapper 证明整棵省略且没有 proof-byte debt。除上述 model-source accounting equivalence、明确 composite consumption、proof sidecar kind 的 validation-only zero-debt rule、image metadata 的 validation-only zero/one exception、per-wire tool-result bool/status zero-or-one rule，以及 per-wire derived tool-result name zero-or-one rule 外，其余每个 payload scalar、variant 和上述 legal collection occurrence 必须恰好被消费一次，或由其 exact `omit-if-none|omit-for-unset` wrapper 证明整棵 absent；`tool-call-arguments` 的 subtree/string alternatives 仍共用 one exact-once debt，不是 exception；每个 prepared node、destination path、guest header 和 constant 也必须恰好被 account。

validator 必须先逐 field 证明 `selected` 等于同一 sealed route/catalog snapshot，并证明 `original` 的 provider、route、catalog digest、selection、current model 与 completion operation 逐 field 等于该 view。在任何 contract expansion 前，Host 必须完整运行第 5.2 节 request-wide reducer，按 queued call-id/name 与 sealed registry 定义验证每个 tool result，随后且只能随后构造该 occurrence 的 `tool-result-status` 与 `tool-result-name` projections；crossed/orphan/unknown call/name 在此阶段 reject，不能由 contract branch 掩盖。Host 也必须对每个 `proof=Some(view)` 先证明 `view.reasoning-kind` exact-equal enclosing `reasoning-block.kind`，并逐 field 验证 stamp、source IDs/index、kind、proof bytes/digest 与 sealed proof sidecar；crossed outer/sidecar kind 或任一 sidecar mismatch 都在 contract expansion、credential lookup 和 network 前 reject。随后必须用 view 的 exact modalities、四个 tool capabilities、四个 reasoning capabilities 与 max-output limit 验证 original、prepared 与 decoder contract；任何 capability 都不得由另一 capability 推导，`unknown|unsupported` 均不满足 `supported`，所有 present budget/output value 都执行 bound comparison。`selected.context-tokens` 在 pure validator 中只验证 snapshot identity，不能把 bytes、characters、logical charge 或 Pack-provided number 当作 token usage；actual context enforcement 使用下述独立 Host measurement。model source、message/block order、tools/schema、call IDs、images、ToolChoice、reasoning、proof、cache、max output 必须同时与 selected view 和 original input 一致；所有 body/schema roots 为 object。crossed provider/route/digest/selection/current-model/operation、任一 capability bit 或 limit mutation 都必须在 credential lookup/network 前 reject。

三种 digest 都是 lowercase `sha256:`。`contract-digest = SHA-256("mcode-provider-adapter-contract-v1\0" || typed-contract)`，typed-contract 只按 `adapter-contract-v1` exact six top-level fields `version,wire-id,model-source,tree,ordinary-header-rules,decoder-kind` 编码；`tree` 内按 `root,nodes,tables` 编码 enum tables，不存在 seventh top-level field，contract 本身也没有 digest field。`body-digest = SHA-256("mcode-provider-wire-body-v1\0" || u64be(body-byte-length) || canonical-body-bytes)`；`ordinary-header-digest = SHA-256("mcode-provider-ordinary-headers-v1\0" || u32be(guest-header-count) || each(u32be(name-length)||lowercase-name||u32be(value-length)||value))`。typed-contract 的 record fields 按本文 table order、list 按 declaration order 编码；string/list 使用 u32be length/count，整数 fixed-width big-endian，bool `00|01`，option `00|01` 后接 payload，variant 用 declared zero-based u8 tag，node/reference index 用 u32be。header digest 只覆盖 Adapter 已验证、canonical sorted 的 guest-owned one-of headers；尚未插入的 fixed、credential、reserved、Host、content-length 与 transport headers 明确排除。成功 result 必须返回并绑定三个 digest、wire ID 与 decoder kind；任一后续 body/header/contract mismatch 在 credential/network 前拒绝。

context limit enforcement 是 validator/serializer 成功后、credential lookup/network 前的独立 Host-only step；不存在 externally parsed counter configuration 或开放 registry interpretation。每个 signed adapter profile 只能携带上一表的 closed Host-private `context-counter-ref-v1`。`LocalId(64)` 的 exact grammar 是 `1..=64` ASCII bytes、lowercase `[a-z][a-z0-9-]*`；四个 version 必须是 nonzero `u16`；四个 digest fields 必须逐 byte 匹配 lowercase `sha256:[0-9a-f]{64}`。record 没有 extension、map、unknown variant 或 optional field；profile parser 对 missing/extra/unknown field、顺序或 JSON type 错误及任何 coercion 全部 reject。

`counter-digest` 是 lowercase `sha256:` 加 SHA-256 over exact ASCII domain separator `mcode-provider-context-counter-ref-v1\0`，随后按上一表 declaration order 编码全部十二个 fields：每个 LocalId/digest string 都是 `u32be(byte-length) || UTF-8 bytes`，每个 version 是 `u16be`；任何 length/version conversion overflow 在 hash 前 reject。

每个 compiled registry entry 必须内嵌四份 source-controlled、build-time generated、runtime immutable 的 versioned canonical component bytes；它们分别是 closed Host compiler/evaluator 构造 actual algorithm implementation、vocabulary data、wire-framing implementation 与 output-reservation implementation 的 sole inputs，entry 不得附带任何未被对应 bytes 决定的 callback、table 或 default。四个 profile digest 的 exact preimage 分别是：`algorithm-digest = SHA-256("mcode-provider-context-algorithm-v1\0" || u16be(registry-version) || u16be(algorithm-version) || u64be(bytes.len) || bytes)`、`vocabulary-digest = SHA-256("mcode-provider-context-vocabulary-v1\0" || u16be(registry-version) || u16be(algorithm-version) || u64be(bytes.len) || bytes)`、`wire-framing-digest = SHA-256("mcode-provider-context-wire-framing-v1\0" || u16be(registry-version) || u16be(wire-framing-version) || u64be(bytes.len) || bytes)`、`output-reservation-digest = SHA-256("mcode-provider-context-output-reservation-v1\0" || u16be(registry-version) || u16be(output-reservation-version) || u64be(bytes.len) || bytes)`，结果均使用 lowercase `sha256:` text。closed compiler/evaluator 的语义由 `registry-id,registry-version` 标识；其语义变化必须 mint 新 registry version 并重算四个 digest。

registry static construction 必须先 canonical re-encode 四份 component bytes、checked 计算上述 preimages，并将重算结果逐 byte 等于 entry tuple 的四个 digest，才可发布 entry 或允许 route lookup；canonical bytes、实际 generated implementation/data 或 compiler/evaluator 语义任一变化而 exact tuple 未变化时，构造必须失败。route activation 随后在 Host compiled deterministic registry 中对这十二个值组成的 exact tuple lookup，并且必须恰好命中 one already-validated entry；zero match、unknown tuple 或 duplicate entries 都在 activation success、generation publication、Pack activity、credential/network 前 reject。registry entry 不解析 profile-supplied algorithm、vocabulary、framing 或 reservation instructions。T7 只有 one exhaustive trusted dummy registry entry；T11 才增加 ten real signed profile references 及其 Host implementations，且不得改变 V1 record、lookup、digest 或 measurement interpretation；本文不声称任何 current real counter 已存在。route/generation immutable binding 同时封存 exact ref tuple 与 `counter-digest`。

compiled entry 的 deterministic contract 恰为 `measure-context(original: &prepare-input, canonical-body: &[u8]) -> checked-u64`：successful output 只有 sole `required-context-tokens:u64`，任何 nonvalue outcome 都是无 payload 的 Host measurement rejection；输入是 validator 已绑定的 exact immutable `original` typed value 与 serializer 已 retained、其 `body-digest` 已验证的完整 canonical body bytes；实现仅按 registry tuple 指定的 algorithm/vocabulary、wire framing 与 requested/default output reservation 计算，不得读取 time、randomness、network、mutable global state、guest/Pack token number、prepared number field 或 remote metadata。每个 intermediate、framing charge、reservation、sum 与 final result 都用 checked `u64`；overflow 是 failure，不 saturate/wrap/coerce。Host 对同一 exact input 重复执行必须 byte-identical 得到同一 `u64`，否则 nondeterminism reject。sealed result 绑定 provider/route/catalog digest/selection/current model/completion operation、exact original snapshot、`body-digest`/retained body bytes、exact ref tuple、`counter-digest` 与 sole `required-context-tokens`；任何 original/body/snapshot/ref crossing 都 reject。`selected.context-tokens=Some(limit)` 时必须满足 `required-context-tokens<=limit`；missing registry entry、binding mismatch、overflow、nondeterminism 或 `limit+1` 均在 credential lookup/network 前 reject。`None` 表示没有 catalog context bound，不允许推导一个默认 limit，但仍执行并 seal profile 所绑定的 counter result。

未知 version/node/path/source/transform/presence、duplicate destination、unconsumed source、unaccounted output、cardinality/constant/capability mismatch、extra body path/header 都在 credential lookup/network 前 reject。T7 dummy validator 逐项 mutation closed source/transform/enum/header/decoder/accounting；fixtures 覆盖每个 legal source/transform pair 及每个 crossed pair；collection fixtures 对 root `system/messages/tools` 与 lexical `blocks` 分别覆盖 legal mapping、exact-once 和 0/1/N/N+1（N 为各自上表 bound；`blocks` 的 0 与所有 collection 的 N+1 reject），并覆盖 removed `schema-members|images` spelling/ordinal、unknown collection、wrong lexical scope、wrong typed list kind、crossed occurrence、duplicate 与 skip；另覆盖 system collection/composite both/neither、`system-messages` checked overflow 与 0/1/N/N+1、system-only、messages-only、system-before-messages ordering、duplicate/skip/cross-case/exact-once 及 forbidden wire/second occurrence/separate system-or-messages/system-joined。proof fixtures 覆盖 `None` wrapper omission、`Some` equal outer/sidecar kind、crossed kind pre-expansion rejection、尝试消费 sidecar kind，以及 omitted/duplicate proof-byte accounting。image fixtures 覆盖 composite/separate reuse、real-wire-shaped no-dimension dummy body、partial metadata output、duplicate metadata source、crossed image occurrence 与 bytes/media both/neither；constant fixtures 覆盖 grammar/type/accounting。derived-string fixtures 分别计算 proof padded/unpadded、每种 image MIME data URI、system join、canonical arguments string 与 Mistral tool-result text/content aggregate 的 exact length，覆盖 64 KiB ordinary boundary、每种 transform 在当前 remaining budget 下的 maximum accepted output 与 first rejected byte、checked arithmetic overflow、decoded fit 但 serialized 8 MiB+1，以及缺失/crossed transform provenance。`canonical-json-string` fixtures 覆盖 `{}`、nested object/member order/number token、quote/backslash/TAB/LF escaping、multibyte UTF-8、deterministic repeat byte equality、decoded N/N+1（N 为 transformed-value cap 与 current remaining budgets 的最小值）、decoded value fit 但 final enclosing string 因 escaping 达到 N+1，以及 arguments transform both/neither/reuse/crossed occurrence/wrong wire；schema 尝试使用该 transform 必须拒绝。T11 Mistral golden 必须以 nested 与 quote/backslash escaping arguments 证明 destination JSON type 是 string，且按同一 strict JSON rules decode 后所得 object tree 依第 6 节重序列化时与 source canonical arguments bytes byte-equal。tool-result status fixtures 对十个 wire rules 分别覆盖 `is-error=false|true`、exact `success|error` token、Google success-output/error-error branch paths、wire crossing、bool/scalar/switch/composite multiple、mandatory neither、duplicate/reuse 与 zero-debt forbidden use。Mistral dummy fixtures 必须以 literal expected chunks 覆盖 success/error nonempty text、empty text、pure image 与 mixed text/image，逐项验证 trim、prefix/placeholder、image order、aggregate bounds 与 normal/composite exclusion；T11 Mistral golden 必须与 frozen `mistral-conversations` body byte-equal。tool-result name fixtures 覆盖同一 sealed definition 被不同 call IDs 重复调用并各自正确 derive、crossed call/name、orphan/unknown call、registry mismatch、duplicate/omitted name source、cross-result reuse 与 forbidden-wire use；T11 Pi golden 必须证明每个 tool result 的 `toolName` 来自 matched queued call，且 omitted、guest-forged 或 cross-result name 在 expansion 前拒绝。model-source fixture 分别以 `requested-selection` 与 `current-model` 生成仅一个 model output path，并验证 selected underlying scalar exact-once、unselected view zero output debt；sealed view fixtures crossed provider/route/digest/selection/current-model/operation，并逐个 mutation modalities、四个 tool capabilities、四个 reasoning capabilities、context 与 max-output limit。context-counter fixtures 必须以 T7 sole dummy registry entry 覆盖 exact limit/limit+1、ref 的每个 field/digest/version mutation、missing/extra/unknown-field/type/coercion profile、zero-match unknown tuple、duplicate registry entries、保持 profile ref/四个 digest 不变而分别 mutation algorithm canonical implementation bytes、vocabulary canonical bytes、wire-framing canonical implementation bytes 与 output-reservation canonical implementation bytes 的 registry-construction rejection、original/body/snapshot/ref digest crossing、每个 checked-u64 stage overflow，以及 repeated execution 的 deterministic result 与 injected nondeterminism；不得假装存在 real counter。contract-digest fixture 必须覆盖 exact six top-level fields、nested `tree.tables`、mixed `fixed|one-of` rules 与 `one-of.values` ordering。T11 为上述 ten wire IDs 各实例化一个 real contract/golden，逐 wire 证明上述 arguments transform、tool-result error/status 与 derived-name accounting，且不得增加 V1 interpretation；任一 real counter component 的语义或数据变化必须产生 new exact tuple 与 `counter-digest`，并以 generation reconciliation fixture 证明 replacement generation publication 而非 no-op。

## 8. Header policy and final outbound set

Provider guest 只能供应 ordinary rule 中 guest-owned `one-of` header：`0..=32` 个，name 必须已经 lowercase，按 `(name,value)` byte order canonical sorted且 case-insensitive unique；Host 只验证 one-of，不插入或改写它。Host 随后只加入 `fixed`、content negotiation、credential/reserved、`host`、`content-length` 与 transport headers。每个 final name 是 lowercase HTTP token `<=64` bytes；value 是 `<=4,096` visible ASCII 加 interior SP/HTAB，不含 CR/LF/control/leading/trailing OWS。每次 insertion 后先重验 lowercase name、value 与 case-insensitive uniqueness；最终按 `(lowercase-name,value)` bytes sort，再重验一次。最终集合最多 32 headers、总量 `<=16 KiB`，charge 为 `4 + sum(4+name.bytes+4+value.bytes)`；credential 或 Host value 超 cap 时，在 socket/network 前 reject。

在 credential lookup/injection/socket/network 前，Host 先完成 AdapterContract 与 prepared body/header validation，再用第 6 节 canonical serializer 将 body 一次写入 bounded sink。valid object-rooted body 的 serialized lower boundary 是 2 bytes，即 `{}`；`wire-body-bytes` 是将发送的 exact UTF-8 bytes，checked `u64` 计数且范围 `2..=8 MiB`。overflow、8 MiB+1 或 `usize` conversion failure 为 `limit` 且 zero credential/network。guest 提供任何大小写的 content-length 已由 deny rule 拒绝；Host 随后插入唯一 lowercase `content-length`，value 是 `wire-body-bytes.len` 的无前导零 ASCII decimal。transport 必须发送同一 retained byte slice，不得之后 compression、re-serialization、chunked framing 或 body mutation；所有 Host/credential/reserved insertion 完成后再执行最终 32/16 KiB/uniqueness check。T7 serializer fixture 覆盖 `{}`、8 MiB、8 MiB+1、stale length/body mutation；若测试 counter zero，必须命名为 invalid/empty-sink counter fixture，不能把 zero 当成 valid body。

永久 lowercase deny names：`authorization`、`proxy-authorization`、`cookie`、`set-cookie`、`x-api-key`、`api-key`、`cf-aig-authorization`、`host`、`content-length`、`connection`、`proxy-connection`、`keep-alive`、`te`、`trailer`、`transfer-encoding`、`upgrade`、`expect`、`user-agent`、`origin`、`referer`、`forwarded`、`via`、`accept-encoding`、`content-encoding`、`x-http-method-override`、`x-method-override`、`x-original-url`、`x-rewrite-url`。`x-forwarded-`、`x-amz-` 前缀和 active adapter 的每个 reserved destination 也 deny。

`fixed(value)` 要求 guest absence，Host 插入；`one-of(values,required)` 在 required 时 exactly one、optional 时 zero or one。undeclared name/value、duplicate after injection、mixed-case alias、OWS variant、guest/Host collision、final header 33、final bytes 16 KiB+1 都 reject。insertion order 不影响结果，T7 dummy fixtures 覆盖每种顺序与 N+1。

## 9. Proof, model and image authority

每个 reasoning proof `1..=64 KiB`，一个 request/response proofs total `<=256 KiB`；`source-content-index` 与 output proof content index 都必须在 `0..=63`，且 selected catalog entry 必须 `reasoning.proof=supported`。Host 在 state insertion 前 seal proof bytes with：provider、route、catalog digest、selection tag/payload、current model、completion operation/authority digest、Pack ID/hash/generation、source request ID、source turn ID、content index、reasoning kind 和 proof digest。proof 只允许同一 Host session/branch lineage、相同 sealed route/model/Pack context 的 read-only sidecar：同一 request retry 可 idempotently reuse；later request 可 reference prior-turn proof exactly once。input proof 为 `Some` 时，Host 在 contract expansion 前要求 sidecar 的 reasoning kind exact-equal enclosing `reasoning-block.kind` 并验证完整 seal；sidecar kind 只比较、不输出，enclosing block kind 是 sole consumable `reasoning-kind` scalar，proof bytes 则保持 mandatory exact-once accounting。duplicate occurrence、foreign branch、catalog/model/alias drift、Pack/hash/generation change、crossed/out-of-range index、crossed outer/sidecar kind、modified bytes、unsupported proof capability 和 N+1 都 reject；guest bytes 不能创建 authority，proof 不写日志。

reported model 缺失或不在 validated catalog 时保持 `None`，不能从 requested/current model 推导；四个 usage counter 各自独立 optional，range 为 `0..=9,223,372,036,854,775,807`，`Some(0)` 是真实 zero，超上限为 `limit`。completion with any tool call 必须 reason=`tool-use`，而 `tool-use` 至少一个 complete call。image binding 必须为每个 raw image 的 bytes/media payload exact-once account；Host 无条件以 stamped sidecar 验证 media/dimensions/frames，只有 contract 实际引用的 metadata 才要求 prepared output exact match。

## 10. Response decoder automaton and normalized events

### 10.1 Frame and pull types

| local type | exact fields/variants |
| --- | --- |
| `response-frame` | `head(response-head) \| data(list<u8>) \| end` |
| `response-head` | `status:u16, media:response-media` |
| `response-media` | `json \| event-stream` |
| `decoder-pull` | `events(list<normalized-event>) \| need-frame` |
| `normalized-event` | `text-delta(text-delta) \| reasoning-delta(reasoning-delta) \| reasoning-proof(reasoning-proof) \| tool-call-start(tool-call-start) \| tool-arguments-delta(tool-arguments-delta) \| tool-call-end(tool-call-end) \| completed(completion-terminal) \| failed(provider-error)` |
| `text-delta` | `content-index:u8, text:string` |
| `reasoning-delta` | `content-index:u8, kind:reasoning-kind, text:string` |
| `reasoning-proof` | `content-index:u8, kind:reasoning-kind, proof:list<u8>` |
| `tool-call-start` | `content-index:u8, call-id:string, name:string` |
| `tool-arguments-delta` | `content-index:u8, call-id:string, delta:string` |
| `tool-call-end` | `content-index:u8, call-id:string` |
| `completion-terminal` | `reason:completion-reason, reported-model:option<model-id>, usage:usage` |
| `completion-reason` | `stop \| tool-use \| length` |
| `usage` | `input-tokens:option<u64>, output-tokens:option<u64>, cache-read-tokens:option<u64>, cache-write-tokens:option<u64>` |

Host decoder reducer 的完整 state set 是 `initial-pull | need-head | draining-body | need-body | draining-after-end | closed`；`closed` 另存唯一 close cause 与 outer terminal。method `Ok` transition 如下，表外 call/result/frame 一律 protocol close：

| current state | legal Host call | required guest result/input | next state and Host action |
| --- | --- | --- | --- |
| `initial-pull` | `pull(limit)`，`limit=1..=16` | exactly `Ok(need-frame)` | `need-head`；这是唯一 initial transition |
| `need-head` | `push(head)` | status `200..=599`、media `json\|event-stream`，guest returns `Ok(accepted)` | `draining-body`；seal the sole head |
| `draining-body` | `pull(limit)`，`limit=1..=free-capacity` | `Ok(events(batch))`，batch `1..=limit`、nonterminal | 保持 `draining-body`，enqueue 后等 capacity |
| `draining-body` | `pull(limit)` | `Ok(need-frame)` 且无 decoder-pending event | `need-body`；仅 output queue empty 后可读 network |
| `need-body` | `push(data)` | data `1..=64 KiB`，guest returns `Ok(accepted)` | `draining-body`；计入 frame/byte caps |
| `need-body` | `push(end)` | guest returns `Ok(accepted)` | `draining-after-end`；seal the sole end，不再读 network |
| `draining-after-end` | `pull(limit)` | `Ok(events(batch))`，batch `1..=limit`、无 terminal | 保持 `draining-after-end`，enqueue 后继续 drain |
| `draining-after-end` | `pull(limit)` | `Ok(events(batch))`，exactly one terminal 且为 batch last item | receipt 时 CAS `closed`，撤销 leases，发布该 sole terminal |
| any nonclosed | active method returns `Err(provider-error)` | closed stable error only | method receipt 时 CAS `closed`；该 frame/batch 不被接受，合成 exactly one stable failed outer terminal |
| any nonclosed | cancel/deadline/trap/cap breach/protocol violation | no further guest result trusted | winner CAS `closed`，关闭 transport、撤销 leases、合成 one stable outer terminal |
| `need-head\|need-body` | transport EOF/cancellation before required frame/end | no decoder call | winner CAS `closed`，关闭 transport并合成 one stable outer terminal |
| `closed` | no `pull`/`push` permitted | n/a | zero post-close decoder guest calls；CAS loser只观察 closed，不发布第二 terminal |

`push` 只在 prior `need-frame` 所形成的 `need-head|need-body` 合法；`pull` 只在 table 所列三个 draining/initial state 合法。`need-frame` 在 `draining-after-end` 非法；events empty、limit+1、terminal before end、terminal nonfinal/repeated、head/data/end 顺序错误都触发 protocol close。Host 在 push 前完成 transport decompression；恰一个 head 先于 data，恰一个 end 后不再有 frame。最多 1,024 data frames；2xx cumulative data `<=16 MiB`，non-2xx cumulative error data `<=64 KiB`，这些不能扩大 Host retained-data budget。

`pull` 没有 `finished` variant。每个 events batch logical charge `<=1 MiB`。每次 successful push 后 Host 按表 drain 到 `need-frame` 或 terminal batch；terminal receipt 后绝不再次调用 decoder。method Err 本身是唯一 guest failure receipt，不转换成 decoder event；若 output queue 已满，Host 只在 private closed state 保留 synthesized terminal，等已接受 items 被消费后发布，不再调用 guest。close/lease revocation 由单一 atomic CAS 线性化，cancel、EOF、trap、deadline、protocol、method Err 与 terminal 的 loser 都不得产生 effect 或 guest call；destructor 仅 bounded best effort，不计入 decoder protocol call。

### 10.2 Event reducer and backpressure

non-2xx 可消费 bounded error data，但不输出 text/reasoning/tool event，end 后只输出一个 stable failed terminal；status/header/error body 不进入 normalized output 或 diagnostics。每个 normalized event `<=128 KiB`；text/reasoning/tool-argument delta 必须 nonempty `Safe+(64 KiB)`；proof `1..=64 KiB`，所有 proofs `<=256 KiB`；所有 events 共用 count `<=65,536`、charge `<=8 MiB`。content-index 是 `0..=63`，最多 64 个，必须 contiguous/nondecreasing；每 index 只能绑定一种 text/reasoning/tool kind，advance 后不可重现。text/reasoning index 保持既有 nondecreasing reducer；一个 tool content-index 则精确表示 one and only one call。

Tool reducer 对 next contiguous index 的 state 是 `unused | open(call-id,name,delta-count,bytes) | sealed`。`tool-call-start` 只能在 `unused` 以 next index 打开 sole call，call ID 是 `1..=128` tracking string且 request-wide unique，name 按 sealed registry 校验。`open` 期间唯一合法的 content event 是同一 content-index、同一 call-id 的 nonempty `tool-arguments-delta`；禁止 second start（无论 ID 是否相同）、text/reasoning/proof event、另一 call interleave、index advance 或 terminal。必须收到 `1..=16,384` 个 delta 后，exact same index/ID 的 `tool-call-end` 才把该 index `sealed`；zero delta reject，文本 `{}` 是一个 nonempty delta并可形成最小 object。seal 后所有 subsequent content events 必须使用 next contiguous index，sealed index 永不重现；late delta/end 都 reject。多个 calls 因而各占 distinct contiguous content-index，不能让第二 call 复用 prior index。decoder 可以在自己的 bounded private state 中 buffer/reorder fragmented wire chunks 来产生此 normalized order，但发布给 Host 的 events 不得暴露 interleaving 或违反 reducer。

每 call cumulative delta `<=1 MiB`，all calls `<=2 MiB`。end 时 strict-parse 一个 duplicate-free、depth `<=64`、nodes `<=16,384`、logical charge `<=1 MiB` 的 canonical JSON object。proof 只能在当前 reasoning index advance 前出现一次，且要求 catalog proof capability；orphan/cross-index reject。所有这些 scalar/list/count/aggregate 分别覆盖 0/1/N/N+1。tool fixtures 另覆盖 second call same index、cross-call interleave、open 时 advance、sealed index 的 late delta 与 late end。

Host normalized-output queue capacity exactly 16。Host 每次只用 `limit=free-capacity` 调 decoder；capacity zero 时不 pull、不读 network、不 push frame。enqueue 一个 batch 后等待 consumer capacity；即便 decoder 已返回 `need-frame`，下一次 network read 也只能在 output queue empty 后进行。batch 0/limit/limit+1、queue 17、terminal 非 final、free capacity 0/1/16、head-then-disconnect 和 byte-at-a-time framing 都是 fixture。

### 10.3 Protocol failures and cleanup

head 前 data、head 重复、end 重复、data/end after closure、无 prior `need-frame` push、missing head/end、terminal before end、terminal not last、repeated terminal、data cap N+1、late event、pull after Host close 都是 protocol failure。cancel、EOF、trap、deadline、cap breach 或 protocol failure 按 10.1 table 的 atomic close 规则关闭 transport并合成一个 stable outer terminal；decoder destructor 不负责 correctness。只有该 table 明列的 call/state/result transition 合法。

## 11. Logical limits, redaction and T7 gates

Provider 使用 checked-`u64` logical charge：`bool/u8/s8=1`、`u16/s16=2`、`u32/s32/char=4`、`u64/s64=8`、string `4+UTF-8 bytes`、list `4+sum`、record/tuple field sum、enum/variant/option/result `4+active payload`、flags（最多 32 cases）为 4、WIT reference 为 4。ABI 只接受 memory32；memory64、f32/f64、future、stream、error-context 和未列 type static reject。Host-to-guest lowering 前、canonical lift fuel 后和 semantic validation 后分别守 bounds；任何 overflow 无 side effect。

所有 current Manager/Feature/Provider component 编译前 `<=4 MiB`，超限 artifact 在 measured Host policy change 前不可执行。T7 binary-only scan 用 locked `wasmparser` 检查 exact nested types、core memory/table min/max/count，拒绝 memory64/shared/atomics/threads；`resources_required()` 不能替代这些 checks；不创建 Store。T8 每 Pack one Store，启用 Wasmtime `async`、`consume_fuel` 与 epoch interruption；instantiate、guest call 与 waiting Host function 只走 async API。每个 Store 使用 `Store::set_hostcall_fuel(16 MiB)` 而不是默认 `128 MiB`；fuel 在每次 Host call 重置。每 memory `64 MiB`、最多 2 memories、每 table `65,536` elements、最多 4 tables、64 core instances；aggregate memory `128 MiB`、table `65,536`；Host resource table capacity `4,096`，admission live resources `4,096`、open operations `1,024`，并在 terminal/cancel/close 时释放 admission。每 operation retained guest-derived data `8 MiB`，每 Pack `64 MiB`；每个 guest segment 为 `100,000,000` fuel units 的 deterministic budget，不把它解释为 instruction count。Host supervisor 持有 monotonic `<=2s` outer deadline；epoch ticker target `<=10ms`，实际 interruption 发生在下一次 epoch check，不承诺 `2.010s` 或 scheduler-independent wall-time SLA；Host exchange wait 有独立 finite deadline，resume 只使用剩余 segment/deadline budget。

`prepare-input` 与 prepared body 各自 charge `<=8 MiB`；单个 decoder event `<=128 KiB`，decoder batch `<=1 MiB`，headers final `<=32/16 KiB`。每个 outer operation/decoder 最多累计 65,536 次 pull；N+1 由 Host 关闭并合成一个 stable failure，之后 zero guest calls。其余 decoder limits 按本文第 10 节。Limit/shape/invariant 映射 stable `limit|invalid-argument`；trap/deadline/cleanup 映射 `cancelled|unavailable|failed`，不带 source string。error/status/header/body/credential/proof raw bytes 不进入 log/diagnostic。

T7 pure/static gates：source WIT 与 current LF golden byte-identical；world zero imports 且 exactly one exported `provider-api` interface，该 interface exactly four freestanding functions plus one decoder resource、无其他 member/export；catalog digest/paging/cache relation、alias mapping、一个 exhaustive trusted dummy AdapterContractV1/schema validator、JSON tree、message reducer、canonical arguments string、Mistral tool-result content composite、per-wire tool-result status/name projection 与 accounting、ToolChoice/reasoning/cache、image/proof/header、sole trusted dummy context-counter registry entry、decoder/backpressure 全用 dummy in-memory fixture；fixture 可用 frozen wire IDs 驱动 closed validation branches，但不调用 network、credential、signature activation、Store、real adapter、real counter 或 real wire，也不包含 ten real wire contract/golden。

## 12. Stage ownership and artifact slice

| stage | exclusive responsibility | T7 不声称 |
| --- | --- | --- |
| T7 | current artifact/parser/static preflight、pure validators、redaction和bounds | Provider runtime、signature activation、credential/network、real wire PASS |
| T8 | Store/limiter/fuel/epoch、generation、decoder resource、cancel/reload/quiescence、outer terminal races | Provider product effect PASS |
| T10 | signed manifest/source trust、authority/profile/header binding、activation；不读取或注入 vault | credential injection PASS |
| T11 | Provider runtime、signed catalog metadata/cache、route/adapter binding、ten context-counter profile refs/Host implementations、vault/grant/injection、Pi ten-wire goldens | T7 real wire/counter PASS |
| T17/T18/T19 | frozen Feature Web/MCP/Usage Host effects | Provider 代替这些 family 的 Host methods |

Parseable artifact slice 必须提供 `mcode:provider-pack/provider@0.0.1` source WIT、byte-identical current LF `provider_current.wit`、`provider_current.jsonl` semantic golden 和 exact static preflight fixture。没有该 parser-checked artifact，本文仅冻结目标 authority，不是 source/golden/runtime 完成声明。若未来发生 breaking change，必须整体发布新的 package/world/current golden；当前 surface 不加解释分支。
