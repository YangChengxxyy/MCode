# Interactive UI Pack 与终端 Host substrate

> 产品 UI 不是 Agent Core。UI family 的 active set 固定为一个 product UiPack 加 `N` 个 Theme-role Pack；Theme 只提供 style tokens。
>
> 本文描述首个开发者预览的 UI target：MCode product/workspace release、UI Manager、product UiPack 与第一方 Theme-role Pack package/release 均为 sole-current `0.0.1`，UI 的 MCode-owned FeaturePack world/interface 同为 `@0.0.1`。不保留历史 UI surface、alias、dual-read、fallback 或 coexistence；这不表示 UI runtime 已实现。

## 1. UI family

`ui` 是 12 个 MCode-owned top-level Manager family 之一。第一方唯一来源为 `https://github.com/MCapricorns/MCode_plugins` 的 `plugins/ui/manager/` 与 `plugins/ui/packs/<pack-id>/`。Core/Host 只装载 UI Manager；Manager 只能经 `UiPackService` 激活自身 signed nested Pack。UiPack 不进入根 `plugins.json`，Manager 不读取 installation state/payload 或执行验签，Host 不扫描或直接加载 Pack。

UiPack 定义布局、composer 展示与 semantic submit、scrollback、交互式命令、picker、通知和 interactive rendering；Host 独占本地 editor buffer、caret、selection、paste/IME 与 UTF-8 boundary。UiPack 缺失、签名/trust/world/version 不匹配或 generation 过期时，interactive UI 不可用并显示安装指引；Core/Host 不提供内建替代 UI。

## 2. Host terminal substrate

Host 独占 terminal lifecycle、能力探测、尺寸、focus/input、paste/IME、受控输出、控制序列与 bidi 清理、ASCII 降级、资源上限、clipboard capability 和取消。UiPack/Theme Pack 不获得 raw terminal、WASI、OS、filesystem、network、process、socket、credential 或 raw handle，只交换有界 typed UI DTO。

image、true-color、hyperlinks 的能力分别为 `Auto|ForceOn|ForceOff`；root 的显式设置优先于探测。每次输出必须保持 UTF-8 boundary，分块不超过 `1 MiB`。远端文本一律视为 untrusted 并清理 control/bidi；诊断不记录原文。clipboard 仅在 active selection 后经 Host capability 使用。

## 3. 跨 family 和 usage UI

```text
User input -> typed UI DTO -> UI Manager -> gateway -> UiPackService -> UiPack
                                                     -> typed Host services -> Session/Provider/Usage/Ask
```

UiPack 不得以共享对象、direct Pack reference、terminal callback、generic JSON 或 raw draw 直接跨 family 调用；所有调用经相应 Host-owned typed Service。Ask 是独立 `ask` family，UiPack 只显示并回传验证后的 typed DTO，不提供授权或 grant 语义。

Usage Manager 按根配置顺序组合 unique-source Usage Pack 的有界贡献；UiPack 只布局固定 `status.trailing/usage.summary` 与 `panel/usage.details`。Usage Pack 不能 custom draw 或抢 slot；Theme Pack 不改变数据或行为。

Web、MCP、Subagents 的进度同样只来自其 typed Service。UI 不启动 direct task、订阅 direct capability 或保有第二条后端。

## 4. Typed headless CLI

T24 的 headless account/provider/model 执行与恢复使用与 interactive UI 相同的 Broker、Providers、Session typed Host services，但不属于 UiPack，也不以 UiPack 缺失为 fallback。secret 只能经 typed stdin 或 anonymous pipe 进入 generic hidden-secret interaction，绝不进入 argv、environment、terminal echo 或 guest DTO。缺少所需 Manager/Pack 或 authority/generation/DTO 无效时必须 fail closed 并显示安装指引。
