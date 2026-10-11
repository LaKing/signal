//! The per-frame executor (ADR 0016, *k* = 1): instantiation, the four frame
//! phases, event delivery in canonical order, parameter bases, suspension, the
//! fault guard, plan swaps and incremental input (ADR 0006). Normative
//! description: `docs/semantics.md`.
//!
//! Every buffer is sized in [`Engine::new`] or [`Engine::prepare_swap`];
//! [`Engine::run_frame`], [`Engine::commit_swap`], the `push_*` methods and
//! [`Engine::drain_log`] allocate, free, lock, perform I/O and panic nowhere
//! (`docs/realtime-rules.md`).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::mem;
use std::ops::Range as Span;
use std::sync::atomic::{AtomicU64, Ordering};

use signal_core::{
    Combine, Direction, Event, EventPayload, EventSink, Feedthrough, Frame, Inputs,
    InstanceAddress, ModuleDescriptor, Outputs, PortAddress, PortKind, PortShape, PrepareContext,
    Primitive, Range, RateDivisor, Registry, RngStream, SinkKind,
};
use signal_graph::ExecutionPlan;

use crate::error::EngineError;
use crate::fade::{FAULT_FADE_FRAMES, Fade, fade_frames, fade_in, fade_out};
use crate::log::{
    DiscardReason, FaultReason, InstanceId, LineSource, LogLine, LogRecord, LogSource, PortId,
};
use crate::parameter::{Applied, Base, effective};

/// Events an event output keeps per frame: the contract suite's 16 events per
/// frame, at *k* = 1. More is an `event_overflow` fault.
const EVENTS_PER_FRAME: usize = 16;

/// The fault guard trips on an output value whose magnitude exceeds this.
const FAULT_MAGNITUDE: f64 = 1e6;

/// The next engine's identity: a prepared swap commits only to its own engine.
static NEXT_ENGINE_ID: AtomicU64 = AtomicU64::new(0);

/// What a run is prepared for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineConfig {
    /// The master rate in frames per second (the project sample rate).
    pub sample_rate: u32,
    /// The master seed; each instance's stream is `RngStream::derive(seed, address)`.
    pub seed: u64,
    /// The capacity of each incremental input queue: pushed external events
    /// ([`Engine::push_external`]) and pushed edits ([`Engine::push_suspend`]).
    pub queue_capacity: usize,
}

impl EngineConfig {
    /// The default [`queue_capacity`](EngineConfig::queue_capacity).
    pub const DEFAULT_QUEUE_CAPACITY: usize = 4096;

    /// A configuration with the default queue capacity.
    pub fn new(sample_rate: u32, seed: u64) -> EngineConfig {
        EngineConfig {
            sample_rate,
            seed,
            queue_capacity: EngineConfig::DEFAULT_QUEUE_CAPACITY,
        }
    }
}

/// An event from outside the graph (ADR 0016 §3): from the session log, the CLI
/// or a test. Same-frame external events are delivered in `sequence` order,
/// before any module-emitted event.
#[derive(Clone, Debug, PartialEq)]
pub struct ExternalEvent {
    /// The frame it is stamped for.
    pub frame: Frame,
    /// Its log sequence number, unique among the external events of a run.
    pub sequence: u64,
    /// An event input port or a parameter of a primitive instance.
    pub target: PortAddress,
    /// The event.
    pub payload: EventPayload,
}

/// [`Engine::push_external`] found its queue full; the event is handed back.
#[derive(Clone, Debug, PartialEq)]
pub struct ExternalFull(pub ExternalEvent);

/// [`Engine::push_suspend`] or [`Engine::push_resume`] found the edit queue
/// full; the address is handed back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditFull(pub InstanceAddress);

impl fmt::Display for ExternalFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "external event queue full (sequence {})",
            self.0.sequence
        )
    }
}

impl std::error::Error for ExternalFull {}

impl fmt::Display for EditFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "edit queue full ({})", self.0)
    }
}

impl std::error::Error for EditFull {}

/// Inputs that reached the engine and had no effect, and so are not in the log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputDiscards {
    /// External events whose target, or edits whose instance, is not in the
    /// plan running at their effective frame (or takes no events).
    pub unrouted: u64,
    /// External events with a non-finite `set` or `ramp` value, and edits that
    /// do not fit the instance's state (a suspend of a suspended instance, a
    /// resume of a running one, a second edit of an instance in one frame).
    pub invalid: u64,
}

/// A parameter's base and effective value after the last frame its instance ran.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParameterValue {
    /// The base: set by `set` and `ramp` events, never clamped (ADR 0016 §4a).
    pub base: f64,
    /// The value the module received: base combined with modulation, clamped.
    pub effective: f64,
}

/// What a port is, with its index in the engine's buffer for that group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    SignalIn(usize),
    EventIn(usize),
    SignalOut(usize),
    EventOut(usize),
    Param(usize),
}

/// How an instance takes part in a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Running,
    Suspended,
    Muted,
}

/// An instance: its feedthrough class, its buffer ranges and its state.
#[derive(Debug)]
struct Slot {
    feedthrough: Feedthrough,
    audio_sink: bool,
    signal_in: Span<usize>,
    event_in: Span<usize>,
    signal_out: Span<usize>,
    event_out: Span<usize>,
    params: Span<usize>,
    suspended: bool,
    /// Muted by the fault guard; the frames since, the fault fade's position.
    fault: Option<u64>,
    fade: Fade,
    discards: u64,
    suspension_discards: u64,
    /// The frame of the last edit applied: at most one edit per frame.
    edited_at: Option<Frame>,
}

impl Slot {
    fn mode(&self) -> Mode {
        if self.fault.is_some() {
            Mode::Muted
        } else if self.suspended {
            Mode::Suspended
        } else {
            Mode::Running
        }
    }
}

/// An event source port of an input, with its number of connections to that
/// input: duplicate connections each deliver every event.
#[derive(Clone, Copy, Debug)]
struct EventSource {
    sink: usize,
    port: PortId,
    copies: usize,
}

/// Adds a connection from `port` (sink `sink`) to `sources`, which are in
/// canonical order. A duplicate connection adds a copy to the existing entry,
/// so its events stay in the source's emission order (ADR 0016 §3).
fn add_event_source(sources: &mut Vec<EventSource>, sink: usize, port: PortId) {
    match sources.iter_mut().find(|s| s.sink == sink) {
        Some(source) => source.copies += 1,
        None => sources.push(EventSource {
            sink,
            port,
            copies: 1,
        }),
    }
}

/// The number of connections behind `sources`, duplicates included.
fn connections(sources: &[EventSource]) -> usize {
    sources.iter().map(|s| s.copies).sum()
}

/// An event input port and its event sources in canonical order.
#[derive(Debug)]
struct EventInput {
    port: PortId,
    sources: Vec<EventSource>,
}

/// A parameter: its base, how it combines, and its sources in canonical order.
#[derive(Debug)]
struct Param {
    port: PortId,
    base: Base,
    combine: Combine,
    range: Range,
    /// Signal sources: (output, scale).
    modulation: Vec<(usize, f64)>,
    /// Modulation held from sources removed by a plan swap, used while the
    /// parameter has no signal sources (`docs/semantics.md` §12).
    held_modulation: Option<f64>,
    /// Event sources, in canonical order.
    events: Vec<EventSource>,
}

/// An external event of the current frame, resolved to its target port.
#[derive(Clone, Copy, Debug)]
struct Staged {
    target: usize,
    sequence: u64,
    /// Up-front events sort before pushed ones of the same target and sequence.
    pushed: bool,
    ordinal: u64,
    payload: EventPayload,
}

