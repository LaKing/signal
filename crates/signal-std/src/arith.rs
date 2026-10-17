//! `std.div`, `std.neg`, `std.mod`, `std.fract`, `std.floor`, `std.round` and
//! `std.sign`: the core set's arithmetic beyond `std.add`, `std.sub` and
//! `std.mul` (ADR 0027 §1). Every type is total: finite inputs give finite
//! outputs, and each edge case is stated with its formula. Every output is
//! clamped to `±1e6` ([`SIGNAL_BOUND`](crate::SIGNAL_BOUND), the engine's
//! magnitude bound, `docs/semantics.md` §7), so a type never faults on its
//! own: that bounds `std.div`'s quotient, and the others' outputs on an input
//! that a fan-in sum or a connection scale took beyond the bound.
//!
//! Ports: the two-operand types take ordered inputs `a`, `b`, the others
//! `in`; each has one output `out`. All are generic, unbounded, direct, at
//! rate divisor 1, category `core`; unconnected inputs read 0. Stateless;
//! constant work per frame.

use crate::math::stateless_type;

/// `a / b`, `+0` when `b` is ±0 (the type clamps the quotient to the bound).
pub(crate) fn div(a: f64, b: f64) -> f64 {
    if b == 0.0 { 0.0 } else { a / b }
}

/// `−x`.
pub(crate) fn neg(x: f64) -> f64 {
    -x
}

/// The floored remainder of `a / b`, with the sign of `b`; `+0` when `b` is ±0.
pub(crate) fn modulo(a: f64, b: f64) -> f64 {
    if b == 0.0 {
        return 0.0;
    }
    // Exact: |r| < |b| with the sign of `a`.
    let mut r = libm::fmod(a, b);
    if r != 0.0 && (r < 0.0) != (b < 0.0) {
        r += b;
        // The sum rounds to `b` only when `r` was tiny against it.
        if r == b {
            r = if b > 0.0 { b.next_down() } else { b.next_up() };
        }
    }
    if r == 0.0 { 0.0 } else { r }
}

/// `x − ⌊x⌋` in `[0, 1)`: [`fract`](crate::fract), with its rounded 1 mapped
/// to the largest double below 1.
pub(crate) fn fraction(x: f64) -> f64 {
    let f = crate::fract(x);
    if f >= 1.0 { 1.0f64.next_down() } else { f }
}

/// `⌊x⌋`.
pub(crate) fn floor(x: f64) -> f64 {
    libm::floor(x)
}

/// `x` rounded to the nearest integer, ties to even.
pub(crate) fn round(x: f64) -> f64 {
    libm::roundeven(x)
}

/// 1 for `x > 0`, −1 for `x < 0`, `+0` for ±0.
pub(crate) fn sign(x: f64) -> f64 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

stateless_type!(
    /// `std.div`: the quotient of two signals.
    ///
    /// **Formula:** `out[n] = a[n] / b[n]`, with:
    /// - `b[n] = ±0` → `+0` (division by zero gives 0, whatever `a`);
    /// - a quotient beyond `±1e6` (e.g. `1 / 1e−7`, or `1e300 / 1e−300`, which
    ///   overflows) → `±1e6`, keeping its sign: the engine's magnitude bound
    ///   ([`SIGNAL_BOUND`](crate::SIGNAL_BOUND)), so the type never faults.
    ///
    /// **Operands:** ordered: `a` is the dividend, `b` the divisor.
    Div,
    binary,
    "std.div",
    "Divides its first input by its second: out = a / b; 0 when b = 0, clamped to +-1e6.",
    div
);

stateless_type!(
    /// `std.neg`: the negation of a signal.
    ///
    /// **Formula:** `out[n] = −in[n]` (flips the sign bit: `neg(+0) = −0`).
    Neg,
    unary,
    "std.neg",
    "Negation: out = -in.",
    neg
);

