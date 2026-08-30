//! Provider route ownership and immutable usage-stamp contract tests.

use std::sync::{Arc, Barrier};

use mcode_config::{
    ArtifactRef, CanonicalVersion, DefaultRoute, PackId, PluginFamily, ProviderId, Sha256Digest,
    SourceBindingId,
};
use mcode_plugin_host::{
    AuthFingerprint, AuthSlotId, EndpointFingerprint, ModelAlias, ModelId, ModelRouteLease,
    ProviderGeneration, ProviderRouteClaim, ProviderRouteError, ProviderRouteId,
    ProviderRouteLedger, ProviderRouteOwner, RequestId, TokenCount, TurnId, UsageContextSnapshot,
    UsageCounters,
};

const MANAGER_DIGEST: &str =
    "sha256:1111111111111111111111111111111111111111111111111111111111111111";
const PACK_DIGEST: &str = "sha256:2222222222222222222222222222222222222222222222222222222222222222";
const ENDPOINT_DIGEST: &str =
    "sha256:3333333333333333333333333333333333333333333333333333333333333333";
const AUTH_DIGEST: &str = "sha256:4444444444444444444444444444444444444444444444444444444444444444";

fn artifact(version: &str, digest: &str) -> ArtifactRef {
    ArtifactRef::new(
        CanonicalVersion::parse(version).expect("canonical version"),
        Sha256Digest::parse(digest).expect("canonical digest"),
    )
}

fn owner(family: PluginFamily, pack_id: &str, generation: u64) -> ProviderRouteOwner {
    ProviderRouteOwner::new(
        family,
        artifact("2.4.0", MANAGER_DIGEST),
        PackId::parse(pack_id).expect("Pack ID"),
        SourceBindingId::parse("official-release").expect("source binding"),
        artifact("3.7.1", PACK_DIGEST),
        ProviderGeneration::new(generation).expect("generation"),
    )
}

fn claim(provider_id: &str, route_id: &str, auth_slot: &str) -> ProviderRouteClaim {
    ProviderRouteClaim::new(
        ProviderId::parse(provider_id).expect("provider ID"),
        ProviderRouteId::parse(route_id).expect("route ID"),
        AuthSlotId::parse(auth_slot).expect("auth slot"),
        EndpointFingerprint::new(
            Sha256Digest::parse(ENDPOINT_DIGEST).expect("endpoint fingerprint"),
        ),
        AuthFingerprint::new(Sha256Digest::parse(AUTH_DIGEST).expect("auth fingerprint")),
    )
}

fn ledger_with_route() -> (ProviderRouteLedger, ProviderRouteOwner) {
    let ledger = ProviderRouteLedger::new();
    let owner = owner(PluginFamily::Providers, "pi", 17);
    ledger
        .register_owner_claims(
            owner.clone(),
            [claim("minimax-cn", "minimax-cn/messages", "X-Api-Key")],
        )
        .expect("register route");
    (ledger, owner)
}

fn route_lease(ledger: &ProviderRouteLedger, owner: &ProviderRouteOwner) -> ModelRouteLease {
    ledger
        .mint_model_route_lease(
            owner,
            &ProviderRouteId::parse("minimax-cn/messages").expect("route"),
            ModelId::parse("MiniMax-M2.7").expect("current model"),
        )
        .expect("mint lease")
}

fn context_with_lease(
    ledger: &ProviderRouteLedger,
    owner: &ProviderRouteOwner,
    lease: &ModelRouteLease,
    request_id: &str,
    turn_id: &str,
) -> UsageContextSnapshot {
    ledger
        .mint_usage_context(
            owner,
            lease,
            RequestId::parse(request_id).expect("request ID"),
            TurnId::parse(turn_id).expect("turn ID"),
            ModelId::parse("requested/model").expect("requested model"),
            Some(ModelAlias::parse("fast-alias").expect("requested alias")),
        )
        .expect("mint context")
}

