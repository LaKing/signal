//! The M3 "ADAA versus oversampling" experiment (`experiments/m3-tanh-aliasing/`):
//! candidate replacements for `std.tanh_shaper`, each with the same ports and
//! parameter, so a project can swap one for another by its type name alone.
//!
//! | Type | Antialiasing | Delay (frames) |
//! |---|---|---|
//! | `lab.tanh_adaa2` | second-order ADAA at the frame rate | 1 |
//! | `lab.tanh_os2_adaa1` | ×2 oversampling, first-order ADAA at 2·sr | [`latency_frames`]`(1, true)` = 65.25 |
//! | `lab.tanh_os4_adaa1` | ×4 oversampling, first-order ADAA at 4·sr | `latency_frames(2, true)` = 71.625 |
//! | `lab.tanh_os8` | ×8 oversampling, plain `tanh` at 8·sr | `latency_frames(3, false)` = 73.75 |
//!
//! The sixth candidate, ×8 oversampling with first-order ADAA at 8·sr
//! (formerly `lab.tanh_os8_adaa1`), was promoted to `std.tanh_shaper_os8`
//! after the owner's verdict of 2026-10-23 (`experiments/m3-tanh-aliasing/verdict.md`).
//! The oversampled candidates here are built from its resampling filters,
//! [`signal_std::oversample`], so they differ from it only in the factor and
//! the shaping at the high rate. The rest stay as the experiment's record.
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
//! **Oversampling** ([`signal_std::oversample`]). `u = drive·in` is raised
//! to *R* = 2^*S* times the rate by *S* cascaded ×2 linear-phase FIR stages,
//! shaped there, and brought back by the same stages. The work per frame is
//! fixed: no branch depends on the signal except ADAA's ill-conditioning
//! switch.

use libm::{exp, fabs, log1p, tanh};
use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs,
    ParameterDescriptor, PortDescriptor, PortKind, PortShape, PrepareContext, Range, RateDivisor,
    Reproducibility,
};
pub use signal_std::oversample::{Oversampled, latency_frames};

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

/// Unbounded, unlike `std.tanh_shaper`'s `[−1, 1]`: a band-limited clipped
/// (square-like) output overshoots 1 (Gibbs), as `std.tanh_shaper_os8`'s does.
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
    values: None,
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
    Os8,
    "lab.tanh_os8",
    3,
    false,
    "tanh of drive·in at 8× the rate (plain tanh), linear-phase FIR resampling (73.75 frames of delay). Lab: M3 aliasing experiment."
);

/// The candidates left in the lab, in the experiment's order (the promoted
/// one, `std.tanh_shaper_os8`, came between `lab.tanh_os4_adaa1` and
/// `lab.tanh_os8`).
pub const TYPES: [&str; 4] = [
    "lab.tanh_adaa2",
    "lab.tanh_os2_adaa1",
    "lab.tanh_os4_adaa1",
    "lab.tanh_os8",
];

/// The delay in frames of the candidate `type_name` (`None` if it is not
/// one).
pub fn delay_frames(type_name: &str) -> Option<f64> {
    match type_name {
        "lab.tanh_adaa2" => Some(1.0),
        "lab.tanh_os2_adaa1" => Some(latency_frames(1, true)),
        "lab.tanh_os4_adaa1" => Some(latency_frames(2, true)),
        "lab.tanh_os8" => Some(latency_frames(3, false)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documented_delays_match_the_filters() {
        assert_eq!(delay_frames("lab.tanh_os2_adaa1"), Some(65.25));
        assert_eq!(delay_frames("lab.tanh_os4_adaa1"), Some(71.625));
        assert_eq!(delay_frames("lab.tanh_os8"), Some(73.75));
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
}
