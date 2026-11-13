//! ADR 0044 test 3: one voice with legato (`voices = { count = 1, legato =
//! true }`) fed by `std.note_seq`'s `notes` plays as the sequencer's mono
//! outputs: the voice's `hz` and `gate` equal `hz` and `gate` bit for bit,
//! and its `velocity` equals `velocity` rounded to 127ths, on patterns with
//! no note shorter than a frame and no audition notes: the studio melody
//! (slides that overlap the next note) and edge cases (touching notes, a
//! note across the loop's wrap, repeated keys). Seeks, stops and live
//! pattern edits are left to `std.note_seq`'s and `std.voice_alloc`'s own
//! tests, which check each half against the rules §3 and §4 share.

use std::path::Path;

use signal_core::Registry;
use signal_engine::{Engine, EngineConfig};
use signal_output::render_blocks;
use signal_project::{Project, SourceFile};

/// A voice that shows what it is fed: each input passed through an add to
/// an output of its own.
const PROBE: &str = r#"schema_version = 1
[composite]
name = "probe"
[instances]
ph = { module = "std.add" }
pg = { module = "std.add" }
pv = { module = "std.add" }
[exports]
inputs = { hz = "ph.a", gate = "pg.a", velocity = "pv.a" }
outputs = { ohz = "ph.out", ogate = "pg.out", ovel = "pv.out" }
"#;

/// The host: the melody's mono output `port` on the left, the one-voice
/// legato slot's matching output on the right, each scaled by 2⁻¹² (exact).
fn host(port: &str, out: &str) -> String {
    format!(
        r#"schema_version = 1
[project]
name = "legato"
sample_rate = 48000
seed = 1
[instances]
transport = {{ module = "std.transport", bpm = 120, ppq = 96 }}
melody = {{ module = "std.note_seq", pattern = "melody", ppq = 96 }}
voice = {{ slot = {{ inputs = {{ notes = {{ shape = "events", kind = "trigger", voice = "alloc" }}, hz = {{ kind = "frequency", voice = "each" }}, gate = {{ kind = "gate", voice = "each" }}, velocity = {{ kind = "control", voice = "each" }} }}, outputs = {{ ohz = "generic", ogate = "generic", ovel = "generic" }} }}, load = "probe", voices = {{ count = 1, legato = true }} }}
out = {{ module = "std.audio_out" }}
[wiring]
connections = [
  {{ from = "transport.tick", to = "melody.tick" }},
  {{ from = "melody.notes", to = "voice.notes" }},
  {{ from = "melody.{port}", to = "out.l", scale = 0.000244140625 }},
  {{ from = "voice.{out}", to = "out.r", scale = 0.000244140625 }},
]
"#
    )
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

/// The left and right channels of `frames` of `project.toml` with
/// `pattern` as `patterns/melody.toml`.
fn channels(project: &str, pattern: &str, frames: u64) -> (Vec<f64>, Vec<f64>) {
    let project = Project::from_sources(
        [
            ("project.toml", project),
            ("composites/probe.toml", PROBE),
            ("patterns/melody.toml", pattern),
        ]
        .into_iter()
        .map(|(path, text)| SourceFile {
            path: path.to_owned(),
            text: text.to_owned(),
        })
        .collect(),
    )
    .unwrap_or_else(|e| panic!("{e:?}"));
    let registry = registry();
    let plan = project.plan(&registry).unwrap().plan;
    let config = EngineConfig::new(project.sample_rate, project.seed);
    let mut engine = Engine::new(&plan, &registry, config, Vec::new()).unwrap();
    let render = render_blocks(&mut engine, frames, 64, |_| Ok(None)).unwrap();
    let left = render.samples.chunks(2).map(|c| c[0]).collect();
    let right = render.samples.chunks(2).map(|c| c[1]).collect();
    (left, right)
}

fn studio_melody() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/studio/patterns/melody.toml"),
    )
    .unwrap()
}

/// Touching notes, a note across the wrap, a repeated key, a long overlap.
const EDGES: &str = r#"schema_version = 1
kind = "notes"
length = 4
notes = [
  { key = 60, start = 0, length = 0.5, velocity = 0.9 },
  { key = 62, start = 0.5, length = 0.5, velocity = 0.6 },
  { key = 62, start = 1, length = 0.25, velocity = 1 },
  { key = 64, start = 1.5, length = 1.25, velocity = 0.35 },
  { key = 67, start = 2, length = 0.5, velocity = 0.8 },
  { key = 55, start = 3.5, length = 1, velocity = 0.5 },
]
"#;

const FRAMES: u64 = 8 * 48_000;

fn first_difference(a: &[f64], b: &[f64]) -> Option<(usize, f64, f64)> {
    a.iter()
        .zip(b)
        .position(|(x, y)| x.to_bits() != y.to_bits())
        .map(|i| (i, a[i], b[i]))
}

#[test]
fn one_legato_voice_plays_the_mono_hz_and_gate() {
    for (name, pattern) in [
        ("studio melody", studio_melody()),
        ("edges", EDGES.to_owned()),
    ] {
        for (port, out) in [("hz", "ohz"), ("gate", "ogate")] {
            let (mono, voice) = channels(&host(port, out), &pattern, FRAMES);
            assert!(mono.iter().any(|x| *x != 0.0), "{name}: {port} plays");
            assert_eq!(
                first_difference(&mono, &voice),
                None,
                "{name}: the voice's {port} (frame, mono, voice)"
            );
        }
    }
}

#[test]
fn one_legato_voice_plays_the_mono_velocity_in_127ths() {
    for (name, pattern) in [
        ("studio melody", studio_melody()),
        ("edges", EDGES.to_owned()),
    ] {
        let (mono, voice) = channels(&host("velocity", "ovel"), &pattern, FRAMES);
        let scale = 4096.0;
        for (frame, (m, v)) in mono.iter().zip(&voice).enumerate() {
            let (m, v) = (m * scale, v * scale);
            let steps = v * 127.0;
            assert!(
                (steps - steps.round()).abs() < 1e-9,
                "{name}: frame {frame}: {v} is no 127th"
            );
            // The voice's velocity holds after a note ends, as the mono
            // one does; both are the note's, the voice's in 127ths.
            assert!(
                (m - v).abs() <= 0.5 / 127.0 + 1e-12,
                "{name}: frame {frame}: mono {m}, voice {v}"
            );
        }
    }
}
