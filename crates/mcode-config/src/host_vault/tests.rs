//! Tests strict private Host-vault mechanics.

// Rust guideline compliant 2026-08-29

use std::fs;
use std::sync::{Arc, Barrier};
use std::thread;

use tempfile::TempDir;

use super::model::{parse_vault, serialize_for_test};
use super::{
    EMPTY_VAULT_BYTES, HostVaultState, VaultRevision, initialize_empty_host_vault,
    read_host_vault_state, relative_path,
};
use crate::secure_fs::owned_file::replace_owned_file;
use crate::{ConfigErrorKind, HomeLayout, OwnedKind, ensure_home_layout, probe_access_control};

const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SECRET_SENTINEL: &str = "c2VudGluZWwtc2VjcmV0";
const MEMBER_SENTINEL: &str = "sentinel.account";

fn layout(temp: &TempDir) -> HomeLayout {
    HomeLayout::from_root(temp.path().join("home")).expect("layout")
}

fn credential(service: &str, account: &str, state: &str, secret: &str) -> String {
    format!(
        "{{\"serviceId\":\"{service}\",\"accountId\":\"{account}\",\"issuerId\":\"issuer\",\"authSchemaId\":\"schema\",\"credentialVersion\":1,\"state\":\"{state}\",\"secretBase64Url\":{secret}}}"
    )
}

fn grant(
    family: &str,
    manager: &str,
    pack: &str,
    operation: &str,
    service: &str,
    account: &str,
) -> String {
    format!(
        "{{\"consumerFamily\":\"{family}\",\"managerId\":\"{manager}\",\"packId\":\"{pack}\",\"operationId\":\"{operation}\",\"serviceId\":\"{service}\",\"accountId\":\"{account}\",\"authorityDigest\":\"{DIGEST}\"}}"
    )
}

fn document(revision: u64, credentials: &str, grants: &str) -> String {
    format!(
        "{{\"formatVersion\":1,\"kind\":\"mcode-host-auth\",\"revision\":{revision},\"credentials\":[{credentials}],\"grants\":[{grants}]}}\n"
    )
}

fn valid_document() -> String {
    let credential = credential(
        "alpha",
        MEMBER_SENTINEL,
        "active",
        &format!("\"{SECRET_SENTINEL}\""),
    );
    let grant = grant(
        "providers",
        "com.mcode.providers",
        "default-pack",
        "read",
        "alpha",
        MEMBER_SENTINEL,
    );
    document(7, &credential, &grant)
}

fn assert_invalid(bytes: impl AsRef<[u8]>) {
    assert!(parse_vault(bytes.as_ref()).is_err());
}

#[test]
fn empty_initialization_is_exact_revision_zero() {
    let temp = TempDir::new().expect("temp");
    let layout = layout(&temp);
    ensure_home_layout(&layout).expect("bootstrap");

    assert_eq!(
        initialize_empty_host_vault(&layout).expect("initialize"),
        VaultRevision::EMPTY
    );
    assert_eq!(
        fs::read(layout.host_auth_json()).expect("vault"),
        EMPTY_VAULT_BYTES
    );
    assert_eq!(
        read_host_vault_state(&layout).expect("state"),
        HostVaultState::Present {
            revision: VaultRevision::EMPTY
        }
    );
}

#[test]
fn absent_read_creates_nothing() {
    let temp = TempDir::new().expect("temp");
    let layout = layout(&temp);
    assert_eq!(
        read_host_vault_state(&layout).expect("state"),
        HostVaultState::Absent
    );
    assert!(!layout.root().exists());
}

#[test]
fn initializer_creates_only_lazy_host_artifacts_after_bootstrap() {
    let temp = TempDir::new().expect("temp");
    let layout = layout(&temp);
    ensure_home_layout(&layout).expect("bootstrap");
    initialize_empty_host_vault(&layout).expect("initialize");

    let mut root_entries = fs::read_dir(layout.root())
        .expect("root entries")
        .map(|entry| entry.expect("entry").file_name())
        .collect::<Vec<_>>();
    root_entries.sort();
    assert_eq!(root_entries, ["plugins"]);
    let mut plugin_entries = fs::read_dir(layout.plugins_dir())
        .expect("plugin entries")
        .map(|entry| entry.expect("entry").file_name())
        .collect::<Vec<_>>();
    plugin_entries.sort();
    assert_eq!(plugin_entries, [".host"]);
    let names = fs::read_dir(layout.host_dir())
        .expect("host entries")
        .map(|entry| entry.expect("entry").file_name())
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"auth.json".into()));
    assert!(names.contains(&"auth.json.lock".into()));
}

