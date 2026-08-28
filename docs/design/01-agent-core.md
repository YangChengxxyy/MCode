# 最小 Agent Core 与产品边界

> 状态：**冻结目标**。Core 只保留 loop 与七个 builtin；Host adapter 属于 substrate。Core 不拥有任何产品 Feature 的持久化、选择、策略或生命周期。

## 1. Agent loop

Core 只消费稳定的消息、tool call 和流式结果 DTO。它构造请求、消费 ProviderPackService 的流、直接执行 canonical builtin，并把经 Host adapter 验证的动态调用转交其所有者的 typed Service。

```text
loop {
    request = build_typed_request(state, tools)
    assistant = collect(ProviderPackService.stream(request))
    state.push(assistant)

    for call in tool_calls(assistant) {
        state.push(dispatch_canonical_or_host_adapter(call))
    }
    drain_steer_or_follow_up()
}
```

七个 builtin 为 `read`、`write`、`edit`、`find`、`grep`、`exec`、`shell`，且名称保留、不可覆盖。动态工具只能是 Manager+Pack 的有界 typed contribution，经 Host adapter 取得 namespaced 名称；它们不是 Core builtin，也不能绕过 [02-tools-permissions.md](02-tools-permissions.md) 的 schema、preflight、取消和 OS 安全契约。

Core 不解析 provider wire/profile、不读 credential、不选择 Provider。它没有 `PermissionEngine`、Core Ask、grant 或 `--yolo`；调用者 capability/family 绑定是 Host 的技术边界，不是用户授权策略。

## 2. Session

Session 是独立产品 Feature：

- Manager：`com.mcode.session`
- first-party 源 Pack：`session_plugins/mcode`
- Host 服务：`SessionPackService`

`SessionPackService` 是唯一可以写 Session durable bytes 的入口。数据只进入 `~/.mcode/session_plugins/<pack-id>/data/`；Host 将每次访问绑定并校验 Pack ID/version/hash/generation，不规定额外磁盘子目录协议。没有全局 Session 根、Host session tree 或 lazy directory bootstrap。

Host 只提供 no-follow owned storage、bounded WAL、atomic append、durability、backpressure、generation fence 与 DTO 验证。SessionPack 独自定义 session/event/branch/resume/rewind/rollback 语义、版本和恢复规则；Host 不能解释 durable bytes、恢复会话或在 Pack 缺失时回放。

## 3. Workspace 与 Compaction

- Workspace checkpoint/rollback：`com.mcode.workspace` + `workspace_plugins/mcode` + `WorkspacePackService`。Core 不保存 checkpoint、解释 rollback 或由文件工具推导 fallback。
- Compaction：Host-wide singleton `com.mcode.compaction` + `compaction_plugins/adaptive` + `CompactionPackService`。Core 没有 compaction 实现、策略接口、registry、hook 或 fallback；Pack 缺失或失败即明确不可用。

## 4. Provider 与 auth

Provider 是 `com.mcode.providers` + `provider_plugins/pi` + `ProviderPackService`。ProviderPack 与 FeaturePack 使用不同 world。Host 独占 auth store、HTTP、TLS、DNS、proxy、reserved headers 和连接安全；ProviderPack 不取得 credential、socket 或 HTTP client。

Provider auth 的特殊文件是 `~/.mcode/provider_plugins/auth.json`。T6 只交付严格空 store、schema、CAS 与 ACL 机械；只有在 T11 已有签名 Pack identity 后，Host 才能创建或注入 entry，或迁移旧 secret。该流程不让 Core、Manager 或 Pack 直接读取 secret。

## 5. 当前实现状态（非目标）

当前仓库的最小 loop 和 canonical builtin library 基础已存在；旧 Core compaction pipeline、direct MCP runtime 与仅供它使用的 vendored `rmcp` 已删除。CLI 的直接 Provider/Session 产品 assembly、旧产品 flags、Tokio runtime 与 headless renderer 也已删除；`run`/`resume` 当前只返回要求安装并激活 Providers/Session Manager 与对应 signed Pack 的确定性 setup 错误，不访问 cwd、state、environment、auth 或 network。旧 `mcode-session` crate、actor、global JSONL/store/path、resume/fork、legacy Session config scope 与 Core Session product public API/runtime 已删除，仓库没有 global Session store、JSONL 或 compatibility fallback。中性的 `AgentEvent`/`MessageDelta`/`TurnOutcome` 是现行最小 Agent loop 协议，Core ids 只保留 `CallId`；tool dispatch 不再携带无 consumer 的 Session identity。旧 `mcode-llm` crate 及其 profile、catalog、identity、header、wire、HTTP、SSE、registry、fallback、旧 Provider/stream/error 实现和全部专属 tests/fixtures/live ignored tests 已整体删除，且未迁移或保留 stub、legacy namespace、adapter、compatibility、fake 或 unavailable Provider。独立的 `mcode-provider-api` 仍只提供 provider-neutral Agent↔Host Rust port，`mcode-agent` 已迁到该边界；该 Rust port 不是 `mcode:provider-pack@0.1.0` world、产品 extension surface 或 Provider 实现，也不声称未来 Host adapter 或 T11 Provider 能力已交付。当前唯一明确剩余的 T5 acceptance residue 是 Core `ReplayWire`/`ReplayDomain`/`ReplayState`、`AssistantPhase`/`TextBlock.phase`、`ThinkingBlock.replay` 与 `ToolCall.item_id` 的 wire-only cleanup，T5 尚未完成。Plugin ABI v1 (`mcode:plugin@0.1.0`) 待 T7 由三个 target world 替换并由 loader/runtime 拒绝；这些 residue 都不是 compatibility 或 fallback。本页列出的 Manager、Service、数据隔离和 auth 阶段均未因本文而声称已实现。详细目录见 [03-plugins.md](03-plugins.md)，阶段见 [04-roadmap.md](04-roadmap.md)。
