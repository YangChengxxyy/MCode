//! Defines Host-generated staging transaction identifiers.
//!
//! Transaction identifiers contain 128 operating-system CSPRNG bits and have
//! one persistent lowercase hexadecimal spelling. This module intentionally
//! exposes generation and formatting but no public parsing or raw construction.

// Rust guideline compliant 2026-08-29

use std::fmt::{self, Display, Formatter};

use crate::{ConfigError, ConfigErrorKind};

const RANDOM_BYTES: usize = 16;
const TRANSACTION_ID_PREFIX: &str = "tx1-";
const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";

/// Identifies one Host-created staging transaction.
///
/// Values are generated from 128 operating-system CSPRNG bits and always use
/// the spelling `tx1-` followed by exactly 32 lowercase hexadecimal digits.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TransactionId(String);

impl TransactionId {
    /// Generates a transaction identifier from the operating-system CSPRNG.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigErrorKind::Random`] when the operating-system random
    /// source cannot fill the required 16-byte buffer.
    pub fn generate() -> Result<Self, ConfigError> {
        let mut random = [0_u8; RANDOM_BYTES];
        getrandom::fill(&mut random).map_err(|_| ConfigError::new(ConfigErrorKind::Random))?;

        let mut spelling = String::with_capacity(TRANSACTION_ID_PREFIX.len() + RANDOM_BYTES * 2);
        spelling.push_str(TRANSACTION_ID_PREFIX);
        for byte in random {
            spelling.push(char::from(LOWER_HEX[usize::from(byte >> 4)]));
            spelling.push(char::from(LOWER_HEX[usize::from(byte & 0x0f)]));
        }
        Ok(Self(spelling))
    }

    /// Returns the canonical persistent spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for TransactionId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
