// Rust guideline compliant 2026-08-31.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use mcode_config::{AuthorityRevision, HomeLayout, PluginFamily};
use mcode_plugin_api::TaskGeneration;

use super::test_support::{
    artifact, assert_published, authority_candidates, candidates, current, director,
    empty_candidates, gateway_calling_component, installed_record, pending_then_ready_component,
    pending_then_rejecting_component, poll_once, preparing, ready_component, revision, snapshot,
    wait_until_disposed,
};
use super::{
    GENERATION_ACTIVITY_INCREMENT, GENERATION_CURRENT, GenerationFence, MAX_GENERATION_ACTIVITIES,
    ManagerGenerationDirector, ReconciliationError, ReconciliationOutcome,
    generation_activity_count,
};
use crate::PackConfigurationError;
use crate::pack_activation::{PackActivationClient, ResourcesTaskSentinel};
use crate::pack_selection::PackSelectionAuthority;
use crate::runtime::{LifecycleState, PluginRuntime, RuntimeError};

#[test]
fn runtime_concurrently_accepts_only_one_generation_director() {
    let runtime = Arc::new(PluginRuntime::new());
    let pack_home = HomeLayout::from_root(
        std::env::current_dir()
            .expect("current test directory")
            .join("target")
            .join("concurrent-pack-home"),
    )
    .expect("valid inactive Pack home");
    let barrier = Arc::new(Barrier::new(2));
    let attempts = (0..2)
        .map(|_| {
            let runtime = Arc::clone(&runtime);
            let barrier = Arc::clone(&barrier);
            let pack_home = pack_home.clone();
            std::thread::spawn(move || {
                barrier.wait();
                ManagerGenerationDirector::new(runtime, pack_home).is_ok()
            })
        })
        .collect::<Vec<_>>();
    let successes = attempts
        .into_iter()
        .map(|attempt| usize::from(attempt.join().expect("director claim thread")))
        .sum::<usize>();

    assert_eq!(successes, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn absent_disables_without_lowering_positive_authority_high_water() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    let selected = artifact("5.0.0", '5');
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(5),
                vec![(PluginFamily::Providers, selected.clone(), ready_component())],
            ))
            .await
            .expect("positive publication"),
        5,
    );

    assert_published(
        director
            .reconcile(empty_candidates(AuthorityRevision::ABSENT))
            .await
            .expect("ABSENT disables current set"),
        AuthorityRevision::ABSENT.get(),
    );
    assert_eq!(snapshot(&director).revision(), AuthorityRevision::ABSENT);
    assert!(current(&director, PluginFamily::Providers).is_none());

    assert_eq!(
        director
            .reconcile(candidates(
                &runtime,
                revision(4),
                vec![(PluginFamily::Providers, selected.clone(), ready_component())],
            ))
            .await,
        Err(ReconciliationError::RevisionRegression)
    );
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(5),
                vec![(PluginFamily::Providers, selected, ready_component())],
            ))
            .await
            .expect("same positive authority restores after absence"),
        5,
    );
    assert_eq!(
        current(&director, PluginFamily::Providers)
            .expect("restored Providers")
            .generation()
            .get(),
        2
    );
}

#[tokio::test(flavor = "current_thread")]
async fn absent_with_an_enabled_candidate_is_a_conflict() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);

    assert_eq!(
        director
            .reconcile(candidates(
                &runtime,
                AuthorityRevision::ABSENT,
                vec![(
                    PluginFamily::Session,
                    artifact("1.0.0", '1'),
                    ready_component(),
                )],
            ))
            .await,
        Err(ReconciliationError::RevisionConflict)
    );
    assert_eq!(snapshot(&director).revision(), AuthorityRevision::ABSENT);
}

