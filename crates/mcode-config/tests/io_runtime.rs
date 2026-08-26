// Rust guideline compliant 2026-08-26

mod common;

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use common::{layer, raw_layer, source};
use mcode_config::{
    AcceptAllConfig, ConfigErrorKind, ConfigLayer, ConfigLimits, ConfigRuntime, ConfigScope,
    ReloadCancellation, SourceTrust, write_config_file, write_config_file_with_limits,
};
use serde_json::json;
use tempfile::tempdir;

#[test]
fn optional_missing_file_is_absent_but_required_missing_file_fails() {
    let directory = tempdir().expect("temp directory");
    let missing = directory.path().join("missing.json");
    let defaults = layer(
        ConfigScope::CompiledDefaults,
        "defaults",
        json!({"default": true}),
    );
    let optional = ConfigLayer::optional_file(source(
        ConfigScope::Global,
        missing.to_str().expect("UTF-8 test path"),
        SourceTrust::Trusted,
    ));
    let inline_empty = raw_layer(
        ConfigScope::Session,
        "inline-empty",
        SourceTrust::Trusted,
        br#"{"formatVersion":1,"config":{}}"#,
    );
    let runtime = ConfigRuntime::load(
        &[defaults.clone(), optional, inline_empty],
        &AcceptAllConfig,
    )
    .expect("optional file");
    assert_eq!(runtime.snapshot().value(), &json!({"default": true}));

    let required = ConfigLayer::required_file(source(
        ConfigScope::Global,
        missing.to_str().expect("UTF-8 test path"),
        SourceTrust::Trusted,
    ));
    let error = ConfigRuntime::load(&[defaults, required], &AcceptAllConfig)
        .expect_err("required file must exist");
    assert_eq!(error.kind(), ConfigErrorKind::Io);
    assert_eq!(error.io_kind(), Some(std::io::ErrorKind::NotFound));
}

#[test]
fn partial_file_reload_rolls_back_the_old_snapshot() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("settings.json");
    write_config_file(&path, &json!({"revision": 1})).expect("initial atomic write");
    let file_layer = || {
        ConfigLayer::required_file(mcode_config::ConfigSource::new(
            ConfigScope::CompiledDefaults,
            path.clone(),
            SourceTrust::Trusted,
        ))
    };
    let runtime = ConfigRuntime::load(&[file_layer()], &AcceptAllConfig).expect("initial load");
    let before = runtime.snapshot();

    fs::write(&path, br#"{"formatVersion":1,"config":{"revision":2"#).expect("simulate torn write");
    let error = runtime
        .reload(
            &[file_layer()],
            &AcceptAllConfig,
            &ReloadCancellation::new(),
        )
        .expect_err("partial reload");
    assert_eq!(error.kind(), ConfigErrorKind::InvalidJson);
    assert!(Arc::ptr_eq(&before, &runtime.snapshot()));
}

