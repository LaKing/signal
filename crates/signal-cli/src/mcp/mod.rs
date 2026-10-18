//! `signal mcp` (M2.12-1, ADR 0029; `docs/agent-interface.md`): a Model
//! Context Protocol server over stdio, so an LLM client can do what a person
//! at the panel does. It is a client of the UI protocol (`docs/ui-protocol.md`):
//! every tool that observes or changes the running project sends UI-protocol
//! messages through the server code of `signal-ui`, never a parallel write
//! path.
//!
//! - `signal mcp <project>` plays the project itself, as `signal ui` does (the
//!   same host, watch and sessions; the null driver at real-time pace unless
//!   `--audio`), and connects in process: [`Host::new_local`] binds no port,
//!   and its connections are served by the same WebSocket code, `hello` and
//!   token as over TCP.
//! - `signal mcp --attach <url> <project>` connects to a running `signal ui`
//!   or Signal.app over its loopback WebSocket with its token.
//!
//! stdout carries the protocol's frames only (one JSON-RPC message per
//! line); everything else goes to stderr.

#![deny(clippy::print_stdout)]

mod client;
mod gain;
mod rpc;
mod tools;

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use clap::Args;
use serde_json::{Value, json};
use signal_core::{Fnv1a64, Registry};
use signal_engine::{ControlSink, RtOptions};
use signal_project::session::SessionError;
use signal_ui::{Host, LocalConnector, ServerOptions};

use crate::Failure;
use crate::drive::{Pace, drive};
use crate::play::write_wav;
use crate::ui::{Played, Serve, default_ui_dir, serve};

use client::Client;

/// Each client's queue of outgoing messages (as `signal ui`'s).
const CLIENT_QUEUE: usize = 256;

pub(crate) const LONG_ABOUT: &str = "\
Run an MCP (Model Context Protocol) server over stdio, so an LLM client
(Claude Code, Claude Desktop, ...) can inspect, play, adjust, patch and
observe a project as a person at the panel can (ADR 0029,
docs/agent-interface.md).

Without --attach it plays the project itself, as `signal ui` does, on the
null driver at real-time pace (no sound) unless --audio, and talks to it in
process through the UI protocol's server code. With --attach it connects to a
running `signal ui` or Signal.app instead (the URL it prints, with its token).

Every change is a change of the project's text files, made through the UI
protocol's write paths (base SHA-256, atomic, never through a link, validated
before it is written). Without --attach every sample sent to the output device
is held within +/- --max-gain (the output ceiling), and a set, ramp, commit,
patch or write that would put a std.audio_out gain above it is refused early;
while the project's files do not load, every change is refused. --read-only
exposes only the inspect, observe and render tools.

Claude Code:     claude mcp add signal -- <path>/signal mcp <project>
Claude Desktop:  see docs/agent-interface.md for the JSON snippet.

stdout carries protocol frames only; logs go to stderr. The server ends when
its stdin closes.";

/// Arguments of `signal mcp`.
#[derive(Args)]
pub(crate) struct McpArgs {
    /// The project folder (containing project.toml). Attached, the folder of
    /// the project the running server plays.
    project: PathBuf,
    /// Attach to a running `signal ui` or Signal.app: the URL it prints
    /// (`http://127.0.0.1:<port>/…#ws=<port>&token=<token>`) or
    /// `ws://127.0.0.1:<port>/#token=<token>`. Loopback only.
    #[arg(long, value_name = "URL")]
    attach: Option<String>,
    /// Play the project's audio on the default output device (CoreAudio).
    /// Without it nothing is heard: the null driver runs at real-time pace.
    #[arg(long)]
    audio: bool,
    /// The output ceiling: every device sample is held within +/- G (without
    /// --attach), and no tool may raise a std.audio_out gain above G (0 to 4).
    #[arg(long, default_value_t = 0.25, value_name = "G")]
    max_gain: f64,
    /// Expose only the inspect, observe and render tools.
    #[arg(long)]
    read_only: bool,
    /// Where renders and sessions are written (default: a folder of its own
    /// under the system's temporary folder). Never the project's folder.
    #[arg(long)]
    out: Option<PathBuf>,
    /// The output device, with --audio (default: the system's default).
    #[arg(long)]
    device: Option<String>,
    /// Frames per callback (null driver: per chunk).
    #[arg(long, default_value_t = 256, value_parser = clap::value_parser!(u32).range(1..=8192))]
    buffer: u32,
    /// Threads a block runs on, the calling thread included (0: one per
    /// performance core).
    #[arg(long, default_value_t = 0)]
    threads: u32,
    /// The null driver as fast as it goes instead of at real-time pace (for
    /// tests; meters and scopes then cover less wall time).
    #[arg(long)]
    fast: bool,
}

