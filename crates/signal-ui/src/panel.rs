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
use std::io::{self, Write};
use std::path::Path;

use signal_core::{InstanceAddress, ParameterDescriptor, PortAddress, PortShape, Registry};
use signal_engine::ParameterValue;
use signal_graph::{CompositeDef, ExecutionPlan};
use signal_project::Project;

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
    /// The values being dragged (each send carries the latest value to all
    /// of a parameter's ports, so only the latest send says what they hold).
    drags: BTreeMap<(String, String), Drag>,
}

/// A parameter's live value that the file may not hold: the latest value
/// sent (dragged, or set by a client), whether the Controller took it for
/// every port, and the last value known to have reached every port.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag {
    /// The latest value sent.
    pub value: f64,
    /// Whether the Controller took it for every port (and no runner refused
    /// it since).
    pub sent: bool,
    /// The last value known to have reached every port: `value` when sent,
    /// else the one before (the file's when there was none).
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
        let composites: BTreeMap<&str, &CompositeDef> = project
            .composites
            .iter()
            .map(|c| (c.name.as_str(), c))
            .collect();
        let mut instances = Vec::new();
        for def in &project.root.instances {
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
                let Some((_, first, inner_value)) = targets.first() else {
                    continue;
                };
                let (min, max) = targets.iter().fold((f64::MIN, f64::MAX), |(lo, hi), t| {
                    (lo.max(t.1.range.min), hi.min(t.1.range.max))
                });
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
        })
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
}

impl Target {
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
) -> Vec<(PortAddress, ParameterDescriptor, Option<f64>)> {
    if let Some(module) = registry.get(module) {
        let descriptor = module
            .descriptor
            .parameters
            .iter()
            .find(|p| p.name == parameter);
        return match (descriptor, PortAddress::new(address, parameter)) {
            (Some(d), Ok(port)) => vec![(port, *d, None)],
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
            targets.push(target);
        }
    }
    targets
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
    let Some(current) = current else {
        return Written::Stopped(
            crate::app::load(dir, registry)
                .map(|(project, plan, _)| (Panel::new(&project, registry), plan)),
        );
    };
    let order: Vec<&str> = target.order.iter().map(String::as_str).collect();
    match current.with_parameter(instance, parameter, value, &order) {
        Ok(project) => {
            let panel = Panel::new(&project, registry);
            let running = match target.unlike(value).filter(|_| !sent) {
                Some(unlike) => current.with_parameter(instance, parameter, Some(unlike), &order),
                // Sent, or a single-valued range (every send was that value).
                None => Ok(project),
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
/// new key goes. Atomic ([`write_file`]).
pub fn write_project(
    dir: &Path,
    instance: &str,
    parameter: &str,
    value: Option<f64>,
    order: &[&str],
) -> Result<(), String> {
    let path = dir.join(signal_project::PROJECT_FILE);
    fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|text| {
            signal_project::write_parameter(&text, instance, parameter, value, order)
                .map_err(|e| e.to_string())
        })
        .and_then(|text| write_file(&path, &text).map_err(|e| e.to_string()))
        .map_err(|error| {
            format!(
                "{instance}.{parameter} was not written to {}: {error}",
                path.display()
            )
        })
}

/// Writes `text` to `path` through a hidden temporary file beside it (the
/// watch ignores hidden files), synced, then renamed over `path`, so a reader
/// sees the old file or the new one.
pub fn write_file(path: &Path, text: &str) -> io::Result<()> {
    let mut name = std::ffi::OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(".tmp");
    let temporary = path.with_file_name(name);
    let written = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}
