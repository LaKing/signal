//! `signal play`: run a project in real time through CoreAudio (or the null
//! driver), optionally applying file edits live (`--watch`) and recording the
//! session; print statistics at the end. Sessions, the diff and the file
//! watch are `signal-project`'s; the driver is `signal-audio`'s.

use std::cell::Cell;
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use clap::{Args, ValueEnum};
use serde_json::json;
use signal_audio::{
    BufferRequest, CallbackStats, CaptureSideSpec, CaptureSpec, CaptureStatus, CoreAudioDriver,
    Demotions, DeviceSelector, JoinPlan, Scheduling, SecondarySpec, SecondaryState,
    SecondaryStatus, StatsSnapshot, live_rate, output_period, rate_notice, rt_thread,
};
use signal_core::{Frame, Registry};
use signal_engine::{
    BoundaryLine, Computation, ControlSink, HangLine, RealtimeConfig, Report, RtOptions, RtPolicy,
    RunnerStatus, SentCommand, Spread, WakeStats,
};
use signal_output::{
    Controller, Driver, NullDriver, Placement, RoleBinding, Runner, SoundSystem, realtime_routed,
    route,
};
use signal_project::live::{ProjectWatcher, ReloadError, WATCH_INTERVAL_MS, reload};
use signal_project::session::{LiveRecorder, SessionError, SessionRecorder, sink_of};
use signal_project::{LoadError, Project, provenance};

use crate::gpu::{Gpu, uses_gpu};
use crate::render::located;
use crate::{Failure, print_stdout};

/// Which driver plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum DriverKind {
    /// The audio device, through CoreAudio.
    Coreaudio,
    /// No device: the null driver, at real-time pace or `--fast`.
    Null,
}

/// Arguments of `signal play`.
#[derive(Args)]
pub(crate) struct PlayArgs {
    /// The project folder (containing project.toml), or a .signal file
    /// (played from a temporary copy).
    pub(crate) project: PathBuf,
    /// The output device's name (see `signal devices`); the default output
    /// device without it.
    #[arg(long)]
    pub(crate) device: Option<String>,
    /// The device buffer to request, in frames.
    #[arg(long, default_value_t = 128, value_parser = clap::value_parser!(u32).range(1..))]
    pub(crate) buffer: u32,
    /// Play this many seconds (rounded to the nearest frame). Without it,
    /// play until Enter is pressed or stdin closes.
    #[arg(long)]
    pub(crate) seconds: Option<f64>,
    /// Poll the project folder every 100 ms and apply changes live; a
    /// project that does not load keeps the last good version playing.
    #[arg(long)]
    pub(crate) watch: bool,
    /// Write the session (docs/session-format.md) to this new directory at
    /// the end, for `signal replay`.
    #[arg(long)]
    pub(crate) record_session: Option<PathBuf>,
    /// The driver.
    #[arg(long, value_enum, default_value_t = DriverKind::Coreaudio)]
    pub(crate) driver: DriverKind,
    /// Null driver only: run as fast as possible instead of in real time.
    #[arg(long)]
    pub(crate) fast: bool,
    /// Null driver only: write the output to this WAV file at the end (kept
    /// in memory while playing).
    #[arg(long)]
    pub(crate) record_audio: Option<PathBuf>,
    /// With --record-session: also record each delivered result of a latent
    /// (GPU-backed) module, so that replay does not need the same GPU.
    #[arg(long)]
    pub(crate) record_boundary_data: bool,
    /// Threads a block runs on, the audio thread included (0: two thirds
    /// of the performance cores; 1: no worker threads). The output is
    /// identical for every count.
    #[arg(long, default_value_t = 0)]
    pub(crate) threads: u32,
    /// How idle worker threads wait and are woken: park50 (the default),
    /// budget-spin, semaphore or fanout (docs/realtime-rules.md "Wake
    /// policies"). Timing only: the output is identical for every policy.
    #[arg(long, value_parser = parse_policy, default_value = "park50")]
    pub(crate) rt_policy: RtPolicy,
    /// The workers' time-constraint computation, as a fraction of the
    /// device period (more than 0, at most 1); budget-spin spins within it.
    #[arg(long, value_parser = parse_computation, default_value = "0.25")]
    pub(crate) rt_computation: Computation,
    /// Record the wake-up trace (each worker's wake latency, the first and
    /// last worker joining, the slowest task, per callback) and report it.
    #[arg(long)]
    pub(crate) rt_trace: bool,
    /// Plays role ROLE on these device channels of the output device, for
    /// this run only (ADR 0038 §6, M4.1): `--route cue=3,4`. Repeatable;
    /// `main` plays on channels 1 to its width without one, and a role
    /// with none is not heard. `--route cue@DEVICE=1,2` plays it on another
    /// output device, by name, through a clock adapter (ADR 0052).
    #[arg(long, value_name = "ROLE[@DEVICE]=CHANNELS", value_parser = parse_route)]
    pub(crate) route: Vec<RouteArg>,
    /// Captures role ROLE from these input channels, for this run only
    /// (ADR 0038 §4, §7): `--capture voice@AT2020USB+=1`. Without a device,
    /// the system's default input; repeatable. A capture role with none
    /// captures main's; main without one, the default input's channels 1
    /// to its width. Through a clock adapter when the input is not the
    /// output device (ADR 0052 §8). CoreAudio only.
    #[arg(long, value_name = "ROLE[@DEVICE]=CHANNELS", value_parser = parse_route)]
    pub(crate) capture: Vec<RouteArg>,
    /// Sends the `dmx` slots of role ROLE as Art-Net (ADR 0057 §2), for
    /// this run only: `--artnet lights=2.0.0.1/0` (port 6454 and universe 0
    /// unless written). Repeatable; a `dmx` role without one is not sent.
    #[arg(long, value_name = "ROLE=IP[:PORT][/UNIVERSE]", value_parser = parse_artnet_arg)]
    pub(crate) artnet: Vec<(String, signal_output::ArtNetTarget)>,
    /// Takes the `osc` slots of role ROLE from OSC messages received on
    /// this address (ADR 0057 §1), for this run only: `--osc
    /// surface=0.0.0.0:9000`; a value `<name>` of the slot reads the last
    /// `/<role>/<name>` message's number. Repeatable; a role without one
    /// reads 0.
    #[arg(long, value_name = "ROLE=IP:PORT", value_parser = parse_osc_arg)]
    pub(crate) osc: Vec<(String, String)>,
    /// Takes the `midi` slots of role ROLE from the MIDI sources whose
    /// names contain NAME (`*`: every source) through CoreMIDI (ADR 0057
    /// §1, §3), for this run only: `--midi keys=Arturia`. Repeatable; a
    /// role without one reads 0.
    #[arg(long, value_name = "ROLE=NAME")]
    pub(crate) midi: Vec<String>,
}

