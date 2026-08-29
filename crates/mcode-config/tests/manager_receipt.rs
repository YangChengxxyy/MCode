use std::fs;
use std::sync::{Arc, Barrier};
use std::thread;

use mcode_config::{
    AccessControlEvidence, ArtifactRef, AuthorityRevision, CanonicalVersion, ConfigErrorKind,
    HomeLayout, OwnedKind, PluginFamily, Sha256Digest, probe_access_control, read_manager_receipt,
    replace_manager_receipt,
};
use serde_json::{Value, json};

const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const OTHER_DIGEST: &str =
    "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";

fn layout() -> (tempfile::TempDir, HomeLayout) {
    let parent = tempfile::tempdir().expect("temporary parent");
    let home = HomeLayout::from_root(parent.path().join("home")).expect("valid layout");
    (parent, home)
}

fn revision(value: u64) -> AuthorityRevision {
    AuthorityRevision::new(value).expect("valid revision")
}

fn artifact(version: &str, digest: &str) -> ArtifactRef {
    ArtifactRef::new(
        CanonicalVersion::parse(version).expect("canonical version"),
        Sha256Digest::parse(digest).expect("digest"),
    )
}

fn create_receipt(home: &HomeLayout, family: PluginFamily) -> Value {
    replace_manager_receipt(
        home,
        family,
        AuthorityRevision::ABSENT,
        &artifact("1.2.3-alpha.1+build.7", DIGEST),
    )
    .expect("initial receipt");
    serde_json::from_slice(
        &fs::read(home.manager_installation_json(family)).expect("receipt bytes"),
    )
    .expect("receipt JSON")
}

fn write_value(home: &HomeLayout, family: PluginFamily, value: &Value) {
    fs::write(
        home.manager_installation_json(family),
        serde_json::to_vec(value).expect("fixture JSON"),
    )
    .expect("write fixture");
}

fn assert_invalid(home: &HomeLayout, family: PluginFamily, value: &Value) {
    write_value(home, family, value);
    let error = read_manager_receipt(home, family).expect_err("receipt must fail");
    assert_eq!(error.kind(), ConfigErrorKind::AuthorityValidation);
    assert_eq!(
        error.path(),
        Some(home.manager_installation_json(family).as_path())
    );
}

#[test]
fn all_family_paths_round_trip_canonically() {
    let (_parent, home) = layout();
    let active = artifact("1.2.3-alpha.1+build.7", DIGEST);
    for family in PluginFamily::ALL {
        let document = replace_manager_receipt(&home, family, AuthorityRevision::ABSENT, &active)
            .expect("write family receipt");
        assert_eq!(document.revision().get(), 1);
        assert_eq!(document.family(), family);
        assert_eq!(document.active(), &active);

        let bytes = fs::read(home.manager_installation_json(family)).expect("receipt");
        let expected = format!(
            "{{\"formatVersion\":1,\"kind\":\"mcode-manager-installation-receipt\",\"revision\":1,\"family\":\"{}\",\"active\":{{\"version\":\"1.2.3-alpha.1+build.7\",\"digest\":\"{DIGEST}\"}}}}\n",
            family.directory_name()
        );
        assert_eq!(bytes, expected.as_bytes());
        assert_eq!(
            read_manager_receipt(&home, family)
                .expect("read")
                .expect("present"),
            document
        );
    }
}

fn assert_private_file(path: &std::path::Path) {
    #[cfg(unix)]
    assert_eq!(
        probe_access_control(path),
        AccessControlEvidence::UnixMode {
            kind: OwnedKind::File,
            mode: 0o600,
        }
    );
    #[cfg(windows)]
    assert!(matches!(
        probe_access_control(path),
        AccessControlEvidence::WindowsProtectedDacl {
            kind: OwnedKind::File,
            owner_allowed: true,
            owner_current_user: true,
            current_user: true,
            system: true,
            protected: true,
            ace_count: 1 | 2,
            extra_aces: 0,
            ..
        }
    ));
}

