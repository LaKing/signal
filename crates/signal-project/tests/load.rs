//! Loading the example project folder into the graph description model, the
//! defaults, and the structural errors the loader itself reports.

use std::collections::BTreeMap;
use std::path::Path;

use signal_core::Registry;
use signal_graph::{
    CompositeDef, ConnectionDef, Exports, InputExport, InstanceDef, OutputExport, ParameterExport,
};
use signal_project::{LoadError, Project, SourceFile};

fn example() -> Project {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/example");
    Project::load(&dir).unwrap()
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn instance(name: &str, module: &str, parameters: &[(&str, f64)]) -> InstanceDef {
    InstanceDef {
        name: name.to_owned(),
        module: module.to_owned(),
        parameters: parameters
            .iter()
            .map(|(k, v)| ((*k).to_owned(), *v))
            .collect::<BTreeMap<_, _>>(),
        scope: None,
    }
}

fn connection(from: &str, to: &str, scale: Option<f64>) -> ConnectionDef {
    ConnectionDef {
        from: from.to_owned(),
        to: to.to_owned(),
        scale,
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

/// The diagnostics of `files`, which must fail to load.
fn load_errors(files: &[(&str, &str)]) -> Vec<String> {
    let files = files
        .iter()
        .map(|(path, text)| SourceFile {
            path: (*path).to_owned(),
            text: (*text).to_owned(),
        })
        .collect();
    match Project::from_sources(files) {
        Ok(_) => panic!("expected load errors"),
        Err(diagnostics) => diagnostics.iter().map(ToString::to_string).collect(),
    }
}

#[test]
fn example_loads_into_the_description_model() {
    let project = example();
    assert_eq!(project.name, "example");
    assert_eq!(project.seed, 7);
    assert_eq!(project.sample_rate, 96_000, "the default sample rate");
    assert_eq!(
        project
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        ["project.toml", "composites/voice.toml"]
    );
    assert_eq!(
        project.root,
        CompositeDef {
            name: String::new(),
            instances: vec![
                instance("out", "std.audio_out", &[("gain", 0.5)]),
                instance("voice", "voice", &[("drive", 2.0), ("freq", 220.0)]),
            ],
            connections: vec![
                connection("voice.out", "out.l", None),
                connection("voice.out", "out.r", Some(0.5)),
            ],
            exports: Exports::default(),
        }
    );
    assert_eq!(
        project.composites,
        [CompositeDef {
            name: "voice".to_owned(),
            instances: vec![
                instance("high", "std.sine_osc", &[("freq", 441.0)]),
                instance("low", "std.sine_osc", &[]),
                instance("shaper", "std.tanh_shaper", &[]),
            ],
            connections: vec![
                connection("low.out", "shaper.in", None),
                connection("high.out", "shaper.in", Some(0.5)),
            ],
            exports: Exports {
                inputs: vec![InputExport {
                    name: "pm".to_owned(),
                    to: strings(&["low.pm", "high.pm"]),
                }],
                outputs: vec![OutputExport {
                    name: "out".to_owned(),
                    from: "shaper.out".to_owned(),
                }],
                parameters: vec![
                    ParameterExport {
                        name: "drive".to_owned(),
                        to: strings(&["shaper.drive"]),
                        range: None,
                    },
                    ParameterExport {
                        name: "freq".to_owned(),
                        to: strings(&["low.freq", "high.freq"]),
                        range: None,
                    },
                ],
            },
        }]
    );
}

#[test]
fn example_plans_without_warnings() {
    let planned = example().plan(&registry()).unwrap();
    assert!(planned.warnings.is_empty(), "{:?}", planned.warnings);
    let addresses: Vec<_> = planned
        .plan
        .instances
        .iter()
        .map(|i| i.address.to_string())
        .collect();
    assert_eq!(
        addresses,
        ["/out", "/voice/high", "/voice/low", "/voice/shaper"]
    );
    // The composite instance's `freq = 220` wins over the inner `freq = 441`.
    let high = &planned.plan.instances[1];
    assert_eq!(high.parameters["freq"], 220.0);
}

#[test]
fn missing_project_file_is_an_io_error() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/no-such-project");
    assert!(matches!(Project::load(&dir), Err(LoadError::Io { .. })));
}

const PROJECT: &str = r#"schema_version = 1

[project]
name = "t"

[instances]
v = { module = "voice" }
"#;

const VOICE: &str = r#"schema_version = 1

[composite]
name = "voice"

[instances]
osc = { module = "std.sine_osc" }

[exports]
outputs = { out = "osc.out" }
"#;

#[test]
fn unknown_key_in_exports_is_an_error() {
    let voice = VOICE.replace("[exports]\n", "[exports]\nresults = { x = \"osc.out\" }\n");
    assert_eq!(
        load_errors(&[("project.toml", PROJECT), ("composites/voice.toml", &voice)]),
        [
            "composites/voice.toml:10:1: error: unknown field `results`, expected one of `inputs`, `outputs`, `parameters`"
        ]
    );
}

#[test]
fn unknown_key_in_a_connection_is_an_error() {
    let project = format!(
        "{PROJECT}\n[wiring]\nconnections = [{{ from = \"v.out\", to = \"v.out\", scal = 2 }}]\n"
    );
    assert_eq!(
        load_errors(&[("project.toml", &project), ("composites/voice.toml", VOICE)]),
        ["project.toml:10:48: error: unknown field `scal`, expected one of `from`, `to`, `scale`"]
    );
}

#[test]
fn an_output_maps_to_exactly_one_port() {
    let voice = VOICE.replace(r#"out = "osc.out""#, r#"out = ["osc.out"]"#);
    assert_eq!(
        load_errors(&[("project.toml", PROJECT), ("composites/voice.toml", &voice)]),
        ["composites/voice.toml:10:19: error: invalid type: sequence, expected a string"]
    );
}

#[test]
fn composite_name_must_match_its_file() {
    assert_eq!(
        load_errors(&[("project.toml", PROJECT), ("composites/other.toml", VOICE)]),
        [
            "composites/other.toml:4:8: error: composite `voice` must be defined in composites/voice.toml, not composites/other.toml"
        ]
    );
}

#[test]
fn schema_version_must_be_one() {
    let project = PROJECT.replace("schema_version = 1", "schema_version = 2");
    assert_eq!(
        load_errors(&[("project.toml", &project)]),
        ["project.toml:1:18: error: unsupported schema_version 2; this version of Signal reads 1"]
    );
}

#[test]
fn sample_rate_must_be_positive() {
    let project = PROJECT.replace("name = \"t\"", "name = \"t\"\nsample_rate = 0");
    assert_eq!(
        load_errors(&[("project.toml", &project)]),
        ["project.toml:5:15: error: sample_rate must be at least 1"]
    );
}

#[test]
fn parameter_values_are_numbers() {
    let project = PROJECT.replace(
        r#"{ module = "voice" }"#,
        r#"{ module = "voice", gain = "loud" }"#,
    );
    assert_eq!(
        load_errors(&[("project.toml", &project)]),
        ["project.toml:7:32: error: invalid type: string \"loud\", expected f64"]
    );
}

const GROUPED: &str = r#"schema_version = 1

[composite]
name = "voice"

[instances]
osc = { module = "std.sine_osc" }
amp = { module = "std.gain" }
amp2 = { module = "std.gain" }

[wiring]
connections = [{ from = "osc.out", to = "amp.in" }]

[exports]
inputs = { hz = { to = "osc.freq", group = "pitch" } }
outputs = { out = { from = "amp.out", group = "audio" } }
parameters = { level = { to = ["amp.db"], group = "audio" }, plain = "amp2.db" }
"#;

/// M2.13-2: `group` on an export (its table form) loads, means the same
/// ports as the plain form, and reaches `graph` beside each export's place
/// in the file; the plain forms still load.
#[test]
fn an_export_may_name_its_schematic_group() {
    let files = [
        ("project.toml", PROJECT),
        ("composites/voice.toml", GROUPED),
    ]
    .iter()
    .map(|(path, text)| SourceFile {
        path: (*path).to_owned(),
        text: (*text).to_owned(),
    })
    .collect();
    let project = Project::from_sources(files).unwrap_or_else(|d| panic!("{d:?}"));
    let voice = &project.composites[0];
    assert_eq!(voice.exports.inputs[0].to, strings(&["osc.freq"]));
    assert_eq!(voice.exports.outputs[0].from, "amp.out");
    let level = &project.plan(&registry()).unwrap();
    let s = project.structure(&level.plan, &registry());
    let v = &s["composites"]["voice"];
    assert_eq!(v["inputs"][0]["group"], "pitch");
    assert_eq!(v["outputs"][0]["group"], "audio");
    let params: Vec<_> = v["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["name"].as_str().unwrap(),
                p["order"].as_u64().unwrap(),
                p.get("group").cloned(),
            )
        })
        .collect();
    assert_eq!(
        params,
        [
            ("level", 0, Some(serde_json::json!("audio"))),
            ("plain", 1, None)
        ]
    );
}

#[test]
fn a_group_table_takes_only_its_ports_and_group() {
    let voice = GROUPED.replace(r#"group = "pitch""#, r#"groupe = "pitch""#);
    let errors = load_errors(&[("project.toml", PROJECT), ("composites/voice.toml", &voice)]);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("unknown field `groupe`, expected one of `to`, `group`, `min`, `max`"),
        "{errors:?}"
    );
    let voice = GROUPED.replace(
        r#"{ from = "amp.out", group = "audio" }"#,
        r#"{ group = "audio" }"#,
    );
    let errors = load_errors(&[("project.toml", PROJECT), ("composites/voice.toml", &voice)]);
    assert!(
        errors.iter().any(|e| e.contains("missing field `from`")),
        "{errors:?}"
    );
}
