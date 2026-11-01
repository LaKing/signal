//! Control writes: a control's value written into the project's files, and
//! its corrections.

use std::fs;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use signal_core::{Frame, PortAddress};
use signal_engine::Stamp;
use signal_graph::ExecutionPlan;
use signal_project::transaction::{self, ProjectLock};

use crate::panel::{
    ParameterView, Target, WriteEdit, Written, after_write_all, write_project_all,
    write_project_chosen,
};

use super::Host;
use super::failure::{Failure, fail};
use super::live::Failed;

/// The top-level instance and parameter of a control address `/<instance>.<parameter>`.
pub(super) fn control_of(port: &PortAddress) -> Option<(String, String)> {
    let instance = port.instance();
    let name = instance.as_str().strip_prefix('/')?;
    (!name.contains('/')).then(|| (name.to_owned(), port.port().to_owned()))
}

/// The base of plan parameter `port` as the plan was built (its files'
/// value, or its default).
pub(super) fn plan_base(plan: Option<&ExecutionPlan>, port: &PortAddress) -> Option<f64> {
    let instance = plan?
        .instances
        .iter()
        .find(|i| i.address == port.instance())?;
    let descriptor = instance
        .descriptor
        .parameters
        .iter()
        .find(|p| p.name == port.port())?;
    Some(
        instance
            .parameters
            .get(descriptor.name)
            .copied()
            .unwrap_or(descriptor.default),
    )
}

/// A file value as compared: its bits, and whether the file sets it.
pub(super) fn bits(base: Option<(f64, bool)>) -> Option<(u64, bool)> {
    base.map(|(value, in_file)| (value.to_bits(), in_file))
}

/// Controls written as one change ([`Host::write_controls`]).
pub(super) struct ControlWrite {
    pub(super) address: String,
    pub(super) instance: String,
    pub(super) parameter: String,
    pub(super) target: Target,
    pub(super) value: Option<f64>,
}

/// What [`Host::write_controls`] did: whether each value's live send was
/// taken, and a note on the running project.
pub(super) struct ControlsWritten {
    pub(super) sent: Vec<bool>,
    pub(super) note: Option<String>,
}

impl Host {
    /// App mode, ADR 0042, 0043: the warnings of the values the open
    /// project's files hold outside their range or value set, as the plan
    /// the panel was built from corrected them, located (`file:line:column:
    /// warning: …`), then how many of them the next Save writes corrected
    /// (`project.toml`'s; a composite's file is edited by hand). Called
    /// whenever the panel is rebuilt (opened, played, reloaded, written).
    pub(super) fn refresh_warnings(&mut self) {
        let mut warnings: Vec<String> =
            signal_project::located(&self.opening.dir, self.panel.corrected_warnings().to_vec())
                .iter()
                .map(ToString::to_string)
                .collect();
        let count = self.panel.corrections().len();
        if count > 0 {
            let values = if count == 1 { "value" } else { "values" };
            warnings.push(match self.opening.document {
                Some(_) => format!("{count} {values} will be written corrected on the next Save"),
                None => format!(
                    "{count} {values} will be written corrected when each is next written (a commit or a restore)"
                ),
            });
        }
        if let Some(mode) = self.app.as_mut() {
            mode.warnings = warnings;
        }
    }

    /// ADR 0042, 0043 (the operator's decision of 2026-10-25): before a
    /// `.signal` project's Save, the controls whose `project.toml` value
    /// plays corrected ("corrected on load") are written with the value
    /// that plays, as one change, so the saved file holds what played. A
    /// folder project is never written by a Save (its files are the user's
    /// own; Save As opens the new file, whose own next Save writes them).
    /// A failure to write them (a read-only file, say) does not stop the
    /// Save: it is kept and said once the file is saved, and the
    /// corrections stay pending. But another writer holding the project's
    /// lock fails the Save (`Err`), unsaved: the corrections could not be
    /// written first, and a Save without them would lose them (ADR 0048).
    /// The queued Saves come here holding the lock; it never waits.
    pub(super) fn correct_before_save(&mut self) -> Result<(), String> {
        if self.opening.document.is_none() {
            return Ok(());
        }
        let _no_wait = transaction::NoWait::enter(Duration::ZERO);
        let holding = match ProjectLock::try_hold(&self.opening.dir) {
            Ok(Some(holding)) => Some(holding),
            Ok(None) if !self.pending_corrections().is_empty() => {
                return Err(
                    "another writer holds the project's lock, so the corrected values could not be written first; nothing was saved, try again"
                        .to_owned(),
                );
            }
            // Nothing to write, or the writer's own `acquire` says why.
            Ok(None) | Err(_) => None,
        };
        let error = self.write_corrections().err();
        drop(holding);
        if let Some(mode) = self.app.as_mut() {
            mode.correction_error = error;
        }
        Ok(())
    }

