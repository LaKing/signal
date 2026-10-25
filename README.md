# Signal

Signal is a platform for building and connecting real-time signal-processing modules, in the spirit of Native Instruments Reaktor. Today it works with audio and control signals, with analysis and visuals at the edges, and lighting is on the roadmap. It is written in Rust and runs on macOS on Apple Silicon.

A Signal **project** is a folder of plain text files: TOML for the patch, its composites and its patterns, and HTML for its panels. **Text is the source of truth**: the app, the schematic, the panels and the agent interface all edit those files and hold nothing hidden. A project can be diffed, reviewed and version-controlled like code. The engine is deterministic: the same project and seed render the same audio bit for bit, on any number of threads and with any block size.

## Status

Signal is pre-1.0 and early: version **0.2.0** is its first public pre-release. Development goes milestone by milestone, and each milestone ends with a human verdict ([`docs/roadmap.md`](docs/roadmap.md)).

| Milestone | Status | What it brought |
|---|---|---|
| M0 Foundations | Done | Vision, ADRs, workspace, real-time rules, CI |
| M1 Core + offline | Done (V6 waived in M1; passes on `std.tanh_shaper_os8` since M3) | Module contract, graph, TOML projects, offline renderer, `projects/kick` and its golden render |
| M2 Real time + live edits | Done | CoreAudio, live edits while playing, session recording and bit-exact replay, the core primitive library |
| M2.5 Signal.app shell | Done | The Mac app, interface setup, meters |
| M2.6 Multi-core engine | Done (one criterion open) | Deterministic multi-core scheduler, real-time workers, GPU modules (lab) |
| M2.7 Full hardware use | Done | Measured wake policies, output adapters, multi-GPU placement, both timing gates passed on the device |
| M2.8 Web UI foundations | Delivered; owner's exit check open | Web UI and controls, panels as text, Signal.app on a WebView, `.signal` files, physical modelling |
| M2.9 Schematic and panel editor | Delivered; owner's exit check open | Infinite-zoom schematic, panel layout editor, patching |
| M2.10 Core set and built-in library | Delivered; owner's exit check open | Minimal core primitives, the `lib.*` library of composites |
| M2.11 Studio demo | Done | Patterns as text, piano roll and step grid, `projects/studio` |
| M2.12 Agent interface | Done (owner trial pending) | `signal mcp` |
| M2.13 Schematic path tracing | Done (owner trial ongoing) | Tracing signals across composites, in-place zoom |
| M2.14 First public release | This release | Licence, documents, release script and workflow |

"Delivered" means the work is in and tested, but the roadmap still lists the owner's hands-on exit check as open.

**What works**

- Offline rendering, deterministic to the bit (`signal render`), and real-time audio through CoreAudio (`signal play`).
- Live edits: save a project file while it plays and the change is heard at once. Sessions are recorded and replay bit for bit.
- Signal.app: a native window with a web UI. Interface setup (devices, channels, buffer), transport, meters, project panels, File → Open, Save, Save As and Revert to Saved, `.signal` single-file projects, and several windows over several screens (Window → New Main, Schematic or Panel Window, Reset Layout; panel cards moved between windows from their context menu).
- Panels as project text, with knobs, faders, 2D pads, buttons, meters, scopes, a piano roll, a step grid and a canvas.
- The schematic: zoom into composites, follow a signal through modules and composite boundaries, see live levels, and patch.
- The `std.*` core set and the built-in `lib.*` library of composites: LFOs, envelopes, oscillators, a ladder filter, a tape echo, drum voices and more.
- Patterns as text, edited live, and the studio demo: a Moog-style voice through a tape echo over a synthesised drum machine.
- `signal mcp`: an LLM client can inspect, play, adjust, patch and render a project. Audio is off by default, and output gain is held under a ceiling.

**What does not yet**

