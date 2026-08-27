# M1 实施计划:最小闭环任务拆解

> 里程碑定义见 04-roadmap.md;本文档把 M1 拆成可执行任务。
> 验收标准(04 §M1):无 TUI 终端中完成一次多轮工具调用会话并可 resume。

## 0. 总依赖图

```text
T0 scaffold ──► T1 mcode-core ──► T2 mcode-llm ──► T4 mcode-agent ──► T5 mcode-session ──► T6 mcode-cli
                        └───────► T3 mcode-tools ──┘
```

T2/T3 并行;T4 依赖两者;T5/T6 串行收尾。每个任务交付 = 代码 + 单测 + `clippy -D warnings` 绿。

## T0 — workspace 脚手架(0.5d)

- [x] 根 `Cargo.toml`(workspace members:6 个 lib crate + `mcode` bin)、`rust-toolchain.toml`(stable,edition 2024)
- [x] `.gitignore`(target/)、`rustfmt.toml`、`clippy.toml`
- [x] 空 lib 骨架 × 6 + `mcode` bin 占位
- [x] `deny.toml` 暂缓;`cargo test --workspace` 绿

## T1 — mcode-core:类型(1d)

交付 `crates/mcode-core/src/`:

| 文件 | 内容 |
| --- | --- |
| `message.rs` | `Message` / `UserMessage` / `AssistantMessage` / `ContentBlock` / `ToolCall` / `ToolResultMessage`(01 §1) |
| `events.rs` | `SessionEvent` / `SessionCommand` 枚举骨架(01 §4) |
| `tool.rs` | `ToolSpec { name, description, params_schema: Value }` |
| `ids.rs` | `SessionId` / `MessageId` / `CallId`(newtype,`Display`) |
| `error.rs` | `McodeError`(thiserror) |

测试:全部类型 serde roundtrip;`Message::Custom` 的 Value 透传。

## T2 — mcode-llm:Provider 抽象 + OpenAI 兼容实现(2–3d)

| 文件 | 内容 |
| --- | --- |
| `provider.rs` | `Provider` trait、`Request`、`StreamEvent`(01 §2) |
| `stream.rs` | `EventStream`:push 端(`Sender`)+ async iterator 端,`Done`/`Error` 终止;背压先不做(unbounded) |
| `openai.rs` | OpenAI 兼容 provider:`reqwest` + SSE 解析;tool_calls 增量聚合(`ToolCallDelta` → 完整 `ToolCall`);`OPENAI_BASE_URL`/`OPENAI_API_KEY` |
| `fake.rs` | `FakeProvider`:脚本化响应序列(`ScriptTurn`:消息轮或 error 轮;内联 `Vec` / JSON 字符串 / JSON 文件),供全部下游测试 |
| `auth.rs` | API key 读取:env → `~/.mcode/auth.toml` |

测试:

- `EventStream`:多 producer push、终止后迭代结束、error 传播
- SSE 解析器:fixture 文件(含 tool_calls 分片)回放断言
- `FakeProvider`:按脚本逐轮返回,断言请求内容(messages/tools 序列化正确)

## T3 — mcode-tools:trait + Registry + 内建工具(3d)

| 文件 | 内容 |
| --- | --- |
| `tool.rs` | `Tool` / `ToolDyn` / blanket impl / `ToolResult`(02 §1–2) |
| `stream.rs` | `ToolStream`:`Progress*` + 恰一个 `Terminal`(02 §3) |
| `registry.rs` | `ToolRegistry`:last-wins、spec 列表序列化 |
| `builtin/read.rs` `write.rs` `edit.rs` `bash.rs` `grep.rs` `find.rs` | 六件套;edit 用 hashline 锚点或唯一字符串替换(M1 从简:唯一字符串,失败要求更多上下文);grep/find 按 capability 做 path preflight 与句柄保留执行 |

