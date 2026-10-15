# ADR 0023: A web-based user interface

- **Status:** Accepted (owner's direction 2026-10-14: "the UI should be HTML-based, able to bring in SVG and pixel graphics, with the basic elements ready: fader, knob, 2D input, scope, meter, button…"; design by the operator, delegated). **Supersedes ADR 0018** (egui): done in M2.8-4, where Signal.app became a tao window with a wry WebView and egui and eframe left the workspace.
- **Date:** 2026-10-14

## Context
The first GUI (M2.5) uses egui. It works, but it is a programmer's toolkit: custom-looking instruments, SVG and bitmap skins, scopes and visual output are hard there, and the UI cannot be shown on another device. The owner wants an HTML-based UI with a ready set of controls, open to SVG and pixel graphics, and later physically modelled visuals. ADR 0013 still holds: the GUI is a client of the edit and observation interface, and it holds no state that is not in text or the Setup.

## Decision
1. **One UI protocol, served locally.** A new crate, `signal-ui`, runs on the control thread. It serves:
   - static UI files over HTTP (`tiny_http`, bound to 127.0.0.1; a small `std::net` server since M2.8-8, see the amendment below);
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
- The app needs `NSLocalNetworkUsageDescription` only if remote UI clients are enabled. Its WebView loads `http://127.0.0.1`, which the bundle's `Info.plist` allows (`NSAppTransportSecurity` → `NSAllowsLocalNetworking`).
- **M2.8-4, Signal.app on the web UI.** The app's control thread runs the protocol's host in *app mode* (`ui-protocol.md`, "Signal.app"): it starts stopped, plays and stops on request, opens projects, and pushes the status line. Its rules are shared with the app's state (`signal_ui::app`: the project's load, how a run plays from the Setup, the status line, the meter labels, what opening a project and exiting record in the Setup), so the app and its tests run the same code. The bundle carries the UI library in `Contents/Resources/ui`.
- **The app's menu is native, through `muda`** (M2.8-11, the owner's request; until then it was a bar in the page, since tao has no menu bar and a hand-built `NSMenu` needs `unsafe` outside the audited modules, AGENTS.md rule 9). The operator locked `muda` 0.21.2 (MIT or Apache-2.0, default features off; it brings `keyboard-types`, `png`, `flate2`, `miniz_oxide`, `zlib-rs`, `crc32fast`, `fdeflate`, `adler2`, `simd-adler32`, all MIT, Apache-2.0, Zlib or 0BSD). The menu bar (`crates/signal-app/src/window.rs`, as data; built in `main.rs`) has:
  - the app menu: About, Hide, Hide Others, Show All, Quit;
  - **File:** Open Project… (Cmd-O), Save Project (Cmd-S), Interface Setup… (Cmd-comma), Quit (Cmd-Q);
  - **Edit:** Undo, Redo, Cut, Copy, Paste, Select All, `muda`'s predefined items, so the page's text fields get the system's shortcuts. This closes the earlier note that they might not.

  Quit closes the window as its close button does (the Setup records its size). Open, Save and Interface Setup are handed to the page as a `signal-menu` event: Save writes every unsaved parameter again (the existing `retry`) and reports "saved" or the errors; Interface Setup shows the window's Setup tab. **Open Project shows the system's folder chooser** (`NSOpenPanel` through `objc2-app-kit`'s safe API, modal): a project folder, or its `project.toml`, which opens its folder (the owner first chose the file); a choice opens as a drop does (a dropped `project.toml` opens its folder too), and a cancel does nothing. The delivery first used the Setup tab's folder field instead, because `signal-app` must depend on `objc2`, `objc2-app-kit` and `objc2-foundation` directly; the operator added those dependencies (2026-10-15, after M2.8-11): `Cargo.lock` gains three dependency lines in `signal-app`'s entry and no package. In a browser, Open Project still shows the Setup tab with its folder field focused (a folder can also be dropped on the window). The WebView tells the page that the native menu is there (`window.signalApp.menu`), and the page then leaves the shortcuts to it. In a browser (`signal ui`, or the app's URL opened elsewhere) there is no native menu: the page keeps its toolbar and takes Cmd-O, Cmd-S and Cmd-comma itself. The in-page menu bar and the IPC Quit are gone.

*Amended 2026-10-15 (M2.8-8, the owner's issue after the external review of 63d4353; recorded by the run as its approved plan, the operator is informed):* **the HTTP server is a small `std::net` server, and `tiny_http` left the workspace.** A client that stopped reading a large asset blocked every other request and the server's shutdown: `tiny_http` 0.12 sets no socket timeouts and gives no access to its streams (so no write deadline can be set), starts a thread per connection when none is idle, and `Request::respond` writes until the client has read everything. `crates/signal-ui/src/http.rs` serves the same files under the same rules with a fixed pool of 4 workers, a 2 s deadline for a request head, a 3 s deadline for a client that takes nothing, files streamed in chunks, and a stop flag polled every 100 ms, so dropping the server returns within a few polls (`ui-protocol.md` "Servers"). The decision above is unchanged: static files over HTTP on 127.0.0.1 by default, beside the WebSocket. `Cargo.lock` changed only by removals: `tiny_http` 0.12.0, `ascii` 1.1.0, `chunked_transfer` 1.5.0 and `httpdate` 1.0.3; nothing was added or re-versioned.
