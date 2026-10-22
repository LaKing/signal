//! Callback statistics (statistics only, never semantics): how long each
//! callback took against its period, on average and at worst, and the xruns.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

use serde::Serialize;

/// Buckets of the callback-duration histogram: 0.01 of the period each, up
/// to 4 periods, and one for longer callbacks.
const RATIO_BUCKETS: usize = 401;

/// Why the audio system stopped a playing stream: it calls no more back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamFault {
    /// The device's sample rate changed (Audio MIDI Setup, another
    /// application): CoreAudio invalidated the stream.
    RateChanged,
    /// The device went away (unplugged, or no default output any more).
    Gone,
}

impl StreamFault {
    fn code(self) -> u8 {
        match self {
            StreamFault::RateChanged => 1,
            StreamFault::Gone => 2,
        }
    }

    fn from_code(code: u8) -> Option<StreamFault> {
        match code {
            1 => Some(StreamFault::RateChanged),
            2 => Some(StreamFault::Gone),
            _ => None,
        }
    }
}

/// Counters shared between the audio callback, which records, and the control
/// thread, which reads. Atomics only: recording allocates, locks and blocks
/// nowhere.
#[derive(Debug)]
pub struct CallbackStats {
    /// Callbacks by duration / period, in steps of 0.01 (M2.7-1: the median
    /// and p99).
    ratios: [AtomicU64; RATIO_BUCKETS],
    callbacks: AtomicU64,
    frames: AtomicU64,
    overruns: AtomicU64,
    stream_errors: AtomicU64,
    /// The time spent in callbacks with a period, in nanoseconds.
    busy_nanos: AtomicU64,
    /// Their periods, in nanoseconds.
    period_nanos: AtomicU64,
    /// The worst duration / period, as f64 bits (finite and ≥ 0, so the bits
    /// order like the values).
    worst: AtomicU64,
    /// The first [`StreamFault`]'s code, 0 before one.
    fault: AtomicU8,
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
    /// The time spent in callbacks as a fraction of their periods, over all
    /// callbacks with a period (0 before any).
    pub average_callback_ratio: f64,
    /// The median callback duration as a fraction of its period: the upper
    /// edge of its 0.01 step (at most the worst).
    pub median_callback_ratio: f64,
    /// The 99th percentile (nearest rank), likewise.
    pub p99_callback_ratio: f64,
}

impl Default for CallbackStats {
    fn default() -> CallbackStats {
        CallbackStats {
            ratios: [const { AtomicU64::new(0) }; RATIO_BUCKETS],
            callbacks: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
            stream_errors: AtomicU64::new(0),
            busy_nanos: AtomicU64::new(0),
            period_nanos: AtomicU64::new(0),
            worst: AtomicU64::new(0),
            fault: AtomicU8::new(0),
        }
    }
}

