//! The Setup and the keyboard shortcuts (ADR 0034, 0036): devices, checks
//! and Apply.

use std::fs;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use signal_audio::{DeviceInfo, DeviceRef, Direction, Resolution, labels, resolve};
use signal_engine::RtPolicy;

use crate::app;
use crate::keys::{self, Keys};
use crate::player::{Backend, Interface, Output, Selection};
use crate::setup::{Choice, capabilities, channels, found, rate_check};

use super::failure::{Failure, fail};
use super::run::stop;
use super::{Host, with};

/// `buffer` of a message (1 to 8192 frames), else `default`.
pub(super) fn buffer_of(message: &Value, default: u32) -> Result<u32, Failure> {
    match message.get("buffer") {
        None => Ok(default),
        // ADR 0036: the device's own buffer (`setup.apply`).
        Some(Value::String(device)) if device == "device" => Ok(default),
        Some(v) => v
            .as_u64()
            .filter(|b| (1..=8192).contains(b))
            .map(|b| b as u32)
            .ok_or_else(|| fail("bad-value", "buffer is 1 to 8192 frames")),
    }
}

/// The output device's name, for `setup.get`'s run.
pub(super) fn device_name(output: &Output) -> String {
    match output {
        Output::Device(driver) => driver.name().to_owned(),
        Output::Null(..) => "null".to_owned(),
    }
}

/// How often the keyboard shortcuts file is looked at (ADR 0034).
const KEYS_POLL: Duration = Duration::from_millis(250);

impl Host {
    pub(super) fn list_devices(&mut self) {
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
    pub(super) fn watch_keys(&mut self) {
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
    pub(super) fn keys_set(&mut self, message: &Value) -> Result<Value, Failure> {
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
    pub(super) fn keys_reset(&mut self, message: &Value) -> Result<Value, Failure> {
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
            // ADR 0038 (M4.1, additive): every role the Setup binds.
            "play": crate::devices::play_json(&self.setup),
        })
    }

    /// `device.route`, `device.unroute` (ADR 0038, M4.1): `crate::devices`
    /// writes the binding (the Setup in memory, or `devices.local.toml`);
    /// then as `setup.apply` with nothing else changed: the Setup saved and,
    /// while playing, the run played again from frame 0 with it.
    pub(super) fn device_route(&mut self, message: &Value, route: bool) -> Result<Value, Failure> {
        if self.recording && self.run.is_some() {
            return Err(fail(
                "recording",
                "a run that records a session or its audio is not rebound (a recording covers one run from frame 0)",
            ));
        }
        let dir = self.opening.dir.clone();
        let routed = if route {
            crate::devices::route(message, &mut self.setup, Some(&dir))
        } else {
            crate::devices::unroute(message, &mut self.setup, Some(&dir))
        }
        .map_err(|refused| fail(refused.code, refused.message))?;
        let reply = self.setup_apply(&json!({}))?;
        let scope = match routed.scope {
            crate::devices::Scope::Machine => "machine",
            crate::devices::Scope::Project => "project",
        };
        Ok(with(
            reply,
            "routed",
            json!({"role": routed.role, "scope": scope, "notes": routed.notes}),
        ))
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
    pub(super) fn setup_get(&self) -> Value {
        json!({
            "type": "setup",
            "path": self.setup_path.display().to_string(),
            "setup": self.setup_json(),
            // ADR 0038 §6 (M4.1, additive): this project's devices.local.toml.
            "local": crate::devices::local_json(&self.opening.dir),
            "problems": self.setup_problems,
            "playing": self.run.is_some(),
            "run": self.run_json(),
            "rtPolicies": RtPolicy::ALL.iter().map(|p| p.name()).collect::<Vec<_>>(),
        })
    }

    /// `setup.devices`: the device list (refreshed when asked, refused while
    /// playing, as the dialog does), each output's capabilities, where the
    /// Setup's devices are in it, and the compute devices.
    pub(super) fn setup_devices(&mut self, message: &Value) -> Result<Value, Failure> {
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
    pub(super) fn setup_check(&self, message: &Value) -> Result<Value, Failure> {
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
    pub(super) fn setup_apply(&mut self, message: &Value) -> Result<Value, Failure> {
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
                self.build_failed();
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
}
