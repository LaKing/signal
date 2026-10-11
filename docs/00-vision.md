# 00 — Product Vision: Signal

| | |
|---|---|
| Status | **Approved by PO, 2026-10-11** (v4–v7). |
| v5 changes | Signal as one macOS application that is internally a platform; ease of use added as goal G12; Q22 (application form) opened. |
| v6 changes | Signal is a GUI `.app` (Q22 answered); G13 interconnectivity added; conversion model (§9a). |
| v7 changes | Q23 answered: the first GUI is the interface setup dialog plus metering. Q2: provisional licence. D18/D19 accepted with the M0 go-ahead. *Setup* term added (§3a). |
| v3 changes | Projects as application instances; no "first application" (platform-first); runtime load/unload of built modules; always-seeded randomness confirmed; 96 kHz default discussed; terminology proposal (§3a). |
| Date | 2026-10-11 |
| Name | **Signal** (decided by PO) |
| Change from v1 | Scope widened from a generative techno engine to a **modular real-time signal platform**; techno/generative music is one application domain. AI is confirmed as development-time only. Reproducibility scope clarified. |
| Next gate | Approval of this document and of the execution model (§9) → detailed requirements and ADRs |

Legend: **[Fact]** established, **[PO]** stated by product owner, **[Rec]** discovery-agent recommendation, **[Assumption]** unconfirmed, **[Open]** unresolved.

---

## 1. Problem statement

People who build real-time audio and signal systems through code currently choose between:

- **Graphical environments** (Max/MSP, Pure Data, Reaktor, TouchDesigner). Powerful, but patches are visual or binary, hard to diff, test, or reproduce, and nearly opaque to AI coding agents.
- **Text environments** (SuperCollider, Faust, TidalCycles, Glicol). Text-first, but each is centred on one language and one domain (usually music) and is not designed around reproducible experiments, automated evaluation, or agent-driven development.
- **Domain-specific products** (DAWs, measurement suites, lighting controllers, VJ tools). Each solves one application, is closed to deep modification, and does not interconnect with the others.

No environment is designed so that **a human and AI coding agents can jointly develop, test, interconnect, and evolve real-time signal modules, from synthesis to measurement to lighting, with every significant decision captured as reproducible text.** [PO]

## 2. Vision statement

> **Signal is a technical and creative platform for developing and interconnecting real-time signal-processing modules.** Modules nest inside modules, can be added and removed while running, and connect audio, control, analysis, visual, and lighting signals under one precise, sample-accurate master clock. Everything significant is text and code: inspectable, reproducible, modifiable. AI agents are first-class collaborators in building the platform and its modules. Humans judge what sounds, looks, and works right. [PO]

## 3. Application domains (examples, not commitments)

The platform must make these *possible* through modules. None is a platform feature in itself. [PO]

| Domain | Example composition of modules | Platform capabilities it stresses |
|---|---|---|
| Live electronic music | sequencers, synths, samplers, mixers, dynamics processors | sample-accurate events, low latency, live editing, long-form generative behaviour |
| Loudspeaker measurement | test-signal generators, audio in/out, live analysis (FFT, impulse response) | input/output latency calibration, precise timing, analysis frames, numerical accuracy |
| Single instruments / effects | effects driven by internal 3D models, hardware emulation, physics simulation | heavy per-sample computation, nonlinear feedback, oversampling |
| Audiovisual tool | live audio input → custom modules → visual output | multi-rate processing (audio vs video frames), visual output devices |
| Reactive DMX controller | audio input → analysis and pattern detection → Art-Net output | non-audio output protocols, low-rate control domains, network I/O |

Experimental and generative music, especially techno, remains the primary **creative** domain. **No application comes first.** Signal is built as a platform and ecosystem. [PO]

Each application instance is a **Project** within the platform. `signal open <project>` reads the project's description files, which declare the modules to use, how they nest and connect, and their parameters, seeds, and I/O. Projects are inspired by Native Instruments Reaktor's Ensembles. [PO]

