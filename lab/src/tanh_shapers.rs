//! The M3 "ADAA versus oversampling" experiment (`experiments/m3-tanh-aliasing/`):
//! candidate replacements for `std.tanh_shaper`, each with the same ports and
//! parameter, so a project can swap one for another by its type name alone.
//!
//! | Type | Antialiasing | Delay (frames) |
//! |---|---|---|
//! | `lab.tanh_adaa2` | second-order ADAA at the frame rate | 1 |
//! | `lab.tanh_os2_adaa1` | ×2 oversampling, first-order ADAA at 2·sr | [`latency_frames`]`(1, true)` = 65.25 |
//! | `lab.tanh_os4_adaa1` | ×4 oversampling, first-order ADAA at 4·sr | `latency_frames(2, true)` = 71.625 |
//! | `lab.tanh_os8_adaa1` | ×8 oversampling, first-order ADAA at 8·sr | `latency_frames(3, true)` = 73.8125 |
//! | `lab.tanh_os8` | ×8 oversampling, plain `tanh` at 8·sr | `latency_frames(3, false)` = 73.75 |
//!
//! **Ports and parameter** (as `std.tanh_shaper`): `in` (signal in, audio,
//! unbounded; unconnected reads 0), `out` (signal out, audio), `drive` (gain,
//! `[0.1, 20]`, default 1, combine `add`). Feedthrough: direct.
//!
//! **Delay.** Each candidate delays its input by the frames above, a
//! property of its filters (linear phase: the delay is the same at every
//! frequency of the passband). A direct type cannot declare a latency (the
//! contract's `BlockLatency` is a latent state-output type's), and the engine
//! compensates nothing, so, as for `std.tanh_shaper`'s half frame, the delay
//! is documented here and in each descriptor's `doc`; a parallel dry path that
//! must stay aligned needs a `std.delay` of the same length.
//!
//! **Oversampling.** `u = drive·in` is computed at the frame rate and raised
//! to *R* = 2^*S* times the rate by *S* cascaded ×2 stages, shaped there, and
//! brought back by the same stages in reverse. Every stage is a linear-phase
//! FIR (Kaiser-windowed sinc), used as a polyphase interpolator (gain 2) on
//! the way up and as a decimator on the way down:
//!
//! - stage 1 (sr ↔ 2·sr), [`STAGE1`]: passband to 0.40·sr, stopband from
//!   0.50·sr, so nothing above the frame rate's Nyquist frequency folds back
//!   into it; this stage sets the passband (19.2 kHz at 48 kHz, 38.4 kHz at
//!   96 kHz) and most of the delay;
//! - stages 2 and 3 (2·sr ↔ 4·sr, 4·sr ↔ 8·sr), [`STAGE2`], [`STAGE3`]:
//!   half-band filters (every other tap 0), stopband from the band whose
//!   content would fold onto 0 … 0.5·sr at the next rate down.
//!
//! The coefficients are designed in `prepare`, in normalized frequency, so
//! they are the same at every sample rate ([`design`]). The work per frame is
//! fixed: no branch depends on the signal except ADAA's ill-conditioning
//! switch.

use libm::{exp, fabs, log1p, sin, sqrt, tanh};
use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs,
    ParameterDescriptor, PortDescriptor, PortKind, PortShape, PrepareContext, Range, RateDivisor,
    Reproducibility,
};

// ---------------------------------------------------------------------------
// Descriptors

const IN: PortDescriptor = PortDescriptor {
    name: "in",
    direction: Direction::In,
    shape: PortShape::Signal,
    rate: RateDivisor::ONE,
    kind: PortKind::Audio,
    unit: None,
    range: None,
};

/// Unbounded, unlike `std.tanh_shaper`'s `[−1, 1]`: the decimation filter's
/// ripple lets a clipped (square-like) output overshoot 1 slightly
/// (Gibbs), as any band-limited square does.
const OUT: PortDescriptor = PortDescriptor {
    name: "out",
    direction: Direction::Out,
    shape: PortShape::Signal,
    rate: RateDivisor::ONE,
    kind: PortKind::Audio,
    unit: None,
    range: None,
};

const ADAA2_OUT: PortDescriptor = PortDescriptor {
    range: Some(Range {
        min: -1.0,
        max: 1.0,
    }),
    ..OUT
};

const PORTS: [PortDescriptor; 2] = [IN, OUT];
const ADAA2_PORTS: [PortDescriptor; 2] = [IN, ADAA2_OUT];

const DRIVE: ParameterDescriptor = ParameterDescriptor {
    name: "drive",
    kind: PortKind::Gain,
    rate: RateDivisor::ONE,
    unit: None,
    range: Range {
        min: 0.1,
        max: 20.0,
    },
    default: 1.0,
    combine: Combine::Add,
};

