# 01 — Project Inception Proposal: Signal

| | |
|---|---|
| Status | **Approved by PO, 2026-10-11** (v2; supersedes v1). D18/D19 accepted with the M0 go-ahead. |
| Date | 2026-10-11 |
| Depends on | `00-vision.md` (draft v4) |
| Already approved | D-EXEC-1 (frame-functional semantics), D-EXEC-2 (k-frame module contract), Q15b (primitives compiled into the binary), Q19 (polyphony belongs to modules), Q18 (always seeded) |
| Later refinements | ADR 0016 (accepted) specifies scheduling (every non-suspended instance runs; reachability is diagnostic only), feedback execution, and same-frame event ordering. Where this snapshot says "pull from outputs", ADR 0016 governs. |
| Terminology | Uses the **provisional** terms of vision §3a (Project, Module, Primitive, Composite, Instance, Port, Parameter, Library). Renaming is cheap until M0 ends. |

**Evidence labels:**
- **[Fact]**: established or verifiable.
- **[Researched]**: crates.io/GitHub API data gathered 2026-10-11. Items marked UNVERIFIED there stay flagged.
- **[Rec]**: recommendation.
- **[Assumption]**: unconfirmed.
- **[Spec]**: speculative future capability.
- **[Approved]**: decided by the PO.

---

## 1. Executive technical summary

Signal is a **platform for developing and interconnecting real-time signal modules**. Applications are **projects**: folders of text files that instantiate, nest, and connect modules. Signal itself contains no music, measurement, or lighting logic. Those live in module libraries.

The design rests on five ideas:

1. **Frame-functional semantics [Approved].** A logical master clock (default 96 kHz) defines frames. In every frame each module instance computes `(inputs[n], state[n−1]) → (outputs[n], state[n])`. Feedback loops must contain at least a one-frame delay, and every instance advances every frame. The meaning of a project is therefore independent of buffer sizes, devices, and execution strategy.
2. **Your execution model first, measured before it is optimised.** When the graph changes, the engine pulls back from the outputs to work out what to compute and in what order. It then executes one frame at a time. The module contract already allows processing *k* frames per call [Approved], so block execution can be added later as a verified optimisation if benchmarks call for it (D-EXEC-3, deferred).
3. **Everything is a module.**
   - **Primitives** are Rust code compiled into Signal [Approved].
   - **Composites** are text: modules made of modules, nested without limit, flattened into one execution plan at load time.
   - **Parameters are input ports with defaults**, so anything can modulate anything.
   - **Polyphony is a module concern** [Approved].
4. **Live mutation via versioned graphs.** Loading, unloading, and reconnecting instances while running works like this:
   - A new execution plan is prepared off the audio thread and swapped in at a frame boundary.
   - Surviving instances keep their state.
   - Every change is logged with the frame where it actually took effect.

   A live session therefore replays bit-exactly offline.
5. **Reproducibility by construction.** Results are determined by:
   - an integer frame clock;
   - seeded RNG streams per instance (always seeded [Approved]);
   - logged external inputs and edits;
   - a vendored RNG;
   - pure-Rust math.

   Results are bit-exact on the same build and hardware class.

The **offline renderer is the reference**. Real time is a driver over the same engine. An **evaluation system** (manifests, metrics, comparisons, listening packages) makes the development loop runnable by AI agents, with human verdicts as the gate.

The core stays **domain-free**. Tempo, beats, MIDI, DMX, and video are module-level concepts, so a measurement project and a techno project use the same engine unchanged.

---

## 2. Engineering goals and non-goals

**Goals**
- E1. **Frame-exact semantics.** Output depends only on the definitions, the seed, logged inputs and edits, and the sample rate. It never depends on callback buffer sizes or wall-clock timing.
- E2. **Bit-exact reproducibility** on the same build and hardware class, including replay of live sessions with their edits.
- E3. **Real-time safety enforced by tooling.** No allocation, locks, blocking syscalls, or unbounded work on the audio thread.
- E4. **Live load, unload, and reconnect** without dropouts.
- E5. **Domain-free core.** No musical, MIDI, or lighting concepts in the engine.
- E6. **Agent-operable development loop.** Every tool supports machine-readable output, there is a module catalogue, and the CLI can render, compare, and run contract tests.
- E7. **Fault containment.** A faulty module is detected, muted, and reported. It does not silently corrupt the session.
- E8. **Rate-agnostic.** Default 96 kHz; tested at 44.1, 48, 96, and 192 kHz.