- Notarization: the app is signed ad hoc, so the first start needs a Gatekeeper step (below).
- Platforms: no Intel Macs, Windows or Linux.
- Inputs and outputs: no MIDI, OSC or DMX yet (M6).
- Audio: no plugin hosting or export, and no sampler.
- Evaluation: the comparison and listening workflow is M3.
- Known shortfalls: `std.tanh_shaper` aliases more than its target on some tones (`std.tanh_shaper_os8` meets it, at 74 frames of delay). Under heavy multi-core load, the worst audio callback is about 0.4 of the period: within the capacity gate, but above the single-voice target.
- Screens and wording may still change between 0.x versions, and so may the project file format.

## System requirements

- A Mac with **Apple Silicon** (M1 or later).
- **macOS 11 or later.** That is the deployment target the build records in `Info.plist`. Development and testing happen on current macOS releases.
- An audio output for real-time playback. Rendering needs no device.

## Download and first start

1. From the [Releases](../../releases) page, download `Signal-<version>-macos-arm64.zip` and open it. The Finder unzips `Signal.app`; move it to Applications if you like.
2. **Gatekeeper.** Signal is signed ad hoc and not notarized, so macOS refuses a plain double-click the first time. Double-click it once and dismiss the warning, then open **System Settings → Privacy & Security**, click **Open Anyway** next to the note about Signal, and confirm. (On macOS 14 and earlier, **right-click** `Signal.app` → **Open** does the same.) Alternatively, in Terminal:

   ```sh
   xattr -dr com.apple.quarantine Signal.app
   ```

   Later starts need nothing extra. macOS asks once for microphone access, which Signal uses for its input meters.
3. **Open an example.** Download `Signal-<version>-projects.zip` and unzip it. In Signal choose **File → Open…** and pick a project folder such as `projects/studio`.

`signal-<version>-macos-arm64.tar.gz` holds the command-line tool alone, and Signal.app carries the same tool as `Signal.app/Contents/Helpers/signal`. To check the downloads, run `shasum -a 256 -c SHA256SUMS`.

## Build from source

```sh
git clone <this repository> signal && cd signal
./install.sh                     # Homebrew rustup, the pinned toolchain, dependencies; then the checks
./start.sh                       # release build of Signal.app (dist/Signal.app), then open it
./start.sh projects/studio       # ... with this project
./start.sh --cli projects/kick   # play it in the terminal instead (live reload)
```

Step by step, `scripts/bundle-app.sh [--release]` builds `dist/Signal.app`. The GUI is `Contents/MacOS/Signal` and the CLI is `Contents/Helpers/signal`; both are signed with the hardened runtime, ad hoc unless `--sign IDENTITY` (or `SIGNAL_SIGN_IDENTITY`) names a keychain identity. `./start.sh` signs with the keychain's first Apple Development identity when there is one, so macOS keeps the microphone permission from one build to the next; release builds stay ad hoc. `scripts/release.sh --out DIR` builds the release downloads. Builds run offline after `./install.sh`. Cargo's target directory is `/tmp/signal-project-target` (`.cargo/config.toml`); [`CONTRIBUTING.md`](CONTRIBUTING.md) explains why and how to override it.

## The command line

```sh
signal render projects/kick --seconds 10 --out renders/kick   # offline: audio.wav, manifest.json, log.jsonl, metrics.json
signal play projects/pulse --watch                            # real time, with live reload of the files; Enter stops
signal play projects/pulse --watch --record-session renders/s # ... and record the session
signal replay renders/s --out renders/replay                  # render a recorded session again, bit for bit
signal ui projects/studio --open                              # play and serve its panels to a browser (127.0.0.1)
signal mcp projects/studio                                    # MCP server for an LLM client (add --audio to hear it)
signal pack projects/kick kick.signal                         # one .signal file; `signal unpack` reverses it
signal vendor projects/pulse lib.tape_echo                    # copy a lib.* composite into a project as its own text
signal modules                                                # the catalogue of primitives and library composites
signal schema                                                 # JSON Schema of project and composite files
signal devices [--compute]                                    # audio devices, or GPU devices
signal bench                                                  # per-frame engine measurements
```

`signal <command> --help` describes every option, and `--json` gives machine-readable output.

## Example projects

The projects in [`projects/`](projects) are test fixtures and demos. They are also the downloadable `Signal-<version>-projects.zip`.

