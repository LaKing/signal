//! The app's state, without a window (ADR 0013): the Setup, the project
//! folder, the devices, and while playing the driver, its `Controller`, the
//! file watch and the meters. The window sends [`Command`]s and draws the
//! [`View`] snapshots it gets back; it holds no signal state.
//!
//! Playing reuses what `signal play --watch` uses: the CoreAudio driver and
//! input capture of `signal-audio`, and `signal_project::live::reload` for
//! file edits, run here on the control thread. Nothing here runs on the
//! audio thread.
//!
//! Meters during an edit: `reload` polls the Controller into the session
//! recorder, which drops meters. The blocks the audio thread meters while
//! an edit is planned and its swap prepared (the time that takes depends on
//! the project) are therefore not seen here: a peak in that gap is missing
//! from the peak and its hold, the RMS window covers fewer blocks for a
//! while, and a clip in it may not latch the indicator. The status line's
//! clipped-sample count comes from the runner and still counts it.
//!
//! The parameter panel ([`crate::panel`]): a drag sends the value to the
//! running engine as a 5 ms ramp through the Controller; the release writes
//! it once into `project.toml` (`signal_project::write_parameter`) and gives
//! the running project the same edit, so the file watch's reload of the
//! written file finds nothing to apply.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use signal_audio::{
    AudioError, ChannelMap, CoreAudioDriver, DeviceInfo, DeviceRef, DeviceSelector, Direction,
    InputCallback, InputCapture, InputMeters, Resolution, Scheduling, StatsSnapshot, choose_output,
    labels, resolve,
};
use signal_core::{Curve, EventPayload, Frame, PortAddress, Registry};
use signal_engine::{
    ControlSink, Controller, Driver, LogLine, Meter, NullDriver, ParameterValue, Provenance,
    RealtimeConfig, Report, RunnerStatus, SentCommand, Stamp, realtime_with,
};
use signal_graph::ExecutionPlan;
use signal_project::live::{ProjectWatcher, ReloadError, WATCH_INTERVAL_MS, reload};
use signal_project::session::{Edit, LiveRecorder, SessionRecorder, sink_of};
use signal_project::{LoadError, PROJECT_FILE, Project, located, write_parameter};

use crate::meter::{ChannelMeter, Reading};
use crate::panel::{InstanceView, Panel, ramp_frames};
use crate::setup::{DeviceEntry, Setup, Window};

/// Room for input meters between two control steps.
const INPUT_METERS: usize = 4096;

/// What the window asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Opens the project folder (stopping playback).
    OpenProject(PathBuf),
    /// Lists the devices again (refused while playing).
    RefreshDevices,
    /// Takes the interface settings into the Setup and saves it; while
    /// playing, stops and starts again from frame 0 with them.
    Apply(Box<Interface>),
    /// Starts playing the project.
    Play,
    /// Stops playing.
    Stop,
    /// Clears a meter's clip indicator.
    ClearClip {
        /// An input meter (else an output meter).
        input: bool,
        /// The channel (from 0).
        channel: usize,
    },
    /// Stops, records the window's size (if known) and saves the Setup.
    Exit(Option<Window>),
    /// A parameter's slider moved: while playing, the value is sent as a
    /// ramp of [`RAMP_MS`](crate::panel::RAMP_MS). Clamped to its range.
    Drag {
        /// The top-level instance.
        instance: String,
        /// Its parameter (for a composite, an exported one).
        parameter: String,
        /// The new base.
        value: f64,
    },
    /// The slider was released: the dragged value is written to
    /// `project.toml`, once.
    Release {
        /// The top-level instance.
        instance: String,
        /// Its parameter.
        parameter: String,
    },
    /// Resets a parameter to its default: sent as a ramp while playing, and
    /// its key removed from `project.toml`.
    Reset {
        /// The top-level instance.
        instance: String,
        /// Its parameter.
        parameter: String,
    },
}

/// What Apply does with the Setup's output or input device.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Selection {
    /// Keeps the Setup's device: the dialog could not find it, and no other
    /// was chosen (a missing saved device is never replaced unasked).
    #[default]
    Keep,
    /// No device: the system's default output, or no input.
    Unset,
    /// This device.
    Device(DeviceRef),
}

impl Selection {
    fn apply(self, saved: &mut Option<DeviceEntry>) {
        match self {
            Selection::Keep => {}
            Selection::Unset => *saved = None,
            Selection::Device(device) => *saved = Some(DeviceEntry::from_ref(device)),
        }
    }
}

