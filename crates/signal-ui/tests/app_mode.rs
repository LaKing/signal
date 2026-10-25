//! M2.8-4: the host in app mode (Signal.app's server): it starts stopped,
//! plays and stops on `transport.*` with the Setup, pushes the status line,
//! opens another project (and serves its panels), keeps a device chosen by
//! identity, takes the wake policy, and commits while stopped. `signal ui`
//! (CLI mode) refuses the app's messages. M2.8-12: .signal project files
//! (open, edited, Save, Save As, Revert to Saved, close) and `signal ui`'s
//! reopen.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{Options, Rig, Transport, copy_dir, devices, rig, root};
use serde_json::{Value, json};
use signal_audio::DeviceInfo;
use signal_core::Registry;
use signal_engine::RtPolicy;
use signal_project::archive;
use signal_ui::player::Backend;
use signal_ui::setup::Setup;
use signal_ui::{AppHostConfig, Edits, Host, ServerOptions};

/// A host in app mode on the null driver, its folder holding the tone
/// project (or a copy of `project`).
fn app_rig(name: &str, transport: Transport, project: Option<&Path>) -> Rig {
    app_rig_with(name, transport, project, false)
}

/// [`app_rig`], playing whenever switched on (`plays`, Signal.app's: ADR
/// 0033) or starting stopped.
fn app_rig_with(name: &str, transport: Transport, project: Option<&Path>, plays: bool) -> Rig {
    app_rig_listing(name, transport, project, plays, devices())
}

/// [`app_rig_with`] on the null backend listing `listed`.
fn app_rig_listing(
    name: &str,
    transport: Transport,
    project: Option<&Path>,
    plays: bool,
    listed: Vec<DeviceInfo>,
) -> Rig {
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
        backend: Backend::Null(listed),
        compute: vec!["CPU: test".to_owned()],
        server: ServerOptions {
            port: 0,
            remote: false,
            ui_dir: root().join("ui"),
            project_ui: PathBuf::new(),
            client_queue: 256,
        },
        plays,
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
        "switch the power off to refresh: listing devices pauses metering and status"
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
    for (id, kind) in [(1, "project.open"), (2, "project.revert"), (3, "app.get")] {
        let refused = rig.request(&mut ws, id, json!({"type": kind, "path": "/tmp"}), "error");
        assert_eq!(refused["code"], "not-allowed", "{kind}");
    }
    let described = rig.request(&mut ws, 4, json!({"type": "describe"}), "describe");
    assert_eq!(described["playing"], true, "signal ui still plays");
    rig.finish();
}

/// M2.12-1 (ADR 0029 §2): `signal ui` stops on `transport.stop` and plays
/// the project it serves again from frame 0 on `transport.play`, `describe`
/// pushed after each; a run that records refuses both.
fn signal_ui_stops_and_plays(transport: Transport) {
    let mut rig = rig(
        "ui-transport",
        Options {
            transport,
            ..Options::default()
        },
    );
    let (mut ws, _) = rig.client();
    for _ in 0..8 {
        rig.pump();
    }
    let stopped = rig.request(&mut ws, 1, json!({"type": "transport.stop"}), "transport");
    assert_eq!(stopped["playing"], false);
    let described = rig.request(&mut ws, 2, json!({"type": "describe"}), "describe");
    assert_eq!(described["playing"], false, "{described}");
    let pong = rig.request(&mut ws, 3, json!({"type": "ping"}), "pong");
    assert_eq!(pong["frame"], Value::Null, "nothing runs");
    let played = rig.request(&mut ws, 4, json!({"type": "transport.play"}), "transport");
    assert_eq!(played["playing"], true);
    let pong = rig.request(&mut ws, 5, json!({"type": "ping"}), "pong");
    assert!(
        pong["frame"].as_u64().is_some_and(|f| f < 48_000),
        "played again from frame 0: {pong}"
    );
    rig.finish();

    let mut rig = rig_recording("ui-transport-recording", transport);
    let (mut ws, _) = rig.client();
    for (id, kind) in [(1, "transport.stop"), (2, "transport.play")] {
        let refused = rig.request(&mut ws, id, json!({"type": kind}), "error");
        assert_eq!(refused["code"], "recording", "{kind}");
    }
    let described = rig.request(&mut ws, 3, json!({"type": "describe"}), "describe");
    assert_eq!(described["playing"], true, "a recording run plays on");
    rig.finish();
}

