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
//! | Type | Primitive | Class | Category |
//! |---|---|---|---|
//! | `std.sine_osc` | [`SineOsc`] | direct | `generator` |
//! | `std.exp_env` | [`ExpEnv`] | direct | `envelope` |
//! | `std.mul` | [`Mul`] | direct | `core` |
//! | `std.add` | [`Add`] | direct | `core` |
//! | `std.tanh_shaper` | [`TanhShaper`] | direct | `filter` |
//! | `std.dc_block` | [`DcBlock`] | direct | `filter` |
//! | `std.unit_delay` | [`UnitDelay`] | state-output | `core` |
//! | `std.lfo` | [`Lfo`] | direct | `generator` |
//! | `std.noise` | [`Noise`] | direct | `generator` |
//! | `std.transport` | [`Transport`] | direct | `event` |
//! | `std.audio_out` | [`AudioOut`] | direct, audio sink | `io` |
//! | `std.osc` | [`Osc`] | direct | `generator` |
//! | `std.svf` | [`Svf`] | direct | `filter` |
//! | `std.onepole` | [`OnePole`] | direct | `filter` |
//! | `std.adsr` | [`Adsr`] | direct | `envelope` |
//! | `std.lag` | [`Lag`] | direct | `filter` |
//! | `std.delay` | [`Delay`] | state-output | `delay` |
//! | `std.mix` | [`Mix`] | direct | `core` |
//! | `std.pan` | [`Pan`] | direct | `core` |
//! | `std.gain` | [`Gain`] | direct | `core` |
//! | `std.sub` | [`Sub`] | direct | `core` |
//! | `std.min` | [`Min`] | direct | `core` |
//! | `std.max` | [`Max`] | direct | `core` |
//! | `std.abs` | [`Abs`] | direct | `core` |
//! | `std.scale` | [`Scale`] | direct | `core` |
//! | `std.clamp` | [`Clamp`] | direct | `core` |
//! | `std.sample_hold` | [`SampleHold`] | direct | `event` |
//! | `std.edge` | [`Edge`] | direct | `event` |
//! | `std.clock_div` | [`ClockDiv`] | direct | `event` |
//! | `std.step_seq` | [`StepSeq`] | direct | `event` |
//! | `std.string` | [`StringModel`] | direct | `physical` |
//! | `std.modal` | [`Modal`] | direct | `physical` |
//! | `std.mass_spring` | [`MassSpring`] | direct | `physical` |
//! | `std.exciter` | [`Exciter`] | direct | `physical` |
//! | `std.count` | [`Count`] | direct (the event bridge) | `event` |
//! | `std.particles` | [`Particles`] | direct, divisor 800 | `visual` |
//! | `std.mesh2d` | [`Mesh2d`]`<false>` | direct, divisor 800 | `visual` |
//! | `std.mesh2d_audio` | [`Mesh2d`]`<true>` | direct | `physical` |
//! | `std.body2d` | [`Body2d`] | direct, divisor 800 | `visual` |
//! | `std.div` | [`Div`] | direct | `core` |
//! | `std.neg` | [`Neg`] | direct | `core` |
//! | `std.mod` | [`Mod`] | direct | `core` |
//! | `std.fract` | [`Fract`] | direct | `core` |
//! | `std.floor` | [`Floor`] | direct | `core` |
//! | `std.round` | [`Round`] | direct | `core` |
//! | `std.sign` | [`Sign`] | direct | `core` |
//! | `std.sqrt` | [`Sqrt`] | direct | `core` |
//! | `std.exp` | [`Exp`] | direct | `core` |
//! | `std.log` | [`Log`] | direct | `core` |
//! | `std.pow` | [`Pow`] | direct | `core` |
//! | `std.sin` | [`Sin`] | direct | `core` |
//! | `std.cos` | [`Cos`] | direct | `core` |
//! | `std.tan` | [`Tan`] | direct | `core` |
//! | `std.tanh` | [`Tanh`] | direct | `core` |
//! | `std.atan2` | [`Atan2`] | direct | `core` |
//! | `std.sin_cycles` | [`SinCycles`] | direct | `core` |
//! | `std.const` | [`Const`] | direct | `core` |
//! | `std.sample_rate` | [`SampleRate`] | direct | `core` |
//! | `std.frames` | [`Frames`] | direct | `core` |
//! | `std.compare` | [`Compare`] | direct | `core` |
//! | `std.select` | [`Select`] | direct | `core` |
//! | `std.and` | [`And`] | direct | `core` |
//! | `std.or` | [`Or`] | direct | `core` |
//! | `std.not` | [`Not`] | direct | `core` |
//! | `std.phasor` | [`Phasor`] | direct | `core` |
//! | `std.counter` | [`Counter`] | direct | `core` |
//! | `std.integrator` | [`Integrator`] | direct | `core` |
//! | `std.buffer1k`, `std.buffer64k`, `std.buffer1m` | [`Buffer`]`<C>` | direct | `core` |
//! | `std.latch` | [`Latch`] | direct | `core` |
//! | `std.to_events` | [`ToEvents`] | direct | `core` |
//! | `std.route` | [`Route`] | direct | `core` |
//! | `std.random` | [`Random`] | direct | `core` |
//! | `std.note_seq` | [`NoteSeq`] | direct, takes a `notes` pattern | `event` |
//! | `std.step_seq8` | [`StepSeq8`] | direct, takes a `steps` pattern | `event` |
//! | `std.ladder` | [`Ladder`] | direct | `filter` |
//! | `std.pack<N>` | [`Pack`]`<N>` | direct | `core` |
//! | `std.unpack<N>` | [`Unpack`]`<N>` | direct | `core` |
//! | `std.index<N>` | [`Index`]`<N>` | direct | `core` |
//! | `std.reduce<N>` | [`Reduce`]`<N>` | direct | `core` |
//! | `std.vadd<N>` | [`Vadd`]`<N>` | direct | `core` |
//! | `std.vmul<N>` | [`Vmul`]`<N>` | direct | `core` |
//! | `std.table<N>` | [`Table`]`<N>` | direct | `core` |
//! | `std.biquad` | [`Biquad`] | direct | `filter` |
//! | `std.comb` | [`Comb`] | direct | `delay` |
//! | `std.allpass_delay` | [`AllpassDelay`] | direct | `delay` |
//! | `std.fdn_reverb` | [`FdnReverb`] | direct | `delay` |
//! | `std.fm_op` | [`FmOp`] | direct | `generator` |
//! | `std.wavetable` | [`Wavetable`] | direct | `generator` |
//!
//! The seven `<N>` types are one type per width N of [`VECTOR_WIDTHS`] (2, 4, 8, 16,
//! 64, 256, 1024), named with the width as a suffix: `std.pack4`,
//! `std.reduce1024`, `std.table16`.
//!
//! Every type carries its palette group ([`category`]; ADR 0027 §1), which
//! `signal modules --category` filters by. The M2.10 core arithmetic,
//! functions, constants and rate, and comparison and selection (from
//! `std.div` on) are total: finite inputs give finite outputs, each edge case
//! documented with the type's formula (`docs/module-contract.md`, "The
//! standard library"). Their results are clamped to ±1e6 ([`SIGNAL_BOUND`],
//! the engine's magnitude bound, `docs/semantics.md` §7), so a core
//! primitive never faults on its own. Core set II (M2.10-2, from
//! `std.phasor` on: state, memory, value events, randomness and vectors)
//! keeps every output and every emitted value within the same bound.
//!
//! The M2.8 physical models (`std.string`, `std.modal`, `std.mass_spring`,
//! `std.exciter`) document their equations and are checked against analytic
//! results in their unit tests. Their decay times are T60s in seconds
//! (`decay_s`).
//!
//! The M2.8-7 visual-rate physics (`std.particles`, `std.mesh2d`,
//! `std.body2d`) run at [`VISUAL_DIVISOR`] and output vectors for
//! `<signal-canvas>`. A type above divisor 1 cannot have event ports
//! (`docs/semantics.md` §16), so their events come through [`Count`]
//! (operator decision operator-request:007, a deviation from the M2.8-7
//! wording "emit and impulse events": the visual types take `std.count`
//! readings ([`BRIDGE_ROWS`] rows) instead of event ports;
//! `docs/module-contract.md` "Events into divided-rate types"). `std.mesh2d_audio` is the same membrane at divisor 1,
//! with an event input and an audio pickup.
//!
//! The M2.10 native DSP primitives (ADR 0027 §3: filters with modulated
//! coefficients, band-limited and FM oscillators, reverb cores) are
//! `std.biquad`, `std.comb`, `std.allpass_delay`, `std.fdn_reverb`,
//! `std.fm_op` and `std.wavetable`. Each rustdoc gives the formula, its
//! source, and why it stays bounded at every setting.
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
mod arith;
mod audio_out;
mod biquad;
mod body2d;
mod bridge;
pub mod category;
mod clock_div;
mod comb;
mod constant;
mod count;
mod dc_block;
mod delay;
mod edge;
mod events;
mod exciter;
mod exp_env;
mod fdn;
mod fm_op;
mod function;
mod gain;
mod ladder;
mod lag;
mod lfo;
mod line;
mod logic;
mod mass_spring;
mod math;
mod memory;
mod mesh2d;
mod mix;
mod modal;
mod mul;
mod noise;
mod note_seq;
mod onepole;
mod osc;
mod pan;
mod particles;
pub mod pattern;
mod random;
mod sample_hold;
mod sine_osc;
mod state;
mod step_seq;
mod step_seq8;
mod string;
mod svf;
mod tanh_shaper;
mod transport;
mod unit_delay;
mod vector;
mod wavetable;

