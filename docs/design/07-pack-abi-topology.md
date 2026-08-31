# FeaturePack ABI: topology and artifact boundary

> 返回 [07-pack-abi.md](07-pack-abi.md)。本文件保留原 authority 的章节编号与规范效力。

## 1. Current topology and artifact boundary

T7 只冻结以下 13 个 current world：

| package | world | exact boundary |
| --- | --- | --- |
| `mcode:plugin@0.0.1` | `mcode:plugin/manager@0.0.1` | import `mcode:plugin/feature-service@0.0.1`; export `mcode:plugin/manager-lifecycle@0.0.1` and `mcode:plugin/manager-tasks@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/session@0.0.1` | import `mcode:feature-pack/session-host@0.0.1`; export `mcode:feature-pack/session-pack@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/compaction@0.0.1` | import `mcode:feature-pack/compaction-host@0.0.1`; export `mcode:feature-pack/compaction-pack@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/resources@0.0.1` | no imports; export `mcode:feature-pack/resources-pack@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/ask@0.0.1` | import `mcode:feature-pack/ask-host@0.0.1`; export `mcode:feature-pack/ask-pack@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/todo@0.0.1` | import `mcode:feature-pack/todo-host@0.0.1`; export `mcode:feature-pack/todo-pack@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/web@0.0.1` | import `mcode:feature-pack/web-host@0.0.1`; export `mcode:feature-pack/web-pack@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/mcp@0.0.1` | import `mcode:feature-pack/mcp-host@0.0.1`; export `mcode:feature-pack/mcp-pack@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/usage@0.0.1` | import `mcode:feature-pack/usage-host@0.0.1`; export `mcode:feature-pack/usage-pack@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/subagents@0.0.1` | import `mcode:feature-pack/subagents-host@0.0.1`; export `mcode:feature-pack/subagents-pack@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/workspace@0.0.1` | import `mcode:feature-pack/workspace-host@0.0.1`; export `mcode:feature-pack/workspace-pack@0.0.1` |
| `mcode:feature-pack@0.0.1` | `mcode:feature-pack/ui@0.0.1` | no imports; export `mcode:feature-pack/ui-pack@0.0.1` |
| `mcode:provider-pack@0.0.1` | `mcode:provider-pack/provider@0.0.1` | zero imports; export `mcode:provider-pack/provider-api@0.0.1`; 详见 [08-provider-pack-abi.md](08-provider-pack-abi.md) |

Manager、11 个 FeaturePack family world/interface 与 Provider reference 都只存在表中 exact `0.0.1` first developer preview；其他历史 package/world/interface 文件、adapter、alias 与并行 current surface 均不存在。`mcode:feature-pack@0.0.1` 是一个 package，包含 11 个物理独立的 family world。每个 world 独立声明自己的 request、progress、result、error、Host interface 与嵌套类型；不能通过 `use` 跨 family 复用类型。Manager 只能 import `feature-service` 并 export `manager-lifecycle` 与 `manager-tasks`，不能反向；FeatureService selection 使用 Host-issued `psel1` stamp，冻结 exact ordered executable set、最多 256 Pack IDs、single/multi cardinality 与 empty atomic deactivate-all。`configured-packs` 在 generation fence 内接通独立 root-composition revision 与 family projection，Providers/Usage 保序、UI 排除 declarative themes；`activate-packs` 对本 generation 当前 stamp 完成 exact ordered secure load/typed instantiation，并在最终 root/generation revalidation 后原子替换，失败保留旧 set。

Main repository 只拥有 ABI/WIT/current goldens、binary static preflight、纯 semantic validator、通用 T8 component/resource runtime 和 family Host service/effect substrate；第一方 Manager/Pack source、build 与 release artifact 只在 `https://github.com/MCapricorns/MCode_plugins`。Parseable WIT source、resolved-world golden 和每-world semantic JSONL golden 是紧随本文的 T7 artifact slice；该 slice 缺失或未通过 parser 时，T7 不得标记通过。
