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

The engine runs a block's units on several threads (ADR 0019 §2, M2.6-2): `EngineConfig::threads` threads, the calling thread (the audio callback, or the offline driver) included. 1 runs no worker threads; 0 is auto (below). The `threads − 1` workers are created with the engine on the control thread and kept across plan swaps; they are joined when the engine is dropped, which happens on the control thread (the Runner comes back there). They are ordinary threads, except while a CoreAudio stream plays (below, "Real-time workers").

**Auto (`threads = 0`).** Offline it is every performance core, read once per process (`signal_engine::performance_cores`). In real time (`realtime()`, so `signal play` and the app) it is **two thirds of the performance cores, rounded down, at least 1** (`signal_engine::realtime_threads`; 12 of 18 on the M5 Max), the rest left to the control thread, the GUI and the system. *Revised by the operator at the M2.6-3 checkpoint (2026-10-12), from the first rule below (P − 1), on the device measurement in `docs/benchmarks.md` "Real-time workers on the device": with 120 kick voices on the SSL 2+ (96 kHz, 128 frames), 18 threads gave 2 xruns and a worst callback of 1.86 periods, 17 (P − 1) gave 0 xruns but 0.77, and 12 gave 0 xruns and 0.53.* The reasoning for leaving cores free, which still holds: the audio thread waits for every worker that joined a block's job before the callback returns, so a joined worker that the system preempts to run something else delays the whole block; with every performance core busy in real time, that something else (the control thread polling the Controller, the GUI, the system's own threads) has nowhere to go. The cost is at most one core in P: on the operator's 5cb390c scaling, 107 kick voices within 25 % of the budget on 8 threads and 175 on 18. The evidence: the seat's sandboxed `signal bench --realtime` table (`docs/benchmarks.md` "Real-time workers (M2.6-3)") measured P − 1 = 17 threads no worse than P = 18 in p99 per block, with real-time scheduling (410–418 µs against 395–417 µs) and without (726–1 176 µs against 1 096–1 777 µs). The operator re-measures it outside the sandbox at the checkpoint; if P − 1 is clearly worse there, the rule is revised (ADR 0019 §2, amended 2026-10-12, operator note 002 (2)). An explicit `--threads` or Setup count is used as given.

**Which segments run on the workers.** A segment (the frames between two edit frames of a block) runs on the calling thread alone unless the plan has two tasks or more, the segment has at least `min_frames` frames (16) and the plan's estimated cost times the segment's frames reaches `parallel_min_ns`, and every task's log has room for the segment's staged external events. So a *k* = 1 block never goes to the workers. A segment on the calling thread alone indexes the plan's buffers directly, as M2.6-1 did, without the views a parallel segment's tasks use (ADR 0020). On the seat, in one session alternating with the base build, the kick at *k* = 1 on one thread measured 375.5 and 369.1 ns per frame against the base's 379.5 and 377.8 (`docs/benchmarks.md` "Threads (M2.6-2)"); the operator re-measures outside the sandbox. The thresholds and the cost table are in `docs/benchmarks.md`; they change the timing, never the output.

**A parallel segment.** The calling thread publishes the segment's job (`crates/signal-engine/src/shared.rs`, ADR 0020), wakes at most one parked worker, and takes ready tasks itself, as the workers do: a task is ready when its dependency counter reaches 0. It returns when every task is complete and every worker that joined the job has left it. Nothing in a segment allocates, frees, locks or blocks: claiming a task, completing it and joining or leaving the job are atomic operations on memory sized when the plan was built.

