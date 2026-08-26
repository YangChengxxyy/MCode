//! Declarative, terminal-independent plugin UI contract.
//!
//! Plugins submit validated view models. They cannot submit ANSI sequences,
//! terminal escape codes, draw callbacks, or raw terminal buffers.

// Rust guideline compliant 2026-08-26.

use serde::{Deserialize, Serialize};

use crate::events::EventKind;
use crate::ids::Identifier;
use crate::limits::MAX_UI_VIEW_BYTES;
use crate::validation::is_terminal_control;

const MIN_INVALIDATION_INTERVAL_MS: u64 = 16;
const MAX_INVALIDATION_INTERVAL_MS: u64 = 86_400_000;
const MAX_VIEW_ITEMS: usize = 256;
const MAX_VIEW_TEXT_BYTES: usize = 48 * 1024;
const MAX_TERMINAL_WIDTH: u16 = 4096;

/// Semantic view kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ViewKind {
    /// Compact status content.
    Status,
    /// Application header content.
    Header,
    /// Application footer content.
    Footer,
    /// A persistent or contextual panel.
    Panel,
    /// An item placed in the activity timeline.
    Timeline,
    /// A modal view interpreted by the host UI.
    Modal,
    /// A compact host-placed widget.
    Widget,
}

/// Host-defined placement region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UiRegion {
    /// Global application chrome.
    Global,
    /// Conversation or activity area.
    Timeline,
    /// Input/composer-adjacent area.
    Composer,
    /// Side panel area.
    Sidebar,
    /// Overlay layer.
    Overlay,
}

/// Width constraints interpreted by the host renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WidthConstraints {
    /// Minimum desired width in terminal cells or equivalent UI units.
    pub min: u16,
    /// Maximum desired width in terminal cells or equivalent UI units.
    pub max: u16,
}

/// When a host should invalidate a declarative view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase", deny_unknown_fields)]
pub enum Invalidation {
    /// Invalidate only when the plugin publishes a replacement.
    Manual,
    /// Invalidate when one of the selected redacted events arrives.
    OnEvents {
        /// Event kinds that invalidate the view.
        events: Vec<EventKind>,
    },
    /// Invalidate on a bounded host timer.
    Interval {
        /// Interval in milliseconds.
        interval_ms: u64,
    },
}

/// Metadata shared by every view kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewMetadata {
    /// Stable plugin-owned view id.
    pub id: Identifier,
    /// Host placement region.
    pub region: UiRegion,
    /// Ordering priority; higher values are host-defined but deterministic.
    pub priority: i16,
    /// Width constraints.
    pub width: WidthConstraints,
    /// Invalidation policy.
    pub invalidation: Invalidation,
}

/// Semantic text tone selected by a host theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TextTone {
    /// Normal text.
    Normal,
    /// Secondary text.
    Muted,
    /// Informational emphasis.
    Accent,
    /// Successful outcome.
    Success,
    /// Warning outcome.
    Warning,
    /// Error outcome.
    Error,
}

/// Declarative content rendered by a host adapter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum ViewContent {
    /// Plain text with semantic styling.
    Text {
        /// Text content; newlines and tabs are allowed.
        text: String,
        /// Host-theme tone.
        tone: TextTone,
        /// Whether the host should emphasize the text.
        emphasized: bool,
    },
    /// Markdown interpreted by a safe host renderer.
    Markdown {
        /// Markdown source without terminal control sequences.
        markdown: String,
    },
    /// A bounded list of text items.
    List {
        /// List item text.
        items: Vec<String>,
        /// Whether host ordering should be numbered.
        ordered: bool,
    },
    /// A bounded table.
    Table {
        /// Column headings.
        columns: Vec<String>,
        /// Rows whose width must match `columns`.
        rows: Vec<Vec<String>>,
    },
    /// Numeric progress.
    Progress {
        /// Human-readable label.
        label: String,
        /// Current progress.
        current: u64,
        /// Total progress; must be nonzero and at least `current`.
        total: u64,
    },
}

