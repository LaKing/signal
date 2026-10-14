//! Signal.app's rules (M2.5), shared by `signal-app`'s `AppState` and the
//! Host's app mode (M2.8-4), so the app's window and its tests run the same
//! code: the project's load, how a run plays from the Setup, the status
//! line, the meter labels, and what opening a project and exiting record in
//! the Setup. Control-thread code.

use std::path::{Path, PathBuf};

use signal_audio::{DeviceInfo, Scheduling, rt_thread};
use signal_core::Registry;
use signal_engine::{Provenance, RealtimeConfig};
use signal_graph::ExecutionPlan;
use signal_output::{Controller, realtime_with};
use signal_project::session::{SessionRecorder, sink_of};
use signal_project::{LoadError, Project, located};

use crate::player::{Backend, Input, Output, open_input, open_output};
use crate::setup::{Setup, Window};

/// Why the devices are not listed again while playing.
pub const REFRESH_WHILE_PLAYING: &str =
    "stop playback to refresh: listing devices pauses metering and status";

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

impl Status {
    /// The status line as the window shows it.
    pub fn line(&self) -> String {
        format!(
            "frame {}   elapsed {:.3} s   xruns {}   load {:.1} % avg, {:.1} % worst   clipped {}   buffer {}   threads {} ({} workers, {})",
            self.frame,
            self.elapsed,
            self.xruns,
            self.load_average * 100.0,
            self.load_worst * 100.0,
            self.clipped,
            self.buffer,
            self.threads,
            self.workers,
            self.scheduling
        )
    }
}

/// The status line of a run: `stopped` without one.
pub fn status_line(status: Option<&Status>) -> String {
    status.map_or_else(|| "stopped".to_owned(), Status::line)
}

/// The status of a run: its Controller, output and input.
pub fn status(
    controller: &Controller,
    output: &Output,
    input: Option<&Input>,
    rate: u32,
    buffer: u32,
    scheduling: &Scheduling,
) -> Status {
    let status = controller.status();
    let stats = output.stats();
    let input_errors = input.map_or(0, Input::stream_errors);
    Status {
        frame: status.next_frame.0,
        elapsed: status.next_frame.0 as f64 / f64::from(rate.max(1)),
        xruns: stats.xruns + input_errors,
        load_average: stats.average_callback_ratio,
        load_worst: stats.worst_callback_ratio,
        clipped: controller.clipped(),
        buffer,
        threads: scheduling.threads,
        workers: scheduling.workers.len(),
        scheduling: scheduling.path(),
    }
}

