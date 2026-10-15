//! The vector adapter (M2.8-7, beside the scope adapter): snapshots of a
//! vector output of the running plan (a visual-rate model's state) for
//! `<signal-canvas>`. The control thread starts a slot on a port with its
//! rows and a decimation `d`; the audio thread then keeps, at every absolute
//! frame that is a multiple of `d`, the port's rows as its readers saw them,
//! one [`VectorSnapshot`] per frame, pushed to a preallocated queue (a full
//! queue drops the snapshot and counts it). The decimation is a multiple of
//! the port's rate divisor, so snapshots fall on its ticks and do not
//! depend on how frames were split into callbacks: a start whose decimation
//! is not a multiple of the port's divisor in the plan it names is refused
//! ([`vector_shape`]); a port that plan lacks is not, and binds when a later
//! plan has it.
//!
//! The check holds after swaps too (M2.8-9): the engine does not say a port's
//! divisor, so the control thread, through which every swap of the run goes
//! ([`Controller::swap`](crate::Controller::swap)), sends each slot a
//! **verdict** per plan version (the engine's own count, `Engine::version`):
//! whether its decimation is a multiple of the port's divisor in that plan.
//! It keeps the running plan and those sent since, so a start carries a
//! verdict for each, and a swap's verdicts go before the swap where the queue
//! has room (withdrawn if the engine refuses it), else after it. A slot binds
//! only on an ok verdict for the running version; without one it stays
//! unbound ([`VectorUnbound`]), so it is never sampled off its port's ticks.
//!
//! As for scopes, nothing is allocated or freed on the audio thread: slots,
//! snapshots and queues are made with the run, and a port replaced by a
//! command goes back to the control thread to be dropped there.
//!
//! [`VectorTaps`] takes the same snapshots offline (a replay, `crate::render`),
//! so a recorded session's visual streams can be compared bit for bit.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};
use signal_core::{Direction, Frame, PortAddress, PortShape};
use signal_engine::Engine;
use signal_graph::ExecutionPlan;

/// Vector slots per run.
pub const VECTOR_SLOTS: usize = 8;
/// The most rows a vector stream carries.
pub const VECTOR_MAX_ROWS: usize = 512;
/// Snapshots the queue to the control thread holds.
const VECTOR_QUEUE: usize = 64;
/// Commands outstanding (sent, their replaced port not yet returned).
const VECTOR_COMMANDS: usize = 32;
/// Plan versions a slot holds a verdict for: the running one and those sent
/// since.
const VERDICTS: usize = 4;

/// A slot's verdicts: (plan version, decimation on the port's ticks).
type Verdicts = [Option<(u64, bool)>; VERDICTS];

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
    /// The decimation must be a positive multiple of the port's rate divisor.
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
            VectorError::Decimation => {
                "the decimation must be a positive multiple of the port's rate divisor"
            }
            VectorError::Rows => "a vector stream has 1 to 512 rows",
            VectorError::Full => "too many vector commands outstanding",
        })
    }
}

impl std::error::Error for VectorError {}

/// Why a started vector slot (or an offline tap) is not bound to its port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VectorUnbound {
    /// The running plan has no vector output of that width at the port.
    Absent,
    /// Its decimation is not a multiple of the port's rate divisor in the
    /// running plan: it would sample frames that are not the port's ticks.
    Decimation,
    /// No verdict for the running plan version reached the slot (a swap the
    /// adapter did not see, or one beyond the versions it keeps).
    Unverified,
}

impl VectorUnbound {
    fn code(this: Option<VectorUnbound>) -> u8 {
        match this {
            None => 0,
            Some(VectorUnbound::Absent) => 1,
            Some(VectorUnbound::Decimation) => 2,
            Some(VectorUnbound::Unverified) => 3,
        }
    }

    fn from_code(code: u8) -> Option<VectorUnbound> {
        match code {
            1 => Some(VectorUnbound::Absent),
            2 => Some(VectorUnbound::Decimation),
            3 => Some(VectorUnbound::Unverified),
            _ => None,
        }
    }
}

/// Whether a stream of `port` every `decimation` frames samples only its
/// ticks in `plan`: a port the plan lacks is never sampled, so it passes.
fn on_ticks(plan: &ExecutionPlan, port: &PortAddress, decimation: u64) -> bool {
    vector_shape(plan, port).is_none_or(|(_, d)| decimation.is_multiple_of(d.max(1)))
}

/// The verdict for plan `version`, if the slot holds one.
fn verdict(verdicts: &Verdicts, version: u64) -> Option<bool> {
    verdicts
        .iter()
        .flatten()
        .find(|(v, _)| *v == version)
        .map(|&(_, ok)| ok)
}

