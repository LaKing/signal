//! ADR 0052 verification 2: the converter's quality, by FFT and sine fits
//! (`adapter_support::spectrum`), against §4's targets:
//!
//! - passband ripple ≤ ±0.01 dB to 20 kHz;
//! - alias and image rejection ≥ 100 dB beyond the lower Nyquist frequency;
//! - THD+N of a 1 kHz sine at −1 dBFS ≤ −100 dB;
//! - the phase interpolation's error below the stopband;
//! - an impulse's delay equal to the reported group delay ±1 frame.
//!
//! The normal suite measures a few tones per ratio; the full sweep prints
//! the tables in `docs/benchmarks.md` "Clock adapter":
//!
//! ```sh
//! cargo test -p signal-output --release --offline --test adapter_quality -- --ignored --nocapture
//! ```

mod adapter_support;

use std::f64::consts::TAU;

use adapter_support::spectrum::{convert_staged, db, fit, largest, staged_len, tone};
use signal_output::adapter::{Decimator, FilterSpec, PHASES, SincTable};

/// −1 dBFS.
const AMPLITUDE: f64 = 0.891_250_938_133_745_5;

const RIPPLE_DB: f64 = 0.01;
const REJECTION_DB: f64 = 100.0;
const THD_N_DB: f64 = -100.0;

/// A ratio under test: its name, source and destination rates, the step
/// (source frames per output frame) and the start's fraction.
struct Ratio {
    name: &'static str,
    source: u32,
    dest: u32,
    step: f64,
}

const RATIOS: [Ratio; 5] = [
    Ratio {
        name: "1 (fraction 0.37)",
        source: 48_000,
        dest: 48_000,
        step: 1.0,
    },
    Ratio {
        name: "1 + 100 ppm",
        source: 48_000,
        dest: 48_000,
        step: 1.0001,
    },
    Ratio {
        name: "1 − 100 ppm",
        source: 48_000,
        dest: 48_000,
        step: 0.9999,
    },
    Ratio {
        name: "48 → 44.1 kHz",
        source: 48_000,
        dest: 44_100,
        step: 48_000.0 / 44_100.0,
    },
    Ratio {
        name: "44.1 → 48 kHz",
        source: 44_100,
        dest: 48_000,
        step: 44_100.0 / 48_000.0,
    },
];

/// The rates the adapter accepts beyond 44.1 and 48 kHz (review
/// 2026-11-06, finding 4): high source rates are halved first
/// ([`Decimator`]), high destinations convert directly.
const HIGH: [Ratio; 11] = [
    Ratio {
        name: "96 → 44.1 kHz",
        source: 96_000,
        dest: 44_100,
        step: 96_000.0 / 44_100.0,
    },
    Ratio {
        name: "96 → 48 kHz",
        source: 96_000,
        dest: 48_000,
        step: 2.0,
    },
    Ratio {
        name: "88.2 → 44.1 kHz",
        source: 88_200,
        dest: 44_100,
        step: 2.0,
    },
    Ratio {
        name: "192 → 44.1 kHz",
        source: 192_000,
        dest: 44_100,
        step: 192_000.0 / 44_100.0,
    },
    Ratio {
        name: "192 → 48 kHz",
        source: 192_000,
        dest: 48_000,
        step: 4.0,
    },
    Ratio {
        name: "192 → 48 kHz + 100 ppm",
        source: 192_000,
        dest: 48_000,
        step: 4.0004,
    },
    Ratio {
        name: "176.4 → 48 kHz",
        source: 176_400,
        dest: 48_000,
        step: 176_400.0 / 48_000.0,
    },
    Ratio {
        name: "176.4 → 44.1 kHz",
        source: 176_400,
        dest: 44_100,
        step: 4.0,
    },
    Ratio {
        name: "44.1 → 96 kHz",
        source: 44_100,
        dest: 96_000,
        step: 44_100.0 / 96_000.0,
    },
    Ratio {
        name: "48 → 192 kHz",
        source: 48_000,
        dest: 192_000,
        step: 0.25,
    },
    Ratio {
        name: "44.1 → 192 kHz",
        source: 44_100,
        dest: 192_000,
        step: 44_100.0 / 192_000.0,
    },
];

