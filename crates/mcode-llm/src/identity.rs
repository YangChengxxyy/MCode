//! Pi-compatible HTTP client identity generation.
//!
//! Provider profiles use the Pi identity by default. Callers can inject a
//! deterministic identity for tests or an explicit user-agent for embedding.

#[cfg(target_os = "windows")]
use std::mem::size_of;

use reqwest::header::HeaderValue;

use crate::error::LlmError;

/// HTTP client identity attached to provider and catalog requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientIdentity {
    user_agent: String,
}

impl ClientIdentity {
    /// Creates a validated explicit user-agent identity.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] when `user_agent` is empty or cannot be
    /// represented as one HTTP header value.
    pub fn new(user_agent: impl Into<String>) -> Result<Self, LlmError> {
        let user_agent = user_agent.into();
        validate_user_agent(&user_agent)?;
        Ok(Self { user_agent })
    }

    /// Creates a deterministic Pi-compatible identity from OS components.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] when a component would produce an invalid
    /// HTTP header value.
    pub fn pi(platform: &str, release: &str, arch: &str) -> Result<Self, LlmError> {
        Self::new(pi_compat_user_agent(platform, release, arch)?)
    }

    /// Creates the default Pi-compatible identity for the current process.
    pub fn system_pi() -> Self {
        let platform = pi_platform(std::env::consts::OS);
        let release = system_release();
        let arch = pi_arch(std::env::consts::ARCH);
        // All system-derived components are stripped of control characters.
        // Keep a defensive fallback so identity generation never blocks basic
        // provider startup on an unusual host.
        Self::pi(platform, &release, arch).unwrap_or_else(|_| Self {
            user_agent: format!("pi ({platform} unknown; {arch})"),
        })
    }

    /// Borrows the complete user-agent value.
    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }
}

/// Formats the Pi compatibility user-agent shape.
///
/// # Errors
///
/// Returns [`LlmError::Config`] when a component is blank or contains bytes
/// that are invalid in an HTTP header value.
pub fn pi_compat_user_agent(platform: &str, release: &str, arch: &str) -> Result<String, LlmError> {
    if [platform, release, arch]
        .into_iter()
        .any(|part| part.trim().is_empty())
    {
        return Err(LlmError::Config(
            "Pi identity components must not be empty".into(),
        ));
    }
    let value = format!(
        "pi ({} {}; {})",
        platform.trim(),
        release.trim(),
        arch.trim()
    );
    validate_user_agent(&value)?;
    Ok(value)
}

fn validate_user_agent(value: &str) -> Result<(), LlmError> {
    if value.trim().is_empty() || HeaderValue::from_str(value).is_err() {
        return Err(LlmError::Config("invalid HTTP user-agent value".into()));
    }
    Ok(())
}

fn pi_platform(platform: &str) -> &str {
    match platform {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
}

fn pi_arch(arch: &str) -> &str {
    match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

fn system_release() -> String {
    #[cfg(unix)]
    {
        // Bind the utsname before borrowing its fields; the borrowed slice
        // must not outlive the temporary struct.
        let utsname = rustix::system::uname();
        let release = utsname.release().to_string_lossy();
        let release = clean_component(&release);
        if !release.is_empty() {
            return release;
        }
    }

    #[cfg(target_os = "windows")]
    if let Some(release) = windows_release() {
        return release;
    }

    clean_component(std::env::consts::OS)
}

#[cfg(target_os = "windows")]
fn windows_release() -> Option<String> {
    use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;

    let mut version = OSVERSIONINFOW {
        dwOSVersionInfoSize: u32::try_from(size_of::<OSVERSIONINFOW>()).ok()?,
        ..OSVERSIONINFOW::default()
    };
    // SAFETY: `version` is a writable OSVERSIONINFOW with its documented size
    // initialized, and the pointer remains valid for the complete API call.
    let status = unsafe { RtlGetVersion(&mut version) };
    if status < 0 {
        return None;
    }
    Some(format!(
        "{}.{}.{}",
        version.dwMajorVersion, version.dwMinorVersion, version.dwBuildNumber
    ))
}

fn clean_component(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_pi_compatibility_identity() {
        assert_eq!(
            pi_compat_user_agent("linux", "6.8.0", "x64").unwrap(),
            "pi (linux 6.8.0; x64)"
        );
    }

    #[test]
    fn maps_rust_platform_and_arch_names() {
        assert_eq!(pi_platform("windows"), "win32");
        assert_eq!(pi_platform("macos"), "darwin");
        assert_eq!(pi_arch("x86_64"), "x64");
        assert_eq!(pi_arch("aarch64"), "arm64");
    }

    #[test]
    fn rejects_header_injection() {
        let error = ClientIdentity::pi("linux\r\nx-evil: yes", "1", "x64").unwrap_err();
        assert!(matches!(error, LlmError::Config(_)));
    }

    #[test]
    fn system_identity_uses_kernel_release() {
        let release = system_release();
        assert!(!release.is_empty());
        assert_ne!(release, std::env::consts::OS);
        #[cfg(target_os = "windows")]
        {
            assert_ne!(release, "Windows_NT");
            assert!(release.split('.').all(|part| part.parse::<u32>().is_ok()));
        }

        let identity = ClientIdentity::system_pi();
        assert!(identity.user_agent().starts_with("pi ("));
        assert!(identity.user_agent().contains(&release));
        assert!(!identity.user_agent().contains("mcode/"));
    }
}

// Rust guideline compliant 2026-08-26
