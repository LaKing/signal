//! Fake module types registered by the tests (no `signal-std`), description
//! builders, and the nested fixture shared by the snapshot and determinism tests.

// Each test binary uses part of this module.
#![allow(dead_code)]

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, InstanceAddress, ModuleDescriptor,
    ModuleType, Outputs, ParameterDescriptor, PortAddress, PortDescriptor, PortKind, PortShape,
    PrepareContext, Primitive, Range, RateDivisor, Registry, SinkKind, StateOutputPrimitive,
};
use signal_graph::{
    CompositeDef, ConnectionDef, ExecutionPlan, Exports, GraphError, InputExport, InstanceDef,
    OutputExport, ParameterExport, build_plan,
};

const TWO: RateDivisor = match RateDivisor::new(2) {
    Some(rate) => rate,
    None => RateDivisor::ONE,
};

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

const fn parameter(
    name: &'static str,
    kind: PortKind,
    max: f64,
    default: f64,
) -> ParameterDescriptor {
    ParameterDescriptor {
        name,
        kind,
        rate: RateDivisor::ONE,
        unit: None,
        range: Range { min: 0.0, max },
        default,
        combine: Combine::Add,
        values: None,
        snap: signal_core::Snap::Nearest,
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
        doc: "fake module type for signal-graph tests",
        ports,
        parameters,
        feedthrough,
        sink,
        latency: None,
        reproducibility: signal_core::Reproducibility::BitExact,
        category: None,
    }
}

use Direction::{In, Out};
use PortKind::{Audio, Control, Trigger};
use PortShape::{Events, Signal};

/// Signal source with a `freq` parameter in [0, 20000].
const OSC: ModuleDescriptor = descriptor(
    "test.osc",
    &[port("out", Out, Signal, Audio)],
    &[parameter("freq", PortKind::Frequency, 20_000.0, 440.0)],
    Feedthrough::Direct,
    None,
);
/// Event input `trig`, control output.
const ENV: ModuleDescriptor = descriptor(
    "test.env",
    &[
        port("trig", In, Events, Trigger),
        port("out", Out, Signal, Control),
    ],
    &[parameter("attack", Control, 10.0, 0.01)],
    Feedthrough::Direct,
    None,
);
const VCA: ModuleDescriptor = descriptor(
    "test.vca",
    &[
        port("in", In, Signal, Audio),
        port("cv", In, Signal, Control),
        port("out", Out, Signal, Audio),
    ],
    &[],
    Feedthrough::Direct,
    None,
);
/// The only state-output type.
const DELAY: ModuleDescriptor = descriptor(
    "test.delay",
    &[
        port("in", In, Signal, Audio),
        port("out", Out, Signal, Audio),
    ],
    &[],
    Feedthrough::StateOutput,
    None,
);
const TRIG: ModuleDescriptor = descriptor(
    "test.trig",
    &[port("out", Out, Events, Trigger)],
    &[],
    Feedthrough::Direct,
    None,
);
/// The only sink type.
const OUT: ModuleDescriptor = descriptor(
    "test.out",
    &[port("in", In, Signal, Audio)],
    &[],
    Feedthrough::Direct,
    Some(SinkKind::Audio),
);
/// Output at rate divisor 2.
const SLOW: ModuleDescriptor = descriptor(
    "test.slow",
    &[PortDescriptor {
        rate: TWO,
        ..port("out", Out, Signal, Audio)
    }],
    &[],
    Feedthrough::Direct,
    None,
);

/// Generic (any kind) input and output, as the core arithmetic types.
const GEN: ModuleDescriptor = descriptor(
    "test.gen",
    &[
        port("in", In, Signal, PortKind::Generic),
        port("out", Out, Signal, PortKind::Generic),
    ],
    &[],
    Feedthrough::Direct,
    None,
);
/// Generic output at rate divisor 2.
const GEN_SLOW: ModuleDescriptor = descriptor(
    "test.gen_slow",
    &[PortDescriptor {
        rate: TWO,
        ..port("out", Out, Signal, PortKind::Generic)
    }],
    &[],
    Feedthrough::Direct,
    None,
);

