//! Race and fault tests for disable fencing.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mcode_plugin_api::{
    CapabilityGrants, GuestInvokeRequest, GuestInvokeTarget, GuestRenderRequest, Identifier,
    PluginId,
};
use mcode_plugin_host::test_util::{FakeGuest, spawn_fake_generation};
use mcode_plugin_host::{
    EventDelivery, HostError, LifecycleState, RuntimeLimits, SandboxLimits, load_wasm_bytes,
};

use common::{infinite_event_wat, infinite_invoke_render_wat, model_event, parse_manifest};

struct GatedGuest {
    events: Arc<AtomicUsize>,
    entered: Arc<Mutex<bool>>,
    release: Arc<Mutex<bool>>,
}

impl FakeGuest for GatedGuest {
    fn on_event(&mut self, _event: &str) -> Result<String, HostError> {
        let prior = self.events.fetch_add(1, Ordering::AcqRel);
        if prior == 0 {
            *self
                .entered
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
            loop {
                if *self
                    .release
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        Ok(String::new())
    }
}

#[test]
fn queued_event_does_not_enter_guest_after_disable() {
    let events = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Mutex::new(false));
    let release = Arc::new(Mutex::new(false));
    let handle = spawn_fake_generation(
        PluginId::parse("com.mcode.race").expect("id"),
        4,
        GatedGuest {
            events: events.clone(),
            entered: entered.clone(),
            release: release.clone(),
        },
        RuntimeLimits::default(),
    )
    .expect("spawn");
    assert_eq!(handle.try_send_event(model_event()), EventDelivery::Queued);
    let started = std::time::Instant::now();
    while !*entered
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
    {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "guest did not enter"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(handle.try_send_event(model_event()), EventDelivery::Queued);
    let stopper = handle.clone();
    let join = std::thread::spawn(move || stopper.disable());
    std::thread::sleep(Duration::from_millis(20));
    *release
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    join.join().expect("disable thread").expect("disable");
    assert_eq!(
        events.load(Ordering::Acquire),
        1,
        "on_event must not first enter after disable"
    );
}

#[test]
fn stale_generation_is_rejected_without_guest_entry() {
    let events = Arc::new(AtomicUsize::new(0));
    let handle = spawn_fake_generation(
        PluginId::parse("com.mcode.stale").expect("id"),
        2,
        GatedGuest {
            events: events.clone(),
            entered: Arc::new(Mutex::new(false)),
            release: Arc::new(Mutex::new(true)),
        },
        RuntimeLimits::default(),
    )
    .expect("spawn");
    handle.disable().expect("disable");
    assert_eq!(handle.try_send_event(model_event()), EventDelivery::Closed);
    assert_eq!(events.load(Ordering::Acquire), 0);
}

struct HoldEventCountInvoke {
    entered: Arc<Mutex<bool>>,
    release: Arc<Mutex<bool>>,
    invokes: Arc<AtomicUsize>,
}

impl FakeGuest for HoldEventCountInvoke {
    fn on_event(&mut self, _event: &str) -> Result<String, HostError> {
        *self
            .entered
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        loop {
            if *self
                .release
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(String::new())
    }

    fn invoke(&mut self, _request: &str) -> Result<String, HostError> {
        self.invokes.fetch_add(1, Ordering::AcqRel);
        Ok("{}".into())
    }
}

#[test]
fn timed_out_invoke_does_not_enter_guest() {
    let entered = Arc::new(Mutex::new(false));
    let release = Arc::new(Mutex::new(false));
    let invokes = Arc::new(AtomicUsize::new(0));
    let limits = RuntimeLimits::new(
        8,
        32 * 1024,
        Duration::from_millis(40),
        Duration::from_secs(2),
        SandboxLimits::default(),
    )
    .expect("limits");
    let handle = spawn_fake_generation(
        PluginId::parse("com.mcode.timeout").expect("id"),
        4,
        HoldEventCountInvoke {
            entered: entered.clone(),
            release: release.clone(),
            invokes: invokes.clone(),
        },
        limits,
    )
    .expect("spawn");
    assert_eq!(handle.try_send_event(model_event()), EventDelivery::Queued);
    let started = std::time::Instant::now();
    while !*entered
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
    {
        assert!(started.elapsed() < Duration::from_secs(2), "guest entered");
        std::thread::sleep(Duration::from_millis(5));
    }
    let caller = handle.clone();
    let join = std::thread::spawn(move || {
        caller.invoke(GuestInvokeRequest {
            call_id: Identifier::parse("call_1").expect("id"),
            target: GuestInvokeTarget::Tool {
                id: Identifier::parse("tool.main").expect("id"),
            },
            generation: 4,
            input: serde_json::json!({}),
        })
    });
    std::thread::sleep(Duration::from_millis(20));
    let result = join.join().expect("invoke thread");
    assert_eq!(result, Err(HostError::Trap));
    *release
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        invokes.load(Ordering::Acquire),
        0,
        "cancelled invoke must not enter guest"
    );
    assert_eq!(handle.state(), LifecycleState::Running);
    handle.stop().expect("stop");
}

fn invoke_request(generation: u64) -> GuestInvokeRequest {
    GuestInvokeRequest {
        call_id: Identifier::parse("call_1").expect("id"),
        target: GuestInvokeTarget::Tool {
            id: Identifier::parse("tool.main").expect("id"),
        },
        generation,
        input: serde_json::json!({}),
    }
}

/// Fuel high enough that an infinite export only stops on epoch interrupt.
const EPOCH_ONLY_FUEL: u64 = u64::MAX / 4;

fn epoch_only_limits(call_timeout: Duration) -> RuntimeLimits {
    RuntimeLimits::new(
        8,
        32 * 1024,
        call_timeout,
        Duration::from_secs(2),
        SandboxLimits::new(EPOCH_ONLY_FUEL, 2 * 1024 * 1024, 1024, 4, 4, 4).expect("sandbox"),
    )
    .expect("limits")
}

fn load_looping_guest(wat: &str, limits: RuntimeLimits) -> mcode_plugin_host::PluginHandle {
    let root = tempfile::tempdir().expect("tempdir");
    let manifest = parse_manifest(root.path(), "plugin.wasm", &[]);
    load_wasm_bytes(
        &manifest,
        wat.as_bytes(),
        &CapabilityGrants::none(),
        1,
        limits,
    )
    .expect("load looping guest")
}

fn render_request(generation: u64) -> GuestRenderRequest {
    GuestRenderRequest {
        view_id: Identifier::parse("status.main").expect("id"),
        generation,
    }
}

fn wait_stopped(handle: &mcode_plugin_host::PluginHandle, message: &str) {
    let started = std::time::Instant::now();
    while handle.state() != LifecycleState::Stopped {
        assert!(started.elapsed() < Duration::from_secs(2), "{message}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn queued_invoke_timeout_does_not_epoch_cancel_on_event() {
    let handle = load_looping_guest(
        infinite_event_wat(),
        epoch_only_limits(Duration::from_millis(40)),
    );
    assert_eq!(handle.try_send_event(model_event()), EventDelivery::Queued);
    std::thread::sleep(Duration::from_millis(30));
    let caller = handle.clone();
    let join = std::thread::spawn(move || caller.invoke(invoke_request(1)));
    assert_eq!(join.join().expect("invoke thread"), Err(HostError::Trap));
    std::thread::sleep(Duration::from_millis(80));
    assert_eq!(
        handle.state(),
        LifecycleState::Running,
        "queued invoke timeout must not isolate a running on-event via epoch"
    );
    assert_eq!(handle.try_send_event(model_event()), EventDelivery::Queued);
    handle.stop().expect("stop");
    assert_eq!(handle.state(), LifecycleState::Stopped);
}

#[test]
fn in_flight_invoke_timeout_isolates_generation() {
    let handle = load_looping_guest(
        infinite_invoke_render_wat(),
        epoch_only_limits(Duration::from_millis(80)),
    );
    let caller = handle.clone();
    let join = std::thread::spawn(move || caller.invoke(invoke_request(1)));
    assert_eq!(join.join().expect("invoke thread"), Err(HostError::Trap));
    wait_stopped(
        &handle,
        "in-flight invoke epoch interrupt must isolate the generation",
    );
    assert_eq!(handle.try_send_event(model_event()), EventDelivery::Closed);
    assert_eq!(handle.invoke(invoke_request(1)), Err(HostError::NotRunning));
    handle.stop().expect("join isolated actor");
    assert_eq!(handle.state(), LifecycleState::Stopped);
}

#[test]
fn in_flight_render_timeout_isolates_generation() {
    let handle = load_looping_guest(
        infinite_invoke_render_wat(),
        epoch_only_limits(Duration::from_millis(80)),
    );
    let caller = handle.clone();
    let join = std::thread::spawn(move || caller.render(render_request(1)));
    assert_eq!(join.join().expect("render thread"), Err(HostError::Trap));
    wait_stopped(
        &handle,
        "in-flight render epoch interrupt must isolate the generation",
    );
    assert_eq!(handle.try_send_event(model_event()), EventDelivery::Closed);
    assert_eq!(handle.render(render_request(1)), Err(HostError::NotRunning));
    handle.stop().expect("join isolated actor");
    assert_eq!(handle.state(), LifecycleState::Stopped);
}
