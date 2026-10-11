//! Flattening: composites are resolved recursively into primitive instances
//! with hierarchical addresses, and connections are resolved through exported
//! ports to primitive ports.
//!
//! Instances, connections and exports are visited in sorted order, so the result
//! and the order of errors do not depend on declaration order.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use signal_core::{Direction, InstanceAddress, ModuleDescriptor, PortAddress, Registry};

use crate::def::{CompositeDef, ConnectionDef};
use crate::error::GraphError;

/// A primitive instance of the flattened graph.
pub(crate) struct FlatInstance {
    pub(crate) descriptor: ModuleDescriptor,
    /// Every declared parameter: the default, or the value set on the instance
    /// or on an enclosing composite instance (the outermost wins).
    pub(crate) parameters: BTreeMap<String, f64>,
}

/// A connection between primitive ports.
pub(crate) struct FlatConnection {
    pub(crate) from: PortAddress,
    pub(crate) to: PortAddress,
    pub(crate) scale: f64,
}

/// The flattened graph: primitive instances and connections between them.
pub(crate) struct FlatGraph {
    pub(crate) instances: BTreeMap<InstanceAddress, FlatInstance>,
    pub(crate) connections: Vec<FlatConnection>,
}

/// The primitive ports behind each export of a composite instance.
#[derive(Default)]
struct Interface {
    inputs: BTreeMap<String, Vec<PortAddress>>,
    outputs: BTreeMap<String, PortAddress>,
    parameters: BTreeMap<String, Vec<PortAddress>>,
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
    };
    flattener.expand(root, None, &mut vec![root.name.as_str()]);
    if flattener.errors.is_empty() {
        Ok(FlatGraph {
            instances: flattener.instances,
            connections: flattener.connections,
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
            let local = self.instantiate(&address, &instance.module, &instance.parameters, stack);
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

    /// Resolves the instance at `address` of module `module`, with the
    /// parameter values set on it.
    fn instantiate(
        &mut self,
        address: &InstanceAddress,
        module: &str,
        parameters: &BTreeMap<String, f64>,
        stack: &mut Vec<&'a str>,
    ) -> Option<Local> {
        if let Some(module_type) = self.registry.get(module) {
            let descriptor = module_type.descriptor;
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
            let instance = FlatInstance {
                descriptor,
                parameters: values,
            };
            self.instances.insert(address.clone(), instance);
            return Some(Local::Primitive(descriptor));
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
        let interface = self.expand(def, Some(address), stack);
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
        let from = self.ok(source.source(from));
        let to = self.ok(target.targets(to, false));
        let (Some(from), Some(to)) = (from, to) else {
            return;
        };
        for to in to {
            self.connections.push(FlatConnection {
                from: from.clone(),
                to,
                scale,
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
            .map(|e| (&e.name, &e.to))
            .collect();
        parameters.sort();
        for (name, targets) in parameters {
            let ports = self.export_targets(scope, locals, targets, true);
            interface.parameters.insert(name.clone(), ports);
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
