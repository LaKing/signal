//! M2.9-1 (ADR 0026 §1): `Project::structure`, what the UI protocol's
//! `graph` gives, for the committed projects (read only, never written):
//! the structure as written, instances in source order, composites by name,
//! and the converters the compiler inserted on the connections they join.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use signal_core::Registry;
use signal_project::Project;

fn structure(project: &str) -> Value {
    let dir: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../projects")
        .join(project);
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let loaded = Project::load(&dir).unwrap();
    let planned = loaded.plan(&registry).unwrap();
    loaded.structure(&planned.plan, &registry)
}

fn names(level: &Value) -> Vec<&str> {
    level["instances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect()
}

fn every_converter(level: &Value) -> Vec<&Value> {
    level["connections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| c["converters"].as_array().unwrap())
        .collect()
}

#[test]
fn kick_is_described_as_written_with_its_nested_composites() {
    let s = structure("kick");
    let top = &s["top"];
    // Source order, not the name order the loader keeps.
    assert_eq!(names(top), ["transport", "drums", "lfo", "accent", "out"]);
    let drums = &top["instances"][1];
    assert_eq!(drums["address"], "/drums");
    assert_eq!(drums["module"], "drums");
    assert_eq!(drums["composite"], true);
    assert_eq!(top["instances"][0]["composite"], false);
    let ports: Vec<_> = drums["ports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["name"].as_str().unwrap(),
                p["direction"].as_str().unwrap(),
                p["address"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        ports,
        [
            ("trigger", "in", "/drums.trigger"),
            ("out", "out", "/drums.out")
        ]
    );
    assert_eq!(drums["ports"][1]["shape"], "signal");
    assert_eq!(drums["ports"][1]["rate"], 1);
    assert_eq!(drums["ports"][0]["shape"], "events");
    let parameters: Vec<_> = drums["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["address"].as_str().unwrap())
        .collect();
    assert_eq!(parameters, ["/drums.kick_decay", "/drums.kick_drive"]);
    let connections = top["connections"].as_array().unwrap();
    assert_eq!(connections.len(), 7);
    assert_eq!(connections[0]["from"], "/transport.beat");
    assert_eq!(connections[0]["to"], "/drums.trigger");
    assert_eq!(connections[3]["to"], "/drums.kick_decay");
    assert_eq!(connections[3]["scale"], 120.0);
    assert_eq!(connections[5]["scale"], Value::Null);

    let composites: Vec<_> = s["composites"].as_object().unwrap().keys().collect();
    assert_eq!(composites, ["drums", "kick"]);
    let inner = &s["composites"]["drums"];
    assert_eq!(names(inner), ["kick"]);
    assert_eq!(inner["instances"][0]["composite"], true);
    assert_eq!(inner["instances"][0]["address"], "kick");
    assert_eq!(
        inner["outputs"],
        json!([{"name": "out", "from": "kick.out", "direction": "out",
        "address": "out", "shape": "signal", "rate": 1, "kind": inner["outputs"][0]["kind"],
        "unit": null, "range": inner["outputs"][0]["range"], "order": 0}])
    );
    assert_eq!(
        inner["parameters"],
        json!([
            {"name": "kick_decay", "to": ["kick.decay"], "order": 0, "range": {"min": 10.0, "max": 2000.0}},
            {"name": "kick_drive", "to": ["kick.drive"], "order": 1, "range": {"min": 0.1, "max": 5.0}},
        ])
    );
    let kick = &s["composites"]["kick"];
    assert_eq!(
        names(kick),
        [
            "osc",
            "pitch_env",
            "amp_env",
            "vca",
            "shaper",
            "dc",
            "feedback"
        ]
    );
    assert_eq!(
        kick["inputs"][0]["to"],
        json!(["pitch_env.trigger", "amp_env.trigger"])
    );
    assert_eq!(kick["connections"][0]["from"], "pitch_env.out");
    assert_eq!(kick["connections"][0]["to"], "osc.freq");
    for level in [top, inner, kick] {
        assert!(every_converter(level).is_empty());
    }
}

#[test]
fn pulse_has_its_seventeen_instances_and_no_converter() {
    let s = structure("pulse");
    assert_eq!(names(&s["top"]).len(), 17);
    assert_eq!(names(&s["top"])[..3], ["transport", "seq", "glide"]);
    assert_eq!(s["top"]["connections"].as_array().unwrap().len(), 20);
    assert!(every_converter(&s["top"]).is_empty());
    assert_eq!(s["composites"], json!({}));
}

#[test]
fn physical_av_names_the_decimate_converter_on_its_rate_change() {
    let s = structure("physical-av");
    let connections = s["top"]["connections"].as_array().unwrap();
    for c in connections {
        let converters = c["converters"].as_array().unwrap();
        if c["from"] == "/env.out" && c["to"] == "/sparks.rate" {
            assert_eq!(converters.len(), 1, "{c}");
            let converter = &converters[0];
            assert_eq!(converter["conversion"], "decimate");
            assert_eq!(converter["fromRate"], 1);
            assert_eq!(converter["toRate"], 800);
            assert_eq!(converter["name"], "out_d800");
            assert!(
                converter["module"]
                    .as_str()
                    .unwrap()
                    .starts_with("core.decimate.d1.d800.signal."),
                "{converter}"
            );
        } else {
            assert!(converters.is_empty(), "{c}");
        }
    }
    let sparks = s["top"]["instances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "sparks")
        .unwrap();
    let rate = sparks["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "rate")
        .unwrap();
    assert_eq!(rate["rate"], 800);
}

/// The studio's structure, as the schematic's node tests read it
/// (`ui/lib/fixtures/studio-graph.json`, M2.13-2: the deep trace of
/// `/synth.hz` and its timing, the boundary notes, the bundles), is the one
/// `graph` gives for `projects/studio`: a copy that cannot drift. Run with
/// `SIGNAL_WRITE_UI_FIXTURE=1` to write it again after the studio changes;
/// without it this test only reads.
#[test]
fn studio_structure_matches_the_ui_fixture() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/lib/fixtures/studio-graph.json");
    let s = structure("studio");
    if std::env::var_os("SIGNAL_WRITE_UI_FIXTURE").is_some() {
        std::fs::create_dir_all(fixture.parent().unwrap()).unwrap();
        std::fs::write(&fixture, serde_json::to_string_pretty(&s).unwrap() + "\n").unwrap();
    }
    let text = std::fs::read_to_string(&fixture).unwrap_or_default();
    let written: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    assert!(
        written == s,
        "{} differs from the studio's structure; write it again with SIGNAL_WRITE_UI_FIXTURE=1",
        fixture.display()
    );
}

/// M2.13-2 (`docs/ui-protocol.md` "graph"): a composite's exports carry
/// `order`, their place in the file among their kind (`graph` lists them by
/// name), for the schematic's port groups.
#[test]
fn composite_exports_carry_their_order_in_the_file() {
    let s = structure("studio");
    let voice = &s["composites"]["moog_voice"];
    let order = |kind: &str| -> Vec<(String, u64)> {
        let mut all: Vec<_> = voice[kind]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["name"].as_str().unwrap().to_owned(),
                    e["order"].as_u64().unwrap(),
                )
            })
            .collect();
        all.sort_by_key(|(_, o)| *o);
        all
    };
    let inputs: Vec<_> = order("inputs").into_iter().map(|(n, _)| n).collect();
    assert_eq!(inputs, ["hz", "gate", "velocity"]);
    let parameters: Vec<_> = order("parameters").into_iter().map(|(n, _)| n).collect();
    assert_eq!(
        &parameters[..4],
        ["glide", "osc1_octave", "osc1_detune", "osc1_wave"]
    );
    assert_eq!(parameters.last().unwrap(), "volume");
    assert!(voice["inputs"][0].get("group").is_none());
}
