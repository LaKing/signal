//! Signal standard library: the M1 primitives (oscillators, envelope, math,
//! shaper, filter, delay, LFO, noise, transport, audio out) and the M2 core
//! primitives (band-limited oscillator, TPT filters, ADSR, lag, fractional
//! delay, mixing, math and event utilities). Every primitive
//! passes the contract suite (`docs/module-contract.md`, `docs/realtime-rules.md`,
//! ADR 0002, 0016) and follows `docs/dsp-conventions.md`: f64 throughout, `libm`
//! for every transcendental function (ADR 0007), everything rate-dependent
//! derived from the sample rate in `prepare`.
//!
//! [`register`] adds every type to a [`Registry`] under its `std.*` name:
//!
//! | Type | Primitive | Class |
//! |---|---|---|
//! | `std.sine_osc` | [`SineOsc`] | direct |
//! | `std.exp_env` | [`ExpEnv`] | direct |
//! | `std.mul` | [`Mul`] | direct |
//! | `std.add` | [`Add`] | direct |
//! | `std.tanh_shaper` | [`TanhShaper`] | direct |
//! | `std.dc_block` | [`DcBlock`] | direct |
//! | `std.unit_delay` | [`UnitDelay`] | state-output |
//! | `std.lfo` | [`Lfo`] | direct |
//! | `std.noise` | [`Noise`] | direct |
//! | `std.transport` | [`Transport`] | direct |
//! | `std.audio_out` | [`AudioOut`] | direct, audio sink |
//! | `std.osc` | [`Osc`] | direct |
//! | `std.svf` | [`Svf`] | direct |
//! | `std.onepole` | [`OnePole`] | direct |
//! | `std.adsr` | [`Adsr`] | direct |
//! | `std.lag` | [`Lag`] | direct |
//! | `std.delay` | [`Delay`] | state-output |
//! | `std.mix` | [`Mix`] | direct |
//! | `std.pan` | [`Pan`] | direct |
//! | `std.gain` | [`Gain`] | direct |
//! | `std.sub` | [`Sub`] | direct |
//! | `std.min` | [`Min`] | direct |
//! | `std.max` | [`Max`] | direct |
//! | `std.abs` | [`Abs`] | direct |
//! | `std.scale` | [`Scale`] | direct |
//! | `std.clamp` | [`Clamp`] | direct |
//! | `std.sample_hold` | [`SampleHold`] | direct |
//! | `std.edge` | [`Edge`] | direct |
//! | `std.clock_div` | [`ClockDiv`] | direct |
//! | `std.step_seq` | [`StepSeq`] | direct |
//!
//! Musical time (tempo, beats) lives here, in [`Transport`], never in the core
//! (ADR 0003).

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

mod add;
mod adsr;
mod audio_out;
mod clock_div;
mod dc_block;
mod delay;
mod edge;
mod exp_env;
mod gain;
mod lag;
mod lfo;
mod math;
mod mix;
mod mul;
mod noise;
mod onepole;
mod osc;
mod pan;
mod sample_hold;
mod sine_osc;
mod step_seq;
mod svf;
mod tanh_shaper;
mod transport;
mod unit_delay;

#[cfg(test)]
mod testing;

pub use add::Add;
pub use adsr::Adsr;
pub use audio_out::AudioOut;
pub use clock_div::ClockDiv;
pub use dc_block::DcBlock;
pub use delay::Delay;
pub use edge::Edge;
pub use exp_env::ExpEnv;
pub use gain::Gain;
pub use lag::Lag;
pub use lfo::Lfo;
pub use math::{Abs, Clamp, Max, Min, Scale, Sub};
pub use mix::Mix;
pub use mul::Mul;
pub use noise::Noise;
pub use onepole::OnePole;
pub use osc::Osc;
pub use pan::Pan;
pub use sample_hold::SampleHold;
pub use sine_osc::SineOsc;
pub use step_seq::StepSeq;
pub use svf::Svf;
pub use tanh_shaper::TanhShaper;
pub use transport::Transport;
pub use unit_delay::UnitDelay;

use signal_core::{
    Combine, DirectPrimitive, Direction, Event, EventPayload, ModuleType, Outputs,
    ParameterDescriptor, PortDescriptor, PortKind, PortShape, Primitive, Range, RateDivisor,
    Registry, RegistryError, StateOutputPrimitive,
};

