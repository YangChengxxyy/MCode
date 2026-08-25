//! API key resolution for providers.
//!
//! Order (first hit wins):
//! 1. an explicit key passed by the caller (CLI flag, config, …);
//! 2. a provider-specific environment variable (e.g. `OPENAI_API_KEY`);
//! 3. the provider's table in `~/.mcode/auth.toml` (`$MCODE_HOME`
//!    overrides the home directory):
//!
//! ```toml
//! [openai]
//! api_key = "sk-..."
//! ```

use std::path::{Path, PathBuf};

use crate::error::LlmError;

/// Default provider table key inside `auth.toml`.
const AUTH_KEY_FIELD: &str = "api_key";

/// Resolve the MCode home directory: `$MCODE_HOME` if set, otherwise
/// `$HOME/.mcode` (or `%USERPROFILE%\.mcode` on Windows).
pub fn mcode_home() -> PathBuf {
    if let Some(dir) = std::env::var_os("MCODE_HOME") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".mcode")
}

/// Path of the auth file (`<mcode_home>/auth.toml`).
pub fn auth_file_path() -> PathBuf {
    mcode_home().join("auth.toml")
}

/// Resolve an API key using the real environment and the default auth
/// file location.
///
/// * `explicit` – key passed by the caller, if any.
/// * `env_var` – environment variable to consult (e.g. `OPENAI_API_KEY`).
/// * `provider_id` – table name in `auth.toml` (e.g. `openai`).
pub fn resolve_api_key(
    explicit: Option<&str>,
    env_var: &str,
    provider_id: &str,
) -> Result<String, LlmError> {
    let auth_path = auth_file_path();
    resolve_api_key_with(
        explicit,
        env_lookup(env_var),
        env_var,
        Some(&auth_path),
        provider_id,
    )
}

/// Testable core of [`resolve_api_key`]: the environment lookup and auth
/// file path are injected so tests never mutate global state.
pub(crate) fn resolve_api_key_with(
    explicit: Option<&str>,
    env: Option<String>,
    env_var: &str,
    auth_path: Option<&Path>,
    provider_id: &str,
) -> Result<String, LlmError> {
    if let Some(key) = explicit.filter(|k| !k.trim().is_empty()) {
        return Ok(key.to_owned());
    }
    if let Some(key) = env.filter(|k| !k.trim().is_empty()) {
        return Ok(key);
    }
    if let Some(path) = auth_path.filter(|p| p.is_file()) {
        let raw = std::fs::read_to_string(path)
            .map_err(|err| LlmError::Config(format!("failed to read {}: {err}", path.display())))?;
        let parsed: toml::Table = raw.parse().map_err(|err| {
            LlmError::Config(format!("invalid TOML in {}: {err}", path.display()))
        })?;
        if let Some(table) = parsed.get(provider_id) {
            if let Some(key) = table.get(AUTH_KEY_FIELD).and_then(as_trimmed_string) {
                if !key.is_empty() {
                    return Ok(key);
                }
            }
        }
    }
    let auth_location = auth_path
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.mcode/auth.toml".into());
    Err(LlmError::Config(format!(
        "no API key for provider '{provider_id}': pass one explicitly, set {env_var}, \
         or add [{provider_id}] {AUTH_KEY_FIELD} = \"...\" to {auth_location}"
    )))
}

fn env_lookup(var: &str) -> Option<String> {
    std::env::var(var).ok()
}

fn as_trimmed_string(value: &toml::Value) -> Option<String> {
    value.as_str().map(|s| s.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    /// Minimal temp-dir guard (avoids a `tempfile` dev-dependency).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "mcode-llm-auth-{tag}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }

        fn write(&self, name: &str, content: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, content).expect("write temp file");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    #[test]
    fn explicit_key_wins() {
        let dir = TempDir::new("explicit");
        let auth = dir.write("auth.toml", "[openai]\napi_key = \"from-file\"\n");
        let key = resolve_api_key_with(
            Some("explicit-key"),
            Some("from-env".into()),
            "OPENAI_API_KEY",
            Some(&auth),
            "openai",
        )
        .unwrap();
        assert_eq!(key, "explicit-key");
    }

    #[test]
    fn env_beats_auth_file() {
        let dir = TempDir::new("env");
        let auth = dir.write("auth.toml", "[openai]\napi_key = \"from-file\"\n");
        let key = resolve_api_key_with(
            None,
            Some("from-env".into()),
            "OPENAI_API_KEY",
            Some(&auth),
            "openai",
        )
        .unwrap();
        assert_eq!(key, "from-env");
    }

    #[test]
    fn auth_file_supplies_key() {
        let dir = TempDir::new("file");
        let auth = dir.write("auth.toml", "[openai]\napi_key = \"from-file\"\n");
        let key =
            resolve_api_key_with(None, None, "OPENAI_API_KEY", Some(&auth), "openai").unwrap();
        assert_eq!(key, "from-file");
    }

    #[test]
    fn auth_file_missing_table_fails_with_config_error() {
        let dir = TempDir::new("missing-table");
        let auth = dir.write("auth.toml", "[other]\napi_key = \"x\"\n");
        let err =
            resolve_api_key_with(None, None, "OPENAI_API_KEY", Some(&auth), "openai").unwrap_err();
        assert!(matches!(err, LlmError::Config(_)));
        assert!(err.to_string().contains("openai"));
    }

    #[test]
    fn blank_keys_are_treated_as_missing() {
        let dir = TempDir::new("blank");
        let auth = dir.write("auth.toml", "[openai]\napi_key = \"   \"\n");
        let err = resolve_api_key_with(Some("  "), None, "OPENAI_API_KEY", Some(&auth), "openai")
            .unwrap_err();
        assert!(matches!(err, LlmError::Config(_)));
    }

    #[test]
    fn invalid_toml_is_a_config_error() {
        let dir = TempDir::new("bad-toml");
        let auth = dir.write("auth.toml", "not [ valid toml");
        let err =
            resolve_api_key_with(None, None, "OPENAI_API_KEY", Some(&auth), "openai").unwrap_err();
        assert!(matches!(err, LlmError::Config(_)));
        assert!(err.to_string().contains("invalid TOML"));
    }

    #[test]
    fn missing_everything_mentions_env_var_and_auth_file() {
        let err = resolve_api_key_with(None, None, "OPENAI_API_KEY", None, "openai").unwrap_err();
        assert!(matches!(err, LlmError::Config(_)));
        assert!(err.to_string().contains("OPENAI_API_KEY"));
        assert!(err.to_string().contains("auth.toml"));
    }
}
