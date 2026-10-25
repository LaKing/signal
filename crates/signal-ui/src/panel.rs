//! The parameter panel (GUI b, ADR 0013) without a window: the project's
//! top-level instances and their parameters, built from the project's files
//! and plan, with the effective values the runner reports.
//!
//! Only a top-level instance's own parameters are editable: a primitive's
//! declared parameters, or the parameters a composite exports. Their value
//! is written into that instance's entry in `project.toml`'s `[instances]`.
//! Parameters inside a composite are shown read-only: their values are in
//! the composite's file, and writing one there would change every instance
//! of that composite, not just this one.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use signal_core::{InstanceAddress, ParameterDescriptor, PortAddress, PortShape, Registry};
use signal_engine::ParameterValue;
use signal_graph::{CompositeDef, ExecutionPlan};
use signal_project::Project;
use signal_project::transaction::ProjectLock;

/// The ramp a dragged value moves along, against zipper noise.
pub const RAMP_MS: f64 = 5.0;

/// [`RAMP_MS`] in frames at `rate`.
pub fn ramp_frames(rate: u32) -> u64 {
    (f64::from(rate) * RAMP_MS / 1000.0).round() as u64
}

/// One parameter as the window shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ParameterView {
    /// The parameter's name: on the instance (`kick_decay`), or for one
    /// inside a composite its path below the instance (`kick/amp_env.decay_ms`).
    pub name: String,
    /// Its unit, if any.
    pub unit: Option<&'static str>,
    /// Its range: the slider's ends.
    pub min: f64,
    /// The range's upper end.
    pub max: f64,
    /// The value without a key in the file.
    pub default: f64,
    /// The base: the value being dragged, else the file's, else the default.
    pub base: f64,
    /// Whether `project.toml` sets it (always false inside a composite).
    pub in_file: bool,
    /// The value the module receives (base and modulation, clamped), from
    /// the runner while playing.
    pub effective: Option<f64>,
    /// Whether a connection modulates it, so the effective value differs
    /// from the base.
    pub modulated: bool,
    /// Whether the panel may write it (top-level parameters only).
    pub editable: bool,
    /// ADR 0041 §8: whether it goes into snapshots (not named in its
    /// instance's `snapshot_exclude`); false inside a composite.
    pub in_snapshots: bool,
    /// ADR 0043: its value set (each value and its name), for a discrete
    /// parameter: a selector instead of a slider; `None` for a continuous one.
    pub values: Option<Vec<(f64, String)>>,
}

/// One top-level instance.
#[derive(Clone, Debug, PartialEq)]
pub struct InstanceView {
    /// Its name in `[instances]`.
    pub name: String,
    /// Its module reference.
    pub module: String,
    /// Its own parameters (for a composite, the exported ones), editable.
    pub parameters: Vec<ParameterView>,
    /// For a composite, the parameters of the primitives inside it, read-only.
    pub inner: Vec<ParameterView>,
}

/// A parameter and where its value goes.
#[derive(Clone, Debug)]
struct Row {
    view: ParameterView,
    /// The primitive parameters it sets: one, or an export's fan-out.
    ports: Vec<PortAddress>,
    /// Each port's own value without this parameter in `project.toml`.
    defaults: Vec<f64>,
}

#[derive(Clone, Debug)]
struct Instance {
    name: String,
    module: String,
    /// The module's parameters in declared order, for where a new key goes.
    order: Vec<String>,
    parameters: Vec<Row>,
    inner: Vec<Row>,
}

/// The panel's state: the instances of the project last loaded or written,
/// and the values being dragged.
#[derive(Clone, Debug, Default)]
pub struct Panel {
    instances: Vec<Instance>,
    /// The project's patterns (`patterns/<name>.toml`) and their kinds, for
    /// the pattern controls' check.
    patterns: Vec<(String, &'static str)>,
    /// The values being dragged (each send carries the latest value to all
    /// of a parameter's ports, so only the latest send says what they hold).
    drags: BTreeMap<(String, String), Drag>,
}

/// A parameter's live value that the file may not hold: the latest value
/// sent (dragged, or set by a client), whether the Controller took it for
/// every port, and the last value the Controller took for every port.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag {
    /// The latest value sent.
    pub value: f64,
    /// Whether the Controller took it for every port (and no runner refused
    /// it since).
    pub sent: bool,
    /// The last value the Controller took for every port: `value` when the
    /// Controller took it, else the one before (the file's when there was
    /// none). A runner refusing it afterwards ([`Panel::unsend`]) leaves it.
    /// It is `describe`'s `base`.
    pub applied: f64,
}

