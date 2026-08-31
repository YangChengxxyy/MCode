// Rust guideline compliant 2026-08-31.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use mcode_config::{
    ArtifactRef, AuthorityRevision, CanonicalVersion, ManagerRecord, PluginFamily, Sha256Digest,
    SourceBindingId, TrustHighWater,
};

use super::{
    ActiveGeneration, CurrentManagerGeneration, ManagerGenerationDirector,
    ManagerGenerationSnapshot, ReconciliationOutcome,
};
use crate::ComponentLimits;
use crate::manager_loading::{ManagerCandidates, test_support};
use crate::runtime::PluginRuntime;

pub(crate) fn revision(value: u64) -> AuthorityRevision {
    AuthorityRevision::new(value).expect("valid authority revision")
}

pub(crate) fn artifact(version: &str, digit: char) -> ArtifactRef {
    ArtifactRef::new(
        CanonicalVersion::parse(version).expect("canonical version"),
        Sha256Digest::parse(format!("sha256:{}", digit.to_string().repeat(64)))
            .expect("canonical digest"),
    )
}

pub(super) fn installed_record(
    artifact: ArtifactRef,
    source: &str,
    trust_sequence: u64,
    trust_digit: char,
) -> ManagerRecord {
    let trust_digest =
        Sha256Digest::parse(format!("sha256:{}", trust_digit.to_string().repeat(64)))
            .expect("canonical trust digest");
    ManagerRecord::installed(
        true,
        SourceBindingId::parse(source).expect("canonical source binding"),
        artifact,
        TrustHighWater::new(trust_sequence, trust_digest).expect("valid trust high-water"),
    )
}

fn current_manager_source() -> String {
    include_str!("../tests/fixtures/current_manager_component.wat").to_owned()
}

fn replace_function(source: String, start: &str, end: &str, replacement: &str) -> String {
    let start_index = source.find(start).expect("function start marker");
    let end_index = source[start_index..]
        .find(end)
        .map(|offset| start_index + offset)
        .expect("function end marker");
    format!(
        "{}{}{}",
        &source[..start_index],
        replacement,
        &source[end_index..]
    )
}

fn outcome_function(name: &str, parameter: &str, result_tag: i32, variant: i32) -> String {
    format!(
        "    (func ${name}{parameter} (result i32)\n\
         \x20     i32.const 0\n\
         \x20     i32.const {result_tag}\n\
         \x20     i32.store\n\
         \x20     i32.const 1\n\
         \x20     i32.const {variant}\n\
         \x20     i32.store\n\
         \x20     i32.const 0)\n"
    )
}

fn spin_function(name: &str, parameter: &str) -> String {
    format!(
        "    (func ${name}{parameter} (result i32)\n\
         \x20     (loop $forever (br $forever))\n\
         \x20     unreachable)\n"
    )
}

fn component_with_functions(initialize: &str, poll: &str, shutdown: &str) -> Vec<u8> {
    let source = replace_function(
        current_manager_source(),
        "    (func $initialize",
        "    (func $poll",
        initialize,
    );
    let source = replace_function(source, "    (func $poll", "    (func $shutdown", poll);
    let source = replace_function(
        source,
        "    (func $shutdown",
        "    (export \"initialize\"",
        shutdown,
    );
    wat::parse_str(source).expect("valid Manager fixture")
}

pub(crate) fn ready_component() -> Vec<u8> {
    wat::parse_str(current_manager_source()).expect("valid current Manager component")
}

pub(super) fn pending_then_ready_component() -> Vec<u8> {
    component_with_functions(
        &outcome_function("initialize", " (param i64)", 0, 1),
        &outcome_function("poll", "", 0, 0),
        &outcome_function("shutdown", "", 0, 3),
    )
}

pub(super) fn pending_once_then_ready_component() -> Vec<u8> {
    component_with_functions(
        concat!(
            "    (func $initialize (param i64) (result i32)\n",
            "      i32.const 8\n",
            "      i32.const 1\n",
            "      i32.store\n",
            "      i32.const 0\n",
            "      i32.const 0\n",
            "      i32.store\n",
            "      i32.const 1\n",
            "      i32.const 1\n",
            "      i32.store\n",
            "      i32.const 0)\n",
        ),
        concat!(
            "    (func $poll (result i32)\n",
            "      i32.const 0\n",
            "      i32.const 0\n",
            "      i32.store\n",
            "      i32.const 1\n",
            "      i32.const 8\n",
            "      i32.load\n",
            "      i32.store\n",
            "      i32.const 8\n",
            "      i32.const 0\n",
            "      i32.store\n",
            "      i32.const 0)\n",
        ),
        &outcome_function("shutdown", "", 0, 3),
    )
}