const fn descriptor(
    type_name: &'static str,
    doc: &'static str,
    ports: &'static [PortDescriptor],
) -> ModuleDescriptor {
    ModuleDescriptor {
        type_name,
        doc,
        ports,
        parameters: &[DRIVE],
        feedthrough: Feedthrough::Direct,
        sink: None,
        latency: None,
        reproducibility: Reproducibility::BitExact,
        category: Some("filter"),
    }
}

// ---------------------------------------------------------------------------
// tanh and its antiderivatives

/// `ln 2`.
const LN_2: f64 = core::f64::consts::LN_2;
/// π²/12 = −Li₂(−1).
const PI2_12: f64 = core::f64::consts::PI * core::f64::consts::PI / 12.0;

/// Below this `|u[n] − u[n−1]|` first-order ADAA uses its limit (as
/// `std.tanh_shaper`).
const ADAA1_EPSILON: f64 = 1e-6;
/// Below this step the mean of `ln cosh` over a step (second-order ADAA's
/// inner quotient) is integrated by three-point Gauss–Legendre instead of
/// as a difference quotient of F₂: no cancellation, and an error near
/// h⁶·max|F₁⁽⁶⁾|/2 016 000 < 1e-17 for steps h below it.
const ADAA2_QUADRATURE_STEP: f64 = 0.01;
/// √(3/5): the outer Gauss–Legendre nodes on [−1, 1].
const GAUSS_NODE: f64 = 0.774_596_669_241_483_4;
/// Below this `|u[n] − u[n−2]|` (and then `|ū − u[n−1]|`) second-order
/// ADAA switches to its limit forms.
const ADAA2_OUTER_EPSILON: f64 = 1e-4;

/// `ln(1 + e^(−2|u|))`.
fn softplus_term(u: f64) -> f64 {
    log1p(exp(-2.0 * fabs(u)))
}

/// F₁(u) = ln cosh u = |u| − ln 2 + ln(1 + e^(−2|u|)), the antiderivative
/// of `tanh`, without overflow.
fn ln_cosh(u: f64) -> f64 {
    fabs(u) - LN_2 + softplus_term(u)
}

/// First-order ADAA of `tanh` on the step from `previous` to `u`: exactly
/// `std.tanh_shaper`'s formula (`docs/dsp-conventions.md`).
pub fn adaa1(u: f64, previous: f64) -> f64 {
    let step = u - previous;
    if fabs(step) < ADAA1_EPSILON {
        tanh((u + previous) / 2.0)
    } else {
        ((fabs(u) - fabs(previous)) + (softplus_term(u) - softplus_term(previous))) / step
    }
}

/// Li₂(w) = Σ w^k/k² for 0 ≤ w ≤ 1/2: 52 terms, so the first left out is
/// below 2⁻⁵³/53² of the sum. Fixed work.
fn li2_small(w: f64) -> f64 {
    let mut power = w;
    let mut sum = 0.0;
    for k in 1..=52_u32 {
        let k = f64::from(k);
        sum += power / (k * k);
        power *= w;
    }
    sum
}

/// L(a) = Li₂(−e^(−2a)) for a ≥ 0, in [−π²/12, 0]. With z = −e^(−2a) ∈
/// [−1, 0) and w = z/(z − 1) ∈ (0, 1/2]: Li₂(z) = −Li₂(w) − ½·ln²(1 − z).
fn li2_term(a: f64) -> f64 {
    let e = exp(-2.0 * a);
    let w = e / (1.0 + e);
    let l = log1p(e);
    -li2_small(w) - 0.5 * l * l
}

/// G(a) = F₂(a) for a ≥ 0, where F₂ is the antiderivative of F₁ with
/// F₂(0) = 0: G(a) = a²/2 − a·ln 2 + (L(a) + π²/12)/2. F₂ is odd.
fn f2_positive(a: f64, l: f64) -> f64 {
    0.5 * a * a - a * LN_2 + 0.5 * (l + PI2_12)
}

/// A value with its L(|u|), so second-order ADAA computes L once per frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Point {
    u: f64,
    l: f64,
}

impl Point {
    fn new(u: f64) -> Point {
        Point {
            u,
            l: li2_term(fabs(u)),
        }
    }

    /// The point u = 0: L(0) = Li₂(−1) = −π²/12.
    const ZERO: Point = Point { u: 0.0, l: -PI2_12 };
}

