//! `std.comb` and `std.allpass_delay`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::line::{Line, STATE_BOUND, finite_or_zero};
use crate::svf::{CUTOFF_MAX, effective_cutoff, flush};
use crate::{SIGNAL_BOUND, at, parameter, signal_port};

/// The damping coefficient `a = 1 − e^(−2π·fc/sr)` of the comb's convex
/// one-pole `lp = s + a·(r − s)`: in `(0, 1)` for every `fc` in
/// `[1, 0.49·sr]`.
fn damping_coefficient(fc: f64, sample_rate: f64) -> f64 {
    1.0 - libm::exp(-std::f64::consts::TAU * fc / sample_rate)
}

/// The longest delay of both types, in ms.
const MAX_MS: f64 = 1000.0;
/// The largest feedback (comb) or gain (all-pass) magnitude: the declared
/// range stops below 1, so the executor's clamp keeps the loop gain below 1.
const MAX_GAIN: f64 = 0.999;
const COMB_TIME_MS: f64 = 25.0;
const COMB_FEEDBACK: f64 = 0.8;
const DAMPING_HZ: f64 = 8000.0;
const ALLPASS_TIME_MS: f64 = 5.0;
const ALLPASS_GAIN: f64 = 0.5;

const COMB: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.comb",
    doc: "Feedback comb filter y[n] = x[n] + feedback·LP(y[n − d]) with a one-pole low-pass (damping) in the loop and a bounded Hermite read, a contraction under any modulation and a Hermite-interpolated delay of up to 1 s: reverb combs and Karplus-Strong strings.",
    ports: &[
        signal_port("in", Direction::In, PortKind::Audio, None, None),
        signal_port("out", Direction::Out, PortKind::Audio, None, None),
    ],
    parameters: &[
        parameter(
            "time_ms",
            PortKind::Control,
            Some("ms"),
            0.0,
            MAX_MS,
            COMB_TIME_MS,
            Combine::Add,
        ),
        parameter(
            "feedback",
            PortKind::Generic,
            None,
            -MAX_GAIN,
            MAX_GAIN,
            COMB_FEEDBACK,
            Combine::Add,
        ),
        parameter(
            "damping_hz",
            PortKind::Frequency,
            Some("Hz"),
            1.0,
            CUTOFF_MAX,
            DAMPING_HZ,
            Combine::Add,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::DELAY),
};

const ALLPASS: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.allpass_delay",
    doc: "Schroeder all-pass delay (−g + z^−d)/(1 − g·z^−d) with a Hermite-interpolated delay of up to 1 s: the diffusers of a reverb.",
    ports: &[
        signal_port("in", Direction::In, PortKind::Audio, None, None),
        signal_port("out", Direction::Out, PortKind::Audio, None, None),
    ],
    parameters: &[
        parameter(
            "time_ms",
            PortKind::Control,
            Some("ms"),
            0.0,
            MAX_MS,
            ALLPASS_TIME_MS,
            Combine::Add,
        ),
        parameter(
            "gain",
            PortKind::Generic,
            None,
            -MAX_GAIN,
            MAX_GAIN,
            ALLPASS_GAIN,
            Combine::Add,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::DELAY),
};

/// The delay in frames for `time_ms` at `sample_rate`, limited to
/// `[1, max]` (a NaN reads as 1 frame).
fn delay_frames(time_ms: f64, sample_rate: f64, max: f64) -> f64 {
    (time_ms * sample_rate / 1000.0).max(1.0).min(max)
}

/// `⌈sr⌉`: the longest delay in frames (1 s).
fn max_frames(sample_rate: f64) -> f64 {
    libm::ceil(MAX_MS / 1000.0 * sample_rate).max(1.0)
}

