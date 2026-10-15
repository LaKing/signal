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
//! rows 19–20  check L      Σ rows 0–18, plus 1
//! rows 21–23  check Q      Σ (rows 0–18)²
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
//! instead. The plan refuses anything else on a bridge input when it is built
//! ([`crate::register`], ADR 0024). The check rows are a second line: what
//! still reaches an input decodes only as a whole reading. Rows that are no
//! reading apply nothing. Two exact checks (no modulus) hold for a sum
//! Σ aⱼ·Rⱼ of readings at scales aⱼ only if Σ aⱼ = 1 (L, affine) and
//! Σⱼ<ₖ aⱼaₖ·|Rⱼ − Rₖ|² = 0 (Q, given Σ aⱼ = 1). So one reading at a scale
//! other than 1, several readings at scale 1, any two readings at any
//! scales and any readings at scales ≥ 0 never decode except as a genuine
//! reading; three or more readings with a negative scale can, in principle,
//! as can any vector forged at scale 1, which is why the plan refuses them.
//!
//! The sum of a window is exact as an integer (in units of 2⁻³²) while the
//! difference can be told modulo 2¹¹⁴: below 2¹⁷ events since the reading
//! applied last (each |q(v)| ≤ 2⁹⁶ units, so the true difference stays below
//! 2¹¹³). A window of more events, or of a saturated count, may have wrapped:
//! it applies nothing and is counted ([`Since::lost`], the consumers' `lost`
//! output). The applied sum is **deterministic**: the integer is converted to
//! binary64 once, exact up to 2⁵³ units (about 2²¹ in value) and rounded to
//! nearest beyond. The consumers form their effect without ever forming S
//! alone, so an effect that binary64 can hold is never lost to an
//! intermediate overflow ([`effect`], [`mean`]). With values limited to ±2⁶⁴
//! and the consumers' factors in their ranges, no effect reaches `f64::MAX`;
//! one beyond the engine's fault guard (10⁶) faults the consumer.

/// Elements of a reading.
pub const BRIDGE_ROWS: usize = DATA_ROWS + L_ROWS + Q_ROWS;
/// The data rows (stamp to sum).
const DATA_ROWS: usize = 19;
/// Check L: Σ data rows + 1 (below 2²⁴).
const L_ROWS: usize = 2;
/// Check Q: Σ (data rows)² (below 19·2³⁸ < 2⁴³).
const Q_ROWS: usize = 3;
/// Events between two applied readings within which the sum's difference is
/// exact modulo 2¹¹⁴: below 2¹⁷.
const EXACT_EVENTS: u64 = 1 << 17;
/// The most rejected windows [`Since::lost`] counts, and the most a
/// consumer's `lost` output says ([`lost`]): 2¹⁹ − 1, inside the fault guard.
pub(crate) const LOST_MAX: u32 = (1 << 19) - 1;

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

    /// The reading's elements: its data rows, then its checks.
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
            put(&mut rows, &mut row, value, width);
        }
        let (l, q) = checks(&rows);
        put(&mut rows, &mut row, l, L_ROWS);
        put(&mut rows, &mut row, q, Q_ROWS);
        rows
    }

    /// Reads a reading from its elements: `None` unless every one is an
    /// integer in [0, 2¹⁹), the stamp fits 64 bits and both checks hold.
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
        if (field(L_ROWS)?, field(Q_ROWS)?) != checks(&rows) {
            return None;
        }
        Some(Reading {
            stamp,
            generation,
            age,
            n,
            sum,
        })
    }
}

/// Writes `value` as `width` digits, most significant first, from `row`.
fn put(rows: &mut [f64; BRIDGE_ROWS], row: &mut usize, value: u128, width: usize) {
    for k in (0..width).rev() {
        let digit = (value >> (DIGIT_BITS as usize * k)) & ((1 << DIGIT_BITS) - 1);
        if let Some(out) = rows.get_mut(*row) {
            *out = digit as f64;
        }
        *row += 1;
    }
}

/// The checks of the data rows: (Σ r + 1, Σ r²), exact. Rows are taken as
/// they are only when they are integers in range ([`Reading::decode`] checks
/// that first; [`Reading::encode`] writes nothing else).
fn checks(rows: &[f64; BRIDGE_ROWS]) -> (u128, u128) {
    rows.iter().take(DATA_ROWS).fold((1, 0), |(l, q), &x| {
        let r = x as u128;
        (l + r, q + r * r)
    })
}

