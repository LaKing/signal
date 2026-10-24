//! ADR 0041 (`docs/ui-protocol.md` "Snapshots"): `snapshot.list`,
//! `snapshot.save`, `snapshot.restore` and `snapshot.delete` over both
//! transports, on the tone project in a folder of its own. A save records
//! every control's live value (an unsaved one included) and is pushed to
//! every client without a reload; a restore after changes writes the
//! snapshot's values back into project.toml as one write and live, so
//! nothing is unsaved; entries that are gone are skipped and reported, a
//! value out of range clamped and reported; a name in use is refused unless
//! overwritten; a stale `base` is a conflict; delete removes one.
//! `signal_ui::snapshot::restore_files` (the files alone) is checked too.

mod common;

use std::fs;

use std::time::{Duration, Instant};

use common::{Got, Options, ReadOnly, Rig, Transport, Ws, receive, rig};
use serde_json::{Value, json};
use signal_core::Registry;
use signal_project::snapshot::{self, SNAPSHOTS_FILE};
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

fn control(describe: &Value, address: &str) -> Value {
    describe["controls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["address"] == address)
        .unwrap_or_else(|| panic!("no control {address}: {describe}"))
        .clone()
}

fn describe(rig: &mut Rig, ws: &mut Ws, id: u64) -> Value {
    rig.request(ws, id, json!({"type": "describe"}), "describe")
}

fn commit(rig: &mut Rig, ws: &mut Ws, id: u64, address: &str, value: f64) {
    rig.request(
        ws,
        id,
        json!({"type": "commit", "address": address, "value": value}),
        "committed",
    );
}

/// Save, change (a commit, an unsaved live edit), restore: the file and the
/// live values are the snapshot's again, in one write, and nothing is
/// unsaved; the save is pushed to every client and reloads nothing.
fn a_restore_undoes_later_changes(transport: Transport) {
    let mut rig = tone("snapshot-round-trip", transport);
    let (mut a, _) = rig.client();
    let (mut b, _) = rig.client();
    let path = rig.dir.join("project.toml");
    let original = fs::read_to_string(&path).unwrap();

    let empty = rig.request(&mut a, 1, json!({"type": "snapshot.list"}), "snapshots");
    assert_eq!(empty["snapshots"], json!([]));
    assert_eq!(empty["base"], Value::Null);

    // An unsaved live value is what plays, so it is what is saved.
    rig.request(
        &mut a,
        2,
        json!({"type": "set", "address": "/b.freq", "value": 110}),
        "ack",
    );
    let saved = rig.request(
        &mut a,
        3,
        json!({"type": "snapshot.save", "name": "  Első verze  "}),
        "snapshot.saved",
    );
    assert_eq!(saved["name"], "Első verze");
    assert_eq!(saved["replaced"], false);
    let pushed = rig.wait(&mut b, "the pushed list", |got| {
        got.of("snapshots").cloned()
    });
    assert_eq!(pushed["snapshots"][0]["name"], "Első verze");
    assert_eq!(pushed["base"], saved["base"]);
    let values = &pushed["snapshots"][0]["values"];
    assert_eq!(values["/a.freq"], 440.0, "{values}");
    assert_eq!(values["/b.freq"], 110.0, "the live value");
    assert_eq!(values["/out.gain"], 0.5);
    assert_eq!(values["/env.decay_ms"], 50.0);
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        original,
        "a save writes no project file"
    );
    rig.no_reload(&mut a, 300);

    // Changes after it: two commits, an unsaved edit back on /b.freq.
    commit(&mut rig, &mut a, 4, "/a.freq", 330.0);
    commit(&mut rig, &mut a, 5, "/out.gain", 0.25);
    rig.request(
        &mut a,
        6,
        json!({"type": "set", "address": "/b.freq", "value": 220}),
        "ack",
    );
    let writes = rig.request(&mut a, 7, json!({"type": "stats"}), "stats")["writes"]
        .as_u64()
        .unwrap();
    let restored = rig.request(
        &mut a,
        8,
        json!({"type": "snapshot.restore", "name": "Első verze", "base": saved["base"]}),
        "snapshot.restored",
    );
    assert_eq!(restored["restored"], saved["count"], "{restored}");
    assert_eq!(
        restored["written"],
        json!(["/a.freq", "/b.freq", "/out.gain"]),
        "{restored}"
    );
    assert_eq!(restored["sent"], true);
    assert_eq!(restored["skipped"], json!([]));
    let after = rig.request(&mut a, 9, json!({"type": "stats"}), "stats")["writes"]
        .as_u64()
        .unwrap();
    assert_eq!(after, writes + 1, "one write");
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("a = { module = \"std.sine_osc\", freq = 440 }"),
        "{text}"
    );
    assert!(
        text.contains("b = { module = \"std.sine_osc\", freq = 110 }"),
        "{text}"
    );
    assert!(text.contains("gain = 0.5"), "{text}");
    let now = describe(&mut rig, &mut a, 10);
    for (address, value) in [("/a.freq", 440.0), ("/b.freq", 110.0), ("/out.gain", 0.5)] {
        assert_eq!(control(&now, address)["base"], value, "{address}");
    }
    assert_eq!(now["unsaved"]["count"], 0, "{}", now["unsaved"]);
    rig.finish();
}

