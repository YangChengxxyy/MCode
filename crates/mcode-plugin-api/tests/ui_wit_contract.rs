//! Parse-based sole-current UI FeaturePack WIT contract tests.

mod support;

use support::{
    assert_denied_names, assert_denied_types, assert_invoke_and_pull, assert_json, assert_lf,
    assert_rule_inventory, assert_semantic_sha256, assert_zero_import_world_topology,
    package_interface, parse, semantic_rules, type_inventory_in_order,
};
use wit_parser::{PackageId, Resolve};

const SOURCE: &str = include_str!("../wit/feature-pack/ui.wit");
const GOLDEN: &str = include_str!("../goldens/feature_ui_current.wit");
const SEMANTICS: &str = include_str!("../goldens/feature_ui_current.jsonl");
const SEMANTICS_SHA256: &str = "fa2124809a4f9fa703f70d3d7474730a8778a2a10b1deac94cad784a8f7045bc";
const PACKAGE_ID: &str = "mcode:feature-pack@0.0.1";
const INVENTORY: &str = r#"ui-request=variant(render-runtime:render-runtime-request,handle-action:handle-action-request,resolve-theme:resolve-theme-request)
render-runtime-request=record(revision:u64,viewport:viewport,effective-capabilities:effective-capabilities,model:ui-model)
handle-action-request=record(revision:u64,action:ui-action)
resolve-theme-request=record(revision:u64,effective-capabilities:effective-capabilities)
viewport=record(columns:u16,rows:u16)
effective-capabilities=record(color:color-capability,unicode:bool,images:bool,hyperlinks:bool)
color-capability=enum(no-color,basic,ansi256,true-color)
ui-model=record(transcript:list<transcript-line>,composer:string,status:list<status-item>,panels:list<panel>,overlay:option<overlay>,picker:option<picker-view>,notifications:list<notification-view>,images:list<image-projection>,hyperlinks:list<hyperlink-projection>)
transcript-line=record(role:transcript-role,content:ui-content)
transcript-role=enum(user,assistant,tool,system)
ui-content=record(lines:list<content-line>)
content-line=record(spans:list<content-span>)
content-span=variant(text:text-span,image:image-span)
text-span=record(text:string,hyperlink:option<hyperlink-stamp>)
image-stamp=alias(string)
hyperlink-stamp=alias(string)
image-span=record(image:image-stamp)
status-item=record(id:string,label:string,value:string,tone:ui-tone)
ui-tone=enum(neutral,info,success,warning,error)
panel=record(id:string,title:string,body:ui-content)
overlay=record(kind:overlay-kind,title:string,body:ui-content)
overlay-kind=enum(dialog,help)
picker-view=record(id:string,title:string,query:string,items:list<picker-item>,selected:option<u16>)
picker-item=record(id:string,label:string,detail:option<string>,disabled:bool)
notification-view=record(id:string,tone:ui-tone,title:string,body:ui-content,actions:list<notification-button>)
notification-button=record(id:string,label:string)
image-projection=record(stamp:image-stamp,media-type:image-media-type,pixel-width:u32,pixel-height:u32,frame-count:u16,alt:string)
image-media-type=enum(png,jpeg,gif,webp,tiff)
hyperlink-projection=record(stamp:hyperlink-stamp,label:string)
ui-action=variant(none,submit-text:submit-text-action,focus:focus-action,scroll:scroll-action,dismiss-overlay,picker:picker-action,notification:notification-action,activate-hyperlink:activate-hyperlink-action)
submit-text-action=record(text:string)
focus-action=record(target:focus-target)
focus-target=variant(composer,transcript,panel:string,picker:string,overlay)
scroll-action=record(target:scroll-target,delta:s16)
scroll-target=variant(transcript,panel:string,picker:string,overlay)
picker-action=variant(move:picker-move,select:picker-select,cancel:picker-cancel)
picker-move=record(picker-id:string,delta:s16)
picker-select=record(picker-id:string,item-id:string)
picker-cancel=record(picker-id:string)
notification-action=variant(dismiss:notification-dismiss,activate:notification-activate)
notification-dismiss=record(notification-id:string)
notification-activate=record(notification-id:string,action-id:string)
activate-hyperlink-action=record(stamp:hyperlink-stamp)
ui-progress=enum(rendering)
ui-pull=variant(pending,progress:ui-progress,complete:ui-result,failed:ui-error)
ui-result=variant(frame:frame-result,action:action-result,theme:theme-result)
frame-result=record(revision:u64,viewport:viewport,clear:frame-clear,paints:list<paint-run>)
frame-clear=enum(all)
paint-run=record(row:u16,column:u16,content:paint-content,semantic-style:ui-style)
paint-content=variant(text:paint-text,image:paint-image)
paint-text=record(text:string,hyperlink:option<hyperlink-stamp>)
paint-image=record(image:image-stamp,columns:u16,rows:u16)
ui-style=record(foreground:theme-token-name,background:option<theme-token-name>,attributes:ui-attributes)
ui-attributes=flags(bold,dim,italic,underline,reverse,strikethrough)
ui-color=variant(default,indexed:u8,rgb:rgb-color)
rgb-color=record(red:u8,green:u8,blue:u8)
action-result=record(revision:u64,command:ui-command)
ui-command=variant(none,submit-text:submit-text-command,focus:focus-command,scroll:scroll-command,dismiss-overlay,picker:picker-command,notification:notification-command,open-hyperlink:open-hyperlink-command)
submit-text-command=record(text:string)
focus-command=record(target:focus-target)
scroll-command=record(target:scroll-target,delta:s16)
picker-command=variant(move:picker-move,select:picker-select,cancel:picker-cancel)
notification-command=variant(dismiss:notification-dismiss,activate:notification-activate)
open-hyperlink-command=record(stamp:hyperlink-stamp)
theme-result=record(revision:u64,tokens:list<theme-token>)
theme-token=record(token:theme-token-name,color:ui-color,attributes:ui-attributes)
theme-token-name=enum(background,surface,surface-raised,text-primary,text-muted,text-dim,border,border-focus,accent,accent-muted,success,warning,error,info,selection-background,selection-text,input-background,input-text,status-background,status-text,tool-title,tool-output,markdown-heading,markdown-link,markdown-code,markdown-quote,diff-added,diff-removed,diff-context,syntax-comment,syntax-keyword,syntax-function,syntax-variable,syntax-string,syntax-number,syntax-type,syntax-operator,syntax-punctuation,progress-track,progress-fill)
ui-error=enum(invalid-argument,wrong-role,stale-revision,unsupported-surface,limit,unavailable,cancelled)
ui-operation=resource
"#;
const SEMANTIC_RULES: &str = "action-reducer capabilities denied-surface frame-reducer image-paint-reducer logical-charge model-bounds operation-authority ownership projection-authority stable-errors stage terminal-matrix text-paint-reducer text-safety theme-reducer topology";

