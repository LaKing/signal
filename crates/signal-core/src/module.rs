//! The primitive module contract (ADR 0002, ADR 0016 §2, §5). Normative text:
//! `docs/module-contract.md`.
//!
//! A primitive implements exactly one of two object-safe traits, chosen by its
//! feedthrough class: [`DirectPrimitive`] or [`StateOutputPrimitive`]. The engine
//! holds instances as [`Primitive`], a box of either.
//!
//! The I/O views are flat and borrowed from buffers the engine allocated before
//! the real-time calls, so building them never allocates. Accessors never panic:
//! an index outside the module's declared ports yields an empty slice or `None`.

use crate::descriptor::{Feedthrough, ModuleDescriptor};
use crate::event::Event;
use crate::registry::Lent;
use crate::rng::RngStream;
use crate::table::Table;
use serde::Serialize;

/// What [`prepare`](DirectPrimitive::prepare) learns about the run it is prepared for.
#[derive(Clone, Debug)]
pub struct PrepareContext {
    /// The master rate in frames per second (e.g. 96000.0).
    pub sample_rate: f64,
    /// The largest number of frames *k* (≥ 1) any later real-time call will be given.
    pub max_frames: usize,
    /// The instance's random stream (ADR 0008). Keep a copy to restart it in `reset`.
    pub rng: RngStream,
    /// What the registry's [`Host`](crate::Host) lends this instance (M2.7-2:
    /// e.g. its GPU device), opaque to the engine; `None` when nothing is
    /// lent. A module that uses it downcasts it and must work without it.
    pub lent: Option<Lent>,
    /// The engine's plan version this instance was created for: 0 for the
    /// plan the engine was built with, *v* + 1 for an instance added by the
    /// swap prepared at version *v* (`docs/semantics.md` §12). Identity data
    /// only: a primitive may use it to tell a re-created instance from one it
    /// has seen, and may carry it in a signal as identity (`std.count.window`
    /// does, in rows 4–6), but no consumer's effect may depend on its value,
    /// only on whether it matches.
    pub generation: u64,
}

/// The inputs of one real-time call, covering `frames` consecutive frames;
/// for a type of rate divisor *d* > 1, its `frames` consecutive ticks (the
/// frames that are multiples of *d*), one value per tick (M2.8-6,
/// `docs/module-contract.md` "Rates and vectors").
///
/// Layout (indices count within each group, in the order of the module
/// descriptor's `ports` and `parameters`):
/// - `signals`: every signal input row, row-major: row *r* is
///   `signals[r * frames .. (r + 1) * frames]`. A signal port is one row, a
///   vector port of N is N rows, its elements in order.
/// - `parameters`: the executor-computed effective value of every parameter for
///   every frame (ADR 0016 §4), in the same layout. Modules never see bases,
///   events on parameters, or modulation separately.
/// - `events`: the events of every event input port, concatenated; port *p*'s
///   events end at `event_ends[p]` and start where port *p − 1*'s end (0 for the
///   first). Within a port they are sorted by offset, and events with the same
///   offset are in the canonical order of ADR 0016 §3.
/// - `connected` (ADR 0033): per port of the descriptor's `ports`, in order,
///   whether the plan gives it at least one connection. It is a property of
///   the plan, not of the values: it changes only at a plan swap, between
///   two calls. An unconnected input still reads 0 (`docs/semantics.md` §3).
/// - `sources_changed` (ADR 0037 §6): per port of the descriptor's `ports`,
///   whether the plan swapped in since the instance's last call gives it
///   another set of sources (source port, scale) than the plan before. Only
///   the first call after such a swap sees it, at its first frame.
#[derive(Clone, Copy, Debug)]
pub struct Inputs<'a> {
    frames: usize,
    signals: &'a [f64],
    parameters: &'a [f64],
    events: &'a [Event],
    event_ends: &'a [usize],
    table: Option<&'a Table>,
    connected: &'a [bool],
    sources_changed: &'a [bool],
}