pub(super) fn rejecting_component() -> Vec<u8> {
    component_with_functions(
        &outcome_function("initialize", " (param i64)", 1, 2),
        &outcome_function("poll", "", 0, 0),
        &outcome_function("shutdown", "", 0, 3),
    )
}

pub(super) fn spinning_poll_component() -> Vec<u8> {
    component_with_functions(
        &outcome_function("initialize", " (param i64)", 0, 0),
        &spin_function("poll", ""),
        &outcome_function("shutdown", "", 0, 3),
    )
}

pub(super) fn stopping_poll_component() -> Vec<u8> {
    component_with_functions(
        &outcome_function("initialize", " (param i64)", 0, 0),
        &outcome_function("poll", "", 0, 2),
        &outcome_function("shutdown", "", 0, 3),
    )
}

pub(super) fn pending_once_current_component() -> Vec<u8> {
    component_with_functions(
        concat!(
            "    (func $initialize (param i64) (result i32)\n",
            "      i32.const 8\n",
            "      i32.const 1\n",
            "      i32.store\n",
            "      i32.const 0\n",
            "      i32.const 0\n",
            "      i32.store\n",
            "      i32.const 1\n",
            "      i32.const 0\n",
            "      i32.store\n",
            "      i32.const 0)\n",
        ),
        concat!(
            "    (func $poll (result i32)\n",
            "      i32.const 0\n",
            "      i32.const 0\n",
            "      i32.store\n",
            "      i32.const 1\n",
            "      i32.const 8\n",
            "      i32.load\n",
            "      i32.store\n",
            "      i32.const 8\n",
            "      i32.const 0\n",
            "      i32.store\n",
            "      i32.const 0)\n",
        ),
        &outcome_function("shutdown", "", 0, 3),
    )
}

pub(super) fn failed_poll_component() -> Vec<u8> {
    component_with_functions(
        &outcome_function("initialize", " (param i64)", 0, 0),
        &outcome_function("poll", "", 1, 2),
        &outcome_function("shutdown", "", 0, 3),
    )
}

pub(super) fn trapping_poll_component() -> Vec<u8> {
    component_with_functions(
        &outcome_function("initialize", " (param i64)", 0, 0),
        "    (func $poll (result i32) unreachable)\n",
        &outcome_function("shutdown", "", 0, 3),
    )
}

pub(super) fn gateway_calling_component() -> Vec<u8> {
    let source = current_manager_source();
    let guest_index = source
        .find("  (core module $guest")
        .expect("guest module marker");
    let mut source = format!(
        "{}{}",
        concat!(
            "(component\n",
            "  (import \"mcode:plugin/feature-service@0.0.1\" (instance $feature-service\n",
            "    (export \"start-task\" (func (param \"request\" string) (result string)))\n",
            "    (export \"poll-task\" (func (param \"request\" string) (result string)))\n",
            "    (export \"cancel-task\" (func (param \"request\" string) (result string)))\n",
            "  ))\n",
            "  (alias export $feature-service \"start-task\" (func $start-task))\n",
            "  (alias export $feature-service \"poll-task\" (func $poll-task))\n",
            "  (alias export $feature-service \"cancel-task\" (func $cancel-task))\n",
            "  (core module $service-memory-module\n",
            "    (memory (export \"memory\") 1 1024)\n",
            "    (func (export \"realloc\") (param i32 i32 i32 i32) (result i32)\n",
            "      i32.const 1024)\n",
            "  )\n",
            "  (core instance $service-memory-instance (instantiate $service-memory-module))\n",
            "  (alias core export $service-memory-instance \"memory\" (core memory $service-memory))\n",
            "  (alias core export $service-memory-instance \"realloc\" (core func $service-realloc))\n",
            "  (core func $lower-start-task (canon lower (func $start-task)\n",
            "    (memory $service-memory) (realloc $service-realloc)))\n",
            "  (core func $lower-poll-task (canon lower (func $poll-task)\n",
            "    (memory $service-memory) (realloc $service-realloc)))\n",
            "  (core func $lower-cancel-task (canon lower (func $cancel-task)\n",
            "    (memory $service-memory) (realloc $service-realloc)))\n",
            "  (core instance $service-environment\n",
            "    (export \"memory\" (memory $service-memory))\n",
            "    (export \"start-task\" (func $lower-start-task))\n",
            "    (export \"poll-task\" (func $lower-poll-task))\n",
            "    (export \"cancel-task\" (func $lower-cancel-task))\n",
            "  )\n",
        ),
        &source[guest_index..],
    );
    let memory_marker = "    (memory (export \"memory\") 1 1024)";
    let guest_index = source
        .find("  (core module $guest")
        .expect("rebuilt guest module marker");
    let memory_index = source[guest_index..]
        .find(memory_marker)
        .map(|offset| guest_index + offset)
        .expect("guest memory marker");
    source.replace_range(
        memory_index..memory_index + memory_marker.len(),
        concat!(
            "    (import \"mcode:plugin/feature-service@0.0.1\" \"memory\" (memory 1 1024))\n",
            "    (import \"mcode:plugin/feature-service@0.0.1\" \"start-task\" (func $call-start-task (param i32 i32 i32)))\n",
            "    (import \"mcode:plugin/feature-service@0.0.1\" \"poll-task\" (func $call-poll-task (param i32 i32 i32)))\n",
            "    (import \"mcode:plugin/feature-service@0.0.1\" \"cancel-task\" (func $call-cancel-task (param i32 i32 i32)))\n",
            "    (export \"memory\" (memory 0))",
        ),
    );
    let source = replace_function(
        source,
        "    (func $initialize",
        "    (func $poll",
        concat!(
            "    (func $initialize (param i64) (result i32)\n",
            "      i32.const 0\n",
            "      i32.const 0\n",
            "      i32.const 64\n",
            "      call $call-start-task\n",
            "      i32.const 0\n",
            "      i32.const 0\n",
            "      i32.const 72\n",
            "      call $call-poll-task\n",
            "      i32.const 0\n",
            "      i32.const 0\n",
            "      i32.const 80\n",
            "      call $call-cancel-task\n",
            "      i32.const 0\n",
            "      i32.const 0\n",
            "      i32.store\n",
            "      i32.const 4\n",
            "      i32.const 0\n",
            "      i32.store\n",
            "      i32.const 0)\n",
        ),
    );
    let source = source.replacen(
        "  (core instance $guest-instance (instantiate $guest))",
        concat!(
            "  (core instance $guest-instance (instantiate $guest\n",
            "    (with \"mcode:plugin/feature-service@0.0.1\" (instance $service-environment))\n",
            "  ))",
        ),
        1,
    );
    wat::parse_str(source).expect("valid gateway-calling Manager fixture")
}

