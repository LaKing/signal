//! AC1 of M2.7-4 (audit finding 2) with a delayed GPU: the null driver plays
//! `lab.gpu_partials` while the service's test hook holds every result back
//! for `D`. A schedule of fast callbacks and pauses makes the live run miss a
//! stretch of results, exhausting the bank's job pool, and then deliver
//! again. The recorded session replays exactly: the output the device
//! received (what `--record-audio` writes) bit for bit, and the same
//! `late_result` faults, although the replay runs far faster than the live
//! run (`settle` keeps it from losing a submission that live made). The ideal
//! render awaits every result: no fault, another log hash, labelled `ideal`.
//! On the noop device (no usable GPU) every result is silence, so the audio
//! difference of the ideal render shows on a GPU only (AC4-gpu, the
//! operator). The test's own device pool (M2.7-2) lends the bank its device.

#![cfg(feature = "lab")]

mod common;

use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use signal_engine::RealtimeConfig;
use signal_lab::gpu_partials::{BLOCK_FRAMES, LATENCY_BLOCKS};
use signal_output::{Driver, NullDriver, realtime};
use signal_project::session::{
    BoundaryKind, BoundaryReplay, LiveRecorder, ReplayOptions, Session, SessionRecorder, replay,
    replay_with,
};
use signal_project::{Project, SourceFile, provenance};

const RATE: u32 = 48_000;
/// How long the hook holds each finished job.
const D: Duration = Duration::from_millis(20);

/// After which callbacks (one block of the bank each) the live run pauses
/// for 3·D: none for the first 7, which miss and exhaust the pool, then
/// after each of the next 6, which brings results back, then 3 more without.
const PAUSES: [bool; 16] = [
    false, false, false, false, false, false, false, true, true, true, true, true, true, false,
    false, false,
];

#[test]
fn a_delayed_gpu_run_replays_exactly_and_ideal_differs() {
    let gpu = common::device();
    gpu.set_completion_delay(D);
    let registry = gpu.registry();
    let project = Project::from_sources(vec![SourceFile {
        path: "project.toml".to_owned(),
        text: format!(
            "schema_version = 1\n\n[project]\nname = \"delayed\"\nsample_rate = {RATE}\nseed = 7\n\n\
             [instances]\nbank = {{ module = \"lab.gpu_partials\" }}\nout = {{ module = \"std.audio_out\" }}\n\n\
             [wiring]\nconnections = [\n  {{ from = \"bank.out\", to = \"out.l\" }},\n  {{ from = \"bank.out\", to = \"out.r\" }},\n]\n"
        ),
    }])
    .unwrap();
    let mut plan = project.plan(&registry).unwrap().plan;
    plan.version = 0;
    let config = RealtimeConfig::with_buffer(RATE, project.seed, BLOCK_FRAMES as u64);
    let (runner, mut controller) = realtime(&plan, &registry, config).unwrap();
    let provenance = provenance::collect(&provenance::source_dir(), &project.files);
    let recorder =
        SessionRecorder::new(&project, &registry, RATE, project.seed, provenance).unwrap();
    let mut live = LiveRecorder::new(recorder, registry.clone());
    let mut driver = NullDriver::new(RATE, 2, 3);
    driver.start(runner).unwrap();
    for pause in PAUSES {
        driver.run(BLOCK_FRAMES as u64, || {}).unwrap();
        controller.poll(&mut live);
        if pause {
            thread::sleep(3 * D);
        }
    }
    let runner = driver.stop().unwrap().unwrap();
    controller.poll(&mut live);
    live.finish(runner.next_frame()).unwrap();
    let played = driver.output().to_vec();
    let recorder = live.into_recorder();

    // The schedule mixed both outcomes: a stretch of at least 5 misses (the
    // pool exhausted), then deliveries.
    let boundaries = recorder.boundaries().unwrap();
    assert!(boundaries.complete, "{:?}", boundaries.reason);
    let outcomes: Vec<bool> = boundaries
        .records
        .iter()
        .map(|r| !matches!(r.outcome, BoundaryKind::Missed(_)))
        .collect();
    eprintln!(
        "live: {} due blocks from block {LATENCY_BLOCKS}, {} delivered, {} missed: {}",
        outcomes.len(),
        boundaries.delivered,
        boundaries.missed,
        outcomes
            .iter()
            .map(|&d| if d { 'D' } else { 'm' })
            .collect::<String>()
    );
    let first_delivery = outcomes.iter().position(|&d| d).expect("a delivery");
    assert!(first_delivery >= 5, "{outcomes:?}");
    assert!(boundaries.delivered > 0 && boundaries.missed > 0);

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("lab-replay");
    let _ = fs::remove_dir_all(&dir);
    recorder.write(&dir).unwrap();
    let session = Session::load(&dir, &registry).unwrap();

    let exact = replay(&session, &registry, None, 1).unwrap();
    assert_eq!(exact.boundaries, BoundaryReplay::Exact);
    let render = &exact.render;
    let stored: Vec<f32> = render
        .samples
        .iter()
        .map(|&x| render.conversion.sample(x))
        .collect();
    assert_eq!(
        stored, played,
        "the exact replay equals what the device received"
    );
    let log = String::from_utf8_lossy(&render.log);
    assert_eq!(log.matches("late_result").count() as u64, boundaries.missed);

    let ideal = replay_with(
        &session,
        &registry,
        ReplayOptions {
            frames: None,
            threads: 1,
            ideal: true,
            ..ReplayOptions::default()
        },
    )
    .unwrap();
    gpu.set_completion_delay(Duration::ZERO);
    assert!(!String::from_utf8_lossy(&ideal.render.log).contains("late_result"));
    assert_ne!(ideal.render.log_fnv1a64(), exact.render.log_fnv1a64());
    let manifest = ideal.render.manifest(Default::default());
    assert!(manifest.ideal && !manifest.device_conversion);
    if !gpu.is_noop() {
        // AC4-gpu: on a GPU the ideal render plays the results live missed.
        let ideal_samples: Vec<f32> = ideal.render.samples.iter().map(|&x| x as f32).collect();
        assert_ne!(ideal_samples, played);
    }
    eprintln!(
        "exact: log {:016x}, ideal: log {:016x}",
        exact.render.log_fnv1a64(),
        ideal.render.log_fnv1a64()
    );
}
