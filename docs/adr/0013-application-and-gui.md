# ADR 0013: Signal.app and GUI architecture

- **Status:** Accepted
- **Date:** 2026-10-11

## Context
Signal is a macOS application with a GUI by default (Q22) and needs its own permissions (microphone, network). Agents and CI need headless operation. The first GUI jobs are interface setup and metering (Q23).

## Decision
- Ship as **Signal.app**. The engine and the GUI run in one process. A headless `signal` CLI ships inside the bundle for agents, CI, and offline rendering.
- The GUI is a **client of the same edit-command and observation interface** as file watching and the CLI. GUI edits are written back to project files via `toml_edit`. The GUI never holds state that is not in text or Setup.
- GUI order:
  1. interface setup dialog (devices, I/O, sound card), then metering;
  2. control panels;
  3. visual patching, a nice-to-have. Manual wiring scales badly for big projects, so text plus automatic conversion stays primary.
- The GUI toolkit is decided by its own ADR when the GUI milestone starts. egui is the leading candidate (researched 2026-10-11). *Decided in ADR 0018: egui through eframe; superseded by ADR 0023 (a web UI in a tao window with a wry WebView) in M2.8-4.*

## Alternatives considered
A separate GUI process talking to an engine process: better isolation, more IPC complexity.

## Consequences
- App bundling, code signing, and permission entitlements arrive with the GUI milestone.
- Until then, development uses the CLI.