#[cfg(test)]
mod testing;

pub use add::Add;
pub use adsr::Adsr;
pub use arith::{Div, Floor, Fract, Mod, Neg, Round, Sign};
pub use audio_out::AudioOut;
pub use biquad::Biquad;
pub use body2d::Body2d;
pub use bridge::BRIDGE_ROWS;
pub use clock_div::ClockDiv;
pub use comb::{AllpassDelay, Comb};
pub use constant::{Const, Frames, SampleRate};
pub use count::Count;
pub use dc_block::DcBlock;
pub use delay::Delay;
pub use edge::Edge;
pub use events::{Latch, Route, ToEvents};
pub use exciter::Exciter;
pub use exp_env::ExpEnv;
pub use fdn::FdnReverb;
pub use fm_op::FmOp;
pub use function::{Atan2, Cos, Exp, Log, Pow, Sin, SinCycles, Sqrt, Tan, Tanh};
pub use gain::Gain;
pub use ladder::Ladder;
pub use lag::Lag;
pub use lfo::Lfo;
pub use logic::{And, Compare, Not, Or, Select};
pub use mass_spring::MassSpring;
pub use math::{Abs, Clamp, Max, Min, Scale, Sub};
pub use memory::{BUFFER_CAPACITIES, Buffer, Buffer1k, Buffer1m, Buffer64k, Table};
pub use mesh2d::Mesh2d;
pub use mix::Mix;
pub use modal::Modal;
pub use mul::Mul;
pub use noise::Noise;
pub use note_seq::NoteSeq;
pub use onepole::OnePole;
pub use osc::Osc;
pub use pan::Pan;
pub use particles::Particles;
pub use random::Random;
pub use sample_hold::SampleHold;
pub use sine_osc::SineOsc;
pub use state::{Counter, Integrator, Phasor};
pub use step_seq::StepSeq;
pub use step_seq8::StepSeq8;
pub use string::StringModel;
pub use svf::Svf;
pub use tanh_shaper::TanhShaper;
pub use transport::Transport;
pub use unit_delay::UnitDelay;
pub use vector::{Index, Pack, Reduce, Unpack, VECTOR_WIDTHS, Vadd, Vmul};
pub use wavetable::Wavetable;