fn context(
    ledger: &ProviderRouteLedger,
    owner: &ProviderRouteOwner,
    request_id: &str,
    turn_id: &str,
) -> UsageContextSnapshot {
    let lease = route_lease(ledger, owner);
    context_with_lease(ledger, owner, &lease, request_id, turn_id)
}

#[test]
fn provider_collision_is_atomic() {
    let ledger = ProviderRouteLedger::new();
    let route_owner = owner(PluginFamily::Providers, "pi", 1);
    ledger
        .register_owner_claims(
            route_owner.clone(),
            [claim("provider-a", "route-a", "Auth-A")],
        )
        .expect("baseline registration");
    let before = ledger.snapshot();

    let error = ledger
        .register_owner_claims(
            route_owner,
            [
                claim("provider-fresh", "route-fresh", "Auth-Fresh"),
                claim("provider-a", "route-extra", "Auth-Extra"),
            ],
        )
        .expect_err("provider collision");

    assert_eq!(error, ProviderRouteError::ProviderIdCollision);
    assert_eq!(ledger.snapshot(), before);
}

#[test]
fn route_collision_is_atomic() {
    let ledger = ProviderRouteLedger::new();
    let route_owner = owner(PluginFamily::Providers, "pi", 1);
    ledger
        .register_owner_claims(
            route_owner.clone(),
            [claim("provider-a", "route-a", "Auth-A")],
        )
        .expect("baseline registration");
    let before = ledger.snapshot();

    let error = ledger
        .register_owner_claims(
            route_owner,
            [
                claim("provider-fresh", "route-fresh", "Auth-Fresh"),
                claim("provider-extra", "route-a", "Auth-Extra"),
            ],
        )
        .expect_err("route collision");

    assert_eq!(error, ProviderRouteError::RouteIdCollision);
    assert_eq!(ledger.snapshot(), before);
}

#[test]
fn auth_slot_collision_is_atomic() {
    let ledger = ProviderRouteLedger::new();
    let route_owner = owner(PluginFamily::Providers, "pi", 1);
    ledger
        .register_owner_claims(
            route_owner.clone(),
            [claim("provider-a", "route-a", "Auth-A")],
        )
        .expect("baseline registration");
    let before = ledger.snapshot();

    let error = ledger
        .register_owner_claims(
            route_owner,
            [
                claim("provider-fresh", "route-fresh", "Auth-Fresh"),
                claim("provider-extra", "route-extra", "Auth-A"),
            ],
        )
        .expect_err("auth-slot collision");

    assert_eq!(error, ProviderRouteError::AuthSlotIdCollision);
    assert_eq!(ledger.snapshot(), before);
}

#[test]
fn same_batch_duplicates_are_rejected_without_mutation() {
    let cases = [
        (
            claim("provider-a", "route-a", "Auth-A"),
            claim("provider-a", "route-b", "Auth-B"),
            ProviderRouteError::DuplicateProviderId,
        ),
        (
            claim("provider-a", "route-a", "Auth-A"),
            claim("provider-b", "route-a", "Auth-B"),
            ProviderRouteError::DuplicateRouteId,
        ),
        (
            claim("provider-a", "route-a", "Auth-A"),
            claim("provider-b", "route-b", "Auth-A"),
            ProviderRouteError::DuplicateAuthSlotId,
        ),
    ];

    for (first, second, expected) in cases {
        let ledger = ProviderRouteLedger::new();
        let before = ledger.snapshot();
        let error = ledger
            .register_owner_claims(owner(PluginFamily::Providers, "pi", 1), [first, second])
            .expect_err("same-batch duplicate");
        assert_eq!(error, expected);
        assert_eq!(ledger.snapshot(), before);
    }
}

