//! Runs one module instance over a stimulus the way the engine will: buffers are
//! built outside the real-time calls, and each call is measured by the
//! counting allocator. A panic in any call of the primitive is caught and
//! recorded with the call and the frames it covered.

use std::panic::{AssertUnwindSafe, catch_unwind};

use signal_core::{
    Direction, Due, Event, EventPayload, EventSink, Inputs, InstanceAddress, ModuleType, Outputs,
    PortShape, PrepareContext, Primitive, RngStream,
};

use crate::alloc_counter::{Counts, Guard};
use crate::stimulus::Stimulus;

/// Each event sink holds this many events per frame of the largest call
/// (`max_frames`); one call pushing more overflows it.
const EVENTS_PER_FRAME: usize = 16;

/// What one render produced.
#[derive(Clone, Debug, Default)]
pub(crate) struct Rendered {
    /// One sample per frame for every signal output port.
    pub signals: Vec<Vec<f64>>,
    /// For every event output port, `(frame, payload)` in emission order.
    pub events: Vec<Vec<(u64, EventPayload)>>,
    /// The first real-time call that touched the heap: frame, call, counts.
    pub allocation: Option<(u64, &'static str, Counts)>,
    /// The first sink overflow: frame and event output index.
    pub overflow: Option<(u64, usize)>,
    /// A panic that ended the render early.
    pub panic: Option<Panic>,
}

/// A panic in a call of the primitive.
#[derive(Clone, Debug)]
pub(crate) struct Panic {
    /// The call: `prepare`, `reset`, `process`, `emit` or `absorb`.
    pub call: &'static str,
    /// The frames the call covered; `None` for `prepare` and `reset`, which
    /// happened before the render's first frame.
    pub frames: Option<(u64, u64)>,
    /// The panic message.
    pub message: String,
}

/// The first frame at which two renders differ, and how.
#[derive(Clone, Debug)]
pub(crate) struct Difference {
    pub frame: u64,
    pub what: String,
}

/// A created and prepared instance of a module type.
pub(crate) struct Instance {
    primitive: Primitive,
    max_frames: usize,
    signal_inputs: usize,
    event_inputs: usize,
    signal_outputs: usize,
    event_outputs: usize,
    /// A panic in `prepare` or `reset`; every later render reports it.
    fault: Option<Panic>,
}

impl Instance {
    /// A fresh instance, prepared at `sample_rate` for calls of at most
    /// `max_frames` frames, with the random stream of `seed`.
    pub fn new(
        module_type: &ModuleType,
        sample_rate: f64,
        max_frames: usize,
        seed: u64,
    ) -> Instance {
        let count = |direction, shape| {
            module_type
                .descriptor
                .ports
                .iter()
                .filter(|p| p.direction == direction && p.shape == shape)
                .count()
        };
        let mut instance = Instance {
            primitive: (module_type.create)(),
            max_frames,
            signal_inputs: count(Direction::In, PortShape::Signal),
            event_inputs: count(Direction::In, PortShape::Events),
            signal_outputs: count(Direction::Out, PortShape::Signal),
            event_outputs: count(Direction::Out, PortShape::Events),
            fault: None,
        };
        instance.prepare(sample_rate, max_frames, seed);
        instance
    }

    /// Calls `prepare` (again).
    pub fn prepare(&mut self, sample_rate: f64, max_frames: usize, seed: u64) {
        let address: InstanceAddress = "/contract".parse().expect("valid address");
        let ctx = PrepareContext {
            sample_rate,
            max_frames: max_frames.max(1),
            rng: RngStream::derive(seed, &address),
        };
        self.max_frames = ctx.max_frames;
        let primitive = &mut self.primitive;
        let result = catch(|| match primitive {
            Primitive::Direct(p) => p.prepare(&ctx),
            Primitive::StateOutput(p) => p.prepare(&ctx),
        });
        self.note_fault("prepare", result);
    }

    /// Calls `reset`, returning what it did with the heap.
    pub fn reset(&mut self) -> Counts {
        let primitive = &mut self.primitive;
        let guard = Guard::start();
        let result = catch(|| match primitive {
            Primitive::Direct(p) => p.reset(),
            Primitive::StateOutput(p) => p.reset(),
        });
        let counts = guard.counts();
        self.note_fault("reset", result);
        counts
    }

    /// The panic of a `prepare` or `reset` call, if one panicked.
    pub fn fault(&self) -> Option<&Panic> {
        self.fault.as_ref()
    }

    fn note_fault(&mut self, call: &'static str, result: Result<(), String>) {
        if let (Err(message), None) = (result, &self.fault) {
            self.fault = Some(Panic {
                call,
                frames: None,
                message,
            });
        }
    }

