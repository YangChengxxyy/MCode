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

- [ ] 根 `Cargo.toml`(workspace members:6 个 crate)、`rust-toolchain.toml`(stable,edition 2024)
- [ ] `.gitignore`(target/)、`rustfmt.toml`、`clippy.toml`
- [ ] 空 lib 骨架 × 6 + `mcode` bin 占位
- [ ] `deny.toml` 暂缓;`cargo test --workspace` 绿(空跑)

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
| `openai.rs` | OpenAI 兼容 provider:`reqwest` + SSE 解析;tool_calls 增量聚合(`toolCallDelta` → 完整 `ToolCall`);`OPENAI_BASE_URL`/`OPENAI_API_KEY` |
| `fake.rs` | `FakeProvider`:脚本化响应序列(录制的 `Vec<AssistantMessage>`),供全部下游测试 |
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
| `permission.rs` | `PermissionEngine`:规则表解析 + glob 匹配(02 §5,只做规则级;ask 一律先返回 Ask) |
| `builtin/read.rs` `write.rs` `edit.rs` `bash.rs` `grep.rs` | 五件套;edit 用 hashline 锚点或唯一字符串替换(M1 从简:唯一字符串,失败要求更多上下文) |

测试:每工具独立单测(tempdir);Registry 覆盖语义;权限规则匹配表驱动测试;bash 超时 + 输出截断。

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
6. 工具错误(含权限 Deny)作为 `is_error` ToolResult 回填,不中断 loop

## T5 — mcode-session:actor + JSONL(2d)

| 文件 | 内容 |
| --- | --- |
| `store.rs` | JSONL 读写:header(`format_version=1`)+ entries(`id`/`parent_id`);append-only,fsync 策略 = 每条 entry flush |
| `tree.rs` | parent_id 树操作:fork(从任意 entry 分新支)、latest branch、按分支重放消息序列 |
| `actor.rs` | `SessionActor` + `SessionHandle`(01 §4);命令:Prompt/Steer/Abort/Fork/Resume;事件 broadcast |
| `paths.rs` | `~/.mcode/sessions/<cwd-slug>/` 规则 + `$MCODE_HOME` 覆盖 |

测试:写入 → 重载字节等价;fork 后两分支独立 append;resume 重放得到与内存一致的 Message 序列;损坏行容错(skip + warn)。

## T6 — mcode-cli:headless 闭环(1–2d)

- [ ] clap:`mcode run "<prompt>"`、`mcode resume <id|latest>`、`--model`、`--cwd`、`--fake <script.json>`(测试注入 FakeProvider)
- [ ] 流式输出:TextDelta 直接写 stdout;工具调用打印一行摘要(name + args 截断);ToolResult 打印 status 行
- [ ] 权限:M1 headless 下 `Ask` → 读 stdin 一行 y/n(超时 30s 按 deny);`--yolo` 跳过
- [ ] e2e 测试:`assert_cmd` + `--fake` 脚本跑完整会话,断言 stdout 序列 + 会话文件生成 + resume 续跑

## 里程碑验收脚本(M1 DoD)

```bash
cargo test --workspace && cargo clippy --workspace -- -D warnings
MCODE_FAKE=fixtures/demo.json mcode run "读取 Cargo.toml 并总结"   # 多轮工具调用
mcode resume latest "继续"                                         # 树恢复 + 续推
ls ~/.mcode/sessions/**/\*.jsonl                                   # 格式含 format_version
```

## 风险与预留

| 风险 | 对策 |
| --- | --- |
| OpenAI tool_calls 分片聚合边界 | T2 fixture 覆盖全部分片形态(fixture 从真实响应录制) |
| edit 工具的误替换 | M1 唯一字符串约束 + 失败回详细错误让模型重试;hashline 模式后置 |
| EventStream 背压 | M1 unbounded;内存问题出现再加 bounded + drop 策略(记 ADR) |
| bash 安全 | M1 起就走 PermissionEngine,默认规则:`bash(*)` → Ask |

## M1 之后立刻要做的(M2 衔接)

- T3 的 permission.rs 预留 hook 调用点(空 HookRunner 占位,M2 填 shell 钩子)
- T5 的 entry 类型预留 `{"type":"custom"}`(插件 CustomMessage 用)
- CLI `--fake` 机制是后续所有 e2e 测试的地基,勿删
