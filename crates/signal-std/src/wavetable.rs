//! `std.wavetable`.

use std::sync::OnceLock;

use signal_core::{
    Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortKind,
    PrepareContext,
};

use crate::line::finite_or_zero;
use crate::sine_osc::FREQ_MAX;
use crate::{BIPOLAR, at, fract, parameter, signal_port};

/// Samples per cycle of every table.
const SIZE: usize = 4096;
/// Mip levels per table: level ℓ holds harmonics 1…[`harmonics`]`(ℓ)`.
const LEVELS: usize = 10;
/// The built-in tables.
const TABLES: usize = 8;
/// The harmonics of level 0.
const TOP_HARMONIC: usize = 512;
/// The formant tables' reference fundamental, in Hz: their formants sit at
/// these frequencies when the oscillator plays it.
const FORMANT_F0: f64 = 110.0;
/// The formants (centre Hz, bandwidth Hz, gain) of the vowels a, e, i, o,
/// after Peterson and Barney's male averages (JASA 24, 1952), simplified.
const VOWELS: [[(f64, f64, f64); 3]; 4] = [
    [
        (730.0, 90.0, 1.0),
        (1090.0, 110.0, 0.5),
        (2440.0, 170.0, 0.25),
    ],
    [
        (530.0, 80.0, 1.0),
        (1840.0, 120.0, 0.45),
        (2480.0, 170.0, 0.3),
    ],
    [
        (270.0, 60.0, 1.0),
        (2290.0, 120.0, 0.4),
        (3010.0, 200.0, 0.3),
    ],
    [
        (570.0, 80.0, 1.0),
        (840.0, 100.0, 0.6),
        (2410.0, 170.0, 0.15),
    ],
];

const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    type_name: "std.wavetable",
    doc: "Mip-mapped wavetable oscillator over eight built-in tables (sine, saw, square, triangle, vowels a, e, i, o), morphed by `position`; one mip level per octave (4096 samples, 512 harmonics down to 1, Hermite reads), so at a steady pitch no harmonic exceeds sr/2.",
    ports: &[
        signal_port("pm", Direction::In, PortKind::Phase, Some("cycles"), None),
        signal_port("out", Direction::Out, PortKind::Audio, None, BIPOLAR),
    ],
    parameters: &[
        parameter(
            "freq",
            PortKind::Frequency,
            Some("Hz"),
            0.0,
            FREQ_MAX,
            440.0,
            Combine::Add,
        ),
        parameter(
            "position",
            PortKind::Generic,
            None,
            0.0,
            (TABLES - 1) as f64,
            0.0,
            Combine::Add,
        ),
    ],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: Some(crate::category::GENERATOR),
};

/// The highest harmonic of mip level `level`: `512/2^level`.
const fn harmonics(level: usize) -> usize {
    TOP_HARMONIC >> level
}

/// Every table at every level: `TABLES × LEVELS` rows of `SIZE + 1` samples
/// (the last repeats the first, for the interpolation's wrap).
#[derive(Debug)]
struct Tables {
    samples: Vec<f64>,
}

impl Tables {
    fn row(&self, table: usize, level: usize) -> &[f64] {
        let start = (table * LEVELS + level) * (SIZE + 1);
        self.samples.get(start..start + SIZE + 1).unwrap_or(&[])
    }
}

/// The amplitude of harmonic `k` of `table` (sine phase: each harmonic is
/// `aₖ·sin(2πk·p)`).
fn harmonic(table: usize, k: usize) -> f64 {
    let kf = k as f64;
    let odd = k % 2 == 1;
    let pi = std::f64::consts::PI;
    match table {
        0 => f64::from(u8::from(k == 1)),
        // 2p − 1 (std.osc's saw): −(2/π)·Σ sin(2πkp)/k.
        1 => -2.0 / (pi * kf),
        // +1 for p < ½ (std.osc's square): (4/π)·Σ_odd sin(2πkp)/k.
        2 if odd => 4.0 / (pi * kf),
        // std.osc's triangle (0 at 0, 1 at ¼): (8/π²)·Σ_odd ±sin(2πkp)/k².
        3 if odd => {
            let sign = if k % 4 == 1 { 1.0 } else { -1.0 };
            sign * 8.0 / (pi * pi * kf * kf)
        }
        4..=7 => {
            let f = kf * FORMANT_F0;
            VOWELS.get(table - 4).map_or(0.0, |formants| {
                formants
                    .iter()
                    .map(|&(centre, width, gain)| {
                        let x = (f - centre) / width;
                        gain * libm::exp(-0.5 * x * x)
                    })
                    .sum::<f64>()
            })
        }
        _ => 0.0,
    }
}