/// `--osc`'s value: `role=IP:PORT`.
pub(crate) fn parse_osc_arg(text: &str) -> Result<(String, String), String> {
    let (role, address) = text
        .split_once('=')
        .ok_or_else(|| format!("`{text}` is not ROLE=IP:PORT (surface=0.0.0.0:9000)"))?;
    if !signal_project::device::valid_role(role) {
        return Err(format!("`{role}` is not a role: [a-z][a-z0-9_]*"));
    }
    address
        .parse::<std::net::SocketAddr>()
        .map_err(|_| format!("`{address}` is not IP:PORT"))?;
    Ok((role.to_owned(), address.to_owned()))
}

/// `--artnet`'s value: `role=IP[:PORT][/UNIVERSE]`.
pub(crate) fn parse_artnet_arg(
    text: &str,
) -> Result<(String, signal_output::ArtNetTarget), String> {
    let (role, target) = text
        .split_once('=')
        .ok_or_else(|| format!("`{text}` is not ROLE=IP[:PORT][/UNIVERSE] (lights=2.0.0.1)"))?;
    if !signal_project::device::valid_role(role) {
        return Err(format!("`{role}` is not a role: [a-z][a-z0-9_]*"));
    }
    Ok((role.to_owned(), signal_output::parse_artnet(target)?))
}

/// One `--route`: a role, the device it plays on (`None`: the output
/// device) and its channels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RouteArg {
    pub(crate) role: String,
    pub(crate) device: Option<String>,
    pub(crate) channels: Vec<u16>,
}

/// `--route`'s value: `role=3,4` or `role@Device name=1,2`.
pub(crate) fn parse_route(text: &str) -> Result<RouteArg, String> {
    let (target, channels) = text
        .rsplit_once('=')
        .ok_or_else(|| format!("`{text}` is not ROLE[@DEVICE]=CHANNELS (cue=3,4)"))?;
    let (role, device) = match target.split_once('@') {
        Some((role, device)) if !device.trim().is_empty() => (role, Some(device.trim().to_owned())),
        Some(_) => return Err(format!("`{text}`: no device after `@`")),
        None => (target, None),
    };
    if !signal_project::device::valid_role(role) {
        return Err(format!("`{role}` is not a role: [a-z][a-z0-9_]*"));
    }
    let channels = channels
        .split(',')
        .map(|c| match c.trim().parse::<u16>() {
            Ok(n) if n > 0 => Ok(n),
            _ => Err(format!("'{}' is not a device channel (1, 2, …)", c.trim())),
        })
        .collect::<Result<Vec<u16>, String>>()?;
    Ok(RouteArg {
        role: role.to_owned(),
        device,
        channels,
    })
}

/// `--rt-policy`'s value.
pub(crate) fn parse_policy(name: &str) -> Result<RtPolicy, String> {
    RtPolicy::from_name(name).ok_or_else(|| {
        let names: Vec<&str> = RtPolicy::ALL.iter().map(|p| p.name()).collect();
        format!("`{name}` is not one of {}", names.join(", "))
    })
}

/// `--rt-computation`'s value.
pub(crate) fn parse_computation(text: &str) -> Result<Computation, String> {
    text.parse::<f64>()
        .ok()
        .and_then(Computation::from_fraction)
        .ok_or_else(|| format!("`{text}` is not a fraction of the period above 0 and at most 1"))
}

/// What the run left behind, for the statistics.
struct Ended {
    status: RunnerStatus,
    /// The audio adapter's counts (ADR 0022): samples clipped, meters
    /// dropped.
    clipped: u64,
    meter_overflows: u64,
    stats: StatsSnapshot,
    /// The device channels played (the sink's width with the null driver).
    device_channels: usize,
    /// How the worker threads were scheduled.
    scheduling: Scheduling,
    audio: Option<Vec<f32>>,
    /// What idle workers blocked on (`ScheduleStats::waiter`).
    waiter: &'static str,
    /// The wake-up trace, with `--rt-trace` and workers.
    wake: Option<WakeStats>,
    /// Sampled demotions of the real-time workers (CoreAudio only).
    demotions: Demotions,
    /// Why the audio system stopped the stream (its rate changed, it went
    /// away), if it did: the command fails with it.
    fault: Option<String>,
    /// How the device was joined (ADR 0036; CoreAudio only): whether it ran
    /// for another app, and the frames per callback it ran at.
    joined: Option<JoinPlan>,
    /// The secondary devices as the run ended (ADR 0052; CoreAudio only).
    secondaries: Vec<SecondaryStatus>,
    /// The input devices as the run ended (ADR 0038 §8; CoreAudio only).
    captures: Vec<CaptureStatus>,
}

/// A distribution in µs, for the JSON.
fn spread_json(spread: &Spread) -> serde_json::Value {
    let us = |ns: u64| (ns as f64 / 100.0).round() / 10.0;
    json!({
        "count": spread.count,
        "median_us": us(spread.median_ns),
        "p99_us": us(spread.p99_ns),
        "max_us": us(spread.max_ns),
    })
}

/// The wake-up trace for the JSON: flat keys first (what
/// `scripts/rt-matrix.sh` reads), then the per-worker latencies.
pub(crate) fn wake_json(wake: &WakeStats) -> serde_json::Value {
    let us = |ns: u64| (ns as f64 / 100.0).round() / 10.0;
    let ms = |ns: u64| (ns as f64 / 1e5).round() / 10.0;
    json!({
        "callbacks": wake.callbacks,
        "callbacks_without_join": wake.callbacks_without_join,
        "wakes_signalled": wake.wakes_signalled,
        "wakes_observed": wake.wakes_observed,
        "wake_median_us": us(wake.wake.median_ns),
        "wake_p99_us": us(wake.wake.p99_ns),
        "wake_max_us": us(wake.wake.max_ns),
        "first_join_median_us": us(wake.first_join.median_ns),
        "first_join_p99_us": us(wake.first_join.p99_ns),
        "last_join_median_us": us(wake.last_join.median_ns),
        "last_join_p99_us": us(wake.last_join.p99_ns),
        "last_join_max_us": us(wake.last_join.max_ns),
        "slowest_task_median_us": us(wake.slowest_task.median_ns),
        "slowest_task_p99_us": us(wake.slowest_task.p99_ns),
        "per_worker": wake.per_worker.iter().map(spread_json).collect::<Vec<_>>(),
        "worker_budget": wake.worker_budget,
        "worker_task_ms": ms(wake.worker_task_ns),
        "job_ms": wake.job_ns.iter().copied().map(ms).collect::<Vec<_>>(),
        "spin_ms": wake.spin_ns.iter().copied().map(ms).collect::<Vec<_>>(),
        "parks": wake.parks,
    })
}

