//! Snapshots (ADR 0041): listing, saving, restoring and their transitions.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use signal_core::{Curve, EventPayload, PortAddress};
use signal_engine::Stamp;
use signal_project::Project;
use signal_project::snapshot::{self as snapshots, SNAPSHOTS_FILE, SnapshotError};
use signal_project::transaction::{self, ProjectLock};

use crate::panel::{Panel, ramp_frames, write_project_chosen_locked};
use crate::panel_write::sha256_hex;
use crate::{app, snapshot};

use super::controls::{ControlWrite, control_of};
use super::failure::{Failure, fail, fail_with};
use super::{Host, lines, with};

/// The `snapshots` message of `all` read from `text` (ADR 0041): the base,
/// the snapshots in the person's order (§9), the undo snapshot apart (§7).
pub(super) fn snapshot_list_json(all: &snapshots::Snapshots, text: Option<&str>) -> Value {
    let list: Vec<Value> = all
        .names()
        .iter()
        .filter_map(|name| {
            let values = all.snapshots.get(name)?;
            Some(json!({"name": name, "count": values.len(), "values": values}))
        })
        .collect();
    json!({
        "type": "snapshots",
        "file": SNAPSHOTS_FILE,
        "base": text.map(|t| sha256_hex(t.as_bytes())),
        "snapshots": list,
        "undo": all.undo.is_some(),
        "undoCount": all.undo.as_ref().map_or(0, |u| u.len()),
        "undoValues": all.undo,
        "transitionMs": all.transition_ms,
    })
}

/// A message's transition time under `key` (`transitionMs` on a restore or
/// undo, `ms` on `snapshot.transition`, and only that key): absent `None`;
/// else a whole number of milliseconds from 0 to 60000 (`bad-value`).
pub(super) fn transition_of(message: &Value, key: &str) -> Result<Option<u32>, Failure> {
    match message.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|ms| u32::try_from(ms).ok())
            .filter(|ms| *ms <= snapshots::MAX_TRANSITION_MS)
            .map(Some)
            .ok_or_else(|| {
                fail(
                    "bad-value",
                    format!(
                        "`{key}`: a transition is a whole number of milliseconds from 0 to {}",
                        snapshots::MAX_TRANSITION_MS
                    ),
                )
            }),
    }
}

/// A snapshot message's `name` (ADR 0041): trimmed, 1 to 64 characters.
pub(super) fn snapshot_name(message: &Value) -> Result<String, Failure> {
    let raw = message
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| fail("bad-value", "a snapshot message needs `name` (text)"))?;
    snapshots::name(raw).map_err(snapshot_failure)
}

/// A snapshot write's check of the file's version (`base`).
pub(super) type BaseCheck = Box<dyn FnOnce(Option<&str>) -> Result<(), SnapshotError>>;

/// A snapshot message's `base`: absent takes the file as it is; null asks
/// that there be no `snapshots.toml`; else the SHA-256 it was listed with.
pub(super) fn base_check(message: &Value) -> Result<BaseCheck, Failure> {
    let expected: Option<Option<String>> = match message.get("base") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(base)) => Some(Some(base.clone())),
        Some(_) => return Err(fail("bad-value", "`base` must be a SHA-256 or null")),
    };
    Ok(Box::new(move |text: Option<&str>| {
        let Some(expected) = expected else {
            return Ok(());
        };
        let now = text.map(|t| sha256_hex(t.as_bytes()));
        if now == expected {
            Ok(())
        } else {
            Err(SnapshotError::Changed(format!(
                "{SNAPSHOTS_FILE} changed since it was listed (base); nothing was written"
            )))
        }
    }))
}

pub(super) fn snapshot_failure(error: SnapshotError) -> Failure {
    fail(error.code(), error.to_string())
}

impl Host {
    /// The snapshots of the project (ADR 0041), as `snapshot.list` answers
    /// and every write pushes them: the file's `base` and each snapshot's
    /// name and values, in name order.
    fn snapshots_json(&self) -> Result<Value, Failure> {
        let (all, text) = snapshots::read(&self.dir).map_err(snapshot_failure)?;
        Ok(snapshot_list_json(&all, text.as_deref()))
    }

