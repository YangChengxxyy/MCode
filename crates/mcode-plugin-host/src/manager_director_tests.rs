// Rust guideline compliant 2026-08-31.

use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::Ordering;
use std::time::Duration;

use mcode_config::{AuthorityRevision, PluginFamily};
use mcode_plugin_api::{MAX_TASK_GENERATION, TaskGeneration};

use super::test_support::{
    artifact, assert_published, candidates, current, director, empty_candidates,
    pending_once_then_ready_component, pending_then_ready_component, poll_once, ready_component,
    rejecting_component, revision, snapshot, spinning_poll_component, wait_until_disposed,
};
use super::{
    GenerationCallError, GenerationFence, ReconciliationError, ReconciliationOutcome,
    generation_activity_count,
};
use crate::manager_loading::family_index;
use crate::runtime::{LifecycleState, PluginRuntime, RuntimeError};
#[tokio::test(flavor = "current_thread")]
async fn absent_set_stays_exactly_disabled_without_creating_runtime_state() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);

    let outcome = director
        .reconcile(empty_candidates(AuthorityRevision::ABSENT))
        .await
        .expect("ABSENT reconciliation");

    assert!(matches!(outcome, ReconciliationOutcome::NoChange { .. }));
    let snapshot = snapshot(&director);
    assert_eq!(snapshot.revision(), AuthorityRevision::ABSENT);
    for family in PluginFamily::ALL {
        assert!(snapshot.current(family).is_none());
        assert!(current(&director, family).is_none());
    }
    assert_eq!(
        runtime.new_owner().err(),
        Some(RuntimeError::RuntimeUninitialized)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn pending_preparation_requires_explicit_poll_before_publication() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    let selected = artifact("1.0.0", '1');

    let outcome = director
        .reconcile(candidates(
            &runtime,
            revision(1),
            vec![(
                PluginFamily::Providers,
                selected.clone(),
                pending_then_ready_component(),
            )],
        ))
        .await
        .expect("prepare pending Manager");

    let ReconciliationOutcome::PreparationPending(progress) = outcome else {
        panic!("initialization must remain pending")
    };
    assert_eq!(progress.revision(), revision(1));
    assert_eq!(progress.pending_count(), 1);
    assert!(progress.is_pending(PluginFamily::Providers));
    assert!(current(&director, PluginFamily::Providers).is_none());
    assert_published(
        director
            .poll_preparation()
            .await
            .expect("explicit preparation poll"),
        1,
    );
    let current = current(&director, PluginFamily::Providers).expect("published generation");
    assert_eq!(current.artifact(), &selected);
    assert_eq!(current.generation(), TaskGeneration::new(1).expect("one"));
}

#[tokio::test(flavor = "current_thread")]
async fn each_poll_preparation_call_polls_the_guest_once() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    let initial = tokio::time::timeout(
        Duration::from_secs(1),
        director.reconcile(candidates(
            &runtime,
            revision(1),
            vec![(
                PluginFamily::Providers,
                artifact("1.0.0", '1'),
                pending_once_then_ready_component(),
            )],
        )),
    )
    .await
    .expect("initial pending call returns")
    .expect("initial pending result");
    assert!(matches!(
        initial,
        ReconciliationOutcome::PreparationPending(_)
    ));

    let first_poll = tokio::time::timeout(Duration::from_secs(1), director.poll_preparation())
        .await
        .expect("first explicit poll returns")
        .expect("first explicit poll result");
    let ReconciliationOutcome::PreparationPending(progress) = first_poll else {
        panic!("one API call must not consume the guest's second poll result")
    };
    assert_eq!(progress.pending_count(), 1);

    assert_published(
        tokio::time::timeout(Duration::from_secs(1), director.poll_preparation())
            .await
            .expect("second explicit poll returns")
            .expect("second explicit poll result"),
        1,
    );
}

