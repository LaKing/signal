//! The executor (ADR 0016), run in blocks of *k* frames (ADR 0019 §1):
//! instantiation, the frame phases, event delivery in canonical order,
//! parameter bases, suspension, the fault guard, plan swaps and incremental
//! input (ADR 0006). A block's output is bit-identical to running its frames
//! one at a time. Normative description: `docs/semantics.md` (§13 for blocks).
//!
//! Every buffer is sized in [`Engine::new`] or [`Engine::prepare_swap`];
//! [`Engine::run_block`], [`Engine::commit_swap`], the `push_*` methods and
//! [`Engine::drain_log`] allocate, free, lock, perform I/O and panic nowhere
//! (`docs/realtime-rules.md`).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::mem;
use std::ops::Range as Span;
use std::sync::atomic::{AtomicU64, Ordering};

use signal_core::{
    Combine, DirectPrimitive, Direction, Event, EventPayload, EventSink, Feedthrough, Frame,
    Inputs, InstanceAddress, ModuleDescriptor, Outputs, PortAddress, PortKind, PortShape,
    PrepareContext, Primitive, Range, RateDivisor, Registry, RngStream, SinkKind,
    StateOutputPrimitive,
};
use signal_graph::ExecutionPlan;

use crate::cores::performance_cores;
use crate::error::EngineError;
use crate::fade::{FAULT_FADE_FRAMES, Fade, fade_frames, fade_in, fade_out};
use crate::log::{
    DiscardReason, FaultReason, InstanceId, LineSource, LogLine, LogRecord, LogSource, PortId,
};
use crate::parameter::{Applied, Base, effective};
use crate::pool::{Pool, Workers};
use crate::shared::{Access, CallView, HookPoint, Job, Layout, Parts, PlanCells, Reader, Window};
use crate::tasks::{DEFAULT_COST_NS, TASK_MIN_NS, Tasks, tasks};
use crate::units::{Unit, Units, units};

/// Events an event output keeps per frame: the contract suite's 16 events per
/// frame. More in one frame is an `event_overflow` fault at that frame.
const EVENTS_PER_FRAME: usize = 16;

/// The staged external events one task's log has room for in a parallel
/// segment: the block capacity *K*.
fn task_spare(capacity: usize) -> usize {
    capacity
}

/// The fault guard trips on an output value whose magnitude exceeds this.
const FAULT_MAGNITUDE: f64 = 1e6;

/// The next engine's identity: a prepared swap commits only to its own engine.
static NEXT_ENGINE_ID: AtomicU64 = AtomicU64::new(0);

/// The most threads an engine runs a block on.
const MAX_THREADS: usize = 1024;

/// When a segment runs on several threads (ADR 0019 §2): thresholds that
/// change the timing, never the output. Documented in `docs/benchmarks.md`.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Schedule {
    /// A task takes further units while its estimate is below this, in ns
    /// per 256-frame block (0: one task per unit).
    pub task_min_ns: u64,
    /// A segment runs on the workers only if the plan's estimate times its
    /// frames reaches this, in ns.
    pub parallel_min_ns: u64,
    /// A segment runs on the workers only if it has at least this many frames.
    pub min_frames: usize,
}

impl Schedule {
    /// The documented thresholds.
    pub const DEFAULT: Schedule = Schedule {
        task_min_ns: TASK_MIN_NS,
        parallel_min_ns: 50_000,
        min_frames: 16,
    };

    /// Every segment of every plan with two tasks or more on the workers,
    /// one task per unit: for tests and the bench.
    pub const FORCED: Schedule = Schedule {
        task_min_ns: 0,
        parallel_min_ns: 0,
        min_frames: 1,
    };
}

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
    /// The most frames [`Engine::run_block`] runs at once, and the `max_frames`
    /// every primitive is prepared with. A performance setting: it never
    /// changes the output.
    pub block_capacity: usize,
    /// The threads a block runs on, the calling thread included: 1 runs no
    /// workers, 0 means the machine's performance cores
    /// ([`performance_cores`](crate::performance_cores), read once; one
    /// fewer in real time, [`realtime_threads`](crate::realtime_threads)). A
    /// machine setting: it never changes the output, and never appears in a
    /// project. [`Engine::config`] reports the resolved number.
    pub threads: usize,
    /// When segments run on the workers (tests and the bench).
    #[doc(hidden)]
    pub schedule: Schedule,
}

impl EngineConfig {
    /// The default [`queue_capacity`](EngineConfig::queue_capacity).
    pub const DEFAULT_QUEUE_CAPACITY: usize = 4096;

    /// The default [`block_capacity`](EngineConfig::block_capacity).
    pub const DEFAULT_BLOCK_CAPACITY: usize = 256;