impl<'a> Inputs<'a> {
    /// Views over engine buffers laid out as described on [`Inputs`].
    pub fn new(
        frames: usize,
        signals: &'a [f64],
        parameters: &'a [f64],
        events: &'a [Event],
        event_ends: &'a [usize],
    ) -> Inputs<'a> {
        Inputs {
            frames,
            signals,
            parameters,
            events,
            event_ends,
            table: None,
            connected: &[],
            sources_changed: &[],
        }
    }

    /// These views with the instance's table (ADR 0028 §4): what the engine
    /// hands an instance whose plan gives it one.
    pub fn with_table(self, table: Option<&'a Table>) -> Inputs<'a> {
        Inputs { table, ..self }
    }

    /// These views with which of the instance's ports have connections
    /// (ADR 0033): `connected[i]` for port *i* of the descriptor's `ports`.
    /// What the engine hands every instance; without it (as from
    /// [`Inputs::new`]) every port reads unconnected.
    pub fn with_connected(self, connected: &'a [bool]) -> Inputs<'a> {
        Inputs { connected, ..self }
    }

    /// Whether port `port` (an index into the descriptor's `ports`) has at
    /// least one connection in the plan (ADR 0033). It changes only at a
    /// plan swap. A module uses it to tell an unconnected input, which reads
    /// 0, from one connected to a source that outputs 0; an index past the
    /// ports, or an output, reads `false`.
    pub fn is_connected(&self, port: usize) -> bool {
        self.connected.get(port).copied().unwrap_or(false)
    }

    /// These views with which of the instance's ports had their sources
    /// changed by a plan swap since its last call (ADR 0037 §6):
    /// `changed[i]` for port *i* of the descriptor's `ports`. What the
    /// engine hands a surviving instance's first call after such a swap;
    /// without it every port reads unchanged.
    pub fn with_sources_changed(self, changed: &'a [bool]) -> Inputs<'a> {
        Inputs {
            sources_changed: changed,
            ..self
        }
    }

    /// Whether a plan swap since this instance's last call changed the set
    /// of sources (source port and scale) of input `port` (an index into
    /// the descriptor's `ports`), ADR 0037 §6. True only in the first call
    /// after that swap, and then of the call's first frame (calls never
    /// span a swap, so it does not depend on how frames are split into
    /// calls); false for an instance the swap added (it has its own first
    /// frame), for an index past the ports, and for an output. A module
    /// uses it to tell a rewired input (another transport's position, say)
    /// from motion on one source: `std.note_seq` and `std.step_seq8` take it
    /// on `follow` as a seek.
    pub fn sources_changed(&self, port: usize) -> bool {
        self.sources_changed.get(port).copied().unwrap_or(false)
    }

    /// The instance's table, if its plan gives it one. It changes only at a
    /// plan swap (a live edit), between two calls; a module reads it in every
    /// call and keeps no index into it across calls.
    pub fn table(&self) -> Option<&'a Table> {
        self.table
    }

    /// The number of frames *k* ≥ 1 in this call.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// The samples of signal input row `port`, one per frame: the port
    /// itself for a type without vector inputs.
    pub fn signal(&self, port: usize) -> &'a [f64] {
        block(self.signals, port, self.frames)
    }

    /// Signal input rows `first .. first + count`, row after row: a vector
    /// port of N from its first row is `rows(first, N)`. Empty if they are
    /// not all there.
    pub fn rows(&self, first: usize, count: usize) -> &'a [f64] {
        let start = first.saturating_mul(self.frames);
        let end = start.saturating_add(count.saturating_mul(self.frames));
        self.signals.get(start..end).unwrap_or(&[])
    }

    /// The number of signal input rows.
    pub fn signal_rows(&self) -> usize {
        self.signals.len() / self.frames.max(1)
    }

    /// The effective values of parameter `index`, one per frame.
    pub fn parameter(&self, index: usize) -> &'a [f64] {
        block(self.parameters, index, self.frames)
    }

    /// The events of event input `port`, with in-block offsets, in canonical order.
    pub fn events(&self, port: usize) -> &'a [Event] {
        let start = match port.checked_sub(1) {
            None => 0,
            Some(previous) => match self.event_ends.get(previous) {
                Some(&end) => end,
                None => return &[],
            },
        };
        match self.event_ends.get(port) {
            Some(&end) => self.events.get(start..end).unwrap_or(&[]),
            None => &[],
        }
    }
}

/// The outputs of one real-time call, covering `frames` consecutive frames
/// (ticks, for a divided rate, as for [`Inputs`]).
///
/// `signals` holds every signal output row, row-major like [`Inputs`]; the
/// module overwrites every sample. `events` has one bounded [`EventSink`] per
/// event output port, in descriptor order.
#[derive(Debug)]
pub struct Outputs<'a> {
    frames: usize,
    signals: &'a mut [f64],
    events: &'a mut [EventSink],
}