impl Panel {
    /// The panel of `project`, planned with `registry`; empty when it does
    /// not plan.
    pub fn new(project: &Project, registry: &Registry) -> Panel {
        let Ok(planned) = project.plan(registry) else {
            return Panel::default();
        };
        let plan = &planned.plan;
        // The project's composites and the library's it uses (`lib.<name>`,
        // ADR 0027 §4): a library composite's exported parameters are
        // controls as a project composite's are (ADR 0033: a panel plays
        // `lib.transport`).
        let composites: BTreeMap<&str, &CompositeDef> = project
            .composites
            .iter()
            .chain(project.library.iter().map(|l| &l.def))
            .map(|c| (c.name.as_str(), c))
            .collect();
        let mut instances = Vec::new();
        for def in &project.root.instances {
            let excluded = project.snapshot_excluded(&def.name);
            let Ok(address) = format!("/{}", def.name).parse::<InstanceAddress>() else {
                continue;
            };
            let mut instance = Instance {
                name: def.name.clone(),
                module: def.module.clone(),
                order: Vec::new(),
                parameters: Vec::new(),
                inner: Vec::new(),
            };
            let names: Vec<String> = match (registry.get(&def.module), composites.get(&*def.module))
            {
                (Some(module), _) => module
                    .descriptor
                    .parameters
                    .iter()
                    .map(|p| p.name.to_owned())
                    .collect(),
                (None, Some(composite)) => composite
                    .exports
                    .parameters
                    .iter()
                    .map(|e| e.name.clone())
                    .collect(),
                (None, None) => Vec::new(),
            };
            for name in &names {
                let targets = resolve(&composites, registry, &address, &def.module, name);
                let Some((_, first, inner_value, _)) = targets.first() else {
                    continue;
                };
                let (min, max) = targets.iter().fold((f64::MIN, f64::MAX), |(lo, hi), t| {
                    (lo.max(t.1.range.min), hi.min(t.1.range.max))
                });
                // ADR 0043: the value set every target shares, if any.
                let values = targets
                    .first()
                    .and_then(|t| t.3.clone())
                    .filter(|first| targets.iter().all(|t| t.3.as_ref() == Some(first)));
                let default = inner_value.unwrap_or(first.default);
                let file = def.parameters.get(name).copied();
                let ports: Vec<PortAddress> = targets.iter().map(|t| t.0.clone()).collect();
                let defaults = targets.iter().map(|t| t.2.unwrap_or(t.1.default)).collect();
                instance.parameters.push(Row {
                    view: ParameterView {
                        name: name.clone(),
                        unit: first.unit,
                        min,
                        max,
                        default,
                        base: file.unwrap_or(default),
                        in_file: file.is_some(),
                        effective: None,
                        modulated: ports.iter().any(|p| modulated(plan, p)),
                        editable: true,
                        in_snapshots: !excluded.contains(name),
                        values,
                    },
                    ports,
                    defaults,
                });
            }
            instance.order = names;
            if registry.get(&def.module).is_none() {
                instance.inner = inner(plan, &address);
            }
            instances.push(instance);
        }
        Panel {
            instances,
            patterns: project
                .patterns()
                .map(|(name, kind)| (name.to_owned(), kind))
                .collect(),
            drags: BTreeMap::new(),
        }
    }

    /// The editable parameter `parameter` of `instance`: its primitive
    /// parameters with their own defaults, and the module's parameter order.
    pub fn target(&self, instance: &str, parameter: &str) -> Option<Target> {
        let i = self.instances.iter().find(|i| i.name == instance)?;
        let row = i.parameters.iter().find(|r| r.view.name == parameter)?;
        Some(Target {
            ports: row.ports.clone(),
            defaults: row.defaults.clone(),
            min: row.view.min,
            max: row.view.max,
            order: i.order.clone(),
            values: row.view.values.clone(),
        })
    }

    /// ADR 0041 §8: whether the control `parameter` of `instance` goes into
    /// snapshots; `None` for no such control.
    pub fn in_snapshots(&self, instance: &str, parameter: &str) -> Option<bool> {
        let i = self.instances.iter().find(|i| i.name == instance)?;
        let row = i.parameters.iter().find(|r| r.view.name == parameter)?;
        Some(row.view.in_snapshots)
    }

    /// Whether the panel has the top-level instance `instance`.
    pub fn has_instance(&self, instance: &str) -> bool {
        self.instances.iter().any(|i| i.name == instance)
    }

