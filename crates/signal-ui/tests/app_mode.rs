//! M2.8-4: the host in app mode (Signal.app's server): it starts stopped,
//! plays and stops on `transport.*` with the Setup, pushes the status line,
//! opens another project (and serves its panels), keeps a device chosen by
//! identity, takes the wake policy, and commits while stopped. `signal ui`
//! (CLI mode) refuses the app's messages. M2.8-12: .signal project files
//! (open, edited, Save, Save As, close) and `signal ui`'s reopen.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{Options, Rig, Transport, copy_dir, devices, rig, root};
use serde_json::{Value, json};
use signal_core::Registry;
use signal_engine::RtPolicy;
use signal_project::archive;
use signal_ui::player::Backend;
use signal_ui::setup::Setup;
use signal_ui::{AppHostConfig, Edits, Host, ServerOptions};

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
        open_root: base.join("Open"),
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

/// M2.8-11: the power switch. Off stops the run (the project stays open),
/// refuses Play and is never written to the Setup; on plays again if it
/// played, and stays stopped if it did not.
fn power_off_stops_and_on_restores(transport: Transport) {
    let mut rig = app_rig("power", transport, None);
    let (mut ws, _) = rig.client();
    let setup_before = fs::read(&rig.setup).ok();
    let app = rig.request(&mut ws, 1, json!({"type": "app.get"}), "app");
    assert_eq!(app["power"], "on", "Signal.app starts switched on");
    rig.request(&mut ws, 2, json!({"type": "transport.play"}), "transport");

    let off = rig.request(
        &mut ws,
        3,
        json!({"type": "app.power", "on": false}),
        "power",
    );
    assert_eq!(
        (off["on"].clone(), off["playing"].clone()),
        (json!(false), json!(false))
    );
    assert!(rig.host().controller().is_none(), "no run while off");
    assert!(!rig.host().powered());
    let app = rig.request(&mut ws, 4, json!({"type": "app.get"}), "app");
    assert_eq!(app["power"], "off");
    assert_eq!(app["project"]["dir"], rig.dir.display().to_string());
    let refused = rig.request(&mut ws, 5, json!({"type": "transport.play"}), "error");
    assert_eq!(refused["code"], "powered-off");
    let described = rig.request(&mut ws, 6, json!({"type": "describe"}), "describe");
    assert!(
        !described["controls"].as_array().unwrap().is_empty(),
        "the project stays open"
    );

    let on = rig.request(
        &mut ws,
        7,
        json!({"type": "app.power", "on": true}),
        "power",
    );
    assert_eq!(
        (on["on"].clone(), on["playing"].clone()),
        (json!(true), json!(true)),
        "it played"
    );
    rig.request(&mut ws, 8, json!({"type": "transport.stop"}), "transport");
    rig.request(
        &mut ws,
        9,
        json!({"type": "app.power", "on": false}),
        "power",
    );
    let on = rig.request(
        &mut ws,
        10,
        json!({"type": "app.power", "on": true}),
        "power",
    );
    assert_eq!(on["playing"], false, "stopped stays stopped");
    assert_eq!(
        fs::read(&rig.setup).ok(),
        setup_before,
        "the power is never written"
    );
    rig.finish();
}

