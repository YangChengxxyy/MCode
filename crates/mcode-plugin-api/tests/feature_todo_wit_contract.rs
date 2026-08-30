//! Parse-based sole-current Todo FeaturePack contract tests.

mod support;

use support::{
    allowed_vocabulary, assert_denied_types, assert_freestanding_function, assert_invoke_and_pull,
    assert_json, assert_lf, assert_rule_inventory, assert_semantic_sha256, assert_world_topology,
    interface_keys, package_interface, parse, semantic_rules, type_inventory,
};
use wit_parser::{Interface, PackageId, Resolve, TypeDefKind};

const SOURCE: &str = include_str!("../wit/feature-pack/todo.wit");
const GOLDEN: &str = include_str!("../goldens/feature_todo_current.wit");
const SEMANTICS: &str = include_str!("../goldens/feature_todo_current.jsonl");
const SEMANTICS_SHA256: &str = "838d81d91a7a27327aec311e8d465bc7259c972a2fc4649e25f7c3016e124596";
const PACKAGE: &str = "mcode:feature-pack@0.0.1";
const FAMILY: &str = "todo";
const PACK: &str = "todo-pack";
const HOST: &str = "todo-host";
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
    "workspace-pack",
    "web-pack",
    "mcp-pack",
];
const PACK_INVENTORY: &str = r#"get-request=record(todo-id:string)
todo-progress=enum(loading,persisting)
task-status=enum(pending,in-progress,completed,deleted)
task-revision=alias(u64)
task=record(todo-id:string,revision:task-revision,status:task-status,subject:string,description:string,active-form:option<string>,blocked-by:list<string>,owner:option<string>)
listed-result=record(items:list<task>,next:option<string>)
todo-result=variant(created:task,current:task,listed:listed-result,updated:task,deleted:task)
snapshot-revision=alias(u64)
list-request=record(snapshot:snapshot-revision,status:option<task-status>,after:option<string>,limit:u16)
create-task-read=record(todo-id:string)
get-task-read=record(todo-id:string)
list-task-read=record(snapshot:snapshot-revision,status:option<task-status>,after:option<string>,limit:u16)
task-read=variant(create:create-task-read,get:get-task-read,list:list-task-read)
listed-task-page=record(items:list<task>,next:option<string>)
task-page=variant(absent,current:task,listed:listed-task-page)
create-mutation=record(todo-id:string,subject:string,description:string,blocked-by:list<string>,owner:option<string>)
set-status-mutation=record(todo-id:string,expected-revision:task-revision,status:task-status,active-form:option<string>)
set-subject-mutation=record(todo-id:string,expected-revision:task-revision,subject:string)
set-description-mutation=record(todo-id:string,expected-revision:task-revision,description:string)
replace-dependencies-mutation=record(todo-id:string,expected-revision:task-revision,blocked-by:list<string>)
set-owner-mutation=record(todo-id:string,expected-revision:task-revision,owner:option<string>)
delete-mutation=record(todo-id:string,expected-revision:task-revision)
todo-mutation=variant(create:create-mutation,set-status:set-status-mutation,set-subject:set-subject-mutation,set-description:set-description-mutation,replace-dependencies:replace-dependencies-mutation,set-owner:set-owner-mutation,delete:delete-mutation)
event-reservation-view=record(reservation-id:string,mutation-digest:string,expected-revision:option<task-revision>)
create-request=record(todo-id:string,subject:string,description:string,blocked-by:list<string>,owner:option<string>,reservation:event-reservation-view)
set-status-request=record(todo-id:string,expected-revision:task-revision,status:task-status,active-form:option<string>,reservation:event-reservation-view)
set-subject-request=record(todo-id:string,expected-revision:task-revision,subject:string,reservation:event-reservation-view)
set-description-request=record(todo-id:string,expected-revision:task-revision,description:string,reservation:event-reservation-view)
replace-dependencies-request=record(todo-id:string,expected-revision:task-revision,blocked-by:list<string>,reservation:event-reservation-view)
set-owner-request=record(todo-id:string,expected-revision:task-revision,owner:option<string>,reservation:event-reservation-view)
delete-request=record(todo-id:string,expected-revision:task-revision,reservation:event-reservation-view)
todo-request=variant(create:create-request,get:get-request,list:list-request,set-status:set-status-request,set-subject:set-subject-request,set-description:set-description-request,replace-dependencies:replace-dependencies-request,set-owner:set-owner-request,delete:delete-request)
task-mutation=record(mutation:todo-mutation,reservation:event-reservation-view)
task-commit=record(task:task)
revision-conflict-result=record(actual:task-revision)
todo-error=variant(invalid-argument,already-exists,not-found,revision-conflict:revision-conflict-result,invalid-transition,dependency-cycle,limit,unavailable,cancelled)
todo-pull=variant(pending,progress:todo-progress,complete:todo-result,failed:todo-error)
todo-host-error=variant(already-exists,not-found,revision-conflict:revision-conflict-result,invalid-transition,dependency-cycle,limit,unavailable)
todo-operation=resource
"#;
const HOST_INVENTORY: &str = r#"create-task-read=record(todo-id:string)
get-task-read=record(todo-id:string)
create-mutation=record(todo-id:string,subject:string,description:string,blocked-by:list<string>,owner:option<string>)
task-status=enum(pending,in-progress,completed,deleted)
task-revision=alias(u64)
set-status-mutation=record(todo-id:string,expected-revision:task-revision,status:task-status,active-form:option<string>)
set-subject-mutation=record(todo-id:string,expected-revision:task-revision,subject:string)
set-description-mutation=record(todo-id:string,expected-revision:task-revision,description:string)
replace-dependencies-mutation=record(todo-id:string,expected-revision:task-revision,blocked-by:list<string>)
set-owner-mutation=record(todo-id:string,expected-revision:task-revision,owner:option<string>)
delete-mutation=record(todo-id:string,expected-revision:task-revision)
todo-mutation=variant(create:create-mutation,set-status:set-status-mutation,set-subject:set-subject-mutation,set-description:set-description-mutation,replace-dependencies:replace-dependencies-mutation,set-owner:set-owner-mutation,delete:delete-mutation)
event-reservation-view=record(reservation-id:string,mutation-digest:string,expected-revision:option<task-revision>)
task-mutation=record(mutation:todo-mutation,reservation:event-reservation-view)
revision-conflict-result=record(actual:task-revision)
todo-host-error=variant(already-exists,not-found,revision-conflict:revision-conflict-result,invalid-transition,dependency-cycle,limit,unavailable)
task=record(todo-id:string,revision:task-revision,status:task-status,subject:string,description:string,active-form:option<string>,blocked-by:list<string>,owner:option<string>)
listed-task-page=record(items:list<task>,next:option<string>)
task-page=variant(absent,current:task,listed:listed-task-page)
task-commit=record(task:task)
snapshot-revision=alias(u64)
list-task-read=record(snapshot:snapshot-revision,status:option<task-status>,after:option<string>,limit:u16)
task-read=variant(create:create-task-read,get:get-task-read,list:list-task-read)
"#;
const RULES: [&str; 17] = [
    "topology",
    "operation-authority",
    "table-authority",
    "revision-scopes",
    "task-bounds",
    "task-invariants",
    "create-reducer",
    "status-matrix",
    "field-mutations",
    "delete-reducer",
    "read-routing",
    "pagination",
    "mutation-digest",
    "operation-reducer",
    "stable-errors",
    "boundary-coverage",
    "stage-scope",
];

