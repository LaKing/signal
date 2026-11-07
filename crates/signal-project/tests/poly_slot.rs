//! ADR 0044 phase 1, step 3: a slot with `voices` resolves into an
//! ordinary instance of a generated composite (§5).
//!
//! - Equivalence (test 2): the plan is byte-identical to the explicit form,
//!   a project composite holding the same instances, wires and exports.
//! - The side table: `voices` with its count, legato, definition and voice
//!   addresses; the `alloc` port matched to no content port.
//! - Errors (§1, §2): a count outside 1 to 64, `sleep`, an `assign` or
//!   `steal` not built, `voices` on a module instance, roles on the wrong
//!   port, a host wire on an `each` port, a `map` of the `alloc` port,
//!   events outputs; warnings for a slot whose voices all get the same
//!   inputs and for an `alloc` port without voices.
//! - Values: interface parameters and `set` reach every voice.

use signal_core::Registry;
use signal_graph::ExecutionPlan;
use signal_project::{Planned, Project, SourceFile};

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

/// A voice: `hz`, `gate` and `velocity` in, `bright` (a shared input),
/// `out`, and the parameters `level` and `tone`.
const TONE: &str = r#"schema_version = 1
[composite]
name = "tone"
[instances]
osc = { module = "std.sine_osc" }
amp = { module = "std.mul" }
vel = { module = "std.mul" }
vol = { module = "std.gain" }
shade = { module = "std.gain" }
[wiring]
connections = [
  { from = "osc.out", to = "amp.a" },
  { from = "amp.out", to = "vel.a" },
  { from = "vel.out", to = "vol.in" },
]
[exports]
inputs = { hz = "osc.freq", gate = "amp.b", velocity = "vel.b", bright = "shade.in" }
outputs = { out = "vol.out" }
parameters = { level = "vol.db", tone = "shade.db" }
"#;

/// The poly slot's inline interface.
const INTERFACE: &str = r#"{ inputs = { notes = { shape = "events", kind = "trigger", voice = "alloc" }, hz = { kind = "frequency", voice = "each" }, gate = { kind = "gate", voice = "each" }, velocity = { kind = "control", voice = "each" }, bright = "control" }, outputs = { out = "audio" }, parameters = { level = {} } }"#;

/// A host of `synth` (written after `synth = `) fed by an edge's events
/// and a constant, its output into a gain.
fn host(synth: &str) -> String {
    format!(
        r#"schema_version = 1
[project]
name = "host"
[instances]
ed = {{ module = "std.edge" }}
c = {{ module = "std.const", value = 0.5 }}
synth = {synth}
post = {{ module = "std.gain" }}
[wiring]
connections = [
  {{ from = "ed.rise", to = "synth.notes" }},
  {{ from = "c.out", to = "synth.bright" }},
  {{ from = "synth.out", to = "post.in" }},
]
"#
    )
}

fn poly(voices: &str, extra: &str) -> String {
    host(&format!(
        "{{ slot = {INTERFACE}, load = \"tone\", voices = {voices}{extra} }}"
    ))
}

fn sources(project: &str, extra: &[(&str, &str)]) -> Vec<SourceFile> {
    [("project.toml", project), ("composites/tone.toml", TONE)]
        .into_iter()
        .chain(extra.iter().copied())
        .map(|(path, text)| SourceFile {
            path: path.to_owned(),
            text: text.to_owned(),
        })
        .collect()
}

fn try_plan(project: &str, extra: &[(&str, &str)]) -> Result<Planned, Vec<String>> {
    let text_of = |found: Vec<signal_project::Diagnostic>| {
        found.iter().map(ToString::to_string).collect::<Vec<_>>()
    };
    let project = Project::from_sources(sources(project, extra)).map_err(text_of)?;
    project.plan(&registry()).map_err(text_of)
}

fn planned(project: &str, extra: &[(&str, &str)]) -> Planned {
    try_plan(project, extra).unwrap_or_else(|e| panic!("{project}\n{e:#?}"))
}