/// A pushed external event and its effective frame.
#[derive(Debug)]
struct Queued {
    at: Frame,
    ordinal: u64,
    event: ExternalEvent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditKind {
    Suspend,
    Resume,
}

/// An edit scheduled with [`Engine::suspend`] or [`Engine::resume`].
#[derive(Debug)]
struct ScheduledEdit {
    frame: Frame,
    kind: EditKind,
    instance: InstanceAddress,
}

/// An edit pushed with [`Engine::push_suspend`] or [`Engine::push_resume`],
/// with its effective frame.
#[derive(Debug)]
struct PushedEdit {
    at: Frame,
    kind: EditKind,
    instance: InstanceAddress,
}

/// Everything that belongs to one instantiated plan: instances, ports, buffers
/// and the frame log, whose records name this plan's ports and instances.
///
/// The real-time [`Controller`](crate::Controller) keeps one without
/// primitives as a structural mirror of the running plan: what a swap is
/// prepared against and what log records are read with.
#[derive(Debug)]
pub(crate) struct PlanState {
    instances: Vec<InstanceAddress>,
    instance_ids: BTreeMap<InstanceAddress, usize>,
    /// Each instance's type and descriptor: what decides whether it survives a swap.
    types: Vec<(&'static str, ModuleDescriptor)>,
    /// Every port, in address order: a [`PortId`] indexes it.
    ports: Vec<PortAddress>,
    roles: Vec<Option<(usize, Role)>>,
    slots: Vec<Slot>,
    /// `None` only for a survivor of a prepared swap, until it is committed.
    primitives: Vec<Option<Primitive>>,
    direct_order: Vec<usize>,
    state_outputs: Vec<usize>,
    last_edit: Vec<Option<(EditKind, Frame)>>,

    signal_in: Vec<f64>,
    signal_sources: Vec<Vec<(usize, f64)>>,
    /// What readers see of every signal output.
    outputs: Vec<f64>,
    /// Where modules write; committed to `outputs` after the fault check.
    scratch: Vec<f64>,
    /// Values fades start from.
    held: Vec<f64>,
    output_audio: Vec<bool>,
    output_ports: Vec<PortId>,

    sinks: Vec<EventSink>,
    sink_ports: Vec<PortId>,
    sink_overflows: Vec<u64>,
    event_inputs: Vec<EventInput>,
    instance_events: Vec<Vec<Event>>,
    instance_event_ends: Vec<Vec<usize>>,

    params: Vec<Param>,
    /// Effective values, laid out for [`Inputs`].
    param_values: Vec<f64>,

    log: Vec<LogRecord>,
    /// Scratch for [`Engine::drain_log`]'s canonical order, as large as `log`.
    log_order: Vec<usize>,
}

impl PlanState {
    /// Instantiates `plan`. With `old` (a swap), an instance of `old` with the
    /// same address, type and descriptor survives: it is not created, and is
    /// returned as a (new, old) pair; every other instance fades in.
    fn build(
        plan: &ExecutionPlan,
        registry: &Registry,
        config: EngineConfig,
        upfront: &[ExternalEvent],
        old: Option<&PlanState>,
    ) -> Result<(PlanState, Vec<(usize, usize)>), EngineError> {
        // Every port of every instance, in address order.
        let mut all_ports = BTreeSet::new();
        for instance in &plan.instances {
            let descriptor = instance.descriptor;
            let names = descriptor.ports.iter().map(|p| p.name);
            for name in names.chain(descriptor.parameters.iter().map(|p| p.name)) {
                all_ports.insert(port_address(&instance.address, name)?);
            }
        }
        let ports: Vec<PortAddress> = all_ports.into_iter().collect();
        let mut roles: Vec<Option<(usize, Role)>> = vec![None; ports.len()];

        let mut instances = Vec::with_capacity(plan.instances.len());
        let mut instance_ids = BTreeMap::new();
        let mut types = Vec::with_capacity(plan.instances.len());
        let mut slots = Vec::with_capacity(plan.instances.len());
        let mut primitives = Vec::with_capacity(plan.instances.len());
        let mut survivors = Vec::new();
        let mut signal_inputs = 0;
        let mut event_inputs = Vec::new();
        let mut output_audio = Vec::new();
        let mut output_ports = Vec::new();
        let mut sinks = Vec::new();
        let mut sink_ports = Vec::new();
        let mut params = Vec::new();
        let mut param_values = Vec::new();

        for (index, instance) in plan.instances.iter().enumerate() {
            let address = &instance.address;
            let descriptor = instance.descriptor;
            if instance_ids.insert(address.clone(), index).is_some() {
                return Err(EngineError::InvalidSchedule);
            }
            let module_type =
                registry
                    .get(instance.type_name)
                    .ok_or_else(|| EngineError::UnknownModuleType {
                        instance: address.clone(),
                        type_name: instance.type_name,
                    })?;
            if module_type.descriptor != descriptor {
                return Err(EngineError::DescriptorMismatch {
                    instance: address.clone(),
                });
            }
            let kind = (instance.type_name, descriptor);
            let survivor = old.and_then(|old| {
                let &o = old.instance_ids.get(address)?;
                (old.types.get(o) == Some(&kind)).then_some(o)
            });
            let primitive = match survivor {
                // Its primitive moves in at commit; the old one passed the
                // feedthrough check with this same descriptor.
                Some(o) => {
                    survivors.push((index, o));
                    None
                }
                None => {
                    let mut primitive = (module_type.create)();
                    if primitive.feedthrough() != descriptor.feedthrough {
                        return Err(EngineError::FeedthroughMismatch {
                            instance: address.clone(),
                        });
                    }
                    let context = PrepareContext {
                        sample_rate: f64::from(config.sample_rate),
                        max_frames: 1,
                        rng: RngStream::derive(config.seed, address),
                    };
                    match &mut primitive {
                        Primitive::Direct(module) => module.prepare(&context),
                        Primitive::StateOutput(module) => module.prepare(&context),
                    }
                    Some(primitive)
                }
            };

            let starts = (
                signal_inputs,
                event_inputs.len(),
                output_audio.len(),
                sinks.len(),
                params.len(),
            );
            for p in descriptor.ports {
                let port = port_address(address, p.name)?;
                if p.rate != RateDivisor::ONE {
                    return Err(EngineError::UnsupportedRate { port });
                }
                let id = ports.binary_search(&port).unwrap_or(usize::MAX);
                let role = match (p.direction, p.shape) {
                    (Direction::In, PortShape::Signal) => {
                        signal_inputs += 1;
                        Role::SignalIn(signal_inputs - 1)
                    }
                    (Direction::In, PortShape::Events) => {
                        event_inputs.push(EventInput {
                            port: PortId(id),
                            sources: Vec::new(),
                        });
                        Role::EventIn(event_inputs.len() - 1)
                    }
                    (Direction::Out, PortShape::Signal) => {
                        output_audio.push(p.kind == PortKind::Audio);
                        output_ports.push(PortId(id));
                        Role::SignalOut(output_audio.len() - 1)
                    }
                    (Direction::Out, PortShape::Events) => {
                        sinks.push(EventSink::with_capacity(EVENTS_PER_FRAME));
                        sink_ports.push(PortId(id));
                        Role::EventOut(sinks.len() - 1)
                    }
                    _ => return Err(EngineError::UnsupportedShape { port }),
                };
                if let Some(slot) = roles.get_mut(id) {
                    *slot = Some((index, role));
                }
            }
            for p in descriptor.parameters {
                let port = port_address(address, p.name)?;
                if p.rate != RateDivisor::ONE {
                    return Err(EngineError::UnsupportedRate { port });
                }
                let id = ports.binary_search(&port).unwrap_or(usize::MAX);
                let initial = instance
                    .parameters
                    .get(p.name)
                    .copied()
                    .unwrap_or(p.default);
                params.push(Param {
                    port: PortId(id),
                    base: Base::new(initial),
                    combine: p.combine,
                    range: p.range,
                    modulation: Vec::new(),
                    held_modulation: None,
                    events: Vec::new(),
                });
                param_values.push(effective(initial, None, p.combine, p.range));
                if let Some(slot) = roles.get_mut(id) {
                    *slot = Some((index, Role::Param(params.len() - 1)));
                }
            }

            slots.push(Slot {
                feedthrough: descriptor.feedthrough,
                audio_sink: descriptor.sink == Some(SinkKind::Audio),
                signal_in: starts.0..signal_inputs,
                event_in: starts.1..event_inputs.len(),
                signal_out: starts.2..output_audio.len(),
                event_out: starts.3..sinks.len(),
                params: starts.4..params.len(),
                suspended: false,
                fault: None,
                // An instance added by a swap fades in like a resume (§12).
                fade: if old.is_some() && survivor.is_none() {
                    Fade::In(0)
                } else {
                    Fade::None
                },
                discards: 0,
                suspension_discards: 0,
                edited_at: None,
            });
            primitives.push(primitive);
            types.push(kind);
            instances.push(address.clone());
        }

        // Sources, in the plan's canonical order (ADR 0016 §3, §4).
        let role_of = |port: &PortAddress| {
            let id = ports.binary_search(port).ok()?;
            Some((id, roles.get(id).copied().flatten()?))
        };
        let mut signal_sources: Vec<Vec<(usize, f64)>> = vec![Vec::new(); signal_inputs];
        let mut edges = Vec::new();
        for (target, sources) in &plan.sources {
            for source in sources {
                let invalid = || EngineError::InvalidConnection {
                    from: source.from.clone(),
                    to: target.clone(),
                };
                let (Some((_, (to, target_role))), Some((from_id, (from, source_role)))) =
                    (role_of(target), role_of(&source.from))
                else {
                    return Err(invalid());
                };
                match (target_role, source_role, source.shape) {
                    (Role::SignalIn(k), Role::SignalOut(o), PortShape::Signal) => signal_sources
                        .get_mut(k)
                        .ok_or_else(invalid)?
                        .push((o, source.scale)),
                    (Role::EventIn(k), Role::EventOut(s), PortShape::Events) => add_event_source(
                        &mut event_inputs.get_mut(k).ok_or_else(invalid)?.sources,
                        s,
                        PortId(from_id),
                    ),
                    (Role::Param(j), Role::SignalOut(o), PortShape::Signal) => params
                        .get_mut(j)
                        .ok_or_else(invalid)?
                        .modulation
                        .push((o, source.scale)),
                    (Role::Param(j), Role::EventOut(s), PortShape::Events) => add_event_source(
                        &mut params.get_mut(j).ok_or_else(invalid)?.events,
                        s,
                        PortId(from_id),
                    ),
                    _ => return Err(invalid()),
                }
                edges.push((from, to));
            }
        }

        // The schedule: every instance once, each direct source before its
        // receivers (a state-output source's output is known before the frame).
        if plan.schedule.len() != plan.instances.len() {
            return Err(EngineError::InvalidSchedule);
        }
        let mut position = vec![usize::MAX; plan.instances.len()];
        for (at, address) in plan.schedule.iter().enumerate() {
            let slot = instance_ids
                .get(address)
                .and_then(|&i| position.get_mut(i))
                .ok_or(EngineError::InvalidSchedule)?;
            if *slot != usize::MAX {
                return Err(EngineError::InvalidSchedule);
            }
            *slot = at;
        }
        for (from, to) in edges {
            let direct = slots
                .get(from)
                .is_some_and(|s| s.feedthrough == Feedthrough::Direct);
            if direct && position.get(from) >= position.get(to) {
                return Err(EngineError::InvalidSchedule);
            }
        }
        let direct_order = plan
            .schedule
            .iter()
            .filter_map(|address| instance_ids.get(address).copied())
            .filter(|&i| {
                slots
                    .get(i)
                    .is_some_and(|s| s.feedthrough == Feedthrough::Direct)
            })
            .collect();
        let state_outputs = (0..slots.len())
            .filter(|&i| {
                slots
                    .get(i)
                    .is_some_and(|s| s.feedthrough == Feedthrough::StateOutput)
            })
            .collect();

        // Buffer bounds. X(t): the most up-front externals stamped for one frame
        // and target of this plan; X_tot: the most for one frame.
        let mut stamped: Vec<(Frame, usize)> = upfront
            .iter()
            .filter_map(|e| Some((e.frame, ports.binary_search(&e.target).ok()?)))
            .collect();
        stamped.sort_unstable();
        let mut most_per_target = vec![0usize; ports.len()];
        let mut most_per_frame = 0usize;
        let (mut run_target, mut run_frame) = (0usize, 0usize);
        let mut previous: Option<(Frame, usize)> = None;
        for &(frame, target) in &stamped {
            let same_frame = previous.is_some_and(|p| p.0 == frame);
            let same_target = same_frame && previous.is_some_and(|p| p.1 == target);
            run_frame = if same_frame { run_frame + 1 } else { 1 };
            run_target = if same_target { run_target + 1 } else { 1 };
            if let Some(most) = most_per_target.get_mut(target) {
                *most = (*most).max(run_target);
            }
            most_per_frame = most_per_frame.max(run_frame);
            previous = Some((frame, target));
        }
        // Per instance: Σ over its event inputs t of X(t) + 16 · (t's event
        // connections, duplicates included), plus the push queue's capacity Q
        // when it has event inputs: all its pushed events of one frame come
        // from that queue. A sink keeps at most 16 events per frame, and each
        // connection delivers each once, so a frame never exceeds it.
        let mut instance_events = Vec::with_capacity(slots.len());
        let mut instance_event_ends = Vec::with_capacity(slots.len());
        for slot in &slots {
            let inputs = event_inputs.get(slot.event_in.clone()).unwrap_or(&[]);
            let pushed = if inputs.is_empty() {
                0
            } else {
                config.queue_capacity
            };
            let capacity: usize = inputs
                .iter()
                .map(|input| {
                    most_per_target.get(input.port.0).copied().unwrap_or(0)
                        + EVENTS_PER_FRAME * connections(&input.sources)
                })
                .sum::<usize>()
                + pushed;
            instance_events.push(Vec::with_capacity(capacity));
            instance_event_ends.push(vec![0; slot.event_in.len()]);
        }
        // The log: one swap record, at most one edit and one fault per
        // instance, every staged external (up-front and pushed), and 16 events
        // per event connection, in one frame.
        let event_connections: usize = event_inputs
            .iter()
            .map(|i| connections(&i.sources))
            .sum::<usize>()
            + params.iter().map(|p| connections(&p.events)).sum::<usize>();
        let log_capacity = 1
            + 2 * slots.len()
            + most_per_frame
            + config.queue_capacity
            + EVENTS_PER_FRAME * event_connections;
        let log = Vec::with_capacity(log_capacity);
        let log_order = Vec::with_capacity(log.capacity());

        let outputs = output_audio.len();
        let state = PlanState {
            last_edit: vec![None; instances.len()],
            instances,
            instance_ids,
            types,
            ports,
            roles,
            slots,
            primitives,
            direct_order,
            state_outputs,
            signal_in: vec![0.0; signal_inputs],
            signal_sources,
            outputs: vec![0.0; outputs],
            scratch: vec![0.0; outputs],
            held: vec![0.0; outputs],
            output_audio,
            output_ports,
            sink_overflows: vec![0; sinks.len()],
            sinks,
            sink_ports,
            event_inputs,
            instance_events,
            instance_event_ends,
            params,
            param_values,
            log,
            log_order,
        };
        Ok((state, survivors))
    }

