//! The M2.11 studio demo `projects/studio` (M2.11-4): its patch and patterns,
//! its golden render (with no fault, no mute and a safe peak), the same hashes
//! in blocks of 1, 64 and 256 frames on one and four threads, the tape echo's
//! bound and decay at every setting, the kick's accent, the hat's choke, a
//! live pattern edit, and its panel. How to update the golden:
//! `docs/testing-and-evaluation.md`. Every variant is built in memory or in a
//! temporary folder; the committed project is only read.
//!
//! Every render here checks each sample with `is_finite()` beside any peak
//! bound, because `f64::max` and a peak computed with it ignore NaN.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use signal_core::{EventPayload, Frame, Registry};
use signal_engine::{Engine, EngineConfig, ExternalEvent, RealtimeConfig, Schedule};
use signal_output::{Driver, NullDriver, Render, realtime, render_blocks};
use signal_project::live::reload;
use signal_project::session::{LiveRecorder, Session, SessionRecorder, replay};
use signal_project::{Project, SourceFile, provenance};

/// The project's rate and seed in `projects/studio/project.toml`.
const RATE: u32 = 48_000;
const SEED: u64 = 11;
/// 120 s at 48 kHz: the golden's length.
const GOLDEN_FRAMES: u64 = 120 * 48_000;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn studio() -> PathBuf {
    repository().join("projects/studio")
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).expect("std names are unique");
    registry
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
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// The project's files that `signal render` loads, relative to its folder.
fn project_files() -> Vec<String> {
    let mut files = vec!["project.toml".to_owned()];
    for dir in ["composites", "patterns"] {
        let mut names: Vec<String> = fs::read_dir(studio().join(dir))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.ends_with(".toml"))
            .collect();
        names.sort();
        files.extend(names.into_iter().map(|n| format!("{dir}/{n}")));
    }
    files
}

/// The studio's sources with `edits` applied: (file, from, to), each `from`
/// present exactly once.
fn sources(edits: &[(&str, &str, &str)]) -> Vec<SourceFile> {
    project_files()
        .into_iter()
        .map(|path| {
            let mut text = fs::read_to_string(studio().join(&path)).unwrap();
            for &(file, from, to) in edits {
                if file == path {
                    assert_eq!(text.matches(from).count(), 1, "{path}: {from}");
                    text = text.replace(from, to);
                }
            }
            SourceFile { path, text }
        })
        .collect()
}

fn project(edits: &[(&str, &str, &str)]) -> Project {
    Project::from_sources(sources(edits)).unwrap()
}

/// A fixture project: `project.toml` is `text`, beside the studio composite
/// `composite` and the pattern `drums` (`kind = "steps"`) if given.
fn fixture(text: &str, composite: &str, drums: Option<&str>) -> Project {
    fixture_with(text, composite, drums, false)
}

/// [`fixture`], with the composite's export ranges (ADR 0042) removed when
/// `unranged`: so a value beyond them reaches the composite, as a connected
/// signal would, and its own clamps are what bound it.
fn fixture_with(text: &str, composite: &str, drums: Option<&str>, unranged: bool) -> Project {
    let mut source =
        fs::read_to_string(studio().join(format!("composites/{composite}.toml"))).unwrap();
    if unranged {
        source = without_ranges(&source);
    }
    let mut files = vec![
        SourceFile {
            path: "project.toml".to_owned(),
            text: text.to_owned(),
        },
        SourceFile {
            path: format!("composites/{composite}.toml"),
            text: source,
        },
    ];
    if let Some(drums) = drums {
        files.push(SourceFile {
            path: "patterns/drums.toml".to_owned(),
            text: drums.to_owned(),
        });
    }
    Project::from_sources(files).unwrap()
}

/// A composite's text with each `name = { to = <targets>, min = …, max = … }`
/// export written `name = <targets>`.
fn without_ranges(text: &str) -> String {
    text.lines()
        .map(
            |line| match (line.find("= { to = "), line.find(", min = ")) {
                (Some(open), Some(min)) if line.trim_end().ends_with('}') => {
                    format!(
                        "{}= {}",
                        &line[..open],
                        &line[open + "= { to = ".len()..min]
                    )
                }
                _ => line.to_owned(),
            },
        )
        .collect::<Vec<_>>()
        .join("\n")
}

