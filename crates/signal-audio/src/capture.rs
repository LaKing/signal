//! Capture (ADR 0038 §8, ADR 0052 §8): the plain code on either side of an
//! input device's frames, tested without a device.
//!
//! - [`CaptureFeed`], the input callback's body: gathers each capture
//!   side's channels from the device's interleaved frames and pushes them,
//!   through a ring when the input device is the clock device (one crystal,
//!   no drift), into a clock adapter otherwise.
//! - [`RingSource`] and [`AdaptedSource`], the clock callback's side: the
//!   [`InputSource`]s the run's sources read, exactly the frames each block
//!   needs. A ring short of frames delivers what it has (the rest reads 0,
//!   counted by the run as missing); an adapter always delivers, silence
//!   while it fills.
//!
//! Neither side allocates, frees, locks or blocks: every buffer is made on
//! the control thread when the stream opens (`docs/realtime-rules.md`).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use rtrb::{Consumer, Producer, RingBuffer};
use signal_output::adapter::{AdapterInput, AdapterOutput};
use signal_output::inputs::InputSource;

use crate::secondary::SecondaryStats;

/// Where one capture side's frames go from the input callback.
pub enum SideSink {
    /// A ring read by a [`RingSource`] (the clock device's input).
    Ring(Producer<f32>),
    /// A clock adapter read by an [`AdaptedSource`] (another device's).
    Adapter(AdapterInput),
}

/// One capture side in the input callback: its device channels and where
/// its frames go.
pub struct SideFeed {
    /// Per port, the device's input channel (from 0) it reads; `None`
    /// pushes 0.
    channels: Vec<Option<usize>>,
    sink: SideSink,
    /// One piece of gathered frames.
    gathered: Vec<f32>,
    /// Frames the ring or adapter had no room for, for the status.
    dropped: Arc<AtomicU64>,
}

impl SideFeed {
    /// A side reading device channels `channels` (from 0, per port) into
    /// `sink`, for callbacks of up to `max_frames`. Control thread.
    pub fn new(
        channels: Vec<Option<usize>>,
        sink: SideSink,
        max_frames: usize,
        dropped: Arc<AtomicU64>,
    ) -> SideFeed {
        let width = channels.len().max(1);
        SideFeed {
            channels,
            sink,
            gathered: vec![0.0; max_frames.max(1) * width],
            dropped,
        }
    }
}

/// The input callback's body: every capture side of one input device.
pub struct CaptureFeed {
    sides: Vec<SideFeed>,
    epoch: Instant,
}

impl CaptureFeed {
    /// The feed of `sides`, its times against `epoch` (the driver's, which
    /// the clock callback shares). Control thread.
    pub fn new(sides: Vec<SideFeed>, epoch: Instant) -> CaptureFeed {
        CaptureFeed { sides, epoch }
    }

    /// One input callback: `data` holds frames of `device_channels`,
    /// interleaved. Real-time safe.
    pub fn capture(&mut self, data: &[f32], device_channels: usize) {
        if device_channels == 0 {
            return;
        }
        let time = u64::try_from(self.epoch.elapsed().as_nanos()).ok();
        for side in &mut self.sides {
            let width = side.channels.len();
            if width == 0 {
                continue;
            }
            let piece = (side.gathered.len() / width).max(1);
            for part in data.chunks(piece * device_channels) {
                let frames = part.len() / device_channels;
                let Some(out) = side.gathered.get_mut(..frames * width) else {
                    break;
                };
                for (src, dst) in part
                    .chunks_exact(device_channels)
                    .zip(out.chunks_exact_mut(width))
                {
                    for (d, c) in dst.iter_mut().zip(&side.channels) {
                        *d = c.and_then(|c| src.get(c)).copied().unwrap_or(0.0);
                    }
                }
                match &mut side.sink {
                    SideSink::Ring(ring) => match ring.write_chunk(out.len()) {
                        Ok(mut chunk) => {
                            let (first, second) = chunk.as_mut_slices();
                            let (x, y) = out.split_at(first.len());
                            first.copy_from_slice(x);
                            second.copy_from_slice(y);
                            chunk.commit_all();
                        }
                        Err(_) => {
                            side.dropped.fetch_add(frames as u64, Ordering::Relaxed);
                        }
                    },
                    SideSink::Adapter(input) => {
                        let taken = input.push(out, time);
                        side.dropped
                            .fetch_add((frames - taken) as u64, Ordering::Relaxed);
                    }
                }
            }
        }
    }
}

