//! Parse-based sole-current Ask FeaturePack WIT contract tests.

mod support;

use support::{
    allowed_vocabulary, assert_denied_names, assert_denied_types, assert_freestanding_function,
    assert_invoke_and_pull, assert_json, assert_lf, assert_named_aliases, assert_rule_inventory,
    assert_semantic_sha256, assert_world_topology, package_interface, parse, semantic_rules,
    type_inventory_with_resource,
};
use wit_parser::{PackageId, Resolve};

const SOURCE: &str = include_str!("../wit/feature-pack/ask.wit");
const GOLDEN: &str = include_str!("../goldens/feature_ask_current.wit");
const SEMANTICS: &str = include_str!("../goldens/feature_ask_current.jsonl");
const SEMANTICS_SHA256: &str = "4d015ed4ac93b8b041e953918aa961fa7cc96f8805a2331b5d6e9d9cc3d6236f";
const PACKAGE: &str = "mcode:feature-pack@0.0.1";
const WORLD: &str = "ask";
const PACK: &str = "ask-pack";
const HOST: &str = "ask-host";
const TYPE_INVENTORY: &str = r#"ask-request=variant(present:present-request)
present-request=record(title:option<string>,questions:list<question>)
question=record(id:string,header:string,question:string,kind:question-kind)
question-kind=variant(confirm,text:text-params,single-choice:choice-params,multi-choice:choice-params)
text-params=record(max-bytes:u16,multiline:bool)
choice-params=record(choices:list<choice>)
choice=record(id:string,label:string,description:string,preview:option<string>)
ask-progress=variant(waiting:waiting-progress)
ask-pull=variant(pending,progress:ask-progress,complete:ask-result,failed:ask-error)
waiting-progress=record(index:u8,total:u8)
ask-result=variant(answered:answers,abandoned)
answers=record(items:list<answer>)
answer=record(question-id:string,value:answer-value)
answer-value=variant(confirmed:bool,text:string,choice:string,choices:list<string>)
ask-error=enum(invalid-argument,invalid-answer,interaction-unavailable,limit,cancelled)
interaction-request=record(title:option<string>,questions:list<question>)
interaction-output=variant(answered:answers,abandoned)
ask-host-error=enum(invalid-answer,interaction-unavailable,limit,cancelled)
ask-operation=resource
"#;

#[test]
fn artifacts_are_identical_lf_and_parse_to_the_exact_contract() {
    assert_eq!(SOURCE.as_bytes(), GOLDEN.as_bytes());
    for (path, source) in [("ask.wit", SOURCE), ("feature_ask_current.wit", GOLDEN)] {
        assert_lf(path, source);
        let (resolve, package_id) = parse(path, source);
        assert_contract(&resolve, package_id);
    }
}

#[test]
fn semantic_golden_is_lf_unique_and_locks_ask_authority() {
    assert_lf("feature_ask_current.jsonl", SEMANTICS);
    assert_semantic_sha256(
        "feature_ask_current.jsonl",
        SEMANTICS,
        SEMANTICS_SHA256,
        (r#""firstIndex":0"#, r#""firstIndex":1"#),
    );
    let rules = semantic_rules(SEMANTICS);
    let expected = "answers artifact-stage choice-bounds deadline-lifecycle error-mapping host-import interaction-authority logical-charge operation-authority operation-resource present-bounds present-reducer question-kinds stage-ownership text-safety topology waiting-reducer"
        .split_ascii_whitespace();
    assert_rule_inventory(&rules, expected);
    assert_json(&rules, "present-bounds", "/questions/min", "1");
    assert_json(&rules, "present-bounds", "/questions/max", "4");
    assert_json(&rules, "question-kinds", "/text/maxBytes/max", "8192");
    assert_json(&rules, "choice-bounds", "/choices/min", "2");
    assert_json(&rules, "choice-bounds", "/preview/some", r#""Safe(16384)""#);
    assert_json(&rules, "host-import", "/cardinality", "1");
    assert_json(&rules, "artifact-stage", "/runtimeBinaryPreflight", "false");
}

#[test]
fn inventory_guards_named_types_and_family_isolation() {
    for (label, mutated) in [
        (
            "named kind erasure",
            SOURCE.replacen("kind: question-kind", "kind: string", 1),
        ),
        (
            "cross-family contamination",
            SOURCE.replacen(
                "    present: func",
                "    type todo-contamination = string;\n\n    present: func",
                1,
            ),
        ),
    ] {
        let (resolve, package_id) = parse(label, &mutated);
        assert_ne!(
            contract_inventory(&resolve, package_id),
            TYPE_INVENTORY,
            "{label} must fail the frozen inventory"
        );
    }
}

fn contract_inventory(resolve: &Resolve, package_id: PackageId) -> String {
    let host = package_interface(resolve, package_id, HOST);
    let pack = package_interface(resolve, package_id, PACK);
    type_inventory_with_resource(resolve, host, pack, "ask-operation", TYPE_INVENTORY)
}

fn assert_contract(resolve: &Resolve, package_id: PackageId) {
    let (pack_id, host_id) = assert_world_topology(
        resolve,
        package_id,
        PACKAGE,
        WORLD,
        &[HOST, PACK],
        HOST,
        PACK,
    );
    let host = &resolve.interfaces[host_id];
    let pack = &resolve.interfaces[pack_id];
    assert_eq!(contract_inventory(resolve, package_id), TYPE_INVENTORY);
    assert_eq!(pack.functions.len(), 2);
    assert_eq!(host.functions.len(), 1);
    assert_invoke_and_pull(
        resolve,
        pack,
        "ask-operation",
        "ask-request",
        "ask-error",
        "ask-pull",
    );
    assert_eq!(
        pack.types.keys().map(String::as_str).collect::<Vec<_>>(),
        ["ask-request", "ask-pull", "ask-error", "ask-operation"]
    );
    assert_named_aliases(
        resolve,
        pack,
        host,
        &["ask-request", "ask-pull", "ask-error"],
    );
    assert_freestanding_function(
        resolve,
        host,
        "present",
        "request",
        "interaction-request",
        "interaction-output",
        "ask-host-error",
    );
    assert_denied_names(
        resolve,
        package_id,
        &["value", "map", "metadata", "pack-operation"],
        &[
            "session-",
            "compaction-",
            "resources-",
            "todo-",
            "web-",
            "mcp-",
            "usage-",
            "subagents-",
            "workspace-",
            "ui-",
        ],
    );
    assert!(allowed_vocabulary(
        SOURCE,
        &[
            "wasi:",
            "future<",
            "stream<",
            "map<",
            "session-",
            "compaction-",
            "resources-",
            "todo-",
            "web-",
            "mcp-",
            "usage-",
            "subagents-",
            "workspace-",
            "ui-",
        ]
    ));
    assert_denied_types(resolve);
}

// Rust guideline compliant 2026-08-30.
