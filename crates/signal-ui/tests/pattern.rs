//! M2.11-1 (ADR 0028 §7, `docs/ui-protocol.md` "`pattern.read`,
//! `pattern.write`"): pattern files read and written over both transports,
//! as `panel.write` and `patch` write: a write and its push, applied live by
//! the watch (a swap); a conflict; text the project would not load with,
//! refused with the problem named; a wrong kind; names, not paths, and never
//! through a link; a first write creating the file; and a message over the
//! 64 KiB limit, which closes the connection and writes nothing. On a
//! fixture project; no committed project is written. (A `.signal` project
//! marked edited: `app_mode.rs`.)

mod common;

use std::fs;
use std::path::PathBuf;

use common::{Got, Options, Rig, Transport, Ws, receive, rig, send};
use serde_json::{Value, json};
use signal_ui::panel_write::sha256_hex;

const PROJECT: &str = r#"schema_version = 1

[project]
name = "melody"
sample_rate = 48000
seed = 1

[instances]
clock = { module = "std.transport", bpm = 120 }
seq = { module = "std.note_seq", pattern = "melody" }
osc = { module = "std.sine_osc" }
out = { module = "std.audio_out", gain = 0.5 }

[wiring]
connections = [
  { from = "clock.tick", to = "seq.tick" },
  { from = "seq.hz", to = "osc.freq" },
  { from = "osc.out", to = "out.l" },
  { from = "osc.out", to = "out.r" },
]
"#;

const MELODY: &str = r#"schema_version = 1
kind = "notes"
length = 4
notes = [
  { key = 60, start = 0, length = 1, velocity = 1 },
  { key = 64, start = 2, length = 1, velocity = 0.5 },
]
"#;

const STEPS: &str =
    "schema_version = 1\nkind = \"steps\"\nsteps = 4\nstep = 0.25\n\n[lanes]\nkick = \"x...\"\n";

/// The fixture project, copied into the rig's folder.
fn fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "signal-ui-pattern-fixture-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("patterns")).unwrap();
    fs::write(dir.join("project.toml"), PROJECT).unwrap();
    fs::write(dir.join("patterns/melody.toml"), MELODY).unwrap();
    dir
}

fn melody(name: &str, transport: Transport) -> Rig {
    rig(
        name,
        Options {
            transport,
            project: Some(fixture(&format!("{name}-{transport:?}"))),
            ..Options::default()
        },
    )
}

fn read(rig: &mut Rig, ws: &mut Ws, id: u64, name: &str) -> Value {
    rig.request(
        ws,
        id,
        json!({"type": "pattern.read", "name": name}),
        "pattern.read",
    )
}

fn write(name: &str, text: &str, base: &Value) -> Value {
    json!({"type": "pattern.write", "name": name, "text": text, "base": base})
}

fn refused(rig: &mut Rig, ws: &mut Ws, id: u64, message: Value) -> Value {
    rig.request(ws, id, message, "error")
}

