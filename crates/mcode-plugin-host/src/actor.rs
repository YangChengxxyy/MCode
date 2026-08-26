//! Single-Store actor that serially owns one plugin generation.

// Rust guideline compliant 2026-08-26.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mcode_plugin_api::{
    CapabilityGrants, GuestInvokeRequest, GuestInvokeResponse, GuestParseError, GuestRenderRequest,
    PluginEvent, PluginId, UiView, parse_guest_error, parse_guest_success,
};
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Engine, Store};

use crate::error::HostError;
use crate::host_api::PluginStore;
use crate::mailbox::{Admission, EventDelivery, Job, MailboxSender};
use crate::sandbox::SandboxLimits;
use crate::wit::Plugin;

/// Poll interval while joining the actor thread.
///
/// Short enough to honor small `shutdown_timeout` values, long enough to
/// avoid a busy-wait. Changing it affects `StopTimeout` tests.
const JOIN_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Brief join window used when stop already exceeded `shutdown_timeout`.
const EXPIRED_STOP_JOIN_WAIT: Duration = Duration::from_millis(1);

/// Host-enforced mailbox and deadline limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeLimits {
    mailbox_capacity: usize,
    mailbox_max_bytes: usize,
    call_timeout: Duration,
    shutdown_timeout: Duration,
    sandbox: SandboxLimits,
}

impl RuntimeLimits {
    /// Creates validated runtime limits.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::InvalidLimits`] for zero capacity, bytes, or
    /// timeouts.
    pub fn new(
        mailbox_capacity: usize,
        mailbox_max_bytes: usize,
        call_timeout: Duration,
        shutdown_timeout: Duration,
        sandbox: SandboxLimits,
    ) -> Result<Self, HostError> {
        if mailbox_capacity == 0
            || mailbox_capacity > 1024
            || mailbox_max_bytes == 0
            || call_timeout.is_zero()
            || shutdown_timeout.is_zero()
        {
            return Err(HostError::InvalidLimits);
        }
        Ok(Self {
            mailbox_capacity,
            mailbox_max_bytes,
            call_timeout,
            shutdown_timeout,
            sandbox,
        })
    }

    /// Bounded mailbox entry capacity.
    #[must_use]
    pub fn mailbox_capacity(self) -> usize {
        self.mailbox_capacity
    }

    /// Bounded queued payload bytes.
    #[must_use]
    pub fn mailbox_max_bytes(self) -> usize {
        self.mailbox_max_bytes
    }

    /// Per-call wall deadline.
    #[must_use]
    pub fn call_timeout(self) -> Duration {
        self.call_timeout
    }

    /// Total stop/disable deadline.
    #[must_use]
    pub fn shutdown_timeout(self) -> Duration {
        self.shutdown_timeout
    }

    /// WASM sandbox limits.
    #[must_use]
    pub fn sandbox(self) -> SandboxLimits {
        self.sandbox
    }
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            mailbox_capacity: 64,
            mailbox_max_bytes: 256 * 1024,
            call_timeout: Duration::from_secs(5),
            shutdown_timeout: Duration::from_secs(2),
            sandbox: SandboxLimits::default(),
        }
    }
}

/// First-party plugin lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    /// Actor is accepting work.
    Running,
    /// Admission is closed and the store is shutting down.
    Stopping,
    /// Actor has exited.
    Stopped,
}

pub(crate) enum GuestKind {
    Wasm {
        bindings: Plugin,
        store: Store<PluginStore>,
    },
    #[cfg(feature = "test-util")]
    Fake(Box<dyn crate::test_util::FakeGuest>),
}

struct ActorState {
    guest: GuestKind,
    engine: Engine,
    admission: Arc<Admission>,
    enter: Arc<Mutex<()>>,
    queued_bytes: Arc<AtomicUsize>,
    limits: RuntimeLimits,
    state: Arc<Mutex<LifecycleState>>,
    current_call: Arc<Mutex<Option<Arc<AtomicBool>>>>,
}

