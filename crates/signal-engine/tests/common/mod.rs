//! Test-only fixture primitives (no `signal-std`), their registry, and builders
//! for plans, engines and external events.

// Each test binary uses part of this module.
#![allow(dead_code)]

use signal_core::{
    Combine, Curve, DirectPrimitive, Direction, Event, EventPayload, EventSink, Feedthrough, Frame,
    Inputs, InstanceAddress, ModuleDescriptor, ModuleType, Outputs, ParameterDescriptor,
    PortAddress, PortDescriptor, PortKind, PortShape, PrepareContext, Primitive, Range,
    RateDivisor, Registry, RngStream, SinkKind, SplitMix64, StateOutputPrimitive,
};
use signal_engine::{Engine, EngineConfig, ExternalEvent, Schedule};
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
        latency: None,
        reproducibility: signal_core::Reproducibility::BitExact,
    }
}

fn first(values: &[f64]) -> f64 {
    values.first().copied().unwrap_or(0.0)
}

/// One frame of a call's inputs, as a fixture body reads them: each signal
/// and parameter as a one-sample slice, and the events of that frame.
pub struct FrameIn<'a, 'b> {
    inputs: &'a Inputs<'b>,
    f: usize,
}

impl FrameIn<'_, '_> {
    fn signal(&self, port: usize) -> &[f64] {
        self.inputs.signal(port).get(self.f..=self.f).unwrap_or(&[])
    }

    fn parameter(&self, index: usize) -> &[f64] {
        self.inputs
            .parameter(index)
            .get(self.f..=self.f)
            .unwrap_or(&[])
    }

    fn events(&self, port: usize) -> &[Event] {
        let events = self.inputs.events(port);
        let start = events.partition_point(|e| (e.offset as usize) < self.f);
        let end = events.partition_point(|e| (e.offset as usize) <= self.f);
        events.get(start..end).unwrap_or(&[])
    }
}

/// One frame of a call's outputs, as a fixture body writes them.
pub struct FrameOut<'a, 'b> {
    outputs: &'a mut Outputs<'b>,
    f: usize,
}

/// Where `write` and `push` put a frame's sample and events: one frame of a
/// direct fixture's call, or a state-output fixture's one-frame call.
trait FrameOutputs {
    fn sample(&mut self, port: usize) -> Option<&mut f64>;
    fn sink(&mut self, port: usize) -> Option<(&mut EventSink, u32)>;
}

impl FrameOutputs for FrameOut<'_, '_> {
    fn sample(&mut self, port: usize) -> Option<&mut f64> {
        self.outputs.signal(port).get_mut(self.f)
    }

    fn sink(&mut self, port: usize) -> Option<(&mut EventSink, u32)> {
        let offset = self.f as u32;
        self.outputs.events(port).map(|sink| (sink, offset))
    }
}

impl FrameOutputs for Outputs<'_> {
    fn sample(&mut self, port: usize) -> Option<&mut f64> {
        self.signal(port).first_mut()
    }

    fn sink(&mut self, port: usize) -> Option<(&mut EventSink, u32)> {
        self.events(port).map(|sink| (sink, 0))
    }
}

fn write(outputs: &mut impl FrameOutputs, port: usize, value: f64) {
    if let Some(out) = outputs.sample(port) {
        *out = value;
    }
}

fn push(outputs: &mut impl FrameOutputs, port: usize, payload: EventPayload) {
    if let Some((sink, offset)) = outputs.sink(port) {
        sink.push(Event { offset, payload });
    }
}

/// Declares a direct fixture: its descriptor, its state and its `process`
/// for one frame. A call of *k* frames runs the body once per frame, on that
/// frame's inputs and outputs, so every fixture is partition-invariant.
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

            fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
                for f in 0..inputs.frames() {
                    let $i = &FrameIn { inputs, f };
                    let $o = &mut FrameOut {
                        outputs: &mut *outputs,
                        f,
                    };
                    let $p = &mut *self;
                    $process
                }
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

/// The thread that last ran a `test.fail*` fixture's failing frame (M2.7-3),
/// so a test can tell a worker's failure from the calling thread's. Tests
/// that read it run one at a time.
pub static FAILED_ON: std::sync::Mutex<Option<std::thread::ThreadId>> = std::sync::Mutex::new(None);

/// Records the thread of a failing frame.
fn failing_here() {
    if let Ok(mut on) = FAILED_ON.lock() {
        *on = Some(std::thread::current().id());
    }
}

