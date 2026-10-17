//! `std.sub`, `std.min`, `std.max`, `std.abs`, `std.scale` and `std.clamp`:
//! stateless arithmetic on signals, beside `std.add` and `std.mul`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs,
    PortDescriptor, PortKind, PrepareContext,
};

use crate::{at, parameter, signal_port};

/// The static bound of the `scale` and `clamp` parameters: the engine's
/// magnitude bound.
const BOUND: f64 = crate::SIGNAL_BOUND;

/// The ports of the two-operand core types (`std.sub`, `std.min`, `std.div`,
/// …): ordered operands `a` and `b`, so a result never depends on the
/// address order of what is connected (ADR 0027 §1).
const BINARY_PORTS: &[PortDescriptor] = &[
    signal_port("a", Direction::In, PortKind::Generic, None, None),
    signal_port("b", Direction::In, PortKind::Generic, None, None),
    signal_port("out", Direction::Out, PortKind::Generic, None, None),
];

/// The ports of the one-operand core types (`std.neg`, `std.sqrt`, …).
const UNARY_PORTS: &[PortDescriptor] = &[
    signal_port("in", Direction::In, PortKind::Generic, None, None),
    signal_port("out", Direction::Out, PortKind::Generic, None, None),
];

/// A stateless `core` type with `ports` and no parameters.
pub(crate) const fn stateless(
    type_name: &'static str,
    doc: &'static str,
    ports: &'static [PortDescriptor],
) -> ModuleDescriptor {
    ModuleDescriptor {
        type_name,
        doc,
        ports,
        parameters: &[],
        feedthrough: Feedthrough::Direct,
        sink: None,
        latency: None,
        reproducibility: signal_core::Reproducibility::BitExact,
        category: Some(crate::category::CORE),
    }
}

/// A two-operand `core` type: inputs `a`, `b`, output `out`.
pub(crate) const fn binary(type_name: &'static str, doc: &'static str) -> ModuleDescriptor {
    stateless(type_name, doc, BINARY_PORTS)
}

/// A one-operand `core` type: input `in`, output `out`.
pub(crate) const fn unary(type_name: &'static str, doc: &'static str) -> ModuleDescriptor {
    stateless(type_name, doc, UNARY_PORTS)
}

/// `out[n] = f(a[n], b[n])`; unconnected inputs read 0.
pub(crate) fn process_binary(
    inputs: &Inputs<'_>,
    outputs: &mut Outputs<'_>,
    f: fn(f64, f64) -> f64,
) {
    let (a, b) = (inputs.signal(0), inputs.signal(1));
    for (n, out) in outputs.signal(0).iter_mut().enumerate() {
        *out = f(at(a, n, 0.0), at(b, n, 0.0));
    }
}

/// `out[n] = f(in[n])`; an unconnected input reads 0.
pub(crate) fn process_unary(inputs: &Inputs<'_>, outputs: &mut Outputs<'_>, f: fn(f64) -> f64) {
    let x = inputs.signal(0);
    for (n, out) in outputs.signal(0).iter_mut().enumerate() {
        *out = f(at(x, n, 0.0));
    }
}

/// `x` clamped to `±SIGNAL_BOUND` (`docs/semantics.md` §7); a zero keeps its
/// sign.
pub(crate) fn bounded(x: f64) -> f64 {
    x.clamp(-crate::SIGNAL_BOUND, crate::SIGNAL_BOUND)
}

