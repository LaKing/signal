//! ADR 0038 §7–9, the audio adapter's input side in real time: a Runner
//! whose capture side (`std.audio_in` at `/mic`) reads a [`RecordedInput`].
//! `/mic.l` and `/mic.r` go straight to the audio sink, so the device output
//! is the input; the record holds exactly the words the engine read; past
//! the input's end the source reads 0, counted as missing. Neither the
//! callback nor the input allocates.

signal_contract::install_counting_allocator!();

use std::collections::BTreeMap;

use signal_contract::alloc_counter::{self, Counts, Guard};
use signal_core::Registry;
use signal_engine::RealtimeConfig;
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, InstanceDef, build_plan};
use signal_output::inputs::{RecordedInput, RunInput};
use signal_output::{CallbackSizes, realtime};

fn plan(registry: &Registry) -> ExecutionPlan {
    let instance = |name: &str, module: &str| InstanceDef {
        name: name.to_owned(),
        module: module.to_owned(),
        parameters: BTreeMap::new(),
        scope: None,
    };
    let connection = |from: &str, to: &str| ConnectionDef {
        from: from.to_owned(),
        to: to.to_owned(),
        scale: None,
    };
    let root = CompositeDef {
        seed: None,
        name: "project".to_owned(),
        instances: vec![
            instance("mic", "std.audio_in"),
            instance("out", "std.audio_out"),
        ],
        connections: vec![connection("mic.l", "out.l"), connection("mic.r", "out.r")],
        exports: Exports::default(),
        sleep: Vec::new(),
    };
    build_plan(&root, &[], registry, 0).unwrap()
}

const FRAMES: usize = 6_000;
const RECORDED: usize = 5_000;

#[test]
fn the_output_is_the_input_and_the_record_what_the_engine_read() {
    assert!(alloc_counter::is_installed());
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let plan = plan(&registry);
    let words: Vec<f32> = (0..RECORDED * 2)
        .map(|i| ((i as f32) * 0.001).sin() * 0.7)
        .collect();
    let (input, record) = RunInput::new(
        "/mic".parse().unwrap(),
        2,
        Box::new(RecordedInput::new(words.clone(), 2)),
        1_024,
        Some(FRAMES),
    );
    let counts = input.counts();
    let mut record = record.unwrap();
    let (mut runner, _controller) =
        realtime(&plan, &registry, RealtimeConfig::new(48_000, 1)).unwrap();
    runner.set_inputs(vec![input]);
    let mut out = vec![0.0f32; 1_024 * 2];
    let mut played = Vec::new();
    let mut sizes = CallbackSizes::new(5);
    let mut total = Counts::default();
    while played.len() < FRAMES * 2 {
        let frames = sizes.next_size().min(1_024).min(FRAMES - played.len() / 2);
        let guard = Guard::start();
        runner.process(&mut out[..frames * 2], 2);
        let counts = guard.counts();
        drop(guard);
        total.allocations += counts.allocations;
        total.deallocations += counts.deallocations;
        played.extend_from_slice(&out[..frames * 2]);
    }
    assert!(total.is_zero(), "{total:?}");
    // The output is the input (each word within [−1, 1]), then silence.
    assert_eq!(played[..RECORDED * 2], words[..]);
    assert!(played[RECORDED * 2..].iter().all(|&x| x == 0.0));
    // The record: exactly what the engine read, zeros past the end.
    let mut recorded = Vec::new();
    record.drain(&mut recorded);
    assert_eq!(recorded.len(), FRAMES * 2);
    assert_eq!(recorded[..RECORDED * 2], words[..]);
    assert!(recorded[RECORDED * 2..].iter().all(|&x| x == 0.0));
    assert!(record.complete());
    use std::sync::atomic::Ordering;
    assert_eq!(counts.frames.load(Ordering::Relaxed), FRAMES as u64);
    assert_eq!(
        counts.missing.load(Ordering::Relaxed),
        (FRAMES - RECORDED) as u64
    );
}

