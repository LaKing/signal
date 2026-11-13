//! ADR 0044 test 9, a measurement and not a gate (hard rule 6): what a
//! steal sounds like. `moog_voice` (the studio's) in a `lib.poly_voice`
//! slot plays note A (A3) and, one beat later while A still sounds, note B
//! (E4). With one voice B steals A's voice (one frame with the gate low,
//! the envelope attacking from its current level); with legato the voice
//! changes note with its gate high; on a free voice B starts from silence.
//! Reported per case: the largest frame-to-frame step of the slot's output
//! in the 10 ms after B starts, with velocities equal (0.8, 0.8), rising
//! (0.3 → 1.0) and falling (1.0 → 0.3). Printed by
//! `cargo test --release -p signal-cli --test poly_steal -- --ignored --nocapture`,
//! with WAVs written for listening under the target's temporary directory;
//! recorded in `docs/benchmarks.md` ("Polyphonic slots").

use std::path::{Path, PathBuf};

use signal_core::Registry;
use signal_engine::{Engine, EngineConfig};
use signal_output::render_blocks;
use signal_project::{Project, SourceFile};

const RATE: u32 = 48_000;
/// 120 bpm: a beat is half a second.
const BEAT: u64 = RATE as u64 / 2;

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn moog_voice() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../projects/studio/composites/moog_voice.toml"),
    )
    .unwrap()
}

/// The pattern: A from beat 0 for 2 beats at `va` (none when `None`), B
/// from beat 1 for 1 beat at `vb`.
fn pattern(va: Option<f64>, vb: f64) -> String {
    let a = va.map_or(String::new(), |v| {
        format!("  {{ key = 57, start = 0, length = 2, velocity = {v} }},\n")
    });
    format!(
        "schema_version = 1\nkind = \"notes\"\nlength = 4\nnotes = [\n{a}  {{ key = 64, start = 1, length = 1, velocity = {vb} }},\n]\n"
    )
}

fn host(voices: &str) -> String {
    format!(
        r#"schema_version = 1
[project]
name = "steal"
sample_rate = {RATE}
seed = 1
[instances]
transport = {{ module = "std.transport", bpm = 120, ppq = 96 }}
melody = {{ module = "std.note_seq", pattern = "melody", ppq = 96 }}
synth = {{ slot = "lib.poly_voice", load = "moog_voice", voices = {voices}, set = {{ glide = 0 }} }}
out = {{ module = "std.audio_out" }}
[wiring]
connections = [
  {{ from = "transport.tick", to = "melody.tick" }},
  {{ from = "melody.notes", to = "synth.notes" }},
  {{ from = "synth.out", to = "out.l" }},
  {{ from = "synth.out", to = "out.r" }},
]
"#
    )
}

/// The left channel of 2 beats of `host(voices)` playing `pattern`.
fn render(voices: &str, pattern: &str) -> Vec<f64> {
    let project = Project::from_sources(
        [
            ("project.toml", host(voices)),
            ("composites/moog_voice.toml", moog_voice()),
            ("patterns/melody.toml", pattern.to_owned()),
        ]
        .into_iter()
        .map(|(path, text)| SourceFile {
            path: path.to_owned(),
            text,
        })
        .collect(),
    )
    .unwrap_or_else(|e| panic!("{e:?}"));
    let registry = registry();
    let plan = project.plan(&registry).unwrap().plan;
    let config = EngineConfig::new(RATE, 1);
    let mut engine = Engine::new(&plan, &registry, config, Vec::new()).unwrap();
    let render = render_blocks(&mut engine, 2 * BEAT, 256, |_| Ok(None)).unwrap();
    render.samples.chunks(2).map(|c| c[0]).collect()
}

/// The largest |x[n] − x[n − 1]| in the 10 ms after B's first frame.
fn worst_step(x: &[f64]) -> f64 {
    // B's on reaches the slot on the tick at beat 1 (a frame or so later).
    let from = BEAT as usize - 8;
    let to = from + RATE as usize / 100;
    x[from..to]
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f64::max)
}

fn write_wav(path: &PathBuf, x: &[f64]) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for &v in x {
        // At a quarter of the level, for listening.
        writer.write_sample((v * 0.25) as f32).unwrap();
    }
    writer.finalize().unwrap();
}

#[test]
#[ignore = "a measurement for the owner's listening (ADR 0044 test 9), not a gate"]
fn steals_measured() {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("poly-steal");
    std::fs::create_dir_all(&out).unwrap();
    println!("case | A→B velocity | worst step in 10 ms after B");
    for (label, va, vb) in [
        ("equal", 0.8, 0.8),
        ("rising", 0.3, 1.0),
        ("falling", 1.0, 0.3),
    ] {
        let free = render("2", &pattern(None, vb));
        let stolen = render("1", &pattern(Some(va), vb));
        let legato = render("{ count = 1, legato = true }", &pattern(Some(va), vb));
        for (case, x) in [
            ("free voice", &free),
            ("steal", &stolen),
            ("legato", &legato),
        ] {
            println!("{case} | {va} → {vb} ({label}) | {:.5}", worst_step(x));
            let file = format!("{label}-{}.wav", case.replace(' ', "-"));
            write_wav(&out.join(file), x);
        }
    }
    println!("WAVs: {}", out.display());
}
