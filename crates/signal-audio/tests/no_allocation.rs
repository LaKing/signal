//! M2-4 AC7: the CoreAudio callback body, `OutputCallback::fill` (the Runner,
//! the channel map and the callback statistics with their `Instant`
//! measurement), allocates and frees nothing, measured with the counting
//! allocator of `signal-contract` (ADR 0017) around each call, including the
//! stop request that hands the Runner back. M2.5-1 AC5: so does the input
//! callback body, `InputCallback::capture` (metering only), run under the
//! same guard, including while its meter queue is full, and `fill` with an
//! explicit channel map.

signal_contract::install_counting_allocator!();

use std::path::Path;
use std::sync::Arc;

use signal_audio::{CallbackStats, ChannelMap, InputCallback, OutputCallback, rt_thread};
use signal_contract::alloc_counter::{self, Counts, Guard};
use signal_core::Registry;
use signal_engine::{
    CallbackSizes, RealtimeConfig, RtOptions, RtPolicy, Schedule, realtime, realtime_with,
};
use signal_project::Project;

/// M2.7-1 C10: under every wake policy, with the Mach semaphores of
/// `semaphore` and the wake-up trace, the callback body on 4 threads (every
/// segment on the workers) allocates and frees nothing on the audio thread:
/// the callback-start wakes (`semaphore_signal`, `Thread::unpark`) included.
#[test]
fn the_callback_allocates_nothing_under_every_wake_policy() {
    assert!(alloc_counter::is_installed());
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/kick");
    let project = Project::load(&dir).unwrap();
    let plan = project.plan(&registry).unwrap().plan;
    let rate = project.sample_rate;
    for policy in RtPolicy::ALL {
        let mut config = RealtimeConfig::with_buffer(rate, project.seed, 128);
        config.engine.threads = 4;
        config.engine.schedule = Schedule::FORCED;
        config.rt = RtOptions {
            policy,
            trace: true,
            ..RtOptions::default()
        };
        let (runner, _controller) =
            realtime_with(&plan, &registry, config, Some(rt_thread::mach_semaphores)).unwrap();
        let expected = if policy == RtPolicy::Semaphore {
            "semaphore"
        } else {
            "park"
        };
        assert_eq!(runner.schedule_stats().waiter, expected);
        let stats = Arc::new(CallbackStats::default());
        let map = ChannelMap::new(2, 2);
        let (mut callback, _control) =
            OutputCallback::new(runner, map, 128, rate, Arc::clone(&stats));
        let mut out = vec![0.0f32; 128 * 2];
        let mut total = Counts::default();
        for n in 0..2_000 {
            // Now and then a gap, so that workers wait when the next starts.
            if n % 50 == 0 {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let guard = Guard::start();
            callback.fill(&mut out);
            let counts = guard.counts();
            drop(guard);
            total.allocations += counts.allocations;
            total.deallocations += counts.deallocations;
        }
        assert!(total.is_zero(), "{policy}: {total:?}");
    }
}

#[test]
fn the_callback_allocates_and_frees_nothing() {
    assert!(alloc_counter::is_installed());
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/kick");
    let project = Project::load(&dir).unwrap();
    let plan = project.plan(&registry).unwrap().plan;
    let rate = project.sample_rate;
    let config = RealtimeConfig::new(rate, project.seed);
    let (runner, mut controller) = realtime(&plan, &registry, config).unwrap();

    let stats = Arc::new(CallbackStats::default());
    // The stereo sink on device channels 4 and 3 of 4.
    let map = ChannelMap::with_targets(2, 4, vec![3, 2]).unwrap();
    let (mut callback, mut control) =
        OutputCallback::new(runner, map, 256, rate, Arc::clone(&stats));
    // A 2-channel input with room for 64 meters, drained every 16 callbacks
    // except from callback 1 000 to 2 000, when its queue fills.
    let (mut input_callback, mut input_meters) = InputCallback::new(2, 64);
    let input: Vec<f32> = (0..1_024 * 2)
        .map(|i| ((i % 7) as f32 - 3.0) / 3.0)
        .collect();
    let mut metered = 0u64;
    // Device buffers up to 1 024 frames, so larger ones run in pieces.
    let mut out = vec![0.0f32; 1_024 * 4];
    let mut sizes = CallbackSizes::new(4);
    let mut total = Counts::default();
    for callback_index in 0..4_000 {
        if callback_index == 3_990 {
            control.request_stop();
        }
        let frames = sizes.next_size().min(1_024);
        let buffer = &mut out[..frames * 4];
        let captured = &input[..frames * 2];
        let guard = Guard::start();
        callback.fill(buffer);
        input_callback.capture(captured);
        let counts = guard.counts();
        drop(guard);
        total.allocations += counts.allocations;
        total.deallocations += counts.deallocations;
        if callback_index % 16 == 0 {
            controller.poll(&mut ());
            if !(1_000..2_000).contains(&callback_index) {
                input_meters.drain(|_| metered += 1);
            }
        }
    }
    assert!(
        control.take_runner().is_some(),
        "the stop request was served"
    );
    assert_eq!(stats.snapshot().callbacks, 4_000);
    input_meters.drain(|_| metered += 1);
    assert!(input_meters.overflows() > 0, "the full input queue was run");
    assert_eq!(metered + input_meters.overflows(), 4_000 * 2);
    assert!(
        total.is_zero(),
        "OutputCallback::fill or InputCallback::capture allocated {} and freed {} times",
        total.allocations,
        total.deallocations
    );
}
