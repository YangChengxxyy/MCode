# ProviderPack ABI: closed adapter contract

> 返回 [08-provider-pack-abi.md](08-provider-pack-abi.md)；§7 续见 [derived transforms and validation](08-provider-pack-abi-validation.md)。本文件保留原 authority 的章节编号与规范效力。

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
