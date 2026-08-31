# T7 ProviderPack ABI authority

> 本文冻结 `mcode:provider-pack/provider@0.0.1` 的 current-only 目标契约，不声称 Provider runtime、catalog network、credential binding 或真实 wire 已实现。本文是仓库内可审查的 ProviderPack authority；紧随 T7 交付的 parseable WIT source、current LF golden 与 semantic JSONL golden 必须是其 machine-verifiable projection。
>
> Provider world 只有 zero-import current surface。只解析当前 typed surface；不保留 `abi_v1.json`、historical golden、compatibility parser/adapter、ABI alias、dual-read 或 fallback；没有 guest Host call、URL/socket/credential DTO、raw handle 或 generic JSON escape hatch。所有名称使用英文，说明使用中文。

## Authority map

以下主题文件共同构成 ProviderPack ABI authority；章节编号在拆分后保持连续且规范效力不变。

- [Surface, ownership and Host authority](08-provider-pack-abi-authority.md) — §1–§2
- [Catalog, digest and selection](08-provider-pack-abi-catalog.md) — §3–§4
- [Prepared request and canonical JSON](08-provider-pack-abi-request.md) — §5–§6
- [Closed adapter contract and source mapping](08-provider-pack-abi-contract.md) — §7 (part 1)
- [Derived transforms and sealed-view validation](08-provider-pack-abi-validation.md) — §7 (part 2)
- [Outbound headers, proof and modality](08-provider-pack-abi-outbound.md) — §8–§9
- [Decoder, limits and artifact gates](08-provider-pack-abi-decoder.md) — §10–§12