/// The watch's counts.
#[derive(Default)]
struct Edits {
    sets: usize,
    swaps: usize,
    errors: usize,
}

/// The control thread's state between callbacks: the Controller, the
/// recorder, and the file watch.
/// The live recorder's poll, and a hung callback (M2.7-9) on stderr: it is
/// a diagnosis, not part of the recording.
struct Recording<'a>(&'a mut LiveRecorder);

impl ControlSink for Recording<'_> {
    fn report(&mut self, report: &Report, command: &SentCommand) {
        self.0.report(report, command);
    }
    fn status(&mut self, status: &RunnerStatus) {
        self.0.status(status);
    }
    fn boundary(&mut self, line: BoundaryLine<'_>) {
        self.0.boundary(line);
    }
    fn hang(&mut self, line: HangLine<'_>) {
        let running: Vec<String> = line.running.iter().map(ToString::to_string).collect();
        eprintln!(
            "signal: hang: callback {} has not returned after {:.1} ms; still running: {}",
            line.callback,
            line.waited.as_secs_f64() * 1e3,
            if running.is_empty() {
                "no primitive call".to_owned()
            } else {
                running.join(", ")
            }
        );
    }
    fn frontier(&mut self, next_frame: Frame) {
        self.0.frontier(next_frame);
    }
}

struct Control<'a> {
    controller: Controller,
    live: LiveRecorder,
    registry: &'a Registry,
    /// The GPU devices, for the placement's warnings after a watched edit.
    gpu: &'a Gpu,
    dir: &'a Path,
    watch: Option<(ProjectWatcher, Instant)>,
    current: Project,
    /// The rate the run plays at (ADR 0036: the device's with CoreAudio).
    rate: u32,
    edits: Edits,
    /// ADR 0057 §2: the data sinks' frames and their Art-Net sender.
    artnet: Option<(signal_output::DataFrames, signal_output::ArtNetSender)>,
    /// ADR 0057 §1: the OSC listeners (kept for the run) and the control
    /// sides' records, taken into the session when the run starts.
    /// ADR 0057 §3: with them, the MIDI feeds, kept for the run.
    osc: (
        Vec<signal_output::osc::OscListener>,
        Vec<signal_output::inputs::InputRecord>,
        Vec<signal_audio::midi_in::MidiFeed>,
    ),
}

impl Control<'_> {
    /// Drains the runner's reports, and applies file changes every 100 ms.
    fn step(&mut self) {
        self.controller.poll(&mut Recording(&mut self.live));
        if let Some((rx, sender)) = self.artnet.as_mut() {
            sender.drain(rx);
            if let Some(failure) = sender.take_failure() {
                eprintln!("signal: {failure}");
            }
        }
        let Some((watcher, last)) = self.watch.as_mut() else {
            return;
        };
        if last.elapsed() < Duration::from_millis(WATCH_INTERVAL_MS) {
            return;
        }
        *last = Instant::now();
        match watcher.poll() {
            Ok(true) => self.reload(),
            Ok(false) => {}
            Err(error) => eprintln!("signal: watch: {}: {error}", self.dir.display()),
        }
    }

    /// Loads the changed project and sends the edit; on any error prints it
    /// and keeps the last good version playing.
    fn reload(&mut self) {
        match reload(
            &mut self.controller,
            &mut self.live,
            &mut self.current,
            self.dir,
            self.registry,
        ) {
            Ok(applied) => {
                self.edits.sets += applied.sets;
                if applied.swap.is_some() {
                    self.edits.swaps += 1;
                }
                eprintln!("signal: watch: {applied}");
                for warning in self.gpu.placement_warnings() {
                    eprintln!("signal: {warning}");
                }
            }
            Err(ReloadError::Invalid(diagnostics)) => {
                self.edits.errors += 1;
                for diagnostic in diagnostics {
                    eprintln!("{diagnostic}");
                }
                eprintln!("signal: watch: keeping the last good version");
            }
            Err(ReloadError::Failed(message)) => {
                self.edits.errors += 1;
                eprintln!("signal: watch: {message}; keeping the last good version");
            }
        }
    }
}

/// The options that do not go together, or are out of range (`play` and
/// `ui`).
pub(crate) fn check(args: &PlayArgs) -> Result<(), Failure> {
    let null = args.driver == DriverKind::Null;
    if !null && (args.fast || args.record_audio.is_some()) {
        return Err(Failure::Usage(
            "--fast and --record-audio need --driver null".to_owned(),
        ));
    }
    if args.record_boundary_data && args.record_session.is_none() {
        return Err(Failure::Usage(
            "--record-boundary-data needs --record-session".to_owned(),
        ));
    }
    if let Some(seconds) = args.seconds
        && !(seconds.is_finite() && seconds > 0.0)
    {
        return Err(Failure::Usage(format!(
            "--seconds must be a positive number, not {seconds}"
        )));
    }
    if let Some(dir) = &args.record_session
        && fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
    {
        return Err(Failure::Usage(format!(
            "{} is not empty; a session is written into a new directory",
            dir.display()
        )));
    }
    Ok(())
}

