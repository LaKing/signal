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
use signal_audio::{CallbackStats, CoreAudioDriver, StatsSnapshot};
use signal_core::Registry;
use signal_engine::{
    Controller, Driver, NullDriver, RealtimeConfig, Runner, RunnerStatus, realtime,
};
use signal_project::live::{ProjectWatcher, ReloadError, WATCH_INTERVAL_MS, reload};
use signal_project::session::{LiveRecorder, SessionError, SessionRecorder, sink_of};
use signal_project::{LoadError, Project, provenance};

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
    /// The project folder (containing project.toml).
    project: PathBuf,
    /// The output device's name (see `signal devices`); the default output
    /// device without it.
    #[arg(long)]
    device: Option<String>,
    /// The device buffer to request, in frames.
    #[arg(long, default_value_t = 128, value_parser = clap::value_parser!(u32).range(1..))]
    buffer: u32,
    /// Play this many seconds (rounded to the nearest frame). Without it,
    /// play until Enter is pressed or stdin closes.
    #[arg(long)]
    seconds: Option<f64>,
    /// Poll the project folder every 100 ms and apply changes live; a
    /// project that does not load keeps the last good version playing.
    #[arg(long)]
    watch: bool,
    /// Write the session (docs/session-format.md) to this new directory at
    /// the end, for `signal replay`.
    #[arg(long)]
    record_session: Option<PathBuf>,
    /// The driver.
    #[arg(long, value_enum, default_value_t = DriverKind::Coreaudio)]
    driver: DriverKind,
    /// Null driver only: run as fast as possible instead of in real time.
    #[arg(long)]
    fast: bool,
    /// Null driver only: write the output to this WAV file at the end (kept
    /// in memory while playing).
    #[arg(long)]
    record_audio: Option<PathBuf>,
    /// Threads a block runs on, the audio thread included (0: one per
    /// performance core; 1: no worker threads). The output is identical for
    /// every count.
    #[arg(long, default_value_t = 0)]
    threads: u32,
}

/// What the run left behind, for the statistics.
struct Ended {
    status: RunnerStatus,
    stats: StatsSnapshot,
    /// The device channels played (the sink's width with the null driver).
    device_channels: usize,
    audio: Option<Vec<f32>>,
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
struct Control<'a> {
    controller: Controller,
    live: LiveRecorder,
    registry: &'a Registry,
    dir: &'a Path,
    watch: Option<(ProjectWatcher, Instant)>,
    current: Project,
    edits: Edits,
}

