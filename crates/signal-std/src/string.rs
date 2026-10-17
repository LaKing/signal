//! `std.string`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::sine_osc::FREQ_MAX;
use crate::svf::flush;
use crate::{at, event_port, parameter, round_half_up, signal_port, triggered};

const FREQ: f64 = 220.0;
const DECAY_S: f64 = 2.0;
const BRIGHTNESS: f64 = 0.5;
const STIFFNESS: f64 = 0.0;
const POSITION: f64 = 0.2;
/// The lowest effective frequency, in Hz.
pub(crate) const FREQ_MIN: f64 = 20.0;
/// The shortest loop, in frames: the highest effective frequency is `sr/8`.
const MIN_PERIOD: f64 = 8.0;
/// The dispersion allpasses in the loop.
const STAGES: usize = 4;
/// The largest magnitude of the dispersion allpasses' coefficient.
const DISPERSION_MAX: f64 = 0.75;
/// Frames of the loop that are not the delay line or the dispersion: the loss
/// filter's 1 and the least delay line plus tuning allpass, 1 + ½.
const FIXED_DELAY: f64 = 2.5;

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.string",
    doc: "Extended Karplus-Strong string: a delay loop tuned exactly by a first-order allpass, a linear-phase loss filter from decay_s and brightness, optional stiffness dispersion; excited by its input and by pluck events.",
    ports: &[
        signal_port("in", Direction::In, PortKind::Audio, None, None),
        event_port("pluck", Direction::In),
        signal_port("out", Direction::Out, PortKind::Audio, None, None),
    ],
    parameters: &[
        parameter(
            "freq",
            PortKind::Frequency,
            Some("Hz"),
            0.0,
            FREQ_MAX,
            FREQ,
            Combine::Add,
        ),
        parameter(
            "decay_s",
            PortKind::Control,
            Some("s"),
            0.01,
            60.0,
            DECAY_S,
            Combine::Add,
        ),
        parameter(
            "brightness",
            PortKind::Control,
            None,
            0.0,
            1.0,
            BRIGHTNESS,
            Combine::Add,
        ),
        parameter(
            "stiffness",
            PortKind::Control,
            None,
            0.0,
            1.0,
            STIFFNESS,
            Combine::Add,
        ),
        parameter(
            "position",
            PortKind::Control,
            None,
            0.01,
            0.99,
            POSITION,
            Combine::Add,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::PHYSICAL),
};

