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
//! `transport.play`/`stop`, `project.open`, `project.save`,
//! `project.revert`), with Signal.app's
//! rules ([`crate::app`]): every run plays from the Setup, as the app's state
//! does. A `.signal` project (M2.8-12) plays from its working folder
//! ([`crate::document`]); `signal ui` reopens only the project it serves.

// The project's writer lock is taken only in writer.rs (crates/signal-ui/clippy.toml).
#![deny(clippy::disallowed_methods)]

mod app_mode;
mod audition;
mod clients;
mod controls;
mod failure;
mod files;
mod graph;
mod live;
mod opening;
mod patching;
mod project_io;
mod queue;
mod run;
mod schematic_edits;
mod setup_keys;
mod snapshotting;
mod subs;
mod watch;
mod writer;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};
use signal_audio::DeviceInfo;
use signal_core::Registry;
use signal_engine::{Provenance, Report, RtOptions, RunnerStatus};
use signal_graph::ExecutionPlan;
use signal_output::{Controller, Meter, ScopeBlock, VectorSnapshot};
use signal_project::Diagnostic;
use signal_project::archive::TempProject;
use signal_project::live::{FolderState, ProjectWatcher};
use signal_project::session::LiveRecorder;
use signal_project::transaction::{self};

use crate::PROTOCOL;
use crate::app::{self};
use crate::keys::{self, Keys};
use crate::panel::Panel;
use crate::player::Backend;
use crate::server::{Inbound, Server, ServerOptions};
use crate::setup::{DEFAULT_BUFFER, Setup, Window};

use app_mode::{AppMode, app_only, power_json};
use audition::Auditions;
use clients::{Client, Clients};
use failure::fail;
use live::LiveState;
use opening::Opening;
use project_io::SaveWait;
use queue::LockQueue;
use run::{Muted, Run};
use subs::Subscriptions;
use writer::{Outcome, WriteCtx};

/// Client messages handled per step at most.
const MESSAGES_PER_STEP: usize = 256;

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
    /// The output ceiling of every run ([`signal_output::Runner::set_ceiling`]):
    /// `signal mcp`'s `--max-gain`; `None` for `signal ui` and the app.
    pub ceiling: Option<f64>,
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
    /// Whether the open project plays whenever the app is switched on (ADR
    /// 0033: Signal.app has no Play or Stop; a project starts and stops from
    /// its own transport): at start, after a project opens, and when the
    /// app is switched on again. Signal.app sets it; `false` starts
    /// stopped, as before, for clients that play and stop themselves.
    pub plays: bool,
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
    /// How many parameters are unsaved (live values the files do not hold,
    /// `unsaved`'s `count`).
    pub unsaved: usize,
}

