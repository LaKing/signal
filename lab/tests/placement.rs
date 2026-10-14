//! M2.7-2 with 3 noop devices and 4 banks (placed noop-0, noop-1, noop-2,
//! noop-0): renders are bit-identical across runs, *k* and thread counts,
//! and so is the placement (AC1b; noop devices compute 0, so the audio
//! part proves the job path and the placement, not GPU arithmetic); the
//! manifest records the device set and the placement (AC7b); a device lost
//! mid-run faults its own bank only, logged, and never blocks (AC1c); and
//! with replacement the bank delivers again on the next generation (AC6).

#![cfg(feature = "lab")]

mod common;

use std::time::{Duration, Instant};

use signal_engine::{ComputeDevice, Engine, EngineConfig, Placement, Schedule};
use signal_gpu::GpuHost;
use signal_lab::gpu_partials::BLOCK_FRAMES;
use signal_output::{Render, render, render_with};

const BANKS: usize = 4;

/// What the manifest records: the pool's devices and the host's placement.
fn record(gpu: &common::Gpu, host: &GpuHost) -> (Vec<ComputeDevice>, Vec<Placement>) {
    let devices = gpu
        .pool
        .devices()
        .iter()
        .map(|d| ComputeDevice {
            id: d.id.clone(),
            name: d.name.clone(),
            backend: d.backend.clone(),
        })
        .collect();
    let placement = host
        .placement()
        .into_iter()
        .map(|p| Placement {
            instance: p.instance,
            device: p.device,
            pinned: p.pinned,
        })
        .collect();
    (devices, placement)
}

/// A fresh pool of 3 noop devices and a render of the 4 banks on it.
fn render_banks(threads: usize, k: usize, frames: u64) -> Render {
    let gpu = common::noop(3);
    let (registry, host) = gpu.registry_and_host();
    let config = EngineConfig {
        threads,
        block_capacity: k,
        schedule: Schedule::FORCED,
        ..EngineConfig::new(96_000, 42)
    };
    let mut engine = Engine::new(
        &common::banks(&registry, BANKS),
        &registry,
        config,
        Vec::new(),
    )
    .unwrap();
    let mut r = render(&mut engine, frames).unwrap();
    (r.compute_devices, r.placement) = record(&gpu, &host);
    drop(engine);
    r
}

#[test]
fn renders_and_placement_are_identical_across_runs_k_and_threads() {
    let frames = 20 * BLOCK_FRAMES as u64;
    let first = render_banks(1, 256, frames);
    let placed: Vec<(&str, Option<&str>)> = first
        .placement
        .iter()
        .map(|p| (p.instance.as_str(), p.device.as_deref()))
        .collect();
    assert_eq!(
        placed,
        [
            ("/bank0", Some("noop-0")),
            ("/bank1", Some("noop-1")),
            ("/bank2", Some("noop-2")),
            ("/bank3", Some("noop-0")),
        ]
    );
    let bits = |r: &Render| r.samples.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    for (threads, k) in [(1, 256), (1, 1), (1, 64), (3, 256), (4, 64), (4, 1)] {
        let again = render_banks(threads, k, frames);
        let at = format!("{threads} threads, k = {k}");
        // The informative part (auditor-2-008): the same devices and placement.
        assert_eq!(again.compute_devices, first.compute_devices, "{at}");
        assert_eq!(again.placement, first.placement, "{at}");
        // AC1b as written: bit-identical audio and log.
        assert!(bits(&again) == bits(&first), "{at}");
        assert_eq!(again.log, first.log, "{at}");
    }
    assert!(!String::from_utf8_lossy(&first.log).contains("late_result"));
    // AC7b: the manifest records the device set and the placement.
    let manifest = first.manifest(Default::default());
    let ids: Vec<&str> = manifest
        .compute_devices
        .iter()
        .map(|d| d.id.as_str())
        .collect();
    assert_eq!(ids, ["noop-0", "noop-1", "noop-2"]);
    assert_eq!(manifest.placement, first.placement);
}