/// Renders `frames` of `project` in blocks of `k` on `threads` threads, with
/// `external` events, swapping to `next` at its frame if given.
fn render(
    project: &Project,
    next: Option<(&Project, u64)>,
    frames: u64,
    (k, threads): (usize, usize),
    external: Vec<ExternalEvent>,
) -> Render {
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
    let mut engine = Engine::new(&plan, &registry, config, external).unwrap();
    let mut swap = next.map(|(project, at)| {
        let mut plan = project.plan(&registry).unwrap().plan;
        plan.version = 1;
        (engine.prepare_swap(&plan, &registry).unwrap(), at)
    });
    let rendered = render_blocks(&mut engine, frames, k, |engine| match &mut swap {
        Some((prepared, at)) if !prepared.is_committed() => {
            if engine.next_frame().0 == *at {
                engine.commit_swap(prepared, Frame(*at))?;
                Ok(None)
            } else {
                Ok(Some(Frame(*at)))
            }
        }
        _ => Ok(None),
    })
    .unwrap();
    assert_eq!(engine.log_overflows(), 0);
    rendered
}

/// The log's fault and mute records.
fn faults(log: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(log)
        .lines()
        .filter(|l| l.contains("\"type\":\"fault\"") || l.contains("\"type\":\"mute\""))
        .map(str::to_owned)
        .collect()
}

/// The frames of the triggers delivered to `target`.
fn triggers(render: &Render, target: &str) -> Vec<u64> {
    String::from_utf8_lossy(&render.log)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|v| v["type"] == "event" && v["target"] == target && v["payload"] == "trigger")
        .map(|v| v["frame"].as_u64().unwrap())
        .collect()
}

fn assert_every_sample_finite(samples: &[f64], at: &str) {
    for (i, &x) in samples.iter().enumerate() {
        assert!(x.is_finite(), "{at}: sample {i} is {x}");
    }
}

/// One channel of an interleaved stereo render.
fn channel(render: &Render, c: usize) -> Vec<f64> {
    render.samples.iter().skip(c).step_by(2).copied().collect()
}

fn peak(x: &[f64]) -> f64 {
    x.iter().fold(0.0f64, |p, x| p.max(x.abs()))
}

fn rms(x: &[f64]) -> f64 {
    (x.iter().map(|x| x * x).sum::<f64>() / x.len() as f64).sqrt()
}

/// The frame of tick `g` at 120 bpm and 96 ppq from frame 0, at 48 kHz:
/// 250 frames a tick.
fn tick(g: u64) -> u64 {
    250 * g
}

