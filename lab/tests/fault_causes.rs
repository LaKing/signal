//! SC7 of M2.7-5: a GPU job that fails, or a lost device, reaches the
//! existing `late_result` path (silence, a logged and counted fault, no
//! mute) with its cause on the log line: `validation`, `mapping` and
//! `device_lost` are told apart, and a channel whose job failed delivers
//! again. Offline, so the engine waits for every due result and each
//! injected failure lands on exactly one block. The lost device is not
//! replaced here (replacement held): `lab/tests/placement.rs` covers the
//! replacement. On the GPU, or where none is usable on the noop device
//! (wgpu's validation is the same).

#![cfg(feature = "lab")]

mod common;

use signal_engine::{Engine, EngineConfig};
use signal_gpu::Injected;
use signal_lab::gpu_partials::{BLOCK_FRAMES, JOBS};
use signal_output::render_with;

#[test]
fn a_failed_job_and_a_lost_device_are_faults_with_their_cause() {
    let gpu = common::device();
    gpu.pool.hold_replacement(true);
    let registry = gpu.registry();
    let mut engine = Engine::new(
        &common::plan(&registry),
        &registry,
        EngineConfig::new(96_000, 42),
        Vec::new(),
    )
    .unwrap();
    let block = BLOCK_FRAMES as u64;
    let (dispatch, map, lose, end) = (4 * block, 10 * block, 16 * block, 26 * block);
    let r = render_with(&mut engine, end, |engine| {
        let frame = engine.next_frame().0;
        if frame == dispatch {
            gpu.fail_next_job(Injected::Dispatch);
        } else if frame == map {
            gpu.fail_next_job(Injected::Map);
        } else if frame == lose {
            gpu.lose_device();
        }
        Ok(())
    })
    .unwrap();
    let log = String::from_utf8(r.log).unwrap();
    let late: Vec<(u64, &str)> = log
        .lines()
        .filter(|l| l.contains("\"reason\":\"late_result\""))
        .map(|l| {
            let frame = l
                .split("\"frame\":")
                .nth(1)
                .and_then(|s| s.split(',').next())
                .and_then(|s| s.parse().ok())
                .unwrap();
            let cause = l.split("\"cause\":").nth(1).unwrap_or("none");
            (frame, cause)
        })
        .collect();
    eprintln!("{late:?}");
    assert!(!log.contains("\"reason\":\"panic\""), "{log}");
    // Before the loss: one validation fault, one mapping fault, nothing
    // else; every other due block was delivered, so the channel recovered.
    let before: Vec<&(u64, &str)> = late.iter().filter(|(f, _)| *f < lose).collect();
    let causes: Vec<&str> = before.iter().map(|(_, c)| *c).collect();
    assert_eq!(causes, ["\"validation\"}", "\"mapping\"}"], "{late:?}");
    let (v, m) = (before[0].0, before[1].0);
    assert!(
        (dispatch..map).contains(&v) && (map..lose).contains(&m),
        "{late:?}"
    );
    // After the loss: from the first `device_lost` on, every due block is
    // one; only results already published before the loss may come first.
    let after: Vec<&(u64, &str)> = late.iter().filter(|(f, _)| *f >= lose).collect();
    let first = after.first().map(|(f, _)| *f).unwrap();
    assert!(first <= lose + JOBS as u64 * block, "{late:?}");
    let due: Vec<u64> = (first / block..end.div_ceil(block))
        .map(|b| b * block)
        .collect();
    let lost: Vec<u64> = after.iter().map(|(f, _)| *f).collect();
    assert_eq!(lost, due);
    assert!(
        after.iter().all(|(_, c)| *c == "\"device_lost\"}"),
        "{late:?}"
    );
    assert_eq!(engine.late_results(), late.len() as u64);
}
