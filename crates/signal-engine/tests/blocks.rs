//! Block execution (M2.6-1, ADR 0019 §1, `docs/semantics.md` §13): for every
//! block size and a seeded random partition, the recorded audio, the log, the
//! parameters and the counters equal the per-frame engine's. The scenarios
//! put every in-block feature at frames inside blocks: faults (signal, event
//! overflow in any emission order) with the undo of the frames after them,
//! external events up front, late and pushed ahead, ramps and modulation,
//! suspension fades and held outputs, swaps with held modulation, a
//! suspend-then-remove, a cycle, and a log loaded by many targets at once.

mod common;

use common::{addr, conn, ext, inst, inst_with, port_addr, ramp, registry, scaled};
use signal_core::{
    Combine, DirectPrimitive, Direction, Event, EventPayload, Feedthrough, Frame, Inputs,
    ModuleDescriptor, ModuleType, Outputs, ParameterDescriptor, PortDescriptor, PortKind,
    PortShape, PrepareContext, Primitive, Range, RateDivisor, Registry, SplitMix64,
};
use signal_engine::{
    Engine, EngineConfig, EngineError, ExternalEvent, InputDiscards, render_blocks, render_with,
};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, InstanceDef, build_plan};

/// The block sizes of the matrix (AC1).
const SIZES: [usize; 7] = [1, 2, 3, 7, 64, 128, 256];
/// Seeds of the random partitions.
const PARTITIONS: [u64; 3] = [1, 2, 3];
/// Thread counts of the matrix (ADR 0019 §2).
const THREADS: [usize; 3] = [1, 4, 8];

/// Pushes `count[f]` triggers for each frame `f` of a call, the last frame
/// first: a block call emits out of frame order, a one-frame call as `test.flood`.
struct FloodReverse;

const FLOOD_REVERSE: ModuleDescriptor = ModuleDescriptor {
    type_name: "test.flood_reverse",
    doc: "fixture: per-frame triggers emitted in reverse frame order",
    ports: &[PortDescriptor {
        name: "out",
        direction: Direction::Out,
        shape: PortShape::Events,
        rate: RateDivisor::ONE,
        kind: PortKind::Trigger,
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
            max: 64.0,
        },
        default: 0.0,
        combine: Combine::Add,
    }],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
};

impl DirectPrimitive for FloodReverse {
    fn describe() -> ModuleDescriptor {
        FLOOD_REVERSE
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {}
    fn reset(&mut self) {}
    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let counts = inputs.parameter(0);
        for f in (0..inputs.frames()).rev() {
            let count = counts.get(f).copied().unwrap_or(0.0) as usize;
            for _ in 0..count {
                if let Some(sink) = outputs.events(0) {
                    sink.push(Event {
                        offset: f as u32,
                        payload: EventPayload::Trigger,
                    });
                }
            }
        }
    }
}

/// The plan of a root composite of fixture instances, `test.flood_reverse`
/// included.
fn plan(instances: Vec<InstanceDef>, connections: Vec<ConnectionDef>) -> ExecutionPlan {
    let root = CompositeDef {
        name: "project".to_owned(),
        instances,
        connections,
        exports: Exports::default(),
    };
    build_plan(&root, &[], &types(), 1).unwrap()
}

fn types() -> Registry {
    let mut registry = registry();
    registry
        .register(ModuleType {
            descriptor: FLOOD_REVERSE,
            create: || Primitive::Direct(Box::new(FloodReverse)),
        })
        .unwrap();
    registry
}