/// Entries whose instance or parameter is gone are skipped and reported, a
/// value out of range is clamped as a commit clamps it, and controls the
/// snapshot does not name are left alone.
fn gone_entries_are_skipped_and_out_of_range_clamped(transport: Transport) {
    let mut rig = tone("snapshot-skip", transport);
    let (mut ws, _) = rig.client();
    fs::write(
        rig.dir.join(SNAPSHOTS_FILE),
        "schema_version = 1\n\n[snapshots.old]\n\"/a.freq\" = 1e12\n\"/a.no_such\" = 1\n\
         \"/gone.freq\" = 3\n\"/out.gain\" = 0.25\n\"/x/inner.freq\" = 1\n",
    )
    .unwrap();
    let max = control(&describe(&mut rig, &mut ws, 1), "/a.freq")["max"]
        .as_f64()
        .unwrap();
    let restored = rig.request(
        &mut ws,
        2,
        json!({"type": "snapshot.restore", "name": "old"}),
        "snapshot.restored",
    );
    let skipped: Vec<&str> = restored["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["address"].as_str().unwrap())
        .collect();
    assert_eq!(
        skipped,
        ["/a.no_such", "/gone.freq", "/x/inner.freq"],
        "{restored}"
    );
    assert!(
        restored["skipped"][1]["reason"]
            .as_str()
            .unwrap()
            .contains("no instance `gone`"),
        "{restored}"
    );
    assert_eq!(
        restored["clamped"],
        json!([{"address": "/a.freq", "value": 1e12, "to": max}])
    );
    let text = fs::read_to_string(rig.dir.join("project.toml")).unwrap();
    assert!(text.contains("gain = 0.25"), "{text}");
    assert!(text.contains("freq = 220"), "/b.freq left alone: {text}");
    let now = describe(&mut rig, &mut ws, 3);
    assert_eq!(control(&now, "/a.freq")["base"], max);
    rig.finish();
}

/// A name in use needs `overwrite`; a stale `base` is a conflict carrying
/// the current one; bad names and unknown snapshots are refused; delete
/// removes one. Nothing is written by a refusal.
fn names_overwrites_conflicts_and_delete(transport: Transport) {
    let mut rig = tone("snapshot-names", transport);
    let (mut ws, _) = rig.client();
    let path = rig.dir.join(SNAPSHOTS_FILE);
    let saved = rig.request(
        &mut ws,
        1,
        json!({"type": "snapshot.save", "name": "Chorus", "base": null}),
        "snapshot.saved",
    );
    let text = fs::read_to_string(&path).unwrap();
    let exists = rig.request(
        &mut ws,
        2,
        json!({"type": "snapshot.save", "name": "Chorus"}),
        "error",
    );
    assert_eq!(exists["code"], "exists");
    let stale = rig.request(
        &mut ws,
        3,
        json!({"type": "snapshot.save", "name": "Chorus", "overwrite": true, "base": "0".repeat(64)}),
        "error",
    );
    assert_eq!(stale["code"], "conflict");
    assert_eq!(stale["base"], saved["base"]);
    for (id, name) in [
        (4, json!("   ")),
        (5, json!("x".repeat(65))),
        (6, json!(3)),
        (60, json!("a\u{200B}b")),
        (61, json!("\u{FEFF}")),
        (62, json!("x\u{2028}y")),
    ] {
        let bad = rig.request(
            &mut ws,
            id,
            json!({"type": "snapshot.save", "name": name}),
            "error",
        );
        assert_eq!(bad["code"], "bad-value", "{name}");
    }
    assert_eq!(fs::read_to_string(&path).unwrap(), text, "nothing written");
    commit(&mut rig, &mut ws, 7, "/a.freq", 330.0);
    let replaced = rig.request(
        &mut ws,
        8,
        json!({"type": "snapshot.save", "name": "Chorus", "overwrite": true, "base": saved["base"]}),
        "snapshot.saved",
    );
    assert_eq!(replaced["replaced"], true);
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .contains("\"/a.freq\" = 330")
    );
    let unknown = rig.request(
        &mut ws,
        9,
        json!({"type": "snapshot.restore", "name": "Verse"}),
        "error",
    );
    assert_eq!(unknown["code"], "unknown-snapshot");
    let deleted = rig.request(
        &mut ws,
        10,
        json!({"type": "snapshot.delete", "name": "Chorus", "base": replaced["base"]}),
        "snapshot.deleted",
    );
    assert_eq!(deleted["name"], "Chorus");
    // The hash of what the call wrote, not of a later read.
    assert_eq!(deleted["base"], sha256_hex(&fs::read(&path).unwrap()));
    assert_eq!(
        replaced["base"].as_str().map(str::len),
        Some(64),
        "{replaced}"
    );
    let list = rig.request(&mut ws, 11, json!({"type": "snapshot.list"}), "snapshots");
    assert_eq!(list["snapshots"], json!([]));
    let again = rig.request(
        &mut ws,
        12,
        json!({"type": "snapshot.delete", "name": "Chorus"}),
        "error",
    );
    assert_eq!(again["code"], "unknown-snapshot");
    // Pushed lists only: drain what is left.
    while let Some(got) = receive(&mut ws) {
        if let Got::Closed = got {
            panic!("closed");
        }
    }
    rig.finish();
}

