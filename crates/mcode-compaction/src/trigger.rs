//! Adaptive compaction trigger foundation (sealed host core).
//!
//! This module distinguishes the **advertised** model context (what a model
//! card claims) from the **effective working context** this host is willing to
//! trust. The hard invariant is
//! `effective <= min(advertised, session clamp, maxWorkingTokens, 400_000)`;
//! user or model settings may lower the 400k cap but can never raise it.
//!
//! The trigger threshold is `max(1, min(floor(effective * triggerRatio),
//! effective - adaptiveReserve))`, where the adaptive reserve combines a
//! policy baseline with bounded allowances for the requested maximum output
//! and tool schema definitions. The reserve deliberately caps the output
//! allowance instead of reserving the model's full claimed maximum output, and
//! the one-token floor keeps a legal sub-token ratio product from yielding a
//! zero threshold, which would fire at zero usage and immediately re-trigger
//! after compacting to a zero target. Compaction
//! aims for `min(floor(effective * targetRatio), threshold -
//! ceil(threshold * minGainRatio))`, which stays strictly below the final
//! threshold — including thresholds lowered by the adaptive reserve or the
//! uncertainty discount — and preserves the required minimum gain, forming
//! the hysteresis band.
//!
//! This foundation is sealed: there is no plugin hook, strategy trait, or
//! third-party replacement API, no cross-provider session forwarding, and no
//! retry loop. A host that receives a smaller provider-reported context length
//! may lower [`TriggerInputs::session_context_cap_tokens`] for the session and
//! attempt at most one compact-and-retry at the upper layer; repeated trigger
//! pressure must surface as a configuration error, not as unbounded retries.
//! All configuration travels as JSON through serde; there is no TOML surface.

use serde::{Deserialize, Serialize};

use crate::types::{COMPACTION_SCHEMA_VERSION, ValidationCode, ValidationError};

/// Hard upper bound on the effective working context; settings can only lower it.
pub const HARD_MAX_WORKING_TOKENS: u64 = 400_000;

/// Default trigger position inside the effective working context.
const DEFAULT_TRIGGER_RATIO: f64 = 0.82;
/// Default post-compaction position; strictly below the trigger ratio.
const DEFAULT_TARGET_RATIO: f64 = 0.55;
/// Default minimum required relative gain from trigger to target.
const DEFAULT_MIN_GAIN_RATIO: f64 = 0.20;
/// Default policy reserve baseline shared with the fixed compaction policy.
const DEFAULT_RESERVE_TOKENS: u64 = 16_384;
/// The output reserve never trusts more than this many requested output tokens.
const RESERVE_OUTPUT_CAP_TOKENS: u64 = 32_768;
/// The output reserve also never exceeds one eighth of the effective context.
const RESERVE_OUTPUT_SHARE_DIVISOR: u64 = 8;
/// Safety cap on tool and schema definition allowances.
const RESERVE_TOOL_SCHEMA_CAP_TOKENS: u64 = 16_384;
/// Maximum relative threshold reduction for estimation uncertainty.
const UNCERTAINTY_DISCOUNT_MAX: f64 = 0.10;
/// Threshold reduction per unit of tool-output share when usage is estimated.
const UNCERTAINTY_DISCOUNT_PER_TOOL_SHARE: f64 = 0.25;

/// Sealed JSON policy for adaptive compaction triggers.
///
/// Ratio fields must be finite and strictly inside `(0, 1)`, `targetRatio`
/// must be below `triggerRatio`, the trigger-to-target gain must reach
/// `minGainRatio` under the same decimal-exact semantics as the token
/// products, and `maxWorkingTokens` may never exceed
/// [`HARD_MAX_WORKING_TOKENS`]. `reserveTokens` is the required baseline of
/// the adaptive reserve and has no schema default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdaptiveTriggerPolicy {
    #[serde(default = "default_schema_version")]
    pub(crate) schema_version: u32,
    #[serde(default = "default_trigger_ratio")]
    pub(crate) trigger_ratio: f64,
    #[serde(default = "default_target_ratio")]
    pub(crate) target_ratio: f64,
    #[serde(default = "default_max_working_tokens")]
    pub(crate) max_working_tokens: u64,
    pub(crate) reserve_tokens: u64,
    #[serde(default = "default_min_gain_ratio")]
    pub(crate) min_gain_ratio: f64,
}

impl AdaptiveTriggerPolicy {
    /// Creates the policy with default ratios and the default reserve baseline.
    pub fn new() -> Self {
        Self {
            schema_version: COMPACTION_SCHEMA_VERSION,
            trigger_ratio: DEFAULT_TRIGGER_RATIO,
            target_ratio: DEFAULT_TARGET_RATIO,
            max_working_tokens: HARD_MAX_WORKING_TOKENS,
            reserve_tokens: DEFAULT_RESERVE_TOKENS,
            min_gain_ratio: DEFAULT_MIN_GAIN_RATIO,
        }
    }

    /// Sets the trigger position inside the effective working context.
    #[must_use]
    pub fn with_trigger_ratio(mut self, trigger_ratio: f64) -> Self {
        self.trigger_ratio = trigger_ratio;
        self
    }

    /// Sets the post-compaction target position; must stay below the trigger.
    #[must_use]
    pub fn with_target_ratio(mut self, target_ratio: f64) -> Self {
        self.target_ratio = target_ratio;
        self
    }

