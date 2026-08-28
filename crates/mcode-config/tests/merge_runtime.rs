// Rust guideline compliant 2026-08-26

mod common;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use common::{layer, raw_layer};
use mcode_config::{
    AcceptAllConfig, ConfigErrorKind, ConfigRuntime, ConfigScope, ReloadCancellation, SourceTrust,
    ValidationFailure,
};
use serde_json::{Value, json};

#[test]
fn fixed_precedence_merges_objects_and_replaces_arrays() {
    let sources = vec![
        layer(
            ConfigScope::Explicit,
            "explicit",
            json!({"winner": "explicit", "nested": {"explicit": true}, "items": [5]}),
        ),
        layer(
            ConfigScope::Project,
            "project",
            json!({"winner": "project", "nested": {"project": true}, "items": [3]}),
        ),
        layer(
            ConfigScope::Global,
            "global",
            json!({"winner": "global", "nested": {"global": true}, "items": [1, 2]}),
        ),
        layer(
            ConfigScope::CompiledDefaults,
            "defaults",
            json!({
                "winner": "defaults",
                "nested": {"defaults": true},
                "items": [0]
            }),
        ),
    ];

    let runtime = ConfigRuntime::load(&sources, &AcceptAllConfig).expect("load layers");
    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.value()["winner"], "explicit");
    assert_eq!(snapshot.value()["items"], json!([5]));
    assert_eq!(
        snapshot.value()["nested"],
        json!({
            "defaults": true,
            "global": true,
            "project": true,
            "explicit": true
        })
    );
}

#[test]
fn nested_null_deletes_members_without_deleting_parent() {
    let sources = vec![
        layer(
            ConfigScope::CompiledDefaults,
            "defaults",
            json!({"outer": {"keep": 1, "remove": {"deep": true}}, "other": 2}),
        ),
        layer(
            ConfigScope::Project,
            "project",
            json!({"outer": {"remove": null}}),
        ),
    ];

    let runtime = ConfigRuntime::load(&sources, &AcceptAllConfig).expect("load delete patch");
    assert_eq!(
        runtime.snapshot().value(),
        &json!({"outer": {"keep": 1}, "other": 2})
    );
    assert!(runtime.snapshot().source_at("/outer/remove").is_none());
}

#[test]
fn provenance_escapes_pointers_and_covers_every_final_value() {
    let sources = vec![
        layer(
            ConfigScope::CompiledDefaults,
            "defaults",
            json!({"a/b": {"~tilde": "default"}, "array": [{"x": 1}]}),
        ),
        layer(
            ConfigScope::Project,
            "project",
            json!({"a/b": {"~tilde": "project"}, "array": [{"x": 2}]}),
        ),
    ];
    let runtime = ConfigRuntime::load(&sources, &AcceptAllConfig).expect("load provenance");
    let snapshot = runtime.snapshot();

    assert_eq!(
        snapshot
            .source_at("/a~1b/~0tilde")
            .expect("escaped leaf provenance")
            .scope,
        ConfigScope::Project
    );
    for pointer in ["/array", "/array/0", "/array/0/x"] {
        assert_eq!(
            snapshot.source_at(pointer).expect("array provenance").scope,
            ConfigScope::Project,
            "pointer {pointer}"
        );
    }
    assert_eq!(
        snapshot.source_at("").expect("root provenance").scope,
        ConfigScope::CompiledDefaults
    );

    let expected = pointers(snapshot.value());
    let actual: BTreeSet<String> = snapshot
        .provenance()
        .keys()
        .map(|pointer| pointer.as_str().to_owned())
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn untrusted_project_is_not_parsed_and_emits_bounded_diagnostic() {
    let sources = vec![
        layer(
            ConfigScope::CompiledDefaults,
            "defaults",
            json!({"safe": true}),
        ),
        raw_layer(
            ConfigScope::Project,
            "untrusted-project",
            SourceTrust::Untrusted,
            br#"{"formatVersion":1,"config":{"token":"inline""#,
        ),
    ];

    let runtime = ConfigRuntime::load(&sources, &AcceptAllConfig).expect("skip project");
    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.value(), &json!({"safe": true}));
    assert_eq!(snapshot.diagnostics().len(), 1);
    assert_eq!(
        snapshot.diagnostics()[0].code().to_string(),
        "untrusted-project-skipped"
    );
    assert_eq!(
        snapshot.diagnostics()[0].source().scope,
        ConfigScope::Project
    );
}