    /// Records the value being dragged, and whether the Controller took it.
    pub fn drag(&mut self, instance: &str, parameter: &str, value: f64, sent: bool) {
        let key = (instance.to_owned(), parameter.to_owned());
        let applied = match (sent, self.drags.get(&key)) {
            (true, _) => value,
            (false, Some(before)) => before.applied,
            (false, None) => self
                .file_base(instance, parameter)
                .map_or(value, |(base, _)| base),
        };
        self.drags.insert(
            key,
            Drag {
                value,
                sent,
                applied,
            },
        );
    }

    /// Ends a drag: its value, and whether its last send reached every port.
    pub fn release(&mut self, instance: &str, parameter: &str) -> Option<(f64, bool)> {
        self.drags
            .remove(&(instance.to_owned(), parameter.to_owned()))
            .map(|d| (d.value, d.sent))
    }

    /// The drag of `parameter` of `instance`, if any.
    pub fn dragged(&self, instance: &str, parameter: &str) -> Option<Drag> {
        self.drags
            .get(&(instance.to_owned(), parameter.to_owned()))
            .copied()
    }

    /// Every drag, by instance and parameter.
    pub fn drags(&self) -> impl Iterator<Item = (&(String, String), &Drag)> {
        self.drags.iter()
    }

    /// Puts a drag back as it was (a rebuilt panel keeps it).
    pub fn restore(&mut self, key: (String, String), drag: Drag) {
        self.drags.insert(key, drag);
    }

    /// A runner refused the send of `value` after the Controller took it:
    /// if that is still the latest value, it did not reach the ports.
    pub fn unsend(&mut self, instance: &str, parameter: &str, value: f64) {
        if let Some(drag) = self
            .drags
            .get_mut(&(instance.to_owned(), parameter.to_owned()))
            && drag.value.to_bits() == value.to_bits()
        {
            drag.sent = false;
        }
    }

    /// The file's value of an editable parameter (its default without a
    /// key), and whether the file sets it.
    pub fn file_base(&self, instance: &str, parameter: &str) -> Option<(f64, bool)> {
        let i = self.instances.iter().find(|i| i.name == instance)?;
        let row = i.parameters.iter().find(|r| r.view.name == parameter)?;
        Some((row.view.base, row.view.in_file))
    }

    /// Takes the instances of `other` (a reloaded or written project),
    /// keeping the drags in progress.
    pub fn replace(&mut self, other: Panel) {
        self.instances = other.instances;
        self.patterns = other.patterns;
    }

    /// The kind of the project's pattern `name` (`patterns/<name>.toml`), if
    /// it has one.
    pub fn pattern_kind(&self, name: &str) -> Option<&'static str> {
        self.patterns
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, kind)| *kind)
    }

    /// What the window draws, with the effective `values` the runner reported.
    pub fn view(&self, values: &BTreeMap<PortAddress, ParameterValue>) -> Vec<InstanceView> {
        let row = |row: &Row| ParameterView {
            effective: row
                .ports
                .first()
                .and_then(|p| values.get(p))
                .map(|v| v.effective),
            ..row.view.clone()
        };
        self.instances
            .iter()
            .map(|i| InstanceView {
                name: i.name.clone(),
                module: i.module.clone(),
                parameters: i
                    .parameters
                    .iter()
                    .map(|r| {
                        let mut view = row(r);
                        if let Some(drag) = self.drags.get(&(i.name.clone(), r.view.name.clone())) {
                            view.base = drag.value;
                        }
                        view
                    })
                    .collect(),
                inner: i.inner.iter().map(row).collect(),
            })
            .collect()
    }
}

/// An editable parameter, for sending and writing its value.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    /// The primitive parameters its value is sent to.
    pub ports: Vec<PortAddress>,
    /// Each port's own default (the value without the key in
    /// `project.toml`), in `ports` order: an export's targets can differ.
    pub defaults: Vec<f64>,
    /// Its range.
    pub min: f64,
    /// Its range's upper end.
    pub max: f64,
    /// The module's parameters in declared order.
    pub order: Vec<String>,
    /// ADR 0043: its value set, if it is discrete.
    pub values: Option<Vec<(f64, String)>>,
}

impl Target {
    /// `value` as this parameter takes it live: clamped to its range and,
    /// with a value set, the nearest of its values (ADR 0043).
    pub fn take(&self, value: f64) -> f64 {
        let clamped = value.max(self.min).min(self.max);
        match &self.values {
            Some(values) => nearest(values, clamped),
            None => clamped,
        }
    }