    /// Lowers the working-context cap; values above 400k are rejected.
    #[must_use]
    pub fn with_max_working_tokens(mut self, max_working_tokens: u64) -> Self {
        self.max_working_tokens = max_working_tokens;
        self
    }

    /// Sets the required adaptive-reserve baseline.
    #[must_use]
    pub fn with_reserve_tokens(mut self, reserve_tokens: u64) -> Self {
        self.reserve_tokens = reserve_tokens;
        self
    }

    /// Sets the minimum relative gain required from trigger to target.
    #[must_use]
    pub fn with_min_gain_ratio(mut self, min_gain_ratio: f64) -> Self {
        self.min_gain_ratio = min_gain_ratio;
        self
    }

    /// Returns the serialized schema version.
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Returns the trigger ratio.
    pub fn trigger_ratio(&self) -> f64 {
        self.trigger_ratio
    }

    /// Returns the post-compaction target ratio.
    pub fn target_ratio(&self) -> f64 {
        self.target_ratio
    }

    /// Returns the configured working-context cap.
    pub fn max_working_tokens(&self) -> u64 {
        self.max_working_tokens
    }

    /// Returns the adaptive-reserve baseline.
    pub fn reserve_tokens(&self) -> u64 {
        self.reserve_tokens
    }

    /// Returns the minimum required trigger-to-target gain ratio.
    pub fn min_gain_ratio(&self) -> f64 {
        self.min_gain_ratio
    }

    /// Validates finiteness, ranges, hysteresis gain, and the hard cap.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError`] with [`ValidationCode::InvalidPolicy`] for
    /// non-finite ratios, ratios outside `(0, 1)`, `targetRatio >=
    /// triggerRatio`, an unreachable minimum gain, a working-context cap above
    /// [`HARD_MAX_WORKING_TOKENS`], or a reserve that leaves no working room.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.schema_version != COMPACTION_SCHEMA_VERSION {
            return Err(ValidationError::new(
                ValidationCode::UnsupportedVersion,
                "adaptive trigger policy uses an unsupported schema version",
            ));
        }
        for (label, ratio) in [
            ("triggerRatio", self.trigger_ratio),
            ("targetRatio", self.target_ratio),
            ("minGainRatio", self.min_gain_ratio),
        ] {
            if !ratio.is_finite() || ratio <= 0.0 || ratio >= 1.0 {
                return Err(ValidationError::new(
                    ValidationCode::InvalidPolicy,
                    format!("{label} must be finite and strictly between 0 and 1"),
                ));
            }
        }
        if self.target_ratio >= self.trigger_ratio {
            return Err(ValidationError::new(
                ValidationCode::InvalidPolicy,
                "targetRatio must stay strictly below triggerRatio",
            ));
        }
        if !hysteresis_gain_meets_minimum(
            self.trigger_ratio,
            self.target_ratio,
            self.min_gain_ratio,
        ) {
            return Err(ValidationError::new(
                ValidationCode::InvalidPolicy,
                format!(
                    "trigger-to-target gain {:.4} is below the required minGainRatio {:.4}",
                    (self.trigger_ratio - self.target_ratio) / self.trigger_ratio,
                    self.min_gain_ratio
                ),
            ));
        }
        if self.max_working_tokens == 0 || self.max_working_tokens > HARD_MAX_WORKING_TOKENS {
            return Err(ValidationError::new(
                ValidationCode::InvalidPolicy,
                format!(
                    "maxWorkingTokens may only lower the hard cap of {HARD_MAX_WORKING_TOKENS}"
                ),
            ));
        }
        if self.reserve_tokens == 0 || self.reserve_tokens >= self.max_working_tokens {
            return Err(ValidationError::new(
                ValidationCode::InvalidPolicy,
                "reserveTokens must be positive and leave working context below the cap",
            ));
        }
        Ok(())
    }
}

impl Default for AdaptiveTriggerPolicy {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-request inputs for one trigger evaluation.
///
/// `advertised_context_tokens` is what the model advertises;
/// `estimated_used_tokens` is the host's conservative usage estimate; the
/// optional fields are the documented wiring points for trusted
/// provider-reported usage, runtime context-length clamps, requested output
/// size, tool/schema overhead, and tool-output share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerInputs {
    #[serde(default = "default_schema_version")]
    pub(crate) schema_version: u32,
    pub(crate) advertised_context_tokens: u64,
    pub(crate) estimated_used_tokens: u64,
    #[serde(default)]
    pub(crate) session_context_cap_tokens: Option<u64>,
    #[serde(default)]
    pub(crate) requested_max_output_tokens: Option<u64>,
    #[serde(default)]
    pub(crate) tool_schema_tokens: Option<u64>,
    #[serde(default)]
    pub(crate) provider_reported_total_tokens: Option<u64>,
    #[serde(default)]
    pub(crate) tool_output_tokens: Option<u64>,
}

impl TriggerInputs {
    /// Creates inputs from advertised capacity and a conservative usage estimate.
    pub fn new(advertised_context_tokens: u64, estimated_used_tokens: u64) -> Self {
        Self {
            schema_version: COMPACTION_SCHEMA_VERSION,
            advertised_context_tokens,
            estimated_used_tokens,
            session_context_cap_tokens: None,
            requested_max_output_tokens: None,
            tool_schema_tokens: None,
            provider_reported_total_tokens: None,
            tool_output_tokens: None,
        }
    }

