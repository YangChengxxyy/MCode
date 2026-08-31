// Rust guideline compliant 2026-08-31.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use mcode_config::{
    AuthorityRevision, PluginFamily, RootComposition, UiSelection, replace_root_composition,
};
use mcode_plugin_api::{
    FeatureTaskControl, FeatureTaskRequest, FeatureTaskStart, FeatureTaskTerminal,
    FeatureTaskUpdate, MAX_MANAGER_TASK_WIRE_BYTES, OperationId, ResourcesContributionsResult,
    ResourcesTaskProgress, ResourcesTaskRequest, ResourcesTaskResult, TaskErrorCode,
};

use super::test_support::{
    artifact, assert_published, boundary_task_component, candidates,
    configured_forwarding_task_component, configured_nonforwarding_cancel_component, current,
    director, distinct_task_component, feature_deadline_policy, forwarding_task_component,
    poll_once, revision, spinning_resources_pack_component, spinning_task_component,
    terminal_resources_pack_component, trapping_task_component, wait_until_disposed,
};
use super::{
    CurrentManagerGeneration, ManagerGenerationCallError, ManagerGenerationDirector,
    generation_activity_count,
};
use crate::pack_loading::tests::{digest, layout, pack_id, publish_installation, write_component};
use crate::runtime::{LifecycleState, PluginRuntime};

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
async fn terminal_resources_task_cannot_reenter_the_pack_guest() {
    let (_parent, director, expected) = active_resources_director(
        terminal_resources_pack_component(),
        configured_forwarding_task_component(),
    )
    .await;
    let control = start_contributions_task(&director, &expected).await;
    let encoded_control = control.encode().expect("Resources task control");

    let completed_response = director
        .poll_current_task(&expected, &encoded_control)
        .await
        .expect("forward terminal Resources poll")
        .response()
        .to_owned();
    let FeatureTaskUpdate::Completed(completed) = FeatureTaskUpdate::<
        ResourcesTaskProgress,
        ResourcesTaskResult,
    >::decode(completed_response.as_bytes())
    .expect("decode terminal Resources result") else {
        panic!("first Pack pull must complete the Resources task");
    };
    assert_eq!(completed.operation_id(), control.operation_id());
    assert_eq!(completed.task_id(), control.task_id());
    assert_eq!(completed.generation(), control.generation());
    assert_eq!(
        completed.result(),
        &ResourcesTaskResult::Contributions(ResourcesContributionsResult { items: Vec::new() })
    );

    let repeated_response = director
        .poll_current_task(&expected, &encoded_control)
        .await
        .expect("reject repeated terminal Resources poll")
        .response()
        .to_owned();
    let FeatureTaskUpdate::Error(error) = FeatureTaskUpdate::<
        ResourcesTaskProgress,
        ResourcesTaskResult,
    >::decode(repeated_response.as_bytes())
    .expect("decode repeated terminal Resources error") else {
        panic!("a terminal Resources identity must become unknown");
    };
    assert_eq!(error.operation_id(), control.operation_id());
    assert_eq!(error.task_id(), control.task_id());
    assert_eq!(error.generation(), control.generation());
    assert_eq!(error.error().code(), TaskErrorCode::UnknownTask);

    let replacement = start_contributions_task(&director, &expected).await;
    assert_ne!(replacement.task_id(), control.task_id());
    assert_eq!(current(&director, PluginFamily::Resources), Some(expected));
}

async fn active_resources_director(
    pack: Vec<u8>,
    manager: Vec<u8>,
) -> (
    tempfile::TempDir,
    ManagerGenerationDirector,
    CurrentManagerGeneration,
) {
    let (parent, home) = layout();
    let selected_pack = pack_id("active-resources");
    write_component(&home, PluginFamily::Resources, &selected_pack, &pack);
    publish_installation(
        &home,
        PluginFamily::Resources,
        &selected_pack,
        Some(digest(&pack)),
    );
    let mut composition = RootComposition::new(None, Vec::new(), Vec::new(), UiSelection::empty())
        .expect("active Resources configuration");
    composition
        .set_singleton(PluginFamily::Resources, Some(selected_pack))
        .expect("select active Resources Pack");
    let configuration = replace_root_composition(&home, AuthorityRevision::ABSENT, &composition)
        .expect("publish active Resources configuration");
    let runtime = Arc::new(PluginRuntime::with_feature_deadline_policy(
        feature_deadline_policy(),
    ));
    let director = ManagerGenerationDirector::new(Arc::clone(&runtime), home)
        .expect("claim active Resources director");
    director
        .publish_pack_configuration(Some(configuration))
        .await
        .expect("publish active Resources Pack selection");
    assert_published(
        director
            .reconcile(candidates(
                &runtime,
                revision(1),
                vec![(PluginFamily::Resources, artifact("1.0.0", '1'), manager)],
            ))
            .await
            .expect("publish configured forwarding Resources Manager"),
        1,
    );
    let expected = current(&director, PluginFamily::Resources)
        .expect("current configured forwarding Resources Manager");
    assert_eq!(
        director
            .poll_current(&expected)
            .await
            .expect("activate configured Resources Pack")
            .outcome(),
        Ok(LifecycleState::Ready)
    );
    (parent, director, expected)
}

