// Rust guideline compliant 2026-08-31.

use std::sync::Arc;

use mcode_config::{
    AuthorityRevision, PluginFamily, RootComposition, UiSelection, replace_root_composition,
};

use super::super::test_support::{
    artifact, assert_published, candidates, configured_pack_component, current, revision,
};
use crate::ComponentWorld;
use crate::PackConfigurationError;
use crate::pack_loading::tests::{
    digest, layout, pack_component, pack_id, publish_installation, write_component,
};
use crate::runtime::{LifecycleState, PluginRuntime};

#[tokio::test(flavor = "current_thread")]
async fn public_pack_configuration_and_activation_reach_the_current_manager_imports() {
    let (_parent, home) = layout();
    let alpha = pack_id("pack-alpha");
    let beta = pack_id("pack-beta");
    let crossed = pack_id("pack-crossed");
    let pack_bytes = pack_component(ComponentWorld::Provider);
    let component_digest = digest(&pack_bytes);
    for pack_id in [&alpha, &beta] {
        write_component(&home, PluginFamily::Providers, pack_id, &pack_bytes);
        publish_installation(
            &home,
            PluginFamily::Providers,
            pack_id,
            Some(component_digest.clone()),
        );
    }
    let crossed_bytes = pack_component(ComponentWorld::Session);
    write_component(&home, PluginFamily::Providers, &crossed, &crossed_bytes);
    publish_installation(
        &home,
        PluginFamily::Providers,
        &crossed,
        Some(digest(&crossed_bytes)),
    );
    let first = RootComposition::new(
        None,
        vec![alpha.clone(), beta.clone()],
        Vec::new(),
        UiSelection::empty(),
    )
    .expect("first Providers configuration");
    let first = replace_root_composition(&home, AuthorityRevision::ABSENT, &first)
        .expect("publish first root configuration");
    let stale_first = first.clone();
    let second = RootComposition::new(
        None,
        vec![beta.clone(), alpha],
        Vec::new(),
        UiSelection::empty(),
    )
    .expect("second Providers configuration");
    let second = replace_root_composition(&home, first.revision(), &second)
        .expect("publish second root configuration");
    let third = RootComposition::new(None, vec![crossed, beta], Vec::new(), UiSelection::empty())
        .expect("failing Providers configuration");
    let third = replace_root_composition(&home, second.revision(), &third)
        .expect("publish failing root configuration");

    let runtime = Arc::new(PluginRuntime::new());
    let director = super::ManagerGenerationDirector::new(Arc::clone(&runtime), home.clone())
        .expect("claim configured-Pack test director");
    director
        .publish_pack_configuration(Some(first))
        .await
        .expect("publish first Pack configuration");
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![(
                    PluginFamily::Providers,
                    artifact("1.0.0", '1'),
                    configured_pack_component(),
                )],
            ))
            .await
            .expect("configured-Pack Providers publication"),
        1,
    );
    let expected = current(&director, PluginFamily::Providers)
        .expect("current configured-Pack Providers Manager");

    let first_poll = director
        .poll_current(&expected)
        .await
        .expect("first configured-packs call");
    assert_eq!(first_poll.outcome(), Ok(LifecycleState::Pending));

    director
        .publish_pack_configuration(Some(second))
        .await
        .expect("publish second Pack configuration");
    let second_poll = director
        .poll_current(&expected)
        .await
        .expect("second configured-packs call");
    assert_eq!(second_poll.outcome(), Ok(LifecycleState::Pending));

    director
        .publish_pack_configuration(Some(third))
        .await
        .expect("publish failing Pack configuration");
    let third_poll = director
        .poll_current(&expected)
        .await
        .expect("failing activate-packs call");
    assert_eq!(third_poll.outcome(), Ok(LifecycleState::Ready));
    assert_eq!(
        director.publish_pack_configuration(Some(stale_first)).await,
        Err(PackConfigurationError::RevisionRegression)
    );
}
