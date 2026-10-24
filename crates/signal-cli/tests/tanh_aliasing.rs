//! The M3 "ADAA versus oversampling" experiment's measurements
//! (`experiments/m3-tanh-aliasing/`, follow-up issue
//! `2026-10-11-follow-up-bring-std-tanh-shaper-aliasing-to-60-db-on-off-div`):
//! V6's measurement (`kick_acceptance.rs`, `shaper_aliasing`; the method is
//! `docs/testing-and-evaluation.md`, "The V6 measurement") run on
//! `std.tanh_shaper`, naive `tanh`, the promoted candidate
//! `std.tanh_shaper_os8` (formerly `lab.tanh_os8_adaa1`, verdict 2026-10-23)
//! and the `lab.tanh_*` candidates left in the lab, at 96 and 48 kHz and at
//! several drives, with each candidate's delay and its gain and phase in the
//! passband. V6 itself now gates `std.tanh_shaper_os8` (`kick_acceptance.rs`).
//!
//! The numbers are **measurements, not verdicts** (AGENTS.md rule 6): no test
//! here compares a candidate with the 60 dB threshold. The quick tests check
//! that the measurement is V6's (it reproduces V6's published values for
//! `std.tanh_shaper`) and that each candidate's delay is the documented one;
//! the table is printed by the ignored test:
//!
//! ```sh
//! cargo test --release -p signal-cli --features lab --test tanh_aliasing -- --ignored --nocapture
//! ```

#![cfg(feature = "lab")]

use signal_core::Registry;
use signal_engine::{Engine, EngineConfig};
use signal_output::render;
use signal_project::{Project, SourceFile};

/// V6's start-up skip, in frames.
const SKIP: u64 = 1000;
/// V6's tones (off-divisor at 96 kHz; at 48 kHz too).
const TONES: [u32; 4] = [1009, 2003, 4001, 8009];
/// The current shaper, then the candidates, in the experiment's order.
const SHAPERS: [&str; 6] = [
    "std.tanh_shaper",
    "lab.tanh_adaa2",
    "lab.tanh_os2_adaa1",
    "lab.tanh_os4_adaa1",
    "std.tanh_shaper_os8",
    "lab.tanh_os8",
];

/// The candidates: every shaper but the current one.
fn candidates() -> impl Iterator<Item = &'static str> {
    SHAPERS.into_iter().skip(1)
}

/// The documented delay of `shaper` in frames: `std.tanh_shaper`'s half
/// frame, `std.tanh_shaper_os8`'s 73.8125, or a lab candidate's.
fn delay_frames(shaper: &str) -> Option<f64> {
    match shaper {
        "std.tanh_shaper" => Some(0.5),
        "std.tanh_shaper_os8" => Some(signal_std::TanhShaperOs8::DELAY_FRAMES),
        _ => signal_lab::tanh_shapers::delay_frames(shaper),
    }
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).expect("std names are unique");
    signal_lab::register(&mut registry).expect("lab names are unique");
    registry
}

/// One second of `shaper` (at `drive`) on a pure `std.sine_osc` tone of
/// `freq` Hz at amplitude `amplitude`, after V6's skip: (left = shaped, right
/// = the sine itself), as V6's project renders it.
fn shaped(shaper: &str, freq: u32, rate: u32, drive: f64, amplitude: f64) -> (Vec<f64>, Vec<f64>) {
    let text = format!(
        "schema_version = 1\n\
         [project]\nname = \"shaper\"\nsample_rate = {rate}\n\
         [instances]\n\
         osc = {{ module = \"std.sine_osc\", freq = {freq} }}\n\
         shaper = {{ module = \"{shaper}\", drive = {drive} }}\n\
         out = {{ module = \"std.audio_out\" }}\n\
         [wiring]\nconnections = [\n\
           {{ from = \"osc.out\", to = \"shaper.in\", scale = {amplitude} }},\n\
           {{ from = \"shaper.out\", to = \"out.l\" }},\n\
           {{ from = \"osc.out\", to = \"out.r\", scale = {amplitude} }},\n\
         ]\n"
    );
    let project = Project::from_sources(vec![SourceFile {
        path: "project.toml".to_owned(),
        text,
    }])
    .unwrap();
    let registry = registry();
    let planned = project.plan(&registry).expect("the project is valid");
    let mut engine = Engine::new(
        &planned.plan,
        &registry,
        EngineConfig::new(rate, 0),
        Vec::new(),
    )
    .unwrap();
    let rendered = render(&mut engine, SKIP + u64::from(rate)).unwrap();
    rendered
        .samples
        .chunks(2)
        .skip(SKIP as usize)
        .map(|f| (f[0], f[1]))
        .unzip()
}