| Project | What it is |
|---|---|
| `studio` | The studio demo: a piano-roll melody on a three-oscillator ladder-filter voice, through a tape echo, over a step-sequenced, fully synthesised drum machine, with panels styled after the originals |
| `studio-transport` | The studio played from its own transport (`lib.transport`, ADR 0033): its sequencers follow a position in beats on a wire, and a Transport card plays, stops, rewinds and sets the tempo and rate |
| `kick` | The M1 test project: a techno kick at 132 bpm with seeded ghost notes; its render is the first golden |
| `pulse` | A gliding saw sequence through an enveloped low-pass filter and a feedback delay |
| `physical` | Physically modelled sound: a plucked string, a struck bell, a mass-spring drum and a bowed string |
| `physical-av` | A struck membrane heard and seen, with sparks and bodies on a canvas panel |
| `library-demo` | Every composite of the built-in `lib.*` library in one patch |
| `gpu-partials` | A lab fixture: 1024 GPU-computed partials. It needs a `--features lab` build and is not a product |

## Documentation

- [`docs/00-vision.md`](docs/00-vision.md): the vision and terminology. [`docs/architecture.md`](docs/architecture.md): how the crates fit together.
- [`docs/project-format.md`](docs/project-format.md): the project files. [`docs/semantics.md`](docs/semantics.md): what a patch means, frame by frame.
- [`docs/ui-protocol.md`](docs/ui-protocol.md), [`docs/ui-controls.md`](docs/ui-controls.md): the UI protocol and the panel controls.
- [`docs/agent-interface.md`](docs/agent-interface.md): `signal mcp`.
- [`docs/module-contract.md`](docs/module-contract.md), [`docs/realtime-rules.md`](docs/realtime-rules.md): writing primitives.
- [`docs/testing-and-evaluation.md`](docs/testing-and-evaluation.md), [`docs/benchmarks.md`](docs/benchmarks.md): tests, goldens and measurements.
- [`docs/adr/`](docs/adr/README.md): every design decision. [`docs/roadmap.md`](docs/roadmap.md): milestones and the owner's decisions.
- [`CHANGELOG.md`](CHANGELOG.md), [`CONTRIBUTING.md`](CONTRIBUTING.md), [`SECURITY.md`](SECURITY.md).

## How Signal is developed

Signal is built mostly by AI agents, and the repository keeps the record of how.

- **devpanel.** Work arrives as issues and runs through devpanel, a self-hosted panel that drives coding agents (Claude Code and Codex). Each issue gets a coordinator, an author and independent auditors. They work in an isolated git worktree with offline builds, and a change is delivered only after the auditors approve the same revision and its checks pass. [`AGENTS.md`](AGENTS.md) is the guide those agents read.
- **An operator agent** turns the owner's requests into issues, plans milestones, writes most ADRs under the owner's delegation, and runs device measurements and reviews.
- **The owner decides.** Direction, licence, and anything that touches how Signal sounds or feels are the owner's calls. Those decisions are recorded with the owner's own words: in the ADRs ([`docs/adr/`](docs/adr/README.md)), in the roadmap's "Owner decisions", and as listening verdicts in [`experiments/`](experiments) (for example `experiments/m2-11-studio/verdict.md`). Metrics inform a verdict but never replace it.
- **Decision IDs.** Identifiers such as `sq-22037848` in the documents are devpanel question IDs: a question an agent asked the owner, and the answer that settled it. `operator-request:NNN` marks an operator's decision within a run.
- **History as it happened.** The commit history, the ADRs and the experiment records are kept unchanged (ADR 0031). They show what was tried, measured and decided along the way.

## Licence

Signal is free software. It is licensed under the **GNU General Public License, version 3 or (at your option) any later version** (`GPL-3.0-or-later`; [`LICENSE`](LICENSE), ADR 0031).

The crates Signal links are under permissive licences (MIT, Apache-2.0, BSD, ISC, Zlib, Unicode), and one is under MPL-2.0. [`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md) lists each crate with its licence and reproduces their licence texts. It is generated from `Cargo.lock` by `scripts/third-party-notices.mjs` and ships inside every download.
