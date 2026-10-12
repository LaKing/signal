//! The tasks of a plan (ADR 0019 §2): its units, in their deterministic
//! order, grouped into chains that are worth handing to another thread.
//! Computed when a plan is instantiated (control-thread code; it allocates).
//!
//! A unit's cost is estimated from its module types' static costs (the
//! [`Registry`](signal_core::Registry)'s, from `docs/benchmarks.md`), times
//! [`BLOCK_FACTOR`], plus [`PER_CALL_NS`] per frame for state-output units and
//! cycle members, which run one frame per call. Walking the units in order,
//! a unit joins the current task while the task is estimated below
//! `task_min_ns` per 256-frame block and the task's last unit feeds it;
//! otherwise it starts a task. Tasks are contiguous runs of a topological
//! order, so their graph has no cycle, and they depend only on the plan.

use std::collections::BTreeSet;
use std::ops::Range as Span;

use crate::units::{Unit, Units};

/// A task takes further units while it is estimated below this, in ns per
/// 256-frame block (ADR 0019 §2: a task should be worth about 2 µs).
pub(crate) const TASK_MIN_NS: u64 = 2_000;

/// The cost of a module type without a registered one, in ns per frame.
pub(crate) const DEFAULT_COST_NS: u32 = 20;

/// Block execution's cost relative to the per-frame figures the costs are
/// measured as: the kick at *k* = 256 against the per-frame engine
/// (141.2 / 261.8 ns per frame, `docs/benchmarks.md`).
pub(crate) const BLOCK_FACTOR: f64 = 141.2 / 261.8;

/// The per-call overhead of a unit that runs one frame per call, in ns per
/// frame: (386.6 − 141.2) / 11, the kick at *k* = 1 against *k* = 256 per
/// instance (`docs/benchmarks.md`).
pub(crate) const PER_CALL_NS: f64 = 22.0;

/// The block a task's size is judged at.
const CHAIN_FRAMES: f64 = 256.0;

/// The tasks of a plan and their dependencies.
#[derive(Clone, Debug, Default)]
pub(crate) struct Tasks {
    /// Each task's units, as a range of positions in [`Units::order`].
    ranges: Vec<Span<usize>>,
    /// The task of each instance.
    pub(crate) instance_task: Vec<u32>,
    /// Each task's number of predecessors, and its successors in order.
    pub(crate) preds: Vec<u32>,
    pub(crate) successors: Vec<Vec<u32>>,
    /// The whole plan's estimated cost per frame, in ns.
    pub(crate) estimate_ns: f64,
    /// Per task: the staged external events of the segment for its
    /// instances (scratch of the parallel decision).
    pub(crate) staged: Vec<usize>,
}

impl Tasks {
    /// The number of tasks.
    pub(crate) fn len(&self) -> usize {
        self.ranges.len()
    }

    /// Task `t`'s units, as positions in [`Units::order`].
    pub(crate) fn units(&self, t: usize) -> Span<usize> {
        self.ranges.get(t).cloned().unwrap_or(0..0)
    }
}

/// The members of `unit`, and whether each runs one frame per call.
fn members<'a>(units: &'a Units, unit: &'a Unit) -> impl Iterator<Item = (usize, bool)> + 'a {
    let (single, cycle) = match unit {
        Unit::Direct(i) => (Some((*i, false)), 0..0),
        Unit::State(i) => (Some((*i, true)), 0..0),
        Unit::Cycle {
            state_outputs,
            direct,
        } => (None, state_outputs.start..direct.end),
    };
    let cycle = units
        .members
        .get(cycle)
        .unwrap_or(&[])
        .iter()
        .map(|&i| (i, true));
    single.into_iter().chain(cycle)
}

