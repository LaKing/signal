//! ADR 0052 §3 and verification 5 (the splices only): a repeat, a skip and
//! the fade on an empty ring each splatter at least 40 dB less than a hard
//! cut to silence at the same frame. Splatter: the energy above 2 kHz of a
//! 440 Hz sine's output, in a Hann-windowed 4096-frame window centred on
//! the splice. And the startup's fade-in starts with the signal.
//!
//! A sine is the aligned splices' best case (its material always
//! correlates); material that does not gets an equal-power crossfade over
//! the frames the old head has left, at least J.

use std::f64::consts::TAU;

use realfft::RealFftPlanner;
use signal_output::adapter::{AdapterConfig, AdapterOutput, AdapterStatus, clock_adapter};

const RATE: u32 = 48_000;
const PERIOD: usize = 128;
const HZ: f64 = 440.0;
const AMPLITUDE: f64 = 0.5;
const WINDOW: usize = 4096;

/// Energy above 2 kHz of `x` (WINDOW long) under a Hann window.
fn splatter(x: &[f64]) -> f64 {
    let n = x.len();
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n);
    let mut input: Vec<f64> = x
        .iter()
        .enumerate()
        .map(|(i, v)| v * (0.5 - 0.5 * (TAU * i as f64 / n as f64).cos()))
        .collect();
    let mut out = fft.make_output_vec();
    fft.process(&mut input, &mut out).unwrap();
    out.iter()
        .enumerate()
        .filter(|(b, _)| *b as f64 * f64::from(RATE) / n as f64 > 2000.0)
        .map(|(_, c)| c.norm_sqr())
        .sum()
}

/// A scripted run: per producer period, how many periods to push (0 = a
/// gap, 3 = catch up); the device pulls one period each time. Returns the
/// output and the frame at which `event` first counted up.
fn scripted(
    pushes: impl Fn(usize) -> usize,
    periods: usize,
    event: impl Fn(&AdapterStatus) -> u64,
) -> (Vec<f64>, Option<usize>) {
    let config = AdapterConfig::new(1, RATE, RATE, PERIOD, PERIOD);
    let (mut input, mut output): (_, AdapterOutput) = clock_adapter(config).unwrap();
    let mut produced = 0usize;
    let mut out = Vec::new();
    let mut buffer = vec![0.0f32; PERIOD];
    let mut at = None;
    let ns = |frames: usize| (frames as u64 * 1_000_000_000) / u64::from(RATE);
    for k in 0..periods {
        for _ in 0..pushes(k) {
            let block: Vec<f32> = (0..PERIOD)
                .map(|i| {
                    (AMPLITUDE * (TAU * HZ * (produced + i) as f64 / f64::from(RATE)).sin()) as f32
                })
                .collect();
            // Stamped at its own time on the producer's clock.
            input.push(&block, Some(ns(produced)));
            produced += PERIOD;
        }
        let before = event(&output.status());
        output.pull(&mut buffer, Some(ns(k * PERIOD)));
        if at.is_none() && event(&output.status()) > before {
            at = Some(out.len());
        }
        out.extend(buffer.iter().map(|&y| f64::from(y)));
    }
    (out, at)
}

/// The splatter of `out` around `at` against a hard cut to silence there
/// (the sine fitted to the frames before it), in dB.
fn against_a_hard_cut(out: &[f64], at: usize) -> f64 {
    let start = at - WINDOW / 2;
    let window = &out[start..start + WINDOW];
    // The hard cut: the output before the splice, then silence.
    let cut: Vec<f64> = (0..WINDOW)
        .map(|i| if i < WINDOW / 2 { window[i] } else { 0.0 })
        .collect();
    10.0 * (splatter(window) / splatter(&cut)).log10()
}

#[test]
fn a_repeat_splatters_40_db_less_than_a_hard_cut() {
    // A gap of two periods, then the producer catches up.
    let (out, at) = scripted(
        |k| match k {
            600..=601 => 0,
            602 => 3,
            _ => 1,
        },
        1000,
        |s| s.repeats,
    );
    let at = at.expect("a repeat");
    let db = against_a_hard_cut(&out, at);
    println!("repeat: {db:.1} dB against a hard cut");
    assert!(db <= -40.0, "{db} dB");
}

#[test]
fn an_empty_ring_fades_out_40_db_below_a_hard_cut() {
    let (out, at) = scripted(|k| usize::from(k < 600), 1000, |s| s.stops);
    let at = at.expect("a stop");
    let db = against_a_hard_cut(&out, at);
    println!("stop: {db:.1} dB against a hard cut");
    assert!(db <= -40.0, "{db} dB");
    assert!(
        out[at + 300..].iter().all(|&y| y == 0.0),
        "silence after the fade"
    );
}

#[test]
fn a_skip_splatters_40_db_less_than_a_hard_cut() {
    // A burst of 12 extra periods: beyond 3 × F*.
    let (out, at) = scripted(|k| if k == 600 { 13 } else { 1 }, 1000, |s| s.skips);
    let at = at.expect("a skip");
    let db = against_a_hard_cut(&out, at);
    println!("skip: {db:.1} dB against a hard cut");
    assert!(db <= -40.0, "{db} dB");
}

/// The fade-in starts with the signal: no step at the start, whatever the
/// start's offset (a 512-frame producer moves the start back by up to half
/// its period).
#[test]
fn the_fade_in_starts_with_the_signal() {
    for (source_period, dest_period) in [(512, 64), (1024, 128), (128, 128)] {
        let config = AdapterConfig::new(1, RATE, RATE, source_period, dest_period);
        let (mut input, mut output) = clock_adapter(config).unwrap();
        let block = vec![0.5f32; source_period];
        let mut out = Vec::new();
        let mut buffer = vec![0.0f32; dest_period];
        let ns = |frames: usize| (frames as u64 * 1_000_000_000) / u64::from(RATE);
        let mut pushed = 0;
        for k in 0..400 {
            let t = k * dest_period;
            while pushed <= t + dest_period {
                input.push(&block, Some(ns(pushed)));
                pushed += source_period;
            }
            output.pull(&mut buffer, Some(ns(t)));
            out.extend_from_slice(&buffer);
        }
        let (at, step) = out
            .windows(2)
            .enumerate()
            .map(|(i, w)| (i, (w[1] - w[0]).abs()))
            .fold((0, 0.0f32), |best, x| if x.1 > best.1 { x } else { best });
        let first = out.iter().position(|&y| y != 0.0).unwrap_or(0);
        // A 10 ms raised cosine on 0.5 steps by at most 0.5·π/2/480.
        assert!(
            step < 0.003,
            "{source_period}/{dest_period}: a step of {step} at {at} (signal from {first}): {:?} {:?}",
            &out[at.saturating_sub(3)..at + 3],
            output.status()
        );
        assert!((out[out.len() - 1] - 0.5).abs() < 1e-4);
    }
}