#[test]
fn missing_read_creates_nothing() {
    let (_parent, home) = layout();
    assert_eq!(
        read_manager_receipt(&home, PluginFamily::Providers).expect("missing read"),
        None
    );
    assert!(!home.root().exists());
}

#[test]
fn mutation_creates_only_requested_ancestors_target_and_lock() {
    let (_parent, home) = layout();
    replace_manager_receipt(
        &home,
        PluginFamily::Web,
        AuthorityRevision::ABSENT,
        &artifact("1.0.0", DIGEST),
    )
    .expect("write receipt");

    let target = home.manager_installation_json(PluginFamily::Web);
    let lock = home
        .manager_dir(PluginFamily::Web)
        .join("installation.json.lock");
    assert!(target.is_file());
    assert!(lock.is_file());
    assert_private_file(&target);
    assert_private_file(&lock);
    assert!(!home.plugins_json().exists());
    for family in PluginFamily::ALL {
        if family != PluginFamily::Web {
            assert!(!home.plugin_dir(family).exists());
        }
    }
}

#[test]
fn family_must_match_the_path_and_identity_is_derived() {
    let (_parent, home) = layout();
    let mut value = create_receipt(&home, PluginFamily::Providers);
    value["family"] = json!("ui");
    assert_invalid(&home, PluginFamily::Providers, &value);

    value["family"] = json!("com.mcode.providers");
    assert_invalid(&home, PluginFamily::Providers, &value);
}

#[test]
fn envelope_and_active_fields_are_flat_exact_and_typed() {
    let (_parent, home) = layout();
    let base = create_receipt(&home, PluginFamily::Providers);
    let mut cases = Vec::new();
    for field in ["formatVersion", "kind", "revision", "family", "active"] {
        let mut value = base.clone();
        value.as_object_mut().expect("root").remove(field);
        cases.push(value);
    }
    let mut extra = base.clone();
    extra["extra"] = Value::Null;
    cases.push(extra);
    for (field, wrong) in [
        ("formatVersion", json!(2)),
        ("formatVersion", json!("1")),
        ("kind", json!("receipt")),
        ("kind", json!(1)),
        ("revision", json!(0)),
        ("revision", json!(-1)),
        ("revision", json!(1.0)),
        ("family", json!(1)),
        ("active", json!([])),
    ] {
        let mut value = base.clone();
        value[field] = wrong;
        cases.push(value);
    }
    for field in ["version", "digest"] {
        let mut value = base.clone();
        value["active"]
            .as_object_mut()
            .expect("active")
            .remove(field);
        cases.push(value);
    }
    let mut active_extra = base.clone();
    active_extra["active"]["extra"] = Value::Null;
    cases.push(active_extra);
    for (field, wrong) in [("version", json!(1)), ("digest", json!(1))] {
        let mut value = base.clone();
        value["active"][field] = wrong;
        cases.push(value);
    }
    for value in cases {
        assert_invalid(&home, PluginFamily::Providers, &value);
    }

    let raw = serde_json::to_string(&base).expect("base JSON");
    fs::write(
        home.manager_installation_json(PluginFamily::Providers),
        raw.replacen("{", "{\"kind\":\"duplicate\",", 1),
    )
    .expect("duplicate root");
    assert_eq!(
        read_manager_receipt(&home, PluginFamily::Providers)
            .expect_err("duplicate root")
            .kind(),
        ConfigErrorKind::DuplicateKey
    );

    let active = serde_json::to_string(&base["active"]).expect("active JSON");
    let duplicate = active.replacen("{", "{\"version\":\"9.9.9\",", 1);
    fs::write(
        home.manager_installation_json(PluginFamily::Providers),
        raw.replacen(&active, &duplicate, 1),
    )
    .expect("duplicate active");
    assert_eq!(
        read_manager_receipt(&home, PluginFamily::Providers)
            .expect_err("duplicate active")
            .kind(),
        ConfigErrorKind::DuplicateKey
    );
}