/// How the tools reach the project.
pub(crate) struct Link {
    pub(crate) client: Client,
    /// In process: the host's control thread (sessions).
    pub(crate) control: Option<Control>,
}

/// The in-process host's control thread, for what restarts the host.
pub(crate) struct Control {
    requests: Sender<Request>,
    wanted: Arc<AtomicBool>,
}

/// What only the control thread can do: restart the host (a session covers
/// one run from frame 0).
enum Request {
    /// Play again from frame 0, recording a session.
    Start(Sender<Result<Connect, String>>),
    /// End the recording run, write the session into the folder, play again.
    Stop(PathBuf, Sender<Result<(Connect, Value), String>>),
}

/// A new host's way in.
struct Connect {
    connector: LocalConnector,
    token: String,
}

impl Control {
    fn ask<T>(&self, request: impl FnOnce(Sender<T>) -> Request) -> Result<T, String> {
        let (tx, rx) = channel();
        // Sent before the control thread is woken: woken first, it could
        // find the queue still empty and play on with the request unseen.
        self.requests
            .send(request(tx))
            .map_err(|_| "the server is stopping".to_owned())?;
        self.wanted.store(true, Ordering::Release);
        rx.recv_timeout(Duration::from_secs(60))
            .map_err(|_| "the host did not answer".to_owned())
    }

    /// Restarts the host recording a session; the new host's client.
    pub(crate) fn session_start(&self) -> Result<Client, String> {
        let connect = self.ask(Request::Start)??;
        connect_local(&connect)
    }

    /// Ends the recorded run: its session written into `dir`, the host
    /// playing again; the new host's client and what was written.
    pub(crate) fn session_stop(&self, dir: &Path) -> Result<(Client, Value), String> {
        let (connect, written) = self.ask(|tx| Request::Stop(dir.to_owned(), tx))??;
        Ok((connect_local(&connect)?, written))
    }
}

fn connect_local(connect: &Connect) -> Result<Client, String> {
    Client::connect_local(connect.connector.connect(), &connect.token)
}

/// What the tools work on.
pub(crate) struct Context<'a> {
    pub(crate) link: Mutex<Link>,
    /// The project folder.
    pub(crate) dir: PathBuf,
    pub(crate) registry: &'a Registry,
    pub(crate) setup: Option<&'a Path>,
    pub(crate) max_gain: f64,
    /// The output ceiling this process's host holds every device sample
    /// within (`--max-gain`), or `None` attached (the running server's
    /// output is its own).
    pub(crate) ceiling: Option<f64>,
    pub(crate) read_only: bool,
    /// `in-process` or `attached`.
    pub(crate) mode: &'static str,
    /// `null`, `coreaudio` or (attached) `the running server's`.
    pub(crate) driver: &'static str,
    /// Renders and sessions.
    pub(crate) out: PathBuf,
}