    /// The structural mirror of `plan`: built like the engine's own state,
    /// with its primitives dropped (control-thread code; it allocates).
    pub(crate) fn mirror(
        plan: &ExecutionPlan,
        registry: &Registry,
        config: EngineConfig,
    ) -> Result<PlanState, EngineError> {
        let (mut state, _) = PlanState::build(plan, registry, config, &[], None)?;
        state.primitives.clear();
        Ok(state)
    }

    /// The audio sink: its address and number of signal inputs, if the plan
    /// has exactly one.
    pub(crate) fn audio_sink(&self) -> Option<(&InstanceAddress, usize)> {
        let mut sinks = self.slots.iter().enumerate().filter(|(_, s)| s.audio_sink);
        let (i, slot) = sinks.next()?;
        if sinks.next().is_some() {
            return None;
        }
        Some((self.instances.get(i)?, slot.signal_in.len()))
    }

    /// `record`, written by this plan, with addresses.
    pub(crate) fn line(&self, record: &LogRecord) -> Option<LogLine<'_>> {
        let source = |source| match source {
            LogSource::External { sequence } => Some(LineSource::External { sequence }),
            LogSource::Port(id) => Some(LineSource::Port(self.ports.get(id.0)?)),
        };
        Some(match *record {
            LogRecord::Swap {
                frame,
                version,
                plan_version,
            } => LogLine::Swap {
                frame,
                version,
                plan_version,
            },
            LogRecord::Event {
                frame,
                target,
                payload,
                source: from,
            } => LogLine::Event {
                frame,
                target: self.ports.get(target.0)?,
                payload,
                source: source(from)?,
            },
            LogRecord::Discard {
                frame,
                target,
                payload,
                source: from,
                reason,
            } => LogLine::Discard {
                frame,
                target: self.ports.get(target.0)?,
                payload,
                source: source(from)?,
                reason,
            },
            LogRecord::Suspend { frame, instance } => LogLine::Suspend {
                frame,
                address: self.instances.get(instance.0)?,
            },
            LogRecord::Resume {
                frame,
                instance,
                discarded,
            } => LogLine::Resume {
                frame,
                address: self.instances.get(instance.0)?,
                discarded,
            },
            LogRecord::Fault {
                frame,
                instance,
                port,
                reason,
            } => LogLine::Fault {
                frame,
                address: self.instances.get(instance.0)?,
                port: self.ports.get(port.0)?,
                reason,
            },
        })
    }

    /// The id and role of `port`.
    fn role_of(&self, port: &PortAddress) -> Option<(usize, Role)> {
        let id = self.ports.binary_search(port).ok()?;
        Some((id, self.roles.get(id).copied().flatten()?.1))
    }

