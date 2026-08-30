// Rust guideline compliant 2026-08-31.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use super::owner::ActiveSegment;
use super::segment::SegmentExecution;
use mcode_plugin_api::MAX_TASK_GENERATION;

use super::{
    LifecycleErrorCode, LifecycleState, OPERATION_FUEL_BUDGET, PluginOwner, PluginRuntime,
    RuntimeError,
};

fn current_manager_source() -> String {
    include_str!("../../tests/fixtures/current_manager_component.wat").to_owned()
}

fn component_with_functions(initialize: &str, poll: &str, shutdown: &str) -> Vec<u8> {
    let source = replace_function(
        current_manager_source(),
        "    (func $initialize",
        "    (func $poll",
        initialize,
    );
    let source = replace_function(source, "    (func $poll", "    (func $shutdown", poll);
    let source = replace_function(
        source,
        "    (func $shutdown",
        "    (export \"initialize\"",
        shutdown,
    );
    wat::parse_str(source).expect("valid executable Manager component")
}

fn replace_function(source: String, start: &str, end: &str, replacement: &str) -> String {
    let start_index = source.find(start).expect("function start marker");
    let end_index = source[start_index..]
        .find(end)
        .map(|offset| start_index + offset)
        .expect("function end marker");
    format!(
        "{}{}{}",
        &source[..start_index],
        replacement,
        &source[end_index..]
    )
}

fn outcome_function(name: &str, parameter: &str, result_tag: i32, variant: i32) -> String {
    format!(
        "    (func ${name}{parameter} (result i32)\n\
         \x20     i32.const 0\n\
         \x20     i32.const {result_tag}\n\
         \x20     i32.store\n\
         \x20     i32.const 1\n\
         \x20     i32.const {variant}\n\
         \x20     i32.store\n\
         \x20     i32.const 0)\n"
    )
}

fn spin_function(name: &str, parameter: &str) -> String {
    format!(
        "    (func ${name}{parameter} (result i32)\n\
         \x20     (loop $forever (br $forever))\n\
         \x20     unreachable)\n"
    )
}

fn trap_function(name: &str, parameter: &str) -> String {
    format!("    (func ${name}{parameter} (result i32) unreachable)\n")
}

fn current_manager_component() -> Vec<u8> {
    wat::parse_str(current_manager_source()).expect("valid current Manager component")
}

fn effect_counting_component() -> Vec<u8> {
    let source = current_manager_source().replacen(
        "    (memory (export \"memory\") 1 1024)",
        "    (memory (export \"memory\") 1 1024)\n    (global $initializations (mut i32) (i32.const 0))",
        1,
    );
    let initialize = r#"    (func $initialize (param i64) (result i32)
      i32.const 0
      i32.const 0
      i32.store
      i32.const 1
      global.get $initializations
      i32.store
      global.get $initializations
      i32.const 1
      i32.add
      global.set $initializations
      i32.const 0)
"#;
    let source = replace_function(
        source,
        "    (func $initialize",
        "    (func $poll",
        initialize,
    );
    wat::parse_str(source).expect("valid effect-counting Manager component")
}

async fn instantiated(
    bytes: Vec<u8>,
) -> (PluginOwner, super::ManagerInstance, super::OperationLease) {
    let runtime = PluginRuntime::new();
    let component = runtime
        .compile_manager(bytes, crate::ComponentLimits::default())
        .expect("compile Manager");
    let mut owner = runtime.new_owner().expect("owner");
    let instance = owner
        .instantiate_manager(&component)
        .await
        .expect("instantiate Manager");
    let operation = owner.open_operation().expect("operation");
    (owner, instance, operation)
}

fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
    let mut context = Context::from_waker(Waker::noop());
    future.as_mut().poll(&mut context)
}

fn parked_fuel(owner: &PluginOwner) -> u64 {
    owner
        .store
        .as_ref()
        .expect("owner has Store")
        .get_fuel()
        .expect("fuel enabled")
}