/// Steps for `ms`, collecting every JSON message `ws` receives.
fn drain(rig: &mut Rig, ws: &mut Ws, ms: u64) -> Vec<Value> {
    let mut got = Vec::new();
    let until = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < until {
        rig.pump();
        while let Some(message) = receive(ws) {
            if let Some(json) = message.json() {
                got.push(json.clone());
            }
        }
    }
    got
}

/// Review of 2026-10-24, 1: a restore that writes one control and only
/// sends another (its file already holds the value) pushes `describe` once
/// every value is sent: each describe another client is pushed shows both.
fn a_mixed_restore_pushes_what_it_sent(transport: Transport) {
    let mut rig = tone("snapshot-mixed", transport);
    let (mut a, _) = rig.client();
    let (mut b, _) = rig.client();
    rig.request(
        &mut a,
        1,
        json!({"type": "snapshot.save", "name": "base"}),
        "snapshot.saved",
    );
    commit(&mut rig, &mut a, 2, "/a.freq", 330.0);
    // Live only: the file keeps 220.
    rig.request(
        &mut a,
        3,
        json!({"type": "set", "address": "/b.freq", "value": 100}),
        "ack",
    );
    drain(&mut rig, &mut b, 300);
    let restored = rig.request(
        &mut a,
        4,
        json!({"type": "snapshot.restore", "name": "base"}),
        "snapshot.restored",
    );
    assert_eq!(restored["written"], json!(["/a.freq"]), "{restored}");
    let pushed: Vec<Value> = drain(&mut rig, &mut b, 300)
        .into_iter()
        .filter(|m| m["type"] == "describe")
        .collect();
    assert!(!pushed.is_empty(), "describe is pushed");
    for describe in &pushed {
        assert_eq!(control(describe, "/a.freq")["base"], 440.0);
        assert_eq!(
            control(describe, "/b.freq")["base"],
            220.0,
            "the sent value"
        );
        assert_eq!(describe["unsaved"]["count"], 0, "{}", describe["unsaved"]);
    }
    rig.finish();
}

