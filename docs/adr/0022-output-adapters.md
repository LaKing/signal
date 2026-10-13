# ADR 0022: Output adapters

- **Status:** Accepted (M2.7-9, the owner's issue after the external re-review of 2026-10-13). Amends ADR 0011: the driver trait moves out of `signal-engine`.
- **Date:** 2026-10-13

## Context
AGENTS.md rule 1 keeps domain concepts out of `signal-core`, `signal-graph` and `signal-engine`. The re-review found audio output policy in the engine: the real-time `Runner` and the offline driver chose the audio sink, applied the specially named `gain` parameter, mapped the sink's inputs to device channels, converted f64 to the device's f32 (clamping and counting clipped samples), metered the channels and wrote `audio.wav`. Network control, video and other outputs are coming; each would have added its own policy to the engine.

The offline WAV writer is reached from `signal-project` (replay) and the device path from `signal-audio`; one adapter must serve both. `signal-project` depends only on `signal-core`, `signal-graph` and `signal-engine`, and `signal-audio` links cpal and the CoreAudio bindings, which replay has no use for.

## Decision
- **The engine knows sinks generically.** A sink is any instance whose descriptor declares a `SinkKind`; the engine attaches no meaning to the kind. `Engine::sinks()` lists them (instance, kind, address, signal inputs) without allocating; `Engine::sink_input_at` reads a sink's summed inputs in the last block, and `Engine::parameter_index` / `parameter_at_frame` its parameters' effective values. Both drivers hand each block to a `SinkOutput` (`bind` before the first block and after every swap, `block` after each block): the offline driver `run_offline`, and `Runner::process(frames, output)` in real time. A live swap must keep the run's sinks (kind, address, inputs; `SinksChanged`).
- **Output policy lives in adapters,** in the crate `signal-output`, which depends on `signal-core`, `signal-graph`, `signal-engine`, `hound` and `rtrb` (no device library). Its audio adapter chooses the audio sink (the plan's one `SinkKind::Audio` instance, with at least one signal input), applies the sink's `gain` parameter if its type declares one, maps sink input *i* to device channel *i* (extra device channels 0), converts with `device_sample` (clamped to [−1, 1], non-finite as 0, counted as clipped) and meters each channel per callback.
- **One adapter, both paths.** The offline render (`render`, `render_with`, `render_blocks`, `Render::write`, `Conversion`) and the real-time audio `Runner` and `Controller` (wrapping the engine's, to which they dereference) are the same adapter. The `Driver` trait and the `NullDriver` move with it (amending ADR 0011's placement); `signal-audio`'s CoreAudio driver implements it. `signal-project` (replay), `signal-cli` and `signal-app` depend on `signal-output`.
- **Names kept.** The moved items keep their names and signatures, so callers changed imports and, where counts moved (clipped samples, dropped meters: `Runner::clipped`, `Controller::clipped`, `meter_overflows`; meters through `Controller::poll_with`), call shapes. No output changes: the expressions are the same (`gain * x`, the clamp and its count, the zeroed tail).
- **Not moved:** port kinds (`PortKind::Audio`, which the engine's fades use) and the render manifest (`Manifest`, whose `audio_fnv1a64` and `device_conversion` name the artifact files; the adapter supplies their values). They are not output policy.
- **Checked** by `crates/signal-engine/tests/domain_scope.rs` (no `gain`, `audio_out`, channel, `device_sample`, `hound` or audio-sink name in the engine's code or dependencies) and `crates/signal-cli/tests/output_identity.rs` (render, a gain ramp into clipping, the null driver with 2 and 3 device channels and the replay of a live run, hashed and pinned against the revision before the move).

## Consequences
- A new kind of output (video, network control) is a new adapter beside the audio one, and the engine does not change.
- `Cargo.lock` gains the path crate `signal-output` and its edges; no registry crate is added, removed or re-versioned (operator decision on AC8, 2026-10-13).
- `signal-engine`'s integration tests depend on `signal-output` as a dev-dependency (a dev-dependency cycle Cargo allows for integration tests); its unit tests use a local `SinkOutput`.
