//! Fake-component lifecycle tests.

mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use mcode_plugin_api::{GuestInvokeRequest, GuestInvokeTarget, Identifier, PluginId};
use mcode_plugin_host::test_util::{
    BlockingGuest, CountingGuest, FakeGuest, spawn_fake_generation,
};
use mcode_plugin_host::{EventDelivery, HostError, LifecycleState, RuntimeLimits};

use common::model_event;

#[test]
fn fake_component_invoke_and_event_then_stop() {
    let guest = CountingGuest::new();
    let events = guest.events.clone();
    let invokes = guest.invokes.clone();
    let handle = spawn_fake_generation(
        PluginId::parse("com.mcode.fake").expect("id"),
        3,
        guest,
        RuntimeLimits::default(),
    )
    .expect("spawn");
    assert_eq!(handle.state(), LifecycleState::Running);
    assert_eq!(handle.try_send_event(model_event()), EventDelivery::Queued);
    handle
        .invoke(GuestInvokeRequest {
            call_id: Identifier::parse("call_1").expect("id"),
            target: GuestInvokeTarget::Tool {
                id: Identifier::parse("tool.main").expect("id"),
            },
            generation: 3,
            input: serde_json::json!({}),
        })
        .expect("invoke");
    std::thread::sleep(Duration::from_millis(50));
    assert!(events.load(Ordering::Acquire) >= 1);
    assert!(invokes.load(Ordering::Acquire) >= 1);
    handle.stop().expect("stop");
    assert_eq!(handle.state(), LifecycleState::Stopped);
    assert_eq!(handle.try_send_event(model_event()), EventDelivery::Closed);
}

#[test]
fn disable_prevents_first_guest_entry() {
    let guest = CountingGuest::new();
    let events = guest.events.clone();
    let handle = spawn_fake_generation(
        PluginId::parse("com.mcode.fake").expect("id"),
        9,
        guest,
        RuntimeLimits::default(),
    )
    .expect("spawn");
    handle.disable().expect("disable");
    assert_eq!(handle.try_send_event(model_event()), EventDelivery::Closed);
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(events.load(Ordering::Acquire), 0);
    let _ = Arc::strong_count(&events);
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

#[test]
fn stale_request_generation_does_not_enter_guest() {
    let guest = CountingGuest::new();
    let invokes = guest.invokes.clone();
    let handle = spawn_fake_generation(
        PluginId::parse("com.mcode.fake").expect("id"),
        3,
        guest,
        RuntimeLimits::default(),
    )
    .expect("spawn");
    assert_eq!(
        handle.invoke(invoke_request(1)),
        Err(HostError::StaleGeneration)
    );
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(invokes.load(Ordering::Acquire), 0);
    handle.stop().expect("stop");
}

struct TrapOnEvent;

impl FakeGuest for TrapOnEvent {
    fn on_event(&mut self, _event: &str) -> Result<String, HostError> {
        Err(HostError::Trap)
    }
}

#[test]
fn on_event_trap_isolates_plugin() {
    let handle = spawn_fake_generation(
        PluginId::parse("com.mcode.trap").expect("id"),
        1,
        TrapOnEvent,
        RuntimeLimits::default(),
    )
    .expect("spawn");
    assert_eq!(handle.try_send_event(model_event()), EventDelivery::Queued);
    let started = std::time::Instant::now();
    while handle.state() != LifecycleState::Stopped {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "trapping on-event must isolate"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(handle.try_send_event(model_event()), EventDelivery::Closed);
    handle.stop().expect("join isolated actor");
}

struct DropGuest {
    dropped: Arc<std::sync::atomic::AtomicUsize>,
}

impl Drop for DropGuest {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::AcqRel);
    }
}

impl FakeGuest for DropGuest {}

#[test]
fn dropping_last_handle_stops_actor() {
    let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let handle = spawn_fake_generation(
        PluginId::parse("com.mcode.drop").expect("id"),
        1,
        DropGuest {
            dropped: dropped.clone(),
        },
        RuntimeLimits::default(),
    )
    .expect("spawn");
    drop(handle);
    let started = std::time::Instant::now();
    while dropped.load(Ordering::Acquire) == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "last handle drop must join actor"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn concurrent_stop_serializes_and_stop_timeout_keeps_join_handle() {
    let entered = Arc::new(std::sync::Mutex::new(false));
    let release = Arc::new(std::sync::Mutex::new(false));
    let limits = RuntimeLimits::new(
        8,
        32 * 1024,
        Duration::from_secs(5),
        Duration::from_millis(40),
        mcode_plugin_host::SandboxLimits::default(),
    )
    .expect("limits");
    let handle = spawn_fake_generation(
        PluginId::parse("com.mcode.stop").expect("id"),
        1,
        BlockingGuest {
            entered: entered.clone(),
            release: release.clone(),
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
    assert_eq!(handle.stop(), Err(HostError::StopTimeout));
    assert_eq!(handle.state(), LifecycleState::Stopping);
    let clone = handle.clone();
    let first = handle.clone();
    let second = handle.clone();
    let a = std::thread::spawn(move || first.stop());
    let b = std::thread::spawn(move || second.stop());
    *release
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    let ra = a.join().expect("stop a");
    let rb = b.join().expect("stop b");
    assert!(ra.is_ok() || matches!(ra, Err(HostError::StopTimeout)));
    assert!(rb.is_ok() || matches!(rb, Err(HostError::StopTimeout)));
    if clone.state() != LifecycleState::Stopped {
        clone.stop().expect("retry stop after release");
    }
    assert_eq!(clone.state(), LifecycleState::Stopped);
}

struct BlockingDropGuest {
    entered: Arc<std::sync::Mutex<bool>>,
    release: Arc<std::sync::Mutex<bool>>,
    dropped: Arc<std::sync::atomic::AtomicUsize>,
}

impl Drop for BlockingDropGuest {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::AcqRel);
    }
}

impl FakeGuest for BlockingDropGuest {
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
}

#[test]
fn dropping_last_handle_joins_busy_actor() {
    let entered = Arc::new(std::sync::Mutex::new(false));
    let release = Arc::new(std::sync::Mutex::new(false));
    let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let limits = RuntimeLimits::new(
        8,
        32 * 1024,
        Duration::from_secs(5),
        Duration::from_millis(40),
        mcode_plugin_host::SandboxLimits::default(),
    )
    .expect("limits");
    let handle = spawn_fake_generation(
        PluginId::parse("com.mcode.dropbusy").expect("id"),
        1,
        BlockingDropGuest {
            entered: entered.clone(),
            release: release.clone(),
            dropped: dropped.clone(),
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
    let dropper = std::thread::spawn(move || drop(handle));
    std::thread::sleep(Duration::from_millis(80));
    assert!(
        !dropper.is_finished(),
        "last handle Drop must keep joining after StopTimeout"
    );
    assert_eq!(dropped.load(Ordering::Acquire), 0);
    *release
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    dropper.join().expect("drop thread");
    assert_eq!(dropped.load(Ordering::Acquire), 1);
}

#[test]
fn stop_with_max_shutdown_timeout_does_not_panic() {
    let limits = RuntimeLimits::new(
        8,
        32 * 1024,
        Duration::from_secs(5),
        Duration::MAX,
        mcode_plugin_host::SandboxLimits::default(),
    )
    .expect("limits");
    let handle = spawn_fake_generation(
        PluginId::parse("com.mcode.maxtimeout").expect("id"),
        1,
        CountingGuest::new(),
        limits,
    )
    .expect("spawn");
    handle.stop().expect("stop");
    assert_eq!(handle.state(), LifecycleState::Stopped);
}