#[test]
fn only_the_canonical_providers_family_can_register() {
    for family in PluginFamily::ALL {
        let ledger = ProviderRouteLedger::new();
        let result = ledger.register_owner_claims(
            owner(family, "pi", 1),
            [claim("provider-a", "route-a", "Auth-A")],
        );
        if family == PluginFamily::Providers {
            assert!(result.is_ok(), "Providers must own provider routes");
            assert_eq!(ledger.snapshot().route_count(), 1);
        } else {
            assert_eq!(
                result.expect_err("non-Providers family"),
                ProviderRouteError::WrongManagerFamily
            );
            assert!(ledger.snapshot().is_empty());
        }
    }
}

#[test]
fn identity_strings_enforce_frozen_grammars() {
    assert!(ProviderId::parse("provider-1").is_ok());
    assert!(ProviderId::parse("Provider-1").is_err());
    assert!(ProviderId::parse("provider--1").is_err());
    assert!(ProviderId::parse("p".repeat(65)).is_err());

    assert!(ProviderRouteId::parse("provider/messages:v1@exact").is_ok());
    assert!(ProviderRouteId::parse("route with space").is_err());
    assert!(ProviderRouteId::parse("r".repeat(257)).is_err());

    assert!(AuthSlotId::parse("X-Api-Key").is_ok());
    assert_ne!(
        AuthSlotId::parse("X-Api-Key").expect("mixed-case auth slot"),
        AuthSlotId::parse("x-api-key").expect("lowercase auth slot")
    );
    assert!(AuthSlotId::parse("X_Api_Key").is_err());
    assert!(AuthSlotId::parse("A".repeat(65)).is_err());

    assert!(ModelId::parse("model/v1@exact").is_ok());
    assert!(ModelAlias::parse("latest:model").is_ok());
    assert!(ModelId::parse("model\tname").is_err());
    assert!(ModelAlias::parse("m".repeat(257)).is_err());

    assert!(RequestId::parse("req:01-ab_cd.ef").is_ok());
    assert!(TurnId::parse("turn-01").is_ok());
    assert!(RequestId::parse("req/01").is_err());
    assert!(TurnId::parse("turn 01").is_err());
    assert!(RequestId::parse("r".repeat(129)).is_err());
}

#[test]
fn canonical_provider_id_crosses_config_and_host_boundaries() {
    let provider_id = ProviderId::parse("synthetic-provider").expect("provider ID");
    let default_route = DefaultRoute::new(provider_id.clone(), "model/v1").expect("default route");
    let route_claim = ProviderRouteClaim::new(
        provider_id,
        ProviderRouteId::parse("synthetic-provider/messages").expect("route ID"),
        AuthSlotId::parse("Bearer-Token").expect("auth slot"),
        EndpointFingerprint::new(
            Sha256Digest::parse(ENDPOINT_DIGEST).expect("endpoint fingerprint"),
        ),
        AuthFingerprint::new(Sha256Digest::parse(AUTH_DIGEST).expect("auth fingerprint")),
    );

    assert_eq!(default_route.provider_id(), route_claim.provider_id());

    let maximum = format!("a{}z", "x".repeat(62));
    let config_id = ProviderId::parse(&maximum).expect("64-byte provider ID");
    let host_id: mcode_plugin_host::ProviderId = config_id.clone();
    let boundary_route = DefaultRoute::new(config_id, "model/v1").expect("64-byte default route");
    let boundary_claim = ProviderRouteClaim::new(
        host_id,
        ProviderRouteId::parse("boundary/messages").expect("route ID"),
        AuthSlotId::parse("Boundary-Auth").expect("auth slot"),
        EndpointFingerprint::new(
            Sha256Digest::parse(ENDPOINT_DIGEST).expect("endpoint fingerprint"),
        ),
        AuthFingerprint::new(Sha256Digest::parse(AUTH_DIGEST).expect("auth fingerprint")),
    );
    assert_eq!(boundary_route.provider_id(), boundary_claim.provider_id());
    assert!(ProviderId::parse("x".repeat(256)).is_err());
}

