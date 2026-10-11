//! The per-frame executor (ADR 0016, *k* = 1): instantiation, the four frame
//! phases, event delivery in canonical order, parameter bases, suspension and the
//! fault guard. Normative description: `docs/semantics.md`.
//!
//! Every buffer is sized in [`Engine::new`]; [`Engine::run_frame`] allocates,
//! locks, performs I/O and panics nowhere (`docs/realtime-rules.md`).

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range as Span;

use signal_core::{
    Combine, Direction, Event, EventPayload, EventSink, Feedthrough, Frame, Inputs,
    InstanceAddress, Outputs, PortAddress, PortKind, PortShape, PrepareContext, Primitive, Range,
    RateDivisor, Registry, RngStream, SinkKind,
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

/// What a run is prepared for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineConfig {
    /// The master rate in frames per second (the project sample rate).
    pub sample_rate: u32,
    /// The master seed; each instance's stream is `RngStream::derive(seed, address)`.
    pub seed: u64,
}

/// An event from outside the graph (ADR 0016 §3): from the session log, the CLI
/// or a test. Same-frame external events are delivered in `sequence` order,
/// before any module-emitted event.
#[derive(Clone, Debug, PartialEq)]
pub struct ExternalEvent {
    /// The frame it is stamped for.
    pub frame: Frame,
    /// Its log sequence number, unique in the list.
    pub sequence: u64,
    /// An event input port or a parameter of a primitive instance.
    pub target: PortAddress,
    /// The event.
    pub payload: EventPayload,
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
    /// Event sources, in canonical order.
    events: Vec<EventSource>,
}

/// An external event resolved to its target port.
#[derive(Clone, Copy, Debug)]
struct Staged {
    frame: Frame,
    target: usize,
    sequence: u64,
    payload: EventPayload,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditKind {
    Suspend,
    Resume,
}

#[derive(Clone, Copy, Debug)]
struct Edit {
    frame: Frame,
    instance: usize,
    kind: EditKind,
}

/// An instantiated [`ExecutionPlan`], run one frame at a time (ADR 0016).
///
/// Instances, ports and parameters live in flat buffers sized at construction.
/// Each frame runs external input (edits, then external events), emit, process
/// in schedule order and absorb; see [`Engine::run_frame`].
#[derive(Debug)]
pub struct Engine {
    config: EngineConfig,
    fade_len: u64,
    next_frame: Frame,

    instances: Vec<InstanceAddress>,
    instance_ids: BTreeMap<InstanceAddress, usize>,
    /// Every port, in address order: a [`PortId`] indexes it.
    ports: Vec<PortAddress>,
    roles: Vec<Option<(usize, Role)>>,
    slots: Vec<Slot>,
    primitives: Vec<Primitive>,
    direct_order: Vec<usize>,
    state_outputs: Vec<usize>,

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

    externals: Vec<Staged>,
    external_next: usize,
    staged: Span<usize>,

    edits: Vec<Edit>,
    edit_next: usize,
    last_edit: Vec<Option<(EditKind, Frame)>>,

    log: Vec<LogRecord>,
    log_overflows: u64,
    input_overflows: u64,
}

impl Engine {
    /// Instantiates `plan`: creates every primitive through `registry`, prepares
    /// it with the sample rate, *k* = 1 and `RngStream::derive(seed, address)`,
    /// and allocates every buffer the frames need.
    ///
    /// `external` is the caller's frame-stamped event list. The plan's schedule
    /// is checked, not trusted: any order that runs each direct source before its
    /// receivers is accepted (ADR 0016 §1), so a test may supply another one.
    pub fn new(
        plan: &ExecutionPlan,
        registry: &Registry,
        config: EngineConfig,
        external: Vec<ExternalEvent>,
    ) -> Result<Engine, EngineError> {
        if config.sample_rate == 0 {
            return Err(EngineError::InvalidSampleRate);
        }

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
        let mut slots = Vec::with_capacity(plan.instances.len());
        let mut primitives = Vec::with_capacity(plan.instances.len());
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
                fade: Fade::None,
                discards: 0,
                suspension_discards: 0,
            });
            primitives.push(primitive);
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