impl Control<'_> {
    /// Drains the runner's reports, and applies file changes every 100 ms.
    fn step(&mut self) {
        self.controller.poll(&mut self.live);
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

pub(crate) fn run(args: &PlayArgs, registry: &Registry, json: bool) -> Result<(), Failure> {
    let null = args.driver == DriverKind::Null;
    if !null && (args.fast || args.record_audio.is_some()) {
        return Err(Failure::Usage(
            "--fast and --record-audio need --driver null".to_owned(),
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

    let dir = &args.project;
    let project = Project::load(dir).map_err(|error| match error {
        LoadError::Invalid(diagnostics) => Failure::Invalid(located(dir, diagnostics)),
        error @ LoadError::Io { .. } => Failure::Io(error.to_string()),
    })?;
    let mut plan = project
        .plan(registry)
        .map_err(|diagnostics| Failure::Invalid(located(dir, diagnostics)))?
        .plan;
    plan.version = 0;
    let (_, width) = sink_of(&project, &plan).map_err(|d| Failure::Invalid(located(dir, d)))?;
    let (rate, seed) = (project.sample_rate, project.seed);
    let limit = args.seconds.map(|s| (s * f64::from(rate)).round() as u64);

    let mut config = RealtimeConfig::with_buffer(rate, seed, u64::from(args.buffer));
    config.engine.threads = args.threads as usize;
    let (runner, controller) =
        realtime(&plan, registry, config).map_err(|e| Failure::Internal(e.to_string()))?;
    let provenance = provenance::collect(&provenance::source_dir(), &project.files);
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
        live: LiveRecorder::new(recorder, registry.clone()),
        registry,
        dir,
        watch,
        current: project,
        edits: Edits::default(),
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
        play_device(args, &mut control, runner, width, limit, &stop)?
    };

    let mut live = control.live;
    if let Err(error) = live.finish(ended.status.next_frame) {
        eprintln!("signal: the session's end was not recorded: {error}");
    }
    let refused = live.refused().to_vec();
    for message in &refused {
        eprintln!("signal: not recorded: {message}");
    }
    if let Some(dir) = &args.record_session {
        live.into_recorder()
            .write(dir)
            .map_err(|error| match error {
                SessionError::Io { .. } => Failure::Io(error.to_string()),
                error => Failure::Internal(error.to_string()),
            })?;
    }
    if let (Some(path), Some(samples)) = (&args.record_audio, &ended.audio) {
        write_wav(path, samples, ended.device_channels, rate)?;
    }

    let status = ended.status;
    let commands = control.controller.command_overflows();
    let overflows = [
        commands,
        status.log_overflows,
        status.meter_overflows,
        status.status_overflows,
        status.engine_log_overflows,
        status.input_overflows,
    ];
    let total: u64 = overflows.iter().sum();
    let stats = ended.stats;
    let driver = if null { "null" } else { "coreaudio" };
    if json {
        let output = json!({
            "project": dir.display().to_string(),
            "driver": driver,
            "frames": status.next_frame.0,
            "sample_rate": rate,
            "sink_channels": width,
            "device_channels": ended.device_channels,
            "buffer": args.buffer,
            "callbacks": stats.callbacks,
            "xruns": {
                "total": stats.xruns,
                "stream_errors": stats.stream_errors,
                "overruns": stats.overruns,
            },
            "worst_callback_ratio": stats.worst_callback_ratio,
            "clipped": status.clipped,
            "late": status.late,
            "queue_overflows": {
                "total": total,
                "commands": commands,
                "log": status.log_overflows,
                "meters": status.meter_overflows,
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
            "audio": args.record_audio.as_ref().map(|p| p.display().to_string()),
        });
        print_stdout(&serde_json::to_string_pretty(&output).unwrap_or_default());
    } else {
        print_stdout(&format!(
            "played {} frames at {rate} Hz ({width} sink channels on {} device channels) with the {driver} driver\n\
             xruns {} ({} stream errors, {} overruns)\n\
             worst callback {:.3} of its period\n\
             clipped samples {}\n\
             queue overflows {total}\n\
             late commands {}",
            status.next_frame.0,
            ended.device_channels,
            stats.xruns,
            stats.stream_errors,
            stats.overruns,
            stats.worst_callback_ratio,
            status.clipped,
            status.late
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
    let rate = control.current.sample_rate;
    let mut driver = NullDriver::new(rate, width, control.current.seed);
    driver
        .start(runner)
        .map_err(|e| Failure::Internal(e.to_string()))?;
    eprintln!(
        "signal: playing {} at {rate} Hz with the null driver; press Enter to stop",
        args.project.display()
    );
    let stats = CallbackStats::default();
    let mut audio = args.record_audio.is_some().then(Vec::new);
    let began = Cell::new(Instant::now());
    let start = Instant::now();
    let mut played = 0u64;
    let mut stopping: Option<Instant> = None;
    loop {
        let left = limit.map_or(u64::MAX, |limit| limit - played);
        if left == 0 {
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
    control.controller.poll(&mut control.live);
    Ok(Ended {
        status: runner.status(),
        stats: stats.snapshot(),
        device_channels: width,
        audio,
    })
}

/// CoreAudio: the device calls the Runner on its own thread; this thread
/// polls at least once per buffer period (so `Stamp::Default`, two buffers
/// ahead, is not yet run) and applies file changes.
fn play_device(
    args: &PlayArgs,
    control: &mut Control<'_>,
    runner: Runner,
    width: usize,
    limit: Option<u64>,
    stop: &AtomicBool,
) -> Result<Ended, Failure> {
    let rate = control.current.sample_rate;
    let device = |e: &dyn std::fmt::Display| Failure::Device(e.to_string());
    let mut driver = CoreAudioDriver::open(args.device.as_deref(), rate, args.buffer, width)
        .map_err(|e| device(&e))?;
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
    eprintln!(
        "signal: playing {} at {rate} Hz on '{}' (buffer {} frames); press Enter to stop",
        args.project.display(),
        driver.name(),
        args.buffer
    );
    let interval = Duration::from_secs_f64(f64::from(args.buffer) / f64::from(rate))
        .min(Duration::from_millis(1));
    let mut stopping: Option<Instant> = None;
    loop {
        thread::sleep(interval);
        control.step();
        let next = control.controller.status().next_frame.0;
        if limit.is_some_and(|limit| next >= limit) {
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
    let status = match driver.stop() {
        Ok(Some(runner)) => runner.status(),
        Ok(None) => control.controller.status(),
        Err(error) => {
            eprintln!("signal: warning: {error}");
            control.controller.status()
        }
    };
    control.controller.poll(&mut control.live);
    Ok(Ended {
        status,
        stats: stats.snapshot(),
        device_channels: driver.channels(),
        audio: None,
    })
}

/// Writes interleaved samples as `signal render` writes `audio.wav`: 32-bit
/// float.
fn write_wav(path: &Path, samples: &[f32], channels: usize, rate: u32) -> Result<(), Failure> {
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
