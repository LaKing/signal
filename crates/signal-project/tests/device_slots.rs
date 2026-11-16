//! ADR 0038 §3 (M4.1): device slots in the project text. A device slot names
//! a layout and a role, never hardware; `stereo` plans as `std.audio_out`
//! (its ports `l`, `r`, `gain`), so the plan is the one a `std.audio_out`
//! of the same name gives, and the role is handed beside the plan
//! (`device::play_sides`). A `capture` side plans as a source,
//! `std.audio_in` or `std.audio_in_mono`, its role handed beside the plan
//! (`device::capture_sides`, ADR 0038 §7). Two outputs of one role are a
//! plan error; every layout of ADR 0038 §2 plays (`mono`, `quad`, `5.1`,
//! `7.1` as their own sinks, their channels in order, a tap inside an
//! include); a discrete capture side is `std.device_in<N>`, its channels
//! exported by name; the capture layouts not built yet, a slot that both plays and
//! captures, a role that is no name, a device slot in a composite and a
//! device slot with module keys are refused with their reason.

use signal_core::Registry;
use signal_project::device::{capture_sides, main_side, play_sides};
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
    assert_eq!(
        (slot.role.as_str(), slot.play.as_deref()),
        ("cue", Some("stereo"))
    );
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

/// ADR 0038 §5.4: a role's versions differ in layout; a session records
/// the largest, whatever the machine plays.
#[test]
fn a_role_may_have_versions_of_other_layouts() {
    let text = format!(
        "{HEAD}[instances]\nfold = {{ device = {{ play = \"stereo\" }} }}\nsurround = {{ device = {{ play = \"5.1\" }} }}\n"
    );
    let project = load(&text).unwrap();
    let plan = project.plan(&registry()).unwrap().plan;
    let sides = play_sides(&project, &plan);
    assert_eq!(sides.len(), 2);
    assert_eq!(main_side(&sides).unwrap().address.as_str(), "/surround");
    assert_eq!(
        sink_of(&project, &plan).unwrap(),
        ("/surround".parse().unwrap(), 6)
    );
    let twice = format!("{text}again = {{ device = {{ play = \"5.1\", role = \"main\" }} }}\n");
    let found = problems(&twice);
    assert!(
        found.contains("/surround: another output already plays role `main` in `5.1`"),
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
            "{ device = { play = \"hexa\" } }",
            "cue: `hexa` is not a layout",
        ),
        (
            "{ device = { play = \"stereo\", capture = \"stereo\" } }",
            "cue: a device slot that both plays and captures (one box, ADR 0038 §3) is not built yet",
        ),
        (
            "{ device = { capture = \"5.1\" } }",
            "cue: the capture layout `5.1` is not built yet",
        ),
        (
            "{ device = { play = \"stereo\", role = \"Cue\" } }",
            "cue: invalid role `Cue`",
        ),
        (
            "{ device = { play = \"stereo\", kind = \"laser\" } }",
            "cue: device kind `laser` is not built",
        ),
        (
            "{ device = { play = \"stereo\", kind = \"midi\" } }",
            "cue: a `midi` slot captures its values by name",
        ),
        (
            "{ device = { role = \"cue\" } }",
            "cue: a device slot needs `play` or `capture`",
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

/// ADR 0038 §7: a capture side is a source of its role, `std.audio_in`
/// (stereo) or `std.audio_in_mono`, wired from its outputs.
#[test]
fn a_capture_slot_is_a_source_of_its_role() {
    let text = "schema_version = 1\n[project]\nname = \"listen\"\nsample_rate = 48000\n\
         [instances]\nmic = { device = { capture = \"mono\", role = \"voice\" } }\n\
         room = { device = { capture = \"stereo\" } }\nout = { module = \"std.audio_out\" }\n\
         [wiring]\nconnections = [\n  { from = \"mic.m\", to = \"out.l\" },\n  { from = \"room.r\", to = \"out.r\" },\n]\n";
    let project = load(text).unwrap();
    let slot = project.device_slots().get("mic").unwrap();
    assert_eq!(slot.capture.as_deref(), Some("mono"));
    assert_eq!(slot.play, None);
    let plan = project.plan(&registry()).unwrap().plan;
    let types: Vec<(String, &str)> = plan
        .instances
        .iter()
        .map(|i| (i.address.to_string(), i.type_name))
        .collect();
    assert!(
        types.contains(&("/mic".to_owned(), "std.audio_in_mono")),
        "{types:?}"
    );
    assert!(
        types.contains(&("/room".to_owned(), "std.audio_in")),
        "{types:?}"
    );
    let sides: Vec<(String, String, usize)> = capture_sides(&project, &plan)
        .iter()
        .map(|s| (s.address.to_string(), s.role.clone(), s.width))
        .collect();
    assert_eq!(
        sides,
        [
            ("/mic".to_owned(), "voice".to_owned(), 1),
            ("/room".to_owned(), "main".to_owned(), 2),
        ]
    );
}

/// ADR 0038 §2, §7: a discrete capture side, `capture = ["vox", "gtr"]`,
/// plans as `std.device_in2` at `<slot>/capture`, its channels exported by
/// name; it is a capture side of the slot's role, two wide; inside another
/// project it is a pass-through whose inputs the host wires (`x.mics_gtr`).
#[test]
fn a_discrete_capture_side_exports_its_channels_by_name() {
    let text = format!(
        "{HEAD}[instances]
mics = {{ device = {{ capture = [\"vox\", \"gtr\"], role = \"band\" }} }}
         out = {{ module = \"std.audio_out\" }}
         [wiring]
connections = [\n  {{ from = \"mics.vox\", to = \"out.l\" }},\n  {{ from = \"mics.gtr\", to = \"out.r\" }},\n]\n"
    );
    let project = load(&text).unwrap();
    let slot = project.device_slots().get("mics").unwrap();
    assert_eq!(slot.capture.as_deref(), Some("discrete"));
    assert_eq!(slot.ports, ["vox", "gtr"]);
    let plan = project.plan(&registry()).unwrap().plan;
    let source = plan
        .instances
        .iter()
        .find(|i| i.address.as_str() == "/mics/capture")
        .unwrap();
    assert_eq!(source.type_name, "std.device_in2");
    let from = |to: &str| {
        plan.sources
            .iter()
            .find(|(port, _)| port.to_string() == to)
            .map(|(_, s)| s[0].from.to_string())
    };
    assert_eq!(from("/out.l").as_deref(), Some("/mics/capture.c1"));
    assert_eq!(from("/out.r").as_deref(), Some("/mics/capture.c2"));
    let sides: Vec<(String, String, usize)> = capture_sides(&project, &plan)
        .iter()
        .map(|s| (s.address.to_string(), s.role.clone(), s.width))
        .collect();
    assert_eq!(sides, [("/mics/capture".to_owned(), "band".to_owned(), 2)]);
    assert!(
        problems(&format!(
            "{HEAD}[instances]\nmics = {{ device = {{ capture = [\"vox\", \"vox\"] }} }}\n"
        ))
        .contains("port `vox` is listed twice")
    );

    let dir =
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("discrete-capture-include");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("child")).unwrap();
    std::fs::write(dir.join("child/project.toml"), &text).unwrap();
    std::fs::write(
        dir.join("project.toml"),
        format!(
            "{HEAD}[instances]\nx = {{ project = \"./child\" }}\nosc = {{ module = \"std.sine_osc\" }}\n\
             [wiring]\nconnections = [ {{ from = \"osc.out\", to = \"x.mics_gtr\" }} ]\n"
        ),
    )
    .unwrap();
    let project = Project::load(&dir).unwrap();
    let plan = project.plan(&registry()).unwrap().plan;
    let pass = plan
        .instances
        .iter()
        .find(|i| i.address.as_str() == "/x/mics/capture")
        .unwrap();
    assert_eq!(pass.type_name, "std.device_in_pass2");
    let from = |to: &str| {
        plan.sources
            .iter()
            .find(|(port, _)| port.to_string() == to)
            .map(|(_, s)| s[0].from.to_string())
    };
    assert_eq!(from("/x/mics/capture.c2_in").as_deref(), Some("/osc.out"));
    assert!(
        capture_sides(&project, &plan).is_empty(),
        "only the host captures"
    );
}

/// ADR 0038 §2: every layout plays, as its own sink with the layout's
/// channels in order: the side's width is the layout's, and inside an
/// included project the sink is its tap.
#[test]
fn every_layout_plays_as_its_own_sink() {
    for (layout, module, width) in [
        ("mono", "std.audio_out_mono", 1),
        ("quad", "std.audio_out_quad", 4),
        ("5.1", "std.audio_out_5_1", 6),
        ("7.1", "std.audio_out_7_1", 8),
    ] {
        let text = format!(
            "{HEAD}[instances]\nosc = {{ module = \"std.sine_osc\", freq = 220.0 }}\nroom = {{ device = {{ play = \"{layout}\" }} }}\n\
             [wiring]\nconnections = [ {{ from = \"osc.out\", to = \"room.{}\" }} ]\n",
            if layout == "mono" { "m" } else { "l" }
        );
        let project = load(&text).unwrap();
        let plan = project.plan(&registry()).unwrap().plan;
        let room = plan
            .instances
            .iter()
            .find(|i| i.address.as_str() == "/room")
            .unwrap();
        assert_eq!(room.descriptor.type_name, module, "{layout}");
        let sides = play_sides(&project, &plan);
        assert_eq!(sides.len(), 1);
        assert_eq!(
            (sides[0].role.as_str(), sides[0].width),
            ("main", width),
            "{layout}"
        );
        assert_eq!(
            signal_project::device::layout_of_width(width),
            layout,
            "{layout}: shown by its width"
        );
    }
    let mut registry = registry();
    assert_eq!(
        registry.sink_tap("std.audio_out_5_1"),
        Some("std.audio_tap_5_1"),
        "inside an include, its tap"
    );
    let _ = &mut registry;
}

/// ADR 0038 §2, §3: a discrete layout names its ports in channel order; it
/// plans as a composite of them on `std.device_out<N>` (`c1…cN`), the slot's
/// sink one level down, its role and port names handed beside the plan.
#[test]
fn a_discrete_layout_plays_its_named_ports_in_order() {
    let text = format!(
        "{HEAD}[instances]\nosc = {{ module = \"std.sine_osc\", freq = 220.0 }}\n\
         out = {{ module = \"std.audio_out\" }}\n\
         stems = {{ device = {{ play = [\"kick\", \"snare\", \"pad_l\", \"pad_r\"], role = \"stems\" }}, gain = 0.5 }}\n\
         [wiring]\nconnections = [ {{ from = \"osc.out\", to = \"stems.snare\" }}, {{ from = \"osc.out\", to = \"out.l\" }} ]\n"
    );
    let project = load(&text).unwrap();
    let slot = &project.device_slots()["stems"];
    assert_eq!(slot.play.as_deref(), Some("discrete"));
    assert_eq!(slot.ports, ["kick", "snare", "pad_l", "pad_r"]);
    let plan = project.plan(&registry()).unwrap().plan;
    let sink = plan
        .instances
        .iter()
        .find(|i| i.address.as_str() == "/stems/play")
        .unwrap();
    assert_eq!(sink.descriptor.type_name, "std.device_out4");
    assert_eq!(sink.parameters["gain"], 0.5);
    let sides = play_sides(&project, &plan);
    let stems = sides.iter().find(|s| s.role == "stems").unwrap();
    assert_eq!(stems.address.as_str(), "/stems/play");
    assert_eq!(
        (stems.width, &stems.ports[..]),
        (
            4,
            &["kick", "snare", "pad_l", "pad_r"].map(String::from)[..]
        )
    );
    assert_eq!(main_side(&sides).unwrap().address.as_str(), "/out");
    assert_eq!(
        signal_project::device::slot_of_sink(&stems.address),
        Some("stems")
    );

    // Two discrete channels are not stereo: a stereo `main` and a discrete
    // `main` of two are versions of one role, not a repeated layout.
    let two = format!(
        "{HEAD}[instances]\nout = {{ module = \"std.audio_out\" }}\npair = {{ device = {{ play = [\"a\", \"b\"] }} }}\n"
    );
    assert_eq!(problems(&two), "");

    for (ports, reason) in [
        ("[]", "1 to 64 ports"),
        ("[\"kick\", \"kick\"]", "listed twice"),
        ("[\"gain\"]", "the slot's parameter"),
        ("[\"Kick\"]", "invalid port name"),
    ] {
        let text = format!("{HEAD}[instances]\nstems = {{ device = {{ play = {ports} }} }}\n");
        let found = problems(&text);
        assert!(found.contains(reason), "{ports}: {found}");
    }
    let many: Vec<String> = (1..=65).map(|k| format!("\"s{k}\"")).collect();
    let text = format!(
        "{HEAD}[instances]\nstems = {{ device = {{ play = [{}] }} }}\n",
        many.join(", ")
    );
    assert!(problems(&text).contains("1 to 64 ports"));
}

/// ADR 0038 §7: inside another project a discrete slot is its tap, its
/// ports exported by name (`kick`, from `<slot>.kick_out`) when the project
/// names no outputs; only the host's roles reach the machine.
#[test]
fn an_included_discrete_slot_is_a_tap_of_its_ports() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("discrete-include");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("child")).unwrap();
    std::fs::write(
        dir.join("child/project.toml"),
        format!(
            "{HEAD}[instances]\nosc = {{ module = \"std.sine_osc\" }}\n\
             stems = {{ device = {{ play = [\"kick\", \"snare\"], role = \"stems\" }} }}\n\
             [wiring]\nconnections = [ {{ from = \"osc.out\", to = \"stems.kick\" }} ]\n"
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("project.toml"),
        format!(
            "{HEAD}[instances]\nx = {{ project = \"./child\" }}\nout = {{ module = \"std.audio_out\" }}\n\
             [wiring]\nconnections = [ {{ from = \"x.kick\", to = \"out.l\" }}, {{ from = \"x.snare\", to = \"out.r\" }} ]\n"
        ),
    )
    .unwrap();
    let project = Project::load(&dir).unwrap();
    let plan = project.plan(&registry()).unwrap().plan;
    let tap = plan
        .instances
        .iter()
        .find(|i| i.address.as_str() == "/x/stems/play")
        .unwrap();
    assert_eq!(tap.descriptor.type_name, "std.device_out_tap2");
    let sides = play_sides(&project, &plan);
    assert_eq!(sides.len(), 1, "only the host's sink: {sides:?}");
    assert_eq!(sides[0].address.as_str(), "/out");
}