/// `std.string`: an extended Karplus–Strong string, a single-delay-loop
/// digital waveguide with exact tuning, a frequency-dependent loss and
/// optional dispersion.
///
/// **Ports**
/// - `in` (signal in, audio, unbounded): excitation added to the loop on
///   every frame; unconnected it reads 0.
/// - `pluck` (event in, trigger): each frame with at least one `Trigger`
///   starts a pluck at `position`; several on one frame count once. `Set` and
///   `Ramp` events are ignored.
/// - `out` (signal out, audio): the string's displacement.
///
/// **Parameters**
/// - `freq` (frequency, Hz, range `[20, sr/8]`, default 220, combine `add`).
///   Declared `[0, (2³² − 1)/2]` as `std.osc`'s; `process` limits the
///   effective value to `[20, sr/8]`, so the loop is at least 8 frames long.
/// - `decay_s` (control, seconds, range `[0.01, 60]`, default 2, combine
///   `add`): T60 of the fundamental, the time it takes to fall by 60 dB.
/// - `brightness` (control, no unit, range `[0, 1]`, default 0.5, combine
///   `add`): 1 decays every harmonic as the fundamental, 0 damps the high
///   harmonics most (see the loss filter).
/// - `stiffness` (control, no unit, range `[0, 1]`, default 0, combine
///   `add`): dispersion; above 0 the upper partials are sharp of the
///   harmonics, as in a stiff string, and the fundamental stays in tune.
/// - `position` (control, fraction of the string, range `[0.01, 0.99]`,
///   default 0.2, combine `add`): where a pluck is made; read on the pluck's
///   frame.
///
/// **Feedthrough:** direct (`in[n]` reaches `out[n]`).
///
/// **Formula.** One loop, sr the sample rate, ω = 2π·f/sr:
///
/// ```text
/// y[n] = T(L(A⁴(y[n − N]))) + in[n] + p[n]          out[n] = y[n]
///
/// A(z) = (a + z⁻¹)/(1 + a·z⁻¹)                      dispersion, 4 in series
/// L(z) = g·(β + (1 − 2β)·z⁻¹ + β·z⁻²)               loss: linear phase, 1 frame
/// T(z) = (η + z⁻¹)/(1 + η·z⁻¹)                      fractional tuning
/// ```
///
/// Its coefficients, from the effective parameters:
///
/// ```text
/// f     = min(max(freq, 20), sr/8),   P = sr/f      period, frames
/// q     = (P − 2.5)/4
/// a     = stiffness·max(−0.75, (1 − q)/(1 + q))     ≤ 0
/// τ_A   = 1 − 2·atan2(a·sin ω, 1 + a·cos ω)/ω       A's phase delay at ω
/// R     = P − 1 − 4·τ_A                             left for N and T
/// N     = ⌊R − ½⌋ ≥ 1,   Δ = R − N ∈ [½, 3/2)
/// η     = sin((1 − Δ)·ω/2) / sin((1 + Δ)·ω/2)       T's phase delay at ω is Δ
/// G     = N + 4·τg(a) + 1 + τg(η)                   loop group delay at ω
/// τg(c) = (1 − c²)/(1 + 2c·cos ω + c²)              a first-order allpass's
/// ρ     = 10^(−3·G/(decay_s·sr))                    loop gain at ω
/// β     = min((1 − brightness)/4, (1 − √ρ)/(2·(1 − cos ω)))
/// g     = ρ / (1 − 2β·(1 − cos ω))
/// ```
///
/// **Tuning.** The loop's phase delay at ω is `N + 4·τ_A + 1 + Δ = P`
/// exactly: `L` is symmetric, so its delay is 1 frame at every frequency, and
/// `η` is the first-order allpass whose phase delay at ω is exactly `Δ`, the
/// exact-phase form of Thiran's `(1 − Δ)/(1 + Δ)`, to which it tends as
/// ω → 0. Keeping `Δ ∈ [½, 3/2)` keeps `T`'s pole well inside the circle
/// (|η| ≤ ⅓ at ω → 0). `a` is limited so that `4·τ_A` never exceeds
/// `P − 2.5`, which keeps `N ≥ 1` and `Δ ≥ ½`: dispersion yields to tuning
/// at high notes.
///
/// **Decay.** The fundamental's envelope falls by the loop gain once per
/// group delay `G`, so `ρ = 10^(−3·G/(decay_s·sr))` makes its T60 `decay_s`.
/// The loss filter's magnitude `g·(1 − 2β·(1 − cos ω′))` falls from `g` at 0
/// to `g·(1 − 4β)` at `sr/2`; with `brightness = 1` (β = 0) it is flat. β is
/// limited so that the DC gain `g` never exceeds `√ρ`: the loop is stable
/// (gain ≤ 1 at every frequency, the allpasses have gain 1), and a DC
/// offset falls at least half as fast as the fundamental. At high notes
/// with long decays the limit can make `brightness` act less.
///
/// **Pluck.** A pluck on frame `n₀` reads `position` (u₀) and the period
/// `K = round_half_up(sr/f)` (frames, at least 8) and adds, over the next `K`
/// frames, the triangle of a string pulled aside at `u₀`, less its mean ½:
///
/// ```text
/// p[n₀ + i] = tri(i/K) − ½,   0 ≤ i < K
/// tri(u)    = u/u₀  (u ≤ u₀),   (1 − u)/(1 − u₀)  (otherwise)
/// ```
///
/// so the harmonics whose node is at `u₀` are missing. A pluck during a
/// pluck restarts it; the two do not add.
///
/// **Initial state** (`prepare`, `reset`): the delay line, the filter states
/// and the pluck are 0. **Recomputed**: the coefficients only on a frame
/// where `f`, `decay_s`, `brightness` or `stiffness` differs from the frame
/// before (always on the first frame). A new `N` moves the read position at
/// once; the tuning and decay are exact for a constant `f`.
///
/// **Buffer.** `prepare` allocates the delay line, `⌈sr/20⌉ + 2` frames,
/// zeroed; nothing else allocates. `reset` zeroes it again. `y[n]` and every
/// allpass state are flushed to exactly 0 below 1e-9
/// (`docs/dsp-conventions.md`, "Denormals"). `sin`, `cos`, `atan2`, `pow` and
/// `sqrt` are `libm`'s. Work per frame is constant, plus a coefficient
/// update on a changed frame.
#[derive(Clone, Debug, Default)]
pub struct StringModel {
    sample_rate: f64,
    /// The loop's history; `line[newest]` is `y` of the last frame.
    line: Vec<f64>,
    newest: usize,
    /// The inputs the coefficients were computed for (NaN: none yet).
    computed: [f64; 4],
    /// The effective frequency.
    freq: f64,
    delay: usize,
    dispersion: f64,
    tuning: f64,
    loss_gain: f64,
    loss_side: f64,
    /// The dispersion allpasses' states.
    stages: [f64; STAGES],
    /// The loss filter's last two inputs.
    loss: [f64; 2],
    tune_state: f64,
    /// The pluck: its next frame, its length `K` (0 when none) and `u₀`.
    pluck_frame: usize,
    pluck_length: usize,
    pluck_position: f64,
}