        // External events: resolved, checked, sorted by (frame, target, sequence).
        let mut sequences = BTreeSet::new();
        let mut externals = Vec::with_capacity(external.len());
        for event in external {
            let Some((target, role)) = role_of(&event.target) else {
                return Err(EngineError::UnknownTarget {
                    target: event.target,
                });
            };
            if !matches!(role.1, Role::EventIn(_) | Role::Param(_)) {
                return Err(EngineError::NotAnEventTarget {
                    target: event.target,
                });
            }
            let finite = match event.payload {
                EventPayload::Set(value) | EventPayload::Ramp { to: value, .. } => {
                    value.is_finite()
                }
                EventPayload::Trigger => true,
            };
            if !finite {
                return Err(EngineError::NonFiniteValue {
                    sequence: event.sequence,
                });
            }
            if !sequences.insert(event.sequence) {
                return Err(EngineError::DuplicateSequence {
                    sequence: event.sequence,
                });
            }
            externals.push(Staged {
                frame: event.frame,
                target,
                sequence: event.sequence,
                payload: event.payload,
            });
        }
        externals.sort_by_key(|e| (e.frame, e.target, e.sequence));

        // Buffer bounds. X(t): the most externals stamped for one frame and
        // target; X_tot: the most for one frame.
        let mut most_per_target = vec![0usize; ports.len()];
        let mut most_per_frame = 0usize;
        let (mut run_target, mut run_frame) = (0usize, 0usize);
        let mut previous: Option<&Staged> = None;
        for event in &externals {
            let same_frame = previous.is_some_and(|p| p.frame == event.frame);
            let same_target = same_frame && previous.is_some_and(|p| p.target == event.target);
            run_frame = if same_frame { run_frame + 1 } else { 1 };
            run_target = if same_target { run_target + 1 } else { 1 };
            if let Some(most) = most_per_target.get_mut(event.target) {
                *most = (*most).max(run_target);
            }
            most_per_frame = most_per_frame.max(run_frame);
            previous = Some(event);
        }
        // Per event input t: cap(t) = X(t) + 16 · (its event connections,
        // duplicates included). A sink keeps at most 16 events per frame, and
        // each connection delivers each once, so a frame never exceeds it.
        let mut instance_events = Vec::with_capacity(slots.len());
        let mut instance_event_ends = Vec::with_capacity(slots.len());
        for slot in &slots {
            let capacity: usize = event_inputs
                .get(slot.event_in.clone())
                .unwrap_or(&[])
                .iter()
                .map(|input| {
                    most_per_target.get(input.port.0).copied().unwrap_or(0)
                        + EVENTS_PER_FRAME * connections(&input.sources)
                })
                .sum();
            instance_events.push(Vec::with_capacity(capacity));
            instance_event_ends.push(vec![0; slot.event_in.len()]);
        }
        // The log: at most one edit and one fault per instance, every staged
        // external, and 16 events per event connection, in one frame.
        let event_connections: usize = event_inputs
            .iter()
            .map(|i| connections(&i.sources))
            .sum::<usize>()
            + params.iter().map(|p| connections(&p.events)).sum::<usize>();
        let log_capacity = 2 * slots.len() + most_per_frame + EVENTS_PER_FRAME * event_connections;