/// Input pushed during a run, at a frame before it runs.
enum Cmd {
    /// An external event and its stamp (earlier than the push: late).
    Event(u64, &'static str, EventPayload),
    Suspend(u64, &'static str),
    Resume(u64, &'static str),
    /// A swap to plan `n`.
    Swap(usize),
}

struct Scenario {
    rate: u32,
    frames: u64,
    plans: Vec<ExecutionPlan>,
    upfront: Vec<ExternalEvent>,
    /// (frame it is pushed before, command), in frame order.
    script: Vec<(u64, Cmd)>,
    /// Parameters and instances whose final state is compared.
    parameters: Vec<&'static str>,
    instances: Vec<&'static str>,
}

/// What a run produced.
#[derive(Debug, PartialEq)]
struct Trace {
    samples: Vec<u64>,
    log: String,
    parameters: Vec<Option<(u64, u64)>>,
    discards: Vec<Option<u64>>,
    muted: Vec<Option<bool>>,
    input_discards: InputDiscards,
    log_overflows: u64,
    input_overflows: u64,
}

enum Partition {
    /// The per-frame reference: `render_with`, one frame per call.
    PerFrame,
    Blocks(usize),
    Random(u64),
}

/// Pushes the commands due at the next frame; returns the next one's frame.
fn step(
    engine: &mut Engine,
    scenario: &Scenario,
    registry: &Registry,
    next: &mut usize,
) -> Result<Option<Frame>, EngineError> {
    engine.reclaim_externals().for_each(drop);
    engine.reclaim_edits().for_each(drop);
    let now = engine.next_frame();
    while let Some((_, cmd)) = scenario.script.get(*next).filter(|(at, _)| *at == now.0) {
        *next += 1;
        match *cmd {
            Cmd::Event(stamp, target, payload) => engine
                .push_external(ExternalEvent {
                    frame: Frame(stamp),
                    sequence: 1_000_000 + *next as u64,
                    target: port_addr(target),
                    payload,
                })
                .unwrap(),
            Cmd::Suspend(frame, a) => engine.push_suspend(addr(a), Frame(frame)).unwrap(),
            Cmd::Resume(frame, a) => engine.push_resume(addr(a), Frame(frame)).unwrap(),
            Cmd::Swap(n) => {
                let mut swap = engine.prepare_swap(&scenario.plans[n], registry)?;
                engine.commit_swap(&mut swap, now)?;
            }
        }
    }
    Ok(scenario.script.get(*next).map(|(at, _)| Frame(*at)))
}

fn run(scenario: &Scenario, partition: &Partition, threads: usize) -> Trace {
    let registry = types();
    let config = common::config_threads(scenario.rate, 42, threads);
    let mut engine = Engine::new(
        &scenario.plans[0],
        &registry,
        config,
        scenario.upfront.clone(),
    )
    .unwrap();
    let mut next = 0;
    let frames = scenario.frames;
    let rendered = match *partition {
        Partition::PerFrame => render_with(&mut engine, frames, |engine| {
            step(engine, scenario, &registry, &mut next).map(|_| ())
        }),
        Partition::Blocks(k) => render_blocks(&mut engine, frames, k, |engine| {
            step(engine, scenario, &registry, &mut next)
        }),
        Partition::Random(seed) => {
            let mut rng = SplitMix64::new(seed);
            render_blocks(&mut engine, frames, 256, |engine| {
                let command = step(engine, scenario, &registry, &mut next)?;
                let cut = Frame(engine.next_frame().0 + 1 + rng.next_u64() % 256);
                Ok(Some(command.map_or(cut, |at| at.min(cut))))
            })
        }
    }
    .unwrap();
    assert_eq!(next, scenario.script.len(), "every command was pushed");
    Trace {
        samples: rendered.samples.iter().map(|x| x.to_bits()).collect(),
        log: String::from_utf8(rendered.log).unwrap(),
        parameters: scenario
            .parameters
            .iter()
            .map(|p| {
                engine
                    .parameter(&port_addr(p))
                    .map(|v| (v.base.to_bits(), v.effective.to_bits()))
            })
            .collect(),
        discards: scenario
            .instances
            .iter()
            .map(|i| engine.discards(&addr(i)))
            .collect(),
        muted: scenario
            .instances
            .iter()
            .map(|i| engine.is_muted(&addr(i)))
            .collect(),
        input_discards: engine.input_discards(),
        log_overflows: engine.log_overflows(),
        input_overflows: engine.input_overflows(),
    }
}

/// Every block size and partition, on 1, 4 and 8 threads (ADR 0019 §2,
/// every segment on the workers), gives the per-frame trace, with the
/// checked mode's exclusivity tracker on.
fn matrix(scenario: &Scenario) -> Trace {
    let reference = run(scenario, &Partition::PerFrame, 1);
    assert_eq!(reference.log_overflows, 0);
    assert_eq!(reference.input_overflows, 0);
    for threads in THREADS {
        for k in SIZES {
            assert_eq!(
                run(scenario, &Partition::Blocks(k), threads),
                reference,
                "k = {k}, {threads} threads"
            );
        }
        for seed in PARTITIONS {
            assert_eq!(
                run(scenario, &Partition::Random(seed), threads),
                reference,
                "random partition {seed}, {threads} threads"
            );
        }
    }
    let exclusivity = signal_engine::exclusivity();
    assert!(exclusivity.checked && exclusivity.claims > 0);
    assert_eq!(exclusivity.violations, 0);
    reference
}

fn set(frame: u64, sequence: u64, target: &str, value: f64) -> ExternalEvent {
    ext(frame, sequence, target, EventPayload::Set(value))
}

/// Faults inside blocks: a NaN and a 2·10⁶ output from frame 8, and event
/// overflows reached by ramps, one emitted in reverse frame order. Events
/// and parameter events keep arriving at the muted instances.
#[test]
fn faults_act_exactly_from_their_frame() {
    let plans = vec![plan(
        vec![
            inst_with("flood", "test.flood", &[("count", 0.0)]),
            inst("reverse", "test.flood_reverse"),
            inst("counter", "test.counter"),
            inst("counter2", "test.counter"),
            inst("faulty", "test.faulty"),
            inst("loud", "test.loud"),
            inst_with("gain", "test.gain", &[("gain", 0.5)]),
            inst("rec", "test.recorder"),
        ],
        vec![
            conn("flood.out", "counter.in"),
            conn("reverse.out", "counter2.in"),
            scaled("counter.out", "rec.zeta", 0.001),
            scaled("counter2.out", "rec.alpha", 0.001),
            conn("faulty.out", "rec.zeta"),
            conn("loud.out", "gain.in"),
            conn("gain.out", "rec.alpha"),
        ],
    )];
    let upfront = vec![
        ext(3, 1, "/flood.count", ramp(30.0, 60)),
        ext(10, 2, "/reverse.count", ramp(40.0, 80)),
        set(50, 3, "/flood.count", 5.0),
        ext(55, 4, "/flood.count", ramp(2.0, 20)),
        set(70, 5, "/reverse.count", 1.0),
        ext(20, 6, "/counter.in", EventPayload::Trigger),
        ext(90, 7, "/counter.in", EventPayload::Trigger),
        ext(5, 8, "/gain.gain", ramp(2.0, 100)),
    ];
    let scenario = Scenario {
        rate: 48_000,
        frames: 400,
        plans,
        upfront,
        script: vec![(0, Cmd::Event(150, "/counter2.in", EventPayload::Trigger))],
        parameters: vec!["/flood.count", "/reverse.count", "/gain.gain"],
        instances: vec![
            "/flood",
            "/reverse",
            "/faulty",
            "/loud",
            "/counter",
            "/counter2",
        ],
    };
    let trace = matrix(&scenario);
    assert_eq!(
        trace.muted,
        vec![
            Some(true),
            Some(true),
            Some(true),
            Some(true),
            Some(false),
            Some(false)
        ]
    );
    assert!(trace.log.contains("\"reason\":\"event_overflow\""));
    assert!(trace.log.contains("\"reason\":\"non_finite\""));
    assert!(trace.log.contains("\"reason\":\"magnitude\""));
    assert!(trace.log.contains("\"reason\":\"muted\""));
}

/// Events in canonical order at their frames inside blocks (up front, from
/// modules, duplicated connections, late, pushed ahead), parameter sets,
/// ramps and modulation per frame, and the sink's gain per frame.
#[test]
fn events_and_parameters_land_at_their_frames() {
    let plans = vec![plan(
        vec![
            inst_with("trig", "test.trigger", &[("period", 3.0)]),
            inst_with("setter", "test.setter", &[("value", 2.0)]),
            inst("pair", "test.pair"),
            inst("seq", "test.sequence"),
            inst("counter", "test.counter"),
            inst_with("osc", "test.osc", &[("freq", 440.0)]),
            inst_with("probe", "test.probe_add", &[("value", 0.2)]),
            inst("mul", "test.probe_mul"),
            inst_with("rec", "test.gained_recorder", &[("gain", 0.5)]),
        ],
        vec![
            conn("pair.out", "seq.in"),
            conn("setter.out", "seq.in"),
            conn("trig.out", "counter.in"),
            conn("trig.out", "counter.in"),
            scaled("osc.out", "probe.value", 0.5),
            conn("setter.out", "mul.value"),
            scaled("seq.out", "rec.l", 0.0001),
            scaled("counter.out", "rec.r", 0.01),
            conn("probe.out", "rec.r"),
            conn("mul.out", "rec.l"),
        ],
    )];
    let upfront = vec![
        set(5, 1, "/setter.value", 1.0),
        set(7, 2, "/seq.in", 3.0),
        set(7, 3, "/seq.in", 4.0),
        ext(9, 4, "/probe.value", EventPayload::Trigger),
        ext(20, 5, "/rec.gain", ramp(1.0, 100)),
        set(70, 6, "/setter.value", 0.25),
        set(90, 7, "/probe.value", 0.1),
        ext(100, 8, "/probe.value", ramp(0.9, 50)),
        ext(130, 9, "/trig.period", ramp(7.0, 40)),
    ];
    let scenario = Scenario {
        rate: 48_000,
        frames: 300,
        plans,
        upfront,
        script: vec![
            (0, Cmd::Event(150, "/seq.in", EventPayload::Set(7.0))),
            (0, Cmd::Event(151, "/mul.value", EventPayload::Set(0.75))),
            (120, Cmd::Event(110, "/counter.in", EventPayload::Trigger)),
            (120, Cmd::Event(200, "/counter.in", EventPayload::Trigger)),
            (
                240,
                Cmd::Event(230, "/setter.value", EventPayload::Set(0.5)),
            ),
        ],
        parameters: vec![
            "/setter.value",
            "/probe.value",
            "/mul.value",
            "/rec.gain",
            "/trig.period",
        ],
        instances: vec!["/probe", "/seq", "/counter"],
    };
    let trace = matrix(&scenario);
    assert!(trace.log.contains("\"reason\":\"not_a_parameter_event\""));
}

/// Suspends and resumes inside blocks: audio outputs fade, other outputs
/// hold, events are discarded and sets act on the base; a second edit of an
/// instance in one frame is discarded.
#[test]
fn suspension_fades_and_holds_inside_blocks() {
    let plans = vec![plan(
        vec![
            inst("dc", "test.dc"),
            inst_with("cst", "test.const", &[("value", 0.3)]),
            inst("frames", "test.frames"),
            inst_with("trig", "test.trigger", &[("period", 2.0)]),
            inst("counter", "test.counter"),
            inst("probe", "test.probe_add"),
            inst("rec", "test.recorder"),
        ],
        vec![
            conn("dc.out", "rec.zeta"),
            conn("cst.out", "rec.alpha"),
            scaled("frames.out", "probe.value", 0.001),
            conn("trig.out", "counter.in"),
            scaled("counter.out", "rec.alpha", 0.01),
            scaled("probe.out", "rec.zeta", 0.1),
        ],
    )];
    let scenario = Scenario {
        rate: 48_000,
        frames: 700,
        plans,
        upfront: vec![
            set(100, 1, "/cst.value", -0.4),
            ext(110, 2, "/counter.in", EventPayload::Trigger),
            ext(160, 3, "/probe.value", ramp(0.8, 30)),
        ],
        script: vec![
            (0, Cmd::Suspend(30, "/dc")),
            (0, Cmd::Suspend(30, "/cst")),
            (0, Cmd::Suspend(30, "/dc")),
            (0, Cmd::Suspend(50, "/frames")),
            (0, Cmd::Resume(51, "/frames")),
            (0, Cmd::Suspend(60, "/trig")),
            (0, Cmd::Suspend(61, "/counter")),
            (0, Cmd::Suspend(150, "/probe")),
            (0, Cmd::Resume(200, "/trig")),
            (0, Cmd::Resume(201, "/counter")),
            (0, Cmd::Resume(250, "/probe")),
            (0, Cmd::Resume(300, "/cst")),
            (0, Cmd::Resume(400, "/dc")),
            (500, Cmd::Suspend(490, "/dc")),
        ],
        parameters: vec!["/cst.value", "/probe.value"],
        instances: vec!["/dc", "/cst", "/counter", "/probe"],
    };
    let trace = matrix(&scenario);
    assert_eq!(trace.input_discards.invalid, 1, "the second suspend of /dc");
}

/// A swap removes a modulation source (held modulation), adds an instance
/// (fade-in), and a suspended instance is removed at its fade's end; a late
/// set releases the held modulation.
#[test]
fn swaps_and_a_removal_split_blocks() {
    let v0 = plan(
        vec![
            inst_with("osc", "test.osc", &[("freq", 300.0)]),
            inst("probe", "test.probe_add"),
            inst("dc", "test.dc"),
            inst("cst", "test.const"),
            inst("rec", "test.recorder"),
        ],
        vec![
            conn("osc.out", "probe.value"),
            conn("dc.out", "rec.zeta"),
            conn("probe.out", "rec.alpha"),
            conn("cst.out", "rec.zeta"),
        ],
    );
    let mut v1 = plan(
        vec![
            inst("probe", "test.probe_add"),
            inst("dc", "test.dc"),
            inst("cst", "test.const"),
            inst("noise", "test.noise"),
            inst("rec", "test.recorder"),
        ],
        vec![
            conn("dc.out", "rec.zeta"),
            conn("probe.out", "rec.alpha"),
            scaled("noise.out", "rec.alpha", 0.25),
        ],
    );
    v1.version = 1;
    let mut v2 = plan(
        vec![
            inst("probe", "test.probe_add"),
            inst("cst", "test.const"),
            inst("noise", "test.noise"),
            inst("rec", "test.recorder"),
        ],
        vec![
            conn("probe.out", "rec.alpha"),
            scaled("noise.out", "rec.alpha", 0.25),
            conn("cst.out", "rec.zeta"),
        ],
    );
    v2.version = 2;
    // F = 40 frames at 8 kHz: /dc, suspended at 150, is removed at 190.
    let scenario = Scenario {
        rate: 8_000,
        frames: 600,
        plans: vec![v0, v1, v2],
        upfront: vec![],
        script: vec![
            (0, Cmd::Event(40, "/probe.value", EventPayload::Set(0.1))),
            (100, Cmd::Swap(1)),
            (100, Cmd::Suspend(150, "/dc")),
            (100, Cmd::Event(120, "/cst.value", EventPayload::Set(0.5))),
            (190, Cmd::Swap(2)),
            (300, Cmd::Event(290, "/probe.value", EventPayload::Set(0.3))),
        ],
        parameters: vec!["/probe.value", "/cst.value"],
        instances: vec!["/probe", "/cst"],
    };
    matrix(&scenario);
}

/// A loop through a unit delay and an event-receiving state-output instance
/// (one-frame sub-blocks).
#[test]
fn a_cycle_runs_frame_by_frame_inside_blocks() {
    let plans = vec![plan(
        vec![
            inst("imp", "test.impulse"),
            inst_with("gain", "test.gain", &[("gain", 0.9)]),
            inst("delay", "test.unit_delay"),
            inst_with("trig", "test.trigger", &[("period", 5.0)]),
            inst("events", "test.event_delay"),
            inst("rec", "test.recorder"),
        ],
        vec![
            conn("imp.out", "gain.in"),
            conn("delay.out", "gain.in"),
            conn("gain.out", "delay.in"),
            conn("gain.out", "rec.zeta"),
            conn("trig.out", "events.in"),
            scaled("events.out", "rec.alpha", 0.01),
        ],
    )];
    let scenario = Scenario {
        rate: 48_000,
        frames: 300,
        plans,
        upfront: vec![ext(40, 1, "/gain.gain", ramp(-0.5, 100))],
        script: vec![
            (0, Cmd::Suspend(77, "/delay")),
            (0, Cmd::Resume(90, "/delay")),
        ],
        parameters: vec!["/gain.gain"],
        instances: vec!["/delay", "/events"],
    };
    matrix(&scenario);
}

/// The log of a block holds every external of the block, all targets in
/// total: many same-frame events to two targets (auditor-1-log-external-bound).
#[test]
fn many_externals_to_several_targets_fit_the_log() {
    let plans = vec![plan(
        vec![
            inst("p1", "test.probe_add"),
            inst("p2", "test.probe_mul"),
            inst("rec", "test.recorder"),
        ],
        vec![conn("p1.out", "rec.zeta"), conn("p2.out", "rec.alpha")],
    )];
    let mut upfront = Vec::new();
    for n in 0..600u64 {
        let value = (n % 10) as f64 / 10.0;
        upfront.push(set(5, 2 * n, "/p1.value", value));
        upfront.push(set(5, 2 * n + 1, "/p2.value", value));
        upfront.push(set(200 + n % 7, 10_000 + n, "/p1.value", value));
    }
    let scenario = Scenario {
        rate: 48_000,
        frames: 300,
        plans,
        upfront,
        script: vec![],
        parameters: vec!["/p1.value", "/p2.value"],
        instances: vec!["/p1", "/p2"],
    };
    let trace = matrix(&scenario);
    assert_eq!(trace.log.lines().count(), 1_800);
}

/// `run_frame` is `run_block(1)`, and a block never exceeds the capacity.
#[test]
fn run_block_runs_up_to_the_capacity() {
    let plan = plan(
        vec![inst("frames", "test.frames"), inst("rec", "test.recorder")],
        vec![conn("frames.out", "rec.zeta")],
    );
    let config = EngineConfig {
        block_capacity: 7,
        ..EngineConfig::new(48_000, 1)
    };
    let mut engine = Engine::new(&plan, &types(), config, vec![]).unwrap();
    assert_eq!(engine.run_block(0), 0);
    assert_eq!(engine.run_block(5), 5);
    assert_eq!(engine.run_block(100), 7);
    assert_eq!(engine.block_frames(), 7);
    engine.run_frame();
    assert_eq!(engine.block_frames(), 1);
    assert_eq!(engine.next_frame(), Frame(13));
    // The frame counter saw every frame once.
    assert_eq!(engine.signal_output(&port_addr("/frames.out")), Some(12.0));
    let zero = EngineConfig {
        block_capacity: 0,
        ..config
    };
    assert!(matches!(
        Engine::new(&plan, &types(), zero, vec![]),
        Err(EngineError::InvalidBlockCapacity)
    ));
}
