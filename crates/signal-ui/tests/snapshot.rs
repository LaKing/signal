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

use common::{Got, Options, Rig, Transport, Ws, receive, rig};
use serde_json::{Value, json};
use signal_core::Registry;
use signal_project::snapshot::{self, SNAPSHOTS_FILE};

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
    for (id, name) in [(4, json!("   ")), (5, json!("x".repeat(65))), (6, json!(3))] {
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

both!(
    a_restore_undoes_later_changes,
    gone_entries_are_skipped_and_out_of_range_clamped,
    names_overwrites_conflicts_and_delete,
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
