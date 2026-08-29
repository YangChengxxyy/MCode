use std::fs;
use std::sync::{Arc, Barrier};
use std::thread;

use mcode_config::{
    ArtifactRef, AuthorityRevision, CanonicalVersion, ConfigErrorKind, HomeLayout, ManagerRecord,
    ManagerRegistry, PluginFamily, Sha256Digest, SourceBindingId, TrustHighWater,
    read_manager_registry, replace_manager_registry,
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

fn installed(enabled: bool) -> ManagerRecord {
    ManagerRecord::installed(
        enabled,
        SourceBindingId::parse("official-release").expect("source"),
        ArtifactRef::new(
            CanonicalVersion::parse("1.2.3-alpha.1+build.7").expect("version"),
            Sha256Digest::parse(DIGEST).expect("digest"),
        ),
        TrustHighWater::new(
            7,
            Sha256Digest::parse(OTHER_DIGEST).expect("manifest digest"),
        )
        .expect("high-water"),
    )
}

fn create_document(home: &HomeLayout) -> Value {
    replace_manager_registry(home, AuthorityRevision::ABSENT, &ManagerRegistry::empty())
        .expect("initial registry");
    serde_json::from_slice(&fs::read(home.plugins_json()).expect("registry bytes"))
        .expect("registry JSON")
}

fn write_value(home: &HomeLayout, value: &Value) {
    fs::write(
        home.plugins_json(),
        serde_json::to_vec(value).expect("serialize malformed fixture"),
    )
    .expect("write malformed fixture");
}

fn assert_authority_invalid(home: &HomeLayout, value: &Value) {
    write_value(home, value);
    let error = read_manager_registry(home).expect_err("authority must fail");
    assert_eq!(error.kind(), ConfigErrorKind::AuthorityValidation);
    assert_eq!(error.path(), Some(home.plugins_json().as_path()));
}

#[test]
fn exact_twelve_round_trip_is_deterministic_and_newline_terminated() {
    let (_parent, home) = layout();
    let mut registry = ManagerRegistry::empty();
    registry.set_manager(PluginFamily::Providers, installed(true));
    registry.set_manager(PluginFamily::Ui, installed(false));

    let written =
        replace_manager_registry(&home, AuthorityRevision::ABSENT, &registry).expect("first write");
    assert_eq!(written.revision().get(), 1);
    assert_eq!(written.registry(), &registry);

    let bytes = fs::read(home.plugins_json()).expect("persisted registry");
    assert_eq!(bytes.last(), Some(&b'\n'));
    let text = std::str::from_utf8(&bytes).expect("UTF-8 registry");
    assert!(text.starts_with(
        "{\"formatVersion\":1,\"kind\":\"mcode-manager-registry\",\"revision\":1,\"managers\":{\"providers\":"
    ));
    let mut previous = 0;
    for family in PluginFamily::ALL {
        let marker = format!("\"{}\":", family.directory_name());
        let position = text.find(&marker).expect("family serialized");
        assert!(position >= previous, "stable family order");
        previous = position;
    }

    let read = read_manager_registry(&home)
        .expect("read succeeds")
        .expect("document exists");
    assert_eq!(read, written);
    assert_eq!(
        read.registry().manager(PluginFamily::Providers),
        &installed(true)
    );
}

#[test]
fn missing_read_creates_nothing() {
    let (_parent, home) = layout();
    assert_eq!(read_manager_registry(&home).expect("missing read"), None);
    assert!(!home.root().exists());
}

#[test]
fn revisions_advance_and_stale_cas_preserves_bytes() {
    let (_parent, home) = layout();
    let first =
        replace_manager_registry(&home, AuthorityRevision::ABSENT, &ManagerRegistry::empty())
            .expect("revision one");
    let second = replace_manager_registry(&home, first.revision(), &ManagerRegistry::empty())
        .expect("revision two");
    assert_eq!(second.revision().get(), 2);
    let before = fs::read(home.plugins_json()).expect("before stale write");

    let error = replace_manager_registry(&home, revision(1), &ManagerRegistry::empty())
        .expect_err("stale CAS");
    assert_eq!(error.kind(), ConfigErrorKind::RevisionConflict);
    assert_eq!(
        fs::read(home.plugins_json()).expect("after stale write"),
        before
    );
}

#[test]
fn concurrent_same_revision_has_exactly_one_success() {
    let (_parent, home) = layout();
    replace_manager_registry(&home, AuthorityRevision::ABSENT, &ManagerRegistry::empty())
        .expect("revision one");
    let home = Arc::new(home);
    let barrier = Arc::new(Barrier::new(9));
    let handles = (0..8)
        .map(|index| {
            let home = Arc::clone(&home);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                let mut registry = ManagerRegistry::empty();
                registry.set_manager(PluginFamily::Providers, installed(index % 2 == 0));
                barrier.wait();
                replace_manager_registry(&home, revision(1), &registry)
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();

    let results = handles
        .into_iter()
        .map(|handle| handle.join().expect("worker did not panic"))
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
    assert_eq!(
        read_manager_registry(&home)
            .expect("final read")
            .expect("final document")
            .revision()
            .get(),
        2
    );
}

#[test]
fn malformed_current_document_blocks_replacement() {
    let (_parent, home) = layout();
    let mut value = create_document(&home);
    value["managers"]["providers"]["enabled"] = Value::Bool(true);
    write_value(&home, &value);
    let before = fs::read(home.plugins_json()).expect("malformed bytes");

    let error = replace_manager_registry(&home, revision(1), &ManagerRegistry::empty())
        .expect_err("invalid current authority");
    assert_eq!(error.kind(), ConfigErrorKind::AuthorityValidation);
    assert_eq!(
        fs::read(home.plugins_json()).expect("unchanged bytes"),
        before
    );
}

#[test]
fn envelope_fields_are_exact_and_typed() {
    let (_parent, home) = layout();
    let base = create_document(&home);
    let mut cases = Vec::new();
    for field in ["formatVersion", "kind", "revision", "managers"] {
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
        ("kind", json!("mcode-pack-registry")),
        ("kind", json!(1)),
        ("revision", json!(0)),
        ("revision", json!(-1)),
        ("revision", json!(1.0)),
        ("revision", json!("1")),
        ("managers", json!([])),
    ] {
        let mut value = base.clone();
        value[field] = wrong;
        cases.push(value);
    }
    for value in cases {
        assert_authority_invalid(&home, &value);
    }

    let raw = fs::read_to_string(home.plugins_json()).expect("fixture");
    let duplicate = raw.replacen("{", "{\"kind\":\"mcode-manager-registry\",", 1);
    fs::write(home.plugins_json(), duplicate).expect("duplicate envelope");
    assert_eq!(
        read_manager_registry(&home)
            .expect_err("duplicate envelope")
            .kind(),
        ConfigErrorKind::DuplicateKey
    );
}

#[test]
fn family_set_rejects_eleven_thirteen_unknown_and_impostors() {
    let (_parent, home) = layout();
    let base = create_document(&home);
    let impostors = [
        "unknown",
        "pack-id",
        ".host",
        ".staging",
        "com.mcode.providers",
    ];

    let mut eleven = base.clone();
    eleven["managers"]
        .as_object_mut()
        .expect("managers")
        .remove("ui");
    assert_authority_invalid(&home, &eleven);

    let mut thirteen = base.clone();
    thirteen["managers"]["unknown"] = base["managers"]["providers"].clone();
    assert_authority_invalid(&home, &thirteen);

    for impostor in impostors {
        let mut value = base.clone();
        let providers = value["managers"]
            .as_object_mut()
            .expect("managers")
            .remove("providers")
            .expect("providers");
        value["managers"][impostor] = providers;
        assert_authority_invalid(&home, &value);
    }
}

#[test]
fn manager_and_nested_records_reject_missing_extra_duplicate_and_wrong_types() {
    let (_parent, home) = layout();
    let base = create_document(&home);
    let record = &base["managers"]["providers"];
    let mut cases = Vec::new();
    for field in ["enabled", "source", "active", "trustHighWater"] {
        let mut value = base.clone();
        value["managers"]["providers"]
            .as_object_mut()
            .expect("record")
            .remove(field);
        cases.push(value);
    }
    let mut extra = base.clone();
    extra["managers"]["providers"]["extra"] = Value::Null;
    cases.push(extra);
    for (field, wrong) in [
        ("enabled", json!(0)),
        ("source", json!([])),
        ("active", json!([])),
        ("trustHighWater", json!([])),
    ] {
        let mut value = base.clone();
        value["managers"]["providers"][field] = wrong;
        cases.push(value);
    }
    for value in cases {
        assert_authority_invalid(&home, &value);
    }

    let encoded = serde_json::to_string(record).expect("record JSON");
    let duplicate = encoded.replacen("{", "{\"enabled\":false,", 1);
    let raw = serde_json::to_string(&base)
        .expect("base JSON")
        .replacen(&encoded, &duplicate, 1);
    fs::write(home.plugins_json(), raw).expect("duplicate record");
    assert_eq!(
        read_manager_registry(&home)
            .expect_err("duplicate record")
            .kind(),
        ConfigErrorKind::DuplicateKey
    );
}

#[test]
fn every_malformed_nullable_state_is_rejected() {
    let (_parent, home) = layout();
    let base = create_document(&home);
    let source = json!("official-release");
    let active = json!({"version":"1.2.3","digest":DIGEST});
    let trust = json!({"sequence":1,"manifestDigest":OTHER_DIGEST});

    for enabled in [false, true] {
        for mask in 0_u8..8 {
            let valid_absent = !enabled && mask == 0;
            let valid_installed = mask == 7;
            if valid_absent || valid_installed {
                continue;
            }
            let mut value = base.clone();
            value["managers"]["providers"] = json!({
                "enabled": enabled,
                "source": if mask & 1 == 0 { Value::Null } else { source.clone() },
                "active": if mask & 2 == 0 { Value::Null } else { active.clone() },
                "trustHighWater": if mask & 4 == 0 { Value::Null } else { trust.clone() }
            });
            assert_authority_invalid(&home, &value);
        }
    }
}

#[test]
fn source_version_digest_and_high_water_are_strict() {
    for invalid in [
        "",
        "A",
        "a_1",
        "a--b",
        "a-",
        "1a",
        "é",
        "con",
        "prn",
        "aux",
        "nul",
        "com1",
        "lpt9",
        "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklm",
    ] {
        assert_eq!(
            SourceBindingId::parse(invalid)
                .expect_err("invalid source")
                .kind(),
            ConfigErrorKind::AuthorityValidation
        );
    }
    for invalid in [
        "1", "01.2.3", "1.02.3", "1.2.03", "v1.2.3", "1.2.3-01", "1.2.3 ",
    ] {
        assert_eq!(
            CanonicalVersion::parse(invalid)
                .expect_err("noncanonical version")
                .kind(),
            ConfigErrorKind::AuthorityValidation
        );
    }
    assert_eq!(
        CanonicalVersion::parse(format!("1.2.3+{}", "a".repeat(129)))
            .expect_err("oversized version")
            .kind(),
        ConfigErrorKind::AuthorityValidation
    );
    for invalid in [
        "",
        "sha256:",
        "SHA256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "sha256:0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef",
        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde",
    ] {
        assert_eq!(
            Sha256Digest::parse(invalid)
                .expect_err("invalid digest")
                .kind(),
            ConfigErrorKind::AuthorityValidation
        );
    }
    assert_eq!(
        TrustHighWater::new(0, Sha256Digest::parse(DIGEST).expect("digest"))
            .expect_err("zero sequence")
            .kind(),
        ConfigErrorKind::AuthorityValidation
    );
    assert_eq!(
        TrustHighWater::new(
            i64::MAX as u64 + 1,
            Sha256Digest::parse(DIGEST).expect("digest"),
        )
        .expect_err("oversized sequence")
        .kind(),
        ConfigErrorKind::AuthorityValidation
    );
    assert_eq!(AuthorityRevision::ABSENT.get(), 0);
    assert_eq!(
        AuthorityRevision::new(i64::MAX as u64 + 1)
            .expect_err("oversized revision")
            .kind(),
        ConfigErrorKind::AuthorityValidation
    );
}

#[test]
fn nested_active_and_high_water_fields_are_exact() {
    let (_parent, home) = layout();
    let mut base = create_document(&home);
    base["managers"]["providers"] = json!({
        "enabled":false,
        "source":"official-release",
        "active":{"version":"1.2.3","digest":DIGEST},
        "trustHighWater":{"sequence":1,"manifestDigest":OTHER_DIGEST}
    });
    for path in ["active", "trustHighWater"] {
        let fields = if path == "active" {
            ["version", "digest"]
        } else {
            ["sequence", "manifestDigest"]
        };
        for field in fields {
            let mut value = base.clone();
            value["managers"]["providers"][path]
                .as_object_mut()
                .expect("nested object")
                .remove(field);
            assert_authority_invalid(&home, &value);
        }
        let mut value = base.clone();
        value["managers"]["providers"][path]["extra"] = Value::Null;
        assert_authority_invalid(&home, &value);
    }

    for (path, field, wrong) in [
        ("active", "version", json!(1)),
        ("active", "digest", json!(1)),
        ("trustHighWater", "sequence", json!(0)),
        ("trustHighWater", "sequence", json!(1.0)),
        ("trustHighWater", "manifestDigest", json!(1)),
    ] {
        let mut value = base.clone();
        value["managers"]["providers"][path][field] = wrong;
        assert_authority_invalid(&home, &value);
    }
}

#[test]
fn bounded_parser_rejects_oversized_deep_node_heavy_and_non_utf8_documents() {
    let (_parent, home) = layout();
    create_document(&home);

    fs::write(home.plugins_json(), vec![b' '; 64 * 1024 + 1]).expect("oversized fixture");
    assert_eq!(
        read_manager_registry(&home).expect_err("oversized").kind(),
        ConfigErrorKind::Oversized
    );

    let deep = format!(
        "{{\"formatVersion\":1,\"kind\":\"mcode-manager-registry\",\"revision\":1,\"managers\":{}{} }}",
        "[".repeat(10),
        "]".repeat(10)
    );
    fs::write(home.plugins_json(), deep).expect("deep fixture");
    assert_eq!(
        read_manager_registry(&home).expect_err("too deep").kind(),
        ConfigErrorKind::TooDeep
    );

    let nodes = format!(
        "{{\"formatVersion\":1,\"kind\":\"mcode-manager-registry\",\"revision\":1,\"managers\":[{}]}}",
        std::iter::repeat_n("null", 600)
            .collect::<Vec<_>>()
            .join(",")
    );
    fs::write(home.plugins_json(), nodes).expect("node-heavy fixture");
    assert_eq!(
        read_manager_registry(&home)
            .expect_err("too many nodes")
            .kind(),
        ConfigErrorKind::TooManyNodes
    );

    fs::write(home.plugins_json(), [b'{', 0xff, b'}']).expect("non-UTF-8 fixture");
    assert_eq!(
        read_manager_registry(&home).expect_err("non-UTF-8").kind(),
        ConfigErrorKind::NonUtf8
    );
}

#[test]
fn parser_errors_do_not_retain_untrusted_member_names() {
    let (_parent, home) = layout();
    create_document(&home);
    let sentinel = "MEMBER-NAME-MUST-NOT-APPEAR-8f392e";
    let duplicate = format!(r#"{{"{sentinel}":null,"{sentinel}":null}}"#);
    fs::write(home.plugins_json(), duplicate).expect("duplicate member fixture");

    let error = read_manager_registry(&home).expect_err("duplicate member");

    assert_eq!(error.kind(), ConfigErrorKind::DuplicateKey);
    assert_eq!(error.path(), Some(home.plugins_json().as_path()));
    assert!(error.pointer().is_none());
    assert!(!format!("{error:?}").contains(sentinel));
    assert!(!error.to_string().contains(sentinel));
}

#[test]
fn revision_exhaustion_and_errors_are_stable_and_redacted() {
    let (_parent, home) = layout();
    let mut value = create_document(&home);
    value["revision"] = json!(i64::MAX);
    write_value(&home, &value);
    let before = fs::read(home.plugins_json()).expect("exhausted bytes");
    let error =
        replace_manager_registry(&home, revision(i64::MAX as u64), &ManagerRegistry::empty())
            .expect_err("revision exhausted");
    assert_eq!(error.kind(), ConfigErrorKind::RevisionExhausted);
    assert_eq!(
        error.to_string().lines().next(),
        Some("owned authority revision is exhausted")
    );
    assert_eq!(
        fs::read(home.plugins_json()).expect("unchanged target"),
        before
    );

    let conflict = replace_manager_registry(&home, revision(42), &ManagerRegistry::empty())
        .expect_err("revision conflict");
    assert_eq!(
        conflict.to_string().lines().next(),
        Some("owned authority revision conflict")
    );
    let debug = format!("{conflict:?}");
    assert!(!debug.contains("42"));
    assert!(!debug.contains(&i64::MAX.to_string()));

    let validation = SourceBindingId::parse("secret value").expect_err("invalid source");
    assert_eq!(
        validation.to_string().lines().next(),
        Some("owned authority document is invalid")
    );
    assert!(!format!("{validation:?}").contains("secret value"));
}

#[test]
fn wrong_target_type_is_rejected_by_owned_file_substrate() {
    let (_parent, home) = layout();
    fs::create_dir_all(home.plugins_json()).expect("wrong target directory");
    let error = read_manager_registry(&home).expect_err("wrong target type");
    assert!(matches!(
        error.kind(),
        ConfigErrorKind::Io | ConfigErrorKind::AccessControl
    ));
    assert!(home.plugins_json().is_dir());
}
