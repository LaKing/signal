//! Primitive module types by name. Building the registry allocates; it is not
//! real time.

use std::any::Any;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::address::InstanceAddress;
use crate::convert::{self, CONVERTER_PREFIX};
use crate::descriptor::{Feedthrough, ModuleDescriptor};
use crate::module::Primitive;
use crate::table::TableRule;

/// The namespace of module references that name composites shipped with the
/// program rather than primitive types (ADR 0027 §4): reserved, so no type can
/// be registered under it. To the core it is only a reserved prefix; what it
/// resolves to is the project loader's.
pub const LIBRARY_PREFIX: &str = "lib.";

/// Something a [`Host`] lends one instance (M2.7-2): opaque to the engine,
/// which passes it to `prepare` in [`PrepareContext::lent`](crate::PrepareContext::lent)
/// and never reads it.
pub type Lent = Arc<dyn Any + Send + Sync>;

/// What the program running the engine lends its instances on the control
/// side (M2.7-2: GPU devices, placed by address). The engine calls
/// [`lend`](Host::lend) once for every plan state it builds (in real time
/// twice per plan: the engine's and its mirror's), before it prepares any
/// new instance of it, with every instance of the plan in address order and
/// its type name. Answers must depend only on those, so both calls agree.
pub trait Host: Send + Sync + fmt::Debug {
    /// What each of `instances` is lent, by index; a shorter answer lends
    /// nothing to the rest.
    fn lend(&self, instances: &[(&InstanceAddress, &'static str)]) -> Vec<Option<Lent>>;
}

/// A primitive module type: its descriptor and a constructor for instances.
///
/// `create` returns an unprepared instance whose trait matches
/// `descriptor.feedthrough`; the engine calls `prepare` before any real-time call.
#[derive(Clone, Copy, Debug)]
pub struct ModuleType {
    /// What the type declares; `descriptor.type_name` is its registry key.
    pub descriptor: ModuleDescriptor,
    /// Creates a new instance.
    pub create: fn() -> Primitive,
}

/// Module types keyed by type name, iterated in name order, with each type's
/// static cost estimate where the library that registers it gives one, and
/// its restricted inputs (ADR 0024).
#[derive(Clone, Debug, Default)]
pub struct Registry {
    types: BTreeMap<&'static str, ModuleType>,
    costs: BTreeMap<&'static str, u32>,
    /// (type, input) → (source type, source output).
    inputs: BTreeMap<(&'static str, &'static str), (&'static str, &'static str)>,
    /// type → what it takes as its table (ADR 0028 §3).
    tables: BTreeMap<&'static str, TableRule>,
    /// (type, input, input): at most one of the two has connections
    /// (ADR 0033).
    exclusive: Vec<(&'static str, &'static str, &'static str)>,
    /// sink type → its tap type (ADR 0035 §3).
    taps: BTreeMap<&'static str, &'static str>,
    host: Option<Arc<dyn Host>>,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Adds `module_type`, refusing a type name that is already registered,
    /// or one in the converters' reserved `core.` namespace (M2.8-6) or the
    /// library's reserved `lib.` namespace ([`LIBRARY_PREFIX`]).
    /// Refuses a declared latency that is not a state-output type's, or whose
    /// `latency_blocks` or `block_frames` is 0 (ADR 0019 §3).
    pub fn register(&mut self, module_type: ModuleType) -> Result<(), RegistryError> {
        let descriptor = module_type.descriptor;
        let name = descriptor.type_name;
        if self.types.contains_key(name) {
            return Err(RegistryError::DuplicateTypeName(name));
        }
        if name.starts_with(CONVERTER_PREFIX) || name.starts_with(LIBRARY_PREFIX) {
            return Err(RegistryError::ReservedTypeName(name));
        }
        if let Some(latency) = descriptor.latency
            && (descriptor.feedthrough != Feedthrough::StateOutput
                || latency.latency_blocks == 0
                || latency.block_frames == 0)
        {
            return Err(RegistryError::InvalidLatency(name));
        }
        self.types.insert(name, module_type);
        Ok(())
    }

    /// The type registered under `type_name`, or the rate converter of that
    /// canonical name in the reserved `core.` namespace
    /// ([`converter_name`](crate::converter_name)): converters are known to
    /// every registry without being registered or listed by
    /// [`iter`](Registry::iter). A malformed `core.` name is `None`, like any
    /// unknown name. Not for the audio thread: a converter is made on first
    /// use, under a lock.
    pub fn get(&self, type_name: &str) -> Option<&ModuleType> {
        match self.types.get(type_name) {
            Some(found) => Some(found),
            None if type_name.starts_with(CONVERTER_PREFIX) => convert::named(type_name),
            None => None,
        }
    }

    /// Records `type_name`'s static cost: nanoseconds per frame per instance,
    /// as measured for `docs/benchmarks.md`. The engine groups cheap units
    /// into tasks by it (ADR 0019 §2); it never changes the output.
    pub fn set_cost(&mut self, type_name: &'static str, ns_per_frame: u32) {
        self.costs.insert(type_name, ns_per_frame);
    }

    /// The static cost recorded for `type_name`, if any.
    pub fn cost(&self, type_name: &str) -> Option<u32> {
        self.costs.get(type_name).copied()
    }

    /// Restricts input `port` of `type_name` (ADR 0024): a plan is built only
    /// if it takes at most one connection there, at scale exactly 1, from
    /// output `from_port` of a `from_type` instance. A structural rule: the
    /// graph compares the names and nothing else.
    pub fn restrict_input(
        &mut self,
        type_name: &'static str,
        port: &'static str,
        from_type: &'static str,
        from_port: &'static str,
    ) {
        self.inputs
            .insert((type_name, port), (from_type, from_port));
    }

    /// The source (type, output) input `port` of `type_name` is restricted
    /// to, if it is ([`restrict_input`](Registry::restrict_input)).
    pub fn input_rule(&self, type_name: &str, port: &str) -> Option<(&'static str, &'static str)> {
        self.inputs
            .iter()
            .find(|((t, p), _)| *t == type_name && *p == port)
            .map(|(_, rule)| *rule)
    }

    /// Makes inputs `a` and `b` of `type_name` exclusive (ADR 0033): a plan
    /// is built only if at most one of them has connections on each
    /// instance. A structural rule, as [`restrict_input`](Registry::restrict_input):
    /// the graph compares the names and nothing else; what the two inputs
    /// mean is the library's.
    pub fn exclusive_inputs(&mut self, type_name: &'static str, a: &'static str, b: &'static str) {
        if !self
            .exclusive
            .iter()
            .any(|&(t, x, y)| t == type_name && ((x, y) == (a, b) || (x, y) == (b, a)))
        {
            self.exclusive.push((type_name, a, b));
        }
    }

    /// The pairs of exclusive inputs of `type_name`
    /// ([`exclusive_inputs`](Registry::exclusive_inputs)), in the order
    /// they were declared.
    pub fn exclusive_pairs(
        &self,
        type_name: &str,
    ) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
        let type_name = type_name.to_owned();
        self.exclusive
            .iter()
            .filter(move |(t, _, _)| *t == type_name)
            .map(|&(_, a, b)| (a, b))
    }

    /// Registers `tap` as the tap of the sink type `sink` (ADR 0035 §3): the
    /// type a project loader plans in place of a `sink` instance where that
    /// instance must not be a sink (inside a project included in another).
    /// A name-to-name table, domain-free like
    /// [`exclusive_inputs`](Registry::exclusive_inputs): what a tap computes
    /// is its library's (`docs/module-contract.md` "Sink taps"). A later
    /// registration for the same sink replaces the earlier one.
    pub fn set_sink_tap(&mut self, sink: &'static str, tap: &'static str) {
        self.taps.insert(sink, tap);
    }

    /// The tap registered for the sink type `sink`, if any
    /// ([`set_sink_tap`](Registry::set_sink_tap)).
    pub fn sink_tap(&self, sink: &str) -> Option<&'static str> {
        self.taps.get(sink).copied()
    }

    /// Declares that instances of `type_name` take a table (ADR 0028 §3, §4):
    /// the data of `rule.kind` a project names for the instance, compiled by
    /// `rule.compile` when the plan is built. A type without a rule takes none.
    pub fn set_table_rule(&mut self, type_name: &'static str, rule: TableRule) {
        self.tables.insert(type_name, rule);
    }

    /// What `type_name` takes as its table, if anything
    /// ([`set_table_rule`](Registry::set_table_rule)).
    pub fn table_rule(&self, type_name: &str) -> Option<TableRule> {
        self.tables.get(type_name).copied()
    }

    /// Sets what lends this registry's instances their resources (M2.7-2).
    pub fn set_host(&mut self, host: Arc<dyn Host>) {
        self.host = Some(host);
    }

    /// The host, if one is set.
    pub fn host(&self) -> Option<&Arc<dyn Host>> {
        self.host.as_ref()
    }

    /// Every registered type, in byte-wise order of type name.
    pub fn iter(&self) -> impl Iterator<Item = &ModuleType> {
        self.types.values()
    }

    /// The number of registered types.
    pub fn len(&self) -> usize {
        self.types.len()
    }

    /// Whether no type is registered.
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }
}

/// Why a module type could not be registered.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegistryError {
    /// A type with this name is already registered.
    DuplicateTypeName(&'static str),
    /// A declared latency on a type that is not state-output, or with 0
    /// blocks or 0 frames per block.
    InvalidLatency(&'static str),
    /// A name in the converters' reserved `core.` namespace (M2.8-6) or the
    /// library's reserved `lib.` namespace.
    ReservedTypeName(&'static str),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::ReservedTypeName(name) if name.starts_with(LIBRARY_PREFIX) => write!(
                f,
                "module type `{name}`: names starting with `{LIBRARY_PREFIX}` are reserved for library composites"
            ),
            RegistryError::ReservedTypeName(name) => write!(
                f,
                "module type `{name}`: names starting with `{CONVERTER_PREFIX}` are reserved for rate converters"
            ),
            RegistryError::DuplicateTypeName(name) => {
                write!(f, "module type `{name}` is already registered")
            }
            RegistryError::InvalidLatency(name) => write!(
                f,
                "module type `{name}` declares a latency, but it is not state-output or its blocks or block frames are 0"
            ),
        }
    }
}

