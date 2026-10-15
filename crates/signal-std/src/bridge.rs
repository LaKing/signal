//! The event bridge into divided-rate types (`docs/module-contract.md`,
//! "Events into divided-rate types"; operator decisions operator-request:007,
//! :017, :014 and :020).
//!
//! A type above divisor 1 cannot have event ports (`docs/semantics.md` §16).
//! [`Count`](crate::Count) runs at divisor 1, takes the events, and outputs
//! a **reading** of [`BRIDGE_ROWS`] elements, each an integer in
//! [0, 2¹⁹), most significant digit first:
//!
//! ```text
//! rows  0–3   stamp        64 bits drawn from the instance's random stream
//! rows  4–6   generation   the plan version it was created for (PrepareContext)
//! rows  7–9   age          frames it has run, this one included
//! rows 10–12  count        events since it was created
//! rows 13–18  sum          Σ q(v) mod 2¹¹⁴, two's complement
//! ```
//!
//! `q(v)` is the event's value rounded to a multiple of 2⁻³² (ties to even),
//! limited to ±2⁶⁴; so every element stays far inside the engine's fault
//! guard whatever the values. Age, count and sum only grow, and a suspended
//! counter's held reading is the same reading, so a consumer that applies
//! the growth of one counter since the reading it last applied ([`Since`])
//! never applies an event twice, however long either is suspended.
//!
//! A bridge input takes **one** connection, at scale 1, from one
//! `std.count.window`; several sources go into the counter's event input
//! instead. Anything else is misuse: rows that are not integers in range
//! (a scale other than 1 usually gives these, as does any other signal)
//! apply nothing.
//!
//! The consumers form their effect without ever forming S alone, so an
//! effect that binary64 can hold is never lost to an intermediate overflow
//! ([`effect`], [`mean`]). With values limited to ±2⁶⁴ (|S| < 2⁸¹) and the
//! consumers' factors in their ranges, no effect reaches `f64::MAX`; one
//! beyond the engine's fault guard (10⁶) faults the consumer.

/// Elements of a reading.
pub const BRIDGE_ROWS: usize = 19;

/// Bits per element: every element is below 2¹⁹ (the fault guard is 1e6).
const DIGIT_BITS: u32 = 19;
const STAMP_ROWS: usize = 4;
const FIELD_ROWS: usize = 3;
const SUM_ROWS: usize = 6;
/// The largest generation, age and count a reading carries: 2⁵⁷ − 1.
const FIELD_MAX: u64 = (1 << (DIGIT_BITS * FIELD_ROWS as u32)) - 1;
/// The sum is kept modulo 2¹¹⁴.
const SUM_MASK: u128 = (1 << (DIGIT_BITS * SUM_ROWS as u32)) - 1;
/// Values are summed in units of 2⁻³².
pub(crate) const RESOLUTION_EXPONENT: i32 = 32;
/// Values are limited to ±2⁶⁴.
const VALUE_LIMIT: f64 = 18_446_744_073_709_551_616.0;

/// `v` in units of 2⁻³², rounded to nearest (ties to even), limited to
/// ±2⁶⁴; a non-finite value is 0.
pub(crate) fn quantize(v: f64) -> i128 {
    if !v.is_finite() {
        return 0;
    }
    // Exact: |v| ≤ 2⁶⁴, so the result is below 2⁹⁶ in magnitude.
    libm::rint(libm::ldexp(
        v.clamp(-VALUE_LIMIT, VALUE_LIMIT),
        RESOLUTION_EXPONENT,
    )) as i128
}

/// What one counter says at one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Reading {
    pub stamp: u64,
    pub generation: u64,
    pub age: u64,
    pub n: u64,
    /// Σ q(v) mod 2¹¹⁴.
    pub sum: u128,
}

impl Reading {
    /// Adds an event of value `v`.
    pub(crate) fn count(&mut self, v: f64) {
        self.n = self.n.saturating_add(1).min(FIELD_MAX);
        self.sum = self.sum.wrapping_add(quantize(v) as u128) & SUM_MASK;
    }

