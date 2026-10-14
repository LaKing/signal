# ADR 0023: A web-based user interface

- **Status:** Accepted (owner's direction 2026-10-14: "the UI should be HTML-based, able to bring in SVG and pixel graphics, with the basic elements ready: fader, knob, 2D input, scope, meter, button…"; design by the operator, delegated). **Supersedes ADR 0018** (egui) once the web UI reaches parity (M2.8-4).
- **Date:** 2026-10-14

## Context
The first GUI (M2.5) uses egui. It works, but it is a programmer's toolkit: custom-looking instruments, SVG and bitmap skins, scopes and visual output are hard there, and the UI cannot be shown on another device. The owner wants an HTML-based UI with a ready set of controls, open to SVG and pixel graphics, and later physically modelled visuals. ADR 0013 still holds: the GUI is a client of the edit and observation interface, and it holds no state that is not in text or the Setup.

## Decision
1. **One UI protocol, served locally.** A new crate, `signal-ui`, runs on the control thread. It serves:
   - static UI files over HTTP (`tiny_http`, bound to 127.0.0.1);
   - a WebSocket (`tungstenite`) carrying the UI protocol:
     - **describe:** instances, ports, parameters with units, ranges, defaults and combine;
     - **subscribe:** parameter values, meters and scope streams at a chosen display rate, sent as binary frames;
     - **set / ramp:** live edits, through the same `Controller` path as the parameter panel;
     - **commit:** write a value back to the project file with `toml_edit`;
     - **setup:** devices and the Setup file.
   - Every connection needs a per-session token. Remote clients (a tablet, another Mac; ADR 0021) are off by default and need an explicit Setup switch.
2. **Signal.app hosts a WebView.** `signal-app` becomes a `tao` window with a `wry` WKWebView that loads the local server. `signal ui <project>` serves the same UI to any browser, for development, agents and other devices. egui stays until the web UI has every M2.5 function, then it is removed (M2.8-4).
3. **Controls are web components in plain ES modules,** with no build step and no npm dependencies (seats build offline):
   - `<signal-fader>`, `<signal-knob>`, `<signal-xy>` (2D input), `<signal-button>`/`<signal-toggle>`, `<signal-meter>`, `<signal-scope>` (time and X–Y), `<signal-value>`.
   - Each renders as **SVG** by default, and takes **skins**: an SVG part set or a pixel **filmstrip or sprite sheet** (PNG), through attributes.
   - Styling uses CSS custom properties.
   - Value mapping, gestures and ballistics are pure functions with `node:test` tests.
4. **Panels are text in the project.** Panels are HTML files in the project's `ui/` folder, with assets in `ui/assets/`. Controls bind to ports by address (`<signal-knob bind="/drums/kick_decay">`). `signal ui check` validates every binding against the plan. A project without panels gets a generated default panel. Moving a control commits the value to the project's TOML, never to the HTML. Editing the panel layout from the GUI comes later.
5. **Visuals and physical models live in the engine; the UI only draws.** Physically modelled sound and visual systems (strings, modal resonators, mass-spring meshes, particles) are engine modules, so they stay deterministic, recordable and replayable. Visual-rate modules use rate divisors and vector ports (pulled forward from M4). Their state streams to UI renderers (`<signal-canvas>`: Canvas 2D or WebGL) as binary frames. The UI never simulates anything Signal must reproduce.

## Alternatives considered
- **Keep egui:** fast to build, but poor for skinned instruments, SVG and pixel assets, and remote UIs.
- **A JS framework (React, Svelte) with a bundler:** needs npm and a build step, which the offline seats cannot run; web components cover the need.
- **Tauri:** brings its own app structure and IPC; wry is the part we need.

## Consequences
- New locked crates: `wry`, `tao`, `tungstenite`, `tiny_http` (all MIT or Apache-2.0). **MPL-2.0 note (ADR 0014):** on macOS one MPL-2.0 crate is linked, `option-ext` (through `dirs`, which wry uses); `cssparser`, `selectors` and `dtoa-short` (MPL-2.0) are locked only for the Linux WebKitGTK backend and are not compiled for macOS. MPL-2.0 is file-level copyleft; Signal does not modify these files.
- The UI has two clients, WebView and browser, and one protocol. UI tests are split: protocol tests in Rust, control logic in `node:test`, and visual checks by the operator.
- The app needs `NSLocalNetworkUsageDescription` only if remote UI clients are enabled.
