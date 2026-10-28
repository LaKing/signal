//! ADR 0037 test 5: a slot's Load, Unload and Replace as live edits
//! (`signal_project::live::send_edit`), on the null driver's real-time run
//! in seeded callback sizes, on 1 and 4 threads. A Load is one swap whose
//! newcomer fades in; an Unload suspends, then swaps at *s* + *F*; a Replace
//! is an unload, then a load a frame later: the old content reaches +0.0 at
//! *s* + *F* − 1, the new one fades in from a fresh state although both are
//! `/deck/osc`, a `std.sine_osc`, and the host's own sound is bit-continuous.
//! Each session replays to what was played (the Replace's middle version is
//! the slot unloaded, as `slot.unload` writes it). An interface edit that
//! changes no match is no edit at all. The callbacks' allocations are
//! counted in `signal-output`'s `chained_swaps.rs`, which drives the same two
//! swaps.

use std::path::PathBuf;

use signal_core::Registry;
use signal_engine::{RealtimeConfig, Stamp};
use signal_output::{Controller, Driver, NullDriver, realtime};
use signal_project::live::{Applied, send_edit};
use signal_project::session::{self, LiveRecorder, Session, SessionRecorder, replay};
use signal_project::{Project, SourceFile, provenance};

const RATE: u32 = 48_000;
/// *F*: 5 ms at 48 kHz.
const F: usize = 240;

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn voice(name: &str, freq: f64) -> SourceFile {
    SourceFile {
        path: format!("composites/{name}.toml"),
        text: format!(
            "schema_version = 1\n\n[composite]\nname = \"{name}\"\n\n[instances]\nosc = {{ module = \"std.sine_osc\", freq = {freq} }}\n\n[exports]\noutputs = {{ out = \"osc.out\" }}\n"
        ),
    }
}

/// The host: its `tone` on the left; the slot `deck` (one audio output
/// `out`, and `extra` declared inputs nothing wires) loading `load` (or
/// empty) on the right; two voices to load, `voice_a` (220 Hz) and
/// `voice_b` (440 Hz), each an `osc` at `/deck/osc`.
fn host(load: Option<&str>, extra: &str) -> Project {
    let load = load.map_or(String::new(), |l| format!(", load = \"{l}\""));
    let text = format!(
        "schema_version = 1\n\n[project]\nname = \"slots\"\nsample_rate = {RATE}\nseed = 5\n\n[instances]\n\
         tone = {{ module = \"std.sine_osc\", freq = 330 }}\n\
         # The slot.\n\
         deck = {{ slot = {{ outputs = {{ out = \"audio\" }}{extra} }}{load} }}\n\
         out = {{ module = \"std.audio_out\" }}\n\n[wiring]\nconnections = [\n  {{ from = \"tone.out\", to = \"out.l\" }},\n  {{ from = \"deck.out\", to = \"out.r\" }},\n]\n"
    );
    Project::from_sources(vec![
        SourceFile {
            path: "project.toml".to_owned(),
            text,
        },
        voice("voice_a", 220.0),
        voice("voice_b", 440.0),
    ])
    .unwrap()
}

struct Run {
    driver: NullDriver,
    controller: Controller,
    live: LiveRecorder,
    /// Commands may be late (a test that stamps them so).
    late: bool,
}

impl Run {
    fn start(project: &Project, registry: &Registry, threads: usize, seed: u64) -> Run {
        let mut plan = project.plan(registry).unwrap().plan;
        plan.version = 0;
        // Commands are stamped two 4096-frame buffers ahead: no callback
        // size makes one late.
        let mut config = RealtimeConfig::with_buffer(RATE, project.seed, 4096);
        config.engine.threads = threads;
        let (runner, controller) = realtime(&plan, registry, config).unwrap();
        let provenance = provenance::collect(&provenance::source_dir(), &project.files);
        let recorder =
            SessionRecorder::new(project, registry, RATE, project.seed, provenance).unwrap();
        let mut driver = NullDriver::new(RATE, 2, seed);
        driver.start(runner).unwrap();
        Run {
            driver,
            controller,
            live: LiveRecorder::new(recorder, registry.clone()),
            late: false,
        }
    }

