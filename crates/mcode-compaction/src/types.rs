//! Versioned data exchanged across the host compaction boundary.
//!
//! Fields are intentionally private outside this crate. The MCode host uses
//! constructors and accessors, while serde remains available for a future
//! append-only session entry. These types are not a plugin contract.

use std::error::Error as StdError;
use std::fmt;
use std::path::PathBuf;

use mcode_core::{Message, MessageId, UserMessage};
use mcode_llm::{LlmError, ModelId};
use serde::{Deserialize, Serialize};

/// Current schema version for persisted compaction values.
pub const COMPACTION_SCHEMA_VERSION: u32 = 1;

const DEFAULT_RESERVE_TOKENS: u64 = 16_384;
const DEFAULT_MAX_SUMMARY_TOKENS: u64 = 4_096;
const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// Keeps one persisted summary below 256 KiB at worst-case UTF-8 width.
pub(crate) const MAX_SUMMARY_CHARS: usize = 65_536;
/// Prevents one compaction operation from calling its provider more than three times.
pub(crate) const MAX_PROVIDER_ATTEMPTS: u32 = 3;

/// Trusted host policy for one compaction operation.
///
/// The policy has no callback, strategy, hook, or provider-selection field.
/// `keep_recent_tokens == None` selects `min(20_000, context_window / 4)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionPolicy {
    pub(crate) schema_version: u32,
    pub(crate) reserve_tokens: u64,
    pub(crate) keep_recent_tokens: Option<u64>,
    pub(crate) max_summary_tokens: u64,
    pub(crate) max_attempts: u32,
}

impl CompactionPolicy {
    /// Creates the host policy with conservative defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Uses `reserve_tokens` as non-history context and generation headroom.
    #[must_use]
    pub fn with_reserve_tokens(mut self, reserve_tokens: u64) -> Self {
        self.reserve_tokens = reserve_tokens;
        self
    }

    /// Keeps approximately this many recent message tokens verbatim.
    #[must_use]
    pub fn with_keep_recent_tokens(mut self, keep_recent_tokens: u64) -> Self {
        self.keep_recent_tokens = Some(keep_recent_tokens);
        self
    }

    /// Restores the context-window-derived recent-token default.
    #[must_use]
    pub fn with_default_keep_recent_tokens(mut self) -> Self {
        self.keep_recent_tokens = None;
        self
    }

    /// Caps accepted summary text at `max_summary_tokens`.
    #[must_use]
    pub fn with_max_summary_tokens(mut self, max_summary_tokens: u64) -> Self {
        self.max_summary_tokens = max_summary_tokens;
        self
    }

    /// Limits total provider attempts to at most three, including the first.
    #[must_use]
    pub fn with_max_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = max_attempts;
        self
    }

    /// Returns the serialized schema version.
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Returns reserved context and generation headroom.
    pub fn reserve_tokens(&self) -> u64 {
        self.reserve_tokens
    }

    /// Returns an explicit recent-token target, if configured.
    pub fn keep_recent_tokens(&self) -> Option<u64> {
        self.keep_recent_tokens
    }

    /// Returns the maximum accepted summary size.
    pub fn max_summary_tokens(&self) -> u64 {
        self.max_summary_tokens
    }

    /// Returns the maximum number of provider attempts, which cannot exceed three.
    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }
}

impl Default for CompactionPolicy {
    fn default() -> Self {
        Self {
            schema_version: COMPACTION_SCHEMA_VERSION,
            reserve_tokens: DEFAULT_RESERVE_TOKENS,
            keep_recent_tokens: None,
            max_summary_tokens: DEFAULT_MAX_SUMMARY_TOKENS,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        }
    }
}

/// Why the host requested planning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TriggerReason {
    /// Apply the fixed context-pressure threshold.
    Automatic,
    /// Plan immediately while retaining all safety checks.
    Manual,
}

impl TriggerReason {
    /// Returns the schema version governing this enum.
    pub const fn schema_version(self) -> u32 {
        COMPACTION_SCHEMA_VERSION
    }
}

/// Model context capacity and current request usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextTokenBudget {
    pub(crate) schema_version: u32,
    pub(crate) context_window_tokens: u64,
    pub(crate) total_tokens: u64,
}

impl ContextTokenBudget {
    /// Creates a token budget from model capacity and current usage.
    pub fn new(context_window_tokens: u64, total_tokens: u64) -> Self {
        Self {
            schema_version: COMPACTION_SCHEMA_VERSION,
            context_window_tokens,
            total_tokens,
        }
    }

