//! AC5 and AC6 of M2.7-2 on noop devices: the pool owns one service (and
//! thread) per device; a lost device's service is replaced by a new
//! generation, with no limit on generations, and the modules' channels take
//! the replacement without blocking; a failed replacement is retried; the
//! watcher sleeps until a loss.

signal_contract::install_counting_allocator!();

use std::sync::Arc;
use std::time::{Duration, Instant};

use signal_contract::alloc_counter::Guard;
use signal_gpu::{
    Channel, DevicePool, Job, KernelSpec, Outcome, PlacedChannel, ServiceState, SlotStatus,
};

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

fn wait(channel: &mut Channel) -> Job {
    let start = Instant::now();
    while channel.peek_done().is_none() {
        assert!(start.elapsed() < Duration::from_secs(10), "no result");
        std::thread::yield_now();
    }
    channel.take_done().unwrap()
}

/// Waits until `pool`'s device 0 satisfies `done`.
fn until(pool: &DevicePool, what: &str, done: impl Fn(&SlotStatus) -> bool) -> SlotStatus {
    let start = Instant::now();
    loop {
        let status = pool.status().remove(0);
        if done(&status) {
            return status;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "{what}: {status:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Takes the replacement once the pool offers it.
fn refreshed(channel: &mut PlacedChannel) {
    let start = Instant::now();
    while !channel.refresh(|_| {}) {
        assert!(start.elapsed() < Duration::from_secs(10), "no replacement");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// AC5: one service, with its own thread, per device; all running.
#[test]
fn one_service_per_device() {
    let pool = DevicePool::noop(3).unwrap();
    let status = pool.status();
    let threads: Vec<&str> = status.iter().map(|s| s.thread.as_str()).collect();
    assert_eq!(
        threads,
        [
            "signal-gpu-noop-0",
            "signal-gpu-noop-1",
            "signal-gpu-noop-2"
        ]
    );
    assert!(
        status
            .iter()
            .all(|s| s.generation == 1 && s.state == Some(ServiceState::Running))
    );
}

/// AC6: a lost device gets a new generation; the channel made on the old one
/// gets a replacement, whose jobs run. The job held from the old channel goes
/// back to it before the swap (auditor-1-003).
#[test]
fn a_lost_device_is_replaced_and_its_channels_rebound() {
    let pool = Arc::new(DevicePool::noop(1).unwrap());
    let mut channel = DevicePool::lease(&pool, 0).channel(SPEC, 2).unwrap();
    assert!(channel.submit(1, |_| {}));
    let held = wait(&mut channel);
    assert_eq!(held.outcome(), Outcome::Ok);
    let old = channel.id();
    pool.lose_device(0);
    let status = until(&pool, "replaced", |s| s.generation == 2);
    assert_eq!(status.state, Some(ServiceState::Running));
    let mut held = Some(held);
    let mut given_back = None;
    let start = Instant::now();
    while !channel.refresh(|old| given_back = held.take().map(|job| old.recycle(job).is_ok())) {
        assert!(start.elapsed() < Duration::from_secs(10), "no replacement");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        given_back,
        Some(true),
        "the held job went back to its own channel"
    );
    assert_ne!(channel.id(), old);
    assert!(channel.submit(2, |_| {}));
    let job = wait(&mut channel);
    assert_eq!((job.seq(), job.outcome()), (2, Outcome::Ok));
}

/// AC6 (auditor-1-002): there is no limit on generations.
#[test]
fn every_loss_gets_a_new_generation() {
    let pool = Arc::new(DevicePool::noop(1).unwrap());
    let mut channel = DevicePool::lease(&pool, 0).channel(SPEC, 1).unwrap();
    for generation in 2..=11 {
        pool.lose_device(0);
        until(&pool, "replaced", |s| {
            s.generation == generation && s.state == Some(ServiceState::Running)
        });
        refreshed(&mut channel);
    }
    assert!(channel.submit(0, |_| {}));
    assert_eq!(wait(&mut channel).outcome(), Outcome::Ok);
}

/// AC6: while the adapter cannot be opened the slot stays lost with its
/// reason and is retried; once it can, it is replaced.
#[test]
fn a_failed_replacement_is_retried() {
    let pool = DevicePool::noop(1).unwrap();
    pool.refuse_replacement(true);
    pool.lose_device(0);
    let status = until(&pool, "lost, with its reason", |s| {
        s.state.is_none()
            && s.last_error
                .as_deref()
                .is_some_and(|e| e.contains("refused"))
    });
    assert_eq!(status.generation, 1);
    pool.refuse_replacement(false);
    until(&pool, "replaced after a retry", |s| {
        s.generation == 2 && s.state == Some(ServiceState::Running)
    });
}

/// While replacement is held the device stays lost, as before M2.7-2; a
/// channel made then is lost at once, and is rebound once released.
#[test]
fn a_channel_made_while_lost_is_rebound_later() {
    let pool = Arc::new(DevicePool::noop(1).unwrap());
    pool.hold_replacement(true);
    pool.lose_device(0);
    std::thread::sleep(Duration::from_millis(200));
    let status = until(&pool, "lost", |s| s.state == Some(ServiceState::Lost));
    assert_eq!(status.generation, 1);
    let mut channel = DevicePool::lease(&pool, 0).channel(SPEC, 1).unwrap();
    assert!(channel.is_lost() && channel.service_lost());
    pool.hold_replacement(false);
    refreshed(&mut channel);
    assert!(!channel.service_lost());
    assert!(channel.submit(0, |_| {}));
    assert_eq!(wait(&mut channel).outcome(), Outcome::Ok);
}

/// auditor-2-007: the watcher sleeps until a loss.
#[test]
fn the_watcher_sleeps_until_a_loss() {
    let pool = DevicePool::noop(2).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(pool.watcher_wakes(), 0);
    pool.lose_device(1);
    let start = Instant::now();
    while pool.status()[1].generation < 2 {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(pool.watcher_wakes() >= 1);
    assert_eq!(
        pool.status()[0].generation,
        1,
        "the other device is untouched"
    );
}

/// auditor-1-005: a module that takes no replacement across more losses
/// than its offer queue holds still reaches the running generation once it
/// refreshes again, and its jobs run there.
#[test]
fn a_module_that_missed_many_generations_reaches_the_current_one() {
    let pool = Arc::new(DevicePool::noop(1).unwrap());
    let mut channel = DevicePool::lease(&pool, 0).channel(SPEC, 1).unwrap();
    for generation in 2..=8 {
        pool.lose_device(0);
        until(&pool, "replaced", |s| {
            s.generation == generation && s.state == Some(ServiceState::Running)
        });
    }
    // Every older generation's channel is lost; the current one is not.
    let start = Instant::now();
    let mut refreshes = 0;
    while channel.service_lost() {
        refreshes += usize::from(channel.refresh(|_| {}));
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "still on a lost generation after {refreshes} refreshes"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    eprintln!("{refreshes} refreshes to the current generation");
    assert!(channel.submit(0, |_| {}));
    assert_eq!(wait(&mut channel).outcome(), Outcome::Ok);
}

/// auditor-2-019: a channel that could not be made on a new generation is
/// tried again by the watcher itself, without another loss or wake.
#[test]
fn a_failed_offer_is_retried() {
    let pool = Arc::new(DevicePool::noop(1).unwrap());
    let mut channel = DevicePool::lease(&pool, 0).channel(SPEC, 1).unwrap();
    pool.refuse_offers(true);
    pool.lose_device(0);
    until(&pool, "replaced", |s| {
        s.generation == 2 && s.state == Some(ServiceState::Running)
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(!channel.refresh(|_| {}), "no channel could be made");
    pool.refuse_offers(false);
    refreshed(&mut channel);
    assert!(!channel.service_lost());
    assert!(channel.submit(0, |_| {}));
    assert_eq!(wait(&mut channel).outcome(), Outcome::Ok);
}

/// Taking a replacement neither allocates nor frees on the caller.
#[test]
fn refresh_does_not_allocate() {
    let pool = Arc::new(DevicePool::noop(1).unwrap());
    let mut channel = DevicePool::lease(&pool, 0).channel(SPEC, 1).unwrap();
    pool.lose_device(0);
    until(&pool, "replaced", |s| s.generation == 2);
    let start = Instant::now();
    loop {
        let guard = Guard::start();
        let swapped = channel.refresh(|_| {});
        let counts = guard.counts();
        drop(guard);
        assert!(counts.is_zero(), "{counts:?}");
        if swapped {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(5));
    }
}
