# MCode 交付计划

只记录未完成工作、依赖和必须保持的边界。当前交付树为 `D:/my_private_pro/MCode`，分支为 `main`。

## 当前检查点：T8

- [ ] 完成 Resources runtime sentinel：exact DTO/validation、Pack worker、Host task table、deadline/cancel/stale/terminal、generation replacement 与真实 guest 集成测试。
- [ ] 完成其余 family-specific Pack invoke/pull 接线，并统一复用 task/lifecycle 边界。
- [ ] 完成 T8 integration audit 与相关门禁，随后进入 T9。

T8 本次 sentinel 只验证通用 runtime/ownership/task 机制。Resources 跨页一致性、真实资源 UTF-8/EOF、prompt 参数关联等完整 reducer 留在 T14。

## 激活数量

| 类型 | 同时生效数量 |
| --- | ---: |
| Provider Packs | N |
| MCP Packs | N；server/tool identity 全局唯一 |
| Usage Packs | N；按 source identity 隔离 |
| UI runtime | 0..1 |
| Theme / Wallpaper | 0..1；可安装多个候选 |
| Session / Compaction / Resources / Ask / Todo / Web / Subagents / Workspace | 0..1 |

所有激活都绑定 exact Manager generation、configured revision 与 canonical digest。替换必须先停止准入、取消任务并排空旧 generation；trap、timeout 或 future-drop 后不得复用失效实例。

## 后续 TODO

- [ ] T9：Session Manager/Pack 与 Host durable service。
- [ ] T10：签名安装、更新、回滚与 crash-safe WAL。
- [ ] T11：Providers Manager、Pi Pack 与 Synthetic Provider Pack。
- [ ] T12：TUI、UI Manager/Pack 与 generic login flow。
- [ ] T13：Workspace Manager/Pack、checkpoint 与 rollback。
- [ ] T14：Resources Manager/Pack 与完整 stateful reducer。
- [ ] T15：Ask Manager/Pack。
- [ ] T16：Todo Manager/Pack。
- [ ] T17：Web Manager、Querit Pack 与 Synthetic Web Pack。
- [ ] T18：MCP Manager/Pack 与多 Pack 聚合。
- [ ] T19：Usage Manager、Host accounting 与 Usage Packs/widgets。
- [ ] T20：Subagents Manager/Pack。
- [ ] T21：singleton Compaction Manager/Pack。
- [ ] T22：产品 export/import。
- [ ] T23：Core 自动更新。
- [ ] T24：最终产品组合与 headless CLI。
- [ ] T25：删除旧路径的可执行识别、读取与兼容代码。
- [ ] T26：最终文档。
- [ ] T27：Windows/Linux/macOS 安全、发布与 e2e 门禁。
- [ ] final：全项目 audit/cleanup、插件指南、release review，发布 `v0.0.1`。

依赖主线：`T8 -> T9 -> T12`；`T10` 在 T11–T21 前完成；T9–T21 完成后进入 T22–T27 与 final。

## 开发门禁

- 一个完整 feature 一个 commit/push；`plan.md` 状态清理可独立提交。
- 先跑 targeted check，再跑该 feature 相关 format/lint/build/test；提交前审阅 exact staged diff。
- final 前执行 workspace 全量门禁、dead-code/旧路径清理、三平台 CI/e2e 与 secret/provenance/release audit。
- `minimax.txt` 永远不得读取、打印、复制、修改或 stage。