#[test]
fn equal_digest_keeps_generation_but_refreshes_provenance() {
    let defaults = layer(
        ConfigScope::CompiledDefaults,
        "defaults",
        json!({"same": 1}),
    );
    let runtime = ConfigRuntime::load(std::slice::from_ref(&defaults), &AcceptAllConfig)
        .expect("initial load");
    let initial = runtime.snapshot();

    let sources = vec![
        defaults,
        layer(ConfigScope::Global, "global", json!({"same": 1})),
    ];
    let outcome = runtime
        .reload(&sources, &AcceptAllConfig, &ReloadCancellation::new())
        .expect("equal reload");
    let refreshed = runtime.snapshot();

    assert!(!outcome.changed());
    assert_eq!(outcome.generation(), initial.generation());
    assert_eq!(outcome.digest(), initial.digest());
    assert_eq!(
        refreshed
            .source_at("/same")
            .expect("refreshed provenance")
            .scope,
        ConfigScope::Global
    );
}

#[test]
fn failed_domain_validation_rolls_back_complete_snapshot() {
    let valid = layer(
        ConfigScope::CompiledDefaults,
        "defaults",
        json!({"enabled": true}),
    );
    let validator = |value: &Value| {
        if value
            .as_object()
            .is_some_and(|object| object.len() == 1 && object["enabled"].is_boolean())
        {
            Ok(())
        } else {
            Err(ValidationFailure::new())
        }
    };
    let runtime = ConfigRuntime::load(std::slice::from_ref(&valid), &validator)
        .expect("initial validated load");
    let before = runtime.snapshot();
    let invalid = vec![
        valid,
        layer(
            ConfigScope::Explicit,
            "invalid-explicit",
            json!({"enabled": "yes", "unknown": true}),
        ),
    ];

    let error = runtime
        .reload(&invalid, &validator, &ReloadCancellation::new())
        .expect_err("domain validation must reject");
    assert_eq!(error.kind(), ConfigErrorKind::DomainValidation);
    let after = runtime.snapshot();
    assert!(Arc::ptr_eq(&before, &after));
}

#[test]
fn cancellation_rolls_back_without_publication() {
    let defaults = layer(
        ConfigScope::CompiledDefaults,
        "defaults",
        json!({"revision": 0}),
    );
    let runtime = ConfigRuntime::load(std::slice::from_ref(&defaults), &AcceptAllConfig)
        .expect("initial load");
    let before = runtime.snapshot();
    let cancellation = ReloadCancellation::new();
    cancellation.cancel();

    let error = runtime
        .reload(
            &[
                defaults,
                layer(ConfigScope::Explicit, "next", json!({"revision": 1})),
            ],
            &AcceptAllConfig,
            &cancellation,
        )
        .expect_err("cancelled reload");
    assert_eq!(error.kind(), ConfigErrorKind::Cancelled);
    assert!(Arc::ptr_eq(&before, &runtime.snapshot()));
}

#[test]
fn concurrent_readers_observe_only_complete_reload_snapshots() {
    let defaults = layer(
        ConfigScope::CompiledDefaults,
        "defaults",
        json!({"revision": 0, "mirror": 0}),
    );
    let runtime = ConfigRuntime::load(std::slice::from_ref(&defaults), &AcceptAllConfig)
        .expect("initial load");
    let stop = Arc::new(AtomicBool::new(false));
    let mut readers = Vec::new();

    for _ in 0..6 {
        let runtime = runtime.clone();
        let stop = stop.clone();
        readers.push(thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                let snapshot = runtime.snapshot();
                let revision = snapshot.value()["revision"]
                    .as_u64()
                    .expect("numeric revision");
                let mirror = snapshot.value()["mirror"].as_u64().expect("numeric mirror");
                assert_eq!(revision, mirror);
                assert_eq!(snapshot.generation(), revision + 1);
            }
        }));
    }

    for revision in 1_u64..=40 {
        let next = layer(
            ConfigScope::CompiledDefaults,
            "defaults",
            json!({"revision": revision, "mirror": revision}),
        );
        let outcome = runtime
            .reload(&[next], &AcceptAllConfig, &ReloadCancellation::new())
            .expect("concurrent reload");
        assert!(outcome.changed());
        assert_eq!(outcome.generation(), revision + 1);
    }
    stop.store(true, Ordering::Release);
    for reader in readers {
        reader.join().expect("reader thread");
    }
}

fn pointers(value: &Value) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    let mut stack = vec![(value, String::new())];
    while let Some((current, pointer)) = stack.pop() {
        result.insert(pointer.clone());
        match current {
            Value::Object(object) => {
                for (key, child) in object {
                    stack.push((child, child_pointer(&pointer, key)));
                }
            }
            Value::Array(array) => {
                for (index, child) in array.iter().enumerate() {
                    stack.push((child, child_pointer(&pointer, &index.to_string())));
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
    result
}

fn child_pointer(parent: &str, token: &str) -> String {
    let encoded = token.replace('~', "~0").replace('/', "~1");
    format!("{parent}/{encoded}")
}
