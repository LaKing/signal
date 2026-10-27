//! ADR 0037 phase 3 (tests 4 and 8 as far as they need no phase 2;
//! `docs/ui-protocol.md` "patch", "graph", "`slot.projects`"): over both
//! transports, on a scratch project with a loaded and an empty slot:
//! `graph`'s slot boxes and templates; Load, Replace, Unload and every
//! interface edit written as text through `patch`, each undone and redone;
//! a rename rewriting the wires and the panel's bindings; removing a wired
//! port refused, naming the wires; a template written into `interfaces/`
//! and named; `slot.projects`; the binding check and the streams through
//! a slot's match (inactive on an empty slot); the slot's interface
//! parameter as a control. No committed project is written.

mod common;

use std::fs;
use std::path::PathBuf;

use common::{Options, Rig, Transport, Ws, rig};
use serde_json::{Value, json};

const HOST: &str = r#"schema_version = 1

[project]
name = "slots"
sample_rate = 48000
seed = 1

[instances]
osc = { module = "std.sine_osc", freq = 220 }
# the effect slot
fx = { slot = { inputs = { x = "audio" }, outputs = { y = "audio" }, parameters = { volume = { unit = "dB" } } }, load = "std.gain", map = { x = "in", y = "out", volume = "db" }, volume = -6 }
spare = { slot = "lib.voice" }
byname = { slot = { inputs = { in = "audio" }, outputs = { out = "audio" } }, load = "std.gain" }
out = { module = "std.audio_out", gain = 0.5 }

[wiring]
connections = [
  { from = "osc.out", to = "fx.x" },
  { from = "fx.y", to = "out.l" },
  { from = "fx.y", to = "out.r" },
]
"#;

const PANEL: &str = r#"<!doctype html>
<signal-panel>
<section class="signal-card"><h2>fx</h2>
<signal-slot bind="/fx"></signal-slot>
<signal-scope bind="/fx.y"></signal-scope>
<signal-knob bind="/fx.volume"></signal-knob>
<signal-scope bind="/spare.out"></signal-scope>
<signal-scope bind="/byname.out"></signal-scope>
</section>
</signal-panel>
"#;

/// A sibling project a slot can load: an oscillator into its sink.
const SIBLING: &str = r#"schema_version = 1

[project]
name = "beside"
sample_rate = 48000

[instances]
osc = { module = "std.sine_osc", freq = 110 }
out = { module = "std.audio_out", gain = 0.25 }

[wiring]
connections = [
  { from = "osc.out", to = "out.l" },
  { from = "osc.out", to = "out.r" },
]
"#;

fn slots(name: &str, transport: Transport) -> Rig {
    let source: PathBuf = std::env::temp_dir().join(format!(
        "signal-ui-slots-source-{name}-{:?}-{}",
        transport,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&source);
    fs::create_dir_all(source.join("ui")).unwrap();
    fs::write(source.join("project.toml"), HOST).unwrap();
    fs::write(source.join("ui/main.html"), PANEL).unwrap();
    let rig = rig(
        name,
        Options {
            transport,
            project: Some(source.clone()),
            ..Options::default()
        },
    );
    let _ = fs::remove_dir_all(&source);
    rig
}

fn graph(rig: &mut Rig, ws: &mut Ws, id: u64) -> Value {
    rig.request(ws, id, json!({"type": "graph"}), "graph")
}

fn instance<'a>(graph: &'a Value, name: &str) -> &'a Value {
    graph["top"]["instances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == name)
        .unwrap_or_else(|| panic!("no instance {name} in {graph}"))
}

/// Patches `ops` at the top level against the file as it is now.
fn patch(rig: &mut Rig, ws: &mut Ws, id: u64, ops: Value) -> Value {
    let base = graph(rig, ws, id)["files"][""].clone();
    rig.request(
        ws,
        id + 1,
        json!({"type": "patch", "level": "", "base": base, "ops": ops}),
        "patched",
    )
}

