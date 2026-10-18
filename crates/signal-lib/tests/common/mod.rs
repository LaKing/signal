//! The fixed stimulus of each library composite and a short render of it
//! through the engine, shared by `pinned.rs`, `bounds.rs` and `library.rs`.

#![allow(dead_code)]

use signal_core::Registry;
use signal_engine::{Engine, EngineConfig};
use signal_output::{Render, render_blocks};
use signal_project::{Project, SourceFile};

/// The render's rate and seed.
pub const RATE: u32 = 48_000;
pub const SEED: u64 = 1;
/// 0.2 s: the default render length.
pub const FRAMES: u64 = 9_600;

pub fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).expect("std names are unique");
    registry
}

/// The render length of `name`'s stimulus: long enough for its whole signal
/// path to reach the output at its defaults. `tape_echo`'s repeats start at
/// 375 ms, so 1 s holds two trips round its feedback loop; `lfo_shapes` at
/// its default 1 Hz wraps once in 1 s; `ladder_voice` and `ar_env` see two
/// and a half gate cycles (and the voice two pitch steps, so its glide moves)
/// in 0.5 s. Every other composite (and any other project) renders 0.2 s.
pub fn frames(name: &str) -> u64 {
    match name {
        "tape_echo" | "lfo_shapes" => 48_000,
        "ladder_voice" | "ar_env" => 24_000,
        _ => FRAMES,
    }
}

/// The stimulus of `name`: instance lines and connection lines (the
/// composite's instance is `x`; `out` is the audio sink, gain 1). Triggers
/// come from phasors' wraps (8, 7 and 5 Hz), pitches and gates from constants
/// and phasors through comparators, signals from a 220 Hz sine and a 330 Hz
/// saw. Every exported input is driven.
pub fn stimulus(name: &str) -> (&'static str, &'static str) {
    match name {
        "analog_kick" | "analog_snare" | "analog_clap" | "analog_tom" | "analog_rim"
        | "analog_cymbal" => (
            "clock = { module = \"std.phasor\", freq = 8 }\n",
            "{ from = \"clock.wrap\", to = \"x.trigger\" },\n\
             { from = \"x.out\", to = \"out.l\" },\n{ from = \"x.out\", to = \"out.r\" },\n",
        ),
        "analog_hat" => (
            "clock = { module = \"std.phasor\", freq = 8 }\n\
             clock_open = { module = \"std.phasor\", freq = 5 }\n",
            "{ from = \"clock.wrap\", to = \"x.closed\" },\n\
             { from = \"clock_open.wrap\", to = \"x.open\" },\n\
             { from = \"x.out\", to = \"out.l\" },\n{ from = \"x.out\", to = \"out.r\" },\n",
        ),
        // The pitch steps between 110 and 220 Hz at 3 Hz (so the glide and
        // the first-note path both act), the gate opens and closes at 5 Hz.
        "ladder_voice" => (
            "step_clock = { module = \"std.phasor\", freq = 3 }\n\
             step = { module = \"std.compare\", mode = 0 }\n\
             hz = { module = \"std.scale\", mul = 110, add = 110 }\n\
             gate_clock = { module = \"std.phasor\", freq = 5 }\n\
             gate = { module = \"std.compare\", mode = 0 }\n\
             half = { module = \"std.const\", value = 0.5 }\n\
             velocity = { module = \"std.const\", value = 0.8 }\n",
            "{ from = \"step_clock.phase\", to = \"step.a\" },\n\
             { from = \"half.out\", to = \"step.b\" },\n\
             { from = \"step.out\", to = \"hz.in\" },\n\
             { from = \"gate_clock.phase\", to = \"gate.a\" },\n\
             { from = \"half.out\", to = \"gate.b\" },\n\
             { from = \"hz.out\", to = \"x.hz\" },\n{ from = \"gate.out\", to = \"x.gate\" },\n\
             { from = \"velocity.out\", to = \"x.velocity\" },\n\
             { from = \"x.out\", to = \"out.l\" },\n{ from = \"x.out\", to = \"out.r\" },\n",
        ),
        "ar_env" => (
            "gate_clock = { module = \"std.phasor\", freq = 5 }\n\
             gate = { module = \"std.compare\", mode = 0 }\n\
             half = { module = \"std.const\", value = 0.5 }\n",
            "{ from = \"gate_clock.phase\", to = \"gate.a\" },\n\
             { from = \"half.out\", to = \"gate.b\" },\n{ from = \"gate.out\", to = \"x.gate\" },\n\
             { from = \"x.out\", to = \"out.l\" },\n{ from = \"x.out\", to = \"out.r\" },\n",
        ),
        // A 7 Hz reset, so the phase restarts and random draws at any rate.
        "lfo_shapes" => (
            "reset_clock = { module = \"std.phasor\", freq = 7 }\n",
            "{ from = \"reset_clock.wrap\", to = \"x.reset\" },\n\
             { from = \"x.out\", to = \"out.l\" },\n{ from = \"x.random\", to = \"out.r\" },\n",
        ),
        "tape_echo" => (
            "tone = { module = \"std.sine_osc\", freq = 220 }\n",
            "{ from = \"tone.out\", to = \"x.in\" },\n\
             { from = \"x.out\", to = \"out.l\" },\n{ from = \"x.out\", to = \"out.r\" },\n",
        ),
        "crossfade" => (
            "tone = { module = \"std.sine_osc\", freq = 220 }\n\
             saw = { module = \"std.osc\", shape = 1, freq = 330 }\n",
            "{ from = \"tone.out\", to = \"x.a\" },\n{ from = \"saw.out\", to = \"x.b\" },\n\
             { from = \"x.out\", to = \"out.l\" },\n{ from = \"x.out\", to = \"out.r\" },\n",
        ),
        "stereo_width" => (
            "tone = { module = \"std.sine_osc\", freq = 220 }\n\
             saw = { module = \"std.osc\", shape = 1, freq = 330 }\n",
            "{ from = \"tone.out\", to = \"x.l\" },\n{ from = \"saw.out\", to = \"x.r\" },\n\
             { from = \"x.out_l\", to = \"out.l\" },\n{ from = \"x.out_r\", to = \"out.r\" },\n",
        ),
        other => panic!("no stimulus for lib.{other}: add one to tests/common/mod.rs"),
    }
}

