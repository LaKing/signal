//! Checks on the flattened graph: connections need equal shape and rate (a kind
//! mismatch is only a warning), and parameter values must lie in their declared
//! range. Fan-in and fan-out are unrestricted: signals into one input are summed
//! and events merged (ADR 0016 §3, §4).

use signal_core::{PortAddress, PortKind, PortShape, RateDivisor};

use crate::error::{GraphError, GraphWarning};
use crate::flatten::FlatGraph;

/// Shape, rate and kind of a port.
type Signature = (PortShape, RateDivisor, PortKind);

fn signature(graph: &FlatGraph, port: &PortAddress) -> Option<Signature> {
    let descriptor = graph.instances.get(&port.instance())?.descriptor;
    let name = port.port();
    if let Some(p) = descriptor.ports.iter().find(|p| p.name == name) {
        return Some((p.shape, p.rate, p.kind));
    }
    // A parameter is a signal input (ADR 0004).
    let p = descriptor.parameters.iter().find(|p| p.name == name)?;
    Some((PortShape::Signal, p.rate, p.kind))
}

/// Validates `graph`, whose connections are in canonical order, appending
/// warnings to `warnings`.
pub(crate) fn validate(
    graph: &FlatGraph,
    warnings: &mut Vec<GraphWarning>,
) -> Result<(), Vec<GraphError>> {
    let mut errors = Vec::new();
    for connection in &graph.connections {
        let (from, to) = (&connection.from, &connection.to);
        let (Some(source), Some(target)) = (signature(graph, from), signature(graph, to)) else {
            continue;
        };
        let ((from_shape, from_rate, from_kind), (to_shape, to_rate, to_kind)) = (source, target);
        if from_shape != to_shape {
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
                from: from.clone(),
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