/// For each high-rate ratio, tones beyond its lower Nyquist frequency, up
/// to the source's: those that fold into the output band at each halving.
fn beyond(r: &Ratio) -> Vec<f64> {
    let low = f64::from(r.source.min(r.dest));
    let top = f64::from(r.source) / 2.0;
    [
        low / 2.0 + 50.0,
        low / 2.0 + 1_000.0,
        30_000.0,
        50_000.0,
        70_000.0,
        74_000.0,
        80_000.0,
        95_000.0,
    ]
    .into_iter()
    .filter(|&hz| hz > low / 2.0 && hz < top - 100.0)
    .collect()
}

/// The converter's side of `r`: its rate after the halvings, its filter,
/// source frames per converter frame and the halvings' delay; and a start
/// past both filters' warm-up, in converter frames.
fn inner(r: &Ratio) -> (u32, FilterSpec, f64, f64, f64) {
    let decimator = Decimator::for_rates(r.source, r.dest, 1).expect("a staging");
    let rate = decimator.rate_out();
    let spec = FilterSpec::for_rates(rate, r.dest);
    let scale = f64::from(1u32 << decimator.halvings());
    let delay = decimator.delay_frames();
    let start = spec.taps as f64 + (2.0 * delay / scale).ceil() + 0.37;
    (rate, spec, scale, delay, start)
}

/// One tone through the conversion (the halvings, then the converter):
/// (gain in dB, residual over the tone in dB, largest residual component
/// over the tone in dB, its frequency in Hz at the output).
fn measure(r: &Ratio, hz: f64, frames: usize) -> (f64, f64, f64, f64) {
    let (rate, _, _, _, start) = inner(r);
    let len = staged_len(r.source, r.dest, r.step, start, frames);
    let input = tone(hz / f64::from(r.source), AMPLITUDE, len);
    let run = convert_staged(r.source, r.dest, &input, r.step, start, frames);
    // Output sample j is the converter's input at start + j·step.
    let f = fit(&run.out, TAU * hz / f64::from(rate) * run.step);
    let (spur_cycles, spur) = largest(&f.residual);
    (
        db(f.amplitude / AMPLITUDE),
        f.residual_db(),
        db(spur / f.amplitude),
        spur_cycles * f64::from(r.dest),
    )
}

/// Log-spaced tones from 20 Hz to 20 kHz, 20 kHz included.
fn sweep(points: usize) -> Vec<f64> {
    (0..points)
        .map(|i| 20.0 * 1000f64.powf(i as f64 / (points - 1) as f64))
        .collect()
}

/// The sweep of one ratio: (worst |gain| dB, worst residual dB, worst
/// spur dB, the spur's tone and frequency).
fn sweep_ratio(r: &Ratio, points: usize, frames: usize) -> (f64, f64, f64, f64, f64) {
    let mut worst = (0.0f64, f64::MIN, f64::MIN, 0.0, 0.0);
    for hz in sweep(points) {
        let (gain, residual, spur, at) = measure(r, hz, frames);
        worst.0 = worst.0.max(gain.abs());
        worst.1 = worst.1.max(residual);
        if spur > worst.2 {
            worst.2 = spur;
            worst.3 = hz;
            worst.4 = at;
        }
    }
    worst
}

/// The rejection of a tone at `hz` (beyond the lower Nyquist frequency):
/// the input's amplitude over everything at the output, in dB.
fn rejection(r: &Ratio, hz: f64, frames: usize) -> f64 {
    let (_, _, _, _, start) = inner(r);
    let len = staged_len(r.source, r.dest, r.step, start, frames);
    let input = tone(hz / f64::from(r.source), AMPLITUDE, len);
    let out = convert_staged(r.source, r.dest, &input, r.step, start, frames).out;
    let rms = (out.iter().map(|&y| f64::from(y).powi(2)).sum::<f64>() / out.len() as f64).sqrt();
    -db(rms * 2f64.sqrt() / AMPLITUDE)
}

/// The tones beyond 48 → 44.1 kHz's lower Nyquist frequency (22.05 kHz).
const BEYOND: [f64; 5] = [22_100.0, 22_500.0, 23_000.0, 23_500.0, 23_950.0];

