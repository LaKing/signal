//! M2.10-8 through the engine: `std.modal`, `std.exciter` and
//! `lib.modal_voice` with every parameter modulated at audio rate (a 1 kHz
//! sine on each, the bank's vector inputs from a `std.pack64` of sines), at
//! 44.1, 48, 96 and 192 kHz, run without a fault or a mute and with every
//! sample finite (AC6, AC10); so do inputs scaled to 10³⁰⁸ or beyond (±∞)
//! and a parameter made NaN by two opposite such connections; and a project
//! of all three renders bit-identically for k = 1, 64 and 256 on 1, 2, 4
//! and 8 threads (AC10). The projects are written by the tests.

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
        "schema_version = 1\n\n[project]\nname = \"physical-mod\"\nsample_rate = {rate}\nseed = 7\n\n\
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
        .filter(|l| l.contains("\"type\":\"fault\"") || l.contains("\"type\":\"mute\""))
        .map(str::to_owned)
        .collect()
}

/// The modulators: a 1 kHz sine (`lfo`), a 30 Hz trigger clock, a 0.5
/// constant and a vector of 64 sines (`rows`, through `std.pack64`).
fn modulators() -> String {
    let mut lines = String::from(
        "lfo = { module = \"std.sine_osc\", freq = 1000 }\n\
         clock = { module = \"std.phasor\", freq = 30 }\n\
         half = { module = \"std.const\", value = 0.5 }\n\
         fb = { module = \"std.unit_delay\" }\n\
         rows = { module = \"std.pack64\" }\n",
    );
    for i in 0..4 {
        lines += &format!(
            "row{i} = {{ module = \"std.sine_osc\", freq = {} }}\n",
            300 + 170 * i
        );
    }
    lines
}

/// The rows' wiring: the four sines spread over the 64 elements of `rows`.
fn row_connections() -> String {
    (0..64)
        .map(|k| format!("{{ from = \"row{}.out\", to = \"rows.e{k}\" }},\n", k % 4))
        .collect()
}

/// Connections from `from` (scaled by `scale`) to every parameter of
/// `instance` named in `parameters`.
fn drive(from: &str, instance: &str, parameters: &[(&str, f64)]) -> String {
    parameters
        .iter()
        .map(|(p, scale)| {
            format!("{{ from = \"{from}\", to = \"{instance}.{p}\", scale = {scale} }},\n")
        })
        .collect()
}

/// The bank's continuous parameters and a modulation depth for each.
const MODAL_PARAMETERS: [(&str, f64); 13] = [
    ("freq", 2000.0),
    ("material", 2.5),
    ("decay_s", 30.0),
    ("law", 1.5),
    ("position", 0.4),
    ("ratio_3", 40.0),
    ("inharmonicity", 0.05),
    ("damping_tilt", 5.0),
    ("brightness", 1.0),
    ("pickup", 0.4),
    ("pickup_mix", 0.5),
    ("spread", 0.05),
    ("gain", 2.0),
];

const EXCITER_PARAMETERS: [(&str, f64); 7] = [
    ("level", 0.5),
    ("duration_ms", 20.0),
    ("hardness", 0.5),
    ("bow_speed", 2.0),
    ("friction", 0.5),
    ("noise_color", 1.0),
    ("position", 0.25),
];

/// The three under test: an exciter into a bank (`x`, 64 modes, decay
/// 60 s, `mode_gain` and `mode_decay` from the rows) and a voice (`v`),
/// every parameter driven by the 1 kHz sine.
fn instances(kind: u32) -> String {
    format!(
        "{}e = {{ module = \"std.exciter\", kind = {kind}, duration_ms = 30 }}\n\
         x = {{ module = \"std.modal\", freq = 2200, material = 2.5, modes = 64, decay_s = 30, \
         position = 0.45, ratio_3 = 50 }}\n\
         v = {{ module = \"lib.modal_voice\", exciter_kind = {kind}, decay = 30, modes = 64 }}\n",
        modulators()
    )
}

fn connections() -> String {
    let mut lines = row_connections();
    lines += "{ from = \"clock.wrap\", to = \"e.trigger\" },\n\
              { from = \"x.out\", to = \"fb.in\" },\n\
              { from = \"fb.out\", to = \"e.velocity\", scale = 0.01 },\n\
              { from = \"e.out\", to = \"x.in\" },\n\
              { from = \"rows.out\", to = \"x.mode_freq\", scale = 0.5 },\n\
              { from = \"rows.out\", to = \"x.mode_gain\", scale = 4 },\n\
              { from = \"rows.out\", to = \"x.mode_decay\", scale = 8 },\n\
              { from = \"lfo.out\", to = \"v.hz\", scale = 200 },\n\
              { from = \"half.out\", to = \"v.hz\", scale = 600 },\n\
              { from = \"clock.phase\", to = \"v.gate\", scale = 2 },\n\
              { from = \"half.out\", to = \"v.velocity\", scale = 2 },\n\
              { from = \"rows.out\", to = \"v.mode_gain\" },\n";
    lines += &drive("lfo.out", "x", &MODAL_PARAMETERS);
    lines += &drive("lfo.out", "e", &EXCITER_PARAMETERS);
    let voice = [
        ("material", 2.5),
        ("decay", 20.0),
        ("strike_position", 0.4),
        ("brightness", 1.0),
        ("pickup_mix", 0.5),
        ("inharmonicity", 0.05),
        ("hardness", 0.5),
        ("exciter_ms", 10.0),
        ("body_gain", 1.0),
    ];
    lines += &drive("lfo.out", "v", &voice);
    lines += "{ from = \"x.out\", to = \"out.l\", scale = 0.001 },\n\
              { from = \"v.out\", to = \"out.r\", scale = 0.001 },\n";
    lines
}