impl<'a> Outputs<'a> {
    /// Views over engine buffers laid out as described on [`Outputs`].
    pub fn new(frames: usize, signals: &'a mut [f64], events: &'a mut [EventSink]) -> Outputs<'a> {
        Outputs {
            frames,
            signals,
            events,
        }
    }

    /// The number of frames *k* ≥ 1 in this call.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// The samples of signal output row `port`, one per frame.
    pub fn signal(&mut self, port: usize) -> &mut [f64] {
        self.rows(port, 1)
    }

    /// Signal output rows `first .. first + count`, row after row (a vector
    /// port's elements). Empty if they are not all there.
    pub fn rows(&mut self, first: usize, count: usize) -> &mut [f64] {
        let start = first.saturating_mul(self.frames);
        let end = start.saturating_add(count.saturating_mul(self.frames));
        self.signals.get_mut(start..end).unwrap_or(&mut [])
    }

    /// Every signal output in port order, for modules that write several at once.
    pub fn signals(&mut self) -> std::slice::ChunksMut<'_, f64> {
        self.signals.chunks_mut(self.frames.max(1))
    }

    /// The sink of event output `port`.
    pub fn events(&mut self, port: usize) -> Option<&mut EventSink> {
        self.events.get_mut(port)
    }
}

fn block(data: &[f64], index: usize, frames: usize) -> &[f64] {
    let start = index.saturating_mul(frames);
    let end = start.saturating_add(frames);
    data.get(start..end).unwrap_or(&[])
}

/// A bounded buffer for the events one output port emits in a call.
///
/// The engine creates it with a fixed capacity outside the real-time path and
/// clears it after collecting the events. [`push`](EventSink::push) never
/// allocates or panics: when the sink is full the event is dropped and the
/// overflow counter, a fault the engine reports, is raised. A clone reserves
/// the full capacity too, so this holds for clones as well.
///
/// A sink made with [`per_frame`](EventSink::per_frame) limits each frame of
/// a call instead of the call: an event counts against the frame of its
/// offset (an offset past the call's last frame counts against the last
/// frame), and the earliest frame that dropped an event is kept, so a block
/// call overflows at exactly the frames a run of one-frame calls would.
#[derive(Debug, Default)]
pub struct EventSink {
    events: Vec<Event>,
    capacity: usize,
    overflows: u64,
    /// Per-frame mode: the most events kept per frame (0: one limit per call).
    per_frame: usize,
    /// Per-frame mode: the events kept per frame of the current call.
    counts: Vec<usize>,
    /// Per-frame mode: the frames of the current call.
    frames: usize,
    first_overflow: Option<usize>,
}

impl Clone for EventSink {
    /// Allocates the full capacity (a derived clone would reserve only the
    /// kept events, and a later `push` would allocate). Not for the real-time path.
    fn clone(&self) -> EventSink {
        let mut events = Vec::with_capacity(self.capacity);
        events.extend_from_slice(&self.events);
        EventSink {
            events,
            capacity: self.capacity,
            overflows: self.overflows,
            per_frame: self.per_frame,
            counts: self.counts.clone(),
            frames: self.frames,
            first_overflow: self.first_overflow,
        }
    }
}

impl EventSink {
    /// A sink holding up to `capacity` events. Allocates: not for the real-time path.
    pub fn with_capacity(capacity: usize) -> EventSink {
        EventSink {
            events: Vec::with_capacity(capacity),
            capacity,
            ..EventSink::default()
        }
    }

    /// A sink keeping up to `per_frame` events in each frame of calls of up to
    /// `max_frames` frames. Allocates: not for the real-time path. Call
    /// [`start_call`](EventSink::start_call) before each call.
    pub fn per_frame(per_frame: usize, max_frames: usize) -> EventSink {
        let max_frames = max_frames.max(1);
        let capacity = per_frame.saturating_mul(max_frames);
        EventSink {
            events: Vec::with_capacity(capacity),
            capacity,
            overflows: 0,
            per_frame,
            counts: vec![0; max_frames],
            frames: 1,
            first_overflow: None,
        }
    }

