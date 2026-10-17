//! Rate converters (ADR 0005, M2.8-6, `docs/semantics.md` §16): ordinary
//! direct primitives that copy each input row to the matching output row,
//! with the input at one rate divisor and the output at another.
//!
//! - **hold** (`core.hold`): the source is slower (its divisor a multiple of
//!   the target's); each output tick shows the source's last tick.
//! - **decimate** (`core.decimate`): the source is faster (the target's
//!   divisor a multiple of its own); each output tick samples it.
//!
//! The graph compiler inserts one where a connection joins two such rates,
//! and a project may name one explicitly. A converter's type depends on its
//! rates, shape and kind, so its descriptor cannot be a `const`: each is made
//! once, leaked, and kept by name for the rest of the process. The table is
//! behind a lock, taken only by [`converter`] and [`named`], which are called
//! while plans are built (by the graph compiler and through
//! [`Registry::get`](crate::Registry::get)), never on the audio thread.
//!
//! **Bounds.** A converter's vector is at most
//! [`MAX_VECTOR_WIDTH`](crate::MAX_VECTOR_WIDTH) elements: a wider one is
//! never made, so a name in project text cannot ask for an unbounded
//! buffer. The table itself is not capped: each distinct converter type
//! costs a fixed few hundred bytes (its descriptor does not grow with the
//! width), and only names that plans actually use are made. A cap would make
//! whether a project plans depend on what the process planned before it,
//! which the project text must decide alone (ADR 0009); a process that plans
//! millions of distinct rate pairs keeps that much (`docs/semantics.md` §16).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock, PoisonError};

use serde::Serialize;

use crate::descriptor::{
    Direction, Feedthrough, MAX_VECTOR_WIDTH, ModuleDescriptor, PortDescriptor, PortKind,
    PortShape, Reproducibility,
};
use crate::module::{DirectPrimitive, Inputs, Outputs, PrepareContext, Primitive};
use crate::registry::ModuleType;
use crate::time::RateDivisor;

/// The reserved type-name namespace of converters.
pub const CONVERTER_PREFIX: &str = "core.";

/// Which implicit conversion a converter makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Conversion {
    /// Up to a faster rate: the target shows the source's last tick.
    Hold,
    /// Down to a slower rate: the target samples the source at its ticks.
    Decimate,
}

impl Conversion {
    /// The conversion from divisor `from` to `to`, if it is one: hold when
    /// `from` is a multiple of `to`, decimate when `to` is one of `from`;
    /// `None` for equal divisors or when neither divides the other.
    pub fn between(from: RateDivisor, to: RateDivisor) -> Option<Conversion> {
        let (s, t) = (from.get(), to.get());
        if s == t {
            None
        } else if s.is_multiple_of(t) {
            Some(Conversion::Hold)
        } else if t.is_multiple_of(s) {
            Some(Conversion::Decimate)
        } else {
            None
        }
    }

    /// Its name: `hold` or `decimate`.
    pub fn name(self) -> &'static str {
        match self {
            Conversion::Hold => "hold",
            Conversion::Decimate => "decimate",
        }
    }
}

impl fmt::Display for Conversion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The name of a port kind in a converter's type name.
fn kind_name(kind: PortKind) -> &'static str {
    match kind {
        PortKind::Audio => "audio",
        PortKind::Control => "control",
        PortKind::Gate => "gate",
        PortKind::Trigger => "trigger",
        PortKind::Frequency => "frequency",
        PortKind::Gain => "gain",
        PortKind::Phase => "phase",
        PortKind::Generic => "generic",
    }
}

const KINDS: [PortKind; 8] = [
    PortKind::Audio,
    PortKind::Control,
    PortKind::Gate,
    PortKind::Trigger,
    PortKind::Frequency,
    PortKind::Gain,
    PortKind::Phase,
    PortKind::Generic,
];

/// The type name of the converter from `from` to `to` for ports of `shape`
/// and `kind`, e.g. `core.hold.d64.d1.signal.audio` or
/// `core.decimate.d1.d64.vector4.control`; `None` if it is not one
/// ([`Conversion::between`], a signal or a vector of at least one element).
/// Only a name: a vector wider than
/// [`MAX_VECTOR_WIDTH`](crate::MAX_VECTOR_WIDTH) has one, but no converter.
pub fn converter_name(
    from: RateDivisor,
    to: RateDivisor,
    shape: PortShape,
    kind: PortKind,
) -> Option<String> {
    let conversion = Conversion::between(from, to)?;
    let shape = match shape {
        PortShape::Signal => "signal".to_owned(),
        PortShape::Vector(n) if n >= 1 => format!("vector{n}"),
        _ => return None,
    };
    Some(format!(
        "{CONVERTER_PREFIX}{conversion}.d{}.d{}.{shape}.{}",
        from.get(),
        to.get(),
        kind_name(kind)
    ))
}

