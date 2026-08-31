//! Authoritative exact-12 Manager component loading contract tests.

use std::fmt::Write as _;
use std::fs;

use mcode_config::{
    ArtifactRef, AuthorityRevision, BundlePath, CanonicalVersion, HomeLayout, ManagerRecord,
    ManagerRegistry, PluginFamily, Sha256Digest, SourceBindingId, TrustHighWater, begin_staging,
    ensure_home_layout, replace_manager_registry,
};
use mcode_plugin_host::runtime::{PluginRuntime, RuntimeError};
use mcode_plugin_host::{ManagerLoadError, load_manager_candidates};
use sha2::{Digest, Sha256};

const VERSION: &str = "1.2.3";
const MISMATCHED_DIGEST: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

fn layout() -> (tempfile::TempDir, HomeLayout) {
    let parent = tempfile::tempdir().expect("temporary parent");
    let home = HomeLayout::from_root(parent.path().join("home")).expect("valid home");
    ensure_home_layout(&home).expect("secure test home");
    (parent, home)
}

fn current_manager_component() -> Vec<u8> {
    wat::parse_str(include_str!("fixtures/current_manager_component.wat"))
        .expect("generated current Manager component")
}

fn wrong_shape_manager_component() -> Vec<u8> {
    let source = include_str!("fixtures/current_manager_component.wat").replacen(
        "(param \"request\" string)",
        "(param \"crossed-request\" string)",
        1,
    );
    wat::parse_str(source).expect("wrong-world component fixture")
}

fn digest(bytes: &[u8]) -> Sha256Digest {
    let mut encoded = String::from("sha256:");
    for byte in Sha256::digest(bytes) {
        write!(encoded, "{byte:02x}").expect("write fixture digest");
    }
    Sha256Digest::parse(encoded).expect("canonical fixture digest")
}

fn artifact(digest: Sha256Digest) -> ArtifactRef {
    ArtifactRef::new(
        CanonicalVersion::parse(VERSION).expect("canonical version"),
        digest,
    )
}

fn installed(enabled: bool, digest: Sha256Digest) -> ManagerRecord {
    ManagerRecord::installed(
        enabled,
        SourceBindingId::parse("official-release").expect("source binding"),
        artifact(digest.clone()),
        TrustHighWater::new(1, digest).expect("trust high-water fixture"),
    )
}

fn publish_registry(home: &HomeLayout, records: &[(PluginFamily, ManagerRecord)]) {
    let mut registry = ManagerRegistry::empty();
    for (family, record) in records {
        registry.set_manager(*family, record.clone());
    }
    replace_manager_registry(home, AuthorityRevision::ABSENT, &registry)
        .expect("publish Manager registry");
}

fn write_components(home: &HomeLayout, artifacts: &[(PluginFamily, &[u8])]) {
    let mut transaction = begin_staging(home).expect("begin secure fixture staging");
    let payload = home
        .transaction_staging_dir(transaction.id())
        .join("payload");
    for (family, bytes) in artifacts {
        let path = BundlePath::parse(format!(
            "{}/manager/versions/{VERSION}/component.wasm",
            family.directory_name()
        ))
        .expect("canonical fixture path");
        transaction
            .write_file(&path, bytes)
            .expect("write secure component fixture");
    }
    drop(transaction);

    for (family, _) in artifacts {
        fs::rename(
            payload.join(family.directory_name()),
            home.plugin_dir(*family),
        )
        .expect("publish component fixture tree");
    }
}

#[test]
fn missing_registry_is_absent_even_when_an_artifact_tree_exists() {
    let (_parent, home) = layout();
    let bytes = current_manager_component();
    write_components(&home, &[(PluginFamily::Providers, &bytes)]);
    let runtime = PluginRuntime::new();

    let candidates = load_manager_candidates(&home, &runtime).expect("missing registry is valid");

    assert_eq!(candidates.revision(), AuthorityRevision::ABSENT);
    assert!(candidates.is_empty());
    assert_eq!(candidates.iter().count(), 0);
    for family in PluginFamily::ALL {
        assert!(candidates.get(family).is_none());
    }
    assert_eq!(
        runtime.new_owner().err(),
        Some(RuntimeError::RuntimeUninitialized)
    );
}

#[test]
fn disabled_installed_manager_does_not_require_its_missing_artifact() {
    let (_parent, home) = layout();
    let bytes = current_manager_component();
    publish_registry(
        &home,
        &[(PluginFamily::Web, installed(false, digest(&bytes)))],
    );

    let candidates = load_manager_candidates(&home, &PluginRuntime::new())
        .expect("disabled missing artifact is skipped");

    assert_eq!(candidates.revision().get(), 1);
    assert!(candidates.is_empty());
    assert!(candidates.get(PluginFamily::Web).is_none());
}

#[test]
fn enabled_manager_requires_the_selected_artifact() {
    let (_parent, home) = layout();
    let bytes = current_manager_component();
    publish_registry(
        &home,
        &[(PluginFamily::Session, installed(true, digest(&bytes)))],
    );

    let error = load_manager_candidates(&home, &PluginRuntime::new())
        .err()
        .expect("enabled artifact is missing");

    assert_eq!(
        error,
        ManagerLoadError::ComponentMissing(PluginFamily::Session)
    );
}

#[test]
fn exact_digest_mismatch_precedes_runtime_initialization() {
    let (_parent, home) = layout();
    let bytes = current_manager_component();
    publish_registry(
        &home,
        &[(
            PluginFamily::Compaction,
            installed(
                true,
                Sha256Digest::parse(MISMATCHED_DIGEST).expect("mismatched digest fixture"),
            ),
        )],
    );
    write_components(&home, &[(PluginFamily::Compaction, &bytes)]);
    let runtime = PluginRuntime::new();

    let error = load_manager_candidates(&home, &runtime)
        .err()
        .expect("digest must mismatch");

    assert_eq!(
        error,
        ManagerLoadError::DigestMismatch(PluginFamily::Compaction)
    );
    assert_eq!(
        runtime.new_owner().err(),
        Some(RuntimeError::RuntimeUninitialized)
    );
}

