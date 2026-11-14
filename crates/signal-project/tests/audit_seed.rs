//! Audit reproduction for an explicitly seeded slot's export.
use signal_project::{Project, SourceFile};

#[test]
fn export_preserves_explicit_slot_seed() {
    let p = Project::from_sources(vec![
        SourceFile { path: "project.toml".into(), text: "schema_version = 1\n[project]\nname = \"audit\"\nseed = 99\n[instances]\ns = { slot = { outputs = { out = {} } }, load = \"voice\", seed = 7 }\n".into() },
        SourceFile { path: "composites/voice.toml".into(), text: "schema_version = 1\n[composite]\nname = \"voice\"\n[instances]\nn = { module = \"std.noise\" }\n[exports]\noutputs = { out = \"n.out\" }\n".into() }
    ]).unwrap();
    let mut r = signal_core::Registry::new();
    signal_std::register(&mut r).unwrap();
    let before = p.plan(&r).unwrap();
    assert_eq!(
        before.plan.instances[0].stream.as_ref().unwrap().seed,
        Some(7)
    );
    let files = signal_project::export::export_slot(&p, "s", None).unwrap();
    let after = Project::from_sources(files).unwrap();
    assert_eq!(
        after.seed, 7,
        "an explicitly seeded instrument changes its random stream when exported"
    );
}
