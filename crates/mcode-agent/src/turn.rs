//! Inner-loop mechanics: one LLM response cycle and one tool dispatch
//! (design doc `01-agent-core.md` §3, the body of the pseudocode loop).
//!
//! Everything here is a free function over `(&TurnEnv, &mut AgentState)`
//! so the agent's double loop in [`crate::agent`] can call it with
//! field-level borrows. Event emission follows the `SessionEvent`
//! vocabulary of `mcode-core`.

use mcode_core::events::{MessageDelta, SessionEvent};
use mcode_core::message::{AssistantMessage, ContentBlock, Message, ToolCall, ToolResultMessage};
use mcode_core::{CallId, McodeError};
use mcode_llm::{LlmError, Request, StreamEvent, StreamExt};
use mcode_tools::permission::{GateResult, PermissionAction};
use mcode_tools::{ToolCtx, ToolError, ToolResult, ToolStream, ToolStreamItem};
use tokio_util::sync::CancellationToken;

use crate::agent::{AgentConfig, AgentState};
use crate::env::{PermissionRequest, TurnEnv};
use crate::hooks::HookEvent;

/// Why an in-flight response cycle ended unsuccessfully.
pub(crate) enum TurnFailure {
    /// The turn's cancellation token fired (`abort()` or `env.cancel`).
    Aborted,
    /// A provider-level failure. The [`SessionEvent::Error`] event has
    /// already been emitted at the failure site.
    Error(McodeError),
}

/// Publish a session event; receiver errors (nobody listening, lagged)
/// are ignored — observers are best-effort.
pub(crate) fn emit(env: &TurnEnv<'_>, event: SessionEvent) {
    let _ = env.events.send(event);
}

/// Append a message to the history and announce it.
pub(crate) fn push_message(env: &TurnEnv<'_>, state: &mut AgentState, msg: Message) {
    state.messages.push(msg.clone());
    emit(env, SessionEvent::MessageAdded(msg));
}

/// Stream one assistant response into the history: build the request,
/// iterate the provider stream, mirror every delta as a
/// [`SessionEvent::MessageDelta`], and keep the fully assembled
/// [`AssistantMessage`] from the terminal `Done` event.
///
/// Cancellation surfaces as [`TurnFailure::Aborted`] — either via the
/// stream's own `Error(Cancelled)` termination or a boundary check.
pub(crate) async fn stream_assistant(
    env: &TurnEnv<'_>,
    token: &CancellationToken,
    config: &AgentConfig,
    state: &mut AgentState,
) -> Result<AssistantMessage, TurnFailure> {
    let request = Request {
        model: config.model.clone(),
        system_prompt: config.system_prompt.clone(),
        messages: state.messages.clone(),
        tools: env.tools.specs(),
        thinking: config.thinking,
    };
    let request = env
        .hooks
        .transform(HookEvent::BeforeProviderRequest, request)
        .await;

    if token.is_cancelled() {
        return Err(TurnFailure::Aborted);
    }

    let mut stream = match env.provider.stream(&request, token.clone()).await {
        Ok(stream) => stream,
        // The request failed before streaming began (connect /
        // config failure). Same telemetry contract as a mid-stream
        // failure: the Error event is emitted here, at the failure
        // site, before the turn unwinds.
        Err(err) => {
            let error = McodeError::from(err);
            emit(env, SessionEvent::Error(error.clone()));
            return Err(TurnFailure::Error(error));
        }
    };

    while let Some(event) = stream.next().await {
        match event {
            StreamEvent::Start => env.hooks.notify(HookEvent::MessageStart).await,
            StreamEvent::TextDelta(delta) => {
                emit(
                    env,
                    SessionEvent::MessageDelta(MessageDelta::TextDelta(delta)),
                );
            }
            StreamEvent::ThinkingDelta(delta) => {
                emit(
                    env,
                    SessionEvent::MessageDelta(MessageDelta::ThinkingDelta(delta)),
                );
            }
            StreamEvent::ToolCallDelta { id, partial_json } => {
                emit(
                    env,
                    SessionEvent::MessageDelta(MessageDelta::ToolCallDelta { id, partial_json }),
                );
            }
            StreamEvent::ToolCallEnd(_) => {
                // The complete call arrives inside the final Done message.
            }
            StreamEvent::Done { message } => {
                let message = env.hooks.transform(HookEvent::MessageEnd, message).await;
                state.messages.push(Message::Assistant(message.clone()));
                emit(
                    env,
                    SessionEvent::MessageAdded(Message::Assistant(message.clone())),
                );
                return Ok(message);
            }
            StreamEvent::Error(err) => {
                if matches!(err, LlmError::Cancelled) {
                    return Err(TurnFailure::Aborted);
                }
                emit(env, SessionEvent::Error(McodeError::from(err.clone())));
                return Err(TurnFailure::Error(McodeError::Provider(err.to_string())));
            }
        }
    }
    // The stream ended without a terminal event. EventStream terminates
    // with `None` (not `Error(Cancelled)`) when a cancelled producer
    // drops its sender before the consumer observes the token, so
    // attribute the termination from the turn token first.
    if token.is_cancelled() {
        return Err(TurnFailure::Aborted);
    }
    let error = McodeError::Provider("stream ended without a terminal event".into());
    emit(env, SessionEvent::Error(error.clone()));
    Err(TurnFailure::Error(error))
}

