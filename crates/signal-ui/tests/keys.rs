//! ADR 0034: the keyboard shortcuts. The built-in keymap is whole; chords
//! and overrides read as `ui/lib/keys.js` reads them (the shared cases in
//! `ui/lib/fixtures/keys-cases.json`); what is wrong with the person's
//! `keys.toml` is reported with its place, a file that does not read falls
//! back to the built-in keys; writing keeps comments and other entries and
//! holds only overrides; and over the protocol `keys.get`, `keys.set` and
//! `keys.reset` work, every client is pushed the change, and an edit made
//! elsewhere applies live.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{Options, Transport, rig};
use serde_json::{Value, json};
use signal_ui::keys::{
    self, Keys, defaults, keys_path, parse_chord, with_override, without_overrides,
};

fn cases() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/lib/fixtures/keys-cases.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn file() -> PathBuf {
    PathBuf::from("/prefs/keys.toml")
}

#[test]
fn chords_read_as_the_page_reads_them() {
    for case in cases()["chords"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        let parsed = parse_chord(text);
        if case["error"] == true {
            assert!(parsed.is_err(), "{text} should be refused: {parsed:?}");
        } else {
            assert_eq!(
                parsed.as_deref(),
                Ok(case["chord"].as_str().unwrap()),
                "{text}"
            );
        }
    }
}

/// The overrides as a file, each a quoted id at the top.
fn as_toml(overrides: &Value) -> String {
    let mut text = String::from("version = 1\n");
    for (id, chords) in overrides.as_object().unwrap() {
        text.push_str(&format!("\"{id}\" = {chords}\n"));
    }
    text
}

#[test]
fn overrides_give_the_conflicts_and_problems_the_page_finds() {
    for case in cases()["overrides"].as_array().unwrap() {
        let about = case["about"].as_str().unwrap();
        let keys = Keys::parse(&as_toml(&case["overrides"]), &file());
        assert_eq!(
            keys.problems.len() as u64,
            case["problems"].as_u64().unwrap(),
            "{about}: {:?}",
            keys.problems
        );
        let got: Vec<Value> = keys
            .conflicts()
            .iter()
            .map(
                |c| json!({"kind": c.kind, "chord": c.chord, "command": c.command, "with": c.with}),
            )
            .collect();
        assert_eq!(Value::Array(got), case["conflicts"], "{about}");
        assert!(!keys.fallback, "{about}");
    }
}

#[test]
fn the_built_in_keymap_is_whole() {
    // The module's object reads (a broken one would be an empty keymap).
    let json = keys::defaults_json(keys::DEFAULTS_MODULE).expect("the object in keys.default.js");
    let parsed: Value = serde_json::from_str(json).expect("keys.default.js holds JSON");
    assert!(parsed["commands"].as_array().is_some_and(|c| !c.is_empty()));
    assert!(
        keys::defaults_json("// export default {x}\nexport default {\"a\": 1};\n")
            == Some("{\"a\": 1}")
    );
    let defaults = defaults();
    assert!(defaults.commands.len() > 100);
    for command in &defaults.commands {
        assert!(
            defaults.scopes.iter().any(|s| s.id == command.scope),
            "{}",
            command.id
        );
        for chord in &command.keys {
            assert_eq!(
                parse_chord(chord).as_deref(),
                Ok(chord.as_str()),
                "{}",
                command.id
            );
            assert_eq!(
                keys::is_wheel(chord),
                command.wheel,
                "{}: {chord}",
                command.id
            );
        }
    }
    let none = Keys::built_in(&file());
    assert!(none.conflicts().is_empty());
    let menu: Vec<(String, Option<String>)> = none.menu_chords();
    assert!(menu.contains(&("file.save".to_owned(), Some("Cmd+S".to_owned()))));
    assert!(menu.contains(&("app.settings".to_owned(), Some("Cmd+,".to_owned()))));
    assert!(menu.contains(&("transport.restart".to_owned(), None)));
}

#[test]
fn problems_say_where_they_are() {
    let text = "version = 1\n\n[view]\npage_down = [\"PageDown\", \"Shift+=\"]\nnothing = [\"N\"]\n\n[edit]\nundo = [\"Cmd+U\"]\n\n[window]\nnew_schematic = [\"Shift+Cmd+N\"]\n";
    let keys = Keys::parse(text, &file());
    assert_eq!(
        keys.problems,
        vec![
            "keys.toml:4:1: view.page_down: 'Shift+=': Shift with '=': write the character Shift types, without Shift; ignored",
            "keys.toml:5:1: unknown command 'view.nothing'; ignored",
            "keys.toml:8:1: edit.undo is fixed (the system's); ignored",
        ]
    );
    assert_eq!(keys.overrides["view.page_down"], vec!["PageDown"]);
    let report = keys.report();
    assert_eq!(
        report.last().unwrap(),
        "keys.toml:11:1: Shift+Cmd+N is both Signal.app ▸ New Main Window and New Schematic Window: New Main Window keeps it"
    );
    let message = keys.message();
    assert_eq!(message["type"], "keys");
    assert_eq!(message["problems"].as_array().unwrap().len(), 4);
    assert_eq!(message["conflicts"][0]["kind"], "same");
    assert_eq!(
        message["overrides"]["window.new_schematic"],
        json!(["Shift+Cmd+N"])
    );
    assert_eq!(keys.keys_of("window.new_schematic"), vec!["Shift+Cmd+N"]);
    assert_eq!(keys.keys_of("window.new_main"), vec!["Shift+Cmd+N"]);
}