fn rig_recording(name: &str, transport: Transport) -> Rig {
    rig(
        name,
        Options {
            transport,
            recording: true,
            ..Options::default()
        },
    )
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

/// A sine through a `std.scale` (as kick909's `attack` reaches its click):
/// a `mul` of −1e5 (the end of its range, ADR 0043) on a sine scaled by 20
/// takes the scale's output beyond ±10⁶.
const SCALED: &str = r#"schema_version = 1

[project]
name = "scaled"
sample_rate = 48000
seed = 1

[instances]
osc = { module = "std.sine_osc", freq = 440 }
click = { module = "std.scale", mul = 0.5 }
out = { module = "std.audio_out", gain = 0.25 }

[wiring]
connections = [
  { from = "osc.out", to = "click.in", scale = 20 },
  { from = "click.out", to = "out.l" },
  { from = "click.out", to = "out.r" },
]
"#;

/// The owner's kick (2026-10-25): a control dragged to its range's end took
/// an instance's output beyond ±10⁶, the fault guard muted it for the run,
/// and setting the control back (a snapshot's restore) showed the value but
/// brought no sound, with nothing saying why. The window now says which
/// instance is muted, why and how it comes back, for as long as the run
/// lasts; playing again clears it.
fn a_muted_instance_is_shown_until_the_project_plays_again(transport: Transport) {
    let source = std::env::temp_dir().join(format!(
        "signal-ui-app-muted-source-{transport:?}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&source);
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("project.toml"), SCALED).unwrap();
    let mut rig = app_rig("muted", transport, Some(&source));
    let (mut ws, _) = rig.client();
    rig.request(&mut ws, 1, json!({"type": "transport.play"}), "transport");
    let app = rig.request(&mut ws, 2, json!({"type": "app.get"}), "app");
    assert_eq!(app["muted"], Value::Null, "nothing muted yet: {app}");

    let set = |value: f64| json!({"type": "commit", "address": "/click.mul", "value": value});
    rig.request(&mut ws, 3, set(-1e5), "committed");
    let muted = rig.wait(&mut ws, "muted", |got| {
        got.of("app")
            .and_then(|a| a["muted"].as_str())
            .map(str::to_owned)
    });
    assert!(
        muted.starts_with("/click was muted at frame ") && muted.contains("beyond ±10⁶"),
        "{muted}"
    );
    // Back in range: the control takes the value, the instance stays muted,
    // and the window still says so.
    rig.request(&mut ws, 4, set(0.5), "committed");
    let app = rig.request(&mut ws, 5, json!({"type": "app.get"}), "app");
    assert_eq!(app["muted"], json!(muted), "still muted: {app}");
    assert_eq!(
        app["mutedInstances"],
        json!(["/click"]),
        "for the schematic: {app}"
    );

    rig.request(&mut ws, 6, json!({"type": "transport.stop"}), "transport");
    rig.request(&mut ws, 7, json!({"type": "transport.play"}), "transport");
    let app = rig.request(&mut ws, 8, json!({"type": "app.get"}), "app");
    assert_eq!(app["muted"], Value::Null, "a new run: {app}");
    assert_eq!(app["mutedInstances"], json!([]), "{app}");

    // Muted again (and the file set back), then the power off: no run, no
    // note; on, a new run from the file plays it.
    let mute = |rig: &mut Rig, ws: &mut common::Ws, id: u64| {
        rig.request(ws, id, set(-1e5), "committed");
        rig.wait(ws, "muted again", |got| {
            got.of("app").and_then(|a| a["muted"].as_str()).map(drop)
        });
        rig.request(ws, id + 1, set(0.5), "committed");
    };
    mute(&mut rig, &mut ws, 9);
    rig.request(
        &mut ws,
        11,
        json!({"type": "app.power", "on": false}),
        "power",
    );
    let app = rig.request(&mut ws, 12, json!({"type": "app.get"}), "app");
    assert_eq!(app["muted"], Value::Null, "switched off: {app}");
    let on = rig.request(
        &mut ws,
        13,
        json!({"type": "app.power", "on": true}),
        "power",
    );
    assert_eq!(on["playing"], true);
    let app = rig.request(&mut ws, 14, json!({"type": "app.get"}), "app");
    assert_eq!(app["muted"], Value::Null, "switched on again: {app}");

    // A swap that removes the muted instance (renamed by another writer)
    // brings a new, unfaulted one: the note goes.
    mute(&mut rig, &mut ws, 15);
    let renamed = fs::read_to_string(rig.dir.join("project.toml"))
        .unwrap()
        .replace("click", "clack");
    fs::write(rig.dir.join("project.toml"), renamed).unwrap();
    rig.wait(&mut ws, "the note gone after the swap", |got| {
        got.of("app").filter(|a| a["muted"].is_null()).map(drop)
    });

    // Opening a project (this one again) ends the run and the note.
    let clack = |value: f64| json!({"type": "commit", "address": "/clack.mul", "value": value});
    rig.request(&mut ws, 17, clack(-1e5), "committed");
    rig.wait(&mut ws, "muted once more", |got| {
        got.of("app").and_then(|a| a["muted"].as_str()).map(drop)
    });
    rig.request(&mut ws, 18, clack(0.5), "committed");
    let dir = rig.dir.display().to_string();
    rig.request(
        &mut ws,
        19,
        json!({"type": "project.open", "path": dir}),
        "project",
    );
    let app = rig.request(&mut ws, 20, json!({"type": "app.get"}), "app");
    assert_eq!(app["muted"], Value::Null, "opened: {app}");
    rig.finish();
    let _ = fs::remove_dir_all(&source);
}

/// The note names the first muted instance, then up to 8 others and how
/// many more; a fault's reason chooses how it says the instance comes back.
fn the_muted_note_lists_eight_then_counts(transport: Transport) {
    let source = std::env::temp_dir().join(format!(
        "signal-ui-app-muted-many-source-{transport:?}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&source);
    fs::create_dir_all(&source).unwrap();
    // Eleven scales of one sine, each muted by the same `mul`.
    let mut text = String::from(
        "schema_version = 1\n\n[project]\nname = \"many\"\nsample_rate = 48000\nseed = 1\n\n[instances]\nosc = { module = \"std.sine_osc\", freq = 440 }\nk = { module = \"std.const\", value = 1 }\nout = { module = \"std.audio_out\", gain = 0.25 }\n",
    );
    let mut wires = String::new();
    for n in 0..11 {
        text.push_str(&format!(
            "s{n:02} = {{ module = \"std.scale\", mul = 0.5 }}\n"
        ));
        wires.push_str(&format!(
            "  {{ from = \"osc.out\", to = \"s{n:02}.in\", scale = 20 }},\n  {{ from = \"k.out\", to = \"s{n:02}.mul\" }},\n  {{ from = \"s{n:02}.out\", to = \"out.l\" }},\n"
        ));
    }
    text.push_str(&format!("\n[wiring]\nconnections = [\n{wires}]\n"));
    fs::write(source.join("project.toml"), text).unwrap();
    let mut rig = app_rig("muted-many", transport, Some(&source));
    let (mut ws, _) = rig.client();
    rig.request(&mut ws, 1, json!({"type": "transport.play"}), "transport");
    rig.request(
        &mut ws,
        2,
        json!({"type": "commit", "address": "/k.value", "value": 1e5}),
        "committed",
    );
    let muted = rig.wait(&mut ws, "eleven muted", |got| {
        got.of("app")
            .and_then(|a| a["muted"].as_str())
            .filter(|m| m.contains("and 2 more"))
            .map(str::to_owned)
    });
    assert!(muted.starts_with("/s00 was muted at frame "), "{muted}");
    assert!(muted.contains("with its values back in range"), "{muted}");
    assert!(
        muted.contains("(also muted: /s01, /s02, /s03, /s04, /s05, /s06, /s07, /s08 and 2 more)"),
        "{muted}"
    );
    rig.finish();
    let _ = fs::remove_dir_all(&source);
}

/// ADR 0042, 0043 (the operator's decision of 2026-10-25): a project whose
/// files hold a value outside its range or value set opens and plays, and
/// Signal.app lists the corrections as warnings beside its diagnostics.
fn corrected_values_are_listed_as_warnings(transport: Transport) {
    let source = std::env::temp_dir().join(format!(
        "signal-ui-app-corrected-source-{transport:?}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&source);
    fs::create_dir_all(&source).unwrap();
    fs::write(
        source.join("project.toml"),
        "schema_version = 1\n\n[project]\nname = \"corrected\"\nsample_rate = 48000\nseed = 1\n\n\
         [instances]\no = { module = \"std.osc\", freq = 250000, shape = 2.01 }\n\
         out = { module = \"std.audio_out\", gain = 0.25 }\n\n\
         [wiring]\nconnections = [{ from = \"o.out\", to = \"out.l\" }]\n",
    )
    .unwrap();
    let mut rig = app_rig("corrected", transport, Some(&source));
    let (mut ws, _) = rig.client();
    let played = rig.request(&mut ws, 1, json!({"type": "transport.play"}), "transport");
    assert_eq!(played["playing"], true, "it plays");
    let app = rig.request(&mut ws, 2, json!({"type": "app.get"}), "app");
    assert_eq!(app["project"]["diagnostics"], json!([]), "{app}");
    let warnings = app["project"]["warnings"].to_string();
    assert!(
        warnings.contains("warning: /o.freq: value 250000 is outside [0, 100000]: 100000 plays")
            && warnings.contains("warning: /o.shape: value 2.01 is not one of 0 sine, 1 saw, 2 square, 3 triangle: 2 plays"),
        "{warnings}"
    );
    rig.finish();
    let _ = fs::remove_dir_all(&source);
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

/// Steps until an `app` push says the project is edited.
fn wait_edited(rig: &mut Rig, ws: &mut common::Ws) {
    rig.wait(ws, "edited", |got| {
        got.of("app")
            .filter(|a| a["project"]["edited"] == true)
            .cloned()
    });
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

/// ADR 0042, 0043 (the operator's decision after the review of c89253a): a
/// `.signal` file whose only difference from what plays is values corrected
/// on load opens neither edited nor revertible (no prompt on open, quit or
/// Revert to Saved, which would only loop), shows the corrections in its
/// warnings with how many the next Save writes, and that Save writes them.
fn a_signal_file_with_corrected_values_is_not_edited(transport: Transport) {
    let mut rig = app_rig("signal-corrected", transport, None);
    let text = common::TONE.replace(
        r#"a = { module = "std.sine_osc", freq = 440 }"#,
        r#"a = { module = "std.osc", freq = 250000, shape = 1.0023 }"#,
    );
    fs::write(rig.dir.join("project.toml"), &text).unwrap();
    let file = packed(&rig, "tone");
    let (mut ws, _) = rig.client();
    let opened = rig.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": file.display().to_string()}),
        "project",
    );
    assert_eq!(opened["loads"], true, "{opened}");
    let working = PathBuf::from(opened["dir"].as_str().unwrap());
    let status = rig.host().project_status().unwrap();
    assert!(!status.edited, "corrections do not edit it");
    assert_eq!(status.unsaved, 0, "nor are they unsaved");
    assert!(!status.revertible(), "nothing to revert");
    let refused = rig.request(&mut ws, 2, json!({"type": "project.revert"}), "error");
    assert_eq!(refused["code"], "not-edited", "{refused}");
    let app = rig.request(&mut ws, 3, json!({"type": "app.get"}), "app");
    assert_eq!(app["project"]["edited"], false);
    let warnings: Vec<String> = app["project"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap().to_owned())
        .collect();
    assert!(
        warnings.iter().any(|w| w.contains(
            "warning: /a.shape: value 1.0023 is not one of 0 sine, 1 saw, 2 square, 3 triangle: 1 plays"
        )) && warnings
            .iter()
            .any(|w| w.contains("warning: /a.freq: value 250000 is outside [0, 100000]: 100000 plays")),
        "{warnings:?}"
    );
    assert_eq!(
        warnings.last().map(String::as_str),
        Some("2 values will be written corrected on the next Save"),
        "{warnings:?}"
    );
    let described = rig.request(&mut ws, 4, json!({"type": "describe"}), "describe");
    assert_eq!(described["unsaved"]["count"], 0, "{}", described["unsaved"]);
    let shape = described["controls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["address"] == "/a.shape")
        .unwrap();
    assert_eq!(shape["base"], 1.0, "what plays");

    // Save writes the corrected values with the rest.
    rig.request(&mut ws, 5, json!({"type": "project.save"}), "saved");
    let unpacked = file.with_extension("check");
    let _ = fs::remove_dir_all(&unpacked);
    archive::unpack(&fs::read(&file).unwrap(), &unpacked).unwrap();
    let saved = fs::read_to_string(unpacked.join("project.toml")).unwrap();
    assert!(
        saved.contains(r#"a = { module = "std.osc", freq = 100000, shape = 1 }"#),
        "{saved}"
    );
    assert!(
        fs::read_to_string(working.join("project.toml"))
            .unwrap()
            .contains("shape = 1 }")
    );
    let status = rig.host().project_status().unwrap();
    assert!(!status.edited && !status.revertible(), "{status:?}");
    let app = rig.request(&mut ws, 6, json!({"type": "app.get"}), "app");
    assert_eq!(app["project"]["warnings"], json!([]), "{app}");
    let _ = fs::remove_dir_all(&unpacked);
    rig.finish();
}

/// TONE with `a` a `std.osc` whose `freq` and `shape` play corrected.
fn corrected_tone() -> String {
    common::TONE.replace(
        r#"a = { module = "std.sine_osc", freq = 440 }"#,
        r#"a = { module = "std.osc", freq = 250000, shape = 1.0023 }"#,
    )
}

/// The `a` line of the project.toml in the `.signal` file `file`.
fn a_line_of(file: &Path) -> String {
    let dir = file.with_extension("check-a");
    let _ = fs::remove_dir_all(&dir);
    archive::unpack(&fs::read(file).unwrap(), &dir).unwrap();
    let text = fs::read_to_string(dir.join("project.toml")).unwrap();
    fs::remove_dir_all(&dir).unwrap();
    text.lines()
        .find(|l| l.starts_with("a ="))
        .unwrap()
        .to_owned()
}

/// Review of 5b54fc5, 1: a correction pending since the file was loaded is
/// written by Save only where the file still holds the value it was
/// corrected from, judged under the writer lock: a value another writer
/// put there meanwhile survives the Save; the other correction is written.
fn save_keeps_a_value_another_writer_changed(transport: Transport) {
    let mut rig = app_rig("signal-corrected-race", transport, None);
    fs::write(rig.dir.join("project.toml"), corrected_tone()).unwrap();
    let file = packed(&rig, "tone");
    let (mut ws, _) = rig.client();
    let opened = rig.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": file.display().to_string()}),
        "project",
    );
    let working = PathBuf::from(opened["dir"].as_str().unwrap());
    // Another writer, between the load and the Save (no step between, so
    // the host's panel still holds the correction of 1.0023).
    let path = working.join("project.toml");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, text.replace("shape = 1.0023", "shape = 3")).unwrap();
    rig.host().save_document().unwrap();
    assert_eq!(
        a_line_of(&file),
        r#"a = { module = "std.osc", freq = 100000, shape = 3 }"#,
        "the outside value survives; the freq correction is written"
    );
    rig.finish();
}

/// Review of 5b54fc5, 2: Save As of a folder project leaves the folder's
/// files as they are (they are the user's own); the new `.signal` file
/// opens with the corrections pending, for its own next Save.
fn save_as_of_a_folder_leaves_its_files(transport: Transport) {
    let source = std::env::temp_dir().join(format!(
        "signal-ui-app-saveas-corrected-source-{transport:?}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&source);
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("project.toml"), corrected_tone()).unwrap();
    let mut rig = app_rig("signal-saveas-corrected", transport, Some(&source));
    let before = fs::read_to_string(rig.dir.join("project.toml")).unwrap();
    let (mut ws, _) = rig.client();
    let file = rig.setup.with_file_name("copy.signal");
    rig.host().save_as(&file).unwrap();
    assert_eq!(
        fs::read_to_string(rig.dir.join("project.toml")).unwrap(),
        before,
        "the folder's project.toml is not written"
    );
    assert_eq!(
        a_line_of(&file),
        r#"a = { module = "std.osc", freq = 250000, shape = 1.0023 }"#
    );
    let app = rig.request(&mut ws, 1, json!({"type": "app.get"}), "app");
    assert_eq!(app["project"]["file"], file.display().to_string());
    let warnings = app["project"]["warnings"].to_string();
    assert!(
        warnings.contains("2 values will be written corrected on the next Save"),
        "{warnings}"
    );
    rig.finish();
    let _ = fs::remove_dir_all(&source);
}

/// Review of 5b54fc5, 4: when the corrected values cannot be written (the
/// working folder read-only), the Save still saves, the corrections stay
/// pending and counted, and the message says why they were not written.
fn a_failed_correction_does_not_block_the_save(transport: Transport) {
    let mut rig = app_rig("signal-corrected-fail", transport, None);
    fs::write(rig.dir.join("project.toml"), corrected_tone()).unwrap();
    let file = packed(&rig, "tone");
    let (mut ws, _) = rig.client();
    let opened = rig.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": file.display().to_string()}),
        "project",
    );
    let working = PathBuf::from(opened["dir"].as_str().unwrap());
    let Some(guard) = common::ReadOnly::new(&working) else {
        rig.finish();
        return;
    };
    rig.request(&mut ws, 2, json!({"type": "project.save"}), "saved");
    drop(guard);
    assert_eq!(
        a_line_of(&file),
        r#"a = { module = "std.osc", freq = 250000, shape = 1.0023 }"#
    );
    let app = rig.request(&mut ws, 3, json!({"type": "app.get"}), "app");
    let message = app["message"].as_str().unwrap_or_default().to_owned();
    assert!(
        message.starts_with("saved; the corrected values were not written: "),
        "{app}"
    );
    let warnings = app["project"]["warnings"].to_string();
    assert!(
        warnings.contains("2 values will be written corrected on the next Save"),
        "{warnings}"
    );
    rig.finish();
}

/// M2.9-2 (ADR 0026 §5): a `panel.write` is a file in a `.signal`
/// project's working folder, so the project becomes edited (here the first
/// write of its generated panel, `ui/main.html` created), and Save packs it.
fn a_panel_write_marks_a_signal_project_edited(transport: Transport) {
    let mut rig = app_rig("signal-panel-write", transport, None);
    let file = packed(&rig, "tone");
    let (mut ws, _) = rig.client();
    let opened = rig.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": file.display().to_string()}),
        "project",
    );
    let working = PathBuf::from(opened["dir"].as_str().unwrap());
    assert!(!rig.host().project_status().unwrap().edited);
    let read = rig.request(
        &mut ws,
        2,
        json!({"type": "panel.read", "file": "main.html"}),
        "panel.read",
    );
    assert_eq!(read["exists"], false);
    let html = "\n  <div class=\"signal-cards\">\n    <section class=\"signal-card\">\n      <h2>out</h2>\n      <div class=\"row\">\n        <signal-fader bind=\"/out.gain\" taper=\"db\" label=\"gain\"></signal-fader>\n      </div>\n    </section>\n  </div>\n";
    let written = rig.request(
        &mut ws,
        3,
        json!({"type": "panel.write", "file": "main.html", "html": html, "base": null}),
        "panel.written",
    );
    assert_eq!(written["created"], true);
    assert!(working.join("ui/main.html").is_file());
    assert!(
        rig.host().project_status().unwrap().edited,
        "the write edits the project"
    );
    rig.wait(&mut ws, "edited", |got| {
        got.of("app")
            .filter(|a| a["project"]["edited"] == true)
            .cloned()
    });
    rig.request(&mut ws, 4, json!({"type": "project.save"}), "saved");
    assert!(!rig.host().project_status().unwrap().edited);
    let unpacked = file.with_extension("check");
    archive::unpack(&fs::read(&file).unwrap(), &unpacked).unwrap();
    assert!(
        fs::read_to_string(unpacked.join("ui/main.html"))
            .unwrap()
            .contains(html),
        "saved with the panel"
    );
    rig.finish();
}

/// M2.11-1 (ADR 0028 §7): a `pattern.write` is a file in a `.signal`
/// project's working folder, so the project becomes edited (here the first
/// write of a pattern no instance names yet), and Save packs it.
fn a_pattern_write_marks_a_signal_project_edited(transport: Transport) {
    let mut rig = app_rig("signal-pattern-write", transport, None);
    let file = packed(&rig, "tone");
    let (mut ws, _) = rig.client();
    let opened = rig.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": file.display().to_string()}),
        "project",
    );
    let working = PathBuf::from(opened["dir"].as_str().unwrap());
    assert!(!rig.host().project_status().unwrap().edited);
    let text = "schema_version = 1\nkind = \"steps\"\nsteps = 4\nstep = 0.25\n\n[lanes]\nkick = \"x.x.\"\n";
    let written = rig.request(
        &mut ws,
        2,
        json!({"type": "pattern.write", "name": "beat", "text": text, "base": null}),
        "pattern.written",
    );
    assert_eq!(written["created"], true);
    assert!(working.join("patterns/beat.toml").is_file());
    assert!(
        rig.host().project_status().unwrap().edited,
        "the write edits the project"
    );
    rig.request(&mut ws, 3, json!({"type": "project.save"}), "saved");
    assert!(!rig.host().project_status().unwrap().edited);
    let unpacked = file.with_extension("check");
    archive::unpack(&fs::read(&file).unwrap(), &unpacked).unwrap();
    assert_eq!(
        fs::read_to_string(unpacked.join("patterns/beat.toml")).unwrap(),
        text,
        "saved with the pattern"
    );
    rig.finish();
}

/// M2.9-3: stopped, a patch writes the file and pushes `graph` at once (no
/// watch reload to wait for), with the instance in it.
fn a_stopped_patch_writes_and_pushes_graph(transport: Transport) {
    let mut rig = app_rig("stopped-patch", transport, None);
    let (mut ws, _) = rig.client();
    let graph = rig.request(&mut ws, 1, json!({"type": "graph"}), "graph");
    assert_eq!(graph["planVersion"], Value::Null);
    let base = graph["files"][""].as_str().unwrap().to_owned();
    // The push comes before the reply.
    common::send(
        &mut ws,
        json!({"type": "patch", "id": 2, "level": "", "base": base, "ops": [
            {"op": "instance.add", "name": "svf", "module": "std.svf", "x": 10, "y": 20},
            {"op": "connection.add", "from": "b.out", "to": "svf.in"}]}),
    );
    let pushed = rig.wait(&mut ws, "graph with svf", |got| {
        got.of("graph")
            .filter(|g| {
                g.get("id").is_none()
                    && g["top"]["instances"]
                        .as_array()
                        .is_some_and(|i| i.iter().any(|i| i["name"] == "svf"))
            })
            .cloned()
    });
    let patched = rig.reply(&mut ws, 2, "patched");
    let text = fs::read_to_string(rig.dir.join("project.toml")).unwrap();
    assert!(text.contains("svf = { module = \"std.svf\" }\n"), "{text}");
    assert_eq!(
        patched["base"],
        signal_ui::panel_write::sha256_hex(text.as_bytes())
    );
    assert_eq!(
        pushed["layout"]["positions"][""]["svf"],
        json!({"x": 10, "y": 20})
    );
    assert_eq!(pushed["files"][""], patched["base"]);
    assert_eq!(pushed["planVersion"], Value::Null, "still stopped");
    rig.finish();
}

/// M2.9-3: a patch edits a `.signal` project's working folder: edited.
fn a_patch_marks_a_signal_project_edited(transport: Transport) {
    let mut rig = app_rig("signal-patch", transport, None);
    let file = packed(&rig, "tone");
    let (mut ws, _) = rig.client();
    rig.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": file.display().to_string()}),
        "project",
    );
    assert!(!rig.host().project_status().unwrap().edited);
    let graph = rig.request(&mut ws, 2, json!({"type": "graph"}), "graph");
    rig.request(
        &mut ws,
        3,
        json!({"type": "patch", "level": "", "base": graph["files"][""], "ops": [{"op": "instance.remove", "name": "env"}]}),
        "patched",
    );
    assert!(
        rig.host().project_status().unwrap().edited,
        "the patch edits the project"
    );
    rig.wait(&mut ws, "edited", |got| {
        got.of("app")
            .filter(|a| a["project"]["edited"] == true)
            .cloned()
    });
    rig.finish();
}

/// Audit P2 (document-check-stall): the control thread never waits for the
/// display's comparison of a .signal project. While a comparison's result
/// is held (as a large project's would take long), steps go on: requests
/// are answered and the edit shows at once; released, its result counts.
fn steps_never_wait_for_a_document_comparison(transport: Transport) {
    let mut rig = app_rig("signal-held", transport, None);
    let file = packed(&rig, "tone");
    let (mut ws, _) = rig.client();
    rig.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": file.display().to_string()}),
        "project",
    );
    rig.host().document().unwrap().hold_comparisons(true);
    commit(&mut rig, &mut ws, 2, 0.25);
    rig.wait(&mut ws, "edited", |got| {
        got.of("app")
            .filter(|a| a["project"]["edited"] == true)
            .cloned()
    });
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while rig.host().document().unwrap().comparisons_held() == 0 {
        assert!(std::time::Instant::now() < until, "no comparison started");
        rig.pump();
    }
    for id in 3..13 {
        let app = rig.request(&mut ws, id, json!({"type": "app.get"}), "app");
        assert_eq!(app["project"]["edited"], true);
    }
    let document = rig.host().document().unwrap();
    assert_eq!(document.comparisons_held(), 1, "no step waited for it");
    assert!(document.comparing());
    document.hold_comparisons(false);
    while rig.host().document().unwrap().comparing() {
        assert!(std::time::Instant::now() < until, "no result");
        rig.pump();
    }
    assert!(rig.host().project_shown().unwrap().edited);
    rig.request(&mut ws, 13, json!({"type": "project.save"}), "saved");
    assert!(!rig.host().project_status().unwrap().edited);
    rig.finish();
}

/// Audit (save-blocks-control): `project.save` packs and writes on a thread
/// of its own; while it is held (as a large project's would take long),
/// steps go on: other requests are answered and a commit is taken, and
/// `saved` comes when it is written. A Save while one runs is answered after
/// it, with the commit made meanwhile written.
fn steps_never_wait_for_a_save(transport: Transport) {
    let mut rig = app_rig("signal-save-held", transport, None);
    let file = packed(&rig, "tone");
    let (mut ws, _) = rig.client();
    rig.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": file.display().to_string()}),
        "project",
    );
    commit(&mut rig, &mut ws, 2, 0.25);
    wait_edited(&mut rig, &mut ws);
    rig.host().document().unwrap().hold_saves(true);
    common::send(&mut ws, json!({"type": "project.save", "id": 3}));
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while rig.host().document().unwrap().saves_held() == 0 {
        assert!(std::time::Instant::now() < until, "no save started");
        rig.pump();
    }
    for id in 4..14 {
        let app = rig.request(&mut ws, id, json!({"type": "app.get"}), "app");
        assert_eq!(app["project"]["edited"], true, "not written yet");
    }
    commit(&mut rig, &mut ws, 14, 0.75);
    common::send(&mut ws, json!({"type": "project.save", "id": 15}));
    rig.request(&mut ws, 16, json!({"type": "ping"}), "pong");
    let document = rig.host().document().unwrap();
    assert_eq!(document.saves_held(), 1, "no step waited for the save");
    assert!(document.saving());
    assert!(gain_of(&file).contains("gain = 0.5"), "not written yet");
    document.hold_saves(false);
    let first = rig.reply(&mut ws, 3, "saved");
    assert_eq!(first["file"], file.display().to_string());
    rig.reply(&mut ws, 15, "saved");
    assert!(gain_of(&file).contains("gain = 0.75"), "the later Save's");
    assert!(!rig.host().project_status().unwrap().edited);
    rig.finish();
}

/// Each save is answered with the file it wrote: the page's Save of the
/// file, then the window's Save As of another file asked while it runs,
/// both finished by opening another project; the first is told its file,
/// not the one the project has by then.
fn a_save_is_answered_with_the_file_it_wrote(transport: Transport) {
    let mut rig = app_rig("signal-save-file", transport, None);
    let file = packed(&rig, "tone");
    let other = file.with_file_name("other.signal");
    let (mut ws, _) = rig.client();
    rig.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": file.display().to_string()}),
        "project",
    );
    commit(&mut rig, &mut ws, 2, 0.25);
    wait_edited(&mut rig, &mut ws);
    rig.host().document().unwrap().hold_saves(true);
    common::send(&mut ws, json!({"type": "project.save", "id": 3}));
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while rig.host().document().unwrap().saves_held() == 0 {
        assert!(std::time::Instant::now() < until, "no save started");
        rig.pump();
    }
    let (answer, answered) = std::sync::mpsc::channel();
    rig.host().save_as_then(
        &other,
        Box::new(move |result| {
            let _ = answer.send(result);
        }),
    );
    let release = rig.host().document().unwrap().save_releaser();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        release();
    });
    let folder = rig.dir.clone();
    rig.host().open_project(&folder, Edits::Discard).unwrap();
    releaser.join().unwrap();
    assert_eq!(answered.try_recv(), Ok(Ok(())), "the Save As, answered");
    let saved = rig.reply(&mut ws, 3, "saved");
    assert_eq!(
        saved["file"],
        file.display().to_string(),
        "the file it wrote"
    );
    assert!(gain_of(&file).contains("gain = 0.25"));
    assert!(gain_of(&other).contains("gain = 0.25"));
    rig.finish();
}

