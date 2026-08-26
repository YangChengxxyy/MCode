// Rust guideline compliant 2026-08-26.

//! Public-API coverage for the sealed adaptive trigger foundation.

use mcode_compaction::{
    AdaptiveTriggerPolicy, HARD_MAX_WORKING_TOKENS, TriggerInputs, ValidationCode, evaluate_trigger,
};

#[test]
fn advertised_one_million_clamps_to_four_hundred_thousand_effective() {
    let policy = AdaptiveTriggerPolicy::new().with_reserve_tokens(16_384);
    let decision = evaluate_trigger(&policy, &TriggerInputs::new(1_000_000, 330_000)).unwrap();
    assert_eq!(decision.advertised_context_tokens(), 1_000_000);
    assert_eq!(decision.effective_context_tokens(), 400_000);
    assert_eq!(decision.effective_context_tokens(), HARD_MAX_WORKING_TOKENS);
    // min(floor(400_000 * 0.82), 400_000 - 16_384) = min(328_000, 383_616).
    assert_eq!(decision.trigger_threshold_tokens(), 328_000);
    assert_eq!(decision.target_tokens(), 220_000);
    assert!(decision.should_compact());
}

#[test]
fn three_hundred_k_and_two_hundred_seventy_two_k_reference_points() {
    let policy = AdaptiveTriggerPolicy::new().with_reserve_tokens(16_384);
    let three_hundred = evaluate_trigger(&policy, &TriggerInputs::new(300_000, 1)).unwrap();
    assert_eq!(three_hundred.effective_context_tokens(), 300_000);
    assert_eq!(three_hundred.trigger_threshold_tokens(), 246_000);
    assert_eq!(three_hundred.target_tokens(), 165_000);
    let two_hundred_seventy_two =
        evaluate_trigger(&policy, &TriggerInputs::new(272_000, 1)).unwrap();
    assert_eq!(two_hundred_seventy_two.effective_context_tokens(), 272_000);
    assert_eq!(two_hundred_seventy_two.trigger_threshold_tokens(), 223_040);
}

#[test]
fn one_hundred_twenty_eight_k_uses_the_dynamic_reserve_bound() {
    let policy = AdaptiveTriggerPolicy::new().with_reserve_tokens(16_384);
    let inputs = TriggerInputs::new(128_000, 1)
        .with_requested_max_output(8_192)
        .with_tool_schema_tokens(4_096);
    let decision = evaluate_trigger(&policy, &inputs).unwrap();
    // 16_384 + 8_192 + 4_096 = 28_672; 128_000 - 28_672 = 99_328 binds below
    // floor(128_000 * 0.82) = 104_960.
    assert_eq!(decision.reserve_tokens(), 28_672);
    assert_eq!(decision.trigger_threshold_tokens(), 99_328);
    assert_eq!(decision.target_tokens(), 70_400);
}

#[test]
fn session_clamp_lowers_the_cap_and_never_raises_it() {
    let policy = AdaptiveTriggerPolicy::new().with_reserve_tokens(1_000);
    let lowered = evaluate_trigger(
        &policy,
        &TriggerInputs::new(300_000, 1).with_session_context_cap(128_000),
    )
    .unwrap();
    assert_eq!(lowered.effective_context_tokens(), 128_000);
    assert_eq!(lowered.trigger_threshold_tokens(), 104_960);
    let raised = evaluate_trigger(
        &policy,
        &TriggerInputs::new(300_000, 1).with_session_context_cap(1_000_000),
    )
    .unwrap();
    assert_eq!(raised.effective_context_tokens(), 300_000);
}

#[test]
fn evaluation_is_stateless_so_the_host_owns_the_single_retry() {
    let policy = AdaptiveTriggerPolicy::new().with_reserve_tokens(16_384);
    let inputs = TriggerInputs::new(1_000_000, 340_000).with_session_context_cap(400_000);
    // Repeated evaluation with the same snapshot returns the same decision;
    // this crate keeps no retry state, so an upper layer may compact and retry
    // once without this foundation looping on its own.
    let first = evaluate_trigger(&policy, &inputs).unwrap();
    let second = evaluate_trigger(&policy, &inputs).unwrap();
    assert_eq!(first, second);
    assert!(first.should_compact());
    // After the host compacts down to the hysteresis target the pressure ends.
    let after = evaluate_trigger(
        &policy,
        &TriggerInputs::new(1_000_000, first.target_tokens()),
    )
    .unwrap();
    assert!(!after.should_compact());
}

