//! `signal ui` (M2.8-1): the built binary plays a fixture on the null driver
//! with `play`'s options, prints the URL to open, serves the protocol on
//! 127.0.0.1, and exits 0 at `--seconds`. `--open` is not run here: it opens
//! a browser (an operator step).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::Message;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn signal_ui_serves_the_protocol_while_it_plays() {
    let setup = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ui-setup.toml");
    let mut child = Command::new(env!("CARGO_BIN_EXE_signal"))
        .args([
            "ui",
            "--driver",
            "null",
            "--seconds",
            "3",
            "--buffer",
            "256",
            "--setup",
        ])
        .arg(&setup)
        .arg(fixture("sine"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut url = None;
    let mut line = String::new();
    while url.is_none() && stderr.read_line(&mut line).unwrap() > 0 {
        url = line
            .trim()
            .strip_prefix("signal: ui at ")
            .map(str::to_owned);
        line.clear();
    }
    let url = url.expect("the URL on stderr");
    let (page, fragment) = url.split_once("/#").unwrap();
    assert!(page.starts_with("http://127.0.0.1:"), "{url}");
    let fragment: Vec<(&str, &str)> = fragment
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .collect();
    let ws = fragment.iter().find(|(k, _)| *k == "ws").unwrap().1;
    let token = fragment.iter().find(|(k, _)| *k == "token").unwrap().1;
    assert_eq!(token.len(), 64, "a 32-byte token");

    let (mut socket, _) = tungstenite::connect(format!("ws://127.0.0.1:{ws}/")).unwrap();
    let send = |socket: &mut tungstenite::WebSocket<_>, value: Value| {
        socket.send(Message::text(value.to_string())).unwrap();
    };
    send(
        &mut socket,
        json!({"type": "hello", "protocol": 1, "token": token}),
    );
    send(&mut socket, json!({"type": "describe", "id": 1}));
    let until = Instant::now() + Duration::from_secs(10);
    let mut welcome = None;
    let mut describe = None;
    while describe.is_none() && Instant::now() < until {
        if let Ok(Message::Text(text)) = socket.read() {
            let value: Value = serde_json::from_str(&text).unwrap();
            match value["type"].as_str() {
                Some("welcome") => welcome = Some(value),
                Some("describe") => describe = Some(value),
                _ => {}
            }
        }
    }
    let welcome = welcome.expect("welcome");
    assert_eq!(welcome["protocol"], 1);
    assert_eq!(welcome["project"], "sine");
    let describe = describe.expect("describe");
    assert!(
        describe["controls"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["address"] == "/out.gain")
    );
    drop(socket);
    // Drain stderr so the child never blocks on it.
    let rest = std::thread::spawn(move || {
        let mut text = String::new();
        while stderr.read_line(&mut text).unwrap_or(0) > 0 {}
        text
    });
    let status = child.wait().unwrap();
    let text = rest.join().unwrap();
    assert!(status.success(), "{status}: {text}");
    assert!(text.contains("played 144000 frames at 48000 Hz"), "{text}");
}
