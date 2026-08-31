// Rust guideline compliant 2026-08-31.

use std::future::Future;
use std::num::NonZeroU32;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use mcode_config::{
    ArtifactRef, AuthorityRevision, CanonicalVersion, HomeLayout, ManagerRecord, PluginFamily,
    Sha256Digest, SourceBindingId, TrustHighWater,
};
use wit_component::{ComponentEncoder, StringEncoding, embed_component_metadata};
use wit_parser::Resolve;

use super::{
    ActiveGeneration, CurrentManagerGeneration, ManagerGenerationDirector,
    ManagerGenerationSnapshot, ReconciliationOutcome,
};
use crate::ComponentLimits;
use crate::manager_loading::{ManagerCandidates, test_support};
use crate::runtime::{FeatureDeadlinePolicyV1, PluginRuntime};

const RESOURCES_IMPORT: &str = "cm32p2|_ex_mcode:feature-pack/resources-pack@0.0.1";
const RESOURCES_EXPORT: &str = "cm32p2|mcode:feature-pack/resources-pack@0.0.1";

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
        "    (func $manager-task",
        shutdown,
    );
    wat::parse_str(source).expect("valid Manager fixture")
}

pub(crate) fn ready_component() -> Vec<u8> {
    wat::parse_str(current_manager_source()).expect("valid current Manager component")
}

pub(super) fn configured_pack_component() -> Vec<u8> {
    wat::parse_str(include_str!(
        "../tests/fixtures/configured_pack_manager_component.wat"
    ))
    .expect("valid configured-Pack Manager component")
}