    /// Applies one edit at `frame` (ADR 0016 §1a). A suspend holds every output
    /// at its frame *s* − 1 value and starts the fade-out of `audio` outputs; a
    /// resume starts their fade-in from 0. Neither affects a muted instance's
    /// outputs. An edit that does not fit the instance's state is discarded.
    fn apply_edit(
        &mut self,
        discards: &mut InputDiscards,
        log_overflows: &mut u64,
        address: &InstanceAddress,
        kind: EditKind,
        frame: Frame,
        pushed: bool,
    ) {
        let Some(&i) = self.instance_ids.get(address) else {
            discards.unrouted = discards.unrouted.saturating_add(1);
            return;
        };
        let Some(slot) = self.slots.get_mut(i) else {
            return;
        };
        let fits = match kind {
            EditKind::Suspend => !slot.suspended,
            EditKind::Resume => slot.suspended,
        };
        if !fits || slot.edited_at == Some(frame) {
            discards.invalid = discards.invalid.saturating_add(1);
            return;
        }
        slot.edited_at = Some(frame);
        let instance = InstanceId(i);
        let record = match kind {
            EditKind::Suspend => {
                slot.suspended = true;
                slot.suspension_discards = 0;
                if slot.fault.is_none() {
                    for o in slot.signal_out.clone() {
                        if let (Some(held), Some(&out)) =
                            (self.held.get_mut(o), self.outputs.get(o))
                        {
                            *held = out;
                        }
                    }
                    slot.fade = Fade::Out(0);
                }
                LogRecord::Suspend { frame, instance }
            }
            EditKind::Resume => {
                slot.suspended = false;
                if slot.fault.is_none() {
                    slot.fade = Fade::In(0);
                }
                LogRecord::Resume {
                    frame,
                    instance,
                    discarded: slot.suspension_discards,
                }
            }
        };
        // What `suspend`/`resume` check against; a later scheduled edit stays.
        if pushed
            && let Some(last) = self.last_edit.get_mut(i)
            && last.is_none_or(|(_, previous)| previous <= frame)
        {
            *last = Some((kind, frame));
        }
        push_record(&mut self.log, log_overflows, record);
    }
}

/// The state a survivor carries from `old` instance `o` into `new` instance
/// `n`: its primitive (moved), suspension, fade and fault state, outputs as
/// readers see them, held values, event sinks and parameter bases with their
/// ramps. Both have the same descriptor, so their ranges have equal lengths.
fn move_instance(new: &mut PlanState, old: &mut PlanState, n: usize, o: usize) {
    if let (Some(a), Some(b)) = (new.primitives.get_mut(n), old.primitives.get_mut(o)) {
        mem::swap(a, b);
    }
    if let (Some(a), Some(&b)) = (new.last_edit.get_mut(n), old.last_edit.get(o)) {
        *a = b;
    }
    let (Some(ns), Some(os)) = (new.slots.get_mut(n), old.slots.get(o)) else {
        return;
    };
    ns.suspended = os.suspended;
    ns.fault = os.fault;
    ns.fade = os.fade;
    ns.discards = os.discards;
    ns.suspension_discards = os.suspension_discards;
    ns.edited_at = os.edited_at;
    let outputs = ns.signal_out.clone().zip(os.signal_out.clone());
    let events = ns.event_out.clone().zip(os.event_out.clone());
    let params = ns.params.clone().zip(os.params.clone());
    for (a, b) in outputs {
        copy(&mut new.outputs, &old.outputs, a, b);
        copy(&mut new.held, &old.held, a, b);
        copy(&mut new.scratch, &old.scratch, a, b);
    }
    for (a, b) in events {
        if let (Some(x), Some(y)) = (new.sinks.get_mut(a), old.sinks.get_mut(b)) {
            mem::swap(x, y);
        }
        copy(&mut new.sink_overflows, &old.sink_overflows, a, b);
    }
    for (a, b) in params {
        if let (Some(x), Some(y)) = (new.params.get_mut(a), old.params.get(b)) {
            x.base = y.base;
        }
        copy(&mut new.param_values, &old.param_values, a, b);
    }
}

fn copy<T: Copy>(to: &mut [T], from: &[T], a: usize, b: usize) {
    if let (Some(x), Some(&y)) = (to.get_mut(a), from.get(b)) {
        *x = y;
    }
}

/// A surviving parameter left without signal sources by a swap whose held
/// modulation is decided at commit (`docs/semantics.md` §12).
#[derive(Debug)]
struct Hold {
    /// The parameter in the new plan.
    param: usize,
    /// The parameter in the old plan.
    old_param: usize,
    /// Its old sources whose instance is absent from the new plan, as a range
    /// of [`PreparedSwap::removed_sources`]. Empty: the old parameter had no
    /// sources either, and any held modulation carries over.
    removed: Span<usize>,
}

/// A plan instantiated by [`Engine::prepare_swap`], ready to be committed with
/// [`Engine::commit_swap`]. After the commit it holds the replaced plan, to be
/// dropped off the real-time thread.
#[derive(Debug)]
pub struct PreparedSwap {
    plan: PlanState,
    /// (new instance, old instance) for every survivor.
    survivors: Vec<(usize, usize)>,
    holds: Vec<Hold>,
    /// (old output, scale) of removed modulation sources, in canonical order.
    removed_sources: Vec<(usize, f64)>,
    /// The identity of the engine that prepared it.
    engine: u64,
    from_version: u64,
    plan_version: u64,
    committed: bool,
}

impl PreparedSwap {
    /// The [`ExecutionPlan::version`] of the plan it swaps in.
    pub fn plan_version(&self) -> u64 {
        self.plan_version
    }

    /// Whether it has been committed: it then holds the replaced plan.
    pub fn is_committed(&self) -> bool {
        self.committed
    }
}

/// Who a prepared swap is for: the engine's identity, the version it must be
/// committed at, and the configuration its plan is built with.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SwapTarget {
    pub(crate) engine: u64,
    pub(crate) version: u64,
    pub(crate) config: EngineConfig,
}

/// [`Engine::prepare_swap`] against `old`, the running plan or its
/// structural mirror: survivors and held modulation depend only on the
/// structure of both plans.
pub(crate) fn prepare_swap(
    old: &PlanState,
    target: SwapTarget,
    upfront: &[ExternalEvent],
    plan: &ExecutionPlan,
    registry: &Registry,
) -> Result<PreparedSwap, EngineError> {
    let (state, survivors) = PlanState::build(plan, registry, target.config, upfront, Some(old))?;

    // Which old instances survive, and the old instance of every output.
    let mut survives = vec![false; old.slots.len()];
    for &(_, o) in &survivors {
        if let Some(s) = survives.get_mut(o) {
            *s = true;
        }
    }
    let mut owner = vec![usize::MAX; old.outputs.len()];
    for (i, slot) in old.slots.iter().enumerate() {
        for o in slot.signal_out.clone() {
            if let Some(w) = owner.get_mut(o) {
                *w = i;
            }
        }
    }
    // A surviving parameter without signal sources in the new plan holds
    // the modulation of its removed sources (absent from the new plan), or
    // carries an existing hold. Lost sources that survive are a plain
    // disconnect: no hold (§5, §12).
    let mut holds = Vec::new();
    let mut removed_sources = Vec::new();
    for &(n, o) in &survivors {
        let (Some(ns), Some(os)) = (state.slots.get(n), old.slots.get(o)) else {
            continue;
        };
        for (nj, oj) in ns.params.clone().zip(os.params.clone()) {
            let (Some(np), Some(op)) = (state.params.get(nj), old.params.get(oj)) else {
                continue;
            };
            if !np.modulation.is_empty() {
                continue;
            }
            let start = removed_sources.len();
            for &(out, scale) in &op.modulation {
                let removed = owner
                    .get(out)
                    .and_then(|&w| survives.get(w))
                    .is_some_and(|&s| !s);
                if removed {
                    removed_sources.push((out, scale));
                }
            }
            let removed = start..removed_sources.len();
            if !removed.is_empty() || op.modulation.is_empty() {
                holds.push(Hold {
                    param: nj,
                    old_param: oj,
                    removed,
                });
            }
        }
    }

    Ok(PreparedSwap {
        plan: state,
        survivors,
        holds,
        removed_sources,
        engine: target.engine,
        from_version: target.version,
        plan_version: plan.version,
        committed: false,
    })
}

/// An instantiated [`ExecutionPlan`], run one frame at a time (ADR 0016).
///
/// Instances, ports and parameters live in flat buffers sized at construction.
/// Each frame runs external input (a plan swap, edits, then external events),
/// emit, process in schedule order and absorb; see [`Engine::run_frame`].
#[derive(Debug)]
pub struct Engine {
    config: EngineConfig,
    fade_len: u64,
    next_frame: Frame,
    /// 0 for the plan of [`Engine::new`], plus one per committed swap.
    version: u64,
    /// Unique in the process, set in [`Engine::new`].
    id: u64,
    plan: PlanState,

    /// The constructor's external list, sorted by (frame, sequence).
    upfront: Vec<ExternalEvent>,
    upfront_next: usize,
    /// Pushed external events in effective-frame order; the first
    /// `queue_consumed` were staged and wait for [`Engine::reclaim_externals`].
    queue: VecDeque<Queued>,
    queue_consumed: usize,
    last_pushed_at: Frame,
    push_ordinal: u64,
    /// The current frame's external events, sorted by (target, sequence).
    staged: Vec<Staged>,

    scheduled: Vec<ScheduledEdit>,
    scheduled_next: usize,
    pushed_edits: VecDeque<PushedEdit>,
    edits_consumed: usize,
    last_pushed_edit_at: Frame,

    /// (version, plan version) of a swap committed for the next frame.
    swap_record: Option<(u64, u64)>,
    discards: InputDiscards,
    log_overflows: u64,
    input_overflows: u64,
}

