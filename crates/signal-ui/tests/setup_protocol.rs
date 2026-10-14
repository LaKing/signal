//! M2.8-1: `setup.*` at parity with the M2.5 interface dialog: the device
//! list with labels and capabilities, refresh refused while playing, the
//! rate check, and Apply: the Setup saved, and the run played again from
//! frame 0 with its output, channel map, buffer, CPU workers and metered
//! input; a recording run is not restarted.

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

both!(
    the_dialog_functions_and_apply_play_again_with_the_setup,
    a_recording_run_saves_the_setup_but_is_not_restarted
);
