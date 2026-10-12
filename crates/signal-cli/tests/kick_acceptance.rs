//! The M1 acceptance suite (`docs/01-inception-proposal.md` §8.4, V1–V8) on the
//! test project `projects/kick`, and its golden render. What each test shows,
//! and how to update the golden: `docs/testing-and-evaluation.md`.
//!
//! Renders run in process (`Project::load` → `plan` → `Engine` → `render`, as
//! `signal render` does) when only the audio and log are needed, and through
//! the `signal` binary when the command itself is under test.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use signal_core::{InstanceAddress, Registry, RngStream};
use signal_engine::{Engine, EngineConfig, Render, render};
use signal_project::{Project, SourceFile};

/// The project's rate and the transport's settings in `projects/kick/project.toml`.
const RATE: u32 = 96_000;
const BPM_MILLI: u128 = 132_000;
const GHOST_PROBABILITY: f64 = 0.15;
const GHOST_OFFSET_MS: f64 = 6.0;
const ACCENT_EVERY: usize = 3;

/// The trigger inputs of the kick's two envelopes.
const KICK_TRIGGERS: [&str; 2] = [
    "/drums/kick/amp_env.trigger",
    "/drums/kick/pitch_env.trigger",
];

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn kick() -> PathBuf {
    repository().join("projects/kick")
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).expect("std names are unique");
    registry
}

/// Renders `project` in process, as `signal render` does.
fn render_project(project: &Project, seed: u64, sample_rate: u32, frames: u64) -> Render {
    let registry = registry();
    let planned = project.plan(&registry).expect("the project is valid");
    let config = EngineConfig::new(sample_rate, seed);
    let mut engine = Engine::new(&planned.plan, &registry, config, Vec::new()).unwrap();
    render(&mut engine, frames).unwrap()
}

fn render_kick(seed: u64, frames: u64) -> Render {
    render_project(&Project::load(&kick()).unwrap(), seed, RATE, frames)
}

/// The samples as `audio.wav` stores them.
fn as_f32(render: &Render) -> Vec<f32> {
    render.samples.iter().map(|&x| x as f32).collect()
}

fn signal(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(args)
        .output()
        .expect("the signal binary runs")
}