    /// Renders the first `frames` frames of `stimulus`. A direct instance gets
    /// calls of the sizes in `blocks`, repeated cyclically (the last one cut to
    /// fit). A state-output instance is run per frame (`emit`, then `absorb`),
    /// as the engine runs it; a latent one (ADR 0019 §3) is waited for before
    /// each `emit`, as offline.
    pub fn render(&mut self, stimulus: &Stimulus, frames: usize, blocks: &[usize]) -> Rendered {
        let mut rendered = Rendered {
            signals: vec![Vec::with_capacity(frames); self.signal_outputs],
            events: vec![Vec::new(); self.event_outputs],
            ..Rendered::default()
        };
        if let Some(fault) = &self.fault {
            rendered.panic = Some(fault.clone());
            return rendered;
        }
        let capacity = EVENTS_PER_FRAME * self.max_frames;
        let mut sinks = vec![EventSink::with_capacity(capacity); self.event_outputs];
        let mut buffers = Buffers::default();
        let mut cursors = vec![0; self.event_inputs];
        let mut block_sizes = blocks.iter().copied().cycle();
        let mut start = 0;
        while start < frames {
            let k = match self.primitive {
                Primitive::Direct(_) => block_sizes.next().unwrap_or(1),
                Primitive::StateOutput(_) => 1,
            }
            .clamp(1, self.max_frames)
            .min(frames - start);
            buffers.fill(stimulus, &mut cursors, start, k, self.signal_inputs);
            let inputs = Inputs::new(
                k,
                &buffers.signals,
                &buffers.parameters,
                &buffers.events,
                &buffers.event_ends,
            );
            let mut out = vec![f64::NAN; self.signal_outputs * k];
            let mut outputs = Outputs::new(k, &mut out, &mut sinks);
            let guard = Guard::start();
            let (call, result) = match &mut self.primitive {
                Primitive::Direct(p) => ("process", catch(|| p.process(&inputs, &mut outputs))),
                Primitive::StateOutput(p) => (
                    "emit",
                    catch(|| {
                        // As the offline engine: wait for a due result
                        // (ADR 0019 §3); give up only on a lost one.
                        loop {
                            match p.due() {
                                Due::Ready => break,
                                Due::Pending => std::thread::yield_now(),
                                Due::Lost => {
                                    p.miss();
                                    break;
                                }
                            }
                        }
                        p.emit(&mut outputs)
                    }),
                ),
            };
            let counts = guard.counts();
            drop(guard);
            if !rendered.note_call(start, k, call, result, counts) {
                return rendered;
            }
            rendered.collect(start, k, &out, &mut sinks);
            if rendered.overflow.is_some() {
                // The render is invalid from here on; every check reports it.
                return rendered;
            }
            if let Primitive::StateOutput(p) = &mut self.primitive {
                let guard = Guard::start();
                let result = catch(|| p.absorb(&inputs));
                let counts = guard.counts();
                drop(guard);
                if !rendered.note_call(start, k, "absorb", result, counts) {
                    return rendered;
                }
            }
            start += k;
        }
        rendered
    }
}

/// The input views' storage for one call.
#[derive(Default)]
struct Buffers {
    signals: Vec<f64>,
    parameters: Vec<f64>,
    events: Vec<Event>,
    event_ends: Vec<usize>,
}

impl Buffers {
    fn fill(
        &mut self,
        stimulus: &Stimulus,
        cursors: &mut [usize],
        start: usize,
        k: usize,
        signal_inputs: usize,
    ) {
        let end = start + k;
        self.signals.clear();
        for samples in stimulus.signals.iter().take(signal_inputs) {
            self.signals.extend_from_slice(&samples[start..end]);
        }
        self.parameters.clear();
        for values in &stimulus.parameters {
            self.parameters.extend_from_slice(&values[start..end]);
        }
        self.events.clear();
        self.event_ends.clear();
        for (events, cursor) in stimulus.events.iter().zip(cursors.iter_mut()) {
            while let Some(&(frame, payload)) = events.get(*cursor) {
                if frame >= end as u64 {
                    break;
                }
                if frame >= start as u64 {
                    self.events.push(Event {
                        offset: (frame - start as u64) as u32,
                        payload,
                    });
                }
                *cursor += 1;
            }
            self.event_ends.push(self.events.len());
        }
    }
}

/// Runs one call of the primitive, turning a panic into its message.
fn catch(call: impl FnOnce()) -> Result<(), String> {
    catch_unwind(AssertUnwindSafe(call)).map_err(|payload| {
        match (
            payload.downcast_ref::<&str>(),
            payload.downcast_ref::<String>(),
        ) {
            (Some(message), _) => (*message).to_owned(),
            (None, Some(message)) => message.clone(),
            (None, None) => "a panic without a message".to_owned(),
        }
    })
}

impl Rendered {
    /// Records the outcome of the call covering `k` frames from `start`.
    /// Returns `false` if it panicked, which ends the render; the panic's own
    /// allocations are then not counted against the call.
    fn note_call(
        &mut self,
        start: usize,
        k: usize,
        call: &'static str,
        result: Result<(), String>,
        counts: Counts,
    ) -> bool {
        if let Err(message) = result {
            self.panic = Some(Panic {
                call,
                frames: Some((start as u64, (start + k) as u64)),
                message,
            });
            return false;
        }
        if self.allocation.is_none() && !counts.is_zero() {
            self.allocation = Some((start as u64, call, counts));
        }
        true
    }