#[test]
fn a_file_that_does_not_read_falls_back_to_the_built_in_keys() {
    let keys = Keys::parse("[view\npage_down = 1\n", &file());
    assert!(keys.fallback);
    assert!(keys.overrides.is_empty());
    assert!(
        keys.problems[0].starts_with("keys.toml:1:"),
        "{:?}",
        keys.problems
    );
    assert!(keys.problems[0].ends_with("using the built-in keys"));
    let keys = Keys::parse("version = 2\n[view]\npage_down = []\n", &file());
    assert!(keys.fallback);
    assert_eq!(
        keys.problems,
        vec!["keys.toml:1:11: version 2 is not 1; using the built-in keys"]
    );
    let keys = Keys::parse("view = 3\n", &file());
    assert!(!keys.fallback);
    assert!(keys.problems[0].starts_with("keys.toml:1:1: 'view' is not a group"));
    // Not a list, not a string.
    let keys = Keys::parse("[view]\npage_down = \"PageDown\"\npage_up = [1]\n", &file());
    assert_eq!(keys.problems.len(), 2, "{:?}", keys.problems);
    assert!(keys.overrides.contains_key("view.page_up"));
    assert!(!keys.overrides.contains_key("view.page_down"));
}

#[test]
fn writing_keeps_comments_and_holds_only_overrides() {
    let text = "# mine\nversion = 1\n\n# the page\n[view]\n# Space too\npage_down = [\"PageDown\", \"Space\", \"F20\"]\n";
    let next = with_override(text, "schematic.fit", Some(&["Cmd+0".to_owned()])).unwrap();
    assert!(
        next.starts_with("# mine\nversion = 1\n\n# the page\n[view]\n# Space too\n"),
        "{next}"
    );
    assert!(next.contains("[schematic]\nfit = [\"Cmd+0\"]\n"), "{next}");
    // Equal to the built-in keys: the override goes, and its empty group.
    let next = with_override(&next, "schematic.fit", Some(&["home".to_owned()])).unwrap();
    assert!(!next.contains("schematic"), "{next}");
    // Back to the built-in keys (None), canonical chords written.
    let next = with_override(&next, "view.page_down", None).unwrap();
    assert!(!next.contains("page_down"), "{next}");
    assert!(next.starts_with("# mine\nversion = 1\n"), "{next}");
    let next = with_override(
        "",
        "view.page_up",
        Some(&["pgup".to_owned(), "f19".to_owned()]),
    )
    .unwrap();
    assert!(next.starts_with("# Signal's keyboard shortcuts"), "{next}");
    assert!(
        next.contains("[view]\npage_up = [\"PageUp\", \"F19\"]\n"),
        "{next}"
    );
    let parsed = Keys::parse(&next, &file());
    assert!(parsed.problems.is_empty());
    assert_eq!(parsed.overrides["view.page_up"], vec!["PageUp", "F19"]);
    // An unbound command is an empty list.
    let next = with_override(&next, "view.line_up", Some(&[])).unwrap();
    assert!(next.contains("line_up = []"), "{next}");
    // A quoted id is edited where it is.
    let quoted = with_override(
        "version = 1\n\"view.top\" = [\"T\"] # here\n",
        "view.top",
        Some(&["Y".to_owned()]),
    )
    .unwrap();
    assert!(quoted.contains("\"view.top\" = [\"Y\"]"), "{quoted}");
    assert!(!quoted.contains("[view]"), "{quoted}");
    // Refused: unknown, fixed, a bad chord, the wrong kind, a broken file.
    assert!(with_override("", "view.nothing", Some(&[])).is_err());
    assert!(with_override("", "edit.undo", Some(&["Cmd+U".to_owned()])).is_err());
    assert!(with_override("", "view.top", Some(&["Shift+=".to_owned()])).is_err());
    assert!(with_override("", "view.top", Some(&["Wheel".to_owned()])).is_err());
    assert!(with_override("", "view.wheel_inside", Some(&["Wheel".to_owned()])).is_err());
    assert!(with_override("[view\n", "view.top", Some(&[])).is_err());
    assert!(
        with_override("version = 2\n", "view.top", Some(&[])).is_err(),
        "a file the server ignores is not written"
    );
    assert!(without_overrides("version = 2\n").is_err());
    // Reset all keeps the comments outside the groups.
    let reset = without_overrides(text).unwrap();
    assert_eq!(reset, "# mine\nversion = 1\n");
}

