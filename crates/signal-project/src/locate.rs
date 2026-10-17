//! Where graph errors and warnings come from in the files.
//!
//! The graph reports hierarchical addresses. An address is resolved by walking
//! its segments from the plan's root through each instance's module to the
//! composite that declares the next segment, which gives the file. Within that
//! definition, an endpoint is found by its text (`inst.port`).

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use signal_core::{Direction, InstanceAddress, PortAddress};
use signal_graph::{GraphError, GraphWarning};

/// A byte range in a file.
pub(crate) type Span = Range<usize>;

/// A position: a file (index into the project's files) and a byte offset.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Location {
    pub(crate) file: usize,
    pub(crate) offset: usize,
}

/// The spans of one definition: the project root or a composite.
pub(crate) struct DefSpans {
    pub(crate) file: usize,
    /// The composite's `name` value; `0..0` for the project root.
    pub(crate) name: Span,
    /// The `[instances]` table, if present.
    pub(crate) instances_table: Option<Span>,
    pub(crate) instances: BTreeMap<String, InstanceSpans>,
    pub(crate) connections: Vec<ConnectionSpans>,
    pub(crate) exports: Vec<ExportSpans>,
}

/// The spans of one instance.
pub(crate) struct InstanceSpans {
    pub(crate) key: Span,
    pub(crate) module: String,
    pub(crate) module_span: Span,
    /// Key and value span of each parameter set on the instance.
    pub(crate) parameters: BTreeMap<String, (Span, Span)>,
    /// Its `gpu` pin (M2.7-2) and the value's span.
    pub(crate) gpu: Option<(String, Span)>,
    /// The pattern it names (ADR 0028 §3) and the value's span.
    pub(crate) pattern: Option<(String, Span)>,
}

/// The spans of one connection.
pub(crate) struct ConnectionSpans {
    pub(crate) entry: Span,
    pub(crate) from: (String, Span),
    pub(crate) to: (String, Span),
    /// The scale's value and span, if given.
    pub(crate) scale: Option<(f64, Span)>,
}

/// What an export exposes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExportKind {
    Input,
    Output,
    Parameter,
}

/// The spans of one export.
pub(crate) struct ExportSpans {
    pub(crate) kind: ExportKind,
    pub(crate) name: String,
    pub(crate) key: Span,
    pub(crate) targets: Vec<(String, Span)>,
}

/// The spans of every definition of a project.
pub(crate) struct SourceMap {
    pub(crate) root: DefSpans,
    pub(crate) composites: BTreeMap<String, DefSpans>,
}

/// The root of a plan: the project, or a synthetic root that instantiates one
/// composite once under its own name.
#[derive(Clone, Copy)]
pub(crate) enum Root<'a> {
    Project,
    Composite(&'a str),
}

/// Which side of a connection an endpoint is on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Want {
    Source,
    Target,
    Any,
}

/// Resolves graph addresses of a plan built from `root`.
pub(crate) struct Locator<'a> {
    pub(crate) map: &'a SourceMap,
    pub(crate) root: Root<'a>,
}

fn at(def: &DefSpans, offset: usize) -> Location {
    Location {
        file: def.file,
        offset,
    }
}

fn segments(address: &InstanceAddress) -> Vec<String> {
    address.segments().map(str::to_owned).collect()
}

fn scope_segments(scope: &Option<InstanceAddress>) -> Vec<String> {
    scope.as_ref().map(segments).unwrap_or_default()
}

/// The instance part of an endpoint `inst.port`.
fn endpoint_instance(text: &str) -> &str {
    text.split_once('.').map_or(text, |(instance, _)| instance)
}

impl<'a> Locator<'a> {
    /// The location of `error`; the root's own location when it cannot be
    /// narrowed down.
    pub(crate) fn error(&self, error: &GraphError) -> Location {
        self.try_error(error).unwrap_or_else(|| self.fallback())
    }