    /// A configuration with the default queue and block capacities.
    pub fn new(sample_rate: u32, seed: u64) -> EngineConfig {
        EngineConfig {
            sample_rate,
            seed,
            queue_capacity: EngineConfig::DEFAULT_QUEUE_CAPACITY,
            block_capacity: EngineConfig::DEFAULT_BLOCK_CAPACITY,
            threads: 1,
            schedule: Schedule::DEFAULT,
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
pub(crate) struct Slot {
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
    /// The events delivered to its event inputs in the current call, at
    /// in-call offsets, and where each input's end in them.
    events: Vec<Event>,
    ends: Vec<usize>,
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
pub(crate) struct Param {
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

/// An external event of the current block, resolved to its target port.
#[derive(Clone, Copy, Debug)]
struct Staged {
    target: usize,
    /// The frame it is delivered at, as an index into the block.
    frame: usize,
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

/// One thread's scratch for instance calls: one call's inputs and outputs,
/// port-major for its frames, a parameter's modulation per block frame, and
/// what the fault guard's rollback restores.
#[derive(Debug, Default)]
pub(crate) struct Lane {
    call_signals: Vec<f64>,
    call_params: Vec<f64>,
    call_outputs: Vec<f64>,
    modulation: Vec<f64>,
    saved_params: Vec<(Base, Option<f64>)>,
    saved: Saved,
}

/// Where a block's records go: the plan's log, or one task's in a parallel
/// segment, and the counters of what did not fit.
#[derive(Debug, Default)]
pub(crate) struct Out {
    log: Vec<LogRecord>,
    log_overflows: u64,
    input_overflows: u64,
}

impl Out {
    /// Empties the log and the counters; allocates nothing.
    pub(crate) fn clear(&mut self) {
        self.log.clear();
        self.log_overflows = 0;
        self.input_overflows = 0;
    }
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
    /// The strongly connected components, in execution order (ADR 0019 §1).
    units: Units,
    /// The units grouped into tasks for the workers (ADR 0019 §2).
    tasks: Tasks,
    last_edit: Vec<Option<(EditKind, Frame)>>,

    /// The block capacity *K*: the stride of every per-frame buffer.
    capacity: usize,
    /// Summed signal inputs of the last frame.
    signal_in: Vec<f64>,
    signal_sources: Vec<Vec<(usize, f64)>>,
    /// What readers saw of every signal output in the last frame.
    outputs: Vec<f64>,
    output_audio: Vec<bool>,
    output_ports: Vec<PortId>,
    sink_ports: Vec<PortId>,
    event_inputs: Vec<EventInput>,
    /// Effective values of the last frame.
    param_values: Vec<f64>,
    /// Every per-instance buffer the units write: slots, primitives,
    /// parameters, the block's outputs, inputs and events (ADR 0020).
    cells: PlanCells,
    /// The block's log and the counters of what did not fit.
    out: Out,
    /// Scratch for [`Engine::drain_log`]'s canonical order, as large as the log.
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
                        max_frames: config.block_capacity,
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
                        sinks.push(EventSink::per_frame(
                            EVENTS_PER_FRAME,
                            config.block_capacity,
                        ));
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
                // Sized below, once every connection is known.
                events: Vec::new(),
                ends: Vec::new(),
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
        for &(from, to) in &edges {
            let direct = slots
                .get(from)
                .is_some_and(|s| s.feedthrough == Feedthrough::Direct);
            if direct && position.get(from) >= position.get(to) {
                return Err(EngineError::InvalidSchedule);
            }
        }
        let direct: Vec<bool> = slots
            .iter()
            .map(|s| s.feedthrough == Feedthrough::Direct)
            .collect();
        let units = units(slots.len(), &edges, &position, &direct);

        // Buffer bounds, over any K consecutive frames (K the block capacity).
        // X(t): the most up-front externals stamped for target t of this plan;
        // X_tot: the most stamped for this plan's targets in total.
        let capacity = config.block_capacity.max(1);
        let window = capacity as u64;
        let mut stamped: Vec<(usize, u64)> = upfront
            .iter()
            .filter_map(|e| Some((ports.binary_search(&e.target).ok()?, e.frame.0)))
            .collect();
        stamped.sort_unstable();
        let mut most_per_target = vec![0usize; ports.len()];
        for group in stamped.chunk_by(|a, b| a.0 == b.0) {
            let frames: Vec<u64> = group.iter().map(|&(_, frame)| frame).collect();
            if let Some(most) = group.first().and_then(|g| most_per_target.get_mut(g.0)) {
                *most = most_within(&frames, window);
            }
        }
        let mut frames: Vec<u64> = stamped.iter().map(|&(_, frame)| frame).collect();
        frames.sort_unstable();
        let most_in_total = most_within(&frames, window);
        // Per instance: Σ over its event inputs t of X(t) + 16·K · (t's event
        // connections, duplicates included), plus the push queue's capacity Q
        // when it has event inputs: all its pushed events of a block come
        // from that queue. A sink keeps at most 16 events per frame, and each
        // connection delivers each once, so a block never exceeds it.
        for slot in &mut slots {
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
                        + EVENTS_PER_FRAME * capacity * connections(&input.sources)
                })
                .sum::<usize>()
                + pushed;
            slot.events = Vec::with_capacity(capacity);
            slot.ends = vec![0; slot.event_in.len()];
        }
        // The log of a block: one swap record, at most one edit per instance
        // and frame, one fault per instance, every staged external (up-front:
        // X_tot, all targets in total; pushed: Q), and 16 events per frame per
        // event connection.
        let event_connections: usize = event_inputs
            .iter()
            .map(|i| connections(&i.sources))
            .sum::<usize>()
            + params.iter().map(|p| connections(&p.events)).sum::<usize>();
        let log_capacity = 1
            + slots.len() * capacity
            + slots.len()
            + most_in_total
            + config.queue_capacity
            + EVENTS_PER_FRAME * capacity * event_connections;
        let log = Vec::with_capacity(log_capacity);
        let log_order = Vec::with_capacity(log.capacity());

        // The tasks (ADR 0019 §2), from each instance's type's static cost.
        let cost_ns: Vec<f64> = types
            .iter()
            .map(|(name, _)| f64::from(registry.cost(name).unwrap_or(DEFAULT_COST_NS)))
            .collect();
        let tasks = tasks(&units, &cost_ns, slots.len(), config.schedule.task_min_ns);

        // One lane per thread: one call's buffers (the most ports and
        // parameters of any instance) and a parameter's modulation per frame.
        let threads = config.threads.clamp(1, MAX_THREADS);
        let most = |len: fn(&Slot) -> usize| slots.iter().map(len).max().unwrap_or(0);
        let lanes = (0..threads)
            .map(|_| Lane {
                call_signals: vec![0.0; most(|s| s.signal_in.len()) * capacity],
                call_params: vec![0.0; most(|s| s.params.len()) * capacity],
                call_outputs: vec![0.0; most(|s| s.signal_out.len()) * capacity],
                modulation: vec![0.0; capacity],
                saved_params: Vec::with_capacity(most(|s| s.params.len())),
                saved: Saved::default(),
            })
            .collect();
        // One log per task when there are workers: each instance's fault and
        // 16 events per frame per event connection into it, plus
        // `task_spare(K)` staged externals (a segment with more for one task
        // runs on the calling thread, `Engine::parallel`).
        let mut outs = Vec::new();
        if threads > 1 {
            let mut bound = vec![task_spare(capacity); tasks.len()];
            for (i, slot) in slots.iter().enumerate() {
                let inputs = event_inputs.get(slot.event_in.clone()).unwrap_or(&[]);
                let targets = params.get(slot.params.clone()).unwrap_or(&[]);
                let into = inputs
                    .iter()
                    .map(|i| connections(&i.sources))
                    .sum::<usize>()
                    + targets
                        .iter()
                        .map(|p| connections(&p.events))
                        .sum::<usize>();
                let t = tasks.instance_task.get(i).copied().unwrap_or(0) as usize;
                if let Some(b) = bound.get_mut(t) {
                    *b += 1 + EVENTS_PER_FRAME * capacity * into;
                }
            }
            outs = bound
                .into_iter()
                .map(|b| Out {
                    log: Vec::with_capacity(b),
                    ..Out::default()
                })
                .collect();
        }

        let owners = |range: fn(&Slot) -> Span<usize>, len: usize| {
            let mut owner = vec![0u32; len];
            for (i, slot) in slots.iter().enumerate() {
                for o in range(slot) {
                    if let Some(w) = owner.get_mut(o) {
                        *w = i as u32;
                    }
                }
            }
            owner
        };
        let outputs = output_audio.len();
        let output_owner = owners(|s| s.signal_out.clone(), outputs);
        let sink_owner = owners(|s| s.event_out.clone(), sinks.len());
        let cells = PlanCells::new(Parts {
            capacity,
            layout: slots
                .iter()
                .map(|s| Layout {
                    signal_in: s.signal_in.clone(),
                    signal_out: s.signal_out.clone(),
                    event_out: s.event_out.clone(),
                    params: s.params.clone(),
                })
                .collect(),
            instance_task: tasks.instance_task.clone(),
            primitives,
            outputs: vec![0.0; outputs * capacity],
            output_owner,
            held: vec![0.0; outputs],
            signal_in: vec![0.0; signal_inputs * capacity],
            param_rows: vec![0.0; param_values.len() * capacity],
            emitted: (0..sinks.len())
                .map(|_| Vec::with_capacity(EVENTS_PER_FRAME * capacity))
                .collect(),
            sinks,
            sink_owner,
            params,
            slots,
            lanes,
            outs,
            preds: tasks.preds.clone(),
            successors: tasks.successors.clone(),
        });

        let state = PlanState {
            last_edit: vec![None; instances.len()],
            instances,
            instance_ids,
            types,
            ports,
            roles,
            units,
            tasks,
            capacity,
            signal_in: vec![0.0; signal_inputs],
            signal_sources,
            outputs: vec![0.0; outputs],
            output_audio,
            output_ports,
            sink_ports,
            event_inputs,
            param_values,
            cells,
            out: Out {
                log,
                ..Out::default()
            },
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
        for primitive in state.cells.primitives.get_mut() {
            *primitive = None;
        }
        Ok(state)
    }

    /// The audio sink: its address and number of signal inputs, if the plan
    /// has exactly one.
    pub(crate) fn audio_sink(&self) -> Option<(&InstanceAddress, usize)> {
        let mut sinks = self
            .cells
            .slots
            .get()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.audio_sink);
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
    /// The address of parameter `j` (in [`Engine::parameter_at`] order).
    pub(crate) fn parameter_port(&self, j: usize) -> Option<&PortAddress> {
        self.ports.get(self.cells.params.get().get(j)?.port.0)
    }

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
        let Some(slot) = self.cells.slots.get_mut().get_mut(i) else {
            return;
        };
        let held = self.cells.held.get_mut();
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
                        if let (Some(held), Some(&out)) = (held.get_mut(o), self.outputs.get(o)) {
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
        push_record(&mut self.out.log, log_overflows, record);
    }
}

/// The state a survivor carries from `old` instance `o` into `new` instance
/// `n`: its primitive (moved), suspension, fade and fault state, outputs as
/// readers see them, held values, event sinks and parameter bases with their
/// ramps. Both have the same descriptor, so their ranges have equal lengths.
fn move_instance(new: &mut PlanState, old: &mut PlanState, n: usize, o: usize) {
    let (nc, oc) = (&mut new.cells, &mut old.cells);
    if let (Some(a), Some(b)) = (
        nc.primitives.get_mut().get_mut(n),
        oc.primitives.get_mut().get_mut(o),
    ) {
        mem::swap(a, b);
    }
    if let (Some(a), Some(&b)) = (new.last_edit.get_mut(n), old.last_edit.get(o)) {
        *a = b;
    }
    let (Some(ns), Some(os)) = (nc.slots.get_mut().get_mut(n), oc.slots.get().get(o)) else {
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
        copy(nc.held.get_mut(), oc.held.get(), a, b);
    }
    for (a, b) in events {
        if let (Some(x), Some(y)) = (nc.sinks.get_mut().get_mut(a), oc.sinks.get_mut().get_mut(b)) {
            mem::swap(x, y);
        }
    }
    for (a, b) in params {
        if let (Some(x), Some(y)) = (nc.params.get_mut().get_mut(a), oc.params.get().get(b)) {
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
    let (old_slots, old_params) = (old.cells.slots.get(), old.cells.params.get());
    let (new_slots, new_params) = (state.cells.slots.get(), state.cells.params.get());
    let mut survives = vec![false; old_slots.len()];
    for &(_, o) in &survivors {
        if let Some(s) = survives.get_mut(o) {
            *s = true;
        }
    }
    let mut owner = vec![usize::MAX; old.outputs.len()];
    for (i, slot) in old_slots.iter().enumerate() {
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
        let (Some(ns), Some(os)) = (new_slots.get(n), old_slots.get(o)) else {
            continue;
        };
        for (nj, oj) in ns.params.clone().zip(os.params.clone()) {
            let (Some(np), Some(op)) = (new_params.get(nj), old_params.get(oj)) else {
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

/// An instantiated [`ExecutionPlan`], run in blocks of frames (ADR 0016,
/// ADR 0019 §1).
///
/// Instances, ports and parameters live in flat buffers sized at construction.
/// Each frame runs external input (a plan swap, edits, then external events),
/// emit, process in schedule order and absorb; a block runs the same frames
/// unit by unit, with the same result. See [`Engine::run_block`].
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
    /// The current block's external events, sorted by (target, frame,
    /// sequence).
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

    /// The first frame of the current (or last) block, and its length.
    block_start: Frame,
    block_frames: usize,

    /// The worker threads (none for one thread), kept across swaps.
    pool: Option<Pool>,
    /// The parallel segments run, which is also each one's job epoch.
    parallel_segments: u64,
    /// The test hook (`set_task_hook`).
    hook: Option<fn(HookPoint)>,
}

/// How an engine schedules its blocks (`Engine::schedule_stats`).
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScheduleStats {
    /// The threads a block may run on, the calling thread included.
    pub threads: usize,
    /// The worker threads running.
    pub workers: usize,
    /// The running plan's units.
    pub units: usize,
    /// The running plan's tasks.
    pub tasks: usize,
    /// The running plan's estimated cost per frame, in ns (rounded): what a
    /// segment's frames multiply against `Schedule::parallel_min_ns`.
    pub estimate_ns: u64,
    /// Segments run on the workers.
    pub parallel_segments: u64,
    /// Tasks the workers ran (not the calling thread).
    pub worker_tasks: u64,
    /// Tasks whose body unwound on a worker; that worker is gone.
    pub worker_panics: u64,
}

/// What a block call's gather changes besides parameter bases, saved so that
/// a fault mid-call can undo the frames after it.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Saved {
    log_len: usize,
    log_overflows: u64,
    input_overflows: u64,
    discards: u64,
    suspension_discards: u64,
}

impl Engine {
    /// Instantiates `plan`: creates every primitive through `registry`, prepares
    /// it with the sample rate, the block capacity and
    /// `RngStream::derive(seed, address)`, and allocates every buffer the
    /// blocks need.
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
        if config.block_capacity == 0 {
            return Err(EngineError::InvalidBlockCapacity);
        }
        // Auto: the performance cores, read once per process.
        let threads = match config.threads {
            0 => performance_cores().0,
            n => n,
        }
        .clamp(1, MAX_THREADS);
        let config = EngineConfig { threads, ..config };
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
        // The most up-front events of any K consecutive frames, all targets
        // in total: what one block stages.
        let frames: Vec<u64> = upfront.iter().map(|e| e.frame.0).collect();
        let most_per_block = most_within(&frames, config.block_capacity as u64);

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
            staged: Vec::with_capacity(most_per_block + config.queue_capacity),
            scheduled: Vec::new(),
            scheduled_next: 0,
            pushed_edits: VecDeque::with_capacity(config.queue_capacity),
            edits_consumed: 0,
            last_pushed_edit_at: Frame::ZERO,
            swap_record: None,
            discards: InputDiscards::default(),
            log_overflows: 0,
            input_overflows: 0,
            block_start: Frame::ZERO,
            block_frames: 0,
            pool: (threads > 1).then(|| Pool::new(threads - 1)),
            parallel_segments: 0,
            hook: None,
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
        if !self.plan.out.log.is_empty() {
            return Err(EngineError::LogNotDrained);
        }
        let (new, old) = (&mut swap.plan, &mut self.plan);
        for &(n, o) in &swap.survivors {
            move_instance(new, old, n, o);
        }
        let new_params = new.cells.params.get_mut();
        let old_params = old.cells.params.get();
        for hold in &swap.holds {
            let held = if hold.removed.is_empty() {
                old_params
                    .get(hold.old_param)
                    .and_then(|p| p.held_modulation)
            } else {
                let removed = swap
                    .removed_sources
                    .get(hold.removed.clone())
                    .unwrap_or(&[]);
                Some(sum(removed, &old.outputs))
            };
            if let Some(param) = new_params.get_mut(hold.param) {
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

    /// Runs the next frame: [`run_block(1)`](Engine::run_block).
    pub fn run_frame(&mut self) {
        self.run_block(1);
    }

    /// Runs the next `frames` frames, at most the block capacity, and returns
    /// how many ran. The outputs and the log are bit-identical to running
    /// those frames one at a time, each in the four phases of ADR 0016 §2:
    /// 1. external input: a swap committed for the frame (its record), the
    ///    edits effective at it, then the external events effective at it
    ///    become inputs;
    /// 2. emit: every state-output instance writes its outputs from state;
    /// 3. process: direct instances in schedule order, each after its events
    ///    and parameters for the frame are applied;
    /// 4. absorb: every state-output instance takes its inputs, events and
    ///    parameters for the frame.
    ///
    /// The block runs the plan's units in order (ADR 0019 §1,
    /// `docs/semantics.md` §13): an acyclic direct instance in one `process`
    /// call, a state-output instance and every cycle one frame at a time. It
    /// is split where an edit takes effect; external events are delivered at
    /// their frames inside it. A swap is committed only between blocks, so
    /// callers split at swap frames.
    ///
    /// Allocates nothing. The block's records are in
    /// [`frame_log`](Engine::frame_log) until the next call, or until
    /// [`drain_log`](Engine::drain_log).
    pub fn run_block(&mut self, frames: usize) -> usize {
        let k = frames.min(self.plan.capacity);
        if k == 0 {
            return 0;
        }
        let start = self.next_frame;
        self.block_start = start;
        self.block_frames = k;
        self.plan.out.log.clear();
        if let Some((version, plan_version)) = self.swap_record.take() {
            let record = LogRecord::Swap {
                frame: start,
                version,
                plan_version,
            };
            push_record(&mut self.plan.out.log, &mut self.log_overflows, record);
        }

        self.stage_externals(start, k);
        for events in self.plan.cells.emitted.get_mut() {
            events.clear();
        }
        // Segments: an edit takes effect only at a segment's first frame.
        let mut a = 0;
        while a < k {
            let frame = Frame(start.0.saturating_add(a as u64));
            self.apply_edits(frame);
            let b = self
                .next_edit_after(frame)
                .and_then(|next| usize::try_from(next.0.saturating_sub(start.0)).ok())
                .map_or(k, |b| b.clamp(a + 1, k));
            self.run_segment(a, b);
            self.refresh_last(b - 1);
            a = b;
        }

        self.next_frame = Frame(start.0.saturating_add(k as u64));
        k
    }

    /// The earliest frame after [`next_frame`](Engine::next_frame) at which
    /// queued input takes effect: a pushed event, a pushed edit or a
    /// scheduled edit. The real-time runner splits its blocks there.
    /// Allocates nothing.
    pub fn next_command_frame(&self) -> Option<Frame> {
        let next = self.next_frame;
        let event = self
            .queue
            .get(self.queue.partition_point(|q| q.at <= next))
            .map(|q| q.at);
        let edit = self
            .pushed_edits
            .get(self.pushed_edits.partition_point(|e| e.at <= next))
            .map(|e| e.at);
        let pending = self.scheduled.get(self.scheduled_next..).unwrap_or(&[]);
        let scheduled = pending
            .get(pending.partition_point(|e| e.frame <= next))
            .map(|e| e.frame);
        [event, edit, scheduled].into_iter().flatten().min()
    }

    /// The frame of the next edit after `frame`, once the edits of `frame`
    /// are applied: where the block is split.
    fn next_edit_after(&self, frame: Frame) -> Option<Frame> {
        let scheduled = self.scheduled.get(self.scheduled_next).map(|e| e.frame);
        let pushed = self.pushed_edits.get(self.edits_consumed).map(|e| e.at);
        [scheduled, pushed]
            .into_iter()
            .flatten()
            .filter(|&f| f > frame)
            .min()
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

    /// The external events effective in the block of `frames` frames from
    /// `start`, up-front then pushed, resolved against the running plan, each
    /// at its effective frame (a late one at the block's first frame), sorted
    /// by (target, frame, sequence).
    fn stage_externals(&mut self, start: Frame, frames: usize) {
        self.staged.clear();
        let end = start.0.saturating_add(frames as u64);
        let index = |at: Frame| usize::try_from(at.0.saturating_sub(start.0)).unwrap_or(0);
        while let Some(event) = self.upfront.get(self.upfront_next) {
            if event.frame.0 >= end {
                break;
            }
            self.upfront_next += 1;
            let ordinal = self.upfront_next as u64;
            let item = resolve(
                &self.plan,
                &mut self.discards,
                event,
                index(event.frame),
                false,
                ordinal,
            );
            push_staged(&mut self.staged, &mut self.input_overflows, item);
        }
        while let Some(queued) = self.queue.get(self.queue_consumed) {
            if queued.at.0 >= end {
                break;
            }
            self.queue_consumed += 1;
            let item = resolve(
                &self.plan,
                &mut self.discards,
                &queued.event,
                index(queued.at),
                true,
                queued.ordinal,
            );
            push_staged(&mut self.staged, &mut self.input_overflows, item);
        }
        self.staged
            .sort_unstable_by_key(|s| (s.target, s.frame, s.sequence, s.pushed, s.ordinal));
    }

    /// Whether a segment of `frames` frames runs on the workers: there are
    /// workers and two tasks or more, the segment is long and costly enough
    /// (`Schedule`), and every task's log has room for its staged external
    /// events. Otherwise the calling thread runs it alone, with the same
    /// result.
    fn parallel(&mut self, frames: usize) -> bool {
        let schedule = self.config.schedule;
        let plan = &mut self.plan;
        if self.pool.is_none()
            || plan.tasks.len() < 2
            || frames < schedule.min_frames
            || plan.tasks.estimate_ns * (frames as f64) < (schedule.parallel_min_ns as f64)
        {
            return false;
        }
        let spare = task_spare(plan.capacity);
        let tasks = &mut plan.tasks;
        tasks.staged.fill(0);
        for staged in &self.staged {
            let Some(&Some((i, _))) = plan.roles.get(staged.target) else {
                continue;
            };
            let t = tasks.instance_task.get(i).copied().unwrap_or(0) as usize;
            if let Some(count) = tasks.staged.get_mut(t) {
                *count += 1;
                if *count > spare {
                    return false;
                }
            }
        }
        true
    }

    /// Frames `a..b` of the block, unit by unit: on the calling thread, or
    /// as tasks on the workers too (ADR 0019 §2). Either way each unit writes
    /// only its own instances' rows and reads its sources' in canonical
    /// order, and the tasks' logs are joined in task order, which is unit
    /// order: the result is the same bit for bit.
    fn run_segment(&mut self, a: usize, b: usize) {
        let parallel = self.parallel(b - a);
        let plan = &mut self.plan;
        let fx = Fixed {
            units: &plan.units,
            signal_sources: &plan.signal_sources,
            event_inputs: &plan.event_inputs,
            outputs: &plan.outputs,
            param_values: &plan.param_values,
            output_audio: &plan.output_audio,
            output_ports: &plan.output_ports,
            sink_ports: &plan.sink_ports,
            staged: &self.staged,
            start: self.block_start,
            fade_len: self.fade_len,
            a,
            b,
        };
        match self.pool.as_mut() {
            Some(pool) if parallel => {
                self.parallel_segments += 1;
                let tasks = &plan.tasks;
                let body = |access: &mut Access<'_>, t: usize| {
                    for u in tasks.units(t) {
                        run_unit(&fx, access, u);
                    }
                };
                let (mut dispatch, stats) = pool.split();
                let job = Job::new(
                    &mut plan.cells,
                    self.parallel_segments,
                    &body,
                    self.hook,
                    stats,
                );
                // The job holds the cells until it is run: no worker is in it
                // once `run` returns.
                dispatch.run(&job);
                let main = &mut plan.out;
                for out in plan.cells.outs.get_mut() {
                    for record in out.log.drain(..) {
                        push_record(&mut main.log, &mut main.log_overflows, record);
                    }
                    main.log_overflows = main.log_overflows.saturating_add(out.log_overflows);
                    main.input_overflows = main.input_overflows.saturating_add(out.input_overflows);
                }
            }
            _ => {
                let c = &mut plan.cells;
                if let Some(lane) = c.lanes.get_mut().first_mut() {
                    let mut whole = Whole {
                        k: plan.capacity,
                        slots: c.slots.get_mut(),
                        primitives: c.primitives.get_mut(),
                        params: c.params.get_mut(),
                        outputs: c.outputs.get_mut(),
                        held: c.held.get_mut(),
                        signal_in: c.signal_in.get_mut(),
                        param_rows: c.param_rows.get_mut(),
                        sinks: c.sinks.get_mut(),
                        emitted: c.emitted.get_mut(),
                        lane,
                        out: &mut plan.out,
                    };
                    for u in 0..fx.units.order.len() {
                        whole_unit(&fx, &mut whole, u);
                    }
                }
            }
        }
        let out = &mut self.plan.out;
        self.log_overflows = self
            .log_overflows
            .saturating_add(mem::take(&mut out.log_overflows));
        self.input_overflows = self
            .input_overflows
            .saturating_add(mem::take(&mut out.input_overflows));
    }

    /// The last frame's values: block frame `f`'s outputs, signal inputs and
    /// effective parameter values, what edits, swaps and the accessors read.
    fn refresh_last(&mut self, f: usize) {
        let plan = &mut self.plan;
        let k = plan.capacity;
        let rows = [
            (&mut plan.outputs, plan.cells.outputs.get()),
            (&mut plan.signal_in, plan.cells.signal_in.get()),
            (&mut plan.param_values, plan.cells.param_rows.get()),
        ];
        for (last, block) in rows {
            for (o, last) in last.iter_mut().enumerate() {
                if let Some(&value) = block.get(o * k + f) {
                    *last = value;
                }
            }
        }
    }

    /// The configuration the engine was built with, `threads` resolved.
    pub fn config(&self) -> EngineConfig {
        self.config
    }

    /// Its worker threads, for a driver to run code on them (ADR 0019 §4).
    pub fn workers(&self) -> Workers {
        match &self.pool {
            Some(pool) => pool.handle(self.config.threads),
            None => Workers::none(self.config.threads),
        }
    }

    /// Calls `hook` when a thread enters and leaves a parallel segment and
    /// before each task it runs (tests: delays, allocation guards).
    #[doc(hidden)]
    pub fn set_task_hook(&mut self, hook: Option<fn(HookPoint)>) {
        self.hook = hook;
    }

    /// How the engine schedules its blocks.
    #[doc(hidden)]
    pub fn schedule_stats(&self) -> ScheduleStats {
        let pool = self.pool.as_ref();
        let stats = pool.map(|p| p.stats());
        let count =
            |c: Option<&std::sync::atomic::AtomicU64>| c.map_or(0, |c| c.load(Ordering::Relaxed));
        ScheduleStats {
            threads: self.config.threads,
            workers: pool.map_or(0, Pool::workers),
            units: self.plan.units.order.len(),
            tasks: self.plan.tasks.len(),
            estimate_ns: self.plan.tasks.estimate_ns.round() as u64,
            parallel_segments: self.parallel_segments,
            worker_tasks: count(stats.map(|s| &s.worker_tasks)),
            worker_panics: count(stats.map(|s| &s.worker_panics)),
        }
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

    /// What the last block did, in execution order (frame by frame after
    /// [`run_frame`](Engine::run_frame)).
    pub fn frame_log(&self) -> &[LogRecord] {
        &self.plan.out.log
    }

    /// Hands the last block's records to `out` frame by frame, each frame's
    /// in canonical order (that of `log.jsonl`, `docs/semantics.md` §9), and
    /// empties the log; allocates nothing. The log holds one block: drain it
    /// after each [`run_block`](Engine::run_block), and before a
    /// [`commit_swap`](Engine::commit_swap), which refuses an undrained log.
    pub fn drain_log(&mut self, out: &mut impl FnMut(&LogRecord)) {
        let plan = &mut self.plan;
        let log = &plan.out.log;
        plan.log_order.clear();
        plan.log_order.extend(0..log.len());
        plan.log_order
            .sort_unstable_by_key(|&k| (log.get(k).map(|r| (r.frame(), r.order())), k));
        for &k in &plan.log_order {
            if let Some(record) = log.get(k) {
                out(record);
            }
        }
        plan.out.log.clear();
    }

    /// Log records dropped because the block's log was full. The buffer is
    /// sized so that this stays 0.
    pub fn log_overflows(&self) -> u64 {
        self.log_overflows
    }

    /// Events dropped, not delivered and not logged, because an instance's
    /// event-input buffer (or an output's block store) was full. The buffers
    /// are sized so that this stays 0.
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
        self.plan
            .cells
            .slots
            .get()
            .get(*self.plan.instance_ids.get(instance)?)
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
            Role::Param(j) => self.parameter_at(j),
            _ => None,
        }
    }

    /// The number of parameters of the running plan.
    pub(crate) fn parameter_count(&self) -> usize {
        self.plan.cells.params.len()
    }

    /// The base and effective value of the running plan's parameter `j`, as
    /// [`parameter`](Engine::parameter) gives them. Allocates nothing.
    pub(crate) fn parameter_at(&self, j: usize) -> Option<ParameterValue> {
        Some(ParameterValue {
            base: self.plan.cells.params.get().get(j)?.base.value(),
            effective: self.plan.param_values.get(j).copied()?,
        })
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
        let slots = self.plan.cells.slots.get();
        (0..slots.len())
            .filter(|&i| slots.get(i).is_some_and(|s| s.audio_sink))
            .map(InstanceId)
            .collect()
    }

    /// The first instance whose descriptor declares an audio sink; allocates
    /// nothing, unlike [`audio_sinks`](Engine::audio_sinks).
    pub(crate) fn first_audio_sink(&self) -> Option<InstanceId> {
        self.plan
            .cells
            .slots
            .get()
            .iter()
            .position(|s| s.audio_sink)
            .map(InstanceId)
    }

    /// The address of `instance`.
    pub(crate) fn instance_address(&self, instance: InstanceId) -> Option<&InstanceAddress> {
        self.plan.instances.get(instance.0)
    }

    /// The frames the last [`run_block`](Engine::run_block) ran.
    pub fn block_frames(&self) -> usize {
        self.block_frames
    }

    /// Signal input `channel` of `instance` (in port order) at frame `f` of
    /// the last block.
    pub(crate) fn instance_input_at(
        &self,
        instance: InstanceId,
        channel: usize,
        f: usize,
    ) -> Option<f64> {
        let slot = self.plan.cells.slots.get().get(instance.0)?;
        if channel >= slot.signal_in.len() {
            return None;
        }
        let s = slot.signal_in.start + channel;
        self.plan
            .cells
            .signal_in
            .get()
            .get(s * self.plan.capacity + f)
            .copied()
    }

    /// The index of parameter `port` in the running plan.
    pub(crate) fn parameter_index(&self, port: &PortAddress) -> Option<usize> {
        match self.role(port)? {
            Role::Param(j) => Some(j),
            _ => None,
        }
    }

    /// The effective value of parameter `j` at frame `f` of the last block, as
    /// [`parameter`](Engine::parameter) gave it after that frame.
    pub(crate) fn parameter_at_frame(&self, j: usize, f: usize) -> Option<f64> {
        self.plan
            .cells
            .param_rows
            .get()
            .get(j * self.plan.capacity + f)
            .copied()
    }

    /// The signal inputs of `instance` in the last frame, in port order.
    pub(crate) fn instance_inputs(&self, instance: InstanceId) -> &[f64] {
        self.plan
            .cells
            .slots
            .get()
            .get(instance.0)
            .and_then(|s| self.plan.signal_in.get(s.signal_in.clone()))
            .unwrap_or(&[])
    }
}

/// What a segment's units read and never write: the plan's structure, the
/// last frame's values, the staged external events and the segment's frames.
#[derive(Clone, Copy)]
struct Fixed<'x> {
    units: &'x Units,
    signal_sources: &'x [Vec<(usize, f64)>],
    event_inputs: &'x [EventInput],
    outputs: &'x [f64],
    param_values: &'x [f64],
    output_audio: &'x [bool],
    output_ports: &'x [PortId],
    sink_ports: &'x [PortId],
    staged: &'x [Staged],
    start: Frame,
    fade_len: u64,
    a: usize,
    b: usize,
}

impl Fixed<'_> {
    fn frame(&self, f: usize) -> Frame {
        Frame(self.start.0.saturating_add(f as u64))
    }
}

/// Unit `u` of the plan over the segment's frames: an acyclic direct
/// instance in one call, a state-output instance and a cycle frame by frame.
fn run_unit(fx: &Fixed<'_>, access: &mut Access<'_>, u: usize) {
    let (a, b) = (fx.a, fx.b);
    let Some(unit) = fx.units.order.get(u) else {
        return;
    };
    match unit {
        &Unit::Direct(i) => run_direct(fx, access, i, a, b),
        &Unit::State(i) => {
            for f in a..b {
                if let Some(mut v) = access.call(i, f, f + 1) {
                    emit(fx, &mut v, i, f);
                    absorb(fx, &mut v, f);
                }
            }
        }
        Unit::Cycle {
            state_outputs,
            direct,
        } => {
            let member = |m: usize| fx.units.members.get(m).copied();
            for f in a..b {
                for i in state_outputs.clone().filter_map(member) {
                    if let Some(mut v) = access.call(i, f, f + 1) {
                        emit(fx, &mut v, i, f);
                    }
                }
                for i in direct.clone().filter_map(member) {
                    run_direct(fx, access, i, f, f + 1);
                }
                for i in state_outputs.clone().filter_map(member) {
                    if let Some(mut v) = access.call(i, f, f + 1) {
                        absorb(fx, &mut v, f);
                    }
                }
            }
        }
    }
}

/// Phase 3 for one direct instance over frames `a..b` of the block: its
/// inputs frame by frame, one `process` call, then its outputs; one view.
fn run_direct(fx: &Fixed<'_>, access: &mut Access<'_>, i: usize, a: usize, b: usize) {
    let Some(mut v) = access.call(i, a, b) else {
        return;
    };
    let len = b - a;
    let rollback = len > 1 && v.slot.mode() == Mode::Running;
    if rollback {
        save(&mut gathering(&mut v));
    }
    match gather(fx, &mut gathering(&mut v), a, b) {
        Mode::Running => {
            let ports = (v.slot.signal_in.len(), v.slot.params.len());
            load_call(v.lane, (&v.signal_in, &v.param_rows), ports, a, len);
            for sink in v.sinks.iter_mut() {
                sink.start_call(len);
            }
            if let Some(Primitive::Direct(module)) = v.primitive.as_mut() {
                process(
                    module.as_mut(),
                    (&*v.slot, &mut *v.lane, &mut *v.sinks),
                    len,
                );
            }
            let fault = commit(fx, &mut running(&mut v), a, b);
            keep_events(&mut running(&mut v), a, b, fault.map(|(n, _, _)| n));
            if let Some(fault) = fault {
                mute(fx, &mut gathering(&mut v), i, (a, b), rollback, fault);
                let mut r = running(&mut v);
                for f in fault.0..b {
                    fade_muted(fx, &mut r, f);
                }
            }
        }
        Mode::Suspended => {
            let mut r = running(&mut v);
            for f in a..b {
                fade_suspended(fx, &mut r, f);
            }
        }
        Mode::Muted => {
            let mut r = running(&mut v);
            for f in a..b {
                fade_muted(fx, &mut r, f);
            }
        }
    }
}

/// Phase 2 for one state-output instance at frame `f` of the block.
fn emit(fx: &Fixed<'_>, v: &mut CallView<'_>, i: usize, f: usize) {
    match v.slot.mode() {
        Mode::Running => {
            for sink in v.sinks.iter_mut() {
                sink.start_call(1);
            }
            if let Some(Primitive::StateOutput(module)) = v.primitive.as_mut() {
                emit_call(module.as_mut(), (&*v.slot, &mut *v.lane, &mut *v.sinks));
            }
            let fault = commit(fx, &mut running(v), f, f + 1);
            keep_events(&mut running(v), f, f + 1, fault.map(|(n, _, _)| n));
            if let Some(fault) = fault {
                mute(fx, &mut gathering(v), i, (f, f + 1), false, fault);
                fade_muted(fx, &mut running(v), f);
            }
        }
        Mode::Suspended => fade_suspended(fx, &mut running(v), f),
        Mode::Muted => fade_muted(fx, &mut running(v), f),
    }
}

/// Phase 4 for one state-output instance at frame `f` of the block.
fn absorb(fx: &Fixed<'_>, v: &mut CallView<'_>, f: usize) {
    if gather(fx, &mut gathering(v), f, f + 1) != Mode::Running {
        return;
    }
    let ports = (v.slot.signal_in.len(), v.slot.params.len());
    load_call(v.lane, (&v.signal_in, &v.param_rows), ports, f, 1);
    if let Some(Primitive::StateOutput(module)) = v.primitive.as_mut() {
        absorb_call(module.as_mut(), (&*v.slot, &*v.lane));
    }
}

/// A view's parts for [`gather`].
#[inline(always)]
fn gathering<'x, 't>(
    v: &'x mut CallView<'t>,
) -> Gather<'x, &'x mut Window<'t>, ViewSources<'x, 't>, ViewEvents<'x, 't>> {
    let (own, own_sinks) = (v.slot.signal_out.clone(), v.slot.event_out.clone());
    Gather {
        slot: &mut *v.slot,
        params: &mut *v.params,
        signal_in: &mut v.signal_in,
        param_rows: &mut v.param_rows,
        lane: &mut *v.lane,
        out: &mut *v.out,
        sources: ViewSources {
            reader: &v.sources,
            window: &v.outputs,
            own,
        },
        events: ViewEvents {
            reader: &v.events,
            sinks: &*v.emitted,
            own: own_sinks,
        },
    }
}

/// A view's parts for the rest of a call.
#[inline(always)]
fn running<'x, 't>(v: &'x mut CallView<'t>) -> Run<'x, &'x mut Window<'t>> {
    Run {
        slot: &mut *v.slot,
        lane: &mut *v.lane,
        sinks: &mut *v.sinks,
        emitted: &mut *v.emitted,
        outputs: &mut v.outputs,
        held: &mut *v.held,
        out: &mut *v.out,
    }
}

/// One `process` call of a direct instance over `len` frames, from its
/// lane's call buffers: its gathered inputs (`load_call`) and events, into
/// its sinks (their call started).
#[inline(always)]
fn process(
    module: &mut dyn DirectPrimitive,
    (slot, lane, sinks): (&Slot, &mut Lane, &mut [EventSink]),
    len: usize,
) {
    let (signal_in, out, params) = (
        slot.signal_in.len(),
        slot.signal_out.len(),
        slot.params.len(),
    );
    let inputs = Inputs::new(
        len,
        lane.call_signals.get(..signal_in * len).unwrap_or(&[]),
        lane.call_params.get(..params * len).unwrap_or(&[]),
        &slot.events,
        &slot.ends,
    );
    let mut outputs = Outputs::new(
        len,
        lane.call_outputs.get_mut(..out * len).unwrap_or(&mut []),
        sinks,
    );
    module.process(&inputs, &mut outputs);
}

/// One `emit` call of a state-output instance (one frame), into its sinks
/// (their call started).
#[inline(always)]
fn emit_call(
    module: &mut dyn StateOutputPrimitive,
    (slot, lane, sinks): (&Slot, &mut Lane, &mut [EventSink]),
) {
    let out = slot.signal_out.len();
    let mut outputs = Outputs::new(
        1,
        lane.call_outputs.get_mut(..out).unwrap_or(&mut []),
        sinks,
    );
    module.emit(&mut outputs);
}

/// One `absorb` call of a state-output instance (one frame), from its
/// lane's call buffers and events.
#[inline(always)]
fn absorb_call(module: &mut dyn StateOutputPrimitive, (slot, lane): (&Slot, &Lane)) {
    let (signal_in, params) = (slot.signal_in.len(), slot.params.len());
    let inputs = Inputs::new(
        1,
        lane.call_signals.get(..signal_in).unwrap_or(&[]),
        lane.call_params.get(..params).unwrap_or(&[]),
        &slot.events,
        &slot.ends,
    );
    module.absorb(&inputs);
}

/// Copies the instance's summed signal inputs and effective parameter values
/// for block frames `a..a + len` into its lane's call buffers, port-major;
/// `ports` is its (signal inputs, parameters).
#[inline(always)]
fn load_call<R: Own>(
    lane: &mut Lane,
    (signal_in, param_rows): (&R, &R),
    ports: (usize, usize),
    a: usize,
    len: usize,
) {
    let pairs = [
        (ports.0, signal_in, &mut lane.call_signals),
        (ports.1, param_rows, &mut lane.call_params),
    ];
    for (count, rows, call) in pairs {
        for p in 0..count {
            let from = rows.frames(p, (a, a + len));
            match (call.get_mut(p * len..(p + 1) * len), from) {
                // One frame (k = 1, state units, cycles): a store, not a call
                // to `memcpy` per port.
                (Some([to]), Some([from])) => *to = *from,
                (Some(to), Some(from)) => to.copy_from_slice(from),
                _ => {}
            }
        }
    }
}

/// The plan's buffers for a segment on the calling thread alone (ADR 0019
/// §2, M2.6-2): taken whole once per segment and indexed by instance as
/// M2.6-1 did, without views, readers or claims. The bodies ([`gather`],
/// [`commit`], the fades) are the views' own; only how an instance's parts
/// are reached differs.
struct Whole<'p> {
    k: usize,
    slots: &'p mut [Slot],
    primitives: &'p mut [Option<Primitive>],
    params: &'p mut [Param],
    outputs: &'p mut [f64],
    held: &'p mut [f64],
    signal_in: &'p mut [f64],
    param_rows: &'p mut [f64],
    sinks: &'p mut [EventSink],
    emitted: &'p mut [Vec<Event>],
    lane: &'p mut Lane,
    out: &'p mut Out,
}

impl Whole<'_> {
    /// Instance `i`'s parts for [`gather`].
    #[inline(always)]
    fn gathering(
        &mut self,
        i: usize,
    ) -> Option<Gather<'_, PlanRows<'_>, PlanSources<'_>, &[Vec<Event>]>> {
        let k = self.k;
        let slot = self.slots.get_mut(i)?;
        let (signal_in, params) = (slot.signal_in.start, slot.params.clone());
        Some(Gather {
            params: self.params.get_mut(params.clone())?,
            slot,
            signal_in: PlanRows {
                rows: &mut *self.signal_in,
                first: signal_in,
                k,
            },
            param_rows: PlanRows {
                rows: &mut *self.param_rows,
                first: params.start,
                k,
            },
            lane: &mut *self.lane,
            out: &mut *self.out,
            sources: PlanSources {
                rows: &*self.outputs,
                k,
            },
            events: &*self.emitted,
        })
    }

    /// Instance `i`'s parts for the rest of a call.
    #[inline(always)]
    fn running(&mut self, i: usize) -> Option<Run<'_, PlanRows<'_>>> {
        let slot = self.slots.get_mut(i)?;
        let (out, events) = (slot.signal_out.clone(), slot.event_out.clone());
        Some(Run {
            sinks: self.sinks.get_mut(events.clone())?,
            emitted: self.emitted.get_mut(events)?,
            held: self.held.get_mut(out.clone())?,
            slot,
            lane: &mut *self.lane,
            outputs: PlanRows {
                rows: &mut *self.outputs,
                first: out.start,
                k: self.k,
            },
            out: &mut *self.out,
        })
    }

    /// Copies instance `i`'s inputs for frames `a..a + len` into the lane.
    #[inline(always)]
    fn load_call(&mut self, i: usize, a: usize, len: usize) {
        let Some(slot) = self.slots.get(i) else {
            return;
        };
        let k = self.k;
        let signal_in = PlanRows {
            rows: &mut *self.signal_in,
            first: slot.signal_in.start,
            k,
        };
        let param_rows = PlanRows {
            rows: &mut *self.param_rows,
            first: slot.params.start,
            k,
        };
        let ports = (slot.signal_in.len(), slot.params.len());
        load_call(self.lane, (&signal_in, &param_rows), ports, a, len);
    }

    /// The fault guard after instance `i`'s call over `a..b` faulted.
    fn mute(
        &mut self,
        fx: &Fixed<'_>,
        i: usize,
        (a, b): (usize, usize),
        rollback: bool,
        fault: (usize, PortId, FaultReason),
    ) {
        if let Some(mut g) = self.gathering(i) {
            mute(fx, &mut g, i, (a, b), rollback, fault);
        }
        if let Some(mut r) = self.running(i) {
            for f in fault.0..b {
                fade_muted(fx, &mut r, f);
            }
        }
    }
}

/// [`run_unit`] for a sequential segment.
fn whole_unit(fx: &Fixed<'_>, w: &mut Whole<'_>, u: usize) {
    let (a, b) = (fx.a, fx.b);
    let Some(unit) = fx.units.order.get(u) else {
        return;
    };
    match unit {
        &Unit::Direct(i) => whole_direct(fx, w, i, a, b),
        &Unit::State(i) => {
            for f in a..b {
                whole_emit(fx, w, i, f);
                whole_absorb(fx, w, i, f);
            }
        }
        Unit::Cycle {
            state_outputs,
            direct,
        } => {
            let member = |m: usize| fx.units.members.get(m).copied();
            for f in a..b {
                for i in state_outputs.clone().filter_map(member) {
                    whole_emit(fx, w, i, f);
                }
                for i in direct.clone().filter_map(member) {
                    whole_direct(fx, w, i, f, f + 1);
                }
                for i in state_outputs.clone().filter_map(member) {
                    whole_absorb(fx, w, i, f);
                }
            }
        }
    }
}

/// [`run_direct`] for a sequential segment.
fn whole_direct(fx: &Fixed<'_>, w: &mut Whole<'_>, i: usize, a: usize, b: usize) {
    let len = b - a;
    let rollback = len > 1 && w.slots.get(i).is_some_and(|s| s.mode() == Mode::Running);
    let mode = {
        let Some(mut g) = w.gathering(i) else {
            return;
        };
        if rollback {
            save(&mut g);
        }
        gather(fx, &mut g, a, b)
    };
    match mode {
        Mode::Running => {
            w.load_call(i, a, len);
            let Some(slot) = w.slots.get(i) else {
                return;
            };
            let sinks = w.sinks.get_mut(slot.event_out.clone()).unwrap_or(&mut []);
            for sink in sinks.iter_mut() {
                sink.start_call(len);
            }
            if let Some(Some(Primitive::Direct(module))) = w.primitives.get_mut(i) {
                process(module.as_mut(), (slot, &mut *w.lane, sinks), len);
            }
            let fault = {
                let Some(mut r) = w.running(i) else {
                    return;
                };
                let fault = commit(fx, &mut r, a, b);
                keep_events(&mut r, a, b, fault.map(|(n, _, _)| n));
                fault
            };
            if let Some(fault) = fault {
                w.mute(fx, i, (a, b), rollback, fault);
            }
        }
        Mode::Suspended => {
            if let Some(mut r) = w.running(i) {
                for f in a..b {
                    fade_suspended(fx, &mut r, f);
                }
            }
        }
        Mode::Muted => {
            if let Some(mut r) = w.running(i) {
                for f in a..b {
                    fade_muted(fx, &mut r, f);
                }
            }
        }
    }
}

/// [`emit`] for a sequential segment.
fn whole_emit(fx: &Fixed<'_>, w: &mut Whole<'_>, i: usize, f: usize) {
    let Some(slot) = w.slots.get(i) else {
        return;
    };
    match slot.mode() {
        Mode::Running => {
            let sinks = w.sinks.get_mut(slot.event_out.clone()).unwrap_or(&mut []);
            for sink in sinks.iter_mut() {
                sink.start_call(1);
            }
            if let Some(Some(Primitive::StateOutput(module))) = w.primitives.get_mut(i) {
                emit_call(module.as_mut(), (slot, &mut *w.lane, sinks));
            }
            let fault = {
                let Some(mut r) = w.running(i) else {
                    return;
                };
                let fault = commit(fx, &mut r, f, f + 1);
                keep_events(&mut r, f, f + 1, fault.map(|(n, _, _)| n));
                fault
            };
            if let Some(fault) = fault {
                w.mute(fx, i, (f, f + 1), false, fault);
            }
        }
        Mode::Suspended => {
            if let Some(mut r) = w.running(i) {
                fade_suspended(fx, &mut r, f);
            }
        }
        Mode::Muted => {
            if let Some(mut r) = w.running(i) {
                fade_muted(fx, &mut r, f);
            }
        }
    }
}

/// [`absorb`] for a sequential segment.
fn whole_absorb(fx: &Fixed<'_>, w: &mut Whole<'_>, i: usize, f: usize) {
    let mode = {
        let Some(mut g) = w.gathering(i) else {
            return;
        };
        gather(fx, &mut g, f, f + 1)
    };
    if mode != Mode::Running {
        return;
    }
    w.load_call(i, f, 1);
    if let (Some(slot), Some(Some(Primitive::StateOutput(module)))) =
        (w.slots.get(i), w.primitives.get_mut(i))
    {
        absorb_call(module.as_mut(), (slot, &*w.lane));
    }
}

/// An instance's own rows of one per-frame block buffer: its `p`-th port at
/// block frame `f`. A view's [`Window`] on a task, or the plan's buffer
/// itself ([`PlanRows`]) in a segment on the calling thread alone.
trait Own {
    fn get(&self, p: usize, f: usize) -> Option<f64>;
    fn set(&mut self, p: usize, f: usize, value: f64);
    fn frames(&self, p: usize, frames: (usize, usize)) -> Option<&[f64]>;
    fn frames_mut(&mut self, p: usize, frames: (usize, usize)) -> Option<&mut [f64]>;
}

impl Own for Window<'_> {
    #[inline(always)]
    fn get(&self, p: usize, f: usize) -> Option<f64> {
        Window::get(self, p, f)
    }
    #[inline(always)]
    fn set(&mut self, p: usize, f: usize, value: f64) {
        Window::set(self, p, f, value);
    }
    #[inline(always)]
    fn frames(&self, p: usize, frames: (usize, usize)) -> Option<&[f64]> {
        Window::frames(self, p, frames)
    }
    #[inline(always)]
    fn frames_mut(&mut self, p: usize, frames: (usize, usize)) -> Option<&mut [f64]> {
        Window::frames_mut(self, p, frames)
    }
}

impl<W: Own> Own for &mut W {
    #[inline(always)]
    fn get(&self, p: usize, f: usize) -> Option<f64> {
        (**self).get(p, f)
    }
    #[inline(always)]
    fn set(&mut self, p: usize, f: usize, value: f64) {
        (**self).set(p, f, value);
    }
    #[inline(always)]
    fn frames(&self, p: usize, frames: (usize, usize)) -> Option<&[f64]> {
        (**self).frames(p, frames)
    }
    #[inline(always)]
    fn frames_mut(&mut self, p: usize, frames: (usize, usize)) -> Option<&mut [f64]> {
        (**self).frames_mut(p, frames)
    }
}

/// An instance's rows of a buffer of the whole plan (stride `k`), from row
/// `first`: what a sequential segment indexes, as M2.6-1 did.
struct PlanRows<'b> {
    rows: &'b mut [f64],
    first: usize,
    k: usize,
}

