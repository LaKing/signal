//! A `std.sine_osc` that survives a plan swap continues its phase exactly
//! (M2-1 AC2), and so do the state types (M2.10-1 `std.frames`; M2.10-2
//! `std.phasor`, `std.counter`, `std.buffer1k`, `std.random`,
//! `std.integrator` and `std.to_events`). It lives here because this
//! crate already depends on both `signal-std` and `signal-engine`.

use signal_core::{EventPayload, Frame, PortAddress, Registry};
use signal_engine::{Engine, EngineConfig, ExternalEvent};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, InstanceDef, build_plan};

const RATE: u32 = 48_000;
const FRAMES: u64 = 4_000;
const SWAP: u64 = 1_000;

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn inst(name: &str, module: &str, parameters: &[(&str, f64)]) -> InstanceDef {
    InstanceDef {
        name: name.to_owned(),
        module: module.to_owned(),
        parameters: parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), *value))
            .collect(),
    }
}

fn conn(from: &str, to: &str) -> ConnectionDef {
    ConnectionDef {
        from: from.to_owned(),
        to: to.to_owned(),
        scale: None,
    }
}

fn plan(instances: Vec<InstanceDef>, connections: Vec<ConnectionDef>) -> ExecutionPlan {
    let root = CompositeDef {
        name: "project".to_owned(),
        instances,
        connections,
        exports: Exports::default(),
    };
    build_plan(&root, &[], &registry(), 1).unwrap()
}

/// `/osc.out` after each frame, with a swap to `swap_to` committed at `SWAP`.
fn osc_trace(start: &ExecutionPlan, swap_to: Option<&ExecutionPlan>) -> Vec<u64> {
    let registry = registry();
    let mut engine = Engine::new(start, &registry, EngineConfig::new(RATE, 7), Vec::new()).unwrap();
    let out: PortAddress = "/osc.out".parse().unwrap();
    let mut trace = Vec::new();
    for frame in 0..FRAMES {
        if frame == SWAP
            && let Some(next) = swap_to
        {
            let mut swap = engine.prepare_swap(next, &registry).unwrap();
            engine.commit_swap(&mut swap, Frame(frame)).unwrap();
        }
        engine.run_frame();
        engine.drain_log(&mut |_| {});
        trace.push(engine.signal_output(&out).unwrap().to_bits());
    }
    trace
}

#[test]
fn surviving_sine_osc_continues_its_phase_exactly() {
    let before = plan(
        vec![
            inst("osc", "std.sine_osc", &[("freq", 441.0)]),
            inst("out", "std.audio_out", &[]),
        ],
        vec![conn("osc.out", "out.l"), conn("osc.out", "out.r")],
    );
    // Another oscillator added, and the outputs rewired.
    let after = plan(
        vec![
            inst("osc", "std.sine_osc", &[("freq", 441.0)]),
            inst("osc2", "std.sine_osc", &[("freq", 1_000.0)]),
            inst("out", "std.audio_out", &[]),
        ],
        vec![conn("osc.out", "out.l"), conn("osc2.out", "out.r")],
    );

    let unswapped = osc_trace(&before, None);
    let swapped = osc_trace(&before, Some(&after));
    assert_eq!(swapped, unswapped);
    // Not trivially equal: the phase has moved well away from 0 by the swap.
    assert!(f64::from_bits(unswapped[SWAP as usize]).abs() > 0.0);
}

/// M2.10-1 AC6 (ADR 0006): a `std.frames` that survives a swap keeps its
/// count, so at the swap frame it reads `SWAP` and goes on; one the swap
/// adds starts at 0. The counts stay below 10⁶, so `lo` is the count (`hi`
/// is 0; the carry is tested in `core_bound.rs`).
#[test]
fn surviving_frames_keeps_its_count_and_an_added_one_starts_at_zero() {
    let before = plan(
        vec![
            inst("count", "std.frames", &[]),
            inst("out", "std.audio_out", &[]),
        ],
        vec![conn("count.lo", "out.l")],
    );
    let after = plan(
        vec![
            inst("count", "std.frames", &[]),
            inst("fresh", "std.frames", &[]),
            inst("out", "std.audio_out", &[]),
        ],
        vec![conn("count.lo", "out.l"), conn("fresh.lo", "out.r")],
    );
    let registry = registry();
    let mut engine =
        Engine::new(&before, &registry, EngineConfig::new(RATE, 7), Vec::new()).unwrap();
    let count: PortAddress = "/count.lo".parse().unwrap();
    let fresh: PortAddress = "/fresh.lo".parse().unwrap();
    for frame in 0..FRAMES {
        if frame == SWAP {
            let mut swap = engine.prepare_swap(&after, &registry).unwrap();
            engine.commit_swap(&mut swap, Frame(frame)).unwrap();
        }
        engine.run_frame();
        engine.drain_log(&mut |_| {});
        assert_eq!(engine.signal_output(&count), Some(frame as f64), "{frame}");
        if frame >= SWAP {
            let added = engine.signal_output(&fresh);
            assert_eq!(added, Some((frame - SWAP) as f64), "{frame}");
        }
    }
}

