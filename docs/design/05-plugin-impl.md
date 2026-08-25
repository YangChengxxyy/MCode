# 插件实现:WASM Host/Guest 端到端机制

> 03-plugins.md 定义契约,本文档定义"怎么跑起来"。
> 代码均为骨架级,标注处(`⚠ verify`)以动工时 wasmtime 实际版本 API 为准。

## 0. 全景调用链

```text
hello.wasm
  ① Component::from_file(带 .cwasm 预编译缓存)
  ② Linker 注册 host-api imports(register_tool / on / log / emit_ui)
  ③ instantiate → 调用 export plugin-init(pi 工厂函数的 WASM 对应物)
        └─ guest 回调 host.register_tool(...) → 暂存 PendingRegistrations
  ④ commit → 每个插件工具包 WasmToolAdapter → Arc<dyn ToolDyn> → ToolRegistry
  ⑤ LLM 调用 → Registry → adapter.execute_dyn → call-tool export → JSON 返回
  ⑥ 钩子:HookRunner 遍历订阅者 → on-event export → 解析 hook-result
```

分层原则:**wasmtime 只出现在 `mcode-plugin-host` 的 adapter 里**。loop、Registry、HookRunner 只见 `ToolDyn`/handler,与 Tier 3 MCP 包装器同构。

## 1. 依赖与特性

```toml
# crates/mcode-plugin-host/Cargo.toml(版本以动工时最新稳定为准 ⚠ verify)
[dependencies]
wasmtime = { version = "38", features = ["component-model", "async", "fuel"] }
wasmtime-wasi = "38"
mcode-plugin-api = { path = "../mcode-plugin-api" }
mcode-tools = { path = "../mcode-tools" }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
anyhow = "1"
tracing = "0.1"
```

## 2. Host:加载器

### 2.1 bindgen 绑定

```rust
// mcode-plugin-host/src/wit.rs
// 由 WIT 文件生成类型安全的 import/export 绑定 ⚠ verify:宏路径随版本变
wasmtime::component::bindgen!({
    path: "../mcode-plugin-api/wit/plugin.wit",
    world: "plugin",
    async: true,
});
```

### 2.2 HostState:host-api 的实现端

```rust
// mcode-plugin-host/src/host_state.rs
use wasmtime_wasi::{WasiCtx, WasiView, ResourceTable};

/// 每个插件实例一份。注册先进 pending,init 成功后 commit,失败整体丢弃
/// (pi 的 createExtensionAPI → commit()/discard() 语义)
pub struct HostState {
    pub wasi: WasiCtx,
    pub table: ResourceTable,
    pub pending: PendingRegistrations,
    pub plugin_name: String,
    pub events_tx: mpsc::Sender<PluginHostEvent>,   // 回宿主的日志/UI/命令注册
}

#[derive(Default)]
pub struct PendingRegistrations {
    pub tools: Vec<ToolSpec>,
    pub commands: Vec<CommandSpec>,
    pub subscriptions: Vec<String>,                  // on("tool_call") 收集到的事件名
}

// host-api trait 实现(bindgen 生成 trait 名 ⚠ verify)
impl HostApi for HostState {
    async fn register_tool(&mut self, spec: ToolSpec) {
        // 命名空间前缀,防止插件工具撞内建工具
        self.pending.tools.push(spec.prefixed(&self.plugin_name));
    }
    async fn on(&mut self, event: String) { self.pending.subscriptions.push(event); }
    async fn log(&mut self, level: String, msg: String) {
        tracing::event!(level, plugin = %self.plugin_name, "{msg}");
    }
    async fn emit_ui(&mut self, renderable_json: String) {
        let _ = self.events_tx.send(PluginHostEvent::Render(renderable_json)).await;
    }
    // register_command 同理
}
```

### 2.3 加载与实例化

