//! `lab.gpu_partials` (M2.6-4): the GPU result against the CPU reference
//! (AC1a), the exact latency of *L* blocks (AC1b), and bit-identical offline
//! renders through the engine for any *k* and thread count (AC1c, AC7).
//! Gated on a usable GPU adapter (AC10).

#![cfg(feature = "lab")]

mod common;

use signal_core::{Due, Inputs, Outputs, PrepareContext, RngStream, StateOutputPrimitive};
use signal_engine::{Engine, EngineConfig, SameMachine, Schedule};
use signal_gpu::GpuService;
use signal_lab::gpu_partials::{
    BLOCK_FRAMES, Bank, GpuPartials, KERNEL, LATENCY_BLOCKS, Params, TOLERANCE, pack, reference,
};
use signal_output::{Render, render};

const L: usize = LATENCY_BLOCKS as usize;

/// One block of the module as the offline engine runs it: per frame, wait
/// for `due`, `emit`, then `absorb` with `params`.
fn run_block(module: &mut GpuPartials, params: Params) -> Vec<f64> {
    let values = [params.freq, params.stretch, params.tilt, params.gain];
    let mut block = Vec::with_capacity(BLOCK_FRAMES);
    for _ in 0..BLOCK_FRAMES {
        loop {
            match module.due() {
                Due::Ready => break,
                Due::Pending => std::thread::yield_now(),
                Due::Lost => panic!("a due result was lost"),
            }
        }
        let mut out = [f64::NAN];
        module.emit(&mut Outputs::new(1, &mut out, &mut []));
        module.absorb(&Inputs::new(1, &[], &values, &[], &[]));
        block.push(out[0]);
    }
    block
}

fn prepared(sample_rate: f64) -> GpuPartials {
    let mut module = GpuPartials::default();
    module.prepare(&PrepareContext {
        sample_rate,
        max_frames: 256,
        rng: RngStream::derive(1, &"/bank".parse().unwrap()),
    });
    module
}

/// Parameters that change every block: frequencies up to Nyquist and past
/// it, inharmonicity, every tilt, and gain.
fn params_of(m: usize) -> Params {
    let x = m as f64;
    Params {
        freq: [27.5, 55.0, 110.0, 440.0, 1_000.0, 3_000.0, 19_000.0][m % 7] * (1.0 + 0.01 * x),
        stretch: [0.0, 0.0001, 0.001, 0.02, 1.0][m % 5],
        tilt: (x * 0.37) % 4.0,
        gain: [1.0, 0.25, 0.5][m % 3],
    }
}

/// AC1a and AC1b: block m of the output is the CPU reference of block m − L
/// within the tolerance (relative to Σ|a_i|), and exactly 0 before.
#[test]
fn gpu_matches_the_cpu_reference_l_blocks_late() {
    if common::gpu().is_none() {
        return;
    }
    for sample_rate in [48_000.0, 96_000.0] {
        let mut module = prepared(sample_rate);
        let mut bank = Bank::new(sample_rate);
        let mut expected = Vec::new();
        let mut worst: f64 = 0.0;
        for m in 0..24 {
            let out = run_block(&mut module, params_of(m));
            if m < L {
                assert!(out.iter().all(|&y| y == 0.0), "block {m} before L");
            } else {
                let (want, scale): (Vec<f64>, f64) = expected.remove(0);
                for (t, (y, w)) in out.iter().zip(&want).enumerate() {
                    let error = (y - w).abs() / scale;
                    worst = worst.max(error);
                    assert!(
                        error <= TOLERANCE,
                        "{sample_rate} Hz, block {m}, frame {t}: GPU {y} vs CPU {w} (Σ|a| = {scale})"
                    );
                }
            }
            let block = bank.next_block(params_of(m));
            let mut want = vec![0.0; BLOCK_FRAMES];
            reference(block, &mut want);
            expected.push((want, block.full_scale()));
        }
        // Measurement, for the report: the largest relative error seen.
        eprintln!("{sample_rate} Hz: worst |GPU − CPU| / Σ|a| = {worst:.3e}");
    }
}

