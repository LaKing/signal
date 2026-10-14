//! Signal core: the domain-free vocabulary shared by all Signal crates.
//!
//! - [`Frame`] and [`RateDivisor`]: the only notion of time (ADR 0003, 0005).
//! - [`InstanceAddress`] and [`PortAddress`]: hierarchical addresses whose order
//!   is the byte-wise order of their canonical text (ADR 0016 §1, §3).
//! - [`ModuleDescriptor`], [`PortDescriptor`], [`ParameterDescriptor`]: what a
//!   module type declares about itself (ADR 0004, 0005, 0016 §2, §4).
//! - [`Event`] and [`EventPayload`]: small, `Copy`, heap-free messages (ADR 0004, 0016 §4a).
//! - [`RngStream`]: the vendored, seeded per-instance random stream (ADR 0008).
//! - [`DirectPrimitive`], [`StateOutputPrimitive`] and their I/O views: the module
//!   contract (ADR 0002, 0016 §5), normative in `docs/module-contract.md`.
//! - [`ModuleType`] and [`Registry`]: primitive types by name, and the
//!   [`Host`] that lends instances what the program provides ([`Lent`]).
//!
//! This crate knows frames, ports, events and addresses only. Musical, MIDI,
//! lighting or video concepts do not belong here (ADR 0001, risk R15).
//!
//! Code that can run on the audio thread follows `docs/realtime-rules.md`; the
//! real-time lints below are enforced for the library (not for its tests).

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

mod address;
mod descriptor;
mod event;
mod module;
mod registry;
mod rng;
mod time;

pub use address::{AddressError, InstanceAddress, PortAddress};
pub use descriptor::{
    BlockLatency, Combine, Direction, Feedthrough, ModuleDescriptor, ParameterDescriptor,
    PortDescriptor, PortKind, PortShape, Range, Reproducibility, SinkKind,
};
pub use event::{Curve, Event, EventPayload};
pub use module::{
    DirectPrimitive, Due, EventSink, Inputs, LossCause, Outputs, PrepareContext, Primitive,
    StateOutputPrimitive,
};
pub use registry::{Host, Lent, ModuleType, Registry, RegistryError};
pub use rng::{Fnv1a64, RngStream, SplitMix64, Xoshiro256PlusPlus, fnv1a64, fnv1a64_words};
pub use time::{Frame, RateDivisor};