/// The coefficients of a string at `freq` Hz (already limited to
/// `[20, sr/8]`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Coefficients {
    /// The delay line's length `N`, frames.
    pub delay: usize,
    /// The dispersion allpasses' `a`.
    pub dispersion: f64,
    /// The tuning allpass's `η`.
    pub tuning: f64,
    /// The loss filter's `g` and `β`.
    pub loss_gain: f64,
    pub loss_side: f64,
}

/// The phase delay at `w` of the first-order allpass `(c + z⁻¹)/(1 + c·z⁻¹)`.
fn allpass_phase_delay(c: f64, w: f64) -> f64 {
    1.0 - 2.0 * libm::atan2(c * libm::sin(w), 1.0 + c * libm::cos(w)) / w
}

/// The group delay at `w` of the first-order allpass `(c + z⁻¹)/(1 + c·z⁻¹)`.
fn allpass_group_delay(c: f64, w: f64) -> f64 {
    (1.0 - c * c) / (1.0 + 2.0 * c * libm::cos(w) + c * c)
}

/// The formula's coefficients.
pub(crate) fn coefficients(
    freq: f64,
    decay_s: f64,
    brightness: f64,
    stiffness: f64,
    sample_rate: f64,
) -> Coefficients {
    let period = sample_rate / freq;
    let w = std::f64::consts::TAU * freq / sample_rate;
    let q = (period - FIXED_DELAY) / STAGES as f64;
    let dispersion = stiffness * (-DISPERSION_MAX).max((1.0 - q) / (1.0 + q));
    let rest = period - 1.0 - STAGES as f64 * allpass_phase_delay(dispersion, w);
    let delay = libm::floor(rest - 0.5).max(1.0);
    let fraction = rest - delay;
    let tuning = libm::sin((1.0 - fraction) * w / 2.0) / libm::sin((1.0 + fraction) * w / 2.0);
    let group = delay
        + STAGES as f64 * allpass_group_delay(dispersion, w)
        + 1.0
        + allpass_group_delay(tuning, w);
    let rho = libm::pow(10.0, -3.0 * group / (decay_s * sample_rate));
    let one_minus_cos = 1.0 - libm::cos(w);
    let loss_side = ((1.0 - brightness) / 4.0).min((1.0 - libm::sqrt(rho)) / (2.0 * one_minus_cos));
    let loss_gain = rho / (1.0 - 2.0 * loss_side * one_minus_cos);
    Coefficients {
        delay: delay as usize,
        dispersion,
        tuning,
        loss_gain,
        loss_side,
    }
}