#[test]
fn json_policy_configuration_roundtrips_and_rejects_dangerous_values() {
    let policy = AdaptiveTriggerPolicy::new()
        .with_trigger_ratio(0.8)
        .with_target_ratio(0.5)
        .with_max_working_tokens(200_000)
        .with_reserve_tokens(8_192)
        .with_min_gain_ratio(0.25);
    let json = serde_json::to_string(&policy).unwrap();
    assert!(json.contains("\"triggerRatio\":0.8"));
    assert!(json.contains("\"maxWorkingTokens\":200000"));
    let parsed: AdaptiveTriggerPolicy = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, policy);
    let decision = evaluate_trigger(
        &parsed,
        &TriggerInputs::new(1_000_000, 150_000).with_session_context_cap(200_000),
    )
    .unwrap();
    assert_eq!(decision.trigger_threshold_tokens(), 160_000);

    for dangerous in [
        r#"{"reserveTokens":8192,"triggerRatio":0.95,"targetRatio":0.94}"#,
        r#"{"reserveTokens":8192,"triggerRatio":0.5,"targetRatio":0.5}"#,
        r#"{"reserveTokens":8192,"maxWorkingTokens":400001}"#,
        r#"{"reserveTokens":0}"#,
        r#"{"reserveTokens":8192,"minGainRatio":1.5}"#,
    ] {
        let parsed: AdaptiveTriggerPolicy = serde_json::from_str(dangerous).unwrap();
        assert_eq!(
            parsed.validate().unwrap_err().code(),
            ValidationCode::InvalidPolicy,
            "{dangerous}"
        );
    }
    // reserveTokens is a required auto-trigger input with no schema default.
    assert!(
        serde_json::from_str::<AdaptiveTriggerPolicy>(r#"{"triggerRatio":0.8,"targetRatio":0.5}"#)
            .is_err()
    );
}

#[test]
fn provider_usage_and_tool_share_inputs_have_bounded_effect() {
    let policy = AdaptiveTriggerPolicy::new().with_reserve_tokens(16_384);
    // Trusted provider usage outranks a stale host estimate.
    let reported = evaluate_trigger(
        &policy,
        &TriggerInputs::new(400_000, 100_000).with_provider_reported_total(328_000),
    )
    .unwrap();
    assert_eq!(reported.evaluated_used_tokens(), 328_000);
    assert!(reported.should_compact());
    // A smaller trusted report replaces a stale larger host estimate instead
    // of being ignored by a max().
    let provider_smaller = evaluate_trigger(
        &policy,
        &TriggerInputs::new(400_000, 328_000).with_provider_reported_total(100_000),
    )
    .unwrap();
    assert_eq!(provider_smaller.evaluated_used_tokens(), 100_000);
    assert!(!provider_smaller.should_compact());
    // Tool-heavy usage without a provider report lowers the threshold by at
    // most ten percent; it never raises it.
    let plain = evaluate_trigger(&policy, &TriggerInputs::new(400_000, 300_000)).unwrap();
    let tool_heavy = evaluate_trigger(
        &policy,
        &TriggerInputs::new(400_000, 300_000).with_tool_output_tokens(300_000),
    )
    .unwrap();
    assert!(tool_heavy.trigger_threshold_tokens() < plain.trigger_threshold_tokens());
    assert!(tool_heavy.trigger_threshold_tokens() >= 328_000 - 32_800);
    assert_eq!(plain.trigger_threshold_tokens(), 328_000);
}

#[test]
fn reserve_bound_threshold_keeps_the_hysteresis_band() {
    let policy = AdaptiveTriggerPolicy::new()
        .with_max_working_tokens(100_000)
        .with_reserve_tokens(99_000);
    let decision = evaluate_trigger(&policy, &TriggerInputs::new(100_000, 1_000)).unwrap();
    assert_eq!(decision.trigger_threshold_tokens(), 1_000);
    // The ratio point (floor(100_000 * 0.55) = 55_000) would sit above the
    // reserve-bound threshold; the target follows the threshold down to keep
    // the band and the minimum 20 percent gain.
    assert_eq!(decision.target_tokens(), 800);
    assert!(decision.should_compact());
    let after = evaluate_trigger(
        &policy,
        &TriggerInputs::new(100_000, decision.target_tokens()),
    )
    .unwrap();
    assert!(!after.should_compact());
}

/// Legal sub-token ratio products must never produce a zero threshold: the
/// threshold is floored at one token so zero usage never triggers and a
/// compacted session cannot re-trigger immediately.
#[test]
fn sub_token_ratio_products_keep_a_positive_threshold() {
    let policy = AdaptiveTriggerPolicy::new()
        .with_trigger_ratio(0.0004)
        .with_target_ratio(0.0002)
        .with_max_working_tokens(1_000)
        .with_reserve_tokens(1);
    let decision = evaluate_trigger(&policy, &TriggerInputs::new(1_000, 0)).unwrap();
    assert_eq!(decision.trigger_threshold_tokens(), 1);
    assert!(decision.target_tokens() < decision.trigger_threshold_tokens());
    assert!(!decision.should_compact());
    let after = evaluate_trigger(
        &policy,
        &TriggerInputs::new(1_000, decision.target_tokens()),
    )
    .unwrap();
    assert!(!after.should_compact());
}

#[test]
fn boundary_and_overflow_inputs_fail_closed_or_saturate() {
    let policy = AdaptiveTriggerPolicy::new().with_reserve_tokens(16_384);
    assert_eq!(
        evaluate_trigger(&policy, &TriggerInputs::new(0, 0))
            .unwrap_err()
            .code(),
        ValidationCode::InvalidInput
    );
    assert_eq!(
        evaluate_trigger(&policy, &TriggerInputs::new(999, 1))
            .unwrap_err()
            .code(),
        ValidationCode::InvalidPolicy
    );
    let saturated = evaluate_trigger(&policy, &TriggerInputs::new(u64::MAX, u64::MAX)).unwrap();
    assert_eq!(saturated.effective_context_tokens(), 400_000);
    assert!(saturated.should_compact());
}
