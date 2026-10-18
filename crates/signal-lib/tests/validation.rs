//! M2.10-4 (ADR 0027 §4, "Validation"): every library composite rebuilt from
//! core primitives (`signal_lib::VALIDATION`) rendered against its native
//! reference on the same stimuli, at 48 and 96 kHz, through the engine.
//!
//! Each case is one project: the stimulus feeds the native `ref` and the
//! composite `x` alike; `ref`'s output goes to the sink's left channel and
//! `x`'s to its right, so both run in the same render. The suite prints, per
//! pair, stimulus, rate and output, the largest absolute error, the RMS error,
//! whether the two are bit-exact (every sample's bits equal, frame 0
//! included) and how many frames differ, and asserts the stated tolerance:
//! bit-exact, a maximum absolute error, or (the shaper) a per-frame bound.
//! The tolerances are measurements with headroom, recorded with their reasons
//! in each composite's file and in `docs/benchmarks.md` ("Validation pairs");
//! the numbers inform, they do not judge quality (AGENTS.md rule 6).
//!
//! Also here: determinism across block sizes and thread counts, the range
//! differences (the delay above 99 999 Hz, the attack at 192 kHz, the filters'
//! cutoff cap at 999 999 Hz), the sample-rate domain (std.sample_rate faults
//! above 999 999 Hz) and the input amplitude each composite takes without a
//! fault. Every render asserts every sample within ±1e6 (AC10).

mod common;

use common::{faults, registry};
use signal_engine::{Engine, EngineConfig};
use signal_output::{Render, render_blocks};
use signal_project::{Project, SourceFile};

/// What a case asserts.
#[derive(Clone, Copy, Debug)]
enum Tolerance {
    /// Every sample's bits equal.
    Exact,
    /// The largest absolute error at most this.
    Abs(f64),
    /// The largest absolute error at most this times the input's largest
    /// magnitude over the render (the case's first input, from a second
    /// render): for an error that scales with the signal, as an
    /// interpolation position's rounding does.
    Relative(f64),
    /// `std.tanh_shaper` against the midpoint form: per frame at most
    /// 1e-9 + [`MIDPOINT_CURVATURE`]·Δu², u = drive·in, from the case's
    /// input (a second render).
    Midpoint { drive: f64 },
}

/// One comparison: `x` (the composite) against `ref` (the native type) on
/// the stimulus `stimulus`, compared at `output`.
struct Case {
    pair: &'static str,
    stimulus: &'static str,
    /// Parameters of `ref` and `x` (`key = value, …`), the same names.
    parameters: String,
    /// Parameters of `ref` only (e.g. `std.osc`'s shape).
    native_only: &'static str,
    /// The stimulus's instances; their outputs reach `ref` and `x` through
    /// `inputs`.
    instances: String,
    /// (source port, input or parameter of both, scale).
    inputs: Vec<(&'static str, &'static str, f64)>,
    /// The output of `ref` and of `x` compared.
    output: (&'static str, &'static str),
    seconds: f64,
    tolerance: Tolerance,
}

const RATES: [u32; 2] = [48_000, 96_000];

/// The midpoint rule's error bound for tanh, with a margin for rounding:
/// the mean of f over a step of size h differs from f at the step's
/// midpoint by f″(ξ)·h²/24 for some ξ in the step, and |tanh″| is at most
/// 4/(3√3) ≈ 0.770, so by at most 0.0321·Δu². (The 1e-9 beside it covers the
/// native's own rounding: its ADAA quotient for |Δu| just above 1e-6, and
/// the midpoint it takes below.)
const MIDPOINT_CURVATURE: f64 = 0.035;

/// The stimuli, as instance lines whose output is `sig.out`, for `rate`.
fn noise() -> String {
    "noise = { module = \"std.noise\" }\nsig = { module = \"std.scale\", mul = 0.5 }\n".to_owned()
}

/// A band-limited saw at half level whose frequency rises from 50 Hz to
/// 19.2 kHz (0.4·48 kHz) over `seconds`, at both rates.
fn saw_sweep(seconds: f64) -> String {
    let top = 19_200.0 - 50.0;
    format!(
        "sweep = {{ module = \"std.phasor\", freq = {:?} }}\n\
         hz = {{ module = \"std.scale\", mul = {top:?}, add = 50 }}\n\
         saw = {{ module = \"std.osc\", shape = 1, freq = 0 }}\n\
         sig = {{ module = \"std.scale\", mul = 0.5 }}\n",
        1.0 / seconds
    )
}

/// Unit impulses at frame 0 and every `period` frames.
fn impulses(period: u32) -> String {
    format!(
        "count = {{ module = \"std.frames\" }}\n\
         period = {{ module = \"std.const\", value = {period} }}\n\
         phase = {{ module = \"std.mod\" }}\n\
         sig = {{ module = \"std.compare\", mode = 2 }}\n"
    )
}

/// One unit impulse, at frame 0.
fn first_frame() -> String {
    "first_count = { module = \"std.frames\" }\nsig = { module = \"std.compare\", mode = 2 }\n"
        .to_owned()
}

/// A gate, 1 for the first half of each of `hz`'s periods, as `step.out`.
fn square(hz: f64) -> String {
    format!(
        "step_clock = {{ module = \"std.phasor\", freq = {hz:?} }}\n\
         half = {{ module = \"std.const\", value = 0.5 }}\n\
         step = {{ module = \"std.compare\", mode = 0 }}\n"
    )
}

/// A 1 kHz sine, as `wobble.out`.
fn audio_rate() -> String {
    "wobble = { module = \"std.sine_osc\", freq = 1000 }\n".to_owned()
}