/// Records `ok` for `version`, replacing that version's or else the oldest.
fn record(verdicts: &mut Verdicts, version: u64, ok: bool) {
    let at = verdicts
        .iter()
        .position(|v| v.is_some_and(|(v, _)| v == version))
        .or_else(|| verdicts.iter().position(Option::is_none))
        .or_else(|| {
            let oldest = verdicts
                .iter()
                .enumerate()
                .min_by_key(|(_, v)| v.map_or(0, |(v, _)| v));
            oldest.map(|(i, _)| i)
        });
    if let Some(entry) = at.and_then(|i| verdicts.get_mut(i)) {
        *entry = Some((version, ok));
    }
}

/// Vector output `port` of `plan`: its elements and rate divisor; `None`
/// when the plan has no such vector output of at most [`VECTOR_MAX_ROWS`]
/// elements.
pub fn vector_shape(plan: &ExecutionPlan, port: &PortAddress) -> Option<(usize, u64)> {
    let instance = plan
        .instances
        .iter()
        .find(|i| i.address == port.instance())?;
    instance
        .descriptor
        .ports
        .iter()
        .find_map(|p| match p.shape {
            PortShape::Vector(n) if p.name == port.port() && p.direction == Direction::Out => {
                Some((n as usize, u64::from(p.rate.get())))
            }
            _ => None,
        })
        .filter(|&(n, _)| n <= VECTOR_MAX_ROWS)
}

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
        verdicts: Verdicts,
    },
    Stop {
        slot: usize,
    },
    /// The verdicts of plan `version`, per slot of start `generations`.
    Verdicts {
        version: u64,
        ok: [bool; VECTOR_SLOTS],
        generations: [u32; VECTOR_SLOTS],
    },
}

/// What the audio thread publishes for the control thread.
#[derive(Debug, Default)]
pub(crate) struct VectorCounts {
    overflows: AtomicU64,
    bound: [AtomicBool; VECTOR_SLOTS],
    unbound: [AtomicU8; VECTOR_SLOTS],
}

struct Slot {
    port: Option<PortAddress>,
    generation: u32,
    rows: usize,
    decimation: u64,
    /// The port's first row in the running plan, while it has it.
    tap: Option<usize>,
    verdicts: Verdicts,
    unbound: Option<VectorUnbound>,
    snapshot: VectorSnapshot,
}

impl Slot {
    /// Binds to the port in the running plan: the same width, and an ok
    /// verdict for the running plan version. Allocates nothing.
    fn bind(&mut self, engine: &Engine) {
        self.tap = None;
        self.unbound = None;
        let Some(port) = &self.port else {
            return;
        };
        let Some(first) = tap(engine, port, self.rows) else {
            self.unbound = Some(VectorUnbound::Absent);
            return;
        };
        match verdict(&self.verdicts, engine.version()) {
            Some(true) => self.tap = Some(first),
            Some(false) => self.unbound = Some(VectorUnbound::Decimation),
            None => self.unbound = Some(VectorUnbound::Unverified),
        }
    }
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
    /// Each started slot's port and decimation.
    started: [Option<(PortAddress, u64)>; VECTOR_SLOTS],
    /// The plans by engine version: the running one and every one sent since.
    plans: VecDeque<(u64, ExecutionPlan)>,
    /// Verdicts the command queue had no room for, sent at the next chance.
    unsent: Vec<(u64, [bool; VECTOR_SLOTS], [u32; VECTOR_SLOTS])>,
}

/// Both halves, made with the run of `plan` (engine version 0).
pub(crate) fn vectors(plan: &ExecutionPlan) -> (Vectors, VectorControl) {
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
            verdicts: [None; VERDICTS],
            unbound: None,
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
            started: Default::default(),
            plans: VecDeque::from([(0, plan.clone())]),
            unsent: Vec::new(),
        },
    )
}

impl Vectors {
    /// Takes the commands sent since the last callback. Allocates nothing.
    pub(crate) fn commands(&mut self, engine: &Engine) {
        while let Ok(command) = self.commands.pop() {
            let (slot, port, generation, rows, decimation, verdicts) = match command {
                Command::Start {
                    slot,
                    generation,
                    port,
                    rows,
                    decimation,
                    verdicts,
                } => (slot, Some(port), generation, rows, decimation, verdicts),
                Command::Stop { slot } => (slot, None, 0, 0, 1, [None; VERDICTS]),
                Command::Verdicts {
                    version,
                    ok,
                    generations,
                } => {
                    // Before the swap they belong to commits; a late one
                    // binds its slot now.
                    for ((s, ok), generation) in self.slots.iter_mut().zip(ok).zip(generations) {
                        if generation != 0 && s.generation == generation {
                            record(&mut s.verdicts, version, ok);
                            s.bind(engine);
                        }
                    }
                    continue;
                }
            };
            let Some(s) = self.slots.get_mut(slot) else {
                let _ = self.returns.push(port);
                continue;
            };
            s.rows = rows.min(VECTOR_MAX_ROWS);
            let old = std::mem::replace(&mut s.port, port);
            s.generation = generation;
            s.decimation = decimation.max(1);
            s.verdicts = verdicts;
            s.bind(engine);
            let _ = self.returns.push(old);
        }
    }

