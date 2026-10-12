//! Units of block execution (ADR 0019 §1, `docs/semantics.md` §13): the
//! plan's strongly connected components, in a deterministic dependency order.
//! Computed when a plan is instantiated (control-thread code; it allocates).

use std::collections::BTreeSet;
use std::ops::Range as Span;

/// How a unit runs the frames of a block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Unit {
    /// An acyclic direct instance: one `process` over the frames.
    Direct(usize),
    /// An acyclic state-output instance: `emit`, then `absorb`, one frame at a
    /// time (`emit` gets no inputs, so it cannot cover later frames).
    State(usize),
    /// A cycle, run in sub-blocks of one frame, its shortest possible loop
    /// delay: each frame its state-output members emit, its direct members
    /// process in schedule order, and its state-output members absorb. Both
    /// are ranges of [`Units::members`].
    Cycle {
        state_outputs: Span<usize>,
        direct: Span<usize>,
    },
}

/// The units of a plan, in execution order, and the members of its cycles.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Units {
    pub(crate) order: Vec<Unit>,
    pub(crate) members: Vec<usize>,
}

/// Condenses `count` instances, connected by `edges` (source instance,
/// receiving instance; every connection, from state-output sources too), into
/// strongly connected components. `position` is each instance's place in the
/// plan's schedule and `direct` its feedthrough class. The units come in a
/// topological order of the condensation, ties broken by the earliest
/// schedule position among a unit's members, so the order depends only on
/// the plan.
pub(crate) fn units(
    count: usize,
    edges: &[(usize, usize)],
    position: &[usize],
    direct: &[bool],
) -> Units {
    let mut successors = vec![Vec::new(); count];
    for &(from, to) in edges {
        if let Some(list) = successors.get_mut(from)
            && to < count
        {
            list.push(to);
        }
    }
    let component = components(&successors);
    let groups = component.iter().copied().max().map_or(0, |m| m + 1);

    // Each component's members, the earliest schedule position among them,
    // and the condensation's edges.
    let mut members_of = vec![Vec::new(); groups];
    for (i, &c) in component.iter().enumerate() {
        if let Some(list) = members_of.get_mut(c) {
            list.push(i);
        }
    }
    let first = |c: usize| {
        members_of
            .get(c)
            .into_iter()
            .flatten()
            .filter_map(|&i| position.get(i).copied())
            .min()
            .unwrap_or(usize::MAX)
    };
    let mut after: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); groups];
    let mut pending = vec![0usize; groups];
    for &(from, to) in edges {
        let (Some(&a), Some(&b)) = (component.get(from), component.get(to)) else {
            continue;
        };
        if a != b
            && let Some(set) = after.get_mut(a)
            && set.insert(b)
            && let Some(p) = pending.get_mut(b)
        {
            *p += 1;
        }
    }

    // Kahn's algorithm, the earliest schedule position first.
    let mut ready: BTreeSet<(usize, usize)> = (0..groups)
        .filter(|&c| pending.get(c) == Some(&0))
        .map(|c| (first(c), c))
        .collect();
    let mut units = Units::default();
    while let Some((_, c)) = ready.pop_first() {
        let list = members_of.get(c).map_or(&[][..], Vec::as_slice);
        match list {
            [i] if direct.get(*i) == Some(&true) => units.order.push(Unit::Direct(*i)),
            [i] => units.order.push(Unit::State(*i)),
            _ => {
                let start = units.members.len();
                units
                    .members
                    .extend(list.iter().filter(|&&i| direct.get(i) != Some(&true)));
                let split = units.members.len();
                let mut directs: Vec<usize> = list
                    .iter()
                    .copied()
                    .filter(|&i| direct.get(i) == Some(&true))
                    .collect();
                directs.sort_by_key(|&i| position.get(i).copied().unwrap_or(usize::MAX));
                units.members.extend(directs);
                units.order.push(Unit::Cycle {
                    state_outputs: start..split,
                    direct: split..units.members.len(),
                });
            }
        }
        for &b in after.get(c).into_iter().flatten() {
            if let Some(p) = pending.get_mut(b) {
                *p -= 1;
                if *p == 0 {
                    ready.insert((first(b), b));
                }
            }
        }
    }
    units
}

