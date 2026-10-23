//! studio-transport (ADR 0033) as committed in M3.1, frozen in
//! `tests/fixtures/studio-transport` (the project's text files; the owner
//! goes on tuning `projects/studio-transport` itself, which these tests do
//! not read): the studio played from
//! `lib.transport`, its sequencers following the position on a wire. Its
//! golden render (16 s, with no fault, no mute and a safe peak), the same
//! hashes in blocks of 1, 64 and 256 frames on one and four threads; every
//! drum hit exactly one frame after the studio's tick clock places it (the
//! needle: a hit fires on the first frame whose position is past its beat);
//! the panel's play, stop, rewind and rate as the engine sees them; and a
//! sequencer given both `tick` and `follow` refused. How to update the
//! golden: `docs/testing-and-evaluation.md`. The committed projects are only
//! read.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use signal_core::{EventPayload, Frame, Registry};
use signal_engine::{Engine, EngineConfig, ExternalEvent, Schedule};
use signal_output::{Render, render_blocks};
use signal_project::{Project, SourceFile};

/// The project's rate in `tests/fixtures/studio-transport/project.toml`.
const RATE: u32 = 48_000;
/// 16 s at 48 kHz (8 bars at 120 bpm): the golden's length.
const GOLDEN_FRAMES: u64 = 16 * 48_000;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn project_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/studio-transport")
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

fn out_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// Renders `frames` of `project` in blocks of `k` on `threads` threads,
/// with `external` events.
fn render(
    project: &Project,
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
    let rendered = render_blocks(&mut engine, frames, k, |_| Ok(None)).unwrap();
    assert_eq!(engine.log_overflows(), 0);
    rendered
}

fn faults(log: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(log)
        .lines()
        .filter(|l| l.contains("\"type\":\"fault\"") || l.contains("\"type\":\"mute\""))
        .map(str::to_owned)
        .collect()
}

/// Every event the drums' lanes delivered: (frame, target, payload).
fn hits(render: &Render) -> Vec<(u64, String, String)> {
    String::from_utf8_lossy(&render.log)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|v| {
            v["type"] == "event"
                && v["source"]["port"]
                    .as_str()
                    .is_some_and(|p| p.starts_with("/drums."))
        })
        .map(|v| {
            (
                v["frame"].as_u64().unwrap(),
                v["target"].as_str().unwrap().to_owned(),
                v["payload"].to_string(),
            )
        })
        .collect()
}

fn peak(x: &[f64]) -> f64 {
    x.iter().fold(0.0f64, |p, x| p.max(x.abs()))
}

fn external(frame: u64, sequence: u64, target: &str, payload: EventPayload) -> ExternalEvent {
    ExternalEvent {
        frame: Frame(frame),
        sequence,
        target: target.parse().unwrap(),
        payload,
    }
}

/// The project as its file documents it: `lib.transport` at 120 bpm, the
/// sequencers following its position and counting no ticks.
#[test]
fn the_sequencers_follow_the_transport() {
    let project = Project::load(&project_dir()).unwrap();
    assert_eq!(
        (project.name.as_str(), project.sample_rate, project.seed),
        ("studio-transport", RATE, 11)
    );
    let transport = project
        .root
        .instances
        .iter()
        .find(|i| i.name == "transport")
        .unwrap();
    assert_eq!(transport.module, "lib.transport");
    let into = |to: &str| {
        project
            .root
            .connections
            .iter()
            .filter(|c| c.to == to)
            .map(|c| c.from.as_str())
            .collect::<Vec<_>>()
    };
    for seq in ["melody", "drums"] {
        assert_eq!(into(&format!("{seq}.follow")), ["transport.position"]);
        assert!(into(&format!("{seq}.tick")).is_empty());
        assert_eq!(
            into(&format!("{seq}.reset")),
            ["transport.jump"],
            "every seek marked"
        );
    }
    assert!(project.library_hashes().contains_key("lib.transport"));
    // The patterns and the voices are the studio's.
    for file in [
        "patterns/melody.toml",
        "patterns/drums.toml",
        "composites/moog_voice.toml",
        "composites/kick909.toml",
    ] {
        assert_eq!(
            fs::read_to_string(project_dir().join(file)).unwrap(),
            fs::read_to_string(repository().join("projects/studio").join(file)).unwrap(),
            "{file}"
        );
    }
}

