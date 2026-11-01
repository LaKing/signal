//! The run: its build, install, poll and stop, the instances the fault guard
//! mutes, and the device wait.

use std::collections::BTreeMap;
use std::fs;
use std::time::{Duration, Instant};

use serde_json::json;
use signal_audio::{ChannelMap, CoreAudioDriver, Scheduling, rt_thread};
use signal_core::{Frame, PortAddress};
use signal_engine::{
    ControlSink, FaultReason, HangLine, LogLine, ParameterValue, RealtimeConfig, Report,
    SentCommand,
};
use signal_graph::ExecutionPlan;
use signal_output::{Controller, Meter, NullDriver, VectorUnbound, realtime_with};
use signal_project::live::{Both, ProjectWatcher};
use signal_project::session::{LiveRecorder, SessionRecorder, sink_of};
use signal_project::{LoadError, Project, located};

use crate::panel::Panel;
use crate::player::{Backend, Input, Output, live_rate, open_input, open_output, output_state};

use super::controls::plan_base;
use super::setup_keys::device_name;
use super::subs::Stream;
use super::{Host, joined, with};

/// The values, reports and mutes of one poll, beside the session recorder.
pub(super) struct Observed<'a> {
    pub(super) values: &'a mut BTreeMap<PortAddress, ParameterValue>,
    pub(super) reports: &'a mut Vec<Report>,
    pub(super) muted: &'a mut Vec<Muted>,
}

/// An instance the fault guard muted (`docs/semantics.md` §7, §14): it is
/// never called again in this run, so its outputs stay 0 whatever its
/// parameters are set to.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Muted {
    frame: Frame,
    pub(super) address: String,
    reason: FaultReason,
    /// The instance's module type when it was muted (from the plan then):
    /// a swap that removes the instance or gives the address another type
    /// brings a new, unfaulted one (`semantics.md` §14), and the entry goes.
    pub(super) module: Option<String>,
}

/// How many other muted instances the note names before "and N more".
const MUTED_LISTED: usize = 8;

impl Muted {
    /// What the operator is told: which instance, why, and how it comes back.
    fn message(&self) -> String {
        let (why, back) = match self.reason {
            FaultReason::NonFinite => (
                "an output or event value that is not finite",
                "with its values back in range, play the project again",
            ),
            FaultReason::Magnitude => (
                "an output or event value beyond ±10⁶",
                "with its values back in range, play the project again",
            ),
            FaultReason::EventOverflow => (
                "more than 16 events on an output in one frame",
                "with fewer events reaching it, play the project again",
            ),
            FaultReason::Panic => (
                "its call panicked",
                "its module broke the module contract; play the project again",
            ),
            FaultReason::LateResult => ("a late result", "play the project again"),
        };
        format!(
            "{} was muted at frame {} ({why}): it stays silent for the rest of this run, \
             whatever its parameters are set to; {back} (Project ▸ Restart from Files, or the \
             power off and on) to hear it",
            self.address, self.frame.0
        )
    }
}

/// Signal.app's note for the instances muted in a run: the first one's
/// message, the addresses of up to [`MUTED_LISTED`] others and how many
/// more, and, when the run dropped log records, that the list may be short
/// (a fault record may be among them). `None` when there is nothing to say.
pub(super) fn muted_message(muted: &[Muted], log_lost: bool) -> Option<String> {
    const LOST: &str =
        "log records were dropped, so an instance the fault guard muted may be missing here";
    let Some((first, rest)) = muted.split_first() else {
        return log_lost.then(|| format!("Some {LOST}"));
    };
    let mut text = first.message();
    if !rest.is_empty() {
        let named: Vec<&str> = rest
            .iter()
            .take(MUTED_LISTED)
            .map(|m| m.address.as_str())
            .collect();
        let more = rest.len().saturating_sub(MUTED_LISTED);
        let tail = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        text = format!("{text} (also muted: {}{tail})", named.join(", "));
    }
    if log_lost {
        text = format!("{text}; some {LOST}");
    }
    Some(text)
}

