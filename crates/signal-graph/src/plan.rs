//! The execution plan: data only. The engine instantiates modules from it and
//! executes it without re-deciding anything (ADR 0006, 0016).

use std::collections::BTreeMap;

use serde::Serialize;
use signal_core::{InstanceAddress, ModuleDescriptor, PortAddress, PortShape, Table};

use crate::error::GraphWarning;

/// A validated, flat, deterministically ordered graph. Holds no module objects.
#[derive(Clone, Debug, Serialize)]
pub struct ExecutionPlan {
    /// The plan version (ADR 0006): every graph change produces a new one. The
    /// caller assigns it.
    pub version: u64,
    /// Every primitive instance, in address order.
    pub instances: Vec<PlanInstance>,
    /// Every instance in execution order: a topological order of the dependency
    /// graph, ties broken by address (ADR 0016 §1, §2).
    pub schedule: Vec<InstanceAddress>,
    /// The state-output instances, in address order. They emit before and
    /// absorb after the direct instances run (ADR 0016 §2).
    pub state_outputs: Vec<InstanceAddress>,
    /// For every input port and parameter, its incoming connections in canonical
    /// order: byte-wise by source port address (ADR 0016 §3, §4). A parameter may
    /// have both kinds of source; [`Source::shape`] tells them apart.
    pub sources: BTreeMap<PortAddress, Vec<Source>>,
    /// Diagnostics; none of them changes what runs.
    pub warnings: Vec<GraphWarning>,
}

/// A primitive instance of the plan.
#[derive(Clone, Debug, Serialize)]
pub struct PlanInstance {
    /// Its hierarchical address.
    pub address: InstanceAddress,
    /// Its primitive type name, the registry key.
    pub type_name: &'static str,
    /// The initial base value of every declared parameter.
    pub parameters: BTreeMap<String, f64>,
    /// The resolved descriptor. Not serialized: it is the registry's entry for
    /// `type_name`.
    #[serde(skip)]
    pub descriptor: ModuleDescriptor,
    /// Its table (ADR 0028 §4), compiled by its type's library from the
    /// data the project names for it, or `None`. Not serialized: it comes
    /// from a project file the caller hashes.
    #[serde(skip)]
    pub table: Option<Table>,
}

/// One incoming connection of an input port.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Source {
    /// The source output port.
    pub from: PortAddress,
    /// The factor applied to a signal source (1 when the connection gives none).
    /// Events are delivered unscaled.
    pub scale: f64,
    /// The source port's shape. Into a parameter, a signal source is modulation
    /// and an event source changes the base (ADR 0016 §4).
    pub shape: PortShape,
}
