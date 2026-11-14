//! Additional audit probes. These files live only in the audit snapshot.
use std::{
    fs,
    path::{Path, PathBuf},
};

fn base(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("audit-more-{name}"));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}
fn put(dir: &Path, path: &str, text: &str) {
    let full = dir.join(path);
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    fs::write(full, text).unwrap();
}
fn registry() -> signal_core::Registry {
    let mut r = signal_core::Registry::new();
    signal_std::register(&mut r).unwrap();
    r
}

#[test]
fn loaded_project_panel_control_can_be_saved() {
    let dir = base("slot-panel");
    put(
        &dir,
        "project.toml",
        "schema_version = 1\n[project]\nname = \"host\"\n[instances]\ninc = { slot = {}, load = \"./inc\" }\n",
    );
    put(
        &dir,
        "inc/project.toml",
        "schema_version = 1\n[project]\nname = \"inc\"\n[instances]\nosc = { module = \"std.sine_osc\", freq = 220 }\n",
    );
    let p = signal_project::Project::load(&dir).unwrap();
    let panel = signal_ui::panel::Panel::new(&p, &registry());
    assert!(
        panel.target("inc/osc", "freq").is_some(),
        "loaded panel's control is not editable"
    );
    let changed =
        signal_project::write_parameters_in(&p.files, &[("inc/osc", "freq", Some(330.0), &[])]);
    assert!(
        changed.is_ok(),
        "an advertised control cannot be committed: {changed:?}"
    );
}

#[test]
fn stack_import_preserves_inherited_random_seed() {
    let dir = base("stack-seed");
    let from = dir.join("source");
    let host = dir.join("host");
    put(
        &from,
        "project.toml",
        "schema_version = 1\n[project]\nname = \"rack\"\nseed = 7\n[instances]\ns = { slot = { outputs = { out = {} } }, load = \"s_inside\", seed = \"run\" }\n",
    );
    put(
        &from,
        "composites/s_inside.toml",
        "schema_version = 1\n[composite]\nname = \"s_inside\"\ninside_of = \"s\"\n[instances]\nn = { module = \"std.noise\" }\n[exports]\noutputs = { out = \"n.out\" }\n",
    );
    put(
        &host,
        "project.toml",
        "schema_version = 1\n[project]\nname = \"host\"\nseed = 99\n[instances]\n",
    );
    let before = signal_project::Project::load(&from).unwrap();
    let (copies, edits, _) = signal_ui::stacks::import_stack(&host, &from).unwrap();
    for (p, text) in copies.iter().chain(&edits) {
        put(&host, p, text);
    }
    let after = signal_project::Project::load(&host).unwrap();
    let r = registry();
    let effective = |p: &signal_project::Project| {
        let plan = p.plan(&r).unwrap();
        let n = plan
            .plan
            .instances
            .iter()
            .find(|i| i.type_name == "std.noise")
            .unwrap();
        n.stream.as_ref().and_then(|s| s.seed).unwrap_or(p.seed)
    };
    assert_eq!(
        effective(&after),
        effective(&before),
        "copying an instrument into another project changes its random stream"
    );
}