/// `out = in + value`; at its frame `at` it panics, or with `nan` set
/// outputs NaN there instead (the fault guard's oracle for the panic).
/// A direct fixture that breaks the module contract on purpose (M2.7-3).
pub const FAIL: ModuleDescriptor = descriptor(
    "test.fail",
    &[
        port("in", In, Signal, Audio),
        port("out", Out, Signal, Audio),
    ],
    &[
        param("value", -10.0, 10.0, 0.0, Combine::Add),
        param("at", 0.0, 1e9, 0.0, Combine::Add),
        param("nan", 0.0, 1.0, 0.0, Combine::Add),
    ],
    Feedthrough::Direct,
    None,
);
direct!(Fail, FAIL, { frame: f64 }, |_s, _ctx| {}, |p, inputs, outputs| {
    let failing = p.frame == first(inputs.parameter(1));
    p.frame += 1.0;
    if failing {
        failing_here();
        if first(inputs.parameter(2)) > 0.5 {
            write(outputs, 0, f64::NAN);
            return;
        }
        panic!("test.fail: a module panics in process");
    }
    write(outputs, 0, first(inputs.signal(0)) + first(inputs.parameter(0)));
});

/// The audio sink `test.recorder`, panicking at its frame `at`: a module
/// without outputs that fails.
pub const FAIL_SINK: ModuleDescriptor = descriptor(
    "test.fail_sink",
    &[port("in", In, Signal, Audio)],
    &[param("at", 0.0, 1e9, 0.0, Combine::Add)],
    Feedthrough::Direct,
    Some(SinkKind::Audio),
);
direct!(FailSink, FAIL_SINK, { frame: f64 }, |_s, _ctx| {}, |p, inputs, _outputs| {
    let failing = p.frame == first(inputs.parameter(0));
    p.frame += 1.0;
    if failing {
        failing_here();
        panic!("test.fail_sink: a sink panics in process");
    }
});

/// `out[n] = in[n − 1]` (state-output). At its frame `at` the call `phase`
/// names (0: `emit`, 1: `absorb`) panics, or with `nan` set `emit` outputs
/// NaN there instead. Its parameters reach it in `absorb`, so `at` > 0.
pub const FAIL_STATE: ModuleDescriptor = descriptor(
    "test.fail_state",
    &[
        port("in", In, Signal, Audio),
        port("out", Out, Signal, Audio),
    ],
    &[
        param("at", 0.0, 1e9, 0.0, Combine::Add),
        param("nan", 0.0, 1.0, 0.0, Combine::Add),
        param("phase", 0.0, 1.0, 0.0, Combine::Add),
    ],
    Feedthrough::StateOutput,
    None,
);

#[derive(Default)]
pub struct FailState {
    last: f64,
    /// Frames absorbed: the frame `emit` is called for.
    frame: f64,
    at: f64,
    nan: bool,
    absorb: bool,
}

impl StateOutputPrimitive for FailState {
    fn describe() -> ModuleDescriptor {
        FAIL_STATE
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {
        *self = FailState {
            at: -1.0,
            ..FailState::default()
        };
    }
    fn reset(&mut self) {
        self.last = 0.0;
    }
    fn emit(&self, outputs: &mut Outputs<'_>) {
        if !self.absorb && self.frame == self.at {
            failing_here();
            if self.nan {
                write(outputs, 0, f64::NAN);
                return;
            }
            panic!("test.fail_state: a module panics in emit");
        }
        write(outputs, 0, self.last);
    }
    fn absorb(&mut self, inputs: &Inputs<'_>) {
        self.at = first(inputs.parameter(0));
        self.nan = first(inputs.parameter(1)) > 0.5;
        self.absorb = first(inputs.parameter(2)) > 0.5;
        if self.absorb && self.frame == self.at {
            failing_here();
            panic!("test.fail_state: a module panics in absorb");
        }
        self.last = first(inputs.signal(0));
        self.frame += 1.0;
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
        direct_type::<Fail>(),
        direct_type::<FailSink>(),
        state_output_type::<UnitDelay>(),
        state_output_type::<EventDelay>(),
        state_output_type::<FailState>(),
    ] {
        registry.register(module_type).unwrap();
    }
    register_rates(&mut registry);
    registry
}

/// The divisors of M2.8-6's AC1.
pub const DIVISORS: [u32; 5] = [1, 2, 3, 64, 1600];
/// The vector sizes of M2.8-6's AC1.
pub const SIZES: [u32; 3] = [1, 4, 256];

/// A vector fixture: its kind, ports, whether it has `gain`, its class and
/// its constructor.
type VectorKind = (
    &'static str,
    Vec<PortDescriptor>,
    bool,
    Feedthrough,
    fn() -> Primitive,
);

fn leak<T>(items: Vec<T>) -> &'static [T] {
    Box::leak(items.into_boxed_slice())
}