/// The mean of F₁ over the segment from `b` to `a`: (F₂(a) − F₂(b))/(a − b).
/// For a step below [`ADAA2_QUADRATURE_STEP`] it is integrated directly
/// (three-point Gauss–Legendre on F₁, weights 5/18, 8/18, 5/18), so small
/// steps lose nothing to cancellation. Otherwise, for a and b of one sign it
/// is summed as (a + b)/2 − ln 2 + (L(a) − L(b))/(2(a − b)) (on |a|, |b|),
/// so the large quadratic part cancels exactly; for opposite signs the
/// numerator adds magnitudes and nothing cancels.
fn mean_ln_cosh(a: Point, b: Point) -> f64 {
    let step = a.u - b.u;
    if fabs(step) < ADAA2_QUADRATURE_STEP {
        let mid = (a.u + b.u) / 2.0;
        let offset = GAUSS_NODE * step / 2.0;
        return (5.0 * ln_cosh(mid - offset) + 8.0 * ln_cosh(mid) + 5.0 * ln_cosh(mid + offset))
            / 18.0;
    }
    if (a.u >= 0.0) == (b.u >= 0.0) {
        let (pa, pb) = (fabs(a.u), fabs(b.u));
        let quotient = (a.l - b.l) / (2.0 * (pa - pb));
        (pa + pb) / 2.0 - LN_2 + quotient
    } else {
        let f2 = |p: Point| f2_positive(fabs(p.u), p.l).copysign(p.u);
        (f2(a) - f2(b)) / step
    }
}

/// Second-order ADAA of `tanh` (Bilbao, Esqueda, Parker, Välimäki 2017) on
/// u[n−2], u[n−1], u[n]: one frame of delay.
///
/// ```text
/// D(a, b) = (F₂(a) − F₂(b)) / (a − b)                  (mean_ln_cosh)
/// y[n] = 2·(D(u[n], u[n−1]) − D(u[n−1], u[n−2])) / (u[n] − u[n−2])
///        if |u[n] − u[n−2]| ≥ 1e-4, else with ū = (u[n] + u[n−2])/2, δ = ū − u[n−1]:
/// y[n] = 2·(F₁(ū) − D(ū, u[n−1])) / δ                  if |δ| ≥ 1e-4
/// y[n] = tanh((ū + u[n−1])/2)                           otherwise
/// ```
///
/// clamped to [−1, 1]. D is integrated directly for steps below 0.01
/// ([`mean_ln_cosh`]), so slow inputs have no cancellation floor: the
/// output is tanh(u[n−1]) to within its smoothing term (below 0.77·Δu²/6)
/// plus 1e-10, tested down to steps of 2e-6.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Adaa2 {
    /// u[n−1].
    p1: Point,
    /// u[n−2].
    p2: Point,
    /// D(u[n−1], u[n−2]).
    d12: f64,
}

impl Default for Adaa2 {
    fn default() -> Adaa2 {
        Adaa2 {
            p1: Point::ZERO,
            p2: Point::ZERO,
            d12: ln_cosh(0.0),
        }
    }
}

impl Adaa2 {
    fn tick(&mut self, u: f64) -> f64 {
        let p0 = Point::new(u);
        let d01 = mean_ln_cosh(p0, self.p1);
        let outer = p0.u - self.p2.u;
        let y = if fabs(outer) >= ADAA2_OUTER_EPSILON {
            2.0 * (d01 - self.d12) / outer
        } else {
            let mid = Point::new((p0.u + self.p2.u) / 2.0);
            let delta = mid.u - self.p1.u;
            if fabs(delta) >= ADAA2_OUTER_EPSILON {
                2.0 * (ln_cosh(mid.u) - mean_ln_cosh(mid, self.p1)) / delta
            } else {
                tanh((mid.u + self.p1.u) / 2.0)
            }
        };
        self.p2 = self.p1;
        self.p1 = p0;
        self.d12 = d01;
        // A weighted mean of tanh, so within [−1, 1]; rounding in the
        // quotients can exceed it by about 1e-10 at full drive, which the
        // declared range does not allow.
        y.clamp(-1.0, 1.0)
    }
}

// ---------------------------------------------------------------------------
// Filters

/// The longest filter, in taps.
pub const MAX_TAPS: usize = 160;

/// A ×2 stage's linear-phase low-pass FIR, as designed by [`design`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StageSpec {
    /// Taps *N* (odd); its delay is (*N* − 1)/2 samples at the stage's rate.
    pub taps: usize,
    /// Cut-off (−6 dB) in cycles per sample at the stage's higher rate.
    pub cutoff: f64,
    /// The Kaiser window's β.
    pub beta: f64,
    /// Half-band: the taps at even offsets from the centre are set to 0.
    pub half_band: bool,
}