pub(crate) fn run(
    args: &PlayArgs,
    registry: &Registry,
    setup: Option<&Path>,
    json: bool,
) -> Result<(), Failure> {
    check(args)?;
    let null = args.driver == DriverKind::Null;
    // A .signal file plays from a temporary folder, kept until the end.
    let opened = crate::files::project_arg(&args.project)?;
    let (dir, shown) = (opened.dir(), opened.shown());
    if args.watch {
        crate::files::temporary_note(&opened);
    }
    let project = Project::load(dir).map_err(|error| match error {
        LoadError::Invalid(diagnostics) => Failure::Invalid(located(shown, diagnostics)),
        error @ LoadError::Io { .. } => Failure::Io(error.to_string()),
    })?;
    let mut plan = project
        .plan(registry)
        .map_err(|diagnostics| Failure::Invalid(located(shown, diagnostics)))?
        .plan;
    plan.version = 0;
    // M2.7-2: GPU-backed instances on this machine's devices (also for
    // ones a watched edit adds); kept until the runner and the controller
    // are gone, so the pool is never dropped on the audio thread.
    let mut gpu = Gpu::new(
        registry,
        uses_gpu(&plan) || args.watch,
        project.gpu_pins(&plan),
        setup,
    );
    for warning in gpu.warnings() {
        eprintln!("signal: {warning}");
    }
    let registry = gpu.registry();
    let (_, width) = sink_of(&project, &plan).map_err(|d| Failure::Invalid(located(shown, d)))?;
    // ADR 0038 (M4.1): every play side, routed with `--route` on the output
    // device; one sink without a route plays as before.
    let sides = signal_project::device::play_sides(&project, &plan);
    // ADR 0038 §5: a side of another layout than stereo is converted too.
    let routed = sides.len() > 1 || !args.route.is_empty() || sides.iter().any(|s| s.width != 2);
    let (width, sinks, routes, secondaries, mixes) = if routed {
        // Listing is read-only: the output device's channels, so that a
        // route past them is refused for that channel, not for the run, and
        // whether another device a route names is present.
        let list = if null {
            Vec::new()
        } else {
            signal_audio::devices().unwrap_or_default()
        };
        let outputs = || {
            list.iter()
                .filter(|d| d.direction == signal_audio::Direction::Output)
        };
        let clock = outputs().find(|d| match &args.device {
            Some(name) => &d.name == name,
            None => d.default,
        });
        let system = SoundSystem {
            roles: args
                .route
                .iter()
                .map(|r| {
                    let placement = match &r.device {
                        None => Placement::Clock,
                        Some(name) if clock.is_some_and(|c| &c.name == name) => Placement::Clock,
                        // The null driver plays the clock device's channels
                        // only: another device is not heard.
                        Some(name) if null => Placement::Missing(
                            name.clone(),
                            "the null driver plays no other device".to_owned(),
                        ),
                        Some(name) if outputs().any(|d| &d.name == name) => {
                            Placement::Other(name.clone())
                        }
                        Some(name) => {
                            Placement::Missing(name.clone(), "no output of that name".to_owned())
                        }
                    };
                    let binding = RoleBinding {
                        placement,
                        channels: r.channels.clone(),
                        ..RoleBinding::clock(Vec::new())
                    };
                    (r.role.clone(), binding)
                })
                .collect(),
        };
        let channels = clock.map(|d| {
            d.configs
                .iter()
                .map(|c| usize::from(c.channels))
                .max()
                .unwrap_or(0)
        });
        let routing = route(&sides, &system, channels);
        for notice in &routing.notices {
            eprintln!("signal: {notice}");
        }
        let secondaries = routing
            .secondaries()
            .into_iter()
            .map(|r| SecondarySpec {
                selector: DeviceSelector::Name(r.device.clone()),
                name: r.device,
                roles: r.roles,
                sources: r.sources,
                targets: r.targets,
            })
            .collect::<Vec<_>>();
        (
            routing.width(),
            routing.addresses(),
            Some(routing.targets()),
            secondaries,
            routing.mixes(),
        )
    } else {
        (width, Vec::new(), None, Vec::new(), Vec::new())
    };
    // ADR 0038 §7: every capture side, routed on the inputs `--capture`
    // names (the default input otherwise).
    let captures = signal_project::device::capture_sides(&project, &plan);
    let captures = if captures.is_empty() {
        Vec::new()
    } else if null {
        eprintln!(
            "signal: the null driver captures nothing; {} read 0",
            captures
                .iter()
                .map(|c| c.address.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        Vec::new()
    } else {
        let list = signal_audio::devices().unwrap_or_default();
        let default_input = list
            .iter()
            .find(|d| d.direction == signal_audio::Direction::Input && d.default)
            .map(|d| d.name.clone());
        let bindings = args
            .capture
            .iter()
            .map(|c| (c.role.clone(), (c.device.clone(), c.channels.clone())))
            .collect();
        let (routed, notices) =
            signal_output::route_captures(&captures, &bindings, default_input.as_deref());
        for notice in &notices {
            eprintln!("signal: {notice}");
        }
        let mut specs: Vec<CaptureSpec> = Vec::new();
        for side in routed {
            let signal_output::Captured::Device(name) = &side.from else {
                continue;
            };
            let spec_side = CaptureSideSpec {
                address: side.address.clone(),
                role: side.role.clone(),
                width: side.width,
                channels: side
                    .channels
                    .iter()
                    .map(|c| c.map(|c| usize::from(c) - 1))
                    .collect(),
            };
            match specs.iter_mut().find(|s| s.name == *name) {
                Some(spec) => spec.sides.push(spec_side),
                None => specs.push(CaptureSpec {
                    selector: DeviceSelector::Name(name.clone()),
                    name: name.clone(),
                    sides: vec![spec_side],
                }),
            }
        }
        specs
    };
    let seed = project.seed;
    let selector = args
        .device
        .clone()
        .map_or(DeviceSelector::Default, DeviceSelector::Name);
    // ADR 0036 (the owner's decision of 2026-10-23): the device's clock is
    // the run's. CoreAudio plays at the rate the output runs at, never
    // switching it; the project's `sample_rate` is its renders' rate.
    let rate = if null {
        project.sample_rate
    } else {
        match live_rate(&selector, project.sample_rate) {
            Ok((name, rate)) => {
                if let Some(notice) = rate_notice(&name, &project.name, project.sample_rate, rate) {
                    eprintln!("signal: {notice}");
                }
                rate
            }
            Err(_) => project.sample_rate,
        }
    };
    for note in project.sample_notes(rate) {
        eprintln!("signal: note: {note}");
    }
    let limit = args.seconds.map(|s| (s * f64::from(rate)).round() as u64);

    // ADR 0036: sized for the period the device runs at (its own while it
    // runs for another app).
    let period = if null {
        args.buffer
    } else {
        output_period(&selector, BufferRequest::Frames(args.buffer)).unwrap_or(args.buffer)
    };
    let mut config = RealtimeConfig::with_buffer(rate, seed, u64::from(period));
    config.engine.threads = args.threads as usize;
    if args.record_boundary_data {
        // About a second of delivered results per latent instance.
        let latent = plan
            .instances
            .iter()
            .filter(|i| i.descriptor.latency.is_some())
            .count();
        config.engine.record_boundary_data = true;
        config.boundary_data = (rate as usize).saturating_mul(latent.max(1));
    }
    config.rt = RtOptions {
        policy: args.rt_policy,
        computation: args.rt_computation,
        trace: args.rt_trace,
    };
    let (mut runner, controller) = realtime_routed(
        &plan,
        registry,
        config,
        Some(rt_thread::mach_semaphores),
        &sinks,
    )
    .map_err(|e| Failure::Internal(e.to_string()))?;
    // ADR 0038 §5: each side in its role's layout.
    runner.convert(mixes);
    // ADR 0057 §2: the `dmx` slots, sent as Art-Net to their roles' nodes.
    let mut data_sides = signal_project::device::data_sides(&project, &plan);
    data_sides.sort_by(|a, b| a.address.cmp(&b.address));
    let artnet = if data_sides.is_empty() {
        None
    } else {
        let targets: Vec<_> = data_sides
            .iter()
            .map(|side| {
                let target = args
                    .artnet
                    .iter()
                    .find(|(role, _)| *role == side.role)
                    .map(|(_, t)| *t);
                match target {
                    Some(t) => eprintln!(
                        "signal: {} ({}): Art-Net to {} universe {}",
                        side.role, side.address, t.address, t.universe
                    ),
                    None => eprintln!(
                        "signal: {} ({}): not sent (nothing binds it: --artnet {}=IP)",
                        side.role, side.address, side.role
                    ),
                }
                target
            })
            .collect();
        let sender = signal_output::ArtNetSender::new(targets)
            .map_err(|e| Failure::Io(format!("Art-Net socket: {e}")))?;
        Some((runner.send_data(signal_output::data_hop(rate)), sender))
    };
    // ADR 0057 §1: the `osc` slots, each from its role's listener; a
    // capture side like an audio one, recorded with the session.
    let record_frames = signal_ui::player::input_record_frames(rate, args.record_session.is_some());
    let mut osc_listeners = Vec::new();
    let mut osc_records = Vec::new();
    let mut osc_inputs = Vec::new();
    let mut midi_feeds = Vec::new();
    let sides = signal_project::device::control_sides(&project, &plan);
    // The `osc` sides bound, one listener per address: roles bound to the
    // same address share its socket.
    let bound: Vec<(usize, (String, signal_output::osc::OscRoute))> = sides
        .iter()
        .enumerate()
        .filter(|(_, side)| side.kind != signal_project::device::MIDI)
        .filter_map(|(index, side)| {
            let (_, address) = args.osc.iter().find(|(role, _)| *role == side.role)?;
            let route = signal_output::osc::OscRoute {
                role: side.role.clone(),
                names: side.ports.clone(),
            };
            Some((index, (address.clone(), route)))
        })
        .collect();
    let bindings: Vec<_> = bound.iter().map(|(_, b)| b.clone()).collect();
    let (listeners, opened) = signal_output::osc::listen_each(&bindings);
    osc_listeners.extend(listeners);
    let mut opened: std::collections::BTreeMap<usize, signal_output::osc::Opened> =
        bound.iter().map(|(index, _)| *index).zip(opened).collect();
    for (index, side) in sides.into_iter().enumerate() {
        // ADR 0057 §3: a `midi` slot, from the CoreMIDI sources named.
        if side.kind == signal_project::device::MIDI {
            let filter = args
                .midi
                .iter()
                .filter_map(|m| m.split_once('='))
                .find(|(role, _)| *role == side.role)
                .map(|(_, name)| name.to_owned());
            let Some(filter) = filter else {
                eprintln!(
                    "signal: {} ({}): reads 0 (nothing binds it: --midi {}=NAME)",
                    side.role, side.address, side.role
                );
                continue;
            };
            let (feed, source) = signal_audio::midi_in::MidiFeed::start(&filter, &side.ports)
                .map_err(|e| Failure::Io(format!("MIDI for {}: {e}", side.role)))?;
            eprintln!(
                "signal: {} ({}): MIDI from {}",
                side.role,
                side.address,
                if feed.sources.is_empty() {
                    format!("no source named `{filter}` yet")
                } else {
                    feed.sources.join(", ")
                }
            );
            let (input, record) = signal_output::inputs::RunInput::new(
                side.address.clone(),
                side.ports.len(),
                Box::new(source),
                (args.buffer as usize).max(signal_output::MAX_CALLBACK_FRAMES),
                record_frames,
            );
            osc_inputs.push(input);
            osc_records.extend(record);
            midi_feeds.push(feed);
            continue;
        }
        let Some((_, address)) = args.osc.iter().find(|(role, _)| *role == side.role) else {
            eprintln!(
                "signal: {} ({}): reads 0 (nothing binds it: --osc {}=IP:PORT)",
                side.role, side.address, side.role
            );
            continue;
        };
        let (source, local) = match opened.remove(&index) {
            Some(Ok(opened)) => opened,
            Some(Err(e)) => return Err(Failure::Io(format!("OSC on {address}: {e}"))),
            None => continue,
        };
        eprintln!(
            "signal: {} ({}): OSC on {local}, /{}/{{{}}}",
            side.role,
            side.address,
            side.role,
            side.ports.join(",")
        );
        let (input, record) = signal_output::inputs::RunInput::new(
            side.address.clone(),
            side.ports.len(),
            Box::new(source),
            (args.buffer as usize).max(signal_output::MAX_CALLBACK_FRAMES),
            record_frames,
        );
        osc_inputs.push(input);
        osc_records.extend(record);
    }
    runner.set_inputs(osc_inputs);
    if let Some(error) = runner.waiter_error() {
        eprintln!("signal: warning: no semaphores ({error}); the semaphore workers park");
    }
    // The placement is made while the plan is prepared (auditor-1-006).
    for warning in gpu.placement_warnings() {
        eprintln!("signal: {warning}");
    }
    let provenance = provenance::build(&project.files);
    let recorder = SessionRecorder::new(&project, registry, rate, seed, provenance)
        .map_err(|e| Failure::Internal(e.to_string()))?;
    let watch = if args.watch {
        let watcher = ProjectWatcher::new(dir).map_err(|e| Failure::Io(e.to_string()))?;
        Some((watcher, Instant::now()))
    } else {
        None
    };
    let mut control = Control {
        controller,
        live: match args.record_boundary_data {
            true => LiveRecorder::new(recorder, registry.clone()).with_boundary_data(),
            false => LiveRecorder::new(recorder, registry.clone()),
        },
        registry,
        gpu: &gpu,
        dir,
        watch,
        current: project,
        rate,
        edits: Edits::default(),
        artnet,
        osc: (osc_listeners, osc_records, midi_feeds),
    };

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        let until_eof = args.seconds.is_none();
        // Enter stops the run; so does a closed stdin when nothing else would.
        thread::spawn(move || {
            let mut line = String::new();
            match std::io::stdin().lock().read_line(&mut line) {
                Ok(0) | Err(_) if !until_eof => {}
                _ => stop.store(true, Ordering::Relaxed),
            }
        });
    }

    let ended = if null {
        play_null(args, &mut control, runner, width, limit, &stop)?
    } else {
        play_device(
            args,
            &mut control,
            runner,
            width,
            (routes, secondaries, captures),
            limit,
            &stop,
        )?
    };

    let mut live = control.live;
    // The stopped runner's own status: its last one may not have reached
    // the control thread, and the boundary records are checked against it.
    live.status(&ended.status);
    if let Err(error) = live.finish(ended.status.next_frame) {
        eprintln!("signal: the session's end was not recorded: {error}");
    }
    let boundaries = live.recorder().boundaries().cloned();
    if let Some(reason) = boundaries.as_ref().and_then(|b| b.reason.as_ref()) {
        eprintln!(
            "signal: warning: the session's boundary records are incomplete ({reason}); it replays only with --ideal"
        );
    }
    let refused = live.refused().to_vec();
    for message in &refused {
        eprintln!("signal: not recorded: {message}");
    }
    if let Some(dir) = &args.record_session {
        let mut recorder = live.into_recorder();
        // The GPU devices the run's latent modules could use (M2.7-8, M2.7-2).
        recorder.set_devices(gpu.devices());
        recorder.write(dir).map_err(|error| match error {
            SessionError::Io { .. } => Failure::Io(error.to_string()),
            error => Failure::Internal(error.to_string()),
        })?;
    }
    if let (Some(path), Some(samples)) = (&args.record_audio, &ended.audio) {
        write_wav(path, samples, ended.device_channels, rate)?;
    }

    if let Some(fault) = ended.fault {
        return Err(Failure::Device(fault));
    }
    let status = ended.status;
    if let Some(failure) = status.failure {
        return Err(Failure::Internal(failure.to_string()));
    }
    let commands = control.controller.command_overflows();
    let overflows = [
        commands,
        status.log_overflows,
        ended.meter_overflows,
        status.status_overflows,
        status.engine_log_overflows,
        status.input_overflows,
    ];
    let total: u64 = overflows.iter().sum();
    let stats = ended.stats;
    let driver = if null { "null" } else { "coreaudio" };
    let scheduling = &ended.scheduling;
    if json {
        let output = json!({
            "project": dir.display().to_string(),
            "driver": driver,
            "frames": status.next_frame.0,
            "sample_rate": rate,
            "sink_channels": width,
            "device_channels": ended.device_channels,
            "buffer": args.buffer,
            "device": ended.joined.map(|j| json!({
                "shared": j.shared,
                "period": j.period,
                "set_buffer": j.set_buffer,
            })),
            "threads": scheduling.threads,
            "workers": scheduling.workers.len(),
            "scheduling": {
                "path": scheduling.path(),
                "summary": scheduling.summary(),
                "workers": scheduling.workers.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "no_workgroup": scheduling.no_workgroup,
            },
            "callbacks": stats.callbacks,
            "xruns": {
                "total": stats.xruns,
                "stream_errors": stats.stream_errors,
                "overruns": stats.overruns,
            },
            "worst_callback_ratio": stats.worst_callback_ratio,
            "p99_callback_ratio": stats.p99_callback_ratio,
            "median_callback_ratio": stats.median_callback_ratio,
            "rt_policy": args.rt_policy.name(),
            "rt_computation": args.rt_computation.fraction(),
            "waiter": ended.waiter,
            "wake": ended.wake.as_ref().map(wake_json),
            "rt_demotions": {
                "exposed": false,
                "samples": ended.demotions.samples,
                "sampled_timeshare": ended.demotions.sampled_timeshare,
                "unsampled": ended.demotions.unsampled,
            },
            "clipped": ended.clipped,
            "hangs": control.controller.hangs(),
            "late": status.late,
            "late_results": status.late_results,
            "queue_overflows": {
                "total": total,
                "commands": commands,
                "log": status.log_overflows,
                "meters": ended.meter_overflows,
                "status": status.status_overflows,
                "engine_log": status.engine_log_overflows,
                "input": status.input_overflows,
            },
            "edits": {
                "sets": control.edits.sets,
                "swaps": control.edits.swaps,
                "errors": control.edits.errors,
            },
            "refused": refused,
            "session": args.record_session.as_ref().map(|d| d.display().to_string()),
            "boundaries": boundaries.as_ref().map(|b| json!({
                "complete": b.complete,
                "data": b.data,
                "delivered": b.delivered,
                "missed": b.missed,
                "reason": b.reason,
            })),
            "audio": args.record_audio.as_ref().map(|p| p.display().to_string()),
            "secondaries": ended.secondaries,
            "captures": ended.captures,
        });
        print_stdout(&serde_json::to_string_pretty(&output).unwrap_or_default());
    } else {
        let wake = ended.wake.as_ref().map_or(String::new(), |w| {
            let us = |ns: u64| ns as f64 / 1e3;
            format!(
                "\nwake latency {:.1} / {:.1} / {:.1} µs (median / p99 / max; {} of {} wakes seen)\n\
                 last worker joined {:.1} / {:.1} µs after the callback's start; slowest task p99 {:.1} µs",
                us(w.wake.median_ns),
                us(w.wake.p99_ns),
                us(w.wake.max_ns),
                w.wakes_observed,
                w.wakes_signalled,
                us(w.last_join.median_ns),
                us(w.last_join.p99_ns),
                us(w.slowest_task.p99_ns)
            )
        });
        print_stdout(&format!(
            "played {} frames at {rate} Hz ({width} sink channels on {} device channels) with the {driver} driver\n\
             threads {} ({} workers), scheduling {}\n\
             wake policy {} (computation {} of the period, workers block on {}){wake}\n\
             xruns {} ({} stream errors, {} overruns)\n\
             worst callback {:.3} of its period (p99 {:.2}, median {:.2})\n\
             clipped samples {}\n\
             queue overflows {total}\n\
             late commands {}\n\
             late GPU results {}{}",
            status.next_frame.0,
            ended.device_channels,
            scheduling.threads,
            scheduling.workers.len(),
            scheduling.summary(),
            args.rt_policy,
            args.rt_computation.fraction(),
            ended.waiter,
            stats.xruns,
            stats.stream_errors,
            stats.overruns,
            stats.worst_callback_ratio,
            stats.p99_callback_ratio,
            stats.median_callback_ratio,
            ended.clipped,
            status.late,
            status.late_results,
            ended
                .secondaries
                .iter()
                .map(|s| format!("\n{}", secondary_line(s)))
                .chain(
                    ended
                        .captures
                        .iter()
                        .map(|c| format!("\n{}", capture_line(c)))
                )
                .collect::<String>()
        ));
    }
    Ok(())
}

/// The null driver on this thread, in chunks of `--buffer` frames, with the
/// control step between chunks; paced to real time unless `--fast`.
fn play_null(
    args: &PlayArgs,
    control: &mut Control<'_>,
    runner: Runner,
    width: usize,
    limit: Option<u64>,
    stop: &AtomicBool,
) -> Result<Ended, Failure> {
    let rate = control.rate;
    let workers = runner.workers();
    let scheduling = Scheduling::ordinary(workers.threads(), workers.count(), "null driver");
    let mut driver = NullDriver::new(rate, width, control.current.seed);
    driver
        .start(runner)
        .map_err(|e| Failure::Internal(e.to_string()))?;
    // ADR 0038 §9: the null driver captures nothing; a recorded session
    // says its sources read 0.
    control.live.set_inputs(
        std::mem::take(&mut control.osc.1)
            .into_iter()
            .map(|record| (record, None))
            .collect(),
    );
    eprintln!(
        "signal: playing {} at {rate} Hz with the null driver; press Enter to stop",
        args.project.display()
    );
    report_scheduling(&scheduling);
    let stats = CallbackStats::default();
    let mut audio = args.record_audio.is_some().then(Vec::new);
    let began = Cell::new(Instant::now());
    let start = Instant::now();
    let mut played = 0u64;
    let mut stopping: Option<Instant> = None;
    loop {
        let left = limit.map_or(u64::MAX, |limit| limit - played);
        // A failed engine plays only silence: stop (docs/semantics.md §14).
        if left == 0 || control.controller.status().failure.is_some() {
            break;
        }
        // Stopped: once every command sent has applied (or a second has
        // passed), so an edit just made is played and recorded.
        if stop.load(Ordering::Relaxed) {
            let since = *stopping.get_or_insert_with(Instant::now);
            if control.controller.in_flight() == 0 || since.elapsed() > Duration::from_secs(1) {
                break;
            }
        }
        let chunk = left.min(u64::from(args.buffer));
        driver
            .run_with(
                chunk,
                || began.set(Instant::now()),
                |out| {
                    // Timed between the callback's start and its output:
                    // exactly `Runner::process`. Statistics only.
                    stats.record(out.len() / width, began.get().elapsed(), rate);
                    if let Some(audio) = audio.as_mut() {
                        audio.extend_from_slice(out);
                    }
                },
            )
            .map_err(|e| Failure::Internal(e.to_string()))?;
        played += chunk;
        control.step();
        if !args.fast {
            let due = start + Duration::from_secs_f64(played as f64 / f64::from(rate));
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                thread::sleep(wait);
            }
        }
    }
    let runner = driver
        .stop()
        .map_err(|e| Failure::Internal(e.to_string()))?
        .ok_or_else(|| Failure::Internal("the null driver lost the runner".to_owned()))?;
    control.controller.poll(&mut Recording(&mut control.live));
    Ok(Ended {
        status: runner.status(),
        clipped: runner.clipped(),
        meter_overflows: runner.meter_overflows(),
        stats: stats.snapshot(),
        device_channels: width,
        scheduling,
        audio,
        waiter: runner.schedule_stats().waiter,
        wake: runner.wake_stats(),
        demotions: Demotions::default(),
        fault: None,
        joined: None,
        secondaries: Vec::new(),
        captures: Vec::new(),
    })
}

