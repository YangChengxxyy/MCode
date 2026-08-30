//! Complete cross-world component preflight acceptance matrix.

#[path = "support/preflight_fixtures.rs"]
mod fixtures;

use mcode_plugin_host::{ComponentLimits, ComponentWorld, PreflightError, preflight_component};

#[test]
fn all_thirteen_diagonals_accept_and_all_cross_world_pairs_reject() {
    let components = fixtures::canonical_components();
    assert_eq!(components.len(), 13);

    let mut accepted = 0;
    let mut rejected = 0;
    for (candidate_world, bytes) in &components {
        for validator_world in ComponentWorld::ALL {
            let result = preflight_component(bytes, validator_world, ComponentLimits::default());
            if *candidate_world == validator_world {
                result.unwrap_or_else(|error| {
                    panic!("{candidate_world:?} diagonal must accept: {error:?}")
                });
                accepted += 1;
            } else {
                assert!(
                    result.is_err(),
                    "{candidate_world:?} must not pass {validator_world:?} validation"
                );
                rejected += 1;
            }
        }
    }

    assert_eq!(accepted, 13);
    assert_eq!(rejected, 156);
}

#[test]
fn binary_and_caller_size_boundaries_precede_world_compilation() {
    let manager = fixtures::canonical_component(ComponentWorld::Manager);
    let exact = ComponentLimits::new(manager.len()).expect("exact positive fixture limit");
    preflight_component(&manager, ComponentWorld::Manager, exact).expect("exact size boundary");

    let too_small = ComponentLimits::new(manager.len() - 1).expect("positive fixture limit");
    assert_eq!(
        preflight_component(&manager, ComponentWorld::Manager, too_small)
            .expect_err("one byte over caller limit"),
        PreflightError::ComponentTooLarge,
    );
    assert_eq!(
        ComponentLimits::new(0).expect_err("zero limit"),
        PreflightError::InvalidLimits,
    );
    assert_eq!(
        ComponentLimits::new(mcode_plugin_host::MAX_COMPONENT_BYTES + 1)
            .expect_err("limit over hard maximum"),
        PreflightError::InvalidLimits,
    );
}
