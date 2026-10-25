//! What a module type declares about itself: ports, parameters, feedthrough
//! class and sink kind (ADR 0004, 0005, 0016 §2, §4). Serialized for
//! `signal modules --json`.
//!
//! Descriptors are built from `'static` data so a primitive can declare its
//! descriptor as a `const` and reading one never allocates.

use serde::{Deserialize, Serialize};

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

/// What a port carries per tick (ADR 0005).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PortShape {
    /// One `f64` per tick.
    Signal,
    /// A stream of [`Event`](crate::Event)s.
    Events,
    /// N ≥ 1 `f64` per tick (M2.8-6): N rows of the I/O views, its elements
    /// in order, each one value per tick (`docs/module-contract.md`). A
    /// vector connects only to a vector of the same N, and N is at most
    /// [`MAX_VECTOR_WIDTH`].
    Vector(u32),
}

/// The most elements a vector port may have (`docs/semantics.md` §16): 16
/// times the widest port of the standard library (`std.mesh2d`'s 256
/// nodes). A wider vector is refused before anything is allocated for it:
/// by the converters ([`converter`](crate::converter)), the graph compiler
/// and the engine. At the default block capacity of 256 frames one row is
/// 2 KiB per buffer, so the widest port takes 8 MiB per buffer.
pub const MAX_VECTOR_WIDTH: u32 = 4096;

impl PortShape {
    /// The rows of the I/O views a port of this shape takes: 1 for a signal,
    /// N for a vector of N, 0 for events.
    pub const fn rows(self) -> usize {
        match self {
            PortShape::Signal => 1,
            PortShape::Vector(n) => n as usize,
            PortShape::Events => 0,
        }
    }

    /// Whether it carries values (a signal or a vector), not events.
    pub const fn is_signal(self) -> bool {
        matches!(self, PortShape::Signal | PortShape::Vector(_))
    }
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
    /// Any kind: a port that does not constrain what its values mean, such
    /// as the core arithmetic, function and compare types (ADR 0027), which
    /// sit between ports of any kind. A connection with a `Generic` end is
    /// never a kind mismatch ([`PortKind::matches`]).
    Generic,
}

impl PortKind {
    /// Whether a connection from a port of kind `self` to one of kind `to`
    /// joins compatible kinds: equal kinds, or either end `Generic`. A
    /// connection that does not is accepted with a kind-mismatch warning
    /// (ADR 0005, dated note of 2026-10-18).
    pub const fn matches(self, to: PortKind) -> bool {
        matches!(self, PortKind::Generic)
            || matches!(to, PortKind::Generic)
            || self as u8 == to as u8
    }
}

/// An inclusive value range `[min, max]`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Range {
    /// Lower bound.
    pub min: f64,
    /// Upper bound.
    pub max: f64,
}

/// One allowed value of a discrete parameter and its name (ADR 0043): an
/// entry of a value set, e.g. `0 sine`. Generic: a value and a label.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ValueLabel {
    /// The value.
    pub value: f64,
    /// Its name, as a control shows it.
    pub label: &'static str,
}

/// How a value between a value set's entries is read (ADR 0043): the rule
/// the module's `process` decides by, so a value snapped by it plays
/// exactly as the unsnapped one did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Snap {
    /// The nearest entry; of two as near, the greater (as `⌊x + ½⌋` reads
    /// a set of consecutive integers).
    #[default]
    Nearest,
    /// Read as `x > 0`: any value above 0 is the nearest entry above 0, any
    /// other the nearest entry at or below 0 (an off/on set: anything
    /// positive is on).
    Positive,
}

impl Snap {
    /// Whether this is the default rule (it is then not serialized).
    pub fn is_nearest(&self) -> bool {
        *self == Snap::Nearest
    }
}

/// `value` as one of `values` read by `snap` (ADR 0043): the entry `snap`
/// selects, or `value` itself when the set is empty. Bounded work, no
/// allocation: a module's `process` may call it.
pub fn snap_value(values: &[ValueLabel], snap: Snap, value: f64) -> f64 {
    let side = |entry: &ValueLabel| match snap {
        Snap::Nearest => true,
        Snap::Positive => (entry.value > 0.0) == (value > 0.0),
    };
    let mut best: Option<f64> = None;
    for pass in [true, false] {
        for entry in values {
            if pass && !side(entry) {
                continue;
            }
            let closer = best.is_none_or(|b| {
                let (d, e) = ((entry.value - value).abs(), (b - value).abs());
                d < e || (d == e && entry.value > b)
            });
            if closer {
                best = Some(entry.value);
            }
        }
        if best.is_some() {
            break;
        }
    }
    best.unwrap_or(value)
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
    /// ADR 0043: a discrete parameter's allowed values, each with its name
    /// (inside `range`, which spans them); `None` for a continuous one. A
    /// parameter with a value set is stepped: a value in a file must be one
    /// of them (a file's other value is snapped, with a warning), and a live
    /// edit is snapped ([`ParameterDescriptor::snap`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<&'static [ValueLabel]>,
    /// ADR 0043: how `process` reads a value between `values`' entries, and
    /// so how a value in a file or a live edit is snapped to one of them.
    #[serde(skip_serializing_if = "Snap::is_nearest")]
    pub snap: Snap,
}

