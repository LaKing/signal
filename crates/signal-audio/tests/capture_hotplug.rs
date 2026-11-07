//! ADR 0038 §8 hot-plug (the owner's AT2020, 2026-11-07): a capture side
//! from another device keeps its run source while the device goes and
//! comes back. The control thread hands the source a new adapter (or none);
//! the audio thread swaps it in at its next read, allocates and frees
//! nothing, and hands the old one back to be dropped off it. With none the
//! source delivers nothing, so the run reads 0 and counts it missing.

signal_contract::install_counting_allocator!();

use std::sync::Arc;
use std::time::Instant;

use signal_audio::{AdaptedSource, SecondaryStats, swap_source};
use signal_contract::alloc_counter::{self, Counts, Guard};
use signal_output::adapter::{AdapterConfig, clock_adapter};
use signal_output::inputs::InputSource;

/// A converting end from a device at `rate` into 48 kHz, filling.
fn source(rate: u32, epoch: Instant) -> AdaptedSource {
    let config = AdapterConfig::new(2, rate, 48_000, 256, 128);
    let (_input, output) = clock_adapter(config).unwrap();
    AdaptedSource::new(output, epoch, Arc::new(SecondaryStats::default()))
}

#[test]
fn a_device_gone_and_back_is_swapped_in_without_a_heap_call_on_the_audio_thread() {
    assert!(alloc_counter::is_installed());
    let epoch = Instant::now();
    let (mut slot, mut run) = swap_source();
    let mut out = vec![0.0f32; 128 * 2];
    let mut total = Counts::default();
    let mut read = |run: &mut signal_audio::SwapSource, out: &mut [f32]| {
        let guard = Guard::start();
        let got = run.read(out, 0);
        let counts = guard.counts();
        drop(guard);
        total.allocations += counts.allocations;
        total.deallocations += counts.deallocations;
        got
    };
    // Absent at the start: nothing delivered.
    assert_eq!(read(&mut run, &mut out), 0);
    // It opens: a whole block delivered (silence while its adapter fills).
    assert!(slot.set(Some(source(44_100, epoch))));
    assert_eq!(read(&mut run, &mut out), 128);
    assert_eq!(read(&mut run, &mut out), 128);
    // It goes away: nothing again, the old adapter handed back.
    assert!(slot.set(None));
    assert_eq!(read(&mut run, &mut out), 0);
    slot.collect();
    // Back, at another rate.
    assert!(slot.set(Some(source(96_000, epoch))));
    assert_eq!(read(&mut run, &mut out), 128);
    // Replaced while one plays: the newest wins.
    assert!(slot.set(Some(source(48_000, epoch))));
    assert_eq!(read(&mut run, &mut out), 128);
    slot.collect();
    assert!(out.iter().all(|x| x.is_finite()));
    assert_eq!(
        total,
        Counts::default(),
        "heap calls in the audio thread's reads"
    );
}

#[test]
fn hand_overs_beyond_the_queue_wait_for_the_next_poll() {
    let epoch = Instant::now();
    let (mut slot, mut run) = swap_source();
    let mut sent = 0;
    while slot.set(Some(source(44_100, epoch))) {
        sent += 1;
        assert!(sent < 100, "the queue is bounded");
    }
    assert!(sent >= 1);
    let mut out = vec![0.0f32; 128 * 2];
    assert_eq!(run.read(&mut out, 0), 128);
    slot.collect();
}
