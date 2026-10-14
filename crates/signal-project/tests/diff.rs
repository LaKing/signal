//! `signal_project::diff` (M2-2 AC1): parameter-only changes give `set`s,
//! anything else a structural edit with the right removed and added
//! instances, formatting gives nothing, and the run settings cannot change.

use signal_core::{Curve, EventPayload, InstanceAddress, PortAddress, Registry};
use signal_project::{
    DiffOptions, ParameterChange, Project, ProjectEdit, SourceFile, diff, diff_with,
};

const VOICE: &str = r#"schema_version = 1

[composite]
name = "voice"

[instances]
low = { module = "std.sine_osc" }
high = { module = "std.sine_osc", freq = 441 }
shaper = { module = "std.tanh_shaper" }

[wiring]
connections = [
  { from = "low.out", to = "shaper.in" },
  { from = "high.out", to = "shaper.in", scale = 0.5 },
]

[exports]
inputs = { pm = ["low.pm", "high.pm"] }
outputs = { out = "shaper.out" }
parameters = { drive = "shaper.drive" }
"#;

const PROJECT: &str = r#"# A voice and a tone into the stereo sink.
schema_version = 1

[project]
name = "diff"
sample_rate = 48000
seed = 7

[instances]
voice = { module = "voice", drive = 2.0 }
tone = { module = "std.sine_osc", freq = 330 }
out = { module = "std.audio_out", gain = 0.5 }

[wiring]
connections = [
  { from = "voice.out", to = "out.l" },
  { from = "tone.out", to = "out.r", scale = 0.5 },
]
"#;

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn project_with(text: &str, voice: &str) -> Project {
    Project::from_sources(vec![
        SourceFile {
            path: "project.toml".to_owned(),
            text: text.to_owned(),
        },
        SourceFile {
            path: "composites/voice.toml".to_owned(),
            text: voice.to_owned(),
        },
    ])
    .unwrap()
}

fn project(text: &str) -> Project {
    project_with(text, VOICE)
}

/// `PROJECT` with `from` replaced by `to` (which must occur).
fn edited(from: &str, to: &str) -> Project {
    assert!(PROJECT.contains(from), "{from}");
    project(&PROJECT.replacen(from, to, 1))
}

fn edit_of(new: &Project) -> ProjectEdit {
    diff(&project(PROJECT), new, &registry()).unwrap()
}

fn set(port: &str, value: f64) -> ParameterChange {
    ParameterChange {
        port: port.parse::<PortAddress>().unwrap(),
        payload: EventPayload::Set(value),
    }
}

fn addresses(list: &[&str]) -> Vec<InstanceAddress> {
    list.iter().map(|a| a.parse().unwrap()).collect()
}

/// The parts of a structural edit: removed, added, sets.
fn structural(
    edit: ProjectEdit,
) -> (
    Vec<InstanceAddress>,
    Vec<InstanceAddress>,
    Vec<ParameterChange>,
) {
    match edit {
        ProjectEdit::Structural {
            removed,
            added,
            sets,
            ..
        } => (removed, added, sets),
        ProjectEdit::Parameters(changes) => panic!("expected a structural edit, got {changes:?}"),
    }
}

#[test]
fn identical_projects_give_an_empty_edit() {
    let edit = edit_of(&project(PROJECT));
    assert!(edit.is_empty());
}

#[test]
fn whitespace_and_comments_give_an_empty_edit() {
    let reformatted = PROJECT
        .replace("# A voice and a tone", "# Reworded: a voice and a tone")
        .replace("tone = { module", "tone   =   {   module")
        .replace("[wiring]", "\n\n[wiring]   # the connections")
        .replace("gain = 0.5 }", "gain = 0.50 }");
    let voice = VOICE.replace("[exports]", "# exports below\n[exports]\n");
    let edit = diff(
        &project(PROJECT),
        &project_with(&reformatted, &voice),
        &registry(),
    )
    .unwrap();
    assert!(edit.is_empty(), "{edit:?}");
}

