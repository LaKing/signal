//! M2.10 (the owner's list) through the engine: the native DSP primitives
//! `std.biquad`, `std.comb`, `std.allpass_delay`, `std.fdn_reverb`,
//! `std.fm_op` and `std.wavetable` stay within the magnitude bound (10⁶,
//! `docs/semantics.md` §7) at their most extreme settings, with audio-rate
//! modulation of their parameters and a 10⁶ input, without a fault; and a
//! project of all six renders bit-identically for k = 1, 64, 256 on 1, 2, 4
//! and 8 threads. The projects are written by the tests.

use signal_core::{InstanceAddress, Registry};
use signal_engine::{Engine, EngineConfig, Schedule};
use signal_output::{Render, render_blocks};
use signal_project::{Project, SourceFile};

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).expect("std names are unique");
    registry
}

fn project(text: &str) -> Project {
    Project::from_sources(vec![SourceFile {
        path: "project.toml".to_owned(),
        text: text.to_owned(),
    }])
    .unwrap()
}

/// `instances` and `connections` (TOML lines) as a project at `rate`.
fn project_text(rate: u32, instances: &str, connections: &str) -> String {
    format!(
        "schema_version = 1\n\n[project]\nname = \"dsp\"\nsample_rate = {rate}\nseed = 3\n\n\
         [instances]\n{instances}\nout = {{ module = \"std.audio_out\", gain = 0.5 }}\n\n\
         [wiring]\nconnections = [\n{connections}]\n"
    )
}

/// Renders `frames` of `project` in blocks of `k` on `threads` threads.
fn render(project: &Project, frames: u64, k: usize, threads: usize) -> (Render, Engine) {
    let registry = registry();
    let plan = project.plan(&registry).unwrap().plan;
    let config = EngineConfig {
        threads,
        schedule: if threads > 1 {
            Schedule::FORCED
        } else {
            Schedule::DEFAULT
        },
        ..EngineConfig::new(project.sample_rate, project.seed)
    };
    let mut engine = Engine::new(&plan, &registry, config, Vec::new()).unwrap();
    let render = render_blocks(&mut engine, frames, k, |_| Ok(None)).unwrap();
    assert_eq!(engine.log_overflows(), 0);
    (render, engine)
}

fn faults(render: &Render) -> Vec<String> {
    String::from_utf8_lossy(&render.log)
        .lines()
        .filter(|l| l.contains("\"type\":\"fault\""))
        .map(str::to_owned)
        .collect()
}

/// The shared stimulus: a 110 Hz saw, a constant 10⁶ and a 1 kHz sine (an
/// audio-rate modulator).
const STIMULUS: &str = "saw = { module = \"std.osc\", shape = 1, freq = 110 }\n\
                        top = { module = \"std.const\", value = 1000000 }\n\
                        lfo = { module = \"std.sine_osc\", freq = 1000 }\n";

/// Each case: the instance under test `x` (with its extreme settings), its
/// connections from the stimulus, and its outputs.
const CASES: [(&str, &str, &str); 6] = [
    (
        "x = { module = \"std.biquad\", mode = 4, cutoff_hz = 20000, q = 40, gain_db = 48 }",
        "{ from = \"saw.out\", to = \"x.in\" },\n\
         { from = \"top.out\", to = \"x.in\" },\n\
         { from = \"lfo.out\", to = \"x.cutoff_hz\", scale = 20000 },\n\
         { from = \"lfo.out\", to = \"x.mode\", scale = 4 },\n\
         { from = \"lfo.out\", to = \"x.q\", scale = 40 },\n",
        "out",
    ),
    (
        "x = { module = \"std.comb\", time_ms = 500, feedback = 0.999, damping_hz = 1000000 }",
        "{ from = \"saw.out\", to = \"x.in\" },\n\
         { from = \"top.out\", to = \"x.in\" },\n\
         { from = \"lfo.out\", to = \"x.time_ms\", scale = 500 },\n",
        "out",
    ),
    (
        "x = { module = \"std.allpass_delay\", time_ms = 500, gain = 0.999 }",
        "{ from = \"saw.out\", to = \"x.in\" },\n\
         { from = \"top.out\", to = \"x.in\" },\n\
         { from = \"lfo.out\", to = \"x.time_ms\", scale = 500 },\n\
         { from = \"lfo.out\", to = \"x.gain\", scale = 2 },\n",
        "out",
    ),
    (
        "x = { module = \"std.fdn_reverb\", size = 2, decay_s = 30, damping_hz = 1000000, mix = 1 }",
        "{ from = \"saw.out\", to = \"x.in_l\" },\n\
         { from = \"top.out\", to = \"x.in_l\" },\n\
         { from = \"top.out\", to = \"x.in_r\" },\n\
         { from = \"lfo.out\", to = \"x.size\", scale = 2 },\n\
         { from = \"lfo.out\", to = \"x.predelay_ms\", scale = 500 },\n",
        "out_l",
    ),
    (
        "m = { module = \"std.fm_op\", ratio = 7, feedback = 1 }\n\
         x = { module = \"std.fm_op\", freq = 20000, ratio = 64, feedback = 1 }",
        "{ from = \"m.out\", to = \"x.pm\", scale = 1000 },\n\
         { from = \"top.out\", to = \"x.pm\" },\n\
         { from = \"lfo.out\", to = \"x.freq\", scale = 20000 },\n\
         { from = \"lfo.out\", to = \"x.feedback\" },\n",
        "out",
    ),
    (
        "x = { module = \"std.wavetable\", freq = 20000, position = 3.5 }",
        "{ from = \"top.out\", to = \"x.pm\" },\n\
         { from = \"lfo.out\", to = \"x.freq\", scale = 100000 },\n\
         { from = \"lfo.out\", to = \"x.position\", scale = 4 },\n",
        "out",
    ),
];