use signal_core::{
    Combine, DirectPrimitive, Direction, Event, EventPayload, ModuleType, Outputs,
    ParameterDescriptor, PortDescriptor, PortKind, PortShape, Primitive, Range, RateDivisor,
    Registry, RegistryError, StateOutputPrimitive,
};

/// Each `std.*` type's static cost in ns per frame per instance: the seat's
/// per-primitive medians of `docs/benchmarks.md` (M2-5, one frame at a time),
/// rounded up. The engine groups cheap units into tasks by them (ADR 0019
/// §2) and takes a plan's worker budget from them; they never change the
/// output. The second group (M2.10-6) is every type without a figure before
/// whose median, less the run's host offset, was at least twice the engine's
/// default of 20 ns (`docs/benchmarks.md` "Idle cost, part 2 (M2.10-6)").
/// The third group is the M2.10 native DSP primitives, each measured when it
/// was added.
/// The visual-rate types (`std.mesh2d`, `std.particles`, `std.body2d`) are
/// left out: the estimate does not divide by a rate divisor, so a per-tick
/// figure would overstate them up to 800 times. Every other type takes the
/// engine's default.
pub const COST_NS: [(&str, u32); 76] = [
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
    // M2.10-6: `signal --json bench`, 2026-10-18 23:32 UTC, host offset 12.2 ns.
    ("std.buffer1k", 55),
    ("std.buffer1m", 60),
    ("std.buffer64k", 56),
    ("std.count", 71),
    ("std.index1024", 1845),
    ("std.index256", 486),
    ("std.index64", 136),
    ("std.ladder", 91),
    ("std.mass_spring", 44),
    ("std.mesh2d_audio", 674),
    ("std.pack1024", 3488),
    ("std.pack16", 62),
    ("std.pack256", 904),
    ("std.pack64", 224),
    ("std.reduce1024", 2746),
    ("std.reduce16", 48),
    ("std.reduce256", 707),
    ("std.reduce64", 177),
    ("std.step_seq8", 49),
    ("std.table1024", 3109),
    ("std.table16", 80),
    ("std.table256", 753),
    ("std.table64", 216),
    ("std.table8", 54),
    ("std.unpack1024", 3714),
    ("std.unpack16", 68),
    ("std.unpack256", 985),
    ("std.unpack64", 255),
    ("std.vadd1024", 5344),
    ("std.vadd16", 93),
    ("std.vadd256", 1347),
    ("std.vadd64", 354),
    ("std.vadd8", 49),
    ("std.vmul1024", 5482),
    ("std.vmul16", 91),
    ("std.vmul256", 1409),
    ("std.vmul64", 359),
    ("std.vmul8", 51),
    // M2.10, the native DSP primitives: `signal --json bench --seconds 0.5`,
    // release, 2026-10-19 22:16 UTC (after the review fixes), load average
    // 9–18 (shared host), host offset 13.1 ns (the median difference over the
    // first 30 entries; `std.svf` read 22.3 against its 22, `std.ladder` 94.7
    // against 91). Each is the median less that offset, rounded up.
    ("std.allpass_delay", 31),
    ("std.biquad", 29),
    ("std.comb", 32),
    ("std.fdn_reverb", 72),
    ("std.fm_op", 42),
    ("std.wavetable", 32),
    // M2.10-8 (indicative, pending the operator's device check):
    // `signal --json bench --seconds 0.5`, release, 2026-10-20 01:17 CEST,
    // devpanel sandbox, host offset 11.1 ns. The bench feeds every vector
    // input with its saw, so `std.modal` is measured with all 192 per-mode
    // rows changing on every frame (every mode recomputed every frame); with
    // the rows unconnected it costs about a tenth of that
    // (`docs/benchmarks.md`, "The modal bank and exciter"). The figure is
    // per type, not per setting: the worst case, 64 modes with every
    // parameter modulated, measured about 4.6–5.1 µs per frame at the
    // primitive level (16 modes about 1.4–1.6 µs; static 0.27 and 0.12 µs),
    // so a bank of 64 fully modulated modes costs about five times this
    // estimate (review of M2.10-8; the same section).
    ("std.exciter", 47),
    ("std.modal", 956),
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
        direct::<StringModel>(),
        direct::<Modal>(),
        direct::<MassSpring>(),
        direct::<Exciter>(),
        direct::<Count>(),
        direct::<Particles>(),
        direct::<Mesh2d<false>>(),
        direct::<Mesh2d<true>>(),
        direct::<Body2d>(),
        direct::<Div>(),
        direct::<Neg>(),
        direct::<Mod>(),
        direct::<Fract>(),
        direct::<Floor>(),
        direct::<Round>(),
        direct::<Sign>(),
        direct::<Sqrt>(),
        direct::<Exp>(),
        direct::<Log>(),
        direct::<Pow>(),
        direct::<Sin>(),
        direct::<Cos>(),
        direct::<Tan>(),
        direct::<Tanh>(),
        direct::<Atan2>(),
        direct::<SinCycles>(),
        direct::<Const>(),
        direct::<SampleRate>(),
        direct::<Frames>(),
        direct::<Compare>(),
        direct::<Select>(),
        direct::<And>(),
        direct::<Or>(),
        direct::<Not>(),
        direct::<Phasor>(),
        direct::<Counter>(),
        direct::<Integrator>(),
        direct::<Buffer1k>(),
        direct::<Buffer64k>(),
        direct::<Buffer1m>(),
        direct::<Latch>(),
        direct::<ToEvents>(),
        direct::<Route>(),
        direct::<Random>(),
        direct::<NoteSeq>(),
        direct::<StepSeq8>(),
        direct::<Ladder>(),
        direct::<Biquad>(),
        direct::<Comb>(),
        direct::<AllpassDelay>(),
        direct::<FdnReverb>(),
        direct::<FmOp>(),
        direct::<Wavetable>(),
    ] {
        registry.register(module_type)?;
    }
    // ADR 0028 §3, §4: the sequencers take a pattern, compiled into their
    // table when the plan is built.
    registry.set_table_rule("std.note_seq", pattern::NOTE_RULE);
    registry.set_table_rule("std.step_seq8", pattern::STEP_RULE);
    for module_type in vector::types() {
        registry.register(*module_type)?;
    }
    for (name, ns) in COST_NS {
        registry.set_cost(name, ns);
    }
    for (name, input) in BRIDGE_INPUTS {
        registry.restrict_input(name, input, "std.count", "window");
    }
    Ok(())
}