#[test]
fn concurrent_initialization_has_one_winner() {
    let temp = TempDir::new().expect("temp");
    let layout = Arc::new(layout(&temp));
    ensure_home_layout(&layout).expect("bootstrap");
    let barrier = Arc::new(Barrier::new(8));
    let handles = (0..8)
        .map(|_| {
            let layout = Arc::clone(&layout);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                initialize_empty_host_vault(&layout)
            })
        })
        .collect::<Vec<_>>();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().expect("thread"))
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
fn valid_nonempty_document_reports_status_only() {
    let temp = TempDir::new().expect("temp");
    let layout = layout(&temp);
    replace_owned_file(&layout, relative_path(), valid_document().as_bytes())
        .expect("write fixture");
    assert_eq!(
        read_host_vault_state(&layout).expect("state"),
        HostVaultState::Present {
            revision: VaultRevision::new(7).expect("revision")
        }
    );

    let valid = valid_document();
    let parsed = parse_vault(valid.as_bytes()).expect("parse");
    let serialized = serialize_for_test(&parsed).expect("serialize");
    assert_eq!(serialized.as_slice(), valid.as_bytes());
}

#[test]
fn schema_shape_and_scalar_bounds_are_strict() {
    let valid = valid_document();
    for invalid in [
        valid.replacen("\"formatVersion\":1,", "", 1),
        valid.replacen("\"kind\":", "\"extra\":0,\"kind\":", 1),
        valid.replacen("\"kind\":\"mcode-host-auth\"", "\"kind\":7", 1),
        valid.replacen("\"formatVersion\":1", "\"formatVersion\":2", 1),
        valid.replacen("\"revision\":7", "\"revision\":9223372036854775808", 1),
        valid.replacen("\"credentialVersion\":1", "\"credentialVersion\":0", 1),
        valid.replacen("\"credentialVersion\":1", "\"credentialVersion\":9223372036854775808", 1),
        format!("{} trailing", valid.trim_end()),
        "{\"formatVersion\":1,\"formatVersion\":1,\"kind\":\"mcode-host-auth\",\"revision\":0,\"credentials\":[],\"grants\":[]}".into(),
    ] {
        assert_invalid(invalid);
    }
    assert_invalid([0xff_u8]);
}

#[test]
fn local_ids_and_descriptor_fields_are_validated() {
    for bad in ["", "A", "a..b", "a._b", "a-", "a/b", &"a".repeat(129)] {
        let invalid = valid_document().replacen(
            "\"serviceId\":\"alpha\"",
            &format!("\"serviceId\":\"{bad}\""),
            1,
        );
        assert_invalid(invalid);
    }
    for field in ["issuerId", "authSchemaId", "accountId", "operationId"] {
        assert_invalid(valid_document().replacen(
            &format!(
                "\"{field}\":\"{}\"",
                if field == "accountId" {
                    MEMBER_SENTINEL
                } else if field == "operationId" {
                    "read"
                } else if field == "issuerId" {
                    "issuer"
                } else {
                    "schema"
                }
            ),
            &format!("\"{field}\":\"BAD\""),
            1,
        ));
    }
}

#[test]
fn credential_state_secret_and_account_order_are_strict() {
    let active_null = credential("alpha", "one", "active", "null");
    let revoked_secret = credential("alpha", "one", "revoked", "\"YQ\"");
    let unknown = credential("alpha", "one", "paused", "null");
    assert_invalid(document(0, &active_null, ""));
    assert_invalid(document(0, &revoked_secret, ""));
    assert_invalid(document(0, &unknown, ""));

    let first = credential("beta", "one", "revoked", "null");
    let second = credential("alpha", "one", "revoked", "null");
    assert_invalid(document(0, &format!("{first},{second}"), ""));
    assert_invalid(document(0, &format!("{first},{first}"), ""));
}

#[test]
fn credential_count_bounds_are_enforced() {
    let active = (0..65)
        .map(|index| credential("alpha", &format!("a{index:03}"), "active", "\"YQ\""))
        .collect::<Vec<_>>()
        .join(",");
    assert_invalid(document(0, &active, ""));
    let revoked = (0..257)
        .map(|index| credential("alpha", &format!("r{index:03}"), "revoked", "null"))
        .collect::<Vec<_>>()
        .join(",");
    assert_invalid(document(0, &revoked, ""));
}