    /// After a swap (and before the first block): each slot finds its port
    /// in the running plan with an ok verdict for it, or waits unbound.
    pub(crate) fn bind(&mut self, engine: &Engine) {
        // A swap's verdicts were queued before the swap: those not taken at
        // the callback's start are taken now.
        self.commands(engine);
        for s in &mut self.slots {
            s.bind(engine);
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
        for ((s, bound), unbound) in self.slots.iter().zip(&counts.bound).zip(&counts.unbound) {
            bound.store(s.tap.is_some(), Ordering::Relaxed);
            unbound.store(VectorUnbound::code(s.unbound), Ordering::Relaxed);
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
        self.send_verdicts();
    }

    /// Forgets the plans older than the running one (engine version
    /// `running`), which stays.
    fn prune(&mut self, running: u64) {
        while self.plans.len() > 1 && self.plans.get(1).is_some_and(|(v, _)| *v <= running) {
            self.plans.pop_front();
        }
    }

    /// Sends the verdicts the queue had no room for, in order.
    fn send_verdicts(&mut self) {
        while let Some(&(version, ok, generations)) = self.unsent.first() {
            let command = Command::Verdicts {
                version,
                ok,
                generations,
            };
            if self.commands.push(command).is_err() {
                return;
            }
            self.unsent.remove(0);
        }
    }

    /// Every started slot's verdict in `plan`.
    fn verdicts_in(&self, plan: &ExecutionPlan) -> [bool; VECTOR_SLOTS] {
        let mut ok = [true; VECTOR_SLOTS];
        for (ok, started) in ok.iter_mut().zip(&self.started) {
            if let Some((port, decimation)) = started {
                *ok = on_ticks(plan, port, *decimation);
            }
        }
        ok
    }

    /// The version the next swap the engine takes will make.
    fn next_version(&self) -> u64 {
        self.plans.back().map_or(0, |(v, _)| v + 1)
    }

    /// Before a swap to `plan` is sent (the engine running version
    /// `running`): every started slot's verdict for the version it would
    /// make, sent now so it is there when the swap commits, but only while
    /// the queue holds nothing older and has room for it and for its
    /// retraction ([`refused`](VectorControl::refused)), so a swap the
    /// engine refuses never leaves a verdict behind. Whether it was sent.
    pub(crate) fn before_swap(&mut self, running: u64, plan: &ExecutionPlan) -> bool {
        self.prune(running);
        self.send_verdicts();
        if !self.unsent.is_empty() || self.commands.slots() < 2 {
            return false;
        }
        let command = Command::Verdicts {
            version: self.next_version(),
            ok: self.verdicts_in(plan),
            generations: self.generations,
        };
        self.commands.push(command).is_ok()
    }

    /// The engine refused the swap whose verdicts went `early`: they are
    /// withdrawn (no slot binds on them), in the room kept for that.
    pub(crate) fn refused(&mut self, early: bool) {
        if early {
            self.unsent
                .push((self.next_version(), [false; VECTOR_SLOTS], self.generations));
            self.send_verdicts();
        }
    }

    /// After the engine took the swap to `plan`: it is the next version.
    /// Verdicts that did not go `early` go now (until they arrive, the
    /// slots stay unbound on it).
    pub(crate) fn swapped(&mut self, plan: &ExecutionPlan, early: bool) {
        let version = self.next_version();
        if !early {
            self.unsent
                .push((version, self.verdicts_in(plan), self.generations));
            self.send_verdicts();
        }
        self.plans.push_back((version, plan.clone()));
    }

    /// Starts `slot` (the engine running version `running`): `plan` is the
    /// caller's, checked up front; the slot's verdicts come from the plans
    /// kept, the running one first.
    pub(crate) fn start(
        &mut self,
        running: u64,
        plan: &ExecutionPlan,
        slot: usize,
        port: PortAddress,
        rows: usize,
        decimation: u64,
    ) -> Result<u32, VectorError> {
        if slot >= VECTOR_SLOTS {
            return Err(VectorError::Slot);
        }
        let divisor = vector_shape(plan, &port).map_or(1, |(_, d)| d.max(1));
        if decimation == 0 || !decimation.is_multiple_of(divisor) {
            return Err(VectorError::Decimation);
        }
        if rows == 0 || rows > VECTOR_MAX_ROWS {
            return Err(VectorError::Rows);
        }
        self.prune(running);
        let mut verdicts = [None; VERDICTS];
        for (verdict, (version, kept)) in verdicts.iter_mut().zip(&self.plans) {
            *verdict = Some((*version, on_ticks(kept, &port, decimation)));
        }
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        let generation = self.next_generation;
        self.send(Command::Start {
            slot,
            generation,
            port: port.clone(),
            rows,
            decimation,
            verdicts,
        })?;
        if let Some(g) = self.generations.get_mut(slot) {
            *g = generation;
        }
        if let Some(s) = self.started.get_mut(slot) {
            *s = Some((port, decimation));
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
        if let Some(s) = self.started.get_mut(slot) {
            *s = None;
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

    pub(crate) fn unbound(counts: &VectorCounts, slot: usize) -> Option<VectorUnbound> {
        counts
            .unbound
            .get(slot)
            .and_then(|u| VectorUnbound::from_code(u.load(Ordering::Relaxed)))
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
    /// From which frame, and why, a plan refused the tap: its decimation off
    /// the port's ticks there, or no plan given for that version.
    pub refused: Vec<(Frame, VectorUnbound)>,
}

/// Offline snapshots of [`VectorTap`]s (allocates; not for the audio
/// thread): what `crate::render` hands a replay.
#[derive(Debug, Default)]
pub(crate) struct VectorTaps {
    taps: Vec<(VectorTap, Option<usize>)>,
    /// Per plan version (`Engine::version`), per tap: on its port's ticks.
    verdicts: Vec<Vec<bool>>,
    pub(crate) tracks: Vec<VectorTrack>,
}

impl VectorTaps {
    /// `plans[v]` is the plan the engine runs at version `v`.
    pub(crate) fn new(taps: &[VectorTap], plans: &[ExecutionPlan]) -> VectorTaps {
        VectorTaps {
            taps: taps.iter().map(|t| (t.clone(), None)).collect(),
            verdicts: plans
                .iter()
                .map(|plan| {
                    taps.iter()
                        .map(|t| on_ticks(plan, &t.port, t.decimation.max(1)))
                        .collect()
                })
                .collect(),
            tracks: vec![VectorTrack::default(); taps.len()],
        }
    }

    /// After a swap (and before the first block), as the real-time slots do:
    /// a tap whose decimation is off its port's ticks in the running plan,
    /// or whose plan version has no plan, is refused there and recorded.
    pub(crate) fn bind(&mut self, engine: &Engine) {
        let version = usize::try_from(engine.version()).unwrap_or(usize::MAX);
        for (k, ((t, first), track)) in self.taps.iter_mut().zip(&mut self.tracks).enumerate() {
            *first = tap(engine, &t.port, t.rows.min(VECTOR_MAX_ROWS));
            let refused = match self.verdicts.get(version).and_then(|v| v.get(k)) {
                _ if first.is_none() => None,
                Some(true) => None,
                Some(false) => Some(VectorUnbound::Decimation),
                None => Some(VectorUnbound::Unverified),
            };
            if let Some(reason) = refused {
                *first = None;
                track.refused.push((engine.next_frame(), reason));
            }
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use signal_core::Registry;
    use signal_graph::{CompositeDef, Exports, InstanceDef, build_plan};

    use super::*;

    /// M2.8-10 AC8: a start whose decimation is not a multiple of the port's
    /// divisor in the plan is refused; a multiple is accepted; a port the
    /// plan lacks stays armed (it binds when a later plan has it).
    #[test]
    fn a_start_checks_the_decimation_against_the_ports_divisor() {
        let mut registry = Registry::new();
        signal_std::register(&mut registry).unwrap();
        let root = CompositeDef {
            name: "project".to_owned(),
            instances: vec![InstanceDef {
                name: "sparks".to_owned(),
                module: "std.particles".to_owned(),
                parameters: BTreeMap::new(),
            }],
            connections: Vec::new(),
            exports: Exports::default(),
        };
        let plan = build_plan(&root, &[], &registry, 0).unwrap();
        let port = |text: &str| -> PortAddress { text.parse().unwrap() };
        assert_eq!(vector_shape(&plan, &port("/sparks.vel")), Some((128, 800)));
        let (_vectors, mut control) = vectors(&plan);
        let mut start =
            |p: &str, decimation: u64| control.start(0, &plan, 0, port(p), 128, decimation);
        assert_eq!(start("/sparks.vel", 1200), Err(VectorError::Decimation));
        assert_eq!(start("/sparks.vel", 0), Err(VectorError::Decimation));
        assert!(start("/sparks.vel", 1600).is_ok());
        assert!(start("/later.vel", 3).is_ok());
    }
}
