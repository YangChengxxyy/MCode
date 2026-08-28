# TUI 设计

> 定位:MCode = **pi 的 Rust 重实现**。功能面对齐 pi(编辑器、命令、渲染、主题、会话管理),**模块/进程架构对齐 grok-build pager**(AppView 根组件、actions/effects 分层、host interaction 弹窗、headless 变体)。
> 对应 crate:`mcode-tui`(+ `mcode-render` 渲染描述)

## 1. 关键架构决策:单进程,边界协议化

grok-build 是 pager(TUI 进程)↔ shell(引擎进程)走 ACP 的双进程架构。MCode **v1 单进程**,但内部强制同一条缝:

```text
┌─ mcode-tui ──────────────────────┐        ┌─ mcode-session ─┐
│ AppView(状态+输入路由+draw)     │  cmds  │ SessionActor    │
│   ├ handle_input → Action        │ ─────► │ (mpsc)          │
│   └ draw(frame) ← UiState        │        │                 │
│ UiPort ◄── SessionEvent 流 ◄───────────── │ (broadcast)     │
└──────────────────────────────────┘        └─────────────────┘
```

- UI 与引擎之间**只过 `SessionCommand`(mpsc)和 `SessionEvent`(broadcast)**,不共享内存对象(grok pager↔shell ACP 消息的单进程等价物)
- 渲染内容一律是 `RenderBlock` 描述(02 §4),TUI 适配器是消费方之一
- 收益:headless 免费(换 NullUi);将来要拆 ACP 双进程时,把两条通道换成 ACP transport 即可,`AppView` 不感知

## 2. 模块划分(抄 grok pager 的形状)

```text
crates/mcode-tui/src/
├── app_view.rs        # AppView:根组件,唯一持有全局状态;handle_input()/dispatch()/draw() 三个入口
├── actions.rs         # Action 枚举 + ActionRegistry(id、快捷键、when 上下文谓词)
├── editor.rs          # 多行 grapheme 编辑器(unicode-width);粘贴为 Action/Effect 数据
├── scrollback.rs      # 有界 transcript + viewport/offset 预算 materialize(零宽/零高不扫历史)
├── interaction.rs     # 有界 host interaction/status 纯状态;无可读性则 fail-closed cancel;不解释 option
├── guard.rs           # TerminalGuard RAII: raw/alternate/cursor/bracketed-paste 与 Windows 输出代码页的进入/逆序恢复;已有 active guard 时拒绝再次进入;测试走 mock
├── output_cp.rs       # 可注入的 Windows 控制台输出代码页后端与 UTF-8 RAII lease
├── effects/           # 副作用层:定时器、异步任务、外部编辑器、clipboard(后续)
├── views/             # welcome、session picker、model picker(后续)
├── notifications/     # toast 通知服务(后续)
├── slash/             # /命令解析与补全(后续)
├── themes/            # 主题加载与配色(语义 token 已在 theme.rs)
└── headless.rs        # 非 TTY/CI 模式:同一 UiPort,纯文本输出(后续)
```

核心形状(grok `app_view.rs` 的纪律;当前基础 API 已按此边界落地):

```rust
/// 根组件。event loop 只调用输入、dispatch 和 draw,不执行 Effect。
pub struct AppView {
    state: AppState,
    capabilities: TerminalCapabilities,
    action_registry: ActionRegistry,
    // named_themes/theme/invalidation;interaction/editor/scrollback 为纯状态,不接 Session
}

impl AppView {
    pub fn with_action_registry(self, registry: ActionRegistry) -> Self;
    pub fn handle_input(&mut self, ev: &crossterm::event::Event) -> InputOutcome;
    pub fn dispatch(&mut self, action: Action) -> Vec<Effect>;
    pub fn draw(&mut self, frame: &mut Frame);
}

let registry = ActionRegistry::default().with_binding(
    ActionBinding::new(KeyPattern::exact(KeyCode::Esc, KeyModifiers::NONE), ActionId::Quit)
        .when(When::HELP_VISIBLE),
);
```

