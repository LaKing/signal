//! The event log: what the executor did in a frame. Records are `Copy` and name
//! ports and instances by index, so the executor writes them without allocating;
//! [`LogLine`] is their `log.jsonl` form (`docs/semantics.md`, "Artifacts").

use serde::Serialize;
use signal_core::{EventPayload, Frame, InstanceAddress, LossCause, PortAddress};

/// A port of the engine's plan. Ids follow the byte-wise order of port addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PortId(pub(crate) usize);

/// An instance of the engine's plan. Ids follow the order of instance addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceId(pub(crate) usize);

/// Where an event came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogSource {
    /// The caller's external list, with its sequence number.
    External {
        /// The event's sequence number.
        sequence: u64,
    },
    /// An event output port of a module, along a connection.
    Port(PortId),
}

/// Why an event had no effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscardReason {
    /// The receiving instance is suspended and the event is not a `set` or
    /// `ramp` to a parameter (ADR 0016 §1a).
    Suspended,
    /// The receiving instance was muted by the fault guard.
    Muted,
    /// A trigger reached a parameter, which only `set` and `ramp` change.
    NotAParameterEvent,
}

/// Why the fault guard muted an instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultReason {
    /// An output sample or emitted event value is NaN or infinite.
    NonFinite,
    /// An output sample or emitted event value has magnitude above 10⁶.
    Magnitude,
    /// An event output overflowed its sink (more than 16 events in a frame).
    EventOverflow,
    /// A module with a declared latency (ADR 0019 §3) did not have its result
    /// when due, in real time (offline the engine waits), or the result was
    /// lost. Its output is silence for that block of its own; it is not muted.
    LateResult,
    /// A call of the instance (`process`, `emit`, `absorb`, `due` or `miss`)
    /// panicked (`docs/semantics.md` §14): it is muted from the call's first
    /// frame, as for the reasons above.
    Panic,
}

/// Why the engine stopped (`docs/semantics.md` §14): never an instance's
/// fault, always the engine's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureReason {
    /// Something unwound in a task outside a primitive's call.
    TaskPanic,
    /// A view, a read or the task board refused an access that a correct run
    /// never makes.
    InvariantViolation,
}

/// An engine-level failure: the engine stopped in the block from `frame`,
/// published nothing of it, and runs no further frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct EngineFailure {
    /// The first frame of the segment that failed.
    pub frame: Frame,
    /// Why.
    pub reason: FailureReason,
}

impl std::fmt::Display for EngineFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let why = match self.reason {
            FailureReason::TaskPanic => "a task panicked outside a module's call",
            FailureReason::InvariantViolation => "an access invariant was violated",
        };
        write!(f, "the engine stopped at frame {}: {why}", self.frame.0)
    }
}

/// One thing the executor did in a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LogRecord {
    /// A plan swap, effective from this frame (`docs/semantics.md` §12).
    Swap {
        /// The frame.
        frame: Frame,
        /// The engine's plan version after the swap: 0 for the first plan,
        /// plus one per swap.
        version: u64,
        /// The swapped-in plan's own [`ExecutionPlan::version`](signal_graph::ExecutionPlan::version).
        plan_version: u64,
    },
    /// An event delivered to an input port or applied to a parameter.
    Event {
        /// The frame.
        frame: Frame,
        /// The receiving port or parameter.
        target: PortId,
        /// The event.
        payload: EventPayload,
        /// Where it came from.
        source: LogSource,
    },
    /// An event that reached its target and had no effect.
    Discard {
        /// The frame.
        frame: Frame,
        /// The receiving port or parameter.
        target: PortId,
        /// The event.
        payload: EventPayload,
        /// Where it came from.
        source: LogSource,
        /// Why it had no effect.
        reason: DiscardReason,
    },
    /// An instance was suspended, effective from this frame.
    Suspend {
        /// The frame.
        frame: Frame,
        /// The instance.
        instance: InstanceId,
    },
    /// An instance resumed, effective from this frame.
    Resume {
        /// The frame.
        frame: Frame,
        /// The instance.
        instance: InstanceId,
        /// Events discarded while it was suspended.
        discarded: u64,
    },
    /// The fault guard muted an instance, or (`LateResult`) a latent module
    /// missed a due result and outputs silence for that block.
    Fault {
        /// The frame of the faulty call.
        frame: Frame,
        /// The instance.
        instance: InstanceId,
        /// The first output found at fault, in descriptor order.
        port: PortId,
        /// What was wrong.
        reason: FaultReason,
        /// For a `LateResult` whose result was lost, why, when the module
        /// knew (M2.7-5).
        cause: Option<LossCause>,
    },
    /// Real time (M2.7-4): a latent instance's due result was delivered.
    /// Not a line of `log.jsonl`: the [`Controller`](crate::Controller)
    /// hands it to [`ControlSink::boundary`](crate::ControlSink::boundary), as
    /// it does a `late_result` fault, the missed outcome.
    Delivered {
        /// The due frame: the first of the instance's block.
        frame: Frame,
        /// The instance.
        instance: InstanceId,
        /// Where its data is in the block's delivered results (engine-internal).
        at: u32,
        /// The words of delivered result carried with it: 0 without data.
        words: u32,
        /// Its data was recorded but did not fit a buffer or queue.
        lost: bool,
    },
}

