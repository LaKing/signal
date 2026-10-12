//! Primitive module types by name. Building the registry allocates; it is not
//! real time.

use std::collections::BTreeMap;
use std::fmt;

use crate::descriptor::{Feedthrough, ModuleDescriptor};
use crate::module::Primitive;

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
/// static cost estimate where the library that registers it gives one.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    types: BTreeMap<&'static str, ModuleType>,
    costs: BTreeMap<&'static str, u32>,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Adds `module_type`, refusing a type name that is already registered.
    /// Refuses a declared latency that is not a state-output type's, or whose
    /// `latency_blocks` or `block_frames` is 0 (ADR 0019 §3).
    pub fn register(&mut self, module_type: ModuleType) -> Result<(), RegistryError> {
        let descriptor = module_type.descriptor;
        let name = descriptor.type_name;
        if self.types.contains_key(name) {
            return Err(RegistryError::DuplicateTypeName(name));
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

    /// The type registered under `type_name`.
    pub fn get(&self, type_name: &str) -> Option<&ModuleType> {
        self.types.get(type_name)
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
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
