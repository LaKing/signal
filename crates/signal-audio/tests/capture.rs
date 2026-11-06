//! ADR 0038 §8, ADR 0052 §8: the capture bodies without a device.
//!
//! - The clock device's input ring is pre-filled by one input period; a
//!   start that let it fill further (the input ran before the output) is
//!   cut back to that at the first read, so the latency is one period.
//! - A full ring drops and counts frames, never blocks.
//! - The input feed gathers each side's channels; a missing channel reads 0.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use signal_audio::{CaptureFeed, SideFeed, SideSink, capture_ring};
use signal_output::inputs::InputSource;

#[test]
fn the_ring_starts_one_period_behind_however_long_the_start_took() {
    let (tx, mut rx) = capture_ring(1, 64, 64);
    let dropped = Arc::new(AtomicU64::new(0));
    // Channel 1 of a two-channel device; a second side reads a channel the
    // device lacks.
    let mut feed = CaptureFeed::new(
        vec![SideFeed::new(
            vec![Some(1)],
            SideSink::Ring(tx),
            64,
            Arc::clone(&dropped),
        )],
        Instant::now(),
    );
    // Ten input callbacks before the output starts: the ring (4 · 64 + 64)
    // fills, then drops.
    for k in 0..10 {
        let data: Vec<f32> = (0..64).flat_map(|f| [0.0, (k * 64 + f) as f32]).collect();
        feed.capture(&data, 2);
    }
    assert!(
        dropped.load(Ordering::Relaxed) > 0,
        "a full ring drops and counts"
    );
    let mut out = vec![9.0f32; 64];
    // The first read: cut back to one period (the newest 64 frames held).
    assert_eq!(rx.read(&mut out, 0), 64);
    let mut next = vec![0.0f32; 64];
    assert_eq!(rx.read(&mut next, 64), 0, "nothing more held: one period");
    // What it read is contiguous material, the latest before the cut.
    assert!(out.windows(2).all(|w| w[1] == w[0] + 1.0), "{out:?}");
}

#[test]
fn a_channel_the_device_lacks_reads_0() {
    let (tx, mut rx) = capture_ring(2, 16, 16);
    let mut feed = CaptureFeed::new(
        vec![SideFeed::new(
            vec![Some(0), None],
            SideSink::Ring(tx),
            16,
            Arc::default(),
        )],
        Instant::now(),
    );
    let data: Vec<f32> = (0..16).flat_map(|f| [f as f32 + 1.0, 7.0]).collect();
    feed.capture(&data, 2);
    let mut out = vec![0.0f32; 32 * 2];
    let got = rx.read(&mut out, 0);
    // The prefill (16 frames of 0) was cut to one period: the captured 16.
    assert_eq!(got, 16);
    for f in 0..16 {
        assert_eq!(out[f * 2], f as f32 + 1.0);
        assert_eq!(out[f * 2 + 1], 0.0);
    }
}