/// A read gives the text, its base and kind; a write replaces the file
/// atomically, every other client is pushed it, and the watch applies it
/// live: a swap that keeps every instance.
fn a_write_is_pushed_and_applied_live(transport: Transport) {
    let mut rig = melody("pattern-write", transport);
    let (mut ws, _) = rig.client();
    let (mut other, _) = rig.client();
    let file = rig.dir.join("patterns/melody.toml");
    let got = read(&mut rig, &mut ws, 1, "melody");
    assert_eq!(got["exists"], true);
    assert_eq!(got["kind"], "notes");
    assert_eq!(got["text"], MELODY);
    assert_eq!(got["base"], sha256_hex(MELODY.as_bytes()));

    let edited = MELODY.replace("key = 64", "key = 67");
    let written = rig.request(
        &mut ws,
        2,
        write("melody", &edited, &got["base"]),
        "pattern.written",
    );
    assert_eq!(written["created"], false);
    assert_eq!(written["file"], "patterns/melody.toml");
    assert_eq!(written["base"], sha256_hex(edited.as_bytes()));
    assert_eq!(fs::read_to_string(&file).unwrap(), edited);
    let leftovers: Vec<_> = fs::read_dir(rig.dir.join("patterns"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name() != "melody.toml")
        .collect();
    assert!(leftovers.is_empty(), "atomic: no temporary file left");

    let pushed = rig.wait(&mut other, "pattern.written", |got| {
        got.of("pattern.written").cloned()
    });
    assert_eq!(pushed["name"], "melody");
    assert_eq!(pushed["base"], written["base"]);
    assert_eq!(pushed["text"], edited);
    // The watch applies it live: a swap, nothing removed.
    let reload = rig.wait(&mut ws, "the reload", |got| got.of("reload").cloned());
    assert_eq!(
        (reload["swap"].clone(), reload["removed"].clone()),
        (json!(1), json!(0))
    );
    rig.finish();
}

/// A file changed since `base` (or present when `base` is null) is a
/// conflict carrying its current text and base; nothing is written.
fn a_changed_file_is_a_conflict(transport: Transport) {
    let mut rig = melody("pattern-conflict", transport);
    let (mut ws, _) = rig.client();
    let file = rig.dir.join("patterns/melody.toml");
    let stale = json!(sha256_hex(b"an older text"));
    let edited = MELODY.replace("key = 64", "key = 67");
    for (id, base) in [(1, stale), (2, Value::Null)] {
        let error = refused(&mut rig, &mut ws, id, write("melody", &edited, &base));
        assert_eq!(error["code"], "conflict", "{error}");
        assert_eq!(error["base"], sha256_hex(MELODY.as_bytes()));
        assert_eq!(error["text"], MELODY);
        assert_eq!(fs::read_to_string(&file).unwrap(), MELODY);
    }
    rig.finish();
}

/// The server parses and validates before writing: text the project would
/// not load with is refused naming the problem (file, line, column), and
/// so is a pattern of the wrong kind for the instance naming it.
fn a_pattern_that_does_not_load_is_not_written(transport: Transport) {
    let mut rig = melody("pattern-refused", transport);
    let (mut ws, _) = rig.client();
    let file = rig.dir.join("patterns/melody.toml");
    let base = json!(sha256_hex(MELODY.as_bytes()));
    for (id, text, named) in [
        (
            1,
            MELODY.replace("length = 4", "length = 4\ntempo = 3"),
            "patterns/melody.toml:4:1: error: unknown field `tempo`",
        ),
        (
            2,
            MELODY.replace("velocity = 0.5", "velocity = 2"),
            "patterns/melody.toml:6:49: error: velocity must be from 0 to 1",
        ),
        (3, "kind = [".to_owned(), "patterns/melody.toml:1:"),
        (
            4,
            STEPS.to_owned(),
            "seq: `std.note_seq` takes a `notes` pattern; patterns/melody.toml is `steps`",
        ),
    ] {
        let error = refused(&mut rig, &mut ws, id, write("melody", &text, &base));
        assert_eq!(error["code"], "does-not-load", "{error}");
        let message = error["message"].as_str().unwrap();
        assert!(message.contains(named), "{named}: {message}");
        assert!(!error["diagnostics"].as_array().unwrap().is_empty());
        assert_eq!(fs::read_to_string(&file).unwrap(), MELODY);
    }
    rig.finish();
}

/// Names, not paths: only `patterns/<name>.toml` of the project served.
/// A linked `patterns/` or file is never read or written; a missing file
/// reads as absent and its first write (base null) creates it.
fn names_links_and_a_first_write(transport: Transport) {
    let mut rig = melody("pattern-names", transport);
    let (mut ws, _) = rig.client();
    for (id, name) in [(1, "../project"), (2, "Melody"), (3, "a/b"), (4, "")] {
        let error = refused(&mut rig, &mut ws, id, write(name, STEPS, &Value::Null));
        assert_eq!(error["code"], "bad-value", "{name}: {error}");
    }
    let missing = read(&mut rig, &mut ws, 5, "beat");
    assert_eq!(missing["exists"], false);
    assert_eq!(missing["base"], Value::Null);
    let created = rig.request(
        &mut ws,
        6,
        write("beat", STEPS, &Value::Null),
        "pattern.written",
    );
    assert_eq!(created["created"], true);
    assert_eq!(
        fs::read_to_string(rig.dir.join("patterns/beat.toml")).unwrap(),
        STEPS
    );
    assert_eq!(read(&mut rig, &mut ws, 7, "beat")["kind"], "steps");

    // A linked patterns/: neither read nor written; the file outside stays.
    let outside = rig.dir.parent().unwrap().join("outside-patterns");
    fs::rename(rig.dir.join("patterns"), &outside).unwrap();
    std::os::unix::fs::symlink(&outside, rig.dir.join("patterns")).unwrap();
    let base = json!(sha256_hex(MELODY.as_bytes()));
    let error = refused(&mut rig, &mut ws, 8, write("melody", MELODY, &base));
    assert_eq!(error["code"], "write-failed", "{error}");
    assert!(error["message"].as_str().unwrap().contains("link"));
    let error = refused(
        &mut rig,
        &mut ws,
        9,
        json!({"type": "pattern.read", "name": "melody"}),
    );
    assert_eq!(error["code"], "unknown-pattern", "{error}");
    assert_eq!(
        fs::read_to_string(outside.join("melody.toml")).unwrap(),
        MELODY
    );
    // A linked file in a real patterns/.
    fs::remove_file(rig.dir.join("patterns")).unwrap();
    fs::create_dir(rig.dir.join("patterns")).unwrap();
    fs::copy(
        outside.join("melody.toml"),
        rig.dir.join("patterns/melody.toml"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        outside.join("beat.toml"),
        rig.dir.join("patterns/beat.toml"),
    )
    .unwrap();
    let base = json!(sha256_hex(STEPS.as_bytes()));
    let error = refused(&mut rig, &mut ws, 10, write("beat", STEPS, &base));
    assert_eq!(error["code"], "write-failed", "{error}");
    rig.finish();
}

/// A message over the 64 KiB limit closes the client's connection (as every
/// message does); the file is not written.
fn a_write_over_the_message_limit_closes_the_connection(transport: Transport) {
    let mut rig = melody("pattern-limit", transport);
    let (mut ws, _) = rig.client();
    let file = rig.dir.join("patterns/melody.toml");
    let base = json!(sha256_hex(MELODY.as_bytes()));
    let padding = format!("# {}\n", "x".repeat(70 * 1024));
    let mut message = write("melody", &format!("{MELODY}{padding}"), &base);
    message["id"] = json!(1);
    assert!(message.to_string().len() > 64 * 1024);
    send(&mut ws, message);
    let mut closed = false;
    for _ in 0..500 {
        rig.pump();
        match receive(&mut ws) {
            Some(Got::Closed) => {
                closed = true;
                break;
            }
            Some(got) => assert!(
                got.of("pattern.written").is_none(),
                "answered a message over the limit"
            ),
            None => {}
        }
    }
    assert!(closed, "the connection is closed");
    assert_eq!(fs::read_to_string(&file).unwrap(), MELODY);
    // A write that fits is taken.
    let (mut ws, _) = rig.client();
    let fits = format!("{MELODY}# {}\n", "x".repeat(60 * 1024));
    rig.request(&mut ws, 2, write("melody", &fits, &base), "pattern.written");
    assert_eq!(fs::read_to_string(&file).unwrap(), fits);
    rig.finish();
}

/// auditor-1-006 (AC10): a link at the write's temporary path
/// (`patterns/.<name>.toml.tmp`) is not followed: the file it points to
/// outside the project is untouched, and the pattern written is a regular
/// file holding the text, for a new file and for an existing one.
fn a_link_at_the_temporary_path_is_not_followed(transport: Transport) {
    let mut rig = melody("pattern-temp-link", transport);
    let (mut ws, _) = rig.client();
    let outside = rig.dir.parent().unwrap().join("sentinel.txt");
    fs::write(&outside, "outside the project").unwrap();
    for (id, name, before, text) in [
        (1, "beat", Value::Null, STEPS.to_owned()),
        (
            2,
            "melody",
            json!(sha256_hex(MELODY.as_bytes())),
            MELODY.replace("key = 64", "key = 67"),
        ),
    ] {
        let temporary = rig.dir.join(format!("patterns/.{name}.toml.tmp"));
        std::os::unix::fs::symlink(&outside, &temporary).unwrap();
        let written = rig.request(&mut ws, id, write(name, &text, &before), "pattern.written");
        assert_eq!(written["base"], sha256_hex(text.as_bytes()));
        assert_eq!(fs::read_to_string(&outside).unwrap(), "outside the project");
        let path = rig.dir.join(format!("patterns/{name}.toml"));
        let kind = fs::symlink_metadata(&path).unwrap().file_type();
        assert!(
            kind.is_file() && !kind.is_symlink(),
            "{name}: a regular file"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        assert!(
            fs::symlink_metadata(&temporary).is_err(),
            "no temporary left"
        );
    }
    rig.finish();
}

/// A panel holding the pattern controls (`<signal-piano-roll>` on a
/// `notes` pattern, its playhead bound to `/seq.position`;
/// `<signal-step-grid>` on a `steps` one; no `bind`) is written by
/// `panel.write` and passes `panel.check`; a missing pattern, a wrong kind,
/// no `pattern`, a `bind`, or a playhead naming no signal output is refused
/// at its line.
fn a_panel_with_pattern_controls(transport: Transport) {
    let mut rig = melody("pattern-panel", transport);
    fs::write(rig.dir.join("patterns/beat.toml"), STEPS).unwrap();
    let (mut ws, _) = rig.client();
    // The watch loads the new pattern file (a project file) into the panel.
    rig.wait(&mut ws, "the reload", |got| got.of("reload").cloned());
    let html = "\n  <div class=\"signal-cards\">\n    <section class=\"signal-card\">\n      <h2>patterns</h2>\n      <div class=\"row\">\n        <signal-piano-roll pattern=\"melody\" label=\"melody\" snap=\"1/16\" beats-per-bar=\"4\" position-bind=\"/seq.position\"></signal-piano-roll>\n        <signal-step-grid pattern=\"beat\" label=\"drums\"></signal-step-grid>\n      </div>\n    </section>\n  </div>\n";
    let written = rig.request(
        &mut ws,
        1,
        json!({"type": "panel.write", "file": "main.html", "html": html, "base": null}),
        "panel.written",
    );
    assert_eq!(written["created"], true, "{written}");
    let check = rig.request(
        &mut ws,
        2,
        json!({"type": "panel.check", "file": "main.html"}),
        "panel.check",
    );
    assert_eq!(check["ok"], true, "{check}");
    assert_eq!(check["controls"], 2);
    assert_eq!(check["bindings"], 1, "the playhead is a binding: {check}");
    let file = rig.dir.join("ui/main.html");
    let mut base = json!(sha256_hex(fs::read_to_string(&file).unwrap().as_bytes()));
    for (id, (from, to, why)) in [
        (
            "pattern=\"melody\" label",
            "pattern=\"beat\" label",
            "wrong kind: patterns/beat.toml is a `steps` pattern; <signal-piano-roll> edits a `notes` pattern",
        ),
        (
            "pattern=\"beat\"",
            "pattern=\"drums\"",
            "unknown pattern `drums`: the project has no patterns/drums.toml; <signal-step-grid> edits a `steps` pattern",
        ),
        (
            "position-bind=\"/seq.position\"",
            "position-bind=\"/osc.freq\"",
            "wrong kind: /osc.freq is a parameter; <signal-piano-roll> position-bind takes a signal output of the plan (its playhead is read, never written)",
        ),
        (
            "position-bind=\"/seq.position\"",
            "position-bind=\"/out.l\"",
            "wrong kind: /out.l is a signal input of the audio sink; <signal-piano-roll> position-bind takes a signal output of the plan (its playhead is read, never written)",
        ),
        (
            "<signal-step-grid pattern=\"beat\"",
            "<signal-step-grid",
            "<signal-step-grid> has no pattern: it needs pattern=\"<name>\", a `steps` pattern of the project (patterns/<name>.toml)",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let before = fs::read_to_string(&file).unwrap();
        let error = rig.request(
            &mut ws,
            10 + id as u64,
            json!({"type": "panel.write", "file": "main.html", "html": html.replace(from, to), "base": base}),
            "error",
        );
        assert_eq!(error["code"], "bad-binding", "{error}");
        assert_eq!(error["diagnostics"][0]["message"], why, "{error}");
        // At the panel line of the control (the file's first lines are the
        // generated head; the controls are on lines 5 and 6 of the html).
        let line = error["diagnostics"][0]["line"].as_u64().unwrap();
        let expected = before[..before.find("<signal-panel>").unwrap()]
            .matches('\n')
            .count() as u64
            + if why.contains("piano") { 6 } else { 7 };
        assert_eq!(line, expected, "{error}");
        assert_eq!(fs::read_to_string(&file).unwrap(), before);
        base = json!(sha256_hex(before.as_bytes()));
    }
    rig.finish();
}

both!(
    a_panel_with_pattern_controls,
    a_link_at_the_temporary_path_is_not_followed,
    a_write_is_pushed_and_applied_live,
    a_changed_file_is_a_conflict,
    a_pattern_that_does_not_load_is_not_written,
    names_links_and_a_first_write,
    a_write_over_the_message_limit_closes_the_connection,
);
