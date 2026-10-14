//! Validation errors and warnings. Each names the addresses it concerns; the
//! project root, which has no address of its own, is shown as `/`.

use std::fmt;

use serde::Serialize;
use signal_core::{
    Conversion, Direction, InstanceAddress, PortAddress, PortKind, PortShape, Range, RateDivisor,
};

/// Why a graph description cannot become an execution plan.
///
/// `scope` fields name the composite instance whose definition is at fault
/// (`None` for the project root); other addresses are hierarchical instance or
/// port addresses.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum GraphError {
    /// Two composites in the library have the same name.
    DuplicateComposite {
        /// The repeated name.
        name: String,
    },
    /// A composite has the name of a registered primitive type, so a module
    /// reference to it would be ambiguous.
    AmbiguousModule {
        /// The name used by both.
        module: String,
    },
    /// An instance refers to a module that is neither a registered primitive
    /// type nor a composite.
    UnknownModule {
        /// The instance.
        instance: InstanceAddress,
        /// The module reference.
        module: String,
    },
    /// A composite contains itself, directly or through other composites.
    RecursiveComposite {
        /// The instance at which the containment cycle closes.
        instance: InstanceAddress,
        /// Composite names along the cycle, first and last equal, e.g. `a, b, a`.
        chain: Vec<String>,
    },
    /// An instance, export or parameter name is not `[a-z][a-z0-9_]*`.
    InvalidName {
        /// Where the name is declared.
        scope: Option<InstanceAddress>,
        /// The name.
        name: String,
    },
    /// Two instances of one composite have the same name.
    DuplicateInstance {
        /// The composite instance (`None`: the project root).
        scope: Option<InstanceAddress>,
        /// The repeated name.
        name: String,
    },
    /// An export name is used more than once among a composite's exported
    /// inputs, outputs and parameters.
    ExportNameCollision {
        /// The composite instance (`None`: the project root).
        scope: Option<InstanceAddress>,
        /// The composite's name.
        composite: String,
        /// The repeated export name.
        name: String,
    },
    /// An exported input or parameter maps to no inner port; it must map to at
    /// least one.
    EmptyExport {
        /// The composite instance (`None`: the project root).
        scope: Option<InstanceAddress>,
        /// The composite's name.
        composite: String,
        /// The export name.
        name: String,
    },
    /// A connection or export endpoint is not `inst.port`.
    InvalidEndpoint {
        /// The composite instance (`None`: the project root).
        scope: Option<InstanceAddress>,
        /// The endpoint text.
        endpoint: String,
    },
    /// A connection or export names an instance the composite does not contain.
    UnknownInstance {
        /// The composite instance (`None`: the project root).
        scope: Option<InstanceAddress>,
        /// The instance name.
        name: String,
    },
    /// The instance has no port of this name (for a composite: no such export).
    UnknownPort {
        /// The port.
        port: PortAddress,
    },
    /// The instance has no parameter of this name (for a composite: no such
    /// exported parameter).
    UnknownParameter {
        /// The parameter.
        parameter: PortAddress,
    },
    /// A port is used in the wrong direction: an input as a source, or an
    /// output as a target.
    WrongDirection {
        /// The port.
        port: PortAddress,
        /// The direction its use requires.
        expected: Direction,
    },
    /// A connection joins ports of different shapes.
    ShapeMismatch {
        /// The source output port.
        from: PortAddress,
        /// The target input port or parameter.
        to: PortAddress,
        /// The source's shape.
        from_shape: PortShape,
        /// The target's shape.
        to_shape: PortShape,
    },
    /// A connection joins ports of different rates.
    RateMismatch {
        /// The source output port.
        from: PortAddress,
        /// The target input port or parameter.
        to: PortAddress,
        /// The source's rate divisor.
        from_rate: RateDivisor,
        /// The target's rate divisor.
        to_rate: RateDivisor,
    },
    /// A parameter value lies outside its declared range.
    ParameterOutOfRange {
        /// The parameter.
        parameter: PortAddress,
        /// The value.
        value: f64,
        /// The declared range.
        range: Range,
    },
    /// A connection's scale is NaN or infinite.
    NonFiniteScale {
        /// The declared source endpoint.
        from: PortAddress,
        /// The declared target endpoint.
        to: PortAddress,
        /// The scale.
        scale: f64,
    },
    /// The dependency graph has a cycle that passes through no state-output
    /// instance (ADR 0016 §2).
    Cycle {
        /// The cycle in connection order: each instance feeds the next, and the
        /// last feeds the first.
        instances: Vec<InstanceAddress>,
    },
}

const CONVERTER: &str = "converter required (ADR 0005), not available in M1";