    /// A value in range that no port has once `written` is in the file
    /// (`None`: the key removed, each port at its own default); `None` when
    /// the range is a single value. The running project takes it for a
    /// parameter whose last send some port refused, so that the M2-2 diff
    /// against the written file sends every port the file's value.
    pub fn unlike(&self, written: Option<f64>) -> Option<f64> {
        let taken: Vec<f64> = match written {
            Some(value) => vec![value],
            None => self.defaults.clone(),
        };
        // A discrete parameter takes one of its values that no port has.
        if let Some(values) = &self.values {
            return values
                .iter()
                .map(|(v, _)| *v)
                .find(|v| !taken.iter().any(|t| t.to_bits() == v.to_bits()));
        }
        let steps = taken.len() + 1;
        (0..=steps)
            .map(|k| self.min + (self.max - self.min) * k as f64 / steps as f64)
            .find(|v| v.is_finite() && !taken.iter().any(|t| t.to_bits() == v.to_bits()))
            .filter(|_| self.min < self.max)
    }
}

/// Whether a signal connection modulates `port` in `plan`.
fn modulated(plan: &ExecutionPlan, port: &PortAddress) -> bool {
    plan.sources
        .get(port)
        .is_some_and(|s| s.iter().any(|s| s.shape == PortShape::Signal))
}

/// The primitive parameters `parameter` of the instance at `address` (of
/// `module`) sets, each with its descriptor and the value set for it inside
/// the composites on the way (the innermost file's value, overridden by
/// each enclosing composite's; `None` when only the default applies).
fn resolve(
    composites: &BTreeMap<&str, &CompositeDef>,
    registry: &Registry,
    address: &InstanceAddress,
    module: &str,
    parameter: &str,
) -> Vec<Resolved> {
    if let Some(module) = registry.get(module) {
        let descriptor = module
            .descriptor
            .parameters
            .iter()
            .find(|p| p.name == parameter);
        return match (descriptor, PortAddress::new(address, parameter)) {
            (Some(d), Ok(port)) => vec![(port, *d, None, descriptor_values(d))],
            _ => Vec::new(),
        };
    }
    let Some(composite) = composites.get(module) else {
        return Vec::new();
    };
    let Some(export) = composite
        .exports
        .parameters
        .iter()
        .find(|e| e.name == parameter)
    else {
        return Vec::new();
    };
    let mut targets = Vec::new();
    for to in &export.to {
        let Some((name, inner)) = to.split_once('.') else {
            continue;
        };
        let Some(def) = composite.instances.iter().find(|i| i.name == name) else {
            continue;
        };
        let Ok(child) = address.child(name) else {
            continue;
        };
        let set = def.parameters.get(inner).copied();
        for mut target in resolve(composites, registry, &child, &def.module, inner) {
            target.2 = set.or(target.2);
            // ADR 0042: the export's own range, inside every target's (the
            // plan refuses one that is not), is the control's.
            if let Some(range) = export.range {
                target.1.range = range;
            }
            // ADR 0043: and its own value set (each value allowed by every
            // target), else the target's.
            if let Some(values) = &export.values {
                target.3 = Some(values.iter().map(|v| (v.value, v.label.clone())).collect());
            }
            targets.push(target);
        }
    }
    targets
}

/// A resolved primitive parameter of a control: its port, its descriptor
/// (with an export's own range), the value a composite sets on it, and its
/// value set (an export's own, else the descriptor's).
type Resolved = (
    PortAddress,
    ParameterDescriptor,
    Option<f64>,
    Option<Vec<(f64, String)>>,
);

/// A descriptor's value set as the panel keeps it (ADR 0043).
fn descriptor_values(p: &ParameterDescriptor) -> Option<Vec<(f64, String)>> {
    p.values.map(|values| {
        values
            .iter()
            .map(|v| (v.value, v.label.to_owned()))
            .collect()
    })
}

/// `value` as the nearest of `values` (the lower of two as near); `value`
/// itself for an empty set.
pub fn nearest(values: &[(f64, String)], value: f64) -> f64 {
    let mut best: Option<f64> = None;
    for (v, _) in values {
        if best.is_none_or(|b| (v - value).abs() < (b - value).abs()) {
            best = Some(*v);
        }
    }
    best.unwrap_or(value)
}

/// The parameters of the primitives inside the composite instance at
/// `address`, read-only, with their initial bases from `plan`.
fn inner(plan: &ExecutionPlan, address: &InstanceAddress) -> Vec<Row> {
    let prefix = format!("{}/", address.as_str());
    let mut rows = Vec::new();
    for instance in &plan.instances {
        let Some(path) = instance.address.as_str().strip_prefix(&prefix) else {
            continue;
        };
        for p in instance.descriptor.parameters {
            let Ok(port) = PortAddress::new(&instance.address, p.name) else {
                continue;
            };
            let base = instance
                .parameters
                .get(p.name)
                .copied()
                .unwrap_or(p.default);
            rows.push(Row {
                view: ParameterView {
                    name: format!("{path}.{}", p.name),
                    unit: p.unit,
                    min: p.range.min,
                    max: p.range.max,
                    default: p.default,
                    base,
                    in_file: false,
                    effective: None,
                    modulated: modulated(plan, &port),
                    editable: false,
                    in_snapshots: false,
                    values: descriptor_values(p),
                },
                ports: vec![port],
                defaults: vec![p.default],
            });
        }
    }
    rows
}

