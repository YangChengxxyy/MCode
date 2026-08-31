//! Sole-current Manager, FeaturePack, and ProviderPack contract tests.

use mcode_plugin_api::{
    FEATURE_PACK_WIT_PACKAGE, FEATURE_SERVICE_INTERFACE_ID, MANAGER_JSON_ABI_VERSION,
    MANAGER_LIFECYCLE_INTERFACE_ID, MANAGER_TASKS_INTERFACE_ID, MANAGER_WIT, MANAGER_WIT_PACKAGE,
    MANAGER_WORLD, MANAGER_WORLD_ID, MANAGER_WORLD_VERSION, PROVIDER_INTERFACE,
    PROVIDER_INTERFACE_ID, PROVIDER_WIT_PACKAGE, PROVIDER_WORLD, PROVIDER_WORLD_ID,
    PROVIDER_WORLD_VERSION,
};

#[test]
fn manager_world_matches_the_current_golden() {
    assert_eq!(MANAGER_JSON_ABI_VERSION, "0.0.1");
    assert_eq!(MANAGER_WIT_PACKAGE, "mcode:plugin@0.0.1");
    assert_eq!(MANAGER_WORLD, "manager");
    assert_eq!(MANAGER_WORLD_VERSION, "0.0.1");
    assert_eq!(MANAGER_WORLD_ID, "mcode:plugin/manager@0.0.1");
    assert_eq!(
        FEATURE_SERVICE_INTERFACE_ID,
        "mcode:plugin/feature-service@0.0.1"
    );
    assert_eq!(
        MANAGER_LIFECYCLE_INTERFACE_ID,
        "mcode:plugin/manager-lifecycle@0.0.1"
    );
    assert_eq!(
        MANAGER_TASKS_INTERFACE_ID,
        "mcode:plugin/manager-tasks@0.0.1"
    );

    let golden = include_bytes!("../goldens/manager_current.wit");
    assert_eq!(MANAGER_WIT.as_bytes(), golden);
    assert!(!MANAGER_WIT.as_bytes().contains(&b'\r'));
}

#[test]
fn feature_pack_package_is_sole_current() {
    assert_eq!(FEATURE_PACK_WIT_PACKAGE, "mcode:feature-pack@0.0.1");
}

#[test]
fn provider_constants_identify_the_sole_current_contract() {
    assert_eq!(PROVIDER_WIT_PACKAGE, "mcode:provider-pack@0.0.1");
    assert_eq!(PROVIDER_WORLD, "provider");
    assert_eq!(PROVIDER_WORLD_VERSION, "0.0.1");
    assert_eq!(PROVIDER_WORLD_ID, "mcode:provider-pack/provider@0.0.1");
    assert_eq!(PROVIDER_INTERFACE, "provider-api");
    assert_eq!(
        PROVIDER_INTERFACE_ID,
        "mcode:provider-pack/provider-api@0.0.1"
    );
}

// Rust guideline compliant 2026-08-30.