/// The rows at tick `t` of a call of `ticks` ticks, from `rows`
/// ([`BRIDGE_ROWS`] rows of `ticks` values, as `Inputs::rows` gives them).
fn at_tick(rows: &[f64], ticks: usize, t: usize) -> [f64; BRIDGE_ROWS] {
    let mut reading = [0.0; BRIDGE_ROWS];
    for (r, x) in reading.iter_mut().enumerate() {
        *x = rows.get(r * ticks + t).copied().unwrap_or(0.0);
    }
    reading
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
/// 2. No counter (every row 0, or age 0) applies nothing and is remembered,
///    so a counter connected later, the same one included, is met afresh
///    (rule 5). A disconnection the consumer never reads (it and the
///    reconnection both fall between two of its ticks, or while it is
///    suspended) is not seen: the same counter's growth is applied once.
/// 3. The same counter (same stamp and generation, age and count not
///    lower): its growth since the last reading, unless it may have wrapped
///    (2¹⁷ events or more, or a saturated count): then nothing, counted in
///    [`lost`](Since::lost).
/// 4. The consumer's first tick: the whole reading if it is the counter's
///    first frame (age 1, the start of a run), else nothing (no history).
/// 5. Any other counter: nothing; it is the baseline from now on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Since {
    last: Option<Last>,
    lost: u32,
}

impl Since {
    /// What to apply at tick `t` of a call of `ticks` ticks.
    pub(crate) fn take(&mut self, rows: &[f64], ticks: usize, t: usize) -> Window {
        let rows = at_tick(rows, ticks, t);
        if rows.iter().all(|x| *x == 0.0) {
            self.last = Some(Last::NoCounter);
            return Window::EMPTY;
        }
        let Some(now) = Reading::decode(rows) else {
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
        let Some(base) = base else {
            return Window::EMPTY;
        };
        growth(base, now).unwrap_or_else(|| {
            self.lost = (self.lost + 1).min(LOST_MAX);
            Window::EMPTY
        })
    }

    /// The windows not applied because their sum may have wrapped (at most
    /// 2¹⁹ − 1): the consumer's `lost` output.
    pub(crate) fn lost(&self) -> u32 {
        self.lost
    }

    /// Test hook: an input that has already counted `lost` windows.
    #[cfg(test)]
    pub(crate) fn with_lost(lost: u32) -> Since {
        Since {
            lost,
            ..Since::default()
        }
    }
}

/// A consumer's `lost` output: what its bridge inputs counted, together, at
/// most [`LOST_MAX`] (inside the fault guard whatever they sum to).
pub(crate) fn lost(inputs: &[&Since]) -> f64 {
    let sum = inputs
        .iter()
        .fold(0u32, |sum, s| sum.saturating_add(s.lost()));
    f64::from(sum.min(LOST_MAX))
}

/// The events of `now` since `base`, one counter's two readings; `None` when
/// the sum's difference may have wrapped: 2¹⁷ events or more (each
/// |q(v)| ≤ 2⁹⁶ units, so fewer keep it below 2¹¹³), or a saturated count
/// (which no longer counts the events).
fn growth(base: Reading, now: Reading) -> Option<Window> {
    let n = now.n - base.n;
    if n >= EXACT_EVENTS || now.n >= FIELD_MAX {
        return None;
    }
    // The sum's difference modulo 2¹¹⁴, as a signed number: exact here.
    let d = now.sum.wrapping_sub(base.sum) & SUM_MASK;
    let d = ((d << (128 - 114)) as i128) >> (128 - 114);
    if d == 0 {
        return Some(Window { n, ..Window::EMPTY });
    }
    // One rounding (exact while |d| < 2⁵³), then exact scaling.
    let (m, e) = libm::frexp(d as f64);
    Some(Window {
        n,
        m,
        e: e - RESOLUTION_EXPONENT,
    })
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
    // Exact: a reading's count can reach 2⁵⁷ − 1, beyond 2⁵³ where `as f64`
    // rounds, but a window [`growth`] applies has n < 2¹⁷.
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
    /// non-finite as 0; at most 2⁻³³ in magnitude adds 0 (exactly 2⁻³³ is a
    /// tie, rounded to even).
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
        let w = growth(base, now).unwrap();
        assert_eq!((w.n, effect(w, 1.0)), (2, 0.5));
    }

    /// M2.8-9 AC7: below 2¹⁷ events since the applied reading the difference
    /// is exact, whatever the values; at 2¹⁷ events of +2⁶⁴ it would read
    /// as −2⁸¹ modulo 2¹¹⁴, so nothing is applied and the window is counted
    /// lost. Also the negative side and a saturated count.
    #[test]
    fn a_window_that_may_have_wrapped_applies_nothing_and_is_counted() {
        let at = |n: u64, value: i128, age: u64| Reading {
            stamp: 3,
            age,
            n,
            sum: (value * i128::from(n)) as u128 & SUM_MASK,
            ..Reading::default()
        };
        for sign in [1i128, -1] {
            let unit = sign << 96;
            let mut since = Since::default();
            // The consumer's first tick, not the counter's first frame.
            assert_eq!(take(&mut since, at(0, unit, 5)), (0, 0.0));
            let below = EXACT_EVENTS - 1;
            let w = since.take(&at(below, unit, 6).encode(), 1, 0);
            assert_eq!(w.n, below);
            assert_eq!(
                effect(w, 1.0),
                sign as f64 * libm::ldexp(below as f64, 64),
                "exact below the bound"
            );
            assert_eq!(since.lost(), 0);
            // 2¹⁷ more events: ±2¹¹³ units both read as −2¹¹³, so the sign
            // cannot be told (+2¹¹³ would be applied as −2⁸¹).
            let now = at(below + EXACT_EVENTS, unit, 7);
            let d = now.sum.wrapping_sub(at(below, unit, 6).sum) & SUM_MASK;
            assert_eq!(((d << 14) as i128) >> 14, -1 << 113);
            assert_eq!(since.take(&now.encode(), 1, 0), Window::EMPTY);
            assert_eq!(since.lost(), 1);
            // The rejected reading is the baseline: the next growth applies.
            let w = since.take(&at(below + EXACT_EVENTS + 1, unit, 8).encode(), 1, 0);
            assert_eq!((w.n, since.lost()), (1, 1));
        }
        // A saturated count no longer counts the events.
        let mut since = Since::default();
        take(&mut since, at(FIELD_MAX - 1, 0, 5));
        assert_eq!(take(&mut since, at(FIELD_MAX, 0, 6)), (0, 0.0));
        assert_eq!(since.lost(), 1);
    }

