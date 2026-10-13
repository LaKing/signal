//! The M2 test project `projects/pulse` (M2-7 AC3) and its golden render, and
//! `std.delay` closing a feedback loop without a `std.unit_delay` (AC2b). How
//! to update the golden: `docs/testing-and-evaluation.md`.
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
use signal_project::{Project, SourceFile};

/// The project's rate and seed in `projects/pulse/project.toml`.
const RATE: u32 = 96_000;
const SEED: u64 = 7;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn pulse() -> PathBuf {
    repository().join("projects/pulse")
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).expect("std names are unique");
    registry
}

/// Renders `project` in process, as `signal render` does.
fn render_project(project: &Project, seed: u64, sample_rate: u32, frames: u64) -> Render {
    let registry = registry();
    let planned = project.plan(&registry).expect("the project is valid");
    let config = EngineConfig::new(sample_rate, seed);
    let mut engine = Engine::new(&planned.plan, &registry, config, Vec::new()).unwrap();
    render(&mut engine, frames).unwrap()
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

/// Asserts that every sample, as rendered and as `audio.wav` stores it, is finite.
fn assert_every_sample_finite(rendered: &Render, at: &str) {
    for (i, &x) in rendered.samples.iter().enumerate() {
        assert!(x.is_finite(), "{at}: sample {i} is {x}");
        assert!((x as f32).is_finite(), "{at}: sample {i} is {x} as f32");
    }
}

/// AC3a: the project is at 96 kHz with seed 7 and uses the modules of the
/// patch it documents, each of which has the contract suite run on it in
/// `crates/signal-std/tests/contract.rs`.
#[test]
fn the_project_is_96_khz_seed_7_with_the_documented_patch() {
    let project = Project::load(&pulse()).unwrap();
    assert_eq!((project.sample_rate, project.seed), (RATE, SEED));
    let planned = project.plan(&registry()).unwrap();
    let used: BTreeSet<&str> = planned.plan.instances.iter().map(|i| i.type_name).collect();
    assert_eq!(
        used,
        BTreeSet::from([
            "std.adsr",
            "std.audio_out",
            "std.dc_block",
            "std.delay",
            "std.edge",
            "std.exp_env",
            "std.gain",
            "std.lag",
            "std.lfo",
            "std.mix",
            "std.onepole",
            "std.osc",
            "std.pan",
            "std.step_seq",
            "std.svf",
            "std.transport",
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
    // The feedback loop has no unit delay: the delay alone closes it.
    assert!(!used.contains("std.unit_delay"));
}

/// A sine into a mixer whose output feeds a delay, and the delay back into
/// the mixer through `std.gain`; `delay_type` closes the loop.
fn feedback_project(delay_type: &str) -> Result<Project, String> {
    let text = format!(
        "schema_version = 1\n\
         [project]\nname = \"feedback\"\nsample_rate = 48000\n\
         [instances]\n\
         osc = {{ module = \"std.sine_osc\", freq = 220 }}\n\
         mix = {{ module = \"std.mix\" }}\n\
         loop = {{ module = \"{delay_type}\" }}\n\
         fb = {{ module = \"std.gain\", db = -3 }}\n\
         out = {{ module = \"std.audio_out\" }}\n\
         [wiring]\nconnections = [\n\
           {{ from = \"osc.out\", to = \"mix.a\" }},\n\
           {{ from = \"mix.out\", to = \"loop.in\" }},\n\
           {{ from = \"loop.out\", to = \"fb.in\" }},\n\
           {{ from = \"fb.out\", to = \"mix.b\" }},\n\
           {{ from = \"mix.out\", to = \"out.l\" }},\n\
           {{ from = \"loop.out\", to = \"out.r\" }},\n\
         ]\n"
    );
    let file = SourceFile {
        path: "project.toml".to_owned(),
        text,
    };
    let project = Project::from_sources(vec![file]).map_err(|d| format!("{d:?}"))?;
    project.plan(&registry()).map_err(|d| format!("{d:?}"))?;
    Ok(project)
}

/// AC2b: a `std.delay` closes a feedback loop without a `std.unit_delay`:
/// validation accepts it and it renders, finite on every sample and bounded
/// by the loop's geometric sum. The same loop through a direct `std.gain` is
/// rejected as a cycle.
#[test]
fn a_delay_closes_a_feedback_loop_without_a_unit_delay() {
    let Err(rejected) = feedback_project("std.gain") else {
        panic!("a loop through a direct std.gain is accepted");
    };
    assert!(rejected.contains("cycle"), "{rejected}");

    let project = feedback_project("std.delay").expect("a delay closes the loop");
    let frames = 48_000 * 4;
    let rendered = render_project(&project, 0, 48_000, frames);
    assert_every_sample_finite(&rendered, "feedback");
    // |mix| ≤ 1 / (1 − 10^(−3/20)) ≈ 3.43, the sum of the repeats.
    let bound = 1.0 / (1.0 - 10f64.powf(-3.0 / 20.0));
    let peak = rendered.samples.iter().fold(0.0f64, |p, x| p.max(x.abs()));
    println!("feedback: peak {peak:.4}, bound {bound:.4}");
    assert!(peak <= bound, "peak {peak}");
    // The default 250 ms is 12 000 frames: the delayed channel is silent
    // before it and sounding after it, and the mix grows with the repeats.
    let right: Vec<f64> = rendered
        .samples
        .iter()
        .skip(1)
        .step_by(2)
        .copied()
        .collect();
    assert!(right[..12_000].iter().all(|&x| x == 0.0));
    assert!(right[12_000..13_000].iter().any(|&x| x != 0.0));
    let left_peak = |range: std::ops::Range<usize>| {
        rendered.samples[2 * range.start..2 * range.end]
            .iter()
            .step_by(2)
            .fold(0.0f64, |p, x| p.max(x.abs()))
    };
    assert!(left_peak(48_000..96_000) > 1.5 * left_peak(0..12_000));
}

/// AC3b: `projects/pulse` renders 16 bars at 44.1, 48, 96 and 192 kHz with
/// every sample finite, a 4× oversampled peak at or below −0.3 dBFS and
/// |mean| < 1e-4 on each channel.
#[test]
fn renders_at_every_rate_within_the_floors() {
    let project = Project::load(&pulse()).unwrap();
    let ceiling = 10f64.powf(-0.3 / 20.0);
    for rate in [44_100u32, 48_000, 96_000, 192_000] {
        let rendered = render_project(&project, SEED, rate, bars(16, rate));
        let at = format!("{rate} Hz");
        assert_every_sample_finite(&rendered, &at);
        let samples: Vec<f32> = rendered.samples.iter().map(|&x| x as f32).collect();
        let metrics = signal_eval::metrics(&samples, rendered.channels);
        let true_peaks = signal_eval::true_peak_4x(&samples, rendered.channels);
        for (channel, (m, &true_peak)) in metrics.channels.iter().zip(&true_peaks).enumerate() {
            println!(
                "pulse {rate} Hz channel {channel}: 4x peak {true_peak:.6} ({:.3} dBFS), mean {:e}",
                20.0 * true_peak.log10(),
                m.mean
            );
            assert_eq!(m.non_finite, 0, "{at} channel {channel}");
            assert!(
                true_peak.is_finite() && true_peak <= ceiling,
                "{at} channel {channel}: 4x peak {true_peak}"
            );
            assert!(
                m.mean.abs() < 1e-4,
                "{at} channel {channel}: mean {}",
                m.mean
            );
        }
    }
}

/// AC3c: the golden render, seed 7, 16 bars at 96 kHz. Its manifest, without
/// the fields that change with the build rather than the sound, equals
/// `tests/golden/pulse/manifest.json`.
#[test]
fn golden_pulse_16_bars() {
    let out = out_dir("pulse-golden");
    let (project, out_text, frames) = (
        pulse().display().to_string(),
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

    let golden = repository().join("tests/golden/pulse/manifest.json");
    let fresh = serde_json::to_string_pretty(&manifest).unwrap();
    let expected = std::fs::read_to_string(&golden).map(|t| serde_json::from_str::<Value>(&t));
    assert!(
        matches!(&expected, Ok(Ok(golden)) if *golden == manifest),
        "the render differs from {}; see docs/testing-and-evaluation.md before updating it. \
         Fresh manifest:\n{fresh}",
        golden.display()
    );
}

/// M2.6-2 (ADR 0019 §2): the golden render on 4 threads, every segment on
/// the workers, has the golden's audio and log hashes.
#[test]
fn golden_pulse_on_workers() {
    let golden = read_json(&repository().join("tests/golden/pulse/manifest.json"));
    let registry = registry();
    let project = Project::load(&pulse()).unwrap();
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