**Non-goals (initially)**
- Block execution: deferred until benchmarks show a need.
- Multithreaded graph execution, SIMD frameworks, GPU.
- Runtime-loaded code packages: primitives are compiled in [Approved].
- Scripting languages or a custom DSL.
- GUI and visual editing.
- Plugin formats.
- Cross-platform support.
- External clock sync.

---

## 3. Requirements and unresolved questions

### 3.1 Functional requirements
| ID | Requirement | Milestone |
|---|---|---|
| F1 | `signal open <project>` loads a project folder. The project and composite files are validated against a schema, with file/line errors. | M1 |
| F2 | Primitives self-describe their ports, parameters, defaults, units, and docs. `signal modules --json` exports the catalogue, which project validation and schema generation use. | M1 |
| F3 | Composites are defined in text, nested arbitrarily, and expose chosen ports and parameters. Instances have hierarchical addresses (`/drums/kick/env.decay`). | M1 |
| F4 | Typed ports with a rate. Initial types: f64 signal and event stream. Feedback without a delay is rejected at load time. | M1 |
| F5 | Offline render of a project for N frames, faster than real time, to WAV. Each render writes a manifest, an event/edit log, and metrics. | M1 |
| F6 | Real-time execution on CoreAudio at the project's master rate. | M2 |
| F7 | Live graph edits while running: add or remove instances, connect or disconnect, set parameters. Each edit is logged with its effective frame. | M2 |
| F8 | Offline replay of a recorded live session, bit-exactly. | M2 |
| F9 | Compare two renders: metric deltas plus a loudness-matched listening package. | M3 |
| F10 | Audio input, recorded during live runs and replayed offline. | M4 |
| F11 | Lower-rate ports (integer divisors of the master clock) and vector ports (e.g. spectra, DMX universes). | M4 |
| F12 | Deterministic asynchronous helpers for non-real-time work: sample loading, heavy precomputation, long-range planning. A request at frame *n* is applied at a declared frame. | M5 |
| F13 | MIDI/OSC input modules and network output modules (e.g. Art-Net), all inputs logged. | M6 |

### 3.2 Nonfunctional requirements
| ID | Requirement | Target (proposed; tuned in M1/M2) |
|---|---|---|
| N1 | Determinism | Bit-exact across runs, callback sizes, and real time vs offline on the same build |
| N2 | Timing | 0-frame error for events and edits relative to their logged frames |
| N3 | Latency | ≤ 128-frame device buffer at 96 kHz (1.33 ms) for the slice project, without xruns |
| N4 | CPU | Slice project worst-case callback < 25 % of the buffer period |
| N5 | Stability | ≥ 30 min real time, 0 xruns, 0 audio-thread allocations, including during live edits |
| N6 | Floors | 0 NaN/Inf; peak ≤ ceiling; \|DC\| < 1e-4; per-module aliasing thresholds for nonlinear primitives |
| N7 | Offline speed | ≥ 10× real time at 96 kHz for the slice project. Feeds the D-EXEC-3 decision. |
| N8 | Agent ergonomics | Every crate and primitive has a contract and a test command; all CLI output is available as JSON |

### 3.3 Domain objectives (guide design; not core features)
- **Music:** sample-accurate sequencing, polymeter, microtiming, generative processes with memory, long-form performances.
- **Measurement:** calibrated I/O alignment, analysis accuracy.
- **Single instruments:** heavy per-frame DSP, physics, 3D models.
- **Audiovisual:** multi-rate, visual output.
- **Lighting:** network protocols, pattern detection.

### 3.4 Experimental objectives
The module-development loop (vision W1): propose → implement in `lab/` → contract tests → render → metrics → compare against a baseline → human verdict → promote or reject.

### 3.5 Long-term ambitions [Spec]
- Musical memory models.
- Spectral, granular, and physical modelling libraries.
- Visual output modules.
- External sync (Link, MIDI clock, PTP).
- GUI and visualisation.
- Plugin export.
- Runtime-loaded module packages (reconsider only if Q15b changes).

### 3.6 Unresolved technical questions
| ID | Question | Proposed handling |
|---|---|---|
| T1 | What happens to sound when an instance is removed or added (clicks)? | D16: engine-applied deterministic fade, default 5 ms, overridable per module |
| T2 | How live edits are issued | D17: watch the project files and diff them into edit commands (depends on Q6) |
| T3 | Non-integer rate ratios (e.g. DMX at ~44 Hz on a 96 kHz clock) | M4: integer divisors only; the nearest divisor is accepted for loosely timed domains |
| T4 | Secondary device clocks (second interface, display, network) | M6 ADR: clock-domain adapter modules |
| T5 | Audio→control feedback in generative modules | Covered by D-EXEC-1: feedback is ordinary graph feedback through a delay, so it stays deterministic |
| T6 | Real time misses the frame scheduled for an edit (preparation too slow) | Apply it at the next possible frame and log the actual frame. Replay stays exact because it uses the log. |
| T7 | Per-frame overhead for large projects | M4 benchmark → D-EXEC-3 |

