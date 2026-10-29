//! ADR 0037, editable insides (2026-10-27): making a slot's own inside does
//! not change what plays.
//!
//! - `moog_voice` in a `lib.voice` slot inside the copy of studio-transport
//!   (`projects/two-studios/studio-transport`, as ADR 0037 test 1 builds
//!   it): its own inside, a copy of `moog_voice`, plans byte-identically to
//!   the slot before the copy, and so renders the same audio.
//! - A primitive wrapped into its own inside (one level down, so another
//!   plan) renders the same audio, bit for bit.
//!
//! The copies are made by `patch` (`slot.inside`) as the host applies it,
//! on fixture projects written under the target's temporary directory; the
//! committed projects are only read.

use std::fs;
use std::path::{Path, PathBuf};

use signal_core::Registry;
use signal_engine::{Engine, EngineConfig};
use signal_project::Project;
use signal_project::patch::{self, Op};

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).expect("std names are unique");
    registry
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("slot-insides")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// `ops` patched into `dir`'s `project.toml`, with the patch's other files,
/// as the host writes them.
fn patch_project(dir: &Path, ops: &[Op]) -> patch::Patched {
    let registry = registry();
    let sources = signal_project::read_sources(dir).unwrap();
    let project = Project::from_sources_in(sources.clone(), dir).ok();
    let module = |m: &str| -> Option<Vec<String>> {
        registry.get(m).map(|t| {
            t.descriptor
                .parameters
                .iter()
                .map(|p| p.name.to_owned())
                .collect()
        })
    };
    let templates = signal_project::interface_templates(&sources);
    let content =
        |load: &str| signal_project::content_names(project.as_ref(), dir, load, &registry);
    let file = |path: &str| {
        sources
            .iter()
            .find(|s| s.path == path)
            .map(|s| s.text.clone())
    };
    let source = |load: &str| signal_project::inside::source(&sources, load, &registry);
    let ctx = patch::Context {
        level: "",
        module: &module,
        templates: &templates,
        content: &content,
        file: &file,
        source: &source,
        bound: &patch::no_bindings,
    };
    let before = fs::read_to_string(dir.join("project.toml")).unwrap();
    let patched = patch::apply(&before, None, ops, &ctx).unwrap_or_else(|e| panic!("{e}"));
    fs::write(dir.join("project.toml"), &patched.text).unwrap();
    for change in &patched.files {
        let path = dir.join(&change.path);
        match &change.after {
            Some(text) => {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, text).unwrap();
            }
            None => fs::remove_file(&path).unwrap(),
        }
    }
    patched
}

fn plan_json(dir: &Path) -> (Project, String) {
    let project = Project::load(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    let plan = project
        .plan(&registry())
        .unwrap_or_else(|e| panic!("{}: {e:#?}", dir.display()))
        .plan;
    let json = serde_json::to_string(&plan).unwrap();
    (project, json)
}

/// `frames` of `dir` rendered: its audio as the sink records it.
fn audio(dir: &Path, frames: u64) -> Vec<f64> {
    let project = Project::load(dir).unwrap();
    let registry = registry();
    let plan = project.plan(&registry).unwrap().plan;
    let config = EngineConfig::new(project.sample_rate, project.seed);
    let mut engine = Engine::new(&plan, &registry, config, Vec::new()).unwrap();
    signal_output::render(&mut engine, frames).unwrap().samples
}

fn copy_folder(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
}

fn op(json: serde_json::Value) -> Op {
    serde_json::from_value(json).unwrap()
}

/// The voice slot of ADR 0037 test 1 given its own inside: the same plan,
/// byte for byte, so the same audio (2 s compared here).
#[test]
fn an_own_copy_of_a_voice_plans_and_plays_the_same() {
    let dir = scratch("voice");
    let source = repository().join("projects/two-studios/studio-transport");
    for folder in ["composites", "patterns"] {
        copy_folder(&source.join(folder), &dir.join(folder));
    }
    let text = fs::read_to_string(source.join("project.toml")).unwrap();
    let slotted = text.replace(
        "synth = { module = \"moog_voice\" }",
        "synth = { slot = \"lib.voice\", load = \"moog_voice\" }",
    );
    assert_ne!(slotted, text);
    fs::write(dir.join("project.toml"), slotted).unwrap();
    let (_, before) = plan_json(&dir);
    let heard = audio(&dir, 96_000);
    let patched = patch_project(
        &dir,
        &[op(
            serde_json::json!({"op": "slot.inside", "slot": "synth"}),
        )],
    );
    assert_eq!(patched.files.len(), 1);
    assert_eq!(patched.files[0].path, "composites/synth_inside.toml");
    let (_, after) = plan_json(&dir);
    assert_eq!(after, before, "the own inside plans byte-identically");
    assert_eq!(audio(&dir, 96_000), heard);
    // The shared original is untouched.
    assert_eq!(
        fs::read(dir.join("composites/moog_voice.toml")).unwrap(),
        fs::read(source.join("composites/moog_voice.toml")).unwrap()
    );
}

/// A primitive wrapped into its own inside plays the same, bit for bit,
/// though its plan is one level deeper.
#[test]
fn a_wrapped_primitive_plays_the_same() {
    let dir = scratch("wrap");
    fs::write(
        dir.join("project.toml"),
        r#"schema_version = 1

[project]
name = "wrap"
sample_rate = 48000

[instances]
osc = { module = "std.sine_osc", freq = 220 }
fx = { slot = { inputs = { x = "audio" }, outputs = { y = "audio" }, parameters = { level = {} } }, load = "std.gain", map = { x = "in", y = "out", level = "db" }, level = -6 }
out = { module = "std.audio_out" }

[wiring]
connections = [
  { from = "osc.out", to = "fx.x" },
  { from = "fx.y", to = "out.l" },
  { from = "fx.y", to = "out.r", scale = 0.5 },
]
"#,
    )
    .unwrap();
    let (_, before) = plan_json(&dir);
    let heard = audio(&dir, 24_000);
    assert!(heard.iter().any(|v| *v != 0.0));
    patch_project(
        &dir,
        &[op(serde_json::json!({"op": "slot.inside", "slot": "fx"}))],
    );
    let (_, after) = plan_json(&dir);
    assert_ne!(after, before);
    let played = audio(&dir, 24_000);
    let bits = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&played), bits(&heard));
    let inside = fs::read_to_string(dir.join("composites/fx_inside.toml")).unwrap();
    assert!(inside.contains("level = \"gain.db\""), "{inside}");
}
