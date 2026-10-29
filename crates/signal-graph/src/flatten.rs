//! Flattening: composites are resolved recursively into primitive instances
//! with hierarchical addresses, and connections are resolved through exported
//! ports to primitive ports.
//!
//! Instances, connections and exports are visited in sorted order, so the result
//! and the order of errors do not depend on declaration order.
//!
//! Composites can expand exponentially (each instantiating several of the
//! next), so flattening counts instances and connections as it makes them
//! and stops at the first budget exceeded ([`PlanBudget`]): what a short
//! text expands to never runs away. A primitive whose vector is wider than
//! [`MAX_VECTOR_WIDTH`] is refused here, before the engine sees it.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use signal_core::{
    Direction, InstanceAddress, MAX_VECTOR_WIDTH, ModuleDescriptor, PortAddress, PortShape, Range,
    Registry, Snap, StreamKey, ValueLabel, converter_shape, snap_value,
};

use crate::def::{CompositeDef, ConnectionDef, InstanceDef};
use crate::error::{GraphError, GraphWarning, PlanBudget};

/// A primitive instance of the flattened graph.
pub(crate) struct FlatInstance {
    pub(crate) descriptor: ModuleDescriptor,
    /// Every declared parameter: the default, or the value set on the instance
    /// or on an enclosing composite instance (the outermost wins).
    pub(crate) parameters: BTreeMap<String, f64>,
    /// Its stream key under the innermost scope root (ADR 0035 §4), if any.
    pub(crate) stream: Option<StreamKey>,
}

/// A connection between primitive ports.
pub(crate) struct FlatConnection {
    pub(crate) from: PortAddress,
    pub(crate) to: PortAddress,
    pub(crate) scale: f64,
    /// The connection as written (its declared endpoints), for errors;
    /// `None` for one the graph inserted (a converter's).
    pub(crate) declared: Option<(PortAddress, PortAddress)>,
}

/// The flattened graph: primitive instances and connections between them.
pub(crate) struct FlatGraph {
    pub(crate) instances: BTreeMap<InstanceAddress, FlatInstance>,
    pub(crate) connections: Vec<FlatConnection>,
    /// Values of exported parameters corrected while flattening (ADR 0042,
    /// 0043): the plan's warnings.
    pub(crate) warnings: Vec<GraphWarning>,
}

/// The primitive ports behind each export of a composite instance.
#[derive(Default)]
struct Interface {
    inputs: BTreeMap<String, Vec<PortAddress>>,
    outputs: BTreeMap<String, PortAddress>,
    parameters: BTreeMap<String, Vec<PortAddress>>,
    /// ADR 0042: each exported parameter's range: its own when it declares
    /// one, else the intersection of its targets' (`None`: no target has
    /// one); a value outside it is clamped to it, with a warning.
    ranges: BTreeMap<String, Option<Range>>,
    /// ADR 0043: each exported parameter's value set: its own, else the one
    /// every target shares (a primitive's, or an inner export's), kept to
    /// its own range; a value that is not one of it is snapped to it, with
    /// a warning.
    value_sets: BTreeMap<String, Vec<(f64, String)>>,
    /// ADR 0043: how each value set is read: its targets' rule
    /// ([`Snap::Positive`] when every target reads it so).
    snaps: BTreeMap<String, Snap>,
}

/// A resolved instance of the composite being expanded, as endpoints see it.
enum Local {
    Primitive(ModuleDescriptor),
    Composite(Interface),
}

impl Local {
    /// The primitive output port behind `port` (an endpoint on this instance).
    fn source(&self, port: PortAddress) -> Result<PortAddress, GraphError> {
        let name = port.port();
        let direction = match self {
            Local::Primitive(descriptor) => {
                let parameter = descriptor.parameters.iter().any(|p| p.name == name);
                descriptor
                    .ports
                    .iter()
                    .find(|p| p.name == name)
                    .map(|p| p.direction)
                    .or(parameter.then_some(Direction::In))
            }
            Local::Composite(interface) => {
                if let Some(output) = interface.outputs.get(name) {
                    return Ok(output.clone());
                }
                let input =
                    interface.inputs.contains_key(name) || interface.parameters.contains_key(name);
                input.then_some(Direction::In)
            }
        };
        match direction {
            Some(Direction::Out) => Ok(port),
            Some(Direction::In) => Err(GraphError::WrongDirection {
                port,
                expected: Direction::Out,
            }),
            None => Err(GraphError::UnknownPort { port }),
        }
    }