/// Fail a tool call from a `Length`-truncated message without executing
/// it: streamed arguments that parse may still be silently incomplete,
/// so none of them are safe to run (pi parity). The model re-issues the
/// call with complete arguments.
pub(crate) fn fail_truncated_call(env: &TurnEnv<'_>, call: &ToolCall) -> ToolResultMessage {
    let call_id = CallId::from(call.id.as_str());
    emit(
        env,
        SessionEvent::ToolStarted {
            call_id: call_id.clone(),
            name: call.name.clone(),
        },
    );
    completed_error(
        env,
        &call_id,
        call,
        "tool call was not executed: the response hit the output token limit, so its \
            arguments may be truncated; re-issue the call with complete arguments"
            .into(),
    )
}

/// Synthesize an `is_error` tool result for a call that was never
/// dispatched because the turn aborted mid-dispatch of a multi-call
/// response. The assistant message carrying *all* the calls is already
/// in the history, and the OpenAI wire format requires every assistant
/// `tool_call` id to be answered by a following tool message — so the
/// loop writes cancellation results for the undispatched remainder
/// before unwinding (pi parity; keeps state consistent on abort).
pub(crate) fn fail_cancelled_call(env: &TurnEnv<'_>, call: &ToolCall) -> ToolResultMessage {
    let call_id = CallId::from(call.id.as_str());
    emit(
        env,
        SessionEvent::ToolStarted {
            call_id: call_id.clone(),
            name: call.name.clone(),
        },
    );
    completed_error(
        env,
        &call_id,
        call,
        "tool call was not executed: the turn was aborted before this call \
            was dispatched"
            .into(),
    )
}