**Waiting policy and its cost.**
- An idle worker spins on the job gate after its last job (or a wake that brought none), then parks (`std::thread::park`). How long is `Schedule::spin_ns`: **offline 50 µs**; offline renders run blocks back to back, so their workers rarely park. **In real time too, 50 µs** (`realtime()` sets `Schedule::realtime`, which is the default). M2.6-5 measured spinning through the device period (`Schedule::spinning`): in the sandbox, without real-time scheduling, it cut the p99 per block 2.5–7× (`docs/benchmarks.md` "Real-time wake-up tail (M2.6-5)"). **On the device it failed:** with the workers in the audio workgroup, 80 kick voices on 12 threads gave 4 060 xruns in 15 s (worst callback 12.4 periods) against 0 xruns parking after 50 µs, and 120 voices 5 397 against 0. A worker that spins through the period runs far past its computation budget, and the kernel throttles it (see "The policy" below). Operator decision at the M2.6-5 checkpoint (2026-10-12): the real-time default stays parking after 50 µs until a device-measured policy beats it. An explicit schedule (tests, the bench) is used as given.
- **The bound.** Whatever a schedule asks, a worker spins at most `MAX_SPIN` = **3 ms** (`crates/signal-engine/src/pool.rs`), a little over two periods of 128 frames at 96 kHz; a longer device period (a larger buffer or a lower rate) parks the workers between callbacks again. A worker spins only after a job or a wake, so an engine that runs no parallel segment (stopped, between runs, or a plan too small for the workers) has every worker parked within 3 ms of its last job: an idle engine burns no core (`pool::tests::idle_workers_park_after_the_spin_bound`). The cost is while it plays a plan large enough for the workers: each worker then keeps its core busy for the whole period, so `threads − 1` cores show as fully used. That is why the spin is limited to the real-time auto count (two thirds of the performance cores, above), which leaves the rest to the control thread, the GUI and the system.
- The calling thread wakes at most one parked worker per parallel segment, and each worker that joins wakes up to two more (a tree: depth about log₂ of the workers). Waking is `Thread::unpark`: an atomic swap, and `dispatch_semaphore_signal` only when the worker is parked. It neither allocates nor blocks. It is the one system call a parallel segment can make on the calling thread, at most once per segment.
- The calling thread never waits for a wake: it runs ready tasks itself. Measured on the seat (planner probe, M2.6-2 plan): `unpark` costs the caller a median 1.7 µs (p99 5.0 µs); a parked worker starts running a median 10 µs (p99 44 µs, worst seen 1.6 ms) after its wake, so the *n*-th worker of the tree arrives after about log₂ *n* such hops. With ordinary workers the caller may therefore run much of a short block alone; real-time workers (below) are woken with the audio thread's deadline.
- The handshake loses no wake. A parking worker stores its `parked` flag and then re-reads the gate; the caller opens the gate and then reads the flags. All four operations are SeqCst, so in their single order either the worker sees the open gate and does not park, or the caller sees the flag and wakes it. An `unpark` that comes before the `park` is kept by the thread, so it is not lost either.
- The calling thread waits for a task a worker has claimed, and for every joined worker to leave the job before it returns. A worker preempted at that moment delays the block. With ordinary-priority workers this is bounded only by the OS scheduler: it is the real-time risk of this milestone, and the reason small plans and *k* = 1 stay on the calling thread.

**A worker that panics** (only a contract-breaking primitive or a test hook can make one) completes its task and leaves the job through drop guards, so the calling thread never hangs; the task's completion guard exists from its claim on, before the test hook runs. The worker is not replaced: the pool runs one worker short until the engine is rebuilt, and `schedule_stats().worker_panics` counts it. If the calling thread panics inside a parallel segment, a drop guard closes the job and waits for every joined worker to leave before the unwinding goes on, so no worker ever holds a reference to the segment's job after it.

**Determinism.** A task writes only its own instances' rows; every sum and event merge is computed by the consumer in canonical source order; the tasks' logs are joined in task order after the segment. The output and the log are the same bit for bit for every thread count (`docs/semantics.md` §13).

## Real-time workers (M2.6-3)

ADR 0019 §2 and §4: while a CoreAudio stream plays, the workers are scheduled like the device's I/O thread.

**The driver's hook.** `Runner::workers()` (and `Engine::workers()`) hands the driver a `signal_engine::Workers` handle, which stays usable while the Runner is in the callback. `Workers::post(f)` runs `f(w)` once on each worker thread `w`, in posting order, between jobs: a worker checks for requests at the top of its loop and again before it parks, and `post` unparks every worker after publishing the request (the same SeqCst handshake as the gate, so no request is missed). `Workers::wait(generation, timeout)` says how many live workers have run it. A request is kept until every live worker has run it, so a wait that times out drops nothing: a late worker runs it when it next loops. The request queue is a `Mutex` taken only by the control thread and by workers between jobs, never in a job, and never by the audio thread. A worker whose request panics ends, as one whose task panics does (it no longer counts; the pool runs one short until the engine is rebuilt).

