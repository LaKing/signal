//! `std.sqrt`, `std.exp`, `std.log`, `std.pow`, `std.sin`, `std.cos`,
//! `std.tan`, `std.tanh`, `std.atan2` and `std.sin_cycles`: the core set's
//! functions (ADR 0027 §1). Every transcendental function is `libm`'s (ADR
//! 0007), never a `std` float method, and every type is total: finite inputs
//! give finite outputs, each edge case stated with its formula.
//!
//! **Bound.** Every output is clamped to `±1e6`
//! ([`SIGNAL_BOUND`](crate::SIGNAL_BOUND), the engine's magnitude bound,
//! `docs/semantics.md` §7), so a type never faults on its own: that bounds
//! `std.exp`, `std.pow` and `std.tan`, which grow beyond it, and `std.sqrt` on
//! an input beyond 10¹² (only a fan-in sum or a connection scale gives one).
//!
//! **Subnormals.** No type here flushes a subnormal result: a result below the
//! normal range is `libm`'s (a subnormal or 0), for `std.exp` and `std.pow`
//! alike. These types hold no state, so nothing decays towards a subnormal
//! across frames; a subnormal output is the exact value of its input.
//!
//! Ports: `std.pow` and `std.atan2` take ordered inputs `a`, `b`, the others
//! `in`; each has one output `out`. All are generic, unbounded, direct, at
//! rate divisor 1, category `core`; unconnected inputs read 0. Angles are in
//! radians except `std.sin_cycles`'s, in cycles. Stateless; constant work per
//! frame.

use crate::math::stateless_type;

/// `√x`, `+0` for `x ≤ 0`.
pub(crate) fn sqrt(x: f64) -> f64 {
    if x > 0.0 { libm::sqrt(x) } else { 0.0 }
}

/// `eˣ` (`libm`; the type clamps it to the bound).
pub(crate) fn exp(x: f64) -> f64 {
    libm::exp(x)
}

/// `ln x`, with `x` raised to the smallest positive normal first.
pub(crate) fn log(x: f64) -> f64 {
    libm::log(x.max(f64::MIN_POSITIVE))
}

/// `aᵇ`, total (see [`Pow`]).
pub(crate) fn pow(a: f64, b: f64) -> f64 {
    if b == 0.0 {
        1.0
    } else if (a == 0.0 && b < 0.0) || (a < 0.0 && libm::trunc(b) != b) {
        // 0 to a negative power (as `std.div`), or no real result (as `std.sqrt`).
        0.0
    } else {
        libm::pow(a, b)
    }
}

/// `sin(2π·x)` with the reduction done in cycles.
pub(crate) fn sin_cycles(x: f64) -> f64 {
    // Exact: the distance to the nearest integer, in [−½, ½].
    let mut r = x - libm::roundeven(x);
    // Exact by Sterbenz: sin(2π·r) = sin(2π·(±½ − r)).
    if r > 0.25 {
        r = 0.5 - r;
    } else if r < -0.25 {
        r = -0.5 - r;
    }
    libm::sin(std::f64::consts::TAU * r)
}

stateless_type!(
    /// `std.sqrt`: the square root.
    ///
    /// **Formula:** `out[n] = √in[n]` (`libm::sqrt`) for `in[n] > 0`; `+0` for
    /// every `in[n] ≤ 0`, −0 included; at most `1e6` (an input above 10¹²).
    Sqrt,
    unary,
    "std.sqrt",
    "Square root: out = sqrt(in); 0 for in <= 0.",
    sqrt
);

stateless_type!(
    /// `std.exp`: the exponential.
    ///
    /// **Formula:** `out[n] = min(e^in[n], 1e6)` (`libm::exp`): a result
    /// beyond `1e6` (`in[n]` above `ln 10⁶ ≈ 13.8155`, overflow included) is
    /// `1e6`, the engine's magnitude bound. A result below the normal range is
    /// `libm`'s, a subnormal or 0 (see the module's "Subnormals").
    Exp,
    unary,
    "std.exp",
    "Exponential: out = e^in, at most 1e6.",
    exp
);

stateless_type!(
    /// `std.log`: the natural logarithm.
    ///
    /// **Formula:** `out[n] = ln(max(in[n], f64::MIN_POSITIVE))` (`libm::log`).
    /// So every `in[n] ≤ 0` gives the log of the smallest positive normal,
    /// `ln(2.2250738585072014e−308) = −708.3964185322641`, and so does a
    /// subnormal input, which keeps the output non-decreasing in `in`.
    Log,
    unary,
    "std.log",
    "Natural logarithm: out = ln(in); in <= 0 gives the log of the smallest positive normal.",
    log
);

