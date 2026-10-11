//! The real-time runner against the offline reference (M2-3): V9 on the kick
//! project (AC1), and a live session issued through the `Controller`,
//! recorded by `LiveRecorder` and replayed by `signal replay` (AC2). Here
//! because both need `signal-std` and project loading, which `signal-engine`
//! must not depend on.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use signal_core::{Curve, EventPayload, Fnv1a64, Frame, InstanceAddress, PortAddress, Registry};
use signal_engine::{
    CallbackSizes, ControlSink, Controller, Driver, Engine, EngineConfig, NullDriver, Outcome,
    Provenance, RealtimeConfig, Report, SentCommand, Stamp, realtime, render,
};
use signal_project::session::{self, LiveRecorder, Session, SessionRecorder, replay};
use signal_project::{Project, ProjectEdit, diff};

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn version(n: u32) -> Project {
    Project::load(&fixture(&format!("session/v{n}"))).unwrap()
}

/// A fresh folder for one test, inside the target directory.
fn out_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("realtime")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn port(text: &str) -> PortAddress {
    text.parse().unwrap()
}

fn addr(text: &str) -> InstanceAddress {
    text.parse().unwrap()
}

fn provenance() -> Provenance {
    Provenance {
        signal_revision: "realtime-test".to_owned(),
        signal_dirty: Some(false),
        rustc_version: "rustc (test)".to_owned(),
        project_file_hashes: Default::default(),
        cargo_lock_hash: "0000000000000000".to_owned(),
    }
}

/// FNV-1a 64 of f32 samples as `audio.wav` stores them.
fn audio_hash(samples: &[f32]) -> String {
    let mut hasher = Fnv1a64::new();
    for sample in samples {
        hasher.update(&sample.to_le_bytes());
    }
    format!("{:016x}", hasher.finish())
}

/// V9 (AC1): 60 s of the kick project through the null driver, for three
/// callback-size seeds, equals the offline render bit for bit as f32. The
/// seeds run in parallel and compare each callback as it arrives.
#[test]
fn v9_null_driver_equals_the_offline_render_of_the_kick() {
    let started = Instant::now();
    let registry = registry();
    let project = Project::load(&repository().join("projects/kick")).unwrap();
    let plan = project.plan(&registry).unwrap().plan;
    let (rate, seed) = (project.sample_rate, project.seed);
    let frames = 60 * u64::from(rate);
    let config = EngineConfig::new(rate, seed);
    let mut engine = Engine::new(&plan, &registry, config, Vec::new()).unwrap();
    let rendered = render(&mut engine, frames).unwrap();
    assert_eq!(rendered.channels, 2);
    let reference: Vec<f32> = rendered.samples.iter().map(|&x| x as f32).collect();
    drop(rendered);

    let primes = [3, 7, 31, 127, 509, 1021, 2039, 4093];
    thread::scope(|scope| {
        for callback_seed in [1, 2, 3] {
            let (plan, registry, reference) = (&plan, &registry, &reference);
            scope.spawn(move || {
                let sizes: Vec<usize> = CallbackSizes::new(callback_seed).take(3).collect();
                assert_eq!(sizes[0], 1);
                assert!(primes.contains(&sizes[1]));
                assert!(sizes[2].is_power_of_two());

                let config = RealtimeConfig::new(rate, seed);
                let (runner, _controller) = realtime(plan, registry, config).unwrap();
                let mut driver = NullDriver::new(rate, 2, callback_seed);
                driver.start(runner).unwrap();
                let mut at = 0;
                driver
                    .run_with(
                        frames,
                        || {},
                        |callback| {
                            let expected = &reference[at..at + callback.len()];
                            if let Some(k) = (0..callback.len())
                                .find(|&k| callback[k].to_bits() != expected[k].to_bits())
                            {
                                panic!(
                                    "seed {callback_seed}: sample {} is {}, offline {}",
                                    at + k,
                                    callback[k],
                                    expected[k]
                                );
                            }
                            at += callback.len();
                        },
                    )
                    .unwrap();
                assert_eq!(at, reference.len());
                let runner = driver.stop().unwrap().unwrap();
                assert_eq!(runner.status().clipped, 0, "the kick never clips");
                assert_eq!(runner.next_frame(), Frame(frames));
            });
        }
    });
    eprintln!("V9: 4 × 60 s of the kick in {:?}", started.elapsed());
}

