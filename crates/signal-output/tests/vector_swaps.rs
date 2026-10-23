//! M2.8-9 AC8: a vector stream's decimation is checked again after every
//! swap, in real time (`Vectors::bind`) and offline (`VectorTaps`). `/x.out`
//! is a vector of 4 at divisor 1 (`test.vec_d1`) or 4 (`test.vec_d4`); a
//! stream every 2 frames is on the ticks of the first and not of the second,
//! so where the second runs it is refused, with its reason, and never
//! sampled. A swap around the adapter cannot be written (the `compile_fail`
//! example on `signal_output::Controller`), so the adapter's plan versions
//! are the engine's.

use std::collections::BTreeMap;

use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Frame, Inputs, ModuleDescriptor, ModuleType, Outputs,
    PortAddress, PortDescriptor, PortKind, PortShape, PrepareContext, Primitive, RateDivisor,
    Registry,
};
use signal_engine::{Engine, EngineConfig, RealtimeConfig, Stamp};
use signal_graph::{CompositeDef, ExecutionPlan, Exports, InstanceDef, build_plan};
use signal_output::{VectorTap, VectorUnbound, realtime, render_blocks_tapped};

/// `out`: a vector of 4 at divisor `D`, every element the tick count.
#[derive(Default)]
struct Vec4<const D: u32> {
    ticks: f64,
}

const fn divisor(d: u32) -> RateDivisor {
    match RateDivisor::new(d) {
        Some(d) => d,
        None => RateDivisor::ONE,
    }
}

const fn out(d: u32) -> [PortDescriptor; 1] {
    [PortDescriptor {
        name: "out",
        direction: Direction::Out,
        shape: PortShape::Vector(4),
        rate: divisor(d),
        kind: PortKind::Control,
        unit: None,
        range: None,
    }]
}

const OUT_D1: [PortDescriptor; 1] = out(1);
const OUT_D4: [PortDescriptor; 1] = out(4);

impl<const D: u32> DirectPrimitive for Vec4<D> {
    fn describe() -> ModuleDescriptor {
        ModuleDescriptor {
            type_name: if D == 1 { "test.vec_d1" } else { "test.vec_d4" },
            doc: "a test hook: a vector of 4 at its divisor",
            ports: if D == 1 { &OUT_D1 } else { &OUT_D4 },
            parameters: &[],
            feedthrough: Feedthrough::Direct,
            sink: None,
            latency: None,
            reproducibility: signal_core::Reproducibility::BitExact,
            category: None,
        }
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {}
    fn reset(&mut self) {
        self.ticks = 0.0;
    }
    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let ticks = inputs.frames();
        let rows = outputs.rows(0, 4);
        for t in 0..ticks {
            self.ticks += 1.0;
            for r in 0..4 {
                if let Some(x) = rows.get_mut(r * ticks + t) {
                    *x = self.ticks;
                }
            }
        }
    }
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    for module_type in [
        ModuleType {
            descriptor: Vec4::<1>::describe(),
            create: || Primitive::Direct(Box::new(Vec4::<1>::default())),
        },
        ModuleType {
            descriptor: Vec4::<4>::describe(),
            create: || Primitive::Direct(Box::new(Vec4::<4>::default())),
        },
    ] {
        registry.register(module_type).unwrap();
    }
    registry
}

/// `/x` of `module`, and the audio sink a run needs.
fn plan(registry: &Registry, module: &str) -> ExecutionPlan {
    let instance = |name: &str, module: &str| InstanceDef {
        name: name.to_owned(),
        module: module.to_owned(),
        parameters: BTreeMap::new(),
        scope: None,
    };
    let root = CompositeDef {
        name: "project".to_owned(),
        instances: vec![instance("x", module), instance("out", "std.audio_out")],
        connections: Vec::new(),
        exports: Exports::default(),
    };
    build_plan(&root, &[], registry, 0).unwrap()
}

fn port() -> PortAddress {
    "/x.out".parse().unwrap()
}

/// The divisor-4 plan from F, the divisor-1 plan again from G.
const F: u64 = 1_024;
const G: u64 = 2_048;

/// Two swaps sent through the adapter before either commits (to the
/// divisor-4 plan at F, back to the divisor-1 plan at G): between them the
/// stream is unbound for its decimation, with no snapshot; before F and from
/// G on, snapshots fall every 2 frames. Then, the swaps confirmed, a stream
/// started on the running plan binds and snapshots at its ticks.
#[test]
fn a_stream_off_its_ports_ticks_after_a_swap_is_refused_in_real_time() {
    let registry = registry();
    let (d1, d4) = (
        plan(&registry, "test.vec_d1"),
        plan(&registry, "test.vec_d4"),
    );
    let (mut runner, mut controller) =
        realtime(&d1, &registry, RealtimeConfig::new(48_000, 1)).unwrap();
    controller.vector_start(&d1, 0, port(), 4, 2).unwrap();
    assert_eq!(
        controller.vector_start(&d4, 1, port(), 4, 2),
        Err(signal_output::VectorError::Decimation),
        "refused up front against the plan named"
    );
    controller.swap(&d4, Stamp::At(Frame(F))).unwrap();
    controller.swap(&d1, Stamp::At(Frame(G))).unwrap();

    let mut buffer = vec![0.0f32; 64 * 2];
    let mut frames = Vec::new();
    let mut between = None;
    while runner.next_frame().0 < 3_072 {
        runner.process(&mut buffer, 2);
        controller.poll(&mut ());
        controller.poll_vectors(|s| frames.push(s.frame.0));
        if controller.status().version == 1 {
            between = Some((controller.vector_bound(0), controller.vector_unbound(0)));
        }
    }
    assert_eq!(
        between,
        Some((false, Some(VectorUnbound::Decimation))),
        "unbound, for its decimation, while the divisor-4 plan runs"
    );
    assert!(frames.iter().all(|f| f % 2 == 0), "{frames:?}");
    assert!(
        frames.iter().all(|f| !(F..G).contains(f)),
        "sampled while the divisor-4 plan ran: {frames:?}"
    );
    assert!(frames.iter().any(|&f| f < F) && frames.iter().any(|&f| f >= G));
    assert!(controller.vector_bound(0));
    assert_eq!(controller.vector_unbound(0), None);

    // The swaps confirmed: a start on the running plan binds at its ticks.
    assert_eq!(controller.status().version, 2);
    controller.vector_stop(0).unwrap();
    controller.vector_start(&d1, 1, port(), 4, 4).unwrap();
    let mut later = Vec::new();
    for _ in 0..8 {
        runner.process(&mut buffer, 2);
        controller.poll(&mut ());
        controller.poll_vectors(|s| later.push((s.slot, s.frame.0)));
    }
    assert!(controller.vector_bound(1));
    assert!(!later.is_empty() && later.iter().all(|&(slot, f)| slot == 1 && f % 4 == 0));
}