/// A capture side the plan does not have (another address, or another
/// width): the engine reads nothing, and the record says 0, as what it read.
#[test]
fn an_unbound_input_records_zeros() {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let plan = plan(&registry);
    let (input, record) = RunInput::new(
        "/elsewhere".parse().unwrap(),
        2,
        Box::new(RecordedInput::new(vec![0.5; 512], 2)),
        256,
        Some(256),
    );
    let mut record = record.unwrap();
    let (mut runner, _controller) =
        realtime(&plan, &registry, RealtimeConfig::new(48_000, 1)).unwrap();
    runner.set_inputs(vec![input]);
    let mut out = vec![0.0f32; 128 * 2];
    runner.process(&mut out, 2);
    assert!(out.iter().all(|&x| x == 0.0));
    let mut recorded = Vec::new();
    record.drain(&mut recorded);
    assert_eq!(recorded.len(), 256);
    assert!(recorded.iter().all(|&x| x == 0.0));
}

/// A live device's input that delivers `on` frames of 0.5, then nothing for
/// `off` frames, then 0.5 again, over and over (an unplug and a replug).
struct Flaky {
    on: usize,
    off: usize,
    at: usize,
}

impl signal_output::inputs::InputSource for Flaky {
    fn read(&mut self, out: &mut [f32], _offset: usize) -> usize {
        let frames = out.len() / 2;
        let mut got = 0;
        // Delivers up to its gap; a read can only miss its last frames, and
        // the time they stand for passes as well.
        while got < frames && self.at % (self.on + self.off) < self.on {
            if let Some(frame) = out.get_mut(got * 2..got * 2 + 2) {
                frame.fill(0.5);
            }
            got += 1;
            self.at += 1;
        }
        self.at += frames - got;
        got
    }
}

/// Plays `input` for `frames` frames in blocks of 64; the left channel.
fn played(input: RunInput, frames: usize) -> Vec<f32> {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let plan = plan(&registry);
    let (mut runner, _controller) =
        realtime(&plan, &registry, RealtimeConfig::new(48_000, 1)).unwrap();
    runner.set_inputs(vec![input]);
    let mut out = vec![0.0f32; 64 * 2];
    let mut left = Vec::new();
    while left.len() < frames {
        let guard = Guard::start();
        runner.process(&mut out, 2);
        assert!(guard.counts().is_zero());
        left.extend(out.as_chunks::<2>().0.iter().map(|f| f[0]));
    }
    left
}

/// ADR 0038 §10: a live input that stops delivering fades its last frame
/// to 0 over 64 frames and fades back in over 64 when it delivers again;
/// a short gap is a short dip; a render's input reads 0 at once.
#[test]
fn a_live_input_s_gaps_fade_out_and_back_in() {
    use signal_output::inputs::GAP_FADE;
    let flaky = |on, off| {
        let (mut input, _) = RunInput::new(
            "/mic".parse().unwrap(),
            2,
            Box::new(Flaky { on, off, at: 0 }),
            64,
            None,
        );
        input.fade_gaps();
        input
    };
    let left = played(flaky(512, 512), 2_048);
    assert!(left[..512].iter().all(|&x| x == 0.5));
    let gap = &left[512..512 + GAP_FADE];
    assert!(gap.windows(2).all(|w| w[1] <= w[0]), "{gap:?}");
    assert!(gap[0] > 0.49 && gap[GAP_FADE - 1] == 0.0, "{gap:?}");
    assert!(left[512 + GAP_FADE..1_024].iter().all(|&x| x == 0.0));
    let rise = &left[1_024..1_024 + GAP_FADE];
    assert!(rise.windows(2).all(|w| w[1] >= w[0]), "{rise:?}");
    assert_eq!(rise[0], 0.0);
    assert!(left[1_024 + GAP_FADE..1_536].iter().all(|&x| x == 0.5));
    // No step larger than the raised cosine's steepest (π/2 · 0.5 / 64).
    let steepest = std::f32::consts::FRAC_PI_2 * 0.5 / GAP_FADE as f32 + 1e-6;
    assert!(left.windows(2).all(|w| (w[1] - w[0]).abs() <= steepest));

    // A gap of 4 frames (a read 4 short) dips by little and comes back as
    // smoothly.
    let left = played(flaky(508, 4), 1_024);
    let low = left.iter().copied().fold(1.0f32, f32::min);
    assert!(low > 0.49, "{low}");
    assert!(left.windows(2).all(|w| (w[1] - w[0]).abs() <= steepest));

    // Without `fade_gaps` (a render's fixture, a replay): 0 at once.
    let (input, _) = RunInput::new(
        "/mic".parse().unwrap(),
        2,
        Box::new(Flaky {
            on: 512,
            off: 512,
            at: 0,
        }),
        64,
        None,
    );
    let left = played(input, 1_024);
    assert!(left[512..].iter().all(|&x| x == 0.0));
}