pub(super) fn configured_forwarding_task_component() -> Vec<u8> {
    let source = include_str!("../tests/fixtures/configured_pack_manager_component.wat");
    let source = source.replacen(
        "  (alias export $feature-service \"activate-packs\" (func $activate-packs))",
        concat!(
            "  (alias export $feature-service \"activate-packs\" (func $activate-packs))\n",
            "  (alias export $feature-service \"start-task\" (func $start-task))\n",
            "  (alias export $feature-service \"poll-task\" (func $poll-task))\n",
            "  (alias export $feature-service \"cancel-task\" (func $cancel-task))",
        ),
        1,
    );
    let source = source.replacen(
        concat!(
            "  (core func $lower-activate-packs (canon lower (func $activate-packs)\n",
            "    (memory $service-memory) (realloc $service-realloc)))",
        ),
        concat!(
            "  (core func $lower-activate-packs (canon lower (func $activate-packs)\n",
            "    (memory $service-memory) (realloc $service-realloc)))\n",
            "  (core func $lower-start-task (canon lower (func $start-task)\n",
            "    (memory $service-memory) (realloc $service-realloc)))\n",
            "  (core func $lower-poll-task (canon lower (func $poll-task)\n",
            "    (memory $service-memory) (realloc $service-realloc)))\n",
            "  (core func $lower-cancel-task (canon lower (func $cancel-task)\n",
            "    (memory $service-memory) (realloc $service-realloc)))",
        ),
        1,
    );
    let source = source.replacen(
        "    (export \"activate-packs\" (func $lower-activate-packs))",
        concat!(
            "    (export \"activate-packs\" (func $lower-activate-packs))\n",
            "    (export \"start-task\" (func $lower-start-task))\n",
            "    (export \"poll-task\" (func $lower-poll-task))\n",
            "    (export \"cancel-task\" (func $lower-cancel-task))",
        ),
        1,
    );
    let source = source.replacen(
        concat!(
            "    (import \"mcode:plugin/feature-service@0.0.1\" \"activate-packs\"\n",
            "      (func $call-activate-packs (param i32 i32 i32)))",
        ),
        concat!(
            "    (import \"mcode:plugin/feature-service@0.0.1\" \"activate-packs\"\n",
            "      (func $call-activate-packs (param i32 i32 i32)))\n",
            "    (import \"mcode:plugin/feature-service@0.0.1\" \"start-task\"\n",
            "      (func $call-start-task (param i32 i32 i32)))\n",
            "    (import \"mcode:plugin/feature-service@0.0.1\" \"poll-task\"\n",
            "      (func $call-poll-task (param i32 i32 i32)))\n",
            "    (import \"mcode:plugin/feature-service@0.0.1\" \"cancel-task\"\n",
            "      (func $call-cancel-task (param i32 i32 i32)))",
        ),
        1,
    );
    let source = replace_function(
        source,
        "    (func $poll",
        "    (func $shutdown",
        concat!(
            "    (func $poll (result i32)\n",
            "      (local $stamp i32)\n",
            "      (local $stamp-len i32)\n",
            "      i32.const 256\n",
            "      call $call-configured-packs\n",
            "      i32.const 256\n",
            "      i32.load8_u\n",
            "      i32.eqz\n",
            "      if (result i32)\n",
            "        i32.const 260\n",
            "        i32.load\n",
            "        local.set $stamp\n",
            "        i32.const 264\n",
            "        i32.load\n",
            "        local.set $stamp-len\n",
            "        local.get $stamp\n",
            "        local.get $stamp-len\n",
            "        call $activate-selection\n",
            "        if (result i32)\n",
            "          i32.const 0\n",
            "          i32.const 0\n",
            "          call $outcome\n",
            "        else\n",
            "          i32.const 1\n",
            "          i32.const 2\n",
            "          call $outcome\n",
            "        end\n",
            "      else\n",
            "        i32.const 1\n",
            "        i32.const 2\n",
            "        call $outcome\n",
            "      end)\n",
        ),
    );
    let source = replace_function(
        source,
        "    (func $manager-task",
        "    (func $realloc",
        concat!(
            "    (func $manager-start (param $ptr i32) (param $len i32) (result i32)\n",
            "      local.get $ptr\n",
            "      local.get $len\n",
            "      i32.const 3072\n",
            "      call $call-start-task\n",
            "      i32.const 3072)\n",
            "    (func $manager-poll (param $ptr i32) (param $len i32) (result i32)\n",
            "      local.get $ptr\n",
            "      local.get $len\n",
            "      i32.const 3072\n",
            "      call $call-poll-task\n",
            "      i32.const 3072)\n",
            "    (func $manager-cancel (param $ptr i32) (param $len i32) (result i32)\n",
            "      local.get $ptr\n",
            "      local.get $len\n",
            "      i32.const 3072\n",
            "      call $call-cancel-task\n",
            "      i32.const 3072)\n",
        ),
    );
    let source = source.replacen(
        "    (export \"manager-task\" (func $manager-task))",
        concat!(
            "    (export \"manager-start\" (func $manager-start))\n",
            "    (export \"manager-poll\" (func $manager-poll))\n",
            "    (export \"manager-cancel\" (func $manager-cancel))",
        ),
        1,
    );
    let source = source.replacen(
        "  (alias core export $guest-instance \"manager-task\" (core func $core-manager-task))",
        concat!(
            "  (alias core export $guest-instance \"manager-start\" (core func $core-manager-start))\n",
            "  (alias core export $guest-instance \"manager-poll\" (core func $core-manager-poll))\n",
            "  (alias core export $guest-instance \"manager-cancel\" (core func $core-manager-cancel))",
        ),
        1,
    );
    let source = source.replacen(
        "core func $core-manager-task",
        "core func $core-manager-start",
        1,
    );
    let source = source.replacen(
        "core func $core-manager-task",
        "core func $core-manager-poll",
        1,
    );
    let source = source.replacen(
        "core func $core-manager-task",
        "core func $core-manager-cancel",
        1,
    );
    let component = wat::parse_str(source).expect("valid configured forwarding Manager fixture");
    wasmparser::Validator::new()
        .validate_all(&component)
        .expect("valid configured forwarding Manager component");
    component
}

pub(super) fn configured_nonforwarding_cancel_component() -> Vec<u8> {
    let source = wasmprinter::print_bytes(configured_forwarding_task_component())
        .expect("print configured forwarding Manager fixture");
    let source = replace_function(
        source,
        "    (func $manager-cancel",
        "    (func $realloc",
        concat!(
            "    (func $manager-cancel (param i32 i32) (result i32)\n",
            "      i32.const 3072\n",
            "      i32.const 0\n",
            "      i32.store\n",
            "      i32.const 3076\n",
            "      i32.const 0\n",
            "      i32.store\n",
            "      i32.const 3072)\n",
        ),
    );
    let component = wat::parse_str(source).expect("valid nonforwarding-cancel Manager fixture WAT");
    wasmparser::Validator::new()
        .validate_all(&component)
        .expect("valid nonforwarding-cancel Manager component");
    component
}

pub(super) fn terminal_resources_pack_component() -> Vec<u8> {
    resources_pack_component(
        r#"global.get $pull-count
    i32.eqz
    if
      i32.const 1
      global.set $pull-count
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
    unreachable"#,
    )
}

pub(super) fn spinning_resources_pack_component() -> Vec<u8> {
    resources_pack_component("(loop $forever (br $forever))\n    unreachable")
}

