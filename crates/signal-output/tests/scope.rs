//! M2.8-1: the scope adapter. The engine's per-frame output rows are each
//! output's own values (two oscillators summed into one sink input are told
//! apart); a scope stream is the same whatever the callback sizes and thread
//! count; scopes leave the audio as it was; a swap keeps a slot whose port
//! remains and unbinds one whose port is gone; a stopped slot's blocks are
//! dropped.

use std::collections::BTreeMap;

use signal_core::{PortAddress, Registry};
use signal_engine::{Engine, EngineConfig, RealtimeConfig, Stamp};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, InstanceDef, build_plan};
use signal_output::{Controller, Driver, NullDriver, SCOPE_POINTS, ScopeError, realtime};

const RATE: u32 = 48_000;
const SEED: u64 = 3;

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

/// `a` (440 Hz) into both inputs, `b` (220 Hz) into the left one too; with
/// `with_b` false, no `b`.
fn plan(registry: &Registry, with_b: bool, version: u64) -> ExecutionPlan {
    let instance = |name: &str, module: &str, parameters: &[(&str, f64)]| InstanceDef {
        name: name.to_owned(),
        module: module.to_owned(),
        parameters: parameters
            .iter()
            .map(|(k, v)| ((*k).to_owned(), *v))
            .collect::<BTreeMap<_, _>>(),
        scope: None,
    };
    let connection = |from: &str, to: &str| ConnectionDef {
        from: from.to_owned(),
        to: to.to_owned(),
        scale: None,
    };
    let mut instances = vec![
        instance("a", "std.sine_osc", &[("freq", 440.0)]),
        instance("out", "std.audio_out", &[("gain", 0.5)]),
    ];
    let mut connections = vec![connection("a.out", "out.l"), connection("a.out", "out.r")];
    if with_b {
        instances.push(instance("b", "std.sine_osc", &[("freq", 220.0)]));
        connections.push(connection("b.out", "out.l"));
    }
    let root = CompositeDef {
        name: "project".to_owned(),
        instances,
        connections,
        exports: Exports::default(),
    };
    let mut plan = build_plan(&root, &[], registry, 0).unwrap();
    plan.version = version;
    plan
}

fn port(text: &str) -> PortAddress {
    text.parse().unwrap()
}

/// `port`'s value at each of the first `frames` frames, run one at a time.
fn reference(port: &PortAddress, frames: u64) -> Vec<f64> {
    let registry = registry();
    let mut engine = Engine::new(
        &plan(&registry, true, 0),
        &registry,
        EngineConfig::new(RATE, SEED),
        Vec::new(),
    )
    .unwrap();
    (0..frames)
        .map(|_| {
            engine.run_frame();
            engine.signal_output(port).unwrap()
        })
        .collect()
}

#[test]
fn the_output_rows_are_each_outputs_own_values() {
    let registry = registry();
    let mut engine = Engine::new(
        &plan(&registry, true, 0),
        &registry,
        EngineConfig::new(RATE, SEED),
        Vec::new(),
    )
    .unwrap();
    let frames = 1000;
    let a = reference(&port("/a.out"), frames);
    let b = reference(&port("/b.out"), frames);
    assert_ne!(a, b);
    let oa = engine.signal_output_index(&port("/a.out")).unwrap();
    let ob = engine.signal_output_index(&port("/b.out")).unwrap();
    assert_eq!(
        engine.signal_output_index(&port("/out.gain")),
        None,
        "not a signal output"
    );
    let mut at = 0usize;
    while at < frames as usize {
        let ran = engine.run_block(97.min(frames as usize - at));
        for f in 0..ran {
            assert_eq!(
                engine.signal_output_at(oa, f).unwrap().to_bits(),
                a[at + f].to_bits()
            );
            assert_eq!(
                engine.signal_output_at(ob, f).unwrap().to_bits(),
                b[at + f].to_bits()
            );
        }
        at += ran;
    }
}

/// The points of the slots started before frame 0 over `frames` frames, as
/// (slot, frame, value), and the interleaved output.
fn capture(
    seed: u64,
    threads: usize,
    scopes: &[(&str, u64)],
    frames: u64,
) -> (Vec<(usize, u64, f32)>, Vec<f32>) {
    let registry = registry();
    let mut config = RealtimeConfig::new(RATE, SEED);
    config.engine.threads = threads;
    let (runner, mut controller) = realtime(&plan(&registry, true, 0), &registry, config).unwrap();
    for (slot, (address, decimation)) in scopes.iter().enumerate() {
        controller
            .scope_start(slot, port(address), *decimation)
            .unwrap();
    }
    let mut driver = NullDriver::new(RATE, 2, seed);
    driver.start(runner).unwrap();
    let mut points = Vec::new();
    let mut drain = |controller: &mut Controller| {
        controller.poll(&mut ());
        controller.poll_scopes(|block| {
            assert_eq!(block.len, SCOPE_POINTS, "only full blocks are pushed");
            for (k, v) in block.values().iter().enumerate() {
                points.push((
                    block.slot,
                    block.first_frame.0 + k as u64 * block.decimation,
                    *v,
                ));
            }
        });
    };
    driver.run(frames, || drain(&mut controller)).unwrap();
    drain(&mut controller);
    assert_eq!(controller.scope_overflows(), 0);
    points.sort_by_key(|&(slot, frame, _)| (slot, frame));
    (points, driver.take_output())
}