Early milestones still need concrete content to prove the platform. They use **test projects**: fixtures that exercise platform capabilities (a kick drum, a test-tone analyser), not products. [Rec]

## 3a. Terminology (proposal — PO to settle)

Reaktor-inspired: a hierarchy, the split between built-in low-level units and user-built containers, and snapshots. Not inherited from Reaktor: binary file formats, GUI-only editing, implicit event-ordering rules.

| Proposed term | Meaning | Reaktor analogue |
|---|---|---|
| **Signal** | The platform: engine, runtime, tools, standard library of modules. | Reaktor |
| **Project** | An application instance: a folder of text files (project file, module definitions, presets, assets) that `signal open` runs. | Ensemble |
| **Module** | Anything with ports and parameters that can be instantiated in a graph. Either a primitive or a composite. | (Module / Macro) |
| **Primitive** | A module type implemented in Rust against the frame contract, compiled and tested by the development workflow. The building block. | Primary module / Core cell |
| **Composite** | A module type defined in text, containing module instances and connections, exposing selected ports and parameters. Nesting is unlimited. | Macro / Core macro |
| **Instance** | One occurrence of a module type in a project graph, with a stable ID and hierarchical address (`/mixer/ch3/eq`). | Module instance |
| **Port** / **Connection** | Typed input/output with a rate; a directed link between ports. | Port / Wire |
| **Parameter** | A named, typed, addressable value of an instance, settable by events or modulation. | Knob / Control |
| **Preset** (or Snapshot) | A stored set of parameter values for a module or project. | Snapshot |
| **Setup** | *[Rec]* Per-machine configuration, kept outside projects: audio device selection, sample rate support, mapping of physical channels to a project's logical inputs and outputs, network interfaces. Edited in the GUI's interface setup dialog. Projects stay portable between machines. | (Audio and MIDI settings) |
| **Library** | A versioned collection of module types (primitives and composites) a project can depend on. | Library / User Library |
| **Instrument** | *Proposed as a role tag, not a structural concept.* A composite marked as a self-contained musical unit. Avoids a music-only word in the core, since DMX or measurement projects have no "instruments". | Instrument |

## 4. Goals

| ID | Goal | Source |
|---|---|---|
| G1 | **Text is the source of truth.** Module definitions, graphs, parameters, and generative rules are version-controllable text that humans and agents can read, diff, validate, and edit. | PO |
| G2 | **Modular and hierarchical.** Everything is a module. A module can contain and expose submodules. Applications are compositions of modules. | PO |
| G3 | **Live mutation.** Modules and connections can be added and removed while the system runs, without dropouts. | PO |
| G4 | **One precise master clock.** All processing is defined relative to an application master clock (e.g. 96 kHz, adjustable) with a functional, frame-based computation model (see §9). | PO |
| G5 | **Reproducibility.** Given definitions, software revision, seeds, and recorded external inputs (including any live graph edits), output is bit-exact on the same build and hardware class. | PO + Rec |
| G6 | **One semantics, two modes.** Real-time and offline (faster-than-real-time) execution share identical semantics. | PO |
| G7 | **Multi-domain I/O through modules.** Audio in and out, MIDI/OSC, network protocols (e.g. Art-Net), and visual output are reachable through modules rather than hard-wired into the core. | PO |
| G8 | **Professional signal quality.** Numerical stability, antialiasing, accurate timing, predictable real-time behaviour, measurable floors. | PO |
| G9 | **AI-native development.** Agents can propose, implement, test, render, measure, compare, and present modules, and the platform is structured to make that effective. | PO |
| G10 | **Reliability boundary.** Experimental modules cannot silently compromise the real-time core. | PO |
| G11 | **Musical intelligence as a supported domain.** The platform must not preclude long-form, stateful, hierarchical musical processes with memory and direction. | PO |
| G12 | **Easy to use.** For now this means: clear project files, precise validation errors, a self-documenting module catalogue, sensible defaults, and a short path from `signal open` to sound. Visual tooling is a later layer, not a substitute. | PO |
| G13 | **Interconnectivity is Signal's core strength.** Ports are typed in the software sense, but as interchangeable as possible in Signal's sense. Signal provides converters, inserts unambiguous ones automatically and visibly, and makes custom conversions easy for module authors. Example: send blue to fixture X while audio signal Y is playing bass. | PO |