#[test]
fn canonical_version_and_digest_grammar_is_reused() {
    let (_parent, home) = layout();
    let base = create_receipt(&home, PluginFamily::Providers);
    for invalid in ["01.2.3", "v1.2.3", "1.2.3 "] {
        let mut value = base.clone();
        value["active"]["version"] = json!(invalid);
        assert_invalid(&home, PluginFamily::Providers, &value);
    }
    for invalid in [
        "sha256:0123",
        "SHA256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    ] {
        let mut value = base.clone();
        value["active"]["digest"] = json!(invalid);
        assert_invalid(&home, PluginFamily::Providers, &value);
    }
}

#[test]
fn revisions_advance_and_stale_cas_preserves_target() {
    let (_parent, home) = layout();
    let active = artifact("1.0.0", DIGEST);
    let first = replace_manager_receipt(
        &home,
        PluginFamily::Providers,
        AuthorityRevision::ABSENT,
        &active,
    )
    .expect("revision one");
    let second = replace_manager_receipt(
        &home,
        PluginFamily::Providers,
        first.revision(),
        &artifact("2.0.0", OTHER_DIGEST),
    )
    .expect("revision two");
    assert_eq!(second.revision().get(), 2);
    let before = fs::read(home.manager_installation_json(PluginFamily::Providers)).expect("before");
    let error = replace_manager_receipt(&home, PluginFamily::Providers, revision(1), &active)
        .expect_err("stale CAS");
    assert_eq!(error.kind(), ConfigErrorKind::RevisionConflict);
    assert_eq!(
        fs::read(home.manager_installation_json(PluginFamily::Providers)).expect("after"),
        before
    );
}

#[test]
fn concurrent_same_revision_has_exactly_one_success() {
    let (_parent, home) = layout();
    replace_manager_receipt(
        &home,
        PluginFamily::Providers,
        AuthorityRevision::ABSENT,
        &artifact("1.0.0", DIGEST),
    )
    .expect("revision one");
    let home = Arc::new(home);
    let barrier = Arc::new(Barrier::new(9));
    let handles = (0..8)
        .map(|index| {
            let home = Arc::clone(&home);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                let version = format!("2.0.{index}");
                let active = artifact(&version, OTHER_DIGEST);
                barrier.wait();
                replace_manager_receipt(&home, PluginFamily::Providers, revision(1), &active)
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().expect("worker"))
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| result
                .as_ref()
                .is_err_and(|error| error.kind() == ConfigErrorKind::RevisionConflict))
            .count(),
        7
    );
}

#[test]
fn exhausted_and_malformed_current_receipts_preserve_target() {
    let (_parent, home) = layout();
    let mut value = create_receipt(&home, PluginFamily::Providers);
    value["revision"] = json!(i64::MAX);
    write_value(&home, PluginFamily::Providers, &value);
    let target = home.manager_installation_json(PluginFamily::Providers);
    let before = fs::read(&target).expect("exhausted bytes");
    let error = replace_manager_receipt(
        &home,
        PluginFamily::Providers,
        revision(i64::MAX as u64),
        &artifact("2.0.0", OTHER_DIGEST),
    )
    .expect_err("exhausted");
    assert_eq!(error.kind(), ConfigErrorKind::RevisionExhausted);
    assert_eq!(fs::read(&target).expect("preserved"), before);

    fs::write(&target, b"not JSON").expect("malformed fixture");
    let before = fs::read(&target).expect("malformed bytes");
    let error = replace_manager_receipt(
        &home,
        PluginFamily::Providers,
        revision(1),
        &artifact("2.0.0", OTHER_DIGEST),
    )
    .expect_err("malformed");
    assert_eq!(error.kind(), ConfigErrorKind::InvalidJson);
    assert_eq!(fs::read(&target).expect("preserved"), before);
}