/// M2.8-6 fixtures: `test.osc`, `test.gain`, `test.unit_delay` and
/// `test.faulty` again at each divisor above 1 as `d<d>.<type>` (every port
/// and parameter at *d*), and the vector types `test.v<kind>.n<N>.d<d>` for
/// every size and divisor: `spread` (a signal to N elements, element *e*
/// weighted (e + 1) / N, times `gain`), `gain` (each element times `gain`),
/// `mix` (the elements summed in order, times `gain`) and `delay` (a
/// state-output unit delay per element).
fn register_rates(registry: &mut Registry) {
    let rated = |d: u32| RateDivisor::new(d).unwrap();
    for t in [
        direct_type::<Osc>(),
        direct_type::<Gain>(),
        direct_type::<Faulty>(),
        state_output_type::<UnitDelay>(),
    ] {
        for &d in &DIVISORS[1..] {
            let mut descriptor = t.descriptor;
            descriptor.type_name =
                Box::leak(format!("d{d}.{}", descriptor.type_name).into_boxed_str());
            descriptor.ports = leak(
                descriptor
                    .ports
                    .iter()
                    .map(|p| PortDescriptor {
                        rate: rated(d),
                        ..*p
                    })
                    .collect(),
            );
            descriptor.parameters = leak(
                descriptor
                    .parameters
                    .iter()
                    .map(|p| ParameterDescriptor {
                        rate: rated(d),
                        ..*p
                    })
                    .collect(),
            );
            registry
                .register(ModuleType {
                    descriptor,
                    create: t.create,
                })
                .unwrap();
        }
    }
    for d in DIVISORS {
        for n in SIZES {
            let at = |p: PortDescriptor| PortDescriptor {
                rate: rated(d),
                ..p
            };
            let v = PortShape::Vector(n);
            let gain = ParameterDescriptor {
                rate: rated(d),
                ..param("gain", -2.0, 2.0, 1.0, Combine::Add)
            };
            let kinds: [VectorKind; 4] = [
                (
                    "spread",
                    vec![
                        at(port("in", In, Signal, Control)),
                        at(port("out", Out, v, Control)),
                    ],
                    true,
                    Feedthrough::Direct,
                    || Primitive::Direct(Box::new(VSpread)),
                ),
                (
                    "gain",
                    vec![
                        at(port("in", In, v, Control)),
                        at(port("out", Out, v, Control)),
                    ],
                    true,
                    Feedthrough::Direct,
                    || Primitive::Direct(Box::new(VGain)),
                ),
                (
                    "mix",
                    vec![
                        at(port("in", In, v, Control)),
                        at(port("out", Out, Signal, Control)),
                    ],
                    true,
                    Feedthrough::Direct,
                    || Primitive::Direct(Box::new(VMix)),
                ),
                (
                    "delay",
                    vec![
                        at(port("in", In, v, Control)),
                        at(port("out", Out, v, Control)),
                    ],
                    false,
                    Feedthrough::StateOutput,
                    || {
                        Primitive::StateOutput(Box::new(VDelay {
                            state: vec![0.0; 256],
                        }))
                    },
                ),
            ];
            for (kind, ports, has_gain, feedthrough, create) in kinds {
                let type_name = Box::leak(format!("test.v{kind}.n{n}.d{d}").into_boxed_str());
                let parameters = if has_gain { vec![gain] } else { Vec::new() };
                let descriptor = ModuleDescriptor {
                    ports: leak(ports),
                    parameters: leak(parameters),
                    ..descriptor(type_name, &[], &[], feedthrough, None)
                };
                registry
                    .register(ModuleType { descriptor, create })
                    .unwrap();
            }
        }
    }
}