impl ProjectStatus {
    /// Whether File ▸ Revert to Saved… applies: a `.signal` project with
    /// changes since it was last saved or opened (edited, or unsaved live
    /// values). A folder project is written straight to its files, so it
    /// has no saved version to go back to.
    pub fn revertible(&self) -> bool {
        self.file.is_some() && (self.edited || self.unsaved > 0)
    }
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

/// The protocol's host.
pub struct Host {
    registry: Registry,
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
    /// The output ceiling of every run ([`HostConfig::ceiling`]).
    ceiling: Option<f64>,
    server: Server,
    run: Option<Run>,
    panel: Panel,
    plan: Option<ExecutionPlan>,
    rate: u32,
    clients: Clients,
    subs: Subscriptions,
    /// The live values, the edits waiting for their reports, the
    /// controls whose write failed or whose live send waits for a
    /// reload, and the live edits of plan ports that are no control.
    live: LiveState,
    reports: Vec<Report>,
    /// Instances muted since the last poll ([`Observed`]).
    muted: Vec<Muted>,
    meters: Vec<Meter>,
    inputs: Vec<Meter>,
    blocks: Vec<ScopeBlock>,
    snapshots: Vec<VectorSnapshot>,
    writes: u64,
    messages: Vec<String>,
    /// Signal.app's state, in app mode.
    app: Option<AppMode>,
    /// The open project: its folder, generation, `.signal` file or
    /// temporary folder, and the presentation texts it may restore.
    opening: Opening,
    /// App mode: the document's saves not yet answered, in the order asked.
    saves: Vec<SaveWait>,
    /// Live notes each client holds (`note`, ADR 0039), and the note-offs
    /// of those whose client went that the Controller has not taken yet.
    auditions: Auditions,
    /// The `unsaved` message last pushed.
    unsaved_pushed: Option<Value>,
    /// The hash of the `graph` (or `graph.error`) message last pushed
    /// (`docs/ui-protocol.md` "graph").
    graph_pushed: Option<u64>,
    /// Stopped: `graph`'s project name and structure, and `modules`, by the
    /// folder (and its state, its presentation files aside) it was loaded from.
    /// Kept across a run, so stopping again loads nothing when the folder is
    /// unchanged.
    graph_stopped: Option<(PathBuf, Option<FolderState>, String, Value, Value)>,
    /// Playing: `graph`'s project name and structure, by plan version;
    /// forgotten when the run or its project is replaced.
    graph_playing: Option<(u64, String, Value)>,
    /// Test hooks: projects loaded, structures built and schematic writes.
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    counts: (u64, u64, u64),
    /// Live reloads of the running project's files run (the watch's load,
    /// plan and diff; none for a presentation-only change).
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    reloads: u64,
    /// Test hooks: run by `patch` just before it takes the project's writer
    /// lock, as another process's write landing then would.
    #[cfg(feature = "test-hooks")]
    before_lock: Option<Box<dyn FnMut() + Send>>,
    /// App mode, stopped and switched on: the project folder's watch, for
    /// the `graph` push (a run has its own), and when it was last polled.
    idle_watch: Option<(ProjectWatcher, Instant)>,
    /// ADR 0034: the person's keyboard shortcuts (`keys.toml` beside the
    /// Setup), the file's stamp as last read, when it was last looked at,
    /// and how many times the keymap changed.
    keys: Keys,
    keys_stamp: Option<(SystemTime, u64)>,
    keys_polled: Instant,
    keys_version: u64,
    /// ADR 0048: the file requests waiting for another writer to release
    /// the project's lock, in order, and how long one waits.
    queue: LockQueue,
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

/// `value` in `[min, max]`; never panics, whatever the range (`f64::clamp`
/// does when `min > max`).
fn within(value: f64, min: f64, max: f64) -> f64 {
    value.max(min).min(max)
}

/// Diagnostics as lines, for a message.
fn lines(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

impl Host {
    /// Starts the servers and plays the project with `config.first`.
    pub fn new(config: HostConfig) -> Result<Host, String> {
        let server = Server::start(&config.server)
            .map_err(|error| format!("the UI server did not start: {error}"))?;
        Host::with_server(config, server)
    }

    /// In process (`in-process`: `signal mcp` and the tests): a host whose servers bind nothing; clients connect with
    /// [`connect_local`](Host::connect_local) and are served by the same
    /// WebSocket connection code as over TCP.
    #[cfg(feature = "in-process")]
    pub fn new_local(config: HostConfig) -> Result<Host, String> {
        let server = Server::start_local(&config.server)
            .map_err(|error| format!("the UI server did not start: {error}"))?;
        Host::with_server(config, server)
    }

    /// In process (`in-process`): a new client's end of an in-process connection (a
    /// [`new_local`](Host::new_local) host's only way in).
    #[cfg(feature = "in-process")]
    pub fn connect_local(&self) -> crate::LocalStream {
        self.server.connect_local()
    }

    /// In process (`in-process`): a new HTTP client's end of an in-process connection,
    /// served by the HTTP workers (its requests carry `Host: 127.0.0.1:0`).
    #[cfg(feature = "in-process")]
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

    /// In process (`in-process`): [`new_app`](Host::new_app) with servers that bind
    /// nothing (clients connect in process).
    #[cfg(feature = "in-process")]
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
            plays,
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
            ceiling: None,
        };
        let mode = AppMode {
            plays,
            ..AppMode::default()
        };
        let mut host = Host::blank(config, server, Some(mode));
        host.opening.open_root = open_root;
        if let Some(path) = app::initial_project(project, &host.setup) {
            // A refused .signal opens nothing; the window says why.
            if let Err(message) = host.open_path(&path) {
                if let Some(mode) = host.app.as_mut() {
                    mode.message = Some(message);
                }
            } else {
                // Saved with the next change to the Setup.
                app::opened(&mut host.setup, path);
                host.autoplay();
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
        let keys_file = keys::keys_path(&config.setup_path);
        let keys_stamp = keys::stamp(&keys_file);
        let keys = Keys::load(&keys_file);
        let mut host = Host {
            registry: config.registry,
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
            ceiling: config.ceiling,
            server,
            run: None,
            panel: Panel::default(),
            plan: None,
            rate: 0,
            clients: Clients::default(),
            subs: Subscriptions::default(),
            live: LiveState::default(),
            reports: Vec::new(),
            muted: Vec::new(),
            meters: Vec::new(),
            inputs: Vec::new(),
            blocks: Vec::new(),
            snapshots: Vec::new(),
            writes: 0,
            messages: Vec::new(),
            app,
            opening: Opening {
                dir: config.dir,
                // Unique across runs of the app (a page reconnecting to a new
                // run never matches an old opening); no semantics depend on it.
                generation: SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map_or(1, |d| u64::try_from(d.as_millis()).unwrap_or(1)),
                source: config.source,
                temporary: config.temporary,
                open_root: PathBuf::new(),
                document: None,
                restorable: std::collections::HashSet::new(),
            },
            saves: Vec::new(),
            auditions: Auditions::default(),
            unsaved_pushed: None,
            graph_pushed: None,
            graph_stopped: None,
            graph_playing: None,
            counts: (0, 0, 0),
            reloads: 0,
            #[cfg(feature = "test-hooks")]
            before_lock: None,
            idle_watch: None,
            keys,
            keys_stamp,
            keys_polled: Instant::now(),
            keys_version: 0,
            queue: LockQueue::default(),
        };
        for problem in host.keys.report() {
            host.messages.push(format!("keys: {problem}"));
        }
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

    /// In process (`in-process`): what connects in-process clients from another thread.
    #[cfg(feature = "in-process")]
    pub fn local_connector(&self) -> crate::LocalConnector {
        self.server.local_connector()
    }

    /// Signal.app exits: stops playing, records the window's size (when
    /// known) in the Setup and saves it. A `.signal` project's working
    /// folder is removed, unless it is edited and `discard` is false: then it
    /// is kept, and the message says where. A save still running is
    /// finished first (and answered), so that what it saves is not lost and
    /// the folder is not removed under it. Returns what to tell the operator.
    pub fn close(&mut self, window: Option<Window>, discard: bool) -> Vec<String> {
        self.stop_app();
        self.finish_saves();
        if let Some(document) = self.opening.document.take() {
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

    /// The null driver's channels per frame (what `run_null` captures), if
    /// the run plays on it: a routed run's (ADR 0038) may be wider than its
    /// main sink.
    pub fn null_channels(&mut self) -> Option<usize> {
        use signal_output::Driver;
        self.run
            .as_mut()
            .and_then(|r| r.output.null())
            .map(|d| d.channels())
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
        match self.clients.get(client) {
            Some(c) => {
                c.gate.store(held, std::sync::atomic::Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    /// Test hooks: the bytes of large messages (over 64 KiB) the host holds
    /// now, in its inbound queue or as jobs waiting for the writer lock.
    #[cfg(feature = "test-hooks")]
    pub fn large_held(&self) -> usize {
        self.server.large_held()
    }

    /// Test hooks: holds the whole large-message budget until the returned
    /// hold is dropped (None when some of it is held already).
    #[cfg(feature = "test-hooks")]
    pub fn hold_large_budget(&self) -> Option<impl Send + use<>> {
        self.server.hold_large()
    }

    /// Test hooks: `hook` runs in `patch` just before it takes the
    /// project's writer lock (ADR 0040), once per patch.
    #[cfg(feature = "test-hooks")]
    pub fn set_before_lock(&mut self, hook: Option<Box<dyn FnMut() + Send>>) {
        self.before_lock = hook;
    }

    /// Test hooks: the open `.signal` project, for one.
    #[cfg(feature = "test-hooks")]
    pub fn document(&self) -> Option<&crate::document::Document> {
        self.opening.document.as_ref()
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
        // The control thread never waits for the writer lock (ADR 0048).
        let _no_wait = transaction::NoWait::enter(Duration::ZERO);
        self.run_waiting();
        self.poll();
        // Before any client's message: a waiting note-off goes first.
        self.send_releases();
        self.check_output();
        self.retry_device();
        self.watch();
        self.watch_stopped();
        self.watch_keys();
        for _ in 0..MESSAGES_PER_STEP {
            match self.server.inbound.try_recv() {
                Ok(message) => self.inbound(message),
                Err(_) => break,
            }
        }
        self.emit();
        self.push_app();
        self.push_unsaved();
        self.push_scopes();
        std::mem::take(&mut self.messages)
    }

    /// Test hooks: the projects loaded (`app::load`), the `graph`
    /// structures built and the `ui/schematic.toml` writes so far.
    #[cfg(feature = "test-hooks")]
    pub fn graph_counts(&self) -> (u64, u64, u64) {
        self.counts
    }

    /// Test hooks: the live reloads of the running project's files run so
    /// far (each loads, plans and diffs the project).
    #[cfg(feature = "test-hooks")]
    pub fn reloads(&self) -> u64 {
        self.reloads
    }

    /// Test hook: the null backend's device list from now on (a device
    /// switched to another rate, plugged in or out), as CoreAudio would list
    /// it.
    #[cfg(feature = "test-hooks")]
    pub fn set_null_devices(&mut self, devices: Vec<DeviceInfo>) {
        if let Backend::Null(listed) = &mut self.backend {
            *listed = devices;
        }
    }

    /// Test hook: the run's output stopped by the audio system with
    /// `fault`, as [`Output::fault`] reports a device's.
    #[cfg(feature = "test-hooks")]
    pub fn fail_output(&mut self, fault: &str) {
        if self.run.as_ref().is_some_and(|run| run.fault.is_none()) {
            self.output_faulted(fault.to_owned());
        }
    }

    /// Whether the app is switched on (always outside app mode).
    pub fn powered(&self) -> bool {
        self.app.as_ref().is_none_or(|m| m.power.is_on())
    }

    /// Waits up to `timeout` for a client's message and handles it: the
    /// control thread's loop while the app is switched off, which then has
    /// nothing to poll.
    pub fn wait(&mut self, timeout: Duration) {
        let _no_wait = transaction::NoWait::enter(Duration::ZERO);
        self.run_waiting();
        if let Ok(message) = self.server.inbound.recv_timeout(timeout) {
            self.inbound(message);
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
                self.clients.connect(
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
                    "generation": self.opening.generation,
                });
                // Before the client can subscribe: one that connects while
                // the app is off streams nothing (M2.8-11).
                if let Some(mode) = &self.app {
                    welcome = with(welcome, "power", power_json(mode.power));
                }
                self.send_text(client, &welcome);
            }
            // A large message's permit is given back once it is handled,
            // or, queued behind the writer lock, once its job has run.
            Inbound::Text {
                client,
                text,
                permit,
            } => self.handle(client, &text, permit),
            Inbound::Gone { client } => self.remove(client),
        }
    }

    /// A client's message of type `kind`, run, and its reply (with `id`)
    /// and pushes sent.
    /// A writer of the project's files writes them through `ctx`.
    fn respond(&mut self, ctx: &WriteCtx, client: u64, id: Value, kind: &str, message: &Value) {
        let result = match kind {
            "describe" => Ok(Some(self.describe(false))),
            "graph" => self.graph().map(Some),
            "schematic.move" => self.schematic_move(ctx, message).map(Some),
            "schematic.reset" => self.schematic_reset(ctx, message).map(Some),
            "modules" => self.modules(message).map(Some),
            "patch" => self.patch(ctx, message).map(Some),
            "subscribe" => self.subscribe(client, message).map(Some),
            "unsubscribe" => self.unsubscribe(client, message).map(Some),
            "set" | "ramp" | "trigger" | "note" => {
                self.edit(client, &id, kind, message).map(|()| None)
            }
            "commit" => self.commit(ctx, message).map(Some),
            "snapshot.list" => self.snapshot_list().map(Some),
            "snapshot.save" => self.snapshot_save(message).map(Some),
            "snapshot.restore" => self.snapshot_restore(ctx, message).map(Some),
            "snapshot.delete" => self.snapshot_delete(message).map(Some),
            "snapshot.undo" => self.snapshot_undo(ctx, message).map(Some),
            "snapshot.move" => self.snapshot_move(message).map(Some),
            "snapshot.transition" => self.snapshot_transition(message).map(Some),
            "snapshot.exclude" => self.snapshot_exclude(ctx, message).map(Some),
            "retry" => self.retry(ctx, message).map(Some),
            "revert" => self.revert(message).map(Some),
            "panel.check" => self.panel_check(message).map(Some),
            "panel.read" => self.panel_read(message).map(Some),
            "panel.write" => self.panel_write(ctx, message).map(Some),
            "pattern.read" => self.pattern_read(message).map(Some),
            "pattern.write" => self.pattern_write(ctx, message).map(Some),
            "file.read" => self.file_read(message).map(Some),
            "file.write" => self.file_write(ctx, message).map(Some),
            "vendor" => self.vendor(message).map(Some),
            "slot.projects" => self.slot_projects().map(Some),
            "setup.get" => Ok(Some(self.setup_get())),
            "setup.devices" => self.setup_devices(message).map(Some),
            "setup.check" => self.setup_check(message).map(Some),
            "setup.apply" => self.setup_apply(message).map(Some),
            // ADR 0038 (M4.1): the binding is written by `crate::devices`.
            "device.route" => self.device_route(message, true).map(Some),
            "device.unroute" => self.device_route(message, false).map(Some),
            "keys.get" => Ok(Some(self.keys.message())),
            "keys.set" => self.keys_set(message).map(Some),
            "keys.reset" => self.keys_reset(message).map(Some),
            "app.get" => match self.app {
                Some(_) => Ok(Some(self.app_json())),
                None => Err(app_only()),
            },
            "app.power" => self.power(message).map(Some),
            "transport.play" => self.transport(true).map(Some),
            "transport.stop" => self.transport(false).map(Some),
            "project.open" => self.project_open(message).map(Some),
            "project.save" => self.project_save(ctx, client, &id),
            "project.revert" => self.project_revert().map(Some),
            "stats" => Ok(Some(self.stats())),
            "ping" => Ok(Some(json!({
                "type": "pong",
                "frame": self.run.as_ref().map(|r| r.controller.status().next_frame.0),
                "latency": self.run.as_ref().map(|r| r.controller.latency_frames()),
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
        let outcome = Outcome::of(kind, message, id, result);
        self.apply_outcome(client, outcome);
    }

    fn stats(&self) -> Value {
        let (clients, evicted) = self.clients.stats();
        let controller = self.run.as_ref().map(|r| &r.controller);
        json!({
            "type": "stats",
            "frame": controller.map(|c| c.status().next_frame.0),
            "clients": clients,
            "evicted": evicted,
            "meterOverflows": controller.map(Controller::meter_overflows),
            "scopeOverflows": controller.map(Controller::scope_overflows),
            "vectorOverflows": controller.map(Controller::vector_overflows),
            "commandOverflows": controller.map(|c| c.command_overflows()),
            "clipped": controller.map(Controller::clipped),
            "ceiling": self.ceiling,
            "ceilinged": controller.map(Controller::ceilinged),
            "writes": self.writes,
        })
    }
}

/// The folder's `ui/`, which the HTTP server serves under `/project/`.
pub fn project_ui(dir: &Path) -> PathBuf {
    dir.join("ui")
}