/// Event output at rate divisor 2.
const SLOW_TRIG: ModuleDescriptor = descriptor(
    "test.slow_trig",
    &[PortDescriptor {
        rate: TWO,
        ..port("out", Out, Events, Trigger)
    }],
    &[],
    Feedthrough::Direct,
    None,
);

/// A port at rate divisor `d`.
const fn at(d: u32, port: PortDescriptor) -> PortDescriptor {
    PortDescriptor {
        rate: match RateDivisor::new(d) {
            Some(rate) => rate,
            None => RateDivisor::ONE,
        },
        ..port
    }
}

/// M2.8-6: types with one signal or vector port at a divisor, for the
/// conversion tests (`tests/rates.rs`). `test.slow64` and `test.slow3`:
/// control outputs at 64 and 3.
const SLOW64: ModuleDescriptor = descriptor(
    "test.slow64",
    &[at(64, port("out", Out, Signal, Control))],
    &[],
    Feedthrough::Direct,
    None,
);
const SLOW3: ModuleDescriptor = descriptor(
    "test.slow3",
    &[at(3, port("out", Out, Signal, Control))],
    &[],
    Feedthrough::Direct,
    None,
);
/// Control inputs at 3, 64 and 1600.
const IN3: ModuleDescriptor = descriptor(
    "test.in3",
    &[at(3, port("in", In, Signal, Control))],
    &[],
    Feedthrough::Direct,
    None,
);
const IN64: ModuleDescriptor = descriptor(
    "test.in64",
    &[at(64, port("in", In, Signal, Control))],
    &[],
    Feedthrough::Direct,
    None,
);
const IN1600: ModuleDescriptor = descriptor(
    "test.in1600",
    &[at(1600, port("in", In, Signal, Control))],
    &[],
    Feedthrough::Direct,
    None,
);
/// Vector outputs of 4: at 1 and at 64.
const VSRC: ModuleDescriptor = descriptor(
    "test.vsrc",
    &[port("out", Out, PortShape::Vector(4), Control)],
    &[],
    Feedthrough::Direct,
    None,
);
const VSLOW: ModuleDescriptor = descriptor(
    "test.vslow",
    &[at(64, port("out", Out, PortShape::Vector(4), Control))],
    &[],
    Feedthrough::Direct,
    None,
);
/// Vector inputs: 4 at 64, 2 at 1.
const VIN64: ModuleDescriptor = descriptor(
    "test.vin64",
    &[at(64, port("in", In, PortShape::Vector(4), Control))],
    &[],
    Feedthrough::Direct,
    None,
);
const VIN2: ModuleDescriptor = descriptor(
    "test.vin2",
    &[port("in", In, PortShape::Vector(2), Control)],
    &[],
    Feedthrough::Direct,
    None,
);

/// Does nothing; signal-graph never runs modules.
struct Null;

