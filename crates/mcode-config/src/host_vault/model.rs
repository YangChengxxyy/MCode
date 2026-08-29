//! Defines and validates the private secret-bearing Host-vault model.
//!
//! The owned-file layer owns input in a `Zeroizing<Vec<u8>>`; this parser only
//! borrows that input, and all textual fields remain borrowed because escapes
//! are rejected before serde runs. Decoded secrets and canonical Base64
//! intermediates are zeroizing allocations. Serialization writes into a
//! pre-sized zeroizing output, and the owned-file transaction keeps that
//! replacement zeroizing through publication. Every return path drops these
//! owners before the input or replacement leaves its scope.

// Rust guideline compliant 2026-08-29

use std::fmt::{self, Debug, Formatter};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::{Zeroize, Zeroizing};

#[cfg(test)]
use super::MAX_HOST_VAULT_BYTES;
use super::{HOST_VAULT_FORMAT_VERSION, HOST_VAULT_KIND, VaultRevision};
use crate::home::is_valid_portable_id;
use crate::manager_registry::is_valid_sha256_digest;
use crate::{ConfigError, ConfigErrorKind, PluginFamily};

const MAX_ACTIVE_CREDENTIALS: usize = 64;
const MAX_REVOKED_CREDENTIALS: usize = 256;
const MAX_GRANTS: usize = 1024;
const MAX_SECRET_BYTES: usize = 8192;
// Eight KiB encodes to at most 10,923 unpadded Base64 characters.
const MAX_ENCODED_SECRET_BYTES: usize = 10_923;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct VaultDocument<'a> {
    format_version: u32,
    kind: &'a str,
    revision: u64,
    #[serde(borrow)]
    credentials: Vec<Credential<'a>>,
    #[serde(borrow)]
    grants: Vec<Grant<'a>>,
}

impl VaultDocument<'_> {
    pub(super) fn revision(&self) -> VaultRevision {
        VaultRevision(self.revision)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Credential<'a> {
    service_id: &'a str,
    account_id: &'a str,
    issuer_id: &'a str,
    auth_schema_id: &'a str,
    credential_version: u64,
    state: CredentialState,
    secret_base64_url: Option<Secret>,
}

#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum CredentialState {
    Active,
    Revoked,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Grant<'a> {
    consumer_family: ConsumerFamily,
    manager_id: &'a str,
    pack_id: &'a str,
    operation_id: &'a str,
    service_id: &'a str,
    account_id: &'a str,
    authority_digest: &'a str,
}

#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum ConsumerFamily {
    Providers,
    Web,
    Usage,
}

impl ConsumerFamily {
    const fn plugin_family(self) -> PluginFamily {
        match self {
            Self::Providers => PluginFamily::Providers,
            Self::Web => PluginFamily::Web,
            Self::Usage => PluginFamily::Usage,
        }
    }

    const fn sort_key(self) -> &'static str {
        match self {
            Self::Providers => "providers",
            Self::Web => "web",
            Self::Usage => "usage",
        }
    }
}

struct Secret(Zeroizing<Vec<u8>>);

impl Debug for Secret {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret([REDACTED])")
    }
}

impl Serialize for Secret {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let encoded_capacity = self.0.len().div_ceil(3) * 4;
        let mut encoded = Zeroizing::new(String::with_capacity(encoded_capacity));
        URL_SAFE_NO_PAD.encode_string(self.0.as_slice(), &mut encoded);
        serializer.serialize_str(encoded.as_str())
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(SecretVisitor)
    }
}

struct SecretVisitor;

impl<'de> Visitor<'de> for SecretVisitor {
    type Value = Secret;

    fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("an unescaped canonical Base64URL secret")
    }

    fn visit_borrowed_str<E>(self, value: &'de str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        decode_secret(value).map_err(|()| E::custom("invalid secret encoding"))
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(E::custom("non-borrowed secret strings are rejected"))
    }

    fn visit_string<E>(self, mut value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        value.zeroize();
        Err(E::custom("owned secret strings are rejected"))
    }
}

fn decode_secret(encoded: &str) -> Result<Secret, ()> {
    if encoded.is_empty()
        || encoded.len() > MAX_ENCODED_SECRET_BYTES
        || encoded.bytes().any(|byte| byte == b'=')
    {
        return Err(());
    }
    let decoded_capacity = encoded
        .len()
        .checked_div(4)
        .and_then(|groups| groups.checked_mul(3))
        .and_then(|bytes| {
            bytes.checked_add(match encoded.len() % 4 {
                0 => 0,
                2 => 1,
                3 => 2,
                _ => return None,
            })
        })
        .ok_or(())?;
    if !(1..=MAX_SECRET_BYTES).contains(&decoded_capacity) {
        return Err(());
    }

    let mut decoded = Zeroizing::new(vec![0_u8; decoded_capacity]);
    let written = URL_SAFE_NO_PAD
        .decode_slice(encoded.as_bytes(), decoded.as_mut_slice())
        .map_err(|_| ())?;
    decoded.truncate(written);
    if decoded.is_empty() || decoded.len() > MAX_SECRET_BYTES {
        return Err(());
    }

    let mut canonical = Zeroizing::new(String::with_capacity(encoded.len()));
    URL_SAFE_NO_PAD.encode_string(decoded.as_slice(), &mut canonical);
    if canonical.as_str() != encoded {
        return Err(());
    }
    Ok(Secret(decoded))
}