#[test]
fn parameter_only_changes_give_only_sets() {
    let edit = edit_of(&edited("freq = 330", "freq = 440"));
    match edit {
        ProjectEdit::Parameters(changes) => assert_eq!(changes, vec![set("/tone.freq", 440.0)]),
        other => panic!("expected sets, got {other:?}"),
    }

    // Two at once, in address order; one through a composite's export lands
    // on the flattened primitive parameter.
    let new = project(
        &PROJECT
            .replace("gain = 0.5", "gain = 0.25")
            .replace("drive = 2.0", "drive = 3.0"),
    );
    match edit_of(&new) {
        ProjectEdit::Parameters(changes) => assert_eq!(
            changes,
            vec![set("/out.gain", 0.25), set("/voice/shaper.drive", 3.0)]
        ),
        other => panic!("expected sets, got {other:?}"),
    }

    // A changed value inside a composite file is a set on the flattened plan.
    let voice = VOICE.replace("freq = 441", "freq = 442");
    let edit = diff(
        &project(PROJECT),
        &project_with(PROJECT, &voice),
        &registry(),
    )
    .unwrap();
    match edit {
        ProjectEdit::Parameters(changes) => {
            assert_eq!(changes, vec![set("/voice/high.freq", 442.0)])
        }
        other => panic!("expected sets, got {other:?}"),
    }
}

#[test]
fn a_removed_parameter_key_sets_the_default() {
    let edit = edit_of(&edited(", freq = 330 }", " }"));
    let ProjectEdit::Parameters(changes) = edit else {
        panic!("expected sets");
    };
    assert_eq!(changes.len(), 1);
    assert_eq!(
        changes[0].port,
        "/tone.freq".parse::<PortAddress>().unwrap()
    );
    // `std.sine_osc`'s default frequency.
    let default = registry()
        .iter()
        .find(|t| t.descriptor.type_name == "std.sine_osc")
        .unwrap()
        .descriptor
        .parameters[0]
        .default;
    assert_eq!(changes[0].payload, EventPayload::Set(default));
}

#[test]
fn ramping_is_a_per_call_option() {
    let new = edited("freq = 330", "freq = 440");
    let options = DiffOptions {
        ramp_frames: Some(480),
    };
    match diff_with(&project(PROJECT), &new, &registry(), &options).unwrap() {
        ProjectEdit::Parameters(changes) => assert_eq!(
            changes,
            vec![ParameterChange {
                port: "/tone.freq".parse().unwrap(),
                payload: EventPayload::Ramp {
                    to: 440.0,
                    frames: 480,
                    curve: Curve::Linear,
                },
            }]
        ),
        other => panic!("expected ramps, got {other:?}"),
    }
    // The default is a set, and so is a ramp of 0 frames.
    assert_eq!(DiffOptions::default().ramp_frames, None);
    let zero = DiffOptions {
        ramp_frames: Some(0),
    };
    match diff_with(&project(PROJECT), &new, &registry(), &zero).unwrap() {
        ProjectEdit::Parameters(changes) => assert_eq!(changes, vec![set("/tone.freq", 440.0)]),
        other => panic!("expected sets, got {other:?}"),
    }
}

#[test]
fn adding_an_instance_is_structural() {
    let new = project(
        &PROJECT
            .replace(
                "out = { module",
                "tone2 = { module = \"std.sine_osc\", freq = 660 }\nout = { module",
            )
            .replace(
                "  { from = \"tone.out\"",
                "  { from = \"tone2.out\", to = \"out.r\" },\n  { from = \"tone.out\"",
            ),
    );
    let (removed, added, sets) = structural(edit_of(&new));
    assert_eq!(removed, vec![]);
    assert_eq!(added, addresses(&["/tone2"]));
    assert_eq!(
        sets,
        vec![],
        "an added instance starts from its file values"
    );
}

