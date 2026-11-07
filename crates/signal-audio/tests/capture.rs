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
        48_000,
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
        48_000,
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

/// An input that records the offset of every read.
struct Probe(Arc<std::sync::Mutex<Vec<usize>>>);

impl signal_output::inputs::InputSource for Probe {
    fn read(&mut self, _out: &mut [f32], offset: usize) -> usize {
        if let Ok(mut offsets) = self.0.lock() {
            offsets.push(offset);
        }
        0
    }
}

/// A device callback longer than the clock callback's scratch is run in
/// pieces; each piece's reads carry their offset in the whole callback
/// (0, 128, 256, 384 for 512 frames in pieces of 128), never 0 again as if
/// each piece were a new callback.
#[test]
fn a_callback_run_in_pieces_keeps_its_inputs_offsets() {
    use signal_audio::{CallbackStats, ChannelMap, OutputCallback};
    use signal_output::inputs::RunInput;
    let mut registry = signal_core::Registry::new();
    signal_std::register(&mut registry).unwrap();
    let project = signal_project::Project::from_sources(vec![signal_project::SourceFile {
        path: "project.toml".to_owned(),
        text: "schema_version = 1\n[project]\nname = \"pieces\"\nsample_rate = 48000\n\
               [instances]\nmic = { device = { capture = \"mono\", role = \"main\" } }\n\
               out = { module = \"std.audio_out\" }\n\
               [wiring]\nconnections = [{ from = \"mic.m\", to = \"out.l\" }]\n"
            .to_owned(),
    }])
    .unwrap();
    let plan = project.plan(&registry).unwrap().plan;
    let (mut runner, _controller) = signal_output::realtime(
        &plan,
        &registry,
        signal_engine::RealtimeConfig::new(48_000, 1),
    )
    .unwrap();
    let offsets = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (input, _) = RunInput::new(
        "/mic".parse().unwrap(),
        1,
        Box::new(Probe(Arc::clone(&offsets))),
        128,
        None,
    );
    runner.set_inputs(vec![input]);
    let stats = Arc::new(CallbackStats::default());
    let (mut callback, _control) =
        OutputCallback::new(runner, ChannelMap::new(2, 2), 128, 48_000, stats);
    let mut out = vec![0.0f32; 512 * 2];
    callback.fill(&mut out);
    let offsets = offsets.lock().unwrap().clone();
    for piece in [0, 128, 256, 384] {
        assert!(offsets.contains(&piece), "{offsets:?}");
    }
    assert!(offsets.windows(2).all(|w| w[1] > w[0]), "{offsets:?}");
}
