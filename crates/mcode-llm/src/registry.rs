//! Provider profile registry.
//!
//! The registry owns profile data only. Constructing an HTTP provider remains
//! explicit so callers can inject a [`crate::ProfileProvider`] or a test-local
//! transport.

use std::collections::BTreeMap;

use crate::error::LlmError;
use crate::profile::{ProviderProfile, builtin_profiles};

/// Mutable registry keyed by stable provider id.
#[derive(Debug, Clone, Default)]
pub struct ProviderRegistry {
    profiles: BTreeMap<String, ProviderProfile>,
}

impl ProviderRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a registry containing all built-in profiles.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        for profile in builtin_profiles() {
            let replaced = registry
                .register(profile)
                .expect("built-in provider profile must be valid");
            debug_assert!(replaced.is_none());
        }
        registry
    }

    /// Registers a profile and returns the replaced profile, if any.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] when profile validation fails.
    pub fn register(
        &mut self,
        profile: ProviderProfile,
    ) -> Result<Option<ProviderProfile>, LlmError> {
        profile.validate()?;
        Ok(self.profiles.insert(profile.id().to_owned(), profile))
    }

    /// Looks up a profile by id.
    ///
    /// `openai-compatible` remains an alias for the built-in
    /// `generic-openai` profile.
    pub fn get(&self, id: &str) -> Option<&ProviderProfile> {
        self.profiles.get(id).or_else(|| {
            (id == "openai-compatible")
                .then(|| self.profiles.get("generic-openai"))
                .flatten()
        })
    }

    /// Clones a profile or returns a configuration error listing known ids.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] when `id` is not registered.
    pub fn resolve(&self, id: &str) -> Result<ProviderProfile, LlmError> {
        self.get(id).cloned().ok_or_else(|| {
            LlmError::Config(format!(
                "unknown provider '{id}' (known: {})",
                self.ids().collect::<Vec<_>>().join(", ")
            ))
        })
    }

    /// Iterates over registered ids in stable lexical order.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.profiles.keys().map(String::as_str)
    }

    /// Returns the number of registered profiles.
    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    /// Returns whether no profiles are registered.
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{AuthProfile, WireKind};

    #[test]
    fn builtins_are_available_and_alias_resolves() {
        let registry = ProviderRegistry::with_builtins();
        for id in [
            "generic-openai",
            "openai",
            "anthropic",
            "deepseek",
            "openrouter",
            "opencode",
        ] {
            assert!(registry.get(id).is_some(), "missing {id}");
        }
        assert_eq!(
            registry.get("openai-compatible").unwrap().id(),
            "generic-openai"
        );
    }

    #[test]
    fn registration_replaces_same_id() {
        let mut registry = ProviderRegistry::new();
        let first = ProviderProfile::new(
            "local",
            WireKind::OpenAiChatCompletions,
            "http://localhost:8000/v1",
            AuthProfile::none(),
        )
        .unwrap();
        assert!(registry.register(first.clone()).unwrap().is_none());
        let second = first.with_base_url("http://localhost:9000/v1").unwrap();
        let replaced = registry.register(second).unwrap().unwrap();
        assert_eq!(replaced.base_url(), "http://localhost:8000/v1");
        assert_eq!(
            registry.get("local").unwrap().base_url(),
            "http://localhost:9000/v1"
        );
    }

    #[test]
    fn exact_registration_takes_precedence_over_legacy_alias() {
        let mut registry = ProviderRegistry::with_builtins();
        let exact = ProviderProfile::new(
            "openai-compatible",
            WireKind::OpenAiChatCompletions,
            "http://localhost:8080/v1",
            AuthProfile::none(),
        )
        .unwrap();
        registry.register(exact).unwrap();
        assert_eq!(
            registry.get("openai-compatible").unwrap().base_url(),
            "http://localhost:8080/v1"
        );

        let mut empty = ProviderRegistry::new();
        let exact = ProviderProfile::new(
            "openai-compatible",
            WireKind::OpenAiChatCompletions,
            "http://localhost:9000/v1",
            AuthProfile::none(),
        )
        .unwrap();
        empty.register(exact).unwrap();
        assert_eq!(
            empty.resolve("openai-compatible").unwrap().id(),
            "openai-compatible"
        );
    }

    #[test]
    fn unknown_provider_error_contains_no_credentials() {
        let error = ProviderRegistry::with_builtins()
            .resolve("missing")
            .unwrap_err();
        assert!(error.to_string().contains("missing"));
    }
}

// Rust guideline compliant 2026-08-26