impl ParameterDescriptor {
    /// `value` as this parameter's value set takes it ([`snap_value`]);
    /// `value` itself for a continuous parameter.
    pub fn snapped(&self, value: f64) -> f64 {
        match self.values {
            Some(values) => snap_value(values, self.snap, value),
            None => value,
        }
    }
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
    /// A free-form palette group (ADR 0027 §1), e.g. for `signal modules
    /// --category`. The vocabulary belongs to the library that registers the
    /// type (`signal_std::category`), never to the core; `None` for none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<&'static str>,
}

impl ModuleDescriptor {
    /// The type's rate (M2.8-6, `docs/semantics.md` §16): the divisor every
    /// port and parameter declares except its signal and vector inputs, 1 if
    /// it has none of those. `None` if those differ, or if a signal or vector
    /// input's divisor neither divides the rate nor is a multiple of it.
    pub fn rate(&self) -> Option<RateDivisor> {
        let signal_input = |p: &PortDescriptor| p.direction == Direction::In && p.shape.is_signal();
        let mut rate: Option<RateDivisor> = None;
        let own = self
            .ports
            .iter()
            .filter(|p| !signal_input(p))
            .map(|p| p.rate)
            .chain(self.parameters.iter().map(|p| p.rate));
        for divisor in own {
            match rate {
                Some(r) if r != divisor => return None,
                _ => rate = Some(divisor),
            }
        }
        let rate = rate.unwrap_or(RateDivisor::ONE);
        let r = rate.get();
        let commensurate = |d: u32| d.is_multiple_of(r) || r.is_multiple_of(d);
        self.ports
            .iter()
            .filter(|p| signal_input(p))
            .all(|p| commensurate(p.rate.get()))
            .then_some(rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_matches_every_kind_and_others_only_themselves() {
        use PortKind::*;
        let all = [
            Audio, Control, Gate, Trigger, Frequency, Gain, Phase, Generic,
        ];
        for a in all {
            for b in all {
                let expected = a == b || a == Generic || b == Generic;
                assert_eq!(a.matches(b), expected, "{a:?} -> {b:?}");
            }
        }
    }

    fn port(name: &'static str, direction: Direction, shape: PortShape, d: u32) -> PortDescriptor {
        PortDescriptor {
            name,
            direction,
            shape,
            rate: RateDivisor::new(d).unwrap(),
            kind: PortKind::Generic,
            unit: None,
            range: None,
        }
    }

    fn rate_of(ports: Vec<PortDescriptor>) -> Option<u32> {
        let descriptor = ModuleDescriptor {
            type_name: "test.rate",
            doc: "",
            ports: Box::leak(ports.into_boxed_slice()),
            parameters: &[],
            feedthrough: Feedthrough::Direct,
            sink: None,
            latency: None,
            reproducibility: Reproducibility::BitExact,
            category: None,
        };
        descriptor.rate().map(RateDivisor::get)
    }

    #[test]
    fn shapes_take_rows() {
        assert_eq!(PortShape::Signal.rows(), 1);
        assert_eq!(PortShape::Vector(256).rows(), 256);
        assert_eq!(PortShape::Events.rows(), 0);
        assert!(PortShape::Vector(4).is_signal() && !PortShape::Events.is_signal());
    }

    /// The rate is what the outputs (and parameters, event ports) share; a
    /// signal input may differ from it by a multiple either way.
    #[test]
    fn a_type_has_one_rate() {
        use Direction::{In, Out};
        use PortShape::{Events, Signal, Vector};
        assert_eq!(rate_of(vec![]), Some(1));
        assert_eq!(rate_of(vec![port("out", Out, Signal, 64)]), Some(64));
        let held = vec![
            port("in", In, Vector(4), 64),
            port("out", Out, Vector(4), 1),
        ];
        assert_eq!(rate_of(held), Some(1));
        let mixed = vec![port("a", Out, Signal, 2), port("b", Out, Signal, 1)];
        assert_eq!(rate_of(mixed), None);
        let odd = vec![port("in", In, Signal, 3), port("out", Out, Signal, 2)];
        assert_eq!(rate_of(odd), None);
        let events = vec![port("e", In, Events, 2), port("out", Out, Signal, 1)];
        assert_eq!(rate_of(events), None);
    }
}