/// The scripted session's results.
struct Live {
    output: Vec<f32>,
    frames: u64,
    session: PathBuf,
}

/// Writes the recorded session and checks it replays to `output`, both with
/// the `signal replay` binary (its audio hash) and in process (the samples).
fn assert_replays(live: &Live) {
    let output = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args([
            "replay",
            &live.session.display().to_string(),
            "--frames",
            &live.frames.to_string(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["frames"], live.frames);
    assert_eq!(json["skipped"], 0);
    assert_eq!(
        json["manifest"]["audio_fnv1a64"].as_str().unwrap(),
        audio_hash(&live.output),
        "signal replay renders what the null driver played"
    );

    let session = Session::load(&live.session, &registry()).unwrap();
    let replayed = replay(&session, &registry(), Some(live.frames)).unwrap();
    let samples: Vec<f32> = replayed.render.samples.iter().map(|&x| x as f32).collect();
    assert_eq!(samples, live.output);
}

/// The recorded commands, by kind: (events, edits, swaps).
fn kinds(edits: &[session::Edit]) -> (usize, usize, usize) {
    let count = |f: fn(&session::Command) -> bool| edits.iter().filter(|e| f(&e.command)).count();
    (
        count(|c| matches!(c, session::Command::Event { .. })),
        count(|c| matches!(c, session::Command::Edit { .. })),
        count(|c| matches!(c, session::Command::Swap { .. })),
    )
}

/// The recorder for the session fixtures, version 0.
fn live_recorder() -> LiveRecorder {
    let v0 = version(0);
    let recorder =
        SessionRecorder::new(&v0, &registry(), v0.sample_rate, v0.seed, provenance()).unwrap();
    LiveRecorder::new(recorder, registry())
}

/// The frame to start the session from: version 0's plan.
fn start(config: RealtimeConfig) -> (signal_engine::Runner, Controller) {
    let mut plan = version(0).plan(&registry()).unwrap().plan;
    plan.version = 0;
    realtime(&plan, &registry(), config).unwrap()
}

/// The sets a file-watching driver sends with a structural swap: survivors
/// whose file value changed.
fn sets(old: &Project, new: &Project) -> Vec<(PortAddress, EventPayload)> {
    match diff(old, new, &registry()).unwrap() {
        ProjectEdit::Structural { sets, .. } => {
            sets.into_iter().map(|s| (s.port, s.payload)).collect()
        }
        _ => panic!("expected a structural edit"),
    }
}

/// AC2, synchronous: sets, a ramp, a trigger, a late event, a structural swap
/// with the diff's sets, a removal, and a suspend and resume, all through the
/// Controller. The removal is the plan-review counterexample: the removal
/// swap (1540) is sent before the suspend it completes (1500), so reports
/// reach the recorder out of frame order. The null driver's output equals
/// `signal replay` of the recorded session. The run is long enough (5 s)
/// that callbacks follow the late event.
#[test]
fn a_session_issued_through_the_controller_replays_to_the_null_driver_output() {
    const FRAMES: u64 = 40_000;
    let mut live = live_recorder();
    let (runner, mut controller) = start(RealtimeConfig::new(8_000, 3));
    let mut driver = NullDriver::new(8_000, 2, 5);
    driver.start(runner).unwrap();

    let at = |frame| Stamp::At(Frame(frame));
    let mut first = true;
    let mut late_sent = false;
    driver
        .run(FRAMES, || {
            controller.poll(&mut live);
            if first {
                first = false;
                let c = &mut controller;
                c.send_event(port("/osc.freq"), EventPayload::Set(550.0), at(100))
                    .unwrap();
                let ramp = EventPayload::Ramp {
                    to: 0.25,
                    frames: 400,
                    curve: Curve::Linear,
                };
                c.send_event(port("/out.gain"), ramp, at(200)).unwrap();
                c.send_event(port("/env.trigger"), EventPayload::Trigger, at(250))
                    .unwrap();
                let added = live.swap_plan(version(1)).unwrap();
                c.swap(&added, at(1_000)).unwrap();
                for (target, payload) in sets(&version(0), &version(1)) {
                    c.send_event(target, payload, at(1_000)).unwrap();
                }
                c.send_event(port("/osc2.freq"), EventPayload::Set(990.0), at(1_000))
                    .unwrap();
                let removed = live.swap_plan(version(2)).unwrap();
                let removal = c.removal_swap_frame(Frame(1_500));
                assert_eq!(removal, Frame(1_540));
                c.swap(&removed, Stamp::At(removal)).unwrap();
                c.send_suspend(addr("/osc2"), at(1_500)).unwrap();
                c.send_suspend(addr("/env"), at(2_500)).unwrap();
                c.send_resume(addr("/env"), at(2_600)).unwrap();
            } else if !late_sent && controller.status().next_frame >= Frame(300) {
                // Stamped for a frame that has run: applies on the next one.
                late_sent = true;
                controller
                    .send_event(port("/osc.freq"), EventPayload::Set(500.0), at(290))
                    .unwrap();
            }
        })
        .unwrap();
    let runner = driver.stop().unwrap().unwrap();
    controller.poll(&mut live);
    // The final frontier from the runner itself, in case the last status was
    // lost (the status queue is lossy).
    live.frontier(runner.next_frame());
    assert!(late_sent);
    assert_eq!(live.refused(), &[] as &[String]);
    assert_eq!(live.pending(), 0);
    assert!(controller.status().late >= 1);

    let recorder = live.into_recorder();
    let edits = recorder.edits();
    // Events: 550, the ramp, the trigger, the diff's set, /osc2's set, the
    // late event; edits: the removal's suspend, /env's suspend and resume.
    assert_eq!(kinds(&edits), (6, 3, 2));
    let order: Vec<(u64, &str)> = edits
        .iter()
        .filter(|e| (1_000..=1_540).contains(&e.frame.0))
        .filter_map(|e| match &e.command {
            session::Command::Swap { .. } => Some((e.frame.0, "swap")),
            session::Command::Edit { .. } => Some((e.frame.0, "suspend")),
            _ => None,
        })
        .collect();
    assert_eq!(
        order,
        [(1_000, "swap"), (1_500, "suspend"), (1_540, "swap")]
    );

    let dir = out_dir("sync");
    recorder.write(&dir).unwrap();
    assert_replays(&Live {
        output: driver.take_output(),
        frames: FRAMES,
        session: dir,
    });
}

/// Waits, polling, until `done` holds; the driver keeps running meanwhile.
fn wait(controller: &mut Controller, live: &mut LiveRecorder, done: impl Fn(&Controller) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        controller.poll(live);
        if done(controller) {
            return;
        }
        assert!(Instant::now() < deadline, "the threaded driver stalled");
        thread::sleep(Duration::from_millis(1));
    }
}

/// AC2, threaded: the same kinds of command sent with the default stamp while
/// a paced null driver runs on its own thread, each step once the runner has
/// reported passing its frame. The driver runs until stopped, after every
/// command is recorded and one more second has run, so every command lies
/// inside the rendered frames.
#[test]
fn a_session_issued_while_the_driver_thread_runs_replays_to_its_output() {
    let mut live = live_recorder();
    let (runner, mut controller) = start(RealtimeConfig::with_buffer(8_000, 3, 256));
    let mut driver = NullDriver::new(8_000, 2, 29).threaded(true);
    driver.start(runner).unwrap();
    let c = &mut controller;
    let live = &mut live;
    let past = |frame: u64| move |c: &Controller| c.status().next_frame >= Frame(frame);

    wait(c, live, past(100));
    c.send_event(port("/osc.freq"), EventPayload::Set(550.0), Stamp::Default)
        .unwrap();
    c.send_event(port("/env.trigger"), EventPayload::Trigger, Stamp::Default)
        .unwrap();
    wait(c, live, past(400));
    let added = live.swap_plan(version(1)).unwrap();
    c.swap(&added, Stamp::Default).unwrap();
    for (target, payload) in sets(&version(0), &version(1)) {
        c.send_event(target, payload, Stamp::Default).unwrap();
    }
    c.send_event(port("/osc2.freq"), EventPayload::Set(990.0), Stamp::Default)
        .unwrap();
    wait(c, live, |c| c.status().version >= 1);
    let suspend = c.default_frame();
    c.send_suspend(addr("/osc2"), Stamp::At(suspend)).unwrap();
    let removed = live.swap_plan(version(2)).unwrap();
    c.swap(&removed, Stamp::At(c.removal_swap_frame(suspend)))
        .unwrap();
    wait(c, live, |c| c.status().version >= 2);
    c.send_suspend(addr("/env"), Stamp::Default).unwrap();
    let resume_after = c.status().next_frame.0 + 100;
    wait(c, live, past(resume_after));
    c.send_resume(addr("/env"), Stamp::Default).unwrap();
    // Everything recorded, then one more second.
    wait(c, live, |c| c.in_flight() == 0);
    let margin = c.status().next_frame.0 + 8_000;
    wait(c, live, past(margin));
    assert_eq!(live.pending(), 0);

    let runner = driver.stop().unwrap().unwrap();
    c.poll(live);
    live.frontier(runner.next_frame());
    let output = driver.take_output();
    let frames = runner.next_frame().0;
    assert_eq!(output.len() as u64, frames * 2);
    assert_eq!(live.refused(), &[] as &[String]);

    let recorder = std::mem::replace(live, live_recorder()).into_recorder();
    let edits = recorder.edits();
    assert_eq!(
        kinds(&edits),
        (4, 3, 2),
        "every scripted command was recorded"
    );
    assert!(edits.iter().all(|e| e.frame.0 < frames));
    let dir = out_dir("threaded");
    recorder.write(&dir).unwrap();
    assert_replays(&Live {
        output,
        frames,
        session: dir,
    });
}

/// The recorder records reports in the order the engine applied them, not
/// the order they arrive, and only once the runner is past their frame: the
/// plan-review counterexample (add at 100, removal swap at 200 reported
/// before the suspend at 150 it completes).
#[test]
fn the_live_recorder_records_in_frame_order_behind_the_frontier() {
    let mut live = live_recorder();
    let added = live.swap_plan(version(1)).unwrap();
    let removed = live.swap_plan(version(2)).unwrap();
    assert_eq!((added.version, removed.version), (1, 2));
    let report = |id, frame, outcome| Report {
        id,
        requested: Frame(frame),
        frame: Frame(frame),
        outcome,
    };
    let swap = |plan_version| SentCommand::Swap { plan_version };
    let suspend = SentCommand::Suspend(addr("/osc2"));
    live.report(&report(3, 150, Outcome::Scheduled), &suspend);
    live.report(&report(1, 100, Outcome::Committed { version: 1 }), &swap(1));
    live.frontier(Frame(150));
    assert_eq!(live.recorder().edits().len(), 1, "only the swap at 100");
    live.report(&report(2, 200, Outcome::Committed { version: 2 }), &swap(2));
    live.frontier(Frame(201));
    assert_eq!(live.refused(), &[] as &[String]);
    let frames: Vec<(u64, u64)> = live
        .recorder()
        .edits()
        .iter()
        .map(|e| (e.frame.0, e.seq))
        .collect();
    assert_eq!(frames, [(100, 1), (150, 3), (200, 2)]);
}
