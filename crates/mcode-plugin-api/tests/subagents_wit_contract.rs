//! Parse-based sole-current Subagents FeaturePack WIT contract tests.

mod support;

use support::{
    assert_component_encoding, assert_denied_names, assert_denied_types,
    assert_freestanding_function, assert_invoke_and_pull, assert_json, assert_lf,
    assert_rule_inventory, assert_semantic_sha256, assert_world_topology, package_interface, parse,
    semantic_rules, type_inventory,
};
use wit_parser::{PackageId, Resolve};

const SOURCE: &str = include_str!("../wit/feature-pack/subagents.wit");
const GOLDEN: &str = include_str!("../goldens/feature_subagents_current.wit");
const SEMANTICS: &str = include_str!("../goldens/feature_subagents_current.jsonl");
const SEMANTICS_SHA256: &str = "a2f125ad719d792916c2c2d8d036412411144964ceb3d012bcf2c430e4c2ba79";
const PACKAGE_ID: &str = "mcode:feature-pack@0.0.1";
const HOST_INVENTORY: &str = r#"step-outcome=enum(continue,success,changes-requested,failed)
step-output=record(outcome:step-outcome,summary:string,retained-session-id:option<string>)
recovery-request=record(job-id:string)
job-outcome=enum(success,changes-requested,failed)
job-result=record(job-id:string,outcome:job-outcome,summary:string,retained-session-id:option<string>)
recovery-receipt=variant(recovered:job-result,unrecoverable)
recovery-output=record(job-id:string,receipt:recovery-receipt)
job-mode=enum(run,review,fix)
step-request=record(job-id:string,attempt:u8,mode:job-mode)
subagents-host-error=enum(isolation-unavailable,stale-job,crash-unrecoverable,limit,unavailable,cancelled)
"#;
const PACK_INVENTORY: &str = r#"recover-request=record(job-id:string)
queued-progress=record(position:u16)
review-round-progress=record(current:u8,total:u8)
job-mode=enum(run,review,fix)
running-progress=record(attempt:u8,phase:job-mode)
subagents-progress=variant(queued:queued-progress,running:running-progress,review-round:review-round-progress,recovering)
isolation-mode=enum(shared,worktree)
role-info=record(id:string,title:string,modes:list<job-mode>)
roles-result=record(items:list<role-info>)
job-outcome=enum(success,changes-requested,failed)
job-result=record(job-id:string,outcome:job-outcome,summary:string,retained-session-id:option<string>)
subagents-result=variant(roles:roles-result,job:job-result)
job-reservation-view=record(reservation-id:string,job-id:string)
enqueue-request=record(job-id:string,reservation:job-reservation-view,role:string,task:string,mode:job-mode,isolation:isolation-mode,retain-session:bool,review-target:option<string>,max-attempts:u8)
subagents-request=variant(roles,enqueue:enqueue-request,recover:recover-request)
subagents-error=enum(invalid-argument,role-not-found,queue-full,isolation-unavailable,stale-job,crash-unrecoverable,limit,unavailable,cancelled)
subagents-pull=variant(pending,progress:subagents-progress,complete:subagents-result,failed:subagents-error)
subagents-operation=resource
"#;
const SEMANTIC_RULES: &str = "denied-surface dto-bounds logical-charge operation-authority ownership progress-reducer recovery-reducer reservation-authority stable-errors stage step-reducer terminal-matrix text-safety topology";

#[test]
fn subagents_artifacts_are_identical_lf_and_have_exact_shape() {
    assert_eq!(SOURCE.as_bytes(), GOLDEN.as_bytes());
    assert_lf("subagents.wit", SOURCE);
    assert_lf("feature_subagents_current.wit", GOLDEN);
    let (resolve, package_id) = parse("subagents.wit", SOURCE);
    assert_component_encoding("subagents.wit", &resolve, package_id);
    assert_shape(&resolve, package_id);
}

