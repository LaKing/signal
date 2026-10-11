# ADR 0017: Unsafe exception: the counting allocator

- **Status:** Accepted (implements the approved plan: inception proposal §6 "Allocation detection", `docs/realtime-rules.md`)
- **Date:** 2026-10-11

## Context
The contract suite and the engine tests must prove that `process`, `emit`, `absorb` and the per-frame executor never allocate. The approved mechanism is a counting `#[global_allocator]`. Implementing `GlobalAlloc` requires `unsafe impl`, and `unsafe_code` is denied workspace-wide (AGENTS.md rule 9).

## Decision
- Exactly one module may use `unsafe`: `crates/signal-contract/src/alloc_counter.rs`, with `#![allow(unsafe_code)]` scoped to that file.
- It wraps `std::alloc::System`, forwarding every call unchanged, and counts allocations and reallocations made by the current thread while a guard is active.
- It is used only in test binaries (`[dev-dependencies]`); no production binary installs it.
- The file carries a safety comment per `unsafe` block, and the contract suite's own tests prove it counts (positive case) and stays at zero (negative case).

## Alternatives considered
- `assert_no_alloc`: works, but has been unmaintained since 2023 and is not in the locked dependency set.
- No allocation checks: rejected. Real-time safety must be enforced by tooling (E3).

## Consequences
Any further `unsafe` still needs its own ADR.
