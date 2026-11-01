//! ADR 0038 §3 (M4.1): device slots in the project text. A device slot names
//! a layout and a role, never hardware; `stereo` plans as `std.audio_out`
//! (its ports `l`, `r`, `gain`), so the plan is the one a `std.audio_out`
//! of the same name gives, and the role is handed beside the plan
//! (`device::play_sides`). Two outputs of one role are a plan error; the
//! layouts not built yet, capture, a role that is no name, a device slot
//! in a composite and a device slot with module keys are refused with
//! their reason.

use signal_core::Registry;
use signal_project::device::{main_side, play_sides};
use signal_project::session::sink_of;
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

fn load(text: &str) -> Result<Project, String> {
    Project::from_sources(sources(&[("project.toml", text)])).map_err(|d| {
        d.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// The diagnostics of loading and planning `text`, joined.
fn problems(text: &str) -> String {
    match load(text) {
        Err(problems) => problems,
        Ok(project) => match project.plan(&registry()) {
            Ok(_) => String::new(),
            Err(d) => d
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        },
    }
}

const HEAD: &str = "schema_version = 1\n[project]\nname = \"cue\"\nsample_rate = 48000\nseed = 1\n";

fn main_and_cue(cue: &str) -> String {
    format!(
        "{HEAD}[instances]\nosc = {{ module = \"std.sine_osc\", freq = 220.0 }}\nout = {{ module = \"std.audio_out\" }}\ncue = {cue}\n\
         [wiring]\nconnections = [\n  {{ from = \"osc.out\", to = \"out.l\" }},\n  {{ from = \"osc.out\", to = \"out.r\" }},\n  {{ from = \"osc.out\", to = \"cue.l\" }},\n  {{ from = \"osc.out\", to = \"cue.r\", scale = 0.5 }},\n]\n"
    )
}

#[test]
fn a_cue_device_slot_is_a_stereo_play_side_of_its_role() {
    let registry = registry();
    let project = load(&main_and_cue(
        "{ device = { play = \"stereo\", role = \"cue\" }, gain = 0.5 }",
    ))
    .unwrap();
    let slot = project.device_slots().get("cue").unwrap();
    assert_eq!((slot.role.as_str(), slot.play.as_str()), ("cue", "stereo"));
    let plan = project.plan(&registry).unwrap().plan;
    let sides = play_sides(&project, &plan);
    let described: Vec<(String, String, usize)> = sides
        .iter()
        .map(|s| (s.address.to_string(), s.role.clone(), s.width))
        .collect();
    assert_eq!(
        described,
        [
            ("/cue".to_owned(), "cue".to_owned(), 2),
            ("/out".to_owned(), "main".to_owned(), 2)
        ]
    );
    assert_eq!(main_side(&sides).unwrap().address.as_str(), "/out");
    assert_eq!(sink_of(&project, &plan).unwrap().0.as_str(), "/out");

    // The role is not in the plan (ADR 0038 §7, test 1): another role gives
    // a byte-identical plan.
    let other = load(&main_and_cue(
        "{ device = { play = \"stereo\", role = \"monitor\" }, gain = 0.5 }",
    ))
    .unwrap();
    let other = other.plan(&registry).unwrap().plan;
    assert_eq!(format!("{plan:?}"), format!("{other:?}"));
    assert_eq!(
        plan.instances
            .iter()
            .find(|i| i.address.as_str() == "/cue")
            .map(|i| i.type_name),
        Some("std.audio_out")
    );
}

#[test]
fn the_role_defaults_to_main_so_two_mains_are_refused() {
    let found = problems(&main_and_cue("{ device = { play = \"stereo\" } }"));
    assert!(
        found.contains("/out: another output already plays role `main`"),
        "{found}"
    );
    // Two `std.audio_out` are two mains as well.
    let found = problems(&main_and_cue("{ module = \"std.audio_out\" }"));
    assert!(
        found.contains("another output already plays role `main`"),
        "{found}"
    );
}

#[test]
fn a_project_without_main_plays_its_first_role() {
    let text = format!(
        "{HEAD}[instances]\nb = {{ device = {{ play = \"stereo\", role = \"stems\" }} }}\na = {{ device = {{ play = \"stereo\", role = \"monitor\" }} }}\n"
    );
    let project = load(&text).unwrap();
    let plan = project.plan(&registry()).unwrap().plan;
    assert_eq!(sink_of(&project, &plan).unwrap().0.as_str(), "/a");
}

#[test]
fn what_is_not_built_or_not_valid_is_refused_with_its_reason() {
    let cases = [
        (
            "{ device = { play = \"5.1\", role = \"surround\" } }",
            "cue: the layout `5.1` is not built yet",
        ),
        (
            "{ device = { play = \"hexa\" } }",
            "cue: `hexa` is not a layout",
        ),
        (
            "{ device = { capture = \"stereo\" } }",
            "cue: a device slot's `capture` (audio input) is not built yet",
        ),
        (
            "{ device = { play = \"stereo\", role = \"Cue\" } }",
            "cue: invalid role `Cue`",
        ),
        (
            "{ device = { play = \"stereo\", kind = \"midi\" } }",
            "cue: device kind `midi` is not built",
        ),
        (
            "{ device = { role = \"cue\" } }",
            "cue: a device slot needs `play`",
        ),
        (
            "{ device = { play = \"stereo\", role = \"cue\", channels = [3, 4] } }",
            "unknown field `channels`",
        ),
        (
            "{ device = { play = \"stereo\" }, module = \"std.audio_out\" }",
            "exactly one of `module`, `project`, `slot` and `device`",
        ),
        (
            "{ device = { play = \"stereo\", role = \"cue\" }, seed = 3 }",
            "`seed` applies to an included project or a slot",
        ),
    ];
    for (cue, expected) in cases {
        let found = problems(&main_and_cue(cue));
        assert!(found.contains(expected), "{cue}: {found}");
    }
}

#[test]
fn a_composite_names_no_device_slot() {
    let found = match Project::from_sources(sources(&[
        (
            "project.toml",
            &format!(
                "{HEAD}[instances]\nvoice = {{ module = \"voice\" }}\nout = {{ module = \"std.audio_out\" }}\n"
            ),
        ),
        (
            "composites/voice.toml",
            "schema_version = 1\n[composite]\nname = \"voice\"\n[instances]\ncue = { device = { play = \"stereo\", role = \"cue\" } }\n",
        ),
    ])) {
        Ok(_) => String::new(),
        Err(d) => d.iter().map(ToString::to_string).collect(),
    };
    assert!(
        found.contains("a device slot is a top-level instance of project.toml"),
        "{found}"
    );
}
