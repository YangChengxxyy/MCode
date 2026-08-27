//! Pure consent and status surfaces with no permission engine.
//!
//! The TUI stores a display-only prompt and emits [`crate::Effect`] values
//! when the user answers. Readability is a viewport check: an unreadable
//! notice cannot be accepted and fail-closes to deny.

// Rust guideline compliant 2026-08-27.

use std::fmt;

use crate::state::Viewport;

/// Minimum columns that can present a consent notice.
///
/// Narrower views cannot keep the four choices on readable rows, so the
/// reducer fail-closes to deny instead of accepting an unseen prompt.
pub const CONSENT_MIN_COLUMNS: u16 = 24;

/// Rows occupied by the input panel in the consent layout contract.
///
/// Matches `render.rs`: bordered single-line input is 3 rows.
pub const CONSENT_INPUT_ROWS: u16 = 3;
/// Rows occupied by the status bar when the viewport is non-empty.
pub const CONSENT_STATUS_ROWS: u16 = 1;
/// Rows occupied by the consent panel border.
pub const CONSENT_BORDER_ROWS: u16 = 2;
/// Inner rows required to show title, one body line, and four choices.
pub const CONSENT_MIN_INNER_ROWS: u16 = 6;

/// Minimum rows that can present a consent notice.
///
/// Layout chrome is input (3) + status (1) + border (2) + inner (6).
/// Logo is omitted while consent is visible so this budget matches draw.
pub const CONSENT_MIN_ROWS: u16 =
    CONSENT_INPUT_ROWS + CONSENT_STATUS_ROWS + CONSENT_BORDER_ROWS + CONSENT_MIN_INNER_ROWS;

/// Maximum display columns used for the consent title.
pub const CONSENT_MAX_TITLE_COLUMNS: usize = 78;

/// Maximum display columns used for the consent body.
pub const CONSENT_MAX_BODY_COLUMNS: usize = 76;

/// Maximum body lines shown in the consent panel.
pub const CONSENT_MAX_BODY_LINES: usize = 12;

/// User-facing consent notice held as pure view state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentPrompt {
    request_id: String,
    tool_name: String,
    summary: String,
}

impl ConsentPrompt {
    /// Creates a display-only consent prompt.
    ///
    /// `request_id` is an opaque host token echoed in
    /// [`crate::Effect::ConsentResolved`]. This type does not evaluate
    /// permission rules.
    pub fn new(
        request_id: impl Into<String>,
        tool_name: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            tool_name: tool_name.into(),
            summary: summary.into(),
        }
    }

    /// Returns the host request identifier.
    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// Returns the tool name shown in the title.
    #[must_use]
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }

    /// Returns the unwrapped summary body.
    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }
}

/// Choice produced by the consent surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ConsentChoice {
    /// Allow this one invocation.
    AllowOnce,
    /// Allow remaining invocations in this session.
    AllowSession,
    /// Persist an allow rule (host-owned).
    AlwaysAllow,
    /// Deny the invocation.
    Deny,
}

impl ConsentChoice {
    /// Returns whether this choice would grant access.
    #[must_use]
    pub const fn allows(self) -> bool {
        !matches!(self, Self::Deny)
    }
}

impl fmt::Display for ConsentChoice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AllowOnce => "allow once",
            Self::AllowSession => "allow for this session",
            Self::AlwaysAllow => "always allow",
            Self::Deny => "deny",
        })
    }
}

/// Status-bar fields supplied by the host as plain data.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatusSurface {
    message: String,
}

impl StatusSurface {
    /// Creates a status surface from `message`.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Returns the status message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Returns the body-panel height used when consent is visible.
///
/// Logo is given zero rows so the notice receives the space `is_readable`
/// reserved for it. Keep this in lock-step with `render.rs`.
#[must_use]
pub const fn consent_body_height(viewport: Viewport) -> u16 {
    let status = if viewport.height > 0 {
        CONSENT_STATUS_ROWS
    } else {
        0
    };
    let input = if viewport.height >= 4 {
        let budget = viewport.height.saturating_sub(status.saturating_add(1));
        if CONSENT_INPUT_ROWS < budget {
            CONSENT_INPUT_ROWS
        } else {
            budget
        }
    } else {
        viewport.height.saturating_sub(status)
    };
    viewport.height.saturating_sub(status.saturating_add(input))
}

/// Returns whether `viewport` can present a consent notice.
#[must_use]
pub const fn is_readable(viewport: Viewport) -> bool {
    if viewport.width < CONSENT_MIN_COLUMNS {
        return false;
    }
    consent_body_height(viewport).saturating_sub(CONSENT_BORDER_ROWS) >= CONSENT_MIN_INNER_ROWS
}
