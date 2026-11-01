//! ADR 0053 (review 2026-11-01, panel-script-holds-token): a project's
//! panel is project content, trusted only within its own project. The page
//! a project serves gets the project's panel token, never the session
//! token; with it, the malicious panel of `tests/fixtures/malicious-panel`
//! can neither open another project, write another folder, nor reach the
//! machine's Setup, keymap or power, and once the app opens another project
//! its connection is closed and its token admits nothing more. The app's
//! own pages (the session token) keep every request.

mod common;

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{Got, Rig, Transport, Ws, connect, copy_dir, devices, receive, root, send};
use serde_json::{Value, json};
use signal_core::Registry;
use signal_ui::player::Backend;
use signal_ui::{AppHostConfig, Host, ServerOptions};

/// Signal.app's host on the null driver with a copy of the malicious
/// panel's project open, and a victim project beside it.
fn app(name: &str, transport: Transport) -> (Rig, PathBuf) {
    let base = std::env::temp_dir().join(format!(
        "signal-ui-panel-scope-{name}-{transport:?}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&base);
    let dir = base.join("project");
    copy_dir(
        &root().join("crates/signal-ui/tests/fixtures/malicious-panel"),
        &dir,
    );
    let victim = base.join("victim");
    fs::create_dir_all(&victim).unwrap();
    fs::write(victim.join("project.toml"), common::TONE).unwrap();
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
        plays: false,
    };
    let host = match transport {
        Transport::Tcp => Host::new_app(config),
        Transport::Local => Host::new_app_local(config),
    }
    .unwrap();
    (
        Rig {
            host: Some(host),
            dir,
            setup,
        },
        victim,
    )
}

/// A connection that says `hello` with `token`: its socket and the
/// `welcome`, or the refusal.
fn hello(rig: &mut Rig, token: &str) -> (Ws, Result<Value, Value>) {
    let mut ws = connect(rig.host());
    send(
        &mut ws,
        json!({"type": "hello", "protocol": 1, "token": token}),
    );
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        rig.pump();
        while let Some(got) = receive(&mut ws) {
            if let Some(welcome) = got.of("welcome") {
                let welcome = welcome.clone();
                return (ws, Ok(welcome));
            }
            if let Some(error) = got.of("error") {
                let error = error.clone();
                return (ws, Err(error));
            }
        }
    }
    panic!("no answer to hello");
}

/// Steps until `ws` is closed, the messages before it ignored.
fn closed(rig: &mut Rig, ws: &mut Ws) {
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        rig.pump();
        while let Some(got) = receive(ws) {
            if matches!(got, Got::Closed) {
                return;
            }
        }
    }
    panic!("the panel's connection was not closed");
}

