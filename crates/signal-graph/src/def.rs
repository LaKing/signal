//! The graph description: composites of module instances, connections and
//! exports, as plain in-memory structs. The project root is itself a
//! [`CompositeDef`]. Parsing files into these structs is `signal-project`'s job.
//!
//! Instances, connections and exports are lists in declaration order. The plan
//! built from them does not depend on that order.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A composite module type: instances, the connections between them, and the
/// ports and parameters it exposes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CompositeDef {
    /// The composite's name, used as the module reference of its instances.
    pub name: String,
    /// The module instances it contains.
    #[serde(default)]
    pub instances: Vec<InstanceDef>,
    /// Connections between its instances' ports.
    #[serde(default)]
    pub connections: Vec<ConnectionDef>,
    /// What it exposes to the composite that contains it.
    #[serde(default)]
    pub exports: Exports,
    /// Its sleep groups (ADR 0044 §11, `docs/semantics.md` §17).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sleep: Vec<SleepDef>,
}

/// A sleep group (ADR 0044 §11): the primitive instances under one of the
/// composite's instances, which are not called while the group is asleep.
/// Domain-free: frames, ports, instances and exact zero.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SleepDef {
    /// The instance whose primitive instances are the group's members: a
    /// local name.
    pub members: String,
    /// The driver: a signal output of another of the composite's instances,
    /// `inst.port`, that wakes the group at 0.5 or more.
    pub driver: String,
    /// The rest length *R* in frames, at least 1: the group falls asleep
    /// only at multiples of it, after *R* quiet frames.
    pub rest: u64,
}

/// An instance of a module inside a composite.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct InstanceDef {
    /// Local name, `[a-z][a-z0-9_]*`: the last segment of its address.
    pub name: String,
    /// A registered primitive type name (e.g. `std.sine_osc`) or a composite name.
    pub module: String,
    /// Parameter values by parameter name (for a composite, by exported parameter name).
    #[serde(default)]
    pub parameters: BTreeMap<String, f64>,
    /// Makes this composite instance a **scope root** (ADR 0035 §4): every
    /// primitive instance under it draws its random stream from the root's
    /// seed and its address relative to the root, not from the plan's seed
    /// and its full address. `None` (the default) changes nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<ScopeDef>,
}

/// A scope root's settings (ADR 0035 §4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeDef {
    /// The seed every random stream under the root derives from; `None`
    /// inherits the run's seed (ADR 0051 §5: an instance moved under the
    /// root keeps the stream it drew at its old address).
    pub seed: Option<u64>,
}

/// A connection from an output port to an input port, both written `inst.port`
/// with local instance names.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ConnectionDef {
    /// The source output port, `inst.port`.
    pub from: String,
    /// The target input port or parameter, `inst.port`.
    pub to: String,
    /// Factor applied to the source before it reaches the target; `None` means 1.
    #[serde(default)]
    pub scale: Option<f64>,
}

/// The ports and parameters a composite exposes. Export names share one
/// namespace, because each becomes a port of the composite's instances.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Exports {
    /// Exported inputs.
    #[serde(default)]
    pub inputs: Vec<InputExport>,
    /// Exported outputs.
    #[serde(default)]
    pub outputs: Vec<OutputExport>,
    /// Exported parameters.
    #[serde(default)]
    pub parameters: Vec<ParameterExport>,
}

/// An exported input: one or more inner input ports, each fed by whatever is
/// connected to the export (fan-out).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct InputExport {
    /// The exported port name.
    pub name: String,
    /// Inner input ports, `inst.port`; at least one.
    pub to: Vec<String>,
}

/// An exported output: exactly one inner output port.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OutputExport {
    /// The exported port name.
    pub name: String,
    /// The inner output port, `inst.port`.
    pub from: String,
}

/// An exported parameter: one or more inner parameter ports, each receiving the
/// value set on the composite's instance and whatever is connected to the export.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ParameterExport {
    /// The exported parameter name.
    pub name: String,
    /// Inner parameter ports, `inst.param`; at least one.
    pub to: Vec<String>,
    /// ADR 0042: the export's own range, which must lie inside every
    /// target's; `None` for the range the targets share. With a value set,
    /// the values' span.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<signal_core::Range>,
    /// ADR 0043: the export's own value set, in increasing order, each value
    /// allowed by every target; `None` for none of its own (it then takes
    /// the value set its targets share, if they have one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<ExportValue>>,
}

/// One entry of an export's value set (ADR 0043).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ExportValue {
    /// The value.
    pub value: f64,
    /// Its name.
    pub label: String,
}