## 5. Non-goals

- **No AI in the runtime signal path.** AI participates only in developing the platform and its modules. *(Decided, PO.)*
- Not a DAW, measurement suite, or lighting desk in itself. These are *applications* built from modules. *(Permanent intent.)*
- No clone of Max/MSP, Reaktor, SuperCollider, TouchDesigner, or Ableton Live. *(Permanent.)*
- The GUI never replaces text as the source of truth. Every GUI action is an edit written back to the project files. *(Permanent intent [Rec].)* The GUI's scope and timing are Q23.
- No third-party plugin hosting (VST3/AU/CLAP) and no plugin build initially.
- No large module or preset library initially.
- No claim that metrics decide musical or visual quality. *(Permanent.)*
- No platform other than macOS on Apple Silicon initially.

## 6. Users and personas

**P1 — Platform Developer (product owner).** Designs the core and modules; fluent in code; works with AI agents; judges results by ear, by eye, and by measurement. Values precision, reproducibility, and deep control.

**P2 — AI Coding Agent.** Develops the core and modules. Needs explicit machine-readable contracts, deterministic outcomes, fast feedback, isolated module boundaries, and clear pass/fail signals. Cannot judge creative quality on its own authority.

**P3 — Project Builder.** Assembles modules into a project (a live set, a measurement rig, a DMX show) by writing text definitions, without changing the core. Often P1; potentially others later (Q3).

**P4 — Operator / Performer.** Runs an application live: a musician, a measurement engineer, a lighting operator. Adds, removes, and steers modules during operation.

**P5 — Evaluator (role).** The human who listens, watches, or inspects measurements and decides keep/revise/reject. Often P1, but a distinct step in workflows.

## 7. Core workflows

- **W1 — Build a module.** Agent or human implements a module against the module contract. Contract tests, renders, and metrics run; the evaluator decides; the module is promoted or rejected.
- **W2 — Compose a project.** Write project description files that instantiate and nest modules and connect their ports. Validate them, then `signal open` the project.
- **W3 — Run and mutate live.** Start an application in real time; add, remove, and reconnect modules and change parameters while it runs. Every change is timestamped and logged.
- **W4 — Render and reproduce offline.** Render any application faster than real time from definitions + seed + recorded inputs, bit-exactly. This includes replaying a live session with its edits.
- **W5 — Analyse and react.** Live input → analysis modules → derived control → outputs (sound, visuals, DMX, measurement reports).
- **W6 — Long-form generative performance.** An open-ended, stateful process develops over tens of minutes and can be reproduced from its manifest.

## 8. Constraints

| ID | Constraint | Source |
|---|---|---|
| C1 | macOS on Apple Silicon initially. | PO |
| C2 | Rust preferred. | PO |
| C3 | Avoid proprietary software, closed formats, heavyweight frameworks; use focused open-source libraries where clearly beneficial. | PO |
| C4 | Don't reimplement OS I/O, standard math, or established infrastructure without good reason. | PO |
| C5 | Real-time path: deterministic timing, bounded execution, controlled memory, resilience to faulty modules. | PO |
| C6 | No domain's conventions (techno, MIDI notes, track timelines, audio-only signals) may be hard-coded into the core. | PO |
| C7 | Human judgement remains the final arbiter of creative quality. | PO |

## 9. Execution model: PO proposal and discovery response

**PO proposal:** a master clock (e.g. 96 kHz, adjustable) defines a *calculation frame*. In every frame, everything needed is calculated, functionally, pulling backward from outputs toward inputs.

**Response [Rec]: agree with the semantics, change the execution.**