    /// The location of `warning`.
    pub(crate) fn warning(&self, warning: &GraphWarning) -> Location {
        let location = match warning {
            GraphWarning::KindMismatch { from, to, .. }
            | GraphWarning::ConverterInserted { from, to, .. } => {
                self.connection_or_instance(from, to)
            }
            GraphWarning::NoSinkReachable { instance } => self.instance_key(instance),
            _ => None,
        };
        location.unwrap_or_else(|| self.fallback())
    }

    /// The declaration of the instance at `address`.
    pub(crate) fn declaration(&self, address: &InstanceAddress) -> Location {
        self.instance_key(address)
            .unwrap_or_else(|| self.fallback())
    }

    fn try_error(&self, error: &GraphError) -> Option<Location> {
        match error {
            GraphError::DuplicateComposite { name } => self.composite_name(name),
            GraphError::AmbiguousModule { module } => self.composite_name(module),
            GraphError::UnknownModule { instance, .. }
            | GraphError::VectorTooWide { instance, .. }
            | GraphError::RecursiveComposite { instance, .. } => {
                let (def, instance) = self.instance(instance)?;
                Some(at(def, instance.module_span.start))
            }
            GraphError::InvalidName { scope, name } => self.named(scope, name),
            GraphError::DuplicateInstance { scope, name } => {
                let def = self.def_at(&scope_segments(scope))?;
                Some(at(def, def.instances.get(name)?.key.start))
            }
            GraphError::ExportNameCollision { scope, name, .. }
            | GraphError::EmptyExport { scope, name, .. } => {
                let def = self.def_at(&scope_segments(scope))?;
                let export = def.exports.iter().rev().find(|e| e.name == *name)?;
                Some(at(def, export.key.start))
            }
            GraphError::InvalidEndpoint { scope, endpoint } => {
                let def = self.def_at(&scope_segments(scope))?;
                endpoint_in(def, |text| text == endpoint, Want::Any)
            }
            GraphError::UnknownInstance { scope, name } => {
                let def = self.def_at(&scope_segments(scope))?;
                endpoint_in(def, |text| endpoint_instance(text) == name, Want::Any)
            }
            GraphError::UnknownPort { port } => {
                let (def, local) = self.port_scope(port)?;
                endpoint_in(def, |text| text == local, Want::Any)
            }
            GraphError::UnknownParameter { parameter } => {
                self.parameter_key(parameter).or_else(|| {
                    let (def, local) = self.port_scope(parameter)?;
                    endpoint_in(def, |text| text == local, Want::Target)
                })
            }
            GraphError::WrongDirection { port, expected } => {
                let (def, local) = self.port_scope(port)?;
                let want = match expected {
                    Direction::Out => Want::Source,
                    Direction::In => Want::Target,
                };
                endpoint_in(def, |text| text == local, want)
            }
            GraphError::ShapeMismatch { from, to, .. }
            | GraphError::RateMismatch { from, to, .. } => self.connection_or_instance(from, to),
            GraphError::ParameterOutOfRange { parameter, .. } => self.winning_value(parameter),
            GraphError::NonFiniteScale { from, to, scale } => {
                // The connection with this very scale, not merely the first
                // one between the same ports.
                let (def, connection) = self
                    .connection_where(from, to, |c| {
                        c.scale
                            .as_ref()
                            .is_some_and(|(value, _)| value.to_bits() == scale.to_bits())
                    })
                    .or_else(|| self.connection(from, to))?;
                let span = connection
                    .scale
                    .as_ref()
                    .map_or(&connection.entry, |(_, span)| span);
                Some(at(def, span.start))
            }
            GraphError::RestrictedInput {
                from, to, scale, ..
            } => {
                // The connection as written, with its scale (one of several
                // between the same ports otherwise).
                let (def, connection) = self
                    .connection_where(from, to, |c| {
                        c.scale.as_ref().map_or(1.0, |(value, _)| *value).to_bits()
                            == scale.to_bits()
                    })
                    .or_else(|| self.connection(from, to))?;
                Some(at(def, connection.entry.start))
            }
            GraphError::Cycle { instances } => self.instance_key(instances.first()?),
            _ => None,
        }
    }

    /// Where an error about the root itself goes: `project.toml` line 1, or
    /// the composite's `name` for a synthetic root.
    fn fallback(&self) -> Location {
        match self.root {
            Root::Composite(name) => self.composite_name(name),
            Root::Project => None,
        }
        .unwrap_or_else(|| at(&self.map.root, 0))
    }

