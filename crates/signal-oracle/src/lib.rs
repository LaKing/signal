//! The semantic oracle (M2.7-6, `docs/testing-and-evaluation.md`): a
//! deliberately simple interpreter of the per-frame semantics of ADR 0016,
//! as `docs/semantics.md` §§1–12 states them, against which the block engine
//! is compared bit for bit.
//!
//! It runs a graph one frame at a time, every frame in the four phases of
//! §2 (external input, emit, process, absorb), and calls every primitive for
//! one frame. It is written to be read, not to be fast, and it shares no code
//! with `signal-engine`: its input is its own ([`Graph`], [`Script`]), in
//! `signal-core` terms; it sorts sources and orders instances itself, and
//! takes everything else from the module descriptors and the primitives a
//! [`Registry`](signal_core::Registry) creates.
//!
//! Rate divisors and vector ports (M2.8-6, `docs/semantics.md` §16): an
//! instance is called at its ticks only and shows its last tick's values
//! between them; a vector port is one value per element. The oracle inserts
//! the converters a graph's cross-rate connections need itself, from the
//! rule §16 states (their addresses, type names and wiring), and takes their
//! types from the registry like any other.
//!
//! Not covered: latent (GPU) modules, input pushed late or through the
//! real-time runner, input naming no port of the plan (dropped here, counted
//! by the engine), and the frame of a panic in a block call
//! (`docs/semantics.md` §14: it depends on the block size).

mod oracle;
mod probe;

use signal_core::{EventPayload, InstanceAddress, PortAddress};

pub use oracle::{OracleError, run};
pub use probe::{PROBE_INPUTS, register_probe};

/// A flat graph: primitive instances and the connections between their
/// ports, in any order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Graph {
    /// The plan version a swap to this graph records.
    pub version: u64,
    /// Every instance.
    pub instances: Vec<Instance>,
    /// Every connection, from an output port to an input port or parameter.
    pub connections: Vec<Connection>,
}

/// A primitive instance.
#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    /// Its address.
    pub address: InstanceAddress,
    /// Its registered type.
    pub type_name: String,
    /// Initial parameter bases by name; the descriptor's default otherwise.
    pub parameters: Vec<(String, f64)>,
}

/// A connection; `scale` multiplies a signal (events are not scaled).
#[derive(Clone, Debug, PartialEq)]
pub struct Connection {
    /// The output port.
    pub from: PortAddress,
    /// The input port or parameter.
    pub to: PortAddress,
    /// The factor.
    pub scale: f64,
}

/// What happens to a graph over a run: its external events, its suspend and
/// resume edits, and plan swaps, each at the frame it is stamped for.
#[derive(Clone, Debug, PartialEq)]
pub struct Script {
    /// Frames per second.
    pub sample_rate: u32,
    /// The master seed.
    pub seed: u64,
    /// The frames to run.
    pub frames: u64,
    /// External events.
    pub events: Vec<ExternalEvent>,
    /// Suspends and resumes, in the order they are scheduled.
    pub edits: Vec<Edit>,
    /// Plan swaps, at most one per frame.
    pub swaps: Vec<Swap>,
}

/// An external event.
#[derive(Clone, Debug, PartialEq)]
pub struct ExternalEvent {
    /// The frame it is stamped for.
    pub frame: u64,
    /// Its sequence number, unique in the run.
    pub sequence: u64,
    /// An event input or a parameter.
    pub target: PortAddress,
    /// The event.
    pub payload: EventPayload,
}

/// A suspend (`resume` false) or resume of an instance.
#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    /// The frame it takes effect.
    pub frame: u64,
    /// The instance.
    pub instance: InstanceAddress,
    /// A resume, else a suspend.
    pub resume: bool,
}

/// A plan swap, effective from `frame`.
#[derive(Clone, Debug, PartialEq)]
pub struct Swap {
    /// The frame.
    pub frame: u64,
    /// The graph swapped in.
    pub graph: Graph,
}

/// What a run produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    /// The audio sink's signal inputs per frame (times its `gain`, if its type
    /// declares one), interleaved: what the offline driver records (§8).
    pub samples: Vec<f64>,
    /// Signal inputs of the sink.
    pub channels: usize,
    /// The log, frame by frame, each frame's records in the order of §9.
    pub log: Vec<Record>,
    /// After the last frame, per instance of the last plan.
    pub instances: Vec<InstanceEnd>,
    /// After the last frame, per parameter of the last plan.
    pub parameters: Vec<ParameterEnd>,
}

/// An instance's state after the run.
#[derive(Clone, Debug, PartialEq)]
pub struct InstanceEnd {
    /// The instance.
    pub address: InstanceAddress,
    /// Whether it is suspended.
    pub suspended: bool,
    /// Whether the fault guard muted it.
    pub muted: bool,
    /// Events that reached it and had no effect.
    pub discards: u64,
}

/// A parameter's values after the run.
#[derive(Clone, Debug, PartialEq)]
pub struct ParameterEnd {
    /// The parameter.
    pub port: PortAddress,
    /// Its base.
    pub base: f64,
    /// Its effective value in the last frame.
    pub effective: f64,
}

/// A log record (`docs/semantics.md` §9).
#[derive(Clone, Debug, PartialEq)]
pub enum Record {
    /// A plan swap.
    Swap {
        /// The frame.
        frame: u64,
        /// The plan version after it.
        version: u64,
        /// The swapped-in graph's version.
        plan_version: u64,
    },
    /// An event that took effect.
    Event {
        /// The frame.
        frame: u64,
        /// The receiving port or parameter.
        target: PortAddress,
        /// The event.
        payload: EventPayload,
        /// Where it came from.
        source: Source,
    },
    /// An event that had no effect.
    Discard {
        /// The frame.
        frame: u64,
        /// The receiving port or parameter.
        target: PortAddress,
        /// The event.
        payload: EventPayload,
        /// Where it came from.
        source: Source,
        /// Why.
        reason: DiscardReason,
    },
    /// A suspend.
    Suspend {
        /// The frame.
        frame: u64,
        /// The instance.
        address: InstanceAddress,
    },
    /// A resume.
    Resume {
        /// The frame.
        frame: u64,
        /// The instance.
        address: InstanceAddress,
        /// Events discarded while it was suspended.
        discarded: u64,
    },
    /// A fault.
    Fault {
        /// The frame.
        frame: u64,
        /// The instance.
        address: InstanceAddress,
        /// Its first offending output, or its first output (none without).
        port: Option<PortAddress>,
        /// Why.
        reason: FaultReason,
    },
}

/// Where an event came from.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// The script, with its sequence number.
    External(u64),
    /// An event output.
    Port(PortAddress),
}

/// Why an event had no effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscardReason {
    /// The receiver is suspended.
    Suspended,
    /// The receiver is muted.
    Muted,
    /// A trigger reached a parameter.
    NotAParameterEvent,
}

/// Why the fault guard muted an instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultReason {
    /// A NaN or infinite output.
    NonFinite,
    /// An output above 10⁶ in magnitude.
    Magnitude,
    /// More than 16 events on an event output in a frame.
    EventOverflow,
    /// A call panicked.
    Panic,
}