/// auditor-1-power-initial-state: a client that connects while the app is
/// off (the panel's frame, reloaded because another project was opened) is
/// told so in `welcome`; no subscription is taken while off and no stream
/// frame reaches any client.
fn a_client_connecting_while_off_is_told_and_gets_no_frames(transport: Transport) {
    let mut rig = app_rig("power-welcome", transport, None);
    let kick = rig.dir.parent().unwrap().join("kick");
    copy_dir(&root().join("projects/kick"), &kick);
    let (mut a, _) = rig.client();
    rig.request(&mut a, 1, json!({"type": "transport.play"}), "transport");
    let sub = rig.watch_parameter(&mut a, 2, "/out.gain");
    rig.wait(&mut a, "a parameters frame", |got| got.frame(3, sub));

    rig.request(
        &mut a,
        3,
        json!({"type": "app.power", "on": false}),
        "power",
    );
    let opened = rig.request(
        &mut a,
        4,
        json!({"type": "project.open", "path": kick.display().to_string()}),
        "project",
    );
    assert_eq!(opened["loads"], true);

    let token = rig.host().token().to_owned();
    let mut b = common::connect(rig.host());
    common::send(
        &mut b,
        json!({"type": "hello", "protocol": 1, "token": token}),
    );
    let welcome = rig.wait(&mut b, "welcome", |got| got.of("welcome").cloned());
    assert_eq!(welcome["power"], "off", "told before it can subscribe");
    let refused = rig.request(
        &mut b,
        1,
        json!({"type": "subscribe", "stream": "parameters", "addresses": ["/out.gain"]}),
        "error",
    );
    assert_eq!(refused["code"], "powered-off");

    let until = std::time::Instant::now() + std::time::Duration::from_millis(300);
    while std::time::Instant::now() < until {
        rig.pump();
        for ws in [&mut a, &mut b] {
            while let Some(got) = common::receive(ws) {
                assert!(
                    !matches!(got, common::Got::Frame(_)),
                    "a stream frame while off"
                );
            }
        }
    }
    let on = rig.request(&mut b, 2, json!({"type": "app.power", "on": true}), "power");
    assert_eq!(
        on["playing"], false,
        "the project opened while off is stopped"
    );
    rig.request(
        &mut b,
        3,
        json!({"type": "subscribe", "stream": "parameters", "addresses": ["/out.gain"]}),
        "subscribed",
    );
    rig.finish();
    let _ = fs::remove_dir_all(&kick);
}

/// M2.8-12: `rig`'s folder packed into `<base>/<name>.signal`.
fn packed(rig: &Rig, name: &str) -> PathBuf {
    let file = rig.setup.with_file_name(format!("{name}.signal"));
    archive::write_atomic(&file, &archive::pack(&rig.dir).unwrap()).unwrap();
    file
}

/// The out.gain written in the project.toml of the folder `dir`.
fn gain_in(dir: &Path) -> String {
    let text = fs::read_to_string(dir.join("project.toml")).unwrap();
    text.lines()
        .find(|l| l.starts_with("out ="))
        .unwrap()
        .to_owned()
}

/// The out.gain the `.signal` file `file` holds.
fn gain_of(file: &Path) -> String {
    let dir = file.with_extension("check");
    let _ = fs::remove_dir_all(&dir);
    archive::unpack(&fs::read(file).unwrap(), &dir).unwrap();
    let gain = gain_in(&dir);
    fs::remove_dir_all(&dir).unwrap();
    gain
}

fn commit(rig: &mut Rig, ws: &mut common::Ws, id: u64, value: f64) {
    rig.request(
        ws,
        id,
        json!({"type": "commit", "address": "/out.gain", "value": value}),
        "committed",
    );
}

