//! M2.8-7: the physical-av demo (`projects/physical-av`) and the event
//! bridge into the visual-rate types. The demo's types and panel, its
//! floors, a recorded session that replays the visual streams bit for bit
//! (AC: replay), and the bridge's lifecycle across suspensions and swaps and
//! its arithmetic at the edges (operator decisions operator-request:007,
//! :017, :014 and :020; M2.8-10). The partition and thread matrix is in
//! `blocks.rs`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use signal_core::{EventPayload, Frame, PortAddress, Registry};
use signal_engine::{
    ControlSink, Engine, EngineConfig, EngineError, ExternalEvent, Provenance, RealtimeConfig,
    Stamp,
};
use signal_output::{
    Driver, NullDriver, Render, VectorTap, VectorTrack, realtime, render, render_blocks_tapped,
};
use signal_project::session::{LiveRecorder, ReplayOptions, Session, SessionRecorder, replay_with};
use signal_project::{Project, SourceFile};

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn demo() -> PathBuf {
    repository().join("projects/physical-av")
}

/// A fresh folder for one test, inside the target directory.
fn out_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("physical_av")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn port(text: &str) -> PortAddress {
    text.parse().unwrap()
}

fn tap(port: &str, rows: usize, decimation: u64) -> VectorTap {
    VectorTap {
        port: port.parse().unwrap(),
        rows,
        decimation,
    }
}

