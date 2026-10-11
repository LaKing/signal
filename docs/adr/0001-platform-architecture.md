# ADR 0001: Platform architecture: domain-free runtime with primitives and composites

- **Status:** Accepted
- **Date:** 2026-10-11

## Context
Signal is one macOS application that is internally a platform. Applications are *projects* built from modules. Five architectures were compared (inception proposal §4): monolith, own runtime, building on fundsp/Firewheel/web-audio-api, process split, and language-centric.

## Decision
- Build our own small, **domain-free** runtime (alternative B).
- **Primitives** are Rust modules compiled into the Signal binary (Q15b). Adding one means rebuilding Signal.
- **Composites** are text-defined modules made of modules, nestable without limit, loadable live.
- Polyphony is a module concern; the core has no voice concept (Q19).
- The core knows frames, ports, events, and addresses only. No tempo, MIDI, DMX, or video concepts.

## Alternatives considered
- Building on Firewheel or fundsp: faster start, but neither has frame-functional semantics, hierarchical text composites, or edit-logged replay. They remain references and test oracles.
- Process split (SuperCollider model): kept possible because the command/edit log is serializable, but not built.
- Runtime-loaded code packages (dylib/WASM): rejected for now.

## Consequences
- We own the graph compiler, plan swapping, and fault handling.
- Domain libraries (music, measurement, lighting) live outside the core.
- Reviews must reject domain concepts entering `signal-core` (risk R15).
