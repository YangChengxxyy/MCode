# Agent 核心:消息、循环、会话、压缩

> 对应 crate:`mcode-core` / `mcode-llm` / `mcode-compaction` / `mcode-agent` / `mcode-session`

## 1. 消息模型(mcode-core)

```rust
pub enum Message {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
    Custom(CustomMessage),        // 插件自定义:serde_json::Value,序列化透传
}

pub struct AssistantMessage {
    pub blocks: Vec<ContentBlock>,
    pub usage: Option<Usage>,
    pub stop_reason: StopReason,  // Stop | ToolUse | Length | Error
}

pub enum ContentBlock {
    Text(String),
    Thinking(String),
    ToolCall(ToolCall),           // { id, name, arguments: Value }(partial_json 只在 delta 层)
    Image(BinaryData),
}

pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub content: Vec<ContentBlock>,
    pub is_error: bool,
    pub details: Option<Value>,   // 传给渲染层,不进 LLM 上下文
}
```

要点:

- `details` 与 `content` 分离(pi 的 ToolResult 模式):LLM 只看 `content`,UI 渲染拿 `details`(结构化 diff、cwd 等),省 token。
- `CustomMessage` 是 pi declaration merging 的 Rust 替代:插件需要持久化自己的消息类型(如 plan 状态)时走这里,会话 log 原样透传。

## 2. Provider 抽象(mcode-llm)

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;                       // "anthropic" | "openai" | ...
    async fn stream(&self, req: &Request, cancel: CancellationToken)
        -> Result<EventStream, LlmError>;
}

pub struct Request {
    pub model: ModelId,
    pub system_prompt: Vec<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,                   // 由 Registry 序列化出(name/desc/json_schema)
    pub thinking: Option<ThinkingConfig>,
}

pub enum StreamEvent {
    Start, TextDelta(String), ThinkingDelta(String),
    ToolCallDelta { id: String, partial_json: String },
    ToolCallEnd(ToolCall),
    Done { message: AssistantMessage },
    Error(LlmError),
}
```

- `EventStream`:push 模型 + async iterator(参考 pi 的 `EventStream`),Done/Error 后完成。实现为非泛型,item 即 `StreamEvent`;用 `channel_with_cancel` 构造时,取消会先排空队列、再以 `Error(Cancelled)` 终止。
- `LlmError` 变体:`Http { status, body }`(非 2xx;流中途收到 `{"error": …}` 对象时 `status: 0`)、`Transport`(连接层失败,无 HTTP 状态)、`Sse`(畸形帧/载荷)、`Timeout`、`Cancelled`、`Config`(缺 key/坏 base URL 等)。
- OpenAI 兼容实现(openai.rs)刻意容错、对齐 pi:缺失 `finish_reason` 按已聚合内容推断(有工具调用 → `ToolUse`,否则 `Stop`);tool-result 图片作为后续 `user` message 转发(视觉模型模式;模型注册表落地后再按视觉能力门控)。逐条清单见 `openai.rs` 模块文档。
- Provider 本身可通过插件注册(与 pi 一致)——插件系统成熟后,oAuth、自定义模型源都是插件。

## 3. AgentLoop(mcode-agent)

结构直接采纳 pi 的 `runAgentLoop`:外层 drain followup 队列,内层 stream→tool 循环;UI-free、session-free。

```rust
pub struct Agent {
    config: AgentConfig,           // model, thinking, system_prompt
    state: AgentState,             // messages, is_streaming, pending_tool_calls
    steer_queue: VecDeque<Message>,    // 用户打断:当前响应结束后立刻插入
    followup_queue: VecDeque<Message>, // 本要停止时继续推进(subagent 回调、定时器)
    queue_mode: QueueMode,         // All | OneAtATime
}

impl Agent {
    pub async fn prompt(&mut self, msg: Message, env: &TurnEnv) -> Result<TurnOutcome>;
    pub fn steer(&mut self, msg: Message);        // 立即接管
    pub fn follow_up(&mut self, msg: Message);    // 排队续推
    pub fn abort(&mut self);
}

