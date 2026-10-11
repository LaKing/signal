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
| Semantics | `(inputs[n], state[n-1]) -> (outputs[n], state[n])`; feedback needs a one-frame delay; every instance advances every frame | 0002 |
| Module contract | `describe`, `prepare` (only place allocation is allowed), `process(k frames)` (*k*-partition-invariant), `reset` | 0002 |
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
                                                   order by pulling from outputs
render artifacts: audio + manifest + logs + metrics ─► signal-eval (analyse, compare)
```

## Threads (real time)

| Thread | Does | Must not |
|---|---|---|
| Audio (CoreAudio callback) | Executes frames; swaps plans at frame boundaries; applies queued events; runs fault guards | See `realtime-rules.md` |
| Control | Parses files, builds plans, calls `prepare`, reads device input/MIDI/network and stamps it, frees old plans, writes logs | Block the audio thread |
| GUI (later) | Setup dialog, meters, panels; a client of the edit and observation interface | Hold state that is not in text or Setup |

Communication between threads uses only lock-free SPSC queues.

## Crates

See `adr/0015-workspace-and-lab-tier.md`. Crates are created when their milestone needs them.

| Crate | Responsibility | Created in |
|---|---|---|
| `signal-core` | Frame, ports/values, events, addresses, RNG, module trait, manifest types | M0 (placeholder), M1 |
| `signal-graph` | Composites, flattening, validation, conversion insertion, plans, edits | M1 |
| `signal-project` | Project folder loading, TOML model, schema export | M1 |
| `signal-std` | Standard primitives | M1 |
| `signal-engine` | Executor, plan swap, fault guards, drivers | M1/M2 |
| `signal-eval` | Analysis, metrics, compare, listening packages | M1/M3 |
| `signal-cli` | `signal` command | M1 |
| `signal-app` | Signal.app shell and GUI | M2.5 |
