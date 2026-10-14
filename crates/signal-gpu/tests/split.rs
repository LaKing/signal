//! AC8 of M2.7-2: the split hook on 3 noop devices. The split is fixed (part
//! *i* on device (first + *i*) mod *n*), the combine runs in part order once
//! every part is back, whatever order they finished in, a failed part fails
//! the job, and the module's side never allocates.

signal_contract::install_counting_allocator!();

use std::sync::Arc;
use std::time::{Duration, Instant};

use signal_contract::alloc_counter::Guard;
use signal_gpu::{DevicePool, KernelSpec, Outcome, SplitChannel};

const SPEC: KernelSpec = KernelSpec {
    wgsl: "@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x < 2u) { output[id.x] = input[id.x]; }
}",
    entry: "main",
    input_words: 2,
    output_words: 2,
    workgroups: (1, 1, 1),
};

fn wait_ready(split: &mut SplitChannel) -> u64 {
    let start = Instant::now();
    loop {
        if let Some(seq) = split.ready() {
            return seq;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "no result");
        std::thread::yield_now();
    }
}

#[test]
fn the_split_and_the_combine_order_are_fixed() {
    let pool = Arc::new(DevicePool::noop(3).unwrap());
    let mut split = SplitChannel::new(&pool, 1, SPEC, 4, 2).unwrap();
    let devices: Vec<_> = (0..4).map(|i| split.device_of(i).unwrap()).collect();
    assert_eq!(devices, [1, 2, 0, 1]);
    // Part 2 finishes last.
    split
        .part(2)
        .unwrap()
        .set_execution_delay(Duration::from_millis(200));
    let mut filled = Vec::new();
    assert!(split.submit(7, |i, input| {
        filled.push(i);
        input.fill(i as f32);
    }));
    assert_eq!(filled, [0, 1, 2, 3]);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(split.ready(), None, "part 2 is still on its device");
    assert_eq!(wait_ready(&mut split), 7);
    let mut order = Vec::new();
    let (seq, outcome) = split.take_combined(|i, _| order.push(i)).unwrap();
    assert_eq!((seq, outcome), (7, Outcome::Ok));
    assert_eq!(order, [0, 1, 2, 3]);
}

#[test]
fn a_failed_part_fails_the_job() {
    let pool = Arc::new(DevicePool::noop(3).unwrap());
    pool.hold_replacement(true);
    let mut split = SplitChannel::new(&pool, 0, SPEC, 3, 1).unwrap();
    pool.lose_device(2);
    assert!(split.submit(1, |_, _| {}));
    wait_ready(&mut split);
    let mut combined = false;
    let (_, outcome) = split.take_combined(|_, _| combined = true).unwrap();
    assert_eq!(outcome, Outcome::DeviceLost);
    assert!(!combined, "nothing is combined from a failed job");
}

#[test]
fn the_split_path_does_not_allocate() {
    let pool = Arc::new(DevicePool::noop(3).unwrap());
    let mut split = SplitChannel::new(&pool, 0, SPEC, 4, 1).unwrap();
    let guard = Guard::start();
    assert!(split.submit(3, |i, input| input.fill(i as f32)));
    let start = Instant::now();
    while split.ready().is_none() && start.elapsed() < Duration::from_secs(10) {
        std::hint::spin_loop();
    }
    let mut sum = 0.0;
    let taken = split.take_combined(|_, words| sum += words.iter().sum::<f32>());
    split.discard_before(3);
    let swapped = split.refresh();
    let counts = guard.counts();
    drop(guard);
    assert!(counts.is_zero(), "{counts:?}");
    assert_eq!(taken.map(|t| t.1), Some(Outcome::Ok));
    assert!(!swapped);
}

/// auditor-1-004, auditor-2-013: after one part's device is replaced, its
/// job in flight never comes back. The parts' first finished jobs then
/// differ: the oldest job is reported `DeviceLost` with nothing combined,
/// never a mix of two jobs; the next job is combined whole.
#[test]
fn a_replaced_part_never_mixes_two_jobs() {
    let pool = Arc::new(DevicePool::noop(2).unwrap());
    let mut split = SplitChannel::new(&pool, 0, SPEC, 2, 2).unwrap();
    assert!(split.submit(1, |i, input| input.fill(i as f32)));
    assert_eq!(wait_ready(&mut split), 1, "both parts of job 1 are back");
    pool.lose_device(0);
    let start = Instant::now();
    while pool.status()[0].generation < 2 || !split.refresh() {
        assert!(start.elapsed() < Duration::from_secs(10), "no replacement");
        std::thread::sleep(Duration::from_millis(5));
    }
    // Part 0's job 1 went with its old channel; part 1 still holds it.
    assert_eq!(split.ready(), Some(1), "the lost job is ready at once");
    assert!(split.submit(2, |i, input| input.fill(i as f32)));
    let mut combined = Vec::new();
    let first = split.take_combined(|i, _| combined.push(i));
    assert_eq!(first, Some((1, Outcome::DeviceLost)));
    assert!(combined.is_empty(), "nothing combined from a lost job");
    assert_eq!(wait_ready(&mut split), 2);
    let second = split.take_combined(|i, _| combined.push(i));
    assert_eq!(second, Some((2, Outcome::Ok)));
    assert_eq!(combined, [0, 1]);
}

