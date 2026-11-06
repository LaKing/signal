//! Secondary output devices (ADR 0052 §1–4, phase 2): the plain code on
//! either side of a clock adapter, tested without a device.
//!
//! - [`SecondaryFeed`], the engine side: the clock device's callback hands
//!   it each piece of the run's audio frame it made; it gathers the
//!   channels its device plays (after the layout, `gain`, clamp and f32
//!   conversion the Runner already did) and pushes them into the adapter's
//!   ring, stamped with the piece's host time. The engine's output is not
//!   touched: the feed only reads it. It also hands on the run's output
//!   ceiling, which the device side enforces.
//! - [`SecondaryCallback`], the device side: the secondary device's own
//!   callback pulls exactly the frames it must deliver from the adapter's
//!   converter, holds them within the device's range and the ceiling (the
//!   converter's interpolation and a slip's crossfade can overshoot what
//!   the Runner held), counting each it held, and scatters them onto its
//!   device channels.
//! - [`SecondaryStats`]: what the device side publishes for the control
//!   thread (state, correction, slips, latency) and the stream's fault.
//!   Atomics only.
//!
//! Neither side allocates, frees, locks or blocks: every buffer is made on
//! the control thread when the route is made (`docs/realtime-rules.md`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

use serde::Serialize;
use signal_output::adapter::{AdapterInput, AdapterOutput, AdapterState};
use signal_output::hold;

/// The engine side of one secondary device: gathers its channels of the
/// run's audio frame and pushes them into its adapter.
#[derive(Debug)]
pub struct SecondaryFeed {
    input: AdapterInput,
    /// The run frame's channels it carries, in the adapter's order.
    sources: Vec<usize>,
    /// One piece of gathered frames.
    gathered: Vec<f32>,
    /// Where it hands on the run's ceiling: the device side's stats.
    stats: Arc<SecondaryStats>,
}

impl SecondaryFeed {
    /// A feed of the run frame's channels `sources` into `input` (whose
    /// adapter carries `sources.len()` channels), for pieces of up to
    /// `max_frames` frames, handing the run's ceiling on to the device
    /// side whose state is `stats`. Control thread.
    pub fn new(
        input: AdapterInput,
        sources: Vec<usize>,
        max_frames: usize,
        stats: Arc<SecondaryStats>,
    ) -> SecondaryFeed {
        let gathered = vec![0.0; max_frames.max(1) * sources.len().max(1)];
        SecondaryFeed {
            input,
            sources,
            gathered,
            stats,
        }
    }

    /// Pushes the feed's channels of `frame` (interleaved, `width` channels
    /// per frame), made at host time `time_ns` if known, under the run's
    /// output ceiling `ceiling` ([`Runner::ceiling`](signal_output::Runner::ceiling);
    /// `None`: the device's range alone). A piece longer than the feed was
    /// made for is pushed in parts. Real-time safe.
    pub fn push(
        &mut self,
        frame: &[f32],
        width: usize,
        time_ns: Option<u64>,
        ceiling: Option<f32>,
    ) {
        self.stats.set_limit(ceiling);
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
    /// Its clock is beyond the loop's reach: faded out until it locks
    /// again (ADR 0052 §6).
    Suspect,
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
            SecondaryState::Suspect => 4,
        }
    }

    fn from_code(code: u8) -> SecondaryState {
        match code {
            1 => SecondaryState::Locking,
            2 => SecondaryState::Locked,
            3 => SecondaryState::Absent,
            4 => SecondaryState::Suspect,
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
            SecondaryState::Suspect => "suspect",
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
    resyncs: AtomicU64,
    starts: AtomicU64,
    stops: AtomicU64,
    callbacks: AtomicU64,
    /// The stream reported the device gone or failed.
    gone: AtomicBool,
    stream_errors: AtomicU64,
    /// The level the device side holds its samples within, as f32 bits:
    /// the run's ceiling, at most 1 (the device's range).
    limit: Limit,
    /// Device samples held within it, since the stream started.
    held: AtomicU64,
}

/// [`SecondaryStats::limit`]: 1 (the device's range) until a feed hands on
/// a ceiling.
#[derive(Debug)]
struct Limit(AtomicU32);

impl Default for Limit {
    fn default() -> Limit {
        Limit(AtomicU32::new(1.0f32.to_bits()))
    }
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
    /// Resyncs (ADR 0052 §6).
    pub resyncs: u64,
    /// Starts after a fill (the first included).
    pub starts: u64,
    /// Fades to silence after the engine side stopped.
    pub stops: u64,
    /// Device callbacks.
    pub callbacks: u64,
    /// Errors the stream reported.
    pub stream_errors: u64,
    /// Device samples held within the device's range or the run's ceiling
    /// after the conversion.
    pub held: u64,
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

    /// Hands on the run's output ceiling (`None`: the device's range).
    fn set_limit(&self, ceiling: Option<f32>) {
        let level = ceiling.map_or(1.0, |c| {
            if c.is_finite() {
                c.clamp(0.0, 1.0)
            } else {
                0.0
            }
        });
        self.limit.0.store(level.to_bits(), Ordering::Relaxed);
    }

    fn limit(&self) -> f32 {
        f32::from_bits(self.limit.0.load(Ordering::Relaxed))
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
            resyncs: self.resyncs.load(Ordering::Relaxed),
            starts: self.starts.load(Ordering::Relaxed),
            stops: self.stops.load(Ordering::Relaxed),
            callbacks: self.callbacks.load(Ordering::Relaxed),
            stream_errors: self.stream_errors.load(Ordering::Relaxed),
            held: self.held.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn publish(&self, output: &AdapterOutput) {
        let status = output.status();
        if !self.is_gone() {
            let state = match status.state {
                AdapterState::Locked => SecondaryState::Locked,
                AdapterState::Suspect => SecondaryState::Suspect,
                AdapterState::Filling | AdapterState::Locking => SecondaryState::Locking,
            };
            self.state.store(state.code(), Ordering::Relaxed);
        }
        self.correction_ppm
            .store(status.correction_ppm.to_bits(), Ordering::Relaxed);
        self.latency_frames
            .store(output.latency_frames().to_bits(), Ordering::Relaxed);
        self.slips.store(status.slips, Ordering::Relaxed);
        self.resyncs.store(status.resyncs, Ordering::Relaxed);
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
        let limit = Some(self.stats.limit());
        let mut held = 0u64;
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
                        *y = if x.is_finite() {
                            hold(x, limit, &mut held)
                        } else {
                            held += 1;
                            0.0
                        };
                    }
                }
            }
            offset += frames as u64;
        }
        if held > 0 {
            self.stats.held.fetch_add(held, Ordering::Relaxed);
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
