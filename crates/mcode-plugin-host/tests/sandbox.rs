//! Sandbox denial tests: WASI imports, memory, and fuel/epoch.

mod common;

use std::time::Duration;

use mcode_plugin_api::CapabilityGrants;
use mcode_plugin_host::{
    EventDelivery, HostError, LifecycleState, compile_component, load_wasm_bytes, new_engine,
};

use common::{
    export_only_wat, huge_memory_wat, infinite_event_wat, model_event, parse_manifest,
    tight_limits, wasi_import_wat,
};

#[test]
fn wasi_imports_are_denied_before_instantiate() {
    let root = tempfile::tempdir().expect("tempdir");
    let manifest = parse_manifest(root.path(), "plugin.wasm", &[]);
    let error = load_wasm_bytes(
        &manifest,
        wasi_import_wat().as_bytes(),
        &CapabilityGrants::none(),
        1,
        tight_limits(),
    )
    .expect_err("wasi denied");
    assert!(matches!(
        error,
        HostError::ForbiddenImport | HostError::ImportMismatch | HostError::InvalidComponent
    ));
}

#[test]
fn host_interface_mismatch_is_fail_closed() {
    let root = tempfile::tempdir().expect("tempdir");
    let manifest = parse_manifest(
        root.path(),
        "plugin.wasm",
        &["mcode:plugin/host@0.1.0".into()],
    );
    let error = load_wasm_bytes(
        &manifest,
        export_only_wat().as_bytes(),
        &CapabilityGrants::none(),
        1,
        tight_limits(),
    )
    .expect_err("import mismatch");
    assert_eq!(error, HostError::ImportMismatch);
}

#[test]
fn oversized_linear_memory_is_rejected() {
    let engine = new_engine().expect("engine");
    let result = compile_component(&engine, huge_memory_wat());
    if result.is_err() {
        return;
    }
    let root = tempfile::tempdir().expect("tempdir");
    let manifest = parse_manifest(root.path(), "plugin.wasm", &[]);
    let error = load_wasm_bytes(
        &manifest,
        huge_memory_wat().as_bytes(),
        &CapabilityGrants::none(),
        1,
        tight_limits(),
    )
    .expect_err("memory denied");
    assert!(matches!(
        error,
        HostError::Instantiate | HostError::InvalidComponent | HostError::Trap
    ));
}

#[test]
fn infinite_on_event_is_interrupted() {
    let root = tempfile::tempdir().expect("tempdir");
    let manifest = parse_manifest(root.path(), "plugin.wasm", &[]);
    let handle = load_wasm_bytes(
        &manifest,
        infinite_event_wat().as_bytes(),
        &CapabilityGrants::none(),
        1,
        tight_limits(),
    )
    .expect("load looping guest");
    assert_eq!(handle.try_send_event(model_event()), EventDelivery::Queued);
    let started = std::time::Instant::now();
    while handle.state() != LifecycleState::Stopped {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "looping on-event must be isolated"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(handle.try_send_event(model_event()), EventDelivery::Closed);
    handle.stop().expect("join isolated actor");
    assert_eq!(handle.state(), LifecycleState::Stopped);
}
