//! `signal ui check` (M2.8-3 AC1): the example panels pass; a broken copy
//! of the kick's panel fails with `file:line:column` diagnostics for an
//! unknown port, wrong kinds, a parameter that `commit` cannot write and an
//! unknown control, reading binds with character references decoded; a
//! project without panels gets the generated one.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn check(project: &Path, json: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_signal"));
    if json {
        command.arg("--json");
    }
    command.args(["ui", "check"]).arg(project).output().unwrap()
}

/// Copies the folder `from` to `to` (files and folders only).
fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy(&path, &target);
        } else {
            fs::copy(&path, &target).unwrap();
        }
    }
}

/// The 1-based line and column of the first `needle` in `text`.
fn position(text: &str, needle: &str) -> (usize, usize) {
    let offset = text.find(needle).unwrap_or_else(|| panic!("{needle}"));
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().unwrap().chars().count() + 1;
    (line, column)
}

#[test]
fn the_example_panels_pass() {
    for (name, controls, bindings) in [("kick", 6, 6), ("pulse", 18, 19)] {
        let project = root().join("projects").join(name);
        let output = check(&project, false);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{name}: {stderr}");
        assert!(
            stderr.contains(&format!(
                "ui/main.html: {controls} controls, {bindings} bindings: ok"
            )),
            "{name}: {stderr}"
        );
        assert!(!stderr.contains("warning"), "{name}: {stderr}");
    }
}

#[test]
fn a_broken_panel_fails_with_located_diagnostics() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ui-check-broken-kick");
    let _ = fs::remove_dir_all(&dir);
    copy(&root().join("projects/kick"), &dir);
    let panel = dir.join("ui/main.html");
    let text = fs::read_to_string(&panel)
        .unwrap()
        // An unknown port, spelt with a character reference: the message
        // names the decoded address, the position is the source's.
        .replace(
            "bind=\"/drums.kick_decay\"",
            "bind=\"/drums&#46;kick_decya\"",
        )
        // A parameter inside the composite: a knob could not commit it.
        .replace(
            "bind=\"/drums.kick_drive\" taper",
            "bind=\"/drums/kick/amp_env.decay_ms\" taper",
        )
        // Wrong kinds: a meter on a parameter, a scope on a parameter.
        .replace("bind=\"/out.r\"", "bind=\"/drums.kick_drive\"")
        .replace("bind=\"/drums/kick/dc.out\"", "bind=\"/out.gain\"")
        // Valid, with a named character reference: no diagnostic.
        .replace("bind=\"/out.l\"", "bind=\"/out&period;l\"")
        // A misspelt control.
        .replace("<signal-fader bind", "<signal-fadr bind");
    fs::write(&panel, &text).unwrap();
    let output = check(&dir, false);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Shown with `--nocapture`: the diagnostics as a person reads them.
    eprint!("{stderr}");
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    let file = panel.display();
    let at = |needle: &str, skip: usize| {
        let (line, column) = position(text.as_str(), needle);
        (line, column + skip)
    };
    let expected = [
        (
            at("/drums&#46;kick_decya", 0),
            "error: unknown port /drums.kick_decya: /drums has kick_decay, kick_drive".to_owned(),
        ),
        (
            at("/drums/kick/amp_env.decay_ms", 0),
            "error: not editable: /drums/kick/amp_env.decay_ms is a parameter inside the \
             composite /drums, which `commit` cannot write; <signal-knob> binds a control (a \
             top-level parameter, /<instance>.<parameter>): export it from the composite, or \
             show it with <signal-value>"
                .to_owned(),
        ),
        (
            at("<signal-fadr", 0),
            "error: unknown control <signal-fadr>; the controls are signal-fader, signal-knob, \
             signal-xy, signal-button, signal-toggle, signal-meter, signal-scope, signal-value"
                .to_owned(),
        ),
        (
            at("bind=\"/drums.kick_drive\" label=\"R\"", 6),
            "error: wrong kind: /drums.kick_drive is a parameter; <signal-meter> binds a signal \
             input of the audio sink"
                .to_owned(),
        ),
        (
            at("bind=\"/out.gain\" rate", 6),
            "error: wrong kind: /out.gain is a parameter; <signal-scope> binds a signal output or \
             input of the plan"
                .to_owned(),
        ),
    ];
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(lines.len(), expected.len(), "{stderr}");
    for (((line, column), message), got) in expected.iter().zip(&lines) {
        assert_eq!(*got, format!("{file}:{line}:{column}: {message}"));
    }

    // --json: the same diagnostics, structured.
    let output = check(&dir, true);
    assert_eq!(output.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["kind"], "validation");
    assert_eq!(value["error"]["message"], "the panels have 5 errors");
    let diagnostics = value["error"]["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 5);
    assert_eq!(diagnostics[0]["file"], file.to_string());
    assert_eq!(diagnostics[0]["line"], expected[0].0.0);
    assert_eq!(diagnostics[0]["column"], expected[0].0.1);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_project_without_panels_gets_the_generated_one() {
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sine");
    let output = check(&project, false);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains("no ui/main.html; the main panel is generated from the plan"),
        "{stderr}"
    );
    let output = check(&project, true);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["panels"], serde_json::json!([]));
    assert_eq!(value["generated"], true);
}