#[test]
fn grants_validate_consumer_identity_pack_digest_order_and_references() {
    let active_credential = credential("alpha", "one", "active", "\"YQ\"");
    let valid = grant(
        "providers",
        "com.mcode.providers",
        "pack",
        "read",
        "alpha",
        "one",
    );
    for invalid in [
        valid.replacen("providers", "session", 1),
        valid.replacen("com.mcode.providers", "com.mcode.web", 1),
        valid.replacen("\"pack\"", "\"BAD\"", 1),
        valid.replacen(DIGEST, "sha256:ABC", 1),
        valid.replacen("\"accountId\":\"one\"", "\"accountId\":\"missing\"", 1),
    ] {
        assert_invalid(document(0, &active_credential, &invalid));
    }
    let later = grant("web", "com.mcode.web", "pack", "read", "alpha", "one");
    assert_invalid(document(0, &active_credential, &format!("{later},{valid}")));
    assert_invalid(document(0, &active_credential, &format!("{valid},{valid}")));

    let revoked = credential("alpha", "one", "revoked", "null");
    assert_invalid(document(0, &revoked, &valid));
}

#[test]
fn grant_count_bound_is_enforced() {
    let credential = credential("alpha", "one", "active", "\"YQ\"");
    let grants = (0..1025)
        .map(|index| {
            grant(
                "providers",
                "com.mcode.providers",
                "pack",
                &format!("op{index:04}"),
                "alpha",
                "one",
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    assert_invalid(document(0, &credential, &grants));
}

#[test]
fn base64url_is_bounded_unpadded_and_canonical() {
    for secret in ["", "=", "YQ=", "YQ==", "+w", "/w", "YR", "***"] {
        let credential = credential("alpha", "one", "active", &format!("\"{secret}\""));
        assert_invalid(document(0, &credential, ""));
    }
    let oversized = "YQ".repeat(5_462);
    assert_invalid(document(
        0,
        &credential("alpha", "one", "active", &format!("\"{oversized}\"")),
        "",
    ));
}

#[test]
fn every_json_escape_is_rejected_before_deserialization() {
    assert_invalid(valid_document().replacen("alpha", "al\\u0070ha", 1));
    assert_invalid(valid_document().replacen(SECRET_SENTINEL, "c2VudGluZWwt\\u0063b2VjcmV0", 1));
}

#[test]
fn existing_valid_malformed_and_excessive_revision_are_preserved() {
    let fixtures = [
        valid_document(),
        "{malformed SENTINEL_SECRET}".into(),
        document(i64::MAX as u64, "", ""),
        document(9_223_372_036_854_775_808, "", ""),
    ];
    for fixture in fixtures {
        let temp = TempDir::new().expect("temp");
        let layout = layout(&temp);
        replace_owned_file(&layout, relative_path(), fixture.as_bytes()).expect("fixture");
        assert!(initialize_empty_host_vault(&layout).is_err());
        assert_eq!(
            fs::read(layout.host_auth_json()).expect("preserved"),
            fixture.as_bytes()
        );
    }
}

#[test]
fn oversized_and_wrong_type_targets_fail_closed() {
    let temp = TempDir::new().expect("temp");
    let layout = layout(&temp);
    ensure_home_layout(&layout).expect("bootstrap");
    fs::create_dir(layout.host_dir()).expect("host");
    fs::create_dir(layout.host_auth_json()).expect("wrong type");
    assert!(matches!(
        read_host_vault_state(&layout)
            .expect_err("wrong type")
            .kind(),
        ConfigErrorKind::Io | ConfigErrorKind::AccessControl
    ));

    fs::remove_dir(layout.host_auth_json()).expect("remove wrong type");
    initialize_empty_host_vault(&layout).expect("initialize");
    fs::write(
        layout.host_auth_json(),
        vec![b'x'; super::MAX_HOST_VAULT_BYTES + 1],
    )
    .expect("oversized fixture");
    assert_eq!(
        read_host_vault_state(&layout)
            .expect_err("oversized")
            .kind(),
        ConfigErrorKind::Oversized
    );
}

#[test]
fn errors_never_render_member_or_secret_values() {
    let temp = TempDir::new().expect("temp");
    let layout = layout(&temp);
    let malformed =
        valid_document().replacen("\"state\":\"active\"", "\"state\":\"SENTINEL_STATE\"", 1);
    replace_owned_file(&layout, relative_path(), malformed.as_bytes()).expect("fixture");
    let error = read_host_vault_state(&layout).expect_err("invalid");
    for rendered in [format!("{error}"), format!("{error:?}")] {
        assert!(!rendered.contains(SECRET_SENTINEL));
        assert!(!rendered.contains(MEMBER_SENTINEL));
        assert!(!rendered.contains("SENTINEL_STATE"));
    }
    assert_eq!(error.path(), Some(layout.host_auth_json().as_path()));
}

#[test]
fn target_and_lock_are_private_owned_files() {
    let temp = TempDir::new().expect("temp");
    let layout = layout(&temp);
    ensure_home_layout(&layout).expect("bootstrap");
    initialize_empty_host_vault(&layout).expect("initialize");
    let target = probe_access_control(&layout.host_auth_json());
    let lock = probe_access_control(&layout.host_dir().join("auth.json.lock"));
    assert!(matches!(
        target,
        crate::AccessControlEvidence::UnixMode {
            kind: OwnedKind::File,
            mode: 0o600
        } | crate::AccessControlEvidence::WindowsProtectedDacl {
            kind: OwnedKind::File,
            owner_allowed: true,
            current_user: true,
            system: true,
            protected: true,
            ace_count: 2,
            extra_aces: 0,
            ..
        }
    ));
    assert!(matches!(
        lock,
        crate::AccessControlEvidence::UnixMode {
            kind: OwnedKind::File,
            mode: 0o600
        } | crate::AccessControlEvidence::WindowsProtectedDacl {
            kind: OwnedKind::File,
            owner_allowed: true,
            current_user: true,
            system: true,
            protected: true,
            ace_count: 2,
            extra_aces: 0,
            ..
        }
    ));
}

#[cfg(unix)]
#[test]
fn host_links_fail_closed() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().expect("temp");
    let layout = layout(&temp);
    ensure_home_layout(&layout).expect("bootstrap");
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).expect("outside");
    symlink(&outside, layout.host_dir()).expect("host symlink");
    assert_eq!(
        initialize_empty_host_vault(&layout)
            .expect_err("link")
            .kind(),
        ConfigErrorKind::LinkEscape
    );

    fs::remove_file(layout.host_dir()).expect("remove link");
    fs::create_dir(layout.host_dir()).expect("host");
    symlink(temp.path().join("target"), layout.host_auth_json()).expect("target symlink");
    assert_eq!(
        read_host_vault_state(&layout).expect_err("link").kind(),
        ConfigErrorKind::LinkEscape
    );
}

#[cfg(windows)]
#[test]
fn host_reparse_points_fail_closed() {
    let temp = TempDir::new().expect("temp");
    let layout = layout(&temp);
    ensure_home_layout(&layout).expect("bootstrap");
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).expect("outside");
    junction::create(&outside, layout.host_dir()).expect("host junction");
    assert_eq!(
        read_host_vault_state(&layout).expect_err("reparse").kind(),
        ConfigErrorKind::LinkEscape
    );
    assert!(initialize_empty_host_vault(&layout).is_err());
    junction::delete(layout.host_dir()).expect("delete junction data");
    fs::remove_dir(layout.host_dir()).expect("remove junction");

    fs::create_dir(layout.host_dir()).expect("host");
    junction::create(&outside, layout.host_auth_json()).expect("target junction");
    assert!(matches!(
        read_host_vault_state(&layout).expect_err("reparse").kind(),
        ConfigErrorKind::LinkEscape | ConfigErrorKind::AccessControl
    ));
}