pub struct TurnEnv<'a> {
    pub provider: &'a dyn Provider,
    pub tools: &'a ToolRegistry,
    pub hooks: &'a HookRunner,     // 每个循环节点过钩子(见 03-plugins)
    pub cancel: CancellationToken,
}
```

内层循环伪代码:

```text
loop {
    let req = build_request(&state, tools);           // hooks.transform(before_provider_request)
    let stream = provider.stream(req)?;
    let assistant = collect(stream, |delta| events.emit(delta))?;  // hooks 每个 message part
    state.push(assistant);

    let calls = extract_tool_calls(&assistant);
    if calls.is_empty() { break; }                    // → 外层检查 steer/followUp

    for call in calls {
        let result = tools.dispatch(call).await?;     // 规则 → Gate → Ask；改写后重跑规则/preflight
        let result = hooks.transform(tool_result, result)?;
        state.push(result);
    }
}
```

**steer vs followUp** 是 loop 级能力,不是插件功能:用户 Esc 打断后补一句 → steer;subagent 完成回调 → followUp。

## 4. 会话(mcode-session)

actor 模型(grok-build 的 `ChatStateHandle` 简化版):

```rust
pub struct SessionHandle {
    actor: mpsc::Sender<SessionCommand>,           // Prompt / Steer / FollowUp / Abort / Fork / Resume
    events: broadcast::Receiver<SessionEvent>,     // UI/遥测订阅
}

pub enum SessionCommand {
    Prompt(Message), Steer(Message), FollowUp(Message), Abort,
    Fork { at: MessageId }, Resume { session: SessionId },
    // T1 骨架;权限相关命令随 T3 权限引擎落地
}

