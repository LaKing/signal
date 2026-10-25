//! Snapshots over the parameter panel (ADR 0041): which parameters a
//! snapshot holds, and what a restore writes and skips. The file itself is
//! `signal_project::snapshot`'s.
//!
//! A snapshot holds every **control** of the project: each top-level
//! instance's own parameters (a composite's or an included project's
//! exported ones), the parameters an ordinary edit (`commit`, the panel)
//! writes into `project.toml`. A parameter inside a composite is the
//! composite's text, the same for each of its instances, so it is in no
//! snapshot, as no edit writes it. Used by the host (`snapshot.*` messages)
//! and by `signal snapshot`, which works on the files alone.

use std::collections::BTreeMap;
use std::path::Path;

use signal_core::{PortAddress, Registry};
use signal_project::Project;
use signal_project::snapshot::Values;

use signal_project::snapshot::{self, SnapshotError, Snapshots};
use signal_project::transaction::ProjectLock;

use crate::panel::{Panel, ParameterView, ProjectWriteError, Target, write_project_chosen_locked};

/// The values a snapshot saves: every control of `panel` at
/// `/instance.parameter`, with the value `base` gives for it from its
/// instance and view (the host: the live base, as `describe` gives it; the
/// files alone: the view's `base`, the file's value or else the default).
/// A modulated control's value is its base, never the modulated signal.
pub fn values(panel: &Panel, base: impl Fn(&str, &ParameterView) -> f64) -> Values {
    let mut values = Values::new();
    for instance in panel.view(&BTreeMap::new()) {
        for p in instance
            .parameters
            .iter()
            .filter(|p| p.editable && p.in_snapshots)
        {
            values.insert(
                format!("/{}.{}", instance.name, p.name),
                base(&instance.name, p),
            );
        }
    }
    values
}

/// A control a restore sets.
#[derive(Clone, Debug)]
pub struct Edit {
    /// `/instance.parameter`.
    pub address: String,
    /// The top-level instance.
    pub instance: String,
    /// Its parameter.
    pub parameter: String,
    /// Its ports, range and parameter order.
    pub target: Target,
    /// The snapshot's value, within the range.
    pub value: f64,
}

impl Edit {
    /// The module's parameter order, for where a new key goes.
    pub fn order(&self) -> Vec<&str> {
        self.target.order.iter().map(String::as_str).collect()
    }

    /// Whether `text` (a `project.toml`) already holds this value: the key
    /// with exactly this value, or no key while every port's own default is
    /// this value. Decided on the text a writer read under the lock.
    pub fn held_by(&self, text: &str) -> bool {
        let bits = self.value.to_bits();
        match signal_project::read_parameter(text, &self.instance, &self.parameter) {
            Some(v) => v.to_bits() == bits,
            None => {
                !self.target.defaults.is_empty()
                    && self.target.defaults.iter().all(|d| d.to_bits() == bits)
            }
        }
    }
}

/// An entry a restore skipped, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    /// The entry's address as the file has it.
    pub address: String,
    /// Why: no such instance, no such parameter, no control.
    pub reason: String,
}

/// An entry a restore brought into its control's range, as an edit clamps.
#[derive(Clone, Debug, PartialEq)]
pub struct Clamped {
    /// `/instance.parameter`.
    pub address: String,
    /// The snapshot's value.
    pub value: f64,
    /// The value restored.
    pub to: f64,
}

/// What restoring a snapshot does to `panel`'s controls.
#[derive(Clone, Debug, Default)]
pub struct Restore {
    /// Each entry whose control exists, in address order.
    pub edits: Vec<Edit>,
    /// Each entry whose instance or parameter is gone.
    pub skipped: Vec<Skipped>,
    /// Each entry out of its control's range, clamped.
    pub clamped: Vec<Clamped>,
}