impl CallbackStats {
    /// The ratio below which a fraction `p` of the recorded callbacks fall
    /// (nearest rank), as the upper edge of its step, at most `worst`; in
    /// the last bucket (beyond 4 periods), `worst`.
    fn rank(&self, p: f64, worst: f64) -> f64 {
        let counts: Vec<u64> = self
            .ratios
            .iter()
            .map(|b| b.load(Ordering::Relaxed))
            .collect();
        let total: u64 = counts.iter().sum();
        if total == 0 {
            return 0.0;
        }
        let k = ((p * total as f64).ceil() as u64).max(1);
        let mut seen = 0;
        for (b, &n) in counts.iter().enumerate() {
            seen += n;
            if seen >= k && b + 1 < RATIO_BUCKETS {
                return ((b + 1) as f64 / 100.0).min(worst);
            }
            if seen >= k {
                return worst;
            }
        }
        worst
    }

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
            let bucket = ((ratio * 100.0) as usize).min(RATIO_BUCKETS - 1);
            if let Some(bucket) = self.ratios.get(bucket) {
                bucket.fetch_add(1, Ordering::Relaxed);
            }
            let busy = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
            self.busy_nanos.fetch_add(busy, Ordering::Relaxed);
            self.period_nanos
                .fetch_add((period * 1e9).round() as u64, Ordering::Relaxed);
            if ratio > 1.0 {
                self.overruns.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Records an error the device stream reported.
    pub fn stream_error(&self) {
        self.stream_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Records why the audio system stopped the stream; the first fault
    /// stays. Atomics only, as every record.
    pub fn stream_fault(&self, fault: StreamFault) {
        let _ = self
            .fault
            .compare_exchange(0, fault.code(), Ordering::Relaxed, Ordering::Relaxed);
    }

    /// Forgets the fault: a new stream starts.
    pub fn clear_fault(&self) {
        self.fault.store(0, Ordering::Relaxed);
    }

    /// The fault that stopped the stream, if one did.
    pub fn fault(&self) -> Option<StreamFault> {
        StreamFault::from_code(self.fault.load(Ordering::Relaxed))
    }

    /// The counts now.
    pub fn snapshot(&self) -> StatsSnapshot {
        let stream_errors = self.stream_errors.load(Ordering::Relaxed);
        let overruns = self.overruns.load(Ordering::Relaxed);
        let busy = self.busy_nanos.load(Ordering::Relaxed);
        let period = self.period_nanos.load(Ordering::Relaxed);
        let worst = f64::from_bits(self.worst.load(Ordering::Relaxed));
        StatsSnapshot {
            callbacks: self.callbacks.load(Ordering::Relaxed),
            frames: self.frames.load(Ordering::Relaxed),
            xruns: stream_errors + overruns,
            stream_errors,
            overruns,
            worst_callback_ratio: worst,
            average_callback_ratio: if period > 0 {
                busy as f64 / period as f64
            } else {
                0.0
            },
            median_callback_ratio: self.rank(0.5, worst),
            p99_callback_ratio: self.rank(0.99, worst),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CallbackStats, StreamFault};
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
        // 85 ms in 300 ms of periods.
        assert!((s.average_callback_ratio - 85.0 / 300.0).abs() < 1e-12);
        stats.record(10, Duration::from_millis(20), 1_000);
        let s = stats.snapshot();
        assert_eq!((s.overruns, s.xruns), (1, 1));
        assert_eq!(s.worst_callback_ratio, 2.0);
    }

    /// M2.7-1: the median and p99 of the callback durations, to 0.01 of the
    /// period, never above the worst.
    #[test]
    fn the_median_and_p99_ratios() {
        let stats = CallbackStats::default();
        assert_eq!(stats.snapshot().p99_callback_ratio, 0.0);
        // 100 frames at 1 kHz: a 100 ms period. 98 callbacks of 20.5 ms, one
        // of 50.5 ms, one of 300 ms.
        for _ in 0..98 {
            stats.record(100, Duration::from_micros(20_500), 1_000);
        }
        stats.record(100, Duration::from_micros(50_500), 1_000);
        stats.record(100, Duration::from_millis(300), 1_000);
        let s = stats.snapshot();
        assert!((s.median_callback_ratio - 0.21).abs() < 1e-9, "{s:?}");
        assert!((s.p99_callback_ratio - 0.51).abs() < 1e-9, "{s:?}");
        assert!((s.worst_callback_ratio - 3.0).abs() < 1e-9, "{s:?}");
        // Beyond 4 periods: the last bucket, read as the worst.
        let long = CallbackStats::default();
        long.record(10, Duration::from_millis(100), 1_000);
        let s = long.snapshot();
        assert_eq!(
            (s.p99_callback_ratio, s.median_callback_ratio),
            (10.0, 10.0)
        );
    }

    #[test]
    fn the_first_fault_stays() {
        let stats = CallbackStats::default();
        assert_eq!(stats.fault(), None);
        stats.stream_fault(StreamFault::RateChanged);
        stats.stream_fault(StreamFault::Gone);
        assert_eq!(stats.fault(), Some(StreamFault::RateChanged));
        stats.clear_fault();
        assert_eq!(stats.fault(), None);
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
        assert_eq!(s.average_callback_ratio, 0.0);
    }
}