impl Engine {
    /// Instantiates `plan`: creates every primitive through `registry`, prepares
    /// it with the sample rate, *k* = 1 and `RngStream::derive(seed, address)`,
    /// and allocates every buffer the frames need.
    ///
    /// `external` is the caller's frame-stamped event list, a convenience beside
    /// [`push_external`](Engine::push_external). The plan's schedule is checked,
    /// not trusted: any order that runs each direct source before its receivers
    /// is accepted (ADR 0016 §1), so a test may supply another one.
    pub fn new(
        plan: &ExecutionPlan,
        registry: &Registry,
        config: EngineConfig,
        external: Vec<ExternalEvent>,
    ) -> Result<Engine, EngineError> {
        if config.sample_rate == 0 {
            return Err(EngineError::InvalidSampleRate);
        }
        let (state, _) = PlanState::build(plan, registry, config, &external, None)?;

        // External events: checked against the plan, unique sequence numbers.
        let mut sequences = BTreeSet::new();
        for event in &external {
            let Some((_, role)) = state.role_of(&event.target) else {
                return Err(EngineError::UnknownTarget {
                    target: event.target.clone(),
                });
            };
            if !matches!(role, Role::EventIn(_) | Role::Param(_)) {
                return Err(EngineError::NotAnEventTarget {
                    target: event.target.clone(),
                });
            }
            if !finite(event.payload) {
                return Err(EngineError::NonFiniteValue {
                    sequence: event.sequence,
                });
            }
            if !sequences.insert(event.sequence) {
                return Err(EngineError::DuplicateSequence {
                    sequence: event.sequence,
                });
            }
        }
        let mut upfront = external;
        upfront.sort_by_key(|e| (e.frame, e.sequence));
        let mut most_per_frame = 0usize;
        let mut run = 0usize;
        for (k, event) in upfront.iter().enumerate() {
            let same = k > 0 && upfront.get(k - 1).is_some_and(|p| p.frame == event.frame);
            run = if same { run + 1 } else { 1 };
            most_per_frame = most_per_frame.max(run);
        }

        Ok(Engine {
            config,
            fade_len: fade_frames(config.sample_rate),
            next_frame: Frame::ZERO,
            version: 0,
            id: NEXT_ENGINE_ID.fetch_add(1, Ordering::Relaxed),
            plan: state,
            upfront,
            upfront_next: 0,
            queue: VecDeque::with_capacity(config.queue_capacity),
            queue_consumed: 0,
            last_pushed_at: Frame::ZERO,
            push_ordinal: 0,
            staged: Vec::with_capacity(most_per_frame + config.queue_capacity),
            scheduled: Vec::new(),
            scheduled_next: 0,
            pushed_edits: VecDeque::with_capacity(config.queue_capacity),
            edits_consumed: 0,
            last_pushed_edit_at: Frame::ZERO,
            swap_record: None,
            discards: InputDiscards::default(),
            log_overflows: 0,
            input_overflows: 0,
        })
    }

    /// Suspends `instance` from `frame` on (ADR 0016 §1a): its time stops until a
    /// [`resume`](Engine::resume). Call it before the frame runs; it may allocate.
    pub fn suspend(&mut self, instance: &InstanceAddress, frame: Frame) -> Result<(), EngineError> {
        self.schedule_edit(instance, frame, EditKind::Suspend)
    }

    /// Resumes a suspended `instance` from `frame` on (ADR 0016 §1a). The frame
    /// must come after its suspend. Call it before the frame runs; it may allocate.
    pub fn resume(&mut self, instance: &InstanceAddress, frame: Frame) -> Result<(), EngineError> {
        self.schedule_edit(instance, frame, EditKind::Resume)
    }

    fn schedule_edit(
        &mut self,
        instance: &InstanceAddress,
        frame: Frame,
        kind: EditKind,
    ) -> Result<(), EngineError> {
        let &i =
            self.plan
                .instance_ids
                .get(instance)
                .ok_or_else(|| EngineError::UnknownInstance {
                    instance: instance.clone(),
                })?;
        if frame < self.next_frame {
            return Err(EngineError::FrameInPast {
                frame,
                next: self.next_frame,
            });
        }
        let last = self.plan.last_edit.get(i).copied().flatten();
        let suspended = matches!(last, Some((EditKind::Suspend, _)));
        let alternates = match kind {
            EditKind::Suspend => !suspended,
            EditKind::Resume => suspended,
        };
        if !alternates || !last.is_none_or(|(_, previous)| frame > previous) {
            return Err(EngineError::InvalidEdit {
                instance: instance.clone(),
                frame,
            });
        }
        if let Some(last) = self.plan.last_edit.get_mut(i) {
            *last = Some((kind, frame));
        }
        let pending = self.scheduled.get(self.scheduled_next..).unwrap_or(&[]);
        let at = self.scheduled_next + pending.partition_point(|e| e.frame <= frame);
        self.scheduled.insert(
            at,
            ScheduledEdit {
                frame,
                kind,
                instance: instance.clone(),
            },
        );
        Ok(())
    }

    /// Queues an external event; allocates nothing. Its effective frame is
    /// `max(event.frame, the effective frame of the previous push)`: events
    /// arrive in frame order, and one stamped for a frame that has already run
    /// is applied on the next frame, which its log records name (§12). Its
    /// target is resolved when it is staged, against the plan running then;
    /// an unknown target or a non-finite value is counted in
    /// [`input_discards`](Engine::input_discards), not logged. Sequence numbers
    /// must be unique across the up-front list and pushed events.
    ///
    /// A full queue hands the event back. Used events stay in the queue until
    /// [`reclaim_externals`](Engine::reclaim_externals).
    pub fn push_external(&mut self, event: ExternalEvent) -> Result<(), ExternalFull> {
        if self.queue.len() >= self.config.queue_capacity {
            return Err(ExternalFull(event));
        }
        let at = event.frame.max(self.last_pushed_at);
        self.last_pushed_at = at;
        self.push_ordinal = self.push_ordinal.saturating_add(1);
        self.queue.push_back(Queued {
            at,
            ordinal: self.push_ordinal,
            event,
        });
        Ok(())
    }

    /// Queues a suspend of `instance` (ADR 0016 §1a); allocates nothing. Its
    /// effective frame is the latest of `frame`, the next frame and the previous
    /// pushed edit's. The instance is resolved when the edit applies; an edit
    /// that names no instance of the plan then, or does not fit its state, is
    /// counted in [`input_discards`](Engine::input_discards).
    pub fn push_suspend(
        &mut self,
        instance: InstanceAddress,
        frame: Frame,
    ) -> Result<(), EditFull> {
        self.push_edit(instance, frame, EditKind::Suspend)
    }

    /// Queues a resume of `instance`; see [`push_suspend`](Engine::push_suspend).
    pub fn push_resume(&mut self, instance: InstanceAddress, frame: Frame) -> Result<(), EditFull> {
        self.push_edit(instance, frame, EditKind::Resume)
    }

    fn push_edit(
        &mut self,
        instance: InstanceAddress,
        frame: Frame,
        kind: EditKind,
    ) -> Result<(), EditFull> {
        if self.pushed_edits.len() >= self.config.queue_capacity {
            return Err(EditFull(instance));
        }
        let at = frame.max(self.next_frame).max(self.last_pushed_edit_at);
        self.last_pushed_edit_at = at;
        self.pushed_edits
            .push_back(PushedEdit { at, kind, instance });
        Ok(())
    }