fn scope(scope: &Option<InstanceAddress>) -> &str {
    scope.as_ref().map_or("/", InstanceAddress::as_str)
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::DuplicateComposite { name } => {
                write!(f, "composite `{name}` is defined more than once")
            }
            GraphError::AmbiguousModule { module } => write!(
                f,
                "`{module}` is both a composite and a registered primitive type"
            ),
            GraphError::UnknownModule { instance, module } => write!(
                f,
                "{instance}: unknown module `{module}` (neither a registered primitive type nor a composite)"
            ),
            GraphError::RecursiveComposite { instance, chain } => {
                write!(f, "{instance}: recursive composite {}", chain.join(" -> "))
            }
            GraphError::InvalidName { scope: s, name } => write!(
                f,
                "{}: invalid name `{name}`; names use a-z, 0-9, _ and start with a-z",
                scope(s)
            ),
            GraphError::DuplicateInstance { scope: s, name } => {
                write!(
                    f,
                    "{}: instance `{name}` is declared more than once",
                    scope(s)
                )
            }
            GraphError::ExportNameCollision {
                scope: s,
                composite,
                name,
            } => write!(
                f,
                "{}: composite `{composite}` exports `{name}` more than once",
                scope(s)
            ),
            GraphError::EmptyExport {
                scope: s,
                composite,
                name,
            } => write!(
                f,
                "{}: composite `{composite}` export `{name}` maps to no inner port",
                scope(s)
            ),
            GraphError::InvalidEndpoint { scope: s, endpoint } => write!(
                f,
                "{}: endpoint `{endpoint}` is not `instance.port`",
                scope(s)
            ),
            GraphError::UnknownInstance { scope: s, name } => {
                write!(f, "{}: no instance named `{name}`", scope(s))
            }
            GraphError::UnknownPort { port } => write!(f, "{port}: unknown port"),
            GraphError::UnknownParameter { parameter } => {
                write!(f, "{parameter}: unknown parameter")
            }
            GraphError::WrongDirection { port, expected } => {
                let (expected, actual) = match expected {
                    Direction::In => ("an input", "an output"),
                    Direction::Out => ("an output", "an input"),
                };
                write!(f, "{port}: expected {expected}, but it is {actual}")
            }
            GraphError::ShapeMismatch {
                from,
                to,
                from_shape,
                to_shape,
            } => write!(
                f,
                "{from} ({from_shape:?}) -> {to} ({to_shape:?}): shape mismatch; {CONVERTER}"
            ),
            GraphError::RateMismatch {
                from,
                to,
                from_rate,
                to_rate,
            } => write!(
                f,
                "{from} (rate divisor {}) -> {to} (rate divisor {}): rate mismatch; only signals and vectors of the same shape convert implicitly, by hold (the source's divisor a multiple of the target's) or decimate (the target's a multiple of the source's)",
                from_rate.get(),
                to_rate.get()
            ),
            GraphError::ParameterOutOfRange {
                parameter,
                value,
                range,
            } => write!(
                f,
                "{parameter}: value {value} is outside [{}, {}]",
                range.min, range.max
            ),
            GraphError::NonFiniteScale { from, to, scale } => {
                write!(f, "{from} -> {to}: scale {scale} is not finite")
            }
            GraphError::Cycle { instances } => {
                f.write_str("dependency cycle without a state-output instance: ")?;
                for instance in instances {
                    write!(f, "{instance} -> ")?;
                }
                match instances.first() {
                    Some(first) => write!(f, "{first}"),
                    None => Ok(()),
                }
            }
        }
    }
}

impl std::error::Error for GraphError {}

/// A diagnostic that does not prevent a plan.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "warning", rename_all = "snake_case")]
#[non_exhaustive]
pub enum GraphWarning {
    /// A connection joins ports of the same shape and rate but different kinds.
    KindMismatch {
        /// The source output port.
        from: PortAddress,
        /// The target input port or parameter.
        to: PortAddress,
        /// The source's kind.
        from_kind: PortKind,
        /// The target's kind.
        to_kind: PortKind,
    },
    /// No sink instance is reachable from this instance, so its output is not
    /// observed. It still runs (ADR 0016 §1).
    NoSinkReachable {
        /// The instance.
        instance: InstanceAddress,
    },
    /// A connection across rate divisors runs through an inserted converter
    /// (ADR 0005, M2.8-6): reported so that the conversion is visible.
    ConverterInserted {
        /// The source output port, as connected.
        from: PortAddress,
        /// The target input port or parameter, as connected.
        to: PortAddress,
        /// The converter instance.
        converter: InstanceAddress,
        /// Hold or decimate.
        conversion: Conversion,
        /// The source's rate divisor.
        from_rate: RateDivisor,
        /// The target's rate divisor.
        to_rate: RateDivisor,
    },
}

impl fmt::Display for GraphWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphWarning::KindMismatch {
                from,
                to,
                from_kind,
                to_kind,
            } => write!(
                f,
                "{from} ({from_kind:?}) -> {to} ({to_kind:?}): kind mismatch"
            ),
            GraphWarning::NoSinkReachable { instance } => {
                write!(f, "{instance}: no sink is reachable; its output is unused")
            }
            GraphWarning::ConverterInserted {
                from,
                to,
                converter,
                conversion,
                from_rate,
                to_rate,
            } => write!(
                f,
                "{from} -> {to}: inserted converter {converter} ({conversion}, rate divisor {} -> {})",
                from_rate.get(),
                to_rate.get()
            ),
        }
    }
}