    /// The primitive input ports or parameters behind `port`; only parameters
    /// when `parameters_only`.
    fn targets(
        &self,
        port: PortAddress,
        parameters_only: bool,
    ) -> Result<Vec<PortAddress>, GraphError> {
        let name = port.port();
        let direction = match self {
            Local::Primitive(descriptor) => {
                if descriptor.parameters.iter().any(|p| p.name == name) {
                    return Ok(vec![port]);
                }
                descriptor
                    .ports
                    .iter()
                    .find(|p| p.name == name)
                    .map(|p| p.direction)
            }
            Local::Composite(interface) => {
                if let Some(targets) = interface.parameters.get(name) {
                    return Ok(targets.clone());
                }
                match interface.inputs.get(name) {
                    Some(targets) if !parameters_only => return Ok(targets.clone()),
                    Some(_) => Some(Direction::In),
                    None => interface
                        .outputs
                        .contains_key(name)
                        .then_some(Direction::Out),
                }
            }
        };
        match direction {
            _ if parameters_only => Err(GraphError::UnknownParameter { parameter: port }),
            Some(Direction::In) => Ok(vec![port]),
            Some(Direction::Out) => Err(GraphError::WrongDirection {
                port,
                expected: Direction::In,
            }),
            None => Err(GraphError::UnknownPort { port }),
        }
    }
}

/// Whether `name` is a single address segment, `[a-z][a-z0-9_]*`.
fn valid_name(name: &str) -> bool {
    !name.contains('/') && format!("/{name}").parse::<InstanceAddress>().is_ok()
}

fn child_address(scope: Option<&InstanceAddress>, name: &str) -> Option<InstanceAddress> {
    if !valid_name(name) {
        return None;
    }
    match scope {
        Some(scope) => scope.child(name).ok(),
        None => format!("/{name}").parse().ok(),
    }
}

fn cmp_scale(a: Option<f64>, b: Option<f64>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.total_cmp(&b),
        _ => a.is_some().cmp(&b.is_some()),
    }
}

/// The instances of the composite being expanded: `None` for one that failed
/// to resolve (already reported), whose endpoints are skipped so one error does
/// not cascade.
type Locals<'d> = BTreeMap<&'d str, (InstanceAddress, Option<Local>)>;

struct Flattener<'a> {
    registry: &'a Registry,
    composites: BTreeMap<&'a str, &'a CompositeDef>,
    instances: BTreeMap<InstanceAddress, FlatInstance>,
    connections: Vec<FlatConnection>,
    errors: Vec<GraphError>,
    warnings: Vec<GraphWarning>,
    /// Instances resolved so far, composite or primitive.
    made: usize,
    /// The budget flattening stopped at, if any: nothing more is expanded.
    exceeded: Option<PlanBudget>,
    /// The scope roots (ADR 0035 §4) being expanded, outermost first: each
    /// root's address and seed.
    scopes: Vec<(InstanceAddress, u64)>,
}

/// The widest vector port of `descriptor`, if any is wider than
/// [`MAX_VECTOR_WIDTH`].
fn too_wide(descriptor: &ModuleDescriptor) -> Option<u32> {
    descriptor
        .ports
        .iter()
        .filter_map(|p| match p.shape {
            PortShape::Vector(n) if n > MAX_VECTOR_WIDTH => Some(n),
            _ => None,
        })
        .max()
}

