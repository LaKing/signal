//! ADR 0038 §7: a run opens its inputs when it starts, so a live edit that
//! adds or removes a capture side is refused like an output's (restart to
//! change it); an edit that keeps them passes. (A role written anew, the
//! plan unchanged, applies at the next start, as an output's.)

use std::fs;
use std::path::PathBuf;

use signal_core::Registry;
use signal_project::Project;
use signal_project::live::check_edit;

/// The project, with `extra` instances; a capture slot `mic` is wired to
/// the output's right channel (an unwired one is not planned).
fn text(extra: &str) -> String {
    let wire = if extra.contains("mic") {
        "  { from = \"mic.m\", to = \"out.r\" },\n"
    } else {
        ""
    };
    format!(
        "schema_version = 1\n[project]\nname = \"live-capture\"\nsample_rate = 48000\n\
         [instances]\nosc = {{ module = \"std.sine_osc\", freq = 220 }}\nout = {{ module = \"std.audio_out\" }}\n{extra}\
         [wiring]\nconnections = [\n  {{ from = \"osc.out\", to = \"out.l\" }},\n{wire}]\n"
    )
}

fn load(name: &str, text: &str) -> Project {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("project.toml"), text).unwrap();
    Project::load(&dir).unwrap()
}

#[test]
fn a_capture_side_added_or_removed_live_is_refused() {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let old = load("live-capture-old", &text(""));
    let mic = "mic = { device = { capture = \"mono\", role = \"voice\" } }\n";
    let added = load("live-capture-added", &text(mic));
    let plan = added.plan(&registry).unwrap().plan;
    let error = check_edit(&old, None, &added, plan, &registry).unwrap_err();
    let message = format!("{error:?}");
    assert!(message.contains("the inputs must stay none"), "{message}");

    // Removed: refused too.
    let plan = old.plan(&registry).unwrap().plan;
    let error = check_edit(&added, None, &old, plan, &registry).unwrap_err();
    assert!(format!("{error:?}").contains("inputs must stay /mic (voice)"));

    // The same inputs, another frequency: passes.
    let same = load(
        "live-capture-same",
        &text(mic).replace("freq = 220", "freq = 330"),
    );
    let plan = same.plan(&registry).unwrap().plan;
    check_edit(&added, None, &same, plan, &registry).unwrap();
}

/// ADR 0057: a run binds its `dmx`, `osc` and `midi` slots when it starts,
/// so a live edit of one's role, kind or ports is refused (restart to
/// change it), as an audio side's is; `projects/light-show`'s `lights`.
#[test]
fn an_external_side_changed_live_is_refused() {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../projects/light-show");
    let old = Project::load(&source).unwrap();
    let original = fs::read_to_string(source.join("project.toml")).unwrap();
    let lights = "role = \"lights\" } }";
    assert!(original.contains(lights));
    let copy = |name: &str, text: &str| {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for entry in walk(&source) {
            let relative = entry.strip_prefix(&source).unwrap();
            let to = dir.join(relative);
            fs::create_dir_all(to.parent().unwrap()).unwrap();
            fs::copy(&entry, &to).unwrap();
        }
        fs::write(dir.join("project.toml"), text).unwrap();
        Project::load(&dir).unwrap()
    };
    let reroled = copy(
        "live-external-role",
        &original.replace(lights, "role = \"stage\" } }"),
    );
    let plan = reroled.plan(&registry).unwrap().plan;
    let error = check_edit(&old, None, &reroled, plan, &registry).unwrap_err();
    let message = format!("{error:?}");
    assert!(
        message.contains("the external sides must stay"),
        "{message}"
    );
    assert!(message.contains("dmx lights"), "{message}");

    // The same sides, the project otherwise as it was: passes.
    let same = copy("live-external-same", &original);
    let plan = same.plan(&registry).unwrap().plan;
    check_edit(&old, None, &same, plan, &registry).unwrap();
}

/// Every file under `dir`, `devices/` and `past-renders/` left out.
fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "devices" && name != "past-renders" {
                files.extend(walk(&path));
            }
        } else {
            files.push(path);
        }
    }
    files
}
