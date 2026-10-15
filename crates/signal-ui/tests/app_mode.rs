//! M2.8-4: the host in app mode (Signal.app's server): it starts stopped,
//! plays and stops on `transport.*` with the Setup, pushes the status line,
//! opens another project (and serves its panels), keeps a device chosen by
//! identity, takes the wake policy, and commits while stopped. `signal ui`
//! (CLI mode) refuses the app's messages.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{Options, Rig, Transport, copy_dir, devices, rig, root};
use serde_json::{Value, json};
use signal_core::Registry;
use signal_engine::RtPolicy;
use signal_ui::player::Backend;
use signal_ui::setup::Setup;
use signal_ui::{AppHostConfig, Host, ServerOptions};

/// A host in app mode on the null driver, its folder holding the tone
/// project (or a copy of `project`).
fn app_rig(name: &str, transport: Transport, project: Option<&Path>) -> Rig {
    let base = std::env::temp_dir().join(format!(
        "signal-ui-app-{name}-{transport:?}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&base);
    let dir = base.join("project");
    match project {
        Some(project) => copy_dir(project, &dir),
        None => {
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("project.toml"), common::TONE).unwrap();
        }
    }
    let setup = base.join("setup.toml");
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let config = AppHostConfig {
        setup_path: setup.clone(),
        project: Some(dir.clone()),
        registry,
        backend: Backend::Null(devices()),
        compute: vec!["CPU: test".to_owned()],
        server: ServerOptions {
            port: 0,
            remote: false,
            ui_dir: root().join("ui"),
            project_ui: PathBuf::new(),
            client_queue: 256,
        },
    };
    let host = match transport {
        Transport::Tcp => Host::new_app(config),
        Transport::Local => Host::new_app_local(config),
    }
    .unwrap();
    Rig {
        host: Some(host),
        dir,
        setup,
    }
}

fn status(rig: &mut Rig, ws: &mut common::Ws, playing: bool) -> Value {
    rig.wait(ws, "status", |got| {
        got.of("status")
            .filter(|s| s["playing"] == playing)
            .cloned()
    })
}

fn it_starts_stopped_plays_and_stops(transport: Transport) {
    let mut rig = app_rig("transport", transport, None);
    let (mut ws, _) = rig.client();
    let app = rig.request(&mut ws, 1, json!({"type": "app.get"}), "app");
    assert_eq!(app["playing"], false);
    assert_eq!(app["project"]["dir"], rig.dir.display().to_string());
    assert_eq!(app["project"]["sampleRate"], 48_000);
    assert_eq!(app["project"]["diagnostics"], json!([]));
    let described = rig.request(&mut ws, 2, json!({"type": "describe"}), "describe");
    assert_eq!(described["playing"], false);
    assert!(
        described["controls"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["address"] == "/out.gain"),
        "the parameter list is there while stopped: {described}"
    );

    let played = rig.request(&mut ws, 3, json!({"type": "transport.play"}), "transport");
    assert_eq!(played["playing"], true);
    // Every client is told what plays now, then the status line follows.
    let described = rig.wait(&mut ws, "describe", |got| {
        got.of("describe").filter(|d| d["playing"] == true).cloned()
    });
    assert_eq!(described["meters"]["output"][0]["label"], "out 1 → 1");
    let line = status(&mut rig, &mut ws, true);
    assert_eq!(line["status"]["xruns"], 0);
    assert_eq!(line["status"]["buffer"], 128, "the Setup's buffer");
    assert!(line["line"].as_str().unwrap().starts_with("frame "));

    let stopped = rig.request(&mut ws, 4, json!({"type": "transport.stop"}), "transport");
    assert_eq!(stopped["playing"], false);
    let line = status(&mut rig, &mut ws, false);
    assert!(line["status"]["frame"].as_u64().unwrap() > 0, "{line}");
    rig.finish();
}

fn a_project_that_does_not_load_shows_its_diagnostics(transport: Transport) {
    let mut rig = app_rig("broken", transport, None);
    fs::write(
        rig.dir.join("project.toml"),
        "schema_version = 1\n[instances\n",
    )
    .unwrap();
    let (mut ws, _) = rig.client();
    let dir = rig.dir.display().to_string();
    let opened = rig.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": dir}),
        "project",
    );
    assert_eq!(opened["loads"], false);
    let app = rig.request(&mut ws, 2, json!({"type": "app.get"}), "app");
    assert!(!app["project"]["diagnostics"].as_array().unwrap().is_empty());
    assert_eq!(app["project"]["sampleRate"], Value::Null);
    let refused = rig.request(&mut ws, 3, json!({"type": "transport.play"}), "error");
    assert_eq!(refused["code"], "does-not-load");
    let app = rig.request(&mut ws, 4, json!({"type": "app.get"}), "app");
    assert_eq!(app["message"], "the project does not load");
    rig.finish();
}

