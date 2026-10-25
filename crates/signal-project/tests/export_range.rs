//! ADR 0042: an exported parameter with its own range. Both forms of an
//! export parse; a range outside a target's (one target, or any of a
//! fan-out) is refused at the export; an export without one takes an inner
//! export's; a value in a file outside an export's range is clamped, with
//! a warning at the value, as for a primitive's (the operator's decision
//! of 2026-10-25); an exported input takes no range; and the
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

/// ADR 0042, 0043 (the operator's decision of 2026-10-25): the warnings of
/// `files`, which must load and plan, and the plan's value of `port`.
fn warned(files: &[(&str, &str)]) -> Vec<String> {
    let project = Project::from_sources(sources(files)).unwrap();
    let planned = project
        .plan(&registry())
        .unwrap_or_else(|e| panic!("it must load: {e:?}"));
    planned.warnings.iter().map(ToString::to_string).collect()
}

/// The plan's value of `port` (`/instance.parameter`) for `files`.
fn played(files: &[(&str, &str)], port: &str) -> f64 {
    let project = Project::from_sources(sources(files)).unwrap();
    let plan = project.plan(&registry()).unwrap().plan;
    let (instance, parameter) = port.rsplit_once('.').unwrap();
    plan.instances
        .iter()
        .find(|i| i.address.as_str() == instance)
        .and_then(|i| i.parameters.get(parameter).copied())
        .unwrap_or_else(|| panic!("no {port}"))
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
fn a_value_outside_the_range_is_clamped_with_a_warning() {
    let voice = voice(r#"{ attack = { to = "click.mul", min = 0, max = 2 } }"#);
    let text = project(r#"v = { module = "voice", attack = -50000 }"#);
    let files = [
        ("project.toml", text.as_str()),
        ("composites/voice.toml", voice.as_str()),
    ];
    let errors = warned(&files);
    assert_eq!(errors.len(), 1, "{errors:?}");
    // A warning at the instance's value; the clamped value plays.
    let (line, entry) = text
        .lines()
        .enumerate()
        .find(|(_, l)| l.starts_with("v = "))
        .unwrap();
    let column = entry.find("-50000").unwrap() + 1;
    assert!(
        errors[0].starts_with(&format!("project.toml:{}:{column}: warning: ", line + 1))
            && errors[0].contains("/v.attack: value -50000 is outside [0, 2]: 0 plays"),
        "{errors:?}"
    );
    assert_eq!(played(&files, "/v/click.mul"), 0.0);
    // Without the range, the same value is within std.scale's ±10⁵ (ADR 0043).
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
    let errors = warned(&[
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
            .any(|e| e.contains("/v.hit: value 5 is outside [0, 2]: 2 plays")),
        "{errors:?}"
    );
    // A value inside a composite file is checked against the inner export.
    let inside = outer(r#"{ hit = "inner.attack" }"#, ", attack = 4");
    let errors = warned(&[
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

/// The exported parameters of a composite file's text that declare neither
/// a range (`min`, `max`) nor a value set (`values`).
fn undeclared(text: &str) -> Vec<String> {
    let doc: toml::Table = toml::from_str(text).unwrap();
    let Some(parameters) = doc
        .get("exports")
        .and_then(|e| e.get("parameters"))
        .and_then(toml::Value::as_table)
    else {
        return Vec::new();
    };
    parameters
        .iter()
        .filter(|(_, export)| {
            let table = export.as_table();
            let ranged = table.is_some_and(|t| t.contains_key("min") && t.contains_key("max"));
            let valued = table.is_some_and(|t| t.contains_key("values"));
            !(ranged || valued)
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// ADR 0043, "MINDEN kapjon értelmezési tartományt vagy értékkészletet":
/// every exported parameter of the repository's project composites (and
/// the frozen studio-transport fixture) and of the built-in library
/// declares a range or a value set itself.
#[test]
fn every_repository_and_library_export_declares_a_range_or_a_value_set() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files: Vec<(String, String)> = Vec::new();
    fn walk(dir: &Path, files: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, files);
            } else if path.extension().is_some_and(|e| e == "toml")
                && path
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|d| d == "composites")
            {
                let text = std::fs::read_to_string(&path).unwrap();
                files.push((path.display().to_string(), text));
            }
        }
    }
    walk(&root.join("projects"), &mut files);
    walk(
        &root.join("crates/signal-cli/tests/fixtures/studio-transport"),
        &mut files,
    );
    let project_files = files.len();
    for library in signal_lib::composites() {
        files.push((format!("lib.{}", library.name), library.text.to_owned()));
    }
    assert!(
        project_files >= 30,
        "{project_files} project composite files"
    );
    assert!(files.len() > project_files + 20, "{} files", files.len());
    let missing: Vec<String> = files
        .iter()
        .flat_map(|(file, text)| {
            undeclared(text)
                .into_iter()
                .map(move |name| format!("{file}: {name}"))
        })
        .collect();
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

/// A voice of a `std.osc` (whose `shape` has a value set: 0 sine, 1 saw,
/// 2 square, 3 triangle), with `exports` as its parameters line.
fn osc_voice(parameters: &str) -> String {
    format!(
        r#"schema_version = 1

[composite]
name = "voice"

[instances]
osc = {{ module = "std.osc", freq = 110 }}

[exports]
outputs = {{ out = "osc.out" }}
parameters = {parameters}
"#
    )
}

/// ADR 0043: an export's own value set parses, its range is the values'
/// span, and a value of it the target does not allow is refused.
#[test]
fn an_export_value_set_must_be_allowed_by_its_target() {
    let voice = osc_voice(
        r#"{ wave = { to = "osc.shape", values = [{ value = 1, label = "saw" }, { value = 0, label = "sine" }] } }"#,
    );
    let planned = plan_ok(&[
        (
            "project.toml",
            &project(r#"v = { module = "voice", wave = 1 }"#),
        ),
        ("composites/voice.toml", &voice),
    ]);
    let wave = &planned.composites[0].exports.parameters[0];
    assert_eq!(wave.range, Some(Range { min: 0.0, max: 1.0 }));
    let labels: Vec<(f64, &str)> = wave
        .values
        .as_ref()
        .unwrap()
        .iter()
        .map(|v| (v.value, v.label.as_str()))
        .collect();
    assert_eq!(labels, [(0.0, "sine"), (1.0, "saw")], "sorted by value");

    let half =
        osc_voice(r#"{ wave = { to = "osc.shape", values = [{ value = 0.5, label = "half" }] } }"#);
    let errors = problems(&[
        ("project.toml", &project(r#"v = { module = "voice" }"#)),
        ("composites/voice.toml", &half),
    ]);
    assert!(
        errors.iter().any(|e| e.contains("export `wave`: its value 0.5 is not one of its target `osc.shape`'s (0 sine, 1 saw, 2 square, 3 triangle)")),
        "{errors:?}"
    );
}

/// ADR 0043 (the operator's decision of 2026-10-25): a value in a file that
/// is not one of the value set (the export's own, or the one it takes from
/// its target) is snapped to them, with a warning at the value, as an
/// out-of-range value is clamped; values and a range together are refused.
#[test]
fn a_value_not_in_the_set_takes_the_nearest_with_a_warning() {
    let own = osc_voice(
        r#"{ wave = { to = "osc.shape", values = [{ value = 0, label = "sine" }, { value = 1, label = "saw" }] } }"#,
    );
    let errors = warned(&[
        (
            "project.toml",
            &project(r#"v = { module = "voice", wave = 2 }"#),
        ),
        ("composites/voice.toml", &own),
    ]);
    assert!(
        errors.iter().any(|e| e.starts_with("project.toml:")
            && e.contains("/v.wave: value 2 is outside [0, 1]: 1 plays")),
        "{errors:?}"
    );
    // Inherited from the target: 1.5 is in the range, not in the set.
    let inherited = osc_voice(r#"{ wave = "osc.shape" }"#);
    let errors = warned(&[
        (
            "project.toml",
            &project(r#"v = { module = "voice", wave = 1.5 }"#),
        ),
        ("composites/voice.toml", &inherited),
    ]);
    assert!(
        errors.iter().any(|e| e.contains(
            "/v.wave: value 1.5 is not one of 0 sine, 1 saw, 2 square, 3 triangle: 2 plays"
        )),
        "{errors:?}"
    );
    // A primitive's own, likewise; the nearest plays, a tie upward (as
    // `process` reads it, review of c89253a).
    let text = project(r#"v = { module = "std.osc", shape = 1.5 }"#);
    let errors = warned(&[("project.toml", &text)]);
    assert_eq!(played(&[("project.toml", &text)], "/v.shape"), 2.0);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("/v.shape: value 1.5 is not one of")),
        "{errors:?}"
    );
    let both = osc_voice(
        r#"{ wave = { to = "osc.shape", min = 0, max = 1, values = [{ value = 0, label = "sine" }] } }"#,
    );
    let errors = problems(&[
        ("project.toml", &project(r#"v = { module = "voice" }"#)),
        ("composites/voice.toml", &both),
    ]);
    assert!(errors.iter().any(|e| e.contains("not both")), "{errors:?}");
}

/// The owner's real project of 2026-10-25, sanitised: a voice composite
/// written before ADR 0043, whose `wave` export has no value set of its own
/// (it takes `std.osc`'s), set to 1.0023 by a slider; and a frequency
/// beyond the new 100 kHz end. An upgrade must never make such a project
/// unopenable (the operator's decision): it loads, the nearest value and
/// the clamped one play, and two warnings say so at the values.
#[test]
fn a_project_from_before_the_ranges_still_loads_and_plays_the_corrected_values() {
    let voice = r#"schema_version = 1

[composite]
name = "voice"

[instances]
osc = { module = "std.osc", freq = 110 }

[exports]
outputs = { out = "osc.out" }
parameters = { wave = "osc.shape", freq = "osc.freq" }
"#;
    let text = project(r#"v = { module = "voice", wave = 1.0023, freq = 250000 }"#);
    let files = [
        ("project.toml", text.as_str()),
        ("composites/voice.toml", voice),
    ];
    let warnings = warned(&files);
    assert!(
        warnings.iter().any(|w| w.starts_with("project.toml:8:")
            && w.contains("warning: /v.wave: value 1.0023 is not one of 0 sine, 1 saw, 2 square, 3 triangle: 1 plays")),
        "{warnings:?}"
    );
    assert!(
        warnings.iter().any(|w| w.starts_with("project.toml:8:")
            && w.contains("warning: /v.freq: value 250000 is outside [0, 100000]: 100000 plays")),
        "{warnings:?}"
    );
    assert_eq!(played(&files, "/v/osc.shape"), 1.0);
    assert_eq!(played(&files, "/v/osc.freq"), 100_000.0);
    // The file keeps its text: nothing was written.
    assert!(text.contains("wave = 1.0023, freq = 250000"));
}

/// Review of c89253a, 5: a value in a composite's file is never written by
/// Signal (Save writes `project.toml`'s controls), so its warning says to
/// edit that file or vendor the composite again; `project.toml`'s says the
/// next write takes it.
#[test]
fn a_value_corrected_in_a_composite_file_says_to_edit_it() {
    let inner = r#"schema_version = 1

[composite]
name = "voice"

[instances]
osc = { module = "std.osc", freq = 250000 }

[exports]
outputs = { out = "osc.out" }
parameters = { wave = "osc.shape" }
"#;
    let files = [
        (
            "project.toml",
            project(r#"v = { module = "voice", wave = 0.4 }"#),
        ),
        ("composites/voice.toml", inner.to_owned()),
    ];
    let files: Vec<(&str, &str)> = files.iter().map(|(p, t)| (*p, t.as_str())).collect();
    let warnings = warned(&files);
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("composites/voice.toml:")
                && w.ends_with(
                    "/v/osc.freq: value 250000 is outside [0, 100000]: 100000 plays \
                 (composites/voice.toml keeps 250000: edit it, or vendor the composite again)"
                )),
        "{warnings:?}"
    );
    assert!(
        warnings.iter().any(|w| w.starts_with("project.toml:")
            && w.ends_with("/v.wave: value 0.4 is not one of 0 sine, 1 saw, 2 square, 3 triangle: 0 plays (the file keeps 0.4 until the parameter is written)")),
        "{warnings:?}"
    );
}

/// Review of c89253a, 7: a fanned-out export without a range of its own
/// takes the intersection of its targets' (as the panel's control and
/// `describe` do), so a value inside one target's range but not the other's
/// is clamped once, at the export, and every target gets the same value.
#[test]
fn a_fan_out_value_is_clamped_to_the_intersection_at_the_export() {
    // osc.freq: [0, 10⁵]; shaper.drive: [0.1, 20].
    let fan = voice(r#"{ both = ["osc.freq", "shaper.drive"] }"#);
    let text = project(r#"v = { module = "voice", both = 50 }"#);
    let files = [
        ("project.toml", text.as_str()),
        ("composites/voice.toml", &fan),
    ];
    let warnings = warned(&files);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("/v.both: value 50 is outside [0.1, 20]: 20 plays")),
        "{warnings:?}"
    );
    assert_eq!(played(&files, "/v/osc.freq"), 20.0);
    assert_eq!(played(&files, "/v/shaper.drive"), 20.0);
}

/// Review of c89253a, 7: an export with its own range and the value set of
/// its target keeps the values inside its range (a value beyond is clamped
/// into it, one between them snapped to those kept), and a range that
/// keeps none of them is refused at the export.
#[test]
fn an_own_range_keeps_the_inherited_values_inside_it() {
    let narrow = osc_voice(r#"{ wave = { to = "osc.shape", min = 0, max = 1 } }"#);
    let text = project(r#"v = { module = "voice", wave = 3 }"#);
    let files = [
        ("project.toml", text.as_str()),
        ("composites/voice.toml", &narrow),
    ];
    let warnings = warned(&files);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("/v.wave: value 3 is outside [0, 1]: 1 plays")),
        "{warnings:?}"
    );
    assert_eq!(played(&files, "/v/osc.shape"), 1.0);
    let none = osc_voice(r#"{ wave = { to = "osc.shape", min = 0.2, max = 0.8 } }"#);
    let errors = problems(&[
        ("project.toml", &project(r#"v = { module = "voice" }"#)),
        ("composites/voice.toml", &none),
    ]);
    assert!(
        errors.iter().any(|e| e.starts_with("composites/voice.toml:")
            && e.contains("export `wave`: its range [0.2, 0.8] holds none of its targets' values (0 sine, 1 saw, 2 square, 3 triangle)")),
        "{errors:?}"
    );
}

/// Review of c89253a, 2: a parameter whose connections are all constants
/// that, with their scales, take it beyond its range is warned of at the
/// connection (the range clamps a modulated value); a sum inside the range,
/// or a source that is not a constant, is not.
#[test]
fn a_constant_beyond_a_parameter_s_range_is_warned_of() {
    let wired = |scale: &str, source: &str| {
        format!(
            r#"schema_version = 1

[project]
name = "constants"
sample_rate = 48000

[instances]
c = {{ {source} }}
v = {{ module = "std.osc", freq = 110 }}
out = {{ module = "std.audio_out" }}

[wiring]
connections = [
  {{ from = "c.out", to = "v.freq", scale = {scale} }},
  {{ from = "v.out", to = "out.l" }},
]
"#
        )
    };
    let beyond = wired("200000", r#"module = "std.const", value = 1"#);
    let warnings = warned(&[("project.toml", &beyond)]);
    assert!(
        warnings.iter().any(|w| w.starts_with("project.toml:")
            && w.contains("warning: /v.freq: the constant /c.out makes it 200110 (connection scales and base together), outside its range [0, 100000]: 100000 plays")),
        "{warnings:?}"
    );
    let inside = wired("1000", r#"module = "std.const", value = 1"#);
    let warnings = warned(&[("project.toml", &inside)]);
    assert!(
        !warnings.iter().any(|w| w.contains("the constant")),
        "{warnings:?}"
    );
    let moving = wired("200000", r#"module = "std.sine_osc""#);
    let warnings = warned(&[("project.toml", &moving)]);
    assert!(
        !warnings.iter().any(|w| w.contains("the constant")),
        "{warnings:?}"
    );
}
