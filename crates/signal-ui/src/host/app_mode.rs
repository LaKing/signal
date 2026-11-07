//! Signal.app's state (app mode): power, transport, and the `status` and
//! `app` messages.

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use signal_project::live::{FolderState, ProjectWatcher};
use signal_project::session::LiveRecorder;

use crate::app::{self, Status};
use crate::document::Document;
use crate::panel::Panel;

use super::Host;
use super::failure::{Failure, fail};
use super::run::{DeviceWait, Run};
use super::setup_keys::device_name;

/// The refusal of an app message outside app mode.
pub(super) fn app_only() -> Failure {
    fail(
        "not-allowed",
        "only Signal.app's server opens projects and plays or stops them",
    )
}

/// The refusal of what needs the app switched on.
pub(super) fn powered_off() -> Failure {
    fail("powered-off", "Signal is switched off")
}

/// The power switch as `welcome`, `app` and `power` carry it.
pub(super) fn power_json(power: app::Power) -> Value {
    json!(if power.is_on() { "on" } else { "off" })
}

/// How often the status line is pushed while playing (app mode).
const STATUS_EVERY: Duration = Duration::from_millis(250);

/// Signal.app's state beside the protocol's (app mode): what the app's
/// window showed beside the panel and the meters.
#[derive(Default)]
pub(super) struct AppMode {
    /// A project folder is open (the host's `dir`).
    pub(super) opened: bool,
    /// The open folder's diagnostics, when it does not load.
    pub(super) diagnostics: Vec<String>,
    /// ADR 0042, 0043: the values its files hold outside their range or
    /// value set, corrected (warnings: the project loads and plays them).
    pub(super) warnings: Vec<String>,
    /// Notes on the run: the channel map, the input.
    pub(super) notes: Vec<String>,
    /// The last refusal or notice.
    pub(super) message: Option<String>,
    /// ADR 0042 §3: why the corrected values were not written before the
    /// Save running now (it saves anyway); its message once it is saved.
    pub(super) correction_error: Option<String>,
    /// The last file-watch outcome, and whether it was an error.
    pub(super) reload: Option<(String, bool)>,
    /// The status line (of the last run once stopped).
    pub(super) status: Option<Status>,
    /// When the status was last pushed.
    pub(super) pushed: Option<Instant>,
    /// The `app` message last pushed.
    pub(super) last: Option<Value>,
    /// The power switch (application state, never written anywhere).
    pub(super) power: app::Power,
    /// The open project plays while switched on ([`AppHostConfig::plays`]).
    pub(super) plays: bool,
    /// Stopped on purpose (`transport.stop`): with `plays`, Setup ▸ Apply
    /// then plays nothing; the next open, switch-on or restart plays again.
    pub(super) held: bool,
    /// Stopped by the device while switched on, with `plays`: it did not
    /// start, or the audio system stopped its stream. Plays again, at the
    /// output's rate (ADR 0036), once the output changes and is there
    /// ([`Host::retry_device`]).
    pub(super) device_wait: Option<DeviceWait>,
    /// A retry of the device is playing the project ([`Host::retry_device`]):
    /// its failure waits for a change, not for another try.
    pub(super) retrying: bool,
    /// Retries of the device in a row ([`Host::retry_device`]): each doubles
    /// the wait before the next look, and after [`MAX_DEVICE_RETRIES`] the
    /// wait ends. Reset by a play that is no retry, and by a retried run
    /// that played [`DEVICE_STABLE`].
    pub(super) retries: u32,
    /// When the last retry played.
    pub(super) retried: Option<Instant>,
}

impl Host {
    /// ADR 0033: with [`AppHostConfig::plays`], switched on and with a
    /// project open, plays it if it does not play yet; a failure (the
    /// project does not load, the device does not start) is the window's
    /// message, and the app stays stopped until the next open, switch-on or
    /// Project ▸ Restart from Files.
    pub(super) fn autoplay(&mut self) {
        let wanted = self
            .app
            .as_ref()
            .is_some_and(|m| m.plays && m.opened && m.power.is_on());
        if wanted
            && self.run.is_none()
            && let Err(failure) = self.play_app()
            && let Some(mode) = self.app.as_mut()
        {
            // Only a failure is noted, after the message opening left
            // (unsaved edits let go, a Setup not saved), which stays.
            mode.message = Some(match mode.message.take() {
                Some(before) => format!("{before}; {}", failure.message),
                None => failure.message,
            });
        }
    }

