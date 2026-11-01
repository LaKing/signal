//! M2.8-1: `setup.*` at parity with the M2.5 interface dialog: the device
//! list with labels and capabilities, refresh refused while playing, the
//! rate check, and Apply: the Setup saved, and the run played again from
//! frame 0 with its output, channel map, buffer, CPU workers and metered
//! input; a recording run is not restarted. Every saved Setup change
//! (`setup.apply`, `device.route`, `device.unroute`) is pushed to every
//! client as `setup`, with the run in force.

mod common;

use common::{Options, Transport, rig};
use serde_json::{Value, json};
use signal_ui::frames::{METERS, SCOPE};
use signal_ui::setup::Setup;

fn index(devices: &Value, name: &str) -> u64 {
    devices
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == name)
        .unwrap()["index"]
        .as_u64()
        .unwrap()
}

fn the_dialog_functions_and_apply_play_again_with_the_setup(transport: Transport) {
    let mut rig = rig(
        "setup",
        Options {
            transport,
            ..Options::default()
        },
    );
    let (mut ws, _) = rig.client();
    let setup = rig.request(&mut ws, 1, json!({"type": "setup.get"}), "setup");
    assert_eq!(setup["playing"], true);
    assert_eq!(setup["setup"]["buffer"], 128);
    assert_eq!(setup["run"]["device"], "null");

    let listed = rig.request(
        &mut ws,
        2,
        json!({"type": "setup.devices"}),
        "setup.devices",
    );
    let devices = listed["devices"].clone();
    assert_eq!(devices.as_array().unwrap().len(), 4);
    let four = index(&devices, "Null Four");
    let old = index(&devices, "Old Out");
    let input = index(&devices, "Null In");
    assert_eq!(devices[four as usize]["label"], "Null Four");
    assert!(
        devices[four as usize]["capabilities"][0]
            .as_str()
            .unwrap()
            .starts_with("4 channels, 44100-96000 Hz")
    );
    assert_eq!(listed["output"], Value::Null, "no saved output");
    assert_eq!(listed["compute"], json!(["CPU: test"]));
    let refused = rig.request(
        &mut ws,
        3,
        json!({"type": "setup.devices", "refresh": true}),
        "error",
    );
    assert_eq!(refused["code"], "playing");

    let check = rig.request(
        &mut ws,
        4,
        json!({"type": "setup.check", "output": old, "buffer": 128}),
        "setup.check",
    );
    assert_eq!(check["ok"], false, "{check}");
    let check = rig.request(
        &mut ws,
        5,
        json!({"type": "setup.check", "output": four}),
        "setup.check",
    );
    assert_eq!(check["ok"], true);
    assert_eq!(check["message"], "The project's 48000 Hz is supported.");
    // By identity, as the dialog sends it: found in the list as it is now.
    let by_identity = json!({
        "name": "Null Four",
        "id": devices[four as usize]["id"],
        "signature": devices[four as usize]["signature"],
    });
    let check = rig.request(
        &mut ws,
        51,
        json!({"type": "setup.check", "output": by_identity}),
        "setup.check",
    );
    assert_eq!(check["ok"], true, "{check}");
    let missing = rig.request(
        &mut ws,
        52,
        json!({"type": "setup.check", "output": {"name": "Gone", "id": "null:gone", "signature": ""}}),
        "error",
    );
    assert_eq!(missing["code"], "unknown-device");

    let bad = rig.request(
        &mut ws,
        6,
        json!({"type": "setup.apply", "outputChannels": "0"}),
        "error",
    );
    assert_eq!(bad["code"], "bad-value");

    // A scope and the input meters, to see them continue after Apply.
    let scope = rig.request(
        &mut ws,
        7,
        json!({"type": "subscribe", "stream": "scope", "port": "/a.out", "rate": 4800}),
        "subscribed",
    );
    let scope_sub = scope["sub"].as_u64().unwrap();
    let inputs = rig.request(
        &mut ws,
        8,
        json!({"type": "subscribe", "stream": "meters", "source": "input", "rate": 60}),
        "subscribed",
    );
    let input_sub = inputs["sub"].as_u64().unwrap();
    let before = rig.wait(&mut ws, "a scope frame", |got| got.frame(SCOPE, scope_sub));
    for _ in 0..200 {
        rig.pump();
    }

    let applied = rig.request(
        &mut ws,
        9,
        json!({
            "type": "setup.apply",
            "output": {"index": four},
            "input": {"index": input},
            "buffer": 256,
            "cpuWorkers": 2,
            "outputChannels": "4, 3",
        }),
        "applied",
    );
    assert_eq!(applied["saved"], true);
    assert_eq!(applied["restarted"], true, "{applied}");
    assert_eq!(applied["frame"], 0);
    assert_eq!(applied["run"]["buffer"], 256);
    assert_eq!(applied["run"]["threads"], 2);
    assert_eq!(applied["run"]["map"], json!([3, 2]));
    assert_eq!(applied["run"]["inputChannels"], 2);

    let (saved, problems) = Setup::load(&rig.setup);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(saved.output.as_ref().unwrap().name, "Null Four");
    assert_eq!(saved.input.as_ref().unwrap().name, "Null In");
    assert_eq!(saved.buffer, 256);
    assert_eq!(saved.cpu_workers, Some(2));
    assert_eq!(saved.output_channels, Some(vec![4, 3]));

    let restarted = rig.wait(&mut ws, "describe after Apply", |got| {
        got.of("describe")
            .filter(|d| d["restarted"] == true)
            .cloned()
    });
    assert_eq!(restarted["meters"]["input"].as_array().unwrap().len(), 2);
    // Playing again from frame 0: the scope continues from the start.
    let after = rig.wait(&mut ws, "a scope frame after Apply", |got| {
        got.frame(SCOPE, scope_sub)
            .filter(|f| f.first < before.first + 256 * 10)
    });
    assert!(
        after.first < before.first + 2560,
        "{} then {}",
        before.first,
        after.first
    );
    let stats = rig.request(&mut ws, 10, json!({"type": "stats"}), "stats");
    assert!(stats["frame"].as_u64().unwrap() < 48_000, "{stats}");

    // The input is metered.
    rig.host().push_input(&[0.5; 512]).unwrap();
    let meters = rig.wait(&mut ws, "input meters", |got| got.frame(METERS, input_sub));
    assert_eq!(meters.count, 2);
    assert_eq!(meters.f32s()[0], 0.5);

    let get = rig.request(&mut ws, 11, json!({"type": "setup.get"}), "setup");
    assert_eq!(get["setup"]["outputChannels"], json!([4, 3]));

    // ADR 0036: `"device"` follows the device's buffer; a number given
    // replaces it, even the one the Setup holds; absent keeps it.
    let apply = |buffer: Value| json!({"type": "setup.apply", "buffer": buffer});
    rig.request(&mut ws, 12, apply(json!("device")), "applied");
    assert!(Setup::load(&rig.setup).0.buffer_device);
    rig.request(&mut ws, 13, json!({"type": "setup.apply"}), "applied");
    assert!(Setup::load(&rig.setup).0.buffer_device, "absent keeps it");
    rig.request(&mut ws, 14, apply(json!(256)), "applied");
    let (saved, _) = Setup::load(&rig.setup);
    assert!(!saved.buffer_device && saved.buffer == 256);
    rig.finish();
}