fn a_project_s_panel_acts_only_on_its_project(transport: Transport) {
    let (mut rig, victim) = app("acts", transport);
    let session = rig.host().token().to_owned();
    let panel = rig.host().panel_tokens().token().expect("a panel token");
    assert_ne!(panel, session);
    // What the project's page is handed: the panel token only.
    let url = rig.host().panel_url();
    assert!(url.contains(&format!("&token={panel}")), "{url}");
    assert!(!url.contains(&session), "{url}");

    let (mut app_ws, _) = rig.client();
    let (mut ws, welcome) = hello(&mut rig, &panel);
    let welcome = welcome.expect("the panel token is admitted");
    assert_eq!(welcome["scope"], "project");

    // What tests/fixtures/malicious-panel/ui/main.html attempts, as it
    // sends it, each refused and nothing done.
    let victim_path = victim.display().to_string();
    for (id, attempt, code) in [
        (
            1,
            json!({"type": "project.open", "path": victim_path, "unsaved": "discard"}),
            "not-allowed",
        ),
        (
            2,
            json!({"type": "file.write", "file": "../victim/project.toml", "text": "pwned = true\n", "base": null}),
            "bad-value",
        ),
        (
            3,
            json!({"type": "setup.apply", "setup": {}}),
            "not-allowed",
        ),
        (4, json!({"type": "keys.set", "keys": {}}), "not-allowed"),
        (5, json!({"type": "app.power", "on": false}), "not-allowed"),
        (
            6,
            json!({"type": "device.route", "role": "main"}),
            "not-allowed",
        ),
        (7, json!({"type": "project.revert"}), "not-allowed"),
    ] {
        let refused = rig.request(&mut ws, id, attempt, "error");
        assert_eq!(refused["code"], code, "{refused}");
    }
    let app_state = rig.request(&mut app_ws, 50, json!({"type": "app.get"}), "app");
    assert_eq!(app_state["project"]["dir"], rig.dir.display().to_string());
    assert_eq!(
        fs::read_to_string(victim.join("project.toml")).unwrap(),
        common::TONE
    );
    // Its own project stays its to read and to play with.
    let read = rig.request(
        &mut ws,
        10,
        json!({"type": "file.read", "file": "project.toml"}),
        "file.read",
    );
    assert!(read["text"].as_str().unwrap().contains("malicious-panel"));
    rig.request(&mut ws, 11, json!({"type": "describe"}), "describe");

    // The Setup saved by the app's page is pushed to the app's pages only:
    // the panel may not read the Setup, so it is not sent it.
    let applied = rig.request(&mut app_ws, 51, json!({"type": "setup.apply"}), "applied");
    assert_eq!(applied["saved"], true, "{applied}");
    rig.wait(&mut app_ws, "the app's setup push", |got| {
        got.of("setup").filter(|v| v.get("id").is_none()).cloned()
    });
    send(&mut ws, json!({"type": "ping", "id": 12}));
    let until = Instant::now() + Duration::from_secs(20);
    'pong: loop {
        assert!(Instant::now() < until, "no pong");
        rig.pump();
        while let Some(got) = receive(&mut ws) {
            assert!(got.of("setup").is_none(), "the panel was sent the Setup");
            if got.of("pong").is_some() {
                break 'pong;
            }
        }
    }

    // The app opens the victim: the panel's connection goes, its token
    // admits nothing, and a new one is made for the project now open.
    rig.request(
        &mut app_ws,
        51,
        json!({"type": "project.open", "path": victim_path}),
        "project",
    );
    closed(&mut rig, &mut ws);
    let (_, again) = hello(&mut rig, &panel);
    let refused = again.expect_err("the old panel token is refused");
    assert_eq!(refused["code"], "unauthorized", "{refused}");
    let next = rig
        .host()
        .panel_tokens()
        .token()
        .expect("the victim's token");
    assert_ne!(next, panel);
    assert_eq!(
        fs::read_to_string(victim.join("project.toml")).unwrap(),
        common::TONE
    );
    // The session token keeps every request: the app's page opens back.
    rig.request(
        &mut app_ws,
        52,
        json!({"type": "project.open", "path": rig.dir.display().to_string()}),
        "project",
    );
    assert_ne!(rig.host().panel_tokens().token().unwrap(), panel);
    rig.finish();
}

/// Review of ADR 0053's change: a panel's messages read after it was
/// evicted (the app opened another project while they were in flight) or
/// waiting for the writer lock never run, and never with the session's
/// authority: its `project.open` of a third folder and its `keys.set`
/// arriving just after the app's open are dropped.
fn a_gone_panel_s_messages_never_run(transport: Transport) {
    let (mut rig, victim) = app("gone", transport);
    let third = victim.parent().unwrap().join("third");
    fs::create_dir_all(&third).unwrap();
    fs::write(third.join("project.toml"), common::TONE).unwrap();
    let keys = rig.setup.with_file_name("keys.toml");
    let panel = rig.host().panel_tokens().token().unwrap();
    let (mut app_ws, _) = rig.client();
    let (mut ws, welcome) = hello(&mut rig, &panel);
    welcome.expect("admitted");
    // The app's open first, then (without a step between) the panel's.
    send(
        &mut app_ws,
        json!({"type": "project.open", "id": 1, "path": victim.display().to_string()}),
    );
    std::thread::sleep(Duration::from_millis(200));
    send(
        &mut ws,
        json!({"type": "project.open", "id": 2, "path": third.display().to_string()}),
    );
    send(
        &mut ws,
        json!({"type": "keys.set", "id": 3, "keys": {"window": {"new_main": []}}}),
    );
    std::thread::sleep(Duration::from_millis(200));
    rig.reply(&mut app_ws, 1, "project");
    for _ in 0..50 {
        rig.pump();
    }
    let state = rig.request(&mut app_ws, 4, json!({"type": "app.get"}), "app");
    assert_eq!(state["project"]["dir"], victim.display().to_string());
    assert!(!keys.exists(), "the gone panel's keys.set ran");
    rig.finish();
}

both!(
    a_project_s_panel_acts_only_on_its_project,
    a_gone_panel_s_messages_never_run
);