impl DirectPrimitive for StringModel {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        let length = libm::ceil(ctx.sample_rate / FREQ_MIN).max(MIN_PERIOD) as usize + 2;
        self.line = vec![0.0; length];
        self.reset();
    }

    fn reset(&mut self) {
        self.line.fill(0.0);
        self.newest = 0;
        self.computed = [f64::NAN; 4];
        self.stages = [0.0; STAGES];
        self.loss = [0.0; 2];
        self.tune_state = 0.0;
        self.pluck_frame = 0;
        self.pluck_length = 0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let x = inputs.signal(0);
        let plucks = inputs.events(0);
        let (freq, decay_s) = (inputs.parameter(0), inputs.parameter(1));
        let (brightness, stiffness) = (inputs.parameter(2), inputs.parameter(3));
        let position = inputs.parameter(4);
        let len = self.line.len().max(1);
        let mut next = 0;
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            let wanted = [
                at(freq, n, FREQ)
                    .max(FREQ_MIN)
                    .min(self.sample_rate / MIN_PERIOD),
                at(decay_s, n, DECAY_S),
                at(brightness, n, BRIGHTNESS),
                at(stiffness, n, STIFFNESS),
            ];
            if wanted != self.computed {
                self.computed = wanted;
                let [f, decay_s, brightness, stiffness] = wanted;
                let c = coefficients(f, decay_s, brightness, stiffness, self.sample_rate);
                self.freq = f;
                self.delay = c.delay.min(len - 1);
                self.dispersion = c.dispersion;
                self.tuning = c.tuning;
                self.loss_gain = c.loss_gain;
                self.loss_side = c.loss_side;
            }
            if triggered(plucks, &mut next, n) {
                self.pluck_frame = 0;
                self.pluck_length = round_half_up(self.sample_rate / self.freq) as usize;
                self.pluck_position = at(position, n, POSITION);
            }

            // y[n − N]: `newest` holds y[n − 1].
            let read = (self.newest + 1 + len - self.delay) % len;
            let mut v = self.line.get(read).copied().unwrap_or(0.0);
            let a = self.dispersion;
            for state in &mut self.stages {
                let y = a * v + *state;
                *state = flush(v - a * y);
                v = y;
            }
            let [l1, l2] = self.loss;
            let b = self.loss_side;
            let w = self.loss_gain * (b * v + (1.0 - 2.0 * b) * l1 + b * l2);
            self.loss = [v, l1];
            let t = self.tuning * w + self.tune_state;
            self.tune_state = flush(w - self.tuning * t);

            let y = flush(t + at(x, n, 0.0) + self.pluck());
            self.newest = (self.newest + 1) % len;
            if let Some(slot) = self.line.get_mut(self.newest) {
                *slot = y;
            }
            *out = y;
        }
    }
}

impl StringModel {
    /// The pluck's sample of this frame, 0 when none sounds; moves it on.
    fn pluck(&mut self) -> f64 {
        if self.pluck_frame >= self.pluck_length {
            return 0.0;
        }
        let u = self.pluck_frame as f64 / self.pluck_length as f64;
        let u0 = self.pluck_position;
        self.pluck_frame += 1;
        let tri = if u <= u0 {
            u / u0
        } else {
            (1.0 - u) / (1.0 - u0)
        };
        tri - 0.5
    }
}

/// Measurements of a decaying partial, shared by the physical models' tests.
#[cfg(test)]
pub(crate) mod measure {
    /// The Hann-windowed DFT of `samples[start..start + len]` at `f` Hz:
    /// its magnitude and its phase referred to frame 0.
    pub(crate) fn component(
        samples: &[f64],
        start: usize,
        len: usize,
        f: f64,
        sample_rate: f64,
    ) -> (f64, f64) {
        let (mut re, mut im) = (0.0, 0.0);
        for i in 0..len {
            let n = start + i;
            let w = 0.5 - 0.5 * libm::cos(std::f64::consts::TAU * i as f64 / len as f64);
            let angle = std::f64::consts::TAU * f * n as f64 / sample_rate;
            re += w * samples[n] * libm::cos(angle);
            im -= w * samples[n] * libm::sin(angle);
        }
        (libm::sqrt(re * re + im * im), libm::atan2(im, re))
    }

