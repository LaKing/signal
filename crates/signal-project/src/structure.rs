//! The project's structure as written (ADR 0026 §1), for the UI protocol's
//! `graph` message (`docs/ui-protocol.md`): the top level's instances and
//! connections and, for every composite the project uses, the same for its
//! inside with its exports. Nothing is flattened; the converters the
//! compiler inserted are listed on the connections they join, read from the
//! plan's warnings. Control-thread code: it allocates.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};
use signal_core::{Direction, ParameterDescriptor, PortDescriptor, Registry};
use signal_graph::{CompositeDef, ExecutionPlan, GraphWarning, InstanceDef};

use crate::Project;

/// What a written endpoint (`instance.port`) finally reaches.
enum Endpoint {
    Port(&'static PortDescriptor),
    Parameter(&'static ParameterDescriptor),
}

/// A composite's exports of one name, by (composite, export name): an
/// output's source, or the targets of an input or a parameter, in source order.
type ByExport<'a, T> = BTreeMap<(&'a str, &'a str), Vec<T>>;

struct Walk<'a> {
    project: &'a Project,
    registry: &'a Registry,
    by_name: BTreeMap<&'a str, &'a CompositeDef>,
    /// Each definition's instances by name (the top level's under its own name).
    instances: BTreeMap<&'a str, BTreeMap<&'a str, &'a InstanceDef>>,
    outputs: ByExport<'a, &'a str>,
    inputs: ByExport<'a, &'a [String]>,
    parameters: ByExport<'a, &'a [String]>,
    /// (flattened source port, flattened target) → the converter between.
    converters: BTreeMap<(String, String), Value>,
}

impl Project {
    /// The structure as written, of this project and its plan `plan`
    /// (`docs/ui-protocol.md` "graph"): `{"top": level, "composites":
    /// {name: level with "inputs", "outputs", "parameters"}}`. Instances and
    /// connections are in source order, composites in name order. Inside a
    /// composite, addresses are relative to an instance of it (`kick.out`);
    /// its connections' converters are those of its first instance in source
    /// order (a type's rates are the same in every instance).
    pub fn structure(&self, plan: &ExecutionPlan, registry: &Registry) -> Value {
        let type_of: BTreeMap<&str, &str> = plan
            .instances
            .iter()
            .map(|i| (i.address.as_str(), i.type_name))
            .collect();
        let mut converters = BTreeMap::new();
        for warning in &plan.warnings {
            if let GraphWarning::ConverterInserted {
                from,
                to,
                converter,
                conversion,
                from_rate,
                to_rate,
                ..
            } = warning
            {
                let name = converter.segments().last().unwrap_or_default();
                converters.insert(
                    (from.as_str().to_owned(), to.as_str().to_owned()),
                    json!({
                        "name": name,
                        "module": type_of.get(converter.as_str()),
                        "conversion": conversion.to_string(),
                        "fromRate": from_rate.get(),
                        "toRate": to_rate.get(),
                    }),
                );
            }
        }
        let mut instances = BTreeMap::new();
        let (mut outputs, mut inputs, mut parameters) =
            (ByExport::new(), ByExport::new(), ByExport::new());
        for def in std::iter::once(&self.root).chain(&self.composites) {
            let name = def.name.as_str();
            let by_name = def.instances.iter().map(|i| (i.name.as_str(), i));
            instances.insert(name, by_name.collect());
            for e in &def.exports.outputs {
                let all: &mut Vec<_> = outputs.entry((name, e.name.as_str())).or_default();
                all.push(e.from.as_str());
            }
            for e in &def.exports.inputs {
                let all: &mut Vec<_> = inputs.entry((name, e.name.as_str())).or_default();
                all.push(e.to.as_slice());
            }
            for e in &def.exports.parameters {
                let all: &mut Vec<_> = parameters.entry((name, e.name.as_str())).or_default();
                all.push(e.to.as_slice());
            }
        }
        let walk = Walk {
            project: self,
            registry,
            by_name: self
                .composites
                .iter()
                .map(|c| (c.name.as_str(), c))
                .collect(),
            instances,
            outputs,
            inputs,
            parameters,
            converters,
        };
        // The first instance (in source order) of every composite reached.
        let mut first = BTreeMap::new();
        walk.reach(&self.root, "", &mut first);
        let top = walk.level(&self.root, "", false);
        let mut composites = Map::new();
        for (name, prefix) in &first {
            let Some(def) = walk.by_name.get(name.as_str()) else {
                continue;
            };
            let mut level = walk.level(def, prefix, true);
            if let Value::Object(fields) = &mut level {
                fields.insert("inputs".to_owned(), walk.exported_inputs(def));
                fields.insert("outputs".to_owned(), walk.exported_outputs(def));
                fields.insert("parameters".to_owned(), walk.exported_parameters(def));
            }
            composites.insert(name.clone(), level);
        }
        json!({"top": top, "composites": composites})
    }

