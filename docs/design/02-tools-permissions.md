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
#[non_exhaustive]
#[serde(tag = "type", content = "content", rename_all = "snake_case")]
pub enum RenderBlock {
    Text(String),
    Markdown(String),
    Diff(Diff),                 // Diff { path, hunks: Vec<DiffHunk> }
    Table(Table),               // Table { caption, headers, rows }
    Tree(Tree),                 // Tree { root: TreeNode };节点可递归
    Progress(Progress),         // label/current/total/state
    Error(ErrorBlock),          // title/message/details/retryable
    Widget(serde_json::Value),  // 插件自定义 widget
}
```

`RenderBlock` 是可 serde 序列化的纯数据,不含回调、终端句柄或后端样式。每个变体都实现 `to_plain_text(width)`:清除 ANSI/OSC 等终端控制序列,按 `unicode-width` 的整串显示宽度截断,并在固定标量预算内保留扩展字素边界;单行宽度不超过 `min(width, 4096)`,总行数不超过 4096,截断标记为 `…`。`Widget` 不可识别时输出格式化 JSON,因此 headless/ACP/web UI 适配器始终有确定的文本降级。

ratatui 适配器位于 `mcode-tui::render`。当 `TerminalCapabilities::supports_unicode()` 为 `false` 时,该适配器进一步使用 ASCII 框线,把非 ASCII 内容替换为 `?`,并把截断标记改为 `.`,保证交给终端的整个缓冲区只含 ASCII。

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

`bash` 是兼容性工具名,继续使用 `command`/`timeout_secs` 和既有权限规则;实际后端为平台 shell:macOS/Linux 按 `/bin/bash` → PATH `bash` → `sh` 选择,Windows 只支持 PowerShell 7 `pwsh.exe`,`details.shell` 固定记录该可执行文件,没有其他 Windows shell 兼容分支。PATH 缺失时按仓库 JSON matrix 固定的版本、架构、asset URL/字节数/SHA-256,按需把 Microsoft 官方 portable ZIP 配置到 `<mcode-home>/bin/powershell/`;仅允许 HTTPS,限制重定向、总时长、下载量、entry 数和解压量,校验 archive SHA-256 与必需 signed runtime chain 的 Authenticode,拒绝 traversal/链接/重复路径,在跨进程锁下 staging 后原子 rename。安装记录保存完整文件清单、大小、mtime 与逐文件 SHA-256;复用时拒绝缺失、额外或大小变化的文件,始终重算必需 runtime 文件并在 mtime 变化时重算其他文件,因此 `pwsh.dll` 等依赖缺失或损坏会触发重建。离线且没有有效缓存时失败关闭。配置单测只注入本地 ZIP;Windows real-shell e2e 仅在 PATH 已有可用的 `pwsh.exe` 时运行,默认测试不会触发下载。

Windows 将用户脚本本身直接编码为 UTF-16LE Base64 后交给 `-EncodedCommand`,不再执行 `UTF8Encoding::new`、`ScriptBlock::Create`、`Encoding.GetString` 或 `Convert.FromBase64String` launcher;因此 `using namespace/module/assembly` 仍是脚本首条,ConstrainedLanguage 允许的基础 cmdlet 也不会被 transport 阻断。`-ExecutionPolicy Bypass` 只处理 execution policy,不假定其解除 WDAC/AppLocker 的 language mode。命令行预算精确计入可执行路径、固定参数、空格、Base64 payload 和 UTF-16 NUL;输出只按 UTF-8/带 BOM UTF-16 安全解码,不使用 ANSI code page。

`timeout_secs` 从工具调用开始计时,覆盖按需配置、install lock 等待、下载与命令执行;预先取消的调用在 shell lookup、配置和 spawn 之前返回。解压、完整缓存校验、Authenticode 与原子发布统一在 blocking task 中执行,外层可在 await 期间观察 deadline/取消并丢弃后续 shell-spawn continuation,因此不会越过已到达的超时/取消启动用户脚本。blocking task 本身不可强制中止,会持有 staging 与 install lock 到结束并可能在后台完成缓存。超时/取消时,Unix 先并发排干 stdout/stderr,之后才 poll/reap leader;只要脱组后代仍持 pipe,live/zombie leader 就继续保留 PID/PGID。终止前必须同时满足当前 `Child::id` 等于保存的 leader、`getpgid(pid)` 等于保存的 PGID、PGID 大于 1 且不是调用者组,否则拒绝 `killpg` 并只走 Child fallback。`setsid` 等脱组进程不在终止保证内,读端会在 containment 清理后关闭。

Windows 以 `CREATE_SUSPENDED` 启动 shell,优先让它继承宿主 Job 并加入 Windows 8+ nested dedicated Job;仅 nesting 失败时尝试 `CREATE_BREAKAWAY_FROM_JOB`,且任一路径都必须在 resume 前成功 assign。专用 Job handle(启用 `KILL_ON_JOB_CLOSE`)是后代 containment/termination 的唯一 authority:超时/取消执行 `TerminateJobObject`,再通过 Tokio 持有的 child process handle kill/reap leader;不使用 `taskkill`、`OpenProcess(PID)` 或任何裸 PID tree cleanup。PID 只在 suspended child 的稳定 process handle 尚在且未 wait/kill 时用于枚举初始线程并立即 `ResumeThread`。通过外部 broker/service 创建、未继承该 Job 的进程不在保证内。

## 7. 待决策

- [ ] `concurrency: Exclusive` 工具(bash)与 parallel 工具的混合调度策略
- [ ] MCP 工具进 Registry 时的名称前缀规则(`mcp:server:tool`?)与本地重名冲突
- [ ] 权限规则的 arg_pattern 语法:glob 够不够,还是要 minimatch/regex 双模式