#[test]
fn numeric_identities_enforce_frozen_bounds() {
    assert_eq!(ProviderGeneration::new(1).expect("generation").get(), 1);
    assert!(ProviderGeneration::new(0).is_err());
    assert!(ProviderGeneration::new(i64::MAX as u64).is_ok());
    assert!(ProviderGeneration::new(i64::MAX as u64 + 1).is_err());

    assert_eq!(TokenCount::new(0).expect("zero tokens").get(), 0);
    assert!(TokenCount::new(i64::MAX as u64).is_ok());
    assert!(TokenCount::new(i64::MAX as u64 + 1).is_err());
}

#[test]
fn unknown_route_cannot_mint_a_lease() {
    let (ledger, route_owner) = ledger_with_route();
    let error = ledger
        .mint_model_route_lease(
            &route_owner,
            &ProviderRouteId::parse("unknown/route").expect("route"),
            ModelId::parse("model").expect("model"),
        )
        .expect_err("unknown route");
    assert_eq!(error, ProviderRouteError::UnknownRoute);
}

#[test]
fn lease_binding_rejects_wrong_owner() {
    let (ledger, route_owner) = ledger_with_route();
    let lease = ledger
        .mint_model_route_lease(
            &route_owner,
            &ProviderRouteId::parse("minimax-cn/messages").expect("route"),
            ModelId::parse("MiniMax-M2.7").expect("model"),
        )
        .expect("lease");
    let error = ledger
        .mint_usage_context(
            &owner(PluginFamily::Providers, "synthetic", 17),
            &lease,
            RequestId::parse("req-wrong-owner").expect("request"),
            TurnId::parse("turn-1").expect("turn"),
            ModelId::parse("model").expect("model"),
            None,
        )
        .expect_err("wrong owner");
    assert_eq!(error, ProviderRouteError::OwnerMismatch);
}

#[test]
fn lease_binding_rejects_stale_generation() {
    let (ledger, route_owner) = ledger_with_route();
    let lease = ledger
        .mint_model_route_lease(
            &route_owner,
            &ProviderRouteId::parse("minimax-cn/messages").expect("route"),
            ModelId::parse("MiniMax-M2.7").expect("model"),
        )
        .expect("lease");
    let error = ledger
        .mint_usage_context(
            &owner(PluginFamily::Providers, "pi", 18),
            &lease,
            RequestId::parse("req-stale").expect("request"),
            TurnId::parse("turn-1").expect("turn"),
            ModelId::parse("model").expect("model"),
            None,
        )
        .expect_err("stale generation");
    assert_eq!(error, ProviderRouteError::StaleGeneration);
}

#[test]
fn lease_cannot_cross_ledger_boundaries() {
    let (first, route_owner) = ledger_with_route();
    let lease = first
        .mint_model_route_lease(
            &route_owner,
            &ProviderRouteId::parse("minimax-cn/messages").expect("route"),
            ModelId::parse("MiniMax-M2.7").expect("model"),
        )
        .expect("lease");
    let second = ProviderRouteLedger::new();
    second
        .register_owner_claims(
            route_owner.clone(),
            [claim("minimax-cn", "minimax-cn/messages", "X-Api-Key")],
        )
        .expect("same visible registration");

    let error = second
        .mint_usage_context(
            &route_owner,
            &lease,
            RequestId::parse("req-foreign").expect("request"),
            TurnId::parse("turn-1").expect("turn"),
            ModelId::parse("model").expect("model"),
            None,
        )
        .expect_err("foreign lease");
    assert_eq!(error, ProviderRouteError::ForeignStamp);
}