    /// Returns the model context-window size.
    pub fn context_window_tokens(self) -> u64 {
        self.context_window_tokens
    }

    /// Returns current total request tokens.
    pub fn total_tokens(self) -> u64 {
        self.total_tokens
    }
}

/// One source message plus optional host identity and token metadata.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionMessage {
    pub(crate) schema_version: u32,
    pub(crate) id: Option<MessageId>,
    pub(crate) message: Message,
    pub(crate) token_count: Option<u64>,
}

impl CompactionMessage {
    /// Wraps a message for planning with conservative token estimation.
    pub fn new(message: Message) -> Self {
        Self {
            schema_version: COMPACTION_SCHEMA_VERSION,
            id: None,
            message,
            token_count: None,
        }
    }

    /// Attaches the append-only session entry identity.
    #[must_use]
    pub fn with_id(mut self, id: MessageId) -> Self {
        self.id = Some(id);
        self
    }

    /// Supplies a trusted host estimate for this complete message.
    #[must_use]
    pub fn with_token_count(mut self, token_count: u64) -> Self {
        self.token_count = Some(token_count);
        self
    }

    /// Returns the optional session entry identity.
    pub fn id(&self) -> Option<&MessageId> {
        self.id.as_ref()
    }

    /// Returns the source message without cloning it.
    pub fn message(&self) -> &Message {
        &self.message
    }

    /// Returns the optional trusted host token estimate.
    pub fn token_count(&self) -> Option<u64> {
        self.token_count
    }
}

impl fmt::Debug for CompactionMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompactionMessage")
            .field("schema_version", &self.schema_version)
            .field("id", &self.id)
            .field("message_kind", &message_kind(&self.message))
            .field("token_count", &self.token_count)
            .finish()
    }
}

/// A caller-recorded operation that survives summary regeneration.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeterministicOperation {
    pub(crate) schema_version: u32,
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) status: String,
}

impl DeterministicOperation {
    /// Creates an operation identified by the stable host `key`.
    pub fn new(
        key: impl Into<String>,
        label: impl Into<String>,
        status: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: COMPACTION_SCHEMA_VERSION,
            key: key.into(),
            label: label.into(),
            status: status.into(),
        }
    }

    /// Returns the stable de-duplication key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Returns the caller-supplied operation label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Returns the caller-supplied operation status.
    pub fn status(&self) -> &str {
        &self.status
    }
}

impl fmt::Debug for DeterministicOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeterministicOperation")
            .field("schema_version", &self.schema_version)
            .field("key", &self.key)
            .field("label", &"<redacted>")
            .field("status", &self.status)
            .finish()
    }
}

/// Authoritative host facts kept separate from model-authored prose.
///
/// Path values persist through a private tagged exact representation
/// (readable UTF-8, raw Unix bytes, or raw Windows UTF-16 code units, both
/// base64 encoded) instead of derived serde `PathBuf` encoding, so non-UTF-8
/// host paths survive JSON session entries without lossy collisions.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeterministicDetails {
    pub(crate) schema_version: u32,
    #[serde(with = "crate::path_encoding::path_vec")]
    pub(crate) files_read: Vec<PathBuf>,
    #[serde(with = "crate::path_encoding::path_vec")]
    pub(crate) files_modified: Vec<PathBuf>,
    pub(crate) commands: Vec<String>,
    pub(crate) todo_operations: Vec<DeterministicOperation>,
    pub(crate) background_operations: Vec<DeterministicOperation>,
}

impl DeterministicDetails {
    /// Creates an empty authoritative sidecar.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one file read by the host.
    #[must_use]
    pub fn with_file_read(mut self, path: impl Into<PathBuf>) -> Self {
        self.files_read.push(path.into());
        self
    }

    /// Records one file modified by the host.
    #[must_use]
    pub fn with_file_modified(mut self, path: impl Into<PathBuf>) -> Self {
        self.files_modified.push(path.into());
        self
    }

    /// Records one command executed by the host.
    #[must_use]
    pub fn with_command(mut self, command: impl Into<String>) -> Self {
        self.commands.push(command.into());
        self
    }

    /// Records one todo operation supplied by the host.
    #[must_use]
    pub fn with_todo_operation(mut self, operation: DeterministicOperation) -> Self {
        self.todo_operations.push(operation);
        self
    }

    /// Records one background operation supplied by the host.
    #[must_use]
    pub fn with_background_operation(mut self, operation: DeterministicOperation) -> Self {
        self.background_operations.push(operation);
        self
    }