    /// The corrections a Save would write ([`Host::write_corrections`]),
    /// each with the value it was corrected from.
    pub(super) fn pending_corrections(&self) -> Vec<(ControlWrite, f64)> {
        self.panel
            .corrections()
            .into_iter()
            .filter(|c| self.panel.dragged(&c.instance, &c.parameter).is_none())
            .filter_map(|c| {
                let target = self.panel.target(&c.instance, &c.parameter)?;
                Some((
                    ControlWrite {
                        address: format!("/{}.{}", c.instance, c.parameter),
                        instance: c.instance,
                        parameter: c.parameter,
                        target,
                        value: Some(c.to),
                    },
                    c.from,
                ))
            })
            .collect()
    }

    /// Writes the pending corrections ([`Host::correct_before_save`]),
    /// each only where the file read under the writer lock still holds the
    /// value it was corrected from (another writer's value is kept, as a
    /// snapshot's restore keeps a value the file already has). A control
    /// with a live value of its own (a drag, unsaved) is left to it.
    /// Nothing to write is `Ok`.
    fn write_corrections(&mut self) -> Result<(), String> {
        let pending = self.pending_corrections();
        if pending.is_empty() {
            return Ok(());
        }
        let orders: Vec<Vec<&str>> = pending
            .iter()
            .map(|(e, _)| e.target.order.iter().map(String::as_str).collect())
            .collect();
        let texts: Vec<(&str, &str, Option<f64>, &[&str])> = pending
            .iter()
            .zip(&orders)
            .map(|((e, _), order)| {
                (
                    e.instance.as_str(),
                    e.parameter.as_str(),
                    e.value,
                    order.as_slice(),
                )
            })
            .collect();
        let chosen = write_project_chosen(&self.opening.dir, &texts, &|text, i| {
            pending.get(i).is_some_and(|(e, from)| {
                signal_project::read_parameter(text, &e.instance, &e.parameter)
                    .is_some_and(|v| v.to_bits() == from.to_bits())
            })
        });
        let (before, after, kept) =
            chosen.map_err(|error| format!("the corrected values were not written: {error}"))?;
        if kept.is_empty() {
            return Ok(());
        }
        let edits: Vec<ControlWrite> = kept
            .iter()
            .filter_map(|&i| pending.get(i))
            .map(|(e, _)| ControlWrite {
                address: e.address.clone(),
                instance: e.instance.clone(),
                parameter: e.parameter.clone(),
                target: e.target.clone(),
                value: e.value,
            })
            .collect();
        // What plays already: nothing to send.
        let sent = vec![true; edits.len()];
        self.controls_written(&edits, &sent, &before, after);
        let describe = self.describe(false);
        self.broadcast(&describe);
        self.push_graph(false);
        Ok(())
    }

    /// `commit`: the value into `project.toml` (the panel's write: once,
    /// with toml_edit), live too if it was not the last value sent; the
    /// running project takes the same edit, so the file watch's reload of
    /// the written file applies nothing.
    pub(super) fn commit(&mut self, message: &Value) -> Result<Value, Failure> {
        if message.get("values").is_some() {
            return self.commit_values(message);
        }
        let (instance, parameter, target, value) = self.commit_target(message)?;
        let address = message.get("address").and_then(Value::as_str).unwrap_or("");
        self.commit_control(address, &instance, &parameter, &target, value, false)
    }

