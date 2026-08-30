//! Sole-current Manager contract tests.

use mcode_plugin_api::{
    FEATURE_SERVICE_INTERFACE_ID, MANAGER_JSON_ABI_VERSION, MANAGER_LIFECYCLE_INTERFACE_ID,
    MANAGER_WIT, MANAGER_WIT_PACKAGE, MANAGER_WORLD, MANAGER_WORLD_ID, MANAGER_WORLD_VERSION,
};

#[test]
fn manager_world_matches_the_current_golden() {
    assert_eq!(MANAGER_JSON_ABI_VERSION, 2);
    assert_eq!(MANAGER_WIT_PACKAGE, "mcode:plugin@0.2.0");
    assert_eq!(MANAGER_WORLD, "manager");
    assert_eq!(MANAGER_WORLD_VERSION, "0.2.0");
    assert_eq!(MANAGER_WORLD_ID, "mcode:plugin/manager@0.2.0");
    assert_eq!(
        FEATURE_SERVICE_INTERFACE_ID,
        "mcode:plugin/feature-service@0.2.0"
    );
    assert_eq!(
        MANAGER_LIFECYCLE_INTERFACE_ID,
        "mcode:plugin/manager-lifecycle@0.2.0"
    );
    assert_eq!(MANAGER_WIT, include_str!("../goldens/manager_current.wit"));
}