/// The interface settings the dialog applies.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Interface {
    /// The output device.
    pub output: Selection,
    /// The input device, metered only.
    pub input: Selection,
    /// The buffer in frames.
    pub buffer: u32,
    /// "CPU workers": threads per block, the audio thread included; `None`
    /// is auto.
    pub cpu_workers: Option<u32>,
    /// The device channel (from 1) of each sink channel; identity without.
    pub output_channels: Option<Vec<u16>>,
}

/// Where audio goes.
#[derive(Clone, Debug)]
pub enum Backend {
    /// CoreAudio, through `signal-audio`.
    CoreAudio,
    /// No device: the null driver, run with [`AppState::run_null`], and this
    /// device list. For tests and scripting.
    Null(Vec<DeviceInfo>),
}

/// One meter as the window shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct MeterView {
    /// `out 1 → 3`, `in 2`.
    pub label: String,
    /// Its reading.
    pub reading: Reading,
}

/// The status line.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Status {
    /// The next frame to play.
    pub frame: u64,
    /// `frame` in seconds.
    pub elapsed: f64,
    /// Output and input xruns.
    pub xruns: u64,
    /// The callback load, average and worst, as fractions of the period.
    pub load_average: f64,
    /// The worst callback load.
    pub load_worst: f64,
    /// Samples clamped to full scale (or non-finite).
    pub clipped: u64,
    /// The buffer requested, in frames.
    pub buffer: u32,
    /// The threads a block runs on, the audio thread included.
    pub threads: usize,
    /// The worker threads.
    pub workers: usize,
    /// How the workers are scheduled: `workgroup`, `time-constraint`,
    /// `none` or `mixed` ([`Scheduling::path`]).
    pub scheduling: &'static str,
}

/// A device list for the dialog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceList {
    /// Every device.
    pub devices: Vec<DeviceInfo>,
    /// The output devices as (index, label), same-named ones told apart.
    pub outputs: Vec<(usize, String)>,
    /// The input devices as (index, label).
    pub inputs: Vec<(usize, String)>,
    /// Why there is no list.
    pub error: Option<String>,
}

/// A snapshot of the state, for the window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// The Setup file.
    pub setup_path: String,
    /// The Setup.
    pub setup: Option<Setup>,
    /// What the Setup file had wrong when read.
    pub setup_problems: Vec<String>,
    /// The project folder.
    pub project: String,
    /// The project's sample rate, once it loaded.
    pub sample_rate: Option<u32>,
    /// Its diagnostics when it does not load.
    pub diagnostics: Vec<String>,
    /// The devices.
    pub devices: DeviceList,
    /// Where the saved output is in the list (`None` without one).
    pub output: Option<Resolution>,
    /// Where the saved input is in the list.
    pub input: Option<Resolution>,
    /// Whether it plays.
    pub playing: bool,
    /// The status line (of the last run once stopped).
    pub status: Option<Status>,
    /// The last file-watch outcome, and whether it was an error.
    pub reload: Option<(String, bool)>,
    /// Notes on the run: the channel map, the input.
    pub notes: Vec<String>,
    /// The last refusal or notice.
    pub message: Option<String>,
    /// The output meters.
    pub outputs: Vec<MeterView>,
    /// The input meters.
    pub inputs: Vec<MeterView>,
    /// The parameter panel: the top-level instances and their parameters.
    pub panel: Vec<InstanceView>,
    /// Files the panel has written since the app started.
    pub writes: u64,
}

enum Output {
    // Boxed, as the null driver: both are large (the CoreAudio driver keeps
    // its workers' scheduling, M2.7-1 its ports too).
    Device(Box<CoreAudioDriver>),
    // The map is what a device would play (the null driver plays the
    // identity).
    Null(Box<NullDriver>, ChannelMap),
}

enum Input {
    Device(InputCapture),
    Null(Box<InputCallback>, InputMeters),
}

impl Input {
    fn meters(&mut self) -> &mut InputMeters {
        match self {
            Input::Device(capture) => capture.meters(),
            Input::Null(_, meters) => meters,
        }
    }
}

struct Playing {
    output: Output,
    controller: Controller,
    live: LiveRecorder,
    watcher: Option<ProjectWatcher>,
    watched: Instant,
    /// The last reload was refused or could not read the folder: it is tried
    /// again every watch interval, without waiting for another change, until
    /// one succeeds (the watch has already taken the change it saw).
    retry: bool,
    dir: PathBuf,
    current: Project,
    input: Option<Input>,
    rate: u32,
    buffer: u32,
    outputs: Vec<(String, ChannelMeter)>,
    inputs: Vec<(String, ChannelMeter)>,
    /// The last base and effective value the runner reported per parameter.
    values: BTreeMap<PortAddress, ParameterValue>,
    /// How the worker threads were scheduled at the start.
    scheduling: Scheduling,
}

