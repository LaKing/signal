//! ADR 0052 §4 and verification 6: the clock adapter's two callback paths,
//! [`AdapterInput::push`] and [`AdapterOutput::pull`], allocate and free
//! nothing, measured with the counting allocator of `signal-contract`
//! (ADR 0017) around every call, through every path: the startup fill and
//! fade-in, steady conversion, repeats (underruns), skips (overruns), the
//! fade to silence on an empty ring and the fill after it, a full ring
//! dropping frames, pulls longer than a piece, and stereo. And no lock:
//! the adapter's sources name no blocking primitive.

signal_contract::install_counting_allocator!();

use std::thread;

use signal_contract::alloc_counter::{self, Counts, Guard};
use signal_output::adapter::{AdapterConfig, AdapterInput, AdapterOutput, clock_adapter};

fn add(total: &mut Counts, counts: Counts) {
    total.allocations += counts.allocations;
    total.deallocations += counts.deallocations;
}

fn push(input: &mut AdapterInput, frames: &[f32], time: Option<u64>, total: &mut Counts) {
    let guard = Guard::start();
    input.push(frames, time);
    add(total, guard.counts());
}

fn pull(output: &mut AdapterOutput, out: &mut [f32], time: Option<u64>, total: &mut Counts) {
    let guard = Guard::start();
    output.pull(out, time);
    add(total, guard.counts());
}

#[test]
fn push_and_pull_allocate_nothing_on_any_path() {
    assert!(alloc_counter::is_installed());
    for (source_rate, dest_rate) in [(48_000, 48_000), (48_000, 44_100), (44_100, 48_000)] {
        let channels = 2;
        // The source period that matches a 128-frame pull at these rates,
        // so one push per pull is the steady state.
        let period = (128 * source_rate as usize).div_ceil(dest_rate as usize);
        let config = AdapterConfig::new(channels, source_rate, dest_rate, period, 128);
        let (mut input, mut output) = clock_adapter(config).unwrap();
        let mut total = Counts::default();
        let block: Vec<f32> = (0..period * channels)
            .map(|n| ((n as f32) * 0.01).sin() * 0.5)
            .collect();
        let mut out = vec![0.0f32; 1024 * channels];
        let period_ns = |frames: usize, rate: u32| frames as u64 * 1_000_000_000 / u64::from(rate);
        let mut t_in = 0u64;
        let mut t_out = 0u64;
        let mut step = |input: &mut AdapterInput,
                        output: &mut AdapterOutput,
                        pushes: usize,
                        pull_frames: usize,
                        stamps: bool,
                        total: &mut Counts| {
            for _ in 0..pushes {
                push(input, &block, stamps.then_some(t_in), total);
                t_in += period_ns(period, source_rate);
            }
            pull(
                output,
                &mut out[..pull_frames * channels],
                stamps.then_some(t_out),
                total,
            );
            t_out += period_ns(pull_frames, dest_rate);
        };
        // Startup fill and steady conversion, with and without stamps.
        for i in 0..2000 {
            step(&mut input, &mut output, 1, 128, i % 500 < 400, &mut total);
        }
        // Underruns: the producer delivers a period every other pull.
        for i in 0..200 {
            step(&mut input, &mut output, i % 2, 128, true, &mut total);
        }
        // Overrun: a burst far beyond 3 × F*, the ring filling up (drops).
        step(&mut input, &mut output, 40, 0, true, &mut total);
        for _ in 0..200 {
            step(&mut input, &mut output, 1, 128, true, &mut total);
        }
        // An empty ring: the producer stops, then starts again.
        for _ in 0..50 {
            step(&mut input, &mut output, 0, 128, true, &mut total);
        }
        for _ in 0..400 {
            step(&mut input, &mut output, 1, 128, true, &mut total);
        }
        // Pulls longer than a piece (max_pull = 512).
        for _ in 0..100 {
            step(&mut input, &mut output, 8, 1024, true, &mut total);
        }
        let status = output.status();
        assert_eq!(
            total,
            Counts::default(),
            "{source_rate}→{dest_rate}: heap calls in push or pull"
        );
        assert!(status.repeats > 0, "{status:?}");
        assert!(status.skips > 0, "{status:?}");
        assert!(status.stops > 0 && status.starts > 1, "{status:?}");
        assert!(status.linearised > 0 && status.raw > 0, "{status:?}");
        assert!(input.dropped_frames() > 0, "the ring filled up");
        assert!(out.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
    }
}

/// The two ends on two threads, as in a route: nothing allocates on
/// either.
#[test]
fn two_threads_allocate_nothing() {
    let config = AdapterConfig::new(1, 48_000, 48_000, 64, 256);
    let (mut input, mut output) = clock_adapter(config).unwrap();
    let producer = thread::spawn(move || {
        let block = vec![0.25f32; 64];
        let mut total = Counts::default();
        for n in 0..20_000u64 {
            push(&mut input, &block, Some(n * 1_333_333), &mut total);
            if n % 4 == 0 {
                thread::yield_now();
            }
        }
        total
    });
    let consumer = thread::spawn(move || {
        let mut out = vec![0.0f32; 256];
        let mut total = Counts::default();
        for n in 0..5_000u64 {
            pull(&mut output, &mut out, Some(n * 5_333_333), &mut total);
            thread::yield_now();
        }
        total
    });
    assert_eq!(producer.join().unwrap(), Counts::default());
    assert_eq!(consumer.join().unwrap(), Counts::default());
}

/// No blocking primitive in the adapter's code lines (comments skipped).
#[test]
fn the_adapter_takes_no_lock() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/adapter");
    let mut files = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        files += 1;
        for (n, line) in text.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            for word in [
                "Mutex",
                "RwLock",
                "Condvar",
                "Barrier",
                "mpsc",
                "park(",
                "sleep(",
                "println!",
                "eprintln!",
                "Instant",
                "SystemTime",
            ] {
                assert!(
                    !code.contains(word),
                    "{}:{}: `{word}`",
                    path.display(),
                    n + 1
                );
            }
        }
    }
    assert!(files >= 4);
}