#[tokio::test(flavor = "current_thread")]
async fn preparation_failure_preserves_snapshot_and_burns_generation() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    let first = artifact("1.0.0", '1');
    let rejected = artifact("1.0.1", '2');
    let third = artifact("1.0.2", '3');
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![(PluginFamily::Providers, first.clone(), ready_component())],
            ))
            .await
            .expect("first publication"),
        1,
    );

    assert_eq!(
        director
            .reconcile(candidates(
                &runtime,
                revision(2),
                vec![(PluginFamily::Providers, rejected, rejecting_component())],
            ))
            .await,
        Err(ReconciliationError::LifecycleRejected(
            PluginFamily::Providers
        ))
    );
    let unchanged = snapshot(&director);
    assert_eq!(unchanged.revision(), revision(1));
    assert_eq!(
        unchanged
            .current(PluginFamily::Providers)
            .expect("old current")
            .artifact(),
        &first
    );
    assert_eq!(
        director
            .reconcile(candidates(
                &runtime,
                revision(2),
                vec![(
                    PluginFamily::Providers,
                    artifact("1.0.9", '9'),
                    ready_component(),
                )],
            ))
            .await,
        Err(ReconciliationError::RevisionConflict)
    );

    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(3),
                vec![(PluginFamily::Providers, third, ready_component())],
            ))
            .await
            .expect("publication after rejected preparation"),
        3,
    );
    assert_eq!(
        current(&director, PluginFamily::Providers)
            .expect("third generation")
            .generation()
            .get(),
        3
    );
}

#[tokio::test(flavor = "current_thread")]
async fn initial_preparation_failure_shuts_down_every_created_instance() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);

    assert_eq!(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![
                    (
                        PluginFamily::Providers,
                        artifact("1.0.0", '1'),
                        ready_component(),
                    ),
                    (
                        PluginFamily::Session,
                        artifact("1.0.0", '1'),
                        rejecting_component(),
                    ),
                ],
            ))
            .await,
        Err(ReconciliationError::LifecycleRejected(
            PluginFamily::Session
        ))
    );

    let shutdowns = runtime.shutdown_observations();
    assert_eq!(shutdowns.len(), 2);
    assert!(shutdowns.contains(&Ok(Ok(LifecycleState::Ready))));
    assert!(shutdowns.contains(&Ok(Ok(LifecycleState::Stopped))));
}

#[tokio::test(flavor = "current_thread")]
async fn every_changed_slot_is_ready_before_any_slot_publishes() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    let old = artifact("1.0.0", '1');
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![
                    (PluginFamily::Providers, old.clone(), ready_component()),
                    (PluginFamily::Session, old.clone(), ready_component()),
                ],
            ))
            .await
            .expect("old complete set"),
        1,
    );

    assert_eq!(
        director
            .reconcile(candidates(
                &runtime,
                revision(2),
                vec![
                    (
                        PluginFamily::Providers,
                        artifact("2.0.0", '2'),
                        ready_component(),
                    ),
                    (
                        PluginFamily::Session,
                        artifact("2.0.0", '2'),
                        rejecting_component(),
                    ),
                ],
            ))
            .await,
        Err(ReconciliationError::LifecycleRejected(
            PluginFamily::Session
        ))
    );
    let snapshot = snapshot(&director);
    assert_eq!(snapshot.revision(), revision(1));
    assert_eq!(
        snapshot
            .current(PluginFamily::Providers)
            .expect("old Providers")
            .artifact(),
        &old
    );
    assert_eq!(
        snapshot
            .current(PluginFamily::Session)
            .expect("old Session")
            .artifact(),
        &old
    );
}

#[tokio::test(flavor = "current_thread")]
async fn identity_and_revision_rules_fail_closed_without_unneeded_generations() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    let first = artifact("1.0.0", '1');
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(2),
                vec![(PluginFamily::Web, first.clone(), ready_component())],
            ))
            .await
            .expect("initial publication"),
        2,
    );

    let same_revision = director
        .reconcile(candidates(
            &runtime,
            revision(2),
            vec![(PluginFamily::Web, first.clone(), ready_component())],
        ))
        .await
        .expect("same identity no-op");
    assert!(matches!(
        same_revision,
        ReconciliationOutcome::NoChange { .. }
    ));
    let newer_same = director
        .reconcile(candidates(
            &runtime,
            revision(3),
            vec![(PluginFamily::Web, first.clone(), ready_component())],
        ))
        .await
        .expect("newer identical revision");
    assert_published(newer_same, 3);
    assert_eq!(
        current(&director, PluginFamily::Web)
            .expect("current Web")
            .generation()
            .get(),
        1
    );

    assert_eq!(
        director
            .reconcile(candidates(
                &runtime,
                revision(2),
                vec![(PluginFamily::Web, first.clone(), ready_component())],
            ))
            .await,
        Err(ReconciliationError::RevisionRegression)
    );
    assert_eq!(
        director
            .reconcile(candidates(
                &runtime,
                revision(3),
                vec![(PluginFamily::Web, artifact("1.0.0", '2'), ready_component(),)],
            ))
            .await,
        Err(ReconciliationError::RevisionConflict)
    );

    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(4),
                vec![(PluginFamily::Web, artifact("1.0.0", '2'), ready_component())],
            ))
            .await
            .expect("same version with changed digest"),
        4,
    );
    assert_eq!(
        current(&director, PluginFamily::Web)
            .expect("changed Web")
            .generation()
            .get(),
        2
    );
}