/// Stage 1 (sr ↔ 2·sr): passband 0 … 0.40·sr, stopband 0.50·sr … sr, i.e.
/// 0.20 and 0.25 cycles per sample at 2·sr. β = 9.0 gives about 90 dB of
/// stopband (Kaiser's formula A ≈ β/0.1102 + 8.7); *N* = 131 meets that
/// transition width of 0.05 (Kaiser: *N* − 1 ≥ (A − 7.95)/(14.36·Δf)).
pub const STAGE1: StageSpec = StageSpec {
    taps: 131,
    cutoff: 0.225,
    beta: 9.0,
    half_band: false,
};

/// Stage 2 (2·sr ↔ 4·sr): half-band, passband to 0.5·sr (0.125 cycles per
/// sample at 4·sr), stopband from 1.5·sr (0.375), whose content would fold
/// onto 0 … 0.5·sr at 2·sr. *N* = 27.
pub const STAGE2: StageSpec = StageSpec {
    taps: 27,
    cutoff: 0.25,
    beta: 9.0,
    half_band: true,
};

/// Stage 3 (4·sr ↔ 8·sr): half-band, passband to 0.5·sr (0.0625 cycles per
/// sample at 8·sr), stopband from 3.5·sr (0.4375). *N* = 19.
pub const STAGE3: StageSpec = StageSpec {
    taps: 19,
    cutoff: 0.25,
    beta: 9.0,
    half_band: true,
};

const STAGES: [StageSpec; 3] = [STAGE1, STAGE2, STAGE3];

/// The delay of the oversampled shapers in frames: stage *s* (1-based)
/// delays (*N*ₛ − 1)/2 samples at 2^*s*·sr each way, and first-order ADAA
/// half a sample at 2^*S*·sr.
pub fn latency_frames(stages: usize, adaa: bool) -> f64 {
    let stages = stages.min(STAGES.len());
    // Up and down: (N − 1)/2 samples each, at 2^s times the frame rate.
    let filters: f64 = STAGES
        .iter()
        .take(stages)
        .zip([2.0, 4.0, 8.0])
        .map(|(spec, rate)| (spec.taps - 1) as f64 / rate)
        .sum();
    let half_sample = if adaa {
        0.5 / f64::from(1_u32 << stages)
    } else {
        0.0
    };
    filters + half_sample
}

/// The zeroth-order modified Bessel function I₀, by its series (fixed 40
/// terms: enough for x ≤ β = 9 to f64 precision).
fn bessel_i0(x: f64) -> f64 {
    let q = x * x / 4.0;
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..=40_u32 {
        let k = f64::from(k);
        term *= q / (k * k);
        sum += term;
    }
    sum
}

/// Writes `spec`'s taps into `taps` (its first `spec.taps` entries; the rest
/// 0): h[n] = 2·f_c·sinc(2·f_c·(n − M))·w[n], M = (N − 1)/2, w the Kaiser
/// window of β, scaled so Σ h = 1 (unit gain at DC). Pure arithmetic with
/// `libm`: the same coefficients on every platform and at every rate.
pub fn design(spec: StageSpec, taps: &mut [f64; MAX_TAPS]) {
    *taps = [0.0; MAX_TAPS];
    let n = spec.taps.min(MAX_TAPS);
    let m = (n as f64 - 1.0) / 2.0;
    let i0_beta = bessel_i0(spec.beta);
    let mut sum = 0.0;
    for (i, h) in taps.iter_mut().take(n).enumerate() {
        let offset = i as f64 - m;
        let x = 2.0 * spec.cutoff * offset;
        let sinc = if offset == 0.0 {
            1.0
        } else {
            sin(core::f64::consts::PI * x) / (core::f64::consts::PI * x)
        };
        let r = if m > 0.0 { offset / m } else { 0.0 };
        let window = bessel_i0(spec.beta * sqrt((1.0 - r * r).max(0.0))) / i0_beta;
        let even_offset = offset != 0.0 && (offset as i64) % 2 == 0;
        *h = if spec.half_band && even_offset {
            0.0
        } else {
            2.0 * spec.cutoff * sinc * window
        };
        sum += *h;
    }
    for h in taps.iter_mut().take(n) {
        *h /= sum;
    }
}

/// A delay line of up to [`MAX_TAPS`] values, newest first: written twice,
/// so the window of its last `len` values is always one contiguous slice.
#[derive(Clone, Debug)]
struct Line {
    buffer: [f64; 2 * MAX_TAPS],
    len: usize,
    position: usize,
}

impl Line {
    fn new(len: usize) -> Line {
        Line {
            buffer: [0.0; 2 * MAX_TAPS],
            len: len.clamp(1, MAX_TAPS),
            position: 0,
        }
    }

