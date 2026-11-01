//! Live edits: `set`, `ramp`, `trigger`, `note` and `revert`, their
//! addresses and reports, the values the files do not hold (`unsaved`),
//! and the live values carried across a new panel.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use signal_core::{Curve, Direction as PortDirection, EventPayload, Frame, PortAddress, PortShape};
use signal_engine::{Outcome, Stamp};

use crate::panel::{Panel, ramp_frames};

use super::audition::{AUDITION_INPUT, NOTE_VELOCITY};
use super::controls::{bits, control_of, plan_base};
use super::failure::{Failure, fail};
use super::{Host, with, within};

/// An edit waiting for its reports.
pub(super) struct Pending {
    client: u64,
    id: Value,
    remaining: usize,
    frame: u64,
    requested: u64,
    refused: bool,
    /// The value it sends, which a runner's refusal marks as not applied.
    edited: Option<Edited>,
}

/// A live value an edit sent.
enum Edited {
    /// To a control (`instance`, `parameter`).
    Control((String, String), f64),
    /// To a plan parameter port that is no control.
    Port(PortAddress, f64),
}

/// A control's failed write (`docs/ui-protocol.md` "Unsaved"): its error,
/// in order of failures; the live value is the panel's drag, never kept
/// here. `reset`: the write was of `null` (each port to its default), with
/// whether the Controller took the defaults.
pub(super) struct Failed {
    pub(super) error: String,
    pub(super) seq: u64,
    pub(super) reset: Option<bool>,
}

/// A parameter whose live value the file does not hold, as `unsaved` lists
/// it.
pub(super) struct Unsaved {
    pub(super) address: String,
    pub(super) file: f64,
    pub(super) in_file: bool,
    pub(super) control: bool,
    /// The live value is known to be the one last sent.
    applied: bool,
    error: Option<(u64, String)>,
}

/// Where an edit goes: a control's ports or one plan parameter.
pub(super) struct Aim {
    pub(super) ports: Vec<PortAddress>,
    min: f64,
    max: f64,
    /// ADR 0043: the value set of a discrete parameter.
    values: Option<Vec<(f64, String)>>,
    /// ADR 0043: how a value between its values is read (and snapped).
    snap: signal_core::Snap,
    control: Option<(String, String)>,
}

impl Aim {
    /// `value` as sent: clamped to the range and, with a value set, snapped
    /// as its modules read it.
    fn take(&self, value: f64) -> f64 {
        let clamped = within(value, self.min, self.max);
        match &self.values {
            Some(values) => crate::panel::snap_to(values, self.snap, clamped),
            None => clamped,
        }
    }

    /// What sending `value` here edits.
    fn edited(&self, value: f64) -> Option<Edited> {
        match &self.control {
            Some(control) => Some(Edited::Control(control.clone(), value)),
            None => self.ports.first().map(|p| Edited::Port(p.clone(), value)),
        }
    }
}

impl Host {
    /// `unsaved` to every client whenever it changed; a failure whose
    /// control is no longer unsaved (live and file agree again) is dropped.
    pub(super) fn push_unsaved(&mut self) {
        let listed: BTreeSet<String> = self.unsaved().into_iter().map(|u| u.address).collect();
        self.failed
            .retain(|(i, p), _| listed.contains(&format!("/{i}.{p}")));
        self.port_edits
            .retain(|port, _| listed.contains(port.as_str()));
        let current = self.unsaved_json();
        if self.unsaved_pushed.as_ref() != Some(&current) {
            self.broadcast(&with(current.clone(), "type", json!("unsaved")));
            self.unsaved_pushed = Some(current);
        }
    }

    /// Every parameter whose live value the file does not hold, or may not
    /// (`docs/ui-protocol.md` "Unsaved"): the controls in the panel's order,
    /// then the plan parameter ports edited live.
    pub(super) fn unsaved(&self) -> Vec<Unsaved> {
        let mut list = Vec::new();
        for instance in self.panel.view(&self.values) {
            for p in &instance.parameters {
                let key = (instance.name.clone(), p.name.clone());
                let Some((file, in_file)) = self.panel.file_base(&key.0, &key.1) else {
                    continue;
                };
                let drag = self.panel.dragged(&key.0, &key.1);
                let failed = self.failed.get(&key);
                let resend = self.resend.contains(&key);
                let unsent = drag.is_some_and(|d| !d.sent)
                    || failed.is_some_and(|f| f.reset == Some(false))
                    || resend;
                let differs = drag.is_some_and(|d| d.value.to_bits() != file.to_bits())
                    || (drag.is_none() && failed.is_some_and(|f| f.reset.is_some()) && in_file);
                if unsent || differs {
                    list.push(Unsaved {
                        address: format!("/{}.{}", key.0, key.1),
                        file,
                        in_file,
                        control: true,
                        applied: !unsent,
                        error: failed.map(|f| (f.seq, f.error.clone())),
                    });
                }
            }
        }
        for (port, &(value, sent)) in &self.port_edits {
            let Some(file) = plan_base(self.plan.as_ref(), port) else {
                continue;
            };
            if !sent || value.to_bits() != file.to_bits() {
                list.push(Unsaved {
                    address: port.as_str().to_owned(),
                    file,
                    in_file: false,
                    control: false,
                    applied: sent,
                    error: None,
                });
            }
        }
        list
    }