/// `std.comb`: a feedback comb filter with a damping low-pass in its loop.
///
/// **Ports**
/// - `in` (signal in, audio, unbounded); unconnected it reads 0.
/// - `out` (signal out, audio).
///
/// **Parameters**
/// - `time_ms` (control, ms, range `[0, 1000]`, default 25, combine `add`):
///   the loop delay `d`, limited in `process` to `[1, ⌈sr⌉]` frames. For a
///   Karplus-Strong string of pitch f, `time_ms ≈ 1000/f` (the damping
///   low-pass adds its own phase delay, which flattens the pitch slightly
///   at high pitches and low `damping_hz`).
/// - `feedback` (generic, no unit, range `[−0.999, 0.999]`, default 0.8,
///   combine `add`): the loop's gain on each pass at DC, and its largest
///   gain on any signal. The range stops below 1, and the executor clamps
///   to it, so the loop is a contraction whatever the modulation (below).
/// - `damping_hz` (frequency, Hz, range `[1, 0.49·sr]`, default 8000,
///   combine `add`): the cutoff of the one-pole low-pass in the loop, which
///   makes high frequencies decay faster (Freeverb's damping, the
///   Karplus-Strong loss filter). Declared `[1, 0.49·(2³² − 1)]` as
///   `std.svf`'s cutoff; `process` limits it to `0.49·sr`. In Hz, not as a
///   coefficient, so the sound does not change with the sample rate. The
///   filter is the impulse-invariant one-pole (not the TPT one, whose
///   impulse response alternates in sign above sr/4 and so can amplify a
///   modulated loop): even at the top of the range it does not reach flat.
///   At 0.49·sr, `a` = 0.954 and the loop's gain at sr/2 is
///   `a/(2 − a)` = 0.91 times `feedback`; at 16 kHz and 48 kHz, 0.71.
///
/// **Feedthrough:** direct (`in[n]` reaches `out[n]`).
///
/// **Formula** (x = `in`, y\[j\] = 0 for j < 0, s\[−1\] = 0, sr the sample rate):
///
/// ```text
/// d   = clamp(time_ms[n]·sr/1000, 1, ⌈sr⌉)                    frames
/// r   = y[n − d]    (bounded Hermite read of the y history, below)
/// a   = 1 − exp(−2π·min(max(damping_hz[n], 1), 0.49·sr)/sr)
/// lp  = s + a·(r − s),   s ← lp                      (one-pole low-pass)
/// y[n] = clamp(x[n] + feedback[n]·lp, −10⁶, 10⁶)   (x[n] = 0 if not finite)
/// out[n] = y[n]
/// ```
///
/// `r` is `std.delay`'s 4-point Hermite read (exact at a whole `d`; for
/// `1 < d < 2` its causal fallback, `docs/dsp-conventions.md`) clamped to
/// `[min(y₀, y₁), max(y₀, y₁)]`, the two samples it lies between. On a
/// smooth signal the clamp rarely acts; next to a sharp step it removes
/// Hermite's overshoot. `a` is recomputed only on a frame where the
/// effective `damping_hz` differs from the frame before. `s` and `y` are
/// set to exactly 0 below 1e-9 in magnitude.
///
/// **Bound, for any modulation.** Let `M = max |x|` over the run so far.
/// Then `|y[n]| ≤ M/(1 − |feedback|_max)`, with `|feedback|_max` the
/// largest magnitude `feedback` took so far (at most 0.999, so the bound is
/// at most `1000·M`). By induction: if every earlier `|y|` and `|s|` is at
/// most `B = M/(1 − |feedback|_max)`, then `|r| ≤ B` (the clamped read lies
/// between two earlier `y`), `|lp| ≤ max(|s|, |r|) ≤ B` (a convex
/// combination, `0 < a < 1`), and `|y| ≤ M + |feedback|·B ≤ B`. It holds
/// whatever `time_ms`, `feedback` and `damping_hz` do on every frame, so
/// the clamp to ±10⁶ (the engine's bound, `docs/semantics.md` §7) acts only
/// when the input itself is that large. For fixed parameters the loop's
/// gain at every frequency is at most `|feedback|`, and an impulse decays by
/// at least 60 dB every `6.9·10³` passes of the loop.
///
/// **Buffer.** `prepare` allocates `⌈sr⌉ + 4` frames and writes every one
/// (the prefault); nothing else allocates. `reset` zeroes them (one pass).
/// Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct Comb {
    sample_rate: f64,
    max_delay: f64,
    line: Line,
    s: f64,
    /// The effective `damping_hz` `a` is for (NaN: none yet), and `a`.
    damping: f64,
    a: f64,
}

