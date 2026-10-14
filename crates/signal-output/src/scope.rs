//! The scope adapter (M2.8-1, beside the audio adapter of ADR 0022): a
//! decimated capture of any signal output of the running plan, for display.
//! The control thread starts a slot on an output port with a decimation `d`;
//! the audio thread then keeps every output value at an absolute frame that
//! is a multiple of `d` (so the stream does not depend on how the frames
//! were split into callbacks), as its readers saw it (fades included), in
//! blocks of [`SCOPE_POINTS`] points pushed to a preallocated queue. A full
//! queue drops the block and counts it.
//!
//! Nothing is allocated or freed on the audio thread: the slots, the blocks
//! and the queues are made with the run; a port address travels to the audio
//! thread in a command and the one it replaces comes back on a return queue,
//! to be dropped on the control thread. The control side keeps at most as
//! many commands outstanding as the return queue holds, so it is never full.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};
use signal_core::{Frame, PortAddress};
use signal_engine::Engine;

/// Scope slots per run.
pub const SCOPE_SLOTS: usize = 8;
/// Points per [`ScopeBlock`].
pub const SCOPE_POINTS: usize = 256;
/// Blocks the queue to the control thread holds.
const SCOPE_QUEUE: usize = 128;
/// Commands outstanding (sent, their replaced port not yet returned).
const SCOPE_COMMANDS: usize = 32;

/// Consecutive points of one slot: point *k* is the output at frame
/// `first_frame + k × decimation`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScopeBlock {
    /// The slot.
    pub slot: usize,
    /// The slot's start it belongs to ([`Controller::scope_start`](crate::Controller::scope_start)).
    pub generation: u32,
    /// The frame of point 0.
    pub first_frame: Frame,
    /// Frames between points.
    pub decimation: u64,
    /// Points used.
    pub len: usize,
    /// The points.
    pub points: [f32; SCOPE_POINTS],
}

impl ScopeBlock {
    const EMPTY: ScopeBlock = ScopeBlock {
        slot: 0,
        generation: 0,
        first_frame: Frame(0),
        decimation: 1,
        len: 0,
        points: [0.0; SCOPE_POINTS],
    };

    /// The points used.
    pub fn values(&self) -> &[f32] {
        self.points.get(..self.len).unwrap_or(&[])
    }
}

/// Why a scope command was not sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeError {
    /// No such slot.
    Slot,
    /// The decimation must be at least 1.
    Decimation,
    /// Too many commands outstanding; poll and try again.
    Full,
}

impl std::fmt::Display for ScopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ScopeError::Slot => "no such scope slot",
            ScopeError::Decimation => "the decimation must be at least 1",
            ScopeError::Full => "too many scope commands outstanding",
        })
    }
}

impl std::error::Error for ScopeError {}

/// A command to the audio thread.
enum Command {
    Start {
        slot: usize,
        generation: u32,
        port: PortAddress,
        decimation: u64,
    },
    Stop {
        slot: usize,
    },
}

/// What the audio thread publishes for the control thread.
#[derive(Debug, Default)]
pub(crate) struct ScopeCounts {
    overflows: AtomicU64,
    bound: [AtomicBool; SCOPE_SLOTS],
}

/// One slot on the audio thread.
struct Slot {
    port: Option<PortAddress>,
    generation: u32,
    decimation: u64,
    /// The output's index in the running plan, while it has the port.
    output: Option<usize>,
    /// The block being filled.
    block: ScopeBlock,
    /// The frame the next point is expected at, while a block is open.
    next: Option<u64>,
}

/// The audio-thread half, inside the [`Runner`](crate::Runner).
pub(crate) struct Scopes {
    slots: Vec<Slot>,
    commands: Consumer<Command>,
    returns: Producer<Option<PortAddress>>,
    blocks: Producer<ScopeBlock>,
    overflows: u64,
}

/// The control-thread half, inside the [`Controller`](crate::Controller).
pub(crate) struct ScopeControl {
    commands: Producer<Command>,
    returns: Consumer<Option<PortAddress>>,
    blocks: Consumer<ScopeBlock>,
    /// The generation each slot runs, 0 when stopped.
    generations: [u32; SCOPE_SLOTS],
    next_generation: u32,
    outstanding: usize,
}

/// Both halves, made with the run.
pub(crate) fn scopes() -> (Scopes, ScopeControl) {
    let (commands_tx, commands_rx) = RingBuffer::new(SCOPE_COMMANDS);
    let (returns_tx, returns_rx) = RingBuffer::new(SCOPE_COMMANDS);
    let (blocks_tx, blocks_rx) = RingBuffer::new(SCOPE_QUEUE);
    let slots = (0..SCOPE_SLOTS)
        .map(|slot| Slot {
            port: None,
            generation: 0,
            decimation: 1,
            output: None,
            block: ScopeBlock {
                slot,
                ..ScopeBlock::EMPTY
            },
            next: None,
        })
        .collect();
    (
        Scopes {
            slots,
            commands: commands_rx,
            returns: returns_tx,
            blocks: blocks_tx,
            overflows: 0,
        },
        ScopeControl {
            commands: commands_tx,
            returns: returns_rx,
            blocks: blocks_rx,
            generations: [0; SCOPE_SLOTS],
            next_generation: 0,
            outstanding: 0,
        },
    )
}