/// auditor-2-021: a job in flight on the surviving part across the loss
/// comes back late, after both lost jobs were taken. `ready()` never answers
/// with it: it always names the job `take_combined` takes next.
#[test]
fn ready_names_what_take_combined_takes_after_a_partial_loss() {
    let pool = Arc::new(DevicePool::noop(2).unwrap());
    let mut split = SplitChannel::new(&pool, 0, SPEC, 2, 2).unwrap();
    split
        .part(1)
        .unwrap()
        .set_execution_delay(Duration::from_millis(400));
    assert!(split.submit(1, |i, input| input.fill(i as f32)));
    assert!(split.submit(2, |i, input| input.fill(i as f32)));
    pool.lose_device(0);
    let start = Instant::now();
    while pool.status()[0].generation < 2 || !split.refresh() {
        assert!(start.elapsed() < Duration::from_secs(10), "no replacement");
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut combined = Vec::new();
    for seq in [1, 2] {
        assert_eq!(split.ready(), Some(seq));
        assert_eq!(
            split.take_combined(|i, _| combined.push(i)),
            Some((seq, Outcome::DeviceLost))
        );
    }
    split.part(1).unwrap().set_execution_delay(Duration::ZERO);
    let start = Instant::now();
    while !split.submit(3, |i, input| input.fill(i as f32)) {
        assert!(start.elapsed() < Duration::from_secs(10), "not accepted");
        std::thread::sleep(Duration::from_millis(5));
    }
    // Part 1's late jobs 1 and 2 are back by now: stale, never ready.
    std::thread::sleep(Duration::from_millis(500));
    let ready = wait_ready(&mut split);
    let taken = split.take_combined(|i, _| combined.push(i));
    assert_eq!(taken.map(|t| t.0), Some(ready));
    assert_eq!(taken, Some((3, Outcome::Ok)));
    assert_eq!(combined, [0, 1]);
}

/// auditor-2-023: when a module discards a part's finished job itself, the
/// job is still reported (`DeviceLost`), never forgotten without an outcome.
#[test]
fn a_discarded_part_still_reports_its_job() {
    let pool = Arc::new(DevicePool::noop(2).unwrap());
    let mut split = SplitChannel::new(&pool, 0, SPEC, 2, 3).unwrap();
    for seq in 1..=3 {
        assert!(split.submit(seq, |i, input| input.fill(i as f32)));
    }
    assert_eq!(wait_ready(&mut split), 1);
    // The noop device returns jobs 2 and 3 right after 1.
    std::thread::sleep(Duration::from_millis(300));
    split.discard_before(3);
    assert_eq!(split.ready(), Some(1), "job 1 is still to be reported");
    let mut taken = Vec::new();
    while let Some(t) = split.take_combined(|_, _| {}) {
        taken.push(t);
    }
    assert_eq!(
        taken,
        [
            (1, Outcome::DeviceLost),
            (2, Outcome::DeviceLost),
            (3, Outcome::Ok)
        ]
    );
}

/// auditor-1-007, auditor-2-018: with one job per part, a replacement does
/// not stall the split: the lost job is taken as `DeviceLost` without another
/// submission, its surviving part goes back to its pool, and the next job is
/// accepted and combined whole.
#[test]
fn a_replacement_never_stalls_one_job_per_part() {
    let pool = Arc::new(DevicePool::noop(2).unwrap());
    let mut split = SplitChannel::new(&pool, 0, SPEC, 2, 1).unwrap();
    assert!(split.submit(1, |i, input| input.fill(i as f32)));
    assert_eq!(wait_ready(&mut split), 1);
    pool.lose_device(0);
    let start = Instant::now();
    while pool.status()[0].generation < 2 || !split.refresh() {
        assert!(start.elapsed() < Duration::from_secs(10), "no replacement");
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut combined = Vec::new();
    assert_eq!(
        split.take_combined(|i, _| combined.push(i)),
        Some((1, Outcome::DeviceLost))
    );
    assert!(combined.is_empty());
    assert!(split.submit(2, |i, input| input.fill(i as f32)), "accepted");
    assert_eq!(wait_ready(&mut split), 2);
    assert_eq!(
        split.take_combined(|i, _| combined.push(i)),
        Some((2, Outcome::Ok))
    );
    assert_eq!(combined, [0, 1]);
}