    /// Per-frame mode: removes the kept events and readies the sink for a call
    /// of `frames` frames (at most the `max_frames` it was made for), with no
    /// frame counted and no overflow seen. Keeps the storage and the overflow
    /// count; allocates nothing.
    pub fn start_call(&mut self, frames: usize) {
        self.events.clear();
        self.frames = frames.clamp(1, self.counts.len().max(1));
        for count in self.counts.iter_mut().take(self.frames) {
            *count = 0;
        }
        self.first_overflow = None;
    }

    /// Per-frame mode: the earliest frame of the current call that dropped an
    /// event, if any.
    pub fn first_overflow(&self) -> Option<usize> {
        self.first_overflow
    }

    /// Appends `event`, or drops it and raises the overflow counter when full.
    /// Returns whether the event was kept.
    pub fn push(&mut self, event: Event) -> bool {
        if self.per_frame > 0 {
            let frame = (event.offset as usize).min(self.frames.saturating_sub(1));
            if let Some(count) = self.counts.get_mut(frame)
                && *count < self.per_frame
                && self.events.len() < self.capacity
            {
                *count += 1;
                self.events.push(event);
                return true;
            }
            self.overflows = self.overflows.saturating_add(1);
            self.first_overflow = Some(self.first_overflow.map_or(frame, |f| f.min(frame)));
            return false;
        }
        // `len < capacity <= Vec::capacity`, so this push never reallocates.
        if self.events.len() < self.capacity {
            self.events.push(event);
            true
        } else {
            self.overflows = self.overflows.saturating_add(1);
            false
        }
    }

    /// The events kept since the last [`clear`](EventSink::clear), in emission order.
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// The maximum number of events kept between clears.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// How many events were dropped because the sink was full, since creation.
    pub fn overflows(&self) -> u64 {
        self.overflows
    }

    /// Removes the kept events, keeping the storage and the overflow count.
    pub fn clear(&mut self) {
        let frames = self.frames;
        self.start_call(frames);
    }
}

/// A primitive of the **direct** feedthrough class: any output may depend on
/// same-frame inputs (ADR 0016 §2). Called once per block with `process`.
///
/// Object safe: the engine holds `Box<dyn DirectPrimitive>`.
///
/// # Example
///
/// ```
/// use signal_core::{
///     Combine, DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor,
///     Outputs, ParameterDescriptor, PortDescriptor, PortKind, PortShape,
///     PrepareContext, RateDivisor, Range,
/// };
///
/// /// `out = in * gain`.
/// struct Gain;
///
/// const fn signal(name: &'static str, direction: Direction) -> PortDescriptor {
///     PortDescriptor {
///         name,
///         direction,
///         shape: PortShape::Signal,
///         rate: RateDivisor::ONE,
///         kind: PortKind::Audio,
///         unit: None,
///         range: None,
///     }
/// }
///
/// const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
///     type_name: "example.gain",
///     doc: "Multiplies its input by `gain`.",
///     ports: &[signal("in", Direction::In), signal("out", Direction::Out)],
///     parameters: &[ParameterDescriptor {
///         name: "gain",
///         kind: PortKind::Gain,
///         rate: RateDivisor::ONE,
///         unit: None,
///         range: Range { min: 0.0, max: 4.0 },
///         default: 1.0,
///         combine: Combine::Multiply,
///         values: None,
///         snap: signal_core::Snap::Nearest,
///     }],
///     feedthrough: Feedthrough::Direct,
///     sink: None,
///     latency: None,
///     reproducibility: signal_core::Reproducibility::BitExact,
///     category: None,
/// };
///
/// impl DirectPrimitive for Gain {
///     fn describe() -> ModuleDescriptor {
///         DESCRIPTOR
///     }
///
///     fn prepare(&mut self, _ctx: &PrepareContext) {}
///
///     fn reset(&mut self) {}
///
///     fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
///         let samples = inputs.signal(0).iter().zip(inputs.parameter(0));
///         for (out, (x, gain)) in outputs.signal(0).iter_mut().zip(samples) {
///             *out = x * gain;
///         }
///     }
/// }
///
/// // What the engine does for one block of k = 2 frames:
/// let mut gain: Box<dyn DirectPrimitive> = Box::new(Gain);
/// let mut out = [0.0; 2];
/// gain.process(
///     &Inputs::new(2, &[1.0, -0.5], &[2.0, 2.0], &[], &[]),
///     &mut Outputs::new(2, &mut out, &mut []),
/// );
/// assert_eq!(out, [2.0, -1.0]);
/// ```
pub trait DirectPrimitive: Send {
    /// The module type's descriptor, available without an instance. Its
    /// `feedthrough` is [`Feedthrough::Direct`].
    fn describe() -> ModuleDescriptor
    where
        Self: Sized;