#[test]
fn bounded_parser_rejects_oversized_deep_node_heavy_and_non_utf8() {
    let (_parent, home) = layout();
    create_receipt(&home, PluginFamily::Providers);
    let target = home.manager_installation_json(PluginFamily::Providers);
    for (bytes, expected) in [
        (vec![b' '; 16 * 1024 + 1], ConfigErrorKind::Oversized),
        (
            format!("{{\"x\":{}{} }}", "[".repeat(8), "]".repeat(8)).into_bytes(),
            ConfigErrorKind::TooDeep,
        ),
        (
            format!(
                "[{}]",
                std::iter::repeat_n("null", 40)
                    .collect::<Vec<_>>()
                    .join(",")
            )
            .into_bytes(),
            ConfigErrorKind::TooManyNodes,
        ),
        (vec![b'{', 0xff, b'}'], ConfigErrorKind::NonUtf8),
    ] {
        fs::write(&target, bytes).expect("bounded fixture");
        assert_eq!(
            read_manager_receipt(&home, PluginFamily::Providers)
                .expect_err("bounded rejection")
                .kind(),
            expected
        );
    }
}

#[test]
fn parser_errors_retain_only_path_and_kind() {
    let (_parent, home) = layout();
    create_receipt(&home, PluginFamily::Providers);
    let sentinel = "MEMBER-NAME-MUST-NOT-APPEAR-8f392e";
    fs::write(
        home.manager_installation_json(PluginFamily::Providers),
        format!(r#"{{"{sentinel}":null,"{sentinel}":null}}"#),
    )
    .expect("duplicate fixture");
    let error = read_manager_receipt(&home, PluginFamily::Providers).expect_err("duplicate");
    assert_eq!(error.kind(), ConfigErrorKind::DuplicateKey);
    assert_eq!(
        error.path(),
        Some(
            home.manager_installation_json(PluginFamily::Providers)
                .as_path()
        )
    );
    assert!(!format!("{error:?}").contains(sentinel));
    assert!(!error.to_string().contains(sentinel));
}

#[test]
fn wrong_target_type_is_rejected() {
    let (_parent, home) = layout();
    fs::create_dir_all(home.manager_installation_json(PluginFamily::Providers))
        .expect("wrong target directory");
    let error = read_manager_receipt(&home, PluginFamily::Providers).expect_err("wrong type");
    assert!(matches!(
        error.kind(),
        ConfigErrorKind::Io | ConfigErrorKind::AccessControl
    ));
}

#[cfg(windows)]
#[test]
fn final_reparse_point_is_rejected_without_touching_its_target() {
    let (parent, home) = layout();
    let outside = parent.path().join("outside");
    fs::create_dir(&outside).expect("outside directory");
    fs::write(outside.join("marker"), b"outside").expect("outside marker");
    replace_manager_receipt(
        &home,
        PluginFamily::Providers,
        AuthorityRevision::ABSENT,
        &artifact("1.0.0", DIGEST),
    )
    .expect("secure manager ancestors");
    fs::remove_file(home.manager_installation_json(PluginFamily::Providers))
        .expect("remove receipt target");
    junction::create(
        &outside,
        home.manager_installation_json(PluginFamily::Providers),
    )
    .expect("receipt reparse fixture");

    let error = read_manager_receipt(&home, PluginFamily::Providers).expect_err("reparse point");
    assert_eq!(error.kind(), ConfigErrorKind::LinkEscape);
    assert_eq!(
        fs::read(outside.join("marker")).expect("outside bytes"),
        b"outside"
    );
}

#[cfg(unix)]
#[test]
fn final_symlink_is_rejected_without_touching_its_target() {
    use std::os::unix::fs::symlink;

    let (parent, home) = layout();
    let target = parent.path().join("outside.json");
    fs::write(&target, b"outside").expect("outside target");
    replace_manager_receipt(
        &home,
        PluginFamily::Providers,
        AuthorityRevision::ABSENT,
        &artifact("1.0.0", DIGEST),
    )
    .expect("secure manager ancestors");
    fs::remove_file(home.manager_installation_json(PluginFamily::Providers))
        .expect("remove receipt target");
    symlink(
        &target,
        home.manager_installation_json(PluginFamily::Providers),
    )
    .expect("receipt symlink");

    let error = read_manager_receipt(&home, PluginFamily::Providers).expect_err("symlink");
    assert_eq!(error.kind(), ConfigErrorKind::LinkEscape);
    assert_eq!(fs::read(&target).expect("outside bytes"), b"outside");
}