/// What the project took after a parameter was written into `project.toml`
/// ([`after_write`]).
pub enum Written {
    /// The running project took the written value: the written project's
    /// panel.
    Taken(Panel),
    /// The written project's panel; the running project could not take the
    /// value no port has (sent after a refused edit), for this reason.
    Refused(Panel, String),
    /// The running project could not take the written value, for this
    /// reason; the next reload applies the file.
    NotTaken(String),
    /// Nothing plays: the folder's panel and plan (whose instances hold the
    /// written values), or its diagnostics when it does not load.
    Stopped(Result<(Panel, ExecutionPlan), Vec<String>>),
}

/// After `value` was written for `parameter` of `instance` into the
/// `project.toml` of `dir` (the panel's release or reset, the protocol's
/// `commit`): the running project `current` takes the same edit, so when the
/// file watch reloads the written file the diff is empty and nothing is
/// applied twice. When the Controller refused it for some port (`sent`
/// false), the ports may hold anything sent before, so the running project
/// takes a value no port has in the file ([`Target::unlike`]): that reload's
/// diff then sends every port the file's value. Stopped, the folder is read
/// again ([`crate::app::load`]).
#[allow(clippy::too_many_arguments)]
pub fn after_write(
    current: Option<&mut Project>,
    dir: &Path,
    target: &Target,
    instance: &str,
    parameter: &str,
    value: Option<f64>,
    sent: bool,
    registry: &Registry,
) -> Written {
    after_write_all(
        current,
        dir,
        &[WriteEdit {
            target,
            instance,
            parameter,
            value,
            sent,
        }],
        registry,
    )
}

/// One parameter written into `project.toml` with others, as one change
/// ([`write_project_all`], [`after_write_all`]): a snapshot's restore.
#[derive(Clone, Copy, Debug)]
pub struct WriteEdit<'a> {
    /// The control.
    pub target: &'a Target,
    /// Its instance.
    pub instance: &'a str,
    /// Its parameter.
    pub parameter: &'a str,
    /// The value written (`None`: the key removed).
    pub value: Option<f64>,
    /// Whether the Controller took its live send for every port.
    pub sent: bool,
}

