# Execution semantics

**Normative.** This page says exactly what a Signal project computes, frame by frame, and what a render writes. It implements [ADR 0016](adr/0016-execution-contract.md) (the execution contract) and refines ADR 0002 (frame semantics), ADR 0004 (events and parameters) and ADR 0008 (reproducibility). If this page and ADR 0016 disagree, ADR 0016 wins and this page has a defect.

The reference implementation is the per-frame executor and the offline driver in `crates/signal-engine` (ADR 0012). Every later engine, including the real-time one, must produce the same results bit for bit.

Scope (M1): one frame per step (*k* = 1), rate divisor 1 for every port, one master rate per project. Plan swaps, incremental input and add/remove fades (M2-1) are in §12. Block execution (*k* > 1, D-EXEC-3) comes later.

## 1. Frames

- Time is the **frame**: an integer count since project start, at the project's master rate (its sample rate, in whole frames per second). Nothing in the engine reads a clock.
- **Every instance runs every frame**, connected or not, unless it is suspended (§6) or muted by the fault guard (§7). An instance from which no sink can be reached only causes a validation warning (ADR 0016 §1).
- Values are f64 (ADR 0007). f32 appears only in `audio.wav` (§8).
- M1 runs ports at rate divisor 1 only. A plan with any other divisor is refused when the engine is built.

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

After every call of an instance (`process` or `emit`), the engine checks what it produced, before anyone reads it:
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

## 8. The offline render

The offline driver renders *N* frames from frame 0:

- **The audio sink.** The plan must contain exactly one instance whose module type declares `sink = audio`. Zero or several is an error.
- **What is recorded.** Each frame, the driver records the sink's **signal inputs** as §3 sums them, one channel per signal input, in the order the module type declares them. This does not depend on whether the sink instance itself is suspended or muted (§10).
- **Sink gain.** If the sink's module type declares a parameter named `gain`, each recorded input is multiplied by that parameter's effective value (§4) in the same frame. While the sink is suspended or muted the executor does not recompute it, so the last effective value applies (§10, choice 11). `std.audio_out` records `gain·l` and `gain·r` this way.
- **`audio.wav`**: IEEE float, 32 bits, at the project sample rate, channels interleaved per frame. Each value is converted from f64 to f32 with round-to-nearest, ties to even.
- **Edits during a render.** `render_with` lets the caller commit a swap or push input before each frame (§12); `signal replay` renders a recorded session this way (`session-format.md`). Each frame's log is drained after it is recorded. A swapped-in plan must keep the audio sink: exactly one, at the same address, with as many signal inputs.

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
  - `event_overflow`.
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
10. **The "random *k*-partitions" clause of ADR 0016's ramp verification is covered at *k* = 1 only.** The test checks that every frame's base equals the closed form of (*i*, *N*), evaluated over random block partitions. Block execution itself, and so the block-partition clause, is not exercised until D-EXEC-3.
11. **The offline driver applies the sink's `gain` parameter** (§8, M1-6). A sink has no outputs, so it cannot expose gained values itself; the driver multiplies the recorded inputs by the sink's effective `gain`. A sink type without a `gain` parameter is recorded unchanged.

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

- A swap is prepared off the real-time thread (it builds the new plan's instances and buffers) and committed between frames, effective from the next frame *n*. Committing allocates nothing and moves state; the replaced plan is dropped elsewhere.
- The engine's **plan version** is 0 for the plan it was built with and goes up by one per swap. The swap record `{"type":"swap","frame":n,"version":v,"plan_version":p}` is the first record of frame *n*; `plan_version` is the swapped-in plan's own `version`, which the caller assigns.
- At most one swap per frame. A swap prepared by another engine or at another version, or already committed, is refused.
- The log holds one frame, and its records name the ports and instances of the plan that wrote it, so a frame's log must be drained before a swap is committed; a commit refuses an undrained log.

### What survives

An instance **survives** when the new plan has an instance at the same address with the same module type (and descriptor). A changed type at the same address is a removal plus an addition. A survivor keeps, moved and not copied:

- its primitive object: DSP state and random stream;
- its parameter bases, with running and pending ramps (§5, §6);
- its suspension, fade and fault state, and its discard counts;
- its outputs as readers last saw them, and the values its fades start from.

The new plan's parameter values do not apply to a survivor: its bases move. A caller that wants a new value sends a `set`.

### Connections

Connection changes between survivors take effect on the swap frame with no fade (crossfading changed connections is not done). This includes a disconnect that leaves a parameter with no signal sources: from the swap frame it uses its base (§5).

### Added instances

An added instance is created and prepared like any instance (its random stream is `RngStream::derive(seed, address)`) and runs from the swap frame. Its `audio` outputs fade in exactly as after a resume (§6): on its *j*-th frame the output is `live · ((j + 1) / F)` for *j* < *F* − 1, and exactly `live` from *j* = *F* − 1 on. Its other signal outputs and its event outputs start at once.

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

`Engine::drain_log` hands the last frame's records to the caller in the order of `log.jsonl` (§9) and empties the log, without allocating, so a real-time caller keeps the log bounded by draining it after every frame.

### Real-time runner

`signal_engine::Runner` runs the engine inside an audio callback of any size, fed by `signal_engine::Controller` from the control thread (M2-3; the mechanics are in `realtime-rules.md`, "The runner"). It changes nothing above: it is the same executor, driven by the same live-edit calls.

- **Order of operations.** Per callback: the queued commands are taken; then the frames run one at a time, a swap committed before the frame it is effective from, events and edits applied by the engine at their effective frame (§2, phase 1); after each frame the audio sink's channels are written and its log records sent; at the end, meters, parameter values (observation only) and the status.
- **Requested and effective frame.** Every command carries a **requested** frame: its stamp. Its **effective** frame follows the rules of "Pushed input and effective frames" above: the latest of the stamp, the next frame to run, and the effective frame of the previous command of its kind (event; suspend or resume); for a swap, one past the previous swap's frame. A command whose requested frame has already run is **late** and applies on the next frame. The runner reports both frames for every command, and counts late ones; `log.jsonl` records only the effective frame, so the log of a live run equals the log of its replay.
- **Latency.** A command stamped by default is requested for the current frame (as the runner last reported it) plus the latency, two device buffers unless configured otherwise. A command arriving within the latency applies exactly at its requested frame, whatever the callback size, unless an earlier command of its kind raised it (its effective frame is never before the previous one's); only a command whose requested frame has already run is late.
- **Output.** Each frame, every audio-sink channel is the value the offline driver records (§8: the sink's input sum times its `gain`), clamped to [−1, 1] and converted to f32 (round to nearest, ties to even). Where no sample is clamped, the output equals the offline render of the same frames and commands bit for bit as f32 (V9), for any sequence of callback sizes (*k*-partition invariance, ADR 0002). Clamped samples are counted. `audio.wav` is not clamped (§8).