stateless_type!(
    /// `std.mod`: the floored remainder.
    ///
    /// **Formula:** `out[n] = a − b·⌊a / b⌋`, computed without the quotient:
    /// `r = fmod(a, b)` (`libm`, exact), then `r + b` when `r ≠ 0` and its
    /// sign differs from `b`'s. So the result has the sign of `b`: in
    /// `[0, b)` for `b > 0`, in `(b, 0]` for `b < 0`. Edge cases:
    /// - `b = ±0` → `+0`;
    /// - when `r + b` rounds to `b` (a tiny `r` of the other sign, e.g.
    ///   `mod(−1e−20, 1)`), the result is the double next to `b` towards 0,
    ///   so the range stays half-open;
    /// - a zero result is `+0`;
    /// - any finite `a`, `b` give a finite result, also when `a / b` would
    ///   overflow.
    ///
    /// **Operands:** ordered: `a` is the dividend, `b` the modulus.
    Mod,
    binary,
    "std.mod",
    "Floored remainder: out = a - b*floor(a/b), with the sign of b; 0 when b = 0.",
    modulo
);

stateless_type!(
    /// `std.fract`: the fractional part of a signal.
    ///
    /// **Formula:** `out[n] = x − ⌊x⌋` (`libm::floor`), in `[0, 1)`. When the
    /// subtraction rounds to 1 (a tiny negative `x`, e.g. `−1e−20`), the result
    /// is the largest double below 1, so the range stays half-open.
    Fract,
    unary,
    "std.fract",
    "Fractional part: out = in - floor(in), in [0, 1).",
    fraction
);

stateless_type!(
    /// `std.floor`: rounding down.
    ///
    /// **Formula:** `out[n] = ⌊in[n]⌋` (`libm::floor`; keeps the sign of a zero,
    /// `floor(−0.5) = −1`, `floor(−0) = −0`).
    Floor,
    unary,
    "std.floor",
    "Rounds down: out = floor(in).",
    floor
);

stateless_type!(
    /// `std.round`: rounding to the nearest integer, ties to even.
    ///
    /// **Formula:** `out[n] = roundeven(in[n])` (`libm::roundeven`): `0.5 → 0`,
    /// `1.5 → 2`, `2.5 → 2`, `−2.5 → −2`; a zero keeps its sign
    /// (`round(−0.4) = −0`).
    Round,
    unary,
    "std.round",
    "Rounds to the nearest integer, ties to even.",
    round
);

