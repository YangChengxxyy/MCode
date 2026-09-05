//! Public contracts for the opaque Wasmtime runtime foundation.

use mcode_plugin_host::runtime::{
    AdmissionError, MAX_LIVE_RESOURCES, MAX_OPEN_OPERATIONS, PluginRuntime, RuntimeError,
};
use mcode_plugin_host::{ComponentLimits, ComponentWorld, PreflightError};

#[path = "support/preflight_fixtures.rs"]
mod fixtures;

fn provider_component() -> Vec<u8> {
    fixtures::canonical_component(ComponentWorld::Provider)
}

#[test]
fn runtime_requires_an_exact_component_before_owner_creation() {
    let runtime = PluginRuntime::new();
    assert_eq!(
        runtime.new_owner().err(),
        Some(RuntimeError::RuntimeUninitialized)
    );

    let core = wat::parse_str("(module)").expect("core module fixture");
    assert_eq!(
        runtime
            .compile_pack(core, ComponentWorld::Web, ComponentLimits::default())
            .err(),
        Some(RuntimeError::Preflight(PreflightError::InvalidComponent))
    );
    assert_eq!(
        runtime.new_owner().err(),
        Some(RuntimeError::RuntimeUninitialized)
    );

    runtime
        .compile_pack(
            provider_component(),
            ComponentWorld::Provider,
            ComponentLimits::default(),
        )
        .expect("compile exact Provider Pack after rejected core module");
    let owner = runtime.new_owner().expect("owner after exact Pack");
    assert!(owner.is_available());
}

#[test]
fn owner_admission_accepts_n_rejects_n_plus_one_and_replaces_one() {
    let runtime = PluginRuntime::new();
    runtime
        .compile_pack(
            provider_component(),
            ComponentWorld::Provider,
            ComponentLimits::default(),
        )
        .expect("initialize runtime");
    let owner = runtime.new_owner().expect("owner");

    let mut resources = (0..MAX_LIVE_RESOURCES)
        .map(|_| owner.admit_resource().expect("resource at N"))
        .collect::<Vec<_>>();
    assert_eq!(
        owner.admit_resource().err(),
        Some(RuntimeError::Admission(AdmissionError::ResourceCapacity))
    );
    drop(resources.pop().expect("one resource"));
    resources.push(owner.admit_resource().expect("replacement resource"));
    assert_eq!(
        owner.admit_resource().err(),
        Some(RuntimeError::Admission(AdmissionError::ResourceCapacity))
    );
    drop(resources);

    let mut owner = owner;
    let mut operations = (0..MAX_OPEN_OPERATIONS)
        .map(|_| owner.open_operation().expect("operation at N"))
        .collect::<Vec<_>>();
    assert_eq!(
        owner.open_operation().err(),
        Some(RuntimeError::Admission(AdmissionError::OperationCapacity))
    );
    drop(operations.pop().expect("one operation"));
    operations.push(owner.open_operation().expect("replacement operation"));
    assert_eq!(
        owner.open_operation().err(),
        Some(RuntimeError::Admission(AdmissionError::OperationCapacity))
    );
}
