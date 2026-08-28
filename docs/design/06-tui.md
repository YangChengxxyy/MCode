# Interactive UI Pack 与终端 Host substrate

> 状态：**冻结目标**。产品 UI 不是 Agent Core；当前 TUI 基础代码不等于 `com.mcode.ui` 已实现。

## 1. T12 的范围

T12 只交付 interactive TUI 与下列产品 Feature：

- Manager：`com.mcode.ui`
- first-party 源 Pack：`ui_plugins/mcode`
- Host 服务：`UiPackService`

布局、编辑器、scrollback、主题、交互式命令、picker、通知和 interactive 渲染属于 UiPack 语义。UiPack 缺失、签名/trust 失败或 world/version 不匹配时，interactive TUI 必须不可用并提供安装指引；Core/Host 不提供内建 TUI 替代品。

T12 **不**交付 headless login/logout、provider/model管理或非交互run/resume，也不把headless作为UiPack的产品fallback。

## 2. Host terminal substrate

Host 只提供终端生命周期获取/恢复、能力探测、尺寸、受控输出、控制序列清理、ASCII 降级、资源上限和取消。UiPack 不获得 raw terminal、WASI、filesystem、process、socket、credential 或 OS handle；它只收发有界 typed UI DTO。Host 不根据 UI DTO 决定产品策略。

```text
用户输入
  │ typed UI DTO
  ▼
com.mcode.ui Manager ─► Manager Plugin gateway ─► UiPackService ─► UiPack
                                                         │
                                                         ├─► Session/Provider/Usage typed Service
                                                         └─► AskPackService
```

跨 feature 调用只能经过各自 Host-owned typed Service，不能通过共享对象、terminal callback、generic JSON、raw handle 或直接 Pack 引用。

## 3. Ask、动态 UI 与异步 feature

Ask 是 `com.mcode.ask` Feature，不是 Core 权限系统。AskPack 定义请求、选项、取消、超时和结果；UiPack 只显示并回传已验证的 typed DTO。没有 `PermissionEngine`、Core Ask、grant、`--yolo`、按工具名特权或 UI 默认授权。

Manager+Pack 的 UI/command contribution 必须是有界、typed、带 Manager/Pack provenance、family、active hash 和 generation 的 descriptor，由 Host adapter 接入。UI DTO 不允许通用 JSON 或 opaque payload 逃生舱。

Web、MCP、AgentRun/Subagents 的进度只来自其 Manager gateway 和 typed Service 的结果；UiPack 不启动 direct task、不订阅 direct capability，也不拥有第二条后端。

## 4. T24 typed headless CLI

T24 在所需 Providers 与 Session Manager/Pack/typed Service 已安装后，提供 headless login/logout、provider/model管理及非交互run/resume。命令经相同的Manager-bound typed Host Service运行，不直接调用Pack；它们不属于UiPack，也不因UiPack缺失而被阻断。

反之，缺少所需 Providers 或 Session Manager/Pack、签名/trust/hash 不匹配、generation 过期或 DTO 无效时，headless 命令必须 fail closed 并给出安装指引。它不是 interactive TUI 的降级或替代路径。

## 5. 当前实现状态（非目标）

当前 `main` 的 TUI/终端代码仅是迁移前实现状态，不是 UiPack 的私有 first-party 通道。`com.mcode.ui`、`UiPackService`、`ui_plugins/mcode`、T12 interactive 目标与 T24 typed headless CLI 均未因本文而声称已落地。

目录/安装权威性见 [03-plugins.md](03-plugins.md)，world 与 terminal/Service 安全契约见 [05-plugin-impl.md](05-plugin-impl.md)。