    /// `unsaved`'s body (also `describe`'s `unsaved`).
    pub(super) fn unsaved_json(&self) -> Value {
        let list = self.unsaved();
        let last = list
            .iter()
            .filter_map(|u| u.error.as_ref())
            .max_by_key(|(seq, _)| *seq)
            .map(|(_, error)| error.clone());
        let parameters: Vec<Value> = list
            .iter()
            .map(|u| {
                json!({
                    "address": u.address,
                    "file": u.file,
                    "inFile": u.in_file,
                    "control": u.control,
                    "applied": u.applied,
                    "error": u.error.as_ref().map(|(_, e)| e),
                })
            })
            .collect();
        json!({"count": list.len(), "parameters": parameters, "lastError": last})
    }

    /// How many parameters are unsaved, as last pushed (each step pushes
    /// `unsaved` when it changed), so the window's status costs no listing.
    pub(super) fn unsaved_count(&self) -> usize {
        self.unsaved_pushed
            .as_ref()
            .and_then(|pushed| pushed.get("count"))
            .and_then(Value::as_u64)
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(0)
    }

    pub(super) fn take_reports(&mut self) {
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
                // A runner refused it: the value did not reach the ports.
                match (&done.edited, done.refused) {
                    (Some(Edited::Control((instance, parameter), value)), true) => {
                        self.panel.unsend(instance, parameter, *value);
                    }
                    (Some(Edited::Port(port, value)), true) => {
                        if let Some(edit) = self.port_edits.get_mut(port)
                            && edit.0.to_bits() == value.to_bits()
                        {
                            edit.1 = false;
                        }
                    }
                    _ => {}
                }
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

    /// A control (`/<instance>.<parameter>`: a top-level instance's
    /// parameter, as the panel edits it) or a plan parameter port.
    pub(super) fn aim(&self, address: &str) -> Result<Aim, Failure> {
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
                values: target.values,
                snap: target.snap,
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
                values: d.values.map(|values| {
                    values
                        .iter()
                        .map(|v| (v.value, v.label.to_owned()))
                        .collect()
                }),
                snap: d.snap,
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
    pub(super) fn edit(
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
        let mut note = None;
        let (ports, payload, drag) = match kind {
            "trigger" => (
                vec![self.event_input(address)?],
                EventPayload::Trigger,
                None,
            ),
            "note" => {
                // A live note (ADR 0039): `std.note_seq.audition`'s value,
                // only into an input named `audition` (into any other event
                // input the value would be a huge impulse or a tick count).
                let port = self.event_input(address)?;
                if port.port() != AUDITION_INPUT {
                    return Err(fail(
                        "not-an-audition-input",
                        format!(
                            "{address} is an event input but no `audition`: a note goes to a sequencer's `audition` (std.note_seq)"
                        ),
                    ));
                }
                let key = message
                    .get("key")
                    .and_then(Value::as_u64)
                    .and_then(|k| u8::try_from(k).ok())
                    .filter(|k| *k <= 127)
                    .ok_or_else(|| fail("bad-value", "`key` must be an integer from 0 to 127"))?;
                let on = message
                    .get("on")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| fail("bad-value", "`on` must be true or false"))?;
                let velocity = match message.get("velocity") {
                    None | Some(Value::Null) => NOTE_VELOCITY,
                    Some(_) => within(number("velocity")?, 0.0, 1.0),
                };
                let payload = EventPayload::Set(signal_std::audition_value(key, velocity, on));
                note = Some((port.clone(), key, on));
                (vec![port], payload, None)
            }
            "set" => {
                let aim = self.aim(address)?;
                let value = aim.take(number("value")?);
                let edited = aim.edited(value);
                (aim.ports, EventPayload::Set(value), edited)
            }
            _ => {
                let aim = self.aim(address)?;
                let to = aim.take(number("to")?);
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
                let edited = aim.edited(to);
                (aim.ports, payload, edited)
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
        // The live value the file may not hold (`docs/ui-protocol.md`
        // "Unsaved"): the latest one, and whether it reached every port.
        match &drag {
            Some(Edited::Control((instance, parameter), value)) => {
                self.panel
                    .drag(instance, parameter, *value, refused.is_none());
                if let Some(failed) = self.failed.get_mut(&(instance.clone(), parameter.clone())) {
                    failed.reset = None;
                }
            }
            Some(Edited::Port(port, value)) => {
                self.port_edits
                    .insert(port.clone(), (*value, refused.is_none()));
            }
            None => {}
        }
        if let Some(error) = refused {
            return Err(fail("refused", error));
        }
        // A note sent for a client already gone (a live edit queued behind
        // the writer lock, run after it left): ended at once, never held
        // for a client that cannot end it.
        if let Some((port, key, true)) = &note
            && !self.clients.contains_key(&client)
        {
            self.releases.insert((port.clone(), *key));
            self.send_releases();
        }
        // The notes this client holds, ended for it if it goes.
        if let Some((port, key, on)) = note.filter(|_| self.clients.contains_key(&client)) {
            // A note of this key taken now supersedes a release still
            // waiting for it (that note was ended; this one is held).
            self.releases.remove(&(port.clone(), key));
            let held = self.auditions.entry(client).or_default();
            if on {
                // One held note per port and client: a new one takes over,
                // as the sequencer's voice does.
                held.retain(|(p, _)| *p != port);
                held.insert((port, key));
            } else {
                // Only the key held ends: an off of a key slid away from (or
                // of another roll's key on the port) leaves the held one,
                // which the sequencer still sounds.
                held.remove(&(port, key));
            }
            if held.is_empty() {
                self.auditions.remove(&client);
            }
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
                edited: drag,
            },
        );
        Ok(())
    }

    /// Moves each port to its value over the panel's ramp; whether the
    /// Controller took every one (true when nothing plays).
    fn send_ramps(&mut self, sends: Vec<(PortAddress, f64)>) -> bool {
        self.send_ramps_at(sends, Stamp::Default)
    }

    /// [`Host::send_ramps`], every ramp stamped `stamp`.
    pub(super) fn send_ramps_at(&mut self, sends: Vec<(PortAddress, f64)>, stamp: Stamp) -> bool {
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
            if run.controller.send_event(port, payload, stamp).is_err() {
                return false;
            }
        }
        true
    }

    /// `revert`: an unsaved parameter's live value back to the file's (each
    /// port to its own default without a key; a plan port to the plan's
    /// base). Refused, the parameter stays unsaved.
    pub(super) fn revert(&mut self, message: &Value) -> Result<Value, Failure> {
        let address = message.get("address").and_then(Value::as_str).unwrap_or("");
        let port: PortAddress = address
            .parse()
            .map_err(|e| fail("bad-address", format!("{address}: {e}")))?;
        let listed = self.unsaved().into_iter().find(|u| u.address == address);
        let Some(unsaved) = listed else {
            return Err(fail("not-unsaved", format!("{address} is not unsaved")));
        };
        let playing = self.run.is_some();
        let control = control_of(&port)
            .and_then(|(i, p)| self.panel.target(&i, &p).map(|t| (i, p, t)))
            .filter(|_| unsaved.control);
        let sent = match &control {
            Some((_, _, target)) if unsaved.in_file => self.send_ramps(
                target
                    .ports
                    .iter()
                    .cloned()
                    .map(|p| (p, unsaved.file))
                    .collect(),
            ),
            Some((_, _, target)) => self.send_ramps(
                target
                    .ports
                    .iter()
                    .cloned()
                    .zip(target.defaults.iter().copied())
                    .collect(),
            ),
            None => self.send_ramps(vec![(port.clone(), unsaved.file)]),
        };
        if !sent {
            // Some ports may have taken it: what they hold is not known.
            match &control {
                Some((instance, parameter, _)) => {
                    if let Some(drag) = self.panel.dragged(instance, parameter) {
                        self.panel.drag(instance, parameter, drag.value, false);
                    }
                }
                None => {
                    if let Some(edit) = self.port_edits.get_mut(&port) {
                        edit.1 = false;
                    }
                }
            }
            return Err(fail(
                "refused",
                format!("{address}: the running project did not take the file's value"),
            ));
        }
        match control {
            Some((instance, parameter, _)) => {
                self.panel.release(&instance, &parameter);
                let key = (instance, parameter);
                self.failed.remove(&key);
                self.resend.remove(&key);
            }
            None => {
                self.port_edits.remove(&port);
            }
        }
        let describe = self.describe(false);
        self.broadcast(&describe);
        Ok(json!({
            "type": "reverted",
            "address": address,
            "value": unsaved.in_file.then_some(unsaved.file),
            "sent": playing,
        }))
    }

    /// Takes `new` (the panel of a reloaded or written project), keeping the
    /// drags, failures and pending resends of the controls whose file value
    /// did not change; a changed value was sent live by the reload (or
    /// written), a control that is gone has nothing left to save.
    pub(super) fn reconcile(&mut self, new: Panel) {
        let old = std::mem::replace(&mut self.panel, new);
        let kept = |panel: &Panel, (instance, parameter): &(String, String)| {
            let after = panel.file_base(instance, parameter);
            after.is_some() && bits(after) == bits(old.file_base(instance, parameter))
        };
        for (key, drag) in old.drags() {
            if kept(&self.panel, key) {
                self.panel.restore(key.clone(), *drag);
            }
        }
        let panel = &self.panel;
        self.failed.retain(|key, _| kept(panel, key));
        self.resend.retain(|key| kept(panel, key));
        // ADR 0042, 0043: the corrections of the files as they are now.
        self.refresh_warnings();
    }

    /// The unsaved controls' live values that a new run of the same project
    /// does not have (it plays the file): their latest values, and failed
    /// resets (`None`).
    pub(super) fn carried(&self) -> Vec<((String, String), Option<f64>)> {
        let mut carried = Vec::new();
        for unsaved in self.unsaved() {
            let key = unsaved
                .address
                .parse::<PortAddress>()
                .ok()
                .and_then(|port| control_of(&port));
            let Some(key) = key.filter(|_| unsaved.control) else {
                continue;
            };
            match self.panel.dragged(&key.0, &key.1) {
                Some(drag) => carried.push((key, Some(drag.value))),
                None if self.failed.get(&key).is_some_and(|f| f.reset.is_some()) => {
                    carried.push((key, None));
                }
                None => {}
            }
        }
        carried
    }

    /// Plays the `carried` values again in the new run (or, stopped, keeps
    /// them as the controls' values), so they stay live and unsaved.
    pub(super) fn play_carried(&mut self, carried: Vec<((String, String), Option<f64>)>) {
        for ((instance, parameter), value) in carried {
            let Some(target) = self.panel.target(&instance, &parameter) else {
                self.failed.remove(&(instance, parameter));
                continue;
            };
            match value {
                Some(v) => {
                    let sent =
                        self.send_ramps(target.ports.iter().cloned().map(|p| (p, v)).collect());
                    self.panel.drag(&instance, &parameter, v, sent);
                }
                None => {
                    let sent = self.send_ramps(
                        target
                            .ports
                            .iter()
                            .cloned()
                            .zip(target.defaults.iter().copied())
                            .collect(),
                    );
                    if let Some(failed) = self.failed.get_mut(&(instance, parameter)) {
                        failed.reset = Some(sent);
                    }
                }
            }
        }
    }

    /// Edits waiting for reports of a stopped run.
    pub(super) fn abandon(&mut self) {
        self.commands.clear();
        for (_, pending) in std::mem::take(&mut self.acks) {
            let ack = json!({"type": "ack", "id": pending.id, "outcome": "abandoned"});
            self.send_text(pending.client, &ack);
        }
    }

    /// ADR 0037 §7: `address` translated through a top-level slot's match
    /// (`/fx.y` → the plan port behind the content's `out`); another
    /// address as it is. A slot port that reaches nothing now (the slot
    /// empty, the port unmatched) is `inactive`.
    pub(super) fn through_slot(&self, address: &str) -> Result<String, Failure> {
        match self.panel.slot_port(address) {
            Some(crate::panel::SlotPort::Port(port)) => Ok(port.to_string()),
            Some(crate::panel::SlotPort::Inactive(why)) => Err(fail(
                "inactive",
                format!("{address} reaches nothing now: {why}"),
            )),
            None => Ok(address.to_owned()),
        }
    }

    /// `address` as an event input of the running plan (`trigger`, `note`).
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
}
