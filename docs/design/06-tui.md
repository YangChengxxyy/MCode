# TUI 设计

> 定位:MCode = **pi 的 Rust 重实现**。功能面对齐 pi(编辑器、命令、渲染、主题、会话管理),**模块/进程架构对齐 grok-build pager**(AppView 根组件、actions/effects 分层、consent 弹窗、headless 变体)。
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
- 渲染内容一律是 `Renderable` 描述(02 §4),TUI 适配器是消费方之一
- 收益:headless 免费(换 NullUi);将来要拆 ACP 双进程时,把两条通道换成 ACP transport 即可,`AppView` 不感知

## 2. 模块划分(抄 grok pager 的形状)

```text
crates/mcode-tui/src/
├── app_view.rs        # AppView:根组件,唯一持有全局状态;handle_input()/draw() 两个入口
├── actions.rs         # Action 枚举 + ActionRegistry(id、快捷键、when 上下文谓词)
├── effects/           # 副作用层:定时器、异步任务、外部编辑器、clipboard
├── scrollback/        # 滚back 区:block/entry/render/layout/search/selection
├── input/             # LineEditor、KeyboardNormalizer、autocomplete、粘贴/bracketed-paste
├── views/             # prompt 输入框、welcome、session picker、model picker
├── modals.rs          # consent(权限确认)、trust、命令面板等模态
├── notifications/     # toast 通知服务
├── slash/             # /命令解析与补全
├── themes/            # 主题加载与配色
└── headless.rs        # 非 TTY/CI 模式:同一 UiPort,纯文本输出
```

核心形状(grok `app_view.rs` 的纪律,直接抄):

```rust
/// 根组件。event loop 只调这两个方法,对输入路由/模态/视图内部零感知。
pub struct AppView {
    state: UiState,
    actions: ActionRegistry,        // ActionId + When(上下文谓词)→ 可配置键位的基础
    consent: ConsentState,
    editor: LineEditor,
    scrollback: Scrollback,
}

impl AppView {
    pub fn handle_input(&mut self, ev: crossterm::event::Event) -> Vec<Effect>;
    pub fn apply_event(&mut self, ev: SessionEvent);   // 引擎 → UI 的唯一入口
    pub fn draw(&mut self, frame: &mut Frame);
}
```

- **Effect** 是唯一出副作用的门:`SendCommand(SessionCommand)`、`Spawn(task)`、`CopyToClipboard`、`OpenEditor`…纯数据,event loop 统一执行 → AppView 可单测(喂 input/event 断言 effect,不需要终端)
- **actions + when 谓词**(grok `ActionRegistry`/`When`):所有键位走注册表而非硬编码 `KeyCode` 匹配 → 键位可配置(对应 pi 的 `DEFAULT_EDITOR_KEYBINDINGS` 纪律:不写死 key 检查)

## 3. 功能面清单(对齐 pi)

| 域 | 功能 | 说明 |
| --- | --- | --- |
| 编辑器 | 多行 LineEditor、undo、外部编辑器($EDITOR)、bracketed paste、图片粘贴 | pi editor;grok `input/` + `external_editor.rs` |
| 滚back | Markdown 渲染、代码高亮、diff 块、工具调用折叠/展开、选择复制、搜索 | pi markdown/工具渲染;grok `scrollback/blocks` |
| 工具渲染 | 经 `Renderable` 描述:Markdown/Diff/Tree/Table/Widget | 02 §4;插件可扩展 |
| 模态 | **consent(权限确认)**、trust 确认、session/model picker、help | grok `consent.rs` 模式:body 限 12 行/76 列,失败 fail-open |
| 命令 | `/model` `/session` `/theme` `/reload` `/plugin` … + 插件注册命令 | 03 §4.3 |
| 状态栏 | 模型、token 用量、thinking 档位、插件状态、cwd/git | pi footer |
| 会话 | resume picker、fork/tree、export | 01 §4 JSONL 树直接支撑 |
| 主题 | json 主题文件,`~/.mcode/themes/` + 插件贡献 | pi themes |
| 通知 | toast(权限解决、插件加载失败、任务完成) | grok `notifications/` |

## 4. 事件流(引擎 → UI)

```text
SessionEvent::MessageDelta   → scrollback 追加流式文本(增量渲染)
SessionEvent::ToolStarted    → 工具块占位(折叠态,显示 call 渲染描述)
SessionEvent::ToolProgress   → 工具块内进度行
SessionEvent::ToolCompleted  → 渲染 ToolResult.details 的 Renderable
PermissionRequested          → consent 模态弹出(输入焦点接管,回答 → SessionCommand::ResolvePermission)
TurnEnded                    → 状态栏 usage 更新、editor 解锁
```

UI 侧只有一个订阅者(UiPort 实现),事件→`apply_event`→state mutation→draw。**UI 永远不回调引擎**,只发 `SessionCommand`。

## 5. consent 模态(grok 模式,重点抄)

- 触发:`SessionEvent::PermissionRequested { tool, args_preview, rules_matched }`
- 展示约束:body ≤12 行、标题 ≤78 列——小终端不可读就不可接受(grok 的注释原话:unreadable notice cannot be accepted)
- 选项:`允许一次 / 本会话允许 / 总是允许(写规则) / 拒绝`
- 失败路径 fail-open 到 `拒绝`(引擎侧),UI 崩溃不阻塞引擎(回答走 oneshot channel,超时按 deny)

## 6. headless

`headless.rs` 实现同一 `UiPort`:SessionEvent → 行式文本(stdout);permission ask → settings 默认策略。CI/脚本场景与 TUI 共用全部引擎代码。grok pager 的 `headless/` 即此模式。

## 7. 依赖

ratatui 0.29 + crossterm 0.28(grok 同款版本线);syntect(代码高亮);pulldown-cmark(markdown);unicode-width/segmentation。

## 8. 待决策

- [ ] 流式渲染节流策略(每个 delta 都 draw vs 16ms 合帧)
- [ ] 超大工具输出(>1MB)在 scrollback 的分页/截断策略(grok pager 有 scratch buffer 机制,可参)
- [ ] 图片粘贴渲染:inline media 是否第一版就做(grok 有 ffmpeg inline media;建议 M4 再议)
- [ ] 双进程 ACP 拆分的触发条件(性能?远程?);v1 单进程已写入 00
