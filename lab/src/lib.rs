//! Signal's lab tier (ADR 0015): experimental primitives, named `lab.*`.
//! Everything here compiles in only with the `lab` feature; without it the
//! crate is empty. Promotion to `std` requires the contract suite,
//! benchmarks and a recorded human verdict.
//!
//! - `lab.gpu_partials` ([`gpu_partials`]): a bank of 1 024 sine partials
//!   computed per block on the GPU, with a declared latency (ADR 0019 §3).

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

#[cfg(feature = "lab")]
pub mod gpu_partials;

#[cfg(feature = "lab")]
use signal_core::{ModuleType, Primitive, Registry, RegistryError, StateOutputPrimitive};

/// Registers every `lab.*` primitive type in `registry`.
#[cfg(feature = "lab")]
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry.register(state_output::<gpu_partials::GpuPartials>())
}

#[cfg(feature = "lab")]
fn state_output<T: StateOutputPrimitive + Default + 'static>() -> ModuleType {
    ModuleType {
        descriptor: T::describe(),
        create: || Primitive::StateOutput(Box::new(T::default())),
    }
}
