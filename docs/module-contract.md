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
| `reset` | either | **no** | Returns to exactly the state `prepare` left, including random streams. |
| drop | control | frees | The engine returns old instances to the control thread for dropping. |

A call that panics anyway is caught by the engine: the instance is muted from that call's first frame and a `panic` fault is logged (`docs/semantics.md` §14). Nothing it wrote in that call is used.

### Trust boundary

M2.7-9. The synchronous calls, `process`, `emit` and `absorb` (and a latent module's `due` and `miss`), run inside the audio callback, and the callback cannot return until every one of them has. Each **must be bounded in time and never block**: no waiting on I/O, a device, a lock, another thread or a timer, however briefly or rarely. The engine contains a broken call that **panics** (above). It does **not** contain one that **hangs**: the whole output stalls, and no recovery is possible without risking the buffers the call still holds (`realtime-rules.md` "The trust boundary"). The engine only **detects** a hang: the callback is counted and reported with the instance still running (`realtime-rules.md` "Hangs are detected, not contained").

So computation that is **experimental or may block** (a device, a model, anything whose time is not bounded) must run **behind a latent (asynchronous) boundary**: the type declares a `latency` ("Declared latency" above), does the work on its own thread or device, and answers `due` at once, `Pending` or `Lost` when the result is not in; the engine never waits for it in real time. The GPU modules are built this way (ADR 0019 §3). A lab primitive (ADR 0015) meets the same boundary as any other.

`PrepareContext` gives:
- `sample_rate`: the master rate in frames per second;
- `max_frames`: the largest *k* any later call receives;
- `rng`: the instance's `RngStream`, `derive(master_seed, instance address)` (ADR 0008);
- `lent` (M2.7-2): what the program running the engine lends the instance, or `None`. The registry's `Host` decides it per plan (`lend`, called on the control side for every plan state the engine builds, the real-time mirror's included, with every instance in address order); the engine passes it on and never reads it. It is opaque (`Arc<dyn Any + Send + Sync>`): a module that uses it downcasts it to the type it expects, and **must work without it**. A GPU-backed module is lent its device, a `signal_gpu::Lease` placed by `signal_gpu::GpuHost` (`realtime-rules.md` "GPU"); without one it has no channel and its due results are lost, logged.

All randomness must come from that stream. Keep a clone of it, so that `reset` can restart it.

A GPU-backed module's channel is a `signal_gpu::PlacedChannel`. When its device is lost and the pool replaces it (M2.7-2), the module takes the new channel with `PlacedChannel::refresh(before)`, which is real-time safe (no allocation, lock or system call) and may be called from `due`: `before` receives the old channel first, and the module gives back every job it holds from it there (a job belongs to its channel); results in flight on the old channel are lost, with the cause `device_lost`.

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
- **Signal inputs**: values in the port's range, or in [−1, 1] when it has none. The range endpoints and 0 come first, then uniform values with endpoints and 0 mixed in.
- **Parameters**: effective values in the declared range, shaped as the executor makes them (ADR 0016 §4a). Each segment starts with a set (a jump) or a linear ramp that reaches its target exactly. The first segments set the minimum, the maximum and 0 (or the default when 0 is out of range).
- **Event inputs**: triggers, sets and ramps on random frames, sometimes several on one frame.

Each case renders 4096 frames. Direct instances get calls of the drawn sizes, repeated. State-output instances run per frame (`emit`, then `absorb`), as the reference engine does; before each `emit` the suite asks `due` until it is not `Pending`, as the offline engine does. Every check except `no_subnormals` runs 32 proptest cases (override with `PROPTEST_CASES`). Every check prepares its instances at 44 100, 48 000, 96 000 and 192 000 Hz.

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