#[tokio::test(flavor = "current_thread")]
async fn same_revision_binds_source_and_trust_high_water() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    let selected = artifact("1.0.0", '1');
    assert_published(
        director
            .reconcile(authority_candidates(
                &runtime,
                revision(7),
                vec![(
                    PluginFamily::Providers,
                    installed_record(selected.clone(), "source-a", 7, '7'),
                    ready_component(),
                )],
            ))
            .await
            .expect("initial authority publication"),
        7,
    );
    let bound = current(&director, PluginFamily::Providers).expect("current Providers");
    assert_eq!(bound.source().as_str(), "source-a");
    assert_eq!(bound.trust_high_water().sequence(), 7);

    for changed in [
        installed_record(selected.clone(), "source-b", 7, '7'),
        installed_record(selected.clone(), "source-a", 8, '8'),
    ] {
        assert_eq!(
            director
                .reconcile(authority_candidates(
                    &runtime,
                    revision(7),
                    vec![(PluginFamily::Providers, changed, ready_component(),)],
                ))
                .await,
            Err(ReconciliationError::RevisionConflict)
        );
    }

    assert_published(
        director
            .reconcile(authority_candidates(
                &runtime,
                revision(8),
                vec![(
                    PluginFamily::Providers,
                    installed_record(selected, "source-b", 8, '8'),
                    ready_component(),
                )],
            ))
            .await
            .expect("new source authority publication"),
        8,
    );
    let rebound = current(&director, PluginFamily::Providers).expect("rebound Providers");
    assert_eq!(rebound.source().as_str(), "source-b");
    assert_eq!(rebound.generation().get(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn unpublished_preparation_rejects_generation_activity_until_publication() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);

    let outcome = director
        .reconcile(candidates(
            &runtime,
            revision(1),
            vec![(
                PluginFamily::Resources,
                artifact("1.0.0", '1'),
                pending_then_ready_component(),
            )],
        ))
        .await
        .expect("pending Resources preparation");
    assert!(matches!(
        outcome,
        ReconciliationOutcome::PreparationPending(_)
    ));
    let preparing = {
        let state = director.lock_state().expect("available director state");
        Arc::clone(
            state
                .preparation
                .as_ref()
                .expect("retained preparation")
                .slots[crate::manager_loading::family_index(PluginFamily::Resources)]
            .as_ref()
            .expect("prepared Resources generation"),
        )
    };

    assert!(preparing.fence.enter().is_none());
    assert_published(
        director
            .poll_preparation()
            .await
            .expect("publish Resources preparation"),
        1,
    );
    assert!(preparing.fence.enter().is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn task_imports_validate_controls_before_generation_admission() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![(
                    PluginFamily::Resources,
                    artifact("1.0.0", '1'),
                    gateway_calling_component(),
                )],
            ))
            .await
            .expect("gateway-calling Resources publication"),
        1,
    );
    let entry = director
        .current_entry(PluginFamily::Resources)
        .expect("available lookup")
        .expect("current Resources entry");

    assert_eq!(entry.fence.admission_attempts(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn newer_target_retires_the_superseded_preparation() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    let pending = director
        .reconcile(candidates(
            &runtime,
            revision(1),
            vec![(
                PluginFamily::Resources,
                artifact("1.0.0", '1'),
                pending_then_ready_component(),
            )],
        ))
        .await
        .expect("pending old Resources");
    assert!(matches!(
        pending,
        ReconciliationOutcome::PreparationPending(_)
    ));
    let superseded = preparing(&director, PluginFamily::Resources);

    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(2),
                vec![(
                    PluginFamily::Resources,
                    artifact("2.0.0", '2'),
                    ready_component(),
                )],
            ))
            .await
            .expect("replacement Resources"),
        2,
    );
    assert!(superseded.owner.lock().await.is_none());
    assert!(superseded.fence.enter().is_none());
    assert_eq!(
        *superseded
            .shutdown_observation
            .lock()
            .expect("shutdown observation"),
        Some(Ok(LifecycleState::Stopped))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn failed_pending_batch_retires_every_prepared_generation() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    let pending = director
        .reconcile(candidates(
            &runtime,
            revision(1),
            vec![
                (
                    PluginFamily::Resources,
                    artifact("1.0.0", '1'),
                    pending_then_ready_component(),
                ),
                (
                    PluginFamily::Ask,
                    artifact("1.0.0", '1'),
                    pending_then_rejecting_component(),
                ),
            ],
        ))
        .await
        .expect("pending two-family batch");
    assert!(matches!(
        pending,
        ReconciliationOutcome::PreparationPending(_)
    ));
    let resources = preparing(&director, PluginFamily::Resources);
    let ask = preparing(&director, PluginFamily::Ask);

    assert_eq!(
        director.poll_preparation().await,
        Err(ReconciliationError::LifecycleRejected(PluginFamily::Ask))
    );
    assert!(resources.owner.lock().await.is_none());
    assert!(ask.owner.lock().await.is_none());
    assert_eq!(
        *resources
            .shutdown_observation
            .lock()
            .expect("Resources shutdown observation"),
        Some(Ok(LifecycleState::Stopped))
    );
    assert_eq!(
        *ask.shutdown_observation
            .lock()
            .expect("Ask shutdown observation"),
        Some(Ok(LifecycleState::Stopped))
    );
    assert_eq!(snapshot(&director).revision(), AuthorityRevision::ABSENT);
}

