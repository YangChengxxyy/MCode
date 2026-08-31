// Rust guideline compliant 2026-08-31.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use mcode_plugin_api::{
    ResourcesCatalogEntry, ResourcesCatalogRequest, ResourcesCatalogResult, ResourcesContribution,
    ResourcesContributionKind, ResourcesContributionsResult, ResourcesMedia, ResourcesMessageRole,
    ResourcesPromptArg, ResourcesPromptEntry, ResourcesPromptMessage, ResourcesPromptParam,
    ResourcesPromptResult, ResourcesReadRequest, ResourcesReadResult, ResourcesRenderPromptRequest,
    ResourcesResourceEntry, ResourcesTaskProgress, ResourcesTaskRequest, ResourcesTaskResult,
};
use wit_component::{ComponentEncoder, StringEncoding, embed_component_metadata};
use wit_parser::Resolve;

use super::{
    ResourcesPackActor, ResourcesPackCallError, ResourcesPackError, ResourcesPackPull, guest,
    lift_pull, lift_result, lower_request,
};
use crate::runtime::{OPERATION_FUEL_BUDGET, PluginRuntime};
use crate::{ComponentLimits, ComponentWorld};

const RESOURCE_IMPORT: &str = "cm32p2|_ex_mcode:feature-pack/resources-pack@0.0.1";
const RESOURCE_EXPORT: &str = "cm32p2|mcode:feature-pack/resources-pack@0.0.1";

async fn actor(component: Vec<u8>) -> ResourcesPackActor {
    let runtime = PluginRuntime::new();
    let component = runtime
        .compile_pack(
            component,
            ComponentWorld::Resources,
            ComponentLimits::default(),
        )
        .expect("compile Resources actor fixture");
    let mut owner = runtime.new_owner().expect("Resources actor owner");
    let instance = owner
        .instantiate_pack(&component)
        .await
        .expect("instantiate Resources actor fixture");
    ResourcesPackActor::from_parts(owner, instance).expect("bind Resources actor")
}

fn resources_component(invoke: &str, pull: &str, destructor: &str) -> Vec<u8> {
    let source = format!(
        r#"(module
  (type $drop-type (func (param i32)))
  (type $resource-type (func (param i32) (result i32)))
  (type $invoke-type (func (param i32 i32 i32 i64 i32) (result i32)))
  (type $realloc-type (func (param i32 i32 i32 i32) (result i32)))
  (type $initialize-type (func))
  (import "{RESOURCE_IMPORT}" "resources-operation_drop" (func $resource-drop (type $drop-type)))
  (import "{RESOURCE_IMPORT}" "resources-operation_new" (func $resource-new (type $resource-type)))
  (import "{RESOURCE_IMPORT}" "resources-operation_rep" (func $resource-rep (type $resource-type)))
  (memory $memory 2 1024)
  (global $pull-count (mut i32) (i32.const 0))
  (export "{RESOURCE_EXPORT}|[method]resources-operation.pull" (func $pull))
  (export "{RESOURCE_EXPORT}|[method]resources-operation.pull_post" (func $pull-post))
  (export "{RESOURCE_EXPORT}|invoke" (func $invoke))
  (export "{RESOURCE_EXPORT}|invoke_post" (func $invoke-post))
  (export "{RESOURCE_EXPORT}|resources-operation_dtor" (func $destructor))
  (export "cm32p2_memory" (memory $memory))
  (export "cm32p2_realloc" (func $realloc))
  (export "cm32p2_initialize" (func $initialize))
  (func $pull (type $resource-type) (param $rep i32) (result i32)
    {pull})
  (func $pull-post (type $drop-type) (param i32))
  (func $invoke (type $invoke-type)
    (param i32 i32 i32 i64 i32) (result i32)
    {invoke})
  (func $invoke-post (type $drop-type) (param i32))
  (func $destructor (type $drop-type) (param $rep i32)
    {destructor})
  (func $realloc (type $realloc-type) (param i32 i32 i32 i32) (result i32)
    i32.const 4096)
  (func $initialize (type $initialize-type))
)"#
    );
    encode_component(&source)
}

fn encode_component(source: &str) -> Vec<u8> {
    let mut resolve = Resolve::default();
    let package = resolve
        .push_str(
            "resources",
            include_str!("../../../mcode-plugin-api/wit/feature-pack/resources.wit"),
        )
        .expect("Resources WIT");
    let world = resolve
        .select_world(&[package], Some("resources"))
        .expect("Resources world");
    let mut module = wat::parse_str(source).expect("Resources actor core module");
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8)
        .expect("embed Resources metadata");
    ComponentEncoder::default()
        .module(&module)
        .expect("decode Resources actor module")
        .validate(true)
        .encode()
        .expect("encode Resources actor component")
}

fn successful_invoke() -> &'static str {
    r#"i32.const 0
    i32.const 0
    i32.store
    i32.const 4
    i32.const 7
    call $resource-new
    i32.store
    i32.const 0"#
}

