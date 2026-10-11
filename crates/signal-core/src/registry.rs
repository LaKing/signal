//! Primitive module types by name. Building the registry allocates; it is not
//! real time.

use std::collections::BTreeMap;
use std::fmt;

use crate::descriptor::ModuleDescriptor;
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

/// Module types keyed by type name, iterated in name order.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    types: BTreeMap<&'static str, ModuleType>,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Adds `module_type`, refusing a type name that is already registered.
    pub fn register(&mut self, module_type: ModuleType) -> Result<(), RegistryError> {
        let name = module_type.descriptor.type_name;
        if self.types.contains_key(name) {
            return Err(RegistryError::DuplicateTypeName(name));
        }
        self.types.insert(name, module_type);
        Ok(())
    }

    /// The type registered under `type_name`.
    pub fn get(&self, type_name: &str) -> Option<&ModuleType> {
        self.types.get(type_name)
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
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::DuplicateTypeName(name) => {
                write!(f, "module type `{name}` is already registered")
            }
        }
    }
}

impl std::error::Error for RegistryError {}