#[test]
fn removing_an_instance_is_structural() {
    let new = project(
        &PROJECT
            .replace("tone = { module = \"std.sine_osc\", freq = 330 }\n", "")
            .replace(
                "  { from = \"tone.out\", to = \"out.r\", scale = 0.5 },\n",
                "",
            ),
    );
    let (removed, added, sets) = structural(edit_of(&new));
    assert_eq!(removed, addresses(&["/tone"]));
    assert_eq!(added, vec![]);
    assert_eq!(sets, vec![]);
}

#[test]
fn retyping_removes_and_adds_the_same_address() {
    let new = edited(
        "{ module = \"std.sine_osc\", freq = 330 }",
        "{ module = \"std.noise\" }",
    );
    let (removed, added, _) = structural(edit_of(&new));
    assert_eq!(removed, addresses(&["/tone"]));
    assert_eq!(added, addresses(&["/tone"]));
}

#[test]
fn rewiring_is_structural_with_no_instance_changes() {
    for new in [
        edited("to = \"out.r\", scale = 0.5", "to = \"out.l\", scale = 0.5"),
        edited("scale = 0.5 }", "scale = 0.25 }"),
    ] {
        let (removed, added, sets) = structural(edit_of(&new));
        assert_eq!((removed, added, sets), (vec![], vec![], vec![]));
    }
}

#[test]
fn a_structural_edit_sets_only_survivors_whose_value_changed() {
    let new = project(
        &PROJECT
            .replace("gain = 0.5", "gain = 0.75")
            .replace(
                "out = { module",
                "tone2 = { module = \"std.sine_osc\", freq = 660 }\nout = { module",
            )
            .replace(
                "  { from = \"tone.out\"",
                "  { from = \"tone2.out\", to = \"out.r\" },\n  { from = \"tone.out\"",
            ),
    );
    let (_, added, sets) = structural(edit_of(&new));
    assert_eq!(added, addresses(&["/tone2"]));
    assert_eq!(sets, vec![set("/out.gain", 0.75)]);
}

#[test]
fn a_changed_sample_rate_or_seed_is_an_error() {
    let errors = edit_errors(&edited("sample_rate = 48000", "sample_rate = 44100"));
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].starts_with("project.toml:6:15: error: sample_rate changed from 48000 to 44100"),
        "{}",
        errors[0]
    );
    assert!(errors[0].contains("restart the project"), "{}", errors[0]);

    let errors = edit_errors(&edited("seed = 7", "seed = 8"));
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].starts_with("project.toml:7:8: error: seed changed from 7 to 8"),
        "{}",
        errors[0]
    );
    assert!(errors[0].contains("restart the project"), "{}", errors[0]);

    // A key left out (its default) is reported at the start of the file.
    let errors = edit_errors(&edited("seed = 7\n", ""));
    assert!(errors[0].starts_with("project.toml:1:1: error: seed changed from 7 to 0"));
}

#[test]
fn an_invalid_new_project_gives_its_diagnostics() {
    let errors = edit_errors(&edited("std.audio_out", "std.no_such_module"));
    assert_eq!(errors.len(), 1);
    assert!(errors[0].starts_with("project.toml:"), "{}", errors[0]);
    assert!(errors[0].contains("std.no_such_module"), "{}", errors[0]);
}

fn edit_errors(new: &Project) -> Vec<String> {
    match diff(&project(PROJECT), new, &registry()) {
        Ok(edit) => panic!("expected errors, got {edit:?}"),
        Err(diagnostics) => diagnostics.iter().map(ToString::to_string).collect(),
    }
}

