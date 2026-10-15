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
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use signal_core::{
    BlockLatency, Combine, DirectPrimitive, Direction, Due, Event, EventPayload, EventSink,
    Feedthrough, Frame, Inputs, InstanceAddress, LossCause, ModuleDescriptor, Outputs, PortAddress,
    PortKind, PortShape, PrepareContext, Primitive, Range, Registry, Reproducibility, RngStream,
    SinkKind, StateOutputPrimitive, fnv1a64_words,
};
use signal_graph::ExecutionPlan;

use crate::cores::performance_cores;
use crate::error::EngineError;
use crate::fade::{FAULT_FADE_FRAMES, Fade, fade_frames, fade_in, fade_out};
use crate::log::{
    DiscardReason, EngineFailure, FailureReason, FaultReason, InstanceId, LineSource, LogLine,
    LogRecord, LogSource, PortId,
};
use crate::manifest::SameMachine;
use crate::parameter::{Applied, Base, effective};
use crate::pool::{Pool, Workers};
use crate::realtime::HangProbe;
use crate::shared::{Access, CallView, Job, Layout, Parts, PlanCells, Reader, Window, caught};
#[cfg(feature = "test-hooks")]
use crate::shared::{Armed, HookPoint, Refuse};
use crate::sink::Sink;
use crate::tasks::{DEFAULT_COST_NS, TASK_MIN_NS, Tasks, tasks};
use crate::units::{Unit, Units, units};
use crate::wake::{MakeWaiter, RtPolicy, WakeStats};

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
    /// How long an idle worker spins before it parks, in ns (at most 3 ms,
    /// whatever is asked: `docs/realtime-rules.md` "Wake policies").
    pub spin_ns: u64,
    /// How idle workers wait and are woken (M2.7-1; `park50` is the
    /// behaviour before it). [`realtime`](crate::realtime) sets it from
    /// [`RealtimeConfig::rt`](crate::RealtimeConfig::rt).
    pub policy: RtPolicy,
    /// The workers' computation budget per period, in ns: what `budget-spin`
    /// spins within. Non-zero under `budget-spin`, it supersedes `spin_ns`
    /// (so `signal bench --rt-schedule spin=…` has no effect there); 0 is
    /// none, and `budget-spin` then spins `spin_ns`.
    pub budget_ns: u64,
    /// Keep the wake-up trace ([`Engine::wake_stats`]).
    pub trace: bool,
}

impl Schedule {
    /// The documented thresholds.
    pub const DEFAULT: Schedule = Schedule {
        task_min_ns: TASK_MIN_NS,
        parallel_min_ns: 50_000,
        min_frames: 16,
        spin_ns: 50_000,
        policy: RtPolicy::Park50,
        budget_ns: 0,
        trace: false,
    };

    /// Every segment of every plan with two tasks or more on the workers,
    /// one task per unit: for tests and the bench.
    pub const FORCED: Schedule = Schedule {
        task_min_ns: 0,
        parallel_min_ns: 0,
        min_frames: 1,
        ..Schedule::DEFAULT
    };

    /// The schedule a real-time engine of `threads` threads with device
    /// period `period_ns` uses ([`realtime`](crate::realtime)): the default,
    /// workers parking after 50 µs. M2.6-5 measured spinning through the
    /// period in the sandbox (no real-time scheduling) and found the tail far
    /// shorter, but on the device, with the workers in the audio workgroup,
    /// it caused thousands of xruns (80 voices: 4 060 in 15 s against 0), most
    /// likely because the system throttles real-time threads that exceed
    /// their computation budget. Operator decision at the M2.6-5 checkpoint:
    /// keep the default until a device-measured policy beats it
    /// (`docs/benchmarks.md` "Real-time wake-up tail"). [`Schedule::spinning`]
    /// keeps the measured variant for `signal bench`.
    pub fn realtime(period_ns: u64, threads: usize) -> Schedule {
        let _ = (period_ns, threads);
        Schedule::DEFAULT
    }

    /// Idle workers spin through the device period `period_ns` (at most 3 ms)
    /// before they park: the M2.6-5 candidate, measured but not the default.
    pub fn spinning(period_ns: u64) -> Schedule {
        Schedule {
            spin_ns: period_ns,
            ..Schedule::DEFAULT
        }
    }
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
    /// Whether the engine runs in real time ([`realtime`](crate::realtime) sets
    /// it). A module with a declared latency (ADR 0019 §3) is then never
    /// waited for: a result not ready when due is a `late_result` fault.
    /// Offline (`false`) the engine waits for it, so renders stay exact.
    pub realtime: bool,
    /// Real time (M2.7-4): with each `delivered` boundary record, also carry
    /// the latent module's delivered result
    /// ([`delivered_result`](signal_core::StateOutputPrimitive::delivered_result)),
    /// for a session recorded with `--record-boundary-data`.
    pub record_boundary_data: bool,
    /// Offline exact replay (M2.7-4, `docs/semantics.md` §15): every due
    /// result of a latent module follows the outcome pushed with
    /// [`Engine::push_boundary`] instead of being waited for.
    pub scripted_boundaries: bool,
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
            realtime: false,
            record_boundary_data: false,
            scripted_boundaries: false,
        }
    }
}

/// A recorded outcome of a latent module's due block, pushed for an exact
/// replay ([`Engine::push_boundary`], M2.7-4).
#[derive(Clone, Debug, PartialEq)]
pub enum Scripted {
    /// The result was missed: the engine asks `due` once, without waiting,
    /// and then calls `miss`, as in real time. With the recorded cause, if
    /// the run recorded one (M2.7-5), which the replay logs as it was.
    Missed(Option<LossCause>),
    /// The result was delivered: with the recorded words, which the module
    /// shows in place of its device's; without, the engine waits for it.
    /// With the recorded hash (M2.7-8), the result shown is checked against
    /// it ([`take_boundary_hash_mismatch`](Engine::take_boundary_hash_mismatch)).
    Delivered {
        /// The recorded words, if the session has them.
        words: Option<Box<[f32]>>,
        /// The recorded FNV-1a 64 of the result
        /// ([`fnv1a64_words`](signal_core::fnv1a64_words)); none in a session
        /// written before hashes.
        fnv1a64: Option<u64>,
    },
}

/// A pushed boundary outcome with its frame.
#[derive(Debug)]
struct PushedBoundary {
    at: Frame,
    instance: InstanceAddress,
    outcome: Scripted,
}

/// A pushed boundary outcome of the current block, resolved to its instance.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StagedBoundary {
    slot: usize,
    frame: Frame,
    /// Its index in the engine's queue of pushed outcomes.
    entry: usize,
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
    /// The external effect its descriptor declares, if any (ADR 0022: the
    /// engine hands a sink's inputs to the driver's output, whatever the
    /// kind).
    sink: Option<SinkKind>,
    /// Declares a latency (ADR 0019 §3): asked [`StateOutputPrimitive::due`]
    /// before each `emit`.
    latent: bool,
    /// Its declared latency, which says which of its frames are due.
    latency: Option<BlockLatency>,
    /// A latent instance's frames run (absorbed) since it was prepared: the
    /// module counts its blocks the same way (`module-contract.md`).
    latent_frames: u64,
    /// The words its delivered result has at most
    /// ([`StateOutputPrimitive::result_words`]); 0 without one.
    result_words: usize,
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
    /// Its rate divisor *d* (M2.8-6, `docs/semantics.md` §16): it is called
    /// on the frames that are multiples of *d* only.
    divisor: u64,
    /// For *d* > 1: what it wrote at its last tick, per signal output row
    /// (0 before its first), which its outputs hold between ticks. Empty for
    /// *d* = 1.
    hold: Vec<f64>,
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
    /// Due results of latent modules given up on (`late_result` faults).
    late_results: u64,
    /// Boundary bookkeeping (M2.7-4).
    boundary: BoundaryCounts,
    /// The delivered results that `Delivered` records carry, by their `at`.
    boundary_data: Vec<f32>,
}

/// What a block did at latent modules' boundaries (M2.7-4), beside
/// `late_results`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BoundaryCounts {
    /// Due results delivered (real time).
    pub(crate) delivered: u64,
    /// Delivered results whose data did not fit the block's buffer.
    pub(crate) data_overflows: u64,
    /// Exact replay: staged outcomes applied.
    pub(crate) used: u64,
    /// Exact replay: due frames without a staged outcome.
    pub(crate) unscripted: u64,
    /// Exact replay: recorded deliveries this run could not reproduce.
    pub(crate) unreproduced: u64,
    /// Exact replay (M2.7-8): the earliest (due frame, instance) whose shown
    /// result differs from its recorded hash. Only ever set offline, in a
    /// scripted replay; never in real time.
    pub(crate) hash_mismatch: Option<(Frame, usize)>,
}

impl BoundaryCounts {
    fn add(&mut self, other: BoundaryCounts) {
        self.delivered = self.delivered.saturating_add(other.delivered);
        self.data_overflows = self.data_overflows.saturating_add(other.data_overflows);
        self.used = self.used.saturating_add(other.used);
        self.unscripted = self.unscripted.saturating_add(other.unscripted);
        self.unreproduced = self.unreproduced.saturating_add(other.unreproduced);
        self.hash_mismatch = first_of(self.hash_mismatch, other.hash_mismatch);
    }
}