/// Feeds the recorder, the output meters and the parameter values from one
/// Controller poll.
struct Sink<'a> {
    live: &'a mut LiveRecorder,
    meters: &'a mut [(String, ChannelMeter)],
    values: &'a mut BTreeMap<PortAddress, ParameterValue>,
}

impl ControlSink for Sink<'_> {
    fn report(&mut self, report: &Report, command: &SentCommand) {
        self.live.report(report, command);
    }
    fn log(&mut self, line: LogLine<'_>) {
        self.live.log(line);
    }
    fn meter(&mut self, meter: &Meter) {
        if let Some((_, channel)) = self.meters.get_mut(meter.channel) {
            channel.push(meter);
        }
    }
    fn parameter(&mut self, port: &PortAddress, value: ParameterValue) {
        match self.values.get_mut(port) {
            Some(known) => *known = value,
            None => {
                self.values.insert(port.clone(), value);
            }
        }
    }
    fn frontier(&mut self, next_frame: Frame) {
        self.live.frontier(next_frame);
    }
}

/// The app's state.
pub struct AppState {
    registry: Registry,
    backend: Backend,
    setup_path: PathBuf,
    setup: Setup,
    setup_problems: Vec<String>,
    project_dir: String,
    sample_rate: Option<u32>,
    diagnostics: Vec<String>,
    devices: DeviceList,
    playing: Option<Playing>,
    status: Option<Status>,
    reload_status: Option<(String, bool)>,
    notes: Vec<String>,
    message: Option<String>,
    panel: Panel,
    writes: u64,
}

impl AppState {
    /// The state for the Setup at `setup_path` (read now; its problems are
    /// shown), opening `project` or else the Setup's last project.
    pub fn new(setup_path: PathBuf, project: Option<PathBuf>, backend: Backend) -> AppState {
        let (setup, setup_problems) = Setup::load(&setup_path);
        let mut registry = Registry::new();
        if let Err(error) = signal_std::register(&mut registry) {
            eprintln!("signal-app: the standard modules did not register: {error}");
        }
        let mut state = AppState {
            registry,
            backend,
            setup_path,
            setup,
            setup_problems,
            project_dir: String::new(),
            sample_rate: None,
            diagnostics: Vec::new(),
            devices: DeviceList::default(),
            playing: None,
            status: None,
            reload_status: None,
            notes: Vec::new(),
            message: None,
            panel: Panel::default(),
            writes: 0,
        };
        state.list_devices();
        if let Some(dir) = project.or_else(|| state.setup.last_project.clone()) {
            state.open(&dir);
            // Saved with the next change to the Setup.
            state.setup.last_project = Some(dir);
        }
        state
    }

    /// The Setup in force.
    pub fn setup(&self) -> &Setup {
        &self.setup
    }

    /// Whether it plays.
    pub fn is_playing(&self) -> bool {
        self.playing.is_some()
    }

    /// Carries out `command`.
    pub fn apply(&mut self, command: Command) {
        self.message = None;
        match command {
            Command::OpenProject(dir) => {
                self.stop();
                self.open(&dir);
                self.setup.last_project = Some(dir);
                self.save();
            }
            Command::RefreshDevices => {
                if self.playing.is_some() {
                    self.message = Some(
                        "stop playback to refresh: listing devices pauses metering and status"
                            .to_owned(),
                    );
                } else {
                    self.list_devices();
                }
            }
            Command::Apply(interface) => {
                let interface = *interface;
                interface.output.apply(&mut self.setup.output);
                interface.input.apply(&mut self.setup.input);
                self.setup.buffer = interface.buffer.max(1);
                self.setup.cpu_workers = interface.cpu_workers.map(|n| n.max(1));
                self.setup.output_channels = interface.output_channels;
                self.save();
                if self.playing.is_some() {
                    self.stop();
                    self.play();
                    if self.playing.is_some() {
                        self.message = Some("applied; playing again from frame 0".to_owned());
                    }
                }
            }
            Command::Play => {
                if self.playing.is_none() {
                    self.play();
                }
            }
            Command::Stop => {
                if self.playing.is_some() {
                    self.stop();
                    self.list_devices();
                }
            }
            Command::ClearClip { input, channel } => {
                if let Some(playing) = self.playing.as_mut() {
                    let meters = if input {
                        &mut playing.inputs
                    } else {
                        &mut playing.outputs
                    };
                    if let Some((_, meter)) = meters.get_mut(channel) {
                        meter.clear_clip();
                    }
                }
            }
            Command::Exit(window) => {
                self.stop();
                if window.is_some() {
                    self.setup.window = window;
                }
                self.save();
            }
            Command::Drag {
                instance,
                parameter,
                value,
            } => self.drag(&instance, &parameter, value),
            Command::Release {
                instance,
                parameter,
            } => {
                if let Some((value, sent)) = self.panel.release(&instance, &parameter) {
                    self.write_back(&instance, &parameter, Some(value), sent);
                }
            }
            Command::Reset {
                instance,
                parameter,
            } => {
                match self.panel.target(&instance, &parameter) {
                    Some(target) => {
                        // Each port back to its own default: an export's
                        // targets can have different ones. Its send is the
                        // latest, so it alone says what the ports hold.
                        let sends = target.ports.iter().cloned().zip(target.defaults);
                        let sent = self.send_ramps(sends.collect());
                        self.panel.release(&instance, &parameter);
                        self.write_back(&instance, &parameter, None, sent);
                    }
                    None => self.not_editable(&instance, &parameter),
                }
            }
        }
    }