fn opening_a_project_stops_records_it_and_serves_its_panels(transport: Transport) {
    let mut rig = app_rig("open", transport, None);
    let kick = rig.dir.parent().unwrap().join("kick");
    copy_dir(&root().join("projects/kick"), &kick);
    let (mut ws, _) = rig.client();
    // The tone project has no ui/ folder.
    let none = rig.request(
        &mut ws,
        1,
        json!({"type": "panel.check", "file": "main.html"}),
        "error",
    );
    assert_eq!(none["code"], "unknown-panel");
    rig.request(&mut ws, 2, json!({"type": "transport.play"}), "transport");
    let opened = rig.request(
        &mut ws,
        3,
        json!({"type": "project.open", "path": kick.display().to_string()}),
        "project",
    );
    assert_eq!(opened["loads"], true);
    let app = rig.request(&mut ws, 4, json!({"type": "app.get"}), "app");
    assert_eq!(app["playing"], false, "opening stops");
    assert_eq!(app["project"]["sampleRate"], 96_000);
    assert_eq!(Setup::load(&rig.setup).0.last_project, Some(kick.clone()));
    let checked = rig.request(
        &mut ws,
        5,
        json!({"type": "panel.check", "file": "main.html"}),
        "panel.check",
    );
    assert_eq!(checked["file"], "ui/main.html");
    assert_eq!(checked["ok"], true, "{checked}");
    rig.finish();
    let _ = fs::remove_dir_all(&kick);
}

fn apply_takes_the_wake_policy_and_plays_again(transport: Transport) {
    let mut rig = app_rig("apply", transport, None);
    let (mut ws, _) = rig.client();
    let setup = rig.request(&mut ws, 1, json!({"type": "setup.get"}), "setup");
    assert_eq!(setup["setup"]["rtPolicy"], "park50");
    assert!(
        setup["rtPolicies"]
            .as_array()
            .unwrap()
            .contains(&json!("semaphore"))
    );
    let stopped = rig.request(
        &mut ws,
        2,
        json!({"type": "setup.apply", "rtPolicy": "budget-spin"}),
        "applied",
    );
    assert_eq!(stopped["restarted"], false, "stopped: saved only");
    assert_eq!(Setup::load(&rig.setup).0.rt_policy, RtPolicy::BudgetSpin);
    rig.request(&mut ws, 3, json!({"type": "transport.play"}), "transport");
    let applied = rig.request(
        &mut ws,
        4,
        json!({"type": "setup.apply", "rtPolicy": "semaphore", "buffer": 256}),
        "applied",
    );
    assert_eq!(applied["restarted"], true, "{applied}");
    assert_eq!(applied["frame"], 0);
    assert_eq!(applied["run"]["buffer"], 256);
    let saved = Setup::load(&rig.setup).0;
    assert_eq!((saved.rt_policy, saved.buffer), (RtPolicy::Semaphore, 256));
    let app = rig.request(&mut ws, 5, json!({"type": "app.get"}), "app");
    assert_eq!(app["message"], "applied; playing again from frame 0");
    let refused = rig.request(
        &mut ws,
        6,
        json!({"type": "setup.apply", "rtPolicy": "spin-forever"}),
        "error",
    );
    assert_eq!(refused["code"], "bad-value");
    rig.finish();
}

