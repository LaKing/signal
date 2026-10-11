//! Test-only fixture primitives (no `signal-std`), their registry, and builders
//! for plans, engines and external events.

// Each test binary uses part of this module.
#![allow(dead_code)]

use signal_core::{
    Combine, Curve, DirectPrimitive, Direction, Event, EventPayload, Feedthrough, Frame, Inputs,
    InstanceAddress, ModuleDescriptor, ModuleType, Outputs, ParameterDescriptor, PortAddress,
    PortDescriptor, PortKind, PortShape, PrepareContext, Primitive, Range, RateDivisor, Registry,
    RngStream, SinkKind, SplitMix64, StateOutputPrimitive,
};
use signal_engine::{Engine, EngineConfig, ExternalEvent};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, InstanceDef, build_plan};

use Direction::{In, Out};
use PortKind::{Audio, Control, Trigger};
use PortShape::{Events, Signal};

const fn port(
    name: &'static str,
    direction: Direction,
    shape: PortShape,
    kind: PortKind,
) -> PortDescriptor {
    PortDescriptor {
        name,
        direction,
        shape,
        rate: RateDivisor::ONE,
        kind,
        unit: None,
        range: None,
    }
}

const fn param(
    name: &'static str,
    min: f64,
    max: f64,
    default: f64,
    combine: Combine,
) -> ParameterDescriptor {
    ParameterDescriptor {
        name,
        kind: Control,
        rate: RateDivisor::ONE,
        unit: None,
        range: Range { min, max },
        default,
        combine,
    }
}

const fn descriptor(
    type_name: &'static str,
    ports: &'static [PortDescriptor],
    parameters: &'static [ParameterDescriptor],
    feedthrough: Feedthrough,
    sink: Option<SinkKind>,
) -> ModuleDescriptor {
    ModuleDescriptor {
        type_name,
        doc: "fixture primitive for signal-engine tests",
        ports,
        parameters,
        feedthrough,
        sink,
    }
}

fn first(values: &[f64]) -> f64 {
    values.first().copied().unwrap_or(0.0)
}

fn write(outputs: &mut Outputs<'_>, port: usize, value: f64) {
    if let Some(out) = outputs.signal(port).first_mut() {
        *out = value;
    }
}

fn push(outputs: &mut Outputs<'_>, port: usize, payload: EventPayload) {
    if let Some(sink) = outputs.events(port) {
        sink.push(Event { offset: 0, payload });
    }
}

/// Declares a direct fixture: its descriptor, its state and its `process`.
macro_rules! direct {
    ($name:ident, $descriptor:expr, { $($field:ident: $ty:ty),* }, |$s:ident, $ctx:ident| $prepare:block, |$p:ident, $i:ident, $o:ident| $process:block) => {
        #[derive(Default)]
        pub struct $name {
            $($field: $ty,)*
        }

        impl DirectPrimitive for $name {
            fn describe() -> ModuleDescriptor {
                $descriptor
            }

            fn prepare(&mut self, $ctx: &PrepareContext) {
                *self = $name::default();
                let $s = self;
                $prepare
            }

            fn reset(&mut self) {}

            fn process(&mut self, $i: &Inputs<'_>, $o: &mut Outputs<'_>) {
                let $p = self;
                $process
            }
        }
    };
}

/// `out` = phase in [0, 1), advanced by `freq / sample_rate` after each frame.
pub const OSC: ModuleDescriptor = descriptor(
    "test.osc",
    &[port("out", Out, Signal, Audio)],
    &[param("freq", 0.0, 20_000.0, 440.0, Combine::Add)],
    Feedthrough::Direct,
    None,
);
direct!(Osc, OSC, { phase: f64, rate: f64 }, |s, ctx| { s.rate = ctx.sample_rate; }, |p, inputs, outputs| {
    write(outputs, 0, p.phase);
    p.phase += first(inputs.parameter(0)) / p.rate;
    if p.phase >= 1.0 {
        p.phase -= 1.0;
    }
});

/// `out` = the number of frames it has run before this one (control).
pub const FRAMES: ModuleDescriptor = descriptor(
    "test.frames",
    &[port("out", Out, Signal, Control)],
    &[],
    Feedthrough::Direct,
    None,
);
direct!(Frames, FRAMES, { count: f64 }, |_s, _ctx| {}, |p, _inputs, outputs| {
    write(outputs, 0, p.count);
    p.count += 1.0;
});

