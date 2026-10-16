//! What can go wrong when instantiating a plan, scheduling an edit, swapping
//! plans, or running offline. Nothing goes wrong inside a frame: every check happens
//! before, except an engine failure (`docs/semantics.md` §14), which stops it.

use std::fmt;

use signal_core::{Frame, InstanceAddress, MAX_VECTOR_WIDTH, PortAddress};

/// Why a plan cannot be instantiated, an external list is refused, or an edit
/// cannot be scheduled.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum EngineError {
    /// The sample rate is 0.
    InvalidSampleRate,
    /// The block capacity is 0.
    InvalidBlockCapacity,
    /// The registry has no type of this name.
    UnknownModuleType {
        /// The instance.
        instance: InstanceAddress,
        /// Its type name.
        type_name: &'static str,
    },
    /// The registry's descriptor for the type differs from the plan's.
    DescriptorMismatch {
        /// The instance.
        instance: InstanceAddress,
    },
    /// The created primitive's class differs from its descriptor's feedthrough.
    FeedthroughMismatch {
        /// The instance.
        instance: InstanceAddress,
    },
    /// A port or parameter name is not `[a-z][a-z0-9_]*`.
    InvalidPortName {
        /// The instance.
        instance: InstanceAddress,
        /// The name.
        name: &'static str,
    },
    /// The type's ports and parameters have no one rate divisor
    /// (`docs/semantics.md` §16): its outputs, parameters and event ports
    /// differ, or a signal input's divisor neither divides nor is a multiple
    /// of theirs.
    MixedRates {
        /// The instance.
        instance: InstanceAddress,
    },
    /// A type at a rate divisor above 1 has what runs only at divisor 1:
    /// event ports, a declared latency or a sink (M2.8-6).
    UnsupportedRate {
        /// The instance.
        instance: InstanceAddress,
        /// What it has.
        what: &'static str,
    },
    /// A port has a shape this engine does not know.
    UnsupportedShape {
        /// The port.
        port: PortAddress,
    },
    /// A vector port is wider than
    /// [`MAX_VECTOR_WIDTH`](signal_core::MAX_VECTOR_WIDTH) (`docs/semantics.md`
    /// §16); refused before anything is allocated for it.
    VectorTooWide {
        /// The port.
        port: PortAddress,
        /// Its elements.
        width: u32,
    },
    /// The plan's frame-sized buffers would take more than their budget at
    /// this block capacity: its value buffers more than
    /// [`MAX_PREPARED_BYTES`](crate::MAX_PREPARED_BYTES), or all of them more
    /// than [`MAX_RESERVED_BYTES`](crate::MAX_RESERVED_BYTES)
    /// (`docs/semantics.md` §16). Refused before any instance is created or
    /// buffer allocated.
    PlanTooLarge {
        /// Which buffers: `value buffers` or `buffers in all`.
        what: &'static str,
        /// The estimate, in bytes; `None` when it overflows.
        bytes: Option<usize>,
        /// The block capacity it was estimated at.
        capacity: usize,
        /// The budget, in bytes.
        budget: usize,
    },
    /// A plan source is not an output of a shape its target accepts.
    InvalidConnection {
        /// The source.
        from: PortAddress,
        /// The target.
        to: PortAddress,
    },
    /// The plan's schedule is not an order of its instances that runs every
    /// direct source before its receivers (ADR 0016 §1, §2).
    InvalidSchedule,
    /// An external event names a port the plan does not have.
    UnknownTarget {
        /// The target.
        target: PortAddress,
    },
    /// An external event names a port that takes no events: an output or a
    /// signal input.
    NotAnEventTarget {
        /// The target.
        target: PortAddress,
    },
    /// An external `set` or `ramp` value is NaN or infinite.
    NonFiniteValue {
        /// The event's sequence number.
        sequence: u64,
    },
    /// Two external events have the same sequence number.
    DuplicateSequence {
        /// The sequence number.
        sequence: u64,
    },
    /// An edit names an instance the plan does not have.
    UnknownInstance {
        /// The instance.
        instance: InstanceAddress,
    },
    /// An edit is stamped for a frame that already ran.
    FrameInPast {
        /// The edit's frame.
        frame: Frame,
        /// The next frame to run.
        next: Frame,
    },
    /// A suspend of an instance that is (or will be) suspended, a resume of one
    /// that is not, or an edit not after the instance's previous edit.
    InvalidEdit {
        /// The instance.
        instance: InstanceAddress,
        /// The edit's frame.
        frame: Frame,
    },
    /// A swap committed for a frame other than the next one.
    SwapFrame {
        /// The swap's frame.
        frame: Frame,
        /// The next frame to run.
        next: Frame,
    },
    /// A prepared swap committed already, or prepared at another plan version.
    StaleSwap,
    /// A second swap committed before the frame of the first one ran.
    SwapPending,
    /// A swap committed before the last frame's log was drained.
    LogNotDrained,
    /// A swap prepared by another engine.
    ForeignSwap,
    /// An instance of a type with a declared latency exposes no result
    /// (`result_words` 0): its delivered results could not be hashed, so a
    /// replay could never show them exact (M2.7-8).
    LatentWithoutResult {
        /// The instance.
        instance: InstanceAddress,
    },
    /// Exact replay: a swap committed before the hash mismatch of an earlier
    /// block was taken (its instance names the old plan's).
    MismatchNotTaken,
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::InvalidSampleRate => f.write_str("the sample rate must be at least 1"),
            EngineError::InvalidBlockCapacity => {
                f.write_str("the block capacity must be at least 1")
            }
            EngineError::UnknownModuleType {
                instance,
                type_name,
            } => write!(f, "{instance}: module type `{type_name}` is not registered"),
            EngineError::DescriptorMismatch { instance } => write!(
                f,
                "{instance}: the registry's descriptor differs from the plan's"
            ),
            EngineError::FeedthroughMismatch { instance } => write!(
                f,
                "{instance}: the primitive's class differs from its declared feedthrough"
            ),
            EngineError::InvalidPortName { instance, name } => {
                write!(f, "{instance}: invalid port name `{name}`")
            }
            EngineError::MixedRates { instance } => write!(
                f,
                "{instance}: its ports and parameters have no one rate divisor (outputs, parameters and event ports share one; a signal input's divides or is a multiple of it)"
            ),
            EngineError::UnsupportedRate { instance, what } => write!(
                f,
                "{instance}: a type at a rate divisor above 1 cannot have {what} (they run at rate divisor 1 only)"
            ),
            EngineError::UnsupportedShape { port } => write!(f, "{port}: unsupported port shape"),
            EngineError::VectorTooWide { port, width } => write!(
                f,
                "{port}: a vector of {width} elements; a vector has at most {MAX_VECTOR_WIDTH}"
            ),
            EngineError::PlanTooLarge {
                what,
                bytes,
                capacity,
                budget,
            } => {
                write!(
                    f,
                    "the plan's {what} at a block capacity of {capacity} frames take "
                )?;
                match bytes {
                    Some(bytes) => write!(f, "{bytes} bytes")?,
                    None => f.write_str("more bytes than can be counted")?,
                }
                write!(f, "; their budget is {budget} bytes")
            }
            EngineError::InvalidConnection { from, to } => {
                write!(f, "{from} -> {to}: not a connection this engine can run")
            }
            EngineError::InvalidSchedule => {
                f.write_str("the schedule is not a topological order of the plan's instances")
            }
            EngineError::UnknownTarget { target } => {
                write!(f, "external event: unknown target {target}")
            }
            EngineError::NotAnEventTarget { target } => write!(
                f,
                "external event: {target} is not an event input or a parameter"
            ),
            EngineError::NonFiniteValue { sequence } => {
                write!(f, "external event {sequence}: value is not finite")
            }
            EngineError::DuplicateSequence { sequence } => {
                write!(f, "external event sequence {sequence} is used twice")
            }
            EngineError::UnknownInstance { instance } => {
                write!(f, "edit: unknown instance {instance}")
            }
            EngineError::FrameInPast { frame, next } => write!(
                f,
                "edit at frame {} is in the past; the next frame is {}",
                frame.0, next.0
            ),
            EngineError::InvalidEdit { instance, frame } => write!(
                f,
                "{instance}: suspend and resume must alternate at increasing frames (frame {})",
                frame.0
            ),
            EngineError::SwapFrame { frame, next } => {
                write!(f, "swap at frame {}: the next frame is {}", frame.0, next.0)
            }
            EngineError::StaleSwap => {
                f.write_str("the swap was prepared at another plan version or already committed")
            }
            EngineError::SwapPending => {
                f.write_str("a swap is already committed for the next frame")
            }
            EngineError::ForeignSwap => f.write_str("the swap was prepared by another engine"),
            EngineError::LogNotDrained => {
                f.write_str("drain the last frame's log before committing a swap")
            }
            EngineError::LatentWithoutResult { instance } => write!(
                f,
                "{instance}: a module type with a declared latency must expose its result (result_words of at least 1), so that its results can be hashed"
            ),
            EngineError::MismatchNotTaken => f.write_str(
                "take the boundary hash mismatch of the last block before committing a swap",
            ),
        }
    }
}

impl std::error::Error for EngineError {}