    /// One control step: drains the runner's reports and meters and the
    /// input meters, and every 100 ms applies changed project files.
    pub fn tick(&mut self) {
        let Some(playing) = self.playing.as_mut() else {
            return;
        };
        playing.controller.poll(&mut Sink {
            live: &mut playing.live,
            meters: &mut playing.outputs,
            values: &mut playing.values,
        });
        if let Some(input) = playing.input.as_mut() {
            let meters = &mut playing.inputs;
            input.meters().drain(|meter| {
                if let Some((_, channel)) = meters.get_mut(meter.channel) {
                    channel.push(meter);
                }
            });
        }
        if playing.watched.elapsed() >= Duration::from_millis(WATCH_INTERVAL_MS) {
            playing.watched = Instant::now();
            let changed = match playing.watcher.as_mut().map(ProjectWatcher::poll) {
                Some(Ok(changed)) => changed,
                Some(Err(error)) => {
                    self.reload_status = Some((format!("watch: {error}"), true));
                    false
                }
                None => false,
            };
            if changed || playing.retry {
                playing.retry = false;
                self.reload_status = Some(
                    match reload(
                        &mut playing.controller,
                        &mut playing.live,
                        &mut playing.current,
                        &playing.dir,
                        &self.registry,
                    ) {
                        Ok(applied) => {
                            self.panel
                                .replace(Panel::new(&playing.current, &self.registry));
                            (applied.to_string(), false)
                        }
                        Err(ReloadError::Invalid(diagnostics)) => (
                            format!(
                                "{}; keeping the last good version",
                                diagnostics
                                    .last()
                                    .map(ToString::to_string)
                                    .unwrap_or_default()
                            ),
                            true,
                        ),
                        Err(ReloadError::Failed(message)) => {
                            playing.retry = true;
                            (
                                format!("{message}; keeping the last good version, retrying"),
                                true,
                            )
                        }
                    },
                );
            }
        }
        self.status = Some(status_of(playing));
    }

    /// The snapshot the window draws.
    pub fn view(&self) -> View {
        let resolution = |entry: &Option<DeviceEntry>, direction| {
            entry
                .as_ref()
                .map(|e| resolve(&self.devices.devices, direction, &e.to_ref()))
        };
        let meters = |list: &[(String, ChannelMeter)]| {
            list.iter()
                .map(|(label, meter)| MeterView {
                    label: label.clone(),
                    reading: meter.reading(),
                })
                .collect()
        };
        View {
            setup_path: self.setup_path.display().to_string(),
            setup: Some(self.setup.clone()),
            setup_problems: self.setup_problems.clone(),
            project: self.project_dir.clone(),
            sample_rate: self.sample_rate,
            diagnostics: self.diagnostics.clone(),
            devices: self.devices.clone(),
            output: resolution(&self.setup.output, Direction::Output),
            input: resolution(&self.setup.input, Direction::Input),
            playing: self.playing.is_some(),
            status: self.status,
            reload: self.reload_status.clone(),
            notes: self.notes.clone(),
            message: self.message.clone(),
            outputs: self
                .playing
                .as_ref()
                .map_or_else(Vec::new, |p| meters(&p.outputs)),
            inputs: self
                .playing
                .as_ref()
                .map_or_else(Vec::new, |p| meters(&p.inputs)),
            panel: match &self.playing {
                Some(playing) => self.panel.view(&playing.values),
                None => self.panel.view(&BTreeMap::new()),
            },
            writes: self.writes,
        }
    }