fn errors(project: &str) -> Vec<String> {
    match try_plan(project, &[]) {
        Ok(_) => panic!("plans:\n{project}"),
        Err(found) => found,
    }
}

fn has(found: &[String], parts: &[&str]) -> bool {
    found.iter().any(|f| parts.iter().all(|p| f.contains(p)))
}

fn dump(plan: &ExecutionPlan) -> String {
    serde_json::to_string_pretty(plan).unwrap()
}

/// The explicit form of three voices of `tone` with `legato` off.
const EXPLICIT: &str = r#"schema_version = 1
[composite]
name = "explicit"
[instances]
alloc = { module = "std.voice_alloc", voices = 3, legato = 0 }
each_hz = { module = "std.unpack64" }
each_gate = { module = "std.unpack64" }
each_velocity = { module = "std.unpack64" }
v0 = { module = "tone" }
v1 = { module = "tone" }
v2 = { module = "tone" }
sum_out_1 = { module = "std.add" }
sum_out = { module = "std.add" }
[wiring]
connections = [
  { from = "alloc.hz", to = "each_hz.in" },
  { from = "each_hz.e0", to = "v0.hz" },
  { from = "each_hz.e1", to = "v1.hz" },
  { from = "each_hz.e2", to = "v2.hz" },
  { from = "alloc.gate", to = "each_gate.in" },
  { from = "each_gate.e0", to = "v0.gate" },
  { from = "each_gate.e1", to = "v1.gate" },
  { from = "each_gate.e2", to = "v2.gate" },
  { from = "alloc.velocity", to = "each_velocity.in" },
  { from = "each_velocity.e0", to = "v0.velocity" },
  { from = "each_velocity.e1", to = "v1.velocity" },
  { from = "each_velocity.e2", to = "v2.velocity" },
  { from = "v0.out", to = "sum_out_1.a" },
  { from = "v1.out", to = "sum_out_1.b" },
  { from = "sum_out_1.out", to = "sum_out.a" },
  { from = "v2.out", to = "sum_out.b" },
]
[exports]
inputs = { notes = "alloc.notes", bright = ["v0.bright", "v1.bright", "v2.bright"] }
outputs = { out = "sum_out.out" }
parameters = { level = ["v0.level", "v1.level", "v2.level"], tone = ["v0.tone", "v1.tone", "v2.tone"] }
"#;

/// Test 2: the generated composite plans exactly as the hand-written one.
#[test]
fn a_poly_slot_plans_as_its_explicit_composite() {
    let generated = planned(&poly("3", ", level = -6, set = { tone = 2 }"), &[]);
    let explicit = planned(
        &host("{ module = \"explicit\", level = -6, tone = 2 }"),
        &[("composites/explicit.toml", EXPLICIT)],
    );
    assert_eq!(dump(&generated.plan), dump(&explicit.plan));
    // Every voice is at `/synth/v<e>`, beside the allocator, the unpacks and
    // the sums.
    let addresses: Vec<&str> = generated
        .plan
        .instances
        .iter()
        .map(|i| i.address.as_str())
        .collect();
    for address in [
        "/synth/alloc",
        "/synth/each_hz",
        "/synth/v0/osc",
        "/synth/v2/vol",
        "/synth/sum_out",
    ] {
        assert!(addresses.contains(&address), "{address}: {addresses:#?}");
    }
}

/// One voice: one add with `b` unconnected, and the allocator's legato.
#[test]
fn one_voice_with_legato() {
    let planned = planned(&poly("{ count = 1, legato = true }", ""), &[]);
    let alloc = planned
        .plan
        .instances
        .iter()
        .find(|i| i.address.as_str() == "/synth/alloc")
        .expect("the allocator");
    let text = serde_json::to_string(alloc).unwrap();
    assert!(text.contains("\"std.voice_alloc\""), "{text}");
    let addresses: Vec<&str> = planned
        .plan
        .instances
        .iter()
        .map(|i| i.address.as_str())
        .collect();
    assert!(addresses.contains(&"/synth/v0/osc"), "{addresses:#?}");
    assert!(!addresses.iter().any(|a| a.starts_with("/synth/v1/")));
    assert!(addresses.contains(&"/synth/sum_out"), "{addresses:#?}");
    let slot = &planned.slots[0];
    let voices = slot.voices.as_ref().expect("voices");
    assert_eq!((voices.count, voices.legato), (1, true));
}

