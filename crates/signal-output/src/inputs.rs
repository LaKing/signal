//! The audio adapter's input side in real time (ADR 0038 §7–9): what the
//! sources of a run read. Input policy, like the output's (ADR 0022): the
//! engine lets a driver write a source's external rows before each block
//! ([`SinkOutput::sources`](signal_engine::SinkOutput::sources)); which
//! frames those are is decided here and by the driver.
//!
//! - A [`RunInput`] serves one capture side (a source by address, its
//!   channels) from an [`InputSource`]: a live device's input ring, a
//!   secondary device's clock adapter, or a recorded input in a replay.
//! - Each block's frames go into the engine as they came (f32 widened to
//!   f64), and, when the run records, the same words into a ring the
//!   control thread drains ([`InputRecord`]): exactly what the engine read,
//!   underrun zeros included (ADR 0038 §9). A full record ring is counted,
//!   never waited for; the record then says it is incomplete.
//! - Frames the source does not deliver read 0 and are counted
//!   (`missing`).
//!
//! Every buffer is made on the control thread when the run is built;
//! [`RunInput::fill`] allocates, frees, locks and blocks nowhere.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};
use signal_core::InstanceAddress;
use signal_engine::{Engine, InstanceId};

/// Where a capture side's frames come from. Real time: [`read`](Self::read)
/// runs on the audio thread and must allocate, lock and block nowhere.
pub trait InputSource: Send {
    /// Fills `out` (interleaved, the side's channels per frame) with the
    /// next frames; returns how many whole frames it delivered (the rest of
    /// `out` is left for the caller to zero). `offset` is where they start
    /// in the device callback that runs them, in frames (0: a new
    /// callback): a source that times its reads (a clock adapter) times
    /// these frames from the callback's start, not from when the block
    /// that needs them happens to run.
    fn read(&mut self, out: &mut [f32], offset: usize) -> usize;
}

/// The counters a [`RunInput`] keeps for the control thread.
#[derive(Debug, Default)]
pub struct InputCounts {
    /// Frames the engine read.
    pub frames: AtomicU64,
    /// Frames the source did not deliver (read as 0): underruns.
    pub missing: AtomicU64,
    /// Samples the record ring had no room for.
    pub record_overflows: AtomicU64,
    /// The largest |sample| read since the start, as f32 bits (a level,
    /// never semantics).
    pub peak: AtomicU32,
}

impl InputCounts {
    /// The largest |sample| read since the start.
    pub fn peak(&self) -> f32 {
        f32::from_bits(self.peak.load(Ordering::Relaxed))
    }
}

/// One capture side of a run: the source instance it writes, the frames it
/// reads them from, and its record.
pub struct RunInput {
    /// The source instance (a capture side's address).
    pub address: InstanceAddress,
    /// Its channels.
    pub width: usize,
    source: Box<dyn InputSource>,
    /// The instance in the running plan, bound at each swap.
    id: Option<InstanceId>,
    /// One block of interleaved frames.
    scratch: Vec<f32>,
    record: Option<Producer<f32>>,
    counts: Arc<InputCounts>,
}

/// The control thread's side of a recorded [`RunInput`]: the words the
/// engine read, in order.
pub struct InputRecord {
    /// The source instance.
    pub address: InstanceAddress,
    /// Its channels.
    pub width: usize,
    words: Consumer<f32>,
    counts: Arc<InputCounts>,
}

impl RunInput {
    /// A capture side at `address` of `width` channels reading from
    /// `source`, for blocks of up to `max_frames`. With `record_frames`, a
    /// record ring of that many frames ([`InputRecord`]). Control thread.
    pub fn new(
        address: InstanceAddress,
        width: usize,
        source: Box<dyn InputSource>,
        max_frames: usize,
        record_frames: Option<usize>,
    ) -> (RunInput, Option<InputRecord>) {
        let counts = Arc::new(InputCounts::default());
        let (record, reader) = match record_frames {
            Some(frames) => {
                let (tx, rx) = RingBuffer::new((frames * width).max(1));
                let reader = InputRecord {
                    address: address.clone(),
                    width,
                    words: rx,
                    counts: Arc::clone(&counts),
                };
                (Some(tx), Some(reader))
            }
            None => (None, None),
        };
        let input = RunInput {
            address,
            width,
            source,
            id: None,
            scratch: vec![0.0; max_frames.max(1) * width.max(1)],
            record,
            counts,
        };
        (input, reader)
    }

