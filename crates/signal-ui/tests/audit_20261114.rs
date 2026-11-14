//! Stack travel regression probes kept only in the audit snapshot.
use std::fs;

#[test]
fn stack_export_keeps_shared_project_composite() {
    let base = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("audit-stack-shared");
    let _ = fs::remove_dir_all(&base);
    let host = base.join("host");
    fs::create_dir_all(host.join("composites")).unwrap();
    fs::create_dir_all(host.join("ui")).unwrap();
    fs::write(host.join("project.toml"), "schema_version = 1\n[project]\nname = \"audit\"\n[instances]\ns = { slot = { outputs = { out = {} } }, load = \"voice\" }\n").unwrap();
    fs::write(host.join("composites/voice.toml"), "schema_version = 1\n[composite]\nname = \"voice\"\n[instances]\nn = { module = \"std.noise\" }\n[exports]\noutputs = { out = \"n.out\" }\n").unwrap();
    fs::write(host.join("ui/plane.toml"), "schema_version = 1\n[[stack]]\nname = \"rack\"\nat = [0, 0]\nwidth = 160\ninstruments = [\"s\"]\n").unwrap();
    let project = signal_project::Project::load(&host).unwrap();
    let mut registry = signal_core::Registry::new();
    signal_std::register(&mut registry).unwrap();
    project.plan(&registry).unwrap();
    let files = signal_ui::stacks::export_stack(&host, "rack", &base.join("out")).unwrap();
    assert!(
        files.iter().any(|(p, _)| p == "composites/voice.toml"),
        "export omits the slot's loaded module: {files:?}"
    );
}