1. **Adopt functional per-frame semantics as the normative model.** Every module is a function `(inputs[n], state[n−1]) → (outputs[n], state[n])`. That makes results independent of buffer size by definition, gives feedback a precise meaning (every cycle must pass through a one-frame delay), and makes determinism natural.
2. **"Pull from outputs" is the right way to *define* what gets computed.** For an acyclic graph, though, pulling and a forward pass in topological order compute exactly the same values. The pull is therefore done **once, when the graph changes**: reachability from outputs determines the active set and its order. It is not repeated at runtime every frame.
3. **Per-frame semantics, block execution.** Literal per-sample dispatch at 96 kHz forfeits vectorisation and cache efficiency, and it doesn't fit inherently block-based work (FFT, convolution, video frames, 3D). Modules outside feedback loops process many frames per call. That is provably equivalent under rule 1. Feedback loops run frame by frame, or at the granularity of the loop's delay.
4. **A literal per-frame pull interpreter is kept as the reference oracle.** It is slow, obviously correct, and used in tests: the optimised engine must match it bit-for-bit.
5. **Lazy pulling has a semantic trap.** If an unobserved module is not computed, its time stops: phases freeze, envelopes pause. Recommended semantics: every module in the running graph advances every frame. Suspending a module is an explicit, visible action.
6. **One master clock, many rates.** Video (~60 Hz), DMX (~44 Hz), analysis hops, and control signals should not run at 96 kHz. Each port has a rate that is an exact subdivision of the master clock, and rate changes are explicit at connections.
7. **Physical clocks drift.** The master clock is *logical*. In real time it follows the primary audio device. Other devices (a second interface, the display, the network) are attached through clock-domain adapters (resampling, frame repeat or drop). Offline there are no physical clocks, so everything is exact.

**PO response (2026-10-11): D-EXEC-1 and D-EXEC-2 approved. D-EXEC-3 deferred to benchmarks.**

| ID | Decision | Status | Reversibility |
|---|---|---|---|
| D-EXEC-1 | **Semantics:** frame-functional. Each frame, each module computes `(inputs[n], state[n−1]) → (outputs[n], state[n])`. Every feedback loop contains at least a one-frame delay. Every module advances every frame. | **Approved** | Hard: defines what every result means |
| D-EXEC-2 | **Module contract:** a module processes *k ≥ 1* consecutive frames per call, with results required to be identical for any *k*. A per-frame engine simply calls it with *k* = 1. | **Approved** | Hard: every primitive is written against it |
| D-EXEC-3 | **Engine scheduling:** start with the PO's model literally. Evaluation order is derived by pulling from outputs whenever the graph changes, then one frame is computed at a time. Block execution (*k* > 1) is added later **only if measurements show a need**, verified bit-exact against the per-frame engine. | Deferred: decided by benchmarks | Easy: internal to the engine |

## 9a. Interconnectivity and conversion model (proposal [Rec], needs PO confirmation)

**Principle.** Any output can be connected to any input. Signal either connects them directly, inserts a converter it can choose unambiguously, or explains which converter is missing and suggests candidates. Nothing is converted invisibly. Inserted converters are real module instances, so they appear in the plan, the logs, and the GUI.

**1. Port descriptions carry meaning, not just a data type.** A port is described by:
- **shape:** scalar, multichannel/vector (N values), or event stream;
- **rate**;
- **kind:** audio, control, gate, trigger, colour, frequency, gain, position, …;
- **unit and range** where they apply (Hz, dB, 0–1).

Meaning is what makes good automatic conversion possible.

**2. Three levels of conversion.**

| Level | When | Examples | Behaviour |
|---|---|---|---|
| Implicit | Lossless and unambiguous | scalar → N channels (broadcast); dB ↔ gain; event *set* → held signal; 0–1 range ↔ declared range | Applied automatically, shown in the plan |
| Default converter | A sensible default exists but has parameters | audio → control (envelope follower: attack and release); control → gate (threshold with hysteresis); N channels → 1 (mean); high rate → low rate (peak, RMS, or last value) | Inserted automatically with default parameters that can be overridden on the connection |
| Explicit | Domain meaning is required | colour → fixture channels (needs a fixture profile); gate → colour; pitch → MIDI note | Validation error listing the converter modules that would fit |

