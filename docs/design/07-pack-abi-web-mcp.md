# FeaturePack ABI: web and MCP

> 返回 [07-pack-abi.md](07-pack-abi.md)。本文件保留原 authority 的章节编号与规范效力。

## 8. `web` world

### 8.1 Exact signatures and local fields

- Imported interface: `mcode:feature-pack/web-host@0.0.1`。
- Exported interface: `mcode:feature-pack/web-pack@0.0.1`。
- Host signatures: `start-search(request: typed-search) -> result<own<web-exchange>,web-host-error>`；`start-fetch(request: typed-fetch) -> result<own<web-exchange>,web-host-error>`。
- Exchange signature: `web-exchange.pull() -> web-exchange-pull`。
- Pack signatures: `invoke(request: web-request) -> result<own<web-operation>,web-error>`；`web-operation.pull() -> web-pull`。

| local type | exact fields/variants |
| --- | --- |
| `web-request` | `search(search-request) \| fetch(fetch-request)` |
| `search-request` | `query:string, count:u8, range:search-range, chunks:u8, country:option<string>, language:option<string>, domains:list<string>` |
| `search-range` | `none \| d7 \| w2 \| m3 \| y1` |
| `fetch-request` | `urls:list<string>, format:content-format, per-page-timeout:u8, metadata:bool` |
| `content-format` | `markdown \| text \| html` |
| `web-progress` | `searching \| fetching(fetch-progress)` |
| `web-pull` | `pending \| progress(web-progress) \| complete(web-result) \| failed(web-error)` |
| `fetch-progress` | `completed:u8, total:u8` |
| `web-result` | `search-results(search-results) \| fetch-results(fetch-results)` |
| `search-results` | `items:list<search-item>, truncated:bool` |
| `search-item` | `source-id:string, search-id:string, url:string, title:string, text:string, published:option<string>, truncated:bool` |
| `fetch-results` | `pages:list<fetch-page>, truncated:bool` |
| `fetch-page` | `source-id:string, search-id:option<string>, url:string, title:option<string>, text:string, published:option<string>, truncated:bool, original-bytes:option<u64>, returned-bytes:u64, returned-lines:u32` |
| `web-error` | `invalid-argument \| invalid-url \| authority-rejected \| remote-unavailable \| protocol \| limit \| cancelled` |
| `typed-search` | exactly the seven fields of `search-request`; no method/origin/path/header/credential/extension field |
| `typed-fetch` | exactly `urls,format,per-page-timeout,metadata`; one request carries the complete deduplicated URL list |
| `web-head` | `status:u16, media:web-media` |
| `web-media` | `json \| event-stream \| text \| html` |
| `web-data` | `bytes:list<u8>` |
| `web-frame` | `head(web-head) \| data(web-data) \| end` |
| `web-exchange-pull` | `pending \| frame(web-frame) \| failed(web-failure)` |
| `web-failure` | `dns \| tls \| timeout \| truncated \| transport \| cancelled` |
| `web-host-error` | `invalid-argument \| authority-rejected \| remote-unavailable \| protocol \| limit \| cancelled` |

