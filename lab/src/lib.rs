//! Signal's lab tier (ADR 0015): experimental primitives, named `lab.*`.
//! Everything here compiles in only with the `lab` feature; without it the
//! crate is empty. Promotion to `std` requires the contract suite,
//! benchmarks and a recorded human verdict.
//!
//! - `lab.gpu_partials` ([`gpu_partials`]): a bank of 1 024 sine partials
//!   computed per block on the GPU, with a declared latency (ADR 0019 §3).
//! - `lab.tanh_adaa2`, `lab.tanh_os2_adaa1`, `lab.tanh_os4_adaa1`,
//!   `lab.tanh_os8` ([`tanh_shapers`]): the M3 "ADAA versus oversampling"
//!   candidates for `std.tanh_shaper` (`experiments/m3-tanh-aliasing/`) that
//!   were not promoted; the one that was is `std.tanh_shaper_os8`.

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
pub mod tanh_shapers;

#[cfg(feature = "lab")]
use signal_core::{
    DirectPrimitive, ModuleType, Primitive, Registry, RegistryError, StateOutputPrimitive,
};

/// The `lab.*` types that run on a GPU device: what a
/// `signal_gpu::GpuHost` places (M2.7-2).
#[cfg(feature = "lab")]
pub const GPU_TYPES: [&str; 1] = ["lab.gpu_partials"];

/// Registers every `lab.*` primitive type in `registry`.
#[cfg(feature = "lab")]
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    use tanh_shapers::{Os2Adaa1, Os4Adaa1, Os8, OversampledShaper, TanhAdaa2};
    registry.register(state_output::<gpu_partials::GpuPartials>())?;
    registry.register(direct::<TanhAdaa2>())?;
    registry.register(direct::<OversampledShaper<Os2Adaa1>>())?;
    registry.register(direct::<OversampledShaper<Os4Adaa1>>())?;
    registry.register(direct::<OversampledShaper<Os8>>())
}

#[cfg(feature = "lab")]
fn direct<T: DirectPrimitive + Default + 'static>() -> ModuleType {
    ModuleType {
        descriptor: T::describe(),
        create: || Primitive::Direct(Box::new(T::default())),
    }
}

#[cfg(feature = "lab")]
fn state_output<T: StateOutputPrimitive + Default + 'static>() -> ModuleType {
    ModuleType {
        descriptor: T::describe(),
        create: || Primitive::StateOutput(Box::new(T::default())),
    }
}
