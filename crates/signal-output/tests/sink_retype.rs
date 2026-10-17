//! A swap may keep the audio sink's address and width but change its type,
//! and so whether it declares `gain`. The audio adapter rebinds the gain on
//! every plan, offline (`AudioSink::same`) and in real time
//! (`AudioSink::follow`): a gain the new type adds is applied, one it drops
//! is no longer, and a new value is respected. `/out` is `std.audio_out`
//! (with `gain`) or `test.bare_out` (the same ports, no parameters), fed a
//! constant 0.5 on both channels.

use std::collections::BTreeMap;

use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Frame, Inputs, ModuleDescriptor, ModuleType, Outputs,
    PortDescriptor, PortKind, PortShape, PrepareContext, Primitive, Registry,
};
use signal_engine::{Engine, EngineConfig, RealtimeConfig, Stamp};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, InstanceDef, build_plan};
use signal_output::{Driver, NullDriver, realtime, render_with};

/// `out`: 0.5 on every frame.
struct Half;

const HALF_OUT: [PortDescriptor; 1] = [PortDescriptor {
    name: "out",
    direction: Direction::Out,
    shape: PortShape::Signal,
    rate: signal_core::RateDivisor::ONE,
    kind: PortKind::Audio,
    unit: None,
    range: None,
}];

impl DirectPrimitive for Half {
    fn describe() -> ModuleDescriptor {
        ModuleDescriptor {
            type_name: "test.half",
            doc: "a test hook: 0.5 on every frame",
            ports: &HALF_OUT,
            parameters: &[],
            feedthrough: Feedthrough::Direct,
            sink: None,
            latency: None,
            reproducibility: signal_core::Reproducibility::BitExact,
            category: None,
        }
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {}
    fn reset(&mut self) {}
    fn process(&mut self, _inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        outputs.signal(0).fill(0.5);
    }
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let mut bare = signal_std::AudioOut::describe();
    bare.type_name = "test.bare_out";
    bare.parameters = &[];
    for module_type in [
        ModuleType {
            descriptor: bare,
            create: || Primitive::Direct(Box::new(signal_std::AudioOut)),
        },
        ModuleType {
            descriptor: Half::describe(),
            create: || Primitive::Direct(Box::new(Half)),
        },
    ] {
        registry.register(module_type).unwrap();
    }
    registry
}

/// `/out` as `std.audio_out` with `gain` (`Some`) or as `test.bare_out`
/// (`None`), both channels fed 0.5.
fn plan(registry: &Registry, gain: Option<f64>, version: u64) -> ExecutionPlan {
    let (module, parameters) = match gain {
        Some(gain) => ("std.audio_out", BTreeMap::from([("gain".to_owned(), gain)])),
        None => ("test.bare_out", BTreeMap::new()),
    };
    let connection = |to: &str| ConnectionDef {
        from: "source.out".to_owned(),
        to: to.to_owned(),
        scale: None,
    };
    let root = CompositeDef {
        name: "project".to_owned(),
        instances: vec![
            InstanceDef {
                name: "source".to_owned(),
                module: "test.half".to_owned(),
                parameters: BTreeMap::new(),
            },
            InstanceDef {
                name: "out".to_owned(),
                module: module.to_owned(),
                parameters,
            },
        ],
        connections: vec![connection("out.l"), connection("out.r")],
        exports: Exports::default(),
    };
    build_plan(&root, &[], registry, version).unwrap()
}

/// The second plan from F, the third from G.
const F: u64 = 512;
const G: u64 = 1_024;
const FRAMES: u64 = 1_536;

/// The gains of the two sequences: added then removed, and removed then
/// added with a new value.
const SEQUENCES: [[Option<f64>; 3]; 2] = [[None, Some(0.25), None], [Some(0.25), None, Some(0.75)]];

/// The sample of frame `f` for `gains`: 0.5 times the gain of the plan
/// running at `f`, if it declares one.
fn expected(gains: [Option<f64>; 3], f: u64) -> f64 {
    let gain = match f {
        f if f < F => gains[0],
        f if f < G => gains[1],
        _ => gains[2],
    };
    gain.map_or(0.5, |g| g * 0.5)
}

/// Checks interleaved stereo `samples` against [`expected`].
fn check(what: &str, gains: [Option<f64>; 3], samples: impl ExactSizeIterator<Item = f64>) {
    assert_eq!(samples.len() as u64, FRAMES * 2, "{what}");
    for (i, x) in samples.enumerate() {
        let f = i as u64 / 2;
        assert_eq!(x, expected(gains, f), "{what} {gains:?}: frame {f}");
    }
}

#[test]
fn a_retyped_sink_s_gain_follows_the_swap_offline() {
    let registry = registry();
    for gains in SEQUENCES {
        let plans: Vec<_> = (0..3)
            .map(|i| plan(&registry, gains[i], i as u64))
            .collect();
        let mut engine = Engine::new(
            &plans[0],
            &registry,
            EngineConfig::new(48_000, 1),
            Vec::new(),
        )
        .unwrap();
        let render = render_with(&mut engine, FRAMES, |engine| {
            let next = match engine.next_frame() {
                Frame(F) => &plans[1],
                Frame(G) => &plans[2],
                _ => return Ok(()),
            };
            let mut swap = engine.prepare_swap(next, &registry)?;
            engine.commit_swap(&mut swap, engine.next_frame())
        })
        .unwrap();
        check("offline", gains, render.samples.iter().copied());
    }
}

#[test]
fn a_retyped_sink_s_gain_follows_the_swap_in_real_time() {
    let registry = registry();
    for gains in SEQUENCES {
        let plans: Vec<_> = (0..3)
            .map(|i| plan(&registry, gains[i], i as u64))
            .collect();
        let (runner, mut controller) =
            realtime(&plans[0], &registry, RealtimeConfig::new(48_000, 1)).unwrap();
        controller.swap(&plans[1], Stamp::At(Frame(F))).unwrap();
        controller.swap(&plans[2], Stamp::At(Frame(G))).unwrap();
        let mut driver = NullDriver::new(48_000, 2, 7);
        driver.start(runner).unwrap();
        driver.run(FRAMES, || controller.poll(&mut ())).unwrap();
        check(
            "real time",
            gains,
            driver.output().iter().map(|&x| f64::from(x)),
        );
    }
}
