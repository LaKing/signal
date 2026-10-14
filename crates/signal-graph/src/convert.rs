//! Implicit rate conversion (ADR 0005, M2.8-6, `docs/semantics.md` §16): a
//! connection between a signal or vector output and an input of the same
//! shape (or a signal and a parameter) whose rate divisors differ by a
//! multiple is rewritten through a hold or decimate converter instance.
//!
//! The converter is an ordinary instance at `<source instance>/<port>_d<t>`
//! (*t* the target's divisor), shared by every target of that source port
//! at *t*. The source feeds it with scale 1 and it feeds each target with
//! the connection's scale. Each rewritten connection is reported as a
//! [`GraphWarning::ConverterInserted`]. Every other connection is left as it
//! is, so validation names the endpoints the user wrote.

use std::collections::BTreeMap;

use signal_core::{
    Conversion, Direction, InstanceAddress, PortAddress, PortKind, PortShape, RateDivisor,
    converter,
};

use crate::error::GraphWarning;
use crate::flatten::{FlatConnection, FlatGraph, FlatInstance};

/// Shape, rate and kind of a source output port.
fn output(graph: &FlatGraph, port: &PortAddress) -> Option<(PortShape, RateDivisor, PortKind)> {
    let descriptor = graph.instances.get(&port.instance())?.descriptor;
    let name = port.port();
    let p = descriptor
        .ports
        .iter()
        .find(|p| p.name == name && p.direction == Direction::Out)?;
    Some((p.shape, p.rate, p.kind))
}

/// Shape and rate of a target input port or parameter (a signal).
fn input(graph: &FlatGraph, port: &PortAddress) -> Option<(PortShape, RateDivisor)> {
    let descriptor = graph.instances.get(&port.instance())?.descriptor;
    let name = port.port();
    if let Some(p) = descriptor
        .ports
        .iter()
        .find(|p| p.name == name && p.direction == Direction::In)
    {
        return Some((p.shape, p.rate));
    }
    let p = descriptor.parameters.iter().find(|p| p.name == name)?;
    Some((PortShape::Signal, p.rate))
}

/// Rewrites every connection that an implicit converter joins (above),
/// adding the converters, and reports each in `warnings`. Returns each
/// inserted converter's source port, by converter, so that validation can
/// name the connection as written.
pub(crate) fn insert(
    graph: &mut FlatGraph,
    warnings: &mut Vec<GraphWarning>,
) -> BTreeMap<InstanceAddress, PortAddress> {
    // (source port, target divisor) → converter address.
    let mut converters: BTreeMap<(PortAddress, RateDivisor), InstanceAddress> = BTreeMap::new();
    let mut added = Vec::new();
    let connections = std::mem::take(&mut graph.connections);
    for connection in connections {
        let found = output(graph, &connection.from).zip(input(graph, &connection.to));
        let Some(((shape, from_rate, kind), (to_shape, to_rate))) = found else {
            graph.connections.push(connection);
            continue;
        };
        let (Some(conversion), true) = (
            Conversion::between(from_rate, to_rate),
            shape.is_signal() && shape == to_shape,
        ) else {
            graph.connections.push(connection);
            continue;
        };
        let key = (connection.from.clone(), to_rate);
        let address = match converters.get(&key) {
            Some(address) => address.clone(),
            None => {
                let name = format!("{}_d{}", connection.from.port(), to_rate.get());
                let made = connection
                    .from
                    .instance()
                    .child(&name)
                    .ok()
                    .zip(converter(from_rate, to_rate, shape, kind));
                let Some((address, module_type)) = made else {
                    graph.connections.push(connection);
                    continue;
                };
                if graph.instances.contains_key(&address) {
                    graph.connections.push(connection);
                    continue;
                }
                graph.instances.insert(
                    address.clone(),
                    FlatInstance {
                        descriptor: module_type.descriptor,
                        parameters: BTreeMap::new(),
                    },
                );
                if let Ok(to) = PortAddress::new(&address, "in") {
                    added.push(FlatConnection {
                        from: connection.from.clone(),
                        to,
                        scale: 1.0,
                    });
                }
                converters.insert(key, address.clone());
                address
            }
        };
        let Ok(from) = PortAddress::new(&address, "out") else {
            graph.connections.push(connection);
            continue;
        };
        warnings.push(GraphWarning::ConverterInserted {
            from: connection.from,
            to: connection.to.clone(),
            converter: address,
            conversion,
            from_rate,
            to_rate,
        });
        added.push(FlatConnection {
            from,
            to: connection.to,
            scale: connection.scale,
        });
    }
    graph.connections.extend(added);
    converters
        .into_iter()
        .map(|((source, _), converter)| (converter, source))
        .collect()
}