    /// Takes back the pushed external events that have been used, freeing their
    /// queue space. Drop them off the real-time thread.
    pub fn reclaim_externals(&mut self) -> impl Iterator<Item = ExternalEvent> + '_ {
        let used = mem::take(&mut self.queue_consumed);
        self.queue.drain(..used).map(|queued| queued.event)
    }

    /// Takes back the addresses of pushed edits that have been applied (or
    /// discarded), freeing their queue space. Drop them off the real-time thread.
    pub fn reclaim_edits(&mut self) -> impl Iterator<Item = InstanceAddress> + '_ {
        let used = mem::take(&mut self.edits_consumed);
        self.pushed_edits.drain(..used).map(|edit| edit.instance)
    }

    /// Instantiates `plan` for a swap (ADR 0006); call it off the real-time
    /// thread, it allocates. An instance with the same address, module type and
    /// descriptor as one running now **survives**: [`commit_swap`](Engine::commit_swap)
    /// moves its state in. Every other instance is created and prepared, and its
    /// `audio` outputs fade in from the swap frame. Up-front events and queued
    /// input stay with the engine.
    pub fn prepare_swap(
        &self,
        plan: &ExecutionPlan,
        registry: &Registry,
    ) -> Result<PreparedSwap, EngineError> {
        prepare_swap(
            &self.plan,
            SwapTarget {
                engine: self.id,
                version: self.version,
                config: self.config,
            },
            &self.upfront,
            plan,
            registry,
        )
    }

    /// The identity a swap must name to commit here, and the configuration
    /// it must be built with.
    pub(crate) fn swap_target(&self) -> SwapTarget {
        SwapTarget {
            engine: self.id,
            version: self.version,
            config: self.config,
        }
    }

    /// Swaps in a prepared plan between frames, effective from `frame`, which
    /// must be [`next_frame`](Engine::next_frame). Allocates nothing: it moves
    /// each survivor's state (O(instances + ports)), sets each held modulation,
    /// then exchanges the plans, so `swap` holds the old one afterwards. The
    /// version goes up by one and the frame's log starts with a swap record.
    ///
    /// Refused, before any state moves, when `swap` was prepared by another
    /// engine, when `frame` is not the next frame, when `swap` was prepared at
    /// another version or is already committed, when a swap is already
    /// committed for this frame, and when the last frame's log has not been
    /// drained (its records name the old plan's ports).
    pub fn commit_swap(
        &mut self,
        swap: &mut PreparedSwap,
        frame: Frame,
    ) -> Result<(), EngineError> {
        if swap.engine != self.id {
            return Err(EngineError::ForeignSwap);
        }
        if frame != self.next_frame {
            return Err(EngineError::SwapFrame {
                frame,
                next: self.next_frame,
            });
        }
        if swap.committed || swap.from_version != self.version {
            return Err(EngineError::StaleSwap);
        }
        if self.swap_record.is_some() {
            return Err(EngineError::SwapPending);
        }
        if !self.plan.log.is_empty() {
            return Err(EngineError::LogNotDrained);
        }
        let (new, old) = (&mut swap.plan, &mut self.plan);
        for &(n, o) in &swap.survivors {
            move_instance(new, old, n, o);
        }
        for hold in &swap.holds {
            let held = if hold.removed.is_empty() {
                old.params
                    .get(hold.old_param)
                    .and_then(|p| p.held_modulation)
            } else {
                let removed = swap
                    .removed_sources
                    .get(hold.removed.clone())
                    .unwrap_or(&[]);
                Some(sum(removed, &old.outputs))
            };
            if let Some(param) = new.params.get_mut(hold.param) {
                param.held_modulation = held;
            }
        }
        mem::swap(&mut self.plan, &mut swap.plan);
        swap.committed = true;
        self.version = self.version.saturating_add(1);
        self.swap_record = Some((self.version, swap.plan_version));
        Ok(())
    }

    /// The frame to swap out an instance suspended at `suspend`: its `audio`
    /// outputs are exactly 0 from there on (`suspend` + *F*).
    pub fn removal_swap_frame(&self, suspend: Frame) -> Frame {
        Frame(suspend.0.saturating_add(self.fade_len))
    }

    /// The plan version: 0 for the plan of [`new`](Engine::new), plus one per
    /// committed swap.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Runs the next frame *n* in the four phases of ADR 0016 §2:
    /// 1. external input: a swap committed for *n* (its record), the edits
    ///    effective at *n*, then the external events effective at *n* become
    ///    inputs;
    /// 2. emit: every state-output instance writes its outputs from state;
    /// 3. process: direct instances in schedule order, each after its frame-*n*
    ///    events and parameters are applied;
    /// 4. absorb: every state-output instance takes its frame-*n* inputs, events
    ///    and parameters.
    ///
    /// Allocates nothing. The frame's records are in [`frame_log`](Engine::frame_log)
    /// until the next call, or until [`drain_log`](Engine::drain_log).
    pub fn run_frame(&mut self) {
        let frame = self.next_frame;
        self.plan.log.clear();
        if let Some((version, plan_version)) = self.swap_record.take() {
            let record = LogRecord::Swap {
                frame,
                version,
                plan_version,
            };
            push_record(&mut self.plan.log, &mut self.log_overflows, record);
        }

        self.apply_edits(frame);
        self.stage_externals(frame);
        for sink in &mut self.plan.sinks {
            sink.clear();
        }
        for k in 0..self.plan.state_outputs.len() {
            if let Some(&i) = self.plan.state_outputs.get(k) {
                self.emit(i, frame);
            }
        }
        for k in 0..self.plan.direct_order.len() {
            if let Some(&i) = self.plan.direct_order.get(k) {
                self.process(i, frame);
            }
        }
        for k in 0..self.plan.state_outputs.len() {
            if let Some(&i) = self.plan.state_outputs.get(k) {
                self.absorb(i, frame);
            }
        }

        self.next_frame = Frame(frame.0.saturating_add(1));
    }

    /// Edits come first in a frame (ADR 0016 §3): the scheduled ones, then the
    /// pushed ones, each resolved against the plan running now.
    fn apply_edits(&mut self, frame: Frame) {
        while let Some(edit) = self.scheduled.get(self.scheduled_next) {
            if edit.frame > frame {
                break;
            }
            self.scheduled_next += 1;
            self.plan.apply_edit(
                &mut self.discards,
                &mut self.log_overflows,
                &edit.instance,
                edit.kind,
                frame,
                false,
            );
        }
        while let Some(edit) = self.pushed_edits.get(self.edits_consumed) {
            if edit.at > frame {
                break;
            }
            self.edits_consumed += 1;
            self.plan.apply_edit(
                &mut self.discards,
                &mut self.log_overflows,
                &edit.instance,
                edit.kind,
                frame,
                true,
            );
        }
    }

    /// The external events effective at `frame`, up-front then pushed, resolved
    /// against the running plan and sorted by (target, sequence).
    fn stage_externals(&mut self, frame: Frame) {
        self.staged.clear();
        while let Some(event) = self.upfront.get(self.upfront_next) {
            if event.frame > frame {
                break;
            }
            self.upfront_next += 1;
            stage(
                &self.plan,
                &mut self.staged,
                &mut self.discards,
                &mut self.input_overflows,
                event,
                false,
                self.upfront_next as u64,
            );
        }
        while let Some(queued) = self.queue.get(self.queue_consumed) {
            if queued.at > frame {
                break;
            }
            self.queue_consumed += 1;
            stage(
                &self.plan,
                &mut self.staged,
                &mut self.discards,
                &mut self.input_overflows,
                &queued.event,
                true,
                queued.ordinal,
            );
        }
        self.staged
            .sort_unstable_by_key(|s| (s.target, s.sequence, s.pushed, s.ordinal));
    }

    /// Phase 2 for one state-output instance.
    fn emit(&mut self, i: usize, frame: Frame) {
        let Some(slot) = self.plan.slots.get(i) else {
            return;
        };
        let (out, events) = (slot.signal_out.clone(), slot.event_out.clone());
        match slot.mode() {
            Mode::Running => {
                if let Some(Some(Primitive::StateOutput(module))) = self.plan.primitives.get_mut(i)
                {
                    let mut outputs = Outputs::new(
                        1,
                        self.plan.scratch.get_mut(out).unwrap_or(&mut []),
                        self.plan.sinks.get_mut(events).unwrap_or(&mut []),
                    );
                    module.emit(&mut outputs);
                }
                self.commit(i, frame);
            }
            Mode::Suspended => self.fade_suspended(i),
            Mode::Muted => self.fade_muted(i),
        }
    }

    /// Phase 3 for one direct instance.
    fn process(&mut self, i: usize, frame: Frame) {
        match self.gather(i, frame) {
            Mode::Running => {
                let plan = &mut self.plan;
                let Some(slot) = plan.slots.get(i) else {
                    return;
                };
                let (signal_in, out, events, params) = (
                    slot.signal_in.clone(),
                    slot.signal_out.clone(),
                    slot.event_out.clone(),
                    slot.params.clone(),
                );
                if let Some(Some(Primitive::Direct(module))) = plan.primitives.get_mut(i) {
                    let inputs = Inputs::new(
                        1,
                        plan.signal_in.get(signal_in).unwrap_or(&[]),
                        plan.param_values.get(params).unwrap_or(&[]),
                        plan.instance_events.get(i).map_or(&[][..], Vec::as_slice),
                        plan.instance_event_ends
                            .get(i)
                            .map_or(&[][..], Vec::as_slice),
                    );
                    let mut outputs = Outputs::new(
                        1,
                        plan.scratch.get_mut(out).unwrap_or(&mut []),
                        plan.sinks.get_mut(events).unwrap_or(&mut []),
                    );
                    module.process(&inputs, &mut outputs);
                }
                self.commit(i, frame);
            }
            Mode::Suspended => self.fade_suspended(i),
            Mode::Muted => self.fade_muted(i),
        }
    }

    /// Phase 4 for one state-output instance.
    fn absorb(&mut self, i: usize, frame: Frame) {
        if self.gather(i, frame) != Mode::Running {
            return;
        }
        let plan = &mut self.plan;
        let Some(slot) = plan.slots.get(i) else {
            return;
        };
        let (signal_in, params) = (slot.signal_in.clone(), slot.params.clone());
        if let Some(Some(Primitive::StateOutput(module))) = plan.primitives.get_mut(i) {
            let inputs = Inputs::new(
                1,
                plan.signal_in.get(signal_in).unwrap_or(&[]),
                plan.param_values.get(params).unwrap_or(&[]),
                plan.instance_events.get(i).map_or(&[][..], Vec::as_slice),
                plan.instance_event_ends
                    .get(i)
                    .map_or(&[][..], Vec::as_slice),
            );
            module.absorb(&inputs);
        }
    }

    /// Computes an instance's frame-*n* inputs when they are final: signal input
    /// sums, events on event inputs, and parameter bases and effective values
    /// (ADR 0016 §3, §4, §4a; §1a when suspended). Returns how the instance
    /// takes part in the frame.
    fn gather(&mut self, i: usize, frame: Frame) -> Mode {
        let plan = &mut self.plan;
        let Some(slot) = plan.slots.get(i) else {
            return Mode::Muted;
        };
        let mode = slot.mode();
        let (signal_in, event_in, params) = (
            slot.signal_in.clone(),
            slot.event_in.clone(),
            slot.params.clone(),
        );

        // Signal inputs: the canonical-order sum of scale · source, from +0
        // (computed even when the instance is not called: the audio sink's
        // inputs are what the offline driver records).
        for k in signal_in {
            let value = plan
                .signal_sources
                .get(k)
                .map_or(0.0, |sources| sum(sources, &plan.outputs));
            if let Some(input) = plan.signal_in.get_mut(k) {
                *input = value;
            }
        }

        let mut discards = 0u64;
        let sinks = &plan.sinks;
        let staged = self.staged.as_slice();

        // Event inputs: external events by sequence, then module events by
        // source port address, each source in emission order (ADR 0016 §3).
        if let (Some(buffer), Some(ends)) = (
            plan.instance_events.get_mut(i),
            plan.instance_event_ends.get_mut(i),
        ) {
            buffer.clear();
            let inputs = plan.event_inputs.get(event_in).unwrap_or(&[]);
            for (input, end) in inputs.iter().zip(ends.iter_mut()) {
                let target = input.port;
                for (payload, source) in arrivals(staged, target, &input.sources, sinks) {
                    let record = match mode {
                        Mode::Running => {
                            if buffer.len() >= buffer.capacity() {
                                // Unreachable within the construction-time
                                // bound; never reallocate, never call it delivered.
                                self.input_overflows = self.input_overflows.saturating_add(1);
                                continue;
                            }
                            buffer.push(Event { offset: 0, payload });
                            LogRecord::Event {
                                frame,
                                target,
                                payload,
                                source,
                            }
                        }
                        Mode::Suspended | Mode::Muted => {
                            discards = discards.saturating_add(1);
                            let reason = if mode == Mode::Muted {
                                DiscardReason::Muted
                            } else {
                                DiscardReason::Suspended
                            };
                            LogRecord::Discard {
                                frame,
                                target,
                                payload,
                                source,
                                reason,
                            }
                        }
                    };
                    push_record(&mut plan.log, &mut self.log_overflows, record);
                }
                *end = buffer.len();
            }
        }

        // Parameters: advance, then events in canonical order, then the
        // effective value (§4a, §4). While suspended, events act on the base only.
        for j in params {
            let (Some(param), Some(value)) = (plan.params.get_mut(j), plan.param_values.get_mut(j))
            else {
                continue;
            };
            if mode == Mode::Running {
                param.base.advance();
            }
            let target = param.port;
            for (payload, source) in arrivals(staged, target, &param.events, sinks) {
                let applied = match mode {
                    Mode::Running => Some(param.base.apply(payload)),
                    Mode::Suspended => Some(param.base.apply_suspended(payload)),
                    Mode::Muted => None,
                };
                // A set releases modulation held from removed sources (§12).
                if applied == Some(Applied::Yes) && is_set(payload) {
                    param.held_modulation = None;
                }
                let reason = match (applied, mode) {
                    (Some(Applied::Yes), _) => None,
                    (None, _) => Some(DiscardReason::Muted),
                    (Some(Applied::NotAParameterEvent), Mode::Suspended) => {
                        Some(DiscardReason::Suspended)
                    }
                    (Some(Applied::NotAParameterEvent), _) => {
                        Some(DiscardReason::NotAParameterEvent)
                    }
                };
                let record = match reason {
                    None => LogRecord::Event {
                        frame,
                        target,
                        payload,
                        source,
                    },
                    Some(reason) => {
                        discards = discards.saturating_add(1);
                        LogRecord::Discard {
                            frame,
                            target,
                            payload,
                            source,
                            reason,
                        }
                    }
                };
                push_record(&mut plan.log, &mut self.log_overflows, record);
            }
            if mode == Mode::Running {
                let modulation = if param.modulation.is_empty() {
                    param.held_modulation
                } else {
                    Some(sum(&param.modulation, &plan.outputs))
                };
                *value = effective(param.base.value(), modulation, param.combine, param.range);
            }
        }

        if discards > 0
            && let Some(slot) = plan.slots.get_mut(i)
        {
            slot.discards = slot.discards.saturating_add(discards);
            if mode == Mode::Suspended {
                slot.suspension_discards = slot.suspension_discards.saturating_add(discards);
            }
        }
        mode
    }

    /// After a call of a running instance: the fault guard, then its outputs
    /// become visible, with the resume (or swap) fade-in on `audio` outputs.
    fn commit(&mut self, i: usize, frame: Frame) {
        let plan = &mut self.plan;
        let Some(slot) = plan.slots.get_mut(i) else {
            return;
        };
        let (out, events) = (slot.signal_out.clone(), slot.event_out.clone());
        let fault = check(
            plan.scratch.get(out.clone()).unwrap_or(&[]),
            plan.output_ports.get(out.clone()).unwrap_or(&[]),
            plan.sinks.get(events.clone()).unwrap_or(&[]),
            plan.sink_ports.get(events.clone()).unwrap_or(&[]),
            plan.sink_overflows
                .get_mut(events.clone())
                .unwrap_or(&mut []),
        );
        if let Some((port, reason)) = fault {
            // Fade from what readers saw at frame n − 1; deliver none of this
            // call's events; never call the instance again.
            for o in out {
                if let (Some(held), Some(&visible)) = (plan.held.get_mut(o), plan.outputs.get(o)) {
                    *held = visible;
                }
            }
            for sink in plan.sinks.get_mut(events).unwrap_or(&mut []) {
                sink.clear();
            }
            slot.fault = Some(0);
            let record = LogRecord::Fault {
                frame,
                instance: InstanceId(i),
                port,
                reason,
            };
            push_record(&mut plan.log, &mut self.log_overflows, record);
            self.fade_muted(i);
            return;
        }

        let fade = slot.fade;
        for o in out {
            let audio = plan.output_audio.get(o).copied().unwrap_or(false);
            if let (Some(visible), Some(&live)) = (plan.outputs.get_mut(o), plan.scratch.get(o)) {
                *visible = match fade {
                    Fade::In(j) if audio => fade_in(live, j, self.fade_len),
                    _ => live,
                };
            }
        }
        if let Fade::In(j) = fade {
            let next = j.saturating_add(1);
            slot.fade = if next >= self.fade_len {
                Fade::None
            } else {
                Fade::In(next)
            };
        }
    }

    /// A suspended instance's frame: other outputs hold; `audio` outputs take
    /// the next fade-out frame (exactly 0 from the F-th suspended frame on).
    fn fade_suspended(&mut self, i: usize) {
        let plan = &mut self.plan;
        let Some(slot) = plan.slots.get_mut(i) else {
            return;
        };
        let Fade::Out(j) = slot.fade else {
            return;
        };
        for o in slot.signal_out.clone() {
            if plan.output_audio.get(o) != Some(&true) {
                continue;
            }
            if let (Some(visible), Some(&held)) = (plan.outputs.get_mut(o), plan.held.get(o)) {
                *visible = fade_out(held, j, self.fade_len);
            }
        }
        slot.fade = Fade::Out(j.saturating_add(1));
    }

    /// A muted instance's frame: every signal output takes the next frame of the
    /// 64-frame fault fade (exactly 0 from its 64th frame on).
    fn fade_muted(&mut self, i: usize) {
        let plan = &mut self.plan;
        let Some(slot) = plan.slots.get_mut(i) else {
            return;
        };
        let Some(j) = slot.fault else {
            return;
        };
        for o in slot.signal_out.clone() {
            if let (Some(visible), Some(&held)) = (plan.outputs.get_mut(o), plan.held.get(o)) {
                *visible = fade_out(held, j, FAULT_FADE_FRAMES);
            }
        }
        slot.fault = Some(j.saturating_add(1));
    }

    /// The configuration the engine was built with.
    pub fn config(&self) -> EngineConfig {
        self.config
    }

    /// The next frame [`run_frame`](Engine::run_frame) will run.
    pub fn next_frame(&self) -> Frame {
        self.next_frame
    }

    /// The suspension fade length *F* in frames: 5 ms at the sample rate,
    /// rounded half up.
    pub fn fade_frames(&self) -> u64 {
        self.fade_len
    }

    /// What the last frame did, in execution order.
    pub fn frame_log(&self) -> &[LogRecord] {
        &self.plan.log
    }

    /// Hands the last frame's records to `out` in canonical order (that of
    /// `log.jsonl`, `docs/semantics.md` §9) and empties the log; allocates
    /// nothing. The log holds one frame: drain it after each
    /// [`run_frame`](Engine::run_frame), and before a
    /// [`commit_swap`](Engine::commit_swap), which refuses an undrained log.
    pub fn drain_log(&mut self, out: &mut impl FnMut(&LogRecord)) {
        let plan = &mut self.plan;
        let log = &plan.log;
        plan.log_order.clear();
        plan.log_order.extend(0..log.len());
        plan.log_order
            .sort_unstable_by_key(|&k| (log.get(k).map_or((u8::MAX, 0), LogRecord::order), k));
        for &k in &plan.log_order {
            if let Some(record) = log.get(k) {
                out(record);
            }
        }
        plan.log.clear();
    }

    /// Log records dropped because the frame's log was full. The buffer is
    /// sized so that this stays 0.
    pub fn log_overflows(&self) -> u64 {
        self.log_overflows
    }

    /// Events dropped, not delivered and not logged, because an instance's
    /// event-input buffer was full. The buffer is sized so that this stays 0.
    pub fn input_overflows(&self) -> u64 {
        self.input_overflows
    }

    /// External events and edits that had no effect and are not in the log.
    pub fn input_discards(&self) -> InputDiscards {
        self.discards
    }

    /// The id of `port` in log records.
    pub fn port_id(&self, port: &PortAddress) -> Option<PortId> {
        self.plan.ports.binary_search(port).ok().map(PortId)
    }

    /// `record` with addresses, as one line of `log.jsonl`. Records name the
    /// ports and instances of the plan that wrote them: convert them before
    /// the next swap.
    pub fn log_line(&self, record: &LogRecord) -> Option<LogLine<'_>> {
        self.plan.line(record)
    }

    fn role(&self, port: &PortAddress) -> Option<Role> {
        Some(self.plan.role_of(port)?.1)
    }

    fn slot(&self, instance: &InstanceAddress) -> Option<&Slot> {
        self.plan.slots.get(*self.plan.instance_ids.get(instance)?)
    }

    /// The value readers saw of signal output `port` in the last frame.
    pub fn signal_output(&self, port: &PortAddress) -> Option<f64> {
        match self.role(port)? {
            Role::SignalOut(o) => self.plan.outputs.get(o).copied(),
            _ => None,
        }
    }

    /// The summed value of signal input `port` in the last frame.
    pub fn signal_input(&self, port: &PortAddress) -> Option<f64> {
        match self.role(port)? {
            Role::SignalIn(k) => self.plan.signal_in.get(k).copied(),
            _ => None,
        }
    }

    /// The base and effective value of parameter `port`.
    pub fn parameter(&self, port: &PortAddress) -> Option<ParameterValue> {
        match self.role(port)? {
            Role::Param(j) => Some(ParameterValue {
                base: self.plan.params.get(j)?.base.value(),
                effective: self.plan.param_values.get(j).copied()?,
            }),
            _ => None,
        }
    }

    /// How many events reached `instance` and had no effect: while it was
    /// suspended or muted, or triggers sent to its parameters.
    pub fn discards(&self, instance: &InstanceAddress) -> Option<u64> {
        self.slot(instance).map(|s| s.discards)
    }

    /// Whether `instance` is suspended.
    pub fn is_suspended(&self, instance: &InstanceAddress) -> Option<bool> {
        self.slot(instance).map(|s| s.suspended)
    }

    /// Whether the fault guard muted `instance`.
    pub fn is_muted(&self, instance: &InstanceAddress) -> Option<bool> {
        self.slot(instance).map(|s| s.fault.is_some())
    }

    /// The instances whose descriptor declares an audio sink.
    pub(crate) fn audio_sinks(&self) -> Vec<InstanceId> {
        (0..self.plan.slots.len())
            .filter(|&i| self.plan.slots.get(i).is_some_and(|s| s.audio_sink))
            .map(InstanceId)
            .collect()
    }

    /// The first instance whose descriptor declares an audio sink; allocates
    /// nothing, unlike [`audio_sinks`](Engine::audio_sinks).
    pub(crate) fn first_audio_sink(&self) -> Option<InstanceId> {
        self.plan
            .slots
            .iter()
            .position(|s| s.audio_sink)
            .map(InstanceId)
    }

    /// The address of `instance`.
    pub(crate) fn instance_address(&self, instance: InstanceId) -> Option<&InstanceAddress> {
        self.plan.instances.get(instance.0)
    }

    /// The signal inputs of `instance` in the last frame, in port order.
    pub(crate) fn instance_inputs(&self, instance: InstanceId) -> &[f64] {
        self.plan
            .slots
            .get(instance.0)
            .and_then(|s| self.plan.signal_in.get(s.signal_in.clone()))
            .unwrap_or(&[])
    }
}

