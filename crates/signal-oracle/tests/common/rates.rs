//! M2.8-6 fixtures (`tests/rates.rs`): types at rate divisors and with
//! vector ports, registered beside the standard primitives and the probe.
//!
//! - Every standard type without event ports, a declared latency or a sink,
//!   again at each divisor of [`DIVISORS`] above 1, as `d<d>.<type>`: the
//!   same primitive, every port and parameter at *d* (a primitive cannot
//!   tell which rate it runs at; it sees ticks).
//! - Vector types for each size of [`SIZES`] and each divisor, as
//!   `vec.<kind>.n<N>.d<d>`: `spread` (a signal to a vector, element *e*
//!   weighted (e + 1) / N), `gain` (a vector times its `gain`), `mix` (a
//!   vector summed in element order) and `delay` (a state-output unit delay
//!   per element). Each has a `gain` parameter except `delay`.
//! - `fail.d64`: a direct type at divisor 64 whose `process` panics when its
//!   `arm` parameter reaches 1 at a tick of the call.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, ModuleType,
    Outputs, ParameterDescriptor, PortDescriptor, PortKind, PortShape, PrepareContext, Primitive,
    Range, RateDivisor, Registry, StateOutputPrimitive,
};

/// The divisors of AC1.
pub const DIVISORS: [u32; 5] = [1, 2, 3, 64, 1600];
/// The vector sizes of AC1.
pub const SIZES: [u32; 3] = [1, 4, 256];
/// The most elements a fixture vector holds.
const MOST: usize = 256;

fn d(n: u32) -> RateDivisor {
    RateDivisor::new(n).unwrap()
}

/// A vector fixture: its kind, ports, whether it has `gain`, and its
/// constructor.
type VectorKind = (&'static str, Vec<PortDescriptor>, bool, fn() -> Primitive);

fn leak<T>(items: Vec<T>) -> &'static [T] {
    Box::leak(items.into_boxed_slice())
}

