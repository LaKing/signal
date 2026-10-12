//! What a module type declares about itself: ports, parameters, feedthrough
//! class and sink kind (ADR 0004, 0005, 0016 §2, §4). Serialized for
//! `signal modules --json`.
//!
//! Descriptors are built from `'static` data so a primitive can declare its
//! descriptor as a `const` and reading one never allocates.

use serde::Serialize;

use crate::time::RateDivisor;

/// Whether a port receives or produces values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// An input port.
    In,
    /// An output port.
    Out,
}

/// What a port carries per tick (ADR 0005). Vector shapes come later.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PortShape {
    /// One `f64` per tick.
    Signal,
    /// A stream of [`Event`](crate::Event)s.
    Events,
}

/// What a port's values mean, used for conversion and display (ADR 0005).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PortKind {
    /// An audio-rate waveform. The executor fades `audio` outputs of a suspended
    /// instance to 0 (ADR 0016 §1a).
    Audio,
    /// A continuous control value.
    Control,
    /// A level that is either on (non-zero) or off (zero).
    Gate,
    /// Momentary impulses.
    Trigger,
    /// A frequency.
    Frequency,
    /// A gain factor.
    Gain,
    /// A phase position.
    Phase,
    /// Anything else.
    Generic,
}

/// An inclusive value range `[min, max]`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Range {
    /// Lower bound.
    pub min: f64,
    /// Upper bound.
    pub max: f64,
}

/// A port of a module type that is not a parameter.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct PortDescriptor {
    /// Port name, `[a-z][a-z0-9_]*`, unique among the module's ports and parameters.
    pub name: &'static str,
    /// Input or output.
    pub direction: Direction,
    /// Signal or event stream.
    pub shape: PortShape,
    /// Rate as a divisor of the master rate.
    pub rate: RateDivisor,
    /// Meaning of the values.
    pub kind: PortKind,
    /// Unit of the values (e.g. `Hz`), if any.
    pub unit: Option<&'static str>,
    /// Expected value range, if any.
    pub range: Option<Range>,
}

/// How a parameter's base value and its summed modulation combine into the
/// effective value (ADR 0016 §4). The result is then clamped to the range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Combine {
    /// `base + modulation`, in the parameter's unit.
    #[default]
    Add,
    /// `base * modulation`.
    Multiply,
    /// The modulation sum when the parameter has connections, else the base.
    Replace,
}

/// A parameter: an input signal port with a default (ADR 0004). External control
/// sets its base with `set`/`ramp` events; connected signals modulate it. The
/// executor computes the effective value per frame (ADR 0016 §4, §4a), so a
/// module only ever reads effective values.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ParameterDescriptor {
    /// Parameter name, `[a-z][a-z0-9_]*`, unique among the module's ports and parameters.
    pub name: &'static str,
    /// Meaning of the value.
    pub kind: PortKind,
    /// Rate as a divisor of the master rate.
    pub rate: RateDivisor,
    /// Unit of the value (e.g. `Hz`), if any.
    pub unit: Option<&'static str>,
    /// The range the effective value is clamped to.
    pub range: Range,
    /// The initial base value.
    pub default: f64,
    /// How base and modulation combine.
    pub combine: Combine,
}

/// The feedthrough class of a primitive (ADR 0016 §2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Feedthrough {
    /// Outputs may depend on same-frame inputs; implements
    /// [`DirectPrimitive`](crate::DirectPrimitive).
    #[default]
    Direct,
    /// Outputs at frame *n* depend on state\[n−1\] only; implements
    /// [`StateOutputPrimitive`](crate::StateOutputPrimitive).
    StateOutput,
}

/// A declared output latency (ADR 0019 §3): the output in the module's block
/// *b* is the result for its block *b* − `latency_blocks`. Only a
/// state-output type declares one; its output then depends on past input
/// only, at block granularity.
///
/// The block is the module's own: `block_frames` consecutive frames counted
/// from the instance's first frame, fixed for its type and therefore for
/// every run. It is part of the result (and recorded in the manifest); the
/// engine's block size *k* still never changes the output, because the engine
/// calls a state-output instance one frame at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct BlockLatency {
    /// *L* ≥ 1.
    pub latency_blocks: u32,
    /// The module's block size in frames, ≥ 1.
    pub block_frames: u32,
}

/// How far a module's output reproduces (ADR 0008, ADR 0019 §3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reproducibility {
    /// Bit-exact for the same build on any machine.
    #[default]
    BitExact,
    /// Bit-exact only on the same machine and build (e.g. GPU arithmetic).
    /// A render that uses such a module records it in its manifest.
    SameMachine,
}

impl Reproducibility {
    /// Whether this is [`Reproducibility::BitExact`] (the descriptor's JSON
    /// leaves the default out).
    pub fn is_bit_exact(&self) -> bool {
        *self == Reproducibility::BitExact
    }
}

/// The external effect of a sink module: used for reachability warnings and by
/// drivers to collect output (ADR 0016 §1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SinkKind {
    /// Writes audio to the driver's output.
    Audio,
}

/// Everything a module type declares about itself, available without an instance.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ModuleDescriptor {
    /// Type name, e.g. `std.sine_osc`; the [`Registry`](crate::Registry) key.
    pub type_name: &'static str,
    /// Human-readable description.
    pub doc: &'static str,
    /// Ports other than parameters. The I/O views index each group (signal
    /// inputs, event inputs, signal outputs, event outputs) in this order.
    pub ports: &'static [PortDescriptor],
    /// Parameters, in the order the I/O views present them.
    pub parameters: &'static [ParameterDescriptor],
    /// Direct or state-output.
    pub feedthrough: Feedthrough,
    /// `Some` if the module has an external effect.
    pub sink: Option<SinkKind>,
    /// `Some` for a state-output type whose output is late by whole blocks
    /// of its own (ADR 0019 §3), e.g. a GPU-backed module.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency: Option<BlockLatency>,
    /// How far its output reproduces.
    #[serde(skip_serializing_if = "Reproducibility::is_bit_exact")]
    pub reproducibility: Reproducibility,
}
