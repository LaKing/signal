# Module contract

**Normative.** This page defines how a primitive is written against `signal-core`. It implements ADR 0002 (frame semantics, *k*-frame contract) and ADR 0016 §2 and §5 (feedthrough classes, module interface). Code that runs on the audio thread also follows `realtime-rules.md`. The types named here are in `crates/signal-core`; their rustdoc carries a minimal, tested example of each class (`DirectPrimitive`, `StateOutputPrimitive`).

## Writing a primitive

1. **Choose the feedthrough class** (next section). It decides which trait you implement.
2. **Write the descriptor** as a `const ModuleDescriptor`:
   - `type_name`: e.g. `std.sine_osc`, unique in the registry.
   - `doc`: one or two sentences for `signal modules`.
   - `ports`: every port that is not a parameter (`PortDescriptor`: name, direction, shape, rate divisor, kind, optional unit and range). Kind `generic` means any kind: a connection with a `generic` end is never a kind mismatch (ADR 0005, note of 2026-10-18).
   - `parameters` (`ParameterDescriptor`: name, kind, rate divisor, optional unit, range, default, `combine`).
   - `feedthrough`: must name the class whose trait you implement.
   - `sink`: `Some(SinkKind::Audio)` if the module has an external effect, else `None`.
   - `category`: the palette group, a free-form tag (`Some("filter")`), or `None`. Its vocabulary belongs to the library that registers the type, never to the core ("The standard library" below).

   Port and parameter names follow `[a-z][a-z0-9_]*` and are unique within the module, because parameters are input ports and share the address form `<instance>.<name>` (ADR 0004). Rates and vector shapes: "Rates and vectors" below.
3. **Implement the trait**: `describe` returns the descriptor; `prepare`, `reset`, and either `process` or `emit` + `absorb`.
4. **Register it** as a `ModuleType { descriptor, create }`, where `create` returns a fresh, unprepared instance boxed as `Primitive::Direct` or `Primitive::StateOutput`. `Registry::register` refuses a type name that is already registered, and names in the `core.` namespace, which is the rate converters' (below).

The module never sees addresses, other instances, the graph, suspension, or the wall clock. Domain concepts (tempo, musical time, protocols) belong in library modules, never in the core (ADR 0001, 0003).

## The two feedthrough classes

| Class | Trait | Contract | Calls per frame |
|---|---|---|---|
| **Direct** (default) | `DirectPrimitive` | Any output may depend on same-frame inputs | `process` |
| **State-output** | `StateOutputPrimitive` | No output depends on same-frame inputs: outputs at frame *n* are a function of state[n−1] only | `emit`, then `absorb` |