#[test]
fn ui_artifacts_are_identical_lf_and_have_exact_zero_import_shape() {
    assert_eq!(SOURCE.as_bytes(), GOLDEN.as_bytes());
    assert_lf("ui.wit", SOURCE);
    assert_lf("feature_ui_current.wit", GOLDEN);
    let (resolve, package_id) = parse("ui.wit", SOURCE);
    assert_shape(&resolve, package_id);
}

#[test]
fn ui_semantics_have_exact_rules_and_critical_values() {
    assert_lf("feature_ui_current.jsonl", SEMANTICS);
    assert_semantic_sha256(
        "feature_ui_current.jsonl",
        SEMANTICS,
        SEMANTICS_SHA256,
        (
            r#""order":["background","surface","surface-raised"#,
            r#""order":["surface","background","surface-raised"#,
        ),
    );
    let rules = semantic_rules(SEMANTICS);
    assert_rule_inventory(&rules, SEMANTIC_RULES.split_ascii_whitespace());
    assert_json(
        &rules,
        "topology",
        "/world",
        r#""mcode:feature-pack/ui@0.0.1""#,
    );
    assert_json(
        &rules,
        "topology",
        "/packInterface",
        r#""mcode:feature-pack/ui-pack@0.0.1""#,
    );
    assert_json(&rules, "topology", "/imports", "[]");
    assert_json(&rules, "model-bounds", "/viewport/columns/max", "512");
    assert_json(
        &rules,
        "projection-authority",
        "/imageStamp/grammar",
        r#""uimg1-[0-9a-f]{32}""#,
    );
    assert_json(
        &rules,
        "projection-authority",
        "/hyperlinkStamp/grammar",
        r#""ulnk1-[0-9a-f]{32}""#,
    );
    for (rule, pointer, expected) in [
        ("action-reducer", "/resultRevisionEqualsRequest", "true"),
        ("theme-reducer", "/resultRevisionEqualsRequest", "true"),
        ("frame-reducer", "/resultRevisionEqualsRequest", "true"),
        ("frame-reducer", "/viewportEqualsRequest", "true"),
        (
            "image-paint-reducer",
            "/columns/max",
            r#""viewport.columns""#,
        ),
        ("image-paint-reducer", "/rows/max", r#""viewport.rows""#),
    ] {
        assert_json(&rules, rule, pointer, expected);
    }
    assert_json(&rules, "theme-reducer", "/tokenCount", "40");
    assert_eq!(
        rules["theme-reducer"]["order"]
            .as_array()
            .expect("theme token order")
            .len(),
        40
    );
    assert_json(&rules, "theme-reducer", "/order/39", r#""progress-fill""#);
    assert_json(&rules, "frame-reducer", "/paintsMax", "8192");
    assert_json(
        &rules,
        "text-paint-reducer",
        "/rightEdge",
        r#""discard whole cluster, saturate cursor, clear predecessor, all later clusters zero-write""#,
    );
    assert_json(&rules, "stage", "/binaryPreflightClaim", "false");
    assert_json(&rules, "stage", "/runtimeClaim", "false");
}

#[test]
fn ui_mutations_cannot_erase_or_cross_family_types() {
    let erased = SOURCE.replacen("image: image-stamp", "image: string", 1);
    let (resolve, package_id) = parse("erased image stamp", &erased);
    let pack = package_interface(&resolve, package_id, "ui-pack");
    assert_ne!(
        type_inventory_in_order(&resolve, pack, INVENTORY),
        INVENTORY
    );

    let crossed = SOURCE.replacen(
        "type image-stamp = string;",
        "type image-stamp = string;\n    type subagents-request = string;",
        1,
    );
    let (resolve, package_id) = parse("cross-family type", &crossed);
    let pack = package_interface(&resolve, package_id, "ui-pack");
    assert_ne!(
        type_inventory_in_order(&resolve, pack, INVENTORY),
        INVENTORY
    );
    assert!(pack.types.contains_key("subagents-request"));

    let relabeled = SOURCE.replacen(
        "invoke: func(request: ui-request)",
        "invoke: func(payload: ui-request)",
        1,
    );
    let (resolve, package_id) = parse("relabeled invoke", &relabeled);
    let pack = package_interface(&resolve, package_id, "ui-pack");
    assert_ne!(pack.functions["invoke"].params[0].name, "request");
}

fn assert_shape(resolve: &Resolve, package_id: PackageId) {
    let pack_id =
        assert_zero_import_world_topology(resolve, package_id, PACKAGE_ID, "ui", "ui-pack");
    let pack = &resolve.interfaces[pack_id];
    assert_eq!(pack.name.as_deref(), Some("ui-pack"));
    assert_eq!(type_inventory_in_order(resolve, pack, INVENTORY), INVENTORY);
    assert_eq!(pack.functions.len(), 2);
    assert_invoke_and_pull(
        resolve,
        pack,
        "ui-operation",
        "ui-request",
        "ui-error",
        "ui-pull",
    );
    assert_denied_names(
        resolve,
        package_id,
        &["value", "metadata", "pack-operation"],
        &["subagents-", "workspace-"],
    );
    assert_denied_types(resolve);
}

// Rust guideline compliant 2026-08-30.