/// Review of 2026-10-24, 2: stopped, an edit of the files by another
/// program is what a save records, and a restore decides what to write on
/// the file as it reads it under the writer lock.
fn stopped_a_save_and_restore_see_the_files(transport: Transport) {
    let mut rig = tone("snapshot-stopped", transport);
    let (mut ws, _) = rig.client();
    let path = rig.dir.join("project.toml");
    rig.request(&mut ws, 1, json!({"type": "transport.stop"}), "transport");
    let edited = common::TONE.replace("freq = 440", "freq = 300");
    fs::write(&path, &edited).unwrap();
    let saved = rig.request(
        &mut ws,
        2,
        json!({"type": "snapshot.save", "name": "outside"}),
        "snapshot.saved",
    );
    let (all, _) = snapshot::read(&rig.dir).unwrap();
    assert_eq!(all.snapshots["outside"]["/a.freq"], 300.0, "{saved}");
    // Another program moves /b.freq; /a.freq stays as saved.
    fs::write(&path, edited.replace("freq = 220", "freq = 111")).unwrap();
    let restored = rig.request(
        &mut ws,
        3,
        json!({"type": "snapshot.restore", "name": "outside"}),
        "snapshot.restored",
    );
    assert_eq!(restored["written"], json!(["/b.freq"]), "{restored}");
    assert_eq!(fs::read_to_string(&path).unwrap(), edited);
    rig.finish();
}

