//! Current Manager component compile and preflight contract tests.

use mcode_plugin_host::{
    ComponentLimits, ImportCategory, PreflightError, preflight_manager_component,
};

const FEATURE_SERVICE_ID: &str = "mcode:plugin/feature-service@0.0.1";
const LIFECYCLE_ID: &str = "mcode:plugin/manager-lifecycle@0.0.1";

fn component_binary(text: &str) -> Vec<u8> {
    let component = wat::parse_str(text).expect("valid component fixture");
    assert!(component.starts_with(b"\0asm\x0d\0\x01\0"));
    component
}

fn feature_import(result: &str) -> String {
    format!(
        r#"(import "{FEATURE_SERVICE_ID}" (instance
    (export "start-task" (func (param "request" string) (result {result})))
    (export "poll-task" (func (param "request" string) (result string)))
    (export "cancel-task" (func (param "request" string) (result string)))
  ))"#
    )
}

fn current_component() -> String {
    format!(
        r#"
(component
  {feature_import}
  (core module $guest
    (memory (export "memory") 1)
    (func $initialize (param i64) (result i32)
      i32.const 0
      i32.const 0
      i32.store
      i32.const 4
      i32.const 0
      i32.store
      i32.const 0)
    (func $poll (result i32)
      i32.const 0
      i32.const 0
      i32.store
      i32.const 4
      i32.const 0
      i32.store
      i32.const 0)
    (func $shutdown (result i32)
      i32.const 0
      i32.const 0
      i32.store
      i32.const 4
      i32.const 0
      i32.store
      i32.const 0)
    (export "initialize" (func $initialize))
    (export "poll" (func $poll))
    (export "shutdown" (func $shutdown))
  )
  (core instance $guest-instance (instantiate $guest))
  (alias core export $guest-instance "memory" (core memory $memory))
  (alias core export $guest-instance "initialize" (core func $core-initialize))
  (alias core export $guest-instance "poll" (core func $core-poll))
  (alias core export $guest-instance "shutdown" (core func $core-shutdown))

  (type $initialization-context (record (field "generation" u64)))
  (type $state (enum "ready" "pending" "stopping" "stopped"))
  (type $error-code (enum "invalid-state" "feature-unavailable" "failed"))
  (type $outcome (result $state (error $error-code)))
  (type $initialize-func
    (func (param "context" $initialization-context) (result $outcome)))
  (type $lifecycle-func (func (result $outcome)))
  (func $initialize (type $initialize-func)
    (canon lift (core func $core-initialize) (memory $memory)))
  (func $poll (type $lifecycle-func)
    (canon lift (core func $core-poll) (memory $memory)))
  (func $shutdown (type $lifecycle-func)
    (canon lift (core func $core-shutdown) (memory $memory)))

  (component $lifecycle-shim
    (type (record (field "generation" u64)))
    (import "import-initialization-context" (type (eq 0)))
    (type (enum "ready" "pending" "stopping" "stopped"))
    (import "import-state" (type (eq 2)))
    (type (enum "invalid-state" "feature-unavailable" "failed"))
    (import "import-error-code" (type (eq 4)))
    (type (result 3 (error 5)))
    (type (func (param "context" 1) (result 6)))
    (type (func (result 6)))
    (import "import-initialize" (func (type 7)))
    (import "import-poll" (func (type 8)))
    (import "import-shutdown" (func (type 8)))
    (type (record (field "generation" u64)))
    (export "initialization-context" (type 9))
    (type (enum "ready" "pending" "stopping" "stopped"))
    (export "state" (type 11))
    (type (enum "invalid-state" "feature-unavailable" "failed"))
    (export "error-code" (type 13))
    (type (result 12 (error 14)))
    (type (func (param "context" 10) (result 15)))
    (type (func (result 15)))
    (export "initialize" (func 0) (func (type 16)))
    (export "poll" (func 1) (func (type 17)))
    (export "shutdown" (func 2) (func (type 17)))
  )
  (instance $lifecycle (instantiate $lifecycle-shim
    (with "import-initialize" (func $initialize))
    (with "import-poll" (func $poll))
    (with "import-shutdown" (func $shutdown))
    (with "import-initialization-context" (type $initialization-context))
    (with "import-state" (type $state))
    (with "import-error-code" (type $error-code))
  ))
  (export "{LIFECYCLE_ID}" (instance $lifecycle))
)
"#,
        feature_import = feature_import("string")
    )
}