impl DirectPrimitive for Comb {
    fn describe() -> ModuleDescriptor {
        COMB
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.max_delay = max_frames(ctx.sample_rate);
        self.line.allocate(self.max_delay as usize);
        self.damping = f64::NAN;
        self.s = 0.0;
    }

    fn reset(&mut self) {
        self.line.clear();
        self.s = 0.0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let x = inputs.signal(0);
        let time = inputs.parameter(0);
        let feedback = inputs.parameter(1);
        let damping = inputs.parameter(2);
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            let fc = effective_cutoff(at(damping, n, DAMPING_HZ), self.sample_rate);
            if fc != self.damping {
                self.damping = fc;
                self.a = damping_coefficient(fc, self.sample_rate);
            }
            let d = delay_frames(at(time, n, COMB_TIME_MS), self.sample_rate, self.max_delay);
            let r = self.line.read_bounded(d - 1.0);
            let lp = self.s + self.a * (r - self.s);
            self.s = flush(lp);
            let input = finite_or_zero(at(x, n, 0.0));
            let y = flush(
                (input + at(feedback, n, COMB_FEEDBACK) * lp).clamp(-SIGNAL_BOUND, SIGNAL_BOUND),
            );
            self.line.push(y);
            *out = y;
        }
    }
}

/// `std.allpass_delay`: a Schroeder all-pass delay.
///
/// **Ports**
/// - `in` (signal in, audio, unbounded); unconnected it reads 0.
/// - `out` (signal out, audio).
///
/// **Parameters**
/// - `time_ms` (control, ms, range `[0, 1000]`, default 5, combine `add`):
///   the delay `d`, limited in `process` to `[1, ⌈sr⌉]` frames.
/// - `gain` (generic, no unit, range `[−0.999, 0.999]`, default 0.5, combine
///   `add`): `g`. The range stops below 1 (the executor clamps to it), so
///   the recursion always decays.
///
/// **Feedthrough:** direct (`−g·in[n]` reaches `out[n]`).
///
/// **Formula** (x = `in`, w\[j\] = 0 for j < 0, sr the sample rate):
///
/// ```text
/// d    = clamp(time_ms[n]·sr/1000, 1, ⌈sr⌉)               frames
/// r    = w[n − d]        (the bounded Hermite read of the w history, as std.comb's)
/// w[n] = clamp(x[n] + g[n]·r, −10¹², 10¹²)   (x[n] = 0 if not finite)
/// out  = clamp(−g[n]·w[n] + r, −10⁶, 10⁶)
/// ```
///
/// For a whole, fixed `d` and fixed `g` the transfer function is
/// `(−g + z^−d)/(1 − g·z^−d)`, of magnitude exactly 1 at every frequency
/// (M. R. Schroeder, "Natural sounding artificial reverberation", JAES 10,
/// 1962). At a fractional `d` the interpolated read rolls off the highest
/// frequencies a little, so the magnitude is at most 1 there instead of
/// exactly 1. `w` and `out` are set to exactly 0 below 1e-9 in magnitude.
///
/// **Bound, for any modulation.** With `M = max |x|` so far and `|g|_max`
/// the largest `|g|` so far (≤ 0.999): `|w| ≤ W = M/(1 − |g|_max)` by the
/// same induction as `std.comb`'s (the clamped read lies between two
/// earlier `w`), so `|out| ≤ |g|·W + W = (1 + |g|_max)·M/(1 − |g|_max)`.
/// For fixed settings the output's level equals the input's (all-pass);
/// the bound is what a modulated delay or gain can do at worst. The clamps
/// (`w` to ±10¹², `out` to ±10⁶, the engine's bound, `docs/semantics.md`
/// §7) act only on inputs near that size.
///
/// **Buffer.** `prepare` allocates `⌈sr⌉ + 4` frames and writes every one
/// (the prefault); nothing else allocates. `reset` zeroes them (one pass).
/// Constant work per frame.
#[derive(Clone, Debug, Default)]
pub struct AllpassDelay {
    sample_rate: f64,
    max_delay: f64,
    line: Line,
}