Host-private closed `WebAuthorityBindingV1` 不是 WIT、guest DTO 或 map，其 exact record 为 `{version:u16 (=1),family:web-authority-family (=web),manager-id:string,manager-generation:u64,pack-id:string,pack-version:string,pack-hash:string,pack-generation:u64,operation:web-authority-operation,method:web-method,origin:string,path:string,query-policy:web-query-policy,service-id:string,account-id:string,auth-slot-id:string,adapter-policy-digest:string,header-policy-digest:string,redirect-policy:web-redirect-policy,retry-policy:web-retry-policy,deadline:web-deadline,authority-digest:string}`。closed local types 为 sole-case enum `web-authority-family=web`、`web-authority-operation=search|fetch`、`web-method=get|post`、`web-query-policy=none|adapter-canonical`、`web-redirect-policy=forbid|same-origin-bounded(u8)`、`web-retry-policy={max-attempts:u8,backoff-ms:u32,retry-transport:bool,statuses:list<u16>}`、`web-deadline={total-ms:u32,per-attempt-ms:u32}`；无 unknown case/field。`same-origin-bounded` count 精确为 `1..=5`；retry `max-attempts=1..=4`、`backoff-ms=0..=60,000`、`statuses` 为 `0..=16` 个 numeric ascending sorted unique `400..=599` 值，`retry-transport` 是 bool；deadline `total-ms=1..=70,000`、`per-attempt-ms=1..=total-ms`。`max-attempts=1` 时 `statuses` 必须为空且 `retry-transport=false`；`max-attempts>1` 时至少 `retry-transport=true` 或 `statuses` 非空。Host 使用 checked `u64` 计算 `max-attempts * per-attempt-ms + (max-attempts - 1) * backoff-ms`，其值必须 `<=total-ms`；overflow、超界或 policy/deadline mismatch 全部在 digest acceptance/transport 前拒绝。authority digest 是 lowercase SHA-256 over ASCII `mcode-web-authority-binding-v1\0` 加除 `authority-digest` 外上述 declaration-order fields；string/list=`u32be` byte-length/count，u16/u32/u64=fixed-width big-endian，bool=`00|01`，enum/variant=zero-based u8 tag，`web-authority-family.web` 固定编码为 `00`，record 按声明顺序且转换 checked。T7 只用 dummy binding，逐字段（含 version、family、identity/generation、operation/method/origin/path/query policy、service/account/auth slot、adapter/header、redirect/retry/deadline）制造 digest/binding mismatch 并证明 zero transport；另以一份 complete dummy binding 固定全部 canonical preimage bytes 与 expected digest golden，明确覆盖 family 的 `00` byte。policy fixtures 对 redirect count、attempt/backoff/status count/status value/total/per-attempt 分别覆盖 0/1/N/N+1，并覆盖 duplicate/unsorted statuses、single-attempt带retry条件、multi-attempt无retry条件、per-attempt>total、checked schedule>total及每个 digest/binding mismatch。T10 才把同一 frozen shape 绑定为具体 signed values，T17 才执行 transport。URL 与 query 始终只是 bounded untrusted payload，不能覆盖 binding。

### 8.2 URL, exchange and stage semantics

Canonical URL 唯一顺序：(1) 要求 `1..=4096` ASCII bytes及 strict absolute http/https；拒绝 raw control/Bidi/backslash、fragment、userinfo、malformed percent、Unicode/punycode、malformed host/port。(2) decode全部 unreserved bytes；percent-decoded非ASCII必须 valid UTF-8；拒绝 decoded C0/C1/DEL/Bidi及 authority/path encoded `/|\\`；query执行同一 percent/UTF-8/control normalization，但 query `/` 不是 path separator。(3) 拒绝 decoding 后 `.`/`..` path segment（含 `%2e` aliases）。(4) lowercase scheme/host；IPv4 canonical dotted decimal，IPv6 RFC5952 lowercase compressed，DNS用下述 grammar；port仅 decimal 1..65535且无 leading zero，移除 http:80/https:443；empty path=`/`；剩余 percent hex uppercase；无 fragment。然后 canonical-string deduplicate，fetch为1..10 unique。URL/query仅 untrusted data，不改变 signed authority；output重跑同一算法。

`search-request.query` 为 `Safe+(1,000)`，`count=1..=20`，`chunks=0..=32`，`range` 只为 `none|d7|w2|m3|y1`；`country` 为 None 或 `[a-z]{2}`。language为 None或 exact syntactic subset：primary=2–3 lowercase letters；随后依次可有一个4-letter script、一个2-letter或3-digit region、0..3个5..8 lowercase-alnum variant；variant unique，单一 `-`，无 extension/private-use/grandfathered/empty，总长2..35。domains为0..20 unique byte-sorted canonical DNS names（非IP）；总长1..253 lowercase ASCII，label 1..63，首尾 alphanumeric、内部 alnum/hyphen，无 empty/trailing dot。`typed-fetch` 的 format 为 `markdown|text|html`，每页 timeout 为 `1..=60s`，metadata 为 bool；同一 fetch 不拆成多个 exchange。

web-exchange success为 pending*->head->data*->end；failed可作为first non-pending或在head后替换未完成suffix，successful轨迹的first non-pending必须且只能head。每pull最多一个frame，head/data/end不能合帧。status 200..599；data nonempty且每frame<=64KiB。上帧被pull前不读下帧；end/failed后zero guest/transport pull，post-terminal为protocol failure且不产生第二terminal。fixture覆盖pre-head transport failure与head-then-disconnect。

| response class | data frame count | cumulative data |
| --- | ---: | ---: |
| search 2xx | `<=32` | `<=2 MiB` |
| fetch 2xx | `<=160` | `<=10 MiB` |
| any non-2xx | one frame | `<=8 KiB` |

