//! M2.11-2 through the engine: `std.ladder` stays within the magnitude
//! bound (10⁶, `docs/semantics.md` §7) under self-oscillation, an audio-rate
//! cutoff sweep, drive 10 and a 10⁶ input, without a fault (AC6); its output
//! is bit-identical for k = 1, 64, 256 on 1, 2, 4 and 8 threads (AC12); and
//! the golden render of a saw through it (AC13). The engine projects are
//! written by the tests; the golden's is `tests/fixtures/ladder`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use signal_core::{InstanceAddress, Registry};
use signal_engine::{Engine, EngineConfig, Schedule};
use signal_output::{Render, render_blocks};
use signal_project::{Project, SourceFile};

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).expect("std names are unique");
    registry
}

fn project(text: &str) -> Project {
    Project::from_sources(vec![SourceFile {
        path: "project.toml".to_owned(),
        text: text.to_owned(),
    }])
    .unwrap()
}

/// `instances` and `connections` (TOML lines) as a project at `rate`.
fn project_text(rate: u32, instances: &str, connections: &str) -> String {
    format!(
        "schema_version = 1\n\n[project]\nname = \"ladder\"\nsample_rate = {rate}\nseed = 3\n\n\
         [instances]\n{instances}\nout = {{ module = \"std.audio_out\", gain = 0.5 }}\n\n\
         [wiring]\nconnections = [\n{connections}]\n"
    )
}

/// Renders `frames` of `project` in blocks of `k` on `threads` threads,
/// every segment on the workers when there is more than one; returns the
/// render and the engine.
fn render(project: &Project, frames: u64, k: usize, threads: usize) -> (Render, Engine) {
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
    let render = render_blocks(&mut engine, frames, k, |_| Ok(None)).unwrap();
    assert_eq!(engine.log_overflows(), 0);
    (render, engine)
}

fn faults(render: &Render) -> Vec<String> {
    String::from_utf8_lossy(&render.log)
        .lines()
        .filter(|l| l.contains("\"type\":\"fault\""))
        .map(str::to_owned)
        .collect()
}

/// AC6: at 44.1 and 192 kHz, 1 s each, the ladder at resonance 1 and
/// drive 10, its input a saw plus a constant 10⁶ and its cutoff swept by a
/// 1 kHz sine across `[1, 0.45·sr]` (an audio-rate LFO), runs without a
/// fault: the engine checks every output value for being finite and within
/// 10⁶. So does a saw alone at resonance 1, where the filter self-oscillates,
/// and a ±1 saw at drive 10 and resonance 1 with the same audio-rate sweep,
/// where the `tanh` is not held in saturation by a constant.
/// The ladder is never muted and every rendered sample is finite.
#[test]
fn stays_within_the_bound_through_the_engine() {
    let cases = [
        (
            "saw = { module = \"std.osc\", shape = 1, freq = 110 }\n\
             top = { module = \"std.const\", value = 1000000 }\n\
             lfo = { module = \"std.sine_osc\", freq = 1000 }\n\
             ladder = { module = \"std.ladder\", cutoff_hz = 20000, resonance = 1, drive = 10 }",
            "{ from = \"saw.out\", to = \"ladder.in\" },\n\
             { from = \"top.out\", to = \"ladder.in\" },\n\
             { from = \"lfo.out\", to = \"ladder.cutoff_hz\", scale = 20000 },\n",
        ),
        (
            "saw = { module = \"std.osc\", shape = 1, freq = 110 }\n\
             ladder = { module = \"std.ladder\", cutoff_hz = 1000, resonance = 1 }",
            "{ from = \"saw.out\", to = \"ladder.in\" },\n",
        ),
        (
            "saw = { module = \"std.osc\", shape = 1, freq = 110 }\n\
             lfo = { module = \"std.sine_osc\", freq = 1000 }\n\
             ladder = { module = \"std.ladder\", cutoff_hz = 20000, resonance = 1, drive = 10 }",
            "{ from = \"saw.out\", to = \"ladder.in\" },\n\
             { from = \"lfo.out\", to = \"ladder.cutoff_hz\", scale = 20000 },\n",
        ),
    ];
    let outputs = "{ from = \"ladder.lp\", to = \"out.l\" },\n\
                   { from = \"ladder.lp12\", to = \"out.r\" },\n";
    let ladder: InstanceAddress = "/ladder".parse().unwrap();
    for rate in [44_100, 192_000] {
        for (instances, connections) in cases {
            let text = project_text(rate, instances, &format!("{connections}{outputs}"));
            let (rendered, engine) = render(&project(&text), u64::from(rate), 256, 1);
            assert_eq!(faults(&rendered), Vec::<String>::new(), "{rate} Hz");
            assert_eq!(engine.is_muted(&ladder), Some(false));
            assert!(rendered.samples.iter().all(|y| y.is_finite()));
            assert!(
                rendered.samples.iter().any(|y| y.abs() > 0.05),
                "{rate} Hz: silent"
            );
        }
    }
}