/// `tests/golden/studio-transport/manifest.json` is
/// `signal render crates/signal-cli/tests/fixtures/studio-transport --frames 768000 --threads 1`
/// (16 s); on the same render the log has no fault or mute, every sample is
/// finite and the peak is at most 0.25 (the output stage's cap) and below
/// −1 dBFS.
#[test]
fn golden_studio_transport_16_seconds() {
    let out = out_dir("studio-transport-golden");
    let (project, out_text, frames) = (
        project_dir().display().to_string(),
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
    assert!(samples.iter().all(|x| x.is_finite()));
    let peak = peak(&samples);
    assert!(
        peak <= 0.25 && peak < 10f64.powf(-1.0 / 20.0),
        "peak {peak}"
    );
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
        names(&["project_file_hashes", "library_composite_hashes"])
    );
    let golden = repository().join("tests/golden/studio-transport/manifest.json");
    let fresh = serde_json::to_string_pretty(&manifest).unwrap();
    let expected = fs::read_to_string(&golden).map(|t| serde_json::from_str::<Value>(&t));
    assert!(
        matches!(&expected, Ok(Ok(golden)) if *golden == manifest),
        "the render differs from {}; see docs/testing-and-evaluation.md before updating it. \
         Fresh manifest:\n{fresh}",
        golden.display()
    );
}

/// 4 s in blocks of 1, 64 and 256 frames, on one thread and on four (every
/// segment on the workers): the same audio and log hashes (ADR 0002, 0019).
#[test]
fn renders_identically_in_blocks_and_on_threads() {
    let project = Project::load(&project_dir()).unwrap();
    let frames = 4 * u64::from(RATE);
    let hashes = |r: &Render| (r.audio_fnv1a64(), r.log_fnv1a64());
    let reference = render(&project, frames, (1, 1), Vec::new());
    assert_eq!(faults(&reference.log), Vec::<String>::new());
    assert!(peak(&reference.samples) > 0.05);
    for k in [1, 64, 256] {
        for threads in [1, 4] {
            let rendered = render(&project, frames, (k, threads), Vec::new());
            assert_eq!(
                hashes(&rendered),
                hashes(&reference),
                "k = {k}, threads = {threads}"
            );
        }
    }
}

/// The needle (ADR 0033): every event the drums deliver in 8 s (every
/// lane's level `Set` and `Trigger`, into every voice) is the studio's,
/// on the frame after the studio's: the studio's tick clock places a 16th
/// on its frame, `24 000·b` for beat *b*; the followed position is exactly
/// *b* there, and the hit fires on the first frame past it.
#[test]
fn every_drum_hit_is_the_studio_s_one_frame_later() {
    let frames = 8 * u64::from(RATE);
    let studio = render(
        &Project::load(&repository().join("projects/studio")).unwrap(),
        frames,
        (256, 1),
        Vec::new(),
    );
    let followed = render(
        &Project::load(&project_dir()).unwrap(),
        frames,
        (256, 1),
        Vec::new(),
    );
    let expected: Vec<(u64, String, String)> = hits(&studio)
        .into_iter()
        .filter(|h| h.0 + 1 < frames)
        .map(|(f, t, p)| (f + 1, t, p))
        .collect();
    assert!(expected.len() > 500, "{} hits", expected.len());
    assert_eq!(hits(&followed), expected);
    assert_eq!(expected[0].0, 1, "the downbeat, on frame 1");
}

