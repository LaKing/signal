//! M2.12-1 (ADR 0029 §2, operator decision of 2026-10-18,
//! `docs/ui-protocol.md` "`file.read`, `file.write`, `vendor`"): the
//! project's own text files read and written whole over both transports,
//! with the rules of `patch` and `pattern.write`: a write applied live by the
//! watch; a conflict carrying the current text; text the project would not
//! load with, refused; only `project.toml` and `composites/<name>.toml`, never
//! through a link; and `vendor`, a library composite copied in by `signal
//! vendor`'s writer after the same checks. On the tone project; no committed
//! project is written.

mod common;

use std::fs;

use common::{Options, Rig, Transport, Ws, rig};
use serde_json::{Value, json};
use signal_ui::panel_write::sha256_hex;

fn tone(name: &str, transport: Transport) -> Rig {
    rig(
        name,
        Options {
            transport,
            ..Options::default()
        },
    )
}

fn read(rig: &mut Rig, ws: &mut Ws, id: u64, file: &str) -> Value {
    rig.request(
        ws,
        id,
        json!({"type": "file.read", "file": file}),
        "file.read",
    )
}

fn write(file: &str, text: &str, base: &Value) -> Value {
    json!({"type": "file.write", "file": file, "text": text, "base": base})
}

/// A read gives the text and its base; a write replaces the file atomically
/// and the watch applies it live (a reload); a new composite is created.
fn a_write_is_applied_live(transport: Transport) {
    let mut rig = tone("file-write", transport);
    let (mut ws, _) = rig.client();
    let path = rig.dir.join("project.toml");
    let before = fs::read_to_string(&path).unwrap();
    let got = read(&mut rig, &mut ws, 1, "project.toml");
    assert_eq!(got["exists"], true);
    assert_eq!(got["text"], before);
    assert_eq!(got["base"], sha256_hex(before.as_bytes()));

    let edited = before.replace("freq = 440", "freq = 330");
    let written = rig.request(
        &mut ws,
        2,
        write("project.toml", &edited, &got["base"]),
        "file.written",
    );
    assert_eq!(written["created"], false);
    assert_eq!(written["base"], sha256_hex(edited.as_bytes()));
    assert_eq!(fs::read_to_string(&path).unwrap(), edited);
    let leftovers: Vec<_> = fs::read_dir(&rig.dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
        .collect();
    assert!(
        leftovers.is_empty(),
        "no temporary file left: {leftovers:?}"
    );
    rig.wait(&mut ws, "the reload", |got| got.of("reload").cloned());

    let missing = read(&mut rig, &mut ws, 3, "composites/voice.toml");
    assert_eq!(missing["exists"], false);
    let voice = "schema_version = 1\n\n[composite]\nname = \"voice\"\n\n[instances]\nosc = { module = \"std.sine_osc\" }\n\n[exports]\noutputs = { out = \"osc.out\" }\n";
    let created = rig.request(
        &mut ws,
        4,
        write("composites/voice.toml", voice, &Value::Null),
        "file.written",
    );
    assert_eq!(created["created"], true);
    assert_eq!(
        fs::read_to_string(rig.dir.join("composites/voice.toml")).unwrap(),
        voice
    );
    rig.finish();
}

/// A stale base is a conflict carrying the current text; text the project
/// would not load with is refused naming the problem; another file, or one
/// reached through a link, is refused. Nothing is written.
fn refusals_write_nothing(transport: Transport) {
    let mut rig = tone("file-refused", transport);
    let (mut ws, _) = rig.client();
    let path = rig.dir.join("project.toml");
    let before = fs::read_to_string(&path).unwrap();
    let stale = json!("0".repeat(64));
    let conflict = rig.request(&mut ws, 1, write("project.toml", &before, &stale), "error");
    assert_eq!(conflict["code"], "conflict");
    assert_eq!(conflict["text"], before);
    assert_eq!(conflict["base"], sha256_hex(before.as_bytes()));

    let base = json!(sha256_hex(before.as_bytes()));
    let broken = before.replace("std.sine_osc", "std.no_such_type");
    let refused = rig.request(&mut ws, 2, write("project.toml", &broken, &base), "error");
    assert_eq!(refused["code"], "does-not-load");
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains("std.no_such_type"),
        "{refused}"
    );
    for (id, file) in [
        (3, "ui/schematic.toml"),
        (4, "patterns/melody.toml"),
        (5, "../project.toml"),
        (6, "composites/Bad.toml"),
    ] {
        let refused = rig.request(&mut ws, id, write(file, "x", &Value::Null), "error");
        assert_eq!(refused["code"], "bad-value", "{file}");
    }
    let outside = rig.dir.parent().unwrap().join("outside");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, rig.dir.join("composites")).unwrap();
    let linked = rig.request(
        &mut ws,
        7,
        write("composites/voice.toml", "x", &Value::Null),
        "error",
    );
    assert_eq!(linked["code"], "write-failed");
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    rig.finish();
}

/// `vendor` copies a library composite as `signal vendor` does, after the
/// host's base check; a stale base and an unknown composite write nothing.
fn vendor_copies_a_library_composite(transport: Transport) {
    let mut rig = tone("file-vendor", transport);
    let (mut ws, _) = rig.client();
    let before = fs::read_to_string(rig.dir.join("project.toml")).unwrap();
    let base = json!(sha256_hex(before.as_bytes()));
    let stale = rig.request(
        &mut ws,
        1,
        json!({"type": "vendor", "composite": "lib.tape_echo", "base": "0".repeat(64)}),
        "error",
    );
    assert_eq!(stale["code"], "conflict");
    let unknown = rig.request(
        &mut ws,
        2,
        json!({"type": "vendor", "composite": "lib.no_such", "base": base}),
        "error",
    );
    assert_eq!(unknown["code"], "unknown-module");
    assert!(!rig.dir.join("composites").exists());
    let vendored = rig.request(
        &mut ws,
        3,
        json!({"type": "vendor", "composite": "lib.tape_echo", "as": "echo", "base": base}),
        "vendored",
    );
    let copied = vendored["copied"].as_array().unwrap();
    assert!(
        copied.contains(&json!("composites/echo.toml")),
        "{vendored}"
    );
    assert!(rig.dir.join("composites/echo.toml").is_file());
    assert_eq!(
        fs::read_to_string(rig.dir.join("project.toml")).unwrap(),
        before,
        "nothing referenced lib.tape_echo"
    );
    rig.finish();
}

both!(
    a_write_is_applied_live,
    refusals_write_nothing,
    vendor_copies_a_library_composite,
);