/// A failed `project.save` (the file's folder made read-only while it
/// runs) is answered `not-saved` when it fails, and is the window's message.
fn a_failed_save_is_answered_when_it_fails(transport: Transport) {
    let mut rig = app_rig("signal-save-fails", transport, None);
    let file = packed(&rig, "tone");
    let (mut ws, _) = rig.client();
    rig.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": file.display().to_string()}),
        "project",
    );
    commit(&mut rig, &mut ws, 2, 0.25);
    wait_edited(&mut rig, &mut ws);
    let Some(read_only) = common::ReadOnly::new(file.parent().unwrap()) else {
        // Run as root: a read-only folder is still writable.
        return rig.finish();
    };
    common::send(&mut ws, json!({"type": "project.save", "id": 3}));
    let failed = rig.reply(&mut ws, 3, "error");
    drop(read_only);
    assert_eq!(failed["code"], "not-saved");
    let message = failed["message"].as_str().unwrap();
    assert!(message.contains("was not saved"), "{message}");
    let app = rig.request(&mut ws, 4, json!({"type": "app.get"}), "app");
    assert_eq!(app["message"], message);
    assert_eq!(app["project"]["edited"], true);
    rig.finish();
}

/// Save, then close at once (the window's Save, then Quit): close waits for
/// the save, which is answered, and removes the folder it saved.
fn close_waits_for_a_save(transport: Transport) {
    let mut rig = app_rig("signal-save-close", transport, None);
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
    rig.host().document().unwrap().hold_saves(true);
    let (answer, answered) = std::sync::mpsc::channel();
    rig.host().save_document_then(Box::new(move |result| {
        let _ = answer.send(result);
    }));
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while rig.host().document().unwrap().saves_held() == 0 {
        assert!(std::time::Instant::now() < until, "no save started");
        rig.pump();
    }
    rig.pump();
    assert!(answered.try_recv().is_err(), "not answered while held");
    let release = rig.host().document().unwrap().save_releaser();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        release();
    });
    let messages = rig.host().close(None, false);
    releaser.join().unwrap();
    assert_eq!(answered.try_recv(), Ok(Ok(())), "answered by the close");
    assert!(
        !messages.iter().any(|m| m.contains("are kept")),
        "{messages:?}"
    );
    assert!(!working.exists(), "saved: removed");
    assert!(gain_of(&file).contains("gain = 0.25"));
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