#[test]
fn resolved_model_rejects_wrong_turn_without_mutation() {
    let (ledger, route_owner) = ledger_with_route();
    let context = context(&ledger, &route_owner, "req-turn", "turn-bound");
    let wrong_turn = TurnId::parse("turn-wrong").expect("turn");
    let error = ledger
        .record_resolved_model(
            &context,
            context.request_id(),
            &wrong_turn,
            ModelId::parse("wrong/model").expect("resolved model"),
        )
        .expect_err("wrong turn");
    assert_eq!(error, ProviderRouteError::TurnMismatch);

    let updated = ledger
        .record_resolved_model(
            &context,
            context.request_id(),
            context.turn_id(),
            ModelId::parse("right/model").expect("resolved model"),
        )
        .expect("correct turn remains available");
    assert_eq!(
        updated.resolved_model().expect("resolved model").as_str(),
        "right/model"
    );
}

#[test]
fn terminal_rejects_wrong_request_without_consuming_terminal() {
    let (ledger, route_owner) = ledger_with_route();
    let context = context(&ledger, &route_owner, "req-bound", "turn-request");
    let wrong_request = RequestId::parse("req-wrong").expect("request");
    let error = ledger
        .mint_terminal_sample(
            &context,
            &wrong_request,
            context.turn_id(),
            UsageCounters::none(),
        )
        .expect_err("wrong request");
    assert_eq!(error, ProviderRouteError::RequestMismatch);

    let sample = ledger
        .mint_terminal_sample(
            &context,
            context.request_id(),
            context.turn_id(),
            UsageCounters::none(),
        )
        .expect("correct request remains terminal-eligible");
    assert_eq!(sample.context().request_id().as_str(), "req-bound");
}

#[test]
fn optional_usage_fields_remain_absent() {
    let (ledger, route_owner) = ledger_with_route();
    let lease = ledger
        .mint_model_route_lease(
            &route_owner,
            &ProviderRouteId::parse("minimax-cn/messages").expect("route"),
            ModelId::parse("MiniMax-M2.7").expect("current model"),
        )
        .expect("mint lease");
    let context = ledger
        .mint_usage_context(
            &route_owner,
            &lease,
            RequestId::parse("req-optional").expect("request"),
            TurnId::parse("turn-optional").expect("turn"),
            ModelId::parse("requested/model").expect("requested model"),
            None,
        )
        .expect("mint context");
    let sample = ledger
        .mint_terminal_sample(
            &context,
            context.request_id(),
            context.turn_id(),
            UsageCounters::none(),
        )
        .expect("terminal sample");

    assert!(sample.context().requested_alias().is_none());
    assert!(sample.context().resolved_model().is_none());
    assert!(sample.counters().input_tokens().is_none());
    assert!(sample.counters().output_tokens().is_none());
    assert!(sample.counters().cache_read_tokens().is_none());
    assert!(sample.counters().cache_write_tokens().is_none());
}