impl DirectPrimitive for Null {
    fn describe() -> ModuleDescriptor {
        OSC
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {}
    fn reset(&mut self) {}
    fn process(&mut self, _inputs: &Inputs<'_>, _outputs: &mut Outputs<'_>) {}
}

/// Does nothing; signal-graph never runs modules.
struct NullState;

impl StateOutputPrimitive for NullState {
    fn describe() -> ModuleDescriptor {
        DELAY
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {}
    fn reset(&mut self) {}
    fn emit(&self, _outputs: &mut Outputs<'_>) {}
    fn absorb(&mut self, _inputs: &Inputs<'_>) {}
}

fn direct() -> Primitive {
    Primitive::Direct(Box::new(Null))
}

fn state_output() -> Primitive {
    Primitive::StateOutput(Box::new(NullState))
}

/// A registry of the fake types.
pub fn registry() -> Registry {
    let mut registry = Registry::new();
    for descriptor in [
        OSC, ENV, VCA, TRIG, OUT, SLOW, SLOW_TRIG, SLOW64, SLOW3, IN3, IN64, IN1600, VSRC, VSLOW,
        VIN64, VIN2, GEN, GEN_SLOW,
    ] {
        let module_type = ModuleType {
            descriptor,
            create: direct,
        };
        registry.register(module_type).unwrap();
    }
    registry
        .register(ModuleType {
            descriptor: DELAY,
            create: state_output,
        })
        .unwrap();
    registry
}

/// Builds the plan of `root` against the fake registry, as version 1.
pub fn build(
    root: &CompositeDef,
    composites: &[CompositeDef],
) -> Result<ExecutionPlan, Vec<GraphError>> {
    build_plan(root, composites, &registry(), 1)
}

/// The errors of a description that must fail.
pub fn errors(root: &CompositeDef, composites: &[CompositeDef]) -> Vec<GraphError> {
    build(root, composites).unwrap_err()
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
        scope: None,
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

pub fn composite(
    name: &str,
    instances: Vec<InstanceDef>,
    connections: Vec<ConnectionDef>,
) -> CompositeDef {
    CompositeDef {
        name: name.to_owned(),
        instances,
        connections,
        exports: Exports::default(),
    }
}

pub fn input(name: &str, to: &[&str]) -> InputExport {
    InputExport {
        name: name.to_owned(),
        to: to.iter().map(|s| (*s).to_owned()).collect(),
    }
}

pub fn output(name: &str, from: &str) -> OutputExport {
    OutputExport {
        name: name.to_owned(),
        from: from.to_owned(),
    }
}

pub fn param(name: &str, to: &[&str]) -> ParameterExport {
    ParameterExport {
        name: name.to_owned(),
        to: to.iter().map(|s| (*s).to_owned()).collect(),
        range: None,
        values: None,
    }
}

/// A project containing a composite (`kit`) containing a composite (`voice`),
/// with an exported fan-out (`voice.trigger` to both envelopes), an exported
/// parameter set from the root, a feedback loop through a state-output instance
/// (`kit`: `mix` -> `echo` -> `mix`), and a disconnected instance (`/lonely`).
pub fn nested() -> (CompositeDef, Vec<CompositeDef>) {
    let voice = CompositeDef {
        exports: Exports {
            inputs: vec![input("trigger", &["amp_env.trig", "pitch_env.trig"])],
            outputs: vec![output("out", "vca.out")],
            parameters: vec![param("pitch", &["osc.freq"])],
        },
        ..composite(
            "voice",
            vec![
                inst_with("osc", "test.osc", &[("freq", 220.0)]),
                inst("amp_env", "test.env"),
                inst("pitch_env", "test.env"),
                inst("vca", "test.vca"),
            ],
            vec![conn("osc.out", "vca.in"), conn("amp_env.out", "vca.cv")],
        )
    };
    let kit = CompositeDef {
        exports: Exports {
            inputs: vec![input("trigger", &["kick.trigger"])],
            outputs: vec![output("out", "mix.out")],
            parameters: vec![param("kick_pitch", &["kick.pitch"])],
        },
        ..composite(
            "kit",
            vec![
                inst("kick", "voice"),
                inst("mix", "test.vca"),
                inst("echo", "test.delay"),
            ],
            vec![
                conn("kick.out", "mix.in"),
                conn("mix.out", "echo.in"),
                ConnectionDef {
                    scale: Some(0.5),
                    ..conn("echo.out", "mix.in")
                },
            ],
        )
    };
    let root = composite(
        "project",
        vec![
            inst("clock", "test.trig"),
            inst_with("drums", "kit", &[("kick_pitch", 55.0)]),
            inst("speaker", "test.out"),
            inst("lonely", "test.osc"),
        ],
        vec![
            conn("clock.out", "drums.trigger"),
            conn("drums.out", "speaker.in"),
        ],
    );
    (root, vec![voice, kit])
}
