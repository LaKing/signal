//! Spectral and pitch analysis of a tone: how much of its energy is
//! inharmonic (aliasing, for a shaped sine), and its frequency by zero
//! crossings.

use realfft::RealFftPlanner;

/// Bins within this distance of a harmonic belong to it (the Hann window's
/// main lobe is ±2 bins wide).
const HARMONIC_BINS: f64 = 2.0;

/// The result of [`aliasing`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aliasing {
    /// The energy of the bins within ±2 bins of the fundamental.
    pub fundamental: f64,
    /// The energy of the bins further than ±2 bins from every harmonic.
    pub inharmonic: f64,
    /// `10·log10(fundamental / inharmonic)`: how far the inharmonic energy is
    /// below the fundamental, in dB. +∞ when there is no inharmonic energy.
    pub ratio_db: f64,
}

/// Measures the inharmonic energy of a tone with fundamental `f0` Hz.
///
/// The samples are windowed with a periodic Hann window
/// `w[n] = 0.5 − 0.5·cos(2πn/N)` (N the number of samples) and transformed
/// with a real FFT; the energy of bin *b* (0 ≤ *b* ≤ N/2) is `|X[b]|²`. With
/// `c = f0·N/sample_rate`, the bin position of the fundamental:
///
/// - the **fundamental** energy is that of the bins with `|b − c| ≤ 2`;
/// - the **inharmonic** energy is that of the bins with `|b − k·c| > 2` for
///   every harmonic `k ≥ 1` with `k·f0 < sample_rate/2`. DC is not a
///   harmonic, so energy at DC counts as inharmonic.
///
/// A harmonic that aliases lands somewhere else in the spectrum, usually away
/// from every harmonic, and is counted as inharmonic. An alias that lands
/// exactly on a harmonic is not seen: when `sample_rate/f0` is a whole number
/// every alias does (`docs/testing-and-evaluation.md`, V6). Use a whole number
/// of periods of `f0`, or the window's leakage is counted too.
///
/// Returns `None` if `samples` is empty or `f0` is not in `(0, sample_rate/2)`.
pub fn aliasing(samples: &[f64], sample_rate: f64, f0: f64) -> Option<Aliasing> {
    let n = samples.len();
    let nyquist = sample_rate / 2.0;
    if n == 0 || !(f0 > 0.0 && f0 < nyquist) {
        return None;
    }
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n);
    let mut input: Vec<f64> = samples
        .iter()
        .enumerate()
        .map(|(i, &x)| {
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
            w * x
        })
        .collect();
    let mut spectrum = fft.make_output_vec();
    fft.process(&mut input, &mut spectrum).ok()?;

    let c = f0 * n as f64 / sample_rate;
    let harmonics = (nyquist / f0).ceil() as usize; // k = 1 ..= harmonics, filtered below
    let near_harmonic = |b: f64| {
        (1..=harmonics)
            .filter(|&k| k as f64 * f0 < nyquist)
            .any(|k| (b - k as f64 * c).abs() <= HARMONIC_BINS)
    };
    let (mut fundamental, mut inharmonic) = (0.0, 0.0);
    for (b, x) in spectrum.iter().enumerate() {
        let b = b as f64;
        let energy = x.norm_sqr();
        if (b - c).abs() <= HARMONIC_BINS {
            fundamental += energy;
        }
        if !near_harmonic(b) {
            inharmonic += energy;
        }
    }
    let ratio_db = if inharmonic == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (fundamental / inharmonic).log10()
    };
    Some(Aliasing {
        fundamental,
        inharmonic,
        ratio_db,
    })
}

/// The frequency of `samples` in Hz, from its upward zero crossings: each is
/// located by linear interpolation between the two samples around it, and the
/// frequency is `(crossings − 1)·sample_rate / (last − first)`. `None` with
/// fewer than two crossings.
pub fn zero_crossing_frequency(samples: &[f64], sample_rate: f64) -> Option<f64> {
    let crossings: Vec<f64> = samples
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
        .map(|(n, w)| n as f64 + w[0] / (w[0] - w[1]))
        .collect();
    match (crossings.first(), crossings.last()) {
        (Some(first), Some(last)) if crossings.len() >= 2 => {
            Some((crossings.len() - 1) as f64 * sample_rate / (last - first))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 96_000.0;

    fn sine(freq: f64, amplitude: f64, n: usize) -> Vec<f64> {
        (0..n)
            .map(|i| amplitude * (std::f64::consts::TAU * freq * i as f64 / SR).sin())
            .collect()
    }

    fn add(a: &[f64], b: &[f64]) -> Vec<f64> {
        a.iter().zip(b).map(|(x, y)| x + y).collect()
    }

    #[test]
    fn a_pure_sine_has_no_inharmonic_energy_to_speak_of() {
        let a = aliasing(&sine(1009.0, 1.0, 96_000), SR, 1009.0).unwrap();
        assert!(a.ratio_db > 150.0, "{a:?}");
    }

    /// An inharmonic tone 60 dB below the fundamental (amplitude 1e-3) reads
    /// 60 dB; the window's energy factor is the same for both.
    #[test]
    fn an_inharmonic_tone_at_minus_60_db_reads_60_db() {
        let signal = add(&sine(1009.0, 1.0, 96_000), &sine(1500.0, 1e-3, 96_000));
        let a = aliasing(&signal, SR, 1009.0).unwrap();
        approx::assert_abs_diff_eq!(a.ratio_db, 60.0, epsilon = 0.1);
    }

    #[test]
    fn harmonics_below_nyquist_are_not_inharmonic() {
        let mut signal = sine(1009.0, 1.0, 96_000);
        for k in [2.0, 3.0, 47.0] {
            signal = add(&signal, &sine(k * 1009.0, 0.1, 96_000));
        }
        let a = aliasing(&signal, SR, 1009.0).unwrap();
        assert!(a.ratio_db > 150.0, "{a:?}");
    }

    #[test]
    fn invalid_input_gives_none() {
        assert_eq!(aliasing(&[], SR, 1000.0), None);
        assert_eq!(aliasing(&[0.0; 16], SR, 0.0), None);
        assert_eq!(aliasing(&[0.0; 16], SR, SR / 2.0), None);
    }

    #[test]
    fn zero_crossings_measure_the_frequency() {
        let f = zero_crossing_frequency(&sine(48.0, 0.3, 96_000), SR).unwrap();
        approx::assert_relative_eq!(f, 48.0, max_relative = 1e-6);
        assert_eq!(zero_crossing_frequency(&[1.0, 2.0, 3.0], SR), None);
    }
}