```rust
// mcode-plugin-host/src/loader.rs
pub struct LoadedPlugin {
    pub name: String,
    pub instance: PluginInstance,
    pub tools: Vec<Arc<dyn ToolDyn>>,
    pub hooks: Vec<(String, HookHandle)>,            // (事件名, 处理器)
}

pub async fn load_plugin(
    engine: &Engine,
    path: &Path,
    limits: &SandboxLimits,
) -> Result<LoadedPlugin, LoadError> {               // 单插件失败 → errors[],不阻塞其余
    let component = Component::from_file(engine, path)?;   // 建议配 ModuleCache 落盘缓存

    let mut linker = Linker::new(engine);
    wasmtime_wasi::add_to_linker_async(&mut linker)?;       // ⚠ verify 函数名
    Plugin::add_to_linker(&mut linker, |s: &mut HostState| s)?;

    let mut store = Store::new(engine, HostState::new(plugin_name(path)));
    store.set_fuel(limits.fuel)?;                           // CPU 预算
    store.limiter(|s| &mut s.mem_limits);                   // 内存上限 ⚠ verify API 名

    let plugin = Plugin::instantiate_async(&mut store, &component, &linker).await?;

    // ③ 工厂调用:guest 在 init 里回调 register_*/on,进 pending
    plugin.call_plugin_init(&mut store).await?;

    // ④ commit:物化注册项
    let pending = std::mem::take(&mut store.data_mut().pending);
    let instance = Arc::new(Mutex::new(PluginInstance { store, plugin }));
    let tools = pending.tools.iter()
        .map(|spec| Arc::new(WasmToolAdapter::new(instance.clone(), spec.clone())) as Arc<dyn ToolDyn>)
        .collect();
    let hooks = pending.subscriptions.iter()
        .map(|ev| (ev.clone(), HookHandle::wasm(instance.clone())))
        .collect();

    Ok(LoadedPlugin { name: plugin_name(path), instance, tools, hooks })
}
```

要点:**`PluginInstance` 被 adapter 和 hook handle 共享**(同一份 store,串行访问用 Mutex;WASM 组件实例本身不可重入，这天然保证了插件状态一致性)。

## 3. Host:两个适配器

### 3.1 WasmToolAdapter(插件工具 → ToolDyn)

```rust
// mcode-plugin-host/src/tool_adapter.rs
pub struct WasmToolAdapter {
    instance: Arc<Mutex<PluginInstance>>,
    spec: ToolSpec,                       // name/description/params_schema(来自 register_tool)
}

#[async_trait]
impl ToolDyn for WasmToolAdapter {
    fn spec(&self) -> ToolSpec { self.spec.clone() }

    async fn execute_dyn(&self, args: Value, ctx: &ToolCtx, out: &mut ToolStream)
        -> Result<ToolResult, ToolError>
    {
        validate_against_schema(&args, &self.spec.params_schema)?;   // host 侧再校验一次

        let mut inst = self.instance.lock().await;
        // ⑤ 调用 export;fuel 耗尽/trap → ToolError::PluginTrap(回给模型,不是崩溃)
        let result_json = inst.plugin
            .call_call_tool(&mut inst.store, &self.spec.name, &args.to_string(), &ctx.call_id)
            .await
            .map_err(|e| ToolError::plugin_trap(e))??;

        // 流式:guest 执行期间可回调 host.emit_ui 产生 Progress;Terminal 由此返回
        Ok(ToolResult::from_json(&result_json)?)
    }
}
```

### 3.2 HookHandle(插件钩子 → HookRunner 处理器)

```rust
// HookRunner 内部对订阅者统一抽象:
pub enum HookHandler {
    Shell(ShellHook),                  // Tier 1 manifest 钩子
    Wasm(Arc<Mutex<PluginInstance>>),  // Tier 2
}

impl HookRunner {
    pub async fn gate(&self, event_name: &str, ev: &mut Event) -> GateResult {
        for sub in self.subscribers(event_name) {   // 按加载顺序(pi 语义)
            let result = match &sub.handler {
                HookHandler::Shell(h) => h.run_gate(ev).await,
                HookHandler::Wasm(inst) => {
                    let mut g = inst.lock().await;
                    // ⑥ trap 隔离:失败记事件 + 本次会话禁用该插件(pi stale-context 语义)
                    match g.plugin.call_on_event(&mut g.store, &to_wit_event(ev)).await {
                        Ok(r) => from_wit_result(r),
                        Err(trap) => { self.disable_plugin(&sub.plugin); GateResult::Pass }
                    }
                }
            };
            match result {
                GateResult::Block(reason) => return GateResult::Block(reason),  // 短路
                GateResult::Modified(new) => *ev = new,                          // 链式传递
                GateResult::Pass => {}
            }
        }
        GateResult::Pass
    }
    // notify / transform 同构,仅聚合语义不同(03 §4.1)
}
```