/// Groups the units of a plan of `instances` instances into tasks; `cost_ns`
/// is each instance's module type's cost in ns per frame.
pub(crate) fn tasks(units: &Units, cost_ns: &[f64], instances: usize, task_min_ns: u64) -> Tasks {
    let unit_cost = |unit: &Unit| -> f64 {
        members(units, unit)
            .map(|(i, per_call)| {
                let cost = cost_ns
                    .get(i)
                    .copied()
                    .unwrap_or(f64::from(DEFAULT_COST_NS));
                cost * BLOCK_FACTOR + if per_call { PER_CALL_NS } else { 0.0 }
            })
            .sum()
    };
    let minimum = task_min_ns as f64;

    let mut ranges: Vec<Span<usize>> = Vec::new();
    let mut estimate = 0.0;
    let mut current = 0.0;
    for (u, unit) in units.order.iter().enumerate() {
        let feeds = u
            .checked_sub(1)
            .and_then(|last| units.successors.get(last))
            .is_some_and(|next| next.binary_search(&u).is_ok());
        let joins = feeds && current * CHAIN_FRAMES < minimum;
        match ranges.last_mut() {
            Some(range) if joins => range.end = u + 1,
            _ => {
                ranges.push(u..u + 1);
                current = 0.0;
            }
        }
        let cost = unit_cost(unit);
        current += cost;
        estimate += cost;
    }

    let mut unit_task = vec![0u32; units.order.len()];
    let mut instance_task = vec![0u32; instances];
    for (t, range) in ranges.iter().enumerate() {
        for u in range.clone() {
            if let Some(slot) = unit_task.get_mut(u) {
                *slot = t as u32;
            }
            if let Some(unit) = units.order.get(u) {
                for (i, _) in members(units, unit) {
                    if let Some(slot) = instance_task.get_mut(i) {
                        *slot = t as u32;
                    }
                }
            }
        }
    }
    let mut after: Vec<BTreeSet<u32>> = vec![BTreeSet::new(); ranges.len()];
    for (u, next) in units.successors.iter().enumerate() {
        let from = unit_task.get(u).copied().unwrap_or(0);
        for &v in next {
            let to = unit_task.get(v).copied().unwrap_or(from);
            if to != from
                && let Some(set) = after.get_mut(from as usize)
            {
                set.insert(to);
            }
        }
    }
    let mut preds = vec![0u32; ranges.len()];
    for set in &after {
        for &to in set {
            if let Some(p) = preds.get_mut(to as usize) {
                *p += 1;
            }
        }
    }
    let n = ranges.len();
    Tasks {
        ranges,
        instance_task,
        preds,
        successors: after.into_iter().map(|s| s.into_iter().collect()).collect(),
        estimate_ns: estimate,
        staged: vec![0; n],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::units;

    /// A chain 0 → 1 → 2 → 3 and an unconnected 4, all direct.
    fn chain() -> Units {
        let edges = [(0, 1), (1, 2), (2, 3)];
        units(5, &edges, &[0, 1, 2, 3, 4], &[true; 5])
    }

    #[test]
    fn without_a_minimum_every_unit_is_a_task() {
        let tasks = tasks(&chain(), &[10.0; 5], 5, 0);
        assert_eq!(tasks.len(), 5);
        assert_eq!(tasks.preds, vec![0, 1, 1, 1, 0]);
        assert_eq!(
            tasks.successors,
            vec![vec![1], vec![2], vec![3], vec![], vec![]]
        );
        assert_eq!(tasks.instance_task, vec![0, 1, 2, 3, 4]);
    }

    /// Cheap units chain along their edges up to the minimum, never across an
    /// unconnected unit.
    #[test]
    fn cheap_units_chain_along_edges() {
        // 5 ns · 0.54 · 256 ≈ 690 ns per unit: a task takes units while it
        // is below 2 000 ns, so three; the unconnected unit 4 never joins.
        let tasks = tasks(&chain(), &[5.0; 5], 5, 2_000);
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks.units(0), 0..3);
        assert_eq!(tasks.units(1), 3..4);
        assert_eq!(tasks.units(2), 4..5);
        assert_eq!(tasks.preds, vec![0, 1, 0]);
        assert_eq!(tasks.instance_task, vec![0, 0, 0, 1, 2]);
        // An expensive unit is a task of its own.
        assert_eq!(super::tasks(&chain(), &[50.0; 5], 5, 2_000).len(), 5);
    }

    /// The grouping depends only on the plan, not on how its edges are listed.
    #[test]
    fn chaining_is_deterministic() {
        let edges = [(0, 2), (1, 2), (2, 3), (4, 5)];
        let mut reversed = edges;
        reversed.reverse();
        let position = [0, 1, 2, 3, 4, 5];
        let a = units(6, &edges, &position, &[true; 6]);
        let b = units(6, &reversed, &position, &[true; 6]);
        let (ta, tb) = (
            tasks(&a, &[5.0; 6], 6, 2_000),
            tasks(&b, &[5.0; 6], 6, 2_000),
        );
        assert_eq!(ta.ranges, tb.ranges);
        assert_eq!(ta.successors, tb.successors);
    }

    /// The estimate: costs times the block factor, plus the per-call cost of
    /// state-output and cycle units.
    #[test]
    fn the_estimate_adds_the_per_call_cost() {
        let units = units(2, &[(1, 0)], &[1, 0], &[true, false]);
        let tasks = tasks(&units, &[10.0, 10.0], 2, 0);
        let expected = 10.0 * BLOCK_FACTOR * 2.0 + PER_CALL_NS;
        assert!((tasks.estimate_ns - expected).abs() < 1e-9);
    }
}