stateless_type!(
    /// `std.sign`: the sign of a signal.
    ///
    /// **Formula:** `out[n] = 1` if `in[n] > 0`, `−1` if `in[n] < 0`, `+0` for
    /// ±0.
    Sign,
    unary,
    "std.sign",
    "Sign: 1 for positive, -1 for negative, 0 for zero.",
    sign
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};
    use signal_core::DirectPrimitive;

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

    #[test]
    fn the_types_compute_their_formulas() {
        let a = [7.0, -7.0, 1.0, 2.5];
        let b = [2.0, 2.0, 0.0, -0.5];
        assert_eq!(run(Div, &[&a, &b]), [3.5, -3.5, 0.0, -5.0]);
        assert_eq!(run(Mod, &[&a, &b]), [1.0, 1.0, 0.0, 0.0]);
        assert_eq!(run(Neg, &[&a]), [-7.0, 7.0, -1.0, -2.5]);
        assert_eq!(run(Fract, &[&[1.25, -1.25]]), [0.25, 0.75]);
        assert_eq!(run(Floor, &[&[1.5, -1.5]]), [1.0, -2.0]);
        assert_eq!(run(Round, &[&[0.5, 1.5]]), [0.0, 2.0]);
        assert_eq!(run(Sign, &[&[3.0, -0.1]]), [1.0, -1.0]);
    }

    /// AC4: division by ±0 gives +0; a quotient beyond the bound is clamped
    /// to ±1e6 with its sign.
    #[test]
    fn div_by_zero_and_overflow() {
        for a in [1.0, -1.0, 0.0, f64::MAX] {
            assert_eq!(bits(div(a, 0.0)), bits(0.0));
            assert_eq!(bits(div(a, -0.0)), bits(0.0));
        }
        assert_eq!(
            run(
                Div,
                &[
                    &[1e300, -1e300, 1e300, 1.0, -1.0],
                    &[1e-300, 1e-300, -1e-300, 1e-7, 1e-7]
                ]
            ),
            [1e6, -1e6, -1e6, 1e6, -1e6]
        );
        assert_eq!(div(-6.0, 4.0), -1.5);
        // The bound itself is kept.
        assert_eq!(run(Div, &[&[1.0], &[1e-6]]), [1e6]);
    }

    /// Every type's output is within ±1e6, also for inputs beyond it (a
    /// fan-in sum or a connection scale can give those).
    #[test]
    fn outputs_are_clamped_to_the_bound() {
        let big = [3e6, -3e6, 1e300];
        assert_eq!(run(Neg, &[&big]), [-1e6, 1e6, -1e6]);
        assert_eq!(run(Floor, &[&big]), [1e6, -1e6, 1e6]);
        assert_eq!(run(Round, &[&big]), [1e6, -1e6, 1e6]);
        assert_eq!(run(Mod, &[&[-1.0, 1.0, -1.0], &big]), [1e6, -1e6, 1e6]);
    }

    #[test]
    fn neg_flips_the_sign_bit() {
        assert_eq!(bits(neg(0.0)), bits(-0.0));
        assert_eq!(neg(-3.0), 3.0);
    }

    /// AC4: floored remainder, the sign of `b`; b = 0 gives 0.
    #[test]
    fn mod_is_floored() {
        assert_eq!(modulo(5.5, 2.0), 1.5);
        assert_eq!(modulo(-5.5, 2.0), 0.5);
        assert_eq!(modulo(5.5, -2.0), -0.5);
        assert_eq!(modulo(-5.5, -2.0), -1.5);
        assert_eq!(bits(modulo(3.0, 0.0)), bits(0.0));
        assert_eq!(bits(modulo(3.0, -0.0)), bits(0.0));
        // Zero results are +0, also from a negative dividend or modulus.
        assert_eq!(bits(modulo(-4.0, 2.0)), bits(0.0));
        assert_eq!(bits(modulo(4.0, -2.0)), bits(0.0));
        assert_eq!(bits(modulo(-0.0, 1.0)), bits(0.0));
        // r + b rounds to b: the double below it.
        assert_eq!(modulo(-1e-20, 1.0), 1.0f64.next_down());
        assert_eq!(modulo(1e-20, -1.0), (-1.0f64).next_up());
        // a / b would overflow; the remainder is still exact and finite.
        let r = modulo(1e300, 1e-300);
        assert!(r.is_finite() && (0.0..1e-300).contains(&r), "{r}");
        let r = modulo(-f64::MAX, 3.0);
        assert!(r.is_finite() && (0.0..3.0).contains(&r), "{r}");
    }

    #[test]
    fn fract_stays_below_one() {
        assert_eq!(fraction(2.75), 0.75);
        assert_eq!(fraction(-0.25), 0.75);
        assert_eq!(bits(fraction(-3.0)), bits(0.0));
        assert_eq!(fraction(-1e-20), 1.0f64.next_down());
        assert_eq!(fraction(1e300), 0.0);
    }

    #[test]
    fn floor_keeps_signed_zero() {
        assert_eq!(floor(-0.5), -1.0);
        assert_eq!(bits(floor(-0.0)), bits(-0.0));
        assert_eq!(floor(2.999), 2.0);
    }

    /// AC4: half to even.
    #[test]
    fn round_ties_to_even() {
        for (x, y) in [
            (0.5, 0.0),
            (1.5, 2.0),
            (2.5, 2.0),
            (-0.5, -0.0),
            (-1.5, -2.0),
            (-2.5, -2.0),
            (2.4, 2.0),
            (2.6, 3.0),
        ] {
            assert_eq!(bits(round(x)), bits(y), "{x}");
        }
        assert_eq!(bits(round(-0.4)), bits(-0.0));
    }

    /// AC4: −1, 0, 1; both zeros give +0.
    #[test]
    fn sign_is_minus_one_zero_or_one() {
        assert_eq!(sign(1e-300), 1.0);
        assert_eq!(sign(-1e-300), -1.0);
        assert_eq!(bits(sign(0.0)), bits(0.0));
        assert_eq!(bits(sign(-0.0)), bits(0.0));
    }
}