**Open product questions** (vision §13):
- Q2 licence: needed before M0 adds dependencies.
- Q3 users.
- Q5 live vs offline priority.
- Q6 live-control style: affects D17.
- Q7 external sync.
- Q9 channel counts.
- Q10 references.
- Q11 verdict format.
- Q13 time.
- Q16 visual output.
- Q17 measurement accuracy.
- Q20 terminology.

---

## 4. Architecture alternatives and tradeoffs

This revision compares alternatives on the platform question. The execution semantics are already approved.

### A. Monolithic application with domain features built in
One program with built-in sequencer, mixer, analyser, and so on, extended by adding features.
- Pros: fastest route to one application.
- Cons: contradicts platform-first and the domain-free core. Every new domain touches the core.

### B. Own frame-functional runtime with primitive and composite modules *(recommended)*
A small domain-free engine. Rust primitives are compiled in; text composites are flattened into an execution plan. Live edits are versioned plan swaps, and the offline renderer is the reference.
- Pros:
  - matches the approved semantics exactly;
  - the core is small and testable;
  - domains are libraries;
  - agents work on isolated primitives against one contract;
  - feedback is natural in a per-frame engine.
- Cons: we own the graph compiler, plan swapping, and fault handling. Per-frame performance is unproven at scale (R13).

### C. Build on an existing engine (fundsp `Net`, Firewheel, `web-audio-api`)
[Researched] All three are active, MIT/Apache, and have sample-accurate scheduling of some kind.
- fundsp has a combinator DSL and a real-time-safe split.
- Firewheel is a DAG engine being upstreamed into Bevy and self-described as "not a DAW engine".
- `web-audio-api` has an `OfflineAudioContext`.
- Cons: none of them has frame-functional semantics with sample-delay feedback across arbitrary graphs, hierarchical text composites, or edit-logged replay. Retrofitting these would fight the dependency.
- Role instead: reference implementations and test oracles for individual DSP algorithms.

### D. Process split: audio server plus control client (SuperCollider model)
- Pros: strong fault isolation.
- Cons: two-process lockstep is needed for offline determinism; more moving parts.
- Not needed now that primitives are compiled in and reviewed. The command/edit log is serializable, so a process split remains possible (R7).

### E. Language-centric (DSL, JIT, or codegen)
Projects are written in a purpose-built language.
- Cons: maximum language-design risk; agents are weakest in new languages.
- Possible later [Spec]: codegen of composites into fused Rust as a performance path behind D-EXEC-3.

### Comparison
Scale: ●●● good · ●● moderate · ● poor.

| Criterion | A Monolith | **B Own runtime** | C Existing engine | D Process split | E Language |
|---|---|---|---|---|---|
| Fit to approved semantics | ● | ●●● | ● | ●● | ●●● |
| Multi-domain extensibility | ● | ●●● | ●● | ●●● | ●● |
| Live load/unload | ● | ●●● | ●● | ●●● | ●● |
| AI-assisted development | ●● | ●●● | ●● | ●● | ● |
| Determinism and replay | ●● | ●●● | ● | ● | ●● |
| Initial complexity | ●●● | ●● | ●●● | ● | ● |
| Long-term maintainability | ● | ●●● | ●● | ●● | ● |
| Raw performance (initially) | ●●● | ●● (per-frame) | ●●● | ●● | ●●● |

---

## 5. Recommended architecture

### 5.1 Layering

```
 project folder (text)                       primitives (Rust, compiled into Signal)
  project.toml, composites/*.toml              self-describing: ports, params, docs
          │                                              │
          ▼                                              ▼
 signal-project: parse → validate (schema from catalogue) → project model
          │
          ▼
 signal-graph: resolve composites → flatten hierarchy → check cycles have delays
               → pull from outputs → execution order → Plan (versioned)
          │                                   ▲
          │                     edits (add/remove/connect/set) ── from file-watch diff,
          ▼                                   │                    CLI, later MIDI/OSC
 signal-engine: per-frame executor ── plan swap at frame boundary ── fault guards
          ├── Offline driver (reference): runs N frames, writes outputs
          └── Realtime driver: CoreAudio callback runs frames; control thread
              prepares plans/instances; lock-free queues both ways
          │
          ▼
 render artifacts: audio + manifest + event/edit/input log + metrics ──► signal-eval
```

### 5.2 Abstractions to design carefully now
These are expensive to change later:

1. **Frame clock.** `Frame(u64)` from project start is the only time the core knows. Musical time (tempo, beats, rational positions, polymeter) is supplied by **transport/clock primitives** in a music library. These emit events and signals, so the core stays domain-free (C6).
2. **Module contract [D-EXEC-2 approved].**
   - **`describe()`**: static ports, parameters, and documentation, used for the catalogue.
   - **`prepare(ctx)`**: the only place allowed to allocate. `ctx` gives the sample rate, the maximum *k*, and an RNG stream.
   - **`process(k, inputs, outputs)`**: processes *k* frames. Event inputs carry frame offsets within the *k* frames. The result must be identical for any partition of *k*.
   - **`reset()`**.
   - **State transfer** for graph swaps: the instance object itself moves into the new plan.
3. **Ports and values.** A port is `(type, rate divisor)`.
   - Initial types: **f64 signal** (one value per tick) and **event stream** (zero or more small messages per tick).
   - Planned: fixed-size vectors (spectra, DMX universes) and opaque handles (buffers from async helpers).
   - Rates are integer divisors of the master clock. Ports also carry a **kind, unit, and range** (vision §9a).
   - Conversion is always done by a module instance, never hidden inside the engine. The graph compiler **inserts** implicit and default converters when the choice is unambiguous and shows them in the plan (D18).
4. **Parameters are input ports with defaults.** Changing a parameter from outside means sending set or ramp events to that port. Modulating it means connecting a signal. Reaktor-style knobs become ordinary modules.
5. **Composites and addressing.**
   - A composite contains instances and connections and exports ports and parameters.
   - Flattening keeps hierarchical addresses, which are used for logging, parameter access, and RNG stream derivation.
   - Each instance's RNG stream is `hash(master seed, address)`. Adding an instance therefore never changes another instance's randomness.
6. **Graph versions and edits.** An edit is a command stamped with a requested frame. The control thread builds the new plan and calls `prepare` on new instances; the audio thread swaps plans at the first frame ≥ the requested frame once the plan is ready. The **effective frame is logged**, and old plans are freed off the audio thread. Edits are the same mechanism whether they come from a file diff, the CLI, or (later) a controller.
7. **Reproducibility record.** The manifest records:
   - Signal revision and toolchain;
   - hashes of the project files and the Cargo.lock;
   - the seed and the sample rate;
   - logs of events, edits, and inputs with frames;
   - the build profile.
8. **Core vs lab.** `lab/` primitives use the same contract and contract tests. They compile into Signal only with `--features lab`. Promotion to the standard library requires passing contract tests, benchmarks, and a recorded verdict.

### 5.3 Kept deliberately simple
- One master rate per project.
- Per-frame, single-threaded execution.
- Two port types initially (f64 signal, event stream).
- Integer rate divisors only.
- Stereo or fixed-channel device I/O.
- Composites are text with no logic: no conditionals or loops in definitions.
- GUI deferred until its milestone (Q23). Live edits come from file changes and CLI commands. The engine's edit-command interface is the same one the GUI will use later.

### 5.4 Fault containment
- After each instance's `process`, a cheap check for non-finite or absurd output. On failure: fade out, mute the instance, and report through a lock-free queue. The project keeps running.
- Per-callback timing is measured, and the worst case and overruns are reported.
- A counting global allocator detects allocations on the audio thread in debug and test builds.
- Clippy lints forbid panicking constructs in `signal-engine` and the standard library. Lab primitives receive the same tests. A real process sandbox is deferred (R7).

---

## 6. Suggested technology stack

All versions are [Researched] as of 2026-10-11 and pinned via Cargo.lock and `rust-toolchain.toml`.

