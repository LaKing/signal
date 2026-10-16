//! The protocol's host on the control thread (`docs/ui-protocol.md`): it
//! owns the run (its output, input, `Controller`, session recorder and file
//! watch), the parameter panel and the clients' state, and is stepped by its
//! owner's control loop. It reaches the audio thread only through the
//! `Controller` and its queues (edits, reports, parameter values, meters,
//! scopes); the player hands the Runner to a driver and takes it back here.
//!
//! Every client message is answered from [`Host::step`]; nothing here waits
//! for a client: messages go out through each client's bounded queue with
//! `try_send`, a full queue drops a data frame (counted) and evicts a client
//! whose control reply does not fit.
//!
//! In app mode ([`Host::new_app`], Signal.app's server, M2.8-4) it starts
//! stopped and takes the app's own messages too (`app.get`,
//! `transport.play`/`stop`, `project.open`, `project.save`), with Signal.app's
//! rules ([`crate::app`]): every run plays from the Setup, as the app's state
//! does. A `.signal` project (M2.8-12) plays from its working folder
//! ([`crate::document`]); `signal ui` reopens only the project it serves.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use signal_audio::{
    ChannelMap, CoreAudioDriver, DeviceInfo, DeviceRef, Direction, Resolution, Scheduling, labels,
    resolve, rt_thread,
};
use signal_core::{
    Curve, Direction as PortDirection, EventPayload, Frame, PortAddress, PortShape, Registry,
};
use signal_engine::{
    CommandId, ControlSink, HangLine, Outcome, ParameterValue, Provenance, RealtimeConfig, Report,
    RtOptions, RtPolicy, RunnerStatus, SentCommand, Stamp,
};
use signal_graph::ExecutionPlan;
use signal_output::{
    Controller, Meter, NullDriver, SCOPE_POINTS, SCOPE_SLOTS, ScopeBlock, VECTOR_MAX_ROWS,
    VECTOR_SLOTS, VectorSnapshot, VectorUnbound, realtime_with,
};
use signal_project::archive::{self, TempProject};
use signal_project::live::{Both, ProjectWatcher, ReloadError, WATCH_INTERVAL_MS, reload_observed};
use signal_project::schematic::{self, SCHEMATIC_FILE};
use signal_project::session::{LiveRecorder, SessionRecorder, sink_of};
use signal_project::{Diagnostic, LoadError, PROJECT_FILE, Project, located};

use crate::PROTOCOL;
use crate::app::{self, Status};
use crate::bindings::{MAIN_PANEL, Targets, check, panel_content};
use crate::document::{Document, project_name};
use crate::frames::{self, frame};
use crate::panel::{Panel, Target, Written, after_write, ramp_frames, write_file, write_project};
use crate::panel_write::{check_content, first_file, replace, sha256_hex};
use crate::player::{Backend, Input, Interface, Output, Selection, open_input, open_output};
use crate::server::{Inbound, Outbound, Server, ServerOptions};
use crate::setup::{
    Choice, DEFAULT_BUFFER, Setup, Window, capabilities, channels, found, rate_check,
};

/// Client messages handled per step at most.
const MESSAGES_PER_STEP: usize = 256;
/// How often the status line is pushed while playing (app mode).
const STATUS_EVERY: Duration = Duration::from_millis(250);
/// The fastest parameter and meter streams.
const MAX_RATE_HZ: f64 = 60.0;

/// How the first run plays: `signal play`'s driver options.
#[derive(Clone, Debug)]
pub struct FirstRun {
    /// The output device's name (CoreAudio); the default output without.
    pub device: Option<String>,
    /// The buffer to request, in frames.
    pub buffer: u32,
    /// Threads per block (0: the default).
    pub threads: u32,
}

/// What a [`Host`] runs.
pub struct HostConfig {
    /// The project folder.
    pub dir: PathBuf,
    /// The project as given: `dir`, or the `.signal` file `dir` was unpacked
    /// from. A `project.open` of it reopens it (M2.8-12); nothing else opens.
    pub source: PathBuf,
    /// The temporary folder a `.signal` file was unpacked into, kept (and
    /// removed) with the host.
    pub temporary: Option<TempProject>,
    /// The modules.
    pub registry: Registry,
    /// The Setup file `setup.*` reads and writes.
    pub setup_path: PathBuf,
    /// CoreAudio, or the null driver with a device list.
    pub backend: Backend,
    /// The first run's driver options; `setup.apply` plays with the Setup's.
    pub first: FirstRun,
    /// The workers' wake policy, computation and trace.
    pub rt: RtOptions,
    /// Record delivered results of latent modules in the session.
    pub boundary_data: bool,
    /// The run records a session or its audio: `setup.apply` saves the
    /// Setup but does not restart it.
    pub recording: bool,
    /// The session's provenance.
    pub provenance: Provenance,
    /// The compute inventory for `setup.devices`.
    pub compute: Vec<String>,
    /// The servers.
    pub server: ServerOptions,
}

/// What Signal.app's server runs ([`Host::new_app`]): the Setup file, the
/// project folder to open (else the Setup's last project), the modules,
/// the devices, the compute inventory and the servers.
pub struct AppHostConfig {
    /// The Setup file the app reads and writes.
    pub setup_path: PathBuf,
    /// The project folder or `.signal` file to open at start (else the
    /// Setup's last one).
    pub project: Option<PathBuf>,
    /// Where opened `.signal` files are unpacked
    /// ([`document::open_root`](crate::document::open_root)).
    pub open_root: PathBuf,
    /// The modules.
    pub registry: Registry,
    /// CoreAudio, or the null driver with a device list.
    pub backend: Backend,
    /// The compute inventory for `setup.devices`.
    pub compute: Vec<String>,
    /// The servers (`project_ui` is the open project's, set by the host).
    pub server: ServerOptions,
}

/// What opening another project (or quitting) does with an edited `.signal`
/// project's changes (M2.8-12): nothing is let go unless asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edits {
    /// Refuse while edited (`edited`), so the caller asks first.
    Ask,
    /// Save them to the file first.
    Save,
    /// Let them go.
    Discard,
}

/// The open project as the window shows it (its title, the close prompt).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectStatus {
    /// The folder the project plays from (a `.signal` file's working folder).
    pub dir: PathBuf,
    /// The `.signal` file, for one.
    pub file: Option<PathBuf>,
    /// Its name: the file's stem, or the folder's name.
    pub name: String,
    /// The working folder differs from the file.
    pub edited: bool,
}

/// Signal.app's state beside the protocol's (app mode): what the app's
/// window showed beside the panel and the meters.
#[derive(Default)]
struct AppMode {
    /// A project folder is open (the host's `dir`).
    opened: bool,
    /// The open folder's diagnostics, when it does not load.
    diagnostics: Vec<String>,
    /// Notes on the run: the channel map, the input.
    notes: Vec<String>,
    /// The last refusal or notice.
    message: Option<String>,
    /// The last file-watch outcome, and whether it was an error.
    reload: Option<(String, bool)>,
    /// The status line (of the last run once stopped).
    status: Option<Status>,
    /// When the status was last pushed.
    pushed: Option<Instant>,
    /// The `app` message last pushed.
    last: Option<Value>,
    /// The power switch (application state, never written anywhere).
    power: app::Power,
}

/// What a stopped host leaves: the recorder and the runner's last status.
pub struct Finished {
    /// The session recorder.
    pub live: LiveRecorder,
    /// The runner's status as it stopped.
    pub status: RunnerStatus,
    /// Samples clipped.
    pub clipped: u64,
    /// Meters dropped.
    pub meter_overflows: u64,
}

/// One run: from a start (the first, or `setup.apply`) to its stop.
struct Run {
    output: Output,
    input: Option<Input>,
    controller: Controller,
    live: LiveRecorder,
    current: Project,
    watcher: Option<ProjectWatcher>,
    watched: Instant,
    retry: bool,
    /// The `project.toml` text `current` is known to hold (read before the
    /// last reload that applied, or written by a commit it took); `None`
    /// when not known.
    file: Option<String>,
    rate: u32,
    buffer: u32,
    threads: u32,
    width: usize,
    input_channels: usize,
    device: String,
    /// The engine's plan version last described.
    version: u64,
    failure_reported: bool,
    /// How the workers are scheduled (app mode: the status line).
    scheduling: Option<Scheduling>,
}

struct Client {
    queue: SyncSender<Outbound>,
    evicted: Arc<AtomicBool>,
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    gate: Arc<AtomicBool>,
    dropped: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct Level {
    peak: f32,
    sum_squares: f64,
    frames: u64,
}

enum Stream {
    Parameters {
        ports: Vec<Option<PortAddress>>,
    },
    Meters {
        input: bool,
        levels: Vec<Level>,
        first: Option<u64>,
    },
    Scope {
        slot: usize,
        port: PortAddress,
        decimation: u64,
        bound: bool,
    },
    Vector {
        slot: usize,
        port: PortAddress,
        rows: usize,
        /// The snapshots per second asked for.
        rate: f64,
        decimation: u64,
        bound: bool,
    },
}

struct Sub {
    client: u64,
    every: Duration,
    last: Instant,
    stream: Stream,
}

/// An edit waiting for its reports.
struct Pending {
    client: u64,
    id: Value,
    remaining: usize,
    frame: u64,
    requested: u64,
    refused: bool,
    /// The value it sends, which a runner's refusal marks as not applied.
    edited: Option<Edited>,
}

/// A live value an edit sent.
enum Edited {
    /// To a control (`instance`, `parameter`).
    Control((String, String), f64),
    /// To a plan parameter port that is no control.
    Port(PortAddress, f64),
}

/// A control's failed write (`docs/ui-protocol.md` "Unsaved"): its error,
/// in order of failures; the live value is the panel's drag, never kept
/// here. `reset`: the write was of `null` (each port to its default), with
/// whether the Controller took the defaults.
struct Failed {
    error: String,
    seq: u64,
    reset: Option<bool>,
}

/// A parameter whose live value the file does not hold, as `unsaved` lists
/// it.
struct Unsaved {
    address: String,
    file: f64,
    in_file: bool,
    control: bool,
    /// The live value is known to be the one last sent.
    applied: bool,
    error: Option<(u64, String)>,
}

/// A refused message: its code and why, and what else the error carries
/// (`panel.write`'s `conflict`: the current text and base).
struct Failure {
    code: &'static str,
    message: String,
    detail: Option<Value>,
}

fn fail(code: &'static str, message: impl Into<String>) -> Failure {
    Failure {
        code,
        message: message.into(),
        detail: None,
    }
}

/// [`fail`], with `detail`'s fields in the error.
fn fail_with(code: &'static str, message: impl Into<String>, detail: Value) -> Failure {
    Failure {
        detail: Some(detail),
        ..fail(code, message)
    }
}

/// Where an edit goes: a control's ports or one plan parameter.
struct Aim {
    ports: Vec<PortAddress>,
    min: f64,
    max: f64,
    control: Option<(String, String)>,
}

impl Aim {
    /// What sending `value` here edits.
    fn edited(&self, value: f64) -> Option<Edited> {
        match &self.control {
            Some(control) => Some(Edited::Control(control.clone(), value)),
            None => self.ports.first().map(|p| Edited::Port(p.clone(), value)),
        }
    }
}

/// The values and reports of one poll, beside the session recorder.
struct Observed<'a> {
    values: &'a mut BTreeMap<PortAddress, ParameterValue>,
    reports: &'a mut Vec<Report>,
}

impl ControlSink for Observed<'_> {
    fn report(&mut self, report: &Report, _command: &SentCommand) {
        self.reports.push(*report);
    }
    fn parameter(&mut self, port: &PortAddress, value: ParameterValue) {
        match self.values.get_mut(port) {
            Some(known) => *known = value,
            None => {
                self.values.insert(port.clone(), value);
            }
        }
    }
    /// A hung callback (M2.7-9): a diagnosis on stderr, not recorded.
    fn hang(&mut self, line: HangLine<'_>) {
        let running: Vec<String> = line.running.iter().map(ToString::to_string).collect();
        eprintln!(
            "signal: hang: callback {} has not returned after {:.1} ms; still running: {}",
            line.callback,
            line.waited.as_secs_f64() * 1e3,
            running.join(", ")
        );
    }
}

/// The protocol's host.
pub struct Host {
    registry: Registry,
    dir: PathBuf,
    setup_path: PathBuf,
    setup: Setup,
    setup_problems: Vec<String>,
    backend: Backend,
    devices: Vec<DeviceInfo>,
    devices_error: Option<String>,
    compute: Vec<String>,
    first: FirstRun,
    rt: RtOptions,
    boundary_data: bool,
    recording: bool,
    provenance: Provenance,
    server: Server,
    run: Option<Run>,
    panel: Panel,
    plan: Option<ExecutionPlan>,
    rate: u32,
    clients: BTreeMap<u64, Client>,
    evicted: u64,
    subs: BTreeMap<u32, Sub>,
    next_sub: u32,
    scope_slots: [Option<u32>; SCOPE_SLOTS],
    vector_slots: [Option<u32>; VECTOR_SLOTS],
    values: BTreeMap<PortAddress, ParameterValue>,
    reports: Vec<Report>,
    meters: Vec<Meter>,
    inputs: Vec<Meter>,
    blocks: Vec<ScopeBlock>,
    snapshots: Vec<VectorSnapshot>,
    commands: BTreeMap<CommandId, u64>,
    acks: BTreeMap<u64, Pending>,
    next_ack: u64,
    writes: u64,
    messages: Vec<String>,
    /// Signal.app's state, in app mode.
    app: Option<AppMode>,
    /// The project as given to `signal ui` (its folder or `.signal` file).
    source: PathBuf,
    /// `signal ui`'s temporary folder of a `.signal` file.
    temporary: Option<TempProject>,
    /// App mode: where `.signal` files are unpacked.
    open_root: PathBuf,
    /// App mode: the open `.signal` file and its working folder.
    document: Option<Document>,
    /// Controls whose last write failed.
    failed: BTreeMap<(String, String), Failed>,
    /// Controls written while their live send was refused: the reload of
    /// the written file sends them the file's value.
    resend: BTreeSet<(String, String)>,
    /// Live edits of plan parameter ports that are no control: the value,
    /// and whether the Controller took it.
    port_edits: BTreeMap<PortAddress, (f64, bool)>,
    next_failure: u64,
    /// The `unsaved` message last pushed.
    unsaved_pushed: Option<Value>,
    /// The `graph` message last pushed (`docs/ui-protocol.md` "graph").
    graph_pushed: Option<Value>,
    /// App mode, stopped and switched on: the project folder's watch, for
    /// the `graph` push (a run has its own), and when it was last polled.
    idle_watch: Option<(ProjectWatcher, Instant)>,
}

