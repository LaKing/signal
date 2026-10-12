# Signal

A technical and creative platform for developing and interconnecting real-time signal-processing modules (audio, control, analysis, visuals, lighting), built in Rust for macOS on Apple Silicon.

Status: **M1 Core + offline** done (V6 waived), developed through devpanel: an offline renderer (`signal render / modules / schema`), the M1 standard modules and the project schema, and the test project `projects/kick`; no real-time audio or GUI yet.

- Vision: [`docs/00-vision.md`](docs/00-vision.md)
- Architecture: [`docs/architecture.md`](docs/architecture.md) · Decisions: [`docs/adr/`](docs/adr/README.md)
- Roadmap: [`docs/roadmap.md`](docs/roadmap.md)
- Set up this Mac: `./install.sh` (toolchain, tools, dependencies, offline verification)
- For AI agents and contributors: [`AGENTS.md`](AGENTS.md)

## Build the app

```sh
./start.sh                       # release build of Signal.app, then open it
./start.sh projects/kick         # ... with this project
./start.sh --cli projects/kick   # play it in the terminal instead (live reload)
```

Or step by step:

```sh
scripts/bundle-app.sh            # debug; --release for an optimised build
open dist/Signal.app
```

This builds `signal-app` and `signal` offline (`--locked --offline`) in cargo's target directory (or `--target-dir DIR`). It writes `dist/Signal.app` (git-ignored), with the GUI as `Contents/MacOS/Signal` and the CLI as `Contents/Helpers/signal`. The bundle is signed ad hoc with the hardened runtime and the audio-input entitlement, and needs no signing identity. On first start macOS asks for microphone access, which the app uses for input metering. The app opens the Setup's last project, or `--project DIR`.

© 2026. All rights reserved (provisional; see ADR 0014).