/// `ports` after each frame (as bits), with a swap to `swap_to` committed at
/// `SWAP`, followed by `set`s of `(parameter, value)` there, as a live edit
/// of a survivor's parameter in the file sends them.
fn trace(
    start: &ExecutionPlan,
    swap_to: Option<&ExecutionPlan>,
    sets: &[(&str, f64)],
    ports: &[&str],
) -> Vec<Vec<u64>> {
    let registry = registry();
    let mut engine = Engine::new(start, &registry, EngineConfig::new(RATE, 7), Vec::new()).unwrap();
    let ports: Vec<PortAddress> = ports.iter().map(|p| p.parse().unwrap()).collect();
    let mut trace = vec![Vec::new(); ports.len()];
    for frame in 0..FRAMES {
        if frame == SWAP
            && let Some(next) = swap_to
        {
            let mut swap = engine.prepare_swap(next, &registry).unwrap();
            engine.commit_swap(&mut swap, Frame(frame)).unwrap();
            for (n, &(target, value)) in sets.iter().enumerate() {
                engine
                    .push_external(ExternalEvent {
                        frame: Frame(frame),
                        sequence: n as u64,
                        target: target.parse().unwrap(),
                        payload: EventPayload::Set(value),
                    })
                    .unwrap();
            }
        }
        engine.run_frame();
        engine.drain_log(&mut |_| {});
        for (t, port) in trace.iter_mut().zip(&ports) {
            // A port the swap adds has no value before it: all ones.
            t.push(engine.signal_output(port).map_or(u64::MAX, f64::to_bits));
        }
    }
    trace
}

/// M2.10-2 AC9 (ADR 0006): a surviving `std.phasor` continues its phase and
/// a surviving `std.counter` its count, bit for bit, across a swap that adds
/// an instance and rewires the outputs.
#[test]
fn surviving_phasor_and_counter_continue_exactly() {
    let before = plan(
        vec![
            inst("ph", "std.phasor", &[("freq", 441.0)]),
            inst("cnt", "std.counter", &[("modulo", 7.0)]),
            inst("out", "std.audio_out", &[]),
        ],
        vec![
            conn("ph.wrap", "cnt.trigger"),
            conn("ph.phase", "out.l"),
            conn("cnt.count", "out.r"),
        ],
    );
    let after = plan(
        vec![
            inst("ph", "std.phasor", &[("freq", 441.0)]),
            inst("cnt", "std.counter", &[("modulo", 7.0)]),
            inst("osc", "std.sine_osc", &[]),
            inst("out", "std.audio_out", &[]),
        ],
        vec![
            conn("ph.wrap", "cnt.trigger"),
            conn("osc.out", "out.l"),
            conn("ph.phase", "out.r"),
        ],
    );
    let ports = ["/ph.phase", "/cnt.count"];
    let unswapped = trace(&before, None, &[], &ports);
    let swapped = trace(&before, Some(&after), &[], &ports);
    assert_eq!(swapped, unswapped);
    // Not trivially equal: both have moved by the swap, and the count goes on.
    let at_swap = |t: &Vec<u64>| f64::from_bits(t[SWAP as usize]);
    assert!(at_swap(&unswapped[0]) > 0.0);
    assert!(at_swap(&unswapped[1]) > 0.0);
    let counts: Vec<f64> = unswapped[1].iter().map(|&b| f64::from_bits(b)).collect();
    assert!(counts[SWAP as usize..].windows(2).any(|w| w[1] != w[0]));
}

