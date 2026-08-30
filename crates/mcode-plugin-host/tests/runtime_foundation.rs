//! Public contracts for the opaque Wasmtime runtime foundation.

use mcode_plugin_host::runtime::{
    AdmissionError, MAX_LIVE_RESOURCES, MAX_OPEN_OPERATIONS, PluginRuntime, RuntimeError,
};
use mcode_plugin_host::{ComponentLimits, PreflightError};

fn current_manager_component() -> Vec<u8> {
    wat::parse_str(include_str!("fixtures/current_manager_component.wat"))
        .expect("valid bounded current Manager fixture")
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
            .compile_manager(core, ComponentLimits::default())
            .err(),
        Some(RuntimeError::Preflight(PreflightError::InvalidComponent))
    );
    assert_eq!(
        runtime.new_owner().err(),
        Some(RuntimeError::RuntimeUninitialized)
    );

    let wrong_shape_source = include_str!("fixtures/current_manager_component.wat").replacen(
        "(param \"request\" string)",
        "(param \"crossed-request\" string)",
        1,
    );
    assert!(wrong_shape_source.contains("crossed-request"));
    let wrong_shape = wat::parse_str(wrong_shape_source).expect("crossed-shape component fixture");
    assert_eq!(
        runtime
            .compile_manager(wrong_shape, ComponentLimits::default())
            .err(),
        Some(RuntimeError::Preflight(PreflightError::ImportShape))
    );
    assert_eq!(
        runtime.new_owner().err(),
        Some(RuntimeError::RuntimeUninitialized)
    );

    runtime
        .compile_manager(current_manager_component(), ComponentLimits::default())
        .expect("compile exact Manager after rejected shape");
    let recovered_owner = runtime.new_owner().expect("owner after exact Manager");
    assert!(recovered_owner.is_available());
}

#[tokio::test(flavor = "current_thread")]
async fn owner_instantiates_only_one_pack() {
    let runtime = PluginRuntime::new();
    let component = runtime
        .compile_manager(current_manager_component(), ComponentLimits::default())
        .expect("compile current 0.0.1 Manager");
    let mut owner = runtime.new_owner().expect("owner");

    owner
        .instantiate_manager(&component)
        .await
        .expect("bound asynchronous instantiation");
    assert_eq!(
        owner.instantiate_manager(&component).await.err(),
        Some(RuntimeError::InstanceActive)
    );
    assert!(owner.is_available());
}

#[tokio::test(flavor = "current_thread")]
async fn owner_rejects_component_from_another_runtime() {
    let first_runtime = PluginRuntime::new();
    let second_runtime = PluginRuntime::new();
    let component = first_runtime
        .compile_manager(current_manager_component(), ComponentLimits::default())
        .expect("compile first runtime component");
    second_runtime
        .compile_manager(current_manager_component(), ComponentLimits::default())
        .expect("initialize second runtime");
    let mut second_owner = second_runtime.new_owner().expect("second owner");

    assert_eq!(
        second_owner.instantiate_manager(&component).await.err(),
        Some(RuntimeError::RuntimeMismatch)
    );
    assert!(second_owner.is_available());
}

#[test]
fn owner_admission_accepts_n_rejects_n_plus_one_and_replaces_one() {
    let runtime = PluginRuntime::new();
    runtime
        .compile_manager(current_manager_component(), ComponentLimits::default())
        .expect("initialize runtime");
    let mut owner = runtime.new_owner().expect("owner");

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