/// Builds every table by additive synthesis, from the highest level (one
/// harmonic) down, each level the one above plus its extra harmonics. The
/// sines come from a table of `sin(2π·j/SIZE)`, indexed by `k·i mod SIZE`,
/// so every level is a sum of exact grid values. Each table is then
/// scaled by `1/max|level|` over its levels, so every level lies in
/// [−1, 1] and all levels of a table share one gain.
fn build() -> Tables {
    let sine: Vec<f64> = (0..SIZE)
        .map(|j| libm::sin(std::f64::consts::TAU * j as f64 / SIZE as f64))
        .collect();
    let mut samples = vec![0.0; TABLES * LEVELS * (SIZE + 1)];
    for table in 0..TABLES {
        let mut level_values = vec![0.0; SIZE];
        let mut done = 0;
        for level in (0..LEVELS).rev() {
            for k in done + 1..=harmonics(level) {
                let a = harmonic(table, k);
                if a == 0.0 {
                    continue;
                }
                for (i, v) in level_values.iter_mut().enumerate() {
                    *v += a * sine.get(k * i % SIZE).copied().unwrap_or(0.0);
                }
            }
            done = harmonics(level);
            let start = (table * LEVELS + level) * (SIZE + 1);
            if let Some((last, body)) = samples
                .get_mut(start..start + SIZE + 1)
                .and_then(<[f64]>::split_last_mut)
            {
                body.copy_from_slice(&level_values);
                *last = level_values.first().copied().unwrap_or(0.0);
            }
        }
        let rows = table * LEVELS * (SIZE + 1)..(table + 1) * LEVELS * (SIZE + 1);
        if let Some(rows) = samples.get_mut(rows) {
            let peak = rows.iter().fold(0.0f64, |m, v| m.max(v.abs()));
            if peak > 0.0 {
                rows.iter_mut().for_each(|v| *v /= peak);
            }
        }
    }
    Tables { samples }
}

/// The tables, built once per process on the first `prepare`: they do not
/// depend on the sample rate.
fn tables() -> &'static Tables {
    static TABLES_ONCE: OnceLock<Tables> = OnceLock::new();
    TABLES_ONCE.get_or_init(build)
}

/// `row` at phase `p` ∈ [0, 1), read with the 4-point, 3rd-order Hermite
/// (Catmull-Rom) interpolator of `std.delay`, wrapping round the cycle.
fn lookup(row: &[f64], p: f64) -> f64 {
    let x = p * SIZE as f64;
    let i = (x as usize).min(SIZE - 1);
    let t = x - i as f64;
    let at = |j: usize| row.get(j % SIZE).copied().unwrap_or(0.0);
    crate::line::hermite(at(i + SIZE - 1), at(i), at(i + 1), at(i + 2), t)
}

