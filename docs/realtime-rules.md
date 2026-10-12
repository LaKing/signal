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
2. **Frames, in blocks.** Commit the swap whose effective frame has come, then run a block of up to the engine's block capacity (`EngineConfig::block_capacity`: the device buffer, at most 256, set by `RealtimeConfig::with_buffer`), ending before the next swap's frame and before the next command's effective frame (`Engine::next_command_frame`; `semantics.md` §13). For each frame of the block, write the audio sink's channels (each times the sink's `gain` in that frame, as the offline driver records them) to the same device channels, interleaved, as f32. Device channels beyond the sink's are 0; sink channels beyond the device's are not played. Each value is **clamped to [−1, 1]** before the conversion, and every clamped (or non-finite, written as 0) sample is counted in `clipped`. Then push the block's log records, frame by frame (the engine's log holds one block). The output is bit-identical whatever the blocks.
3. **Meters, parameters and status.** Push one meter per sink channel (peak and sum of squares over the callback), the base and effective value of up to `PARAMETERS_PER_CALLBACK` (64) parameters, going round the plan's parameters from callback to callback, send the input the engine has used back to the control thread, and push the status last: the next frame, the plan version and every counter.

**Latency.** A command stamped `Stamp::Default` is requested for the current frame, as last reported, plus the latency: two device buffers by default (`RealtimeConfig::latency_frames`, `2 × buffer_frames`). A command whose requested frame has already run is **late**: it applies on the next frame that runs. Its report carries both frames, and the runner counts it in `late`. The engine's log (`log.jsonl`) carries only effective frames, so a live log equals its replay.

**Queues.** Only `rtrb` SPSC queues cross threads:

| Queue | Direction | When full |
|---|---|---|
| commands | control → audio | `Controller` refuses the send, hands the command back and counts it (`command_overflows`). |
| returns: reports, used events and addresses, replaced plans | audio → control | Never full: the Controller keeps at most as many commands in flight as the engine's queues and half this queue hold, and refuses further sends as above. Should it fill anyway, the runner holds the item, counts `return_stalls`, and withholds the status until the item is sent; nothing is dropped. |
| log | audio → control | The record is dropped and counted (`log_overflows`). |
| meters | audio → control | The meter is dropped and counted (`meter_overflows`). |
| parameter values (observation only) | audio → control | The value is dropped and counted (`parameter_overflows`); the next round reports it again. |
| status | audio → control | Dropped and counted (`status_overflows`); counters are cumulative, so the next status says everything. |

None of them blocks, and none changes the audio: a dropped log record or meter is a report, not an input.

**Dropping.** Nothing that owns heap memory is dropped on the audio thread. Used events and addresses, and a committed swap's box (which then holds the replaced plan), go back on the return queue, and `Controller::poll` drops them on the control thread. `poll` also hands reports, log lines, meters and parameter values (read with the plan that wrote them) to a `ControlSink` without allocating; the Controller performs no file I/O.

`NullDriver` in threaded mode checks a one-slot `rtrb` stop queue between callbacks and may sleep for a callback's duration after it (`pace`), outside `process`; `stop` joins its thread and hands the Runner back.

`CoreAudioDriver` (`signal-audio`, M2-4) runs the Runner in cpal's output callback through `OutputCallback::fill`: the Runner writes the sink's channels into a scratch buffer allocated at `start` (a device buffer larger than it is run in pieces), and `ChannelMap` copies sink channel *i* to device channel *i*, other device channels 0. `stop` pushes a token on a one-slot `rtrb` queue; the callback then moves the Runner into a one-slot return queue (if that push fails it keeps the Runner and retries, never dropping it on the audio thread) and plays silence, and `stop` drops the stream on the control thread. Each callback is timed with `Instant` into `CallbackStats`, atomics only, for statistics only: xruns are the stream's errors (CoreAudio overloads among them) plus callbacks whose duration exceeded their period. `signal play` polls the Controller at least once per buffer period, and right before sending a file edit, so `Stamp::Default` (two buffers ahead) is normally still ahead of the runner.

Verified by `crates/signal-engine/tests/realtime_no_allocation.rs` (V11), `blocks_no_allocation.rs` (callbacks larger than the block capacity) and `realtime.rs`, by `crates/signal-cli/tests/realtime.rs` (V9, replay of a live session), and for the CoreAudio callback body by `crates/signal-audio/tests/no_allocation.rs` and `callback.rs`.

## Worker threads

The engine runs a block's units on several threads (ADR 0019 §2, M2.6-2): `EngineConfig::threads` threads, the calling thread (the audio callback, or the offline driver) included. 1 runs no worker threads; 0 is one thread per performance core, read once per process (`signal_engine::performance_cores`). The `threads − 1` workers are ordinary threads, created with the engine on the control thread and kept across plan swaps; they are joined when the engine is dropped, which happens on the control thread (the Runner comes back there). Real-time priority and the audio workgroup are M2.6-3.

**Which segments run on the workers.** A segment (the frames between two edit frames of a block) runs on the calling thread alone unless the plan has two tasks or more, the segment has at least `min_frames` frames (16) and the plan's estimated cost times the segment's frames reaches `parallel_min_ns`, and every task's log has room for the segment's staged external events. So a *k* = 1 block never goes to the workers. A segment on the calling thread alone indexes the plan's buffers directly, as M2.6-1 did, without the views a parallel segment's tasks use (ADR 0020). On the seat, in one session alternating with the base build, the kick at *k* = 1 on one thread measured 375.5 and 369.1 ns per frame against the base's 379.5 and 377.8 (`docs/benchmarks.md` "Threads (M2.6-2)"); the operator re-measures outside the sandbox. The thresholds and the cost table are in `docs/benchmarks.md`; they change the timing, never the output.

**A parallel segment.** The calling thread publishes the segment's job (`crates/signal-engine/src/shared.rs`, ADR 0020), wakes at most one parked worker, and takes ready tasks itself, as the workers do: a task is ready when its dependency counter reaches 0. It returns when every task is complete and every worker that joined the job has left it. Nothing in a segment allocates, frees, locks or blocks: claiming a task, completing it and joining or leaving the job are atomic operations on memory sized when the plan was built.

**Waiting policy and its cost.**
- An idle worker spins on the job gate for about 50 µs after its last job, then parks (`std::thread::park`). Offline renders run blocks back to back, so their workers rarely park; in real time with a 128-frame buffer (1.33 ms at 96 kHz) they park between callbacks.
- The calling thread wakes at most one parked worker per parallel segment, and each worker that joins wakes up to two more (a tree: depth about log₂ of the workers). Waking is `Thread::unpark`: an atomic swap, and `dispatch_semaphore_signal` only when the worker is parked. It neither allocates nor blocks. It is the one system call a parallel segment can make on the calling thread, at most once per segment.
- The calling thread never waits for a wake: it runs ready tasks itself. Measured on the seat (planner probe, M2.6-2 plan): `unpark` costs the caller a median 1.7 µs (p99 5.0 µs); a parked worker starts running a median 10 µs (p99 44 µs, worst seen 1.6 ms) after its wake, so the *n*-th worker of the tree arrives after about log₂ *n* such hops. In real time the caller may therefore run much of a short block alone, until M2.6-3.
- The handshake loses no wake. A parking worker stores its `parked` flag and then re-reads the gate; the caller opens the gate and then reads the flags. All four operations are SeqCst, so in their single order either the worker sees the open gate and does not park, or the caller sees the flag and wakes it. An `unpark` that comes before the `park` is kept by the thread, so it is not lost either.
- The calling thread waits for a task a worker has claimed, and for every joined worker to leave the job before it returns. A worker preempted at that moment delays the block. With ordinary-priority workers this is bounded only by the OS scheduler: it is the real-time risk of this milestone, and the reason small plans and *k* = 1 stay on the calling thread.

**A worker that panics** (only a contract-breaking primitive or a test hook can make one) completes its task and leaves the job through drop guards, so the calling thread never hangs; the task's completion guard exists from its claim on, before the test hook runs. The worker is not replaced: the pool runs one worker short until the engine is rebuilt, and `schedule_stats().worker_panics` counts it. If the calling thread panics inside a parallel segment, a drop guard closes the job and waits for every joined worker to leave before the unwinding goes on, so no worker ever holds a reference to the segment's job after it.

**Determinism.** A task writes only its own instances' rows; every sum and event merge is computed by the consumer in canonical source order; the tasks' logs are joined in task order after the segment. The output and the log are the same bit for bit for every thread count (`docs/semantics.md` §13).

## Review checklist (paste into PRs touching real-time code)

- [ ] No allocation, free, lock, syscall, I/O, or logging reachable from `process` / the executor
- [ ] No `unwrap` / `expect` / `panic` / unchecked indexing on the real-time path
- [ ] Worst-case work per frame is bounded and stated
- [ ] State updates are *k*-partition-invariant (contract suite passes)
- [ ] Feedback/decay paths are denormal-safe
- [ ] Randomness comes only from the seeded `ctx` stream
- [ ] Contract suite and benchmarks pass; new costs recorded
- [ ] `crates/signal-engine/src/shared.rs` (ADR 0020): every `unsafe` block has a `SAFETY:` comment naming its ordering (Release on completion, Acquire before reading; the gate's SeqCst open, AcqRel join, Release leave); shared reads require `T: Sync`; no view escapes its `Access`; the checked-mode tests report no violation