/// The project at `dir`, its plan and its sink's width; or its diagnostics,
/// located in the folder.
pub fn load(
    dir: &Path,
    registry: &Registry,
) -> Result<(Project, ExecutionPlan, usize), Vec<String>> {
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

fn strings(diagnostics: Vec<signal_project::Diagnostic>) -> Vec<String> {
    diagnostics.iter().map(ToString::to_string).collect()
}

/// How a run plays with the Setup: its buffer, CPU workers and the advanced
/// keys (M2.7-1) for the workers' wake policy and computation.
pub fn realtime_config(setup: &Setup, rate: u32, seed: u64) -> RealtimeConfig {
    let mut config = RealtimeConfig::with_buffer(rate, seed, u64::from(setup.buffer));
    config.engine.threads = setup.cpu_workers.map_or(0, |n| n as usize);
    config.rt.policy = setup.rt_policy;
    config.rt.computation = setup.rt_computation;
    config
}

/// A run started by [`start`].
pub struct Started {
    /// The output, playing.
    pub output: Output,
    /// The run's Controller.
    pub controller: Controller,
    /// The session recorder from frame 0.
    pub recorder: SessionRecorder,
    /// The Setup's input, metered, and its channels.
    pub input: Option<(Input, usize)>,
    /// The project's sample rate.
    pub rate: u32,
    /// The buffer requested.
    pub buffer: u32,
    /// How the workers are scheduled.
    pub scheduling: Scheduling,
}

/// Plays `plan` (of `project`, `width` sink channels) from frame 0 with the
/// Setup: its output and channel map, buffer, CPU workers, wake policy, and
/// its input, metered. Notes on the run (the channel map, the input) go to
/// `notes`; the error is the refusal to show.
#[allow(clippy::too_many_arguments)]
pub fn start(
    backend: &Backend,
    devices: &[DeviceInfo],
    setup: &Setup,
    registry: &Registry,
    project: &Project,
    mut plan: ExecutionPlan,
    width: usize,
    notes: &mut Vec<String>,
) -> Result<Started, String> {
    let (rate, seed, buffer) = (project.sample_rate, project.seed, setup.buffer);
    let mut output = open_output(backend, devices, setup, rate, width, seed, notes)?;
    plan.version = 0;
    let config = realtime_config(setup, rate, seed);
    let (runner, controller) =
        realtime_with(&plan, registry, config, Some(rt_thread::mach_semaphores))
            .map_err(|error| error.to_string())?;
    let recorder = SessionRecorder::new(project, registry, rate, seed, Provenance::default())
        .map_err(|error| error.to_string())?;
    let workers = runner.workers();
    output.start(runner).map_err(|error| error.to_string())?;
    let scheduling = output
        .scheduling()
        .cloned()
        .unwrap_or_else(|| Scheduling::ordinary(workers.threads(), workers.count(), "null driver"));
    let input = open_input(backend, devices, setup, rate, notes);
    Ok(Started {
        output,
        controller,
        recorder,
        input,
        rate,
        buffer,
        scheduling,
    })
}

/// The label of the output meter of sink channel `sink`, played on device
/// channel `target`.
pub fn meter_label(sink: usize, target: Option<usize>) -> String {
    match target {
        Some(device) => format!("out {} → {}", sink + 1, device + 1),
        None => format!("out {} (not played)", sink + 1),
    }
}

/// The project folder to open at start: the one asked for, else the
/// Setup's last project.
pub fn initial_project(asked: Option<PathBuf>, setup: &Setup) -> Option<PathBuf> {
    asked.or_else(|| setup.last_project.clone())
}

/// A project folder was opened: it becomes the Setup's last project (saved
/// with the next change to the Setup, or by the caller).
pub fn opened(setup: &mut Setup, dir: PathBuf) {
    setup.last_project = Some(dir);
}

/// The app exits: the window's size, when known, goes into the Setup, which
/// the caller then saves.
pub fn exit(setup: &mut Setup, window: Option<Window>) {
    if window.is_some() {
        setup.window = window;
    }
}

/// Saves the Setup at `path`; the error is the message to show.
pub fn save(setup: &Setup, path: &Path) -> Result<(), String> {
    setup
        .save(path)
        .map_err(|error| format!("the Setup was not saved to {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use signal_engine::{Computation, RtPolicy};

    #[test]
    fn the_run_config_takes_the_setup_s_workers_and_wake_policy() {
        let setup = Setup {
            buffer: 256,
            cpu_workers: Some(3),
            rt_policy: RtPolicy::Semaphore,
            rt_computation: Computation::DEFAULT,
            ..Setup::default()
        };
        let config = realtime_config(&setup, 48_000, 7);
        assert_eq!(config.engine.threads, 3);
        assert_eq!(config.rt.policy, RtPolicy::Semaphore);
        assert_eq!(config.rt.computation, Computation::DEFAULT);
        let auto = realtime_config(&Setup::default(), 48_000, 7);
        assert_eq!(auto.engine.threads, 0, "auto: one per performance core");
        assert_eq!(auto.rt.policy, RtPolicy::default());
    }

    #[test]
    fn the_status_line_is_the_window_s() {
        let status = Status {
            frame: 48_000,
            elapsed: 0.5,
            xruns: 1,
            load_average: 0.125,
            load_worst: 0.5,
            clipped: 2,
            buffer: 128,
            threads: 4,
            workers: 3,
            scheduling: "workgroup",
        };
        assert_eq!(
            status_line(Some(&status)),
            "frame 48000   elapsed 0.500 s   xruns 1   load 12.5 % avg, 50.0 % worst   clipped 2   buffer 128   threads 4 (3 workers, workgroup)"
        );
        assert_eq!(status_line(None), "stopped");
    }

    #[test]
    fn meter_labels_name_the_device_channel() {
        assert_eq!(meter_label(0, Some(2)), "out 1 → 3");
        assert_eq!(meter_label(1, None), "out 2 (not played)");
    }

    #[test]
    fn opening_and_exiting_record_in_the_setup() {
        let mut setup = Setup::default();
        assert_eq!(initial_project(None, &setup), None);
        opened(&mut setup, PathBuf::from("/p/kick"));
        assert_eq!(
            initial_project(None, &setup),
            Some(PathBuf::from("/p/kick"))
        );
        assert_eq!(
            initial_project(Some(PathBuf::from("/q")), &setup),
            Some(PathBuf::from("/q"))
        );
        let window = Window {
            width: 900.0,
            height: 600.0,
        };
        exit(&mut setup, Some(window));
        exit(&mut setup, None);
        assert_eq!(setup.window, Some(window), "an unknown size keeps the last");
    }
}
