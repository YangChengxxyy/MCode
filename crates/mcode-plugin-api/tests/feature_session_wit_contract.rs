//! Parse-based sole-current Session FeaturePack WIT contract tests.

mod support;

use support::{
    allowed_vocabulary, assert_denied_names, assert_denied_types, assert_freestanding_function,
    assert_invoke_and_pull, assert_json, assert_lf, assert_named_aliases, assert_rule_inventory,
    assert_semantic_sha256, assert_world_topology, package_interface, parse, semantic_rules,
    type_inventory_with_resource,
};
use wit_parser::{PackageId, Resolve};

const SOURCE: &str = include_str!("../wit/feature-pack/session.wit");
const GOLDEN: &str = include_str!("../goldens/feature_session_current.wit");
const SEMANTICS: &str = include_str!("../goldens/feature_session_current.jsonl");
const SEMANTICS_SHA256: &str = "372ea60241293e3240e359ae37bc5865f3975cb440e65bc2c473169f88148e13";
const PACKAGE: &str = "mcode:feature-pack@0.0.1";
const WORLD: &str = "session";
const PACK: &str = "session-pack";
const HOST: &str = "session-host";
const TYPE_INVENTORY: &str = r#"session-request=variant(create:create-request,open:open-request,append:append-request,read:read-request,fork:fork-request,rewind:rewind-request)
create-request=record(session-id:string,root-branch-id:string)
open-request=record(session-id:string)
append-request=record(session-id:string,branch-id:string,expected-head:head-stamp,reservation:event-reservation-view)
read-request=record(session-id:string,branch-id:string,snapshot-head:head-stamp,after:option<string>,limit:u16)
fork-request=record(session-id:string,from-branch-id:string,at-event-id:string,new-branch-id:string,reservation:branch-reservation-view)
rewind-request=record(session-id:string,branch-id:string,to-event-id:string,new-branch-id:string,reservation:branch-reservation-view)
session-progress=enum(recovering,replaying,committing)
session-pull=variant(pending,progress:session-progress,complete:session-result,failed:session-error)
session-result=variant(created:created-result,opened:opened-result,appended:appended-result,events:events-result,branched:branched-result)
session-error=variant(invalid-argument,not-found,conflict:conflict-result,corrupt,limit,cancelled,unavailable)
created-result=record(session-id:string,branch-id:string,head:head-stamp)
opened-result=record(heads:list<branch-head>)
appended-result=record(head:head-stamp)
events-result=record(items:list<session-event>,next:option<string>)
branched-result=record(branch-id:string,head:head-stamp)
branch-head=record(branch-id:string,head:head-stamp)
session-event=record(event-id:string,digest:string,bytes:u64,kind:event-kind,call-id:option<string>)
event-kind=enum(message,tool-call,tool-result,usage)
head-stamp=variant(empty,event:string)
event-reservation-view=record(event-id:string,payload-digest:string,branch-id:string,expected-head:head-stamp)
branch-mutation-kind=enum(fork,rewind)
branch-reservation-view=record(reservation-id:string,kind:branch-mutation-kind,source-branch-id:string,source-head:head-stamp,target-event-id:string,new-branch-id:string,mutation-digest:string)
ledger-read=variant(open:open-ledger-read,append:append-ledger-read,events:events-ledger-read,fork:fork-ledger-read,rewind:rewind-ledger-read)
open-ledger-read=record(session-id:string)
append-ledger-read=record(session-id:string,branch-id:string,expected-head:head-stamp,reservation:event-reservation-view)
events-ledger-read=record(session-id:string,branch-id:string,snapshot-head:head-stamp,after:option<string>,limit:u16)
fork-ledger-read=record(session-id:string,from-branch-id:string,at-event-id:string,new-branch-id:string,reservation:branch-reservation-view)
rewind-ledger-read=record(session-id:string,branch-id:string,to-event-id:string,new-branch-id:string,reservation:branch-reservation-view)
ledger-page=variant(opened:opened-ledger-page,append:append-ledger-view,events:events-ledger-page,fork:fork-ledger-view,rewind:rewind-ledger-view)
opened-ledger-page=record(heads:list<branch-head>)
append-ledger-view=record(branch-id:string,actual-head:head-stamp,reservation:event-reservation-view)
events-ledger-page=record(items:list<session-event>,next:option<string>)
fork-ledger-view=record(from-branch-id:string,source-head:head-stamp,at-event-id:string,new-branch-id:string,reservation:branch-reservation-view)
rewind-ledger-view=record(branch-id:string,source-head:head-stamp,to-event-id:string,new-branch-id:string,reservation:branch-reservation-view)
ledger-mutation=variant(create:create-ledger-mutation,append:append-ledger-mutation,fork:fork-ledger-mutation,rewind:rewind-ledger-mutation)
create-ledger-mutation=record(session-id:string,root-branch-id:string)
append-ledger-mutation=record(session-id:string,branch-id:string,expected-head:head-stamp,reservation:event-reservation-view)
fork-ledger-mutation=record(session-id:string,from-branch-id:string,at-event-id:string,new-branch-id:string,reservation:branch-reservation-view)
rewind-ledger-mutation=record(session-id:string,branch-id:string,to-event-id:string,new-branch-id:string,reservation:branch-reservation-view)
ledger-commit=variant(created:created-ledger-commit,appended:appended-ledger-commit,branched:branched-ledger-commit)
created-ledger-commit=record(session-id:string,branch-id:string,head:head-stamp)
appended-ledger-commit=record(head:head-stamp)
branched-ledger-commit=record(branch-id:string,head:head-stamp)
session-host-error=variant(not-found,conflict:conflict-result,corrupt,limit,unavailable)
conflict-result=record(actual:head-stamp)
session-operation=resource
"#;