/// The internal wiring of the stimuli named in `instances`.
fn stimulus_wiring(instances: &str) -> String {
    let mut lines = String::new();
    if instances.contains("noise =") {
        lines.push_str("{ from = \"noise.out\", to = \"sig.in\" },\n");
    }
    if instances.contains("sweep =") {
        lines.push_str(
            "{ from = \"sweep.phase\", to = \"hz.in\" },\n\
             { from = \"hz.out\", to = \"saw.freq\" },\n\
             { from = \"saw.out\", to = \"sig.in\" },\n",
        );
    }
    if instances.contains("period =") {
        lines.push_str(
            "{ from = \"count.lo\", to = \"phase.a\" },\n\
             { from = \"period.out\", to = \"phase.b\" },\n\
             { from = \"phase.out\", to = \"sig.a\" },\n",
        );
    }
    if instances.contains("first_count =") {
        lines.push_str("{ from = \"first_count.lo\", to = \"sig.a\" },\n");
    }
    if instances.contains("clock = { module = \"std.phasor\", freq = 7 }") {
        lines.push_str(
            "{ from = \"clock.wrap\", to = \"ref.trigger\" },\n\
             { from = \"clock.wrap\", to = \"x.trigger\" },\n",
        );
    }
    if instances.contains("step_clock =") {
        lines.push_str(
            "{ from = \"step_clock.phase\", to = \"step.a\" },\n\
             { from = \"half.out\", to = \"step.b\" },\n",
        );
    }
    lines
}

/// The project of `case` at `rate`, with `left` and `right` to the sink.
fn project(case: &Case, rate: u32, left: &str, right: &str) -> Project {
    let comma = |p: &str| {
        if p.is_empty() {
            String::new()
        } else {
            format!(", {p}")
        }
    };
    let mut wiring = stimulus_wiring(&case.instances);
    for (from, to, scale) in &case.inputs {
        for target in ["ref", "x"] {
            wiring.push_str(&format!(
                "{{ from = \"{from}\", to = \"{target}.{to}\", scale = {scale:?} }},\n"
            ));
        }
    }
    wiring.push_str(&format!(
        "{{ from = \"{left}\", to = \"out.l\" }},\n{{ from = \"{right}\", to = \"out.r\" }},\n"
    ));
    let native = signal_lib::VALIDATION
        .iter()
        .find(|p| p.composite == case.pair)
        .unwrap()
        .native;
    let text = format!(
        "schema_version = 1\n\n[project]\nname = \"validation\"\nsample_rate = {rate}\nseed = 1\n\n\
         [instances]\nref = {{ module = \"{native}\"{}{} }}\nx = {{ module = \"lib.{}\"{} }}\n{}\
         out = {{ module = \"std.audio_out\", gain = 1 }}\n\n[wiring]\nconnections = [\n{wiring}]\n",
        comma(&case.parameters),
        comma(case.native_only),
        case.pair,
        comma(&case.parameters),
        case.instances,
    );
    Project::from_sources(vec![SourceFile {
        path: "project.toml".to_owned(),
        text,
    }])
    .unwrap_or_else(|d| panic!("{} / {}: {d:?}", case.pair, case.stimulus))
}

/// Renders `project` for `frames` in blocks of `block` on `threads`.
fn render(project: &Project, frames: u64, block: usize, threads: usize) -> Render {
    let registry = registry();
    let plan = project
        .plan(&registry)
        .unwrap_or_else(|d| panic!("{d:?}"))
        .plan;
    let mut config = EngineConfig::new(project.sample_rate, project.seed);
    config.threads = threads;
    let mut engine = Engine::new(&plan, &registry, config, Vec::new()).unwrap();
    render_blocks(&mut engine, frames, block, |_| Ok(None)).unwrap()
}

/// The left and right channels.
fn channels(render: &Render) -> (Vec<f64>, Vec<f64>) {
    let left = render.samples.iter().step_by(2).copied().collect();
    let right = render.samples.iter().skip(1).step_by(2).copied().collect();
    (left, right)
}

/// Every sample finite and within ±1e6.
fn assert_bounded(render: &Render, what: &str) {
    for s in &render.samples {
        assert!(s.is_finite() && s.abs() <= 1e6, "{what}: sample {s}");
    }
}

/// The comparison of a native and a composite channel.
struct Errors {
    max: f64,
    rms: f64,
    differing: usize,
}

fn errors(native: &[f64], composite: &[f64]) -> Errors {
    let mut sum = 0.0;
    let mut max = 0.0f64;
    let mut differing = 0;
    for (a, b) in native.iter().zip(composite) {
        if a.to_bits() != b.to_bits() {
            differing += 1;
        }
        let e = (a - b).abs();
        max = max.max(e);
        sum += e * e;
    }
    Errors {
        max,
        rms: (sum / native.len().max(1) as f64).sqrt(),
        differing,
    }
}

/// Runs `case` at both rates: renders, reports and asserts.
fn run(case: &Case) {
    for rate in RATES {
        run_at(case, rate);
    }
}