impl std::error::Error for RegistryError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// M2.8-6: converters are known to every registry by canonical name,
    /// without being listed; the `core.` namespace cannot be registered.
    #[test]
    fn converters_are_served_and_their_namespace_reserved() {
        let mut registry = Registry::new();
        let hold = *registry.get("core.hold.d64.d1.signal.control").unwrap();
        assert_eq!(hold.descriptor.type_name, "core.hold.d64.d1.signal.control");
        assert!(registry.get("core.hold.d0.d1.signal.audio").is_none());
        assert!(registry.is_empty());
        let mut taken = hold;
        taken.descriptor.type_name = "core.x";
        assert_eq!(
            registry.register(taken),
            Err(RegistryError::ReservedTypeName("core.x"))
        );
    }

    /// M2.10-3 (ADR 0027 §4): `lib.` names library composites, so no type
    /// can be registered under it.
    #[test]
    fn the_library_namespace_is_reserved() {
        let mut registry = Registry::new();
        let mut taken = *registry.get("core.hold.d64.d1.signal.control").unwrap();
        taken.descriptor.type_name = "lib.tape_echo";
        let error = registry.register(taken).unwrap_err();
        assert_eq!(error, RegistryError::ReservedTypeName("lib.tape_echo"));
        assert!(error.to_string().contains("library composites"), "{error}");
        assert!(registry.get("lib.tape_echo").is_none());
    }
}