/// V6's ratio (dB of inharmonic energy below the fundamental) of `shaper`,
/// or of naive `tanh(drive·x)` on the same sine for `"naive"`.
fn ratio_db(shaper: &str, freq: u32, rate: u32, drive: f64) -> f64 {
    let source = if shaper == "naive" {
        "std.tanh_shaper"
    } else {
        shaper
    };
    let (out, sine) = shaped(source, freq, rate, drive, 1.0);
    let samples: Vec<f64> = if shaper == "naive" {
        sine.iter().map(|x| (drive * x).tanh()).collect()
    } else {
        out
    };
    signal_eval::aliasing(&samples, f64::from(rate), f64::from(freq))
        .unwrap()
        .ratio_db
}

/// The complex amplitude of `x` at `freq` Hz (a single DFT bin over a whole
/// number of periods).
fn bin(x: &[f64], freq: f64, rate: f64) -> (f64, f64) {
    let w = std::f64::consts::TAU * freq / rate;
    x.iter().enumerate().fold((0.0, 0.0), |(re, im), (n, v)| {
        let phase = w * n as f64;
        (re + v * phase.cos(), im - v * phase.sin())
    })
}

/// At drive 0.1 on a sine of amplitude 0.1 (u ≤ 0.01: `tanh` linear within
/// 3e-5): the gain in dB of `shaper`'s output against `tanh(drive·x)` of the
/// same sine at `freq`, and the delay in frames read from the phase.
fn passband(shaper: &str, freq: u32, rate: u32) -> (f64, f64) {
    let drive = 0.1;
    let (out, sine) = shaped(shaper, freq, rate, drive, 0.1);
    let reference: Vec<f64> = sine.iter().map(|x| (drive * x).tanh()).collect();
    let (f, r) = (f64::from(freq), f64::from(rate));
    let (a, b) = (bin(&out, f, r), bin(&reference, f, r));
    let gain_db = 10.0 * ((a.0 * a.0 + a.1 * a.1) / (b.0 * b.0 + b.1 * b.1)).log10();
    // Phase lag of out against the reference, in cycles, unwrapped by the
    // documented delay (which can exceed one period at 10 kHz).
    let lag = (b.1.atan2(b.0) - a.1.atan2(a.0)) / std::f64::consts::TAU;
    let documented = delay_frames(shaper).unwrap() * f / r;
    let lag = lag + (documented - lag).round();
    (gain_db, lag * r / f)
}

/// V6's measurement here is V6's: `std.tanh_shaper` at drive 20 and 96 kHz
/// reads the values V6 publishes (`docs/testing-and-evaluation.md`), to 0.05
/// dB.
#[test]
fn the_measurement_reproduces_v6_for_the_current_shaper() {
    let published = [(1009, 49.0, 43.8), (4001, 26.1, 18.5)];
    for (freq, adaa, naive) in published {
        let measured = ratio_db("std.tanh_shaper", freq, 96_000, 20.0);
        let measured_naive = ratio_db("naive", freq, 96_000, 20.0);
        assert!((measured - adaa).abs() < 0.05, "{freq} Hz: {measured}");
        assert!(
            (measured_naive - naive).abs() < 0.05,
            "{freq} Hz: {measured_naive}"
        );
    }
}

/// Every candidate delays a passband tone by its documented delay (to 0.01
/// frame at 1 kHz and 96 kHz), and passes it at unit gain within 0.02 dB
/// (ADAA's own droop there: −0.005 dB for first order, −0.012 dB for second).
#[test]
fn every_candidate_has_its_documented_delay() {
    for shaper in candidates() {
        let documented = delay_frames(shaper).unwrap();
        let (gain_db, delay) = passband(shaper, 1000, 96_000);
        assert!(
            (delay - documented).abs() < 0.01,
            "{shaper}: {delay} vs {documented}"
        );
        assert!(gain_db.abs() < 0.02, "{shaper}: {gain_db} dB");
    }
}

/// The candidates are the lab's and the promoted one.
#[test]
fn the_candidates_are_the_lab_s_and_the_promoted_one() {
    let mut expected: Vec<&str> = signal_lab::tanh_shapers::TYPES.to_vec();
    expected.insert(3, "std.tanh_shaper_os8");
    assert_eq!(candidates().collect::<Vec<_>>(), expected);
}