    /// The commands the running engine applied so far, as the session
    /// records them (empty when stopped).
    pub fn edits(&self) -> Vec<Edit> {
        self.playing
            .as_ref()
            .map_or_else(Vec::new, |p| p.live.recorder().edits())
    }

    /// Null backend only: runs `frames` frames through the null driver.
    pub fn run_null(&mut self, frames: u64) -> Result<(), String> {
        match self.playing.as_mut().map(|p| &mut p.output) {
            Some(Output::Null(driver, _)) => driver.run(frames, || {}).map_err(|e| e.to_string()),
            _ => Err("not playing on the null driver".to_owned()),
        }
    }

    /// Null backend only: one input callback of `data` (interleaved frames
    /// of the input's channels), through the input callback body.
    pub fn push_input(&mut self, data: &[f32]) -> Result<(), String> {
        match self.playing.as_mut().and_then(|p| p.input.as_mut()) {
            Some(Input::Null(callback, _)) => {
                callback.capture(data);
                Ok(())
            }
            _ => Err("no null input is playing".to_owned()),
        }
    }

    fn save(&mut self) {
        if let Err(error) = self.setup.save(&self.setup_path) {
            self.message = Some(format!(
                "the Setup was not saved to {}: {error}",
                self.setup_path.display()
            ));
        }
    }

    fn list_devices(&mut self) {
        let listed = match &self.backend {
            Backend::CoreAudio => signal_audio::devices(),
            Backend::Null(devices) => Ok(devices.clone()),
        };
        self.devices = match listed {
            Ok(devices) => DeviceList {
                outputs: labels(&devices, Direction::Output),
                inputs: labels(&devices, Direction::Input),
                devices,
                error: None,
            },
            Err(error) => DeviceList {
                error: Some(error.to_string()),
                ..DeviceList::default()
            },
        };
    }

    /// Loads the project at `dir` for its rate and diagnostics.
    fn open(&mut self, dir: &Path) {
        self.project_dir = dir.display().to_string();
        self.reload_status = None;
        match load(dir, &self.registry) {
            Ok((project, _, _)) => {
                self.sample_rate = Some(project.sample_rate);
                self.diagnostics.clear();
                self.panel = Panel::new(&project, &self.registry);
            }
            Err(diagnostics) => {
                self.sample_rate = None;
                self.diagnostics = diagnostics;
                self.panel = Panel::default();
            }
        }
    }

    fn not_editable(&mut self, instance: &str, parameter: &str) {
        self.refuse(format!(
            "{instance}.{parameter} is not editable here: only the parameters of a top-level \
             instance are (a parameter inside a composite is set in the composite's file)"
        ));
    }

    /// A slider moved: the value is held as the drag's and, while playing,
    /// sent.
    fn drag(&mut self, instance: &str, parameter: &str, value: f64) {
        let Some(target) = self.panel.target(instance, parameter) else {
            return self.not_editable(instance, parameter);
        };
        if !value.is_finite() {
            return self.refuse(format!("{instance}.{parameter}: the value must be finite"));
        }
        let value = value.clamp(target.min, target.max);
        let sent = self.send_ramps(target.ports.into_iter().map(|p| (p, value)).collect());
        self.panel.drag(instance, parameter, value, sent);
    }

    /// While playing, moves each port to its value over the ramp, stamped by
    /// default. Whether the Controller took every one (true when stopped:
    /// there is nothing to send).
    fn send_ramps(&mut self, sends: Vec<(PortAddress, f64)>) -> bool {
        let Some(playing) = self.playing.as_mut() else {
            return true;
        };
        let frames = ramp_frames(playing.rate);
        for (port, to) in sends {
            let payload = EventPayload::Ramp {
                to,
                frames,
                curve: Curve::Linear,
            };
            if let Err(error) = playing
                .controller
                .send_event(port.clone(), payload, Stamp::Default)
            {
                self.message = Some(format!(
                    "{port}: {error}; the value is applied when the written file is reloaded"
                ));
                return false;
            }
        }
        true
    }