/// `out` = its parameter `value` (control).
pub const CONST: ModuleDescriptor = descriptor(
    "test.const",
    &[port("out", Out, Signal, Control)],
    &[param("value", -10.0, 10.0, 0.0, Combine::Add)],
    Feedthrough::Direct,
    None,
);
direct!(Const, CONST, {}, |_s, _ctx| {}, |_p, inputs, outputs| {
    write(outputs, 0, first(inputs.parameter(0)));
});

/// `out` = its parameter `value` (audio): a constant live value for fades.
pub const DC: ModuleDescriptor = descriptor(
    "test.dc",
    &[port("out", Out, Signal, Audio)],
    &[param("value", -1.0, 1.0, 0.75, Combine::Add)],
    Feedthrough::Direct,
    None,
);
direct!(Dc, DC, {}, |_s, _ctx| {}, |_p, inputs, outputs| {
    write(outputs, 0, first(inputs.parameter(0)));
});

/// `out = in · gain`.
pub const GAIN: ModuleDescriptor = descriptor(
    "test.gain",
    &[
        port("in", In, Signal, Audio),
        port("out", Out, Signal, Audio),
    ],
    &[param("gain", -4.0, 4.0, 1.0, Combine::Multiply)],
    Feedthrough::Direct,
    None,
);
direct!(Gain, GAIN, {}, |_s, _ctx| {}, |_p, inputs, outputs| {
    write(
        outputs,
        0,
        first(inputs.signal(0)) * first(inputs.parameter(0)),
    );
});

/// `out` = 1 on its first frame, then 0.
pub const IMPULSE: ModuleDescriptor = descriptor(
    "test.impulse",
    &[port("out", Out, Signal, Audio)],
    &[],
    Feedthrough::Direct,
    None,
);
direct!(Impulse, IMPULSE, { done: bool }, |_s, _ctx| {}, |p, _inputs, outputs| {
    write(outputs, 0, if p.done { 0.0 } else { 1.0 });
    p.done = true;
});

/// A trigger every `period` frames it runs, starting with its first.
pub const TRIGGER: ModuleDescriptor = descriptor(
    "test.trigger",
    &[port("out", Out, Events, Trigger)],
    &[param("period", 1.0, 1000.0, 1.0, Combine::Add)],
    Feedthrough::Direct,
    None,
);
direct!(TriggerSource, TRIGGER, { count: u64 }, |_s, _ctx| {}, |p, inputs, outputs| {
    let period = (first(inputs.parameter(0)) as u64).max(1);
    if p.count.is_multiple_of(period) {
        push(outputs, 0, EventPayload::Trigger);
    }
    p.count += 1;
});

/// `set(value)` every frame.
pub const SETTER: ModuleDescriptor = descriptor(
    "test.setter",
    &[port("out", Out, Events, Control)],
    &[param("value", -10.0, 10.0, 0.0, Combine::Add)],
    Feedthrough::Direct,
    None,
);
direct!(Setter, SETTER, {}, |_s, _ctx| {}, |_p, inputs, outputs| {
    push(outputs, 0, EventPayload::Set(first(inputs.parameter(0))));
});

/// `set(1)` then `set(2)` every frame.
pub const PAIR: ModuleDescriptor = descriptor(
    "test.pair",
    &[port("out", Out, Events, Control)],
    &[],
    Feedthrough::Direct,
    None,
);
direct!(Pair, PAIR, {}, |_s, _ctx| {}, |_p, _inputs, outputs| {
    push(outputs, 0, EventPayload::Set(1.0));
    push(outputs, 0, EventPayload::Set(2.0));
});

/// `out` = this frame's `set` values read as decimal digits in arrival order
/// (`[1, 1, 2, 2]` gives 1122): an order-sensitive receiver.
pub const SEQUENCE: ModuleDescriptor = descriptor(
    "test.sequence",
    &[
        port("in", In, Events, Control),
        port("out", Out, Signal, Control),
    ],
    &[],
    Feedthrough::Direct,
    None,
);
direct!(
    Sequence,
    SEQUENCE,
    {},
    |_s, _ctx| {},
    |_p, inputs, outputs| {
        let digits = inputs.events(0).iter().fold(0.0, |acc, e| match e.payload {
            EventPayload::Set(v) => acc * 10.0 + v,
            _ => acc,
        });
        write(outputs, 0, digits);
    }
);

