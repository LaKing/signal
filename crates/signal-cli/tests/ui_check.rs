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
             signal-xy, signal-button, signal-toggle, signal-meter, signal-scope, signal-value, \
             signal-canvas, signal-piano-roll, signal-step-grid"
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

/// M2.11-1 (ADR 0028 §7): the pattern controls take no `bind`; their
/// `pattern` names a project pattern of their kind (`notes` for the piano
/// roll, `steps` for the step grid), else an error at the panel line.
#[test]
fn pattern_controls_name_a_pattern_of_their_kind() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ui-check-patterns");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("patterns")).unwrap();
    fs::create_dir_all(dir.join("ui")).unwrap();
    fs::write(
        dir.join("project.toml"),
        "schema_version = 1\n\n[project]\nname = \"patterns\"\n\n[instances]\nclock = { module = \"std.transport\" }\nseq = { module = \"std.note_seq\", pattern = \"melody\" }\nout = { module = \"std.audio_out\" }\n\n[wiring]\nconnections = [\n  { from = \"clock.tick\", to = \"seq.tick\" },\n  { from = \"seq.gate\", to = \"out.l\" },\n]\n",
    )
    .unwrap();
    fs::write(
        dir.join("patterns/melody.toml"),
        "schema_version = 1\nkind = \"notes\"\nlength = 4\nnotes = [\n  { key = 60, start = 0, length = 1, velocity = 1 },\n]\n",
    )
    .unwrap();
    fs::write(
        dir.join("patterns/drums.toml"),
        "schema_version = 1\nkind = \"steps\"\nsteps = 4\nstep = 0.25\n\n[lanes]\nkick = \"x...\"\n",
    )
    .unwrap();
    let good = "<signal-panel>\n<signal-piano-roll pattern=\"melody\" label=\"melody\"></signal-piano-roll>\n<signal-step-grid pattern=\"drums\" step=\"0\"></signal-step-grid>\n</signal-panel>\n";
    fs::write(dir.join("ui/main.html"), good).unwrap();
    let output = check(&dir, false);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains("ui/main.html: 2 controls, 0 bindings: ok"),
        "{stderr}"
    );

    let bad = "<signal-panel>\n<signal-piano-roll pattern=\"drums\"></signal-piano-roll>\n<signal-step-grid pattern=\"beat\"></signal-step-grid>\n<signal-step-grid bind=\"/seq.tick\"></signal-step-grid>\n</signal-panel>\n";
    let panel = dir.join("ui/main.html");
    fs::write(&panel, bad).unwrap();
    let output = check(&dir, false);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    let file = panel.display();
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(
        lines,
        [
            format!(
                "{file}:2:29: error: wrong kind: patterns/drums.toml is a `steps` pattern; <signal-piano-roll> edits a `notes` pattern"
            ),
            format!(
                "{file}:3:28: error: unknown pattern `beat`: the project has no patterns/beat.toml; <signal-step-grid> edits a `steps` pattern"
            ),
            format!(
                "{file}:4:25: error: <signal-step-grid> takes no bind: it edits the `steps` pattern its `pattern` attribute names"
            ),
            format!(
                "{file}:4:1: error: <signal-step-grid> has no pattern: it needs pattern=\"<name>\", a `steps` pattern of the project (patterns/<name>.toml)"
            ),
        ],
        "{stderr}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// M2.11-3: a pattern control's playhead attribute (`position-bind` on the
/// piano roll, `step-bind` on the step grid) names a signal output of the
/// plan; an unknown port, an input or a parameter is an error at its value.
#[test]
fn pattern_controls_bind_their_playhead_to_an_output() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ui-check-playheads");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("patterns")).unwrap();
    fs::create_dir_all(dir.join("ui")).unwrap();
    fs::write(
        dir.join("project.toml"),
        "schema_version = 1\n\n[project]\nname = \"playheads\"\n\n[instances]\nclock = { module = \"std.transport\" }\nseq = { module = \"std.note_seq\", pattern = \"melody\" }\ndrums = { module = \"std.step_seq8\", pattern = \"drums\" }\nout = { module = \"std.audio_out\" }\n\n[wiring]\nconnections = [\n  { from = \"clock.tick\", to = \"seq.tick\" },\n  { from = \"clock.tick\", to = \"drums.tick\" },\n  { from = \"seq.gate\", to = \"out.l\" },\n]\n",
    )
    .unwrap();
    fs::write(
        dir.join("patterns/melody.toml"),
        "schema_version = 1\nkind = \"notes\"\nlength = 4\nnotes = [\n  { key = 60, start = 0, length = 1, velocity = 1 },\n]\n",
    )
    .unwrap();
    fs::write(
        dir.join("patterns/drums.toml"),
        "schema_version = 1\nkind = \"steps\"\nsteps = 4\nstep = 0.25\n\n[lanes]\nkick = \"x...\"\n",
    )
    .unwrap();
    let good = "<signal-panel>\n<signal-piano-roll pattern=\"melody\" position-bind=\"/seq.position\"></signal-piano-roll>\n<signal-step-grid pattern=\"drums\" step-bind=\"/drums.step\"></signal-step-grid>\n</signal-panel>\n";
    let panel = dir.join("ui/main.html");
    fs::write(&panel, good).unwrap();
    let output = check(&dir, false);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains("ui/main.html: 2 controls, 2 bindings: ok"),
        "{stderr}"
    );

    let bad = "<signal-panel>\n<signal-piano-roll pattern=\"melody\" position-bind=\"/seq.nope\"></signal-piano-roll>\n<signal-step-grid pattern=\"drums\" step-bind=\"/out.l\"></signal-step-grid>\n<signal-step-grid pattern=\"drums\" step-bind=\"/clock.bpm\"></signal-step-grid>\n<signal-piano-roll pattern=\"melody\" position-bind=\"\"></signal-piano-roll>\n</signal-panel>\n";
    fs::write(&panel, bad).unwrap();
    let output = check(&dir, false);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    let file = panel.display();
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(lines.len(), 4, "{stderr}");
    assert!(
        lines[0].starts_with(&format!("{file}:2:52: error: ")) && lines[0].contains("/seq.nope"),
        "{stderr}"
    );
    assert_eq!(
        lines[1],
        format!(
            "{file}:3:46: error: wrong kind: /out.l is a signal input of the audio sink; <signal-step-grid> step-bind takes a signal output of the plan (its playhead is read, never written)"
        ),
        "{stderr}"
    );
    assert_eq!(
        lines[2],
        format!(
            "{file}:4:46: error: wrong kind: /clock.bpm is a parameter; <signal-step-grid> step-bind takes a signal output of the plan (its playhead is read, never written)"
        ),
        "{stderr}"
    );
    assert_eq!(
        lines[3],
        format!(
            "{file}:5:52: error: <signal-piano-roll> position-bind is empty: it needs a signal output of the plan (its playhead is read, never written)"
        ),
        "{stderr}"
    );
    let _ = fs::remove_dir_all(&dir);
}
