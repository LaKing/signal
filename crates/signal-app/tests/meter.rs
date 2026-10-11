//! M2.5-1 AC2: meter ballistics from per-block peak and sum of squares.

use signal_app::meter::{ChannelMeter, dbfs};
use signal_core::Frame;
use signal_engine::Meter;

const RATE: u32 = 48_000;

fn block(first: u64, frames: u64, peak: f32, sum_squares: f64) -> Meter {
    Meter {
        first_frame: Frame(first),
        frames,
        channel: 0,
        peak,
        sum_squares,
    }
}

#[test]
fn the_peak_is_held_for_one_and_a_half_seconds() {
    let mut meter = ChannelMeter::new(RATE);
    meter.push(&block(0, 128, 0.5, 0.0));
    let hold = (1.5 * f64::from(RATE)) as u64;
    // Quieter blocks up to exactly 1.5 s after the loud one ended.
    let mut at = 128;
    while at + 128 <= 128 + hold {
        meter.push(&block(at, 128, 0.1, 0.0));
        at += 128;
    }
    let r = meter.reading();
    assert!((r.held - dbfs(0.5)).abs() < 1e-9, "held: {}", r.held);
    assert!((r.peak - dbfs(0.1)).abs() < 1e-6);
    // The next block is past the hold: the held peak falls to it.
    meter.push(&block(at, 128, 0.1, 0.0));
    assert!((meter.reading().held - dbfs(0.1)).abs() < 1e-6);
    // A louder block is held at once.
    meter.push(&block(at + 128, 128, 0.8, 0.0));
    assert!((meter.reading().held - dbfs(0.8)).abs() < 1e-6);
}

#[test]
fn a_full_scale_sine_reads_minus_3_01_dbfs_rms() {
    let mut meter = ChannelMeter::new(RATE);
    let mut phase = 0u64;
    for b in 0..(RATE as u64 / 128) {
        let samples: Vec<f64> = (0..128)
            .map(|i| {
                (2.0 * std::f64::consts::PI * 1_000.0 * (phase + i) as f64 / f64::from(RATE)).sin()
            })
            .collect();
        phase += 128;
        let peak = samples.iter().fold(0.0f64, |m, x| m.max(x.abs())) as f32;
        let sum = samples.iter().map(|x| x * x).sum();
        meter.push(&block(b * 128, 128, peak, sum));
    }
    let r = meter.reading();
    assert!((r.rms - (-3.0103)).abs() < 0.01, "rms {}", r.rms);
    assert!(r.peak.abs() < 0.01, "peak {}", r.peak);
}

#[test]
fn the_clip_indicator_latches_until_cleared() {
    let mut meter = ChannelMeter::new(RATE);
    meter.push(&block(0, 64, 0.9, 1.0));
    assert!(!meter.reading().clip);
    meter.push(&block(64, 64, 1.0, 1.0));
    assert!(meter.reading().clip);
    for b in 2..1_000 {
        meter.push(&block(b * 64, 64, 0.1, 0.1));
    }
    assert!(meter.reading().clip, "latched long after");
    meter.clear_clip();
    assert!(!meter.reading().clip);
}

#[test]
fn silence_reads_minus_infinity() {
    let mut meter = ChannelMeter::new(RATE);
    let r = meter.reading();
    assert_eq!(
        (r.peak, r.held, r.rms),
        (f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY)
    );
    meter.push(&block(0, 128, 0.0, 0.0));
    let r = meter.reading();
    assert_eq!(
        (r.peak, r.held, r.rms),
        (f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY)
    );
    assert_eq!(dbfs(1.0), 0.0);
}