/// `std.wavetable`: a band-limited wavetable oscillator with mip levels and
/// table morphing.
///
/// **Tables** (built once per process from additive harmonics, sine phase,
/// 4096 samples per cycle): 0 sine, 1 saw (`2p − 1`, as `std.osc`'s),
/// 2 square (+1 for p < ½), 3 triangle (`std.osc`'s), 4–7 the vowels a, e,
/// i, o: harmonic `k` has amplitude `Σ gain·exp(−½·((k·110 − F)/B)²)` over
/// three formants (F, B, gain) each, so the formants are those of the vowel
/// when the oscillator plays 110 Hz and move with the pitch, as a
/// wavetable's do. Each table is scaled by one gain so that its loudest
/// level peaks at exactly 1.
///
/// **Mip levels.** Level ℓ = 0…9 of a table holds its harmonics 1 to
/// `Kℓ = 512/2^ℓ` (512, 256, …, 1). Level ℓ is used only while
/// `Kℓ·f ≤ sr/2`, so **at a constant `freq` and with `pm` = 0, no harmonic
/// is above sr/2**. (A modulated `freq` or a `pm` input bends the phase
/// within the cycle, which widens the spectrum like any phase modulation,
/// and a jump in `pm` is a discontinuity: neither is band-limited here.)
/// With
/// `r = 1024·f/sr` (so `Kℓ·f ≤ sr/2` ⇔ `r ≤ 2^ℓ`), the brightest usable level
/// is `ℓ = max(⌈log₂ r⌉, 0)`, and the output crossfades it with the next
/// darker level `ℓ + 1` by `w = clamp(log₂ r − ℓ + 1, 0, 1)`: the harmonics
/// between `Kℓ₊₁` and `Kℓ` fade out over the octave, continuously in `f`
/// (at an octave boundary `w` = 1 below and the next level at `w` = 0 above
/// give the same table). Above `r = 256` (f > sr/4) only the sine level 9
/// remains. Below `r = ½` level 0 plays alone, so a note below `sr/2048`
/// (23 Hz at 48 kHz) is limited to 512 harmonics.
///
/// **Ports**
/// - `pm` (signal in, phase, cycles, unbounded): phase offset for this frame
///   only, as `std.osc`'s; unconnected it reads 0.
/// - `out` (signal out, audio, `[−1, 1]`).
///
/// **Parameters**
/// - `freq` (frequency, Hz, range `[0, sr/2]`, default 440, combine `add`).
///   Declared `[0, (2³² − 1)/2]` as `std.osc`'s; `process` limits it to sr/2.
/// - `position` (generic, no unit, range `[0, 7]`, default 0, combine
///   `add`): the table, morphing linearly between neighbours (1.5 is half
///   saw, half square).
///
/// **Feedthrough:** direct (`pm[n]` reaches `out[n]`).
///
/// **Formula** (φ in cycles, f64, φ\[0\] = 0, sr the sample rate):
///
/// ```text
/// f   = min(max(freq[n], 0), sr/2),   r = 1024·f/sr
/// ℓ, w as above (ℓ = 0, w = 0 when r ≤ ½);  ℓ' = min(ℓ + 1, 9)
/// i   = min(⌊position[n]⌋, 6),   t = position[n] − i
/// p   = fract(φ[n] + pm[n])       (pm[n] = 0 if not finite)
/// T(j, λ) = H(row(j, λ), p)       std.delay's 4-point Hermite at p·4096, wrapping
/// out = clamp(out, −1, 1)          (below)
/// out = (1 − t)·((1 − w)·T(i, ℓ) + w·T(i, ℓ'))
///     +      t ·((1 − w)·T(i+1, ℓ) + w·T(i+1, ℓ'))
/// φ[n+1] = fract(φ[n] + f/sr)
/// ```
///
/// The mixes over `t` and `w` are convex and the tables lie in [−1, 1]; the
/// Hermite read can overshoot a table's peak by a hair, so the output is
/// clamped to [−1, 1]. `ℓ` and `w` (one `log2`) are recomputed only on a
/// frame where `f` differs from the frame before.
///
/// **Residual aliasing.** The harmonics are all below sr/2; what remains is
/// the table interpolation's images, near `4096·f ± k·f` for harmonic `k`.
/// They are largest at low pitch, where level 0's top harmonics have the
/// fewest samples per period (8 for harmonic 512). Measured with a DFT in
/// the unit tests (saw, 48 kHz, windows on which every harmonic and alias
/// falls on a bin), the largest inharmonic component relative to the
/// fundamental:
///
/// | f (Hz) | 20.0 | 30.0 | 60.0 | 120.0 | 1230 | 4570 | 9130 |
/// |---|---:|---:|---:|---:|---:|---:|---:|
/// | dB | −98.4 | −102.1 | −116.0 | −129.1 | −173.3 | −194.7 | −206.7 |
///
/// (`bass_aliasing`, `aliasing_is_far_below_the_fundamental`; a naive
/// `2p − 1` saw reads −26.0, −15.6 and −9.5 dB at the last three). The
/// first version, with 2048 samples and linear interpolation, read −79.4 dB
/// at 30 Hz (the M2.10 review measured −80 dB at 31 Hz to −94 dB at
/// 127 Hz); doubling the tables and reading them with Hermite cost 16 table
/// reads per frame instead of 8.
/// The price is brightness: every harmonic up to sr/4 plays at full
/// amplitude, but those between sr/4 and sr/2 are faded by the crossfade
/// (at an octave boundary none is left above sr/4).
///
/// **Memory.** The tables (8 × 10 × 4097 values, 2.6 MB) are built on the
/// first `prepare` in the process and shared, read-only, by every instance;
/// an instance holds only a reference to them. Constant work per frame:
/// sixteen table reads (two tables, two levels, four points).
#[derive(Clone, Debug, Default)]
pub struct Wavetable {
    sample_rate: f64,
    phase: f64,
    tables: Option<&'static Tables>,
    /// The `f` the level and fade are for (NaN: none yet).
    freq: f64,
    level: usize,
    fade: f64,
}