fn refused(rig: &mut Rig, ws: &mut Ws, id: u64, ops: Value) -> Value {
    let base = graph(rig, ws, id)["files"][""].clone();
    rig.request(
        ws,
        id + 1,
        json!({"type": "patch", "level": "", "base": base, "ops": ops}),
        "error",
    )
}

/// `graph`: a slot box per slot, loaded or empty, with its interface's
/// ports, its match table and its map; the templates a slot may start from.
fn graph_gives_slot_boxes_and_templates(transport: Transport) {
    let mut rig = slots("slot-graph", transport);
    let (mut ws, _) = rig.client();
    let g = graph(&mut rig, &mut ws, 1);
    let fx = instance(&g, "fx");
    assert_eq!(fx["module"], "std.gain");
    assert_eq!(fx["slot"]["load"], "std.gain");
    assert_eq!(
        fx["slot"]["map"],
        json!({"x": "in", "y": "out", "volume": "db"})
    );
    assert_eq!(fx["parameters"][0]["name"], "volume");
    let spare = instance(&g, "spare");
    assert_eq!(spare["module"], "");
    assert_eq!(spare["slot"]["from"], "lib.voice");
    assert_eq!(spare["ports"].as_array().unwrap().len(), 4, "{spare}");
    let templates: Vec<&str> = g["interfaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(templates, ["lib.deck", "lib.instrument", "lib.voice"]);
    // `describe`: the slot's interface parameter is a control, under its own name.
    let d = rig.request(&mut ws, 2, json!({"type": "describe"}), "describe");
    let volume = d["controls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["address"] == "/fx.volume")
        .unwrap_or_else(|| panic!("no /fx.volume control: {d}"));
    assert_eq!(volume["base"], -6.0);
    assert!(
        !d["controls"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["address"] == "/fx.db"),
        "the content's own name is no control"
    );
    rig.finish();
}

/// Load, Replace and Unload as patches, each undone byte for byte.
fn load_replace_unload_are_patches(transport: Transport) {
    let mut rig = slots("slot-load", transport);
    let (mut ws, _) = rig.client();
    let path = rig.dir.join("project.toml");
    let unloaded = patch(
        &mut rig,
        &mut ws,
        1,
        json!([{"op": "slot.unload", "slot": "fx"}]),
    );
    let text = fs::read_to_string(&path).unwrap();
    let fx = text.lines().find(|l| l.starts_with("fx = ")).unwrap();
    assert!(!fx.contains("load = "), "{text}");
    assert!(text.contains("# the effect slot"), "comments kept: {text}");
    rig.wait(&mut ws, "the reload", |got| got.of("reload").cloned());
    // Undone: the file as it was.
    let base = graph(&mut rig, &mut ws, 3)["files"][""].clone();
    rig.request(
        &mut ws,
        4,
        json!({"type": "patch", "level": "", "base": base, "ops": unloaded["inverse"]}),
        "patched",
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), HOST);
    rig.wait(&mut ws, "the reload", |got| got.of("reload").cloned());
    // Replace: std.mix keeps `y = "out"` and `volume`? No: mix has no `db`;
    // its map entries it cannot take are removed, and the reply says so.
    let replaced = patch(
        &mut rig,
        &mut ws,
        5,
        json!([{"op": "slot.load", "slot": "fx", "load": "std.mix", "map": {"x": "a", "y": "out"}}]),
    );
    assert!(
        replaced.get("notes").is_none(),
        "an explicit map: {replaced}"
    );
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("load = \"std.mix\", map = { x = \"a\", y = \"out\" }"),
        "{text}"
    );
    rig.wait(&mut ws, "the reload", |got| got.of("reload").cloned());
    // Load into the empty slot: a project beside this one.
    let beside = rig.dir.parent().unwrap().join("beside");
    fs::create_dir_all(&beside).unwrap();
    fs::write(beside.join("project.toml"), SIBLING).unwrap();
    let projects = rig.request(
        &mut ws,
        7,
        json!({"type": "slot.projects"}),
        "slot.projects",
    );
    let listed = projects["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["load"] == "../beside")
        .unwrap_or_else(|| panic!("../beside not listed: {projects}"));
    assert_eq!(listed["name"], "beside");
    let names: Vec<&str> = listed["ports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["l", "r"], "its sink's channels");
    patch(
        &mut rig,
        &mut ws,
        8,
        json!([{"op": "slot.interface", "slot": "spare", "detach": true}, {"op": "slot.load", "slot": "spare", "load": "../beside", "map": {"out": "l"}}]),
    );
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("load = \"../beside\""), "{text}");
    rig.wait(&mut ws, "the reload", |got| got.of("reload").cloned());
    let g = graph(&mut rig, &mut ws, 10);
    let spare = instance(&g, "spare");
    assert_eq!(spare["slot"]["project"], true);
    assert_eq!(spare["slot"]["load"], "../beside");
    rig.finish();
}

/// Interface edits through `patch`: a rename rewrites the wires and the
/// panel's bindings; a wired port's removal is refused naming its wires;
/// a template written to interfaces/ and set.
fn interface_edits_rewrite_wires_and_bindings(transport: Transport) {
    let mut rig = slots("slot-interface", transport);
    let (mut ws, _) = rig.client();
    let path = rig.dir.join("project.toml");
    let panel = rig.dir.join("ui/main.html");
    let renamed = patch(
        &mut rig,
        &mut ws,
        1,
        json!([{"op": "slot.interface", "slot": "fx", "rename": {"y": "wet"}}]),
    );
    assert_eq!(renamed["panels"], json!(["ui/main.html"]), "{renamed}");
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("{ from = \"fx.wet\", to = \"out.l\" }"),
        "{text}"
    );
    assert!(
        text.contains("wet = \"out\""),
        "the map key follows: {text}"
    );
    let html = fs::read_to_string(&panel).unwrap();
    assert!(html.contains("<signal-scope bind=\"/fx.wet\">"), "{html}");
    assert!(html.contains("<signal-slot bind=\"/fx\">"), "{html}");
    rig.wait(&mut ws, "the reload", |got| got.of("reload").cloned());
    // Undone: the wires, the map key and the binding renamed back.
    let base = graph(&mut rig, &mut ws, 3)["files"][""].clone();
    let undone = rig.request(
        &mut ws,
        4,
        json!({"type": "patch", "level": "", "base": base, "ops": renamed["inverse"]}),
        "patched",
    );
    assert_eq!(undone["panels"], json!(["ui/main.html"]));
    assert_eq!(fs::read_to_string(&path).unwrap(), HOST);
    assert_eq!(fs::read_to_string(&panel).unwrap(), PANEL);
    rig.wait(&mut ws, "the reload", |got| got.of("reload").cloned());
    // Removing a wired port: refused, its wires named, nothing written.
    let refusal = refused(
        &mut rig,
        &mut ws,
        5,
        json!([{"op": "slot.interface", "slot": "fx", "remove": ["y"]}]),
    );
    assert_eq!(refusal["code"], "in-use", "{refusal}");
    assert!(
        refusal["message"]
            .as_str()
            .unwrap()
            .contains("fx.y → out.l"),
        "{refusal}"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), HOST);
    // Add an input, retype it.
    patch(
        &mut rig,
        &mut ws,
        7,
        json!([{"op": "slot.interface", "slot": "fx", "add": [{"name": "side", "role": "input", "kind": "audio"}]}]),
    );
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .contains("inputs = { x = \"audio\", side = \"audio\" }")
    );
    rig.wait(&mut ws, "the reload", |got| got.of("reload").cloned());
    // A template: written into interfaces/, then the empty slot set to it.
    let template = "schema_version = 1\n\n[interface]\nname = \"bus\"\n\n[inputs]\nin = { kind = \"audio\" }\n\n[outputs]\nout = { kind = \"audio\" }\n";
    let written = rig.request(
        &mut ws,
        9,
        json!({"type": "file.write", "file": "interfaces/bus.toml", "text": template, "base": null}),
        "file.written",
    );
    assert_eq!(written["created"], true);
    let g = graph(&mut rig, &mut ws, 10);
    let bus = g["interfaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "bus")
        .unwrap_or_else(|| panic!("no bus template: {g}"));
    assert_eq!(bus["library"], false);
    assert!(bus["base"].is_string());
    patch(
        &mut rig,
        &mut ws,
        11,
        json!([{"op": "slot.interface", "slot": "spare", "template": "bus"}]),
    );
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .contains("spare = { slot = \"bus\" }")
    );
    rig.finish();
}