#[cfg(unix)]
#[test]
fn permissive_target_and_lock_fail_closed() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().expect("temp");
    let layout = layout(&temp);
    ensure_home_layout(&layout).expect("bootstrap");
    initialize_empty_host_vault(&layout).expect("initialize");
    fs::set_permissions(layout.host_auth_json(), fs::Permissions::from_mode(0o644))
        .expect("target mode");
    assert_eq!(
        read_host_vault_state(&layout).expect_err("access").kind(),
        ConfigErrorKind::AccessControl
    );
    fs::set_permissions(layout.host_auth_json(), fs::Permissions::from_mode(0o600))
        .expect("restore target");
    fs::set_permissions(
        layout.host_dir().join("auth.json.lock"),
        fs::Permissions::from_mode(0o644),
    )
    .expect("lock mode");
    assert_eq!(
        initialize_empty_host_vault(&layout)
            .expect_err("access")
            .kind(),
        ConfigErrorKind::AccessControl
    );
}

#[test]
fn production_source_avoids_unzeroized_json_shortcuts() {
    let host = include_str!("../host_vault.rs");
    let model = include_str!("model.rs");
    for source in [host, model] {
        assert!(!source.contains("serde_json::Value"));
        assert!(!source.contains("serde_json::to_vec"));
        assert!(!source.contains(".to_string()"));
    }
}