**3. Converters are ordinary modules.** Any primitive or composite can declare itself a converter from one kind to another. Signal searches the catalogue for a conversion path. If exactly one shortest path exists it is used; if several are equally good, Signal asks instead of guessing. Module authors add custom conversions simply by writing a converter module, in Rust or as a text composite.

**4. Connections can configure conversion inline:** for example `{ from = "mic.out", to = "lights.level", via = "std.envelope", attack_ms = 5 }`.

**Example: "send blue to fixture X while audio Y is playing bass"** (illustrative syntax)
```toml
[instances]
y       = { module = "std.audio_in", channels = 2 }
bass    = { module = "std.band_energy", low_hz = 30, high_hz = 120 }   # audio → control 0–1
colour  = { module = "std.gate_select", on = "#0000ff", off = "#000000", threshold = 0.6, hysteresis = 0.1 }
fixture = { module = "dmx.fixture", profile = "fixtures/par-rgb.toml", address = 17 }
artnet  = { module = "net.artnet_out", universe = 0, host = "10.0.0.50" }

[wiring]
connections = [
  { from = "y.out",          to = "bass.in" },        # stereo → mono: default converter (mean), shown in plan
  { from = "bass.out",       to = "colour.in" },      # control → gate: inside gate_select
  { from = "colour.out",     to = "fixture.colour" },
  { from = "fixture.dmx",    to = "artnet.universe" },# rate: 96 kHz → ~44 Hz, default converter (last value)
]
```
Five instances, four connections, two converters inserted automatically and visibly.

**Not proposed:** an expression language inside connections (e.g. `when bass > 0.6 then blue`). It would be convenient, but it is the start of a DSL (§10.1). Revisit with evidence.

## 10. Complexity challenges [Rec, each needs PO agreement]

1. **No new programming language yet.** Declarative text for structure and parameters; algorithms are Rust modules behind a stable contract. Revisit a DSL or scripting layer when usage proves the need.
2. **Offline first.** The offline renderer is the reference. Real time is a driver over the same code.
3. **Live mutation = new graph version + atomic swap at a frame boundary**, prepared off the audio thread, with state carried over by stable module IDs. Avoid in-place mutation of a running graph.
4. **Two separate activities: developing a module and using one.** *Developing* a module, especially a primitive, is a non-real-time engineering task: code, tests, renders, review. *Using* a built module (loading, instantiating, connecting, unloading) happens live while a project runs. [PO] Built primitives ship compiled into the Signal binary (Q15b). Runtime-loaded code packages are a non-goal for now.
5. **Platform-first, but driven by test projects.** Each new capability (port types, rates, I/O kinds) is added when a concrete test project needs it, not speculatively for all five domains at once.
6. **Every milestone ends with a human verdict** (listen, watch, or inspect).

## 11. Assumptions (to be confirmed)

| ID | Assumption | Impact if wrong |
|---|---|---|
| A1 | Team is the PO plus AI agents; no external users in year one. | Docs, stability guarantees, UX. |
| A2 | Milestones are quality-gated, not date-gated. | Roadmap cuts. |
| A3 | Default master clock 96 kHz [PO, open to discussion], set per project. The primary audio device runs at the master rate, so there is no resampling on it. The engine and every module are rate-agnostic and tested at 44.1, 48, 96, and 192 kHz. | Device-rate mismatches need adapters early. |
| A4 | Headless (CLI + text) operation is acceptable until a visual domain is targeted. | Earlier UI work. |
| A5 | Bit-exact reproducibility holds on the same build and hardware class. Across compiler or library upgrades a documented tolerance is acceptable. | Stricter math and dependency discipline. |
| A6 | **Confirmed (Q18).** All random generation is seeded and the seed recorded, so "random" output is bit-exact. A run may draw a fresh seed for novelty, and that seed is recorded. | — |
| A7 | Live external inputs (audio in, MIDI, network) are reproducible only by recording them during the run. | Replay of live sessions would be impossible. |

## 12. Success criteria