/// Offline, the same: a tap every 2 frames is refused from the swap to the
/// divisor-4 plan, recorded with its frame and reason, never sampled there;
/// a plan version with no plan given is refused as unverified.
#[test]
fn an_offline_tap_off_its_ports_ticks_after_a_swap_is_refused() {
    let registry = registry();
    let (d1, d4) = (
        plan(&registry, "test.vec_d1"),
        plan(&registry, "test.vec_d4"),
    );
    let taps = [VectorTap {
        port: port(),
        rows: 4,
        decimation: 2,
    }];
    let run = |plans: &[ExecutionPlan]| {
        let mut engine =
            Engine::new(&d1, &registry, EngineConfig::new(48_000, 5), Vec::new()).unwrap();
        let before = |engine: &mut Engine| {
            if engine.next_frame() == Frame(F) {
                let mut swap = engine.prepare_swap(&d4, &registry)?;
                engine.commit_swap(&mut swap, Frame(F))?;
            }
            Ok((engine.next_frame() < Frame(F)).then_some(Frame(F)))
        };
        let (_, mut tracks) =
            render_blocks_tapped(&mut engine, 2_048, 256, before, &taps, plans).unwrap();
        tracks.remove(0)
    };
    let track = run(&[d1.clone(), d4.clone()]);
    assert_eq!(track.refused, vec![(Frame(F), VectorUnbound::Decimation)]);
    assert_eq!(track.snapshots.len() as u64, F / 2);
    assert!(track.snapshots.iter().all(|(f, _)| f.0 < F && f.0 % 2 == 0));
    let track = run(std::slice::from_ref(&d1));
    assert_eq!(track.refused, vec![(Frame(F), VectorUnbound::Unverified)]);
    assert!(track.snapshots.iter().all(|(f, _)| f.0 < F));
}

/// Finding auditor-1-006: a swap the engine refuses (here 32 prepared by
/// another run's Controller, enough to fill the vector command queue while
/// the runner takes nothing) leaves no verdict that a later swap could use.
/// The swap that follows, to the divisor-4 plan at frame 64, is never
/// sampled off its ticks, though its own verdict could not go before it;
/// once that verdict is delivered the stream stays unbound for its
/// decimation, and a swap back binds it again.
#[test]
fn a_refused_swap_leaves_no_verdict_behind() {
    let registry = registry();
    let (d1, d4) = (
        plan(&registry, "test.vec_d1"),
        plan(&registry, "test.vec_d4"),
    );
    let (mut runner, mut controller) =
        realtime(&d1, &registry, RealtimeConfig::new(48_000, 1)).unwrap();
    let (_other_runner, other) = realtime(&d1, &registry, RealtimeConfig::new(48_000, 1)).unwrap();
    controller.vector_start(&d1, 0, port(), 4, 2).unwrap();
    let mut buffer = vec![0.0f32; 64 * 2];
    runner.process(&mut buffer, 2);
    controller.poll(&mut ());
    controller.poll_vectors(|_| {});
    for _ in 0..32 {
        let ready = other.prepare_swap(&d1).unwrap();
        assert!(controller.send_swap(ready, Stamp::At(Frame(64))).is_err());
    }
    controller.swap(&d4, Stamp::At(Frame(64))).unwrap();
    let mut frames = Vec::new();
    for _ in 0..8 {
        runner.process(&mut buffer, 2);
        controller.poll(&mut ());
        controller.poll_vectors(|s| frames.push(s.frame.0));
    }
    assert_eq!(controller.status().version, 1);
    assert!(
        frames.iter().all(|f| *f < 64 || f % 4 == 0),
        "sampled off the divisor-4 plan's ticks: {frames:?}"
    );
    assert_eq!(
        controller.vector_unbound(0),
        Some(VectorUnbound::Decimation)
    );
    // Back to divisor 1: bound again, every 2 frames.
    let back = runner.next_frame().0 + 256;
    controller.swap(&d1, Stamp::At(Frame(back))).unwrap();
    let mut after = Vec::new();
    for _ in 0..12 {
        runner.process(&mut buffer, 2);
        controller.poll(&mut ());
        controller.poll_vectors(|s| after.push(s.frame.0));
    }
    assert!(controller.vector_bound(0));
    assert!(after.iter().any(|&f| f >= back) && after.iter().all(|f| f % 2 == 0));
}