/// The bridge inputs (`crate::bridge`): each takes one `std.count.window`
/// at scale 1, and the plan refuses anything else there (ADR 0024).
const BRIDGE_INPUTS: [(&str, &str); 4] = [
    ("std.particles", "emit"),
    ("std.particles", "impulse"),
    ("std.mesh2d", "strike"),
    ("std.body2d", "kick"),
];

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

/// The engine's magnitude bound, 10⁶ (`docs/semantics.md` §7): a signal output
/// beyond it in magnitude is a `magnitude` fault, which mutes the instance
/// for the run. The core types (ADR 0027 §1) clamp every result that could
/// grow beyond it to `±SIGNAL_BOUND`, so a core primitive never faults on its
/// own; `std.const`'s range and the `scale` and `clamp` parameters are
/// bounded by it too.
pub(crate) const SIGNAL_BOUND: f64 = 1e6;

/// Decaying state whose magnitude falls below this is set to exactly 0
/// (`docs/dsp-conventions.md`, "Denormals").
pub(crate) const FLUSH_THRESHOLD: f64 = 1e-9;

/// `round_half_up(x) = ⌊x + ½⌋`, the one rounding rule for converting time to
/// frames (`docs/dsp-conventions.md`).
pub(crate) fn round_half_up(x: f64) -> f64 {
    libm::floor(x + 0.5)
}