    /// Returns de-duplicated file-read records after compaction.
    pub fn files_read(&self) -> &[PathBuf] {
        &self.files_read
    }

    /// Returns de-duplicated file-modification records after compaction.
    pub fn files_modified(&self) -> &[PathBuf] {
        &self.files_modified
    }

    /// Returns de-duplicated command records after compaction.
    pub fn commands(&self) -> &[String] {
        &self.commands
    }

    /// Returns merged todo records after compaction.
    pub fn todo_operations(&self) -> &[DeterministicOperation] {
        &self.todo_operations
    }

    /// Returns merged background-operation records after compaction.
    pub fn background_operations(&self) -> &[DeterministicOperation] {
        &self.background_operations
    }

    /// Returns the serialized schema version.
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

impl Default for DeterministicDetails {
    fn default() -> Self {
        Self {
            schema_version: COMPACTION_SCHEMA_VERSION,
            files_read: Vec::new(),
            files_modified: Vec::new(),
            commands: Vec::new(),
            todo_operations: Vec::new(),
            background_operations: Vec::new(),
        }
    }
}

impl fmt::Debug for DeterministicDetails {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeterministicDetails")
            .field("schema_version", &self.schema_version)
            .field("files_read", &self.files_read.len())
            .field("files_modified", &self.files_modified.len())
            .field("commands", &self.commands.len())
            .field("todo_operations", &self.todo_operations.len())
            .field("background_operations", &self.background_operations.len())
            .finish()
    }
}

/// Immutable snapshot consumed by the pure planner and LLM compactor.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionInput {
    pub(crate) schema_version: u32,
    pub(crate) model: ModelId,
    pub(crate) budget: ContextTokenBudget,
    pub(crate) trigger_reason: TriggerReason,
    pub(crate) messages: Vec<CompactionMessage>,
    pub(crate) previous_summary: Option<String>,
    pub(crate) previous_details: Option<DeterministicDetails>,
    pub(crate) details: DeterministicDetails,
}

impl CompactionInput {
    /// Creates an automatic compaction snapshot for `model`.
    pub fn new(
        model: impl Into<ModelId>,
        budget: ContextTokenBudget,
        messages: Vec<CompactionMessage>,
    ) -> Self {
        Self {
            schema_version: COMPACTION_SCHEMA_VERSION,
            model: model.into(),
            budget,
            trigger_reason: TriggerReason::Automatic,
            messages,
            previous_summary: None,
            previous_details: None,
            details: DeterministicDetails::new(),
        }
    }

    /// Changes whether planning is automatic or manually requested.
    #[must_use]
    pub fn with_trigger_reason(mut self, trigger_reason: TriggerReason) -> Self {
        self.trigger_reason = trigger_reason;
        self
    }

    /// Supplies the previous summary as a distinct, non-recursive input.
    #[must_use]
    pub fn with_previous_summary(mut self, previous_summary: impl Into<String>) -> Self {
        self.previous_summary = Some(previous_summary.into());
        self
    }

    /// Supplies the previous authoritative sidecar for merging.
    #[must_use]
    pub fn with_previous_details(mut self, details: DeterministicDetails) -> Self {
        self.previous_details = Some(details);
        self
    }

    /// Supplies authoritative facts observed since the prior compaction.
    #[must_use]
    pub fn with_details(mut self, details: DeterministicDetails) -> Self {
        self.details = details;
        self
    }

    /// Returns the model id passed by the current host configuration.
    pub fn model(&self) -> &ModelId {
        &self.model
    }

    /// Returns the current model token budget.
    pub fn budget(&self) -> ContextTokenBudget {
        self.budget
    }

    /// Returns the planning trigger reason.
    pub fn trigger_reason(&self) -> TriggerReason {
        self.trigger_reason
    }

    /// Returns source messages in branch order.
    pub fn messages(&self) -> &[CompactionMessage] {
        &self.messages
    }

    /// Returns the prior summary, separate from the new source span.
    pub fn previous_summary(&self) -> Option<&str> {
        self.previous_summary.as_deref()
    }

    /// Returns facts observed since the prior compaction.
    pub fn details(&self) -> &DeterministicDetails {
        &self.details
    }
}

impl fmt::Debug for CompactionInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompactionInput")
            .field("schema_version", &self.schema_version)
            .field("model", &self.model)
            .field("budget", &self.budget)
            .field("trigger_reason", &self.trigger_reason)
            .field("message_count", &self.messages.len())
            .field(
                "previous_summary_chars",
                &self
                    .previous_summary
                    .as_ref()
                    .map(|summary| summary.chars().count()),
            )
            .field("has_previous_details", &self.previous_details.is_some())
            .field("details", &self.details)
            .finish()
    }
}