#[tokio::test(flavor = "current_thread")]
async fn published_cleanup_retains_serialization_until_quiescence() {
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
                    ready_component(),
                )],
            ))
            .await
            .expect("old Usage"),
        1,
    );
    let old = director
        .current_entry(PluginFamily::Usage)
        .expect("available lookup")
        .expect("old Usage entry");
    let activity = old.fence.enter().expect("held old activity");

    let replacement = candidates(
        &runtime,
        revision(2),
        vec![(
            PluginFamily::Usage,
            artifact("2.0.0", '2'),
            ready_component(),
        )],
    );
    assert_published(
        tokio::time::timeout(Duration::from_secs(1), director.reconcile(replacement))
            .await
            .expect("publication while cleanup is pending")
            .expect("new Usage"),
        2,
    );
    assert_eq!(snapshot(&director).revision(), revision(2));
    let mut blocked = Box::pin(director.reconcile(candidates(
        &runtime,
        revision(3),
        vec![(
            PluginFamily::Usage,
            artifact("3.0.0", '3'),
            ready_component(),
        )],
    )));
    assert!(poll_once(blocked.as_mut()).is_pending());
    assert_eq!(snapshot(&director).revision(), revision(2));

    drop(activity);
    assert_published(
        tokio::time::timeout(Duration::from_secs(1), blocked)
            .await
            .expect("cleanup releases reconciliation serialization")
            .expect("third Usage"),
        3,
    );
    wait_until_disposed(&old).await;
}