pub(super) fn parse_vault(bytes: &[u8]) -> Result<VaultDocument<'_>, ConfigError> {
    std::str::from_utf8(bytes).map_err(|_| ConfigError::new(ConfigErrorKind::NonUtf8))?;
    if bytes.contains(&b'\\') {
        return Err(authority_error());
    }

    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let document = VaultDocument::deserialize(&mut deserializer).map_err(classify_json_error)?;
    deserializer.end().map_err(classify_json_error)?;
    validate_document(&document)?;
    Ok(document)
}

fn validate_document(document: &VaultDocument<'_>) -> Result<(), ConfigError> {
    if document.format_version != HOST_VAULT_FORMAT_VERSION
        || document.kind != HOST_VAULT_KIND
        || document.revision > i64::MAX as u64
    {
        return Err(authority_error());
    }

    let mut active = 0_usize;
    let mut revoked = 0_usize;
    for credential in &document.credentials {
        validate_local_id(credential.service_id)?;
        validate_local_id(credential.account_id)?;
        validate_local_id(credential.issuer_id)?;
        validate_local_id(credential.auth_schema_id)?;
        if credential.credential_version == 0 || credential.credential_version > i64::MAX as u64 {
            return Err(authority_error());
        }
        match (credential.state, credential.secret_base64_url.as_ref()) {
            (CredentialState::Active, Some(_)) => active += 1,
            (CredentialState::Revoked, None) => revoked += 1,
            _ => return Err(authority_error()),
        }
    }
    if active > MAX_ACTIVE_CREDENTIALS || revoked > MAX_REVOKED_CREDENTIALS {
        return Err(authority_error());
    }
    if document
        .credentials
        .windows(2)
        .any(|pair| credential_key(&pair[0]) >= credential_key(&pair[1]))
    {
        return Err(authority_error());
    }

    if document.grants.len() > MAX_GRANTS {
        return Err(authority_error());
    }
    for grant in &document.grants {
        validate_grant(grant)?;
        let references = document
            .credentials
            .iter()
            .filter(|credential| {
                credential.state == CredentialState::Active
                    && credential.service_id == grant.service_id
                    && credential.account_id == grant.account_id
            })
            .count();
        if references != 1 {
            return Err(authority_error());
        }
    }
    if document
        .grants
        .windows(2)
        .any(|pair| grant_key(&pair[0]) >= grant_key(&pair[1]))
    {
        return Err(authority_error());
    }
    Ok(())
}

fn validate_grant(grant: &Grant<'_>) -> Result<(), ConfigError> {
    validate_local_id(grant.operation_id)?;
    validate_local_id(grant.service_id)?;
    validate_local_id(grant.account_id)?;
    if grant.manager_id != grant.consumer_family.plugin_family().id()
        || !is_valid_portable_id(grant.pack_id)
        || !is_valid_sha256_digest(grant.authority_digest)
    {
        return Err(authority_error());
    }
    Ok(())
}

fn validate_local_id(value: &str) -> Result<(), ConfigError> {
    let bytes = value.as_bytes();
    if !(1..=128).contains(&bytes.len())
        || !bytes[0].is_ascii_lowercase()
        || !bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
        || matches!(bytes.last(), Some(b'.' | b'_' | b'-'))
        || bytes.windows(2).any(|pair| {
            matches!(pair[0], b'.' | b'_' | b'-') && matches!(pair[1], b'.' | b'_' | b'-')
        })
    {
        return Err(authority_error());
    }
    Ok(())
}

fn credential_key<'a>(credential: &'a Credential<'a>) -> (&'a [u8], &'a [u8]) {
    (
        credential.service_id.as_bytes(),
        credential.account_id.as_bytes(),
    )
}

fn grant_key<'a>(grant: &'a Grant<'a>) -> (&'a [u8], &'a [u8], &'a [u8], &'a [u8]) {
    (
        grant.consumer_family.sort_key().as_bytes(),
        grant.manager_id.as_bytes(),
        grant.pack_id.as_bytes(),
        grant.operation_id.as_bytes(),
    )
}

fn classify_json_error(error: serde_json::Error) -> ConfigError {
    if error.is_syntax() || error.is_eof() {
        ConfigError::new(ConfigErrorKind::InvalidJson)
    } else {
        authority_error()
    }
}

fn authority_error() -> ConfigError {
    ConfigError::new(ConfigErrorKind::AuthorityValidation)
}

#[cfg(test)]
pub(super) fn serialize_for_test(
    document: &VaultDocument<'_>,
) -> Result<Zeroizing<Vec<u8>>, ConfigError> {
    let mut bytes = Zeroizing::new(Vec::with_capacity(MAX_HOST_VAULT_BYTES));
    {
        let mut serializer = serde_json::Serializer::new(&mut *bytes);
        document
            .serialize(&mut serializer)
            .map_err(|_| ConfigError::new(ConfigErrorKind::Serialization))?;
    }
    bytes.push(b'\n');
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::decode_secret;

    #[test]
    fn secret_debug_is_redacted() {
        let secret = decode_secret("c2VudGluZWw").expect("secret");
        let rendered = format!("{secret:?}");
        assert_eq!(rendered, "Secret([REDACTED])");
        assert!(!rendered.contains("sentinel"));
    }
}
