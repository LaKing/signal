# Changelog

All notable changes to Signal are recorded here, by milestone (`docs/roadmap.md`). The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions are `MAJOR.MINOR.N`, where `N` counts the commits since the tag `vMAJOR.MINOR.0` (`scripts/version.sh`); a section covers a `MAJOR.MINOR` line.

## [Unreleased]

Since 0.2.0 (there is no 0.3 release; the next is 0.4): projects inside projects and slots, the transport as a signal on a wire, snapshots, and a schematic redrawn as a front plate over a circuit board. Every golden render from 0.2.0 sounds the same.

### Added

- **Transport as a signal** (M3.1, ADR 0033). `std.playhead` puts a position in beats on a wire, exact and drift-free, with tempo, run, a rate that can hold or reverse, and seek; `std.note_seq` and `std.step_seq8` follow it. `lib.transport` wraps it, and `projects/studio-transport` plays the studio from a Transport card on its own panel.
- **Playing keys and Space** (M2.15a, ADR 0039). The piano roll's keys play while held, stopped or playing: velocity from where the key is pressed, a slide moves the note legato, the key lit. Space plays and stops the project's transport in the main, panel and schematic windows; several transports start on the same frame.
- **Keyboard shortcuts and scrolling** (M2.15, ADR 0034). One keymap as text, your overrides in `keys.toml`, a Keyboard Shortcuts editor in Signal ▸ Settings… (⌘,) and Help ▸ Keyboard Shortcuts. Page Up and Page Down scroll the view.
- **A project as content, and slots** (M3.2, ADR 0035, 0037). A whole project can be an instance in another (`project = "../studio-transport"`) with its own exports. A slot is a socket with an editable interface that holds a voice, an instrument or a deck. Load, Unload and Replace are glitch-free live edits. A deck follows the host's transport when wired, and its `release` decides what happens when the wire goes. A slot opens and is edited like a composite, each with its own inside. Slot ports can be typed by what they connect to (ADR 0045, phase 1). A slot can have its own panel part, and Group into slot turns controls into one (ADR 0046, phase 1). Examples: `projects/two-studios`, `projects/two-decks`.
- **Snapshots** (ADR 0041). Named snapshots of a project's parameters in `snapshots.toml`, with Save, Restore, Delete and an undo snapshot. You can leave a control out, drag snapshots into your own order, and set a transition time that ramps the controls instead of jumping.
- **The schematic as a front plate and a board** (ADR 0047, phases 1–5). The panel is a fixed front plate. Below it, the board shows chips under their controls, routed wires and ribbons with one colour per net. Zooming into a chip shows its die, and composites are drawn as packages. Panel controls appear as nodes of their own, and a slot's part is its face.
- **Schematic navigation**. Pan on composites; the trackpad pans, pinch and Shift+wheel zoom, and a slow mouse-wheel notch zooms. ⌘= and ⌘- zoom and ⌘0 fits, as in a browser. ⌘-drag selects with a marquee. The context menu's Show in Panel, Parameters and Schematic reveal and flash what you picked. The meters show the selected instance's inputs and outputs.
- **Add module browser**. A full-window dialog with the module types as a tree, a search and a detail pane.
- **Ranges for every parameter** (ADR 0042, 0043). Every parameter has a range or a set of values, and every exported parameter, `lib.*` included, declares its own range. A value outside the range in a file is corrected with a warning, never refused. A control whose bound value lies outside its own range shows the real value, pins its pointer at the end and marks it, but never writes it.
- **`std.tanh_shaper_os8`** (M3). An oversampled `tanh` shaper that meets the 60 dB aliasing target (V6), next to the unchanged `std.tanh_shaper`.
- Signal.app: **File ▸ Revert to Saved…** for `.signal` projects, **Project ▸ Restart from Files**, and a note naming the instances the fault guard muted.
- `signal mcp`: slot and snapshot tools; `transport tempo` reaches `lib.transport` and moves every transport on one frame.

### Changed

- Signal.app's toolbar has no Play or Stop. A project plays while the power is on and starts and stops from its own panel.
- **The device's clock is the run's** (ADR 0036). Signal plays at the output device's rate and never switches it, so it joins the device without a glitch. When a device does not start, Signal says in plain words which rates it supports and what to do.
- The wheel scrolls the page. A control takes the wheel only when it is clicked or focused, while ⌥ is held, or when it fills its window, and a hint says how.
- Typing a value: a double-click on a value opens the field, and a decimal comma or dot both work.
- Saving and writing files go through one writer per project (ADR 0040, 0048). A write that waits for another writer queues in order and never stalls the audio or the control thread. An undo never overwrites another client's newer edit.
- A change to presentation files only (panels, styles) is pushed to the page without reloading the project. A pattern changed on disk is pushed to the page, and an unsaved draft is kept.

### Fixed

- The schematic: a dragged composite's pin labels follow its box; primitive boxes draw again; labels keep off the sidebar.
- Host robustness: a held note's release is never lost, and a note held for a client that has gone is never kept. A request queued after Save or Open applies to the project that is open when it runs. Large messages and pattern writes stay within their limits. A session that records nothing keeps no history in memory.
- `signal mcp` and patch simulation refuse a slot project or a panel file outside the allowed roots without reading it.

### Known limitations

- Includes are not watched: an included project's own files are read again only on a reload of the host.
- The app is signed ad hoc, not notarized: the first start needs the Gatekeeper step in the release notes.
- macOS on Apple Silicon only.

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