| Concern | Recommendation | Notes / alternatives |
|---|---|---|
| Language | Rust stable, edition 2024 | [PO] |
| Numeric type | **f64 for all signal values and state**; f32 only at device and file boundaries | Per-frame scalar execution removes f32's SIMD advantage; f64 suits measurement and is simpler for agents. D6. |
| Audio I/O | **cpal 0.18** behind a thin driver trait | Active, Apache-2.0, CoreAudio via objc2. Unreleased changes add xrun reporting and breaking renames, so pin. `objc2-audio-toolbox` only for gaps (UNVERIFIED availability of AudioWorkgroup and buffer-size APIs). |
| Control↔audio queues | **rtrb 0.4** (wait-free SPSC) | ringbuf 0.5 as alternative |
| Deferred free / plan swap | Own small module (return queue to the control thread); `basedrop` and `arc-swap` as references | |
| Allocation detection | Own counting `#[global_allocator]` (~40 lines) | `assert_no_alloc` has been unmaintained since 2023 |
| Denormals | Per-module flushing conventions plus optional FTZ from one audited module, the same offline and live (D9) | `no_denormals` flags FP-control changes as UB in Rust |
| Math | `libm` (pure Rust) for transcendental functions in primitives | Independent of the platform libm. Benchmark the cost. |
| WAV | **hound 3.5** | |
| Analysis | **rustfft 6.4 / realfft 3.5**, **ebur128 0.1.10** | Onset and spectral-flux code is our own (no maintained crate found) |
| Definitions | **serde + toml 1.1 (+ toml_edit) + schemars 1.2** | RON 0.12 as alternative; avoid serde_yaml (deprecated) and knuffel (dormant) |
| Machine output | serde_json; logs as JSONL | |
| File watching (live edits) | `notify` crate (not researched — verify in M0) | Only if D17 is approved |
| RNG | Vendored SplitMix64 + Xoshiro256++ with golden vectors | `rand` 0.10 allows value-breaking minor releases and marks StdRng/SmallRng non-portable [Researched] |
| Testing | **cargo-nextest, proptest, insta, criterion 0.8**, approx 0.5 | |
| CLI | `clap` (not researched — verify in M0) | |
| MIDI / OSC (M6) | `midir` 0.11 + `wmidi` 4; `rosc` 0.11 | |
| Deferred | rhai or wasmtime (scripting), clack (CLAP), egui (GUI) | upstream nih-plug is in maintenance mode |

---

## 7. Repository and module structure

The prefix is `signal-`. Crates are created only when their milestone needs them.

```
/
├─ AGENTS.md                 agent entry point: commands, rules, where contracts live
├─ Cargo.toml                workspace; rust-toolchain.toml pins the toolchain
├─ docs/
│  ├─ 00-vision.md, 01-inception-proposal.md
│  ├─ architecture.md               (M0)
│  ├─ realtime-rules.md             (M0: audio-thread rules + review checklist)
│  ├─ semantics.md                  (M1: normative frame, port, event, edit semantics)
│  ├─ module-contract.md            (M1: how to write a primitive; the contract-test suite)
│  ├─ project-format.md             (M1: project and composite files; schema link)
│  ├─ dsp-conventions.md            (M1: units, ranges, antialiasing, denormals)
│  ├─ testing-and-evaluation.md     (M1)
│  ├─ roadmap.md                    (M0)
│  └─ adr/NNNN-*.md                 (one per approved decision)
├─ crates/
│  ├─ signal-core      Frame, ports/values, events, addresses, RNG, module trait, manifest types
│  ├─ signal-graph     composites, flattening, validation, cycle/delay check, plan compiler, edits
│  ├─ signal-engine    per-frame executor, plan swap, fault guards, offline + realtime drivers
│  ├─ signal-project   project folder loading, TOML model, schema export
│  ├─ signal-std       standard primitive library (osc, env, math, delay, shaper, transport, …)
│  ├─ signal-eval      analysis, metrics, compare, listening packages
│  └─ signal-cli       `signal open | render | replay | modules | schema | compare`
├─ lab/                      experimental primitives (feature `lab`)
├─ projects/                 test projects (fixtures), e.g. projects/kick/
├─ experiments/              per-experiment folders: hypothesis, renders, report, verdict (M3)
└─ tests/golden/             golden manifests and audio hashes (audio regenerated, not committed)
```

`signal-std` is the first library. Domain libraries such as `signal-music` and `signal-measure` split out when they grow.

---

## 8. First vertical-slice specification

### 8.1 Test project `projects/kick/` (a fixture, not a product)
The project exercises every platform mechanism with the smallest musical content that can be judged by ear.

| Platform mechanism | How the project exercises it |
|---|---|
| Primitives | `sine_osc` (f64 phase), `exp_env` (event-triggered), `mul`, `add`, `tanh_shaper` (antialiased: ADAA **or** oversampling; the other becomes the first lab experiment), `dc_block`, `unit_delay` (z⁻¹), `lfo`, `transport` (tempo → beat events at exact rational positions; seeded probability; per-event microtiming), `audio_out` |
| Composite | `kick` defined in text from `sine_osc` + two `exp_env` + `tanh_shaper` + `dc_block`, exporting `trigger`, `decay`, and `drive` |
| Nesting | `drums` composite contains `kick`, so addresses look like `/drums/kick/amp_env` |
| Parameters as ports | `lfo` connected to `/drums/kick.decay` |
| Feedback | a small feedback-FM path through `unit_delay` inside `kick` (or a separate tone). A loop without the delay must be rejected. |
| Determinism | seeded ghost triggers from `transport` |
| Live edits (M2) | while running, add and later remove a second instance (e.g. a `noise` primitive through `exp_env`), reconnect the LFO, and change tempo |

