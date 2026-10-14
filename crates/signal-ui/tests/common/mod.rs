//! The protocol tests' rig: a project in a folder of its own, a [`Host`] on
//! the null driver stepped by the test's thread (the control thread), and
//! `tungstenite` clients.

#![allow(dead_code)]

use std::fs;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use signal_audio::{ConfigRange, DeviceInfo, Direction};
use signal_core::Registry;
use signal_engine::{Provenance, RtOptions};
use signal_ui::player::Backend;
use signal_ui::{FirstRun, Host, HostConfig, LocalStream, ServerOptions, project_ui};
use tungstenite::{Message, WebSocket};

/// How the test's clients reach the server: over loopback TCP, or over an
/// in-process stream served by the same WebSocket connection code (for
/// where no port can be bound).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Tcp,
    Local,
}

/// Runs scenario `name(transport)` as two tests: `name::tcp` and
/// `name::in_process`.
#[macro_export]
macro_rules! both {
    ($($name:ident),* $(,)?) => {$(
        mod $name {
            #[test]
            fn tcp() {
                super::$name($crate::common::Transport::Tcp);
            }
            #[test]
            fn in_process() {
                super::$name($crate::common::Transport::Local);
            }
        }
    )*};
}

/// A client's stream: TCP or in-process.
pub enum Stream {
    Tcp(TcpStream),
    Local(LocalStream),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Tcp(s) => s.read(buf),
            Stream::Local(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Tcp(s) => s.write(buf),
            Stream::Local(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.flush(),
            Stream::Local(s) => s.flush(),
        }
    }
}

pub type Ws = WebSocket<Stream>;

/// Two oscillators summed into the left input of a stereo sink, the first
/// also into the right; an envelope that reaches no sink (for `trigger`).
pub const TONE: &str = r#"schema_version = 1

[project]
name = "tone"
sample_rate = 48000
seed = 1

[instances]
a = { module = "std.sine_osc", freq = 440 }
b = { module = "std.sine_osc", freq = 220 }
env = { module = "std.exp_env", decay_ms = 50 }
out = { module = "std.audio_out", gain = 0.5 }

[wiring]
connections = [
  { from = "a.out", to = "out.l" },
  { from = "b.out", to = "out.l" },
  { from = "a.out", to = "out.r" },
]
"#;

fn device(name: &str, direction: Direction, channels: u16, min: u32, max: u32) -> DeviceInfo {
    DeviceInfo {
        name: name.to_owned(),
        id: Some(format!("null:{}", name.to_lowercase().replace(' ', "-"))),
        direction,
        default: false,
        configs: vec![ConfigRange {
            channels,
            min_rate: min,
            max_rate: max,
            buffer: Some((16, 4096)),
            sample_format: "f32".to_owned(),
        }],
    }
}

/// A 2-channel output, a 4-channel output, a 44.1 kHz-only output and a
/// 2-channel input.
pub fn devices() -> Vec<DeviceInfo> {
    let mut stereo = device("Null Out", Direction::Output, 2, 44_100, 96_000);
    stereo.default = true;
    vec![
        stereo,
        device("Null Four", Direction::Output, 4, 44_100, 96_000),
        device("Old Out", Direction::Output, 2, 44_100, 44_100),
        device("Null In", Direction::Input, 2, 44_100, 96_000),
    ]
}

pub struct Rig {
    pub host: Option<Host>,
    pub dir: PathBuf,
    pub setup: PathBuf,
}

pub struct Options {
    pub client_queue: usize,
    pub recording: bool,
    pub transport: Transport,
    /// A project folder to copy instead of the tone project.
    pub project: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            client_queue: 256,
            recording: false,
            transport: Transport::Tcp,
            project: None,
        }
    }
}

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Copies the folder `from` to `to` (files and folders only).
pub fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            fs::copy(&path, &target).unwrap();
        }
    }
}

/// A rig for test `name`, its folder holding the tone project (or a copy
/// of `options.project`).
pub fn rig(name: &str, options: Options) -> Rig {
    let base = std::env::temp_dir().join(format!(
        "signal-ui-{name}-{:?}-{}",
        options.transport,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&base);
    let dir = base.join("project");
    match &options.project {
        Some(project) => copy_dir(project, &dir),
        None => {
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("project.toml"), TONE).unwrap();
        }
    }
    let setup = base.join("setup.toml");
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let config = HostConfig {
        dir: dir.clone(),
        registry,
        setup_path: setup.clone(),
        backend: Backend::Null(devices()),
        first: FirstRun {
            device: None,
            buffer: 128,
            threads: 1,
        },
        rt: RtOptions::default(),
        boundary_data: false,
        recording: options.recording,
        provenance: Provenance::default(),
        compute: vec!["CPU: test".to_owned()],
        server: ServerOptions {
            port: 0,
            remote: false,
            ui_dir: root().join("ui"),
            project_ui: project_ui(&dir),
            client_queue: options.client_queue,
        },
    };
    let host = match options.transport {
        Transport::Tcp => Host::new(config),
        Transport::Local => Host::new_local(config),
    }
    .unwrap();
    Rig {
        host: Some(host),
        dir,
        setup,
    }
}

/// What a client received.
#[derive(Debug)]
pub enum Got {
    Json(Value),
    Frame(Vec<u8>),
    Closed,
}

impl Got {
    pub fn json(&self) -> Option<&Value> {
        match self {
            Got::Json(v) => Some(v),
            _ => None,
        }
    }

