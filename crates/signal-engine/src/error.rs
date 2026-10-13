//! What can go wrong when instantiating a plan, scheduling an edit, swapping
//! plans, or rendering. Nothing goes wrong inside a frame: every check happens
//! before, except an engine failure (`docs/semantics.md` §14), which stops it.

use std::fmt;

use signal_core::{Frame, InstanceAddress, PortAddress};

use crate::log::EngineFailure;

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
    /// A port or parameter has a rate divisor other than 1 (M1 runs only 1).
    UnsupportedRate {
        /// The port.
        port: PortAddress,
    },
    /// A port has a shape this engine does not know.
    UnsupportedShape {
        /// The port.
        port: PortAddress,
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
            EngineError::UnsupportedRate { port } => {
                write!(f, "{port}: only rate divisor 1 runs in M1")
            }
            EngineError::UnsupportedShape { port } => write!(f, "{port}: unsupported port shape"),
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
        }
    }
}

impl std::error::Error for EngineError {}

/// Why an offline render failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum RenderError {
    /// The engine has already run frames; a render starts at frame 0.
    AlreadyStarted,
    /// The plan has no audio sink instance.
    NoAudioSink,
    /// The plan has more than one audio sink instance (M1 supports one).
    SeveralAudioSinks {
        /// The sinks, in address order.
        instances: Vec<InstanceAddress>,
    },
    /// The audio sink has no signal inputs, or more than a WAV file can hold.
    AudioChannels {
        /// The sink.
        instance: InstanceAddress,
        /// Its number of signal inputs.
        channels: usize,
    },
    /// A swap during [`render_with`](crate::render_with) did not keep the
    /// audio sink: exactly one, at the same address, as wide.
    SinkChanged {
        /// The sink the render started with.
        before: InstanceAddress,
        /// The audio sinks of the swapped-in plan.
        after: Vec<InstanceAddress>,
        /// The number of signal inputs the render records.
        channels: usize,
    },
    /// The caller's step before a frame of [`render_with`](crate::render_with)
    /// failed: a swap or an edit was refused.
    Edit(EngineError),
    /// The engine failed and stopped (`docs/semantics.md` §14): nothing of
    /// the render is kept.
    EngineFailed(EngineFailure),
    /// Writing an artifact failed.
    Io(std::io::Error),
    /// Writing `audio.wav` failed.
    Wav(hound::Error),
    /// Serializing a log line or the manifest failed.
    Json(serde_json::Error),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::AlreadyStarted => f.write_str("a render must start at frame 0"),
            RenderError::NoAudioSink => f.write_str("the plan has no audio sink"),
            RenderError::SeveralAudioSinks { instances } => {
                f.write_str("the plan has several audio sinks:")?;
                for instance in instances {
                    write!(f, " {instance}")?;
                }
                Ok(())
            }
            RenderError::AudioChannels { instance, channels } => write!(
                f,
                "{instance}: an audio sink needs 1 to 65535 signal inputs, it has {channels}"
            ),
            RenderError::SinkChanged {
                before,
                after,
                channels,
            } => {
                write!(
                    f,
                    "a swap must keep the audio sink {before} with {channels} inputs; the new plan's audio sinks:"
                )?;
                for instance in after {
                    write!(f, " {instance}")?;
                }
                Ok(())
            }
            RenderError::Edit(error) => write!(f, "applying an edit: {error}"),
            RenderError::EngineFailed(failure) => write!(f, "{failure}"),
            RenderError::Io(error) => write!(f, "writing render artifacts: {error}"),
            RenderError::Wav(error) => write!(f, "writing audio.wav: {error}"),
            RenderError::Json(error) => write!(f, "serializing render artifacts: {error}"),
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RenderError::Io(error) => Some(error),
            RenderError::Wav(error) => Some(error),
            RenderError::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for RenderError {
    fn from(error: std::io::Error) -> RenderError {
        RenderError::Io(error)
    }
}

impl From<hound::Error> for RenderError {
    fn from(error: hound::Error) -> RenderError {
        RenderError::Wav(error)
    }
}

impl From<serde_json::Error> for RenderError {
    fn from(error: serde_json::Error) -> RenderError {
        RenderError::Json(error)
    }
}