/// A stateless core type whose output is one formula of its operands:
/// `stateless_type!(Div, binary, "std.div", "doc", div)` declares the
/// struct `Div` (the attributes before it are its rustdoc) and its
/// [`DirectPrimitive`] with the [`binary`] (or [`unary`]) ports, computing
/// `out[n] = bounded(div(a[n], b[n]))`: every output is clamped to
/// `±SIGNAL_BOUND`, the engine's magnitude bound, so the type never faults on
/// its own (the one place the M2.10-1 stateless types apply it).
macro_rules! stateless_type {
    ($(#[$doc:meta])* $name:ident, binary, $type_name:literal, $summary:literal, $f:expr) => {
        stateless_type!(@impl $(#[$doc])* $name, binary, process_binary, $type_name, $summary,
            |a, b| crate::math::bounded(($f)(a, b)));
    };
    ($(#[$doc:meta])* $name:ident, unary, $type_name:literal, $summary:literal, $f:expr) => {
        stateless_type!(@impl $(#[$doc])* $name, unary, process_unary, $type_name, $summary,
            |x| crate::math::bounded(($f)(x)));
    };
    (@impl $(#[$doc:meta])* $name:ident, $shape:ident, $process:ident, $type_name:literal, $summary:literal, $f:expr) => {
        $(#[$doc])*
        #[derive(Clone, Debug, Default)]
        pub struct $name;

        impl signal_core::DirectPrimitive for $name {
            fn describe() -> signal_core::ModuleDescriptor {
                crate::math::$shape($type_name, $summary)
            }

            fn prepare(&mut self, _ctx: &signal_core::PrepareContext) {}

            fn reset(&mut self) {}

            fn process(
                &mut self,
                inputs: &signal_core::Inputs<'_>,
                outputs: &mut signal_core::Outputs<'_>,
            ) {
                crate::math::$process(inputs, outputs, $f);
            }
        }
    };
}
pub(crate) use stateless_type;

/// `std.sub`: the difference of two signals.
///
/// **Ports**
/// - `a`, `b` (signal in, generic, unbounded); unconnected they read 0.
/// - `out` (signal out, generic).
///
/// **Parameters:** none.
///
/// **Feedthrough:** direct.
///
/// **Formula:** `out[n] = a[n] − b[n]`. Stateless; constant work per frame.
///
/// **Operands:** ordered: `a` is the minuend and `b` the subtrahend, by port,
/// never by connection order.
#[derive(Clone, Debug, Default)]
pub struct Sub;

impl DirectPrimitive for Sub {
    fn describe() -> ModuleDescriptor {
        binary(
            "std.sub",
            "Subtracts its second input from its first: out = a - b.",
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        process_binary(inputs, outputs, |a, b| a - b);
    }
}

/// `std.min`: the smaller of two signals.
///
/// **Ports**
/// - `a`, `b` (signal in, generic, unbounded); unconnected they read 0.
/// - `out` (signal out, generic).
///
/// **Parameters:** none.
///
/// **Feedthrough:** direct.
///
/// **Formula:** `out[n] = min(a[n], b[n])`, as `f64::min` (IEEE 754
/// `minNum`): when exactly one input is NaN the output is the other input, and
/// it is NaN only when both are. Stateless; constant work per frame.
///
/// **Operands:** ordered ports `a` and `b`; swapping them changes nothing.
/// On the supported target (aarch64, Apple Silicon) `f64::min` compiles to
/// `FMINNM`, which orders −0 below +0, so `min(−0, +0) = min(+0, −0) = −0`
/// (a unit test pins it). Rust itself leaves the zero unspecified, so another
/// target may differ there.
#[derive(Clone, Debug, Default)]
pub struct Min;

impl DirectPrimitive for Min {
    fn describe() -> ModuleDescriptor {
        binary(
            "std.min",
            "The smaller of its two inputs (f64::min: a NaN input yields the other input).",
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        process_binary(inputs, outputs, f64::min);
    }
}

/// `std.max`: the larger of two signals.
///
/// **Ports**
/// - `a`, `b` (signal in, generic, unbounded); unconnected they read 0.
/// - `out` (signal out, generic).
///
/// **Parameters:** none.
///
/// **Feedthrough:** direct.
///
/// **Formula:** `out[n] = max(a[n], b[n])`, as `f64::max` (IEEE 754
/// `maxNum`): when exactly one input is NaN the output is the other input, and
/// it is NaN only when both are. Stateless; constant work per frame.
///
/// **Operands:** ordered ports `a` and `b`; swapping them changes nothing.
/// On the supported target (aarch64, Apple Silicon) `f64::max` compiles to
/// `FMAXNM`, which orders −0 below +0, so `max(−0, +0) = max(+0, −0) = +0`
/// (a unit test pins it). Rust itself leaves the zero unspecified, so another
/// target may differ there.
#[derive(Clone, Debug, Default)]
pub struct Max;

impl DirectPrimitive for Max {
    fn describe() -> ModuleDescriptor {
        binary(
            "std.max",
            "The larger of its two inputs (f64::max: a NaN input yields the other input).",
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        process_binary(inputs, outputs, f64::max);
    }
}

const ABS: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.abs",
    doc: "Absolute value: out = |in|.",
    ports: &[
        signal_port("in", Direction::In, PortKind::Generic, None, None),
        signal_port("out", Direction::Out, PortKind::Generic, None, None),
    ],
    parameters: &[],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::CORE),
};

/// `std.abs`: the absolute value of a signal.
///
/// **Ports**
/// - `in` (signal in, generic, unbounded); unconnected it reads 0.
/// - `out` (signal out, generic, ≥ 0).
///
/// **Parameters:** none.
///
/// **Feedthrough:** direct.
///
/// **Formula:** `out[n] = |in[n]|` (`f64::abs`, which clears the sign bit).
/// Stateless; constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Abs;

impl DirectPrimitive for Abs {
    fn describe() -> ModuleDescriptor {
        ABS
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let x = inputs.signal(0);
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            *out = at(x, n, 0.0).abs();
        }
    }
}

const SCALE: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.scale",
    doc: "Scale and offset: out = in·mul + add.",
    ports: &[
        signal_port("in", Direction::In, PortKind::Generic, None, None),
        signal_port("out", Direction::Out, PortKind::Generic, None, None),
    ],
    parameters: &[
        parameter(
            "mul",
            PortKind::Generic,
            None,
            -BOUND,
            BOUND,
            1.0,
            Combine::Add,
        ),
        parameter(
            "add",
            PortKind::Generic,
            None,
            -BOUND,
            BOUND,
            0.0,
            Combine::Add,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::CORE),
};

/// `std.scale`: a linear map of a signal.
///
/// **Ports**
/// - `in` (signal in, generic, unbounded); unconnected it reads 0.
/// - `out` (signal out, generic).
///
/// **Parameters**
/// - `mul` (generic, no unit, range `[−1e6, 1e6]`, default 1, combine `add`).
/// - `add` (generic, no unit, range `[−1e6, 1e6]`, default 0, combine `add`).
///
/// **Feedthrough:** direct.
///
/// **Formula:** `out[n] = in[n]·mul[n] + add[n]`. Stateless; constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Scale;

impl DirectPrimitive for Scale {
    fn describe() -> ModuleDescriptor {
        SCALE
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let (x, mul, add) = (inputs.signal(0), inputs.parameter(0), inputs.parameter(1));
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            *out = at(x, n, 0.0) * at(mul, n, 1.0) + at(add, n, 0.0);
        }
    }
}

const CLAMP: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.clamp",
    doc: "Clamps its input to [lo, hi]; when lo > hi the output is lo.",
    ports: &[
        signal_port("in", Direction::In, PortKind::Generic, None, None),
        signal_port("out", Direction::Out, PortKind::Generic, None, None),
    ],
    parameters: &[
        parameter(
            "lo",
            PortKind::Generic,
            None,
            -BOUND,
            BOUND,
            -1.0,
            Combine::Add,
        ),
        parameter(
            "hi",
            PortKind::Generic,
            None,
            -BOUND,
            BOUND,
            1.0,
            Combine::Add,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::CORE),
};

/// `std.clamp`: a signal limited to a range.
///
/// **Ports**
/// - `in` (signal in, generic, unbounded); unconnected it reads 0.
/// - `out` (signal out, generic).
///
/// **Parameters**
/// - `lo` (generic, no unit, range `[−1e6, 1e6]`, default −1, combine `add`).
/// - `hi` (generic, no unit, range `[−1e6, 1e6]`, default 1, combine `add`).
///
/// **Feedthrough:** direct.
///
/// **Formula:**
///
/// ```text
/// out[n] = lo[n]                          if lo[n] > hi[n]
///        = min(max(in[n], lo[n]), hi[n])  otherwise
/// ```
///
/// So an empty range (`lo > hi`) outputs `lo`, never panics (unlike
/// `f64::clamp`), and a NaN input outputs `lo` (`f64::max` ignores NaN).
/// Stateless; constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Clamp;

impl DirectPrimitive for Clamp {
    fn describe() -> ModuleDescriptor {
        CLAMP
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let (x, lo, hi) = (inputs.signal(0), inputs.parameter(0), inputs.parameter(1));
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            let (lo, hi) = (at(lo, n, -1.0), at(hi, n, 1.0));
            *out = if lo > hi {
                lo
            } else {
                at(x, n, 0.0).max(lo).min(hi)
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    fn run<P: DirectPrimitive>(mut p: P, signals: &[&[f64]], parameters: &[&[f64]]) -> Vec<f64> {
        let frames = signals[0].len();
        p.prepare(&context(48_000.0, 3, 0));
        let run = render(
            &mut p,
            &Timeline {
                signals,
                parameters,
                triggers: &[],
            },
            frames,
            3,
        );
        run.signals.into_iter().next().unwrap()
    }

    const A: [f64; 5] = [1.5, -2.0, 0.0, 3.0, -0.0];
    const B: [f64; 5] = [2.0, 0.25, 7.0, -1.0, 0.0];

    #[test]
    fn sub_min_max_abs() {
        assert_eq!(run(Sub, &[&A, &B], &[]), [-0.5, -2.25, -7.0, 4.0, -0.0]);
        assert_eq!(run(Min, &[&A, &B], &[]), [1.5, -2.0, 0.0, -1.0, -0.0]);
        assert_eq!(run(Max, &[&A, &B], &[]), [2.0, 0.25, 7.0, 3.0, 0.0]);
        let abs = run(Abs, &[&A], &[]);
        assert_eq!(abs, [1.5, 2.0, 0.0, 3.0, 0.0]);
        assert!(abs.iter().all(|y| y.is_sign_positive()));
    }

    /// The signed-zero case on aarch64 (FMINNM/FMAXNM order −0 below +0), at
    /// run time, in either operand order.
    #[cfg(target_arch = "aarch64")]
    #[test]
    fn min_and_max_of_signed_zeros() {
        let a = [-0.0, 0.0];
        let b = [0.0, -0.0];
        for v in run(Min, &[&a, &b], &[]) {
            assert_eq!(v.to_bits(), (-0.0f64).to_bits());
        }
        for v in run(Max, &[&a, &b], &[]) {
            assert_eq!(v.to_bits(), 0.0f64.to_bits());
        }
    }

    /// AC1: min and max ignore one NaN, as `f64::min`/`max`, and give NaN for two.
    #[test]
    fn min_and_max_with_nan() {
        let nan = f64::NAN;
        let (a, b) = ([nan, 1.0, nan], [2.0, nan, nan]);
        let min = run(Min, &[&a, &b], &[]);
        let max = run(Max, &[&a, &b], &[]);
        assert_eq!(&min[..2], [2.0, 1.0]);
        assert_eq!(&max[..2], [2.0, 1.0]);
        assert!(min[2].is_nan() && max[2].is_nan());
    }

    #[test]
    fn scale_is_mul_then_add() {
        let x = [1.0, -2.0, 0.5];
        let out = run(Scale, &[&x], &[&[2.0, 1e6, -1.0], &[0.5, 0.0, -1e6]]);
        assert_eq!(out, [2.5, -2e6, -1e6 - 0.5]);
    }

    /// AC1: clamp limits to [lo, hi], outputs lo when lo > hi, and lo for NaN.
    #[test]
    fn clamp_to_the_range_and_lo_when_empty() {
        let x = [-3.0, 0.25, 3.0, 0.0, 5.0, f64::NAN];
        let lo = [-1.0, -1.0, -1.0, 2.0, 2.0, -0.5];
        let hi = [1.0, 1.0, 1.0, -2.0, 2.0, 0.5];
        assert_eq!(
            run(Clamp, &[&x], &[&lo, &hi]),
            [-1.0, 0.25, 1.0, 2.0, 2.0, -0.5]
        );
    }
}
