//! Public provider-route and live-context bound contract tests.

use mcode_config::{
    ArtifactRef, CanonicalVersion, PackId, PluginFamily, ProviderId, Sha256Digest, SourceBindingId,
};
use mcode_plugin_host::{
    AuthFingerprint, AuthSlotId, EndpointFingerprint, MAX_PROVIDER_ROUTE_CLAIMS,
    MAX_PROVIDER_ROUTE_CLAIMS_PER_BATCH, MAX_USAGE_CONTEXTS_PER_LEDGER, ModelId, ModelRouteLease,
    ProviderGeneration, ProviderRouteClaim, ProviderRouteError, ProviderRouteId,
    ProviderRouteLedger, ProviderRouteOwner, RequestId, TurnId, UsageContextSnapshot,
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

fn owner() -> ProviderRouteOwner {
    ProviderRouteOwner::new(
        PluginFamily::Providers,
        artifact("2.4.0", MANAGER_DIGEST),
        PackId::parse("pi").expect("Pack ID"),
        SourceBindingId::parse("official-release").expect("source binding"),
        artifact("3.7.1", PACK_DIGEST),
        ProviderGeneration::new(1).expect("generation"),
    )
}

fn claim(index: usize) -> ProviderRouteClaim {
    ProviderRouteClaim::new(
        ProviderId::parse(format!("provider-{index:04}")).expect("provider ID"),
        ProviderRouteId::parse(format!("route-{index:04}")).expect("route ID"),
        AuthSlotId::parse(format!("Auth-{index:04}")).expect("auth slot"),
        EndpointFingerprint::new(
            Sha256Digest::parse(ENDPOINT_DIGEST).expect("endpoint fingerprint"),
        ),
        AuthFingerprint::new(Sha256Digest::parse(AUTH_DIGEST).expect("auth fingerprint")),
    )
}

fn claim_with_ids(provider: &str, route: &str, auth_slot: &str) -> ProviderRouteClaim {
    ProviderRouteClaim::new(
        ProviderId::parse(provider).expect("provider ID"),
        ProviderRouteId::parse(route).expect("route ID"),
        AuthSlotId::parse(auth_slot).expect("auth slot"),
        EndpointFingerprint::new(
            Sha256Digest::parse(ENDPOINT_DIGEST).expect("endpoint fingerprint"),
        ),
        AuthFingerprint::new(Sha256Digest::parse(AUTH_DIGEST).expect("auth fingerprint")),
    )
}

fn ledger_with_lease() -> (ProviderRouteLedger, ProviderRouteOwner, ModelRouteLease) {
    let ledger = ProviderRouteLedger::new();
    let route_owner = owner();
    ledger
        .register_owner_claims(route_owner.clone(), [claim(0)])
        .expect("register route");
    let lease = ledger
        .mint_model_route_lease(
            &route_owner,
            &ProviderRouteId::parse("route-0000").expect("route ID"),
            ModelId::parse("current/model").expect("current model"),
        )
        .expect("route lease");
    (ledger, route_owner, lease)
}

fn mint_context(
    ledger: &ProviderRouteLedger,
    route_owner: &ProviderRouteOwner,
    lease: &ModelRouteLease,
    request_id: &str,
) -> Result<UsageContextSnapshot, ProviderRouteError> {
    ledger.mint_usage_context(
        route_owner,
        lease,
        RequestId::parse(request_id).expect("request ID"),
        TurnId::parse("turn-bounds").expect("turn ID"),
        ModelId::parse("requested/model").expect("requested model"),
        None,
    )
}

#[test]
fn registration_batch_bounds_are_exact_and_atomic() {
    let ledger = ProviderRouteLedger::new();
    let route_owner = owner();
    let empty = ledger.snapshot();

    assert_eq!(
        ledger
            .register_owner_claims(route_owner.clone(), Vec::<ProviderRouteClaim>::new())
            .expect_err("empty batch"),
        ProviderRouteError::EmptyClaimBatch
    );
    assert_eq!(ledger.snapshot(), empty);

    let exact = (0..MAX_PROVIDER_ROUTE_CLAIMS_PER_BATCH)
        .map(claim)
        .collect::<Vec<_>>();
    ledger
        .register_owner_claims(route_owner.clone(), exact)
        .expect("exact batch bound");
    assert_eq!(ledger.snapshot().route_count(), 256);

    let before_oversized = ledger.snapshot();
    let oversized = (10_000..10_000 + MAX_PROVIDER_ROUTE_CLAIMS_PER_BATCH + 1)
        .map(claim)
        .collect::<Vec<_>>();
    assert_eq!(
        ledger
            .register_owner_claims(route_owner, oversized)
            .expect_err("257th claim"),
        ProviderRouteError::ClaimBatchTooLarge
    );
    assert_eq!(ledger.snapshot(), before_oversized);
}

#[test]
fn route_capacity_and_error_priority_are_exact_and_atomic() {
    let ledger = ProviderRouteLedger::new();
    let route_owner = owner();
    for start in (0..MAX_PROVIDER_ROUTE_CLAIMS).step_by(MAX_PROVIDER_ROUTE_CLAIMS_PER_BATCH) {
        let claims = (start..start + MAX_PROVIDER_ROUTE_CLAIMS_PER_BATCH)
            .map(claim)
            .collect::<Vec<_>>();
        ledger
            .register_owner_claims(route_owner.clone(), claims)
            .expect("fill route capacity");
    }
    assert_eq!(ledger.snapshot().route_count(), 4_096);
    let full = ledger.snapshot();

    assert_eq!(
        ledger
            .register_owner_claims(route_owner.clone(), [claim(MAX_PROVIDER_ROUTE_CLAIMS)])
            .expect_err("4097th route"),
        ProviderRouteError::RouteCapacityExceeded
    );
    assert_eq!(ledger.snapshot(), full);

    let duplicate_batch = [
        claim_with_ids("duplicate-provider", "duplicate-route-a", "Duplicate-A"),
        claim_with_ids("duplicate-provider", "duplicate-route-b", "Duplicate-B"),
    ];
    assert_eq!(
        ledger
            .register_owner_claims(route_owner.clone(), duplicate_batch)
            .expect_err("duplicate before capacity"),
        ProviderRouteError::DuplicateProviderId
    );
    assert_eq!(ledger.snapshot(), full);

    let colliding = claim_with_ids("provider-0000", "fresh-route", "Fresh-Auth");
    assert_eq!(
        ledger
            .register_owner_claims(route_owner, [colliding])
            .expect_err("collision before capacity"),
        ProviderRouteError::ProviderIdCollision
    );
    assert_eq!(ledger.snapshot(), full);
}

#[test]
fn terminal_releases_one_full_live_context_slot() {
    let (ledger, route_owner, lease) = ledger_with_lease();
    let first =
        mint_context(&ledger, &route_owner, &lease, "req-capacity-0").expect("first live context");
    for index in 1..MAX_USAGE_CONTEXTS_PER_LEDGER {
        mint_context(
            &ledger,
            &route_owner,
            &lease,
            &format!("req-capacity-{index}"),
        )
        .expect("fill live context capacity");
    }

    assert_eq!(
        mint_context(&ledger, &route_owner, &lease, "req-capacity-0")
            .expect_err("duplicate at capacity"),
        ProviderRouteError::RequestAlreadyRegistered
    );
    assert_eq!(
        mint_context(&ledger, &route_owner, &lease, "req-capacity-overflow")
            .expect_err("full live ledger"),
        ProviderRouteError::UsageContextCapacityExceeded
    );

    ledger
        .mint_terminal_sample(
            &first,
            first.request_id(),
            first.turn_id(),
            UsageCounters::none(),
        )
        .expect("terminal releases a live slot");
    mint_context(&ledger, &route_owner, &lease, "req-capacity-replacement")
        .expect("replacement live context");
}