/// The phase interpolation's error: for fractions between the table's
/// rows, the interpolated coefficients against the exact response
/// (normalised the same way), as (worst sum of |error| over the taps, worst
/// root-sum-square), in dB.
fn interpolation_error(spec: FilterSpec, samples: usize) -> (f64, f64) {
    let table = SincTable::new(spec);
    let taps = table.taps();
    let half = (taps / 2) as f64;
    let mut row = vec![0.0f32; taps];
    let (mut worst_sum, mut worst_rss) = (0.0f64, 0.0f64);
    for i in 0..samples {
        // Fractions spread over every phase and between rows.
        let frac = ((i as u64 * 0x9E37_79B9) & 0xFFFF_FFFF) as u32;
        table.coefficients(frac, &mut row);
        let f = f64::from(frac) / 4_294_967_296.0;
        let exact: Vec<f64> = (0..taps)
            .map(|k| spec.response(f + half - 1.0 - k as f64))
            .collect();
        let sum: f64 = exact.iter().sum();
        let (mut s, mut q) = (0.0, 0.0);
        for (c, e) in row.iter().zip(&exact) {
            let d = f64::from(*c) - e / sum;
            s += d.abs();
            q += d * d;
        }
        worst_sum = worst_sum.max(s);
        worst_rss = worst_rss.max(q.sqrt());
    }
    (db(worst_sum), db(worst_rss))
}

/// An impulse at source frame `at` comes out where the read position
/// passes it: the converter is symmetric about its read position, so its
/// group delay is its lookahead, half the taps. Returns the peak's output
/// frame (parabolic interpolation) minus where the read position meets
/// `at`, in output frames.
fn impulse_offset(r: &Ratio, at: usize) -> f64 {
    let (_, spec, scale, delay, _) = inner(r);
    let mut input = vec![0.0f32; at + (4 * spec.taps) * scale as usize + 2 * delay as usize];
    input[at] = 1.0;
    let start = 0.37;
    // Where the impulse is in the converter's frames: each halving's
    // output n is its filter centred on input frame 2n.
    let at = (at as f64 + delay) / scale;
    let step = r.step / scale;
    let frames = ((at + 2.0 * spec.taps as f64) / step) as usize;
    let run = convert_staged(r.source, r.dest, &input, r.step, start, frames);
    let (out, step) = (run.out, run.step);
    let (peak, _) =
        out.iter().enumerate().fold(
            (0, f32::MIN),
            |best, (j, &y)| if y > best.1 { (j, y) } else { best },
        );
    let (a, b, c) = (
        f64::from(out[peak - 1]),
        f64::from(out[peak]),
        f64::from(out[peak + 1]),
    );
    let peak = peak as f64 + 0.5 * (a - c) / (a - 2.0 * b + c);
    peak - (at - start) / step
}

#[test]
fn converter_meets_the_targets() {
    for r in &RATIOS {
        let (ripple, residual, spur, _, _) = sweep_ratio(r, 10, 8192);
        assert!(ripple <= RIPPLE_DB, "{}: ripple {ripple} dB", r.name);
        assert!(spur <= -REJECTION_DB, "{}: spur {spur} dB", r.name);
        let (_, thd_n, _, _) = measure(r, 1000.0, 8192);
        assert!(thd_n <= THD_N_DB, "{}: THD+N {thd_n} dB", r.name);
        let _ = residual;
        let offset = impulse_offset(r, 500);
        assert!(
            offset.abs() <= 1.0,
            "{}: impulse {offset} frames off",
            r.name
        );
    }
    let down = &RATIOS[3];
    for hz in [22_100.0, 23_000.0] {
        let rejected = rejection(down, hz, 8192);
        assert!(rejected >= REJECTION_DB, "{hz} Hz: {rejected} dB");
    }
    for spec in [
        FilterSpec::for_rates(48_000, 48_000),
        FilterSpec::for_rates(48_000, 44_100),
        // The longest a high rate uses (96 → 48 kHz, unhalved).
        FilterSpec::for_rates(96_000, 48_000),
    ] {
        let (sum, _) = interpolation_error(spec, 2000);
        assert!(sum <= -REJECTION_DB, "phase interpolation {sum} dB");
    }
}