/// [`after_write`] for several parameters written as one change: the
/// running project takes every edit at once, each one whose live send was
/// refused as a value no port has.
pub fn after_write_all(
    current: Option<&mut Project>,
    dir: &Path,
    edits: &[WriteEdit<'_>],
    registry: &Registry,
) -> Written {
    let Some(current) = current else {
        return Written::Stopped(
            crate::app::load(dir, registry)
                .map(|(project, plan, _)| (Panel::new(&project, registry), plan)),
        );
    };
    let orders: Vec<Vec<&str>> = edits
        .iter()
        .map(|e| e.target.order.iter().map(String::as_str).collect())
        .collect();
    let written: Vec<(&str, &str, Option<f64>, &[&str])> = edits
        .iter()
        .zip(&orders)
        .map(|(e, order)| (e.instance, e.parameter, e.value, order.as_slice()))
        .collect();
    match current.with_parameters(&written) {
        Ok(project) => {
            let panel = Panel::new(&project, registry);
            // Sent, or a single-valued range (every send was that value):
            // the written value; else one no port has.
            let unlikes: Vec<Option<f64>> = edits
                .iter()
                .map(|e| e.target.unlike(e.value).filter(|_| !e.sent))
                .collect();
            let running = if unlikes.iter().all(Option::is_none) {
                Ok(project)
            } else {
                let held: Vec<(&str, &str, Option<f64>, &[&str])> = written
                    .iter()
                    .zip(&unlikes)
                    .map(|(&(i, p, v, o), unlike)| (i, p, unlike.or(v), o))
                    .collect();
                current.with_parameters(&held)
            };
            match running {
                Ok(running) => {
                    *current = running;
                    Written::Taken(panel)
                }
                Err(error) => Written::Refused(panel, error.to_string()),
            }
        }
        Err(error) => Written::NotTaken(error.to_string()),
    }
}

/// Writes `value` for `parameter` of `instance` (`None`: removes the key)
/// into the `project.toml` of `dir` with `toml_edit`
/// ([`write_parameter`](signal_project::write_parameter)), keeping the rest of
/// the file as it was; `order` is the module's parameter order, for where a
/// new key goes. Atomic ([`write_file`]). Gives the file's text before and
/// after.
///
/// The edit is based on the file as read at the start. It is read, edited
/// and written under the project's writer lock (ADR 0040), and the file is
/// read once more just before the rename: one that no longer holds the text
/// the edit was based on is [`ProjectWriteError::Changed`], and nothing is
/// written. Under the lock that catches only a writer that takes none (an
/// editor). Unlocked (a volume without locks) it also catches another
/// Signal writer, except one renaming in the instant between that last read
/// and this rename: without a lock the two are not one step.
pub fn write_project(
    dir: &Path,
    instance: &str,
    parameter: &str,
    value: Option<f64>,
    order: &[&str],
) -> Result<(String, String), ProjectWriteError> {
    write_project_between(
        dir,
        &[(instance, parameter, value, order)],
        &|_, _| true,
        &mut || {},
    )
    .map(|(b, a, _)| (b, a))
}

/// [`write_project`] for several parameters at once, `(instance,
/// parameter, value, order)` each: one edit of the text, read, edited and
/// written under the writer lock as one change, compared just before its
/// rename (a snapshot's restore, ADR 0041). Nothing is written when any edit
/// fails.
pub fn write_project_all(
    dir: &Path,
    edits: &[(&str, &str, Option<f64>, &[&str])],
) -> Result<(String, String), ProjectWriteError> {
    write_project_between(dir, edits, &|_, _| true, &mut || {}).map(|(b, a, _)| (b, a))
}

/// [`write_project_all`] of the edits `keep` takes, decided from the text
/// read under the writer lock (`keep(text, index)`), so the choice is made
/// on the version written over, not on a cached one: a snapshot's restore
/// writes only the values the file does not hold. Gives the texts before and
/// after and the indices written; nothing is written (and the lock is the
/// only step taken) when `keep` takes none.
pub fn write_project_chosen(
    dir: &Path,
    edits: &[(&str, &str, Option<f64>, &[&str])],
    keep: &dyn Fn(&str, usize) -> bool,
) -> Result<(String, String, Vec<usize>), ProjectWriteError> {
    write_project_between(dir, edits, keep, &mut || {})
}

/// [`write_project_all`], running `between` after the edit is made and
/// before the file is read again (a test interleaves another writer there).
fn write_project_between(
    dir: &Path,
    edits: &[(&str, &str, Option<f64>, &[&str])],
    keep: &dyn Fn(&str, usize) -> bool,
    between: &mut dyn FnMut(),
) -> Result<(String, String, Vec<usize>), ProjectWriteError> {
    let lock = ProjectLock::acquire(dir).map_err(|e| {
        ProjectWriteError::Failed(format!(
            "{} not written to {}: {e}",
            what_of(edits),
            dir.join(signal_project::PROJECT_FILE).display()
        ))
    })?;
    write_project_locked(dir, &lock, edits, keep, between)
}

/// "out.gain was", or "3 parameters were", for an error.
fn what_of(edits: &[(&str, &str, Option<f64>, &[&str])]) -> String {
    match edits {
        [(instance, parameter, ..)] => format!("{instance}.{parameter} was"),
        _ => format!("{} parameters were", edits.len()),
    }
}

/// [`write_project_chosen`] by a writer that already holds the project's
/// writer `lock` (ADR 0040: never taken twice), so a restore writes its undo
/// snapshot and `project.toml` under one lock (ADR 0041 §7).
pub fn write_project_chosen_locked(
    dir: &Path,
    lock: &ProjectLock,
    edits: &[(&str, &str, Option<f64>, &[&str])],
    keep: &dyn Fn(&str, usize) -> bool,
) -> Result<(String, String, Vec<usize>), ProjectWriteError> {
    write_project_locked(dir, lock, edits, keep, &mut || {})
}

fn write_project_locked(
    dir: &Path,
    _lock: &ProjectLock,
    edits: &[(&str, &str, Option<f64>, &[&str])],
    keep: &dyn Fn(&str, usize) -> bool,
    between: &mut dyn FnMut(),
) -> Result<(String, String, Vec<usize>), ProjectWriteError> {
    use signal_project::transaction::replace_unchanged;
    let path = dir.join(signal_project::PROJECT_FILE);
    let what = what_of(edits);
    let failed = |error: String| {
        ProjectWriteError::Failed(format!("{what} not written to {}: {error}", path.display()))
    };
    let before = fs::read_to_string(&path).map_err(|e| failed(e.to_string()))?;
    let kept: Vec<usize> = (0..edits.len()).filter(|&i| keep(&before, i)).collect();
    if kept.is_empty() {
        return Ok((before.clone(), before, kept));
    }
    let chosen: Vec<_> = kept.iter().filter_map(|&i| edits.get(i).copied()).collect();
    let after =
        signal_project::write_parameters(&before, &chosen).map_err(|e| failed(e.to_string()))?;
    between();
    let written = replace_unchanged(&path, Some(before.as_bytes()), after.as_bytes())
        .map_err(|e| failed(e.to_string()))?;
    if !written {
        return Err(ProjectWriteError::Changed(format!(
            "{what} not written to {}: {} changed while the value was \
             written (another writer); nothing was written",
            path.display(),
            signal_project::PROJECT_FILE
        )));
    }
    Ok((before, after, kept))
}

/// Why [`write_project`] wrote nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectWriteError {
    /// `project.toml` changed between the read the edit was based on and
    /// the rename (another writer, or the person's editor): the protocol's
    /// `conflict`.
    Changed(String),
    /// Anything else (no lock within its wait, unreadable, not TOML, the
    /// write failed): `write-failed`.
    Failed(String),
}

