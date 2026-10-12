# ADR 0019: Multi-core and GPU execution

- **Status:** Accepted (operator decision, delegated by the owner, 2026-10-12). Decides D-EXEC-3 (`docs/00-vision.md` §9).
- **Date:** 2026-10-12

## Context
The owner's goal (2026-10-12): Signal should be able to use all CPU and GPU cores.

Measurements (`docs/benchmarks.md`, M5 Max, 6 Super + 12 Performance cores):
- At 96 kHz a frame lasts 10.4 µs. An instance costs 9–57 ns per frame, and the engine's fixed per-frame overhead is about 2 ns. The kick uses 2.5 % of the budget; 14 kick voices use 25 % on one core.
- Handing work to another core and waiting for it costs on the order of a microsecond (wake-up, cache transfer). Doing that every frame would cost more than the work itself.

So cores can only be shared over **blocks** of frames. The engine runs one frame at a time today (D-EXEC-3, deferred "until measurements show a need"). The owner's goal is that need.

The module contract already allows blocks: `process`, `emit` and `absorb` take *k* frames with in-block event offsets, and every primitive passes the contract suite's `partition_invariance` check (ADR 0002).

A GPU is a separate device with its own queue. Submitting work and reading results back takes on the order of 0.1–1 ms, comparable to a whole 128-frame buffer at 96 kHz (1.33 ms). GPU floating-point results are not guaranteed bit-identical across GPU models and drivers.

## Decision

### 1. Block execution (D-EXEC-3: decided)
- The engine runs the plan **one block of *k* frames at a time**. *k* is a runtime performance setting (default: the callback size in real time, capped at 256; 256 offline). It never appears in project files and never changes the output.
- **Semantics do not change.** ADR 0016's per-frame model stays normative: the block engine must produce output **bit-identical** to the per-frame engine for every *k* and every partition of frames into blocks. The goldens and the V-tests are the proof.
- **Feedback.** The plan is condensed into strongly connected components. An acyclic instance runs as one task of *k* frames. A cycle (closed through state-output modules, ADR 0016 §2) runs as one **unit** whose members are interleaved in sub-blocks no longer than the cycle's shortest delay (one frame for `std.unit_delay`). The unit is one task for the scheduler.
- **Exact frames.** Plan swaps, edits and external events keep their exact frames: a block is split at a swap frame, and events are delivered with in-block offsets, as the contract already defines.

### 2. Parallel scheduling on CPU cores
- Each block, the plan's units form a dependency graph. A fixed pool of worker threads, created when the engine starts, runs the units whose inputs are ready. The audio thread itself takes part. There is no allocation, no lock and no blocking system call on these threads during a block (`docs/realtime-rules.md`).
- **Determinism does not depend on threads.** A unit writes only its own output buffers. Wherever several sources meet (signal sums, parameter modulation, event merging), the **consumer** combines them in the canonical source order of ADR 0016 §3, never in arrival order. Random streams stay per instance (ADR 0008). Therefore the output is **bit-identical for any number of threads, any *k* and any timing.** AGENTS.md rule 3 is extended accordingly.
- Small units are grouped into chains, deterministically and from static cost weights, so that each task is worth its hand-off. A plan too small to gain from parallelism runs on the audio thread alone. The worker count is a machine setting (Setup; default: the performance cores minus one), and `--threads` is available on the CLI. It is never a project setting.
- **Real-time workers on macOS** join the audio device's I/O workgroup (`kAudioDevicePropertyIOThreadOSWorkgroup`, `os_workgroup_join`), so that the system schedules them with the audio thread's deadline. Where no workgroup is available they get the Mach time-constraint policy (`thread_policy_set`). Both need FFI: see §4.
- Offline rendering uses the same scheduler with ordinary threads, so renders and replays run faster than real time on every core.

### 3. GPU
- The per-block audio graph does **not** run on the GPU. Its latency per round trip is close to a whole buffer, and its arithmetic is not bit-reproducible across devices.
- The GPU is used through **GPU-backed modules with a declared latency** of *L* ≥ 1 blocks. Such a module submits block *b* and outputs the result of block *b* at block *b* + *L*, exactly, whatever the GPU's speed. Its output depends only on past input, like a state-output module, so its timing is deterministic and it may close feedback loops.
- If a result is not ready when it is due:
  - offline, the engine waits (the render stays exact);
  - in real time, the audio thread never waits. The module's output for that block is a fault (silence or hold, as for the fault guard), and the fault is logged with its frame.
- **Reproducibility class.** A GPU-backed module declares that it is reproducible on the same machine and build, not bit-exact across machines. A render that uses one records this in its manifest. Bit-exact guarantees for projects without GPU modules are unchanged.
- The GPU device lives in a new crate `signal-gpu` using **wgpu** (MIT OR Apache-2.0, Metal backend on macOS, already in the lock through eframe). Visual output (a later milestone) will use the same device.

### 4. Unsafe code
Joining the workgroup and setting the thread policy need FFI. They live in one audited module, `crates/signal-audio/src/rt_thread.rs`, under the same rules as ADR 0017. It may call only `os_workgroup_join` and `os_workgroup_leave` (declared there with `extern "C"`), `thread_policy_set` (`libc`) and the CoreAudio property read (`objc2-core-audio`), and every `unsafe` block carries a `SAFETY:` comment. `unsafe_code` stays denied everywhere else. ADR 0020 adds a third audited module for the scheduler of §2, `crates/signal-engine/src/shared.rs` (disjoint access to a plan's state, and handing a block's tasks to the workers).

## Alternatives considered
- **Parallelism per frame:** impossible within a 10.4 µs frame at 96 kHz (see Context).
- **One project per core (voice-level or project-level parallelism only):** simpler, but it leaves a single large project on one core.
- **Non-deterministic parallel sums (arrival order):** faster to write, but breaks ADR 0008 and replay.
- **The audio graph on the GPU:** latency and reproducibility (see §3).

## Consequences
- `docs/00-vision.md` D-EXEC-3 is decided by this ADR. The roadmap gains **M2.6 Multi-core engine** before M3.
- Every engine test gains a determinism matrix: *k* × thread count × callback size, against the per-frame engine and the goldens.
- `docs/benchmarks.md` gains scaling figures: kick voices within 25 % of the budget at 1, 2, 4… threads.
- A second unsafe module exists (§4), audited like ADR 0017.
- GPU modules introduce a second reproducibility class, which manifests record.