/// Runs `case` at `rate`: renders, reports and asserts.
fn run_at(case: &Case, rate: u32) {
    let frames = (case.seconds * f64::from(rate)).round() as u64;
    let (native_port, composite_port) = case.output;
    let left = format!("ref.{native_port}");
    let right = format!("x.{composite_port}");
    let rendered = render(&project(case, rate, &left, &right), frames, 64, 1);
    let what = format!(
        "{} / {} / {} Hz / {composite_port}",
        case.pair, case.stimulus, rate
    );
    assert_eq!(faults(&rendered), Vec::<String>::new(), "{what}");
    assert_bounded(&rendered, &what);
    let (native, composite) = channels(&rendered);
    let e = errors(&native, &composite);
    println!(
        "VALIDATION {what}: max {:.3e}, rms {:.3e}, bit-exact {}, {} of {} frames differ",
        e.max,
        e.rms,
        e.differing == 0,
        e.differing,
        native.len()
    );
    match case.tolerance {
        Tolerance::Exact => assert_eq!(e.differing, 0, "{what}: not bit-exact"),
        Tolerance::Abs(bound) => assert!(e.max <= bound, "{what}: {} > {bound}", e.max),
        Tolerance::Relative(factor) => {
            let peak = input_of(case, rate, frames)
                .iter()
                .fold(0.0f64, |m, x| m.max(x.abs()));
            let bound = factor * peak;
            println!(
                "VALIDATION {what}: input peak {peak:.3e}, error/peak {:.3e}",
                e.max / peak.max(f64::MIN_POSITIVE)
            );
            assert!(e.max <= bound, "{what}: {} > {factor:e}·{peak}", e.max);
        }
        Tolerance::Midpoint { drive } => {
            let x = input_of(case, rate, frames);
            let mut previous = 0.0;
            for (n, ((a, b), x)) in native.iter().zip(&composite).zip(&x).enumerate() {
                let u = drive * x;
                let step = u - previous;
                let bound = 1e-9 + MIDPOINT_CURVATURE * step * step;
                assert!((a - b).abs() <= bound, "{what}, frame {n}: {a} vs {b}");
                previous = u;
            }
        }
    }
}

/// The input `case` feeds its pair (its first input, scale applied), as
/// rendered from the same stimulus at `rate`.
fn input_of(case: &Case, rate: u32, frames: u64) -> Vec<f64> {
    let (from, _, scale) = case.inputs[0];
    let input = render(&project(case, rate, from, from), frames, 64, 1);
    channels(&input).0.iter().map(|x| scale * x).collect()
}

fn case(
    pair: &'static str,
    stimulus: &'static str,
    parameters: &str,
    instances: String,
    inputs: Vec<(&'static str, &'static str, f64)>,
    output: (&'static str, &'static str),
    tolerance: Tolerance,
) -> Case {
    Case {
        pair,
        stimulus,
        parameters: parameters.to_owned(),
        native_only: "",
        instances,
        inputs,
        output,
        seconds: 0.25,
        tolerance,
    }
}

/// The filters: noise at a fixed cutoff, a saw sweep, impulses, steps of
/// cutoff (and q) and a cutoff modulated at audio rate, every output.
fn filter_cases(pair: &'static str, outputs: &[&'static str], q: bool) -> Vec<Case> {
    let mut cases = Vec::new();
    for &output in outputs {
        let o = (output, output);
        let resonant = if q {
            "cutoff_hz = 2000, q = 5"
        } else {
            "cutoff_hz = 2000"
        };
        let sharp = if q {
            "cutoff_hz = 5000, q = 10"
        } else {
            "cutoff_hz = 5000"
        };
        let stepped = if q {
            "cutoff_hz = 300, q = 0.7"
        } else {
            "cutoff_hz = 300"
        };
        cases.push(case(
            pair,
            "noise",
            "cutoff_hz = 1000",
            noise(),
            vec![("sig.out", "in", 1.0)],
            o,
            Tolerance::Exact,
        ));
        cases.push(case(
            pair,
            "saw sweep",
            resonant,
            saw_sweep(0.25),
            vec![("sig.out", "in", 1.0)],
            o,
            Tolerance::Exact,
        ));
        cases.push(case(
            pair,
            "impulses",
            sharp,
            impulses(1000),
            vec![("sig.out", "in", 1.0)],
            o,
            Tolerance::Exact,
        ));
        let mut steps = vec![("sig.out", "in", 1.0), ("step.out", "cutoff_hz", 2700.0)];
        if q {
            steps.push(("step.out", "q", 4.0));
        }
        cases.push(case(
            pair,
            "parameter steps",
            stepped,
            noise() + &square(20.0),
            steps,
            o,
            Tolerance::Exact,
        ));
        cases.push(case(
            pair,
            "audio-rate cutoff",
            "cutoff_hz = 2000",
            noise() + &audio_rate(),
            vec![("sig.out", "in", 1.0), ("wobble.out", "cutoff_hz", 1500.0)],
            o,
            Tolerance::Exact,
        ));
    }
    cases
}

/// `lib.svf_core` against `std.svf`: bit-exact, fixed and modulated cutoff.
#[test]
fn svf_core_is_bit_exact_with_std_svf() {
    for c in filter_cases("svf_core", &["lp", "bp", "hp"], true) {
        run(&c);
    }
}

/// `lib.onepole_core` against `std.onepole`: bit-exact.
#[test]
fn onepole_core_is_bit_exact_with_std_onepole() {
    for c in filter_cases("onepole_core", &["lp", "hp"], false) {
        run(&c);
    }
}

/// The lag's stated tolerance: the rounding order of its coefficient and a
/// flush that can fall a frame apart (lag_core.toml).
const LAG_TOLERANCE: f64 = 1e-9;

/// `lib.lag_core` against `std.lag`, within 1e-9.
#[test]
fn lag_core_is_within_its_tolerance_of_std_lag() {
    for c in lag_cases() {
        run(&c);
    }
}

/// The lag's comparisons.
fn lag_cases() -> Vec<Case> {
    let t = Tolerance::Abs(LAG_TOLERANCE);
    let o = ("out", "out");
    let i = vec![("sig.out", "in", 1.0)];
    vec![
        case(
            "lag_core",
            "noise",
            "rise_ms = 5, fall_ms = 50",
            noise(),
            i.clone(),
            o,
            t,
        ),
        case(
            "lag_core",
            "saw sweep",
            "rise_ms = 1, fall_ms = 3",
            saw_sweep(0.25),
            i.clone(),
            o,
            t,
        ),
        case(
            "lag_core",
            "impulses",
            "rise_ms = 0.5, fall_ms = 20",
            impulses(2000),
            i.clone(),
            o,
            t,
        ),
        case(
            "lag_core",
            "square input",
            "rise_ms = 10, fall_ms = 30",
            square(10.0),
            vec![("step.out", "in", 1.0)],
            o,
            t,
        ),
        case(
            "lag_core",
            "parameter steps",
            "rise_ms = 2, fall_ms = 2",
            noise() + &square(20.0),
            vec![
                ("sig.out", "in", 1.0),
                ("step.out", "rise_ms", 30.0),
                ("step.out", "fall_ms", 7.0),
            ],
            o,
            t,
        ),
        case(
            "lag_core",
            "audio-rate time",
            "rise_ms = 10, fall_ms = 10",
            noise() + &audio_rate(),
            vec![("sig.out", "in", 1.0), ("wobble.out", "rise_ms", 8.0)],
            o,
            t,
        ),
        case(
            "lag_core",
            "zero time",
            "rise_ms = 0, fall_ms = 0",
            noise(),
            i,
            o,
            t,
        ),
    ]
}