- **Effect** 是唯一出副作用的门:当前基础变体是 `Redraw`、`SubmitInput`、`RequestQuit`、`InteractionResolved`;后续的 `SendCommand(SessionCommand)`、`Spawn(task)`、`CopyToClipboard`、`OpenEditor` 也必须保持纯数据,由 event loop 统一执行 → AppView 可单测(喂 input/event 断言 effect,不需要终端)。`TerminalGuard` 负责 raw/alternate/cursor 的 RAII 恢复,测试只用 mock,不在单测里进入真实 raw mode。Windows enter 只把控制台**输出**代码页切到 UTF-8(65001),不修改输入代码页;只有当前 console font 为非 raster 字体(无法取得 font metadata 时则要求已启用 virtual-terminal processing),且代码页查询/切换成功时 `supports_unicode()` 才为 true,能力探测、查询或切换失败均走全 ASCII 渲染。enter 在任何代码页或终端 mutation 前把共享事务(lease、终端阶段状态与 owner)发布到全局 slot,并串行化 acquisition、阶段提交、取消与回滚;异常恢复先取消事务,等待正在执行的 mutation,在返回前还原全部已尝试阶段(包括返回错误但 mutation 可能已生效的阶段),slot 仍仅由进入方释放。`is_restored` 仅在每个终端清理命令均成功且输出代码页责任结束后为 true;任一终端阶段或代码页瞬时恢复失败时为 false,成功重试后为 true,`restore_count` 只计一次终端清理序列。显式 restore、Drop、panic 和异常退出恢复路径幂等还原终端阶段与原输出代码页,失败责任保留供后续路径重试,enter 事务在完整回滚前不释放 slot。
- **ActionRegistry + When**:所有键位先经可注入注册表解析;默认键位只在 `ActionRegistry::default()` 注册。解析分两层:显式 `Exact` 绑定先于 `Text` 字符回退(同层内后注册优先),因此显式 `Ctrl+Alt` 命令绑定不会被文本输入吞掉。`Text` 只接受可打印字符(允许 `Shift`;也接受 Windows 终端把 AltGr 上报为 `Ctrl+Alt` 的组合,如德式键盘的 `@`/`€`/`{}`),单 `Ctrl`、单 `Alt` 等命令修饰键不算文本。`When` 提供 help/input 内建谓词,也可用命名的无捕获函数读取 `AppState` 定义上下文谓词,无需改 `AppView` 输入 API。`Resize` 是几何事件,不属于键位配置,由注册表直接翻译。状态栏/帮助面板的键位提示由同一注册表生成(`hints` 模块),并按当前 `AppState` 评估 `When` 谓词、按派发优先级验证绑定存活(同键后注册覆盖会使被覆盖绑定不再显示):未绑定或当前不生效的动作在状态栏省略、在帮助面板标注 `unbound`,空 registry 不显示内建键位;动态键名(含非 ASCII 绑定字符)在渲染时与其它文本走同一条清理/ASCII 降级路径;`AppView::set_action_registry` 替换注册表会合并 `Content` 失效,事件循环无需等待无关状态变化即可刷新提示;`DetectBackground` 在检测背景实际变化时也总会产生重绘——`Auto` 选择下为 `Theme` 失效(主题重新解析),显式选择下为 `Content` 失效,因为自定义 `When` 可读取检测背景使存活绑定与提示改变,失效驱动的重绘不能停在旧提示上。

## 3. 功能面清单(对齐 pi)

| 域 | 功能 | 说明 |
| --- | --- | --- |
| 编辑器 | 多行 LineEditor、undo、外部编辑器($EDITOR)、bracketed paste、图片粘贴 | pi editor;grok `input/` + `external_editor.rs` |
| 滚back | Markdown 渲染、代码高亮、diff 块、工具调用折叠/展开、选择复制、搜索 | pi markdown/工具渲染;grok `scrollback/blocks` |
| 工具渲染 | 经 `RenderBlock` 描述:Text/Markdown/Diff/Table/Tree/Progress/Error/Widget | 02 §4;插件可扩展 |
| 模态 | **host interaction(有界、纯数据的宿主请求)**、trust 确认、session/model picker、help | `interaction.rs`:body 限 12 行/76 列,不可读时 fail-closed cancel |
| 命令 | `/model` `/session` `/theme` `/reload` `/plugin` … + 插件注册命令 | 03 §4.3 |
| 状态栏 | 模型、token 用量、thinking 档位、插件状态、cwd/git | pi footer |
| 会话 | resume picker、fork/tree、export | 01 §4 JSONL 树直接支撑 |
| 主题 | json 主题文件,`~/.mcode/themes/` + 插件贡献 | pi themes |
| 通知 | toast(插件加载失败、任务完成) | grok `notifications/` |