/// How often a CoreAudio run polls its secondary devices (ADR 0052 §6).
const SECONDARY_POLL: Duration = Duration::from_millis(500);

/// A secondary device's status line (ADR 0052 §6): `Speakers (cue):
/// adapted, locked, +37.2 ppm, 7.3 ms, 0 slips`, the rates when they differ,
/// why when it is not playing.
fn secondary_line(s: &SecondaryStatus) -> String {
    let mut line = format!(
        "{} ({}): {}, {}",
        s.name,
        s.roles.join(", "),
        s.clock,
        s.state.name()
    );
    if let (Some(rate), Some(clock)) = (s.rate, s.clock_rate) {
        line.push_str(&format!(
            ", {} → {} kHz",
            f64::from(clock) / 1e3,
            f64::from(rate) / 1e3
        ));
    }
    if s.state != SecondaryState::Absent {
        line.push_str(&format!(", {:+.1} ppm", s.drift_ppm));
        if let Some(ms) = s.latency_ms {
            line.push_str(&format!(", {ms:.1} ms"));
        }
        line.push_str(&format!(
            ", {} slip{}",
            s.slips,
            if s.slips == 1 { "" } else { "s" }
        ));
    }
    if s.resyncs > 0 {
        line.push_str(&format!(
            ", {} resync{}",
            s.resyncs,
            if s.resyncs == 1 { "" } else { "s" }
        ));
    }
    if s.rebuilds > 0 {
        line.push_str(&format!(", rebuilt {}×", s.rebuilds));
    }
    if s.held > 0 {
        line.push_str(&format!(", {} held", s.held));
    }
    if let Some(reason) = &s.reason {
        line.push_str(&format!(" ({reason})"));
    }
    line
}

