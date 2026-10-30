//! ADR 0046 (tests 2, 3 and 4): a slot's own panel part.
//!
//! - Group into slot on a copy of the studio, through `patch`: the entry,
//!   the main panel's controls on `synth` moved into `ui/parts/synth.html`
//!   with relative binds, the open slot card in their place, `graph`'s
//!   `slot.part` and `describe.parts` rooted, both panels passing
//!   `panel.check`; its undo gives `project.toml`, `ui/main.html` and
//!   `ui/windows.toml` back byte for byte and leaves no part; its redo
//!   groups again. A card moved whole renames its id in `ui/windows.toml`.
//! - The bookkeeping of §7 on a scratch project: an interface rename
//!   rewrites the part, a remove is `in-use` while the part binds the port
//!   (and with `removeControls` takes its controls, undone byte for byte),
//!   Load, Replace and Unload leave the part's bytes alone, a slot deleted
//!   takes its part and its undo gives it back.
//! - The binding check (`signal ui check`'s, and `panel.check`'s) of parts:
//!   `.p` on an interface port or parameter, `/inner` active and inactive,
//!   an address outside the slot refused, the marker against the file's
//!   name, a part of a slot in a composite rooted at each instance, a part
//!   naming no slot, and relative binds refused in a host panel.
//!
//! No committed project is written.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{Options, Rig, Transport, Ws, rig, root};
use serde_json::{Value, json};
use signal_core::Registry;
use signal_project::{Project, Severity};
use signal_ui::bindings::{Checked, check_project};

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

fn send_patch(rig: &mut Rig, ws: &mut Ws, id: u64, ops: Value, kind: &str) -> Value {
    let base = graph(rig, ws, id)["files"][""].clone();
    rig.request(
        ws,
        id + 1,
        json!({"type": "patch", "level": "", "base": base, "ops": ops}),
        kind,
    )
}

/// A patch of `project.toml` written, and its reload seen.
fn patch(rig: &mut Rig, ws: &mut Ws, id: u64, ops: Value) -> Value {
    let reply = send_patch(rig, ws, id, ops, "patched");
    rig.wait(ws, "the reload", |got| got.of("reload").cloned());
    reply
}

fn read(dir: &Path, file: &str) -> Option<String> {
    fs::read_to_string(dir.join(file)).ok()
}

const WINDOWS: &str = "# the windows\n[[layout.\"k\".windows]]\nview = \"main\"\nscreen = 0\nx = 0\ny = 0\nwidth = 900\nheight = 700\ncards = [\"tape-echo\", \"master\"]\n";

