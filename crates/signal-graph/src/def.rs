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
}
