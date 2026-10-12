# Benchmarks (V13) and the soak (V12)

What the engine costs at 96 kHz, one frame at a time (*k* = 1, ADR 0016; the input to D-EXEC-3) and, since M2.6-1, in blocks of *k* frames (ADR 0019 §1; "Block sizes" below). The numbers are **measurements**, never thresholds: no test compares a timing to a limit, and the decision is the owner's (AGENTS.md rule 6).

## Method

`signal bench [--json] [--seconds <s>]` (`crates/signal-cli/src/bench.rs`) builds each case as an execution plan and runs it in the offline `Engine` one frame at a time (`run_frame`; the block rows run blocks, see "Block sizes"). Every case runs at 96 kHz with seed 42. After 1024 untimed frames, it times repeats of 1024 frames for `--seconds` (default 1 s, at least 5 repeats) and reports nanoseconds per frame: the median and the 99th percentile (nearest rank) over the repeats. The real-time budget is 10⁹ / 96 000 = **10 417 ns per frame**. The only clock reads are around each repeat, in the bench command. The engine is unchanged.

| Case | Graph | Reported |
|---|---|---|
| Engine, empty plan | No instances | The fixed per-frame overhead |
| Stimulus | A 440 Hz band-limited saw (`std.osc`), a 2 Hz `std.lfo` used as a gate, and a 132 bpm `std.transport` with ghost notes (probability 0.15) | Subtracted from each primitive's graph |
| Each registered `std.*` primitive | The stimulus and 16 instances at default parameters. Signal inputs come from the saw (phase inputs scaled to 0.05 cycles), gate inputs from the LFO, the first event input from the beats and ghost notes, and any other event input from the accents. Outputs are left open; every instance still runs. | (graph − stimulus median) / 16, per instance; the p99 column uses the graph's p99 |
| Kick project | `projects/kick` as committed (compiled into the binary) | Per frame, and as a fraction of the budget |
| Kick voices | The kick project with its `drums` instance duplicated *N* times (every connection to and from it repeated) and the copies summed by a tree of 4-input `std.mix` into `out` | The largest *N* whose **median** stays within 25 % and within 50 % of the budget (doubling, then bisection; every *N* measured is listed in `kick_voices.measured`) |
| Block sizes | The kick project, and the kick voices at the *N* within 25 % | Per frame, in blocks of *k* = 1, 64 and 256 (`blocks.measured`) |

## Reproducing a row

From the repository root, on an otherwise idle Mac, with a release build:

```sh
cargo build --release --locked --offline -p signal-cli
"$(cargo metadata --locked --offline --no-deps --format-version 1 | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/release/signal" --json bench > bench.json
# or, from a release bundle (scripts/bundle-app.sh --release):
dist/Signal.app/Contents/Helpers/signal --json bench > bench.json
```

`bench.json` holds every number in the tables below: `engine_empty`, `stimulus`, `primitives[]` (`median_ns` and `p99_ns` per instance, `graph` for the whole graph), `kick`, and `kick_voices.within_25_percent` / `within_50_percent`. It also has `profile`, which must be `release`, and `machine`, read with `sysctl` (`hw.model`, `machdep.cpu.brand_string`, `hw.ncpu`, `hw.perflevel0.physicalcpu`, `hw.memsize`, `kern.osproductversion`). Without `--json`, `signal bench` prints the same results as a table. To add a machine, add a column for it below, or replace the seat measurement, and give its date and machine. On the operator's Mac, `sysctl` is not sandboxed, so the re-run fills `machine` from `sysctl`: use those fields for the new row's machine data.

## Results

**Seat measurement**, 2026-10-12 00:39 UTC, from a devpanel seat (M2-5): `signal --json bench` from a release build, default `--seconds 1`, 46 s in total.

- Machine (from `system_profiler SPHardwareDataType`, `uname -a` and `sw_vers` on the same host, **not from `sysctl`**): Mac Studio (Mac17,14), Apple M5 Max, 18 cores (6 Super and 12 Performance), 64 GB, macOS 27.0.1. Every `sysctl` query in the seat sandbox failed with `sysctl: sysctl fmt -1 1024 1: Operation not permitted`, so `machine` in the seat's JSON is all null. The operator accepted this fallback for the seat row (operator decision sq-9d416a2d, 2026-10-12, on request 007; technical decisions are delegated to the operator). The operator's own `signal --json bench` re-run on the M5 Studio adds the `sysctl` machine fields.
- Conditions: shared with other work. The load average was 2.2–2.6 during the run. The run was not pinned to a core.

**Operator measurement**, 2026-10-12 01:14 UTC, on the same Mac outside the sandbox: `signal --json bench` from a release build at commit d480dfa plus M2-5, default `--seconds 1`.

