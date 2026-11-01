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

mod app_mode;
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

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};
use signal_audio::{DeviceInfo, DeviceRef, Direction, Resolution, labels, resolve};
use signal_core::{Direction as PortDirection, EventPayload, PortAddress, PortShape, Registry};
use signal_engine::{
    CommandId, ParameterValue, Provenance, Report, RtOptions, RtPolicy, RunnerStatus, Stamp,
};
use signal_graph::ExecutionPlan;
use signal_output::{
    Controller, Meter, SCOPE_SLOTS, ScopeBlock, VECTOR_MAX_ROWS, VECTOR_SLOTS, VectorSnapshot,
};
use signal_project::Diagnostic;
use signal_project::archive::TempProject;
use signal_project::live::{FolderState, ProjectWatcher};
use signal_project::session::LiveRecorder;
use signal_project::transaction::{self};

use crate::PROTOCOL;
use crate::app::{self};
use crate::document::Document;
use crate::frames::{self, frame};
use crate::keys::{self, Keys};
use crate::panel::Panel;
use crate::player::{Backend, Interface, Selection};
use crate::server::{Inbound, Outbound, Server, ServerOptions};
use crate::setup::{
    Choice, DEFAULT_BUFFER, Setup, Window, capabilities, channels, found, rate_check,
};

use app_mode::{AppMode, app_only, power_json, powered_off};
use failure::{Failure, fail};
use live::{Failed, Pending};
use project_io::SaveWait;
use queue::{Job, LIVE_EDITS, Waiting};
use run::{Muted, Run, stop};
use setup_keys::buffer_of;
use subs::{rate_interval, rate_of, scope_decimation, vector_decimation, vector_shape};

/// Client messages handled per step at most.
const MESSAGES_PER_STEP: usize = 256;

/// The event input a `note` goes to (ADR 0039): `std.note_seq`'s.
pub const AUDITION_INPUT: &str = "audition";
/// A `note` without a `velocity` (ADR 0039): the piano roll's default note's.
const NOTE_VELOCITY: f64 = 0.8;

