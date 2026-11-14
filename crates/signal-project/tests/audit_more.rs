//! Additional audit regressions.
use signal_project::{Project, ProjectEdit, SourceFile};
fn reg() -> signal_core::Registry {
    let mut r = signal_core::Registry::new();
    signal_std::register(&mut r).unwrap();
    r
}
fn source(text: String) -> Project {
    Project::from_sources(vec![SourceFile {
        path: "project.toml".into(),
        text,
    }])
    .unwrap()
}
fn device(role: &str, capture: bool) -> Project {
    let main = if capture {
        "out = { module = \"std.audio_out\" }\n"
    } else {
        ""
    };
    let side = if capture { "capture" } else { "play" };
    source(format!(
        "schema_version = 1\n[project]\nname = \"role\"\n[instances]\nd = {{ device = {{ {side} = \"stereo\", role = \"{role}\" }} }}\n{main}"
    ))
}
#[test]
fn changing_output_role_requires_restart() {
    check_role(false);
}
#[test]
fn changing_capture_role_requires_restart() {
    check_role(true);
}
fn check_role(capture: bool) {
    let r = reg();
    let old = device("main", capture);
    let new = device("cue", capture);
    let plan = new.plan(&r).unwrap().plan;
    eprintln!("diff: {:?}", signal_project::diff(&old, &new, &r).unwrap());
    let result = signal_project::live::check_edit(&old, None, &new, plan, &r);
    let before = old.plan(&r).unwrap().plan;
    let (_runner, mut controller) = signal_output::realtime(
        &before,
        &r,
        signal_engine::RealtimeConfig::new(old.sample_rate, old.seed),
    )
    .unwrap();
    let provenance =
        signal_project::provenance::collect(&signal_project::provenance::source_dir(), &old.files);
    let recorder = signal_project::session::SessionRecorder::new(
        &old,
        &r,
        old.sample_rate,
        old.seed,
        provenance,
    )
    .unwrap();
    let mut live = signal_project::session::LiveRecorder::new(recorder, r.clone());
    let sent = signal_project::live::send_edit(&mut controller, &mut live, &old, new, &r);
    eprintln!("send_edit: {:?}", sent.as_ref().map(|(_, a)| a));
    assert!(
        result.is_err() && sent.is_err(),
        "re-role accepted without rebuilding fixed run routing: {result:?}"
    );
}
#[test]
fn changing_sleep_policy_requires_plan_swap() {
    let r = reg();
    let old = Project::load(
        &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../projects/studio-poly"),
    )
    .unwrap();
    let mut files = old.files.clone();
    let top = files.iter_mut().find(|f| f.path == "project.toml").unwrap();
    top.text = top
        .text
        .replace("voices = 8", "voices = { count = 8, sleep = false }");
    let new = Project::from_sources(files).unwrap();
    assert_eq!(old.plan(&r).unwrap().plan.sleep_groups.len(), 8);
    assert_eq!(new.plan(&r).unwrap().plan.sleep_groups.len(), 0);
    let diff = signal_project::diff(&old, &new, &r).unwrap();
    assert!(
        matches!(diff, ProjectEdit::Structural { .. }),
        "sleep semantic change discarded: {diff:?}"
    );
}