/// The high rates the adapter accepts, through the same measurements:
/// the halvings and the converter together meet §4's targets.
#[test]
fn high_rates_meet_the_targets() {
    for r in &HIGH {
        let (ripple, _, spur, tone_hz, spur_hz) = sweep_ratio(r, 10, 8192);
        let (_, thd_n, _, _) = measure(r, 1000.0, 8192);
        let offset = impulse_offset(r, 2000);
        let worst = beyond(r)
            .into_iter()
            .map(|hz| (hz, rejection(r, hz, 8192)))
            .fold((0.0, f64::MAX), |w, x| if x.1 < w.1 { x } else { w });
        println!(
            "{}: ripple {ripple:.5} dB, spur {spur:.1} dB ({tone_hz:.0} → {spur_hz:.0} Hz), THD+N {thd_n:.1} dB, impulse {offset:+.3}, least rejection {:.1} dB at {:.0} Hz",
            r.name, worst.1, worst.0
        );
        assert!(ripple <= RIPPLE_DB, "{}: ripple {ripple} dB", r.name);
        assert!(spur <= -REJECTION_DB, "{}: spur {spur} dB", r.name);
        assert!(thd_n <= THD_N_DB, "{}: THD+N {thd_n} dB", r.name);
        assert!(offset.abs() <= 1.0, "{}: impulse {offset}", r.name);
        assert!(
            worst.1 >= REJECTION_DB,
            "{}: {} dB at {} Hz",
            r.name,
            worst.1,
            worst.0
        );
    }
}

#[test]
#[ignore = "long: the full sweep; run with --release"]
fn converter_quality_full() {
    println!(
        "| ratio | taps | worst gain deviation 20 Hz–20 kHz (dB) | worst residual, any tone (dB) | worst spur (dB) at tone → spur | THD+N 1 kHz −1 dBFS (dB) | impulse vs group delay (frames) |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|");
    for r in RATIOS.iter().chain(&HIGH) {
        let (_, spec, _, _, _) = inner(r);
        let (ripple, residual, spur, tone_hz, spur_hz) = sweep_ratio(r, 61, 32_768);
        let (_, thd_n, _, _) = measure(r, 1000.0, 32_768);
        let offset = impulse_offset(r, 2000);
        println!(
            "| {} | {} | {ripple:.5} | {residual:.1} | {spur:.1} at {tone_hz:.0} → {spur_hz:.0} Hz | {thd_n:.1} | {offset:+.3} |",
            r.name, spec.taps
        );
    }
    for r in &HIGH {
        let rejections: Vec<String> = beyond(r)
            .iter()
            .map(|&hz| format!("{hz:.0} Hz: {:.1} dB", rejection(r, hz, 32_768)))
            .collect();
        if !rejections.is_empty() {
            println!("{} rejection: {}", r.name, rejections.join(", "));
        }
    }
    let down = &RATIOS[3];
    let rejections: Vec<String> = BEYOND
        .iter()
        .map(|&hz| format!("{hz:.0} Hz: {:.1} dB", rejection(down, hz, 32_768)))
        .collect();
    println!(
        "48 → 44.1 kHz rejection beyond 22.05 kHz: {}",
        rejections.join(", ")
    );
    let up = &RATIOS[4];
    let images: Vec<String> = [20_500.0, 21_000.0, 21_500.0, 22_000.0]
        .iter()
        .map(|&hz| {
            let (gain, _, spur, at) = measure(up, hz, 32_768);
            format!(
                "{hz:.0} Hz (gain {gain:.2} dB): largest other {:.1} dB at {at:.0} Hz",
                spur + gain
            )
        })
        .collect();
    println!(
        "44.1 → 48 kHz, tones in the transition band: {}",
        images.join("; ")
    );
    for (name, spec) in [
        (
            "96 taps (equal rates)",
            FilterSpec::for_rates(48_000, 48_000),
        ),
        (
            "160 taps (48 → 44.1)",
            FilterSpec::for_rates(48_000, 44_100),
        ),
        (
            "160 taps (44.1 → 48)",
            FilterSpec::for_rates(44_100, 48_000),
        ),
    ] {
        let (sum, rss) = interpolation_error(spec, 200_000);
        let (ripple, stopband) = prototype(spec);
        println!(
            "{name}: phase interpolation at {PHASES} phases: worst Σ|error| {sum:.1} dB, worst rss {rss:.1} dB; prototype: passband ripple {ripple:.6} dB, stopband {stopband:.1} dB (Kaiser's formula predicted {:.1} dB, β {:.2})",
            spec.predicted_attenuation_db, spec.beta
        );
    }
}