#[test]
fn a_scope_stream_is_its_outputs_values_whatever_the_callback_sizes() {
    let frames = 20_000;
    let scopes = [("/a.out", 1), ("/b.out", 7)];
    let (points, _) = capture(1, 1, &scopes, frames);
    assert_eq!(
        capture(2, 1, &scopes, frames).0,
        points,
        "another callback partition"
    );
    assert_eq!(capture(1, 4, &scopes, frames).0, points, "four threads");
    let a = reference(&port("/a.out"), frames);
    let b = reference(&port("/b.out"), frames);
    let mut counts = [0usize; 2];
    for &(slot, frame, value) in &points {
        let (wanted, d) = if slot == 0 { (&a, 1) } else { (&b, 7) };
        assert_eq!(frame % d, 0, "slot {slot}: frame {frame}");
        assert_eq!(
            value.to_bits(),
            (wanted[frame as usize] as f32).to_bits(),
            "slot {slot} frame {frame}"
        );
        counts[slot] += 1;
    }
    // Full blocks only: 20 000 points of `a`, 2 858 of `b`.
    assert_eq!(counts, [78 * SCOPE_POINTS, 11 * SCOPE_POINTS]);
}

/// M2.8-3: a scope on a signal input shows what the instance receives: the
/// sum of its sources (here `/out.l` = a + b), whatever the partition.
#[test]
fn a_scope_on_a_signal_input_is_the_sum_of_its_sources() {
    let frames = 6_000;
    let scopes = [("/out.l", 1), ("/out.r", 5)];
    let (points, _) = capture(1, 1, &scopes, frames);
    assert_eq!(
        capture(2, 4, &scopes, frames).0,
        points,
        "another partition"
    );
    let a = reference(&port("/a.out"), frames);
    let b = reference(&port("/b.out"), frames);
    let mut counts = [0usize; 2];
    for &(slot, frame, value) in &points {
        let f = frame as usize;
        let wanted = if slot == 0 { a[f] + b[f] } else { a[f] };
        assert_eq!(
            value.to_bits(),
            (wanted as f32).to_bits(),
            "slot {slot} frame {frame}"
        );
        counts[slot] += 1;
    }
    assert_eq!(counts, [23 * SCOPE_POINTS, 4 * SCOPE_POINTS]);
}

#[test]
fn scopes_leave_the_audio_unchanged() {
    let frames = 12_000;
    let (_, plain) = capture(5, 2, &[], frames);
    let (_, scoped) = capture(5, 2, &[("/a.out", 1), ("/b.out", 3)], frames);
    assert_eq!(plain.len(), scoped.len());
    assert!(
        plain
            .iter()
            .zip(&scoped)
            .all(|(x, y)| x.to_bits() == y.to_bits())
    );
}

#[test]
fn a_swap_keeps_a_slot_whose_port_remains_and_unbinds_the_others() {
    let registry = registry();
    let (runner, mut controller) = realtime(
        &plan(&registry, true, 0),
        &registry,
        RealtimeConfig::new(RATE, SEED),
    )
    .unwrap();
    controller.scope_start(0, port("/a.out"), 1).unwrap();
    controller.scope_start(1, port("/b.out"), 1).unwrap();
    let mut driver = NullDriver::new(RATE, 2, 1);
    driver.start(runner).unwrap();
    driver.run(4_800, || {}).unwrap();
    assert!(controller.scope_bound(0) && controller.scope_bound(1));
    controller.poll(&mut ());
    controller
        .swap(&plan(&registry, false, 1), Stamp::Default)
        .unwrap();
    let mut late = Vec::new();
    driver
        .run(9_600, || {
            controller.poll(&mut ());
            controller.poll_scopes(|b| late.push((b.slot, b.first_frame.0)));
        })
        .unwrap();
    assert_eq!(controller.status().version, 1, "the swap committed");
    assert!(controller.scope_bound(0), "`/a.out` remains");
    assert!(!controller.scope_bound(1), "`/b.out` is gone");
    assert!(
        late.iter()
            .any(|&(slot, first)| slot == 0 && first >= 9_600),
        "slot 0 goes on"
    );
}

#[test]
fn a_stopped_slot_drops_its_blocks_and_bad_commands_are_refused() {
    let registry = registry();
    let (runner, mut controller) = realtime(
        &plan(&registry, true, 0),
        &registry,
        RealtimeConfig::new(RATE, SEED),
    )
    .unwrap();
    assert_eq!(
        controller.scope_start(8, port("/a.out"), 1),
        Err(ScopeError::Slot)
    );
    assert_eq!(
        controller.scope_start(0, port("/a.out"), 0),
        Err(ScopeError::Decimation)
    );
    controller.scope_start(0, port("/a.out"), 1).unwrap();
    let mut driver = NullDriver::new(RATE, 2, 1);
    driver.start(runner).unwrap();
    driver.run(4_800, || {}).unwrap();
    controller.scope_stop(0).unwrap();
    let mut blocks = 0;
    controller.poll_scopes(|_| blocks += 1);
    assert_eq!(blocks, 0, "queued blocks of a stopped slot are dropped");
    driver.run(4_800, || {}).unwrap();
    controller.poll_scopes(|_| blocks += 1);
    assert_eq!(blocks, 0);
    // A restart is a new generation; its blocks arrive.
    let generation = controller.scope_start(0, port("/b.out"), 2).unwrap();
    driver.run(4_800, || {}).unwrap();
    controller.poll_scopes(|b| {
        assert_eq!(b.generation, generation);
        assert_eq!(b.decimation, 2);
        blocks += 1;
    });
    assert!(blocks > 0);
}