/// The envelope's stated tolerance: the flush threshold, by which a stage
/// change can fall a frame apart (adsr_core.toml).
const ADSR_TOLERANCE: f64 = 1e-9;

/// `lib.adsr_core` against `std.adsr`, within 1e-9: gates (a square, noise,
/// one-frame impulses, a saw sweep), triggers, parameter steps and a zero
/// attack. Audio-
/// rate modulation is not meaningful for its millisecond parameters.
#[test]
fn adsr_core_is_within_its_tolerance_of_std_adsr() {
    for c in adsr_cases() {
        run(&c);
    }
}

/// The envelope's comparisons.
fn adsr_cases() -> Vec<Case> {
    let t = Tolerance::Abs(ADSR_TOLERANCE);
    let o = ("out", "out");
    let triggers = "clock = { module = \"std.phasor\", freq = 7 }\n";
    let mut cases = vec![
        case(
            "adsr_core",
            "square gate",
            "attack_ms = 5, decay_ms = 30, sustain = 0.6, release_ms = 20",
            square(6.0),
            vec![("step.out", "gate", 1.0)],
            o,
            t,
        ),
        case(
            "adsr_core",
            "noise gate",
            "attack_ms = 1, decay_ms = 5, sustain = 0.4, release_ms = 3",
            noise(),
            vec![("sig.out", "gate", 2.0)],
            o,
            t,
        ),
        case(
            "adsr_core",
            "impulse gate",
            "attack_ms = 0.5, decay_ms = 2, sustain = 0.5, release_ms = 4",
            impulses(1500),
            vec![("sig.out", "gate", 1.0)],
            o,
            t,
        ),
        // The saw sweep as a gate: on above 0.5, ever faster.
        case(
            "adsr_core",
            "saw sweep gate",
            "attack_ms = 0.2, decay_ms = 1, sustain = 0.6, release_ms = 0.5",
            saw_sweep(0.25),
            vec![("sig.out", "gate", 2.0)],
            o,
            t,
        ),
        case(
            "adsr_core",
            "parameter steps",
            "attack_ms = 3, decay_ms = 20, sustain = 0.3, release_ms = 15",
            square(4.0) + "wobble = { module = \"std.phasor\", freq = 23 }\n",
            vec![
                ("step.out", "gate", 1.0),
                ("wobble.phase", "sustain", 0.5),
                ("wobble.phase", "decay_ms", 40.0),
                ("wobble.phase", "release_ms", 25.0),
            ],
            o,
            t,
        ),
        case(
            "adsr_core",
            "zero attack",
            "attack_ms = 0, decay_ms = 10, sustain = 1, release_ms = 0.1",
            square(9.0),
            vec![("step.out", "gate", 1.0)],
            o,
            t,
        ),
    ];
    // Triggers (the 7 Hz clock's wraps, wired by `stimulus_wiring`) while
    // the gate is on restart the attack.
    let mut retrigger = case(
        "adsr_core",
        "gate and triggers",
        "attack_ms = 8, decay_ms = 25, sustain = 0.5, release_ms = 40",
        square(3.0) + triggers,
        vec![("step.out", "gate", 1.0)],
        o,
        t,
    );
    retrigger.seconds = 0.5;
    cases.push(retrigger);
    cases
}

/// `lib.osc_core` against `std.osc` at shape 0 (sine): bit-exact, at fixed
/// and swept frequency, with impulses, noise and audio-rate phase
/// modulation, and frequency steps.
#[test]
fn osc_core_sine_is_bit_exact_with_std_osc() {
    for c in osc_cases() {
        run(&c);
    }
}

/// The oscillator's comparisons (its sine against `std.osc` at shape 0).
fn osc_cases() -> Vec<Case> {
    let o = ("out", "out");
    let mut cases = vec![
        case(
            "osc_core",
            "fixed 440 Hz",
            "freq = 440",
            String::new(),
            vec![],
            o,
            Tolerance::Exact,
        ),
        case(
            "osc_core",
            "noise pm",
            "freq = 330",
            noise(),
            vec![("sig.out", "pm", 0.2)],
            o,
            Tolerance::Exact,
        ),
        case(
            "osc_core",
            "saw sweep freq",
            "freq = 0",
            "glide = { module = \"std.phasor\", freq = 4 }\n".to_owned(),
            vec![("glide.phase", "freq", 15_000.0)],
            o,
            Tolerance::Exact,
        ),
        case(
            "osc_core",
            "impulses pm",
            "freq = 1000",
            impulses(700),
            vec![("sig.out", "pm", 0.25)],
            o,
            Tolerance::Exact,
        ),
        case(
            "osc_core",
            "freq steps",
            "freq = 220",
            square(10.0),
            vec![("step.out", "freq", 660.0)],
            o,
            Tolerance::Exact,
        ),
        case(
            "osc_core",
            "audio-rate pm",
            "freq = 220",
            audio_rate(),
            vec![("wobble.out", "pm", 0.3)],
            o,
            Tolerance::Exact,
        ),
    ];
    for c in &mut cases {
        c.native_only = "shape = 0";
    }
    cases
}