/// The fractional part `x − ⌊x⌋` for finite `x`: in `[0, 1)`, except that the
/// subtraction rounds to exactly 1 for a tiny negative `x` (e.g. `−1e−20`).
/// The oscillators use it as is; `std.fract` (`arith::fraction`) maps that 1
/// to the largest double below 1.
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

/// The default ticks per beat of `std.transport`'s `tick` and of the
/// sequencers that count them (ADR 0028 §5).
pub(crate) const DEFAULT_PPQ: f64 = 96.0;
/// The most ticks per beat.
pub(crate) const MAX_PPQ: f64 = 960.0;

/// The `ppq` parameter (ticks per beat, integer, `[1, 960]`, default 96) of
/// `std.transport` and of the sequencers, which must equal the transport's.
pub(crate) const PPQ: ParameterDescriptor = parameter(
    "ppq",
    PortKind::Generic,
    Some("ticks/beat"),
    1.0,
    MAX_PPQ,
    DEFAULT_PPQ,
    Combine::Add,
);

/// `ppq` read at `index`: rounded half up into `[1, 960]` (NaN reads as 1).
pub(crate) fn ppq_at(values: &[f64], index: usize) -> u64 {
    (round_half_up(at(values, index, DEFAULT_PPQ)).clamp(1.0, MAX_PPQ) as u64).max(1)
}

/// The most ticks one `Set` on a sequencer's `tick` counts for: the
/// transport's largest `Set(n − 1)`. A frame holds at most ⌊R⌋ + 1 ticks,
/// R = bpm·ppq / (60·sr) ≤ 400·960 / 60 = 6400 at the lowest rate (1 Hz),
/// so the transport's count is always exact, and a `Set` from any other
/// source costs at most this many steps of a sequencer's work (16 events a
/// frame at most): bounded work per call (`docs/realtime-rules.md`).
pub(crate) const MAX_TICKS_PER_EVENT: u64 = 6400;

