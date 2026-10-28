//! `projects/two-decks` (ADR 0037, phase 2): two `lib.deck` slots, one
//! loading `./deck` (a copy of studio-transport whose transport exports the
//! playhead's follow inputs) and following the host's transport, the other
//! loading `../studio` on its own clock, mixed by the host.
//!
//! - `golden_two_decks_16_seconds`: the golden render
//!   (`tests/golden/two-decks/manifest.json`), with no fault or mute and a
//!   safe peak.
//! - ADR 0037 test 6: a deck following a host at another tempo plays what
//!   the deck plays alone at that tempo, bit for bit (every hit where a
//!   local follower's would be), on every partition and thread count; the
//!   unfollowed deck B plays the studio's own audio; the deck alone (its
//!   follow inputs unwired) plays the studio-transport copy's audio, so the
//!   vendored transport with its follow inputs changes nothing unconnected.
//! - ADR 0035 test 10: the slot plans byte-identically to the explicit
//!   include with the renamed wires.
//! - ADR 0037 test 8: `signal ui check` passes on two-decks.
//! - A live Replace of deck B's content through `send_edit` on the null
//!   driver: unloaded, then loaded a frame later.
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
use signal_engine::{Engine, EngineConfig, RealtimeConfig, Schedule};
use signal_graph::ExecutionPlan;
use signal_output::{Driver, NullDriver, realtime};
use signal_project::live::send_edit;
use signal_project::session::{LiveRecorder, SessionRecorder};
use signal_project::{Project, provenance};

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

fn host() -> PathBuf {
    repository().join("projects/two-decks")
}

fn deck() -> PathBuf {
    host().join("deck")
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("two-decks")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Copies the project folder `from` (its files and subfolders) to `to`.
fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
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
            engine
                .signal_output_index(&port)
                .unwrap_or_else(|| panic!("{p}"))
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

/// The audio a project records alone, per channel, before the conversion
/// to f32.
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

fn bits(values: &[f64]) -> Vec<u64> {
    values.iter().map(|v| v.to_bits()).collect()
}

fn faults(log: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(log)
        .lines()
        .filter(|line| line.contains("\"fault\"") || line.contains("\"mute\""))
        .map(str::to_owned)
        .collect()
}

#[test]
fn golden_two_decks_16_seconds() {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("two-decks-golden");
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
            "library_interface_hashes",
            "included_projects"
        ])
    );
    let golden = repository().join("tests/golden/two-decks/manifest.json");
    let fresh = serde_json::to_string_pretty(&manifest).unwrap();
    let expected = fs::read_to_string(&golden).map(|t| serde_json::from_str::<Value>(&t));
    assert!(
        matches!(&expected, Ok(Ok(golden)) if *golden == manifest),
        "the render differs from {}; see docs/testing-and-evaluation.md before updating it. \
         Fresh manifest:\n{fresh}",
        golden.display()
    );
}

/// A host at `bpm` following into deck A (as two-decks, but at that
/// tempo), and the deck alone with its own transport at `bpm`.
fn at_tempo(bpm: u32) -> (PathBuf, PathBuf) {
    let dir = scratch(&format!("tempo-{bpm}"));
    let host_text = fs::read_to_string(host().join("project.toml"))
        .unwrap()
        .replace(
            "transport = { module = \"lib.transport\", bpm = 120,",
            &format!("transport = {{ module = \"lib.transport\", bpm = {bpm},"),
        )
        .replace(
            "load = \"../studio\"",
            &format!(
                "load = \"{}\"",
                repository().join("projects/studio").display()
            ),
        );
    assert!(host_text.contains(&format!("bpm = {bpm}")));
    fs::write(dir.join("project.toml"), host_text).unwrap();
    copy_dir(&deck(), &dir.join("deck"));
    let alone_dir = dir.join("alone");
    copy_dir(&deck(), &alone_dir);
    let deck_text = fs::read_to_string(alone_dir.join("project.toml"))
        .unwrap()
        .replace(
            "transport = { module = \"transport\", bpm = 120,",
            &format!("transport = {{ module = \"transport\", bpm = {bpm},"),
        );
    assert!(deck_text.contains(&format!("bpm = {bpm}")));
    fs::write(alone_dir.join("project.toml"), deck_text).unwrap();
    (dir, alone_dir)
}

/// ADR 0037 test 6: deck A following a host at 126 bpm plays what the deck
/// plays alone at 126 bpm, bit for bit: every hit of its sequencers lands
/// where a local follower's does, on every partition and thread count.
#[test]
fn a_following_deck_plays_like_a_local_follower() {
    let long = if cfg!(debug_assertions) {
        96_000
    } else {
        192_000
    };
    let short = if cfg!(debug_assertions) { 24_000 } else { long };
    let (dir, alone_dir) = at_tempo(126);
    let own = alone(&alone_dir, long as u64);
    let (project, plan) = plan_of(&dir);
    let ports = ["/deck_a/out.l_out", "/deck_a/out.r_out"];
    let reference = capture(
        &plan,
        (project.sample_rate, project.seed),
        long,
        (256, 1),
        &ports,
    );
    for (port, (got, want)) in ports.iter().zip(reference.iter().zip(&own)) {
        assert_eq!(bits(got), bits(want), "{port}");
    }
    assert!(own[0].iter().any(|x| *x != 0.0), "it plays");
    for k in [1, 64, 256] {
        for threads in [1, 2, 4] {
            let run = capture(
                &plan,
                (project.sample_rate, project.seed),
                short,
                (k, threads),
                &ports,
            );
            for (port, (got, want)) in ports.iter().zip(run.iter().zip(&own)) {
                assert_eq!(
                    bits(got),
                    bits(&want[..short]),
                    "{port}: k = {k}, threads = {threads}"
                );
            }
        }
    }
}

