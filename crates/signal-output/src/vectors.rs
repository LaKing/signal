//! The vector adapter (M2.8-7, beside the scope adapter): snapshots of a
//! vector output of the running plan (a visual-rate model's state) for
//! `<signal-canvas>`. The control thread starts a slot on a port with its
//! rows and a decimation `d`; the audio thread then keeps, at every absolute
//! frame that is a multiple of `d`, the port's rows as its readers saw them,
//! one [`VectorSnapshot`] per frame, pushed to a preallocated queue (a full
//! queue drops the snapshot and counts it). The decimation is a multiple of
//! the port's rate divisor, so snapshots fall on its ticks and do not
//! depend on how frames were split into callbacks.
//!
//! As for scopes, nothing is allocated or freed on the audio thread: slots,
//! snapshots and queues are made with the run, and a port replaced by a
//! command goes back to the control thread to be dropped there.
//!
//! [`VectorTaps`] takes the same snapshots offline (a replay, `crate::render`),
//! so a recorded session's visual streams can be compared bit for bit.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};
use signal_core::{Frame, PortAddress};
use signal_engine::Engine;

/// Vector slots per run.
pub const VECTOR_SLOTS: usize = 8;
/// The most rows a vector stream carries.
pub const VECTOR_MAX_ROWS: usize = 512;
/// Snapshots the queue to the control thread holds.
const VECTOR_QUEUE: usize = 64;
/// Commands outstanding (sent, their replaced port not yet returned).
const VECTOR_COMMANDS: usize = 32;

/// The rows of a vector output at one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VectorSnapshot {
    /// The slot.
    pub slot: usize,
    /// The slot's start it belongs to ([`Controller::vector_start`](crate::Controller::vector_start)).
    pub generation: u32,
    /// The frame.
    pub frame: Frame,
    /// Frames between snapshots.
    pub decimation: u64,
    /// Rows used.
    pub len: usize,
    /// The rows, as `f32`.
    pub values: [f32; VECTOR_MAX_ROWS],
}

impl VectorSnapshot {
    const EMPTY: VectorSnapshot = VectorSnapshot {
        slot: 0,
        generation: 0,
        frame: Frame(0),
        decimation: 1,
        len: 0,
        values: [0.0; VECTOR_MAX_ROWS],
    };

    /// The rows used.
    pub fn values(&self) -> &[f32] {
        self.values.get(..self.len).unwrap_or(&[])
    }
}

/// Why a vector command was not sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VectorError {
    /// No such slot.
    Slot,
    /// The decimation must be at least 1.
    Decimation,
    /// Rows must be 1 to [`VECTOR_MAX_ROWS`].
    Rows,
    /// Too many commands outstanding; poll and try again.
    Full,
}

impl std::fmt::Display for VectorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            VectorError::Slot => "no such vector slot",
            VectorError::Decimation => "the decimation must be at least 1",
            VectorError::Rows => "a vector stream has 1 to 512 rows",
            VectorError::Full => "too many vector commands outstanding",
        })
    }
}

impl std::error::Error for VectorError {}

/// Writes the `out.len()` rows of the output whose first row is `first` at
/// block frame `f`, as `f32`. Allocates nothing.
fn capture(engine: &Engine, first: usize, f: usize, out: &mut [f32]) {
    for (r, value) in out.iter_mut().enumerate() {
        *value = engine.signal_output_at(first + r, f).unwrap_or(0.0) as f32;
    }
}

/// The first row of vector output `port` in the running plan, if it has
/// exactly `rows` rows: a slot never reads past its port into another's, so
/// a swap that changes the port's width leaves it unbound (M2.8-7, finding
/// auditor-1-007) until the control side starts it again with the new
/// width. Allocates nothing.
fn tap(engine: &Engine, port: &PortAddress, rows: usize) -> Option<usize> {
    engine
        .signal_output_rows(port)
        .filter(|&(_, n)| n == rows)
        .map(|(first, _)| first)
}

/// The block frames of `0..frames`, the first at absolute frame `first`,
/// that are multiples of `d`.
fn multiples(first: u64, frames: usize, d: u64) -> impl Iterator<Item = usize> {
    let start = ((d - first % d) % d) as usize;
    (start..frames).step_by(d.max(1) as usize)
}

enum Command {
    Start {
        slot: usize,
        generation: u32,
        port: PortAddress,
        rows: usize,
        decimation: u64,
    },
    Stop {
        slot: usize,
    },
}

