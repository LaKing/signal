# ADR 0005: Ports, rates, and the conversion model

- **Status:** Accepted
- **Date:** 2026-10-11

## Context
Interconnectivity is Signal's core strength (G13): ports should be typed in the software sense but as interchangeable as possible for users. Example: send blue to fixture X while audio Y is playing bass.

## Decision
- A port is described by **shape** (scalar, N-channel/vector, event stream), **rate** (an integer divisor of the master clock), **kind** (audio, control, gate, trigger, colour, frequency, …), and **unit/range** where applicable.
- Initial implementation: f64 signal and event stream. Vectors and other kinds follow when a test project needs them.
- **Conversion levels:** *implicit* when lossless and unambiguous; *default converter* with overridable parameters when an obvious choice exists; *explicit* when domain meaning is required.
- **Converters are ordinary modules** that declare a conversion from one kind to another. The graph compiler inserts one only when exactly one shortest path exists. Inserted converters are visible in the plan, logs, and GUI. Ambiguity or a missing path is a validation error with suggested converters.
- Connections may configure conversion inline (`via = ...` plus parameters).
- No expression language inside connections for now.

**Note (2026-10-18, operator decision): `generic` matches every kind.** A port of kind `generic` means "any kind": a connection where either end is `generic` is never a kind mismatch, so it gives no warning. A connection between two different kinds neither of which is `generic` is still accepted with a kind-mismatch warning. The core arithmetic, function and compare types (ADR 0027) are `generic` precisely so they can sit between typed ports. Converter insertion is unaffected: an implicit converter is chosen by shape and rate divisor only and carries the source port's kind, so a `generic` end neither triggers nor blocks one (`signal_core::PortKind::matches`, `crates/signal-graph/src/validate.rs`).

## Alternatives considered
- Always explicit converters: simpler, but less convenient.
- One untyped audio-rate signal everywhere: maximally interchangeable, but loses meaning and wastes CPU on low-rate domains.

## Consequences
- Module authors add custom conversions by writing a converter module.
- `signal plan --explain` (M1/M2) must show the inserted converters.
- Risk R16 (automatic conversion becoming magic) is mitigated by the visibility and uniqueness rules.
