//! ADR 0046 test 1 (equivalence): Group into slot changes no plan.
//!
//! - The studio's `synth` (a project composite, `moog_voice`) grouped into
//!   a slot plans byte-identically, so the studio's golden audio and log
//!   are reproduced from the grouped copy (120 s, as the golden), and its
//!   renders in blocks of 1, 64 and 256 frames on one and four threads give
//!   the ungrouped project's hashes.
//! - A primitive (the studio's `limiter`, `std.tanh_shaper`) and a project
//!   include (`two-studios`' `studio`, `../studio`, and `studio_transport`,
//!   a bare name) group with byte-identical plans.
//!
//! The groups are made by `patch` (`slot.group`) as the host applies it,
//! the panels' bindings read from each copy's `ui/`, on copies written
//! under the target's temporary directory; the committed projects are
//! only read.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use signal_core::Registry;
use signal_engine::{Engine, EngineConfig, Schedule};
use signal_output::render_blocks;
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
        .join("slot-group")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            fs::copy(&path, &target).unwrap();
        }
    }
}

/// `ops` patched into `dir`'s `project.toml` as the host patches it (the
/// panels' bindings from `dir/ui`).
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
    let ui = dir.join("ui");
    let paths = vec![String::new()];
    let bound = |name: &str| signal_ui::parts::bound_on(&ui, &paths, name);
    let ctx = patch::Context {
        level: "",
        module: &module,
        templates: &templates,
        content: &content,
        file: &file,
        source: &source,
        bound: &bound,
    };
    let before = fs::read_to_string(dir.join("project.toml")).unwrap();
    let patched = patch::apply(&before, None, ops, &ctx).unwrap_or_else(|e| panic!("{e}"));
    fs::write(dir.join("project.toml"), &patched.text).unwrap();
    patched
}

fn plan_json(dir: &Path) -> String {
    let project = Project::load(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    let plan = project
        .plan(&registry())
        .unwrap_or_else(|e| panic!("{}: {e:#?}", dir.display()))
        .plan;
    serde_json::to_string(&plan).unwrap()
}

fn group(dir: &Path, instance: &str) -> String {
    let op: Op =
        serde_json::from_value(serde_json::json!({"op": "slot.group", "instance": instance}))
            .unwrap();
    patch_project(dir, &[op]).text
}

/// `frames` of `dir` in blocks of `k` on `threads` threads: audio and log hashes.
fn hashes(dir: &Path, frames: u64, k: usize, threads: usize) -> (u64, u64) {
    let project = Project::load(dir).unwrap();
    let registry = registry();
    let plan = project.plan(&registry).unwrap().plan;
    let config = EngineConfig {
        threads,
        schedule: if threads > 1 {
            Schedule::FORCED
        } else {
            Schedule::DEFAULT
        },
        ..EngineConfig::new(project.sample_rate, project.seed)
    };
    let mut engine = Engine::new(&plan, &registry, config, Vec::new()).unwrap();
    let rendered = render_blocks(&mut engine, frames, k, |_| Ok(None)).unwrap();
    (rendered.audio_fnv1a64(), rendered.log_fnv1a64())
}

#[test]
fn the_studios_synth_grouped_plans_and_plays_the_same() {
    let dir = scratch("studio");
    copy_dir(&repository().join("projects/studio"), &dir);
    let before = plan_json(&dir);
    let reference = hashes(&dir, 48_000, 1, 1);
    let text = group(&dir, "synth");
    assert!(
        text.contains("synth = { slot = { inputs = { gate = {}, hz = {}, velocity = {} }, outputs = { out = {} }, parameters = { a_attack = {}"),
        "{text}"
    );
    assert!(text.contains("load = \"moog_voice\" }"), "{text}");
    assert_eq!(
        plan_json(&dir),
        before,
        "the grouped studio plans byte-identically"
    );
    for k in [1, 64, 256] {
        for threads in [1, 4] {
            assert_eq!(
                hashes(&dir, 48_000, k, threads),
                reference,
                "k = {k}, {threads} threads"
            );
        }
    }
    // The golden: 120 s rendered from the grouped copy give the studio's
    // golden audio and log (its project.toml's hash differs, as it should).
    let out = scratch("studio-render");
    let output = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(["render"])
        .arg(&dir)
        .args(["--frames", "5760000", "--threads", "1", "--json", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let read = |path: PathBuf| -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    };
    let manifest = read(out.join("manifest.json"));
    let golden = read(repository().join("tests/golden/studio/manifest.json"));
    for field in ["audio_fnv1a64", "log_fnv1a64", "frames"] {
        assert_eq!(manifest[field], golden[field], "{field}");
    }
    assert_ne!(
        manifest["provenance"]["project_file_hashes"]["project.toml"],
        golden["provenance"]["project_file_hashes"]["project.toml"]
    );
}

#[test]
fn a_primitive_and_a_project_include_group_with_the_same_plan() {
    let dir = scratch("primitive");
    copy_dir(&repository().join("projects/studio"), &dir);
    let before = plan_json(&dir);
    let text = group(&dir, "limiter");
    assert!(
        text.contains(r#"limiter = { slot = { inputs = { in = {} }, outputs = { out = {} }, parameters = { drive = {} } }, load = "std.tanh_shaper", drive = 1 }"#),
        "{text}"
    );
    assert_eq!(plan_json(&dir), before);

    // Two studios side by side, so `../studio` resolves as committed.
    let base = scratch("includes");
    copy_dir(&repository().join("projects/studio"), &base.join("studio"));
    copy_dir(
        &repository().join("projects/two-studios"),
        &base.join("two-studios"),
    );
    let host = base.join("two-studios");
    let before = plan_json(&host);
    let text = group(&host, "studio");
    assert!(
        text.contains(
            r#"studio = { slot = { outputs = { l = {}, r = {} } }, load = "../studio" }"#
        ),
        "{text}"
    );
    assert_eq!(plan_json(&host), before);
    let text = group(&host, "studio_transport");
    assert!(
        text.contains(r#"studio_transport = { slot = { outputs = { l = {}, r = {} } }, load = "./studio-transport" }"#),
        "{text}"
    );
    assert_eq!(plan_json(&host), before);
}