#[test]
fn publication_cleanup_survives_caller_runtime_migration() {
    let first_executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("first caller runtime");
    let second_executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("second caller runtime");
    let third_executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("third caller runtime");
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    first_executor.block_on(async {
        assert_published(
            director
                .reconcile(candidates(
                    &runtime,
                    revision(1),
                    vec![(
                        PluginFamily::Usage,
                        artifact("1.0.0", '1'),
                        ready_component(),
                    )],
                ))
                .await
                .expect("old Usage"),
            1,
        );
    });
    let old = director
        .current_entry(PluginFamily::Usage)
        .expect("available lookup")
        .expect("old Usage entry");
    let activity = old.fence.enter().expect("held old activity");

    second_executor.block_on(async {
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
                .expect("replacement Usage"),
            2,
        );
    });
    drop(activity);

    third_executor.block_on(async {
        assert_published(
            tokio::time::timeout(
                Duration::from_secs(1),
                director.reconcile(candidates(
                    &runtime,
                    revision(3),
                    vec![(
                        PluginFamily::Usage,
                        artifact("3.0.0", '3'),
                        ready_component(),
                    )],
                )),
            )
            .await
            .expect("cleanup releases serialization across runtimes")
            .expect("third Usage"),
            3,
        );
        director.shutdown().await.expect("director shutdown");
    });
    assert_eq!(
        *old.shutdown_observation
            .lock()
            .expect("old Usage shutdown observation"),
        Some(Ok(LifecycleState::Ready))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_closes_admission_then_drains_the_current_store() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![(
                    PluginFamily::Session,
                    artifact("1.0.0", '1'),
                    ready_component(),
                )],
            ))
            .await
            .expect("current Session"),
        1,
    );
    let entry = director
        .current_entry(PluginFamily::Session)
        .expect("available lookup")
        .expect("current Session entry");
    let activity = entry.fence.enter().expect("held Session activity");
    let mut shutdown = Box::pin(director.shutdown());

    assert!(poll_once(shutdown.as_mut()).is_pending());
    assert!(entry.fence.enter().is_none());
    drop(activity);
    tokio::time::timeout(Duration::from_secs(1), shutdown)
        .await
        .expect("bounded director shutdown")
        .expect("available director shutdown");
    assert!(entry.owner.lock().await.is_none());
    assert_eq!(
        *entry
            .shutdown_observation
            .lock()
            .expect("Session shutdown observation"),
        Some(Ok(LifecycleState::Ready))
    );
    assert_eq!(director.snapshot(), Err(ReconciliationError::Closed));
    assert_eq!(
        director.reconcile(empty_candidates(revision(2))).await,
        Err(ReconciliationError::Closed)
    );
    assert_eq!(
        director.publish_pack_configuration(None).await,
        Err(PackConfigurationError::Closed)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_retires_a_retained_pending_preparation() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    let outcome = director
        .reconcile(candidates(
            &runtime,
            revision(1),
            vec![(
                PluginFamily::Session,
                artifact("1.0.0", '1'),
                pending_then_ready_component(),
            )],
        ))
        .await
        .expect("pending Session preparation");
    assert!(matches!(
        outcome,
        ReconciliationOutcome::PreparationPending(_)
    ));
    let entry = preparing(&director, PluginFamily::Session);

    director.shutdown().await.expect("shutdown preparation");

    assert!(entry.owner.lock().await.is_none());
    assert_eq!(
        *entry
            .shutdown_observation
            .lock()
            .expect("Session shutdown observation"),
        Some(Ok(LifecycleState::Stopped))
    );
    assert_eq!(director.snapshot(), Err(ReconciliationError::Closed));
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_shutdown_detaches_generation_cleanup() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![(
                    PluginFamily::Session,
                    artifact("1.0.0", '1'),
                    ready_component(),
                )],
            ))
            .await
            .expect("current Session"),
        1,
    );
    let entry = director
        .current_entry(PluginFamily::Session)
        .expect("available lookup")
        .expect("current Session entry");
    let activity = entry.fence.enter().expect("held Session activity");
    let mut shutdown = Box::pin(director.shutdown());

    assert!(poll_once(shutdown.as_mut()).is_pending());
    assert!(entry.fence.enter().is_none());
    drop(shutdown);
    drop(activity);

    wait_until_disposed(&entry).await;
    assert_eq!(
        *entry
            .shutdown_observation
            .lock()
            .expect("Session shutdown observation"),
        Some(Ok(LifecycleState::Ready))
    );
    assert_eq!(director.snapshot(), Err(ReconciliationError::Closed));
}

