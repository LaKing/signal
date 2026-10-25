//! Error messages with file:line:column for each kind of mistake, including
//! graph errors mapped back to instances and connections, and errors in
//! composites the project does not use.

use signal_core::Registry;
use signal_project::{Project, SourceFile};

const VALID: &str = r#"schema_version = 1

[project]
name = "t"
sample_rate = 48000

[instances]
osc = { module = "std.sine_osc", freq = 440 }
out = { module = "std.audio_out" }

[wiring]
connections = [
  { from = "osc.out", to = "out.l" },
  { from = "osc.out", to = "out.r" },
]
"#;

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn sources(files: &[(&str, &str)]) -> Vec<SourceFile> {
    files
        .iter()
        .map(|(path, text)| SourceFile {
            path: (*path).to_owned(),
            text: (*text).to_owned(),
        })
        .collect()
}

/// The diagnostics of loading and planning `files`, one per line.
fn errors(files: &[(&str, &str)]) -> String {
    let diagnostics = match Project::from_sources(sources(files)) {
        Err(diagnostics) => diagnostics,
        Ok(project) => match project.plan(&registry()) {
            Err(diagnostics) => diagnostics,
            Ok(_) => panic!("expected errors"),
        },
    };
    diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn project_errors(text: &str) -> String {
    errors(&[("project.toml", text)])
}

#[test]
fn valid_project_plans() {
    let project = Project::from_sources(sources(&[("project.toml", VALID)])).unwrap();
    let planned = project.plan(&registry()).unwrap();
    assert_eq!(planned.plan.instances.len(), 2);
    assert!(planned.warnings.is_empty());
}

/// M2.8-6 (ADR 0005): a connection across rate divisors plans through an
/// inserted converter, reported as a warning at the connection as written.
#[test]
fn an_inserted_converter_is_a_warning_at_its_connection() {
    let mut registry = registry();
    let lfo = *registry.get("std.lfo").unwrap();
    let mut descriptor = lfo.descriptor;
    let d64 = signal_core::RateDivisor::new(64).unwrap();
    descriptor.type_name = "test.slow_lfo";
    descriptor.ports = Box::leak(
        descriptor
            .ports
            .iter()
            .map(|p| signal_core::PortDescriptor { rate: d64, ..*p })
            .collect(),
    );
    descriptor.parameters = Box::leak(
        descriptor
            .parameters
            .iter()
            .map(|p| signal_core::ParameterDescriptor { rate: d64, ..*p })
            .collect(),
    );
    registry
        .register(signal_core::ModuleType {
            descriptor,
            create: lfo.create,
        })
        .unwrap();
    let text = VALID
        .replace(
            "out = { module",
            "lfo = { module = \"test.slow_lfo\" }\nout = { module",
        )
        .replace(
            "  { from = \"osc.out\", to = \"out.r\" },",
            "  { from = \"osc.out\", to = \"out.r\" },\n  { from = \"lfo.out\", to = \"osc.freq\", scale = 10 },",
        );
    let project = Project::from_sources(sources(&[("project.toml", &text)])).unwrap();
    let planned = project.plan(&registry).unwrap();
    let warnings: Vec<String> = planned.warnings.iter().map(ToString::to_string).collect();
    // The kind mismatch (control into a frequency) names the connection as
    // written too, not the converter's port.
    assert_eq!(
        warnings,
        [
            "project.toml:16:3: warning: /lfo.out -> /osc.freq: inserted converter /lfo/out_d1 (hold, rate divisor 64 -> 1)",
            "project.toml:16:3: warning: /lfo.out (Control) -> /osc.freq (Frequency): kind mismatch",
        ]
    );
    assert!(
        planned
            .plan
            .instances
            .iter()
            .any(|i| i.type_name == "core.hold.d64.d1.signal.control")
    );
}

#[test]
fn syntax_error() {
    let text = VALID.replace(r#"name = "t""#, r#"name = "t"#);
    insta::assert_snapshot!("syntax_error", project_errors(&text));
}

#[test]
fn unknown_key() {
    let text = VALID.replace("sample_rate", "sample_rat");
    insta::assert_snapshot!("unknown_key", project_errors(&text));
}

#[test]
fn unknown_module() {
    let text = VALID.replace("std.sine_osc", "std.sine");
    insta::assert_snapshot!("unknown_module", project_errors(&text));
}

/// Audit P1 (`docs/semantics.md` §16): a converter named with a vector too
/// wide to allocate is refused at its module, for its width.
#[test]
fn a_vector_too_wide_is_an_error_at_its_module() {
    let text = VALID.replace(
        "std.sine_osc\", freq = 440",
        "core.hold.d2.d1.vector4294967295.generic\"",
    );
    assert_eq!(
        project_errors(&text),
        "project.toml:8:18: error: /osc: `core.hold.d2.d1.vector4294967295.generic` has a \
         vector of 4294967295 elements; a vector has at most 4096"
    );
}

/// M2.10-2 AC6: the vector types exist at the registered widths only, so a
/// width outside `signal_std::VECTOR_WIDTHS` is an unknown module.
#[test]
fn an_unregistered_vector_width_is_an_unknown_module() {
    let text = VALID.replace("std.sine_osc\", freq = 440", "std.pack3\"");
    assert_eq!(
        project_errors(&text),
        "project.toml:8:18: error: /osc: unknown module `std.pack3` (neither a registered \
         primitive type nor a composite)"
    );
}

/// M2.10-2 AC3: a table's values are its parameters, validated when the
/// project loads: one outside ±1e6 is refused at the value.
#[test]
fn a_table_value_out_of_range_is_refused_at_load() {
    let text = VALID.replace(
        "osc = { module = \"std.sine_osc\", freq = 440 }",
        "osc = { module = \"std.sine_osc\", freq = 440 }\ntab = { module = \"std.table4\", v0 = 1, v2 = 2e6 }",
    );
    assert_eq!(
        project_errors(&text),
        "project.toml:9:45: error: /tab.v2: value 2000000 is outside [-100000, 100000]"
    );
}

#[test]
fn unknown_port() {
    let text = VALID.replacen(r#"from = "osc.out""#, r#"from = "osc.output""#, 1);
    insta::assert_snapshot!("unknown_port", project_errors(&text));
}

#[test]
fn out_of_range_parameter() {
    let text = VALID.replace(
        r#"{ module = "std.audio_out" }"#,
        r#"{ module = "std.audio_out", gain = 5 }"#,
    );
    insta::assert_snapshot!("out_of_range_parameter", project_errors(&text));
}

#[test]
fn delay_free_feedback_cycle() {
    let text = r#"schema_version = 1

[project]
name = "loop"

[instances]
a = { module = "std.add" }
b = { module = "std.add" }
out = { module = "std.audio_out" }

[wiring]
connections = [
  { from = "a.out", to = "b.a" },
  { from = "b.out", to = "a.a" },
  { from = "b.out", to = "out.l" },
]
"#;
    insta::assert_snapshot!("delay_free_feedback_cycle", project_errors(text));
}

const VOICE_PROJECT: &str = r#"schema_version = 1

[project]
name = "t"

[instances]
v = { module = "voice", drive = 50 }
out = { module = "std.audio_out" }

[wiring]
connections = [{ from = "v.out", to = "out.l" }]
"#;

const VOICE: &str = r#"schema_version = 1

[composite]
name = "voice"

[instances]
osc = { module = "std.sine_osc" }
shaper = { module = "std.tanh_shaper" }

[wiring]
connections = [{ from = "osc.out", to = "shaper.in" }]

[exports]
outputs = { out = "shaper.out" }
parameters = { drive = "shaper.drive" }
"#;

/// An error inside a used composite names the composite's file.
#[test]
fn error_in_used_composite() {
    let voice = VOICE.replace(
        r#"{ module = "std.sine_osc" }"#,
        r#"{ module = "std.nope" }"#,
    );
    let project = VOICE_PROJECT.replace("drive = 50", "drive = 2");
    insta::assert_snapshot!(
        "error_in_used_composite",
        errors(&[
            ("project.toml", &project),
            ("composites/voice.toml", &voice)
        ])
    );
}

/// A value set through a parameter export points at the outer key's value.
#[test]
fn out_of_range_through_export() {
    insta::assert_snapshot!(
        "out_of_range_through_export",
        errors(&[
            ("project.toml", VOICE_PROJECT),
            ("composites/voice.toml", VOICE)
        ])
    );
}

const SPARE: &str = r#"schema_version = 1

[composite]
name = "spare"

[instances]
osc = { module = "std.sine_osc" }

[exports]
outputs = { out = "osc.out" }
"#;

/// A composite no instance uses is still validated: an unknown parameter key
/// in it is an error at the key.
#[test]
fn unused_composite_unknown_parameter() {
    let spare = SPARE.replace(
        r#"{ module = "std.sine_osc" }"#,
        r#"{ module = "std.sine_osc", typo = 1 }"#,
    );
    insta::assert_snapshot!(
        "unused_composite_unknown_parameter",
        errors(&[("project.toml", VALID), ("composites/spare.toml", &spare)])
    );
}

/// Unknown modules, ports and export targets in an unused composite are
/// errors too (each on its own instance, so none hides another).
#[test]
fn unused_composite_other_errors() {
    let spare = r#"schema_version = 1

[composite]
name = "spare"

[instances]
osc = { module = "std.sine_osc" }
lost = { module = "std.nope" }
shaper = { module = "std.tanh_shaper" }

[wiring]
connections = [{ from = "osc.output", to = "shaper.in" }]

[exports]
outputs = { out = "ghost.out" }
"#;
    insta::assert_snapshot!(
        "unused_composite_other_errors",
        errors(&[("project.toml", VALID), ("composites/spare.toml", spare)])
    );
}

/// The control for the two cases above: the valid composite file is accepted,
/// so the errors come from the defects alone.
#[test]
fn unused_valid_composite_is_accepted() {
    assert!(plans(&[
        ("project.toml", VALID),
        ("composites/spare.toml", SPARE)
    ]));
}

/// Whether `files` load and plan without errors.
fn plans(files: &[(&str, &str)]) -> bool {
    let project = Project::from_sources(sources(files)).unwrap();
    project.plan(&registry()).is_ok()
}

/// Two connections between the same ports: the non-finite scale is reported at
/// the connection that has it (line 13), not at the first one.
#[test]
fn non_finite_scale_on_a_repeated_connection() {
    let text = r#"schema_version = 1

[project]
name = "t"

[instances]
osc = { module = "std.sine_osc" }
out = { module = "std.audio_out" }

[wiring]
connections = [
  { from = "osc.out", to = "out.l", scale = 1.0 },
  { from = "osc.out", to = "out.l", scale = nan },
]
"#;
    insta::assert_snapshot!(
        "non_finite_scale_on_a_repeated_connection",
        project_errors(text)
    );
    assert!(plans(&[("project.toml", &text.replace("nan", "1.0"))]));
}

/// Two exported parameters alias one inner parameter. Values are assigned in
/// key order, so `z` wins over `a`, and the range error points at `z`'s value.
#[test]
fn out_of_range_through_aliased_exports() {
    let voice = VOICE.replace(
        r#"parameters = { drive = "shaper.drive" }"#,
        r#"parameters = { a = "shaper.drive", z = "shaper.drive" }"#,
    );
    let project = VOICE_PROJECT.replace("drive = 50", "a = 1, z = 50");
    insta::assert_snapshot!(
        "out_of_range_through_aliased_exports",
        errors(&[
            ("project.toml", &project),
            ("composites/voice.toml", &voice)
        ])
    );
    let fixed = project.replace("z = 50", "z = 5");
    assert!(plans(&[
        ("project.toml", &fixed),
        ("composites/voice.toml", &voice)
    ]));
}

/// M2.8-9 AC6 (ADR 0024): a bridge input takes one `std.count.window`, at
/// scale 1. A scale other than 1, any fan-in (auditor-1's R1 + R2 − R3
/// included) and another source are refused when the plan is built, each
/// at the connection as written.
fn bridge(wiring: &str) -> String {
    format!(
        r#"schema_version = 1

[project]
name = "t"

[instances]
c = {{ module = "std.count" }}
c2 = {{ module = "std.count" }}
c3 = {{ module = "std.count" }}
abs = {{ module = "std.abs" }}
p = {{ module = "std.particles" }}

[wiring]
connections = [
{wiring}
]
"#
    )
}

#[test]
fn a_misconnected_bridge_input_is_refused_as_written() {
    let takes =
        |to: &str| format!("{to} takes one connection, at scale 1, from the window of a std.count");
    let emit = takes("/p.emit");
    for (wiring, expected) in [
        (
            r#"  { from = "c.window", to = "p.emit", scale = 2 },"#,
            format!("project.toml:15:3: error: /c.window -> /p.emit: {emit}; this one has scale 2"),
        ),
        (
            r#"  { from = "c.window", to = "p.impulse", scale = 0.5 },"#,
            format!(
                "project.toml:15:3: error: /c.window -> /p.impulse: {}; this one has scale 0.5",
                takes("/p.impulse")
            ),
        ),
        (
            "  { from = \"c.window\", to = \"p.emit\" },\n  { from = \"c2.window\", to = \"p.emit\" },",
            format!(
                "project.toml:15:3: error: /c.window -> /p.emit: {emit}; it has 2 connections\n\
                 project.toml:16:3: error: /c2.window -> /p.emit: {emit}; it has 2 connections"
            ),
        ),
        // Auditor-1's R1 + R2 − R3, which the check rows alone let through.
        (
            "  { from = \"c.window\", to = \"p.emit\" },\n  { from = \"c2.window\", to = \"p.emit\" },\n  { from = \"c3.window\", to = \"p.emit\", scale = -1 },",
            format!(
                "project.toml:15:3: error: /c.window -> /p.emit: {emit}; it has 3 connections\n\
                 project.toml:16:3: error: /c2.window -> /p.emit: {emit}; it has 3 connections\n\
                 project.toml:17:3: error: /c3.window -> /p.emit: {emit}; it has 3 connections"
            ),
        ),
        (
            r#"  { from = "abs.out", to = "p.emit" },"#,
            format!(
                "project.toml:15:3: error: /abs.out -> /p.emit: {emit}; this one comes from the out of a std.abs"
            ),
        ),
    ] {
        assert_eq!(project_errors(&bridge(wiring)), expected, "{wiring}");
    }
    // Through a composite's input export: the connection as written.
    let sparks = r#"schema_version = 1

[composite]
name = "sparks"

[instances]
p = { module = "std.particles" }

[exports]
inputs = { emit = "p.emit" }
"#;
    let project = bridge("").replace(
        "p = { module = \"std.particles\" }",
        "s = { module = \"sparks\" }",
    );
    let project = project.replace(
        "[\n\n]",
        "[\n  { from = \"c.window\", to = \"s.emit\", scale = 2 },\n]",
    );
    assert_eq!(
        errors(&[
            ("project.toml", &project),
            ("composites/sparks.toml", sparks)
        ]),
        format!(
            "project.toml:15:3: error: /c.window -> /s.emit: {}; this one has scale 2",
            takes("/s.emit")
        )
    );
    assert!(plans(&[(
        "project.toml",
        &bridge(
            "  { from = \"c.window\", to = \"p.emit\" },\n  { from = \"c.window\", to = \"p.impulse\", scale = 1.0 },"
        )
    )]));
    assert!(plans(&[("project.toml", &bridge(""))]));
}
