# ADR 0016: Execution contract — scheduling, feedback, and same-frame ordering

- **Status:** Proposed. Awaiting PO approval; blocks the M1 module interface. Revision 2: event delivery to state-output primitives, suspension, and exact ramps.
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
- Suspension is explicit (an edit command), logged with its effective frame. Exact behaviour: §1a.
- Instances that reach no sink (a module with an external effect: audio out, network out, file out, meter) get a **validation warning**, not removal.
- **Schedule order:** a topological order of the per-frame dependency graph (§2), with ties broken by lexicographic instance address. The tie-break does not affect output (§4 guarantees that); it only makes logs and debugging reproducible.
- The PO's "pull from outputs" model remains the *mental model* of what a value depends on. It is not used to select what runs.

### 1a. Suspension
A `suspend` edit with effective frame *s* and a `resume` edit with effective frame *r* (> *s*) define the suspended interval [*s*, *r*). During the interval the instance's **time stops**: it is not called (no `process`, `emit`, or `absorb`), its state is frozen, and the frames it has run do not increase.

| Aspect | While suspended |
|---|---|
| **Signal outputs** | Hold the value of frame *s* − 1. Exception: ports of kind `audio` are faded by the executor from the held value to 0 over the fade length *F* (ADR 0006, default 5 ms) and then output exactly 0, which avoids DC. |
| **Event outputs** | Emit nothing. A suspended trigger producer never repeats its last trigger. |
| **Incoming signals** | Ignored. |
| **Incoming parameter events** (`set`, `ramp`) | Applied by the executor to the parameter base, in canonical order (§3), so settings stay editable. A `set` takes effect at once (and cancels any ramp). A `ramp` issued while suspended is held as pending (only the latest; a later `set` clears it). At frame *r* it is applied as if stamped for *r*, ordered before every event actually stamped for *r*, so it starts from the base value at *r*. A ramp already running when the instance was suspended pauses and continues on resume with its remaining frames (§4a counts only frames the instance runs). |
| **Other incoming events** (triggers, messages) | Discarded, not queued: queuing would be unbounded. Each discard increments a per-instance diagnostic counter, which is reported but has no effect on output. |

On resume at frame *r*, the instance computes frame *r* from its frozen state as if no time had passed. Ports of kind `audio` fade in from 0 to their live value over *F* frames. Suspend and resume of a state-output instance follow the same rules: its emit at *r* uses its frozen state.

### 2. Feedback: two kinds of primitives and a fixed frame order
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
**Delivery.** An event stamped for frame *n* is part of the receiver's frame-*n* **inputs**, like a signal sample. Events travel along connections, so the dependency rules of §2 apply. A state-output instance's emitted events belong to its emit phase.
- **Direct receiver:** frame-*n* events are applied (parameter bases updated per §4a, event inputs presented to `process`) before the receiver computes its frame-*n* outputs. They affect frame *n*'s output.
- **State-output receiver:** frame-*n* events of *every* source (external or module-emitted, emitted before or after its emit phase) are consumed in **absorb**. They affect state[n] and therefore outputs from frame *n* + 1 onward. They can never alter the frame-*n* output, which was already fixed from state[n−1] during emit. `emit` receives no inputs and no parameter values, so the contract enforces this.

**Canonical order of events arriving at one input port in frame *n*.** Sort by:
1. **Source class:** external events first (edits, CLI/GUI parameter changes, MIDI/OSC/network input, from the session log), then module-emitted events.
2. **Within external:** log sequence number.
3. **Within module-emitted:** lexicographic order of the source port address (`/drums/kick/env.out` < `/lfo.out`), then the order in which that source emitted them in the frame.

The rule depends only on addresses and logs, never on schedule. Renaming an instance can change same-frame order between sources; this is documented, deterministic, and replay-safe.

**Edits vs events in the same frame.** Graph edits (add, remove, connect, suspend) apply first, then parameter and other events, in the order above.

