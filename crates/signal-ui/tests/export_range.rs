//! ADR 0042: a composite export's own range is its control's. `describe`
//! and the schematic's `graph` give it; `set` and `commit` clamp to it; a
//! snapshot's restore clamps to it and reports `clamped`.

mod common;

use std::fs;
use std::path::PathBuf;

use common::{Options, Rig, Transport, Ws, rig};
use serde_json::{Value, json};
use signal_project::snapshot::SNAPSHOTS_FILE;

const PROJECT: &str = r#"schema_version = 1

[project]
name = "ranged"
sample_rate = 48000

[instances]
v = { module = "voice" }
out = { module = "std.audio_out", gain = 0.25 }

[wiring]
connections = [
  { from = "v.out", to = "out.l" },
  { from = "v.out", to = "out.r" },
]
"#;

/// A sine through a `std.scale` (as kick909's click), its `mul` exported
/// as `attack` with the range [0, 2], and `freq` without one.
const VOICE: &str = r#"schema_version = 1

[composite]
name = "voice"

[instances]
osc = { module = "std.sine_osc", freq = 220 }
click = { module = "std.scale", mul = 0.5 }

[wiring]
connections = [{ from = "osc.out", to = "click.in" }]

[exports]
outputs = { out = "click.out" }
parameters = { attack = { to = "click.mul", min = 0, max = 2 }, freq = "osc.freq" }
"#;