fn sequenced_pull() -> &'static str {
    r#"global.get $pull-count
    i32.const 1
    i32.add
    global.set $pull-count
    global.get $pull-count
    i32.const 1
    i32.eq
    if
      i32.const 0
      i32.const 0
      i32.store
      i32.const 0
      return
    end
    global.get $pull-count
    i32.const 2
    i32.eq
    if
      i32.const 0
      i32.const 1
      i32.store
      i32.const 8
      i32.const 0
      i32.store
      i32.const 0
      return
    end
    global.get $pull-count
    i32.const 3
    i32.eq
    if
      i32.const 0
      i32.const 2
      i32.store
      i32.const 8
      i32.const 3
      i32.store
      i32.const 16
      i32.const 0
      i32.store
      i32.const 20
      i32.const 0
      i32.store
      i32.const 0
      return
    end
    i32.const 0
    i32.const 3
    i32.store
    i32.const 8
    i32.const 1
    i32.store
    i32.const 0"#
}

fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
    let mut context = Context::from_waker(Waker::noop());
    future.as_mut().poll(&mut context)
}

#[tokio::test]
async fn invoke_pull_and_drop_share_one_operation_lease() {
    let mut actor = actor(resources_component(
        successful_invoke(),
        sequenced_pull(),
        "nop",
    ))
    .await;
    let mut operation = actor
        .invoke(&ResourcesTaskRequest::Contributions)
        .await
        .expect("invoke Resources operation");
    let after_invoke = operation.operation.remaining;
    assert!(after_invoke < OPERATION_FUEL_BUDGET);

    assert_eq!(
        actor.pull(&mut operation).await,
        Ok(ResourcesPackPull::Pending)
    );
    assert!(operation.operation.remaining < after_invoke);
    assert_eq!(
        actor.pull(&mut operation).await,
        Ok(ResourcesPackPull::Progress(ResourcesTaskProgress::Loading))
    );
    assert_eq!(
        actor.pull(&mut operation).await,
        Ok(ResourcesPackPull::Complete(
            ResourcesTaskResult::Contributions(ResourcesContributionsResult { items: Vec::new() })
        ))
    );
    assert_eq!(
        actor.pull(&mut operation).await,
        Ok(ResourcesPackPull::Failed(ResourcesPackError::NotFound))
    );
    actor
        .drop_operation(operation)
        .await
        .expect("drop guest Resources operation");
    assert!(actor.owner.is_available());
}

#[tokio::test]
async fn dropping_an_inflight_pack_segment_disposes_the_store() {
    let mut actor = actor(resources_component(
        "loop $forever (br $forever) end unreachable",
        sequenced_pull(),
        "nop",
    ))
    .await;
    let mut invoke = Box::pin(actor.invoke(&ResourcesTaskRequest::Contributions));
    assert!(poll_once(invoke.as_mut()).is_pending());
    drop(invoke);
    assert!(!actor.owner.is_available());
}

#[tokio::test]
async fn dropping_an_inflight_pull_disposes_the_store() {
    let mut actor = actor(resources_component(
        successful_invoke(),
        "loop $forever (br $forever) end unreachable",
        "nop",
    ))
    .await;
    let mut operation = actor
        .invoke(&ResourcesTaskRequest::Contributions)
        .await
        .expect("invoke Resources operation");
    let mut pull = Box::pin(actor.pull(&mut operation));
    assert!(poll_once(pull.as_mut()).is_pending());
    drop(pull);
    assert!(!actor.owner.is_available());
}

#[tokio::test]
async fn pull_trap_disposes_the_store() {
    let mut actor = actor(resources_component(
        successful_invoke(),
        "unreachable",
        "nop",
    ))
    .await;
    let mut operation = actor
        .invoke(&ResourcesTaskRequest::Contributions)
        .await
        .expect("invoke Resources operation");

    assert_eq!(
        actor.pull(&mut operation).await,
        Err(ResourcesPackCallError::Runtime)
    );
    assert!(!actor.owner.is_available());
}

#[tokio::test]
async fn destructor_trap_disposes_the_store() {
    let mut actor = actor(resources_component(
        successful_invoke(),
        sequenced_pull(),
        "unreachable",
    ))
    .await;
    let operation = actor
        .invoke(&ResourcesTaskRequest::Contributions)
        .await
        .expect("invoke Resources operation");

    assert_eq!(
        actor.drop_operation(operation).await,
        Err(ResourcesPackCallError::Runtime)
    );
    assert!(!actor.owner.is_available());
}

#[tokio::test]
async fn an_operation_cannot_cross_pack_actors() {
    let component = resources_component(successful_invoke(), sequenced_pull(), "nop");
    let mut first = actor(component.clone()).await;
    let mut second = actor(component).await;
    let mut operation = first
        .invoke(&ResourcesTaskRequest::Contributions)
        .await
        .expect("first actor operation");

    assert_eq!(
        second.pull(&mut operation).await,
        Err(ResourcesPackCallError::OperationMismatch)
    );
    assert!(first.owner.is_available());
    assert!(second.owner.is_available());
    first
        .drop_operation(operation)
        .await
        .expect("drop with original actor");
}