fn a_chosen_device_is_found_again_by_identity(transport: Transport) {
    let mut rig = app_rig("choices", transport, None);
    let (mut ws, _) = rig.client();
    let listed = rig.request(
        &mut ws,
        1,
        json!({"type": "setup.devices", "choices": {
            "output": {"name": "Null Four", "id": "null:null-four"},
            "input": {"name": "Null In", "id": "null:gone"},
        }}),
        "setup.devices",
    );
    let four = listed["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "Null Four")
        .unwrap()["index"]
        .clone();
    assert_eq!(listed["choices"]["output"], json!({"found": four}));
    assert_eq!(
        listed["choices"]["input"]["unresolved"],
        "the saved input 'Null In' (id null:gone) is not present; choose a device"
    );
    rig.request(&mut ws, 2, json!({"type": "transport.play"}), "transport");
    let refused = rig.request(
        &mut ws,
        3,
        json!({"type": "setup.devices", "refresh": true}),
        "error",
    );
    assert_eq!(
        refused["message"],
        "stop playback to refresh: listing devices pauses metering and status"
    );
    rig.finish();
}

fn a_commit_while_stopped_writes_once(transport: Transport) {
    let mut rig = app_rig("commit", transport, None);
    let (mut ws, _) = rig.client();
    let committed = rig.request(
        &mut ws,
        1,
        json!({"type": "commit", "address": "/out.gain", "value": 0.25}),
        "committed",
    );
    assert_eq!(committed["writes"], 1);
    let text = fs::read_to_string(rig.dir.join("project.toml")).unwrap();
    assert!(text.contains("out = { module = \"std.audio_out\", gain = 0.25 }"));
    let described = rig.request(&mut ws, 2, json!({"type": "describe"}), "describe");
    let gain = described["controls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["address"] == "/out.gain")
        .unwrap()
        .clone();
    assert_eq!(gain["base"], 0.25);
    rig.finish();
}

/// auditor-1-7: a composite's inner parameter shows the value a stopped
/// commit wrote for the parameter it exports.
fn a_stopped_commit_updates_the_inner_bases(transport: Transport) {
    let mut rig = app_rig("inner", transport, Some(&root().join("projects/kick")));
    let (mut ws, _) = rig.client();
    let inner = |described: &Value| {
        described["instances"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["address"] == "/drums/kick/amp_env")
            .unwrap()["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "decay_ms")
            .unwrap()["base"]
            .clone()
    };
    let before = rig.request(&mut ws, 1, json!({"type": "describe"}), "describe");
    assert_eq!(inner(&before), 420.0);
    rig.request(
        &mut ws,
        2,
        json!({"type": "commit", "address": "/drums.kick_decay", "value": 500}),
        "committed",
    );
    let after = rig.request(&mut ws, 3, json!({"type": "describe"}), "describe");
    assert_eq!(inner(&after), 500.0);
    rig.finish();
}

fn signal_ui_refuses_the_app_s_messages(transport: Transport) {
    let mut rig = rig(
        "not-app",
        Options {
            transport,
            ..Options::default()
        },
    );
    let (mut ws, _) = rig.client();
    for (id, kind) in [(1, "project.open"), (2, "transport.stop"), (3, "app.get")] {
        let refused = rig.request(&mut ws, id, json!({"type": kind, "path": "/tmp"}), "error");
        assert_eq!(refused["code"], "not-allowed", "{kind}");
    }
    let described = rig.request(&mut ws, 4, json!({"type": "describe"}), "describe");
    assert_eq!(described["playing"], true, "signal ui still plays");
    rig.finish();
}

/// M2.8-8: a commit that fails while stopped is unsaved, and stays so (its
/// value live) once the project plays: a new run plays the file, and the
/// host plays the unsaved value again in it.
fn a_failed_stopped_commit_is_still_unsaved_when_playing(transport: Transport) {
    let mut rig = app_rig("unsaved", transport, None);
    let (mut ws, _) = rig.client();
    let Some(read_only) = common::ReadOnly::new(&rig.dir) else {
        return rig.finish();
    };
    let refused = rig.request(
        &mut ws,
        1,
        json!({"type": "commit", "address": "/out.gain", "value": 0.125}),
        "error",
    );
    assert_eq!(refused["code"], "write-failed");
    let unsaved = rig.unsaved(&mut ws, "listed while stopped", |u| u["count"] == 1);
    assert_eq!(unsaved["parameters"][0]["address"], "/out.gain");
    rig.request(&mut ws, 2, json!({"type": "transport.play"}), "transport");
    let described = rig.request(&mut ws, 3, json!({"type": "describe"}), "describe");
    assert_eq!(described["playing"], true);
    assert_eq!(described["unsaved"]["count"], 1, "still unsaved");
    assert_eq!(
        described["unsaved"]["parameters"][0]["error"],
        refused["message"]
    );
    let sub = rig.watch_parameter(&mut ws, 4, "/out.gain");
    rig.wait_base(&mut ws, sub, 0.125);
    drop(read_only);
    rig.request(
        &mut ws,
        5,
        json!({"type": "retry", "address": "/out.gain"}),
        "committed",
    );
    rig.unsaved(&mut ws, "saved", |u| u["count"] == 0);
    rig.finish();
}

both!(
    a_failed_stopped_commit_is_still_unsaved_when_playing,
    it_starts_stopped_plays_and_stops,
    a_project_that_does_not_load_shows_its_diagnostics,
    opening_a_project_stops_records_it_and_serves_its_panels,
    apply_takes_the_wake_policy_and_plays_again,
    a_chosen_device_is_found_again_by_identity,
    a_commit_while_stopped_writes_once,
    a_stopped_commit_updates_the_inner_bases,
    signal_ui_refuses_the_app_s_messages,
);