struct PluginInner {
    plugin_id: PluginId,
    generation: u64,
    admission: Arc<Admission>,
    mailbox: MailboxSender,
    engine: Engine,
    worker: Mutex<Option<JoinHandle<()>>>,
    limits: RuntimeLimits,
    state: Arc<Mutex<LifecycleState>>,
    stop: Mutex<()>,
    current_call: Arc<Mutex<Option<Arc<AtomicBool>>>>,
}

/// Cloneable handle to one plugin generation.
///
/// Dropping the last clone closes admission, interrupts the guest, and joins
/// the actor thread. A shutdown-deadline miss keeps joining so the worker is
/// never detached.
#[derive(Clone)]
pub struct PluginHandle {
    inner: Arc<PluginInner>,
}

impl std::fmt::Debug for PluginHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginHandle")
            .field("plugin_id", &self.plugin_id())
            .field("generation", &self.generation())
            .field("state", &self.state())
            .finish()
    }
}

impl PluginHandle {
    pub(crate) fn spawn(
        plugin_id: PluginId,
        generation: u64,
        engine: Engine,
        guest: GuestKind,
        limits: RuntimeLimits,
    ) -> Result<Self, HostError> {
        let admission = Arc::new(Admission::new(generation));
        let enter = Arc::new(Mutex::new(()));
        let (mailbox, rx, queued_bytes) = crate::mailbox::channel(
            limits.mailbox_capacity,
            limits.mailbox_max_bytes,
            admission.clone(),
        );
        let state = Arc::new(Mutex::new(LifecycleState::Running));
        let current_call = Arc::new(Mutex::new(None));
        let actor = ActorState {
            guest,
            engine: engine.clone(),
            admission: admission.clone(),
            enter,
            queued_bytes,
            limits,
            state: state.clone(),
            current_call: current_call.clone(),
        };
        let worker = std::thread::Builder::new()
            .name(format!("mcode-plugin-{plugin_id}"))
            .spawn(move || actor_loop(actor, rx))
            .map_err(|_| HostError::ActorSpawn)?;
        Ok(Self {
            inner: Arc::new(PluginInner {
                plugin_id,
                generation,
                admission,
                mailbox,
                engine,
                worker: Mutex::new(Some(worker)),
                limits,
                state,
                stop: Mutex::new(()),
                current_call,
            }),
        })
    }

    /// Returns the plugin id.
    #[must_use]
    pub fn plugin_id(&self) -> &PluginId {
        &self.inner.plugin_id
    }