/// What a converter's type name says: (from, to, shape, kind). `None` for a
/// name that is not exactly what [`converter_name`] makes.
fn parse(name: &str) -> Option<(RateDivisor, RateDivisor, PortShape, PortKind)> {
    let rest = name.strip_prefix(CONVERTER_PREFIX)?;
    let mut parts = rest.split('.');
    let (word, from, to, shape, kind) = (
        parts.next()?,
        parts.next()?,
        parts.next()?,
        parts.next()?,
        parts.next()?,
    );
    if parts.next().is_some() {
        return None;
    }
    let divisor = |text: &str| RateDivisor::new(text.strip_prefix('d')?.parse().ok()?);
    let (from, to) = (divisor(from)?, divisor(to)?);
    let shape = match shape {
        "signal" => PortShape::Signal,
        _ => PortShape::Vector(shape.strip_prefix("vector")?.parse().ok()?),
    };
    let kind = KINDS.into_iter().find(|&k| kind_name(k) == kind)?;
    // Only the canonical spelling: no leading zeros, no `+`, the right word.
    let canonical = converter_name(from, to, shape, kind)?;
    let conversion = Conversion::between(from, to)?;
    (canonical == name && word == conversion.name()).then_some((from, to, shape, kind))
}

/// Every converter type made so far, by name.
fn table() -> &'static Mutex<BTreeMap<String, &'static ModuleType>> {
    static TABLE: OnceLock<Mutex<BTreeMap<String, &'static ModuleType>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// The converter type from divisor `from` to `to` for ports of `shape` and
/// `kind`: input `in` at `from`, output `out` at `to`, both of that shape and
/// kind, no parameters. `None` when no implicit conversion joins the two
/// divisors, or for an event or empty shape, or a vector wider than
/// [`MAX_VECTOR_WIDTH`](crate::MAX_VECTOR_WIDTH) (nothing is made or kept
/// for it). Made once per process and kept; takes a lock and may allocate,
/// so never on the audio thread.
pub fn converter(
    from: RateDivisor,
    to: RateDivisor,
    shape: PortShape,
    kind: PortKind,
) -> Option<&'static ModuleType> {
    if matches!(shape, PortShape::Vector(n) if n > MAX_VECTOR_WIDTH) {
        return None;
    }
    let name = converter_name(from, to, shape, kind)?;
    let mut table = table().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(&found) = table.get(&name) {
        return Some(found);
    }
    let port = |name, direction, rate| PortDescriptor {
        name,
        direction,
        shape,
        rate,
        kind,
        unit: None,
        range: None,
    };
    let ports: &'static [PortDescriptor] = Box::leak(Box::new([
        port("in", Direction::In, from),
        port("out", Direction::Out, to),
    ]));
    let type_name: &'static str = Box::leak(name.clone().into_boxed_str());
    let module_type: &'static ModuleType = Box::leak(Box::new(ModuleType {
        descriptor: ModuleDescriptor {
            type_name,
            doc: "Rate converter (ADR 0005): copies its input to its output at its own rate.",
            ports,
            parameters: &[],
            feedthrough: Feedthrough::Direct,
            sink: None,
            latency: None,
            reproducibility: Reproducibility::BitExact,
            category: None,
        },
        create: || Primitive::Direct(Box::new(Converter)),
    }));
    table.insert(name, module_type);
    Some(module_type)
}

/// The converter type named `name`, if it is the canonical name of one
/// ([`converter_name`]); what [`Registry::get`](crate::Registry::get) serves
/// in the reserved `core.` namespace. A malformed name is `None`.
pub fn named(name: &str) -> Option<&'static ModuleType> {
    let (from, to, shape, kind) = parse(name)?;
    converter(from, to, shape, kind)
}

/// The port shape that `name`, the canonical name of a converter
/// ([`converter_name`]), declares, even a vector too wide for a converter
/// to be made ([`converter`]); `None` for any other name. Lets the graph
/// compiler tell a name refused for its width from an unknown one. Makes
/// nothing and takes no lock.
pub fn converter_shape(name: &str) -> Option<PortShape> {
    parse(name).map(|(_, _, shape, _)| shape)
}

/// The primitive every converter type creates: each output row is its input
/// row, tick for tick.
struct Converter;