impl Own for PlanRows<'_> {
    #[inline(always)]
    fn get(&self, p: usize, f: usize) -> Option<f64> {
        self.rows.get((self.first + p) * self.k + f).copied()
    }
    #[inline(always)]
    fn set(&mut self, p: usize, f: usize, value: f64) {
        if let Some(x) = self.rows.get_mut((self.first + p) * self.k + f) {
            *x = value;
        }
    }
    #[inline(always)]
    fn frames(&self, p: usize, (a, b): (usize, usize)) -> Option<&[f64]> {
        let row = (self.first + p) * self.k;
        self.rows.get(row + a..row + b)
    }
    #[inline(always)]
    fn frames_mut(&mut self, p: usize, (a, b): (usize, usize)) -> Option<&mut [f64]> {
        let row = (self.first + p) * self.k;
        self.rows.get_mut(row + a..row + b)
    }
}

/// Where a call reads the outputs of its signal sources: output `o` at block
/// frames `a..b`, handed to `read` (`None` if it cannot be read).
trait Sources {
    fn read<T>(
        &self,
        o: usize,
        frames: (usize, usize),
        read: impl FnOnce(Option<&[f64]>) -> T,
    ) -> T;
}

/// A view's sources: other instances' outputs through its reader, its own
/// (a state-output instance reading itself) in its window; `own` are its
/// output indices.
struct ViewSources<'v, 't> {
    reader: &'v Reader<'t, f64>,
    window: &'v Window<'t>,
    own: Span<usize>,
}

