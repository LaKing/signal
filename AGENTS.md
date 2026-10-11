# AGENTS.md — working on Signal

Signal is a macOS (Apple Silicon) application that is internally a platform for developing and interconnecting real-time signal modules. It is written in Rust. Start here, then read only what your task needs.

## Read first
| If you are… | Read |
|---|---|
| Doing anything | `docs/00-vision.md` §2–4 and §3a (terminology), `docs/architecture.md` |
| Touching code that can run on the audio thread | `docs/realtime-rules.md` (normative) |
| Changing a design decision | `docs/adr/` — never contradict an accepted ADR silently; propose a superseding ADR |
| Picking up work | `docs/roadmap.md` — current milestone and exit conditions |

## Commands
If `cargo` is not on PATH (Homebrew rustup), prefix with `PATH=/opt/homebrew/opt/rustup/bin:$PATH`. The toolchain is pinned by `rust-toolchain.toml`. Do not change it without an explicit decision.

```sh
./install.sh                                                     # operator: toolchain, tools, fetch deps, verify
cargo fmt --all --check                                          # formatting (CI)
cargo clippy --workspace --all-targets --offline -- -D warnings  # lints as errors (CI)
cargo test --workspace --offline                                 # tests (CI uses cargo nextest run --workspace)
```

Development runs through **devpanel** (`~/devpanel`, project `signal-project`); its seats build **offline**.

## Hard rules
1. **No domain concepts in the core.** `signal-core`, `signal-graph`, and `signal-engine` know frames, ports, events, and addresses only. Tempo, MIDI, DMX, and video belong to library modules (ADR 0001, 0003).
2. **Real-time rules are mandatory** for `process` and the executor: no allocation, locks, I/O, or panics (`docs/realtime-rules.md`).
3. **Determinism.** No wall-clock time or unseeded randomness in semantics. Output must be identical for any *k*-partition of frames (ADR 0002, 0008).
4. **Text is the source of truth.** Anything a project means must be expressible in its files. The GUI and tools edit files; they hold no hidden state (ADR 0009, 0013).
5. **Experimental work goes in `lab/`** behind the `lab` feature. Promotion requires the contract suite, benchmarks, and a recorded human verdict (ADR 0015).
6. **Metrics never decide quality.** Present renders and metrics; a human records the verdict.
7. **Dependencies** come only from the root `[workspace.dependencies]`, which `./install.sh` has already fetched. Moving a locked crate into another crate is fine; adding a new crate is an operator step (edit the root Cargo.toml, run `./install.sh`). Licences must be permissive (MIT, Apache-2.0, BSD, ISC, Zlib); MPL-2.0 only with a note; never GPL (ADR 0014).
8. **Don't build ahead of the roadmap.** Create crates, port types, and features when a milestone or test project needs them.
9. **`unsafe` is denied workspace-wide.** Exceptions need an ADR and a single audited module.

## Terminology
Use the terms in `docs/00-vision.md` §3a consistently: Project, Module, Primitive, Composite, Instance, Port, Connection, Parameter, Preset, Library, Setup.

## Reporting results
State what you ran and its outcome. If something is skipped or failing, say so. Label claims as fact, measurement, or assumption.
