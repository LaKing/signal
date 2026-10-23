//! ADR 0035 and ADR 0037, phase 1 (offline): projects included as
//! instances and slots, resolved by `signal-project` into ordinary composite
//! instances before the graph sees them.
//!
//! - Seeds (ADR 0035 test 3): an include draws the streams it draws alone;
//!   another `seed` changes them; two same-seed includes are identical; the
//!   host's seed leaves includes alone.
//! - Canonical order (test 4): sums whose association depends on source
//!   addresses (`/a`, `/q`, `/z` and a converter) are bit-identical
//!   included under `/m`.
//! - Refusals (test 8): a direct and an indirect cycle, depth 9, a sink
//!   without a tap.
//! - Rate (test 9): a 48 kHz include in a 96 kHz host equals itself rendered
//!   alone at 96 kHz, and the rate is noted.
//! - Diagnostics located in included files; `[exports]` in `project.toml`
//!   checked and meaning nothing alone; `set`; the manifest's fields.
//! - Slots (ADR 0037 tests 1–3, 7): plans byte-identical to the explicit
//!   form for a composite, a primitive and a project; the interface inline,
//!   from a project template and from `lib.voice` alike; the empty slot;
//!   interface resolution and matching diagnostics, located by line and
//!   column; the library templates read.
//!
//! Fixtures are written into fresh folders under the target's temporary
//! directory; the committed projects are only read.

use std::fs;
use std::path::{Path, PathBuf};

use signal_core::{
    DirectPrimitive, ModuleDescriptor, ModuleType, PortAddress, Primitive, Registry,
};
use signal_engine::{Engine, EngineConfig, Schedule};
use signal_graph::ExecutionPlan;
use signal_project::{Diagnostic, Project, Role, Severity};

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

/// A fresh folder for the fixture `name`.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("content")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes `files` (relative path, text) under `dir`.
fn write(dir: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let path = dir.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
}