fn no_arg_initialize_component() -> String {
    current_component()
        .replacen(
            "(func $initialize (param i64) (result i32)",
            "(func $initialize (result i32)",
            1,
        )
        .replacen(
            "(type $initialize-func\n    (func (param \"context\" $initialization-context) (result $outcome)))",
            "(type $initialize-func (func (result $outcome)))",
            1,
        )
        .replacen(
            "(type (func (param \"context\" 1) (result 6)))",
            "(type (func (result 6)))",
            1,
        )
        .replacen(
            "(type (func (param \"context\" 10) (result 15)))",
            "(type (func (result 15)))",
            1,
        )
}

fn component_with_import(name: &str) -> String {
    format!(
        r#"(component
  (import "{name}" (instance
    (export "denied" (func))
  ))
)"#
    )
}

fn component_with_extra_import(name: &str) -> String {
    format!(
        r#"(component
  {feature_import}
  (import "{name}" (instance
    (export "denied" (func))
  ))
)"#,
        feature_import = feature_import("string")
    )
}

fn wrong_export_component() -> String {
    format!(
        r#"(component
  {feature_import}
  (component $empty)
  (instance $empty-instance (instantiate $empty))
  (export "{LIFECYCLE_ID}" (instance $empty-instance))
)"#,
        feature_import = feature_import("string")
    )
}

#[test]
fn current_bindings_preflight_without_instantiation() {
    let component = component_binary(&current_component());
    preflight_manager_component(&component, ComponentLimits::default())
        .expect("current Manager component");
}

#[test]
fn plaintext_wat_is_rejected_at_binary_boundary() {
    let component = current_component();
    assert!(!component.as_bytes().starts_with(b"\0asm"));
    assert_eq!(
        preflight_manager_component(component.as_bytes(), ComponentLimits::default())
            .expect_err("plaintext WAT"),
        PreflightError::InvalidComponent
    );
}

#[test]
fn core_wasm_binary_is_rejected_at_component_boundary() {
    let module = wat::parse_str("(module)").expect("core module fixture");
    assert!(module.starts_with(b"\0asm\x01\0\0\0"));
    assert_eq!(
        preflight_manager_component(&module, ComponentLimits::default())
            .expect_err("core wasm binary"),
        PreflightError::InvalidComponent
    );
}

#[test]
fn initialize_requires_host_bound_generation_context() {
    let component = no_arg_initialize_component();
    assert!(!component.contains("(func $initialize (param i64)"));
    assert!(!component.contains("(param \"context\""));
    let component = component_binary(&component);
    assert_eq!(
        preflight_manager_component(&component, ComponentLimits::default())
            .expect_err("no-argument initialize"),
        PreflightError::ExportShape
    );
}

#[test]
fn compile_input_is_bounded_before_wasmtime_compile() {
    let component = component_binary(&current_component());
    let limits = ComponentLimits::new(component.len() - 1).expect("positive limit");
    assert_eq!(
        preflight_manager_component(&component, limits).expect_err("oversized"),
        PreflightError::ComponentTooLarge
    );
    assert_eq!(
        ComponentLimits::new(0).expect_err("zero limit"),
        PreflightError::InvalidLimits
    );
}

#[test]
fn every_noncurrent_mcode_import_is_rejected_before_shape_matching() {
    for name in [
        "mcode:plugin/feature-service@0.0.2",
        "mcode:plugin/feature-service@0.2.0",
        "mcode:plugin/host@0.1.0",
        "mcode:plugin/manager-lifecycle@0.0.2",
        "mcode:plugin/other@0.0.1",
    ] {
        let noncurrent = component_binary(&component_with_import(name));
        assert_eq!(
            preflight_manager_component(&noncurrent, ComponentLimits::default()).expect_err(name),
            PreflightError::DeniedImport(ImportCategory::MCodeVersion),
            "{name}"
        );
    }
}

#[test]
fn noncurrent_manager_lifecycle_exports_are_rejected() {
    for name in [
        "mcode:plugin/manager-lifecycle@0.0.2",
        "mcode:plugin/manager-lifecycle@0.2.0",
    ] {
        let noncurrent = current_component().replacen(LIFECYCLE_ID, name, 1);
        let noncurrent = component_binary(&noncurrent);
        assert_eq!(
            preflight_manager_component(&noncurrent, ComponentLimits::default()).expect_err(name),
            PreflightError::UnexpectedExport,
            "{name}"
        );
    }
}

