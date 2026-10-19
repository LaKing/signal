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
//!
//! The worker budget (M2.10-5, `docs/realtime-rules.md` "Worker threads"):
//! the thread count, up to the engine's, whose estimated segment time at
//! the block capacity, plus [`THREAD_GAIN_NS`] per thread beyond the first,
//! is the least: a greedy list schedule of the tasks over their estimates,
//! with a hand-off cost per task and each worker's wake latency
//! ([`makespan`]). In real time (M2.10-6), a plan whose serial estimate fits
//! in [`SERIAL_SHARE`] of the engine block's period runs on the calling
//! thread alone. Constants, measured once: never run-time timing. Timing
//! only: which threads run the tasks never changes the output.

use std::collections::{BTreeSet, VecDeque};
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

/// The worker budget's cost of handing a task to the board on several
/// threads, in ns per task per segment (claiming it, completing it,
/// releasing its successors, and its inputs moving between cores):
/// measured, not timed at run time. On the M5 Max (2026-10-18, release,
/// load average 2.2–2.8), kick-voice plans of 4, 16 and 80 voices at
/// *k* = 128 on 2–4 threads with spinning workers gave (T · t_T − t_1) /
/// tasks = 1.5–2.0 µs per task: an upper bound, since it also counts any
/// imbalance (`docs/benchmarks.md` "Idle cost, part 2 (M2.10-6)").
pub(crate) const HANDOFF_NS: f64 = 1_500.0;

/// The worker budget's wake latency of one hop of the wake tree, in ns:
/// the median first worker's join of `signal bench --realtime` (24 and 80
/// kick voices, time-constraint workers, 4.6–7.2 µs on the M5 Max,
/// 2026-10-18). Measured, not timed at run time.
pub(crate) const WAKE_HOP_NS: f64 = 6_000.0;

/// What each thread beyond the first must shorten a segment's estimate by
/// to be used, in ns: one wake hop ([`WAKE_HOP_NS`]), the latency a worker
/// costs before it helps. On the M5 Max this keeps every thread for 80
/// kick voices up to 18 threads (the 18th saves 6.9 µs per 128-frame
/// segment in the simulation) and gives the studio 8 of 12 offline, near
/// its measured useful width (`docs/benchmarks.md` "Idle cost, part 2
/// (M2.10-6)"); in real time [`SERIAL_SHARE`] decides first.
pub(crate) const THREAD_GAIN_NS: f64 = WAKE_HOP_NS;

/// In real time, the share of the engine block's period (the block capacity
/// at the sample rate; not the device buffer, which may be longer than the
/// 256-frame block) within which a plan's serial estimate keeps its worker
/// budget at 1 (M2.10-6): such a plan runs on the calling thread alone, and
/// no worker is woken for it. Parallelism buys deadline margin at the price
/// of CPU, since a worker that joins a segment spins in it until its last
/// task is claimed; a plan that fits in this share alone leaves most of the
/// period free. An assumption, not a measurement: below the workers'
/// default computation fraction of 0.25 (`--rt-computation`), so that a
/// plan pinned here still fits in the quarter unless its estimate is more
/// than 1.67× too low (the measured errors and the exception, physical-av,
/// are in `docs/realtime-rules.md` "The worker budget" and
/// `docs/benchmarks.md` "Idle cost, part 2 (M2.10-6)").
pub(crate) const SERIAL_SHARE: f64 = 0.15;

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
    /// The worker budget: the threads, the calling thread included, that
    /// the plan's estimated schedule can use (at least 1).
    pub(crate) budget: usize,
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
/// is each instance's module type's cost in ns per frame. The budget is
/// taken among 1 to `threads` threads, for segments of `frames` frames (the
/// block capacity), and in real time `period_ns` is that block's period
/// ([`worker_budget`]).
pub(crate) fn tasks(
    units: &Units,
    cost_ns: &[f64],
    instances: usize,
    task_min_ns: u64,
    threads: usize,
    frames: usize,
    period_ns: Option<f64>,
) -> Tasks {
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
    let mut task_cost: Vec<f64> = Vec::new();
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
                task_cost.push(0.0);
                current = 0.0;
            }
        }
        let cost = unit_cost(unit);
        current += cost;
        estimate += cost;
        if let Some(task) = task_cost.last_mut() {
            *task += cost;
        }
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
    let successors: Vec<Vec<u32>> = after.into_iter().map(|s| s.into_iter().collect()).collect();
    let budget = worker_budget(&task_cost, &preds, &successors, threads, frames, period_ns);
    Tasks {
        ranges,
        instance_task,
        preds,
        successors,
        estimate_ns: estimate,
        budget,
        staged: vec![0; n],
    }
}