### 8.2 Illustrative files (syntax not final; D7)
```toml
# projects/kick/project.toml
[project]
name = "kick"
sample_rate = 96000
seed = 42

[instances]
clock  = { module = "std.transport", bpm = 132, ghost_probability = 0.15, ghost_offset_ms = 6.0 }
drums  = { module = "drums" }                      # composite from composites/drums.toml
lfo    = { module = "std.lfo", shape = "sine", period_s = 29.1 }
out    = { module = "std.audio_out", channels = 2 }

[wiring]
connections = [
  { from = "clock.beat", to = "drums.trigger" },
  { from = "lfo.out",    to = "drums.kick_decay", scale = 120.0 },
  { from = "drums.out",  to = "out.in" },
]
```

### 8.3 Commands (all support `--json`)
- `signal render projects/kick --seconds 120 --out renders/r1/` writes `audio.wav`, `manifest.json`, `log.jsonl`, and `metrics.json`.
- `signal open projects/kick` runs in real time. With `--watch`, saving a project file applies the diff as live edits (D17). The session log is written on exit.
- `signal replay renders/live-session/` renders a recorded live session offline.
- `signal modules --json` exports the primitive catalogue. `signal schema` exports the project JSON Schema.

### 8.4 Acceptance (automated unless marked)
| ID | Check | M |
|---|---|---|
| V1 | Same project and seed → identical audio hash across runs | M1 |
| V2 | Every primitive passes the contract suite: identical output for random *k*-partitions, deterministic `reset`, finite output under fuzzed inputs, no allocation in `process` | M1 |
| V3 | Every event takes effect on its exact frame. Each kick onset in the audio is on the trigger frame. | M1 |
| V4 | A different seed changes only the ghost triggers | M1 |
| V5 | Floors: no NaN/Inf, peak ≤ −0.3 dBFS, \|DC\| < 1e-4 | M1 |
| V6 | Shaper aliasing below the threshold set in M1 (initial target: inharmonic content ≥ 60 dB below the fundamental for 1–8 kHz test tones) | M1 |
| V7 | A feedback loop without a delay is rejected with a clear error; with `unit_delay` it renders | M1 |
| V8 | The project renders at 44.1, 48, 96, and 192 kHz. The kick's pitch envelope is correct in Hz at each rate. | M1 |
| V9 | Real-time driver against a null sink with randomised callback sizes → bit-identical to offline | M2 |
| V10 | A recorded live session with edits replays offline bit-identically | M2 |
| V11 | Zero audio-thread allocations, including during edits | M2 |
| V12 *(manual)* | 30 min live at 96 kHz with a 128-frame buffer and periodic edits: 0 xruns; worst-case callback < 25 % of the period | M2 |
| V13 | Benchmark: per-frame cost per instance; maximum instances at 96 kHz within budget. Recorded as input to D-EXEC-3. | M2 |
| V14 *(human)* | PO judges the kick usable; verdict recorded in the repo | M1 |

---

## 9. Testing and validation approach

| Layer | What | Tooling |
|---|---|---|
| 1. Unit | DSP math against analytic results; rational time → frame conversion in `transport` | nextest, approx |
| 2. **Contract suite** | Run generically on every primitive, standard and lab: *k*-partition invariance, reset determinism, fuzzed inputs give finite output, no allocation, rate-agnostic behaviour, denormal behaviour | proptest + counting allocator |
| 3. Graph | Flattening, addressing, cycle/delay detection, plan diffing, state transfer across swaps | nextest, insta |
| 4. Engine | Event and edit frame exactness, real time vs offline equality (null driver), session replay | nextest |
| 5. Golden renders | Manifests and audio hashes for test projects. An intentional sound change needs a reviewed note and a verdict. | insta |
| 6. Metrics | LUFS, true peak, DC, spectral centroid/flux over time, onsets, CPU per callback | signal-eval |
| 7. Performance | ns/frame per primitive; instances-per-budget; offline speed factor; regression thresholds | criterion |
| 8. Comparison | `signal compare A B`: metric deltas plus loudness-matched, randomised A/B excerpts | signal-eval |
| 9. Human verdict | `experiments/<id>/verdict.md` (format per Q11) | manual |
| 10. Real-time soak | Long runs with scripted edits, logging xruns and timing | local, manual or scheduled |

**Principle:** metrics detect defects and describe changes. Humans judge quality.

**CI** [Assumption]: hosted macOS arm64 runners for layers 1–8; real-time layers run locally. To be chosen in M0 once the repo is under git.

