# ADR 0007: Numerics: f64 signals, denormals, pure-Rust math

- **Status:** Accepted
- **Date:** 2026-10-11

## Context
Per-frame scalar execution removes f32's SIMD advantage. Measurement work benefits from f64. Determinism requires platform-independent math.

## Decision
- All signal values and module state are **f64**. f32 is used only at device and file boundaries.
- Transcendental functions in primitives use the pure-Rust `libm` crate, not the platform libm.
- Denormals: per-module flushing conventions in `dsp-conventions.md`. Optional flush-to-zero comes only from one audited module and is applied identically offline and live.
- No fast-math. The toolchain is pinned by `rust-toolchain.toml`.

## Alternatives considered
- f32 buffers with f64 state: faster with SIMD, but irrelevant to a per-frame engine.
- Relying on FTZ alone: Rust treats changing FP control flags as UB.

## Consequences
Revisit f32 only if D-EXEC-3 introduces SIMD block execution and benchmarks justify it.