    /// Writes `value` (`None`: the default, by removing the key) into
    /// `project.toml`. When the last send reached every port (`sent`), the
    /// running project takes the same edit, so the watch's reload of the
    /// written file finds nothing to apply. When the Controller refused it
    /// for some port, the ports may hold anything sent before, so the running
    /// project takes a value no port has in the file
    /// ([`Target::unlike`](crate::panel::Target::unlike)): that reload's
    /// diff then sends every port the file's value.
    fn write_back(&mut self, instance: &str, parameter: &str, value: Option<f64>, sent: bool) {
        let Some(target) = self.panel.target(instance, parameter) else {
            return self.not_editable(instance, parameter);
        };
        let order: Vec<&str> = target.order.iter().map(String::as_str).collect();
        let dir = PathBuf::from(&self.project_dir);
        let path = dir.join(PROJECT_FILE);
        let written = fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| {
                write_parameter(&text, instance, parameter, value, &order)
                    .map_err(|e| e.to_string())
            })
            .and_then(|text| write_file(&path, &text).map_err(|e| e.to_string()));
        if let Err(error) = written {
            return self.refuse(format!(
                "{instance}.{parameter} was not written to {}: {error}",
                path.display()
            ));
        }
        self.writes += 1;
        match self.playing.as_mut() {
            Some(playing) => {
                match playing
                    .current
                    .with_parameter(instance, parameter, value, &order)
                {
                    Ok(project) => {
                        self.panel.replace(Panel::new(&project, &self.registry));
                        let unlike = target.unlike(value).filter(|_| !sent);
                        let running = match unlike {
                            Some(unlike) => playing.current.with_parameter(
                                instance,
                                parameter,
                                Some(unlike),
                                &order,
                            ),
                            // Sent, or a single-valued range (every send was
                            // that value).
                            None => Ok(project),
                        };
                        match running {
                            Ok(running) => playing.current = running,
                            Err(error) => {
                                self.message = Some(format!(
                                    "{instance}.{parameter}: the running project could not \
                                     take the refused value ({error}); the next reload applies \
                                     what differs from the file"
                                ));
                            }
                        }
                    }
                    Err(error) => {
                        self.message = Some(format!(
                            "{instance}.{parameter} was written, but the running project could \
                             not take it ({error}); the next reload applies the file"
                        ));
                    }
                }
            }
            None => match load(&dir, &self.registry) {
                Ok((project, _, _)) => self.panel.replace(Panel::new(&project, &self.registry)),
                Err(diagnostics) => self.diagnostics = diagnostics,
            },
        }
    }

    fn refuse(&mut self, message: String) {
        self.message = Some(message);
    }

    /// Where `entry` (if any) is among the devices of `direction`: the
    /// device to open, or why none is.
    fn selector(
        &self,
        entry: &DeviceEntry,
        direction: Direction,
    ) -> Result<(DeviceSelector, &DeviceInfo), String> {
        match resolve(&self.devices.devices, direction, &entry.to_ref()) {
            Resolution::Found(index) => {
                let Some(device) = self.devices.devices.get(index) else {
                    return Err("the device list changed".to_owned());
                };
                match &device.id {
                    Some(id) => Ok((DeviceSelector::Id(id.clone()), device)),
                    // Opening by name takes the first of that name: only
                    // when the name is unique.
                    None if self.devices.devices.iter().any(|d| {
                        d.direction == direction
                            && d.name == device.name
                            && !std::ptr::eq(d, device)
                    }) =>
                    {
                        Err(format!(
                            "the {} '{}' has no id and shares its name; it cannot be opened",
                            match direction {
                                Direction::Output => "output",
                                Direction::Input => "input",
                            },
                            device.name
                        ))
                    }
                    None => Ok((DeviceSelector::Name(device.name.clone()), device)),
                }
            }
            Resolution::Ambiguous { reason, .. } | Resolution::Missing { reason, .. } => {
                Err(reason)
            }
        }
    }

    fn play(&mut self) {
        self.notes.clear();
        let dir = PathBuf::from(&self.project_dir);
        if self.project_dir.is_empty() {
            return self.refuse("open a project folder first".to_owned());
        }
        let (project, mut plan, width) = match load(&dir, &self.registry) {
            Ok(loaded) => loaded,
            Err(diagnostics) => {
                self.diagnostics = diagnostics;
                return self.refuse("the project does not load".to_owned());
            }
        };
        self.diagnostics.clear();
        self.sample_rate = Some(project.sample_rate);
        self.panel.replace(Panel::new(&project, &self.registry));
        let (rate, seed, buffer) = (project.sample_rate, project.seed, self.setup.buffer);
        let output = match &self.setup.output {
            Some(entry) => match self.selector(entry, Direction::Output) {
                Ok((selector, device)) => Some((selector, device.clone())),
                Err(reason) => return self.refuse(reason),
            },
            None => None,
        };
        let targets: Option<Vec<usize>> = self.setup.output_channels.as_ref().map(|c| {
            c.iter()
                .map(|&c| usize::from(c).saturating_sub(1))
                .collect()
        });
        let map_note = |error: AudioError, channels: &[u16]| {
            format!("output channel map {channels:?}: {error}; playing on the identity map",)
        };
        let opened = match &self.backend {
            Backend::CoreAudio => {
                let selector = output.map_or(DeviceSelector::Default, |(s, _)| s);
                let open = |targets| {
                    CoreAudioDriver::open_selected(&selector, rate, buffer, width, targets)
                };
                match open(targets.clone()) {
                    Err(error @ AudioError::ChannelMap(_)) => {
                        let channels = self.setup.output_channels.clone().unwrap_or_default();
                        self.notes.push(map_note(error, &channels));
                        open(None)
                    }
                    other => other,
                }
                .map(|driver| Output::Device(Box::new(driver)))
            }
            Backend::Null(_) => {
                // As `CoreAudioDriver::open_selected` chooses, on the listed
                // device (the default output is taken to be that wide).
                let wanted = targets
                    .as_deref()
                    .map_or(width, |t| ChannelMap::width_for(width, t));
                let channels = match &output {
                    Some((_, device)) => {
                        choose_output(device, rate, buffer, wanted).map(|c| usize::from(c.channels))
                    }
                    None => Ok(wanted),
                };
                channels.map(|channels| {
                    let map = match (targets, &self.setup.output_channels) {
                        (Some(targets), Some(saved)) => ChannelMap::with_targets(
                            width, channels, targets,
                        )
                        .unwrap_or_else(|error| {
                            self.notes.push(map_note(error, saved));
                            ChannelMap::new(width, channels)
                        }),
                        _ => ChannelMap::new(width, channels),
                    };
                    Output::Null(Box::new(NullDriver::new(rate, channels, seed)), map)
                })
            }
        };
        let mut output = match opened {
            Ok(output) => output,
            Err(error) => return self.refuse(error.to_string()),
        };
        let target = |sink: usize| match &output {
            Output::Device(driver) => driver.map().target(sink),
            Output::Null(_, map) => map.target(sink),
        };
        let outputs: Vec<(String, ChannelMeter)> = (0..width)
            .map(|sink| {
                let label = match target(sink) {
                    Some(device) => format!("out {} → {}", sink + 1, device + 1),
                    None => format!("out {} (not played)", sink + 1),
                };
                (label, ChannelMeter::new(rate))
            })
            .collect();

        plan.version = 0;
        let mut config = RealtimeConfig::with_buffer(rate, seed, u64::from(buffer));
        config.engine.threads = self.setup.cpu_workers.map_or(0, |n| n as usize);
        // Advanced Setup keys, not in the dialog (M2.7-1).
        config.rt.policy = self.setup.rt_policy;
        config.rt.computation = self.setup.rt_computation;
        let made = realtime_with(
            &plan,
            &self.registry,
            config,
            Some(signal_audio::rt_thread::mach_semaphores),
        );
        let (runner, controller) = match made {
            Ok(run) => run,
            Err(error) => return self.refuse(error.to_string()),
        };
        let recorder =
            match SessionRecorder::new(&project, &self.registry, rate, seed, Provenance::default())
            {
                Ok(recorder) => recorder,
                Err(error) => return self.refuse(error.to_string()),
            };
        let workers = runner.workers();
        let started = match &mut output {
            Output::Device(driver) => driver.start(runner),
            Output::Null(driver, _) => driver.start(runner),
        };
        if let Err(error) = started {
            return self.refuse(error.to_string());
        }
        let scheduling = match &output {
            Output::Device(driver) => driver.scheduling().cloned(),
            Output::Null(..) => None,
        }
        .unwrap_or_else(|| Scheduling::ordinary(workers.threads(), workers.count(), "null driver"));

        let input = self.open_input(rate, buffer);
        let inputs = input.as_ref().map_or(0, |(_, channels)| *channels);
        let watcher = match ProjectWatcher::new(&dir) {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                self.reload_status = Some((format!("not watching: {error}"), true));
                None
            }
        };
        self.reload_status = watcher
            .as_ref()
            .map(|_| ("watching for edits".to_owned(), false));
        let playing = Playing {
            output,
            controller,
            live: LiveRecorder::new(recorder, self.registry.clone()),
            watcher,
            watched: Instant::now(),
            retry: false,
            dir,
            current: project,
            input: input.map(|(input, _)| input),
            rate,
            buffer,
            outputs,
            inputs: (0..inputs)
                .map(|channel| (format!("in {}", channel + 1), ChannelMeter::new(rate)))
                .collect(),
            values: BTreeMap::new(),
            scheduling,
        };
        self.status = Some(status_of(&playing));
        self.playing = Some(playing);
    }

    /// The input, metered only, and its channels; a note when the Setup's
    /// input cannot be used (playing goes on without it).
    fn open_input(&mut self, rate: u32, buffer: u32) -> Option<(Input, usize)> {
        let entry = self.setup.input.clone()?;
        let (selector, device) = match self.selector(&entry, Direction::Input) {
            Ok((selector, device)) => (selector, device.clone()),
            Err(reason) => {
                self.notes.push(format!("no input meters: {reason}"));
                return None;
            }
        };
        let opened = match &self.backend {
            Backend::CoreAudio => {
                InputCapture::open(&selector, rate, buffer, INPUT_METERS).map(|mut capture| {
                    let channels = capture.meters().channels();
                    (Input::Device(capture), channels)
                })
            }
            Backend::Null(_) => signal_audio::choose_input(&device, rate, buffer).map(|choice| {
                let channels = usize::from(choice.channels);
                let (callback, meters) = InputCallback::new(channels, INPUT_METERS);
                (Input::Null(Box::new(callback), meters), channels)
            }),
        };
        match opened {
            Ok(input) => Some(input),
            Err(error) => {
                self.notes.push(format!("no input meters: {error}"));
                None
            }
        }
    }

    fn stop(&mut self) {
        let Some(mut playing) = self.playing.take() else {
            return;
        };
        let stopped = match &mut playing.output {
            Output::Device(driver) => driver.stop(),
            Output::Null(driver, _) => driver.stop(),
        };
        if let Err(error) = &stopped {
            self.message = Some(format!("stopping: {error}"));
        }
        playing.controller.poll(&mut Sink {
            live: &mut playing.live,
            meters: &mut playing.outputs,
            values: &mut playing.values,
        });
        let mut status = status_of(&playing);
        if let Ok(Some(runner)) = stopped {
            status.frame = runner.status().next_frame.0;
            status.clipped = runner.status().clipped;
            status.elapsed = status.frame as f64 / f64::from(playing.rate);
        }
        self.status = Some(status);
        // The streams, the Runner and the input are dropped here, on the
        // control thread.
    }
}