fn joined(diagnostics: Vec<Diagnostic>) -> String {
    diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// `object` with `key` set.
fn with(mut object: Value, key: &str, value: Value) -> Value {
    if let Some(map) = object.as_object_mut() {
        map.insert(key.to_owned(), value);
    }
    object
}

/// The top-level instance and parameter of a control address `/<instance>.<parameter>`.
fn control_of(port: &PortAddress) -> Option<(String, String)> {
    let instance = port.instance();
    let name = instance.as_str().strip_prefix('/')?;
    (!name.contains('/')).then(|| (name.to_owned(), port.port().to_owned()))
}

/// `value` in `[min, max]`; never panics, whatever the range (`f64::clamp`
/// does when `min > max`).
/// The base of plan parameter `port` as the plan was built (its files'
/// value, or its default).
fn plan_base(plan: Option<&ExecutionPlan>, port: &PortAddress) -> Option<f64> {
    let instance = plan?
        .instances
        .iter()
        .find(|i| i.address == port.instance())?;
    let descriptor = instance
        .descriptor
        .parameters
        .iter()
        .find(|p| p.name == port.port())?;
    Some(
        instance
            .parameters
            .get(descriptor.name)
            .copied()
            .unwrap_or(descriptor.default),
    )
}

/// A file value as compared: its bits, and whether the file sets it.
fn bits(base: Option<(f64, bool)>) -> Option<(u64, bool)> {
    base.map(|(value, in_file)| (value.to_bits(), in_file))
}

fn within(value: f64, min: f64, max: f64) -> f64 {
    value.max(min).min(max)
}

/// A message's `rate`, `default` without one: a number of at least 1.
fn rate_of(message: &Value, default: f64) -> Result<f64, Failure> {
    let rate = match message.get("rate") {
        None => default,
        Some(rate) => rate.as_f64().unwrap_or(f64::NAN),
    };
    if rate.is_finite() && rate >= 1.0 {
        Ok(rate)
    } else {
        Err(fail("bad-rate", "rate must be a number of at least 1"))
    }
}

/// The interval of a parameter or meter stream: its `rate` (default 30)
/// from 1 to 60 per second, faster ones taken as 60.
fn rate_interval(message: &Value) -> Result<Duration, Failure> {
    let rate = rate_of(message, 30.0)?.min(MAX_RATE_HZ);
    Duration::try_from_secs_f64(1.0 / rate).map_err(|e| fail("bad-rate", e.to_string()))
}

/// A scope's decimation at `sample_rate`: its `rate` in points per second
/// (default and at most the sample rate; at least 1).
fn scope_decimation(message: &Value, sample_rate: u32) -> Result<u64, Failure> {
    let sample_rate = f64::from(sample_rate.max(1));
    let rate = rate_of(message, sample_rate)?.min(sample_rate);
    // From 1 to the sample rate, so this is a finite number of frames.
    Ok((sample_rate / rate).round().max(1.0) as u64)
}

/// A vector stream's decimation: its `rate` in snapshots per second
/// (default 60; at least 1, at most the port's tick rate), as a whole number
/// of the port's ticks, so every snapshot is a tick.
fn vector_decimation(rate: f64, sample_rate: u32, divisor: u64) -> u64 {
    let divisor = divisor.max(1);
    let ticks = f64::from(sample_rate.max(1)) / divisor as f64;
    let rate = rate.min(ticks.max(1.0));
    divisor * (ticks / rate).round().max(1.0) as u64
}

/// Vector output `port` of `plan`: its elements and rate divisor
/// ([`signal_output::vector_shape`]); `None` without a plan.
fn vector_shape(plan: Option<&ExecutionPlan>, port: &PortAddress) -> Option<(usize, u64)> {
    signal_output::vector_shape(plan?, port)
}

impl Host {
    /// Starts the servers and plays the project with `config.first`.
    pub fn new(config: HostConfig) -> Result<Host, String> {
        let server = Server::start(&config.server)
            .map_err(|error| format!("the UI server did not start: {error}"))?;
        Host::with_server(config, server)
    }

    /// Test hooks: a host whose servers bind nothing; clients connect with
    /// [`connect_local`](Host::connect_local) and are served by the same
    /// WebSocket connection code as over TCP.
    #[cfg(feature = "test-hooks")]
    pub fn new_local(config: HostConfig) -> Result<Host, String> {
        let server = Server::start_local(&config.server)
            .map_err(|error| format!("the UI server did not start: {error}"))?;
        Host::with_server(config, server)
    }

    /// Test hooks: a new client's end of an in-process connection (a
    /// [`new_local`](Host::new_local) host's only way in).
    #[cfg(feature = "test-hooks")]
    pub fn connect_local(&self) -> crate::LocalStream {
        self.server.connect_local()
    }

    /// Test hooks: a new HTTP client's end of an in-process connection,
    /// served by the HTTP workers (its requests carry `Host: 127.0.0.1:0`).
    #[cfg(feature = "test-hooks")]
    pub fn connect_local_http(&self) -> crate::LocalStream {
        self.server.local_connector().connect_http()
    }

    /// Signal.app's server (app mode): starts the servers, opens the project
    /// (else the Setup's last one) without playing it.
    pub fn new_app(config: AppHostConfig) -> Result<Host, String> {
        let server = Server::start(&config.server)
            .map_err(|error| format!("the UI server did not start: {error}"))?;
        Ok(Host::with_app(config, server))
    }

    /// Test hooks: [`new_app`](Host::new_app) with servers that bind
    /// nothing (clients connect in process).
    #[cfg(feature = "test-hooks")]
    pub fn new_app_local(config: AppHostConfig) -> Result<Host, String> {
        let server = Server::start_local(&config.server)
            .map_err(|error| format!("the UI server did not start: {error}"))?;
        Ok(Host::with_app(config, server))
    }

    fn with_app(config: AppHostConfig, server: Server) -> Host {
        let AppHostConfig {
            setup_path,
            project,
            open_root,
            registry,
            backend,
            compute,
            server: options,
        } = config;
        // App mode plays every run from the Setup (`app::start`): the
        // first run's options and `rt` are not used.
        let config = HostConfig {
            dir: PathBuf::new(),
            source: PathBuf::new(),
            temporary: None,
            registry,
            setup_path,
            backend,
            first: FirstRun {
                device: None,
                buffer: DEFAULT_BUFFER,
                threads: 0,
            },
            rt: RtOptions::default(),
            boundary_data: false,
            recording: false,
            provenance: Provenance::default(),
            compute,
            server: options,
        };
        let mut host = Host::blank(config, server, Some(AppMode::default()));
        host.open_root = open_root;
        if let Some(path) = app::initial_project(project, &host.setup) {
            // A refused .signal opens nothing; the window says why.
            if let Err(message) = host.open_path(&path) {
                if let Some(mode) = host.app.as_mut() {
                    mode.message = Some(message);
                }
            } else {
                // Saved with the next change to the Setup.
                app::opened(&mut host.setup, path);
            }
        }
        host
    }

    fn with_server(config: HostConfig, server: Server) -> Result<Host, String> {
        let mut host = Host::blank(config, server, None);
        let (run, notes) = host.build(true)?;
        host.messages.extend(notes);
        host.install(run);
        Ok(host)
    }

    fn blank(config: HostConfig, server: Server, app: Option<AppMode>) -> Host {
        let (setup, setup_problems) = Setup::load(&config.setup_path);
        let mut host = Host {
            registry: config.registry,
            dir: config.dir,
            setup_path: config.setup_path,
            setup,
            setup_problems,
            backend: config.backend,
            devices: Vec::new(),
            devices_error: None,
            compute: config.compute,
            first: config.first,
            rt: config.rt,
            boundary_data: config.boundary_data,
            recording: config.recording,
            provenance: config.provenance,
            server,
            run: None,
            panel: Panel::default(),
            plan: None,
            rate: 0,
            clients: BTreeMap::new(),
            evicted: 0,
            subs: BTreeMap::new(),
            next_sub: 1,
            scope_slots: [None; SCOPE_SLOTS],
            vector_slots: [None; VECTOR_SLOTS],
            values: BTreeMap::new(),
            reports: Vec::new(),
            meters: Vec::new(),
            inputs: Vec::new(),
            blocks: Vec::new(),
            snapshots: Vec::new(),
            commands: BTreeMap::new(),
            acks: BTreeMap::new(),
            next_ack: 1,
            writes: 0,
            messages: Vec::new(),
            app,
            source: config.source,
            temporary: config.temporary,
            open_root: PathBuf::new(),
            document: None,
            failed: BTreeMap::new(),
            resend: BTreeSet::new(),
            port_edits: BTreeMap::new(),
            next_failure: 1,
            unsaved_pushed: None,
            graph_pushed: None,
            idle_watch: None,
        };
        host.list_devices();
        host
    }

    /// The URL to open: the page, with the WebSocket port and the session
    /// token in its fragment.
    pub fn url(&self) -> String {
        self.server.url()
    }

    /// The URL of the project's main panel, with the same fragment.
    pub fn panel_url(&self) -> String {
        self.server.panel_url()
    }

    /// The URL of the page at `path` (from `/`, e.g. [`crate::APP_PAGE`]),
    /// with the same fragment.
    pub fn page_url(&self, path: &str) -> String {
        self.server.url_of(path)
    }

    /// Test hooks: what connects in-process clients from another thread.
    #[cfg(feature = "test-hooks")]
    pub fn local_connector(&self) -> crate::LocalConnector {
        self.server.local_connector()
    }

    /// Signal.app exits: stops playing, records the window's size (when
    /// known) in the Setup and saves it. A `.signal` project's working
    /// folder is removed, unless it is edited and `discard` is false: then it
    /// is kept, and the message says where. Returns what to tell the operator.
    pub fn close(&mut self, window: Option<Window>, discard: bool) -> Vec<String> {
        self.stop_app();
        if let Some(document) = self.document.take() {
            let name = document.name();
            if let Some(kept) = document.close(discard) {
                self.messages.push(format!(
                    "the unsaved edits of {name} are kept in {}",
                    kept.display()
                ));
            }
        }
        app::exit(&mut self.setup, window);
        if let Err(message) = app::save(&self.setup, &self.setup_path) {
            self.messages.push(message);
        }
        std::mem::take(&mut self.messages)
    }

    /// The HTTP server's address.
    pub fn http_addr(&self) -> std::net::SocketAddr {
        self.server.http_addr()
    }

    /// The WebSocket's address.
    pub fn ws_addr(&self) -> std::net::SocketAddr {
        self.server.ws_addr()
    }

    /// The session token.
    pub fn token(&self) -> &str {
        self.server.token()
    }

    /// The running Controller, if playing.
    pub fn controller(&self) -> Option<&Controller> {
        self.run.as_ref().map(|r| &r.controller)
    }

    /// The project's sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// Null driver only: runs `frames` frames, handing each callback's
    /// output to `capture`.
    pub fn run_null(&mut self, frames: u64, capture: impl FnMut(&[f32])) -> Result<(), String> {
        match self.run.as_mut().and_then(|r| r.output.null()) {
            Some(driver) => driver
                .run_with(frames, || {}, capture)
                .map_err(|e| e.to_string()),
            None => Err("not playing on the null driver".to_owned()),
        }
    }

    /// Null input only: one input callback of `data`.
    pub fn push_input(&mut self, data: &[f32]) -> Result<(), String> {
        match self.run.as_mut().and_then(|r| r.input.as_mut()) {
            Some(input) => input.push(data),
            None => Err("no input is metered".to_owned()),
        }
    }

    /// Test hooks: how long a connection accepted from now on has for its
    /// handshake and `hello` with the token (5 s by default).
    #[cfg(feature = "test-hooks")]
    pub fn set_admission_deadline(&self, deadline: Duration) {
        self.server.set_admission_deadline(deadline);
    }

    /// Test hooks: holds (or releases) client `client`'s writer, so its
    /// queue fills as if its socket took nothing.
    #[cfg(feature = "test-hooks")]
    pub fn hold_writer(&self, client: u64, held: bool) -> bool {
        match self.clients.get(&client) {
            Some(c) => {
                c.gate.store(held, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    /// Test hooks: the open `.signal` project, for one.
    #[cfg(feature = "test-hooks")]
    pub fn document(&self) -> Option<&Document> {
        self.document.as_ref()
    }

    /// Stops the run; the Runner is dropped here.
    pub fn finish(mut self) -> Option<Finished> {
        let mut run = self.run.take()?;
        let runner = run.output.stop().ok().flatten();
        run.controller.poll(&mut run.live);
        let (status, clipped, meter_overflows) = match &runner {
            Some(r) => (r.status(), r.clipped(), r.meter_overflows()),
            None => (
                run.controller.status(),
                run.controller.clipped(),
                run.controller.meter_overflows(),
            ),
        };
        Some(Finished {
            live: run.live,
            status,
            clipped,
            meter_overflows,
        })
    }

    /// One control step: the runner's reports and observations, file
    /// changes every 100 ms, the clients' messages and the streams due.
    /// Returns what to tell the operator.
    pub fn step(&mut self) -> Vec<String> {
        self.poll();
        self.watch();
        self.watch_stopped();
        for _ in 0..MESSAGES_PER_STEP {
            match self.server.inbound.try_recv() {
                Ok(message) => self.inbound(message),
                Err(_) => break,
            }
        }
        self.emit();
        self.push_app();
        self.push_unsaved();
        std::mem::take(&mut self.messages)
    }

    /// `unsaved` to every client whenever it changed; a failure whose
    /// control is no longer unsaved (live and file agree again) is dropped.
    fn push_unsaved(&mut self) {
        let listed: BTreeSet<String> = self.unsaved().into_iter().map(|u| u.address).collect();
        self.failed
            .retain(|(i, p), _| listed.contains(&format!("/{i}.{p}")));
        self.port_edits
            .retain(|port, _| listed.contains(port.as_str()));
        let current = self.unsaved_json();
        if self.unsaved_pushed.as_ref() != Some(&current) {
            self.broadcast(&with(current.clone(), "type", json!("unsaved")));
            self.unsaved_pushed = Some(current);
        }
    }

    /// Every parameter whose live value the file does not hold, or may not
    /// (`docs/ui-protocol.md` "Unsaved"): the controls in the panel's order,
    /// then the plan parameter ports edited live.
    fn unsaved(&self) -> Vec<Unsaved> {
        let mut list = Vec::new();
        for instance in self.panel.view(&self.values) {
            for p in &instance.parameters {
                let key = (instance.name.clone(), p.name.clone());
                let Some((file, in_file)) = self.panel.file_base(&key.0, &key.1) else {
                    continue;
                };
                let drag = self.panel.dragged(&key.0, &key.1);
                let failed = self.failed.get(&key);
                let resend = self.resend.contains(&key);
                let unsent = drag.is_some_and(|d| !d.sent)
                    || failed.is_some_and(|f| f.reset == Some(false))
                    || resend;
                let differs = drag.is_some_and(|d| d.value.to_bits() != file.to_bits())
                    || (drag.is_none() && failed.is_some_and(|f| f.reset.is_some()) && in_file);
                if unsent || differs {
                    list.push(Unsaved {
                        address: format!("/{}.{}", key.0, key.1),
                        file,
                        in_file,
                        control: true,
                        applied: !unsent,
                        error: failed.map(|f| (f.seq, f.error.clone())),
                    });
                }
            }
        }
        for (port, &(value, sent)) in &self.port_edits {
            let Some(file) = plan_base(self.plan.as_ref(), port) else {
                continue;
            };
            if !sent || value.to_bits() != file.to_bits() {
                list.push(Unsaved {
                    address: port.as_str().to_owned(),
                    file,
                    in_file: false,
                    control: false,
                    applied: sent,
                    error: None,
                });
            }
        }
        list
    }

    /// `unsaved`'s body (also `describe`'s `unsaved`).
    fn unsaved_json(&self) -> Value {
        let list = self.unsaved();
        let last = list
            .iter()
            .filter_map(|u| u.error.as_ref())
            .max_by_key(|(seq, _)| *seq)
            .map(|(_, error)| error.clone());
        let parameters: Vec<Value> = list
            .iter()
            .map(|u| {
                json!({
                    "address": u.address,
                    "file": u.file,
                    "inFile": u.in_file,
                    "control": u.control,
                    "applied": u.applied,
                    "error": u.error.as_ref().map(|(_, e)| e),
                })
            })
            .collect();
        json!({"count": list.len(), "parameters": parameters, "lastError": last})
    }

    /// App mode: the status line every [`STATUS_EVERY`] while playing, and
    /// the `app` message whenever it changed.
    fn push_app(&mut self) {
        if let Some(document) = self.document.as_mut() {
            document.refresh();
        }
        let Some(mode) = self.app.as_mut() else {
            return;
        };
        let due = match &self.run {
            Some(run) => {
                let due = mode.pushed.is_none_or(|at| at.elapsed() >= STATUS_EVERY);
                if due && let Some(scheduling) = &run.scheduling {
                    mode.status = Some(app::status(
                        &run.controller,
                        &run.output,
                        run.input.as_ref(),
                        run.rate,
                        run.buffer,
                        scheduling,
                    ));
                }
                due
            }
            // Once after a stop: the last run's final status.
            None => mode.pushed.is_none() && mode.status.is_some(),
        };
        if due {
            mode.pushed = Some(Instant::now());
            let status = self.status_json();
            self.broadcast(&status);
        }
        let current = self.app_json();
        if self.app.as_ref().and_then(|m| m.last.as_ref()) != Some(&current) {
            self.broadcast(&current);
            if let Some(mode) = self.app.as_mut() {
                mode.last = Some(current);
            }
        }
    }

    /// `status`: the status line of the run (of the last one once stopped).
    fn status_json(&self) -> Value {
        let status = self.app.as_ref().and_then(|m| m.status.as_ref());
        json!({
            "type": "status",
            "playing": self.run.is_some(),
            "line": app::status_line(status),
            "status": status.map(|s| json!({
                "frame": s.frame,
                "elapsed": s.elapsed,
                "xruns": s.xruns,
                "loadAverage": s.load_average,
                "loadWorst": s.load_worst,
                "clipped": s.clipped,
                "buffer": s.buffer,
                "threads": s.threads,
                "workers": s.workers,
                "scheduling": s.scheduling,
            })),
        })
    }

    /// `app`: what Signal.app's window shows beside the panel and meters.
    fn app_json(&self) -> Value {
        let Some(mode) = &self.app else {
            return Value::Null;
        };
        json!({
            "type": "app",
            "project": mode.opened.then(|| json!({
                "dir": self.dir.display().to_string(),
                "file": self.document.as_ref().map(|d| d.file().display().to_string()),
                "name": self.project_name(),
                "edited": self.document.as_ref().is_some_and(Document::edited),
                "sampleRate": (mode.diagnostics.is_empty() && self.rate > 0).then_some(self.rate),
                "diagnostics": mode.diagnostics,
            })),
            "playing": self.run.is_some(),
            "power": power_json(mode.power),
            "reload": mode.reload.as_ref().map(|(message, error)| json!({"message": message, "error": error})),
            "notes": mode.notes,
            "message": mode.message,
            "writes": self.writes,
            "setupPath": self.setup_path.display().to_string(),
            "setupProblems": self.setup_problems,
        })
    }

    /// App mode: opens the project folder `dir` (stopped) for its rate,
    /// panel and diagnostics, and serves its `ui/`.
    fn open(&mut self, dir: &Path) {
        self.dir = dir.to_owned();
        self.idle_watch = None;
        self.server.set_project_ui(project_ui(dir));
        let loaded = app::load(dir, &self.registry);
        let Some(mode) = self.app.as_mut() else {
            return;
        };
        mode.opened = true;
        mode.reload = None;
        match loaded {
            Ok((project, plan, _)) => {
                self.rate = project.sample_rate;
                mode.diagnostics.clear();
                self.panel = Panel::new(&project, &self.registry);
                self.plan = Some(plan);
            }
            Err(diagnostics) => {
                self.rate = 0;
                mode.diagnostics = diagnostics;
                self.panel = Panel::default();
                self.plan = None;
            }
        }
    }

    /// App mode: plays the open project from frame 0 with the Setup
    /// ([`app::start`]), as the app's Play does.
    fn play_app(&mut self) -> Result<(), Failure> {
        let opened = match self.app.as_mut() {
            Some(mode) => {
                mode.notes.clear();
                mode.opened
            }
            None => return Err(app_only()),
        };
        if self.run.is_some() {
            return Ok(());
        }
        if !opened {
            return Err(fail("no-project", "open a project folder first"));
        }
        // Read before the load: a later edit makes it differ from the file.
        let file = fs::read_to_string(self.dir.join(signal_project::PROJECT_FILE)).ok();
        let (project, plan, width) = match app::load(&self.dir, &self.registry) {
            Ok(loaded) => loaded,
            Err(diagnostics) => {
                if let Some(mode) = self.app.as_mut() {
                    mode.diagnostics = diagnostics;
                }
                return Err(fail("does-not-load", "the project does not load"));
            }
        };
        self.rate = project.sample_rate;
        // The unsaved controls stay unsaved: their values, kept while the
        // run starts, are played again in it.
        let carried = self.carried();
        self.panel = Panel::new(&project, &self.registry);
        self.play_carried(carried.clone());
        let mut notes = Vec::new();
        let started = app::start(
            &self.backend,
            &self.devices,
            &self.setup,
            &self.registry,
            &project,
            plan,
            width,
            &mut notes,
        );
        if let Some(mode) = self.app.as_mut() {
            mode.diagnostics.clear();
            mode.notes.extend(notes);
        }
        let started = started.map_err(|error| fail("device", error))?;
        let (watcher, reload) = match ProjectWatcher::new(&self.dir) {
            Ok(watcher) => (Some(watcher), ("watching for edits".to_owned(), false)),
            Err(error) => (None, (format!("not watching: {error}"), true)),
        };
        if let Some(mode) = self.app.as_mut() {
            mode.reload = Some(reload);
            mode.pushed = None;
        }
        let (input, input_channels) = match started.input {
            Some((input, channels)) => (Some(input), channels),
            None => (None, 0),
        };
        let device = device_name(&started.output);
        let run = Run {
            output: started.output,
            input,
            controller: started.controller,
            live: LiveRecorder::new(started.recorder, self.registry.clone()),
            current: project,
            watcher,
            watched: Instant::now(),
            retry: false,
            file,
            rate: started.rate,
            buffer: started.buffer,
            threads: u32::try_from(started.scheduling.threads).unwrap_or(u32::MAX),
            width,
            input_channels,
            device,
            version: 0,
            failure_reported: false,
            scheduling: Some(started.scheduling),
        };
        self.install(run);
        self.rearm();
        self.play_carried(carried);
        Ok(())
    }

    /// Stops the run, as the app's Stop does: its last status stays, edits
    /// waiting for reports are abandoned, subscriptions are kept (scopes
    /// start again with the next run). The unsaved controls keep their values
    /// (played again by the next run); live edits of plan ports end.
    fn stop_app(&mut self) {
        let Some(mut run) = self.run.take() else {
            return;
        };
        let stopped = run.output.stop();
        run.controller.poll(&mut run.live);
        if let Some(mode) = self.app.as_mut() {
            if let Err(error) = &stopped {
                mode.message = Some(format!("stopping: {error}"));
            }
            if let Some(scheduling) = &run.scheduling {
                let mut status = app::status(
                    &run.controller,
                    &run.output,
                    run.input.as_ref(),
                    run.rate,
                    run.buffer,
                    scheduling,
                );
                if let Ok(Some(runner)) = &stopped {
                    status.frame = runner.status().next_frame.0;
                    status.clipped = runner.clipped();
                    status.elapsed = status.frame as f64 / f64::from(run.rate.max(1));
                }
                mode.status = Some(status);
            }
            // The final status is pushed by the next step.
            mode.pushed = None;
        }
        // The streams, the Runner and the input are dropped here, on the
        // control thread.
        drop(run);
        self.values.clear();
        self.resend.clear();
        self.port_edits.clear();
        self.abandon();
    }

    /// `transport.play`, `transport.stop` (app mode).
    fn transport(&mut self, play: bool) -> Result<Value, Failure> {
        if self.app.is_none() {
            return Err(app_only());
        }
        let result = if play && !self.powered() {
            Err(powered_off())
        } else if play {
            self.play_app()
        } else {
            if self.run.is_some() {
                self.stop_app();
                self.list_devices();
            }
            Ok(())
        };
        self.noted(result)?;
        Ok(json!({"type": "transport", "playing": self.run.is_some()}))
    }

    /// `app.power` (app mode): off stops the run (the project, its panel and
    /// its unsaved values stay, as with Stop) and lets go of every
    /// subscription, so nothing runs and nothing streams; on plays again if
    /// it played when switched off.
    fn power(&mut self, message: &Value) -> Result<Value, Failure> {
        let on = message
            .get("on")
            .and_then(Value::as_bool)
            .ok_or_else(|| fail("bad-message", "app.power needs `on`: true or false"))?;
        let playing = self.run.is_some();
        let Some(mode) = self.app.as_mut() else {
            return Err(app_only());
        };
        let result = if on {
            if mode.power.on() {
                self.play_app()
            } else {
                Ok(())
            }
        } else {
            mode.power.off(playing);
            // The device is released, the workers joined (the Runner is
            // dropped here) and the file watch ends with the run.
            self.stop_app();
            let subs: Vec<u32> = self.subs.keys().copied().collect();
            for sub in subs {
                self.drop_sub(sub);
            }
            Ok(())
        };
        self.noted(result)?;
        Ok(json!({"type": "power", "on": self.powered(), "playing": self.run.is_some()}))
    }

    /// Whether the app is switched on (always outside app mode).
    pub fn powered(&self) -> bool {
        self.app.as_ref().is_none_or(|m| m.power.is_on())
    }

    /// Waits up to `timeout` for a client's message and handles it: the
    /// control thread's loop while the app is switched off, which then has
    /// nothing to poll.
    pub fn wait(&mut self, timeout: Duration) {
        if let Ok(message) = self.server.inbound.recv_timeout(timeout) {
            self.inbound(message);
        }
    }

    /// `project.open`. App mode: stops, opens the folder or `.signal` file at
    /// `path` (made absolute: it becomes the Setup's last project) and saves
    /// the Setup; while a `.signal` project is edited it is refused
    /// (`edited`) unless `unsaved` is `"save"` or `"discard"`. `signal ui`:
    /// only the project it serves, which is reopened ([`Host::reopen`]).
    fn project_open(&mut self, message: &Value) -> Result<Value, Failure> {
        let path = message
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| fail("bad-message", "project.open needs `path`"));
        if self.app.is_none() {
            return self.reopen(path?);
        }
        let path = path?;
        let edits = match message.get("unsaved").and_then(Value::as_str) {
            None => Edits::Ask,
            Some("save") => Edits::Save,
            Some("discard") => Edits::Discard,
            Some(other) => {
                return Err(fail(
                    "bad-message",
                    format!("`unsaved` is \"save\" or \"discard\", not \"{other}\""),
                ));
            }
        };
        let path = std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path));
        self.open_with(&path, edits)
    }

    /// App mode: opens the folder (or its `project.toml`) or the `.signal`
    /// file at `path`, as `project.open` does; the error is what to show.
    pub fn open_project(&mut self, path: &Path, edits: Edits) -> Result<(), String> {
        match self.open_with(path, edits) {
            // As after a `project.open`: every client is told.
            Ok(_) => {
                let describe = self.describe(false);
                self.broadcast(&describe);
                self.push_graph(false);
                Ok(())
            }
            Err(failure) => {
                if let Some(mode) = self.app.as_mut() {
                    mode.message = Some(failure.message.clone());
                }
                Err(failure.message)
            }
        }
    }

    fn open_with(&mut self, path: &Path, edits: Edits) -> Result<Value, Failure> {
        if self.app.is_none() {
            return Err(app_only());
        }
        let edited = self.document.as_mut().is_some_and(Document::check);
        if edited && edits == Edits::Ask {
            return Err(fail(
                "edited",
                format!("Save changes to {}?", self.project_name()),
            ));
        }
        // Saved first, so that reopening the same file opens what was saved.
        if edited && edits == Edits::Save {
            self.save_document().map_err(|m| fail("not-saved", m))?;
        }
        // The new project is unpacked before anything stops: a refused one
        // changes nothing.
        // A folder named `*.signal` is a folder.
        let prepared = if archive::is_project_file(path) && !path.is_dir() {
            Some(Document::open(path, &self.open_root).map_err(|m| fail("archive", m))?)
        } else {
            None
        };
        if let Some(mode) = self.app.as_mut() {
            mode.message = None;
            // Opened stopped: switching on again plays nothing.
            mode.power.forget();
        }
        // Another project: the unsaved values of this one are let go, and
        // the window says so.
        let unsaved = self.unsaved().len();
        self.stop_app();
        if let Some(previous) = self.document.take() {
            // Not edited, saved, or let go on request.
            previous.close(true);
        }
        let dir = match &prepared {
            Some(document) => document.working().to_owned(),
            None => folder_of(path),
        };
        self.document = prepared;
        self.open(&dir);
        self.failed.clear();
        app::opened(&mut self.setup, path.to_owned());
        let saved = app::save(&self.setup, &self.setup_path);
        let mode = self.app.as_mut();
        if let Some(mode) = mode {
            mode.message = match (&saved, unsaved) {
                (Err(message), _) => Some(message.clone()),
                (Ok(()), 0) => None,
                (Ok(()), 1) => {
                    Some("1 unsaved edit of the previous project was not written".to_owned())
                }
                (Ok(()), n) => Some(format!(
                    "{n} unsaved edits of the previous project were not written"
                )),
            };
        }
        let diagnostics = self
            .app
            .as_ref()
            .map_or_else(Vec::new, |m| m.diagnostics.clone());
        Ok(json!({
            "type": "project",
            "dir": dir.display().to_string(),
            "file": self.document.as_ref().map(|d| d.file().display().to_string()),
            "loads": diagnostics.is_empty(),
            "diagnostics": diagnostics,
        }))
    }

    /// App mode, at start: opens `path`, a folder or a `.signal` file.
    fn open_path(&mut self, path: &Path) -> Result<(), String> {
        if archive::is_project_file(path) && !path.is_dir() {
            let document = Document::open(path, &self.open_root)?;
            let dir = document.working().to_owned();
            self.document = Some(document);
            self.open(&dir);
        } else {
            self.open(&folder_of(path));
        }
        Ok(())
    }

    /// The open project's name: its file's stem, or its folder's name.
    fn project_name(&self) -> String {
        match &self.document {
            Some(document) => document.name(),
            None => project_name(&self.dir),
        }
    }

    /// App mode: the open project as the window shows it, its edited state
    /// looked at now; `None` without one.
    pub fn project_status(&mut self) -> Option<ProjectStatus> {
        if let Some(document) = self.document.as_mut() {
            document.check();
        }
        self.project_shown()
    }

    /// App mode: the open project as last looked at (each step looks at
    /// most every 250 ms); `None` without one.
    pub fn project_shown(&self) -> Option<ProjectStatus> {
        if !self.app.as_ref().is_some_and(|m| m.opened) {
            return None;
        }
        Some(ProjectStatus {
            dir: self.dir.clone(),
            file: self.document.as_ref().map(|d| d.file().to_owned()),
            name: self.project_name(),
            edited: self.document.as_ref().is_some_and(Document::edited),
        })
    }

    /// Save of a `.signal` project: the working folder written back to the
    /// file atomically. A folder project has nothing to write here (its
    /// parameters are written by `retry`).
    pub fn save_document(&mut self) -> Result<(), String> {
        match self.document.as_mut() {
            Some(document) => document.save(),
            None => Ok(()),
        }
    }

    /// Save As (app mode): the project written to the `.signal` file
    /// `file`, which it is from then on (the Setup's last project). A folder
    /// project's folder is left as it is: the project is opened again, from
    /// the new file, stopped.
    pub fn save_as(&mut self, file: &Path) -> Result<(), String> {
        if self.app.is_none() {
            return Err(app_only().message);
        }
        let saved = match self.document.as_mut() {
            Some(document) => document.save_as(file),
            None => {
                let bytes = archive::pack(&self.dir)
                    .map_err(|e| format!("{} was not saved: {e}", file.display()))?;
                archive::write_atomic(file, &bytes)
                    .map_err(|e| format!("{} was not saved: {e}", file.display()))?;
                return self.open_project(file, Edits::Discard);
            }
        };
        saved?;
        app::opened(&mut self.setup, file.to_owned());
        app::save(&self.setup, &self.setup_path)
    }

    /// `project.save` (app mode): [`Host::save_document`].
    fn project_save(&mut self) -> Result<Value, Failure> {
        if self.app.is_none() {
            return Err(app_only());
        }
        let saved = self.save_document().map_err(|m| fail("not-saved", m));
        self.noted(saved)?;
        Ok(json!({
            "type": "saved",
            "file": self.document.as_ref().map(|d| d.file().display().to_string()),
        }))
    }

    /// `signal ui`'s `project.open`: only the project it serves (its folder,
    /// its `project.toml`, or the `.signal` file it was started with), which
    /// is played again from frame 0 from its files: a `.signal` file is
    /// unpacked again into a new temporary folder. Anything else is refused
    /// (`docs/ui-protocol.md` "Security notes").
    fn reopen(&mut self, path: &str) -> Result<Value, Failure> {
        let asked = Path::new(path);
        let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        let served = same(asked, &self.source)
            || (asked.file_name().is_some_and(|n| n == PROJECT_FILE)
                && asked.parent().is_some_and(|p| same(p, &self.source)));
        if !served {
            return Err(app_only());
        }
        if self.recording {
            return Err(fail(
                "recording",
                "a run that records a session or its audio is not reopened (a recording covers one run from frame 0)",
            ));
        }
        let temporary = match &self.temporary {
            Some(_) => Some(
                TempProject::unpack(&self.source)
                    .map_err(|e| fail("archive", format!("{}: {e}", self.source.display())))?,
            ),
            None => None,
        };
        if let Some(old) = self.run.take() {
            stop(old);
        }
        self.abandon();
        self.failed.clear();
        if let Some(temporary) = temporary {
            self.dir = temporary.dir().to_owned();
            // The previous copy is removed once its run is stopped.
            self.temporary = Some(temporary);
            self.server.set_project_ui(project_ui(&self.dir));
        }
        let built = self.build(true);
        let error = built.as_ref().err().cloned();
        if let Ok((run, notes)) = built {
            self.install(run);
            self.rearm();
            self.messages.extend(notes);
        }
        Ok(json!({
            "type": "project",
            "dir": self.dir.display().to_string(),
            "file": self.temporary.as_ref().map(|_| self.source.display().to_string()),
            "loads": error.is_none(),
            "diagnostics": error.into_iter().collect::<Vec<_>>(),
        }))
    }

    /// App mode: the message to show for an app message's outcome (cleared
    /// by each one, as the app's commands do).
    fn noted<T>(&mut self, result: Result<T, Failure>) -> Result<T, Failure> {
        if let Some(mode) = self.app.as_mut() {
            mode.message = result.as_ref().err().map(|f| f.message.clone());
        }
        result
    }

    fn list_devices(&mut self) {
        let listed = match &self.backend {
            Backend::CoreAudio => signal_audio::devices(),
            Backend::Null(devices) => Ok(devices.clone()),
        };
        match listed {
            Ok(devices) => {
                self.devices = devices;
                self.devices_error = None;
            }
            Err(error) => {
                self.devices = Vec::new();
                self.devices_error = Some(error.to_string());
            }
        }
    }

    /// Plays the folder's project: with the first run's options, or with
    /// the Setup's (as the dialog's Apply plays again).
    fn build(&self, first: bool) -> Result<(Run, Vec<String>), String> {
        let dir = &self.dir;
        // Read before the load: a later edit makes it differ from the file.
        let file = fs::read_to_string(dir.join(signal_project::PROJECT_FILE)).ok();
        let project = Project::load(dir).map_err(|error| match error {
            LoadError::Invalid(diagnostics) => joined(located(dir, diagnostics)),
            error => error.to_string(),
        })?;
        let mut plan = project
            .plan(&self.registry)
            .map_err(|d| joined(located(dir, d)))?
            .plan;
        plan.version = 0;
        let (_, width) = sink_of(&project, &plan).map_err(|d| joined(located(dir, d)))?;
        let (rate, seed) = (project.sample_rate, project.seed);
        let mut notes = Vec::new();
        let (mut output, buffer, threads) = if first {
            let output = match &self.backend {
                Backend::CoreAudio => Output::Device(Box::new(
                    CoreAudioDriver::open(
                        self.first.device.as_deref(),
                        rate,
                        self.first.buffer,
                        width,
                    )
                    .map_err(|e| e.to_string())?,
                )),
                Backend::Null(_) => Output::Null(
                    Box::new(NullDriver::new(rate, width, seed)),
                    ChannelMap::new(width, width),
                ),
            };
            (output, self.first.buffer, self.first.threads)
        } else {
            let output = open_output(
                &self.backend,
                &self.devices,
                &self.setup,
                rate,
                width,
                seed,
                &mut notes,
            )?;
            (
                output,
                self.setup.buffer,
                self.setup.cpu_workers.unwrap_or(0),
            )
        };
        let mut config = RealtimeConfig::with_buffer(rate, seed, u64::from(buffer));
        config.engine.threads = threads as usize;
        config.rt = self.rt;
        if self.boundary_data {
            // About a second of delivered results per latent instance.
            let latent = plan
                .instances
                .iter()
                .filter(|i| i.descriptor.latency.is_some())
                .count();
            config.engine.record_boundary_data = true;
            config.boundary_data = (rate as usize).saturating_mul(latent.max(1));
        }
        let (runner, controller) = realtime_with(
            &plan,
            &self.registry,
            config,
            Some(rt_thread::mach_semaphores),
        )
        .map_err(|e| e.to_string())?;
        let recorder = SessionRecorder::new(
            &project,
            &self.registry,
            rate,
            seed,
            self.provenance.clone(),
        )
        .map_err(|e| e.to_string())?;
        let live = LiveRecorder::new(recorder, self.registry.clone());
        let live = if self.boundary_data {
            live.with_boundary_data()
        } else {
            live
        };
        let device = device_name(&output);
        output.start(runner).map_err(|e| e.to_string())?;
        let input = if first {
            None
        } else {
            open_input(&self.backend, &self.devices, &self.setup, rate, &mut notes)
        };
        let (input, input_channels) = match input {
            Some((input, channels)) => (Some(input), channels),
            None => (None, 0),
        };
        let watcher = match ProjectWatcher::new(dir) {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                notes.push(format!("not watching {}: {error}", dir.display()));
                None
            }
        };
        let run = Run {
            output,
            input,
            controller,
            live,
            current: project,
            watcher,
            watched: Instant::now(),
            retry: false,
            file,
            rate,
            buffer,
            threads,
            width,
            input_channels,
            device,
            version: 0,
            failure_reported: false,
            scheduling: None,
        };
        Ok((run, notes))
    }

    /// A new run plays the file: no live edit is left in it (the unsaved
    /// controls' values are played again by [`Host::play_carried`]).
    fn install(&mut self, mut run: Run) {
        self.panel = Panel::new(&run.current, &self.registry);
        self.plan = run.current.plan(&self.registry).ok().map(|p| p.plan);
        self.rate = run.rate;
        run.version = run.controller.status().version;
        self.values.clear();
        self.resend.clear();
        self.port_edits.clear();
        self.run = Some(run);
    }

    /// Drains the Controller: reports and values (beside the recorder),
    /// meters, scopes, input meters; a new plan version is described again.
    fn poll(&mut self) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        let mut observed = Observed {
            values: &mut self.values,
            reports: &mut self.reports,
        };
        let meters = &mut self.meters;
        run.controller
            .poll_with(&mut Both(&mut run.live, &mut observed), |m: &Meter| {
                meters.push(*m);
            });
        let blocks = &mut self.blocks;
        run.controller.poll_scopes(|b| blocks.push(*b));
        let snapshots = &mut self.snapshots;
        run.controller.poll_vectors(|s| snapshots.push(*s));
        if let Some(input) = run.input.as_mut() {
            let inputs = &mut self.inputs;
            input.meters().drain(|m| inputs.push(*m));
        }
        let status = run.controller.status();
        let swapped = status.version != run.version;
        run.version = status.version;
        if let Some(failure) = status.failure
            && !run.failure_reported
        {
            run.failure_reported = true;
            self.messages.push(failure.to_string());
            if let Some(mode) = self.app.as_mut() {
                mode.message = Some(failure.to_string());
            }
        }
        let mut bound = Vec::new();
        for (&sub, s) in &mut self.subs {
            if let Stream::Scope {
                slot, bound: was, ..
            } = &mut s.stream
            {
                let now = run.controller.scope_bound(*slot);
                if now != *was {
                    *was = now;
                    bound.push((s.client, sub, now, false));
                }
            }
            if let Stream::Vector {
                slot, bound: was, ..
            } = &mut s.stream
            {
                let now = run.controller.vector_bound(*slot);
                if now != *was {
                    *was = now;
                    bound.push((s.client, sub, now, true));
                }
            }
        }
        for (client, sub, now, vector) in bound {
            let kind = match (vector, now) {
                (true, true) => "vector.bound",
                (true, false) => "vector.unbound",
                (false, true) => "scope.bound",
                (false, false) => "scope.unbound",
            };
            let mut message = json!({"type": kind, "sub": sub});
            // Why a vector stream is unbound (M2.8-9): its port's width, or
            // its decimation off the port's ticks in the running plan.
            if vector && !now {
                let slot = self.subs.get(&sub).and_then(|s| match &s.stream {
                    Stream::Vector { slot, .. } => Some(*slot),
                    _ => None,
                });
                let reason = slot
                    .zip(self.run.as_ref())
                    .and_then(|(slot, run)| run.controller.vector_unbound(slot));
                if let Some(reason) = reason {
                    message = with(
                        message,
                        "reason",
                        json!(match reason {
                            VectorUnbound::Absent => "absent",
                            VectorUnbound::Decimation => "decimation",
                            VectorUnbound::Unverified => "unverified",
                        }),
                    );
                }
            }
            self.send_text(client, &message);
        }
        self.take_reports();
        self.take_meters();
        self.take_blocks();
        self.take_snapshots();
        if swapped {
            let describe = self.describe(false);
            self.broadcast(&describe);
            self.push_graph(true);
        }
    }

    /// Every 100 ms: a changed project folder is applied live.
    fn watch(&mut self) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        if run.watched.elapsed() < Duration::from_millis(WATCH_INTERVAL_MS) {
            return;
        }
        run.watched = Instant::now();
        let changed = match run.watcher.as_mut().map(ProjectWatcher::poll) {
            Some(Ok(changed)) => changed,
            Some(Err(error)) => {
                self.messages.push(format!("watch: {error}"));
                false
            }
            None => false,
        };
        if !(changed || run.retry) {
            return;
        }
        run.retry = false;
        // Read before the reload: a later edit makes it differ from the file.
        let file = fs::read_to_string(self.dir.join(signal_project::PROJECT_FILE)).ok();
        run.file = None;
        let mut observed = Observed {
            values: &mut self.values,
            reports: &mut self.reports,
        };
        let result = reload_observed(
            &mut run.controller,
            &mut run.live,
            &mut observed,
            &mut run.current,
            &self.dir,
            &self.registry,
        );
        let mut reloaded = None;
        // A swap's `graph` is pushed once it commits (`poll`).
        let swapping = matches!(&result, Ok(applied) if applied.swap.is_some());
        let notice = match result {
            Ok(applied) => {
                run.file = file;
                reloaded = Some((
                    Panel::new(&run.current, &self.registry),
                    run.current.plan(&self.registry).ok().map(|p| p.plan),
                ));
                self.messages.push(format!("watch: {applied}"));
                let edited = applied.sets > 0 || applied.swap.is_some();
                (
                    json!({
                        "type": "reload",
                        "sets": applied.sets,
                        "swap": applied.swap,
                        "removed": applied.removed,
                        "message": applied.to_string(),
                    }),
                    edited && applied.swap.is_none(),
                )
            }
            Err(ReloadError::Invalid(diagnostics)) => {
                let message = format!("{}; keeping the last good version", joined(diagnostics));
                self.messages.push(format!("watch: {message}"));
                (json!({"type": "reload.error", "message": message}), false)
            }
            Err(ReloadError::Failed(message)) => {
                run.retry = true;
                let message = format!("{message}; keeping the last good version, retrying");
                self.messages.push(format!("watch: {message}"));
                (json!({"type": "reload.error", "message": message}), false)
            }
        };
        if let Some((panel, plan)) = reloaded {
            // The reload sent every port what differs from the file: a
            // control written while its live send was refused now holds it.
            for (instance, parameter) in std::mem::take(&mut self.resend) {
                self.panel.release(&instance, &parameter);
            }
            self.reconcile(panel);
            self.replace_plan(plan);
            self.reshape_vectors(true);
        }
        if let Some(mode) = self.app.as_mut() {
            let error = notice.0.get("type") == Some(&json!("reload.error"));
            let message = notice.0.get("message").and_then(Value::as_str);
            mode.reload = message.map(|m| (m.to_owned(), error));
        }
        self.take_reports();
        self.broadcast(&notice.0);
        // A swap is described once it commits (`poll`); new bases now.
        if notice.1 {
            let describe = self.describe(false);
            self.broadcast(&describe);
        }
        // `ui/schematic.toml` changes come this way too.
        if !swapping {
            self.push_graph(false);
        }
    }

    /// The plan now running: a live port edit whose plan base changed (or
    /// whose port went) is dropped, the base being what the file now says.
    fn replace_plan(&mut self, plan: Option<ExecutionPlan>) {
        let old = std::mem::replace(&mut self.plan, plan);
        let new = self.plan.as_ref();
        self.port_edits.retain(|port, _| {
            let after = plan_base(new, port);
            after.is_some()
                && after.map(f64::to_bits) == plan_base(old.as_ref(), port).map(f64::to_bits)
        });
    }

    fn take_reports(&mut self) {
        for report in std::mem::take(&mut self.reports) {
            let Some(key) = self.commands.remove(&report.id) else {
                continue;
            };
            let Some(pending) = self.acks.get_mut(&key) else {
                continue;
            };
            pending.remaining = pending.remaining.saturating_sub(1);
            pending.frame = pending.frame.max(report.frame.0);
            pending.requested = pending.requested.max(report.requested.0);
            pending.refused |= report.outcome == Outcome::Refused;
            if pending.remaining == 0
                && let Some(done) = self.acks.remove(&key)
            {
                // A runner refused it: the value did not reach the ports.
                match (&done.edited, done.refused) {
                    (Some(Edited::Control((instance, parameter), value)), true) => {
                        self.panel.unsend(instance, parameter, *value);
                    }
                    (Some(Edited::Port(port, value)), true) => {
                        if let Some(edit) = self.port_edits.get_mut(port)
                            && edit.0.to_bits() == value.to_bits()
                        {
                            edit.1 = false;
                        }
                    }
                    _ => {}
                }
                let ack = json!({
                    "type": "ack",
                    "id": done.id,
                    "frame": done.frame,
                    "requested": done.requested,
                    "late": done.frame > done.requested,
                    "outcome": if done.refused { "refused" } else { "scheduled" },
                });
                self.send_text(done.client, &ack);
            }
        }
    }

    fn take_meters(&mut self) {
        for (meters, input) in [
            (std::mem::take(&mut self.meters), false),
            (std::mem::take(&mut self.inputs), true),
        ] {
            for m in &meters {
                for sub in self.subs.values_mut() {
                    if let Stream::Meters {
                        input: wants,
                        levels,
                        first,
                    } = &mut sub.stream
                        && *wants == input
                    {
                        if levels.len() <= m.channel {
                            levels.resize(m.channel + 1, Level::default());
                        }
                        if let Some(level) = levels.get_mut(m.channel) {
                            level.peak = level.peak.max(m.peak);
                            level.sum_squares += m.sum_squares;
                            level.frames += m.frames;
                        }
                        first.get_or_insert(m.first_frame.0);
                    }
                }
            }
        }
    }

    fn take_blocks(&mut self) {
        for block in std::mem::take(&mut self.blocks) {
            let Some(sub) = self.scope_slots.get(block.slot).copied().flatten() else {
                continue;
            };
            let Some(client) = self.subs.get(&sub).map(|s| s.client) else {
                continue;
            };
            let values = block.values();
            let mut bytes = frame(
                frames::SCOPE,
                sub,
                block.first_frame.0,
                values.len() as u32,
                block.decimation as u32,
                values.len() * 4,
            );
            for v in values {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            self.send_binary(client, bytes);
        }
    }

    /// Vector frames (kind 4, M2.8-7): one per snapshot, its rows as `f32`.
    fn take_snapshots(&mut self) {
        for snapshot in std::mem::take(&mut self.snapshots) {
            let Some(sub) = self.vector_slots.get(snapshot.slot).copied().flatten() else {
                continue;
            };
            let Some(client) = self.subs.get(&sub).map(|s| s.client) else {
                continue;
            };
            let values = snapshot.values();
            let mut bytes = frame(
                frames::VECTOR,
                sub,
                snapshot.frame.0,
                values.len() as u32,
                u32::try_from(snapshot.decimation).unwrap_or(u32::MAX),
                values.len() * 4,
            );
            for v in values {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            self.send_binary(client, bytes);
        }
    }

    /// The parameter and meter frames due.
    fn emit(&mut self) {
        let now = Instant::now();
        let next_frame = self
            .run
            .as_ref()
            .map_or(0, |r| r.controller.status().next_frame.0);
        let mut out = Vec::new();
        for (&id, sub) in &mut self.subs {
            if now.duration_since(sub.last) < sub.every {
                continue;
            }
            match &mut sub.stream {
                Stream::Parameters { ports } => {
                    sub.last = now;
                    let mut bytes = frame(
                        frames::PARAMETERS,
                        id,
                        next_frame,
                        ports.len() as u32,
                        0,
                        ports.len() * 16,
                    );
                    for port in ports.iter() {
                        let value = port.as_ref().and_then(|p| self.values.get(p));
                        let (base, effective) =
                            value.map_or((f64::NAN, f64::NAN), |v| (v.base, v.effective));
                        bytes.extend_from_slice(&base.to_le_bytes());
                        bytes.extend_from_slice(&effective.to_le_bytes());
                    }
                    out.push((sub.client, bytes));
                }
                Stream::Meters { levels, first, .. } => {
                    let Some(start) = first.take() else {
                        continue;
                    };
                    sub.last = now;
                    let covered = levels.iter().map(|l| l.frames).max().unwrap_or(0);
                    let mut bytes = frame(
                        frames::METERS,
                        id,
                        start,
                        levels.len() as u32,
                        covered.min(u64::from(u32::MAX)) as u32,
                        levels.len() * 8,
                    );
                    for level in levels.iter_mut() {
                        let rms = if level.frames > 0 {
                            (level.sum_squares / level.frames as f64).sqrt() as f32
                        } else {
                            0.0
                        };
                        bytes.extend_from_slice(&level.peak.to_le_bytes());
                        bytes.extend_from_slice(&rms.to_le_bytes());
                        *level = Level::default();
                    }
                    out.push((sub.client, bytes));
                }
                Stream::Scope { .. } | Stream::Vector { .. } => {}
            }
        }
        for (client, bytes) in out {
            self.send_binary(client, bytes);
        }
    }

    fn inbound(&mut self, message: Inbound) {
        match message {
            Inbound::Connected {
                client,
                queue,
                evicted,
                gate,
            } => {
                self.clients.insert(
                    client,
                    Client {
                        queue,
                        evicted,
                        gate,
                        dropped: 0,
                    },
                );
                let mut welcome = json!({
                    "type": "welcome",
                    "protocol": PROTOCOL,
                    "client": client,
                    "project": self.run.as_ref().map(|r| r.current.name.clone()),
                    "sampleRate": self.rate,
                    "frame": self.run.as_ref().map(|r| r.controller.status().next_frame.0),
                    "planVersion": self.run.as_ref().map(|r| r.version),
                });
                // Before the client can subscribe: one that connects while
                // the app is off streams nothing (M2.8-11).
                if let Some(mode) = &self.app {
                    welcome = with(welcome, "power", power_json(mode.power));
                }
                self.send_text(client, &welcome);
            }
            Inbound::Text { client, text } => self.handle(client, &text),
            Inbound::Gone { client } => self.remove(client),
        }
    }

    fn handle(&mut self, client: u64, text: &str) {
        let message: Value = match serde_json::from_str(text) {
            Ok(message) => message,
            Err(_) => {
                let error = json!({"type": "error", "code": "bad-message", "message": "not JSON"});
                return self.send_text(client, &error);
            }
        };
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let kind = message.get("type").and_then(Value::as_str).unwrap_or("");
        let result = match kind {
            "describe" => Ok(Some(self.describe(false))),
            "graph" => self.graph().map(Some),
            "schematic.move" => self.schematic_move(&message).map(Some),
            "schematic.reset" => self.schematic_reset(&message).map(Some),
            "subscribe" => self.subscribe(client, &message).map(Some),
            "unsubscribe" => self.unsubscribe(client, &message).map(Some),
            "set" | "ramp" | "trigger" => self.edit(client, &id, kind, &message).map(|()| None),
            "commit" => self.commit(&message).map(Some),
            "retry" => self.retry(&message).map(Some),
            "revert" => self.revert(&message).map(Some),
            "panel.check" => self.panel_check(&message).map(Some),
            "panel.read" => self.panel_read(&message).map(Some),
            "panel.write" => self.panel_write(&message).map(Some),
            "setup.get" => Ok(Some(self.setup_get())),
            "setup.devices" => self.setup_devices(&message).map(Some),
            "setup.check" => self.setup_check(&message).map(Some),
            "setup.apply" => self.setup_apply(&message).map(Some),
            "app.get" => match self.app {
                Some(_) => Ok(Some(self.app_json())),
                None => Err(app_only()),
            },
            "app.power" => self.power(&message).map(Some),
            "transport.play" => self.transport(true).map(Some),
            "transport.stop" => self.transport(false).map(Some),
            "project.open" => self.project_open(&message).map(Some),
            "project.save" => self.project_save().map(Some),
            "stats" => Ok(Some(self.stats())),
            "ping" => Ok(Some(json!({
                "type": "pong",
                "frame": self.run.as_ref().map(|r| r.controller.status().next_frame.0),
            }))),
            "hello" => Err(fail(
                "already-authenticated",
                "hello is the first message only",
            )),
            _ => Err(fail(
                "unknown-type",
                format!("unknown message type '{kind}'"),
            )),
        };
        match result {
            Ok(Some(reply)) => {
                let restarted =
                    kind == "setup.apply" && reply.get("restarted") == Some(&json!(true));
                // The plan or what plays changed: every client is told.
                let changed = matches!(
                    kind,
                    "transport.play" | "transport.stop" | "project.open" | "app.power"
                );
                let reply = with(reply, "id", id);
                self.send_text(client, &reply);
                if restarted || changed {
                    let describe = self.describe(restarted);
                    self.broadcast(&describe);
                    self.push_graph(false);
                }
                // Every client sees the positions written.
                if kind.starts_with("schematic.") {
                    self.push_graph(true);
                }
                // Every client gets the written panel (ADR 0026 §5).
                if kind == "panel.write" {
                    let pushed = json!({
                        "type": "panel.written",
                        "file": reply.get("file").cloned().unwrap_or(Value::Null),
                        "base": reply.get("base").cloned().unwrap_or(Value::Null),
                        "html": message.get("html").cloned().unwrap_or(Value::Null),
                    });
                    self.broadcast(&pushed);
                }
            }
            Ok(None) => {}
            Err(failure) => {
                let mut error = json!({
                    "type": "error",
                    "id": id,
                    "code": failure.code,
                    "message": failure.message,
                });
                if let (Some(Value::Object(detail)), Some(error)) =
                    (failure.detail, error.as_object_mut())
                {
                    for (key, value) in detail {
                        error.entry(key).or_insert(value);
                    }
                }
                self.send_text(client, &error);
            }
        }
    }

    /// `describe`: the running plan, the controls, the sinks, meters and
    /// scopes.
    fn describe(&self, restarted: bool) -> Value {
        let empty = Vec::new();
        let instances = self.plan.as_ref().map_or(&empty, |p| &p.instances);
        let described: Vec<Value> = instances
            .iter()
            .map(|i| {
                let ports: Vec<Value> = i
                    .descriptor
                    .ports
                    .iter()
                    .filter_map(|p| {
                        let address = PortAddress::new(&i.address, p.name).ok()?;
                        let value = serde_json::to_value(p).ok()?;
                        Some(with(value, "address", json!(address.as_str())))
                    })
                    .collect();
                let parameters: Vec<Value> = i
                    .descriptor
                    .parameters
                    .iter()
                    .filter_map(|p| {
                        let address = PortAddress::new(&i.address, p.name).ok()?;
                        let base = i.parameters.get(p.name).copied().unwrap_or(p.default);
                        let value = serde_json::to_value(p).ok()?;
                        let value = with(value, "address", json!(address.as_str()));
                        Some(with(value, "base", json!(base)))
                    })
                    .collect();
                json!({
                    "address": i.address.as_str(),
                    "module": i.type_name,
                    "sink": i.descriptor.sink,
                    "ports": ports,
                    "parameters": parameters,
                })
            })
            .collect();
        let sinks: Vec<Value> = instances
            .iter()
            .filter(|i| i.descriptor.sink.is_some())
            .enumerate()
            .map(|(index, i)| {
                let inputs = i
                    .descriptor
                    .ports
                    .iter()
                    .filter(|p| p.direction == PortDirection::In && p.shape == PortShape::Signal)
                    .count();
                json!({
                    "index": index,
                    "address": i.address.as_str(),
                    "kind": i.descriptor.sink,
                    "inputs": inputs,
                })
            })
            .collect();
        let combine_of = |port: &PortAddress| {
            instances
                .iter()
                .find(|i| i.address == port.instance())
                .and_then(|i| {
                    i.descriptor
                        .parameters
                        .iter()
                        .find(|p| p.name == port.port())
                })
                .map(|p| json!(p.combine))
        };
        let mut controls = Vec::new();
        for instance in self.panel.view(&self.values) {
            for p in &instance.parameters {
                let Some(target) = self.panel.target(&instance.name, &p.name) else {
                    continue;
                };
                let address = format!("/{}.{}", instance.name, p.name);
                // The live base: the last value the Controller took for every
                // port (not one it refused; one a runner refused afterwards
                // still counts), each port's default after a reset.
                let reset = self
                    .failed
                    .get(&(instance.name.clone(), p.name.clone()))
                    .is_some_and(|f| f.reset.is_some());
                let base = match self.panel.dragged(&instance.name, &p.name) {
                    Some(drag) => drag.applied,
                    None if reset => p.default,
                    None => p.base,
                };
                controls.push(json!({
                    "address": address,
                    "instance": instance.name,
                    "parameter": p.name,
                    "module": instance.module,
                    "ports": target.ports.iter().map(PortAddress::as_str).collect::<Vec<_>>(),
                    "unit": p.unit,
                    "min": p.min,
                    "max": p.max,
                    "default": p.default,
                    "base": base,
                    "inFile": p.in_file,
                    "modulated": p.modulated,
                    "combine": target.ports.first().and_then(combine_of),
                }));
            }
        }
        let (output_meters, input_meters): (Vec<Value>, Vec<Value>) = match &self.run {
            Some(run) => (
                (0..run.width)
                    .map(|c| {
                        let device = run.output.target(c);
                        json!({"channel": c, "device": device, "label": app::meter_label(c, device)})
                    })
                    .collect(),
                (0..run.input_channels)
                    .map(|c| json!({"channel": c}))
                    .collect(),
            ),
            None => (Vec::new(), Vec::new()),
        };
        let mut describe = json!({
            "type": "describe",
            "planVersion": self.run.as_ref().map(|r| r.version),
            "sampleRate": self.rate,
            "project": self.run.as_ref().map(|r| r.current.name.clone()),
            "playing": self.run.is_some(),
            "instances": described,
            "controls": controls,
            "sinks": sinks,
            "meters": {"output": output_meters, "input": input_meters},
            "scopes": {"slots": SCOPE_SLOTS, "points": SCOPE_POINTS, "maxRate": self.rate},
            "vectors": {"slots": VECTOR_SLOTS, "maxRows": VECTOR_MAX_ROWS},
            "unsaved": self.unsaved_json(),
        });
        if restarted {
            describe = with(describe, "restarted", json!(true));
        }
        describe
    }

    /// `graph` (ADR 0026 §1): the project's structure as written
    /// ([`Project::structure`]), and its schematic positions from
    /// `ui/schematic.toml`, read now. Playing, of the running project (the
    /// last good version, as `describe`); stopped (app mode), of the folder.
    fn graph(&self) -> Result<Value, Failure> {
        let (name, structure) = match &self.run {
            Some(run) => {
                let plan = self
                    .plan
                    .as_ref()
                    .ok_or_else(|| fail("does-not-load", "the running project has no plan"))?;
                (
                    run.current.name.clone(),
                    run.current.structure(plan, &self.registry),
                )
            }
            None => {
                if self.app.as_ref().is_some_and(|m| !m.opened) {
                    return Err(fail("no-project", "open a project folder first"));
                }
                let (project, plan, _) = app::load(&self.dir, &self.registry)
                    .map_err(|diagnostics| fail("does-not-load", diagnostics.join("\n")))?;
                (
                    project.name.clone(),
                    project.structure(&plan, &self.registry),
                )
            }
        };
        let (positions, error) = match fs::read_to_string(self.dir.join(SCHEMATIC_FILE)) {
            Ok(text) => match schematic::read_positions(&text) {
                Ok(positions) => (positions, None),
                Err(error) => (Default::default(), Some(error)),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (Default::default(), None)
            }
            Err(error) => (
                Default::default(),
                Some(format!("{SCHEMATIC_FILE}: {error}")),
            ),
        };
        let positions: serde_json::Map<String, Value> = positions
            .into_iter()
            .map(|(level, entries)| {
                let entries: serde_json::Map<String, Value> = entries
                    .into_iter()
                    .map(|(instance, (x, y))| (instance, json!({"x": x, "y": y})))
                    .collect();
                (level, Value::Object(entries))
            })
            .collect();
        Ok(json!({
            "type": "graph",
            "project": name,
            "planVersion": self.run.as_ref().map(|r| r.version),
            "top": structure.get("top"),
            "composites": structure.get("composites"),
            "layout": {"file": SCHEMATIC_FILE, "positions": positions, "error": error},
        }))
    }

    /// Pushes `graph` to every client when it differs from the last one
    /// pushed, or (`force`) whatever it is: after a swap and a schematic
    /// write. A project that does not load pushes nothing.
    fn push_graph(&mut self, force: bool) {
        let Ok(graph) = self.graph() else {
            return;
        };
        if force || self.graph_pushed.as_ref() != Some(&graph) {
            self.broadcast(&graph);
            self.graph_pushed = Some(graph);
        }
    }

    /// App mode, stopped: every 100 ms a changed folder pushes `graph` (its
    /// structure or its positions). Not while switched off: switching on
    /// pushes what changed meanwhile (`handle`).
    fn watch_stopped(&mut self) {
        let opened = self.app.as_ref().is_some_and(|m| m.opened);
        if self.run.is_some() || !opened {
            self.idle_watch = None;
            return;
        }
        if !self.powered() {
            return;
        }
        let Some((watcher, polled)) = self.idle_watch.as_mut() else {
            self.idle_watch = ProjectWatcher::new(&self.dir)
                .ok()
                .map(|w| (w, Instant::now()));
            return;
        };
        if polled.elapsed() < Duration::from_millis(WATCH_INTERVAL_MS) {
            return;
        }
        *polled = Instant::now();
        if watcher.poll().unwrap_or(false) {
            self.push_graph(false);
        }
    }

    /// `schematic.move` (ADR 0026 §3): instance `instance` of level `level`
    /// (`""` the top level, else a composite the project uses) at `x`, `y`
    /// (finite, rounded to integers), written into `ui/schematic.toml`.
    fn schematic_move(&mut self, message: &Value) -> Result<Value, Failure> {
        let text = |key: &str| {
            message
                .get(key)
                .and_then(Value::as_str)
                .ok_or_else(|| fail("bad-message", format!("schematic.move needs `{key}`")))
        };
        let (level, instance) = (text("level")?, text("instance")?);
        let coordinate = |key: &str| {
            message
                .get(key)
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite() && v.abs() <= 1e9)
                .map(|v| v.round() as i64)
                .ok_or_else(|| {
                    fail(
                        "bad-value",
                        format!("`{key}` must be a finite number, at most 1e9"),
                    )
                })
        };
        let (x, y) = (coordinate("x")?, coordinate("y")?);
        let graph = self.graph()?;
        let instances = level_of(&graph, level)?["instances"].as_array();
        if !instances.is_some_and(|all| all.iter().any(|i| i["name"] == instance)) {
            let at = if level.is_empty() {
                "the top level".to_owned()
            } else {
                format!("composite {level}")
            };
            return Err(fail(
                "unknown-instance",
                format!("{at} has no instance `{instance}`"),
            ));
        }
        self.write_schematic(|text| schematic::set_position(text, level, instance, x, y))?;
        Ok(json!({
            "type": "schematic.moved",
            "level": level,
            "instance": instance,
            "x": x,
            "y": y,
            "file": SCHEMATIC_FILE,
        }))
    }

    /// `schematic.reset`: removes the table of `level`, or without one the
    /// whole `ui/schematic.toml`.
    fn schematic_reset(&mut self, message: &Value) -> Result<Value, Failure> {
        let path = self.dir.join(SCHEMATIC_FILE);
        match message.get("level") {
            None | Some(Value::Null) => {
                match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(fail("write-failed", format!("{}: {error}", path.display())));
                    }
                }
                self.seen(Path::new(SCHEMATIC_FILE));
                Ok(json!({"type": "schematic.reset", "level": null, "file": SCHEMATIC_FILE}))
            }
            Some(Value::String(level)) => {
                let graph = self.graph()?;
                level_of(&graph, level)?;
                if path.exists() {
                    self.write_schematic(|text| schematic::remove_level(text, level))?;
                }
                Ok(json!({"type": "schematic.reset", "level": level, "file": SCHEMATIC_FILE}))
            }
            Some(_) => Err(fail("bad-message", "`level` is a string or null")),
        }
    }

    /// `ui/schematic.toml` edited by `edit`, written atomically as `commit`
    /// writes `project.toml`, and taken as seen by the watch (the write's
    /// `graph` is pushed at once instead).
    fn write_schematic(
        &mut self,
        edit: impl FnOnce(&str) -> Result<String, String>,
    ) -> Result<(), Failure> {
        let path = self.dir.join(SCHEMATIC_FILE);
        let failed = |error: String| {
            fail(
                "write-failed",
                format!("{} was not written: {error}", path.display()),
            )
        };
        let before = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(failed(error.to_string())),
        };
        let after = edit(&before).map_err(failed)?;
        if let Some(folder) = path.parent() {
            fs::create_dir_all(folder).map_err(|e| failed(e.to_string()))?;
        }
        write_file(&path, &after).map_err(|e| failed(e.to_string()))?;
        self.seen(Path::new(SCHEMATIC_FILE));
        Ok(())
    }

    /// Takes `file` as it is now as seen by the watches.
    fn seen(&mut self, file: &Path) {
        let run = self.run.as_mut().and_then(|r| r.watcher.as_mut());
        let idle = self.idle_watch.as_mut().map(|(w, _)| w);
        for watcher in run.into_iter().chain(idle) {
            let stamp = watcher.stamp(file);
            watcher.accept(file, stamp);
        }
    }

    fn subscribe(&mut self, client: u64, message: &Value) -> Result<Value, Failure> {
        if !self.powered() {
            return Err(powered_off());
        }
        let stream = message.get("stream").and_then(Value::as_str).unwrap_or("");
        let id = self.next_sub;
        let (stream, every, extra) = match stream {
            "parameters" => {
                let addresses: Vec<String> = message
                    .get("addresses")
                    .and_then(Value::as_array)
                    .ok_or_else(|| fail("bad-message", "parameters need `addresses`"))?
                    .iter()
                    .map(|a| a.as_str().unwrap_or("").to_owned())
                    .collect();
                let mut ports = Vec::new();
                for address in &addresses {
                    let aim = self.aim(address)?;
                    ports.push(aim.ports.into_iter().next());
                }
                (
                    Stream::Parameters { ports },
                    rate_interval(message)?,
                    json!({"addresses": addresses}),
                )
            }
            "meters" => {
                let input = match message.get("source").and_then(Value::as_str) {
                    None | Some("output") => false,
                    Some("input") => true,
                    Some(other) => {
                        return Err(fail("bad-message", format!("no meter source '{other}'")));
                    }
                };
                (
                    Stream::Meters {
                        input,
                        levels: Vec::new(),
                        first: None,
                    },
                    rate_interval(message)?,
                    json!({"source": if input { "input" } else { "output" }}),
                )
            }
            "scope" => {
                let address = message.get("port").and_then(Value::as_str).unwrap_or("");
                let port: PortAddress = address
                    .parse()
                    .map_err(|e| fail("bad-address", format!("{address}: {e}")))?;
                // A signal output, or a signal input (the sum of its sources).
                let is_signal = self.plan.as_ref().is_some_and(|plan| {
                    plan.instances
                        .iter()
                        .find(|i| i.address == port.instance())
                        .is_some_and(|i| {
                            i.descriptor
                                .ports
                                .iter()
                                .any(|p| p.name == port.port() && p.shape == PortShape::Signal)
                        })
                });
                if !is_signal {
                    return Err(fail(
                        "not-a-signal-output",
                        format!("{address} is not a signal output or input of the running plan"),
                    ));
                }
                let decimation = scope_decimation(message, self.rate)?;
                let slot = self
                    .scope_slots
                    .iter()
                    .position(Option::is_none)
                    .ok_or_else(|| fail("no-scope-slot", "every scope slot is in use"))?;
                let run = self
                    .run
                    .as_mut()
                    .ok_or_else(|| fail("not-playing", "nothing plays"))?;
                run.controller
                    .scope_start(slot, port.clone(), decimation)
                    .map_err(|e| fail("busy", e.to_string()))?;
                if let Some(s) = self.scope_slots.get_mut(slot) {
                    *s = Some(id);
                }
                (
                    Stream::Scope {
                        slot,
                        port,
                        decimation,
                        bound: false,
                    },
                    Duration::ZERO,
                    json!({"port": address, "decimation": decimation, "slot": slot}),
                )
            }
            "vector" => {
                let address = message.get("port").and_then(Value::as_str).unwrap_or("");
                let port: PortAddress = address
                    .parse()
                    .map_err(|e| fail("bad-address", format!("{address}: {e}")))?;
                // A vector output: its rows and rate divisor.
                let Some((rows, divisor)) = vector_shape(self.plan.as_ref(), &port) else {
                    return Err(fail(
                        "not-a-vector-output",
                        format!(
                            "{address} is not a vector output (of at most {VECTOR_MAX_ROWS} elements) of the running plan"
                        ),
                    ));
                };
                let rate = rate_of(message, 60.0)?;
                let decimation = vector_decimation(rate, self.rate, divisor);
                let slot = self
                    .vector_slots
                    .iter()
                    .position(Option::is_none)
                    .ok_or_else(|| fail("no-vector-slot", "every vector slot is in use"))?;
                let (Some(plan), Some(run)) = (self.plan.as_ref(), self.run.as_mut()) else {
                    return Err(fail("not-playing", "nothing plays"));
                };
                run.controller
                    .vector_start(plan, slot, port.clone(), rows, decimation)
                    .map_err(|e| fail("busy", e.to_string()))?;
                if let Some(s) = self.vector_slots.get_mut(slot) {
                    *s = Some(id);
                }
                (
                    Stream::Vector {
                        slot,
                        port,
                        rows,
                        rate,
                        decimation,
                        bound: false,
                    },
                    Duration::ZERO,
                    json!({"port": address, "rows": rows, "decimation": decimation, "slot": slot}),
                )
            }
            other => return Err(fail("bad-message", format!("no stream '{other}'"))),
        };
        self.next_sub = self.next_sub.wrapping_add(1).max(1);
        let kind = match &stream {
            Stream::Parameters { .. } => "parameters",
            Stream::Meters { .. } => "meters",
            Stream::Scope { .. } => "scope",
            Stream::Vector { .. } => "vector",
        };
        self.subs.insert(
            id,
            Sub {
                client,
                every,
                last: Instant::now(),
                stream,
            },
        );
        let reply = json!({"type": "subscribed", "sub": id, "stream": kind});
        let reply = match (reply, extra) {
            (Value::Object(mut a), Value::Object(b)) => {
                a.extend(b);
                Value::Object(a)
            }
            (reply, _) => reply,
        };
        Ok(reply)
    }

    fn unsubscribe(&mut self, client: u64, message: &Value) -> Result<Value, Failure> {
        let sub = message
            .get("sub")
            .and_then(Value::as_u64)
            .and_then(|s| u32::try_from(s).ok())
            .ok_or_else(|| fail("bad-message", "unsubscribe needs `sub`"))?;
        match self.subs.get(&sub) {
            Some(s) if s.client == client => {}
            _ => return Err(fail("unknown-sub", format!("no subscription {sub}"))),
        }
        self.drop_sub(sub);
        Ok(json!({"type": "unsubscribed", "sub": sub}))
    }

    fn drop_sub(&mut self, sub: u32) {
        match self.subs.remove(&sub) {
            Some(Sub {
                stream: Stream::Scope { slot, .. },
                ..
            }) => {
                if let Some(s) = self.scope_slots.get_mut(slot) {
                    *s = None;
                }
                if let Some(run) = self.run.as_mut() {
                    let _ = run.controller.scope_stop(slot);
                }
            }
            Some(Sub {
                stream: Stream::Vector { slot, .. },
                ..
            }) => {
                if let Some(s) = self.vector_slots.get_mut(slot) {
                    *s = None;
                }
                if let Some(run) = self.run.as_mut() {
                    let _ = run.controller.vector_stop(slot);
                }
            }
            _ => {}
        }
    }

    /// A control (`/<instance>.<parameter>`: a top-level instance's
    /// parameter, as the panel edits it) or a plan parameter port.
    fn aim(&self, address: &str) -> Result<Aim, Failure> {
        let port: PortAddress = address
            .parse()
            .map_err(|e| fail("bad-address", format!("{address}: {e}")))?;
        if let Some((instance, parameter)) = control_of(&port)
            && let Some(target) = self.panel.target(&instance, &parameter)
        {
            return Ok(Aim {
                ports: target.ports,
                min: target.min,
                max: target.max,
                control: Some((instance, parameter)),
            });
        }
        let descriptor = self.plan.as_ref().and_then(|plan| {
            plan.instances
                .iter()
                .find(|i| i.address == port.instance())
                .and_then(|i| {
                    i.descriptor
                        .parameters
                        .iter()
                        .find(|p| p.name == port.port())
                })
        });
        match descriptor {
            Some(d) => Ok(Aim {
                ports: vec![port],
                min: d.range.min,
                max: d.range.max,
                control: None,
            }),
            None => Err(fail(
                "unknown-address",
                format!("{address} is neither a control nor a parameter of the running plan"),
            )),
        }
    }

    /// `set`, `ramp`, `trigger`: through the Controller, acknowledged once
    /// every port's report is in.
    fn edit(
        &mut self,
        client: u64,
        id: &Value,
        kind: &str,
        message: &Value,
    ) -> Result<(), Failure> {
        let address = message.get("address").and_then(Value::as_str).unwrap_or("");
        let stamp = match message.get("at").and_then(Value::as_u64) {
            Some(frame) => Stamp::At(Frame(frame)),
            None => Stamp::Default,
        };
        let number = |key: &str| {
            message
                .get(key)
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite())
                .ok_or_else(|| fail("bad-value", format!("`{key}` must be a finite number")))
        };
        let (ports, payload, drag) = match kind {
            "trigger" => {
                let port: PortAddress = address
                    .parse()
                    .map_err(|e| fail("bad-address", format!("{address}: {e}")))?;
                let is_trigger = self.plan.as_ref().is_some_and(|plan| {
                    plan.instances
                        .iter()
                        .find(|i| i.address == port.instance())
                        .is_some_and(|i| {
                            i.descriptor.ports.iter().any(|p| {
                                p.name == port.port()
                                    && p.direction == PortDirection::In
                                    && p.shape == PortShape::Events
                            })
                        })
                });
                if !is_trigger {
                    return Err(fail(
                        "unknown-address",
                        format!("{address} is not an event input of the running plan"),
                    ));
                }
                (vec![port], EventPayload::Trigger, None)
            }
            "set" => {
                let aim = self.aim(address)?;
                let value = within(number("value")?, aim.min, aim.max);
                let edited = aim.edited(value);
                (aim.ports, EventPayload::Set(value), edited)
            }
            _ => {
                let aim = self.aim(address)?;
                let to = within(number("to")?, aim.min, aim.max);
                let frames = match (
                    message.get("frames").and_then(Value::as_u64),
                    message.get("ms").and_then(Value::as_f64),
                ) {
                    (Some(frames), _) => frames,
                    (None, Some(ms)) if ms.is_finite() && ms >= 0.0 => {
                        (f64::from(self.rate) * ms / 1000.0).round() as u64
                    }
                    _ => ramp_frames(self.rate),
                };
                let payload = EventPayload::Ramp {
                    to,
                    frames,
                    curve: Curve::Linear,
                };
                let edited = aim.edited(to);
                (aim.ports, payload, edited)
            }
        };
        let run = self
            .run
            .as_mut()
            .ok_or_else(|| fail("not-playing", "nothing plays"))?;
        let mut sent = Vec::new();
        let mut refused = None;
        for port in ports {
            match run.controller.send_event(port, payload, stamp) {
                Ok(command) => sent.push(command),
                Err(error) => {
                    refused = Some(error.to_string());
                    break;
                }
            }
        }
        // The live value the file may not hold (`docs/ui-protocol.md`
        // "Unsaved"): the latest one, and whether it reached every port.
        match &drag {
            Some(Edited::Control((instance, parameter), value)) => {
                self.panel
                    .drag(instance, parameter, *value, refused.is_none());
                if let Some(failed) = self.failed.get_mut(&(instance.clone(), parameter.clone())) {
                    failed.reset = None;
                }
            }
            Some(Edited::Port(port, value)) => {
                self.port_edits
                    .insert(port.clone(), (*value, refused.is_none()));
            }
            None => {}
        }
        if let Some(error) = refused {
            return Err(fail("refused", error));
        }
        let key = self.next_ack;
        self.next_ack += 1;
        for command in &sent {
            self.commands.insert(*command, key);
        }
        self.acks.insert(
            key,
            Pending {
                client,
                id: id.clone(),
                remaining: sent.len(),
                frame: 0,
                requested: 0,
                refused: false,
                edited: drag,
            },
        );
        Ok(())
    }

    /// Moves each port to its value over the panel's ramp; whether the
    /// Controller took every one (true when nothing plays).
    fn send_ramps(&mut self, sends: Vec<(PortAddress, f64)>) -> bool {
        let Some(run) = self.run.as_mut() else {
            return true;
        };
        let frames = ramp_frames(run.rate);
        for (port, to) in sends {
            let payload = EventPayload::Ramp {
                to,
                frames,
                curve: Curve::Linear,
            };
            if run
                .controller
                .send_event(port, payload, Stamp::Default)
                .is_err()
            {
                return false;
            }
        }
        true
    }

    /// `commit`: the value into `project.toml` (the panel's write: once,
    /// with toml_edit), live too if it was not the last value sent; the
    /// running project takes the same edit, so the file watch's reload of
    /// the written file applies nothing.
    fn commit(&mut self, message: &Value) -> Result<Value, Failure> {
        let address = message.get("address").and_then(Value::as_str).unwrap_or("");
        let port: PortAddress = address
            .parse()
            .map_err(|e| fail("bad-address", format!("{address}: {e}")))?;
        let not_editable = || {
            fail(
                "not-editable",
                format!(
                    "{address} is not editable: only the parameters of a top-level instance are \
                     (a parameter inside a composite is set in the composite's file)"
                ),
            )
        };
        let (instance, parameter) = control_of(&port).ok_or_else(not_editable)?;
        let target = self
            .panel
            .target(&instance, &parameter)
            .ok_or_else(not_editable)?;
        let value = match message.get("value") {
            None => {
                return Err(fail(
                    "bad-value",
                    "commit needs `value` (null: the default)",
                ));
            }
            Some(Value::Null) => None,
            Some(v) => Some(within(
                v.as_f64()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| fail("bad-value", "`value` must be a finite number or null"))?,
                target.min,
                target.max,
            )),
        };
        self.commit_control(address, &instance, &parameter, &target, value, false)
    }

    /// Writes `value` for a control (sent live first unless every port
    /// already took exactly that value): `commit`, and `retry`
    /// (`retrying`: a live send that is refused writes nothing). A failed
    /// write keeps the value as the control's live drag, with the error
    /// (`docs/ui-protocol.md` "Unsaved"); a write keeps the control unsaved
    /// until the reload of the written file sends it, when its live send was
    /// refused.
    fn commit_control(
        &mut self,
        address: &str,
        instance: &str,
        parameter: &str,
        target: &Target,
        value: Option<f64>,
        retrying: bool,
    ) -> Result<Value, Failure> {
        let key = (instance.to_owned(), parameter.to_owned());
        let last = self.panel.dragged(instance, parameter);
        let sent = match value {
            Some(v) => match last {
                Some(d) if d.sent && d.value.to_bits() == v.to_bits() => true,
                _ => self.send_ramps(target.ports.iter().cloned().map(|p| (p, v)).collect()),
            },
            // Each port back to its own default, as the panel's Reset.
            None => self.send_ramps(
                target
                    .ports
                    .iter()
                    .cloned()
                    .zip(target.defaults.iter().copied())
                    .collect(),
            ),
        };
        let unwritten = |host: &mut Host, error: String| {
            match value {
                Some(v) => host.panel.drag(instance, parameter, v, sent),
                None => {
                    host.panel.release(instance, parameter);
                }
            }
            let seq = host.next_failure;
            host.next_failure += 1;
            host.failed.insert(
                key.clone(),
                Failed {
                    error,
                    seq,
                    reset: value.is_none().then_some(sent),
                },
            );
        };
        if retrying && !sent {
            let error = format!("{address}: the running project did not take the value");
            unwritten(self, error.clone());
            return Err(fail("refused", format!("{error}; nothing was written")));
        }
        let order: Vec<&str> = target.order.iter().map(String::as_str).collect();
        let (before, after) = match write_project(&self.dir, instance, parameter, value, &order) {
            Ok(texts) => texts,
            Err(error) => {
                unwritten(self, error.clone());
                return Err(fail("write-failed", error));
            }
        };
        let file = Path::new(signal_project::PROJECT_FILE);
        // The written file, read at once: a later edit differs from it.
        let stamp = self
            .run
            .as_ref()
            .and_then(|r| r.watcher.as_ref())
            .map(|w| w.stamp(file));
        // The running project held exactly the file this write read: any
        // other writer's edit is in `before`, so it is not held.
        let known = self
            .run
            .as_mut()
            .and_then(|r| r.file.take())
            .is_some_and(|text| text == before);
        self.panel.release(instance, parameter);
        self.writes += 1;
        let mut note = None;
        // Whether the running project now holds exactly the file: then the
        // watch need not reload the write it would otherwise see.
        let mut held = false;
        let current = self.run.as_mut().map(|r| &mut r.current);
        match after_write(
            current,
            &self.dir,
            target,
            instance,
            parameter,
            value,
            sent,
            &self.registry,
        ) {
            // The plan too, which `describe` reads the instances' (and so a
            // composite's inner parameters') bases from.
            Written::Taken(panel) => {
                held = sent && known;
                self.reconcile(panel);
                let plan = self
                    .run
                    .as_ref()
                    .and_then(|r| r.current.plan(&self.registry).ok())
                    .map(|p| p.plan);
                self.replace_plan(plan);
            }
            Written::Stopped(Ok((panel, plan))) => {
                self.reconcile(panel);
                self.plan = Some(plan);
            }
            Written::Refused(panel, error) => {
                self.reconcile(panel);
                note = Some(format!(
                    "the running project could not take the refused value ({error}); the next \
                     reload applies what differs from the file"
                ));
            }
            Written::NotTaken(error) => {
                note = Some(format!(
                    "written, but the running project could not take it ({error}); the next \
                     reload applies the file"
                ));
            }
            // The folder's diagnostics, shown by the app (app mode).
            Written::Stopped(Err(diagnostics)) => {
                if let Some(mode) = self.app.as_mut() {
                    mode.diagnostics = diagnostics;
                }
            }
        }
        // The running project now holds `after`. The watch takes the file as
        // seen only when it still is `after` once stamped (no edit came
        // between the write and the stamp; a later one differs from it).
        if held
            && let Some(stamp) = stamp
            && fs::read_to_string(self.dir.join(file)).is_ok_and(|text| text == after)
            && let Some(run) = self.run.as_mut()
        {
            run.file = Some(after);
            if let Some(watcher) = run.watcher.as_mut() {
                watcher.accept(file, stamp);
            }
        }
        self.failed.remove(&key);
        if sent {
            self.resend.remove(&key);
        } else {
            self.resend.insert(key);
        }
        let describe = self.describe(false);
        self.broadcast(&describe);
        Ok(json!({
            "type": "committed",
            "address": address,
            "value": value,
            "file": signal_project::PROJECT_FILE,
            "writes": self.writes,
            "sent": sent,
            "note": note,
        }))
    }

    /// `retry`: commits an unsaved control's live value again (the latest
    /// one sent; the file's when only its live send is in doubt, the default
    /// after a failed reset).
    fn retry(&mut self, message: &Value) -> Result<Value, Failure> {
        let address = message.get("address").and_then(Value::as_str).unwrap_or("");
        let port: PortAddress = address
            .parse()
            .map_err(|e| fail("bad-address", format!("{address}: {e}")))?;
        let listed = self.unsaved().into_iter().find(|u| u.address == address);
        let Some(unsaved) = listed else {
            return Err(fail("not-unsaved", format!("{address} is not unsaved")));
        };
        let control = control_of(&port)
            .and_then(|(i, p)| self.panel.target(&i, &p).map(|t| (i, p, t)))
            .filter(|_| unsaved.control);
        let Some((instance, parameter, target)) = control else {
            return Err(fail(
                "not-editable",
                format!("{address} is no control: it can be reverted, not written"),
            ));
        };
        let reset = self
            .failed
            .get(&(instance.clone(), parameter.clone()))
            .is_some_and(|f| f.reset.is_some());
        let value = match self.panel.dragged(&instance, &parameter) {
            Some(drag) => Some(drag.value),
            None if reset => None,
            None => unsaved.in_file.then_some(unsaved.file),
        };
        self.commit_control(address, &instance, &parameter, &target, value, true)
    }

    /// `revert`: an unsaved parameter's live value back to the file's (each
    /// port to its own default without a key; a plan port to the plan's
    /// base). Refused, the parameter stays unsaved.
    fn revert(&mut self, message: &Value) -> Result<Value, Failure> {
        let address = message.get("address").and_then(Value::as_str).unwrap_or("");
        let port: PortAddress = address
            .parse()
            .map_err(|e| fail("bad-address", format!("{address}: {e}")))?;
        let listed = self.unsaved().into_iter().find(|u| u.address == address);
        let Some(unsaved) = listed else {
            return Err(fail("not-unsaved", format!("{address} is not unsaved")));
        };
        let playing = self.run.is_some();
        let control = control_of(&port)
            .and_then(|(i, p)| self.panel.target(&i, &p).map(|t| (i, p, t)))
            .filter(|_| unsaved.control);
        let sent = match &control {
            Some((_, _, target)) if unsaved.in_file => self.send_ramps(
                target
                    .ports
                    .iter()
                    .cloned()
                    .map(|p| (p, unsaved.file))
                    .collect(),
            ),
            Some((_, _, target)) => self.send_ramps(
                target
                    .ports
                    .iter()
                    .cloned()
                    .zip(target.defaults.iter().copied())
                    .collect(),
            ),
            None => self.send_ramps(vec![(port.clone(), unsaved.file)]),
        };
        if !sent {
            // Some ports may have taken it: what they hold is not known.
            match &control {
                Some((instance, parameter, _)) => {
                    if let Some(drag) = self.panel.dragged(instance, parameter) {
                        self.panel.drag(instance, parameter, drag.value, false);
                    }
                }
                None => {
                    if let Some(edit) = self.port_edits.get_mut(&port) {
                        edit.1 = false;
                    }
                }
            }
            return Err(fail(
                "refused",
                format!("{address}: the running project did not take the file's value"),
            ));
        }
        match control {
            Some((instance, parameter, _)) => {
                self.panel.release(&instance, &parameter);
                let key = (instance, parameter);
                self.failed.remove(&key);
                self.resend.remove(&key);
            }
            None => {
                self.port_edits.remove(&port);
            }
        }
        let describe = self.describe(false);
        self.broadcast(&describe);
        Ok(json!({
            "type": "reverted",
            "address": address,
            "value": unsaved.in_file.then_some(unsaved.file),
            "sent": playing,
        }))
    }

    /// Takes `new` (the panel of a reloaded or written project), keeping the
    /// drags, failures and pending resends of the controls whose file value
    /// did not change; a changed value was sent live by the reload (or
    /// written), a control that is gone has nothing left to save.
    fn reconcile(&mut self, new: Panel) {
        let old = std::mem::replace(&mut self.panel, new);
        let kept = |panel: &Panel, (instance, parameter): &(String, String)| {
            let after = panel.file_base(instance, parameter);
            after.is_some() && bits(after) == bits(old.file_base(instance, parameter))
        };
        for (key, drag) in old.drags() {
            if kept(&self.panel, key) {
                self.panel.restore(key.clone(), *drag);
            }
        }
        let panel = &self.panel;
        self.failed.retain(|key, _| kept(panel, key));
        self.resend.retain(|key| kept(panel, key));
    }

    /// The unsaved controls' live values that a new run of the same project
    /// does not have (it plays the file): their latest values, and failed
    /// resets (`None`).
    fn carried(&self) -> Vec<((String, String), Option<f64>)> {
        let mut carried = Vec::new();
        for unsaved in self.unsaved() {
            let key = unsaved
                .address
                .parse::<PortAddress>()
                .ok()
                .and_then(|port| control_of(&port));
            let Some(key) = key.filter(|_| unsaved.control) else {
                continue;
            };
            match self.panel.dragged(&key.0, &key.1) {
                Some(drag) => carried.push((key, Some(drag.value))),
                None if self.failed.get(&key).is_some_and(|f| f.reset.is_some()) => {
                    carried.push((key, None));
                }
                None => {}
            }
        }
        carried
    }

    /// Plays the `carried` values again in the new run (or, stopped, keeps
    /// them as the controls' values), so they stay live and unsaved.
    fn play_carried(&mut self, carried: Vec<((String, String), Option<f64>)>) {
        for ((instance, parameter), value) in carried {
            let Some(target) = self.panel.target(&instance, &parameter) else {
                self.failed.remove(&(instance, parameter));
                continue;
            };
            match value {
                Some(v) => {
                    let sent =
                        self.send_ramps(target.ports.iter().cloned().map(|p| (p, v)).collect());
                    self.panel.drag(&instance, &parameter, v, sent);
                }
                None => {
                    let sent = self.send_ramps(
                        target
                            .ports
                            .iter()
                            .cloned()
                            .zip(target.defaults.iter().copied())
                            .collect(),
                    );
                    if let Some(failed) = self.failed.get_mut(&(instance, parameter)) {
                        failed.reset = Some(sent);
                    }
                }
            }
        }
    }

    /// `panel.check`: the binding check of a panel file in the project's
    /// `ui/` against the running plan, as `signal ui check` makes it.
    fn panel_check(&self, message: &Value) -> Result<Value, Failure> {
        let name = message.get("file").and_then(Value::as_str).unwrap_or("");
        let unknown = || {
            fail(
                "unknown-panel",
                format!("no panel '{name}' in the project's ui/ folder"),
            )
        };
        if !name.ends_with(".html") {
            return Err(unknown());
        }
        let path = crate::http::file(&project_ui(&self.dir), name).ok_or_else(unknown)?;
        let text = std::fs::read_to_string(path).map_err(|_| unknown())?;
        let plan = self
            .plan
            .as_ref()
            .ok_or_else(|| fail("not-playing", "the project has no plan"))?;
        let targets = Targets {
            plan,
            panel: &self.panel,
        };
        let checked = check(&format!("ui/{name}"), &text, &targets);
        let diagnostics: Vec<Value> = checked
            .findings
            .iter()
            .map(|f| {
                let value = serde_json::to_value(&f.diagnostic).unwrap_or(Value::Null);
                with(value, "control", json!(f.control))
            })
            .collect();
        Ok(json!({
            "type": "panel.check",
            "file": checked.file,
            "controls": checked.controls,
            "bindings": checked.bindings,
            "ok": !checked.failed(),
            "diagnostics": diagnostics,
        }))
    }

    /// The panel file `name` of `panel.read` and `panel.write`: an `.html`
    /// file in the project's `ui/` by the static files' rules
    /// ([`crate::http::file`]: no `..`, no hidden segment, no escape through
    /// a link), or `ui/main.html` when it does not exist yet (the generated
    /// default panel's first write). Never outside the project served.
    fn panel_path(&self, name: &str) -> Result<(PathBuf, bool), Failure> {
        let unknown = || {
            fail(
                "unknown-panel",
                format!("no panel '{name}' in the project's ui/ folder"),
            )
        };
        if !name.ends_with(".html") {
            return Err(unknown());
        }
        let ui = project_ui(&self.dir);
        // Written only where it resolves inside the project served, never
        // through a link (a linked `ui/` or file is served, not edited).
        let link =
            |path: &Path| fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
        if link(&ui) || link(&ui.join(name)) {
            return Err(fail(
                "unknown-panel",
                format!(
                    "ui/{name} is reached through a link: the editor writes only the project's own files"
                ),
            ));
        }
        if let Some(path) = crate::http::file(&ui, name) {
            let project = fs::canonicalize(&self.dir).map_err(|_| unknown())?;
            return if path.starts_with(&project) {
                Ok((path, true))
            } else {
                Err(unknown())
            };
        }
        let main = ui.join(MAIN_PANEL);
        if name == MAIN_PANEL && fs::symlink_metadata(&main).is_err() {
            return Ok((main, false));
        }
        Err(unknown())
    }

    /// `panel.read` (ADR 0026 §5): a panel file's text and its SHA-256 (the
    /// `base` a `panel.write` names), the content of its `<signal-panel>`
    /// (or why it has no one to edit), and whether a write can be checked
    /// (`planned`: the project has a plan). A missing `ui/main.html` reads
    /// as `exists: false` with `base: null`: the generated panel.
    fn panel_read(&self, message: &Value) -> Result<Value, Failure> {
        let name = message.get("file").and_then(Value::as_str).unwrap_or("");
        let (path, exists) = self.panel_path(name)?;
        let planned = self.plan.is_some();
        if !exists {
            return Ok(
                json!({"type": "panel.read", "file": name, "exists": false, "base": null,
                "text": null, "html": null, "editable": true, "reason": null, "planned": planned}),
            );
        }
        let text = fs::read_to_string(&path)
            .map_err(|e| fail("unknown-panel", format!("ui/{name}: {e}")))?;
        let (html, reason) = match panel_content(&text) {
            Ok((start, end)) => (json!(text[start..end]), Value::Null),
            Err(reason) => (Value::Null, json!(reason)),
        };
        Ok(json!({
            "type": "panel.read",
            "file": name,
            "exists": true,
            "base": sha256_hex(text.as_bytes()),
            "text": text,
            "html": html,
            "editable": reason.is_null(),
            "reason": reason,
            "planned": planned,
        }))
    }

    /// `panel.write` (ADR 0026 §5): replaces the content of the file's one
    /// `<signal-panel>` with `html`, keeping every byte outside it, after
    /// checking `html` against the whitelist again, that the file is still
    /// the version `base` names (`conflict` otherwise, with its text), and
    /// the bindings as `panel.check` does; written atomically as `commit`
    /// writes, and taken as seen by the watch. A missing `ui/main.html`
    /// (`base: null`) is created with the standard head.
    fn panel_write(&mut self, message: &Value) -> Result<Value, Failure> {
        let name = message.get("file").and_then(Value::as_str).unwrap_or("");
        let html = message
            .get("html")
            .and_then(Value::as_str)
            .ok_or_else(|| fail("bad-message", "panel.write needs `html`"))?;
        let base = match message.get("base") {
            Some(Value::String(base)) => Some(base.as_str()),
            None | Some(Value::Null) => None,
            Some(_) => return Err(fail("bad-message", "`base` is a string or null")),
        };
        let (path, exists) = self.panel_path(name)?;
        check_content(html).map_err(|why| fail("bad-html", format!("not written: {why}")))?;
        let before = if exists {
            Some(
                fs::read_to_string(&path)
                    .map_err(|e| fail("write-failed", format!("ui/{name}: {e}")))?,
            )
        } else {
            None
        };
        let current = before.as_deref().map(|t| sha256_hex(t.as_bytes()));
        if current.as_deref() != base {
            let html = before
                .as_deref()
                .and_then(|t| panel_content(t).ok().map(|(s, e)| t[s..e].to_owned()));
            return Err(fail_with(
                "conflict",
                format!("ui/{name} changed since the version the editor started from"),
                json!({"file": name, "base": current, "text": before, "html": html}),
            ));
        }
        let after = match &before {
            Some(text) => {
                let range = panel_content(text).map_err(|why| fail("not-editable", why))?;
                replace(text, range, html)
            }
            None => first_file(&self.project_name(), html),
        };
        let plan = self.plan.as_ref().ok_or_else(|| {
            fail(
                "not-playing",
                "the project has no plan: the bindings cannot be checked",
            )
        })?;
        let targets = Targets {
            plan,
            panel: &self.panel,
        };
        let checked = check(&format!("ui/{name}"), &after, &targets);
        if checked.failed() {
            let diagnostics: Vec<Value> = checked
                .findings
                .iter()
                .map(|f| {
                    let value = serde_json::to_value(&f.diagnostic).unwrap_or(Value::Null);
                    with(value, "control", json!(f.control))
                })
                .collect();
            let first = checked
                .findings
                .iter()
                .find(|f| f.diagnostic.severity == signal_project::Severity::Error)
                .map(|f| f.diagnostic.message.clone())
                .unwrap_or_default();
            return Err(fail_with(
                "bad-binding",
                format!("not written: {first}"),
                json!({"file": name, "diagnostics": diagnostics}),
            ));
        }
        let failed =
            |e: std::io::Error| fail("write-failed", format!("ui/{name} was not written: {e}"));
        if let Some(folder) = path.parent() {
            fs::create_dir_all(folder).map_err(failed)?;
        }
        write_file(&path, &after).map_err(failed)?;
        self.seen(&Path::new("ui").join(name));
        Ok(json!({
            "type": "panel.written",
            "file": name,
            "base": sha256_hex(after.as_bytes()),
            "created": !exists,
        }))
    }

    fn setup_json(&self) -> Value {
        json!({
            "output": self.setup.output,
            "input": self.setup.input,
            "buffer": self.setup.buffer,
            "cpuWorkers": self.setup.cpu_workers,
            "outputChannels": self.setup.output_channels,
            "rtPolicy": self.setup.rt_policy.name(),
        })
    }

    fn run_json(&self) -> Value {
        match &self.run {
            Some(run) => json!({
                "device": run.device,
                "rate": run.rate,
                "buffer": run.buffer,
                "threads": run.threads,
                "map": (0..run.width).map(|c| run.output.target(c)).collect::<Vec<_>>(),
                "inputChannels": run.input_channels,
            }),
            None => Value::Null,
        }
    }

    /// `setup.get`: the Setup file and the run in force.
    fn setup_get(&self) -> Value {
        json!({
            "type": "setup",
            "path": self.setup_path.display().to_string(),
            "setup": self.setup_json(),
            "problems": self.setup_problems,
            "playing": self.run.is_some(),
            "run": self.run_json(),
            "rtPolicies": RtPolicy::ALL.iter().map(|p| p.name()).collect::<Vec<_>>(),
        })
    }

    /// `setup.devices`: the device list (refreshed when asked, refused while
    /// playing, as the dialog does), each output's capabilities, where the
    /// Setup's devices are in it, and the compute devices.
    fn setup_devices(&mut self, message: &Value) -> Result<Value, Failure> {
        if message.get("refresh").and_then(Value::as_bool) == Some(true) {
            if self.run.is_some() {
                return Err(fail("playing", app::REFRESH_WHILE_PLAYING));
            }
            self.list_devices();
        }
        let label = |direction, index| {
            labels(&self.devices, direction)
                .into_iter()
                .find(|(i, _)| *i == index)
                .map(|(_, label)| label)
        };
        let devices: Vec<Value> = self
            .devices
            .iter()
            .enumerate()
            .map(|(index, d)| {
                let value = serde_json::to_value(d).unwrap_or(Value::Null);
                let value = with(value, "index", json!(index));
                let value = with(value, "label", json!(label(d.direction, index)));
                // What tells same-named devices apart without an id: a page
                // keeps a chosen device by its name, id and signature.
                let value = with(value, "signature", json!(DeviceRef::of(d).signature));
                with(value, "capabilities", json!(capabilities(d)))
            })
            .collect();
        let resolution = |entry: &Option<crate::setup::DeviceEntry>, direction| match entry {
            None => Value::Null,
            Some(entry) => match resolve(&self.devices, direction, &entry.to_ref()) {
                Resolution::Found(index) => json!({"found": index}),
                Resolution::Missing { reason, .. } | Resolution::Ambiguous { reason, .. } => {
                    json!({"unresolved": reason})
                }
            },
        };
        // A page's chosen devices, found again by identity after a refresh
        // (`Choice` and `found`, the dialog's draft).
        let chosen = |key: &str, direction| {
            let wanted = message.get("choices")?.get(key)?;
            let name = wanted.get("name")?.as_str()?;
            let wanted = DeviceRef {
                name: name.to_owned(),
                id: wanted.get("id").and_then(Value::as_str).map(str::to_owned),
                signature: wanted
                    .get("signature")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            };
            Some(match found(&self.devices, direction, &wanted) {
                Choice::Device(device) => match resolve(&self.devices, direction, &device) {
                    Resolution::Found(index) => json!({"found": index}),
                    _ => json!({"unresolved": "the device list changed"}),
                },
                Choice::Unresolved(reason) => json!({"unresolved": reason}),
                Choice::Unset => Value::Null,
            })
        };
        let choices = json!({
            "output": chosen("output", Direction::Output),
            "input": chosen("input", Direction::Input),
        });
        Ok(json!({
            "type": "setup.devices",
            "devices": devices,
            "error": self.devices_error,
            "output": resolution(&self.setup.output, Direction::Output),
            "input": resolution(&self.setup.input, Direction::Input),
            "choices": choices,
            "compute": self.compute,
        }))
    }

    /// The listed device at `index` of `direction`.
    fn device(&self, index: &Value, direction: Direction) -> Result<&DeviceInfo, Failure> {
        index
            .as_u64()
            .and_then(|i| self.devices.get(usize::try_from(i).ok()?))
            .filter(|d| d.direction == direction)
            .ok_or_else(|| fail("unknown-device", format!("no such device {index}")))
    }

    /// `setup.check`: whether an output plays the project's rate with a
    /// buffer, as the dialog shows it.
    fn setup_check(&self, message: &Value) -> Result<Value, Failure> {
        let device = match message.get("output") {
            None | Some(Value::Null) => self
                .devices
                .iter()
                .find(|d| d.direction == Direction::Output && d.default)
                .ok_or_else(|| fail("unknown-device", "no default output is listed"))?,
            Some(index) => self.device(index, Direction::Output)?,
        };
        let buffer = buffer_of(message, self.setup.buffer)?;
        let (ok, text) = match rate_check(device, self.rate, buffer) {
            Ok(line) => (true, line),
            Err(refusal) => (false, refusal),
        };
        Ok(json!({"type": "setup.check", "ok": ok, "message": text}))
    }

    fn selection(&self, value: Option<&Value>, direction: Direction) -> Result<Selection, Failure> {
        match value {
            None => Ok(Selection::Keep),
            Some(Value::String(keep)) if keep == "keep" => Ok(Selection::Keep),
            Some(Value::Null) => Ok(Selection::Unset),
            Some(Value::Object(object)) => match (object.get("index"), object.get("name")) {
                (Some(index), _) => Ok(Selection::Device(DeviceRef::of(
                    self.device(index, direction)?,
                ))),
                (None, Some(Value::String(name))) if !name.is_empty() => {
                    Ok(Selection::Device(DeviceRef {
                        name: name.clone(),
                        id: object.get("id").and_then(Value::as_str).map(str::to_owned),
                        signature: object
                            .get("signature")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                    }))
                }
                _ => Err(fail(
                    "bad-device",
                    "a device is {\"index\": n} or {\"name\", \"id\", \"signature\"}",
                )),
            },
            Some(other) => Err(fail("bad-device", format!("not a device: {other}"))),
        }
    }

    /// `setup.apply`: the dialog's Apply. The settings go into the Setup,
    /// which is saved; while playing, the run stops and plays again from
    /// frame 0 with them.
    fn setup_apply(&mut self, message: &Value) -> Result<Value, Failure> {
        let output = self.selection(message.get("output"), Direction::Output)?;
        let input = self.selection(message.get("input"), Direction::Input)?;
        let buffer = buffer_of(message, self.setup.buffer)?;
        let rt_policy = match message.get("rtPolicy") {
            None => self.setup.rt_policy,
            Some(name) => name.as_str().and_then(RtPolicy::from_name).ok_or_else(|| {
                let names: Vec<&str> = RtPolicy::ALL.iter().map(|p| p.name()).collect();
                fail(
                    "bad-value",
                    format!("rtPolicy is one of {}", names.join(", ")),
                )
            })?,
        };
        let cpu_workers = match message.get("cpuWorkers") {
            None => self.setup.cpu_workers,
            Some(Value::Null) => None,
            Some(Value::String(auto)) if auto == "auto" => None,
            Some(v) => Some(
                v.as_u64()
                    .filter(|n| (1..=64).contains(n))
                    .map(|n| n as u32)
                    .ok_or_else(|| fail("bad-value", "cpuWorkers is \"auto\" or 1 to 64"))?,
            ),
        };
        let output_channels = match message.get("outputChannels") {
            None => self.setup.output_channels.clone(),
            Some(Value::Null) => None,
            Some(Value::String(text)) => channels(text).map_err(|e| fail("bad-value", e))?,
            Some(Value::Array(list)) => Some(
                list.iter()
                    .map(|c| {
                        c.as_u64()
                            .and_then(|c| u16::try_from(c).ok())
                            .filter(|&c| c > 0)
                            .ok_or_else(|| {
                                fail(
                                    "bad-value",
                                    format!("'{c}' is not a device channel (1, 2, …)"),
                                )
                            })
                    })
                    .collect::<Result<Vec<u16>, Failure>>()?,
            ),
            Some(other) => {
                return Err(fail("bad-value", format!("not a channel map: {other}")));
            }
        };
        Interface {
            output,
            input,
            buffer,
            cpu_workers,
            output_channels,
        }
        .apply(&mut self.setup);
        self.setup.rt_policy = rt_policy;
        if let Err(error) = app::save(&self.setup, &self.setup_path) {
            if let Some(mode) = self.app.as_mut() {
                mode.message = Some(error.clone());
            }
            return Ok(json!({
                "type": "applied",
                "saved": false,
                "restarted": false,
                "error": error,
            }));
        }
        if self.app.is_some() {
            return Ok(self.apply_app());
        }
        let Some(old) = self.run.take() else {
            return Ok(
                json!({"type": "applied", "saved": true, "restarted": false, "setup": self.setup_json()}),
            );
        };
        if self.recording {
            self.run = Some(old);
            return Err(fail(
                "recording",
                "the Setup was saved; a run that records a session or its audio is not \
                 restarted (a recording covers one run from frame 0)",
            ));
        }
        let carried = self.carried();
        stop(old);
        self.abandon();
        match self.build(false) {
            Ok((run, notes)) => {
                self.install(run);
                self.rearm();
                self.play_carried(carried);
                self.messages
                    .push("setup: applied; playing again from frame 0".to_owned());
                self.messages.extend(notes.iter().cloned());
                // The clients get `describe` with `restarted` after this reply.
                Ok(json!({
                    "type": "applied",
                    "saved": true,
                    "restarted": true,
                    "frame": 0,
                    "setup": self.setup_json(),
                    "run": self.run_json(),
                    "notes": notes,
                }))
            }
            Err(error) => {
                self.messages
                    .push(format!("setup: applied, not playing: {error}"));
                Ok(json!({
                    "type": "applied",
                    "saved": true,
                    "restarted": false,
                    "setup": self.setup_json(),
                    "error": error,
                }))
            }
        }
    }

    /// App mode, after the Setup was saved: while playing, the run stops and
    /// plays again from frame 0 with it, as the app's Apply does.
    fn apply_app(&mut self) -> Value {
        if let Some(mode) = self.app.as_mut() {
            mode.message = None;
        }
        if self.run.is_none() {
            return json!({"type": "applied", "saved": true, "restarted": false, "setup": self.setup_json()});
        }
        self.stop_app();
        match self.play_app() {
            Ok(()) => {
                let notes = self.app.as_ref().map_or_else(Vec::new, |m| m.notes.clone());
                if let Some(mode) = self.app.as_mut() {
                    mode.message = Some("applied; playing again from frame 0".to_owned());
                }
                json!({
                    "type": "applied",
                    "saved": true,
                    "restarted": true,
                    "frame": 0,
                    "setup": self.setup_json(),
                    "run": self.run_json(),
                    "notes": notes,
                })
            }
            Err(failure) => {
                if let Some(mode) = self.app.as_mut() {
                    mode.message = Some(failure.message.clone());
                }
                json!({
                    "type": "applied",
                    "saved": true,
                    "restarted": false,
                    "setup": self.setup_json(),
                    "error": failure.message,
                })
            }
        }
    }

    /// Edits waiting for reports of a stopped run.
    fn abandon(&mut self) {
        self.commands.clear();
        for (_, pending) in std::mem::take(&mut self.acks) {
            let ack = json!({"type": "ack", "id": pending.id, "outcome": "abandoned"});
            self.send_text(pending.client, &ack);
        }
    }

    /// Vector subscriptions whose port the plan now has with another number
    /// of elements or another rate divisor (a swap replaced its type) take
    /// the new shape, and their client is told (`vector.reshaped`), so a
    /// stream never carries another port's rows (the adapter refuses to
    /// bind a slot whose width differs from the port's). With `restart`,
    /// a reshaped slot is started again on the running Controller.
    fn reshape_vectors(&mut self, restart: bool) {
        let plan = self.plan.as_ref();
        let mut told = Vec::new();
        for (&sub, s) in &mut self.subs {
            let Stream::Vector {
                slot,
                port,
                rows,
                rate,
                decimation,
                ..
            } = &mut s.stream
            else {
                continue;
            };
            let Some((n, divisor)) = vector_shape(plan, port) else {
                continue;
            };
            let d = vector_decimation(*rate, self.rate, divisor);
            if (n, d) == (*rows, *decimation) {
                continue;
            }
            (*rows, *decimation) = (n, d);
            if restart && let (Some(plan), Some(run)) = (plan, self.run.as_mut()) {
                let _ = run.controller.vector_start(plan, *slot, port.clone(), n, d);
            }
            told.push((
                s.client,
                json!({"type": "vector.reshaped", "sub": sub, "rows": n, "decimation": d}),
            ));
        }
        for (client, message) in told {
            self.send_text(client, &message);
        }
    }

    /// The scope and vector subscriptions, on a new run's Controller.
    fn rearm(&mut self) {
        self.reshape_vectors(false);
        let (Some(plan), Some(run)) = (self.plan.as_ref(), self.run.as_mut()) else {
            return;
        };
        for sub in self.subs.values_mut() {
            if let Stream::Scope {
                slot,
                port,
                decimation,
                bound,
            } = &mut sub.stream
            {
                *bound = false;
                let _ = run.controller.scope_start(*slot, port.clone(), *decimation);
            }
            if let Stream::Vector {
                slot,
                port,
                rows,
                decimation,
                bound,
                ..
            } = &mut sub.stream
            {
                *bound = false;
                let _ = run
                    .controller
                    .vector_start(plan, *slot, port.clone(), *rows, *decimation);
            }
        }
    }

    fn stats(&self) -> Value {
        let clients: Vec<Value> = self
            .clients
            .iter()
            .map(|(id, c)| json!({"client": id, "dropped": c.dropped}))
            .collect();
        let controller = self.run.as_ref().map(|r| &r.controller);
        json!({
            "type": "stats",
            "frame": controller.map(|c| c.status().next_frame.0),
            "clients": clients,
            "evicted": self.evicted,
            "meterOverflows": controller.map(Controller::meter_overflows),
            "scopeOverflows": controller.map(Controller::scope_overflows),
            "vectorOverflows": controller.map(Controller::vector_overflows),
            "commandOverflows": controller.map(|c| c.command_overflows()),
            "clipped": controller.map(Controller::clipped),
            "writes": self.writes,
        })
    }

    fn send_text(&mut self, client: u64, value: &Value) {
        let Some(c) = self.clients.get(&client) else {
            return;
        };
        match c.queue.try_send(Outbound::Text(value.to_string())) {
            Ok(()) => {}
            // A client that cannot take a reply is not keeping up.
            Err(TrySendError::Full(_)) => self.evict(client),
            Err(TrySendError::Disconnected(_)) => self.remove(client),
        }
    }

    fn send_binary(&mut self, client: u64, bytes: Vec<u8>) {
        let Some(c) = self.clients.get_mut(&client) else {
            return;
        };
        match c.queue.try_send(Outbound::Binary(bytes)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => c.dropped += 1,
            Err(TrySendError::Disconnected(_)) => self.remove(client),
        }
    }

    fn broadcast(&mut self, value: &Value) {
        let clients: Vec<u64> = self.clients.keys().copied().collect();
        for client in clients {
            self.send_text(client, value);
        }
    }

    /// Flags the client and drops its queue: its thread sees either and
    /// ends on its own; nothing here waits for it.
    fn evict(&mut self, client: u64) {
        if let Some(c) = self.clients.remove(&client) {
            c.evicted.store(true, Ordering::Relaxed);
            self.evicted += 1;
        }
        self.release(client);
    }

    fn remove(&mut self, client: u64) {
        self.clients.remove(&client);
        self.release(client);
    }

    fn release(&mut self, client: u64) {
        let subs: Vec<u32> = self
            .subs
            .iter()
            .filter(|(_, s)| s.client == client)
            .map(|(&id, _)| id)
            .collect();
        for sub in subs {
            self.drop_sub(sub);
        }
    }
}