/// The ticks one event of a `tick` stream stands for: `std.transport`'s
/// `tick` contract (ADR 0028 §5) is that each frame holding *n* ≥ 1 ticks
/// carries a `Trigger` and, when *n* > 1, a `Set(n − 1)`, so a counter adds
/// 1 for a `Trigger` and *v* (rounded half up, at least 0, at most
/// [`MAX_TICKS_PER_EVENT`]) for a `Set(v)`; a `Ramp` counts nothing.
pub(crate) fn tick_count(payload: EventPayload) -> u64 {
    match payload {
        EventPayload::Trigger => 1,
        EventPayload::Set(v) if v.is_finite() => {
            round_half_up(v).clamp(0.0, MAX_TICKS_PER_EVENT as f64) as u64
        }
        _ => 0,
    }
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

/// The rate divisor of the visual-rate types (M2.8-7): 60 Hz at 48 kHz,
/// 120 Hz at 96 kHz. A type's rate is fixed by its descriptor, so the visual
/// types run at this divisor at every master rate.
pub const VISUAL_DIVISOR: u32 = 800;

/// `d` as a divisor (1 for 0, which no caller passes).
const fn divisor(d: u32) -> RateDivisor {
    match RateDivisor::new(d) {
        Some(d) => d,
        None => RateDivisor::ONE,
    }
}

/// A port of any shape at rate divisor `d`.
pub(crate) const fn port_at(
    name: &'static str,
    direction: Direction,
    shape: PortShape,
    d: u32,
    kind: PortKind,
    unit: Option<&'static str>,
    range: Option<Range>,
) -> PortDescriptor {
    PortDescriptor {
        name,
        direction,
        shape,
        rate: divisor(d),
        kind,
        unit,
        range,
    }
}

/// A parameter at rate divisor `d`.
#[allow(clippy::too_many_arguments)]
pub(crate) const fn parameter_at(
    name: &'static str,
    d: u32,
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
        rate: divisor(d),
        unit,
        range: Range { min, max },
        default,
        combine,
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

    const NAMES: [&str; 74] = [
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
        "std.string",
        "std.modal",
        "std.mass_spring",
        "std.exciter",
        "std.count",
        "std.particles",
        "std.mesh2d",
        "std.mesh2d_audio",
        "std.body2d",
        "std.div",
        "std.neg",
        "std.mod",
        "std.fract",
        "std.floor",
        "std.round",
        "std.sign",
        "std.sqrt",
        "std.exp",
        "std.log",
        "std.pow",
        "std.sin",
        "std.cos",
        "std.tan",
        "std.tanh",
        "std.atan2",
        "std.sin_cycles",
        "std.const",
        "std.sample_rate",
        "std.frames",
        "std.compare",
        "std.select",
        "std.and",
        "std.or",
        "std.not",
        "std.phasor",
        "std.counter",
        "std.integrator",
        "std.buffer1k",
        "std.buffer64k",
        "std.buffer1m",
        "std.latch",
        "std.to_events",
        "std.route",
        "std.random",
    ];

    /// M2.11-1: the sequencers of ADR 0028 §6, category `event`, each with
    /// its pattern kind.
    const SEQUENCERS: [&str; 2] = ["std.note_seq", "std.step_seq8"];

    /// M2.11-2: the transistor-ladder filter.
    const LADDER: [&str; 1] = ["std.ladder"];

    /// M2.10 (the owner's list): the native DSP primitives, with their
    /// categories.
    const DSP: [(&str, &str); 6] = [
        ("std.biquad", category::FILTER),
        ("std.comb", category::DELAY),
        ("std.allpass_delay", category::DELAY),
        ("std.fdn_reverb", category::DELAY),
        ("std.fm_op", category::GENERATOR),
        ("std.wavetable", category::GENERATOR),
    ];

    /// The native DSP primitives are direct, at the master rate, in their
    /// categories.
    #[test]
    fn the_dsp_primitives_are_direct_in_their_categories() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        for (name, group) in DSP {
            let d = registry.get(name).unwrap().descriptor;
            assert_eq!(d.category, Some(group), "{name}");
            assert_eq!(d.feedthrough, signal_core::Feedthrough::Direct, "{name}");
            assert_eq!(d.rate(), Some(RateDivisor::ONE), "{name}");
        }
    }

    /// M2.11-2 AC1: `std.ladder` is a direct `filter` at the master rate.
    #[test]
    fn the_ladder_is_a_direct_filter() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        let d = registry.get("std.ladder").unwrap().descriptor;
        assert_eq!(d.category, Some(category::FILTER));
        assert_eq!(d.feedthrough, signal_core::Feedthrough::Direct);
        assert_eq!(d.rate(), Some(RateDivisor::ONE));
    }

    /// M2.10-6: every name in [`COST_NS`] is a registered type, and the
    /// registry holds its cost after `register`, so no entry is silently
    /// lost to a misspelt or renamed type (`Registry::set_cost` checks
    /// nothing).
    #[test]
    fn every_cost_names_a_registered_type() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        for (name, ns) in COST_NS {
            assert!(registry.get(name).is_some(), "{name} is not registered");
            assert_eq!(registry.cost(name), Some(ns), "{name}");
        }
    }

    /// A `Set` counts at most the transport's largest per-frame count, so a
    /// sequencer's work per event is bounded.
    #[test]
    fn a_tick_set_counts_at_most_the_transport_s_largest() {
        assert_eq!(tick_count(EventPayload::Trigger), 1);
        assert_eq!(tick_count(EventPayload::Set(20.0)), 20);
        assert_eq!(tick_count(EventPayload::Set(6400.0)), 6400);
        assert_eq!(tick_count(EventPayload::Set(1e9)), MAX_TICKS_PER_EVENT);
        assert_eq!(tick_count(EventPayload::Set(-3.0)), 0);
        assert_eq!(tick_count(EventPayload::Set(f64::NAN)), 0);
        // 400 bpm at 960 ppq and 1 Hz: at most 6401 ticks in a frame.
        assert_eq!(400 * 960 / 60 + 1, MAX_TICKS_PER_EVENT + 1);
    }

    #[test]
    fn the_sequencers_take_their_pattern_kinds() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        for (name, kind) in SEQUENCERS.into_iter().zip([pattern::NOTES, pattern::STEPS]) {
            let d = registry.get(name).unwrap().descriptor;
            assert_eq!(d.category, Some(category::EVENT), "{name}");
            assert_eq!(registry.table_rule(name).map(|r| r.kind), Some(kind));
        }
        assert!(registry.table_rule("std.step_seq").is_none());
    }

    #[test]
    fn register_adds_every_type_under_its_name() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(
            registry.len(),
            NAMES.len() + SEQUENCERS.len() + LADDER.len() + DSP.len() + vector::types().len()
        );
        let vectors = vector::types().iter().map(|t| t.descriptor.type_name);
        for name in NAMES
            .into_iter()
            .chain(SEQUENCERS)
            .chain(LADDER)
            .chain(DSP.map(|(name, _)| name))
            .chain(vectors)
        {
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
        assert_eq!(
            registry.len(),
            NAMES.len() + SEQUENCERS.len() + LADDER.len() + DSP.len() + vector::types().len()
        );

        // A type registered first under a std name blocks that name.
        let mut registry = Registry::new();
        registry.register(direct::<Mul>()).unwrap();
        assert_eq!(
            register(&mut registry),
            Err(RegistryError::DuplicateTypeName("std.mul"))
        );
    }

    /// AC1, AC3: every std type has a category, and only from [`category::ALL`].
    #[test]
    fn every_type_has_a_category_of_the_vocabulary() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        for module_type in registry.iter() {
            let d = module_type.descriptor;
            let category = d.category.unwrap_or_else(|| panic!("{}", d.type_name));
            assert!(
                category::ALL.contains(&category),
                "{}: {category}",
                d.type_name
            );
        }
        let vectors = vector::types().iter().map(|t| t.descriptor.type_name);
        for name in NAMES[39..].iter().copied().chain(vectors) {
            let d = registry.get(name).unwrap().descriptor;
            assert_eq!(d.category, Some(category::CORE), "{name}");
            assert_eq!(d.feedthrough, signal_core::Feedthrough::Direct, "{name}");
            assert_eq!(d.rate(), Some(RateDivisor::ONE), "{name}");
        }
        // M2.10-2 AC1, AC4: the core set's earlier state and event types keep
        // the groups M2.10-1 gave them.
        for (name, group) in [
            ("std.unit_delay", category::CORE),
            ("std.delay", category::DELAY),
            ("std.lag", category::FILTER),
            ("std.onepole", category::FILTER),
            ("std.sample_hold", category::EVENT),
            ("std.edge", category::EVENT),
            ("std.clock_div", category::EVENT),
        ] {
            assert_eq!(registry.get(name).unwrap().descriptor.category, Some(group));
        }
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