/// M2.7-2 (AC7): a `gpu` pin is read by address, through composites; it is
/// a string on a primitive instance, never a composite's, and no composite
/// may export a parameter named `gpu`.
#[test]
fn gpu_pins_are_read_by_address_and_checked() {
    let pinned = project_with(
        &PROJECT.replacen(
            "tone = { module = \"std.sine_osc\", freq = 330 }",
            "tone = { module = \"std.sine_osc\", freq = 330, gpu = \"1\" }",
            1,
        ),
        &VOICE.replacen(
            "high = { module = \"std.sine_osc\", freq = 441 }",
            "high = { module = \"std.sine_osc\", freq = 441, gpu = \"metal-apple-m5-max\" }",
            1,
        ),
    );
    let plan = pinned.plan(&registry()).unwrap().plan;
    let pins: Vec<(String, String)> = pinned.gpu_pins(&plan).into_iter().collect();
    assert_eq!(
        pins,
        [
            ("/tone".to_owned(), "1".to_owned()),
            ("/voice/high".to_owned(), "metal-apple-m5-max".to_owned()),
        ]
    );

    let errors = |text: &str, voice: &str| -> Vec<String> {
        let files = vec![
            SourceFile {
                path: "project.toml".to_owned(),
                text: text.to_owned(),
            },
            SourceFile {
                path: "composites/voice.toml".to_owned(),
                text: voice.to_owned(),
            },
        ];
        match Project::from_sources(files) {
            Ok(_) => Vec::new(),
            Err(diagnostics) => diagnostics.iter().map(ToString::to_string).collect(),
        }
    };
    let on_composite = errors(
        &PROJECT.replacen("drive = 2.0 }", "drive = 2.0, gpu = \"0\" }", 1),
        VOICE,
    );
    assert_eq!(on_composite.len(), 1, "{on_composite:?}");
    assert!(
        on_composite[0].starts_with("project.toml:10:")
            && on_composite[0].contains("`voice` is a composite"),
        "{on_composite:?}"
    );
    let empty = errors(
        &PROJECT.replacen("freq = 330 }", "freq = 330, gpu = \"\" }", 1),
        VOICE,
    );
    assert!(empty[0].contains("`gpu` is empty"), "{empty:?}");
    let number = errors(
        &PROJECT.replacen("freq = 330 }", "freq = 330, gpu = 1 }", 1),
        VOICE,
    );
    assert!(number[0].contains("project.toml:11:"), "{number:?}");
    let exported = errors(
        PROJECT,
        &VOICE.replacen("drive = \"shaper.drive\"", "gpu = \"shaper.drive\"", 1),
    );
    assert!(
        exported
            .iter()
            .any(|e| e.starts_with("composites/voice.toml:") && e.contains("named `gpu`")),
        "{exported:?}"
    );
}

/// M2.7-2: a running project keeps its GPU placement; a changed pin, or a
/// pin on an added instance, needs a restart. Removing a pinned instance
/// does not.
#[test]
fn a_changed_gpu_pin_needs_a_restart() {
    let tone = "tone = { module = \"std.sine_osc\", freq = 330 }";
    let pinned = |pin: &str| tone.replacen(" }", &format!(", gpu = \"{pin}\" }}"), 1);
    let errors = edit_errors(&edited(tone, &pinned("0")));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with("project.toml:11:") && errors[0].contains("restart the project"),
        "{errors:?}"
    );
    let added = format!("{tone}\nextra = {{ module = \"std.sine_osc\", gpu = \"0\" }}");
    assert!(edit_errors(&edited(tone, &added))[0].contains("/extra: its `gpu` pin changed"));
    // Same pin on both sides: no restart.
    let old = project(&PROJECT.replacen(tone, &pinned("0"), 1));
    let same = project(
        &PROJECT
            .replacen(tone, &pinned("0"), 1)
            .replacen("330", "331", 1),
    );
    assert!(diff(&old, &same, &registry()).is_ok());
    let removed = project(&PROJECT.replacen(&format!("{tone}\n"), "", 1).replacen(
        "  { from = \"tone.out\", to = \"out.r\", scale = 0.5 },\n",
        "",
        1,
    ));
    assert!(diff(&old, &removed, &registry()).is_ok());
}