/// The panel's controls as the engine receives them: `run` 0 from 2 s to
/// 3 s stops every hit (and the position holds), and playing on loses
/// nothing; the Rewind button's
/// trigger into `/transport/playhead.seek` at 5 s plays the downbeat on the
/// next frame; `rate` 0.5 from 6 s halves the hits' rate.
#[test]
fn play_stop_rewind_and_rate_from_the_panel() {
    let project = Project::load(&project_dir()).unwrap();
    let s = u64::from(RATE);
    let events = vec![
        external(2 * s, 1, "/transport/playhead.run", EventPayload::Set(0.0)),
        external(3 * s, 2, "/transport/playhead.run", EventPayload::Set(1.0)),
        external(5 * s, 3, "/transport/playhead.seek", EventPayload::Trigger),
        external(6 * s, 4, "/transport/playhead.rate", EventPayload::Set(0.5)),
    ];
    let rendered = render(&project, 8 * s, (64, 1), events);
    assert_eq!(faults(&rendered.log), Vec::<String>::new());
    let hits = hits(&rendered);
    let frames: Vec<u64> = hits.iter().map(|h| h.0).collect();
    // Stopped: nothing from the first frame that did not move (2 s + 1)
    // until it moves again (3 s + 1 at the earliest).
    assert!(
        frames.iter().all(|&f| !(2 * s + 1..=3 * s).contains(&f)),
        "silent while stopped"
    );
    // It resumes where it stopped: at 2 s the position is exactly beat 4,
    // not yet crossed, so beat 4's hits are held back, not lost: they fire
    // on the first frame that moves past it.
    let resumed = frames.iter().copied().find(|&f| f > 2 * s).unwrap();
    assert_eq!(resumed, 3 * s + 1, "beat 4, on the first frame past it");
    let next = frames.iter().copied().find(|&f| f > resumed).unwrap();
    assert_eq!(next, resumed + 6000, "then beat 4.25");
    // The rewind: the cymbal and kick of beat 0 on the frame after it.
    let downbeat: Vec<&(u64, String, String)> = hits.iter().filter(|h| h.0 == 5 * s + 1).collect();
    assert!(
        downbeat.iter().any(|h| h.1 == "/cymbal/env.trigger"),
        "{downbeat:?}"
    );
    assert!(
        downbeat.iter().any(|h| h.1.starts_with("/kick/")),
        "{downbeat:?}"
    );
    // Half rate from 6 s: 16ths every 12 000 frames.
    let after: Vec<u64> = frames
        .iter()
        .copied()
        .filter(|&f| f > 6 * s)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    assert!(
        after.windows(2).all(|w| (w[1] - w[0]) % 12_000 == 0),
        "{after:?}"
    );
}

/// A sequencer given both a tick clock and a position is refused when the
/// plan is built, with the connection's place in `project.toml` and both
/// ports named.
#[test]
fn tick_and_follow_together_are_refused() {
    let text = fs::read_to_string(project_dir().join("project.toml"))
        .unwrap()
        .replace(
            "[instances]\n",
            "[instances]\nclock = { module = \"std.transport\" }\n",
        )
        .replace(
            "  { from = \"transport.position\", to = \"drums.follow\" },\n",
            "  { from = \"transport.position\", to = \"drums.follow\" },\n  { from = \"clock.tick\", to = \"drums.tick\" },\n",
        );
    let mut files = vec![SourceFile {
        path: "project.toml".to_owned(),
        text,
    }];
    for dir in ["composites", "patterns"] {
        let mut names: Vec<String> = fs::read_dir(project_dir().join(dir))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        for name in names {
            files.push(SourceFile {
                path: format!("{dir}/{name}"),
                text: fs::read_to_string(project_dir().join(dir).join(&name)).unwrap(),
            });
        }
    }
    let project = Project::from_sources(files).unwrap();
    let Err(diagnostics) = project.plan(&registry()) else {
        panic!("planned");
    };
    let text: Vec<String> = diagnostics.iter().map(ToString::to_string).collect();
    assert_eq!(text.len(), 1, "{text:?}");
    assert!(
        text[0].contains(
            "/transport.position -> /drums.follow: /drums.follow and /drums.tick are both connected; \
             a std.step_seq8 takes `tick` or `follow`, not both"
        ),
        "{text:?}"
    );
    assert!(text[0].starts_with("project.toml:"), "{text:?}");
}
