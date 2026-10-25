//! Validation errors and warnings. Each names the addresses it concerns; the
//! project root, which has no address of its own, is shown as `/`.

use std::fmt;

use serde::Serialize;
use signal_core::{
    Conversion, Direction, InstanceAddress, MAX_VECTOR_WIDTH, PortAddress, PortKind, PortShape,
    Range, RateDivisor,
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
    /// A primitive instance is marked a scope root (ADR 0035 §4); only a
    /// composite instance can be one.
    ScopeOnPrimitive {
        /// The instance.
        instance: InstanceAddress,
        /// Its primitive type.
        module: String,
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
    /// ADR 0042: an exported parameter's own range is not inside the range
    /// of one of its targets.
    ExportRangeOutside {
        /// The composite instance (`None`: the project root).
        scope: Option<InstanceAddress>,
        /// The composite's name.
        composite: String,
        /// The export name.
        name: String,
        /// The target, `inst.param`.
        target: String,
        /// The export's declared range, then the target's.
        ranges: Box<[Range; 2]>,
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
    /// A connection into a restricted input (ADR 0024,
    /// `Registry::restrict_input`) that is not its one connection, at scale
    /// 1, from the source it is restricted to.
    RestrictedInput {
        /// The declared source endpoint.
        from: PortAddress,
        /// The declared target endpoint.
        to: PortAddress,
        /// The connection's scale.
        scale: f64,
        /// The (type, output) the input is restricted to.
        source: (&'static str, &'static str),
        /// What is wrong with it.
        reason: Box<Restriction>,
    },
    /// Two inputs of one instance that its type makes exclusive (ADR 0033,
    /// `Registry::exclusive_inputs`) both have connections: reported on a
    /// connection into the second.
    ExclusiveInputs {
        /// The declared source endpoint of that connection.
        from: PortAddress,
        /// Its declared target endpoint.
        to: PortAddress,
        /// The primitive input it reaches, and the other input, which has
        /// connections too.
        inputs: Box<(PortAddress, PortAddress)>,
        /// The instance's type.
        type_name: &'static str,
    },
    /// An instance's module declares a vector port of more than
    /// [`MAX_VECTOR_WIDTH`] elements (`docs/semantics.md` §16): for a
    /// converter named in the project, the width in its name.
    VectorTooWide {
        /// The instance.
        instance: InstanceAddress,
        /// Its module.
        module: String,
        /// The widest vector it declares.
        width: u32,
    },
    /// The plan exceeds one of its budgets ([`PlanBudget`],
    /// `docs/semantics.md` §16), checked before the engine allocates
    /// anything for it.
    PlanTooLarge {
        /// Which budget.
        budget: PlanBudget,
        /// What was counted: the whole plan's, or, when flattening stopped
        /// at the instance or connection budget, the count it stopped at
        /// (the plan has at least that many).
        count: usize,
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
            GraphError::ScopeOnPrimitive { instance, module } => write!(
                f,
                "{instance}: `{module}` is a primitive type; only a composite instance can be a scope root"
            ),
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
            GraphError::ExportRangeOutside {
                scope: s,
                composite,
                name,
                target,
                ranges,
            } => write!(
                f,
                "{}: composite `{composite}` export `{name}`: its range [{}, {}] is not inside the range [{}, {}] of its target `{target}`",
                scope(s),
                ranges[0].min,
                ranges[0].max,
                ranges[1].min,
                ranges[1].max
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
            GraphError::RestrictedInput {
                from,
                to,
                scale,
                source: (from_type, from_port),
                reason,
            } => {
                write!(
                    f,
                    "{from} -> {to}: {to} takes one connection, at scale 1, from the \
                     {from_port} of a {from_type}; "
                )?;
                match &**reason {
                    Restriction::Scaled => write!(f, "this one has scale {scale}"),
                    Restriction::FanIn { connections } => {
                        write!(f, "it has {connections} connections")
                    }
                    Restriction::Source { type_name, port } => {
                        write!(f, "this one comes from the {port} of a {type_name}")
                    }
                }
            }
            GraphError::ExclusiveInputs {
                from,
                to,
                inputs,
                type_name,
            } => {
                let (input, other) = &**inputs;
                write!(
                    f,
                    "{from} -> {to}: {input} and {other} are both connected; a {type_name} takes \
                 `{}` or `{}`, not both",
                    other.port(),
                    input.port()
                )
            }
            GraphError::VectorTooWide {
                instance,
                module,
                width,
            } => write!(
                f,
                "{instance}: `{module}` has a vector of {width} elements; a vector has at most {MAX_VECTOR_WIDTH}"
            ),
            GraphError::PlanTooLarge { budget, count } => write!(
                f,
                "the plan exceeds its budget of {} {} ({count} counted)",
                budget.limit(),
                budget.what()
            ),
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

/// Why a connection into a restricted input is refused
/// ([`GraphError::RestrictedInput`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Restriction {
    /// Its scale is not exactly 1.
    Scaled,
    /// The input has more than one connection.
    FanIn {
        /// How many.
        connections: usize,
    },
    /// It comes from another output than the one the input is restricted to.
    Source {
        /// The source instance's type.
        type_name: &'static str,
        /// The source output.
        port: String,
    },
}

/// A budget of a plan ([`GraphError::PlanTooLarge`]): limits on what a
/// project can make the engine allocate, whatever its text expands to
/// (`docs/semantics.md` §16).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanBudget {
    /// Instances, composite or primitive, after flattening:
    /// [`MAX_PLAN_INSTANCES`](crate::MAX_PLAN_INSTANCES).
    Instances,
    /// Connections between primitive ports after flattening:
    /// [`MAX_PLAN_CONNECTIONS`](crate::MAX_PLAN_CONNECTIONS).
    Connections,
    /// Value rows (a row per signal port, per vector element and per
    /// parameter), converters included:
    /// [`MAX_PLAN_ROWS`](crate::MAX_PLAN_ROWS).
    Rows,
}

impl PlanBudget {
    /// The limit.
    pub fn limit(self) -> usize {
        match self {
            PlanBudget::Instances => crate::MAX_PLAN_INSTANCES,
            PlanBudget::Connections => crate::MAX_PLAN_CONNECTIONS,
            PlanBudget::Rows => crate::MAX_PLAN_ROWS,
        }
    }

    /// What it counts, in words.
    pub fn what(self) -> &'static str {
        match self {
            PlanBudget::Instances => "instances",
            PlanBudget::Connections => "primitive connections",
            PlanBudget::Rows => "value rows (signal ports, vector elements and parameters)",
        }
    }
}

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
