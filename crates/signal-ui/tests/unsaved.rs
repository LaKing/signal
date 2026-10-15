//! M2.8-8: a failed write is visible (`docs/ui-protocol.md` "Unsaved"). A
//! commit into a read-only project folder answers `write-failed`; the
//! parameter is listed as unsaved in `describe` and through the `unsaved`
//! push, to every client and to one that connects later; `retry` writes the
//! latest live value once the cause is fixed, `revert` sets the live value
//! back to the file's, and either clears it. A refused edit never clears it.

mod common;

use std::fs;

use common::{Got, Options, ReadOnly, Rig, Transport, Ws, receive, rig, send};
use serde_json::{Value, json};

const GAIN: &str = "/out.gain";
/// `out.gain` in the tone project's file.
const FILE_GAIN: f64 = 0.5;

fn control<'a>(describe: &'a Value, address: &str) -> &'a Value {
    describe["controls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["address"] == address)
        .unwrap_or_else(|| panic!("no control {address}"))
}

fn commit(rig: &mut Rig, ws: &mut Ws, id: u64, value: f64) -> Value {
    rig.request(
        ws,
        id,
        json!({"type": "commit", "address": GAIN, "value": value}),
        "error",
    )
}

fn listed(unsaved: &Value) -> Option<&Value> {
    unsaved["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["address"] == GAIN)
}

fn rig_for(name: &str, transport: Transport) -> Rig {
    rig(
        name,
        Options {
            transport,
            ..Options::default()
        },
    )
}

fn a_failed_commit_stays_unsaved_until_retried(transport: Transport) {
    let mut rig = rig_for("unsaved-retry", transport);
    let path = rig.dir.join("project.toml");
    let before = fs::read_to_string(&path).unwrap();
    let (mut a, _) = rig.client();
    let (mut b, _) = rig.client();
    let Some(read_only) = ReadOnly::new(&rig.dir) else {
        return rig.finish();
    };

    let refused = commit(&mut rig, &mut a, 1, 0.125);
    assert_eq!(refused["code"], "write-failed", "{refused}");
    let error = refused["message"].as_str().unwrap().to_owned();
    for ws in [&mut a, &mut b] {
        let unsaved = rig.unsaved(ws, "the failed write listed", |u| u["count"] == 1);
        let entry = listed(&unsaved).expect("/out.gain listed");
        assert_eq!(entry["error"], error.as_str());
        assert_eq!(entry["file"], FILE_GAIN);
        assert_eq!(entry["inFile"], true);
        assert_eq!(entry["control"], true);
        assert_eq!(entry["applied"], true);
        assert_eq!(unsaved["lastError"], error.as_str());
    }
    // A client that connects later (a page reload) is told in `describe`.
    let (mut c, _) = rig.client();
    let described = rig.request(&mut c, 2, json!({"type": "describe"}), "describe");
    assert_eq!(described["unsaved"]["count"], 1);
    assert_eq!(
        listed(&described["unsaved"]).unwrap()["error"],
        error.as_str()
    );
    assert_eq!(control(&described, GAIN)["base"], 0.125);
    // Live 0.125, the file as it was.
    let sub = rig.watch_parameter(&mut c, 3, GAIN);
    rig.wait_base(&mut c, sub, 0.125);
    assert_eq!(fs::read_to_string(&path).unwrap(), before);

    // The cause fixed, `retry` writes the live value and clears it.
    drop(read_only);
    let committed = rig.request(
        &mut a,
        4,
        json!({"type": "retry", "address": GAIN}),
        "committed",
    );
    assert_eq!(committed["value"], 0.125);
    let expected =
        signal_project::write_parameter(&before, "out", "gain", Some(0.125), &["gain"]).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), expected);
    let unsaved = rig.unsaved(&mut b, "nothing unsaved", |u| u["count"] == 0);
    assert_eq!(unsaved["lastError"], Value::Null);
    let described = rig.request(&mut c, 5, json!({"type": "describe"}), "describe");
    assert_eq!(described["unsaved"]["count"], 0);
    let again = rig.request(
        &mut a,
        6,
        json!({"type": "retry", "address": GAIN}),
        "error",
    );
    assert_eq!(again["code"], "not-unsaved");
    rig.finish();
}