async fn start_contributions_task(
    director: &ManagerGenerationDirector,
    expected: &CurrentManagerGeneration,
) -> FeatureTaskControl {
    let request = FeatureTaskRequest::new(
        OperationId::parse("contributions").expect("contributions operation"),
        expected.generation(),
        ResourcesTaskRequest::Contributions,
    )
    .encode()
    .expect("Resources start request");
    let response = director
        .start_current_task(expected, &request)
        .await
        .expect("forward Resources start")
        .response()
        .to_owned();
    let FeatureTaskStart::Handle(handle) =
        FeatureTaskStart::decode(response.as_bytes()).expect("decode Resources task handle")
    else {
        panic!("active Resources Pack must accept the task");
    };
    FeatureTaskControl::new(
        handle.operation_id().clone(),
        handle.task_id().clone(),
        handle.generation(),
    )
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_an_inflight_resources_pull_closes_without_retiring_the_manager() {
    let (_parent, director, expected) = active_resources_director(
        spinning_resources_pack_component(),
        configured_forwarding_task_component(),
    )
    .await;
    let control = start_contributions_task(&director, &expected).await;
    let encoded_control = control.encode().expect("Resources task control");
    let mut poll = Box::pin(director.poll_current_task(&expected, &encoded_control));
    assert!(poll_once(poll.as_mut()).is_pending());

    let (poll_result, cancel_result) = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(
            poll,
            director.cancel_current_task(&expected, &encoded_control)
        )
    })
    .await
    .expect("in-flight Resources cancellation completes within its bound");
    assert_eq!(
        poll_result,
        Err(ManagerGenerationCallError::TaskClosed(Box::new(
            expected.clone()
        )))
    );
    let cancel_response = cancel_result
        .expect("forward Resources cancellation")
        .response()
        .to_owned();
    let FeatureTaskTerminal::Closed(closed) =
        FeatureTaskTerminal::decode(cancel_response.as_bytes()).expect("decode Resources close")
    else {
        panic!("the winning Resources cancellation must close its task");
    };
    assert_eq!(closed.operation_id(), control.operation_id());
    assert_eq!(closed.task_id(), control.task_id());
    assert_eq!(closed.generation(), control.generation());
    assert_eq!(
        current(&director, PluginFamily::Resources),
        Some(expected.clone())
    );

    let repeated_response = director
        .cancel_current_task(&expected, &encoded_control)
        .await
        .expect("reject repeated Resources cancellation")
        .response()
        .to_owned();
    let FeatureTaskTerminal::Error(error) =
        FeatureTaskTerminal::decode(repeated_response.as_bytes())
            .expect("decode repeated Resources cancellation")
    else {
        panic!("a consumed Resources cancellation must become unknown");
    };
    assert_eq!(error.operation_id(), control.operation_id());
    assert_eq!(error.task_id(), control.task_id());
    assert_eq!(error.generation(), control.generation());
    assert_eq!(error.error().code(), TaskErrorCode::UnknownTask);
    assert_eq!(current(&director, PluginFamily::Resources), Some(expected));
}

#[tokio::test(flavor = "current_thread")]
async fn an_unforwarded_cancel_consumes_its_presignalled_resources_tombstone() {
    let (_parent, director, expected) = active_resources_director(
        spinning_resources_pack_component(),
        configured_nonforwarding_cancel_component(),
    )
    .await;
    let entry = director
        .current_entry(PluginFamily::Resources)
        .expect("available Resources entry lookup")
        .expect("current Resources entry");
    let control = start_contributions_task(&director, &expected).await;
    let encoded_control = control.encode().expect("Resources task control");
    let mut poll = Box::pin(director.poll_current_task(&expected, &encoded_control));
    assert!(poll_once(poll.as_mut()).is_pending());

    let (poll_result, cancel_result) = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(
            poll,
            director.cancel_current_task(&expected, &encoded_control)
        )
    })
    .await
    .expect("unforwarded Resources cancellation completes within its bound");
    let task_closed = Err(ManagerGenerationCallError::TaskClosed(Box::new(
        expected.clone(),
    )));
    assert_eq!(poll_result, task_closed);
    assert_eq!(
        cancel_result,
        Err(ManagerGenerationCallError::TaskClosed(Box::new(
            expected.clone()
        )))
    );
    assert!(!entry.task_sentinel.is_cancelling(&control));
    assert_eq!(current(&director, PluginFamily::Resources), Some(expected));
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_a_presignalled_queued_cancel_retires_its_exact_generation() {
    let (_parent, director, expected) = active_resources_director(
        spinning_resources_pack_component(),
        configured_forwarding_task_component(),
    )
    .await;
    let entry = director
        .current_entry(PluginFamily::Resources)
        .expect("available Resources entry lookup")
        .expect("current Resources entry");
    let control = start_contributions_task(&director, &expected).await;
    let encoded_control = control.encode().expect("Resources task control");
    let mut poll = Box::pin(director.poll_current_task(&expected, &encoded_control));
    assert!(poll_once(poll.as_mut()).is_pending());
    let mut cancel = Box::pin(director.cancel_current_task(&expected, &encoded_control));
    assert!(poll_once(cancel.as_mut()).is_pending());
    assert!(entry.task_sentinel.is_cancelling(&control));

    drop(cancel);

    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), poll)
            .await
            .expect("in-flight Resources poll observes generation retirement"),
        Err(ManagerGenerationCallError::Cancelled(Box::new(
            expected.clone()
        )))
    );
    wait_until_disposed(&entry).await;
    assert!(!entry.task_sentinel.is_cancelling(&control));
    assert!(current(&director, PluginFamily::Resources).is_none());
    assert_eq!(
        director
            .cancel_current_task(&expected, &encoded_control)
            .await,
        Err(ManagerGenerationCallError::Stale)
    );
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