    fn collect(&mut self, start: usize, k: usize, out: &[f64], sinks: &mut [EventSink]) {
        for (samples, chunk) in self.signals.iter_mut().zip(out.chunks(k)) {
            samples.extend_from_slice(chunk);
        }
        for (port, (events, sink)) in self.events.iter_mut().zip(sinks.iter_mut()).enumerate() {
            events.extend(
                sink.events()
                    .iter()
                    .map(|e| (start as u64 + u64::from(e.offset), e.payload)),
            );
            sink.clear();
            if sink.overflows() > 0 && self.overflow.is_none() {
                self.overflow = Some((start as u64, port));
            }
        }
    }

    /// The first value (signal sample, or event value) for which `bad` holds,
    /// with its frame and a description naming the output.
    pub fn first_value(
        &self,
        names: &(Vec<&'static str>, Vec<&'static str>),
        bad: impl Fn(f64) -> bool,
    ) -> Option<Difference> {
        let mut first: Option<Difference> = None;
        let mut offer = |frame: u64, what: String| {
            if first.as_ref().is_none_or(|d| frame < d.frame) {
                first = Some(Difference { frame, what });
            }
        };
        for (port, samples) in self.signals.iter().enumerate() {
            if let Some((frame, &x)) = samples.iter().enumerate().find(|&(_, &x)| bad(x)) {
                offer(
                    frame as u64,
                    format!("signal output `{}` is {x:e}", name(&names.0, port)),
                );
            }
        }
        for (port, events) in self.events.iter().enumerate() {
            let found = events
                .iter()
                .find(|(_, payload)| value_of(payload).is_some_and(&bad));
            if let Some(&(frame, payload)) = found {
                offer(
                    frame,
                    format!(
                        "event output `{}` emitted {payload:?}",
                        name(&names.1, port)
                    ),
                );
            }
        }
        first
    }
}

/// The first frame at which `a` and `b` differ bit for bit: signal samples by
/// `f64::to_bits`, events by frame and payload (values by `to_bits`).
pub(crate) fn first_difference(
    a: &Rendered,
    b: &Rendered,
    names: &(Vec<&'static str>, Vec<&'static str>),
) -> Option<Difference> {
    let mut first: Option<Difference> = None;
    let mut offer = |frame: u64, what: String| {
        if first.as_ref().is_none_or(|d| frame < d.frame) {
            first = Some(Difference { frame, what });
        }
    };
    for (port, (x, y)) in a.signals.iter().zip(&b.signals).enumerate() {
        let differs = x
            .iter()
            .zip(y)
            .enumerate()
            .find(|(_, (p, q))| p.to_bits() != q.to_bits());
        if let Some((frame, (p, q))) = differs {
            offer(
                frame as u64,
                format!("signal output `{}`: {p:e} vs {q:e}", name(&names.0, port)),
            );
        }
    }
    for (port, (x, y)) in a.events.iter().zip(&b.events).enumerate() {
        let port = name(&names.1, port);
        let differs = x
            .iter()
            .zip(y)
            .find(|((fa, pa), (fb, pb))| fa != fb || bits(pa) != bits(pb));
        match differs {
            Some(((fa, pa), (fb, pb))) => offer(
                (*fa).min(*fb),
                format!("event output `{port}`: {pa:?} at frame {fa} vs {pb:?} at frame {fb}"),
            ),
            None => {
                let common = x.len().min(y.len());
                if let Some(&(frame, payload)) = x.get(common).or(y.get(common)) {
                    offer(
                        frame,
                        format!(
                            "event output `{port}`: {} vs {} events; the first unmatched is {payload:?} at frame {frame}",
                            x.len(),
                            y.len()
                        ),
                    );
                }
            }
        }
    }
    first
}

fn name(names: &[&'static str], index: usize) -> String {
    match names.get(index) {
        Some(name) => (*name).to_owned(),
        None => format!("#{index}"),
    }
}

fn value_of(payload: &EventPayload) -> Option<f64> {
    match *payload {
        EventPayload::Trigger => None,
        EventPayload::Set(v) => Some(v),
        EventPayload::Ramp { to, .. } => Some(to),
    }
}

/// An event payload as bits, for bit-exact comparison.
fn bits(payload: &EventPayload) -> (u8, u64, u64, Option<signal_core::Curve>) {
    match *payload {
        EventPayload::Trigger => (0, 0, 0, None),
        EventPayload::Set(v) => (1, v.to_bits(), 0, None),
        EventPayload::Ramp { to, frames, curve } => (2, to.to_bits(), frames, Some(curve)),
    }
}