    /// A snapshot message's project: refused in an app with no project
    /// open, and (`plans`) when the project does not load and plan. With
    /// `plans` and nothing playing, the panel is first built again from the
    /// folder: stopped, nothing else follows an edit of the files (the watch
    /// pushes `graph` only), so a save or restore would work on old values.
    fn snapshot_project(&mut self, plans: bool) -> Result<(), Failure> {
        if self.app.as_ref().is_some_and(|m| !m.opened) {
            return Err(fail("no-project", "open a project folder first"));
        }
        if plans && self.run.is_none() {
            match app::load(&self.dir, &self.registry) {
                Ok((project, plan, _)) => {
                    let panel = Panel::new(&project, &self.registry);
                    self.reconcile(panel);
                    self.plan = Some(plan);
                }
                Err(diagnostics) => {
                    return Err(fail(
                        "does-not-load",
                        format!(
                            "the project does not load, so it has no parameters:\n{}",
                            diagnostics.join("\n")
                        ),
                    ));
                }
            }
        }
        if plans && self.plan.is_none() {
            return Err(fail(
                "does-not-load",
                "the project does not load, so it has no parameters",
            ));
        }
        Ok(())
    }

    /// `snapshot.list`.
    pub(super) fn snapshot_list(&mut self) -> Result<Value, Failure> {
        self.snapshot_project(false)?;
        self.snapshots_json()
    }

    /// After a write of `snapshots.toml`: the watch takes it as seen (no
    /// reload: nothing that plays changed) and every client gets the list.
    pub(super) fn snapshots_written(&mut self) {
        self.seen(Path::new(SNAPSHOTS_FILE));
        if let Ok(list) = self.snapshots_json() {
            self.broadcast(&list);
        }
    }