/// The file at `name` inside the `.signal` file `file`, if it holds one.
fn file_in(file: &Path, name: &str) -> Option<String> {
    let dir = file.with_extension("check");
    let _ = fs::remove_dir_all(&dir);
    archive::unpack(&fs::read(file).unwrap(), &dir).unwrap();
    let text = fs::read_to_string(dir.join(name)).ok();
    fs::remove_dir_all(&dir).unwrap();
    text
}

/// The owner's request of 2026-10-25 ("jó lenne egy file revert is"):
/// `project.revert` lets a `.signal` project's changes since its last Save
/// go (a commit to `project.toml`, a snapshot, a pattern) and opens the file
/// again, as `project.open` of it with `unsaved: "discard"`: a new working
/// folder, the old one removed, not edited, playing from frame 0 again.
fn revert_opens_the_saved_file_again(transport: Transport) {
    let mut rig = app_rig_with("signal-revert", transport, None, true);
    let file = packed(&rig, "tone");
    let (mut ws, _) = rig.client();
    rig.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": file.display().to_string()}),
        "project",
    );
    // The saved version: a commit and a snapshot, written to the file.
    commit(&mut rig, &mut ws, 2, 0.25);
    rig.request(
        &mut ws,
        3,
        json!({"type": "snapshot.save", "name": "saved"}),
        "snapshot.saved",
    );
    rig.request(&mut ws, 4, json!({"type": "project.save"}), "saved");
    let snapshots = file_in(&file, "snapshots.toml").expect("saved with its snapshot");
    assert!(snapshots.contains("saved"), "{snapshots}");
    let status = rig.host().project_status().unwrap();
    assert!(!status.edited);
    assert!(!status.revertible(), "nothing to go back from");
    let working = status.dir;

    // Changes since: another value, another snapshot, a new pattern.
    commit(&mut rig, &mut ws, 5, 0.75);
    rig.request(
        &mut ws,
        6,
        json!({"type": "snapshot.save", "name": "later"}),
        "snapshot.saved",
    );
    let text = "schema_version = 1\nkind = \"steps\"\nsteps = 4\nstep = 0.25\n\n[lanes]\nkick = \"x.x.\"\n";
    rig.request(
        &mut ws,
        7,
        json!({"type": "pattern.write", "name": "beat", "text": text, "base": null}),
        "pattern.written",
    );
    assert!(gain_in(&working).contains("gain = 0.75"));
    assert!(working.join("patterns/beat.toml").is_file());
    let status = rig.host().project_status().unwrap();
    assert!(status.edited && status.revertible());

    let reverted = rig.request(&mut ws, 8, json!({"type": "project.revert"}), "project");
    assert_eq!(reverted["file"], file.display().to_string());
    assert_eq!(reverted["loads"], true, "{reverted}");
    let dir = PathBuf::from(reverted["dir"].as_str().unwrap());
    assert_ne!(dir, working, "unpacked again into a new working folder");
    assert!(!working.exists(), "the old working folder is removed");
    assert!(gain_in(&dir).contains("gain = 0.25"), "the file's value");
    assert_eq!(
        fs::read_to_string(dir.join("snapshots.toml")).unwrap(),
        snapshots,
        "the file's snapshots"
    );
    assert!(
        !dir.join("patterns/beat.toml").exists(),
        "the new pattern goes"
    );
    assert!(
        gain_of(&file).contains("gain = 0.25"),
        "the file is untouched"
    );
    let status = rig.host().project_status().unwrap();
    assert_eq!(status.dir, dir);
    assert_eq!(status.file.as_deref(), Some(file.as_path()));
    assert!(!status.edited);
    assert_eq!(status.unsaved, 0);
    assert_eq!(Setup::load(&rig.setup).0.last_project, Some(file.clone()));
    let described = rig.wait(&mut ws, "describe playing", |got| {
        got.of("describe")
            .filter(|d| d.get("id").is_none() && d["playing"] == true)
            .cloned()
    });
    assert_eq!(described["playing"], true, "played again");
    rig.finish();
}