---

## 10. Milestone roadmap

> The living roadmap is `docs/roadmap.md`. It adds **M2.5 Signal.app shell** (interface setup dialog + metering, per Q23). The table below is the inception snapshot.

Milestones are quality-gated, not date-gated. Each one ends with something audible or measurable.

| M | Deliverable | Depends | Automated acceptance | Exit condition |
|---|---|---|---|---|
| **M0 Foundations** | Vision approved; ADRs for §12; git repo; workspace; `rust-toolchain.toml`; AGENTS.md; `realtime-rules.md`; CI (fmt, clippy, nextest) | — | CI green | Decisions approved or explicitly deferred |
| **M1 Core + offline** | signal-core, -graph, -project, -std (slice primitives), -engine (offline driver), -cli `render/modules/schema`, contract suite, `projects/kick` | M0 | V1–V8, golden render | V14 listening verdict recorded |
| **M2 Real time + live edits** | CoreAudio driver, plan swap, edit commands, `--watch` file diff, session log and `replay`, fault guards, benchmarks | M1 | V9–V11, V13 | V12 soak passed. **Vertical slice complete.** |
| **M3 Evaluation loop** | `signal compare`, listening packages, `experiments/` workflow; first lab experiment (ADAA vs oversampling shaper), executed by an agent | M1 | An agent runs the full W1 loop from the CLI; report validates | PO confirms the package was decision-useful |
| **M4 Second domain: analysis** | Audio input (recorded and replayed), rate divisors, vector ports, FFT analyser primitive. Test project `projects/tone-analyser` (generator → device out → device in → spectrum and latency report). D-EXEC-3 decided from benchmarks. | M2 | Input replay bit-exact; analyser matches analytic test signals; loopback latency measured | PO judges the measurements credible |
| **M5 Stateful and generative** | Async helpers with deterministic apply frames; sampler primitive (via an async loader); stateful sequencing, hierarchical musical time in `transport`; test project with a ≥ 20 min generative performance | M2 | 20-min render deterministic; async results applied at declared frames | Long-form listening review: development, recurrence, contrast audible |
| **M6 External I/O** | MIDI/OSC input modules (logged); Art-Net output module; clock-domain adapter ADR and first adapter; test project: audio-reactive DMX | M4 | Logged-input replay identical; Art-Net packet timing within spec | PO runs a live session with external control and replays it |
| Later [Spec] | Visual output, GUI, external sync, plugin export, scripting evaluation, codegen | — | — | — |

---

## 11. Technical risks and mitigations

| ID | Risk | L / I | Mitigation |
|---|---|---|---|
| R1 | Real-time safety violations creep in as agents add primitives | High / High | Contract suite on every primitive; counting allocator; lints; `realtime-rules.md`; small audio path |
| R2 | Frame-exactness breaks subtly (rounding in `transport`, edit timing, *k*-dependent state) | Med / High | Normative `semantics.md`; exact rational conversion; partition-invariance property tests; offline as reference |
| R3 | Feedback becomes hard again if block execution arrives (D-EXEC-3) | Med / Med | The block engine must match the per-frame oracle; feedback strongly-connected components stay per-frame |
| R4 | Live edits glitch (clicks, preparation latency) or break replay | Med / High | Deterministic fades (D16); effective frames logged; plans prepared off-thread; soak tests with scripted edits |
| R5 | Cross-build numerical drift breaks golden files | Med / Med | Pinned toolchain; pure-Rust `libm`; goldens tied to the toolchain; cross-build comparison with a tolerance |
| R6 | Denormal CPU spikes | Med / Med | D9 conventions plus a contract-suite check |
| R7 | Faulty lab primitive crashes the process | Med / High live | Lab gated by feature flag and the same contract suite; fault guards. The serializable command log keeps a future process split open. |
| R8 | Project-format churn breaks existing projects | Med / Med | Schema version in every file; typed model separate from syntax; migration tool when the version bumps |
| R9 | Platform over-engineering before anything is audible | Med / High | Test-project-driven capabilities; listening or inspection gate per milestone; §5.3 list |
| R10 | "Technically valid, musically useless" output | High / High | Human verdicts at every gate; metrics framed as defect detectors |
| R11 | Dependency churn (cpal renames, rand value changes) | Med / Low | Thin driver trait; vendored RNG; pinned lockfile |
| R12 | Generative intelligence underspecified | High / Med | Research from M5; reproducible experiments make progress cumulative |
| **R13** | **Per-frame execution too slow for large projects** (dispatch per instance per frame) | Med / Med | Benchmarks from M2; the D-EXEC-2 contract already allows blocks; codegen as a later option |
| **R14** | **Compiled-in primitives make Signal's build time and binary grow** | Med / Low | Library crates behind feature flags; incremental builds; check in M5 |
| **R15** | **Domain leakage into the core** (tempo, MIDI, DMX concepts creeping into signal-core) | Med / High | ADR rule plus review checklist: the core knows frames, ports, events, and addresses only |
| **R16** | **Automatic conversion becomes "magic"**: surprising inserted converters, ambiguous paths, hidden latency | Med / Med | Insert only on a unique shortest path; inserted converters visible in the plan, logs, and GUI; `signal plan --explain`; ambiguity is an error with suggestions |
| **R17** | **GUI and text diverge**: GUI-only state, or comments lost when GUI edits are written back | Med / High | GUI is a client of the edit-command interface only; write-back via toml_edit; round-trip tests |

