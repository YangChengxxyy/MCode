// Rust guideline compliant 2026-08-26

use std::path::PathBuf;

use mcode_config::{ConfigLayer, ConfigScope, ConfigSource, SourceTrust};
use serde_json::{Value, json};

pub fn source(scope: ConfigScope, label: &str, trust: SourceTrust) -> ConfigSource {
    ConfigSource::new(scope, PathBuf::from(label), trust)
}

pub fn layer(scope: ConfigScope, label: &str, config: Value) -> ConfigLayer {
    let envelope = serde_json::to_vec(&json!({
        "formatVersion": 1,
        "config": config,
    }))
    .expect("serialize test envelope");
    ConfigLayer::inline(source(scope, label, SourceTrust::Trusted), envelope)
}

pub fn raw_layer(
    scope: ConfigScope,
    label: &str,
    trust: SourceTrust,
    bytes: impl AsRef<[u8]>,
) -> ConfigLayer {
    ConfigLayer::inline(source(scope, label, trust), bytes)
}