/// `out` = the number of events received so far, this frame's included.
pub const COUNTER: ModuleDescriptor = descriptor(
    "test.counter",
    &[
        port("in", In, Events, Trigger),
        port("out", Out, Signal, Control),
    ],
    &[],
    Feedthrough::Direct,
    None,
);
direct!(Counter, COUNTER, { count: f64 }, |_s, _ctx| {}, |p, inputs, outputs| {
    p.count += inputs.events(0).len() as f64;
    write(outputs, 0, p.count);
});

/// Effective value of `value` (combine Add, range [0, 1]) as its output.
pub const PROBE_ADD: ModuleDescriptor = descriptor(
    "test.probe_add",
    &[port("out", Out, Signal, Control)],
    &[param("value", 0.0, 1.0, 0.5, Combine::Add)],
    Feedthrough::Direct,
    None,
);
direct!(
    ProbeAdd,
    PROBE_ADD,
    {},
    |_s, _ctx| {},
    |_p, inputs, outputs| {
        write(outputs, 0, first(inputs.parameter(0)));
    }
);

/// Effective value of `value` (combine Multiply, range [0, 1]) as its output.
pub const PROBE_MUL: ModuleDescriptor = descriptor(
    "test.probe_mul",
    &[port("out", Out, Signal, Control)],
    &[param("value", 0.0, 1.0, 0.5, Combine::Multiply)],
    Feedthrough::Direct,
    None,
);
direct!(
    ProbeMul,
    PROBE_MUL,
    {},
    |_s, _ctx| {},
    |_p, inputs, outputs| {
        write(outputs, 0, first(inputs.parameter(0)));
    }
);

/// Effective value of `value` (combine Replace, range [0, 1]) as its output.
pub const PROBE_REPLACE: ModuleDescriptor = descriptor(
    "test.probe_replace",
    &[port("out", Out, Signal, Control)],
    &[param("value", 0.0, 1.0, 0.5, Combine::Replace)],
    Feedthrough::Direct,
    None,
);
direct!(
    ProbeReplace,
    PROBE_REPLACE,
    {},
    |_s, _ctx| {},
    |_p, inputs, outputs| {
        write(outputs, 0, first(inputs.parameter(0)));
    }
);

/// `out` = the next value of its random stream.
pub const NOISE: ModuleDescriptor = descriptor(
    "test.noise",
    &[port("out", Out, Signal, Audio)],
    &[],
    Feedthrough::Direct,
    None,
);
direct!(Noise, NOISE, { rng: Option<RngStream> }, |s, ctx| { s.rng = Some(ctx.rng.clone()); }, |p, _inputs, outputs| {
    let value = p.rng.as_mut().map_or(0.0, RngStream::next_f64);
    write(outputs, 0, value);
});

/// `rate` and `frames`: what `prepare` was given.
pub const CONTEXT: ModuleDescriptor = descriptor(
    "test.context",
    &[
        port("rate", Out, Signal, Control),
        port("frames", Out, Signal, Control),
    ],
    &[],
    Feedthrough::Direct,
    None,
);
direct!(Context, CONTEXT, { rate: f64, frames: f64 }, |s, ctx| {
    s.rate = ctx.sample_rate;
    s.frames = ctx.max_frames as f64;
}, |p, _inputs, outputs| {
    write(outputs, 0, p.rate);
    write(outputs, 1, p.frames);
});

/// 0.5, then NaN from its 8th frame (frame index 8) on.
pub const FAULTY: ModuleDescriptor = descriptor(
    "test.faulty",
    &[port("out", Out, Signal, Audio)],
    &[],
    Feedthrough::Direct,
    None,
);
direct!(Faulty, FAULTY, { count: u64 }, |_s, _ctx| {}, |p, _inputs, outputs| {
    write(outputs, 0, if p.count >= 8 { f64::NAN } else { 0.5 });
    p.count += 1;
});

/// 0.5, then 2·10⁶ from its 8th frame on.
pub const LOUD: ModuleDescriptor = descriptor(
    "test.loud",
    &[port("out", Out, Signal, Audio)],
    &[],
    Feedthrough::Direct,
    None,
);
direct!(Loud, LOUD, { count: u64 }, |_s, _ctx| {}, |p, _inputs, outputs| {
    write(outputs, 0, if p.count >= 8 { 2e6 } else { 0.5 });
    p.count += 1;
});

/// Pushes `count` triggers every frame: 16 fills a sink, 17 overflows it.
pub const FLOOD: ModuleDescriptor = descriptor(
    "test.flood",
    &[port("out", Out, Events, Trigger)],
    &[param("count", 0.0, 64.0, 16.0, Combine::Add)],
    Feedthrough::Direct,
    None,
);
direct!(Flood, FLOOD, {}, |_s, _ctx| {}, |_p, inputs, outputs| {
    for _ in 0..first(inputs.parameter(0)) as usize {
        push(outputs, 0, EventPayload::Trigger);
    }
});

