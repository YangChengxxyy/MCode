//! Parse-based sole-current Compaction FeaturePack contract tests.

mod support;

use support::{
    allowed_vocabulary, assert_denied_types, assert_freestanding_function, assert_invoke_and_pull,
    assert_json, assert_lf, assert_rule_inventory, assert_semantic_sha256, assert_world_topology,
    interface_keys, package_interface, parse, semantic_rules, type_inventory,
};
use wit_parser::{PackageId, Resolve};

const SOURCE: &str = include_str!("../wit/feature-pack/compaction.wit");
const GOLDEN: &str = include_str!("../goldens/feature_compaction_current.wit");
const SEMANTICS: &str = include_str!("../goldens/feature_compaction_current.jsonl");
const SEMANTICS_SHA256: &str = "c72ab0b21a96fbabb8aadb673a76734962032eba06c1aff952e35dc6fcfbe6c0";
const PACKAGE: &str = "mcode:feature-pack@0.0.1";
const FAMILY: &str = "compaction";
const PACK: &str = "compaction-pack";
const HOST: &str = "compaction-host";
const DENIED_VOCABULARY: &[&str] = &[
    "value",
    "metadata",
    "pack-operation",
    "wasi:",
    "map<",
    "future<",
    "stream<",
    "f32",
    "f64",
    "error-context",
    "web-pack",
    "todo-pack",
    "workspace-pack",
];
const INVENTORY: &str = r#"summarizing-progress=record(completed:u16,total:u16)
compaction-progress=variant(assessing,validating,summarizing:summarizing-progress)
assessment-result=record(needed:bool,target-tokens:u32)
summary-result=record(text:string,covered-through:string,input-tokens:option<u64>,output-tokens:option<u64>)
compaction-result=variant(assessment:assessment-result,summary:summary-result)
invalid-terminal-reason=enum(length,error,cancel,tool-call)
compaction-error=variant(invalid-argument,stale-head,provider-unavailable,invalid-terminal:invalid-terminal-reason,limit,cancelled,unavailable)
compaction-pull=variant(pending,progress:compaction-progress,complete:compaction-result,failed:compaction-error)
item-kind=enum(system,user,assistant,tool-call,tool-result)
summary-item=record(event-id:string,kind:item-kind,text:string,call-id:option<string>)
head-stamp=variant(empty,event:string)
assess-request=record(session-id:string,branch-id:string,head:head-stamp,input-tokens:u64,context-limit:u64,reserve-output:u64)
summarize-request=record(session-id:string,branch-id:string,head:head-stamp,items:list<summary-item>,target-tokens:u32)
compaction-request=variant(assess:assess-request,summarize:summarize-request)
summary-request=record(session-id:string,branch-id:string,head:head-stamp,items:list<summary-item>,target-tokens:u32)
summary-output=record(text:string,covered-through:string,input-tokens:option<u64>,output-tokens:option<u64>)
compaction-host-error=variant(stale-head,provider-unavailable,invalid-terminal:invalid-terminal-reason,limit,cancelled,unavailable)
compaction-operation=resource
"#;
const HOST_INVENTORY: &str = r#"summary-output=record(text:string,covered-through:string,input-tokens:option<u64>,output-tokens:option<u64>)
invalid-terminal-reason=enum(length,error,cancel,tool-call)
compaction-host-error=variant(stale-head,provider-unavailable,invalid-terminal:invalid-terminal-reason,limit,cancelled,unavailable)
item-kind=enum(system,user,assistant,tool-call,tool-result)
summary-item=record(event-id:string,kind:item-kind,text:string,call-id:option<string>)
head-stamp=variant(empty,event:string)
summary-request=record(session-id:string,branch-id:string,head:head-stamp,items:list<summary-item>,target-tokens:u32)
"#;
const RULES: [&str; 14] = [
    "topology",
    "operation-authority",
    "table-authority",
    "scalar-grammar",
    "safe-text-and-charge",
    "assessment-reducer",
    "summarize-input",
    "tool-pairing",
    "progress-reducer",
    "import-cardinality",
    "summary-output",
    "stable-errors",
    "boundary-coverage",
    "stage-scope",
];

#[test]
fn artifacts_are_identical_lf_and_parse_to_the_exact_contract() {
    assert_eq!(SOURCE.as_bytes(), GOLDEN.as_bytes());
    for (path, source) in [("compaction.wit", SOURCE), ("golden", GOLDEN)] {
        assert_lf(path, source);
        let (resolve, package_id) = parse(path, source);
        assert_contract(&resolve, package_id);
    }
}

#[test]
fn semantics_have_exact_rules_and_critical_values() {
    assert_lf("feature_compaction_current.jsonl", SEMANTICS);
    assert_semantic_sha256(
        "feature_compaction_current.jsonl",
        SEMANTICS,
        SEMANTICS_SHA256,
        (r#""notNeededTarget":0"#, r#""notNeededTarget":1"#),
    );
    let rules = semantic_rules(SEMANTICS);
    assert_rule_inventory(&rules, RULES);
    assert_json(&rules, "scalar-grammar", "/eventIdBytes", "37");
    assert_json(
        &rules,
        "safe-text-and-charge",
        "/requestChargeMax",
        "8388608",
    );
    assert_json(
        &rules,
        "assessment-reducer",
        "/neededTarget",
        r#""min(1048576, context-limit - reserve-output)""#,
    );
    assert_json(&rules, "summarize-input", "/items/max", "2048");
    assert_json(&rules, "import-cardinality", "/summarize/summarize", "1");
    assert_json(
        &rules,
        "summary-output",
        "/terminal",
        r#""exact typed structural copy of accepted summary-output""#,
    );
    assert_json(&rules, "boundary-coverage", "/dimensions/3", r#""N+1""#);
}

#[test]
fn mutations_detect_type_erasure_and_cross_family_contamination() {
    let erased = SOURCE.replacen("head: head-stamp", "head: string", 1);
    let (resolve, package_id) = parse("erased", &erased);
    let pack = package_interface(&resolve, package_id, PACK);
    assert_ne!(type_inventory(&resolve, pack), INVENTORY);

    let contaminated = SOURCE.replacen(
        "world compaction",
        "interface web-pack {}\n\nworld compaction",
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
    assert_eq!(host.functions.len(), 1);
    assert_eq!(type_inventory(resolve, pack), INVENTORY);
    assert_eq!(type_inventory(resolve, host), HOST_INVENTORY);
    assert_invoke_and_pull(
        resolve,
        pack,
        "compaction-operation",
        "compaction-request",
        "compaction-error",
        "compaction-pull",
    );
    assert_freestanding_function(
        resolve,
        host,
        "summarize",
        "request",
        "summary-request",
        "summary-output",
        "compaction-host-error",
    );
    assert!(allowed_vocabulary(SOURCE, DENIED_VOCABULARY));
    assert_denied_types(resolve);
}

// Rust guideline compliant 2026-08-29.