/// M2.10-2 AC2, AC9, review decision 1 (AGENTS.md rule 4): a surviving
/// `std.buffer1k` keeps what it wrote, and its `size` follows the live edit
/// that comes with the swap, as the file says; one the swap adds starts at
/// zero. The buffer writes each frame's number at it, modulo 64, until the
/// swap; after it the writes stop, `size` is set to 8, and a tap at the
/// frame number reads element `frame mod 8`; at `REGROW` `size` is set back
/// to 64, and the tap reads `frame mod 64` again: the elements beyond 8 were
/// kept.
#[test]
fn surviving_buffer_keeps_its_contents_and_follows_a_live_size_edit() {
    const REGROW: u64 = 2_500;
    let buffer =
        |name: &'static str| inst(name, "std.buffer1k", &[("size", 64.0), ("interp", 0.0)]);
    let before = plan(
        vec![
            inst("n", "std.frames", &[]),
            inst("on", "std.const", &[("value", 1.0)]),
            buffer("buf"),
            inst("out", "std.audio_out", &[]),
        ],
        vec![
            conn("n.lo", "buf.index"),
            conn("n.lo", "buf.value"),
            conn("on.out", "buf.gate"),
            conn("buf.out0", "out.l"),
        ],
    );
    let after = plan(
        vec![
            inst("n", "std.frames", &[]),
            buffer("buf"),
            buffer("fresh"),
            inst("out", "std.audio_out", &[]),
        ],
        vec![
            conn("n.lo", "buf.read0"),
            conn("n.lo", "fresh.read0"),
            conn("buf.out0", "out.l"),
            conn("fresh.out0", "out.r"),
        ],
    );
    let registry = registry();
    let mut engine =
        Engine::new(&before, &registry, EngineConfig::new(RATE, 7), Vec::new()).unwrap();
    let set = |engine: &mut Engine, frame: u64, value: f64| {
        engine
            .push_external(ExternalEvent {
                frame: Frame(frame),
                sequence: frame,
                target: "/buf.size".parse().unwrap(),
                payload: EventPayload::Set(value),
            })
            .unwrap();
    };
    let (buf, fresh): (PortAddress, PortAddress) =
        ("/buf.out0".parse().unwrap(), "/fresh.out0".parse().unwrap());
    // The last frame before the swap that wrote element `e`.
    let written = |e: u64| (0..SWAP).rev().find(|f| f % 64 == e).unwrap() as f64;
    for frame in 0..FRAMES {
        if frame == SWAP {
            let mut swap = engine.prepare_swap(&after, &registry).unwrap();
            engine.commit_swap(&mut swap, Frame(frame)).unwrap();
            set(&mut engine, frame, 8.0);
        }
        if frame == REGROW {
            set(&mut engine, frame, 64.0);
        }
        engine.run_frame();
        engine.drain_log(&mut |_| {});
        if frame >= SWAP {
            let size = if frame < REGROW { 8 } else { 64 };
            let read = engine.signal_output(&buf).unwrap();
            assert_eq!(read, written(frame % size), "{frame}");
            assert_eq!(engine.signal_output(&fresh), Some(0.0), "{frame}");
        }
    }
}

/// Review item 3 (ADR 0006): a surviving `std.random` continues its stream,
/// a surviving `std.integrator` its state, and a surviving `std.to_events`
/// its last value sent, bit for bit, across a swap that adds an instance and
/// rewires the outputs. The to_events input moves 1/48 000 per frame with a
/// threshold of 0.01, so it sends every 481 frames and not at the swap: had
/// the swap lost its last value, it would send there (on_change).
#[test]
fn surviving_random_integrator_and_to_events_continue_exactly() {
    let instances = |extra: bool| {
        let mut v = vec![
            inst("ph", "std.phasor", &[("freq", 441.0)]),
            inst("slow", "std.phasor", &[("freq", 1.0)]),
            inst("rnd", "std.random", &[("lo", -1.0), ("hi", 1.0)]),
            inst("int", "std.integrator", &[("cutoff", 50.0)]),
            inst("te", "std.to_events", &[("threshold", 0.01)]),
            inst("lat", "std.latch", &[]),
            inst("out", "std.audio_out", &[]),
        ];
        if extra {
            v.push(inst("osc", "std.sine_osc", &[]));
        }
        v
    };
    let common = || {
        vec![
            conn("ph.wrap", "rnd.trigger"),
            conn("ph.phase", "int.in"),
            conn("slow.phase", "te.in"),
            conn("te.out", "lat.in"),
        ]
    };
    let mut before_connections = common();
    before_connections.extend([conn("rnd.out", "out.l"), conn("int.out", "out.r")]);
    let mut after_connections = common();
    after_connections.extend([conn("osc.out", "out.l"), conn("lat.out", "out.r")]);
    let before = plan(instances(false), before_connections);
    let after = plan(instances(true), after_connections);
    let ports = ["/rnd.out", "/int.out", "/lat.out"];
    let unswapped = trace(&before, None, &[], &ports);
    let swapped = trace(&before, Some(&after), &[], &ports);
    assert_eq!(swapped, unswapped);
    // Not trivially equal: each moves after the swap.
    for t in &unswapped {
        let after_swap: Vec<u64> = t[SWAP as usize..].to_vec();
        assert!(after_swap.windows(2).any(|w| w[1] != w[0]));
    }
    // The latch holds across the swap frame: to_events did not send there.
    let latch = &swapped[2];
    assert_eq!(latch[SWAP as usize], latch[SWAP as usize - 1]);
}
