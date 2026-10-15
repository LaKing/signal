# Execution semantics

**Normative.** This page says exactly what a Signal project computes, frame by frame, and what a render writes. It implements [ADR 0016](adr/0016-execution-contract.md) (the execution contract) and refines ADR 0002 (frame semantics), ADR 0004 (events and parameters) and ADR 0008 (reproducibility). If this page and ADR 0016 disagree, ADR 0016 wins and this page has a defect.

The reference implementation is the per-frame executor and the offline driver in `crates/signal-engine` (ADR 0012). Every later engine, including the real-time one, must produce the same results bit for bit, with one exception (§14): after a panic in a `process` call of several frames the fault acts from that call's first frame, which depends on how the frames are split into calls.

Scope: the semantics are stated frame by frame; the engine runs them in blocks of *k* frames with the same result bit for bit (§13, M2.6-1), except after a panic in a call of several frames (§14). One master rate per project; ports at other rate divisors and vector ports (M2.8-6) are in §16. Plan swaps, incremental input and add/remove fades (M2-1) are in §12.

## 1. Frames

- Time is the **frame**: an integer count since project start, at the project's master rate (its sample rate, in whole frames per second). Nothing in the engine reads a clock.
- **Every instance runs every frame**, connected or not, unless it is suspended (§6) or muted by the fault guard (§7). An instance from which no sink can be reached only causes a validation warning (ADR 0016 §1).
- Values are f64 (ADR 0007). f32 appears only in `audio.wav` (§8).
- An instance of a type at rate divisor *d* > 1 runs every frame too, but is **called** on its ticks only, the frames that are multiples of *d*; its outputs hold between them (§16). Everything below holds for it as stated, frame by frame.

## 2. Frame phases

Each frame *n* runs in four phases (ADR 0016 §2):

1. **External input.** A plan swap committed for *n* takes effect (§12). Then the edits effective at *n* take effect (suspend, resume; §6). Then the external events effective at *n* become inputs of their targets. An input's effective frame is the frame it is stamped for, or a later one for input pushed late (§12).
2. **Emit.** Every state-output instance writes its frame-*n* outputs from its state alone. It gets no inputs and no parameter values.
3. **Process.** Direct instances run in schedule order. Before an instance runs, its frame-*n* inputs are computed: signal inputs (§3), events (§4) and parameters (§5).
4. **Absorb.** Every state-output instance gets its frame-*n* inputs, computed the same way, and updates its state. Its frame-*n* output was already fixed in phase 2, so these inputs show first at frame *n* + 1.

The **schedule** is a topological order of the dependency graph: each connection A → B makes B run after A, unless A is state-output. Ties are broken by instance address. The output does not depend on which valid order is used. The engine checks the order it is given rather than trusting it, and accepts any valid one.

A feedback loop must pass through a state-output instance (for example a unit delay); a loop without one is rejected when the plan is built, naming the loop's instances.

## 3. Signals

A signal input's value is the sum of its connections' values, in **canonical source order** (byte-wise order of the source port address), each multiplied by the connection's scale, starting from +0:

`in = ((0 + s₁·x₁) + s₂·x₂) + …`

An unconnected input reads 0. The value read is the source's output as other instances see it, after any fade (§6, §7). Floating-point addition is not associative, so the fixed order is what makes sums bit-stable.

## 4. Events

### Delivery

An event stamped for frame *n* is one of its receiver's frame-*n* inputs (ADR 0016 §3):

- A **direct** receiver gets it before `process`, so it can affect the receiver's frame-*n* output.
- A **state-output** receiver gets it in absorb, whatever the source (external or module, emitted before or after its emit phase). It changes the output first at frame *n* + 1.

Module-emitted events travel along connections within the same frame. Each connection delivers its own copy, unchanged; a connection's `scale` applies to signals only (§10). In a per-frame call every event's offset is 0.

An event output keeps at most **16 events per frame**. A 17th is an overflow, which the fault guard treats as a fault (§7).

### Canonical order

The events reaching one input port in one frame are applied in this order:

1. **External events**, by sequence number (each external event carries a unique one).
2. **Module events**, by source port address, byte-wise.
3. Within one source, in the order it emitted them.

Example: `/drums/kick/env.out` sorts before `/lfo.out`, because `d` sorts before `l`. Two setters `/a` and `/b` send `set(0.25)` and `set(0.75)` to `/p.value` in the same frame. `/a.out` < `/b.out`, so `/b`'s set applies last and the base is 0.75. Rename `/a` to `/c`: now `/b.out` < `/c.out`, and the base is 0.25. The order depends only on addresses and sequence numbers, never on the schedule.

## 5. Parameters

A parameter is an input port with a **base** and optional **modulation** (ADR 0004, ADR 0016 §4):

- **Events change the base:** `set` and `ramp`. They come from outside or along event connections into the parameter.
- **Signal connections are modulation.** They never change the base.

### The base, each frame

For a frame the instance runs (ADR 0016 §4a):

1. **Advance.** If a ramp is running, the base becomes the ramp's value at this frame (below).
2. **Apply events** in canonical order, each acting on the current value:
   - `set(v)`: the base becomes *v*, and any running ramp ends.
   - `ramp(to, N)` with *N* = 0: exactly `set(to)`.
   - `ramp(to, N)` with *N* ≥ 1: any running ramp ends. A new one starts from *b* = the current base, at *n₀* = this frame. The base stays *b* this frame.
   - A trigger sent to a parameter has no effect. It is logged as a discard and counted (§10).

### Ramp values

A ramp (*b*, *to*, *N*, *n₀*, *c*) at the frame where the instance has run *i* frames since *n₀*:

- for 0 ≤ *i* < *N*: `b + (to − b) · c(i / N)`. This is evaluated in f64 from the integers *i* and *N*: first the division *i*/*N*, then the curve, then one multiply and one add. It is never accumulated.
- for *i* ≥ *N*: exactly `to` (assigned, not computed), and the ramp ends.

Every curve has `c(0) = 0` and `c(1) = 1` exactly. M1 has one curve, `linear`: `c(x) = x`.

Worked examples, a ramp event at frame 10 from base 0:

| Frame | 10 | 11 | 12 | 13 | 14 | 15 |
|---|---|---|---|---|---|---|
| `ramp(1, 0)` | 1 | 1 | 1 | 1 | 1 | 1 |
| `ramp(1, 1)` | 0 | 1 | 1 | 1 | 1 | 1 |
| `ramp(1, 4)` | 0 | 0.25 | 0.5 | 0.75 | 1 | 1 |

**An interruption starts from the interrupted value.** Take a ramp 0 → 1 over 8 frames from frame 0. At frame 4 a `ramp(0, 4)` arrives. Step 1 first advances the old ramp to 0.5, so the new ramp starts from *b* = 0.5:

| Frame | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 |
|---|---|---|---|---|---|---|---|---|---|
| base | 0 | 0.125 | 0.25 | 0.375 | 0.5 | 0.375 | 0.25 | 0.125 | 0 |

A `set(0.3)` at frame 4 instead makes the base 0.3 from frame 4 on.

**Suspended frames do not count.** *i* counts only frames the instance runs (§6). Take a ramp 0 → 1 over 8 frames from frame 2, with the instance suspended on frames 5–14. The base is 0.25 at frame 4, stays there while suspended, is 0.375 at frame 15, and reaches exactly 1 at frame 20 instead of frame 10.

### The effective value

The module never sees the base, the events or the modulation separately. It receives the **effective value**:

1. The modulation sum *m* is computed like a signal input (§3), over the parameter's signal connections.
2. The value is combined as the parameter declares:
   - with no signal connections: the base, whatever `combine` says, unless the parameter holds modulation from instances removed by a plan swap (§12). Then the held sum is *m*, until a `set` (or `ramp` with *N* = 0) reaches the parameter or a later swap gives it a signal connection. A parameter that only lost connections to instances still in the plan returns to the base on the swap frame;
   - `add`: `base + m` (the default);
   - `multiply`: `base · m`;
   - `replace`: `m`.
3. The result is **clamped** to the declared range, in every case. The base itself is never clamped.

Example, with range [0, 1], base 0.75 and modulation 0.5:
- Add gives 1.25, clamped to 1.
- Multiply gives 0.375.
- Replace gives 0.5.

With no modulation, `set(7)` gives base 7 and effective value 1.

A `set` during modulation changes only the base. For example: base 0.25, one modulation source of 0.125 at scale 2, Add. The effective value is 0.25 + 0.25 = 0.5. After `set(0.0625)` the base is 0.0625 and the effective value is 0.3125; the modulation is untouched.

## 6. Suspension

`suspend(instance, s)` and `resume(instance, r)` are edits. They take effect at the start of their frame (phase 1). Per instance they must alternate, at increasing frames (*r* > *s*), and neither may be stamped for a frame that has already run. During [*s*, *r*) the instance's **time stops** (ADR 0016 §1a): it is not called, its state is frozen, and its ramps do not advance.

### Outputs

- **Signal outputs** hold their value from frame *s* − 1.
- **Event outputs** emit nothing; a suspended trigger source never repeats its last trigger.
- **Outputs of kind `audio`** fade instead, over *F* frames.

**The fade length** *F* is 5 ms at the sample rate, rounded half up, and at least 1 frame. It is computed exactly in integers: *F* = max(1, ⌊(rate · 5 + 500) / 1000⌋).

| Rate | 44 100 | 48 000 | 96 000 | 192 000 |
|---|---|---|---|---|
| *F* | 221 | 240 | 480 | 960 |

**The fades.** Gains are integer ratios. Each value is one f64 division (integers converted to f64), then one multiply by the value. The last frame of a fade is assigned, not computed.

- **Suspend at *s*.** `held` is the port's output at *s* − 1, what readers saw, including any fade-in in progress. On the *j*-th suspended frame (*j* = 0 at *s*) the output is:
  - `held · ((F − 1 − j) / F)` for *j* < *F* − 1;
  - exactly +0.0 from *j* = *F* − 1 on, i.e. from frame *s* + *F* − 1.
- **Resume at *r*.** The output fades in from 0 again, whatever it was. On the *j*-th running frame (*j* = 0 at *r*) the output is:
  - `live · ((j + 1) / F)` for *j* < *F* − 1, where `live` is what the module wrote;
  - exactly `live` at *j* = *F* − 1, i.e. frame *r* + *F* − 1, and after.

  An emit at *r* uses the frozen state.
- A new suspend or resume always restarts its fade and abandons the one in progress.

| Case | Suspended frames | After resume |
|---|---|---|
| Suspended for at least *F* frames | Exactly +0.0 from *s* + *F* − 1 to *r* − 1 | Fades in from 0; exactly live from *r* + *F* − 1 |
| Suspended for one frame (*r* = *s* + 1) | Only *s*, at `held · ((F − 1) / F)` | A full *F*-frame fade from 0 |
| Suspended during a fade-in | Fades out from the actual output at *s* − 1; exactly +0.0 at *s* + *F* − 1 | — |
| Resumed during a fade-out | — | Fades in from 0; exactly live at *r* + *F* − 1 |

Worked trace at 48 kHz (*F* = 240): a constant audio output of 0.75, suspended at frame 10 and resumed at frame 11.

| Frame | 9 | 10 (*s*) | 11 (*r*) | 12 | 13 | … | 249 | 250 | 251 |
|---|---|---|---|---|---|---|---|---|---|
| Output | 0.75 | 0.746875 | 0.003125 | 0.00625 | 0.009375000000000001 | … | 0.746875 | 0.75 | 0.75 |

Frame 10 is 0.75 · (239/240), and frame 11 is 0.75 · (1/240). The fade-in spans frames 11 to 250, all 240 of them, and frame 250 is exactly 0.75.

Interrupted fades at the same rate:
- Resumed at 11, then suspended again at 111: `held` is 0.75 · (100/240) = 0.3125, and frame 350 is the first exact +0.0.
- Suspended at 10, resumed at 110: frame 109 is 0.4375, frame 110 is 0.003125, and frame 349 is exactly 0.75.

After a suspension shorter than *F*, the output therefore steps from the partly faded held value down to `live/F`. This is what ADR 0016 §1a prescribes ("fade in from 0 to their live value over *F* frames").

### Inputs while suspended

- **Signals** are ignored.
- **`set`** and `ramp` with *N* = 0, sent to a parameter, change the base at once and cancel any ramp, running or pending.
- **`ramp`** with *N* ≥ 1 is held as **pending**; only the latest is kept. At resume it is applied after the advance step and before the events stamped for *r*, so it starts from the base at *r*.
- **A ramp running at *s*** pauses and continues at *r* with its remaining frames.
- **Every other event**, such as a trigger, or any event on a non-parameter input, is **discarded** and counted. The count is reported per instance and in the resume record of `log.jsonl`. It never affects output.

## 7. Faults

After every call of an instance (`process` or `emit`), the engine checks what it produced, before anyone reads it (a call that panics is a fault too: §14):
- every signal output value, and every value carried by an emitted `set` or `ramp`, must be finite (otherwise reason `non_finite`) and at most 10⁶ in magnitude (otherwise reason `magnitude`);
- no event output may have overflowed (otherwise reason `event_overflow`).

On a fault at frame *n*:
- The instance is **muted**: it is never called again in this run.
- Its events of frame *n* are not delivered, and events sent to it are discarded and counted.
- Every signal output fades to 0 over **64 frames** from its output at *n* − 1 (0 if *n* is its first frame). On frame *n* + *j* the output is:
  - `held · ((63 − j) / 64)` for *j* < 63;
  - exactly +0.0 from *n* + 63 on.

  The evaluation order is the same as in §6.
- A `fault` record names the frame, the instance, the first offending output port (signal outputs before event outputs, in descriptor order) and the reason.
- Everything else keeps running.

**Late results** (ADR 0019 §3, M2.6-4). An instance whose type declares a latency (`module-contract.md`, "Declared latency") is asked before each `emit` whether its due result is in. Offline the engine waits until it is. In real time it never waits: a result not in when due (or lost) is a fault with reason `late_result`, logged at the frame it was due (the first frame of the module's block) with the instance's first output port, and counted (`Engine::late_results`, `RunnerStatus::late_results`). The instance outputs silence for that block of its own and is **not muted**; the guard above still applies to what it outputs.

## 8. The offline render

The offline driver renders *N* frames from frame 0. Since ADR 0022 (M2.7-9) the engine's offline driver (`run_offline`) hands every sink's inputs to an output generically; what follows is the audio adapter's policy (`signal-output`: `render`, `Render::write`), which the real-time audio runner shares (§12):

- **The audio sink.** The plan must contain exactly one instance whose module type declares `sink = audio`. Zero or several is an error.
- **What is recorded.** Each frame, the driver records the sink's **signal inputs** as §3 sums them, one channel per signal input, in the order the module type declares them. This does not depend on whether the sink instance itself is suspended or muted (§10).
- **Sink gain.** If the sink's module type declares a parameter named `gain`, each recorded input is multiplied by that parameter's effective value (§4) in the same frame. While the sink is suspended or muted the executor does not recompute it, so the last effective value applies (§10, choice 11). `std.audio_out` records `gain·l` and `gain·r` this way.
- **`audio.wav`**: IEEE float, 32 bits, at the project sample rate, channels interleaved per frame. Each value is converted from f64 to f32 with round-to-nearest, ties to even.
- **Blocks.** The driver runs blocks of the engine's block capacity (§13; `signal render --block <k>`, default 256). The block size changes nothing above, except after a panic in a `process` call of several frames, whose fault acts from the call's first frame (§14).
- **Edits during a render.** `render_with` lets the caller commit a swap or push input before each frame (§12); `render_blocks` before the frames it names, splitting blocks there. `signal replay` renders a recorded session this way (`session-format.md`), split at its commands' frames. Each block's log is drained after it is recorded. A swapped-in plan must keep the audio sink: exactly one, at the same address, with as many signal inputs.

## 9. Artifacts

A render directory holds `audio.wav` (§8), `log.jsonl` and `manifest.json`. `signal replay` writes the same artifacts for a recorded session; the session itself is specified in `session-format.md`. The `Manifest` type is defined in `signal-engine` (not `signal-core`, as `docs/architecture.md` once listed; its crate table now says so).

### `log.jsonl`

One JSON object per line, with a newline after every line, field order as shown. Records:

```json
{"type":"swap","frame":2,"version":1,"plan_version":1}
{"type":"event","frame":2,"target":"/p.value","payload":{"set":0.125},"source":{"external":{"sequence":1}}}
{"type":"event","frame":0,"target":"/c.in","payload":"trigger","source":{"port":"/t.out"}}
{"type":"event","frame":4,"target":"/p.value","payload":{"ramp":{"to":0.5,"frames":3,"curve":"linear"}},"source":{"external":{"sequence":2}}}
{"type":"discard","frame":3,"target":"/c.in","payload":"trigger","source":{"port":"/t.out"},"reason":"suspended"}
{"type":"suspend","frame":3,"address":"/c"}
{"type":"resume","frame":5,"address":"/c","discarded":2}
{"type":"fault","frame":8,"address":"/f","port":"/f.out","reason":"non_finite"}
```

- **`event`**: one record for every event that took effect: delivered to an event input of a running instance, applied to a parameter's base, or held as a pending ramp. One record per receiving port and connection.
- **`discard`**: an event that reached its target and had no effect. Reasons:
  - `suspended`;
  - `muted`;
  - `not_a_parameter_event` (a trigger sent to a parameter).
- **`suspend`** / **`resume`**: an edit and the frame it took effect. `discarded` counts the events discarded during that suspension.
- **`swap`**: a plan swap and the frame it took effect (§12). `version` is the engine's plan version after it; `plan_version` is the swapped-in plan's own version.
- **`fault`**: see §7. Reasons:
  - `non_finite`;
  - `magnitude`;
  - `event_overflow`;
  - `panic` (§14).

  A fault of an instance without outputs (a `panic`, or a `late_result`) has no `port` in its line; the line is still written.

  A `late_result` whose result was lost, and whose module said why (`StateOutputPrimitive::lost_cause`, M2.7-5), has a `cause`: `validation`, `out_of_memory`, `mapping`, `internal`, `uncaptured`, `stopped` or `device_lost`. Other lines have none, so logs without such a loss are unchanged. An exact replay logs the cause its session recorded (§15). For example: `{"type":"fault","frame":512,"address":"/bank","port":"/bank.out","reason":"late_result","cause":"device_lost"}`.
- **Order within a frame:**
  1. a swap;
  2. edits, by instance address;
  3. events and discards, by target port address (in delivery order within a target);
  4. faults, by instance address.

  Frames are in increasing order. The log therefore does not depend on the schedule.
- Numbers use the shortest representation that reads back to the same f64.

### `manifest.json`

Pretty-printed JSON with a final newline:

```json
{
  "provenance": {
    "signal_revision": "cf613bd1fc85a9a8dc6de643e68080ea99ea1265",
    "signal_dirty": false,
    "rustc_version": "rustc 1.99.0 (b940084d7 2026-09-28)",
    "project_file_hashes": { "project.toml": "…" },
    "cargo_lock_hash": "…"
  },
  "seed": 42,
  "sample_rate": 96000,
  "frames": 960000,
  "debug_assertions": false,
  "log_fnv1a64": "…",
  "audio_fnv1a64": "…"
}
```

- **`provenance`** is supplied by the caller (the CLI): the Signal revision and whether the source was dirty, the `rustc --version` line, a hash per project file (by path relative to the project folder) and a hash of `Cargo.lock`, in a format the caller chooses.
  - `signal_dirty` is `false` for a clean source, `true` for uncommitted changes, and `"unknown"` when the source state could not be read.
  - `signal render` writes `"unknown"` for any string field whose source is unavailable (`docs/project-format.md`, "Render provenance").
- **The engine fills in the rest:**
  - `seed`: the master seed;
  - `sample_rate`;
  - `frames`: the number of frames rendered;
  - `debug_assertions`: the build profile, true for a debug build;
  - `log_fnv1a64`: the hash of the bytes of `log.jsonl` exactly as written;
  - `audio_fnv1a64`: the hash of the f32 samples as `audio.wav` stores them (4 bytes each, little-endian, interleaved). That is the WAV data chunk, so both hashes can be checked from the files.
  - `same_machine` (ADR 0019 §3, M2.6-4), only when a plan of the render has an instance whose type is reproducible on the same machine and build only (a GPU-backed module): one entry per instance, by address, `{"instance", "type", "latency_blocks", "block_frames"}`. The render is then the same bit for bit on that machine and build, not across machines. Without one the key is absent and the manifest is exactly as before.
  - `compute_devices` and `placement` (M2.7-2), only when a plan of the render has a GPU-backed instance: `compute_devices` lists the devices the run could place it on, sorted by id, each `{"id", "name", "backend"}` (absent without one); `placement` lists every GPU-backed instance by address, `{"instance", "device", "pinned"}`: the id of the device it ran on (`null` without one: its due results were lost, logged) and the project's pin as written, if any. On the same machine with the same device set the placement is the same on every run; different GPU models may give different results (`same_machine`). Without a GPU-backed instance both keys are absent and the manifest is exactly as before.
  - `ideal` (M2.7-4, §15), only `true`, only for an ideal render of a session (`signal replay --ideal`, or a session without boundary records): every latent result was awaited, so it is not what was played. Absent otherwise.
  - `device_conversion` (M2.7-4, §15), only `true`, only for the exact replay of a session with boundary records: `audio.wav` and `audio_fnv1a64` store each sample as the device received it (finite samples clamped to [−1, 1], anything else 0, then rounded to f32) instead of rounding it to f32 alone. Absent otherwise.
  - `trust_provenance` (M2.7-8, §15), only `true`, only for an exact replay that `--trust-provenance` let run although the session was recorded by another build or on another machine, with results to recompute: each was still checked against its recorded hash. Absent otherwise.
- **FNV-1a 64** (`signal_core::fnv1a64` and the incremental `signal_core::Fnv1a64`):
  - start from `0xcbf29ce484222325`;
  - for each byte, XOR it in, then multiply by `0x100000001b3` modulo 2⁶⁴.

  Hashes are written as 16 lowercase hex digits. FNV-1a is **not cryptographic**: it detects accidental change, not tampering.

## 10. Choices where ADR 0016 is silent

These choices are part of the semantics above. Each was made for M1 and can change only through review:

1. **Scale on event connections is not applied.** Events arrive unchanged; a connection's `scale` multiplies signals only. (A validation warning for a scale on an event connection is a possible follow-up.)
2. **The offline driver records the audio sink's input sums** whether or not the sink instance is suspended or muted.
3. **The `Manifest` type lives in `signal-engine`.** The crate table in `docs/architecture.md` was edited to say so.
4. **An event-output overflow (more than 16 events in a frame) is a fault**, handled like a non-finite or oversized value. `docs/module-contract.md` says the engine reports overflows as faults.
5. **`audio_fnv1a64` covers the f32 samples as written to `audio.wav`**, not the f64 values, so that it can be checked from the file.
6. **The fault fade starts from the port's last output** (frame *n* − 1, as readers saw it).
7. **Each frame's log records are written in canonical order** (§9), so the log, like the audio, does not depend on the schedule.
8. **A trigger delivered to a parameter is discarded and counted** (reason `not_a_parameter_event`). Only `set` and `ramp` change a base.
9. **Every resume fades in from 0, even after a suspension shorter than *F*** (ADR 0016 §1a), so a short suspension steps down to `live/F`.
10. **The "random *k*-partitions" clause of ADR 0016's ramp verification.** A test checks that every frame's base equals the closed form of (*i*, *N*), evaluated over random block partitions; since block execution (§13), the determinism matrix also runs ramps in blocks of every size and in random partitions.
11. **The offline driver applies the sink's `gain` parameter** (§8, M1-6). A sink has no outputs, so it cannot expose gained values itself; the driver multiplies the recorded inputs by the sink's effective `gain`. A sink type without a `gain` parameter is recorded unchanged. Since ADR 0022 this is the audio adapter's policy (`signal-output`), not the engine's; the result is the same bit for bit.

## 11. Determinism

The output follows from these inputs alone:
- the plan, and each plan swapped in with its swap frame (§12);
- the seed: each instance's random stream is `RngStream::derive(seed, address)` (ADR 0008);
- the sample rate;
- the external events and edits, with their effective frames;
- the build.

Two renders with the same inputs produce byte-identical `audio.wav`, `log.jsonl` and hashes. Any valid schedule produces the same output and the same log.

The rounding rules:
- everything is computed in f64;
- sums are taken in canonical order (§3);
- ramp and fade gains divide first, then multiply once (§5, §6, §7);
- ends of ramps and fades are assigned;
- f64 is converted to f32 only for `audio.wav`, rounding to nearest, ties to even.

## 12. Plan swap and live edits

ADR 0006: a running engine replaces its plan at an exact frame, and takes external events and edits while it runs. This section is normative for `Engine::prepare_swap`, `commit_swap`, `push_external`, `push_suspend`, `push_resume` and `drain_log`.

### Swaps and versions

- A swap is prepared off the real-time thread (it builds the new plan's instances and buffers) and committed between blocks, effective from the next frame *n*, the first of the next block (§13). Committing allocates nothing and moves state; the replaced plan is dropped elsewhere.
- The engine's **plan version** is 0 for the plan it was built with and goes up by one per swap. The swap record `{"type":"swap","frame":n,"version":v,"plan_version":p}` is the first record of frame *n*; `plan_version` is the swapped-in plan's own `version`, which the caller assigns.
- At most one swap per frame. A swap prepared by another engine or at another version, or already committed, is refused.
- The log holds one block, and its records name the ports and instances of the plan that wrote it, so a block's log must be drained before a swap is committed; a commit refuses an undrained log.

### What survives

An instance **survives** when the new plan has an instance at the same address with the same module type (and descriptor). A changed type at the same address is a removal plus an addition. A survivor keeps, moved and not copied:

- its primitive object: DSP state and random stream;
- its parameter bases, with running and pending ramps (§5, §6);
- its suspension, fade and fault state, and its discard counts;
- its outputs as readers last saw them, and the values its fades start from.

The new plan's parameter values do not apply to a survivor: its bases move. A caller that wants a new value sends a `set`.

### Connections

Connection changes between survivors take effect on the swap frame with no fade (crossfading changed connections is not done). This includes a disconnect that leaves a parameter with no signal sources: from the swap frame it uses its base (§5).

A connection across rate divisors runs through a converter instance (§16), so changing one changes the instances, not only the connections: adding the first such connection from a source port to a divisor adds its converter, whose `audio` outputs fade in as any added instance's (below); removing the last one removes its converter, as any removal (a live edit suspends it first, so its `audio` output fades out to exactly +0.0 before the swap).

### Added instances

An added instance is created and prepared like any instance (its random stream is `RngStream::derive(seed, address)`, its `generation` *v* + 1 for a swap prepared at plan version *v*; an instance of the plan the engine was built with has 0) and runs from the swap frame. Its `audio` outputs fade in exactly as after a resume (§6): on its *j*-th frame the output is `live · ((j + 1) / F)` for *j* < *F* − 1, and exactly `live` from *j* = *F* − 1 on. Its other signal outputs and its event outputs start at once.

### Removal

- **Removal is two steps.** Suspend the instance at frame *s* (§6), then swap to a plan without it at *s* + *F* (`Engine::removal_swap_frame(s)`). From *s* + *F* − 1 on its `audio` outputs are exactly +0.0, so dropping their terms from the canonical sums that read them (§3) changes nothing bit for bit: the audio fades out exactly like an ordinary suspend.
- A removal without the prior suspend drops the instance's contribution on the swap frame with no fade.
- Outputs of other kinds hold their value while suspended (§6), so their terms step to absent at the swap.
- **Held modulation.** A surviving parameter left with no signal sources, whose lost sources include removed instances, holds the canonical sum of `scale · output` over those removed sources, read at the commit from their outputs as readers last saw them. After the suspend-then-remove protocol that sum is exactly +0.0, so a Multiply or Replace parameter keeps the value it had under the suspension instead of returning to its base; skipping the protocol holds whatever the removed source's last output was. A surviving source that was disconnected in the same swap does not contribute. The hold lasts until a `set` (or `ramp` with *N* = 0) reaches the parameter or a later swap gives it a signal source. While it lasts, a live session differs from a fresh load of the same files.

### Pushed input and effective frames

- External events and suspends and resumes may be pushed while the engine runs, into fixed-capacity queues (`EngineConfig::queue_capacity`, default 4096). A full queue refuses the input and hands it back; nothing is dropped. Pushed input stays in its queue after it is used until the caller reclaims it (`reclaim_externals`, `reclaim_edits`), so that nothing is freed on the real-time thread; until then it counts against the capacity.
- Input arrives in frame order. Its **effective frame** is the frame it is stamped for, raised to the effective frame of the previous push of the same kind, and for an edit to the next frame to run. An input stamped for a frame that has already run is therefore applied on the next frame, and its log records carry that effective frame: the log is the truth for replay.
- Same-frame external events are ordered by target, then sequence number, whether they came from the constructor's list or were pushed, so pushing a list's events in frame order gives exactly what the list gives. Sequence numbers must be unique across the list and pushed events; this is the caller's duty (duplicates are ordered list first, then in push order).
- Targets and instances are resolved at the effective frame, against the plan running then: an event for an instance that a later swap adds applies once it exists. External events whose target is not an event input or parameter of that plan, or whose `set` or `ramp` value is not finite, and edits that name no instance of it or do not fit the instance's state (a suspend of a suspended instance, a resume of a running one, a second edit of one instance in a frame) have no effect: they are counted (`Engine::input_discards`), not logged.
- Pushed edits and the edits scheduled with `suspend` and `resume` before they run may be mixed. Edits effective at the same frame apply scheduled first, then pushed. A pushed edit, once applied, is what `suspend` and `resume` check against, unless a later scheduled edit of the instance is recorded: then, until that frame, only `push_resume` can resume the instance.

### Draining the log

`Engine::drain_log` hands the last block's records to the caller frame by frame, each frame's in the order of `log.jsonl` (§9), and empties the log, without allocating, so a real-time caller keeps the log bounded by draining it after every block. What it hands out is exactly the per-frame log of those frames (§13).

### Real-time runner

`signal_engine::Runner` runs the engine inside an audio callback of any size, fed by `signal_engine::Controller` from the control thread (M2-3; the mechanics are in `realtime-rules.md`, "The runner"). It changes nothing above: it is the same executor, driven by the same live-edit calls.

- **Order of operations.** Per callback: the queued commands are taken; then the frames run in blocks of up to the engine's block capacity (the device buffer, at most 256), split at every swap's frame and every command's effective frame (§13), a swap committed before the block it is effective from, events and edits applied by the engine at their effective frame (§2, phase 1); after each block the audio sink's channels are written frame by frame and its log records sent; at the end, meters, parameter values (observation only) and the status.
- **Requested and effective frame.** Every command carries a **requested** frame: its stamp. Its **effective** frame follows the rules of "Pushed input and effective frames" above: the latest of the stamp, the next frame to run, and the effective frame of the previous command of its kind (event; suspend or resume); for a swap, one past the previous swap's frame. A command whose requested frame has already run is **late** and applies on the next frame. The runner reports both frames for every command, and counts late ones; `log.jsonl` records only the effective frame, so the log of a live run equals the log of its replay.
- **Latency.** A command stamped by default is requested for the current frame (as the runner last reported it) plus the latency, two device buffers unless configured otherwise. A command arriving within the latency applies exactly at its requested frame, whatever the callback size, unless an earlier command of its kind raised it (its effective frame is never before the previous one's); only a command whose requested frame has already run is late.
- **Output.** Each frame, every audio-sink channel is the value the offline driver records (§8: the sink's input sum times its `gain`), clamped to [−1, 1] and converted to f32 (round to nearest, ties to even). Where no sample is clamped, the output equals the offline render of the same frames and commands bit for bit as f32 (V9), for any sequence of callback sizes (*k*-partition invariance, ADR 0002), except from a panic in a `process` call of several frames on, whose fault frame the callback and block sizes place (§14). Clamped samples are counted. `audio.wav` is not clamped (§8).

## 13. Block execution

ADR 0019 §1 (D-EXEC-3, M2.6-1). `Engine::run_block(k)` runs the next *k* frames, 1 ≤ *k* ≤ `EngineConfig::block_capacity` (default 256, also the `max_frames` every primitive is prepared with); `run_frame` is `run_block(1)`. *k* is a performance setting: it never appears in project files and never changes the output. **For every block size and every partition of a run into blocks, the audio, the log, the parameter values and every counter are bit-identical to running the frames one at a time as §§1–12 describe.** The one exception (§14, ADR 0020 as amended for M2.7-3): after a `panic` fault in a `process` call of several frames, which can only be placed at the call's first frame. The thread count never changes anything, a panic included. The goldens, the V-tests and the determinism matrix (`crates/signal-engine/tests/blocks.rs`, `crates/signal-cli/tests/blocks.rs`, anchored to hashes of the per-frame engine) check this.

### Units

- When a plan is instantiated (when the engine is built and at `prepare_swap`), it is condensed into strongly connected components over every connection, from state-output sources too. The **units** run in a topological order of the condensation, ties broken by the earliest schedule position among a unit's members, so the order depends only on the plan.
- An instance outside any cycle is a unit of its own. A **direct** one is called once with `process` for the frames of the block (or of a segment, below), its inputs computed frame by frame before the call. A **state-output** one covers the block's frames too, but calls `emit` and `absorb` one frame at a time: `emit` gets no inputs, so it cannot write the output of frame *n* + 1 when that depends on the input of frame *n* (`std.unit_delay` writes one value to every frame it is given). `module-contract.md` states this limit.
- An instance at rate divisor *d* > 1 (§16) is gathered frame by frame like any other, but a direct one's `process` covers only the segment's ticks, from its first tick to the segment's end; the frames before that tick show its held outputs. A state-output one emits and absorbs at its ticks only.
- A **cycle** (closed through state-output instances, §2) is one unit whose members are interleaved in sub-blocks of one frame, its shortest possible loop delay (1 frame for `std.unit_delay`; `std.delay` is at least 1 frame): each frame its state-output members emit, its direct members process in schedule order, and its state-output members absorb.
- Every consumer combines its sources in canonical order (§3, §4), so the unit order decides only when values are computed, never what they are.
- A **latent** state-output instance (a declared latency, ADR 0019 §3) runs the same way, one frame at a time. Its latency counts blocks **of its own** (`block_frames`, fixed for its type and recorded in the manifest), not the engine's *k*: so its output, too, is the same for every *k* and thread count. Before each of its `emit`s the engine asks for the due result, waiting offline and never in real time (§7, "Late results").

### Inside a block

- **External input.** The external events effective in the block are staged at their effective frames (a late one at the block's first frame) and delivered with in-block offsets, in the order of §4. Edits **split** the block internally: an edit applies at the first frame of a segment, exactly as in phase 1 (§2).
- **Signals and parameters**, per frame and with the same arithmetic: canonical-order sums (§3), modulation, ramps and effective values (§5); a suspended or muted instance's effective value stays as it was.
- **Suspension** (§6): fades and held outputs per frame.
- **Events between modules.** An event output keeps at most 16 events **per frame** of a call; the 17th event of a frame is an `event_overflow` at that frame, whatever the order in which a block call emits its frames' events. An event whose offset is past the call's last frame counts against that last frame (a breach of the contract).
- **The fault guard** checks a call's frames in order (signal outputs, then event outputs, in descriptor order). A fault at frame *n* acts exactly from *n* (§7): the frames before *n* are committed, the outputs fade from their values at *n* − 1, none of the call's events from *n* on is delivered, and what the instance's inputs did after *n* (log records, parameter bases and ramps, discard counts) is undone and done again as for a muted instance.

### Splitting

- A block never spans a plan swap: swaps are committed only between blocks, so callers split at swap frames. The offline driver splits where its caller asks (`render_blocks`); `signal render --block <k>` sets the block size; `signal replay` splits at the session's command frames.
- The real-time runner splits each callback into blocks of up to its capacity at every swap frame and at every command's effective frame (§12, "Real-time runner").
- Pushed events and edits that a caller queues ahead land at their effective frames inside a block.

### The log

The log holds one block. Draining it yields the block's frames in order, each frame's records in the canonical order of §9: exactly the per-frame log. Its capacity covers a block: per frame, one edit per instance and 16 events per event connection; one fault per instance; every external event of the block (pushed: the queue capacity; up front: the most stamped in any *K* consecutive frames, all targets in total).

### Threads

ADR 0019 §2 (M2.6-2). A segment may run on several threads (`EngineConfig::threads`, `--threads`, the Setup's "CPU workers"; `docs/realtime-rules.md` "Worker threads"). The units are grouped into **tasks**: runs of consecutive units in the order above, a unit joining the previous one's task only if that one feeds it and the task is still estimated cheap (`docs/benchmarks.md`). A task becomes ready when the tasks it reads have completed. A task writes only its own instances' outputs, inputs, parameters and event stores; every sum, modulation and event merge is still computed by the receiving instance in canonical source order (§3, §4), never in the order tasks complete; each task logs into its own log, and the logs are joined in task order, which is unit order, after the segment. **The thread count, like *k*, is a performance setting: for every thread count and every timing, the audio, the log, the parameter values and every counter are bit-identical to one thread**, and so to the per-frame engine. The determinism matrix runs threads 1, 2, 3, 4 and 8 at *k* = 1, 64 and 256 through the offline driver and the real-time runner.

## 14. Failures

M2.7-3 (ADR 0020, amended). A failure is never silent wrong output: either one instance is faulted, and the rest of the plan runs on without its output, or the engine stops with an error. Nothing that failed is published as if it had succeeded.

### An instance that panics

A call of a primitive (`process`, `emit`, `absorb`, or a latent module's `due` or `miss`) that panics breaks the module contract (`module-contract.md`). The engine catches the panic at that call, on whichever thread ran it, and:

- **The faulted instance** is the one whose call panicked. Its state is no longer trusted: it is **muted** (§7) and never called again.
- **From when:** the first frame *n* of the call that panicked. Nothing the call wrote is published: none of its outputs and none of its events. Its outputs fade over 64 frames from what readers saw at *n* − 1, exactly as for a fault found at *n* (§7), and are exactly +0.0 from *n* + 63 on; if *n* is the instance's first frame, they are 0 throughout. An `absorb` at frame *f* comes after that frame's `emit` was read: frame *f*'s outputs stand, the fade starts at *f* + 1, and the fault is logged at *f*.
- **Dependent outputs** are computed as always, in canonical order (§3, §4), from the muted instance's outputs: a faulted source counts as its fading value, then as 0. Events sent to it are discarded with reason `muted`.
- **The fault record** is a `fault` line (§9) with the frame *n*, the instance, its first output port (signal outputs before event outputs; none for an instance without outputs) and the reason `panic`. The policy is applied, and logged, the same way for every instance.
- **How long:** for the rest of the run. A swap that keeps the instance keeps its fault (§12, "What survives"); a swap that replaces it (a removal, or another type at the address) brings a new, unfaulted instance. There is no reset edit.
- **Threads and block size.** The result is identical for every thread count: a panic on a worker and the same panic on the calling thread give the same output and log. A call of one frame (every `emit` and `absorb`, every call in a cycle, every call at *k* = 1) is exact to its frame, as every other fault is. A `process` call of several frames gives no frame for its panic, so the fault acts from the call's first frame, which depends on how the run is split into blocks: the one exception to §13's partition rule. For an instance at rate divisor *d* > 1 (§16) a call counts in ticks: a call of one tick is exact to that tick however many frames its segment spans, and a call of several ticks acts from its first tick.
- **Real time.** Catching a panic allocates, locks and blocks nothing. The panic itself does (its payload, and the default hook's message on stderr): that is the cost of the broken contract, not of the recovery. A payload whose destructor panics again is caught as well (after eight such panics in a row the last payload is leaked rather than dropped), so nothing unwinds out of the engine.

### An engine-level failure

The engine itself fails when something unwinds in a task outside a primitive's call (engine code; in tests, the test hook), or when it finds one of its own invariants broken. The invariants are those of ADR 0020's disjoint access and of the task board: a view of an instance its task does not own, an instance's parts missing from the plan's buffers, a read of a row whose producer has not completed (or failed), or of the reader's own row, or outside the call's frames, a window outside its frames or the block, a conflicting claim (detected only in checked, debug, builds: release builds do not track claims), a ready task without a slot, a successor without a counter, a claim that finds its slot empty. A correct run never meets one; the goldens and the determinism matrix run with every check on.

- The task's completion is recorded as **failed** on the task board, and the job fails. A failed task is never marked done, so no other task reads its rows; a task claimed after the failure completes without running. After a failure the calling thread waits only for the claims already made: each claim is settled by exactly one completion (its task's, or a failed one the claim makes itself when it finds no task), and a task never pushed is never claimed, so the wait always ends.
- **The engine stops with an error.** The block publishes nothing, `Engine::run_block` returns 0 for it and for every later call, and `Engine::failure` says where and why (`frame`: the first frame of the segment; `reason`: `task_panic` or `invariant_violation`). It never continues silently:
  - `render` and its variants end with `RenderError::EngineFailed`; `signal render` and `signal replay` fail with that message, and write nothing.
  - The real-time runner keeps the frames of the blocks it ran before the failed one in the same callback (they completed and were valid), outputs silence from the failed block's first frame on, in that callback and every later one, and reports the failure in its status (`RunnerStatus::failure`); `signal play` stops and fails with the message, and Signal.app writes it to stderr and shows it.
- A failure on a worker and on the calling thread take the same path and give the same error. No worker is lost: the pool keeps every thread (`ScheduleStats::task_failures` counts the failed tasks).

## 15. Exact replay and ideal render

M2.7-4 (audit finding 2; ADR 0019 §3). Offline the engine waits for every due result of a latent module (§7, §13); in real time it never waits, and a result not in when due is a `late_result` fault, silence for that block. A live run's output therefore depends on when results arrived, which is not in its commands. Two operations on a session (`session-format.md` §6) take that into account:

- **Exact replay** reproduces the performance. It applies each recorded outcome (`session-format.md` §4a) at its due frame: a **recorded miss is enforced** without waiting (the engine asks the module's `due` once, as real time does, and calls `miss`, logging the same `late_result`), a **recorded delivery** shows the recorded data when the session has it (`inject_result`), else waits for the same device's result; before each `absorb` it lets the module **settle** (`settle`), so that work live submitted is never refused because the replay runs faster than the run did. The audio is stored as the device received it (§9, `device_conversion`). Its output and log are those of the live run, bit for bit, on the same machine and build (with recorded data, on any machine with the same build), for every *k* and thread count (for *k*, with §14's exception: a panic in a `process` call of several frames). A due block without a recorded outcome, or an outcome at no due block, is an error; so are incomplete records.

  **What "exact" guarantees, and how it is checked** (M2.7-8). An exact replay that succeeds has shown, at every delivered due block, the result the performance showed: the audio thread hashed every delivered result (FNV-1a 64 of its words, allocating nothing), the session records the hash beside the outcome, and the replay hashes the result it shows there, recorded data or recomputed, and compares. The first block that differs, by frame and then instance order, fails the replay (`ReplayMismatch { module, block }`, exit 2), whatever else then goes wrong; it names `--ideal` and `--record-boundary-data`. Recorded misses, the commands and every module without a declared latency are reproduced by construction (§11). So "exact" means: the same commands at the same frames, the same misses, and delivered results equal to the performance's as far as a 64-bit non-cryptographic hash tells (it detects accidental difference, not tampering); the output then follows bit for bit, on the recording's build. A latent module must expose its result to be hashed (`module-contract.md`); the engine refuses one that does not.

  Results recomputed rather than recorded are the performance's only on its build and machine (`same_machine`, ADR 0019 §3). An exact replay that must recompute any therefore first compares the session's build (revision, dirty flag, compiler, `Cargo.lock`) and machine (CPU architecture, every GPU adapter) with its own, and is refused before it starts on any difference, unless `--trust-provenance`, which the manifest then records (§9). That comparison is a pre-check, not the proof: two dirty builds at the same revision pass it, and the hashes still catch a result that differs. A session recorded before hashes replays as before, its results unverified, and says how many.
- **Ideal render** awaits every result: a fresh offline render of the same edits, as `signal render` would make of them. It is what the performance would have been on an infinitely fast device. Its manifest says `ideal`.

For a project without latent modules the two render the same audio and log, and nothing in its session or its plain replay changes; only an explicit `--ideal` adds the `ideal` label to the manifest. An exact replay renders at most the recorded frames; the frames after them were never played.

A session's boundary records depend on when results arrived in its run, which is not in its commands: two runs of the same commands may record different outcomes, and each replays exactly to its own run.

A frame is **due** for a latent instance when the frames it has run since it was prepared are a multiple of `block_frames` and at least *L* blocks (`module-contract.md`): the engine counts them, carried across swaps like every survivor's state, and the module counts its blocks the same way.

## 16. Rate divisors and vector ports

M2.8-6 (ADR 0005; ADR 0023 §5). A port runs at an integer **rate divisor** *d* of the master rate, and a port may carry a **vector** of *N* values. The oracle (`crates/signal-oracle`) implements this section independently of the engine, and the two match bit for bit (`testing-and-evaluation.md`).

### Rates and ticks

- A type's **rate** is the divisor its outputs, event ports and parameters share; each of its signal or vector inputs may declare a divisor that divides it or is a multiple of it. A type without one rate is refused when the engine is built, and so is a type at a rate above 1 with event ports, a declared latency or a sink: those run at divisor 1 only (`module-contract.md`, "Rates and vectors").
- An instance at rate *d* is **called** only on its **ticks**: the frames *n* with *n* mod *d* = 0, counted from frame 0 of the run. Which frames are ticks therefore depends neither on the block size nor on the partition nor on the thread count.
- It still **runs** every frame (§1): its inputs are summed (§3), its parameters' bases advance, take their events and give effective values frame by frame (§5, ramps counted in master frames), and suspension, faults and fades act frame by frame (§6, §7).
- **Outputs hold between ticks.** On a frame that is not a tick, each output is what the instance wrote at its last tick (0 before its first): what the fades, the fault guard and every reader see, as if it had written that value again. A state-output instance emits and absorbs at its ticks only. An instance that resumes, or is added by a swap, on a frame that is not a tick fades in from that held value; a survivor of a swap keeps it.
- **Per-tick calls.** In a call, `frames()` is the number of ticks the call covers, and its inputs, effective parameter values and outputs have one value per tick (`module-contract.md`). `PrepareContext` gives the master rate and the block capacity, as for divisor 1.
- **Panics.** A divided-rate `process` call covers the ticks of its segment, from the first; a panic acts from that first tick (§14). Before the call the engine keeps what the segment's gather will change, so a fault at a tick undoes what the frames after it gathered, exactly as one frame at a time.

### Vectors

- A port of shape `Vector(N)`, *N* ≥ 1, carries *N* `f64` per tick, as *N* rows (its elements, in order).
- A vector connection joins equal shapes only, element by element: element *e* of the target sums element *e* of each source, in canonical source order (§3), each times the connection's scale. A vector never connects to a signal or a parameter; parameters are scalar.
- Faults are found and faded per element, and name the vector port (§7); `audio` vector outputs fade per element (§6).

### Converters

- A signal or vector connection whose source divisor *s* and target divisor *t* differ, one a multiple of the other, between ports of the same shape (or a signal into a parameter), runs through a **converter**, inserted when the plan is built: a **hold** when *s* > *t*, a **decimate** when *s* < *t*. Every other connection across rates, or across shapes, is an error naming the connection as written; so is an event connection across rates.
- A converter is an **ordinary instance**: a direct primitive of type `core.<hold|decimate>.d<s>.d<t>.<signal|vectorN>.<kind>` (the source port's shape and kind), with input `in` at *s* and output `out` at *t*, which copies its input to its output at its own ticks. Its value at a tick of *t* is therefore the source's output at that frame: the source's held value for a hold, a sample of it for a decimate.
- It lives at `<source instance>/<port>_d<t>`, beneath the source primitive, where no other instance can be. One converter serves every connection from that source port to targets at *t*: the source feeds its `in` with scale 1, its `out` feeds each target with the connection's own scale, so its input is the source's output itself and it never faults.
- Its port takes part in its targets' canonical sums (§3) at its own address, which sorts where the source's ports sort among every other instance's.
- It is visible: in the plan, as an instance with its connections, and as the warning `<from> -> <to>: inserted converter <address> (<hold|decimate>, rate divisor <s> -> <t>)`, which `signal render` and the project loader report at the connection. A kind mismatch through it is reported from the source port.
- Swaps and live edits treat it as any instance (§12): it survives when it is in both plans with the same type; it is added, fading in if `audio`, when a cross-rate connection appears; it is removed when its last connection goes, suspended first by a live edit. Removing a converter's **source** at *t* > 1 can step at the swap: the converter shows its last tick, which may have read the source in the middle of its fade.
