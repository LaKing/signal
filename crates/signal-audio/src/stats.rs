//! Callback statistics (statistics only, never semantics): how long each
//! callback took against its period, and the xruns.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Serialize;

/// Counters shared between the audio callback, which records, and the control
/// thread, which reads. Atomics only: recording allocates, locks and blocks
/// nowhere.
#[derive(Debug, Default)]
pub struct CallbackStats {
    callbacks: AtomicU64,
    frames: AtomicU64,
    overruns: AtomicU64,
    stream_errors: AtomicU64,
    /// The worst duration / period, as f64 bits (finite and ≥ 0, so the bits
    /// order like the values).
    worst: AtomicU64,
}

/// What [`CallbackStats`] counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct StatsSnapshot {
    /// Callbacks recorded.
    pub callbacks: u64,
    /// Frames they held.
    pub frames: u64,
    /// `stream_errors + overruns`.
    pub xruns: u64,
    /// Errors the device stream reported (CoreAudio overloads among them).
    pub stream_errors: u64,
    /// Callbacks whose duration exceeded their period.
    pub overruns: u64,
    /// The worst callback duration as a fraction of its period.
    pub worst_callback_ratio: f64,
}

impl CallbackStats {
    /// Records a callback of `frames` frames at `sample_rate` that took
    /// `elapsed`. A callback without a period (no frames) is counted, but has
    /// no ratio.
    pub fn record(&self, frames: usize, elapsed: Duration, sample_rate: u32) {
        self.callbacks.fetch_add(1, Ordering::Relaxed);
        self.frames.fetch_add(frames as u64, Ordering::Relaxed);
        let period = frames as f64 / f64::from(sample_rate);
        let ratio = elapsed.as_secs_f64() / period;
        if period > 0.0 && ratio.is_finite() {
            self.worst.fetch_max(ratio.to_bits(), Ordering::Relaxed);
            if ratio > 1.0 {
                self.overruns.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Records an error the device stream reported.
    pub fn stream_error(&self) {
        self.stream_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// The counts now.
    pub fn snapshot(&self) -> StatsSnapshot {
        let stream_errors = self.stream_errors.load(Ordering::Relaxed);
        let overruns = self.overruns.load(Ordering::Relaxed);
        StatsSnapshot {
            callbacks: self.callbacks.load(Ordering::Relaxed),
            frames: self.frames.load(Ordering::Relaxed),
            xruns: stream_errors + overruns,
            stream_errors,
            overruns,
            worst_callback_ratio: f64::from_bits(self.worst.load(Ordering::Relaxed)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CallbackStats;
    use std::time::Duration;

    #[test]
    fn the_worst_ratio_and_overruns() {
        let stats = CallbackStats::default();
        // 100 frames at 1 kHz: a 100 ms period.
        stats.record(100, Duration::from_millis(25), 1_000);
        stats.record(100, Duration::from_millis(50), 1_000);
        stats.record(100, Duration::from_millis(10), 1_000);
        let s = stats.snapshot();
        assert_eq!((s.callbacks, s.frames, s.overruns, s.xruns), (3, 300, 0, 0));
        assert_eq!(s.worst_callback_ratio, 0.5);
        stats.record(10, Duration::from_millis(20), 1_000);
        let s = stats.snapshot();
        assert_eq!((s.overruns, s.xruns), (1, 1));
        assert_eq!(s.worst_callback_ratio, 2.0);
    }

    #[test]
    fn stream_errors_are_xruns() {
        let stats = CallbackStats::default();
        stats.stream_error();
        stats.stream_error();
        let s = stats.snapshot();
        assert_eq!((s.stream_errors, s.overruns, s.xruns), (2, 0, 2));
    }

    #[test]
    fn a_callback_without_frames_has_no_ratio() {
        let stats = CallbackStats::default();
        stats.record(0, Duration::from_millis(5), 48_000);
        let s = stats.snapshot();
        assert_eq!((s.callbacks, s.overruns), (1, 0));
        assert_eq!(s.worst_callback_ratio, 0.0);
    }
}