/// The estimated time of one segment of `frames` frames on `lanes`
/// threads, in ns: a greedy list schedule of the tasks (`cost` ns per frame
/// each, released by their `preds` along `successors`). On one thread it is
/// the serial work, as the calling thread runs a segment alone. On more,
/// every task also costs [`HANDOFF_NS`] (claiming and completing it on the
/// board) and worker `i` (lane `i`, from 1) arrives only after its wake:
/// [`WAKE_HOP_NS`] per hop of `park50`'s wake tree (the open wakes worker 1,
/// each joiner two more, so worker `i` is ⌊log₂ i⌋ + 1 hops away). Ready
/// tasks wait in a FIFO, the first seeded in task order and each released
/// in its predecessor's successor order; the lowest idle lane that has
/// arrived takes the oldest; of tasks finishing together the lower index
/// finishes first. Deterministic: it depends only on its arguments.
fn makespan(
    cost: &[f64],
    preds: &[u32],
    successors: &[Vec<u32>],
    lanes: usize,
    frames: usize,
) -> f64 {
    let frames = frames.max(1) as f64;
    let lanes = lanes.max(1);
    if lanes == 1 {
        return cost.iter().sum::<f64>() * frames;
    }
    let time = |t: usize| cost.get(t).copied().unwrap_or(0.0) * frames + HANDOFF_NS;
    let mut pending = preds.to_vec();
    let mut ready: VecDeque<usize> = (0..cost.len())
        .filter(|&t| preds.get(t) == Some(&0))
        .collect();
    // Per lane: when it is next free (its arrival at first), and whether it
    // runs a task.
    let mut lane: Vec<(f64, bool)> = (0..lanes).map(|l| (arrival(l), false)).collect();
    // (finish time, task, lane) of the tasks running.
    let mut running: Vec<(f64, usize, usize)> = Vec::with_capacity(lanes);
    let mut now = 0.0;
    loop {
        for (l, (free, busy)) in lane.iter_mut().enumerate() {
            if *busy || *free > now {
                continue;
            }
            let Some(t) = ready.pop_front() else { break };
            *busy = true;
            running.push((now + time(t), t, l));
        }
        let first = running
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
            .map(|(i, r)| (i, r.0));
        // A lane still to arrive matters only while a task waits for one.
        let arrives = if ready.is_empty() {
            f64::INFINITY
        } else {
            lane.iter()
                .filter(|&&(free, busy)| !busy && free > now)
                .map(|&(free, _)| free)
                .fold(f64::INFINITY, f64::min)
        };
        match first {
            Some((_, finish)) if arrives < finish => now = arrives,
            None if arrives.is_finite() => now = arrives,
            None => return now,
            Some((i, _)) => {
                let (finish, t, l) = running.swap_remove(i);
                now = finish;
                if let Some(slot) = lane.get_mut(l) {
                    *slot = (finish, false);
                }
                for &s in successors.get(t).map_or(&[][..], |s| &s[..]) {
                    if let Some(p) = pending.get_mut(s as usize) {
                        *p = p.saturating_sub(1);
                        if *p == 0 {
                            ready.push_back(s as usize);
                        }
                    }
                }
            }
        }
    }
}

/// When lane `lane` can first take a task in a segment, in ns: 0 for the
/// calling thread, else its worker's hops in the wake tree times
/// [`WAKE_HOP_NS`] ([`makespan`]).
fn arrival(lane: usize) -> f64 {
    if lane == 0 {
        0.0
    } else {
        f64::from(lane.ilog2() + 1) * WAKE_HOP_NS
    }
}