/// At 44.1, 48, 96 and 192 kHz, 1 s each, for a struck, a noise and a bowed
/// exciter: nothing faults or is muted, every sample is finite, and the
/// bank and the voice both sound.
#[test]
fn every_parameter_at_audio_rate_stays_within_the_bound() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for kind in [1, 2, 3] {
            let text = project_text(rate, &instances(kind), &connections());
            let (rendered, engine) = render(&project(&text), u64::from(rate), 256, 1);
            assert_eq!(
                faults(&rendered),
                Vec::<String>::new(),
                "{rate} Hz, kind {kind}"
            );
            for name in ["/x", "/e", "/v"] {
                let address: InstanceAddress = name.parse().unwrap();
                assert_ne!(engine.is_muted(&address), Some(true), "{name}");
            }
            assert!(rendered.samples.iter().all(|y| y.is_finite()));
            let peak = |channel: usize| {
                rendered
                    .samples
                    .chunks(2)
                    .fold(0.0f64, |p, s| p.max(s[channel].abs()))
            };
            println!(
                "{rate} Hz, kind {kind}: peaks {:.4} {:.4}",
                peak(0),
                peak(1)
            );
            assert!(
                peak(0) > 1e-4 && peak(1) > 1e-4,
                "{rate} Hz, kind {kind}: silent"
            );
        }
    }
}

/// Inputs scaled to 10³⁰⁸ (finite) and beyond (+∞ from a sum of two), and
/// a parameter made NaN by two opposite such connections (∞ − ∞), into the
/// bank's input, its `decay_s` and `gain` and a row, the exciter's
/// `velocity` and `level`, and the voice's `hz`, `velocity`, `gate` and a
/// row: nothing faults or is muted and every sample is finite.
#[test]
fn huge_and_non_finite_inputs_never_fault() {
    let huge = "{ from = \"one.out\", to = \"TARGET\", scale = 1e308 },\n";
    let infinite = format!("{huge}{huge}");
    let nan = format!(
        "{infinite}{{ from = \"one.out\", to = \"TARGET\", scale = -1e308 }},\n\
         {{ from = \"one.out\", to = \"TARGET\", scale = -1e308 }},\n"
    );
    let targets = [
        "x.in",
        "x.decay_s",
        "x.gain",
        "x.mode_gain",
        "e.velocity",
        "e.level",
        "v.hz",
        "v.velocity",
        "v.gate",
        "v.mode_decay",
    ];
    for (what, lines) in [("1e308", huge.to_owned()), ("∞", infinite), ("NaN", nan)] {
        for target in targets {
            let instances = format!(
                "{}one = {{ module = \"std.const\", value = 1 }}\n\
                 big = {{ module = \"std.pack64\" }}\n",
                instances(3)
            );
            // A vector input gets the value in element 0 of `big`.
            let extra = match target.contains("mode_") {
                true => format!(
                    "{}{{ from = \"big.out\", to = \"{target}\" }},\n",
                    lines.replace("TARGET", "big.e0")
                ),
                false => lines.replace("TARGET", target),
            };
            let text = project_text(48_000, &instances, &format!("{}{extra}", connections()));
            let (rendered, engine) = render(&project(&text), 24_000, 64, 1);
            assert_eq!(
                faults(&rendered),
                Vec::<String>::new(),
                "{what} into {target}"
            );
            for name in ["/x", "/e", "/v"] {
                let address: InstanceAddress = name.parse().unwrap();
                assert_ne!(
                    engine.is_muted(&address),
                    Some(true),
                    "{what} into {target}: {name}"
                );
            }
            assert!(
                rendered.samples.iter().all(|y| y.is_finite()),
                "{what} into {target}"
            );
        }
    }
}

/// The exciter, the bank and the voice in one project, modulated at audio
/// rate, render the same samples and log for k = 1, 64 and 256 on 1, 2, 4
/// and 8 threads as one frame at a time on one thread.
#[test]
fn bit_identical_across_k_and_threads() {
    let text = project_text(48_000, &instances(1), &connections());
    let project = project(&text);
    let frames = 20_000;
    let (reference, _) = render(&project, frames, 1, 1);
    assert!(faults(&reference).is_empty());
    assert!(reference.samples.iter().any(|y| y.abs() > 1e-4));
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