/// M2.8-12 AC13, AC15-AC18: a .signal file opens into a working folder
/// under the Open folder, the Setup records the file, a commit makes it
/// edited and Save writes it back; opening another project while edited is
/// refused until the edits are let go, which removes the working folder.
fn a_signal_file_opens_in_a_working_folder(transport: Transport) {
    let mut rig = app_rig("signal-open", transport, None);
    let file = packed(&rig, "tone");
    let open_root = rig.setup.with_file_name("Open");
    let (mut ws, _) = rig.client();
    let opened = rig.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": file.display().to_string()}),
        "project",
    );
    assert_eq!(opened["loads"], true, "{opened}");
    assert_eq!(opened["file"], file.display().to_string());
    let working = PathBuf::from(opened["dir"].as_str().unwrap());
    assert!(working.starts_with(&open_root), "{}", working.display());
    assert!(working.join("project.toml").is_file());
    assert_eq!(Setup::load(&rig.setup).0.last_project, Some(file.clone()));
    let app = rig.request(&mut ws, 2, json!({"type": "app.get"}), "app");
    assert_eq!(app["project"]["file"], file.display().to_string());
    assert_eq!(app["project"]["name"], "tone");
    assert_eq!(app["project"]["edited"], false);
    assert_eq!(app["project"]["sampleRate"], 48_000);

    // A live edit is written to the working folder: the project is edited.
    commit(&mut rig, &mut ws, 3, 0.25);
    assert!(gain_in(&working).contains("gain = 0.25"));
    let status = rig.host().project_status().unwrap();
    assert!(status.edited);
    assert_eq!(status.file.as_deref(), Some(file.as_path()));
    let edited = rig.wait(&mut ws, "edited", |got| {
        got.of("app")
            .filter(|a| a["project"]["edited"] == true)
            .cloned()
    });
    assert_eq!(edited["project"]["name"], "tone");
    assert!(gain_of(&file).contains("gain = 0.5"), "not saved yet");

    // Save writes the archive back and clears edited.
    let saved = rig.request(&mut ws, 4, json!({"type": "project.save"}), "saved");
    assert_eq!(saved["file"], file.display().to_string());
    assert!(gain_of(&file).contains("gain = 0.25"));
    assert!(!rig.host().project_status().unwrap().edited);
    let leftovers: Vec<_> = fs::read_dir(file.parent().unwrap())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "the write is atomic, no temporary left"
    );

    // Edited again: another project is not opened until asked.
    commit(&mut rig, &mut ws, 5, 0.125);
    let folder = rig.dir.display().to_string();
    let refused = rig.request(
        &mut ws,
        6,
        json!({"type": "project.open", "path": folder}),
        "error",
    );
    assert_eq!(refused["code"], "edited");
    assert_eq!(refused["message"], "Save changes to tone?");
    assert!(working.is_dir(), "nothing was let go");
    let opened = rig.request(
        &mut ws,
        7,
        json!({"type": "project.open", "path": folder, "unsaved": "discard"}),
        "project",
    );
    assert_eq!(opened["file"], Value::Null);
    assert!(
        !working.exists(),
        "the working folder goes with its project"
    );
    assert!(
        gain_of(&file).contains("gain = 0.25"),
        "Don't Save left the file"
    );
    rig.finish();
}

/// AC20: Save in the prompt writes the edits before the other project opens.
fn opening_another_project_can_save_first(transport: Transport) {
    let mut rig = app_rig("signal-save-first", transport, None);
    let file = packed(&rig, "tone");
    let (mut ws, _) = rig.client();
    let opened = rig.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": file.display().to_string()}),
        "project",
    );
    let working = PathBuf::from(opened["dir"].as_str().unwrap());
    commit(&mut rig, &mut ws, 2, 0.75);
    let folder = rig.dir.clone();
    rig.host().open_project(&folder, Edits::Save).unwrap();
    assert!(gain_of(&file).contains("gain = 0.75"));
    assert!(!working.exists());
    assert_eq!(Setup::load(&rig.setup).0.last_project, Some(folder));
    rig.finish();
}

/// auditor-1-reopen-save-stale (AC20): reopening the open .signal file with
/// Save opens what was saved, so the next Save keeps the edit.
fn reopening_the_same_file_with_save_keeps_the_edit(transport: Transport) {
    let mut rig = app_rig("signal-reopen-save", transport, None);
    let file = packed(&rig, "tone");
    let (mut ws, _) = rig.client();
    rig.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": file.display().to_string()}),
        "project",
    );
    commit(&mut rig, &mut ws, 2, 0.75);
    rig.host().open_project(&file, Edits::Save).unwrap();
    let status = rig.host().project_status().unwrap();
    assert_eq!(status.file.as_deref(), Some(file.as_path()));
    assert!(!status.edited);
    assert!(
        gain_in(&status.dir).contains("gain = 0.75"),
        "the reopened copy has the edit"
    );
    assert!(gain_of(&file).contains("gain = 0.75"));
    rig.host().save_document().unwrap();
    assert!(
        gain_of(&file).contains("gain = 0.75"),
        "the next Save keeps it"
    );
    assert!(gain_in(&status.dir).contains("gain = 0.75"));
    rig.finish();
}

