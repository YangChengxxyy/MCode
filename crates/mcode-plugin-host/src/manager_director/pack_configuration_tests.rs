// Rust guideline compliant 2026-08-31.

use std::sync::Arc;

use mcode_config::{
    AuthorityRevision, HomeLayout, PackId, PluginFamily, RootComposition, UiSelection,
    replace_root_composition,
};

use super::super::test_support::{
    artifact, assert_published, candidates, configured_pack_component, current, director, revision,
};
use crate::PackConfigurationError;
use crate::runtime::{LifecycleState, PluginRuntime};

#[tokio::test(flavor = "current_thread")]
async fn public_pack_configuration_reaches_the_current_manager_import() {
    let parent = tempfile::tempdir().expect("temporary parent");
    let home = HomeLayout::from_root(parent.path().join("home")).expect("valid home");
    let alpha = PackId::parse("pack-alpha").expect("alpha Pack ID");
    let beta = PackId::parse("pack-beta").expect("beta Pack ID");
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
    let second = RootComposition::new(None, vec![beta, alpha], Vec::new(), UiSelection::empty())
        .expect("second Providers configuration");
    let second = replace_root_composition(&home, first.revision(), &second)
        .expect("publish second root configuration");

    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
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
    assert_eq!(second_poll.outcome(), Ok(LifecycleState::Ready));
    assert_eq!(
        director.publish_pack_configuration(Some(stale_first)).await,
        Err(PackConfigurationError::RevisionRegression)
    );
}
