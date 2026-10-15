//! The event bridge into divided-rate types (`docs/module-contract.md`,
//! "Events into divided-rate types"; operator decisions operator-request:007
//! and operator-request:017).
//!
//! A type above divisor 1 cannot have event ports (`docs/semantics.md` §16).
//! [`Count`](crate::Count) runs at divisor 1, takes the events, and outputs
//! a **window**: a vector of 4 describing the events of its last
//! [`WINDOW_FRAMES`] frames,
//!
//! ```text
//! window[0] = ⌊n / 2¹⁹⌋     window[1] = n mod 2¹⁹      (n events, exact)
//! window[2] = m             window[3] = e              (their summed value S = m·2^e)
//! ```
//!
//! with `m = 0, e = 0` or `½ ≤ |m| < 1` and `e` an integer, so every element
//! stays far inside the engine's fault guard whatever the values. A type at
//! divisor [`WINDOW_FRAMES`] reads the window on a divisor-1 vector input at
//! its ticks: at a tick it is exactly the events since its previous tick.
//!
//! The encoding is not additive: a bridge input takes **one** connection, at
//! scale 1, from one `std.count.window`. Several sources go into the
//! counter's event input instead. [`decode`] recognises most misuse (two
//! same-sign windows summed give `|m| ≥ 1`, a scale of ½ gives `|m| < ½`) and
//! then reads the input as `n` triggers; opposite-sign windows that cancel to
//! `m = 0`, or a scale such as 1.5 with an even exponent, decode silently
//! wrong. Nothing in the loader checks this.
//!
//! The consumers form their effect without ever forming S alone, so an
//! effect that binary64 can hold is never lost to an intermediate overflow
//! ([`effect`], [`mean`]); an effect beyond `f64::MAX` is ±∞ and faults the
//! consumer through the engine's guard.

use crate::VISUAL_DIVISOR;

/// Rows of a window.
pub(crate) const WINDOW_ROWS: usize = 4;
/// Frames of a window: the divisor of the types that read one.
pub(crate) const WINDOW_FRAMES: usize = VISUAL_DIVISOR as usize;
/// The radix of the count's two elements, 2¹⁹.
pub(crate) const COUNT_RADIX: u64 = 1 << 19;
/// `std.count` scales event values by 2⁻⁶⁴ before summing them, so that no
/// finite sum it can be given overflows; the exponent it outputs adds the 64
/// back.
pub(crate) const SUM_SCALE_EXPONENT: i32 = 64;

/// A decoded window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Window {
    /// Events.
    pub n: u64,
    /// Their summed value is `m·2^e`.
    pub m: f64,
    pub e: i32,
}

impl Window {
    pub const EMPTY: Window = Window { n: 0, m: 0.0, e: 0 };
}

/// The window `std.count` outputs for `n` events whose values, each scaled by
/// 2⁻⁶⁴, sum to `scaled`.
pub(crate) fn encode(n: u64, scaled: f64) -> [f64; WINDOW_ROWS] {
    let hi = (n / COUNT_RADIX) as f64;
    let lo = (n % COUNT_RADIX) as f64;
    if scaled == 0.0 || !scaled.is_finite() {
        // An empty window is (0, 0): an idle counter adds nothing.
        return [hi, lo, 0.0, 0.0];
    }
    let (m, e) = libm::frexp(scaled);
    [hi, lo, m, f64::from(e + SUM_SCALE_EXPONENT)]
}

fn whole(x: f64) -> bool {
    x.is_finite() && x >= 0.0 && libm::floor(x) == x
}

/// Reads a window from its four rows. Something that is not one window at
/// scale 1 (see the module doc) is read as `n` triggers.
pub(crate) fn decode([hi, lo, m, e]: [f64; WINDOW_ROWS]) -> Window {
    let radix = COUNT_RADIX as f64;
    let mantissa = m == 0.0 || (0.5..1.0).contains(&m.abs());
    let exponent = e.is_finite() && libm::floor(e) == e && e.abs() <= 4096.0;
    if whole(hi) && whole(lo) && lo < radix && hi < radix && mantissa && exponent {
        let n = hi as u64 * COUNT_RADIX + lo as u64;
        return match m == 0.0 {
            true => Window { n, ..Window::EMPTY },
            false => Window { n, m, e: e as i32 },
        };
    }
    let part = |x: f64| match x.is_finite() {
        true => libm::floor(x + 0.5).clamp(0.0, radix - 1.0) as u64,
        false => 0,
    };
    let n = part(hi) * COUNT_RADIX + part(lo);
    if n == 0 {
        return Window::EMPTY;
    }
    let (m, e) = libm::frexp(n as f64);
    Window { n, m, e }
}

/// The window's rows at tick `t` of a call of `ticks` ticks, from `rows`
/// (four rows of `ticks` values, as `Inputs::rows` gives them).
pub(crate) fn at_tick(rows: &[f64], ticks: usize, t: usize) -> Window {
    let row = |r: usize| rows.get(r * ticks + t).copied().unwrap_or(0.0);
    decode([row(0), row(1), row(2), row(3)])
}

/// `S × factor`, rounded once in the mantissa and once by `ldexp`: finite
/// whenever the result is.
pub(crate) fn effect(w: Window, factor: f64) -> f64 {
    if w.m == 0.0 || factor == 0.0 || !factor.is_finite() {
        return 0.0;
    }
    let (mf, ef) = libm::frexp(factor);
    let (p, ep) = libm::frexp(w.m * mf);
    libm::ldexp(p, w.e + ef + ep)
}