impl LogRecord {
    /// The frame it belongs to.
    pub fn frame(&self) -> Frame {
        match *self {
            LogRecord::Swap { frame, .. }
            | LogRecord::Event { frame, .. }
            | LogRecord::Discard { frame, .. }
            | LogRecord::Suspend { frame, .. }
            | LogRecord::Resume { frame, .. }
            | LogRecord::Fault { frame, .. }
            | LogRecord::Delivered { frame, .. } => frame,
        }
    }

    /// The order of records within a frame in `log.jsonl`: a swap, then edits
    /// by instance, then events and discards by target port (a stable sort
    /// keeps their delivery order), then faults by instance. It does not depend
    /// on the schedule.
    pub(crate) fn order(&self) -> (u8, usize) {
        match *self {
            LogRecord::Swap { .. } => (0, 0),
            LogRecord::Suspend { instance, .. } | LogRecord::Resume { instance, .. } => {
                (1, instance.0)
            }
            LogRecord::Event { target, .. } | LogRecord::Discard { target, .. } => (2, target.0),
            LogRecord::Fault { instance, .. } => (3, instance.0),
            LogRecord::Delivered { instance, .. } => (4, instance.0),
        }
    }
}

/// A [`LogRecord`] as one line of `log.jsonl`, with addresses.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LogLine<'a> {
    /// See [`LogRecord::Swap`].
    Swap {
        /// The frame.
        frame: Frame,
        /// The engine's plan version after the swap.
        version: u64,
        /// The swapped-in plan's own version.
        plan_version: u64,
    },
    /// See [`LogRecord::Event`].
    Event {
        /// The frame.
        frame: Frame,
        /// The receiving port or parameter.
        target: &'a PortAddress,
        /// The event.
        payload: EventPayload,
        /// Where it came from.
        source: LineSource<'a>,
    },
    /// See [`LogRecord::Discard`].
    Discard {
        /// The frame.
        frame: Frame,
        /// The receiving port or parameter.
        target: &'a PortAddress,
        /// The event.
        payload: EventPayload,
        /// Where it came from.
        source: LineSource<'a>,
        /// Why it had no effect.
        reason: DiscardReason,
    },
    /// See [`LogRecord::Suspend`].
    Suspend {
        /// The frame.
        frame: Frame,
        /// The instance.
        address: &'a InstanceAddress,
    },
    /// See [`LogRecord::Resume`].
    Resume {
        /// The frame.
        frame: Frame,
        /// The instance.
        address: &'a InstanceAddress,
        /// Events discarded while it was suspended.
        discarded: u64,
    },
    /// See [`LogRecord::Fault`].
    Fault {
        /// The frame.
        frame: Frame,
        /// The instance.
        address: &'a InstanceAddress,
        /// The first output found at fault; for an instance without outputs,
        /// none (the line leaves the field out).
        #[serde(skip_serializing_if = "Option::is_none")]
        port: Option<&'a PortAddress>,
        /// What was wrong.
        reason: FaultReason,
        /// For a `late_result` whose result was lost, why (M2.7-5); the
        /// line leaves the field out when there is none.
        #[serde(skip_serializing_if = "Option::is_none")]
        cause: Option<LossCause>,
    },
}

/// A [`LogSource`] with addresses: `{"external": {"sequence": n}}` or
/// `{"port": "/a.out"}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LineSource<'a> {
    /// From the external list.
    External {
        /// The event's sequence number.
        sequence: u64,
    },
    /// From a module's event output.
    Port(&'a PortAddress),
}
