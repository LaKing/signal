# ADR 0003: Time model: the core knows only frames

- **Status:** Accepted
- **Date:** 2026-10-11

## Context
Multiple domains (music, measurement, lighting, visuals) need different notions of time. Long polymeters drift if positions are floats.

## Decision
- Core time is `Frame(u64)` since project start, at the project's master rate (default 96 kHz; rate-agnostic, tested at 44.1, 48, 96, and 192 kHz).
- Musical time (tempo, beats, rational positions, polymeter, microtiming) is produced by **transport/clock primitives** in a library. They use exact rationals and one documented rounding rule.
- The real-time master clock is logical and follows the primary audio device. Secondary devices attach through clock-domain adapters (designed in M6).

## Alternatives considered
- A tempo/beat clock in the core: simpler for music, but leaks a domain into the core.
- f64 beats: drift in long polymeters.

## Consequences
- Projects needing a shared tempo connect a transport module explicitly.
- `docs/semantics.md` (M1) is normative for rounding and ordering.
