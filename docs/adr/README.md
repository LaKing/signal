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
| [0014](0014-licence.md) | Licence: provisional, all rights reserved |
| [0015](0015-workspace-and-lab-tier.md) | Workspace layout and the lab tier |
| [0016](0016-execution-contract.md) | Execution contract: scheduling, feedback, same-frame ordering  |
