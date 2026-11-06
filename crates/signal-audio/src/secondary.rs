//! Secondary output devices (ADR 0052 §1–4, phase 2): the plain code on
//! either side of a clock adapter, tested without a device.
//!
//! - [`SecondaryFeed`], the engine side: the clock device's callback hands
//!   it each piece of the run's audio frame it made; it gathers the
//!   channels its device plays (after the layout, `gain`, clamp and f32
//!   conversion the Runner already did) and pushes them into the adapter's
//!   ring, stamped with the piece's host time. The engine's output is not
//!   touched: the feed only reads it.
//! - [`SecondaryCallback`], the device side: the secondary device's own
//!   callback pulls exactly the frames it must deliver from the adapter's
//!   converter and scatters them onto its device channels.
//! - [`SecondaryStats`]: what the device side publishes for the control
//!   thread (state, correction, slips, latency) and the stream's fault.
//!   Atomics only.
//!
//! Neither side allocates, frees, locks or blocks: every buffer is made on
//! the control thread when the route is made (`docs/realtime-rules.md`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use serde::Serialize;
use signal_output::adapter::{AdapterInput, AdapterOutput, AdapterState};

/// The engine side of one secondary device: gathers its channels of the
/// run's audio frame and pushes them into its adapter.
#[derive(Debug)]
pub struct SecondaryFeed {
    input: AdapterInput,
    /// The run frame's channels it carries, in the adapter's order.
    sources: Vec<usize>,
    /// One piece of gathered frames.
    gathered: Vec<f32>,
}

impl SecondaryFeed {
    /// A feed of the run frame's channels `sources` into `input` (whose
    /// adapter carries `sources.len()` channels), for pieces of up to
    /// `max_frames` frames. Control thread.
    pub fn new(input: AdapterInput, sources: Vec<usize>, max_frames: usize) -> SecondaryFeed {
        let gathered = vec![0.0; max_frames.max(1) * sources.len().max(1)];
        SecondaryFeed {
            input,
            sources,
            gathered,
        }
    }

    /// Pushes the feed's channels of `frame` (interleaved, `width` channels
    /// per frame), made at host time `time_ns` if known. A piece longer
    /// than the feed was made for is pushed in parts. Real-time safe.
    pub fn push(&mut self, frame: &[f32], width: usize, time_ns: Option<u64>) {
        let channels = self.sources.len();
        if channels == 0 || width == 0 {
            return;
        }
        let piece = (self.gathered.len() / channels).max(1);
        for part in frame.chunks(piece * width) {
            let frames = part.len() / width;
            let Some(out) = self.gathered.get_mut(..frames * channels) else {
                return;
            };
            for (src, dst) in part.chunks_exact(width).zip(out.chunks_exact_mut(channels)) {
                for (d, &s) in dst.iter_mut().zip(&self.sources) {
                    *d = src.get(s).copied().unwrap_or(0.0);
                }
            }
            self.input.push(out, time_ns);
        }
    }

    /// Frames the ring had no room for (the device stopped consuming).
    pub fn dropped_frames(&self) -> u64 {
        self.input.dropped_frames()
    }
}

/// A secondary device's state as its status shows it (ADR 0052 §6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecondaryState {
    /// The stream is not running yet.
    Starting,
    /// Silence until the ring holds its target, or the loop in acquire.
    Locking,
    /// The loop in track.
    Locked,
    /// The device went away or its stream failed.
    Absent,
}

impl SecondaryState {
    fn code(self) -> u8 {
        match self {
            SecondaryState::Starting => 0,
            SecondaryState::Locking => 1,
            SecondaryState::Locked => 2,
            SecondaryState::Absent => 3,
        }
    }

    fn from_code(code: u8) -> SecondaryState {
        match code {
            1 => SecondaryState::Locking,
            2 => SecondaryState::Locked,
            3 => SecondaryState::Absent,
            _ => SecondaryState::Starting,
        }
    }

    /// The word the status shows.
    pub fn name(self) -> &'static str {
        match self {
            SecondaryState::Starting => "starting",
            SecondaryState::Locking => "locking",
            SecondaryState::Locked => "locked",
            SecondaryState::Absent => "absent",
        }
    }
}

/// What a secondary device's callback publishes and the control thread
/// reads. Atomics only.
#[derive(Debug, Default)]
pub struct SecondaryStats {
    state: AtomicU8,
    /// The correction in use, ppm, as f64 bits.
    correction_ppm: AtomicU64,
    /// The adapter's latency, source frames, as f64 bits.
    latency_frames: AtomicU64,
    slips: AtomicU64,
    starts: AtomicU64,
    stops: AtomicU64,
    callbacks: AtomicU64,
    /// The stream reported the device gone or failed.
    gone: AtomicBool,
    stream_errors: AtomicU64,
}