fn load(dir: &Path) -> Project {
    Project::load(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
}

fn plan(dir: &Path) -> ExecutionPlan {
    let project = load(dir);
    match project.plan(&registry()) {
        Ok(planned) => planned.plan,
        Err(errors) => panic!("{}: {errors:#?}", dir.display()),
    }
}

/// The diagnostics loading and planning `dir` gives, as text.
fn problems(dir: &Path) -> Vec<String> {
    let found: Vec<Diagnostic> = match Project::load(dir) {
        Err(signal_project::LoadError::Invalid(found)) => found,
        Err(other) => panic!("{other}"),
        Ok(project) => match project.plan(&registry()) {
            Err(found) => found,
            Ok(planned) => planned.warnings,
        },
    };
    found.iter().map(ToString::to_string).collect()
}

fn has(problems: &[String], parts: &[&str]) -> bool {
    problems
        .iter()
        .any(|p| parts.iter().all(|part| p.contains(part)))
}

/// The plan as `signal render --plan-json` would dump it.
fn dump(plan: &ExecutionPlan) -> String {
    serde_json::to_string_pretty(plan).unwrap()
}

/// `frames` of `plan` at `rate` and `seed`, in blocks of `k` on `threads`
/// threads: every frame's value of each signal output in `ports`.
fn capture(
    plan: &ExecutionPlan,
    (rate, seed): (u32, u64),
    frames: usize,
    (k, threads): (usize, usize),
    ports: &[&str],
) -> Vec<Vec<f64>> {
    let registry = registry();
    let config = EngineConfig {
        threads,
        schedule: if threads > 1 {
            Schedule::FORCED
        } else {
            Schedule::DEFAULT
        },
        ..EngineConfig::new(rate, seed)
    };
    let mut engine = Engine::new(plan, &registry, config, Vec::new()).unwrap();
    let rows: Vec<usize> = ports
        .iter()
        .map(|p| {
            let port: PortAddress = p.parse().unwrap();
            engine
                .signal_output_index(&port)
                .unwrap_or_else(|| panic!("no signal output {p}"))
        })
        .collect();
    let mut out = vec![Vec::with_capacity(frames); ports.len()];
    let mut done = 0;
    while done < frames {
        let ran = engine.run_block(k.min(frames - done));
        assert!(ran > 0);
        for f in 0..ran {
            for (values, &row) in out.iter_mut().zip(&rows) {
                values.push(engine.signal_output_at(row, f).unwrap());
            }
        }
        done += ran;
    }
    out
}

/// The audio a project records alone (`audio.wav` before f32), per
/// channel.
fn alone(dir: &Path, rate: Option<u32>, frames: u64) -> [Vec<f64>; 2] {
    let project = load(dir);
    let registry = registry();
    let plan = project.plan(&registry).unwrap().plan;
    let config = EngineConfig::new(rate.unwrap_or(project.sample_rate), project.seed);
    let mut engine = Engine::new(&plan, &registry, config, Vec::new()).unwrap();
    let rendered = signal_output::render(&mut engine, frames).unwrap();
    let l = rendered.samples.iter().step_by(2).copied().collect();
    let r = rendered
        .samples
        .iter()
        .skip(1)
        .step_by(2)
        .copied()
        .collect();
    [l, r]
}

fn bits(values: &[f64]) -> Vec<u64> {
    values.iter().map(|v| v.to_bits()).collect()
}

/// A project using every kind of randomness the library has, so a
/// changed stream shows.
const NOISY: &str = r#"
schema_version = 1

[project]
name = "noisy"
sample_rate = 48000
seed = 21

[instances]
clock = { module = "std.transport", bpm = 300 }
a = { module = "std.noise" }
q = { module = "std.noise" }
z = { module = "std.noise" }
rnd = { module = "std.random" }
p = { module = "std.particles" }
out = { module = "std.audio_out", gain = 0.5 }

[wiring]
connections = [
  { from = "a.out", to = "out.l", scale = 0.3 },
  { from = "q.out", to = "out.l", scale = 0.2 },
  { from = "z.out", to = "out.l", scale = 0.1 },
  { from = "p.lost", to = "out.l" },
  { from = "clock.tick", to = "rnd.trigger" },
  { from = "rnd.out", to = "out.r" },
  { from = "a.out", to = "out.r", scale = 0.1 },
]
"#;

/// A host of `noisy` (in `noisy/`) as `m`, and of a second copy as `n`
/// when `twice`, at `rate`, with `entry` the include's extra keys.
fn noisy_host(name: &str, entry: &str, twice: bool, rate: u32) -> PathBuf {
    let dir = scratch(name);
    let second = if twice {
        format!("n = {{ project = \"noisy\"{entry} }}\n")
    } else {
        String::new()
    };
    let r = if twice { "n.l" } else { "m.r" };
    let host = format!(
        r#"schema_version = 1

[project]
name = "host"
sample_rate = {rate}
seed = 99

[instances]
m = {{ project = "noisy"{entry} }}
{second}out = {{ module = "std.audio_out" }}

[wiring]
connections = [
  {{ from = "m.l", to = "out.l" }},
  {{ from = "{r}", to = "out.r" }},
]
"#
    );
    write(
        &dir,
        &[("project.toml", &host), ("noisy/project.toml", NOISY)],
    );
    dir
}

/// ADR 0035 test 3, with test 4's canonical order: included under `/m`,
/// `noisy` draws the streams it draws alone and sums its sources in the
/// same order (`/m/a` < `/m/p/lost_d1` < `/m/q` < `/m/z` exactly as alone),
/// so its tap's outputs are its own audio bit for bit, on any partition and
/// thread count. The host's seed (99, or another run seed) reaches none of
/// it.
#[test]
fn an_include_draws_its_own_streams_and_sums_in_its_own_order() {
    let host = noisy_host("seeds", "", false, 48_000);
    let frames = 4_800;
    let [l, r] = alone(&host.join("noisy"), None, frames as u64);
    let plan = plan(&host);
    // The stream keys are the include's: its seed, its own addresses.
    let a = plan
        .instances
        .iter()
        .find(|i| i.address.as_str() == "/m/a")
        .unwrap();
    let key = a.stream.as_ref().unwrap();
    assert_eq!((key.seed, key.relative.as_str()), (21, "/a"));
    assert!(dump(&plan).contains("\"relative\": \"/a\""));
    // The converter inside sits under its source's address.
    assert!(
        plan.instances
            .iter()
            .any(|i| i.address.as_str() == "/m/p/lost_d1")
    );
    for (k, threads) in [(1, 1), (64, 2), (256, 4), (64, 8)] {
        for seed in [99, 1] {
            let taps = capture(
                &plan,
                (48_000, seed),
                frames,
                (k, threads),
                &["/m/out.l_out", "/m/out.r_out"],
            );
            assert_eq!(bits(&taps[0]), bits(&l), "k = {k}, threads = {threads}");
            assert_eq!(bits(&taps[1]), bits(&r), "k = {k}, threads = {threads}");
        }
    }
    assert!(l.iter().any(|x| *x != 0.0) && r.iter().any(|x| *x != 0.0));
}

/// ADR 0035 test 3: another `seed` gives other streams; two same-seed
/// includes of one project are identical.
#[test]
fn seeds_scope_the_include() {
    let frames = 2_400;
    let reference = alone(
        &noisy_host("seed-ref", "", false, 48_000).join("noisy"),
        None,
        frames,
    );
    let other = noisy_host("seed-5", ", seed = 5", false, 48_000);
    let taps = capture(
        &plan(&other),
        (48_000, 99),
        frames as usize,
        (64, 1),
        &["/m/out.l_out"],
    );
    assert_ne!(
        bits(&taps[0]),
        bits(&reference[0]),
        "seed = 5 changes the streams"
    );

    let twice = noisy_host("seed-twice", ", seed = 5", true, 48_000);
    let plan = plan(&twice);
    let taps = capture(
        &plan,
        (48_000, 99),
        frames as usize,
        (64, 1),
        &["/m/out.l_out", "/n/out.l_out"],
    );
    assert_eq!(
        bits(&taps[0]),
        bits(&taps[1]),
        "the same project twice, one seed"
    );
    let streams = |prefix: &str| {
        plan.instances
            .iter()
            .filter(|i| i.address.as_str().starts_with(prefix))
            .map(|i| i.stream.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(streams("/m/"), streams("/n/"));
}

/// ADR 0035 §8, test 9: an include runs at its host's rate, which is no
/// error: a 48 kHz project in a 96 kHz host equals itself rendered alone
/// at 96 kHz, and the difference is noted.
#[test]
fn an_include_plays_at_the_host_rate_and_the_rate_is_noted() {
    let host = noisy_host("rate", "", false, 96_000);
    let frames = 4_800;
    let [l, _] = alone(&host.join("noisy"), Some(96_000), frames as u64);
    let taps = capture(
        &plan(&host),
        (96_000, 99),
        frames,
        (256, 1),
        &["/m/out.l_out"],
    );
    assert_eq!(bits(&taps[0]), bits(&l));
    let project = load(&host);
    assert_eq!(
        project.rate_notes(96_000),
        ["noisy is written for 48000 Hz and plays at 96000 Hz here"]
    );
    assert!(project.rate_notes(48_000).is_empty());
    let include = &project.includes()[0];
    assert_eq!(
        (
            include.key.as_str(),
            include.name.as_str(),
            include.sample_rate,
            include.seed
        ),
        ("m", "noisy", 48_000, 21)
    );
}

/// ADR 0035 §8, test 8: a project that includes itself, directly or
/// through another, is refused at the entry that closes the cycle, naming
/// the chain.
#[test]
fn cycles_are_refused_naming_the_chain() {
    let direct = scratch("cycle-direct");
    let me = r#"schema_version = 1
[project]
name = "me"
[instances]
me = { project = "." }
out = { module = "std.audio_out" }
"#;
    write(&direct, &[("project.toml", me)]);
    let found = problems(&direct);
    let me = fs::canonicalize(&direct).unwrap().display().to_string();
    let chain = format!("{me} → {me}");
    assert!(
        has(
            &found,
            &[
                "project.toml:5:",
                "includes a project that includes it",
                &chain
            ]
        ),
        "the chain by canonical paths: {found:#?}"
    );

    let dir = scratch("cycle-indirect");
    let a = r#"schema_version = 1
[project]
name = "a"
[instances]
b = { project = "../b" }
out = { module = "std.audio_out" }
"#;
    let b = r#"schema_version = 1
[project]
name = "b"
[instances]
a = { project = "../a" }
out = { module = "std.audio_out" }
"#;
    write(&dir, &[("a/project.toml", a), ("b/project.toml", b)]);
    let found = problems(&dir.join("a"));
    let root = fs::canonicalize(&dir).unwrap().display().to_string();
    let chain = format!("{root}/a → {root}/b → {root}/a");
    assert!(
        has(&found, &["../b/project.toml:5:", &chain]),
        "the closing entry, in b's file: {found:#?}"
    );
    assert!(
        has(&found, &["project.toml:5:", "`../b` does not load"]),
        "{found:#?}"
    );

    // The same project twice, not in a cycle, is fine.
    let twice = noisy_host("cycle-twice", "", true, 48_000);
    plan(&twice);
}

/// ADR 0035 §8, test 8: includes nest 8 deep at most; the ninth is refused.
#[test]
fn includes_nest_eight_deep_at_most() {
    let dir = scratch("depth");
    let level = |n: usize, last: usize| {
        let inner = if n < last {
            format!("inner = {{ project = \"../p{} \" }}\n", n + 1).replace(" \"", "\"")
        } else {
            String::new()
        };
        let wire = if n < last {
            "connections = [ { from = \"inner.l\", to = \"out.l\" } ]\n"
        } else {
            "connections = []\n"
        };
        format!(
            "schema_version = 1\n[project]\nname = \"p{n}\"\n[instances]\n{inner}out = {{ module = \"std.audio_out\" }}\n[wiring]\n{wire}"
        )
    };
    let texts: Vec<String> = (0..=9).map(|n| level(n, 9)).collect();
    let files: Vec<(String, &str)> = texts
        .iter()
        .enumerate()
        .map(|(n, t)| (format!("p{n}/project.toml"), t.as_str()))
        .collect();
    let files: Vec<(&str, &str)> = files.iter().map(|(p, t)| (p.as_str(), *t)).collect();
    write(&dir, &files);
    let found = problems(&dir.join("p0"));
    assert!(
        has(&found, &["../p8/project.toml", "nest 8 deep at most"]),
        "p8 (depth 8) may not include p9: {found:#?}"
    );
    // Eight deep is fine: p1 hosts p2 … p9.
    plan(&dir.join("p1"));
}

/// A sink type of a test library with no registered tap.
#[derive(Default)]
struct OtherSink;

impl DirectPrimitive for OtherSink {
    fn describe() -> ModuleDescriptor {
        ModuleDescriptor {
            type_name: "test.other_out",
            ..signal_std::AudioOut::describe()
        }
    }
    fn prepare(&mut self, _: &signal_core::PrepareContext) {}
    fn reset(&mut self) {}
    fn process(&mut self, _: &signal_core::Inputs<'_>, _: &mut signal_core::Outputs<'_>) {}
}

/// ADR 0035 §3, test 8: a project whose sink has no tap is refused as an
/// include, at the sink, in its file.
#[test]
fn a_sink_without_a_tap_is_refused() {
    let dir = scratch("no-tap");
    let inner = r#"schema_version = 1
[project]
name = "av"
[instances]
screen = { module = "test.other_out" }
"#;
    let host = r#"schema_version = 1
[project]
name = "host"
[instances]
av = { project = "av" }
out = { module = "std.audio_out" }
"#;
    write(&dir, &[("project.toml", host), ("av/project.toml", inner)]);
    let mut registry = registry();
    registry
        .register(ModuleType {
            descriptor: OtherSink::describe(),
            create: || Primitive::Direct(Box::new(OtherSink)),
        })
        .unwrap();
    // Alone it plans.
    load(&dir.join("av")).plan(&registry).unwrap();
    let found: Vec<String> = load(&dir)
        .plan(&registry)
        .err()
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(
        has(
            &found,
            &[
                "av/project.toml:5:",
                "`test.other_out` is a sink with no tap"
            ]
        ),
        "{found:#?}"
    );
}

/// ADR 0035 §1: an included project's problems are located in its files,
/// named from the host's folder, with the entry that includes it.
#[test]
fn diagnostics_are_located_in_included_files() {
    let dir = scratch("located");
    let host = r#"schema_version = 1
[project]
name = "host"
[instances]
inner = { project = "parts/inner" }
out = { module = "std.audio_out" }
"#;
    let inner = r#"schema_version = 1
[project]
name = "inner"
[instances]
v = { module = "voice" }
out = { module = "std.audio_out" }
"#;
    let voice = r#"schema_version = 1
[composite]
name = "voice"
[instances]
osc = { module = "std.sine_osc", typo = 1 }
"#;
    write(
        &dir,
        &[
            ("project.toml", host),
            ("parts/inner/project.toml", inner),
            ("parts/inner/composites/voice.toml", voice),
        ],
    );
    let found = problems(&dir);
    assert!(
        has(
            &found,
            &["parts/inner/composites/voice.toml:5:", "unknown parameter"]
        ),
        "{found:#?}"
    );
    assert!(
        has(&found, &["project.toml:5:", "`parts/inner` does not plan"]),
        "{found:#?}"
    );
    // A syntax error there stops the load, located the same way.
    write(
        &dir,
        &[("parts/inner/composites/voice.toml", "schema_version = [")],
    );
    let found = problems(&dir);
    assert!(
        has(&found, &["parts/inner/composites/voice.toml:1:"]),
        "{found:#?}"
    );
    assert!(has(&found, &["`parts/inner` does not load"]), "{found:#?}");
}

/// ADR 0035 §2: `[exports]` in `project.toml` are checked and change
/// nothing alone; included, they are the instance's ports, and an
/// exported parameter is set like a composite's. `set` overrides a
/// top-level parameter of the include; a key it does not have is an error.
#[test]
fn exports_and_set_reach_the_include() {
    let dir = scratch("exports");
    let inner = r#"schema_version = 1
[project]
name = "tone"
[instances]
osc = { module = "std.sine_osc", freq = 220 }
amp = { module = "std.gain" }
out = { module = "std.audio_out" }
[wiring]
connections = [
  { from = "osc.out", to = "amp.in" },
  { from = "amp.out", to = "out.l" },
]
[exports]
outputs = { tone = "amp.out" }
parameters = { freq = "osc.freq" }
"#;
    let host = r#"schema_version = 1
[project]
name = "host"
[instances]
t = { project = "tone", freq = 330, set = { "amp.db" = -6 } }
out = { module = "std.audio_out" }
[wiring]
connections = [ { from = "t.tone", to = "out.l" } ]
"#;
    write(
        &dir,
        &[("project.toml", host), ("tone/project.toml", inner)],
    );
    let alone = plan(&dir.join("tone"));
    let without = r#"schema_version = 1
[project]
name = "tone"
[instances]
osc = { module = "std.sine_osc", freq = 220 }
amp = { module = "std.gain" }
out = { module = "std.audio_out" }
[wiring]
connections = [
  { from = "osc.out", to = "amp.in" },
  { from = "amp.out", to = "out.l" },
]
"#;
    let plain = scratch("exports-none");
    write(&plain, &[("project.toml", without)]);
    assert_eq!(
        dump(&alone),
        dump(&plan(&plain)),
        "exports change nothing alone"
    );

    let hosted = plan(&dir);
    let parameter = |address: &str, name: &str| {
        hosted
            .instances
            .iter()
            .find(|i| i.address.as_str() == address)
            .unwrap()
            .parameters[name]
    };
    assert_eq!(parameter("/t/osc", "freq"), 330.0);
    assert_eq!(parameter("/t/amp", "db"), -6.0);
    assert_eq!(
        hosted.sources[&"/out.l".parse::<PortAddress>().unwrap()][0]
            .from
            .as_str(),
        "/t/amp.out"
    );
    // With `[exports] outputs`, the sink's channels are not exported.
    assert!(
        hosted
            .instances
            .iter()
            .any(|i| i.type_name == "std.audio_tap")
    );

    let wrong = host.replace("\"amp.db\"", "\"nope.db\"");
    write(&dir, &[("project.toml", &wrong)]);
    let found = problems(&dir);
    assert!(
        has(&found, &["project.toml:5:", "`set` key `nope.db`"]),
        "{found:#?}"
    );

    let bad = inner.replace("\"amp.out\" }", "\"amp.nope\" }");
    write(&dir, &[("tone/project.toml", &bad), ("project.toml", host)]);
    let found = problems(&dir.join("tone"));
    assert!(
        has(&found, &["project.toml:", "amp.nope"]),
        "exports are checked alone: {found:#?}"
    );
}

/// ADR 0035 §13: the manifest gains the include's files and an entry per
/// include; a project without includes has neither.
#[test]
fn the_manifest_lists_includes() {
    let host = noisy_host("manifest", "", false, 48_000);
    let project = load(&host);
    let hashes = project.include_file_hashes();
    assert_eq!(
        hashes.keys().collect::<Vec<_>>(),
        ["include:m/project.toml"]
    );
    let included = project.included_projects();
    assert_eq!(included.len(), 1);
    assert_eq!(
        (
            included[0].key.as_str(),
            included[0].path.as_str(),
            included[0].name.as_str(),
            included[0].seed
        ),
        ("m", "noisy", "noisy", 21)
    );
    let alone = load(&host.join("noisy"));
    assert!(alone.include_file_hashes().is_empty() && alone.included_projects().is_empty());
}

/// The `moog_voice`-like composite slot fixtures: a voice composite, a
/// sequencer-free host playing it from constants.
const VOICE: &str = r#"schema_version = 1
[composite]
name = "voice"
[instances]
osc = { module = "std.sine_osc" }
amp = { module = "std.mul" }
vol = { module = "std.gain" }
[wiring]
connections = [ { from = "osc.out", to = "amp.a" }, { from = "amp.out", to = "vol.in" } ]
[exports]
inputs = { hz = "osc.freq", gate = "amp.b" }
outputs = { out = "vol.out" }
parameters = { level = "vol.db" }
"#;

const DECK: &str = r#"schema_version = 1
[interface]
name = "deck"
[inputs]
position = { kind = "control" }
[outputs]
l = { kind = "audio" }
r = { kind = "audio" }
"#;

/// A host with `synth` written as `entry`, and its wires on `synth`'s
/// ports `hz`, `gate` and `out`.
fn voice_host(name: &str, entry: &str, extra: &[(&str, &str)]) -> PathBuf {
    let dir = scratch(name);
    let host = format!(
        r#"schema_version = 1
[project]
name = "host"
sample_rate = 48000
[instances]
pitch = {{ module = "std.const", value = 220 }}
gate = {{ module = "std.const", value = 1 }}
synth = {entry}
out = {{ module = "std.audio_out" }}
[wiring]
connections = [
  {{ from = "pitch.out", to = "synth.hz" }},
  {{ from = "gate.out", to = "synth.gate" }},
  {{ from = "synth.out", to = "out.l" }},
  {{ from = "synth.out", to = "out.r", scale = 0.5 }},
]
"#
    );
    let mut files = vec![
        ("project.toml", host.as_str()),
        ("composites/voice.toml", VOICE),
    ];
    files.extend_from_slice(extra);
    write(&dir, &files);
    dir
}

/// ADR 0037 test 1: a slot loaded with a composite plans byte-identically
/// to the explicit instance, whether its interface is inline, from a
/// project template or from `lib.voice`, and renders identically on every
/// partition and thread count.
#[test]
fn a_slot_plans_as_the_explicit_instance() {
    let explicit = plan(&voice_host("slot-explicit", r#"{ module = "voice" }"#, &[]));
    let inline = r#"{ slot = { inputs = { hz = "frequency", gate = "gate", velocity = "control" }, outputs = { out = "audio" } }, load = "voice" }"#;
    let template = r#"{ slot = "voice_if", load = "voice" }"#;
    let template_file = r#"schema_version = 1
[interface]
name = "voice_if"
[inputs]
hz = { kind = "frequency" }
gate = "gate"
velocity = "control"
[outputs]
out = "audio"
"#;
    let library = r#"{ slot = "lib.voice", load = "voice" }"#;
    let forms = [
        voice_host("slot-inline", inline, &[]),
        voice_host(
            "slot-template",
            template,
            &[("interfaces/voice_if.toml", template_file)],
        ),
        voice_host("slot-library", library, &[]),
    ];
    for dir in &forms {
        let planned = plan(dir);
        assert_eq!(dump(&planned), dump(&explicit), "{}", dir.display());
    }
    let reference = capture(&explicit, (48_000, 0), 2_000, (1, 1), &["/synth/vol.out"]);
    let slotted = plan(&forms[2]);
    for k in [1, 64, 256] {
        for threads in [1, 2, 4, 8] {
            let run = capture(
                &slotted,
                (48_000, 0),
                2_000,
                (k, threads),
                &["/synth/vol.out"],
            );
            assert_eq!(
                bits(&run[0]),
                bits(&reference[0]),
                "k = {k}, threads = {threads}"
            );
        }
    }
    // The match table: `velocity` is matched by nothing, and dropped
    // nothing (no wire names it); `out` is heard.
    let project = load(&forms[2]);
    let planned = project.plan(&registry()).unwrap();
    let slot = &planned.slots[0];
    assert_eq!(slot.slot, "/synth");
    assert_eq!(slot.interface.from.as_deref(), Some("lib.voice"));
    let matched: Vec<(&str, Option<&str>)> = slot
        .matches
        .iter()
        .map(|m| (m.port.as_str(), m.content.as_deref()))
        .collect();
    assert_eq!(
        matched,
        [
            ("gate", Some("gate")),
            ("hz", Some("hz")),
            ("velocity", None),
            ("out", Some("out"))
        ]
    );
    assert!(slot.unheard.is_empty() && slot.dropped.is_empty());
    assert_eq!(
        project.library_interface_hashes()["lib.voice"],
        signal_lib::interface("voice").unwrap().hash()
    );
}

/// ADR 0037 test 1: a primitive in a slot, with `map` renames, a `false`
/// and a parameter set through the interface, plans as the explicit
/// instance with the renamed wires and value; the wire on the unmatched
/// port is left out.
#[test]
fn a_primitive_in_a_slot_plans_as_the_explicit_instance() {
    let host = |entry: &str, wires: &str| {
        format!(
            r#"schema_version = 1
[project]
name = "host"
[instances]
pitch = {{ module = "std.const", value = 0.25 }}
synth = {entry}
out = {{ module = "std.audio_out" }}
[wiring]
connections = [
{wires}]
"#
        )
    };
    let explicit = scratch("slot-prim-explicit");
    write(
        &explicit,
        &[(
            "project.toml",
            &host(
                r#"{ module = "std.gain", db = -6 }"#,
                "  { from = \"pitch.out\", to = \"synth.in\" },\n  { from = \"synth.out\", to = \"out.l\" },\n",
            ),
        )],
    );
    let slotted = scratch("slot-prim");
    write(
        &slotted,
        &[(
            "project.toml",
            &host(
                r#"{ slot = { inputs = { x = "audio", side = "audio" }, outputs = { y = "audio" }, parameters = { volume = {} } }, load = "std.gain", map = { x = "in", y = "out", volume = "db", side = false }, volume = -6 }"#,
                "  { from = \"pitch.out\", to = \"synth.x\" },\n  { from = \"pitch.out\", to = \"synth.side\" },\n  { from = \"synth.y\", to = \"out.l\" },\n",
            ),
        )],
    );
    assert_eq!(dump(&plan(&slotted)), dump(&plan(&explicit)));
    let planned = load(&slotted).plan(&registry()).unwrap();
    let slot = &planned.slots[0];
    assert_eq!(slot.dropped, ["pitch.out -> synth.side"]);
    let side = slot.matches.iter().find(|m| m.port == "side").unwrap();
    assert_eq!(side.reason.as_deref(), Some("`map` sets it to false"));
}

/// ADR 0037 test 1, §5: a project in a `lib.deck` slot plans as the plain
/// include under the slot's name, its seed scope included.
#[test]
fn a_project_in_a_slot_plans_as_the_include() {
    let dir = scratch("slot-project");
    let host = |entry: &str| {
        format!(
            r#"schema_version = 1
[project]
name = "host"
seed = 4
[instances]
deck = {entry}
out = {{ module = "std.audio_out" }}
[wiring]
connections = [ {{ from = "deck.l", to = "out.l" }}, {{ from = "deck.r", to = "out.r" }} ]
"#
        )
    };
    write(
        &dir,
        &[
            (
                "explicit/project.toml",
                &host(r#"{ project = "../noisy", seed = 8 }"#),
            ),
            (
                "slot/project.toml",
                &host(r#"{ slot = "lib.deck", load = "../noisy", seed = 8 }"#),
            ),
            ("noisy/project.toml", NOISY),
        ],
    );
    let explicit = plan(&dir.join("explicit"));
    let slotted = plan(&dir.join("slot"));
    assert_eq!(dump(&slotted), dump(&explicit));
    let planned = load(&dir.join("slot")).plan(&registry()).unwrap();
    let slot = &planned.slots[0];
    let matched: Vec<&str> = slot
        .matches
        .iter()
        .filter_map(|m| m.content.as_deref())
        .collect();
    assert_eq!(matched, ["l", "r"], "the default sink exports");
    assert!(load(&dir.join("slot")).includes()[0].slot);
}

/// ADR 0037 test 2: an empty slot plans as the host without it and its
/// wires; the receivers have no source (`is_connected` false); its wires
/// are still checked against its interface.
#[test]
fn an_empty_slot_is_valid_and_silent() {
    let empty = voice_host("slot-empty", r#"{ slot = "lib.voice" }"#, &[]);
    let dir = scratch("slot-none");
    let none = r#"schema_version = 1
[project]
name = "host"
sample_rate = 48000
[instances]
pitch = { module = "std.const", value = 220 }
gate = { module = "std.const", value = 1 }
out = { module = "std.audio_out" }
"#;
    write(&dir, &[("project.toml", none)]);
    let planned = plan(&empty);
    let without = plan(&dir);
    assert_eq!(dump(&planned), dump(&without));
    assert!(planned.sources[&"/out.l".parse::<PortAddress>().unwrap()].is_empty());
    let resolution = &load(&empty).plan(&registry()).unwrap().slots[0];
    assert!(resolution.load.is_none() && resolution.matches.is_empty());
    assert_eq!(resolution.dropped.len(), 4);

    let undeclared = fs::read_to_string(empty.join("project.toml"))
        .unwrap()
        .replace("to = \"synth.gate\"", "to = \"synth.trigger\"");
    fs::write(empty.join("project.toml"), &undeclared).unwrap();
    let found = problems(&empty);
    assert!(
        has(
            &found,
            &[
                "project.toml:13:",
                "/synth: `trigger` is not a port of the slot's interface (from `lib.voice`)"
            ]
        ),
        "{found:#?}"
    );
    let shape =
        r#"{ slot = { inputs = { hz = "events", gate = "gate" }, outputs = { out = "audio" } } }"#;
    let found = problems(&voice_host("slot-empty-shape", shape, &[]));
    assert!(
        has(
            &found,
            &[
                "project.toml:12:",
                "`hz` is declared events on the slot's interface; `pitch.out` is a signal"
            ]
        ),
        "{found:#?}"
    );
}

/// ADR 0037 test 3: `from`, `remove` and retyping entries resolve as
/// specified; a `remove` of an unknown name and a name both removed and
/// declared are errors, located.
#[test]
fn interfaces_resolve_from_templates() {
    let retyped = r#"{ slot = { from = "lib.voice", remove = ["velocity"], inputs = { gate = "trigger" }, outputs = { aux = "audio" } }, load = "voice" }"#;
    let project = load(&voice_host("if-retype", retyped, &[]));
    let planned = project.plan(&registry()).unwrap();
    let interface = &planned.slots[0].interface;
    let ports: Vec<(&str, Role, signal_core::PortKind)> = interface
        .ports
        .iter()
        .map(|p| (p.name.as_str(), p.role, p.kind))
        .collect();
    use signal_core::PortKind::{Audio, Frequency, Trigger};
    assert_eq!(
        ports,
        [
            ("gate", Role::Input, Trigger),
            ("hz", Role::Input, Frequency),
            ("out", Role::Output, Audio),
            ("aux", Role::Output, Audio),
        ]
    );
    // `gate` retyped to a trigger: the content's gate input is a generic
    // port, so no kind warning; `aux` matches nothing.
    let aux = planned.slots[0]
        .matches
        .iter()
        .find(|m| m.port == "aux")
        .unwrap();
    assert!(aux.content.is_none());

    let unknown = r#"{ slot = { from = "lib.voice", remove = ["nope"] }, load = "voice" }"#;
    let found = problems(&voice_host("if-unknown", unknown, &[]));
    assert!(
        has(
            &found,
            &[
                "project.toml:8:",
                "`nope` is not a port or parameter of the interface `lib.voice`"
            ]
        ),
        "{found:#?}"
    );
    let both = r#"{ slot = { from = "lib.voice", remove = ["gate"], inputs = { gate = "gate" } }, load = "voice" }"#;
    let found = problems(&voice_host("if-both", both, &[]));
    assert!(
        has(
            &found,
            &["project.toml:8:", "`gate` is both removed and declared"]
        ),
        "{found:#?}"
    );
    let missing = r#"{ slot = "lib.nope" }"#;
    let found = problems(&voice_host("if-missing", missing, &[]));
    assert!(
        has(
            &found,
            &["unknown library interface `lib.nope`", "lib.deck"]
        ),
        "{found:#?}"
    );
    let reserved = r#"{ slot = { inputs = { remove = "gate" } } }"#;
    let found = problems(&voice_host("if-reserved", reserved, &[]));
    assert!(has(&found, &["`remove` is reserved"]), "{found:#?}");
}

/// ADR 0037 test 3: matching by exact name, `map` renames and `false`,
/// mono to stereo fan-out; the errors (direction, shape, rate, unknown
/// `map` key or value, two inputs onto one) and warnings (kind, nothing
/// matched, the "did you mean"), each located.
#[test]
fn matching_diagnostics() {
    // Mono to stereo: both outputs map to `out`.
    let stereo = r#"{ slot = "lib.instrument", load = "voice", map = { l = "out", r = "out" } }"#;
    let dir = voice_host("match-stereo", stereo, &[]);
    let text = fs::read_to_string(dir.join("project.toml"))
        .unwrap()
        .replace(
            "from = \"synth.out\", to = \"out.l\"",
            "from = \"synth.l\", to = \"out.l\"",
        )
        .replace(
            "from = \"synth.out\", to = \"out.r\"",
            "from = \"synth.r\", to = \"out.r\"",
        );
    fs::write(dir.join("project.toml"), text).unwrap();
    let planned = plan(&dir);
    let sources = |port: &str| {
        planned.sources[&port.parse::<PortAddress>().unwrap()]
            .iter()
            .map(|s| s.from.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(sources("/out.l"), ["/synth/vol.out"]);
    assert_eq!(sources("/out.r"), ["/synth/vol.out"]);

    let cases: [(&str, &[&str]); 7] = [
        (
            r#"{ slot = { inputs = { out = "audio" }, outputs = { hz = "audio" } }, load = "voice" }"#,
            &[
                "project.toml:8:",
                "`voice`'s `out` is an output; the slot's interface declares `out` an input",
            ],
        ),
        (
            r#"{ slot = { inputs = { hz = { shape = { vector = 2 } } } }, load = "voice" }"#,
            &[
                "project.toml:8:",
                "`voice` input `hz` is a signal; the slot's interface declares `hz` a vector of 2",
            ],
        ),
        (
            r#"{ slot = { inputs = { hz = { rate = 64 } } }, load = "voice" }"#,
            &[
                "project.toml:8:",
                "runs at rate divisor 1; the slot's interface declares `hz` at 64",
            ],
        ),
        (
            r#"{ slot = "lib.voice", load = "voice", map = { nope = "hz" } }"#,
            &[
                "project.toml:8:",
                "`map` key `nope` is not a port or parameter",
            ],
        ),
        (
            r#"{ slot = "lib.voice", load = "voice", map = { velocity = "nope" } }"#,
            &[
                "project.toml:8:",
                "`map` value `nope`: `voice` has no port or parameter `nope`",
            ],
        ),
        (
            r#"{ slot = "lib.voice", load = "voice", map = { velocity = "gate" } }"#,
            &[
                "project.toml:8:",
                "`gate` and `velocity` both match `voice`'s `gate`",
            ],
        ),
        (
            r#"{ slot = { inputs = { hz = "generic", gate = "gate" }, outputs = { out = "audio" } }, load = "std.audio_tap" }"#,
            &[
                "project.toml:8:",
                "warning",
                "`std.audio_tap` matches none of the slot's ports",
            ],
        ),
    ];
    for (i, (entry, expected)) in cases.iter().enumerate() {
        let found = problems(&voice_host(&format!("match-{i}"), entry, &[]));
        assert!(has(&found, expected), "{entry}: {found:#?}");
    }

    // A kind mismatch is a warning; the plan builds.
    let kind = r#"{ slot = { from = "lib.voice", outputs = { out = "gate" } }, load = "voice" }"#;
    let found = problems(&voice_host("match-kind", kind, &[]));
    assert!(
        has(
            &found,
            &[
                "project.toml:8:",
                "warning",
                "kind mismatch: `voice`'s `out` is audio; the slot's interface declares `out` gate"
            ]
        ),
        "{found:#?}"
    );
    // A wired slot port unmatched beside an unmatched content port of its
    // kind and direction: did you mean a `map`?
    let mean = r#"{ slot = { inputs = { pitch = "frequency", gate = "gate" }, outputs = { out = "audio" } }, load = "voice" }"#;
    let dir = voice_host("match-mean", mean, &[]);
    let text = fs::read_to_string(dir.join("project.toml"))
        .unwrap()
        .replace("to = \"synth.hz\"", "to = \"synth.pitch\"");
    fs::write(dir.join("project.toml"), text).unwrap();
    let found = problems(&dir);
    let warning = found.iter().find(|p| p.contains("did you mean")).cloned();
    assert!(
        warning
            .as_deref()
            .is_some_and(|w| w.contains("map = { pitch = \"") && w.contains("warning")),
        "{found:#?}"
    );
    // A value on the slot that is no interface parameter is an error.
    let value = r#"{ slot = "lib.voice", load = "voice", level = 2 }"#;
    let found = problems(&voice_host("match-value", value, &[]));
    assert!(
        has(
            &found,
            &[
                "project.toml:8:",
                "`level` is not a parameter of the slot's interface"
            ]
        ),
        "{found:#?}"
    );
}

/// ADR 0037 §2: the reserved keys and their misuse are refused when the
/// file is read.
#[test]
fn slot_and_project_keys_are_checked() {
    let cases: [(&str, &str); 5] = [
        (
            r#"{ module = "voice", slot = "lib.voice" }"#,
            "exactly one of `module`, `project` and `slot`",
        ),
        (
            r#"{ module = "voice", load = "voice" }"#,
            "`load` is a slot's key",
        ),
        (
            r#"{ module = "voice", seed = 1 }"#,
            "`seed` applies to an included project or a slot",
        ),
        (
            r#"{ slot = "lib.voice", load = "voice", seed = 1 }"#,
            "`seed` is a project's seed scope; this slot loads no project",
        ),
        (
            r#"{ slot = "lib.voice", load = "voice", gpu = "0" }"#,
            "`gpu` belongs on a primitive module instance",
        ),
    ];
    for (i, (entry, expected)) in cases.iter().enumerate() {
        let found = problems(&voice_host(&format!("keys-{i}"), entry, &[]));
        assert!(
            has(&found, &["project.toml:8:", expected]),
            "{entry}: {found:#?}"
        );
    }
    // In memory there is no folder to include from.
    let files = vec![signal_project::SourceFile {
        path: "project.toml".to_owned(),
        text:
            "schema_version = 1\n[project]\nname = \"x\"\n[instances]\nm = { project = \"../m\" }\n"
                .to_owned(),
    }];
    let found = Project::from_sources(files).err().unwrap();
    assert!(
        found[0].message.contains("cannot include another"),
        "{found:#?}"
    );
    assert_eq!(found[0].severity, Severity::Error);
}

/// ADR 0037 §3, test 7: the library templates read, with the ports the ADR
/// gives them.
#[test]
fn the_library_templates_read() {
    for (name, expected) in [
        ("voice", &["gate", "hz", "velocity", "out"][..]),
        ("instrument", &["gate", "hz", "velocity", "l", "r"][..]),
        (
            "deck",
            &["jump", "position", "running", "tempo", "l", "r", "release"][..],
        ),
    ] {
        let dir = scratch(&format!("lib-{name}"));
        let host = format!(
            "schema_version = 1\n[project]\nname = \"x\"\n[instances]\ns = {{ slot = \"lib.{name}\" }}\nout = {{ module = \"std.audio_out\" }}\n"
        );
        write(&dir, &[("project.toml", &host)]);
        let planned = load(&dir).plan(&registry()).unwrap();
        let ports: Vec<&str> = planned.slots[0]
            .interface
            .ports
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(ports, expected, "lib.{name}");
    }
    let dir = scratch("template-file");
    write(
        &dir,
        &[
            (
                "project.toml",
                "schema_version = 1\n[project]\nname = \"x\"\n[instances]\ns = { slot = \"deck\" }\nout = { module = \"std.audio_out\" }\n",
            ),
            ("interfaces/deck.toml", DECK),
            (
                "interfaces/bad.toml",
                "schema_version = 1\n[interface]\nname = \"good\"\n",
            ),
        ],
    );
    let found = problems(&dir);
    assert!(
        has(
            &found,
            &[
                "interfaces/bad.toml:3:",
                "interface `good` must be defined in interfaces/good.toml"
            ]
        ),
        "{found:#?}"
    );
}

/// The UI's `graph` and `describe` content for a host with an include and
/// a slot build (phase 3 draws them; phase 1 only must not fail).
#[test]
fn structure_and_content_build_for_includes_and_slots() {
    let host = noisy_host("structure", "", false, 48_000);
    let project = load(&host);
    let registry = registry();
    let planned = project.plan(&registry).unwrap();
    let graph = project.structure(&planned.plan, &registry);
    assert!(graph["top"].is_object(), "{graph}");
    let content = project.content_json(&registry, 96_000);
    assert_eq!(content["includes"][0]["key"], "m");
    assert_eq!(
        content["includes"][0]["note"],
        "noisy is written for 48000 Hz and plays at 96000 Hz here"
    );
    let slotted = voice_host(
        "structure-slot",
        r#"{ slot = "lib.voice", load = "voice" }"#,
        &[],
    );
    let project = load(&slotted);
    let planned = project.plan(&registry).unwrap();
    project.structure(&planned.plan, &registry);
    let content = project.content_json(&registry, 48_000);
    assert_eq!(content["slots"][0]["slot"], "/synth");
    assert_eq!(content["slots"][0]["interface"]["from"], "lib.voice");
    assert!(
        load(&host.join("noisy"))
            .content_json(&registry, 48_000)
            .is_null()
    );
}

/// ADR 0037 §2: a composite's exports that name its slot's ports are
/// renamed through the match like wires; through an empty slot they are
/// left out, with the wires on them in the definition using the composite;
/// an export naming a port the interface lacks is an error.
#[test]
fn composite_exports_through_a_slot() {
    let host = |rack: &str, wired: bool| {
        let dir = scratch(if wired { "rack" } else { "rack-plain" });
        let wires = if wired {
            "  { from = \"tone.out\", to = \"rack.x\" },\n  { from = \"rack.y\", to = \"out.l\" },\n"
        } else {
            ""
        };
        let project = format!(
            "schema_version = 1\n[project]\nname = \"host\"\n[instances]\ntone = {{ module = \"std.sine_osc\" }}\nrack = {{ module = \"rack\" }}\nout = {{ module = \"std.audio_out\" }}\n[wiring]\nconnections = [\n{wires}]\n"
        );
        write(
            &dir,
            &[("project.toml", &project), ("composites/rack.toml", rack)],
        );
        dir
    };
    let rack = |fx: &str, exports: &str| {
        format!(
            "schema_version = 1\n[composite]\nname = \"rack\"\n[instances]\n{fx}\nkeep = {{ module = \"std.sine_osc\" }}\n[exports]\n{exports}"
        )
    };
    let slot = r#"fx = { slot = { inputs = { in = "audio" }, outputs = { out = "audio" } }, load = "std.gain" }"#;
    let exports = "inputs = { x = \"fx.in\" }\noutputs = { y = \"fx.out\" }\n";
    let slotted = plan(&host(&rack(slot, exports), true));
    let explicit = plan(&host(
        &rack("fx = { module = \"std.gain\" }", exports),
        true,
    ));
    assert_eq!(dump(&slotted), dump(&explicit));

    let empty = r#"fx = { slot = { inputs = { in = "audio" }, outputs = { out = "audio" } } }"#;
    let emptied = plan(&host(&rack(empty, exports), true));
    let without = plan(&host(&rack("", ""), false));
    assert_eq!(
        dump(&emptied),
        dump(&without),
        "exports and wires through the empty slot go"
    );

    let bad = "inputs = { x = \"fx.nope\" }\n";
    let found = problems(&host(&rack(slot, bad), false));
    assert!(
        has(
            &found,
            &[
                "composites/rack.toml:8:",
                "`nope` is not a port of the slot's interface"
            ]
        ),
        "{found:#?}"
    );
}

/// ADR 0035 §1: a `set` key naming a parameter the inner instance lacks is
/// an error at the key.
#[test]
fn an_unknown_set_parameter_is_located_at_its_key() {
    let host = noisy_host("set-unknown", ", set = { \"a.nope\" = 1 }", false, 48_000);
    let found = problems(&host);
    assert!(
        has(
            &found,
            &[
                "project.toml:9:34:",
                "`set` key `a.nope`: `a` of noisy has no parameter `nope`"
            ]
        ),
        "{found:#?}"
    );
}

/// ADR 0035 §2: an export of the sink's tap output (`out.l_out`) loads
/// alone, and included it is the tap's output.
#[test]
fn a_project_may_export_its_tap_outputs() {
    let dir = scratch("tap-exports");
    let inner = r#"schema_version = 1
[project]
name = "inner"
[instances]
osc = { module = "std.sine_osc" }
out = { module = "std.audio_out" }
[wiring]
connections = [ { from = "osc.out", to = "out.l" } ]
[exports]
outputs = { left = "out.l_out" }
"#;
    let host = r#"schema_version = 1
[project]
name = "host"
[instances]
i = { project = "inner" }
out = { module = "std.audio_out" }
[wiring]
connections = [ { from = "i.left", to = "out.l" } ]
"#;
    write(
        &dir,
        &[("project.toml", host), ("inner/project.toml", inner)],
    );
    plan(&dir.join("inner"));
    let hosted = plan(&dir);
    assert_eq!(
        hosted.sources[&"/out.l".parse::<PortAddress>().unwrap()][0]
            .from
            .as_str(),
        "/i/out.l_out"
    );
}

/// ADR 0035 §1: the instance keys are reserved as export names.
#[test]
fn reserved_keys_cannot_be_exported() {
    for name in ["seed", "set", "project", "slot", "load", "map", "module"] {
        let dir = scratch(&format!("reserved-{name}"));
        let project = format!(
            "schema_version = 1\n[project]\nname = \"x\"\n[instances]\nosc = {{ module = \"std.sine_osc\" }}\nout = {{ module = \"std.audio_out\" }}\n[exports]\nparameters = {{ {name} = \"osc.freq\" }}\n"
        );
        write(&dir, &[("project.toml", &project)]);
        let found = problems(&dir);
        assert!(
            has(
                &found,
                &[
                    "project.toml:8:",
                    &format!("cannot export a parameter named `{name}`")
                ]
            ),
            "{found:#?}"
        );
    }
}

/// ADR 0035 §13 (phase 4 not yet): `signal pack` refuses a project with
/// includes rather than writing an archive that does not load.
#[test]
fn pack_refuses_a_project_with_includes() {
    let host = noisy_host("pack", "", false, 48_000);
    let error = signal_project::archive::pack(&host).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("includes other projects (noisy)"),
        "{error}"
    );
    signal_project::archive::pack(&host.join("noisy")).unwrap();
}