    /// `def`'s instances in source order.
    fn in_source_order<'a>(&self, def: &'a CompositeDef) -> Vec<&'a signal_graph::InstanceDef> {
        let spans = if def.name.is_empty() {
            Some(&self.spans.root)
        } else {
            self.spans.composites.get(&def.name)
        };
        let mut instances: Vec<_> = def.instances.iter().collect();
        instances.sort_by_key(|i| {
            spans
                .and_then(|s| s.instances.get(&i.name))
                .map_or(usize::MAX, |s| s.key.start)
        });
        instances
    }
}

impl Walk<'_> {
    fn reach(&self, def: &CompositeDef, prefix: &str, first: &mut BTreeMap<String, String>) {
        for instance in self.project.in_source_order(def) {
            if let Some(inner) = self.by_name.get(instance.module.as_str()) {
                let address = format!("{prefix}/{}", instance.name);
                if !first.contains_key(&inner.name) {
                    first.insert(inner.name.clone(), address.clone());
                    self.reach(inner, &address, first);
                }
            }
        }
    }

    /// A level: its instances and connections. `prefix` is the address of an
    /// instance of it (empty for the top level); `relative` names its parts
    /// without the leading `/`.
    fn level(&self, def: &CompositeDef, prefix: &str, relative: bool) -> Value {
        let lead = if relative { "" } else { "/" };
        let instances: Vec<Value> = self
            .project
            .in_source_order(def)
            .into_iter()
            .map(|instance| {
                let name = &instance.name;
                let at = |port: &str| format!("{lead}{name}.{port}");
                let (composite, ports, parameters) =
                    match self.by_name.get(instance.module.as_str()) {
                        Some(inner) => {
                            let mut ports = Vec::new();
                            for export in &inner.exports.inputs {
                                let found = export.to.first().and_then(|t| self.endpoint(inner, t));
                                ports.push(port_json(
                                    &export.name,
                                    &at(&export.name),
                                    Direction::In,
                                    found,
                                ));
                            }
                            for export in &inner.exports.outputs {
                                let found = self.endpoint(inner, &export.from);
                                ports.push(port_json(
                                    &export.name,
                                    &at(&export.name),
                                    Direction::Out,
                                    found,
                                ));
                            }
                            let parameters = inner
                                .exports
                                .parameters
                                .iter()
                                .filter_map(|export| {
                                    let target = export.to.first()?;
                                    match self.endpoint(inner, target)? {
                                        Endpoint::Parameter(p) => {
                                            Some(parameter_json(p, &export.name, &at(&export.name)))
                                        }
                                        Endpoint::Port(_) => None,
                                    }
                                })
                                .collect();
                            (true, ports, parameters)
                        }
                        None => match self.registry.get(&instance.module) {
                            Some(module) => (
                                false,
                                module
                                    .descriptor
                                    .ports
                                    .iter()
                                    .map(|p| {
                                        port_json(
                                            p.name,
                                            &at(p.name),
                                            p.direction,
                                            Some(Endpoint::Port(p)),
                                        )
                                    })
                                    .collect(),
                                module
                                    .descriptor
                                    .parameters
                                    .iter()
                                    .map(|p| parameter_json(p, p.name, &at(p.name)))
                                    .collect(),
                            ),
                            None => (false, Vec::new(), Vec::new()),
                        },
                    };
                json!({
                    "name": name,
                    "address": format!("{lead}{name}"),
                    "module": instance.module,
                    "composite": composite,
                    "ports": ports,
                    "parameters": parameters,
                })
            })
            .collect();
        let connections: Vec<Value> = def
            .connections
            .iter()
            .map(|c| {
                let mut converters: Vec<Value> = Vec::new();
                for from in self.primitives(def, prefix, &c.from, true) {
                    for to in self.primitives(def, prefix, &c.to, false) {
                        if let Some(entry) = self.converters.get(&(from.clone(), to))
                            && !converters.contains(entry)
                        {
                            converters.push(entry.clone());
                        }
                    }
                }
                json!({
                    "from": format!("{lead}{}", c.from),
                    "to": format!("{lead}{}", c.to),
                    "scale": c.scale,
                    "converters": converters,
                })
            })
            .collect();
        json!({"instances": instances, "connections": connections})
    }

    fn exported_inputs(&self, def: &CompositeDef) -> Value {
        Value::Array(
            def.exports
                .inputs
                .iter()
                .map(|e| {
                    let found = e.to.first().and_then(|t| self.endpoint(def, t));
                    with_targets(
                        port_json(&e.name, &e.name, Direction::In, found),
                        "to",
                        json!(e.to),
                    )
                })
                .collect(),
        )
    }

    fn exported_outputs(&self, def: &CompositeDef) -> Value {
        Value::Array(
            def.exports
                .outputs
                .iter()
                .map(|e| {
                    let found = self.endpoint(def, &e.from);
                    with_targets(
                        port_json(&e.name, &e.name, Direction::Out, found),
                        "from",
                        json!(e.from),
                    )
                })
                .collect(),
        )
    }

    fn exported_parameters(&self, def: &CompositeDef) -> Value {
        Value::Array(
            def.exports
                .parameters
                .iter()
                .map(|e| json!({"name": e.name, "to": e.to}))
                .collect(),
        )
    }

    /// The primitive port or parameter that endpoint `text` of `def` reaches
    /// (an export's first target, recursively).
    fn endpoint(&self, def: &CompositeDef, text: &str) -> Option<Endpoint> {
        let (instance, port) = text.split_once('.')?;
        let instance = self.instance(def, instance)?;
        if let Some(inner) = self.by_name.get(instance.module.as_str()) {
            let key = (inner.name.as_str(), port);
            let target = self
                .outputs
                .get(&key)
                .and_then(|all| all.first().copied())
                .or_else(|| first_target(&self.inputs, key))
                .or_else(|| first_target(&self.parameters, key))?;
            return self.endpoint(inner, target);
        }
        let descriptor = &self.registry.get(&instance.module)?.descriptor;
        descriptor
            .ports
            .iter()
            .find(|p| p.name == port)
            .map(Endpoint::Port)
            .or_else(|| {
                descriptor
                    .parameters
                    .iter()
                    .find(|p| p.name == port)
                    .map(Endpoint::Parameter)
            })
    }

    /// The flattened primitive ports endpoint `text` of `def` reaches, `def`
    /// instantiated at `prefix`: one for a source (`out`), every target of a
    /// fanned-out export for a target.
    fn primitives(&self, def: &CompositeDef, prefix: &str, text: &str, out: bool) -> Vec<String> {
        let Some((name, port)) = text.split_once('.') else {
            return Vec::new();
        };
        let Some(instance) = self.instance(def, name) else {
            return Vec::new();
        };
        let address = format!("{prefix}/{name}");
        let Some(inner) = self.by_name.get(instance.module.as_str()) else {
            return vec![format!("{address}.{port}")];
        };
        let key = (inner.name.as_str(), port);
        if out {
            self.outputs
                .get(&key)
                .into_iter()
                .flatten()
                .flat_map(|from| self.primitives(inner, &address, from, true))
                .collect()
        } else {
            self.inputs
                .get(&key)
                .into_iter()
                .chain(self.parameters.get(&key))
                .flatten()
                .flat_map(|targets| targets.iter())
                .flat_map(|t| self.primitives(inner, &address, t, false))
                .collect()
        }
    }

    /// Instance `name` of `def`.
    fn instance(&self, def: &CompositeDef, name: &str) -> Option<&'_ InstanceDef> {
        self.instances.get(def.name.as_str())?.get(name).copied()
    }
}