Declared/streamed N+1 在 buffer 前 reject；non-2xx 的 status/header/error body 不进入 normalized output 或 diagnostic。Host 生成 `source-id=wsrc1-[0-9a-f]{32}` 与 `search-id=wsea1-[0-9a-f]{32}`；search result 有 `0..=request.count` items，source ID unique，search ID 必须等于本次 sealed search，item 顺序保持 Host canonical rank。每个 output URL 再跑上述 canonical validator；title 是 `0..=512` Safe，text 是 `0..=50 KiB` Safe，published 为 None 或 Host-normalized exact 20-byte UTC `YYYY-MM-DDTHH:MM:SSZ`。

fetch pages 必须与 deduplicated request URLs 数量相等并保持 request order；Host 不得因 count/aggregate/line cap 丢弃 fetch page，只能在对应 page 内截断 text suffix并设置该 page `truncated=true`。source ID unique。`search-id=Some` 只可来自 Host provenance table 中该 canonical URL 的 exact prior search，否则为 None。title/published 使用同一 bound；metadata=false 时 title、published、original-bytes 必须全为 None。`returned-bytes` 必须等于 text UTF-8 byte length且 `<=50 KiB`；`returned-lines` 必须等于 empty 时 0、否则 LF count 加“末尾不是 LF”这一项，且 `<=2,000`；`original-bytes` present 时 `0..=10 MiB`。完整 result record logical charge `<=50 KiB`、总 returned lines `<=2,000`。fetch outer truncated精确等于任一page truncated；search outer truncated精确等于任一item truncated或Host因request count/aggregate cap丢弃ranked item suffix。published仅来自Host validated metadata。每个 guest-active segment独立受该 segment supervisor 的 `<=2s` deadline。Web operation clock 使用第 2.5 节在 operation allocation 前记录的同一 `started-at`（因此必然早于 `start-search|start-fetch`），absolute deadline 精确为 `started-at + accepted WebAuthorityBindingV1.deadline.total-ms`，不能放宽到固定 70 秒。fetch 每页 page clock 在该 URL 的 first transport attempt 前开始，absolute deadline 精确为 `page-started-at + validated fetch-request.per-page-timeout * 1,000ms`，跨该页全部 retry/backoff 且绝不重置；每个 transport attempt 另以 `attempt-started-at + accepted WebAuthorityBindingV1.deadline.per-attempt-ms` 为 attempt deadline，只有 policy 允许且 page/operation clock 均未到期时 attempt timeout 才能 retry，retry 只刷新 attempt clock。每次 transport wait 都取 attempt、page（fetch only）与 operation 三个 absolute deadline 的最早者；page 或 operation deadline winner 关闭同一 CAS，attempt deadline winner若不能合法 retry也关闭该 CAS。awaited exchange 位于 segment 外但计入 operation/page clock；resume仅刷新 segment deadline，不能刷新剩余 fuel、pull、page 或 operation budget，任一 deadline 后都无 late effect。Host error同名映射outer；exchange dns/tls/timeout/truncated/transport->remote-unavailable，cancelled->cancelled，non-2xx/schema failure->protocol；无第二 channel。每个 query/language/domain/URL/output field/list/count/byte/line aggregate 分别覆盖 0/1/N/N+1；fetch aggregate N+1 fixture必须仍返回与URL等长且同序的pages并在page内截断，缺页或多页reject。fixture 另覆盖 URL `%2e%2e`、`%7e` alias、encoded control/Bidi/separator、malformed UTF-8、default port、duplicate canonical form 和 URL 10/11。T7 只用 in-memory exchange fixture；T17 接入 Querit/Synthetic 的真实 Host transport，不新增 generic HTTP 或改变 `0.0.1`。

## 9. `mcp` world

### 9.1 Exact signatures and local fields

- Imported interface: `mcode:feature-pack/mcp-host@0.0.1`。
- Exported interface: `mcode:feature-pack/mcp-pack@0.0.1`。
- Sole Host signature: `start-invoke(request: typed-invocation) -> result<own<mcp-exchange>,mcp-host-error>`。
- Exchange signature: `mcp-exchange.pull() -> mcp-exchange-pull`。
- Pack signatures: `invoke(request: mcp-request) -> result<own<mcp-operation>,mcp-error>`；`mcp-operation.pull() -> mcp-pull`。

