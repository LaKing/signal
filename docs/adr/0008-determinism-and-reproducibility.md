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

## Notes
**2026-11-19, the build's identity is embedded** (operator, after an outside review of `4d37bf8`). The manifest's revision, dirty flag, `rustc` version and `Cargo.lock` hash were read when a render ran: `git` in the checkout the binary was built from and the first `rustc` on the PATH, so the same executable reported another compiler under another PATH, and a checkout that moved on named a revision the binary was not built from. `signal-project`'s `build.rs` now reads them when it is built (the compiler cargo builds with, `git --no-optional-locks`, the lock file) and `provenance::build` reports those; the build script runs again when the revision, the index, the sources (`crates/`, `lab/`), the lock file or the toolchain change. `provenance::collect` still reads a given checkout at run time. A replay's manifest now also names the hashes its session's first version brings (includes, samples, interfaces), as a render's does.
