//! The 4× oversampled ("true") peak.

/// Taps on each side of an interpolated point.
const HALF_TAPS: usize = 8;
/// The oversampling factor.
const FACTOR: usize = 4;

/// The 4× oversampled peak of each channel of `samples`, interleaved by
/// frame with `channels` channels, as `audio.wav` stores them.
///
/// Between every two samples three points are interpolated, at ¼, ½ and ¾ of
/// a frame, with a Hann-windowed sinc of 16 taps (8 on each side); samples
/// beyond either end read as 0. The original samples are included, so the
/// result is never below the sample peak. Non-finite samples are skipped
/// (they are counted by [`metrics`](crate::metrics)). A trailing partial frame
/// is ignored.
pub fn true_peak_4x(samples: &[f32], channels: u16) -> Vec<f64> {
    let width = usize::from(channels);
    let frames = samples.len().checked_div(width).unwrap_or(0);
    let kernels: Vec<[f64; 2 * HALF_TAPS]> = (1..FACTOR).map(kernel).collect();
    (0..width)
        .map(|channel| {
            let x: Vec<f64> = samples
                .iter()
                .skip(channel)
                .step_by(width)
                .take(frames)
                .map(|&s| if s.is_finite() { f64::from(s) } else { 0.0 })
                .collect();
            let mut peak = x.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
            for n in 0..x.len() {
                // The points between x[n] and x[n + 1] use x[n − 7] ..= x[n + 8].
                let start = n as isize - HALF_TAPS as isize + 1;
                for taps in &kernels {
                    let y: f64 = taps
                        .iter()
                        .enumerate()
                        .map(|(t, h)| {
                            let i = start + t as isize;
                            usize::try_from(i)
                                .ok()
                                .and_then(|i| x.get(i))
                                .map_or(0.0, |v| h * v)
                        })
                        .sum();
                    peak = peak.max(y.abs());
                }
            }
            peak
        })
        .collect()
}

/// The 16 taps for the point `phase/4` of a frame after a sample.
fn kernel(phase: usize) -> [f64; 2 * HALF_TAPS] {
    let fraction = phase as f64 / FACTOR as f64;
    let mut taps = [0.0; 2 * HALF_TAPS];
    for (t, tap) in taps.iter_mut().enumerate() {
        // Distance from the interpolated point to tap t, in frames.
        let d = (t as f64 - (HALF_TAPS as f64 - 1.0)) - fraction;
        let sinc = (std::f64::consts::PI * d).sin() / (std::f64::consts::PI * d);
        let window = 0.5 + 0.5 * (std::f64::consts::PI * d / HALF_TAPS as f64).cos();
        *tap = sinc * window;
    }
    // Unity gain at DC, so a constant is interpolated exactly.
    let sum: f64 = taps.iter().sum();
    taps.map(|tap| tap / sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sine at a quarter of the sample rate, sampled 45° off its peaks:
    /// every sample is ±0.7071, the true peak is 1. It fades in and out over
    /// 512 frames, so the ends of the signal do not ring.
    #[test]
    fn finds_the_peak_between_samples() {
        let (frames, fade) = (4096, 512);
        let samples: Vec<f32> = (0..frames)
            .map(|n| {
                let edge = n.min(frames - 1 - n);
                let gain = if edge < fade {
                    0.5 - 0.5 * (std::f64::consts::PI * edge as f64 / fade as f64).cos()
                } else {
                    1.0
                };
                let x =
                    (std::f64::consts::FRAC_PI_2 * n as f64 + std::f64::consts::FRAC_PI_4).sin();
                (gain * x) as f32
            })
            .collect();
        let sample_peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((f64::from(sample_peak) - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        let peak = true_peak_4x(&samples, 1)[0];
        assert!((peak - 1.0).abs() < 1e-2, "{peak}");
    }

    #[test]
    fn never_below_the_sample_peak_and_per_channel() {
        let samples = [0.5f32, -0.25, 0.0, 0.0, 0.0, 0.0];
        let peaks = true_peak_4x(&samples, 2);
        assert_eq!(peaks.len(), 2);
        assert!(peaks[0] >= 0.5 && peaks[1] >= 0.25);
        assert!(true_peak_4x(&[], 2).iter().all(|&p| p == 0.0));
    }
}