    /// Lowers this session's working-context cap; the clamp can only lower.
    #[must_use]
    pub fn with_session_context_cap(mut self, cap_tokens: u64) -> Self {
        self.session_context_cap_tokens = Some(cap_tokens);
        self
    }

    /// Supplies the requested maximum output tokens for the adaptive reserve.
    #[must_use]
    pub fn with_requested_max_output(mut self, tokens: u64) -> Self {
        self.requested_max_output_tokens = Some(tokens);
        self
    }

    /// Supplies tool and schema definition overhead for the adaptive reserve.
    #[must_use]
    pub fn with_tool_schema_tokens(mut self, tokens: u64) -> Self {
        self.tool_schema_tokens = Some(tokens);
        self
    }

    /// Supplies trusted provider-reported total usage for this session.
    #[must_use]
    pub fn with_provider_reported_total(mut self, tokens: u64) -> Self {
        self.provider_reported_total_tokens = Some(tokens);
        self
    }

    /// Supplies the estimated tool-output share of current usage.
    #[must_use]
    pub fn with_tool_output_tokens(mut self, tokens: u64) -> Self {
        self.tool_output_tokens = Some(tokens);
        self
    }

    /// Returns the advertised model context capacity.
    pub fn advertised_context_tokens(&self) -> u64 {
        self.advertised_context_tokens
    }

    /// Returns the host's conservative usage estimate.
    pub fn estimated_used_tokens(&self) -> u64 {
        self.estimated_used_tokens
    }

    /// Returns the runtime session clamp, when applied.
    pub fn session_context_cap_tokens(&self) -> Option<u64> {
        self.session_context_cap_tokens
    }

    /// Returns the requested maximum output tokens, when known.
    pub fn requested_max_output_tokens(&self) -> Option<u64> {
        self.requested_max_output_tokens
    }

    /// Returns the tool and schema overhead estimate, when supplied.
    pub fn tool_schema_tokens(&self) -> Option<u64> {
        self.tool_schema_tokens
    }

    /// Returns trusted provider-reported usage, when available.
    pub fn provider_reported_total_tokens(&self) -> Option<u64> {
        self.provider_reported_total_tokens
    }

    /// Returns the estimated tool-output share, when supplied.
    pub fn tool_output_tokens(&self) -> Option<u64> {
        self.tool_output_tokens
    }
}

/// One pure, stateless trigger evaluation result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerDecision {
    #[serde(default = "default_schema_version")]
    pub(crate) schema_version: u32,
    pub(crate) advertised_context_tokens: u64,
    pub(crate) effective_context_tokens: u64,
    pub(crate) reserve_tokens: u64,
    pub(crate) trigger_threshold_tokens: u64,
    pub(crate) target_tokens: u64,
    pub(crate) evaluated_used_tokens: u64,
    pub(crate) min_gain_tokens: u64,
    pub(crate) should_compact: bool,
}

impl TriggerDecision {
    /// Returns the advertised capacity that entered the evaluation.
    pub fn advertised_context_tokens(&self) -> u64 {
        self.advertised_context_tokens
    }

    /// Returns the effective working context after all clamps.
    pub fn effective_context_tokens(&self) -> u64 {
        self.effective_context_tokens
    }

    /// Returns the adaptive reserve applied to this evaluation.
    pub fn reserve_tokens(&self) -> u64 {
        self.reserve_tokens
    }

    /// Returns the computed trigger threshold.
    pub fn trigger_threshold_tokens(&self) -> u64 {
        self.trigger_threshold_tokens
    }

    /// Returns the post-compaction hysteresis target.
    pub fn target_tokens(&self) -> u64 {
        self.target_tokens
    }

    /// Returns the usage value the decision compared against the threshold.
    pub fn evaluated_used_tokens(&self) -> u64 {
        self.evaluated_used_tokens
    }

    /// Returns the minimum absolute token gain a compaction must deliver.
    pub fn min_gain_tokens(&self) -> u64 {
        self.min_gain_tokens
    }

    /// Returns whether compaction should start now.
    pub fn should_compact(&self) -> bool {
        self.should_compact
    }