/// The continuous prototype's response (the filter the table samples):
/// its impulse response at 1/`PHASES` frame steps, zero-padded and
/// transformed. Returns (worst |gain| in dB to the passband edge, the
/// stopband's highest level in dB, from the stopband edge up).
fn prototype(spec: FilterSpec) -> (f64, f64) {
    let half = spec.taps as f64 / 2.0;
    let points = spec.taps * PHASES;
    let n = (points * 8).next_power_of_two();
    let mut x = vec![0.0f64; n];
    for (i, slot) in x.iter_mut().take(points + 1).enumerate() {
        *slot = spec.response(i as f64 / PHASES as f64 - half) / PHASES as f64;
    }
    let fft = realfft::RealFftPlanner::<f64>::new().plan_fft_forward(n);
    let mut out = fft.make_output_vec();
    fft.process(&mut x, &mut out).unwrap();
    let dc = out[0].norm();
    let (mut ripple, mut stop) = (0.0f64, 0.0f64);
    for (b, c) in out.iter().enumerate() {
        let f = b as f64 * PHASES as f64 / n as f64;
        let level = c.norm() / dc;
        if f <= spec.passband {
            ripple = ripple.max(db(level).abs());
        } else if f >= spec.stopband {
            stop = stop.max(level);
        }
    }
    (ripple, db(stop))
}

/// The halvings' cost in a push: 64 channels, a 512-frame push at the
/// source rate (verification 6, finding 4). A timing; not a target.
#[test]
#[ignore = "a benchmark; run with --release"]
fn halving_cost() {
    for (source, dest) in [
        (96_000, 44_100),
        (192_000, 48_000),
        (192_000, 44_100),
        (176_400, 44_100),
    ] {
        let channels = 64;
        let mut d = Decimator::for_rates(source, dest, channels).unwrap();
        let input: Vec<f32> = (0..512 * channels)
            .map(|n| (n as f32 * 0.001).sin())
            .collect();
        let mut out = vec![0.0f32; 512 * channels];
        let pushes = 2_000;
        let start = std::time::Instant::now();
        for _ in 0..pushes {
            d.process(&input, &mut out);
        }
        let us = start.elapsed().as_secs_f64() * 1e6 / f64::from(pushes);
        println!(
            "{source} → {dest}: taps {:?}, {:.0} multiply-adds per source frame, {us:.1} µs per 512-frame push of 64 channels ({:.2} % of its {:.2} ms); {}",
            d.taps(),
            d.multiply_adds_per_frame(),
            us / (512.0 / f64::from(source) * 1e6) * 100.0,
            512.0 / f64::from(source) * 1e3,
            std::hint::black_box(out[0])
        );
    }
}

/// The converter's cost, ns per output frame, stereo (verification 6).
/// Timing on the host it runs on; a measurement, not a target.
#[test]
#[ignore = "a benchmark; run with --release"]
fn converter_cost() {
    use signal_output::adapter::{ONE, Resampler};
    for (name, source, dest) in [("96 taps", 48_000, 48_000), ("160 taps", 48_000, 44_100)] {
        let spec = FilterSpec::for_rates(source, dest);
        let mut r = Resampler::new(spec, 2, 8192);
        let frames: Vec<f32> = (0..8192).map(|n| (n as f32 * 0.001).sin()).collect();
        r.write(&frames);
        let step = (f64::from(source) / f64::from(dest) * 1.0001 * ONE as f64) as u64;
        let mut out = [0.0f32; 2];
        let reads = 2_000_000u64;
        let start = std::time::Instant::now();
        let mut position = (spec.taps as u64) << 32;
        for _ in 0..reads {
            r.read_add(position, 1.0, &mut out);
            position += step;
            if (position >> 32) > 3000 {
                position = (spec.taps as u64) << 32;
            }
        }
        let ns = start.elapsed().as_nanos() as f64 / reads as f64;
        println!(
            "{name}, 2 channels: {ns:.1} ns per output frame ({:.1} µs for a 128-frame callback, {:.1} µs with a crossfade's second head); {}",
            ns * 128.0 / 1000.0,
            2.0 * ns * 128.0 / 1000.0,
            std::hint::black_box(out[0])
        );
    }
}
