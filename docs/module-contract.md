# Module contract

**Normative.** This page defines how a primitive is written against `signal-core`. It implements ADR 0002 (frame semantics, *k*-frame contract) and ADR 0016 §2 and §5 (feedthrough classes, module interface). Code that runs on the audio thread also follows `realtime-rules.md`. The types named here are in `crates/signal-core`; their rustdoc carries a minimal, tested example of each class (`DirectPrimitive`, `StateOutputPrimitive`).

## Writing a primitive

1. **Choose the feedthrough class** (next section). It decides which trait you implement.
2. **Write the descriptor** as a `const ModuleDescriptor`:
   - `type_name`: e.g. `std.sine_osc`, unique in the registry.
   - `doc`: one or two sentences for `signal modules`.
   - `ports`: every port that is not a parameter (`PortDescriptor`: name, direction, shape, rate divisor, kind, optional unit and range).
   - `parameters` (`ParameterDescriptor`: name, kind, rate divisor, optional unit, range, default, `combine`).
   - `feedthrough`: must name the class whose trait you implement.
   - `sink`: `Some(SinkKind::Audio)` if the module has an external effect, else `None`.

   Port and parameter names follow `[a-z][a-z0-9_]*` and are unique within the module, because parameters are input ports and share the address form `<instance>.<name>` (ADR 0004). M1 uses rate divisor 1 only.
3. **Implement the trait**: `describe` returns the descriptor; `prepare`, `reset`, and either `process` or `emit` + `absorb`.
4. **Register it** as a `ModuleType { descriptor, create }`, where `create` returns a fresh, unprepared instance boxed as `Primitive::Direct` or `Primitive::StateOutput`. `Registry::register` refuses a type name that is already registered.

The module never sees addresses, other instances, the graph, suspension, or the wall clock. Domain concepts (tempo, musical time, protocols) belong in library modules, never in the core (ADR 0001, 0003).

## The two feedthrough classes

| Class | Trait | Contract | Calls per frame |
|---|---|---|---|
| **Direct** (default) | `DirectPrimitive` | Any output may depend on same-frame inputs | `process` |
| **State-output** | `StateOutputPrimitive` | No output depends on same-frame inputs: outputs at frame *n* are a function of state[n−1] only | `emit`, then `absorb` |

- **`process(inputs, outputs)`** computes `frames()` = *k* ≥ 1 consecutive frames. It reads signal inputs, effective parameter values and input events, writes every output sample, and may push output events. The result must be identical for every partition of the timeline into calls (ADR 0002).
- **`emit(outputs)`** writes the outputs for the frame from state alone. It receives **no inputs and no parameter values** and takes `&self`, so it cannot change state. Events it pushes belong to the emit phase (ADR 0016 §3).
- **`absorb(inputs)`** receives the frame's now-final signal inputs, input events (from every source, ADR 0016 §3) and effective parameter values, and computes state[n]. It has no outputs. An input therefore first affects output at frame *n* + 1.
- The per-frame reference engine calls `emit` and `absorb` with one frame. Block execution of state-output instances (D-EXEC-3) is deferred. When it is decided, this section will state its limits.
- A primitive that needs both kinds of output is built as a composite of the two classes (ADR 0016 §2).

## Lifecycle: what may run where

```
create ─► prepare(ctx) ─► { process | emit, absorb }*  ─► (reset ─► …)*  ─► drop
          control thread    audio thread                  either            control thread
```

| Call | Thread | May allocate | Notes |
|---|---|---|---|
| `describe` | any | no | An associated function: available from the type, without an instance. |
| `create` | control | yes | Returns an unprepared instance. |
| `prepare(ctx)` | control | **yes, only here** | Size buffers for `ctx.max_frames`, keep `ctx.rng`, precompute tables, validate. Leaves the instance in its initial state. |
| `process`, `emit`, `absorb` | audio | **no** | `realtime-rules.md`: no allocation or free, locks, I/O, syscalls, panics (`unwrap`, `expect`, failing indexing), unbounded loops, wall-clock time, or unseeded randomness. Bounded work per frame. Finite outputs. |
| `reset` | either | **no** | Returns to exactly the state `prepare` left, including random streams. |
| drop | control | frees | The engine returns old instances to the control thread for dropping. |

`PrepareContext` gives:
- `sample_rate`: the master rate in frames per second;
- `max_frames`: the largest *k* any later call receives;
- `rng`: the instance's `RngStream`, `derive(master_seed, instance address)` (ADR 0008).

All randomness must come from that stream. Keep a clone of it, so that `reset` can restart it.

## I/O views

The engine owns every buffer. It lends them to the module per call as flat views, so building a view never allocates. Indices count **within each group, in descriptor order**:
- signal inputs: `ports` with direction `In` and shape `Signal`;
- event inputs: `In` + `Events`;
- signal outputs: `Out` + `Signal`;
- event outputs: `Out` + `Events`;
- parameters: the `parameters` list.

| View | Accessor | Carries |
|---|---|---|
| `Inputs` | `frames()` | *k* ≥ 1, the frames in this call |
| | `signal(i)` | *k* samples of signal input *i*. The executor has already summed its connections (fan-in = sum, ADR 0016 §4), so an input with no connection reads 0. |
| | `parameter(i)` | *k* **effective** values of parameter *i*. The executor applies `set`/`ramp` events to the base, adds modulation per `combine`, and clamps to the range (ADR 0016 §4, §4a). Modules never apply parameter events themselves. |
| | `events(i)` | The events on event input *i*, each with its frame `offset` inside the block (0 ≤ offset < *k*). They are sorted by offset; events with the same offset are already in the canonical order of ADR 0016 §3. |
| `Outputs` | `frames()` | *k*, as above |
| | `signal(i)`, `signals()` | *k* samples of signal output *i* (or all outputs at once). The module writes every sample. |
| | `events(i)` | The bounded `EventSink` of event output *i* |

Accessors never panic. An index outside the declared ports yields an empty slice (or `None` for an event sink), so a shape mismatch is a visible defect, not a crash.

**Event output is bounded.** `EventSink::push` keeps the event if there is room. When the sink is full it **drops the event, increments the sink's overflow counter, and returns `false`**, without allocating or panicking. The engine sizes each sink before the real-time calls, collects and clears it after each call, and reports overflows as faults. Events pushed by one module on one port keep their emission order. Their order relative to other sources is set by the receiver's canonical order (ADR 0016 §3).

**Events** (`Event { offset, payload }`) are `Copy`, heap-free, and at most 32 bytes. Payloads are `Trigger`, `Set(v)` and `Ramp { to, frames, curve }`. `Curve::Linear` is `c(x) = x`, and every curve satisfies `c(0) = 0` and `c(1) = 1` exactly (ADR 0016 §4a).

## Checks

The contract suite (M1-2, `signal-contract`) checks every primitive in `crates/` and `lab/`: no allocation in the real-time calls, bit-identical output for random *k*-partitions, and finite output for fuzzed input (`realtime-rules.md`).