/// AC1b: a parameter change reaches the output exactly L blocks after the
/// block whose first frame saw it, whatever frame of a block it comes on.
#[test]
fn a_change_reaches_the_output_exactly_l_blocks_later() {
    if common::gpu().is_none() {
        return;
    }
    let silent = Params {
        gain: 0.0,
        ..Params::default()
    };
    let loud = Params {
        gain: 0.5,
        ..Params::default()
    };
    let mut module = prepared(96_000.0);
    let mut blocks = Vec::new();
    // Gain 0 for blocks 0–3; on from the middle of block 4 (read at the
    // first frame of block 5); so block 5 + L is the first one audible.
    for _ in 0..5 {
        blocks.push(run_block(&mut module, silent));
    }
    let values = |p: Params| [p.freq, p.stretch, p.tilt, p.gain];
    let mut mid = Vec::new();
    for t in 0..BLOCK_FRAMES {
        while module.due() != Due::Ready {
            std::thread::yield_now();
        }
        let mut out = [f64::NAN];
        module.emit(&mut Outputs::new(1, &mut out, &mut []));
        let p = if t >= 100 { loud } else { silent };
        module.absorb(&Inputs::new(1, &[], &values(p), &[], &[]));
        mid.push(out[0]);
    }
    blocks.push(mid);
    for _ in 0..4 {
        blocks.push(run_block(&mut module, loud));
    }
    let first_audible = blocks
        .iter()
        .position(|b| b.iter().any(|&y| y != 0.0))
        .unwrap();
    assert_eq!(first_audible, 6 + L, "blocks 0–5 are silent at their start");
    // `due` gave each block its own result: the shift is exact for every block.
    assert!(blocks[6 + L].iter().filter(|&&y| y != 0.0).count() > BLOCK_FRAMES / 2);
}

fn render_with(threads: usize, block_capacity: usize, frames: u64) -> Render {
    let registry = common::registry();
    let config = EngineConfig {
        threads,
        block_capacity,
        schedule: Schedule::FORCED,
        ..EngineConfig::new(96_000, 42)
    };
    let mut engine = Engine::new(&common::plan(&registry), &registry, config, Vec::new()).unwrap();
    render(&mut engine, frames).unwrap()
}

/// AC1c and AC7: offline renders wait for every due result, so they are the
/// same bit for bit on repeat and for every *k* and thread count; nothing is
/// late, and the manifest records the `same_machine` instance.
#[test]
fn offline_renders_are_bit_identical_and_record_same_machine() {
    if common::gpu().is_none() {
        return;
    }
    let frames = 48_000;
    let first = render_with(1, 256, frames);
    let bits = |r: &Render| r.samples.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    for (threads, k) in [(1, 256), (1, 1), (1, 64), (4, 256), (3, 64)] {
        let again = render_with(threads, k, frames);
        assert!(bits(&again) == bits(&first), "{threads} threads, k = {k}");
        assert_eq!(again.log, first.log, "{threads} threads, k = {k}");
    }
    let log = String::from_utf8(first.log.clone()).unwrap();
    assert!(
        !log.contains("late_result"),
        "offline never gives up: {log}"
    );
    let silent = 2 * L * BLOCK_FRAMES; // L blocks, two channels interleaved
    assert!(first.samples[..silent].iter().all(|&x| x == 0.0));
    assert!(first.samples[silent..].iter().any(|&x| x != 0.0));
    assert_eq!(
        first.same_machine,
        [SameMachine {
            instance: "/bank".to_owned(),
            type_name: "lab.gpu_partials".to_owned(),
            latency_blocks: Some(LATENCY_BLOCKS),
            block_frames: Some(BLOCK_FRAMES as u32),
        }]
    );
    assert_eq!(
        first.manifest(Default::default()).same_machine,
        first.same_machine
    );
}

/// The kernel validates and one job of it travels the whole path: on the
/// GPU, or else on a private noop device (wgpu validates the WGSL with naga
/// before any backend sees it; the noop device then computes nothing).
#[test]
fn the_kernel_validates_and_a_job_comes_back() {
    let own;
    let service = match common::gpu() {
        Some(service) => service,
        None => {
            own = GpuService::start_noop().unwrap();
            &own
        }
    };
    let mut channel = service.channel(KERNEL, 1).unwrap();
    let mut bank = Bank::new(96_000.0);
    let block = bank.next_block(Params::default());
    assert!(channel.submit(0, |input| pack(block, input)));
    let started = std::time::Instant::now();
    while channel.peek_done().is_none() {
        assert!(started.elapsed().as_secs() < 10, "no result");
        std::thread::yield_now();
    }
    let job = channel.take_done().unwrap();
    assert!(!job.failed());
    assert_eq!(job.output().len(), BLOCK_FRAMES);
}

/// AC10: without a usable GPU the module has no channel; offline, every due
/// result is lost (the engine does not wait for it): a logged `late_result`
/// fault per block from block L on, and silence. Skipped where a GPU is.
#[test]
fn without_a_gpu_every_due_result_is_lost_and_logged() {
    if GpuService::shared().is_some() {
        eprintln!("skipped: a GPU is usable here");
        return;
    }
    let frames = 4 * BLOCK_FRAMES as u64 + 10;
    let r = render_with(1, 256, frames);
    assert!(r.samples.iter().all(|&x| x == 0.0));
    let log = String::from_utf8(r.log).unwrap();
    let late: Vec<&str> = log.lines().filter(|l| l.contains("late_result")).collect();
    assert_eq!(late.len(), 3, "blocks 2, 3 and 4: {log}");
    assert!(late[0].contains("\"frame\":512"), "{}", late[0]);
}