/// The binding check and the streams through a slot: `/fx.y` reaches
/// std.gain's `out`; `/spare.out` is inactive (a warning, not an error);
/// `<signal-slot>` binds a slot.
fn bindings_reach_through_the_match(transport: Transport) {
    let mut rig = slots("slot-bindings", transport);
    let (mut ws, _) = rig.client();
    let checked = rig.request(
        &mut ws,
        1,
        json!({"type": "panel.check", "file": "main.html"}),
        "panel.check",
    );
    assert_eq!(checked["ok"], true, "{checked}");
    let diagnostics = checked["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1, "{checked}");
    assert_eq!(diagnostics[0]["severity"], "warning", "{checked}");
    assert_eq!(
        diagnostics[0]["message"], "inactive: /spare.out: /spare is empty",
        "{checked}"
    );
    assert_eq!(diagnostics[0]["line"], 7);
    let scope = rig.request(
        &mut ws,
        2,
        json!({"type": "subscribe", "stream": "scope", "port": "/fx.y", "rate": 4800}),
        "subscribed",
    );
    assert!(scope["sub"].is_u64(), "{scope}");
    assert_eq!(scope["port"], "/fx.y", "{scope}");
    let inactive = rig.request(
        &mut ws,
        3,
        json!({"type": "subscribe", "stream": "scope", "port": "/spare.out", "rate": 4800}),
        "error",
    );
    assert_eq!(inactive["code"], "inactive", "{inactive}");
    rig.finish();
}

both!(
    graph_gives_slot_boxes_and_templates,
    load_replace_unload_are_patches,
    interface_edits_rewrite_wires_and_bindings,
    bindings_reach_through_the_match,
);

/// Two renames in one patch (`y` → `wet`, then `wet` → `y`) leave the
/// panel's bindings as they were; the snapshots' values of a renamed slot
/// parameter move to its new address and a removed one's go, the undo
/// snapshot's too (ADR 0041), and a slot control can be left out of
/// snapshots (`snapshot.exclude`).
fn renames_chain_and_snapshots_follow(transport: Transport) {
    let mut rig = slots("slot-chain", transport);
    let (mut ws, _) = rig.client();
    let panel = rig.dir.join("ui/main.html");
    patch(
        &mut rig,
        &mut ws,
        1,
        json!([
            {"op": "slot.interface", "slot": "fx", "rename": {"y": "wet"}},
            {"op": "slot.interface", "slot": "fx", "rename": {"wet": "y"}}
        ]),
    );
    assert_eq!(fs::read_to_string(&panel).unwrap(), PANEL);
    assert_eq!(
        fs::read_to_string(rig.dir.join("project.toml")).unwrap(),
        HOST
    );

    let snapshots = rig.dir.join("snapshots.toml");
    let mut all = signal_project::snapshot::Snapshots::default();
    let values: signal_project::snapshot::Values = [
        ("/fx.volume".to_owned(), -6.0),
        ("/out.gain".to_owned(), 0.5),
    ]
    .into();
    all.snapshots.insert("a".to_owned(), values.clone());
    all.undo = Some(values);
    fs::write(&snapshots, signal_project::snapshot::render(&all)).unwrap();
    let renamed = patch(
        &mut rig,
        &mut ws,
        3,
        json!([{"op": "slot.interface", "slot": "fx", "rename": {"volume": "amount"}}]),
    );
    assert_eq!(renamed["snapshots"], "snapshots.toml", "{renamed}");
    let text = fs::read_to_string(&snapshots).unwrap();
    assert!(
        !text.contains("/fx.volume") && text.matches("\"/fx.amount\" = -6").count() == 2,
        "{text}"
    );
    rig.wait(&mut ws, "the reload", |got| got.of("reload").cloned());
    // The slot's control is left out of snapshots, by its interface name.
    rig.request(
        &mut ws,
        5,
        json!({"type": "snapshot.exclude", "address": "/fx.amount", "exclude": true}),
        "snapshot.excluded",
    );
    assert!(
        fs::read_to_string(rig.dir.join("project.toml"))
            .unwrap()
            .contains("snapshot_exclude = [\"amount\"]")
    );
    patch(
        &mut rig,
        &mut ws,
        6,
        json!([{"op": "slot.interface", "slot": "fx", "remove": ["amount"]}]),
    );
    let text = fs::read_to_string(&snapshots).unwrap();
    assert!(
        !text.contains("/fx.") && text.contains("/out.gain"),
        "{text}"
    );
    let project = fs::read_to_string(rig.dir.join("project.toml")).unwrap();
    assert!(!project.contains("snapshot_exclude"), "{project}");
    rig.finish();
}

both!(renames_chain_and_snapshots_follow);

/// ADR 0041: a removal then a rename onto the removed name, in one patch,
/// keeps the renamed parameter's values; the write is pushed to every
/// client as the snapshot list; a rename that moves no value leaves
/// `snapshots.toml` as it is, byte for byte.
fn snapshots_follow_in_op_order(transport: Transport) {
    let mut rig = slots("slot-snap-order", transport);
    let (mut ws, _) = rig.client();
    let snapshots = rig.dir.join("snapshots.toml");
    let mut all = signal_project::snapshot::Snapshots::default();
    let values: signal_project::snapshot::Values = [
        ("/fx.volume".to_owned(), -6.0),
        ("/fx.level".to_owned(), 0.25),
    ]
    .into();
    all.snapshots.insert("a".to_owned(), values);
    // Not the canonical text: a rename that moves nothing must keep it.
    let written = format!("{}# kept\n", signal_project::snapshot::render(&all));
    fs::write(&snapshots, &written).unwrap();
    patch(
        &mut rig,
        &mut ws,
        1,
        json!([{"op": "slot.interface", "slot": "fx", "rename": {"x": "input"}}]),
    );
    assert_eq!(
        fs::read_to_string(&snapshots).unwrap(),
        written,
        "nothing moved"
    );
    rig.wait(&mut ws, "the reload", |got| got.of("reload").cloned());
    // Another client: it gets the snapshot list the write pushes.
    let (mut other, _) = rig.client();
    let reply = patch(
        &mut rig,
        &mut ws,
        3,
        json!([
            {"op": "slot.interface", "slot": "fx", "add": [{"name": "level", "role": "parameter"}]},
            {"op": "slot.interface", "slot": "fx", "remove": ["volume"]},
            {"op": "slot.interface", "slot": "fx", "rename": {"level": "volume"}}
        ]),
    );
    assert_eq!(reply["snapshots"], "snapshots.toml", "{reply}");
    let text = fs::read_to_string(&snapshots).unwrap();
    assert!(
        text.contains("\"/fx.volume\" = 0.25") && !text.contains("/fx.level"),
        "{text}"
    );
    let pushed = rig.wait(&mut other, "the snapshot list", |got| {
        got.of("snapshots").cloned()
    });
    assert_eq!(pushed["snapshots"][0]["name"], "a", "{pushed}");
    rig.finish();
}

both!(snapshots_follow_in_op_order);
