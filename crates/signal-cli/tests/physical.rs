//! The M2.8-5 test project `projects/physical` (AC3) and its golden render.
//! How to update the golden: `docs/testing-and-evaluation.md`.
//!
//! Every render here checks each sample with `is_finite()` beside any peak
//! bound, because `f64::max` and a peak computed with it ignore NaN.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use signal_core::Registry;
use signal_engine::{Engine, EngineConfig};
use signal_output::{Render, render};
use signal_project::Project;

/// The project's rate and seed in `projects/physical/project.toml`.
const RATE: u32 = 48_000;
const SEED: u64 = 5;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn physical() -> PathBuf {
    repository().join("projects/physical")
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).expect("std names are unique");
    registry
}

/// The frame count of `n` 4-beat bars at 120 bpm: a beat is half a second.
fn bars(n: u64, rate: u32) -> u64 {
    2 * n * u64::from(rate)
}

fn signal(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(args)
        .output()
        .expect("the signal binary runs")
}

/// A fresh output folder for one test, inside the target directory.
fn out_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The project uses all four physical models, each of which has the contract
/// suite run on it in `crates/signal-std/tests/contract.rs`, as do the other
/// primitives of its patch.
#[test]
fn the_project_uses_the_four_physical_models() {
    let project = Project::load(&physical()).unwrap();
    assert_eq!((project.sample_rate, project.seed), (RATE, SEED));
    let planned = project.plan(&registry()).unwrap();
    let used: BTreeSet<&str> = planned.plan.instances.iter().map(|i| i.type_name).collect();
    assert_eq!(
        used,
        BTreeSet::from([
            "std.audio_out",
            "std.clock_div",
            "std.dc_block",
            "std.exciter",
            "std.mass_spring",
            "std.modal",
            "std.step_seq",
            "std.string",
            "std.transport",
            "std.unit_delay",
        ])
    );
    let contract =
        std::fs::read_to_string(repository().join("crates/signal-std/tests/contract.rs")).unwrap();
    for name in &used {
        assert!(
            contract.contains(&format!("contract_tests!(super::std_type(\"{name}\"))")),
            "{name} has no contract tests in signal-std/tests/contract.rs"
        );
    }
}

/// Four bars render at 44.1, 48 and 96 kHz with every sample finite, a 4×
/// oversampled peak at or below −1 dBFS and |mean| < 1e-3 on each channel.
#[test]
fn renders_at_every_rate_within_the_floors() {
    let project = Project::load(&physical()).unwrap();
    let registry = registry();
    let planned = project.plan(&registry).expect("the project is valid");
    let ceiling = 10f64.powf(-1.0 / 20.0);
    for rate in [44_100u32, 48_000, 96_000] {
        let config = EngineConfig::new(rate, SEED);
        let mut engine = Engine::new(&planned.plan, &registry, config, Vec::new()).unwrap();
        let rendered: Render = render(&mut engine, bars(4, rate)).unwrap();
        for (i, &x) in rendered.samples.iter().enumerate() {
            assert!(
                x.is_finite() && (x as f32).is_finite(),
                "{rate} Hz: sample {i} is {x}"
            );
        }
        let samples: Vec<f32> = rendered.samples.iter().map(|&x| x as f32).collect();
        let metrics = signal_eval::metrics(&samples, rendered.channels);
        let true_peaks = signal_eval::true_peak_4x(&samples, rendered.channels);
        for (channel, (m, &true_peak)) in metrics.channels.iter().zip(&true_peaks).enumerate() {
            println!(
                "physical {rate} Hz channel {channel}: 4x peak {true_peak:.6} ({:.3} dBFS), mean {:e}",
                20.0 * true_peak.log10(),
                m.mean
            );
            assert_eq!(m.non_finite, 0);
            assert!(
                true_peak <= ceiling,
                "{rate} Hz channel {channel}: {true_peak}"
            );
            assert!(
                m.mean.abs() < 1e-3,
                "{rate} Hz channel {channel}: {}",
                m.mean
            );
        }
    }
}

/// AC3: the golden render, seed 5, 16 bars at 48 kHz. Its manifest, without
/// the fields that change with the build rather than the sound, equals
/// `tests/golden/physical/manifest.json`.
#[test]
fn golden_physical_16_bars() {
    let out = out_dir("physical-golden");
    let (project, out_text, frames) = (
        physical().display().to_string(),
        out.display().to_string(),
        bars(16, RATE).to_string(),
    );
    let output = signal(&[
        "render", &project, "--frames", &frames, "--out", &out_text, "--json",
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let mut manifest = read_json(&out.join("manifest.json"));
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
        names(&["project_file_hashes"])
    );

    let golden = repository().join("tests/golden/physical/manifest.json");
    let fresh = serde_json::to_string_pretty(&manifest).unwrap();
    let expected = std::fs::read_to_string(&golden).map(|t| serde_json::from_str::<Value>(&t));
    assert!(
        matches!(&expected, Ok(Ok(golden)) if *golden == manifest),
        "the render differs from {}; see docs/testing-and-evaluation.md before updating it. \
         Fresh manifest:\n{fresh}",
        golden.display()
    );
}

/// The golden render on 4 threads, every segment on the workers, has the
/// golden's audio and log hashes.
#[test]
fn golden_physical_on_workers() {
    let golden = read_json(&repository().join("tests/golden/physical/manifest.json"));
    let registry = registry();
    let project = Project::load(&physical()).unwrap();
    let planned = project.plan(&registry).expect("the project is valid");
    let rate = u32::try_from(golden["sample_rate"].as_u64().unwrap()).unwrap();
    let config = EngineConfig {
        threads: 4,
        schedule: signal_engine::Schedule::FORCED,
        ..EngineConfig::new(rate, golden["seed"].as_u64().unwrap())
    };
    let mut engine = Engine::new(&planned.plan, &registry, config, Vec::new()).unwrap();
    let rendered = render(&mut engine, golden["frames"].as_u64().unwrap()).unwrap();
    assert!(engine.schedule_stats().worker_tasks > 0);
    assert_eq!(
        format!("{:016x}", rendered.audio_fnv1a64()),
        golden["audio_fnv1a64"].as_str().unwrap()
    );
    assert_eq!(
        format!("{:016x}", rendered.log_fnv1a64()),
        golden["log_fnv1a64"].as_str().unwrap()
    );
}
