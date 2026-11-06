//! ADR 0052 §6 and verification 5 (the clock faults): a clock adapter in
//! virtual time, the producer at 48 kHz in 128-frame pushes, the device
//! pulling 128 frames at 48 kHz × (1 + ε).
//!
//! - A device that stalls for 50 ms (a time skip) resyncs and locks again.
//! - An ε step to 2000 ppm, beyond the loop's ±1000 ppm, resyncs (a slip
//!   while tracking), then becomes clock suspect (three slips within 10 s)
//!   and is faded out; back at 50 ppm it locks and plays again.
//! - A steady 100 ppm device never resyncs.

use std::f64::consts::TAU;

use signal_output::adapter::{AdapterConfig, AdapterState, AdapterStatus, clock_adapter};

const RATE: u32 = 48_000;
const PERIOD: usize = 128;

/// What one run saw.
struct Faults {
    /// The status at the end.
    end: AdapterStatus,
    /// (device time, state, the pull's peak) per pull.
    pulls: Vec<(f64, AdapterState, f32)>,
}

/// Runs `seconds` of virtual time; `epsilon(t)` is the device's offset at
/// device time `t`, and `stall` a (time, seconds) when its callbacks stop.
fn run(seconds: f64, epsilon: impl Fn(f64) -> f64, stall: Option<(f64, f64)>) -> Faults {
    let config = AdapterConfig::new(1, RATE, RATE, PERIOD, PERIOD);
    let (mut input, mut output) = clock_adapter(config).unwrap();
    let period = PERIOD as f64 / f64::from(RATE);
    let (mut next_push, mut next_pull) = (0.0f64, 0.003f64);
    let mut produced = 0usize;
    let mut stalled = false;
    let mut block = vec![0.0f32; PERIOD];
    let mut out = vec![0.0f32; PERIOD];
    let mut pulls = Vec::new();
    while next_pull < seconds {
        if next_push <= next_pull {
            for (i, x) in block.iter_mut().enumerate() {
                *x = (0.5 * (TAU * 440.0 * (produced + i) as f64 / f64::from(RATE)).sin()) as f32;
            }
            input.push(&block, Some((next_push * 1e9) as u64));
            produced += PERIOD;
            next_push += period;
        } else {
            if let Some((at, len)) = stall
                && !stalled
                && next_pull >= at
            {
                stalled = true;
                next_pull += len;
                continue;
            }
            output.pull(&mut out, Some((next_pull * 1e9) as u64));
            let peak = out.iter().fold(0.0f32, |p, x| p.max(x.abs()));
            pulls.push((next_pull, output.status().state, peak));
            next_pull += period / (1.0 + epsilon(next_pull) * 1e-6);
        }
    }
    Faults {
        end: output.status(),
        pulls,
    }
}

/// The state at the last pull before `t`.
fn state_at(f: &Faults, t: f64) -> AdapterState {
    f.pulls
        .iter()
        .take_while(|p| p.0 < t)
        .last()
        .map(|p| p.1)
        .unwrap()
}

/// The largest peak of the pulls in [a, b).
fn peak_between(f: &Faults, a: f64, b: f64) -> f32 {
    f.pulls
        .iter()
        .filter(|p| p.0 >= a && p.0 < b)
        .fold(0.0, |m, p| m.max(p.2))
}

#[test]
fn a_device_that_stalls_50_ms_resyncs_and_locks_again() {
    let f = run(30.0, |_| 50.0, Some((10.0, 0.050)));
    assert_eq!(state_at(&f, 9.9), AdapterState::Locked);
    assert!(f.end.resyncs >= 1, "{:?}", f.end);
    assert_eq!(f.end.suspects, 0, "{:?}", f.end);
    assert_eq!(f.end.state, AdapterState::Locked, "{:?}", f.end);
    assert!(peak_between(&f, 25.0, 30.0) > 0.4);
}

#[test]
fn a_clock_beyond_the_clamp_becomes_suspect_and_comes_back() {
    let epsilon = |t: f64| {
        if (10.0..40.0).contains(&t) {
            2000.0
        } else {
            50.0
        }
    };
    let f = run(70.0, epsilon, None);
    assert_eq!(state_at(&f, 9.9), AdapterState::Locked);
    // Resyncs while at the clamp, then suspect and silent.
    let suspect_at = f
        .pulls
        .iter()
        .find(|p| p.1 == AdapterState::Suspect)
        .map(|p| p.0)
        .expect("becomes suspect");
    // Three slips within 10 s (the clamp is reached only now and then).
    assert!(
        (12.0..25.0).contains(&suspect_at),
        "suspect at {suspect_at:.1} s"
    );
    assert!(f.end.resyncs >= 1, "{:?}", f.end);
    assert_eq!(f.end.suspects, 1, "{:?}", f.end);
    assert!(peak_between(&f, suspect_at + 0.1, 40.0) < 1e-6, "faded out");
    // Back at 50 ppm: locked and heard again.
    assert_eq!(f.end.state, AdapterState::Locked, "{:?}", f.end);
    assert!(peak_between(&f, 65.0, 70.0) > 0.4);
}

#[test]
fn a_steady_device_never_resyncs() {
    let f = run(60.0, |_| 100.0, None);
    assert_eq!(f.end.state, AdapterState::Locked);
    assert_eq!(f.end.resyncs, 0, "{:?}", f.end);
    assert_eq!(f.end.slips, 0, "{:?}", f.end);
}