/// At 44.1 and 192 kHz, 1 s each, every case runs without a fault (the
/// engine checks every output value for being finite and within 10⁶), the
/// instance is never muted, every rendered sample is finite, and the output
/// is not silent.
#[test]
fn stays_within_the_bound_through_the_engine() {
    let x: InstanceAddress = "/x".parse().unwrap();
    for rate in [44_100, 192_000] {
        for (instance, connections, output) in CASES {
            let wiring = format!(
                "{connections}{{ from = \"x.{output}\", to = \"out.l\" }},\n\
                 {{ from = \"x.{output}\", to = \"out.r\" }},\n"
            );
            let text = project_text(rate, &format!("{STIMULUS}{instance}"), &wiring);
            let (rendered, engine) = render(&project(&text), u64::from(rate), 256, 1);
            assert_eq!(
                faults(&rendered),
                Vec::<String>::new(),
                "{rate} Hz, {instance}"
            );
            assert_eq!(engine.is_muted(&x), Some(false), "{instance}");
            assert!(rendered.samples.iter().all(|y| y.is_finite()));
            assert!(
                rendered.samples.iter().any(|y| y.abs() > 0.05),
                "{rate} Hz, {instance}: silent"
            );
        }
    }
}

/// The M2.10 review's modulation cases (M1) through the engine at 48 kHz,
/// 1 s each: `std.comb` and `std.allpass_delay` side by side on a ±1 square
/// (`std.osc` shape 2, period 37 frames), their delay alternating 1.5 and 2
/// frames on every frame (`std.frames` mod 2), or swept 20 ± 5 frames at
/// 3 kHz, or 100 ± 20 frames at 2 kHz; `feedback` and `gain` ±0.99 and
/// ±0.999; the comb's damping at 0.49·sr, 16 kHz and 12 kHz. The comb's
/// peak stays within `max|x|/(1 − |fb|)` and the all-pass's within
/// `(1 + |g|)·max|x|/(1 − |g|)` (the rustdoc's bounds, `max|x|` = 1), far
/// below the rail, and nothing faults. The largest peaks are printed.
#[test]
fn modulated_comb_and_allpass_stay_within_their_bounds() {
    let modulations = [
        (
            "1.5/2 frames",
            "count = { module = \"std.frames\" }\n\
             two = { module = \"std.const\", value = 2 }\n\
             alt = { module = \"std.mod\" }\n",
            "{ from = \"count.lo\", to = \"alt.a\" },\n\
             { from = \"two.out\", to = \"alt.b\" },\n",
            "alt.out",
            1.5,
            0.5,
        ),
        (
            "20±5 at 3 kHz",
            "m = { module = \"std.sine_osc\", freq = 3000 }\n",
            "",
            "m.out",
            20.0,
            5.0,
        ),
        (
            "100±20 at 2 kHz",
            "m = { module = \"std.sine_osc\", freq = 2000 }\n",
            "",
            "m.out",
            100.0,
            20.0,
        ),
    ];
    let ms = |frames: f64| frames * 1000.0 / 48_000.0;
    let (mut worst_comb, mut worst_allpass) = (0.0f64, 0.0f64);
    for (name, instances, wiring, source, base, depth) in modulations {
        for fb in [-0.999f64, -0.99, 0.99, 0.999] {
            for damping in [0.49 * 48_000.0, 16_000.0, 12_000.0] {
                let text = project_text(
                    48_000,
                    &format!(
                        "sq = {{ module = \"std.osc\", shape = 2, freq = {} }}\n{instances}\
                         c = {{ module = \"std.comb\", time_ms = {}, feedback = {fb}, damping_hz = {damping} }}\n\
                         a = {{ module = \"std.allpass_delay\", time_ms = {}, gain = {fb} }}",
                        48_000.0 / 37.0,
                        ms(base),
                        ms(base)
                    ),
                    &format!(
                        "{wiring}{{ from = \"sq.out\", to = \"c.in\" }},\n\
                         {{ from = \"sq.out\", to = \"a.in\" }},\n\
                         {{ from = \"{source}\", to = \"c.time_ms\", scale = {} }},\n\
                         {{ from = \"{source}\", to = \"a.time_ms\", scale = {} }},\n\
                         {{ from = \"c.out\", to = \"out.l\" }},\n\
                         {{ from = \"a.out\", to = \"out.r\" }},\n",
                        ms(depth),
                        ms(depth)
                    ),
                );
                let (rendered, _) = render(&project(&text), 48_000, 256, 1);
                assert_eq!(faults(&rendered), Vec::<String>::new(), "{name}");
                // The sink's gain is 0.5.
                let peak = |channel: usize| {
                    rendered
                        .samples
                        .iter()
                        .skip(channel)
                        .step_by(2)
                        .fold(0.0f64, |m, y| m.max(y.abs()))
                        / 0.5
                };
                let (comb, allpass) = (peak(0), peak(1));
                let g = fb.abs();
                assert!(comb > 0.5, "{name}: comb silent");
                assert!(
                    comb <= 1.0 / (1.0 - g),
                    "{name}, fb {fb}, {damping} Hz: comb {comb}"
                );
                assert!(
                    allpass <= (1.0 + g) / (1.0 - g),
                    "{name}, g {fb}: all-pass {allpass}"
                );
                worst_comb = worst_comb.max(comb);
                worst_allpass = worst_allpass.max(allpass);
            }
        }
    }
    eprintln!("largest peaks: comb {worst_comb:.2}, all-pass {worst_allpass:.2}");
}

