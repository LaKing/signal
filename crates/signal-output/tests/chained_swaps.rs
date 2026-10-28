//! ADR 0037 §6, §9: a slot's Replace is two swaps one frame apart (an
//! unload, then a load), both prepared before either is sent:
//! `Controller::prepare_swap_after` prepares the second against the plan the
//! first swaps in. Through the Runner, in seeded callback sizes on 1 and 4
//! threads: the leaving content fades out to +0.0 at *s* + *F* − 1, the
//! slot is empty for one frame, the new content fades in exactly from a
//! fresh state although it shares the old one's address and type, the
//! host's own sound is bit-continuous, and the callbacks allocate nothing.
//! A second swap sent without the first is refused as stale.

signal_contract::install_counting_allocator!();

use signal_contract::alloc_counter::Guard;
use signal_core::{Frame, Registry};
use signal_engine::{ControlError, EngineConfig, EngineError, RealtimeConfig, Stamp};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, InstanceDef, build_plan};
use signal_output::{CallbackSizes, realtime, render};

const RATE: u32 = 48_000;
/// *F*: 5 ms at 48 kHz.
const F: u64 = 240;

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn instance(name: &str, module: &str, parameters: &[(&str, f64)]) -> InstanceDef {
    InstanceDef {
        name: name.to_owned(),
        module: module.to_owned(),
        parameters: parameters
            .iter()
            .map(|(k, v)| ((*k).to_owned(), *v))
            .collect(),
        scope: None,
    }
}

fn wire(from: &str, to: &str) -> ConnectionDef {
    ConnectionDef {
        from: from.to_owned(),
        to: to.to_owned(),
        scale: None,
    }
}

/// The host's `tone` on the left; on the right a slot's content `/deck/osc`
/// at `freq` (`None`: the slot is empty), as the slot's resolution plans it.
fn plan(freq: Option<f64>, tone: bool, version: u64) -> ExecutionPlan {
    let mut instances = vec![instance("out", "std.audio_out", &[])];
    let mut connections = Vec::new();
    if tone {
        instances.push(instance("tone", "std.sine_osc", &[("freq", 330.0)]));
        connections.push(wire("tone.out", "out.l"));
    }
    let mut defs = Vec::new();
    if let Some(freq) = freq {
        instances.push(instance("deck", "voice", &[]));
        connections.push(wire("deck.out", "out.r"));
        defs.push(CompositeDef {
            name: "voice".to_owned(),
            instances: vec![instance("osc", "std.sine_osc", &[("freq", freq)])],
            connections: Vec::new(),
            exports: Exports {
                outputs: vec![signal_graph::OutputExport {
                    name: "out".to_owned(),
                    from: "osc.out".to_owned(),
                }],
                ..Exports::default()
            },
        });
    }
    let root = CompositeDef {
        name: "project".to_owned(),
        instances,
        connections,
        exports: Exports::default(),
    };
    let mut plan = build_plan(&root, &defs, &registry(), 0).unwrap();
    plan.version = version;
    plan
}

/// The right channel of a fresh `/deck/osc` at `freq` from its first frame,
/// as f64 (gain 1).
fn fresh(freq: f64, frames: u64) -> Vec<f64> {
    let registry = registry();
    let mut engine = signal_engine::Engine::new(
        &plan(Some(freq), false, 0),
        &registry,
        EngineConfig::new(RATE, 1),
        Vec::new(),
    )
    .unwrap();
    let rendered = render(&mut engine, frames).unwrap();
    rendered.samples.chunks(2).map(|f| f[1]).collect()
}