/// Every candidate renders the same bits on one thread and on four workers
/// (the forced schedule), with its drive modulated by an LFO.
#[test]
fn the_candidates_render_the_same_on_any_thread_count() {
    let mut text = String::from(
        "schema_version = 1\n[project]\nname = \"threads\"\nsample_rate = 48000\n[instances]\n\
         osc = { module = \"std.sine_osc\", freq = 2003 }\n\
         lfo = { module = \"std.lfo\", freq_hz = 3 }\n\
         out = { module = \"std.audio_out\", gain = 0.1 }\n",
    );
    let mut wiring = String::from("[wiring]\nconnections = [\n");
    for (i, shaper) in candidates().enumerate() {
        text.push_str(&format!("s{i} = {{ module = \"{shaper}\", drive = 10 }}\n"));
        wiring.push_str(&format!(
            "{{ from = \"osc.out\", to = \"s{i}.in\" }},\n\
             {{ from = \"lfo.out\", to = \"s{i}.drive\", scale = 9 }},\n\
             {{ from = \"s{i}.out\", to = \"out.{}\" }},\n",
            if i % 2 == 0 { "l" } else { "r" }
        ));
    }
    wiring.push_str("]\n");
    let project = Project::from_sources(vec![SourceFile {
        path: "project.toml".to_owned(),
        text: text + &wiring,
    }])
    .unwrap();
    let registry = registry();
    let planned = project.plan(&registry).expect("the project is valid");
    let hashes = [1, 4].map(|threads| {
        let config = EngineConfig {
            threads,
            schedule: signal_engine::Schedule::FORCED,
            ..EngineConfig::new(48_000, 1)
        };
        let mut engine = Engine::new(&planned.plan, &registry, config, Vec::new()).unwrap();
        let rendered = render(&mut engine, 24_000).unwrap();
        assert!(rendered.samples.iter().all(|x| x.is_finite()));
        if threads > 1 {
            assert!(engine.schedule_stats().worker_tasks > 0);
        }
        rendered.audio_fnv1a64()
    });
    assert_eq!(hashes[0], hashes[1]);
}

/// The experiment's table (`experiments/m3-tanh-aliasing/README.md`):
/// V6's ratio for every shaper at drives 2, 5 and 20, at 96 and 48 kHz; then
/// each shaper's gain and delay at 1, 10, 18 and 20 kHz; then the peak of a
/// sine at drive 20 (a clipped wave, band-limited, overshoots: a square
/// whose only harmonic below the passband edge is its fundamental peaks at
/// 4/π).
#[test]
#[ignore = "prints the M3 aliasing experiment's table (a few seconds in a release build)"]
fn print_the_experiment_table() {
    for rate in [96_000, 48_000] {
        for drive in [20.0, 5.0, 2.0] {
            println!("\n### {} kHz, drive {drive}\n", rate / 1000);
            println!("| Shaper | 1009 Hz | 2003 Hz | 4001 Hz | 8009 Hz |");
            println!("|---|---:|---:|---:|---:|");
            for shaper in std::iter::once("naive").chain(SHAPERS) {
                let cells: Vec<String> = TONES
                    .iter()
                    .map(|&f| format!("{:.1}", ratio_db(shaper, f, rate, drive)))
                    .collect();
                println!("| `{shaper}` | {} |", cells.join(" | "));
            }
        }
    }
    for rate in [96_000, 48_000] {
        println!(
            "\n### Passband at {} kHz (drive 0.1, sine 0.1): gain dB / delay frames\n",
            rate / 1000
        );
        println!("| Shaper | 1 kHz | 10 kHz | 18 kHz | 20 kHz |");
        println!("|---|---:|---:|---:|---:|");
        for shaper in SHAPERS {
            let cells: Vec<String> = [1000, 10_000, 18_000, 20_000]
                .iter()
                .map(|&f| {
                    let (gain, delay) = passband(shaper, f, rate);
                    format!("{gain:+.3} / {delay:.3}")
                })
                .collect();
            println!("| `{shaper}` | {} |", cells.join(" | "));
        }
    }
    println!("\n### Peak of a sine at drive 20, 48 kHz\n");
    println!("| Shaper | 100 Hz | 1009 Hz | 4001 Hz | 8009 Hz |");
    println!("|---|---:|---:|---:|---:|");
    for shaper in SHAPERS {
        let peaks: Vec<String> = [100, 1009, 4001, 8009]
            .iter()
            .map(|&freq| {
                let (out, _) = shaped(shaper, freq, 48_000, 20.0, 1.0);
                format!("{:.4}", out.iter().fold(0.0_f64, |m, x| m.max(x.abs())))
            })
            .collect();
        println!("| `{shaper}` | {} |", peaks.join(" | "));
    }
}