## 4. 事件流(引擎 → UI)

```text
SessionEvent::MessageDelta   → scrollback 追加流式文本(增量渲染)
SessionEvent::ToolStarted    → 工具块占位(折叠态,显示 call 渲染描述)
SessionEvent::ToolProgress   → 工具块内进度行
SessionEvent::ToolCompleted  → 渲染 ToolResult.details 的 RenderBlock
TurnEnded                    → 状态栏 usage 更新、editor 解锁
```

UI 侧只有一个订阅者(UiPort 实现),事件→`apply_event`→state mutation→draw。**UI 永远不回调引擎**,只发 `SessionCommand`。

## 5. host interaction 模态

- 触发:宿主 `Action::PresentInteraction(InteractionPrompt)`;prompt 只含 opaque `request_id`、title、body 与通用 option,不接 Core 授权引擎。
- `request_id` 与 option ID 在构造时验证非空、无控制字符且不超过 64 个 Unicode scalar;合法 ID 原样存储和回传,显示字段独立做 terminal sanitization 与列/行截断。
- 用户按当前 live digit binding 产生 `InteractionResponse::Selected(option_id)`,按 live cancel binding 产生 `Cancelled`;TUI 不解释 response 的策略含义。
- 展示约束:body ≤12 行、标题 ≤78 列、option ≤9 个;renderer 先保留标题、至少一行 body 与全部 option 行,再按剩余高度裁 body/空白。模态期间 input 固定为单行高度,不可读 viewport 不能接受隐藏 option。
- 失败路径 **fail-closed cancel**:不可读 prompt、被拒绝的第二个 `request_id` 与缩小后的活动 prompt 都发出 `InteractionResolved { response: Cancelled }`;第二个请求不覆盖活动 prompt,每个 ID 恰有一个 resolution。
- `TerminalGuard::enter` 成对发送 `EnableBracketedPaste`/`DisableBracketedPaste`;interaction 模态期间 `Event::Paste` 与文本/退格/换行受 `INTERACTION_HIDDEN` 门隔离,不修改隐藏编辑器。

## 6. headless

`headless.rs` 实现同一 `UiPort`:SessionEvent → 行式文本(stdout)。CI/脚本场景与 TUI 共用全部引擎代码;headless 不等待 Core 授权提示。grok pager 的 `headless/` 即此模式。

能力降级契约:`RenderBlock::to_plain_text(width)` 负责控制序列清理、整串 Unicode 显示宽度、固定标量预算内的扩展字素截断及固定行数/列数预算。TUI 的 `TerminalCapabilities::supports_unicode() == false` 是更严格的终端边界:Logo、两个面板框线、输入、状态和内容都只输出 ASCII;非 ASCII 数据显示为 `?`,截断标记使用 `.`。这保证不支持 Unicode 的终端不会因替换字符破坏布局。

## 7. 依赖

ratatui 0.29 + crossterm 0.28(grok 同款版本线);syntect(代码高亮);pulldown-cmark(markdown);unicode-width/segmentation。

## 8. 待决策

- [ ] 流式渲染节流策略(每个 delta 都 draw vs 16ms 合帧)
- [ ] 超大工具输出(>1MB)在 scrollback 的分页/截断策略(grok pager 有 scratch buffer 机制,可参)
- [ ] 图片粘贴渲染:inline media 是否第一版就做(grok 有 ffmpeg inline media;建议 M4 再议)
- [ ] 双进程 ACP 拆分的触发条件(性能?远程?);v1 单进程已写入 00
