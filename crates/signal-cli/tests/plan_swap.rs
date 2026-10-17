//! A `std.sine_osc` that survives a plan swap continues its phase exactly
//! (M2-1 AC2). It lives here because this crate already depends on both
//! `signal-std` and `signal-engine`.

use signal_core::{Frame, PortAddress, Registry};
use signal_engine::{Engine, EngineConfig};
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