    /// A message of `kind`.
    pub fn of(&self, kind: &str) -> Option<&Value> {
        self.json()
            .filter(|v| v.get("type").and_then(Value::as_str) == Some(kind))
    }

    /// A binary frame of `kind` for `sub`.
    pub fn frame(&self, kind: u8, sub: u64) -> Option<Frame> {
        match self {
            Got::Frame(bytes) => {
                let frame = Frame::parse(bytes);
                (frame.kind == kind && u64::from(frame.sub) == sub).then_some(frame)
            }
            _ => None,
        }
    }
}

/// A decoded binary frame.
#[derive(Debug, Clone)]
pub struct Frame {
    pub kind: u8,
    pub version: u8,
    pub sub: u32,
    pub first: u64,
    pub count: u32,
    pub extra: u32,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn parse(bytes: &[u8]) -> Frame {
        assert!(bytes.len() >= 24, "a frame has a 24-byte header");
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        Frame {
            kind: bytes[0],
            version: bytes[1],
            sub: u32_at(4),
            first: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            count: u32_at(16),
            extra: u32_at(20),
            payload: bytes[24..].to_vec(),
        }
    }

    pub fn f32s(&self) -> Vec<f32> {
        let (chunks, _) = self.payload.as_chunks::<4>();
        chunks.iter().map(|c| f32::from_le_bytes(*c)).collect()
    }

    pub fn f64s(&self) -> Vec<f64> {
        let (chunks, _) = self.payload.as_chunks::<8>();
        chunks.iter().map(|c| f64::from_le_bytes(*c)).collect()
    }
}

/// Opens a WebSocket to the host (the handshake only): over TCP, or for a
/// host that binds nothing (port 0) over an in-process stream.
pub fn connect(host: &Host) -> Ws {
    let poll = Some(Duration::from_millis(2));
    if host.ws_addr().port() == 0 {
        let (mut ws, _) =
            tungstenite::client("ws://localhost/", Stream::Local(host.connect_local())).unwrap();
        if let Stream::Local(stream) = ws.get_mut() {
            stream.set_read_timeout(poll);
        }
        ws
    } else {
        let stream = TcpStream::connect(host.ws_addr()).unwrap();
        let url = format!("ws://{}/", host.ws_addr());
        let (mut ws, _) = tungstenite::client(url.as_str(), Stream::Tcp(stream)).unwrap();
        if let Stream::Tcp(stream) = ws.get_mut() {
            stream.set_read_timeout(poll).unwrap();
        }
        ws
    }
}

pub fn send(ws: &mut Ws, value: Value) {
    ws.send(Message::text(value.to_string())).unwrap();
}

/// The next message, if one came within the read timeout.
pub fn receive(ws: &mut Ws) -> Option<Got> {
    match ws.read() {
        Ok(Message::Text(text)) => Some(Got::Json(serde_json::from_str(&text).unwrap())),
        Ok(Message::Binary(bytes)) => Some(Got::Frame(bytes.to_vec())),
        Ok(Message::Close(_)) => Some(Got::Closed),
        Ok(_) => None,
        Err(tungstenite::Error::Io(e))
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            None
        }
        Err(_) => Some(Got::Closed),
    }
}

impl Rig {
    pub fn host(&mut self) -> &mut Host {
        self.host.as_mut().unwrap()
    }

    /// One control step after a null-driver callback's worth of frames.
    pub fn pump(&mut self) {
        let host = self.host();
        let _ = host.run_null(256, |_| {});
        host.step();
    }

    /// Steps until `ws` receives what `wanted` accepts.
    pub fn wait<T>(
        &mut self,
        ws: &mut Ws,
        what: &str,
        mut wanted: impl FnMut(&Got) -> Option<T>,
    ) -> T {
        let until = Instant::now() + Duration::from_secs(20);
        while Instant::now() < until {
            self.pump();
            while let Some(got) = receive(ws) {
                if let Some(found) = wanted(&got) {
                    return found;
                }
                if matches!(got, Got::Closed) {
                    panic!("closed while waiting for {what}");
                }
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// A reply of `kind` with `id`.
    pub fn reply(&mut self, ws: &mut Ws, id: u64, kind: &str) -> Value {
        self.wait(ws, kind, |got| {
            got.json()
                .filter(|v| v.get("id") == Some(&json!(id)))
                .map(|v| {
                    assert_eq!(v["type"], kind, "the reply to {id}: {v}");
                    v.clone()
                })
        })
    }

    /// Sends `message` with `id` and waits for its reply of `kind`.
    pub fn request(&mut self, ws: &mut Ws, id: u64, message: Value, kind: &str) -> Value {
        let mut message = message;
        message["id"] = json!(id);
        send(ws, message);
        self.reply(ws, id, kind)
    }

    /// A client that said hello: its socket and its client id.
    pub fn client(&mut self) -> (Ws, u64) {
        let token = self.host().token().to_owned();
        let mut ws = connect(self.host());
        send(
            &mut ws,
            json!({"type": "hello", "protocol": 1, "token": token}),
        );
        let welcome = self.wait(&mut ws, "welcome", |got| got.of("welcome").cloned());
        assert_eq!(welcome["protocol"], 1);
        (ws, welcome["client"].as_u64().unwrap())
    }

    /// Drops the host (its servers stop) and lets the detached client
    /// threads see it before the folder goes.
    pub fn finish(mut self) {
        drop(self.host.take());
        thread::sleep(Duration::from_millis(100));
        if let Some(base) = self.dir.parent() {
            let _ = fs::remove_dir_all(base);
        }
    }
}