/// Dispatch one tool call through the three-stage permission pipeline
/// (`02-tools-permissions.md` §5) and return the resulting
/// [`ToolResultMessage`].
///
/// 1. **Rules** ([`PermissionEngine`]): `Deny` short-circuits into an
///    error result; `Allow` proceeds; `Ask`/`NoMatch` continue.
/// 2. **Hook gate** (`HookEvent::ToolCall`): may rewrite the arguments
///    in place or block. Runs for every non-denied call (in yolo mode
///    the rules stage is skipped but the gate still runs).
/// 3. **Ask** ([`PermissionPrompt`]): the remaining `Ask` decisions are
///    resolved by the injected callback, bracketed by the
///    `PermissionRequested` / `PermissionResolved` telemetry events.
///    `NoMatch` + gate pass proceeds (documented default-allow).
///
/// Every denial — rule, hook block, declined prompt, unknown tool, tool
/// failure — is returned **as an `is_error` tool result** so the loop
/// continues and the model can react (`01-agent-core.md` §3); nothing
/// here crashes the turn.
pub(crate) async fn dispatch_tool_call(
    env: &TurnEnv<'_>,
    token: &CancellationToken,
    call: &ToolCall,
) -> ToolResultMessage {
    let call_id = CallId::from(call.id.as_str());
    emit(
        env,
        SessionEvent::ToolStarted {
            call_id: call_id.clone(),
            name: call.name.clone(),
        },
    );

    let Some(tool) = env.tools.get(&call.name) else {
        return completed_error(env, &call_id, call, format!("unknown tool: {}", call.name));
    };

    // Stage 1: rule table.
    let mut args = call.arguments.clone();
    let action = env.permissions.evaluate(&call.name, &args);
    if matches!(action, PermissionAction::Deny) {
        return completed_error(
            env,
            &call_id,
            call,
            format!(
                "permission denied by rule: {}({}) — adjust the permission rules to \
                    allow this call",
                call.name,
                serde_json::to_string(&args).unwrap_or_else(|_| "...".into())
            ),
        );
    }

    // Stage 2: plugin hook gate (may rewrite args in place / block).
    if let GateResult::Block(reason) = env.hooks.gate(HookEvent::ToolCall, &mut args).await {
        return completed_error(
            env,
            &call_id,
            call,
            format!("permission denied: blocked by hook: {reason}"),
        );
    }

    // Stage 3: ask the user.
    if matches!(action, PermissionAction::Ask) {
        let request_id = CallId::new().into_inner();
        emit(
            env,
            SessionEvent::PermissionRequested {
                request_id: request_id.clone(),
                tool_name: call.name.clone(),
                arguments: args.clone(),
            },
        );
        env.hooks.notify(HookEvent::PermissionRequested).await;
        let allowed = env
            .permission_prompt
            .prompt(PermissionRequest {
                request_id: request_id.clone(),
                tool_name: call.name.clone(),
                arguments: args.clone(),
            })
            .await;
        emit(
            env,
            SessionEvent::PermissionResolved {
                request_id,
                allowed,
            },
        );
        env.hooks.notify(HookEvent::PermissionResolved).await;
        if !allowed {
            return completed_error(
                env,
                &call_id,
                call,
                "permission denied: the request was declined".into(),
            );
        }
    }

    // Execute. Progress items stream out live while the tool runs; the
    // M1 dispatcher convention pushes the terminal result onto the tool
    // stream ourselves (first terminal wins, so a self-terminating tool
    // keeps its own result).
    let ctx = ToolCtx::new(env.cwd.clone(), env.session_id.clone(), call_id.clone())
        .with_cancel(token.clone());
    let (mut producer, mut consumer) = ToolStream::channel();
    let terminal_pusher = producer.clone();
    let handle = tokio::spawn(async move { tool.execute_dyn(args, &ctx, &mut producer).await });

    // Tools watch `ctx.cancel` (same token); we do not select on it here
    // — the post-dispatch boundary check in the loop unwinds the turn.
    let exec_result: Result<ToolResult, ToolError> = match handle.await {
        Ok(result) => result,
        Err(join_err) => Err(ToolError::PluginTrap(format!(
            "tool task failed: {join_err}"
        ))),
    };
    let result = exec_result.unwrap_or_else(|err| ToolResult::error(err.to_string()));

    let _ = terminal_pusher.terminal(result.clone());
    drop(terminal_pusher);

    let mut streamed_terminal = None;
    while let Some(item) = consumer.recv().await {
        match item {
            ToolStreamItem::Progress(progress) => emit(
                env,
                SessionEvent::ToolProgress {
                    call_id: call_id.clone(),
                    message: progress.message,
                },
            ),
            ToolStreamItem::Terminal(result) => streamed_terminal = Some(result),
        }
    }
    let result = streamed_terminal.unwrap_or(result);
    let result = env.hooks.transform(HookEvent::ToolResult, result).await;

    let message = ToolResultMessage {
        tool_call_id: call.id.clone(),
        content: result.content,
        is_error: result.is_error,
        details: result.details,
    };
    emit(
        env,
        SessionEvent::ToolCompleted {
            call_id,
            result: message.clone(),
        },
    );
    message
}

/// Synthesize an `is_error` tool result, emit its `ToolCompleted` event,
/// and return it for the loop to write back into the context.
fn completed_error(
    env: &TurnEnv<'_>,
    call_id: &CallId,
    call: &ToolCall,
    reason: String,
) -> ToolResultMessage {
    let message = ToolResultMessage {
        tool_call_id: call.id.clone(),
        content: vec![ContentBlock::Text(reason.into())],
        is_error: true,
        details: None,
    };
    emit(
        env,
        SessionEvent::ToolCompleted {
            call_id: call_id.clone(),
            result: message.clone(),
        },
    );
    message
}