/// The strongly connected component of every vertex (Tarjan's algorithm,
/// iterative). Component numbers are arbitrary; the caller orders them.
fn components(successors: &[Vec<usize>]) -> Vec<usize> {
    const UNSEEN: usize = usize::MAX;
    let count = successors.len();
    let mut index = vec![UNSEEN; count];
    let mut low = vec![0usize; count];
    let mut on_stack = vec![false; count];
    let mut stack = Vec::new();
    let mut component = vec![UNSEEN; count];
    let mut next_index = 0;
    let mut next_component = 0;
    // (vertex, next successor to visit)
    let mut calls: Vec<(usize, usize)> = Vec::new();
    for root in 0..count {
        if index.get(root) != Some(&UNSEEN) {
            continue;
        }
        calls.push((root, 0));
        while let Some(&mut (v, ref mut next)) = calls.last_mut() {
            if *next == 0 && index.get(v) == Some(&UNSEEN) {
                set(&mut index, v, next_index);
                set(&mut low, v, next_index);
                next_index += 1;
                stack.push(v);
                set(&mut on_stack, v, true);
            }
            let successor = successors.get(v).and_then(|s| s.get(*next)).copied();
            if let Some(w) = successor {
                *next += 1;
                if index.get(w) == Some(&UNSEEN) {
                    calls.push((w, 0));
                } else if on_stack.get(w) == Some(&true) {
                    let lowest = low.get(v).copied().min(index.get(w).copied());
                    if let Some(lowest) = lowest {
                        set(&mut low, v, lowest);
                    }
                }
                continue;
            }
            calls.pop();
            if low.get(v) == index.get(v) {
                while let Some(w) = stack.pop() {
                    set(&mut on_stack, w, false);
                    set(&mut component, w, next_component);
                    if w == v {
                        break;
                    }
                }
                next_component += 1;
            }
            if let Some(&(parent, _)) = calls.last() {
                let lowest = low.get(parent).copied().min(low.get(v).copied());
                if let Some(lowest) = lowest {
                    set(&mut low, parent, lowest);
                }
            }
        }
    }
    component
}

fn set<T>(values: &mut [T], i: usize, value: T) {
    if let Some(slot) = values.get_mut(i) {
        *slot = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0 → 1 → 2 → 3 → 1 (a loop closed through the state-output 3), and 4,
    /// a state-output instance outside the loop, feeding 0.
    #[test]
    fn a_loop_is_one_unit_and_the_rest_are_single_units() {
        let edges = [(0, 1), (1, 2), (2, 3), (3, 1), (4, 0)];
        let direct = [true, true, true, false, false];
        let position = [1, 2, 3, 4, 0];
        let units = units(5, &edges, &position, &direct);
        assert_eq!(
            units.order,
            vec![
                Unit::State(4),
                Unit::Direct(0),
                Unit::Cycle {
                    state_outputs: 0..1,
                    direct: 1..3
                },
            ]
        );
        assert_eq!(units.members, vec![3, 1, 2]);
    }

    /// The order depends only on the plan: any listing of the edges gives the
    /// same units, and unconnected instances follow the schedule.
    #[test]
    fn the_order_is_deterministic() {
        let edges = [(0, 2), (1, 2), (2, 3), (3, 2), (4, 5)];
        let direct = [true, true, true, false, true, true];
        let position = [0, 1, 2, 3, 4, 5];
        let expected = units(6, &edges, &position, &direct);
        let mut reversed = edges;
        reversed.reverse();
        assert_eq!(units(6, &reversed, &position, &direct), expected);
        assert_eq!(
            expected.order.first(),
            Some(&Unit::Direct(0)),
            "{expected:?}"
        );
        let later = units(6, &edges, &[4, 5, 2, 3, 0, 1], &direct);
        assert_eq!(later.order.first(), Some(&Unit::Direct(4)), "{later:?}");
    }

    /// A state-output instance that feeds itself is a unit of its own.
    #[test]
    fn a_self_loop_is_a_state_unit() {
        let units = units(1, &[(0, 0)], &[0], &[false]);
        assert_eq!(units.order, vec![Unit::State(0)]);
    }
}