/// The restore of `values` into `panel`'s controls. Controls the snapshot
/// does not name are not touched.
pub fn plan(panel: &Panel, values: &Values) -> Restore {
    let mut restore = Restore::default();
    for (address, value) in values {
        let skip = |reason: String| Skipped {
            address: address.clone(),
            reason,
        };
        let Ok(port) = address.parse::<PortAddress>() else {
            restore
                .skipped
                .push(skip("not a parameter's address".to_owned()));
            continue;
        };
        let parameter = port.port().to_owned();
        let Some(instance) = port
            .instance()
            .as_str()
            .strip_prefix('/')
            .filter(|i| !i.contains('/'))
            .map(str::to_owned)
        else {
            restore.skipped.push(skip(
                "not a top-level instance's parameter (it is set in a composite's file)".to_owned(),
            ));
            continue;
        };
        if !panel.has_instance(&instance) {
            restore
                .skipped
                .push(skip(format!("the project has no instance `{instance}`")));
            continue;
        }
        let Some(target) = panel.target(&instance, &parameter) else {
            restore
                .skipped
                .push(skip(format!("`{instance}` has no parameter `{parameter}`")));
            continue;
        };
        if panel.in_snapshots(&instance, &parameter) == Some(false) {
            restore.skipped.push(skip("excluded".to_owned()));
            continue;
        }
        // ADR 0043: a discrete control's value is snapped to its values.
        let to = target.take(*value);
        if to.to_bits() != value.to_bits() {
            restore.clamped.push(Clamped {
                address: address.clone(),
                value: *value,
                to,
            });
        }
        restore.edits.push(Edit {
            address: address.clone(),
            instance,
            parameter,
            target,
            value: to,
        });
    }
    restore
}

/// What [`restore_files`] did.
#[derive(Clone, Debug, Default)]
pub struct Restored {
    /// The restore.
    pub restore: Restore,
    /// The controls written (the others already held their value).
    pub written: Vec<String>,
}

/// Restores `values` into the project folder `dir` from its files alone
/// (`signal snapshot restore`): every control whose file value differs is
/// written into `project.toml` as one change ([`crate::panel::write_project_chosen`],
/// decided on the text read under the writer lock); a
/// running Signal applies it through its watch. Values are clamped and
/// gone entries skipped as [`plan`] says.
pub fn restore_files(
    dir: &Path,
    registry: &Registry,
    values: &Values,
) -> Result<Restored, RestoreError> {
    let lock = ProjectLock::acquire(dir).map_err(|e| {
        RestoreError::Write(ProjectWriteError::Failed(format!(
            "{} was not written: {e}",
            dir.join(signal_project::PROJECT_FILE).display()
        )))
    })?;
    let panel = planned_panel(dir, registry)?;
    restore_locked(dir, &lock, &panel, values)
}

/// [`restore_files`] by a writer that holds the project's writer `lock`,
/// with `panel` read under it.
fn restore_locked(
    dir: &Path,
    lock: &ProjectLock,
    panel: &Panel,
    values: &Values,
) -> Result<Restored, RestoreError> {
    let restore = plan(panel, values);
    let orders: Vec<Vec<&str>> = restore.edits.iter().map(Edit::order).collect();
    let edits: Vec<(&str, &str, Option<f64>, &[&str])> = restore
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
    let (_, _, kept) = write_project_chosen_locked(dir, lock, &edits, &|text, i| {
        restore.edits.get(i).is_some_and(|e| !e.held_by(text))
    })
    .map_err(RestoreError::Write)?;
    let written = kept
        .iter()
        .filter_map(|&i| restore.edits.get(i).map(|e| e.address.clone()))
        .collect();
    Ok(Restored { restore, written })
}

/// The undo snapshot of a restore of `values` into `panel`'s controls
/// (ADR 0041 §7): the value `base` gives (now) for each control the restore
/// can change, the snapshot's entries that exist and go into snapshots.
pub fn undo_of(panel: &Panel, values: &Values, base: impl Fn(&str, &str) -> f64) -> Values {
    plan(panel, values)
        .edits
        .iter()
        .map(|e| (e.address.clone(), base(&e.instance, &e.parameter)))
        .collect()
}

