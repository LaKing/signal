//! M2.8-12 project files: `signal pack` and `signal unpack` (AC10), and
//! `render`, `play` and `ui check` taking a .signal file wherever they take
//! a project folder, from a temporary folder removed after the run (AC11),
//! with a render identical to the folder's (AC12).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn projects() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects")
}

/// A fresh folder for one test, inside the target directory, with a
/// temporary folder of its own for the runs (`TMPDIR`).
fn base(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("project-files")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("tmp")).unwrap();
    dir
}

fn signal(base: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(args)
        .env("TMPDIR", base.join("tmp"))
        .output()
        .expect("the signal binary runs")
}

fn text(path: &Path) -> String {
    path.display().to_string()
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "stdout is JSON: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// Every run's temporary folder is gone after it.
fn no_temporary_left(base: &Path) {
    let left: Vec<_> = fs::read_dir(base.join("tmp")).unwrap().flatten().collect();
    assert!(left.is_empty(), "{left:?}");
}

fn pack(base: &Path, project: &str) -> PathBuf {
    let file = base.join(format!("{project}.signal"));
    let out = signal(
        base,
        &[
            "pack",
            &text(&projects().join(project)),
            &text(&file),
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(json(&out)["file"], text(&file));
    file
}

/// AC10: pack, then unpack gives the project's files back; packing again
/// gives the same bytes.
#[test]
fn pack_and_unpack_round_trip() {
    let base = base("round-trip");
    let file = pack(&base, "kick");
    let folder = base.join("kick");
    let out = signal(&base, &["unpack", &text(&file), &text(&folder), "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let files = json(&out)["files"].clone();
    assert_eq!(
        files,
        serde_json::json!([
            "composites/drums.toml",
            "composites/kick.toml",
            "project.toml",
            "ui/assets/knob.svg",
            "ui/main.html"
        ])
    );
    for name in files.as_array().unwrap() {
        let name = name.as_str().unwrap();
        assert_eq!(
            fs::read(folder.join(name)).unwrap(),
            fs::read(projects().join("kick").join(name)).unwrap(),
            "{name}"
        );
    }
    let again = base.join("again.signal");
    let out = signal(&base, &["pack", &text(&folder), &text(&again)]);
    assert!(out.status.success());
    assert_eq!(
        fs::read(&again).unwrap(),
        fs::read(&file).unwrap(),
        "deterministic"
    );
}

/// AC7, AC10: a refused archive writes nothing and names the reason (exit 1,
/// `archive`); a folder without project.toml is not packed.
#[test]
fn refusals_exit_1_with_the_reason() {
    let base = base("refusals");
    let bad = base.join("bad.signal");
    fs::write(&bad, "not a zip").unwrap();
    let into = base.join("out");
    let out = signal(&base, &["unpack", &text(&bad), &text(&into), "--json"]);
    assert_eq!(out.status.code(), Some(1));
    let error = &json(&out)["error"];
    assert_eq!(error["kind"], "archive");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("not a .signal archive"),
        "{error}"
    );
    assert!(!into.exists(), "nothing is written");

    let file = pack(&base, "pulse");
    fs::create_dir_all(&into).unwrap();
    fs::write(into.join("mine.txt"), "keep").unwrap();
    let out = signal(&base, &["unpack", &text(&file), &text(&into)]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("exists and is not empty"));
    assert_eq!(fs::read_dir(&into).unwrap().count(), 1);

    let empty = base.join("empty");
    fs::create_dir_all(&empty).unwrap();
    let out = signal(
        &base,
        &[
            "pack",
            &text(&empty),
            &text(&base.join("e.signal")),
            "--json",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        json(&out)["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no project.toml")
    );
    assert!(!base.join("e.signal").exists());

    let out = signal(
        &base,
        &[
            "render",
            &text(&bad),
            "--frames",
            "10",
            "--out",
            &text(&base.join("r")),
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "a refused .signal renders nothing"
    );
    no_temporary_left(&base);
}

/// AC11, AC12: rendering a .signal file gives the folder's manifest: the
/// same audio and log hashes and the same project files.
#[test]
fn a_signal_file_renders_as_its_folder() {
    let base = base("render");
    for project in ["kick", "pulse", "physical"] {
        let file = pack(&base, project);
        let render = |source: &Path, out: &str| {
            let out = signal(
                &base,
                &[
                    "render",
                    &text(source),
                    "--frames",
                    "9600",
                    "--out",
                    &text(&base.join(out)),
                    "--json",
                ],
            );
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            json(&out)["manifest"].clone()
        };
        let folder = render(&projects().join(project), &format!("{project}-folder"));
        let packed = render(&file, &format!("{project}-file"));
        assert_eq!(packed, folder, "{project}");
        assert!(folder["audio_fnv1a64"].is_string());
    }
    no_temporary_left(&base);
}

/// AC11: `play` and `ui check` take a .signal file too, and say where its
/// edits go (a temporary copy) when watching.
#[test]
fn play_and_ui_check_take_a_signal_file() {
    let base = base("play");
    let file = pack(&base, "pulse");
    let out = signal(
        &base,
        &[
            "play",
            &text(&file),
            "--driver",
            "null",
            "--fast",
            "--seconds",
            "0.1",
            "--watch",
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(json(&out)["frames"].as_u64().unwrap() > 0);
    assert!(String::from_utf8_lossy(&out.stderr).contains("played from a temporary copy"));

    let out = signal(&base, &["ui", "check", &text(&file), "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let checked = json(&out);
    assert_eq!(checked["panels"][0]["file"], "ui/main.html");
    assert_eq!(checked["generated"], false);
    no_temporary_left(&base);
}