    fn composite_name(&self, name: &str) -> Option<Location> {
        let def = self.map.composites.get(name)?;
        Some(at(def, def.name.start))
    }

    /// The definition instantiated at the instance address `segments` (empty:
    /// the root). `None` for the synthetic root, which has no file.
    fn def_at(&self, segments: &[String]) -> Option<&'a DefSpans> {
        let (mut def, rest) = match self.root {
            Root::Project => (&self.map.root, segments),
            Root::Composite(name) => {
                let (first, rest) = segments.split_first()?;
                if first != name {
                    return None;
                }
                (self.map.composites.get(name)?, rest)
            }
        };
        for segment in rest {
            let module = &def.instances.get(segment)?.module;
            def = self.map.composites.get(module)?;
        }
        Some(def)
    }

    /// The declaration of the instance at `address`, and the definition
    /// declaring it.
    fn instance(&self, address: &InstanceAddress) -> Option<(&'a DefSpans, &'a InstanceSpans)> {
        let segments = segments(address);
        let (last, scope) = segments.split_last()?;
        let def = self.def_at(scope)?;
        Some((def, def.instances.get(last)?))
    }

    /// The `gpu` pin of the instance at `address`, if it has one.
    pub(crate) fn pin(&self, address: &InstanceAddress) -> Option<&'a str> {
        let (_, instance) = self.instance(address)?;
        instance.gpu.as_ref().map(|(pin, _)| pin.as_str())
    }

    /// The pattern the instance at `address` names, if any.
    pub(crate) fn pattern(&self, address: &InstanceAddress) -> Option<&'a str> {
        let (_, instance) = self.instance(address)?;
        instance.pattern.as_ref().map(|(name, _)| name.as_str())
    }

    fn instance_key(&self, address: &InstanceAddress) -> Option<Location> {
        let (def, instance) = self.instance(address)?;
        Some(at(def, instance.key.start))
    }

    fn parameter_key(&self, parameter: &PortAddress) -> Option<Location> {
        let (def, instance) = self.instance(&parameter.instance())?;
        let (key, _) = instance.parameters.get(parameter.port())?;
        Some(at(def, key.start))
    }

    /// The definition declaring `port`'s instance, and the endpoint text that
    /// names `port` there.
    fn port_scope(&self, port: &PortAddress) -> Option<(&'a DefSpans, String)> {
        let segments = segments(&port.instance());
        let (last, scope) = segments.split_last()?;
        Some((self.def_at(scope)?, format!("{last}.{}", port.port())))
    }

    /// An instance, export or parameter name declared in the definition at
    /// `scope`, or a parameter set on the instance at `scope`.
    fn named(&self, scope: &Option<InstanceAddress>, name: &str) -> Option<Location> {
        let def = self.def_at(&scope_segments(scope));
        let declared = def.and_then(|def| {
            let key = def
                .instances
                .get(name)
                .map(|i| &i.key)
                .or_else(|| def.exports.iter().find(|e| e.name == name).map(|e| &e.key))?;
            Some(at(def, key.start))
        });
        declared.or_else(|| {
            let (def, instance) = self.instance(scope.as_ref()?)?;
            let (key, _) = instance.parameters.get(name)?;
            Some(at(def, key.start))
        })
    }

    /// The declared connection `from -> to`, both endpoints in one definition.
    fn connection(
        &self,
        from: &PortAddress,
        to: &PortAddress,
    ) -> Option<(&'a DefSpans, &'a ConnectionSpans)> {
        self.connection_where(from, to, |_| true)
    }

    /// The first declared connection `from -> to` that `accept`s, both
    /// endpoints in one definition (the same pair may be connected twice).
    fn connection_where(
        &self,
        from: &PortAddress,
        to: &PortAddress,
        accept: impl Fn(&ConnectionSpans) -> bool,
    ) -> Option<(&'a DefSpans, &'a ConnectionSpans)> {
        let (def, from_text) = self.port_scope(from)?;
        let (to_def, to_text) = self.port_scope(to)?;
        if !std::ptr::eq(def, to_def) {
            return None;
        }
        let connection = def
            .connections
            .iter()
            .find(|c| c.from.0 == from_text && c.to.0 == to_text && accept(c))?;
        Some((def, connection))
    }

    /// The declared connection, else the target's or the source's instance.
    fn connection_or_instance(&self, from: &PortAddress, to: &PortAddress) -> Option<Location> {
        self.connection(from, to)
            .map(|(def, connection)| at(def, connection.entry.start))
            .or_else(|| self.instance_key(&to.instance()))
            .or_else(|| self.instance_key(&from.instance()))
    }

    /// The value that set `parameter`: the outermost enclosing composite
    /// instance whose parameter reaches it through exports (outermost wins in
    /// flattening), else the primitive instance's own value, else its key.
    fn winning_value(&self, parameter: &PortAddress) -> Option<Location> {
        let address = parameter.instance();
        let segments = segments(&address);
        let name = parameter.port();
        for depth in 1..segments.len() {
            let (scope, rest) = segments.split_at(depth);
            let Some((last, outer)) = scope.split_last() else {
                continue;
            };
            let Some(def) = self.def_at(outer) else {
                continue;
            };
            let Some(instance) = def.instances.get(last) else {
                continue;
            };
            let Some(composite) = self.map.composites.get(&instance.module) else {
                continue;
            };
            let exports = self.reaching_exports(composite, rest, name);
            // The instance's values are assigned in key order, so of several
            // aliases of one inner parameter the last key wins.
            if let Some((_, value)) = instance
                .parameters
                .iter()
                .rev()
                .find(|(key, _)| exports.contains(*key))
                .map(|(_, spans)| spans)
            {
                return Some(at(def, value.start));
            }
        }
        let (def, instance) = self.instance(&address)?;
        let span = instance
            .parameters
            .get(name)
            .map_or(&instance.key, |(_, value)| value);
        Some(at(def, span.start))
    }

    /// The exported parameters of `def` that reach parameter `name` of the
    /// instance at relative path `path` inside it.
    fn reaching_exports(&self, def: &DefSpans, path: &[String], name: &str) -> BTreeSet<String> {
        let Some((first, more)) = path.split_first() else {
            return BTreeSet::new();
        };
        let inner: BTreeSet<String> = if more.is_empty() {
            BTreeSet::from([name.to_owned()])
        } else {
            match def
                .instances
                .get(first)
                .and_then(|i| self.map.composites.get(&i.module))
            {
                Some(composite) => self.reaching_exports(composite, more, name),
                None => return BTreeSet::new(),
            }
        };
        def.exports
            .iter()
            .filter(|e| e.kind == ExportKind::Parameter)
            .filter(|e| {
                e.targets.iter().any(|(text, _)| {
                    text.split_once('.')
                        .is_some_and(|(instance, port)| instance == first && inner.contains(port))
                })
            })
            .map(|e| e.name.clone())
            .collect()
    }
}

/// The first endpoint in `def` whose text matches, preferring the wanted side
/// (a connection's `from` or an output export is a source; a connection's `to`
/// or an input or parameter export target is a target).
fn endpoint_in(def: &DefSpans, matches: impl Fn(&str) -> bool, want: Want) -> Option<Location> {
    let connections = def.connections.iter().flat_map(|c| {
        [
            (&c.from.0, &c.from.1, Want::Source),
            (&c.to.0, &c.to.1, Want::Target),
        ]
    });
    let exports = def.exports.iter().flat_map(|e| {
        let side = match e.kind {
            ExportKind::Output => Want::Source,
            ExportKind::Input | ExportKind::Parameter => Want::Target,
        };
        e.targets.iter().map(move |(text, span)| (text, span, side))
    });
    let candidates: Vec<_> = connections
        .chain(exports)
        .filter(|(text, _, _)| matches(text))
        .collect();
    candidates
        .iter()
        .find(|(_, _, side)| want == Want::Any || *side == want)
        .or_else(|| candidates.first())
        .map(|(_, span, _)| at(def, span.start))
}
