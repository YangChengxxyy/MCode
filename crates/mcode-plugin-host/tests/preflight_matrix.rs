//! Complete cross-world component preflight acceptance matrix.

#[path = "support/preflight_fixtures.rs"]
mod fixtures;

use mcode_plugin_host::{ComponentLimits, ComponentWorld, PreflightError, preflight_component};

#[test]
fn all_four_diagonals_accept_and_all_cross_world_pairs_reject() {
    let components = fixtures::canonical_components();
    assert_eq!(components.len(), 4);

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

    assert_eq!(accepted, 4);
    assert_eq!(rejected, 12);
}

#[test]
fn binary_and_caller_size_boundaries_precede_world_compilation() {
    let provider = fixtures::canonical_component(ComponentWorld::Provider);
    let exact = ComponentLimits::new(provider.len()).expect("exact positive fixture limit");
    preflight_component(&provider, ComponentWorld::Provider, exact).expect("exact size boundary");

    let too_small = ComponentLimits::new(provider.len() - 1).expect("positive fixture limit");
    assert_eq!(
        preflight_component(&provider, ComponentWorld::Provider, too_small)
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