/// The safe cut selected by the planner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum CompactionCut {
    /// Compact every message before `next_message_index`.
    MessageBoundary {
        /// First retained message index.
        next_message_index: usize,
        /// Identity expected at that index, when supplied by the host.
        next_message_id: Option<MessageId>,
        /// True only when the cut is inside a user turn.
        split_turn: bool,
    },
    /// Compact a prefix of one user text block.
    UserTextPrefix {
        /// User message containing the split block.
        message_index: usize,
        /// Identity expected at `message_index`, when available.
        message_id: Option<MessageId>,
        /// Text block containing the split point.
        block_index: usize,
        /// Unicode scalar values placed in the compacted prefix.
        char_offset: usize,
    },
}

impl CompactionCut {
    /// Returns the first retained source-message index.
    pub fn retained_message_index(&self) -> usize {
        match self {
            Self::MessageBoundary {
                next_message_index, ..
            } => *next_message_index,
            Self::UserTextPrefix { message_index, .. } => *message_index,
        }
    }

    /// Returns whether the plan splits one user turn.
    pub fn is_split_prefix(&self) -> bool {
        match self {
            Self::MessageBoundary { split_turn, .. } => *split_turn,
            Self::UserTextPrefix { .. } => true,
        }
    }
}

/// Pure, versioned instructions for replacing one history prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionPlan {
    pub(crate) schema_version: u32,
    pub(crate) trigger_reason: TriggerReason,
    pub(crate) source_message_count: usize,
    pub(crate) source_first_id: Option<MessageId>,
    pub(crate) source_last_id: Option<MessageId>,
    pub(crate) cut: CompactionCut,
    pub(crate) context_window_tokens: u64,
    pub(crate) total_tokens_before: u64,
    pub(crate) trigger_threshold_tokens: u64,
    pub(crate) keep_recent_tokens: u64,
    pub(crate) result_budget_tokens: u64,
    pub(crate) max_summary_tokens: u64,
    pub(crate) estimated_compacted_tokens: u64,
    pub(crate) estimated_retained_tokens: u64,
    pub(crate) estimated_fixed_overhead_tokens: u64,
}

impl CompactionPlan {
    /// Returns the serialized schema version.
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Returns the reason recorded by the planner.
    pub fn trigger_reason(&self) -> TriggerReason {
        self.trigger_reason
    }

    /// Returns the source message count used during planning.
    pub fn source_message_count(&self) -> usize {
        self.source_message_count
    }

    /// Returns the first source entry id, when supplied by the host.
    pub fn source_first_id(&self) -> Option<&MessageId> {
        self.source_first_id.as_ref()
    }

    /// Returns the source branch-tip id, when supplied by the host.
    pub fn source_last_id(&self) -> Option<&MessageId> {
        self.source_last_id.as_ref()
    }

    /// Returns the validated cut point.
    pub fn cut(&self) -> &CompactionCut {
        &self.cut
    }

    /// Returns the fixed automatic trigger threshold.
    pub fn trigger_threshold_tokens(&self) -> u64 {
        self.trigger_threshold_tokens
    }

    /// Returns the recent-token target used during planning.
    pub fn keep_recent_tokens(&self) -> u64 {
        self.keep_recent_tokens
    }

    /// Returns the maximum rebuilt request size.
    pub fn result_budget_tokens(&self) -> u64 {
        self.result_budget_tokens
    }

    /// Returns the accepted summary-token ceiling.
    pub fn max_summary_tokens(&self) -> u64 {
        self.max_summary_tokens
    }

    /// Returns estimated source tokens entering the summary.
    pub fn estimated_compacted_tokens(&self) -> u64 {
        self.estimated_compacted_tokens
    }

    /// Returns estimated source tokens retained verbatim.
    pub fn estimated_retained_tokens(&self) -> u64 {
        self.estimated_retained_tokens
    }
}

/// Exact latest user message retained as non-model-authored metadata.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct LatestUserRequest {
    pub(crate) schema_version: u32,
    pub(crate) message_index: usize,
    pub(crate) message_id: Option<MessageId>,
    pub(crate) message: UserMessage,
}

impl LatestUserRequest {
    /// Returns the original source-message index.
    pub fn message_index(&self) -> usize {
        self.message_index
    }

