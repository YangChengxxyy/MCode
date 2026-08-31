// Rust guideline compliant 2026-08-31.

use std::sync::Arc;

use mcode_config::PluginFamily;

use super::test_support::{
    artifact, assert_published, candidates, current, director, failed_poll_component,
    pending_once_current_component, pending_then_ready_component, preparing, ready_component,
    revision, stopping_poll_component, trapping_poll_component, wait_until_disposed,
};
use super::{ManagerGenerationCallError, ReconciliationError, ReconciliationOutcome};
use crate::runtime::{LifecycleErrorCode, LifecycleState, PluginRuntime};

#[tokio::test(flavor = "current_thread")]
async fn ready_and_pending_current_polls_remain_current() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![
                    (
                        PluginFamily::Session,
                        artifact("1.0.0", '1'),
                        ready_component(),
                    ),
                    (
                        PluginFamily::Compaction,
                        artifact("1.0.0", '2'),
                        pending_once_current_component(),
                    ),
                ],
            ))
            .await
            .expect("publish current poll fixtures"),
        1,
    );
    let ready = current(&director, PluginFamily::Session).expect("current Session");
    let pending = current(&director, PluginFamily::Compaction).expect("current Compaction");

    assert_eq!(
        director
            .poll_current(&ready)
            .await
            .expect("ready current poll")
            .outcome(),
        Ok(LifecycleState::Ready)
    );
    assert_eq!(
        current(&director, PluginFamily::Session),
        Some(ready.clone())
    );
    assert_eq!(
        director
            .poll_current(&pending)
            .await
            .expect("pending current poll")
            .outcome(),
        Ok(LifecycleState::Pending)
    );
    assert_eq!(
        current(&director, PluginFamily::Compaction),
        Some(pending.clone())
    );
    assert_eq!(
        director
            .poll_current(&pending)
            .await
            .expect("second stateful current poll")
            .outcome(),
        Ok(LifecycleState::Ready)
    );
    assert!(director.poll_current(&ready).await.is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn current_generation_tag_cannot_cross_directors() {
    let runtime_a = Arc::new(PluginRuntime::new());
    let runtime_b = Arc::new(PluginRuntime::new());
    let director_a = director(&runtime_a);
    let director_b = director(&runtime_b);
    let selected = artifact("1.0.0", '1');
    assert_published(
        director_a
            .reconcile(candidates(
                &runtime_a,
                revision(1),
                vec![(PluginFamily::Resources, selected.clone(), ready_component())],
            ))
            .await
            .expect("publish first director"),
        1,
    );
    assert_published(
        director_b
            .reconcile(candidates(
                &runtime_b,
                revision(1),
                vec![(PluginFamily::Resources, selected, trapping_poll_component())],
            ))
            .await
            .expect("publish second director"),
        1,
    );
    let tag_a = current(&director_a, PluginFamily::Resources).expect("first tag");
    let tag_b = current(&director_b, PluginFamily::Resources).expect("second tag");
    let entry_b = director_b
        .current_entry(PluginFamily::Resources)
        .expect("available entry lookup")
        .expect("second entry");

    assert_eq!(
        director_b.poll_current(&tag_a).await,
        Err(ManagerGenerationCallError::Stale)
    );
    assert_eq!(entry_b.fence.admission_attempts(), 0);
    assert_eq!(current(&director_b, PluginFamily::Resources), Some(tag_b));
}

#[tokio::test(flavor = "current_thread")]
async fn failed_current_poll_retires_and_disposes_exact_generation() {
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
                    failed_poll_component(),
                )],
            ))
            .await
            .expect("publish rejecting current generation"),
        1,
    );
    let expected = current(&director, PluginFamily::Todo).expect("current Todo");
    let entry = director
        .current_entry(PluginFamily::Todo)
        .expect("available entry lookup")
        .expect("current Todo entry");

    let outcome = director
        .poll_current(&expected)
        .await
        .expect("guest lifecycle rejection is a completed poll");

    assert_eq!(outcome.generation(), &expected);
    assert_eq!(outcome.outcome(), Err(LifecycleErrorCode::Failed));
    assert!(current(&director, PluginFamily::Todo).is_none());
    assert!(
        director
            .current_entry(PluginFamily::Todo)
            .expect("available entry lookup")
            .is_none()
    );
    wait_until_disposed(&entry).await;
    assert_eq!(
        *entry
            .shutdown_observation
            .lock()
            .expect("shutdown observation"),
        Some(Ok(LifecycleState::Stopped))
    );
    assert_eq!(
        director.poll_current(&expected).await,
        Err(ManagerGenerationCallError::Stale)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn terminal_current_poll_invalidates_retained_preparation() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    let retained = artifact("1.0.0", '1');
    let replacing = artifact("2.0.0", '2');
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![
                    (
                        PluginFamily::Ask,
                        retained.clone(),
                        stopping_poll_component(),
                    ),
                    (PluginFamily::Web, artifact("1.0.0", '3'), ready_component()),
                ],
            ))
            .await
            .expect("publish initial set"),
        1,
    );
    let pending = director
        .reconcile(candidates(
            &runtime,
            revision(2),
            vec![
                (
                    PluginFamily::Ask,
                    retained.clone(),
                    stopping_poll_component(),
                ),
                (
                    PluginFamily::Web,
                    replacing.clone(),
                    pending_then_ready_component(),
                ),
            ],
        ))
        .await
        .expect("retain pending replacement");
    assert!(matches!(
        pending,
        ReconciliationOutcome::PreparationPending(_)
    ));
    let prepared_entry = preparing(&director, PluginFamily::Web);
    let old = current(&director, PluginFamily::Ask).expect("unchanged Ask generation");

    assert_eq!(
        director
            .poll_current(&old)
            .await
            .expect("terminal current poll")
            .outcome(),
        Ok(LifecycleState::Stopping)
    );
    assert_eq!(
        director.poll_preparation().await,
        Err(ReconciliationError::NoPreparation)
    );
    wait_until_disposed(&prepared_entry).await;
    assert_eq!(
        *prepared_entry
            .shutdown_observation
            .lock()
            .expect("prepared shutdown observation"),
        Some(Ok(LifecycleState::Stopped))
    );

    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(2),
                vec![
                    (PluginFamily::Ask, retained, ready_component()),
                    (PluginFamily::Web, replacing, ready_component()),
                ],
            ))
            .await
            .expect("fresh same-authority reconciliation"),
        2,
    );
    assert!(current(&director, PluginFamily::Ask).is_some());
    assert!(current(&director, PluginFamily::Web).is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn old_revision_tag_selects_the_live_generation_and_reports_current_revision() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    let ready_artifact = artifact("1.0.0", '1');
    let trap_artifact = artifact("1.0.0", '2');
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![
                    (
                        PluginFamily::Workspace,
                        ready_artifact.clone(),
                        ready_component(),
                    ),
                    (
                        PluginFamily::Ui,
                        trap_artifact.clone(),
                        trapping_poll_component(),
                    ),
                ],
            ))
            .await
            .expect("publish revision one"),
        1,
    );
    let old_ready = current(&director, PluginFamily::Workspace).expect("old ready tag");
    let old_trap = current(&director, PluginFamily::Ui).expect("old trap tag");
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(2),
                vec![
                    (PluginFamily::Workspace, ready_artifact, ready_component()),
                    (PluginFamily::Ui, trap_artifact, ready_component()),
                ],
            ))
            .await
            .expect("advance revision without replacing generations"),
        2,
    );

    let ready = director
        .poll_current(&old_ready)
        .await
        .expect("old tag selects live ready generation");
    assert_eq!(ready.generation().revision(), revision(2));
    let selected_trap = current(&director, PluginFamily::Ui).expect("revision two trap tag");
    assert_eq!(
        director.poll_current(&old_trap).await,
        Err(ManagerGenerationCallError::Runtime(Box::new(selected_trap)))
    );
}
