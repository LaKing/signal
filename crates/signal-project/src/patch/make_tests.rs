//! ADR 0051 §5, Make instrument (`slot.make`): instances of the top level
//! moved into a new slot's own inside, every crossing connection through
//! its interface; the audio is the original's bit for bit (random streams
//! kept by `seed = "run"`); the undo gives every file back byte for byte;
//! the refusals.

use std::cell::RefCell;
use std::collections::BTreeMap as Files;

use super::*;
use crate::slot::{ContentNames, ContentPort, Role};
use crate::{Project, SourceFile};

static NO_TEMPLATES: BTreeMap<String, Interface> = BTreeMap::new();

fn registry() -> signal_core::Registry {
    let mut registry = signal_core::Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn modules(module: &str) -> Option<Vec<String>> {
    let registry = registry();
    let t = registry.get(module)?;
    Some(
        t.descriptor
            .parameters
            .iter()
            .map(|p| p.name.to_owned())
            .collect(),
    )
}

fn content(load: &str) -> Option<ContentNames> {
    let registry = registry();
    let ports = signal_graph::ports_of(load, &[], &registry).ok()?;
    Some(ContentNames {
        ports: ports
            .iter()
            .map(|p| ContentPort {
                name: p.name.clone(),
                role: if p.parameter {
                    Role::Parameter
                } else if p.direction == signal_core::Direction::Out {
                    Role::Output
                } else {
                    Role::Input
                },
                shape: p.shape,
                rate: p.rate.get(),
                kind: p.kind,
            })
            .collect(),
        settable: ports
            .iter()
            .filter(|p| p.parameter)
            .map(|p| p.name.clone())
            .collect(),
    })
}

fn source(load: &str) -> Option<crate::inside::Source> {
    crate::inside::source(&[], load, &registry())
}

fn no_bindings(_: &str) -> Vec<String> {
    Vec::new()
}

fn apply_with(
    text: &str,
    ops: &[Op],
    files: &RefCell<Files<String, String>>,
) -> Result<Patched, PatchError> {
    let file = |path: &str| files.borrow().get(path).cloned();
    let ctx = Context {
        level: "",
        module: &modules,
        templates: &NO_TEMPLATES,
        content: &content,
        file: &file,
        source: &source,
        bound: &no_bindings,
    };
    let patched = apply(text, None, ops, &ctx)?;
    for change in &patched.files {
        match &change.after {
            Some(text) => {
                files.borrow_mut().insert(change.path.clone(), text.clone());
            }
            None => {
                files.borrow_mut().remove(&change.path);
            }
        }
    }
    Ok(patched)
}

/// The left channel of `frames` frames of the project `text` with `files`.
fn render(text: &str, files: &Files<String, String>, frames: u64) -> Vec<u64> {
    let mut sources = vec![SourceFile {
        path: "project.toml".into(),
        text: text.to_owned(),
    }];
    sources.extend(files.iter().map(|(path, text)| SourceFile {
        path: path.clone(),
        text: text.clone(),
    }));
    let project = Project::from_sources(sources).unwrap_or_else(|d| panic!("{d:?}\n{text}"));
    let registry = registry();
    let plan = project
        .plan(&registry)
        .unwrap_or_else(|d| panic!("{d:?}\n{text}"))
        .plan;
    let config = signal_engine::EngineConfig::new(48_000, 9);
    let mut engine = signal_engine::Engine::new(&plan, &registry, config, Vec::new()).unwrap();
    let rendered = signal_output::render(&mut engine, frames).unwrap();
    rendered
        .samples
        .iter()
        .step_by(2)
        .map(|x| x.to_bits())
        .collect()
}

const HOST: &str = r#"schema_version = 1

[project]
name = "make"
sample_rate = 48000

[instances]
lfo = { module = "std.sine_osc", freq = 2 }
# the voice
osc = { module = "std.sine_osc", freq = 220 }   # a sine
n = { module = "std.noise" }
mix = { module = "std.add" }
amp = { module = "std.gain", db = -6 }
out = { module = "std.audio_out" }

[wiring]
connections = [
  { from = "osc.out", to = "mix.a" },
  { from = "n.out", to = "mix.b", scale = 0.1 },
  { from = "mix.out", to = "amp.in" },
  { from = "lfo.out", to = "osc.pm", scale = 0.01 },
  { from = "lfo.out", to = "amp.db", scale = 3 },
  { from = "amp.out", to = "out.l" },
  { from = "amp.out", to = "out.r" },
]
"#;

fn make(slot: &str, instances: &[&str]) -> Op {
    serde_json::from_value(
        serde_json::json!({"op": "slot.make", "slot": slot, "instances": instances}),
    )
    .unwrap()
}

#[test]
fn make_instrument_moves_the_instances_and_keeps_the_audio() {
    let files = RefCell::new(Files::new());
    let patched = apply_with(HOST, &[make("voice", &["osc", "n", "mix", "amp"])], &files)
        .unwrap_or_else(|e| panic!("{e}"));
    let inside = files
        .borrow()
        .get("composites/voice_inside.toml")
        .cloned()
        .unwrap();
    assert!(
        patched.text.contains(
            r#"voice = { slot = { inputs = { osc_pm = {} }, outputs = { amp_out = {} }, parameters = { amp_db = {} } }, load = "voice_inside", seed = "run" }"#
        ),
        "{}",
        patched.text
    );
    assert!(
        patched
            .text
            .contains(r#"{ from = "lfo.out", to = "voice.osc_pm", scale = 0.01 }"#),
        "{}",
        patched.text
    );
    assert!(
        patched
            .text
            .contains(r#"{ from = "voice.amp_out", to = "out.r" }"#),
        "{}",
        patched.text
    );
    assert!(!patched.text.contains("osc = {"), "{}", patched.text);
    assert!(inside.contains("inside_of = \"voice\""), "{inside}");
    assert!(
        inside.contains(r#"n = { module = "std.noise" }"#),
        "{inside}"
    );
    assert!(
        inside.contains(r#"{ from = "n.out", to = "mix.b", scale = 0.1 }"#),
        "{inside}"
    );
    // The audio, bit for bit: the noise draws its stream at /voice/n as at /n.
    let original = render(HOST, &Files::new(), 4_800);
    let made = render(&patched.text, &files.borrow(), 4_800);
    assert_eq!(original, made);
    // The undo: both files back byte for byte; the redo, again.
    let inverse: Vec<Op> =
        serde_json::from_str(&serde_json::to_string(&patched.inverse).unwrap()).unwrap();
    let undone =
        apply_with(&patched.text, &inverse, &files).unwrap_or_else(|e| panic!("undo: {e}"));
    assert_eq!(undone.text, HOST);
    assert!(files.borrow().get("composites/voice_inside.toml").is_none());
    let redone =
        apply_with(&undone.text, &undone.inverse, &files).unwrap_or_else(|e| panic!("redo: {e}"));
    assert_eq!(redone.text, patched.text);
    assert_eq!(
        files.borrow().get("composites/voice_inside.toml"),
        Some(&inside)
    );
    println!("{}\n---\n{inside}", patched.text);
}

#[test]
fn make_instrument_refuses_what_it_cannot_move() {
    let files = RefCell::new(Files::new());
    for (slot, instances, expected) in [
        ("voice", vec!["nope"], "no instance `nope`"),
        ("lfo", vec!["osc"], "`lfo` is taken"),
        ("Voice", vec!["osc"], "a slot's name"),
        ("voice", vec!["osc", "osc"], "each instance once"),
    ] {
        let found = apply_with(HOST, &[make(slot, &instances)], &files).map(|_| ());
        let text = format!("{found:?}");
        assert!(text.contains(expected), "{slot} {instances:?}: {text}");
    }
}