/// `(S / n) × factor`, the mean value per event times `factor`, formed the
/// same way: finite whenever the result is. 0 without events.
pub(crate) fn mean(w: Window, factor: f64) -> f64 {
    if w.n == 0 || w.m == 0.0 || factor == 0.0 || !factor.is_finite() {
        return 0.0;
    }
    // Exact: n < 2⁵³.
    let (mn, en) = libm::frexp(w.n as f64);
    let (q, eq) = libm::frexp(w.m / mn);
    let (mf, ef) = libm::frexp(factor);
    let (p, ep) = libm::frexp(q * mf);
    libm::ldexp(p, w.e - en + eq + ef + ep)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scaled(sum: f64) -> f64 {
        libm::ldexp(sum, -SUM_SCALE_EXPONENT)
    }

    /// Finding auditor-1-005: 2 × Set(1e308) is (m₁, 1025), which binary64
    /// cannot hold as one number, yet an effect at strength 1e-305
    /// (frexp(1e-305) has exponent −1013) is 2000 and the mean per event is
    /// exactly 1e308. Only an effect that is itself beyond `f64::MAX` is ∞.
    #[test]
    fn effects_are_formed_before_decoding() {
        let big = scaled(1e308) + scaled(1e308);
        let w = decode(encode(2, big));
        let (m1, e1) = libm::frexp(1e308);
        assert_eq!((w.m, w.e), (m1, e1 + 1));
        assert_eq!(e1, 1024);
        assert_eq!(libm::frexp(1e-305).1, -1013);
        let x = effect(w, 1e-305);
        println!("2 × 1e308 × 1e-305 = {x}");
        assert!(x.is_finite());
        assert!((x / 2000.0 - 1.0).abs() <= 3.0 * f64::EPSILON);
        assert_eq!(mean(w, 1.0), 1e308);
        assert_eq!(effect(w, 1.0), f64::INFINITY);
        assert_eq!(effect(w, -1.0), f64::NEG_INFINITY);
    }

    /// The auditor's two Set(600000): exactly 1 200 000, every element
    /// inside the guard.
    #[test]
    fn payloads_beyond_the_guard_are_kept() {
        let rows = encode(2, scaled(600_000.0) + scaled(600_000.0));
        assert!(rows.iter().all(|x| x.is_finite() && x.abs() <= 1e6));
        let w = decode(rows);
        assert_eq!(w.n, 2);
        assert_eq!(effect(w, 1.0), 1_200_000.0);
        assert_eq!(mean(w, 1.0), 600_000.0);
    }

    /// Counts past 2¹⁹ use both elements.
    #[test]
    fn the_count_is_exact_past_2_19() {
        let rows = encode(560_000, scaled(560_000.0));
        assert_eq!(&rows[..2], &[1.0, 35_712.0]);
        let w = decode(rows);
        assert_eq!(w.n, 560_000);
        assert_eq!(effect(w, 1.0), 560_000.0);
    }

    /// Finding auditor-2-empty-window-exponent: an empty window is (0, 0),
    /// so an idle counter summed with an active one decodes to exactly the
    /// active one.
    #[test]
    fn an_idle_window_adds_nothing() {
        let idle = encode(0, 0.0);
        assert_eq!(idle, [0.0; 4]);
        let active = encode(3, scaled(7.5));
        let sum: Vec<f64> = idle.iter().zip(&active).map(|(a, b)| a + b).collect();
        let w = decode([sum[0], sum[1], sum[2], sum[3]]);
        assert_eq!(w, decode(active));
        assert_eq!(effect(w, 1.0), 7.5);
    }

    /// Misuse that is recognised reads as `n` triggers; opposite-sign
    /// windows that cancel decode as S = 0 (documented, undetected).
    #[test]
    fn detected_misuse_decodes_as_triggers() {
        let a = encode(2, scaled(3.0));
        let b = encode(1, scaled(5.0));
        let two: Vec<f64> = a.iter().zip(&b).map(|(a, b)| a + b).collect();
        let w = decode([two[0], two[1], two[2], two[3]]);
        assert_eq!((w.n, effect(w, 1.0)), (3, 3.0));
        let half: Vec<f64> = b.iter().map(|x| 0.5 * x).collect();
        let w = decode([half[0], half[1], half[2], half[3]]);
        assert_eq!(w.n, 1);
        assert_eq!(effect(w, 1.0), 1.0);
        let plus = encode(1, scaled(4.0));
        let minus = encode(1, scaled(-4.0));
        let cancel: Vec<f64> = plus.iter().zip(&minus).map(|(a, b)| a + b).collect();
        let w = decode([cancel[0], cancel[1], cancel[2], cancel[3]]);
        assert_eq!((w.n, effect(w, 1.0)), (2, 0.0));
    }

    /// Anything else that is no window (negative, non-finite) reads as at
    /// most that many triggers, never a panic.
    #[test]
    fn garbage_decodes_to_triggers_or_nothing() {
        assert_eq!(decode([f64::NAN, -1.0, 2.0, 0.5]), Window::EMPTY);
        assert_eq!(decode([0.0, 1.4, 0.0, 0.0]).n, 1);
        assert_eq!(decode([-3.0, -3.0, -3.0, -3.0]), Window::EMPTY);
    }
}
