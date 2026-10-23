//! The CoreAudio callback body without a device (M2-4 AC5a): `OutputCallback`
//! runs a real Runner of the kick project into device buffers larger than its
//! scratch, maps the sink to the device's channels, measures every callback,
//! and hands the Runner back on a stop request. No test here calls cpal.

use std::path::Path;
use std::sync::Arc;

use signal_audio::{CallbackStats, ChannelMap, OutputCallback};
use signal_core::Registry;
use signal_engine::{Engine, EngineConfig, RealtimeConfig};
use signal_output::{Runner, realtime, render};
use signal_project::Project;

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

/// The kick's Runner, and its first `frames` frames rendered offline as f32.
fn kick(frames: u64) -> (Runner, Vec<f32>, u32) {
    let registry = registry();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/kick");
    let project = Project::load(&dir).unwrap();
    let plan = project.plan(&registry).unwrap().plan;
    let (rate, seed) = (project.sample_rate, project.seed);
    let mut engine =
        Engine::new(&plan, &registry, EngineConfig::new(rate, seed), Vec::new()).unwrap();
    let reference = render(&mut engine, frames)
        .unwrap()
        .samples
        .iter()
        .map(|&x| x as f32)
        .collect();
    let (runner, _controller) =
        realtime(&plan, &registry, RealtimeConfig::new(rate, seed)).unwrap();
    (runner, reference, rate)
}

#[test]
fn device_buffers_larger_than_the_scratch_play_the_offline_render() {
    let (runner, reference, rate) = kick(3 * 1_000);
    let stats = Arc::new(CallbackStats::default());
    // 64-frame scratch, 1 000-frame device buffers, 4 device channels.
    let (mut callback, _control) =
        OutputCallback::new(runner, ChannelMap::new(2, 4), 64, rate, Arc::clone(&stats));
    let mut out = vec![9.0f32; 1_000 * 4];
    let mut played = Vec::new();
    for _ in 0..3 {
        callback.fill(&mut out);
        for frame in out.as_chunks::<4>().0 {
            played.extend_from_slice(&frame[..2]);
            assert_eq!(
                &frame[2..],
                &[0.0, 0.0],
                "device channels 3 and 4 are silent"
            );
        }
    }
    assert_eq!(played, reference);
    let s = stats.snapshot();
    assert_eq!((s.callbacks, s.frames), (3, 3_000));
    assert!(s.worst_callback_ratio > 0.0 && s.worst_callback_ratio.is_finite());
}

#[test]
fn a_stop_request_hands_the_runner_back_and_silence_follows() {
    let (runner, _, rate) = kick(0);
    let stats = Arc::new(CallbackStats::default());
    let (mut callback, mut control) =
        OutputCallback::new(runner, ChannelMap::new(2, 2), 128, rate, stats);
    let mut out = vec![0.0f32; 128 * 2];
    callback.fill(&mut out);
    assert!(control.take_runner().is_none(), "still playing");
    control.request_stop();
    out.fill(9.0);
    callback.fill(&mut out);
    assert!(out.iter().all(|&x| x == 0.0), "silence once stopped");
    let runner = control.take_runner().expect("the runner comes back");
    assert_eq!(
        runner.next_frame().0,
        128,
        "it ran one callback, then stopped"
    );
    out.fill(9.0);
    callback.fill(&mut out);
    assert!(out.iter().all(|&x| x == 0.0));
    assert!(control.take_runner().is_none());
}

/// ADR 0036 §4: with a fade-out, a stop request ramps the output down over
/// the fade (the Runner running on), hands the Runner back once it has
/// faded out, and silence follows; a Runner asked to fade in rises from
/// silence. The steps stay small: no click on either edge.
#[test]
fn a_fade_out_runs_before_the_runner_goes_back() {
    let (mut runner, reference, rate) = kick(4_800);
    let fade = signal_output::fade::fade_frames(rate);
    runner.fade_in(fade);
    let stats = Arc::new(CallbackStats::default());
    let (callback, mut control) =
        OutputCallback::new(runner, ChannelMap::new(2, 2), 128, rate, stats);
    let mut callback = callback.with_fade_out(fade);
    let mut played = Vec::new();
    let mut out = vec![0.0f32; 100 * 2];
    // 40 callbacks of 100 frames, the stop requested after 30.
    for n in 0..40 {
        if n == 30 {
            control.request_stop();
        }
        callback.fill(&mut out);
        played.extend_from_slice(&out);
    }
    let runner = control.take_runner().expect("the runner comes back");
    let fade = fade as usize;
    // Whole callbacks: the one the fade ends in is the last it runs.
    assert_eq!(
        runner.next_frame().0,
        3_000 + fade.div_ceil(100) as u64 * 100,
        "it ran on for the fade"
    );
    let left: Vec<f32> = played.chunks(2).map(|f| f[0]).collect();
    let wanted: Vec<f32> = reference.chunks(2).map(|f| f[0]).collect();
    // The rise: from 0, below the reference in magnitude, then exact.
    assert_eq!(left[0], 0.0);
    for i in 0..fade {
        assert!(left[i].abs() <= wanted[i].abs() + 1e-7, "frame {i}");
    }
    assert_eq!(&left[fade..3_000], &wanted[fade..3_000]);
    // The fall, then silence.
    for i in 3_000..3_000 + fade {
        assert!(left[i].abs() <= wanted[i].abs() + 1e-7, "frame {i}");
    }
    assert!(left[3_000 + fade..].iter().all(|&x| x == 0.0));
}
