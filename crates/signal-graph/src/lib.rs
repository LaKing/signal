//! Signal graph: turns a domain-free graph description into a validated, flat,
//! deterministically ordered [`ExecutionPlan`] (ADR 0006, 0016).
//!
//! - [`CompositeDef`] and friends: the description, as in-memory structs. The
//!   project root is itself a composite.
//! - [`build_plan`]: flattens composites into primitive instances with
//!   hierarchical addresses, validates connections and parameter values, and
//!   schedules every instance.
//! - [`GraphError`] and [`GraphWarning`]: what can be wrong, with addresses.
//!
//! Domain-free: knows instances, ports, connections and addresses only.
//! Converter insertion (ADR 0005) and live edits come in later milestones.

mod def;
mod error;
mod flatten;
mod plan;
mod schedule;
mod validate;

use std::collections::BTreeMap;

use signal_core::{Direction, PortAddress, Registry};

pub use def::{
    CompositeDef, ConnectionDef, Exports, InputExport, InstanceDef, OutputExport, ParameterExport,
};
pub use error::{GraphError, GraphWarning};
pub use plan::{ExecutionPlan, PlanInstance, Source};

/// Builds the execution plan of the project `root`.
///
/// Module references resolve to primitive types in `registry` or to
/// `composites`. `version` is the plan version to record (ADR 0006).
///
/// Errors are reported in stages: composite library, flattening, connection and
/// parameter validation, then dependency cycles. Every error of the first
/// failing stage is returned.
pub fn build_plan(
    root: &CompositeDef,
    composites: &[CompositeDef],
    registry: &Registry,
    version: u64,
) -> Result<ExecutionPlan, Vec<GraphError>> {
    let mut graph = flatten::flatten(root, composites, registry)?;
    // Canonical order (ADR 0016 §3): by target, then byte-wise by source.
    graph.connections.sort_by(|a, b| {
        (&a.to, &a.from)
            .cmp(&(&b.to, &b.from))
            .then_with(|| a.scale.total_cmp(&b.scale))
    });

    let mut warnings = Vec::new();
    validate::validate(&graph, &mut warnings)?;
    let schedule = schedule::schedule(&graph).map_err(|error| vec![error])?;
    warnings.extend(
        schedule::unreachable(&graph)
            .into_iter()
            .map(|instance| GraphWarning::NoSinkReachable { instance }),
    );

    let mut sources: BTreeMap<PortAddress, Vec<Source>> = BTreeMap::new();
    for (address, instance) in &graph.instances {
        let descriptor = instance.descriptor;
        let inputs = descriptor
            .ports
            .iter()
            .filter(|p| p.direction == Direction::In)
            .map(|p| p.name)
            .chain(descriptor.parameters.iter().map(|p| p.name));
        for name in inputs {
            if let Ok(port) = PortAddress::new(address, name) {
                sources.insert(port, Vec::new());
            }
        }
    }
    for connection in graph.connections {
        sources.entry(connection.to).or_default().push(Source {
            from: connection.from,
            scale: connection.scale,
        });
    }

    let instances = graph
        .instances
        .into_iter()
        .map(|(address, instance)| PlanInstance {
            address,
            type_name: instance.descriptor.type_name,
            parameters: instance.parameters,
            descriptor: instance.descriptor,
        })
        .collect();

    Ok(ExecutionPlan {
        version,
        instances,
        schedule: schedule.order,
        state_outputs: schedule.state_outputs,
        sources,
        warnings,
    })
}