#[test]
fn later_digest_mismatch_precedes_all_runtime_initialization() {
    let (_parent, home) = layout();
    let bytes = current_manager_component();
    let valid_digest = digest(&bytes);
    publish_registry(
        &home,
        &[
            (
                PluginFamily::Providers,
                installed(true, valid_digest.clone()),
            ),
            (
                PluginFamily::Session,
                installed(
                    true,
                    Sha256Digest::parse(MISMATCHED_DIGEST).expect("mismatched digest fixture"),
                ),
            ),
        ],
    );
    write_components(
        &home,
        &[
            (PluginFamily::Providers, &bytes),
            (PluginFamily::Session, &bytes),
        ],
    );
    let runtime = PluginRuntime::new();

    let error = load_manager_candidates(&home, &runtime)
        .err()
        .expect("later digest must mismatch");

    assert_eq!(
        error,
        ManagerLoadError::DigestMismatch(PluginFamily::Session)
    );
    assert_eq!(
        runtime.new_owner().err(),
        Some(RuntimeError::RuntimeUninitialized)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn exact_valid_manager_loads_with_registry_identity() {
    let (_parent, home) = layout();
    let bytes = current_manager_component();
    let selected = artifact(digest(&bytes));
    publish_registry(
        &home,
        &[(
            PluginFamily::Resources,
            installed(true, selected.digest().clone()),
        )],
    );
    write_components(&home, &[(PluginFamily::Resources, &bytes)]);
    let runtime = PluginRuntime::new();

    let mut candidates = load_manager_candidates(&home, &runtime).expect("valid Manager load");

    assert_eq!(candidates.revision().get(), 1);
    assert_eq!(candidates.len(), 1);
    let candidate = candidates
        .get(PluginFamily::Resources)
        .expect("Resources candidate");
    assert_eq!(candidate.family(), PluginFamily::Resources);
    assert_eq!(candidate.artifact(), &selected);

    let component = candidates
        .take(PluginFamily::Resources)
        .expect("take Resources candidate")
        .into_component();
    assert!(candidates.is_empty());
    let mut owner = runtime.new_owner().expect("initialized runtime owner");
    owner
        .instantiate_manager(&component)
        .await
        .expect("instantiate loaded Manager");
}

#[test]
fn matching_digest_wrong_world_is_rejected_as_compilation() {
    let (_parent, home) = layout();
    let bytes = wrong_shape_manager_component();
    publish_registry(
        &home,
        &[(PluginFamily::Ask, installed(true, digest(&bytes)))],
    );
    write_components(&home, &[(PluginFamily::Ask, &bytes)]);

    let error = load_manager_candidates(&home, &PluginRuntime::new())
        .err()
        .expect("wrong Manager world must fail");

    assert_eq!(error, ManagerLoadError::Compilation(PluginFamily::Ask));
}

#[test]
fn later_compilation_failure_does_not_publish_runtime_readiness() {
    let (_parent, home) = layout();
    let valid_bytes = current_manager_component();
    let wrong_bytes = wrong_shape_manager_component();
    publish_registry(
        &home,
        &[
            (
                PluginFamily::Providers,
                installed(true, digest(&valid_bytes)),
            ),
            (PluginFamily::Session, installed(true, digest(&wrong_bytes))),
        ],
    );
    write_components(
        &home,
        &[
            (PluginFamily::Providers, &valid_bytes),
            (PluginFamily::Session, &wrong_bytes),
        ],
    );
    let runtime = PluginRuntime::new();

    let result = load_manager_candidates(&home, &runtime);

    assert_eq!(
        result.err(),
        Some(ManagerLoadError::Compilation(PluginFamily::Session))
    );
    assert_eq!(
        runtime.new_owner().err(),
        Some(RuntimeError::RuntimeUninitialized)
    );
}

#[test]
fn all_twelve_enabled_slots_load_without_extra_candidates() {
    let (_parent, home) = layout();
    let bytes = current_manager_component();
    let expected_digest = digest(&bytes);
    let records =
        PluginFamily::ALL.map(|family| (family, installed(true, expected_digest.clone())));
    let artifacts = PluginFamily::ALL.map(|family| (family, bytes.as_slice()));
    publish_registry(&home, &records);
    write_components(&home, &artifacts);

    let candidates = load_manager_candidates(&home, &PluginRuntime::new())
        .expect("all exact Manager slots load");

    assert_eq!(candidates.len(), 12);
    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate.family())
            .collect::<Vec<_>>(),
        PluginFamily::ALL
    );
    for family in PluginFamily::ALL {
        assert_eq!(
            candidates.get(family).map(|candidate| candidate.family()),
            Some(family)
        );
    }
}

#[test]
fn later_failure_returns_no_partial_candidate_set() {
    let (_parent, home) = layout();
    let bytes = current_manager_component();
    let expected_digest = digest(&bytes);
    publish_registry(
        &home,
        &[
            (
                PluginFamily::Providers,
                installed(true, expected_digest.clone()),
            ),
            (
                PluginFamily::Session,
                installed(true, expected_digest.clone()),
            ),
        ],
    );
    write_components(&home, &[(PluginFamily::Providers, &bytes)]);

    let result = load_manager_candidates(&home, &PluginRuntime::new());

    assert_eq!(
        result.err(),
        Some(ManagerLoadError::ComponentMissing(PluginFamily::Session))
    );
}
