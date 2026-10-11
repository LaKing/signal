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
- **Runtime reporting** (M2): xrun counter, worst-case callback time, fault reports. The runner's status reports clipped samples, late commands and queue overflows ("The runner" below).
- **Soak tests** (M2): ≥ 30 min with scripted live edits.

## The runner

`signal_engine::Runner` is what a driver's audio callback calls; `signal_engine::Controller` feeds it from the control thread (`realtime()` builds both). Drivers implement `signal_engine::Driver` (ADR 0011): CoreAudio in `signal-audio`, and `NullDriver`, which draws callback sizes from 1 to 4096 from a seeded stream and runs synchronously or on its own thread.

**Order of operations in `Runner::process(out, channels)`**, per callback:

1. **Commands.** Take every queued command in send order. Each has a **requested frame**, its stamp. Its **effective frame** is the frame it applies from:
   - an event: the latest of its stamp, the next frame to run, and the last event's effective frame;
   - a suspend or resume: the same over suspends and resumes;
   - a swap: the latest of its stamp, the next frame, and one past the last swap's frame (one swap per frame).

   These are the engine's own rules (`semantics.md` §12). Events, suspends and resumes go into the engine's queues; swaps wait for their frame.
2. **Frames, one at a time.** Commit the swap whose effective frame has come, run the frame, and write the audio sink's channels (each times the sink's `gain`, as the offline driver records them) to the same device channels, interleaved, as f32. Device channels beyond the sink's are 0; sink channels beyond the device's are not played. Each value is **clamped to [−1, 1]** before the conversion, and every clamped (or non-finite, written as 0) sample is counted in `clipped`. Then push the frame's log records (the engine's log holds one frame).
3. **Meters and status.** Push one meter per sink channel (peak and sum of squares over the callback), send the input the engine has used back to the control thread, and push the status last: the next frame, the plan version and every counter.

**Latency.** A command stamped `Stamp::Default` is requested for the current frame, as last reported, plus the latency: two device buffers by default (`RealtimeConfig::latency_frames`, `2 × buffer_frames`). A command whose requested frame has already run is **late**: it applies on the next frame that runs. Its report carries both frames, and the runner counts it in `late`. The engine's log (`log.jsonl`) carries only effective frames, so a live log equals its replay.

**Queues.** Only `rtrb` SPSC queues cross threads:

| Queue | Direction | When full |
|---|---|---|
| commands | control → audio | `Controller` refuses the send, hands the command back and counts it (`command_overflows`). |
| returns: reports, used events and addresses, replaced plans | audio → control | Never full: the Controller keeps at most as many commands in flight as the engine's queues and half this queue hold, and refuses further sends as above. Should it fill anyway, the runner holds the item, counts `return_stalls`, and withholds the status until the item is sent; nothing is dropped. |
| log | audio → control | The record is dropped and counted (`log_overflows`). |
| meters | audio → control | The meter is dropped and counted (`meter_overflows`). |
| status | audio → control | Dropped and counted (`status_overflows`); counters are cumulative, so the next status says everything. |

None of them blocks, and none changes the audio: a dropped log record or meter is a report, not an input.

**Dropping.** Nothing that owns heap memory is dropped on the audio thread. Used events and addresses, and a committed swap's box (which then holds the replaced plan), go back on the return queue, and `Controller::poll` drops them on the control thread. `poll` also hands reports, log lines and meters to a `ControlSink` without allocating; the Controller performs no file I/O.

`NullDriver` in threaded mode checks a one-slot `rtrb` stop queue between callbacks and may sleep for a callback's duration after it (`pace`), outside `process`; `stop` joins its thread and hands the Runner back.

Verified by `crates/signal-engine/tests/realtime_no_allocation.rs` (V11) and `realtime.rs`, and by `crates/signal-cli/tests/realtime.rs` (V9, replay of a live session).

## Review checklist (paste into PRs touching real-time code)

- [ ] No allocation, free, lock, syscall, I/O, or logging reachable from `process` / the executor
- [ ] No `unwrap` / `expect` / `panic` / unchecked indexing on the real-time path
- [ ] Worst-case work per frame is bounded and stated
- [ ] State updates are *k*-partition-invariant (contract suite passes)
- [ ] Feedback/decay paths are denormal-safe
- [ ] Randomness comes only from the seeded `ctx` stream
- [ ] Contract suite and benchmarks pass; new costs recorded