/// Revert waits for a Save still running: the file it opens again is the
/// one that Save wrote, so the edit saved is kept.
fn revert_waits_for_a_pending_save(transport: Transport) {
    let mut rig = app_rig("signal-revert-save", transport, None);
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
    rig.host().document().unwrap().hold_saves(true);
    let (answer, answered) = std::sync::mpsc::channel();
    rig.host().save_document_then(Box::new(move |result| {
        let _ = answer.send(result);
    }));
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while rig.host().document().unwrap().saves_held() == 0 {
        assert!(std::time::Instant::now() < until, "no save started");
        rig.pump();
    }
    assert!(gain_of(&file).contains("gain = 0.5"), "not written yet");
    // An edit after the Save packed: still a change to revert once it is
    // written.
    commit(&mut rig, &mut ws, 3, 0.75);
    let release = rig.host().document().unwrap().save_releaser();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        release();
    });
    rig.host().revert_project().unwrap();
    releaser.join().unwrap();
    assert_eq!(answered.try_recv(), Ok(Ok(())), "the Save, answered first");
    assert!(
        gain_of(&file).contains("gain = 0.25"),
        "the Save was written"
    );
    let status = rig.host().project_status().unwrap();
    assert_ne!(status.dir, working);
    assert!(!working.exists());
    assert!(
        gain_in(&status.dir).contains("gain = 0.25"),
        "opened again from what the Save wrote"
    );
    assert!(!status.edited);
    rig.finish();
}

/// The window's message names what let the unsaved values go: Revert to
/// Saved says they were discarded by it; opening another project keeps its
/// own wording.
fn revert_says_its_unsaved_edits_were_discarded(transport: Transport) {
    let mut rig = app_rig("signal-revert-unsaved", transport, None);
    let file = packed(&rig, "tone");
    let (mut ws, _) = rig.client();
    let opened = rig.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": file.display().to_string()}),
        "project",
    );
    let working = PathBuf::from(opened["dir"].as_str().unwrap());
    // A commit whose write fails stays an unsaved live value.
    let Some(read_only) = common::ReadOnly::new(&working) else {
        return rig.finish();
    };
    let refused = rig.request(
        &mut ws,
        2,
        json!({"type": "commit", "address": "/out.gain", "value": 0.125}),
        "error",
    );
    drop(read_only);
    assert_eq!(refused["code"], "write-failed");
    rig.unsaved(&mut ws, "listed", |u| u["count"] == 1);
    let status = rig.host().project_status().unwrap();
    assert_eq!(status.unsaved, 1);
    assert!(!status.edited && status.revertible(), "unsaved live values");
    rig.request(&mut ws, 3, json!({"type": "project.revert"}), "project");
    let app = rig.request(&mut ws, 4, json!({"type": "app.get"}), "app");
    assert_eq!(
        app["message"], "1 unsaved edit was discarded by Revert to Saved",
        "{app}"
    );
    assert!(!working.exists());

    // Opening another project keeps its wording.
    let working = rig.host().project_status().unwrap().dir;
    let Some(read_only) = common::ReadOnly::new(&working) else {
        return rig.finish();
    };
    rig.request(
        &mut ws,
        5,
        json!({"type": "commit", "address": "/out.gain", "value": 0.25}),
        "error",
    );
    drop(read_only);
    rig.unsaved(&mut ws, "listed again", |u| u["count"] == 1);
    let folder = rig.dir.display().to_string();
    rig.request(
        &mut ws,
        6,
        json!({"type": "project.open", "path": folder, "unsaved": "discard"}),
        "project",
    );
    let app = rig.request(&mut ws, 7, json!({"type": "app.get"}), "app");
    assert_eq!(
        app["message"], "1 unsaved edit of the previous project was not written",
        "{app}"
    );
    rig.finish();
}

/// Second-opinion review: a Save As in flight decides which file Revert to
/// Saved opens again. `project.revert` finishes it first, so the new file
/// (with what it saved) is reopened, not the old one.
fn revert_follows_a_save_as_in_flight(transport: Transport) {
    let mut rig = app_rig("signal-revert-save-as", transport, None);
    let file = packed(&rig, "tone");
    let other = file.with_file_name("other.signal");
    let (mut ws, _) = rig.client();
    rig.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": file.display().to_string()}),
        "project",
    );
    commit(&mut rig, &mut ws, 2, 0.25);
    rig.host().document().unwrap().hold_saves(true);
    let (answer, answered) = std::sync::mpsc::channel();
    rig.host().save_as_then(
        &other,
        Box::new(move |result| {
            let _ = answer.send(result);
        }),
    );
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while rig.host().document().unwrap().saves_held() == 0 {
        assert!(std::time::Instant::now() < until, "no save started");
        rig.pump();
    }
    // A change after the Save As packed, to revert.
    commit(&mut rig, &mut ws, 3, 0.75);
    let release = rig.host().document().unwrap().save_releaser();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        release();
    });
    let reverted = rig.request(&mut ws, 4, json!({"type": "project.revert"}), "project");
    releaser.join().unwrap();
    assert_eq!(
        answered.try_recv(),
        Ok(Ok(())),
        "the Save As, answered first"
    );
    assert_eq!(
        reverted["file"],
        other.display().to_string(),
        "the new file"
    );
    let dir = PathBuf::from(reverted["dir"].as_str().unwrap());
    assert!(
        gain_in(&dir).contains("gain = 0.25"),
        "what the Save As wrote"
    );
    assert!(
        gain_of(&file).contains("gain = 0.5"),
        "the old file untouched"
    );
    assert!(!rig.host().project_status().unwrap().edited);
    rig.finish();
}

