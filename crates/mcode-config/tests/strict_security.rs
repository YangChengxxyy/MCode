// Rust guideline compliant 2026-08-26

mod common;

use common::{layer, raw_layer, source};
use mcode_config::{
    AcceptAllConfig, ConfigErrorKind, ConfigLayer, ConfigLimits, ConfigRuntime, ConfigScope,
    SourceTrust,
};
use serde_json::{Map, Value, json};

const DUPLICATE_JSON: &[u8] = br#"{"formatVersion":1,"config":{"nested":{"same":1,"same":2}}}"#;

fn object_with_member(key: &str, value: Value) -> Value {
    let mut object = Map::new();
    object.insert(key.to_owned(), value);
    Value::Object(object)
}

#[test]
fn duplicate_keys_are_rejected_in_every_precedence_layer() {
    for scope in [
        ConfigScope::CompiledDefaults,
        ConfigScope::Global,
        ConfigScope::Project,
        ConfigScope::Explicit,
    ] {
        let duplicate = raw_layer(scope, "duplicate", SourceTrust::Trusted, DUPLICATE_JSON);
        let sources = if scope == ConfigScope::CompiledDefaults {
            vec![duplicate]
        } else {
            vec![
                layer(ConfigScope::CompiledDefaults, "defaults", json!({})),
                duplicate,
            ]
        };
        let error = ConfigRuntime::load(&sources, &AcceptAllConfig)
            .expect_err("duplicate key must fail every layer");
        assert_eq!(error.kind(), ConfigErrorKind::DuplicateKey, "{scope}");
        assert_eq!(
            error.config_source().expect("duplicate source").scope,
            scope
        );
    }
}

#[test]
fn strict_json_rejects_comments_commas_trailing_content_and_partial_input() {
    let cases: &[&[u8]] = &[
        br#"{"formatVersion":1,// comment
             "config":{}}"#,
        br#"{"formatVersion":1,"config":{},}"#,
        br#"{"formatVersion":1,"config":{}} true"#,
        br#"{"formatVersion":1,"config":{"partial":true}"#,
    ];

    for bytes in cases {
        let source = raw_layer(
            ConfigScope::CompiledDefaults,
            "invalid-json",
            SourceTrust::Trusted,
            bytes,
        );
        let error = ConfigRuntime::load(&[source], &AcceptAllConfig)
            .expect_err("non-strict JSON must fail");
        assert_eq!(error.kind(), ConfigErrorKind::InvalidJson);
    }
}

#[test]
fn envelope_requires_exact_current_version_and_shape() {
    let cases = [
        (
            br#"{"formatVersion":2,"config":{}}"#.as_slice(),
            ConfigErrorKind::UnsupportedFormatVersion,
        ),
        (
            br#"{"formatVersion":"1","config":{}}"#.as_slice(),
            ConfigErrorKind::InvalidEnvelope,
        ),
        (
            br#"{"formatVersion":1,"config":{},"extra":true}"#.as_slice(),
            ConfigErrorKind::InvalidEnvelope,
        ),
        (
            br#"{"formatVersion":1}"#.as_slice(),
            ConfigErrorKind::InvalidEnvelope,
        ),
    ];

    for (bytes, expected) in cases {
        let source = raw_layer(
            ConfigScope::CompiledDefaults,
            "bad-envelope",
            SourceTrust::Trusted,
            bytes,
        );
        let error =
            ConfigRuntime::load(&[source], &AcceptAllConfig).expect_err("bad envelope must fail");
        assert_eq!(error.kind(), expected);
    }
}

#[test]
fn credential_like_variants_fail_closed_for_inline_values() {
    let unsafe_configs = [
        json!({"token": "inline"}),
        json!({"API_KEY": 42}),
        json!({"apiKeys": ["inline"]}),
        json!({"API_KEYS": ["inline"]}),
        json!({"accessKeys": ["inline"]}),
        json!({"cachedAccessToken": "inline"}),
        json!({"clientSecret": ["inline"]}),
        json!({"password": {"value": "inline"}}),
        json!({"session-cookie": true}),
        json!({"authorization": "Bearer inline"}),
        json!({"credentials": {"user": "inline"}}),
        json!({"secretRef": "orphan-reference"}),
        json!({"outer": [{"refreshToken": "inline"}]}),
    ];

    for config in unsafe_configs {
        let source = layer(ConfigScope::CompiledDefaults, "unsafe", config);
        let error = ConfigRuntime::load(&[source], &AcceptAllConfig)
            .expect_err("inline credential must fail");
        assert_eq!(error.kind(), ConfigErrorKind::CredentialValue);
        assert!(error.pointer().is_some());
    }
}