---

## 12. Decisions requiring approval

Each approved item becomes an ADR.

| ID | Decision | Recommendation | Main alternative | Reversibility |
|---|---|---|---|---|
| D-EXEC-1 | Frame-functional semantics | **Approved** | — | — |
| D-EXEC-2 | *k*-frame module contract | **Approved** | — | — |
| D-EXEC-3 | Engine scheduling | **Deferred**: per-frame first; decided by M2/M4 benchmarks | — | Easy |
| D1 | Architecture B: own domain-free runtime with primitives and composites | **Yes** | C (build on Firewheel/fundsp) | Hard |
| D2 | Offline-first: slice = M1 (offline) + M2 (real time and live edits) | **Yes** | Both drivers in one milestone | Easy |
| D3 | The core knows only `Frame(u64)`; musical time lives in `transport` primitives (exact rationals) | **Yes** | Tempo/beat clock in the core | Hard |
| D4 | Events are generic messages on event ports; MIDI only via adapter modules | **Yes** | MIDI-like notes in the core | Hard |
| D5 | Composites flattened into versioned plans; cycles need an explicit delay; edits swap plans at frame boundaries with effective frames logged | **Yes** | In-place mutation of the running graph | Hard |
| D6 | **f64** signal values and state; f32 only at boundaries | **Yes** (changed from v1) | f32 buffers, f64 state | Medium |
| D7 | TOML project and composite files; serde model + JSON Schema generated from the primitive catalogue | **TOML** | RON, KDL | Medium |
| D8 | Algorithms are compiled Rust; no scripting or DSL before evidence | **Yes** | Rhai from M1 | Easy to add later |
| D9 | Denormal conventions + optional audited FTZ, the same in both modes | **Yes** | FTZ only | Easy |
| D10 | Bit-exact on the same build and hardware class; all external influences logged with effective frames | **Yes** (per Q12/Q18) | Bit-exact across builds | Hard |
| D11 | Workspace layout (§7) with `lab/` behind a feature flag and promotion criteria | **Yes** | Single crate | Easy |
| D12 | cpal behind a driver trait | **Yes** | Direct objc2 CoreAudio | Easy |
| D13 | Vendored RNG; per-instance streams from `hash(seed, address)` | **Yes** | Pinned `rand_xoshiro` | Easy |
| D14 | Parameters are input ports with defaults | **Yes** | Separate parameter system | Hard |
| D15 | Ports = (shape, integer rate divisor, kind, unit/range); start with f64 signal and event stream | **Approved; amended in v2.1** (kind/unit added) | Single audio-rate type everywhere | Medium |
| D18 | **Conversion model (vision §9a):** implicit, default, and explicit conversion levels; converters are ordinary modules that declare A→B; the compiler inserts a converter only when exactly one shortest path exists, visibly in the plan; otherwise validation fails and suggests converters | **Approved** | Explicit converters always (simpler, less convenient) | Medium |
| D19 | **GUI architecture:** Signal.app runs the engine and the GUI in one process; the GUI is a client of the same edit-command and observation interface as files and CLI; GUI edits are written back to project files (toml_edit preserves formatting); a headless `signal` CLI ships inside the bundle | **Approved**; first GUI = interface setup dialog + metering (Q23) | Separate GUI process talking to an engine process | Medium |
| D16 | Engine applies a deterministic fade (default 5 ms) when instances are added or removed; modules may override | **Yes** | No engine fade; each module's responsibility | Easy |
| D17 | Live edits issued by watching project files and diffing them into edit commands (plus CLI commands) | **Yes, pending Q6** | Command console or OSC only | Easy |

**All decisions approved by PO on 2026-10-11.** Licence: provisional, all rights reserved (Q2 delegated; ADR 0014). Each decision is recorded as an ADR in `docs/adr/`.