#[test]
fn every_ambient_wasi_category_is_rejected() {
    let fixtures = [
        ("wasi:filesystem/types@0.2.0", ImportCategory::Filesystem),
        ("wasi:sockets/tcp@0.2.0", ImportCategory::Network),
        ("wasi:cli/run@0.2.0", ImportCategory::Process),
        ("wasi:cli/terminal-input@0.2.0", ImportCategory::Terminal),
        ("wasi:http/outgoing-handler@0.2.0", ImportCategory::Http),
        ("wasi:random/random@0.2.0", ImportCategory::Random),
        ("wasi:clocks/monotonic-clock@0.2.0", ImportCategory::Clocks),
        ("wasi:keyvalue/store@0.2.0", ImportCategory::Secret),
        ("wasi:logging/logging@0.1.0", ImportCategory::Logging),
        ("wasi:io/streams@0.2.0", ImportCategory::Wasi),
    ];

    for (name, category) in fixtures {
        let component = component_binary(&component_with_import(name));
        assert_eq!(
            preflight_manager_component(&component, ComponentLimits::default()).expect_err(name),
            PreflightError::DeniedImport(category),
            "{name}"
        );
    }
}

#[test]
fn extra_log_ui_and_raw_host_interfaces_are_rejected() {
    let fixtures = [
        ("mcode:host/log@1.0.0", ImportCategory::Logging),
        ("mcode:host/ui@1.0.0", ImportCategory::UserInterface),
        ("mcode:host/raw-handles@1.0.0", ImportCategory::RawHost),
        ("vendor:extra/gateway@1.0.0", ImportCategory::Extra),
    ];

    for (name, category) in fixtures {
        let component = component_binary(&component_with_extra_import(name));
        assert_eq!(
            preflight_manager_component(&component, ComponentLimits::default()).expect_err(name),
            PreflightError::DeniedImport(category),
            "{name}"
        );
    }
}

#[test]
fn renamed_feature_parameters_are_rejected() {
    for function in ["start-task", "poll-task", "cancel-task"] {
        let expected =
            format!(r#"(export "{function}" (func (param "request" string) (result string)))"#);
        let renamed =
            format!(r#"(export "{function}" (func (param "payload" string) (result string)))"#);
        let component = current_component().replacen(&expected, &renamed, 1);
        assert!(component.contains(&renamed), "{function}");
        let component = component_binary(&component);

        assert_eq!(
            preflight_manager_component(&component, ComponentLimits::default())
                .expect_err(function),
            PreflightError::ImportShape,
            "{function}"
        );
    }
}

#[test]
fn matching_names_with_crossed_shapes_are_rejected() {
    let extra_import_member = current_component().replacen(
        r#"(export "cancel-task" (func (param "request" string) (result string)))"#,
        r#"(export "cancel-task" (func (param "request" string) (result string)))
    (export "debug" (func))"#,
        1,
    );
    let extra_import_member = component_binary(&extra_import_member);
    assert_eq!(
        preflight_manager_component(&extra_import_member, ComponentLimits::default())
            .expect_err("extra import member"),
        PreflightError::ImportShape
    );

    let wrong_import = format!(
        r#"(component
  {feature_import}
  (component $empty)
  (instance $empty-instance (instantiate $empty))
  (export "{LIFECYCLE_ID}" (instance $empty-instance))
)"#,
        feature_import = feature_import("u32")
    );
    let wrong_import = component_binary(&wrong_import);
    assert_eq!(
        preflight_manager_component(&wrong_import, ComponentLimits::default())
            .expect_err("crossed import"),
        PreflightError::ImportShape
    );

    let wrong_export = component_binary(&wrong_export_component());
    assert_eq!(
        preflight_manager_component(&wrong_export, ComponentLimits::default())
            .expect_err("crossed export"),
        PreflightError::ExportShape
    );

    let extra_export_member = current_component().replacen(
        r#"(export "shutdown" (func 2) (func (type 17)))"#,
        r#"(export "shutdown" (func 2) (func (type 17)))
    (export "debug" (func 2) (func (type 17)))"#,
        1,
    );
    assert!(extra_export_member.contains(r#"(export "debug""#));
    let extra_export_member = component_binary(&extra_export_member);
    assert_eq!(
        preflight_manager_component(&extra_export_member, ComponentLimits::default())
            .expect_err("extra export member"),
        PreflightError::ExportShape
    );
}