/// `buffer` of a message (1 to 8192 frames), else `default`.
fn buffer_of(message: &Value, default: u32) -> Result<u32, Failure> {
    match message.get("buffer") {
        None => Ok(default),
        Some(v) => v
            .as_u64()
            .filter(|b| (1..=8192).contains(b))
            .map(|b| b as u32)
            .ok_or_else(|| fail("bad-value", "buffer is 1 to 8192 frames")),
    }
}

/// The refusal of an app message outside app mode.
/// The folder of `path`: its parent for a project's `project.toml`.
fn folder_of(path: &Path) -> PathBuf {
    match path.parent() {
        Some(dir) if path.file_name().is_some_and(|n| n == PROJECT_FILE) && path.is_file() => {
            dir.to_owned()
        }
        _ => path.to_owned(),
    }
}

fn app_only() -> Failure {
    fail(
        "not-allowed",
        "only Signal.app's server opens projects and plays or stops them",
    )
}

/// Level `level` of a `graph` message: `""` the top level, else a composite.
fn level_of<'a>(graph: &'a Value, level: &str) -> Result<&'a Value, Failure> {
    let found = if level.is_empty() {
        graph.get("top")
    } else {
        graph.get("composites").and_then(|c| c.get(level))
    };
    found.ok_or_else(|| {
        fail(
            "unknown-level",
            format!("the project uses no composite `{level}`"),
        )
    })
}