    fn clear(&mut self) {
        self.buffer = [0.0; 2 * MAX_TAPS];
        self.position = 0;
    }

    fn push(&mut self, x: f64) {
        self.position = if self.position == 0 {
            self.len - 1
        } else {
            self.position - 1
        };
        for at in [self.position, self.position + self.len] {
            if let Some(slot) = self.buffer.get_mut(at) {
                *slot = x;
            }
        }
    }

    /// The last `len` values, newest first.
    fn window(&self) -> &[f64] {
        self.buffer
            .get(self.position..self.position + self.len)
            .unwrap_or(&[])
    }
}

/// Σ aᵢ·bᵢ over the shorter of the two, in four interleaved partial sums
/// (a fixed order, so the same bits in every run and partition).
fn dot(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len().min(b.len());
    let (a, b) = (a.get(..n).unwrap_or(&[]), b.get(..n).unwrap_or(&[]));
    let (a4, a_rest) = a.as_chunks::<4>();
    let (b4, b_rest) = b.as_chunks::<4>();
    let mut sums = [0.0; 4];
    for (x, y) in a4.iter().zip(b4) {
        for ((s, x), y) in sums.iter_mut().zip(x).zip(y) {
            *s += x * y;
        }
    }
    let rest: f64 = a_rest.iter().zip(b_rest).map(|(x, y)| x * y).sum();
    let [s0, s1, s2, s3] = sums;
    ((s0 + s1) + (s2 + s3)) + rest
}

/// One polyphase branch of a filter: the taps h[2j + p] (j = 0, 1, …),
/// trimmed to those from the first to the last that is not 0 (a half-band
/// filter's odd branch is one tap).
#[derive(Clone, Debug)]
struct Branch {
    taps: [f64; MAX_TAPS / 2 + 1],
    /// The j of the first kept tap.
    start: usize,
    len: usize,
}

impl Branch {
    fn new(h: &[f64], p: usize, scale: f64) -> Branch {
        let all: Vec<f64> = h.iter().skip(p).step_by(2).map(|h| h * scale).collect();
        let start = all.iter().position(|h| *h != 0.0).unwrap_or(0);
        let end = all.iter().rposition(|h| *h != 0.0).map_or(start, |e| e + 1);
        let mut taps = [0.0; MAX_TAPS / 2 + 1];
        for (t, h) in taps.iter_mut().zip(all.get(start..end).unwrap_or(&[])) {
            *t = *h;
        }
        Branch {
            taps,
            start,
            len: end - start,
        }
    }

    /// Σⱼ h[2j + p]·line[j], `line` newest first.
    fn apply(&self, line: &Line) -> f64 {
        let window = line.window();
        let window = window.get(self.start..).unwrap_or(&[]);
        dot(self.taps.get(..self.len).unwrap_or(&[]), window)
    }
}

/// One ×2 stage: an interpolator (low rate → high) and a decimator (high →
/// low) with the same taps, both polyphase: every product is computed at the
/// low rate, and a tap that is 0 is never multiplied.
#[derive(Clone, Debug)]
struct Stage {
    /// h[2j], h[2j + 1].
    branches: [Branch; 2],
    /// The interpolator's input x[m], newest first: ⌈N/2⌉ values.
    up: Line,
    /// The decimator's even input v[2m], v[2m − 2], …
    down_even: Line,
    /// The decimator's odd input v[2m − 1], v[2m − 3], …
    down_odd: Line,
}

impl Stage {
    /// Allocates (a temporary while splitting the taps): `prepare` only.
    fn new(spec: StageSpec) -> Stage {
        let mut taps = [0.0; MAX_TAPS];
        design(spec, &mut taps);
        let n = spec.taps.min(MAX_TAPS);
        let h = taps.get(..n).unwrap_or(&[]);
        let half = n.div_ceil(2);
        Stage {
            branches: [Branch::new(h, 0, 1.0), Branch::new(h, 1, 1.0)],
            up: Line::new(half),
            down_even: Line::new(half),
            down_odd: Line::new(half),
        }
    }

    fn clear(&mut self) {
        self.up.clear();
        self.down_even.clear();
        self.down_odd.clear();
    }

    /// The two high-rate samples v[2m], v[2m + 1] for low-rate x[m]:
    /// v[2m + p] = 2·Σⱼ h[2j + p]·x[m − j].
    fn interpolate(&mut self, x: f64) -> [f64; 2] {
        self.up.push(x);
        let [even, odd] = &self.branches;
        [2.0 * even.apply(&self.up), 2.0 * odd.apply(&self.up)]
    }