**Entering and leaving** (`crates/signal-audio/src/rt_thread.rs`, the audited module, and `scheduling.rs`). `CoreAudioDriver::start`, before it builds the stream (so before the first block):
1. finds the output device's CoreAudio object: cpal hides it, so its id is matched by position in the system's device list (`kAudioHardwarePropertyDevices`, which cpal's `Host::devices` lists in the same order, unfiltered), read before and after cpal's listing, which must agree (three attempts); and reads its I/O workgroup (`kAudioDevicePropertyIOThreadOSWorkgroup`), or keeps why there is none;
2. posts `rt_thread::enter` to the workers: each sets the Mach **time-constraint policy** from the device's period, then **joins the workgroup** (only real-time threads can join one); it reports `workgroup` (policy and join), `time-constraint` (the policy, with the join error or why there is no workgroup) or `none` (the policy was refused);
3. waits up to 1 s; a worker that has not answered is reported `none (pending …)` and enters later.
If building or playing the stream fails, the workers leave again. `stop`, once the Runner is back (or the callback is gone) and the stream is paused, posts `rt_thread::exit`: leave the workgroup, then the standard policy back. A worker that ends while joined leaves first (its thread-local state does it on drop). Each joined thread holds its own reference to the workgroup, and so does each pending request, so no one leaves a released workgroup.

**The policy.** `TimeConstraint::from_period(frames, rate, timebase)`: period and constraint are the device period (128 frames at 96 kHz: 1.333 ms, 32 000 ticks of 125/3 ns), computation a quarter of it (333 µs, the V12 budget of a callback), clamped to 50 µs – 50 ms, preemptible. A quarter, not more: a time-constraint thread that keeps running beyond its computation can be demoted by the kernel, and the workers of a project within V12 need less. *M2.6-5 tried idle workers that spin through the period, so far past their computation: on the device the kernel throttled them and the stream broke up (thousands of xruns, "Waiting policy" above), which the sandbox could not show. Workers therefore park after 50 µs in real time as well.**assumption** that the operator's V12 soak and `signal bench --realtime --rt-schedule base --rt-schedule default` on the device decide.* In the seat's sandboxed table the device-like rows (`rt-all`: the calling thread in the same policy) ran 24 kick voices in a median of 280–411 µs and a p99 of 395–418 µs per block on 8 to 18 threads, at or near that budget (`docs/benchmarks.md`).

**Only beside a real-time caller.** In the same table, time-constraint workers beside an *ordinary* calling thread (`rt-workers`) had a p99 of 13–16 ms per block and a worst of 2.7 s: workers waiting at real-time priority for a task the ordinary caller holds can keep it from running. The driver therefore puts the workers into real time only around a CoreAudio stream, whose callback thread is real-time; the null driver, offline renders and replays leave them ordinary (only `signal bench --realtime` sets the policy, to measure it).

**Reporting.** `signal play` prints `signal: T threads (W workers): <path>` and adds `threads`, `workers` and `scheduling` (`path`, `summary`, each worker's path, why there was no workgroup) to its JSON; the app's status line shows `threads T (W workers, <path>)`. Statistics only: none of it reaches the Runner, the engine log, `log.jsonl`, a recorded session or the audio, so the determinism matrix is untouched (`crates/signal-audio/tests/rt_workers.rs` plays the kick with time-constraint workers bit for bit like the offline render).

## GPU (M2.6-4)

ADR 0019 §3. The per-block graph never runs on the GPU; GPU-backed modules declare a latency of *L* ≥ 1 blocks of their own (`module-contract.md`, "Declared latency").

**The device service** (`crates/signal-gpu`). `GpuService` creates the wgpu adapter and device (Metal backend) on the control side: `GpuService::shared()`, first called from a module's `prepare`, starts it once per process, or reports why there is no usable GPU. It owns one thread, `signal-gpu`, the **only** thread that submits GPU work or waits for it (`Device::poll`). A module talks to it through a `Channel`: a fixed pool of job buffers, allocated in `prepare`, moved (never copied, allocated or freed) through two `rtrb` SPSC queues, submit and done; the queues' atomic indices are the completion signal. `submit`, `peek_done`, `take_done` and `recycle` neither block, lock, allocate nor make a system call. The GPU thread never parks: it polls every channel and sleeps 50 µs when idle (`IDLE_SLEEP`; 1 ms while no channel exists, `UNUSED_SLEEP`), so a submitting thread never has to wake it. A dropped channel's jobs are dropped on the GPU thread; a stopped service makes every channel report its jobs lost.

**Never waited on in real time.** The engine asks a latent module whether its due result is in (`due`) before each `emit`: in real time once, and a result not in is a `late_result` fault (silence for that block, logged and counted), never a wait. So the GPU is never waited on by the audio callback or the real-time workers. Offline (renders, replays, the bench) the engine waits for it, yielding the thread. Verified by `lab/tests/realtime.rs` (the null driver while the service's test hook, `set_completion_delay`, holds every result back for 60 s: one second of audio runs in a fraction of a second with a fault per due block) and `crates/signal-engine/tests/latency.rs` (a fake latent module, no GPU). The audio side of `lab.gpu_partials` per block is its phases and the job's input: bounded, about 2.3 µs per 256-frame block on the seat (`docs/benchmarks.md`).

**Optional.** Only `lab.*` types use the GPU, and they compile in only with the `lab` feature (ADR 0015); every other module, and so every existing project and golden, runs on the CPU, bit-exact as before. Without the `lab` feature nothing but `signal-gpu` (and `signal-app`, through eframe) links wgpu.

## Review checklist (paste into PRs touching real-time code)

- [ ] No allocation, free, lock, syscall, I/O, or logging reachable from `process` / the executor
- [ ] No `unwrap` / `expect` / `panic` / unchecked indexing on the real-time path
- [ ] Worst-case work per frame is bounded and stated
- [ ] State updates are *k*-partition-invariant (contract suite passes)
- [ ] Feedback/decay paths are denormal-safe
- [ ] Randomness comes only from the seeded `ctx` stream
- [ ] Contract suite and benchmarks pass; new costs recorded
- [ ] GPU-backed modules (ADR 0019 §3): GPU work and every wait for it only on the `signal-gpu` thread; `due` never blocks; the engine waits offline only
- [ ] `crates/signal-engine/src/shared.rs` (ADR 0020): every `unsafe` block has a `SAFETY:` comment naming its ordering (Release on completion, Acquire before reading; the gate's SeqCst open, AcqRel join, Release leave); shared reads require `T: Sync`; no view escapes its `Access`; the checked-mode tests report no violation
- [ ] `crates/signal-audio/src/rt_thread.rs` (ADR 0019 §4): every `unsafe` block has a `SAFETY:` comment (`unsafe_scope.rs` checks); every workgroup call holds a live reference; a joined thread leaves with its own token and reference before it ends