/// auditor-1-stale-retry: a newer live edit replaces the failed value, and
/// `retry` writes the newest; live and file agreeing again clears it.
fn a_newer_live_edit_is_what_retry_writes(transport: Transport) {
    let mut rig = rig_for("unsaved-newer", transport);
    let path = rig.dir.join("project.toml");
    let before = fs::read_to_string(&path).unwrap();
    let (mut ws, _) = rig.client();
    let Some(read_only) = ReadOnly::new(&rig.dir) else {
        return rig.finish();
    };
    assert_eq!(commit(&mut rig, &mut ws, 1, 0.125)["code"], "write-failed");
    rig.request(
        &mut ws,
        2,
        json!({"type": "set", "address": GAIN, "value": 0.3}),
        "ack",
    );
    let described = rig.request(&mut ws, 3, json!({"type": "describe"}), "describe");
    assert_eq!(control(&described, GAIN)["base"], 0.3);
    assert_eq!(described["unsaved"]["count"], 1, "still unsaved");
    let second = commit(&mut rig, &mut ws, 4, 0.2);
    assert_eq!(second["code"], "write-failed");
    let described = rig.request(&mut ws, 5, json!({"type": "describe"}), "describe");
    assert_eq!(control(&described, GAIN)["base"], 0.2);
    assert_eq!(fs::read_to_string(&path).unwrap(), before);

    drop(read_only);
    let committed = rig.request(
        &mut ws,
        6,
        json!({"type": "retry", "address": GAIN}),
        "committed",
    );
    assert_eq!(committed["value"], 0.2, "the newest live value");
    let written =
        signal_project::write_parameter(&before, "out", "gain", Some(0.2), &["gain"]).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), written);
    rig.unsaved(&mut ws, "saved", |u| u["count"] == 0);
    let sub = rig.watch_parameter(&mut ws, 7, GAIN);
    rig.wait_base(&mut ws, sub, 0.2);

    // A failed write, then the live value set back to the file's: live and
    // file agree, nothing is unsaved (no write).
    let read_only = ReadOnly::new(&rig.dir).unwrap();
    assert_eq!(commit(&mut rig, &mut ws, 8, 0.5)["code"], "write-failed");
    rig.unsaved(&mut ws, "listed", |u| u["count"] == 1);
    // (The push comes before the edit's ack: wait for the push.)
    send(
        &mut ws,
        json!({"type": "set", "address": GAIN, "value": 0.2, "id": 9}),
    );
    rig.unsaved(&mut ws, "live and file agree", |u| u["count"] == 0);
    // A live edit not committed is unsaved, without an error.
    send(
        &mut ws,
        json!({"type": "set", "address": GAIN, "value": 0.6, "id": 10}),
    );
    let unsaved = rig.unsaved(&mut ws, "an edit not committed", |u| u["count"] == 1);
    assert_eq!(listed(&unsaved).unwrap()["error"], Value::Null);
    assert_eq!(fs::read_to_string(&path).unwrap(), written);
    drop(read_only);
    rig.finish();
}

/// Sends without running the null driver (its commands pile up), then
/// steps the host: the replies that came.
fn unpumped(rig: &mut Rig, ws: &mut Ws, messages: Vec<Value>) -> Vec<Value> {
    for message in messages {
        send(ws, message);
    }
    let mut got = Vec::new();
    for _ in 0..20 {
        rig.host().step();
        while let Some(reply) = receive(ws) {
            assert!(!matches!(reply, Got::Closed), "the client was closed");
            if let Some(json) = reply.json() {
                got.push(json.clone());
            }
        }
    }
    got
}

/// Fills the Controller's command queue with `set`s of `value` (the live
/// value, so nothing else changes) until one is refused; the number taken.
fn fill(rig: &mut Rig, ws: &mut Ws, value: f64, ids: &mut u64) -> usize {
    let mut taken = 0;
    for _ in 0..25 {
        let batch: Vec<Value> = (0..200)
            .map(|_| {
                *ids += 1;
                json!({"type": "set", "address": GAIN, "value": value, "id": *ids})
            })
            .collect();
        let replies = unpumped(rig, ws, batch);
        let refused = replies
            .iter()
            .filter(|r| r["type"] == "error" && r["code"] == "refused")
            .count();
        taken += 200 - refused;
        if refused > 0 {
            return taken;
        }
    }
    panic!("the command queue never filled");
}

