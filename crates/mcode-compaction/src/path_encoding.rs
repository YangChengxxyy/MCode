//! Exact, tagged JSON persistence for host path values.
//!
//! Derived serde `PathBuf` serialization rejects Unix paths that are not valid
//! UTF-8, and a lossy `to_string_lossy` fallback would collide distinct paths.
//! This module therefore persists every path as a tagged exact representation:
//! readable UTF-8 text when possible, otherwise the raw Unix bytes or the raw
//! Windows UTF-16 code units, both base64 encoded. Round trips are exact and
//! the emitted JSON stays valid, schema-versioned data.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::{Deserialize, Serialize};

/// Tagged exact representation of one host path.
///
/// The tags keep UTF-8, raw Unix byte, and raw Windows UTF-16 paths distinct so
/// no two different paths can serialize to the same JSON value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "encoding", rename_all = "snake_case")]
enum PathRepr {
    /// Exact UTF-8 text; the readable form used whenever the path is valid UTF-8.
    Utf8 { value: String },
    /// Raw Unix OS bytes encoded as RFC 4648 base64; never produced for UTF-8 paths.
    UnixBytes { base64: String },
    /// Raw Windows UTF-16 code units (possibly containing unpaired surrogates)
    /// encoded as base64 of the little-endian unit sequence; never produced for
    /// UTF-8-representable paths.
    WindowsUtf16 { base64: String },
}

impl PathRepr {
    /// Converts a path into its exact tagged representation.
    fn of(path: &Path) -> Self {
        match path.to_str() {
            Some(value) => Self::Utf8 {
                value: value.to_owned(),
            },
            None => Self::of_non_utf8(path),
        }
    }

    /// Encodes a path that is not valid UTF-8 using its platform-native form.
    #[cfg(unix)]
    fn of_non_utf8(path: &Path) -> Self {
        use std::os::unix::ffi::OsStrExt;
        Self::UnixBytes {
            base64: base64_encode(path.as_os_str().as_bytes()),
        }
    }

    /// Encodes a path with unpaired surrogates as exact UTF-16 code units.
    #[cfg(windows)]
    fn of_non_utf8(path: &Path) -> Self {
        use std::os::windows::ffi::OsStrExt;
        let units: Vec<u8> = path
            .as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect();
        Self::WindowsUtf16 {
            base64: base64_encode(&units),
        }
    }

    /// Other platforms cannot expose non-UTF-8 paths exactly; the lossy form
    /// is only reached there and is documented as unsupported.
    #[cfg(not(any(unix, windows)))]
    fn of_non_utf8(path: &Path) -> Self {
        Self::Utf8 {
            value: path.to_string_lossy().into_owned(),
        }
    }

    /// Rebuilds the exact path from its tagged representation.
    ///
    /// # Errors
    ///
    /// Returns a message when the base64 payload is malformed, the UTF-16
    /// payload has an odd byte length, or the raw form is not representable on
    /// the current platform.
    fn into_path(self) -> Result<PathBuf, String> {
        match self {
            Self::Utf8 { value } => Ok(PathBuf::from(value)),
            Self::UnixBytes { base64 } => {
                let bytes = base64_decode(&base64)?;
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStringExt;
                    Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
                }
                #[cfg(not(unix))]
                {
                    let _ = bytes;
                    Err("unix_bytes path encoding is not representable on this platform".into())
                }
            }
            Self::WindowsUtf16 { base64 } => {
                let bytes = base64_decode(&base64)?;
                if bytes.len() % 2 != 0 {
                    return Err("windows_utf16 path payload must contain whole code units".into());
                }
                let units: Vec<u16> = bytes
                    .chunks_exact(2)
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect();
                #[cfg(windows)]
                {
                    use std::os::windows::ffi::OsStringExt;
                    Ok(PathBuf::from(std::ffi::OsString::from_wide(&units)))
                }
                #[cfg(not(windows))]
                {
                    let _ = units;
                    Err("windows_utf16 path encoding is not representable on this platform".into())
                }
            }
        }
    }
}

/// Serde bridge for `Vec<PathBuf>` fields inside persisted compaction values.
pub(crate) mod path_vec {
    use super::PathRepr;
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
    use std::path::PathBuf;

    /// Serializes paths as a list of tagged exact representations.
    pub(crate) fn serialize<S>(paths: &[PathBuf], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let reprs: Vec<PathRepr> = paths.iter().map(|path| PathRepr::of(path)).collect();
        reprs.serialize(serializer)
    }