#[test]
fn the_file_is_beside_the_setup_and_written_atomically() {
    let base = std::env::temp_dir().join(format!("signal-keys-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let setup = base.join("Signal/setup.toml");
    let path = keys_path(&setup);
    assert_eq!(path, base.join("Signal/keys.toml"));
    assert_eq!(
        Keys::load(&path),
        Keys::built_in(&path),
        "no file: the built-in keys"
    );
    keys::write_override(&path, "view.top", Some(&["Cmd+Home".to_owned()])).unwrap();
    assert!(!base.join("Signal/keys.toml.tmp").exists());
    assert_eq!(Keys::load(&path).overrides["view.top"], vec!["Cmd+Home"]);
    keys::reset(&path).unwrap();
    assert!(Keys::load(&path).overrides.is_empty());
    // A linked keys.toml is written where the link points; the link stays.
    let kept = base.join("dotfiles/keys.toml");
    fs::create_dir_all(kept.parent().unwrap()).unwrap();
    fs::write(&kept, "# mine\nversion = 1\n").unwrap();
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&kept, &path).unwrap();
    keys::write_override(&path, "view.top", Some(&["F9".to_owned()])).unwrap();
    assert!(
        fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        fs::read_to_string(&kept)
            .unwrap()
            .contains("top = [\"F9\"]")
    );
    fs::remove_dir_all(&base).unwrap();
}

fn keys_over_the_protocol(transport: Transport) {
    let mut rig = rig(
        "keys",
        Options {
            transport,
            ..Options::default()
        },
    );
    let path = keys_path(&rig.setup);
    let (mut ws, _) = rig.client();
    let (mut other, _) = rig.client();
    let got = rig.request(&mut ws, 1, json!({"type": "keys.get"}), "keys");
    assert_eq!(got["path"], path.display().to_string());
    assert_eq!(got["overrides"], json!({}));
    assert_eq!(got["fallback"], false);
    let version = rig.host().keys_version();

    let set = rig.request(
        &mut ws,
        2,
        json!({"type": "keys.set", "command": "view.page_down", "keys": ["pagedown", "f20"]}),
        "keys",
    );
    assert_eq!(
        set["overrides"]["view.page_down"],
        json!(["PageDown", "F20"])
    );
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .contains("page_down = [\"PageDown\", \"F20\"]")
    );
    assert!(rig.host().keys_version() > version);
    // The other client is pushed the change.
    let pushed = rig.wait(&mut other, "keys", |got| {
        got.of("keys").filter(|v| v.get("id").is_none()).cloned()
    });
    assert_eq!(
        pushed["overrides"]["view.page_down"],
        json!(["PageDown", "F20"])
    );

    let refused = rig.request(
        &mut ws,
        3,
        json!({"type": "keys.set", "command": "edit.undo", "keys": ["Cmd+U"]}),
        "error",
    );
    assert_eq!(refused["code"], "bad-value");
    let refused = rig.request(
        &mut ws,
        4,
        json!({"type": "keys.set", "command": "view.top", "keys": "Home"}),
        "error",
    );
    assert_eq!(refused["code"], "bad-message");

    // An edit made elsewhere applies live, with what is wrong in it.
    let text = fs::read_to_string(&path).unwrap() + "nothing = [\"N\"]\n";
    // A different length: the stamp changes even within the same second.
    fs::write(&path, text).unwrap();
    let pushed = rig.wait(&mut other, "the outside edit", |got| {
        got.of("keys")
            .filter(|v| v["problems"].as_array().is_some_and(|p| !p.is_empty()))
            .cloned()
    });
    assert!(
        pushed["problems"][0]
            .as_str()
            .unwrap()
            .contains("unknown command 'view.nothing'")
    );
    assert_eq!(
        pushed["overrides"]["view.page_down"],
        json!(["PageDown", "F20"])
    );

    let reset = rig.request(
        &mut ws,
        5,
        json!({"type": "keys.reset", "command": "view.page_down"}),
        "keys",
    );
    assert!(
        reset["overrides"].get("view.page_down").is_none(),
        "{reset}"
    );
    let reset = rig.request(&mut ws, 6, json!({"type": "keys.reset"}), "keys");
    assert_eq!(reset["overrides"], json!({}));
    assert_eq!(reset["problems"], json!([]));
    rig.finish();
}

both!(keys_over_the_protocol);

#[test]
fn a_wheel_modifier_of_two_keys_hides_only_chords_holding_both() {
    let keys = Keys::parse("[view]\nwheel_inside = [\"Ctrl+Alt+Wheel\"]\n", &file());
    assert!(keys.problems.is_empty(), "{:?}", keys.problems);
    assert!(
        keys.conflicts().is_empty(),
        "Ctrl+Wheel (a pinch) is not Ctrl+Alt"
    );
    let keys = Keys::parse(
        "[view]\nwheel_inside = [\"Ctrl+Alt+Wheel\"]\n[piano_roll]\nzoom_keys = [\"Ctrl+Alt+Shift+Wheel\"]\n",
        &file(),
    );
    assert_eq!(keys.conflicts().len(), 1);
    assert_eq!(keys.conflicts()[0].kind, "wheel");
}
