# Architecture Decision Records

Approved by the product owner on 2026-10-11 unless noted. Origin: `../01-inception-proposal.md` §12.

To change a decision, add a new ADR that supersedes the old one. Do not rewrite history.

| ADR | Title |
|---|---|
| [0001](0001-platform-architecture.md) | Platform architecture: domain-free runtime with primitives and composites |
| [0002](0002-frame-semantics-and-module-contract.md) | Frame-functional semantics and the k-frame module contract |
| [0003](0003-time-model.md) | Time model: the core knows only frames |
| [0004](0004-events-and-parameters.md) | Generic events; parameters are input ports |
| [0005](0005-ports-rates-and-conversion.md) | Ports, rates, and the conversion model |
| [0006](0006-graph-versions-and-live-edits.md) | Versioned graphs and live edits |
| [0007](0007-numerics.md) | Numerics: f64 signals, denormals, pure-Rust math |
| [0008](0008-determinism-and-reproducibility.md) | Determinism and reproducibility |
| [0009](0009-project-format.md) | Project format: TOML plus generated JSON Schema |
| [0010](0010-algorithms-in-rust-no-scripting.md) | Algorithms are compiled Rust; no scripting or DSL yet |
| [0011](0011-audio-io.md) | Audio I/O via cpal behind a driver trait |
| [0012](0012-offline-first.md) | Offline-first delivery |
| [0013](0013-application-and-gui.md) | Signal.app and GUI architecture |
| [0014](0014-licence.md) | Licence: provisional, all rights reserved (superseded by 0031) |
| [0015](0015-workspace-and-lab-tier.md) | Workspace layout and the lab tier |
| [0016](0016-execution-contract.md) | Execution contract: scheduling, feedback, same-frame ordering  |
| [0017](0017-unsafe-exception-counting-allocator.md) | Unsafe exception: the counting allocator |
| [0018](0018-gui-toolkit.md) | GUI toolkit: egui through eframe (superseded by 0023, M2.8-4) |
| [0019](0019-multi-core-and-gpu-execution.md) | Multi-core and GPU execution (decides D-EXEC-3) |
| [0020](0020-unsafe-exception-disjoint-plan-state.md) | Unsafe exception: disjoint access to a plan's state |
| [0022](0022-output-adapters.md) | Output adapters: sinks generic in the engine, audio policy in `signal-output` |
| [0023](0023-web-ui.md) | A web-based user interface (supersedes 0018) |
| [0024](0024-restricted-inputs.md) | Restricted inputs (narrows 0016 §4 for the inputs a library restricts) |
| [0025](0025-project-files.md) | Project files: a project folder as one `.signal` zip, opened, saved and saved as in Signal.app |
| [0026](0026-schematic-and-panel-editor.md) | The signal schematic (`graph`, infinite-zoom SVG, layout in `ui/schematic.toml`) and the panel layout editor (M2.9) |
| [0027](0027-core-set-and-built-in-library.md) | The core set (complete minimal std primitives), the built-in `lib.*` library of composites, and what stays native (M2.10) |
| [0028](0028-patterns-as-project-text.md) | Patterns as project text: `patterns/*.toml` note and step data, the transport's tick clock, `std.note_seq` and `std.step_seq8`, `pattern.read`/`pattern.write` (M2.11) |
| [0029](0029-agent-interface-mcp.md) | An agent interface: `signal mcp`, the Model Context Protocol over the UI protocol (M2.12) |
| [0030](0030-schematic-path-tracing-and-wire-scaling.md) | Path tracing across composite boundaries, continuous in-place zoom, and wires that scale (M2.13) |
| [0031](0031-licence-gpl-and-first-public-release.md) | Licence GPL-3.0-or-later and the first public release, v0.2.0 (M2.14) |
| [0032](0032-several-windows-over-the-screens.md) | Several windows over the screens: main, schematic and panel windows, panel cards dragged between them, the layout in `ui/windows.toml` per screen configuration |
| [0033](0033-the-transport-as-a-signal.md) | The transport as a signal: `std.playhead`'s position in beats on a wire, sequencers that follow it, `lib.transport`, connected and exclusive inputs, and Signal.app without Play or Stop (M3.1) |
| [0036](0036-joining-the-device-without-a-glitch.md) | Joining the device without a glitch: the device's clock is the run's (no rate switch), a device in use by another app keeps its buffer, the output fades in and out; unsafe exception `device_props.rs` (reads only) |
| [0034](0034-keyboard-shortcuts-and-scrolling.md) | Keyboard shortcuts and scrolling: one keymap as text (`ui/lib/keys.default.js`), the person's overrides in `keys.toml` beside the Setup, Settings and Help ▸ Keyboard Shortcuts, Page keys that scroll the view, and the wheel taken by a control only when it owns it |
| [0035](0035-a-project-as-content.md) | A project as content: a project included as a composite instance (`project = …`), its sinks as taps, a scope root for its seeds, live joins through the added-instance fade, mixing by `lib.mixer`, panel edits into the host's `set` (M3.2) |
| [0037](0037-slots.md) | Slots: an instance with its own editable interface (optional templates `interfaces/<name>.toml`, `lib.voice`, `lib.instrument`, `lib.deck`) whose `load` connects by name; load, unload and replace as live edits; `std.playhead` follows with a wirable `release` (M3.2) |
| [0038](0038-devices-as-modules.md) | Devices as modules: device slots that name a channel layout (mono, stereo, quad, 5.1, 7.1, discrete) and a role, never hardware; the machine's sound system per role in the Setup; layout adapters in the output adapter (ITU-R BS.775 downmix, LFE off by default, no invented surround, machine-configurable, the author's own fold-down preferred) with the conversion always shown; per-project machine overrides in `devices.local.toml` (never packed, git-ignored); every present sound card shown; one clocked device in v1 (aggregate devices for several cards); a card that plays and records is one box; audio input through generic sources, recorded for replay (M4.1) |
| [0039](0039-play-keys-and-space-for-the-transport.md) | The piano roll's keys play (`std.note_seq`'s `audition` input, merged into its voice in canonical order; the UI protocol's `note`, ended when its client goes) and Space plays and stops the project's transport (`transport.toggle` in a new `project` scope below the view; the `run` of every `std.playhead` a top-level control reaches, toggled together) (M2.15) |
| [0040](0040-several-writers-on-one-project.md) | Several writers on one project folder: staging files of each writer's own, a per-project writer lock (`flock` on the folder) around every version check and write, vendoring revalidated before it commits and rolled back never over an edit (`signal_project::transaction`) |