#[tokio::test(flavor = "current_thread")]
async fn drop_fail_closes_and_detaches_current_store_cleanup() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![(
                    PluginFamily::Todo,
                    artifact("1.0.0", '1'),
                    ready_component(),
                )],
            ))
            .await
            .expect("current Todo"),
        1,
    );
    let entry = director
        .current_entry(PluginFamily::Todo)
        .expect("available lookup")
        .expect("current Todo entry");
    let activity = entry.fence.enter().expect("held Todo activity");

    drop(director);
    assert!(entry.fence.enter().is_none());
    drop(activity);
    wait_until_disposed(&entry).await;
    assert_eq!(
        *entry
            .shutdown_observation
            .lock()
            .expect("Todo shutdown observation"),
        Some(Ok(LifecycleState::Ready))
    );
}

#[test]
fn activity_reservation_never_wraps_at_usize_max() {
    let fence = Arc::new(GenerationFence::new(Arc::new(AtomicU64::new(
        super::PUBLICATION_OPEN,
    ))));
    fence.mark_current();
    fence.state.store(
        GENERATION_CURRENT + MAX_GENERATION_ACTIVITIES * GENERATION_ACTIVITY_INCREMENT,
        Ordering::Release,
    );

    assert!(fence.enter().is_none());
    assert_eq!(
        generation_activity_count(fence.state.load(Ordering::Acquire)),
        MAX_GENERATION_ACTIVITIES
    );

    fence.state.store(
        GENERATION_CURRENT + (MAX_GENERATION_ACTIVITIES - 1) * GENERATION_ACTIVITY_INCREMENT,
        Ordering::Release,
    );
    let last = fence.enter().expect("last representable activity");
    assert_eq!(
        generation_activity_count(fence.state.load(Ordering::Acquire)),
        MAX_GENERATION_ACTIVITIES
    );
    assert!(fence.enter().is_none());
    drop(last);
    assert_eq!(
        generation_activity_count(fence.state.load(Ordering::Acquire)),
        MAX_GENERATION_ACTIVITIES - 1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn poisoned_state_returns_unavailable_without_panicking() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![(
                    PluginFamily::Providers,
                    artifact("1.0.0", '1'),
                    ready_component(),
                )],
            ))
            .await
            .expect("current Providers generation"),
        1,
    );
    let expected = director
        .current(PluginFamily::Providers)
        .expect("available lookup")
        .expect("current Providers generation");
    let entry = director
        .current_entry(PluginFamily::Providers)
        .expect("available entry lookup")
        .expect("current Providers entry");
    let poison = catch_unwind(AssertUnwindSafe(|| {
        let _state = director.state.lock().expect("acquire state for poisoning");
        panic!("poison director state");
    }));
    assert!(poison.is_err());

    assert_eq!(director.snapshot(), Err(ReconciliationError::Unavailable));
    assert_eq!(
        director.current(PluginFamily::Providers),
        Err(ReconciliationError::Unavailable)
    );
    assert_eq!(
        director
            .reconcile(empty_candidates(AuthorityRevision::ABSENT))
            .await,
        Err(ReconciliationError::Unavailable)
    );
    assert_eq!(
        director.publish_pack_configuration(None).await,
        Err(PackConfigurationError::Unavailable)
    );
    assert_eq!(
        director.poll_current(&expected).await,
        Err(super::ManagerGenerationCallError::Unavailable)
    );
    assert!(entry.fence.enter().is_none());
    wait_until_disposed(&entry).await;
}