impl Sources for ViewSources<'_, '_> {
    #[inline(always)]
    fn read<T>(
        &self,
        o: usize,
        frames: (usize, usize),
        read: impl FnOnce(Option<&[f64]>) -> T,
    ) -> T {
        if self.own.contains(&o) {
            read(self.window.frames(o - self.own.start, frames))
        } else {
            let row = self.reader.frames(o, frames);
            read(row.as_deref())
        }
    }
}

/// A sequential segment's sources: every output row of the plan, stride `k`.
struct PlanSources<'v> {
    rows: &'v [f64],
    k: usize,
}

impl Sources for PlanSources<'_> {
    #[inline(always)]
    fn read<T>(
        &self,
        o: usize,
        (a, b): (usize, usize),
        read: impl FnOnce(Option<&[f64]>) -> T,
    ) -> T {
        read(self.rows.get(o * self.k + a..o * self.k + b))
    }
}

/// Where a call reads the events its sources emitted: sink `sink`'s, handed
/// to `read`; `None` if they cannot be read.
trait Emitted {
    fn read<T>(&self, sink: usize, read: impl FnOnce(&[Event]) -> T) -> Option<T>;
}

/// A view's: other instances' sinks through its reader, its own in the view;
/// `own` are its sink indices.
struct ViewEvents<'v, 't> {
    reader: &'v Reader<'t, Vec<Event>>,
    sinks: &'v [Vec<Event>],
    own: Span<usize>,
}

