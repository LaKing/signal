//! M2.8-4: Open project, the app's rule shared with the server in the app
//! (`signal_ui::app`): it stops playback, loads the folder for its rate and
//! diagnostics, and records it in the Setup as the last project.

use std::fs;
use std::path::{Path, PathBuf};

use signal_app::setup::Setup;
use signal_app::state::{AppState, Backend, Command};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(name)
}

/// A folder of its own for each test, with a Setup file.
fn base(name: &str) -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("signal-open-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    let setup = base.join("setup.toml");
    Setup::default().save(&setup).unwrap();
    (base, setup)
}

#[test]
fn opening_a_project_stops_and_records_it_in_the_setup() {
    let (base, setup) = base("kick");
    let tone = fixture("tests/fixtures/tone");
    let kick = fixture("../../projects/kick");
    let mut state = AppState::new(setup.clone(), Some(tone), Backend::Null(Vec::new()));
    state.apply(Command::Play);
    assert!(state.view().playing, "{:?}", state.view().message);

    state.apply(Command::OpenProject(kick.clone()));
    let view = state.view();
    assert!(!view.playing, "opening stops");
    assert_eq!(view.project, kick.display().to_string());
    assert_eq!(view.sample_rate, Some(96_000));
    assert!(view.diagnostics.is_empty());
    assert_eq!(Setup::load(&setup).0.last_project, Some(kick));
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn a_project_that_does_not_load_shows_its_diagnostics_and_does_not_play() {
    let (base, setup) = base("broken");
    let broken = base.join("broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(
        broken.join("project.toml"),
        "schema_version = 1\n[instances\n",
    )
    .unwrap();
    let mut state = AppState::new(setup.clone(), None, Backend::Null(Vec::new()));
    state.apply(Command::OpenProject(broken.clone()));
    let view = state.view();
    assert_eq!(view.sample_rate, None);
    assert!(!view.diagnostics.is_empty());
    assert_eq!(Setup::load(&setup).0.last_project, Some(broken));
    state.apply(Command::Play);
    let view = state.view();
    assert!(!view.playing);
    assert_eq!(view.message.as_deref(), Some("the project does not load"));
    fs::remove_dir_all(&base).unwrap();
}
