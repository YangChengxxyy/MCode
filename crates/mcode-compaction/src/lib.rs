//! Private, closed context compaction for the MCode host.
//!
//! This unpublished crate owns planning, bounded transcript serialization,
//! current-provider summary generation, and transactional validation. Its
//! public Rust items exist only so sibling MCode host crates can call across a
//! crate boundary. They are not a stable extension SDK and must not be exposed
//! through plugin registries, hooks, callbacks, or third-party strategy traits.
//!
//! The operation is deliberately sans-state: [`compact_context`] borrows an
//! immutable [`CompactionInput`] and returns a [`CompactionOutput`] only after
//! summary and rebuilt-context validation. It never mutates agent state or a
//! session store. See the crate `README.md` for the actor/session integration
//! contract.

#![forbid(unsafe_code)]

mod details;
mod estimate;
mod llm;
mod path_encoding;
mod planner;
mod topology;
mod transcript;
mod trigger;
mod types;
mod validation;

#[doc(inline)]
pub use estimate::TokenEstimator;
#[doc(inline)]
pub use llm::compact_context;
#[doc(inline)]
pub use planner::plan_compaction;
#[doc(inline)]
pub use trigger::{
    AdaptiveTriggerPolicy, HARD_MAX_WORKING_TOKENS, TriggerDecision, TriggerInputs,
    evaluate_trigger,
};
#[doc(inline)]
pub use types::{
    COMPACTION_SCHEMA_VERSION, CompactionCut, CompactionDetails, CompactionError, CompactionInput,
    CompactionMessage, CompactionOutput, CompactionPlan, CompactionPolicy, ContextTokenBudget,
    DeterministicDetails, DeterministicOperation, LatestUserRequest, ToolResultTruncation,
    TriggerReason, ValidationCode, ValidationError,
};
#[doc(inline)]
pub use validation::{SUMMARY_HEADINGS, rebuild_context, validate_rebuilt_context};

// Rust guideline compliant 2026-08-26.
