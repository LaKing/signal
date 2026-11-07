//! The engine's event delivery on a dense plan (review 2026-11-01,
//! `control-and-realtime-cost`; `docs/realtime-rules.md` "The engine's
//! event delivery: its worst case"): a source emitting `count` events on
//! every frame, at most 16, delivered to `fanout` event inputs, rendered in
//! blocks of 256 on one thread. A timing on the host it runs on, against
//! the same plan with no events; a benchmark, not a target:
//!
//! ```sh
//! cargo test -p signal-engine --release --offline --test event_cost -- --ignored --nocapture
//! ```
//!
//! The normal suite checks that the dense plan renders the same in any
//! block size, as every event plan must.

mod common;

use std::time::Instant;

use common::{conn, inst, inst_with, registry};
use signal_core::{
    Combine, DirectPrimitive, Direction, Event, EventPayload, Feedthrough, Inputs,
    ModuleDescriptor, ModuleType, Outputs, ParameterDescriptor, PortDescriptor, PortKind,
    PortShape, PrepareContext, Primitive, Range, RateDivisor, Registry,
};
use signal_engine::{Engine, EngineConfig};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, InstanceDef, build_plan};
use signal_output::render_blocks;

/// `count` `set(1)` events on every frame, in frame order.
struct Flood;

const FLOOD: ModuleDescriptor = ModuleDescriptor {
    type_name: "test.flood_set",
    doc: "fixture: `count` set events on every frame",
    ports: &[PortDescriptor {
        name: "out",
        direction: Direction::Out,
        shape: PortShape::Events,
        rate: RateDivisor::ONE,
        kind: PortKind::Control,
        unit: None,
        range: None,
    }],
    parameters: &[ParameterDescriptor {
        name: "count",
        kind: PortKind::Control,
        rate: RateDivisor::ONE,
        unit: None,
        range: Range {
            min: 0.0,
            max: 16.0,
        },
        default: 0.0,
        combine: Combine::Add,
        values: None,
        snap: signal_core::Snap::Nearest,
    }],
    feedthrough: Feedthrough::Direct,
    sink: None,
    source: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: None,
};

impl DirectPrimitive for Flood {
    fn describe() -> ModuleDescriptor {
        FLOOD
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {}
    fn reset(&mut self) {}
    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let counts = inputs.parameter(0);
        for f in 0..inputs.frames() {
            let count = counts.get(f).copied().unwrap_or(0.0) as usize;
            for n in 0..count {
                if let Some(sink) = outputs.events(0) {
                    sink.push(Event {
                        offset: f as u32,
                        payload: EventPayload::Set((n % 9 + 1) as f64),
                    });
                }
            }
        }
    }
}

fn types() -> Registry {
    let mut registry = registry();
    registry
        .register(ModuleType {
            descriptor: FLOOD,
            create: || Primitive::Direct(Box::new(Flood)),
        })
        .unwrap();
    registry
}

/// The source at `count` events per frame into `fanout` order-sensitive
/// receivers (`test.sequence`), the first into the sink.
fn dense(count: f64, fanout: usize) -> ExecutionPlan {
    let mut instances: Vec<InstanceDef> = vec![
        inst_with("flood", "test.flood_set", &[("count", count)]),
        inst("sink", "test.recorder"),
    ];
    let mut connections: Vec<ConnectionDef> = Vec::new();
    for n in 0..fanout {
        instances.push(inst(&format!("r{n}"), "test.sequence"));
        connections.push(conn("flood.out", &format!("r{n}.in")));
    }
    // One receiver heard (a fan-in is a graph error, ADR 0054); the others
    // run as well.
    connections.push(conn("r0.out", "sink.zeta"));
    let root = CompositeDef {
        name: "project".to_owned(),
        instances,
        connections,
        exports: Exports::default(),
    };
    build_plan(&root, &[], &types(), 1).unwrap()
}

/// `frames` frames of `plan` in blocks of `block` on one thread: the
/// audio and the log's length.
fn render(plan: &ExecutionPlan, frames: u64, block: usize) -> (Vec<f64>, usize) {
    let config = EngineConfig {
        block_capacity: block,
        threads: 1,
        ..EngineConfig::new(48_000, 1)
    };
    let mut engine = Engine::new(plan, &types(), config, Vec::new()).unwrap();
    let render = render_blocks(&mut engine, frames, block, |_| Ok(None)).unwrap();
    (render.samples, render.log.len())
}

#[test]
fn a_dense_event_plan_renders_the_same_in_any_block() {
    let plan = dense(16.0, 3);
    let whole = render(&plan, 600, 256);
    assert!(whole.1 > 0, "events delivered and logged");
    for block in [1, 7, 64] {
        assert_eq!(render(&plan, 600, block), whole, "block {block}");
    }
}

#[test]
#[ignore = "a benchmark; run with --release"]
fn event_delivery_cost() {
    let frames = 48_000 * 5;
    let blocks = frames / 256;
    println!("blocks of 256 frames, one thread, {frames} frames");
    println!("| events per frame | receivers | µs per block | over no events (µs) |");
    println!("|---:|---:|---:|---:|");
    for fanout in [1usize, 8] {
        let time = |count: f64| {
            let plan = dense(count, fanout);
            // Once to warm up.
            render(&plan, 48_000, 256);
            let start = Instant::now();
            std::hint::black_box(render(&plan, frames, 256));
            start.elapsed().as_secs_f64() * 1e6 / blocks as f64
        };
        let none = time(0.0);
        for count in [0.0, 1.0, 4.0, 16.0] {
            let us = time(count);
            println!("| {count} | {fanout} | {us:.1} | {:+.1} |", us - none);
        }
    }
}