/// The expected difference of the naive saw: without polyBLEP, `saw`
/// differs from `std.osc` at shape 1 only on frames whose phase is within
/// one step Δ of the wrap, by the residual (at most 1), and equals it
/// elsewhere bit for bit.
#[test]
fn osc_core_saw_differs_from_std_osc_only_by_the_polyblep_residual() {
    for rate in RATES {
        for freq in [440.0, 2003.0] {
            let mut c = case(
                "osc_core",
                "saw against shape 1",
                &format!("freq = {freq:?}"),
                String::new(),
                vec![],
                ("out", "saw"),
                Tolerance::Abs(1.0),
            );
            c.native_only = "shape = 1";
            let frames = (0.25 * f64::from(rate)) as u64;
            let rendered = render(&project(&c, rate, "ref.out", "x.saw"), frames, 64, 1);
            assert_eq!(faults(&rendered), Vec::<String>::new());
            let (native, composite) = channels(&rendered);
            let e = errors(&native, &composite);
            println!(
                "VALIDATION osc_core / saw {freq} Hz against shape 1 / {rate} Hz / saw: max {:.3e}, rms {:.3e}, bit-exact {}, {} of {} frames differ",
                e.max,
                e.rms,
                e.differing == 0,
                e.differing,
                native.len()
            );
            let dt = freq / f64::from(rate);
            let mut phase = 0.0f64;
            for (n, (a, b)) in native.iter().zip(&composite).enumerate() {
                let near_wrap = phase < dt || phase > 1.0 - dt;
                if near_wrap {
                    assert!((a - b).abs() <= 1.0 + 1e-12, "frame {n}: {a} vs {b}");
                } else {
                    assert_eq!(a.to_bits(), b.to_bits(), "frame {n}: {a} vs {b}");
                }
                let next = phase + dt;
                phase = next - next.floor();
            }
            assert!(e.differing > 0);
        }
    }
}

/// The delay's stated tolerance for fractional and modulated delays, as a
/// factor of the input's largest magnitude: the interpolation position's
/// rounding (delay_core.toml) moves the read by about an ulp of the frame
/// count (up to 10⁶, so about 1e-10 of a frame), and the error is that
/// times the interpolated signal's slope, which scales with the input.
/// Measured at most 1.7e-11·peak on short delays and 3.4e-10·peak with the
/// frame count past 10⁶ and a delay of 959 940 frames (review of M2.10-4).
const DELAY_FRACTIONAL_TOLERANCE: f64 = 1e-9;

/// `lib.delay_core` against `std.delay`: bit-exact at whole-frame delays from
/// frame 0 (an impulse at frame 0 included), within 1e-9 at fractional and
/// audio-rate modulated delays.
#[test]
fn delay_core_is_bit_exact_at_whole_frame_delays() {
    for c in delay_cases() {
        run(&c);
    }
}

/// The delay's comparisons (the long delay and the documented difference
/// between 1 and 2 frames have tests of their own, below).
fn delay_cases() -> Vec<Case> {
    let o = ("out", "out");
    let i = vec![("sig.out", "in", 1.0)];
    let fractional = Tolerance::Relative(DELAY_FRACTIONAL_TOLERANCE);
    let mut cases = vec![
        // 250 ms and 10 ms are whole frames at 48 and 96 kHz.
        case(
            "delay_core",
            "noise, 250 ms",
            "time_ms = 250",
            noise(),
            i.clone(),
            o,
            Tolerance::Exact,
        ),
        case(
            "delay_core",
            "impulses, 10 ms",
            "time_ms = 10",
            impulses(1000),
            i.clone(),
            o,
            Tolerance::Exact,
        ),
        case(
            "delay_core",
            "saw sweep, 5 ms",
            "time_ms = 5",
            saw_sweep(0.5),
            i.clone(),
            o,
            Tolerance::Exact,
        ),
        case(
            "delay_core",
            "time steps 100/250 ms",
            "time_ms = 100",
            noise() + &square(5.0),
            vec![("sig.out", "in", 1.0), ("step.out", "time_ms", 150.0)],
            o,
            Tolerance::Exact,
        ),
        case(
            "delay_core",
            "noise, 10.01 ms",
            "time_ms = 10.01",
            noise(),
            i.clone(),
            o,
            fractional,
        ),
        case(
            "delay_core",
            "audio-rate time",
            "time_ms = 5",
            noise() + &audio_rate(),
            vec![("sig.out", "in", 1.0), ("wobble.out", "time_ms", 1.0)],
            o,
            fractional,
        ),
        // A large input: the error scales with it (noise at ±100).
        case(
            "delay_core",
            "noise at ±100, 10.01 ms",
            "time_ms = 10.01",
            noise(),
            vec![("sig.out", "in", 200.0)],
            o,
            fractional,
        ),
    ];
    for c in &mut cases {
        c.seconds = 0.5;
    }
    cases
}

/// The fractional delay at its longest and with the frame count past 10⁶
/// (where `std.frames`' `lo` and the buffer's positions wrap): 9999.37 ms
/// at 96 kHz, 959 939.52 frames, noise at ±1, 1 050 000 frames, within
/// 1e-9 of the input's peak.
#[test]
fn delay_core_long_fractional_delay_past_a_million_frames() {
    let mut c = case(
        "delay_core",
        "noise at ±1, 9999.37 ms, past 10⁶ frames",
        "time_ms = 9999.37",
        noise(),
        vec![("sig.out", "in", 2.0)],
        ("out", "out"),
        Tolerance::Relative(DELAY_FRACTIONAL_TOLERANCE),
    );
    c.seconds = 1_050_000.0 / 96_000.0;
    run_at(&c, 96_000);
}

