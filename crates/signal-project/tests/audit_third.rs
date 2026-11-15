//! The outside audit of 2026-11-15, its reproductions kept as tests.
use signal_project::{Library, Project};
use std::{
    fs,
    path::{Path, PathBuf},
};
fn fixture(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("audit-third")
        .join(name);
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}
fn write(dir: &Path, path: &str, text: &str) {
    let p = dir.join(path);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}
fn reg() -> signal_core::Registry {
    let mut r = signal_core::Registry::new();
    signal_std::register(&mut r).unwrap();
    r
}
#[test]
fn included_capture_never_opens_machine_input() {
    let base = fixture("included-capture");
    write(
        &base,
        "child/project.toml",
        "schema_version = 1\n[project]\nname = \"child\"\n[instances]\nmic = { device = { capture = \"mono\", role = \"vocal\" } }\n[exports]\noutputs = { out = \"mic.m\" }\n",
    );
    write(
        &base,
        "project.toml",
        "schema_version = 1\n[project]\nname = \"host\"\n[instances]\nx = { project = \"./child\" }\nout = { module = \"std.audio_out\" }\n[wiring]\nconnections = [{ from = \"x.out\", to = \"out.l\" }]\n",
    );
    let project = Project::load(&base).unwrap();
    // ADR 0038 §7: only the host's roles reach the machine. The included
    // capture is refused (its pass-through is not built), never routed to
    // the default microphone as `main`.
    let refused = project
        .plan(&reg())
        .err()
        .expect("an included capture is refused");
    assert!(
        refused
            .iter()
            .any(|d| d.message.contains("captures from the machine")),
        "{refused:?}"
    );
}

#[test]
fn own_copy_retargets_nested_slot_load() {
    let base = fixture("nested-load");
    let host = base.join("host");
    let src = base.join("src");
    write(
        &host,
        "project.toml",
        "schema_version = 1\n[project]\nname = \"host\"\n[instances]\ns = { slot = { outputs = { out = {} } }, load = \"../src\" }\n",
    );
    write(
        &src,
        "project.toml",
        "schema_version = 1\n[project]\nname = \"src\"\n[instances]\nx = { slot = { outputs = { out = {} } }, load = \"tone\" }\n[exports]\noutputs = { out = \"x.out\" }\n",
    );
    for (dir, hz) in [(&host, 220), (&src, 440)] {
        write(
            dir,
            "composites/tone.toml",
            &format!(
                "schema_version = 1\n[composite]\nname = \"tone\"\n[instances]\nosc = {{ module = \"std.sine_osc\", freq = {hz} }}\n[exports]\noutputs = {{ out = \"osc.out\" }}\n"
            ),
        );
    }
    let freq = |p: Project| {
        p.plan(&reg())
            .unwrap()
            .plan
            .instances
            .iter()
            .find(|i| i.address.as_str() == "/s/x/osc")
            .unwrap()
            .parameters["freq"]
    };
    assert_eq!(freq(Project::load(&host).unwrap()), 440.0);
    let p = signal_project::vendor::prepare_slot(&host, "s", Library::builtin()).unwrap();
    signal_project::vendor::write(&host, p, Library::builtin()).unwrap();
    assert_eq!(
        freq(Project::load(&host).unwrap()),
        440.0,
        "own-copy redirected the nested slot to host content"
    );
}

#[test]
fn own_copy_keeps_included_output_as_a_tap() {
    let base = fixture("own-playable-project");
    let host = base.join("host");
    let src = base.join("src");
    write(
        &host,
        "project.toml",
        "schema_version = 1\n[project]\nname = \"host\"\n[instances]\ns = { slot = { outputs = { out = {} } }, load = \"../src\" }\nout = { module = \"std.audio_out\" }\n[wiring]\nconnections = [{ from = \"s.out\", to = \"out.l\" }]\n",
    );
    write(
        &src,
        "project.toml",
        "schema_version = 1\n[project]\nname = \"src\"\n[instances]\nosc = { module = \"std.sine_osc\" }\nout = { module = \"std.audio_out\" }\n[wiring]\nconnections = [{ from = \"osc.out\", to = \"out.l\" }]\n[exports]\noutputs = { out = \"osc.out\" }\n",
    );
    let before = Project::load(&host).unwrap().plan(&reg()).unwrap().plan;
    assert_eq!(
        before
            .instances
            .iter()
            .find(|i| i.address.as_str() == "/s/out")
            .unwrap()
            .type_name,
        "std.audio_tap"
    );
    let p = signal_project::vendor::prepare_slot(&host, "s", Library::builtin()).unwrap();
    let written = signal_project::vendor::write(&host, p, Library::builtin()).unwrap();
    eprintln!("vendor reported success: {written:?}");
    let after = Project::load(&host).unwrap().plan(&reg());
    assert!(
        after.is_ok(),
        "Make own copy wrote an unplayable project: {:?}",
        after.as_ref().err()
    );
    assert_eq!(
        after
            .unwrap()
            .plan
            .instances
            .iter()
            .find(|i| i.address.as_str() == "/s/out")
            .unwrap()
            .type_name,
        "std.audio_tap"
    );
}

/// The taps a copy writes are the ones the plan uses inside another project.
#[test]
fn the_copied_taps_are_the_registered_ones() {
    let registry = reg();
    for (sink, tap) in signal_project::device::SINK_TAPS {
        assert_eq!(registry.sink_tap(sink), Some(tap), "{sink}");
    }
    for (_, sink) in signal_project::device::BUILT_LAYOUTS {
        assert!(
            signal_project::device::tap_of_sink(sink).is_some(),
            "{sink}"
        );
    }
}