## 4. Guest:Rust SDK(cargo-component 模板)

`mcode-plugin-sdk`(guest 侧 crate)封装 bindgen + 回调注册表，让插件作者写声明式代码：

```rust
// guest SDK 内部(展开原理)
wit_bindgen::generate!({ path: "wit/plugin.wit", world: "plugin" });

static REGISTRY: OnceLock<Registry> = OnceLock::new();      // wasm 单线程,可用 thread_local

#[macro_export]
macro_rules! plugin {
    ($init:expr) => {
        struct Guest;
        impl exports::Guest for Guest {
            fn plugin_init() {
                let mut api = $crate::PluginApi::default();
                ($init)(&mut api);                    // 收集注册进 REGISTRY
            }
            fn on_event(e: Event) -> HookResult {
                $crate::REGISTRY.dispatch_hook(e)     // 按事件名路由到注册的闭包
            }
            fn call_tool(name: String, args: String, _id: String) -> Result<String, String> {
                $crate::REGISTRY.dispatch_tool(&name, &args)
            }
        }
        export!(Guest);
    };
}

// 插件作者视角:
mcode_plugin_sdk::plugin!(|api| {
    api.on("tool_call", |ev| { /* … */ HookResult::pass() });
    api.register_tool("todo", |args| { /* … */ Ok(json!({"todos": []}).to_string()) });
});
```

**异步问题**:WIT 同步 export 内跑不了 tokio。guest SDK 内置一个 mini executor(`futures::executor::block_on` 或 pollster),插件作者写 `async fn`,SDK 在 export 边界 block_on。宿主侧调用保持 async。这是 WASM 插件体验上最大的坑，SDK 必须吃掉它。

## 5. 热重载(pi `/reload` 语义)

```text
mcode plugin reload <name>
  ① 对订阅了 session_shutdown 的该插件发 notify(优雅退出机会)
  ② HookRunner 摘除其订阅;Registry 摘除其工具(前缀匹配)
  ③ drop PluginInstance(store + instance 一起释放,WASI 资源随 table 回收)
  ④ load_plugin 重跑(③→④ 间请求中的工具调用返回明确的 "plugin reloading" 错误)
  ⑤ 失败则回滚:旧实例其实已释放 → 记 errors[],插件呈"未加载",不自动复活旧版
```

## 6. 沙箱配置(默认拒绝,声明开通)

```rust
pub struct SandboxLimits {
    pub fuel: u64,                    // 默认 50M 指令当量/调用
    pub max_memory: usize,            // 默认 64 MiB
    pub allow_net: Vec<String>,       // capability 声明的域名白名单,默认空
    pub fs: FsPolicy,                 // 默认 ReadWrite(cwd) only;manifest 声明可扩大
}
```

- WASI preopen 只挂 cwd(及 manifest `capabilities.fs` 显式声明的路径)
- 网络走 `wasi:http` import,宿主在 Linker 里按 `allow_net` 拦截
- manifest 声明了但 trust 不足 → 加载拒绝,进 errors[](能力声明 × trust 是叉乘)

## 7. Tier 3(MCP)如何复用同一套

`McpToolAdapter: ToolDyn` 与 `WasmToolAdapter` 并列,内部是 stdio JSON-RPC client;MCP 没有钩子语义,所以只产出工具不产出 HookHandle。HookRunner 对两者无感。

## 8. M3 任务拆解(把本文档落到 roadmap)

1. WIT v0.1 定稿(plugin.wit)+ bindgen 打通 hello 插件
2. HostState + PendingRegistrations + commit/discard
3. WasmToolAdapter 进 Registry(e2e:LLM 调用到插件工具)
4. HookRunner 三种语义 + Wasm HookHandle + trap 隔离
5. guest SDK(plugin! 宏 + mini executor)
6. 热重载 + errors[] 状态页
7. 沙箱默认值 + capability 声明
