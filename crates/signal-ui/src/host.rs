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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use signal_audio::{
    ChannelMap, CoreAudioDriver, DeviceInfo, DeviceRef, Direction, Resolution, labels, resolve,
    rt_thread,
};
use signal_core::{
    Curve, Direction as PortDirection, EventPayload, Frame, PortAddress, PortShape, Registry,
};
use signal_engine::{
    CommandId, ControlSink, HangLine, Outcome, ParameterValue, Provenance, RealtimeConfig, Report,
    RtOptions, RunnerStatus, SentCommand, Stamp,
};
use signal_graph::ExecutionPlan;
use signal_output::{
    Controller, Meter, NullDriver, SCOPE_POINTS, SCOPE_SLOTS, ScopeBlock, realtime_with,
};
use signal_project::live::{Both, ProjectWatcher, ReloadError, WATCH_INTERVAL_MS, reload_observed};
use signal_project::session::{LiveRecorder, SessionRecorder, sink_of};
use signal_project::{Diagnostic, LoadError, Project, located};

use crate::PROTOCOL;
use crate::frames::{self, frame};
use crate::panel::{Panel, ramp_frames, write_project};
use crate::player::{Backend, Input, Interface, Output, Selection, open_input, open_output};
use crate::server::{Inbound, Outbound, Server, ServerOptions};
use crate::setup::{Setup, capabilities, channels, rate_check};

/// Client messages handled per step at most.
const MESSAGES_PER_STEP: usize = 256;
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
    rate: u32,
    buffer: u32,
    threads: u32,
    width: usize,
    input_channels: usize,
    device: String,
    /// The engine's plan version last described.
    version: u64,
    failure_reported: bool,
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
}

/// A refused message: its code and why.
struct Failure {
    code: &'static str,
    message: String,
}

fn fail(code: &'static str, message: impl Into<String>) -> Failure {
    Failure {
        code,
        message: message.into(),
    }
}

/// Where an edit goes: a control's ports or one plan parameter.
struct Aim {
    ports: Vec<PortAddress>,
    min: f64,
    max: f64,
    control: Option<(String, String)>,
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
    values: BTreeMap<PortAddress, ParameterValue>,
    reports: Vec<Report>,
    meters: Vec<Meter>,
    inputs: Vec<Meter>,
    blocks: Vec<ScopeBlock>,
    commands: BTreeMap<CommandId, u64>,
    acks: BTreeMap<u64, Pending>,
    next_ack: u64,
    writes: u64,
    messages: Vec<String>,
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

