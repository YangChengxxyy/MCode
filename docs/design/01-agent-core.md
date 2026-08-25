# Agent 核心:消息、循环、会话、压缩

> 对应 crate:`mcode-core` / `mcode-llm` / `mcode-agent` / `mcode-session`

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
        hooks.gate(tool_call)?;                       // 可改写参数 / cancel
        let result = tools.dispatch(call).await?;     // 权限三级求值在 dispatch 内
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

## 5. Compaction(mcode-agent)

独立策略对象,参考 grok-build 的 `xai-grok-compaction` 的"与 rewind 分离"原则:压缩只动消息历史,文件回滚是另一套。

```rust
pub struct CompactionPolicy {
    pub trigger: Trigger,           // TokenRatio(f32) | Manual | TurnCount(usize)
    pub keep_last_turns: usize,
}

pub trait Compactor: Send + Sync {
    async fn compact(&self, history: &[Message]) -> Result<Vec<Message>>;
    // 默认实现:LLM 摘要替换旧消息;策略中保留最近 N 轮原文
}
```

生命周期配钩子:`before_compact`(Gate,可取消)→ compact → `after_compact` + `SessionEvent::Compacted`。

## 6. 待决策

- [ ] 多 provider 并发(failover/并行采样)是否进 M2?
- [ ] steer 时的 partial assistant message 是否保留进上下文(pi 保留,grok 截断)?
- [ ] Token 计数器:各家 tokenizer 差异大,M1 用字符估算 + provider usage 回读?