fn name(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn port(name: &'static str, direction: Direction, shape: PortShape, rate: u32) -> PortDescriptor {
    PortDescriptor {
        name,
        direction,
        shape,
        rate: d(rate),
        kind: PortKind::Control,
        unit: None,
        range: None,
    }
}

fn gain(rate: u32) -> ParameterDescriptor {
    ParameterDescriptor {
        name: "gain",
        kind: PortKind::Gain,
        rate: d(rate),
        unit: None,
        range: Range {
            min: -2.0,
            max: 2.0,
        },
        default: 1.0,
        combine: Combine::Add,
    }
}

fn descriptor(
    type_name: &'static str,
    ports: Vec<PortDescriptor>,
    parameters: Vec<ParameterDescriptor>,
    feedthrough: Feedthrough,
) -> ModuleDescriptor {
    ModuleDescriptor {
        type_name,
        doc: "M2.8-6 test fixture",
        ports: leak(ports),
        parameters: leak(parameters),
        feedthrough,
        sink: None,
        latency: None,
        reproducibility: signal_core::Reproducibility::BitExact,
        category: None,
    }
}

/// A signal to a vector: element `e` is `in · gain · (e + 1) / N`.
struct Spread;

impl DirectPrimitive for Spread {
    fn describe() -> ModuleDescriptor {
        descriptor("vec.spread", Vec::new(), Vec::new(), Feedthrough::Direct)
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

/// A vector times `gain`, element by element.
struct Gain;

impl DirectPrimitive for Gain {
    fn describe() -> ModuleDescriptor {
        descriptor("vec.gain", Vec::new(), Vec::new(), Feedthrough::Direct)
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

/// A vector summed in element order, times `gain`.
struct Mix;

impl DirectPrimitive for Mix {
    fn describe() -> ModuleDescriptor {
        descriptor("vec.mix", Vec::new(), Vec::new(), Feedthrough::Direct)
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

/// A unit delay per element (state-output): each element's last input.
struct Delay {
    state: Vec<f64>,
}

impl StateOutputPrimitive for Delay {
    fn describe() -> ModuleDescriptor {
        descriptor(
            "vec.delay",
            Vec::new(),
            Vec::new(),
            Feedthrough::StateOutput,
        )
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

/// Outputs its `arm` parameter; panics in a call any of whose ticks sees
/// it at 1 or more.
struct Fail;

impl DirectPrimitive for Fail {
    fn describe() -> ModuleDescriptor {
        descriptor("fail.d64", Vec::new(), Vec::new(), Feedthrough::Direct)
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {}
    fn reset(&mut self) {}
    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let arm = inputs.parameter(0);
        assert!(arm.iter().all(|&a| a < 1.0), "armed");
        outputs.signal(0).copy_from_slice(arm);
    }
}

/// The vector type `kind` (`spread`, `gain`, `mix`, `delay`) of `n`
/// elements at divisor `rate`.
pub fn vector_type(kind: &str, n: u32, rate: u32) -> String {
    format!("vec.{kind}.n{n}.d{rate}")
}

/// The standard type `type_name` at divisor `rate` (itself at 1).
pub fn divided_type(type_name: &str, rate: u32) -> String {
    if rate == 1 {
        type_name.to_owned()
    } else {
        format!("d{rate}.{type_name}")
    }
}

/// Registers the fixtures in `registry`, which holds the standard types.
pub fn register(registry: &mut Registry) {
    let standard: Vec<ModuleType> = registry
        .iter()
        .filter(|t| {
            let d = t.descriptor;
            d.type_name.starts_with("std.")
                && d.sink.is_none()
                && d.latency.is_none()
                && d.ports.iter().all(|p| p.shape != PortShape::Events)
                // M2.8-7: the visual-rate and vector types have their own
                // rates and shapes; the fixtures re-rate scalar types only.
                && d.rate() == Some(RateDivisor::ONE)
                && d.ports.iter().all(|p| !matches!(p.shape, PortShape::Vector(_)))
        })
        .copied()
        .collect();
    for &rate in &DIVISORS[1..] {
        for t in &standard {
            let mut descriptor = t.descriptor;
            descriptor.type_name = name(divided_type(t.descriptor.type_name, rate));
            descriptor.ports = leak(
                descriptor
                    .ports
                    .iter()
                    .map(|p| PortDescriptor {
                        rate: d(rate),
                        ..*p
                    })
                    .collect(),
            );
            descriptor.parameters = leak(
                descriptor
                    .parameters
                    .iter()
                    .map(|p| ParameterDescriptor {
                        rate: d(rate),
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
    let direct = |create: fn() -> Primitive| create;
    for &rate in &DIVISORS {
        for &n in &SIZES {
            let v = PortShape::Vector(n);
            let types: [VectorKind; 4] = [
                (
                    "spread",
                    vec![
                        port("in", Direction::In, PortShape::Signal, rate),
                        port("out", Direction::Out, v, rate),
                    ],
                    true,
                    direct(|| Primitive::Direct(Box::new(Spread))),
                ),
                (
                    "gain",
                    vec![
                        port("in", Direction::In, v, rate),
                        port("out", Direction::Out, v, rate),
                    ],
                    true,
                    direct(|| Primitive::Direct(Box::new(Gain))),
                ),
                (
                    "mix",
                    vec![
                        port("in", Direction::In, v, rate),
                        port("out", Direction::Out, PortShape::Signal, rate),
                    ],
                    true,
                    direct(|| Primitive::Direct(Box::new(Mix))),
                ),
                (
                    "delay",
                    vec![
                        port("in", Direction::In, v, rate),
                        port("out", Direction::Out, v, rate),
                    ],
                    false,
                    direct(|| {
                        Primitive::StateOutput(Box::new(Delay {
                            state: vec![0.0; MOST],
                        }))
                    }),
                ),
            ];
            for (kind, ports, has_gain, create) in types {
                let feedthrough = if kind == "delay" {
                    Feedthrough::StateOutput
                } else {
                    Feedthrough::Direct
                };
                let parameters = if has_gain {
                    vec![gain(rate)]
                } else {
                    Vec::new()
                };
                let type_name = name(vector_type(kind, n, rate));
                let descriptor = descriptor(type_name, ports, parameters, feedthrough);
                registry
                    .register(ModuleType { descriptor, create })
                    .unwrap();
            }
        }
    }
    let arm = ParameterDescriptor {
        name: "arm",
        range: Range { min: 0.0, max: 2.0 },
        default: 0.0,
        combine: Combine::Replace,
        ..gain(64)
    };
    registry
        .register(ModuleType {
            descriptor: descriptor(
                "fail.d64",
                vec![port("out", Direction::Out, PortShape::Signal, 64)],
                vec![arm],
                Feedthrough::Direct,
            ),
            create: || Primitive::Direct(Box::new(Fail)),
        })
        .unwrap();
}

/// The standard types and the probe, with the fixtures.
pub fn registry() -> Registry {
    let mut registry = super::registry();
    register(&mut registry);
    registry
}
