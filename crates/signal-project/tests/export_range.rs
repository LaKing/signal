//! ADR 0042: an exported parameter with its own range. Both forms of an
//! export parse; a range outside a target's (one target, or any of a
//! fan-out) is refused at the export; an export without one takes an inner
//! export's; a value in a file outside an export's range is an error at the
//! value, as for a primitive's; an exported input takes no range; and the
//! studio projects, whose composites now declare ranges, load and plan with
//! every value they hold inside them.

use std::path::Path;

use signal_core::{Range, Registry};
use signal_project::{Project, SourceFile};

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

/// The diagnostics of loading and planning `files` (empty when both work).
fn problems(files: &[(&str, &str)]) -> Vec<String> {
    let project = match Project::from_sources(sources(files)) {
        Ok(project) => project,
        Err(diagnostics) => return diagnostics.iter().map(ToString::to_string).collect(),
    };
    match project.plan(&registry()) {
        Ok(_) => Vec::new(),
        Err(diagnostics) => diagnostics.iter().map(ToString::to_string).collect(),
    }
}

fn project(instance: &str) -> String {
    format!(
        r#"schema_version = 1

[project]
name = "ranges"
sample_rate = 48000

[instances]
{instance}
out = {{ module = "std.audio_out" }}

[wiring]
connections = [{{ from = "v.out", to = "out.l" }}]
"#
    )
}

/// A voice whose `attack` scales a sine through a `std.scale` (as kick909's
/// click), with `exports` as its `[exports]` parameters line.
fn voice(parameters: &str) -> String {
    format!(
        r#"schema_version = 1

[composite]
name = "voice"

[instances]
osc = {{ module = "std.sine_osc", freq = 100 }}
click = {{ module = "std.scale", mul = 0.5 }}
shaper = {{ module = "std.tanh_shaper" }}

[wiring]
connections = [
  {{ from = "osc.out", to = "click.in" }},
  {{ from = "click.out", to = "shaper.in" }},
]

[exports]
outputs = {{ out = "shaper.out" }}
parameters = {parameters}
"#
    )
}

fn plan_ok(files: &[(&str, &str)]) -> Project {
    let project = Project::from_sources(sources(files)).unwrap();
    project.plan(&registry()).unwrap();
    project
}