/// Review of 2026-10-24, 7: a restore whose write fails still sends every
/// value (each stays live and unsaved, as after a failed commit) and says
/// what it skipped and clamped beside the error.
fn a_failed_restore_sends_and_reports(transport: Transport) {
    let mut rig = tone("snapshot-failed", transport);
    let (mut ws, _) = rig.client();
    fs::write(
        rig.dir.join(SNAPSHOTS_FILE),
        "schema_version = 1\n\n[snapshots.old]\n\"/a.freq\" = 330\n\"/gone.freq\" = 3\n",
    )
    .unwrap();
    // project.toml cannot be read for the write (a folder in its place),
    // while snapshots.toml takes the undo snapshot.
    let path = rig.dir.join("project.toml");
    let kept = rig.dir.join("project.toml.kept");
    fs::rename(&path, &kept).unwrap();
    fs::create_dir(&path).unwrap();
    let refused = rig.request(
        &mut ws,
        1,
        json!({"type": "snapshot.restore", "name": "old"}),
        "error",
    );
    fs::remove_dir(&path).unwrap();
    fs::rename(&kept, &path).unwrap();
    assert_eq!(refused["code"], "write-failed", "{refused}");
    assert_eq!(refused["unwritten"], json!(["/a.freq"]), "{refused}");
    assert_eq!(refused["skipped"][0]["address"], "/gone.freq", "{refused}");
    assert_eq!(refused["clamped"], json!([]));
    assert_eq!(refused["sent"], true);
    let now = describe(&mut rig, &mut ws, 2);
    assert_eq!(control(&now, "/a.freq")["base"], 330.0, "sent live");
    let unsaved: Vec<&str> = now["unsaved"]["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["address"].as_str().unwrap())
        .collect();
    assert_eq!(unsaved, ["/a.freq"]);
    rig.finish();
}

/// The owner's decision of 2026-10-24 ("legyen egy undo snapshot"): a
/// restore keeps what it replaces as the undo snapshot; undo brings those
/// values back exactly (written and live), and a second undo redoes. The
/// undo snapshot sits apart from the named ones, and the file after the
/// redo is byte for byte the file after the restore.
fn undo_restores_and_a_second_undo_redoes(transport: Transport) {
    let mut rig = tone("snapshot-undo", transport);
    let (mut ws, _) = rig.client();
    let snapshots_path = rig.dir.join(SNAPSHOTS_FILE);
    commit(&mut rig, &mut ws, 1, "/a.freq", 330.0);
    rig.request(
        &mut ws,
        2,
        json!({"type": "snapshot.save", "name": "A"}),
        "snapshot.saved",
    );
    let list = rig.request(&mut ws, 3, json!({"type": "snapshot.list"}), "snapshots");
    assert_eq!(list["undo"], false);
    let nothing = rig.request(&mut ws, 4, json!({"type": "snapshot.undo"}), "error");
    assert_eq!(nothing["code"], "unknown-snapshot");
    // Later: a commit and an unsaved live value.
    commit(&mut rig, &mut ws, 5, "/a.freq", 500.0);
    rig.request(
        &mut ws,
        6,
        json!({"type": "set", "address": "/b.freq", "value": 100}),
        "ack",
    );
    let before = describe(&mut rig, &mut ws, 7);
    let restored = rig.request(
        &mut ws,
        8,
        json!({"type": "snapshot.restore", "name": "A"}),
        "snapshot.restored",
    );
    assert_eq!(
        restored["restored"],
        before["controls"].as_array().unwrap().len()
    );
    let after_restore = fs::read(&snapshots_path).unwrap();
    let list = rig.request(&mut ws, 9, json!({"type": "snapshot.list"}), "snapshots");
    assert_eq!(list["undo"], true);
    assert_eq!(list["undoValues"]["/a.freq"], 500.0);
    assert_eq!(
        list["undoValues"]["/b.freq"], 100.0,
        "the live value, as it played"
    );
    assert_eq!(
        list["snapshots"].as_array().unwrap().len(),
        1,
        "the undo snapshot is no named one"
    );
    let undone = rig.request(
        &mut ws,
        10,
        json!({"type": "snapshot.undo", "base": list["base"]}),
        "snapshot.undone",
    );
    assert_eq!(undone["name"], "undo");
    let back = describe(&mut rig, &mut ws, 11);
    for c in before["controls"].as_array().unwrap() {
        let address = c["address"].as_str().unwrap();
        assert_eq!(
            control(&back, address)["base"],
            c["base"],
            "{address} exactly as before"
        );
    }
    assert_eq!(back["unsaved"]["count"], 0, "{}", back["unsaved"]);
    let text = fs::read_to_string(rig.dir.join("project.toml")).unwrap();
    assert!(text.contains("freq = 500"), "{text}");
    assert!(text.contains("freq = 100"), "{text}");
    // Undo again: the restore is redone.
    rig.request(
        &mut ws,
        12,
        json!({"type": "snapshot.undo"}),
        "snapshot.undone",
    );
    let redone = describe(&mut rig, &mut ws, 13);
    assert_eq!(control(&redone, "/a.freq")["base"], 330.0);
    assert_eq!(control(&redone, "/b.freq")["base"], 220.0);
    assert_eq!(
        fs::read(&snapshots_path).unwrap(),
        after_restore,
        "byte-stable"
    );
    rig.finish();
}

/// If the undo snapshot cannot be written (a conflict, a folder that
/// refuses the write), the restore does not happen: nothing is written or
/// sent, and the reply says why.
fn a_failed_undo_write_blocks_the_restore(transport: Transport) {
    let mut rig = tone("snapshot-undo-blocked", transport);
    let (mut ws, _) = rig.client();
    fs::write(
        rig.dir.join(SNAPSHOTS_FILE),
        "schema_version = 1\n\n[snapshots.old]\n\"/a.freq\" = 330\n",
    )
    .unwrap();
    let path = rig.dir.join("project.toml");
    let text = fs::read_to_string(&path).unwrap();
    let stale = rig.request(
        &mut ws,
        1,
        json!({"type": "snapshot.restore", "name": "old", "base": "0".repeat(64)}),
        "error",
    );
    assert_eq!(stale["code"], "conflict", "{stale}");
    if let Some(read_only) = ReadOnly::new(&rig.dir) {
        let refused = rig.request(
            &mut ws,
            2,
            json!({"type": "snapshot.restore", "name": "old"}),
            "error",
        );
        assert_eq!(refused["code"], "write-failed", "{refused}");
        assert!(
            refused["message"]
                .as_str()
                .unwrap()
                .contains(SNAPSHOTS_FILE),
            "{refused}"
        );
        drop(read_only);
    }
    assert_eq!(fs::read_to_string(&path).unwrap(), text, "nothing written");
    let now = describe(&mut rig, &mut ws, 3);
    assert_eq!(control(&now, "/a.freq")["base"], 440.0, "nothing sent");
    assert_eq!(now["unsaved"]["count"], 0);
    assert!(!snapshot::read(&rig.dir).unwrap().0.undo.is_some());
    rig.finish();
}

/// The owner's decision of 2026-10-24 ("lehessen állítani hogy a
/// snapshotokba bekerüljön vagy nem a tulajdonságaiban"): a control left
/// out of snapshots (`snapshot.exclude`, its name in the instance's
/// `snapshot_exclude`) is not saved, a restore of an older snapshot that
/// holds it leaves it alone (skipped, `excluded`), and the undo snapshot
/// does not hold it either. Writing the property reloads nothing.
fn excluded_controls_survive_save_restore_and_undo(transport: Transport) {
    let mut rig = tone("snapshot-exclude", transport);
    let (mut ws, _) = rig.client();
    let path = rig.dir.join("project.toml");
    let excluded = rig.request(
        &mut ws,
        1,
        json!({"type": "snapshot.exclude", "address": "/out.gain", "exclude": true}),
        "snapshot.excluded",
    );
    assert_eq!(excluded["written"], true);
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains(
            "out = { module = \"std.audio_out\", gain = 0.5, snapshot_exclude = [\"gain\"] }"
        ),
        "{text}"
    );
    rig.no_reload(&mut ws, 300);
    assert_eq!(
        control(&describe(&mut rig, &mut ws, 2), "/out.gain")["inSnapshots"],
        false
    );
    let saved = rig.request(
        &mut ws,
        3,
        json!({"type": "snapshot.save", "name": "S"}),
        "snapshot.saved",
    );
    let (all, _) = snapshot::read(&rig.dir).unwrap();
    assert!(!all.snapshots["S"].contains_key("/out.gain"), "{saved}");
    commit(&mut rig, &mut ws, 4, "/out.gain", 0.25);
    // An older snapshot that still holds the gain.
    let mut text = fs::read_to_string(rig.dir.join(SNAPSHOTS_FILE)).unwrap();
    text.push_str("\n[snapshots.old]\n\"/a.freq\" = 300\n\"/out.gain\" = 0.9\n");
    fs::write(rig.dir.join(SNAPSHOTS_FILE), text).unwrap();
    let restored = rig.request(
        &mut ws,
        5,
        json!({"type": "snapshot.restore", "name": "old"}),
        "snapshot.restored",
    );
    assert_eq!(
        restored["skipped"],
        json!([{"address": "/out.gain", "reason": "excluded"}])
    );
    assert_eq!(restored["written"], json!(["/a.freq"]));
    let (all, _) = snapshot::read(&rig.dir).unwrap();
    assert!(!all.undo.as_ref().unwrap().contains_key("/out.gain"));
    rig.request(
        &mut ws,
        6,
        json!({"type": "snapshot.undo"}),
        "snapshot.undone",
    );
    let now = describe(&mut rig, &mut ws, 7);
    assert_eq!(
        control(&now, "/out.gain")["base"],
        0.25,
        "left alone throughout"
    );
    assert_eq!(control(&now, "/a.freq")["base"], 440.0);
    // Back into snapshots: the key goes.
    rig.request(
        &mut ws,
        8,
        json!({"type": "snapshot.exclude", "address": "/out.gain", "exclude": false}),
        "snapshot.excluded",
    );
    assert!(
        !fs::read_to_string(&path)
            .unwrap()
            .contains("snapshot_exclude")
    );
    let bad = rig.request(
        &mut ws,
        9,
        json!({"type": "snapshot.exclude", "address": "/a/inner.freq"}),
        "error",
    );
    assert_eq!(bad["code"], "not-editable");
    rig.finish();
}