impl Emitted for ViewEvents<'_, '_> {
    #[inline(always)]
    fn read<T>(&self, sink: usize, read: impl FnOnce(&[Event]) -> T) -> Option<T> {
        if self.own.contains(&sink) {
            self.sinks.get(sink - self.own.start).map(|e| read(e))
        } else {
            let row = self.reader.row(sink)?;
            row.first().map(|e| read(e))
        }
    }
}

/// A sequential segment's: every sink of the plan.
impl Emitted for &[Vec<Event>] {
    #[inline(always)]
    fn read<T>(&self, sink: usize, read: impl FnOnce(&[Event]) -> T) -> Option<T> {
        self.get(sink).map(|e| read(e))
    }
}

/// The canonical-order sum of `scale ·` each source in `sources` (output
/// index, scale) over block frames `a..b`, from +0 (§3), into `to`. One frame
/// is folded in a register; more are summed one source at a time, which does
/// each frame's additions in the same order, so the results are the same.
/// Both kinds of segment sum here, so they sum alike.
#[inline(always)]
fn sum_sources(
    to: &mut [f64],
    sources: &[(usize, f64)],
    from: &impl Sources,
    (a, b): (usize, usize),
) {
    if let [total] = to {
        let mut sum = 0.0;
        for &(o, scale) in sources {
            let x = from.read(o, (a, a + 1), |row| row.and_then(|r| r.first().copied()));
            sum += scale * x.unwrap_or(0.0);
        }
        *total = sum;
        return;
    }
    to.fill(0.0);
    for &(o, scale) in sources {
        from.read(o, (a, b), |row| add_source(to, row, scale));
    }
}