/// A ring of one capture side from the clock device's input (ADR 0038 §8):
/// made pre-filled with one period of zeros, so the engine reads a fixed,
/// constant-latency stream.
pub struct RingSource {
    ring: Consumer<f32>,
    width: usize,
    /// The prefill, in frames: what the ring holds at the first read, the
    /// rest (frames captured before the output started) discarded, so the
    /// latency is one input period however long the start took.
    prefill: usize,
    started: bool,
}

/// A ring for a side of `width` channels from an input of `period` frames
/// per callback against a clock callback of `clock_period`: room for four
/// of the longer, pre-filled with one input period of zeros. Control
/// thread.
pub fn capture_ring(
    width: usize,
    period: usize,
    clock_period: usize,
) -> (Producer<f32>, RingSource) {
    let width = width.max(1);
    let longer = period.max(clock_period).max(1);
    let (mut tx, rx) = RingBuffer::new(4 * longer * width + period * width);
    for _ in 0..period * width {
        let _ = tx.push(0.0);
    }
    (
        tx,
        RingSource {
            ring: rx,
            width,
            prefill: period,
            started: false,
        },
    )
}

impl InputSource for RingSource {
    fn read(&mut self, out: &mut [f32], _offset: usize) -> usize {
        if !self.started {
            self.started = true;
            let excess = (self.ring.slots() / self.width).saturating_sub(self.prefill);
            if let Ok(chunk) = self.ring.read_chunk(excess * self.width) {
                chunk.commit_all();
            }
        }
        let frames = (out.len() / self.width).min(self.ring.slots() / self.width);
        let n = frames * self.width;
        if let Ok(chunk) = self.ring.read_chunk(n) {
            let (first, second) = chunk.as_slices();
            if let Some(to) = out.get_mut(..first.len()) {
                to.copy_from_slice(first);
            }
            if let Some(to) = out.get_mut(first.len()..n) {
                to.copy_from_slice(second);
            }
            chunk.commit_all();
            frames
        } else {
            0
        }
    }
}

/// A clock adapter's converting end for one capture side from another
/// device (ADR 0052 §8): it converts exactly the frames the block needs,
/// timed against the driver's epoch, and publishes its state.
pub struct AdaptedSource {
    output: AdapterOutput,
    epoch: Instant,
    stats: Arc<SecondaryStats>,
    /// The current device callback's start (the time read at its first
    /// block, offset 0).
    callback: Option<u64>,
}

impl AdaptedSource {
    /// Over `output`, its times against `epoch`, its state in `stats`.
    /// Control thread.
    pub fn new(output: AdapterOutput, epoch: Instant, stats: Arc<SecondaryStats>) -> AdaptedSource {
        AdaptedSource {
            output,
            epoch,
            stats,
            callback: None,
        }
    }
}

impl InputSource for AdaptedSource {
    fn read(&mut self, out: &mut [f32], offset: usize) -> usize {
        let channels = self.output.config().channels.max(1);
        let frames = out.len() / channels;
        // The time of this pull's first frame on the clock device: the
        // callback's start plus its offset in it. A clock callback runs
        // several blocks at once, each pulling: the time read now would put
        // a whole callback's frames at one instant (a period's sawtooth in
        // the fill).
        if offset == 0 || self.callback.is_none() {
            self.callback = u64::try_from(self.epoch.elapsed().as_nanos()).ok();
        }
        let rate = f64::from(self.output.config().dest_rate.max(1));
        let time = self
            .callback
            .map(|t| t.saturating_add((offset as f64 / rate * 1e9) as u64));
        if let Some(whole) = out.get_mut(..frames * channels) {
            self.output.pull(whole, time);
        }
        self.stats.publish(&self.output);
        frames
    }
}