#[test]
fn acronym_credential_variants_enforce_nested_patch_and_material_rules() {
    let cases = [
        ("camel API token", "cachedAPIToken"),
        ("camel API key", "cachedAPIKey"),
        ("camel OAuth token", "cachedOAuthToken"),
        ("camel plural API token", "cachedAPITokens"),
        ("snake API token", "cached_api_token"),
        ("kebab API key", "cached-api-key"),
        ("upper snake API token", "CACHED_API_TOKEN"),
        ("upper kebab API key", "CACHED-API-KEY"),
        ("mixed all-caps suffix", "cachedAPITOKEN"),
        ("joined all-caps API token", "CACHEDAPITOKEN"),
        ("joined all-caps API key", "CACHEDAPIKEY"),
        ("joined all-caps OAuth token", "CACHEDOAUTHTOKEN"),
        ("numeric all-caps API token", "cachedAPITOKEN2"),
        ("joined numeric all-caps API token", "CACHEDAPITOKEN2"),
        ("versioned joined all-caps API key", "CACHEDAPIKEY_V2"),
        ("compact versioned all-caps API key", "CACHEDAPIKEYV2"),
        ("mixed all-caps API secret", "cachedAPISECRET"),
        ("joined all-caps API secret", "CACHEDAPISECRET"),
        ("mixed all-caps JWT token", "cachedJWTTOKEN"),
        ("joined all-caps JWT token", "CACHEDJWTTOKEN"),
        ("numeric all-caps JWT token", "cachedJWTTOKEN2"),
    ];

    for (case, key) in cases {
        let inline = layer(
            ConfigScope::CompiledDefaults,
            case,
            json!({
                "providers": {
                    "primary": object_with_member(key, json!("inline"))
                }
            }),
        );
        let error = ConfigRuntime::load(&[inline], &AcceptAllConfig)
            .expect_err("nested inline credential must fail");
        assert_eq!(error.kind(), ConfigErrorKind::CredentialValue, "{case}");
        assert!(error.pointer().is_some(), "{case}");

        let strict_material = layer(
            ConfigScope::CompiledDefaults,
            case,
            json!({
                "providers": [object_with_member(
                    key,
                    json!({"secretRef": "provider/default"}),
                )]
            }),
        );
        ConfigRuntime::load(&[strict_material], &AcceptAllConfig)
            .expect("exact secretRef must be valid in material values");

        let malformed_material = layer(
            ConfigScope::CompiledDefaults,
            case,
            json!({
                "providers": [object_with_member(
                    key,
                    json!({"secretRef": "provider/default", "extra": true}),
                )]
            }),
        );
        let error = ConfigRuntime::load(&[malformed_material], &AcceptAllConfig)
            .expect_err("non-exact secretRef must fail in material values");
        assert_eq!(error.kind(), ConfigErrorKind::CredentialValue, "{case}");

        let defaults = layer(
            ConfigScope::CompiledDefaults,
            "defaults",
            json!({
                "providers": {
                    "primary": object_with_member(
                        key,
                        json!({"secretRef": "provider/default"}),
                    )
                }
            }),
        );
        let deletion = layer(
            ConfigScope::Explicit,
            case,
            json!({
                "providers": {
                    "primary": object_with_member(key, Value::Null)
                }
            }),
        );
        let runtime = ConfigRuntime::load(&[defaults, deletion], &AcceptAllConfig)
            .expect("null must remain a deletion marker in nested patch objects");
        assert_eq!(
            runtime.snapshot().value(),
            &json!({"providers": {"primary": {}}}),
            "{case}"
        );

        let material_null = layer(
            ConfigScope::CompiledDefaults,
            case,
            json!({
                "providers": [object_with_member(key, Value::Null)]
            }),
        );
        let error = ConfigRuntime::load(&[material_null], &AcceptAllConfig)
            .expect_err("null credential must fail in array material values");
        assert_eq!(error.kind(), ConfigErrorKind::CredentialValue, "{case}");
    }
}

#[test]
fn token_quantity_fields_are_not_treated_as_credentials() {
    let config = json!({
        "maxTokens": 4_096,
        "max_tokens": 8_192,
        "MAX_TOKENS": 12_288,
        "tokenBudget": 16_384,
        "token-budget": 32_768,
        "cachedTokens": 5,
        "cached_tokens": 6,
        "CACHEDTOKENS": 7,
        "PROMPTTOKENS": 8,
        "CONTEXTWINDOWTOKENS": 9,
        "tokenCount": 10,
        "monkey": "capuchin",
        "keyboardLayout": "dvorak",
        "usage": {
            "promptTokens": 10,
            "completion_tokens": 20,
            "totalTokens": 30,
            "reasoningTokens": 40,
            "contextWindowTokens": 50
        }
    });
    let source = layer(
        ConfigScope::CompiledDefaults,
        "token-quantities",
        config.clone(),
    );
    let runtime = ConfigRuntime::load(&[source], &AcceptAllConfig).expect("token quantities");
    assert_eq!(runtime.snapshot().value(), &config);
}