测试:每工具独立单测(tempdir);Registry 覆盖语义;直接 dispatch(无授权回调);bash 超时 + 输出截断。

## T4 — mcode-agent:双循环(2d)

| 文件 | 内容 |
| --- | --- |
| `agent.rs` | `Agent` + steer/followUp 队列 + abort(01 §3) |
| `turn.rs` | 内层循环:build_request → stream → collect → tool dispatch → 回填 |
| `loop_test.rs` | 集成测试 |

测试(FakeProvider 驱动,无网络):

1. 单轮文本回复即停
2. 工具调用 → 执行 → 回填 → 二轮停止(多轮工具循环)
3. steer:流式中插入,断言插队语义
4. followUp:agent 将停时续推
5. abort: CancellationToken 中途取消,状态一致
6. 工具错误(未知工具/非法参数/取消/执行失败)作为 `is_error` ToolResult 回填,不中断 loop;已注册工具无需授权回调即可执行

## T5 — mcode-session:actor + JSONL(2d)

| 文件 | 内容 |
| --- | --- |
| `store.rs` | JSONL 读写:header(`format_version=1`)+ entries(`id`/`parent_id`);append-only,fsync 策略 = 每条 entry flush |
| `tree.rs` | parent_id 树操作:fork(从任意 entry 分新支)、latest branch、按分支重放消息序列 |
| `actor.rs` | `SessionActor` + `SessionHandle`(01 §4);命令:Prompt/Steer/Abort/Fork/Resume;事件 broadcast |
| `paths.rs` | `~/.mcode/sessions/<cwd-slug>/` 规则 + `$MCODE_HOME` 覆盖 |

测试:写入 → 重载字节等价;fork 后两分支独立 append;resume 重放得到与内存一致的 Message 序列;损坏行容错(skip + warn)。

## T6 — mcode-cli:headless 闭环(1–2d)

- [x] clap:`mcode run "<prompt>"`、`mcode resume <id|latest|path> "<prompt>"`(prompt 必填)、`--model`、`--cwd`、`--fake <script.json>` / `$MCODE_FAKE`
- [x] 流式输出:TextDelta 直接写 stdout;状态行 `==> tool <name> <args≤120>` / `<== ok|error <首行 ≤120>`;thinking/progress/错误走 stderr
- [x] 已注册 schema-valid 工具直接执行;headless 不等待 Core 授权提示
- [x] e2e 测试:`assert_cmd` + `--fake` 脚本跑完整会话,断言 stdout 序列 + 会话文件生成 + resume 续跑

## 里程碑验收脚本(M1 DoD)

```bash
cargo test --workspace && cargo clippy --workspace -- -D warnings
MCODE_FAKE=crates/mcode/tests/fixtures/demo.json mcode run "读取 Cargo.toml 并总结"   # 多轮工具调用
MCODE_FAKE=crates/mcode/tests/fixtures/demo_resume.json mcode resume latest "继续"      # 树恢复 + 续推
ls ~/.mcode/sessions/**/\*.jsonl                                   # 格式含 format_version
```

## 风险与预留

| 风险 | 对策 |
| --- | --- |
| OpenAI tool_calls 分片聚合边界 | T2 fixture 覆盖全部分片形态(fixture 从真实响应录制) |
| edit 工具的误替换 | M1 唯一字符串约束 + 失败回详细错误让模型重试;hashline 模式后置 |
| EventStream 背压 | M1 unbounded;内存问题出现再加 bounded + drop 策略(记 ADR) |
| bash 安全 | 平台 shell 走既有 containment/超时;Core 不做 Ask/Deny 授权 |

## M1 之后立刻要做的(M2 衔接)

- T4 的 HookRunner 占位仍在 loop 节点(notify/transform/gate);插件授权钩子未实现
- T5 的 entry 类型预留 `{"type":"custom"}`(插件 CustomMessage 用)
- CLI `--fake` 机制是后续所有 e2e 测试的地基,勿删