#[test]
fn artifacts_are_identical_lf_and_parse_to_the_exact_contract() {
    assert_eq!(SOURCE.as_bytes(), GOLDEN.as_bytes());
    for (path, source) in [
        ("session.wit", SOURCE),
        ("feature_session_current.wit", GOLDEN),
    ] {
        assert_lf(path, source);
        let (resolve, package_id) = parse(path, source);
        assert_contract(&resolve, package_id);
    }
}

#[test]
fn semantic_golden_is_lf_unique_and_locks_session_authority() {
    assert_lf("feature_session_current.jsonl", SEMANTICS);
    assert_semantic_sha256(
        "feature_session_current.jsonl",
        SEMANTICS,
        SEMANTICS_SHA256,
        (r#""postTerminalPulls":0"#, r#""postTerminalPulls":1"#),
    );
    let rules = semantic_rules(SEMANTICS);
    let expected = "append-reducer artifact-stage branch-mutation-digest create-reducer deadline-lifecycle dto-bounds error-mapping event-validity fork-rewind-reducer host-imports identities ledger-reads logical-charge open-read-reducer operation-authority operation-resource pagination stage-ownership table-binding text-safety topology"
        .split_ascii_whitespace();
    assert_rule_inventory(&rules, expected);
    assert_json(&rules, "dto-bounds", "/readLimit/max", "256");
    assert_json(&rules, "dto-bounds", "/eventPayloadBytes/max", "8388608");
    assert_json(
        &rules,
        "identities",
        "/hostIssued/branch-reservation-id",
        r#""sbr1-[0-9a-f]{32}""#,
    );
    assert_json(
        &rules,
        "branch-mutation-digest",
        "/domainAscii",
        r#""mcode-session-branch-mutation-v1\u0000""#,
    );
    assert_json(
        &rules,
        "append-reducer",
        "/imports/1",
        r#""commit-ledger(append) x1""#,
    );
    assert_json(
        &rules,
        "fork-rewind-reducer",
        "/fork/acceptedCommit",
        r#"{"case":"branched","branchId":"exact new-branch-id","head":"exact event(at-event-id)"}"#,
    );
    assert_json(
        &rules,
        "fork-rewind-reducer",
        "/fork/terminal",
        r#"{"case":"branched","copy":"exact accepted commit"}"#,
    );
    assert_json(
        &rules,
        "fork-rewind-reducer",
        "/rewind/acceptedCommit",
        r#"{"case":"branched","branchId":"exact new-branch-id","head":"exact event(to-event-id)"}"#,
    );
    assert_json(
        &rules,
        "fork-rewind-reducer",
        "/rewind/terminal",
        r#"{"case":"branched","copy":"exact accepted commit"}"#,
    );
    assert_json(&rules, "artifact-stage", "/runtimeBinaryPreflight", "false");
}

#[test]
fn inventory_guards_named_types_and_family_isolation() {
    for (label, mutated) in [
        (
            "named head erasure",
            SOURCE.replacen("expected-head: head-stamp", "expected-head: string", 1),
        ),
        (
            "cross-family contamination",
            SOURCE.replacen(
                "    load-ledger: func",
                "    type todo-contamination = string;\n\n    load-ledger: func",
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
    type_inventory_with_resource(resolve, host, pack, "session-operation", TYPE_INVENTORY)
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
    let pack = &resolve.interfaces[pack_id];
    let host = &resolve.interfaces[host_id];
    assert_eq!(contract_inventory(resolve, package_id), TYPE_INVENTORY);
    assert_eq!(pack.functions.len(), 2);
    assert_eq!(host.functions.len(), 2);
    assert_invoke_and_pull(
        resolve,
        pack,
        "session-operation",
        "session-request",
        "session-error",
        "session-pull",
    );
    assert_eq!(
        pack.types.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "session-request",
            "session-pull",
            "session-error",
            "session-operation"
        ]
    );
    assert_named_aliases(
        resolve,
        pack,
        host,
        &["session-request", "session-pull", "session-error"],
    );
    assert_freestanding_function(
        resolve,
        host,
        "load-ledger",
        "request",
        "ledger-read",
        "ledger-page",
        "session-host-error",
    );
    assert_freestanding_function(
        resolve,
        host,
        "commit-ledger",
        "mutation",
        "ledger-mutation",
        "ledger-commit",
        "session-host-error",
    );
    assert_denied_names(
        resolve,
        package_id,
        &["value", "map", "metadata", "pack-operation"],
        &[
            "compaction-",
            "resources-",
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
            "compaction-",
            "resources-",
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