/// Restores from the files alone what `take` gives from the snapshots (a
/// named snapshot, or the undo snapshot), as `signal snapshot restore` and
/// `undo` do: under one writer lock, the files' values of the controls it
/// can change become the undo snapshot ([`undo_of`]; an empty set leaves
/// the undo snapshot as it was), then `project.toml` is written
/// ([`restore_files`]). If that write fails, `snapshots.toml` is put back
/// (compared), so the undo snapshot from before is not lost.
pub fn restore_files_with(
    dir: &Path,
    registry: &Registry,
    take: impl FnOnce(&mut Snapshots) -> Result<Values, SnapshotError>,
) -> Result<Restored, RestoreError> {
    let lock = ProjectLock::acquire(dir).map_err(|e| {
        RestoreError::Write(ProjectWriteError::Failed(format!(
            "{} was not written: {e}",
            dir.join(signal_project::PROJECT_FILE).display()
        )))
    })?;
    let panel = planned_panel(dir, registry)?;
    let edited = snapshot::edit_locked(
        dir,
        &lock,
        |_| Ok(()),
        |all| {
            let before = all.undo.clone();
            let values = take(all)?;
            let undo = undo_of(&panel, &values, |i, p| {
                panel.file_base(i, p).map_or(0.0, |(base, _)| base)
            });
            all.undo = if undo.is_empty() { before } else { Some(undo) };
            Ok(values)
        },
    )
    .map_err(RestoreError::Snapshot)?;
    match restore_locked(dir, &lock, &panel, &edited.result) {
        Ok(restored) => Ok(restored),
        Err(error) => {
            // Nothing is sent from the files alone: the undo snapshot goes
            // back to the one before, and the error says how that went.
            let undo = match (edited.wrote, &edited.before) {
                (true, Some(before)) => {
                    match snapshot::put_back(dir, &lock, &edited.after, before) {
                        Ok(true) => "the undo snapshot from before was put back".to_owned(),
                        Ok(false) => format!(
                            "{} changed meanwhile (another writer); its undo snapshot was left as it is",
                            snapshot::SNAPSHOTS_FILE
                        ),
                        Err(e) => {
                            format!("the undo snapshot from before could not be put back: {e}")
                        }
                    }
                }
                _ => "the undo snapshot did not change".to_owned(),
            };
            Err(match error {
                RestoreError::Write(e) => {
                    RestoreError::Write(ProjectWriteError::Failed(format!("{e}; {undo}")))
                }
                other => other,
            })
        }
    }
}

/// The panel of the project in `dir` as its files are now, refused
/// ([`RestoreError::DoesNotLoad`]) when it does not load or does not plan:
/// a project that does not plan has no controls, so a save of it would be
/// an empty snapshot (with `--overwrite`, replacing a good one) and a
/// restore would skip every entry.
pub fn planned_panel(dir: &Path, registry: &Registry) -> Result<Panel, RestoreError> {
    let project = Project::load(dir).map_err(|e| RestoreError::DoesNotLoad(e.to_string()))?;
    if let Err(diagnostics) = project.plan(registry) {
        let lines: Vec<String> = diagnostics.iter().map(ToString::to_string).collect();
        return Err(RestoreError::DoesNotLoad(format!(
            "the project does not plan, so it has no parameters:\n{}",
            lines.join("\n")
        )));
    }
    Ok(Panel::new(&project, registry))
}

/// Why [`restore_files`] wrote nothing.
#[derive(Debug)]
pub enum RestoreError {
    /// The project does not load.
    DoesNotLoad(String),
    /// `project.toml` was not written.
    Write(ProjectWriteError),
    /// `snapshots.toml` was not read or written.
    Snapshot(SnapshotError),
}

impl std::fmt::Display for RestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RestoreError::DoesNotLoad(message) => f.write_str(message),
            RestoreError::Write(error) => write!(f, "{error}"),
            RestoreError::Snapshot(error) => write!(f, "{error}"),
        }
    }
}