impl ProjectWriteError {
    /// The UI protocol's error code for it.
    pub fn code(&self) -> &'static str {
        match self {
            ProjectWriteError::Changed(_) => "conflict",
            ProjectWriteError::Failed(_) => "write-failed",
        }
    }
}

impl std::fmt::Display for ProjectWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectWriteError::Changed(message) | ProjectWriteError::Failed(message) => {
                f.write_str(message)
            }
        }
    }
}

/// Writes `text` to `path` through a hidden staging file of this writer's
/// beside it (the watch ignores hidden files), synced, then renamed over
/// `path`, so a reader sees the old file or the new one
/// ([`signal_project::transaction::write_atomic`], ADR 0040). The staging
/// file is created exclusively under a name of its own (`O_EXCL`, which does
/// not follow a link), so nothing outside the folder is written, the renamed
/// file is a regular file, and another writer's staging file is never
/// removed or renamed. A caller that checks the file's version first holds
/// the project's [`signal_project::transaction::ProjectLock`] from the check
/// to the write.
pub fn write_file(path: &Path, text: &str) -> io::Result<()> {
    signal_project::transaction::write_atomic(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// audit 2026-10-23 (writer): the auditor's interleaving, fixed. Another
    /// writer has its staging file open at the shared name earlier builds
    /// used (`.main.html.tmp`) with one byte of its text written; this write
    /// commits exactly its own complete text and never removes, renames or
    /// writes the other writer's file (before the fix it removed it, or
    /// committed it as its own and returned `Ok`).
    #[test]
    fn a_write_never_takes_another_writers_staging_file() {
        let dir = std::env::temp_dir().join(format!("signal-panel-writers-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main.html");
        fs::write(&path, "old").unwrap();
        let theirs = dir.join(".main.html.tmp");
        let mut second = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&theirs)
            .unwrap();
        io::Write::write_all(&mut second, b"B").unwrap();
        let a = "A".repeat(1 << 20);
        write_file(&path, &a).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), a);
        assert_eq!(fs::read(&theirs).unwrap(), b"B");
        drop(second);
        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            2,
            "theirs and the file"
        );
        // `write_project` the same way, under the project's lock, which it
        // releases.
        fs::write(
            dir.join("project.toml"),
            "schema_version = 1\n[instances]\nout = { module = \"std.audio_out\", gain = 0.25 }\n",
        )
        .unwrap();
        let (_, after) = write_project(&dir, "out", "gain", Some(0.5), &["gain"]).unwrap();
        assert!(after.contains("gain = 0.5"), "{after}");
        assert!(
            signal_project::transaction::ProjectLock::try_acquire(&dir)
                .unwrap()
                .is_some()
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    /// ADR 0040 (review): `write_project` compares, as the other writers
    /// do. A writer that takes no lock (an editor, or any writer on a volume
    /// without locks) replaces `project.toml` after the commit read it and
    /// before its rename: the commit is `conflict`, the other writer's text
    /// stays, and no staging file is left.
    #[test]
    fn a_commit_refuses_a_file_changed_before_its_rename() {
        let dir = std::env::temp_dir().join(format!("signal-panel-commit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("project.toml");
        let ours =
            "schema_version = 1\n[instances]\nout = { module = \"std.audio_out\", gain = 0.25 }\n";
        let theirs = ours.replace("0.25", "0.125");
        fs::write(&path, ours).unwrap();
        let refused = write_project_between(
            &dir,
            &[("out", "gain", Some(0.5), &["gain"][..])],
            &|_, _| true,
            &mut || {
                fs::write(&path, &theirs).unwrap();
            },
        )
        .unwrap_err();
        assert_eq!(refused.code(), "conflict", "{refused}");
        assert_eq!(fs::read_to_string(&path).unwrap(), theirs);
        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            1,
            "no staging file left"
        );
        // Read again, the same commit is written onto their text.
        let (before, after) = write_project(&dir, "out", "gain", Some(0.5), &["gain"]).unwrap();
        assert_eq!(before, theirs);
        assert!(after.contains("gain = 0.5"), "{after}");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Copies the folder `from` to `to`, recursively.
    fn copy_dir(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), &target).unwrap();
            }
        }
    }

    /// Review of ADR 0040: a commit in a project that includes others
    /// (two-studios, with the studio beside it) is not taken by the running
    /// project. An included project's file changed on disk and not reloaded
    /// yet stays the reload's to apply: the running project is not rebuilt
    /// from the folder, so the diff to the folder still holds the change.
    #[test]
    fn a_commit_with_includes_leaves_their_changes_to_the_reload() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let base =
            std::env::temp_dir().join(format!("signal-panel-includes-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let dir = base.join("two-studios");
        copy_dir(&root.join("projects/two-studios"), &dir);
        copy_dir(&root.join("projects/studio"), &base.join("studio"));
        let mut registry = Registry::new();
        signal_std::register(&mut registry).unwrap();
        let mut current = Project::load(&dir).unwrap();
        let before = current.files.clone();
        let target = Panel::new(&current, &registry)
            .target("mix_l", "gain_a")
            .unwrap();
        // The included studio's bus, edited on disk and not reloaded.
        let studio = base.join("studio/project.toml");
        let text = fs::read_to_string(&studio).unwrap();
        let edited = text.replace(
            "bus = { module = \"std.mix\", gain_a = 0.45,",
            "bus = { module = \"std.mix\", gain_a = 0.4,",
        );
        assert_ne!(edited, text);
        fs::write(&studio, edited).unwrap();
        let order: Vec<&str> = target.order.iter().map(String::as_str).collect();
        write_project(&dir, "mix_l", "gain_a", Some(0.3), &order).unwrap();
        let written = after_write(
            Some(&mut current),
            &dir,
            &target,
            "mix_l",
            "gain_a",
            Some(0.3),
            true,
            &registry,
        );
        assert!(matches!(written, Written::NotTaken(_)), "not taken");
        assert_eq!(current.files, before, "the running project is unchanged");
        let reloaded = Project::load(&dir).unwrap();
        match signal_project::diff(&current, &reloaded, &registry).unwrap() {
            signal_project::ProjectEdit::Parameters(changes) => {
                let ports: Vec<String> = changes.iter().map(|c| c.port.to_string()).collect();
                assert!(
                    ports.contains(&"/mix_l.gain_a".to_owned())
                        && ports.contains(&"/studio/bus.gain_a".to_owned()),
                    "the reload applies the commit and the include's change: {ports:?}"
                );
            }
            edit => panic!("{edit:?}"),
        }
        fs::remove_dir_all(&base).unwrap();
    }

    /// M2.8-9 AC2 (`docs/ui-protocol.md` "Unsaved"): `describe`'s `base` is
    /// a drag's `applied`, the last value the Controller took for every
    /// port. A runner refusing that value afterwards leaves it (the value is
    /// listed `applied: false`), and a later value the Controller refuses
    /// keeps it. (Through the protocol a runner cannot refuse a `set`: the
    /// Controller keeps at most as many commands in flight as the engine's
    /// event queue holds, so the queue never overflows.)
    #[test]
    fn the_base_is_the_last_value_the_controller_took() {
        let mut panel = Panel::default();
        panel.drag("out", "gain", 0.3, true);
        assert_eq!(panel.dragged("out", "gain").unwrap().applied, 0.3);
        // The Controller takes 0.2; a runner refuses it.
        panel.drag("out", "gain", 0.2, true);
        panel.unsend("out", "gain", 0.2);
        let drag = panel.dragged("out", "gain").unwrap();
        assert!(!drag.sent);
        assert_eq!(drag.applied, 0.2);
        // The Controller refuses 0.1: the base stays the runner-refused 0.2.
        panel.drag("out", "gain", 0.1, false);
        assert_eq!(panel.dragged("out", "gain").unwrap().applied, 0.2);
    }
}
