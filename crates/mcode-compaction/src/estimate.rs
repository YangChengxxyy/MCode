//! Conservative token estimation without a provider tokenizer callback.
//!
//! The host may attach trusted per-message counts to [`CompactionMessage`].
//! Missing counts use the concrete estimator below; no implementation trait is
//! exported, so this cannot become a plugin strategy surface.

use mcode_core::{ContentBlock, Message};
use serde::{Deserialize, Serialize};

use crate::types::{COMPACTION_SCHEMA_VERSION, CompactionMessage};

/// Message framing allowance for provider role and JSON overhead.
const MESSAGE_OVERHEAD_TOKENS: u64 = 8;
/// Structured tool-call framing allowance beyond its string fields.
const TOOL_CALL_OVERHEAD_TOKENS: u64 = 12;
/// Unknown image tokenization needs a non-zero provider framing allowance.
const IMAGE_OVERHEAD_TOKENS: u64 = 256;
/// Any tokenizer covers at least one byte of input per generated token, so the
/// UTF-8 length of the text is a provable token upper bound for every provider
/// tokenizer, including byte-fallback encoders that may spend up to four
/// tokens on a single four-byte Unicode scalar.
const BYTES_PER_TOKEN_UPPER_BOUND: u64 = 1;

/// Concrete conservative fallback for unknown provider tokenizers.
///
/// This type has no configurable algorithm and no callback. A trusted host
/// with provider-native counts supplies them on [`CompactionMessage`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenEstimator {
    schema_version: u32,
}

impl TokenEstimator {
    /// Creates the built-in conservative estimator.
    pub const fn conservative() -> Self {
        Self {
            schema_version: COMPACTION_SCHEMA_VERSION,
        }
    }

    /// Estimates plain text with a provably conservative upper bound.
    ///
    /// The estimate equals the UTF-8 byte length of the text. Every real
    /// tokenizer maps each emitted token to at least one input byte, so no
    /// provider tokenizer can exceed this bound for the text itself; byte
    /// fallback encoders that spend one token per byte are fully covered.
    /// Multi-byte scalars (emoji, CJK, combining marks) are therefore counted
    /// at up to their full byte width instead of a byte-average heuristic.
    pub fn estimate_text(self, text: &str) -> u64 {
        usize_to_u64(text.len()).div_ceil(BYTES_PER_TOKEN_UPPER_BOUND)
    }

    /// Estimates one complete model-visible MCode message.
    ///
    /// Thinking replay payloads are counted because Responses and Anthropic
    /// adapters resend `replay.data` on later turns; the visible summary can
    /// be far smaller than the encrypted reasoning item.
    /// [`Message::Custom`] returns zero because provider adapters exclude
    /// plugin-persisted state from LLM context.
    pub fn estimate_message(self, message: &Message) -> u64 {
        let content = match message {
            Message::User(user) => self.estimate_blocks(&user.content),
            Message::Assistant(assistant) => self.estimate_blocks(&assistant.blocks),
            Message::ToolResult(result) => self
                .estimate_text(&result.tool_call_id)
                .saturating_add(self.estimate_blocks(&result.content)),
            Message::Custom(_) => return 0,
        };
        MESSAGE_OVERHEAD_TOKENS.saturating_add(content)
    }

    /// Estimates complete messages with saturating arithmetic.
    pub fn estimate_messages<'a>(self, messages: impl IntoIterator<Item = &'a Message>) -> u64 {
        messages.into_iter().fold(0_u64, |total, message| {
            total.saturating_add(self.estimate_message(message))
        })
    }

    fn estimate_blocks(self, blocks: &[ContentBlock]) -> u64 {
        blocks.iter().fold(0_u64, |total, block| {
            total.saturating_add(match block {
                ContentBlock::Text(text) => self.estimate_text(&text.text),
                ContentBlock::Thinking(thinking) => {
                    let text = self.estimate_text(&thinking.text);
                    let replay = thinking
                        .replay
                        .as_ref()
                        .map_or(0, |state| self.estimate_text(&state.data));
                    text.saturating_add(replay)
                }
                ContentBlock::ToolCall(call) => TOOL_CALL_OVERHEAD_TOKENS
                    .saturating_add(self.estimate_text(&call.id))
                    .saturating_add(
                        call.item_id
                            .as_deref()
                            .map_or(0, |item_id| self.estimate_text(item_id)),
                    )
                    .saturating_add(self.estimate_text(&call.name))
                    .saturating_add(self.estimate_text(&call.arguments.to_string())),
                ContentBlock::Image(image) => IMAGE_OVERHEAD_TOKENS
                    .saturating_add(self.estimate_text(&image.mime_type))
                    .saturating_add(self.estimate_text(&image.data)),
            })
        })
    }
}