/// The audio sink: two inputs, declared `zeta` before `alpha`, so channel order
/// (port order) differs from alphabetical order.
pub const RECORDER: ModuleDescriptor = descriptor(
    "test.recorder",
    &[
        port("zeta", In, Signal, Audio),
        port("alpha", In, Signal, Audio),
    ],
    &[],
    Feedthrough::Direct,
    Some(SinkKind::Audio),
);
direct!(
    Recorder,
    RECORDER,
    {},
    |_s, _ctx| {},
    |_p, _inputs, _outputs| {}
);

/// An audio sink with a `gain` parameter, which the driver applies.
pub const GAINED_RECORDER: ModuleDescriptor = descriptor(
    "test.gained_recorder",
    &[port("l", In, Signal, Audio), port("r", In, Signal, Audio)],
    &[param("gain", 0.0, 4.0, 1.0, Combine::Multiply)],
    Feedthrough::Direct,
    Some(SinkKind::Audio),
);
direct!(
    GainedRecorder,
    GAINED_RECORDER,
    {},
    |_s, _ctx| {},
    |_p, _inputs, _outputs| {}
);

/// `out[n] = in[n − 1]`, `out[0] = 0` (state-output).
pub const UNIT_DELAY: ModuleDescriptor = descriptor(
    "test.unit_delay",
    &[
        port("in", In, Signal, Audio),
        port("out", Out, Signal, Audio),
    ],
    &[],
    Feedthrough::StateOutput,
    None,
);

#[derive(Default)]
pub struct UnitDelay {
    last: f64,
}

impl StateOutputPrimitive for UnitDelay {
    fn describe() -> ModuleDescriptor {
        UNIT_DELAY
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.last = 0.0;
    }
    fn reset(&mut self) {
        self.last = 0.0;
    }
    fn emit(&self, outputs: &mut Outputs<'_>) {
        write(outputs, 0, self.last);
    }
    fn absorb(&mut self, inputs: &Inputs<'_>) {
        self.last = first(inputs.signal(0));
    }
}

/// `out[n]` = events absorbed in frames before *n* (state-output).
pub const EVENT_DELAY: ModuleDescriptor = descriptor(
    "test.event_delay",
    &[
        port("in", In, Events, Trigger),
        port("out", Out, Signal, Control),
    ],
    &[],
    Feedthrough::StateOutput,
    None,
);

#[derive(Default)]
pub struct EventDelay {
    count: f64,
}

impl StateOutputPrimitive for EventDelay {
    fn describe() -> ModuleDescriptor {
        EVENT_DELAY
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.count = 0.0;
    }
    fn reset(&mut self) {
        self.count = 0.0;
    }
    fn emit(&self, outputs: &mut Outputs<'_>) {
        write(outputs, 0, self.count);
    }
    fn absorb(&mut self, inputs: &Inputs<'_>) {
        self.count += inputs.events(0).len() as f64;
    }
}

fn direct_type<T: DirectPrimitive + Default + 'static>() -> ModuleType {
    ModuleType {
        descriptor: T::describe(),
        create: || Primitive::Direct(Box::new(T::default())),
    }
}

fn state_output_type<T: StateOutputPrimitive + Default + 'static>() -> ModuleType {
    ModuleType {
        descriptor: T::describe(),
        create: || Primitive::StateOutput(Box::new(T::default())),
    }
}

/// Every fixture type.
pub fn registry() -> Registry {
    let mut registry = Registry::new();
    for module_type in [
        direct_type::<Osc>(),
        direct_type::<Frames>(),
        direct_type::<Const>(),
        direct_type::<Dc>(),
        direct_type::<Gain>(),
        direct_type::<Impulse>(),
        direct_type::<TriggerSource>(),
        direct_type::<Setter>(),
        direct_type::<Pair>(),
        direct_type::<Sequence>(),
        direct_type::<Counter>(),
        direct_type::<ProbeAdd>(),
        direct_type::<ProbeMul>(),
        direct_type::<ProbeReplace>(),
        direct_type::<Noise>(),
        direct_type::<Context>(),
        direct_type::<Faulty>(),
        direct_type::<Loud>(),
        direct_type::<Flood>(),
        direct_type::<Recorder>(),
        direct_type::<GainedRecorder>(),
        state_output_type::<UnitDelay>(),
        state_output_type::<EventDelay>(),
    ] {
        registry.register(module_type).unwrap();
    }
    registry
}