    fn with_server(config: HostConfig, server: Server) -> Result<Host, String> {
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
            values: BTreeMap::new(),
            reports: Vec::new(),
            meters: Vec::new(),
            inputs: Vec::new(),
            blocks: Vec::new(),
            commands: BTreeMap::new(),
            acks: BTreeMap::new(),
            next_ack: 1,
            writes: 0,
            messages: Vec::new(),
        };
        host.list_devices();
        let (run, notes) = host.build(true)?;
        host.messages.extend(notes);
        host.install(run);
        Ok(host)
    }

    /// The URL to open: the page, with the WebSocket port and the session
    /// token in its fragment.
    pub fn url(&self) -> String {
        self.server.url()
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
        for _ in 0..MESSAGES_PER_STEP {
            match self.server.inbound.try_recv() {
                Ok(message) => self.inbound(message),
                Err(_) => break,
            }
        }
        self.emit();
        std::mem::take(&mut self.messages)
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
        let device = match &output {
            Output::Device(driver) => driver.name().to_owned(),
            Output::Null(..) => "null".to_owned(),
        };
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
            rate,
            buffer,
            threads,
            width,
            input_channels,
            device,
            version: 0,
            failure_reported: false,
        };
        Ok((run, notes))
    }

    fn install(&mut self, mut run: Run) {
        self.panel = Panel::new(&run.current, &self.registry);
        self.plan = run.current.plan(&self.registry).ok().map(|p| p.plan);
        self.rate = run.rate;
        run.version = run.controller.status().version;
        self.values.clear();
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
                    bound.push((s.client, sub, now));
                }
            }
        }
        for (client, sub, now) in bound {
            let kind = if now { "scope.bound" } else { "scope.unbound" };
            self.send_text(client, &json!({"type": kind, "sub": sub}));
        }
        self.take_reports();
        self.take_meters();
        self.take_blocks();
        if swapped {
            let describe = self.describe(false);
            self.broadcast(&describe);
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
        let notice = match result {
            Ok(applied) => {
                self.panel = Panel::new(&run.current, &self.registry);
                self.plan = run.current.plan(&self.registry).ok().map(|p| p.plan);
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
        self.take_reports();
        self.broadcast(&notice.0);
        // A swap is described once it commits (`poll`); new bases now.
        if notice.1 {
            let describe = self.describe(false);
            self.broadcast(&describe);
        }
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
                Stream::Scope { .. } => {}
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
                let welcome = json!({
                    "type": "welcome",
                    "protocol": PROTOCOL,
                    "client": client,
                    "project": self.run.as_ref().map(|r| r.current.name.clone()),
                    "sampleRate": self.rate,
                    "frame": self.run.as_ref().map(|r| r.controller.status().next_frame.0),
                    "planVersion": self.run.as_ref().map(|r| r.version),
                });
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
            "subscribe" => self.subscribe(client, &message).map(Some),
            "unsubscribe" => self.unsubscribe(client, &message).map(Some),
            "set" | "ramp" | "trigger" => self.edit(client, &id, kind, &message).map(|()| None),
            "commit" => self.commit(&message).map(Some),
            "setup.get" => Ok(Some(self.setup_get())),
            "setup.devices" => self.setup_devices(&message).map(Some),
            "setup.check" => self.setup_check(&message).map(Some),
            "setup.apply" => self.setup_apply(&message).map(Some),
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
                let reply = with(reply, "id", id);
                self.send_text(client, &reply);
                if restarted {
                    let describe = self.describe(true);
                    self.broadcast(&describe);
                }
            }
            Ok(None) => {}
            Err(failure) => {
                let error = json!({
                    "type": "error",
                    "id": id,
                    "code": failure.code,
                    "message": failure.message,
                });
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
                    "base": p.base,
                    "inFile": p.in_file,
                    "modulated": p.modulated,
                    "combine": target.ports.first().and_then(combine_of),
                }));
            }
        }
        let (output_meters, input_meters): (Vec<Value>, Vec<Value>) = match &self.run {
            Some(run) => (
                (0..run.width)
                    .map(|c| json!({"channel": c, "device": run.output.target(c)}))
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
        });
        if restarted {
            describe = with(describe, "restarted", json!(true));
        }
        describe
    }

    fn subscribe(&mut self, client: u64, message: &Value) -> Result<Value, Failure> {
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
                let is_output = self.plan.as_ref().is_some_and(|plan| {
                    plan.instances
                        .iter()
                        .find(|i| i.address == port.instance())
                        .is_some_and(|i| {
                            i.descriptor.ports.iter().any(|p| {
                                p.name == port.port()
                                    && p.direction == PortDirection::Out
                                    && p.shape == PortShape::Signal
                            })
                        })
                });
                if !is_output {
                    return Err(fail(
                        "not-a-signal-output",
                        format!("{address} is not a signal output of the running plan"),
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
            other => return Err(fail("bad-message", format!("no stream '{other}'"))),
        };
        self.next_sub = self.next_sub.wrapping_add(1).max(1);
        let kind = match &stream {
            Stream::Parameters { .. } => "parameters",
            Stream::Meters { .. } => "meters",
            Stream::Scope { .. } => "scope",
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
        if let Some(Sub {
            stream: Stream::Scope { slot, .. },
            ..
        }) = self.subs.remove(&sub)
        {
            if let Some(s) = self.scope_slots.get_mut(slot) {
                *s = None;
            }
            if let Some(run) = self.run.as_mut() {
                let _ = run.controller.scope_stop(slot);
            }
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
                (
                    aim.ports,
                    EventPayload::Set(value),
                    aim.control.map(|c| (c, value)),
                )
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
                (aim.ports, payload, aim.control.map(|c| (c, to)))
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
        if let Some(((instance, parameter), value)) = drag {
            self.panel
                .drag(&instance, &parameter, value, refused.is_none());
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
        let last = self.panel.release(&instance, &parameter);
        let sent = match value {
            Some(v) => match last {
                Some((sent, true)) if sent.to_bits() == v.to_bits() => true,
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
        let order: Vec<&str> = target.order.iter().map(String::as_str).collect();
        write_project(&self.dir, &instance, &parameter, value, &order)
            .map_err(|error| fail("write-failed", error))?;
        self.writes += 1;
        let mut note = None;
        match self.run.as_mut() {
            Some(run) => match run
                .current
                .with_parameter(&instance, &parameter, value, &order)
            {
                Ok(project) => {
                    self.panel = Panel::new(&project, &self.registry);
                    let unlike = target.unlike(value).filter(|_| !sent);
                    let running = match unlike {
                        Some(unlike) => {
                            run.current
                                .with_parameter(&instance, &parameter, Some(unlike), &order)
                        }
                        None => Ok(project),
                    };
                    match running {
                        Ok(running) => run.current = running,
                        Err(error) => {
                            note = Some(format!(
                                "the running project could not take the refused value ({error}); \
                             the next reload applies what differs from the file"
                            ))
                        }
                    }
                }
                Err(error) => {
                    note = Some(format!(
                        "written, but the running project could not take it ({error}); the next \
                         reload applies the file"
                    ));
                }
            },
            None => {
                if let Ok(project) = Project::load(&self.dir) {
                    self.panel = Panel::new(&project, &self.registry);
                }
            }
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

    fn setup_json(&self) -> Value {
        json!({
            "output": self.setup.output,
            "input": self.setup.input,
            "buffer": self.setup.buffer,
            "cpuWorkers": self.setup.cpu_workers,
            "outputChannels": self.setup.output_channels,
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
        })
    }

    /// `setup.devices`: the device list (refreshed when asked, refused while
    /// playing, as the dialog does), each output's capabilities, where the
    /// Setup's devices are in it, and the compute devices.
    fn setup_devices(&mut self, message: &Value) -> Result<Value, Failure> {
        if message.get("refresh").and_then(Value::as_bool) == Some(true) {
            if self.run.is_some() {
                return Err(fail(
                    "playing",
                    "stop playback to refresh: listing devices pauses metering and status",
                ));
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
        Ok(json!({
            "type": "setup.devices",
            "devices": devices,
            "error": self.devices_error,
            "output": resolution(&self.setup.output, Direction::Output),
            "input": resolution(&self.setup.input, Direction::Input),
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
        if let Err(error) = self.setup.save(&self.setup_path) {
            return Ok(json!({
                "type": "applied",
                "saved": false,
                "restarted": false,
                "error": format!("the Setup was not saved to {}: {error}", self.setup_path.display()),
            }));
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
        stop(old);
        self.abandon();
        match self.build(false) {
            Ok((run, notes)) => {
                self.install(run);
                self.rearm();
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

    /// Edits waiting for reports of a stopped run.
    fn abandon(&mut self) {
        self.commands.clear();
        for (_, pending) in std::mem::take(&mut self.acks) {
            let ack = json!({"type": "ack", "id": pending.id, "outcome": "abandoned"});
            self.send_text(pending.client, &ack);
        }
    }

    /// The scope subscriptions, on a new run's Controller.
    fn rearm(&mut self) {
        let Some(run) = self.run.as_mut() else {
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
