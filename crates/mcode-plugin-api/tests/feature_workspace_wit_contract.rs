//! Parse-based sole-current Workspace FeaturePack contract tests.

mod support;

use support::{
    allowed_vocabulary, assert_denied_types, assert_freestanding_function, assert_invoke_and_pull,
    assert_json, assert_lf, assert_rule_inventory, assert_semantic_sha256, assert_world_topology,
    interface_keys, package_interface, parse, semantic_rules, type_inventory,
};
use wit_parser::{PackageId, Resolve};

const SOURCE: &str = include_str!("../wit/feature-pack/workspace.wit");
const GOLDEN: &str = include_str!("../goldens/feature_workspace_current.wit");
const SEMANTICS: &str = include_str!("../goldens/feature_workspace_current.jsonl");
const SEMANTICS_SHA256: &str = "ed50e8fb40928a7f5546df61a3abe040539849d7f5fa8bd8efa2fefff17a1d3c";
const PACKAGE: &str = "mcode:feature-pack@0.0.1";
const FAMILY: &str = "workspace";
const PACK: &str = "workspace-pack";
const HOST: &str = "workspace-host";
const DENIED_VOCABULARY: &[&str] = &[
    "type value",
    "record value",
    "value:",
    "type metadata",
    "record metadata",
    "metadata:",
    "pack-operation",
    "wasi:",
    "map<",
    "future<",
    "stream<",
    "f32",
    "f64",
    "error-context",
    "todo-pack",
    "web-pack",
    "mcp-pack",
];
const PACK_INVENTORY: &str = r#"inspect-request=record(checkpoint-id:string,fingerprint:string,offset:u32,limit:u16)
workspace-progress=enum(scanning,snapshotting,rolling-back)
checkpoint-result=record(checkpoint-id:string,fingerprint:string,files:u64,dirs:u64,bytes:u64)
workspace-path=alias(string)
conflict-result=record(paths:list<workspace-path>,truncated:bool)
rolled-back-result=record(fingerprint:string)
workspace-error=variant(invalid-argument,not-found,conflict:conflict-result,unrollbackable,unsafe-entry,limit,unavailable,cancelled)
scan-request=record(checkpoint-id:string,fingerprint:string,offset:u32,limit:u16)
workspace-snapshot-view=record(fingerprint:string,files:u64,dirs:u64,bytes:u64)
rollback-output=record(fingerprint:string)
tracking-kind=enum(tracked,untracked,ignored)
change-kind=enum(added,modified,deleted,metadata,unrollbackable)
change=record(path:workspace-path,tracking:tracking-kind,kind:change-kind,hash:option<string>)
inspected-result=record(items:list<change>,next:option<u32>)
workspace-result=variant(checkpoint:checkpoint-result,inspected:inspected-result,rolled-back:rolled-back-result)
workspace-pull=variant(pending,progress:workspace-progress,complete:workspace-result,failed:workspace-error)
scan-page=record(items:list<change>,next:option<u32>,snapshot:workspace-snapshot-view)
checkpoint-reservation-view=record(checkpoint-id:string,reservation-id:string,expected-current:string)
checkpoint-request=record(reservation:checkpoint-reservation-view)
rollback-request=record(checkpoint-id:string,expected-current:string,reservation:checkpoint-reservation-view)
workspace-request=variant(checkpoint:checkpoint-request,inspect:inspect-request,rollback:rollback-request)
workspace-host-error=variant(not-found,conflict:conflict-result,unrollbackable,unsafe-entry,limit,unavailable,cancelled)
workspace-operation=resource
"#;
const HOST_INVENTORY: &str = r#"scan-request=record(checkpoint-id:string,fingerprint:string,offset:u32,limit:u16)
rollback-output=record(fingerprint:string)
workspace-path=alias(string)
tracking-kind=enum(tracked,untracked,ignored)
change-kind=enum(added,modified,deleted,metadata,unrollbackable)
change=record(path:workspace-path,tracking:tracking-kind,kind:change-kind,hash:option<string>)
workspace-snapshot-view=record(fingerprint:string,files:u64,dirs:u64,bytes:u64)
scan-page=record(items:list<change>,next:option<u32>,snapshot:workspace-snapshot-view)
checkpoint-reservation-view=record(checkpoint-id:string,reservation-id:string,expected-current:string)
rollback-request=record(checkpoint-id:string,expected-current:string,reservation:checkpoint-reservation-view)
conflict-result=record(paths:list<workspace-path>,truncated:bool)
workspace-host-error=variant(not-found,conflict:conflict-result,unrollbackable,unsafe-entry,limit,unavailable,cancelled)
"#;
const RULES: [&str; 15] = [
    "topology",
    "operation-authority",
    "table-authority",
    "workspace-path",
    "scalar-bounds",
    "first-scan",
    "scan-cursor",
    "scan-page",
    "invalid-page-effects",
    "checkpoint-reducer",
    "inspect-reducer",
    "rollback-reducer",
    "stable-errors",
    "logical-charge-and-boundaries",
    "stage-scope",
];

