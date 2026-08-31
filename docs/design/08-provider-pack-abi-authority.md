# ProviderPack ABI: surface and Host authority

> 返回 [08-provider-pack-abi.md](08-provider-pack-abi.md)。本文件保留原 authority 的章节编号与规范效力。

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