/// A fresh output folder for one test, inside the target directory.
fn out_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Renders `projects/kick` with the binary; `extra` adds options.
fn render_cli(out: &Path, frames: u64, extra: &[&str]) -> Output {
    let (project, out, frames) = (
        kick().display().to_string(),
        out.display().to_string(),
        frames.to_string(),
    );
    let args = [
        &[
            "render", &project, "--frames", &frames, "--out", &out, "--json",
        ][..],
        extra,
    ]
    .concat();
    signal(&args)
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// `round_half_up(p/q) = ⌊(2p + q) / 2q⌋`, the transport's rounding.
fn ratio(p: u128, q: u128) -> u64 {
    ((2 * p + q) / (2 * q)) as u64
}

/// The frame of beat `k`: `round_half_up(k·60000·sr / bpm_milli)`.
fn beat(k: u64, rate: u32) -> u64 {
    ratio(u128::from(k) * 60_000 * u128::from(rate), BPM_MILLI)
}

/// The frame count of `n` 4-beat bars.
fn bars(n: u64, rate: u32) -> u64 {
    beat(4 * n, rate)
}

/// The frames of the ghost notes of `seed` below `frames`: every off-beat
/// 16th `j` (j mod 4 ≠ 0) at `round_half_up(j·15000·sr / bpm_milli) +
/// round_half_up(offset_ms·sr/1000)`, kept when its draw from the transport's
/// stream is below the probability. One draw per candidate, in order.
fn expected_ghosts(seed: u64, rate: u32, frames: u64) -> Vec<u64> {
    let offset = (GHOST_OFFSET_MS * f64::from(rate) / 1000.0 + 0.5).floor() as u64;
    let address: InstanceAddress = "/transport".parse().unwrap();
    let mut stream = RngStream::derive(seed, &address);
    (1u64..)
        .filter(|j| j % 4 != 0)
        .map(|j| ratio(u128::from(j) * 15_000 * u128::from(rate), BPM_MILLI) + offset)
        .take_while(|&frame| frame < frames)
        .filter(|_| stream.next_f64() < GHOST_PROBABILITY)
        .collect()
}

/// The lines of `log.jsonl`.
fn log_lines(log: &[u8]) -> Vec<Value> {
    std::str::from_utf8(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The frames of the `event` records from port `source` to port `target`.
fn event_frames(lines: &[Value], source: &str, target: &str) -> Vec<u64> {
    lines
        .iter()
        .filter(|l| l["type"] == "event" && l["source"]["port"] == source && l["target"] == target)
        .map(|l| l["frame"].as_u64().unwrap())
        .collect()
}

/// V1: the same project and seed rendered twice give identical hashes and
/// identical audio files.
#[test]
fn v1_same_seed_same_audio_hash() {
    let (a, b) = (out_dir("kick-v1-a"), out_dir("kick-v1-b"));
    for out in [&a, &b] {
        let output = render_cli(out, bars(4, RATE), &[]);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
    }
    let (first, second) = (
        read_json(&a.join("manifest.json")),
        read_json(&b.join("manifest.json")),
    );
    for hash in ["audio_fnv1a64", "log_fnv1a64"] {
        assert_eq!(first[hash], second[hash], "{hash}");
    }
    assert_eq!(
        std::fs::read(a.join("audio.wav")).unwrap(),
        std::fs::read(b.join("audio.wav")).unwrap()
    );
}

/// V2: every primitive the project uses is a `std.*` type, and each has the
/// contract suite run on it in `crates/signal-std/tests/contract.rs`. Those
/// tests are referenced here, not repeated; they run with the workspace tests.
#[test]
fn v2_every_primitive_used_has_contract_tests() {
    let project = Project::load(&kick()).unwrap();
    let planned = project.plan(&registry()).unwrap();
    let used: BTreeSet<&str> = planned.plan.instances.iter().map(|i| i.type_name).collect();
    let contract =
        std::fs::read_to_string(repository().join("crates/signal-std/tests/contract.rs")).unwrap();
    for name in &used {
        assert!(name.starts_with("std."), "{name} is not a std primitive");
        assert!(
            contract.contains(&format!("contract_tests!(super::std_type(\"{name}\"))")),
            "{name} has no contract tests in signal-std/tests/contract.rs"
        );
    }
    assert_eq!(
        used,
        BTreeSet::from([
            "std.audio_out",
            "std.dc_block",
            "std.exp_env",
            "std.lfo",
            "std.mul",
            "std.sine_osc",
            "std.tanh_shaper",
            "std.transport",
            "std.unit_delay",
        ])
    );
}

/// V3: every beat and ghost trigger in `log.jsonl` is on the transport's
/// integer grid, and the first kick starts on the frame after its trigger.
///
/// The expected first non-zero frame, from the definitions: beat 0 is at frame
/// 0. `amp_env` (`std.exp_env`, attack 0.3 ms) has an attack of A =
/// round_half_up(0.3·96000/1000) = 29 frames and outputs L + (1 − L)·i/A, so
/// with L = 0 its frame-0 output is exactly 0; the oscillator's frame-0 output
/// is sin(2π·(0 + 0.05·0)) = 0 as well. At frame 1 `amp_env` is 1/29 and the
/// oscillator sin(2π·180/96000) ≠ 0; the shaper's ADAA step from 0 and the DC
/// blocker pass a non-zero value. So the first non-zero sample is **frame 1**,
/// the frame after the trigger.
#[test]
fn v3_triggers_on_the_integer_grid_and_first_onset() {
    let frames = bars(16, RATE);
    let rendered = render_kick(42, frames);
    let lines = log_lines(&rendered.log);

    let beats: Vec<u64> = (0..64).map(|k| beat(k, RATE)).collect();
    let ghosts = expected_ghosts(42, RATE, frames);
    assert!(!ghosts.is_empty());
    for target in KICK_TRIGGERS {
        assert_eq!(
            event_frames(&lines, "/transport.beat", target),
            beats,
            "{target}"
        );
        assert_eq!(
            event_frames(&lines, "/transport.ghost", target),
            ghosts,
            "{target}"
        );
    }
    let accents: Vec<u64> = beats.iter().copied().step_by(ACCENT_EVERY).collect();
    assert_eq!(
        event_frames(&lines, "/transport.accent", "/accent.trigger"),
        accents
    );
    // Every record is an event that took effect: nothing discarded, no faults.
    assert!(lines.iter().all(|l| l["type"] == "event"));
    assert_eq!(
        lines.len(),
        2 * (beats.len() + ghosts.len()) + accents.len()
    );

    let samples = as_f32(&rendered);
    let channels = usize::from(rendered.channels);
    let first = samples
        .chunks(channels)
        .position(|frame| frame.iter().any(|&x| x != 0.0))
        .unwrap();
    assert_eq!(beats[0], 0);
    assert_eq!(first, 1, "first non-zero frame");
}

/// V4: seeds 42 and 43 give the same beats and accents, different ghosts, and
/// audio that is bit-identical up to the first frame where the ghosts differ.
#[test]
fn v4_seed_changes_only_ghosts() {
    let frames = bars(4, RATE);
    let (a, b) = (render_kick(42, frames), render_kick(43, frames));
    let (la, lb) = (log_lines(&a.log), log_lines(&b.log));
    let target = KICK_TRIGGERS[0];
    assert_eq!(
        event_frames(&la, "/transport.beat", target),
        event_frames(&lb, "/transport.beat", target)
    );
    assert_eq!(
        event_frames(&la, "/transport.accent", "/accent.trigger"),
        event_frames(&lb, "/transport.accent", "/accent.trigger")
    );
    let (ga, gb): (BTreeSet<u64>, BTreeSet<u64>) = (
        event_frames(&la, "/transport.ghost", target)
            .into_iter()
            .collect(),
        event_frames(&lb, "/transport.ghost", target)
            .into_iter()
            .collect(),
    );
    assert_ne!(ga, gb);
    let first = *ga.symmetric_difference(&gb).next().unwrap();
    println!("V4: first differing ghost at frame {first}");

    let channels = usize::from(a.channels);
    let bits = |r: &Render| -> Vec<u32> { as_f32(r).iter().map(|x| x.to_bits()).collect() };
    let (sa, sb) = (bits(&a), bits(&b));
    let split = first as usize * channels;
    assert_eq!(sa[..split], sb[..split], "identical before frame {first}");
    assert_ne!(sa[split..], sb[split..], "different from frame {first} on");
}

/// V5: a 64-bar render has no NaN or infinity, a 4× oversampled peak at or
/// below −0.3 dBFS, and |mean| < 1e-4 on each channel.
#[test]
fn v5_floors_over_64_bars() {
    let rendered = render_kick(42, bars(64, RATE));
    let samples = as_f32(&rendered);
    let metrics = signal_eval::metrics(&samples, rendered.channels);
    let true_peaks = signal_eval::true_peak_4x(&samples, rendered.channels);
    let ceiling = 10f64.powf(-0.3 / 20.0);
    for (channel, (m, &true_peak)) in metrics.channels.iter().zip(&true_peaks).enumerate() {
        println!(
            "V5 channel {channel}: sample peak {:.6} ({:.3} dBFS), 4x peak {true_peak:.6} ({:.3} dBFS), mean {:e}",
            m.peak,
            20.0 * m.peak.log10(),
            20.0 * true_peak.log10(),
            m.mean
        );
        assert_eq!(m.non_finite, 0, "channel {channel}");
        assert!(
            true_peak <= ceiling,
            "channel {channel}: 4x peak {true_peak}"
        );
        assert!(m.mean.abs() < 1e-4, "channel {channel}: mean {}", m.mean);
    }
}

/// Frames skipped before the analysis (the shaper starts from u[−1] = 0).
const V6_SKIP: u64 = 1000;
/// One second at 96 kHz: 1 Hz bins and a whole number of periods of every
/// whole-hertz V6 tone, so the Hann window adds no leakage.
const V6_FRAMES: usize = 96_000;

/// `std.tanh_shaper` at drive 20 on a pure `std.sine_osc` tone of `freq` Hz at
/// 96 kHz: the inharmonic ratio in dB with ADAA (the shaper), and of naive
/// `tanh(20·x)` on the same sine for comparison.
fn shaper_aliasing(freq: u32) -> (f64, f64) {
    let text = format!(
        "schema_version = 1\n\
         [project]\nname = \"shaper\"\nsample_rate = 96000\n\
         [instances]\n\
         osc = {{ module = \"std.sine_osc\", freq = {freq} }}\n\
         shaper = {{ module = \"std.tanh_shaper\", drive = 20 }}\n\
         out = {{ module = \"std.audio_out\" }}\n\
         [wiring]\nconnections = [\n\
           {{ from = \"osc.out\", to = \"shaper.in\" }},\n\
           {{ from = \"shaper.out\", to = \"out.l\" }},\n\
           {{ from = \"osc.out\", to = \"out.r\" }},\n\
         ]\n"
    );
    let file = SourceFile {
        path: "project.toml".to_owned(),
        text,
    };
    let project = Project::from_sources(vec![file]).unwrap();
    let rendered = render_project(&project, 0, RATE, V6_SKIP + V6_FRAMES as u64);
    let frames = rendered.samples.chunks(2).skip(V6_SKIP as usize);
    let (shaped, naive): (Vec<f64>, Vec<f64>) =
        frames.map(|f| (f[0], (20.0 * f[1]).tanh())).unzip();
    let ratio = |x: &[f64]| {
        signal_eval::aliasing(x, f64::from(RATE), f64::from(freq))
            .unwrap()
            .ratio_db
    };
    (ratio(&shaped), ratio(&naive))
}

/// V6 (gating, owner decision sq-4aece389 option A): the shaper's inharmonic
/// energy is at least 60 dB below the fundamental at 1009, 2003, 4001 and
/// 8009 Hz. These tones do not divide 96 kHz, so the aliases land between the
/// harmonics and are counted. Naive tanh is printed for comparison.
#[test]
#[ignore = "std.tanh_shaper misses 60 dB on off-divisor tones; follow-up issue \
            2026-10-11-follow-up-bring-std-tanh-shaper-aliasing-to-60-db-on-off-div. \
            Measured ADAA / naive tanh: 1009 Hz 49.0 / 43.8 dB, 2003 Hz 33.5 / 27.3 dB, \
            4001 Hz 26.1 / 18.5 dB, 8009 Hz 22.0 / 13.1 dB"]
fn v6_shaper_aliasing_at_least_60_db() {
    let mut misses = Vec::new();
    for freq in [1009, 2003, 4001, 8009] {
        let (adaa, naive) = shaper_aliasing(freq);
        println!("V6 {freq} Hz: ADAA {adaa:.1} dB, naive tanh {naive:.1} dB");
        // A NaN measurement is a miss, not a pass.
        if adaa.is_nan() || adaa < 60.0 {
            misses.push(format!("{freq} Hz: {adaa:.1} dB"));
        }
    }
    assert!(misses.is_empty(), "below 60 dB: {}", misses.join(", "));
}

/// V7: the kick with its feedback taken from the shaper straight into the
/// oscillator's phase input, without the unit delay, is rejected: exit code 1
/// and an error that lists the cycle.
#[test]
fn v7_delay_free_feedback_is_rejected() {
    let variant = out_dir("kick-v7-project");
    std::fs::create_dir_all(variant.join("composites")).unwrap();
    for file in ["project.toml", "composites/drums.toml"] {
        std::fs::copy(kick().join(file), variant.join(file)).unwrap();
    }
    let delayed = r#"{ from = "feedback.out", to = "osc.pm", scale = 0.05 }"#;
    let direct = r#"{ from = "shaper.out", to = "osc.pm", scale = 0.05 }"#;
    let text = std::fs::read_to_string(kick().join("composites/kick.toml")).unwrap();
    assert!(text.contains(delayed));
    std::fs::write(
        variant.join("composites/kick.toml"),
        text.replace(delayed, direct),
    )
    .unwrap();

    let out = out_dir("kick-v7-out");
    let (project, out_text) = (variant.display().to_string(), out.display().to_string());
    let args = ["render", &project, "--frames", "100", "--out", &out_text];
    let output = signal(&args);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    for needle in [
        "dependency cycle without a state-output instance:",
        "/drums/kick/osc",
        "/drums/kick/shaper",
    ] {
        assert!(stderr.contains(needle), "{needle} in {stderr}");
    }
    assert!(!out.exists(), "nothing is written");

    let output = signal(&[&args[..], &["--json"]].concat());
    assert_eq!(output.status.code(), Some(1));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["error"]["kind"], "validation");
}

/// The kick's pitch (`projects/kick/composites/kick.toml`): `osc.freq` is
/// 48 Hz + 132·`pitch_env`, and `pitch_env` (attack 0) is e^(−t/τ) from the
/// trigger frame, τ = 35 ms: f(t) = 48 + 132·e^(−t/τ), 180 Hz at the trigger.
const PITCH_BASE_HZ: f64 = 48.0;
const PITCH_SCALE_HZ: f64 = 132.0;
const PITCH_DECAY_S: f64 = 0.035;
/// The kick's other stages that move zero crossings: `dc` (`std.dc_block` at
/// its default cutoff), the feedback phase modulation into `osc.pm` (cycles,
/// through a unit delay), and the largest shaper drive (2.5, plus 1.5 on an
/// accent; beat 0 is accented). `amp_env` and `vca` scale by a positive factor
/// and move no crossing.
const DC_CUTOFF_HZ: f64 = 10.0;
const FEEDBACK_CYCLES: f64 = 0.05;
const MAX_DRIVE: f64 = 4.0;
/// The early sweep is checked up to 3τ after the trigger (f within 5 % of the
/// way to 48 Hz), before the first possible ghost retrigger (≈ 120 ms).
const SWEEP_SECONDS: f64 = 3.0 * PITCH_DECAY_S;

/// The mean of f(t) over `a..b` seconds after the trigger, for decay `tau`.
fn sweep_mean(a: f64, b: f64, tau: f64) -> f64 {
    PITCH_BASE_HZ + PITCH_SCALE_HZ * tau * ((-a / tau).exp() - (-b / tau).exp()) / (b - a)
}

/// The early-sweep tolerance in Hz for one period `a..b` (seconds after the
/// trigger) at `rate`. A period between two like zero crossings reads the mean
/// of the output's instantaneous frequency over it, which is the mean of f(t)
/// ([`sweep_mean`]) plus the drift, over the period, of every phase offset the
/// kick adds between `pitch_env` and the crossing. Each term is that drift,
/// derived from the kick's constants; a constant offset cancels.
///
/// - `dc`: the DC blocker leads by atan(fc/f) rad, which grows as f falls:
///   (atan(fc/f(b)) − atan(fc/f(a))) / (2π·(b − a)). Rate-independent; the
///   largest term (≈ 0.23 Hz on the first periods). The steady-state lead
///   overstates it while the filter (time constant 16 ms) settles.
/// - `drift`: two phase offsets proportional to f/sr. The oscillator sums
///   f[n]/sr frame by frame, which exceeds the integral of a falling f by
///   (f(0) − f(t))/(2·sr) cycles; and the feedback (0.05 cycles through a unit
///   delay) offsets the crossing by at most c·f/sr cycles, c = 2π·0.05 /
///   (1 − 2π·0.05) ≈ 0.46. Together (½ + c)·(f(a) − f(b)) / (sr·(b − a)):
///   0.058 Hz on the first period at 44.1 kHz, 0.013 Hz at 192 kHz.
/// - `resolution`: the measurement's own. A crossing is placed by linear
///   interpolation between two frames; for tanh(g·sin(θ·n)), θ = 2π·f/sr per
///   frame, the cubic term misplaces it by at most k/(6√3) frames, k =
///   θ²·(g²/3 + 1/6), so the period's frequency by 2·k/(6√3) / (sr·(b − a)²).
///   Below 0.001 Hz at every rate. The f32 WAV adds about 1e-7 frames.
///
/// The test also requires the tolerance to reject a rate-dependent sweep: the
/// envelope counting τ in frames of another of the four rates (τ·r/sr, at
/// least 8.8 % off) reads about 4 Hz or more off near t = τ. A 1 % error in τ
/// (decay 35.35 ms) reads about 0.5 Hz off there, above the ≈ 0.26 Hz
/// allowed, and fails.
fn sweep_tolerance(a: f64, b: f64, rate: f64) -> f64 {
    use std::f64::consts::TAU;
    let f = |t: f64| PITCH_BASE_HZ + PITCH_SCALE_HZ * (-t / PITCH_DECAY_S).exp();
    let (fa, fb, span) = (f(a), f(b), b - a);
    let dc = ((DC_CUTOFF_HZ / fb).atan() - (DC_CUTOFF_HZ / fa).atan()) / (TAU * span);
    let feedback = TAU * FEEDBACK_CYCLES;
    let drift = (0.5 + feedback / (1.0 - feedback)) * (fa - fb) / (rate * span);
    let theta = TAU * fa / rate;
    let misplaced = theta * theta * (MAX_DRIVE * MAX_DRIVE / 3.0 + 1.0 / 6.0) / (6.0 * 3f64.sqrt());
    let resolution = 2.0 * misplaced / (rate * span * span);
    dc + drift + resolution
}

/// The zero crossings of `samples` in one direction, in fractional frames,
/// placed by linear interpolation as `signal_eval::zero_crossing_frequency`
/// places the upward ones.
fn crossings(samples: &[f64], upward: bool) -> Vec<f64> {
    samples
        .windows(2)
        .enumerate()
        .filter(|(_, w)| match upward {
            true => w[0] < 0.0 && w[1] >= 0.0,
            false => w[0] > 0.0 && w[1] <= 0.0,
        })
        .map(|(n, w)| n as f64 + w[0] / (w[0] - w[1]))
        .collect()
}

/// V8's early sweep on `samples`, the left channel from the first kick's
/// trigger to 3τ after it: every period between two upward or two downward
/// crossings reads the mean of f(t) over it within [`sweep_tolerance`]. Times
/// are taken half a frame earlier than the crossing, the shaper's delay
/// (`std.tanh_shaper`, "Half-frame delay").
fn check_sweep(samples: &[f64], rate: u32) {
    let sr = f64::from(rate);
    let mut periods: Vec<(f64, f64)> = [true, false]
        .into_iter()
        .flat_map(|upward| {
            let times: Vec<f64> = crossings(samples, upward)
                .into_iter()
                .map(|n| (n - 0.5) / sr)
                .collect();
            times.windows(2).map(|w| (w[0], w[1])).collect::<Vec<_>>()
        })
        .collect();
    periods.sort_by(|x, y| x.0.total_cmp(&y.0));
    assert!(periods.len() >= 10, "{rate} Hz: {} periods", periods.len());

    let mut misses = Vec::new();
    for (i, &(a, b)) in periods.iter().enumerate() {
        let (measured, analytic) = (1.0 / (b - a), sweep_mean(a, b, PITCH_DECAY_S));
        let tolerance = sweep_tolerance(a, b, sr);
        println!(
            "V8 {rate} Hz sweep {:6.2}..{:6.2} ms: {measured:8.4} Hz, f(t) {analytic:8.4} Hz, \
             {:+.4} Hz (tolerance {tolerance:.4})",
            a * 1e3,
            b * 1e3,
            measured - analytic
        );
        if i == 0 {
            // The first period scaled back to t = 0: the onset the curve implies.
            let onset = PITCH_BASE_HZ
                + PITCH_SCALE_HZ * (measured - PITCH_BASE_HZ) / (analytic - PITCH_BASE_HZ);
            println!("V8 {rate} Hz onset: {onset:.3} Hz (f(0) = 180 Hz)");
        }
        // A NaN measurement is a miss, not a pass.
        let error = measured - analytic;
        if error.is_nan() || error.abs() > tolerance {
            misses.push(format!(
                "{a:.4}..{b:.4} s: {measured} Hz, f(t) {analytic} Hz"
            ));
        }
    }
    assert!(misses.is_empty(), "{rate} Hz sweep: {}", misses.join("; "));

    // The tolerance still rejects τ counted in frames of another rate.
    for other in [44_100u32, 48_000, 96_000, 192_000] {
        let wrong = PITCH_DECAY_S * f64::from(other) / sr;
        let caught = periods.iter().any(|&(a, b)| {
            (sweep_mean(a, b, wrong) - sweep_mean(a, b, PITCH_DECAY_S)).abs()
                > sweep_tolerance(a, b, sr)
        });
        assert!(caught || other == rate, "{rate} Hz: τ at {other} Hz passes");
    }
}

/// V8: the project renders at 44.1, 48, 96 and 192 kHz, and at each rate the
/// first kick's pitch follows its definition. The early sweep (from the
/// trigger to 3τ) reads f(t) = 48 + 132·e^(−t/τ) period by period within a
/// tolerance derived from the kick ([`sweep_tolerance`]); the late tail is
/// 48 Hz within 1 %, measured by zero crossings from 250 ms after its trigger
/// (the pitch envelope is then within 0.22 % of 48 Hz) to the kick's next
/// trigger.
#[test]
fn v8_pitch_at_every_rate() {
    for rate in [44_100u32, 48_000, 96_000, 192_000] {
        let out = out_dir(&format!("kick-v8-{rate}"));
        let output = render_cli(&out, bars(1, rate), &["--sample-rate", &rate.to_string()]);
        assert_eq!(output.status.code(), Some(0), "{rate} Hz: {output:?}");

        let log = std::fs::read(out.join("log.jsonl")).unwrap();
        let mut triggers: Vec<u64> = ["/transport.beat", "/transport.ghost"]
            .iter()
            .flat_map(|source| event_frames(&log_lines(&log), source, KICK_TRIGGERS[0]))
            .collect();
        triggers.sort_unstable();
        let (trigger, next) = (triggers[0] as usize, triggers[1] as usize);
        let start = trigger + (0.25 * f64::from(rate) + 0.5).floor() as usize;
        assert!(
            (next - start) as f64 >= 3.0 * f64::from(rate) / 48.0,
            "{rate} Hz: the tail window {start}..{next} is shorter than three periods"
        );

        let mut wav = hound::WavReader::open(out.join("audio.wav")).unwrap();
        let left: Vec<f64> = wav
            .samples::<f32>()
            .step_by(2)
            .map(|s| f64::from(s.unwrap()))
            .collect();
        let sweep_end = trigger + (SWEEP_SECONDS * f64::from(rate)) as usize;
        assert!(sweep_end <= next, "{rate} Hz: a retrigger inside the sweep");
        check_sweep(&left[trigger..sweep_end], rate);

        let freq =
            signal_eval::zero_crossing_frequency(&left[start..next], f64::from(rate)).unwrap();
        let error = freq / 48.0 - 1.0;
        println!(
            "V8 {rate} Hz: {freq:.4} Hz ({:+.3} %), frames {start}..{next}",
            error * 100.0
        );
        assert!(error.abs() < 0.01, "{rate} Hz: {freq} Hz");
    }
}

/// The golden render: seed 42, 16 bars. Its manifest, without the fields that
/// change with the build rather than the sound, equals
/// `tests/golden/kick/manifest.json`.
#[test]
fn golden_kick_16_bars() {
    let out = out_dir("kick-golden");
    let output = render_cli(&out, bars(16, RATE), &[]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let mut manifest = read_json(&out.join("manifest.json"));
    let fields = manifest.as_object_mut().unwrap();
    fields.remove("debug_assertions");
    let provenance = fields["provenance"].as_object_mut().unwrap();
    for volatile in [
        "signal_revision",
        "signal_dirty",
        "rustc_version",
        "cargo_lock_hash",
    ] {
        provenance.remove(volatile);
    }
    let keys = |v: &Value| -> BTreeSet<String> { v.as_object().unwrap().keys().cloned().collect() };
    let names =
        |list: &[&str]| -> BTreeSet<String> { list.iter().map(|s| s.to_string()).collect() };
    assert_eq!(
        keys(&manifest),
        names(&[
            "provenance",
            "seed",
            "sample_rate",
            "frames",
            "log_fnv1a64",
            "audio_fnv1a64"
        ]),
        "a new manifest field must be added to the golden or listed as volatile"
    );
    assert_eq!(
        keys(&manifest["provenance"]),
        names(&["project_file_hashes"])
    );

    let golden = repository().join("tests/golden/kick/manifest.json");
    let fresh = serde_json::to_string_pretty(&manifest).unwrap();
    let expected = std::fs::read_to_string(&golden).map(|t| serde_json::from_str::<Value>(&t));
    assert!(
        matches!(&expected, Ok(Ok(golden)) if *golden == manifest),
        "the render differs from {}; see docs/testing-and-evaluation.md before updating it. \
         Fresh manifest:\n{fresh}",
        golden.display()
    );
}

/// M2.6-2 (ADR 0019 §2): the golden render on 4 threads, every segment on
/// the workers, has the golden's audio and log hashes.
#[test]
fn golden_kick_on_workers() {
    let golden = read_json(&repository().join("tests/golden/kick/manifest.json"));
    let registry = registry();
    let project = Project::load(&kick()).unwrap();
    let planned = project.plan(&registry).expect("the project is valid");
    let rate = u32::try_from(golden["sample_rate"].as_u64().unwrap()).unwrap();
    let config = EngineConfig {
        threads: 4,
        schedule: signal_engine::Schedule::FORCED,
        ..EngineConfig::new(rate, golden["seed"].as_u64().unwrap())
    };
    let mut engine = Engine::new(&planned.plan, &registry, config, Vec::new()).unwrap();
    let rendered = render(&mut engine, golden["frames"].as_u64().unwrap()).unwrap();
    assert!(engine.schedule_stats().worker_tasks > 0);
    assert_eq!(
        format!("{:016x}", rendered.audio_fnv1a64()),
        golden["audio_fnv1a64"].as_str().unwrap()
    );
    assert_eq!(
        format!("{:016x}", rendered.log_fnv1a64()),
        golden["log_fnv1a64"].as_str().unwrap()
    );
    assert_eq!(rendered.frames, golden["frames"].as_u64().unwrap());
}
