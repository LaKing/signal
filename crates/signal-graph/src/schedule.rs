//! The per-frame dependency graph, its schedule and reachability (ADR 0016 §1, §2).

use std::collections::{BTreeMap, BTreeSet};

use signal_core::{Feedthrough, InstanceAddress};

use crate::error::GraphError;
use crate::flatten::FlatGraph;

type Adjacency = BTreeMap<InstanceAddress, BTreeSet<InstanceAddress>>;

/// The schedule of every instance and the state-output instances.
pub(crate) struct Schedule {
    pub(crate) order: Vec<InstanceAddress>,
    pub(crate) state_outputs: Vec<InstanceAddress>,
}

/// Orders every instance topologically by Kahn's algorithm, taking the
/// lexicographically smallest ready instance first. A connection A → B is the
/// edge "B after A" unless A is state-output (ADR 0016 §2).
pub(crate) fn schedule(graph: &FlatGraph) -> Result<Schedule, GraphError> {
    let mut successors: Adjacency = BTreeMap::new();
    let mut predecessors: Adjacency = BTreeMap::new();
    for connection in &graph.connections {
        let from = connection.from.instance();
        let state_output = graph
            .instances
            .get(&from)
            .is_some_and(|i| i.descriptor.feedthrough == Feedthrough::StateOutput);
        if state_output {
            continue;
        }
        let to = connection.to.instance();
        successors
            .entry(from.clone())
            .or_default()
            .insert(to.clone());
        predecessors.entry(to).or_default().insert(from);
    }

    let mut pending: BTreeMap<&InstanceAddress, usize> = graph
        .instances
        .keys()
        .map(|address| (address, predecessors.get(address).map_or(0, BTreeSet::len)))
        .collect();
    let mut ready: BTreeSet<&InstanceAddress> = pending
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(address, _)| *address)
        .collect();
    let mut order = Vec::with_capacity(graph.instances.len());
    while let Some(next) = ready.pop_first() {
        pending.remove(next);
        order.push(next.clone());
        for successor in successors.get(next).into_iter().flatten() {
            if let Some(count) = pending.get_mut(successor) {
                *count -= 1;
                if *count == 0 {
                    ready.insert(successor);
                }
            }
        }
    }

    if !pending.is_empty() {
        let remaining: BTreeSet<&InstanceAddress> = pending.keys().copied().collect();
        return Err(GraphError::Cycle {
            instances: find_cycle(&remaining, &predecessors),
        });
    }

    let state_outputs = graph
        .instances
        .iter()
        .filter(|(_, i)| i.descriptor.feedthrough == Feedthrough::StateOutput)
        .map(|(address, _)| address.clone())
        .collect();
    Ok(Schedule {
        order,
        state_outputs,
    })
}

/// A cycle among the instances Kahn's algorithm could not schedule, in edge
/// order and starting at its smallest address.
///
/// Every remaining instance has a remaining predecessor, so walking backwards
/// from any of them (always to the smallest remaining predecessor) must repeat
/// an instance; the walk between the repeats is a cycle.
fn find_cycle(
    remaining: &BTreeSet<&InstanceAddress>,
    predecessors: &Adjacency,
) -> Vec<InstanceAddress> {
    let mut walk: Vec<&InstanceAddress> = Vec::new();
    let mut current = remaining.first().copied();
    while let Some(instance) = current {
        if let Some(start) = walk.iter().position(|a| *a == instance) {
            let mut cycle: Vec<InstanceAddress> = walk.drain(start..).rev().cloned().collect();
            if let Some(smallest) = cycle
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.cmp(b.1))
                .map(|(i, _)| i)
            {
                cycle.rotate_left(smallest);
            }
            return cycle;
        }
        walk.push(instance);
        current = predecessors
            .get(instance)
            .into_iter()
            .flatten()
            .find(|p| remaining.contains(p));
    }
    // Unreachable for a graph Kahn's algorithm could not finish; report the walk.
    walk.into_iter().cloned().collect()
}

/// Instances from which no sink instance is reachable along connections, in
/// address order. All connections count, including those from state-output
/// instances: reachability is about data flow, not same-frame order.
pub(crate) fn unreachable(graph: &FlatGraph) -> Vec<InstanceAddress> {
    let mut predecessors: Adjacency = BTreeMap::new();
    for connection in &graph.connections {
        predecessors
            .entry(connection.to.instance())
            .or_default()
            .insert(connection.from.instance());
    }
    let mut stack: Vec<&InstanceAddress> = graph
        .instances
        .iter()
        .filter(|(_, i)| i.descriptor.sink.is_some())
        .map(|(address, _)| address)
        .collect();
    let mut reached: BTreeSet<&InstanceAddress> = stack.iter().copied().collect();
    while let Some(instance) = stack.pop() {
        for predecessor in predecessors.get(instance).into_iter().flatten() {
            if reached.insert(predecessor) {
                stack.push(predecessor);
            }
        }
    }
    graph
        .instances
        .keys()
        .filter(|address| !reached.contains(address))
        .cloned()
        .collect()
}