    /// The frequency and T60 of the partial near `f` Hz, from two windows of
    /// `len` frames starting at `t1` and `t2`: the phase advance between them
    /// (unwrapped against `f`) and their magnitude ratio.
    pub(crate) fn partial(
        samples: &[f64],
        f: f64,
        t1: usize,
        t2: usize,
        len: usize,
        sample_rate: f64,
    ) -> (f64, f64) {
        let (m1, p1) = component(samples, t1, len, f, sample_rate);
        let (m2, p2) = component(samples, t2, len, f, sample_rate);
        // At exactly f the referred phase does not move; the residual turn
        // over t2 − t1 frames is the frequency error.
        let tau = std::f64::consts::TAU;
        let mut d = p2 - p1;
        d -= tau * libm::round(d / tau);
        let seconds = (t2 - t1) as f64 / sample_rate;
        let frequency = f + d / (tau * seconds);
        let db = 20.0 * libm::log10(m1 / m2);
        (frequency, 60.0 * seconds / db)
    }

    /// The difference from `reference` to `f`, in cents.
    pub(crate) fn cents(f: f64, reference: f64) -> f64 {
        1200.0 * libm::log2(f / reference)
    }
}

#[cfg(test)]
mod tests {
    use super::measure::{cents, partial};
    use super::*;
    use crate::testing::{Timeline, context, render};

    /// Plucks a string at frame 0 and renders `frames` frames.
    fn pluck(params: [f64; 5], sample_rate: f64, frames: usize) -> Vec<f64> {
        let mut string = StringModel::default();
        string.prepare(&context(sample_rate, 256, 0));
        let lanes: Vec<Vec<f64>> = params.iter().map(|&p| vec![p; frames]).collect();
        let lanes: Vec<&[f64]> = lanes.iter().map(Vec::as_slice).collect();
        let x = vec![0.0; frames];
        let run = render(
            &mut string,
            &Timeline {
                signals: &[&x],
                parameters: &lanes,
                triggers: &[&[0]],
            },
            frames,
            256,
        );
        run.signals[0].clone()
    }

    /// AC2: the fundamental within 0.1 cent at 55, 220, 880 and 3520 Hz at
    /// 44.1 and 96 kHz, and its T60 within 5 % of `decay_s`, with the loss
    /// and dispersion filters active.
    #[test]
    fn the_fundamental_is_in_tune_and_decays_in_decay_s() {
        let decay_s = 1.5;
        for sample_rate in [44_100.0, 96_000.0] {
            for f in [55.0, 220.0, 880.0, 3520.0] {
                for (brightness, stiffness) in [(0.5, 0.0), (0.5, 0.5), (0.2, 1.0), (1.0, 0.0)] {
                    let frames = (1.6 * sample_rate) as usize;
                    let y = pluck(
                        [f, decay_s, brightness, stiffness, 0.3],
                        sample_rate,
                        frames,
                    );
                    let len = (0.5 * sample_rate) as usize;
                    let (t1, t2) = ((0.1 * sample_rate) as usize, (0.6 * sample_rate) as usize);
                    let (measured, t60) = partial(&y, f, t1, t2, len, sample_rate);
                    let off = cents(measured, f);
                    let c = coefficients(f, decay_s, brightness, stiffness, sample_rate);
                    println!(
                        "{sample_rate} Hz, f {f}, brightness {brightness}, stiffness {stiffness}: \
                         {off:+.6} cent, T60 {t60:.4} s (N {}, a {:.4}, η {:.4}, β {:.4}, g {:.6})",
                        c.delay, c.dispersion, c.tuning, c.loss_side, c.loss_gain
                    );
                    assert!(off.abs() < 0.1, "{f} Hz at {sample_rate}: {off} cent");
                    assert!(
                        (t60 / decay_s - 1.0).abs() < 0.05,
                        "{f} Hz at {sample_rate}: T60 {t60}"
                    );
                }
            }
        }
    }

