//! Checks on the flattened graph, after converter insertion: connections need
//! equal shape and rate (a kind mismatch is only a warning; a rate mismatch
//! left here is one no converter joins), except that a parameter accepts
//! both a signal and events, and
//! parameter values must lie in their declared range. Fan-in and fan-out are
//! unrestricted: signals into one input are summed and events merged (ADR 0016
//! §3, §4), except into an input the registry restricts ([`restricted`], ADR
//! 0024).

use std::collections::BTreeMap;

use signal_core::{InstanceAddress, PortAddress, PortKind, PortShape, RateDivisor, Registry};

use crate::error::{GraphError, GraphWarning, PlanBudget, Restriction};
use crate::flatten::{FlatConnection, FlatGraph};

/// Before converters are inserted: an input the registry restricts
/// (`Registry::restrict_input`, ADR 0024) takes at most one connection, at
/// scale exactly 1, from the output it is restricted to. Each offending
/// connection is reported as written.
pub(crate) fn restricted(graph: &FlatGraph, registry: &Registry) -> Result<(), Vec<GraphError>> {
    let mut into: BTreeMap<&PortAddress, Vec<&FlatConnection>> = BTreeMap::new();
    for connection in &graph.connections {
        into.entry(&connection.to).or_default().push(connection);
    }
    let mut errors = Vec::new();
    for (to, mut connections) in into {
        let Some(target) = graph.instances.get(&to.instance()) else {
            continue;
        };
        let Some(source) = registry.input_rule(target.descriptor.type_name, to.port()) else {
            continue;
        };
        connections.sort_by(|a, b| a.from.cmp(&b.from));
        let count = connections.len();
        for connection in connections {
            let from_type = graph
                .instances
                .get(&connection.from.instance())
                .map_or("", |i| i.descriptor.type_name);
            let reason = if count > 1 {
                Restriction::FanIn { connections: count }
            } else if (from_type, connection.from.port()) != source {
                Restriction::Source {
                    type_name: from_type,
                    port: connection.from.port().to_owned(),
                }
            } else if connection.scale != 1.0 {
                Restriction::Scaled
            } else {
                continue;
            };
            let (from, to) = connection
                .declared
                .clone()
                .unwrap_or_else(|| (connection.from.clone(), connection.to.clone()));
            errors.push(GraphError::RestrictedInput {
                from,
                to,
                scale: connection.scale,
                source,
                reason: Box::new(reason),
            });
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// After converters are inserted: the plan's instances, connections and
/// value rows within their budgets ([`PlanBudget`]). Flattening already
/// stopped at the instance and connection budgets; converters add to both.
pub(crate) fn budget(graph: &FlatGraph) -> Result<(), Vec<GraphError>> {
    let rows = graph
        .instances
        .values()
        .map(|i| {
            let ports = i.descriptor.ports.iter().map(|p| p.shape.rows());
            ports.fold(i.descriptor.parameters.len(), usize::saturating_add)
        })
        .fold(0, usize::saturating_add);
    let counts = [
        (PlanBudget::Instances, graph.instances.len()),
        (PlanBudget::Connections, graph.connections.len()),
        (PlanBudget::Rows, rows),
    ];
    let errors: Vec<GraphError> = counts
        .into_iter()
        .filter(|&(budget, count)| count > budget.limit())
        .map(|(budget, count)| GraphError::PlanTooLarge { budget, count })
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Shape, rate and kind of a port, and whether it is a parameter.
type Signature = (PortShape, RateDivisor, PortKind, bool);

fn signature(graph: &FlatGraph, port: &PortAddress) -> Option<Signature> {
    let descriptor = graph.instances.get(&port.instance())?.descriptor;
    let name = port.port();
    if let Some(p) = descriptor.ports.iter().find(|p| p.name == name) {
        return Some((p.shape, p.rate, p.kind, false));
    }
    // A parameter is an input port (ADR 0004) whose signals modulate it and
    // whose events change its base (ADR 0016 §4); its own shape is a signal.
    let p = descriptor.parameters.iter().find(|p| p.name == name)?;
    Some((PortShape::Signal, p.rate, p.kind, true))
}

/// Validates `graph`, whose connections are in canonical order, appending
/// warnings to `warnings`. `converted` names each inserted converter's
/// source port: a kind mismatch through one is reported from that source,
/// as the connection was written (the converter carries its kind).
pub(crate) fn validate(
    graph: &FlatGraph,
    converted: &BTreeMap<InstanceAddress, PortAddress>,
    warnings: &mut Vec<GraphWarning>,
) -> Result<(), Vec<GraphError>> {
    let mut errors = Vec::new();
    for connection in &graph.connections {
        let (from, to) = (&connection.from, &connection.to);
        let written = converted.get(&from.instance()).unwrap_or(from);
        let (Some(source), Some(target)) = (signature(graph, from), signature(graph, to)) else {
            continue;
        };
        let (from_shape, from_rate, from_kind, _) = source;
        let (to_shape, to_rate, to_kind, to_parameter) = target;
        // A parameter (a signal input) also takes events, which set its base.
        let events_into_parameter = to_parameter && from_shape == PortShape::Events;
        if from_shape != to_shape && !events_into_parameter {
            errors.push(GraphError::ShapeMismatch {
                from: from.clone(),
                to: to.clone(),
                from_shape,
                to_shape,
            });
        }
        if from_rate != to_rate {
            errors.push(GraphError::RateMismatch {
                from: from.clone(),
                to: to.clone(),
                from_rate,
                to_rate,
            });
        }
        if from_kind != to_kind {
            warnings.push(GraphWarning::KindMismatch {
                from: written.clone(),
                to: to.clone(),
                from_kind,
                to_kind,
            });
        }
    }

    for (address, instance) in &graph.instances {
        for parameter in instance.descriptor.parameters {
            let Some(&value) = instance.parameters.get(parameter.name) else {
                continue;
            };
            let range = parameter.range;
            if !(range.min <= value && value <= range.max)
                && let Ok(parameter) = PortAddress::new(address, parameter.name)
            {
                errors.push(GraphError::ParameterOutOfRange {
                    parameter,
                    value,
                    range,
                });
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