#[test]
fn every_request_and_pull_case_maps_without_shape_loss() {
    let guest::ResourcesRequest::Catalog(catalog) =
        lower_request(&ResourcesTaskRequest::Catalog(ResourcesCatalogRequest {
            offset: 2,
            limit: 8,
        }))
    else {
        panic!("catalog request must lower to the catalog WIT case");
    };
    assert_eq!(catalog.offset, 2);
    assert_eq!(catalog.limit, 8);

    let guest::ResourcesRequest::Read(read) =
        lower_request(&ResourcesTaskRequest::Read(ResourcesReadRequest {
            id: "guide".into(),
            offset: 3,
            max_bytes: 64,
        }))
    else {
        panic!("read request must lower to the read WIT case");
    };
    assert_eq!(read.id, "guide");
    assert_eq!(read.offset, 3);
    assert_eq!(read.max_bytes, 64);

    let guest::ResourcesRequest::RenderPrompt(prompt) = lower_request(
        &ResourcesTaskRequest::RenderPrompt(ResourcesRenderPromptRequest {
            id: "explain".into(),
            args: vec![ResourcesPromptArg {
                name: "topic".into(),
                value: "Rust".into(),
            }],
        }),
    ) else {
        panic!("render-prompt request must lower to the render-prompt WIT case");
    };
    let [argument] = prompt.args.as_slice() else {
        panic!("render-prompt request must preserve exactly one argument");
    };
    assert_eq!(prompt.id, "explain");
    assert_eq!(argument.name, "topic");
    assert_eq!(argument.value, "Rust");

    assert!(matches!(
        lower_request(&ResourcesTaskRequest::Contributions),
        guest::ResourcesRequest::Contributions
    ));

    assert_eq!(
        lift_pull(guest::ResourcesPull::Pending),
        ResourcesPackPull::Pending
    );
    assert_eq!(
        lift_pull(guest::ResourcesPull::Progress(
            guest::ResourcesProgress::Rendering
        )),
        ResourcesPackPull::Progress(ResourcesTaskProgress::Rendering)
    );
    assert_eq!(
        lift_pull(guest::ResourcesPull::Failed(
            guest::ResourcesError::Cancelled
        )),
        ResourcesPackPull::Failed(ResourcesPackError::Cancelled)
    );
    assert_eq!(lift_result(catalog_result()), expected_catalog_result());
    assert_eq!(
        lift_result(guest::ResourcesResult::Read(guest::ReadResult {
            text: "body".into(),
            next_offset: None,
        })),
        ResourcesTaskResult::Read(ResourcesReadResult {
            text: "body".into(),
            next_offset: None,
        })
    );
    assert_eq!(lift_result(prompt_result()), expected_prompt_result());
    assert_eq!(
        lift_result(guest::ResourcesResult::Contributions(
            guest::ContributionsResult {
                items: vec![guest::Contribution {
                    id: "status".into(),
                    kind: guest::ContributionKind::Status,
                }],
            }
        )),
        ResourcesTaskResult::Contributions(ResourcesContributionsResult {
            items: vec![ResourcesContribution {
                id: "status".into(),
                kind: ResourcesContributionKind::Status,
            }],
        })
    );
}

fn catalog_result() -> guest::ResourcesResult {
    guest::ResourcesResult::Catalog(guest::CatalogResult {
        items: vec![
            guest::CatalogEntry::Resource(guest::ResourceEntry {
                id: "guide".into(),
                title: "Guide".into(),
                media: guest::ResourceMedia::Markdown,
                size_hint: Some(12),
            }),
            guest::CatalogEntry::Prompt(guest::PromptEntry {
                id: "explain".into(),
                title: "Explain".into(),
                params: vec![guest::PromptParam {
                    name: "topic".into(),
                    label: "Topic".into(),
                    required: true,
                }],
            }),
        ],
        next_offset: Some(2),
    })
}

fn expected_catalog_result() -> ResourcesTaskResult {
    ResourcesTaskResult::Catalog(ResourcesCatalogResult {
        items: vec![
            ResourcesCatalogEntry::Resource(ResourcesResourceEntry {
                id: "guide".into(),
                title: "Guide".into(),
                media: ResourcesMedia::Markdown,
                size_hint: Some(12),
            }),
            ResourcesCatalogEntry::Prompt(ResourcesPromptEntry {
                id: "explain".into(),
                title: "Explain".into(),
                params: vec![ResourcesPromptParam {
                    name: "topic".into(),
                    label: "Topic".into(),
                    required: true,
                }],
            }),
        ],
        next_offset: Some(2),
    })
}

fn prompt_result() -> guest::ResourcesResult {
    guest::ResourcesResult::Prompt(guest::PromptResult {
        id: "explain".into(),
        messages: vec![guest::PromptMessage {
            role: guest::MessageRole::Assistant,
            text: "Answer".into(),
        }],
    })
}

fn expected_prompt_result() -> ResourcesTaskResult {
    ResourcesTaskResult::Prompt(ResourcesPromptResult {
        id: "explain".into(),
        messages: vec![ResourcesPromptMessage {
            role: ResourcesMessageRole::Assistant,
            text: "Answer".into(),
        }],
    })
}