/// The earlier of two (frame, instance) mismatches, either possibly absent.
/// Not `Option::min`, for which `None` is the least.
fn first_of(a: Option<(Frame, usize)>, b: Option<(Frame, usize)>) -> Option<(Frame, usize)> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

impl Out {
    /// Empties the log and the counters; allocates nothing.
    pub(crate) fn clear(&mut self) {
        self.log.clear();
        self.log_overflows = 0;
        self.input_overflows = 0;
        self.late_results = 0;
        self.boundary = BoundaryCounts::default();
        self.boundary_data.clear();
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
        // Each port's rows: 1 for a signal or parameter, N for a vector.
        let mut port_rows = vec![0usize; ports.len()];
        // M2.7-2: what the host lends each instance, asked once per plan
        // state with every instance in address order; never read here.
        let mut lent = match registry.host() {
            Some(host) => {
                let all: Vec<_> = plan
                    .instances
                    .iter()
                    .map(|i| (&i.address, i.type_name))
                    .collect();
                host.lend(&all)
            }
            None => Vec::new(),
        };
        lent.resize(plan.instances.len(), None);

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
            // M2.8-6: one rate per type (`docs/semantics.md` §16); a divided
            // rate has no event ports, no declared latency and no sink.
            let divisor = u64::from(
                descriptor
                    .rate()
                    .ok_or_else(|| EngineError::MixedRates {
                        instance: address.clone(),
                    })?
                    .get(),
            );
            if divisor > 1 {
                let limit = if descriptor
                    .ports
                    .iter()
                    .any(|p| p.shape == PortShape::Events)
                {
                    Some("event ports")
                } else if descriptor.latency.is_some() {
                    Some("a declared latency")
                } else if descriptor.sink.is_some() {
                    Some("a sink")
                } else {
                    None
                };
                if let Some(what) = limit {
                    return Err(EngineError::UnsupportedRate {
                        instance: address.clone(),
                        what,
                    });
                }
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
                        lent: lent.get_mut(index).and_then(Option::take),
                    };
                    match &mut primitive {
                        Primitive::Direct(module) => module.prepare(&context),
                        Primitive::StateOutput(module) => module.prepare(&context),
                    }
                    // M2.7-8: every delivered result of a latent module is
                    // hashed, so a latent module must expose its result.
                    if let Primitive::StateOutput(module) = &primitive
                        && descriptor.latency.is_some()
                        && module.result_words() == 0
                    {
                        return Err(EngineError::LatentWithoutResult {
                            instance: address.clone(),
                        });
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
                let id = ports.binary_search(&port).unwrap_or(usize::MAX);
                // A vector of N is N rows, from its first (M2.8-6).
                let rows = p.shape.rows();
                if let Some(r) = port_rows.get_mut(id) {
                    *r = rows;
                }
                let role = match (p.direction, p.shape) {
                    (_, PortShape::Vector(0)) => {
                        return Err(EngineError::UnsupportedShape { port });
                    }
                    (Direction::In, PortShape::Signal | PortShape::Vector(_)) => {
                        signal_inputs += rows;
                        Role::SignalIn(signal_inputs - rows)
                    }
                    (Direction::In, PortShape::Events) => {
                        event_inputs.push(EventInput {
                            port: PortId(id),
                            sources: Vec::new(),
                        });
                        Role::EventIn(event_inputs.len() - 1)
                    }
                    (Direction::Out, PortShape::Signal | PortShape::Vector(_)) => {
                        for _ in 0..rows {
                            output_audio.push(p.kind == PortKind::Audio);
                            output_ports.push(PortId(id));
                        }
                        Role::SignalOut(output_audio.len() - rows)
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
                let id = ports.binary_search(&port).unwrap_or(usize::MAX);
                if let Some(r) = port_rows.get_mut(id) {
                    *r = 1;
                }
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
                sink: descriptor.sink,
                latent: descriptor.latency.is_some(),
                latency: descriptor.latency,
                latent_frames: 0,
                result_words: match (&primitive, survivor) {
                    (Some(Primitive::StateOutput(module)), _) if descriptor.latency.is_some() => {
                        module.result_words()
                    }
                    (_, Some(o)) => old
                        .and_then(|old| old.cells.slots.get().get(o))
                        .map_or(0, |s| s.result_words),
                    _ => 0,
                },
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
                divisor,
                hold: if divisor > 1 {
                    vec![0.0; output_audio.len() - starts.2]
                } else {
                    Vec::new()
                },
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
                let (Some((to_id, (to, target_role))), Some((from_id, (from, source_role)))) =
                    (role_of(target), role_of(&source.from))
                else {
                    return Err(invalid());
                };
                // A signal or a vector: element-wise, equal shapes only.
                let rows = source.shape.rows();
                let same = |id: usize| port_rows.get(id) == Some(&rows);
                match (target_role, source_role, source.shape) {
                    (Role::SignalIn(k), Role::SignalOut(o), shape)
                        if shape.is_signal() && same(to_id) && same(from_id) =>
                    {
                        for e in 0..rows {
                            signal_sources
                                .get_mut(k + e)
                                .ok_or_else(invalid)?
                                .push((o + e, source.scale));
                        }
                    }
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
        // X_tot, all targets in total; pushed: Q), 16 events per frame per
        // event connection, and a `late_result` per frame per latent instance.
        let event_connections: usize = event_inputs
            .iter()
            .map(|i| connections(&i.sources))
            .sum::<usize>()
            + params.iter().map(|p| connections(&p.events)).sum::<usize>();
        let log_capacity = 1
            + slots.len() * capacity
            + slots.len()
            + slots.iter().filter(|s| s.latent).count() * capacity
            + most_in_total
            + config.queue_capacity
            + EVENTS_PER_FRAME * capacity * event_connections;
        let log = Vec::with_capacity(log_capacity);
        let log_order = Vec::with_capacity(log.capacity());
        // Delivered results carried in a block (M2.7-4): each latent
        // instance's words, once per of its blocks a block can begin.
        let data_words = |slot: &Slot| match slot.latency {
            Some(l) if config.record_boundary_data && config.realtime => {
                slot.result_words * (capacity / (l.block_frames.max(1) as usize) + 1)
            }
            _ => 0,
        };
        let boundary_data = Vec::with_capacity(slots.iter().map(data_words).sum());

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
        // One log per task when there are workers: each instance's fault,
        // 16 events per frame per event connection into it and a latent
        // instance's `late_result` per frame, plus
        // `task_spare(K)` staged externals (a segment with more for one task
        // runs on the calling thread, `Engine::parallel`).
        let mut outs = Vec::new();
        if threads > 1 {
            let mut bound = vec![task_spare(capacity); tasks.len()];
            let mut words = vec![0usize; tasks.len()];
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
                    if slot.latent {
                        *b += capacity;
                    }
                }
                if let Some(w) = words.get_mut(t) {
                    *w += data_words(slot);
                }
            }
            outs = bound
                .into_iter()
                .zip(words)
                .map(|(b, w)| Out {
                    log: Vec::with_capacity(b),
                    boundary_data: Vec::with_capacity(w),
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
                boundary_data,
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

    /// The plan's sinks, in instance order ([`InstanceId`]): each instance whose descriptor
    /// declares a sink kind, with its kind and number of signal inputs.
    pub(crate) fn sinks(&self) -> Vec<(InstanceAddress, SinkKind, usize)> {
        let slots = self.cells.slots.get();
        slots
            .iter()
            .zip(&self.instances)
            .filter_map(|(slot, address)| Some((address.clone(), slot.sink?, slot.signal_in.len())))
            .collect()
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
                cause,
            } => LogLine::Fault {
                frame,
                address: self.instances.get(instance.0)?,
                // A fault of an instance without outputs (a `panic`, or a
                // `late_result`) names no port; its line is still written
                // (`docs/semantics.md` §9, §14).
                port: self.ports.get(port.0),
                reason,
                cause,
            },
            // Not a line of `log.jsonl`: it goes to `ControlSink::boundary`.
            LogRecord::Delivered { .. } => return None,
        })
    }

    /// The address of instance `instance` of this plan.
    pub(crate) fn instance_address(&self, instance: InstanceId) -> Option<&InstanceAddress> {
        self.instances.get(instance.0)
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
    ns.latent_frames = os.latent_frames;
    ns.result_words = os.result_words;
    ns.fault = os.fault;
    ns.fade = os.fade;
    ns.discards = os.discards;
    ns.suspension_discards = os.suspension_discards;
    ns.edited_at = os.edited_at;
    for (a, &b) in ns.hold.iter_mut().zip(&os.hold) {
        *a = b;
    }
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
    /// Exact replay (M2.7-4): pushed boundary outcomes in frame order; the
    /// first `boundaries_consumed` were staged.
    boundaries: VecDeque<PushedBoundary>,
    boundaries_consumed: usize,
    /// The current block's pushed outcomes, sorted by (instance, frame).
    staged_boundaries: Vec<StagedBoundary>,
    /// Boundary bookkeeping since the start.
    boundary: BoundaryCounts,
    /// Exact replay: pushed outcomes that applied to no due frame.
    boundary_unused: u64,

    /// (version, plan version) of a swap committed for the next frame.
    swap_record: Option<(u64, u64)>,
    discards: InputDiscards,
    log_overflows: u64,
    input_overflows: u64,
    /// Due results of latent modules given up on (ADR 0019 §3).
    late_results: u64,

    /// The first frame of the current (or last) block, and its length.
    block_start: Frame,
    block_frames: usize,

    /// The worker threads (none for one thread), kept across swaps.
    pool: Option<Pool>,
    /// The parallel segments run, which is also each one's job epoch.
    parallel_segments: u64,
    /// The test hook (`set_task_hook`).
    #[cfg(feature = "test-hooks")]
    hook: Option<fn(HookPoint)>,
    /// Why the engine stopped (`docs/semantics.md` §14): it runs no frame
    /// after.
    failure: Option<EngineFailure>,
    /// Why the `semaphore` policy's waiter could not be made (it parks).
    waiter_error: Option<String>,
    /// Real time: the hang probe its calls record themselves in (M2.7-9).
    hang: Option<Arc<HangProbe>>,
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
    /// Tasks that failed (something unwound outside a primitive's call), on
    /// any lane; each stopped the engine (`docs/semantics.md` §14).
    pub task_failures: u64,
    /// The wake policy.
    pub policy: RtPolicy,
    /// What idle workers block on: `"park"`, or `"semaphore"` (the policy's
    /// waiter); `"none"` without workers.
    pub waiter: &'static str,
}

/// Test hooks only: a broken invariant for [`Engine::inject`] to arm, found
/// by the next blocks as a defect would be (`docs/semantics.md` §14).
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Injection {
    /// Every view of this instance is refused (on the calling thread alone,
    /// its parts are not found).
    RefuseView(InstanceAddress),
    /// Every read of this instance's rows by another task is refused.
    RefuseRead(InstanceAddress),
    /// The next push of a ready task finds no slot.
    DropReady,
    /// The next claim of a task finds its slot empty.
    MissingSlot,
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
        Engine::with_waiter(plan, registry, config, external, None)
    }

    /// [`new`](Engine::new), with `make` making the workers' [`Waiter`]
    /// (`crate::Waiter`) when the schedule's policy is `semaphore`. Without
    /// it, or if it fails ([`waiter_error`](Engine::waiter_error)), the
    /// workers park.
    pub fn with_waiter(
        plan: &ExecutionPlan,
        registry: &Registry,
        config: EngineConfig,
        external: Vec<ExternalEvent>,
        make: Option<MakeWaiter>,
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

        let workers = threads - 1;
        let (waiter, waiter_error) = match make {
            Some(make) if workers > 0 && config.schedule.policy == RtPolicy::Semaphore => {
                match make(workers) {
                    Ok(waiter) => (Some(waiter), None),
                    Err(error) => (None, Some(error)),
                }
            }
            _ => (None, None),
        };
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
            boundaries: VecDeque::with_capacity(config.queue_capacity),
            boundaries_consumed: 0,
            staged_boundaries: Vec::with_capacity(config.queue_capacity),
            boundary: BoundaryCounts::default(),
            boundary_unused: 0,
            last_pushed_edit_at: Frame::ZERO,
            swap_record: None,
            discards: InputDiscards::default(),
            log_overflows: 0,
            input_overflows: 0,
            late_results: 0,
            block_start: Frame::ZERO,
            block_frames: 0,
            // With the wake-up trace on and no workers, a pool of none keeps
            // the trace (the slowest task of each callback); it never runs a
            // parallel segment (`parallel`) and spawns no thread.
            pool: (workers > 0 || config.schedule.trace)
                .then(|| Pool::new(workers, &config.schedule, waiter)),
            parallel_segments: 0,
            #[cfg(feature = "test-hooks")]
            hook: None,
            failure: None,
            waiter_error,
            hang: None,
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

    /// Exact replay (M2.7-4, offline, with
    /// [`scripted_boundaries`](EngineConfig::scripted_boundaries)): queues the
    /// recorded outcome of `instance`'s due block at `frame`. Push them in
    /// frame order, before the block that holds their frame runs. The
    /// instance is resolved against the plan running then; an outcome that
    /// meets no due frame is counted in
    /// [`boundary_mismatches`](Engine::boundary_mismatches). A full queue
    /// hands the outcome back.
    pub fn push_boundary(
        &mut self,
        instance: InstanceAddress,
        frame: Frame,
        outcome: Scripted,
    ) -> Result<(), Scripted> {
        if self.boundaries.len() >= self.config.queue_capacity {
            return Err(outcome);
        }
        self.boundaries.push_back(PushedBoundary {
            at: frame,
            instance,
            outcome,
        });
        Ok(())
    }

    /// Drops the pushed boundary outcomes that have been staged, freeing
    /// their queue space (offline only).
    pub fn reclaim_boundaries(&mut self) {
        let used = mem::take(&mut self.boundaries_consumed);
        self.boundaries.drain(..used);
    }

    /// The pushed boundary outcomes of the block of `frames` frames from
    /// `start`, resolved to instances and sorted by (instance, frame).
    fn stage_boundaries(&mut self, start: Frame, frames: usize) {
        self.staged_boundaries.clear();
        let end = start.0.saturating_add(frames as u64);
        while let Some(pushed) = self.boundaries.get(self.boundaries_consumed) {
            if pushed.at.0 >= end {
                break;
            }
            let entry = self.boundaries_consumed;
            self.boundaries_consumed += 1;
            match self.plan.instance_ids.get(&pushed.instance) {
                Some(&slot) if pushed.at >= start => self.staged_boundaries.push(StagedBoundary {
                    slot,
                    frame: pushed.at,
                    entry,
                }),
                _ => self.boundary_unused = self.boundary_unused.saturating_add(1),
            }
        }
        self.staged_boundaries
            .sort_unstable_by_key(|s| (s.slot, s.frame));
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
    /// committed for this frame, when the last frame's log has not been
    /// drained (its records name the old plan's ports), and, in an exact
    /// replay, when a hash mismatch has not been taken
    /// ([`take_boundary_hash_mismatch`](Engine::take_boundary_hash_mismatch):
    /// it names an instance of the old plan).
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
        // A plain field test: set only by an offline exact replay, never in
        // real time.
        if self.boundary.hash_mismatch.is_some() {
            return Err(EngineError::MismatchNotTaken);
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
    ///
    /// A failure of the engine itself (`docs/semantics.md` §14) stops it: the
    /// call returns 0, the block's frames and log are not to be used, and
    /// [`failure`](Engine::failure) says why; every later call returns 0.
    pub fn run_block(&mut self, frames: usize) -> usize {
        let k = frames.min(self.plan.capacity);
        if k == 0 || self.failure.is_some() {
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
        if self.config.scripted_boundaries {
            self.stage_boundaries(start, k);
        }
        let used_before = self.boundary.used;
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
            if let Some(reason) = self.run_segment(a, b) {
                self.failure = Some(EngineFailure { frame, reason });
                self.plan.out.log.clear();
                return 0;
            }
            self.refresh_last(b - 1);
            a = b;
        }

        let used = self.boundary.used.saturating_sub(used_before);
        let unused = (self.staged_boundaries.len() as u64).saturating_sub(used);
        self.boundary_unused = self.boundary_unused.saturating_add(unused);
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
        if self.pool.as_ref().is_none_or(|pool| pool.workers() == 0)
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
    /// order: the result is the same bit for bit. Returns why the engine
    /// failed in it, if it did (`docs/semantics.md` §14): a task unwound
    /// outside a primitive's call, or an access was refused.
    ///
    /// An exact replay's hash mismatch found before such a failure is kept
    /// (M2.7-8): it is the replay's first cause, whatever failed after it.
    /// The failure exits return before the segment's counters are merged,
    /// so it is taken from the outs here. Allocates nothing; only an offline
    /// exact replay ever sets one.
    fn run_segment(&mut self, a: usize, b: usize) -> Option<FailureReason> {
        let failure = self.run_segment_units(a, b);
        if failure.is_some() {
            let plan = &mut self.plan;
            let mut found = plan.out.boundary.hash_mismatch.take();
            for out in plan.cells.outs.get_mut() {
                found = first_of(found, out.boundary.hash_mismatch.take());
            }
            self.boundary.hash_mismatch = first_of(self.boundary.hash_mismatch, found);
        }
        failure
    }

    /// [`run_segment`](Engine::run_segment) without keeping a mismatch on a
    /// failure.
    fn run_segment_units(&mut self, a: usize, b: usize) -> Option<FailureReason> {
        let parallel = self.parallel(b - a);
        let trace = self.config.schedule.trace;
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
            wait: !self.config.realtime,
            hang: self.hang.as_deref(),
            record_data: self.config.realtime && self.config.record_boundary_data,
            script: (self.config.scripted_boundaries && !self.config.realtime)
                .then_some((&self.staged_boundaries[..], &self.boundaries)),
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
                let job = Job::new(&mut plan.cells, self.parallel_segments, &body, stats);
                #[cfg(feature = "test-hooks")]
                let job = job.with_hook(self.hook);
                // The job holds the cells until it is run: no worker is in it
                // once `run` returns.
                dispatch.run(&job);
                if let Some(reason) = job.failure() {
                    return Some(reason);
                }
                if plan.cells.take_refusals() > 0 {
                    return Some(FailureReason::InvariantViolation);
                }
                let main = &mut plan.out;
                for out in plan.cells.outs.get_mut() {
                    main.log_overflows = main.log_overflows.saturating_add(out.log_overflows);
                    main.input_overflows = main.input_overflows.saturating_add(out.input_overflows);
                    main.late_results = main.late_results.saturating_add(out.late_results);
                    main.boundary.add(out.boundary);
                    out.boundary = BoundaryCounts::default();
                    let (base, words) = (main.boundary_data.len(), out.boundary_data.len());
                    let fits = base + words <= main.boundary_data.capacity();
                    if fits {
                        main.boundary_data.extend_from_slice(&out.boundary_data);
                    }
                    out.boundary_data.clear();
                    for record in out.log.drain(..) {
                        let record = match record {
                            LogRecord::Delivered {
                                frame,
                                instance,
                                at,
                                words,
                                lost,
                                fnv1a64,
                            } if words > 0 => {
                                let lost = lost || !fits;
                                if !fits {
                                    main.boundary.data_overflows =
                                        main.boundary.data_overflows.saturating_add(1);
                                }
                                LogRecord::Delivered {
                                    frame,
                                    instance,
                                    at: if lost { 0 } else { at + base as u32 },
                                    words: if lost { 0 } else { words },
                                    lost,
                                    fnv1a64,
                                }
                            }
                            record => record,
                        };
                        push_record(&mut main.log, &mut main.log_overflows, record);
                    }
                }
            }
            pool => {
                #[cfg(feature = "test-hooks")]
                let refuse_view = plan.cells.refused_view();
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
                        refused: false,
                        #[cfg(feature = "test-hooks")]
                        refuse_view,
                    };
                    let pool = pool.as_deref();
                    // Anything that unwinds here is outside a primitive's
                    // call (those are caught at the call): the engine fails.
                    let ran = caught(|| {
                        match pool.filter(|_| trace) {
                            // The wake-up trace (M2.7-1): the same units in the
                            // same order, task by task (tasks are contiguous runs
                            // of the unit order), each task's body timed, so a
                            // segment on the calling thread has its slowest task
                            // too.
                            Some(pool) => {
                                let tasks = &plan.tasks;
                                let mut slowest = 0u64;
                                for t in 0..tasks.len() {
                                    let began = Instant::now();
                                    for u in tasks.units(t) {
                                        whole_unit(&fx, &mut whole, u);
                                    }
                                    let body = u64::try_from(began.elapsed().as_nanos())
                                        .unwrap_or(u64::MAX);
                                    slowest = slowest.max(body.max(1));
                                }
                                pool.stats()
                                    .slowest_task_ns
                                    .fetch_max(slowest, Ordering::Relaxed);
                            }
                            None => {
                                for u in 0..fx.units.order.len() {
                                    whole_unit(&fx, &mut whole, u);
                                }
                            }
                        }
                    });
                    if !ran {
                        return Some(FailureReason::TaskPanic);
                    }
                    if whole.refused {
                        return Some(FailureReason::InvariantViolation);
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
        self.late_results = self
            .late_results
            .saturating_add(mem::take(&mut out.late_results));
        self.boundary.add(mem::take(&mut out.boundary));
        None
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
            Some(pool) if pool.workers() > 0 => pool.handle(self.config.threads),
            _ => Workers::none(self.config.threads),
        }
    }

    /// Calls `hook` when a thread enters and leaves a parallel segment, and
    /// before and after the body of each task it runs (tests: delays,
    /// allocation guards, failures). Tests only (feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn set_task_hook(&mut self, hook: Option<fn(HookPoint)>) {
        self.hook = hook;
    }

    /// Arms a broken invariant for the next blocks to find, as a defect
    /// would (`docs/semantics.md` §14). Tests only (feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn inject(&mut self, injection: Injection) {
        let index = |address: &InstanceAddress| {
            self.plan
                .instances
                .iter()
                .position(|a| a == address)
                .unwrap_or(usize::MAX)
        };
        let armed = match injection {
            Injection::RefuseView(address) => Armed::Refuse(Refuse::View(index(&address))),
            Injection::RefuseRead(address) => Armed::Refuse(Refuse::Read(index(&address))),
            Injection::DropReady => Armed::DropReady,
            Injection::MissingSlot => Armed::MissingSlot,
        };
        self.plan.cells.inject(armed);
    }

    /// Why the engine stopped, if it did (`docs/semantics.md` §14): it ran
    /// nothing of that block's frames that anyone may use, and runs no
    /// further frame.
    pub fn failure(&self) -> Option<EngineFailure> {
        self.failure
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
            task_failures: count(stats.map(|s| &s.task_failures)),
            policy: self.config.schedule.policy,
            waiter: match pool {
                None => "none",
                Some(pool) if pool.workers() == 0 => "none",
                Some(pool) if pool.waits_on_waiter() => "semaphore",
                Some(_) => "park",
            },
        }
    }

    /// Why the `semaphore` policy's waiter could not be made, so that the
    /// workers park instead.
    #[doc(hidden)]
    pub fn waiter_error(&self) -> Option<&str> {
        self.waiter_error.as_deref()
    }

    /// Real time: the hang probe its primitive calls record themselves in
    /// (M2.7-9); one slot per thread lane.
    pub(crate) fn set_hang_probe(&mut self, probe: Arc<HangProbe>) {
        self.hang = Some(probe);
    }

    /// A driver's callback of `frames` frames starts (the audio thread;
    /// allocates, locks and blocks nowhere): under `semaphore` and `fanout`
    /// the workers are woken if the running plan can run in parallel in it
    /// (two tasks or more, and a block of `frames` reaching the schedule's
    /// thresholds), and the wake-up trace starts the callback. Timing only.
    #[doc(hidden)]
    pub fn begin_callback(&mut self, frames: usize) {
        let Some(pool) = self.pool.as_ref() else {
            return;
        };
        let schedule = self.config.schedule;
        let k = frames.min(self.config.block_capacity);
        let tasks = &self.plan.tasks;
        let parallel = tasks.len() >= 2
            && k >= schedule.min_frames
            && tasks.estimate_ns * (k as f64) >= schedule.parallel_min_ns as f64;
        pool.begin_callback(parallel);
    }

    /// The callback ends: the wake-up trace folds it in.
    #[doc(hidden)]
    pub fn end_callback(&mut self) {
        if let Some(pool) = self.pool.as_ref() {
            pool.end_callback();
        }
    }

    /// The wake-up trace (`Schedule::trace`), if kept and there are workers.
    #[doc(hidden)]
    pub fn wake_stats(&self) -> Option<WakeStats> {
        self.pool.as_ref().and_then(Pool::wake_stats)
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
        self.drain_log_with(&mut |record, _| out(record));
    }

    /// [`drain_log`](Engine::drain_log), handing each record with the
    /// delivered result it carries (a `Delivered` record with `words`, in
    /// real time with
    /// [`record_boundary_data`](EngineConfig::record_boundary_data)); empty
    /// for every other record. Allocates nothing.
    pub fn drain_log_with(&mut self, out: &mut impl FnMut(&LogRecord, &[f32])) {
        let plan = &mut self.plan;
        let log = &plan.out.log;
        plan.log_order.clear();
        plan.log_order.extend(0..log.len());
        plan.log_order
            .sort_unstable_by_key(|&k| (log.get(k).map(|r| (r.frame(), r.order())), k));
        let data = &plan.out.boundary_data;
        for &k in &plan.log_order {
            if let Some(record) = log.get(k) {
                let words = match *record {
                    LogRecord::Delivered { at, words, .. } => {
                        let (at, words) = (at as usize, words as usize);
                        data.get(at..at + words).unwrap_or(&[])
                    }
                    _ => &[],
                };
                out(record, words);
            }
        }
        plan.out.log.clear();
        plan.out.boundary_data.clear();
    }

    /// Log records dropped because the block's log was full. The buffer is
    /// sized so that this stays 0.
    pub fn log_overflows(&self) -> u64 {
        self.log_overflows
    }

    /// Due results of latent modules given up on, since the start: each is a
    /// `late_result` fault in the log (ADR 0019 §3). Always 0 offline, unless
    /// a result was lost.
    pub fn late_results(&self) -> u64 {
        self.late_results
    }

    /// Real time (M2.7-4): due results of latent modules delivered, since
    /// the start; each is a `Delivered` record.
    pub fn delivered_results(&self) -> u64 {
        self.boundary.delivered
    }

    /// Delivered results whose data did not fit the block's buffer (their
    /// records say `lost`). The buffer is sized so that this stays 0.
    pub fn boundary_data_overflows(&self) -> u64 {
        self.boundary.data_overflows
    }

    /// Exact replay (M2.7-4): due frames without a pushed outcome, plus
    /// pushed outcomes that met no due frame. 0 when the recorded outcomes
    /// and this run's due frames agree.
    pub fn boundary_mismatches(&self) -> u64 {
        self.boundary
            .unscripted
            .saturating_add(self.boundary_unused)
    }

    /// Exact replay (M2.7-4): recorded deliveries this run could not
    /// reproduce (the device lost the result, or the module refused the
    /// recorded data).
    pub fn boundary_unreproduced(&self) -> u64 {
        self.boundary.unreproduced
    }

    /// Exact replay (M2.7-8): the earliest due block, since the last call,
    /// whose shown result differs from the hash pushed with its outcome, as
    /// (due frame, instance address); and forgets it. Take it before every
    /// [`commit_swap`](Engine::commit_swap), which refuses while one is
    /// untaken (its instance is one of the plan that ran). Only an offline
    /// exact replay sets it; call it off the real-time thread (it clones the
    /// address). The mismatch is forgotten only once its address is known.
    pub fn take_boundary_hash_mismatch(&mut self) -> Option<(Frame, InstanceAddress)> {
        let (frame, slot) = self.boundary.hash_mismatch?;
        let address = self.instance_address(InstanceId(slot))?.clone();
        self.boundary.hash_mismatch = None;
        Some((frame, address))
    }

    /// The current plan's instances whose type is reproducible on the same
    /// machine only (ADR 0019 §3), in address order: what a manifest records.
    pub fn same_machine(&self) -> Vec<SameMachine> {
        let plan = &self.plan;
        let mut found: Vec<SameMachine> = plan
            .instances
            .iter()
            .zip(&plan.types)
            .filter(|(_, (_, d))| d.reproducibility == Reproducibility::SameMachine)
            .map(|(address, (type_name, d))| SameMachine {
                instance: address.to_string(),
                type_name: (*type_name).to_owned(),
                latency_blocks: d.latency.map(|l| l.latency_blocks),
                block_frames: d.latency.map(|l| l.block_frames),
            })
            .collect();
        found.sort_by(|a, b| a.instance.cmp(&b.instance));
        found
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

    /// The running plan's sinks (ADR 0022, "the sink handoff"): every
    /// instance whose descriptor declares a sink kind, in instance order
    /// ([`InstanceId`]), with its kind and number of signal inputs. The
    /// engine attaches no meaning to a kind; a driver's [`SinkOutput`](crate::SinkOutput) picks the
    /// sinks it serves and reads their inputs with
    /// [`sink_input_at`](Engine::sink_input_at). Allocates nothing, so a
    /// real-time output may call it when it rebinds after a swap.
    pub fn sinks(&self) -> impl Iterator<Item = Sink<'_>> {
        self.plan
            .cells
            .slots
            .get()
            .iter()
            .zip(&self.plan.instances)
            .enumerate()
            .filter_map(|(i, (slot, address))| {
                Some(Sink {
                    id: InstanceId(i),
                    kind: slot.sink?,
                    address,
                    inputs: slot.signal_in.len(),
                })
            })
    }

    /// The address of `instance`.
    pub fn instance_address(&self, instance: InstanceId) -> Option<&InstanceAddress> {
        self.plan.instances.get(instance.0)
    }

    /// The frames the last [`run_block`](Engine::run_block) ran.
    pub fn block_frames(&self) -> usize {
        self.block_frames
    }

    /// Signal input `input` of `instance` (in port order) at frame `f` of the
    /// last block ([`block_frames`](Engine::block_frames)): the sum the
    /// executor computed, whether or not the instance itself is suspended or
    /// muted. How a sink's inputs are handed to a driver (ADR 0022).
    pub fn sink_input_at(&self, instance: InstanceId, input: usize, f: usize) -> Option<f64> {
        let slot = self.plan.cells.slots.get().get(instance.0)?;
        if input >= slot.signal_in.len() {
            return None;
        }
        let s = slot.signal_in.start + input;
        self.plan
            .cells
            .signal_in
            .get()
            .get(s * self.plan.capacity + f)
            .copied()
    }

    /// The index of parameter `port` in the running plan, for
    /// [`parameter_at_frame`](Engine::parameter_at_frame).
    pub fn parameter_index(&self, port: &PortAddress) -> Option<usize> {
        match self.role(port)? {
            Role::Param(j) => Some(j),
            _ => None,
        }
    }

    /// The effective value of parameter `j` at frame `f` of the last block, as
    /// [`parameter`](Engine::parameter) gave it after that frame.
    pub fn parameter_at_frame(&self, j: usize, f: usize) -> Option<f64> {
        self.plan
            .cells
            .param_rows
            .get()
            .get(j * self.plan.capacity + f)
            .copied()
    }

    /// The index of signal output `port` in the running plan, for
    /// [`signal_output_at`](Engine::signal_output_at).
    pub fn signal_output_index(&self, port: &PortAddress) -> Option<usize> {
        match self.role(port)? {
            Role::SignalOut(o) => Some(o),
            _ => None,
        }
    }

    /// The first row and the number of rows of signal or vector output
    /// `port` in the running plan (1 for a signal, N for a vector of N), for
    /// [`signal_output_at`](Engine::signal_output_at). Allocates nothing.
    pub fn signal_output_rows(&self, port: &PortAddress) -> Option<(usize, usize)> {
        let (id, role) = self.plan.role_of(port)?;
        let Role::SignalOut(o) = role else {
            return None;
        };
        let rows = self
            .plan
            .output_ports
            .get(o..)
            .unwrap_or(&[])
            .iter()
            .take_while(|p| p.0 == id)
            .count();
        Some((o, rows))
    }

    /// Signal output `o` at frame `f` of the last block
    /// ([`block_frames`](Engine::block_frames)), as its readers saw it (fades
    /// included): the row [`signal_output`](Engine::signal_output) reads its
    /// last frame from.
    pub fn signal_output_at(&self, o: usize, f: usize) -> Option<f64> {
        self.plan
            .cells
            .outputs
            .get()
            .get(o * self.plan.capacity + f)
            .copied()
    }

    /// The index of signal input `port` in the running plan, for
    /// [`signal_input_at`](Engine::signal_input_at).
    pub fn signal_input_index(&self, port: &PortAddress) -> Option<usize> {
        match self.role(port)? {
            Role::SignalIn(k) => Some(k),
            _ => None,
        }
    }

    /// Signal input `k` at frame `f` of the last block: the sum of its
    /// sources, each scaled, as the executor computed it (the row
    /// [`signal_input`](Engine::signal_input) reads its last frame from).
    pub fn signal_input_at(&self, k: usize, f: usize) -> Option<f64> {
        self.plan
            .cells
            .signal_in
            .get()
            .get(k * self.plan.capacity + f)
            .copied()
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
    /// Offline: wait for a latent module's due result (ADR 0019 §3).
    wait: bool,
    /// Real time: where each lane records the instance it is calling
    /// (M2.7-9, hang detection).
    hang: Option<&'x HangProbe>,
    /// Real time: carry delivered results with their records (M2.7-4).
    record_data: bool,
    /// Exact replay: the block's pushed outcomes, sorted by (instance,
    /// frame), and the queue they index (M2.7-4).
    script: Option<(&'x [StagedBoundary], &'x VecDeque<PushedBoundary>)>,
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
    let lane = access.lane();
    let Some(unit) = fx.units.order.get(u) else {
        return;
    };
    match unit {
        &Unit::Direct(i) => run_direct(fx, access, i, a, b),
        &Unit::State(i) => {
            for f in a..b {
                if let Some(mut v) = access.call(i, f, f + 1) {
                    emit(fx, &mut v, (lane, i), f);
                    absorb(fx, &mut v, (lane, i), f);
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
                        emit(fx, &mut v, (lane, i), f);
                    }
                }
                for i in direct.clone().filter_map(member) {
                    run_direct(fx, access, i, f, f + 1);
                }
                for i in state_outputs.clone().filter_map(member) {
                    if let Some(mut v) = access.call(i, f, f + 1) {
                        absorb(fx, &mut v, (lane, i), f);
                    }
                }
            }
        }
    }
}

/// Phase 3 for one direct instance over frames `a..b` of the block: its
/// inputs frame by frame, one `process` call, then its outputs; one view.
fn run_direct(fx: &Fixed<'_>, access: &mut Access<'_>, i: usize, a: usize, b: usize) {
    let lane = access.lane();
    let Some(mut v) = access.call(i, a, b) else {
        return;
    };
    if v.slot.divisor > 1 {
        divided_direct(fx, &mut v, (lane, i), a, b);
        return;
    }
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
            let completed = match v.primitive.as_mut() {
                Some(Primitive::Direct(module)) => guarded(fx.hang, (lane, i), || {
                    process(
                        module.as_mut(),
                        (&*v.slot, &mut *v.lane, &mut *v.sinks),
                        len,
                    );
                }),
                _ => true,
            };
            if !completed {
                let fault = panicked(fx, &mut running(&mut v), a);
                mute(fx, &mut gathering(&mut v), i, (a, b), rollback, fault);
                let mut r = running(&mut v);
                for f in a..b {
                    fade_muted(fx, &mut r, f);
                }
                return;
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

/// [`run_direct`] for an instance of rate divisor *d* > 1 (M2.8-6,
/// `docs/semantics.md` §16): its inputs are gathered every frame, as for
/// *d* = 1, but `process` is called once for the segment's ticks only (from
/// its first tick `t₁`), and its outputs hold its last tick's values between
/// ticks. The rollback snapshot is taken before the gather, so a fault at a
/// tick undoes what the frames after it gathered, exactly as per frame; a
/// panic acts from `t₁`.
fn divided_direct(
    fx: &Fixed<'_>,
    v: &mut CallView<'_>,
    (lane, i): (usize, usize),
    a: usize,
    b: usize,
) {
    let rollback = b - a > 1 && v.slot.mode() == Mode::Running;
    if rollback {
        save(&mut gathering(v));
    }
    match gather(fx, &mut gathering(v), a, b) {
        Mode::Running => {}
        Mode::Suspended => {
            let mut r = running(v);
            for f in a..b {
                fade_suspended(fx, &mut r, f);
            }
            return;
        }
        Mode::Muted => {
            let mut r = running(v);
            for f in a..b {
                fade_muted(fx, &mut r, f);
            }
            return;
        }
    }
    let d = v.slot.divisor;
    let first = first_tick(fx, d, a, b);
    let held = first.unwrap_or(b);
    if held > a
        && let Some(fault) = commit_hold(fx, &mut running(v), a, held)
    {
        mute(fx, &mut gathering(v), i, (a, b), rollback, fault);
        let mut r = running(v);
        for f in fault.0..b {
            fade_muted(fx, &mut r, f);
        }
        return;
    }
    let Some(t1) = first else {
        return;
    };
    let ticks = ticks_from(d, t1, b);
    let ports = (v.slot.signal_in.len(), v.slot.params.len());
    load_ticks(v.lane, (&v.signal_in, &v.param_rows), ports, (t1, d, ticks));
    let completed = match v.primitive.as_mut() {
        Some(Primitive::Direct(module)) => guarded(fx.hang, (lane, i), || {
            process(
                module.as_mut(),
                (&*v.slot, &mut *v.lane, &mut *v.sinks),
                ticks,
            );
        }),
        _ => true,
    };
    if !completed {
        let fault = panicked(fx, &mut running(v), t1);
        mute(fx, &mut gathering(v), i, (a, b), rollback, fault);
        let mut r = running(v);
        for f in t1..b {
            fade_muted(fx, &mut r, f);
        }
        return;
    }
    let mut r = running(v);
    expand(&mut r, (d, ticks), b - t1);
    if let Some(fault) = commit(fx, &mut r, t1, b) {
        mute(fx, &mut gathering(v), i, (a, b), rollback, fault);
        let mut r = running(v);
        for f in fault.0..b {
            fade_muted(fx, &mut r, f);
        }
    }
}

/// Whether block frame `f` is a tick of `slot`: its frame is a multiple of
/// the rate divisor (always, for divisor 1).
#[inline(always)]
fn is_tick(fx: &Fixed<'_>, slot: &Slot, f: usize) -> bool {
    slot.divisor <= 1 || fx.frame(f).0.is_multiple_of(slot.divisor)
}

/// The first block frame in `a..b` whose frame is a multiple of `d`.
fn first_tick(fx: &Fixed<'_>, d: u64, a: usize, b: usize) -> Option<usize> {
    let past = fx.frame(a).0 % d;
    let ahead = if past == 0 { 0 } else { d - past };
    let t = a.saturating_add(usize::try_from(ahead).unwrap_or(usize::MAX));
    (t < b).then_some(t)
}

/// The ticks of divisor `d` in `t1..b`, `t1` a tick.
fn ticks_from(d: u64, t1: usize, b: usize) -> usize {
    let d = usize::try_from(d).unwrap_or(usize::MAX).max(1);
    (b - t1 - 1) / d + 1
}

/// Copies the instance's summed signal inputs and effective parameter values
/// at its `ticks` ticks from block frame `t1` (one every `d` frames) into
/// its lane's call buffers, port-major: [`load_call`] for a divided rate.
fn load_ticks<R: Own>(
    lane: &mut Lane,
    (signal_in, param_rows): (&R, &R),
    ports: (usize, usize),
    (t1, d, ticks): (usize, u64, usize),
) {
    let d = usize::try_from(d).unwrap_or(usize::MAX).max(1);
    let pairs = [
        (ports.0, signal_in, &mut lane.call_signals),
        (ports.1, param_rows, &mut lane.call_params),
    ];
    for (count, rows, call) in pairs {
        for p in 0..count {
            let Some(to) = call.get_mut(p * ticks..(p + 1) * ticks) else {
                continue;
            };
            for (j, to) in to.iter_mut().enumerate() {
                *to = rows.get(p, t1 + j * d).unwrap_or(0.0);
            }
        }
    }
}

/// After a divided-rate call of `ticks` ticks (one every `d` frames) from a
/// tick: spreads the lane's call outputs, port-major per tick, to one value
/// per frame of the `len` frames from that tick, each frame showing its
/// latest tick, in place (from the last row and frame back, so nothing is
/// overwritten before it is read); then keeps the last tick's values in
/// the slot's `hold`.
fn expand<R>(r: &mut Run<'_, R>, (d, ticks): (u64, usize), len: usize) {
    let d = usize::try_from(d).unwrap_or(usize::MAX).max(1);
    let call = &mut r.lane.call_outputs;
    for p in (0..r.slot.hold.len()).rev() {
        for c in (0..len).rev() {
            let value = call.get(p * ticks + c / d).copied().unwrap_or(0.0);
            if let Some(to) = call.get_mut(p * len + c) {
                *to = value;
            }
        }
    }
    for (p, hold) in r.slot.hold.iter_mut().enumerate() {
        if let Some(&value) = call.get(p * len + len - 1) {
            *hold = value;
        }
    }
}

/// A divided-rate instance's frames `a..b` with no tick among them, running:
/// its outputs show its held values ([`commit`], with fade-ins as for any
/// value it writes). Returns a fault for [`mute`], as `commit` does (a held
/// value already passed the guard at its tick, so none is expected).
fn commit_hold<R: Own>(
    fx: &Fixed<'_>,
    r: &mut Run<'_, R>,
    a: usize,
    b: usize,
) -> Option<(usize, PortId, FaultReason)> {
    let len = b - a;
    for (p, &value) in r.slot.hold.iter().enumerate() {
        if let Some(row) = r.lane.call_outputs.get_mut(p * len..(p + 1) * len) {
            row.fill(value);
        }
    }
    commit(fx, r, a, b)
}

/// Phase 2 for one state-output instance at frame `f` of the block.
fn emit(fx: &Fixed<'_>, v: &mut CallView<'_>, (lane, i): (usize, usize), f: usize) {
    match v.slot.mode() {
        Mode::Running if !is_tick(fx, v.slot, f) => {
            if let Some(fault) = commit_hold(fx, &mut running(v), f, f + 1) {
                mute(fx, &mut gathering(v), i, (f, f + 1), false, fault);
                fade_muted(fx, &mut running(v), f);
            }
        }
        Mode::Running => {
            for sink in v.sinks.iter_mut() {
                sink.start_call(1);
            }
            let completed = match v.primitive.as_mut() {
                Some(Primitive::StateOutput(module)) => guarded(fx.hang, (lane, i), || {
                    if v.slot.latent {
                        await_due(fx, module.as_mut(), &*v.slot, &mut *v.out, (i, f));
                    }
                    emit_call(module.as_mut(), (&*v.slot, &mut *v.lane, &mut *v.sinks));
                }),
                _ => true,
            };
            if !completed {
                let fault = panicked(fx, &mut running(v), f);
                mute(fx, &mut gathering(v), i, (f, f + 1), false, fault);
                fade_muted(fx, &mut running(v), f);
                return;
            }
            let fault = commit(fx, &mut running(v), f, f + 1);
            keep_events(&mut running(v), f, f + 1, fault.map(|(n, _, _)| n));
            if let Some(fault) = fault {
                mute(fx, &mut gathering(v), i, (f, f + 1), false, fault);
                fade_muted(fx, &mut running(v), f);
            } else {
                hold_emitted(&mut running(v));
            }
        }
        Mode::Suspended => fade_suspended(fx, &mut running(v), f),
        Mode::Muted => fade_muted(fx, &mut running(v), f),
    }
}

/// After a divided-rate instance's `emit` at a tick: its outputs hold what
/// it wrote until its next tick. Nothing for divisor 1 (no `hold`).
#[inline(always)]
fn hold_emitted<R>(r: &mut Run<'_, R>) {
    for (hold, &value) in r.slot.hold.iter_mut().zip(&r.lane.call_outputs) {
        *hold = value;
    }
}

/// Phase 4 for one state-output instance at frame `f` of the block.
fn absorb(fx: &Fixed<'_>, v: &mut CallView<'_>, (lane, i): (usize, usize), f: usize) {
    if gather(fx, &mut gathering(v), f, f + 1) != Mode::Running || !is_tick(fx, v.slot, f) {
        return;
    }
    let ports = (v.slot.signal_in.len(), v.slot.params.len());
    load_call(v.lane, (&v.signal_in, &v.param_rows), ports, f, 1);
    if let Some(Primitive::StateOutput(module)) = v.primitive.as_mut() {
        let completed = guarded(fx.hang, (lane, i), || {
            settle(fx, module.as_mut(), v.slot);
            absorb_call(module.as_mut(), (&*v.slot, &*v.lane));
        });
        count_frame(v.slot);
        if !completed {
            absorb_panicked(fx, &mut running(v), i, f);
        }
    }
}

/// Before the `emit` of a latent instance `i` at frame `f` (ADR 0019 §3):
/// offline, wait until its due result is in, so the render stays exact; in
/// real time never wait, and give up on a result that is not in. A result
/// given up on (or lost) is a `late_result` fault: the module outputs
/// silence for that block, and the fault is logged and counted. The
/// instance is not muted. Offline waiting yields the thread; real time
/// calls `due` once, and a due result it delivers is a `Delivered` record
/// (M2.7-4). An exact replay applies the outcome pushed for the frame
/// instead (`docs/semantics.md` §15).
fn await_due(
    fx: &Fixed<'_>,
    module: &mut dyn StateOutputPrimitive,
    slot: &Slot,
    out: &mut Out,
    (i, f): (usize, usize),
) {
    if let Some((staged, queue)) = fx.script {
        let frame = fx.frame(f);
        // An outcome applies only at a due frame; one pushed for any other
        // frame stays unused, a mismatch.
        let pushed = staged
            .binary_search_by_key(&(i, frame), |s| (s.slot, s.frame))
            .ok()
            .filter(|_| due_frame(slot))
            .and_then(|k| staged.get(k))
            .and_then(|s| queue.get(s.entry));
        match pushed.map(|p| &p.outcome) {
            Some(Scripted::Missed(cause)) => {
                out.boundary.used = out.boundary.used.saturating_add(1);
                // Once, without waiting, as in real time: the module's own
                // bookkeeping (recycling) runs as it did live. The fault is
                // logged with the recorded cause.
                let _ = module.due();
                missed(fx, module, slot, out, (i, f), Some(*cause));
            }
            Some(Scripted::Delivered {
                words: Some(words),
                fnv1a64,
            }) => {
                out.boundary.used = out.boundary.used.saturating_add(1);
                if module.inject_result(words) {
                    check_hash(fx, module, out, (i, f), *fnv1a64);
                } else {
                    out.boundary.unreproduced = out.boundary.unreproduced.saturating_add(1);
                }
            }
            Some(Scripted::Delivered {
                words: None,
                fnv1a64,
            }) => {
                out.boundary.used = out.boundary.used.saturating_add(1);
                if wait_due(module) {
                    check_hash(fx, module, out, (i, f), *fnv1a64);
                } else {
                    out.boundary.unreproduced = out.boundary.unreproduced.saturating_add(1);
                    missed(fx, module, slot, out, (i, f), None);
                }
            }
            None => {
                if due_frame(slot) {
                    out.boundary.unscripted = out.boundary.unscripted.saturating_add(1);
                }
                if !wait_due(module) {
                    missed(fx, module, slot, out, (i, f), None);
                }
            }
        }
        return;
    }
    if fx.wait {
        if !wait_due(module) {
            missed(fx, module, slot, out, (i, f), None);
        }
        return;
    }
    if module.due() != Due::Ready {
        missed(fx, module, slot, out, (i, f), None);
    } else if due_frame(slot) {
        delivered(fx, module, slot, out, (i, f));
    }
}

/// Exact replay (M2.7-8): the result instance `i` shows at its due frame
/// (block frame `f`), recomputed or injected, against its recorded hash; a
/// difference is kept as the block's earliest mismatch. Allocates nothing.
fn check_hash(
    fx: &Fixed<'_>,
    module: &dyn StateOutputPrimitive,
    out: &mut Out,
    (i, f): (usize, usize),
    recorded: Option<u64>,
) {
    if let Some(recorded) = recorded
        && fnv1a64_words(module.delivered_result()) != recorded
    {
        let at = Some((fx.frame(f), i));
        out.boundary.hash_mismatch = first_of(out.boundary.hash_mismatch, at);
    }
}

/// Offline: asks `due` until it is not `Pending`, yielding the thread;
/// whether the result is in.
fn wait_due(module: &mut dyn StateOutputPrimitive) -> bool {
    loop {
        match module.due() {
            Due::Ready => return true,
            Due::Pending => std::thread::yield_now(),
            Due::Lost => return false,
        }
    }
}

/// Whether the latent instance's next frame is the first of a block whose
/// result is due: its `latent_frames` (frames run since it was prepared) at
/// a block boundary, from block *L* on (`module-contract.md`).
fn due_frame(slot: &Slot) -> bool {
    slot.latency.is_some_and(|l| {
        let block = u64::from(l.block_frames.max(1));
        slot.latent_frames.is_multiple_of(block)
            && slot.latent_frames / block >= u64::from(l.latency_blocks)
    })
}

/// The module misses its due result: `miss`, counted and logged as a
/// `late_result` fault, with the cause an exact replay recorded
/// (`scripted`), else the one the module gives for its last `due` (asked
/// before `miss`, which may forget it).
fn missed(
    fx: &Fixed<'_>,
    module: &mut dyn StateOutputPrimitive,
    slot: &Slot,
    out: &mut Out,
    (i, f): (usize, usize),
    scripted: Option<Option<LossCause>>,
) {
    let cause = scripted.unwrap_or_else(|| module.lost_cause());
    module.miss();
    out.late_results = out.late_results.saturating_add(1);
    let port = fx
        .output_ports
        .get(slot.signal_out.start)
        .or_else(|| fx.sink_ports.get(slot.event_out.start))
        .copied()
        .unwrap_or(PortId(usize::MAX));
    let record = LogRecord::Fault {
        frame: fx.frame(f),
        instance: InstanceId(i),
        port,
        reason: FaultReason::LateResult,
        cause,
    };
    push_record(&mut out.log, &mut out.log_overflows, record);
}

/// Real time: the due result was delivered. Counted, and recorded as a
/// `Delivered` record with the result's hash (M2.7-8, always), and with the
/// result's words when they are carried and fit the block's buffer
/// (allocates nothing).
fn delivered(
    fx: &Fixed<'_>,
    module: &mut dyn StateOutputPrimitive,
    slot: &Slot,
    out: &mut Out,
    (i, f): (usize, usize),
) {
    out.boundary.delivered = out.boundary.delivered.saturating_add(1);
    let result = module.delivered_result();
    let fnv1a64 = fnv1a64_words(result);
    let (mut at, mut words, mut lost) = (0, 0, false);
    if fx.record_data && slot.result_words > 0 {
        let data = &mut out.boundary_data;
        if data.len() + result.len() <= data.capacity() {
            at = data.len() as u32;
            words = result.len() as u32;
            data.extend_from_slice(result);
        } else {
            lost = true;
            out.boundary.data_overflows = out.boundary.data_overflows.saturating_add(1);
        }
    }
    let record = LogRecord::Delivered {
        frame: fx.frame(f),
        instance: InstanceId(i),
        at,
        words,
        lost,
        fnv1a64,
    };
    push_record(&mut out.log, &mut out.log_overflows, record);
}

/// After an `absorb` of a latent instance: one more of its frames run.
fn count_frame(slot: &mut Slot) {
    if slot.latency.is_some() {
        slot.latent_frames = slot.latent_frames.saturating_add(1);
    }
}

/// Exact replay, before an `absorb` of a latent instance: yields while the
/// module cannot yet accept the work `absorb` may submit
/// ([`StateOutputPrimitive::settle`]).
fn settle(fx: &Fixed<'_>, module: &mut dyn StateOutputPrimitive, slot: &Slot) {
    if fx.script.is_none() || slot.latency.is_none() {
        return;
    }
    while module.settle() == Due::Pending {
        std::thread::yield_now();
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
    /// An instance's parts were not in the buffers ([`has`](Whole::has)): a
    /// broken invariant, which stops the engine (`docs/semantics.md` §14).
    refused: bool,
    /// Test hooks only: the instance whose parts are refused.
    #[cfg(feature = "test-hooks")]
    refuse_view: Option<usize>,
}

impl Whole<'_> {
    /// Whether instance `i`'s parts are all in the plan's buffers, as they
    /// are in a correct run; otherwise a refusal, as a view's would be.
    #[inline(always)]
    fn has(&mut self, i: usize) -> bool {
        let k = self.k;
        let has = self.slots.get(i).is_some_and(|s| {
            s.params.end <= self.params.len()
                && s.signal_out.end <= self.held.len()
                && s.event_out.end <= self.sinks.len().min(self.emitted.len())
                && s.signal_out.end.saturating_mul(k) <= self.outputs.len()
                && s.signal_in.end.saturating_mul(k) <= self.signal_in.len()
                && s.params.end.saturating_mul(k) <= self.param_rows.len()
        });
        #[cfg(feature = "test-hooks")]
        let has = has && self.refuse_view != Some(i);
        if !has {
            self.refused = true;
        }
        has
    }

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

    /// Copies instance `i`'s inputs at its `ticks` ticks from `t1` (one
    /// every `d` frames) into the lane ([`load_ticks`]).
    fn load_ticks(&mut self, i: usize, ticks: (usize, u64, usize)) {
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
        load_ticks(self.lane, (&signal_in, &param_rows), ports, ticks);
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
    if !w.has(i) {
        return;
    }
    if w.slots.get(i).is_some_and(|s| s.divisor > 1) {
        whole_divided(fx, w, i, a, b);
        return;
    }
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
            let completed = match w.primitives.get_mut(i) {
                Some(Some(Primitive::Direct(module))) => {
                    let lane = &mut *w.lane;
                    guarded(fx.hang, (0, i), || {
                        process(module.as_mut(), (slot, lane, sinks), len);
                    })
                }
                _ => true,
            };
            if !completed {
                let Some(mut r) = w.running(i) else {
                    return;
                };
                let fault = panicked(fx, &mut r, a);
                w.mute(fx, i, (a, b), rollback, fault);
                return;
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

/// [`divided_direct`] for a sequential segment.
fn whole_divided(fx: &Fixed<'_>, w: &mut Whole<'_>, i: usize, a: usize, b: usize) {
    let Some(slot) = w.slots.get(i) else {
        return;
    };
    let (d, rollback) = (slot.divisor, b - a > 1 && slot.mode() == Mode::Running);
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
        Mode::Running => {}
        Mode::Suspended => {
            if let Some(mut r) = w.running(i) {
                for f in a..b {
                    fade_suspended(fx, &mut r, f);
                }
            }
            return;
        }
        Mode::Muted => {
            if let Some(mut r) = w.running(i) {
                for f in a..b {
                    fade_muted(fx, &mut r, f);
                }
            }
            return;
        }
    }
    let first = first_tick(fx, d, a, b);
    let held = first.unwrap_or(b);
    if held > a {
        let Some(mut r) = w.running(i) else {
            return;
        };
        if let Some(fault) = commit_hold(fx, &mut r, a, held) {
            w.mute(fx, i, (a, b), rollback, fault);
            return;
        }
    }
    let Some(t1) = first else {
        return;
    };
    let ticks = ticks_from(d, t1, b);
    w.load_ticks(i, (t1, d, ticks));
    let completed = match (w.slots.get(i), w.primitives.get_mut(i)) {
        (Some(slot), Some(Some(Primitive::Direct(module)))) => {
            let sinks = w.sinks.get_mut(slot.event_out.clone()).unwrap_or(&mut []);
            let lane = &mut *w.lane;
            guarded(fx.hang, (0, i), || {
                process(module.as_mut(), (slot, lane, sinks), ticks);
            })
        }
        _ => true,
    };
    let Some(mut r) = w.running(i) else {
        return;
    };
    if !completed {
        let fault = panicked(fx, &mut r, t1);
        w.mute(fx, i, (a, b), rollback, fault);
        return;
    }
    expand(&mut r, (d, ticks), b - t1);
    if let Some(fault) = commit(fx, &mut r, t1, b) {
        w.mute(fx, i, (a, b), rollback, fault);
    }
}

/// [`emit`] for a sequential segment.
fn whole_emit(fx: &Fixed<'_>, w: &mut Whole<'_>, i: usize, f: usize) {
    if !w.has(i) {
        return;
    }
    let Some(slot) = w.slots.get(i) else {
        return;
    };
    match slot.mode() {
        Mode::Running if !is_tick(fx, slot, f) => {
            let Some(mut r) = w.running(i) else {
                return;
            };
            if let Some(fault) = commit_hold(fx, &mut r, f, f + 1) {
                w.mute(fx, i, (f, f + 1), false, fault);
            }
        }
        Mode::Running => {
            let sinks = w.sinks.get_mut(slot.event_out.clone()).unwrap_or(&mut []);
            for sink in sinks.iter_mut() {
                sink.start_call(1);
            }
            let completed = match w.primitives.get_mut(i) {
                Some(Some(Primitive::StateOutput(module))) => {
                    let (lane, out) = (&mut *w.lane, &mut *w.out);
                    guarded(fx.hang, (0, i), || {
                        if slot.latent {
                            await_due(fx, module.as_mut(), slot, out, (i, f));
                        }
                        emit_call(module.as_mut(), (slot, lane, sinks));
                    })
                }
                _ => true,
            };
            if !completed {
                let Some(mut r) = w.running(i) else {
                    return;
                };
                let fault = panicked(fx, &mut r, f);
                w.mute(fx, i, (f, f + 1), false, fault);
                return;
            }
            let fault = {
                let Some(mut r) = w.running(i) else {
                    return;
                };
                let fault = commit(fx, &mut r, f, f + 1);
                keep_events(&mut r, f, f + 1, fault.map(|(n, _, _)| n));
                if fault.is_none() {
                    hold_emitted(&mut r);
                }
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
    if !w.has(i) {
        return;
    }
    let mode = {
        let Some(mut g) = w.gathering(i) else {
            return;
        };
        gather(fx, &mut g, f, f + 1)
    };
    if mode != Mode::Running || w.slots.get(i).is_some_and(|s| !is_tick(fx, s, f)) {
        return;
    }
    w.load_call(i, f, 1);
    let completed = match (w.slots.get(i), w.primitives.get_mut(i)) {
        (Some(slot), Some(Some(Primitive::StateOutput(module)))) => {
            let lane = &*w.lane;
            guarded(fx.hang, (0, i), || {
                settle(fx, module.as_mut(), slot);
                absorb_call(module.as_mut(), (slot, lane));
            })
        }
        _ => true,
    };
    if let Some(slot) = w.slots.get_mut(i) {
        count_frame(slot);
    }
    if !completed && let Some(mut r) = w.running(i) {
        absorb_panicked(fx, &mut r, i, f);
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
    // (computed even when the instance is not called: a sink's inputs are
    // what the drivers hand to their output). Adding one source at a time over
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

/// Runs one call of instance `i` of a primitive on lane `lane`; false if it
/// panicked. The panic goes no further, nor does one from its payload's
/// destructor ([`caught`]): the caller mutes the instance
/// (`docs/semantics.md` §14). Nothing of the call is trusted, so nothing it
/// wrote is read again. In real time the lane's slot of the hang probe names
/// the instance while the call runs (M2.7-9): a call that never returns is
/// not contained, only reported.
#[inline(always)]
fn guarded(hang: Option<&HangProbe>, (lane, i): (usize, usize), call: impl FnOnce()) -> bool {
    if let Some(hang) = hang {
        hang.enter(lane, i);
    }
    let completed = caught(call);
    if let Some(hang) = hang {
        hang.leave(lane);
    }
    completed
}

/// After a `process` or `emit` call over frames `a..` panicked: none of its
/// outputs or events is published, and the outputs fade from what readers
/// saw at `a − 1`, as for a fault found at `a` ([`commit`]). Returns the
/// fault for [`mute`]: frame `a`, the first output port (signal, then event;
/// none for an instance without outputs), reason `panic`.
fn panicked<R: Own>(fx: &Fixed<'_>, r: &mut Run<'_, R>, a: usize) -> (usize, PortId, FaultReason) {
    for (p, o) in r.slot.signal_out.clone().enumerate() {
        let visible = previous(fx.outputs, &r.outputs, (o, p), a);
        if let Some(held) = r.held.get_mut(p) {
            *held = visible;
        }
    }
    (a, first_port(fx, r.slot), FaultReason::Panic)
}

/// After an `absorb` at frame `f` panicked: frame `f`'s outputs were already
/// published by its `emit` and stand; the instance is muted from `f + 1`,
/// its outputs fading from those of `f`, and the fault is logged at `f`.
fn absorb_panicked<R: Own>(fx: &Fixed<'_>, r: &mut Run<'_, R>, i: usize, f: usize) {
    for p in 0..r.slot.signal_out.len() {
        let visible = r.outputs.get(p, f).unwrap_or(0.0);
        if let Some(held) = r.held.get_mut(p) {
            *held = visible;
        }
    }
    r.slot.fault = Some(0);
    let record = LogRecord::Fault {
        frame: fx.frame(f),
        instance: InstanceId(i),
        port: first_port(fx, r.slot),
        reason: FaultReason::Panic,
        cause: None,
    };
    push_record(&mut r.out.log, &mut r.out.log_overflows, record);
}

/// An instance's first output port, signal before event, for a fault that no
/// output caused; `PortId(usize::MAX)` (no port in the log line) if it has
/// none.
fn first_port(fx: &Fixed<'_>, slot: &Slot) -> PortId {
    fx.output_ports
        .get(slot.signal_out.start)
        .filter(|_| !slot.signal_out.is_empty())
        .or_else(|| {
            fx.sink_ports
                .get(slot.event_out.start)
                .filter(|_| !slot.event_out.is_empty())
        })
        .copied()
        .unwrap_or(PortId(usize::MAX))
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
        cause: None,
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