#[tokio::test(flavor = "current_thread")]
async fn maps_every_lifecycle_state_and_error_exactly() {
    let first = component_with_functions(
        &outcome_function("initialize", " (param i64)", 0, 0),
        &outcome_function("poll", "", 0, 1),
        &outcome_function("shutdown", "", 0, 2),
    );
    let (mut owner, instance, mut operation) = instantiated(first).await;
    assert_eq!(
        instance.initialize(&mut owner, &mut operation, 1).await,
        Ok(Ok(LifecycleState::Ready))
    );
    assert_eq!(
        instance.poll(&mut owner, &mut operation).await,
        Ok(Ok(LifecycleState::Pending))
    );
    assert_eq!(
        instance.shutdown(&mut owner, &mut operation).await,
        Ok(Ok(LifecycleState::Stopping))
    );

    let second = component_with_functions(
        &outcome_function("initialize", " (param i64)", 0, 3),
        &outcome_function("poll", "", 1, 0),
        &outcome_function("shutdown", "", 1, 1),
    );
    let (mut owner, instance, mut operation) = instantiated(second).await;
    assert_eq!(
        instance.initialize(&mut owner, &mut operation, 1).await,
        Ok(Ok(LifecycleState::Stopped))
    );
    assert_eq!(
        instance.poll(&mut owner, &mut operation).await,
        Ok(Err(LifecycleErrorCode::InvalidState))
    );
    assert_eq!(
        instance.shutdown(&mut owner, &mut operation).await,
        Ok(Err(LifecycleErrorCode::FeatureUnavailable))
    );

    let third = component_with_functions(
        &outcome_function("initialize", " (param i64)", 1, 2),
        &outcome_function("poll", "", 0, 0),
        &outcome_function("shutdown", "", 0, 0),
    );
    let (mut owner, instance, mut operation) = instantiated(third).await;
    assert_eq!(
        instance.initialize(&mut owner, &mut operation, 1).await,
        Ok(Err(LifecycleErrorCode::Failed))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn generation_zero_and_n_plus_one_never_enter_guest() {
    let (mut owner, instance, mut operation) = instantiated(effect_counting_component()).await;
    let initial_fuel = operation.remaining();

    assert_eq!(
        instance.initialize(&mut owner, &mut operation, 0).await,
        Err(RuntimeError::InvalidGeneration)
    );
    assert_eq!(operation.remaining(), initial_fuel);
    assert_eq!(
        instance
            .initialize(&mut owner, &mut operation, MAX_TASK_GENERATION,)
            .await,
        Ok(Ok(LifecycleState::Ready))
    );
    let after_n = operation.remaining();
    assert!(after_n < initial_fuel);
    assert_eq!(
        instance
            .initialize(&mut owner, &mut operation, MAX_TASK_GENERATION + 1,)
            .await,
        Err(RuntimeError::InvalidGeneration)
    );
    assert_eq!(operation.remaining(), after_n);
    assert_eq!(
        instance.initialize(&mut owner, &mut operation, 1).await,
        Ok(Ok(LifecycleState::Pending))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn instance_and_operation_cannot_cross_store_owners() {
    let runtime = PluginRuntime::new();
    let component = runtime
        .compile_manager(
            current_manager_component(),
            crate::ComponentLimits::default(),
        )
        .expect("compile Manager");
    let mut first_owner = runtime.new_owner().expect("first owner");
    let instance = first_owner
        .instantiate_manager(&component)
        .await
        .expect("instantiate Manager");
    let mut first_operation = first_owner.open_operation().expect("first operation");
    let mut second_owner = runtime.new_owner().expect("second owner");
    let mut second_operation = second_owner.open_operation().expect("second operation");

    assert_eq!(
        instance.poll(&mut first_owner, &mut second_operation).await,
        Err(RuntimeError::OwnerMismatch)
    );
    assert_eq!(
        instance
            .poll(&mut second_owner, &mut second_operation)
            .await,
        Err(RuntimeError::InstanceMismatch)
    );
    assert_eq!(
        instance.poll(&mut first_owner, &mut first_operation).await,
        Ok(Ok(LifecycleState::Ready))
    );
    assert!(first_owner.is_available());
    assert!(second_owner.is_available());
}

#[tokio::test(flavor = "current_thread")]
async fn initialize_poll_and_shutdown_share_one_decreasing_fuel_remainder() {
    let (mut owner, instance, mut operation) = instantiated(current_manager_component()).await;

    assert_eq!(
        instance.initialize(&mut owner, &mut operation, 1).await,
        Ok(Ok(LifecycleState::Ready))
    );
    let after_initialize = operation.remaining();
    assert!(after_initialize < OPERATION_FUEL_BUDGET);
    assert_eq!(parked_fuel(&owner), 0);

    assert_eq!(
        instance.poll(&mut owner, &mut operation).await,
        Ok(Ok(LifecycleState::Ready))
    );
    let after_poll = operation.remaining();
    assert!(after_poll < after_initialize);
    assert_eq!(parked_fuel(&owner), 0);

    assert_eq!(
        instance.shutdown(&mut owner, &mut operation).await,
        Ok(Ok(LifecycleState::Ready))
    );
    assert!(operation.remaining() < after_poll);
    assert_eq!(parked_fuel(&owner), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn guest_trap_accounts_fuel_and_disposes_store() {
    let component = component_with_functions(
        &outcome_function("initialize", " (param i64)", 0, 0),
        &trap_function("poll", ""),
        &outcome_function("shutdown", "", 0, 3),
    );
    let (mut owner, instance, mut operation) = instantiated(component).await;

    assert_eq!(
        instance.poll(&mut owner, &mut operation).await,
        Err(RuntimeError::Guest)
    );
    assert!(operation.remaining() < OPERATION_FUEL_BUDGET);
    assert!(!owner.is_available());
    assert_eq!(
        instance.shutdown(&mut owner, &mut operation).await,
        Err(RuntimeError::StoreDisposed)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_component_call_accounts_fuel_and_disposes_store() {
    let component = component_with_functions(
        &outcome_function("initialize", " (param i64)", 0, 0),
        &spin_function("poll", ""),
        &outcome_function("shutdown", "", 0, 3),
    );
    let (mut owner, instance, mut operation) = instantiated(component).await;

    let mut pending_call = Box::pin(instance.poll(&mut owner, &mut operation));
    assert!(poll_once(pending_call.as_mut()).is_pending());
    drop(pending_call);

    assert!(operation.remaining() < OPERATION_FUEL_BUDGET);
    assert!(!owner.is_available());
    assert_eq!(
        instance.shutdown(&mut owner, &mut operation).await,
        Err(RuntimeError::StoreDisposed)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn component_call_with_unbounded_test_fuel_traps_at_epoch_deadline() {
    let component = component_with_functions(
        &outcome_function("initialize", " (param i64)", 0, 0),
        &spin_function("poll", ""),
        &outcome_function("shutdown", "", 0, 0),
    );
    let (mut owner, instance, mut operation) = instantiated(component).await;
    operation.remaining = u64::MAX;

    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(4),
        instance.poll(&mut owner, &mut operation),
    )
    .await;

    assert_eq!(result, Ok(Err(RuntimeError::Guest)));
    assert!(started.elapsed() >= Duration::from_millis(250));
    assert!(!owner.is_available());
}

#[test]
fn active_segment_and_fuel_increase_dispose_store() {
    let runtime = PluginRuntime::new();
    runtime
        .compile_manager(
            current_manager_component(),
            crate::ComponentLimits::default(),
        )
        .expect("initialize runtime");
    let mut owner = runtime.new_owner().expect("owner");
    let mut operation = owner.open_operation().expect("operation");
    let installed = operation.remaining();
    let mut execution = SegmentExecution::start(&mut owner, &mut operation).expect("segment");
    execution
        .store_mut()
        .set_fuel(installed + 1)
        .expect("mutate test fuel");

    assert_eq!(execution.complete(), Err(RuntimeError::FuelIncreased));
    assert!(!owner.is_available());

    let mut fresh_owner = runtime.new_owner().expect("fresh owner");
    let first_operation = fresh_owner.open_operation().expect("first operation");
    let mut second_operation = fresh_owner.open_operation().expect("second operation");
    let store = fresh_owner.store.as_mut().expect("Store");
    store.data_mut().active_segment = Some(ActiveSegment {
        owner: fresh_owner.identity.clone(),
        operation: first_operation.operation,
        installed: first_operation.remaining(),
    });
    assert_eq!(
        SegmentExecution::start(&mut fresh_owner, &mut second_operation).err(),
        Some(RuntimeError::SegmentActive)
    );
    assert!(!fresh_owner.is_available());
}