/// Flattens `root`, resolving composite references in `composites` and
/// primitive types in `registry`. Returns every error found.
pub(crate) fn flatten(
    root: &CompositeDef,
    composites: &[CompositeDef],
    registry: &Registry,
) -> Result<FlatGraph, Vec<GraphError>> {
    let mut sorted: Vec<&CompositeDef> = composites.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    let mut library = BTreeMap::new();
    let mut errors = Vec::new();
    for def in sorted {
        if library.insert(def.name.as_str(), def).is_some() {
            let error = GraphError::DuplicateComposite {
                name: def.name.clone(),
            };
            if !errors.contains(&error) {
                errors.push(error);
            }
        } else if registry.get(&def.name).is_some() {
            errors.push(GraphError::AmbiguousModule {
                module: def.name.clone(),
            });
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut flattener = Flattener {
        registry,
        composites: library,
        instances: BTreeMap::new(),
        connections: Vec::new(),
        errors,
        warnings: Vec::new(),
        made: 0,
        exceeded: None,
        scopes: Vec::new(),
    };
    flattener.expand(root, None, &mut vec![root.name.as_str()]);
    if flattener.errors.is_empty() {
        Ok(FlatGraph {
            instances: flattener.instances,
            connections: flattener.connections,
            warnings: flattener.warnings,
        })
    } else {
        Err(flattener.errors)
    }
}

impl<'a> Flattener<'a> {
    /// Expands `def` instantiated at `scope` (`None`: the project root) and
    /// returns its exports as primitive ports. `stack` holds the names of the
    /// composites being expanded, outermost first.
    fn expand(
        &mut self,
        def: &'a CompositeDef,
        scope: Option<&InstanceAddress>,
        stack: &mut Vec<&'a str>,
    ) -> Interface {
        self.check_exports(def, scope);

        let mut instances: Vec<_> = def.instances.iter().collect();
        instances.sort_by(|a, b| a.name.cmp(&b.name));
        let mut locals: Locals<'a> = BTreeMap::new();
        for instance in instances {
            if locals.contains_key(instance.name.as_str()) {
                self.errors.push(GraphError::DuplicateInstance {
                    scope: scope.cloned(),
                    name: instance.name.clone(),
                });
                continue;
            }
            let Some(address) = child_address(scope, &instance.name) else {
                self.errors.push(GraphError::InvalidName {
                    scope: scope.cloned(),
                    name: instance.name.clone(),
                });
                continue;
            };
            let local = self.instantiate(&address, instance, stack);
            locals.insert(instance.name.as_str(), (address, local));
        }

        let mut connections: Vec<&ConnectionDef> = def.connections.iter().collect();
        connections.sort_by(|a, b| {
            (&a.from, &a.to)
                .cmp(&(&b.from, &b.to))
                .then_with(|| cmp_scale(a.scale, b.scale))
        });
        for connection in connections {
            self.connect(scope, &locals, connection);
        }

        self.interface(def, scope, &locals)
    }

    /// Reports invalid, colliding and empty exports of `def`.
    fn check_exports(&mut self, def: &CompositeDef, scope: Option<&InstanceAddress>) {
        let exports = &def.exports;
        let names = exports
            .inputs
            .iter()
            .map(|e| &e.name)
            .chain(exports.outputs.iter().map(|e| &e.name))
            .chain(exports.parameters.iter().map(|e| &e.name));
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for name in names {
            *counts.entry(name.as_str()).or_default() += 1;
        }
        for (name, count) in counts {
            if !valid_name(name) {
                self.errors.push(GraphError::InvalidName {
                    scope: scope.cloned(),
                    name: name.to_owned(),
                });
            } else if count > 1 {
                self.errors.push(GraphError::ExportNameCollision {
                    scope: scope.cloned(),
                    composite: def.name.clone(),
                    name: name.to_owned(),
                });
            }
        }

        let fan_outs = exports
            .inputs
            .iter()
            .map(|e| (&e.name, &e.to))
            .chain(exports.parameters.iter().map(|e| (&e.name, &e.to)));
        let mut empty: Vec<&String> = fan_outs
            .filter(|(_, to)| to.is_empty())
            .map(|(name, _)| name)
            .collect();
        empty.sort();
        for name in empty {
            self.errors.push(GraphError::EmptyExport {
                scope: scope.cloned(),
                composite: def.name.clone(),
                name: name.clone(),
            });
        }
    }

    /// Resolves `instance` at `address`: its module, with the parameter
    /// values set on it, under its scope root if it is one.
    fn instantiate(
        &mut self,
        address: &InstanceAddress,
        instance: &'a InstanceDef,
        stack: &mut Vec<&'a str>,
    ) -> Option<Local> {
        let module = instance.module.as_str();
        let parameters = &instance.parameters;
        if self.exceeded.is_some() {
            return None;
        }
        self.made += 1;
        if self.made > PlanBudget::Instances.limit() {
            self.exceed(PlanBudget::Instances, self.made);
            return None;
        }
        if let Some(module_type) = self.registry.get(module) {
            let descriptor = module_type.descriptor;
            if let Some(width) = too_wide(&descriptor) {
                self.errors.push(GraphError::VectorTooWide {
                    instance: address.clone(),
                    module: module.to_owned(),
                    width,
                });
                return None;
            }
            let mut values: BTreeMap<String, f64> = descriptor
                .parameters
                .iter()
                .map(|p| (p.name.to_owned(), p.default))
                .collect();
            for (name, value) in parameters {
                match values.get_mut(name) {
                    Some(slot) => *slot = *value,
                    None => self.unknown_parameter(address, name),
                }
            }
            if instance.scope.is_some() {
                self.errors.push(GraphError::ScopeOnPrimitive {
                    instance: address.clone(),
                    module: module.to_owned(),
                });
                return None;
            }
            let instance = FlatInstance {
                descriptor,
                parameters: values,
                stream: self.stream_key(address),
            };
            self.instances.insert(address.clone(), instance);
            return Some(Local::Primitive(descriptor));
        }

        // A converter's name whose vector is too wide for one to be made.
        if let Some(PortShape::Vector(width)) = converter_shape(module)
            && width > MAX_VECTOR_WIDTH
        {
            self.errors.push(GraphError::VectorTooWide {
                instance: address.clone(),
                module: module.to_owned(),
                width,
            });
            return None;
        }
        let Some(&def) = self.composites.get(module) else {
            self.errors.push(GraphError::UnknownModule {
                instance: address.clone(),
                module: module.to_owned(),
            });
            return None;
        };
        if let Some(start) = stack.iter().position(|name| *name == def.name) {
            let mut chain: Vec<String> = stack[start..].iter().map(|s| (*s).to_owned()).collect();
            chain.push(def.name.clone());
            self.errors.push(GraphError::RecursiveComposite {
                instance: address.clone(),
                chain,
            });
            return None;
        }

        let errors_before = self.errors.len();
        stack.push(def.name.as_str());
        if let Some(scope) = instance.scope {
            self.scopes.push((address.clone(), scope.seed));
        }
        let interface = self.expand(def, Some(address), stack);
        if instance.scope.is_some() {
            self.scopes.pop();
        }
        stack.pop();
        if self.errors.len() > errors_before {
            // The composite is broken, so its exports may be incomplete.
            return None;
        }
        for (name, value) in parameters {
            let Some(targets) = interface.parameters.get(name) else {
                self.unknown_parameter(address, name);
                continue;
            };
            // ADR 0042, 0043 (the operator's decision of 2026-10-25): a value
            // outside the export's range (its own, else the intersection of
            // its targets', as the panel's control has it) is clamped to it,
            // one not in its value set snapped as its targets read it; a
            // warning says so, and the corrected value is what the targets
            // get.
            let mut value = *value;
            if let Some(Some(range)) = interface.ranges.get(name)
                && range.min <= range.max
                && !(range.min <= value && value <= range.max)
            {
                let to = value.max(range.min).min(range.max);
                self.corrected(address, name, value, to, outside(*range));
                value = to;
            }
            if let Some(set) = interface.value_sets.get(name)
                && !set.iter().any(|(v, _)| *v == value)
            {
                let snap = interface.snaps.get(name).copied().unwrap_or_default();
                let to = snapped(set, snap, value);
                let allowed = crate::allowed_text(set.iter().map(|(v, l)| (*v, l.as_str())));
                self.corrected(address, name, value, to, not_one_of(&allowed));
                value = to;
            }
            let value = &value;
            for target in targets {
                let slot = self
                    .instances
                    .get_mut(&target.instance())
                    .and_then(|instance| instance.parameters.get_mut(target.port()));
                if let Some(slot) = slot {
                    *slot = *value;
                }
            }
        }
        Some(Local::Composite(interface))
    }

    /// Records a corrected value of `name` on the instance at `address`.
    fn corrected(
        &mut self,
        address: &InstanceAddress,
        name: &str,
        value: f64,
        to: f64,
        reason: String,
    ) {
        if let Ok(parameter) = PortAddress::new(address, name) {
            self.warnings.push(GraphWarning::ParameterCorrected {
                parameter,
                value,
                to,
                reason: reason.into(),
            });
        }
    }

    /// The stream key of the primitive at `address` (ADR 0035 §4): the
    /// innermost scope root's seed and the address relative to it; `None`
    /// outside every scope root.
    fn stream_key(&self, address: &InstanceAddress) -> Option<StreamKey> {
        let (root, seed) = self.scopes.last()?;
        // The root is a strict prefix of `address`: it is being expanded.
        let relative = address.as_str().strip_prefix(root.as_str())?;
        Some(StreamKey {
            seed: *seed,
            relative: relative.parse().ok()?,
        })
    }

    /// Stops flattening at `budget`, reporting it once with the count it
    /// stopped at.
    fn exceed(&mut self, budget: PlanBudget, count: usize) {
        if self.exceeded.is_none() {
            self.exceeded = Some(budget);
            self.errors.push(GraphError::PlanTooLarge { budget, count });
        }
    }

    fn unknown_parameter(&mut self, instance: &InstanceAddress, name: &str) {
        self.errors.push(match PortAddress::new(instance, name) {
            Ok(parameter) => GraphError::UnknownParameter { parameter },
            Err(_) => GraphError::InvalidName {
                scope: Some(instance.clone()),
                name: name.to_owned(),
            },
        });
    }

    /// Resolves endpoint `text` (`inst.port`) to its local instance and declared
    /// port address. `None` when an error was reported or the instance is
    /// unresolved.
    fn endpoint<'l>(
        &mut self,
        scope: Option<&InstanceAddress>,
        locals: &'l Locals<'_>,
        text: &str,
    ) -> Option<(&'l Local, PortAddress)> {
        let invalid = || GraphError::InvalidEndpoint {
            scope: scope.cloned(),
            endpoint: text.to_owned(),
        };
        let Some((instance, port)) = text.split_once('.') else {
            self.errors.push(invalid());
            return None;
        };
        let Some((address, local)) = locals.get(instance) else {
            self.errors.push(GraphError::UnknownInstance {
                scope: scope.cloned(),
                name: instance.to_owned(),
            });
            return None;
        };
        let local = local.as_ref()?;
        match PortAddress::new(address, port) {
            Ok(port) => Some((local, port)),
            Err(_) => {
                self.errors.push(invalid());
                None
            }
        }
    }

    fn connect(
        &mut self,
        scope: Option<&InstanceAddress>,
        locals: &Locals<'_>,
        connection: &ConnectionDef,
    ) {
        if self.exceeded.is_some() {
            return;
        }
        let source = self.endpoint(scope, locals, &connection.from);
        let target = self.endpoint(scope, locals, &connection.to);
        let (Some((source, from)), Some((target, to))) = (source, target) else {
            return;
        };
        let scale = connection.scale.unwrap_or(1.0);
        if !scale.is_finite() {
            self.errors
                .push(GraphError::NonFiniteScale { from, to, scale });
            return;
        }
        let declared = (from.clone(), to.clone());
        let from = self.ok(source.source(from));
        let to = self.ok(target.targets(to, false));
        let (Some(from), Some(to)) = (from, to) else {
            return;
        };
        let count = self.connections.len().saturating_add(to.len());
        if count > PlanBudget::Connections.limit() {
            self.exceed(PlanBudget::Connections, count);
            return;
        }
        for to in to {
            self.connections.push(FlatConnection {
                from: from.clone(),
                to,
                scale,
                declared: Some(declared.clone()),
            });
        }
    }

    /// The exports of `def` as primitive ports, reporting unresolvable targets.
    fn interface(
        &mut self,
        def: &CompositeDef,
        scope: Option<&InstanceAddress>,
        locals: &Locals<'_>,
    ) -> Interface {
        let mut interface = Interface::default();
        let exports = &def.exports;

        let mut inputs: Vec<_> = exports.inputs.iter().map(|e| (&e.name, &e.to)).collect();
        inputs.sort();
        for (name, targets) in inputs {
            let ports = self.export_targets(scope, locals, targets, false);
            interface.inputs.insert(name.clone(), ports);
        }

        let mut parameters: Vec<_> = exports
            .parameters
            .iter()
            .map(|e| (&e.name, &e.to, e.range, e.values.as_deref()))
            .collect();
        parameters.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        for (name, targets, declared, values) in parameters {
            let mut set = self.export_values(def, scope, locals, name, targets, values);
            let ports = self.export_targets(scope, locals, targets, true);
            interface.parameters.insert(name.clone(), ports);
            let range = self.export_range(def, scope, locals, name, targets, declared);
            // ADR 0043: an own range with a value set taken from the targets
            // keeps the values inside it; none left is refused.
            if let (Some(declared), None, Some((entries, _))) = (declared, values, &mut set) {
                let allowed = crate::allowed_text(entries.iter().map(|(v, l)| (*v, l.as_str())));
                entries.retain(|(v, _)| declared.min <= *v && *v <= declared.max);
                if entries.is_empty() {
                    self.errors.push(GraphError::ExportRangeWithoutValues {
                        scope: scope.cloned(),
                        composite: def.name.clone(),
                        name: name.clone(),
                        range: declared,
                        allowed,
                    });
                }
            }
            if let Some((entries, snap)) = set.filter(|(entries, _)| !entries.is_empty()) {
                interface.value_sets.insert(name.clone(), entries);
                interface.snaps.insert(name.clone(), snap);
            }
            interface.ranges.insert(name.clone(), range);
        }

        let mut outputs: Vec<_> = exports.outputs.iter().map(|e| (&e.name, &e.from)).collect();
        outputs.sort();
        for (name, from) in outputs {
            if let Some((local, port)) = self.endpoint(scope, locals, from)
                && let Some(port) = self.ok(local.source(port))
            {
                interface.outputs.insert(name.clone(), port);
            }
        }
        interface
    }

    /// ADR 0042: the range of exported parameter `name`: `declared`, which
    /// must lie inside every target's range (a primitive parameter's, or an
    /// inner composite's export's), else the intersection of the targets'
    /// ranges (the narrowest, as the panel's control and `describe` have
    /// it).
    fn export_range(
        &mut self,
        def: &CompositeDef,
        scope: Option<&InstanceAddress>,
        locals: &Locals<'_>,
        name: &str,
        targets: &[String],
        declared: Option<Range>,
    ) -> Option<Range> {
        let narrow = |a: Option<Range>, b: Range| match a {
            Some(a) => Range {
                min: a.min.max(b.min),
                max: a.max.min(b.max),
            },
            None => b,
        };
        let mut sorted: Vec<&String> = targets.iter().collect();
        sorted.sort();
        let mut shared: Option<Range> = None;
        for target in sorted {
            let Some((instance, port)) = target.split_once('.') else {
                continue;
            };
            let Some((_, Some(local))) = locals.get(instance) else {
                continue;
            };
            let range = match local {
                Local::Primitive(descriptor) => descriptor
                    .parameters
                    .iter()
                    .find(|p| p.name == port)
                    .map(|p| p.range),
                Local::Composite(inner) => inner.ranges.get(port).copied().flatten(),
            };
            let Some(range) = range else {
                continue;
            };
            if let Some(declared) = declared
                && !(range.min <= declared.min && declared.max <= range.max)
            {
                self.errors.push(GraphError::ExportRangeOutside {
                    scope: scope.cloned(),
                    composite: def.name.clone(),
                    name: name.to_owned(),
                    target: target.clone(),
                    ranges: Box::new([declared, range]),
                });
            }
            shared = Some(narrow(shared, range));
        }
        declared.or(shared)
    }

    /// ADR 0043: the value set of exported parameter `name`: `declared`,
    /// each value of which must be allowed by every target that has a value
    /// set (a primitive's, or an inner export's), else the set every target
    /// shares (`None` when one has none or two differ); with the rule it is
    /// read by ([`Snap::Positive`] when every target reads it so).
    fn export_values(
        &mut self,
        def: &CompositeDef,
        scope: Option<&InstanceAddress>,
        locals: &Locals<'_>,
        name: &str,
        targets: &[String],
        declared: Option<&[crate::def::ExportValue]>,
    ) -> Option<(Vec<(f64, String)>, Snap)> {
        let mut sorted: Vec<&String> = targets.iter().collect();
        sorted.sort();
        let mut sets: Vec<Option<Vec<(f64, String)>>> = Vec::new();
        let mut positive = true;
        for target in sorted {
            let Some((instance, port)) = target.split_once('.') else {
                continue;
            };
            let Some((_, Some(local))) = locals.get(instance) else {
                continue;
            };
            let (set, snap): (Option<Vec<(f64, String)>>, Snap) = match local {
                Local::Primitive(descriptor) => {
                    let p = descriptor.parameters.iter().find(|p| p.name == port);
                    (
                        p.and_then(|p| p.values).map(|values| {
                            values
                                .iter()
                                .map(|v| (v.value, v.label.to_owned()))
                                .collect()
                        }),
                        p.map(|p| p.snap).unwrap_or_default(),
                    )
                }
                Local::Composite(inner) => (
                    inner.value_sets.get(port).cloned(),
                    inner.snaps.get(port).copied().unwrap_or_default(),
                ),
            };
            positive &= snap == Snap::Positive;
            if let (Some(declared), Some(set)) = (declared, &set) {
                for entry in declared {
                    if !set.iter().any(|(v, _)| *v == entry.value) {
                        self.errors.push(GraphError::ExportValueOutside {
                            scope: scope.cloned(),
                            composite: def.name.clone(),
                            name: name.to_owned(),
                            value: entry.value,
                            target: Box::new((
                                target.clone(),
                                crate::allowed_text(set.iter().map(|(v, l)| (*v, l.as_str()))),
                            )),
                        });
                    }
                }
            }
            sets.push(set);
        }
        let snap = if positive && !sets.is_empty() {
            Snap::Positive
        } else {
            Snap::Nearest
        };
        if let Some(declared) = declared {
            return Some((
                declared
                    .iter()
                    .map(|e| (e.value, e.label.clone()))
                    .collect(),
                snap,
            ));
        }
        let first = sets.first()?.clone()?;
        sets.iter()
            .all(|s| s.as_ref() == Some(&first))
            .then_some((first, snap))
    }

    fn export_targets(
        &mut self,
        scope: Option<&InstanceAddress>,
        locals: &Locals<'_>,
        targets: &[String],
        parameters_only: bool,
    ) -> Vec<PortAddress> {
        let mut sorted: Vec<&String> = targets.iter().collect();
        sorted.sort();
        let mut ports = BTreeSet::new();
        for target in sorted {
            if let Some((local, port)) = self.endpoint(scope, locals, target)
                && let Some(resolved) = self.ok(local.targets(port, parameters_only))
            {
                ports.extend(resolved);
            }
        }
        ports.into_iter().collect()
    }

    /// The value of `result`, recording its error instead.
    fn ok<T>(&mut self, result: Result<T, GraphError>) -> Option<T> {
        result.map_err(|error| self.errors.push(error)).ok()
    }
}

/// One port or parameter of a module as an instance of it shows it: a
/// primitive's declared port or parameter, or a composite's export with the
/// type of the inner port behind it ([`crate::ports_of`]).
#[derive(Clone, Debug, PartialEq)]
pub struct ModulePort {
    /// Its name: the port, parameter or export name.
    pub name: String,
    /// In or out; a parameter is in.
    pub direction: Direction,
    /// Whether it is a parameter.
    pub parameter: bool,
    /// Its shape; a parameter is a signal.
    pub shape: PortShape,
    /// Its rate divisor.
    pub rate: signal_core::RateDivisor,
    /// Its kind.
    pub kind: signal_core::PortKind,
    /// Its unit, if it declares one: a primitive's port or parameter
    /// descriptor's, or for an export the inner port's or parameter's
    /// behind it (the first in address order). Presentation only.
    pub unit: Option<String>,
    /// Its expected range, if it declares one: a primitive's port's or
    /// parameter's; for an export the inner port's or parameter's behind
    /// it (the first in address order), and for an exported parameter its
    /// own range where ADR 0042 gives it one. Presentation only.
    pub range: Option<signal_core::Range>,
}

/// The ports of `descriptor`'s type, in descriptor order: ports, then
/// parameters.
fn descriptor_ports(descriptor: &ModuleDescriptor) -> Vec<ModulePort> {
    let ports = descriptor.ports.iter().map(|p| ModulePort {
        name: p.name.to_owned(),
        direction: p.direction,
        parameter: false,
        shape: p.shape,
        rate: p.rate,
        kind: p.kind,
        unit: p.unit.map(str::to_owned),
        range: p.range,
    });
    let parameters = descriptor.parameters.iter().map(|p| ModulePort {
        name: p.name.to_owned(),
        direction: Direction::In,
        parameter: true,
        shape: PortShape::Signal,
        rate: p.rate,
        kind: p.kind,
        unit: p.unit.map(str::to_owned),
        range: Some(p.range),
    });
    ports.chain(parameters).collect()
}

/// See [`crate::ports_of`].
pub(crate) fn ports_of(
    module: &str,
    composites: &[CompositeDef],
    registry: &Registry,
) -> Result<Vec<ModulePort>, Vec<GraphError>> {
    if let Some(module_type) = registry.get(module) {
        return Ok(descriptor_ports(&module_type.descriptor));
    }
    let instance = InstanceDef {
        name: "module".to_owned(),
        module: module.to_owned(),
        ..InstanceDef::default()
    };
    let root = CompositeDef {
        instances: vec![instance],
        ..CompositeDef::default()
    };
    let mut library = BTreeMap::new();
    for def in composites {
        library.entry(def.name.as_str()).or_insert(def);
    }
    let mut flattener = Flattener {
        registry,
        composites: library,
        instances: BTreeMap::new(),
        connections: Vec::new(),
        errors: Vec::new(),
        warnings: Vec::new(),
        made: 0,
        exceeded: None,
        scopes: Vec::new(),
    };
    let address: InstanceAddress = "/module".parse().map_err(|_| Vec::new())?;
    let Some(root_instance) = root.instances.first() else {
        return Err(Vec::new());
    };
    let local = flattener.instantiate(&address, root_instance, &mut vec![""]);
    let interface = match local {
        Some(Local::Composite(interface)) if flattener.errors.is_empty() => interface,
        _ => return Err(flattener.errors),
    };
    let port_of = |port: &PortAddress| {
        let descriptor = flattener.instances.get(&port.instance())?.descriptor;
        descriptor
            .ports
            .iter()
            .find(|p| p.name == port.port())
            .copied()
    };
    let parameter_of = |port: &PortAddress| {
        let descriptor = flattener.instances.get(&port.instance())?.descriptor;
        descriptor
            .parameters
            .iter()
            .find(|p| p.name == port.port())
            .copied()
    };
    let mut ports = Vec::new();
    for (name, targets) in &interface.inputs {
        // The first inner port (in address order) gives the type; an input
        // that fans out to ports of different types is the graph's to check
        // per connection.
        if let Some(port) = targets.first().and_then(port_of) {
            ports.push(ModulePort {
                name: name.clone(),
                direction: Direction::In,
                parameter: false,
                shape: port.shape,
                rate: port.rate,
                kind: port.kind,
                unit: port.unit.map(str::to_owned),
                range: port.range,
            });
        } else if let Some(parameter) = targets.first().and_then(parameter_of) {
            // An exported input onto an inner parameter is still an input.
            ports.push(ModulePort {
                name: name.clone(),
                direction: Direction::In,
                parameter: false,
                shape: PortShape::Signal,
                rate: parameter.rate,
                kind: parameter.kind,
                unit: parameter.unit.map(str::to_owned),
                range: Some(parameter.range),
            });
        }
    }
    for (name, from) in &interface.outputs {
        if let Some(port) = port_of(from) {
            ports.push(ModulePort {
                name: name.clone(),
                direction: Direction::Out,
                parameter: false,
                shape: port.shape,
                rate: port.rate,
                kind: port.kind,
                unit: port.unit.map(str::to_owned),
                range: port.range,
            });
        }
    }
    for (name, targets) in &interface.parameters {
        if let Some(parameter) = targets.first().and_then(parameter_of) {
            ports.push(ModulePort {
                name: name.clone(),
                direction: Direction::In,
                parameter: true,
                shape: PortShape::Signal,
                rate: parameter.rate,
                kind: parameter.kind,
                unit: parameter.unit.map(str::to_owned),
                // ADR 0042: its own range, else the one its targets share.
                range: interface
                    .ranges
                    .get(name)
                    .copied()
                    .flatten()
                    .or(Some(parameter.range)),
            });
        }
    }
    ports.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(ports)
}

/// A warning's reason for a value outside `range` (ADR 0042).
pub(crate) fn outside(range: Range) -> String {
    format!("is outside [{}, {}]", range.min, range.max)
}

/// A warning's reason for a value not in a value set (ADR 0043).
pub(crate) fn not_one_of(allowed: &str) -> String {
    format!("is not one of {allowed}")
}

/// `value` as one of `set` read by `snap` ([`snap_value`], ADR 0043).
fn snapped(set: &[(f64, String)], snap: Snap, value: f64) -> f64 {
    let entries: Vec<ValueLabel> = set
        .iter()
        .map(|(v, _)| ValueLabel {
            value: *v,
            label: "",
        })
        .collect();
    snap_value(&entries, snap, value)
}