/// M2.8-6: a plan with instances at divisors 1, 3, 64 and 1600, vectors of
/// 4 and 256 elements (a gain and a delay in a feedback loop at 64), holds
/// and decimates, into `test.gained_recorder` `/out`. `second` replaces the
/// connection `o64.out -> out.r` with `o3.out -> out.r`: a converter removed
/// (`/o64/out_d1` stays for `osc.freq`) and one added (`/o3/out_d1`).
pub fn rates_plan(second: bool) -> ExecutionPlan {
    let instances = vec![
        inst_with("osc", "test.osc", &[("freq", 110.0)]),
        inst_with("o3", "d3.test.osc", &[("freq", 50.0)]),
        inst_with("o64", "d64.test.osc", &[("freq", 2.0)]),
        inst("vs", "test.vspread.n256.d64"),
        inst_with("vg", "test.vgain.n256.d64", &[("gain", 0.5)]),
        inst("vd", "test.vdelay.n256.d64"),
        inst("vm", "test.vmix.n256.d64"),
        inst("v4", "test.vspread.n4.d1600"),
        inst("m4", "test.vmix.n4.d1600"),
        inst("ud", "d3.test.unit_delay"),
        inst_with("out", "test.gained_recorder", &[("gain", 0.5)]),
    ];
    let mut connections = vec![
        conn("osc.out", "vs.in"),
        conn("vs.out", "vg.in"),
        scaled("vd.out", "vg.in", 0.25),
        conn("vg.out", "vd.in"),
        conn("vd.out", "vm.in"),
        conn("o64.out", "v4.in"),
        conn("v4.out", "m4.in"),
        conn("o3.out", "ud.in"),
        scaled("o64.out", "osc.freq", 10.0),
        conn("osc.out", "out.l"),
        scaled("vm.out", "out.l", 0.01),
        conn("ud.out", "out.l"),
        conn("m4.out", "out.r"),
    ];
    connections.push(if second {
        conn("o3.out", "out.r")
    } else {
        conn("o64.out", "out.r")
    });
    plan(instances, connections)
}

/// A signal to a vector of N: element `e` is `in · gain · (e + 1) / N`.
struct VSpread;

impl DirectPrimitive for VSpread {
    fn describe() -> ModuleDescriptor {
        descriptor("test.vspread", &[], &[], Feedthrough::Direct, None)
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {}
    fn reset(&mut self) {}
    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let n = outputs.signals().count() as f64;
        let (x, g) = (inputs.signal(0), inputs.parameter(0));
        for (e, row) in outputs.signals().enumerate() {
            let w = (e + 1) as f64 / n;
            for (out, (x, g)) in row.iter_mut().zip(x.iter().zip(g)) {
                *out = x * g * w;
            }
        }
    }
}

/// Each element times `gain`.
struct VGain;

impl DirectPrimitive for VGain {
    fn describe() -> ModuleDescriptor {
        descriptor("test.vgain", &[], &[], Feedthrough::Direct, None)
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {}
    fn reset(&mut self) {}
    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let g = inputs.parameter(0);
        for (e, row) in outputs.signals().enumerate() {
            for (out, (x, g)) in row.iter_mut().zip(inputs.signal(e).iter().zip(g)) {
                *out = x * g;
            }
        }
    }
}

/// The elements summed in order, times `gain`.
struct VMix;

impl DirectPrimitive for VMix {
    fn describe() -> ModuleDescriptor {
        descriptor("test.vmix", &[], &[], Feedthrough::Direct, None)
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {}
    fn reset(&mut self) {}
    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let g = inputs.parameter(0);
        let rows = inputs.signal_rows();
        for (t, out) in outputs.signal(0).iter_mut().enumerate() {
            let mut sum = 0.0;
            for e in 0..rows {
                sum += inputs.signal(e)[t];
            }
            *out = sum * g[t];
        }
    }
}

/// A unit delay per element (up to 256).
struct VDelay {
    state: Vec<f64>,
}

impl StateOutputPrimitive for VDelay {
    fn describe() -> ModuleDescriptor {
        descriptor("test.vdelay", &[], &[], Feedthrough::StateOutput, None)
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.state.fill(0.0);
    }
    fn reset(&mut self) {
        self.state.fill(0.0);
    }
    fn emit(&self, outputs: &mut Outputs<'_>) {
        for (row, &value) in outputs.signals().zip(&self.state) {
            row.fill(value);
        }
    }
    fn absorb(&mut self, inputs: &Inputs<'_>) {
        for (e, state) in self.state.iter_mut().enumerate().take(inputs.signal_rows()) {
            if let Some(&x) = inputs.signal(e).last() {
                *state = x;
            }
        }
    }
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
    build_plan(&root, &[], &latent_registry(), 1).unwrap()
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
    let config = EngineConfig::new(sample_rate, seed);
    Engine::new(plan, &registry(), config, external).unwrap()
}