    /// Returns the optional session entry identity.
    pub fn message_id(&self) -> Option<&MessageId> {
        self.message_id.as_ref()
    }

    /// Returns the original user message verbatim.
    pub fn message(&self) -> &UserMessage {
        &self.message
    }
}

impl fmt::Debug for LatestUserRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LatestUserRequest")
            .field("schema_version", &self.schema_version)
            .field("message_index", &self.message_index)
            .field("message_id", &self.message_id)
            .field("content_blocks", &self.message.content.len())
            .finish()
    }
}

/// Audit record for one bounded tool-result serialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResultTruncation {
    pub(crate) schema_version: u32,
    pub(crate) message_index: usize,
    pub(crate) tool_call_id: String,
    pub(crate) original_chars: usize,
    pub(crate) serialized_chars: usize,
}

impl ToolResultTruncation {
    /// Returns the source message index.
    pub fn message_index(&self) -> usize {
        self.message_index
    }

    /// Returns the paired tool-call id.
    pub fn tool_call_id(&self) -> &str {
        &self.tool_call_id
    }

    /// Returns the original rendered character count.
    pub fn original_chars(&self) -> usize {
        self.original_chars
    }

    /// Returns the bounded rendered character count.
    pub fn serialized_chars(&self) -> usize {
        self.serialized_chars
    }
}

/// Validated metadata accompanying a generated summary.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionDetails {
    pub(crate) schema_version: u32,
    pub(crate) provider_id: String,
    pub(crate) model: ModelId,
    pub(crate) attempts: u32,
    pub(crate) total_tokens_before: u64,
    pub(crate) estimated_tokens_after: u64,
    pub(crate) latest_user_request: Option<LatestUserRequest>,
    pub(crate) deterministic: DeterministicDetails,
    pub(crate) tool_result_truncations: Vec<ToolResultTruncation>,
    pub(crate) transcript_omitted_messages: usize,
}

impl CompactionDetails {
    /// Returns the provider selected by the host.
    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    /// Returns the host-selected model id.
    pub fn model(&self) -> &ModelId {
        &self.model
    }

    /// Returns total provider attempts used.
    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// Returns token usage before compaction.
    pub fn total_tokens_before(&self) -> u64 {
        self.total_tokens_before
    }

    /// Returns conservative rebuilt-context tokens.
    pub fn estimated_tokens_after(&self) -> u64 {
        self.estimated_tokens_after
    }

    /// Returns the latest real user message verbatim.
    pub fn latest_user_request(&self) -> Option<&LatestUserRequest> {
        self.latest_user_request.as_ref()
    }

    /// Returns authoritative merged host facts.
    pub fn deterministic(&self) -> &DeterministicDetails {
        &self.deterministic
    }

    /// Returns tool-result truncation audit records.
    pub fn tool_result_truncations(&self) -> &[ToolResultTruncation] {
        &self.tool_result_truncations
    }

    /// Returns whole model-visible messages omitted from the summary transcript.
    pub fn transcript_omitted_messages(&self) -> usize {
        self.transcript_omitted_messages
    }

    /// Returns the serialized schema version.
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

impl fmt::Debug for CompactionDetails {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompactionDetails")
            .field("schema_version", &self.schema_version)
            .field("provider_id", &self.provider_id)
            .field("model", &self.model)
            .field("attempts", &self.attempts)
            .field("total_tokens_before", &self.total_tokens_before)
            .field("estimated_tokens_after", &self.estimated_tokens_after)
            .field("latest_user_request", &self.latest_user_request)
            .field("deterministic", &self.deterministic)
            .field(
                "tool_result_truncations",
                &self.tool_result_truncations.len(),
            )
            .field(
                "transcript_omitted_messages",
                &self.transcript_omitted_messages,
            )
            .finish()
    }
}

/// Fully validated transactional compaction result.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionOutput {
    pub(crate) schema_version: u32,
    pub(crate) summary: String,
    pub(crate) plan: CompactionPlan,
    pub(crate) details: CompactionDetails,
}

impl CompactionOutput {
    /// Returns the fixed-schema model summary.
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// Returns the immutable cut plan.
    pub fn plan(&self) -> &CompactionPlan {
        &self.plan
    }

    /// Returns validated deterministic and execution metadata.
    pub fn details(&self) -> &CompactionDetails {
        &self.details
    }