pub(crate) fn candidates(
    runtime: &PluginRuntime,
    authority: AuthorityRevision,
    entries: Vec<(PluginFamily, ArtifactRef, Vec<u8>)>,
) -> ManagerCandidates {
    let entries = entries
        .into_iter()
        .map(|(family, artifact, bytes)| {
            (
                family,
                installed_record(artifact, "test-release", 1, 'a'),
                bytes,
            )
        })
        .collect();
    authority_candidates(runtime, authority, entries)
}

pub(super) fn pending_then_rejecting_component() -> Vec<u8> {
    component_with_functions(
        &outcome_function("initialize", " (param i64)", 0, 1),
        &outcome_function("poll", "", 1, 2),
        &outcome_function("shutdown", "", 0, 3),
    )
}

pub(super) fn authority_candidates(
    runtime: &PluginRuntime,
    authority: AuthorityRevision,
    entries: Vec<(PluginFamily, ManagerRecord, Vec<u8>)>,
) -> ManagerCandidates {
    let entries = entries
        .into_iter()
        .map(|(family, record, bytes)| {
            let component = runtime
                .compile_manager(bytes, ComponentLimits::default())
                .expect("compile Manager fixture");
            (family, record, component)
        })
        .collect();
    test_support::candidates(authority, entries)
}

pub(super) fn empty_candidates(authority: AuthorityRevision) -> ManagerCandidates {
    test_support::candidates(authority, Vec::new())
}

pub(crate) fn director(runtime: &Arc<PluginRuntime>) -> ManagerGenerationDirector {
    ManagerGenerationDirector::new(Arc::clone(runtime)).expect("claim test runtime director")
}

pub(crate) fn assert_published(outcome: ReconciliationOutcome, expected: u64) {
    assert_eq!(outcome.revision().get(), expected);
    assert!(matches!(outcome, ReconciliationOutcome::Published { .. }));
}

pub(super) fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
    let mut context = Context::from_waker(Waker::noop());
    future.as_mut().poll(&mut context)
}

pub(super) fn snapshot(director: &ManagerGenerationDirector) -> ManagerGenerationSnapshot {
    director.snapshot().expect("available director snapshot")
}

pub(crate) fn current(
    director: &ManagerGenerationDirector,
    family: PluginFamily,
) -> Option<CurrentManagerGeneration> {
    director.current(family).expect("available director lookup")
}

pub(super) fn preparing(
    director: &ManagerGenerationDirector,
    family: PluginFamily,
) -> Arc<ActiveGeneration> {
    let state = director.lock_state().expect("available director state");
    Arc::clone(
        state
            .preparation
            .as_ref()
            .expect("retained preparation")
            .slots[crate::manager_loading::family_index(family)]
        .as_ref()
        .expect("prepared family generation"),
    )
}

pub(super) async fn wait_until_disposed(entry: &Arc<ActiveGeneration>) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if entry.owner.lock().await.is_none() {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("generation owner disposal");
}