#[tokio::test(flavor = "current_thread")]
async fn disable_then_enable_keeps_monotonic_family_generation() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    let selected = artifact("1.0.0", '1');
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![(PluginFamily::Session, selected.clone(), ready_component())],
            ))
            .await
            .expect("enable"),
        1,
    );
    assert_published(
        director
            .reconcile(empty_candidates(revision(2)))
            .await
            .expect("disable"),
        2,
    );
    assert!(current(&director, PluginFamily::Session).is_none());
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(3),
                vec![(PluginFamily::Session, selected, ready_component())],
            ))
            .await
            .expect("re-enable"),
        3,
    );
    assert_eq!(
        current(&director, PluginFamily::Session)
            .expect("re-enabled")
            .generation()
            .get(),
        2
    );
}

#[tokio::test(flavor = "current_thread")]
async fn exhausted_generation_keeps_old_generation_active() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    let first = artifact("1.0.0", '1');
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![(PluginFamily::Ask, first.clone(), ready_component())],
            ))
            .await
            .expect("initial publication"),
        1,
    );
    director
        .lock_state()
        .expect("available director state")
        .high_water[family_index(PluginFamily::Ask)] = MAX_TASK_GENERATION;

    assert_eq!(
        director
            .reconcile(candidates(
                &runtime,
                revision(2),
                vec![(PluginFamily::Ask, artifact("2.0.0", '2'), ready_component(),)],
            ))
            .await,
        Err(ReconciliationError::GenerationExhausted(PluginFamily::Ask))
    );
    let current = current(&director, PluginFamily::Ask).expect("old current");
    assert_eq!(current.artifact(), &first);
    assert_eq!(current.generation().get(), 1);
    assert_eq!(current.revision(), revision(1));
}

#[tokio::test(flavor = "current_thread")]
async fn complete_snapshot_publishes_before_held_old_activity_drains() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = Arc::new(director(&runtime));
    let old = artifact("1.0.0", '1');
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![
                    (PluginFamily::Providers, old.clone(), ready_component()),
                    (PluginFamily::Web, old, ready_component()),
                ],
            ))
            .await
            .expect("old set"),
        1,
    );
    let held_entry = director
        .current_entry(PluginFamily::Providers)
        .expect("available lookup")
        .expect("old Providers entry");
    let held_activity = held_entry.fence.enter().expect("old activity");

    let replacement = candidates(
        &runtime,
        revision(2),
        vec![
            (
                PluginFamily::Providers,
                artifact("2.0.0", '2'),
                ready_component(),
            ),
            (PluginFamily::Web, artifact("2.0.0", '2'), ready_component()),
        ],
    );
    let outcome = tokio::time::timeout(Duration::from_secs(1), director.reconcile(replacement))
        .await
        .expect("publication does not wait for old activity")
        .expect("new set");
    assert_published(outcome, 2);
    let published = snapshot(&director);
    assert_eq!(published.revision(), revision(2));
    assert_eq!(
        published
            .current(PluginFamily::Providers)
            .expect("new Providers")
            .generation()
            .get(),
        2
    );
    assert_eq!(
        published
            .current(PluginFamily::Web)
            .expect("new Web")
            .generation()
            .get(),
        2
    );
    assert!(held_entry.owner.lock().await.is_some());

    drop(held_activity);
    wait_until_disposed(&held_entry).await;
}