    /// The low-rate y[m] = Σᵢ h[i]·v[2m − i] for the high-rate pair v[2m],
    /// v[2m + 1] (v[2m + 1] is kept for the next frame):
    /// Σⱼ h[2j]·v[2m − 2j] + Σⱼ h[2j + 1]·v[2m − 1 − 2j].
    fn decimate(&mut self, pair: [f64; 2]) -> f64 {
        let [v_even, v_odd] = pair;
        self.down_even.push(v_even);
        let [even, odd] = &self.branches;
        let y = even.apply(&self.down_even) + odd.apply(&self.down_odd);
        self.down_odd.push(v_odd);
        y
    }
}

/// The highest oversampling factor.
const MAX_FACTOR: usize = 8;

/// `tanh` of `u` raised to 2^`stages` times the rate, shaped by plain
/// `tanh` or first-order ADAA there, and brought back.
#[derive(Clone, Debug)]
pub struct Oversampled {
    stages: [Stage; 3],
    active: usize,
    adaa: bool,
    /// The last shaped input at the high rate (first-order ADAA's u[n−1]).
    previous: f64,
}

impl Oversampled {
    /// 2^`stages` times oversampling (1 ≤ `stages` ≤ 3), with first-order
    /// ADAA at the high rate if `adaa`. Allocates while it designs the
    /// filters: call it in `prepare` only.
    pub fn new(stages: usize, adaa: bool) -> Oversampled {
        Oversampled {
            stages: STAGES.map(Stage::new),
            active: stages.clamp(1, STAGES.len()),
            adaa,
            previous: 0.0,
        }
    }

    /// Back to silence.
    pub fn clear(&mut self) {
        for stage in &mut self.stages {
            stage.clear();
        }
        self.previous = 0.0;
    }

    /// One frame: the shaped, delayed output for `u` = drive·in.
    pub fn tick(&mut self, u: f64) -> f64 {
        let mut samples = [0.0; MAX_FACTOR];
        let mut count = 1;
        if let Some(first) = samples.first_mut() {
            *first = u;
        }
        let active = self.active;
        for stage in self.stages.iter_mut().take(active) {
            let mut next = [0.0; MAX_FACTOR];
            for (x, pair) in samples
                .iter()
                .take(count)
                .zip(next.as_chunks_mut::<2>().0.iter_mut())
            {
                *pair = stage.interpolate(*x);
            }
            samples = next;
            count *= 2;
        }
        for v in samples.iter_mut().take(count) {
            let u = *v;
            *v = if self.adaa {
                adaa1(u, self.previous)
            } else {
                tanh(u)
            };
            self.previous = u;
        }
        for stage in self.stages.iter_mut().take(active).rev() {
            let mut next = [0.0; MAX_FACTOR];
            for (y, pair) in next
                .iter_mut()
                .zip(samples.as_chunks::<2>().0.iter().take(count / 2))
            {
                *y = stage.decimate(*pair);
            }
            samples = next;
            count /= 2;
        }
        samples.first().copied().unwrap_or(0.0)
    }
}

// ---------------------------------------------------------------------------
// The primitives

fn for_each_frame(
    inputs: &Inputs<'_>,
    outputs: &mut Outputs<'_>,
    mut tick: impl FnMut(f64) -> f64,
) {
    let x = inputs.signal(0);
    let drive = inputs.parameter(0);
    for (n, out) in outputs.signal(0).iter_mut().enumerate() {
        let u = drive.get(n).copied().unwrap_or(0.0) * x.get(n).copied().unwrap_or(0.0);
        *out = tick(u);
    }
}

/// `lab.tanh_adaa2`: `tanh` of drive·in by second-order ADAA, one frame of
/// delay. The work per frame is fixed: one Li₂ series of 52 terms (two on
/// the ill-conditioned branch).
#[derive(Clone, Debug, Default)]
pub struct TanhAdaa2 {
    state: Adaa2,
}

impl DirectPrimitive for TanhAdaa2 {
    fn describe() -> ModuleDescriptor {
        descriptor(
            "lab.tanh_adaa2",
            "tanh saturation of drive·in, antialiased with second-order ADAA (1 frame of delay). Lab: M3 aliasing experiment.",
            &ADAA2_PORTS,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.state = Adaa2::default();
    }

    fn reset(&mut self) {
        self.state = Adaa2::default();
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let state = &mut self.state;
        for_each_frame(inputs, outputs, |u| state.tick(u));
    }
}

/// An oversampled candidate's configuration.
pub trait OversampledConfig: Send + 'static {
    /// ×2 stages: the factor is 2^`STAGES`.
    const STAGES: usize;
    /// First-order ADAA at the high rate (else plain `tanh`).
    const ADAA: bool;
    /// The type's name.
    const NAME: &'static str;
    /// `doc` for `signal modules`.
    const DOC: &'static str;
}