/// Runs the null driver until `taken` acks came.
fn drain(rig: &mut Rig, ws: &mut Ws, taken: usize) {
    let mut acks = 0;
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while acks < taken {
        assert!(std::time::Instant::now() < until, "{acks} of {taken} acks");
        rig.pump();
        while let Some(got) = receive(ws) {
            assert!(!matches!(got, Got::Closed), "the client was closed");
            if got.of("ack").is_some() {
                acks += 1;
            }
        }
    }
}

/// Without running the null driver: `message`'s reply.
fn ask(rig: &mut Rig, ws: &mut Ws, id: u64, message: Value) -> Value {
    let mut message = message;
    message["id"] = json!(id);
    unpumped(rig, ws, vec![message])
        .into_iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("no reply to {id}"))
}

/// auditor-1-refused-edit-clears-unsaved: a failed write, then a `set` back
/// to the file's value that the Controller refuses (its command queue is
/// full: the null driver does not run, so nothing drains it). The ports
/// still hold the failed value: the control stays listed, with its error,
/// until a revert (or retry) is taken.
fn a_refused_edit_back_to_the_file_value_stays_unsaved(transport: Transport) {
    // The queue's acks arrive in one step once the driver runs: the client's
    // queue holds them all (else the host evicts it, as it should).
    let mut rig = rig(
        "unsaved-refused",
        Options {
            transport,
            client_queue: 4096,
            ..Options::default()
        },
    );
    let path = rig.dir.join("project.toml");
    let before = fs::read_to_string(&path).unwrap();
    let (mut ws, _) = rig.client();
    let Some(probe) = ReadOnly::new(&rig.dir) else {
        return rig.finish();
    };
    drop(probe);
    let mut ids = 1000;
    for retrying in [false, true] {
        let mut read_only = ReadOnly::new(&rig.dir);
        let failed = commit(&mut rig, &mut ws, 1, 0.125);
        assert_eq!(failed["code"], "write-failed");
        let error = failed["message"].clone();
        let taken = fill(&mut rig, &mut ws, 0.125, &mut ids);
        let back = ask(
            &mut rig,
            &mut ws,
            2,
            json!({"type": "set", "address": GAIN, "value": FILE_GAIN}),
        );
        assert_eq!(back["code"], "refused", "{back}");
        let described = ask(&mut rig, &mut ws, 3, json!({"type": "describe"}));
        let entry = listed(&described["unsaved"]).expect("still listed");
        assert_eq!(entry["applied"], false);
        assert_eq!(entry["error"], error);
        assert_eq!(
            control(&described, GAIN)["base"],
            0.125,
            "the last value applied, not the refused one"
        );
        let revert = ask(
            &mut rig,
            &mut ws,
            4,
            json!({"type": "revert", "address": GAIN}),
        );
        assert_eq!(revert["code"], "refused", "{revert}");
        let described = ask(&mut rig, &mut ws, 5, json!({"type": "describe"}));
        assert_eq!(listed(&described["unsaved"]).unwrap()["error"], error);

        drain(&mut rig, &mut ws, taken);
        let sub = rig.watch_parameter(&mut ws, 6, GAIN);
        if retrying {
            // The folder writable again: `retry` writes the latest value
            // sent (the file's) and sends it.
            drop(read_only.take());
            let committed = rig.request(
                &mut ws,
                7,
                json!({"type": "retry", "address": GAIN}),
                "committed",
            );
            assert_eq!(committed["value"], FILE_GAIN);
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                signal_project::write_parameter(&before, "out", "gain", Some(FILE_GAIN), &["gain"])
                    .unwrap()
            );
        } else {
            let reverted = rig.request(
                &mut ws,
                7,
                json!({"type": "revert", "address": GAIN}),
                "reverted",
            );
            assert_eq!(reverted["value"], FILE_GAIN);
            assert_eq!(reverted["sent"], true);
            assert_eq!(fs::read_to_string(&path).unwrap(), before);
        }
        rig.unsaved(&mut ws, "nothing unsaved", |u| u["count"] == 0);
        rig.wait_base(&mut ws, sub, FILE_GAIN);
        rig.request(
            &mut ws,
            8,
            json!({"type": "unsubscribe", "sub": sub}),
            "unsubscribed",
        );
        drop(read_only);
    }
    rig.finish();
}

