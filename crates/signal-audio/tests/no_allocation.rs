//! M2-4 AC7: the CoreAudio callback body, `OutputCallback::fill` (the Runner,
//! the channel map and the callback statistics with their `Instant`
//! measurement), allocates and frees nothing, measured with the counting
//! allocator of `signal-contract` (ADR 0017) around each call, including the
//! stop request that hands the Runner back.

signal_contract::install_counting_allocator!();

use std::path::Path;
use std::sync::Arc;

use signal_audio::{CallbackStats, ChannelMap, OutputCallback};
use signal_contract::alloc_counter::{self, Counts, Guard};
use signal_core::Registry;
use signal_engine::{CallbackSizes, RealtimeConfig, realtime};
use signal_project::Project;

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
    let (mut callback, mut control) =
        OutputCallback::new(runner, ChannelMap::new(2, 2), 256, rate, Arc::clone(&stats));
    // Device buffers up to 1 024 frames, so larger ones run in pieces.
    let mut out = vec![0.0f32; 1_024 * 2];
    let mut sizes = CallbackSizes::new(4);
    let mut total = Counts::default();
    for callback_index in 0..4_000 {
        if callback_index == 3_990 {
            control.request_stop();
        }
        let frames = sizes.next_size().min(1_024);
        let buffer = &mut out[..frames * 2];
        let guard = Guard::start();
        callback.fill(buffer);
        let counts = guard.counts();
        drop(guard);
        total.allocations += counts.allocations;
        total.deallocations += counts.deallocations;
        if callback_index % 16 == 0 {
            controller.poll(&mut ());
        }
    }
    assert!(
        control.take_runner().is_some(),
        "the stop request was served"
    );
    assert_eq!(stats.snapshot().callbacks, 4_000);
    assert!(
        total.is_zero(),
        "OutputCallback::fill allocated {} and freed {} times",
        total.allocations,
        total.deallocations
    );
}
