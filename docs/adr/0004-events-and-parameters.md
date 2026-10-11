# ADR 0004: Generic events; parameters are input ports

- **Status:** Accepted. Refined by ADR 0016 (proposed): same-frame ordering, base vs modulation.
- **Date:** 2026-10-11

## Context
MIDI note semantics must not be the core abstraction. Parameter systems and modulation systems are often separate, which hinders interconnectivity.

## Decision
- Events are small generic messages on event ports, e.g. trigger, set, ramp. Events on the same frame are applied in the canonical order of ADR 0016 §3.
- MIDI, OSC, and similar protocols exist only as adapter modules.
- **Parameters are input ports with defaults.** External control sends set/ramp events to the port; modulation connects a signal to it. Panels and knobs are ordinary modules.

## Alternatives considered
- MIDI-like note events in the core.
- A separate parameter/automation system.

## Consequences
- Anything can modulate anything.
- Every parameter is addressable by its hierarchical port address.
