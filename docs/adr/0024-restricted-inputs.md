# ADR 0024: Restricted inputs

- **Status:** Accepted (operator note 002 of M2.8-9, 2026-10-15: "Reject a scale other than 1 and any fan-in on bridge inputs when the plan is built, with an error that names the connection as written"; design by the planner, to be confirmed by the operator at checkpoint). **Narrows ADR 0016 §4** ("fan-in on signal ports = sum; each connection may scale") for the inputs a library restricts; every other input is unchanged.
- **Date:** 2026-10-15

## Context
`std.count` carries events into the visual-rate types as a **reading**: integers that only grow, from which a consumer applies what the same counter counted since its last tick (`docs/module-contract.md`, "Events into divided-rate types"). A reading means something only as it was written. A scaled one, or the sum of several, can still look like a reading: before M2.8-9 a window at scale 2, or connected twice, decoded as a valid one about one time in eight and applied twice the events. Check rows in the reading make one scaled reading, several summed, any two combined and any combined at scales ≥ 0 undecodable, but not every combination (three readings with a negative scale can still forge one). Only the graph sees how an input is connected.

## Decision
1. A library may **restrict an input** of one of its types when it registers it: `Registry::restrict_input(type, input, source type, source output)`. A plan is built only if the input has at most one connection, at scale exactly 1, from that output of an instance of that type.
2. The check is **structural**: the graph compares type and port names and scales, after flattening and before converters are inserted, and knows nothing of what the input means (ADR 0001: no domain concept in the core). It lives in `signal-graph` (`GraphError::RestrictedInput`, with the reason: scaled, fan-in, or another source) and in `signal-core`'s `Registry`; the engine is unchanged.
3. Each offending connection is reported **as written**: its declared endpoints, and in a project its file, line and column.
4. `signal-std` restricts its four bridge inputs (`std.particles.emit` and `impulse`, `std.mesh2d.strike`, `std.body2d.kick`) to `std.count.window`. The check rows stay as a second line for what is not built through a registry that has the rule.

## Consequences
- A misconnected bridge never runs: the project does not load, and a live edit that would misconnect one is refused before anything is sent.
- An input that is not restricted still sums its connections, each scaled (ADR 0016 §4, `semantics.md` §3).
- Tests: `crates/signal-project/tests/errors.rs` (`a_misconnected_bridge_input_is_refused_as_written`: scale 2, fan-in, three counters with one at scale −1, another source, through a composite's input export).