/// Each `std.*` type's static cost in ns per frame per instance: the seat's
/// per-primitive medians of `docs/benchmarks.md` (M2-5, one frame at a time),
/// rounded up. The engine groups cheap units into tasks by them (ADR 0019
/// §2); they never change the output.
pub const COST_NS: [(&str, u32); 30] = [
    ("std.abs", 11),
    ("std.add", 11),
    ("std.adsr", 44),
    ("std.audio_out", 12),
    ("std.clamp", 18),
    ("std.clock_div", 21),
    ("std.dc_block", 18),
    ("std.delay", 23),
    ("std.edge", 19),
    ("std.exp_env", 34),
    ("std.gain", 15),
    ("std.lag", 25),
    ("std.lfo", 20),
    ("std.max", 12),
    ("std.min", 12),
    ("std.mix", 32),
    ("std.mul", 12),
    ("std.noise", 9),
    ("std.onepole", 17),
    ("std.osc", 25),
    ("std.pan", 19),
    ("std.sample_hold", 15),
    ("std.scale", 19),
    ("std.sine_osc", 19),
    ("std.step_seq", 57),
    ("std.sub", 12),
    ("std.svf", 22),
    ("std.tanh_shaper", 30),
    ("std.transport", 29),
    ("std.unit_delay", 12),
];

/// Registers every `std.*` primitive type in `registry`, with its static
/// cost ([`COST_NS`]).
///
/// Fails with [`RegistryError::DuplicateTypeName`] if one of the names is
/// already registered (for example when called twice on the same registry);
/// the types before it in the table above stay registered.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    for module_type in [
        direct::<SineOsc>(),
        direct::<ExpEnv>(),
        direct::<Mul>(),
        direct::<Add>(),
        direct::<TanhShaper>(),
        direct::<DcBlock>(),
        state_output::<UnitDelay>(),
        direct::<Lfo>(),
        direct::<Noise>(),
        direct::<Transport>(),
        direct::<AudioOut>(),
        direct::<Osc>(),
        direct::<Svf>(),
        direct::<OnePole>(),
        direct::<Adsr>(),
        direct::<Lag>(),
        state_output::<Delay>(),
        direct::<Mix>(),
        direct::<Pan>(),
        direct::<Gain>(),
        direct::<Sub>(),
        direct::<Min>(),
        direct::<Max>(),
        direct::<Abs>(),
        direct::<Scale>(),
        direct::<Clamp>(),
        direct::<SampleHold>(),
        direct::<Edge>(),
        direct::<ClockDiv>(),
        direct::<StepSeq>(),
    ] {
        registry.register(module_type)?;
    }
    for (name, ns) in COST_NS {
        registry.set_cost(name, ns);
    }
    Ok(())
}

fn direct<T: DirectPrimitive + Default + 'static>() -> ModuleType {
    ModuleType {
        descriptor: T::describe(),
        create: || Primitive::Direct(Box::new(T::default())),
    }
}

fn state_output<T: StateOutputPrimitive + Default + 'static>() -> ModuleType {
    ModuleType {
        descriptor: T::describe(),
        create: || Primitive::StateOutput(Box::new(T::default())),
    }
}

/// Decaying state whose magnitude falls below this is set to exactly 0
/// (`docs/dsp-conventions.md`, "Denormals").
pub(crate) const FLUSH_THRESHOLD: f64 = 1e-9;

/// `round_half_up(x) = ⌊x + ½⌋`, the one rounding rule for converting time to
/// frames (`docs/dsp-conventions.md`).
pub(crate) fn round_half_up(x: f64) -> f64 {
    libm::floor(x + 0.5)
}

/// The fractional part `x − ⌊x⌋`, in `[0, 1)` for finite `x`.
pub(crate) fn fract(x: f64) -> f64 {
    x - libm::floor(x)
}

/// `sin(2π·cycles)`: the output of a phase given in cycles.
pub(crate) fn sine_of_cycles(cycles: f64) -> f64 {
    libm::sin(std::f64::consts::TAU * cycles)
}

/// The value of `values` at `index`, or `fallback` if there is none.
pub(crate) fn at(values: &[f64], index: usize, fallback: f64) -> f64 {
    values.get(index).copied().unwrap_or(fallback)
}

/// Whether `events` (sorted by offset) hold a `Trigger` on frame `n`. `next`
/// is the index of the first event not yet consumed; call this for
/// increasing `n`, starting with `next = 0`. `Set` and `Ramp` events are
/// skipped, and several triggers on one frame count once.
pub(crate) fn triggered(events: &[Event], next: &mut usize, n: usize) -> bool {
    let mut triggered = false;
    while let Some(event) = events.get(*next) {
        if event.offset as usize > n {
            break;
        }
        triggered |= event.offset as usize == n && event.payload == EventPayload::Trigger;
        *next += 1;
    }
    triggered
}