/// The `late_result` faults of `log`: (frame, address, cause).
fn late(log: &[u8]) -> Vec<(u64, String, String)> {
    let field = |line: &str, key: &str| {
        line.split(&format!("\"{key}\":"))
            .nth(1)
            .and_then(|s| s.split([',', '}']).next())
            .map(|s| s.trim_matches('"').to_owned())
    };
    String::from_utf8_lossy(log)
        .lines()
        .filter(|l| l.contains("\"reason\":\"late_result\""))
        .map(|l| {
            (
                field(l, "frame").unwrap().parse().unwrap(),
                field(l, "address").unwrap(),
                field(l, "cause").unwrap_or_else(|| "none".to_owned()),
            )
        })
        .collect()
}

/// Renders the 4 banks offline, losing device 1 at block 10; with `replace`,
/// waits at block 12 for its next generation. The log's `late_result` faults.
fn lose_device_1(replace: bool) -> Vec<(u64, String, String)> {
    let gpu = common::noop(3);
    gpu.pool.hold_replacement(!replace);
    let (registry, _host) = gpu.registry_and_host();
    let mut engine = Engine::new(
        &common::banks(&registry, BANKS),
        &registry,
        EngineConfig::new(96_000, 42),
        Vec::new(),
    )
    .unwrap();
    let block = BLOCK_FRAMES as u64;
    let started = Instant::now();
    let r = render_with(&mut engine, 40 * block, |engine| {
        let frame = engine.next_frame().0;
        if frame == 10 * block {
            gpu.pool.lose_device(1);
        } else if replace && frame == 12 * block {
            while gpu.pool.status()[1].generation < 2 {
                assert!(started.elapsed() < Duration::from_secs(20), "not replaced");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        Ok(())
    })
    .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "the render never blocked"
    );
    let faults = late(&r.log);
    eprintln!("{faults:?}");
    assert_eq!(engine.late_results(), faults.len() as u64);
    faults
}

/// AC1c: device 1 is lost for good (replacement held): its bank's every due
/// block from the first fault on is a logged `late_result` with cause
/// `device_lost`, also after its job pool is spent (auditor-2-009); the
/// other banks, on the other devices, never miss; the render ends.
#[test]
fn a_lost_device_faults_its_own_instances_and_the_others_continue() {
    let faults = late_and_check(false);
    let block = BLOCK_FRAMES as u64;
    let first = faults[0].0;
    let due: Vec<u64> = (first / block..40).map(|b| b * block).collect();
    let frames: Vec<u64> = faults.iter().map(|f| f.0).collect();
    assert_eq!(
        frames, due,
        "every due block of /bank1 from the first fault on"
    );
}

/// AC6: with replacement the bank on the lost device misses only while its
/// device is lost and the blocks in flight then, each `device_lost`; then it
/// delivers again on the next generation, for more than twice its job pool
/// of blocks (no job leaked).
#[test]
fn a_replaced_device_delivers_again() {
    let faults = late_and_check(true);
    let block = BLOCK_FRAMES as u64;
    let last = faults.last().unwrap().0;
    assert!(
        last < 20 * block,
        "delivered again from block 20 on: {faults:?}"
    );
}

fn late_and_check(replace: bool) -> Vec<(u64, String, String)> {
    let faults = lose_device_1(replace);
    assert!(!faults.is_empty(), "the loss was a fault");
    assert!(
        faults.iter().all(|(_, address, _)| address == "/bank1"),
        "only the bank on device 1 missed: {faults:?}"
    );
    assert!(
        faults.iter().all(|(_, _, cause)| cause == "device_lost"),
        "every miss says why: {faults:?}"
    );
    assert!(faults[0].0 >= 10 * BLOCK_FRAMES as u64);
    faults
}