#[test]
fn only_exact_nonempty_secret_reference_shape_is_safe() {
    let safe = layer(
        ConfigScope::CompiledDefaults,
        "safe",
        json!({
            "apiKey": {"secretRef": "provider/default"},
            "password": {"secretRef": "login/default"}
        }),
    );
    let runtime = ConfigRuntime::load(&[safe], &AcceptAllConfig).expect("safe references");
    assert_eq!(
        runtime.snapshot().value()["apiKey"]["secretRef"],
        "provider/default"
    );

    for config in [
        json!({"apiKey": {"secretRef": "", "extra": true}}),
        json!({"apiKey": {"secretRef": ""}}),
        json!({"apiKey": {"secretRef": 1}}),
        json!({"apiKey": {"secret_ref": "name"}}),
    ] {
        let source = layer(ConfigScope::CompiledDefaults, "bad-reference", config);
        let error = ConfigRuntime::load(&[source], &AcceptAllConfig)
            .expect_err("non-exact reference must fail");
        assert_eq!(error.kind(), ConfigErrorKind::CredentialValue);
    }
}

#[test]
fn credential_deletion_is_allowed_but_later_deletion_cannot_hide_inline_input() {
    let sources = vec![
        layer(
            ConfigScope::CompiledDefaults,
            "defaults",
            json!({"apiKey": {"secretRef": "provider/default"}}),
        ),
        layer(ConfigScope::Explicit, "delete", json!({"apiKey": null})),
    ];
    let runtime = ConfigRuntime::load(&sources, &AcceptAllConfig).expect("credential deletion");
    assert_eq!(runtime.snapshot().value(), &json!({}));

    let unsafe_then_delete = vec![
        layer(
            ConfigScope::CompiledDefaults,
            "unsafe-defaults",
            json!({"apiKey": "inline-sentinel"}),
        ),
        layer(ConfigScope::Explicit, "delete", json!({"apiKey": null})),
    ];
    let error = ConfigRuntime::load(&unsafe_then_delete, &AcceptAllConfig)
        .expect_err("unsafe source cannot be hidden");
    assert_eq!(error.kind(), ConfigErrorKind::CredentialValue);
}

#[test]
fn credential_null_inside_array_replacements_is_rejected() {
    for config in [
        json!({"providers": [{"apiKey": null}]}),
        json!([{"accessToken": null}]),
    ] {
        let source = layer(ConfigScope::CompiledDefaults, "material-null", config);
        let error = ConfigRuntime::load(&[source], &AcceptAllConfig)
            .expect_err("array replacement null is material");
        assert_eq!(error.kind(), ConfigErrorKind::CredentialValue);
        assert!(error.pointer().is_some());
    }
}

#[test]
fn strings_are_never_interpolated() {
    let source = layer(
        ConfigScope::CompiledDefaults,
        "defaults",
        json!({"literal": "${MCODE_HOME}/settings.json", "escaped": "$${TOKEN}"}),
    );
    let runtime = ConfigRuntime::load(&[source], &AcceptAllConfig).expect("literal strings");
    assert_eq!(
        runtime.snapshot().value()["literal"],
        "${MCODE_HOME}/settings.json"
    );
    assert_eq!(runtime.snapshot().value()["escaped"], "$${TOKEN}");
}