/// The demo uses the visual-rate physics, the bridge and the audio
/// membrane, each with the contract suite run on it in
/// `crates/signal-std/tests/contract.rs`; its panel's bindings check.
#[test]
fn the_project_uses_the_visual_physics_and_its_panel_checks() {
    let project = Project::load(&demo()).unwrap();
    assert_eq!((project.sample_rate, project.seed), (48_000, 7));
    let planned = project.plan(&registry()).unwrap();
    let used: BTreeSet<&str> = planned
        .plan
        .instances
        .iter()
        .map(|i| i.type_name)
        .filter(|t| !t.starts_with("core."))
        .collect();
    assert_eq!(
        used,
        BTreeSet::from([
            "std.abs",
            "std.audio_out",
            "std.body2d",
            "std.clock_div",
            "std.count",
            "std.dc_block",
            "std.lag",
            "std.mesh2d",
            "std.mesh2d_audio",
            "std.particles",
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
    let output = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(["ui", "check", &demo().display().to_string()])
        .output()
        .unwrap();
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("{said}");
    assert!(output.status.success(), "{said}");
    assert!(said.contains("13 controls, 14 bindings: ok"), "{said}");
}

/// Two bars at 48 kHz: every sample finite, the 4× oversampled peak at or
/// below −1 dBFS and |mean| < 1e-3 on each channel.
#[test]
fn renders_within_the_floors() {
    let project = Project::load(&demo()).unwrap();
    let registry = registry();
    let plan = project.plan(&registry).unwrap().plan;
    let config = EngineConfig::new(project.sample_rate, project.seed);
    let mut engine = Engine::new(&plan, &registry, config, Vec::new()).unwrap();
    // 8 beats at 90 bpm.
    let rendered: Render = render(&mut engine, 8 * 32_000).unwrap();
    let samples: Vec<f32> = rendered.samples.iter().map(|&x| x as f32).collect();
    assert!(samples.iter().all(|x| x.is_finite()));
    let metrics = signal_eval::metrics(&samples, rendered.channels);
    let peaks = signal_eval::true_peak_4x(&samples, rendered.channels);
    let ceiling = 10f64.powf(-1.0 / 20.0);
    for (channel, (m, &peak)) in metrics.channels.iter().zip(&peaks).enumerate() {
        println!(
            "physical-av channel {channel}: 4x peak {peak:.6} ({:.3} dBFS), mean {:e}",
            20.0 * peak.log10(),
            m.mean
        );
        assert!(peak > 0.1, "the membrane is heard: {peak}");
        assert!(peak <= ceiling, "channel {channel}: {peak}");
        assert!(m.mean.abs() < 1e-3, "channel {channel}: {}", m.mean);
    }
}

fn provenance() -> Provenance {
    Provenance {
        signal_revision: "physical-av-test".to_owned(),
        signal_dirty: Some(false),
        rustc_version: "rustc (test)".to_owned(),
        project_file_hashes: Default::default(),
        library_composite_hashes: Default::default(),
        cargo_lock_hash: "0000000000000000".to_owned(),
    }
}

/// AC (replay): the demo played through the real-time runner and the null
/// driver, with strikes and a set issued through the Controller and the
/// session recorded, while the vector adapter streams the membrane seen, the
/// sparks and (started a second later, mid-run) the bodies. The replay of
/// the recorded session, its taps at the same decimation, reproduces every
/// streamed snapshot bit for bit (as `f32`, as the stream carries it), at
/// the same frames, all on ticks; and the audio. Nothing was dropped.
#[test]
fn a_recorded_session_replays_the_visual_streams_bit_for_bit() {
    const FRAMES: u64 = 4 * 48_000;
    let registry = registry();
    let project = Project::load(&demo()).unwrap();
    let (rate, seed) = (project.sample_rate, project.seed);
    let mut plan = project.plan(&registry).unwrap().plan;
    plan.version = 0;
    let recorder = SessionRecorder::new(&project, &registry, rate, seed, provenance()).unwrap();
    let mut live = LiveRecorder::new(recorder, registry.clone());
    let (runner, mut controller) =
        realtime(&plan, &registry, RealtimeConfig::new(rate, seed)).unwrap();
    let streams = [
        ("/membrane_v.z", 256),
        ("/sparks.pos", 128),
        ("/sparks.life", 64),
        ("/bodies.bodies", 24),
    ];
    for (slot, &(p, rows)) in streams.iter().enumerate().take(3) {
        controller
            .vector_start(&plan, slot, port(p), rows, 800)
            .unwrap();
    }
    let mut driver = NullDriver::new(rate, 2, 3);
    driver.start(runner).unwrap();
    let mut live_snapshots: Vec<(usize, u64, Vec<f32>)> = Vec::new();
    let mut first = true;
    let mut bodies_started = false;
    driver
        .run(FRAMES, || {
            controller.poll(&mut live);
            controller
                .poll_vectors(|s| live_snapshots.push((s.slot, s.frame.0, s.values().to_vec())));
            if first {
                first = false;
                // Three strikes on top of the beat: turned down so nothing
                // clips (the device clamps, the replay's audio would not).
                controller
                    .send_event(
                        port("/out.gain"),
                        EventPayload::Set(0.5),
                        Stamp::At(Frame(10)),
                    )
                    .unwrap();
                for at in [20_000, 50_001, 50_002] {
                    controller
                        .send_event(port("/hub.in"), EventPayload::Trigger, Stamp::At(Frame(at)))
                        .unwrap();
                }
                controller
                    .send_event(
                        port("/sparks.speed"),
                        EventPayload::Set(1.6),
                        Stamp::At(Frame(70_000)),
                    )
                    .unwrap();
            } else if !bodies_started && controller.status().next_frame >= Frame(48_000) {
                bodies_started = true;
                let (p, rows) = streams[3];
                controller
                    .vector_start(&plan, 3, port(p), rows, 800)
                    .unwrap();
            }
        })
        .unwrap();
    let runner = driver.stop().unwrap().unwrap();
    controller.poll(&mut live);
    controller.poll_vectors(|s| live_snapshots.push((s.slot, s.frame.0, s.values().to_vec())));
    live.frontier(runner.next_frame());
    assert_eq!(controller.vector_overflows(), 0);
    assert_eq!(live.refused(), &[] as &[String]);
    assert_eq!(live.pending(), 0);
    let output = driver.take_output();

    let dir = out_dir("session");
    live.into_recorder().write(&dir).unwrap();
    let session = Session::load(&dir, &registry).unwrap();
    let options = ReplayOptions {
        frames: Some(FRAMES),
        threads: 1,
        vectors: streams.iter().map(|&(p, rows)| tap(p, rows, 800)).collect(),
        ..ReplayOptions::default()
    };
    let replayed = replay_with(&session, &registry, options).unwrap();
    let samples: Vec<f32> = replayed.render.samples.iter().map(|&x| x as f32).collect();
    let first = samples
        .iter()
        .zip(&output)
        .position(|(a, b)| a.to_bits() != b.to_bits());
    assert_eq!(samples.len(), output.len());
    assert!(
        first.is_none(),
        "the replay renders what the null driver played; first difference at sample {first:?}: {:?}",
        first.map(|k| (samples[k], output[k]))
    );

    for (slot, &(p, _)) in streams.iter().enumerate() {
        let replay: BTreeMap<u64, &Vec<f64>> = replayed.vectors[slot]
            .snapshots
            .iter()
            .map(|(f, rows)| (f.0, rows))
            .collect();
        let streamed: Vec<&(usize, u64, Vec<f32>)> =
            live_snapshots.iter().filter(|s| s.0 == slot).collect();
        println!(
            "{p}: {} snapshots streamed, {} replayed",
            streamed.len(),
            replay.len()
        );
        assert!(streamed.len() >= 100, "{p}: {}", streamed.len());
        let mut moving = false;
        for (_, frame, values) in &streamed {
            assert_eq!(frame % 800, 0, "{p}: a snapshot off the ticks");
            let rows = replay
                .get(frame)
                .unwrap_or_else(|| panic!("{p}: no replay at {frame}"));
            let replayed: Vec<u32> = rows.iter().map(|&x| (x as f32).to_bits()).collect();
            let played: Vec<u32> = values.iter().map(|x| x.to_bits()).collect();
            assert_eq!(played, replayed, "{p} at frame {frame}");
            moving |= values != &streamed[0].2;
        }
        assert!(moving, "{p} never moved");
    }
}

/// A one-file project at 48 kHz.
fn project(instances: &str, connections: &str) -> Project {
    let text = format!(
        "schema_version = 1\n\n[project]\nname = \"bridge\"\nsample_rate = 48000\nseed = 5\n\n[instances]\n{instances}\n[wiring]\nconnections = [\n{connections}]\n"
    );
    Project::from_sources(vec![SourceFile {
        path: "project.toml".to_owned(),
        text,
    }])
    .unwrap()
}

/// What a scripted run does at a frame.
enum Step {
    Event(&'static str, EventPayload),
    Suspend(&'static str),
    Resume(&'static str),
    Swap(usize),
}

/// Renders `frames` of `plans[0]` with `script` (frame, step) pushed as a
/// driver does, swapping to `plans[n]` where it says, and returns the log
/// and the taps' tracks.
fn scripted(
    plans: &[Project],
    script: &[(u64, Step)],
    frames: u64,
    taps: &[VectorTap],
) -> (String, Vec<VectorTrack>) {
    let registry = registry();
    let mut built: Vec<_> = plans
        .iter()
        .map(|p| p.plan(&registry).unwrap().plan)
        .collect();
    for (version, plan) in built.iter_mut().enumerate() {
        plan.version = version as u64;
    }
    let config = EngineConfig::new(48_000, 5);
    let mut engine = Engine::new(&built[0], &registry, config, Vec::new()).unwrap();
    let mut next = 0;
    let before = |engine: &mut Engine| -> Result<Option<Frame>, EngineError> {
        engine.reclaim_externals().for_each(drop);
        engine.reclaim_edits().for_each(drop);
        let now = engine.next_frame();
        while let Some((_, step)) = script.get(next).filter(|(at, _)| *at == now.0) {
            next += 1;
            match step {
                Step::Event(target, payload) => engine
                    .push_external(ExternalEvent {
                        frame: now,
                        sequence: next as u64,
                        target: port(target),
                        payload: *payload,
                    })
                    .unwrap(),
                Step::Suspend(address) => {
                    engine.push_suspend(address.parse().unwrap(), now).unwrap()
                }
                Step::Resume(address) => engine.push_resume(address.parse().unwrap(), now).unwrap(),
                Step::Swap(v) => {
                    let mut swap = engine.prepare_swap(&built[*v], &registry)?;
                    engine.commit_swap(&mut swap, now)?;
                }
            }
        }
        Ok(script.get(next).map(|(at, _)| Frame(*at)))
    };
    // The plan of each engine version: the first, then one per swap in order.
    let by_version: Vec<_> = std::iter::once(built[0].clone())
        .chain(script.iter().filter_map(|(_, step)| match step {
            Step::Swap(v) => Some(built[*v].clone()),
            _ => None,
        }))
        .collect();
    let (rendered, tracks) =
        render_blocks_tapped(&mut engine, frames, 256, before, taps, &by_version).unwrap();
    (String::from_utf8(rendered.log).unwrap(), tracks)
}

/// Live particles per tick of a `life` track.
fn live(track: &VectorTrack) -> BTreeMap<u64, usize> {
    track
        .snapshots
        .iter()
        .map(|(f, life)| (f.0, life.iter().filter(|&&l| l > 0.0).count()))
        .collect()
}

const COUNTER_AND_SPARKS: &str = "c = { module = \"std.count\" }\nsparks = { module = \"std.particles\", lifetime_s = 60, gravity = 0 }\nout = { module = \"std.audio_out\" }\n";
const COUNTER_AND_SPARKS2: &str = "c = { module = \"std.count\" }\nsparks2 = { module = \"std.particles\", lifetime_s = 60, gravity = 0 }\nout = { module = \"std.audio_out\" }\n";

/// AC7, a new consumer: one replaced (removed, another added) at frame
/// 1000 applies nothing the counter counted before its first tick (1600):
/// not the 50 events at frame 100 its predecessor applied at tick 800, and
/// not 900 and 1100, at most one tick's worth (`std.count`, "A new or
/// replaced consumer"); the event at 2000 acts at 2400. With the removal
/// protocol (suspend at 700, swap at 940), the suspended consumer skips
/// tick 800 and is removed, and the new one applies nothing at 1600 either.
#[test]
fn a_new_consumer_applies_no_history_at_its_first_tick() {
    let v0 = || {
        project(
            COUNTER_AND_SPARKS,
            "  { from = \"c.window\", to = \"sparks.emit\" },\n",
        )
    };
    let v1 = || {
        project(
            COUNTER_AND_SPARKS2,
            "  { from = \"c.window\", to = \"sparks2.emit\" },\n",
        )
    };
    let taps = [tap("/sparks.life", 64, 800), tap("/sparks2.life", 64, 800)];
    let mut script: Vec<(u64, Step)> = (0..50)
        .map(|_| (100, Step::Event("/c.in", EventPayload::Trigger)))
        .collect();
    script.push((900, Step::Event("/c.in", EventPayload::Trigger)));
    script.push((1000, Step::Swap(1)));
    script.push((1100, Step::Event("/c.in", EventPayload::Trigger)));
    script.push((2000, Step::Event("/c.in", EventPayload::Trigger)));
    let (_, tracks) = scripted(&[v0(), v1()], &script, 3200, &taps);
    let (old, new) = (live(&tracks[0]), live(&tracks[1]));
    assert_eq!(old.get(&800), Some(&50));
    assert_eq!(old.get(&1600), None, "removed");
    assert_eq!(new.get(&1600), Some(&0), "{new:?}");
    assert_eq!(new.get(&2400), Some(&1), "{new:?}");

    let script = vec![
        (700, Step::Suspend("/sparks")),
        (750, Step::Event("/c.in", EventPayload::Trigger)),
        (940, Step::Swap(1)),
        (1000, Step::Event("/c.in", EventPayload::Trigger)),
    ];
    let (_, tracks) = scripted(&[v0(), v1()], &script, 2400, &taps);
    let (old, new) = (live(&tracks[0]), live(&tracks[1]));
    assert_eq!(old.get(&800), Some(&0), "suspended across tick 800");
    assert_eq!(new.get(&1600), Some(&0), "{new:?}");
}

const PARTICLES_A: &str =
    "a = { module = \"std.particles\", rate = 200 }\nout = { module = \"std.audio_out\" }\n";
const BODIES_A: &str = "a = { module = \"std.body2d\" }\nout = { module = \"std.audio_out\" }\n";

/// Finding auditor-1-007, replay: a swap at frame 1000 retypes `/a` from
/// std.particles (`vel`: 128 elements) to std.body2d (`vel`: 12). A tap
/// of 128 rows stops at the swap instead of reading the bodies' and other
/// ports' rows; a tap of 12 rows starts there. Every snapshot has exactly
/// its port's elements.
#[test]
fn a_tap_never_reads_another_port_after_a_swap_changes_its_width() {
    let script = vec![(1000, Step::Swap(1))];
    let taps = [tap("/a.vel", 128, 800), tap("/a.vel", 12, 800)];
    let (_, tracks) = scripted(
        &[project(PARTICLES_A, ""), project(BODIES_A, "")],
        &script,
        3200,
        &taps,
    );
    let frames = |t: &VectorTrack| t.snapshots.iter().map(|(f, _)| f.0).collect::<Vec<_>>();
    assert_eq!(frames(&tracks[0]), vec![0, 800]);
    assert_eq!(frames(&tracks[1]), vec![1600, 2400]);
    assert!(
        tracks[0]
            .snapshots
            .iter()
            .all(|(_, rows)| rows.len() == 128)
    );
    assert!(tracks[1].snapshots.iter().all(|(_, rows)| rows.len() == 12));
}

/// Finding auditor-1-007, live: the same swap through the Controller and
/// the null driver. The slot of 128 rows becomes unbound at the swap and
/// streams nothing more; started again with the body's 12 rows, it streams
/// those. No snapshot ever carries rows of another port.
#[test]
fn a_live_stream_is_unbound_when_a_swap_changes_its_width() {
    let registry = registry();
    let v0 = project(PARTICLES_A, "");
    let mut plan = v0.plan(&registry).unwrap().plan;
    plan.version = 0;
    let mut swapped = project(BODIES_A, "").plan(&registry).unwrap().plan;
    swapped.version = 1;
    let (runner, mut controller) =
        realtime(&plan, &registry, RealtimeConfig::new(48_000, 5)).unwrap();
    controller
        .vector_start(&plan, 0, port("/a.vel"), 128, 800)
        .unwrap();
    let mut driver = NullDriver::new(48_000, 2, 3);
    driver.start(runner).unwrap();
    let mut snapshots: Vec<(u32, u64, usize)> = Vec::new();
    let mut swap_sent = false;
    let mut restarted = false;
    let mut unbound_seen = false;
    driver
        .run(12_000, || {
            controller.poll(&mut ());
            controller
                .poll_vectors(|s| snapshots.push((s.generation, s.frame.0, s.values().len())));
            let next = controller.status().next_frame;
            if !swap_sent {
                swap_sent = true;
                controller.swap(&swapped, Stamp::At(Frame(1000))).unwrap();
            } else if !restarted && controller.status().version == 1 && next >= Frame(2_000) {
                unbound_seen = !controller.vector_bound(0);
                restarted = true;
                controller
                    .vector_start(&swapped, 0, port("/a.vel"), 12, 800)
                    .unwrap();
            }
        })
        .unwrap();
    drop(driver.stop().unwrap());
    controller.poll_vectors(|s| snapshots.push((s.generation, s.frame.0, s.values().len())));
    assert!(
        restarted && unbound_seen,
        "the 128-row slot was unbound after the swap"
    );
    let before: Vec<_> = snapshots.iter().filter(|s| s.2 == 128).collect();
    let after: Vec<_> = snapshots.iter().filter(|s| s.2 == 12).collect();
    println!(
        "{} snapshots of 128 rows, {} of 12",
        before.len(),
        after.len()
    );
    assert!(before.iter().all(|s| s.1 < 1000), "{before:?}");
    assert!(!before.is_empty() && after.len() >= 5);
    assert!(after.iter().all(|s| s.1 >= 1000));
    assert_eq!(before.len() + after.len(), snapshots.len());
}

/// Every snapshot of a track, bit for bit.
fn bits(track: &VectorTrack) -> Vec<(u64, Vec<u64>)> {
    track
        .snapshots
        .iter()
        .map(|(f, rows)| (f.0, rows.iter().map(|x| x.to_bits()).collect()))
        .collect()
}

/// Asserts that `script` on `plans` gives the taps' tracks of `reference`
/// on `plans[0]`, a run without edits whose events are exactly the ones the
/// bridge applies (M2.8-10: what was applied twice, or not at all, shows).
fn matches_reference(
    plans: &[Project],
    script: &[(u64, Step)],
    reference: &[(u64, Step)],
    frames: u64,
    taps: &[VectorTap],
) -> (String, Vec<VectorTrack>) {
    let (log, tracks) = scripted(plans, script, frames, taps);
    let (_, expected) = scripted(&plans[..1], reference, frames, taps);
    for ((track, expected), tap) in tracks.iter().zip(&expected).zip(taps) {
        assert_eq!(bits(track), bits(expected), "{}", tap.port);
    }
    (log, tracks)
}

fn trigger(at: u64, target: &'static str) -> (u64, Step) {
    (at, Step::Event(target, EventPayload::Trigger))
}

fn set(at: u64, target: &'static str, v: f64) -> (u64, Step) {
    (at, Step::Event(target, EventPayload::Set(v)))
}

const FAN_OUT: &str = "c = { module = \"std.count\" }\np = { module = \"std.particles\", lifetime_s = 60, gravity = 0 }\nm = { module = \"std.mesh2d\" }\nb = { module = \"std.body2d\", gravity = 0, kick_speed = 0.05 }\nout = { module = \"std.audio_out\" }\n";
const FAN_OUT_WIRING: &str = "  { from = \"c.window\", to = \"p.emit\" },\n  { from = \"c.window\", to = \"p.impulse\" },\n  { from = \"c.window\", to = \"m.strike\" },\n  { from = \"c.window\", to = \"b.kick\" },\n";

fn fan_out_taps() -> [VectorTap; 4] {
    [
        tap("/p.life", 64, 800),
        tap("/p.vel", 128, 800),
        tap("/m.z", 256, 800),
        tap("/b.vel", 12, 800),
    ]
}

/// AC2 (M2.8-10, scenario B of the issue): an event at 700, then the
/// counter suspended at 750 for 40 ticks. Every consumer (emit, impulse,
/// strike, kick) applies it once at tick 800 and nothing more while the
/// counter holds its reading: the particles, the membrane and the bodies
/// equal a run in which the counter is never suspended, at every tick. The
/// event sent while it is suspended is discarded by the engine (logged);
/// after the resume, two new events (non-dyadic values) are applied once.
#[test]
fn a_suspended_counter_applies_its_events_once() {
    let resume = 750 + 40 * 800;
    let script = vec![
        trigger(700, "/c.in"),
        (750, Step::Suspend("/c")),
        trigger(10_000, "/c.in"),
        (resume, Step::Resume("/c")),
        set(resume + 250, "/c.in", 0.1),
        set(resume + 350, "/c.in", 0.2),
    ];
    let reference = vec![
        trigger(700, "/c.in"),
        set(resume + 250, "/c.in", 0.1),
        set(resume + 350, "/c.in", 0.2),
    ];
    let (log, tracks) = matches_reference(
        &[project(FAN_OUT, FAN_OUT_WIRING)],
        &script,
        &reference,
        36_000,
        &fan_out_taps(),
    );
    assert!(
        log.contains(r#""type":"discard","frame":10000,"target":"/c.in""#),
        "{log}"
    );
    let emitted = live(&tracks[0]);
    assert!(
        (800..33_600).step_by(800).all(|t| emitted[&t] == 1),
        "{emitted:?}"
    );
    assert_eq!(emitted[&33_600], 3);
}

/// AC6: the summed values across a suspension are exact at 2⁻³²: the kicks
/// equal those of a run without the suspension (the bodies' velocities, bit
/// for bit), with non-dyadic values before it and after it.
#[test]
fn payloads_across_a_suspension_are_exact() {
    let bodies = "c = { module = \"std.count\" }\nb = { module = \"std.body2d\", gravity = 0, kick_speed = 1 }\nout = { module = \"std.audio_out\" }\n";
    let wiring = "  { from = \"c.window\", to = \"b.kick\" },\n";
    let script = vec![
        set(100, "/c.in", 0.1),
        set(300, "/c.in", 0.2),
        (900, Step::Suspend("/c")),
        set(5_000, "/c.in", 7.0),
        (8_001, Step::Resume("/c")),
        set(8_100, "/c.in", -0.05),
        set(8_200, "/c.in", 1.0 / 3.0),
    ];
    let reference = vec![
        set(100, "/c.in", 0.1),
        set(300, "/c.in", 0.2),
        set(8_100, "/c.in", -0.05),
        set(8_200, "/c.in", 1.0 / 3.0),
    ];
    let (_, tracks) = matches_reference(
        &[project(bodies, wiring)],
        &script,
        &reference,
        10_400,
        &[tap("/b.vel", 12, 800)],
    );
    let vel = |t: u64| tracks[0].snapshots[(t / 800) as usize].1.clone();
    assert_ne!(vel(800), vel(0), "the kick at 800 acted");
    assert_ne!(vel(8_800), vel(8_000), "the kick at 8800 acted");
}

const COUNTER_AND_P: &str = "c = { module = \"std.count\" }\np = { module = \"std.particles\", lifetime_s = 60, gravity = 0 }\nout = { module = \"std.audio_out\" }\n";
const C2_AND_P: &str = "c2 = { module = \"std.count\" }\np = { module = \"std.particles\", lifetime_s = 60, gravity = 0 }\nout = { module = \"std.audio_out\" }\n";
const ONLY_P: &str = "p = { module = \"std.particles\", lifetime_s = 60, gravity = 0 }\nout = { module = \"std.audio_out\" }\n";
const C_TO_P: &str = "  { from = \"c.window\", to = \"p.emit\" },\n";
const C2_TO_P: &str = "  { from = \"c2.window\", to = \"p.emit\" },\n";

/// The removal protocol's fade length at 48 kHz: 5 ms.
const F: u64 = 240;

/// AC3: the removal protocol (suspend the counter at *s*, swap at *s* + F)
/// for *s* at every phase of a tick, 1601 to 2400, (2160, 2400] included:
/// (i) replaced by another counter, (ii) removed, (iii) removed and added
/// again at the same address in a later swap. A (1590) is applied once at
/// 1600; B (*s* − 1) once if a tick lies in [*s* − 1, *s* + F), else not at
/// all; the new counter's events before the consumer's first tick of it (D)
/// are not applied, those after it (E) once. Nothing is applied twice; what
/// is lost lies within one tick and only because the counter went.
#[test]
fn the_removal_protocol_never_applies_an_event_twice() {
    let plans_i = [project(COUNTER_AND_P, C_TO_P), project(C2_AND_P, C2_TO_P)];
    let plans_ii = [project(COUNTER_AND_P, C_TO_P), project(ONLY_P, "")];
    let plans_iii = [
        project(COUNTER_AND_P, C_TO_P),
        project(ONLY_P, ""),
        project(COUNTER_AND_P, C_TO_P),
    ];
    let taps = [tap("/p.life", 64, 800)];
    let ceil = |f: u64| f.div_ceil(800) * 800;
    let started = std::time::Instant::now();
    for s in 1601..=2400u64 {
        let b_applied = (s - 1..s + F).any(|f| f % 800 == 0);
        let base = 1 + usize::from(b_applied);
        let prefix = || {
            vec![
                trigger(1590, "/c.in"),
                trigger(s - 1, "/c.in"),
                (s, Step::Suspend("/c")),
                (s + F, Step::Swap(1)),
            ]
        };
        // (i) Replaced: c2 runs from s + F and is first read at t.
        let t = ceil(s + F);
        let mut replaced = prefix();
        replaced.push(trigger(s + F + 1, "/c2.in"));
        replaced.push(trigger(t + 1, "/c2.in"));
        let d_applied = t == s + F;
        // (ii) Removed.
        let removed = prefix();
        // (iii) Added again at s + F + 1 and first read at t3.
        let t3 = ceil(s + F + 1);
        let mut readded = prefix();
        readded.push((s + F + 1, Step::Swap(2)));
        readded.push(trigger(s + F + 2, "/c.in"));
        readded.push(trigger(t3 + 1, "/c.in"));
        readded.sort_by_key(|(f, _)| *f);
        let d3_applied = t3 == s + F + 1;
        for (plans, script, expected) in [
            (&plans_i[..], replaced, base + usize::from(d_applied) + 1),
            (&plans_ii[..], removed, base),
            (&plans_iii[..], readded, base + usize::from(d3_applied) + 1),
        ] {
            let (_, tracks) = scripted(plans, &script, 4001, &taps);
            let counts = live(&tracks[0]);
            assert_eq!(counts[&1600], 1 + usize::from(s == 1601), "s = {s}");
            assert_eq!(counts[&4000], expected, "s = {s}: {counts:?}");
            let mut previous = 0;
            for (&tick, &n) in &counts {
                assert!(n >= previous, "s = {s}: fewer particles at {tick}");
                previous = n;
            }
        }
    }
    println!(
        "the removal protocol at 800 phases x 3 variants: {:?}",
        started.elapsed()
    );

    // Scenario A of the issue: an event at 795, the counter suspended at
    // 1590 and removed at 1830: applied once at 800, not again at 1600.
    let script = vec![
        trigger(795, "/c.in"),
        (1590, Step::Suspend("/c")),
        (1590 + F, Step::Swap(1)),
    ];
    let (_, tracks) = scripted(&plans_ii, &script, 3201, &taps);
    let counts = live(&tracks[0]);
    assert_eq!((counts[&800], counts[&1600], counts[&3200]), (1, 1, 1));
}

/// AC4: a consumer suspended from 700 to 4001, across four ticks, applies
/// every event of that time once, at its first tick after the resume
/// (4800): four emissions and the kicks of a run in which the four events
/// arrive after the resume.
#[test]
fn a_suspended_consumer_applies_every_event_once() {
    let instances = "c = { module = \"std.count\" }\np = { module = \"std.particles\", lifetime_s = 60, gravity = 0 }\nb = { module = \"std.body2d\", gravity = 0, kick_speed = 0.05 }\nout = { module = \"std.audio_out\" }\n";
    let wiring = "  { from = \"c.window\", to = \"p.emit\" },\n  { from = \"c.window\", to = \"b.kick\" },\n";
    let edits = || {
        vec![
            (700, Step::Suspend("/p")),
            (700, Step::Suspend("/b")),
            (4001, Step::Resume("/p")),
            (4001, Step::Resume("/b")),
        ]
    };
    let mut script = edits();
    script.extend([
        trigger(750, "/c.in"),
        trigger(1500, "/c.in"),
        trigger(3000, "/c.in"),
        trigger(4100, "/c.in"),
        trigger(5000, "/c.in"),
    ]);
    script.sort_by_key(|(f, _)| *f);
    let mut reference = edits();
    reference.extend((0..4).map(|_| trigger(4100, "/c.in")));
    reference.push(trigger(5000, "/c.in"));
    reference.sort_by_key(|(f, _)| *f);
    let taps = [
        tap("/p.life", 64, 800),
        tap("/p.vel", 128, 800),
        tap("/b.vel", 12, 800),
    ];
    let plans = [project(instances, wiring)];
    let (_, tracks) = scripted(&plans, &script, 6401, &taps);
    let (_, expected) = scripted(&plans, &reference, 6401, &taps);
    for ((track, expected), tap) in tracks.iter().zip(&expected).zip(&taps) {
        assert_eq!(bits(track), bits(expected), "{}", tap.port);
    }
    let counts = live(&tracks[0]);
    assert_eq!((counts[&4000], counts[&4800], counts[&5600]), (0, 4, 5));
}

/// Finding auditor-2-reconnect-replays-disconnected-period: the kick input
/// disconnected at 1000 (a swap between survivors) and reconnected at
/// 10000 applies nothing the counter counted meanwhile: the bodies equal a
/// run without those events.
#[test]
fn a_reconnected_input_replays_nothing() {
    let instances = "c = { module = \"std.count\" }\nb = { module = \"std.body2d\" }\nout = { module = \"std.audio_out\" }\n";
    let wired = || project(instances, "  { from = \"c.window\", to = \"b.kick\" },\n");
    let script = vec![
        trigger(100, "/c.in"),
        (1000, Step::Swap(1)),
        trigger(2000, "/c.in"),
        trigger(6000, "/c.in"),
        trigger(9000, "/c.in"),
        (10_000, Step::Swap(2)),
        trigger(10_500, "/c.in"),
    ];
    let reference = vec![trigger(100, "/c.in"), trigger(10_500, "/c.in")];
    matches_reference(
        &[wired(), project(instances, ""), wired()],
        &script,
        &reference,
        12_001,
        &[tap("/b.vel", 12, 800)],
    );
}

/// Exception E2 (operator-request:014): a counter suspended at its second
/// frame keeps showing its first frame, so a consumer added later (at
/// 10000) applies that frame's event once, at its first tick, and never
/// again. A counter suspended later in its life gives a new consumer
/// nothing.
#[test]
fn a_new_consumer_meets_a_counter_held_at_its_first_frame() {
    let alone = "c = { module = \"std.count\" }\nout = { module = \"std.audio_out\" }\n";
    let plans = [project(alone, ""), project(COUNTER_AND_P, C_TO_P)];
    let taps = [tap("/p.life", 64, 800)];
    let script = vec![
        set(0, "/c.in", 100.0),
        (1, Step::Suspend("/c")),
        (10_000, Step::Swap(1)),
    ];
    let (_, tracks) = scripted(&plans, &script, 12_001, &taps);
    let counts = live(&tracks[0]);
    assert_eq!((counts[&10_400], counts[&12_000]), (1, 1), "{counts:?}");

    let script = vec![
        trigger(700, "/c.in"),
        (750, Step::Suspend("/c")),
        (10_000, Step::Swap(1)),
    ];
    let (_, tracks) = scripted(&plans, &script, 12_001, &taps);
    let counts = live(&tracks[0]);
    assert_eq!((counts[&10_400], counts[&12_000]), (0, 0), "{counts:?}");
}

/// Exception E1 (operator-request:014): a consumer suspended from 700 to
/// 4001, across its counter's replacement at 1000, loses what the new
/// counter counted before its first tick after the resume (1100 to 3500),
/// and applies nothing twice; the event at 4900 acts at 5600.
#[test]
fn a_consumer_suspended_across_a_counter_replacement() {
    let plans = [project(COUNTER_AND_P, C_TO_P), project(C2_AND_P, C2_TO_P)];
    let script = vec![
        (700, Step::Suspend("/p")),
        (1000, Step::Swap(1)),
        trigger(1100, "/c2.in"),
        trigger(1900, "/c2.in"),
        trigger(2700, "/c2.in"),
        trigger(3500, "/c2.in"),
        (4001, Step::Resume("/p")),
        trigger(4900, "/c2.in"),
    ];
    let (_, tracks) = scripted(&plans, &script, 6401, &[tap("/p.life", 64, 800)]);
    let counts = live(&tracks[0]);
    assert_eq!(
        (counts[&4800], counts[&5600], counts[&6400]),
        (0, 1, 1),
        "{counts:?}"
    );
}

const COUNTER_AND_BODIES: &str = "c = { module = \"std.count\" }\nb = { module = \"std.body2d\", gravity = 0, kick_speed = 0.01 }\nout = { module = \"std.audio_out\" }\n";
const ONLY_BODIES: &str = "b = { module = \"std.body2d\", gravity = 0, kick_speed = 0.01 }\nout = { module = \"std.audio_out\" }\n";
const C_TO_B: &str = "  { from = \"c.window\", to = \"b.kick\" },\n";

/// Finding auditor-1-003 (operator-request:020): a counter removed and
/// added again at the same address, between two ticks of its consumer, is
/// a new counter (its creation generation differs), so the consumer never
/// subtracts one instance's sum from another's. (a) The round-1 script;
/// (b) report 011's, whose two instances have identical first event
/// frames. Both give nothing at the consumer's first tick of the new
/// counter, then its later events once: the bodies equal a run without the
/// edits whose events are the ones applied.
#[test]
fn a_counter_re_added_at_its_address_is_a_new_counter() {
    let plans = [
        project(COUNTER_AND_BODIES, C_TO_B),
        project(ONLY_BODIES, ""),
        project(COUNTER_AND_BODIES, C_TO_B),
    ];
    let taps = [tap("/b.vel", 12, 800)];
    let script = vec![
        set(0, "/c.in", 100.0),
        (10, Step::Suspend("/c")),
        (250, Step::Swap(1)),
        (300, Step::Swap(2)),
        set(400, "/c.in", 1.0),
        set(600, "/c.in", 1.0),
        set(1000, "/c.in", 1.0),
    ];
    let reference = vec![set(0, "/c.in", 100.0), set(1000, "/c.in", 1.0)];
    matches_reference(&plans, &script, &reference, 3201, &taps);

    let script = vec![
        set(0, "/c.in", 100.0),
        set(50, "/c.in", 900.0),
        (100, Step::Suspend("/c")),
        (801, Step::Resume("/c")),
        (802, Step::Suspend("/c")),
        (1042, Step::Swap(1)),
        (1100, Step::Swap(2)),
        set(1100, "/c.in", 100.0),
        set(1200, "/c.in", 1.0),
        set(1300, "/c.in", 1.0),
        set(1700, "/c.in", 1.0),
    ];
    let reference = vec![
        set(0, "/c.in", 100.0),
        set(50, "/c.in", 900.0),
        set(1700, "/c.in", 1.0),
    ];
    matches_reference(&plans, &script, &reference, 3201, &taps);
}

/// The one accepted loss (operator-request:017, M2.8-10): a counter
/// replaced at frame 1000 (a new instance) loses the events it counted
/// after its consumer's last tick (900) and those the new counter counted
/// before the consumer's first tick of it (1100): within one tick. The new
/// counter's event at 1700 acts at 2400.
#[test]
fn a_replaced_counter_loses_at_most_one_tick() {
    let plans = [project(COUNTER_AND_P, C_TO_P), project(C2_AND_P, C2_TO_P)];
    let script = vec![
        trigger(900, "/c.in"),
        (1000, Step::Swap(1)),
        trigger(1100, "/c2.in"),
        trigger(1700, "/c2.in"),
    ];
    let (_, tracks) = scripted(&plans, &script, 2401, &[tap("/p.life", 64, 800)]);
    let counts = live(&tracks[0]);
    assert_eq!(
        (counts[&800], counts[&1600], counts[&2400]),
        (0, 0, 1),
        "{counts:?}"
    );
}

/// Finding auditor-1-005, with M2.8-10's documented range: two Set(1e308)
/// are limited to 2⁶⁴ each, so at strength 2⁻⁶⁰ the visual membrane's
/// strike node moves by exactly `g·h·32` at tick 800 and nothing faults. At
/// strength 1 it moves beyond the fault guard: the membrane faults
/// (`magnitude`, logged) at tick 800 while the rest of the run, a second
/// counter's particles included, goes on. (An effect beyond f64::MAX, which
/// M2.8-7 could reach, is out of reach: |S| < 2⁸¹ and strengths ≤ 10⁴.)
#[test]
fn values_are_limited_and_an_effect_beyond_the_guard_faults() {
    let membrane = |strength: f64| {
        project(
            &format!(
                "c = {{ module = \"std.count\" }}\nm = {{ module = \"std.mesh2d\", strength = {strength:e} }}\nk = {{ module = \"std.count\" }}\np = {{ module = \"std.particles\", lifetime_s = 60, gravity = 0 }}\nout = {{ module = \"std.audio_out\" }}\n"
            ),
            "  { from = \"c.window\", to = \"m.strike\" },\n  { from = \"k.window\", to = \"p.emit\" },\n",
        )
    };
    let script = vec![
        set(10, "/c.in", 1e308),
        set(10, "/c.in", 1e308),
        trigger(10, "/k.in"),
    ];
    let taps = [tap("/m.z", 256, 800), tap("/p.life", 64, 800)];

    let (log, tracks) = scripted(&[membrane(2f64.powi(-60))], &script, 2400, &taps);
    assert!(!log.contains("\"type\":\"fault\""), "{log}");
    let z = &tracks[0]
        .snapshots
        .iter()
        .find(|(f, _)| f.0 == 800)
        .unwrap()
        .1;
    let h = 800.0 / 48_000.0;
    let g = (-3.0 * std::f64::consts::LN_10 / 3.0 * h).exp();
    let node = 8 * 16 + 8;
    println!(
        "strike node at tick 800: {} (expected {})",
        z[node],
        g * h * 32.0
    );
    assert!((z[node] / (g * h * 32.0) - 1.0).abs() < 1e-12);

    let (log, tracks) = scripted(&[membrane(1.0)], &script, 3200, &taps);
    let faults: Vec<&str> = log
        .lines()
        .filter(|l| l.contains("\"type\":\"fault\""))
        .collect();
    println!("{faults:?}");
    assert!(
        faults.iter().any(|l| l.contains("\"address\":\"/m\"")
            && l.contains("\"reason\":\"magnitude\"")
            && l.contains("\"frame\":800")),
        "{faults:?}"
    );
    assert!(
        faults.iter().all(|l| l.contains("\"address\":\"/m\"")),
        "{faults:?}"
    );
    let particles = live(&tracks[1]);
    assert_eq!(
        (particles.get(&800), particles.get(&2400)),
        (Some(&1), Some(&1))
    );
}

/// The generation rows (4 to 6) of a `std.count` reading.
fn generation(rows: &[f64]) -> f64 {
    rows[4] * 2f64.powi(38) + rows[5] * 2f64.powi(19) + rows[6]
}

/// Operator decision operator-request:020, condition 3: a live session
/// through the Controller and the null driver removes the counter (suspend
/// at 3000, swap at 3240) and adds it again at the same address (4000). Its
/// reading, streamed with the bodies it kicks, carries generation 0 before
/// and 2 after; the replay of the recorded session reproduces every
/// streamed snapshot bit for bit, so it gives instances the same creation
/// versions. A fresh load of the same project gives the counter 0.
#[test]
fn a_replayed_session_reproduces_creation_generations() {
    const FRAMES: u64 = 9_600;
    let registry = registry();
    let wired = || project(COUNTER_AND_BODIES, C_TO_B);
    let v0 = wired();
    let mut plan = v0.plan(&registry).unwrap().plan;
    plan.version = 0;
    let recorder = SessionRecorder::new(&v0, &registry, 48_000, 5, provenance()).unwrap();
    let mut live = LiveRecorder::new(recorder, registry.clone());
    let (runner, mut controller) =
        realtime(&plan, &registry, RealtimeConfig::new(48_000, 5)).unwrap();
    let streams = [("/c.window", signal_std::BRIDGE_ROWS), ("/b.vel", 12)];
    for (slot, &(p, rows)) in streams.iter().enumerate() {
        controller
            .vector_start(&plan, slot, port(p), rows, 800)
            .unwrap();
    }
    let mut driver = NullDriver::new(48_000, 2, 3);
    driver.start(runner).unwrap();
    let mut snapshots: Vec<(usize, u64, Vec<f32>)> = Vec::new();
    let mut stage = 0;
    driver
        .run(FRAMES, || {
            controller.poll(&mut live);
            controller.poll_vectors(|s| snapshots.push((s.slot, s.frame.0, s.values().to_vec())));
            let status = controller.status();
            if stage == 0 {
                stage = 1;
                for at in [100, 2000] {
                    controller
                        .send_event(port("/c.in"), EventPayload::Trigger, Stamp::At(Frame(at)))
                        .unwrap();
                }
                controller
                    .send_suspend("/c".parse().unwrap(), Stamp::At(Frame(3000)))
                    .unwrap();
                let removed = live.swap_plan(project(ONLY_BODIES, "")).unwrap();
                controller
                    .swap(&removed, Stamp::At(Frame(3000 + F)))
                    .unwrap();
            } else if stage == 1 && status.version == 1 {
                stage = 2;
                let readded = live.swap_plan(wired()).unwrap();
                controller.swap(&readded, Stamp::At(Frame(4000))).unwrap();
            } else if stage == 2 && status.version == 2 {
                stage = 3;
                for at in [5000, 6500] {
                    let at = Frame(at).max(status.next_frame);
                    controller
                        .send_event(port("/c.in"), EventPayload::Trigger, Stamp::At(at))
                        .unwrap();
                }
            }
        })
        .unwrap();
    let runner = driver.stop().unwrap().unwrap();
    controller.poll(&mut live);
    controller.poll_vectors(|s| snapshots.push((s.slot, s.frame.0, s.values().to_vec())));
    live.frontier(runner.next_frame());
    assert_eq!(stage, 3, "both swaps committed");
    assert_eq!(controller.vector_overflows(), 0);
    assert_eq!(live.refused(), &[] as &[String]);

    let dir = out_dir("generations");
    live.into_recorder().write(&dir).unwrap();
    let session = Session::load(&dir, &registry).unwrap();
    let options = ReplayOptions {
        frames: Some(FRAMES),
        threads: 1,
        vectors: streams.iter().map(|&(p, rows)| tap(p, rows, 800)).collect(),
        ..ReplayOptions::default()
    };
    let replayed = replay_with(&session, &registry, options).unwrap();
    for (slot, &(p, _)) in streams.iter().enumerate() {
        let replay: BTreeMap<u64, &Vec<f64>> = replayed.vectors[slot]
            .snapshots
            .iter()
            .map(|(f, rows)| (f.0, rows))
            .collect();
        let streamed: Vec<&(usize, u64, Vec<f32>)> =
            snapshots.iter().filter(|s| s.0 == slot).collect();
        assert!(streamed.len() >= 8, "{p}: {}", streamed.len());
        assert_eq!(streamed.len(), replay.len(), "{p}");
        for (_, frame, values) in &streamed {
            let rows = replay
                .get(frame)
                .unwrap_or_else(|| panic!("{p}: no replay at {frame}"));
            let replayed: Vec<u32> = rows.iter().map(|&x| (x as f32).to_bits()).collect();
            let played: Vec<u32> = values.iter().map(|x| x.to_bits()).collect();
            assert_eq!(played, replayed, "{p} at frame {frame}");
        }
    }
    let generations: Vec<(u64, f64)> = replayed.vectors[0]
        .snapshots
        .iter()
        .map(|(f, rows)| (f.0, generation(rows)))
        .collect();
    println!("/c.window generations: {generations:?}");
    assert!(generations.iter().all(|&(f, g)| f >= 3240 || g == 0.0));
    assert!(generations.iter().all(|&(f, g)| f < 4000 || g == 2.0));
    assert!(generations.iter().any(|&(f, _)| f >= 4000));

    let fresh = wired().plan(&registry).unwrap().plan;
    let mut engine =
        Engine::new(&fresh, &registry, EngineConfig::new(48_000, 5), Vec::new()).unwrap();
    let taps = [tap("/c.window", signal_std::BRIDGE_ROWS, 800)];
    let (_, tracks) = render_blocks_tapped(
        &mut engine,
        1,
        1,
        |_| Ok(None),
        &taps,
        std::slice::from_ref(&fresh),
    )
    .unwrap();
    assert_eq!(generation(&tracks[0].snapshots[0].1), 0.0);
}