impl DirectPrimitive for Converter {
    /// The scalar hold from divisor 2 to 1, for reference only: every
    /// converter type has its own descriptor ([`converter`]).
    fn describe() -> ModuleDescriptor {
        const PORTS: &[PortDescriptor] = &[
            PortDescriptor {
                name: "in",
                direction: Direction::In,
                shape: PortShape::Signal,
                rate: match RateDivisor::new(2) {
                    Some(d) => d,
                    None => RateDivisor::ONE,
                },
                kind: PortKind::Generic,
                unit: None,
                range: None,
            },
            PortDescriptor {
                name: "out",
                direction: Direction::Out,
                shape: PortShape::Signal,
                rate: RateDivisor::ONE,
                kind: PortKind::Generic,
                unit: None,
                range: None,
            },
        ];
        ModuleDescriptor {
            type_name: "core.hold.d2.d1.signal.generic",
            doc: "Rate converter (ADR 0005): copies its input to its output at its own rate.",
            ports: PORTS,
            parameters: &[],
            feedthrough: Feedthrough::Direct,
            sink: None,
            latency: None,
            reproducibility: Reproducibility::BitExact,
            category: None,
        }
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        for (r, row) in outputs.signals().enumerate() {
            for (out, &x) in row.iter_mut().zip(inputs.signal(r)) {
                *out = x;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(n: u32) -> RateDivisor {
        RateDivisor::new(n).unwrap()
    }

    #[test]
    fn conversions_join_multiples_only() {
        assert_eq!(Conversion::between(d(64), d(1)), Some(Conversion::Hold));
        assert_eq!(Conversion::between(d(1), d(64)), Some(Conversion::Decimate));
        assert_eq!(
            Conversion::between(d(64), d(1600)),
            Some(Conversion::Decimate)
        );
        assert_eq!(Conversion::between(d(2), d(3)), None);
        assert_eq!(Conversion::between(d(3), d(3)), None);
    }

    #[test]
    fn a_converter_is_made_once_and_its_name_reads_back() {
        let a = converter(d(64), d(1), PortShape::Signal, PortKind::Audio).unwrap();
        let b = converter(d(64), d(1), PortShape::Signal, PortKind::Audio).unwrap();
        assert!(std::ptr::eq(a, b));
        assert_eq!(a.descriptor.type_name, "core.hold.d64.d1.signal.audio");
        assert!(std::ptr::eq(
            named("core.hold.d64.d1.signal.audio").unwrap(),
            a
        ));
        let v = named("core.decimate.d1.d64.vector4.control").unwrap();
        assert_eq!(v.descriptor.ports[0].shape, PortShape::Vector(4));
        assert_eq!(v.descriptor.ports[0].rate, d(1));
        assert_eq!(v.descriptor.ports[1].rate, d(64));
        assert_eq!(v.descriptor.rate(), Some(d(64)));
    }

    #[test]
    fn malformed_names_are_not_converters() {
        for name in [
            "core.hold.d0.d1.signal.audio",
            "core.hold.d1.d64.signal.audio",
            "core.decimate.d64.d1.signal.audio",
            "core.hold.d64.d1.vector0.audio",
            "core.hold.d64.d1.signal.loud",
            "core.hold.d064.d1.signal.audio",
            "core.hold.d64.d1.signal",
            "core.hold.d64.d1.signal.audio.x",
            "core.hold.d2.d3.signal.audio",
            "core.copy",
        ] {
            assert!(named(name).is_none(), "{name}");
        }
    }

    /// Audit P1 (2026-10-15): a project could name a converter of 2³² − 1
    /// elements, whose one buffer would be terabytes. Its name still reads
    /// back (so the graph can say why it is refused), but no converter is
    /// made; the widest allowed one is.
    #[test]
    fn a_converter_wider_than_the_limit_is_never_made() {
        let name = "core.hold.d2.d1.vector4294967295.generic";
        assert_eq!(converter_shape(name), Some(PortShape::Vector(u32::MAX)));
        assert!(named(name).is_none());
        let over = PortShape::Vector(MAX_VECTOR_WIDTH + 1);
        assert!(converter(d(2), d(1), over, PortKind::Generic).is_none());
        let widest = PortShape::Vector(MAX_VECTOR_WIDTH);
        let made = converter(d(2), d(1), widest, PortKind::Generic).unwrap();
        assert_eq!(made.descriptor.ports[0].shape, widest);
        assert_eq!(converter_shape("core.copy"), None);
        assert_eq!(
            converter_shape("core.hold.d64.d1.signal.audio"),
            Some(PortShape::Signal)
        );
    }

    #[test]
    fn a_converter_copies_every_row() {
        let ty = converter(d(2), d(1), PortShape::Vector(2), PortKind::Generic).unwrap();
        let Primitive::Direct(mut module) = (ty.create)() else {
            panic!("a converter is direct");
        };
        let mut out = [0.0; 4];
        module.process(
            &Inputs::new(2, &[1.0, 2.0, 3.0, 4.0], &[], &[], &[]),
            &mut Outputs::new(2, &mut out, &mut []),
        );
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);
    }
}