fn resources_pack_component(pull: &str) -> Vec<u8> {
    let source = format!(
        r#"(module
  (type $drop-type (func (param i32)))
  (type $resource-type (func (param i32) (result i32)))
  (type $invoke-type (func (param i32 i32 i32 i64 i32) (result i32)))
  (type $realloc-type (func (param i32 i32 i32 i32) (result i32)))
  (type $initialize-type (func))
  (import "{RESOURCES_IMPORT}" "resources-operation_drop" (func $resource-drop (type $drop-type)))
  (import "{RESOURCES_IMPORT}" "resources-operation_new" (func $resource-new (type $resource-type)))
  (import "{RESOURCES_IMPORT}" "resources-operation_rep" (func $resource-rep (type $resource-type)))
  (memory $memory 2 1024)
  (global $pull-count (mut i32) (i32.const 0))
  (export "{RESOURCES_EXPORT}|[method]resources-operation.pull" (func $pull))
  (export "{RESOURCES_EXPORT}|[method]resources-operation.pull_post" (func $pull-post))
  (export "{RESOURCES_EXPORT}|invoke" (func $invoke))
  (export "{RESOURCES_EXPORT}|invoke_post" (func $invoke-post))
  (export "{RESOURCES_EXPORT}|resources-operation_dtor" (func $destructor))
  (export "cm32p2_memory" (memory $memory))
  (export "cm32p2_realloc" (func $realloc))
  (export "cm32p2_initialize" (func $initialize))
  (func $pull (type $resource-type) (param $rep i32) (result i32)
    {pull})
  (func $pull-post (type $drop-type) (param i32))
  (func $invoke (type $invoke-type)
    (param i32 i32 i32 i64 i32) (result i32)
    i32.const 0
    i32.const 0
    i32.store
    i32.const 4
    i32.const 7
    call $resource-new
    i32.store
    i32.const 0)
  (func $invoke-post (type $drop-type) (param i32))
  (func $destructor (type $drop-type) (param $rep i32))
  (func $realloc (type $realloc-type) (param i32 i32 i32 i32) (result i32)
    i32.const 4096)
  (func $initialize (type $initialize-type))
)"#,
    );
    encode_resources_component(&source)
}

pub(super) fn feature_deadline_policy() -> FeatureDeadlinePolicyV1 {
    let milliseconds = NonZeroU32::new(5_000).expect("nonzero feature deadline");
    FeatureDeadlinePolicyV1 {
        session_ms: milliseconds,
        compaction_ms: milliseconds,
        resources_ms: milliseconds,
        ask_ms: milliseconds,
        todo_ms: milliseconds,
        mcp_ms: milliseconds,
        usage_ms: milliseconds,
        subagents_ms: milliseconds,
        workspace_ms: milliseconds,
        ui_ms: milliseconds,
    }
}

fn encode_resources_component(source: &str) -> Vec<u8> {
    let mut resolve = Resolve::default();
    let package = resolve
        .push_str(
            "resources",
            include_str!("../../mcode-plugin-api/wit/feature-pack/resources.wit"),
        )
        .expect("Resources WIT");
    let world = resolve
        .select_world(&[package], Some("resources"))
        .expect("Resources world");
    let mut module = wat::parse_str(source).expect("terminal Resources core module");
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8)
        .expect("embed Resources metadata");
    ComponentEncoder::default()
        .module(&module)
        .expect("decode terminal Resources module")
        .validate(true)
        .encode()
        .expect("encode terminal Resources component")
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

fn distinct_task_source() -> String {
    let source = replace_function(
        current_manager_source(),
        "    (func $manager-task",
        "    (func $realloc",
        concat!(
            "    (data (i32.const 128) \"started\")\n",
            "    (data (i32.const 144) \"polled\")\n",
            "    (data (i32.const 160) \"cancelled\")\n",
            "    (func $task-result (param $ptr i32) (param $len i32) (result i32)\n",
            "      i32.const 8\n",
            "      local.get $ptr\n",
            "      i32.store\n",
            "      i32.const 12\n",
            "      local.get $len\n",
            "      i32.store\n",
            "      i32.const 8)\n",
            "    (func $manager-start (param i32 i32) (result i32)\n",
            "      i32.const 128\n",
            "      i32.const 7\n",
            "      call $task-result)\n",
            "    (func $manager-poll (param i32 i32) (result i32)\n",
            "      i32.const 144\n",
            "      i32.const 6\n",
            "      call $task-result)\n",
            "    (func $manager-cancel (param i32 i32) (result i32)\n",
            "      i32.const 160\n",
            "      i32.const 9\n",
            "      call $task-result)\n",
        ),
    );
    let source = source.replace(
        "    (export \"manager-task\" (func $manager-task))",
        concat!(
            "    (export \"manager-start\" (func $manager-start))\n",
            "    (export \"manager-poll\" (func $manager-poll))\n",
            "    (export \"manager-cancel\" (func $manager-cancel))",
        ),
    );
    let source = source.replace(
        "  (alias core export $guest-instance \"manager-task\" (core func $core-manager-task))",
        concat!(
            "  (alias core export $guest-instance \"manager-start\" (core func $core-manager-start))\n",
            "  (alias core export $guest-instance \"manager-poll\" (core func $core-manager-poll))\n",
            "  (alias core export $guest-instance \"manager-cancel\" (core func $core-manager-cancel))",
        ),
    );
    let source = source.replacen(
        "core func $core-manager-task",
        "core func $core-manager-start",
        1,
    );
    let source = source.replacen(
        "core func $core-manager-task",
        "core func $core-manager-poll",
        1,
    );
    source.replacen(
        "core func $core-manager-task",
        "core func $core-manager-cancel",
        1,
    )
}