/// The side table: the voices, and the `alloc` port matched to nothing.
#[test]
fn the_side_table_records_the_voices() {
    let planned = planned(&poly("4", ""), &[]);
    let slot = planned
        .slots
        .iter()
        .find(|s| s.slot == "/synth")
        .expect("the slot");
    let voices = slot.voices.as_ref().expect("voices");
    assert_eq!(voices.count, 4);
    assert!(!voices.legato);
    assert_eq!(voices.definition, "poly:project/synth");
    assert_eq!(voices.voices, ["v0", "v1", "v2", "v3"]);
    let notes = slot.matches.iter().find(|m| m.port == "notes").unwrap();
    assert_eq!(notes.content, None);
    assert_eq!(
        notes.reason.as_deref(),
        Some("it feeds the slot's allocator")
    );
    let hz = slot.matches.iter().find(|m| m.port == "hz").unwrap();
    assert_eq!(hz.content.as_deref(), Some("hz"));
    assert!(slot.dropped.is_empty(), "{:#?}", slot.dropped);
    // A slot without `voices` records none, and its `voice` fields are
    // ignored: `notes` matches nothing in `tone`, and its wire is dropped.
    let mono = self::planned(
        &host(&format!("{{ slot = {INTERFACE}, load = \"tone\" }}")),
        &[],
    );
    assert!(mono.slots[0].voices.is_none());
    assert_eq!(mono.slots[0].dropped, ["ed.rise -> synth.notes"]);
    assert!(
        has(
            &mono
                .warnings
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            &["/synth: `notes` feeds nothing: the slot has no voices"]
        ),
        "{:#?}",
        mono.warnings
    );
}

/// §1: `voices` out of range, the keys not built, and `voices` off a slot.
#[test]
fn voices_is_checked_at_the_value() {
    for (voices, message) in [
        ("0", "`voices` is a count from 1 to 64; 0 is outside it"),
        ("65", "`voices` is a count from 1 to 64; 65 is outside it"),
        ("{ count = 2, sleep = true }", "`sleep` is not yet built"),
        (
            "{ count = 2, assign = \"lowest\" }",
            "only `\"rotate\"` is built",
        ),
        (
            "{ count = 2, steal = \"none\" }",
            "only `\"oldest\"` is built",
        ),
        ("{ legato = true }", "missing field `count`"),
        ("{ count = 2, spread = 1 }", "unknown field `spread`"),
    ] {
        let found = errors(&poly(voices, ""));
        assert!(
            has(&found, &["project.toml:", message]),
            "{voices}: {found:#?}"
        );
    }
    // Off a slot: a number on a module instance is a parameter value (the
    // allocator's own `voices`), the table is refused, and elsewhere the
    // key is a slot's.
    let found = errors(&host("{ module = \"tone\", voices = 2 }"));
    assert!(has(&found, &["voices"]), "{found:#?}");
    let found = errors(&host("{ module = \"tone\", voices = { count = 2 } }"));
    assert!(
        has(&found, &["a `voices` table is a slot's key"]),
        "{found:#?}"
    );
    let found = errors(&host("{ device = { play = \"stereo\" }, voices = 2 }"));
    assert!(has(&found, &["`voices` is a slot's key"]), "{found:#?}");
    let found = errors(&poly("2.5", ""));
    assert!(
        has(
            &found,
            &["`voices` is a whole count from 1 to 64; 2.5 is not"]
        ),
        "{found:#?}"
    );
}