/// AC12: two ladders, one self-oscillating with an audio-rate cutoff sweep,
/// one driven with an LFO sweep, render the same samples and log for k = 1,
/// 64 and 256 on 1, 2, 4 and 8 threads as one frame at a time on one thread.
#[test]
fn bit_identical_across_k_and_threads() {
    let text = project_text(
        48_000,
        "saw = { module = \"std.osc\", shape = 1, freq = 110 }\n\
         audio = { module = \"std.sine_osc\", freq = 700 }\n\
         slow = { module = \"std.lfo\", freq_hz = 3 }\n\
         a = { module = \"std.ladder\", cutoff_hz = 2000, resonance = 1 }\n\
         b = { module = \"std.ladder\", cutoff_hz = 900, resonance = 0.6, drive = 5 }",
        "{ from = \"saw.out\", to = \"a.in\" },\n\
         { from = \"saw.out\", to = \"b.in\" },\n\
         { from = \"audio.out\", to = \"a.cutoff_hz\", scale = 1500 },\n\
         { from = \"slow.out\", to = \"b.cutoff_hz\", scale = 600 },\n\
         { from = \"a.lp\", to = \"out.l\" },\n\
         { from = \"b.lp12\", to = \"out.r\" },\n",
    );
    let project = project(&text);
    let frames = 20_000;
    let (reference, _) = render(&project, frames, 1, 1);
    assert!(faults(&reference).is_empty());
    for k in [1, 64, 256] {
        for threads in [1, 2, 4, 8] {
            let (rendered, engine) = render(&project, frames, k, threads);
            assert_eq!(rendered, reference, "k = {k}, {threads} threads");
            if threads > 1 {
                assert!(engine.schedule_stats().worker_tasks > 0);
            }
        }
    }
}

fn signal(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(args)
        .output()
        .expect("the signal binary runs")
}

/// AC13: the golden render, 1 s of the fixture `tests/fixtures/ladder` (a
/// 110 Hz saw through the ladder at resonance 0.8 and drive 2, its cutoff
/// swept 200–1400 Hz at 2 Hz) at 48 kHz, seed 1. Its manifest, without the
/// fields that change with the build rather than the sound, equals
/// `tests/golden/ladder/manifest.json` (`docs/testing-and-evaluation.md`).
#[test]
fn golden_ladder_1_second() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ladder");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ladder-golden");
    let _ = std::fs::remove_dir_all(&out);
    let (fixture_text, out_text) = (fixture.display().to_string(), out.display().to_string());
    let output = signal(&[
        "render",
        &fixture_text,
        "--frames",
        "48000",
        "--out",
        &out_text,
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let text = std::fs::read_to_string(out.join("manifest.json")).unwrap();
    let mut manifest: Value = serde_json::from_str(&text).unwrap();
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

    let golden = repository().join("tests/golden/ladder/manifest.json");
    let fresh = serde_json::to_string_pretty(&manifest).unwrap();
    let expected = std::fs::read_to_string(&golden).map(|t| serde_json::from_str::<Value>(&t));
    assert!(
        matches!(&expected, Ok(Ok(golden)) if *golden == manifest),
        "the render differs from {}; see docs/testing-and-evaluation.md before updating it. \
         Fresh manifest:\n{fresh}",
        golden.display()
    );
}