fn revert_restores_the_file_value_live(transport: Transport) {
    let mut rig = rig_for("unsaved-revert", transport);
    let path = rig.dir.join("project.toml");
    let before = fs::read_to_string(&path).unwrap();
    let (mut ws, _) = rig.client();
    let Some(read_only) = ReadOnly::new(&rig.dir) else {
        return rig.finish();
    };
    let sub = rig.watch_parameter(&mut ws, 1, GAIN);
    // (Each wait drops what it does not want: the push, then the frames.)
    assert_eq!(commit(&mut rig, &mut ws, 2, 0.3)["code"], "write-failed");
    rig.unsaved(&mut ws, "listed", |u| u["count"] == 1);
    rig.wait_base(&mut ws, sub, 0.3);
    let reverted = rig.request(
        &mut ws,
        3,
        json!({"type": "revert", "address": GAIN}),
        "reverted",
    );
    assert_eq!(reverted["value"], FILE_GAIN);
    rig.unsaved(&mut ws, "nothing unsaved", |u| u["count"] == 0);
    rig.wait_base(&mut ws, sub, FILE_GAIN);
    let described = rig.request(&mut ws, 4, json!({"type": "describe"}), "describe");
    assert_eq!(control(&described, GAIN)["base"], FILE_GAIN);
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    let again = rig.request(
        &mut ws,
        5,
        json!({"type": "revert", "address": GAIN}),
        "error",
    );
    assert_eq!(again["code"], "not-unsaved");
    drop(read_only);
    rig.finish();
}

/// A reload that writes the unsaved parameter's key sends the file's value
/// live and clears it; a reload of another key keeps it.
fn a_reload_that_writes_the_parameter_clears_it(transport: Transport) {
    let mut rig = rig_for("unsaved-reload", transport);
    let path = rig.dir.join("project.toml");
    let before = fs::read_to_string(&path).unwrap();
    let (mut ws, _) = rig.client();
    let Some(read_only) = ReadOnly::new(&rig.dir) else {
        return rig.finish();
    };
    assert_eq!(commit(&mut rig, &mut ws, 1, 0.3)["code"], "write-failed");
    rig.unsaved(&mut ws, "listed", |u| u["count"] == 1);
    drop(read_only);

    fs::write(&path, before.replace("freq = 440", "freq = 330")).unwrap();
    let reload = rig.wait(&mut ws, "the reload of another key", |got| {
        got.of("reload").cloned()
    });
    assert_eq!(reload["sets"], 1, "{reload}");
    let described = rig.request(&mut ws, 2, json!({"type": "describe"}), "describe");
    assert_eq!(described["unsaved"]["count"], 1, "kept");
    assert_eq!(control(&described, GAIN)["base"], 0.3);

    let sub = rig.watch_parameter(&mut ws, 3, GAIN);
    fs::write(
        &path,
        before
            .replace("freq = 440", "freq = 330")
            .replace("gain = 0.5", "gain = 0.75"),
    )
    .unwrap();
    rig.wait(&mut ws, "the reload of the key", |got| {
        got.of("reload").cloned()
    });
    rig.unsaved(&mut ws, "cleared by the reload", |u| u["count"] == 0);
    rig.wait_base(&mut ws, sub, 0.75);
    rig.finish();
}

both!(
    a_failed_commit_stays_unsaved_until_retried,
    a_newer_live_edit_is_what_retry_writes,
    a_refused_edit_back_to_the_file_value_stays_unsaved,
    revert_restores_the_file_value_live,
    a_reload_that_writes_the_parameter_clears_it
);