    /// Plays at least `frames` more frames, polling after every callback.
    fn play(&mut self, frames: u64) {
        let (controller, live) = (&mut self.controller, &mut self.live);
        self.driver.run(frames, || controller.poll(live)).unwrap();
        self.controller.poll(&mut self.live);
    }

    /// Stops: what was played (left, right), the recorded edits, and the
    /// session's replay as f32.
    fn finish(mut self, registry: &Registry, name: &str) -> Played {
        while self.controller.in_flight() > 0 {
            self.play(1024);
        }
        self.play(1024);
        let played = self.driver.take_output();
        let runner = self.driver.stop().unwrap().unwrap();
        self.controller.poll(&mut self.live);
        self.live.finish(runner.next_frame()).unwrap();
        assert_eq!(self.live.refused(), &[] as &[String]);
        if !self.late {
            assert_eq!(runner.status().late, 0, "nothing late");
        }
        let recorder = self.live.into_recorder();
        let edits = recorder.edits();
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("slot-live/{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        recorder.write(&dir).unwrap();
        let session = Session::load(&dir, registry).unwrap();
        let replayed = replay(&session, registry, None, 1).unwrap();
        let replayed: Vec<f32> = replayed.render.samples.iter().map(|&x| x as f32).collect();
        assert_eq!(replayed, played, "{name}: replay equals what was played");
        Played {
            left: played.chunks(2).map(|c| c[0]).collect(),
            right: played.chunks(2).map(|c| c[1]).collect(),
            edits,
        }
    }
}

struct Played {
    left: Vec<f32>,
    right: Vec<f32>,
    edits: Vec<session::Edit>,
}

impl Played {
    /// Each edit's kind and frame.
    fn edits(&self) -> Vec<(&'static str, usize)> {
        self.edits
            .iter()
            .map(|e| {
                let kind = match e.command {
                    session::Command::Swap { .. } => "swap",
                    session::Command::Edit { .. } => "edit",
                    session::Command::Event { .. } => "event",
                };
                (kind, e.frame.0 as usize)
            })
            .collect()
    }
}

/// A fresh `std.sine_osc` at `freq` from its first frame, as f64 (what the
/// slot's content plays before the sink's conversion).
fn fresh(freq: f64, frames: u64) -> Vec<f64> {
    let registry = registry();
    let text = format!(
        "schema_version = 1\n[project]\nname = \"fresh\"\nsample_rate = {RATE}\nseed = 5\n[instances]\nosc = {{ module = \"std.sine_osc\", freq = {freq} }}\nout = {{ module = \"std.audio_out\" }}\n[wiring]\nconnections = [ {{ from = \"osc.out\", to = \"out.r\" }} ]\n"
    );
    let project = Project::from_sources(vec![SourceFile {
        path: "project.toml".to_owned(),
        text,
    }])
    .unwrap();
    let plan = project.plan(&registry).unwrap().plan;
    let mut engine = signal_engine::Engine::new(
        &plan,
        &registry,
        signal_engine::EngineConfig::new(RATE, 5),
        Vec::new(),
    )
    .unwrap();
    let rendered = signal_output::render(&mut engine, frames).unwrap();
    rendered.samples.chunks(2).map(|f| f[1]).collect()
}

/// Frame `j` of a fade-in of `F` frames to `live`, as the sink plays it.
fn faded_in(live: f64, j: usize) -> f32 {
    let done = j + 1;
    if done >= F {
        live as f32
    } else {
        (live * (done as f64 / F as f64)) as f32
    }
}

/// The host's tone alone, as played on the left.
fn tone(frames: u64) -> Vec<f32> {
    fresh(330.0, frames).iter().map(|&x| x as f32).collect()
}

#[test]
fn a_replace_unloads_then_loads_with_no_state_carried() {
    let registry = registry();
    let a = fresh(220.0, 60_000);
    let b = fresh(440.0, 60_000);
    let tone = tone(60_000);
    for (threads, seed) in [(1, 11), (4, 12)] {
        let old = host(Some("voice_a"), "");
        let mut run = Run::start(&old, &registry, threads, seed);
        run.play(3_000);
        let new = host(Some("voice_b"), "");
        let (_, applied) =
            send_edit(&mut run.controller, &mut run.live, &old, new, &registry).unwrap();
        assert_eq!(
            applied,
            Applied {
                sets: 0,
                swap: Some(1),
                removed: 1,
                replaced: 1,
                load_swap: Some(2),
                unfinished: false,
            }
        );
        assert_eq!(
            applied.to_string(),
            "replaced 1 slot content: unloaded by a swap to version 1, loaded by a swap to version 2 (0 sets, 1 removed)"
        );
        run.play(30_000);
        let played = run.finish(&registry, &format!("replace-{threads}"));
        let edits = played.edits();
        assert_eq!(
            edits.iter().map(|e| e.0).collect::<Vec<_>>(),
            ["edit", "swap", "swap"]
        );
        let s = edits[0].1;
        assert_eq!(edits[1].1, s + F, "the unload after the fade");
        assert_eq!(edits[2].1, s + F + 1, "the load a frame later");
        // The host's own sound is bit-continuous.
        assert_eq!(played.left, tone[..played.left.len()]);
        // The old voice until s, faded out to +0.0 at s + F − 1; the slot
        // empty at s + F.
        assert_eq!(played.right[s - 1], a[s - 1] as f32);
        assert!(
            played.right[s + F - 1..=s + F]
                .iter()
                .all(|x| x.to_bits() == 0),
            "+0.0 at s + F − 1 and s + F"
        );
        // The new voice from its own first frame, fading in: a fresh
        // oscillator, not the old one's phase at 440 Hz.
        for (j, &live) in b.iter().enumerate().take(2 * F) {
            assert_eq!(played.right[s + F + 1 + j], faded_in(live, j), "j {j}");
        }
    }
}

#[test]
fn a_load_fades_in_and_an_unload_fades_out() {
    let registry = registry();
    let a = fresh(220.0, 60_000);
    let tone = tone(60_000);
    for (threads, seed) in [(1, 21), (4, 22)] {
        // Load: one swap, the newcomer fading in.
        let empty = host(None, "");
        let mut run = Run::start(&empty, &registry, threads, seed);
        run.play(2_000);
        let loaded = host(Some("voice_a"), "");
        let (now, applied) = send_edit(
            &mut run.controller,
            &mut run.live,
            &empty,
            loaded,
            &registry,
        )
        .unwrap();
        assert_eq!((applied.swap, applied.load_swap), (Some(1), None));
        assert_eq!((applied.removed, applied.replaced), (0, 0));
        run.play(12_000);

        // Unload: suspended, then swapped out after the fade.
        let (_, applied) = send_edit(
            &mut run.controller,
            &mut run.live,
            &now,
            host(None, ""),
            &registry,
        )
        .unwrap();
        assert_eq!((applied.swap, applied.removed), (Some(2), 1));
        run.play(12_000);
        let played = run.finish(&registry, &format!("load-unload-{threads}"));
        let edits = played.edits();
        assert_eq!(
            edits.iter().map(|e| e.0).collect::<Vec<_>>(),
            ["swap", "edit", "swap"]
        );
        let load = edits[0].1;
        assert!(played.right[..load].iter().all(|&x| x == 0.0));
        for (j, &live) in a.iter().enumerate().take(2 * F) {
            assert_eq!(played.right[load + j], faded_in(live, j), "j {j}");
        }
        let s = edits[1].1;
        assert_eq!(edits[2].1, s + F);
        assert!(played.right[s] != 0.0);
        assert!(
            played.right[s + F - 1..].iter().all(|&x| x.to_bits() == 0),
            "+0.0 from s + F − 1 on"
        );
        assert_eq!(played.left, tone[..played.left.len()]);
    }
}

/// A Replace's two swaps stamped for one frame already played (a lead too
/// short for the runner): both are late, and they commit on two
/// consecutive frames, never one (one swap per frame); the session records
/// them where they applied and replays to what was played. (No suspend is
/// sent: a suspend as late as its swap lands on the swap's frame, after the
/// swap removed its instance, and is discarded, so the removal has no fade;
/// `send_edit` stamps the swap F after the suspend from a fresh status, so
/// only a control thread stalled for more than the latency gets there.)
#[test]
fn late_swaps_of_a_replace_commit_on_consecutive_frames_and_replay() {
    let registry = registry();
    let old = host(Some("voice_a"), "");
    let mut run = Run::start(&old, &registry, 1, 41);
    run.late = true;
    run.play(5_000);
    let mid = host(None, "");
    let new = host(Some("voice_b"), "");
    let unload = run.live.swap_plan(mid).unwrap();
    let load = run.live.swap_plan(new).unwrap();
    let ready = run.controller.prepare_swap(&unload).unwrap();
    let after = run.controller.prepare_swap_after(&ready, &load).unwrap();
    let past = signal_core::Frame(100);
    run.controller.send_swap(ready, Stamp::At(past)).unwrap();
    run.controller.send_swap(after, Stamp::At(past)).unwrap();
    run.play(4_000);
    let played = run.finish(&registry, "late");
    let edits = played.edits();
    assert_eq!(
        edits.iter().map(|e| e.0).collect::<Vec<_>>(),
        ["swap", "swap"]
    );
    assert!(
        edits[0].1 >= 5_000,
        "late: applied on the next frame to run"
    );
    assert_eq!(edits[1].1, edits[0].1 + 1, "the load on the next frame");
}

/// An own copy of the content under another name (`voice_a_inside`, what
/// the editor's "edit the slot's insides" writes) plays alike: pointing
/// `load` at it is no Replace and sends nothing, so making the copy is
/// inaudible; a copy that differs (another frequency) is a Replace.
#[test]
fn loading_an_identical_copy_is_no_replace() {
    let registry = registry();
    let old = host(Some("voice_a"), "");
    let with_copy = |freq: f64| {
        let mut files = host(Some("voice_a"), "").files;
        files[0].text = files[0]
            .text
            .replace("load = \"voice_a\"", "load = \"voice_a_inside\"");
        files.push(voice("voice_a_inside", freq));
        Project::from_sources(files).unwrap()
    };
    let mut run = Run::start(&old, &registry, 1, 51);
    run.play(2_000);
    let (now, applied) = send_edit(
        &mut run.controller,
        &mut run.live,
        &old,
        with_copy(220.0),
        &registry,
    )
    .unwrap();
    assert_eq!(applied, Applied::default(), "{applied}");
    run.play(2_000);
    let (_, applied) = send_edit(
        &mut run.controller,
        &mut run.live,
        &now,
        {
            // Another copy that differs: a Replace.
            let mut files = now.files.clone();
            files.push(voice("voice_a_other", 330.0));
            files[0].text = files[0]
                .text
                .replace("load = \"voice_a_inside\"", "load = \"voice_a_other\"");
            Project::from_sources(files).unwrap()
        },
        &registry,
    )
    .unwrap();
    assert_eq!(
        (applied.replaced, applied.load_swap),
        (1, Some(2)),
        "{applied}"
    );
    let played = run.finish(&registry, "own-copy");
    assert_eq!(
        played.edits().iter().map(|e| e.0).collect::<Vec<_>>(),
        ["edit", "swap", "swap"],
        "only the real Replace swaps"
    );
}

/// An interface edit that changes no match (an input nothing wires) plans
/// byte-identically: no swap, nothing sent.
#[test]
fn an_interface_edit_that_changes_no_match_is_no_edit() {
    let registry = registry();
    let old = host(Some("voice_a"), "");
    let new = host(Some("voice_a"), ", inputs = { cue = \"gate\" }");
    let mut run = Run::start(&old, &registry, 1, 31);
    run.play(1_000);
    let (_, applied) = send_edit(&mut run.controller, &mut run.live, &old, new, &registry).unwrap();
    assert_eq!(applied, Applied::default());
    assert_eq!(applied.to_string(), "no change to apply");
    let played = run.finish(&registry, "interface");
    assert!(played.edits.is_empty());
}
