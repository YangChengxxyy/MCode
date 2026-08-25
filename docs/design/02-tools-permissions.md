# 工具系统与权限

> 对应 crate:`mcode-tools`
> 参考:grok-build `mcode-tools` 同构设计(Tool/ToolDyn/schemars/ToolStream)

## 1. Tool trait

```rust
/// 内建工具与 Rust 侧插件工具实现此 trait;async fn in trait(edition 2024)
#[async_trait]
pub trait Tool: Send + Sync + 'static {
    type Args: for<'de> Deserialize<'de> + schemars::JsonSchema;
    type Output: Serialize;

    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn prompt_snippet(&self) -> Option<&str> { None }   // 进 system prompt 的用法提示(pi 的 promptSnippet)

    async fn execute(
        &self,
        args: Self::Args,
        ctx: &ToolCtx,
        out: &mut ToolStream,
    ) -> Result<ToolResult, ToolError>;
}
```

单源 schema:`schemars` 派生一份 JSON Schema,同时用于(1) LLM tool spec;(2) 运行时参数校验。grok-build 已验证此模式。

## 2. 类型擦除与注册

```rust
#[async_trait]
pub trait ToolDyn: Send + Sync {
    fn spec(&self) -> ToolSpec;      // name/description/params_schema
    async fn execute_dyn(&self, args: Value, ctx: &ToolCtx, out: &mut ToolStream)
        -> Result<ToolResult, ToolError>;
}

// blanket impl:任何 Tool 自动成为 ToolDyn(负责 Value → Args 反序列化 + schema 校验)

pub struct ToolRegistry {
    tools: RwLock<HashMap<String, Arc<dyn ToolDyn>>>,
}
// 规则:同名后注册者覆盖(pi 的 last-wins,允许插件覆盖内建工具)
// 能力标记:concurrency: Exclusive | Parallel,mutates_fs: bool → 供调度与权限推断
```

## 3. 流式输出

```rust
pub struct ToolStream { tx: mpsc::Sender<ToolStreamItem> }

pub enum ToolStreamItem {
    Progress(ToolProgress),   // 增量文本/计数,UI 实时渲染
    Terminal(ToolResult),     // 有且仅有一个,流到此结束
}
```

约束(抄 grok-build):`[Progress*]` 后恰好一个 `Terminal`;`ToolResult { content, is_error, details }` 的 `details` 只进 UI 不进 LLM。

## 4. ToolCtx 与渲染描述

```rust
pub struct ToolCtx {
    pub cwd: PathBuf,
    pub session_id: SessionId,
    pub cancel: CancellationToken,
    pub emit_event: Box<dyn Fn(SessionEvent) + Send>,  // 可选:工具发额外 UI 事件
}

// 渲染描述(UI 中立,pi 的 renderCall/renderResult 的协议化):
pub enum Renderable {
    Markdown(String),
    Diff { path: String, hunks: Vec<DiffHunk> },
    Tree { root: String, entries: Vec<String> },
    Table { headers: Vec<String>, rows: Vec<Vec<String>> },
    Widget(serde_json::Value),        // 插件自定义 widget,适配器不识别的降级为文本
}
```

ratatui 适配器把 `Renderable` 画出来;headless 适配器降级为纯文本;将来 ACP/web UI 适配器复用同一格式。

## 5. 权限三级求值(顺序固定)

```text
tool_call 到达 dispatch
  1. PermissionEngine(规则表,无交互)
       pattern 例:"bash(cargo *)→allow", "write(**/*.env)→deny", "bash(rm *)→ask"
       → Allow / Deny / Ask / NoMatch(继续)
  2. HookRunner::gate(pre_tool_use)        ← 插件可阻断/改写参数(03-plugins)
       → Allow(args') / Block(reason) / Pass
  3. Ask → PermissionPrompt 回调到 UI
       TUI 弹确认;headless 走 settings 默认策略(deny/allow-once/allow-session)
  任一 Deny/Block → ToolError::PermissionDenied 回填给模型(不是进程错误)
```

```rust
pub enum PermissionAction { Allow, Deny, Ask, NoMatch }

pub struct PermissionRule {
    pub tool: String,              // "bash"
    pub arg_pattern: Glob,         // "cargo *"
    pub action: PermissionAction,
    pub scope: Scope,              // Project | User | Session | Once
}

// 结果全程遥测:PermissionRequested → PermissionResolved(rule/hook/user, decision)
```

- **yolo 模式**:`settings.permissions.default = "allow"`,跳过 1/3(钩子 2 仍执行)。
- 文件修改工具参与 per-file 串行化队列(pi 的 `withFileMutationQueue` 思想):同一文件写操作排队,避免并发写。

## 6. 内建工具清单(M1)

read / write / edit / bash / grep —— 全部实现 `Tool`,进同一个 Registry,作为插件 API 的 reference 实现。

## 7. 待决策

- [ ] `concurrency: Exclusive` 工具(bash)与 parallel 工具的混合调度策略
- [ ] MCP 工具进 Registry 时的名称前缀规则(`mcp:server:tool`?)与本地重名冲突
- [ ] 权限规则的 arg_pattern 语法:glob 够不够,还是要 minimatch/regex 双模式
