//! M2.10-3 (ADR 0027 §4): every library composite is an ordinary composite
//! that the loader accepts under the library's rules, plans on its own
//! through the std registry (so a cycle among library composites, or a
//! reference to a missing one, fails here), declares a category of the
//! ADR 0027 vocabulary and a description naming each export, and renders at
//! its default levels on its stimulus within ±1 (AC9 as reworded: the
//! output stays within the engine's bound and below the demo's limiter
//! ceiling; audible safety is the operator's judgement).

mod common;

use common::{faults, peak, project, registry, render};
use signal_project::{Project, SourceFile, library_modules};

/// Renders `module` (with `parameters` set) with its inputs `inputs` held at
/// `values` and its outputs `outputs` to the sink's left and right: the
/// log's faults and the first frame's samples.
fn constants(
    module: &str,
    parameters: &str,
    inputs: [&str; 2],
    values: [f64; 2],
    outputs: [&str; 2],
) -> (Vec<String>, [f64; 2]) {
    // A constant is within ±10⁵ (ADR 0043): the value is a tenth of it,
    // scaled by 10 on its connection (exact for these values).
    let text = format!(
        "schema_version = 1\n\n[project]\nname = \"bound\"\nsample_rate = 48000\nseed = 1\n\n\
         [instances]\nx = {{ module = \"{module}\", {parameters} }}\n\
         u = {{ module = \"std.const\", value = {:?} }}\n\
         v = {{ module = \"std.const\", value = {:?} }}\n\
         out = {{ module = \"std.audio_out\", gain = 1 }}\n\n\
         [wiring]\nconnections = [\n\
         {{ from = \"u.out\", to = \"x.{}\", scale = 10 }},\n{{ from = \"v.out\", to = \"x.{}\", scale = 10 }},\n\
         {{ from = \"x.{}\", to = \"out.l\" }},\n{{ from = \"x.{}\", to = \"out.r\" }},\n]\n",
        values[0] / 10.0,
        values[1] / 10.0,
        inputs[0],
        inputs[1],
        outputs[0],
        outputs[1]
    );
    let project = Project::from_sources(vec![SourceFile {
        path: "project.toml".to_owned(),
        text,
    }])
    .unwrap();
    let rendered = render(&project);
    (
        faults(&rendered),
        [rendered.samples[0], rendered.samples[1]],
    )
}

/// auditor-1-003: at the engine's bound (±1e6) the crossfade weighs each
/// input before the sum, so no intermediate faults, and mix 1 is `b`, mix 0
/// is `a`, exactly.
#[test]
fn crossfade_is_exact_and_faultless_at_the_bound() {
    for (mix, expected) in [(1.0, 1e6), (0.0, -1e6), (0.5, 0.0)] {
        let (faults, out) = constants(
            "lib.crossfade",
            &format!("mix = {mix:?}"),
            ["a", "b"],
            [-1e6, 1e6],
            ["out", "out"],
        );
        assert_eq!(faults, Vec::<String>::new(), "mix {mix}");
        assert_eq!(out, [expected, expected], "mix {mix}");
    }
}

/// auditor-1-003: at the engine's bound (±1e6) the stereo width takes the
/// halves before the difference, so no intermediate faults, and width 1
/// passes the pair unchanged.
#[test]
fn stereo_width_is_exact_and_faultless_at_the_bound() {
    let cases = [
        ([1e6, 1e6], 1.0, [1e6, 1e6]),
        ([1e6, 1e6], 0.0, [1e6, 1e6]),
        ([1e6, 1e6], 2.0, [1e6, 1e6]),
        ([1e6, -1e6], 1.0, [1e6, -1e6]),
        ([1e6, -1e6], 0.0, [0.0, 0.0]),
        ([-1e6, 1e6], 1.0, [-1e6, 1e6]),
    ];
    for (input, width, expected) in cases {
        let (faults, out) = constants(
            "lib.stereo_width",
            &format!("width = {width:?}"),
            ["l", "r"],
            input,
            ["out_l", "out_r"],
        );
        assert_eq!(faults, Vec::<String>::new(), "{input:?} at width {width}");
        assert_eq!(out, expected, "{input:?} at width {width}");
    }
}

#[test]
fn every_library_composite_loads_and_plans_alone() {
    let registry = registry();
    for file in signal_lib::composites() {
        let project = project(file.name, "");
        let reference = format!("lib.{}", file.name);
        let used = project.library.iter().find(|l| l.name == reference);
        assert_eq!(used.map(|l| l.hash.clone()), Some(file.hash()));
        project
            .plan(&registry)
            .unwrap_or_else(|d| panic!("{reference}: {d:?}"));
    }
}

#[test]
fn every_library_composite_declares_its_category_and_documents_its_exports() {
    let modules = library_modules(&registry()).unwrap();
    assert_eq!(modules.len(), signal_lib::composites().len());
    for module in &modules {
        let name = module["module"].as_str().unwrap();
        let category = module["category"].as_str().unwrap_or_default();
        assert!(
            signal_std::category::ALL.contains(&category),
            "{name}: category `{category}`"
        );
        let description = module["description"].as_str().unwrap_or_default();
        assert!(description.len() > 40, "{name}: no description");
        let exports = module["ports"]
            .as_array()
            .unwrap()
            .iter()
            .chain(module["parameters"].as_array().unwrap());
        for export in exports {
            let export = export["name"].as_str().unwrap();
            assert!(
                description.contains(export),
                "{name}: the description does not name the export `{export}`"
            );
        }
        // M2.10-4: a validation reference names its native type
        // (`reference_for`) and says what it is; no other composite does.
        let bare = name.strip_prefix("lib.").unwrap();
        assert_eq!(
            module.get("reference_for").and_then(|v| v.as_str()),
            signal_lib::reference_for(bare),
            "{name}"
        );
        if let Some(native) = signal_lib::reference_for(bare) {
            assert!(
                description.starts_with(native) && description.contains("validation reference"),
                "{name}: the description names neither {native} nor its role"
            );
        }
    }
}

/// AC9 (reworded): at its defaults, on its stimulus, no composite faults and
/// none exceeds ±1.
#[test]
fn default_levels_stay_within_one() {
    for file in signal_lib::composites() {
        let rendered = render(&project(file.name, ""));
        assert_eq!(faults(&rendered), Vec::<String>::new(), "lib.{}", file.name);
        let peak = peak(&rendered);
        println!("lib.{}: default peak {peak:.4}", file.name);
        assert!(peak <= 1.0, "lib.{}: default peak {peak}", file.name);
    }
}
