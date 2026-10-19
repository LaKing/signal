//! `std.fdn_reverb`.

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::line::{Line, STATE_BOUND, finite_or_zero};
use crate::svf::{CUTOFF_MAX, effective_cutoff, flush, integrator_gain};
use crate::{SIGNAL_BOUND, at, parameter, round_half_up, signal_port};

/// The number of delay lines.
const LINES: usize = 8;
/// The lines' lengths at `size` 1, in ms, ascending. Their ratios are not
/// close to small-integer ratios, so the lines' modes interleave.
const BASE_MS: [f64; LINES] = [31.7, 37.3, 41.9, 46.1, 53.9, 59.3, 67.1, 73.7];
const SIZE_MIN: f64 = 0.25;
const SIZE_MAX: f64 = 2.0;
const SIZE: f64 = 1.0;
const DECAY_MIN: f64 = 0.1;
const DECAY_MAX: f64 = 30.0;
const DECAY: f64 = 2.0;
const DAMPING_HZ: f64 = 6000.0;
const PREDELAY_MAX: f64 = 500.0;
const PREDELAY: f64 = 10.0;
const MIX: f64 = 0.3;
/// Primes are looked up beyond the longest line by this many frames, more
/// than any prime gap below 10⁹ (282).
const PRIME_MARGIN: usize = 512;
/// The input's gain into each line it feeds.
const INJECT: f64 = 0.5;
/// The output's gain on each line, `1/√8`.
const TAP: f64 = std::f64::consts::FRAC_1_SQRT_2 / 2.0;
/// `std.fdn_reverb`'s output tap signs: left `+ − + − + − + −`, right
/// `+ + − − + + − −` (two orthogonal rows of the 8×8 Hadamard matrix, so the
/// two outputs are decorrelated).
const LEFT: [f64; LINES] = [1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
const RIGHT: [f64; LINES] = [1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0];

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.fdn_reverb",
    doc: "Stereo reverb: an 8-line feedback delay network (Householder matrix, distinct prime line lengths scaled by size, per-line gain for the decay time T60 and a damping one-pole), with pre-delay and dry/wet mix.",
    ports: &[
        signal_port("in_l", Direction::In, PortKind::Audio, None, None),
        signal_port("in_r", Direction::In, PortKind::Audio, None, None),
        signal_port("out_l", Direction::Out, PortKind::Audio, None, None),
        signal_port("out_r", Direction::Out, PortKind::Audio, None, None),
    ],
    parameters: &[
        parameter(
            "size",
            PortKind::Generic,
            None,
            SIZE_MIN,
            SIZE_MAX,
            SIZE,
            Combine::Add,
        ),
        parameter(
            "decay_s",
            PortKind::Control,
            Some("s"),
            DECAY_MIN,
            DECAY_MAX,
            DECAY,
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
        parameter(
            "predelay_ms",
            PortKind::Control,
            Some("ms"),
            0.0,
            PREDELAY_MAX,
            PREDELAY,
            Combine::Add,
        ),
        parameter("mix", PortKind::Generic, None, 0.0, 1.0, MIX, Combine::Add),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::DELAY),
};

/// `std.fdn_reverb`: a stereo reverb on an 8-line feedback delay network
/// (J.-M. Jot and A. Chaigne, "Digital delay networks for designing
/// artificial reverberators", AES 90th Convention, 1991; J. O. Smith,
/// *Physical Audio Signal Processing*, "Feedback Delay Networks").
///
/// **Ports**
/// - `in_l`, `in_r` (signal in, audio, unbounded); unconnected they read 0.
///   For a mono source connect `in_l` only: the network spreads it to both
///   outputs.
/// - `out_l`, `out_r` (signal out, audio).
///
/// **Parameters**
/// - `size` (generic, no unit, range `[0.25, 2]`, default 1, combine `add`):
///   scales the line lengths, 31.7–73.7 ms at 1. Changing it moves the
///   lines' read positions by whole frames, so a modulated size clicks: it
///   is meant to be set, not swept.
/// - `decay_s` (control, s, range `[0.1, 30]`, default 2, combine `add`):
///   T60, the time for the network's low frequencies to fall by 60 dB
///   (the physical models' `decay_s`, `docs/dsp-conventions.md`).
/// - `damping_hz` (frequency, Hz, range `[1, 0.49·sr]`, default 6000,
///   combine `add`): the cutoff of each line's TPT one-pole low-pass, so high
///   frequencies decay faster than T60. Declared `[1, 0.49·(2³² − 1)]` as
///   `std.svf`'s cutoff; `process` limits it to `0.49·sr`.
/// - `predelay_ms` (control, ms, range `[0, 500]`, default 10, combine
///   `add`): delays the input of the network (not the dry signal), read
///   with `std.delay`'s Hermite interpolation.
/// - `mix` (generic, no unit, range `[0, 1]`, default 0.3, combine `add`):
///   0 dry only, 1 wet only, linear in between.
///
/// **Feedthrough:** direct (the dry part of `in_l`, `in_r` reaches the
/// outputs on the same frame).
///
/// **Formula** (sr the sample rate, all lines and states 0 at frame 0):
///
/// ```text
/// line lengths, whenever size changes (and on the first frame):
///   for i = 0…7:  tᵢ = max(round_half_up(BASE_MSᵢ·size·sr/1000), Lᵢ₋₁ + 1, 2)
///                 Lᵢ = the smallest prime ≥ tᵢ                      frames
/// gains, whenever size or decay_s changes:  gᵢ = 10^(−3·Lᵢ/(sr·decay_s))
/// G = g/(1 + g),  g = tan(π·min(max(damping_hz, 1), 0.49·sr)/sr)
///
/// per frame (xl, xr = in_l, in_r, 0 if not finite):
///   ul = Pl(p),  ur = Pr(p),  p = clamp(predelay_ms·sr/1000, 0, ⌈0.5·sr⌉)
///                    (the pre-delay lines after pushing xl, xr; Hermite)
///   rᵢ = lineᵢ[n − Lᵢ]
///   vᵢ = G·(rᵢ − sᵢ),  lpᵢ = vᵢ + sᵢ,  sᵢ ← lpᵢ + vᵢ           (damping)
///   aᵢ = gᵢ·lpᵢ
///   fᵢ = aᵢ − (2/8)·Σⱼ aⱼ + ½·(ul for i < 4, ur for i ≥ 4)   (Householder)
///   lineᵢ[n] = clamp(fᵢ, −10¹², 10¹²)
///   wet_l = (1/√8)·Σ LEFTᵢ·rᵢ,  wet_r = (1/√8)·Σ RIGHTᵢ·rᵢ
///   out_l = clamp((1 − mix)·xl + mix·wet_l, −10⁶, 10⁶), out_r likewise
/// ```
///
/// with `BASE_MS = 31.7, 37.3, 41.9, 46.1, 53.9, 59.3, 67.1, 73.7`,
/// `LEFT = + − + − + − + −`, `RIGHT = + + − − + + − −`. `A = I − (2/8)·11ᵀ`
/// is the 8×8 Householder reflection: orthogonal (lossless) and dense
/// (every line feeds every line), at 8 additions and one product per frame
/// instead of 64 products. The lengths are distinct primes, so mutually prime
/// at every size and rate, which spreads the network's modes (no common
/// period). `gᵢ` loses 60 dB in `decay_s` seconds whatever the line's length
/// (Jot's rule), so every line decays at the same rate at DC.
///
/// **Damping per line.** All eight lines share one `damping_hz`, and its
/// one-pole acts once per pass round a line. A short line makes more passes
/// per second, so its high frequencies decay faster than a long line's: the
/// high-frequency decay time of each line is proportional to its length,
/// and the shortest line's (31.7 ms at size 1) is about 2.3 times shorter
/// than the longest's (73.7 ms). The difference is small next to T60 at low
/// frequencies, which `gᵢ` equalises; a frequency-dependent T60 per line
/// (Jot's absorbent filters) is not implemented.
///
/// **Loop gain < 1 at every setting.** One pass round the network is
/// `A·diag(gᵢ·LPᵢ(ω)·z^−Lᵢ)`. `A` is orthogonal (norm 1), `|LPᵢ(ω)| ≤ 1`, and
/// `gᵢ = 10^(−3·Lᵢ/(sr·decay_s)) < 1` for every `Lᵢ ≥ 2` and finite
/// `decay_s`, so for fixed parameters the loop's gain at every frequency is
/// at most `max gᵢ < 1` and the network is stable (small-gain theorem).
/// The largest `gᵢ` belongs to the shortest line at the longest decay:
/// `L₀ ≥ round_half_up(7.925·10⁻³·sr)`, so `max gᵢ ≤ 10^(−3·7.9·10⁻³/30)
/// ≈ 0.99818` at any rate from 44.1 kHz up. When parameters change the
/// clamp of the lines to ±10¹² keeps the state finite, and the output clamp
/// keeps it within the engine's bound (`docs/semantics.md` §7).
///
/// Each damping state `sᵢ` and each line value whose magnitude falls below
/// 1e-9 is set to exactly 0 (`docs/dsp-conventions.md`, "Denormals").
///
/// **Memory.** `prepare` allocates the eight lines (`⌈0.1474·sr⌉ + 524`
/// frames each: the longest target, 8 frames of room for distinct primes,
/// the 512-frame prime margin and the interpolator's 4), the two pre-delay
/// lines (`⌈0.5·sr⌉ + 5`), writing every element of each (the prefault, so
/// no page is first touched on the audio thread), and a table of the
/// smallest prime at or above every length (a sieve of Eratosthenes), so a
/// change of `size` costs eight table reads. Nothing else allocates;
/// `reset` zeroes the lines (one pass). `pow` (8, on a size or decay change)
/// and `tan` (on a damping change) are `libm`'s. Constant work per frame:
/// two Hermite reads, eight one-poles and the 8-line mix.
#[derive(Clone, Debug, Default)]
pub struct FdnReverb {
    sample_rate: f64,
    lines: [Line; LINES],
    pre: [Line; 2],
    /// `next_prime[t]`: the smallest prime ≥ `t` (or `t` itself if none is
    /// in the table).
    next_prime: Vec<u32>,
    max_predelay: f64,
    lengths: [usize; LINES],
    gains: [f64; LINES],
    damping: [f64; LINES],
    /// The `size`, `decay_s` and effective `damping_hz` the coefficients are
    /// for (NaN: none yet).
    key: [f64; 3],
    one_pole: f64,
}

/// The smallest prime at or above every `t` in `0..len`, from a sieve of
/// Eratosthenes over `0..len` (`t` itself where no prime is left).
fn next_primes(len: usize) -> Vec<u32> {
    let mut composite = vec![false; len];
    for c in composite.iter_mut().take(2) {
        *c = true;
    }
    let mut p = 2;
    while p * p < len {
        if !composite.get(p).copied().unwrap_or(true) {
            let mut m = p * p;
            while m < len {
                if let Some(c) = composite.get_mut(m) {
                    *c = true;
                }
                m += p;
            }
        }
        p += 1;
    }
    let mut next = vec![0u32; len];
    let mut prime = None;
    for t in (0..len).rev() {
        if !composite.get(t).copied().unwrap_or(true) {
            prime = Some(t);
        }
        if let Some(slot) = next.get_mut(t) {
            *slot = u32::try_from(prime.unwrap_or(t)).unwrap_or(u32::MAX);
        }
    }
    next
}

impl FdnReverb {
    /// The longest target length at `sample_rate`, in frames.
    fn longest(sample_rate: f64) -> usize {
        libm::ceil(BASE_MS[LINES - 1] * SIZE_MAX * sample_rate / 1000.0) as usize + LINES
    }

    /// Recomputes the line lengths and gains for `size` and `decay_s`.
    fn retune(&mut self, size: f64, decay: f64) {
        let top = self.next_prime.len().saturating_sub(1);
        let mut previous = 1;
        for (i, base) in BASE_MS.iter().enumerate() {
            let target = (round_half_up(base * size * self.sample_rate / 1000.0) as usize)
                .max(previous + 1)
                .max(2)
                .min(top);
            let length = self.next_prime.get(target).map_or(target, |&p| p as usize);
            if let (Some(l), Some(g)) = (self.lengths.get_mut(i), self.gains.get_mut(i)) {
                *l = length;
                *g = libm::pow(10.0, -3.0 * length as f64 / (self.sample_rate * decay));
            }
            previous = length;
        }
    }
}

impl DirectPrimitive for FdnReverb {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        let longest = Self::longest(ctx.sample_rate);
        self.next_prime = next_primes(longest + PRIME_MARGIN);
        for line in &mut self.lines {
            line.allocate(longest + PRIME_MARGIN);
        }
        self.max_predelay = libm::ceil(PREDELAY_MAX / 1000.0 * ctx.sample_rate);
        for line in &mut self.pre {
            line.allocate(self.max_predelay as usize + 1);
        }
        self.key = [f64::NAN; 3];
        self.damping = [0.0; LINES];
    }

    fn reset(&mut self) {
        for line in self.lines.iter_mut().chain(self.pre.iter_mut()) {
            line.clear();
        }
        self.damping = [0.0; LINES];
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let (in_l, in_r) = (inputs.signal(0), inputs.signal(1));
        let size = inputs.parameter(0);
        let decay = inputs.parameter(1);
        let damping = inputs.parameter(2);
        let predelay = inputs.parameter(3);
        let mix = inputs.parameter(4);
        let mut signals = outputs.signals();
        let (Some(out_l), Some(out_r)) = (signals.next(), signals.next()) else {
            return;
        };
        for (n, (out_l, out_r)) in out_l.iter_mut().zip(out_r.iter_mut()).enumerate() {
            let fc = effective_cutoff(at(damping, n, DAMPING_HZ), self.sample_rate);
            let key = [at(size, n, SIZE), at(decay, n, DECAY), fc];
            if key != self.key {
                if key[0] != self.key[0] || key[1] != self.key[1] {
                    self.retune(key[0], key[1]);
                }
                if key[2] != self.key[2] {
                    let g = integrator_gain(fc, self.sample_rate);
                    self.one_pole = g / (1.0 + g);
                }
                self.key = key;
            }
            let xl = finite_or_zero(at(in_l, n, 0.0));
            let xr = finite_or_zero(at(in_r, n, 0.0));
            let [pre_l, pre_r] = &mut self.pre;
            pre_l.push(xl);
            pre_r.push(xr);
            let p = (at(predelay, n, PREDELAY) * self.sample_rate / 1000.0)
                .max(0.0)
                .min(self.max_predelay);
            let (ul, ur) = (pre_l.read(p), pre_r.read(p));

            let mut r = [0.0; LINES];
            let mut a = [0.0; LINES];
            let mut sum = 0.0;
            for i in 0..LINES {
                let (Some(line), Some(&length), Some(s), Some(&g), Some(ri), Some(ai)) = (
                    self.lines.get(i),
                    self.lengths.get(i),
                    self.damping.get_mut(i),
                    self.gains.get(i),
                    r.get_mut(i),
                    a.get_mut(i),
                ) else {
                    continue;
                };
                *ri = line.sample(length.saturating_sub(1));
                let v = self.one_pole * (*ri - *s);
                let lp = v + *s;
                *s = flush((lp + v).clamp(-STATE_BOUND, STATE_BOUND));
                *ai = g * lp;
                sum += *ai;
            }
            let reflect = sum * (2.0 / LINES as f64);
            let (mut wet_l, mut wet_r) = (0.0, 0.0);
            for (i, line) in self.lines.iter_mut().enumerate() {
                let input = if i < LINES / 2 { ul } else { ur };
                let ai = a.get(i).copied().unwrap_or(0.0);
                line.push(flush(
                    (ai - reflect + INJECT * input).clamp(-STATE_BOUND, STATE_BOUND),
                ));
                let ri = r.get(i).copied().unwrap_or(0.0);
                wet_l += LEFT.get(i).copied().unwrap_or(0.0) * ri;
                wet_r += RIGHT.get(i).copied().unwrap_or(0.0) * ri;
            }
            let m = at(mix, n, MIX);
            *out_l = ((1.0 - m) * xl + m * TAP * wet_l).clamp(-SIGNAL_BOUND, SIGNAL_BOUND);
            *out_r = ((1.0 - m) * xr + m * TAP * wet_r).clamp(-SIGNAL_BOUND, SIGNAL_BOUND);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    /// `[out_l, out_r]` for the inputs with constant parameters
    /// `[size, decay_s, damping_hz, predelay_ms, mix]`.
    fn reverb(left: &[f64], right: &[f64], p: [f64; 5], sr: f64) -> Vec<Vec<f64>> {
        let n = left.len();
        let columns: Vec<Vec<f64>> = p.iter().map(|&v| vec![v; n]).collect();
        let parameters: Vec<&[f64]> = columns.iter().map(Vec::as_slice).collect();
        let mut fdn = FdnReverb::default();
        fdn.prepare(&context(sr, 512, 0));
        render(
            &mut fdn,
            &Timeline {
                signals: &[left, right],
                parameters: &parameters,
                triggers: &[],
            },
            n,
            512,
        )
        .signals
    }

    fn energy(s: &[f64]) -> f64 {
        s.iter().map(|y| y * y).sum()
    }

    fn is_prime(n: usize) -> bool {
        n >= 2
            && (2..)
                .take_while(|d| d * d <= n)
                .all(|d| !n.is_multiple_of(d))
    }

    /// At every rate tested and across the size range, the eight lengths are
    /// distinct primes in ascending order, each the smallest prime at or above
    /// its target, and every gain is below 1, at most 0.9982.
    #[test]
    fn lengths_are_distinct_primes_and_every_gain_is_below_1() {
        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut fdn = FdnReverb::default();
            fdn.prepare(&context(sr, 64, 0));
            for size in [SIZE_MIN, 0.5, 0.777, 1.0, 1.5, SIZE_MAX] {
                fdn.retune(size, DECAY_MAX);
                let l = fdn.lengths;
                assert!(
                    l.iter().all(|&x| is_prime(x)),
                    "{sr} Hz, size {size}: {l:?}"
                );
                assert!(l.windows(2).all(|w| w[0] < w[1]), "{l:?}");
                let mut previous = 1;
                for (i, &x) in l.iter().enumerate() {
                    let target =
                        (round_half_up(BASE_MS[i] * size * sr / 1000.0) as usize).max(previous + 1);
                    assert!(x >= target && (target..x).all(|t| !is_prime(t)), "line {i}");
                    previous = x;
                }
                let worst = fdn.gains.iter().fold(0.0f64, |m, &g| m.max(g));
                assert!(worst < 0.99820, "{sr} Hz, size {size}: {worst}");
            }
        }
    }

    /// The sieve agrees with trial division.
    #[test]
    fn the_prime_table_is_right() {
        let next = next_primes(2000);
        for (t, &p) in next.iter().enumerate().take(1990) {
            let p = p as usize;
            assert!(p >= t && is_prime(p) && (t..p).all(|q| !is_prime(q)), "{t}");
        }
    }

    /// After an impulse the tail's energy falls: in every 100 ms window from
    /// 0.3 s to 2 s it is below the window before, and from 0.5 s to 1.5 s
    /// (decay_s 1) it falls by 60 dB within 10 dB (the damping makes the high
    /// frequencies fall faster than T60, which applies at DC).
    #[test]
    fn an_impulse_decays_at_the_decay_time() {
        let sr = 48_000.0;
        let frames = 2 * sr as usize;
        let mut left = vec![0.0; frames];
        left[0] = 1.0;
        let right = vec![0.0; frames];
        let out = reverb(&left, &right, [1.0, 1.0, 12_000.0, 0.0, 1.0], sr);
        let w = sr as usize / 10;
        for channel in &out {
            let windows: Vec<f64> = (3..20)
                .map(|k| energy(&channel[k * w..(k + 1) * w]))
                .collect();
            assert!(windows.windows(2).all(|p| p[1] < p[0]), "{windows:?}");
            let drop = 10.0 * libm::log10(windows[2] / windows[12]);
            assert!((drop - 60.0).abs() < 10.0, "fell {drop} dB in 1 s");
        }
    }

    /// No runaway at the most resonant settings (largest size, 30 s decay,
    /// no damping to speak of, all wet): 60 s of ±1 noise into both inputs
    /// at 44.1 kHz keep every sample finite and the level steady (the RMS of
    /// the last 10 s within 3 dB of that of seconds 30 to 40, below the
    /// small-gain bound), and when the input stops the energy falls.
    #[test]
    fn no_runaway_for_60_s_at_the_largest_settings() {
        let sr = 44_100.0;
        let frames = 60 * sr as usize;
        let tail = 10 * sr as usize;
        let mut state = 7u64;
        let mut noise = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        };
        let mut left: Vec<f64> = (0..frames).map(|_| noise()).collect();
        let mut right: Vec<f64> = (0..frames).map(|_| noise()).collect();
        left.extend(std::iter::repeat_n(0.0, tail));
        right.extend(std::iter::repeat_n(0.0, tail));
        let out = reverb(
            &left,
            &right,
            [SIZE_MAX, DECAY_MAX, 0.49 * sr, PREDELAY_MAX, 1.0],
            sr,
        );
        let second = sr as usize;
        for channel in &out {
            assert!(channel.iter().all(|y| y.is_finite()));
            let rms = |a: usize, b: usize| {
                libm::sqrt(energy(&channel[a * second..b * second]) / ((b - a) * second) as f64)
            };
            let (middle, end) = (rms(30, 40), rms(50, 60));
            assert!(
                (20.0 * libm::log10(end / middle)).abs() < 3.0,
                "{middle} → {end}"
            );
            // A loose small-gain bound on the RMS: the input's RMS (1/√3)
            // times the injection (½) and the loop's largest gain
            // 1/(1 − 0.9982), times 2 for the two inputs.
            assert!(
                end < 0.5 / libm::sqrt(3.0) * 2.0 / (1.0 - 0.9982),
                "rms {end}"
            );
            let after = rms(60, 70);
            assert!(after < end, "{end} → {after} after the input stopped");
        }
    }

    /// Mix 0 is the dry signal exactly, and the wet signal starts after the
    /// pre-delay plus the shortest line.
    #[test]
    fn mix_and_predelay() {
        let sr = 48_000.0;
        let left: Vec<f64> = (0..4800).map(|n| libm::sin(n as f64 * 0.1)).collect();
        let right: Vec<f64> = (0..4800).map(|n| libm::cos(n as f64 * 0.07)).collect();
        let dry = reverb(&left, &right, [1.0, 2.0, 6000.0, 20.0, 0.0], sr);
        assert_eq!(dry, vec![left.clone(), right.clone()]);
        let mut impulse = vec![0.0; 9600];
        impulse[0] = 1.0;
        let wet = reverb(
            &impulse,
            &vec![0.0; 9600],
            [1.0, 2.0, 6000.0, 20.0, 1.0],
            sr,
        );
        let first = wet[0].iter().position(|&y| y != 0.0).unwrap();
        // 20 ms = 960 frames, then L₀ = the smallest prime ≥ 1522.
        assert_eq!(first, 960 + 1523);
    }

    /// A non-finite input in a direct call is taken as 0.
    #[test]
    fn a_non_finite_input_is_taken_as_0() {
        let left: Vec<f64> = (0..3000)
            .map(|n| ((n * 7) % 5) as f64 / 2.0 - 1.0)
            .collect();
        let mut poisoned = left.clone();
        let mut zeroed = left.clone();
        poisoned[5] = f64::NAN;
        zeroed[5] = 0.0;
        let p = [0.5, 3.0, 4000.0, 1.0, 0.7];
        let got = reverb(&poisoned, &left, p, 48_000.0);
        assert_eq!(got, reverb(&zeroed, &left, p, 48_000.0));
    }
}