    /// App mode: the status line every [`STATUS_EVERY`] while playing, and
    /// the `app` message whenever it changed.
    pub(super) fn push_app(&mut self) {
        if let Some(document) = self.opening.document.as_mut() {
            document.refresh();
        }
        self.answer_saves();
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
    pub(super) fn app_json(&self) -> Value {
        let Some(mode) = &self.app else {
            return Value::Null;
        };
        json!({
            "type": "app",
            "generation": self.opening.generation,
            "project": mode.opened.then(|| json!({
                "dir": self.opening.dir.display().to_string(),
                "file": self.opening.document.as_ref().map(|d| d.file().display().to_string()),
                "name": self.project_name(),
                "edited": self.opening.document.as_ref().is_some_and(Document::edited),
                "sampleRate": (mode.diagnostics.is_empty() && self.rate > 0).then_some(self.rate),
                "diagnostics": mode.diagnostics,
                "warnings": mode.warnings,
            })),
            "playing": self.run.is_some(),
            "power": power_json(mode.power),
            "reload": mode.reload.as_ref().map(|(message, error)| json!({"message": message, "error": error})),
            "notes": mode.notes,
            "message": mode.message,
            "muted": self.run.as_ref().and_then(|run| run.muted_note.clone()),
            // The schematic marks them (every one, unlike the note).
            "mutedInstances": self
                .run
                .as_ref()
                .map(|run| run.muted.iter().map(|m| m.address.clone()).collect::<Vec<_>>())
                .unwrap_or_default(),
            "writes": self.writes,
            "setupPath": self.setup_path.display().to_string(),
            "setupProblems": self.setup_problems,
        })
    }

    /// App mode: plays the open project from frame 0 with the Setup
    /// ([`app::start`]), as `transport.play` and the switch do (Project ▸ Restart from Files).
    pub(super) fn play_app(&mut self) -> Result<(), Failure> {
        let opened = match self.app.as_mut() {
            Some(mode) => {
                mode.notes.clear();
                mode.device_wait = None;
                if !mode.retrying {
                    mode.retries = 0;
                }
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
        let file = signal_project::read_regular_to_string(
            self.opening.dir.join(signal_project::PROJECT_FILE),
        )
        .ok();
        let state = FolderState::read(&self.opening.dir).ok();
        let dir = self.opening.dir.clone();
        let (project, plan, width) = match self.load(&dir) {
            Ok(loaded) => {
                // Stopping again with the folder unchanged loads nothing.
                self.remember_stopped(state, &loaded.0, &loaded.1);
                loaded
            }
            Err(diagnostics) => {
                if let Some(mode) = self.app.as_mut() {
                    mode.diagnostics = diagnostics;
                    mode.warnings.clear();
                }
                return Err(fail("does-not-load", "the project does not load"));
            }
        };
        self.rate = project.sample_rate;
        // The unsaved controls stay unsaved: their values, kept while the
        // run starts, are played again in it.
        let carried = self.carried();
        self.panel = Panel::new(&project, &self.registry);
        self.refresh_warnings();
        self.play_carried(carried.clone());
        // ADR 0038 §6 (M4.1): this project's devices.local.toml over the Setup.
        let (setup, mut notes) = crate::devices::effective_setup(&self.setup, &self.local_dir());
        let started = app::start(
            &self.backend,
            &self.devices,
            &setup,
            &self.registry,
            &project,
            plan,
            width,
            self.recording,
            &mut notes,
        );
        if let Some(mode) = self.app.as_mut() {
            mode.diagnostics.clear();
            mode.notes.extend(notes);
        }
        let started = match started {
            Ok(started) => started,
            Err(error) => {
                let error = match self.wait_for_device() {
                    Some(waiting) => format!("{error}; {waiting}"),
                    None => error,
                };
                return Err(fail("device", error));
            }
        };
        let (watcher, reload) = match ProjectWatcher::new(&self.opening.dir) {
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
            // ADR 0038 §9: a recording run records what its sources read.
            live: LiveRecorder::new(self.history(started.recorder), self.registry.clone())
                .with_inputs(started.inputs),
            current: project,
            watcher,
            watched: Instant::now(),
            polled: Instant::now(),
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
            muted: Vec::new(),
            muted_note: None,
            log_dropped: 0,
            log_lost: false,
            fault: None,
            scheduling: Some(started.scheduling),
        };
        self.install(run);
        self.rearm();
        self.play_carried(carried);
        if let Some(mode) = self.app.as_mut() {
            mode.held = false;
        }
        Ok(())
    }

    /// Stops the run, as `transport.stop` and the power switch do: its last status stays, edits
    /// waiting for reports are abandoned, subscriptions are kept (scopes
    /// start again with the next run). The unsaved controls keep their values
    /// (played again by the next run); live edits of plan ports end.
    pub(super) fn stop_app(&mut self) {
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
        self.live.run_ended();
        self.auditions.clear();
        self.abandon();
    }

    /// `transport.play`, `transport.stop` (app mode; `signal ui`'s are
    /// [`Host::serve_transport`]).
    pub(super) fn transport(&mut self, play: bool) -> Result<Value, Failure> {
        if self.app.is_none() {
            return self.serve_transport(play);
        }
        let result = if play && !self.powered() {
            Err(powered_off())
        } else if play {
            // Plays the project again from frame 0 from its files, playing
            // or not (Project ▸ Restart from Files, ADR 0033): a run is
            // stopped first, its unsaved controls kept and played again.
            if self.run.is_some() {
                self.stop_app();
            }
            self.play_app()
        } else {
            if self.run.is_some() {
                self.stop_app();
                self.list_devices();
            }
            // Stopped on purpose: Setup ▸ Apply does not play it (ADR 0033),
            // nor does the device coming back.
            if let Some(mode) = self.app.as_mut() {
                mode.held = true;
                mode.device_wait = None;
            }
            Ok(())
        };
        self.noted(result)?;
        Ok(json!({"type": "transport", "playing": self.run.is_some()}))
    }

    /// `signal ui`'s `transport.stop` and `transport.play` (M2.12-1, ADR 0029
    /// §2): stop stops the run as app mode's `transport.stop` does (edits waiting for
    /// reports are abandoned; the unsaved controls keep their values); play
    /// plays the project it serves again from frame 0 from its files, as
    /// `project.open` of that project does. Both are refused while the run
    /// records, since a recording covers one run from frame 0.
    fn serve_transport(&mut self, play: bool) -> Result<Value, Failure> {
        if self.recording {
            return Err(fail(
                "recording",
                "a run that records a session or its audio is not stopped or played again (a recording covers one run from frame 0)",
            ));
        }
        self.stop_app();
        if play {
            self.live.failed.clear();
            let (run, notes) = match self.build(true) {
                Ok(built) => built,
                Err(error) => {
                    self.build_failed();
                    return Err(fail("does-not-load", error));
                }
            };
            self.install(run);
            self.rearm();
            self.messages.extend(notes);
        }
        Ok(json!({"type": "transport", "playing": self.run.is_some()}))
    }

    /// `app.power` (app mode): off stops the run (the project, its panel and
    /// its unsaved values stay, as with Stop) and lets go of every
    /// subscription, so nothing runs and nothing streams; on plays again if
    /// it played when switched off.
    pub(super) fn power(&mut self, message: &Value) -> Result<Value, Failure> {
        let on = message
            .get("on")
            .and_then(Value::as_bool)
            .ok_or_else(|| fail("bad-message", "app.power needs `on`: true or false"))?;
        let playing = self.run.is_some();
        let Some(mode) = self.app.as_mut() else {
            return Err(app_only());
        };
        let result = if on {
            // ADR 0033: with `plays`, switched on plays the open project,
            // whatever played when it went off.
            let resume = mode.power.on();
            if resume || (mode.plays && mode.opened && self.run.is_none()) {
                self.play_app()
            } else {
                Ok(())
            }
        } else {
            mode.power.off(playing);
            mode.device_wait = None;
            // The device is released, the workers joined (the Runner is
            // dropped here) and the file watch ends with the run.
            self.stop_app();
            let subs: Vec<u32> = self.subs.by_id.keys().copied().collect();
            for sub in subs {
                self.drop_sub(sub);
            }
            Ok(())
        };
        self.noted(result)?;
        Ok(json!({"type": "power", "on": self.powered(), "playing": self.run.is_some()}))
    }

    /// App mode: the message to show for an app message's outcome (cleared
    /// by each one, as the app's commands do).
    pub(super) fn noted<T>(&mut self, result: Result<T, Failure>) -> Result<T, Failure> {
        if let Some(mode) = self.app.as_mut() {
            mode.message = result.as_ref().err().map(|f| f.message.clone());
        }
        result
    }
}