#[test]
fn both_forms_parse_and_the_table_carries_its_range() {
    let voice = voice(
        r#"{ attack = { to = "click.mul", min = 0, max = 2 }, freq = "osc.freq", drive = { to = "shaper.drive", group = "tone" } }"#,
    );
    let project = plan_ok(&[
        (
            "project.toml",
            &project(r#"v = { module = "voice", attack = 1.5 }"#),
        ),
        ("composites/voice.toml", &voice),
    ]);
    let exports = &project.composites[0].exports.parameters;
    let range = |name: &str| exports.iter().find(|e| e.name == name).unwrap().range;
    assert_eq!(range("attack"), Some(Range { min: 0.0, max: 2.0 }));
    assert_eq!(range("freq"), None, "the plain form has none");
    assert_eq!(range("drive"), None, "a group alone has none");
}

#[test]
fn a_range_needs_both_ends_finite_and_ordered() {
    for (table, want) in [
        (r#"{ to = "click.mul", min = 0 }"#, "both `min` and `max`"),
        (r#"{ to = "click.mul", max = 2 }"#, "both `min` and `max`"),
        (
            r#"{ to = "click.mul", min = 3, max = 2 }"#,
            "is above its `max`",
        ),
        (
            r#"{ to = "click.mul", min = 0, max = inf }"#,
            "must be finite",
        ),
    ] {
        let voice = voice(&format!("{{ attack = {table} }}"));
        let errors = problems(&[
            ("project.toml", &project(r#"v = { module = "voice" }"#)),
            ("composites/voice.toml", &voice),
        ]);
        assert!(
            errors.iter().any(|e| e.contains(want)),
            "{table}: {errors:?}"
        );
        assert!(
            errors
                .iter()
                .all(|e| e.starts_with("composites/voice.toml:")),
            "{errors:?}"
        );
    }
}

#[test]
fn a_range_outside_its_target_s_is_refused_at_the_export() {
    // std.tanh_shaper's drive ranges over [0.1, 20].
    let voice = voice(r#"{ drive = { to = "shaper.drive", min = 0, max = 2 } }"#);
    let errors = problems(&[
        ("project.toml", &project(r#"v = { module = "voice" }"#)),
        ("composites/voice.toml", &voice),
    ]);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with("composites/voice.toml:")
            && errors[0].contains(
                "export `drive`: its range [0, 2] is not inside the range [0.1, 20] of its target `shaper.drive`"
            ),
        "{errors:?}"
    );
}

#[test]
fn a_fan_out_range_must_fit_every_target() {
    // osc.freq: [0, 2^31 − ½]; shaper.drive: [0.1, 20]. [1, 10] fits both,
    // [1, 30] not the shaper's.
    let fits = voice(r#"{ both = { to = ["osc.freq", "shaper.drive"], min = 1, max = 10 } }"#);
    plan_ok(&[
        (
            "project.toml",
            &project(r#"v = { module = "voice", both = 5 }"#),
        ),
        ("composites/voice.toml", &fits),
    ]);
    let wide = voice(r#"{ both = { to = ["osc.freq", "shaper.drive"], min = 1, max = 30 } }"#);
    let errors = problems(&[
        ("project.toml", &project(r#"v = { module = "voice" }"#)),
        ("composites/voice.toml", &wide),
    ]);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("target `shaper.drive`"), "{errors:?}");
}

#[test]
fn a_value_outside_the_range_is_an_error_at_the_value() {
    let voice = voice(r#"{ attack = { to = "click.mul", min = 0, max = 2 } }"#);
    let text = project(r#"v = { module = "voice", attack = -1000000 }"#);
    let errors = problems(&[("project.toml", &text), ("composites/voice.toml", &voice)]);
    assert_eq!(errors.len(), 1, "{errors:?}");
    // The primitive rule: an error located at the instance's value.
    let (line, entry) = text
        .lines()
        .enumerate()
        .find(|(_, l)| l.starts_with("v = "))
        .unwrap();
    let column = entry.find("-1000000").unwrap() + 1;
    assert!(
        errors[0].starts_with(&format!("project.toml:{}:{column}: error: ", line + 1))
            && errors[0].contains("/v.attack: value -1000000 is outside [0, 2]"),
        "{errors:?}"
    );
    // Without the range, the same value is within std.scale's ±1e6.
    let plain = self::voice(r#"{ attack = "click.mul" }"#);
    plan_ok(&[("project.toml", &text), ("composites/voice.toml", &plain)]);
}

#[test]
fn an_inner_export_s_range_reaches_an_outer_export_and_its_values() {
    let voice = voice(r#"{ attack = { to = "click.mul", min = 0, max = 2 } }"#);
    let outer = |parameters: &str, value: &str| {
        format!(
            r#"schema_version = 1

[composite]
name = "rack"

[instances]
inner = {{ module = "voice"{value} }}

[exports]
outputs = {{ out = "inner.out" }}
parameters = {parameters}
"#
        )
    };
    // An outer export narrower than the inner one is fine; wider is not.
    let narrow = outer(r#"{ hit = { to = "inner.attack", min = 0, max = 1 } }"#, "");
    plan_ok(&[
        (
            "project.toml",
            &project(r#"v = { module = "rack", hit = 1 }"#),
        ),
        ("composites/voice.toml", &voice),
        ("composites/rack.toml", &narrow),
    ]);
    let wide = outer(r#"{ hit = { to = "inner.attack", min = 0, max = 3 } }"#, "");
    let errors = problems(&[
        ("project.toml", &project(r#"v = { module = "rack" }"#)),
        ("composites/voice.toml", &voice),
        ("composites/rack.toml", &wide),
    ]);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("composite `rack` export `hit`") && e.contains("[0, 2]")),
        "{errors:?}"
    );
    // An outer export without a range takes the inner one's for values.
    let plain = outer(r#"{ hit = "inner.attack" }"#, "");
    let errors = problems(&[
        (
            "project.toml",
            &project(r#"v = { module = "rack", hit = 5 }"#),
        ),
        ("composites/voice.toml", &voice),
        ("composites/rack.toml", &plain),
    ]);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("/v.hit: value 5 is outside [0, 2]")),
        "{errors:?}"
    );
    // A value inside a composite file is checked against the inner export.
    let inside = outer(r#"{ hit = "inner.attack" }"#, ", attack = 4");
    let errors = problems(&[
        ("project.toml", &project(r#"v = { module = "rack" }"#)),
        ("composites/voice.toml", &voice),
        ("composites/rack.toml", &inside),
    ]);
    assert!(
        errors.iter().any(|e| e.starts_with("composites/rack.toml:")
            && e.contains("/v/inner.attack: value 4 is outside [0, 2]")),
        "{errors:?}"
    );
}

#[test]
fn an_exported_input_takes_no_range() {
    let voice = r#"schema_version = 1

[composite]
name = "voice"

[instances]
amp = { module = "std.gain" }

[exports]
inputs = { in = { to = "amp.in", min = 0, max = 1 } }
outputs = { out = "amp.out" }
"#;
    let text = r#"schema_version = 1

[project]
name = "ranges"

[instances]
v = { module = "voice" }
out = { module = "std.audio_out" }

[wiring]
connections = [{ from = "v.out", to = "out.l" }]
"#;
    let errors = problems(&[("project.toml", text), ("composites/voice.toml", voice)]);
    assert!(
        errors
            .iter()
            .any(|e| e.starts_with("composites/voice.toml:")
                && e.contains("an exported input has none")),
        "{errors:?}"
    );
}

#[test]
fn the_studio_projects_load_with_their_ranges_and_every_value_inside() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for dir in [
        "projects/studio",
        "projects/studio-transport",
        "projects/two-studios",
        "projects/two-studios/studio-transport",
        "crates/signal-cli/tests/fixtures/studio-transport",
    ] {
        let project = Project::load(&root.join(dir)).unwrap_or_else(|e| panic!("{dir}: {e}"));
        project
            .plan(&registry())
            .unwrap_or_else(|e| panic!("{dir}: {e:?}"));
    }
    let studio = Project::load(&root.join("projects/studio")).unwrap();
    let kick = studio
        .composites
        .iter()
        .find(|c| c.name == "kick909")
        .unwrap();
    let attack = kick
        .exports
        .parameters
        .iter()
        .find(|e| e.name == "attack")
        .unwrap();
    assert_eq!(attack.range, Some(Range { min: 0.0, max: 2.0 }));
    // Every export of the studio's composites has a range (ADR 0042 §5),
    // key_track's [0, 2] since the owner's decision of 2026-10-25.
    for composite in &studio.composites {
        for export in &composite.exports.parameters {
            assert!(export.range.is_some(), "{}.{}", composite.name, export.name);
        }
    }
}