    /// Prepares the instance for a run: size buffers for `ctx.max_frames`, keep
    /// `ctx.rng`, precompute tables. The only call that may allocate; never on
    /// the audio thread. Leaves the instance in its initial state.
    fn prepare(&mut self, ctx: &PrepareContext);

    /// Returns to the state `prepare` left, including random streams. Real-time
    /// safe: no allocation, locks, I/O or panics.
    fn reset(&mut self);

    /// Computes `inputs.frames()` frames: reads inputs, parameters and events,
    /// writes every output sample and may push events. Real-time rules apply, and
    /// the result must not depend on how frames are split into calls (ADR 0002).
    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>);
}

/// A primitive of the **state-output** feedthrough class: no output depends on
/// same-frame inputs; outputs at frame *n* are a function of state\[n−1\] only
/// (ADR 0016 §2). Each frame the engine calls `emit`, then later `absorb`.
///
/// The engine calls `emit` and `absorb` with one frame (`frames() == 1`), also
/// in block execution (D-EXEC-3, ADR 0019 §1): `emit` gets no inputs, so it
/// cannot write a frame whose output depends on inputs of an earlier frame of
/// the same call.
///
/// Object safe: the engine holds `Box<dyn StateOutputPrimitive>`.
///
/// # Example
///
/// ```
/// use signal_core::{
///     Direction, Feedthrough, Inputs, ModuleDescriptor, Outputs, PortDescriptor,
///     PortKind, PortShape, PrepareContext, RateDivisor, StateOutputPrimitive,
/// };
///
/// /// `out[n] = in[n - 1]`, `out[0] = 0`.
/// #[derive(Default)]
/// struct UnitDelay {
///     last: f64,
/// }
///
/// const fn signal(name: &'static str, direction: Direction) -> PortDescriptor {
///     PortDescriptor {
///         name,
///         direction,
///         shape: PortShape::Signal,
///         rate: RateDivisor::ONE,
///         kind: PortKind::Generic,
///         unit: None,
///         range: None,
///     }
/// }
///
/// const DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
///     type_name: "example.unit_delay",
///     doc: "Delays its input by one frame.",
///     ports: &[signal("in", Direction::In), signal("out", Direction::Out)],
///     parameters: &[],
///     feedthrough: Feedthrough::StateOutput,
///     sink: None,
///     latency: None,
///     reproducibility: signal_core::Reproducibility::BitExact,
///     category: None,
/// };
///
/// impl StateOutputPrimitive for UnitDelay {
///     fn describe() -> ModuleDescriptor {
///         DESCRIPTOR
///     }
///
///     fn prepare(&mut self, _ctx: &PrepareContext) {
///         self.last = 0.0;
///     }
///
///     fn reset(&mut self) {
///         self.last = 0.0;
///     }
///
///     fn emit(&self, outputs: &mut Outputs<'_>) {
///         outputs.signal(0).fill(self.last);
///     }
///
///     fn absorb(&mut self, inputs: &Inputs<'_>) {
///         if let Some(&x) = inputs.signal(0).last() {
///             self.last = x;
///         }
///     }
/// }
///
/// // Two frames as the per-frame engine runs them: emit, then absorb.
/// let mut delay: Box<dyn StateOutputPrimitive> = Box::new(UnitDelay::default());
/// let mut seen = Vec::new();
/// for x in [0.5, 0.25] {
///     let mut out = [f64::NAN];
///     delay.emit(&mut Outputs::new(1, &mut out, &mut []));
///     delay.absorb(&Inputs::new(1, &[x], &[], &[], &[]));
///     seen.push(out[0]);
/// }
/// assert_eq!(seen, [0.0, 0.5]);
/// ```
pub trait StateOutputPrimitive: Send {
    /// The module type's descriptor, available without an instance. Its
    /// `feedthrough` is [`Feedthrough::StateOutput`].
    fn describe() -> ModuleDescriptor
    where
        Self: Sized;

    /// As [`DirectPrimitive::prepare`]: the only call that may allocate.
    fn prepare(&mut self, ctx: &PrepareContext);

    /// As [`DirectPrimitive::reset`]: back to the prepared state, real-time safe.
    fn reset(&mut self);

