//! ADR 0052 §3: a splice's cost is bounded whatever the channels and the
//! rate. Its alignment correlates a mix of at most `ALIGN_CHANNELS`
//! channels, coarse then fine within `ALIGN_COARSE_BUDGET`
//! (`AdapterOutput::align_multiply_adds`), so a repair at 192 kHz and 64
//! channels costs about what an ordinary crossfade does. The review of
//! 2026-11-06 measured a forced repair at 192 kHz, 128-frame callbacks and
//! 64 channels at 19.2 ms against a 0.667 ms deadline.
//!
//! The count is checked in the normal suite; the timing is a benchmark
//! (`docs/benchmarks.md` "Clock adapter"):
//!
//! ```sh
//! cargo test -p signal-output --release --offline --test adapter_repair_cost -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use signal_output::adapter::{
    AdapterConfig, AdapterOutput, AdapterStatus, FilterSpec, MAX_CHANNELS, clock_adapter,
};

const PERIOD: usize = 128;

/// The alignment's bound does not grow with the channels, and grows with
/// the rate only through the mixes and the fine search (linear in the
/// splice), never with its square.
#[test]
fn the_alignment_is_bounded_whatever_the_channels_and_the_rate() {
    let align = |channels: usize, rate: u32| {
        let config = AdapterConfig::new(channels, rate, rate, PERIOD, PERIOD);
        clock_adapter(config).unwrap().1.align_multiply_adds()
    };
    for rate in [44_100, 48_000, 96_000, 192_000] {
        assert_eq!(align(8, rate), align(16, rate), "{rate} Hz");
        assert_eq!(align(8, rate), align(MAX_CHANNELS, rate), "{rate} Hz");
    }
    let (low, high) = (align(2, 48_000), align(64, 192_000));
    // Four times the splice (960 frames, not 240) at most five times the
    // cost; the full search was sixteen times, and 32 times the channels.
    assert!(high <= 5 * low, "{low} at 48 kHz, {high} at 192 kHz");
    assert!(high <= 150_000, "{high}");
    // Less than a fifth of one 128-frame head's own work at 64 channels
    // (the full search was 118 M multiply-adds, 150 heads).
    let head = PERIOD * FilterSpec::for_rates(192_000, 192_000).taps * 65;
    assert!(high * 5 < head, "alignment {high}, a head {head}");
}

/// A scripted run at `rate`, `channels` wide: per producer period how many
/// periods to push; the device pulls one period each time. Each pull's
/// time and the pull at which `event` first counted up.
fn scripted(
    rate: u32,
    channels: usize,
    periods: usize,
    pushes: impl Fn(usize) -> usize,
    event: impl Fn(&AdapterStatus) -> u64,
) -> (Vec<Duration>, Option<usize>) {
    let config = AdapterConfig::new(channels, rate, rate, PERIOD, PERIOD);
    let (mut input, mut output): (_, AdapterOutput) = clock_adapter(config).unwrap();
    let ns = |frames: usize| (frames as u64 * 1_000_000_000) / u64::from(rate);
    let mut produced = 0usize;
    let mut block = vec![0.0f32; PERIOD * channels];
    let mut buffer = vec![0.0f32; PERIOD * channels];
    let mut times = Vec::with_capacity(periods);
    let mut at = None;
    for k in 0..periods {
        for _ in 0..pushes(k) {
            for (i, frame) in block.chunks_exact_mut(channels).enumerate() {
                let n = (produced + i) as f32;
                for (c, x) in frame.iter_mut().enumerate() {
                    *x = (n * (0.013 + 0.0007 * c as f32)).sin() * 0.5;
                }
            }
            input.push(&block, Some(ns(produced)));
            produced += PERIOD;
        }
        let before = event(&output.status());
        let start = Instant::now();
        output.pull(&mut buffer, Some(ns(k * PERIOD)));
        times.push(start.elapsed());
        if at.is_none() && event(&output.status()) > before {
            at = Some(k);
        }
    }
    (times, at)
}

/// A repair: its name, the pushes that force it and its counter.
type Repair = (&'static str, fn(usize) -> usize, fn(&AdapterStatus) -> u64);

/// Forced repairs at 192 kHz, 128-frame callbacks (deadline 0.667 ms), 16
/// and 64 channels: the slowest pull around a repeat, a skip and a restart
/// against the run's median. A timing on the host it runs on.
#[test]
#[ignore = "a benchmark; run with --release"]
fn repair_cost_at_192_khz() {
    let rate = 192_000;
    let deadline = PERIOD as f64 / f64::from(rate) * 1e3;
    println!("192 kHz, {PERIOD}-frame callbacks: deadline {deadline:.3} ms");
    println!("| channels | repair | median pull (ms) | slowest pull from the repair on (ms) |");
    println!("|---:|---|---:|---:|");
    for channels in [2, 16, 64] {
        let runs: [Repair; 3] = [
            (
                "repeat",
                |k| match k {
                    3000..=3001 => 0,
                    3002 => 3,
                    _ => 1,
                },
                |s| s.repeats,
            ),
            ("skip", |k| if k == 3000 { 13 } else { 1 }, |s| s.skips),
            (
                "restart",
                |k| usize::from(!(3000..3100).contains(&k)),
                |s| s.stops,
            ),
        ];
        for (name, pushes, event) in runs {
            let (times, at) = scripted(rate, channels, 4000, pushes, event);
            let at = at.unwrap_or_else(|| panic!("{name}: no repair"));
            let mut sorted = times.clone();
            sorted.sort();
            let median = sorted[sorted.len() / 2].as_secs_f64() * 1e3;
            let slowest = times[at..]
                .iter()
                .max()
                .map_or(0.0, |t| t.as_secs_f64() * 1e3);
            println!("| {channels} | {name} | {median:.4} | {slowest:.4} |");
        }
    }
}