#[test]
fn subagents_semantics_have_exact_rules_and_critical_values() {
    assert_lf("feature_subagents_current.jsonl", SEMANTICS);
    assert_semantic_sha256(
        "feature_subagents_current.jsonl",
        SEMANTICS,
        SEMANTICS_SHA256,
        (
            r#""payloadFreeCases":["roles"]"#,
            r#""payloadFreeCases":[]"#,
        ),
    );
    let rules = semantic_rules(SEMANTICS);
    assert_rule_inventory(&rules, SEMANTIC_RULES.split_ascii_whitespace());
    assert_json(
        &rules,
        "topology",
        "/world",
        r#""mcode:feature-pack/subagents@0.0.1""#,
    );
    assert_json(
        &rules,
        "topology",
        "/hostInterface",
        r#""mcode:feature-pack/subagents-host@0.0.1""#,
    );
    assert_json(
        &rules,
        "topology",
        "/packInterface",
        r#""mcode:feature-pack/subagents-pack@0.0.1""#,
    );
    assert_json(
        &rules,
        "operation-authority",
        "/payloadFreeCases",
        r#"["roles"]"#,
    );
    assert_json(
        &rules,
        "dto-bounds",
        "/jobId/grammar",
        r#""sub1-[0-9a-f]{32}""#,
    );
    assert_json(
        &rules,
        "dto-bounds",
        "/reservationId/grammar",
        r#""sjr1-[0-9a-f]{32}""#,
    );
    assert_json(
        &rules,
        "dto-bounds",
        "/retainedSessionId/grammar",
        r#""rs1-[0-9a-f]{32}""#,
    );
    assert_json(&rules, "dto-bounds", "/attempt/max", "8");
    assert_json(&rules, "dto-bounds", "/task/bytesMax", "65536");
    assert_json(&rules, "reservation-authority", "/reviewTarget/run", "null");
    assert_json(&rules, "progress-reducer", "/attempts/contiguous", "true");
    assert_json(
        &rules,
        "progress-reducer",
        "/review/fixedMode",
        r#""review""#,
    );
    assert_json(
        &rules,
        "recovery-reducer",
        "/copiedFields",
        r#"["job-id","outcome","summary","retained-session-id"]"#,
    );
    assert_json(&rules, "stage", "/binaryPreflightClaim", "false");
    assert_json(&rules, "stage", "/runtimeClaim", "false");
}

#[test]
fn subagents_mutations_cannot_erase_or_cross_family_types() {
    let erased = SOURCE.replacen(
        "reservation: job-reservation-view",
        "reservation: string",
        1,
    );
    let (resolve, package_id) = parse("erased reservation", &erased);
    let pack = package_interface(&resolve, package_id, "subagents-pack");
    assert_ne!(type_inventory(&resolve, pack), PACK_INVENTORY);

    let crossed = SOURCE.replacen(
        "interface subagents-pack {",
        "interface subagents-pack {\n    type ui-model = string;",
        1,
    );
    let (resolve, package_id) = parse("cross-family type", &crossed);
    let pack = package_interface(&resolve, package_id, "subagents-pack");
    assert_ne!(type_inventory(&resolve, pack), PACK_INVENTORY);
    assert!(pack.types.contains_key("ui-model"));

    let relabeled = SOURCE.replacen(
        "invoke: func(request: subagents-request)",
        "invoke: func(payload: subagents-request)",
        1,
    );
    let (resolve, package_id) = parse("relabeled invoke", &relabeled);
    let pack = package_interface(&resolve, package_id, "subagents-pack");
    assert_ne!(pack.functions["invoke"].params[0].name, "request");
}

fn assert_shape(resolve: &Resolve, package_id: PackageId) {
    let (pack_id, host_id) = assert_world_topology(
        resolve,
        package_id,
        PACKAGE_ID,
        "subagents",
        &["subagents-host", "subagents-pack"],
        "subagents-host",
        "subagents-pack",
    );
    let host = &resolve.interfaces[host_id];
    let pack = &resolve.interfaces[pack_id];
    assert_eq!(host.name.as_deref(), Some("subagents-host"));
    assert_eq!(pack.name.as_deref(), Some("subagents-pack"));
    assert_eq!(type_inventory(resolve, host), HOST_INVENTORY);
    assert_eq!(type_inventory(resolve, pack), PACK_INVENTORY);
    assert_eq!(host.functions.len(), 2);
    assert_freestanding_function(
        resolve,
        host,
        "run-step",
        "request",
        "step-request",
        "step-output",
        "subagents-host-error",
    );
    assert_freestanding_function(
        resolve,
        host,
        "recover-step",
        "request",
        "recovery-request",
        "recovery-output",
        "subagents-host-error",
    );
    assert_eq!(pack.functions.len(), 2);
    assert_invoke_and_pull(
        resolve,
        pack,
        "subagents-operation",
        "subagents-request",
        "subagents-error",
        "subagents-pull",
    );
    assert_denied_names(
        resolve,
        package_id,
        &["value", "metadata", "pack-operation"],
        &["ui-", "workspace-"],
    );
    assert_denied_types(resolve);
}

// Rust guideline compliant 2026-08-30.
