# Architecture overview

This is the living summary of Signal's architecture. Decisions and their rationale live in `adr/`; this page links them together. Inception history: `01-inception-proposal.md`.

## What Signal is

One macOS application (`Signal.app`, plus a headless `signal` CLI in the bundle) that is internally a platform. Signal provides:
- the **execution model**: master frame clock and frame-functional semantics;
- the **connection model**: ports, rates, kinds, events, and conversion;
- a **standard library of primitives**;
- **I/O drivers**;
- **tooling**: render, replay, compare, and test.

**Projects** (application instances) choose modules, nest them into composites, and connect them. Signal itself contains no musical, measurement, or lighting logic. [ADR 0001]

## Concepts

| Concept | Summary | ADR |
|---|---|---|
| Frame | `Frame(u64)` at the project's master rate (default 96 kHz) is the only time the core knows | 0003 |
| Module | Primitive (Rust, compiled in) or composite (text, nestable) | 0001 |
| Semantics | `(inputs[n], state[n-1]) -> (outputs[n], state[n])`; feedback needs a one-frame delay; every non-suspended instance advances every frame | 0002, 0016 |
| Frame phases | external input → emit (state-output primitives) → process (direct primitives, topological order) → absorb | 0016 |
| Same-frame order | external events by log sequence, then module events by source address; events set the base, signals modulate | 0016 |
| Module contract | `describe` (incl. feedthrough class), `prepare` (only place allocation is allowed), `process(k frames)` for direct primitives or `emit`/`absorb` for state-output primitives (*k*-partition-invariant), `reset` | 0002, 0016 |
| Port | shape + rate (integer divisor) + kind + unit/range; parameters are input ports with defaults | 0004, 0005 |
| Conversion | implicit / default / explicit; converters are modules, auto-inserted only when unambiguous and always visible | 0005 |
| Event | generic messages on event ports; MIDI/OSC only via adapter modules | 0004 |
| Plan | composites flattened into a versioned, ordered execution plan | 0006 |
| Edit | add/remove/connect/set commands; plans swapped at frame boundaries; effective frame logged | 0006 |
| Reproducibility | seeded per-instance RNG streams, logs, manifest; bit-exact on the same build | 0008 |
| Project files | TOML + generated JSON Schema; machine Setup kept separate | 0009 |

## Data flow

```
project folder (TOML) ─► signal-project ─► signal-graph ─► Plan vN ─► signal-engine ─► drivers
                                               ▲   resolve composites,            ├─ offline (reference)
           edits: file-watch diff, CLI, GUI ───┘   flatten, insert converters,    └─ realtime (CoreAudio)
                                                   check feedback delays,
                                                   schedule all instances (ADR 0016)
render artifacts: audio + manifest + logs + metrics ─► signal-eval (analyse, compare)
```

## Threads (real time)

| Thread | Does | Must not |
|---|---|---|
| Audio (CoreAudio callback) | Executes frames; swaps plans at frame boundaries; applies queued events; runs fault guards | See `realtime-rules.md` |
| Control | Parses files, builds plans, calls `prepare`, reads device input/MIDI/network and stamps it, frees old plans, writes logs | Block the audio thread |
| GUI (`signal-app`) | Setup dialog, meters, panels; a client of the edit and observation interface | Hold state that is not in text or Setup |
| GPU (`signal-gpu`, only when a GPU-backed module is prepared) | Owns the wgpu device; runs GPU-backed modules' jobs and waits for them (ADR 0019 §3, `realtime-rules.md` "GPU") | Be waited on by the audio thread or the workers |

Communication between threads uses only lock-free SPSC queues (`rtrb`). The audio thread runs `signal_engine::Runner`; the control thread holds its `Controller`, which prepares swaps, drops what the runner returns, and hands logs, meters, parameter values and command reports to a sink; recording a session from them happens outside the engine (`signal_project::session::LiveRecorder`). Signal.app runs a control thread (the `Controller`, the file watch, the input meters and the Setup) and the GUI thread. They exchange commands and view snapshots over `rtrb` queues too, and only the control thread touches the `Controller`. Since ADR 0019 the audio thread also drives a pool of **worker** threads that execute a block's independent units (created with the engine and kept across swaps; ordinary threads, put into the Mach time-constraint policy and the output device's I/O workgroup while a CoreAudio stream plays, M2.6-3); they synchronise through atomics only (`signal-engine/src/shared.rs`, ADR 0020), and the result does not depend on how the work is split (`realtime-rules.md` "Worker threads").

## Crates

See `adr/0015-workspace-and-lab-tier.md`. Crates are created when their milestone needs them.

| Crate | Responsibility | Created in |
|---|---|---|
| `signal-core` | Frame, ports/values, events, addresses, RNG, module trait | M0 (placeholder), M1 |
| `signal-graph` | Composites, flattening, validation, conversion insertion, plans, edits | M1 |
| `signal-project` | Project folder loading, TOML model, schema export, project diff; session record, read and replay (`session-format.md`) and build provenance, shared by `signal` and Signal.app. Depends on `signal-engine` for sessions; the engine depends on no project or session code | M1/M2 |
| `signal-std` | Standard primitives | M1 |
| `signal-engine` | Executor, plan swap, fault guards, render manifest (semantics: `semantics.md`); the real-time runner and controller, the driver trait, offline and null drivers (`realtime-rules.md`, "The runner") | M1/M2 |
| `signal-audio` | `CoreAudioDriver` through cpal (ADR 0011), device listing, callback statistics | M2 |
| `signal-eval` | Analysis, metrics, compare, listening packages | M1/M3 |
| `signal-oracle` | Test support: the semantic oracle, a deliberately simple per-frame interpreter of `semantics.md` the block engine is compared with (`testing-and-evaluation.md`). Depends on `signal-core` alone | M2.7 |
| `signal-cli` | `signal` command layer only: `render`, `replay`, `play`, `bench`, `devices`, `modules`, `schema` | M1/M2 |
| `signal-app` | Signal.app shell and GUI (egui, ADR 0018) | M2.5 (skeleton in M2) |
| `signal-gpu` | GPU device service (wgpu, Metal): one GPU thread, jobs through lock-free queues (ADR 0019 §3) | M2.6 |
| `signal-lab` (`lab/`) | Lab primitives (ADR 0015), compiled in only with its `lab` feature (`signal` and `cargo test -p signal-lab --features lab`): `lab.gpu_partials` | M2.6 |