both!(
    a_restore_undoes_later_changes,
    gone_entries_are_skipped_and_out_of_range_clamped,
    names_overwrites_conflicts_and_delete,
    a_mixed_restore_pushes_what_it_sent,
    stopped_a_save_and_restore_see_the_files,
    a_failed_restore_sends_and_reports,
    undo_restores_and_a_second_undo_redoes,
    a_failed_undo_write_blocks_the_restore,
    excluded_controls_survive_save_restore_and_undo,
);

/// `signal snapshot`'s restore from the files alone: the values that differ
/// written into project.toml as one change, the gone ones skipped.
#[test]
fn a_restore_from_the_files_alone() {
    let dir = std::env::temp_dir().join(format!("signal-ui-snapshot-files-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("project.toml"), common::TONE).unwrap();
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let values = [
        ("/a.freq".to_owned(), 440.0),
        ("/b.freq".to_owned(), 330.0),
        ("/gone.freq".to_owned(), 1.0),
    ]
    .into_iter()
    .collect();
    let restored = signal_ui::snapshot::restore_files(&dir, &registry, &values).unwrap();
    assert_eq!(restored.written, ["/b.freq"]);
    assert_eq!(restored.restore.skipped.len(), 1);
    let text = fs::read_to_string(dir.join("project.toml")).unwrap();
    assert_eq!(text, common::TONE.replace("freq = 220", "freq = 330"));
    assert!(snapshot::read(&dir).unwrap().0.snapshots.is_empty());
    let _ = fs::remove_dir_all(&dir);
}