#[test]
fn duplicate_generation_binding_disposes_the_store() {
    let runtime = Arc::new(PluginRuntime::new());
    runtime
        .compile_manager(ready_component(), crate::ComponentLimits::default())
        .expect("initialize runtime");
    let mut owner = runtime.new_owner().expect("available owner");
    let authority = PackSelectionAuthority::new();
    let pack_home = HomeLayout::from_root(
        std::env::current_dir()
            .expect("current test directory")
            .join("target")
            .join("duplicate-binding-pack-home"),
    )
    .expect("valid inactive Pack home");
    let activation = || {
        PackActivationClient::new(
            Arc::clone(&runtime),
            pack_home.clone(),
            PluginFamily::Session,
            authority.client(PluginFamily::Session),
            Arc::new(ResourcesTaskSentinel::new(
                TaskGeneration::new(1).expect("task generation"),
            )),
        )
    };

    assert_eq!(
        owner.bind_generation_context(
            Arc::new(GenerationFence::new(Arc::new(AtomicU64::new(
                super::PUBLICATION_OPEN,
            )))),
            activation(),
            crate::FeatureCaller::new(
                PluginFamily::Session,
                TaskGeneration::new(1).expect("task generation"),
            ),
        ),
        Ok(())
    );
    assert_eq!(
        owner.bind_generation_context(
            Arc::new(GenerationFence::new(Arc::new(AtomicU64::new(
                super::PUBLICATION_OPEN,
            )))),
            activation(),
            crate::FeatureCaller::new(
                PluginFamily::Session,
                TaskGeneration::new(1).expect("task generation"),
            ),
        ),
        Err(RuntimeError::GenerationBound)
    );
    assert!(!owner.is_available());
    assert_eq!(
        owner.bind_generation_context(
            Arc::new(GenerationFence::new(Arc::new(AtomicU64::new(
                super::PUBLICATION_OPEN,
            )))),
            activation(),
            crate::FeatureCaller::new(
                PluginFamily::Session,
                TaskGeneration::new(1).expect("task generation"),
            ),
        ),
        Err(RuntimeError::StoreDisposed)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn director_publishes_all_twelve_enabled_slots() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    let entries = PluginFamily::ALL
        .into_iter()
        .map(|family| (family, artifact("1.0.0", '1'), ready_component()))
        .collect();

    assert_published(
        director
            .reconcile(candidates(&runtime, revision(1), entries))
            .await
            .expect("all twelve publication"),
        1,
    );
    let published = snapshot(&director);
    for family in PluginFamily::ALL {
        let generation = published.current(family).expect("enabled family");
        assert_eq!(generation.family(), family);
        assert_eq!(generation.generation().get(), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn publication_gate_closes_admission_for_the_complete_manager_set() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    let entries = PluginFamily::ALL
        .into_iter()
        .map(|family| (family, artifact("1.0.0", '1'), ready_component()))
        .collect();
    assert_published(
        director
            .reconcile(candidates(&runtime, revision(1), entries))
            .await
            .expect("all twelve publication"),
        1,
    );
    let generations = PluginFamily::ALL.map(|family| {
        director
            .current_entry(family)
            .expect("available lookup")
            .expect("enabled family")
    });

    let open_epoch = director.publication_state.load(Ordering::SeqCst);
    director
        .publication_state
        .store(open_epoch + 1, Ordering::SeqCst);
    for generation in &generations {
        assert!(generation.fence.enter().is_none());
    }

    director
        .publication_state
        .store(open_epoch + 2, Ordering::SeqCst);
    let activities = generations
        .iter()
        .map(|generation| {
            generation
                .fence
                .enter()
                .expect("the complete set reopens together")
        })
        .collect::<Vec<_>>();
    drop(activities);

    director
        .publication_state
        .store(super::PUBLICATION_CLOSED, Ordering::SeqCst);
    for generation in &generations {
        assert!(generation.fence.enter().is_none());
    }
}

#[test]
fn admission_rejects_a_complete_publication_epoch_crossing() {
    let publication_state = Arc::new(AtomicU64::new(super::PUBLICATION_OPEN));
    let fence = Arc::new(GenerationFence::new(Arc::clone(&publication_state)));
    fence.mark_current();

    let crossed = fence.enter_after_gate_load_for_test(|| {
        publication_state.store(super::PUBLICATION_IN_PROGRESS, Ordering::SeqCst);
        publication_state.store(2, Ordering::SeqCst);
    });

    assert!(crossed.is_none());
    assert_eq!(
        generation_activity_count(fence.state.load(Ordering::Acquire)),
        0
    );
}
