//! Signal evaluation: audio analysis and metrics (peaks, DC, non-finite values,
//! spectra for aliasing checks), later comparisons and listening packages.
//! Metrics detect defects; humans judge quality.
//!
//! M1: [`metrics`], the per-channel level and defect counts written to
//! `metrics.json`; [`aliasing`] (inharmonic energy of a tone),
//! [`zero_crossing_frequency`] and [`true_peak_4x`], used by the acceptance
//! tests (`docs/testing-and-evaluation.md`). Compare comes with M3.

mod aliasing;
mod peak;

pub use aliasing::{Aliasing, aliasing, zero_crossing_frequency};
pub use peak::true_peak_4x;

use std::fs;
use std::io;
use std::path::Path;

use serde::Serialize;

/// The metrics of one render, written as `metrics.json`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Metrics {
    /// The number of frames analysed.
    pub frames: u64,
    /// One entry per channel, in channel order.
    pub channels: Vec<ChannelMetrics>,
}

/// The metrics of one channel. Peak, RMS and mean are taken over the finite
/// samples only (0 when there are none); non-finite samples are counted.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChannelMetrics {
    /// The largest absolute value.
    pub peak: f64,
    /// The root mean square.
    pub rms: f64,
    /// The mean (DC offset).
    pub mean: f64,
    /// The number of NaN or infinite samples.
    pub non_finite: u64,
    /// The number of subnormal samples.
    pub subnormal: u64,
}

/// The metrics of `samples`, interleaved by frame with `channels` channels, as
/// `audio.wav` stores them (32-bit float). Sums are accumulated in f64 in
/// sample order. A trailing partial frame is ignored.
pub fn metrics(samples: &[f32], channels: u16) -> Metrics {
    let width = usize::from(channels);
    let frames = samples.len().checked_div(width).unwrap_or(0);
    let channels = (0..width)
        .map(|channel| {
            let values = samples
                .iter()
                .skip(channel)
                .step_by(width)
                .take(frames)
                .copied();
            channel_metrics(values)
        })
        .collect();
    Metrics {
        frames: frames as u64,
        channels,
    }
}

fn channel_metrics(values: impl Iterator<Item = f32>) -> ChannelMetrics {
    let (mut peak, mut sum, mut squares) = (0.0_f64, 0.0_f64, 0.0_f64);
    let (mut finite, mut non_finite, mut subnormal) = (0_u64, 0_u64, 0_u64);
    for value in values {
        if !value.is_finite() {
            non_finite += 1;
            continue;
        }
        if value.is_subnormal() {
            subnormal += 1;
        }
        let x = f64::from(value);
        peak = peak.max(x.abs());
        sum += x;
        squares += x * x;
        finite += 1;
    }
    let (mean, rms) = if finite == 0 {
        (0.0, 0.0)
    } else {
        let n = finite as f64;
        (sum / n, (squares / n).sqrt())
    };
    ChannelMetrics {
        peak,
        rms,
        mean,
        non_finite,
        subnormal,
    }
}

impl Metrics {
    /// Writes the metrics to `path` as pretty-printed JSON with a final newline.
    pub fn write(&self, path: &Path) -> io::Result<()> {
        let mut json = serde_json::to_vec_pretty(self)?;
        json.push(b'\n');
        fs::write(path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_rms_and_mean_per_channel() {
        // Channel 0: 1, -1 (peak 1, RMS 1, mean 0); channel 1: 0.5, 0.5.
        let m = metrics(&[1.0, 0.5, -1.0, 0.5], 2);
        assert_eq!(m.frames, 2);
        assert_eq!(
            m.channels,
            [
                ChannelMetrics {
                    peak: 1.0,
                    rms: 1.0,
                    mean: 0.0,
                    non_finite: 0,
                    subnormal: 0,
                },
                ChannelMetrics {
                    peak: 0.5,
                    rms: 0.5,
                    mean: 0.5,
                    non_finite: 0,
                    subnormal: 0,
                },
            ]
        );
    }

    #[test]
    fn non_finite_samples_are_counted_and_excluded() {
        let m = metrics(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.25], 1);
        let c = &m.channels[0];
        assert_eq!(c.non_finite, 3);
        assert_eq!((c.peak, c.rms, c.mean), (0.25, 0.25, -0.25));
    }

    #[test]
    fn subnormal_samples_are_counted() {
        let m = metrics(&[1e-40, f32::MIN_POSITIVE, 0.0, -1e-42], 1);
        assert_eq!(m.channels[0].subnormal, 2);
        assert_eq!(m.channels[0].non_finite, 0);
    }

    #[test]
    fn empty_input_is_all_zero() {
        let m = metrics(&[], 2);
        assert_eq!(m.frames, 0);
        for c in &m.channels {
            assert_eq!(
                (c.peak, c.rms, c.mean, c.non_finite, c.subnormal),
                (0.0, 0.0, 0.0, 0, 0)
            );
        }
        assert!(metrics(&[1.0], 0).channels.is_empty());
    }
}
