# ADR 0016: Execution contract — scheduling, feedback, and same-frame ordering

- **Status:** Proposed. Awaiting PO approval; blocks the M1 module interface.
- **Date:** 2026-10-11
- **Refines:** ADR 0002 (frame semantics, module contract) and ADR 0004 (events, parameters). It does not change their intent; it removes ambiguities found in review.

## Context
A review of M0 found three gaps that would let two valid implementations produce different output:
1. Vision §9 said output reachability decides which instances are computed, while ADR 0002 says every instance advances every frame. Read together, a disconnected oscillator could freeze or run, depending on the implementation.
2. "Every feedback loop contains a one-frame delay" does not say how the executor breaks a cycle within a frame.
3. "Events on the same frame are applied in emission order" does not order events from *different* producers, and it does not say how events combine with connected modulation. Different valid schedules could yield different parameter values.

This ADR makes the output a pure function of the graph, the logs, and the seed, independent of schedule.

## Decision

### 1. Scheduling: every instance runs; reachability is diagnostic only
- The **active set is every non-suspended instance in the plan**, connected or not. Output reachability never removes an instance from execution.
- Suspension is explicit (an edit command), logged with its effective frame. A suspended instance does not advance and holds its outputs at their last value. On resume it continues from its held state; its time does not catch up.
- Instances that reach no sink (a module with an external effect: audio out, network out, file out, meter) get a **validation warning**, not removal.
- **Schedule order:** a topological order of the per-frame dependency graph (§2), with ties broken by lexicographic instance address. The tie-break does not affect output (§4 guarantees that); it only makes logs and debugging reproducible.
- The PO's "pull from outputs" model remains the *mental model* of what a value depends on. It is not used to select what runs.

### 2. Feedback: two kinds of primitives and a three-phase frame
Every primitive declares one of two **feedthrough classes**:

| Class | Meaning | Calls per frame |
|---|---|---|
| **Direct** (default) | Any output may depend on same-frame inputs | `process` |
| **State-output** (delay-like) | No output depends on same-frame inputs; outputs at frame *n* are a function of state[n−1] only. Examples: `unit_delay`, delay lines of *d* ≥ 1 frames, some filters written in this form. | `emit`, then `absorb` |

Mixed primitives (some outputs direct, some state-only) are not supported initially. They are built as composites of the two classes.

**Dependency graph.** Each connection A.out → B.in is a dependency edge "B after A", **except** when A is state-output. Then the edge is dropped, because A's output is known before the frame starts.

**Validation.** The dependency graph must be acyclic. Equivalently, every cycle in the connection graph passes through at least one state-output instance. Otherwise loading fails with an error naming the cycle's instances.

**Each frame *n*, in this order:**
1. **External input:** apply the logged external events, edits, and input samples stamped for frame *n* (§3).
2. **Emit:** every state-output instance writes its outputs for frame *n* from state[n−1]. Order among them is irrelevant, since each reads only its own state.
3. **Process:** direct instances run in schedule order. Each reads inputs that are already final for frame *n*.
4. **Absorb:** every state-output instance reads its now-final inputs for frame *n* and computes state[n]. Order among them is irrelevant.

With *k* > 1 (D-EXEC-3, deferred), a strongly connected component containing state-output instances may run in blocks of at most the minimum loop delay in frames (1 for a unit delay). Parts of the graph outside such components may use any *k*. The per-frame engine is the reference for both.

### 3. Same-frame event ordering
**Delivery.** An event stamped for frame *n* takes effect **before** the receiving instance computes frame *n*. Events travel along connections like signals, so the dependency rules of §2 apply. A state-output instance's emitted events belong to the emit phase.

**Canonical order of events arriving at one input port in frame *n*.** Sort by:
1. **Source class:** external events first (edits, CLI/GUI parameter changes, MIDI/OSC/network input, from the session log), then module-emitted events.
2. **Within external:** log sequence number.
3. **Within module-emitted:** lexicographic order of the source port address (`/drums/kick/env.out` < `/lfo.out`), then the order in which that source emitted them in the frame.

The rule depends only on addresses and logs, never on schedule. Renaming an instance can change same-frame order between sources; this is documented, deterministic, and replay-safe.

**Edits vs events in the same frame.** Graph edits (add, remove, connect, suspend) apply first, then parameter and other events, in the order above.

### 4. Parameters, events, and modulation
A parameter is an input port with a **base value** and optional **modulation** (ADR 0004).
- **Events change only the base.** `set(v)` sets the base immediately. `ramp(to, frames, curve)` moves the base from its current value, starting this frame. A later `set` or `ramp` in canonical order cancels a running ramp. After all of a frame's events are applied, the base for frame *n* is whatever the last event left.
- **Connected signals are modulation.** Several signals connected to one input are **summed** (fan-in on signal ports = sum; each connection may scale). Modulation never overwrites the base.
- **Effective value** = `combine(base[n], modulation_sum[n])`. The parameter declares `combine`: the default is additive in the parameter's unit, and alternatives are multiplicative or replace. The result is clamped to the declared range. A parameter with no connections uses its base.
- Event fan-in follows §3. Signal fan-in is a commutative sum, but the summation order is fixed to the canonical source order of §3, so floating-point results are bit-stable.

### 5. Consequences for the module interface (M1)
- `describe()` declares the feedthrough class, plus per parameter its range, unit, and `combine`.
- Direct primitives implement `process`; state-output primitives implement `emit` and `absorb`.
- The executor, not the module, applies events to parameter bases and computes effective values, so every primitive gets identical semantics.

## Alternatives considered
- **Reachability-based activity (lazy):** rejected. Time would stop for unobserved instances, and output would depend on what is observed.
- **Engine-inserted implicit delay on any cycle:** rejected. It hides latency and makes the cycle's delay position implementation-defined.
- **Mixed-feedthrough primitives with per-port dependency declarations** (Simulink-style): more expressive but more complex for module authors and agents. Can be added later without breaking the two-class model.
- **Event order by schedule position:** rejected. Not stable across equally valid schedules.
- **Event order by project-file connection order:** rejected. Flattening composites and file diffs reorder it.

## Verification (M1 contract and engine tests)
- Schedule-independence: several valid topological orders (random tie-breaks) produce bit-identical output.
- A cycle without a state-output instance is rejected; with one it renders, and the per-frame result matches a hand-computed reference.
- A disconnected oscillator advances: its phase after N frames equals the analytic value whether or not it was connected.
- Event ordering: two sources send conflicting `set` events on the same frame; the result matches the canonical rule, and renaming the sources flips it predictably.
- Base and modulation: a `set` during active modulation changes only the base; the effective value follows the declared `combine`.