    /// Writes the outputs for the current frames from state alone: it gets no
    /// inputs and no parameter values, and cannot change state. Event outputs
    /// pushed here belong to the emit phase (ADR 0016 §3). Real-time rules apply.
    fn emit(&self, outputs: &mut Outputs<'_>);

    /// Reads the now-final inputs, events and effective parameter values for the
    /// current frames and computes the new state, which the next `emit` reads.
    /// Real-time rules apply.
    fn absorb(&mut self, inputs: &Inputs<'_>);

    /// For a type that declares a [`BlockLatency`](crate::BlockLatency) (ADR
    /// 0019 §3): whether the result the next `emit` shows has arrived. The
    /// engine asks before every `emit` of such an instance. Offline it asks
    /// again until the answer is not [`Due::Pending`], so renders stay exact;
    /// in real time it never waits, and calls [`miss`](Self::miss) instead.
    /// Never blocks; real-time rules apply. Others keep the default, `Ready`.
    fn due(&mut self) -> Due {
        Due::Ready
    }

    /// The engine gives up on the result [`due`](Self::due) did not report
    /// ready (a fault, logged as `late_result`): the module outputs silence
    /// for that block, and discards the result when it arrives. Real-time
    /// rules apply.
    fn miss(&mut self) {}

    /// The most words [`delivered_result`](Self::delivered_result) returns:
    /// asked once after `prepare`, off the real-time thread, to size the
    /// buffers that carry delivered results to a session (M2.7-4). A type
    /// that declares a [`BlockLatency`](crate::BlockLatency) must return at
    /// least 1: the engine refuses a latent instance that exposes no result,
    /// since its delivered results could not be hashed (M2.7-8). 0, the
    /// default, is for every other type.
    fn result_words(&self) -> usize {
        0
    }

    /// The due result shown in the current block, as words: asked at every
    /// due frame whose result was delivered (`due` answered `Ready`), to hash
    /// it (M2.7-8) and, for a session that records delivered data
    /// (`--record-boundary-data`), to keep it; in an exact replay also after
    /// [`inject_result`](Self::inject_result), when it is the injected
    /// result. Real-time rules apply.
    fn delivered_result(&self) -> &[f32] {
        &[]
    }

    /// Exact replay of a session with recorded data: show `words`, a result
    /// [`delivered_result`](Self::delivered_result) gave in the recorded run,
    /// as this block's due result, in place of the device's. `false` (the
    /// default) when the type cannot.
    fn inject_result(&mut self, _words: &[f32]) -> bool {
        false
    }

    /// Exact replay only, before each `absorb`: [`Due::Pending`] while the
    /// work that `absorb` may submit cannot be accepted yet because work given
    /// up on is still in flight; the engine yields and asks again.
    /// [`Due::Lost`] means it never will be (no device). Never blocks.
    fn settle(&mut self) -> Due {
        Due::Ready
    }

    /// Why the due result the last [`due`](Self::due) reported
    /// [`Due::Lost`] will never arrive, when the module knows (M2.7-5): the
    /// engine records it with the `late_result` fault. Asked right after
    /// `due`, before [`miss`](Self::miss). `None`, the default, records no
    /// cause. Real-time rules apply.
    fn lost_cause(&self) -> Option<LossCause> {
        None
    }
}

/// What [`StateOutputPrimitive::due`] reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Due {
    /// The next `emit` has what it needs: the due result, or none is due.
    Ready,
    /// The due result has not arrived yet.
    Pending,
    /// The due result will never arrive (no device, or it was never
    /// submitted): the engine calls `miss` without waiting.
    Lost,
}

/// Why a due result was lost ([`StateOutputPrimitive::lost_cause`]): the
/// `cause` of a `late_result` fault in the log and of a missed boundary in
/// a session (M2.7-5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LossCause {
    /// The work was refused as invalid.
    Validation,
    /// The device ran out of memory.
    OutOfMemory,
    /// The result could not be read back.
    Mapping,
    /// An internal error of the device.
    Internal,
    /// An error no one captured was raised while the work was in flight.
    Uncaptured,
    /// The device was shutting down.
    Stopped,
    /// The device was lost.
    DeviceLost,
}

impl LossCause {
    /// Every cause.
    pub const ALL: [LossCause; 7] = [
        LossCause::Validation,
        LossCause::OutOfMemory,
        LossCause::Mapping,
        LossCause::Internal,
        LossCause::Uncaptured,
        LossCause::Stopped,
        LossCause::DeviceLost,
    ];