/// Unfollowed decks equal themselves alone: deck B is the studio's own
/// audio; the deck alone (its follow inputs unwired, its vendored transport
/// with the follow inputs) is the studio-transport copy's audio.
#[test]
fn unfollowed_decks_play_their_own_audio() {
    let frames = if cfg!(debug_assertions) {
        48_000
    } else {
        192_000
    };
    let studio = alone(&repository().join("projects/studio"), frames as u64);
    let (project, plan) = plan_of(&host());
    let b = capture(
        &plan,
        (project.sample_rate, project.seed),
        frames,
        (256, 1),
        &["/deck_b/out.l_out", "/deck_b/out.r_out"],
    );
    assert_eq!(bits(&b[0]), bits(&studio[0]));
    assert_eq!(bits(&b[1]), bits(&studio[1]));
    let deck_alone = alone(&deck(), frames as u64);
    let copy_alone = alone(
        &repository().join("projects/two-studios/studio-transport"),
        frames as u64,
    );
    assert_eq!(bits(&deck_alone[0]), bits(&copy_alone[0]));
    assert_eq!(bits(&deck_alone[1]), bits(&copy_alone[1]));
}

/// ADR 0035 test 10: deck A in its `lib.deck` slot plans byte-identically
/// to the explicit include with the renamed wires.
#[test]
fn the_slot_plans_as_the_explicit_include() {
    let dir = scratch("explicit");
    copy_dir(&deck(), &dir.join("deck"));
    let text = fs::read_to_string(host().join("project.toml")).unwrap();
    let explicit = text
        .replace(
            "deck_a = { slot = \"lib.deck\", load = \"./deck\", release = 0 }",
            "deck_a = { project = \"./deck\", release = 0 }",
        )
        .replace(
            "deck_b = { slot = \"lib.deck\", load = \"../studio\" }",
            &format!(
                "deck_b = {{ project = \"{}\" }}",
                repository().join("projects/studio").display()
            ),
        );
    assert_ne!(explicit, text);
    fs::write(dir.join("project.toml"), explicit).unwrap();
    let (_, slotted) = plan_of(&host());
    let (_, plain) = plan_of(&dir);
    assert_eq!(
        serde_json::to_string(&slotted).unwrap(),
        serde_json::to_string(&plain).unwrap()
    );
}

/// ADR 0037 test 8: the two-decks panel's bindings check (slot cards, the
/// slot's interface parameter, scopes through the slots' matches).
#[test]
fn ui_check_passes_on_two_decks() {
    let output = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(["ui", "check", host().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(said.contains("bindings: ok"), "{said}");
}

/// A live Replace on two-decks (null driver): deck B's `../studio` replaced
/// by `./deck` is an unload (deck B's instances suspended, then swapped
/// out after the fade) and a load a frame later.
#[test]
fn replacing_a_deck_is_an_unload_then_a_load() {
    let dir = scratch("replace");
    copy_dir(&host(), &dir);
    // `../studio` from the scratch folder: the committed studio, by path.
    let text = fs::read_to_string(dir.join("project.toml"))
        .unwrap()
        .replace(
            "load = \"../studio\"",
            &format!(
                "load = \"{}\"",
                repository().join("projects/studio").display()
            ),
        );
    fs::write(dir.join("project.toml"), &text).unwrap();
    let registry = registry();
    let old = Project::load(&dir).unwrap();
    let mut plan = old.plan(&registry).unwrap().plan;
    plan.version = 0;
    let config = RealtimeConfig::with_buffer(48_000, old.seed, 1024);
    let (runner, mut controller) = realtime(&plan, &registry, config).unwrap();
    let provenance = provenance::collect(&provenance::source_dir(), &old.files);
    let recorder = SessionRecorder::new(&old, &registry, 48_000, old.seed, provenance).unwrap();
    let mut live = LiveRecorder::new(recorder, registry.clone());
    let mut driver = NullDriver::new(48_000, 2, 3);
    driver.start(runner).unwrap();
    driver.run(4_096, || controller.poll(&mut live)).unwrap();

    let replaced = text.replace(
        &format!(
            "load = \"{}\"",
            repository().join("projects/studio").display()
        ),
        "load = \"./deck\"",
    );
    fs::write(dir.join("project.toml"), replaced).unwrap();
    let new = Project::load(&dir).unwrap();
    let (_, applied) = send_edit(&mut controller, &mut live, &old, new, &registry).unwrap();
    assert_eq!(applied.replaced, 1, "{applied}");
    assert_eq!((applied.swap, applied.load_swap), (Some(1), Some(2)));
    assert!(applied.removed > 10, "deck B's instances: {applied}");
    while controller.in_flight() > 0 {
        driver.run(1_024, || controller.poll(&mut live)).unwrap();
    }
    driver.run(1_024, || controller.poll(&mut live)).unwrap();
    let runner = driver.stop().unwrap().unwrap();
    controller.poll(&mut live);
    assert_eq!(runner.status().version, 2);
    assert_eq!(runner.status().late, 0);
    assert_eq!(live.refused(), &[] as &[String]);
}