impl Default for TokenEstimator {
    fn default() -> Self {
        Self::conservative()
    }
}

pub(crate) fn estimate_source_message(message: &CompactionMessage) -> u64 {
    if matches!(&message.message, Message::Custom(_)) {
        return 0;
    }
    message
        .token_count
        .unwrap_or_else(|| TokenEstimator::conservative().estimate_message(&message.message))
}

pub(crate) fn estimate_partial_message(source: &CompactionMessage, partial: &Message) -> u64 {
    let estimator = TokenEstimator::conservative();
    let partial_fallback = estimator.estimate_message(partial);
    let Some(source_tokens) = source.token_count else {
        return partial_fallback;
    };
    let source_fallback = estimator.estimate_message(&source.message).max(1);
    scale_tokens(source_tokens, partial_fallback, source_fallback)
}

pub(crate) fn estimate_previous_summary(summary: Option<&str>) -> u64 {
    summary.map_or(0, |summary| {
        MESSAGE_OVERHEAD_TOKENS
            .saturating_add(TokenEstimator::conservative().estimate_text(summary))
    })
}

pub(crate) fn estimate_summary_message(summary_message: &Message) -> u64 {
    TokenEstimator::conservative().estimate_message(summary_message)
}

fn scale_tokens(total: u64, part: u64, whole: u64) -> u64 {
    if part == 0 || total == 0 {
        return 0;
    }
    let numerator = u128::from(total).saturating_mul(u128::from(part));
    let scaled = numerator.div_ceil(u128::from(whole));
    u64::try_from(scaled).unwrap_or(u64::MAX).min(total)
}

pub(crate) fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcode_core::{
        AssistantMessage, CustomMessage, ReplayState, ReplayWire, StopReason, ThinkingBlock,
        UserMessage,
    };

    #[test]
    fn byte_bound_is_conservative_for_multibyte_and_ascii_text() {
        let estimator = TokenEstimator::conservative();
        // A four-byte emoji can cost four tokens under a byte-fallback tokenizer.
        assert_eq!(estimator.estimate_text("😀"), 4);
        // CJK scalars are three UTF-8 bytes each.
        assert_eq!(estimator.estimate_text("你好"), 6);
        // Combining marks still count their full byte width.
        assert_eq!(estimator.estimate_text("e\u{0301}"), 3);
        // ASCII code counts one byte per potential token.
        assert_eq!(estimator.estimate_text("abcdef"), 6);
        // Mixed content sums every byte.
        let mixed = "a你b\u{0301}c😀";
        assert_eq!(estimator.estimate_text(mixed), mixed.len() as u64);
        assert!(mixed.chars().count() < mixed.len());
    }

    #[test]
    fn byte_bound_dominates_the_old_char_average_heuristic() {
        let estimator = TokenEstimator::conservative();
        let emoji_only = "😀".repeat(1_000);
        // The old max(chars, ceil(bytes/3)) formula under-counted this input
        // by up to 2,000 tokens for byte-fallback tokenizers.
        let old_estimate =
            (emoji_only.chars().count() as u64).max(emoji_only.len().div_ceil(3) as u64);
        assert_eq!(old_estimate, 1_334);
        assert_eq!(estimator.estimate_text(&emoji_only), 4_000);
    }

    #[test]
    fn message_estimates_include_overhead_and_exclude_custom_state() {
        let estimator = TokenEstimator::conservative();
        let user = Message::User(UserMessage::text("你好"));
        assert_eq!(
            estimator.estimate_message(&user),
            MESSAGE_OVERHEAD_TOKENS + 6
        );
        let custom = Message::Custom(CustomMessage {
            kind: "plugin:state".to_owned(),
            data: serde_json::json!({"bytes": "你好"}),
        });
        assert_eq!(estimator.estimate_message(&custom), 0);
    }

    #[test]
    fn thinking_estimate_includes_opaque_replay_payload() {
        let estimator = TokenEstimator::conservative();
        let summary = "short";
        let encrypted = "opaque-encrypted-reasoning-payload-xxxxxxxx";
        let thinking = ThinkingBlock::new(summary)
            .with_replay(ReplayState::new(ReplayWire::OpenAiResponses, encrypted));
        let message = Message::Assistant(AssistantMessage {
            blocks: vec![ContentBlock::Thinking(thinking)],
            usage: None,
            stop_reason: StopReason::Stop,
        });
        let expected = MESSAGE_OVERHEAD_TOKENS
            .saturating_add(estimator.estimate_text(summary))
            .saturating_add(estimator.estimate_text(encrypted));
        assert_eq!(estimator.estimate_message(&message), expected);
        assert!(estimator.estimate_text(encrypted) > estimator.estimate_text(summary));
    }
}

// Rust guideline compliant 2026-08-26.