/// Adds `scale ·` source row `from` to `to`, frame by frame: the canonical
/// sum, one source at a time (each frame's additions keep their order).
fn add_source(to: &mut [f64], from: Option<&[f64]>, scale: f64) {
    match from {
        Some(from) => {
            for (total, &x) in to.iter_mut().zip(from) {
                *total += scale * x;
            }
        }
        None => {
            for total in to.iter_mut() {
                *total += scale * 0.0;
            }
        }
    }
}

/// Computes an instance's inputs for frames `a..b` of the block, frame by
/// frame, once they are final: signal input sums, events on event inputs
/// (at in-call offsets), and parameter bases and effective values (ADR
/// 0016 §3, §4, §4a; §1a when suspended). Returns how the instance takes
/// part in these frames.
fn gather<R: Own, S: Sources, E: Emitted>(
    fx: &Fixed<'_>,
    g: &mut Gather<'_, R, S, E>,
    a: usize,
    b: usize,
) -> Mode {
    let mode = g.slot.mode();
    let (signal_in, event_in, params) = (
        g.slot.signal_in.clone(),
        g.slot.event_in.clone(),
        g.slot.params.clone(),
    );

    // Signal inputs: the canonical-order sum of scale · source, from +0
    // (computed even when the instance is not called: the audio sink's
    // inputs are what the drivers record). Adding one source at a time over
    // the frames does each frame's additions in the same order.
    for (p, s) in signal_in.enumerate() {
        let from = fx.signal_sources.get(s).map_or(&[][..], Vec::as_slice);
        let Some(row) = g.signal_in.frames_mut(p, (a, b)) else {
            continue;
        };
        sum_sources(row, from, &g.sources, (a, b));
    }

    let mut discards = 0u64;

    // Event inputs: external events by sequence, then module events by
    // source port address, each source in emission order (ADR 0016 §3).
    let slot = &mut *g.slot;
    slot.events.clear();
    let inputs = fx.event_inputs.get(event_in).unwrap_or(&[]);
    for (input, end) in inputs.iter().zip(slot.ends.iter_mut()) {
        let target = input.port;
        for f in a..b {
            for staged in staged_at(fx.staged, target, f) {
                let source = LogSource::External {
                    sequence: staged.sequence,
                };
                let to = (target, a, f, staged.payload, source);
                deliver(fx, &mut slot.events, g.out, &mut discards, mode, to);
            }
            for source in &input.sources {
                let from = LogSource::Port(source.port);
                g.events.read(source.sink, |events| {
                    for event in events.iter().filter(|e| e.offset as usize == f) {
                        for _ in 0..source.copies {
                            let to = (target, a, f, event.payload, from);
                            deliver(fx, &mut slot.events, g.out, &mut discards, mode, to);
                        }
                    }
                });
            }
        }
        *end = slot.events.len();
    }

    // Parameters, frame by frame: advance, then events in canonical order,
    // then the effective value (§4a, §4). While suspended, events act on
    // the base only, and the effective value stays.
    for (q, j) in params.enumerate() {
        let Some(param) = g.params.get_mut(q) else {
            continue;
        };
        let target = param.port;
        if mode == Mode::Running
            && !param.modulation.is_empty()
            && let Some(modulation) = g.lane.modulation.get_mut(a..b)
        {
            sum_sources(modulation, &param.modulation, &g.sources, (a, b));
        }
        for f in a..b {
            if mode == Mode::Running {
                param.base.advance();
            }
            for staged in staged_at(fx.staged, target, f) {
                let source = LogSource::External {
                    sequence: staged.sequence,
                };
                let to = (target, f, staged.payload, source);
                apply(
                    fx,
                    (&mut param.base, &mut param.held_modulation),
                    g.out,
                    &mut discards,
                    mode,
                    to,
                );
            }
            for source in &param.events {
                let from = LogSource::Port(source.port);
                g.events.read(source.sink, |events| {
                    for event in events.iter().filter(|e| e.offset as usize == f) {
                        for _ in 0..source.copies {
                            let to = (target, f, event.payload, from);
                            apply(
                                fx,
                                (&mut param.base, &mut param.held_modulation),
                                g.out,
                                &mut discards,
                                mode,
                                to,
                            );
                        }
                    }
                });
            }
            let value = if mode == Mode::Running {
                let modulation = if param.modulation.is_empty() {
                    param.held_modulation
                } else {
                    g.lane.modulation.get(f).copied()
                };
                effective(param.base.value(), modulation, param.combine, param.range)
            } else {
                previous(fx.param_values, &g.param_rows, (j, q), f)
            };
            g.param_rows.set(q, f, value);
        }
    }

    if discards > 0 {
        g.slot.discards = g.slot.discards.saturating_add(discards);
        if mode == Mode::Suspended {
            g.slot.suspension_discards = g.slot.suspension_discards.saturating_add(discards);
        }
    }
    mode
}