    /// Seeded readings, digits small (they doubled into a valid reading
    /// before the checks) and large.
    fn readings() -> Vec<Reading> {
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let mut all = vec![
            reading(2, 1, 1, &[]),
            reading(1, 1, 1, &[]),
            reading(7, 0, 801, &[2.0, 3.0, 0.5]),
        ];
        for k in 0..64 {
            let small = k % 2 == 0;
            let mut r = Reading {
                stamp: if small { next() >> 40 } else { next() },
                generation: next() % 4,
                age: 1 + next() % if small { 1 << 17 } else { FIELD_MAX },
                ..Reading::default()
            };
            for _ in 0..(next() % 5) {
                r.count(if small {
                    1.0
                } else {
                    (next() >> 11) as f64 * 1e-3
                });
            }
            all.push(r);
        }
        all
    }

    fn combine(parts: &[(f64, Reading)]) -> [f64; BRIDGE_ROWS] {
        let mut rows = [0.0; BRIDGE_ROWS];
        for (a, r) in parts {
            for (out, x) in rows.iter_mut().zip(r.encode()) {
                *out += a * x;
            }
        }
        rows
    }

    /// M2.8-9 AC6 (defence in depth; the plan refuses these connections):
    /// a reading at a scale other than 1, several at scale 1, any two at any
    /// scales (2A − B, ½A + ½B) and any at scales ≥ 0 decode only as a
    /// genuine reading, never as a doubled or mixed one.
    #[test]
    fn scaled_or_summed_readings_never_decode() {
        let all = readings();
        for &r in &all {
            for scale in [2.0, 3.0, 0.5, 0.25, -1.0, 1.0 + libm::ldexp(1.0, -19)] {
                assert_eq!(
                    Reading::decode(combine(&[(scale, r)])),
                    None,
                    "{scale}·{r:?}"
                );
            }
            for copies in [2.0, 3.0] {
                assert_eq!(Reading::decode(combine(&[(copies, r)])), None);
            }
            assert_eq!(Reading::decode(combine(&[(1.0, r)])), Some(r));
        }
        for (i, &a) in all.iter().enumerate() {
            for &b in all.iter().skip(i + 1) {
                for parts in [
                    [(1.0, a), (1.0, b)],
                    [(2.0, a), (-1.0, b)],
                    [(-1.0, a), (2.0, b)],
                    [(0.5, a), (0.5, b)],
                    [(0.75, a), (0.25, b)],
                ] {
                    assert_eq!(Reading::decode(combine(&parts)), None, "{parts:?}");
                }
            }
        }
        // Auditor-1's 2A − B: a doubled first frame (n 2, sum 2) before the
        // checks.
        let a = reading(2, 1, 1, &[1.0]);
        let b = reading(1, 1, 1, &[]);
        assert_eq!(Reading::decode(combine(&[(2.0, a), (-1.0, b)])), None);
        let c = all[3];
        assert_eq!(
            Reading::decode(combine(&[(0.5, a), (0.25, b), (0.25, c)])),
            None
        );
        // What the checks cannot tell (ADR 0024: the plan refuses the
        // connections): R1 + R2 − R3 with orthogonal differences decodes.
        let r1 = reading(2, 1, 1, &[1.0]);
        let r2 = reading(1, 1, 1, &[1.0, 1.0]);
        let r3 = reading(1, 1, 1, &[1.0]);
        assert_eq!(
            Reading::decode(combine(&[(1.0, r1), (1.0, r2), (-1.0, r3)])),
            Some(reading(2, 1, 1, &[1.0, 1.0]))
        );
    }

    /// D4: a disconnection the consumer never reads (it and the reconnection
    /// between two of its ticks) is not seen: the events counted meanwhile
    /// are applied once.
    #[test]
    fn a_reconnection_between_ticks_applies_once() {
        let mut since = Since::default();
        assert_eq!(take(&mut since, reading(4, 0, 1, &[1.0])), (1, 1.0));
        assert_eq!(
            take(&mut since, reading(4, 0, 801, &[1.0, 2.0, 3.0])),
            (2, 5.0)
        );
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
