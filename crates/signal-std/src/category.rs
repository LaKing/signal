//! The palette groups of the standard library (ADR 0027 §1): the values of
//! [`ModuleDescriptor::category`](signal_core::ModuleDescriptor::category)
//! that `std.*` types carry, and that `signal modules --category` filters by.
//!
//! The vocabulary lives here, never in the domain-free crates (AGENTS.md
//! rule 1): to the core a category is a free-form tag. `core` is a palette
//! group, not ADR 0027's core set: core-set members that hide a loop or carry
//! events are in their own groups (e.g. `std.delay` in `delay`, `std.edge` in
//! `event`).

/// Stateless arithmetic, functions, constants, comparison and selection, and
/// the basic mixing utilities.
pub const CORE: &str = "core";
/// Oscillators and noise.
pub const GENERATOR: &str = "generator";
/// Filters, smoothers and shapers.
pub const FILTER: &str = "filter";
/// Envelopes.
pub const ENVELOPE: &str = "envelope";
/// Delay lines.
pub const DELAY: &str = "delay";
/// Physical models and their exciters.
pub const PHYSICAL: &str = "physical";
/// Triggers, clocks, sequencers and the event bridge.
pub const EVENT: &str = "event";
/// Visual-rate types (vectors for `<signal-canvas>`).
pub const VISUAL: &str = "visual";
/// Outputs to the outside world.
pub const IO: &str = "io";

/// Every category, in the order above.
pub const ALL: [&str; 9] = [
    CORE, GENERATOR, FILTER, ENVELOPE, DELAY, PHYSICAL, EVENT, VISUAL, IO,
];