/// The address of port `name` of `instance`.
fn port_address(
    instance: &InstanceAddress,
    name: &'static str,
) -> Result<PortAddress, EngineError> {
    PortAddress::new(instance, name).map_err(|_| EngineError::InvalidPortName {
        instance: instance.clone(),
        name,
    })
}

/// Whether a `set` or `ramp` carries a finite value (a trigger always does).
fn finite(payload: EventPayload) -> bool {
    match payload {
        EventPayload::Set(value) | EventPayload::Ramp { to: value, .. } => value.is_finite(),
        EventPayload::Trigger => true,
    }
}

/// Whether `payload` sets a base at once: `set`, or `ramp` over 0 frames.
fn is_set(payload: EventPayload) -> bool {
    matches!(
        payload,
        EventPayload::Set(_) | EventPayload::Ramp { frames: 0, .. }
    )
}

/// Stages one external event of the current frame, resolved against `plan`.
fn stage(
    plan: &PlanState,
    staged: &mut Vec<Staged>,
    discards: &mut InputDiscards,
    overflows: &mut u64,
    event: &ExternalEvent,
    pushed: bool,
    ordinal: u64,
) {
    let target = match plan.role_of(&event.target) {
        Some((target, Role::EventIn(_) | Role::Param(_))) => target,
        _ => {
            discards.unrouted = discards.unrouted.saturating_add(1);
            return;
        }
    };
    if !finite(event.payload) {
        discards.invalid = discards.invalid.saturating_add(1);
        return;
    }
    if staged.len() >= staged.capacity() {
        // Unreachable within the construction-time bound; never reallocate.
        *overflows = overflows.saturating_add(1);
        return;
    }
    staged.push(Staged {
        target,
        sequence: event.sequence,
        pushed,
        ordinal,
        payload: event.payload,
    });
}

