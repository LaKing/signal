//! Project diff (ADR 0006, D17): what a change to a project's files means for
//! a running engine, as edit commands on the flattened plan.

use std::collections::BTreeMap;

use signal_core::{Curve, EventPayload, InstanceAddress, PortAddress, Registry};
use signal_graph::{ExecutionPlan, PlanInstance};

use crate::locate::Location;
use crate::{Diagnostic, Project, Severity};

/// A new base value for one parameter of the flattened plan, sent as an
/// external `set` (or `ramp`) event.
#[derive(Clone, Debug, PartialEq)]
pub struct ParameterChange {
    /// The parameter.
    pub port: PortAddress,
    /// `Set(value)`, or `Ramp` when [`DiffOptions::ramp_frames`] is given.
    pub payload: EventPayload,
}

/// The edit that turns a running `old` project into `new`.
#[derive(Clone, Debug)]
pub enum ProjectEdit {
    /// Only parameter values changed: one change per parameter whose file
    /// value differs, in address order. Empty when nothing that runs changed
    /// (identical files, or whitespace and comments only).
    Parameters(Vec<ParameterChange>),
    /// Instances, types or connections changed: swap to `plan`. `removed` and
    /// `added` are the instances that do not survive (same address and module
    /// type), for the removal protocol (`docs/semantics.md` §12); a retyped
    /// instance is in both. `sets` are the survivors' parameters whose file
    /// value changed: a survivor keeps its base across a swap.
    Structural {
        /// The new plan. Its `version` is the caller's to assign.
        plan: ExecutionPlan,
        /// Instances of the old plan absent from the new one, in address order.
        removed: Vec<InstanceAddress>,
        /// Instances of the new plan absent from the old one, in address order.
        added: Vec<InstanceAddress>,
        /// Parameter changes of surviving instances.
        sets: Vec<ParameterChange>,
    },
}

impl ProjectEdit {
    /// Whether applying the edit changes nothing.
    pub fn is_empty(&self) -> bool {
        matches!(self, ProjectEdit::Parameters(changes) if changes.is_empty())
    }
}

/// Per-call settings of [`diff_with`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiffOptions {
    /// Move each changed parameter to its new value over this many frames
    /// (linear `ramp` events). `None` or `Some(0)`: a `set` (the default).
    pub ramp_frames: Option<u64>,
}

/// [`diff_with`] with the default options: parameter changes are `set`s.
pub fn diff(
    old: &Project,
    new: &Project,
    registry: &Registry,
) -> Result<ProjectEdit, Vec<Diagnostic>> {
    diff_with(old, new, registry, &DiffOptions::default())
}

/// The edit that turns a running `old` project into `new`, both planned with
/// `registry`.
///
/// A changed sample rate or seed cannot be applied to a running engine: it is
/// an error, located at the key in `new`'s `project.toml`. Errors planning
/// either project are returned as they are.
pub fn diff_with(
    old: &Project,
    new: &Project,
    registry: &Registry,
    options: &DiffOptions,
) -> Result<ProjectEdit, Vec<Diagnostic>> {
    let mut restart = Vec::new();
    let (sample_rate_at, seed_at) = new.settings_at;
    if old.sample_rate != new.sample_rate {
        restart.push(restart_error(
            new,
            sample_rate_at,
            format!(
                "sample_rate changed from {} to {}; a running project cannot change it: restart the project",
                old.sample_rate, new.sample_rate
            ),
        ));
    }
    if old.seed != new.seed {
        restart.push(restart_error(
            new,
            seed_at,
            format!(
                "seed changed from {} to {}; a running project cannot change it: restart the project",
                old.seed, new.seed
            ),
        ));
    }
    if !restart.is_empty() {
        return Err(restart);
    }

    let before = old.plan(registry)?.plan;
    let after = new.plan(registry)?.plan;
    // M2.7-2: placement is decided when the project starts.
    let (old_pins, new_pins) = (old.gpu_pins(&before), new.gpu_pins(&after));
    let before_addresses: BTreeMap<String, ()> = before
        .instances
        .iter()
        .map(|i| (i.address.to_string(), ()))
        .collect();
    let repinned: Vec<Diagnostic> = after
        .instances
        .iter()
        .filter(|i| {
            let key = i.address.to_string();
            let changed = old_pins.get(&key) != new_pins.get(&key);
            changed && (before_addresses.contains_key(&key) || new_pins.contains_key(&key))
        })
        .map(|i| {
            new.instance_error(
                &i.address,
                format!(
                    "{}: its `gpu` pin changed; a running project keeps its GPU placement: restart the project",
                    i.address
                ),
            )
        })
        .collect();
    if !repinned.is_empty() {
        return Err(repinned);
    }
    let change = |value: f64| match options.ramp_frames {
        Some(frames) if frames > 0 => EventPayload::Ramp {
            to: value,
            frames,
            curve: Curve::Linear,
        },
        _ => EventPayload::Set(value),
    };

    let old_instances: BTreeMap<&InstanceAddress, &PlanInstance> =
        before.instances.iter().map(|i| (&i.address, i)).collect();
    let mut sets = Vec::new();
    let mut added = Vec::new();
    for instance in &after.instances {
        match old_instances.get(&instance.address) {
            Some(previous) if previous.type_name == instance.type_name => {
                for (name, &value) in &instance.parameters {
                    let unchanged = previous
                        .parameters
                        .get(name)
                        .is_some_and(|v| v.to_bits() == value.to_bits());
                    if unchanged {
                        continue;
                    }
                    if let Ok(port) = PortAddress::new(&instance.address, name) {
                        sets.push(ParameterChange {
                            port,
                            payload: change(value),
                        });
                    }
                }
            }
            _ => added.push(instance.address.clone()),
        }
    }
    let new_instances: BTreeMap<&InstanceAddress, &PlanInstance> =
        after.instances.iter().map(|i| (&i.address, i)).collect();
    let removed: Vec<InstanceAddress> = before
        .instances
        .iter()
        .filter(|i| {
            new_instances
                .get(&i.address)
                .is_none_or(|n| n.type_name != i.type_name)
        })
        .map(|i| i.address.clone())
        .collect();

    if added.is_empty() && removed.is_empty() && before.sources == after.sources {
        Ok(ProjectEdit::Parameters(sets))
    } else {
        Ok(ProjectEdit::Structural {
            plan: after,
            removed,
            added,
            sets,
        })
    }
}

fn restart_error(project: &Project, offset: usize, message: String) -> Diagnostic {
    project.diagnostic(Severity::Error, Location { file: 0, offset }, message)
}