**Platform foundation (first milestones)**
- A text-defined application of nested modules runs offline and in real time with identical output; bit-exact across runs, buffer sizes, and against the per-frame reference oracle.
- Modules are added and removed while running with no dropouts. A live session including its edits is replayed offline bit-exactly.
- Every event lands on its computed frame (0-frame error).
- Automated floors pass (no NaN/Inf, peak ceiling, DC, aliasing); ≥ 30 min real time with 0 xruns and 0 audio-thread allocations.
- **A human verdict:** the first application's output is judged usable.

**Longer term (indicative)**
- At least two different application domains (e.g. music and audio-reactive DMX) built from shared modules without core changes.
- A ≥ 20-minute generative performance judged coherent and reproducible from its manifest.
- An agent completes the module-development loop (W1) unaided up to the human verdict.

## 13. Open questions

Answered:
- **Q1:** the name is Signal.
- **Q4:** AI is development-only.
- **Q8:** sampling and live input are in scope.
- **Q12:** bit-exact (A5/A6).
- **Q14:** no first application. Platform-first, with projects as application instances.
- **Q15:** built modules are loaded and unloaded on the fly. Building them is offline development work. Packaging is settled by Q15b below.
- **Q18:** always seeded.
- **Q20:** terminology of §3a accepted.
- **Q23:** the first GUI is (a): an **interface setup dialog** (input/output and sound card selection), then **metering**. Control panels (b) follow naturally. Visual patching (c) is a nice-to-have, not a priority: manual wiring scales badly for big projects (a lesson from Reaktor), so text and automatic conversion stay the primary way to build structure.
- **Q2:** the PO delegated the choice. Provisionally *all rights reserved, unpublished* (ADR 0014). It is the only reversible option, since opening the code later is always possible. Revisit before any public release.
- **Q21:** 96 kHz is the default; the engine is rate-agnostic.
- **Q22:** Signal is a macOS `.app` with a GUI by default, with its own permissions (microphone, network). A headless command-line entry point inside the bundle remains, for agents, CI, and offline rendering. [Rec for the headless part]
- **Q15b:** (a) primitives are compiled into the Signal binary. Adding a new primitive means rebuilding Signal; loading and unloading built modules in a running project is live.
- **Q19:** polyphony is the responsibility of modules. There is no voice concept in the core.

| ID | Question | Why it matters |
|---|---|---|
| Q3 | Who uses Signal in year one: you only, a circle, the public? | Stability and documentation scope. |
| Q5 | Priority of live operation vs offline exploration in year one? | Order of real-time features. |
| Q6 | Live control means hardware controllers, editing text live, the GUI, or all of these? | Input and reload design. |
| Q7 | External sync (MIDI clock, Ableton Link, word clock, PTP/AES67)? | Clock-domain design. |
| Q9 | Channel counts: stereo, multichannel interfaces, spatial? (Measurement and DMX suggest multichannel I/O.) | Port and I/O design. |
| Q10 | Reference artists, sounds, or works that define "good" for the first application. | Anchors evaluation. |
| Q11 | How verdicts are recorded (keep/reject, ratings, notes)? | Evaluation tooling. |
| Q13 | Time or budget expectations? | Roadmap sizing. |
| Q16 | Visual output: what targets (window on screen, projector, Syphon, video file), and what latency or sync to audio? | Rendering stack and clock adapters. Can wait until that domain is targeted. |
| Q17 | Measurement accuracy needs (latency calibration, sample-exact input/output alignment, calibrated levels)? | I/O timing design. Can wait until that domain is targeted. |
| Q18 | Do you confirm A6 (always seeded, even for "random" generation)? | Determinism design. |

## 14. Approval checklist

- [x] Problem, vision, and platform scope (§1–3) accepted
- [x] Goals G1–G11 and non-goals accepted
- [x] Personas and workflows accepted
- [x] Execution model (§9): D-EXEC-1 and D-EXEC-2 approved; D-EXEC-3 deferred
- [x] Complexity challenges (§10) accepted or rejected individually
- [x] Assumptions A1–A7 confirmed or corrected
- [x] Terminology (§3a, Q20) settled
- [ ] Remaining questions answered or deferred
