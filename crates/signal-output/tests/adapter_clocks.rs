//! ADR 0052 verification 1, 3 and 4: the clock adapter against fake clocks
//! (`adapter_support::harness`).
//!
//! The normal suite runs a short version of each; the full runs are
//! `#[ignore]`d and print the tables in `docs/benchmarks.md` "Clock
//! adapter":
//!
//! ```sh
//! cargo test -p signal-output --release --offline --test adapter_clocks -- --ignored --nocapture
//! ```

mod adapter_support;

use adapter_support::harness::{Run, Scenario, run};

/// The ADR's period pairs (source/destination) and rates.
const PAIRS: [(usize, usize, u32, u32); 4] = [
    (128, 128, 48_000, 48_000),
    (128, 512, 48_000, 48_000),
    (64, 441, 48_000, 44_100),
    (512, 64, 48_000, 48_000),
];

/// The ADR's clock errors, ppm.
const EPSILONS: [f64; 7] = [0.0, 20.0, -20.0, 100.0, -100.0, 300.0, -300.0];

/// Verification 3's targets.
const LOCK_SECONDS: f64 = 5.0;
const MEAN_ERROR_FRAMES: f64 = 0.5;
const RATIO_JITTER_PPM: f64 = 0.5;

/// Track statistics from `settle` seconds after the switch to track.
fn track(r: &Run, settle: f64) -> (f64, f64) {
    let from = r.track_at.expect("reached track") + settle;
    let (mean_e, _, _) = r.track_stats(from);
    (mean_e, r.correction_jitter(from))
}

/// One scenario against verification 3: zero slips and stops, lock within
/// 5 s for |ε| ≤ 100 ppm, then mean |e| and the ratio's jitter in bound.
fn converges(s: &Scenario, settle: f64) -> (Option<f64>, f64, f64) {
    let r = run(s);
    let name = format!(
        "{}/{} {}→{} at {:+} ppm",
        s.source_period, s.dest_period, s.source_rate, s.dest_rate, s.epsilon_ppm
    );
    assert_eq!(r.status.slips, 0, "{name}: {:?}", r.status);
    assert_eq!(r.status.stops, 0, "{name}");
    assert_eq!(r.dropped_frames, 0, "{name}");
    assert!(r.track_at.is_some(), "{name}: never locked: {:?}", r.status);
    let lock = r.lock_seconds();
    if s.epsilon_ppm.abs() <= 100.0 {
        let lock = lock.unwrap();
        assert!(lock <= LOCK_SECONDS, "{name}: locked after {lock:.2} s");
    }
    let (mean_e, jitter) = track(&r, settle);
    assert!(mean_e <= MEAN_ERROR_FRAMES, "{name}: mean |e| {mean_e}");
    assert!(
        jitter <= RATIO_JITTER_PPM,
        "{name}: ratio jitter {jitter} ppm"
    );
    (lock, mean_e, jitter)
}

/// Verification 1 and 3, short: every period pair at 0, +100 and −300 ppm,
/// 12 s each, jitter ±0.5 device period.
#[test]
fn every_pair_locks_and_tracks() {
    for (sp, dp, sr, dr) in PAIRS {
        for epsilon in [0.0, 100.0, -300.0] {
            let mut s = Scenario::new(sr, dr, sp, dp);
            s.epsilon_ppm = epsilon;
            s.seconds = 12.0;
            converges(&s, 0.0);
        }
    }
}

/// Verification 1 and 3, full: every pair at every ε, 300 s each, track
/// statistics from 60 s after lock.
#[test]
#[ignore = "long: 28 runs of 300 simulated seconds; run with --release"]
fn every_pair_locks_and_tracks_full() {
    println!(
        "| periods | rates | ε (ppm) | lock (s) | mean abs e (frames) | ratio jitter (ppm rms) | slips |"
    );
    println!("|---|---|---:|---:|---:|---:|---:|");
    for (sp, dp, sr, dr) in PAIRS {
        for epsilon in EPSILONS {
            let mut s = Scenario::new(sr, dr, sp, dp);
            s.epsilon_ppm = epsilon;
            s.seconds = 300.0;
            let (lock, mean_e, jitter) = converges(&s, 60.0);
            println!(
                "| {sp}/{dp} | {sr}→{dr} | {epsilon:+} | {} | {mean_e:.4} | {jitter:.4} | 0 |",
                lock.map_or("–".into(), |l| format!("{l:.2}"))
            );
        }
    }
}