/// AC4-AC7 in the app: a refused archive opens nothing (the open project
/// stays, no working folder is left) and the reason is named.
fn a_refused_archive_opens_nothing(transport: Transport) {
    let mut rig = app_rig("signal-refused", transport, None);
    let open_root = rig.setup.with_file_name("Open");
    let bad = rig.setup.with_file_name("bad.signal");
    fs::write(&bad, "not a zip").unwrap();
    let (mut ws, _) = rig.client();
    let refused = rig.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": bad.display().to_string()}),
        "error",
    );
    assert_eq!(refused["code"], "archive");
    let message = refused["message"].as_str().unwrap();
    assert!(
        message.contains("bad.signal was not opened: not a .signal archive"),
        "{message}"
    );
    let app = rig.request(&mut ws, 2, json!({"type": "app.get"}), "app");
    assert_eq!(
        app["project"]["dir"],
        rig.dir.display().to_string(),
        "the project stays"
    );
    let made = fs::read_dir(&open_root).map_or(0, |d| d.count());
    assert_eq!(made, 0, "no working folder is left");
    assert_eq!(
        Setup::load(&rig.setup).0.last_project,
        None,
        "never the Setup's"
    );
    rig.finish();
}

/// AC19: Save As of a folder project writes the file and opens it from
/// then on; the folder is left as it is. Save then writes the file, and
/// Save As of a .signal project moves it to the new file.
fn save_as_writes_the_file_and_leaves_the_folder(transport: Transport) {
    let mut rig = app_rig("signal-save-as", transport, None);
    let folder = rig.dir.clone();
    let before = fs::read(folder.join("project.toml")).unwrap();
    let file = rig.setup.with_file_name("tone.signal");
    let (mut ws, _) = rig.client();
    rig.host().save_as(&file).unwrap();
    let status = rig.host().project_status().unwrap();
    assert_eq!(status.file.as_deref(), Some(file.as_path()));
    assert!(!status.edited);
    assert_ne!(status.dir, folder, "a working folder of its own");
    assert_eq!(Setup::load(&rig.setup).0.last_project, Some(file.clone()));

    commit(&mut rig, &mut ws, 1, 0.25);
    assert!(rig.host().project_status().unwrap().edited);
    rig.host().save_document().unwrap();
    assert!(
        gain_of(&file).contains("gain = 0.25"),
        "the next Save writes there"
    );
    assert_eq!(
        fs::read(folder.join("project.toml")).unwrap(),
        before,
        "the folder is left as it was"
    );

    let copy = rig.setup.with_file_name("copy.signal");
    commit(&mut rig, &mut ws, 2, 0.125);
    rig.host().save_as(&copy).unwrap();
    let status = rig.host().project_status().unwrap();
    assert_eq!(status.file.as_deref(), Some(copy.as_path()));
    assert!(!status.edited);
    assert!(gain_of(&copy).contains("gain = 0.125"));
    assert!(
        gain_of(&file).contains("gain = 0.25"),
        "the old file is left"
    );
    assert_eq!(Setup::load(&rig.setup).0.last_project, Some(copy));
    rig.finish();
}

/// AC15, AC20: closing removes the working folder unless it is edited and
/// the edits were not let go (an unanswered prompt): then it is kept.
fn closing_keeps_an_edited_working_folder(transport: Transport) {
    for discard in [false, true] {
        let mut rig = app_rig(&format!("signal-close-{discard}"), transport, None);
        let file = packed(&rig, "tone");
        let (mut ws, _) = rig.client();
        let opened = rig.request(
            &mut ws,
            1,
            json!({"type": "project.open", "path": file.display().to_string()}),
            "project",
        );
        let working = PathBuf::from(opened["dir"].as_str().unwrap());
        commit(&mut rig, &mut ws, 2, 0.25);
        let messages = rig.host().close(None, discard);
        assert_eq!(working.exists(), !discard, "discard {discard}");
        if !discard {
            assert!(
                messages
                    .iter()
                    .any(|m| m.contains("unsaved edits of tone are kept")),
                "{messages:?}"
            );
            assert!(
                gain_in(&working).contains("gain = 0.25"),
                "the edits are there"
            );
        }
        rig.finish();
    }
    // Not edited: closing removes it.
    let mut rig = app_rig("signal-close-clean", transport, None);
    let file = packed(&rig, "tone");
    rig.host().open_project(&file, Edits::Ask).unwrap();
    let working = rig.host().project_status().unwrap().dir;
    assert!(working.is_dir());
    rig.host().close(None, false);
    assert!(!working.exists());
    rig.finish();
}

