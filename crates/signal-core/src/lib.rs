//! Signal core: the domain-free vocabulary shared by all Signal crates.
//!
//! Planned contents (M1): `Frame`, port and value types, events, hierarchical
//! addresses, the seeded RNG, the module contract, and manifest types.
//! See `docs/architecture.md` and `docs/adr/0002-frame-semantics-and-module-contract.md`.
//!
//! This crate knows frames, ports, events and addresses only. Musical, MIDI,
//! lighting or video concepts do not belong here (ADR 0001, risk R15).