/// Verification 3: a 0 → 50 ppm ramp over 10 minutes without a slip,
/// short: the same ramp over 30 s (20 times steeper).
#[test]
fn a_ramp_without_a_slip() {
    let mut s = Scenario::new(48_000, 48_000, 128, 128);
    s.ramp_to_ppm = Some(50.0);
    s.ramp_seconds = 30.0;
    s.seconds = 40.0;
    let r = run(&s);
    assert_eq!(r.status.slips, 0, "{:?}", r.status);
    // A type-2 loop follows a ramp with a constant error, rate / Ki: at
    // 1.67 ppm/s in track about 5 frames.
    let peak = r
        .samples
        .iter()
        .filter(|x| x.t > 5.0)
        .map(|x| x.error.abs())
        .fold(0.0, f64::max);
    assert!(peak < 10.0, "the fill lags the ramp by {peak} frames");
}

/// Verification 3, full: 0 → 50 ppm over 10 minutes.
#[test]
#[ignore = "long: 11 simulated minutes; run with --release"]
fn a_ramp_without_a_slip_full() {
    for (sp, dp, sr, dr) in PAIRS {
        let mut s = Scenario::new(sr, dr, sp, dp);
        s.ramp_to_ppm = Some(50.0);
        s.ramp_seconds = 600.0;
        s.seconds = 660.0;
        let r = run(&s);
        assert_eq!(r.status.slips, 0, "{sp}/{dp}: {:?}", r.status);
        let (mean_e, rms, peak) = r.track_stats(10.0);
        println!(
            "ramp {sp}/{dp} {sr}→{dr}: slips 0, mean abs e {mean_e:.4} frames, correction − needed: rms {rms:.4} ppm, peak {peak:.4} ppm"
        );
    }
}

/// Verification 4 at a given length: ε = +100 ppm, jitter on, a marker
/// every `every` seconds; zero slips and every marker at the same delay
/// (±1 frame: a spread of at most 2).
fn no_drift_accumulates(seconds: f64, every: f64) -> Run {
    let mut s = Scenario::new(48_000, 48_000, 128, 128);
    s.epsilon_ppm = 100.0;
    s.seconds = seconds;
    s.marker_seconds = Some(every);
    s.record_from = seconds;
    let r = run(&s);
    assert_eq!(r.status.slips, 0, "{:?}", r.status);
    assert_eq!(r.status.stops, 0);
    let expected = (seconds / every) as usize - 1;
    assert!(
        r.marker_delays.len() >= expected,
        "{} markers of {expected}",
        r.marker_delays.len()
    );
    // The first marker may fall in the acquire phase.
    let spread = r.marker_spread(1);
    assert!(spread <= 2.0, "marker delays spread over {spread} frames");
    // And the delay is the reported latency (48 kHz both sides: frames
    // alike).
    for delay in r.marker_delays.iter().skip(1) {
        assert!(
            (delay - r.latency_frames).abs() <= 1.0,
            "{delay} against {}",
            r.latency_frames
        );
    }
    r
}

/// Verification 4, short: 2 minutes, a marker every 5 s.
#[test]
fn markers_keep_their_delay() {
    no_drift_accumulates(120.0, 5.0);
}

/// Verification 4: one hour (172.8 M frames), a marker every minute.
#[test]
#[ignore = "long: one simulated hour; run with --release"]
fn one_hour_without_drift() {
    let r = no_drift_accumulates(3600.0, 60.0);
    let delays = &r.marker_delays;
    let (min, max) = delays
        .iter()
        .skip(1)
        .fold((f64::MAX, f64::MIN), |(lo, hi), &d| (lo.min(d), hi.max(d)));
    println!(
        "one hour at +100 ppm: {} pulls, slips {}, {} markers, delay {min:.3}..{max:.3} frames (spread {:.4}), reported latency {:.1} source frames, final correction {:.3} ppm",
        r.pulls,
        r.status.slips,
        delays.len(),
        max - min,
        r.latency_frames,
        r.status.correction_ppm
    );
}