/// `(ℓ, w)` for `r = 1024·f/sr`.
fn level_of(r: f64) -> (usize, f64) {
    if r.is_nan() || r <= 0.5 {
        return (0, 0.0);
    }
    let octave = libm::log2(r);
    let level = libm::ceil(octave).max(0.0);
    let fade = (octave - level + 1.0).clamp(0.0, 1.0);
    ((level as usize).min(LEVELS - 1), fade)
}

impl DirectPrimitive for Wavetable {
    fn describe() -> ModuleDescriptor {
        DESCRIPTOR
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.tables = Some(tables());
        self.freq = f64::NAN;
        self.phase = 0.0;
    }

    fn reset(&mut self) {
        self.phase = 0.0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let pm = inputs.signal(0);
        let freq = inputs.parameter(0);
        let position = inputs.parameter(1);
        let nyquist = self.sample_rate / 2.0;
        let out = outputs.signal(0);
        let Some(tables) = self.tables else {
            out.fill(0.0);
            return;
        };
        for (n, out) in out.iter_mut().enumerate() {
            let f = at(freq, n, 440.0).max(0.0).min(nyquist);
            if f != self.freq {
                self.freq = f;
                (self.level, self.fade) =
                    level_of((2 * TOP_HARMONIC) as f64 * f / self.sample_rate);
            }
            let (level, darker, w) = (self.level, (self.level + 1).min(LEVELS - 1), self.fade);
            let pos = at(position, n, 0.0).clamp(0.0, (TABLES - 1) as f64);
            let i = (libm::floor(pos) as usize).min(TABLES - 2);
            let t = pos - i as f64;
            let p = fract(self.phase + finite_or_zero(at(pm, n, 0.0)));
            let mut morph = [0.0; 2];
            for (j, value) in morph.iter_mut().enumerate() {
                let bright = lookup(tables.row(i + j, level), p);
                let dark = lookup(tables.row(i + j, darker), p);
                *value = (1.0 - w) * bright + w * dark;
            }
            let [this, next] = morph;
            *out = ((1.0 - t) * this + t * next).clamp(-1.0, 1.0);
            self.phase = fract(self.phase + f / self.sample_rate);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Timeline, context, render};

    fn osc(freq: &[f64], position: &[f64], sr: f64) -> Vec<f64> {
        let n = freq.len();
        let pm = vec![0.0; n];
        let mut wt = Wavetable::default();
        wt.prepare(&context(sr, 512, 0));
        render(
            &mut wt,
            &Timeline {
                signals: &[&pm],
                parameters: &[freq, position],
                triggers: &[],
            },
            n,
            512,
        )
        .signals
        .remove(0)
    }

    /// `|X[b]|` of a rectangular-window DFT of `x` at bin `b`, normalised so a
    /// unit sine on a bin reads 1.
    fn dft(x: &[f64], b: usize) -> f64 {
        let n = x.len();
        let (cos, sin) = unit_circle(n);
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &v) in x.iter().enumerate() {
            re += v * cos[(b * i) % n];
            im -= v * sin[(b * i) % n];
        }
        2.0 / n as f64 * libm::sqrt(re * re + im * im)
    }

