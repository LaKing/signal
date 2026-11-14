//! Focused regression probes, kept only in the audit snapshot.
use signal_project::{Project, SourceFile, archive, live::ProjectWatcher};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn dir(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("audit-20261114-{name}"));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}
fn write(p: &Path, include: Option<&str>) {
    fs::create_dir_all(p).unwrap();
    let inc = include
        .map(|i| format!("inc = {{ project = {i:?} }}\n"))
        .unwrap_or_default();
    fs::write(p.join("project.toml"), format!("schema_version = 1\n[project]\nname = \"audit\"\nseed = 7\n[instances]\n{inc}out = {{ module = \"std.audio_out\" }}\n")).unwrap();
}

#[test]
fn embedded_external_project_keeps_its_internal_include() {
    let base = dir("embedded-nested");
    let host = base.join("host");
    write(&host, Some("../outside"));
    write(&base.join("outside"), Some("child"));
    write(&base.join("outside/child"), None);
    Project::load(&host).unwrap();
    let bytes = archive::pack_embedded(&host, None).unwrap();
    let dest = base.join("unpacked");
    archive::unpack(&bytes, &dest).unwrap();
    assert!(
        dest.join("includes/inc/child/project.toml").is_file(),
        "nested included project was silently omitted"
    );
    Project::load(&dest).unwrap();
}

#[test]
fn plain_pack_refuses_outside_dependency_of_internal_include() {
    let base = dir("pack-transitive");
    let host = base.join("host");
    write(&host, Some("child"));
    write(&host.join("child"), Some("../../outside"));
    write(&base.join("outside"), None);
    Project::load(&host).unwrap();
    assert!(
        archive::pack(&host).is_err(),
        "pack succeeded with a dependency missing from its archive"
    );
}

#[test]
fn watcher_discovers_new_dependency_of_external_include() {
    let base = dir("watch-transitive");
    let host = base.join("host");
    let outside = base.join("outside");
    let leaf = base.join("leaf");
    write(&host, Some("../outside"));
    write(&outside, None);
    write(&leaf, None);
    let mut watch = ProjectWatcher::new(&host).unwrap();
    write(&outside, Some("../leaf"));
    assert!(!watch.changes().unwrap().is_empty());
    let path = leaf.join("project.toml");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str("\n# edited leaf after it was included\n");
    fs::write(&path, text).unwrap();
    assert!(
        !watch.changes().unwrap().is_empty(),
        "new nested include is never watched"
    );
}

#[test]
fn exported_slot_preserves_structured_parameter_value() {
    let project = Project::from_sources(vec![
        SourceFile { path: "project.toml".into(), text: "schema_version = 1\n[project]\nname = \"audit\"\n[instances]\ns = { slot = { parameters = { level = {} } }, load = \"voice\", level = -20 }\n".into() },
        SourceFile { path: "composites/voice.toml".into(), text: "schema_version = 1\n[composite]\nname = \"voice\"\n[instances]\ng = { module = \"std.gain\", db = -3 }\n[exports]\nparameters = { level = { to = \"g.db\", min = -40, max = 0 } }\n".into() }
    ]).unwrap();
    let mut registry = signal_core::Registry::new();
    signal_std::register(&mut registry).unwrap();
    let before = project.plan(&registry).unwrap();
    assert_eq!(
        before
            .plan
            .instances
            .iter()
            .find(|i| i.address.as_str() == "/s/g")
            .unwrap()
            .parameters["db"],
        -20.0
    );
    let exported = signal_project::export::export_slot(&project, "s", None).unwrap();
    let after = Project::from_sources(exported)
        .unwrap()
        .plan(&registry)
        .unwrap();
    assert_eq!(
        after
            .plan
            .instances
            .iter()
            .find(|i| i.address.as_str() == "/g")
            .unwrap()
            .parameters["db"],
        -20.0,
        "export silently drops the slot's value"
    );
}