fn ranged(name: &str, transport: Transport) -> (Rig, PathBuf) {
    let source = std::env::temp_dir().join(format!(
        "signal-ui-ranged-source-{name}-{transport:?}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&source);
    fs::create_dir_all(source.join("composites")).unwrap();
    fs::write(source.join("project.toml"), PROJECT).unwrap();
    fs::write(source.join("composites/voice.toml"), VOICE).unwrap();
    let rig = rig(
        name,
        Options {
            transport,
            project: Some(source.clone()),
            ..Options::default()
        },
    );
    (rig, source)
}

fn control(describe: &Value, address: &str) -> Value {
    describe["controls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["address"] == address)
        .unwrap_or_else(|| panic!("no control {address}: {describe}"))
        .clone()
}

fn describe(rig: &mut Rig, ws: &mut Ws, id: u64) -> Value {
    rig.request(ws, id, json!({"type": "describe"}), "describe")
}

fn attack_in_file(rig: &Rig) -> String {
    let text = fs::read_to_string(rig.dir.join("project.toml")).unwrap();
    text.lines()
        .find(|l| l.starts_with("v = "))
        .unwrap()
        .to_owned()
}

/// The control's range is the export's; the plain export keeps its
/// target's.
fn describe_and_graph_give_the_export_s_range(transport: Transport) {
    let (mut rig, source) = ranged("export-range-describe", transport);
    let (mut ws, _) = rig.client();
    let described = describe(&mut rig, &mut ws, 1);
    let attack = control(&described, "/v.attack");
    assert_eq!(
        (attack["min"].clone(), attack["max"].clone()),
        (json!(0.0), json!(2.0))
    );
    assert_eq!(attack["default"], 0.5);
    let freq = control(&described, "/v.freq");
    assert_eq!(freq["min"], 0.0, "std.sine_osc's own range: {freq}");
    assert_eq!(
        freq["max"], 100_000.0,
        "std.sine_osc's own range (ADR 0043): {freq}"
    );

    let graph = rig.request(&mut ws, 2, json!({"type": "graph"}), "graph");
    let text = graph.to_string();
    assert!(
        text.contains(r#""range":{"max":2.0,"min":0.0}"#),
        "the export's range in graph: {text}"
    );
    rig.finish();
    let _ = fs::remove_dir_all(&source);
}

/// `set` and `commit` clamp to the export's range.
fn set_and_commit_clamp_to_the_export_s_range(transport: Transport) {
    let (mut rig, source) = ranged("export-range-commit", transport);
    let (mut ws, _) = rig.client();
    rig.request(
        &mut ws,
        1,
        json!({"type": "set", "address": "/v.attack", "value": -1e6}),
        "ack",
    );
    let now = describe(&mut rig, &mut ws, 2);
    assert_eq!(control(&now, "/v.attack")["base"], 0.0, "set clamped");
    rig.request(
        &mut ws,
        3,
        json!({"type": "commit", "address": "/v.attack", "value": 5}),
        "committed",
    );
    assert!(
        attack_in_file(&rig).contains("attack = 2"),
        "{}",
        attack_in_file(&rig)
    );
    rig.finish();
    let _ = fs::remove_dir_all(&source);
}

/// A snapshot holding a value outside the export's range is restored
/// clamped, and says so.
fn a_restore_clamps_to_the_export_s_range(transport: Transport) {
    let (mut rig, source) = ranged("export-range-restore", transport);
    let (mut ws, _) = rig.client();
    fs::write(
        rig.dir.join(SNAPSHOTS_FILE),
        "schema_version = 1\n\n[snapshots.kiallas]\n\"/v.attack\" = -1000000\n",
    )
    .unwrap();
    let restored = rig.request(
        &mut ws,
        1,
        json!({"type": "snapshot.restore", "name": "kiallas"}),
        "snapshot.restored",
    );
    assert_eq!(
        restored["clamped"],
        json!([{"address": "/v.attack", "value": -1e6, "to": 0.0}]),
        "{restored}"
    );
    assert!(
        attack_in_file(&rig).contains("attack = 0"),
        "{}",
        attack_in_file(&rig)
    );
    rig.finish();
    let _ = fs::remove_dir_all(&source);
}

/// ADR 0043: a discrete control (std.osc's `shape`, and an export of it
/// with its own value set): `describe` gives its values, named; `set`,
/// `commit` and a restore take the nearest of them.
fn a_value_set_control_takes_the_nearest_value(transport: Transport) {
    let source = std::env::temp_dir().join(format!(
        "signal-ui-value-set-source-{transport:?}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&source);
    fs::create_dir_all(source.join("composites")).unwrap();
    fs::write(
        source.join("project.toml"),
        r#"schema_version = 1

[project]
name = "choices"
sample_rate = 48000

[instances]
o = { module = "std.osc", freq = 110, shape = 1 }
v = { module = "voice" }
out = { module = "std.audio_out", gain = 0.25 }

[wiring]
connections = [
  { from = "o.out", to = "out.l" },
  { from = "v.out", to = "out.r" },
]
"#,
    )
    .unwrap();
    fs::write(
        source.join("composites/voice.toml"),
        r#"schema_version = 1

[composite]
name = "voice"

[instances]
osc = { module = "std.osc", freq = 220 }

[exports]
outputs = { out = "osc.out" }
parameters = { wave = { to = "osc.shape", values = [{ value = 0, label = "sine" }, { value = 2, label = "square" }] } }
"#,
    )
    .unwrap();
    let mut rig = rig(
        "value-set",
        Options {
            transport,
            project: Some(source.clone()),
            ..Options::default()
        },
    );
    let (mut ws, _) = rig.client();
    let described = describe(&mut rig, &mut ws, 1);
    assert_eq!(
        control(&described, "/o.shape")["values"],
        json!([
            {"value": 0.0, "label": "sine"},
            {"value": 1.0, "label": "saw"},
            {"value": 2.0, "label": "square"},
            {"value": 3.0, "label": "triangle"},
        ])
    );
    assert_eq!(
        control(&described, "/v.wave")["values"],
        json!([{"value": 0.0, "label": "sine"}, {"value": 2.0, "label": "square"}]),
        "the export's own"
    );
    assert_eq!(
        control(&described, "/out.gain")["values"],
        Value::Null,
        "continuous"
    );
    rig.request(
        &mut ws,
        2,
        json!({"type": "set", "address": "/o.shape", "value": 2.4}),
        "ack",
    );
    assert_eq!(
        control(&describe(&mut rig, &mut ws, 3), "/o.shape")["base"],
        2.0
    );
    rig.request(
        &mut ws,
        4,
        json!({"type": "commit", "address": "/v.wave", "value": 1.2}),
        "committed",
    );
    let text = fs::read_to_string(rig.dir.join("project.toml")).unwrap();
    assert!(text.contains("wave = 2"), "the nearest of 0 and 2: {text}");
    fs::write(
        rig.dir.join(SNAPSHOTS_FILE),
        "schema_version = 1\n\n[snapshots.odd]\n\"/o.shape\" = 0.4\n",
    )
    .unwrap();
    let restored = rig.request(
        &mut ws,
        5,
        json!({"type": "snapshot.restore", "name": "odd"}),
        "snapshot.restored",
    );
    assert_eq!(
        restored["clamped"],
        json!([{"address": "/o.shape", "value": 0.4, "to": 0.0}]),
        "{restored}"
    );
    rig.finish();
    let _ = fs::remove_dir_all(&source);
}

both!(
    a_value_set_control_takes_the_nearest_value,
    describe_and_graph_give_the_export_s_range,
    set_and_commit_clamp_to_the_export_s_range,
    a_restore_clamps_to_the_export_s_range,
);