/// The documented difference for delays between 1 and 2 frames: the native
/// replaces its newest Hermite point by the frame before (a causal
/// fallback); the composite reads the current input. Measured, and bounded
/// by the size of the input (noise at ±0.5).
#[test]
fn delay_core_differs_between_one_and_two_frames_as_documented() {
    for rate in RATES {
        // 1.5 frames.
        let ms = 1.5 * 1000.0 / f64::from(rate);
        let c = case(
            "delay_core",
            "noise, 1.5 frames",
            &format!("time_ms = {ms:?}"),
            noise(),
            vec![("sig.out", "in", 1.0)],
            ("out", "out"),
            Tolerance::Abs(1.0),
        );
        let frames = (0.25 * f64::from(rate)) as u64;
        let rendered = render(&project(&c, rate, "ref.out", "x.out"), frames, 64, 1);
        assert_eq!(faults(&rendered), Vec::<String>::new());
        let (native, composite) = channels(&rendered);
        let e = errors(&native, &composite);
        println!(
            "VALIDATION delay_core / noise, 1.5 frames (documented difference) / {rate} Hz / out: max {:.3e}, rms {:.3e}, bit-exact {}, {} of {} frames differ",
            e.max,
            e.rms,
            e.differing == 0,
            e.differing,
            native.len()
        );
        assert!(e.max <= 1.0 && e.differing > 0);
    }
}

/// `lib.tanh_shaper_core` against `std.tanh_shaper` at low drive: per frame
/// within 1e-9 + 0.035·Δu² (the midpoint rule's curvature term,
/// [`MIDPOINT_CURVATURE`]). Only low drive is a meaningful comparison: the
/// error grows with Δu², so at drive 5 or 20 on noise it is of the order of
/// the output itself (a review probe measured 0.24 and 0.73).
#[test]
fn tanh_shaper_core_is_within_the_midpoint_bound() {
    for c in shaper_cases() {
        run(&c);
    }
}

/// The shaper's comparisons, at low drive.
fn shaper_cases() -> Vec<Case> {
    let o = ("out", "out");
    let mut cases = Vec::new();
    for drive in [0.1, 0.5, 1.0] {
        let p = format!("drive = {drive:?}");
        cases.extend([
            case(
                "tanh_shaper_core",
                "noise",
                &p,
                noise(),
                vec![("sig.out", "in", 1.0)],
                o,
                Tolerance::Midpoint { drive },
            ),
            case(
                "tanh_shaper_core",
                "saw sweep",
                &p,
                saw_sweep(0.25),
                vec![("sig.out", "in", 1.0)],
                o,
                Tolerance::Midpoint { drive },
            ),
            case(
                "tanh_shaper_core",
                "impulses",
                &p,
                impulses(500),
                vec![("sig.out", "in", 1.0)],
                o,
                Tolerance::Midpoint { drive },
            ),
            case(
                "tanh_shaper_core",
                "1 kHz sine",
                &p,
                audio_rate(),
                vec![("wobble.out", "in", 1.0)],
                o,
                Tolerance::Midpoint { drive },
            ),
        ]);
    }
    // Drive steps between 0.1 and 1: the midpoint bound at its largest,
    // |Δu| ≤ 1 (drive at most 1, noise at ±0.5), so 0.035 + 1e-9.
    cases.push(case(
        "tanh_shaper_core",
        "drive steps",
        "drive = 0.1",
        noise() + &square(20.0),
        vec![("sig.out", "in", 1.0), ("step.out", "drive", 0.9)],
        o,
        Tolerance::Abs(MIDPOINT_CURVATURE + 1e-9),
    ));
    cases
}

/// The comparisons of the pair whose composite is `composite`; empty for a
/// composite with none, which `every_validation_pair_is_compared` refuses.
fn comparisons(composite: &str) -> Vec<Case> {
    match composite {
        "svf_core" => filter_cases("svf_core", &["lp", "bp", "hp"], true),
        "onepole_core" => filter_cases("onepole_core", &["lp", "hp"], false),
        "lag_core" => lag_cases(),
        "adsr_core" => adsr_cases(),
        "osc_core" => osc_cases(),
        "delay_core" => delay_cases(),
        "tanh_shaper_core" => shaper_cases(),
        _ => Vec::new(),
    }
}

/// M2.10-4 review: every pair of `signal_lib::VALIDATION` has comparison
/// cases, each of that pair and against a stated tolerance, and together
/// they compare every output the pair lists (so a new pair cannot leave the
/// library demo's rule without being compared here).
#[test]
fn every_validation_pair_is_compared() {
    for pair in &signal_lib::VALIDATION {
        let cases = comparisons(pair.composite);
        assert!(
            !cases.is_empty(),
            "lib.{} has no comparison with {}: add its cases to `comparisons`",
            pair.composite,
            pair.native
        );
        assert!(cases.iter().all(|c| c.pair == pair.composite));
        for output in pair.outputs {
            assert!(
                cases.iter().any(|c| c.output.1 == *output),
                "lib.{}: output {output} is never compared",
                pair.composite
            );
        }
    }
}

