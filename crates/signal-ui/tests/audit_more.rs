//! Additional audit regressions.
use signal_project::Project;
use std::{
    fs,
    path::{Path, PathBuf},
};
fn write(dir: &Path, path: &str, text: &str) {
    let p = dir.join(path);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}
fn fixture(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("audit-more/{name}"));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}
fn registry() -> signal_core::Registry {
    let mut r = signal_core::Registry::new();
    signal_std::register(&mut r).unwrap();
    r
}
fn freq(dir: &Path, address: &str) -> f64 {
    let plan = Project::load(dir).unwrap().plan(&registry()).unwrap().plan;
    plan.instances
        .iter()
        .find(|i| i.address.to_string() == address)
        .unwrap()
        .parameters["freq"]
}
#[test]
fn importing_identical_parent_with_different_child_preserves_sound() {
    let base = fixture("stack-dependencies");
    let src = base.join("src");
    let dst = base.join("dst");
    for (dir, name, slot, hz) in [(&src, "rack", "s", 440), (&dst, "host", "existing", 220)] {
        write(
            dir,
            "project.toml",
            &format!(
                "schema_version = 1\n[project]\nname = \"{name}\"\n[instances]\n{slot} = {{ slot = {{ outputs = {{ out = {{}} }} }}, load = \"voice\" }}\n"
            ),
        );
        write(
            dir,
            "composites/voice.toml",
            "schema_version = 1\n[composite]\nname = \"voice\"\n[instances]\nx = { module = \"tone\" }\n[exports]\noutputs = { out = \"x.out\" }\n",
        );
        write(
            dir,
            "composites/tone.toml",
            &format!(
                "schema_version = 1\n[composite]\nname = \"tone\"\n[instances]\nosc = {{ module = \"std.sine_osc\", freq = {hz} }}\n[exports]\noutputs = {{ out = \"osc.out\" }}\n"
            ),
        );
    }
    assert_eq!(freq(&src, "/s/x/osc"), 440.0);
    let (copies, edits, _) = signal_ui::stacks::import_stack(&dst, &src).unwrap();
    eprintln!(
        "copies: {:?}",
        copies.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
    let library = signal_project::Library::builtin();
    let prepared = signal_project::vendor::Prepared::new(&dst, copies, edits, library).unwrap();
    signal_project::vendor::write(&dst, prepared, library).unwrap();
    assert_eq!(freq(&dst, "/existing/x/osc"), 220.0);
    assert_eq!(
        freq(&dst, "/s/x/osc"),
        440.0,
        "import silently substitutes host dependency"
    );
}

#[test]
fn making_linked_project_own_preserves_dependency_context() {
    let base = fixture("vendor-dependencies");
    let src = base.join("src");
    let dst = base.join("dst");
    write(
        &src,
        "project.toml",
        "schema_version = 1\n[project]\nname = \"source\"\n[instances]\nx = { module = \"voice\" }\n[exports]\noutputs = { out = \"x.out\" }\n",
    );
    write(
        &dst,
        "project.toml",
        "schema_version = 1\n[project]\nname = \"host\"\n[instances]\ns = { slot = { outputs = { out = {} } }, load = \"../src\" }\n",
    );
    for (dir, hz) in [(&src, 440), (&dst, 220)] {
        write(
            dir,
            "composites/voice.toml",
            "schema_version = 1\n[composite]\nname = \"voice\"\n[instances]\nx = { module = \"tone\" }\n[exports]\noutputs = { out = \"x.out\" }\n",
        );
        write(
            dir,
            "composites/tone.toml",
            &format!(
                "schema_version = 1\n[composite]\nname = \"tone\"\n[instances]\nosc = {{ module = \"std.sine_osc\", freq = {hz} }}\n[exports]\noutputs = {{ out = \"osc.out\" }}\n"
            ),
        );
    }
    assert_eq!(freq(&dst, "/s/x/x/osc"), 440.0);
    let prepared =
        signal_project::vendor::prepare_slot(&dst, "s", signal_project::Library::builtin())
            .unwrap();
    let after = Project::from_sources_in(prepared.after(), &dst)
        .unwrap()
        .plan(&registry())
        .unwrap()
        .plan;
    let value = after
        .instances
        .iter()
        .find(|i| i.address.as_str() == "/s/x/x/osc")
        .unwrap()
        .parameters["freq"];
    assert_eq!(
        value, 440.0,
        "own-copy silently substitutes host dependency"
    );
}