    /// `cos` and `sin` of `2π·j/n` for `j < n`, cached for the last `n`.
    fn unit_circle(n: usize) -> (Vec<f64>, Vec<f64>) {
        thread_local! {
            static CACHE: std::cell::RefCell<(usize, Vec<f64>, Vec<f64>)> =
                const { std::cell::RefCell::new((0, Vec::new(), Vec::new())) };
        }
        CACHE.with(|c| {
            let mut c = c.borrow_mut();
            if c.0 != n {
                let angle = |j: usize| std::f64::consts::TAU * j as f64 / n as f64;
                *c = (
                    n,
                    (0..n).map(|j| libm::cos(angle(j))).collect(),
                    (0..n).map(|j| libm::sin(angle(j))).collect(),
                );
            }
            (c.1.clone(), c.2.clone())
        })
    }

    /// The largest inharmonic component relative to the fundamental, in dB,
    /// for a tone of `f` Hz whose window of `len` frames holds whole periods
    /// of it, so every harmonic and every alias falls on a bin.
    fn worst_alias_db(samples: &[f64], f: f64, sr: f64) -> f64 {
        let n = samples.len();
        let bin = sr / n as f64;
        let harmonic_bin = |b: usize| {
            let k = libm::round(b as f64 * bin / f);
            k >= 1.0 && (k * f / bin - b as f64).abs() < 0.5
        };
        let fundamental = dft(samples, libm::round(f / bin) as usize);
        let worst = (1..n / 2)
            .filter(|&b| !harmonic_bin(b))
            .map(|b| dft(samples, b))
            .fold(0.0f64, f64::max);
        20.0 * libm::log10(worst / fundamental)
    }

    /// A short window (4800 frames at 48 kHz, 10 Hz bins; the tone a whole
    /// number of periods long, so harmonics and aliases fall on bins): the
    /// worst inharmonic component of the saw is more than 100 dB below the
    /// fundamental at 1230, 4570 and 9130 Hz (on bins, not divisors of the
    /// rate), where a naive `2p − 1` saw's is above −40 dB.
    #[test]
    fn aliasing_is_far_below_the_fundamental() {
        let sr = 48_000.0;
        let len = 4800;
        for f in [1230.0, 4570.0, 9130.0] {
            let out = osc(&vec![f; len + 4800], &vec![1.0; len + 4800], sr);
            let measured = worst_alias_db(&out[4800..], f, sr);
            let naive: Vec<f64> = (0..len)
                .map(|n| 2.0 * fract(f * (n + 4800) as f64 / sr) - 1.0)
                .collect();
            let naive_db = worst_alias_db(&naive, f, sr);
            eprintln!(
                "std.wavetable saw at {f} Hz: worst alias {measured:.1} dB, naive {naive_db:.1} dB"
            );
            assert!(measured < -100.0, "{f} Hz: {measured} dB");
            assert!(naive_db > -40.0, "{f} Hz: naive {naive_db} dB");
        }
    }

    /// Bass aliasing (M2.10 review, L1): at low pitch level 0 (512
    /// harmonics, 4 samples per period of the top one) plays, and the
    /// interpolation's images are largest. A 4801-frame window at 48 kHz
    /// (bins of 9.998 Hz, not dividing the rate, so aliases fall between
    /// harmonics) and saws of 2, 3, 6 and 12 bins (≈ 20, 30, 60, 120 Hz).
    #[test]
    fn bass_aliasing() {
        let sr = 48_000.0;
        let len = 4801;
        for (p, limit) in [(2.0, -95.0), (3.0, -100.0), (6.0, -110.0), (12.0, -120.0)] {
            let f = p * sr / len as f64;
            let out = osc(&vec![f; len], &vec![1.0; len], sr);
            let measured = worst_alias_db(&out, f, sr);
            eprintln!("std.wavetable saw at {f:.2} Hz: worst alias {measured:.1} dB");
            assert!(measured < limit, "{f} Hz: {measured} dB");
        }
    }