/// One pinned render of a composite: parameters set on its instance, and
/// the stimulus's instance and connection lines.
pub struct Case {
    pub parameters: String,
    pub instances: String,
    pub connections: String,
}

/// The renders that pin `name` (`tests/pinned.rs`): together they reach
/// every exported input, output and parameter path. Most composites: one,
/// their stimulus at their defaults. `lfo_shapes`: one per shape at 20 Hz
/// (four wraps in 0.2 s, beside the 7 Hz reset), each with `out` on the left
/// and that shape's own output on the right, so every output and every
/// position of the selector is in the hash.
pub fn cases(name: &str) -> Vec<Case> {
    let (instances, connections) = stimulus(name);
    match name {
        "lfo_shapes" => ["sine", "triangle", "saw", "square", "random"]
            .iter()
            .enumerate()
            .map(|(shape, output)| Case {
                parameters: format!("rate = 20, shape = {shape}"),
                instances: instances.to_owned(),
                connections: format!(
                    "{{ from = \"reset_clock.wrap\", to = \"x.reset\" }},\n\
                     {{ from = \"x.out\", to = \"out.l\" }},\n\
                     {{ from = \"x.{output}\", to = \"out.r\" }},\n"
                ),
            })
            .collect(),
        _ => vec![Case {
            parameters: String::new(),
            instances: instances.to_owned(),
            connections: connections.to_owned(),
        }],
    }
}

/// The project rendering `lib.<name>` on its stimulus, with `parameters`
/// (`key = value, …`) set on its instance.
pub fn project(name: &str, parameters: &str) -> Project {
    let (instances, connections) = stimulus(name);
    project_with(name, parameters, instances, connections)
}

/// The project rendering `lib.<name>` with `parameters` on its instance and
/// these stimulus lines.
pub fn project_with(name: &str, parameters: &str, instances: &str, connections: &str) -> Project {
    let set = if parameters.is_empty() {
        String::new()
    } else {
        format!(", {parameters}")
    };
    let text = format!(
        "schema_version = 1\n\n[project]\nname = \"{name}\"\nsample_rate = {RATE}\nseed = {SEED}\n\n\
         [instances]\nx = {{ module = \"lib.{name}\"{set} }}\n{instances}\
         out = {{ module = \"std.audio_out\", gain = 1 }}\n\n\
         [wiring]\nconnections = [\n{connections}]\n"
    );
    Project::from_sources(vec![SourceFile {
        path: "project.toml".to_owned(),
        text,
    }])
    .unwrap_or_else(|d| panic!("lib.{name}: {d:?}"))
}

/// Renders `project` for [`frames`] of its name (the composite's) in blocks
/// of 64.
pub fn render(project: &Project) -> Render {
    let registry = registry();
    let plan = project
        .plan(&registry)
        .unwrap_or_else(|d| panic!("{}: {d:?}", project.name))
        .plan;
    let config = EngineConfig::new(project.sample_rate, project.seed);
    let mut engine = Engine::new(&plan, &registry, config, Vec::new()).unwrap();
    render_blocks(&mut engine, frames(&project.name), 64, |_| Ok(None)).unwrap()
}

/// The log's fault and mute records.
pub fn faults(render: &Render) -> Vec<String> {
    String::from_utf8_lossy(&render.log)
        .lines()
        .filter(|l| l.contains("\"type\":\"fault\"") || l.contains("\"type\":\"mute\""))
        .map(str::to_owned)
        .collect()
}

/// The largest absolute sample; infinite when one is not finite.
pub fn peak(render: &Render) -> f64 {
    render.samples.iter().fold(0.0f64, |m, s| {
        if s.is_finite() {
            m.max(s.abs())
        } else {
            f64::INFINITY
        }
    })
}