- **`process(inputs, outputs)`** computes `frames()` = *k* ≥ 1 consecutive frames. It reads signal inputs, effective parameter values and input events, writes every output sample, and may push output events. The result must be identical for every partition of the timeline into calls (ADR 0002).
- **`emit(outputs)`** writes the outputs for the frame from state alone. It receives **no inputs and no parameter values** and takes `&self`, so it cannot change state. Events it pushes belong to the emit phase (ADR 0016 §3).
- **`absorb(inputs)`** receives the frame's now-final signal inputs, input events (from every source, ADR 0016 §3) and effective parameter values, and computes state[n]. It has no outputs. An input therefore first affects output at frame *n* + 1.
- The engine calls `emit` and `absorb` with **one frame**, also in block execution (ADR 0019 §1, `semantics.md` §13): a state-output instance covers a block's frames one call per frame. `emit` gets no inputs, so it could not write a later frame of the same call whose output depends on an earlier frame's input. `process` gets blocks of up to `max_frames` frames.
- An event output keeps at most 16 events per frame of a call (a frame is the event's `offset`); more in one frame is an `event_overflow` fault at that frame. Push events with offsets inside the call (0 ≤ offset < *k*); an offset past the call counts against its last frame.
- A primitive that needs both kinds of output is built as a composite of the two classes (ADR 0016 §2).

## Declared latency (GPU-backed modules)

ADR 0019 §3 (M2.6-4). A **state-output** type may declare in its descriptor:
- `latency: Some(BlockLatency { latency_blocks, block_frames })`: its output in its block *b* is the result for its block *b* − *L*, *L* = `latency_blocks` ≥ 1. The block is the **module's own**: `block_frames` ≥ 1 consecutive frames, counted from the instance's first frame (after `prepare` or `reset`). It is fixed for the type, so for every run, and it is part of the result; the manifest records it. The engine's block size *k* and the thread count still never change the output: the engine calls the instance one frame at a time, as every state-output instance.
- `reproducibility: Reproducibility::SameMachine`: its output is the same bit for bit on the same machine and build only (GPU arithmetic differs between GPUs and drivers). The default, `BitExact`, is every other type's. A render that uses a `same_machine` instance lists it in `manifest.json` (`semantics.md` §9); renders without one keep their manifests unchanged.

`Registry::register` refuses a latency on a direct type, or with 0 blocks or 0 frames (`RegistryError::InvalidLatency`). The output depends only on earlier blocks' input, so the engine treats the type as the state-output class it is: it may close a cycle.

Two more methods of `StateOutputPrimitive`, with defaults the other types keep:
- **`due(&mut self) -> Due`**, asked by the engine before every `emit` of a latent instance: `Ready` (the next `emit` has its result, or none is due yet), `Pending` (not arrived), or `Lost` (it never will: no device, or the job could not be submitted). It never blocks.
- **`miss(&mut self)`**: the engine gave up on the due result; the module outputs silence for that block and discards the result when it arrives.

**Block-counting invariant** (M2.7-4). A latent module counts its blocks exactly as the engine does: one frame per frame in which the instance runs (`emit` then `absorb`), from `prepare`; the engine never calls `reset` on a running instance. A result is due at the first frame of its block *m* ≥ *L*, and at every other frame `due` answers `Ready` without consuming anything. The engine relies on this to record each due block's outcome and to replay it.

Four more methods (M2.7-4), with defaults a module without a device keeps:
- **`result_words(&self) -> usize`**: the most words its delivered result has, asked once after `prepare`, off the real-time thread. A latent type must return at least 1 (M2.7-8): the engine refuses a latent instance that exposes no result, because its delivered results could not be hashed and an exact replay could never be shown to be one. 0, the default, is for the other types.
- **`delivered_result(&self) -> &[f32]`**: the result shown in the current block. Asked at every delivered due block, to hash it (M2.7-8, always) and, in a session recorded with `--record-boundary-data`, to keep it; in an exact replay also after `inject_result`, when it must be the injected words, so that recorded data is checked against its hash too. Real-time rules apply.
- **`inject_result(&mut self, words) -> bool`**: exact replay with recorded data shows `words` as this block's due result instead of the device's (and calls it instead of `due`); `false` (the default) when the type cannot.
- **`settle(&mut self) -> Due`**: exact replay only, before each `absorb`: `Pending` while the work that `absorb` may submit cannot be accepted yet because work given up on is still in flight (the engine yields and asks again), `Lost` when it never will be. Never blocks. Without it a replay, which runs faster than the live run, could be refused a submission live made.

At a recorded miss an exact replay calls `due` once, without waiting, and then `miss`, whatever `due` answered, as real time does: a module's bookkeeping in `due` (returning stale jobs to its pool) runs as it did live.

What the engine does with them (`semantics.md` §7, §13):
- **Offline** (`EngineConfig::realtime == false`: renders, ideal replays, the bench, the contract suite) it asks again, yielding the thread, until `due` is not `Pending`, so renders are exact and repeatable. A result that is `Lost` (no usable GPU) is still a `late_result` fault, and `signal render` warns about it (stderr, and `late_results` in its JSON).
- **In real time** (`realtime()`, so the null driver, `signal play` and the app) it asks once and never waits. `Pending` or `Lost` is a **`late_result` fault**: `miss`, a `fault` record at the block's first frame, and `Engine::late_results` / `RunnerStatus::late_results` count it. The instance is not muted; the next block asks again.

**The cause of a lost result** (M2.7-5). When `due` answers `Lost` because the work failed (a GPU job ended `Failed` or `DeviceLost`), `lost_cause` says why (`LossCause`: `validation`, `out_of_memory`, `mapping`, `internal`, `uncaptured`, `stopped`, `device_lost`); the engine asks it right after that `due` and before `miss`, so `miss` may forget it, and logs it as the fault's `cause` (`semantics.md` §9). The default, `None`, logs no cause; real-time rules apply.

The GPU work itself runs on its device's `signal-gpu-<id>` thread (`crates/signal-gpu`, `realtime-rules.md` "GPU"); `due`, `miss`, `emit` and `absorb` only move preallocated job buffers through lock-free queues. `lab.gpu_partials` (`lab/src/gpu_partials.rs`) is the example: *L* = 2, blocks of 256 frames, with a CPU reference and a documented tolerance.

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
| `process`, `emit`, `absorb`, `due`, `miss` | audio | **no** | `realtime-rules.md`: no allocation or free, locks, I/O, syscalls, panics (`unwrap`, `expect`, failing indexing), unbounded loops, wall-clock time, or unseeded randomness. Bounded work per frame. Finite outputs. |
| `reset` | either | **no** | Returns to exactly the state `prepare` left, including random streams. The engine never calls `reset` on a running instance. |
| drop | control | frees | The engine returns old instances to the control thread for dropping. |

A call that panics anyway is caught by the engine: the instance is muted from that call's first frame and a `panic` fault is logged (`docs/semantics.md` §14). Nothing it wrote in that call is used.

### Trust boundary

M2.7-9. The synchronous calls, `process`, `emit` and `absorb` (and a latent module's `due` and `miss`), run inside the audio callback, and the callback cannot return until every one of them has. Each **must be bounded in time and never block**: no waiting on I/O, a device, a lock, another thread or a timer, however briefly or rarely. The engine contains a broken call that **panics** (above). It does **not** contain one that **hangs**: the whole output stalls, and no recovery is possible without risking the buffers the call still holds (`realtime-rules.md` "The trust boundary"). The engine only **detects** a hang: the callback is counted and reported with the instance still running (`realtime-rules.md` "Hangs are detected, not contained").

So computation that is **experimental or may block** (a device, a model, anything whose time is not bounded) must run **behind a latent (asynchronous) boundary**: the type declares a `latency` ("Declared latency" above), does the work on its own thread or device, and answers `due` at once, `Pending` or `Lost` when the result is not in; the engine never waits for it in real time. The GPU modules are built this way (ADR 0019 §3). A lab primitive (ADR 0015) meets the same boundary as any other.

`PrepareContext` gives:
- `sample_rate`: the master rate in frames per second, whatever the type's rate divisor *d*: a type that needs its own tick rate divides it by *d*, which it knows from its descriptor;
- `max_frames`: the largest *k* any later call receives (for a divided rate, also a bound on the ticks of a call, which never has more ticks than frames);
- `rng`: the instance's `RngStream`, `derive(master_seed, instance address)` (ADR 0008);
- `lent` (M2.7-2): what the program running the engine lends the instance, or `None`. The registry's `Host` decides it per plan (`lend`, called on the control side for every plan state the engine builds, the real-time mirror's included, with every instance in address order); the engine passes it on and never reads it. It is opaque (`Arc<dyn Any + Send + Sync>`): a module that uses it downcasts it to the type it expects, and **must work without it**. A GPU-backed module is lent its device, a `signal_gpu::Lease` placed by `signal_gpu::GpuHost` (`realtime-rules.md` "GPU"); without one it has no channel and its due results are lost, logged.
- `generation` (M2.8-10, operator decision operator-request:020): the engine's plan version the instance was created for: 0 for the plan the engine was built with, *v* + 1 for an instance a swap prepared at version *v* adds (`semantics.md` §12). A fresh load gives every instance 0, and a replay of a session gives each the value it had live. **It is identity data only** (operator ruling, M2.8-9): a primitive may use it to tell an instance re-created at the same address from one it has seen, and may carry it in a signal as identity (`std.count.window` carries it in rows 4–6, so a tap of the window sees its value), but no consumer's effect may depend on its value, only on whether it matches. `std.count`'s consumers only compare it.

All randomness must come from that stream. Keep a clone of it, so that `reset` can restart it.

A GPU-backed module's channel is a `signal_gpu::PlacedChannel`. When its device is lost and the pool replaces it (M2.7-2), the module takes the new channel with `PlacedChannel::refresh(before)`, which is real-time safe (no allocation, lock or system call) and may be called from `due`: `before` receives the old channel first, and the module gives back every job it holds from it there (a job belongs to its channel); results in flight on the old channel are lost, with the cause `device_lost`.

## I/O views

The engine owns every buffer. It lends them to the module per call as flat views, so building a view never allocates. Indices count **within each group, in descriptor order**:
- signal inputs: `ports` with direction `In` and shape `Signal` or `Vector`, one **row** per signal and *N* rows per vector of *N*;
- event inputs: `In` + `Events`;
- signal outputs: `Out` + `Signal` or `Vector`, in rows likewise;
- event outputs: `Out` + `Events`;
- parameters: the `parameters` list.

| View | Accessor | Carries |
|---|---|---|
| `Inputs` | `frames()` | *k* ≥ 1, the frames in this call |
| | `signal(i)` | *k* samples of signal input row *i*. The executor has already summed its connections (fan-in = sum, ADR 0016 §4), so an input with no connection reads 0. |
| | `rows(first, n)`, `signal_rows()` | Rows `first .. first + n`, row after row (a vector's elements); the number of rows |
| | `parameter(i)` | *k* **effective** values of parameter *i*. The executor applies `set`/`ramp` events to the base, adds modulation per `combine`, and clamps to the range (ADR 0016 §4, §4a). Modules never apply parameter events themselves. |
| | `events(i)` | The events on event input *i*, each with its frame `offset` inside the block (0 ≤ offset < *k*). They are sorted by offset; events with the same offset are already in the canonical order of ADR 0016 §3. |
| | `table()` | The instance's table, if its plan gives it one (below, "Tables"); `None` otherwise |
| | `is_connected(p)` | Whether port *p* of the descriptor's `ports` (by index, inputs and outputs alike) has a connection in the plan (ADR 0033); `false` for an output or an index past the ports. A property of the plan, the same in every call of it, changed only at a swap (below, "Connected inputs") |
| `Outputs` | `frames()` | *k*, as above |
| | `signal(i)`, `signals()`, `rows(first, n)` | *k* samples of signal output row *i* (or all rows at once, or rows `first .. first + n`). The module writes every sample. |
| | `events(i)` | The bounded `EventSink` of event output *i* |

Accessors never panic. An index outside the declared ports yields an empty slice (or `None` for an event sink), so a shape mismatch is a visible defect, not a crash.

**Event output is bounded.** `EventSink::push` keeps the event if there is room. When the sink is full it **drops the event, increments the sink's overflow counter, and returns `false`**, without allocating or panicking. The engine sizes each sink before the real-time calls, collects and clears it after each call, and reports overflows as faults. Events pushed by one module on one port keep their emission order. Their order relative to other sources is set by the receiver's canonical order (ADR 0016 §3).

**Events** (`Event { offset, payload }`) are `Copy`, heap-free, and at most 32 bytes. Payloads are `Trigger`, `Set(v)` and `Ramp { to, frames, curve }`. `Curve::Linear` is `c(x) = x`, and every curve satisfies `c(0) = 0` and `c(1) = 1` exactly (ADR 0016 §4a).

**Tables** (ADR 0028 §4, M2.11-1). A type may take data a project names for an instance, such as a pattern file (`project-format.md` §12): its library registers a `TableRule` (`Registry::set_table_rule`) with the kind of data it takes and a `compile` function, which turns the format's source (a header and rows of numbers) into an immutable `Table` of `f64` rows when the plan is built, off the audio thread. The core and the engine only carry the rows: the plan holds each instance's table, and every `process` and `absorb` call hands it over as `Inputs::table()` (`emit` gets no inputs, so no table). The table changes only at a plan swap, between two calls, when a live edit changes the data (`semantics.md` §12); the instance is kept with its state. So a module reads the table in every call and keeps no index into it across calls (a binary search per use), and a missing table is a valid state (it plays nothing). Reading it allocates nothing.

**Connected inputs** (ADR 0033). An unconnected input reads 0, like one connected to a source that outputs 0; a type whose behaviour depends on whether an input is wired at all asks `Inputs::is_connected(p)`. The engine builds the flags with each plan's slots, so a swap that connects or disconnects an input reaches the kept instance at the swap frame, its state kept, allocating nothing (`crates/signal-engine/tests/connected.rs`); the oracle hands them over the same way. A library can make two inputs of a type **exclusive** (`Registry::exclusive_inputs(type, a, b)`, a structural rule as ADR 0024's): a plan that connects both on one instance is refused with `GraphError::ExclusiveInputs` at a connection into the second. `std.note_seq` and `std.step_seq8` use both: `follow` connected, they follow a position; `tick` and `follow` together are refused.

## Rates and vectors

M2.8-6 (ADR 0005, `semantics.md` §16).

- **One rate per type.** Every port and parameter declares a rate divisor *d*. A type's **rate** is the divisor its outputs, event ports and parameters share (1 if it has none of them); each signal or vector input may declare another, which divides it or is a multiple of it (`ModuleDescriptor::rate`). The engine refuses a type without one rate (`EngineError::MixedRates`), and a type at a rate above 1 with event ports, a declared latency or a sink (`EngineError::UnsupportedRate`); those run at divisor 1 only.
- **Per-tick calls.** A type at rate *d* is called on its **ticks** only, the frames that are multiples of *d*. In every call `frames()` is the number of ticks the call covers, and every slice holds one value per tick: its inputs and effective parameter values at those ticks, and its outputs for them. The type never sees the frames between ticks; its outputs hold their last tick's values there. So the module contract is the same as at divisor 1, counted in ticks: the result must not depend on how ticks are split into calls, and the contract suite checks a divided-rate type over ticks.
- **Vectors.** A port of shape `Vector(N)`, 1 ≤ *N* ≤ 4096 (`MAX_VECTOR_WIDTH`; a wider one is refused when the plan is built, `semantics.md` §16 "Limits"), carries *N* `f64` per tick as *N* consecutive rows of the views, its elements in order (element *e* is row `first + e`). A vector connects only to a vector of the same *N*, element by element; never to a signal or a parameter. Parameters are scalar.
- **Events into divided-rate types** (M2.8-7, M2.8-10; operator decisions operator-request:007, :017, :014 and :020, 2026-10-15). A type above divisor 1 has no event ports, and that rule stands. Events reach one through `std.count`, the bridge (`crates/signal-std/src/bridge.rs`, `count.rs`): it runs at divisor 1, takes the events on `in`, and outputs `window`, a **reading** of 24 elements, each an integer below 2¹⁹ (inside the fault guard whatever the values): a stamp drawn from its random stream, its creation `generation` (`PrepareContext`), the frames it has run, the number and summed value of its events since it was created (a trigger is worth 1, `set` and `ramp` their value), and two exact checks of those (Σ rows + 1 and Σ rows², M2.8-9). All of them only grow, and a suspended counter holds its reading (`semantics.md` §6), which adds nothing. A type at divisor 800 (`VISUAL_DIVISOR`) reads it on a divisor-1 vector input at its ticks and applies what the **same counter** counted since the reading it applied last, so it never applies an event twice, however long the counter or the consumer is suspended: **an event at frame *f* acts at the first tick at or after *f***, ⌈*f*/800⌉·800, at most one tick late, once, in any partition and on any number of threads; for a consumer suspended across ticks, at its first tick after the resume. **Values** are rounded to a multiple of 2⁻³² (ties to even) and limited to ±2⁶⁴, and summed exactly as integers modulo 2¹¹⁴: the sum a consumer applies is **deterministic**: exact as an integer (in units of 2⁻³²), then converted to binary64 once, which is exact up to 2⁵³ units (about 2²¹ in value) and rounds to nearest beyond; a value of at most 2⁻³³ in magnitude adds 0 (exactly 2⁻³³ is a tie, rounded to even; it still counts). (M2.8-7 kept any finite value; M2.8-10 gives the sum a fixed resolution so that differences are exact.) Consumers form their effect from a mantissa and an exponent, so an effect that binary64 holds is never lost to an intermediate overflow. **The losses, side by side:**
  - *Counter replacement*, the accepted loss (`semantics.md` §12): a live edit that replaces or removes a `std.count` instance, including removing it and adding it again at the same address, loses, for each consumer that runs across the edit, at most one visual tick of events: those the old counter counted after the consumer's last tick of it, and those the new counter counted before the consumer's first tick of it. Nothing is applied twice. A counter added again at the same address is a new counter: its creation generation differs.
  - *A new or replaced consumer* applies nothing a counter counted before the consumer's first tick of it, so at most the events since the previous tick are not applied, with one exception (E2).
  - *E2* (operator-request:014): if a consumer's first reading of a counter is that counter's very first frame (age 1), it applies that frame's events. A counter suspended at its second frame keeps showing its first frame, so a consumer added later applies that one frame's events once.
  - *E1* (operator-request:014): a consumer that is suspended while its counter is replaced or removed loses what the old counter counted after the consumer's last tick and what the new counter counted before the consumer's first tick after resuming, however long the suspension. Nothing is applied twice.
  - *Disconnection*: a consumer that reads its bridge input disconnected (at a tick) applies nothing of what the counter counted until it reads it connected again. A disconnection and a reconnection that both fall between two of its ticks, or while it is suspended, are not seen: the events counted meanwhile are applied once, at its next tick.
  - *Wrap* (M2.8-9): the difference of two readings' sums is exact modulo 2¹¹⁴ only below 2¹⁷ events (each value at most 2⁶⁴). A window of 2¹⁷ events or more since the reading a consumer applied last (about 2.7 s with an event on every frame while the consumer is suspended), or of a saturated count, applies nothing, never a value that may have wrapped, and is **counted**: each consumer's `lost` output (a signal at divisor 800, at most 2¹⁹ − 1, reset with the instance) is the number of such readings, for a scope, a tap or a panel to show.
  - *Identity*: counters are told apart by (stamp, generation). The generation is exact for a counter re-created at one address (it saturates at 2⁵⁷ − 1 plan versions). Counters at different addresses differ by a 64-bit draw from their random streams, which two addresses could in principle share (about 2⁻⁶⁴ per pair).

  The visual types `std.particles` (`emit`, `impulse`), `std.mesh2d` (`strike`) and `std.body2d` (`kick`) take events this way. **This is a deviation from M2.8-7's wording** ("emit and impulse events"): their inputs are `std.count` readings, not event ports. A bridge input takes one connection at scale 1 from a `std.count`'s `window`: **building the plan refuses** a scale other than 1, more than one connection, and any other source there, each at the connection as written (M2.8-9, ADR 0024: `std` registers the rule with `Registry::restrict_input`, the graph checks it, `GraphError::RestrictedInput`). What still reaches an input decodes only as a whole reading (the check rows): one reading at a scale other than 1, several at scale 1, any two at any scales and any at scales ≥ 0 never decode except as a genuine one; rows that are no reading apply nothing. The bridge is for types at divisor 800 only; a divisor-1 type takes events directly (`std.mesh2d_audio`'s `strike`).
  - Tests: `crates/signal-std/src/count.rs` and `bridge.rs` (the reading against a brute force, every element inside the guard, the quantization, every consumer rule), `crates/signal-cli/tests/blocks.rs` (`events_act_at_the_first_tick_at_or_after_their_frame`) and `crates/signal-cli/tests/physical_av.rs` (suspensions, the removal protocol at every phase, re-adds, reconnection, E1, E2, the value limit, replay of creation generations).
- **Converters.** Where a connection joins two rates one of which divides the other, the graph compiler inserts a converter instance, an ordinary direct primitive of type `core.<hold|decimate>.d<from>.d<to>.<signal|vectorN>.<kind>` that copies each input row to its output row at its own rate (`signal_core::converter`). A project may also name one explicitly by that type name; every registry knows them (`Registry::get`) without listing them. A `core.` name that is not exactly such a name is an unknown module.

## The standard library

The `std.*` types are listed with their class and category in the rustdoc of `crates/signal-std/src/lib.rs`, and `signal modules` prints them (`--category <name>` for one group, `--json` for the descriptors). Each type's rustdoc states its ports, parameters and formula.

**Categories** (ADR 0027 §1). Every `std.*` type has one, from the vocabulary in `signal_std::category`; the core crates see only a free-form tag. The `core` category is a palette group, not ADR 0027's core set: the core-set members that existed before M2.10-2 and hide a loop or carry events kept their own groups (`std.delay` in `delay`, `std.lag` and `std.onepole` in `filter`, `std.noise` in `generator`, `std.sample_hold`, `std.edge` and `std.clock_div` in `event`); every M2.10-2 type is in `core`.

| Category | Types |
|---|---|
| `core` | `add`, `sub`, `mul`, `div`, `neg`, `abs`, `min`, `max`, `clamp`, `scale`, `mod`, `fract`, `floor`, `round`, `sign`; `sqrt`, `exp`, `log`, `pow`, `sin`, `cos`, `tan`, `tanh`, `atan2`, `sin_cycles`; `const`, `sample_rate`, `frames`; `compare`, `select`, `and`, `or`, `not`; `gain`, `mix`, `pan`, `unit_delay`; `phasor`, `counter`, `integrator`; `buffer1k`, `buffer64k`, `buffer1m`, `table<N>`; `latch`, `to_events`, `route`; `random`; `pack<N>`, `unpack<N>`, `index<N>`, `reduce<N>`, `vadd<N>`, `vmul<N>` |
| `generator` | `sine_osc`, `osc`, `fm_op`, `wavetable`, `lfo`, `noise` |
| `filter` | `svf`, `onepole`, `ladder`, `biquad`, `dc_block`, `lag`, `tanh_shaper` |
| `envelope` | `exp_env`, `adsr` |
| `delay` | `delay`, `comb`, `allpass_delay`, `fdn_reverb` |
| `physical` | `string`, `modal`, `mass_spring`, `exciter`, `mesh2d_audio` |
| `event` | `sample_hold`, `edge`, `clock_div`, `step_seq`, `count`, `transport`, `note_seq`, `step_seq8` |
| `visual` | `particles`, `mesh2d`, `body2d` |
| `io` | `audio_out` |

**The native DSP primitives** (M2.10, the owner's list; ADR 0027 §3: algorithms whose per-sample feedback crosses many terms stay native) are direct, at divisor 1, and bounded at every parameter combination: each rustdoc states its formula, its source, and its bound argument, and each output is clamped to ±10⁶ so it never faults on its own (`crates/signal-cli/tests/dsp_bound.rs` drives every one at its extremes through the engine).
- `biquad`: the eight RBJ cookbook responses by `mode` (0 low-pass, 1 high-pass, 2 band-pass at 0 dB peak, 3 notch, 4 peak, 5 low shelf, 6 high shelf, 7 all-pass), with `cutoff_hz`, `q` and `gain_db`. It runs the cookbook's transfer functions as a TPT state-variable filter with output mixing (Simper), not as the cookbook's TDF-II: the two agree for fixed coefficients (a unit test runs both), and only the SVF is stable under audio-rate modulation.
- `comb` (feedback comb with a one-pole damping low-pass in the loop) and `allpass_delay` (Schroeder all-pass): `time_ms` up to 1 s, read with `std.delay`'s Hermite interpolation clamped between the two samples it lies between; `feedback` and `gain` range over ±0.999. Both loops are contractions whatever the modulation: the clamped read and the convex one-pole (`lp = s + a·(r − s)`, `0 < a < 1`) never exceed the largest earlier value, so `|comb out| ≤ max|in|/(1 − |feedback|)` and the all-pass's state `≤ max|in|/(1 − |g|)` (its output `≤ (1 + |g|)·max|in|/(1 − |g|)`).
- `fdn_reverb`: 8 lines, Householder matrix, distinct prime lengths scaled by `size`, per-line gains for the T60 `decay_s` and damping one-poles, `predelay_ms`, `mix`; stereo in and out (mono: `in_l` only). Loop gain ≤ max line gain < 1 (at most 0.9982 from 44.1 kHz up).
- `fm_op`: a phase-modulation operator (ratio or fixed frequency, `pm` in cycles, self-feedback through the mean of its last two outputs, `level`). Not band-limited: sidebands beyond sr/2 alias.
- `wavetable`: eight built-in tables (sine, saw, square, triangle, vowels a, e, i, o) morphed by `position`, one mip level per octave so that, at a constant pitch and without `pm`, no harmonic exceeds sr/2, crossfaded between levels.

**The core arithmetic, functions, constants and comparisons** (M2.10-1) are direct, at divisor 1, and **total**: finite inputs give finite outputs.
- **Results are clamped to ±10⁶, the engine's magnitude bound (`semantics.md` §7), so a core primitive never faults on its own.** That bounds `div`, `exp`, `pow` and `tan`, which grow beyond it (e.g. `div(1, 1e−7)`, `exp(14)`, `pow(10, 7)`, `tan` near π/2 all give ±10⁶), and every other M2.10-1 type's output on an input that a fan-in sum or a connection scale took beyond the bound. One constant, `SIGNAL_BOUND` in `signal-std`, holds it. The types that predate M2.10-1 (`add`, `sub`, `mul`, `abs`, `min`, `max`, `scale`, `clamp`, `gain`, `mix`, `pan`, `unit_delay`) are unchanged: `mul`, `add`, `sub`, `scale` and `gain` can still exceed the bound, and the fault guard then mutes them as before.
- Division by ±0 gives 0 (`div`, `mod`, and `pow` of 0 to a negative power).
- `sqrt` of x ≤ 0 is 0, and so is `pow` of a negative base to a non-integer power.
- `log` of x ≤ 0 is the log of the smallest positive normal, about −708.40.
- No stateless type flushes a subnormal result: below the normal range `exp` and `pow` give `libm`'s value, a subnormal or 0. They hold no state, so nothing decays into the subnormal range across frames.
- `mod` is floored, with the sign of `b`; `fract` is in [0, 1); `round` rounds half to even; `sign` gives −1, 0 or 1.
- `sin_cycles` takes cycles and is exact at whole, half and quarter cycles.
- `compare` outputs a gate, 0 or 1, by `mode` (0 `lt`, 1 `le`, 2 `eq`, 3 `ge`, 4 `gt`, 5 `ne`); a ramped or modulated mode passes through every mode between its ends. `select`, `and`, `or` and `not` take gates, where > 0 is true.
- `const`'s `value` ranges over ±10⁶, the magnitude bound.
- `sample_rate` outputs the master rate and its period, from `prepare`. `frames` counts the frames its instance has run on two outputs, `lo` = count mod 10⁶ and `hi` = ⌊count / 10⁶⌋, so count = `hi`·10⁶ + `lo`: both exact integers within the bound, exact up to 10¹² frames, where the count stops (`lo` = 0, `hi` = 10⁶). A single output would fault after 10⁶ frames, about 21 s at 48 kHz. A plan swap that keeps the instance keeps the count (ADR 0006).

Transcendental functions are `libm`'s (ADR 0007). These types are not rate-generic: see the dated note in ADR 0027 §5.

**Core set II** (M2.10-2: state, memory, value events, randomness, vectors) is direct, at divisor 1, and every output and every emitted `set` or `ramp` value stays within ±10⁶, so these types never fault on their own either. Each type's rustdoc has its formula.
- **State.** `phasor` accumulates `freq/sr` in f64 into a phase in [0, 1) (a `reset` trigger sets `reset_phase`; a `wrap` trigger on the frame a wrapped phase shows). `counter` adds `step` on each trigger frame modulo `modulo` (≤ 10⁶), so the count is an integer in [0, `modulo`); a `reset` wins over a same-frame trigger; a larger count is two counters, the second counting the first's `wrap`. `integrator` is one TPT step `y = g·x + s`, `s = y + g·x`, with `g = tan(π·cutoff/sr)` or `gain/(2·sr)`; `y` and `s` are clamped to ±10⁶. A swap that keeps the instance keeps its state (ADR 0006).
- **`buffer1k`, `buffer64k`, `buffer1m`.** A circular memory whose **capacity** C is the type's, 2¹⁰, 2¹⁶ or 2²⁰ frames (8 MiB): `prepare` cannot see parameter values, so the memory it allocates is fixed by the type name, as a vector's width is. `prepare` allocates C frames **and writes every one** (prefault: the pages are touched on the control thread, so `process` takes no first-touch page fault; nothing locks them against paging under memory pressure). N, the frames in use, is the parameter `size` (range [1, C], default C, `round_half_up`, clamped), **read every frame** like any parameter: a `set`, ramp, modulation or live edit takes effect on its frame, so the running instance uses the N its file says (AGENTS.md rule 4). The buffer holds no position of its own (positions are inputs), so when N changes every position is taken modulo the new N; the contents are kept, so elements at N and above are hidden, not cleared, and show again when N grows. One write (`index`, `value`, `gate` > 0) and four read taps `read0..3` → `out0..3`, as signal outputs, so they connect without unpacking. Within a frame **the write comes first**: a tap reading the written element sees the new value, so a loop through a buffer needs a `std.unit_delay`. Positions and interpolation neighbours wrap modulo N. Values are clamped on write and on read. A swap that keeps the instance keeps its contents (ADR 0006). `reset` zeroes every element written since `prepare` (up to the highest written index). The capacities are `signal_std::BUFFER_CAPACITIES`; a capacity not in it has no descriptor and does not compile.
- **`table<N>`.** Read-only: values `v0..v{N−1}` are the instance's parameters, validated against their range (±10⁶) when the project loads; a read at a fractional `pos`, clamped to the ends or (with `wrap`) wrapped modulo N. The values cannot come from a project asset file: the loader reads no other file, and an archive keeps none (`project-format.md` §1, §10; ADR 0027, dated note).
- **Interpolation** (`buffer`, `table`): `interp` 0 none (the element at ⌊p⌋), 1 linear, 2 Hermite (`std.delay`'s 4-point Catmull-Rom). Each is exact at an integer position; a Hermite overshoot is clamped.
- **Value events.** `latch` holds the last `set` value (the last in canonical order on a frame). `to_events` sends `set(in)` on a trigger, or when `in` moved more than `threshold` from the value it sent last, at most once per frame. `route` sends each event to `a` if `gate` > 0 at its frame, else to `b`, in order. Merging is event fan-in (ADR 0016 §3): there is no merge type.
- **`random`.** On a trigger frame, one draw `u` of the instance's stream (`RngStream::derive(seed, address)`, ADR 0008) gives `lo + (hi − lo)·u`, or the largest double below `hi` where that rounds up to `hi`, so the value is in [`lo`, `hi`) of that frame (reversed bounds give [`hi`, `lo`); equal ones give `lo`). The value is **held**: a later change of `lo` or `hi` does not move it, so a held value can lie outside bounds changed after its trigger, until the next trigger. Before the first trigger it is the lower bound.
- **Vectors.** `pack`, `unpack`, `index` (a fractional element read, linear), `reduce` (`sum`, `min`, `max`, `mean`, folded in element order; the sum is clamped), `vadd` and `vmul` (elementwise), each a type per width. They compute from the elements as received (a fan-in sum may exceed ±10⁶) and clamp only their results.
- **Width registration.** A width is part of a `'static` port descriptor, so the vector types and tables are a type per width: `std.pack4`, `std.reduce1024`, `std.table16`, with the width as a suffix (as in the converters' `vector4`). The widths are `signal_std::VECTOR_WIDTHS`, 2, 4, 8, 16, 64, 256 and 1024: the one list in `crates/signal-std/src/vector.rs`. Their descriptors are made once per process, when the registry is first filled, and kept, as the converters' are. **To add a width**, add it to that list (at most `MAX_VECTOR_WIDTH`); every operation and the table then exist at it, and `signal modules` lists them. Another width is an unknown module in a project, and a compile error in Rust (`Pack::<3>`: a `const` assertion in `signal_std`'s descriptor lookup), so no type can silently take another width's descriptor.

**The tick clock and the sequencers** (M2.11-1, ADR 0028 §5–6).
- **`transport.tick`** gives `ppq` ticks per beat (a parameter, integer, [1, 960], default 96), tick *j* at `a + round_half_up((j − ppq·b)·60000·sr / (bpm_milli·ppq))` on the beats' anchor (*a*, *b*), so tick `ppq·k` is beat *k* and the ticks re-anchor with the tempo. **Its contract:** a frame holding *n* ≥ 1 ticks carries a `Trigger`, then, if *n* > 1, a `Set(n − 1)`; count 1 for a `Trigger` and *v* for a `Set(v)` (the sequencers count at most 6400 for one `Set`, the transport's largest, so their work per call stays bounded whatever sends them `Set`s). No tick is dropped or moved, and the port never holds more than 2 events in a frame, so it never overflows at any rate; `bpm·ppq ≤ 60·sr` (every audio rate) is sufficient for one plain `Trigger` per tick. Unconnected, it changes nothing.
- **`note_seq`** (a `notes` pattern) and **`step_seq8`** (a `steps` pattern of up to 8 lanes) count the ticks by that contract, so their positions are exact at any rate; their `ppq` parameter must equal the transport's. A note at `start` beats fires on tick `round_half_up(start·ppq)`. `note_seq` is monophonic, last-note priority, with `gate`, `key`, `hz`, `velocity`, `on`, `off` and a `position` playhead; `step_seq8` has `t1`…`t8`, `accent`, a `step` playhead, `swing` (0–75 %, whole ticks) and `accent_level` (ADR 0028 §6's accent factor: a type's ports and parameters share one namespace, and the `accent` output has the name). Their outputs are per frame: at most one `on` and one `off`, one hit per lane, one `accent`.
- **A `step_seq8` hit** on `tN` is a `Set(level)` (× `accent_level` when accented), then a `Trigger`: envelopes and exciters fire on the `Trigger` and skip the `Set`; `std.count` values both (its value sum is level + 1); into a parameter the `Set` sets its base and the `Trigger` is a logged discard, one per hit, counted, never a fault.
- **Cost: ticks are processed one by one.** A sequencer does a tick's work (a binary search, its notes or steps) for every tick it counts, so a frame costs in proportion to the ticks it holds: the transport puts ⌊bpm·ppq / (60·sr)⌋ + 1 at most on a frame (1 at 48 kHz up to 400 bpm and 960 ppq; 6400 only at the lowest rate, 1 Hz). The 6400 cap per `Set` keeps a call bounded in time whatever arrives, but `tick` is meant for `transport.tick`: driving it from another stream of high-count `Set`s (a `std.count` value, a modulator) is a misuse, which costs up to 6400 ticks' work per event and plays no meaningful position.
- **Wrapping.** A position counts `T = max(1, round_half_up(L·ppq))` ticks per loop. A step whose (swung) tick is at or past *T*, or a note whose start rounds to *T* (`start` just below `length`), fires on the next loop's tick where it falls in time, from the second loop on, so nothing that starts within the loop is lost. `note_seq`'s `position` is `min(p, T − 1)/ppq` with the frame's `ppq`, within ±1e6.

**The position and following it** (ADR 0033).
- **`std.playhead`** outputs `position` (beats), `running` (0/1), `tempo` (bpm·rate while running) and `jump` (a `Trigger` on every frame a seek lands on); `bpm` ([20, 400], default 120; non-finite reads 120), `run` (default 1, combine `replace`: a connected gate starts and stops it) and `rate` (a factor of the tempo, [−16, 16], default 1, combine `replace`: a connected signal replaces the base) set its speed `v = run·bpm·rate/(60·sr)` beats per frame, and `seek` (`Set(b)`, or `Trigger` for 0) jumps it on its frame. Within a segment of constant bpm, rate and run the position is `p₀ + ((n − f₀)·bpm·rate·run)/(60·sr)` from the segment's start, never a per-frame sum, so a steady tempo does not drift (the exact position correctly rounded at an integer bpm·rate); each change rebases it with Neumaier's compensated summation, so a rate changing on every frame stays within a few units in the last place. Clamped to ±10⁶ beats.
- **Following a position.** With `follow` connected, `std.note_seq` and `std.step_seq8` compare each frame's position with the frame before. Forward by at most one beat, the beats `[previous, current)` are crossed, looping over the pattern's length: a note or step fires on the first frame whose position is past its beat, exact in beats (no ticks; `ppq` is not read; swing is `swing/100·step` beats), a note ends on the first frame past its end, and consecutive frames share their bound, so every beat fires once. On a grid of whole frames this is the tick clock's playing one frame later. Not moving fires nothing and ends the sounding note. A `Trigger` on `reset` is a **seek** — wire `std.playhead.jump` there, so that every seek is one, a forward seek of less than a beat included; backwards, more than one beat forward, more than 64 loops in one frame, the first frame or a swap that connected `follow` is one too (inferred, a fallback): nothing fires, the sounding note ends on that frame, and the next frame resumes from the new place: a note starting exactly there fires, one spanning it is not restarted. A frame's work is a binary search per loop the motion touches (at most 67).
- **`lib.transport`** is `std.playhead` with its parameters and `seek` exported and `jump` out (below); it has no `tick` (a count cannot follow a seek or a reverse), so a tick-counting sequencer keeps `std.transport`.

**Ordered operands.** A type whose result depends on the order of its operands takes them on separate, named ports (`a`, `b`, or `in` and named parameters), so the result never depends on the address order of what is connected. Several sources into one port are summed first, in canonical source order (`semantics.md` §3).
- `sub`, `div`, `mod`, `pow`, `atan2` (`a` is y, `b` is x), `compare` and `select` are order-dependent, and their ports say which operand is which.
- `add` and `mul` are commutative.
- `min` and `max` are commutative: on aarch64 `f64::min` and `f64::max` compile to `FMINNM` and `FMAXNM`, which order −0 below +0, so `min(±0, ∓0)` = −0 and `max(±0, ∓0)` = +0 in either order (a unit test pins it; Rust leaves this unspecified, so another target may differ).
- `abs`, `clamp` and `scale` have one signal operand.

## The library

The **built-in library** (ADR 0027 §4, M2.10-3) is composites shipped with Signal, referenced as `lib.<name>` (`project-format.md` §4). They live in the crate `signal-lib` as `crates/signal-lib/lib/<name>.toml`, embedded with `include_str!`; `signal-project` resolves `lib.` references and reads them by the rules of any composite file. They are ordinary composites (ADR 0010: no logic), so a library composite costs what its primitives cost (ADR 0027 §2) and runs through the same engine, determinism and fault guards. `signal modules` lists them after the primitive types (`--category` included), each with its category, description, content hash, ports and parameters (the composite's own defaults); `signal schema` accepts their references. A **validation reference** (M2.10-4, ADR 0027 §4 "Validation") is a library composite that rebuilds a native std type from core primitives to be compared with it (`crates/signal-lib/tests/validation.rs`), not a building block: `signal_lib::VALIDATION` pairs it with its native type, and its `signal modules` entry carries `"reference_for": "std.<type>"` (text: a `reference for:` line), which no other entry has; the schematic palette lists these in their own group `reference`, last and closed (`ui-controls.md`). Their range differences from the natives (the ±1e6 bound on every intermediate) are in each one's description and in `docs/benchmarks.md` ("Validation pairs").

**What a library composite must be.**
- It uses `std.*` and other `lib.*` modules only, never a project composite; no `gpu` pin, no `pattern`. A cycle among library composites is refused when a plan is built.
- It declares its `category` (the ADR 0027 vocabulary, `signal_std::category`) and a `description` that names every export with its unit or range and default (`crates/signal-lib/tests/library.rs` checks both and that each plans on its own).
- **Safe levels.** At its defaults, on its stimulus, its output stays within ±1 (`tests/library.rs`); any exported value that scales a signal is clamped inside it (a drum's `velocity` to [0, 2], the tape echo's `feedback` to [0, 0.75]), so no setting makes it fault: `tests/bounds.rs` renders each one with every parameter at its range's minimum and maximum, all together and one at a time, and finds no fault and no output beyond ±10⁶. Whether a level is pleasant is a listener's judgement.
- **Ordered operands** where association matters (`std.sub`, `std.add`, `std.mul` rather than a fan-in sum of two exported inputs), so the output never depends on the source addresses outside the composite (ADR 0027, Context).
- **Headroom.** Arithmetic is arranged so that no intermediate exceeds the inputs or the result: weights before sums, halves before differences (`lib.crossfade`, `lib.stereo_width`; `tests/library.rs` renders both at ±10⁶). A result within the engine's bound never faults on the way.
- **Pinned output.** `tests/pinned.rs` renders each on a fixed stimulus at 48 kHz, long enough for its whole signal path (`tests/common/mod.rs`: `frames` and `cases`; a composite whose outputs or settings a single render does not reach, as `lfo_shapes`' shapes, is pinned by several renders), and compares the audio hash with `tests/pinned.toml`. A new library composite needs a stimulus that reaches every export. **Versioning** (`crates/signal-lib/CHANGELOG.md`): the library is the app's MAJOR.MINOR; an output change is a new name, or a MINOR change with a changelog entry under the current version naming the composite and its new hash (the test fails without it); earlier versions' sections are history, frozen by their hashes (`tests/released.toml`); names are never reused; a removal warns for one MINOR first.

**Provenance.** A render manifest lists every library composite the plan instantiates with its hash (`library_composite_hashes`, `project-format.md` §7); a session records the library composites of every plan version and the library version (`session-format.md` §2), and `signal replay` warns when this build's differ; a `.signal` archive records the library version in its comment (`project-format.md` §10).

**Vendoring.** `signal vendor <project> lib.<name> [--as <copy>]` copies a library composite and those it uses into the project's `composites/`, where they are the project's own, editable text, and rewrites the project's references to them; it never overwrites a file: a dependency the project already has with exactly the copy's text (vendored before) is kept, and any other file of a copy's name refuses the whole operation. It writes all or nothing as far as the file system allows (temporary files, linked or renamed into place, the project's files last, the project loaded again, and everything taken back on a failure) (`project-format.md` §4).

**The first composites** (M2.10-3; `signal modules --category <group>` gives each one's exports):

| Composite | Category | What |
|---|---|---|
| `lib.ladder_voice` | `generator` | A monophonic three-oscillator voice with a ladder low-pass, two contours and glide (from the M2.11 studio's voice) |
| `lib.tape_echo` | `delay` | A delay with wow, flutter and a darkened, saturated feedback whose loop gain stays below 1 |
| `lib.analog_kick`, `lib.analog_snare`, `lib.analog_clap`, `lib.analog_hat`, `lib.analog_tom`, `lib.analog_rim`, `lib.analog_cymbal` | `generator` | Synthesised drum voices (from the M2.11 studio's drum machine); the hat has `closed` and `open` inputs, the closed choking the open |
| `lib.lfo_shapes` | `generator` | Sine, triangle, saw, square and sample-and-hold random from one `std.phasor`, with `rate` and `shape` |
| `lib.ar_env` | `envelope` | Attack-release from a gate through `std.lag` |
| `lib.crossfade` | `core` | `a·(1 − mix) + b·mix`, the mix clamped to [0, 1] |
| `lib.stereo_width` | `core` | Mid/side width, `width` clamped to [0, 2] |
| `lib.transport` | `event` | ADR 0033: the transport as a signal, `std.playhead` with `bpm`, `run`, `rate` and the `seek` input exported, `position`, `running`, `tempo` and `jump` out, for `std.note_seq.follow` and `std.step_seq8.follow`. Not in `projects/library-demo` (`signal_lib::NOT_IN_DEMO`) |
| `lib.modal_voice` | `generator` | M2.10-8: `std.exciter` into `std.modal` once per rising gate, a loudness contour, velocity to level and brightness; every exciter and bank control exported, and the bank's `mode_freq`, `mode_gain`, `mode_decay` vector inputs (each element `v` a factor `1 + v`, unconnected 1). Not in `projects/library-demo` (`signal_lib::NOT_IN_DEMO`: the demo's golden is never regenerated) |

`projects/library-demo` uses every one but the validation references and the composites added after it (`signal_lib::NOT_IN_DEMO`: `lib.modal_voice`, `lib.transport`) (`testing-and-evaluation.md`); the seven validation references (`lib.<type>_core`, above) are not in this table and not in the demo. The schematic palette lists them (`library`, and the references under `reference`; `ui-controls.md`) and adds them by a patch; not yet: opening a library composite's inside in the UI (ADR 0027 §6). The validation of library composites against native references (ADR 0027 §4) is M2.10-4's (above, and `testing-and-evaluation.md`).

### Interfaces

**Slot interface templates** (ADR 0037 §3) ship with the library beside the composites: `crates/signal-lib/interfaces/<name>.toml`, referenced from a slot as `slot = "lib.<name>"` (`project-format.md` §14), read-only, and listed by `signal_lib::interfaces()`. A template is "a composite with only exports" whose ports declare their types, since nothing inside them could tell; it has no render, so `tests/pinned.toml` pins each one's **text hash** (its `[interfaces]` table, `tests/pinned.rs`), and `CHANGELOG.md` names each with its hash, under the same rules as a composite's render hash. A template is **never changed incompatibly**: a changed port set or type is a new name. A render manifest lists the templates its slots start from (`library_interface_hashes`). The templates of phase 1, because its use cases need them (hard rule 8):

| Template | Inputs | Outputs | Parameters | Matches |
|---|---|---|---|---|
| `lib.voice` | `hz` (frequency, Hz), `gate` (gate), `velocity` (control, 0 to 1) | `out` (audio) | — | the studios' `moog_voice`, with no `map` |
| `lib.instrument` | as `lib.voice` | `l`, `r` (audio) | — | a mono voice through `map = { l = "out", r = "out" }` |
| `lib.deck` | `position` (control, beats), `jump` (events, trigger), `running` (gate), `tempo` (control, bpm) | `l`, `r` (audio) | `release` | any project, through its default sink exports `l`, `r` (ADR 0035 §2) |

The conventions: a template's port names are the ones the matching modules already use (exact names, ADR 0037 §4), a stereo pair is `l` and `r`, and a kind is given where it says what a port carries (a mismatch is only a warning). Others (`lib.effect`, …) wait until a project needs them.

## Sink taps

A **sink tap** (ADR 0035 §3) is the type a project loader plans in place of a sink instance where that instance must not be a sink: inside a project included in another, whose host's own sink is the run's one device sink. A library registers it with `Registry::set_sink_tap(sink, tap)` (queried with `Registry::sink_tap`), a name-to-name table the core does not interpret, like `exclusive_inputs`. A tap:

- has the sink's inputs and parameters, under the same names, so the instance keeps its address, inputs, parameters and connections;
- has no sink kind, and for each signal input `x` an audio output `x_out` carrying exactly what the sink's adapter would have taken from the sink: the same expression on the same parameter row, so a tap's outputs equal, as numbers, what the included project records alone (−0 included);
- clamps nothing: the clamp to the device's range happens once, at the host's sink.

`std.audio_tap` is `std.audio_out`'s (`l_out = gain·l`, `r_out = gain·r`). A sink type with no tap makes a project that holds one refused as an include, at the instance. Unlike a sink, whose inputs are recorded whatever they hold, a tap's outputs are checked like any output: a non-finite value or one beyond 10⁶ faults the tap (`semantics.md` §7).

## Testing a primitive: the contract suite

The contract suite, `crates/signal-contract`, checks the rules on this page and in `realtime-rules.md` for every primitive in `crates/` and `lab/`. It is a dev-dependency only. Its own tests (`crates/signal-contract/tests/self_test.rs`) are a complete example.

### Usage

Add `signal-contract = { workspace = true }` under `[dev-dependencies]`. Then write a test file under `tests/`:

```rust
signal_contract::install_counting_allocator!(); // once per test binary

mod sine_osc {
    signal_contract::contract_tests!(super::sine_osc_type());
}
```

The argument is an expression of type `ModuleType`. `contract_tests!(module_type, stimulus)` takes a custom stimulus generator instead of the default: a `fn(&ModuleDescriptor, seed: u64, frames: usize) -> Stimulus`. The macro expands to one `#[test]` per check, named after the check. To test several module types in one file, give each its own `mod`. Without `install_counting_allocator!()`, `no_allocation` fails rather than passing vacuously.

The default stimulus (`signal_contract::stimulus::random`) is derived from a seed:
- **Signal inputs**: values in the port's range, or in [−1, 1] when it has none, for every row (each element of a vector). The range endpoints and 0 come first, then uniform values with endpoints and 0 mixed in.
- **Parameters**: effective values in the declared range, shaped as the executor makes them (ADR 0016 §4a). Each segment starts with a set (a jump) or a linear ramp that reaches its target exactly. The first segments set the minimum, the maximum and 0 (or the default when 0 is out of range).
- **Event inputs**: triggers, sets and ramps on random frames, sometimes several on one frame.

Each case renders 4096 frames; for a type at a rate divisor above 1 these are 4096 of its ticks, which is all it ever sees (M2.8-6, "Rates and vectors"), so every check applies to it as written. Vector outputs are reported per element, as `port[e]`. A descriptor without one rate, or at a divided rate with event ports, a latency or a sink, or with a vector of 0 elements, fails every check. Direct instances get calls of the drawn sizes, repeated. State-output instances run per frame (`emit`, then `absorb`), as the reference engine does; before each `emit` the suite asks `due` until it is not `Pending`, as the offline engine does. Every check except `no_subnormals` runs 32 proptest cases (override with `PROPTEST_CASES`). Every check prepares its instances at 44 100, 48 000, 96 000 and 192 000 Hz.

### What each check proves

| Test | Proves | Fails when |
|---|---|---|
| `partition_invariance` | ADR 0002: output does not depend on how frames are split into calls | Rendering with *k* = 1 and with random call sizes (1–1024) differs in any signal sample (`f64::to_bits`) or in any emitted event, its payload bits or its frame |
| `reset_determinism` | `reset` returns to exactly the prepared state, random streams included | A render after `prepare` and a render after `reset` differ |
| `finite_output` | Finite in-range inputs give finite outputs, at range endpoints and 0 too | Any output sample, or `Set`/`Ramp` value emitted, is NaN or infinite. Unwritten samples read as NaN. |
| `no_allocation` | `process`, `emit`, `absorb` and `reset` neither allocate nor free | The counting allocator sees any allocation, reallocation or free during one of those calls |
| `state_output_honesty` | ADR 0016 §2: a type declared state-output never lets frame-*n* inputs change frame-*n* outputs | Changing frame *n*'s signal inputs and parameters, and adding an event, changes any output at frame ≤ *n*. Passes trivially for direct types. |
| `rate_agnostic` | `prepare` fully applies the rate it is given | An instance run at one rate and prepared again at another renders differently from a fresh instance prepared at that rate |
| `no_subnormals` | Decaying state is denormal-safe | After an impulse on frame 0 and 65 536 frames of silence, any output is subnormal. The impulse is 1 on signal inputs, clamped into the port's range; if that equals silence (0, or the in-range value nearest 0), the endpoint farther from silence is used, e.g. −1 for [−1, 0]. Event inputs get a trigger and parameters stay at default. It must be normal or exactly 0. |

Every check also fails on anything that makes a render invalid, reported before its own comparison:
- an event output overflows its sink (capacity: 16 events per frame of the largest call size, `max_frames`);
- a call of the primitive (`prepare`, `reset`, `process`, `emit`, `absorb`) panics. The report names the call and the frames it covered;
- the stimulus does not match the descriptor.

`state_output_honesty` renders nothing for direct types (it passes trivially), so for them it cannot see an overflow or a panic. The other six checks still do.

A `StateOutputPrimitive` cannot read same-frame input, because `emit` gets none. So `state_output_honesty` catches a type whose descriptor declares state-output while `create` returns an instance that does not honour it. The graph relies on that declaration to break cycles.

### Reading a failure

A failing test panics with one report:

```
contract check `partition_invariance` failed for module type `test.trigger_per_call`: at 44100 Hz, seed 9484394313631669105: k = 1 and blocks [2] differ; first difference at frame 1: event output `events_out`: Trigger at frame 1 vs Trigger at frame 2
minimal failing case: seed 9484394313631669105, blocks [2]; replay it with `Check::PartitionInvariance.run_case(&module_type, stimulus, &Case { seed: 9484394313631669105, blocks: vec![2] })`
```

- **Check and module type**: which rule broke, and for which type.
- **Rate and seed**: the seed determines the stimulus and the instance's random stream.
- **Frame of the first difference, and the output**: the first frame where the two renders (or the bad value) appear, named by output port. For `no_allocation` it is the first frame of the offending call, plus the call and the counts. For a panic it is the frames of the call that panicked (`frames 7..8` with calls of one frame), plus the panic message; `prepare` and `reset` panics are reported as before frame 0.
- **Minimal failing case**: what proptest shrank the case to. The call sizes shrink, the seed does not. `Check::run_case` replays it directly, without proptest.

Proptest records each failing case in `<test file stem>.proptest-regressions`, beside the test file, and replays it first on every later run. Commit that file with the fix, so that the case stays covered.