/// Prints one run's loop, every `EVERY` pulls (environment: `SR`, `DR`,
/// `SP`, `DP`, `EPS` in ppm, `JIT`, `SECS`, `NOISE` in µs, `TS` = 0 for the
/// raw fill).
#[test]
#[ignore = "a tool: prints a trace"]
fn trace() {
    let var = |name: &str, default: f64| -> f64 {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let mut s = Scenario::new(
        var("SR", 48_000.0) as u32,
        var("DR", 48_000.0) as u32,
        var("SP", 128.0) as usize,
        var("DP", 128.0) as usize,
    );
    s.epsilon_ppm = var("EPS", 0.0);
    s.jitter = var("JIT", 0.5);
    s.timestamps = var("TS", 1.0) > 0.0;
    s.stamp_noise_us = var("NOISE", 0.0);
    s.seconds = var("SECS", 20.0);
    let r = run(&s);
    println!("{:?}", r.status);
    for x in r.samples.iter().step_by(var("EVERY", 75.0) as usize) {
        println!(
            "{:8.2} s  e {:8.3}  correction {:9.3} ppm  needed {:9.3} ppm  locked {}",
            x.t, x.error, x.correction_ppm, x.epsilon_ppm, x.locked
        );
    }
}

/// The harness is deterministic: the same scenario gives the same run.
#[test]
fn the_harness_is_deterministic() {
    let mut s = Scenario::new(48_000, 44_100, 64, 441);
    s.epsilon_ppm = -100.0;
    s.seconds = 15.0;
    s.marker_seconds = Some(2.0);
    let a = run(&s);
    let b = run(&s);
    assert_eq!(a.marker_delays, b.marker_delays);
    assert_eq!(a.status, b.status);
    assert!(!a.marker_delays.is_empty());
}

/// The comparison behind phase 1's linearised fill: the raw fill (ADR 0052
/// §2 as written, τ = 0.5 s) against the linearised one, and the
/// linearised one under noise on the host time stamps; and the ADR's
/// 20 ppm/s slew limit in acquire. Prints the table; asserts nothing.
#[test]
#[ignore = "long: a comparison table; run with --release"]
fn fill_estimates_compared() {
    println!(
        "| periods | ε (ppm) | variant | lock (s) | mean abs e (frames) | ratio jitter (ppm rms) | slips |"
    );
    println!("|---|---:|---|---:|---:|---:|---:|");
    for (sp, dp, sr, dr) in PAIRS {
        for epsilon in [0.0, 100.0, 300.0] {
            for variant in [
                "linearised",
                "stamps ±50 µs",
                "stamps ±200 µs",
                "raw fill",
                "acquire slew 20 ppm/s",
            ] {
                let mut s = Scenario::new(sr, dr, sp, dp);
                s.epsilon_ppm = epsilon;
                s.seconds = 300.0;
                match variant {
                    "stamps ±50 µs" => s.stamp_noise_us = 50.0,
                    "stamps ±200 µs" => s.stamp_noise_us = 200.0,
                    "raw fill" => s.timestamps = false,
                    "acquire slew 20 ppm/s" => s.params.acquire_slew = 20e-6,
                    _ => {}
                }
                let r = run(&s);
                let (mean_e, jitter) = match r.track_at {
                    Some(t) => (r.track_stats(t + 60.0).0, r.correction_jitter(t + 60.0)),
                    None => (r.track_stats(150.0).0, r.correction_jitter(150.0)),
                };
                println!(
                    "| {sp}/{dp} | {epsilon:+} | {variant} | {} | {mean_e:.3} | {jitter:.3} | {} |",
                    r.lock_seconds()
                        .map_or("never".into(), |l| format!("{l:.2}")),
                    r.status.slips
                );
            }
        }
    }
}

/// Stress beyond the ADR's harness: callbacks that may run before their
/// stamp (the stamp at the centre of ±0.5 period), and a jittered producer
/// too. Prints; asserts nothing.
#[test]
#[ignore = "long: a stress table; run with --release"]
fn jitter_stress() {
    println!("| periods | ε (ppm) | variant | slips | final J (ms) | ratio jitter (ppm rms) |");
    println!("|---|---:|---|---:|---:|---:|");
    for (sp, dp, sr, dr) in PAIRS {
        for epsilon in [0.0, 100.0] {
            for variant in ["stamp at the centre", "producer jitter ±0.5 too"] {
                let mut s = Scenario::new(sr, dr, sp, dp);
                s.epsilon_ppm = epsilon;
                s.seconds = 300.0;
                match variant {
                    "stamp at the centre" => s.stamp_centre = true,
                    _ => s.source_jitter = 0.5,
                }
                let r = run(&s);
                println!(
                    "| {sp}/{dp} | {epsilon:+} | {variant} | {} | {:.0} | {:.3} |",
                    r.status.slips,
                    r.status.jitter_seconds * 1e3,
                    r.correction_jitter(150.0)
                );
            }
        }
    }
}