stateless_type!(
    /// `std.pow`: a power.
    ///
    /// **Formula:** `out[n] = a[n]^b[n]`, the first matching case:
    /// 1. `b = ±0` → 1, whatever `a` (also `0⁰ = 1`);
    /// 2. `a = ±0` and `b < 0` → `+0` (a division by zero gives 0, as in
    ///    `std.div`);
    /// 3. `a < 0` and `b` not an integer → `+0` (no real result, as in
    ///    `std.sqrt`: `pow(a, 0.5)` agrees with `sqrt(a)`'s zero convention for
    ///    every `a ≤ 0`, and equals `sqrt(a)` up to rounding for `a > 0`, since
    ///    `libm::pow` and `libm::sqrt` are different algorithms);
    /// 4. otherwise `libm::pow(a, b)`, a result beyond `±1e6` (overflow
    ///    included) clamped to `±1e6` keeping its sign (a negative `a` with an
    ///    odd integer `b` is negative), the engine's magnitude bound. Results
    ///    below the normal range are `libm`'s (a subnormal or 0).
    ///
    /// **Operands:** ordered: `a` is the base, `b` the exponent.
    Pow,
    binary,
    "std.pow",
    "Power: out = a^b; 1 for b = 0; 0 for a = 0 with b < 0 and for a < 0 with a non-integer b; clamped to +-1e6.",
    pow
);

stateless_type!(
    /// `std.sin`: the sine.
    ///
    /// **Formula:** `out[n] = sin(in[n])` (`libm::sin`), `in` in radians; in
    /// `[−1, 1]`.
    Sin,
    unary,
    "std.sin",
    "Sine of an angle in radians.",
    libm::sin
);

stateless_type!(
    /// `std.cos`: the cosine.
    ///
    /// **Formula:** `out[n] = cos(in[n])` (`libm::cos`), `in` in radians; in
    /// `[−1, 1]`.
    Cos,
    unary,
    "std.cos",
    "Cosine of an angle in radians.",
    libm::cos
);

stateless_type!(
    /// `std.tan`: the tangent.
    ///
    /// **Formula:** `out[n] = tan(in[n])` (`libm::tan`), `in` in radians,
    /// clamped to `[−1e6, 1e6]`, the engine's magnitude bound. No double is an
    /// odd multiple of π/2, so `libm` is finite near a pole (about 1.633e16 at
    /// the double nearest π/2); within about 1e−6 of a pole the output is
    /// `±1e6`.
    Tan,
    unary,
    "std.tan",
    "Tangent of an angle in radians, clamped to +-1e6.",
    libm::tan
);

stateless_type!(
    /// `std.tanh`: the hyperbolic tangent.
    ///
    /// **Formula:** `out[n] = tanh(in[n])` (`libm::tanh`); in `[−1, 1]`.
    Tanh,
    unary,
    "std.tanh",
    "Hyperbolic tangent: out = tanh(in).",
    libm::tanh
);

stateless_type!(
    /// `std.atan2`: the angle of a point.
    ///
    /// **Formula:** `out[n] = atan2(a[n], b[n])` (`libm::atan2`), the angle of
    /// the point `(x, y) = (b, a)` in radians, in `[−π, π]`. Every finite pair
    /// has a value: `atan2(±0, +0) = ±0`, `atan2(±0, −0) = ±π`,
    /// `atan2(y > 0, ±0) = π/2`.
    ///
    /// **Operands:** ordered: `a` is `y`, `b` is `x` (as `atan2(y, x)`).
    Atan2,
    binary,
    "std.atan2",
    "Angle in radians of the point (x = b, y = a): out = atan2(a, b), in [-pi, pi].",
    libm::atan2
);