    /// The dispersion is active: with stiffness the second partial is sharp
    /// of 2·f, without it it is not.
    #[test]
    fn stiffness_sharpens_the_upper_partials() {
        let (sample_rate, f) = (48_000.0, 220.0);
        let frames = 48_000;
        let second = |stiffness| {
            let y = pluck([f, 2.0, 1.0, stiffness, 0.3], sample_rate, frames);
            partial(&y, 2.0 * f, 4800, 24_000, 19_200, sample_rate).0
        };
        let (plain, stiff) = (second(0.0), second(1.0));
        println!("second partial: {plain:.4} Hz plain, {stiff:.4} Hz stiff");
        assert!(cents(plain, 2.0 * f).abs() < 0.1);
        assert!(cents(stiff, 2.0 * f) > 1.0);
    }

    /// The pluck's position shapes the harmonics as a triangle's do, harmonic
    /// k ∝ sin(kπ·u₀)/k²: at ½ the second is missing, at ¼ it is
    /// sin(π/2)/(4·sin(π/4)) ≈ 0.354 of the first. (Not exactly 0 at ½: the
    /// loop loses its gain once per period, which modulates the partials by
    /// about (1 − ρ)/2π.)
    #[test]
    fn the_pluck_position_shapes_the_harmonics() {
        let (sample_rate, f) = (48_000.0, 200.0);
        let ratio = |position| {
            let y = pluck([f, 2.0, 1.0, 0.0, position], sample_rate, 9600);
            let level = |h: f64| super::measure::component(&y, 2400, 4800, h * f, sample_rate).0;
            level(2.0) / level(1.0)
        };
        let (middle, quarter) = (ratio(0.5), ratio(0.25));
        let expected = 1.0 / (4.0 * libm::sin(std::f64::consts::FRAC_PI_4));
        println!("second/first: {middle:e} at ½, {quarter:.5} at ¼ (expected {expected:.5})");
        assert!(middle < 0.01);
        assert!((quarter / expected - 1.0).abs() < 0.02);
    }

    /// The loop is stable at every extreme: a full-scale input for 2 s at the
    /// longest decay stays finite and bounded by the loop's geometric sum.
    #[test]
    fn coefficients_keep_the_loop_stable() {
        for sample_rate in [44_100.0, 192_000.0] {
            for f in [20.0, 1000.0, sample_rate / 8.0] {
                for (brightness, stiffness) in [(0.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                    let c = coefficients(f, 60.0, brightness, stiffness, sample_rate);
                    assert!(c.delay >= 1);
                    assert!(c.loss_side >= 0.0 && c.loss_side <= 0.25);
                    assert!(c.loss_gain > 0.0 && c.loss_gain < 1.0, "{c:?}");
                    assert!(c.tuning.abs() < 1.0 && c.dispersion.abs() < 1.0);
                }
            }
        }
    }

    /// Partition invariance of the pluck across calls, and `reset`.
    #[test]
    fn plucks_span_calls_and_reset_restarts() {
        let frames = 4000;
        let mut string = StringModel::default();
        string.prepare(&context(48_000.0, 7, 0));
        let lanes = [
            vec![440.0; frames],
            vec![1.0; frames],
            vec![0.5; frames],
            vec![0.3; frames],
            vec![0.2; frames],
        ];
        let lanes: Vec<&[f64]> = lanes.iter().map(Vec::as_slice).collect();
        let x = vec![0.0; frames];
        let timeline = Timeline {
            signals: &[&x],
            parameters: &lanes,
            triggers: &[&[3, 1000, 1050]],
        };
        let a = render(&mut string, &timeline, frames, 7).signals[0].clone();
        string.reset();
        let b = render(&mut string, &timeline, frames, 1).signals[0].clone();
        assert_eq!(a, b);
        assert!(a[..3].iter().all(|&y| y == 0.0));
        assert_eq!(a[3], -0.5);
    }
}