    /// The reading's elements.
    pub(crate) fn encode(&self) -> [f64; BRIDGE_ROWS] {
        let mut rows = [0.0; BRIDGE_ROWS];
        let fields = [
            (u128::from(self.stamp), STAMP_ROWS),
            (u128::from(self.generation.min(FIELD_MAX)), FIELD_ROWS),
            (u128::from(self.age.min(FIELD_MAX)), FIELD_ROWS),
            (u128::from(self.n.min(FIELD_MAX)), FIELD_ROWS),
            (self.sum & SUM_MASK, SUM_ROWS),
        ];
        let mut row = 0;
        for (value, width) in fields {
            for k in (0..width).rev() {
                let digit = (value >> (DIGIT_BITS as usize * k)) & ((1 << DIGIT_BITS) - 1);
                if let Some(out) = rows.get_mut(row) {
                    *out = digit as f64;
                }
                row += 1;
            }
        }
        rows
    }

    /// Reads a reading from its elements: `None` unless every one is an
    /// integer in [0, 2¹⁹) and the stamp fits 64 bits.
    pub(crate) fn decode(rows: [f64; BRIDGE_ROWS]) -> Option<Reading> {
        let radix = f64::from(1u32 << DIGIT_BITS);
        let mut digits = rows.iter();
        let mut field = |width: usize| -> Option<u128> {
            let mut value = 0u128;
            for _ in 0..width {
                let x = *digits.next()?;
                if !(x.is_finite() && (0.0..radix).contains(&x) && libm::floor(x) == x) {
                    return None;
                }
                value = (value << DIGIT_BITS) | x as u128;
            }
            Some(value)
        };
        let stamp = u64::try_from(field(STAMP_ROWS)?).ok()?;
        let generation = field(FIELD_ROWS)? as u64;
        let age = field(FIELD_ROWS)? as u64;
        let n = field(FIELD_ROWS)? as u64;
        let sum = field(SUM_ROWS)?;
        Some(Reading {
            stamp,
            generation,
            age,
            n,
            sum,
        })
    }
}

/// The reading at tick `t` of a call of `ticks` ticks, from `rows`
/// ([`BRIDGE_ROWS`] rows of `ticks` values, as `Inputs::rows` gives them).
fn at_tick(rows: &[f64], ticks: usize, t: usize) -> Option<Reading> {
    let mut reading = [0.0; BRIDGE_ROWS];
    for (r, x) in reading.iter_mut().enumerate() {
        *x = rows.get(r * ticks + t).copied().unwrap_or(0.0);
    }
    Reading::decode(reading)
}

/// What a consumer applied last on one bridge input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Last {
    /// The input read no counter (all zero: unconnected, or removed).
    NoCounter,
    Counter(Reading),
}

/// One bridge input of a consumer: the reading it applied last, so that at
/// each tick it applies exactly what the same counter counted since.
///
/// 1. Rows that are no reading apply nothing; what was applied last stays.
/// 2. No counter (age 0) applies nothing and is remembered, so a counter
///    connected later, the same one included, is met afresh (rule 5).
/// 3. The same counter (same stamp and generation, age and count not
///    lower): its growth since the last reading.
/// 4. The consumer's first tick: the whole reading if it is the counter's
///    first frame (age 1, the start of a run), else nothing (no history).
/// 5. Any other counter: nothing; it is the baseline from now on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Since {
    last: Option<Last>,
}

impl Since {
    /// What to apply at tick `t` of a call of `ticks` ticks.
    pub(crate) fn take(&mut self, rows: &[f64], ticks: usize, t: usize) -> Window {
        let Some(now) = at_tick(rows, ticks, t) else {
            return Window::EMPTY;
        };
        if now.age == 0 {
            self.last = Some(Last::NoCounter);
            return Window::EMPTY;
        }
        let base = match self.last {
            Some(Last::Counter(last))
                if last.stamp == now.stamp
                    && last.generation == now.generation
                    && now.age >= last.age
                    && now.n >= last.n =>
            {
                Some(last)
            }
            None if now.age == 1 => Some(Reading::default()),
            _ => None,
        };
        self.last = Some(Last::Counter(now));
        base.map_or(Window::EMPTY, |base| growth(base, now))
    }
}

