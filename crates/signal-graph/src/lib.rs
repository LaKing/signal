//! Signal graph: turns a domain-free graph description into a validated, flat,
//! deterministically ordered [`ExecutionPlan`] (ADR 0006, 0016).
//!
//! - [`CompositeDef`] and friends: the description, as in-memory structs. The
//!   project root is itself a composite.
//! - [`build_plan`]: flattens composites into primitive instances with
//!   hierarchical addresses, inserts hold and decimate converters between
//!   rate divisors (ADR 0005, M2.8-6), validates connections and parameter
//!   values, and schedules every instance. A parameter accepts signal
//!   connections (modulation) and event connections (base changes, ADR 0016
//!   §4).
//! - [`GraphError`] and [`GraphWarning`]: what can be wrong, with addresses.
//! - Budgets ([`PlanBudget`]): the most instances, connections and value rows
//!   a plan may have, and the widest vector
//!   ([`MAX_VECTOR_WIDTH`](signal_core::MAX_VECTOR_WIDTH)), so that no
//!   project text makes the engine allocate without bound.
//!
//! Domain-free: knows instances, ports, connections and addresses only.

mod convert;
mod def;
mod error;
mod flatten;
mod plan;
mod schedule;
mod validate;

use std::collections::BTreeMap;

use signal_core::{Direction, PortAddress, PortShape, Registry};

pub use def::{
    CompositeDef, ConnectionDef, Exports, InputExport, InstanceDef, OutputExport, ParameterExport,
};
pub use error::{GraphError, GraphWarning, PlanBudget, Restriction};
pub use plan::{ExecutionPlan, PlanInstance, Source};

/// The most instances, composite or primitive, a plan flattens to: about 9
/// times the largest benchmark project's 30 041 (`signal bench
/// --write-voices 4096`). Counted while flattening, so composites nested to
/// expand exponentially stop at it.
pub const MAX_PLAN_INSTANCES: usize = 1 << 18;

/// The most connections between primitive ports a plan flattens to (an
/// export that fans out counts once per port it reaches).
pub const MAX_PLAN_CONNECTIONS: usize = 1 << 20;

/// The most value rows a plan has, converters included: a row per signal
/// input and output, per vector element and per parameter. The engine keeps
/// a buffer of block-capacity frames per row (at the default 256, 2 KiB), so
/// this bounds those buffers to 1 GiB; the engine checks its whole footprint
/// again at the block capacity it runs (`docs/semantics.md` §16).
pub const MAX_PLAN_ROWS: usize = 1 << 19;

/// Builds the execution plan of the project `root`.
///
/// Module references resolve to primitive types in `registry` or to
/// `composites`. `version` is the plan version to record (ADR 0006).
///
/// Errors are reported in stages: composite library, flattening, connection and
/// parameter validation, then dependency cycles. Every error of the first
/// failing stage is returned. Before validation, a connection across rate
/// divisors that hold or decimate joins runs through an inserted converter,
/// reported as a [`GraphWarning::ConverterInserted`] (`docs/semantics.md` §16).
pub fn build_plan(
    root: &CompositeDef,
    composites: &[CompositeDef],
    registry: &Registry,
    version: u64,
) -> Result<ExecutionPlan, Vec<GraphError>> {
    let mut graph = flatten::flatten(root, composites, registry)?;
    // Before converters: the sources and connections as written (ADR 0024).
    validate::restricted(&graph, registry)?;
    // Canonical order (ADR 0016 §3): by target, then byte-wise by source.
    let canonical = |graph: &mut flatten::FlatGraph| {
        graph.connections.sort_by(|a, b| {
            (&a.to, &a.from)
                .cmp(&(&b.to, &b.from))
                .then_with(|| a.scale.total_cmp(&b.scale))
        });
    };
    canonical(&mut graph);

    let mut warnings = Vec::new();
    let converted = convert::insert(&mut graph, &mut warnings);
    canonical(&mut graph);
    // After converters, which add rows: the whole plan's.
    validate::budget(&graph)?;
    validate::validate(&graph, &converted, &mut warnings)?;
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
        // Validation passed, so the source is a declared output port.
        let shape = graph
            .instances
            .get(&connection.from.instance())
            .and_then(|i| {
                let name = connection.from.port();
                i.descriptor.ports.iter().find(|p| p.name == name)
            })
            .map_or(PortShape::Signal, |p| p.shape);
        sources.entry(connection.to).or_default().push(Source {
            from: connection.from,
            scale: connection.scale,
            shape,
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
            table: None,
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