/// Takes the mutes `pending` (from a poll, or a reload's poll) into `run`,
/// each said once in `messages`, with the instance's module type from
/// `plan`; the note is made again when anything was taken.
pub(super) fn take_muted(
    pending: &mut Vec<Muted>,
    run: &mut Run,
    plan: Option<&ExecutionPlan>,
    messages: &mut Vec<String>,
) {
    if pending.is_empty() {
        return;
    }
    for mut muted in pending.drain(..) {
        muted.module = plan.and_then(|plan| module_at(plan, &muted.address));
        messages.push(muted.message());
        run.muted.push(muted);
    }
    run.muted_note = muted_message(&run.muted, run.log_lost);
}

/// The module type of the instance at `address` in `plan`.
pub(super) fn module_at(plan: &ExecutionPlan, address: &str) -> Option<String> {
    plan.instances
        .iter()
        .find(|i| i.address.as_str() == address)
        .map(|i| i.descriptor.type_name.to_owned())
}

impl ControlSink for Observed<'_> {
    fn report(&mut self, report: &Report, _command: &SentCommand) {
        self.reports.push(*report);
    }
    /// A fault that mutes its instance for the run: every reason but
    /// `late_result`, which only silences one block of its own (§7).
    fn log(&mut self, line: LogLine<'_>) {
        if let LogLine::Fault {
            frame,
            address,
            reason,
            ..
        } = line
            && reason != FaultReason::LateResult
        {
            self.muted.push(Muted {
                frame,
                address: address.to_string(),
                reason,
                module: None,
            });
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

/// Whether an output as [`output_state`] gives it can play: it is there, at
/// any rate (ADR 0036: a run plays at the rate the output runs at).
pub(super) fn ready_at(state: Option<&(String, Option<u32>)>) -> bool {
    state.is_some()
}

/// Stops a run on this thread; its Runner and streams are dropped here.
pub(super) fn stop(mut run: Run) {
    let _ = run.output.stop();
}

/// How often the output is looked at again while a run stopped by the
/// device waits for it (app mode, `plays`): the device list is read again.
const DEVICE_POLL: Duration = Duration::from_secs(1);

/// Retries of a device in a row before the wait ends (a flapping device).
const MAX_DEVICE_RETRIES: u32 = 5;

/// A retried run that played this long resets the count of retries.
const DEVICE_STABLE: Duration = Duration::from_secs(30);

/// What a run stopped by the device waits for.
pub(super) struct DeviceWait {
    /// The output as last seen ([`output_state`]): its key and rate.
    seen: Option<(String, Option<u32>)>,
    /// When it was last looked at.
    probed: Instant,
    /// Try once at the next look whatever the output: it was there when the
    /// run stopped (a rate switched, a device busy for a moment), or the
    /// project's files changed.
    pub(super) once: bool,
}

/// One run: from a start (the first, or `setup.apply`) to its stop.
pub(super) struct Run {
    pub(super) output: Output,
    pub(super) input: Option<Input>,
    pub(super) controller: Controller,
    pub(super) live: LiveRecorder,
    pub(super) current: Project,
    pub(super) watcher: Option<ProjectWatcher>,
    pub(super) watched: Instant,
    pub(super) retry: bool,
    /// The `project.toml` text `current` is known to hold (read before the
    /// last reload that applied, or written by a commit it took); `None`
    /// when not known.
    pub(super) file: Option<String>,
    pub(super) rate: u32,
    pub(super) buffer: u32,
    pub(super) threads: u32,
    pub(super) width: usize,
    pub(super) input_channels: usize,
    pub(super) device: String,
    /// The engine's plan version last described.
    pub(super) version: u64,
    pub(super) failure_reported: bool,
    /// The instances the fault guard muted in this run, in the order
    /// reported: each stays silent until the project plays again.
    pub(super) muted: Vec<Muted>,
    /// Signal.app's note for them (`app.muted`), made when they change.
    pub(super) muted_note: Option<String>,
    /// The log records the runner and the engine had dropped when last
    /// polled; once it grows, a fault record may have been lost.
    pub(super) log_dropped: u64,
    /// Whether the run dropped log records.
    pub(super) log_lost: bool,
    /// Why the audio system stopped the output's stream, once reported
    /// (`signal ui`, `signal mcp`: the run is kept, so what it recorded is
    /// written, and the command ends with it).
    pub(super) fault: Option<String>,
    /// How the workers are scheduled (app mode: the status line).
    pub(super) scheduling: Option<Scheduling>,
}

impl Host {
    /// The run's output stopped by the audio system (the device's rate
    /// changed, it went away). App mode: the run stops, with the reason in
    /// plain words, and with `plays` it waits for the device
    /// ([`Self::retry_device`]). `signal ui`, `signal mcp`: the run is kept
    /// (what it recorded is written when it finishes) and the fault is
    /// reported once ([`Self::output_fault`]).
    pub(super) fn check_output(&mut self) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        if run.fault.is_some() {
            return;
        }
        let Some(fault) = run.output.fault() else {
            return;
        };
        self.output_faulted(fault);
    }

    /// The run's output stopped with `fault`: [`Self::check_output`]'s
    /// outcome in either mode.
    pub(super) fn output_faulted(&mut self, fault: String) {
        if self.app.is_some() {
            return self.output_failed(fault);
        }
        if let Some(run) = self.run.as_mut() {
            run.fault = Some(fault.clone());
            self.messages.push(fault);
        }
    }

    /// Why the audio system stopped the run's output stream, outside app
    /// mode (`signal ui`, `signal mcp` end the run with it); `None` while
    /// it plays.
    pub fn output_fault(&self) -> Option<&str> {
        self.run.as_ref().and_then(|run| run.fault.as_deref())
    }

    /// App mode: stops the run whose output failed with `fault`, and says so.
    fn output_failed(&mut self, fault: String) {
        self.stop_app();
        let message = match self.wait_for_device() {
            Some(waiting) => format!("{fault}; stopped, {waiting}"),
            None => format!("{fault}; stopped"),
        };
        self.messages.push(message.clone());
        if let Some(mode) = self.app.as_mut() {
            mode.message = Some(message);
        }
    }

    /// App mode with `plays`, switched on and not held: the run stopped by
    /// the device waits for its output ([`DEVICE_POLL`]); the output as it
    /// is now is noted. An output that is there (at any rate: the next run
    /// plays at it, ADR 0036) is tried once more at the next look, unless
    /// that try is what failed. Returns
    /// what the window says it waits for, or `None` when nothing waits.
    pub(super) fn wait_for_device(&mut self) -> Option<String> {
        let waits = self
            .app
            .as_ref()
            .is_some_and(|m| m.plays && m.opened && m.power.is_on() && !m.held);
        if !waits {
            return None;
        }
        self.list_devices();
        let seen = output_state(&self.devices, &self.setup);
        let ready = ready_at(seen.as_ref());
        let mode = self.app.as_mut()?;
        let once = ready && !mode.retrying;
        mode.device_wait = Some(DeviceWait {
            seen,
            probed: Instant::now(),
            once,
        });
        Some(if once {
            "Signal tries the output again in a moment".to_owned()
        } else if ready {
            "Signal tries again when the output or the project's files change".to_owned()
        } else {
            "Signal plays again once the output is there".to_owned()
        })
    }

    /// A run stopped by the device (app mode, `plays`): every
    /// [`DEVICE_POLL`] the device list is read again, on this thread. The
    /// project plays again from frame 0, at the output's rate (ADR 0036),
    /// once the output changed (a rate switched in Audio MIDI Setup, a
    /// device plugged in, another default) and is there, or once
    /// at the next look when [`DeviceWait::once`] says so. An output that
    /// did not change is not tried again. Nothing runs while it waits.
    pub(super) fn retry_device(&mut self) {
        let Some(mode) = self.app.as_mut() else {
            return;
        };
        let Some(wait) = mode.device_wait.as_mut() else {
            return;
        };
        if self.run.is_some() || !mode.power.is_on() || mode.held || !mode.opened {
            mode.device_wait = None;
            return;
        }
        // A retried run that played long enough was no flapping device.
        if mode.retried.is_some_and(|at| at.elapsed() >= DEVICE_STABLE) {
            mode.retries = 0;
            mode.retried = None;
        }
        // Backoff: each retry in a row doubles the wait (1, 2, 4, 8, 16 s).
        if wait.probed.elapsed() < DEVICE_POLL * (1 << mode.retries.min(4)) {
            return;
        }
        wait.probed = Instant::now();
        let (seen, once) = (wait.seen.clone(), wait.once);
        self.list_devices();
        let now = output_state(&self.devices, &self.setup);
        let changed = now != seen;
        if !once && !(changed && ready_at(now.as_ref())) {
            if let Some(wait) = self.app.as_mut().and_then(|m| m.device_wait.as_mut()) {
                wait.seen = now;
            }
            return;
        }
        if let Some(mode) = self.app.as_mut() {
            if mode.retries >= MAX_DEVICE_RETRIES {
                mode.device_wait = None;
                let message = format!(
                    "the output failed {MAX_DEVICE_RETRIES} times in a row; Signal stopped trying: restart the project (Project ▸ Restart from Files) or switch the power off and on"
                );
                mode.message = Some(message.clone());
                self.messages.push(message);
                return;
            }
            mode.retries += 1;
            mode.retried = Some(Instant::now());
            mode.retrying = true;
        }
        let result = self.play_app();
        if let Some(mode) = self.app.as_mut() {
            mode.retrying = false;
        }
        let message = match result {
            Ok(()) => format!(
                "the output runs at {} Hz: playing again from frame 0",
                self.rate
            ),
            Err(failure) => failure.message,
        };
        self.messages.push(message.clone());
        if let Some(mode) = self.app.as_mut() {
            mode.message = Some(message);
        }
    }

    /// Plays the folder's project: with the first run's options, or with
    /// the Setup's (as the dialog's Apply plays again).
    pub(super) fn build(&self, first: bool) -> Result<(Run, Vec<String>), String> {
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
        let seed = project.seed;
        let mut notes = Vec::new();
        // ADR 0036: the run plays at the rate the output runs at, never
        // switching it (the owner's decision of 2026-10-23).
        let (rate, notice) = match (&self.backend, first) {
            (Backend::CoreAudio, true) => {
                let selector = self.first.device.clone().map_or(
                    signal_audio::DeviceSelector::Default,
                    signal_audio::DeviceSelector::Name,
                );
                signal_audio::live_rate(&selector, project.sample_rate).map_or(
                    (project.sample_rate, None),
                    |(name, rate)| {
                        let notice = signal_audio::rate_notice(
                            &name,
                            &project.name,
                            project.sample_rate,
                            rate,
                        );
                        (rate, notice)
                    },
                )
            }
            (Backend::Null(_), true) => (project.sample_rate, None),
            (_, false) => live_rate(
                &self.backend,
                &self.devices,
                &self.setup,
                project.sample_rate,
                &project.name,
            ),
        };
        notes.extend(notice);
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
        // ADR 0036: the period the device runs at (its own while in use).
        let buffer = output.period().unwrap_or(buffer);
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
        let (mut runner, controller) = realtime_with(
            &plan,
            &self.registry,
            config,
            Some(rt_thread::mach_semaphores),
        )
        .map_err(|e| e.to_string())?;
        runner.set_ceiling(self.ceiling);
        let recorder = SessionRecorder::new(
            &project,
            &self.registry,
            rate,
            seed,
            self.provenance.clone(),
        )
        .map_err(|e| e.to_string())?;
        let live = LiveRecorder::new(self.history(recorder), self.registry.clone());
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
            muted: Vec::new(),
            muted_note: None,
            log_dropped: 0,
            log_lost: false,
            fault: None,
            scheduling: None,
        };
        Ok((run, notes))
    }

    /// The run's recorder: keeping the run's history (versions, commands,
    /// boundary records) only when the host records a session; otherwise
    /// the bookkeeping alone, which stays bounded however long it plays.
    pub(super) fn history(&self, recorder: SessionRecorder) -> SessionRecorder {
        if self.recording {
            recorder
        } else {
            recorder.without_history()
        }
    }

    /// A new run plays the file: no live edit is left in it (the unsaved
    /// controls' values are played again by [`Host::play_carried`]).
    pub(super) fn install(&mut self, mut run: Run) {
        self.graph_playing = None;
        self.panel = Panel::new(&run.current, &self.registry);
        self.refresh_warnings();
        self.plan = run.current.plan(&self.registry).ok().map(|p| p.plan);
        self.rate = run.rate;
        run.version = run.controller.status().version;
        self.values.clear();
        self.resend.clear();
        self.port_edits.clear();
        self.auditions.clear();
        self.releases.clear();
        // Mutes a last poll left are the last run's, never this one's.
        self.muted.clear();
        self.run = Some(run);
    }

    /// Drains the Controller: reports and values (beside the recorder),
    /// meters, scopes, input meters; a new plan version is described again.
    pub(super) fn poll(&mut self) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        let mut observed = Observed {
            values: &mut self.values,
            reports: &mut self.reports,
            muted: &mut self.muted,
        };
        let meters = &mut self.meters;
        run.controller
            .poll_with(&mut Both(&mut run.live, &mut observed), |m: &Meter| {
                meters.push(*m);
            });
        // A muted instance is silent for the rest of the run, while its
        // controls still show (and take) the values sent: say so, once each.
        // Signal.app shows them as long as the run lasts (`app.muted`).
        take_muted(&mut self.muted, run, self.plan.as_ref(), &mut self.messages);
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
        // A dropped log record may have been a fault's.
        let dropped = status.log_overflows + status.engine_log_overflows;
        if dropped > run.log_dropped {
            run.log_dropped = dropped;
            if !run.log_lost {
                run.log_lost = true;
                run.muted_note = muted_message(&run.muted, true);
            }
        }
        // A swap that removed a muted instance, or gave its address another
        // type, brought a new, unfaulted one (`semantics.md` §14).
        if swapped && !run.muted.is_empty() {
            let plan = self.plan.as_ref();
            let before = run.muted.len();
            run.muted
                .retain(|m| plan.is_none_or(|plan| module_at(plan, &m.address) == m.module));
            if run.muted.len() != before {
                run.muted_note = muted_message(&run.muted, run.log_lost);
            }
        }
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
        for (&sub, s) in &mut self.subs.by_id {
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
                let slot = self.subs.by_id.get(&sub).and_then(|s| match &s.stream {
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

    /// The plan now running: a live port edit whose plan base changed (or
    /// whose port went) is dropped, the base being what the file now says.
    pub(super) fn replace_plan(&mut self, plan: Option<ExecutionPlan>) {
        let old = std::mem::replace(&mut self.plan, plan);
        let new = self.plan.as_ref();
        self.port_edits.retain(|port, _| {
            let after = plan_base(new, port);
            after.is_some()
                && after.map(f64::to_bits) == plan_base(old.as_ref(), port).map(f64::to_bits)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn muted(address: &str, reason: FaultReason) -> Muted {
        Muted {
            frame: Frame(7),
            address: address.to_owned(),
            reason,
            module: None,
        }
    }

    /// The note: the recovery said by reason; dropped log records said,
    /// alone too; nothing to say without either.
    #[test]
    fn the_muted_note_says_how_and_whether_the_list_is_whole() {
        assert_eq!(muted_message(&[], false), None);
        let lost = muted_message(&[], true).unwrap();
        assert!(lost.starts_with("Some log records were dropped"), "{lost}");
        let magnitude = muted_message(&[muted("/a", FaultReason::Magnitude)], false).unwrap();
        assert!(
            magnitude.contains("with its values back in range"),
            "{magnitude}"
        );
        let panic = muted("/b", FaultReason::Panic).message();
        assert!(
            panic.contains("its call panicked") && panic.contains("broke the module contract"),
            "{panic}"
        );
        assert!(!panic.contains("values back in range"), "{panic}");
        let overflow = muted("/c", FaultReason::EventOverflow).message();
        assert!(
            overflow.contains("with fewer events reaching it"),
            "{overflow}"
        );
        let both = muted_message(&[muted("/a", FaultReason::NonFinite)], true).unwrap();
        assert!(
            both.starts_with("/a was muted at frame 7")
                && both.ends_with("an instance the fault guard muted may be missing here"),
            "{both}"
        );
    }
}