#[test]
fn terminal_sample_preserves_exact_stamped_fields() {
    let (ledger, route_owner) = ledger_with_route();
    let context = context(&ledger, &route_owner, "req-exact", "turn-exact");
    let context = ledger
        .record_resolved_model(
            &context,
            context.request_id(),
            context.turn_id(),
            ModelId::parse("wire/resolved-model").expect("resolved model"),
        )
        .expect("record resolved model");
    let counters = UsageCounters::new(
        Some(TokenCount::new(101).expect("input")),
        Some(TokenCount::new(37).expect("output")),
        None,
        Some(TokenCount::new(11).expect("cache write")),
    );
    let sample = ledger
        .mint_terminal_sample(&context, context.request_id(), context.turn_id(), counters)
        .expect("terminal sample");

    let stamped_context = sample.context();
    let lease = stamped_context.lease();
    let ownership = lease.ownership();
    let stamped_owner = ownership.owner();
    let stamped_claim = ownership.claim();
    assert_eq!(stamped_owner.manager_family(), PluginFamily::Providers);
    assert_eq!(stamped_owner.manager_id(), "com.mcode.providers");
    assert_eq!(stamped_owner.manager_artifact().version().as_str(), "2.4.0");
    assert_eq!(
        stamped_owner.manager_artifact().digest().as_str(),
        MANAGER_DIGEST
    );
    assert_eq!(stamped_owner.pack_id().as_str(), "pi");
    assert_eq!(stamped_owner.pack_source().as_str(), "official-release");
    assert_eq!(stamped_owner.pack_artifact().version().as_str(), "3.7.1");
    assert_eq!(stamped_owner.pack_artifact().digest().as_str(), PACK_DIGEST);
    assert_eq!(stamped_owner.generation().get(), 17);
    assert_eq!(stamped_claim.provider_id().as_str(), "minimax-cn");
    assert_eq!(stamped_claim.route_id().as_str(), "minimax-cn/messages");
    assert_eq!(stamped_claim.auth_slot_id().as_str(), "X-Api-Key");
    assert_eq!(
        stamped_claim.endpoint_fingerprint().digest().as_str(),
        ENDPOINT_DIGEST
    );
    assert_eq!(
        stamped_claim.auth_fingerprint().digest().as_str(),
        AUTH_DIGEST
    );
    assert_eq!(lease.current_model().as_str(), "MiniMax-M2.7");
    assert_eq!(stamped_context.request_id().as_str(), "req-exact");
    assert_eq!(stamped_context.turn_id().as_str(), "turn-exact");
    assert_eq!(
        stamped_context.requested_model().as_str(),
        "requested/model"
    );
    assert_eq!(
        stamped_context
            .requested_alias()
            .expect("requested alias")
            .as_str(),
        "fast-alias"
    );
    assert_eq!(
        stamped_context
            .resolved_model()
            .expect("resolved model")
            .as_str(),
        "wire/resolved-model"
    );
    assert_eq!(
        sample.counters().input_tokens().map(TokenCount::get),
        Some(101)
    );
    assert_eq!(
        sample.counters().output_tokens().map(TokenCount::get),
        Some(37)
    );
    assert!(sample.counters().cache_read_tokens().is_none());
    assert_eq!(
        sample.counters().cache_write_tokens().map(TokenCount::get),
        Some(11)
    );
}

#[test]
fn concurrent_terminal_mint_has_exactly_one_winner() {
    let (ledger, route_owner) = ledger_with_route();
    let context = Arc::new(context(
        &ledger,
        &route_owner,
        "req-terminal",
        "turn-terminal",
    ));
    let barrier = Arc::new(Barrier::new(2));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let worker_ledger = ledger.clone();
        let worker_context = Arc::clone(&context);
        let worker_barrier = Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            worker_barrier.wait();
            worker_ledger.mint_terminal_sample(
                &worker_context,
                worker_context.request_id(),
                worker_context.turn_id(),
                UsageCounters::none(),
            )
        }));
    }
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().expect("terminal worker"))
        .collect();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .collect::<Vec<_>>(),
        vec![&ProviderRouteError::RequestAlreadyTerminal]
    );
}

#[test]
fn resolved_model_is_rejected_after_terminal() {
    let (ledger, route_owner) = ledger_with_route();
    let context = context(&ledger, &route_owner, "req-finished", "turn-finished");
    ledger
        .mint_terminal_sample(
            &context,
            context.request_id(),
            context.turn_id(),
            UsageCounters::none(),
        )
        .expect("terminal sample");

    let error = ledger
        .record_resolved_model(
            &context,
            context.request_id(),
            context.turn_id(),
            ModelId::parse("late/model").expect("resolved model"),
        )
        .expect_err("resolved after terminal");
    assert_eq!(error, ProviderRouteError::RequestAlreadyTerminal);
}