/// What [`gather`] works on for one instance: its slot, parameters, rows of
/// the signal input and parameter value buffers, lane and log, and where it
/// reads its sources' outputs and events.
struct Gather<'x, R, S, E> {
    slot: &'x mut Slot,
    params: &'x mut [Param],
    signal_in: R,
    param_rows: R,
    lane: &'x mut Lane,
    out: &'x mut Out,
    sources: S,
    events: E,
}

/// What the rest of a call (its fault guard and commit, events, fades and
/// holds) works on for one instance: its slot, lane, sinks and the events
/// they keep, its rows of the output buffer, held outputs and the log.
struct Run<'x, R> {
    slot: &'x mut Slot,
    lane: &'x mut Lane,
    sinks: &'x mut [EventSink],
    emitted: &'x mut [Vec<Event>],
    outputs: R,
    held: &'x mut [f64],
    out: &'x mut Out,
}

/// The staged external events for `target` at block frame `f`, by sequence.
fn staged_at(staged: &[Staged], target: PortId, f: usize) -> &[Staged] {
    let key = (target.0, f);
    let start = staged.partition_point(|e| (e.target, e.frame) < key);
    let end = staged.partition_point(|e| (e.target, e.frame) <= key);
    staged.get(start..end).unwrap_or(&[])
}

/// One event arriving at an event input: (target, the call's first frame,
/// its frame, payload, source).
type Arrival = (PortId, usize, usize, EventPayload, LogSource);

