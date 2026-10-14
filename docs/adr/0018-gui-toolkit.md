# ADR 0018: GUI toolkit: egui through eframe

- **Status:** Superseded by [ADR 0023](0023-web-ui.md) (web UI) in M2.8-4, 2026-10-14: Signal.app is a tao window with a wry WebView, and egui and eframe are gone from the workspace. Accepted 2026-10-11 (operator decision, delegated by the owner). The OFL font exception below lapsed with egui.
- **Date:** 2026-10-11

## Context
ADR 0013 puts the engine and the GUI in one process and makes the GUI a client of the edit-command and observation interface. It leaves the toolkit to this ADR. The first GUI jobs (Q23) are the interface setup dialog, metering, and then simple control panels. The owner asked for a basic UI in the M2 work.

## Decision
- Use **egui 0.36 through eframe 0.36.2** (MIT OR Apache-2.0), pinned in the root `[workspace.dependencies]`, in the crate `signal-app` only. The engine, graph and driver crates never depend on it.
- Immediate mode suits meters and parameter panels that redraw from engine snapshots. Signal state is never kept in the widgets: the GUI reads observation snapshots (meters, parameter values) through lock-free queues, and sends edit commands that are written back to project files with `toml_edit` (ADR 0009, 0013).
- The GUI thread never blocks the audio thread. Meter data comes from the audio thread as fixed-size records over an SPSC queue (`rtrb`), dropped when the queue is full.
- egui's bundled default fonts are under OFL-1.1 and the Ubuntu font licence. These are font licences that allow bundling, and they are recorded here as the exception to ADR 0014's list.

## Alternatives considered
- **iced**: retained mode with an Elm architecture; more code for live meters, and less mature text input.
- **Slint**: declarative and polished, but its licence is GPL or royalty-free with conditions, or commercial, which ADR 0014 rules out without an owner decision.
- **Native AppKit through objc2**: the most native look, but needs a large amount of `unsafe` FFI (AGENTS.md rule 9).

## Consequences
- The look is egui's own, not native AppKit. Native menus and the file dialog may need small additions later.
- Visual patching (GUI c), if it ever comes, would need a node-graph widget on top of egui.