/// Plays `deck` at 220 Hz, then replaces it with 440 Hz: suspend at the
/// default frame *s*, the empty slot at *s* + *F*, the new content at
/// *s* + *F* + 1. Returns the device samples, *s*, and the allocations
/// counted in the callbacks.
fn replace(threads: usize, seed: u64) -> (Vec<f32>, u64, u64) {
    let registry = registry();
    // A 4096-frame buffer: commands are stamped two buffers ahead, so even
    // the largest callback cannot make one late.
    let mut config = RealtimeConfig::with_buffer(RATE, 1, 4096);
    config.engine.threads = threads;
    let (mut runner, mut controller) =
        realtime(&plan(Some(220.0), true, 0), &registry, config).unwrap();
    let mut sizes = CallbackSizes::new(seed);
    let mut played = Vec::new();
    let mut buffer = vec![0.0f32; 2 * 4096];
    let mut allocations = 0;
    let mut s = None;
    let mut callback = |runner: &mut signal_output::Runner, played: &mut Vec<f32>| {
        let n = sizes.next_size();
        let guard = Guard::start();
        runner.process(&mut buffer[..2 * n], 2);
        let counts = guard.counts();
        drop(guard);
        played.extend_from_slice(&buffer[..2 * n]);
        counts.allocations + counts.deallocations
    };
    while played.len() < 2 * 20_000 {
        allocations += callback(&mut runner, &mut played);
        controller.poll(&mut ());
        if s.is_none() && played.len() >= 2 * 4_000 {
            let unload = controller.prepare_swap(&plan(None, true, 1)).unwrap();
            let load = controller
                .prepare_swap_after(&unload, &plan(Some(440.0), true, 2))
                .unwrap();
            let start = controller.default_frame();
            controller
                .send_suspend("/deck/osc".parse().unwrap(), Stamp::At(start))
                .unwrap();
            let frame = controller.removal_swap_frame(start);
            controller.send_swap(unload, Stamp::At(frame)).unwrap();
            controller
                .send_swap(load, Stamp::At(Frame(frame.0 + 1)))
                .unwrap();
            s = Some(start.0);
        }
    }
    while controller.in_flight() > 0 {
        allocations += callback(&mut runner, &mut played);
        controller.poll(&mut ());
    }
    assert_eq!(runner.status().version, 2);
    assert_eq!(runner.status().late, 0, "nothing late");
    (played, s.unwrap(), allocations)
}

#[test]
fn a_replace_unloads_then_loads_a_fresh_content_without_allocating() {
    let host: Vec<f32> = {
        // The host alone: its tone, never edited.
        let registry = registry();
        let mut engine = signal_engine::Engine::new(
            &plan(None, true, 0),
            &registry,
            EngineConfig::new(RATE, 1),
            Vec::new(),
        )
        .unwrap();
        let rendered = render(&mut engine, 40_000).unwrap();
        rendered.samples.chunks(2).map(|f| f[0] as f32).collect()
    };
    let old = fresh(220.0, 40_000);
    let new = fresh(440.0, 40_000);
    for (threads, seed) in [(1, 7), (4, 8), (1, 9)] {
        let (played, s, allocations) = replace(threads, seed);
        assert_eq!(allocations, 0, "threads {threads}: the callbacks allocated");
        let s = s as usize;
        let f = F as usize;
        let right: Vec<f32> = played.chunks(2).map(|c| c[1]).collect();
        let left: Vec<f32> = played.chunks(2).map(|c| c[0]).collect();
        // The host's own sound is bit-continuous.
        assert_eq!(left, host[..left.len()], "threads {threads}");
        // The old content plays until s, then fades out: exactly +0.0 from
        // s + F − 1, through the empty frame s + F.
        assert_eq!(right[s - 1], old[s - 1] as f32);
        assert!(right[s] != 0.0);
        assert!(
            right[s + f - 1..=s + f].iter().all(|x| x.to_bits() == 0),
            "+0.0 at s + F − 1 and s + F"
        );
        // The new content fades in from its own first frame on s + F + 1:
        // fresh (its phase starts at 0), not the old oscillator's state,
        // although both are `/deck/osc` of type `std.sine_osc`.
        let load = s + f + 1;
        for j in 0..2 * f {
            let live = new[j];
            let done = (j + 1) as u64;
            let expected = if done >= F {
                live
            } else {
                live * (done as f64 / F as f64)
            };
            assert_eq!(right[load + j], expected as f32, "threads {threads}, j {j}");
        }
    }
}

/// The second swap is prepared against the first: sent without it, or
/// prepared by another Controller, it is refused before anything is queued.
#[test]
fn a_chained_swap_needs_its_first() {
    let registry = registry();
    let config = RealtimeConfig::new(RATE, 1);
    let (_runner, controller) = realtime(&plan(Some(220.0), true, 0), &registry, config).unwrap();
    let (_other_runner, mut other) =
        realtime(&plan(Some(220.0), true, 0), &registry, config).unwrap();
    let unload = controller.prepare_swap(&plan(None, true, 1)).unwrap();
    let load = controller
        .prepare_swap_after(&unload, &plan(Some(440.0), true, 2))
        .unwrap();
    let mut controller = controller;
    let refused = controller.send_swap(load, Stamp::Default).unwrap_err();
    assert!(
        matches!(refused, ControlError::Engine(EngineError::StaleSwap)),
        "{refused:?}"
    );
    assert_eq!(controller.in_flight(), 0);
    // Another Controller's first swap cannot be built on.
    let foreign = other.prepare_swap(&plan(None, true, 1)).unwrap();
    assert!(matches!(
        controller.prepare_swap_after(&foreign, &plan(Some(440.0), true, 2)),
        Err(ControlError::Engine(EngineError::StaleSwap))
    ));
    let _ = other.send_swap(foreign, Stamp::Default);
}
