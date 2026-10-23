//! `projects/two-studios` (ADR 0035, phase 1): the studio and a copy of the
//! studio played from its transport, each included as a composite instance
//! of the whole project and mixed by the host.
//!
//! - `golden_two_studios_16_seconds`: the golden render
//!   (`tests/golden/two-studios/manifest.json`, with its included files and
//!   `included_projects`), with no fault or mute and a safe peak.
//! - ADR 0035 test 1, identity: each include's tap outputs are its own
//!   render's audio bit for bit (f32 of them is its `audio.wav`, −0
//!   included), for blocks of 1, 64 and 256 frames on 1, 2, 4 and 8
//!   threads.
//! - Test 2, straight-through: a host whose only content is one include
//!   wired into its `audio_out` plays that project's audio, equal as
//!   numbers, with sign-of-zero differences counted (0 here).
//! - ADR 0037 test 1: `moog_voice` in a `lib.voice` slot inside the copy
//!   of studio-transport plans byte-identically to the copy and renders the
//!   studio-transport golden's audio.
//!
//! Fixture hosts are written under the target's temporary directory; the
//! committed projects are only read. How to update the golden:
//! `docs/testing-and-evaluation.md`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use signal_core::{PortAddress, Registry};
use signal_engine::{Engine, EngineConfig, Schedule};
use signal_graph::ExecutionPlan;
use signal_project::Project;

/// 16 s at 48 kHz: the golden's length.
const GOLDEN_FRAMES: u64 = 16 * 48_000;

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
        .join("two-studios")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn plan_of(dir: &Path) -> (Project, ExecutionPlan) {
    let project = Project::load(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    let plan = project
        .plan(&registry())
        .unwrap_or_else(|e| panic!("{}: {e:#?}", dir.display()))
        .plan;
    (project, plan)
}

/// `frames` of `plan` in blocks of `k` on `threads` threads: every frame's
/// value of each signal output in `ports`.
fn capture(
    plan: &ExecutionPlan,
    (rate, seed): (u32, u64),
    frames: usize,
    (k, threads): (usize, usize),
    ports: &[&str],
) -> Vec<Vec<f64>> {
    let registry = registry();
    let config = EngineConfig {
        threads,
        schedule: if threads > 1 {
            Schedule::FORCED
        } else {
            Schedule::DEFAULT
        },
        ..EngineConfig::new(rate, seed)
    };
    let mut engine = Engine::new(plan, &registry, config, Vec::new()).unwrap();
    let rows: Vec<usize> = ports
        .iter()
        .map(|p| {
            let port: PortAddress = p.parse().unwrap();
            engine.signal_output_index(&port).unwrap()
        })
        .collect();
    let mut out = vec![Vec::with_capacity(frames); ports.len()];
    let mut done = 0;
    while done < frames {
        let ran = engine.run_block(k.min(frames - done));
        assert!(ran > 0);
        for f in 0..ran {
            for (values, &row) in out.iter_mut().zip(&rows) {
                values.push(engine.signal_output_at(row, f).unwrap());
            }
        }
        done += ran;
    }
    out
}

/// The audio a project records alone, per channel, as `audio.wav` stores it
/// (f32), and as numbers before that.
fn alone(dir: &Path, frames: u64) -> [Vec<f64>; 2] {
    let (project, plan) = plan_of(dir);
    let registry = registry();
    let config = EngineConfig::new(project.sample_rate, project.seed);
    let mut engine = Engine::new(&plan, &registry, config, Vec::new()).unwrap();
    let rendered = signal_output::render(&mut engine, frames).unwrap();
    let l = rendered.samples.iter().step_by(2).copied().collect();
    let r = rendered
        .samples
        .iter()
        .skip(1)
        .step_by(2)
        .copied()
        .collect();
    [l, r]
}

fn f32_bytes(values: &[f64]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|v| (*v as f32).to_le_bytes())
        .collect()
}

fn studio() -> PathBuf {
    repository().join("projects/studio")
}

fn host() -> PathBuf {
    repository().join("projects/two-studios")
}

fn copy() -> PathBuf {
    host().join("studio-transport")
}

fn faults(log: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(log)
        .lines()
        .filter(|line| line.contains("\"fault\"") || line.contains("\"mute\""))
        .map(str::to_owned)
        .collect()
}

