//! Inner-loop mechanics: one LLM response cycle and one tool dispatch
//! (design doc `01-agent-core.md` §3, the body of the pseudocode loop).
//!
//! Everything here is a free function over `(&TurnEnv, &mut AgentState)`
//! so the agent's double loop in [`crate::agent`] can call it with
//! field-level borrows. Event emission follows the `SessionEvent`
//! vocabulary of `mcode-core`.

use std::any::Any;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::task::{Context, Poll};

use mcode_core::events::{MessageDelta, SessionEvent};
use mcode_core::message::{AssistantMessage, ContentBlock, Message, ToolCall, ToolResultMessage};
use mcode_core::{CallId, McodeError};
use mcode_llm::{LlmError, Request, StreamEvent, StreamExt};
use mcode_tools::permission::{GateResult, PermissionAction};
use mcode_tools::{
    PreparedSearch, ToolCtx, ToolDyn, ToolError, ToolResult, ToolStream, ToolStreamItem,
    prepare_search_async_with_access,
};
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
///    in place or block. A rewrite repeats rule evaluation and rebinds a
///    fresh [`PreparedSearch`] for tools declaring `search_access` before
///    stage 3. Runs for every non-denied call (in yolo mode the rules stage
///    is skipped but the gate still runs).
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

    // Stage 1: rule table. Tools that opt into search preflight resolve
    // once on a cancellable worker and match that ready handle-backed key
    // so `./secrets` and Windows aliases cannot bypass path rules. Any
    // resolve failure is terminal; the retained root is passed to
    // execution and never re-resolved. Same-name plugin overrides remain
    // unbound unless they explicitly declare a search access mode.
    let mut args = call.arguments.clone();
    let mut prepared = match bind_permission(env, token, tool.as_ref(), &call.name, &args).await {
        Ok(bound) => bound,
        Err(message) => return completed_error(env, &call_id, call, message),
    };
    if matches!(prepared.action, PermissionAction::Deny) {
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
    // A rewrite re-prepares and re-evaluates so the hook cannot unbind the
    // permission key from the execution root. Unchanged args keep the root.
    let before_hook = args.clone();
    if let GateResult::Block(reason) = env.hooks.gate(HookEvent::ToolCall, &mut args).await {
        return completed_error(
            env,
            &call_id,
            call,
            format!("permission denied: blocked by hook: {reason}"),
        );
    }
    if args != before_hook {
        prepared = match bind_permission(env, token, tool.as_ref(), &call.name, &args).await {
            Ok(bound) => bound,
            Err(message) => return completed_error(env, &call_id, call, message),
        };
        if matches!(prepared.action, PermissionAction::Deny) {
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
    }

    // Stage 3: ask the user.
    if matches!(prepared.action, PermissionAction::Ask) {
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
    let mut ctx = ToolCtx::new(env.cwd.clone(), env.session_id.clone(), call_id.clone())
        .with_cancel(token.clone());
    if let Some(search) = prepared.search {
        ctx = ctx.with_prepared_search(search);
    }
    let (mut producer, mut consumer) = ToolStream::channel();
    let mut terminal_pusher = Some(producer.clone());
    let execute =
        CatchUnwind::new(async move { tool.execute_dyn(args, &ctx, &mut producer).await });
    tokio::pin!(execute);

    // Structured select: progress is consumed live, and dropping this
    // future drops `execute` instead of detaching a spawned task. Poll
    // panics and completion-path Drop panics become owned-string Err
    // values. Poll-panic cleanup preserves its first error and discards a
    // later destructor error; cancel/abort Drop likewise discards destructor
    // errors. Unknown panic payloads are forgotten at the catch boundary so
    // their Drop cannot unwind the prompt.
    let mut exec_result = None;
    let mut streamed_terminal = None;
    loop {
        tokio::select! {
            biased;
            item = consumer.recv() => {
                match item {
                    Some(ToolStreamItem::Progress(progress)) => emit(
                        env,
                        SessionEvent::ToolProgress {
                            call_id: call_id.clone(),
                            message: progress.message,
                        },
                    ),
                    Some(ToolStreamItem::Terminal(result)) => streamed_terminal = Some(result),
                    None => break,
                }
            }
            result = &mut execute, if exec_result.is_none() => {
                let result = match result {
                    Ok(Ok(value)) => value,
                    Ok(Err(err)) => ToolResult::error(err.to_string()),
                    Err(message) => panic_tool_result(message),
                };
                if let Some(pusher) = terminal_pusher.take() {
                    let _ = pusher.terminal(result.clone());
                }
                exec_result = Some(result);
            }
        }
    }
    let result = streamed_terminal
        .or(exec_result)
        .unwrap_or_else(|| ToolResult::error("tool task ended without a result".to_owned()));
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
    // Let abort/steer observers scheduled on ToolCompleted run before
    // the next dispatch in a multi-call response.
    tokio::task::yield_now().await;
    message
}

struct BoundPermission {
    action: PermissionAction,
    search: Option<std::sync::Arc<PreparedSearch>>,
}

async fn bind_permission(
    env: &TurnEnv<'_>,
    token: &CancellationToken,
    tool: &dyn ToolDyn,
    tool_name: &str,
    args: &serde_json::Value,
) -> Result<BoundPermission, String> {
    let Some(access) = tool.search_access() else {
        return Ok(BoundPermission {
            action: env.permissions.evaluate(tool_name, args),
            search: None,
        });
    };
    let path = args
        .get("path")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let prepared = prepare_search_async_with_access(env.cwd.clone(), path, token.clone(), access)
        .await
        .map_err(|error| error.to_string())?;
    let action = env.permissions.evaluate_salient(tool_name, prepared.key());
    Ok(BoundPermission {
        action,
        search: Some(std::sync::Arc::new(prepared)),
    })
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

// Rust guideline compliant 2026-08-26.
fn panic_tool_result(message: String) -> ToolResult {
    ToolResult::error(ToolError::PluginTrap(message).to_string())
}

/// Generic message used when the payload is not a `String` or `&str`.
///
/// Unknown payloads are forgotten rather than dropped, so this string is
/// the only text an unknown/non-string `panic_any` payload can force onto
/// the model.
const TOOL_PANIC_MESSAGE: &str = "tool panicked";

/// Copies a panic payload into an owned message at the catch boundary.
///
/// `String` and `&str` are safe to drop after the copy. Every other
/// payload is forgotten: a tool can `panic_any` a type whose `Drop`
/// panics, and dropping that box outside `catch_unwind` would unwind
/// the prompt task.
fn owned_panic_message(payload: Box<dyn Any + Send>) -> String {
    let payload = match payload.downcast::<String>() {
        Ok(text) => return *text,
        Err(payload) => payload,
    };
    let payload = match payload.downcast::<&str>() {
        Ok(text) => return (*text).to_owned(),
        Err(payload) => payload,
    };
    std::mem::forget(payload);
    TOOL_PANIC_MESSAGE.to_owned()
}

/// Runs `f` under `catch_unwind` and never lets a raw panic payload escape.
fn catch_unwind_message<R>(f: impl FnOnce() -> R) -> Result<R, String> {
    match std::panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => Ok(value),
        Err(payload) => Err(owned_panic_message(payload)),
    }
}

/// Catches panics from a pinned tool future without detaching it.
///
/// The inner future lives in `Pin<Box<F>>` so poll and drop never move a
/// `!Unpin` future after it is pinned. `poll` and every `inner` destructor
/// run inside `catch_unwind`. Caught payloads are reduced to an owned
/// `String` before leaving the catch boundary; unknown payloads are
/// forgotten so a panicking `Drop` cannot unwind the prompt task. A
/// destructor panic after `Poll::Ready` is returned as `Err`. After a poll
/// panic, cleanup preserves that first error and discards a later destructor
/// panic; the wrapper's `Drop` (cancel/abort) also discards a destructor
/// panic so it cannot unwind the prompt task.
struct CatchUnwind<F> {
    inner: Option<Pin<Box<F>>>,
}

impl<F> CatchUnwind<F> {
    fn new(inner: F) -> Self {
        Self {
            inner: Some(Box::pin(inner)),
        }
    }

    /// Drops `F` at its pinned heap address. Moving `Pin<Box<F>>` moves the
    /// pointer, not `F`. The result is an owned message, never a raw payload.
    fn drop_inner(inner: Pin<Box<F>>) -> Result<(), String> {
        catch_unwind_message(move || drop(inner))
    }
}

impl<F> Drop for CatchUnwind<F> {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            // The result is already an owned `String`; dropping it is safe.
            drop(Self::drop_inner(inner));
        }
    }
}