/// One event arriving at event input `target` at block frame `f`: delivered
/// and logged, or discarded while the instance is suspended or muted.
fn deliver(
    fx: &Fixed<'_>,
    buffer: &mut Vec<Event>,
    out: &mut Out,
    discards: &mut u64,
    mode: Mode,
    (target, a, f, payload, source): Arrival,
) {
    let frame = fx.frame(f);
    let record = match mode {
        Mode::Running => {
            if buffer.len() >= buffer.capacity() {
                // Unreachable within the construction-time bound; never
                // reallocate, never call it delivered.
                out.input_overflows = out.input_overflows.saturating_add(1);
                return;
            }
            let offset = u32::try_from(f - a).unwrap_or(u32::MAX);
            buffer.push(Event { offset, payload });
            LogRecord::Event {
                frame,
                target,
                payload,
                source,
            }
        }
        Mode::Suspended | Mode::Muted => {
            *discards = discards.saturating_add(1);
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
    push_record(&mut out.log, &mut out.log_overflows, record);
}

/// One event arriving at a parameter: (target, its frame, payload, source).
type ParamArrival = (PortId, usize, EventPayload, LogSource);

/// One event arriving at parameter `target` at block frame `f`: applied to
/// its base (or held while suspended) and logged, or discarded.
fn apply(
    fx: &Fixed<'_>,
    (base, held_modulation): (&mut Base, &mut Option<f64>),
    out: &mut Out,
    discards: &mut u64,
    mode: Mode,
    (target, f, payload, source): ParamArrival,
) {
    let applied = match mode {
        Mode::Running => Some(base.apply(payload)),
        Mode::Suspended => Some(base.apply_suspended(payload)),
        Mode::Muted => None,
    };
    // A set releases modulation held from removed sources (§12).
    if applied == Some(Applied::Yes) && is_set(payload) {
        *held_modulation = None;
    }
    let reason = match (applied, mode) {
        (Some(Applied::Yes), _) => None,
        (None, _) => Some(DiscardReason::Muted),
        (Some(Applied::NotAParameterEvent), Mode::Suspended) => Some(DiscardReason::Suspended),
        (Some(Applied::NotAParameterEvent), _) => Some(DiscardReason::NotAParameterEvent),
    };
    let frame = fx.frame(f);
    let record = match reason {
        None => LogRecord::Event {
            frame,
            target,
            payload,
            source,
        },
        Some(reason) => {
            *discards = discards.saturating_add(1);
            LogRecord::Discard {
                frame,
                target,
                payload,
                source,
                reason,
            }
        }
    };
    push_record(&mut out.log, &mut out.log_overflows, record);
}

/// Saves what a block call's gather changes, for [`restore`].
fn save<R, S, E>(g: &mut Gather<'_, R, S, E>) {
    let lane = &mut *g.lane;
    lane.saved_params.clear();
    for param in g.params.iter() {
        if lane.saved_params.len() < lane.saved_params.capacity() {
            lane.saved_params.push((param.base, param.held_modulation));
        }
    }
    lane.saved = Saved {
        log_len: g.out.log.len(),
        log_overflows: g.out.log_overflows,
        input_overflows: g.out.input_overflows,
        discards: g.slot.discards,
        suspension_discards: g.slot.suspension_discards,
    };
}

/// Undoes the gather since [`save`].
fn restore<R, S, E>(g: &mut Gather<'_, R, S, E>) {
    let lane = &*g.lane;
    for (param, &(base, held)) in g.params.iter_mut().zip(&lane.saved_params) {
        param.base = base;
        param.held_modulation = held;
    }
    g.slot.discards = lane.saved.discards;
    g.slot.suspension_discards = lane.saved.suspension_discards;
    g.out.log.truncate(lane.saved.log_len);
    g.out.log_overflows = lane.saved.log_overflows;
    g.out.input_overflows = lane.saved.input_overflows;
}

/// After a call of a running instance over frames `a..b` of the block: the
/// fault guard, frame by frame, then that frame's outputs become visible,
/// with the resume (or swap) fade-in on `audio` outputs. A fault found at
/// frame n acts exactly from n: the outputs fade from what readers saw at
/// n − 1. Returns the fault for [`finish`].
fn commit<R: Own>(
    fx: &Fixed<'_>,
    r: &mut Run<'_, R>,
    a: usize,
    b: usize,
) -> Option<(usize, PortId, FaultReason)> {
    let len = b - a;
    let (out, events) = (r.slot.signal_out.clone(), r.slot.event_out.clone());
    let mut fault = None;
    for c in 0..len {
        let found = check(
            r.lane.call_outputs.get(..out.len() * len).unwrap_or(&[]),
            len,
            c,
            fx.output_ports.get(out.clone()).unwrap_or(&[]),
            r.sinks,
            fx.sink_ports.get(events.clone()).unwrap_or(&[]),
        );
        if let Some((port, reason)) = found {
            fault = Some((a + c, port, reason));
            break;
        }
        let f = a + c;
        let fade = r.slot.fade;
        for (p, o) in out.clone().enumerate() {
            let audio = fx.output_audio.get(o).copied().unwrap_or(false);
            let live = r.lane.call_outputs.get(p * len + c).copied().unwrap_or(0.0);
            let visible = match fade {
                Fade::In(j) if audio => fade_in(live, j, fx.fade_len),
                _ => live,
            };
            r.outputs.set(p, f, visible);
        }
        if let Fade::In(j) = fade {
            let next = j.saturating_add(1);
            r.slot.fade = if next >= fx.fade_len {
                Fade::None
            } else {
                Fade::In(next)
            };
        }
    }

    if let Some((n, _, _)) = fault {
        // Fade from what readers saw at frame n − 1.
        for (p, o) in out.enumerate() {
            let visible = previous(fx.outputs, &r.outputs, (o, p), n);
            if let Some(held) = r.held.get_mut(p) {
                *held = visible;
            }
        }
    }
    fault
}

/// The fault guard mutes the instance after a call that faulted at frame `n`
/// (`fault`; with `rollback`, the gather of the frames after it is undone
/// and done again as for a muted instance, by `regather`), and logs the
/// fault; the caller then fades its outputs from `n`.
fn mute<R, S, E>(
    fx: &Fixed<'_>,
    g: &mut Gather<'_, R, S, E>,
    i: usize,
    (a, b): (usize, usize),
    rollback: bool,
    (n, port, reason): (usize, PortId, FaultReason),
) where
    R: Own,
    S: Sources,
    E: Emitted,
{
    if rollback && n + 1 < b {
        restore(g);
        gather(fx, g, a, n + 1);
        g.slot.fault = Some(0);
        gather(fx, g, n + 1, b);
    } else {
        g.slot.fault = Some(0);
    }
    let record = LogRecord::Fault {
        frame: fx.frame(n),
        instance: InstanceId(i),
        port,
        reason,
    };
    push_record(&mut g.out.log, &mut g.out.log_overflows, record);
}

/// The call's events over frames `a..b` for their receivers, up to the
/// fault's frame `fault` if there is one.
fn keep_events<R>(r: &mut Run<'_, R>, a: usize, b: usize, fault: Option<usize>) {
    let len = b - a;
    let until = fault.map_or(len, |n| n - a);
    for (sink, store) in r.sinks.iter().zip(r.emitted.iter_mut()) {
        for event in sink.events() {
            let c = (event.offset as usize).min(len - 1);
            if c >= until {
                continue;
            }
            if store.len() < store.capacity() {
                let offset = u32::try_from(a + c).unwrap_or(u32::MAX);
                store.push(Event {
                    offset,
                    payload: event.payload,
                });
            } else {
                // Unreachable: a sink keeps 16 events per frame.
                r.out.input_overflows = r.out.input_overflows.saturating_add(1);
            }
        }
    }
}

/// A suspended instance's frame `f`: `audio` outputs take the next fade-out
/// frame (exactly 0 from the F-th suspended frame on), the other outputs hold
/// what readers saw in the frame before.
fn fade_suspended<R: Own>(fx: &Fixed<'_>, r: &mut Run<'_, R>, f: usize) {
    let fade = r.slot.fade;
    for (p, o) in r.slot.signal_out.clone().enumerate() {
        let audio = fx.output_audio.get(o) == Some(&true);
        let value = match fade {
            Fade::Out(j) if audio => {
                fade_out(r.held.get(p).copied().unwrap_or(0.0), j, fx.fade_len)
            }
            _ => previous(fx.outputs, &r.outputs, (o, p), f),
        };
        r.outputs.set(p, f, value);
    }
    if let Fade::Out(j) = fade {
        r.slot.fade = Fade::Out(j.saturating_add(1));
    }
}

/// A muted instance's frame `f`: every signal output takes the next frame of
/// the 64-frame fault fade (exactly 0 from its 64th frame on).
fn fade_muted<R: Own>(fx: &Fixed<'_>, r: &mut Run<'_, R>, f: usize) {
    for (p, o) in r.slot.signal_out.clone().enumerate() {
        let value = match r.slot.fault {
            Some(j) => fade_out(r.held.get(p).copied().unwrap_or(0.0), j, FAULT_FADE_FRAMES),
            None => previous(fx.outputs, &r.outputs, (o, p), f),
        };
        r.outputs.set(p, f, value);
    }
    if let Some(j) = r.slot.fault {
        r.slot.fault = Some(j.saturating_add(1));
    }
}

/// Port `global` (the instance's port `local`) in the frame before block
/// frame `f`: from its rows (a view's window holds the frame before its
/// call), or from `last` (the previous block's last frame) at the block's
/// first frame.
fn previous(last: &[f64], rows: &impl Own, (global, local): (usize, usize), f: usize) -> f64 {
    let value = match f.checked_sub(1) {
        Some(before) => rows.get(local, before),
        None => last.get(global).copied(),
    };
    value.unwrap_or(0.0)
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

/// One external event of the current block, delivered at block frame
/// `frame`, resolved against `plan`; `None`, and counted, when it has no
/// target there or a non-finite value.
fn resolve(
    plan: &PlanState,
    discards: &mut InputDiscards,
    event: &ExternalEvent,
    frame: usize,
    pushed: bool,
    ordinal: u64,
) -> Option<Staged> {
    let target = match plan.role_of(&event.target) {
        Some((target, Role::EventIn(_) | Role::Param(_))) => target,
        _ => {
            discards.unrouted = discards.unrouted.saturating_add(1);
            return None;
        }
    };
    if !finite(event.payload) {
        discards.invalid = discards.invalid.saturating_add(1);
        return None;
    }
    Some(Staged {
        target,
        frame,
        sequence: event.sequence,
        pushed,
        ordinal,
        payload: event.payload,
    })
}

/// Stages `item`; never allocates.
fn push_staged(staged: &mut Vec<Staged>, overflows: &mut u64, item: Option<Staged>) {
    let Some(item) = item else {
        return;
    };
    if staged.len() >= staged.capacity() {
        // Unreachable within the construction-time bound; never reallocate.
        *overflows = overflows.saturating_add(1);
        return;
    }
    staged.push(item);
}

/// The canonical-order sum of `scale · output`, starting from +0.
fn sum(sources: &[(usize, f64)], outputs: &[f64]) -> f64 {
    sources.iter().fold(0.0, |total, &(o, scale)| {
        total + scale * outputs.get(o).copied().unwrap_or(0.0)
    })
}

/// The fault guard for frame `c` of a call of `len` frames: the first output,
/// signal outputs then event outputs in descriptor order, that is non-finite,
/// larger than 10⁶ in magnitude, or an event output that dropped an event at
/// that frame (more than 16 in it).
fn check(
    signals: &[f64],
    len: usize,
    c: usize,
    signal_ports: &[PortId],
    sinks: &[EventSink],
    sink_ports: &[PortId],
) -> Option<(PortId, FaultReason)> {
    for (p, &port) in signal_ports.iter().enumerate() {
        if let Some(reason) = signals.get(p * len + c).copied().and_then(fault_of) {
            return Some((port, reason));
        }
    }
    for (sink, &port) in sinks.iter().zip(sink_ports) {
        if sink.first_overflow() == Some(c) {
            return Some((port, FaultReason::EventOverflow));
        }
        for event in sink.events() {
            if (event.offset as usize).min(len.saturating_sub(1)) != c {
                continue;
            }
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

/// The most of `frames` (sorted) within any `window` consecutive frames.
fn most_within(frames: &[u64], window: u64) -> usize {
    let mut most = 0;
    let mut first = 0;
    for (last, &frame) in frames.iter().enumerate() {
        while frames
            .get(first)
            .is_some_and(|&f| f.saturating_add(window) <= frame)
        {
            first += 1;
        }
        most = most.max(last + 1 - first);
    }
    most
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
