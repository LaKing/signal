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
use crate::rng::RngStream;

/// What [`prepare`](DirectPrimitive::prepare) learns about the run it is prepared for.
#[derive(Clone, Debug)]
pub struct PrepareContext {
    /// The master rate in frames per second (e.g. 96000.0).
    pub sample_rate: f64,
    /// The largest number of frames *k* (≥ 1) any later real-time call will be given.
    pub max_frames: usize,
    /// The instance's random stream (ADR 0008). Keep a copy to restart it in `reset`.
    pub rng: RngStream,
}

/// The inputs of one real-time call, covering `frames` consecutive frames.
///
/// Layout (indices count within each group, in the order of the module
/// descriptor's `ports` and `parameters`):
/// - `signals`: every signal input port, port-major: port *p* is
///   `signals[p * frames .. (p + 1) * frames]`.
/// - `parameters`: the executor-computed effective value of every parameter for
///   every frame (ADR 0016 §4), in the same layout. Modules never see bases,
///   events on parameters, or modulation separately.
/// - `events`: the events of every event input port, concatenated; port *p*'s
///   events end at `event_ends[p]` and start where port *p − 1*'s end (0 for the
///   first). Within a port they are sorted by offset, and events with the same
///   offset are in the canonical order of ADR 0016 §3.
#[derive(Clone, Copy, Debug)]
pub struct Inputs<'a> {
    frames: usize,
    signals: &'a [f64],
    parameters: &'a [f64],
    events: &'a [Event],
    event_ends: &'a [usize],
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
        }
    }

    /// The number of frames *k* ≥ 1 in this call.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// The samples of signal input `port`, one per frame.
    pub fn signal(&self, port: usize) -> &'a [f64] {
        block(self.signals, port, self.frames)
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

/// The outputs of one real-time call, covering `frames` consecutive frames.
///
/// `signals` holds every signal output port, port-major like [`Inputs`]; the
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

    /// The samples of signal output `port`, one per frame.
    pub fn signal(&mut self, port: usize) -> &mut [f64] {
        let start = port.saturating_mul(self.frames);
        let end = start.saturating_add(self.frames);
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
#[derive(Debug, Default)]
pub struct EventSink {
    events: Vec<Event>,
    capacity: usize,
    overflows: u64,
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
        }
    }
}

impl EventSink {
    /// A sink holding up to `capacity` events. Allocates: not for the real-time path.
    pub fn with_capacity(capacity: usize) -> EventSink {
        EventSink {
            events: Vec::with_capacity(capacity),
            capacity,
            overflows: 0,
        }
    }

    /// Appends `event`, or drops it and raises the overflow counter when full.
    /// Returns whether the event was kept.
    pub fn push(&mut self, event: Event) -> bool {
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
        self.events.clear();
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
///     }],
///     feedthrough: Feedthrough::Direct,
///     sink: None,
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
/// The per-frame reference engine calls `emit` and `absorb` with one frame
/// (`frames() == 1`). Block execution for state-output instances (D-EXEC-3) is
/// deferred.
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
