//! Bounded task identities, generation, and stable failure codes.

// Rust guideline compliant 2026-08-29.

use std::fmt::{self, Display, Formatter};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

/// Minimum encoded length of a canonical operation key.
pub const MIN_OPERATION_ID_BYTES: usize = 1;
/// Maximum encoded length of a canonical operation key.
pub const MAX_OPERATION_ID_BYTES: usize = 128;
/// Exact encoded length of a task ID.
pub const TASK_ID_BYTES: usize = 38;
/// Largest generation exactly representable by common JSON number consumers.
pub const MAX_TASK_GENERATION: u64 = 9_007_199_254_740_991;

const TASK_PREFIX: &str = "task1-";

/// Identifies one declarative Manager operation key.
///
/// Values use the same canonical grammar as Host-vault `operationId`: the
/// first byte is lowercase ASCII; remaining bytes are lowercase ASCII,
/// digits, or `.`, `_`, and `-`; separators are neither trailing nor adjacent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct OperationId(String);

impl OperationId {
    /// Parses one canonical operation ID.
    ///
    /// # Errors
    ///
    /// Returns [`TaskIdentityError::InvalidOperationId`] for any non-canonical
    /// value.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, TaskIdentityError> {
        let value = value.as_ref();
        if !is_valid_operation_id(value) {
            return Err(TaskIdentityError::InvalidOperationId);
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the canonical operation ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for OperationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(|_| D::Error::custom("invalid operation ID"))
    }
}

impl Display for OperationId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Returns whether `value` is a canonical declarative operation key.
///
/// This validator performs no allocation and is the shared production
/// authority for task wire and Host-vault `operationId` coordinates.
#[must_use]
pub fn is_valid_operation_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    (MIN_OPERATION_ID_BYTES..=MAX_OPERATION_ID_BYTES).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || is_operation_separator(*byte)
        })
        && !bytes
            .last()
            .is_some_and(|byte| is_operation_separator(*byte))
        && !bytes
            .windows(2)
            .any(|pair| is_operation_separator(pair[0]) && is_operation_separator(pair[1]))
}

/// Identifies one Host-issued task.
///
/// Values are exactly `task1-` followed by 32 lowercase hexadecimal digits.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct TaskId(String);

impl TaskId {
    /// Parses one canonical task ID.
    ///
    /// # Errors
    ///
    /// Returns [`TaskIdentityError::InvalidTaskId`] for any non-canonical
    /// value.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, TaskIdentityError> {
        let value = value.as_ref();
        if value.len() != TASK_ID_BYTES
            || !value.starts_with(TASK_PREFIX)
            || !value[TASK_PREFIX.len()..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(TaskIdentityError::InvalidTaskId);
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the canonical task ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for TaskId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(|_| D::Error::custom("invalid task ID"))
    }
}

impl Display for TaskId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Identifies one active Manager generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct TaskGeneration(u64);

impl TaskGeneration {
    /// Creates a bounded nonzero generation.
    ///
    /// # Errors
    ///
    /// Returns [`TaskIdentityError::InvalidGeneration`] for zero or a value
    /// greater than [`MAX_TASK_GENERATION`].
    pub const fn new(value: u64) -> Result<Self, TaskIdentityError> {
        if value == 0 || value > MAX_TASK_GENERATION {
            return Err(TaskIdentityError::InvalidGeneration);
        }
        Ok(Self(value))
    }

    /// Returns the numeric generation.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for TaskGeneration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        Self::new(value).map_err(|_| D::Error::custom("invalid task generation"))
    }
}

/// Reports canonical task-identity validation failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TaskIdentityError {
    /// An operation ID violated its exact grammar.
    #[error("operation ID is invalid")]
    InvalidOperationId,
    /// A task ID violated its exact grammar.
    #[error("task ID is invalid")]
    InvalidTaskId,
    /// A generation was zero or exceeded its bound.
    #[error("task generation is invalid")]
    InvalidGeneration,
}

/// Stable non-sensitive FeatureService task failure code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskErrorCode {
    /// The request did not satisfy its family-specific contract.
    InvalidRequest,
    /// The caller identity did not match the selected family.
    CallerMismatch,
    /// The requested operation was not declared by the active Manager.
    UndeclaredOperation,
    /// The request generation was not active.
    StaleGeneration,
    /// The task ID was not active for the bound caller.
    UnknownTask,
    /// The bounded task capacity was exhausted.
    TaskLimitReached,
    /// The task was cancelled.
    Cancelled,
    /// The selected feature was unavailable.
    FeatureUnavailable,
    /// The operation failed without exposing implementation details.
    Failed,
}

/// Contains one stable task failure without untrusted text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskFailure {
    code: TaskErrorCode,
}

impl TaskFailure {
    /// Creates a task failure from a stable code.
    #[must_use]
    pub const fn new(code: TaskErrorCode) -> Self {
        Self { code }
    }

    /// Returns the stable failure code.
    #[must_use]
    pub const fn code(self) -> TaskErrorCode {
        self.code
    }
}

fn is_operation_separator(byte: u8) -> bool {
    matches!(byte, b'.' | b'_' | b'-')
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_OPERATION_ID_BYTES, MAX_TASK_GENERATION, OperationId, TaskGeneration, TaskId,
        is_valid_operation_id,
    };

    #[test]
    fn operation_keys_enforce_the_host_vault_grammar() {
        for valid in ["read", "read.item", "read_item", "read-item", "r1"] {
            assert!(is_valid_operation_id(valid), "{valid}");
            assert_eq!(
                OperationId::parse(valid).expect("operation key").as_str(),
                valid
            );
        }

        let maximum = "a".repeat(MAX_OPERATION_ID_BYTES);
        assert!(is_valid_operation_id(&maximum));
        OperationId::parse(&maximum).expect("maximum operation key");

        for invalid in [
            "",
            "Read",
            "1read",
            ".read",
            "read.",
            "read..item",
            "read-_item",
            "read/item",
            "r\u{e9}ad",
        ] {
            assert!(!is_valid_operation_id(invalid), "{invalid}");
            assert!(OperationId::parse(invalid).is_err(), "{invalid}");
        }
        let oversized = "a".repeat(MAX_OPERATION_ID_BYTES + 1);
        assert!(!is_valid_operation_id(&oversized));
        assert!(OperationId::parse(oversized).is_err());
    }

    #[test]
    fn task_ids_enforce_the_host_issued_nonce_grammar() {
        TaskId::parse("task1-0123456789abcdef0123456789abcdef").expect("task ID");

        for invalid in [
            "task1-0123456789abcdef0123456789abcde",
            "task1-0123456789abcdef0123456789abcdeF",
            "read",
        ] {
            assert!(TaskId::parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn task_generation_uses_the_exact_json_safe_bound() {
        assert!(TaskGeneration::new(0).is_err());
        assert_eq!(
            TaskGeneration::new(MAX_TASK_GENERATION)
                .expect("maximum generation")
                .get(),
            MAX_TASK_GENERATION
        );
        assert!(TaskGeneration::new(MAX_TASK_GENERATION + 1).is_err());
    }
}