    /// No harmonic above sr/2: at 5 kHz (r = 106.7, levels 7 and 8, at most
    /// 4 harmonics) harmonic 5 (25 kHz) would alias to 23 kHz; that bin is
    /// empty, while harmonic 4 (20 kHz) is present.
    #[test]
    fn levels_stop_below_nyquist() {
        let sr = 48_000.0;
        let out = osc(&vec![5000.0; 9600], &vec![1.0; 9600], sr);
        let window = &out[4800..];
        assert!(dft(window, 2000) > 1e-3);
        assert!(dft(window, 2300) < 1e-9);
        assert_eq!(level_of(1024.0 * 5000.0 / sr).0, 7);
    }

    /// The fade is continuous in frequency: at every octave boundary the
    /// tables on either side are the same, and a slow sweep over 6 octaves
    /// never jumps more than a sine of that frequency could.
    #[test]
    fn mip_levels_crossfade_continuously() {
        for octave in 0..9 {
            let r = libm::ldexp(1.0, octave);
            let below = level_of(r);
            let above = level_of(r * (1.0 + 1e-12));
            assert_eq!(below.0 + 1, above.0, "octave {octave}");
            assert!(below.1 > 1.0 - 1e-9 && above.1 < 1e-9);
        }
        let sr = 48_000.0;
        let frames = 96_000;
        let freq: Vec<f64> = (0..frames)
            .map(|n| 40.0 * libm::exp2(6.0 * n as f64 / frames as f64))
            .collect();
        let out = osc(&freq, &vec![2.0; frames], sr);
        for n in 1..frames {
            let step = (out[n] - out[n - 1]).abs();
            // A band-limited square's slope: at most (sum of harmonics)
            // 2π·f·K/sr·…; bound generously by 4·π·f·(harmonics in use)/sr.
            let limit = 4.0 * std::f64::consts::PI * freq[n] / sr * 512.0 + 0.05;
            assert!(step <= limit.min(2.0), "frame {n}: step {step}");
        }
    }

    /// Morphing is continuous in `position`: at every integer position and
    /// in between, a change of 1e-6 moves the output by at most 2e-6 on
    /// every frame (the tables are within [−1, 1]); position 1.5 is exactly
    /// the mean of tables 1 and 2.
    #[test]
    fn morphing_is_continuous() {
        let sr = 48_000.0;
        let frames = 2000;
        let freq = vec![220.0; frames];
        for pos in [0.0, 0.5, 1.0, 3.0, 5.25, 6.0, 7.0] {
            let a = osc(&freq, &vec![pos; frames], sr);
            for d in [-1e-6, 1e-6] {
                let q = (pos + d).clamp(0.0, 7.0);
                let b = osc(&freq, &vec![q; frames], sr);
                for n in 0..frames {
                    assert!((a[n] - b[n]).abs() <= 2e-6 + 1e-15, "pos {pos}, frame {n}");
                }
            }
        }
        let mid = osc(&freq, &vec![1.5; frames], sr);
        let one = osc(&freq, &vec![1.0; frames], sr);
        let two = osc(&freq, &vec![2.0; frames], sr);
        for n in 0..frames {
            approx::assert_abs_diff_eq!(mid[n], 0.5 * one[n] + 0.5 * two[n], epsilon = 1e-15);
        }
    }

    /// The tables are what they say: the saw at a low pitch tracks `2p − 1`
    /// (away from the jump) within the Gibbs ripple, and every table at
    /// every level lies in [−1, 1] with its loudest level peaking at 1.
    #[test]
    fn the_tables_are_the_named_waveforms() {
        let t = tables();
        for table in 0..TABLES {
            let peak = (0..LEVELS)
                .flat_map(|l| t.row(table, l).iter())
                .fold(0.0f64, |m, v| m.max(v.abs()));
            approx::assert_abs_diff_eq!(peak, 1.0, epsilon = 1e-12);
        }
        let saw = t.row(1, 0);
        // Level 0's peak is Gibbs' 1.09, so the table is 2p − 1 scaled by ~1/1.09.
        let scale = saw[SIZE / 4] / -0.5;
        for i in (SIZE / 8..7 * SIZE / 8).step_by(64) {
            let p = i as f64 / SIZE as f64;
            approx::assert_abs_diff_eq!(saw[i] / scale, 2.0 * p - 1.0, epsilon = 0.01);
        }
        let sine = t.row(0, 0);
        approx::assert_abs_diff_eq!(sine[SIZE / 4], 1.0, epsilon = 1e-15);
    }
}