/// The events arriving at `target` this frame in canonical order: the staged
/// external events for it (sorted by sequence), then each event source's
/// events in source order and emission order. A source connected several
/// times delivers each event once per connection before its next event.
fn arrivals<'a>(
    staged: &'a [Staged],
    target: PortId,
    sources: &'a [EventSource],
    sinks: &'a [EventSink],
) -> impl Iterator<Item = (EventPayload, LogSource)> + 'a {
    let start = staged.partition_point(|e| e.target < target.0);
    let end = staged.partition_point(|e| e.target <= target.0);
    let outside = staged.get(start..end).unwrap_or(&[]).iter().map(|e| {
        (
            e.payload,
            LogSource::External {
                sequence: e.sequence,
            },
        )
    });
    let modules = sources.iter().flat_map(move |source| {
        let events = sinks.get(source.sink).map_or(&[][..], EventSink::events);
        let from = LogSource::Port(source.port);
        events
            .iter()
            .flat_map(move |e| std::iter::repeat_n((e.payload, from), source.copies))
    });
    outside.chain(modules)
}

/// The canonical-order sum of `scale · output`, starting from +0.
fn sum(sources: &[(usize, f64)], outputs: &[f64]) -> f64 {
    sources.iter().fold(0.0, |total, &(o, scale)| {
        total + scale * outputs.get(o).copied().unwrap_or(0.0)
    })
}

/// The fault guard: the first output, signal outputs then event outputs in
/// descriptor order, that is non-finite, larger than 10⁶ in magnitude, or an
/// overflowing event sink.
fn check(
    signals: &[f64],
    signal_ports: &[PortId],
    sinks: &[EventSink],
    sink_ports: &[PortId],
    overflows_seen: &mut [u64],
) -> Option<(PortId, FaultReason)> {
    for (&value, &port) in signals.iter().zip(signal_ports) {
        if let Some(reason) = fault_of(value) {
            return Some((port, reason));
        }
    }
    for ((sink, &port), seen) in sinks.iter().zip(sink_ports).zip(overflows_seen) {
        if sink.overflows() > *seen {
            *seen = sink.overflows();
            return Some((port, FaultReason::EventOverflow));
        }
        for event in sink.events() {
            let value = match event.payload {
                EventPayload::Set(value) | EventPayload::Ramp { to: value, .. } => value,
                EventPayload::Trigger => continue,
            };
            if let Some(reason) = fault_of(value) {
                return Some((port, reason));
            }
        }
    }
    None
}

fn fault_of(value: f64) -> Option<FaultReason> {
    if !value.is_finite() {
        Some(FaultReason::NonFinite)
    } else if value.abs() > FAULT_MAGNITUDE {
        Some(FaultReason::Magnitude)
    } else {
        None
    }
}

/// Appends `record` if the log has room; never allocates.
fn push_record(log: &mut Vec<LogRecord>, overflows: &mut u64, record: LogRecord) {
    if log.len() < log.capacity() {
        log.push(record);
    } else {
        *overflows = overflows.saturating_add(1);
    }
}