/// A copy of [`SecondaryStats`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct SecondarySnapshot {
    /// Starting, locking, locked or absent.
    pub state: SecondaryState,
    /// The drift correction in use: how far the device's clock runs from
    /// the clock device's, ppm.
    pub drift_ppm: f64,
    /// The adapter's latency, source frames (the device's own comes on top).
    pub latency_frames: f64,
    /// Repeats and skips (ADR 0052 §3).
    pub slips: u64,
    /// Starts after a fill (the first included).
    pub starts: u64,
    /// Fades to silence after the engine side stopped.
    pub stops: u64,
    /// Device callbacks.
    pub callbacks: u64,
    /// Errors the stream reported.
    pub stream_errors: u64,
}

impl SecondaryStats {
    /// Records the stream's report that the device is gone (error callback:
    /// atomics only).
    pub fn device_gone(&self) {
        self.gone.store(true, Ordering::Relaxed);
        self.state
            .store(SecondaryState::Absent.code(), Ordering::Relaxed);
    }

    /// Counts an error the stream reported.
    pub fn stream_error(&self) {
        self.stream_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Whether the stream reported the device gone.
    pub fn is_gone(&self) -> bool {
        self.gone.load(Ordering::Relaxed)
    }

    /// The counters now.
    pub fn snapshot(&self) -> SecondarySnapshot {
        SecondarySnapshot {
            state: SecondaryState::from_code(self.state.load(Ordering::Relaxed)),
            drift_ppm: f64::from_bits(self.correction_ppm.load(Ordering::Relaxed)),
            latency_frames: f64::from_bits(self.latency_frames.load(Ordering::Relaxed)),
            slips: self.slips.load(Ordering::Relaxed),
            starts: self.starts.load(Ordering::Relaxed),
            stops: self.stops.load(Ordering::Relaxed),
            callbacks: self.callbacks.load(Ordering::Relaxed),
            stream_errors: self.stream_errors.load(Ordering::Relaxed),
        }
    }

    fn publish(&self, output: &AdapterOutput) {
        let status = output.status();
        if !self.is_gone() {
            let state = match status.state {
                AdapterState::Locked => SecondaryState::Locked,
                AdapterState::Filling | AdapterState::Locking => SecondaryState::Locking,
            };
            self.state.store(state.code(), Ordering::Relaxed);
        }
        self.correction_ppm
            .store(status.correction_ppm.to_bits(), Ordering::Relaxed);
        self.latency_frames
            .store(output.latency_frames().to_bits(), Ordering::Relaxed);
        self.slips.store(status.slips, Ordering::Relaxed);
        self.starts.store(status.starts, Ordering::Relaxed);
        self.stops.store(status.stops, Ordering::Relaxed);
        self.callbacks.fetch_add(1, Ordering::Relaxed);
    }
}

/// The device side of one secondary device: pulls from the adapter and
/// scatters onto the device's channels.
#[derive(Debug)]
pub struct SecondaryCallback {
    output: AdapterOutput,
    /// Per adapter channel, the device channel (from 0) it plays on.
    targets: Vec<usize>,
    /// One piece of pulled frames.
    scratch: Vec<f32>,
    stats: Arc<SecondaryStats>,
}

impl SecondaryCallback {
    /// A callback pulling from `output` (whose adapter carries
    /// `targets.len()` channels) onto device channels `targets` (from 0),
    /// for pieces of up to `max_frames`. Control thread.
    pub fn new(
        output: AdapterOutput,
        targets: Vec<usize>,
        max_frames: usize,
        stats: Arc<SecondaryStats>,
    ) -> SecondaryCallback {
        let scratch = vec![0.0; max_frames.max(1) * targets.len().max(1)];
        SecondaryCallback {
            output,
            targets,
            scratch,
            stats,
        }
    }

    /// One device callback: `data` holds frames of `device_channels`,
    /// interleaved; `time_ns` is the callback's host time, if known.
    /// Channels no route plays are silent. Real-time safe.
    pub fn fill(&mut self, data: &mut [f32], device_channels: usize, time_ns: Option<u64>) {
        data.fill(0.0);
        let channels = self.targets.len();
        if channels == 0 || device_channels == 0 {
            return;
        }
        let piece = (self.scratch.len() / channels).max(1);
        let frame_ns = self.frame_ns();
        let mut offset = 0u64;
        for out in data.chunks_mut(piece * device_channels) {
            let frames = out.len() / device_channels;
            let Some(src) = self.scratch.get_mut(..frames * channels) else {
                break;
            };
            let time = time_ns.map(|t| t.saturating_add((offset as f64 * frame_ns) as u64));
            self.output.pull(src, time);
            for (s, d) in src
                .chunks_exact(channels)
                .zip(out.chunks_exact_mut(device_channels))
            {
                for (&x, &t) in s.iter().zip(&self.targets) {
                    if let Some(y) = d.get_mut(t) {
                        *y = x;
                    }
                }
            }
            offset += frames as u64;
        }
        self.stats.publish(&self.output);
    }

    /// The adapter.
    pub fn output(&self) -> &AdapterOutput {
        &self.output
    }

    fn frame_ns(&self) -> f64 {
        1e9 / f64::from(self.output.config().dest_rate.max(1))
    }
}