fn studio(name: &str, transport: Transport) -> Rig {
    let source = std::env::temp_dir().join(format!(
        "signal-ui-parts-source-{name}-{transport:?}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&source);
    common::copy_dir(&root().join("projects/studio"), &source);
    fs::write(source.join("ui/windows.toml"), WINDOWS).unwrap();
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

fn grouping_moves_the_controls_and_is_undone_byte_for_byte(transport: Transport) {
    let mut rig = studio("parts-group", transport);
    let (mut ws, _) = rig.client();
    let dir = rig.dir.clone();
    let files = ["project.toml", "ui/main.html", "ui/windows.toml"];
    let before: Vec<Option<String>> = files.iter().map(|f| read(&dir, f)).collect();

    let reply = patch(
        &mut rig,
        &mut ws,
        1,
        json!([{"op": "slot.group", "instance": "synth"}]),
    );
    let notes = reply["notes"].to_string();
    assert!(
        notes.contains("moved 27 controls on synth into ui/parts/synth.html"),
        "{reply}"
    );
    let project = read(&dir, "project.toml").unwrap();
    assert!(
        project.contains("synth = { slot = { inputs = { gate = {}, hz = {}, velocity = {} }"),
        "{project}"
    );
    let main = read(&dir, "ui/main.html").unwrap();
    assert!(
        main.contains("<signal-slot bind=\"/synth\"></signal-slot>"),
        "{main}"
    );
    assert!(!main.contains("bind=\"/synth."), "{main}");
    // Outside the panel element the main panel is as it was.
    assert!(main.starts_with(&before[1].as_ref().unwrap()[..400]));
    let part = read(&dir, "ui/parts/synth.html").unwrap();
    assert!(part.contains("<signal-panel part=\"synth\">"), "{part}");
    assert!(part.contains("bind=\".cutoff\""), "{part}");
    // Nothing moved whole: the windows stay.
    assert_eq!(read(&dir, "ui/windows.toml"), before[2]);
    let changed: Vec<&str> = reply["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["file"].as_str().unwrap())
        .collect();
    assert_eq!(changed, ["ui/main.html", "ui/parts/synth.html"]);

    // `graph`: the slot box carries its part, its controls rooted.
    let g = graph(&mut rig, &mut ws, 3);
    let slot = &instance(&g, "synth")["slot"];
    assert_eq!(slot["load"], "moog_voice");
    let part_json = &slot["part"];
    assert_eq!(part_json["file"], "ui/parts/synth.html");
    assert_eq!(part_json["error"], Value::Null);
    let controls = part_json["controls"].as_array().unwrap();
    assert_eq!(controls.len(), 27);
    assert!(
        controls
            .iter()
            .any(|c| c["bind"] == ".cutoff" && c["binds"] == json!(["/synth.cutoff"]))
    );
    assert_eq!(instance(&g, "echo")["slot"], Value::Null);

    // Both panels pass the binding check, the part at its root.
    for (id, file) in [(4, "parts/synth.html"), (5, "main.html")] {
        let checked = rig.request(
            &mut ws,
            id,
            json!({"type": "panel.check", "file": file}),
            "panel.check",
        );
        assert_eq!(checked["ok"], true, "{checked}");
        let errors: Vec<&Value> = checked["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["severity"] == "error")
            .collect();
        assert!(errors.is_empty(), "{checked}");
    }
    // `describe.parts`.
    let described = rig.request(&mut ws, 30, json!({"type": "describe"}), "describe");
    let parts = described["parts"].as_array().unwrap();
    assert_eq!(parts[0]["slot"], "/synth");
    assert_eq!(parts[0]["roots"], json!(["/synth"]));
    assert_eq!(parts[0]["controls"].as_array().unwrap().len(), 27);

    // Undo: every file back byte for byte, no part left.
    let undone = patch(&mut rig, &mut ws, 6, reply["inverse"].clone());
    for (file, was) in files.iter().zip(&before) {
        assert_eq!(&read(&dir, file), was, "{file}");
    }
    assert!(!dir.join("ui/parts/synth.html").exists());
    // Redo: grouped again.
    patch(&mut rig, &mut ws, 8, undone["inverse"].clone());
    assert_eq!(read(&dir, "ui/parts/synth.html").unwrap(), part);
    assert_eq!(read(&dir, "ui/main.html").unwrap(), main);
    assert_eq!(read(&dir, "project.toml").unwrap(), project);

    // A card moved whole: its id follows it into ui/windows.toml.
    let echo = patch(
        &mut rig,
        &mut ws,
        10,
        json!([{"op": "slot.group", "instance": "echo"}]),
    );
    let windows = read(&dir, "ui/windows.toml").unwrap();
    assert!(
        windows.contains("cards = [\"slot.echo\", \"echo:tape-echo\", \"master\"]"),
        "{windows}"
    );
    let main = read(&dir, "ui/main.html").unwrap();
    assert!(!main.contains("TAPE ECHO"));
    patch(&mut rig, &mut ws, 12, echo["inverse"].clone());
    assert_eq!(read(&dir, "ui/windows.toml"), before[2]);
    rig.finish();
}

#[test]
fn grouping_moves_the_controls_and_is_undone_byte_for_byte_tcp() {
    grouping_moves_the_controls_and_is_undone_byte_for_byte(Transport::Tcp);
}

#[test]
fn grouping_moves_the_controls_and_is_undone_byte_for_byte_local() {
    grouping_moves_the_controls_and_is_undone_byte_for_byte(Transport::Local);
}

/// A project with a loaded slot (`fx`) and an empty one (`spare`), each
/// with a part, and a composite whose slot has a part.
const HOST: &str = r#"schema_version = 1

[project]
name = "parts"
sample_rate = 48000
seed = 1

[instances]
osc = { module = "std.sine_osc", freq = 220 }
fx = { slot = { inputs = { x = "audio" }, outputs = { y = "audio", z = {} }, parameters = { volume = { unit = "dB" } } }, load = "std.gain", map = { x = "in", y = "out", volume = "db" }, volume = -6 }
spare = { slot = { inputs = { a = {} }, outputs = { b = {} }, parameters = { level = {} } } }
rack1 = { module = "rack" }
rack2 = { module = "rack" }
out = { module = "std.audio_out", gain = 0.5 }

[wiring]
connections = [
  { from = "osc.out", to = "fx.x" },
  { from = "fx.y", to = "out.l" },
  { from = "fx.y", to = "out.r" },
  { from = "osc.out", to = "rack1.in" },
  { from = "osc.out", to = "rack2.in" },
]
"#;

const RACK: &str = r#"schema_version = 1

[composite]
name = "rack"

[instances]
fx = { slot = { inputs = { in = {} }, outputs = { out = {} } }, load = "std.gain" }
sink = { module = "std.gain" }

[wiring]
connections = [
  { from = "fx.out", to = "sink.in" },
]

[exports]
inputs = { in = "fx.in" }
"#;

const MAIN: &str = r#"<!doctype html>
<signal-panel>
  <div class="signal-cards">
    <section class="signal-card">
      <h2>source</h2>
      <signal-knob bind="/osc.freq"></signal-knob>
    </section>
    <signal-slot bind="/fx"></signal-slot>
  </div>
</signal-panel>
"#;

fn part(key: &str, marker: &str, controls: &str) -> String {
    let _ = key;
    format!(
        "<!-- a part -->\n<signal-panel part=\"{marker}\">\n  <div class=\"signal-cards\">\n    <section class=\"signal-card\">\n      <h2>part</h2>\n{controls}    </section>\n  </div>\n</signal-panel>\n"
    )
}

const FX_PART: &str = "      <signal-knob bind=\".volume\" label=\"volume\"></signal-knob>\n      <signal-scope bind=\".y\"></signal-scope>\n      <signal-meter bind=\".z\" label=\"z\"></signal-meter>\n";

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("signal-ui-parts-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("ui/parts")).unwrap();
    fs::create_dir_all(dir.join("composites")).unwrap();
    fs::write(dir.join("project.toml"), HOST).unwrap();
    fs::write(dir.join("composites/rack.toml"), RACK).unwrap();
    fs::write(dir.join("ui/main.html"), MAIN).unwrap();
    fs::write(dir.join("ui/parts/fx.html"), part("fx", "fx", FX_PART)).unwrap();
    dir
}

fn bookkeeping_rig(name: &str) -> Rig {
    let source = scratch(&format!("source-{name}"));
    let rig = rig(
        name,
        Options {
            project: Some(source.clone()),
            ..Options::default()
        },
    );
    let _ = fs::remove_dir_all(&source);
    rig
}

#[test]
fn the_part_follows_the_slots_edits() {
    let mut rig = bookkeeping_rig("parts-follow");
    let (mut ws, _) = rig.client();
    let dir = rig.dir.clone();
    let part_file = "ui/parts/fx.html";
    let original = read(&dir, part_file).unwrap();

    // A rename of an interface parameter: the part's `.volume` follows.
    let renamed = patch(
        &mut rig,
        &mut ws,
        1,
        json!([{"op": "slot.interface", "slot": "fx", "rename": {"volume": "level"}}]),
    );
    assert_eq!(
        read(&dir, part_file).unwrap(),
        original.replace("bind=\".volume\"", "bind=\".level\"")
    );
    assert!(
        renamed["panels"].to_string().contains(part_file),
        "{renamed}"
    );
    patch(&mut rig, &mut ws, 3, renamed["inverse"].clone());
    assert_eq!(read(&dir, part_file).unwrap(), original);

    // A remove is refused while the part binds the port, naming its control.
    let refused = send_patch(
        &mut rig,
        &mut ws,
        5,
        json!([{"op": "slot.interface", "slot": "fx", "remove": ["z"]}]),
        "error",
    );
    assert_eq!(refused["code"], "in-use", "{refused}");
    assert!(
        refused["message"].as_str().unwrap().contains("z"),
        "{refused}"
    );
    assert_eq!(read(&dir, part_file).unwrap(), original);
    // With removeControls, the controls go too; the undo brings them back.
    let removed = patch(
        &mut rig,
        &mut ws,
        7,
        json!([{"op": "slot.interface", "slot": "fx", "remove": ["z"], "removeControls": true}]),
    );
    let without = read(&dir, part_file).unwrap();
    assert!(!without.contains(".z"), "{without}");
    assert!(without.contains(".volume"), "{without}");
    patch(&mut rig, &mut ws, 9, removed["inverse"].clone());
    assert_eq!(read(&dir, part_file).unwrap(), original);

    // Load, Replace and Unload leave the part's bytes alone.
    for (id, op) in [
        (11, json!({"op": "slot.unload", "slot": "fx"})),
        (
            13,
            json!({"op": "slot.load", "slot": "fx", "load": "std.gain"}),
        ),
        (
            15,
            json!({"op": "slot.load", "slot": "fx", "load": "std.gain", "map": {"x": "in", "y": "out", "volume": "db"}}),
        ),
    ] {
        patch(&mut rig, &mut ws, id, json!([op]));
        assert_eq!(read(&dir, part_file).unwrap(), original, "{id}");
    }

    // A slot deleted takes its part; the undo gives it back byte for byte.
    let main = read(&dir, "ui/main.html").unwrap();
    fs::write(
        dir.join("ui/main.html"),
        main.replace("    <signal-slot bind=\"/fx\"></signal-slot>\n", ""),
    )
    .unwrap();
    let deleted = patch(
        &mut rig,
        &mut ws,
        17,
        json!([
            {"op": "connection.remove", "from": "osc.out", "to": "fx.x"},
            {"op": "connection.remove", "from": "fx.y", "to": "out.l"},
            {"op": "connection.remove", "from": "fx.y", "to": "out.r"},
            {"op": "instance.remove", "name": "fx"}
        ]),
    );
    assert!(!dir.join(part_file).exists());
    assert!(
        deleted["files"].to_string().contains(part_file),
        "{deleted}"
    );
    patch(&mut rig, &mut ws, 19, deleted["inverse"].clone());
    assert_eq!(read(&dir, part_file).unwrap(), original);
    rig.finish();
}

#[test]
fn an_untrusted_panel_restore_writes_only_a_panel() {
    let mut rig = bookkeeping_rig("parts-restore");
    let (mut ws, _) = rig.client();
    let dir = rig.dir.clone();
    let main = read(&dir, "ui/main.html").unwrap();
    for text in [
        main.replace(
            "<!doctype html>",
            "<!doctype html><script>alert(1)</script>",
        ),
        main.replace("<h2>source</h2>", "<script>alert(1)</script>"),
        // The review's: a comment the reader ends at its first `-->`.
        main.replace(
            "<h2>source</h2>",
            "<h2>source</h2><!--><script>alert(1)</script>-->",
        ),
        main.replace(
            "<h2>source</h2>",
            "<h2>source</h2><!-- --!><img src=x onerror=alert(1)> -->",
        ),
    ] {
        let refused = send_patch(
            &mut rig,
            &mut ws,
            1,
            json!([{"op": "panel.restore", "file": "main.html", "text": text, "was": main}]),
            "error",
        );
        assert_eq!(refused["code"], "bad-value", "{refused}");
    }
    let refused = send_patch(
        &mut rig,
        &mut ws,
        3,
        json!([{"op": "panel.restore", "file": "parts/spare.html", "text": "<script>x</script><signal-panel part=\"spare\"></signal-panel>"}]),
        "error",
    );
    assert_eq!(refused["code"], "bad-value", "{refused}");
    // A new part's start tag: only as a part is written (`on…` after a newline, a tab or `/`).
    for tag in [
        "<signal-panel\nonclick=\"alert(1)\" part=\"spare\">",
        "<signal-panel\tonclick=\"alert(1)\" part=\"spare\">",
        "<signal-panel/onclick=\"alert(1)\" part=\"spare\">",
    ] {
        let text = format!("{tag}\n  <div class=\"signal-cards\"></div>\n</signal-panel>\n");
        let refused = send_patch(
            &mut rig,
            &mut ws,
            5,
            json!([{"op": "panel.restore", "file": "parts/spare.html", "text": text}]),
            "error",
        );
        assert_eq!(refused["code"], "bad-value", "{refused}");
    }
    assert!(!dir.join("ui/parts/spare.html").exists());
    assert_eq!(read(&dir, "ui/main.html").unwrap(), main);
    rig.finish();
}

/// The review's: a hand-written part (a page of its own around its
/// `<signal-panel>`) removed with its slot comes back with the undo, byte
/// for byte: the host restores what it removed itself.
#[test]
fn a_hand_written_part_deleted_with_its_slot_comes_back() {
    let mut rig = bookkeeping_rig("parts-hand-written");
    let (mut ws, _) = rig.client();
    let dir = rig.dir.clone();
    let part = format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><style>.x {{ color: red }}</style></head><body>\n<!-- by hand --><p>notes</p>\n{}</body></html>\n",
        part("fx", "fx", FX_PART)
    );
    fs::write(dir.join("ui/parts/fx.html"), &part).unwrap();
    let main = read(&dir, "ui/main.html").unwrap();
    fs::write(
        dir.join("ui/main.html"),
        main.replace("    <signal-slot bind=\"/fx\"></signal-slot>\n", ""),
    )
    .unwrap();
    let deleted = patch(
        &mut rig,
        &mut ws,
        1,
        json!([
            {"op": "connection.remove", "from": "osc.out", "to": "fx.x"},
            {"op": "connection.remove", "from": "fx.y", "to": "out.l"},
            {"op": "connection.remove", "from": "fx.y", "to": "out.r"},
            {"op": "instance.remove", "name": "fx"}
        ]),
    );
    assert!(!dir.join("ui/parts/fx.html").exists());
    let undone = patch(&mut rig, &mut ws, 3, deleted["inverse"].clone());
    assert_eq!(read(&dir, "ui/parts/fx.html").unwrap(), part);
    patch(&mut rig, &mut ws, 5, undone["inverse"].clone());
    assert!(!dir.join("ui/parts/fx.html").exists());
    rig.finish();
}

/// The review's: every interface edit that drops a name the part binds is
/// `in-use` (a template set here), unless its controls go too; a rename
/// onto a name the part binds already is refused (its undo could not part
/// the two).
#[test]
fn every_edit_dropping_a_bound_name_is_in_use() {
    let mut rig = bookkeeping_rig("parts-dropped");
    let (mut ws, _) = rig.client();
    let dir = rig.dir.clone();
    let spare = part(
        "spare",
        "spare",
        "      <signal-knob bind=\".level\" label=\"level\"></signal-knob>\n",
    );
    fs::write(dir.join("ui/parts/spare.html"), &spare).unwrap();
    let refused = send_patch(
        &mut rig,
        &mut ws,
        1,
        json!([{"op": "slot.interface", "slot": "spare", "template": "lib.voice"}]),
        "error",
    );
    assert_eq!(refused["code"], "in-use", "{refused}");
    assert!(
        refused["message"].as_str().unwrap().contains("level"),
        "{refused}"
    );
    assert_eq!(read(&dir, "ui/parts/spare.html").unwrap(), spare);
    let set = patch(
        &mut rig,
        &mut ws,
        3,
        json!([{"op": "slot.interface", "slot": "spare", "template": "lib.voice", "removeControls": true}]),
    );
    assert!(
        !read(&dir, "ui/parts/spare.html")
            .unwrap()
            .contains(".level")
    );
    patch(&mut rig, &mut ws, 5, set["inverse"].clone());
    assert_eq!(read(&dir, "ui/parts/spare.html").unwrap(), spare);

    // A rename and its undo give the part back byte for byte; a rename onto
    // a name the part already binds (`.gain`, not in the interface) is refused.
    let fx = read(&dir, "ui/parts/fx.html").unwrap();
    let renamed = patch(
        &mut rig,
        &mut ws,
        7,
        json!([{"op": "slot.interface", "slot": "fx", "rename": {"volume": "w"}}]),
    );
    patch(&mut rig, &mut ws, 9, renamed["inverse"].clone());
    assert_eq!(read(&dir, "ui/parts/fx.html").unwrap(), fx);
    fs::write(
        dir.join("ui/parts/fx.html"),
        fx.replace(
            "bind=\".volume\"",
            "bind=\".volume\"></signal-knob>\n      <signal-knob bind=\".gain\"",
        ),
    )
    .unwrap();
    let refused = send_patch(
        &mut rig,
        &mut ws,
        11,
        json!([{"op": "slot.interface", "slot": "fx", "rename": {"volume": "gain"}}]),
        "error",
    );
    assert_eq!(refused["code"], "bad-value", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains(".gain already"),
        "{refused}"
    );
    rig.finish();
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn checked(dir: &Path) -> Vec<Checked> {
    let project = Project::load(dir).unwrap();
    check_project(dir, &project, &registry()).unwrap()
}

fn findings(all: &[Checked], file: &str) -> Vec<(Severity, String)> {
    all.iter()
        .find(|c| c.file == file)
        .unwrap_or_else(|| panic!("{file} not checked: {all:?}"))
        .findings
        .iter()
        .map(|f| (f.diagnostic.severity, f.diagnostic.message.clone()))
        .collect()
}

#[test]
fn a_parts_binds_are_checked_inside_its_slot() {
    let dir = scratch("check");
    // The loaded slot's part: its interface's ports and parameter.
    let all = checked(&dir);
    assert_eq!(findings(&all, "ui/main.html"), vec![]);
    let fx = findings(&all, "ui/parts/fx.html");
    // `.z` is an auto output nothing matches: inactive, not an error.
    assert_eq!(fx.len(), 1, "{fx:?}");
    assert_eq!(fx[0].0, Severity::Warning);
    assert!(fx[0].1.contains("inactive: /fx.z"), "{fx:?}");

    // Inside the content: active when it is there, inactive when not;
    // outside the interface or the slot: errors.
    let controls = "      <signal-value bind=\"/gain.db\"></signal-value>\n      <signal-knob bind=\".nothere\"></signal-knob>\n      <signal-scope bind=\"osc.out\"></signal-scope>\n      <signal-scope bind=\"../osc.out\"></signal-scope>\n";
    fs::write(dir.join("ui/parts/fx.html"), part("fx", "fx", controls)).unwrap();
    let fx = findings(&checked(&dir), "ui/parts/fx.html");
    assert_eq!(fx[0].0, Severity::Warning, "{fx:?}");
    assert!(
        fx[0]
            .1
            .contains("inactive: /fx/gain.db: not in /fx's current content"),
        "{fx:?}"
    );
    assert_eq!(fx[1].0, Severity::Error);
    assert!(
        fx[1].1.contains("not in fx's interface: `.nothere`"),
        "{fx:?}"
    );
    for f in &fx[2..] {
        assert_eq!(f.0, Severity::Error);
        assert!(f.1.contains("bind it from the host's panel"), "{fx:?}");
    }
    assert_eq!(fx.len(), 4);

    // The empty slot: its interface binds inactive.
    let controls = "      <signal-knob bind=\".level\"></signal-knob>\n      <signal-scope bind=\".b\"></signal-scope>\n";
    fs::write(
        dir.join("ui/parts/spare.html"),
        part("spare", "spare", controls),
    )
    .unwrap();
    let spare = findings(&checked(&dir), "ui/parts/spare.html");
    assert_eq!(spare.len(), 2, "{spare:?}");
    assert!(
        spare
            .iter()
            .all(|f| f.0 == Severity::Warning && f.1.contains("inactive")),
        "{spare:?}"
    );

    // The marker against the file's name.
    fs::write(dir.join("ui/parts/spare.html"), part("spare", "fx", "")).unwrap();
    let spare = findings(&checked(&dir), "ui/parts/spare.html");
    assert!(
        spare
            .iter()
            .any(|f| f.0 == Severity::Error && f.1.contains("part=\"spare\"")),
        "{spare:?}"
    );

    // A part of no slot.
    fs::write(dir.join("ui/parts/osc.html"), part("osc", "osc", "")).unwrap();
    let osc = findings(&checked(&dir), "ui/parts/osc.html");
    assert!(osc[0].1.contains("no slot of the project"), "{osc:?}");
    fs::remove_file(dir.join("ui/parts/osc.html")).unwrap();
    fs::remove_file(dir.join("ui/parts/spare.html")).unwrap();

    // A slot in a composite: its part rooted at each instance.
    let controls = "      <signal-scope bind=\".out\"></signal-scope>\n      <signal-value bind=\"/nothing.x\"></signal-value>\n";
    fs::write(
        dir.join("ui/parts/rack.fx.html"),
        part("rack.fx", "rack/fx", controls),
    )
    .unwrap();
    let rack = findings(&checked(&dir), "ui/parts/rack.fx.html");
    let inactive: Vec<&String> = rack.iter().map(|f| &f.1).collect();
    assert_eq!(rack.len(), 2, "{rack:?}");
    assert!(inactive[0].contains("/rack1/fx/nothing.x"), "{rack:?}");
    assert!(inactive[1].contains("/rack2/fx/nothing.x"), "{rack:?}");

    // Relative binds are a part's: in a host panel, an error.
    fs::write(dir.join("ui/main.html"), MAIN.replace("/osc.freq", ".freq")).unwrap();
    let main = findings(&checked(&dir), "ui/main.html");
    assert_eq!(main.len(), 1);
    assert!(main[0].1.contains("only a slot's part"), "{main:?}");
    let _ = fs::remove_dir_all(&dir);
}

/// The page's script: the panel's own modules (`ui/signal-ui.js`, the slot
/// card) against the server at `url`, the card attached while the
/// connection is still connecting, as `<signal-panel>` makes it the
/// default connection at once; what the card read of its part, as JSON.
const PAGE: &str = r#"
globalThis.HTMLElement = class {
  constructor() { this.attributesMap = new Map(); }
  attachShadow() { this.shadowRoot = { querySelector: () => null, append() {}, firstChild: null }; return this.shadowRoot; }
  getAttribute(n) { return this.attributesMap.get(n) ?? null; }
  setAttribute(n, v) { this.attributesMap.set(n, String(v)); }
  hasAttribute(n) { return this.attributesMap.has(n); }
  addEventListener() {}
};
globalThis.customElements = { get: () => undefined, define: () => {} };
const [ui, url, token] = process.argv.slice(2);
const { connect } = await import(`${ui}/signal-ui.js`);
const { SignalSlot } = await import(`${ui}/controls/slot.js`);
const connection = connect({ url, token, backoff: [100] });
const slot = new SignalSlot();
slot.setAttribute('bind', '/echo');
let html = null;
slot.showPart = async () => { html = slot.partState?.html ?? null; };
slot.attach(connection);
const status = connection.status;
const until = Date.now() + 15000;
while (Date.now() < until && !slot.partState) await new Promise((r) => setTimeout(r, 50));
console.log(JSON.stringify({ attachedWhile: status, part: slot.partState, html }));
process.exit(0);
"#;

/// The operator's visual check (2026-10-30): the open slot card of a slot
/// grouped through `patch` reads its part over a real server with the
/// page's own modules, though it is attached before the connection opens
/// (the read is made once it is open, never refused as `closed`).
#[test]
fn the_page_reads_its_part_from_a_real_server() {
    let mut rig = studio("parts-page", Transport::Tcp);
    let (mut ws, _) = rig.client();
    patch(
        &mut rig,
        &mut ws,
        1,
        json!([{"op": "slot.group", "instance": "echo"}]),
    );
    let script = rig.dir.parent().unwrap().join("page.mjs");
    fs::write(&script, PAGE).unwrap();
    let url = format!("ws://{}/", rig.host().ws_addr());
    let token = rig.host().token().to_owned();
    let ui = format!(
        "file://{}",
        root().join("ui").canonicalize().unwrap().display()
    );
    let mut child = match std::process::Command::new("node")
        .arg(&script)
        .args([&ui, &url, &token])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipping: node is not on PATH");
            rig.finish();
            return;
        }
        Err(error) => panic!("node did not run: {error}"),
    };
    let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while child.try_wait().unwrap().is_none() && std::time::Instant::now() < until {
        rig.pump();
    }
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let read: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|_| panic!("{stdout}\n{}", String::from_utf8_lossy(&output.stderr)));
    assert_eq!(read["attachedWhile"], "connecting", "{read}");
    assert_eq!(read["part"]["exists"], true, "{read}");
    assert_eq!(read["part"]["error"], Value::Null, "{read}");
    let html = read["html"].as_str().unwrap();
    assert!(html.contains("bind=\".time_ms\""), "{html}");
    assert!(html.contains("<h2>TAPE ECHO</h2>"), "{html}");
    rig.finish();
}