/// Complete declarative view submitted by a plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum UiView {
    /// Compact status content.
    Status {
        /// Shared metadata.
        metadata: ViewMetadata,
        /// Declarative content.
        content: ViewContent,
    },
    /// Application header content.
    Header {
        /// Shared metadata.
        metadata: ViewMetadata,
        /// Declarative content.
        content: ViewContent,
    },
    /// Application footer content.
    Footer {
        /// Shared metadata.
        metadata: ViewMetadata,
        /// Declarative content.
        content: ViewContent,
    },
    /// A persistent or contextual panel.
    Panel {
        /// Shared metadata.
        metadata: ViewMetadata,
        /// Declarative content.
        content: ViewContent,
    },
    /// An item in the activity timeline.
    Timeline {
        /// Shared metadata.
        metadata: ViewMetadata,
        /// Declarative content.
        content: ViewContent,
    },
    /// A host-rendered modal.
    Modal {
        /// Shared metadata.
        metadata: ViewMetadata,
        /// Declarative content.
        content: ViewContent,
    },
    /// A compact host-placed widget.
    Widget {
        /// Shared metadata.
        metadata: ViewMetadata,
        /// Declarative content.
        content: ViewContent,
    },
}

impl UiView {
    /// Returns this view's kind.
    #[must_use]
    pub fn kind(&self) -> ViewKind {
        match self {
            Self::Status { .. } => ViewKind::Status,
            Self::Header { .. } => ViewKind::Header,
            Self::Footer { .. } => ViewKind::Footer,
            Self::Panel { .. } => ViewKind::Panel,
            Self::Timeline { .. } => ViewKind::Timeline,
            Self::Modal { .. } => ViewKind::Modal,
            Self::Widget { .. } => ViewKind::Widget,
        }
    }

    /// Returns this view's shared metadata.
    #[must_use]
    pub fn metadata(&self) -> &ViewMetadata {
        match self {
            Self::Status { metadata, .. }
            | Self::Header { metadata, .. }
            | Self::Footer { metadata, .. }
            | Self::Panel { metadata, .. }
            | Self::Timeline { metadata, .. }
            | Self::Modal { metadata, .. }
            | Self::Widget { metadata, .. } => metadata,
        }
    }

    /// Returns this view's declarative content.
    #[must_use]
    pub fn content(&self) -> &ViewContent {
        match self {
            Self::Status { content, .. }
            | Self::Header { content, .. }
            | Self::Footer { content, .. }
            | Self::Panel { content, .. }
            | Self::Timeline { content, .. }
            | Self::Modal { content, .. }
            | Self::Widget { content, .. } => content,
        }
    }

    /// Validates layout bounds, content shape, and terminal safety.
    ///
    /// # Errors
    ///
    /// Returns [`UiValidationError`] for invalid width or invalidation bounds,
    /// oversized content, malformed tables/progress, or any raw terminal
    /// control sequence.
    pub fn validate(&self) -> Result<(), UiValidationError> {
        validate_metadata(self.metadata())?;
        validate_content(self.content())?;
        let serialized = serde_json::to_vec(self).map_err(|_| UiValidationError::Serialization)?;
        if serialized.len() > MAX_UI_VIEW_BYTES {
            return Err(UiValidationError::TooLarge);
        }
        Ok(())
    }
}

pub(crate) fn validate_metadata(metadata: &ViewMetadata) -> Result<(), UiValidationError> {
    if metadata.width.min == 0
        || metadata.width.min > metadata.width.max
        || metadata.width.max > MAX_TERMINAL_WIDTH
    {
        return Err(UiValidationError::InvalidWidth);
    }
    match &metadata.invalidation {
        Invalidation::Manual => {}
        Invalidation::OnEvents { events } => {
            if events.is_empty() || events.len() > 16 {
                return Err(UiValidationError::InvalidInvalidation);
            }
        }
        Invalidation::Interval { interval_ms }
            if !(MIN_INVALIDATION_INTERVAL_MS..=MAX_INVALIDATION_INTERVAL_MS)
                .contains(interval_ms) =>
        {
            return Err(UiValidationError::InvalidInvalidation);
        }
        Invalidation::Interval { .. } => {}
    }
    Ok(())
}