#[test]
fn terminal_context_id_can_be_reused_without_reactivating_old_clones() {
    let (ledger, route_owner) = ledger_with_route();
    let lease = route_lease(&ledger, &route_owner);
    let old = context_with_lease(&ledger, &route_owner, &lease, "req-reused", "turn-old");
    let old_clone = old.clone();
    ledger
        .mint_terminal_sample(&old, old.request_id(), old.turn_id(), UsageCounters::none())
        .expect("old terminal");

    let new = context_with_lease(&ledger, &route_owner, &lease, "req-reused", "turn-new");
    assert_eq!(
        ledger
            .mint_terminal_sample(
                &old_clone,
                old_clone.request_id(),
                old_clone.turn_id(),
                UsageCounters::none(),
            )
            .expect_err("old terminal clone"),
        ProviderRouteError::RequestAlreadyTerminal
    );
    assert_eq!(
        ledger
            .record_resolved_model(
                &old_clone,
                old_clone.request_id(),
                old_clone.turn_id(),
                ModelId::parse("late/old-model").expect("model"),
            )
            .expect_err("old resolved clone"),
        ProviderRouteError::RequestAlreadyTerminal
    );
    ledger
        .mint_terminal_sample(&new, new.request_id(), new.turn_id(), UsageCounters::none())
        .expect("new request terminal");
}

#[test]
fn stale_pre_resolved_snapshot_does_not_consume_terminal() {
    let (ledger, route_owner) = ledger_with_route();
    let initial = context(
        &ledger,
        &route_owner,
        "req-stale-context",
        "turn-stale-context",
    );
    let resolved = ledger
        .record_resolved_model(
            &initial,
            initial.request_id(),
            initial.turn_id(),
            ModelId::parse("resolved/model").expect("resolved model"),
        )
        .expect("resolved context");

    assert_eq!(
        ledger
            .mint_terminal_sample(
                &initial,
                initial.request_id(),
                initial.turn_id(),
                UsageCounters::none(),
            )
            .expect_err("stale initial context"),
        ProviderRouteError::StaleUsageContext
    );
    ledger
        .mint_terminal_sample(
            &resolved,
            resolved.request_id(),
            resolved.turn_id(),
            UsageCounters::none(),
        )
        .expect("resolved snapshot remains terminal-eligible");
}

#[test]
fn resolved_and_terminal_race_has_one_linearized_outcome() {
    for iteration in 0..32 {
        let (ledger, route_owner) = ledger_with_route();
        let initial = context(
            &ledger,
            &route_owner,
            &format!("req-resolve-race-{iteration}"),
            "turn-resolve-race",
        );
        let barrier = Arc::new(Barrier::new(3));

        let resolved_ledger = ledger.clone();
        let resolved_context = initial.clone();
        let resolved_barrier = Arc::clone(&barrier);
        let resolved_worker = std::thread::spawn(move || {
            resolved_barrier.wait();
            resolved_ledger.record_resolved_model(
                &resolved_context,
                resolved_context.request_id(),
                resolved_context.turn_id(),
                ModelId::parse("resolved/race-model").expect("resolved model"),
            )
        });

        let terminal_ledger = ledger.clone();
        let terminal_context = initial.clone();
        let terminal_barrier = Arc::clone(&barrier);
        let terminal_worker = std::thread::spawn(move || {
            terminal_barrier.wait();
            terminal_ledger.mint_terminal_sample(
                &terminal_context,
                terminal_context.request_id(),
                terminal_context.turn_id(),
                UsageCounters::none(),
            )
        });

        barrier.wait();
        let resolved = resolved_worker.join().expect("resolved worker");
        let terminal = terminal_worker.join().expect("terminal worker");
        match (resolved, terminal) {
            (Ok(updated), Err(ProviderRouteError::StaleUsageContext)) => {
                ledger
                    .mint_terminal_sample(
                        &updated,
                        updated.request_id(),
                        updated.turn_id(),
                        UsageCounters::none(),
                    )
                    .expect("updated context terminal retry");
            }
            (Err(ProviderRouteError::RequestAlreadyTerminal), Ok(_sample)) => {}
            (resolved, terminal) => {
                panic!("unexpected race result: resolved={resolved:?}, terminal={terminal:?}")
            }
        }
    }
}