    /// Its name in the log and in sessions.
    pub fn name(self) -> &'static str {
        match self {
            LossCause::Validation => "validation",
            LossCause::OutOfMemory => "out_of_memory",
            LossCause::Mapping => "mapping",
            LossCause::Internal => "internal",
            LossCause::Uncaptured => "uncaptured",
            LossCause::Stopped => "stopped",
            LossCause::DeviceLost => "device_lost",
        }
    }

    /// The cause named `name`.
    pub fn from_name(name: &str) -> Option<LossCause> {
        LossCause::ALL.into_iter().find(|c| c.name() == name)
    }
}

/// A primitive instance as the engine holds it: a boxed implementation of the
/// trait for its feedthrough class.
pub enum Primitive {
    /// A direct primitive.
    Direct(Box<dyn DirectPrimitive>),
    /// A state-output primitive.
    StateOutput(Box<dyn StateOutputPrimitive>),
}

impl Primitive {
    /// The feedthrough class of this instance.
    pub fn feedthrough(&self) -> Feedthrough {
        match self {
            Primitive::Direct(_) => Feedthrough::Direct,
            Primitive::StateOutput(_) => Feedthrough::StateOutput,
        }
    }
}

impl std::fmt::Debug for Primitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Primitive")
            .field(&self.feedthrough())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventPayload;

    fn trigger(offset: u32) -> Event {
        Event {
            offset,
            payload: EventPayload::Trigger,
        }
    }

    #[test]
    fn input_views_split_ports() {
        let signals = [1.0, 2.0, 3.0, 4.0];
        let parameters = [0.5, 0.5];
        let events = [trigger(0), trigger(1), trigger(1)];
        let ends = [1, 1, 3];
        let inputs = Inputs::new(2, &signals, &parameters, &events, &ends);
        assert_eq!(inputs.frames(), 2);
        assert_eq!(inputs.signal(0), [1.0, 2.0]);
        assert_eq!(inputs.signal(1), [3.0, 4.0]);
        assert_eq!(inputs.signal(2), [] as [f64; 0]);
        assert_eq!(inputs.parameter(0), [0.5, 0.5]);
        assert_eq!(inputs.parameter(1), [] as [f64; 0]);
        assert_eq!(inputs.events(0), [trigger(0)]);
        assert_eq!(inputs.events(1), []);
        assert_eq!(inputs.events(2), [trigger(1), trigger(1)]);
        assert_eq!(inputs.events(3), []);
    }

    /// A vector port is rows: `rows` hands several at once, element-major.
    #[test]
    fn views_hand_out_rows() {
        let signals = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let inputs = Inputs::new(2, &signals, &[], &[], &[]);
        assert_eq!(inputs.signal_rows(), 3);
        assert_eq!(inputs.rows(1, 2), [3.0, 4.0, 5.0, 6.0]);
        assert_eq!(inputs.rows(2, 2), [] as [f64; 0]);
        let mut out = [0.0; 4];
        let mut outputs = Outputs::new(2, &mut out, &mut []);
        outputs.rows(0, 2).copy_from_slice(&[7.0, 8.0, 9.0, 10.0]);
        assert!(outputs.rows(1, 2).is_empty());
        assert_eq!(out, [7.0, 8.0, 9.0, 10.0]);
    }

    #[test]
    fn output_views_split_ports() {
        let mut signals = [0.0; 4];
        let mut sinks = [EventSink::with_capacity(1)];
        let mut outputs = Outputs::new(2, &mut signals, &mut sinks);
        outputs.signal(1).copy_from_slice(&[7.0, 8.0]);
        assert!(outputs.signal(2).is_empty());
        for (port, chunk) in outputs.signals().enumerate() {
            chunk[0] = port as f64;
        }
        assert!(outputs.events(0).is_some_and(|s| s.push(trigger(0))));
        assert!(outputs.events(1).is_none());
        assert_eq!(signals, [0.0, 0.0, 1.0, 8.0]);
        assert_eq!(sinks[0].events(), [trigger(0)]);
    }

    #[test]
    fn event_sink_overflow_drops_and_counts_without_reallocating() {
        let mut sink = EventSink::with_capacity(2);
        let storage = (sink.events.as_ptr(), sink.events.capacity());
        assert!(sink.push(trigger(0)));
        assert!(sink.push(trigger(1)));
        assert!(!sink.push(trigger(2)));
        assert!(!sink.push(trigger(3)));
        assert_eq!(sink.events(), [trigger(0), trigger(1)]);
        assert_eq!(sink.overflows(), 2);
        assert_eq!((sink.events.as_ptr(), sink.events.capacity()), storage);

        sink.clear();
        assert!(sink.events().is_empty());
        assert!(sink.push(trigger(4)));
        assert_eq!(sink.overflows(), 2);
        assert_eq!(sink.capacity(), 2);
        assert_eq!((sink.events.as_ptr(), sink.events.capacity()), storage);

        let mut none = EventSink::default();
        assert!(!none.push(trigger(0)));
        assert_eq!(none.overflows(), 1);
    }

    /// A per-frame sink keeps 16 events per frame whatever the emission order,
    /// and reports the earliest frame that dropped one: 33 events at offset 1,
    /// then 17 at offset 0, overflow at frame 0, as one-frame calls would.
    #[test]
    fn per_frame_sink_overflows_at_the_earliest_frame_in_any_order() {
        let mut sink = EventSink::per_frame(16, 2);
        let storage = (sink.events.as_ptr(), sink.events.capacity());
        sink.start_call(2);
        for _ in 0..33 {
            sink.push(trigger(1));
        }
        assert_eq!(sink.first_overflow(), Some(1));
        for _ in 0..17 {
            sink.push(trigger(0));
        }
        assert_eq!(sink.first_overflow(), Some(0));
        let kept = |offset| sink.events().iter().filter(|e| e.offset == offset).count();
        assert_eq!((kept(0), kept(1)), (16, 16));
        assert_eq!(sink.overflows(), 18);
        assert_eq!((sink.events.as_ptr(), sink.events.capacity()), storage);

        // A new call starts with no frame counted and no overflow seen.
        sink.start_call(2);
        assert!(sink.events().is_empty());
        assert_eq!(sink.first_overflow(), None);
        for _ in 0..16 {
            assert!(sink.push(trigger(1)));
        }
        assert_eq!(sink.first_overflow(), None);
        assert_eq!(sink.overflows(), 18);
        assert_eq!((sink.events.as_ptr(), sink.events.capacity()), storage);
    }

    /// In a one-frame call every event counts against that frame, whatever its
    /// offset: the limit of the per-frame engine's 16-event sink.
    #[test]
    fn per_frame_sink_in_a_one_frame_call_limits_the_call() {
        let mut sink = EventSink::per_frame(16, 4);
        sink.start_call(1);
        for k in 0..16 {
            assert!(sink.push(trigger(k % 3)));
        }
        assert!(!sink.push(trigger(0)));
        assert_eq!(sink.first_overflow(), Some(0));
        assert_eq!(sink.events().len(), 16);
    }

    /// Regression (auditor-1-001): a derived clone reserved no storage, so the
    /// clone's first push allocated.
    #[test]
    fn cloned_sink_keeps_storage_reserved() {
        let template = EventSink::with_capacity(2);
        let mut sink = template.clone();
        let storage = (sink.events.as_ptr(), sink.events.capacity());
        assert!(storage.1 >= 2);
        assert!(sink.push(trigger(0)));
        assert!(sink.push(trigger(1)));
        assert_eq!((sink.events.as_ptr(), sink.events.capacity()), storage);
        assert!(!sink.push(trigger(2)));
        assert_eq!(sink.events(), [trigger(0), trigger(1)]);
        assert_eq!(sink.overflows(), 1);
        assert_eq!((sink.events.as_ptr(), sink.events.capacity()), storage);

        // A clone of a partly filled sink keeps its events and overflow count
        // and still has room for the rest of the capacity without allocating.
        let mut partial = EventSink::with_capacity(3);
        partial.push(trigger(5));
        let mut copy = partial.clone();
        let storage = (copy.events.as_ptr(), copy.events.capacity());
        assert!(copy.push(trigger(6)));
        assert!(copy.push(trigger(7)));
        assert!(!copy.push(trigger(8)));
        assert_eq!(copy.events(), [trigger(5), trigger(6), trigger(7)]);
        assert_eq!(copy.overflows(), 1);
        assert_eq!((copy.events.as_ptr(), copy.events.capacity()), storage);
    }
}