#[test]
fn artifacts_are_identical_lf_and_parse_to_the_exact_contract() {
    assert_eq!(SOURCE.as_bytes(), GOLDEN.as_bytes());
    for (path, source) in [("todo.wit", SOURCE), ("golden", GOLDEN)] {
        assert_lf(path, source);
        let (resolve, package_id) = parse(path, source);
        assert_contract(&resolve, package_id);
    }
}

#[test]
fn semantics_have_exact_rules_and_critical_values() {
    assert_lf("feature_todo_current.jsonl", SEMANTICS);
    assert_semantic_sha256(
        "feature_todo_current.jsonl",
        SEMANTICS,
        SEMANTICS_SHA256,
        (
            r#""pending":{"pending":"current/noop""#,
            r#""pending":{"pending":"updated/no-active-form""#,
        ),
    );
    let rules = semantic_rules(SEMANTICS);
    assert_rule_inventory(&rules, RULES);
    assert_json(
        &rules,
        "table-authority",
        "/todoId/grammar",
        r#""todo1-[0-9a-f]{32}""#,
    );
    assert_json(
        &rules,
        "revision-scopes",
        "/taskRevision/max",
        "9223372036854775807",
    );
    assert_json(&rules, "task-bounds", "/blockedBy/max", "64");
    assert_json(&rules, "create-reducer", "/alreadyExists/commitCount", "0");
    assert_json(
        &rules,
        "status-matrix",
        "/completed/completed",
        r#""invalid-transition""#,
    );
    assert_json(
        &rules,
        "delete-reducer",
        "/denied/reservationConsumed",
        "false",
    );
    assert_json(&rules, "pagination", "/limit/max", "256");
    assert_json(
        &rules,
        "mutation-digest",
        "/domainAscii",
        r#""mcode-todo-mutation-v1\u0000""#,
    );
    assert_json(&rules, "boundary-coverage", "/dimensions/3", r#""N+1""#);
}

#[test]
fn mutations_detect_type_erasure_and_cross_family_contamination() {
    let erased = SOURCE.replacen("revision: task-revision", "revision: u64", 1);
    let (resolve, package_id) = parse("erased", &erased);
    assert_ne!(
        type_inventory(&resolve, package_interface(&resolve, package_id, PACK)),
        PACK_INVENTORY
    );

    let contaminated =
        SOURCE.replacen("world todo", "interface workspace-pack {}\n\nworld todo", 1);
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
        "todo-operation",
        "todo-request",
        "todo-error",
        "todo-pull",
    );
    assert_freestanding_function(
        resolve,
        host,
        "load-tasks",
        "request",
        "task-read",
        "task-page",
        "todo-host-error",
    );
    assert_freestanding_function(
        resolve,
        host,
        "commit-task-event",
        "mutation",
        "task-mutation",
        "task-commit",
        "todo-host-error",
    );
    assert_semantic_list_case(resolve, pack, "todo-request");
    assert_semantic_list_case(resolve, pack, "task-read");
    assert_semantic_list_case(resolve, host, "task-read");
    assert!(allowed_vocabulary(SOURCE, DENIED_VOCABULARY));
    assert_denied_types(resolve);
}

fn assert_semantic_list_case(resolve: &Resolve, interface: &Interface, name: &str) {
    let TypeDefKind::Variant(value) = &resolve.types[interface.types[name]].kind else {
        panic!("variant required")
    };
    assert!(value.cases.iter().any(|case| case.name == "list"));
}

// Rust guideline compliant 2026-08-29.