pub(crate) fn run(
    args: &McpArgs,
    registry: &Registry,
    setup: Option<&Path>,
) -> Result<(), Failure> {
    if !args.max_gain.is_finite() || !(0.0..=4.0).contains(&args.max_gain) {
        return Err(Failure::Usage(format!(
            "--max-gain is a gain from 0 to 4, not {}",
            args.max_gain
        )));
    }
    if args.project.extension().is_some_and(|e| e == "signal") {
        return Err(Failure::Usage(
            "signal mcp plays a project folder: unpack the .signal file first (`signal unpack`)"
                .to_owned(),
        ));
    }
    let dir = args
        .project
        .canonicalize()
        .map_err(|e| Failure::Io(format!("{}: {e}", args.project.display())))?;
    let out = match &args.out {
        Some(out) => out.clone(),
        None => std::env::temp_dir().join(format!("signal-mcp-{}", std::process::id())),
    };
    std::fs::create_dir_all(&out).map_err(|e| Failure::Io(format!("{}: {e}", out.display())))?;
    let out = out
        .canonicalize()
        .map_err(|e| Failure::Io(format!("{}: {e}", out.display())))?;
    if out.starts_with(&dir) {
        return Err(Failure::Usage(
            "--out is inside the project folder: renders and sessions never go there".to_owned(),
        ));
    }
    match &args.attach {
        Some(url) => attach(args, url, dir, out, registry, setup),
        None => in_process(args, dir, out, registry, setup),
    }
}

/// `--attach`: the tools talk to a running server over loopback.
fn attach(
    args: &McpArgs,
    url: &str,
    dir: PathBuf,
    out: PathBuf,
    registry: &Registry,
    setup: Option<&Path>,
) -> Result<(), Failure> {
    let client = Client::attach(url).map_err(Failure::Usage)?;
    // The folder named must be the one the server plays (`graph.projectId`,
    // the SHA-256 of its canonical path).
    let graph = client
        .request(json!({"type": "graph"}))
        .map_err(|e| Failure::Usage(format!("the server's graph: {e}")))?;
    let id = signal_ui::panel_write::sha256_hex(dir.as_os_str().as_encoded_bytes());
    if graph.get("projectId").and_then(Value::as_str) != Some(id.as_str()) {
        return Err(Failure::Usage(format!(
            "{} is not the project the server at {url} plays",
            dir.display()
        )));
    }
    eprintln!("signal: mcp attached to {}", dir.display());
    let context = Context {
        link: Mutex::new(Link {
            client,
            control: None,
        }),
        dir,
        registry,
        setup,
        max_gain: args.max_gain,
        ceiling: None,
        read_only: args.read_only,
        mode: "attached",
        driver: "the running server's",
        out,
    };
    serve_stdio(&context);
    Ok(())
}

/// The host this process plays, and how.
struct Hosted<'a> {
    played: Played<'a>,
    server: ServerOptions,
    registry: &'a Registry,
    setup: Option<&'a Path>,
}

/// A running host and what its recording needs.
struct Running {
    host: Host,
    serve_gpu: crate::gpu::Gpu,
    width: usize,
    /// The run records a session.
    recording: bool,
    /// The samples played since the recording run started (null driver).
    audio: Option<Vec<f32>>,
}

impl Hosted<'_> {
    fn start(&mut self, recording: bool) -> Result<Running, Failure> {
        self.played.recording = recording;
        let Serve {
            config, gpu, width, ..
        } = serve(&self.played, self.registry, self.setup, self.server.clone())?;
        let host = Host::new_local(config).map_err(Failure::Device)?;
        Ok(Running {
            host,
            serve_gpu: gpu,
            width,
            recording,
            audio: (recording && self.played.null).then(Vec::new),
        })
    }
}