| local type | exact fields/variants |
| --- | --- |
| `mcp-request` | `servers(servers-request) \| tools(tools-request) \| invoke(invoke-request)` |
| `servers-request` | `snapshot-digest:string, after:option<string>, limit:u16` |
| `tools-request` | `snapshot-digest:string, server-id:string, after:option<string>, limit:u16` |
| `invoke-request` | `snapshot-digest:string, schema-digest:string, server-id:string, tool-id:string, arguments:mcp-json-document` |
| `mcp-progress` | `discovering \| invoking` |
| `mcp-pull` | `pending \| progress(mcp-progress) \| complete(mcp-result) \| failed(mcp-error)` |
| `mcp-result` | `servers(server-page) \| tools(tool-page) \| invoked(mcp-output)` |
| `server-page` | `items:list<server-info>, next:option<string>` |
| `server-info` | `server-id:string, title:string` |
| `tool-page` | `server-id:string, items:list<tool-info>, next:option<string>` |
| `tool-info` | `tool-id:string, title:string, description:option<string>, schema-digest:string, schema:mcp-schema-document` |
| `mcp-output` | `text(string) \| json(mcp-json-document)` |
| `mcp-error` | `invalid-argument \| snapshot-mismatch \| schema-mismatch \| server-not-found \| tool-not-found \| protocol \| limit \| transport-unavailable \| cancelled` |
| `typed-invocation` | `snapshot-digest:string, schema-digest:string, server-id:string, tool-id:string, arguments:mcp-json-document` |
| `mcp-head` | `invocation-id:string, snapshot-digest:string, schema-digest:string` |
| `mcp-exchange-output` | `text(string) \| json(mcp-json-document)` |
| `mcp-frame` | `head(mcp-head) \| output(mcp-exchange-output) \| failed(mcp-failure) \| end` |
| `mcp-exchange-pull` | `pending \| frame(mcp-frame)` |
| `mcp-failure` | `transport \| protocol \| timeout \| cancelled` |
| `mcp-host-error` | `invalid-argument \| snapshot-mismatch \| schema-mismatch \| protocol \| limit \| transport-unavailable \| cancelled` |

### 9.2 Independent MCP AST and schema

`mcp-json-document` 是独立 family-local AST：`{root:u32,nodes:list<mcp-json-node>}`，nodes `1..=16,384`、depth `<=64`、logical charge `<=1 MiB`。

| AST type | exact fields/variants |
| --- | --- |
| `mcp-json-document` | `root:u32, nodes:list<mcp-json-node>` |
| `mcp-json-node` | `null \| boolean(bool) \| number(string) \| %string(string) \| array(mcp-json-array) \| object(mcp-json-object)` |
| `mcp-json-array` | `children:list<u32>` |
| `mcp-json-object` | `members:list<mcp-json-member>` |
| `mcp-json-member` | `key:string, value:u32` |

`mcp-json-node` 只使用上述 six cases；number/string payload 的 bounds 如下，array/object 必须使用 family-local named records，不能共享 Provider/Usage DTO。root 必须为最后 node；所有 node reachable；每个 non-root 恰好一个 parent；每个 child index 严格小于 parent index；array/object children/members 各 `0..=1,024`；arguments root 必须 object。number 是 `1..=128` ASCII bytes，exact grammar 是 `0|-?(?:[1-9][0-9]*(?:\.[0-9]*[1-9])?|0\.[0-9]*[1-9])(?:e-?[1-9][0-9]*)?`。key `Safe(256)`、UTF-8 byte-sorted 且 unique；string value `Safe(64 KiB)`。malformed index/reachability/parent/order/number/string、duplicate key 和 N+1 均在 start 前 reject。

每个 tool entry 携带 snapshot-bound、独立 `mcp-schema-document`：

| schema type | exact fields/variants |
| --- | --- |
| `mcp-schema-document` | `root:u32, nodes:list<mcp-schema-node>` |
| `mcp-schema-node` | `any \| null \| boolean \| number(mcp-number-schema) \| %string(mcp-string-schema) \| array(mcp-array-schema) \| object(mcp-object-schema)` |
| `mcp-number-schema` | `integer:bool, minimum:option<mcp-number-text>, maximum:option<mcp-number-text>` |
| `mcp-string-schema` | `min-bytes:u32, max-bytes:u32` |
| `mcp-array-schema` | `item:u32, min-items:u16, max-items:u16` |
| `mcp-object-schema` | `properties:list<mcp-schema-property>, additional:mcp-additional` |
| `mcp-schema-property` | `key:string, schema:u32, required:bool` |
| `mcp-additional` | `forbid \| allow-any \| schema(u32)` |
| `mcp-number-text` | alias of `string` with the canonical-number bounds below |