/// An input device's status line (ADR 0038 §10): `AT2020USB+ (voice):
/// capture, adapted, locked, +12.0 ppm, peak 0.31, 0 missing`.
fn capture_line(c: &CaptureStatus) -> String {
    let mut line = format!(
        "{} ({}): capture, {}, {}",
        c.name,
        c.roles.join(", "),
        c.clock,
        c.state.name()
    );
    if c.state != SecondaryState::Absent {
        if c.clock == "adapted" {
            line.push_str(&format!(", {:+.1} ppm", c.drift_ppm));
        }
        line.push_str(&format!(
            ", peak {:.3}, {} missing, {} dropped",
            c.peak, c.missing, c.dropped
        ));
    }
    if let Some(reason) = &c.reason {
        line.push_str(&format!(" ({reason})"));
    }
    line
}

/// CoreAudio: the device calls the Runner on its own thread; this thread
/// polls at least once per buffer period (so `Stamp::Default`, two buffers
/// ahead, is not yet run) and applies file changes.
fn play_device(
    args: &PlayArgs,
    control: &mut Control<'_>,
    runner: Runner,
    width: usize,
    (routes, secondaries, captures): (
        Option<Vec<Option<usize>>>,
        Vec<SecondarySpec>,
        Vec<CaptureSpec>,
    ),
    limit: Option<u64>,
    stop: &AtomicBool,
) -> Result<Ended, Failure> {
    let rate = control.rate;
    let device = |e: &dyn std::fmt::Display| Failure::Device(e.to_string());
    let mut driver = match routes {
        None => CoreAudioDriver::open(args.device.as_deref(), rate, args.buffer, width),
        Some(routes) => {
            let selector = args
                .device
                .clone()
                .map_or(DeviceSelector::Default, DeviceSelector::Name);
            CoreAudioDriver::open_routed(
                &selector,
                rate,
                BufferRequest::Frames(args.buffer),
                routes,
            )
        }
    }
    .map_err(|e| device(&e))?
    .with_secondaries(secondaries)
    .with_captures(
        captures.clone(),
        signal_ui::player::input_record_frames(rate, args.record_session.is_some()),
    );
    let map = driver.map();
    if !map.dropped().is_empty() {
        eprintln!(
            "signal: warning: the device has {} channels; sink channels {}..={} are not played",
            map.device_channels(),
            map.dropped().start + 1,
            map.dropped().end
        );
    }
    driver.start(runner).map_err(|e| device(&e))?;
    // ADR 0038 §9: a recorded session records what the sources read.
    let mut records = signal_ui::player::input_records(driver.take_input_records(), &captures);
    records.extend(
        std::mem::take(&mut control.osc.1)
            .into_iter()
            .map(|r| (r, None)),
    );
    control.live.set_inputs(records);
    let shared = driver.joined().is_some_and(|j| j.shared);
    eprintln!(
        "signal: playing {} at {rate} Hz on '{}' (buffer {} frames{}); press Enter to stop",
        args.project.display(),
        driver.name(),
        driver.period(),
        if shared {
            ", the device's own: it runs for another app"
        } else {
            ""
        }
    );
    let scheduling = driver
        .scheduling()
        .cloned()
        .ok_or_else(|| Failure::Internal("the driver reported no scheduling".to_owned()))?;
    report_scheduling(&scheduling);
    for secondary in driver.secondaries() {
        eprintln!("signal: {}", secondary_line(&secondary));
    }
    for capture in driver.captures() {
        eprintln!("signal: {}", capture_line(&capture));
    }
    let interval = Duration::from_secs_f64(f64::from(driver.period()) / f64::from(rate))
        .min(Duration::from_millis(1));
    let mut polled = Instant::now();
    let mut stopping: Option<Instant> = None;
    let mut demotions = Demotions::default();
    let mut fault = None;
    loop {
        thread::sleep(interval);
        control.step();
        // C6: macOS counts no demotions for a process; sample the policy.
        demotions.sample(&scheduling.ports);
        // ADR 0052 §6: a secondary device gone is stopped, one back is
        // rebuilt; an input device too (hot-plug).
        if polled.elapsed() >= SECONDARY_POLL {
            polled = Instant::now();
            let mut notices = driver.poll_secondaries();
            notices.extend(driver.poll_captures());
            for notice in notices {
                eprintln!("signal: {notice}");
            }
        }
        let status = control.controller.status();
        // A failed engine plays only silence: stop (docs/semantics.md §14).
        if limit.is_some_and(|limit| status.next_frame.0 >= limit) || status.failure.is_some() {
            break;
        }
        // The audio system stopped the stream (its rate changed, it went
        // away): it calls no more back.
        if let Some(stopped) = driver.fault() {
            fault = Some(stopped);
            break;
        }
        // Stopped: once every command sent has applied (or a second has
        // passed), so an edit just made is played and recorded.
        if stop.load(Ordering::Relaxed) {
            let since = *stopping.get_or_insert_with(Instant::now);
            if control.controller.in_flight() == 0 || since.elapsed() > Duration::from_secs(1) {
                break;
            }
        }
    }
    let stats = driver.stats();
    // As they played, before the stop takes their streams down.
    let secondaries_at_stop = driver.secondaries();
    let captures_at_stop = driver.captures();
    // The adapter's counts as of the last callback, should the device not
    // hand the runner back.
    let adapter_counts = (
        control.controller.clipped(),
        control.controller.meter_overflows(),
    );
    let (status, waiter, wake, counts) = match driver.stop() {
        Ok(Some(runner)) => (
            runner.status(),
            runner.schedule_stats().waiter,
            runner.wake_stats(),
            (runner.clipped(), runner.meter_overflows()),
        ),
        Ok(None) => (control.controller.status(), "unknown", None, adapter_counts),
        Err(error) => {
            // A stream the audio system stopped hands no Runner back: the
            // fault says why.
            if fault.is_none() {
                eprintln!("signal: warning: {error}");
            }
            (control.controller.status(), "unknown", None, adapter_counts)
        }
    };
    control.controller.poll(&mut Recording(&mut control.live));
    Ok(Ended {
        secondaries: secondaries_at_stop,
        captures: captures_at_stop,
        status,
        clipped: counts.0,
        meter_overflows: counts.1,
        stats: stats.snapshot(),
        device_channels: driver.channels(),
        scheduling,
        audio: None,
        waiter,
        wake,
        demotions,
        fault,
        joined: driver.joined(),
    })
}

/// `signal: 8 threads (7 workers): workgroup` on stderr.
fn report_scheduling(scheduling: &Scheduling) {
    eprintln!(
        "signal: {} threads ({} workers): {}",
        scheduling.threads,
        scheduling.workers.len(),
        scheduling.summary()
    );
}

/// Writes interleaved samples as `signal render` writes `audio.wav`: 32-bit
/// float.
pub(crate) fn write_wav(
    path: &Path,
    samples: &[f32],
    channels: usize,
    rate: u32,
) -> Result<(), Failure> {
    let io = |e: hound::Error| Failure::Io(format!("{}: {e}", path.display()));
    let spec = hound::WavSpec {
        channels: u16::try_from(channels).map_err(|e| Failure::Internal(e.to_string()))?,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut wav = hound::WavWriter::create(path, spec).map_err(io)?;
    for &sample in samples {
        wav.write_sample(sample).map_err(io)?;
    }
    wav.finalize().map_err(io)
}
