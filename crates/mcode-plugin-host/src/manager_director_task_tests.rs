// Rust guideline compliant 2026-08-31.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use mcode_config::PluginFamily;
use mcode_plugin_api::{FeatureTaskStart, MAX_MANAGER_TASK_WIRE_BYTES, TaskErrorCode};

use super::test_support::{
    artifact, assert_published, boundary_task_component, candidates, current, director,
    distinct_task_component, forwarding_task_component, poll_once, revision,
    spinning_task_component, trapping_task_component, wait_until_disposed,
};
use super::{ManagerGenerationCallError, generation_activity_count};
use crate::runtime::PluginRuntime;

#[tokio::test(flavor = "current_thread")]
async fn current_task_exports_are_distinct_and_generation_stamped() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    let selected = artifact("1.0.0", '1');
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![(
                    PluginFamily::Resources,
                    selected.clone(),
                    distinct_task_component(),
                )],
            ))
            .await
            .expect("publish distinct task Manager"),
        1,
    );
    let expected = current(&director, PluginFamily::Resources).expect("current Resources");

    let start = director
        .start_current_task(&expected, "start request")
        .await
        .expect("start task export");
    assert_eq!(start.response(), "started");
    assert_eq!(start.generation(), &expected);
    assert_eq!(
        director
            .poll_current_task(&expected, "poll request")
            .await
            .expect("poll task export")
            .response(),
        "polled"
    );
    assert_eq!(
        director
            .cancel_current_task(&expected, "cancel request")
            .await
            .expect("cancel task export")
            .response(),
        "cancelled"
    );

    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(2),
                vec![(PluginFamily::Resources, selected, distinct_task_component())],
            ))
            .await
            .expect("advance authority without replacing Manager"),
        2,
    );
    let advanced = director
        .start_current_task(&expected, "old revision tag")
        .await
        .expect("old tag selects retained Manager");
    assert_eq!(advanced.response(), "started");
    assert_eq!(advanced.generation().revision(), revision(2));
}

#[tokio::test(flavor = "current_thread")]
async fn current_manager_task_export_reaches_the_feature_service_import() {
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
                    forwarding_task_component(),
                )],
            ))
            .await
            .expect("publish forwarding task Manager"),
        1,
    );
    let expected = current(&director, PluginFamily::Providers).expect("current Providers");

    let response = director
        .start_current_task(&expected, "{}")
        .await
        .expect("forward task through Manager")
        .response()
        .to_owned();
    let FeatureTaskStart::Rejected(rejection) =
        FeatureTaskStart::decode(response.as_bytes()).expect("decode Host task rejection")
    else {
        panic!("inactive Pack task path must reject before task allocation");
    };
    assert_eq!(rejection.error().code(), TaskErrorCode::FeatureUnavailable);
}

#[tokio::test(flavor = "current_thread")]
async fn oversized_task_input_is_rejected_before_guest_entry() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![(
                    PluginFamily::Ask,
                    artifact("1.0.0", '1'),
                    trapping_task_component(),
                )],
            ))
            .await
            .expect("publish trapping task Manager"),
        1,
    );
    let expected = current(&director, PluginFamily::Ask).expect("current Ask");
    let oversized = "x".repeat(MAX_MANAGER_TASK_WIRE_BYTES + 1);

    assert_eq!(
        director.start_current_task(&expected, &oversized).await,
        Err(ManagerGenerationCallError::InvalidRequest(Box::new(
            expected.clone()
        )))
    );
    assert_eq!(
        current(&director, PluginFamily::Ask),
        Some(expected.clone())
    );

    assert_eq!(
        director.start_current_task(&expected, "enter guest").await,
        Err(ManagerGenerationCallError::Runtime(Box::new(
            expected.clone()
        )))
    );
    assert!(current(&director, PluginFamily::Ask).is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn task_wire_accepts_exact_bounds_and_retires_on_oversized_output() {
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
                    boundary_task_component(),
                )],
            ))
            .await
            .expect("publish task wire boundary Manager"),
        1,
    );
    let expected = current(&director, PluginFamily::Resources).expect("current Resources");
    let exact = "x".repeat(MAX_MANAGER_TASK_WIRE_BYTES);

    assert_eq!(
        director
            .start_current_task(&expected, &exact)
            .await
            .expect("exact input and output bounds")
            .response()
            .len(),
        MAX_MANAGER_TASK_WIRE_BYTES
    );
    assert_eq!(
        director.poll_current_task(&expected, "poll").await,
        Err(ManagerGenerationCallError::Runtime(Box::new(
            expected.clone()
        )))
    );
    assert!(current(&director, PluginFamily::Resources).is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn reload_cancels_inflight_and_queued_task_exports() {
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
                    spinning_task_component(),
                )],
            ))
            .await
            .expect("publish spinning task Manager"),
        1,
    );
    let old = current(&director, PluginFamily::Usage).expect("current Usage");
    let old_entry = director
        .current_entry(PluginFamily::Usage)
        .expect("available entry lookup")
        .expect("current Usage entry");
    let first_director = Arc::clone(&director);
    let mut first = Box::pin(first_director.start_current_task(&old, "first"));
    assert!(poll_once(first.as_mut()).is_pending());
    let second_director = Arc::clone(&director);
    let mut second = Box::pin(second_director.start_current_task(&old, "second"));
    assert!(poll_once(second.as_mut()).is_pending());
    assert_eq!(
        generation_activity_count(old_entry.fence.state.load(Ordering::Acquire)),
        2
    );

    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(2),
                vec![(
                    PluginFamily::Usage,
                    artifact("2.0.0", '2'),
                    distinct_task_component(),
                )],
            ))
            .await
            .expect("replace spinning task Manager"),
        2,
    );

    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), first)
            .await
            .expect("in-flight task export cancellation"),
        Err(ManagerGenerationCallError::Cancelled(Box::new(old.clone())))
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), second)
            .await
            .expect("queued task export cancellation"),
        Err(ManagerGenerationCallError::Cancelled(Box::new(old.clone())))
    );
    assert_eq!(
        generation_activity_count(old_entry.fence.state.load(Ordering::Acquire)),
        0
    );
    wait_until_disposed(&old_entry).await;
    let replacement = current(&director, PluginFamily::Usage).expect("replacement Usage");
    assert_eq!(
        director
            .start_current_task(&replacement, "replacement")
            .await
            .expect("replacement task export")
            .response(),
        "started"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_an_entered_task_export_retires_the_generation() {
    let runtime = Arc::new(PluginRuntime::new());
    let director = director(&runtime);
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![(
                    PluginFamily::Web,
                    artifact("1.0.0", '1'),
                    spinning_task_component(),
                )],
            ))
            .await
            .expect("publish spinning Web task Manager"),
        1,
    );
    let expected = current(&director, PluginFamily::Web).expect("current Web");
    let entry = director
        .current_entry(PluginFamily::Web)
        .expect("available entry lookup")
        .expect("current Web entry");
    let mut task = Box::pin(director.start_current_task(&expected, "drop"));
    assert!(poll_once(task.as_mut()).is_pending());

    drop(task);

    assert!(current(&director, PluginFamily::Web).is_none());
    wait_until_disposed(&entry).await;
    assert_eq!(
        director.start_current_task(&expected, "stale").await,
        Err(ManagerGenerationCallError::Stale)
    );
}