### 4. Parameters, events, and modulation
A parameter is an input port with a **base value** and optional **modulation** (ADR 0004).
- **Events change only the base**, exactly as defined in §4a.
- **Connected signals are modulation.** Several signals connected to one input are **summed** (fan-in on signal ports = sum; each connection may scale). Modulation never overwrites the base.
- **Effective value** = `combine(base[n], modulation_sum[n])`. The parameter declares `combine`: the default is additive in the parameter's unit, and alternatives are multiplicative or replace. The result is clamped to the declared range. A parameter with no connections uses its base.
- Event fan-in follows §3. Signal fan-in is a commutative sum, but the summation order is fixed to the canonical source order of §3, so floating-point results are bit-stable.

### 4a. Base value per frame and ramps (normative)
For each parameter and each frame *n* the instance runs, the executor computes `base[n]` in this order:
1. **Advance:** if a ramp is running, `base[n]` = the ramp's value at *n* (formula below); otherwise `base[n]` = `base[n−1]`.
2. **Apply events** stamped for *n* in canonical order (§3), each acting on the current value:
   - `set(v)`: base := *v*; any running ramp is cancelled.
   - `ramp(to, N, curve)`, with *N* an integer number of frames ≥ 0:
     - **If *N* = 0:** identical to `set(to)`.
     - **If *N* ≥ 1:** cancel any running ramp. Let *b* = the current base (the result of step 1 and any earlier events in this frame) and *n₀* = *n*. Start a new ramp; base stays *b* for frame *n₀*.
3. The resulting value is `base[n]`.

**Ramp value.** For a ramp (*b*, *to*, *N*, *n₀*, *c*) and a frame *n₀* + *i*, where *i* counts frames the instance has run since *n₀*:
- **0 ≤ *i* < *N*:** value = `b + (to − b) · c(i / N)`, evaluated in f64 from the integers *i* and *N*. It is never accumulated step by step, so there is no drift and the result is independent of *k*.
- ***i* ≥ *N*:** value = `to`, assigned exactly (not computed), and the ramp ends.

**Endpoints (*N* ≥ 1).** The value at the start frame *n₀* is exactly *b*. The value reaches *to* exactly after *N* frames in which the instance runs. Without suspension that is frame *n₀* + *N*; each suspended frame in between postpones completion by one frame (§1a). A ramp with *N* = 1 and no suspension therefore moves from *b* at *n₀* to *to* at *n₀* + 1. For *N* = 0 the value is *to* immediately at *n₀*.

**Curves.** Every curve satisfies `c(0) = 0` and `c(1) = 1` exactly. `linear` is `c(x) = x`. Further curves are defined in `semantics.md` (M1) with the same constraint.

**Units and range.** *N* is in master-clock frames. A parameter port at rate divisor *d* samples `base` at its tick frames. Ramps act on the base; clamping applies only to the effective value (§4), never to the base.

### 5. Consequences for the module interface (M1)
- `describe()` declares the feedthrough class, plus per parameter its range, unit, and `combine`.
- Direct primitives implement `process`; state-output primitives implement `emit` (no inputs, no parameters: output from state only) and `absorb` (inputs, events, and parameters for the frame → new state).
- The executor owns suspension (§1a): holding and fading outputs, base updates, and discard counters. Modules never see suspension.
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
- State-output delivery: an event emitted by a direct module to a `unit_delay`-like instance in frame *n* changes that instance's output first at frame *n* + 1, never at *n*.
- Suspension: a suspended trigger source emits no events; signal outputs hold (audio fades to exactly 0 within *F* frames); `set` while suspended is visible after resume; a running ramp resumes with its remaining frames; discarded triggers are counted.
- Ramps: for *N* ∈ {0, 1, 7, 96000} and random *k*-partitions: for *N* = 0 the value at *n₀* is `to`; for *N* ≥ 1 the value at *n₀* is *b* and the value after *N* running frames is `to` bit-exactly (at *n₀* + *N* without suspension, later by the number of suspended frames with suspension); an interrupting `set`/`ramp` starts from the interrupted ramp's value at that frame.