pub fn addr(text: &str) -> InstanceAddress {
    text.parse().unwrap()
}

pub fn port_addr(text: &str) -> PortAddress {
    text.parse().unwrap()
}

pub fn inst(name: &str, module: &str) -> InstanceDef {
    InstanceDef {
        name: name.to_owned(),
        module: module.to_owned(),
        parameters: Default::default(),
    }
}

pub fn inst_with(name: &str, module: &str, parameters: &[(&str, f64)]) -> InstanceDef {
    InstanceDef {
        parameters: parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), *value))
            .collect(),
        ..inst(name, module)
    }
}

pub fn conn(from: &str, to: &str) -> ConnectionDef {
    ConnectionDef {
        from: from.to_owned(),
        to: to.to_owned(),
        scale: None,
    }
}

pub fn scaled(from: &str, to: &str, scale: f64) -> ConnectionDef {
    ConnectionDef {
        scale: Some(scale),
        ..conn(from, to)
    }
}

/// The plan of a project (a root composite) of fixture instances.
pub fn plan(instances: Vec<InstanceDef>, connections: Vec<ConnectionDef>) -> ExecutionPlan {
    let root = CompositeDef {
        name: "project".to_owned(),
        instances,
        connections,
        exports: Exports::default(),
    };
    build_plan(&root, &[], &registry(), 1).unwrap()
}

/// An engine for `plan` at `sample_rate` and seed 42.
pub fn engine(plan: &ExecutionPlan, sample_rate: u32, external: Vec<ExternalEvent>) -> Engine {
    engine_seeded(plan, sample_rate, 42, external)
}

pub fn engine_seeded(
    plan: &ExecutionPlan,
    sample_rate: u32,
    seed: u64,
    external: Vec<ExternalEvent>,
) -> Engine {
    let config = EngineConfig { sample_rate, seed };
    Engine::new(plan, &registry(), config, external).unwrap()
}

pub fn ext(frame: u64, sequence: u64, target: &str, payload: EventPayload) -> ExternalEvent {
    ExternalEvent {
        frame: Frame(frame),
        sequence,
        target: port_addr(target),
        payload,
    }
}

pub fn ramp(to: f64, frames: u64) -> EventPayload {
    EventPayload::Ramp {
        to,
        frames,
        curve: Curve::Linear,
    }
}

/// Runs `frames` frames and returns `read` after each.
pub fn trace(engine: &mut Engine, frames: usize, read: impl Fn(&Engine) -> f64) -> Vec<f64> {
    (0..frames)
        .map(|_| {
            engine.run_frame();
            read(engine)
        })
        .collect()
}

/// The output of signal port `port` after each of `frames` frames.
pub fn output_trace(engine: &mut Engine, frames: usize, port: &str) -> Vec<f64> {
    let port = port_addr(port);
    trace(engine, frames, |e| e.signal_output(&port).unwrap())
}

/// The base of parameter `port` after each of `frames` frames.
pub fn base_trace(engine: &mut Engine, frames: usize, port: &str) -> Vec<f64> {
    let port = port_addr(port);
    trace(engine, frames, |e| e.parameter(&port).unwrap().base)
}

/// A topological order of `plan`'s per-frame dependency graph with ties broken
/// by a seeded random choice instead of by address (ADR 0016 §1).
pub fn random_order(plan: &ExecutionPlan, seed: u64) -> Vec<InstanceAddress> {
    let state_output = |a: &InstanceAddress| {
        plan.instances
            .iter()
            .any(|i| &i.address == a && i.descriptor.feedthrough == Feedthrough::StateOutput)
    };
    let mut edges = Vec::new();
    for (target, sources) in &plan.sources {
        for source in sources {
            let from = source.from.instance();
            if !state_output(&from) {
                edges.push((from, target.instance()));
            }
        }
    }
    let mut remaining: Vec<InstanceAddress> =
        plan.instances.iter().map(|i| i.address.clone()).collect();
    let mut rng = SplitMix64::new(seed);
    let mut order = Vec::new();
    while !remaining.is_empty() {
        let ready: Vec<usize> = (0..remaining.len())
            .filter(|&k| {
                !edges
                    .iter()
                    .any(|(from, to)| to == &remaining[k] && remaining.contains(from))
            })
            .collect();
        let pick = ready[(rng.next_u64() % ready.len() as u64) as usize];
        order.push(remaining.remove(pick));
    }
    order
}
