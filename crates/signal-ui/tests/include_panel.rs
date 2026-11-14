//! ADR 0035 §10 (test 12, its override half): an include's top-level
//! parameters are controls (`/<include>/<instance>.<p>`) whose `commit`
//! writes an override into the including entry's `set` table, which a
//! reload keeps; Reset removes the key and an empty table with it; an
//! exported parameter is a control on the entry itself. No committed
//! project is written.

mod common;

use std::fs;

use common::{Options, Transport, rig};
use serde_json::{Value, json};

const HOST: &str = r#"schema_version = 1

[project]
name = "host"
sample_rate = 48000
seed = 1

[instances]
inc = { project = "inc" }
out = { module = "std.audio_out", gain = 0.5 }

[wiring]
connections = [
  { from = "inc.l", to = "out.l" },
  { from = "inc.r", to = "out.r" },
]
"#;

const INCLUDED: &str = r#"schema_version = 1

[project]
name = "inc"
sample_rate = 48000
seed = 2

[instances]
osc = { module = "std.sine_osc", freq = 220 }
amp = { module = "std.gain", db = -6 }
out = { module = "std.audio_out", gain = 0.25 }

[wiring]
connections = [
  { from = "osc.out", to = "amp.in" },
  { from = "amp.out", to = "out.l" },
  { from = "amp.out", to = "out.r" },
]

[exports]
parameters = { level = "amp.db" }
"#;

fn control<'a>(described: &'a Value, address: &str) -> Option<&'a Value> {
    described["controls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["address"] == address)
}

fn an_include_s_parameters_write_its_entry_and_set(transport: Transport) {
    let source = std::env::temp_dir().join(format!(
        "signal-ui-include-panel-source-{transport:?}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&source);
    fs::create_dir_all(source.join("inc")).unwrap();
    fs::write(source.join("project.toml"), HOST).unwrap();
    fs::write(source.join("inc/project.toml"), INCLUDED).unwrap();
    let mut rig = rig(
        "include-panel",
        Options {
            transport,
            project: Some(source.clone()),
            ..Options::default()
        },
    );
    let _ = fs::remove_dir_all(&source);
    let (mut ws, _) = rig.client();
    let path = rig.dir.join("project.toml");
    let d = rig.request(&mut ws, 1, json!({"type": "describe"}), "describe");
    assert!(control(&d, "/inc.level").is_some(), "the export: {d}");
    assert!(control(&d, "/inc/osc.freq").is_some(), "an override: {d}");
    assert!(
        control(&d, "/inc/amp.db").is_none(),
        "a parameter an export reaches is the export's"
    );
    // An override, into the entry's `set`.
    rig.request(
        &mut ws,
        2,
        json!({"type": "commit", "address": "/inc/osc.freq", "value": 330}),
        "committed",
    );
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains(r#"inc = { project = "inc", set = { "osc.freq" = 330 } }"#),
        "{text}"
    );
    assert_eq!(
        fs::read_to_string(rig.dir.join("inc/project.toml")).unwrap(),
        INCLUDED,
        "the included file is never written"
    );
    rig.wait(&mut ws, "the reload", |got| got.of("reload").cloned());
    let d = rig.request(&mut ws, 3, json!({"type": "describe"}), "describe");
    assert_eq!(control(&d, "/inc/osc.freq").unwrap()["base"], 330.0);
    // The export, on the entry.
    rig.request(
        &mut ws,
        4,
        json!({"type": "commit", "address": "/inc.level", "value": -12}),
        "committed",
    );
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("level = -12"), "{text}");
    rig.wait(&mut ws, "the reload", |got| got.of("reload").cloned());
    // Reset: the key goes, and the empty table with it.
    rig.request(
        &mut ws,
        5,
        json!({"type": "commit", "address": "/inc/osc.freq", "value": null}),
        "committed",
    );
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("set ="), "{text}");
    rig.finish();
}

both!(an_include_s_parameters_write_its_entry_and_set);
