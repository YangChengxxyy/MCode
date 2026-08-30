//! Parse-based sole-current Resources FeaturePack WIT contract tests.

mod support;

use support::{
    allowed_vocabulary, assert_denied_names, assert_denied_types, assert_invoke_and_pull,
    assert_json, assert_lf, assert_rule_inventory, assert_semantic_sha256,
    assert_zero_import_world_topology, package_interface, parse, semantic_rules,
    type_inventory_in_order,
};
use wit_parser::{PackageId, Resolve, TypeDefKind};

const SOURCE: &str = include_str!("../wit/feature-pack/resources.wit");
const GOLDEN: &str = include_str!("../goldens/feature_resources_current.wit");
const SEMANTICS: &str = include_str!("../goldens/feature_resources_current.jsonl");
const SEMANTICS_SHA256: &str = "78d11c718249a1e8387ee4d87ca011bbaeadba17e85a92d1ed64e961a3509bf4";
const PACKAGE: &str = "mcode:feature-pack@0.0.1";
const WORLD: &str = "resources";
const PACK: &str = "resources-pack";
const TYPE_INVENTORY: &str = r#"resources-request=variant(catalog:catalog-request,read:read-request,render-prompt:render-prompt-request,contributions:contributions-request)
catalog-request=record(offset:u32,limit:u16)
read-request=record(id:string,offset:u64,max-bytes:u32)
render-prompt-request=record(id:string,args:list<prompt-arg>)
contributions-request=record()
resources-progress=enum(loading,rendering)
resources-pull=variant(pending,progress:resources-progress,complete:resources-result,failed:resources-error)
resources-result=variant(catalog:catalog-result,read:read-result,prompt:prompt-result,contributions:contributions-result)
catalog-result=record(items:list<catalog-entry>,next-offset:option<u32>)
catalog-entry=variant(resource:resource-entry,prompt:prompt-entry)
resource-entry=record(id:string,title:string,media:resource-media,size-hint:option<u64>)
resource-media=enum(text,markdown)
prompt-entry=record(id:string,title:string,params:list<prompt-param>)
prompt-param=record(name:string,label:string,required:bool)
prompt-arg=record(name:string,value:string)
read-result=record(text:string,next-offset:option<u64>)
prompt-result=record(id:string,messages:list<prompt-message>)
prompt-message=record(role:message-role,text:string)
message-role=enum(system,user,assistant)
contributions-result=record(items:list<contribution>)
contribution=record(id:string,kind:contribution-kind)
contribution-kind=enum(status,panel)
resources-error=enum(invalid-argument,not-found,limit,unavailable,cancelled)
resources-operation=resource
"#;

#[test]
fn artifacts_are_identical_lf_and_parse_to_the_exact_contract() {
    assert_eq!(SOURCE.as_bytes(), GOLDEN.as_bytes());
    for (path, source) in [
        ("resources.wit", SOURCE),
        ("feature_resources_current.wit", GOLDEN),
    ] {
        assert_lf(path, source);
        let (resolve, package_id) = parse(path, source);
        assert_contract(&resolve, package_id);
    }
}

#[test]
fn semantic_golden_is_lf_unique_and_locks_resources_authority() {
    assert_lf("feature_resources_current.jsonl", SEMANTICS);
    assert_semantic_sha256(
        "feature_resources_current.jsonl",
        SEMANTICS,
        SEMANTICS_SHA256,
        (
            r#""plainProgressAtMostOnce":true"#,
            r#""plainProgressAtMostOnce":false"#,
        ),
    );
    let rules = semantic_rules(SEMANTICS);
    let expected = "artifact-stage catalog-entry-bounds catalog-identity catalog-pagination contributions deadline-lifecycle error-mapping logical-charge operation-authority operation-resource prompt-arguments prompt-result read-utf8 reducer-matrix stage-ownership text-safety topology zero-import"
        .split_ascii_whitespace();
    assert_rule_inventory(&rules, expected);
    assert_json(&rules, "zero-import", "/hostMethods", "0");
    assert_json(&rules, "catalog-entry-bounds", "/total/max", "8192");
    assert_json(&rules, "catalog-pagination", "/requestLimit/max", "128");
    assert_json(&rules, "read-utf8", "/maxBytes/min", "4");
    assert_json(&rules, "read-utf8", "/maxBytes/max", "65536");
    assert_json(&rules, "prompt-result", "/messages/max", "16");
    assert_json(&rules, "artifact-stage", "/runtimeBinaryPreflight", "false");
}

#[test]
fn inventory_guards_named_types_escaped_resource_and_family_isolation() {
    let (resolve, package_id) = parse("escaped resource", SOURCE);
    let interface = package_interface(&resolve, package_id, PACK);
    let entry_id = *interface.types.get("catalog-entry").expect("catalog-entry");
    let TypeDefKind::Variant(entry) = &resolve.types[entry_id].kind else {
        panic!("catalog-entry variant")
    };
    assert_eq!(
        entry.cases[0].name, "resource",
        "escaped case keeps its semantic name"
    );

    for (label, mutated) in [
        (
            "named catalog erasure",
            SOURCE.replacen("items: list<catalog-entry>", "items: list<string>", 1),
        ),
        (
            "cross-family contamination",
            SOURCE.replacen(
                "    resource resources-operation",
                "    type ask-contamination = string;\n\n    resource resources-operation",
                1,
            ),
        ),
    ] {
        let (resolve, package_id) = parse(label, &mutated);
        assert_ne!(
            type_inventory_in_order(
                &resolve,
                package_interface(&resolve, package_id, PACK),
                TYPE_INVENTORY,
            ),
            TYPE_INVENTORY,
            "{label} must fail the frozen inventory"
        );
    }
}

fn assert_contract(resolve: &Resolve, package_id: PackageId) {
    let pack_id = assert_zero_import_world_topology(resolve, package_id, PACKAGE, WORLD, PACK);
    let pack = &resolve.interfaces[pack_id];
    assert_eq!(
        type_inventory_in_order(resolve, pack, TYPE_INVENTORY),
        TYPE_INVENTORY
    );
    assert_eq!(pack.functions.len(), 2);
    assert_invoke_and_pull(
        resolve,
        pack,
        "resources-operation",
        "resources-request",
        "resources-error",
        "resources-pull",
    );
    assert_denied_names(
        resolve,
        package_id,
        &["value", "map", "metadata", "pack-operation"],
        &[
            "session-",
            "compaction-",
            "ask-",
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
            "ask-",
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