- Machine (from `sysctl`, the JSON's `machine`): `Mac17,14`, Apple M5 Max, 18 cores (6 performance-level-0), 68 719 476 736 bytes (64 GiB), macOS 27.0.1.
- Conditions: not idle (load average 4.2, other sessions running).
- It agrees with the seat measurement within run-to-run noise: empty plan 2.0 ns; stimulus 75.9 ns; kick 261.3 ns median, 343.7 ns p99 (2.5 % of the budget); **14** kick voices within 25 % (median 2 495.6 ns) and **28** within 50 % (median 5 036.8 ns). Every per-primitive median is within 1.2 ns of the seat's table below (largest differences: `std.audio_out` 13.2, `std.transport` 30.2, `std.clamp` 18.6). The full JSON is reproducible with the command above.

### Engine and projects (ns per frame)

| Case | Median | p99 | Share of the 96 kHz budget (median) |
|---|---:|---:|---:|
| Engine, empty plan | 2.0 | 2.6 | 0.02 % |
| Stimulus (saw, LFO, transport) | 71.2 | 90.7 | 0.7 % |
| Kick project (`projects/kick`, 11 primitive instances) | 261.8 | 383.5 | 2.5 % |

### Kick voices (median criterion)

| Within | Limit (ns per frame) | Most voices | Median at that *N* | p99 at that *N* |
|---|---:|---:|---:|---:|
| 25 % of the budget | 2 604 | **14** | 2 560.5 | 2 669.0 |
| 50 % of the budget | 5 208 | **28** | 5 206.9 | 5 460.6 |

Measured *N* → median ns per frame: 1 → 296, 2 → 462, 4 → 790, 8 → 1 487, 12 → 2 191, 14 → 2 561, 15 → 2 790, 16 → 2 906, 24 → 4 467, 28 → 5 207, 29 → 5 353, 30 → 5 497, 32 → 5 869. Each added voice costs about 180 ns per frame.

### Primitives (ns per frame per instance)

| Primitive | Median | p99 |
|---|---:|---:|
| `std.abs` | 10.6 | 13.0 |
| `std.add` | 11.0 | 12.8 |
| `std.adsr` | 43.2 | 46.8 |
| `std.audio_out` | 12.0 | 19.8 |
| `std.clamp` | 17.5 | 27.8 |
| `std.clock_div` | 20.8 | 22.9 |
| `std.dc_block` | 17.7 | 21.2 |
| `std.delay` | 22.9 | 25.7 |
| `std.edge` | 18.8 | 20.8 |
| `std.exp_env` | 33.9 | 39.7 |
| `std.gain` | 14.5 | 17.3 |
| `std.lag` | 24.2 | 25.9 |
| `std.lfo` | 19.7 | 23.1 |
| `std.max` | 11.4 | 12.9 |
| `std.min` | 11.4 | 13.7 |
| `std.mix` | 31.4 | 39.2 |
| `std.mul` | 11.8 | 19.3 |
| `std.noise` | 8.9 | 11.2 |
| `std.onepole` | 16.5 | 19.5 |
| `std.osc` | 24.9 | 27.6 |
| `std.pan` | 18.3 | 20.5 |
| `std.sample_hold` | 15.0 | 23.6 |
| `std.scale` | 18.3 | 20.4 |
| `std.sine_osc` | 18.1 | 20.1 |
| `std.step_seq` | 56.7 | 64.2 |
| `std.sub` | 11.4 | 13.3 |
| `std.svf` | 21.8 | 24.2 |
| `std.tanh_shaper` | 29.8 | 34.0 |
| `std.transport` | 29.0 | 31.8 |
| `std.unit_delay` | 11.5 | 19.6 |

### Block sizes (M2.6-1)

`signal bench` times the kick and the kick voices (the count within 25 % of the budget at *k* = 1) in blocks of *k* = 1, 64 and 256 frames (`blocks` in the JSON; ADR 0019 §1, `semantics.md` §13). Each repeat is still 1024 frames, run as 1024 / *k* calls of `Engine::run_block(k)`, with the engine's block capacity set to *k*. The output is the same at every *k* (the determinism matrix in `docs/testing-and-evaluation.md`); only the time changes.

**Seat measurement**, 2026-10-12 03:41 UTC, from a devpanel seat (M2.6-1 candidate, uncommitted, on 72886ec): `signal --json bench` from a release build, default `--seconds 1`. Machine from `system_profiler SPHardwareDataType` and `sw_vers` (the JSON's `machine` is null: `sysctl` is sandboxed, as for the M2-5 seat row): Mac Studio (Mac17,14), Apple M5 Max, 18 cores (6 Super and 12 Performance), 64 GB, macOS 27.0.1. Conditions: shared with other work, load average 2.9–3.3, not pinned to a core. These are the seat's numbers, not the checks'.

| *k* | Kick median (ns/frame) | Kick p99 | 9 kick voices, median | 9 kick voices, p99 |
|---:|---:|---:|---:|---:|
| 1 | 386.6 | 443.1 | 2 535.9 | 2 793.3 |
| 64 | 149.0 | 172.6 | 1 074.1 | 1 192.1 |
| 256 | 141.2 | 166.7 | 1 028.0 | 1 117.5 |

- **Measurement:** at *k* = 256 the kick costs 141 ns per frame (1.4 % of the budget) and 9 kick voices 1 028 ns (9.9 %), 2.7× and 2.5× less than at *k* = 1 in the same run. Most of the gain is reached by *k* = 64.
- **Measurement:** in the same run, *k* = 1 (`run_frame`) costs more than the per-frame engine did in the M2-5 rows above: kick 386.6 ns against 261.8 ns, and 9 voices within 25 % of the budget against 14. The block engine does per-call work (staging inputs into call buffers, the fault scan, the event store, refreshing the last-frame values) that the per-frame engine did not.
- **Assumption:** part of that difference is load (2.9–3.3 here, 2.2–2.6 then). Only an idle re-run of both builds would separate the two. The real-time runner splits a callback at every command's frame, so a dense command stream brings its blocks toward *k* = 1.

### Threads (M2.6-2)

ADR 0019 §2 runs a segment's units on several threads (`docs/realtime-rules.md` "Worker threads", `semantics.md` §13 "Threads"). How the engine decides, from static figures in the code:

- **Costs.** Each `std.*` type's cost is its per-instance median in the primitives table above, rounded up (`signal_std::COST_NS`, registered with the types; a type without one costs 20 ns). A unit's estimate per frame is its members' costs times **0.54** (141.2 / 261.8: the kick at *k* = 256 against the per-frame engine, the M2.6-1 and M2-5 rows above), plus **22 ns** per member for state-output units and cycles, which run one frame per call ((386.6 − 141.2) / 11: the kick at *k* = 1 against *k* = 256, per instance).
- **Tasks.** A unit joins the task of the unit before it while that task is estimated below **2 000 ns per 256-frame block** and that unit feeds it; otherwise it starts a task (`Schedule::task_min_ns`).
- **When the workers run.** A segment runs on the calling thread alone unless the plan has two tasks or more, the segment has at least **16** frames (`min_frames`: a block of *k* = 1 never goes to the workers) and the plan's estimate times the segment's frames reaches **`parallel_min_ns`** (below).

`signal bench` gains three tables (`--json`: `blocks.measured[].on_performance_cores`, `scaling`, `parallel_threshold`). "Performance cores" is every performance level whose `hw.perflevelN.name` is not `Efficiency`, read with `sysctl`; where `sysctl` does not answer (the seat's sandbox) it is `available_parallelism`, and `scaling.cores_source` says which.

- **Block sizes on all performance cores:** the M2.6-1 rows above again, on the performance cores with the default schedule, beside one thread.
- **Scaling:** the most kick voices within 25 % and 50 % of the budget at *k* = 128 on 1, 2, 4, 8 and all performance cores, default schedule (doubling, then bisection, every *N* listed).
- **Small-plan threshold:** kick voices *N* = 1, 2, 4, 8, 16 in segments of *k* = 16, 64, 128 and 256 frames, on 2 threads with every segment on the workers (`Schedule::FORCED`, one task per unit) against one thread. `crossover_ns` is the smallest estimate per segment (the plan's estimate per frame times *k*) at which two threads were faster.

**Seat measurement**, 2026-10-12, from a devpanel seat (M2.6-2 candidate, uncommitted, on 31b48df): `signal bench` from a release build, default `--seconds 1`, with the base 31b48df built the same way, in one session, alternating: base, candidate, base, candidate. Machine as for the M2.6-1 row: Mac Studio (Mac17,14), Apple M5 Max, 18 cores (6 Super and 12 Performance), macOS 27.0.1. `sysctl` is sandboxed on the seat, so the performance cores are `available_parallelism`: **18** (`cores_source` `fallback`). Conditions: shared with other work, not pinned. These are the seat's numbers, not the checks'; the operator re-measures outside the sandbox.

Block sizes, kick and 9 kick voices (ns per frame, median; two runs each: the base, then the candidate on 1 thread and on 18):

| *k* | Kick, base | Kick, 1 thread | Kick, 18 threads | 9 voices, base | 9 voices, 1 thread | 9 voices, 18 threads |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 379.5 / 377.8 | 375.5 / 369.1 | 375.9 / 369.5 | 2 471.4 / 2 464.9 | 2 407.5 / 2 410.6 | 2 405.7 / 2 400.4 |
| 64 | 145.4 / 143.8 | 122.7 / 121.2 | 123.1 / 121.8 | 1 047.2 / 1 043.5 | 933.8 / 927.7 | 518.6 / 513.5 |
| 256 | 139.3 / 137.4 | 117.3 / 118.4 | 117.3 / 116.5 | 1 010.9 / 1 009.9 | 890.8 / 903.0 | 326.0 / 325.8 |

- **Measurement (AC13): *k* = 1 is not slower than the base.** The kick measured 375.5 and 369.1 against the base's 379.5 and 377.8; the plan's pass rule (at most the larger base run plus the base spread, 379.5 + 1.7 = 381.2) holds for both runs, which are below both base runs. 9 voices: 2 407.5 and 2 410.6 against 2 471.4 and 2 464.9 (2.5 % faster). A *k* = 1 block never goes to the workers (`min_frames`), and a segment on the calling thread alone builds no views (`docs/realtime-rules.md` "Worker threads", ADR 0020): it indexes the plan's buffers as M2.6-1 did, and a one-frame call copies its inputs into the call buffers with a store per port, not a `memcpy` call per port (the profile's one excess over the base). Earlier attempts in the same session order: views on that path, one per call, 463.3 and 460.9 against 377.2 and 376.1; without views but with the `memcpy` per port, 383.6 and 380.9 against 374.9 and 377.1 (the rule missed by 1.6–4.3 ns).
- **Measurement:** at *k* = 64 and 256 the candidate is 13–16 % faster than the base on one thread. On 18 threads, at *k* = 256, 9 kick voices cost 326 ns per frame against 891–903 on one thread (2.7×); at *k* = 64, 514–519 against 928–934. The kick alone does not gain: it is one voice (one chain of tasks).

Scaling at *k* = 128 (the most kick voices whose median stays within the limit; default schedule; two runs):

| Threads | Within 25 % | Within 50 % |
|---:|---:|---:|
| 1 | 26 / 26 | 54 / 53 |
| 2 | 33 / 35 | 67 / 67 |
| 4 | 63 / 63 | 126 / 125 |
| 8 | 92 / 92 | 189 / 189 |
| 18 (all performance cores) | 129 / 130 | 268 / 269 |

- **Fact to report (AC4):** within 25 % of the budget, all 18 performance cores run **129** and **130** kick voices against **26** on one thread, **5.0×** (5.0× and 5.1× within 50 %). Not a threshold.
- **Measurement:** the previous revision's run gave 196 against 24 (8.2×). The tasks' views now hand out windows whose every element is bounds-checked and readers that check each read's frames, at a cost on every parallel segment (with views, *k* = 64 on one thread measured 155–159 ns against 132 before); part of the difference may be the shared machine. Not separated here.

The small-plan threshold (2 threads, every segment on the workers, against 1 thread; estimate per segment in µs; ns per frame; the first run):

| Voices | *k* = 16 | *k* = 64 | *k* = 128 | *k* = 256 |
|---:|---|---|---|---|
| 1 | 3.2: 159 → 288 | 12.7: 136 → 189 | 25.3: 132 → 172 | 50.7: 132 → 150 |
| 2 | 5.2: 262 → 336 | 21.0: 235 → 216 | 42.0: 228 → 198 | 84.0: 223 → 188 |
| 4 | 9.4: 464 → 562 | 37.6: 418 → 376 | 75.3: 402 → 346 | 150.5: 396 → 330 |
| 8 | 18.3: 900 → 1 025 | 73.0: 823 → 715 | 146.0: 792 → 661 | 292.1: 775 → 635 |
| 16 | 35.4: 1 741 → 1 937 | 141.7: 1 586 → 1 375 | 283.4: 1 531 → 1 281 | 566.8: 1 496 → 1 221 |

- **Measurement (AC14):** the smallest estimate at which two threads were faster was 21.0 µs per segment (2 voices, *k* = 64; the same in both runs). Every *k* = 16 row lost (up to 35.4 µs), and a single voice lost at every size, up to 50.7 µs (its tasks form one chain, and this table runs one task per unit); every row of 2 voices or more from 21.0 µs on, *k* = 16 aside, was faster. **`parallel_min_ns` stays 50 µs.** One row lies above it and loses, the single voice at *k* = 256 (50.7 µs): with the default schedule, which chains it into fewer tasks, the kick on 18 threads costs what it costs on one (117.3 and 116.5 against 117.3 and 118.4 above). The estimate overstates block costs (one voice: 198 ns per frame estimated, 132 measured at *k* = 256); it only orders plans.

### Real-time workers on the device

**Operator measurement**, 2026-10-12, M5 Max (18 performance cores: 6 Super, 12 Performance), SSL 2+ Mk II at 96 kHz with a 128-frame buffer, release build of the M2.6-3 delivery. Projects from `signal bench --write-voices N`, with the `out` gain set to 0.001 so the runs were silent. Every worker joined the device's I/O workgroup (`scheduling workgroup`).

15-second runs, 120 voices (885 primitive instances):

| `--threads` | xruns | Worst callback (of the period) |
|---:|---:|---:|
| 1 | 22 305 | 1.619 |
| 4 | 0 | 0.917 |
| 6 | 0 | 0.683 |
| 12 | 0 | 0.528 |
| 17 (the first auto rule, P − 1) | 0 | 0.769 |
| 18 | 2 | 1.860 |

15-second runs at 8 and 12 threads: 40 voices 0.234 and 0.288; 60 voices 0.424 and 0.314; 80 voices 0.437 and 0.332, all with 0 xruns.

**30-minute soak**, 80 voices (585 instances), `--threads 12`, `--watch` with 178 live edits of `accent.decay_ms`: 172 800 000 frames, **0 xruns** (0 stream errors, 0 overruns), 0 late commands, 0 clipped samples, 0 edit errors, worst callback **0.405** of the period.

What it means:
- **Fact:** with real-time workers in the audio workgroup, Signal plays at least 120 kick voices live without an xrun. One thread cannot play that load at all.
- **Fact:** using every core (18) is worse than 12. Auto is now two thirds of the performance cores (`docs/realtime-rules.md` "Worker threads").
- **Measurement, open:** under multi-core load the worst callback is about 0.3–0.4 of the period whatever the voice count (40 to 120 voices at 12 threads), so a fixed cost (most likely waking parked workers) sets the tail, not the DSP. V12's "0 xruns" holds for this load. Its "worst callback < 0.25" holds for the single-core kick soak (0.104) but not yet under multi-core load. The follow-up issue M2.6-5 targets the wake-up tail.

### Real-time workers (M2.6-3)

`signal bench --realtime [--rt-voices N] [--seconds s]` measures how the workers' scheduling changes the time of a block as a device would ask for it (ADR 0019 §2, §4; `docs/realtime-rules.md` "Real-time workers"). It runs only this table:

- **Load:** *N* kick voices (default 24, the "Kick voices" graph) in blocks of 128 frames at 96 kHz, **paced**: one block per 1.333 ms period, the thread sleeping until the next one, so idle workers spin out and park as they do between device callbacks. After 8 untimed blocks, every block is timed on its own (at least 100 blocks; `--seconds` / 1.333 ms of them, 751 by default).
- **Threads:** 1, 8, the real-time auto count (`realtime_threads`: P − 1 when this table was measured, two thirds of the performance cores since the operator's revision) and all of them, with the default schedule.
- **Variants:** `ordinary` (every thread ordinary: the 5cb390c scheduling); `rt-workers` (the workers in the time-constraint policy the CoreAudio driver sets, `TimeConstraint::from_period(128, 96000, timebase)`: period and constraint 1.333 ms, computation 333 µs; **the calling thread ordinary, as in `ordinary`**, so only the workers differ); and `rt-all` (the calling thread in the same policy too, as a device's I/O thread is). Offline there is no device, so no workgroup: the workers' path is `time-constraint (offline: no device workgroup)`; the workgroup itself is measured only by the operator's device run.
- **Reported** per row: the median, p99 (nearest rank) and maximum time per block, as fractions of the period too, the blocks over 25 % and over 100 % of the period, and each worker's path (`--json`: `rows[]`, with `caller`; `policy`; `cores_source`).

To reproduce, from the repository root, on the Mac, outside the sandbox:

```sh
cargo build --release --locked --offline -p signal-cli
T="$(cargo metadata --locked --offline --no-deps --format-version 1 | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
"$T/release/signal" --json bench --realtime > rt-1.json; "$T/release/signal" bench --realtime
"$T/release/signal" --json bench --realtime > rt-2.json; "$T/release/signal" bench --realtime
```

**Seat measurement, sandboxed**, 2026-10-12 09:48–09:50 UTC, from a devpanel seat (M2.6-3 candidate, uncommitted, on 5cb390c): `signal bench --realtime` from a release build, defaults (24 voices, 751 blocks per row), run twice in a row in one session. Machine as for the M2.6-2 row: Mac Studio (Mac17,14), Apple M5 Max, 18 cores (6 Super and 12 Performance), macOS 27.0.1; `sysctl` is sandboxed, so the performance cores are `available_parallelism`, **18** (`cores_source` `fallback`), and P − 1 is 17. `thread_policy_set` was accepted in the sandbox (every worker reported `time-constraint`). Conditions: shared with other work, not pinned; load average 3.4 at the start and 18.5 at the end (other sessions). These are the seat's numbers, not the checks'; the operator re-measures with the commands above.

µs per block (run 1 / run 2; the period is 1 333 µs, 25 % of it 333 µs):

| Threads | Variant | Median | p99 | Max | Blocks over the period |
|---:|---|---:|---:|---:|---:|
| 1 | ordinary | 1 146 / 1 153 | 1 431 / 1 429 | 1 626 / 1 743 | 169 / 196 |
| 1 | rt-workers | 1 151 / 1 148 | 1 428 / 1 422 | 1 813 / 1 685 | 198 / 161 |
| 1 | rt-all | 956 / 959 | 1 161 / 1 168 | 1 190 / 1 175 | 0 / 0 |
| 8 | ordinary | 397 / 396 | 1 296 / 783 | 4 730 / 2 359 | 7 / 2 |
| 8 | rt-workers | 291 / 255 | 12 843 / 13 264 | 1 173 205 / 22 014 | 144 / 184 |
| 8 | rt-all | 403 / 409 | 417 / 415 | 425 / 432 | 0 / 0 |
| 17 (P − 1) | ordinary | 401 / 395 | 726 / 1 176 | 4 233 / 4 239 | 6 / 7 |
| 17 (P − 1) | rt-workers | 386 / 347 | 13 635 / 16 302 | 2 674 076 / 2 677 197 | 138 / 169 |
| 17 (P − 1) | rt-all | 411 / 402 | 418 / 410 | 1 566 / 436 | 3 / 0 |
| 18 (P) | ordinary | 399 / 390 | 1 096 / 1 777 | 4 680 / 6 007 | 3 / 9 |
| 18 (P) | rt-workers | 390 / 417 | 14 714 / 14 162 | 27 575 / 17 213 | 207 / 219 |
| 18 (P) | rt-all | 350 / 280 | 417 / 395 | 465 / 401 | 0 / 0 |

- **Measurement:** with the calling thread and the workers in the time-constraint policy (`rt-all`, the device's arrangement), the p99 per block on 8, 17 and 18 threads is 395–418 µs, against 726–1 777 µs with ordinary threads in the same session, and the worst block 401–1 566 µs against 2 359–6 007 µs; 3 blocks in 4 506 went over the period, against 34 with ordinary threads. The medians barely move (280–411 µs against 390–401 µs).
- **Measurement:** time-constraint workers beside an **ordinary** calling thread (`rt-workers`) are much worse than either: p99 12.8–16.3 ms, the worst block 2.7 s. Real-time workers waiting for a task the ordinary caller holds can keep it from running. The CoreAudio driver puts the workers into real time only around a device stream, whose callback thread is real-time (`docs/realtime-rules.md`).
- **Measurement (AC-auto-threads):** P − 1 = 17 threads was not worse than P = 18: p99 418 / 410 µs against 417 / 395 µs with real-time threads, 726 / 1 176 µs against 1 096 / 1 777 µs with ordinary ones. The real-time auto is P − 1 (`docs/realtime-rules.md` "Worker threads").
- **Measurement:** paced, 24 voices cost 1 146–1 153 µs per block on one ordinary thread, about 3.5 times what "Scaling" above measures for back-to-back blocks (26 voices within 333 µs on the operator's Mac). A block after a sleep starts with colder caches and, perhaps, a slower core; part of it is the shared machine. So the "within 25 %" voice counts of the back-to-back tables overstate what one device period carries; not separated here.
- **Context, not the same metric:** the operator's 5cb390c measurement outside the sandbox (operator note 002) gave 26 / 34 / 65 / 107 / 175 kick voices within 25 % at 1 / 2 / 4 / 8 / 18 threads (k = 128), a tail jitter at 18 threads of 2.6 µs median and 6.1 µs p99, and 0 xruns on the device at `--threads` 1 and 8. The `ordinary` rows here are the 5cb390c scheduling in this table's terms.

### GPU partial bank (M2.6-4)

ADR 0019 §3. `signal bench --gpu [--seconds s]`, in a `--features lab` build (the full `signal bench` of such a build adds the same section as `gpu_partials`), times `lab.gpu_partials` (1 024 partials, blocks of 256 frames = 2 666 667 ns at 96 kHz, *L* = 2) one block at a time, the parameters changing every block, for `--seconds` per row (at least 5 blocks), and reports ns **per block** (median, p99, and the median as a fraction of a block's period):

- **GPU round trip** (`per_block.gpu_round_trip`): from the submission of a block to its result in hand, on the calling thread (the job runs on the `signal-gpu` thread, including the upload, the dispatch and the read-back). It must stay well under *L* − 1 = 1 block for the real-time path to have its results in time; `null` with `no_gpu` (the reason) when there is no usable GPU.
- **CPU reference** (`per_block.cpu_reference`): `signal_lab::gpu_partials::reference`, the same block in f64 on one core.
- **Audio side** (`per_block.audio_side`): what the module does on the audio thread per block besides one `emit`/`absorb` per frame: the phases and the job's input.

**Seat measurement**, 2026-10-12, devpanel seat (sandboxed, M5 Max as above, shared load), release build, `signal bench --gpu --seconds 1`:

| Row | Median ns/block | p99 ns/block | Median, % of a block |
|---|---|---|---|
| GPU round trip | not measured: the seat has no usable GPU (`metal found no adapters` inside the sandbox) | | |
| CPU reference | 1 077 958 | 2 473 209 | 40.4 % |
| Audio side | 2 334 | 2 625 | 0.1 % |

The tolerance is checked by `lab/tests/gpu_partials.rs` on a GPU: |GPU − CPU| ≤ 1e-4 of the bank's full scale Σ|a_i| per sample (first estimated at 1e-5 without a GPU; **operator measurement**, 2026-10-12, Apple M5 Max with Metal: worst 4.5e-5 at 48 kHz and 1.9e-5 at 96 kHz, identical on every run; the excess over the simulated kernel comes from the GPU's `sin`, which is not correctly rounded). Operator GPU bench, same machine: 1 024 partials per 256-frame block, GPU round trip 0.91 ms median (0.96 ms p99), CPU f64 reference 1.07 ms, audio-thread side 2.3 µs. Without a GPU, the seat measured the design instead: the kernel's arithmetic in f32 on the CPU (libm's `sinf` for the GPU's `sin`) stays within **8.1e-7** of full scale of the f64 reference over 192 blocks at 44.1, 96 and 192 kHz (`f32_kernel_arithmetic_is_within_the_tolerance`, a measurement of the packing, not of the GPU).

### Pending operator steps for the M2.6-4 checkpoint

The seat has no usable GPU: every GPU test here ran on wgpu's noop device or skipped with its reason (`lab/tests`, `crates/signal-gpu`), and no GPU number is measured. On the Mac outside the sandbox, from the repository root, with `PATH=/opt/homebrew/opt/rustup/bin:$PATH` and `T=/tmp/signal-project-target`:

1. **The checks at the pinned target**, default build: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`, `cargo test --workspace --locked --offline`.
2. **The lab build and its GPU tests:** `cargo clippy -p signal-lab -p signal-cli --features lab --all-targets --locked --offline -- -D warnings`, then `cargo test -p signal-gpu -p signal-lab --features signal-lab/lab --locked --offline -- --nocapture` and `cargo test -p signal-cli --features lab --locked --offline` (in a lab build `schema_output` is ignored: its snapshot is the default build's catalogue). Expect `GPU: <name> (Metal)` (not `skipped`) and `device: <name> (Metal)` lines, the two `worst |GPU − CPU| / Σ|a|` lines (at 48 and 96 kHz) at or below `1e-4`, and every test passing: AC1a (tolerance), AC1b (*L* blocks late), AC1c (bit-identical offline renders over *k* 1, 64, 256 and 1, 3, 4 threads), AC3 (the 60 s hold: about 373 late results in well under 10 s).
3. **Repeated renders on the device:** `cargo build --release -p signal-cli --features lab --locked --offline`, then `"$T/release/signal" render projects/gpu-partials --seconds 10 --out /tmp/gpu-a` and `"$T/release/signal" render projects/gpu-partials --seconds 10 --out /tmp/gpu-b`: the two `manifest.json` have the same `audio_fnv1a64` and `log_fnv1a64`, `log.jsonl` holds no `late_result`, and `same_machine` lists `/bank`.
4. **The bench row:** `"$T/release/signal" --json bench --gpu --seconds 2`, twice; add an operator row to the table above (GPU round trip, CPU reference, audio side).
5. **Real time on the device:** `"$T/release/signal" play projects/gpu-partials --seconds 60 --json`, with the interface as the default output: report `xruns`, `worst_callback_ratio` and `late_results` (expected 0 with an idle GPU; every late result is a block of silence, logged), and listen for clicks.
6. **The real-time tail is no worse (AC12):** with the `--features lab` release build, `scripts/soak.sh --voices 80 --threads 12 --signal "$T/release/signal" 30 128`, the M2.6-3 checkpoint's load (0 xruns, worst 0.405 there). The tail must not be worse; then the same soak with `projects/gpu-partials` playing beside it in a second `signal play` is optional information, not a criterion.
7. **Signal.app (GUI).** The app is built without the `lab` feature, so it must behave exactly as at the M2.6-3 checkpoint. Run `./start.sh projects/kick` (a release build of `dist/Signal.app`, opened on the kick). Then:
   - press play: the meters move, and the status line ends `threads T (W workers, workgroup)` with T the Setup's "CPU workers" (auto: two thirds of the performance cores);
   - move a parameter in the parameter panel (e.g. the kick's decay): the sound follows, and the setup dialog still lists the devices and keeps the chosen output after a restart;
   - let it play for 5 minutes: no dropout heard, and the status line shows `xruns 0`;
   - type the absolute path of `projects/gpu-partials` into "Project folder" and press Open (or drop the folder onto the window): the app must refuse it, showing in red the diagnostic ``/bank: unknown module `lab.gpu_partials` (neither a registered primitive type nor a composite)``, without crashing, because GPU modules exist only in a `--features lab` build of `signal`.
   Report anything else verbatim, with the app's status line.

## What this means for D-EXEC-3

These are interpretations of a single seat measurement. They are not a decision; D-EXEC-3 stays deferred (roadmap M4).

- **Fact (measured):** the per-frame engine runs the kick project in 2.5 % of the 96 kHz budget. Fourteen kick voices (about 100 primitive instances) fit within 25 %, and 28 voices within 50 %. Today's test projects do not need block processing to run in real time.
- **Measurement:** the fixed per-frame cost of the engine is negligible (about 2 ns). The cost is per instance. The cheapest primitives (`std.abs`, `std.add`, `std.mul`, `std.noise`) cost 9–12 ns, so roughly 10 ns per instance per frame goes to dispatch, input gathering and parameter handling rather than to the DSP. Heavier primitives cost 20–57 ns.
- **Assumption:** block execution (*k* > 1) would mostly reduce that per-instance floor. It would matter for graphs of several hundred small instances (at about 10 ns each, about 260 trivial instances use 25 % of the budget), not for the kick. Whether such graphs are expected is a roadmap question for M4.
- **Caveat:** the p99 columns and the voice counts depend on the machine's load. Re-run on an idle machine before the decision, and record each run as its own column.

## The soak (V12)

`scripts/soak.sh [--driver coreaudio|null] [--signal PATH] [--target-dir DIR] [--voices N] [--threads n] [minutes] [buffer]` copies `projects/kick` into a new temporary folder (`$TMPDIR/signal-soak.*`); the committed project is never edited. With `--voices N` (M2.6-3) the folder holds instead the kick with its `drums` composite duplicated *N* times into a tree of 4-input `std.mix`, written by `signal bench --write-voices N <folder>` (the "Kick voices" graph, its `out` gain divided by *N* so that it does not clip; the `accent` line is the same). It runs `signal --json play <copy> --watch --driver <driver> --buffer <buffer> --seconds <minutes × 60>` at the project's 96 kHz, with `--threads n` when given (otherwise `signal play`'s auto, the performance cores minus one). Every 10 s it rewrites `accent` `decay_ms` in the copy, alternating 300 and 200, with `sed` into a hidden file and `mv` (no Python). At the end it prints the frames, the voices, the threads and workers with their scheduling path (`workgroup`, `time-constraint`, `none` or `mixed`, from the play JSON), the edits written and applied, the xruns, the worst callback as a fraction of the period, and the verdict:

- **PASS**: 0 xruns and a worst callback under 0.25 of the period (V12). Exit 0.
- **FAIL**: anything else. Exit 1. Exit 2 means the soak could not run.

Without `--signal` it builds `signal-cli` with `--release --locked --offline` and takes `release/signal` from the target directory cargo reports (`--target-dir`, else `CARGO_TARGET_DIR` or `.cargo/config.toml` through `cargo metadata`), as `scripts/bundle-app.sh` does. `--signal dist/Signal.app/Contents/Helpers/signal` uses the bundled CLI. The play JSON and log stay in the temporary folder, and their paths are printed.

### The operator's V12 run

On the Mac, with the default output device set to the interface under test, from the repository root:

```sh
scripts/soak.sh 30 128
```

That runs for 30 minutes on the default device (CoreAudio) at 96 kHz with a 128-frame buffer and 180 parameter edits. V12 passes when the last line reads `V12 PASS`: `xruns 0` and `worst callback` below `0.250`. Check also that `edits … applied as parameter sets` is close to the number written (an edit in the last moments may not apply) with `0 errors`.

**Result, operator measurement, 2026-10-12 01:17–01:47 UTC:** the SSL 2+ Mk II (default output), 96 kHz, 128-frame buffer, 30 minutes, release build. To run it silently at night, the operator ran the script unchanged from a temporary root whose `projects/kick` copy had `out` gain 0.001 (−60 dB); every primitive still runs, so the timing is the same.

```text
soak: 172800000 frames at 96000 Hz, buffer 128, coreaudio driver
edits 178 written, 177 applied as parameter sets, 0 errors
xruns 0
worst callback 0.104 of the period
V12 PASS (0 xruns; worst callback < 0.25 of the period)
```

The operator's null-driver check on the same Mac (`scripts/soak.sh --driver null 1`, not idle) also reported FAIL (33 xruns, worst 9.4 periods), which confirms that the null driver's wall-clock pacing on an ordinary thread is not a V12 measurement.

### Seat check (null driver)

Seat measurement, 2026-10-12, same machine and conditions as above: `scripts/soak.sh --driver null 1` (CARGO_TARGET_DIR set to the seat's scratch folder):

```text
soak: 5760000 frames at 96000 Hz, buffer 128, null driver
edits 5 written, 5 applied as parameter sets, 0 errors
xruns 18
worst callback 3.419 of the period
V12 FAIL (needs 0 xruns and a worst callback < 0.25 of the period)
```

The script works end to end: the copy, the build, the play, the edits applied live, and the parsed verdict. The null driver's FAIL is not a V12 result. The null driver runs `Runner::process` on an ordinary thread and times it with the wall clock. On the loaded, sandboxed seat, that thread is preempted. Plain `signal play projects/kick --driver null --seconds 10` without edits showed 4–5 overruns and a worst callback of 1.4–2.5 periods per run, while the kick needs about 2.5 % of the period on the median. Only the operator's run on the device decides V12.

M2.6-3 seat check, 2026-10-12, same conditions: `scripts/soak.sh --driver null --voices 8 --threads 2 --signal <release signal> 0.2` wrote the 8-voice project (63 primitive instances), applied the edit live (1 written, 1 applied, 0 errors) and printed `voices 8, threads 2 (1 workers), scheduling none (null driver)`; its null-driver verdict (FAIL, 432 xruns) is, as above, not a V12 result.

### Pending operator steps for the M2.6-3 checkpoint

On the Mac outside the sandbox, from the repository root, with `PATH=/opt/homebrew/opt/rustup/bin:$PATH`:

1. **Rerun the checks at the pinned target path** (`/tmp/signal-project-target`, `.cargo/config.toml`): `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`, `cargo test --workspace --locked --offline`.
2. **The workgroup join on the device.** With the interface as the default output: `"$T/release/signal" play projects/kick --threads 8 --seconds 10` (`$T` as in "Real-time workers (M2.6-3)"). The line after `playing …` should read `signal: 8 threads (7 workers): workgroup`. Then the same with `--device "<another output's name>"` (see `signal devices`): `workgroup` there too. Anything else (`time-constraint (…)`, `none (…)`, `mixed (…)`) is reported with its reason, verbatim.
3. **The app display.** Open a project in Signal.app and play it: the status line ends `threads T (W workers, workgroup)`, with T the Setup's "CPU workers" (auto: the performance cores minus one).
4. **Re-measure the real-time table** with the four commands under "Real-time workers (M2.6-3)" (two runs), add them as an operator row, and confirm or revise the real-time auto rule (`docs/realtime-rules.md` "Worker threads") from the P − 1 and P rows.
5. **V12 at 30 minutes with many voices**, reporting the voice count, the threads and the scheduling path the soak prints:
   - *N* as the issue states it, about 60 % of one core's budget times the performance cores *P*: one core's whole budget at *k* = 128 is 4 × the one-thread "within 25 %" count of `signal --json bench` (`scaling.rows[]` where `threads` is 1, `within_25_percent.voices`), so *N* = round(2.4 × N25(1 thread) × *P*); on the operator's 5cb390c numbers 2.4 × 26 × 18 ≈ **1 123**. Then `scripts/soak.sh --voices <N> --threads <P> 30 128`.
   - For information, predicted by arithmetic: that load is 0.6 × *P* core-periods of work per period, so even perfectly spread over *P* threads every callback takes at least 0.6 of the period, above V12's 0.25: a FAIL is expected by construction. Beside it, the heaviest load consistent with V12 is about 0.6 × N25(*P* threads) = 0.6 × 175 ≈ **105** voices (`scaling.rows[]` where `threads` is *P*), which still needs about four cores. The choice of *N* is the operator's.
   - V12 passes with `xruns 0` and a `worst callback` below `0.250`; the soak result (AC-soak-result) is the operator's measurement.