/// The events of `now` since `base`, one counter's two readings.
fn growth(base: Reading, now: Reading) -> Window {
    let n = now.n - base.n;
    // The sum's difference modulo 2¹¹⁴, as a signed number.
    let d = now.sum.wrapping_sub(base.sum) & SUM_MASK;
    let d = ((d << (128 - 114)) as i128) >> (128 - 114);
    if d == 0 {
        return Window { n, ..Window::EMPTY };
    }
    // One rounding (exact while |d| < 2⁵³), then exact scaling.
    let (m, e) = libm::frexp(d as f64);
    Window {
        n,
        m,
        e: e - RESOLUTION_EXPONENT,
    }
}

/// What a consumer applies at a tick.
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

    fn reading(stamp: u64, generation: u64, age: u64, values: &[f64]) -> Reading {
        let mut r = Reading {
            stamp,
            generation,
            age,
            ..Reading::default()
        };
        for &v in values {
            r.count(v);
        }
        r
    }

    /// One tick of `since` reading `r` (one call of one tick).
    fn take(since: &mut Since, r: Reading) -> (u64, f64) {
        let w = since.take(&r.encode(), 1, 0);
        (w.n, effect(w, 1.0))
    }

    /// Finding auditor-1-005: an effect binary64 holds is formed without
    /// overflow; only an effect itself beyond `f64::MAX` is ∞.
    #[test]
    fn effects_are_formed_before_decoding() {
        let (m1, e1) = libm::frexp(1e308);
        let w = Window {
            n: 2,
            m: m1,
            e: e1 + 1,
        };
        assert_eq!(e1, 1024);
        let x = effect(w, 1e-305);
        assert!(x.is_finite());
        assert!((x / 2000.0 - 1.0).abs() <= 3.0 * f64::EPSILON);
        assert_eq!(mean(w, 1.0), 1e308);
        assert_eq!(effect(w, 1.0), f64::INFINITY);
        assert_eq!(effect(w, -1.0), f64::NEG_INFINITY);
    }

    /// Every element is an integer below 2¹⁹ and the reading reads back,
    /// at the extremes of every field.
    #[test]
    fn readings_round_trip_inside_the_guard() {
        let mut r = reading(u64::MAX, FIELD_MAX, FIELD_MAX, &[-VALUE_LIMIT, 0.5]);
        r.n = FIELD_MAX;
        let rows = r.encode();
        assert!(
            rows.iter()
                .all(|x| *x >= 0.0 && *x < 524_288.0 && libm::floor(*x) == *x)
        );
        assert_eq!(Reading::decode(rows), Some(r));
        let one = reading(1, 0, 1, &[1.0]);
        assert_eq!(Reading::decode(one.encode()), Some(one));
    }

    /// Values: multiples of 2⁻³² exactly, ties to even, the ±2⁶⁴ limit,
    /// non-finite as 0; below 2⁻³³ adds 0.
    #[test]
    fn values_are_quantized_at_the_documented_resolution() {
        assert_eq!(quantize(1.0), 1 << 32);
        assert_eq!(quantize(-0.75), -(3 << 30));
        assert_eq!(quantize(libm::ldexp(1.0, -33)), 0);
        assert_eq!(quantize(libm::ldexp(3.0, -33)), 2);
        assert_eq!(quantize(1e308), 1 << 96);
        assert_eq!(quantize(-1e308), -(1 << 96));
        assert_eq!(quantize(f64::NAN), 0);
        assert_eq!(quantize(1e-11), 0);
    }

    /// Rule 3: a counter's growth, once; a held reading applies nothing.
    #[test]
    fn the_same_counter_applies_its_growth_once() {
        let mut since = Since::default();
        assert_eq!(take(&mut since, reading(7, 0, 1, &[2.0])), (1, 2.0));
        let r = reading(7, 0, 801, &[2.0, 3.0, 0.5]);
        assert_eq!(take(&mut since, r), (2, 3.5));
        // Held (the counter suspended), however often.
        for _ in 0..600 {
            assert_eq!(take(&mut since, r), (0, 0.0));
        }
        assert_eq!(
            take(&mut since, reading(7, 0, 900, &[2.0, 3.0, 0.5, 4.0])),
            (1, 4.0)
        );
    }

    /// AC6: non-dyadic values summed across a suspension are exact at 2⁻³²:
    /// the difference is the integer sum of q(v).
    #[test]
    fn sums_across_a_suspension_are_exact() {
        let before = [0.1, -1.0 / 3.0, 600_000.0, 1e-11];
        let after = [0.7, 2e20, -0.3];
        let all: Vec<f64> = before.iter().chain(&after).copied().collect();
        let mut since = Since::default();
        take(&mut since, reading(3, 0, 1, &[]));
        take(&mut since, reading(3, 0, 50, &before));
        for _ in 0..40 {
            take(&mut since, reading(3, 0, 50, &before));
        }
        let w = since.take(&reading(3, 0, 51, &all).encode(), 1, 0);
        let exact: i128 = after.iter().map(|v| quantize(*v)).sum();
        assert_eq!(w.n, 3);
        assert_eq!(
            effect(w, 1.0),
            libm::ldexp(exact as f64, -RESOLUTION_EXPONENT)
        );
    }

    /// The sum wraps modulo 2¹¹⁴; the difference across the wrap is exact.
    #[test]
    fn the_sum_wraps_exactly() {
        let base = Reading {
            stamp: 1,
            age: 5,
            n: 1,
            sum: SUM_MASK - 5,
            ..Reading::default()
        };
        let mut now = base;
        now.age = 6;
        now.count(1.0);
        now.count(-0.5);
        let w = growth(base, now);
        assert_eq!((w.n, effect(w, 1.0)), (2, 0.5));
    }

    /// Rules 4 and 5: a consumer's first tick applies a counter's first
    /// frame (E2) and nothing older; another counter is met afresh,
    /// another generation included (a re-created counter).
    #[test]
    fn a_counter_met_afresh_applies_no_history() {
        let mut since = Since::default();
        assert_eq!(take(&mut since, reading(1, 0, 750, &[1.0])), (0, 0.0));
        assert_eq!(take(&mut since, reading(1, 0, 751, &[1.0, 1.0])), (1, 1.0));
        let mut fresh = Since::default();
        assert_eq!(take(&mut fresh, reading(1, 0, 1, &[100.0])), (1, 100.0));
        // Report 011: one payload 100 and one 900 applied; a re-created
        // counter at the same address (same stamp, generation 2) with 100,
        // 1, 1: nothing, then its growth.
        let mut since = Since::default();
        take(&mut since, reading(5, 0, 1, &[100.0]));
        take(&mut since, reading(5, 0, 100, &[100.0, 900.0]));
        assert_eq!(
            take(&mut since, reading(5, 2, 501, &[100.0, 1.0, 1.0])),
            (0, 0.0)
        );
        assert_eq!(
            take(&mut since, reading(5, 2, 1301, &[100.0, 1.0, 1.0, 1.0])),
            (1, 1.0)
        );
        // Another stamp, or a lower age or count: nothing.
        assert_eq!(take(&mut since, reading(6, 2, 1302, &[5.0])), (0, 0.0));
        assert_eq!(take(&mut since, reading(6, 2, 10, &[5.0, 5.0])), (0, 0.0));
    }

    /// Rules 1 and 2: rows that are no reading keep what was applied last;
    /// no counter is remembered, so a reconnected counter replays nothing.
    #[test]
    fn no_counter_and_garbage_apply_nothing() {
        let mut since = Since::default();
        take(&mut since, reading(9, 0, 1, &[1.0]));
        let garbage = [0.5; BRIDGE_ROWS];
        assert_eq!(since.take(&garbage, 1, 0), Window::EMPTY);
        assert_eq!(take(&mut since, reading(9, 0, 801, &[1.0, 1.0])), (1, 1.0));
        assert_eq!(since.take(&[0.0; BRIDGE_ROWS], 1, 0), Window::EMPTY);
        assert_eq!(take(&mut since, reading(9, 0, 9000, &[1.0; 20])), (0, 0.0));
        assert_eq!(take(&mut since, reading(9, 0, 9800, &[1.0; 21])), (1, 1.0));
        let halved = reading(9, 0, 5, &[1.0]).encode().map(|x| 0.5 * x);
        assert_eq!(Reading::decode(halved), None);
        assert_eq!(Reading::decode([-1.0; BRIDGE_ROWS]), None);
        assert_eq!(Reading::decode([f64::NAN; BRIDGE_ROWS]), None);
    }
}