/// The worker budget: of 1 to `threads` threads, the count `p` that
/// minimises the estimated segment time ([`makespan`] at `frames` frames)
/// plus [`THREAD_GAIN_NS`] for each thread beyond the first, the fewest on
/// a tie. So a thread is used only if the threads up to it shorten the
/// segment by at least [`THREAD_GAIN_NS`] each, and a list schedule that
/// takes longer on more threads never gets them. Control-thread work, at
/// instantiation and `prepare_swap`: one simulation per thread count, each
/// O(tasks × lanes), so O(threads² × tasks) in all. Negligible at real
/// counts (≤ 18 threads, hundreds of tasks); at the 1 024 threads
/// `MAX_THREADS` allows, a large plan would take seconds.
///
/// In real time (`period_ns`, the block's period), a plan whose serial
/// estimate is at most [`SERIAL_SHARE`] of the period gets 1 (M2.10-6).
fn worker_budget(
    cost: &[f64],
    preds: &[u32],
    successors: &[Vec<u32>],
    threads: usize,
    frames: usize,
    period_ns: Option<f64>,
) -> usize {
    let serial = makespan(cost, preds, successors, 1, frames);
    if period_ns.is_some_and(|period| serial <= SERIAL_SHARE * period) {
        return 1;
    }
    let mut best = (f64::INFINITY, 1);
    for p in 1..=threads.max(1) {
        let score = makespan(cost, preds, successors, p, frames) + THREAD_GAIN_NS * (p - 1) as f64;
        if score < best.0 {
            best = (score, p);
        }
    }
    best.1
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
        let tasks = tasks(&chain(), &[10.0; 5], 5, 0, 1, 256, None);
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
        let tasks = tasks(&chain(), &[5.0; 5], 5, 2_000, 1, 256, None);
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks.units(0), 0..3);
        assert_eq!(tasks.units(1), 3..4);
        assert_eq!(tasks.units(2), 4..5);
        assert_eq!(tasks.preds, vec![0, 1, 0]);
        assert_eq!(tasks.instance_task, vec![0, 0, 0, 1, 2]);
        // An expensive unit is a task of its own.
        assert_eq!(
            super::tasks(&chain(), &[50.0; 5], 5, 2_000, 1, 256, None).len(),
            5
        );
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
            tasks(&a, &[5.0; 6], 6, 2_000, 1, 256, None),
            tasks(&b, &[5.0; 6], 6, 2_000, 1, 256, None),
        );
        assert_eq!(ta.ranges, tb.ranges);
        assert_eq!(ta.successors, tb.successors);
    }

    /// The estimate: costs times the block factor, plus the per-call cost of
    /// state-output and cycle units.
    #[test]
    fn the_estimate_adds_the_per_call_cost() {
        let units = units(2, &[(1, 0)], &[1, 0], &[true, false]);
        let tasks = tasks(&units, &[10.0, 10.0], 2, 0, 1, 256, None);
        let expected = 10.0 * BLOCK_FACTOR * 2.0 + PER_CALL_NS;
        assert!((tasks.estimate_ns - expected).abs() < 1e-9);
    }

    /// `voices` chains of four units costing `chain` each, summed by a tree
    /// of 4-input units into one (the shape of `signal bench`'s kick
    /// voices), one task per unit; and the units' costs.
    fn voices(voices: usize, chain: [f64; 4]) -> (Units, Vec<f64>) {
        let mut edges = Vec::new();
        let mut cost = Vec::new();
        let mut level = Vec::new();
        for _ in 0..voices {
            let first = cost.len();
            cost.extend(chain);
            edges.extend((first..first + 3).map(|i| (i, i + 1)));
            level.push(first + 3);
        }
        while level.len() > 1 {
            let mut next = Vec::new();
            for group in level.chunks(4) {
                let sum = cost.len();
                cost.push(31.4);
                edges.extend(group.iter().map(|&i| (i, sum)));
                next.push(sum);
            }
            level = next;
        }
        let n = cost.len();
        let position: Vec<usize> = (0..n).collect();
        (units(n, &edges, &position, &vec![true; n]), cost)
    }

    /// AC2, M2.10-5: a chain gains nothing from a second thread, so its
    /// budget is 1 whatever the engine's threads; independent tasks long
    /// enough to be worth their hand-off and wake get the fewest threads
    /// that run them in as few rounds as T threads can (12 tasks on 8
    /// threads take two rounds, as on 6, so 6); tasks shorter than their
    /// hand-off stay on the calling thread.
    #[test]
    fn the_budget_is_what_the_estimated_schedule_can_use() {
        let chain = units(4, &[(0, 1), (1, 2), (2, 3)], &[0, 1, 2, 3], &[true; 4]);
        assert_eq!(tasks(&chain, &[500.0; 4], 4, 0, 8, 128, None).budget, 1);
        assert_eq!(tasks(&chain, &[500.0; 4], 4, 0, 1, 128, None).budget, 1);
        let independent = |n: usize| units(n, &[], &(0..n).collect::<Vec<_>>(), &vec![true; n]);
        for n in [1usize, 3, 7, 12] {
            for t in [1usize, 2, 8, 12, 18] {
                // 2 000 ns per frame: 138 µs per task at 128 frames.
                let budget = tasks(&independent(n), &vec![2_000.0; n], n, 0, t, 128, None).budget;
                let rounds = n.div_ceil(n.min(t));
                let fewest = (1..=t).find(|&p| n.div_ceil(p) == rounds).unwrap();
                assert_eq!(budget, fewest, "{n} tasks on {t} threads");
            }
        }
        // 80 tasks of 1 ns per frame: 69 ns each at 128 frames, far below
        // the hand-off.
        for t in [2usize, 12] {
            assert_eq!(
                tasks(&independent(80), &[1.0; 80], 80, 0, t, 128, None).budget,
                1
            );
        }
    }

    /// AC5, M2.10-5: 80 voices of unequal tasks into a sum tree are work
    /// bound on every thread count tried, so their budget is every thread at
    /// 128 frames. The reason is checked, not assumed: the last thread
    /// shortens the simulated segment by more than [`THREAD_GAIN_NS`]. (18
    /// threads are checked on the real kick voices, whose units differ:
    /// `crates/signal-cli/tests/blocks.rs`; with these costs the 18th saves
    /// 5.4 µs.)
    #[test]
    fn a_heavy_plan_gets_every_thread() {
        let (units, cost) = voices(80, [76.3, 37.9, 24.9, 43.2]);
        for t in [2usize, 8, 12] {
            let tasks = tasks(&units, &cost, cost.len(), 0, t, 128, None);
            let task_cost: Vec<f64> = (0..tasks.len())
                .map(|k| tasks.units(k).map(|u| cost[u] * BLOCK_FACTOR).sum())
                .collect();
            let time = |p| makespan(&task_cost, &tasks.preds, &tasks.successors, p, 128);
            let saved = time(t - 1) - time(t);
            assert!(
                saved > THREAD_GAIN_NS,
                "{t}: the last thread saves {saved} ns"
            );
            assert_eq!(tasks.budget, t, "80 voices on {t} threads");
        }
    }

    /// M2.10-6: in real time, a plan whose serial estimate is at most
    /// [`SERIAL_SHARE`] of the block's period gets one thread, and one
    /// beyond it gets the rule above; offline (no period) it always does.
    #[test]
    fn in_real_time_a_light_plan_runs_alone() {
        let independent = units(8, &[], &(0..8).collect::<Vec<_>>(), &[true; 8]);
        let cost = [2_000.0; 8];
        let offline = tasks(&independent, &cost, 8, 0, 8, 128, None).budget;
        assert_eq!(offline, 8);
        // The serial estimate: 8 tasks of 2 000 ns per frame at 128 frames.
        let serial: f64 = cost.iter().map(|c| c * BLOCK_FACTOR).sum::<f64>() * 128.0;
        let at = |share: f64| {
            let period = serial / share;
            tasks(&independent, &cost, 8, 0, 8, 128, Some(period)).budget
        };
        assert_eq!(at(0.14), 1);
        assert_eq!(at(0.16), offline);
    }

    /// The simulation's hand-off and wake terms: two independent tasks on
    /// two threads take the longer of the calling thread's task and the
    /// worker's (its wake, then its task), each with its hand-off.
    #[test]
    fn the_simulation_counts_hand_off_and_wake() {
        let cost = [100.0, 100.0];
        let one = makespan(&cost, &[0, 0], &[vec![], vec![]], 1, 128);
        assert!((one - 25_600.0).abs() < 1e-9);
        let two = makespan(&cost, &[0, 0], &[vec![], vec![]], 2, 128);
        assert!(
            (two - (WAKE_HOP_NS + 12_800.0 + HANDOFF_NS)).abs() < 1e-9,
            "{two}"
        );
        assert_eq!(arrival(1), WAKE_HOP_NS);
        assert_eq!(arrival(3), 2.0 * WAKE_HOP_NS);
        assert_eq!(arrival(4), 3.0 * WAKE_HOP_NS);
    }

    /// The budget depends only on the plan, not on how its edges are listed.
    #[test]
    fn the_budget_is_deterministic() {
        let edges = [(0, 3), (1, 3), (2, 4), (4, 5)];
        let mut reversed = edges;
        reversed.reverse();
        let position = [0, 1, 2, 3, 4, 5];
        let cost = [10.0, 30.0, 20.0, 5.0, 40.0, 15.0];
        let a = tasks(
            &units(6, &edges, &position, &[true; 6]),
            &cost,
            6,
            0,
            8,
            128,
            None,
        );
        let b = tasks(
            &units(6, &reversed, &position, &[true; 6]),
            &cost,
            6,
            0,
            8,
            128,
            None,
        );
        assert_eq!(a.budget, b.budget);
        assert!((1..=3).contains(&a.budget), "{}", a.budget);
    }
}