/// AC1, AC2, AC4, AC5: the transport, sequencers, patterns and voices as
/// `project.toml` documents them; every primitive in it has the contract
/// suite.
#[test]
fn the_project_is_wired_as_documented() {
    let project = Project::load(&studio()).unwrap();
    assert_eq!((project.sample_rate, project.seed), (RATE, SEED));
    let planned = project.plan(&registry()).unwrap();
    let instance = |address: &str| {
        planned
            .plan
            .instances
            .iter()
            .find(|i| i.address.to_string() == address)
            .unwrap_or_else(|| panic!("{address}"))
    };
    let parameter = |address: &str, name: &str| instance(address).parameters[name];
    assert_eq!(instance("/transport").type_name, "std.transport");
    assert_eq!(parameter("/transport", "bpm"), 120.0);
    assert_eq!(parameter("/transport", "ppq"), 96.0);
    assert_eq!(instance("/melody").type_name, "std.note_seq");
    assert_eq!(parameter("/melody", "ppq"), 96.0);
    assert_eq!(instance("/drums").type_name, "std.step_seq8");
    assert_eq!(parameter("/drums", "ppq"), 96.0);
    assert_eq!(parameter("/out", "gain"), 0.25);
    let transports = planned
        .plan
        .instances
        .iter()
        .filter(|i| i.type_name == "std.transport")
        .count();
    assert_eq!(transports, 1);

    // The voice: three oscillators, noise, a mixer, the ladder, two
    // envelopes and the glide.
    let types_in = |prefix: &str| -> Vec<&str> {
        planned
            .plan
            .instances
            .iter()
            .filter(|i| i.address.to_string().starts_with(prefix))
            .map(|i| i.type_name)
            .collect()
    };
    let count = |list: &[&str], name: &str| list.iter().filter(|&&t| t == name).count();
    let synth = types_in("/synth/");
    for (name, n) in [
        ("std.osc", 3),
        ("std.noise", 1),
        ("std.mix", 1),
        ("std.ladder", 1),
        ("std.adsr", 2),
        ("std.lag", 1),
    ] {
        assert_eq!(count(&synth, name), n, "/synth: {name}");
    }
    let echo = types_in("/echo/");
    for name in [
        "std.delay",
        "std.lfo",
        "std.noise",
        "std.tanh_shaper",
        "std.onepole",
    ] {
        assert!(echo.contains(&name), "/echo: {name}");
    }
    assert_eq!(parameter("/echo/delay", "time_ms"), 375.0);
    assert_eq!(parameter("/echo/wow_lfo", "freq_hz"), 0.5);
    let kick = types_in("/kick/");
    for name in [
        "std.sine_osc",
        "std.exp_env",
        "std.noise",
        "std.tanh_shaper",
    ] {
        assert!(kick.contains(&name), "/kick: {name}");
    }
    // About 230 Hz falling to 50 Hz: 50 + 180·pitch_env.
    assert_eq!(parameter("/kick/osc", "freq"), 50.0);
    let kick_text = fs::read_to_string(studio().join("composites/kick909.toml")).unwrap();
    assert!(kick_text.contains(r#"{ from = "pitch_env.out", to = "osc.freq", scale = 180 }"#));
    for voice in ["/snare/", "/clap/", "/hats/", "/tom/", "/rim/", "/cymbal/"] {
        assert!(!types_in(voice).is_empty(), "{voice}");
    }

    // The patterns: a 4-bar line in A minor, one bar of 16 sixteenths in
    // eight lanes.
    let melody = fs::read_to_string(studio().join("patterns/melody.toml")).unwrap();
    assert!(melody.contains("kind = \"notes\"") && melody.contains("length = 16\n"));
    let keys: Vec<u32> = melody
        .lines()
        .filter_map(|l| l.trim().strip_prefix("{ key = "))
        .map(|l| l.split(',').next().unwrap().parse().unwrap())
        .collect();
    assert!(keys.len() >= 16);
    let a_minor = [9, 11, 0, 2, 4, 5, 7];
    assert!(keys.iter().all(|k| a_minor.contains(&(k % 12))), "{keys:?}");
    let drums = fs::read_to_string(studio().join("patterns/drums.toml")).unwrap();
    assert!(drums.contains("kind = \"steps\"") && drums.contains("steps = 16\nstep = 0.25\n"));
    let lanes: Vec<(&str, &str)> = drums
        .split("[lanes]")
        .nth(1)
        .unwrap()
        .split("[levels]")
        .next()
        .unwrap()
        .lines()
        .filter_map(|l| l.split_once(" = "))
        .map(|(name, cells)| (name, cells.trim_matches('"')))
        .collect();
    let names: Vec<&str> = lanes.iter().map(|l| l.0).collect();
    assert_eq!(
        names,
        ["kick", "snare", "clap", "chh", "ohh", "ltom", "rim", "cym"]
    );
    assert!(lanes.iter().all(|l| l.1.len() == 16));
    // Four on the floor, with accents.
    let kick_lane = lanes[0].1.as_bytes();
    assert!((0..16).step_by(4).all(|i| kick_lane[i] != b'.'));
    assert!(lanes.iter().any(|l| l.1.contains('X')));

    let used: BTreeSet<&str> = planned.plan.instances.iter().map(|i| i.type_name).collect();
    let contract =
        fs::read_to_string(repository().join("crates/signal-std/tests/contract.rs")).unwrap();
    for name in &used {
        assert!(
            contract.contains(&format!("contract_tests!(super::std_type(\"{name}\"))")),
            "{name} has no contract tests in signal-std/tests/contract.rs"
        );
    }
}

/// AC11, AC6, AC9: the golden render, seed 11, 120 s at 48 kHz. Its
/// manifest, without the fields that change with the build rather than the
/// sound, equals `tests/golden/studio/manifest.json`; on the same render the
/// log has no fault or mute, every sample is finite and the peak is below
/// −1 dBFS (the output stage caps it at 0.25, −12 dBFS). One thread: the
/// output is the same on any number.
#[test]
fn golden_studio_120_seconds() {
    let out = out_dir("studio-golden");
    let (project, out_text, frames) = (
        studio().display().to_string(),
        out.display().to_string(),
        GOLDEN_FRAMES.to_string(),
    );
    let output = signal(&[
        "render",
        &project,
        "--frames",
        &frames,
        "--threads",
        "1",
        "--out",
        &out_text,
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let log = fs::read(out.join("log.jsonl")).unwrap();
    assert_eq!(faults(&log), Vec::<String>::new());
    let mut reader = hound::WavReader::open(out.join("audio.wav")).unwrap();
    let samples: Vec<f64> = reader
        .samples::<f32>()
        .map(|s| f64::from(s.unwrap()))
        .collect();
    assert_eq!(samples.len() as u64, 2 * GOLDEN_FRAMES);
    assert_every_sample_finite(&samples, "golden");
    let peak = peak(&samples);
    println!("studio: peak {peak:.4} ({:.2} dBFS)", 20.0 * peak.log10());
    assert!(peak < 10f64.powf(-1.0 / 20.0), "peak {peak}");
    assert!(peak <= 0.25, "peak {peak}");
    assert!(peak > 0.05, "the project plays: peak {peak}");

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

    let golden = repository().join("tests/golden/studio/manifest.json");
    let fresh = serde_json::to_string_pretty(&manifest).unwrap();
    let expected = fs::read_to_string(&golden).map(|t| serde_json::from_str::<Value>(&t));
    assert!(
        matches!(&expected, Ok(Ok(golden)) if *golden == manifest),
        "the render differs from {}; see docs/testing-and-evaluation.md before updating it. \
         Fresh manifest:\n{fresh}",
        golden.display()
    );
}

/// AC10: 2 bars of the project in blocks of 1, 64 and 256 frames, on one
/// thread and on four (every segment on the workers), have the same audio
/// and log hashes.
#[test]
fn renders_identically_in_blocks_and_on_threads() {
    let project = Project::load(&studio()).unwrap();
    let frames = 4 * u64::from(RATE);
    let hashes = |r: &Render| (r.audio_fnv1a64(), r.log_fnv1a64());
    let reference = render(&project, None, frames, (1, 1), Vec::new());
    assert_eq!(faults(&reference.log), Vec::<String>::new());
    assert!(peak(&reference.samples) > 0.05);
    for k in [1, 64, 256] {
        for threads in [1, 4] {
            let rendered = render(&project, None, frames, (k, threads), Vec::new());
            assert_eq!(
                hashes(&rendered),
                hashes(&reference),
                "k = {k}, {threads} threads"
            );
        }
    }
}

/// A project of the tape echo alone: its input is a 440 Hz sine burst (peak
/// 1, a 20 ms decay) on every beat at 120 bpm, or once on `/burst.trigger`
/// with `once`; the echo on the left channel, the input on the right.
fn echo_fixture(parameters: &str, once: bool) -> Project {
    let beat = if once {
        ""
    } else {
        "  { from = \"transport.beat\", to = \"burst.trigger\" },\n"
    };
    let text = format!(
        "schema_version = 1\n\
         [project]\nname = \"echo\"\nsample_rate = 48000\nseed = 3\n\
         [instances]\n\
         transport = {{ module = \"std.transport\", bpm = 120 }}\n\
         tone = {{ module = \"std.sine_osc\", freq = 440 }}\n\
         burst = {{ module = \"std.exp_env\", attack_ms = 0, decay_ms = 20 }}\n\
         input = {{ module = \"std.mul\" }}\n\
         echo = {{ module = \"tape_delay\"{parameters} }}\n\
         out = {{ module = \"std.audio_out\" }}\n\
         [wiring]\nconnections = [\n{beat}\
           {{ from = \"tone.out\", to = \"input.a\" }},\n\
           {{ from = \"burst.out\", to = \"input.b\" }},\n\
           {{ from = \"input.out\", to = \"echo.in\" }},\n\
           {{ from = \"echo.out\", to = \"out.l\" }},\n\
           {{ from = \"input.out\", to = \"out.r\" }},\n\
         ]\n"
    );
    // Its corners lie beyond the exports' ranges: the inner clamps are tested.
    fixture_with(&text, "tape_delay", None, true)
}

/// ADR 0042: the ranged composites refuse a value beyond an export's range
/// (here the echo's `feedback`, [0, 0.75]); the tests below remove the
/// ranges to reach the composites' own clamps.
#[test]
fn the_ranged_echo_refuses_a_feedback_beyond_its_range() {
    let text = "schema_version = 1\n[project]\nname = \"echo\"\n[instances]\necho = { module = \"tape_delay\", feedback = 1000000 }\nout = { module = \"std.audio_out\" }\n[wiring]\nconnections = [{ from = \"echo.out\", to = \"out.l\" }]\n";
    let errors = fixture(text, "tape_delay", None)
        .plan(&registry())
        .err()
        .unwrap();
    assert!(
        errors.iter().any(|e| e
            .to_string()
            .contains("/echo.feedback: value 1000000 is outside [0, 0.75]")),
        "{errors:?}"
    );
}

/// AC3: at corner values of every exported parameter, alone and together,
/// the echo never faults, every sample is finite and its output stays within
/// 16 times the input's peak, the bound `composites/tape_delay.toml` derives
/// from its loop gain of at most 0.9375.
#[test]
fn the_tape_delay_is_bounded_at_every_setting() {
    let corners = [
        "",
        ", feedback = 1000000, tone = 1000000, mix = 1",
        ", feedback = -1000000, tone = -1000000, mix = -1000000",
        ", feedback = 0.75, tone = 4999, mix = 1000000",
        ", time_ms = 0, feedback = 1000000, mix = 1",
        ", time_ms = 10000, feedback = 1000000, mix = 1",
        ", wow = 1000000, flutter = 1000000, feedback = 1000000, mix = 1",
        ", wow = -1000000, flutter = -1000000, feedback = 1000000, mix = 0.5",
        ", time_ms = 1, wow = 1000000, flutter = -1000000, feedback = 1000000, tone = 1000000, mix = 1",
    ];
    for parameters in corners {
        let rendered = render(
            &echo_fixture(parameters, false),
            None,
            3 * u64::from(RATE),
            (256, 1),
            Vec::new(),
        );
        assert_eq!(faults(&rendered.log), Vec::<String>::new(), "{parameters}");
        assert_every_sample_finite(&rendered.samples, parameters);
        let (echo, input) = (channel(&rendered, 0), channel(&rendered, 1));
        let (out, peak_in) = (peak(&echo), peak(&input));
        println!("echo{parameters}: peak {out:.4}, input {peak_in:.4}");
        assert!(out <= 16.0 * peak_in, "{parameters}: peak {out}");
    }
}

/// AC3: at the default repeat time with the feedback, tone and mix at their
/// maxima, one burst's repeats die away: over eight whole repeats (375 ms
/// apart) each is quieter than the one before, and the eighth is at most
/// 0.9375⁷ of the first.
#[test]
fn the_tape_delay_s_repeats_decay() {
    let burst = ExternalEvent {
        frame: Frame(0),
        sequence: 1,
        target: "/burst.trigger".parse().unwrap(),
        payload: EventPayload::Trigger,
    };
    let repeat = 375 * 48;
    let rendered = render(
        &echo_fixture(", feedback = 1000000, tone = 1000000, mix = 1", true),
        None,
        10 * repeat,
        (256, 1),
        vec![burst],
    );
    assert_eq!(faults(&rendered.log), Vec::<String>::new());
    let echo = channel(&rendered, 0);
    // Repeat r arrives r·375 ms after the burst, give or take the wow and
    // flutter (a few ms); each window starts 50 ms before it.
    let peaks: Vec<f64> = (1..=8)
        .map(|r| peak(&echo[r * repeat as usize - 2400..(r + 1) * repeat as usize - 2400]))
        .collect();
    println!("repeats: {peaks:.5?}");
    assert!(peaks[0] > 0.05, "{peaks:?}");
    assert!(peaks.windows(2).all(|w| w[1] < w[0]), "{peaks:?}");
    assert!(peaks[7] <= 0.9375f64.powi(7) * peaks[0], "{peaks:?}");
}

/// A project of the voice alone on a held note: `hz` a constant 110 Hz from
/// frame 0, the gate and velocity 1, with the voice's `parameters`.
fn voice_fixture(parameters: &str) -> Project {
    let text = format!(
        "schema_version = 1\n\
         [project]\nname = \"voice\"\nsample_rate = 48000\nseed = 3\n\
         [instances]\n\
         hz = {{ module = \"std.const\", value = 110 }}\n\
         gate = {{ module = \"std.const\", value = 1 }}\n\
         voice = {{ module = \"moog_voice\"{parameters} }}\n\
         out = {{ module = \"std.audio_out\" }}\n\
         [wiring]\nconnections = [\n\
           {{ from = \"hz.out\", to = \"voice.hz\" }},\n\
           {{ from = \"gate.out\", to = \"voice.gate\" }},\n\
           {{ from = \"gate.out\", to = \"voice.velocity\" }},\n\
           {{ from = \"voice.out\", to = \"out.l\" }},\n\
           {{ from = \"voice.out\", to = \"out.r\" }},\n\
         ]\n"
    );
    // The octave's 7 lies beyond its export's range: its inner clamp is tested.
    fixture_with(&text, "moog_voice", None, true)
}

/// The voice's samples over 0.25 s with `parameters`.
fn voice(parameters: &str) -> Vec<f64> {
    let rendered = render(
        &voice_fixture(parameters),
        None,
        u64::from(RATE) / 4,
        (256, 1),
        Vec::new(),
    );
    assert_eq!(faults(&rendered.log), Vec::<String>::new(), "{parameters}");
    assert_every_sample_finite(&rendered.samples, parameters);
    channel(&rendered, 0)
}

/// The oscillators' range is a stepped switch: an octave value is clamped to
/// [−2, 2] and rounded, so 0.4 plays as 0, 0.6 as 1 and 7 as 2, and 0 and 1
/// differ.
#[test]
fn the_oscillator_range_is_stepped() {
    let at = |octave: f64| voice(&format!(", osc1_octave = {octave}"));
    assert_eq!(at(0.4), at(0.0));
    assert_eq!(at(0.6), at(1.0));
    assert_eq!(at(-1.4), at(-1.0));
    assert_eq!(at(7.0), at(2.0));
    assert_ne!(at(0.0), at(1.0));
}

/// The first note does not glide up from 0 Hz: with the pitch present from
/// frame 0, a glide of 40 ms or 2 s plays exactly as no glide.
#[test]
fn the_first_note_does_not_glide_from_zero() {
    let none = voice(", glide = 0");
    assert_eq!(voice(""), none);
    assert_eq!(voice(", glide = 2000"), none);
}

/// A project of one studio drum composite played by a `steps` pattern.
fn drum_fixture(composite: &str, wiring: &str, pattern: &str) -> Project {
    let text = format!(
        "schema_version = 1\n\
         [project]\nname = \"drum\"\nsample_rate = 48000\nseed = 5\n\
         [instances]\n\
         transport = {{ module = \"std.transport\", bpm = 120 }}\n\
         drums = {{ module = \"std.step_seq8\", pattern = \"drums\" }}\n\
         voice = {{ module = \"{composite}\" }}\n\
         out = {{ module = \"std.audio_out\" }}\n\
         [wiring]\nconnections = [\n\
           {{ from = \"transport.tick\", to = \"drums.tick\" }},\n\
         {wiring}\
           {{ from = \"voice.out\", to = \"out.l\" }},\n\
           {{ from = \"voice.out\", to = \"out.r\" }},\n\
         ]\n"
    );
    fixture(&text, composite, Some(pattern))
}

/// AC4: an accented kick (its velocity ×1.5) is louder and more saturated
/// than a plain one: a higher peak, and a higher RMS-to-peak ratio over its
/// first 60 ms (the drive rises with the velocity), read from the output.
#[test]
fn an_accent_raises_the_kick_s_level_and_drive() {
    let pattern = "schema_version = 1\nkind = \"steps\"\nsteps = 8\nstep = 0.5\n\
                   [lanes]\nkick = \"x...X...\"\n[levels]\nkick = 0.9\n";
    let wiring = "  { from = \"drums.t1\", to = \"voice.trigger\" },\n\
                  { from = \"drums.t1\", to = \"voice.velocity\" },\n";
    let rendered = render(
        &drum_fixture("kick909", wiring, pattern),
        None,
        2 * u64::from(RATE),
        (256, 1),
        Vec::new(),
    );
    assert_eq!(faults(&rendered.log), Vec::<String>::new());
    let kick = channel(&rendered, 0);
    // The plain hit at beat 0, the accented one at beat 2 (1 s).
    let hit = |at: usize| &kick[at..at + 2880];
    let (plain, accented) = (hit(0), hit(48_000));
    let shape = |x: &[f64]| rms(x) / peak(x);
    println!(
        "kick: plain peak {:.4}, rms/peak {:.4}; accented peak {:.4}, rms/peak {:.4}",
        peak(plain),
        shape(plain),
        peak(accented),
        shape(accented)
    );
    assert!(peak(accented) > 1.1 * peak(plain));
    assert!(shape(accented) > shape(plain));
}

/// AC5: a closed hat chokes the open one: 250–375 ms after a closed hit
/// that follows an open one, the output is a small fraction of what the open
/// hat alone leaves there.
#[test]
fn the_closed_hat_chokes_the_open_hat() {
    let wiring = "  { from = \"drums.t1\", to = \"voice.closed\" },\n\
                  { from = \"drums.t2\", to = \"voice.open\" },\n";
    let pattern = |closed: &str| {
        format!(
            "schema_version = 1\nkind = \"steps\"\nsteps = 8\nstep = 0.25\n\
             [lanes]\nchh = \"{closed}\"\nohh = \"x.......\"\n"
        )
    };
    let tail = |closed: &str| {
        let rendered = render(
            &drum_fixture("hat909", wiring, &pattern(closed)),
            None,
            48_000,
            (256, 1),
            Vec::new(),
        );
        assert_eq!(faults(&rendered.log), Vec::<String>::new());
        // The closed hit on step 2 (frame 6000); 250–375 ms after it.
        rms(&channel(&rendered, 0)[18_000..24_000])
    };
    let (open, choked) = (tail("........"), tail(".x......"));
    println!("hat: open alone {open:.5}, choked {choked:.5}");
    assert!(open > 0.01, "{open}");
    assert!(choked < 0.05 * open, "{choked} against {open}");
}

/// The test copies of the studio with probes counting the melody's notes and
/// the rim lane's hits, before and after the edit: the note at 2.5 beats
/// moved to 2.75, and a rim hit added on step 9 (tick 192).
const PROBES: (&str, &str, &str) = (
    "project.toml",
    "[wiring]\nconnections = [\n",
    "on_probe = { module = \"std.count\" }\nrim_probe = { module = \"std.count\" }\n\n\
     [wiring]\nconnections = [\n  { from = \"melody.on\", to = \"on_probe.in\" },\n  \
     { from = \"drums.t7\", to = \"rim_probe.in\" },\n",
);
const NOTE: (&str, &str, &str) = (
    "patterns/melody.toml",
    "{ key = 45, start = 2.5, length = 0.4, velocity = 0.7 }",
    "{ key = 45, start = 2.75, length = 0.2, velocity = 0.7 }",
);
const STEP: (&str, &str, &str) = (
    "patterns/drums.toml",
    "rim = \"...x.......x....\"",
    "rim = \"...x....x..x....\"",
);

/// AC12: a pattern edit while playing (one note and one step changed) is a
/// swap at an exact frame that keeps the sequencers' positions: the output
/// is the unedited one up to the swap, the moved note starts on its new tick
/// (264, not 240) and the added rim hit fires on tick 192, both after the
/// swap at tick 100, with every other note and hit where it was.
#[test]
fn a_pattern_edit_takes_effect_at_the_edit_frame() {
    let old = project(&[PROBES]);
    let new = project(&[PROBES, NOTE, STEP]);
    let swap = tick(100) + 7;
    let frames = tick(400);
    let edited = render(&old, Some((&new, swap)), frames, (64, 1), Vec::new());
    let unedited = render(&old, None, frames, (64, 1), Vec::new());
    assert_eq!(faults(&edited.log), Vec::<String>::new());
    assert_eq!(
        edited.samples[..2 * swap as usize],
        unedited.samples[..2 * swap as usize]
    );
    // Notes every 48 ticks; the one at 240 moves to 264.
    let notes: Vec<u64> = (0..=8).map(|n| tick(48 * n)).collect();
    assert_eq!(triggers(&unedited, "/on_probe.in"), notes);
    let mut moved = notes.clone();
    moved[5] = tick(264);
    assert_eq!(triggers(&edited, "/on_probe.in"), moved);
    // Rim on steps 4 and 12 (ticks 72 and 264); the edit adds tick 192.
    assert_eq!(triggers(&unedited, "/rim_probe.in"), [tick(72), tick(264)]);
    assert_eq!(
        triggers(&edited, "/rim_probe.in"),
        [tick(72), tick(192), tick(264)]
    );
}

/// AC12: the same edit made live, on the null driver: the pattern files of a
/// copy are rewritten while it plays and reloaded (the watch's reload), which
/// is one swap and no restart; the session recorded from it replays to
/// exactly what was played.
#[test]
fn a_live_pattern_edit_is_recorded_and_replays_bit_identically() {
    let registry = registry();
    let dir = std::env::temp_dir().join(format!("signal-studio-live-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    for file in sources(&[]) {
        let path = dir.join(&file.path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, file.text).unwrap();
    }
    let mut current = Project::load(&dir).unwrap();
    let mut plan = current.plan(&registry).unwrap().plan;
    plan.version = 0;
    let (runner, mut controller) =
        realtime(&plan, &registry, RealtimeConfig::new(RATE, current.seed)).unwrap();
    let provenance = provenance::collect(&provenance::source_dir(), &current.files);
    let recorder =
        SessionRecorder::new(&current, &registry, RATE, current.seed, provenance).unwrap();
    let mut live = LiveRecorder::new(recorder, registry.clone());
    let mut driver = NullDriver::new(RATE, 2, 3);
    driver.start(runner).unwrap();
    let run = |driver: &mut NullDriver,
               controller: &mut signal_output::Controller,
               live: &mut LiveRecorder,
               blocks: u64| {
        for _ in 0..blocks {
            driver.run(256, || {}).unwrap();
            controller.poll(live);
        }
    };
    run(&mut driver, &mut controller, &mut live, 40);
    for (file, from, to) in [NOTE, STEP] {
        let path = dir.join(file);
        let text = fs::read_to_string(&path).unwrap().replace(from, to);
        fs::write(&path, text).unwrap();
    }
    let applied = reload(&mut controller, &mut live, &mut current, &dir, &registry).unwrap();
    assert_eq!(
        (applied.sets, applied.swap, applied.removed),
        (0, Some(1), 0)
    );
    run(&mut driver, &mut controller, &mut live, 160);
    let runner = driver.stop().unwrap().unwrap();
    controller.poll(&mut live);
    live.finish(runner.next_frame()).unwrap();
    assert_eq!(live.refused(), &[] as &[String]);

    let recorder = live.into_recorder();
    let swaps = recorder
        .edits()
        .iter()
        .filter(|e| matches!(e.command, signal_project::session::Command::Swap { .. }))
        .count();
    assert_eq!(swaps, 1);
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("studio/live");
    let _ = fs::remove_dir_all(&out);
    recorder.write(&out).unwrap();
    let session = Session::load(&out, &registry).unwrap();
    let replayed = replay(&session, &registry, None, 1).unwrap();
    assert_eq!(faults(&replayed.render.log), Vec::<String>::new());
    let samples: Vec<f32> = replayed.render.samples.iter().map(|&x| x as f32).collect();
    assert_eq!(samples, driver.output(), "replay equals what was played");
    let _ = fs::remove_dir_all(&dir);
}

/// AC7, AC8: `signal ui check projects/studio` passes with no warning; the
/// panel has the three sections, the piano roll and the step grid on the
/// sequencers' playheads, meters and a scope on the master bus, and names no
/// brand or model.
#[test]
fn the_studio_panel_passes_and_names_no_brand() {
    let output = signal(&["ui", "check", &studio().display().to_string()]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains("ui/main.html: 65 controls, 65 bindings: ok"),
        "{stderr}"
    );
    assert!(!stderr.contains("warning"), "{stderr}");

    let html = fs::read_to_string(studio().join("ui/main.html")).unwrap();
    for needle in [
        "<h2>MONO SYNTH</h2>",
        "<h2>RHYTHM COMPOSER</h2>",
        "<h2>TAPE ECHO</h2>",
        r#"<signal-piano-roll pattern="melody" position-bind="/melody.position""#,
        r#"<signal-step-grid pattern="drums" step-bind="/drums.step""#,
        r#"<signal-meter bind="/out.l""#,
        r#"<signal-meter bind="/out.r""#,
        r#"<signal-scope bind="/limiter.out""#,
        r#"<link rel="stylesheet" href="studio.css">"#,
    ] {
        assert!(html.contains(needle), "{needle}");
    }
    let mut ui = vec![html];
    ui.push(fs::read_to_string(studio().join("ui/studio.css")).unwrap());
    for entry in fs::read_dir(studio().join("ui/assets")).unwrap() {
        ui.push(fs::read_to_string(entry.unwrap().path()).unwrap());
    }
    for text in &ui {
        let lower = text.to_lowercase();
        for brand in [
            "moog",
            "minimoog",
            "roland",
            "909",
            "808",
            "space echo",
            "re-201",
            "echoplex",
            "korg",
        ] {
            assert!(!lower.contains(brand), "the panel names {brand}");
        }
    }
}
