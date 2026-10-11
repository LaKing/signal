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

## Alternatives considered
- Always explicit converters: simpler, but less convenient.
- One untyped audio-rate signal everywhere: maximally interchangeable, but loses meaning and wastes CPU on low-rate domains.

## Consequences
- Module authors add custom conversions by writing a converter module.
- `signal plan --explain` (M1/M2) must show the inserted converters.
- Risk R16 (automatic conversion becoming magic) is mitigated by the visibility and uniqueness rules.
