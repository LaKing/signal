# Changelog

All notable changes to Signal are recorded here, by milestone (`docs/roadmap.md`). The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions are `MAJOR.MINOR.N`, where `N` counts the commits since the tag `vMAJOR.MINOR.0` (`scripts/version.sh`); a section covers a `MAJOR.MINOR` line.

## [Unreleased]

### Added

- **M3.1 Transport as a signal** (ADR 0033, owner's direction of 2026-10-22). `std.playhead`: a position in beats on a signal wire, with `bpm`, `run`, `rate` (a multiplier of the tempo: 0 holds, negative plays backwards; a connected signal replaces it) and a `seek` input, computed from the last change of tempo, rate or run so that it never drifts, and summed with compensation while the rate changes on every frame. `std.note_seq` and `std.step_seq8` take a `follow` input: connected to a position, they play from it instead of counting ticks, exact in beats, a stopped transport releasing its note and a jump firing nothing until the position moves on. `lib.transport` in the built-in library, and `projects/studio-transport`, the studio played from its own Transport card (play, stop, tempo, rate, rewind).
- Signal.app: **Project ▸ Restart from Files** plays the open project again from frame 0 from its files.
- A panel binds the exported parameters of a built-in library composite (`lib.*`), as it binds a project composite's.
- The module contract: a primitive can ask which of its inputs the plan connects (`Inputs::is_connected`), and a library can make two inputs exclusive (`Registry::exclusive_inputs`); a project that connects both is refused with the connection named.

### Changed

- Signal.app's toolbar has no Play or Stop: the open project plays while the power is on (when the app starts, when a project opens, when the power comes on) and starts and stops from its own panel. In Signal.app, `transport.play` while playing plays the project again from frame 0. To list the devices again, switch the power off.

## [0.2.0] - 2026-10-22

The first public release (M2.14, ADR 0031): Signal plays in real time, edits live, runs as a Mac app with a web UI, and has a schematic, a built-in library, patterns and an agent interface.

### Added

- **M2 Real time and live edits** (ADR 0006, 0011). CoreAudio output through cpal behind a driver trait; plan swaps for live edits and `signal play --watch`, which applies saved project files while it plays; session logs and `signal replay`, which renders a recorded live session again bit for bit; fault guards around primitives; benchmarks (`signal bench`, `docs/benchmarks.md`). The core primitive library: oscillators, filters, envelopes, delay, utility and event primitives. Verified by V9–V11 and V13, and a 30-minute soak at 96 kHz with 0 xruns (V12).
- **M2.5 Signal.app shell** (ADR 0013). A signed `Signal.app` with an interface setup dialog (devices, inputs and outputs, sample rate, buffer) kept as the machine Setup, input and output meters, a parameter panel, and `./start.sh`.
- **M2.6 Multi-core engine** (ADR 0019). Block execution and a deterministic multi-core scheduler: the output is identical for every block size and thread count. Real-time worker threads run in the audio workgroup. `signal-gpu` and GPU-backed modules with declared latency (a lab proof on Metal).
- **M2.7 Full hardware use** (ADR 0019, 0022). Switchable wake policies measured on the device (park50 is the default), task-failure semantics, replay verified by hashes and provenance, output adapters (`signal-output`: the audio sink, its gain, channel mapping and device conversion), loom models of the scheduler, an independent semantic oracle the engine is tested against, and a multi-GPU device pool with deterministic placement (`signal devices --compute`). Both timing gates passed on the device.
- **M2.8 Web UI foundations** (ADR 0023, 0024, 0025). The `signal-ui` protocol and local server (`signal ui`); web-component controls (fader, knob, 2D input, button, meter, scope) with SVG and pixel skins; panels as project text; Signal.app on a native window with a WebView (wry/tao), with File → Open, Save and Save As, and `.signal` project files (a project folder in one zip; `signal pack`, `signal unpack`). Physical-modelling primitives for sound and visuals (`std.mesh2d`, `std.particles`, `std.body2d`, `<signal-canvas>`), rate divisors and vector ports.
- **M2.9 Schematic and panel editor** (ADR 0026). The signal schematic: an infinite-zoom SVG view of the patch with semantic zoom into composites, live levels and a deterministic layout kept in `ui/schematic.toml`; a panel layout editor; patching from the schematic.
- **M2.10 Core set and built-in library** (ADR 0027). A complete minimal core set of `std.*` primitives by category, a validation suite that rebuilds std modules from core primitives against their native references, and the `lib.*` library of composites shipped with Signal (LFOs, envelopes, oscillators, filters, effects, drum voices; `signal vendor` copies one into a project).
- **M2.11 Studio demo** (ADR 0028). Patterns as project text (`patterns/*.toml`) with live edits, the transport's tick clock, `std.note_seq`, `std.step_seq8`, the `std.ladder` filter, the piano-roll and step-grid controls, and `projects/studio`: a piano roll driving a Moog-style voice through a tape delay over a step-sequenced drum machine with a synthesised 909-style kick.
- **M2.12 Agent interface** (ADR 0029). `signal mcp`: a Model Context Protocol server over the UI protocol, so an LLM client can inspect, play, adjust, patch and render a project. Audio is off unless `--audio`, and the output is held under a gain ceiling (`--max-gain`).
- **M2.13 Schematic path tracing** (ADR 0030). Tracing a signal upstream and downstream through modules and across composite boundaries; composites that open in place with continuous wires; pin notes that read the way the signal flows; labels and text that scale with zoom.
- **M2.14 Release** (ADR 0031). `README.md`, `CONTRIBUTING.md`, `SECURITY.md`, this changelog, `THIRD-PARTY-NOTICES.md` generated from `Cargo.lock`, `scripts/release.sh` and a GitHub Actions release workflow.
- **Several windows over the screens** (ADR 0032, owner's request of 2026-10-22). Signal.app opens any number of main, schematic and panel windows on the running project (Window ▸ New Main Window ⇧⌘N, New Schematic Window, New Panel Window), each on any screen or full screen, with the system's Minimize, Zoom, Bring All to Front and window list. The project panel's cards are moved between windows from their context menu (right-click a card: Move to another window, Move to new panel window, Bring back to main window, Move up / Move down); a main window keeps a placeholder (Show, Bring back) for a card held elsewhere. The layout is kept in the project's `ui/windows.toml` under a key describing the connected screens (names, resolutions, scale, arrangement); another screen configuration opens in the default state (one main window on the main screen with every card), and Window ▸ Reset Layout returns to it. Closing the last window quits as before; closing any other just closes it.
- **Schematic: open on a click, a toolbar** (ADR 0030, owner's requests of 2026-10-22). A composite opens in place on a click and closes with the × in its frame's corner or Escape; zooming and panning only move the view. The schematic's buttons (Edit patch, breadcrumbs, Fit, Reset layout, Reset all) sit on a toolbar row above the drawing instead of floating over it.

### Changed

- Signal is licensed under **GPL-3.0-or-later** (ADR 0031, superseding ADR 0014's provisional "all rights reserved"). Dependencies stay permissive.
- The GUI moved from egui (ADR 0018) to the web UI (ADR 0023).
- Versions count commits: `MAJOR.MINOR.N` (`scripts/version.sh`), built into Signal.app and `signal --version`.

### Known limitations

- Under heavy multi-core load the worst audio callback is about 0.4–0.45 of the period: within the capacity gate, above the 0.25 that holds for single-voice projects.
- `std.tanh_shaper` aliasing on off-divisor tones is below the 60 dB target (V6, waived for M1; planned for M3).
- The app is signed ad hoc, not notarized: the first start needs the Gatekeeper step in the release notes.
- macOS on Apple Silicon only.

## [0.1.0] - 2026-10-11

M0 and M1: the foundations and the offline core. (Version numbers began later, with the tag `v0.1.0` on 2026-10-16; builds between that tag and `v0.2.0` are `0.1.N`.)

### Added

- **M0 Foundations.** The vision, the inception proposal and ADRs 0001–0015; a Cargo workspace with a pinned toolchain; `AGENTS.md`; the real-time rules; CI (fmt, clippy, tests).
- **M1 Core and offline.** Core types and the module contract (ADR 0002) with its contract test suite, and the execution contract (ADR 0016); the graph crate (composites, flattening, validation, scheduling); TOML project loading and a generated JSON Schema (ADR 0009); the first primitives; the offline driver; `signal render`, `signal modules` and `signal schema`; the test project `projects/kick` and its golden render. Acceptance V1–V5, V7 and V8 pass, V6 is waived until M3, and the owner's listening verdict (V14) is in `experiments/m1-kick-v14/verdict.md`.