/// §2: roles on the wrong port, a host wire on an `each` port, a `map` of
/// the `alloc` port, an events output.
#[test]
fn roles_are_checked() {
    let with = |interface: &str, wires: &str| {
        format!(
            r#"schema_version = 1
[project]
name = "host"
[instances]
ed = {{ module = "std.edge" }}
c = {{ module = "std.const" }}
synth = {{ slot = {interface}, load = "tone", voices = 2 }}
[wiring]
connections = [{wires}]
"#
        )
    };
    let cases = [
        (
            r#"{ outputs = { out = { kind = "audio", voice = "each" } } }"#,
            "`out` is `each`; only an input may be",
        ),
        (
            r#"{ inputs = { bright = { voice = "each" } } }"#,
            "`bright` is `each`, fed by the allocator's output of its name",
        ),
        (
            r#"{ inputs = { hz = { shape = { vector = 4 }, voice = "each" } } }"#,
            "`hz` is `each` and a vector of 4",
        ),
        (
            r#"{ inputs = { bright = { voice = "alloc" } } }"#,
            "the allocator's input takes note events",
        ),
        (
            r#"{ inputs = { a = { shape = "events", voice = "alloc" }, b = { shape = "events", voice = "alloc" } } }"#,
            "`b` is a second `alloc` port",
        ),
        (
            r#"{ inputs = { bright = { voice = "sum" } } }"#,
            "`bright` is `sum`; only an output may be",
        ),
        (
            r#"{ outputs = { out = { kind = "audio", voice = "shared" } } }"#,
            "`out` is `shared`; only an input may be",
        ),
        (
            r#"{ outputs = { out = "events" } }"#,
            "merging events is not yet built",
        ),
    ];
    for (interface, message) in cases {
        let found = errors(&with(interface, ""));
        assert!(
            has(&found, &["project.toml:", message]),
            "{interface}: {found:#?}"
        );
    }
    let found = errors(&with(
        r#"{ inputs = { hz = { kind = "frequency", voice = "each" } } }"#,
        r#"{ from = "c.out", to = "synth.hz" }"#,
    ));
    assert!(
        has(&found, &["`hz` is fed by the slot's allocator"]),
        "{found:#?}"
    );
    let found = errors(&poly("2", ", map = { notes = \"gate\" }"));
    assert!(
        has(
            &found,
            &["`notes` feeds the slot's allocator; it has no content port"]
        ),
        "{found:#?}"
    );
}

/// §2: the warnings of a slot whose voices all get the same inputs, and of
/// one from a mono template.
#[test]
fn voices_without_an_allocator_warn() {
    let text = host(
        r#"{ slot = { inputs = { bright = "control", notes = "events" }, outputs = { out = "audio" } }, load = "tone", voices = 2 }"#,
    );
    let planned = planned(&text, &[]);
    let warnings: Vec<String> = planned.warnings.iter().map(ToString::to_string).collect();
    assert!(
        has(&warnings, &["every voice gets the same inputs"]),
        "{warnings:#?}"
    );
    let text = host(
        r#"{ slot = { from = "lib.voice", inputs = { notes = "events", bright = "control" } }, load = "tone", voices = 2 }"#,
    );
    let planned = self::planned(&text, &[]);
    let warnings: Vec<String> = planned.warnings.iter().map(ToString::to_string).collect();
    assert!(
        has(&warnings, &["did you mean `lib.poly_voice`?"]),
        "{warnings:#?}"
    );
}

/// A port's written role reaches the resolved interface, and the
/// format's limit is the allocator's width.
#[test]
fn the_role_is_on_the_resolved_port() {
    assert_eq!(signal_std::MAX_VOICES, 64);
    let project = Project::from_sources(sources(&poly("2", ""), &[])).unwrap();
    let planned = project.plan(&registry()).unwrap();
    let hz = planned.slots[0]
        .interface
        .ports
        .iter()
        .find(|p| p.name == "hz")
        .unwrap();
    assert_eq!(hz.voice, Some(signal_project::Voice::Each));
}