/// Without `--attach`: this thread plays the project and steps its host;
/// another serves the protocol on stdio.
fn in_process(
    args: &McpArgs,
    dir: PathBuf,
    out: PathBuf,
    registry: &Registry,
    setup: Option<&Path>,
) -> Result<(), Failure> {
    let null = !args.audio;
    let mut hosted = Hosted {
        played: Played {
            project: &dir,
            null,
            device: args.device.clone(),
            buffer: args.buffer,
            threads: args.threads,
            rt: RtOptions::default(),
            boundary_data: false,
            recording: false,
            ceiling: Some(args.max_gain),
        },
        server: ServerOptions {
            port: 0,
            remote: false,
            ui_dir: default_ui_dir(),
            project_ui: PathBuf::new(),
            client_queue: CLIENT_QUEUE,
        },
        registry,
        setup,
    };
    let mut running = hosted.start(false)?;
    eprintln!(
        "signal: mcp playing {} at {} Hz{}",
        dir.display(),
        running.host.sample_rate(),
        if null {
            " with the null driver (no sound; --audio plays it)"
        } else {
            " on the output device (--audio)"
        }
    );
    // The client connects from the protocol's thread: the host answers its
    // `hello` only as this thread steps it.
    let first = Connect {
        connector: running.host.local_connector(),
        token: running.host.token().to_owned(),
    };
    let (requests, received) = channel();
    let wanted = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let pace = Pace {
        null,
        buffer: args.buffer,
        fast: args.fast,
        limit: None,
    };
    let failed = thread::scope(|scope| {
        let stopping = Arc::clone(&stop);
        let wake = Arc::clone(&wanted);
        let control = Control {
            requests,
            wanted: Arc::clone(&wanted),
        };
        let dir = dir.clone();
        let protocol = scope.spawn(move || {
            // Whatever ends this thread, a tool's panic included, stops the
            // host: the control loop waits for nothing else.
            let _stopping = StopOnDrop([stopping, wake]);
            let served = connect_local(&first).map(|client| {
                let context = Context {
                    link: Mutex::new(Link {
                        client,
                        control: Some(control),
                    }),
                    dir,
                    registry,
                    setup,
                    max_gain: args.max_gain,
                    ceiling: Some(args.max_gain),
                    read_only: args.read_only,
                    mode: "in-process",
                    driver: if null { "null" } else { "coreaudio" },
                    out,
                };
                serve_stdio(&context);
            });
            served.err()
        });
        control_loop(&mut hosted, &mut running, &pace, &stop, &wanted, &received);
        protocol.join().ok().flatten()
    });
    if let Some(error) = failed {
        return Err(Failure::Internal(format!(
            "the in-process connection failed: {error}"
        )));
    }
    if let Some(finished) = running.host.finish()
        && let Some(failure) = finished.status.failure
    {
        eprintln!("signal: the engine failed: {failure}");
    }
    Ok(())
}

/// Sets its flags when dropped: the protocol thread's end, however it ends,
/// stops the control loop.
struct StopOnDrop([Arc<AtomicBool>; 2]);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        for flag in &self.0 {
            flag.store(true, Ordering::Release);
        }
    }
}

/// Whether the running engine stopped on a failure (`docs/semantics.md`
/// §14): it plays nothing until a client plays the project again
/// (`transport.play`), which builds a new run.
fn faulted(host: &Host) -> bool {
    host.controller()
        .is_some_and(|c| c.status().failure.is_some())
}

/// Plays the host until stdin closes, doing what the tools ask of the
/// control thread in between.
fn control_loop(
    hosted: &mut Hosted,
    running: &mut Running,
    pace: &Pace,
    stop: &AtomicBool,
    wanted: &AtomicBool,
    received: &Receiver<Request>,
) {
    while !stop.load(Ordering::Acquire) {
        let interrupted = || stop.load(Ordering::Acquire) || wanted.load(Ordering::Acquire);
        drive(
            &mut running.host,
            pace,
            &interrupted,
            running.audio.as_mut(),
        );
        // A failed engine returns at once: keep serving its clients (they
        // see the failure) until asked for something, or until one of them
        // plays the project again (a new run, which `drive` then plays).
        while !interrupted() && faulted(&running.host) {
            for message in running.host.step() {
                eprintln!("signal: {message}");
            }
            thread::sleep(Duration::from_millis(10));
        }
        if !wanted.swap(false, Ordering::AcqRel) {
            continue;
        }
        while let Ok(request) = received.try_recv() {
            match request {
                Request::Start(reply) => {
                    let started = restart(hosted, running, true).map(|(connect, old)| {
                        drop(old.host.finish());
                        connect
                    });
                    let _ = reply.send(started);
                }
                Request::Stop(folder, reply) => {
                    let answer = if !running.recording {
                        Err("no session is being recorded: call session_start first".to_owned())
                    } else {
                        let ceiling = hosted.played.ceiling;
                        restart(hosted, running, false).and_then(|(connect, old)| {
                            write_session(old, &folder, ceiling).map(|written| (connect, written))
                        })
                    };
                    let _ = reply.send(answer);
                }
            }
        }
    }
}