fn validate_content(content: &ViewContent) -> Result<(), UiValidationError> {
    match content {
        ViewContent::Text { text, .. } => validate_text(text),
        ViewContent::Markdown { markdown } => validate_text(markdown),
        ViewContent::List { items, .. } => {
            if items.len() > MAX_VIEW_ITEMS {
                return Err(UiValidationError::TooManyItems);
            }
            for item in items {
                validate_text(item)?;
            }
            Ok(())
        }
        ViewContent::Table { columns, rows } => {
            if columns.is_empty()
                || columns.len() > MAX_VIEW_ITEMS
                || rows.len() > MAX_VIEW_ITEMS
                || rows.iter().any(|row| row.len() != columns.len())
            {
                return Err(UiValidationError::InvalidTable);
            }
            for value in columns.iter().chain(rows.iter().flatten()) {
                validate_text(value)?;
            }
            Ok(())
        }
        ViewContent::Progress {
            label,
            current,
            total,
        } => {
            validate_text(label)?;
            if *total == 0 || current > total {
                return Err(UiValidationError::InvalidProgress);
            }
            Ok(())
        }
    }
}

fn validate_text(value: &str) -> Result<(), UiValidationError> {
    if value.len() > MAX_VIEW_TEXT_BYTES {
        return Err(UiValidationError::TooLarge);
    }
    if value.chars().any(is_terminal_control) {
        return Err(UiValidationError::TerminalControl);
    }
    Ok(())
}

/// Declarative UI validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UiValidationError {
    /// Width bounds were inconsistent or unreasonable.
    #[error("plugin view width constraints are invalid")]
    InvalidWidth,
    /// Invalidation policy was empty or outside timer bounds.
    #[error("plugin view invalidation policy is invalid")]
    InvalidInvalidation,
    /// Content exceeded a byte limit.
    #[error("plugin view exceeds its size limit")]
    TooLarge,
    /// A list or table exceeded its item limit.
    #[error("plugin view has too many items")]
    TooManyItems,
    /// Table rows did not match the declared columns.
    #[error("plugin view table shape is invalid")]
    InvalidTable,
    /// Progress bounds were inconsistent.
    #[error("plugin view progress bounds are invalid")]
    InvalidProgress,
    /// Content contained ANSI or another raw terminal control.
    #[error("plugin view contains a forbidden terminal control")]
    TerminalControl,
    /// The bounded view could not be serialized.
    #[error("plugin view could not be serialized")]
    Serialization,
}

#[cfg(test)]
mod tests {
    use super::{
        Invalidation, TextTone, UiRegion, UiValidationError, UiView, ViewContent, ViewMetadata,
        WidthConstraints,
    };
    use crate::ids::Identifier;

    fn metadata() -> ViewMetadata {
        ViewMetadata {
            id: Identifier::parse("status.main").expect("id"),
            region: UiRegion::Global,
            priority: 0,
            width: WidthConstraints { min: 1, max: 80 },
            invalidation: Invalidation::Manual,
        }
    }

    #[test]
    fn widget_rejects_ansi_and_invalid_width() {
        let ansi = UiView::Widget {
            metadata: metadata(),
            content: ViewContent::Text {
                text: "\u{1b}[31mred".into(),
                tone: TextTone::Error,
                emphasized: false,
            },
        };
        assert_eq!(ansi.validate(), Err(UiValidationError::TerminalControl));

        let mut bad_metadata = metadata();
        bad_metadata.width = WidthConstraints { min: 90, max: 80 };
        let bad_width = UiView::Widget {
            metadata: bad_metadata,
            content: ViewContent::Text {
                text: "safe".into(),
                tone: TextTone::Normal,
                emphasized: false,
            },
        };
        assert_eq!(bad_width.validate(), Err(UiValidationError::InvalidWidth));
    }
}