/// The refusal of what needs the app switched on.
fn powered_off() -> Failure {
    fail("powered-off", "Signal is switched off")
}

/// The power switch as `welcome`, `app` and `power` carry it.
fn power_json(power: app::Power) -> Value {
    json!(if power.is_on() { "on" } else { "off" })
}

/// The output device's name, for `setup.get`'s run.
fn device_name(output: &Output) -> String {
    match output {
        Output::Device(driver) => driver.name().to_owned(),
        Output::Null(..) => "null".to_owned(),
    }
}

/// Stops a run on this thread; its Runner and streams are dropped here.
fn stop(mut run: Run) {
    let _ = run.output.stop();
}

/// The folder's `ui/`, which the HTTP server serves under `/project/`.
pub fn project_ui(dir: &Path) -> PathBuf {
    dir.join("ui")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// auditor-1-rate-panic: no client rate panics; the unsupported ones are
    /// refused with `bad-rate`.
    #[test]
    fn rates_are_checked_without_panicking() {
        fn refused<T>(result: Result<T, Failure>) -> bool {
            matches!(
                result,
                Err(Failure {
                    code: "bad-rate",
                    ..
                })
            )
        }
        for rate in [
            json!(1e-20),
            json!(1e-320),
            json!(0),
            json!(-1),
            json!(0.5),
            json!(f64::NAN),
            json!("NaN"),
            json!(null),
        ] {
            let message = json!({"rate": rate});
            assert!(refused(rate_interval(&message)), "{message}");
            assert!(refused(scope_decimation(&message, 48_000)), "{message}");
        }
        let huge = json!({"rate": 1e300});
        assert_eq!(
            rate_interval(&huge).ok(),
            Some(Duration::from_secs_f64(1.0 / 60.0))
        );
        assert_eq!(scope_decimation(&huge, 48_000).ok(), Some(1));
        let slow = json!({"rate": 1});
        assert_eq!(rate_interval(&slow).ok(), Some(Duration::from_secs(1)));
        assert_eq!(scope_decimation(&slow, 48_000).ok(), Some(48_000));
        assert_eq!(
            rate_interval(&json!({})).ok(),
            Some(Duration::from_secs_f64(1.0 / 30.0))
        );
        assert_eq!(scope_decimation(&json!({}), 48_000).ok(), Some(1));
        // A range whose ends cross (an export's targets) clamps, never panics.
        assert_eq!(within(5.0, 3.0, 1.0), 1.0);
        assert_eq!(within(-5.0, 0.0, 1.0), 0.0);
        assert_eq!(
            scope_decimation(&json!({"rate": 4800}), 48_000).ok(),
            Some(10)
        );
    }
}