/// How often the keyboard shortcuts file is looked at (ADR 0034).
const KEYS_POLL: Duration = Duration::from_millis(250);

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
    /// The output ceiling of every run ([`HostConfig::ceiling`]).
    ceiling: Option<f64>,
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
    /// Instances muted since the last poll ([`Observed`]).
    muted: Vec<Muted>,
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
    /// App mode: the document's saves not yet answered, in the order asked.
    saves: Vec<SaveWait>,
    /// Controls whose last write failed.
    failed: BTreeMap<(String, String), Failed>,
    /// Controls written while their live send was refused: the reload of
    /// the written file sends them the file's value.
    resend: BTreeSet<(String, String)>,
    /// Live edits of plan parameter ports that are no control: the value,
    /// and whether the Controller took it.
    port_edits: BTreeMap<PortAddress, (f64, bool)>,
    /// Live notes each client holds (`note`, ADR 0039): the port and key,
    /// ended when the client goes so that no note is left sounding.
    auditions: BTreeMap<u64, BTreeSet<(PortAddress, u8)>>,
    /// The note-offs of notes whose client went (or was evicted) that the
    /// Controller has not taken yet (a full command queue): sent again
    /// each step until it takes them. At most one per port and key, so
    /// bounded by the plan's audition inputs times 128 keys.
    releases: BTreeSet<(PortAddress, u8)>,
    next_failure: u64,
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
    /// The free scope slots last pushed (`scopes`), and when they last rose.
    scopes_pushed: Option<usize>,
    scopes_freed: Option<Instant>,
    /// A scope slot was released since the last `scopes` push.
    scopes_released: bool,
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
    /// ADR 0046 §8: the SHA-256 of every presentation text a patch of this
    /// host wrote or replaced (`ui/…`): a client's `panel.restore` of one
    /// of them (an undo or redo the host made) is taken whole; any other
    /// text is judged strictly ([`Host::panel_restore_allowed`]).
    restorable: std::collections::HashSet<String>,
    /// ADR 0048: the file requests waiting for
    /// another writer to release the project's lock, in order.
    waiting: VecDeque<Waiting>,
    /// The open project's generation: a new one each time another project
    /// (or a new copy of it) is opened, so a request queued for the old one
    /// is never run on the new one.
    generation: u64,
    /// How long a request waits for the lock before it fails.
    lock_wait: Duration,
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
            ceiling: config.ceiling,
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
            muted: Vec::new(),
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
            saves: Vec::new(),
            failed: BTreeMap::new(),
            resend: BTreeSet::new(),
            port_edits: BTreeMap::new(),
            auditions: BTreeMap::new(),
            releases: BTreeSet::new(),
            next_failure: 1,
            unsaved_pushed: None,
            graph_pushed: None,
            graph_stopped: None,
            graph_playing: None,
            counts: (0, 0, 0),
            reloads: 0,
            #[cfg(feature = "test-hooks")]
            before_lock: None,
            scopes_pushed: None,
            scopes_freed: None,
            scopes_released: false,
            idle_watch: None,
            keys,
            keys_stamp,
            keys_polled: Instant::now(),
            keys_version: 0,
            restorable: std::collections::HashSet::new(),
            waiting: VecDeque::new(),
            // Unique across runs of the app (a page reconnecting to a new
            // run never matches an old opening); no semantics depend on it.
            generation: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(1, |d| u64::try_from(d.as_millis()).unwrap_or(1)),
            lock_wait: transaction::LOCK_WAIT,
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
                    "generation": self.generation,
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
    fn respond(&mut self, client: u64, id: Value, kind: &str, message: &Value) {
        let result = match kind {
            "describe" => Ok(Some(self.describe(false))),
            "graph" => self.graph().map(Some),
            "schematic.move" => self.schematic_move(message).map(Some),
            "schematic.reset" => self.schematic_reset(message).map(Some),
            "modules" => self.modules(message).map(Some),
            "patch" => self.patch(message).map(Some),
            "subscribe" => self.subscribe(client, message).map(Some),
            "unsubscribe" => self.unsubscribe(client, message).map(Some),
            "set" | "ramp" | "trigger" | "note" => {
                self.edit(client, &id, kind, message).map(|()| None)
            }
            "commit" => self.commit(message).map(Some),
            "snapshot.list" => self.snapshot_list().map(Some),
            "snapshot.save" => self.snapshot_save(message).map(Some),
            "snapshot.restore" => self.snapshot_restore(message).map(Some),
            "snapshot.delete" => self.snapshot_delete(message).map(Some),
            "snapshot.undo" => self.snapshot_undo(message).map(Some),
            "snapshot.move" => self.snapshot_move(message).map(Some),
            "snapshot.transition" => self.snapshot_transition(message).map(Some),
            "snapshot.exclude" => self.snapshot_exclude(message).map(Some),
            "retry" => self.retry(message).map(Some),
            "revert" => self.revert(message).map(Some),
            "panel.check" => self.panel_check(message).map(Some),
            "panel.read" => self.panel_read(message).map(Some),
            "panel.write" => self.panel_write(message).map(Some),
            "pattern.read" => self.pattern_read(message).map(Some),
            "pattern.write" => self.pattern_write(message).map(Some),
            "file.read" => self.file_read(message).map(Some),
            "file.write" => self.file_write(message).map(Some),
            "vendor" => self.vendor(message).map(Some),
            "slot.projects" => self.slot_projects().map(Some),
            "setup.get" => Ok(Some(self.setup_get())),
            "setup.devices" => self.setup_devices(message).map(Some),
            "setup.check" => self.setup_check(message).map(Some),
            "setup.apply" => self.setup_apply(message).map(Some),
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
            "project.save" => self.project_save(client, &id),
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
        match result {
            Ok(Some(reply)) => {
                let restarted =
                    kind == "setup.apply" && reply.get("restarted") == Some(&json!(true));
                // The plan or what plays changed: every client is told.
                let changed = matches!(
                    kind,
                    "transport.play"
                        | "transport.stop"
                        | "project.open"
                        | "project.revert"
                        | "app.power"
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
                        "plate": reply.get("plate").cloned().unwrap_or(Value::Null),
                        "html": message.get("html").cloned().unwrap_or(Value::Null),
                    });
                    self.broadcast(&pushed);
                }
                // Every client learns of the written pattern (ADR 0028 §7).
                if kind == "pattern.write" {
                    let pushed = json!({
                        "type": "pattern.written",
                        "name": reply.get("name").cloned().unwrap_or(Value::Null),
                        "base": reply.get("base").cloned().unwrap_or(Value::Null),
                        "text": message.get("text").cloned().unwrap_or(Value::Null),
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

    /// The scope slots of a run, as `describe` and `scopes` give them:
    /// `free` now, and `sinceFreedMs` since the free count last rose.
    fn scopes_json(&self) -> Value {
        json!({
            "slots": SCOPE_SLOTS,
            "free": self.scope_slots.iter().filter(|s| s.is_none()).count(),
            "sinceFreedMs": self.scopes_freed.map(|at| at.elapsed().as_millis()),
        })
    }

    /// Pushes `scopes` to every client whenever the free scope slot count
    /// changed since the last push, or a slot was released (`sinceFreedMs`
    /// restarted) though another was taken in the same step.
    fn push_scopes(&mut self) {
        let free = self.scope_slots.iter().filter(|s| s.is_none()).count();
        let released = std::mem::take(&mut self.scopes_released);
        if self.scopes_pushed == Some(free) && !released {
            return;
        }
        self.scopes_pushed = Some(free);
        let message = with(self.scopes_json(), "type", json!("scopes"));
        self.broadcast(&message);
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
                let port: PortAddress = self
                    .through_slot(address)?
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
                let port: PortAddress = self
                    .through_slot(address)?
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
                // Every release restarts the age, even if another client
                // takes the slot before the next push.
                self.scopes_freed = Some(Instant::now());
                self.scopes_released = true;
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

    /// The keyboard shortcuts in force (ADR 0034): the person's
    /// `keys.toml` as last read, checked.
    pub fn keys(&self) -> &Keys {
        &self.keys
    }

    /// How many times the keymap changed since the host started (Signal.app
    /// gives its menu items their chords again on a new one).
    pub fn keys_version(&self) -> u64 {
        self.keys_version
    }

    /// Looks at `keys.toml` every [`KEYS_POLL`]: a change made elsewhere (an
    /// editor, another Signal) applies at once, pushed to every client.
    fn watch_keys(&mut self) {
        if self.keys_polled.elapsed() < KEYS_POLL {
            return;
        }
        self.keys_polled = Instant::now();
        let path = keys::keys_path(&self.setup_path);
        if keys::stamp(&path) != self.keys_stamp {
            self.reload_keys();
        }
    }

    /// Reads `keys.toml` again, tells the operator what is wrong with it and
    /// pushes `keys` to every client.
    fn reload_keys(&mut self) {
        let path = keys::keys_path(&self.setup_path);
        self.keys_stamp = keys::stamp(&path);
        self.keys = Keys::load(&path);
        self.keys_version += 1;
        for problem in self.keys.report() {
            self.messages.push(format!("keys: {problem}"));
        }
        let pushed = self.keys.message();
        self.broadcast(&pushed);
    }

    /// `keys.set`: command `command` gets the chords `keys` (null: its
    /// built-in ones) in `keys.toml`, written at once; every client is
    /// pushed the new keymap.
    fn keys_set(&mut self, message: &Value) -> Result<Value, Failure> {
        let command = message
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| fail("bad-message", "keys.set needs a command"))?;
        let chords: Option<Vec<String>> = match message.get("keys") {
            None | Some(Value::Null) => None,
            Some(Value::Array(list)) => Some(
                list.iter()
                    .map(|k| k.as_str().map(str::to_owned))
                    .collect::<Option<Vec<String>>>()
                    .ok_or_else(|| fail("bad-message", "keys must be a list of chords"))?,
            ),
            Some(_) => {
                return Err(fail(
                    "bad-message",
                    "keys must be a list of chords, or null",
                ));
            }
        };
        let path = keys::keys_path(&self.setup_path);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(fail("write-failed", format!("{}: {error}", path.display()))),
        };
        let next = keys::with_override(&text, command, chords.as_deref())
            .map_err(|reason| fail("bad-value", reason))?;
        keys::write_atomic(&path, &next)
            .map_err(|error| fail("write-failed", format!("{}: {error}", path.display())))?;
        self.reload_keys();
        Ok(self.keys.message())
    }

    /// `keys.reset`: `command` back to its built-in chords, or (without
    /// one) every command.
    fn keys_reset(&mut self, message: &Value) -> Result<Value, Failure> {
        if let Some(command) = message.get("command").and_then(Value::as_str) {
            let reset = json!({"command": command, "keys": Value::Null});
            return self.keys_set(&reset);
        }
        let path = keys::keys_path(&self.setup_path);
        keys::reset(&path).map_err(|reason| fail("write-failed", reason))?;
        self.reload_keys();
        Ok(self.keys.message())
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
            // A chosen device by identity (`{"name", "id", "signature"}`),
            // found in the list as it is now, or by index.
            Some(Value::Object(object)) if !object.contains_key("index") => {
                let Selection::Device(wanted) =
                    self.selection(message.get("output"), Direction::Output)?
                else {
                    return Err(fail("bad-device", "not a device"));
                };
                match resolve(&self.devices, Direction::Output, &wanted) {
                    Resolution::Found(index) => self
                        .devices
                        .get(index)
                        .ok_or_else(|| fail("unknown-device", "the device list changed"))?,
                    Resolution::Ambiguous { reason, .. } | Resolution::Missing { reason, .. } => {
                        return Err(fail("unknown-device", reason));
                    }
                }
            }
            Some(Value::Object(object)) => {
                let index = object.get("index").unwrap_or(&Value::Null);
                self.device(index, Direction::Output)?
            }
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
        // ADR 0036: `"buffer": "device"` follows the device's buffer; a
        // number given replaces it, even the one the Setup holds.
        match message.get("buffer") {
            Some(Value::String(device)) if device == "device" => self.setup.buffer_device = true,
            Some(_) => self.setup.buffer_device = false,
            None => {}
        }
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
        let result = if self.run.is_some() {
            self.stop_app();
            self.play_app()
        } else if self.app.as_ref().is_some_and(|m| {
            // ADR 0033: with `plays`, a project left stopped by a failure
            // (not by a `transport.stop`) plays with the Setup just applied.
            m.plays && m.opened && m.power.is_on() && !m.held
        }) {
            self.play_app()
        } else {
            return json!({"type": "applied", "saved": true, "restarted": false, "setup": self.setup_json()});
        };
        match result {
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
            "ceiling": self.ceiling,
            "ceilinged": controller.map(Controller::ceilinged),
            "writes": self.writes,
        })
    }

    fn send_text(&mut self, client: u64, value: &Value) {
        self.send_serialised(client, value.to_string());
    }

    fn send_serialised(&mut self, client: u64, text: String) {
        let Some(c) = self.clients.get(&client) else {
            return;
        };
        match c.queue.try_send(Outbound::Text(text)) {
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

    /// A message already serialised, to every client.
    fn broadcast_text(&mut self, text: &str) {
        let clients: Vec<u64> = self.clients.keys().copied().collect();
        for client in clients {
            self.send_serialised(client, text.to_owned());
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
        self.end_notes(client);
    }

    fn remove(&mut self, client: u64) {
        self.clients.remove(&client);
        self.release(client);
        self.end_notes(client);
    }

    /// A client gone or evicted with a key held (a closed page, a lost
    /// connection, a stalled socket): its notes end, unacknowledged, so
    /// none is left sounding. A note-off the Controller refuses (its
    /// command queue full) waits in `releases` and is sent again by every
    /// step until it is taken ([`Host::send_releases`]).
    fn end_notes(&mut self, client: u64) {
        // Its live edits still waiting behind the writer lock are dropped:
        // a note it can no longer end, or a value it can no longer see,
        // is never played for it (its file writes still run, as asked).
        self.waiting.retain(|w| match &w.job {
            Job::Message {
                client: c,
                lock: false,
                kind,
                ..
            } => *c != client || !LIVE_EDITS.contains(&kind.as_str()),
            _ => true,
        });
        if let Some(held) = self.auditions.remove(&client) {
            self.releases.extend(held);
        }
        self.send_releases();
    }

    /// Sends the waiting note-offs; keeps those the Controller refuses.
    fn send_releases(&mut self) {
        if self.releases.is_empty() {
            return;
        }
        let Some(run) = self.run.as_mut() else {
            // No run: nothing sounds.
            self.releases.clear();
            return;
        };
        self.releases.retain(|(port, key)| {
            let off = EventPayload::Set(signal_std::audition_value(*key, 0.0, false));
            run.controller
                .send_event(port.clone(), off, Stamp::Default)
                .is_err()
        });
    }

    /// Test hooks: the note-offs still waiting for the Controller.
    #[cfg(feature = "test-hooks")]
    pub fn releases_waiting(&self) -> usize {
        self.releases.len()
    }

    /// `address` as an event input of the running plan (`trigger`, `note`).
    /// ADR 0037 §7: `address` translated through a top-level slot's match
    /// (`/fx.y` → the plan port behind the content's `out`); another
    /// address as it is. A slot port that reaches nothing now (the slot
    /// empty, the port unmatched) is `inactive`.
    fn through_slot(&self, address: &str) -> Result<String, Failure> {
        match self.panel.slot_port(address) {
            Some(crate::panel::SlotPort::Port(port)) => Ok(port.to_string()),
            Some(crate::panel::SlotPort::Inactive(why)) => Err(fail(
                "inactive",
                format!("{address} reaches nothing now: {why}"),
            )),
            None => Ok(address.to_owned()),
        }
    }

    fn event_input(&self, address: &str) -> Result<PortAddress, Failure> {
        let address = &self.through_slot(address)?;
        let port: PortAddress = address
            .parse()
            .map_err(|e| fail("bad-address", format!("{address}: {e}")))?;
        let found = self.plan.as_ref().is_some_and(|plan| {
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
        if found {
            Ok(port)
        } else {
            Err(fail(
                "unknown-address",
                format!("{address} is not an event input of the running plan"),
            ))
        }
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

/// The folder's `ui/`, which the HTTP server serves under `/project/`.
pub fn project_ui(dir: &Path) -> PathBuf {
    dir.join("ui")
}