impl<F: Future> Future for CatchUnwind<F> {
    type Output = Result<F::Output, String>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // `Pin<Box<F>>` is always `Unpin`, so the wrapper is `Unpin`.
        let this = self.get_mut();
        let Some(inner) = this.inner.as_mut() else {
            return Poll::Ready(Err(TOOL_PANIC_MESSAGE.to_owned()));
        };
        match catch_unwind_message(|| inner.as_mut().poll(cx)) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(output)) => match this.inner.take() {
                Some(inner) => match Self::drop_inner(inner) {
                    Ok(()) => Poll::Ready(Ok(output)),
                    Err(message) => Poll::Ready(Err(message)),
                },
                None => Poll::Ready(Ok(output)),
            },
            Err(message) => {
                if let Some(inner) = this.inner.take() {
                    drop(Self::drop_inner(inner));
                }
                Poll::Ready(Err(message))
            }
        }
    }
}

#[cfg(test)]
mod catch_unwind_tests {
    use super::*;
    use std::cell::Cell;
    use std::future::Future;
    use std::marker::PhantomPinned;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    struct PanicOnDrop;

    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            panic!("intentional tool drop panic");
        }
    }

    /// Panic payload whose destructor panics again if the box is dropped.
    struct PanickingPayload;

    impl Drop for PanickingPayload {
        fn drop(&mut self) {
            panic!("payload drop must not escape isolation");
        }
    }

    struct PanicAnyOnDrop;

    impl Drop for PanicAnyOnDrop {
        fn drop(&mut self) {
            std::panic::panic_any(PanickingPayload);
        }
    }

    struct PollPanicAny;

    impl Future for PollPanicAny {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
            std::panic::panic_any(PanickingPayload);
        }
    }

    struct ReadyDropPanicAny {
        _guard: PanicAnyOnDrop,
    }

    impl Future for ReadyDropPanicAny {
        type Output = &'static str;

        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<&'static str> {
            Poll::Ready("ok")
        }
    }

    struct PendingDropPanicAny {
        _guard: PanicAnyOnDrop,
    }

    impl Future for PendingDropPanicAny {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
            Poll::Pending
        }
    }

    struct PollPanicThenDropPanicAny {
        _guard: PanicAnyOnDrop,
    }

    impl Future for PollPanicThenDropPanicAny {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
            panic!("intentional poll panic");
        }
    }

    struct PendingDrop {
        _guard: PanicOnDrop,
    }

    impl Future for PendingDrop {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
            Poll::Pending
        }
    }

    struct PollPanicDrop {
        _guard: PanicOnDrop,
    }

    impl Future for PollPanicDrop {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
            panic!("intentional poll panic");
        }
    }

    struct ReadyDrop {
        _guard: PanicOnDrop,
    }

    impl Future for ReadyDrop {
        type Output = &'static str;

        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<&'static str> {
            Poll::Ready("ok")
        }
    }

    /// Records its address on first poll and asserts Drop sees the same one.
    struct AddressSensitive {
        first_addr: Cell<Option<usize>>,
        complete: bool,
        _pinned: PhantomPinned,
    }

    impl AddressSensitive {
        fn pending() -> Self {
            Self {
                first_addr: Cell::new(None),
                complete: false,
                _pinned: PhantomPinned,
            }
        }

        fn ready() -> Self {
            Self {
                first_addr: Cell::new(None),
                complete: true,
                _pinned: PhantomPinned,
            }
        }

        fn record_or_check(&self) {
            let addr = std::ptr::from_ref(self) as usize;
            match self.first_addr.get() {
                None => self.first_addr.set(Some(addr)),
                Some(first) => assert_eq!(first, addr, "pinned future was moved"),
            }
        }
    }

    impl Future for AddressSensitive {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
            let this = self.as_ref().get_ref();
            this.record_or_check();
            if this.complete {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
    }

    impl Drop for AddressSensitive {
        fn drop(&mut self) {
            if self.first_addr.get().is_some() {
                self.record_or_check();
            }
        }
    }

    #[test]
    fn drop_of_pending_inner_does_not_unwind() {
        drop(CatchUnwind::new(PendingDrop {
            _guard: PanicOnDrop,
        }));
    }

    #[tokio::test]
    async fn abort_after_poll_drops_inner_without_unwind() {
        let execute = CatchUnwind::new(PendingDrop {
            _guard: PanicOnDrop,
        });
        tokio::pin!(execute);
        tokio::select! {
            biased;
            _ = &mut execute => panic!("pending future completed"),
            _ = std::future::ready(()) => {}
        }
    }

    #[tokio::test]
    async fn poll_panic_then_inner_drop_does_not_unwind() {
        let result = CatchUnwind::new(PollPanicDrop {
            _guard: PanicOnDrop,
        })
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ready_then_drop_panic_maps_to_error() {
        let result = CatchUnwind::new(ReadyDrop {
            _guard: PanicOnDrop,
        })
        .await;
        let err = result.expect_err("completion Drop panic must be an error");
        assert_eq!(err, "intentional tool drop panic");
    }

    #[test]
    fn owned_panic_message_preserves_str_and_string() {
        assert_eq!(owned_panic_message(Box::new("hello")), "hello");
        assert_eq!(
            owned_panic_message(Box::new(String::from("hello"))),
            "hello"
        );
    }

    #[test]
    fn owned_panic_message_forgets_panicking_payload() {
        assert_eq!(
            owned_panic_message(Box::new(PanickingPayload)),
            TOOL_PANIC_MESSAGE
        );
    }

    #[test]
    fn catch_unwind_message_forgets_panicking_payload() {
        let result: Result<(), String> =
            catch_unwind_message(|| std::panic::panic_any(PanickingPayload));
        assert_eq!(result, Err(TOOL_PANIC_MESSAGE.to_owned()));
    }

    #[tokio::test]
    async fn poll_panic_any_payload_drop_does_not_unwind() {
        let result = CatchUnwind::new(PollPanicAny).await;
        assert_eq!(result, Err(TOOL_PANIC_MESSAGE.to_owned()));
    }

    #[tokio::test]
    async fn ready_then_drop_panic_any_payload_maps_to_error() {
        let result = CatchUnwind::new(ReadyDropPanicAny {
            _guard: PanicAnyOnDrop,
        })
        .await;
        assert_eq!(result, Err(TOOL_PANIC_MESSAGE.to_owned()));
    }

    #[test]
    fn drop_of_pending_panic_any_payload_does_not_unwind() {
        drop(CatchUnwind::new(PendingDropPanicAny {
            _guard: PanicAnyOnDrop,
        }));
    }

    #[tokio::test]
    async fn abort_after_poll_drops_panic_any_payload_without_unwind() {
        let execute = CatchUnwind::new(PendingDropPanicAny {
            _guard: PanicAnyOnDrop,
        });
        tokio::pin!(execute);
        tokio::select! {
            biased;
            _ = &mut execute => panic!("pending future completed"),
            _ = std::future::ready(()) => {}
        }
    }

    #[tokio::test]
    async fn poll_panic_then_inner_drop_panic_any_does_not_unwind() {
        let result = CatchUnwind::new(PollPanicThenDropPanicAny {
            _guard: PanicAnyOnDrop,
        })
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn unpin_false_ready_future_stays_at_poll_address() {
        CatchUnwind::new(AddressSensitive::ready())
            .await
            .expect("ready future must complete");
    }

    #[tokio::test]
    async fn unpin_false_pending_future_stays_at_poll_address_after_cancel() {
        let execute = CatchUnwind::new(AddressSensitive::pending());
        tokio::pin!(execute);
        tokio::select! {
            biased;
            _ = &mut execute => panic!("pending future completed"),
            _ = std::future::ready(()) => {}
        }
    }
}