/// All six in one project, modulated at audio rate and by an LFO, render
/// the same samples and log for k = 1, 64 and 256 on 1, 2, 4 and 8 threads
/// as one frame at a time on one thread.
#[test]
fn bit_identical_across_k_and_threads() {
    let text = project_text(
        48_000,
        "saw = { module = \"std.osc\", shape = 1, freq = 110 }\n\
         audio = { module = \"std.sine_osc\", freq = 700 }\n\
         slow = { module = \"std.lfo\", freq_hz = 3 }\n\
         bq = { module = \"std.biquad\", mode = 0, cutoff_hz = 2000, q = 4 }\n\
         comb = { module = \"std.comb\", time_ms = 4.5, feedback = 0.99 }\n\
         ap = { module = \"std.allpass_delay\", time_ms = 7, gain = 0.6 }\n\
         verb = { module = \"std.fdn_reverb\", size = 0.7, decay_s = 3, mix = 0.5 }\n\
         m = { module = \"std.fm_op\", ratio = 2, feedback = 0.3, level = 0.2 }\n\
         c = { module = \"std.fm_op\", freq = 220 }\n\
         wt = { module = \"std.wavetable\", freq = 330, position = 2 }",
        "{ from = \"saw.out\", to = \"bq.in\" },\n\
         { from = \"audio.out\", to = \"bq.cutoff_hz\", scale = 1500 },\n\
         { from = \"bq.out\", to = \"comb.in\" },\n\
         { from = \"slow.out\", to = \"comb.time_ms\", scale = 0.5 },\n\
         { from = \"comb.out\", to = \"ap.in\" },\n\
         { from = \"m.out\", to = \"c.pm\" },\n\
         { from = \"slow.out\", to = \"wt.position\", scale = 3 },\n\
         { from = \"audio.out\", to = \"wt.freq\", scale = 100 },\n\
         { from = \"ap.out\", to = \"verb.in_l\", scale = 0.2 },\n\
         { from = \"c.out\", to = \"verb.in_r\", scale = 0.3 },\n\
         { from = \"wt.out\", to = \"verb.in_r\", scale = 0.3 },\n\
         { from = \"verb.out_l\", to = \"out.l\" },\n\
         { from = \"verb.out_r\", to = \"out.r\" },\n",
    );
    let project = project(&text);
    let frames = 20_000;
    let (reference, _) = render(&project, frames, 1, 1);
    assert!(faults(&reference).is_empty());
    assert!(reference.samples.iter().any(|y| y.abs() > 0.01));
    for k in [1, 64, 256] {
        for threads in [1, 2, 4, 8] {
            let (rendered, engine) = render(&project, frames, k, threads);
            assert_eq!(rendered, reference, "k = {k}, {threads} threads");
            if threads > 1 {
                assert!(engine.schedule_stats().worker_tasks > 0);
            }
        }
    }
}