/// The first target of the first export `key` in `all`.
fn first_target<'a>(all: &ByExport<'a, &'a [String]>, key: (&str, &str)) -> Option<&'a str> {
    all.get(&key)?.first()?.first().map(String::as_str)
}

/// A port as `describe` gives it (`name`, `direction`, `shape`, `rate`,
/// `kind`, `unit`, `range`) with its `address`; an export takes the shape of
/// what it reaches, and one that reaches a parameter is a signal input.
fn port_json(name: &str, address: &str, direction: Direction, found: Option<Endpoint>) -> Value {
    let mut value = match found {
        Some(Endpoint::Port(p)) => serde_json::to_value(p).unwrap_or(Value::Null),
        Some(Endpoint::Parameter(p)) => json!({
            "shape": "signal", "rate": p.rate, "kind": p.kind, "unit": p.unit, "range": p.range,
        }),
        None => json!({"shape": null, "rate": null, "kind": null, "unit": null, "range": null}),
    };
    if let Value::Object(fields) = &mut value {
        fields.insert("name".to_owned(), json!(name));
        fields.insert("direction".to_owned(), json!(direction));
        fields.insert("address".to_owned(), json!(address));
    }
    value
}

fn parameter_json(p: &ParameterDescriptor, name: &str, address: &str) -> Value {
    let mut value = serde_json::to_value(p).unwrap_or(Value::Null);
    if let Value::Object(fields) = &mut value {
        fields.insert("name".to_owned(), json!(name));
        fields.insert("address".to_owned(), json!(address));
    }
    value
}

fn with_targets(mut value: Value, key: &str, targets: Value) -> Value {
    if let Value::Object(fields) = &mut value {
        fields.insert(key.to_owned(), targets);
    }
    value
}