/// Replaces the host with a new one playing from frame 0 (`recording`: a
/// session of the run); the old one, still to be finished.
fn restart(
    hosted: &mut Hosted,
    running: &mut Running,
    recording: bool,
) -> Result<(Connect, Running), String> {
    let new = hosted.start(recording).map_err(|f| f.message())?;
    let old = std::mem::replace(running, new);
    let connect = Connect {
        connector: running.host.local_connector(),
        token: running.host.token().to_owned(),
    };
    Ok((connect, old))
}

/// Ends the recorded run `old` and writes its session into `folder/session`
/// and the audio it played into `folder/audio.wav`, beside it (the session
/// format holds no audio: `docs/session-format.md` §1). The audio is what
/// the device got, held within the output `ceiling`; `captured.json` names
/// it, so a replay is compared held the same way.
fn write_session(old: Running, folder: &Path, ceiling: Option<f64>) -> Result<Value, String> {
    let Running {
        host,
        serve_gpu,
        width,
        audio,
        ..
    } = old;
    let rate = host.sample_rate();
    let finished = host
        .finish()
        .ok_or_else(|| "nothing was playing: the session has no run".to_owned())?;
    let mut live = finished.live;
    live.status(&finished.status);
    live.finish(finished.status.next_frame)
        .map_err(|e| format!("the session's end was not recorded: {e}"))?;
    let session = folder.join("session");
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let mut recorder = live.into_recorder();
    recorder.set_devices(serve_gpu.devices());
    recorder
        .write(&session)
        .map_err(|error: SessionError| error.to_string())?;
    // Under the null driver this process played every sample itself; on a
    // device (`--audio`) they were the device's, and none is kept.
    let (wav, hash) = match audio {
        Some(samples) => {
            let wav = folder.join("audio.wav");
            write_wav(&wav, &samples, width, rate).map_err(|f| f.message())?;
            let mut hasher = Fnv1a64::new();
            for sample in &samples {
                hasher.update(&sample.to_le_bytes());
            }
            (
                json!(wav.display().to_string()),
                json!(format!("{:016x}", hasher.finish())),
            )
        }
        None => (Value::Null, Value::Null),
    };
    let captured = json!({
        "session": session.display().to_string(),
        "wav": wav,
        "frames": finished.status.next_frame.0,
        "capturedFnv1a64": hash,
        "ceiling": ceiling,
    });
    std::fs::write(folder.join("captured.json"), captured.to_string())
        .map_err(|e| format!("{}: {e}", folder.display()))?;
    Ok(captured)
}

/// Serves MCP on stdin and stdout until stdin closes.
fn serve_stdio(context: &Context) {
    let mut server = rpc::Server::new(context);
    let stdin = std::io::stdin();
    let mut line = String::new();
    let mut input = stdin.lock();
    loop {
        line.clear();
        match input.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = server.handle(line.trim_end()) {
            let mut stdout = std::io::stdout().lock();
            // One message per line, no embedded newline (serde_json escapes them).
            if writeln!(stdout, "{reply}")
                .and_then(|()| stdout.flush())
                .is_err()
            {
                break;
            }
        }
    }
}