    /// Its counters.
    pub fn counts(&self) -> Arc<InputCounts> {
        Arc::clone(&self.counts)
    }

    /// Finds its source in the running plan by address (before the first
    /// block and after every swap). A plan without it reads nothing.
    pub fn bind(&mut self, engine: &Engine) {
        self.id = engine
            .sources()
            .find(|s| *s.address == self.address && s.rows == self.width)
            .map(|s| s.id);
    }

    /// Before a block of `frames` frames, `offset` frames into the device
    /// callback: reads them from the source and writes them into the
    /// engine's external rows, and into the record. Real-time safe.
    pub fn fill(&mut self, engine: &mut Engine, frames: usize, offset: usize) {
        let width = self.width;
        if width == 0 {
            return;
        }
        let piece = (self.scratch.len() / width).max(1);
        let k = engine.capacity();
        let mut at = 0;
        while at < frames {
            let n = (frames - at).min(piece);
            let Some(buffer) = self.scratch.get_mut(..n * width) else {
                return;
            };
            let got = self.source.read(buffer, offset + at).min(n);
            if let Some(rest) = buffer.get_mut(got * width..) {
                rest.fill(0.0);
            }
            self.counts
                .missing
                .fetch_add((n - got) as u64, Ordering::Relaxed);
            let peak = buffer
                .iter()
                .fold(0.0f32, |p, x| if x.abs() > p { x.abs() } else { p });
            if peak > self.counts.peak() {
                self.counts.peak.store(peak.to_bits(), Ordering::Relaxed);
            }
            if let Some(rows) = self.id.and_then(|id| engine.source_rows_mut(id)) {
                for (f, frame) in buffer.chunks_exact(width).enumerate() {
                    for (r, &x) in frame.iter().enumerate() {
                        if let Some(y) = rows.get_mut(r * k + at + f) {
                            *y = f64::from(x);
                        }
                    }
                }
            }
            if let Some(record) = self.record.as_mut() {
                if let Ok(mut chunk) = record.write_chunk(buffer.len()) {
                    let (first, second) = chunk.as_mut_slices();
                    let (x, y) = buffer.split_at(first.len());
                    first.copy_from_slice(x);
                    second.copy_from_slice(y);
                    chunk.commit_all();
                } else {
                    self.counts
                        .record_overflows
                        .fetch_add(buffer.len() as u64, Ordering::Relaxed);
                }
            }
            at += n;
        }
        self.counts
            .frames
            .fetch_add(frames as u64, Ordering::Relaxed);
    }
}

impl InputRecord {
    /// Takes the words recorded since the last call into `into` (control
    /// thread).
    pub fn drain(&mut self, into: &mut Vec<f32>) {
        let n = self.words.slots();
        if let Ok(chunk) = self.words.read_chunk(n) {
            let (first, second) = chunk.as_slices();
            into.extend_from_slice(first);
            into.extend_from_slice(second);
            chunk.commit_all();
        }
    }

    /// Whether every word the engine read reached the record so far.
    pub fn complete(&self) -> bool {
        self.counts.record_overflows.load(Ordering::Relaxed) == 0
    }

    /// The counters.
    pub fn counts(&self) -> &InputCounts {
        &self.counts
    }
}

/// An [`InputSource`] over frames held in memory: a recorded input in a
/// replay, a render fixture, a test. Past its end it delivers nothing.
pub struct RecordedInput {
    words: Vec<f32>,
    width: usize,
    at: usize,
}

impl RecordedInput {
    /// Over `words`, interleaved frames of `width` channels.
    pub fn new(words: Vec<f32>, width: usize) -> RecordedInput {
        RecordedInput {
            words,
            width: width.max(1),
            at: 0,
        }
    }
}

impl InputSource for RecordedInput {
    fn read(&mut self, out: &mut [f32], _offset: usize) -> usize {
        let rest = self.words.get(self.at..).unwrap_or(&[]);
        let frames = (rest.len().min(out.len())) / self.width;
        let n = frames * self.width;
        if let (Some(to), Some(from)) = (out.get_mut(..n), rest.get(..n)) {
            to.copy_from_slice(from);
        }
        self.at += n;
        frames
    }
}