fn a_recording_run_saves_the_setup_but_is_not_restarted(transport: Transport) {
    let mut rig = rig(
        "setup-recording",
        Options {
            recording: true,
            transport,
            ..Options::default()
        },
    );
    let (mut ws, _) = rig.client();
    let refused = rig.request(
        &mut ws,
        1,
        json!({"type": "setup.apply", "buffer": 512}),
        "error",
    );
    assert_eq!(refused["code"], "recording");
    assert_eq!(Setup::load(&rig.setup).0.buffer, 512);
    let setup = rig.request(&mut ws, 2, json!({"type": "setup.get"}), "setup");
    assert_eq!(setup["run"]["buffer"], 128, "the run plays on as it was");
    rig.finish();
}

/// The next `setup` push (no `id`) on `ws`.
fn setup_push(rig: &mut common::Rig, ws: &mut common::Ws, what: &str) -> Value {
    rig.wait(ws, what, |got| {
        got.of("setup").filter(|v| v.get("id").is_none()).cloned()
    })
}

fn a_saved_setup_change_is_pushed_to_every_client(transport: Transport) {
    let mut rig = rig(
        "setup-push",
        Options {
            transport,
            ..Options::default()
        },
    );
    let (mut a, _) = rig.client();
    let (mut b, _) = rig.client();
    let listed = rig.request(&mut a, 1, json!({"type": "setup.devices"}), "setup.devices");
    let four = index(&listed["devices"], "Null Four");

    // A refused Apply saves nothing and pushes nothing: the first push the
    // other client sees is the next saved change's.
    let bad = rig.request(
        &mut a,
        2,
        json!({"type": "setup.apply", "outputChannels": "0"}),
        "error",
    );
    assert_eq!(bad["code"], "bad-value");
    let applied = rig.request(
        &mut a,
        3,
        json!({"type": "setup.apply", "output": {"index": four}, "buffer": 256}),
        "applied",
    );
    assert_eq!(applied["restarted"], true, "{applied}");
    for ws in [&mut b, &mut a] {
        let push = setup_push(&mut rig, ws, "the setup push after Apply");
        assert_eq!(push["setup"]["buffer"], 256, "{push}");
        assert_eq!(push["setup"]["output"]["name"], "Null Four", "{push}");
        assert_eq!(push["run"]["buffer"], 256, "the run in force: {push}");
        assert_eq!(push["playing"], true);
        assert_eq!(
            push["resolution"]["output"],
            json!({"found": four}),
            "{push}"
        );
        assert_eq!(push["resolution"]["input"], Value::Null);
        // What `setup.get` answers, keys and all.
        let get = rig.request(ws, 4, json!({"type": "setup.get"}), "setup");
        for key in [
            "path",
            "setup",
            "local",
            "problems",
            "playing",
            "run",
            "rtPolicies",
        ] {
            assert_eq!(push[key], get[key], "{key}");
        }
    }

    // A cable from the other window: `device.route`, then `device.unroute`.
    let routed = rig.request(
        &mut b,
        5,
        json!({"type": "device.route", "role": "main", "channels": [3, 4]}),
        "applied",
    );
    assert_eq!(routed["saved"], true, "{routed}");
    // Played again from frame 0, as after Apply: `describe` with
    // `restarted`, then the push.
    let restarted = rig.wait(&mut a, "describe after device.route", |got| {
        got.of("describe")
            .filter(|d| d["restarted"] == true)
            .cloned()
    });
    assert_eq!(restarted["playing"], true);
    let push = setup_push(&mut rig, &mut a, "the setup push after device.route");
    assert_eq!(push["setup"]["outputChannels"], json!([3, 4]), "{push}");
    assert_eq!(push["run"]["map"], json!([2, 3]), "{push}");
    setup_push(&mut rig, &mut b, "the router's own push");

    rig.request(
        &mut b,
        6,
        json!({"type": "device.unroute", "role": "main"}),
        "applied",
    );
    let push = setup_push(&mut rig, &mut a, "the setup push after device.unroute");
    assert_eq!(push["setup"]["outputChannels"], Value::Null, "{push}");
    assert_eq!(push["run"]["map"], json!([0, 1]), "{push}");
    setup_push(&mut rig, &mut b, "the unrouter's own push");

    // This project's binding (`devices.local.toml`) is in `local`.
    rig.request(
        &mut a,
        7,
        json!({"type": "device.route", "role": "main", "channels": [2, 1], "scope": "project"}),
        "applied",
    );
    let push = setup_push(&mut rig, &mut b, "the setup push after a project route");
    assert_eq!(
        push["local"]["play"]["main"]["channels"],
        json!([2, 1]),
        "{push}"
    );
    rig.finish();
}

both!(
    the_dialog_functions_and_apply_play_again_with_the_setup,
    a_recording_run_saves_the_setup_but_is_not_restarted,
    a_saved_setup_change_is_pushed_to_every_client
);