#[test]
fn atomic_write_replaces_existing_file_and_roundtrips() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("settings.json");
    write_config_file(&path, &json!({"revision": 1, "items": [1, 2]})).expect("first write");
    write_config_file(&path, &json!({"revision": 2, "items": [3]})).expect("replacement write");

    let source = mcode_config::ConfigSource::new(
        ConfigScope::CompiledDefaults,
        path.clone(),
        SourceTrust::Trusted,
    );
    let runtime = ConfigRuntime::load(&[ConfigLayer::required_file(source)], &AcceptAllConfig)
        .expect("load atomically written file");
    assert_eq!(
        runtime.snapshot().value(),
        &json!({"revision": 2, "items": [3]})
    );

    let names: Vec<OsString> = fs::read_dir(directory.path())
        .expect("list temp directory")
        .map(|entry| entry.expect("directory entry").file_name())
        .collect();
    assert!(names.contains(&OsString::from("settings.json")));
    assert!(names.contains(&OsString::from("settings.json.lock")));
    assert!(
        names
            .iter()
            .all(|name| !name.to_string_lossy().ends_with(".tmp")),
        "temporary file leaked: {names:?}"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = fs::metadata(&path)
            .expect("settings metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
fn explicit_node_limits_count_the_complete_roundtrip_envelope() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("settings.json");
    let value = json!({"enabled": true});

    let payload_only_limit = ConfigLimits {
        max_nodes: 3,
        ..ConfigLimits::default()
    };
    let error = write_config_file_with_limits(&path, &value, payload_only_limit)
        .expect_err("envelope overhead must exceed a payload-only limit");
    assert_eq!(error.kind(), ConfigErrorKind::TooManyNodes);
    assert!(!path.exists());

    let exact_envelope_limit = ConfigLimits {
        max_nodes: 7,
        ..ConfigLimits::default()
    };
    write_config_file_with_limits(&path, &value, exact_envelope_limit)
        .expect("write at exact envelope node limit");
    let source =
        mcode_config::ConfigSource::new(ConfigScope::CompiledDefaults, path, SourceTrust::Trusted);
    let runtime = ConfigRuntime::load_with_options(
        &[ConfigLayer::required_file(source)],
        &AcceptAllConfig,
        exact_envelope_limit,
        &ReloadCancellation::new(),
    )
    .expect("read at the same exact envelope node limit");
    assert_eq!(runtime.snapshot().value(), &value);
}

#[test]
fn concurrent_atomic_writers_leave_one_complete_envelope() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("settings.json");
    let mut writers = Vec::new();
    for writer in 0_u64..12 {
        let path = path.clone();
        writers.push(thread::spawn(move || {
            write_config_file(&path, &json!({"writer": writer, "mirror": writer}))
                .expect("concurrent atomic write");
        }));
    }
    for writer in writers {
        writer.join().expect("writer thread");
    }

    let runtime = ConfigRuntime::load(
        &[ConfigLayer::required_file(mcode_config::ConfigSource::new(
            ConfigScope::CompiledDefaults,
            path,
            SourceTrust::Trusted,
        ))],
        &AcceptAllConfig,
    )
    .expect("final complete envelope");
    assert_eq!(
        runtime.snapshot().value()["writer"],
        runtime.snapshot().value()["mirror"]
    );
}

#[test]
fn failed_security_check_does_not_modify_existing_file() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("settings.json");
    write_config_file(&path, &json!({"safe": true})).expect("initial write");
    let before = fs::read(&path).expect("read initial bytes");

    let error =
        write_config_file(&path, &json!({"apiKey": "inline-forbidden"})).expect_err("unsafe write");
    assert_eq!(error.kind(), ConfigErrorKind::CredentialValue);
    assert_eq!(fs::read(&path).expect("read preserved bytes"), before);
}

#[test]
fn non_utf8_source_paths_are_safe_in_provenance_and_debug() {
    let path = non_utf8_path();
    let source =
        mcode_config::ConfigSource::new(ConfigScope::CompiledDefaults, path, SourceTrust::Trusted);
    let bytes = br#"{"formatVersion":1,"config":{"safe":true}}"#;
    let layer = ConfigLayer::inline(source, bytes);
    let rendered_layer = format!("{layer:?}");
    assert!(rendered_layer.contains("ConfigLayer"));

    let runtime = ConfigRuntime::load(&[layer], &AcceptAllConfig).expect("non-UTF-8 provenance");
    let snapshot = runtime.snapshot();
    let provenance = snapshot.source_at("/safe").expect("source provenance");
    let rendered_source = format!("{provenance:?}");
    assert!(rendered_source.contains("ConfigSource"));
}

#[cfg(unix)]
fn non_utf8_path() -> PathBuf {
    use std::os::unix::ffi::OsStringExt;

    PathBuf::from(OsString::from_vec(vec![b's', b'e', b't', 0xff]))
}

#[cfg(windows)]
fn non_utf8_path() -> PathBuf {
    use std::os::windows::ffi::OsStringExt;

    PathBuf::from(OsString::from_wide(&[b's' as u16, 0xd800, b't' as u16]))
}

#[cfg(not(any(unix, windows)))]
fn non_utf8_path() -> PathBuf {
    PathBuf::from("portable-logical-path")
}

#[cfg(windows)]
#[test]
fn windows_replace_file_semantics_replace_an_existing_destination() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("settings.json");
    write_config_file(&path, &json!({"windows": "old"})).expect("old destination");
    write_config_file(&path, &json!({"windows": "new"})).expect("ReplaceFileW destination");

    let runtime = ConfigRuntime::load(
        &[ConfigLayer::required_file(mcode_config::ConfigSource::new(
            ConfigScope::CompiledDefaults,
            path,
            SourceTrust::Trusted,
        ))],
        &AcceptAllConfig,
    )
    .expect("load replaced destination");
    assert_eq!(runtime.snapshot().value()["windows"], "new");
}
