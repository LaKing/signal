# ADR 0008: Determinism and reproducibility

- **Status:** Accepted
- **Date:** 2026-10-11

## Context
The PO requires (mostly) bit-exact reproducibility, with all randomness seeded (Q12, Q18).

## Decision
- A run is bit-exact on the same build and hardware class given: project files, Signal revision, toolchain, master seed, sample rate, and the logs of external inputs, events, and edits with effective frames.
- All randomness is seeded. A fresh seed for novelty is still recorded.
- The RNG is vendored: SplitMix64 for derivation, Xoshiro256++ for streams, with golden-vector tests. Each instance's stream is `hash(master seed, instance address)`.
- Every render writes a manifest (revision, toolchain, file hashes, Cargo.lock hash, seed, rate, log hashes, build profile).
- Live inputs are reproducible only through recording them.
- Across toolchain or library upgrades a documented tolerance applies, not bit-exactness.

## Alternatives considered
- The `rand` crate's StdRng/SmallRng: declared non-portable, and minor releases may change value streams.
- Bit-exact across all builds: would force extreme pinning.

## Consequences
Golden renders are tied to the pinned toolchain; updating the toolchain is an explicit, reviewed act.
