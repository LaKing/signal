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

use crate::panel::{Panel, ParameterView, ProjectWriteError, Target, write_project_all};

/// The values a snapshot saves: every control of `panel` at
/// `/instance.parameter`, with the value `base` gives for it from its
/// instance and view (the host: the live base, as `describe` gives it; the
/// files alone: the view's `base`, the file's value or else the default).
/// A modulated control's value is its base, never the modulated signal.
pub fn values(panel: &Panel, base: impl Fn(&str, &ParameterView) -> f64) -> Values {
    let mut values = Values::new();
    for instance in panel.view(&BTreeMap::new()) {
        for p in instance.parameters.iter().filter(|p| p.editable) {
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
        let to = value.max(target.min).min(target.max);
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
/// written into `project.toml` as one change ([`write_project_all`]); a
/// running Signal applies it through its watch. Values are clamped and
/// gone entries skipped as [`plan`] says.
pub fn restore_files(
    dir: &Path,
    registry: &Registry,
    values: &Values,
) -> Result<Restored, RestoreError> {
    let project = Project::load(dir).map_err(|e| RestoreError::DoesNotLoad(e.to_string()))?;
    let panel = Panel::new(&project, registry);
    let restore = plan(&panel, values);
    let changed: Vec<&Edit> = restore
        .edits
        .iter()
        .filter(|e| {
            panel
                .file_base(&e.instance, &e.parameter)
                .is_none_or(|(base, _)| base.to_bits() != e.value.to_bits())
        })
        .collect();
    let orders: Vec<Vec<&str>> = changed
        .iter()
        .map(|e| e.target.order.iter().map(String::as_str).collect())
        .collect();
    let edits: Vec<(&str, &str, Option<f64>, &[&str])> = changed
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
    if !edits.is_empty() {
        write_project_all(dir, &edits).map_err(RestoreError::Write)?;
    }
    let written = changed.iter().map(|e| e.address.clone()).collect();
    Ok(Restored { restore, written })
}

/// Why [`restore_files`] wrote nothing.
#[derive(Debug)]
pub enum RestoreError {
    /// The project does not load.
    DoesNotLoad(String),
    /// `project.toml` was not written.
    Write(ProjectWriteError),
}

impl std::fmt::Display for RestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RestoreError::DoesNotLoad(message) => f.write_str(message),
            RestoreError::Write(error) => write!(f, "{error}"),
        }
    }
}