    /// The batch form of `commit` (external review, 2026-10-31: Space
    /// starting several transports): `values`, `[{address, value}]`, every
    /// value sent live stamped one frame (`at`, else the default stamp
    /// resolved once) and written into `project.toml` in one write, so
    /// controls started together start on the same frame whenever the
    /// message is delivered.
    fn commit_values(&mut self, message: &Value) -> Result<Value, Failure> {
        if message.get("address").is_some() || message.get("value").is_some() {
            return Err(fail(
                "bad-message",
                "commit takes `address` and `value`, or `values`, not both",
            ));
        }
        let values = message
            .get("values")
            .and_then(Value::as_array)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                fail(
                    "bad-message",
                    "`values` must be a non-empty list of {address, value}",
                )
            })?;
        let mut edits: Vec<ControlWrite> = Vec::with_capacity(values.len());
        for entry in values {
            let (instance, parameter, target, value) = self.commit_target(entry)?;
            let address = entry["address"].as_str().unwrap_or("").to_owned();
            if edits
                .iter()
                .any(|e| e.instance == instance && e.parameter == parameter)
            {
                return Err(fail(
                    "bad-value",
                    format!("{address} is named twice in `values`"),
                ));
            }
            edits.push(ControlWrite {
                address,
                instance,
                parameter,
                target,
                value,
            });
        }
        let frame = match message.get("at") {
            None | Some(Value::Null) => self.run.as_ref().map(|r| r.controller.default_frame()),
            Some(at) => Some(Frame(at.as_u64().ok_or_else(|| {
                fail("bad-value", "`at` must be a frame (a whole number)")
            })?)),
        };
        let stamp = frame.map_or(Stamp::Default, Stamp::At);
        let written = self.write_controls_at(&edits, false, stamp)?;
        let values: Vec<Value> = edits
            .iter()
            .zip(&written.sent)
            .map(|(e, sent)| json!({"address": e.address, "value": e.value, "sent": sent}))
            .collect();
        Ok(json!({
            "type": "committed",
            "values": values,
            "frame": frame.map(|f| f.0),
            "file": signal_project::PROJECT_FILE,
            "writes": self.writes,
            "note": written.note,
        }))
    }

    /// A `commit`'s (or a `values` entry's) control and value, checked.
    fn commit_target(
        &self,
        message: &Value,
    ) -> Result<(String, String, Target, Option<f64>), Failure> {
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
        let value =
            match message.get("value") {
                None => {
                    return Err(fail(
                        "bad-value",
                        "commit needs `value` (null: the default)",
                    ));
                }
                Some(Value::Null) => None,
                Some(v) => Some(target.take(v.as_f64().filter(|v| v.is_finite()).ok_or_else(
                    || fail("bad-value", "`value` must be a finite number or null"),
                )?)),
            };
        Ok((instance, parameter, target, value))
    }

    /// Writes `value` for a control (sent live first unless every port
    /// already took exactly that value): `commit`, and `retry`
    /// (`retrying`: a live send that is refused writes nothing). A failed
    /// write keeps the value as the control's live drag, with the error
    /// (`docs/ui-protocol.md` "Unsaved"); a write keeps the control unsaved
    /// until the reload of the written file sends it, when its live send was
    /// refused.
    fn commit_control(
        &mut self,
        address: &str,
        instance: &str,
        parameter: &str,
        target: &Target,
        value: Option<f64>,
        retrying: bool,
    ) -> Result<Value, Failure> {
        let edit = ControlWrite {
            address: address.to_owned(),
            instance: instance.to_owned(),
            parameter: parameter.to_owned(),
            target: target.clone(),
            value,
        };
        let written = self.write_controls(std::slice::from_ref(&edit), retrying)?;
        Ok(json!({
            "type": "committed",
            "address": address,
            "value": value,
            "file": signal_project::PROJECT_FILE,
            "writes": self.writes,
            "sent": written.sent.first().copied().unwrap_or(true),
            "note": written.note,
        }))
    }

    /// Writes the values of `edits` into `project.toml` as one change (one
    /// read, edit and rename under the writer lock, ADR 0040), each sent live
    /// first unless every port already took exactly that value: `commit`,
    /// `retry` (`retrying`: a live send that is refused writes nothing) and
    /// a snapshot's restore. A failed write keeps each value as its
    /// control's live drag, with the error (`docs/ui-protocol.md`
    /// "Unsaved"); a write keeps a control unsaved until the reload of the
    /// written file sends it, when its live send was refused.
    fn write_controls(
        &mut self,
        edits: &[ControlWrite],
        retrying: bool,
    ) -> Result<ControlsWritten, Failure> {
        self.write_controls_at(edits, retrying, Stamp::Default)
    }

    /// [`Host::write_controls`], every live send stamped `stamp`.
    fn write_controls_at(
        &mut self,
        edits: &[ControlWrite],
        retrying: bool,
        stamp: Stamp,
    ) -> Result<ControlsWritten, Failure> {
        let sent = self.send_controls(edits, stamp);
        if retrying && sent.iter().any(|s| !s) {
            let addresses: Vec<&str> = edits.iter().map(|e| e.address.as_str()).collect();
            let error = format!(
                "{}: the running project did not take the value",
                addresses.join(", ")
            );
            self.controls_unwritten(edits, &sent, &error);
            return Err(fail("refused", format!("{error}; nothing was written")));
        }
        let orders: Vec<Vec<&str>> = edits
            .iter()
            .map(|e| e.target.order.iter().map(String::as_str).collect())
            .collect();
        let texts: Vec<(&str, &str, Option<f64>, &[&str])> = edits
            .iter()
            .zip(&orders)
            .map(|(e, order)| {
                (
                    e.instance.as_str(),
                    e.parameter.as_str(),
                    e.value,
                    order.as_slice(),
                )
            })
            .collect();
        let (before, after) = match write_project_all(&self.opening.dir, &texts) {
            Ok(texts) => texts,
            Err(error) => {
                self.controls_unwritten(edits, &sent, &error.to_string());
                return Err(fail(error.code(), error.to_string()));
            }
        };
        let note = self.controls_written(edits, &sent, &before, after);
        let describe = self.describe(false);
        self.broadcast(&describe);
        // `graph.files` names the written file's new SHA-256.
        self.push_graph(false);
        Ok(ControlsWritten { sent, note })
    }

    /// Sends each value of `edits` live (a 5 ms ramp to every port; `None`:
    /// each port to its own default), unless every port already took
    /// exactly that value: whether the Controller took each. With an
    /// explicit stamp (a batch `commit`'s shared frame) every value is
    /// sent, so none keeps the earlier stamp of a value sent before
    /// (second review, item 4).
    fn send_controls(&mut self, edits: &[ControlWrite], stamp: Stamp) -> Vec<bool> {
        let mut sent = Vec::with_capacity(edits.len());
        let shared = matches!(stamp, Stamp::At(_));
        for edit in edits {
            let last = self
                .panel
                .dragged(&edit.instance, &edit.parameter)
                .filter(|_| !shared);
            let ports = &edit.target.ports;
            sent.push(match edit.value {
                Some(v) => match last {
                    Some(d) if d.sent && d.value.to_bits() == v.to_bits() => true,
                    _ => self.send_ramps_at(ports.iter().cloned().map(|p| (p, v)).collect(), stamp),
                },
                // Each port back to its own default, as the panel's Reset.
                None => self.send_ramps_at(
                    ports
                        .iter()
                        .cloned()
                        .zip(edit.target.defaults.iter().copied())
                        .collect(),
                    stamp,
                ),
            });
        }
        sent
    }

    /// A write of `edits` that failed (`error`): each value stays its
    /// control's live drag, unsaved with the error (`docs/ui-protocol.md`
    /// "Unsaved").
    pub(super) fn controls_unwritten(
        &mut self,
        edits: &[ControlWrite],
        sent: &[bool],
        error: &str,
    ) {
        for (edit, &sent) in edits.iter().zip(sent) {
            match edit.value {
                Some(v) => self.panel.drag(&edit.instance, &edit.parameter, v, sent),
                None => {
                    self.panel.release(&edit.instance, &edit.parameter);
                }
            }
            let seq = self.live.next_failure;
            self.live.next_failure += 1;
            self.live.failed.insert(
                (edit.instance.clone(), edit.parameter.clone()),
                Failed {
                    error: error.to_owned(),
                    seq,
                    reset: edit.value.is_none().then_some(sent),
                },
            );
        }
    }

    /// After `edits` were written into `project.toml` (`before` the text
    /// read, `after` the text written) with their live sends `sent`: the
    /// running project takes the same edit, so the watch's reload applies
    /// nothing twice; each control is saved, or unsaved until the reload
    /// sends it when its live send was refused. A note on the running
    /// project, if any. `describe` and `graph` are the caller's to push.
    pub(super) fn controls_written(
        &mut self,
        edits: &[ControlWrite],
        sent: &[bool],
        before: &str,
        after: String,
    ) -> Option<String> {
        let file = Path::new(signal_project::PROJECT_FILE);
        // The written file, read at once: a later edit differs from it.
        let stamp = self
            .run
            .as_ref()
            .and_then(|r| r.watcher.as_ref())
            .map(|w| w.stamp(file));
        // The running project held exactly the file this write read: any
        // other writer's edit is in `before`, so it is not held.
        let known = self
            .run
            .as_mut()
            .and_then(|r| r.file.take())
            .is_some_and(|text| text == before);
        for edit in edits {
            self.panel.release(&edit.instance, &edit.parameter);
        }
        self.writes += 1;
        let mut note = None;
        // Whether the running project now holds exactly the file: then the
        // watch need not reload the write it would otherwise see.
        let mut held = false;
        let written: Vec<WriteEdit<'_>> = edits
            .iter()
            .zip(sent)
            .map(|(e, &sent)| WriteEdit {
                target: &e.target,
                instance: &e.instance,
                parameter: &e.parameter,
                value: e.value,
                sent,
            })
            .collect();
        let current = self.run.as_mut().map(|r| &mut r.current);
        match after_write_all(current, &self.opening.dir, &written, &self.registry) {
            // The plan too, which `describe` reads the instances' (and so a
            // composite's inner parameters') bases from.
            Written::Taken(panel) => {
                held = sent.iter().all(|s| *s) && known;
                self.reconcile(panel);
                let plan = self
                    .run
                    .as_ref()
                    .and_then(|r| r.current.plan(&self.registry).ok())
                    .map(|p| p.plan);
                self.replace_plan(plan);
            }
            Written::Stopped(Ok((panel, plan))) => {
                self.reconcile(panel);
                self.plan = Some(plan);
            }
            Written::Refused(panel, error) => {
                self.reconcile(panel);
                note = Some(format!(
                    "the running project could not take the refused value ({error}); the next \
                     reload applies what differs from the file"
                ));
            }
            Written::NotTaken(error) => {
                note = Some(format!(
                    "written, but the running project could not take it ({error}); the next \
                     reload applies the file"
                ));
            }
            // The folder's diagnostics, shown by the app (app mode).
            Written::Stopped(Err(diagnostics)) => {
                if let Some(mode) = self.app.as_mut() {
                    mode.diagnostics = diagnostics;
                }
            }
        }
        // The running project now holds `after`. The watch takes the file as
        // seen only when it still is `after` once stamped (no edit came
        // between the write and the stamp; a later one differs from it).
        if held
            && let Some(stamp) = stamp
            && fs::read_to_string(self.opening.dir.join(file)).is_ok_and(|text| text == after)
            && let Some(run) = self.run.as_mut()
        {
            run.file = Some(after);
            if let Some(watcher) = run.watcher.as_mut() {
                watcher.accept(file, stamp);
            }
        }
        for (edit, &sent) in edits.iter().zip(sent) {
            let key = (edit.instance.clone(), edit.parameter.clone());
            self.live.failed.remove(&key);
            if sent {
                self.live.resend.remove(&key);
            } else {
                self.live.resend.insert(key);
            }
        }
        note
    }

    /// A control's live base, as `describe` gives it: the last value the
    /// Controller took for every port (not one it refused; one a runner
    /// refused afterwards still counts), each port's default after a reset,
    /// else the file's value (its default without a key).
    pub(super) fn control_base(&self, instance: &str, p: &ParameterView) -> f64 {
        let reset = self
            .live
            .failed
            .get(&(instance.to_owned(), p.name.clone()))
            .is_some_and(|f| f.reset.is_some());
        match self.panel.dragged(instance, &p.name) {
            Some(drag) => drag.applied,
            None if reset => p.default,
            None => p.base,
        }
    }

    /// `retry`: commits an unsaved control's live value again (the latest
    /// one sent; the file's when only its live send is in doubt, the default
    /// after a failed reset).
    pub(super) fn retry(&mut self, message: &Value) -> Result<Value, Failure> {
        let address = message.get("address").and_then(Value::as_str).unwrap_or("");
        let port: PortAddress = address
            .parse()
            .map_err(|e| fail("bad-address", format!("{address}: {e}")))?;
        let listed = self.unsaved().into_iter().find(|u| u.address == address);
        let Some(unsaved) = listed else {
            return Err(fail("not-unsaved", format!("{address} is not unsaved")));
        };
        let control = control_of(&port)
            .and_then(|(i, p)| self.panel.target(&i, &p).map(|t| (i, p, t)))
            .filter(|_| unsaved.control);
        let Some((instance, parameter, target)) = control else {
            return Err(fail(
                "not-editable",
                format!("{address} is no control: it can be reverted, not written"),
            ));
        };
        let reset = self
            .live
            .failed
            .get(&(instance.clone(), parameter.clone()))
            .is_some_and(|f| f.reset.is_some());
        let value = match self.panel.dragged(&instance, &parameter) {
            Some(drag) => Some(drag.value),
            None if reset => None,
            None => unsaved.in_file.then_some(unsaved.file),
        };
        self.commit_control(address, &instance, &parameter, &target, value, true)
    }
}