    /// `snapshot.save`: every control's live base under `name`.
    pub(super) fn snapshot_save(&mut self, message: &Value) -> Result<Value, Failure> {
        self.snapshot_project(true)?;
        let name = snapshot_name(message)?;
        let overwrite = match message.get("overwrite") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(fail("bad-value", "`overwrite` must be true or false")),
        };
        let check = base_check(message)?;
        let values = snapshot::values(&self.panel, |instance, p| self.control_base(instance, p));
        let count = values.len();
        let (replaced, text, _) = snapshots::save(&self.dir, &name, values, overwrite, check)
            .map_err(|e| self.snapshot_refused(e))?;
        self.snapshots_written();
        Ok(json!({
            "type": "snapshot.saved",
            "name": name,
            "count": count,
            "replaced": replaced,
            // The file as this call left it (written, or read under the
            // lock when it already held this), never a later read.
            "base": sha256_hex(text.as_bytes()),
        }))
    }

    /// `snapshot.delete`.
    pub(super) fn snapshot_delete(&mut self, message: &Value) -> Result<Value, Failure> {
        self.snapshot_project(false)?;
        let name = snapshot_name(message)?;
        let check = base_check(message)?;
        let (values, text, _) =
            snapshots::delete(&self.dir, &name, check).map_err(|e| self.snapshot_refused(e))?;
        self.snapshots_written();
        Ok(json!({
            "type": "snapshot.deleted",
            "name": name,
            "count": values.len(),
            "base": sha256_hex(text.as_bytes()),
        }))
    }

    /// `snapshot.move` (ADR 0041 §9): the snapshot `name` to place `to`
    /// (0-based) in the person's order; an index past the end is the last
    /// place. Written as `order` in `snapshots.toml` by the ADR 0040 writer,
    /// with the optional `base`; the reply carries the new list.
    pub(super) fn snapshot_move(&mut self, message: &Value) -> Result<Value, Failure> {
        self.snapshot_project(false)?;
        let name = snapshot_name(message)?;
        let to = message
            .get("to")
            .and_then(Value::as_u64)
            .ok_or_else(|| fail("bad-value", "`to` must be a place, a whole number from 0"))?;
        let to = usize::try_from(to).unwrap_or(usize::MAX);
        let check = base_check(message)?;
        let (index, text, _) = snapshots::move_to(&self.dir, &name, to, check)
            .map_err(|e| self.snapshot_refused(e))?;
        self.snapshots_written();
        let all = snapshots::parse(&text).map_err(snapshot_failure)?;
        // The list as written, with what moved.
        let list = snapshot_list_json(&all, Some(&text));
        let list = with(list, "type", json!("snapshot.moved"));
        let list = with(list, "name", json!(name));
        Ok(with(list, "index", json!(index)))
    }

    /// `snapshot.restore`: the snapshot's values written into `project.toml`
    /// as one change and sent live, as `commit` writes and sends one value;
    /// entries that are gone are skipped, values out of range clamped, and
    /// both reported.
    pub(super) fn snapshot_restore(&mut self, message: &Value) -> Result<Value, Failure> {
        self.snapshot_project(true)?;
        let name = snapshot_name(message)?;
        let check = base_check(message)?;
        let transition = transition_of(message, "transitionMs")?;
        let wanted = name.clone();
        self.restore_from(
            check,
            move |all| {
                all.snapshots
                    .get(&wanted)
                    .cloned()
                    .ok_or(SnapshotError::Unknown(wanted))
            },
            &name,
            "snapshot.restored",
            transition,
        )
    }

    /// `snapshot.undo` (ADR 0041 §7): the undo snapshot restored, after the
    /// values playing now take its place, so a second undo redoes.
    pub(super) fn snapshot_undo(&mut self, message: &Value) -> Result<Value, Failure> {
        self.snapshot_project(true)?;
        let check = base_check(message)?;
        let transition = transition_of(message, "transitionMs")?;
        self.restore_from(
            check,
            |all| all.undo.take().ok_or(SnapshotError::NoUndo),
            "undo",
            "snapshot.undone",
            transition,
        )
    }

    /// `snapshot.transition` (ADR 0041 §10): the snapshot transition time,
    /// `ms` from 0 to 60000, written as `transition_ms` in `snapshots.toml`
    /// by the ADR 0040 writer with the optional `base`.
    pub(super) fn snapshot_transition(&mut self, message: &Value) -> Result<Value, Failure> {
        self.snapshot_project(false)?;
        let ms = transition_of(message, "ms")?
            .ok_or_else(|| fail("bad-value", "`ms` must be a whole number of milliseconds"))?;
        let check = base_check(message)?;
        let (text, _) = snapshots::set_transition(&self.dir, ms, check)
            .map_err(|e| self.snapshot_refused(e))?;
        self.snapshots_written();
        Ok(json!({
            "type": "snapshot.transition",
            "ms": ms,
            "base": sha256_hex(text.as_bytes()),
        }))
    }

    /// A control's live base by name ([`Host::control_base`]): the last value
    /// the Controller took, each port's default after a failed reset, else
    /// the file's value.
    fn live_base(&self, instance: &str, parameter: &str) -> f64 {
        let key = (instance.to_owned(), parameter.to_owned());
        let reset = self
            .live
            .failed
            .get(&key)
            .is_some_and(|f| f.reset.is_some());
        let default = self
            .panel
            .target(instance, parameter)
            .and_then(|t| t.defaults.first().copied());
        match (self.panel.dragged(instance, parameter), default) {
            (Some(drag), _) => drag.applied,
            (None, Some(default)) if reset => default,
            _ => self
                .panel
                .file_base(instance, parameter)
                .map_or(0.0, |(base, _)| base),
        }
    }

    /// Restores what `take` gives from the snapshots (ADR 0041 §3, §7), all
    /// under one writer lock: first the values playing now of the controls
    /// this restore can change (the entries that exist and go into
    /// snapshots) become the undo snapshot, in the write that reads the
    /// snapshot (an empty set leaves the undo snapshot as it was; a write
    /// that fails or is a `conflict` restores nothing); then `project.toml`
    /// is written. If that write fails, `snapshots.toml` is put back
    /// (compared), so the undo snapshot from before is kept.
    fn restore_from(
        &mut self,
        check: BaseCheck,
        take: impl FnOnce(&mut snapshots::Snapshots) -> Result<snapshots::Values, SnapshotError>,
        name: &str,
        kind: &str,
        transition: Option<u32>,
    ) -> Result<Value, Failure> {
        let lock = ProjectLock::acquire(&self.dir).map_err(|e| {
            fail(
                "write-failed",
                format!(
                    "{} was not written: {e}",
                    self.dir.join(SNAPSHOTS_FILE).display()
                ),
            )
        })?;
        let edited = snapshots::edit_locked(&self.dir, &lock, check, |all| {
            let before = all.undo.clone();
            let values = take(all)?;
            let undo = snapshot::undo_of(&self.panel, &values, |i, p| self.live_base(i, p));
            all.undo = if undo.is_empty() { before } else { Some(undo) };
            Ok((values, all.transition_ms))
        })
        .map_err(|e| self.snapshot_refused(e))?;
        if edited.wrote {
            self.snapshots_written();
        }
        let undo = edited
            .wrote
            .then(|| {
                edited
                    .before
                    .clone()
                    .map(|before| (edited.after.clone(), before))
            })
            .flatten();
        let (values, file_ms) = &edited.result;
        // The request's `transitionMs`, else the file's.
        let ms = transition.unwrap_or(*file_ms);
        self.apply_snapshot(name, kind, values, lock, undo, ms)
    }

    /// The ramp each port of `control` moves to `value` along (ADR 0041
    /// §10): with no transition (`ms` 0) the panel's 5 ms, as before; a
    /// stepped control ([`crate::panel::stepped`]: one with a value set, ADR
    /// 0043) switches at once (0 frames, at the start); every
    /// other control moves linearly over `ms`, from the value playing then
    /// (a ramp still running included: `semantics.md` "Ramp values").
    fn transition_frames(&self, control: &ControlWrite, value: f64, ms: u32) -> u64 {
        if ms == 0 {
            return ramp_frames(self.rate);
        }
        // ADR 0043: a value set's values only, so it never passes between.
        let _ = value;
        if control.target.stepped {
            return 0;
        }
        (f64::from(self.rate) * f64::from(ms) / 1000.0).round() as u64
    }

    /// Sends `value` to every port of `control` over `frames` (a linear
    /// ramp; 0: at once): whether the Controller took every one (true when
    /// nothing plays).
    fn send_over(&mut self, control: &ControlWrite, value: f64, frames: u64) -> bool {
        let Some(run) = self.run.as_mut() else {
            return true;
        };
        for port in &control.target.ports {
            let payload = EventPayload::Ramp {
                to: value,
                frames,
                curve: Curve::Linear,
            };
            if run
                .controller
                .send_event(port.clone(), payload, Stamp::Default)
                .is_err()
            {
                return false;
            }
        }
        true
    }

    /// Restores `values` (the snapshot `name`, or the undo snapshot) into the
    /// controls: written where `commit` writes them, as one change, under
    /// the held `lock`, and sent live; the reply of type `kind`. `undo` is
    /// the undo snapshot's write (the text written, the text before). When
    /// `project.toml` cannot be written, the values are still sent; the new
    /// undo snapshot is kept when any of them reached the engine (it holds
    /// what played before, which nothing else holds now: the values may not
    /// have been saved), and put back to the one before only when none did.
    /// The reply's `undoSnapshot` says which: `new`, `previous`,
    /// `unchanged` (none was written) or `changed` (another writer changed
    /// the file meanwhile; left as it is).
    fn apply_snapshot(
        &mut self,
        name: &str,
        kind: &str,
        values: &snapshots::Values,
        lock: ProjectLock,
        undo: Option<(String, String)>,
        transition_ms: u32,
    ) -> Result<Value, Failure> {
        let restore = snapshot::plan(&self.panel, values);
        let skipped: Vec<Value> = restore
            .skipped
            .iter()
            .map(|s| json!({"address": s.address, "reason": s.reason}))
            .collect();
        let clamped: Vec<Value> = restore
            .clamped
            .iter()
            .map(|c| json!({"address": c.address, "value": c.value, "to": c.to}))
            .collect();
        let controls: Vec<ControlWrite> = restore
            .edits
            .iter()
            .map(|edit| ControlWrite {
                address: edit.address.clone(),
                instance: edit.instance.clone(),
                parameter: edit.parameter.clone(),
                target: edit.target.clone(),
                value: Some(edit.value),
            })
            .collect();
        // What is written is decided on `project.toml` as read under the
        // writer lock: each value it does not hold.
        let orders: Vec<Vec<&str>> = restore.edits.iter().map(snapshot::Edit::order).collect();
        let texts: Vec<(&str, &str, Option<f64>, &[&str])> = restore
            .edits
            .iter()
            .zip(&orders)
            .map(|(e, order)| {
                (
                    e.instance.as_str(),
                    e.parameter.as_str(),
                    Some(e.value),
                    order.as_slice(),
                )
            })
            .collect();
        let chosen = write_project_chosen_locked(&self.dir, &lock, &texts, &|text, i| {
            restore.edits.get(i).is_some_and(|e| !e.held_by(text))
        });
        // A control whose file holds the value is only sent, and only when
        // its live value may differ (a drag, a failed write or reset, ports
        // in doubt).
        let settled = |host: &Host, c: &ControlWrite| {
            let key = (c.instance.clone(), c.parameter.clone());
            host.panel.dragged(&c.instance, &c.parameter).is_none()
                && !host.live.resend.contains(&key)
                && !host.live.failed.contains_key(&key)
        };
        let (writes, sends, written) = match chosen {
            Ok((before, after, kept)) => {
                let (writes, rest): (Vec<_>, Vec<_>) = controls
                    .into_iter()
                    .enumerate()
                    .partition(|(i, _)| kept.contains(i));
                let writes: Vec<ControlWrite> = writes.into_iter().map(|(_, c)| c).collect();
                let sends: Vec<ControlWrite> = rest
                    .into_iter()
                    .map(|(_, c)| c)
                    .filter(|c| !settled(self, c))
                    .collect();
                (writes, sends, Ok((before, after)))
            }
            Err(error) => {
                // Nothing written: every value is still sent, and each one
                // the file did not hold stays live and unsaved with the
                // error, as after a failed commit.
                let (writes, rest): (Vec<_>, Vec<_>) = controls.into_iter().partition(|c| {
                    self.panel
                        .file_base(&c.instance, &c.parameter)
                        .is_none_or(|(base, _)| Some(base.to_bits()) != c.value.map(f64::to_bits))
                });
                let sends = rest.into_iter().filter(|c| !settled(self, c)).collect();
                (writes, sends, Err(error))
            }
        };
        // ADR 0041 §10: each value moves over the transition (a stepped one
        // switches at its start); a value every port already took is not
        // sent again.
        let mut sent_writes = Vec::with_capacity(writes.len());
        for control in &writes {
            let value = control.value.unwrap_or_default();
            let last = self.panel.dragged(&control.instance, &control.parameter);
            let taken = match last {
                Some(d) if d.sent && d.value.to_bits() == value.to_bits() => true,
                _ => {
                    let frames = self.transition_frames(control, value, transition_ms);
                    self.send_over(control, value, frames)
                }
            };
            sent_writes.push(taken);
        }
        let mut sent = sent_writes.iter().all(|s| *s);
        let mut note = None;
        let failure = match written {
            Ok((before, after)) => {
                if !writes.is_empty() {
                    note = self.controls_written(&writes, &sent_writes, &before, after);
                }
                None
            }
            Err(error) => {
                self.controls_unwritten(&writes, &sent_writes, &error.to_string());
                Some(error)
            }
        };
        // Whether any value reached an engine (stopped, nothing plays).
        let playing = self.run.is_some();
        let mut reached = playing && sent_writes.iter().any(|s| *s);
        for control in &sends {
            let value = control.value.unwrap_or_default();
            let frames = self.transition_frames(control, value, transition_ms);
            let taken = self.send_over(control, value, frames);
            sent &= taken;
            reached |= playing && taken;
            if taken {
                // Live and file agree again: nothing is unsaved.
                self.panel.release(&control.instance, &control.parameter);
                let key = (control.instance.clone(), control.parameter.clone());
                self.live.failed.remove(&key);
                self.live.resend.remove(&key);
            } else {
                self.panel
                    .drag(&control.instance, &control.parameter, value, false);
            }
        }
        // ADR 0041 §7: which undo snapshot stays, decided under the lock.
        let mut undo_note = None;
        let undo_state = match (&failure, &undo) {
            (_, None) => "unchanged",
            (None, Some(_)) => "new",
            // Something played: the new undo snapshot holds what played
            // before the restore, which is what an undo must bring back.
            (Some(_), Some(_)) if reached => "new",
            (Some(_), Some((written, before))) => {
                match snapshots::put_back(&self.dir, &lock, written, before) {
                    Ok(true) => "previous",
                    Ok(false) => {
                        undo_note = Some(format!(
                            "{SNAPSHOTS_FILE} changed meanwhile (another writer); its undo snapshot was left as it is"
                        ));
                        "changed"
                    }
                    Err(error) => {
                        undo_note = Some(format!(
                            "the undo snapshot from before could not be put back: {error}"
                        ));
                        "new"
                    }
                }
            }
        };
        drop(lock);
        if undo_state == "previous" {
            self.snapshots_written();
        }
        // Pushed once every value is sent and written, so it shows them all.
        if !writes.is_empty() || !sends.is_empty() {
            let describe = self.describe(false);
            self.broadcast(&describe);
        }
        let wrote = failure.is_none() && !writes.is_empty();
        if wrote {
            // `graph.files` names the written file's new SHA-256.
            self.push_graph(false);
        }
        let addresses: Vec<&str> = writes.iter().map(|w| w.address.as_str()).collect();
        if let Some(error) = failure {
            let undo_said = match undo_state {
                "new" => "the undo snapshot holds what played before",
                "previous" => "nothing played, so the undo snapshot is the one from before",
                "changed" => "the undo snapshot is as another writer left it",
                _ => "the undo snapshot did not change",
            };
            let tail = undo_note.map(|n| format!("; {n}")).unwrap_or_default();
            return Err(fail_with(
                error.code(),
                format!("{error}; the values were sent live and stay unsaved; {undo_said}{tail}"),
                json!({
                    "name": name,
                    "restored": restore.edits.len(),
                    "undoSnapshot": undo_state,
                    "unwritten": addresses,
                    "sent": sent,
                    "skipped": skipped,
                    "clamped": clamped,
                }),
            ));
        }
        Ok(json!({
            "type": kind,
            "name": name,
            "restored": restore.edits.len(),
            "written": addresses,
            "file": signal_project::PROJECT_FILE,
            "sent": sent,
            "skipped": skipped,
            "clamped": clamped,
            "undoSnapshot": undo_state,
            "transitionMs": transition_ms,
            "note": note,
        }))
    }

    /// `snapshot.exclude` (ADR 0041 §8): a control left out of snapshots
    /// (`exclude: true`) or put back in, written as its name in the
    /// instance's `snapshot_exclude` in `project.toml` (under the writer
    /// lock, compared just before the rename: ADR 0040). Not a parameter
    /// value: nothing is sent and nothing plays differently.
    pub(super) fn snapshot_exclude(&mut self, message: &Value) -> Result<Value, Failure> {
        use signal_project::transaction::replace_unchanged;
        self.snapshot_project(false)?;
        let address = message.get("address").and_then(Value::as_str).unwrap_or("");
        let port: PortAddress = address
            .parse()
            .map_err(|e| fail("bad-address", format!("{address}: {e}")))?;
        let exclude = match message.get("exclude") {
            None => true,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(fail("bad-value", "`exclude` must be true or false")),
        };
        let not_editable = || {
            fail(
                "not-editable",
                format!(
                    "{address} is no control: only a top-level instance's parameters go into snapshots"
                ),
            )
        };
        let (instance, parameter) = control_of(&port).ok_or_else(not_editable)?;
        let path = self.dir.join(signal_project::PROJECT_FILE);
        let failed = |error: String| {
            fail(
                "write-failed",
                format!("{address} was not written to {}: {error}", path.display()),
            )
        };
        let lock = ProjectLock::acquire(&self.dir).map_err(|e| failed(e.to_string()))?;
        let before = fs::read_to_string(&path).map_err(|e| failed(e.to_string()))?;
        let after = signal_project::write_snapshot_exclude(&before, &instance, &parameter, exclude)
            .map_err(|e| failed(e.to_string()))?;
        // Checked on the text read under the lock, as it would be left: a
        // control of the project as it plans then (never a stale panel's).
        let text = after.as_deref().unwrap_or(&before);
        let checked = signal_project::read_sources(&self.dir)
            .map_err(|e| fail("does-not-load", e.to_string()))
            .and_then(|mut files| {
                for file in files
                    .iter_mut()
                    .filter(|f| f.path == signal_project::PROJECT_FILE)
                {
                    text.clone_into(&mut file.text);
                }
                let project = Project::from_sources_in(files, &self.dir)
                    .map_err(|d| fail("does-not-load", lines(&d)))?;
                let planned = project.plan(&self.registry);
                Ok((project, planned))
            })?;
        let (project_after, planned) = checked;
        if let Err(diagnostics) = planned {
            return Err(fail(
                "does-not-load",
                format!(
                    "{address}: the project would not plan, so nothing was written:\n{}",
                    lines(&diagnostics)
                ),
            ));
        }
        // Putting a name back in needs no control: a stale one (no longer a
        // parameter, which stops the project planning) is removed so, as long
        // as the text plans afterwards.
        if exclude
            && Panel::new(&project_after, &self.registry)
                .target(&instance, &parameter)
                .is_none()
        {
            return Err(not_editable());
        }
        let written = match &after {
            Some(after) => {
                if !replace_unchanged(&path, Some(before.as_bytes()), after.as_bytes())
                    .map_err(|e| failed(e.to_string()))?
                {
                    return Err(fail(
                        "conflict",
                        format!(
                            "{address} was not written: {} changed meanwhile (another writer); nothing was written",
                            signal_project::PROJECT_FILE
                        ),
                    ));
                }
                true
            }
            None => false,
        };
        drop(lock);
        if let Some(after) = after {
            self.writes += 1;
            let file = Path::new(signal_project::PROJECT_FILE);
            // Whether the reload of the folder will push what changed instead.
            let mut deferred = false;
            match self.run.as_mut() {
                // The running project takes the same edit of its own text
                // (not the file's, which may hold an edit the watch has not
                // applied yet), so the reload applies exactly what it would
                // have; only when the file was the running project's text
                // is the write taken as seen.
                Some(run) => {
                    let known = run.file.as_deref() == Some(before.as_str());
                    let own = run
                        .current
                        .files
                        .iter()
                        .find(|f| f.path == signal_project::PROJECT_FILE)
                        .map(|f| f.text.clone())
                        .unwrap_or_default();
                    let edited = signal_project::write_snapshot_exclude(
                        &own, &instance, &parameter, exclude,
                    );
                    let project = match edited {
                        Ok(Some(text)) => run.current.with_project_text(&text).map(Some),
                        Ok(None) => Ok(None),
                        Err(error) => Err(error),
                    };
                    match project {
                        Ok(project) => {
                            if let Some(project) = project {
                                run.current = project;
                            }
                            let panel = Panel::new(&run.current, &self.registry);
                            if known {
                                run.file = Some(after);
                                if let Some(watcher) = run.watcher.as_mut() {
                                    let stamp = watcher.stamp(file);
                                    watcher.accept(file, stamp);
                                }
                            }
                            self.reconcile(panel);
                        }
                        // A project with includes (or a text the running
                        // project cannot take): it is not rebuilt (the reload
                        // applies the file; an exclusion changes nothing it
                        // plays, so it pushes nothing), and the panel is taken
                        // from the project as checked above.
                        Err(_) => deferred = true,
                    }
                }
                None => {
                    if let Ok((project, plan, _)) = app::load(&self.dir, &self.registry) {
                        self.reconcile(Panel::new(&project, &self.registry));
                        self.plan = Some(plan);
                    }
                }
            }
            if deferred {
                self.reconcile(Panel::new(&project_after, &self.registry));
            }
            let describe = self.describe(false);
            self.broadcast(&describe);
            self.push_graph(false);
        }
        Ok(json!({
            "type": "snapshot.excluded",
            "address": address,
            "exclude": exclude,
            "inSnapshots": !exclude,
            "file": signal_project::PROJECT_FILE,
            "written": written,
        }))
    }

    /// The SHA-256 of `snapshots.toml` now, `None` without one.
    fn snapshot_base(&self) -> Option<String> {
        let path = self.dir.join(SNAPSHOTS_FILE);
        // A link is never read (its target's bytes are not the file's).
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return None;
        }
        transaction::current(&path)
            .ok()
            .flatten()
            .map(|bytes| sha256_hex(&bytes))
    }

    /// A refused snapshot write: a `conflict` carries the file's base now.
    fn snapshot_refused(&self, error: SnapshotError) -> Failure {
        match error {
            SnapshotError::Changed(message) => {
                fail_with("conflict", message, json!({"base": self.snapshot_base()}))
            }
            error => snapshot_failure(error),
        }
    }
}