/// Second-opinion review: Revert to Saved refuses when there is nothing to
/// revert (`not-edited`), and when the file is gone, corrupt or a folder
/// now (`archive`); refused, the working folder keeps its edits, the
/// project stays edited and plays on.
fn a_refused_revert_keeps_the_edits(transport: Transport) {
    let mut rig = app_rig_with("signal-revert-refused", transport, None, true);
    let file = packed(&rig, "tone");
    let (mut ws, _) = rig.client();
    let opened = rig.request(
        &mut ws,
        1,
        json!({"type": "project.open", "path": file.display().to_string()}),
        "project",
    );
    let working = PathBuf::from(opened["dir"].as_str().unwrap());
    let refused = rig.request(&mut ws, 2, json!({"type": "project.revert"}), "error");
    assert_eq!(refused["code"], "not-edited", "{refused}");
    assert_eq!(rig.host().project_status().unwrap().dir, working);

    commit(&mut rig, &mut ws, 3, 0.25);
    let refused_with = |rig: &mut Rig, ws: &mut common::Ws, id: u64, what: &str| {
        let refused = rig.request(ws, id, json!({"type": "project.revert"}), "error");
        assert_eq!(refused["code"], "archive", "{what}: {refused}");
        let status = rig.host().project_status().unwrap();
        assert_eq!(status.dir, working, "{what}");
        assert!(status.edited, "{what}: still edited");
        assert!(gain_in(&working).contains("gain = 0.25"), "{what}: kept");
        let app = rig.request(ws, id + 1, json!({"type": "app.get"}), "app");
        assert_eq!(app["playing"], true, "{what}: plays on");
        refused["message"].as_str().unwrap().to_owned()
    };
    fs::write(&file, b"not a zip").unwrap();
    refused_with(&mut rig, &mut ws, 4, "corrupt");
    fs::remove_file(&file).unwrap();
    let message = refused_with(&mut rig, &mut ws, 6, "removed");
    assert!(message.contains("is no longer a file"), "{message}");
    fs::create_dir(&file).unwrap();
    let message = refused_with(&mut rig, &mut ws, 8, "a folder");
    assert!(message.contains("is no longer a file"), "{message}");
    fs::remove_dir(&file).unwrap();
    rig.finish();
}

/// A folder project is written straight to its files: it has no saved
/// version, so Revert to Saved does not apply (`no-file`) and changes
/// nothing.
fn a_folder_project_is_not_reverted(transport: Transport) {
    let mut rig = app_rig("folder-revert", transport, None);
    let (mut ws, _) = rig.client();
    commit(&mut rig, &mut ws, 1, 0.25);
    let status = rig.host().project_status().unwrap();
    assert_eq!(status.file, None);
    assert!(!status.revertible());
    let refused = rig.request(&mut ws, 2, json!({"type": "project.revert"}), "error");
    assert_eq!(refused["code"], "no-file");
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains("no saved version to revert to"),
        "{refused}"
    );
    assert!(rig.host().revert_project().is_err());
    assert_eq!(rig.host().project_status().unwrap().dir, rig.dir);
    assert!(gain_in(&rig.dir).contains("gain = 0.25"), "nothing let go");
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

fn graph_names(graph: &Value) -> Vec<&str> {
    graph["top"]["instances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect()
}

/// M2.9-1 (ADR 0026 §1, §3): stopped, `graph` describes the folder, and a
/// change of it (its structure or `ui/schematic.toml`) pushes `graph`; a
/// folder that does not load is an error naming its diagnostics.
fn graph_while_stopped(transport: Transport) {
    let mut rig = app_rig(
        "graph-stopped",
        transport,
        Some(&root().join("projects/kick")),
    );
    let (mut ws, _) = rig.client();
    let graph = rig.request(&mut ws, 1, json!({"type": "graph"}), "graph");
    assert_eq!(
        graph_names(&graph),
        ["transport", "drums", "lfo", "accent", "out"]
    );
    assert_eq!(graph["planVersion"], Value::Null, "stopped");
    let path = rig.dir.join("project.toml");
    let text = fs::read_to_string(&path).unwrap().replace(
        "out = { module",
        "extra = { module = \"std.sine_osc\" }\nout = { module",
    );
    fs::write(&path, &text).unwrap();
    rig.wait(&mut ws, "graph after a stopped edit", |got| {
        got.of("graph")
            .filter(|g| g.get("id").is_none() && graph_names(g).contains(&"extra"))
            .cloned()
    });
    let moved = rig.request(
        &mut ws,
        2,
        json!({"type": "schematic.move", "level": "drums", "instance": "kick", "x": 5, "y": 6}),
        "schematic.moved",
    );
    assert_eq!(moved["file"], "ui/schematic.toml");
    fs::write(
        rig.dir.join("ui/schematic.toml"),
        "[top]\nout = { x = 1, y = 2 }\n",
    )
    .unwrap();
    rig.wait(&mut ws, "graph after a positions edit", |got| {
        got.of("graph")
            .filter(|g| g["layout"]["positions"][""]["out"] == json!({"x": 1, "y": 2}))
            .cloned()
    });
    fs::write(&path, "schema_version = 1\n[instances\n").unwrap();
    let refused = rig.request(&mut ws, 3, json!({"type": "graph"}), "error");
    assert_eq!(refused["code"], "does-not-load");
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains("project.toml"),
        "{refused}"
    );
    rig.finish();
}

/// M2.9-1, M2.8-11: switched off, nothing watches the folder; switching on
/// pushes the `graph` of what changed meanwhile.
fn graph_is_pushed_when_switched_on_again(transport: Transport) {
    let mut rig = app_rig(
        "graph-power",
        transport,
        Some(&root().join("projects/kick")),
    );
    let (mut ws, _) = rig.client();
    rig.request(&mut ws, 1, json!({"type": "graph"}), "graph");
    rig.request(
        &mut ws,
        2,
        json!({"type": "app.power", "on": false}),
        "power",
    );
    fs::write(
        rig.dir.join("ui/schematic.toml"),
        "[top]\nlfo = { x = 9, y = 9 }\n",
    )
    .unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_millis(400);
    while std::time::Instant::now() < until {
        rig.pump();
        while let Some(got) = common::receive(&mut ws) {
            let seen = got.of("graph").map(|g| g["layout"]["positions"].clone());
            assert!(
                seen.as_ref().is_none_or(|p| *p == json!({})),
                "the edit made while off is not watched: {seen:?}"
            );
        }
    }
    rig.request(
        &mut ws,
        3,
        json!({"type": "app.power", "on": true}),
        "power",
    );
    rig.wait(&mut ws, "graph after switching on", |got| {
        got.of("graph")
            .filter(|g| g["layout"]["positions"][""]["lfo"] == json!({"x": 9, "y": 9}))
            .cloned()
    });
    rig.finish();
}

/// The kick with its `drums` composite duplicated `voices` times and the
/// copies summed by a tree of 4-input `std.mix`, as `signal bench
/// --write-voices` writes it, in a new folder (a copy of `projects/kick`).
fn voices_project(name: &str, voices: usize) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("signal-ui-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    copy_dir(&root().join("projects/kick"), &dir);
    let mut instances = String::from(
        "transport = { module = \"std.transport\", bpm = 132, ghost_probability = 0.15, ghost_offset_ms = 6, accent_every = 3 }\n\
         lfo = { module = \"std.lfo\", freq_hz = 0.034 }\n\
         accent = { module = \"std.exp_env\", decay_ms = 200 }\n",
    );
    let mut connections =
        String::from("  { from = \"transport.accent\", to = \"accent.trigger\" },\n");
    let mut outputs = Vec::new();
    for i in 0..voices {
        instances.push_str(&format!("drums_{i} = {{ module = \"drums\" }}\n"));
        connections.push_str(&format!(
            "  {{ from = \"transport.beat\", to = \"drums_{i}.trigger\" }},\n\
             \x20 {{ from = \"transport.ghost\", to = \"drums_{i}.trigger\" }},\n\
             \x20 {{ from = \"lfo.out\", to = \"drums_{i}.kick_decay\", scale = 120 }},\n\
             \x20 {{ from = \"accent.out\", to = \"drums_{i}.kick_drive\", scale = 1.5 }},\n"
        ));
        outputs.push(format!("drums_{i}.out"));
    }
    let mut level = 0;
    while outputs.len() > 1 || level == 0 {
        let mut next = Vec::new();
        for (j, group) in outputs.chunks(4).enumerate() {
            let mix = format!("mix_{level}_{j}");
            instances.push_str(&format!("{mix} = {{ module = \"std.mix\" }}\n"));
            for (input, from) in ["a", "b", "c", "d"].iter().zip(group) {
                connections.push_str(&format!(
                    "  {{ from = \"{from}\", to = \"{mix}.{input}\" }},\n"
                ));
            }
            next.push(format!("{mix}.out"));
        }
        outputs = next;
        level += 1;
    }
    instances.push_str(&format!(
        "out = {{ module = \"std.audio_out\", gain = {} }}\n",
        0.75 / voices as f64
    ));
    for side in ["l", "r"] {
        connections.push_str(&format!(
            "  {{ from = \"{}\", to = \"out.{side}\" }},\n",
            outputs[0]
        ));
    }
    let text = format!(
        "schema_version = 1\n\n[project]\nname = \"kick-voices-{voices}\"\nsample_rate = 96000\nseed = 42\n\n\
         [instances]\n{instances}\n[wiring]\nconnections = [\n{connections}]\n"
    );
    fs::write(dir.join("project.toml"), text).unwrap();
    dir
}