/// The project at `dir`, its plan and its sink's width; or its diagnostics,
/// located in the folder.
fn load(dir: &Path, registry: &Registry) -> Result<(Project, ExecutionPlan, usize), Vec<String>> {
    let project = Project::load(dir).map_err(|error| match error {
        LoadError::Invalid(diagnostics) => strings(located(dir, diagnostics)),
        error => vec![error.to_string()],
    })?;
    let planned = project
        .plan(registry)
        .map_err(|diagnostics| strings(located(dir, diagnostics)))?;
    let (_, width) = sink_of(&project, &planned.plan).map_err(|d| strings(located(dir, d)))?;
    Ok((project, planned.plan, width))
}

/// Writes `text` to `path` through a hidden temporary file beside it (the
/// watch ignores hidden files), synced, then renamed over `path`, so a reader
/// sees the old file or the new one.
fn write_file(path: &Path, text: &str) -> io::Result<()> {
    let mut name = std::ffi::OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(".tmp");
    let temporary = path.with_file_name(name);
    let written = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

fn strings(diagnostics: Vec<signal_project::Diagnostic>) -> Vec<String> {
    diagnostics.iter().map(ToString::to_string).collect()
}

fn status_of(playing: &Playing) -> Status {
    let status: RunnerStatus = playing.controller.status();
    let stats = match &playing.output {
        Output::Device(driver) => driver.stats().snapshot(),
        Output::Null(..) => StatsSnapshot::default(),
    };
    let input_errors = match &playing.input {
        Some(Input::Device(capture)) => capture.stream_errors(),
        _ => 0,
    };
    Status {
        frame: status.next_frame.0,
        elapsed: status.next_frame.0 as f64 / f64::from(playing.rate.max(1)),
        xruns: stats.xruns + input_errors,
        load_average: stats.average_callback_ratio,
        load_worst: stats.worst_callback_ratio,
        clipped: status.clipped,
        buffer: playing.buffer,
        threads: playing.scheduling.threads,
        workers: playing.scheduling.workers.len(),
        scheduling: playing.scheduling.path(),
    }
}