impl DirectPrimitive for AllpassDelay {
    fn describe() -> ModuleDescriptor {
        ALLPASS
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.max_delay = max_frames(ctx.sample_rate);
        self.line.allocate(self.max_delay as usize);
    }

    fn reset(&mut self) {
        self.line.clear();
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let x = inputs.signal(0);
        let time = inputs.parameter(0);
        let gain = inputs.parameter(1);
        for (n, out) in outputs.signal(0).iter_mut().enumerate() {
            let d = delay_frames(
                at(time, n, ALLPASS_TIME_MS),
                self.sample_rate,
                self.max_delay,
            );
            let g = at(gain, n, ALLPASS_GAIN);
            let r = self.line.read_bounded(d - 1.0);
            let input = finite_or_zero(at(x, n, 0.0));
            let w = flush((input + g * r).clamp(-STATE_BOUND, STATE_BOUND));
            self.line.push(w);
            *out = flush((-g * w + r).clamp(-SIGNAL_BOUND, SIGNAL_BOUND));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::svf::response::{amplitude, db, sine};
    use crate::testing::{Timeline, context, render};

    fn run<P: DirectPrimitive + Default>(
        input: &[f64],
        parameters: &[&[f64]],
        sample_rate: f64,
    ) -> Vec<f64> {
        let mut p = P::default();
        p.prepare(&context(sample_rate, 512, 0));
        render(
            &mut p,
            &Timeline {
                signals: &[input],
                parameters,
                triggers: &[],
            },
            input.len(),
            512,
        )
        .signals
        .remove(0)
    }

    fn comb(input: &[f64], time_ms: f64, feedback: f64, damping: f64, sr: f64) -> Vec<f64> {
        let n = input.len();
        run::<Comb>(
            input,
            &[&vec![time_ms; n], &vec![feedback; n], &vec![damping; n]],
            sr,
        )
    }

    fn allpass(input: &[f64], time_ms: f64, gain: f64, sr: f64) -> Vec<f64> {
        let n = input.len();
        run::<AllpassDelay>(input, &[&vec![time_ms; n], &vec![gain; n]], sr)
    }

    fn impulse(frames: usize) -> Vec<f64> {
        let mut x = vec![0.0; frames];
        x[0] = 1.0;
        x
    }

    /// With damping at 0.49·sr the loop low-pass is not flat, so the exact
    /// check uses the formula: at a whole delay of 10 frames the impulse
    /// response is `1` then, every 10 frames, the previous echo through the
    /// one-pole times the feedback; nothing in between before the low-pass
    /// spreads it.
    #[test]
    fn the_comb_follows_its_formula() {
        let sr = 1000.0;
        let out = comb(&impulse(200), 10.0, 0.5, 100.0, sr);
        let a = 1.0 - libm::exp(-std::f64::consts::TAU * 100.0 / sr);
        let (mut y, mut s) = (vec![0.0f64; 200], 0.0f64);
        for n in 0..200 {
            let r = if n >= 10 { y[n - 10] } else { 0.0 };
            let lp = s + a * (r - s);
            s = flush(lp);
            y[n] = flush(if n == 0 { 1.0 } else { 0.0 } + 0.5 * lp);
            assert_eq!(out[n], y[n], "frame {n}");
        }
        assert_eq!(out[0], 1.0);
        assert!(out[1..10].iter().all(|&v| v == 0.0));
    }

    /// The comb's peaks: a sine at a multiple of 1/d passes with the
    /// analytic gain `1/|1 − fb·e^{−jωd}·LP(e^{jω})|`, with
    /// `LP(z) = a/(1 − (1 − a)·z⁻¹)`, within 0.05 dB.
    #[test]
    fn the_comb_response_matches_the_analytic_one() {
        let sr = 48_000.0;
        let (time_ms, fb, damping) = (2.0, 0.7, 6000.0);
        let d = time_ms * sr / 1000.0;
        for f in [500.0, 1000.0, 1250.0, 3000.0, 7000.0] {
            let out = comb(&sine(f, sr), time_ms, fb, damping, sr);
            let measured = db(amplitude(&out[out.len() / 2..], f, sr));
            let w = std::f64::consts::TAU * f / sr;
            let a = 1.0 - libm::exp(-std::f64::consts::TAU * damping / sr);
            let (den_re, den_im) = (1.0 - (1.0 - a) * libm::cos(w), (1.0 - a) * libm::sin(w));
            let den = den_re * den_re + den_im * den_im;
            let (lp_re, lp_im) = (a * den_re / den, -a * den_im / den);
            let (c, s) = (libm::cos(w * d), -libm::sin(w * d));
            let (loop_re, loop_im) = (fb * (c * lp_re - s * lp_im), fb * (c * lp_im + s * lp_re));
            let denominator = libm::sqrt((1.0 - loop_re).powi(2) + loop_im * loop_im);
            let analytic = db(1.0 / denominator);
            assert!(
                (measured - analytic).abs() < 0.05,
                "{f} Hz: measured {measured} dB, analytic {analytic} dB"
            );
        }
    }

    /// Karplus-Strong: a 5 ms burst into a comb of 1000/220 ms at feedback
    /// 0.995 rings with a period of 218 ± 1 frames (220 Hz within 0.5 %, the
    /// low-pass's phase delay; the autocorrelation's peak) and decays: the
    /// last 0.1 s of 2 s is quieter than the first.
    #[test]
    fn karplus_strong_rings_at_its_pitch_and_decays() {
        let sr = 48_000.0;
        let frames = 2 * sr as usize;
        let mut input = vec![0.0; frames];
        let mut state = 12345u64;
        for x in input.iter_mut().take(240) {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            *x = (state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0;
        }
        let out = comb(&input, 1000.0 / 220.0, 0.995, 12_000.0, sr);
        let tail = &out[frames / 2..];
        let correlation =
            |lag: usize| -> f64 { tail.iter().zip(&tail[lag..]).map(|(a, b)| a * b).sum() };
        let period = (150..300)
            .max_by(|&a, &b| correlation(a).total_cmp(&correlation(b)))
            .unwrap();
        assert!((217..=219).contains(&period), "period {period} frames");
        let rms = |s: &[f64]| libm::sqrt(s.iter().map(|y| y * y).sum::<f64>() / s.len() as f64);
        let (early, late) = (rms(&out[240..5040]), rms(&out[frames - 4800..]));
        assert!(late < 0.5 * early, "early {early}, late {late}");
        assert!(out.iter().all(|y| y.is_finite()));
    }

    /// At the largest feedback, either sign, and the slowest damping: a
    /// constant 1 settles to the analytic DC gain `1/(1 − feedback)` (1000 at
    /// 0.999, about 0.5 at −0.999), and a ±1 square for 5 s stays finite and
    /// bounded; so does an audio-rate sweep of the delay, at 44.1 and
    /// 192 kHz.
    #[test]
    fn the_comb_is_bounded_at_every_setting() {
        for sr in [44_100.0, 192_000.0] {
            let frames = 5 * sr as usize;
            for fb in [-0.999, 0.999] {
                let out = comb(&vec![1.0; frames], 1.0, fb, 0.49 * sr, sr);
                let dc = 1.0 / (1.0 - fb);
                let last = *out.last().unwrap();
                // Within 1 %: at −0.999 the loop's transient (at odd
                // multiples of 1/(2d)) still decays as 0.999 per pass.
                assert!((last / dc - 1.0).abs() < 1e-2, "{sr} Hz, fb {fb}: {last}");
                let square: Vec<f64> = (0..frames)
                    .map(|n| if n % 97 < 48 { 1.0 } else { -1.0 })
                    .collect();
                let out = comb(&square, 1.0, fb, 0.49 * sr, sr);
                let peak = out.iter().fold(0.0f64, |m, y| m.max(y.abs()));
                assert!(
                    out.iter().all(|y| y.is_finite()) && peak < 2e4,
                    "{sr} Hz, fb {fb}: peak {peak}"
                );
            }
            let n = sr as usize;
            let square: Vec<f64> = (0..n).map(|i| if i % 7 < 4 { 1.0 } else { -1.0 }).collect();
            let time: Vec<f64> = (0..n)
                .map(|i| 500.0 + 499.9 * libm::sin(std::f64::consts::TAU * 1000.0 * i as f64 / sr))
                .collect();
            let out = run::<Comb>(&square, &[&time, &vec![0.999; n], &vec![1e9; n]], sr);
            assert!(out.iter().all(|y| y.is_finite() && y.abs() <= SIGNAL_BOUND));
        }
    }

    /// The all-pass at a whole delay: the impulse response is `−g`, then
    /// `(1 − g²)·g^(m−1)` at `m·d`, and its magnitude is 1 at every frequency
    /// (measured within 0.01 dB).
    #[test]
    fn the_allpass_is_all_pass() {
        let sr = 1000.0;
        let g = 0.6;
        let out = allpass(&impulse(100), 7.0, g, sr);
        assert_eq!(out[0], -g);
        for m in 1..10 {
            let expected = (1.0 - g * g) * libm::pow(g, (m - 1) as f64);
            approx::assert_abs_diff_eq!(out[7 * m], expected, epsilon = 1e-15);
        }
        assert!(out.iter().enumerate().all(|(n, &y)| n % 7 == 0 || y == 0.0));
        let sr = 48_000.0;
        for f in [50.0, 700.0, 3000.0, 12_000.0, 20_000.0] {
            // (Not 0.999: its transient lasts longer than the 1 s settling.)
            for g in [-0.9, 0.5, 0.9] {
                let out = allpass(&sine(f, sr), 3.0, g, sr);
                let measured = db(amplitude(&out[out.len() / 2..], f, sr));
                assert!(measured.abs() < 0.01, "{f} Hz, g {g}: {measured} dB");
            }
        }
    }

    /// The all-pass stays finite and within the bound under an audio-rate
    /// sweep of its delay and gain at their extremes.
    #[test]
    fn the_allpass_is_bounded_under_modulation() {
        for sr in [44_100.0, 192_000.0] {
            let frames = sr as usize;
            let input: Vec<f64> = (0..frames)
                .map(|n| if n % 7 < 4 { 1.0 } else { -1.0 })
                .collect();
            let time: Vec<f64> = (0..frames)
                .map(|n| 1.0 + 999.0 * (0.5 + 0.5 * libm::sin(n as f64 * 0.13)))
                .collect();
            let gain: Vec<f64> = (0..frames)
                .map(|n| 0.999 * libm::sin(n as f64 * 0.05))
                .collect();
            let out = run::<AllpassDelay>(&input, &[&time, &gain], sr);
            assert!(out.iter().all(|y| y.is_finite() && y.abs() <= SIGNAL_BOUND));
        }
    }

    /// The review's modulation cases (M2.10 review, M1): the delay
    /// alternating 1.5 and 2 frames on every frame, 20 ± 5 frames at 3 kHz
    /// and 100 ± 20 frames at 2 kHz, with `feedback`/`gain` ±0.99 and
    /// ±0.999 and the damping at 0.49·sr, 16 kHz and 12 kHz, on a ±1 square
    /// (period 37 frames) and on ±1 noise, 1 s at 48 kHz. Every comb output
    /// stays within `max|x|/(1 − |fb|)` and every all-pass output within
    /// `(1 + |g|)·max|x|/(1 − |g|)`, the bounds the rustdoc proves; the
    /// largest ratio of peak to bound is printed.
    #[test]
    fn modulated_delays_stay_within_the_proved_bounds() {
        let sr = 48_000.0;
        let frames = sr as usize;
        let ms = |d: f64| d * 1000.0 / sr;
        let tau = std::f64::consts::TAU;
        let modulations: [(&str, Vec<f64>); 3] = [
            (
                "1.5/2 frames",
                (0..frames)
                    .map(|n| ms(if n % 2 == 0 { 1.5 } else { 2.0 }))
                    .collect(),
            ),
            (
                "20±5 at 3 kHz",
                (0..frames)
                    .map(|n| ms(20.0 + 5.0 * libm::sin(tau * 3000.0 * n as f64 / sr)))
                    .collect(),
            ),
            (
                "100±20 at 2 kHz",
                (0..frames)
                    .map(|n| ms(100.0 + 20.0 * libm::sin(tau * 2000.0 * n as f64 / sr)))
                    .collect(),
            ),
        ];
        let square: Vec<f64> = (0..frames)
            .map(|n| if n % 37 < 18 { 1.0 } else { -1.0 })
            .collect();
        let mut state = 99u64;
        let noise: Vec<f64> = (0..frames)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
            })
            .collect();
        let (mut worst_comb, mut worst_allpass) = (0.0f64, 0.0f64);
        for (name, time) in &modulations {
            for input in [&square, &noise] {
                let peak_in = input.iter().fold(0.0f64, |m, x| m.max(x.abs()));
                for fb in [-0.999, -0.99, 0.99, 0.999] {
                    let constant = |v: f64| vec![v; frames];
                    for damping in [0.49 * sr, 16_000.0, 12_000.0] {
                        let out =
                            run::<Comb>(input, &[time, &constant(fb), &constant(damping)], sr);
                        let peak = out.iter().fold(0.0f64, |m, y| m.max(y.abs()));
                        let bound = peak_in / (1.0 - fb.abs());
                        assert!(peak <= bound, "comb {name}, fb {fb}, {damping} Hz: {peak}");
                        worst_comb = worst_comb.max(peak / bound);
                    }
                    let out = run::<AllpassDelay>(input, &[time, &constant(fb)], sr);
                    let peak = out.iter().fold(0.0f64, |m, y| m.max(y.abs()));
                    let bound = (1.0 + fb.abs()) * peak_in / (1.0 - fb.abs());
                    assert!(peak <= bound, "all-pass {name}, g {fb}: {peak}");
                    worst_allpass = worst_allpass.max(peak / bound);
                }
            }
        }
        eprintln!("peak/bound: comb {worst_comb:.4}, all-pass {worst_allpass:.4}");
    }

    /// Both decay to exactly 0 after an impulse (flushed, no subnormals).
    #[test]
    fn impulses_decay_to_exactly_zero() {
        let input = impulse(200_000);
        let c = comb(&input, 3.0, 0.9, 5000.0, 48_000.0);
        let a = allpass(&input, 3.0, 0.9, 48_000.0);
        for out in [c, a] {
            assert_eq!(*out.last().unwrap(), 0.0);
            assert!(out.iter().all(|y| !y.is_subnormal()));
        }
    }
}