/// What the audio thread publishes for the control thread.
#[derive(Debug, Default)]
pub(crate) struct VectorCounts {
    overflows: AtomicU64,
    bound: [AtomicBool; VECTOR_SLOTS],
}

struct Slot {
    port: Option<PortAddress>,
    generation: u32,
    rows: usize,
    decimation: u64,
    /// The port's first row in the running plan, while it has it.
    tap: Option<usize>,
    snapshot: VectorSnapshot,
}

/// The audio-thread half, inside the [`Runner`](crate::Runner).
pub(crate) struct Vectors {
    slots: Vec<Slot>,
    commands: Consumer<Command>,
    returns: Producer<Option<PortAddress>>,
    snapshots: Producer<VectorSnapshot>,
    overflows: u64,
}

/// The control-thread half, inside the [`Controller`](crate::Controller).
pub(crate) struct VectorControl {
    commands: Producer<Command>,
    returns: Consumer<Option<PortAddress>>,
    snapshots: Consumer<VectorSnapshot>,
    generations: [u32; VECTOR_SLOTS],
    next_generation: u32,
    outstanding: usize,
}

/// Both halves, made with the run.
pub(crate) fn vectors() -> (Vectors, VectorControl) {
    let (commands_tx, commands_rx) = RingBuffer::new(VECTOR_COMMANDS);
    let (returns_tx, returns_rx) = RingBuffer::new(VECTOR_COMMANDS);
    let (snapshots_tx, snapshots_rx) = RingBuffer::new(VECTOR_QUEUE);
    let slots = (0..VECTOR_SLOTS)
        .map(|slot| Slot {
            port: None,
            generation: 0,
            rows: 0,
            decimation: 1,
            tap: None,
            snapshot: VectorSnapshot {
                slot,
                ..VectorSnapshot::EMPTY
            },
        })
        .collect();
    (
        Vectors {
            slots,
            commands: commands_rx,
            returns: returns_tx,
            snapshots: snapshots_tx,
            overflows: 0,
        },
        VectorControl {
            commands: commands_tx,
            returns: returns_rx,
            snapshots: snapshots_rx,
            generations: [0; VECTOR_SLOTS],
            next_generation: 0,
            outstanding: 0,
        },
    )
}

impl Vectors {
    /// Takes the commands sent since the last callback. Allocates nothing.
    pub(crate) fn commands(&mut self, engine: &Engine) {
        while let Ok(command) = self.commands.pop() {
            let (slot, port, generation, rows, decimation) = match command {
                Command::Start {
                    slot,
                    generation,
                    port,
                    rows,
                    decimation,
                } => (slot, Some(port), generation, rows, decimation),
                Command::Stop { slot } => (slot, None, 0, 0, 1),
            };
            let Some(s) = self.slots.get_mut(slot) else {
                let _ = self.returns.push(port);
                continue;
            };
            s.rows = rows.min(VECTOR_MAX_ROWS);
            s.tap = port.as_ref().and_then(|p| tap(engine, p, s.rows));
            let old = std::mem::replace(&mut s.port, port);
            s.generation = generation;
            s.decimation = decimation.max(1);
            let _ = self.returns.push(old);
        }
    }

    /// After a swap (and before the first block): each slot finds its port
    /// in the running plan, or waits unbound while the plan lacks it.
    pub(crate) fn bind(&mut self, engine: &Engine) {
        for s in &mut self.slots {
            s.tap = s.port.as_ref().and_then(|p| tap(engine, p, s.rows));
        }
    }

    /// Captures block frames `0..frames`, the first at absolute frame `first`.
    pub(crate) fn block(&mut self, engine: &Engine, first: u64, frames: usize) {
        for s in &mut self.slots {
            let Some(tap) = s.tap else {
                continue;
            };
            for f in multiples(first, frames, s.decimation) {
                s.snapshot.frame = Frame(first + f as u64);
                s.snapshot.generation = s.generation;
                s.snapshot.decimation = s.decimation;
                s.snapshot.len = s.rows;
                if let Some(out) = s.snapshot.values.get_mut(..s.rows) {
                    capture(engine, tap, f, out);
                }
                if self.snapshots.push(s.snapshot).is_err() {
                    self.overflows = self.overflows.saturating_add(1);
                }
            }
        }
    }

    /// Publishes the counts for the control thread.
    pub(crate) fn publish(&self, counts: &VectorCounts) {
        counts.overflows.store(self.overflows, Ordering::Relaxed);
        for (s, bound) in self.slots.iter().zip(&counts.bound) {
            bound.store(s.tap.is_some(), Ordering::Relaxed);
        }
    }
}