/// M2.8-12 AC22 (auditor-1-browser-open): `signal ui` started from a
/// .signal file reopens it from the file when its Setup field names it,
/// and refuses every other path, as the security note requires.
fn signal_ui_reopens_only_the_project_it_serves(transport: Transport) {
    let mut rig = rig(
        "reopen",
        Options {
            transport,
            packed: true,
            ..Options::default()
        },
    );
    let source = rig.setup.with_file_name("project.signal");
    let first = rig.dir.clone();
    let (mut ws, _) = rig.client();
    commit(&mut rig, &mut ws, 1, 0.25);
    assert!(
        gain_in(&first).contains("gain = 0.25"),
        "edited in the temporary copy"
    );
    let reopened = rig.request(
        &mut ws,
        2,
        json!({"type": "project.open", "path": source.display().to_string()}),
        "project",
    );
    assert_eq!(reopened["loads"], true, "{reopened}");
    assert_eq!(reopened["file"], source.display().to_string());
    let again = PathBuf::from(reopened["dir"].as_str().unwrap());
    assert_ne!(again, first, "a fresh unpack");
    assert!(!first.exists(), "the previous copy is removed");
    assert!(
        gain_in(&again).contains("gain = 0.5"),
        "the file's value again"
    );
    let described = rig.request(&mut ws, 3, json!({"type": "describe"}), "describe");
    assert_eq!(described["playing"], true, "playing again");

    let other = rig.setup.with_file_name("other.signal");
    fs::copy(&source, &other).unwrap();
    for (id, path) in [(4, other.display().to_string()), (5, "/tmp".to_owned())] {
        let refused = rig.request(
            &mut ws,
            id,
            json!({"type": "project.open", "path": path}),
            "error",
        );
        assert_eq!(refused["code"], "not-allowed", "{path}");
    }
    rig.finish();

    let mut recording = common::rig(
        "reopen-recording",
        Options {
            transport,
            packed: true,
            recording: true,
            ..Options::default()
        },
    );
    let source = recording.setup.with_file_name("project.signal");
    let (mut ws, _) = recording.client();
    let refused = recording.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": source.display().to_string()}),
        "error",
    );
    assert_eq!(refused["code"], "recording");
    recording.finish();
}

/// Second-opinion review of M2.8-12: a folder named `*.signal` is opened as
/// the folder it is, not read as an archive.
fn a_folder_named_like_a_signal_file_opens_as_a_folder(transport: Transport) {
    let mut rig = app_rig("dotsignal", transport, None);
    let folder = rig.dir.parent().unwrap().join("kick.signal");
    copy_dir(&root().join("projects/kick"), &folder);
    let (mut ws, _) = rig.client();
    let opened = rig.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": folder.display().to_string()}),
        "project",
    );
    assert_eq!(opened["loads"], true, "{opened}");
    assert_eq!(
        opened["dir"],
        folder.display().to_string(),
        "the folder itself, no working copy"
    );
    rig.finish();
    let _ = fs::remove_dir_all(&folder);
}

both!(
    power_off_stops_and_on_restores,
    a_client_connecting_while_off_is_told_and_gets_no_frames,
    a_failed_stopped_commit_is_still_unsaved_when_playing,
    it_starts_stopped_plays_and_stops,
    a_project_that_does_not_load_shows_its_diagnostics,
    opening_a_project_stops_records_it_and_serves_its_panels,
    apply_takes_the_wake_policy_and_plays_again,
    a_chosen_device_is_found_again_by_identity,
    a_commit_while_stopped_writes_once,
    a_stopped_commit_updates_the_inner_bases,
    signal_ui_refuses_the_app_s_messages,
    a_signal_file_opens_in_a_working_folder,
    opening_another_project_can_save_first,
    reopening_the_same_file_with_save_keeps_the_edit,
    a_refused_archive_opens_nothing,
    save_as_writes_the_file_and_leaves_the_folder,
    closing_keeps_an_edited_working_folder,
    signal_ui_reopens_only_the_project_it_serves,
    a_folder_named_like_a_signal_file_opens_as_a_folder,
);