#[test]
fn debug_error_and_diagnostics_redact_all_json_values() {
    let sentinel = "VALUE-MUST-NOT-APPEAR-8f392e";
    let unsafe_bytes = format!(r#"{{"formatVersion":1,"config":{{"apiToken":"{sentinel}"}}}}"#);
    let unsafe_layer = ConfigLayer::inline(
        source(
            ConfigScope::CompiledDefaults,
            "redaction-source",
            SourceTrust::Trusted,
        ),
        &unsafe_bytes,
    );
    assert!(!format!("{unsafe_layer:?}").contains(sentinel));
    let error =
        ConfigRuntime::load(&[unsafe_layer], &AcceptAllConfig).expect_err("unsafe credential");
    assert!(!format!("{error:?}").contains(sentinel));
    assert!(!error.to_string().contains(sentinel));

    let safe_layer = layer(
        ConfigScope::CompiledDefaults,
        "safe-redaction-source",
        json!({"ordinary": sentinel}),
    );
    let runtime = ConfigRuntime::load(&[safe_layer], &AcceptAllConfig).expect("safe snapshot");
    assert!(!format!("{:?}", runtime.snapshot()).contains(sentinel));
    assert!(!format!("{runtime:?}").contains(sentinel));

    let untrusted = raw_layer(
        ConfigScope::Project,
        "untrusted-redaction-source",
        SourceTrust::Untrusted,
        unsafe_bytes,
    );
    let defaults = layer(ConfigScope::CompiledDefaults, "defaults", json!({}));
    let runtime = ConfigRuntime::load(&[defaults, untrusted], &AcceptAllConfig)
        .expect("untrusted diagnostic");
    assert!(!format!("{:?}", runtime.snapshot().diagnostics()).contains(sentinel));
}

#[test]
fn bytes_depth_nodes_and_utf8_are_bounded() {
    let defaults = ConfigLimits::default();

    let mut byte_limits = defaults;
    byte_limits.max_source_bytes = 16;
    byte_limits.max_total_bytes = 16;
    let oversized = layer(
        ConfigScope::CompiledDefaults,
        "oversized",
        json!({"large": "more than sixteen bytes"}),
    );
    let error = ConfigRuntime::load_with_options(
        &[oversized],
        &AcceptAllConfig,
        byte_limits,
        &mcode_config::ReloadCancellation::new(),
    )
    .expect_err("oversized source");
    assert_eq!(error.kind(), ConfigErrorKind::Oversized);

    let mut depth_limits = defaults;
    depth_limits.max_depth = 2;
    let deep = raw_layer(
        ConfigScope::CompiledDefaults,
        "deep",
        SourceTrust::Trusted,
        br#"{"formatVersion":1,"config":{"one":{"two":true}}}"#,
    );
    let error = ConfigRuntime::load_with_options(
        &[deep],
        &AcceptAllConfig,
        depth_limits,
        &mcode_config::ReloadCancellation::new(),
    )
    .expect_err("deep source");
    assert_eq!(error.kind(), ConfigErrorKind::TooDeep);

    let mut node_limits = defaults;
    node_limits.max_nodes = 6;
    let many_nodes = layer(
        ConfigScope::CompiledDefaults,
        "many-nodes",
        json!({"a": 1, "b": 2, "c": 3, "d": 4}),
    );
    let error = ConfigRuntime::load_with_options(
        &[many_nodes],
        &AcceptAllConfig,
        node_limits,
        &mcode_config::ReloadCancellation::new(),
    )
    .expect_err("node bound");
    assert_eq!(error.kind(), ConfigErrorKind::TooManyNodes);

    let invalid_utf8 = raw_layer(
        ConfigScope::CompiledDefaults,
        "invalid-utf8",
        SourceTrust::Trusted,
        [b'{', 0xff, b'}'],
    );
    let error = ConfigRuntime::load(&[invalid_utf8], &AcceptAllConfig).expect_err("invalid UTF-8");
    assert_eq!(error.kind(), ConfigErrorKind::NonUtf8);
}

#[test]
fn invalid_limits_source_count_and_nonproject_trust_fail() {
    let defaults = layer(ConfigScope::CompiledDefaults, "defaults", json!({}));
    let limits = ConfigLimits {
        max_depth: 0,
        ..ConfigLimits::default()
    };
    let error = ConfigRuntime::load_with_options(
        std::slice::from_ref(&defaults),
        &AcceptAllConfig,
        limits,
        &mcode_config::ReloadCancellation::new(),
    )
    .expect_err("invalid limits");
    assert_eq!(error.kind(), ConfigErrorKind::InvalidLimits);

    let limits = ConfigLimits {
        max_sources: 1,
        ..ConfigLimits::default()
    };
    let error = ConfigRuntime::load_with_options(
        &[defaults, layer(ConfigScope::Global, "global", json!({}))],
        &AcceptAllConfig,
        limits,
        &mcode_config::ReloadCancellation::new(),
    )
    .expect_err("source count");
    assert_eq!(error.kind(), ConfigErrorKind::TooManySources);

    let untrusted_global = raw_layer(
        ConfigScope::Global,
        "untrusted-global",
        SourceTrust::Untrusted,
        br#"{"formatVersion":1,"config":{}}"#,
    );
    let defaults = layer(ConfigScope::CompiledDefaults, "defaults", json!({}));
    let error = ConfigRuntime::load(&[defaults, untrusted_global], &AcceptAllConfig)
        .expect_err("untrusted global");
    assert_eq!(error.kind(), ConfigErrorKind::UntrustedSource);
}