/// The configuration of an engine on `threads` threads (ADR 0019 §2); with
/// more than one, every segment of a plan with two tasks or more runs on the
/// workers, one task per unit (`Schedule::FORCED`).
pub fn config_threads(sample_rate: u32, seed: u64, threads: usize) -> EngineConfig {
    EngineConfig {
        threads,
        schedule: if threads > 1 {
            Schedule::FORCED
        } else {
            Schedule::DEFAULT
        },
        ..EngineConfig::new(sample_rate, seed)
    }
}

/// [`engine`] on `threads` threads ([`config_threads`]).
pub fn engine_threads(
    plan: &ExecutionPlan,
    sample_rate: u32,
    threads: usize,
    external: Vec<ExternalEvent>,
) -> Engine {
    let config = config_threads(sample_rate, 42, threads);
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

/// M2.7-4: `test.mixed_latent`, a latent module without a GPU: 1 block of
/// 4 frames, `same_machine`; `out` in block m is Σ `in` over block m − 1.
/// The due result of an even block is in at the first `due`, an odd
/// block's only at the third, so real time delivers the even blocks and
/// misses the odd ones. `polls` is the `due` calls of the current block,
/// `settles` the `settle` calls since `prepare`. Its delivered result is one
/// word, which an exact replay may inject.
pub const MIXED_LATENT: ModuleDescriptor = ModuleDescriptor {
    latency: Some(signal_core::BlockLatency {
        latency_blocks: 1,
        block_frames: MIXED_BLOCK as u32,
    }),
    reproducibility: signal_core::Reproducibility::SameMachine,
    ..descriptor(
        "test.mixed_latent",
        &[
            port("in", In, Signal, Audio),
            port("out", Out, Signal, Audio),
            port("polls", Out, Signal, Control),
            port("settles", Out, Signal, Control),
        ],
        &[],
        Feedthrough::StateOutput,
        None,
    )
};

/// `test.mixed_latent`'s block, in frames.
pub const MIXED_BLOCK: usize = 4;

#[derive(Default)]
pub struct MixedLatent {
    frame: usize,
    block: u64,
    sum: f64,
    pending: f64,
    shown: [f32; 1],
    polls: u32,
    settles: u32,
    resolved: bool,
}

impl StateOutputPrimitive for MixedLatent {
    fn describe() -> ModuleDescriptor {
        MIXED_LATENT
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {
        *self = MixedLatent::default();
    }
    fn reset(&mut self) {
        *self = MixedLatent::default();
    }
    fn due(&mut self) -> signal_core::Due {
        use signal_core::Due;
        if self.frame != 0 || self.resolved {
            return Due::Ready;
        }
        if self.block == 0 {
            self.resolved = true;
            return Due::Ready;
        }
        self.polls += 1;
        let needed = if self.block.is_multiple_of(2) { 1 } else { 3 };
        if self.polls < needed {
            return Due::Pending;
        }
        self.shown = [self.pending as f32];
        self.resolved = true;
        Due::Ready
    }
    fn miss(&mut self) {
        self.shown = [0.0];
        self.resolved = true;
    }
    fn result_words(&self) -> usize {
        1
    }
    fn delivered_result(&self) -> &[f32] {
        &self.shown
    }
    fn inject_result(&mut self, words: &[f32]) -> bool {
        let [word] = words else {
            return false;
        };
        self.shown = [*word];
        self.resolved = true;
        true
    }
    fn settle(&mut self) -> signal_core::Due {
        self.settles += 1;
        signal_core::Due::Ready
    }
    fn emit(&self, outputs: &mut Outputs<'_>) {
        outputs.signal(0).fill(f64::from(self.shown[0]));
        outputs.signal(1).fill(f64::from(self.polls));
        outputs.signal(2).fill(f64::from(self.settles));
    }
    fn absorb(&mut self, inputs: &Inputs<'_>) {
        self.sum += inputs.signal(0).first().copied().unwrap_or(0.0);
        self.frame += 1;
        if self.frame == MIXED_BLOCK {
            self.pending = self.sum;
            self.sum = 0.0;
            self.frame = 0;
            self.block += 1;
            self.resolved = false;
            self.polls = 0;
        }
    }
}

/// [`registry`] plus `test.mixed_latent`.
pub fn latent_registry() -> Registry {
    let mut registry = registry();
    registry
        .register(state_output_type::<MixedLatent>())
        .unwrap();
    registry
}