impl VectorControl {
    fn send(&mut self, command: Command) -> Result<(), VectorError> {
        if self.outstanding >= VECTOR_COMMANDS {
            self.drain_returns();
        }
        if self.outstanding >= VECTOR_COMMANDS {
            return Err(VectorError::Full);
        }
        self.commands.push(command).map_err(|_| VectorError::Full)?;
        self.outstanding += 1;
        Ok(())
    }

    fn drain_returns(&mut self) {
        while self.returns.pop().is_ok() {
            self.outstanding = self.outstanding.saturating_sub(1);
        }
    }

    pub(crate) fn start(
        &mut self,
        slot: usize,
        port: PortAddress,
        rows: usize,
        decimation: u64,
    ) -> Result<u32, VectorError> {
        if slot >= VECTOR_SLOTS {
            return Err(VectorError::Slot);
        }
        if decimation == 0 {
            return Err(VectorError::Decimation);
        }
        if rows == 0 || rows > VECTOR_MAX_ROWS {
            return Err(VectorError::Rows);
        }
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        let generation = self.next_generation;
        self.send(Command::Start {
            slot,
            generation,
            port,
            rows,
            decimation,
        })?;
        if let Some(g) = self.generations.get_mut(slot) {
            *g = generation;
        }
        Ok(generation)
    }

    pub(crate) fn stop(&mut self, slot: usize) -> Result<(), VectorError> {
        if slot >= VECTOR_SLOTS {
            return Err(VectorError::Slot);
        }
        self.send(Command::Stop { slot })?;
        if let Some(g) = self.generations.get_mut(slot) {
            *g = 0;
        }
        Ok(())
    }

    /// Hands each snapshot of a running slot's current start to `snapshot`.
    pub(crate) fn poll(&mut self, mut snapshot: impl FnMut(&VectorSnapshot)) {
        self.drain_returns();
        while let Ok(s) = self.snapshots.pop() {
            if self.generations.get(s.slot) == Some(&s.generation) && s.generation != 0 {
                snapshot(&s);
            }
        }
    }

    pub(crate) fn bound(counts: &VectorCounts, slot: usize) -> bool {
        counts
            .bound
            .get(slot)
            .is_some_and(|b| b.load(Ordering::Relaxed))
    }

    pub(crate) fn overflows(counts: &VectorCounts) -> u64 {
        counts.overflows.load(Ordering::Relaxed)
    }
}

/// A vector output to snapshot offline: `rows` rows of `port`, at every
/// frame that is a multiple of `decimation`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorTap {
    /// The vector output.
    pub port: PortAddress,
    /// Its rows.
    pub rows: usize,
    /// Frames between snapshots.
    pub decimation: u64,
}

/// What a [`VectorTap`] took: per snapshot its frame and rows, at the
/// frames the real-time adapter takes them. The rows are the engine's `f64`
/// values; the real-time stream carries them `as f32`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VectorTrack {
    /// `(frame, rows)` in frame order.
    pub snapshots: Vec<(Frame, Vec<f64>)>,
}

/// Offline snapshots of [`VectorTap`]s (allocates; not for the audio
/// thread): what `crate::render` hands a replay.
#[derive(Debug, Default)]
pub(crate) struct VectorTaps {
    taps: Vec<(VectorTap, Option<usize>)>,
    pub(crate) tracks: Vec<VectorTrack>,
}

impl VectorTaps {
    pub(crate) fn new(taps: &[VectorTap]) -> VectorTaps {
        VectorTaps {
            taps: taps.iter().map(|t| (t.clone(), None)).collect(),
            tracks: vec![VectorTrack::default(); taps.len()],
        }
    }

    pub(crate) fn bind(&mut self, engine: &Engine) {
        for (t, first) in &mut self.taps {
            *first = tap(engine, &t.port, t.rows.min(VECTOR_MAX_ROWS));
        }
    }

    pub(crate) fn block(&mut self, engine: &Engine, first: u64, frames: usize) {
        for ((tap, at), track) in self.taps.iter().zip(&mut self.tracks) {
            let Some(row) = *at else {
                continue;
            };
            for f in multiples(first, frames, tap.decimation.max(1)) {
                let values = (0..tap.rows.min(VECTOR_MAX_ROWS))
                    .map(|r| engine.signal_output_at(row + r, f).unwrap_or(0.0))
                    .collect();
                track.snapshots.push((Frame(first + f as u64), values));
            }
        }
    }
}