/// AC4: one case per pair renders the same audio and log at k = 1, 64 and
/// 256 on 1 and 4 threads.
#[test]
fn the_validation_renders_are_deterministic_across_k_and_threads() {
    let cases = [
        filter_cases("svf_core", &["lp"], true).remove(4),
        filter_cases("onepole_core", &["lp"], false).remove(3),
        case(
            "lag_core",
            "noise",
            "rise_ms = 5, fall_ms = 50",
            noise(),
            vec![("sig.out", "in", 1.0)],
            ("out", "out"),
            Tolerance::Exact,
        ),
        case(
            "adsr_core",
            "square gate",
            "attack_ms = 5, decay_ms = 30, sustain = 0.6, release_ms = 20",
            square(6.0),
            vec![("step.out", "gate", 1.0)],
            ("out", "out"),
            Tolerance::Exact,
        ),
        case(
            "osc_core",
            "audio-rate pm",
            "freq = 220",
            audio_rate(),
            vec![("wobble.out", "pm", 0.3)],
            ("out", "out"),
            Tolerance::Exact,
        ),
        case(
            "delay_core",
            "audio-rate time",
            "time_ms = 5",
            noise() + &audio_rate(),
            vec![("sig.out", "in", 1.0), ("wobble.out", "time_ms", 1.0)],
            ("out", "out"),
            Tolerance::Exact,
        ),
        case(
            "tanh_shaper_core",
            "noise",
            "drive = 1",
            noise(),
            vec![("sig.out", "in", 1.0)],
            ("out", "out"),
            Tolerance::Exact,
        ),
    ];
    for c in &cases {
        let (native, composite) = c.output;
        let p = project(
            c,
            48_000,
            &format!("ref.{native}"),
            &format!("x.{composite}"),
        );
        let frames = 9_600;
        let base = render(&p, frames, 1, 1);
        for block in [1, 64, 256] {
            for threads in [1, 4] {
                let other = render(&p, frames, block, threads);
                let same = base.samples.len() == other.samples.len()
                    && base
                        .samples
                        .iter()
                        .zip(&other.samples)
                        .all(|(a, b)| a.to_bits() == b.to_bits());
                assert!(
                    same,
                    "{}: audio differs at k = {block}, {threads} threads",
                    c.pair
                );
                assert_eq!(
                    base.log, other.log,
                    "{}: log differs at k = {block}, {threads} threads",
                    c.pair
                );
            }
        }
    }
}

/// Range difference: above 99 999 Hz the delay holds at most 999 990 frames
/// (positions are signals, the frame count wraps at 10⁶). At 192 kHz and
/// 6000 ms an impulse at frame 0 comes out after exactly 999 990 frames,
/// where `std.delay` holds it for 1 152 000.
#[test]
fn delay_core_holds_at_most_999_990_frames_above_99_999_hz() {
    let c = case(
        "delay_core",
        "impulse, 6000 ms",
        "time_ms = 6000",
        first_frame(),
        vec![("sig.out", "in", 1.0)],
        ("out", "out"),
        Tolerance::Exact,
    );
    let rendered = render(&project(&c, 192_000, "ref.out", "x.out"), 999_992, 256, 1);
    assert_eq!(faults(&rendered), Vec::<String>::new());
    assert_bounded(&rendered, "delay at 192 kHz");
    let (native, composite) = channels(&rendered);
    assert!(native.iter().all(|s| *s == 0.0));
    for (n, s) in composite.iter().enumerate() {
        let expected = if n == 999_990 { 1.0 } else { 0.0 };
        assert_eq!(*s, expected, "frame {n}");
    }
}

/// The attack length rounds as the native's, ⌊(attack·sr)/1000 + ½⌋, at the
/// rounding boundaries too (review of M2.10-4, auditor-1): attacks whose
/// frame count is a half-integer, and one ulp either side, at both rates,
/// and the reviewer's 0.6770833333333333 ms at 48 kHz (32 frames natively;
/// an attack computed as attack·(sr/1000) gave 33 and an error of 0.03).
/// A sustained gate, through the attack and into the decay, within 1e-9.
#[test]
fn adsr_core_attack_length_rounds_as_the_native() {
    let o = ("out", "out");
    for rate in RATES {
        let mut attacks = vec![0.6770833333333333];
        for frames in [0.5, 1.5, 31.5, 32.5, 100.5, 1000.5] {
            let ms = frames * 1000.0 / f64::from(rate);
            attacks.extend([
                f64::from_bits(ms.to_bits() - 1),
                ms,
                f64::from_bits(ms.to_bits() + 1),
            ]);
        }
        for attack in attacks {
            let c = case(
                "adsr_core",
                "attack rounding",
                &format!("attack_ms = {attack:?}, decay_ms = 1, sustain = 0.5"),
                "on = { module = \"std.const\", value = 1 }\n".to_owned(),
                vec![("on.out", "gate", 1.0)],
                o,
                Tolerance::Abs(ADSR_TOLERANCE),
            );
            let frames = (attack * f64::from(rate) / 1000.0) as u64 + 200;
            let rendered = render(&project(&c, rate, "ref.out", "x.out"), frames, 64, 1);
            assert_eq!(faults(&rendered), Vec::<String>::new(), "{attack} ms");
            let (native, composite) = channels(&rendered);
            let e = errors(&native, &composite);
            println!(
                "VALIDATION adsr_core / attack {attack:?} ms / {rate} Hz / out: max {:.3e}, rms {:.3e}, bit-exact {}, {} of {} frames differ",
                e.max,
                e.rms,
                e.differing == 0,
                e.differing,
                native.len()
            );
            assert!(
                e.max <= ADSR_TOLERANCE,
                "attack {attack} ms at {rate} Hz: {}",
                e.max
            );
        }
    }
}

/// Range difference: the attack is at most 999 999 frames. At 192 kHz an
/// attack of 6000 ms (1 152 000 frames natively) rises as i/999 999.
#[test]
fn adsr_core_attack_is_at_most_999_999_frames() {
    let c = case(
        "adsr_core",
        "gate on, attack 6000 ms",
        "attack_ms = 6000",
        "on = { module = \"std.const\", value = 1 }\n".to_owned(),
        vec![("on.out", "gate", 1.0)],
        ("out", "out"),
        Tolerance::Exact,
    );
    let rendered = render(&project(&c, 192_000, "ref.out", "x.out"), 2000, 64, 1);
    assert_eq!(faults(&rendered), Vec::<String>::new());
    let (native, composite) = channels(&rendered);
    for i in 0..2000 {
        assert_eq!(native[i], i as f64 / 1_152_000.0, "native, frame {i}");
        assert_eq!(composite[i], i as f64 / 999_999.0, "composite, frame {i}");
    }
}