    /// Returns the serialized schema version.
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

/// Evaluates the adaptive trigger once; the call is pure and stateless.
///
/// The effective working context is
/// `min(advertised, session clamp, maxWorkingTokens, 400_000)`. The threshold
/// is `max(1, min(floor(effective * triggerRatio), effective - reserve))` —
/// never zero, so zero usage never triggers — where the reserve adapts to the
/// requested output (capped, never the model's full claim) and tool/schema
/// overhead. When no trusted provider usage is
/// available and tool outputs dominate, the threshold is lowered by a bounded
/// deterministic uncertainty discount — never by any model self-assessment.
/// The usage compared against the threshold is the trusted provider-reported
/// total when present and the host estimate otherwise; the post-compaction
/// target stays strictly below the final threshold with the minimum relative
/// gain.
///
/// # Errors
///
/// Returns [`ValidationError`] with [`ValidationCode::InvalidPolicy`] for
/// invalid policies or reserves that leave no usable working context, and
/// [`ValidationCode::InvalidInput`] for a zero advertised context.
pub fn evaluate_trigger(
    policy: &AdaptiveTriggerPolicy,
    inputs: &TriggerInputs,
) -> Result<TriggerDecision, ValidationError> {
    policy.validate()?;
    if inputs.schema_version != COMPACTION_SCHEMA_VERSION {
        return Err(ValidationError::new(
            ValidationCode::UnsupportedVersion,
            "trigger inputs use an unsupported schema version",
        ));
    }
    if inputs.advertised_context_tokens == 0 {
        return Err(ValidationError::new(
            ValidationCode::InvalidInput,
            "advertised context tokens must be non-zero",
        ));
    }

    let effective = effective_context_tokens(
        inputs.advertised_context_tokens,
        policy.max_working_tokens,
        inputs.session_context_cap_tokens,
    );
    let reserve = adaptive_reserve(policy, effective, inputs);
    if reserve >= effective {
        return Err(ValidationError::new(
            ValidationCode::InvalidPolicy,
            format!(
                "adaptive reserve of {reserve} tokens leaves no working context inside {effective}"
            ),
        ));
    }

    // The threshold is floored at one token: a legal policy can spell a ratio
    // whose exact product with the effective context floors to zero, and a
    // zero threshold would report `should_compact` even at zero usage and
    // re-trigger immediately after compacting to a zero target. The reserve
    // bound is already at least one because `reserve < effective` is checked.
    let mut threshold = floor_ratio_product(effective, policy.trigger_ratio)
        .min(effective.saturating_sub(reserve))
        .max(1);
    if inputs.provider_reported_total_tokens.is_none() {
        if let Some(tool_output_tokens) = inputs.tool_output_tokens {
            let share = tool_output_tokens as f64 / (inputs.estimated_used_tokens.max(1)) as f64;
            let discount =
                (share * UNCERTAINTY_DISCOUNT_PER_TOOL_SHARE).min(UNCERTAINTY_DISCOUNT_MAX);
            let reduction = floor_ratio_product(threshold, discount);
            threshold = threshold.saturating_sub(reduction).max(1);
        }
    }

    // The target must follow the final threshold down: a ratio-only target
    // could sit above a reserve- or discount-lowered threshold, so compacting
    // to it would immediately re-trigger and destroy the promised hysteresis
    // band. The min-gain band keeps the required relative distance from the
    // actual threshold.
    let target = floor_ratio_product(effective, policy.target_ratio)
        .min(threshold.saturating_sub(ceil_ratio_product(threshold, policy.min_gain_ratio)));
    // A trusted provider report replaces the host estimate; a max() would
    // ignore smaller trusted reports entirely.
    let evaluated_used = match inputs.provider_reported_total_tokens {
        Some(reported) => reported,
        None => inputs.estimated_used_tokens,
    };
    let min_gain_tokens = ceil_ratio_product(evaluated_used, policy.min_gain_ratio);
    Ok(TriggerDecision {
        schema_version: COMPACTION_SCHEMA_VERSION,
        advertised_context_tokens: inputs.advertised_context_tokens,
        effective_context_tokens: effective,
        reserve_tokens: reserve,
        trigger_threshold_tokens: threshold,
        target_tokens: target,
        evaluated_used_tokens: evaluated_used,
        min_gain_tokens,
        should_compact: evaluated_used >= threshold,
    })
}

/// Clamps the advertised context to the effective working context.
fn effective_context_tokens(
    advertised: u64,
    max_working_tokens: u64,
    session_cap: Option<u64>,
) -> u64 {
    advertised
        .min(max_working_tokens)
        .min(HARD_MAX_WORKING_TOKENS)
        .min(session_cap.unwrap_or(u64::MAX))
        .max(1)
}

/// Computes the adaptive reserve for one evaluation.
fn adaptive_reserve(policy: &AdaptiveTriggerPolicy, effective: u64, inputs: &TriggerInputs) -> u64 {
    let output_allowance = inputs
        .requested_max_output_tokens
        .unwrap_or(0)
        .min(RESERVE_OUTPUT_CAP_TOKENS)
        .min(effective / RESERVE_OUTPUT_SHARE_DIVISOR);
    let tool_allowance = inputs
        .tool_schema_tokens
        .unwrap_or(0)
        .min(RESERVE_TOOL_SCHEMA_CAP_TOKENS);
    policy
        .reserve_tokens
        .saturating_add(output_allowance)
        .saturating_add(tool_allowance)
}

/// Computes `floor(value * ratio)` exactly for finite `ratio` in `[0, 1)`.
///
/// The ratio is interpreted as the exact decimal its shortest round-trip
/// spelling denotes (JSON `0.82` behaves as 82/100, not as the slightly
/// smaller binary double), so `floor(300_000 * 0.82)` is exactly `246_000`.
/// Pathological ratios outside the practical decimal range fall back to the
/// exact binary product, which stays deterministic and at most one below the
/// decimal intent.
fn floor_ratio_product(value: u64, ratio: f64) -> u64 {
    ratio_product_quotient(value, ratio, false)
}

/// Computes `ceil(value * ratio)` exactly for finite `ratio` in `[0, 1)`,
/// rounding up only when the exact product has a fractional remainder.
fn ceil_ratio_product(value: u64, ratio: f64) -> u64 {
    ratio_product_quotient(value, ratio, true)
}

/// Shared exact quotient of `value * ratio`; `round_up` selects the ceiling.
fn ratio_product_quotient(value: u64, ratio: f64, round_up: bool) -> u64 {
    debug_assert!(ratio.is_finite() && (0.0..1.0).contains(&ratio));
    if let Some((numerator, denominator)) = decimal_ratio_parts(ratio) {
        if let Some(product) = u128::from(value).checked_mul(numerator) {
            return quotient_with_remainder(product, denominator, round_up);
        }
    }
    // Exact binary fallback for ratios outside the practical decimal range.
    let (mantissa, exponent) = binary_ratio_parts(ratio);
    let product = u128::from(value).saturating_mul(u128::from(mantissa));
    let shift = (-exponent).min(127) as u32;
    let denominator = 1_u128 << shift;
    quotient_with_remainder(product, denominator, round_up)
}

/// Divides with an explicit rounding direction, saturating instead of
/// overflowing because the quotient never exceeds `value`.
fn quotient_with_remainder(product: u128, denominator: u128, round_up: bool) -> u64 {
    let quotient = product / denominator;
    let remainder = product % denominator;
    let result = if round_up && remainder > 0 {
        quotient + 1
    } else {
        quotient
    };
    u64::try_from(result).unwrap_or(u64::MAX)
}

/// Parses the shortest decimal spelling of a ratio in `[0, 1)` into an exact
/// fraction whose power-of-ten denominator fits 128-bit arithmetic.
///
/// Returns `None` for spellings that are not a plain `0.<digits>` form, have
/// no fractional digits, or exceed the supported digit count.
fn decimal_ratio_parts(ratio: f64) -> Option<(u128, u128)> {
    let text = format!("{ratio}");
    let fraction = text.strip_prefix("0.")?;
    if fraction.is_empty()
        || fraction.len() > 27
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let numerator: u128 = fraction.parse().ok()?;
    let denominator = 10_u128.pow(fraction.len() as u32);
    (numerator > 0 && numerator < denominator).then_some((numerator, denominator))
}

/// Splits a finite `f64` into `(mantissa, exponent)` with
/// `value == mantissa * 2^exponent` and `mantissa < 2^53`.
fn binary_ratio_parts(ratio: f64) -> (u64, i32) {
    debug_assert!(ratio.is_finite() && (0.0..1.0).contains(&ratio));
    let bits = ratio.to_bits();
    let raw_exponent = ((bits >> 52) & 0x7ff) as i32;
    let mantissa = if raw_exponent == 0 {
        bits & ((1_u64 << 52) - 1)
    } else {
        (bits & ((1_u64 << 52) - 1)) | (1_u64 << 52)
    };
    (mantissa, raw_exponent - 1075)
}

/// Checks `(trigger_ratio - target_ratio) / trigger_ratio >= min_gain_ratio`
/// under the shortest-decimal exact semantics used for the token products.
///
/// Binary subtraction and division would reject legitimate boundary
/// configurations such as `0.82/0.656/0.2`, whose decimal gain is exactly
/// 20 percent while the binary computation yields about
/// `0.19999999999999993`. Spellings or products beyond the supported decimal
/// precision fall back to the deterministic binary comparison.
fn hysteresis_gain_meets_minimum(
    trigger_ratio: f64,
    target_ratio: f64,
    min_gain_ratio: f64,
) -> bool {
    match decimal_hysteresis_comparison(trigger_ratio, target_ratio, min_gain_ratio) {
        Some(meets_minimum) => meets_minimum,
        None => (trigger_ratio - target_ratio) / trigger_ratio >= min_gain_ratio,
    }
}

/// Exact decimal cross-multiplication of the gain comparison; `None` when a
/// ratio is not spellable as a supported `0.<digits>` decimal or a product
/// would exceed 128-bit arithmetic.
fn decimal_hysteresis_comparison(
    trigger_ratio: f64,
    target_ratio: f64,
    min_gain_ratio: f64,
) -> Option<bool> {
    let (trigger_numerator, trigger_denominator) = decimal_ratio_parts(trigger_ratio)?;
    let (target_numerator, target_denominator) = decimal_ratio_parts(target_ratio)?;
    let (minimum_numerator, minimum_denominator) = decimal_ratio_parts(min_gain_ratio)?;
    // gain = (trigger - target) / trigger
    //      = (trigger_numerator * target_denominator
    //         - target_numerator * trigger_denominator)
    //        / (target_denominator * trigger_numerator)
    let gain_numerator = trigger_numerator
        .checked_mul(target_denominator)?
        .checked_sub(target_numerator.checked_mul(trigger_denominator)?)?;
    let gain_denominator = target_denominator.checked_mul(trigger_numerator)?;
    // Both denominators are positive, so cross-multiplication preserves the
    // inequality without any division or rounding.
    Some(
        gain_numerator.checked_mul(minimum_denominator)?
            >= minimum_numerator.checked_mul(gain_denominator)?,
    )
}

fn default_schema_version() -> u32 {
    COMPACTION_SCHEMA_VERSION
}

fn default_trigger_ratio() -> f64 {
    DEFAULT_TRIGGER_RATIO
}

fn default_target_ratio() -> f64 {
    DEFAULT_TARGET_RATIO
}

fn default_max_working_tokens() -> u64 {
    HARD_MAX_WORKING_TOKENS
}

fn default_min_gain_ratio() -> f64 {
    DEFAULT_MIN_GAIN_RATIO
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_products_are_exact_at_known_ratios() {
        assert_eq!(floor_ratio_product(400_000, 0.82), 328_000);
        assert_eq!(floor_ratio_product(300_000, 0.82), 246_000);
        assert_eq!(floor_ratio_product(272_000, 0.82), 223_040);
        assert_eq!(floor_ratio_product(128_000, 0.82), 104_960);
        assert_eq!(floor_ratio_product(400_000, 0.55), 220_000);
        assert_eq!(floor_ratio_product(3, 0.5), 1);
        assert_eq!(floor_ratio_product(10, 0.1), 1);
    }

    #[test]
    fn floor_products_stay_within_one_of_the_float_product() {
        for value in [1_u64, 7, 128, 4_097, 65_535, 400_000] {
            for ratio in [0.1_f64, 0.3, 0.55, 0.82, 0.95] {
                let exact = floor_ratio_product(value, ratio);
                let float_product = value as f64 * ratio;
                assert!(
                    (exact as f64) <= float_product + 1.0e-6
                        && exact as f64 + 1.0 > float_product - 1.0e-6,
                    "value {value} ratio {ratio}: exact {exact} float {float_product}"
                );
            }
        }
    }

    #[test]
    fn ceil_products_are_exact() {
        // 0.25 is an exact binary rational, so ceil(100 * 0.25) is 25.
        assert_eq!(ceil_ratio_product(100, 0.25), 25);
        assert_eq!(ceil_ratio_product(101, 0.25), 26);
        assert_eq!(ceil_ratio_product(0, 0.2), 0);
        // Decimal semantics: 0.2 behaves as 1/5, so 101/5 rounds up to 21.
        assert_eq!(ceil_ratio_product(100, 0.2), 20);
        assert_eq!(ceil_ratio_product(101, 0.2), 21);
        // 18446744073709551615 * 82/100 = 15126330140441832324.3 exactly.
        assert_eq!(
            ceil_ratio_product(u64::MAX, 0.82),
            15_126_330_140_441_832_325
        );
    }

    #[test]
    fn default_policy_json_keeps_camel_case_fields_and_requires_reserve() {
        let policy = AdaptiveTriggerPolicy::new().with_reserve_tokens(12_345);
        let json = serde_json::to_string(&policy).unwrap();
        assert!(json.contains("\"triggerRatio\":0.82"));
        assert!(json.contains("\"targetRatio\":0.55"));
        assert!(json.contains("\"maxWorkingTokens\":400000"));
        assert!(json.contains("\"minGainRatio\":0.2"));
        assert!(json.contains("\"reserveTokens\":12345"));
        let parsed: AdaptiveTriggerPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, policy);
        parsed.validate().unwrap();

        let missing_reserve = json.replace("\"reserveTokens\":12345,", "");
        assert!(serde_json::from_str::<AdaptiveTriggerPolicy>(&missing_reserve).is_err());

        let dangerous: Result<AdaptiveTriggerPolicy, _> =
            serde_json::from_str(r#"{"reserveTokens":1000,"triggerRatio":0.95,"targetRatio":0.9}"#);
        assert_eq!(
            dangerous.unwrap().validate().unwrap_err().code(),
            ValidationCode::InvalidPolicy
        );
    }

    #[test]
    fn invalid_policies_are_rejected() {
        let invalid = || {
            [
                AdaptiveTriggerPolicy::new().with_trigger_ratio(f64::NAN),
                AdaptiveTriggerPolicy::new().with_trigger_ratio(f64::INFINITY),
                AdaptiveTriggerPolicy::new().with_trigger_ratio(0.0),
                AdaptiveTriggerPolicy::new().with_trigger_ratio(1.0),
                AdaptiveTriggerPolicy::new().with_target_ratio(f64::NAN),
                AdaptiveTriggerPolicy::new().with_target_ratio(1.0),
                AdaptiveTriggerPolicy::new().with_min_gain_ratio(0.0),
                AdaptiveTriggerPolicy::new().with_min_gain_ratio(1.0),
                AdaptiveTriggerPolicy::new().with_max_working_tokens(0),
                AdaptiveTriggerPolicy::new().with_max_working_tokens(HARD_MAX_WORKING_TOKENS + 1),
                AdaptiveTriggerPolicy::new().with_reserve_tokens(0),
                AdaptiveTriggerPolicy::new().with_reserve_tokens(500_000),
                AdaptiveTriggerPolicy::new()
                    .with_trigger_ratio(0.6)
                    .with_target_ratio(0.6),
            ]
        };
        for policy in invalid() {
            assert_eq!(
                policy.validate().unwrap_err().code(),
                ValidationCode::InvalidPolicy,
                "{policy:?}"
            );
        }
        // The hysteresis band must reach the minimum gain: 0.82 -> 0.70 has a
        // gain of about 0.146, below the default 0.20.
        assert_eq!(
            AdaptiveTriggerPolicy::new()
                .with_target_ratio(0.70)
                .validate()
                .unwrap_err()
                .code(),
            ValidationCode::InvalidPolicy
        );
        // A reachable band stays valid.
        AdaptiveTriggerPolicy::new()
            .with_target_ratio(0.60)
            .validate()
            .unwrap();
    }

    #[test]
    fn advertised_contexts_clamp_to_the_hard_cap() {
        for (advertised, expected_effective) in [
            (1_000_000, 400_000),
            (400_000, 400_000),
            (300_000, 300_000),
            (272_000, 272_000),
            (128_000, 128_000),
            (u64::MAX, 400_000),
        ] {
            let policy = AdaptiveTriggerPolicy::new();
            let decision = evaluate_trigger(&policy, &TriggerInputs::new(advertised, 1)).unwrap();
            assert_eq!(decision.advertised_context_tokens(), advertised);
            assert_eq!(decision.effective_context_tokens(), expected_effective);
            assert!(
                decision.effective_context_tokens()
                    <= decision
                        .advertised_context_tokens()
                        .min(HARD_MAX_WORKING_TOKENS)
            );
        }
    }

    #[test]
    fn trigger_formula_matches_required_reference_points() {
        let policy = AdaptiveTriggerPolicy::new().with_reserve_tokens(16_384);
        // 1M advertised -> 400k effective, trigger 328_000 (reserve bound is higher).
        let one_million = evaluate_trigger(&policy, &TriggerInputs::new(1_000_000, 1)).unwrap();
        assert_eq!(one_million.trigger_threshold_tokens(), 328_000);
        assert_eq!(one_million.target_tokens(), 220_000);
        // 300k advertised -> 246_000 trigger.
        let three_hundred = evaluate_trigger(&policy, &TriggerInputs::new(300_000, 1)).unwrap();
        assert_eq!(three_hundred.trigger_threshold_tokens(), 246_000);
        assert_eq!(three_hundred.target_tokens(), 165_000);
        // 272k advertised -> 223_040 trigger.
        let two_hundred_seventy_two =
            evaluate_trigger(&policy, &TriggerInputs::new(272_000, 1)).unwrap();
        assert_eq!(two_hundred_seventy_two.trigger_threshold_tokens(), 223_040);
    }

    #[test]
    fn dynamic_reserve_binds_below_the_ratio_threshold() {
        let policy = AdaptiveTriggerPolicy::new().with_reserve_tokens(16_384);
        let inputs = TriggerInputs::new(128_000, 1)
            .with_requested_max_output(8_192)
            .with_tool_schema_tokens(4_096);
        let decision = evaluate_trigger(&policy, &inputs).unwrap();
        // 16_384 baseline + 8_192 output (below caps and effective/8 = 16_000)
        // + 4_096 tool allowance = 28_672; 128_000 - 28_672 = 99_328 binds
        // below floor(128_000 * 0.82) = 104_960.
        assert_eq!(decision.reserve_tokens(), 28_672);
        assert_eq!(decision.trigger_threshold_tokens(), 99_328);
        assert_eq!(decision.target_tokens(), 70_400);
    }

    #[test]
    fn output_reserve_never_trusts_the_full_claimed_maximum() {
        let policy = AdaptiveTriggerPolicy::new().with_reserve_tokens(1_000);
        let inputs = TriggerInputs::new(400_000, 1).with_requested_max_output(200_000);
        let decision = evaluate_trigger(&policy, &inputs).unwrap();
        assert_eq!(
            decision.reserve_tokens(),
            1_000 + RESERVE_OUTPUT_CAP_TOKENS.min(400_000 / RESERVE_OUTPUT_SHARE_DIVISOR)
        );
        assert_eq!(
            decision.trigger_threshold_tokens(),
            floor_ratio_product(400_000, 0.82).min(400_000 - decision.reserve_tokens())
        );
    }

    #[test]
    fn session_clamp_only_lowers_the_effective_context() {
        let policy = AdaptiveTriggerPolicy::new();
        let lowered = evaluate_trigger(
            &policy,
            &TriggerInputs::new(300_000, 1).with_session_context_cap(100_000),
        )
        .unwrap();
        assert_eq!(lowered.effective_context_tokens(), 100_000);
        assert_eq!(
            lowered.trigger_threshold_tokens(),
            floor_ratio_product(100_000, 0.82)
        );
        // A clamp above the advertised context cannot raise the effective value.
        let raised = evaluate_trigger(
            &policy,
            &TriggerInputs::new(300_000, 1).with_session_context_cap(500_000),
        )
        .unwrap();
        assert_eq!(raised.effective_context_tokens(), 300_000);
    }

    #[test]
    fn trusted_provider_report_replaces_the_host_estimate() {
        let policy = AdaptiveTriggerPolicy::new();
        // Without a report the host estimate drives the decision.
        let below = evaluate_trigger(&policy, &TriggerInputs::new(400_000, 327_999)).unwrap();
        assert!(!below.should_compact());
        assert_eq!(below.evaluated_used_tokens(), 327_999);
        let at = evaluate_trigger(&policy, &TriggerInputs::new(400_000, 328_000)).unwrap();
        assert!(at.should_compact());
        // A trusted provider report above the host estimate drives the decision.
        let provider_high = evaluate_trigger(
            &policy,
            &TriggerInputs::new(400_000, 1).with_provider_reported_total(328_000),
        )
        .unwrap();
        assert!(provider_high.should_compact());
        assert_eq!(provider_high.evaluated_used_tokens(), 328_000);
        // A smaller trusted report outranks a stale larger host estimate
        // instead of being ignored by a max().
        let provider_low = evaluate_trigger(
            &policy,
            &TriggerInputs::new(400_000, 328_000).with_provider_reported_total(100_000),
        )
        .unwrap();
        assert_eq!(provider_low.evaluated_used_tokens(), 100_000);
        assert!(!provider_low.should_compact());
    }

    #[test]
    fn hysteresis_gain_uses_decimal_exact_semantics() {
        // 0.82 - 0.656 = 0.164 and 0.164 / 0.82 is exactly 0.2 in decimal;
        // binary arithmetic yields about 0.19999999999999993 and used to
        // reject this legitimate boundary configuration.
        AdaptiveTriggerPolicy::new()
            .with_trigger_ratio(0.82)
            .with_target_ratio(0.656)
            .validate()
            .unwrap();
        // One step below the exact boundary stays invalid: the decimal gain
        // 163/820 is below 1/5.
        assert_eq!(
            AdaptiveTriggerPolicy::new()
                .with_trigger_ratio(0.82)
                .with_target_ratio(0.657)
                .validate()
                .unwrap_err()
                .code(),
            ValidationCode::InvalidPolicy
        );
    }

    #[test]
    fn reserve_bound_threshold_pulls_the_target_below_itself() {
        // A legal baseline reserve can push the threshold far below the ratio
        // point; the target must follow so the hysteresis band survives.
        let policy = AdaptiveTriggerPolicy::new()
            .with_max_working_tokens(100_000)
            .with_reserve_tokens(99_000);
        let decision = evaluate_trigger(&policy, &TriggerInputs::new(100_000, 1_000)).unwrap();
        assert_eq!(decision.trigger_threshold_tokens(), 1_000);
        // The ratio point (55_000) would sit above the reserve-bound
        // threshold; the target keeps the minimum 20 percent band instead:
        // 1_000 - ceil(1_000 * 0.2) = 800.
        assert_eq!(decision.target_tokens(), 800);
        assert!(decision.should_compact());
        // Compacting down to the target ends the pressure instead of
        // re-triggering immediately.
        let after = evaluate_trigger(
            &policy,
            &TriggerInputs::new(100_000, decision.target_tokens()),
        )
        .unwrap();
        assert!(!after.should_compact());
    }

    #[test]
    fn sub_token_ratio_products_keep_a_positive_threshold() {
        // A legal policy whose exact ratio products floor to zero must not
        // produce a zero threshold: `0 >= 0` would fire at zero usage and
        // compacting to a zero target would immediately re-trigger.
        let policy = AdaptiveTriggerPolicy::new()
            .with_trigger_ratio(0.0004)
            .with_target_ratio(0.0002)
            .with_max_working_tokens(1_000)
            .with_reserve_tokens(1);
        let decision = evaluate_trigger(&policy, &TriggerInputs::new(1_000, 0)).unwrap();
        assert_eq!(decision.trigger_threshold_tokens(), 1);
        // The target keeps the minimum gain below the floored threshold.
        assert_eq!(decision.target_tokens(), 0);
        assert!(decision.target_tokens() < decision.trigger_threshold_tokens());
        assert!(!decision.should_compact());
        // Compacting down to the target ends the pressure instead of looping.
        let after = evaluate_trigger(
            &policy,
            &TriggerInputs::new(1_000, decision.target_tokens()),
        )
        .unwrap();
        assert!(!after.should_compact());
    }

    #[test]
    fn tool_heavy_estimates_trigger_earlier_but_bounded() {
        let policy = AdaptiveTriggerPolicy::new();
        let base = evaluate_trigger(&policy, &TriggerInputs::new(400_000, 300_000)).unwrap();
        assert_eq!(base.trigger_threshold_tokens(), 328_000);
        // 80% tool share without a provider report: 0.8 * 0.25 = 0.2 capped at 0.1.
        let estimated = evaluate_trigger(
            &policy,
            &TriggerInputs::new(400_000, 300_000).with_tool_output_tokens(240_000),
        )
        .unwrap();
        assert_eq!(
            estimated.trigger_threshold_tokens(),
            328_000 - floor_ratio_product(328_000, 0.10)
        );
        // The same tool share with trusted provider usage keeps the threshold.
        let reported = evaluate_trigger(
            &policy,
            &TriggerInputs::new(400_000, 300_000)
                .with_tool_output_tokens(240_000)
                .with_provider_reported_total(300_000),
        )
        .unwrap();
        assert_eq!(reported.trigger_threshold_tokens(), 328_000);
    }

    #[test]
    fn target_below_trigger_keeps_hysteresis_after_compaction() {
        let policy = AdaptiveTriggerPolicy::new();
        let decision = evaluate_trigger(&policy, &TriggerInputs::new(400_000, 328_000)).unwrap();
        assert!(decision.should_compact());
        let after = evaluate_trigger(
            &policy,
            &TriggerInputs::new(400_000, decision.target_tokens()),
        )
        .unwrap();
        assert!(!after.should_compact());
        assert_eq!(after.min_gain_tokens(), ceil_ratio_product(220_000, 0.20));
    }

    #[test]
    fn evaluation_rejects_empty_contexts_and_saturating_reserves() {
        let policy = AdaptiveTriggerPolicy::new();
        let empty = evaluate_trigger(&policy, &TriggerInputs::new(0, 0));
        assert_eq!(empty.unwrap_err().code(), ValidationCode::InvalidInput);
        // A tiny advertised context cannot host the default reserve.
        let saturated = evaluate_trigger(&policy, &TriggerInputs::new(1_000, 1));
        assert_eq!(saturated.unwrap_err().code(), ValidationCode::InvalidPolicy);
        // Overflow-proof usage values are accepted and force compaction.
        let decision = evaluate_trigger(&policy, &TriggerInputs::new(400_000, u64::MAX)).unwrap();
        assert!(decision.should_compact());
        assert_eq!(
            decision.min_gain_tokens(),
            ceil_ratio_product(u64::MAX, 0.20)
        );
    }
}

// Rust guideline compliant 2026-08-26.