#[test]
fn artifacts_are_identical_lf_and_parse_to_the_exact_contract() {
    assert_eq!(SOURCE.as_bytes(), GOLDEN.as_bytes());
    for (path, source) in [("workspace.wit", SOURCE), ("golden", GOLDEN)] {
        assert_lf(path, source);
        let (resolve, package_id) = parse(path, source);
        assert_contract(&resolve, package_id);
    }
}

#[test]
fn semantics_have_exact_rules_and_critical_values() {
    assert_lf("feature_workspace_current.jsonl", SEMANTICS);
    assert_semantic_sha256(
        "feature_workspace_current.jsonl",
        SEMANTICS,
        SEMANTICS_SHA256,
        (r#""scanPullCap":65536"#, r#""scanPullCap":65535"#),
    );
    let rules = semantic_rules(SEMANTICS);
    assert_rule_inventory(&rules, RULES);
    assert_json(
        &rules,
        "table-authority",
        "/checkpointId/grammar",
        r#""cp1-[0-9a-f]{32}""#,
    );
    assert_json(&rules, "workspace-path", "/utf8Bytes/max", "512");
    assert_json(&rules, "scalar-bounds", "/scanLimit/max", "256");
    assert_json(&rules, "first-scan", "/checkpoint/offset", "0");
    assert_json(&rules, "first-scan", "/rollback/limit", "256");
    assert_json(&rules, "scan-cursor", "/rollbackScanMax", "65535");
    assert_json(&rules, "invalid-page-effects", "/applyRollback", "0");
    assert_json(&rules, "rollback-reducer", "/applyRollbackCount", "1");
    assert_json(
        &rules,
        "logical-charge-and-boundaries",
        "/resultChargeMax",
        "1048576",
    );
}

#[test]
fn mutations_detect_type_erasure_and_cross_family_contamination() {
    let erased = SOURCE.replacen("path: workspace-path", "path: string", 1);
    let (resolve, package_id) = parse("erased", &erased);
    assert_ne!(
        type_inventory(&resolve, package_interface(&resolve, package_id, PACK)),
        PACK_INVENTORY
    );

    let contaminated = SOURCE.replacen(
        "world workspace",
        "interface todo-pack {}\n\nworld workspace",
        1,
    );
    let (resolve, package_id) = parse("contaminated", &contaminated);
    assert_ne!(interface_keys(&resolve, package_id), [PACK, HOST]);
    assert!(!allowed_vocabulary(&contaminated, DENIED_VOCABULARY));
}

fn assert_contract(resolve: &Resolve, package_id: PackageId) {
    let (pack_id, host_id) = assert_world_topology(
        resolve,
        package_id,
        PACKAGE,
        FAMILY,
        &[PACK, HOST],
        HOST,
        PACK,
    );

    let pack = &resolve.interfaces[pack_id];
    let host = &resolve.interfaces[host_id];
    assert_eq!(pack.functions.len(), 2);
    assert_eq!(host.functions.len(), 2);
    assert_eq!(type_inventory(resolve, pack), PACK_INVENTORY);
    assert_eq!(type_inventory(resolve, host), HOST_INVENTORY);
    assert_invoke_and_pull(
        resolve,
        pack,
        "workspace-operation",
        "workspace-request",
        "workspace-error",
        "workspace-pull",
    );
    assert_freestanding_function(
        resolve,
        host,
        "scan",
        "request",
        "scan-request",
        "scan-page",
        "workspace-host-error",
    );
    assert_freestanding_function(
        resolve,
        host,
        "apply-rollback",
        "request",
        "rollback-request",
        "rollback-output",
        "workspace-host-error",
    );
    assert!(allowed_vocabulary(SOURCE, DENIED_VOCABULARY));
    assert_denied_types(resolve);
}

// Rust guideline compliant 2026-08-29.