stateless_type!(
    /// `std.sin_cycles`: the sine of a phase in cycles, the oscillator
    /// convention.
    ///
    /// **Formula:** `out[n] = sin(2π·x)`, `x = in[n]` in cycles, reduced in
    /// cycles before the multiplication: `r = x − roundeven(x)` (exact, in
    /// `[−½, ½]`), then `r ← ½ − r` when `r > ¼` and `r ← −½ − r` when
    /// `r < −¼` (exact), and `out = libm::sin(2π·r)`. So it is exactly ±0 at
    /// every whole and half cycle and exactly ±1 at quarter cycles, for any
    /// `|x|` (from 2⁵² on every double is a whole cycle), and odd in `x` up to
    /// the sign of a zero (a whole cycle gives `+0` for either sign of `x`).
    /// Elsewhere it may differ from `sin(2π·x)` computed directly by a few
    /// ulps.
    SinCycles,
    unary,
    "std.sin_cycles",
    "Sine of a phase in cycles: out = sin(2*pi*in), exact at whole, half and quarter cycles.",
    sin_cycles
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};
    use signal_core::DirectPrimitive;
    use std::f64::consts::{FRAC_PI_2, PI, TAU};

    /// Renders `p` over `signals` in calls of 3 frames.
    fn run<P: DirectPrimitive>(mut p: P, signals: &[&[f64]]) -> Vec<f64> {
        let frames = signals[0].len();
        p.prepare(&context(48_000.0, 3, 0));
        let timeline = Timeline {
            signals,
            ..Timeline::default()
        };
        render(&mut p, &timeline, frames, 3).signals.remove(0)
    }

    fn bits(x: f64) -> u64 {
        x.to_bits()
    }

    /// The types compute `libm`'s values, through the formulas above.
    #[test]
    fn the_types_compute_their_formulas() {
        let x = [0.3, -1.7, 2.0, 0.0];
        let each = |f: fn(f64) -> f64| x.map(f).to_vec();
        assert_eq!(run(Sqrt, &[&x]), each(sqrt));
        assert_eq!(run(Exp, &[&x]), each(libm::exp));
        assert_eq!(run(Log, &[&x]), each(log));
        assert_eq!(run(Sin, &[&x]), each(libm::sin));
        assert_eq!(run(Cos, &[&x]), each(libm::cos));
        assert_eq!(run(Tan, &[&x]), each(libm::tan));
        assert_eq!(run(Tanh, &[&x]), each(libm::tanh));
        assert_eq!(run(SinCycles, &[&x]), each(sin_cycles));
        let y = [2.0, 3.0, -0.5, 1.0];
        assert_eq!(
            run(Pow, &[&x, &y]),
            [
                libm::pow(0.3, 2.0),
                libm::pow(-1.7, 3.0),
                libm::pow(2.0, -0.5),
                0.0
            ]
        );
        assert_eq!(
            run(Atan2, &[&x, &y]),
            [
                libm::atan2(0.3, 2.0),
                libm::atan2(-1.7, 3.0),
                libm::atan2(2.0, -0.5),
                0.0
            ]
        );
    }

    /// AC5: x ≤ 0 gives +0.
    #[test]
    fn sqrt_of_non_positive_is_zero() {
        assert_eq!(sqrt(4.0), 2.0);
        for x in [-1.0, -1e-300, -0.0, 0.0, -f64::MAX] {
            assert_eq!(bits(sqrt(x)), bits(0.0), "{x}");
        }
    }

    /// AC5: a result beyond 1e6 is 1e6; below the normal range it is
    /// libm's, a subnormal included (no flush; auditor-2-exp-underflow-subnormal
    /// superseded by the M2.10-1 review).
    #[test]
    fn exp_is_clamped_to_the_bound_and_not_flushed() {
        assert_eq!(
            run(
                Exp,
                &[&[0.0, 13.0, 14.0, 710.0, f64::MAX, -720.0, -f64::MAX]]
            ),
            [1.0, libm::exp(13.0), 1e6, 1e6, 1e6, libm::exp(-720.0), 0.0]
        );
        assert!(libm::exp(-720.0) > 0.0 && libm::exp(-720.0) < f64::MIN_POSITIVE);
    }

    /// AC5: x ≤ 0 (and a subnormal x) gives the log of the smallest normal.
    #[test]
    fn log_of_non_positive_is_the_log_of_the_smallest_normal() {
        let floor = libm::log(f64::MIN_POSITIVE);
        assert_eq!(floor, -708.3964185322641);
        for x in [0.0, -0.0, -1.0, -f64::MAX, 1e-310] {
            assert_eq!(log(x), floor, "{x}");
        }
        assert_eq!(log(1.0), 0.0);
        assert_eq!(log(f64::MAX), libm::log(f64::MAX));
    }

    /// AC5: every domain case of `pow`.
    #[test]
    fn pow_is_total() {
        // 1. b = ±0.
        for a in [0.0, -0.0, -3.0, 5.0] {
            assert_eq!(pow(a, 0.0), 1.0);
            assert_eq!(pow(a, -0.0), 1.0);
        }
        // 2. 0 to a negative power.
        assert_eq!(bits(pow(0.0, -1.0)), bits(0.0));
        assert_eq!(bits(pow(-0.0, -3.0)), bits(0.0));
        assert_eq!(bits(pow(0.0, -0.5)), bits(0.0));
        // 3. a negative base with a non-integer exponent.
        assert_eq!(bits(pow(-2.0, 0.5)), bits(0.0));
        assert_eq!(bits(pow(-2.0, -1.5)), bits(0.0));
        // 4. libm, clamped (by the type) to ±1e6.
        assert_eq!(pow(-2.0, 3.0), -8.0);
        assert_eq!(pow(-2.0, -2.0), 0.25);
        // Exact for this value; in general pow(a, 0.5) and sqrt(a) agree only
        // up to rounding.
        assert_eq!(pow(2.0, 0.5), libm::sqrt(2.0));
        assert_eq!(
            run(
                Pow,
                &[
                    &[10.0, 10.0, -10.0, 1e-300, 10.0, -10.0],
                    &[7.0, 400.0, 401.0, -2.0, 6.0, 7.0]
                ]
            ),
            [1e6, 1e6, -1e6, 1e6, 1e6, -1e6]
        );
        assert_eq!(bits(pow(0.0, 2.0)), bits(0.0));
        assert_eq!(pow(1.0, 1e300), 1.0);
    }

    /// AC5: near a pole the tangent is clamped to ±1e6.
    #[test]
    fn tan_is_clamped_near_poles() {
        // libm is finite there, far beyond the bound.
        assert!((libm::tan(FRAC_PI_2) - 1.633_123_935_319_537e16).abs() < 1e3);
        let x = [
            FRAC_PI_2,
            -FRAC_PI_2,
            45.553_093_477_052,
            FRAC_PI_2 - 1e-7,
            FRAC_PI_2 + 1e-7,
            1.0,
        ];
        assert_eq!(
            run(Tan, &[&x]),
            [
                1e6,
                -1e6,
                libm::tan(45.553_093_477_052).clamp(-1e6, 1e6),
                1e6,
                -1e6,
                libm::tan(1.0)
            ]
        );
        assert!(run(Tan, &[&[3.0 * FRAC_PI_2]])[0].abs() == 1e6);
    }

    /// AC5: sqrt of an input beyond 10¹² is 1e6.
    #[test]
    fn sqrt_is_clamped_to_the_bound() {
        assert_eq!(run(Sqrt, &[&[1e12, 4e12, 1e300]]), [1e6, 1e6, 1e6]);
    }

    #[test]
    fn sin_cos_tanh_are_bounded() {
        for x in [-1e300, -7.5, 0.0, 1.0, 1e300] {
            assert!(libm::sin(x).abs() <= 1.0 && libm::cos(x).abs() <= 1.0);
            assert!(libm::tanh(x).abs() <= 1.0);
        }
        assert_eq!(libm::tanh(1e300), 1.0);
    }

    /// AC5: radians, every finite pair, signed zeros as documented.
    #[test]
    fn atan2_signed_zeros() {
        assert_eq!(bits(libm::atan2(0.0, 0.0)), bits(0.0));
        assert_eq!(bits(libm::atan2(-0.0, 0.0)), bits(-0.0));
        assert_eq!(libm::atan2(0.0, -0.0), PI);
        assert_eq!(libm::atan2(-0.0, -0.0), -PI);
        assert_eq!(libm::atan2(1.0, 0.0), FRAC_PI_2);
        assert_eq!(libm::atan2(1.0, -1.0), 3.0 * PI / 4.0);
    }

    /// AC5: exact at whole and half cycles (and ±1 at quarter cycles), odd,
    /// and within a few ulps of sin(2πx) elsewhere.
    #[test]
    fn sin_cycles_is_exact_at_whole_and_half_cycles() {
        for x in [
            -3.0,
            -2.5,
            -0.5,
            0.0,
            -0.0,
            0.5,
            1.0,
            2.5,
            1e6 + 0.5,
            2f64.powi(60),
        ] {
            assert_eq!(sin_cycles(x), 0.0, "{x}");
        }
        for x in [0.25, 1.25, -0.75, 1e6 + 0.25] {
            assert_eq!(sin_cycles(x), 1.0, "{x}");
        }
        for x in [0.75, -0.25, -1.25] {
            assert_eq!(sin_cycles(x), -1.0, "{x}");
        }
        // libm's direct form is not exact there.
        assert_ne!(libm::sin(TAU * 0.5), 0.0);
        let mut x = -10.0;
        while x < 10.0 {
            assert_eq!(sin_cycles(-x), -sin_cycles(x), "{x}");
            assert!((sin_cycles(x) - libm::sin(TAU * x)).abs() < 1e-13, "{x}");
            x += 0.013;
        }
    }
}