    /// Returns the live generation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.inner.generation
    }

    /// Returns current lifecycle state.
    #[must_use]
    pub fn state(&self) -> LifecycleState {
        *self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Tries to enqueue one redacted event without awaiting guest code.
    #[must_use]
    pub fn try_send_event(&self, event: PluginEvent) -> EventDelivery {
        if event.validate().is_err() {
            return EventDelivery::Closed;
        }
        let Ok(payload) = serde_json::to_string(&event) else {
            return EventDelivery::Closed;
        };
        self.inner.mailbox.try_enqueue(Job::Event {
            generation: self.inner.generation,
            payload,
        })
    }

    /// Invokes a declared tool or command through the mailbox.
    ///
    /// # Errors
    ///
    /// Returns [`HostError`] when admission is closed, the mailbox is full, the
    /// guest traps, the JSON contract is violated, or `request.generation`
    /// does not match this handle.
    pub fn invoke(&self, request: GuestInvokeRequest) -> Result<GuestInvokeResponse, HostError> {
        if request.generation != self.inner.generation {
            return Err(HostError::StaleGeneration);
        }
        let request = serde_json::to_string(&request).map_err(|_| HostError::InvalidGuestOutput)?;
        let output = self.call_with_reply(|reply, cancelled| Job::Invoke {
            generation: self.inner.generation,
            request,
            reply,
            cancelled,
        })?;
        match parse_guest_success(&output) {
            Ok(Some(value)) => {
                serde_json::from_value(value).map_err(|_| HostError::InvalidGuestOutput)
            }
            Ok(None) => Ok(GuestInvokeResponse { output: None }),
            Err(error) => Err(host_error_from_guest_parse(error)),
        }
    }

    /// Renders one declared view through the mailbox.
    ///
    /// # Errors
    ///
    /// Returns [`HostError`] for mailbox, trap, view-validation, or stale
    /// generation failures.
    pub fn render(&self, request: GuestRenderRequest) -> Result<UiView, HostError> {
        if request.generation != self.inner.generation {
            return Err(HostError::StaleGeneration);
        }
        let request = serde_json::to_string(&request).map_err(|_| HostError::InvalidGuestOutput)?;
        let output = self.call_with_reply(|reply, cancelled| Job::Render {
            generation: self.inner.generation,
            request,
            reply,
            cancelled,
        })?;
        let value = match parse_guest_success(&output) {
            Ok(Some(value)) => value,
            Ok(None) => return Err(HostError::InvalidGuestOutput),
            Err(error) => return Err(host_error_from_guest_parse(error)),
        };
        let response: mcode_plugin_api::GuestRenderResponse =
            serde_json::from_value(value).map_err(|_| HostError::InvalidGuestOutput)?;
        let view: UiView =
            serde_json::from_value(response.view).map_err(|_| HostError::InvalidGuestOutput)?;
        view.validate().map_err(|_| HostError::InvalidGuestOutput)?;
        Ok(view)
    }

    /// Atomically closes admission, fences the generation, interrupts the
    /// guest, and joins the actor within the shutdown deadline.
    ///
    /// Concurrent calls serialize. A timeout leaves the join handle in place
    /// so a later call can retry. The actor is marked [`LifecycleState::Stopped`]
    /// only after the worker has been joined.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::StopTimeout`] when the actor does not exit in time.
    pub fn stop(&self) -> Result<(), HostError> {
        shutdown_plugin(&self.inner)
    }

    /// Alias for [`Self::stop`].
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::stop`].
    pub fn disable(&self) -> Result<(), HostError> {
        self.stop()
    }

    fn call_with_reply(
        &self,
        make: impl FnOnce(SyncSender<Result<String, HostError>>, Arc<AtomicBool>) -> Job,
    ) -> Result<String, HostError> {
        if !self.inner.admission.is_open() {
            return Err(HostError::NotRunning);
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::sync_channel(1);
        match self.inner.mailbox.try_enqueue(make(tx, cancelled.clone())) {
            EventDelivery::Queued => {}
            EventDelivery::Full => return Err(HostError::MailboxFull),
            EventDelivery::Closed => return Err(HostError::MailboxClosed),
            EventDelivery::Stale => return Err(HostError::StaleGeneration),
        }
        match rx.recv_timeout(self.inner.limits.call_timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                cancelled.store(true, Ordering::Release);
                interrupt_if_current(&self.inner.current_call, &cancelled, &self.inner.engine);
                Err(HostError::Trap)
            }
            Err(RecvTimeoutError::Disconnected) => Err(HostError::MailboxClosed),
        }
    }
}

impl Drop for PluginInner {
    fn drop(&mut self) {
        if shutdown_plugin(self).is_ok() {
            return;
        }
        // Last owner: keep joining so StopTimeout cannot detach the actor.
        let _ = join_worker(self, None);
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = LifecycleState::Stopped;
    }
}

fn shutdown_plugin(inner: &PluginInner) -> Result<(), HostError> {
    let started = Instant::now();
    let _stop = inner
        .stop
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    {
        let mut state = inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *state != LifecycleState::Stopped {
            *state = LifecycleState::Stopping;
        }
    }
    inner.admission.close();
    inner.engine.increment_epoch();
    inner.mailbox.disconnect();
    let remaining = inner
        .limits
        .shutdown_timeout
        .checked_sub(started.elapsed())
        .unwrap_or(EXPIRED_STOP_JOIN_WAIT);
    // Duration::MAX (and other huge timeouts) cannot always be represented as
    // Instant; None means join until the worker exits instead of panicking.
    let deadline = Instant::now().checked_add(remaining);
    join_worker(inner, deadline)?;
    *inner
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = LifecycleState::Stopped;
    Ok(())
}

fn join_worker(inner: &PluginInner, deadline: Option<Instant>) -> Result<(), HostError> {
    loop {
        let finished = {
            let worker = inner
                .worker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match worker.as_ref() {
                None => return Ok(()),
                Some(handle) => handle.is_finished(),
            }
        };
        if finished {
            if let Some(handle) = inner
                .worker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                let _ = handle.join();
            }
            return Ok(());
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            inner.engine.increment_epoch();
            return Err(HostError::StopTimeout);
        }
        inner.engine.increment_epoch();
        std::thread::sleep(JOIN_POLL_INTERVAL);
    }
}

/// Publishes the in-flight invoke/render cancel flag so a timeout can epoch-
/// interrupt only that call.
struct CurrentCallGuard {
    slot: Arc<Mutex<Option<Arc<AtomicBool>>>>,
}

impl CurrentCallGuard {
    fn try_arm(
        slot: &Arc<Mutex<Option<Arc<AtomicBool>>>>,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<Self, HostError> {
        {
            let mut current = slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if cancelled.load(Ordering::Acquire) {
                return Err(HostError::Trap);
            }
            *current = Some(cancelled.clone());
        }
        let guard = Self { slot: slot.clone() };
        // Recheck after publish: a timeout may have set the flag while the
        // slot was still empty, so it could not increment the epoch.
        if cancelled.load(Ordering::Acquire) {
            return Err(HostError::Trap);
        }
        Ok(guard)
    }
}

impl Drop for CurrentCallGuard {
    fn drop(&mut self) {
        *self
            .slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

fn interrupt_if_current(
    slot: &Mutex<Option<Arc<AtomicBool>>>,
    cancelled: &Arc<AtomicBool>,
    engine: &Engine,
) {
    // Hold `current_call` across `increment_epoch`: Wasmtime 48 epoch
    // atomics are Relaxed, so this mutex is the happens-before with the
    // actor's post-deadline recheck in `prepare_cancellable_store`.
    let current = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if current
        .as_ref()
        .is_some_and(|flag| Arc::ptr_eq(flag, cancelled))
    {
        engine.increment_epoch();
    }
}

#[cfg(feature = "test-util")]
fn cancelled_under_current_call(
    slot: &Mutex<Option<Arc<AtomicBool>>>,
    cancelled: &AtomicBool,
) -> bool {
    let _published = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cancelled.load(Ordering::Acquire)
}

fn run_cancellable_call(
    actor: &mut ActorState,
    generation: u64,
    cancelled: &Arc<AtomicBool>,
    call: impl FnOnce(
        &mut GuestKind,
        &Engine,
        &Mutex<Option<Arc<AtomicBool>>>,
        &mut bool,
    ) -> Result<String, HostError>,
) -> (Result<String, HostError>, bool) {
    if cancelled.load(Ordering::Acquire) {
        return (Err(HostError::Trap), false);
    }
    let _guard = match CurrentCallGuard::try_arm(&actor.current_call, cancelled) {
        Ok(guard) => guard,
        Err(error) => return (Err(error), false),
    };
    let mut entered = false;
    let current_call = actor.current_call.clone();
    let guest_result = enter_guest(actor, generation, |guest, engine| {
        call(guest, engine, &current_call, &mut entered)
    });
    // Epoch/fuel traps taint the Wasmtime instance; queued cancels do not.
    let isolated = entered && matches!(guest_result, Err(HostError::Trap));
    let reply = if cancelled.load(Ordering::Acquire) {
        Err(HostError::Trap)
    } else {
        guest_result
    };
    (reply, isolated)
}

fn actor_loop(mut actor: ActorState, rx: std::sync::mpsc::Receiver<Job>) {
    while let Ok(job) = rx.recv() {
        let bytes = job.payload_bytes();
        actor.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
        if !actor.admission.is_open() {
            reject_job(job, HostError::NotRunning);
            break;
        }
        let sandbox = actor.limits.sandbox;
        let isolated = match job {
            Job::Invoke {
                generation,
                request,
                reply,
                cancelled,
            } => {
                let (result, isolated) = run_cancellable_call(
                    &mut actor,
                    generation,
                    &cancelled,
                    |guest, engine, current_call, entered| {
                        call_invoke(
                            guest,
                            engine,
                            &request,
                            sandbox,
                            &cancelled,
                            current_call,
                            entered,
                        )
                    },
                );
                let _ = reply.send(result);
                isolated
            }
            Job::Event {
                generation,
                payload,
            } => match enter_guest(&mut actor, generation, |guest, store| {
                call_on_event(guest, store, &payload, sandbox)
            }) {
                Ok(output) => parse_guest_error(&output)
                    .map_err(host_error_from_guest_parse)
                    .is_err(),
                Err(error) => should_isolate(&error),
            },
            Job::Render {
                generation,
                request,
                reply,
                cancelled,
            } => {
                let (result, isolated) = run_cancellable_call(
                    &mut actor,
                    generation,
                    &cancelled,
                    |guest, engine, current_call, entered| {
                        call_render(
                            guest,
                            engine,
                            &request,
                            sandbox,
                            &cancelled,
                            current_call,
                            entered,
                        )
                    },
                );
                let _ = reply.send(result);
                isolated
            }
        };
        if isolated || !actor.admission.is_open() {
            actor.admission.close();
            break;
        }
    }
    actor.admission.close();
    *actor
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = LifecycleState::Stopped;
}

fn reject_job(job: Job, error: HostError) {
    match job {
        Job::Invoke { reply, .. } | Job::Render { reply, .. } => {
            let _ = reply.send(Err(error));
        }
        Job::Event { .. } => {}
    }
}

fn should_isolate(error: &HostError) -> bool {
    matches!(
        error,
        HostError::Trap
            | HostError::GuestOutputTooLarge
            | HostError::InvalidGuestOutput
            | HostError::Guest { .. }
    )
}

fn host_error_from_guest_parse(error: GuestParseError) -> HostError {
    match error {
        GuestParseError::TooLarge => HostError::GuestOutputTooLarge,
        GuestParseError::InvalidJson => HostError::InvalidGuestOutput,
        GuestParseError::Guest { code } => HostError::Guest { code },
    }
}

fn enter_guest<T>(
    actor: &mut ActorState,
    generation: u64,
    call: impl FnOnce(&mut GuestKind, &Engine) -> Result<T, HostError>,
) -> Result<T, HostError> {
    let _enter = actor
        .enter
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !actor.admission.is_open() || generation != actor.admission.generation() {
        return Err(HostError::StaleGeneration);
    }
    call(&mut actor.guest, &actor.engine)
}

fn prepare_store(store: &mut Store<PluginStore>, sandbox: SandboxLimits) -> Result<(), HostError> {
    store
        .set_fuel(sandbox.call_fuel)
        .map_err(|_| HostError::Trap)?;
    store.set_epoch_deadline(1);
    Ok(())
}

/// Sets fuel and an epoch deadline, then refuses entry if this call already
/// timed out.
///
/// `set_epoch_deadline(1)` is relative to the current epoch. A timeout may
/// already have incremented the epoch after this call was published; resetting
/// the deadline to the new epoch + 1 would then let the guest run until fuel
/// exhaustion. Wasmtime 48 `current_epoch` / `increment_epoch` are Relaxed, so
/// observing that newer epoch does not synchronize with `cancelled`. The
/// deadline write and the cancel recheck therefore share `current_call` with
/// [`interrupt_if_current`]: either this call sees the flag and stays out, or
/// it armed the deadline before the increment and the guest is interrupted.
fn prepare_cancellable_store(
    store: &mut Store<PluginStore>,
    sandbox: SandboxLimits,
    cancelled: &AtomicBool,
    current_call: &Mutex<Option<Arc<AtomicBool>>>,
) -> Result<(), HostError> {
    store
        .set_fuel(sandbox.call_fuel)
        .map_err(|_| HostError::Trap)?;
    let _published = current_call
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    store.set_epoch_deadline(1);
    if cancelled.load(Ordering::Acquire) {
        return Err(HostError::Trap);
    }
    Ok(())
}

fn call_invoke(
    guest: &mut GuestKind,
    _engine: &Engine,
    request: &str,
    sandbox: SandboxLimits,
    cancelled: &AtomicBool,
    current_call: &Mutex<Option<Arc<AtomicBool>>>,
    entered: &mut bool,
) -> Result<String, HostError> {
    match guest {
        GuestKind::Wasm { bindings, store } => {
            prepare_cancellable_store(store, sandbox, cancelled, current_call)?;
            *entered = true;
            bindings
                .call_invoke(store, request)
                .map_err(|_| HostError::Trap)
        }
        #[cfg(feature = "test-util")]
        GuestKind::Fake(fake) => {
            if cancelled_under_current_call(current_call, cancelled) {
                return Err(HostError::Trap);
            }
            *entered = true;
            fake.invoke(request)
        }
    }
}

fn call_on_event(
    guest: &mut GuestKind,
    _engine: &Engine,
    payload: &str,
    sandbox: SandboxLimits,
) -> Result<String, HostError> {
    match guest {
        GuestKind::Wasm { bindings, store } => {
            prepare_store(store, sandbox)?;
            bindings
                .call_on_event(store, payload)
                .map_err(|_| HostError::Trap)
        }
        #[cfg(feature = "test-util")]
        GuestKind::Fake(fake) => fake.on_event(payload),
    }
}

fn call_render(
    guest: &mut GuestKind,
    _engine: &Engine,
    request: &str,
    sandbox: SandboxLimits,
    cancelled: &AtomicBool,
    current_call: &Mutex<Option<Arc<AtomicBool>>>,
    entered: &mut bool,
) -> Result<String, HostError> {
    match guest {
        GuestKind::Wasm { bindings, store } => {
            prepare_cancellable_store(store, sandbox, cancelled, current_call)?;
            *entered = true;
            bindings
                .call_render(store, request)
                .map_err(|_| HostError::Trap)
        }
        #[cfg(feature = "test-util")]
        GuestKind::Fake(fake) => {
            if cancelled_under_current_call(current_call, cancelled) {
                return Err(HostError::Trap);
            }
            *entered = true;
            fake.render(request)
        }
    }
}

pub(crate) fn instantiate_wasm(
    engine: &Engine,
    component: &Component,
    sandbox: SandboxLimits,
    grants: &CapabilityGrants,
    ui_declared: bool,
) -> Result<GuestKind, HostError> {
    let mut linker = Linker::new(engine);
    Plugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |state: &mut PluginStore| state)
        .map_err(|_| HostError::Instantiate)?;
    let mut store = Store::new(
        engine,
        PluginStore::new(sandbox.store_limits(), grants.clone(), ui_declared),
    );
    store.limiter(|state| &mut state.limits);
    prepare_store(&mut store, sandbox)?;
    let bindings =
        Plugin::instantiate(&mut store, component, &linker).map_err(|_| HostError::Instantiate)?;
    let output = bindings
        .call_construct(&mut store)
        .map_err(|_| HostError::Trap)?;
    parse_guest_error(&output).map_err(host_error_from_guest_parse)?;
    Ok(GuestKind::Wasm { bindings, store })
}