    /// Deserializes tagged exact representations back into exact paths.
    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Vec<PathBuf>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let reprs = Vec::<PathRepr>::deserialize(deserializer)?;
        reprs
            .into_iter()
            .map(|repr| {
                PathRepr::into_path(repr)
                    .map_err(|message| Error::custom(format!("invalid persisted path: {message}")))
            })
            .collect()
    }
}

/// Encodes bytes as padded RFC 4648 base64 through the workspace engine.
fn base64_encode(input: &[u8]) -> String {
    BASE64_STANDARD.encode(input)
}

/// Decodes padded RFC 4648 base64 through the workspace engine, rejecting
/// malformed input with a stable message.
fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    BASE64_STANDARD
        .decode(input)
        .map_err(|error| format!("invalid base64 payload: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_test_vectors() {
        for (plain, encoded) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"fooba", "Zm9vYmE="),
            (b"foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(plain), encoded);
            assert_eq!(base64_decode(encoded).as_deref(), Ok(plain));
        }
    }

    #[test]
    fn base64_decode_rejects_malformed_payloads() {
        assert!(base64_decode("Zm9v!").is_err());
        assert!(base64_decode("Zm9").is_err());
        assert!(base64_decode("Zg=").is_err());
        assert!(base64_decode("Zg==Zm9v").is_err());
        assert!(base64_decode("====").is_err());
    }

    #[test]
    fn utf8_paths_roundtrip_through_readable_json() {
        let path = PathBuf::from("src/compaction/lib.rs");
        let repr = PathRepr::of(&path);
        assert_eq!(
            repr,
            PathRepr::Utf8 {
                value: "src/compaction/lib.rs".to_owned()
            }
        );
        let json = serde_json::to_string(&repr).unwrap();
        assert!(json.contains("\"encoding\":\"utf8\""));
        assert!(json.contains("src/compaction/lib.rs"));
        let parsed: PathRepr = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.into_path().unwrap(), path);
    }

    #[cfg(unix)]
    #[test]
    fn unix_raw_bytes_roundtrip_exactly_without_lossy_collisions() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let lossy_a = PathBuf::from(OsString::from_vec(vec![b'n', 0x80]));
        let lossy_b = PathBuf::from(OsString::from_vec(vec![b'n', 0x81]));
        assert_eq!(lossy_a.to_string_lossy(), lossy_b.to_string_lossy());
        for path in [&lossy_a, &lossy_b] {
            let repr = PathRepr::of(path);
            assert!(matches!(repr, PathRepr::UnixBytes { .. }), "{repr:?}");
            assert_eq!(repr.into_path().unwrap(), *path);
            let json = serde_json::to_string(&repr).unwrap();
            let parsed: PathRepr = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed.into_path().unwrap(), *path);
        }
        let json_a = serde_json::to_string(&PathRepr::of(&lossy_a)).unwrap();
        let json_b = serde_json::to_string(&PathRepr::of(&lossy_b)).unwrap();
        assert_ne!(json_a, json_b);
    }

    #[cfg(windows)]
    #[test]
    fn windows_unpaired_surrogates_roundtrip_exactly_without_lossy_collisions() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;

        let surrogate_a = PathBuf::from(OsString::from_wide(&[b'a' as u16, 0xD800, b'b' as u16]));
        let surrogate_b = PathBuf::from(OsString::from_wide(&[b'a' as u16, 0xDFFF, b'b' as u16]));
        assert_eq!(surrogate_a.to_string_lossy(), surrogate_b.to_string_lossy());
        for path in [&surrogate_a, &surrogate_b] {
            assert!(path.to_str().is_none());
            let repr = PathRepr::of(path);
            assert!(matches!(repr, PathRepr::WindowsUtf16 { .. }), "{repr:?}");
            assert_eq!(repr.clone().into_path().unwrap(), *path);
            let json = serde_json::to_string(&repr).unwrap();
            let parsed: PathRepr = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed.into_path().unwrap(), *path);
        }
        let json_a = serde_json::to_string(&PathRepr::of(&surrogate_a)).unwrap();
        let json_b = serde_json::to_string(&PathRepr::of(&surrogate_b)).unwrap();
        assert_ne!(json_a, json_b);
    }
}

// Rust guideline compliant 2026-08-26.
