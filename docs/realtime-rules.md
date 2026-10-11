# Real-time rules

**Normative.** These rules apply to all code that can run on the audio thread: the engine's per-frame executor, plan swapping, and every primitive's `process`. Review checklist at the end.

Why: an audio callback has a hard deadline (128 frames at 96 kHz is 1.33 ms). Missing it is an audible dropout. Most dropouts come from operations whose worst-case time is unbounded, even when their average is fast.

## Forbidden on the audio thread

| Never | Because | Do instead |
|---|---|---|
| Allocate or free memory (`Box::new`, `Vec::push` beyond capacity, `String`, `format!`, `clone()` of heap types, dropping heap types) | The allocator may lock or call the OS | Allocate in `prepare`. Hand drops to the control thread via the return queue. |
| Take a lock (`Mutex`, `RwLock`, `std::sync::mpsc`, `println!`, logging macros) | Priority inversion: the audio thread waits on a lower-priority thread | Lock-free SPSC queues (rtrb); atomics |
| Make syscalls or do I/O (files, network, `std::time::SystemTime`, thread spawn, sleeping) | Unbounded latency | Do it on the control thread and send results as events |
| `panic!`, `unwrap()`, `expect()`, unchecked indexing that can fail at runtime | Unwinding allocates; an abort ends the session | Handle with defaults or a fault flag. Validate in `prepare`. |
| Unbounded loops or recursion, or work dependent on input data size | No worst-case bound | Fixed iteration counts; bounded buffers |
| Read wall-clock time for semantics | Breaks determinism | Use the frame counter |
| Use randomness not derived from the instance's seeded stream | Breaks determinism | `ctx` RNG stream |

## Required

- **Bounded work per frame.** Document the worst case for any primitive whose cost varies.
- **Finite output.** NaN/Inf or absurd magnitudes trigger the engine's fault guard (fade, mute, report). Primitives should not rely on that guard.
- **Denormal safety** per `dsp-conventions.md` (M1): flush decaying state in feedback paths.
- ***k*-partition invariance** (ADR 0002): results must not depend on how frames are grouped into calls.
- **No semantics tied to the callback.** Control-rate work ticks on an absolute frame grid, never "once per callback".

## How it is enforced

- **Contract suite** (M1), run on every primitive in `crates/` and `lab/`:
  - allocation-counting global allocator during `process`;
  - random *k*-partitions compared bit for bit;
  - fuzzed inputs must give finite outputs.
- **Lints:** real-time crates deny `clippy::unwrap_used`, `clippy::expect_used`, `clippy::panic`, `clippy::indexing_slicing` (configured per crate when it is created).
- **Runtime reporting** (M2): xrun counter, worst-case callback time, fault reports.
- **Soak tests** (M2): ≥ 30 min with scripted live edits.

## Review checklist (paste into PRs touching real-time code)

- [ ] No allocation, free, lock, syscall, I/O, or logging reachable from `process` / the executor
- [ ] No `unwrap` / `expect` / `panic` / unchecked indexing on the real-time path
- [ ] Worst-case work per frame is bounded and stated
- [ ] State updates are *k*-partition-invariant (contract suite passes)
- [ ] Feedback/decay paths are denormal-safe
- [ ] Randomness comes only from the seeded `ctx` stream
- [ ] Contract suite and benchmarks pass; new costs recorded