        let outputs = output_audio.len();
        Ok(Engine {
            config,
            fade_len: fade_frames(config.sample_rate),
            next_frame: Frame::ZERO,
            last_edit: vec![None; instances.len()],
            instances,
            instance_ids,
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
            externals,
            external_next: 0,
            staged: 0..0,
            edits: Vec::new(),
            edit_next: 0,
            log: Vec::with_capacity(log_capacity),
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
        let &i = self
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
        let last = self.last_edit.get(i).copied().flatten();
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
        if let Some(last) = self.last_edit.get_mut(i) {
            *last = Some((kind, frame));
        }
        let at = self.edits.partition_point(|e| e.frame <= frame);
        self.edits.insert(
            at,
            Edit {
                frame,
                instance: i,
                kind,
            },
        );
        Ok(())
    }

    /// Runs the next frame *n* in the four phases of ADR 0016 §2:
    /// 1. external input: the edits stamped for *n*, then the external events
    ///    stamped for *n* become inputs;
    /// 2. emit: every state-output instance writes its outputs from state;
    /// 3. process: direct instances in schedule order, each after its frame-*n*
    ///    events and parameters are applied;
    /// 4. absorb: every state-output instance takes its frame-*n* inputs, events
    ///    and parameters.
    ///
    /// Allocates nothing. The frame's records are in [`frame_log`](Engine::frame_log)
    /// until the next call.
    pub fn run_frame(&mut self) {
        let frame = self.next_frame;
        self.log.clear();

        self.apply_edits(frame);
        self.stage_externals(frame);
        for sink in &mut self.sinks {
            sink.clear();
        }
        for k in 0..self.state_outputs.len() {
            if let Some(&i) = self.state_outputs.get(k) {
                self.emit(i, frame);
            }
        }
        for k in 0..self.direct_order.len() {
            if let Some(&i) = self.direct_order.get(k) {
                self.process(i, frame);
            }
        }
        for k in 0..self.state_outputs.len() {
            if let Some(&i) = self.state_outputs.get(k) {
                self.absorb(i, frame);
            }
        }

        self.next_frame = Frame(frame.0.saturating_add(1));
    }

    /// Edits come first in a frame (ADR 0016 §3). A suspend holds every output
    /// at its frame *s* − 1 value and starts the fade-out of `audio` outputs; a
    /// resume starts their fade-in from 0. Neither affects a muted instance.
    fn apply_edits(&mut self, frame: Frame) {
        while let Some(&edit) = self.edits.get(self.edit_next) {
            if edit.frame > frame {
                break;
            }
            self.edit_next += 1;
            let Some(slot) = self.slots.get_mut(edit.instance) else {
                continue;
            };
            let instance = InstanceId(edit.instance);
            let record = match edit.kind {
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
            push_record(&mut self.log, &mut self.log_overflows, record);
        }
    }

    fn stage_externals(&mut self, frame: Frame) {
        let start = self.external_next;
        let mut end = start;
        while self.externals.get(end).is_some_and(|e| e.frame <= frame) {
            end += 1;
        }
        self.external_next = end;
        self.staged = start..end;
    }

    /// Phase 2 for one state-output instance.
    fn emit(&mut self, i: usize, frame: Frame) {
        let Some(slot) = self.slots.get(i) else {
            return;
        };
        let (out, events) = (slot.signal_out.clone(), slot.event_out.clone());
        match slot.mode() {
            Mode::Running => {
                if let Some(Primitive::StateOutput(module)) = self.primitives.get_mut(i) {
                    let mut outputs = Outputs::new(
                        1,
                        self.scratch.get_mut(out).unwrap_or(&mut []),
                        self.sinks.get_mut(events).unwrap_or(&mut []),
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
                let Some(slot) = self.slots.get(i) else {
                    return;
                };
                let (signal_in, out, events, params) = (
                    slot.signal_in.clone(),
                    slot.signal_out.clone(),
                    slot.event_out.clone(),
                    slot.params.clone(),
                );
                if let Some(Primitive::Direct(module)) = self.primitives.get_mut(i) {
                    let inputs = Inputs::new(
                        1,
                        self.signal_in.get(signal_in).unwrap_or(&[]),
                        self.param_values.get(params).unwrap_or(&[]),
                        self.instance_events.get(i).map_or(&[][..], Vec::as_slice),
                        self.instance_event_ends
                            .get(i)
                            .map_or(&[][..], Vec::as_slice),
                    );
                    let mut outputs = Outputs::new(
                        1,
                        self.scratch.get_mut(out).unwrap_or(&mut []),
                        self.sinks.get_mut(events).unwrap_or(&mut []),
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
        let Some(slot) = self.slots.get(i) else {
            return;
        };
        let (signal_in, params) = (slot.signal_in.clone(), slot.params.clone());
        if let Some(Primitive::StateOutput(module)) = self.primitives.get_mut(i) {
            let inputs = Inputs::new(
                1,
                self.signal_in.get(signal_in).unwrap_or(&[]),
                self.param_values.get(params).unwrap_or(&[]),
                self.instance_events.get(i).map_or(&[][..], Vec::as_slice),
                self.instance_event_ends
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
        let Some(slot) = self.slots.get(i) else {
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
            let value = self
                .signal_sources
                .get(k)
                .map_or(0.0, |sources| sum(sources, &self.outputs));
            if let Some(input) = self.signal_in.get_mut(k) {
                *input = value;
            }
        }

        let mut discards = 0u64;
        let sinks = &self.sinks;
        let externals = &self.externals;
        let staged = self.staged.clone();

        // Event inputs: external events by sequence, then module events by
        // source port address, each source in emission order (ADR 0016 §3).
        if let (Some(buffer), Some(ends)) = (
            self.instance_events.get_mut(i),
            self.instance_event_ends.get_mut(i),
        ) {
            buffer.clear();
            let inputs = self.event_inputs.get(event_in).unwrap_or(&[]);
            for (input, end) in inputs.iter().zip(ends.iter_mut()) {
                let target = input.port;
                let arrivals = arrivals(externals, staged.clone(), target, &input.sources, sinks);
                for (payload, source) in arrivals {
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
                    push_record(&mut self.log, &mut self.log_overflows, record);
                }
                *end = buffer.len();
            }
        }

        // Parameters: advance, then events in canonical order, then the
        // effective value (§4a, §4). While suspended, events act on the base only.
        for j in params {
            let (Some(param), Some(value)) = (self.params.get_mut(j), self.param_values.get_mut(j))
            else {
                continue;
            };
            if mode == Mode::Running {
                param.base.advance();
            }
            let target = param.port;
            for (payload, source) in
                arrivals(externals, staged.clone(), target, &param.events, sinks)
            {
                let applied = match mode {
                    Mode::Running => Some(param.base.apply(payload)),
                    Mode::Suspended => Some(param.base.apply_suspended(payload)),
                    Mode::Muted => None,
                };
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
                push_record(&mut self.log, &mut self.log_overflows, record);
            }
            if mode == Mode::Running {
                let modulation =
                    (!param.modulation.is_empty()).then(|| sum(&param.modulation, &self.outputs));
                *value = effective(param.base.value(), modulation, param.combine, param.range);
            }
        }

        if discards > 0
            && let Some(slot) = self.slots.get_mut(i)
        {
            slot.discards = slot.discards.saturating_add(discards);
            if mode == Mode::Suspended {
                slot.suspension_discards = slot.suspension_discards.saturating_add(discards);
            }
        }
        mode
    }

    /// After a call of a running instance: the fault guard, then its outputs
    /// become visible, with the resume fade-in on `audio` outputs.
    fn commit(&mut self, i: usize, frame: Frame) {
        let Some(slot) = self.slots.get_mut(i) else {
            return;
        };
        let (out, events) = (slot.signal_out.clone(), slot.event_out.clone());
        let fault = check(
            self.scratch.get(out.clone()).unwrap_or(&[]),
            self.output_ports.get(out.clone()).unwrap_or(&[]),
            self.sinks.get(events.clone()).unwrap_or(&[]),
            self.sink_ports.get(events.clone()).unwrap_or(&[]),
            self.sink_overflows
                .get_mut(events.clone())
                .unwrap_or(&mut []),
        );
        if let Some((port, reason)) = fault {
            // Fade from what readers saw at frame n − 1; deliver none of this
            // call's events; never call the instance again.
            for o in out {
                if let (Some(held), Some(&visible)) = (self.held.get_mut(o), self.outputs.get(o)) {
                    *held = visible;
                }
            }
            for sink in self.sinks.get_mut(events).unwrap_or(&mut []) {
                sink.clear();
            }
            slot.fault = Some(0);
            let record = LogRecord::Fault {
                frame,
                instance: InstanceId(i),
                port,
                reason,
            };
            push_record(&mut self.log, &mut self.log_overflows, record);
            self.fade_muted(i);
            return;
        }

        let fade = slot.fade;
        for o in out {
            let audio = self.output_audio.get(o).copied().unwrap_or(false);
            if let (Some(visible), Some(&live)) = (self.outputs.get_mut(o), self.scratch.get(o)) {
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
        let Some(slot) = self.slots.get_mut(i) else {
            return;
        };
        let Fade::Out(j) = slot.fade else {
            return;
        };
        for o in slot.signal_out.clone() {
            if self.output_audio.get(o) != Some(&true) {
                continue;
            }
            if let (Some(visible), Some(&held)) = (self.outputs.get_mut(o), self.held.get(o)) {
                *visible = fade_out(held, j, self.fade_len);
            }
        }
        slot.fade = Fade::Out(j.saturating_add(1));
    }

    /// A muted instance's frame: every signal output takes the next frame of the
    /// 64-frame fault fade (exactly 0 from its 64th frame on).
    fn fade_muted(&mut self, i: usize) {
        let Some(slot) = self.slots.get_mut(i) else {
            return;
        };
        let Some(j) = slot.fault else {
            return;
        };
        for o in slot.signal_out.clone() {
            if let (Some(visible), Some(&held)) = (self.outputs.get_mut(o), self.held.get(o)) {
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
        &self.log
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

    /// The id of `port` in log records.
    pub fn port_id(&self, port: &PortAddress) -> Option<PortId> {
        self.ports.binary_search(port).ok().map(PortId)
    }

    /// `record` with addresses, as one line of `log.jsonl`.
    pub fn log_line(&self, record: &LogRecord) -> Option<LogLine<'_>> {
        Some(match *record {
            LogRecord::Event {
                frame,
                target,
                payload,
                source,
            } => LogLine::Event {
                frame,
                target: self.ports.get(target.0)?,
                payload,
                source: self.line_source(source)?,
            },
            LogRecord::Discard {
                frame,
                target,
                payload,
                source,
                reason,
            } => LogLine::Discard {
                frame,
                target: self.ports.get(target.0)?,
                payload,
                source: self.line_source(source)?,
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

    fn line_source(&self, source: LogSource) -> Option<LineSource<'_>> {
        Some(match source {
            LogSource::External { sequence } => LineSource::External { sequence },
            LogSource::Port(id) => LineSource::Port(self.ports.get(id.0)?),
        })
    }

    fn role(&self, port: &PortAddress) -> Option<Role> {
        let id = self.ports.binary_search(port).ok()?;
        Some(self.roles.get(id).copied().flatten()?.1)
    }

    fn slot(&self, instance: &InstanceAddress) -> Option<&Slot> {
        self.slots.get(*self.instance_ids.get(instance)?)
    }

    /// The value readers saw of signal output `port` in the last frame.
    pub fn signal_output(&self, port: &PortAddress) -> Option<f64> {
        match self.role(port)? {
            Role::SignalOut(o) => self.outputs.get(o).copied(),
            _ => None,
        }
    }

    /// The summed value of signal input `port` in the last frame.
    pub fn signal_input(&self, port: &PortAddress) -> Option<f64> {
        match self.role(port)? {
            Role::SignalIn(k) => self.signal_in.get(k).copied(),
            _ => None,
        }
    }

    /// The base and effective value of parameter `port`.
    pub fn parameter(&self, port: &PortAddress) -> Option<ParameterValue> {
        match self.role(port)? {
            Role::Param(j) => Some(ParameterValue {
                base: self.params.get(j)?.base.value(),
                effective: self.param_values.get(j).copied()?,
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
        (0..self.slots.len())
            .filter(|&i| self.slots.get(i).is_some_and(|s| s.audio_sink))
            .map(InstanceId)
            .collect()
    }

    /// The address of `instance`.
    pub(crate) fn instance_address(&self, instance: InstanceId) -> Option<&InstanceAddress> {
        self.instances.get(instance.0)
    }

    /// The signal inputs of `instance` in the last frame, in port order.
    pub(crate) fn instance_inputs(&self, instance: InstanceId) -> &[f64] {
        self.slots
            .get(instance.0)
            .and_then(|s| self.signal_in.get(s.signal_in.clone()))
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

/// The events arriving at `target` this frame in canonical order: the staged
/// external events for it (sorted by sequence), then each event source's
/// events in source order and emission order. A source connected several
/// times delivers each event once per connection before its next event.
fn arrivals<'a>(
    externals: &'a [Staged],
    staged: Span<usize>,
    target: PortId,
    sources: &'a [EventSource],
    sinks: &'a [EventSink],
) -> impl Iterator<Item = (EventPayload, LogSource)> + 'a {
    let frame = externals.get(staged).unwrap_or(&[]);
    let start = frame.partition_point(|e| e.target < target.0);
    let end = frame.partition_point(|e| e.target <= target.0);
    let outside = frame.get(start..end).unwrap_or(&[]).iter().map(|e| {
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