/// M2.9-1b: stopped, `graph` is cached per folder state; on the
/// 4096-voice project a `schematic.move` writes once and loads nothing
/// (the `schematic.moved` reply and the host's counts; the `graph` itself:
/// `the_4096_voice_graph_reaches_the_client_and_it_stays_open`).
fn a_stopped_move_on_4096_voices_writes_once_and_loads_nothing(transport: Transport) {
    let source = voices_project(&format!("voices-{transport:?}"), 4096);
    let started = std::time::Instant::now();
    let mut rig = app_rig("move-4096", transport, Some(&source));
    let (mut ws, _) = rig.client();
    eprintln!("4096 voices: opened in {:?}", started.elapsed());
    let before = rig.host().graph_counts();
    assert_eq!(before.0, 1, "opening loaded the project once");
    assert_eq!(before.1, 1, "and kept its structure");
    let started = std::time::Instant::now();
    rig.request(
        &mut ws,
        2,
        json!({"type": "schematic.move", "level": "", "instance": "drums_7", "x": 10, "y": 20}),
        "schematic.moved",
    );
    eprintln!("4096 voices: moved in {:?}", started.elapsed());
    let after = rig.host().graph_counts();
    assert_eq!(after.0, before.0, "no load, no recompile");
    assert_eq!(after.1, before.1, "the structure was not built again");
    assert_eq!(after.2, before.2 + 1, "one write");
    let positions = fs::read_to_string(rig.dir.join("ui/schematic.toml")).unwrap();
    assert!(
        positions.contains("drums_7 = { x = 10, y = 20 }"),
        "{positions}"
    );
    rig.finish();
    let _ = fs::remove_dir_all(source);
}

/// M2.9-1b: stopped, `graph` loads the folder again only after it changed,
/// whatever asks: a request, `app.power`, `project.open`, `transport.*`.
fn the_stopped_graph_is_cached_until_the_folder_changes(transport: Transport) {
    let mut rig = app_rig(
        "graph-cache",
        transport,
        Some(&root().join("projects/kick")),
    );
    let (mut ws, _) = rig.client();
    rig.request(&mut ws, 1, json!({"type": "graph"}), "graph");
    rig.request(&mut ws, 2, json!({"type": "graph"}), "graph");
    let loads = |rig: &mut Rig| rig.host().graph_counts().0;
    assert_eq!(loads(&mut rig), 1, "opened once, then cached");
    rig.request(
        &mut ws,
        3,
        json!({"type": "app.power", "on": false}),
        "power",
    );
    rig.request(
        &mut ws,
        4,
        json!({"type": "app.power", "on": true}),
        "power",
    );
    rig.request(&mut ws, 5, json!({"type": "graph"}), "graph");
    assert_eq!(loads(&mut rig), 1, "switching off and on loads nothing");
    let dir = rig.dir.display().to_string();
    rig.request(
        &mut ws,
        6,
        json!({"type": "project.open", "path": dir}),
        "project",
    );
    rig.request(&mut ws, 7, json!({"type": "graph"}), "graph");
    assert_eq!(loads(&mut rig), 2, "opening loads once, and graph uses it");
    rig.request(&mut ws, 8, json!({"type": "transport.play"}), "transport");
    rig.request(&mut ws, 9, json!({"type": "transport.stop"}), "transport");
    rig.request(&mut ws, 10, json!({"type": "graph"}), "graph");
    assert_eq!(loads(&mut rig), 3, "only play's own load");
    let path = rig.dir.join("project.toml");
    let text = fs::read_to_string(&path).unwrap().replace(
        "out = { module",
        "extra = { module = \"std.sine_osc\" }\nout = { module",
    );
    fs::write(&path, &text).unwrap();
    rig.wait(&mut ws, "graph after a stopped edit", |got| {
        got.of("graph")
            .filter(|g| g.get("id").is_none() && graph_names(g).contains(&"extra"))
            .cloned()
    });
    rig.request(&mut ws, 11, json!({"type": "graph"}), "graph");
    assert_eq!(loads(&mut rig), 4, "the edit loads once");
    rig.finish();
}

/// M2.9-1b: a folder that stops loading pushes `graph.error` with its
/// diagnostics; loading again pushes `graph`.
fn a_project_that_stops_loading_pushes_graph_error(transport: Transport) {
    let mut rig = app_rig(
        "graph-error",
        transport,
        Some(&root().join("projects/kick")),
    );
    let (mut ws, _) = rig.client();
    rig.request(&mut ws, 1, json!({"type": "graph"}), "graph");
    let path = rig.dir.join("project.toml");
    let good = fs::read_to_string(&path).unwrap();
    fs::write(&path, "schema_version = 1\n[instances\n").unwrap();
    let error = rig.wait(&mut ws, "graph.error", |got| got.of("graph.error").cloned());
    assert_eq!(error["code"], "does-not-load");
    assert!(
        error["message"].as_str().unwrap().contains("project.toml"),
        "{error}"
    );
    fs::write(&path, good).unwrap();
    rig.wait(&mut ws, "graph once it loads again", |got| {
        got.of("graph").filter(|g| g.get("id").is_none()).cloned()
    });
    rig.finish();
}

/// M2.9-1b (auditor-1-benchmark-graph-disconnect): the 4096-voice `graph`,
/// about 6 MB, more than a client's 4 MiB write buffer, reaches the client
/// as one message, and the connection stays open after it.
fn the_4096_voice_graph_reaches_the_client_and_it_stays_open(transport: Transport) {
    let source = voices_project(&format!("voices-graph-{transport:?}"), 4096);
    let mut rig = app_rig("graph-4096", transport, Some(&source));
    let (mut ws, _) = rig.client();
    let graph = rig.request(&mut ws, 1, json!({"type": "graph"}), "graph");
    // transport, lfo, accent, 4096 voices, 1365 mixers, out.
    assert_eq!(graph["top"]["instances"].as_array().unwrap().len(), 5465);
    assert!(
        graph.to_string().len() > 4 << 20,
        "larger than the write buffer"
    );
    // Still connected: asked again, it answers, and the next message too.
    let again = rig.request(&mut ws, 2, json!({"type": "graph"}), "graph");
    assert_eq!(again["project"], graph["project"]);
    // (Not `describe`: this project's is about 22 MB, past the test client's
    // own 16 MiB frame limit; a browser has none.)
    rig.request(&mut ws, 3, json!({"type": "ping"}), "pong");
    rig.finish();
    let _ = fs::remove_dir_all(source);
}

both!(
    the_4096_voice_graph_reaches_the_client_and_it_stays_open,
    a_stopped_move_on_4096_voices_writes_once_and_loads_nothing,
    the_stopped_graph_is_cached_until_the_folder_changes,
    a_project_that_stops_loading_pushes_graph_error,
);

/// ADR 0033: Signal.app (`plays`) has no Play or Stop. Switched on, the
/// open project plays: at start, after `project.open`, and when switched on
/// again even if a `transport.stop` stopped it before; `transport.play`
/// (Project ▸ Restart from Files) plays it again from frame 0 while it
/// plays; switched off, nothing runs and a project opened then plays when
/// switched on.
fn it_plays_while_switched_on(transport: Transport) {
    let mut rig = app_rig_with("plays", transport, None, true);
    let (mut ws, _) = rig.client();
    let app = rig.request(&mut ws, 1, json!({"type": "app.get"}), "app");
    assert_eq!(
        (app["power"].clone(), app["playing"].clone()),
        (json!("on"), json!(true)),
        "plays at start"
    );
    // Restart from Files: the run starts again from frame 0.
    let before = rig.wait(&mut ws, "status", |got| {
        got.of("status")
            .filter(|s| s["playing"] == true && s["status"]["frame"].as_u64().unwrap_or(0) >= 4_800)
            .cloned()
    });
    let reached = before["status"]["frame"].as_u64().unwrap();
    let restarted = rig.request(&mut ws, 2, json!({"type": "transport.play"}), "transport");
    assert_eq!(restarted["playing"], true);
    let after = rig.wait(&mut ws, "status", |got| {
        got.of("status").filter(|s| s["playing"] == true).cloned()
    });
    assert!(
        after["status"]["frame"].as_u64().unwrap() < reached,
        "from frame 0 again: {after} after {reached}"
    );
    // A protocol stop still stops (`signal ui`, `signal mcp`, a script) and
    // holds through Setup ▸ Apply; the switch plays it again.
    rig.request(&mut ws, 3, json!({"type": "transport.stop"}), "transport");
    let applied = rig.request(
        &mut ws,
        30,
        json!({"type": "setup.apply", "buffer": 256}),
        "applied",
    );
    assert_eq!(applied["restarted"], false, "a deliberate stop holds");
    assert!(rig.host().controller().is_none());
    rig.request(
        &mut ws,
        4,
        json!({"type": "app.power", "on": false}),
        "power",
    );
    let on = rig.request(
        &mut ws,
        5,
        json!({"type": "app.power", "on": true}),
        "power",
    );
    assert_eq!(on["playing"], true, "on plays");
    // Opened while off: it plays once switched on.
    rig.request(
        &mut ws,
        6,
        json!({"type": "app.power", "on": false}),
        "power",
    );
    let path = rig.dir.display().to_string();
    let opened = rig.request(
        &mut ws,
        7,
        json!({"type": "project.open", "path": path}),
        "project",
    );
    assert_eq!(opened["loads"], true);
    assert!(rig.host().controller().is_none(), "nothing runs while off");
    let on = rig.request(
        &mut ws,
        8,
        json!({"type": "app.power", "on": true}),
        "power",
    );
    assert_eq!(on["playing"], true);
    // Opened while on: it plays.
    let opened = rig.request(
        &mut ws,
        9,
        json!({"type": "project.open", "path": path}),
        "project",
    );
    assert_eq!(opened["loads"], true);
    let app = rig.request(&mut ws, 10, json!({"type": "app.get"}), "app");
    assert_eq!(app["playing"], true);
    rig.finish();
}