/// Pushes one `Trigger` on frame `n` of event output `port`.
pub(crate) fn push_trigger(outputs: &mut Outputs<'_>, port: usize, n: usize) {
    if let Some(sink) = outputs.events(port) {
        sink.push(Event {
            offset: u32::try_from(n).unwrap_or(u32::MAX),
            payload: EventPayload::Trigger,
        });
    }
}

/// A master-rate signal port.
pub(crate) const fn signal_port(
    name: &'static str,
    direction: Direction,
    kind: PortKind,
    unit: Option<&'static str>,
    range: Option<Range>,
) -> PortDescriptor {
    PortDescriptor {
        name,
        direction,
        shape: PortShape::Signal,
        rate: RateDivisor::ONE,
        kind,
        unit,
        range,
    }
}

/// A master-rate event port carrying triggers.
pub(crate) const fn event_port(name: &'static str, direction: Direction) -> PortDescriptor {
    PortDescriptor {
        name,
        direction,
        shape: PortShape::Events,
        rate: RateDivisor::ONE,
        kind: PortKind::Trigger,
        unit: None,
        range: None,
    }
}

/// A master-rate parameter.
pub(crate) const fn parameter(
    name: &'static str,
    kind: PortKind,
    unit: Option<&'static str>,
    min: f64,
    max: f64,
    default: f64,
    combine: Combine,
) -> ParameterDescriptor {
    ParameterDescriptor {
        name,
        kind,
        rate: RateDivisor::ONE,
        unit,
        range: Range { min, max },
        default,
        combine,
    }
}

/// The range `[−1, 1]` of a bipolar unit signal.
pub(crate) const BIPOLAR: Option<Range> = Some(Range {
    min: -1.0,
    max: 1.0,
});

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: [&str; 30] = [
        "std.sine_osc",
        "std.exp_env",
        "std.mul",
        "std.add",
        "std.tanh_shaper",
        "std.dc_block",
        "std.unit_delay",
        "std.lfo",
        "std.noise",
        "std.transport",
        "std.audio_out",
        "std.osc",
        "std.svf",
        "std.onepole",
        "std.adsr",
        "std.lag",
        "std.delay",
        "std.mix",
        "std.pan",
        "std.gain",
        "std.sub",
        "std.min",
        "std.max",
        "std.abs",
        "std.scale",
        "std.clamp",
        "std.sample_hold",
        "std.edge",
        "std.clock_div",
        "std.step_seq",
    ];

    #[test]
    fn register_adds_every_type_under_its_name() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), NAMES.len());
        for name in NAMES {
            let module_type = registry.get(name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(module_type.descriptor.type_name, name);
            assert_eq!(
                (module_type.create)().feedthrough(),
                module_type.descriptor.feedthrough,
                "{name}: `create` matches the declared class"
            );
        }
    }

    #[test]
    fn registering_twice_is_refused_as_a_duplicate_name() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(
            register(&mut registry),
            Err(RegistryError::DuplicateTypeName("std.sine_osc"))
        );
        assert_eq!(registry.len(), NAMES.len());

        // A type registered first under a std name blocks that name.
        let mut registry = Registry::new();
        registry.register(direct::<Mul>()).unwrap();
        assert_eq!(
            register(&mut registry),
            Err(RegistryError::DuplicateTypeName("std.mul"))
        );
    }

    #[test]
    fn port_and_parameter_names_are_unique_per_type() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        for module_type in registry.iter() {
            let d = module_type.descriptor;
            let mut names: Vec<&str> = d.ports.iter().map(|p| p.name).collect();
            names.extend(d.parameters.iter().map(|p| p.name));
            let count = names.len();
            names.sort_unstable();
            names.dedup();
            assert_eq!(names.len(), count, "{}", d.type_name);
            for p in d.parameters {
                assert!(
                    p.range.min <= p.default && p.default <= p.range.max,
                    "{}.{}",
                    d.type_name,
                    p.name
                );
            }
        }
    }

    #[test]
    fn round_half_up_rounds_ties_up() {
        assert_eq!(round_half_up(0.5), 1.0);
        assert_eq!(round_half_up(1.5), 2.0);
        assert_eq!(round_half_up(2.4999), 2.0);
        assert_eq!(round_half_up(-0.5), 0.0);
        assert_eq!(round_half_up(-2.5), -2.0);
        assert_eq!(round_half_up(-2.6), -3.0);
    }
}
