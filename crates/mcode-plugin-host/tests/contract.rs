//! Manifest, registry, and WIT contract tests.

mod common;

use mcode_plugin_api::{
    CapabilityGrants, CapabilityKind, PluginId, PluginSource, Provenance, TrustLevel,
};
use mcode_plugin_host::{
    HostBindings, HostError, PluginRegistration, PluginRegistry, RegistryChange, RegistryError,
    compile_component, load_wasm_bytes, new_engine,
};
use serde_json::json;

use common::{
    base_manifest_json, construct_error_wat, construct_nonempty_wat, export_only_wat,
    parse_manifest, tight_limits, wasi_import_wat,
};

#[test]
fn registry_is_transactional_and_fail_closed_on_collisions() {
    let root_a = tempfile::tempdir().expect("tempdir");
    let root_b = tempfile::tempdir().expect("tempdir");
    let manifest_a = parse_manifest(root_a.path(), "plugin.wasm", &[]);
    let mut value = base_manifest_json("plugin.wasm");
    value["id"] = json!("com.mcode.other");
    value["name"] = json!("Other");
    let manifest_b = mcode_plugin_api::PluginManifest::parse_json(
        &serde_json::to_vec(&value).expect("json"),
        root_b.path(),
    )
    .expect("manifest b");

    let registry = PluginRegistry::new();
    let plugin_a =
        PluginRegistration::new(manifest_a, provenance_for("com.mcode.fixture", "1.0.0"))
            .expect("reg a");
    let mut tx = registry
        .prepare([RegistryChange::Register(plugin_a)], HostBindings::empty())
        .expect("prepare");
    tx.validate().expect("validate");
    let snapshot = tx.commit().expect("commit");
    assert_eq!(snapshot.generation(), 1);
    assert!(snapshot.tool("fixture_tool").is_some());

    let plugin_b = PluginRegistration::new(
        manifest_b,
        Provenance::new(
            PluginId::parse("com.mcode.other").expect("id"),
            "1.0.0",
            PluginSource::Bundled {
                bundle: "other".into(),
            },
            TrustLevel::BuiltIn,
        )
        .expect("prov"),
    )
    .expect("reg b");
    let mut collision = registry
        .prepare([RegistryChange::Register(plugin_b)], HostBindings::empty())
        .expect("prepare collision");
    assert!(matches!(
        collision.validate(),
        Err(RegistryError::Collision { .. })
    ));
    assert_eq!(registry.snapshot().generation(), 1);
}

#[test]
fn untrusted_project_plugins_cannot_publish() {
    let root = tempfile::tempdir().expect("tempdir");
    let manifest = parse_manifest(root.path(), "plugin.wasm", &[]);
    let provenance = Provenance::new(
        manifest.id().clone(),
        manifest.version(),
        PluginSource::Project {
            root: root.path().to_path_buf(),
        },
        TrustLevel::Untrusted,
    )
    .expect("prov");
    let registration = PluginRegistration::new(manifest, provenance).expect("reg");
    let registry = PluginRegistry::new();
    let mut tx = registry
        .prepare(
            [RegistryChange::Register(registration)],
            HostBindings::empty(),
        )
        .expect("prepare");
    assert!(matches!(
        tx.validate(),
        Err(RegistryError::UntrustedPlugin(_))
    ));
}

#[test]
fn export_only_component_compiles_and_wasi_component_is_identifiable() {
    let engine = new_engine().expect("engine");
    compile_component(&engine, export_only_wat()).expect("ok component");
    compile_component(&engine, wasi_import_wat()).expect("wasi component parses");
}

fn provenance_for(id: &str, version: &str) -> Provenance {
    Provenance::new(
        PluginId::parse(id).expect("id"),
        version,
        PluginSource::Bundled { bundle: id.into() },
        TrustLevel::BuiltIn,
    )
    .expect("provenance")
}

#[test]
fn grants_are_not_used_to_unlock_wasi() {
    let root = tempfile::tempdir().expect("tempdir");
    let manifest = parse_manifest(root.path(), "plugin.wasm", &[]);
    let mut grants = CapabilityGrants::none();
    grants.allow(CapabilityKind::Filesystem);
    grants.allow(CapabilityKind::Network);
    grants.allow(CapabilityKind::Secrets);
    grants.allow(CapabilityKind::Ui);
    let error = load_wasm_bytes(
        &manifest,
        wasi_import_wat().as_bytes(),
        &grants,
        1,
        tight_limits(),
    )
    .expect_err("wasi denied even with grants");
    assert!(matches!(
        error,
        HostError::ForbiddenImport | HostError::ImportMismatch | HostError::InvalidComponent
    ));
}

#[test]
fn construct_rejects_error_envelope_and_nonempty_json() {
    let root = tempfile::tempdir().expect("tempdir");
    let manifest = parse_manifest(root.path(), "plugin.wasm", &[]);
    let error = load_wasm_bytes(
        &manifest,
        construct_error_wat().as_bytes(),
        &CapabilityGrants::none(),
        1,
        tight_limits(),
    )
    .expect_err("construct error");
    assert!(matches!(error, HostError::Guest { .. }));

    let nonempty = load_wasm_bytes(
        &manifest,
        construct_nonempty_wat().as_bytes(),
        &CapabilityGrants::none(),
        1,
        tight_limits(),
    )
    .expect_err("construct nonempty");
    assert_eq!(nonempty, HostError::InvalidGuestOutput);
}

#[test]
fn strict_manifest_rejects_non_wasm_runtime_and_entry_fields() {
    let root = tempfile::tempdir().expect("tempdir");
    let mut runtime = common::base_manifest_json("plugin.wasm");
    runtime["runtime"] = serde_json::json!("firstParty");
    assert!(matches!(
        mcode_plugin_api::PluginManifest::parse_json(
            &serde_json::to_vec(&runtime).expect("json"),
            root.path()
        ),
        Err(mcode_plugin_api::ManifestError::UnknownField { .. })
    ));

    let mut entry = common::base_manifest_json("plugin.wasm");
    entry["entry"] = serde_json::json!("plugin.wasm");
    assert!(matches!(
        mcode_plugin_api::PluginManifest::parse_json(
            &serde_json::to_vec(&entry).expect("json"),
            root.path()
        ),
        Err(mcode_plugin_api::ManifestError::UnknownField { .. })
    ));

    let native = common::base_manifest_json("plugin.dll");
    assert_eq!(
        mcode_plugin_api::PluginManifest::parse_json(
            &serde_json::to_vec(&native).expect("json"),
            root.path()
        ),
        Err(mcode_plugin_api::ManifestError::NativeDynamicLibraryForbidden)
    );
}