/// Range difference: above 649 610 Hz the filters' cutoff is capped at
/// 318 309 Hz (so π·fc stays within ±1e6). At 999 999 Hz with the cutoff at
/// its maximum neither faults; the difference from the native is measured.
#[test]
fn the_filters_cutoff_cap_acts_only_above_649_610_hz() {
    for (pair, outputs) in [
        ("svf_core", &["lp", "bp", "hp"][..]),
        ("onepole_core", &["lp", "hp"][..]),
    ] {
        for &output in outputs {
            let c = case(
                pair,
                "noise, cutoff 1e6",
                "cutoff_hz = 1000000",
                noise(),
                vec![("sig.out", "in", 1.0)],
                (output, output),
                Tolerance::Abs(2.0),
            );
            let rendered = render(
                &project(
                    &c,
                    999_999,
                    &format!("ref.{output}"),
                    &format!("x.{output}"),
                ),
                4800,
                64,
                1,
            );
            assert_eq!(faults(&rendered), Vec::<String>::new(), "{pair}");
            assert_bounded(&rendered, pair);
            let (native, composite) = channels(&rendered);
            let e = errors(&native, &composite);
            println!(
                "VALIDATION {pair} / noise, cutoff at its maximum (cap 318 309 Hz against 489 999.5 Hz) / 999999 Hz / {output}: max {:.3e}, rms {:.3e}, bit-exact {}, {} of {} frames differ",
                e.max,
                e.rms,
                e.differing == 0,
                e.differing,
                native.len()
            );
        }
    }
}

/// The sample-rate domain: above 999 999 Hz `std.sample_rate`'s own output
/// exceeds ±1e6, so the composites that read the rate fault there (a
/// magnitude fault, muted) and every sample stays within ±1e6.
#[test]
fn above_999_999_hz_the_rate_faults_and_samples_stay_bounded() {
    for pair in signal_lib::VALIDATION
        .iter()
        .filter(|p| p.composite != "tanh_shaper_core")
    {
        let port = pair.outputs[0];
        let input = match pair.composite {
            "adsr_core" => "gate",
            "osc_core" => "pm",
            _ => "in",
        };
        let c = case(
            pair.composite,
            "noise",
            "",
            noise(),
            vec![("sig.out", input, 1.0)],
            (port, port),
            Tolerance::Exact,
        );
        let rendered = render(
            &project(&c, 1_000_001, &format!("ref.{port}"), &format!("x.{port}")),
            256,
            64,
            1,
        );
        let faults = faults(&rendered);
        assert!(
            faults
                .iter()
                .any(|f| f.contains("\"magnitude\"") && f.contains("rate")),
            "{}: {faults:?}",
            pair.composite
        );
        assert_bounded(&rendered, pair.composite);
    }
}

/// The input amplitude each composite takes without a fault, on noise
/// scaled by 1, 10², 10⁴ and 10⁶ at its defaults: reported, and every sample
/// stays within ±1e6 with only magnitude faults. Where neither side faults,
/// the outputs are also compared (review of M2.10-4): the bit-exact pairs
/// stay bit-exact (the delay at its default 250 ms, whole frames), the lag
/// within 1e-9 of the input's size and the envelope within 1e-9; the
/// shaper's error is only printed, its drive·in being far beyond low drive.
/// Several composites' intermediates go through the core types that clamp
/// silently to ±1e6 (`std.sub`, `std.div`, `std.tan`, `std.exp`,
/// `std.scale`, `std.clamp`) as well as `std.add` and `std.mul`, which fault,
/// so near the bound a composite could clamp without a fault; the
/// comparison is what would show it.
#[test]
fn input_amplitude_headroom_is_reported_and_bounded() {
    for pair in &signal_lib::VALIDATION {
        let port = pair.outputs[0];
        let input = match pair.composite {
            "adsr_core" => "gate",
            "osc_core" => "pm",
            _ => "in",
        };
        let mut headroom = 0.0;
        for scale in [1.0, 1e2, 1e4, 1e6] {
            let mut c = case(
                pair.composite,
                "noise",
                "",
                noise(),
                vec![("sig.out", input, 2.0 * scale)],
                (port, port),
                Tolerance::Exact,
            );
            if pair.composite == "osc_core" {
                c.native_only = "shape = 0";
            }
            let rendered = render(
                &project(&c, 48_000, &format!("ref.{port}"), &format!("x.{port}")),
                4800,
                64,
                1,
            );
            assert_bounded(&rendered, pair.composite);
            let faults = faults(&rendered);
            assert!(
                faults
                    .iter()
                    .all(|f| f.contains("\"magnitude\"") || f.contains("\"mute\"")),
                "{}: {faults:?}",
                pair.composite
            );
            let composite_faulted = faults.iter().any(|f| f.contains("\"address\":\"/x/"));
            if !composite_faulted {
                headroom = scale;
            }
            if faults.is_empty() {
                let (native, composite) = channels(&rendered);
                let e = errors(&native, &composite);
                println!(
                    "HEADROOM lib.{} at input amplitude {scale:e}: no fault, max error {:.3e}, bit-exact {}",
                    pair.composite,
                    e.max,
                    e.differing == 0
                );
                match pair.composite {
                    "tanh_shaper_core" => {}
                    "lag_core" => assert!(e.max <= 1e-9 * scale, "lag at {scale}: {}", e.max),
                    "adsr_core" => assert!(e.max <= 1e-9, "adsr at {scale}: {}", e.max),
                    other => assert_eq!(e.differing, 0, "{other} at {scale}: not bit-exact"),
                }
            }
        }
        println!(
            "HEADROOM lib.{}: no fault up to input amplitude {headroom:e}",
            pair.composite
        );
    }
}