pub(super) fn distinct_task_component() -> Vec<u8> {
    wat::parse_str(distinct_task_source()).expect("valid distinct task Manager fixture")
}

pub(super) fn boundary_task_component() -> Vec<u8> {
    let source = distinct_task_source().replace(
        "    (memory (export \"memory\") 1 1024)",
        "    (memory (export \"memory\") 2 1024)",
    );
    let source = replace_function(
        source,
        "    (func $manager-start",
        "    (func $manager-poll",
        concat!(
            "    (func $manager-start (param i32 i32) (result i32)\n",
            "      i32.const 8\n",
            "      i32.const 0\n",
            "      i32.store\n",
            "      i32.const 12\n",
            "      i32.const 65536\n",
            "      i32.store\n",
            "      i32.const 8)\n",
        ),
    );
    let source = replace_function(
        source,
        "    (func $manager-poll",
        "    (func $manager-cancel",
        concat!(
            "    (func $manager-poll (param i32 i32) (result i32)\n",
            "      i32.const 8\n",
            "      i32.const 0\n",
            "      i32.store\n",
            "      i32.const 12\n",
            "      i32.const 65537\n",
            "      i32.store\n",
            "      i32.const 8)\n",
        ),
    );
    wat::parse_str(source).expect("valid task wire boundary Manager fixture")
}

pub(super) fn spinning_task_component() -> Vec<u8> {
    let source = wasmprinter::print_bytes(distinct_task_component())
        .expect("print distinct task Manager fixture");
    let source = replace_function(
        source,
        "    (func $manager-start",
        "    (func $manager-poll",
        &spin_function("manager-start", " (param i32 i32)"),
    );
    wat::parse_str(source).expect("valid spinning task Manager fixture")
}

pub(super) fn trapping_task_component() -> Vec<u8> {
    let source = wasmprinter::print_bytes(distinct_task_component())
        .expect("print distinct task Manager fixture");
    let source = replace_function(
        source,
        "    (func $manager-start",
        "    (func $manager-poll",
        "    (func $manager-start (param i32 i32) (result i32) unreachable)\n",
    );
    wat::parse_str(source).expect("valid trapping task Manager fixture")
}

pub(super) fn gateway_calling_component() -> Vec<u8> {
    let mut source = current_manager_source().replacen(
        "(import \"mcode:plugin/feature-service@0.0.1\" (instance",
        "(import \"mcode:plugin/feature-service@0.0.1\" (instance $feature-service",
        1,
    );
    let guest_index = source
        .find("  (core module $guest")
        .expect("guest module marker");
    source.insert_str(
        guest_index,
        concat!(
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

pub(super) fn forwarding_task_component() -> Vec<u8> {
    let source = wasmprinter::print_bytes(gateway_calling_component())
        .expect("print gateway-calling Manager fixture");
    let source = replace_function(
        source,
        "    (func $manager-task",
        "    (func $realloc",
        concat!(
            "    (func $manager-task (param $ptr i32) (param $len i32) (result i32)\n",
            "      local.get $ptr\n",
            "      local.get $len\n",
            "      i32.const 96\n",
            "      call $call-start-task\n",
            "      i32.const 96)\n",
        ),
    );
    wat::parse_str(source).expect("valid forwarding task Manager fixture")
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
    let pack_home = HomeLayout::from_root(
        std::env::current_dir()
            .expect("current test directory")
            .join("target")
            .join("inactive-pack-home"),
    )
    .expect("valid inactive Pack home");
    ManagerGenerationDirector::new(Arc::clone(runtime), pack_home)
        .expect("claim test runtime director")
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
