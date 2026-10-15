//! M2.8-7: the physical-av demo (`projects/physical-av`) and the event
//! bridge into the visual-rate types. The demo's types and panel, its
//! floors, a recorded session that replays the visual streams bit for bit
//! (AC: replay), and the bridge's lifecycle across swaps and its arithmetic
//! at the edges (operator decisions operator-request:007 and :017). The
//! partition and thread matrix is in `blocks.rs`.

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
        controller.vector_start(slot, port(p), rows, 800).unwrap();
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
                controller.vector_start(3, port(p), rows, 800).unwrap();
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
                Step::Swap(v) => {
                    let mut swap = engine.prepare_swap(&built[*v], &registry)?;
                    engine.commit_swap(&mut swap, now)?;
                }
            }
        }
        Ok(script.get(next).map(|(at, _)| Frame(*at)))
    };
    let (rendered, tracks) = render_blocks_tapped(&mut engine, frames, 256, before, taps).unwrap();
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

/// The bridge across swaps: a consumer replaced (removed, another added) at
/// frame 1000 applies, at its first tick (1600), exactly the events since
/// the previous tick, 900 and 1100, which no consumer had applied; not the
/// 50 events at frame 100 its predecessor applied at tick 800 (no burst).
/// With the removal protocol (suspend at 700, swap at 940), the suspended
/// consumer skips tick 800, so an event at 750 is applied by nobody
/// (`docs/semantics.md` §6) and the new consumer applies only the event at
/// 1000.
#[test]
fn a_replaced_consumer_takes_every_pending_event() {
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
    let (_, tracks) = scripted(&[v0(), v1()], &script, 3200, &taps);
    let (old, new) = (live(&tracks[0]), live(&tracks[1]));
    assert_eq!(old.get(&800), Some(&50));
    assert_eq!(old.get(&1600), None, "removed");
    assert_eq!(new.get(&1600), Some(&2), "{new:?}");
    assert_eq!(new.get(&2400), Some(&2));

    let script = vec![
        (700, Step::Suspend("/sparks")),
        (750, Step::Event("/c.in", EventPayload::Trigger)),
        (940, Step::Swap(1)),
        (1000, Step::Event("/c.in", EventPayload::Trigger)),
    ];
    let (_, tracks) = scripted(&[v0(), v1()], &script, 2400, &taps);
    let (old, new) = (live(&tracks[0]), live(&tracks[1]));
    assert_eq!(old.get(&800), Some(&0), "suspended across tick 800");
    assert_eq!(new.get(&1600), Some(&1), "{new:?}");
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
        .vector_start(0, port("/a.vel"), 128, 800)
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
                controller.vector_start(0, port("/a.vel"), 12, 800).unwrap();
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

/// The one exception, accepted by the operator (operator-request:017): a
/// counter replaced at frame 1000 (a new instance) loses the events it
/// counted since its consumer's last tick: the event at 900 never acts, the
/// one at 1100 (the new counter's) does, at tick 1600.
#[test]
fn a_replaced_counter_loses_only_events_between_the_last_tick_and_the_swap() {
    let p = "p = { module = \"std.particles\", lifetime_s = 60, gravity = 0 }\nout = { module = \"std.audio_out\" }\n";
    let v0 = project(
        &format!("c = {{ module = \"std.count\" }}\n{p}"),
        "  { from = \"c.window\", to = \"p.emit\" },\n",
    );
    let v1 = project(
        &format!("c2 = {{ module = \"std.count\" }}\n{p}"),
        "  { from = \"c2.window\", to = \"p.emit\" },\n",
    );
    let script = vec![
        (900, Step::Event("/c.in", EventPayload::Trigger)),
        (1000, Step::Swap(1)),
        (1100, Step::Event("/c2.in", EventPayload::Trigger)),
    ];
    let (_, tracks) = scripted(&[v0, v1], &script, 2400, &[tap("/p.life", 64, 800)]);
    let counts = live(&tracks[0]);
    assert_eq!(counts.get(&800), Some(&0));
    assert_eq!(counts.get(&1600), Some(&1), "{counts:?}");
}

/// Finding auditor-1-005: the effect of two Set(1e308) (a sum binary64
/// cannot hold) at strength 1e-305 is formed before it is decoded: the
/// visual membrane's strike node moves by exactly `g·h·2000` at tick 800,
/// and nothing faults. At strength 1 the effect itself is beyond f64::MAX:
/// the membrane faults (`non_finite`, logged) at tick 800 while the rest of
/// the run, a second counter's particles included, goes on.
#[test]
fn effects_are_formed_before_decoding_and_only_an_unrepresentable_one_faults() {
    let membrane = |strength: &str| {
        project(
            &format!(
                "c = {{ module = \"std.count\" }}\nm = {{ module = \"std.mesh2d\", strength = {strength} }}\nk = {{ module = \"std.count\" }}\np = {{ module = \"std.particles\", lifetime_s = 60, gravity = 0 }}\nout = {{ module = \"std.audio_out\" }}\n"
            ),
            "  { from = \"c.window\", to = \"m.strike\" },\n  { from = \"k.window\", to = \"p.emit\" },\n",
        )
    };
    let script = vec![
        (10, Step::Event("/c.in", EventPayload::Set(1e308))),
        (10, Step::Event("/c.in", EventPayload::Set(1e308))),
        (10, Step::Event("/k.in", EventPayload::Trigger)),
    ];
    let taps = [tap("/m.z", 256, 800), tap("/p.life", 64, 800)];

    let (log, tracks) = scripted(&[membrane("1e-305")], &script, 2400, &taps);
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
        g * h * 2000.0
    );
    assert!((z[node] / (g * h * 2000.0) - 1.0).abs() < 1e-12);

    let (log, tracks) = scripted(&[membrane("1")], &script, 3200, &taps);
    let faults: Vec<&str> = log
        .lines()
        .filter(|l| l.contains("\"type\":\"fault\""))
        .collect();
    println!("{faults:?}");
    assert!(
        faults.iter().any(|l| l.contains("\"address\":\"/m\"")
            && l.contains("\"reason\":\"non_finite\"")
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
