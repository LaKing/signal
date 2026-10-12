//! `std.sub`, `std.min`, `std.max`, `std.abs`, `std.scale` and `std.clamp`:
//! stateless arithmetic on signals, beside `std.add` and `std.mul`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs,
    PortDescriptor, PortKind, PrepareContext,
};

use crate::{at, parameter, signal_port};

/// The static bound of the `scale` and `clamp` parameters.
const BOUND: f64 = 1e6;

/// The ports of `std.sub`, `std.min` and `std.max`.
const BINARY_PORTS: &[PortDescriptor] = &[
    signal_port("a", Direction::In, PortKind::Generic, None, None),
    signal_port("b", Direction::In, PortKind::Generic, None, None),
    signal_port("out", Direction::Out, PortKind::Generic, None, None),
];

const fn binary(type_name: &'static str, doc: &'static str) -> ModuleDescriptor {
    ModuleDescriptor {
        type_name,
        doc,
        ports: BINARY_PORTS,
        parameters: &[],
        feedthrough: Feedthrough::Direct,
        sink: None,
        latency: None,
        reproducibility: signal_core::Reproducibility::BitExact,
    }
}

/// `out[n] = f(a[n], b[n])`; unconnected inputs read 0.
fn process_binary(inputs: &Inputs<'_>, outputs: &mut Outputs<'_>, f: fn(f64, f64) -> f64) {
    let (a, b) = (inputs.signal(0), inputs.signal(1));
    for (n, out) in outputs.signal(0).iter_mut().enumerate() {
        *out = f(at(a, n, 0.0), at(b, n, 0.0));
    }
}

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