/// The default output ("Null Out") running at `rate`, as listed.
fn listing_at(rate: u32) -> Vec<DeviceInfo> {
    let mut listed = devices();
    listed[0].rate = Some(rate);
    listed
}

/// Steps for `ms`: nothing is asked meanwhile.
fn idle(rig: &mut Rig, ms: u64) {
    let until = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < until {
        rig.pump();
    }
}

/// The `app` push whose `playing` is `playing`.
fn app_playing(rig: &mut Rig, ws: &mut common::Ws, playing: bool) -> Value {
    // `app` is pushed on a change only, so a run that started before this
    // client said hello is not pushed to it (in a release build the start
    // can win that race over TCP): ask for the state too, as Signal.app's
    // window does when it connects, and take the reply or a push.
    common::send(ws, json!({"type": "app.get"}));
    rig.wait(ws, "app", |got| {
        got.of("app").filter(|a| a["playing"] == playing).cloned()
    })
}

/// The owner's decision of 2026-10-23 (ADR 0036): the device's clock is the
/// run's. The default output runs at 44.1 kHz and the project is written
/// for 48 kHz: it plays at 44.1 kHz, never switching the device, with a
/// notice (the incident of 2026-10-22 refused it). A stream the audio
/// system stops while playing (its rate changed) plays again at the
/// output's new rate; a deliberate stop holds.
fn a_project_plays_at_the_rate_of_the_output(transport: Transport) {
    let mut rig = app_rig_listing("device-wait", transport, None, true, listing_at(44_100));
    let (mut ws, _) = rig.client();
    let app = app_playing(&mut rig, &mut ws, true);
    assert_eq!(rig.host().sample_rate(), 44_100);
    let notes = app["notes"].as_array().unwrap();
    assert!(
        notes.iter().any(|n| n
            .as_str()
            .is_some_and(|n| n.starts_with("'Null Out' runs at 44100 Hz; ")
                && n.ends_with(" is written for 48000 Hz and plays at 44100 Hz"))),
        "{notes:?}"
    );
    // The audio system stops the stream (the rate changed while playing):
    // it plays again, at the new rate.
    rig.host().set_null_devices(listing_at(48_000));
    rig.host().fail_output(
        "the device 'Null Out' changed its sample rate to 48000 Hz while playing at 44100 Hz",
    );
    let app = app_playing(&mut rig, &mut ws, false);
    assert_eq!(
        app["message"],
        "the device 'Null Out' changed its sample rate to 48000 Hz while playing at 44100 Hz; stopped, Signal tries the output again in a moment"
    );
    let app = app_playing(&mut rig, &mut ws, true);
    assert_eq!(
        app["message"],
        "the output runs at 48000 Hz: playing again from frame 0"
    );
    assert_eq!(rig.host().sample_rate(), 48_000);
    // Stopped on purpose: the device coming back does not play it.
    rig.host()
        .fail_output("the device 'Null Out' went away while playing");
    app_playing(&mut rig, &mut ws, false);
    rig.request(&mut ws, 2, json!({"type": "transport.stop"}), "transport");
    rig.host().set_null_devices(listing_at(44_100));
    idle(&mut rig, 1_300);
    assert!(rig.host().controller().is_none(), "a deliberate stop holds");
    rig.finish();
}

/// Review of 2026-10-23: an output that is there when the run stopped (a
/// rate switched away and back, a device busy for a moment) is tried once
/// at the next look, at the rate it runs at then (ADR 0036).
fn a_device_back_is_tried_once(transport: Transport) {
    let mut rig = app_rig_listing("device-once", transport, None, true, listing_at(48_000));
    let (mut ws, _) = rig.client();
    app_playing(&mut rig, &mut ws, true);
    rig.host()
        .fail_output("the device 'Null Out' changed its sample rate while playing at 48000 Hz");
    let app = app_playing(&mut rig, &mut ws, false);
    assert_eq!(
        app["message"],
        "the device 'Null Out' changed its sample rate while playing at 48000 Hz; stopped, Signal tries the output again in a moment"
    );
    let app = app_playing(&mut rig, &mut ws, true);
    assert_eq!(
        app["message"],
        "the output runs at 48000 Hz: playing again from frame 0"
    );
    // Stopped while the device runs at 44.1 kHz: it plays at 44.1 kHz,
    // the project's sample_rate unchanged.
    rig.host().set_null_devices(listing_at(44_100));
    rig.host().fail_output(
        "the device 'Null Out' changed its sample rate to 44100 Hz while playing at 48000 Hz",
    );
    app_playing(&mut rig, &mut ws, false);
    let app = app_playing(&mut rig, &mut ws, true);
    assert_eq!(
        app["message"],
        "the output runs at 44100 Hz: playing again from frame 0"
    );
    let text = fs::read_to_string(rig.dir.join("project.toml")).unwrap();
    assert!(
        text.contains("sample_rate = 48000"),
        "the file is not changed"
    );
    rig.finish();
}

/// `signal ui` and `signal mcp` (not app mode): a stream the audio system
/// stops keeps the run, so what it recorded is written when it finishes,
/// and the fault is reported once for the command to end with.
fn signal_ui_keeps_a_run_whose_output_stopped(transport: Transport) {
    let mut rig = rig_recording("output-fault", transport);
    let (_ws, _) = rig.client();
    assert_eq!(rig.host().output_fault(), None);
    rig.host()
        .fail_output("the device 'X' went away while playing");
    let told = rig.host().step();
    assert_eq!(told, ["the device 'X' went away while playing"]);
    let told = rig.host().step();
    assert!(
        !told.iter().any(|m| m.contains("went away")),
        "once: {told:?}"
    );
    assert!(rig.host().controller().is_some(), "the run is kept");
    assert_eq!(
        rig.host().output_fault(),
        Some("the device 'X' went away while playing")
    );
    let host = rig.host.take().unwrap();
    assert!(
        host.finish().is_some(),
        "what it recorded is there to write"
    );
    rig.finish();
}

both!(
    a_device_back_is_tried_once,
    signal_ui_keeps_a_run_whose_output_stopped,
    a_project_plays_at_the_rate_of_the_output,
    it_plays_while_switched_on,
    power_off_stops_and_on_restores,
    a_muted_instance_is_shown_until_the_project_plays_again,
    the_muted_note_lists_eight_then_counts,
    corrected_values_are_listed_as_warnings,
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
    signal_ui_stops_and_plays,
    a_signal_file_opens_in_a_working_folder,
    a_signal_file_with_corrected_values_is_not_edited,
    save_keeps_a_value_another_writer_changed,
    save_as_of_a_folder_leaves_its_files,
    a_failed_correction_does_not_block_the_save,
    a_panel_write_marks_a_signal_project_edited,
    a_pattern_write_marks_a_signal_project_edited,
    a_stopped_patch_writes_and_pushes_graph,
    a_patch_marks_a_signal_project_edited,
    steps_never_wait_for_a_document_comparison,
    steps_never_wait_for_a_save,
    a_save_is_answered_with_the_file_it_wrote,
    a_failed_save_is_answered_when_it_fails,
    close_waits_for_a_save,
    opening_another_project_can_save_first,
    reopening_the_same_file_with_save_keeps_the_edit,
    revert_opens_the_saved_file_again,
    revert_waits_for_a_pending_save,
    a_folder_project_is_not_reverted,
    revert_says_its_unsaved_edits_were_discarded,
    revert_follows_a_save_as_in_flight,
    a_refused_revert_keeps_the_edits,
    a_refused_archive_opens_nothing,
    save_as_writes_the_file_and_leaves_the_folder,
    closing_keeps_an_edited_working_folder,
    signal_ui_reopens_only_the_project_it_serves,
    a_folder_named_like_a_signal_file_opens_as_a_folder,
    graph_while_stopped,
    graph_is_pushed_when_switched_on_again,
);