impl Scopes {
    /// Takes the commands sent since the last callback. Allocates nothing:
    /// a replaced port goes back on the return queue.
    pub(crate) fn commands(&mut self, engine: &Engine) {
        while let Ok(command) = self.commands.pop() {
            let (slot, port, generation, decimation) = match command {
                Command::Start {
                    slot,
                    generation,
                    port,
                    decimation,
                } => (slot, Some(port), generation, decimation),
                Command::Stop { slot } => (slot, None, 0, 1),
            };
            let Some(s) = self.slots.get_mut(slot) else {
                // Never sent (the control side checks the slot); a port
                // would be dropped here otherwise, so hand it back.
                let _ = self.returns.push(port);
                continue;
            };
            s.output = port.as_ref().and_then(|p| engine.signal_output_index(p));
            let old = std::mem::replace(&mut s.port, port);
            s.generation = generation;
            s.decimation = decimation.max(1);
            s.block.len = 0;
            s.next = None;
            // The control side keeps the return queue from filling.
            let _ = self.returns.push(old);
        }
    }

    /// After a swap (and before the first block): each slot finds its port
    /// in the running plan, or waits unbound while the plan lacks it.
    pub(crate) fn bind(&mut self, engine: &Engine) {
        for s in &mut self.slots {
            s.output = s.port.as_ref().and_then(|p| engine.signal_output_index(p));
            s.block.len = 0;
            s.next = None;
        }
    }

    /// Captures block frames `0..frames`, the first at absolute frame `first`.
    pub(crate) fn block(&mut self, engine: &Engine, first: u64, frames: usize) {
        for s in &mut self.slots {
            let Some(o) = s.output else {
                continue;
            };
            let d = s.decimation;
            // The first frame of the block that is a multiple of `d`.
            let mut f = ((d - first % d) % d) as usize;
            while f < frames {
                let frame = first + f as u64;
                if s.next.is_some_and(|next| next != frame) {
                    // A gap: points of a block are consecutive.
                    s.block.len = 0;
                }
                if s.block.len == 0 {
                    s.block.first_frame = Frame(frame);
                    s.block.decimation = d;
                    s.block.generation = s.generation;
                }
                let value = engine.signal_output_at(o, f).unwrap_or(0.0) as f32;
                if let Some(point) = s.block.points.get_mut(s.block.len) {
                    *point = value;
                }
                s.block.len += 1;
                s.next = Some(frame + d);
                if s.block.len >= SCOPE_POINTS {
                    if self.blocks.push(s.block).is_err() {
                        self.overflows = self.overflows.saturating_add(1);
                    }
                    s.block.len = 0;
                }
                f = f.saturating_add(d as usize);
            }
        }
    }

    /// Publishes the counts for the control thread.
    pub(crate) fn publish(&self, counts: &ScopeCounts) {
        counts.overflows.store(self.overflows, Ordering::Relaxed);
        for (s, bound) in self.slots.iter().zip(&counts.bound) {
            bound.store(s.output.is_some(), Ordering::Relaxed);
        }
    }
}

impl ScopeControl {
    fn send(&mut self, command: Command) -> Result<(), ScopeError> {
        if self.outstanding >= SCOPE_COMMANDS {
            self.drain_returns();
        }
        if self.outstanding >= SCOPE_COMMANDS {
            return Err(ScopeError::Full);
        }
        self.commands.push(command).map_err(|_| ScopeError::Full)?;
        self.outstanding += 1;
        Ok(())
    }

    fn drain_returns(&mut self) {
        // The ports are dropped here, on the control thread.
        while self.returns.pop().is_ok() {
            self.outstanding = self.outstanding.saturating_sub(1);
        }
    }

    pub(crate) fn start(
        &mut self,
        slot: usize,
        port: PortAddress,
        decimation: u64,
    ) -> Result<u32, ScopeError> {
        if slot >= SCOPE_SLOTS {
            return Err(ScopeError::Slot);
        }
        if decimation == 0 {
            return Err(ScopeError::Decimation);
        }
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        let generation = self.next_generation;
        self.send(Command::Start {
            slot,
            generation,
            port,
            decimation,
        })?;
        if let Some(g) = self.generations.get_mut(slot) {
            *g = generation;
        }
        Ok(generation)
    }

    pub(crate) fn stop(&mut self, slot: usize) -> Result<(), ScopeError> {
        if slot >= SCOPE_SLOTS {
            return Err(ScopeError::Slot);
        }
        self.send(Command::Stop { slot })?;
        if let Some(g) = self.generations.get_mut(slot) {
            *g = 0;
        }
        Ok(())
    }

    /// Hands each block of a running slot's current start to `block`;
    /// blocks of a stopped or restarted slot are dropped.
    pub(crate) fn poll(&mut self, mut block: impl FnMut(&ScopeBlock)) {
        self.drain_returns();
        while let Ok(b) = self.blocks.pop() {
            if self.generations.get(b.slot) == Some(&b.generation) && b.generation != 0 {
                block(&b);
            }
        }
    }

    pub(crate) fn bound(counts: &ScopeCounts, slot: usize) -> bool {
        counts
            .bound
            .get(slot)
            .is_some_and(|b| b.load(Ordering::Relaxed))
    }

    pub(crate) fn overflows(counts: &ScopeCounts) -> u64 {
        counts.overflows.load(Ordering::Relaxed)
    }
}