#[tokio::test(flavor = "current_thread")]
async fn retirement_cancels_inflight_and_queued_calls_then_disposes_store() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = Arc::new(director(&runtime));
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![(
                    PluginFamily::Usage,
                    artifact("1.0.0", '1'),
                    spinning_poll_component(),
                )],
            ))
            .await
            .expect("spinning old generation"),
        1,
    );
    let old = director
        .current_entry(PluginFamily::Usage)
        .expect("available lookup")
        .expect("old Usage entry");
    let first_entry = Arc::clone(&old);
    let mut first = Box::pin(first_entry.poll_lifecycle());
    assert!(poll_once(first.as_mut()).is_pending());
    let second_entry = Arc::clone(&old);
    let mut second = Box::pin(second_entry.poll_lifecycle());
    assert!(poll_once(second.as_mut()).is_pending());

    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(2),
                vec![(
                    PluginFamily::Usage,
                    artifact("2.0.0", '2'),
                    ready_component(),
                )],
            ))
            .await
            .expect("replace spinning generation"),
        2,
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), first)
            .await
            .expect("in-flight cancellation"),
        Err(GenerationCallError::Cancelled)
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), second)
            .await
            .expect("queued cancellation"),
        Err(GenerationCallError::Cancelled)
    );
    wait_until_disposed(&old).await;
    assert_eq!(
        old.poll_lifecycle().await,
        Err(GenerationCallError::Retired)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn owner_lock_serializes_lifecycle_store_access() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![(
                    PluginFamily::Workspace,
                    artifact("1.0.0", '1'),
                    ready_component(),
                )],
            ))
            .await
            .expect("publish Workspace"),
        1,
    );
    let entry = director
        .current_entry(PluginFamily::Workspace)
        .expect("available lookup")
        .expect("Workspace entry");
    let owner = entry.owner.lock().await;
    let queued_entry = Arc::clone(&entry);
    let mut queued = Box::pin(queued_entry.poll_lifecycle());
    assert!(poll_once(queued.as_mut()).is_pending());
    drop(owner);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), queued)
            .await
            .expect("serialized lifecycle poll"),
        Ok(LifecycleState::Ready)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn enter_retire_protocol_rejects_new_work_and_notifies_quiescence() {
    let fence = Arc::new(GenerationFence::new(Arc::new(AtomicU8::new(
        super::PUBLICATION_OPEN,
    ))));
    fence.mark_current();
    let activity = fence.enter().expect("activity before retirement");
    fence.mark_retired();
    fence.signal_cancellation();
    assert!(fence.enter().is_none());

    let drain_fence = Arc::clone(&fence);
    let mut drain = Box::pin(async move { drain_fence.drain().await });
    assert!(poll_once(drain.as_mut()).is_pending());
    drop(activity);
    tokio::time::timeout(Duration::from_secs(1), drain)
        .await
        .expect("quiescent drain");
    assert_eq!(
        generation_activity_count(fence.state.load(Ordering::Acquire)),
        0
    );
}

#[tokio::test(flavor = "current_thread")]
async fn later_generation_exhaustion_does_not_reserve_an_earlier_family() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![
                    (
                        PluginFamily::Providers,
                        artifact("1.0.0", '1'),
                        ready_component(),
                    ),
                    (PluginFamily::Ask, artifact("1.0.0", '1'), ready_component()),
                ],
            ))
            .await
            .expect("initial two-family publication"),
        1,
    );
    {
        let mut state = director.lock_state().expect("available director state");
        state.high_water[family_index(PluginFamily::Ask)] = MAX_TASK_GENERATION;
    }

    assert_eq!(
        director
            .reconcile(candidates(
                &runtime,
                revision(2),
                vec![
                    (
                        PluginFamily::Providers,
                        artifact("2.0.0", '2'),
                        ready_component(),
                    ),
                    (PluginFamily::Ask, artifact("2.0.0", '2'), ready_component(),),
                ],
            ))
            .await,
        Err(ReconciliationError::GenerationExhausted(PluginFamily::Ask))
    );
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(3),
                vec![(
                    PluginFamily::Providers,
                    artifact("3.0.0", '3'),
                    ready_component(),
                )],
            ))
            .await
            .expect("Providers retry after atomic reservation failure"),
        3,
    );
    assert_eq!(
        current(&director, PluginFamily::Providers)
            .expect("current Providers")
            .generation()
            .get(),
        2
    );
}