    /// Returns the serialized schema version.
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

impl fmt::Debug for CompactionOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompactionOutput")
            .field("schema_version", &self.schema_version)
            .field("summary_chars", &self.summary.chars().count())
            .field("plan", &self.plan)
            .field("details", &self.details)
            .finish()
    }
}

/// Stable validation category for host error handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ValidationCode {
    /// A serialized value uses an unsupported schema version.
    UnsupportedVersion,
    /// Trusted policy values are internally inconsistent.
    InvalidPolicy,
    /// Input token or message metadata is invalid.
    InvalidInput,
    /// A tool result has no preceding unresolved call.
    OrphanToolResult,
    /// A tool call has no result in the source history.
    UnresolvedToolCall,
    /// A tool-call id occurs more than once.
    DuplicateToolCall,
    /// No cut can preserve tool pairs and useful recent context.
    NoSafeCut,
    /// A persisted cut index or text offset is outside the source.
    CutOutOfRange,
    /// A persisted cut id does not match the source snapshot.
    CutIdMismatch,
    /// The provider returned no summary text.
    EmptySummary,
    /// The provider response stopped before a complete summary was guaranteed.
    IncompleteSummary,
    /// The provider returned too little substantive summary text.
    SummaryTooShort,
    /// The fixed summary schema is incomplete or out of order.
    MissingHeading,
    /// The summary visibly repeats the compactor prompt.
    PromptEcho,
    /// The summary consists mostly of placeholders or repetition.
    DegenerateSummary,
    /// The accepted summary exceeds its token ceiling.
    SummaryBudgetExceeded,
    /// The rebuilt context saves less than the fixed minimum.
    InsufficientSavings,
    /// The rebuilt context exceeds available model tokens.
    ResultBudgetExceeded,
    /// The summary response attempted to call a tool.
    UnexpectedToolCall,
}

/// Versioned, serializable validation failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationError {
    pub(crate) schema_version: u32,
    pub(crate) code: ValidationCode,
    pub(crate) message: String,
}

impl ValidationError {
    pub(crate) fn new(code: ValidationCode, message: impl Into<String>) -> Self {
        Self {
            schema_version: COMPACTION_SCHEMA_VERSION,
            code,
            message: message.into(),
        }
    }

    /// Returns the stable error category.
    pub fn code(&self) -> ValidationCode {
        self.code
    }

    /// Returns the diagnostic without conversation content.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the serialized schema version.
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl StdError for ValidationError {}

/// Failure from provider execution or post-generation validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum CompactionError {
    /// Deterministic input, plan, summary, or rebuilt-context failure.
    Validation { error: ValidationError },
    /// Provider failure after `attempts` bounded attempts.
    Provider { error: LlmError, attempts: u32 },
    /// Caller or provider cancellation; never retried.
    Cancelled { attempts: u32 },
}

impl CompactionError {
    /// Returns the validation cause, when present.
    pub fn validation(&self) -> Option<&ValidationError> {
        match self {
            Self::Validation { error } => Some(error),
            Self::Provider { .. } | Self::Cancelled { .. } => None,
        }
    }

    /// Returns whether cancellation ended the operation.
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled { .. })
    }

    /// Returns attempts recorded by provider or cancellation failures.
    ///
    /// Validation failures intentionally carry no execution metadata and
    /// therefore return zero, even when validation followed a model response.
    pub fn attempts(&self) -> u32 {
        match self {
            Self::Validation { .. } => 0,
            Self::Provider { attempts, .. } | Self::Cancelled { attempts } => *attempts,
        }
    }
}

impl fmt::Display for CompactionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation { error } => {
                write!(formatter, "compaction validation failed: {error}")
            }
            Self::Provider { error, attempts } => {
                write!(
                    formatter,
                    "compaction provider failed after {attempts} attempt(s): {error}"
                )
            }
            Self::Cancelled { attempts } => {
                write!(
                    formatter,
                    "compaction cancelled after {attempts} attempt(s)"
                )
            }
        }
    }
}

impl StdError for CompactionError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Validation { error } => Some(error),
            Self::Provider { error, .. } => Some(error),
            Self::Cancelled { .. } => None,
        }
    }
}

impl From<ValidationError> for CompactionError {
    fn from(error: ValidationError) -> Self {
        Self::Validation { error }
    }
}

pub(crate) fn message_kind(message: &Message) -> &'static str {
    match message {
        Message::User(_) => "user",
        Message::Assistant(_) => "assistant",
        Message::ToolResult(_) => "tool_result",
        Message::Custom(_) => "custom",
    }
}

// Rust guideline compliant 2026-08-26.