/// An oversampled `tanh` shaper (see the module's documentation).
#[derive(Debug)]
pub struct OversampledShaper<C: OversampledConfig> {
    shaper: Option<Box<Oversampled>>,
    _config: core::marker::PhantomData<C>,
}

impl<C: OversampledConfig> Default for OversampledShaper<C> {
    fn default() -> Self {
        OversampledShaper {
            shaper: None,
            _config: core::marker::PhantomData,
        }
    }
}

impl<C: OversampledConfig> DirectPrimitive for OversampledShaper<C> {
    fn describe() -> ModuleDescriptor {
        descriptor(C::NAME, C::DOC, &PORTS)
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        // Designs the filters; the only allocation.
        self.shaper = Some(Box::new(Oversampled::new(C::STAGES, C::ADAA)));
    }

    fn reset(&mut self) {
        if let Some(shaper) = &mut self.shaper {
            shaper.clear();
        }
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        match &mut self.shaper {
            Some(shaper) => for_each_frame(inputs, outputs, |u| shaper.tick(u)),
            None => outputs.signal(0).fill(0.0),
        }
    }
}

macro_rules! oversampled {
    ($ty:ident, $name:literal, $stages:literal, $adaa:literal, $doc:literal) => {
        #[doc = concat!("`", $name, "`: ", $doc)]
        #[derive(Debug)]
        pub enum $ty {}
        impl OversampledConfig for $ty {
            const STAGES: usize = $stages;
            const ADAA: bool = $adaa;
            const NAME: &'static str = $name;
            const DOC: &'static str = $doc;
        }
    };
}

oversampled!(
    Os2Adaa1,
    "lab.tanh_os2_adaa1",
    1,
    true,
    "tanh of drive·in at 2× the rate with first-order ADAA, linear-phase FIR resampling (65.25 frames of delay). Lab: M3 aliasing experiment."
);
oversampled!(
    Os4Adaa1,
    "lab.tanh_os4_adaa1",
    2,
    true,
    "tanh of drive·in at 4× the rate with first-order ADAA, linear-phase FIR resampling (71.625 frames of delay). Lab: M3 aliasing experiment."
);
oversampled!(
    Os8Adaa1,
    "lab.tanh_os8_adaa1",
    3,
    true,
    "tanh of drive·in at 8× the rate with first-order ADAA, linear-phase FIR resampling (73.8125 frames of delay). Lab: M3 aliasing experiment."
);
oversampled!(
    Os8,
    "lab.tanh_os8",
    3,
    false,
    "tanh of drive·in at 8× the rate (plain tanh), linear-phase FIR resampling (73.75 frames of delay). Lab: M3 aliasing experiment."
);

/// The candidates' type names, in the experiment's order.
pub const TYPES: [&str; 5] = [
    "lab.tanh_adaa2",
    "lab.tanh_os2_adaa1",
    "lab.tanh_os4_adaa1",
    "lab.tanh_os8_adaa1",
    "lab.tanh_os8",
];