nodes `1..=4,096`、logical charge `<=512 KiB`、depth `<=64`、single-parent、root last、all reachable；root 为 object。number bounds 为 None 或 `1..=128` ASCII canonical number，按 exact rational comparison；min 不得大于 max，integer=true 时边界也必须为整数；string bounds 为 `0..=65,536`，array bounds 为 `0..=1,024`，各自 min 不得大于 max。property key 是 `Safe+(256)`，按 byte-sort unique，最多 `1,024` 个；所有 schema reference 都是 child index。evaluator递归精确匹配：any接受任一 node；null/boolean/string/array/object仅同名case；number仅number，integer=true要求exact rational denominator=1。min/max inclusive exact arbitrary-precision rational；string按UTF-8 bytes；array按item count并逐项验item schema；object required必须存在，declared property按schema，undeclared在forbid拒绝、allow-any接受、schema(i)逐项验证。arguments 与 JSON response 均完整消费 root，并针对同一个 tool snapshot 的同一 `mcp-schema-document` 校验；不存在独立 output schema。closed `any` node 与 `allow-any` additional 是本文明确授权的 bounded AST cases，仍受 document node/depth/charge/Safe/graph bounds，绝不是 raw `Value`、map、raw JSON 或 extensions escape。无coercion/default/union/pattern/unknown。schema-digest为lowercase sha256 over `mcode-mcp-schema-v1\0`+schema node declaration order；tool snapshot digest为sha256 over `mcode-mcp-tool-snapshot-v1\0`+当前 immutable Pack catalog的完整 server list：server按server-id byte order，每项编码 `{server-id,server-title,tools}`，包括zero-tool server；tools按tool-id byte order，每项编码 `{tool-id,title,description,schema-digest}`。两者 framing：string/list=u32be length/count，integer fixed big-endian，bool=00/01，option=00/01+payload，variant=zero-based u8，record按上述声明field order，conversion checked。digest golden必须证明修改server title、添加或删除zero-tool server、修改任一tool field均改变snapshot digest。

successful轨迹的first non-pending frame必须且只能head；success=head->恰一output->end，failed可作为first non-pending或在head后替换未完成suffix。每pull最多一frame、最多3 non-pending、one-frame buffering，consumer pull前不生产下一帧；terminal后zero pull，post-terminal protocol failure。start Host error同名映射outer；exchange transport/timeout->transport-unavailable，protocol->protocol，cancelled->cancelled；digest mismatch映射对应outer mismatch，无第二channel。fixture覆盖pre-head transport failure与head-then-disconnect。`snapshot-digest`、`schema-digest` 都是 lowercase `sha256:` 加 64 lowercase hex。server/tool ID 是 `VisibleAscii(256)`；server title、tool title 是 `Label(256)`，description 为 None 或 `0..=4 KiB` Safe。cursor 是 None 或 Host-issued `mcpc1-[0-9a-f]{32}`；server page items `0..=limit`，按 server ID byte-sort unique；tool page 的 echoed server ID 必须等于 request，items `0..=limit`、按 tool ID byte-sort unique；limit `1..=128`。cursor private row 绑定 caller/Pack/generation/`OperationId`、server、snapshot digest、完整 query 与 last ID。

typed-invocation五 fields按声明顺序与bound invoke request exact typed structural equality。Host生成 `invocation-id=mcp1-[0-9a-f]{32}`；head invocation ID必须命中本次single-use Host row，head digest等于request且一次。text Safe(1MiB)，JSON <=1MiB且重验schema；terminal invoked与accepted output exact typed structural equality。server/tool not found、malformed/foreign/crossed invocation ID、crossed digest/cursor/head/output 在 retain 前 reject；fixture逐项覆盖这些 invocation ID failures。每个 identity/title/description/cursor/page/output/AST aggregate 分别覆盖 0/1/N/N+1。MCP 不暴露 process、socket、URL、path、stdio、raw JSON-RPC 或当前 vault credential。`servers`/`tools` 使用 Pack snapshot，Host 只参与 `invoke`。T7 验证各自 AST/schema graph 和 crossed snapshot；T18 负责真实 Host transport。