#[test]
fn golden_two_studios_16_seconds() {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("two-studios-golden");
    let _ = fs::remove_dir_all(&out);
    let output = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args([
            "render",
            host().to_str().unwrap(),
            "--frames",
            &GOLDEN_FRAMES.to_string(),
            "--threads",
            "1",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let log = fs::read(out.join("log.jsonl")).unwrap();
    assert_eq!(faults(&log), Vec::<String>::new());
    let mut reader = hound::WavReader::open(out.join("audio.wav")).unwrap();
    let samples: Vec<f64> = reader
        .samples::<f32>()
        .map(|s| f64::from(s.unwrap()))
        .collect();
    assert_eq!(samples.len() as u64, 2 * GOLDEN_FRAMES);
    assert!(samples.iter().all(|x| x.is_finite()));
    let peak = samples.iter().fold(0.0f64, |m, x| m.max(x.abs()));
    assert!(peak <= 0.25, "peak {peak}");
    assert!(peak > 0.05, "the project plays: peak {peak}");

    let mut manifest: Value =
        serde_json::from_str(&fs::read_to_string(out.join("manifest.json")).unwrap()).unwrap();
    let fields = manifest.as_object_mut().unwrap();
    fields.remove("debug_assertions");
    let provenance = fields["provenance"].as_object_mut().unwrap();
    for volatile in [
        "signal_revision",
        "signal_dirty",
        "rustc_version",
        "cargo_lock_hash",
    ] {
        provenance.remove(volatile);
    }
    let keys = |v: &Value| -> BTreeSet<String> { v.as_object().unwrap().keys().cloned().collect() };
    let names =
        |list: &[&str]| -> BTreeSet<String> { list.iter().map(|s| s.to_string()).collect() };
    assert_eq!(
        keys(&manifest),
        names(&[
            "provenance",
            "seed",
            "sample_rate",
            "frames",
            "log_fnv1a64",
            "audio_fnv1a64"
        ]),
        "a new manifest field must be added to the golden or listed as volatile"
    );
    assert_eq!(
        keys(&manifest["provenance"]),
        names(&[
            "project_file_hashes",
            "library_composite_hashes",
            "included_projects"
        ])
    );
    let golden = repository().join("tests/golden/two-studios/manifest.json");
    let fresh = serde_json::to_string_pretty(&manifest).unwrap();
    let expected = fs::read_to_string(&golden).map(|t| serde_json::from_str::<Value>(&t));
    assert!(
        matches!(&expected, Ok(Ok(golden)) if *golden == manifest),
        "the render differs from {}; see docs/testing-and-evaluation.md before updating it. \
         Fresh manifest:\n{fresh}",
        golden.display()
    );
}

/// ADR 0035 test 1: each include's tap outputs, converted to f32, are its
/// own `audio.wav` byte for byte, on every partition and thread count. A
/// debug build checks 0.5 s of the matrix and 2 s on one partition; a
/// release build 2 s throughout.
#[test]
fn each_include_plays_its_own_audio_bit_for_bit() {
    let long = if cfg!(debug_assertions) {
        96_000
    } else {
        192_000
    };
    let short = if cfg!(debug_assertions) { 24_000 } else { long };
    let studio_alone = alone(&studio(), long as u64);
    let copy_alone = alone(&copy(), long as u64);
    let (project, plan) = plan_of(&host());
    let ports = [
        "/studio/out.l_out",
        "/studio/out.r_out",
        "/studio_transport/out.l_out",
        "/studio_transport/out.r_out",
    ];
    let expected = [
        &studio_alone[0],
        &studio_alone[1],
        &copy_alone[0],
        &copy_alone[1],
    ];
    let reference = capture(
        &plan,
        (project.sample_rate, project.seed),
        long,
        (256, 1),
        &ports,
    );
    for (port, (got, want)) in ports.iter().zip(reference.iter().zip(expected)) {
        assert_eq!(f32_bytes(got), f32_bytes(want), "{port}");
        let bits: Vec<u64> = got.iter().map(|v| v.to_bits()).collect();
        let want: Vec<u64> = want.iter().map(|v| v.to_bits()).collect();
        assert_eq!(bits, want, "{port}: as numbers, −0 included");
    }
    for k in [1, 64, 256] {
        for threads in [1, 2, 4, 8] {
            let run = capture(
                &plan,
                (project.sample_rate, project.seed),
                short,
                (k, threads),
                &ports,
            );
            for (port, (got, want)) in ports.iter().zip(run.iter().zip(expected)) {
                assert_eq!(
                    f32_bytes(got),
                    f32_bytes(&want[..short]),
                    "{port}: k = {k}, threads = {threads}"
                );
            }
        }
    }
    assert!(studio_alone[0].iter().any(|x| *x != 0.0) && copy_alone[0].iter().any(|x| *x != 0.0));
}

/// ADR 0035 test 2: a host whose only content is one include wired straight
/// into its `audio_out` (scale 1, gain 1) plays the included project's
/// audio, equal as numbers; a −0 of the include reads +0 in the host's sum,
/// and those are counted (none in either project).
#[test]
fn a_straight_through_host_plays_the_include() {
    let frames = if cfg!(debug_assertions) {
        48_000
    } else {
        192_000
    };
    for (name, project) in [("studio", studio()), ("studio-transport", copy())] {
        let dir = scratch(&format!("straight-{name}"));
        let host = format!(
            "schema_version = 1\n[project]\nname = \"straight\"\nsample_rate = 48000\n[instances]\ninner = {{ project = \"{}\" }}\nout = {{ module = \"std.audio_out\" }}\n[wiring]\nconnections = [\n  {{ from = \"inner.l\", to = \"out.l\" }},\n  {{ from = \"inner.r\", to = \"out.r\" }},\n]\n",
            project.display()
        );
        fs::write(dir.join("project.toml"), host).unwrap();
        let own = alone(&project, frames);
        let hosted = alone(&dir, frames);
        let mut zero_signs = 0;
        for channel in 0..2 {
            for (a, b) in own[channel].iter().zip(&hosted[channel]) {
                assert_eq!(a, b, "{name}");
                if a.to_bits() != b.to_bits() {
                    zero_signs += 1;
                }
            }
        }
        assert_eq!(zero_signs, 0, "{name}: sign-of-zero differences");
    }
}

/// ADR 0037 test 1: `moog_voice` in a `lib.voice` slot inside the copy of
/// studio-transport plans byte-identically to the copy, which renders the
/// studio-transport golden's audio (its whole length in a release build).
#[test]
fn a_voice_slot_in_studio_transport_is_the_golden() {
    let dir = scratch("voice-slot");
    let source = copy();
    for folder in ["composites", "patterns"] {
        fs::create_dir_all(dir.join(folder)).unwrap();
        for entry in fs::read_dir(source.join(folder)).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), dir.join(folder).join(entry.file_name())).unwrap();
        }
    }
    let text = fs::read_to_string(source.join("project.toml")).unwrap();
    let slotted = text.replace(
        "synth = { module = \"moog_voice\" }",
        "synth = { slot = \"lib.voice\", load = \"moog_voice\" }",
    );
    assert_ne!(slotted, text);
    fs::write(dir.join("project.toml"), slotted).unwrap();
    let (_, explicit) = plan_of(&source);
    let (project, slot) = plan_of(&dir);
    assert_eq!(
        serde_json::to_string(&slot).unwrap(),
        serde_json::to_string(&explicit).unwrap()
    );
    let frames = if cfg!(debug_assertions) {
        96_000
    } else {
        GOLDEN_FRAMES
    };
    let registry = registry();
    let config = EngineConfig::new(project.sample_rate, project.seed);
    let mut engine = Engine::new(&slot, &registry, config, Vec::new()).unwrap();
    let rendered = signal_output::render(&mut engine, frames).unwrap();
    if frames == GOLDEN_FRAMES {
        let golden: Value = serde_json::from_str(
            &fs::read_to_string(repository().join("tests/golden/studio-transport/manifest.json"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            format!("{:016x}", rendered.audio_fnv1a64()),
            golden["audio_fnv1a64"].as_str().unwrap()
        );
    } else {
        // A debug build renders 2 s (the golden's 16 s take minutes there)
        // and compares both channels with the copy's own render as
        // `audio.wav` stores it; the plans are byte-identical above, so the
        // release build's golden hash follows for the whole length.
        let [l, r] = alone(&source, frames);
        let left: Vec<f64> = rendered.samples.iter().step_by(2).copied().collect();
        let right: Vec<f64> = rendered
            .samples
            .iter()
            .skip(1)
            .step_by(2)
            .copied()
            .collect();
        assert_eq!(f32_bytes(&left), f32_bytes(&l));
        assert_eq!(f32_bytes(&right), f32_bytes(&r));
        assert!(l.iter().any(|x| *x != 0.0), "it plays");
    }
}

/// ADR 0037 §3: `signal modules --interfaces` lists the library templates
/// with their pinned text hashes.
#[test]
fn modules_lists_the_library_interfaces() {
    let output = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(["--json", "modules", "--interfaces"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let listed: Value = serde_json::from_slice(&output.stdout).unwrap();
    let names: Vec<(&str, &str)> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap(), t["hash"].as_str().unwrap()))
        .collect();
    let expected: Vec<(String, String)> = signal_lib_interfaces();
    let expected: Vec<(&str, &str)> = expected
        .iter()
        .map(|(n, h)| (n.as_str(), h.as_str()))
        .collect();
    assert_eq!(names, expected);
}

/// `lib.<name>` and text hash of every library template, as pinned (the
/// `[interfaces]` rows of `crates/signal-lib/tests/pinned.toml`).
fn signal_lib_interfaces() -> Vec<(String, String)> {
    let text =
        fs::read_to_string(repository().join("crates/signal-lib/tests/pinned.toml")).unwrap();
    let section = text.split("[interfaces]").nth(1).unwrap();
    section
        .lines()
        .filter_map(|line| {
            let (name, hash) = line.split_once(" = ")?;
            Some((format!("lib.{name}"), hash.trim_matches('"').to_owned()))
        })
        .collect()
}