/// The delay in frames of the candidate `type_name` (`None` if it is not
/// one).
pub fn delay_frames(type_name: &str) -> Option<f64> {
    match type_name {
        "lab.tanh_adaa2" => Some(1.0),
        "lab.tanh_os2_adaa1" => Some(latency_frames(1, true)),
        "lab.tanh_os4_adaa1" => Some(latency_frames(2, true)),
        "lab.tanh_os8_adaa1" => Some(latency_frames(3, true)),
        "lab.tanh_os8" => Some(latency_frames(3, false)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documented_delays_match_the_filters() {
        assert_eq!(latency_frames(1, true), 65.25);
        assert_eq!(latency_frames(2, true), 71.625);
        assert_eq!(latency_frames(3, true), 73.8125);
        assert_eq!(latency_frames(3, false), 73.75);
    }

    #[test]
    fn li2_matches_known_values() {
        // Li₂(−1) = −π²/12; Li₂(−e^(−2a)) → 0 as a → ∞.
        assert!((li2_term(0.0) + PI2_12).abs() < 1e-15);
        assert!(li2_term(30.0).abs() < 1e-25);
        // Li₂(−1/2)·… check through the derivative: dL/da = 2·ln(1 + e^(−2a)).
        for a in [0.1, 0.5, 1.0, 3.0] {
            let h = 1e-5;
            let numeric = (li2_term(a + h) - li2_term(a - h)) / (2.0 * h);
            let exact = 2.0 * log1p(exp(-2.0 * a));
            assert!((numeric - exact).abs() < 1e-9, "{a}: {numeric} vs {exact}");
        }
    }

    #[test]
    fn f2_is_the_antiderivative_of_ln_cosh() {
        for u in [-7.0, -1.3, -0.2, 0.0, 0.4, 2.0, 15.0] {
            let h = 1e-5;
            let f2 = |u: f64| f2_positive(fabs(u), li2_term(fabs(u))).copysign(u);
            let numeric = (f2(u + h) - f2(u - h)) / (2.0 * h);
            assert!((numeric - ln_cosh(u)).abs() < 1e-8, "{u}");
        }
    }

    /// For a slow input ADAA2 is `tanh` of the input one frame earlier.
    #[test]
    fn adaa2_is_tanh_one_frame_late_for_slow_inputs() {
        let mut state = Adaa2::default();
        let mut inputs = Vec::new();
        for n in 0..20_000 {
            let u = 3.0 * libm::sin(core::f64::consts::TAU * n as f64 / 20_000.0);
            inputs.push(u);
            let y = state.tick(u);
            // From frame 3 on: before it the start from u = 0 is a kink.
            if n >= 3 {
                let expected = tanh(inputs[n - 1]);
                assert!((y - expected).abs() < 1e-6, "{n}: {y} vs {expected}");
            }
        }
    }

    /// The numerical floor: on slow sines (steps from about 2e-6 to 1e-3,
    /// across both branch switches and the zero crossings) ADAA2 stays
    /// within its smoothing term (below 0.77·Δu²/6, |tanh''| ≤ 0.77) plus 1e-10
    /// of tanh one frame late.
    #[test]
    fn adaa2_has_no_cancellation_floor_on_slow_inputs() {
        for period in [20_000.0, 200_000.0, 2_000_000.0, 10_000_000.0] {
            let mut state = Adaa2::default();
            let mut previous = 0.0;
            let mut worst: f64 = 0.0;
            for n in 0..400_000 {
                let u = 3.0 * libm::sin(core::f64::consts::TAU * n as f64 / period);
                let y = state.tick(u);
                if n >= 3 {
                    let step = core::f64::consts::TAU * 3.0 / period;
                    let bound = 0.77 * step * step / 6.0 + 1e-10;
                    let error = (y - tanh(previous)).abs();
                    worst = worst.max(error);
                    assert!(
                        error <= bound,
                        "period {period}, frame {n}: {error} > {bound}"
                    );
                }
                previous = u;
            }
            println!("period {period}: worst {worst:e}");
        }
    }

    #[test]
    fn adaa2_is_finite_and_bounded_for_wild_steps() {
        let mut state = Adaa2::default();
        let values = [
            0.0,
            20.0,
            -20.0,
            20.0,
            1e-9,
            0.0,
            0.0,
            1e-6,
            -1e-6,
            5.0,
            5.0 + 1e-7,
            5.0,
        ];
        for u in values.iter().cycle().take(200) {
            let y = state.tick(*u);
            assert!(y.is_finite() && y.abs() <= 1.0 + 1e-9, "{u}: {y}");
        }
    }

    #[test]
    fn the_filters_have_unit_dc_gain_and_are_symmetric() {
        for spec in STAGES {
            let mut taps = [0.0; MAX_TAPS];
            design(spec, &mut taps);
            let n = spec.taps;
            assert!((taps[..n].iter().sum::<f64>() - 1.0).abs() < 1e-14);
            for i in 0..n {
                assert_eq!(taps[i], taps[n - 1 - i]);
            }
            assert!(taps[n..].iter().all(|h| *h == 0.0));
        }
    }

    /// At a low drive the shaper is nearly linear: a slow sine comes out
    /// with the documented delay.
    #[test]
    fn a_slow_sine_comes_out_delayed_by_the_documented_frames() {
        for (stages, adaa) in [(1, true), (2, true), (3, true), (3, false)] {
            let mut shaper = Oversampled::new(stages, adaa);
            let delay = latency_frames(stages, adaa);
            let period = 400.0;
            let amplitude = 0.01;
            let mut worst: f64 = 0.0;
            for n in 0..4_000 {
                let t = n as f64;
                let y = shaper.tick(amplitude * libm::sin(core::f64::consts::TAU * t / period));
                if n > 500 {
                    let expected =
                        tanh(amplitude * libm::sin(core::f64::consts::TAU * (t - delay) / period));
                    worst = worst.max((y - expected).abs());
                }
            }
            assert!(worst < 1e-4 * amplitude, "{stages} {adaa}: {worst}");
        }
    }
}