pub enum SessionEvent {
    TurnStarted,
    MessageDelta(MessageDelta),   // TextDelta / ThinkingDelta / ToolCallDelta,镜像 §2 StreamEvent
    MessageAdded(Message),
    ToolStarted { call_id: CallId, name: String },
    ToolProgress { call_id: CallId, message: String },
    ToolCompleted { call_id: CallId, result: ToolResultMessage },
    PermissionRequested { request_id: String, tool_name: String, arguments: serde_json::Value }, // T3 细化
    PermissionResolved { request_id: String, allowed: bool },
    TurnEnded(TurnOutcome),       // Completed | Steered | Aborted
    Error(McodeError),            // 字符串载荷,保 Clone + Serialize(broadcast 需要)
    Compacted { before: usize, after: usize },
}
```

**存储格式**(抄 pi v3,带版本头):

```jsonl
{"type":"header","format_version":1,"session_id":"…","cwd":"…","created_at":…}
{"type":"message","id":"a1","parent_id":null,"message":{...}}
{"type":"message","id":"a2","parent_id":"a1","message":{...}}
{"type":"label","id":"a2","label":"探索实现方案"}
{"type":"custom","id":"a3","parent_id":"a2","kind":"plugin:plan","data":{...}}
```

- `parent_id` 构成树 → **fork/分支不建新文件**,tree 命令从同一 jsonl 渲染任意分支。
- 目录:`~/.mcode/sessions/<cwd-slug>/<timestamp>_<uuid>.jsonl`。
- `format_version` 在 header,加载时自动迁移(pi 的教训:第一版就带上)。
- 遥测事件流(可选,独立 `events.jsonl`):TurnStarted/ToolCompleted/PermissionResolved 等,供统计与调试,不影响会话恢复。

## 5. Compaction(mcode-compaction + mcode-session)

压缩继续遵循“与 rewind 分离”原则:压缩只替换模型上下文中的旧历史,文件回滚是另一套。实现位于未发布的 `mcode-compaction` 闭合核心,而不是 `mcode-agent` 中可替换的策略对象；它不提供 `Compactor` trait、registry、callback 或插件 hook。

核心入口是具体函数:`plan_compaction` 纯规划安全 cut，`compact_context` 使用宿主当前选择的 `Provider`/model 生成摘要，`rebuild_context` 重建并复验候选上下文。`CompactionPolicy` 固定 85% 自动压力阈值、reserve/recent/summary 预算和最多三次 provider 尝试；手动触发只绕过压力阈值，不绕过拓扑、预算和版本检查。

自适应触发 foundation 已在同一闭合核心内:`AdaptiveTriggerPolicy`(仅 JSON/serde,无 TOML)区分 advertised 与 effective working context,硬不变量 `effective <= min(advertised, 会话 clamp, maxWorkingTokens, 400_000)` 且设置只能下调 400k;触发点为 `max(1, min(floor(effective*triggerRatio), effective-自适应reserve))`(下限 1 token:合法但极小的 ratio 乘积也不得产生零阈值,否则零使用量也会触发且压缩到零目标后立即重触发),`triggerRatio` 默认 0.82、`targetRatio` 默认 0.55 形成滞回,压缩目标为 `min(floor(effective*targetRatio), threshold-ceil(threshold*minGainRatio))`,reserve 或不确定性折抗压低 threshold 时目标同步下调,始终低于实际触发点并保住最小增益;可信 provider 上报的 total usage 存在时直接取代宿主估算(而非取二者最大值),reserve 由 baseline + 受限的本次请求输出/工具 schema 余量自适应(不直接预留模型宣称的全部 max output)。`evaluate_trigger` 无状态、纯函数,上层可在 provider 上报更小 context length 后下调会话 clamp 并自行做至多一次 compact-and-retry;本 crate 不实现无限重试。

闭合边界与数据规则:

- `CompactionInput` 是不可变快照；实现不选择或切换 provider，也不在失败路径修改 `AgentState`/`SessionStore`。
- prior summary 作为独立输入，只出现一次；字符/token 上限先校验，其 token 会从摘要请求的 transcript 预算中扣除。
- transcript 永不序列化 `Message::Custom`。cut 前的 custom 值在重建时原样保留，不能用非权威模型摘要替代插件持久化状态。
- transcript 优先保留靠近 cut 的较新消息;整条省略的旧消息数和 tool-result 截断记录进入 `CompactionDetails`。段只有写出有意义正文或完整可审计截断标记加闭合标记才算 included(仅 header 不算);tool-result 内层 writer 与外层段预算统一,截断/省略计数来自最终实际输出,预算不足以审计时 fail closed;正文中拼写出的字面 `<<<END MESSAGE>>>` 行以等长转义(`<<<END-MESSAGE>>>`)呈现,不能伪造结构标记或令闭合审计误判;正文渲染按外层 writer 的剩余字符预算封顶(渲染长度统计不分配内存),超大用户/工具正文在截断生效前不会放大内存。
- token 估算在无已知 tokenizer 时采用可证保守上界(UTF-8 字节数)，previous summary 与 fit-to-budget 校验因此不会接受真实可能超限的内容。
- 宿主路径持久化使用 tagged 精确表示(UTF-8 可读形式、Unix 原始 bytes base64、Windows 非法 UTF-16 以 code units base64)，round-trip 精确且无 lossy 碰撞。
- 只有自然结束的 `StopReason::Stop` 可接受；`Length`、`Error`、`ToolUse` 均失败。模型原始输出先校验，再注入宿主生成的 Files/Commands；该确定性 sidecar 才是文件、命令、todo 和后台操作的权威记录。
- 插件不得观察、取消或改写压缩输入、私有 provider request 或候选输出；普通 AgentLoop 的 provider/context/message hooks 不包围压缩调用。

后续 `mcode-session` actor 接入必须事务化:先对快照运行并验证，提交前复查 branch tip/count/cut id，在同一串行临界区先 append 版本化 compaction entry，成功后才安装候选并发出 `SessionEvent::Compacted`；任一步失败都保持原状态。

## 6. 待决策

- [ ] 多 provider 并发(failover/并行采样)是否进 M2?
- [ ] steer 时的 partial assistant message 是否保留进上下文(pi 保留,grok 截断)?
- [ ] Token 计数器:各家 tokenizer 差异大,M1 用字符估算 + provider usage 回读?
