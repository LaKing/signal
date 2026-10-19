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

### `std.ladder` against `std.svf` (M2.11-2)

**Seat measurement**, 2026-10-17 14:15–14:20 UTC, from a devpanel seat. Command: `signal --json bench` from a release build of f6f82e2 plus the M2.11-2 change (uncommitted), default `--seconds 1`, the method above. Both rows come from the same run. They are a single-host measurement, not a threshold. The table above is from an earlier run on a less loaded host, so compare these two rows with each other, not with it.

- Machine (from `system_profiler SPHardwareDataType`, `uname -a` and `sw_vers`; `sysctl` is refused in the seat sandbox, so `machine` in the JSON is all null): Mac Studio (Mac17,14), Apple M5 Max, 18 cores (6 Super and 12 Performance), 64 GB, macOS 27.0.1.
- Conditions: shared with other work. The load average was 5.3 at the start and 5.0 at the end. The run was not pinned to a core. Stimulus 122.2 ns and empty plan 8.5 ns in this run.
- **Default parameters only.** The bench runs every primitive at its defaults, so the ladder runs at resonance 0 and drive 0 (k = 0). There the zero-delay solve stops after two Newton steps, three `tanh` a frame, which is its cheapest path. The resonant worst case (up to 8 steps, nine `tanh`) is not measured by `signal bench`.

| Primitive | Median | p99 |
|---|---:|---:|
| `std.ladder` | 99.0 | 101.4 |
| `std.svf` | 32.7 | 35.1 |

The ladder costs about 3.0× the state-variable filter per instance at default parameters.

**The resonant worst case** (review of M2.11-2, after the closed-bracket fix of the solve). **Measurement on one host**, 2026-10-17 14:44 UTC, the same Mac Studio (Mac17,14, Apple M5 Max, 64 GB, macOS 27.0.1), shared with other work (load average 3.0–3.6), not pinned. `signal bench` cannot set parameters, so this is timed differently: `signal render --threads 1` of three projects at 96 kHz, 480 000 frames, from a release build of 1fdedc1 plus the M2.11-2 change (uncommitted): a 440 Hz saw into `std.audio_out` alone (the base), and the same with 32 ladders on the saw summed into the output, once at default parameters and once at `cutoff_hz` 43 200 (0.45·sr), resonance 1 and drive 10. The cost per ladder frame is (time − base time)/(32 · 480 000), from the median of 3 runs each.

| Ladders at | `--block 1` | `--block 256` |
|---|---:|---:|
| default parameters | 99.1 | 80.4 |
| cutoff 0.45·sr, resonance 1, drive 10 | 183.7 | 169.6 |

At `--block 1` the default row agrees with `signal bench` above (99.0). The worst case costs about 1.85× the default, about 5.6× `std.svf`. There the solve takes 3.5 Newton steps on average and at most 5 (counted in a copy of the solve on the same input), against 2 at default parameters; the bound is 8.

### The studio project (M2.11-4)

`projects/studio` at its defaults: 177 primitive instances (the voice 39, the tape echo 21, the seven drum voices 108, the transport, sequencers and master bus 9) at 48 kHz. (The first measurement below was of the first version, 161 instances; the review's revision added the stepped octave, the first-note glide gate and the kick's reordered drive.)

**Method, which differs from `signal bench`'s.** `signal bench` times the kick project and per-primitive figures and cannot run another project, so this is wall time of `signal render`, as for the ladder's worst case above. On a release build of e790596 plus the M2.11-4 change (uncommitted), it is `/usr/bin/time -p signal render projects/studio --seconds 60 --threads 1 --out …` (2 880 000 frames, default `--block` 256), and the same render of a project holding only `std.audio_out` (the base: start-up, writing the WAV and the log). DSP time per second of audio is (studio − base)/60, from the median of 3 runs each. The figure includes the engine's per-block work and the log, not only the primitives.

**Measurement on one host**, 2026-10-17 16:37 UTC, in a devpanel seat on the operator's Mac Studio, shared with other work (load average 2.6–6.0), not pinned. `sysctl` is not readable in the seat, so the machine fields are those of the rows above, not read here; the operator's re-run fills them.

| `--threads` | studio, 60 s (runs) | base | DSP per second of audio | per frame | budget at 48 kHz |
|---|---|---:|---:|---:|---:|
| 1 | 4.10, 4.35, 4.41 s | 0.11 s | **0.071 s** | 1.47 µs | 7.1 % of one core |
| default (one per performance core) | 2.42, 2.44, 2.49 s | 0.11 s | 0.039 s (wall) | — | — |

**Re-measured after the review revision** (177 instances), 2026-10-17 20:05 local, same method and seat, under heavier load (load average 8.6–10.6, a full check running beside it), so an upper figure:

| `--threads` | studio, 60 s (runs) | base | DSP per second of audio | per frame | budget at 48 kHz |
|---|---|---:|---:|---:|---:|
| 1 | 4.72, 4.74, 5.01 s | 0.08 s | **0.078 s** | 1.62 µs | 7.8 % of one core |

The operator confirms the figure on the Mac outside the seat with the same commands.

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
- **Measurement, open:** under multi-core load the worst callback is about 0.3–0.4 of the period whatever the voice count (40 to 120 voices at 12 threads), so a fixed cost (most likely waking parked workers) sets the tail, not the DSP. V12's "0 xruns" holds for this load. Its "worst callback < 0.25" holds for the single-core kick soak (0.104) but not yet under multi-core load. The follow-up issue M2.6-5 targets the wake-up tail ("Real-time wake-up tail (M2.6-5)" below: the spin that shortened the tail in the sandbox broke the stream on the device and was rejected; the worst callback under multi-core load stays about 0.4 of the period, and V12's headroom criterion stays open).

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

### Wake-policy matrix on the device (M2.7-1)

**Operator measurement**, 2026-10-13 02:25–02:37, M5 Max, SSL 2+ Mk II at 96 kHz with a 128-frame buffer, release build of the M2.7-1 delivery. `scripts/rt-matrix.sh`, default matrix: 4 policies × threads 8/12/14 × 80/120 silent voices × computation 0.25/0.75, 10 s per cell. All 48 cells are in `docs/measurements/rt-matrix-2026-10-13.csv`. Columns are fractions of the period unless marked µs.

At computation 0.25:

| Voices | Threads | Median (all policies) | p99 (all policies) | Worst (range over policies) | Worker wake p99 |
|---:|---:|---:|---:|---:|---:|
| 80 | 8 | 0.32–0.33 | 0.34–0.35 | 0.38–0.50 | 18–21 µs |
| 80 | 12 | 0.23–0.26 | 0.29–0.31 | 0.35–0.39 | 18–21 µs |
| 80 | 14 | 0.24–0.27 | 0.29–0.32 | 0.34–0.36 | 360–426 µs |
| 120 | 8 | 0.39–0.40 | 0.41 | 0.58–0.63 | 18–37 µs |
| 120 | 12 | 0.34–0.35 | 0.41–0.43 | 0.48–0.59 | 18–37 µs |
| 120 | 14 | 0.35–0.36 | 0.41–0.43 | 0.50–1.04 | 491–590 µs |

xruns: 0 in every cell, except `budget-spin` at computation 0.75 on 14 threads (16 and 15) and `fanout` at 14 threads, 120 voices, computation 0.25 (2).

What it shows:
- **Fact:** the p99 is only 0.02–0.08 of the period above the median for every policy. The median is the load itself. The earlier reading of M2.6-3 and M2.6-5, that a fixed wake-up cost set the tail, was wrong: at 80–120 voices the work itself is 0.25–0.40 of the period.
- **Fact:** no policy beats `park50` beyond noise; `semaphore` and `fanout` differ by at most a few hundredths. 14 threads gain nothing over 12, and their workers wake late (p99 360–590 µs).
- **Decisions (operator, 2026-10-13):** the real-time default stays `park50`, the computation budget stays 0.25 of the period, and auto stays two thirds of the performance cores (12 of 18). The other policies stay selectable with `--rt-policy` for other machines.
- **The timing gates, reconciled** (2026-10-13, after the external re-review, which objected that worst − median measures jitter and not headroom): **V12** keeps its absolute rule at nominal load (`projects/kick`: worst < 0.25 of the period); a **capacity gate** at heavy load (80 voices) requires 0 xruns and an absolute worst < 0.75 of the period (25 % headroom). Worst − median is reported as jitter, not gated.
- **First capacity measurement** (operator, 2026-10-13 02:51–03:21, M2.7-1 release build): `scripts/soak.sh --silent --voices 80 30 128`, auto (12 threads, `park50`, workgroup), 178 live edits all applied: 0 xruns, median 0.27, p99 0.32, worst 0.444 of the period.
- **Gate soaks at 68b4077** (operator, 2026-10-13, SSL 2+, 96 kHz, 128 frames, auto = 12 threads, `park50`, workgroup, silent, 30 minutes each with live edits, every edit applied):

  | Soak | xruns | Median | Worst | Worst − median | Verdict |
  |---|---:|---:|---:|---:|---|
  | V12: `projects/kick` (17:41–18:11 local) | 0 | 0.050 | 0.137 | 0.087 | **V12 PASS** (< 0.25) |
  | Capacity: 80 voices, 592 instances (18:11–18:41) | 0 | 0.280 | 0.513 | 0.233 | **capacity PASS** (< 0.75) |
  | Capacity re-run after M2.7-9 (hang probe on the hot path), 23:24–23:54 | 0 | 0.290 | 0.454 | 0.164 | **capacity PASS** (< 0.75) |


### Real-time wake-up tail (M2.6-5)

The operator's device figures this starts from ("Real-time workers on the device" above): 80 kick voices, `--threads` 12 (the real-time auto count), SSL 2+ at 96 kHz with 128-frame buffers, 30 minutes with live edits, 0 xruns and a **worst callback of 0.405** of the period at db0363c, **0.408** at dafa32b (M2.6-4 checkpoint). V12 asks for < 0.25.

**What the table measures now.** Besides the time per block, each `signal bench --realtime` row has the engine's wake-up trace (`rows[].wake`, M2.7-1). Until M2.7-3 the table also read a `tail` off the engine's task hook; that hook is a test-only API now (feature `test-hooks`, which no build of `signal` turns on), so `rows[].tail` is gone. Its columns map onto the trace as follows; **tail figures from before M2.7-3 are not comparable with the trace's**:
- **slowest task** (`tail.slowest_task`) → `wake.slowest_task_*_us`: the longest task body of each callback. The trace counts callbacks run on the calling thread alone too, the tail only blocks that ran on the workers; the text table shows it for every row, one thread included.
- **workers started** (`tail.worker_start`, from the first thread into the job until the last joined worker entered it): retired. The nearest figure is `wake.last_join_*_us`, from the callback's start; the text table shows it as "last join" ("—" on one thread, where no worker joins).
- **first participation** (`tail.open_after`): retired. The nearest is `wake.first_join_*_us`, the first *worker* to join (not the calling thread).
- the **workers joined** (`tail.joined`): retired; `wake.callbacks_without_join` counts the callbacks no worker joined.
- the **blocks run on the workers** (`tail.parallel_blocks`): retired; `wake.callbacks − wake.callbacks_without_join`. The blocks with more than one job (`tail.multi_job_blocks`, always 0 for this load): retired.

The trace's spreads are the median, p99 (nearest rank) and maximum over the row's callbacks, in the JSON and in the text table.

`--rt-schedule <s>` (repeatable) runs the table once per worker schedule, named in `rows[].schedule`: `base` is the scheduling of dafa32b (pinned in the bench: 50 µs spin, 2 µs chains); `default` is what `signal play` and the app use (`Schedule::realtime`); otherwise `base` with `spin=<ns|period|quarter>` (the idle spin) and `task=<ns>` (`Schedule::task_min_ns`, the chain size). **Since M2.6-5 the rows without `--rt-schedule` are `default`, not the dafa32b scheduling**: compare against `base` rows or a dafa32b build, as below.

**Seat measurement, sandboxed**, 2026-10-12 13:56–14:28 UTC, devpanel seat, Mac Studio (Mac17,14) M5 Max as in "Real-time workers (M2.6-3)" (`cores_source` `fallback`, 18; real-time auto 12). Two release builds, each with its own scratch target directory: **dafa32b** (`git archive dafa32b`, unchanged) and the **candidate**. Shared machine, not pinned: load average 2.6 to 24.5 (other sessions). Runs alternated, dafa32b binary then candidate, with the same commands:

```sh
signal --json bench --realtime --rt-voices 24      # dafa32b binary
signal --json bench --realtime --rt-voices 24 --rt-schedule base --rt-schedule spin=quarter ...   # candidate
signal --json bench --realtime --rt-voices 80      # and the same at 80 voices
```

751 blocks per row. Every row below is `rt-all` (the calling thread and the workers in the time-constraint policy, as a device runs them; no workgroup offline). µs per block, round 1 / round 2. The workgroup path is only on the device.

**The options** (rounds 1–2, 13:56–14:13 UTC). `prewake` and `workers` were measured on an intermediate candidate and removed after (below); `spin` and `task` remain in the bench.
- `prewake=<n>`: waking *n* parked workers at the start of each block, before its input is staged.
- `workers=<ns>`: at most one lane per that many ns of the segment's estimated cost.

| Voices | Threads | Schedule | Median | p99 | Max | Over the period | Workers started p99 | Workers started max | Slowest task p99 | Workers joined (median) |
|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 24 | 8 | dafa32b binary | 385 / 390 | 394 / 395 | 399 / 397 | 0 / 0 | – | – | – | – |
| 24 | 8 | `base` | 413 / 261 | 420 / 393 | 432 / 396 | 0 / 0 | 53 / 69 | 65 / 83 | 87 / 50 | 5 / 5 |
| 24 | 8 | `prewake=1` | 405 / 227 | 413 / 385 | 441 / 389 | 0 / 0 | 60 / 63 | 68 / 67 | 56 / 50 | 5 / 5 |
| 24 | 8 | `prewake=all` | 242 / 381 | 416 / 390 | 419 / 402 | 0 / 0 | 27 / 32 | 71 / 60 | 58 / 48 | 5 / 5 |
| 24 | 8 | `workers=100000` | 473 / 450 | 482 / 456 | 487 / 458 | 0 / 0 | 37 / 45 | 74 / 55 | 53 / 48 | 4 / 4 |
| 24 | 8 | `spin=quarter` | 246 / 196 | 418 / 386 | 1 087 / 1 380 | 0 / 1 | 75 / 75 | 542 / 84 | 88 / 54 | 5 / 7 |
| 24 | 8 | `spin=period` | 110 / 98 | 119 / 157 | 145 / 189 | 0 / 0 | 3 / 4 | 21 / 29 | 35 / 27 | 7 / 7 |
| 24 | 8 | `task=1000` | 263 / 388 | 427 / 396 | 428 / 438 | 0 / 0 | 74 / 58 | 234 / 92 | 58 / 60 | 5 / 5 |
| 24 | 8 | `task=4000` | 414 / 245 | 420 / 392 | 774 / 398 | 0 / 0 | 39 / 66 | 43 / 98 | 56 / 51 | 5 / 5 |
| 24 | 8 | `task=8000` | 406 / 388 | 414 / 397 | 420 / 403 | 0 / 0 | 61 / 71 | 70 / 78 | 84 / 60 | 5 / 5 |
| 24 | 12 | dafa32b binary | 395 / 389 | 401 / 396 | 415 / 412 | 0 / 0 | – | – | – | – |
| 24 | 12 | `base` | 324 / 388 | 422 / 393 | 429 / 395 | 0 / 0 | 62 / 43 | 74 / 44 | 99 / 48 | 5 / 5 |
| 24 | 12 | `prewake=1` | 406 / 383 | 416 / 389 | 424 / 394 | 0 / 0 | 50 / 45 | 71 / 53 | 100 / 62 | 5 / 5 |
| 24 | 12 | `prewake=all` | 414 / 315 | 420 / 395 | 426 / 400 | 0 / 0 | 11 / 13 | 62 / 38 | 92 / 70 | 5 / 5 |
| 24 | 12 | `workers=100000` | 474 / 449 | 481 / 455 | 489 / 477 | 0 / 0 | 35 / 44 | 70 / 53 | 53 / 46 | 4 / 4 |
| 24 | 12 | `spin=quarter` | 215 / 197 | 342 / 311 | 345 / 335 | 0 / 0 | 71 / 71 | 101 / 90 | 96 / 62 | 11 / 11 |
| 24 | 12 | `spin=period` | 100 / 91 | 108 / 133 | 125 / 177 | 0 / 0 | 6 / 5 | 29 / 39 | 34 / 33 | 11 / 11 |
| 24 | 12 | `task=1000` | 419 / 393 | 427 / 400 | 823 / 402 | 0 / 0 | 43 / 42 | 57 / 66 | 61 / 51 | 5 / 5 |
| 24 | 12 | `task=4000` | 414 / 389 | 421 / 395 | 426 / 404 | 0 / 0 | 56 / 41 | 61 / 47 | 88 / 48 | 5 / 5 |
| 24 | 12 | `task=8000` | 306 / 389 | 414 / 398 | 427 / 1 441 | 0 / 1 | 65 / 60 | 76 / 678 | 97 / 64 | 5 / 5 |
| 80 | 8 | dafa32b binary | 763 / 760 | 2 121 / 2 091 | 2 758 / 2 739 | 118 / 154 | – | – | – | – |
| 80 | 8 | `base` | 637 / 689 | 2 104 / 2 112 | 2 754 / 2 752 | 74 / 91 | 677 / 677 | 688 / 686 | 676 / 676 | 7 / 7 |
| 80 | 8 | `prewake=1` | 485 / 455 | 2 097 / 2 074 | 2 751 / 2 747 | 51 / 46 | 675 / 674 | 678 / 677 | 676 / 675 | 7 / 7 |
| 80 | 8 | `prewake=all` | 565 / 513 | 1 494 / 1 487 | 2 129 / 2 135 | 82 / 85 | 672 / 673 | 676 / 678 | 683 / 682 | 7 / 7 |
| 80 | 8 | `workers=100000` | 691 / 592 | 2 108 / 2 085 | 2 757 / 2 711 | 120 / 58 | 679 / 677 | 693 / 679 | 677 / 675 | 7 / 7 |
| 80 | 8 | `spin=quarter` | 593 / 574 | 1 725 / 1 699 | 2 080 / 1 728 | 21 / 15 | 675 / 675 | 679 / 697 | 673 / 670 | 5 / 7 |
| 80 | 8 | `spin=period` | 285 / 281 | 328 / 414 | 354 / 476 | 0 / 0 | 3 / 3 | 22 / 26 | 28 / 27 | 7 / 7 |
| 80 | 8 | `task=1000` | 756 / 640 | 2 114 / 2 275 | 2 747 / 2 739 | 142 / 161 | 679 / 676 | 683 / 685 | 676 / 675 | 7 / 7 |
| 80 | 8 | `task=4000` | 747 / 760 | 2 110 / 2 111 | 2 732 / 2 753 | 145 / 137 | 679 / 679 | 681 / 703 | 677 / 678 | 7 / 7 |
| 80 | 8 | `task=8000` | 690 / 688 | 2 693 / 2 107 | 2 751 / 2 749 | 153 / 67 | 679 / 678 | 715 / 706 | 682 / 680 | 7 / 7 |
| 80 | 12 | dafa32b binary | 611 / 618 | 2 757 / 2 156 | 2 815 / 2 852 | 164 / 133 | – | – | – | – |
| 80 | 12 | `base` | 594 / 440 | 2 124 / 2 051 | 2 747 / 2 747 | 95 / 38 | 705 / 705 | 1 349 / 1 344 | 694 / 688 | 11 / 11 |
| 80 | 12 | `prewake=1` | 575 / 594 | 2 150 / 2 127 | 3 467 / 2 751 | 52 / 120 | 686 / 696 | 1 347 / 1 344 | 692 / 693 | 11 / 11 |
| 80 | 12 | `prewake=all` | 600 / 599 | 1 494 / 1 504 | 2 198 / 2 206 | 160 / 168 | 679 / 678 | 740 / 685 | 692 / 693 | 11 / 11 |
| 80 | 12 | `workers=100000` | 457 / 469 | 2 071 / 2 059 | 2 130 / 3 415 | 49 / 28 | 705 / 705 | 1 324 / 1 347 | 688 / 684 | 11 / 11 |
| 80 | 12 | `spin=quarter` | 365 / 578 | 1 693 / 2 072 | 2 355 / 2 367 | 12 / 31 | 690 / 691 | 1 062 / 1 347 | 678 / 694 | 11 / 11 |
| 80 | 12 | `spin=period` | 236 / 238 | 294 / 289 | 322 / 328 | 0 / 0 | 5 / 5 | 26 / 32 | 31 / 30 | 11 / 11 |
| 80 | 12 | `task=1000` | 431 / 465 | 2 061 / 2 126 | 2 745 / 2 753 | 32 / 56 | 696 / 694 | 1 323 / 707 | 685 / 690 | 11 / 11 |
| 80 | 12 | `task=4000` | 598 / 596 | 2 129 / 2 119 | 2 823 / 2 761 | 132 / 106 | 997 / 814 | 1 349 / 1 347 | 704 / 693 | 11 / 11 |
| 80 | 12 | `task=8000` | 594 / 588 | 2 114 / 2 136 | 2 170 / 2 773 | 140 / 97 | 809 / 1 292 | 1 275 / 1 348 | 698 / 699 | 11 / 11 |

**The rule, fixed before measuring:** an option is kept if, in both rounds, against the candidate's `base` row of the same run, on 8 and on 12 threads (the auto count): the p99 and the maximum fall, the median is at most 5 % higher, no more blocks go over the period, and the component it targets falls (workers started p99 for the spin, the early wake and the lane limit; slowest task p99 for the chain size).

- **Measured best in the sandbox, rejected on the device: `spin=period`**, workers that spin through the device period instead of parking after 50 µs. In the sandbox it was the only option that passed everywhere (24 and 80 voices, 8 and 12 threads, both rounds): every worker joined (11 of 11 on 12 threads, against 5 with `base` at 24 voices), the workers started within a p99 of 3–6 µs (43–705 µs with `base`), and the p99 per block fell 2.5–4× at 24 voices and 5–7× at 80 voices. **Operator measurement on the device** (2026-10-12, SSL 2+ at 96 kHz / 128 frames, workers in the audio workgroup, silent copies, 15 s): with the spin, 80 voices on 12 threads gave **4 060 xruns** (worst callback 12.4 periods) and 120 voices 5 397 (14.7), 80 voices on 4 threads 31 (7.6); without it (13 threads, which keep parking, or the default restored) 80 voices gave **0 xruns** (worst 0.37–0.38) and 120 voices 0 (0.51). Spinning real-time threads exceed their computation budget and are throttled; the sandbox has neither real-time scheduling nor a workgroup, so it could not show this. The default stays parking after 50 µs (`Schedule::realtime`); the variant remains measurable as `--rt-schedule spin=period` (`Schedule::spinning`). The 3 ms bound (`MAX_SPIN`) still caps any configured spin.
- **Measured and rejected:**
  - `prewake=1` passed only at 80 voices on 8 threads (with a p99 within 1 % of `base`), and failed on 12 threads and at 24 voices.
  - `prewake=all` lowered the p99 at 80 voices to 1.49–1.50 ms but put more blocks over the period (82–168 against 38–91), and did nothing at 24 voices.
  - `workers=100000` (fewer lanes for smaller blocks) made 24 voices slower (median 449–474 µs against 261–413 µs), joining 4 workers.
  - `spin=quarter` (333 µs, the time-constraint computation) passed only at 80 voices on 8 threads, and at 24 voices raised the maximum to 1.1–1.4 ms.
  - `task=1000`, `4000` and `8000` (the chain size, against 2 000 ns) changed nothing consistently.

  The code of `prewake` and `workers` was removed after this session; `spin` and `task` stay as bench schedules.
- **Measurement:** at 80 voices with `base`, the workers-started and slowest-task p99 are both about 676–705 µs (about half a period) on 8 and 12 threads, and their maxima about 1 345 µs (a whole period). The spin removes both. The slowest task, which is the task body alone, also falls from 50–99 µs to 27–35 µs at 24 voices.
- **Assumption, not measured:** a worker that parks lets its core go idle and slow down, and a task on it runs slower until it is fully running again. Spinning keeps the core running. The seat cannot read core frequencies or idle states.
- **Measurement (instrumentation cost):** the candidate's `base` rows against the dafa32b binary, both without the spin, differ within this session's noise. At 24 voices the p99 was 385–422 µs and 394–404 µs.

**All cores: the reason for the limit.** On 18 threads (every performance core) `spin=period` was far worse. At 24 and 80 voices, in each of four rounds (rounds 1–2 and two confirmation rounds of the first candidate), the p99 per block was 3.36–3.37 ms with 363–373 of 751 blocks over the period. With `base` it was 0.39–0.42 ms and 0 blocks at 24 voices, and 3.31–3.54 ms and 61–88 blocks at 80 voices. Workers started reached a whole period (about 1.34 ms). A spinning worker then holds a core that something else needs and is preempted for whole periods. So `Schedule::realtime` spins only up to the real-time auto count of threads (two thirds of the performance cores), the measured 8 and 12. With more threads, including counts between 13 and 17 that were not measured, the workers park after 50 µs as before.

**The shipped schedule** (rounds 5–6, 14:23–14:28 UTC, the final candidate: `--rt-schedule base --rt-schedule default`, alternating with the dafa32b binary as above). On 18 threads `default` is `base`:

| Voices | Threads | Schedule | Median | p99 | Max | Over the period | Workers started p99 | Workers started max | Slowest task p99 | Workers joined (median) |
|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 24 | 8 | dafa32b binary | 397 / 391 | 404 / 397 | 409 / 401 | 0 / 0 | – | – | – | – |
| 24 | 8 | `base` | 403 / 405 | 414 / 417 | 439 / 436 | 0 / 0 | 71 / 69 | 76 / 122 | 55 / 56 | 5 / 5 |
| 24 | 8 | `default` | 106 / 106 | 115 / 179 | 118 / 217 | 0 / 0 | 3 / 3 | 17 / 31 | 33 / 45 | 7 / 7 |
| 24 | 12 | dafa32b binary | 392 / 279 | 400 / 398 | 414 / 414 | 0 / 0 | – | – | – | – |
| 24 | 12 | `base` | 310 / 347 | 416 / 417 | 422 / 424 | 0 / 0 | 76 / 70 | 94 / 117 | 95 / 85 | 5 / 5 |
| 24 | 12 | `default` | 97 / 99 | 114 / 135 | 684 / 201 | 0 / 0 | 5 / 5 | 32 / 47 | 34 / 42 | 11 / 11 |
| 24 | 18 | dafa32b binary | 387 / 390 | 394 / 398 | 401 / 403 | 0 / 0 | – | – | – | – |
| 24 | 18 | `base` | 297 / 409 | 416 / 417 | 421 / 426 | 0 / 0 | 75 / 52 | 100 / 147 | 91 / 65 | 5 / 5 |
| 24 | 18 | `default` | 410 / 243 | 416 / 414 | 420 / 418 | 0 / 0 | 41 / 72 | 45 / 147 | 56 / 88 | 5 / 11 |
| 80 | 8 | dafa32b binary | 765 / 692 | 2 106 / 2 105 | 2 762 / 2 766 | 172 / 96 | – | – | – | – |
| 80 | 8 | `base` | 647 / 566 | 2 660 / 2 089 | 3 355 / 2 752 | 177 / 68 | 679 / 676 | 686 / 685 | 676 / 674 | 7 / 7 |
| 80 | 8 | `default` | 312 / 313 | 354 / 466 | 382 / 547 | 0 / 0 | 3 / 3 | 22 / 27 | 38 / 44 | 7 / 7 |
| 80 | 12 | dafa32b binary | 614 / 645 | 2 754 / 2 750 | 2 818 / 2 758 | 124 / 201 | – | – | – | – |
| 80 | 12 | `base` | 641 / 637 | 2 758 / 2 154 | 2 829 / 2 826 | 136 / 90 | 1 292 / 709 | 1 345 / 1 347 | 702 / 698 | 11 / 11 |
| 80 | 12 | `default` | 260 / 260 | 358 / 316 | 462 / 363 | 0 / 0 | 5 / 5 | 36 / 31 | 44 / 36 | 11 / 11 |
| 80 | 18 | dafa32b binary | 589 / 603 | 3 332 / 3 541 | 4 879 / 4 870 | 74 / 81 | – | – | – | – |
| 80 | 18 | `base` | 500 / 549 | 3 535 / 3 545 | 4 891 / 6 154 | 101 / 104 | 1 374 / 1 364 | 1 385 / 1 381 | 1 359 / 1 367 | 11 / 11 |
| 80 | 18 | `default` | 601 / 501 | 3 545 / 3 543 | 4 714 / 4 326 | 99 / 79 | 1 375 / 1 380 | 2 676 / 2 650 | 1 362 / 1 359 | 11 / 11 |

**Next to the device figures.**
- **Measurement (sandboxed, offline, no workgroup):** 80 voices on 12 threads, the device's load, went from a p99 per block of 2.15–2.76 ms (both builds without the spin) to 0.32–0.36 ms with the spin, which is 0.24–0.27 of the 1.333 ms period. The worst block went from 2.76–2.83 ms to 0.36–0.46 ms (0.27–0.35 of the period).
- **Not the same metric:** the device's worst callback is a 30-minute maximum on the real I/O thread inside the workgroup. The seat's numbers are 1-second rows on a shared machine.
- **Whether V12's worst < 0.25 now holds on the device is the operator's soak** (below). The seat cannot decide it either way.

Also not measured on the seat: the CPU the spin costs while playing, about `threads − 1` busy cores, which the operator reads during the soak.

**Measurement (not a device arrangement):** time-constraint workers beside an **ordinary** calling thread (`rt-workers`) stay bad, as in M2.6-3, and the spin does not fix it. At 80 voices (rounds 3–6), on 8 threads the p99 was 7.3–7.7 ms with `default` against 9.1–10.2 ms with `base`. On 12 threads it was 10.3–62 ms against 10.8–24.8 ms, with a worst block of 0.69 s: spinning real-time workers starve the ordinary caller even more. The CoreAudio driver never runs that arrangement, because the workers enter the time-constraint policy only around a real-time callback thread.

### Pending operator steps for the M2.6-5 checkpoint

On the Mac outside the sandbox, from the repository root, with `PATH=/opt/homebrew/opt/rustup/bin:$PATH` and `T=/tmp/signal-project-target`:

1. **The checks at the pinned target:**
   - `cargo fmt --all --check`
   - `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`
   - `cargo clippy -p signal-lab -p signal-cli --features lab --all-targets --locked --offline -- -D warnings`
   - `cargo test --workspace --locked --offline`
2. **The V12 soak (AC3):**
   - `cargo build --release --locked --offline -p signal-cli --target-dir "$T"`
   - `scripts/soak.sh --voices 80 --silent --signal "$T/release/signal" 30 128`, with the interface as the default output and no `--threads` (auto: 12 of 18; the workers park after 50 µs, M2.6-5 operator decision).
   - `--silent` sets the copy's `out` gain to 0.001 (−60 dB), as the operator's silent copies before.
   - Report the last lines verbatim: `xruns` (expected 0), `worst callback` against **0.405 / 0.408** (V12's target is < 0.25; record it as measured if not reached), `edits … written, … applied`, and the `scheduling` path (`workgroup`).
   - While it plays, read the CPU in a second terminal: `ps -o %cpu= -p "$(pgrep -n signal)"`. About 1 100 % is expected, 11 spinning workers.
3. **The device's tail (AC2):**
   - `"$T/release/signal" --json bench --realtime --rt-voices 80 --rt-schedule base --rt-schedule default > rt-m265-1.json`, then the same into `rt-m265-2.json`.
   - The `rt-all` rows on 8 and 12 threads compare the old and the new schedule outside the sandbox (still without the workgroup).
   - Add them as an operator row here.
4. **If the soak is worse than 0.408:** run the same soak with `--signal` pointing at a build of dafa32b in the same session, to separate the change from the day's conditions, and report both. The seat could not test whether the kernel demotes a time-constraint worker that spins past its computation (`docs/realtime-rules.md` "The policy"); a worse device tail would be the sign.

### Real-time wake policies (M2.7-1)

The policies (`park50`, the default, `budget-spin`, `semaphore` and `fanout`), the computation knob and the wake-up trace are in `docs/realtime-rules.md` "Wake policies". The device decides between them: the sandbox has no audio device, no workgroup and no real-time scheduling, and in M2.6-5 a policy that won 2.5–7× in the sandbox broke the stream up on the device.

**The harness.** `scripts/rt-matrix.sh` plays `signal play --json --rt-trace` on the default output with silent copies (`out` gain 0.001) of `signal bench --write-voices N` projects, for a fixed number of seconds per cell, over policies × threads × voices × computation budgets, unattended (stdin closed, a failed cell recorded as `error` and the run goes on). It writes `cells.csv` (one row per cell: xruns, worst, p99 and median callback as fractions of the period, wake latency median / p99 / max with the wakes seen of those sent, first and last join from the callback's start, slowest task p99, sampled demotions, what the workers blocked on, the scheduling path), `table.md` and each cell's JSON and log. Rows are labelled `device` (CoreAudio) or `sandboxed` (the null driver); budget-spin rows at a computation of 0.75 or more are flagged ⚠. **The default matrix:** 4 policies × threads 8, 12, 14 × voices 80, 120 × computation 0.25, 0.75 = 48 cells × (10 s + 4 s overhead per cell: start and load, device open and workers entering at most 1 s, stop at most 1 s, 1 s settle) = **672 s, 11.2 minutes** (`scripts/rt-matrix.sh --estimate` prints it from the defaults; the build and the voice projects come first).

**Seat run, SANDBOXED (shape only)**, 2026-10-12 22:53 UTC, devpanel seat, null driver, a release build of the candidate: `scripts/rt-matrix.sh --driver null --threads 2,4 --voices 8 --computation 0.25,0.75 --seconds 2`, 16 cells, every cell `ok`, 60 s wall time with an already-built release (the estimate said 96 s: the null driver opens no device). Its numbers mean nothing for a device (the null driver plays random callback sizes of 1 to 4 096 frames on ordinary threads) and are not recorded here; they decide nothing. `crates/signal-cli/tests/rt_matrix.rs` runs a two-cell null matrix on every `cargo test`.

**Test time (sandboxed, measured).** The determinism matrix of `crates/signal-cli/tests/realtime.rs`, now under every policy (C9), took 116.4 s on the seat against 53.1 s before (debug build, one run each).

### Pending operator steps for the M2.7-1 checkpoint

On the Mac outside the sandbox, from the repository root, with `PATH=/opt/homebrew/opt/rustup/bin:$PATH` and `T=/tmp/signal-project-target`, the SSL 2+ as the default output at 96 kHz:

1. **The checks at the pinned target:**
   - `cargo fmt --all --check`
   - `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`
   - `cargo clippy --workspace --all-targets --locked --offline --features signal-cli/lab -- -D warnings`
   - `cargo test --workspace --locked --offline`
2. **The build:** `cargo build --release --locked --offline -p signal-cli --target-dir "$T"`
3. **The default matrix** (about 11 minutes, unattended): `scripts/rt-matrix.sh --signal "$T/release/signal" --out ~/signal-rt-matrix-1`. Keep the folder; paste `table.md` below as an operator measurement.
4. **The confirmation** of the best candidate (the decision rule below), against `park50`, longer: `scripts/rt-matrix.sh --signal "$T/release/signal" --policies <best>,park50 --threads <best threads> --voices 80,120 --computation <best computation> --seconds 60 --out ~/signal-rt-matrix-2`. To check the SDK's statement that a computation up to 0.5 is raised to half the period (`docs/realtime-rules.md` "The policy"), add `--computation 0.25,0.5` once.
5. **The soak** with the choice: `scripts/soak.sh --voices 80 --silent --rt-policy <best> --rt-computation <best computation> --threads <best threads> --signal "$T/release/signal" 30 128`. Report its last lines verbatim (`xruns`, `worst callback`, `wake policy …`, `scheduling`).
6. **The decision rule, fixed before measuring:** at 80 and at 120 voices, the policy, thread count and computation with **0 xruns and a worst callback under 0.25** (V12, C15); if several, the lowest worst at 120 voices; if none, the lowest worst with 0 xruns; a tie goes to `park50`. The auto thread count is that row's thread count. Also read the wake components (which part of the tail a policy removed) and `sampled_timeshare` (a demotion seen).
7. **Record** the matrix, the confirmation, the soak and the decision here, labelled as operator measurements. Changing a default is then an operator change: the policy in `RtPolicy`'s `#[default]` and the computation in `Computation::DEFAULT` (`crates/signal-engine/src/wake.rs`), the auto count in `realtime_threads` (`crates/signal-engine/src/cores.rs`), each with `docs/realtime-rules.md`. Until then the default stays `park50` at 0.25 on two thirds of the performance cores.

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

### Pending operator steps for the M2.7-5 checkpoint

The seat ran every `signal-gpu` and lab GPU test on wgpu's noop device; the tests that need a GPU skipped with their reason. On the Mac outside the sandbox, as above:

1. `cargo test -p signal-gpu --locked --offline -- --nocapture`: `adapter_two_channels_of_one_shader_each_get_their_own_result` (the audit's finding 3: `[3, 7]`, `Ok`) and `adapter_an_invalid_dispatch_is_never_a_success` (finding 4: refused, or the job fails) must print `GPU: <name> (Metal)`, not `skipped`, and pass.
2. `cargo test --workspace --locked --offline --features signal-cli/lab -- --nocapture`: every test passes on the GPU, including `lab/tests/fault_causes.rs` and `lab/tests/device_loss.rs`, and the M2.6-4 tolerance lines are unchanged (at or below `1e-4`).
3. Steps 3–5 of the M2.6-4 list above with the M2.7-5 build: identical repeated renders, the bench row, and `signal play projects/gpu-partials` with `late_results` 0 on an idle GPU.

### Multi-GPU device pool (M2.7-2)

`realtime-rules.md` "GPU" (devices, placement, generations). Every GPU adapter is a device of a pool, with its own service thread; GPU-backed instances are placed on them deterministically, and `signal bench --gpu` measures the bank's round trip **per device** (`gpu_partials.devices[]`: `id`, `adapter`, `per_block.gpu_round_trip`, or `no_gpu` with the reason; the CPU reference and the audio side once, in `gpu_partials.per_block`; `gpu_partials.no_gpu` when there is no device at all). Only a `--features gpu` (or `lab`) build lists or uses GPUs: the default `signal`, and so the helper `scripts/bundle-app.sh` bundles, links no wgpu (AC2b; decision of 2026-10-14, made for the run by the top-level supervisor, to be confirmed or reversed by the operator). Signal.app's interface setup shows the same inventory read-only, under "Compute devices".

**`signal devices --compute`** lists the compute devices: text, or with `--json` this object:

```json
{
  "cpu": {
    "source": "sysctl",
    "performance_cores": 12,
    "levels": [
      { "index": 0, "name": "Performance", "physical": 12, "logical": 12 },
      { "index": 1, "name": "Efficiency", "physical": 6, "logical": 6 }
    ]
  },
  "gpu": {
    "built": true,
    "available": true,
    "reason": null,
    "adapters": [
      {
        "id": "metal-apple-m5-max", "index": 0, "name": "Apple M5 Max",
        "backend": "Metal", "type": "integrated", "vendor": 0, "device_id": 0,
        "driver": "", "driver_info": "",
        "limits": {
          "max_compute_workgroups_per_dimension": 65535,
          "max_compute_invocations_per_workgroup": 1024,
          "max_compute_workgroup_size": [1024, 1024, 64],
          "max_compute_workgroup_storage_size": 32768,
          "max_storage_buffer_binding_size": 4294967296,
          "max_buffer_size": 4294967296
        }
      }
    ]
  }
}
```

(The values are an illustration, not a measurement.)

| Key | Meaning |
|---|---|
| `cpu.source` | `"sysctl"` (`hw.perflevelN.*`), or `"fallback"` where `sysctl` does not answer (a sandbox): then one level `all` of `available_parallelism` cores |
| `cpu.performance_cores` | the cores of every level but `Efficiency`: what `threads = 0` derives from (ADR 0019 §2) |
| `cpu.levels[]` | `index` (`N` of `hw.perflevelN`, 0 the fastest), `name`, `physical` and `logical` cores |
| `gpu.built` | whether this build lists GPUs (`--features gpu` or `lab`); `false` in the default build |
| `gpu.available` | whether any adapter is listed |
| `gpu.reason` | why none is, when none is (`null` otherwise) |
| `gpu.adapters[]` | every Metal adapter, sorted by `id`: `id` (stable: `<backend>-<name>`, then `-<PCI bus id>` where the backend reports one, then `-2`, `-3`… for adapters still alike; an assumption, as wgpu exposes no Metal registry id), `index` (its place in this list; a project's `gpu = "<index>"` counts the devices the Setup allows instead), `name`, `backend`, `type` (`integrated`, `discrete`, `virtual`, `cpu`, `other`), `vendor` and `device_id` (PCI ids, 0 where the backend has none), `driver`, `driver_info`, and the compute `limits` above, in bytes where sizes |

**Seat result** (the devpanel seat, 2026-10-14; sandboxed, no GPU, no `sysctl`): a seat result, which decides nothing. The default build's `signal --json devices --compute` reports `cpu.source` `"fallback"` and `gpu.built` `false`; the `--features lab` build reports `gpu.built` `true`, `available` `false` with the reason "no Metal adapter (no usable GPU on this machine)", and `signal --json bench --gpu` reports no device (`devices: []`, `no_gpu` with that reason). Every multi-device test ran on wgpu's noop device (3 noop devices; noop computes nothing, so its audio is silence: those tests prove the job path, the placement, the loss and the replacement, not GPU arithmetic).

### Pending operator steps for the M2.7-2 checkpoint

On the M5 Max outside the sandbox, from the repository root, with `PATH=/opt/homebrew/opt/rustup/bin:$PATH` and `T=/tmp/signal-project-target`. With one adapter every placement is that adapter; the multi-device logic is the noop tests', run here on Metal too (AC10).

1. **Inventory:** `cargo build --release -p signal-cli --features lab --locked --offline`, then `"$T/release/signal" devices --compute` and `"$T/release/signal" --json devices --compute`. Expect `cpu.source` `sysctl` with the Performance and Efficiency levels, and one adapter, Metal, with its id (expected `metal-apple-m5-max`), type and limits. Report the JSON verbatim. Then open Signal.app (`./start.sh projects/kick`), the interface setup, "Compute devices": the same lines as the text output.
2. **The lab GPU tests:** `cargo test -p signal-gpu -p signal-lab --features signal-lab/lab --locked --offline -- --nocapture` and `cargo test -p signal-cli --features lab --locked --offline`. Expect `GPU: Apple M5 Max (Metal)` (not `skipped`), every test passing (the pool, placement, split and loss tests run on noop devices there as here; the adapter tests on Metal), and the M2.6-4 tolerance lines at or below `1e-4`.
3. **The bench per device:** `"$T/release/signal" --json bench --gpu --seconds 2`, twice: `gpu_partials.devices` has one entry, `metal-apple-m5-max`, with `per_block.gpu_round_trip` (expected near the M2.6-4 row: about 0.9 ms median per block). Add an operator row.
4. **Placement recorded and repeatable:** `"$T/release/signal" render projects/gpu-partials --seconds 10 --out /tmp/gpu-a`, the same to `/tmp/gpu-b`: the two `manifest.json` have the same `audio_fnv1a64`, `log_fnv1a64`, `compute_devices` (the one adapter) and `placement` (`/bank` on it). Then a copy of the project with `gpu = "0"` on `bank`: the same hashes, and `placement[0].pinned` is `"0"`.
5. **The Setup's limit:** a Setup with `gpu_devices = []` (`"$T/release/signal" --setup /tmp/no-gpu.toml render projects/gpu-partials --seconds 2 --out /tmp/gpu-none`): the summary says the Setup limited the devices to none, `placement[0].device` is `null`, the audio is silence and `log.jsonl` holds one `late_result` per due block.
6. **Real time unchanged:** `"$T/release/signal" play projects/gpu-partials --seconds 60 --json` with the interface as the default output: `xruns` 0 and `late_results` 0 on an idle GPU, as at the M2.7-5 checkpoint. A device loss cannot be provoked on the device; it is the noop tests' (`lab/tests/placement.rs`).

### Validation pairs (M2.10-4)

ADR 0027 §4 "Validation": each std type rebuilt from core primitives as a library composite (`signal_lib::VALIDATION`) against its native reference. The exactness column is what `crates/signal-lib/tests/validation.rs` asserts and measures (noise, saw sweep, impulses, parameter steps and audio-rate modulation, at 48 and 96 kHz; `cargo test -p signal-lib --test validation -- --nocapture` prints every comparison). The cost columns are a **measurement on one host**, not a threshold, and they inform; whether a composite is fit for a use is the owner's decision (AGENTS.md rule 6).

**Method (render-based).** `scripts/validation-cost.sh [seconds] [runs]` writes temporary projects at 48 kHz: a base (a `std.noise` into `std.audio_out`; for the envelope, also a 6 Hz square gate) and the base plus 32 instances of the native type or of its composite, each fed by the noise (the envelope by the gate), at their defaults (the shaper at drive 0.5; `std.osc` at shape 0, the sine `lib.osc_core` rebuilds, a setting of the native's alone: the script's `pair` takes such settings separately, and before the M2.10-4 review it left `std.osc` at its default saw) and with their outputs open (every instance still runs). It times `signal render <project> --seconds <s> --threads 1 --block 256` with `/usr/bin/time -p`, takes the median of the runs, and reports (time − base time)/(32 · frames) per instance and frame. Release build (`cargo build --release --locked --offline -p signal-cli`, the script builds it unless given `--signal`).

**Seat measurement**, 2026-10-18 01:33–01:37 UTC, from a devpanel seat, of the M2.10-4 candidate uncommitted on b481fdb (the delivery's parent is 30a9d51, whose engine, `signal-cli` and `signal-std` code are b481fdb's: the commits between change tests, docs and one comment): `scripts/validation-cost.sh 5 3`. Machine (from `system_profiler SPHardwareDataType` and `sw_vers`; `sysctl` is refused in the seat): Mac Studio (Mac17,14), Apple M5 Max, 18 cores (6 Super and 12 Performance), 64 GB, macOS 27.0.1. Conditions: shared with other work and with this issue's own test build at the start (load average 8.1 at the start, 6.0 near the end), not pinned to a core. `/usr/bin/time` resolves 0.01 s, which over 32 × 240 000 frames is 1.3 ns per instance and frame, so the native figures below about 5 ns are at the method's resolution, and the ratios over them are rough. The operator confirms the figures on the device with the same command.

| Pair | Exactness or tolerance (measured largest error) | Native ns/frame | Composite ns/frame | Ratio |
|---|---|---:|---:|---:|
| `std.svf` ↔ `lib.svf_core` (fixed and modulated cutoff) | bit-exact, every output | 15.6 | 740.9 | 47.5 |
| `std.onepole` ↔ `lib.onepole_core` | bit-exact, both outputs | 5.2 | 278.6 | 53.6 |
| `std.lag` ↔ `lib.lag_core` | ≤ 1e-9 (9.7e-17; bit-exact in 6 of 7 cases) | 2.6 | 384.1 | 147.7 |
| `std.adsr` ↔ `lib.adsr_core` | ≤ 1e-9 (2.4e-14; bit-exact on one-frame gates) | 5.2 | 2 668.0 | 513.1 |
| `std.osc` (sine) ↔ `lib.osc_core` (naive) | bit-exact (sine); the naive saw differs by ≤ 1 only within one phase step of a wrap | 11.7 ¹ | 109.4 ¹ | 9.4 ¹ |
| `std.delay` ↔ `lib.delay_core` (buffer) | bit-exact at whole-frame delays; ≤ 1e-9 × the input's peak at fractional and audio-rate delays (2.5e-11 × peak on short delays, noise at ±0.5 and ±100; 3.4e-10 × peak at 959 940 frames past 10⁶ frames); 1 < d < 2 differs by design (6.2e-2 on noise at ±0.5) | 52.1 | 132.8 | 2.5 |
| `std.tanh_shaper` ↔ `lib.tanh_shaper_core` (no ADAA) | ≤ 1e-9 + 0.035·Δu² per frame, at drive 0.1 to 1 only (drive 0.1: 4.1e-5; drive 0.5: 4.7e-3; drive 1: 2.8e-2, on impulses, where Δu = 1; noise 6.2e-6, 7.6e-4, 5.7e-3; at drive 5 and 20 on full-scale noise 0.24 and 0.73, no meaningful comparison) | 10.4 | 57.3 | 5.5 |

¹ Re-measured at the M2.10-4 review with `std.osc` at shape 0: the first measurement left it at its default, the band-limited saw (2.6 ns native, 117.2 ns composite, ratio 45.1), so it timed the saw against the composite. Same host, 2026-10-18 03:12 UTC, release build of the delivery after its review fixes, uncommitted on 30a9d51, `scripts/validation-cost.sh --only osc_sine 5 5` and again `… 10 7`, the same figures both times; load average 7.1 then 6.4 at the start and 6.7 and 6.6 at the end (shared with other work, not pinned to a core). The sine's `libm::sin` is most of the native's cost.

**Why each pair is exact or not** (also in each composite's file):

- **Bit-exact.** svf and onepole compute the native's coefficients (`tan(π·fc/sr)`, `1/q`, `1/(1 + k·g + g²)`) and recurrence with the same operations in the same order, the flush to 0 below 1e-9 included (abs, compare, select). The oscillator's sine is `sin(2π·(φ + pm))` on its own accumulator `φ ← fract(φ + f/sr)`, the native's formula; `std.phasor` (which multiplies by 1/sr) and `std.sin_cycles` (another reduction) would not be. The delay writes the input at the frame count into a `std.buffer1m` of 10⁶ frames and reads it with the same Hermite interpolator; at whole-frame delays both read the same sample, frame 0 included (the delay time passes a unit delay before its clamp, so the first frame reads an unwritten slot, 0, as the native's first output).
- **Operation order (lag, envelope, fractional delay).** The engine bounds every signal at ±1e6 (`docs/semantics.md` §7), the composites' intermediates included. τ = ms·sr/1000 frames and the product ms·sr pass 10⁶ beyond about 20 ms at 48 kHz, and the envelope's frame counts in a long decay pass it too, so the lag computes −1/τ as (−1000/sr)/ms, the envelope its decay and release as the recursion E ← E·e^r instead of the native's closed form e^(−t/τ) (its rounding grows with t, about 3e-10 at worst), and a flush to the target or to 0 can fall one frame apart (at most the flush threshold, 1e-9). The envelope's attack length is the native's exactly: attack·sr can pass 10⁶ too, so it is computed scaled by 2⁻¹⁶, attack·(sr·2⁻¹⁶)/(1000·2⁻¹⁶), which rounds as (attack·sr)/1000 bit for bit (a power of two scales a product and its rounding alike); `adsr_core_attack_length_rounds_as_the_native` checks attacks at half-integer frame counts and one ulp either side at both rates (largest error 8.9e-16; review of M2.10-4: computed as attack·(sr/1000), 0.6770833333333333 ms at 48 kHz rounded to 33 frames instead of 32, an error of 0.03). The fractional delay's position p − ⌊p⌋ and time·(sr/1000) round differently from the native's 1 − frac(d − 1) and (time·sr)/1000: the read moves by about an ulp of a position up to 10⁶ (about 1e-10 of a frame), and the error is that times the signal's slope, so it scales with the input and grows with the frame count; the tolerance is stated relative to the input's peak (review of M2.10-4: noise at ±100 gave 1.7e-9, past an absolute 1e-9).
- **Expected differences, by design.** The oscillator has no polyBLEP, so its saw is not a reference for `std.osc`'s band-limited saw (nor square or triangle): they agree bit for bit except near each wrap, where the difference is the polyBLEP residual, up to 1. The shaper has no ADAA: it takes tanh of the midpoint (u[n] + u[n−1])/2, which keeps the native's half frame of delay; the native's first-order ADAA is the mean of tanh over the step, which differs from the midpoint by the curvature term (f″(ξ)·Δu²/24 for some ξ in the step, |tanh″| ≤ 4/(3√3), so at most 0.0321·Δu²; the suite asserts 0.035·Δu² + 1e-9), so the error grows with drive and with the step, and the comparison means something at low drive only (and the native takes exactly the midpoint form when |Δu| < 1e-6). Between 1 and 2 frames of delay the native replaces the current input, which a state-output primitive cannot read, by the frame before; the composite reads the current input.

**Range and domain differences** (none at 8 to 192 kHz for the filters; none up to 99 999 Hz for the delay, which holds 10 s there): every intermediate of a composite is a signal, so where a native's internal state is unbounded a composite must stay within ±1e6. The envelope's attack is at most 999 999 frames (20.8 s at 48 kHz, 10.4 s at 96 kHz, 5.2 s at 192 kHz; `std.adsr` allows 60 s); the delay holds at most 999 990 frames above 99 999 Hz (5.2 s at 192 kHz); the filters' cutoff is capped at 318 309 Hz (only above 649 610 Hz, where 0.49·sr exceeds it); above 999 999 Hz `std.sample_rate` itself faults (all but the shaper read it). Beyond ±1e6 an intermediate faults where it is a `std.add` or `std.mul` and is clamped silently where it is one of the M2.10-1 types that bound their output (`std.sub`, `std.div`, `std.tan`, `std.exp`, `std.scale`, `std.clamp`), so near the bound a composite can fault, or clamp, where the native does neither. The input amplitude each takes without a fault (noise at its defaults, measured by the suite's `HEADROOM` lines, which also compare the outputs where nothing faults): 10⁶ for the envelope (its gate) and the delay, as their natives; 10⁴ for the filters (their states are about twice their outputs), the lag (in − out), the oscillator's phase input (|pm| up to about 159 000 cycles) and the shaper (drive·in, and the sum of two frames). Up to there the filters, the oscillator and the delay stay bit-exact, the lag and the envelope within their tolerance; a fault mutes the instance and every output stays within ±1e6.

**Reading the figures** (measurement, no verdict): the acyclic composites cost a few times their natives (the delay 2.5×, the shaper 5.5×); the ones with a one-sample loop cost about 50 to 150 times (the loop's members run one frame at a time, below), the oscillator 9.4 times, its one-node loop beside a native whose `sin` already costs 11.7 ns, the envelope, a stage machine of about 120 instances almost all in its loop, about 500 times. ADR 0027 §2 and §3 anticipated this split.

### The cost of cycle units (M2.10-4)

Where the cost of a node in a one-sample loop goes (ADR 0027 Context: about 22 ns per node and frame), **measured on one host** (the machine above, 2026-10-18 01:28–01:37 UTC, shared, load average 4–20 during the runs), release builds, one thread. Three methods:

**1. Graph timings** (`scripts/validation-cost.sh`, second part; the cost per node and frame of a chain of *N* instances from a noise source, open or closed into a loop through one `std.unit_delay`, median of 3 renders of 5 s at 48 kHz; aggregate figures):

| Graph | `--block 256` | `--block 1` |
|---|---:|---:|
| 100 × `std.add`, acyclic chain | 2.5 | 24.6 |
| 100 × `std.add`, loop | 24.6 | 24.2 |
| 1000 × `std.add`, acyclic chain | 2.7 | 26.3 |
| 1000 × `std.add`, loop | 25.3 | 24.9 |
| 1000 × `std.select`, loop (3 inputs) | 23.6 | 24.0 |

- **Measurement:** an acyclic chain run one frame at a time (`--block 1`) costs what a loop costs (about 24 ns per node and frame), and a loop costs the same at `--block 256` as at `--block 1`. So the loop's cost is the per-call cost of running a node for one frame, not anything specific to cycles: a cycle unit runs its members in sub-blocks of one frame (`docs/semantics.md` §13), so every call pays the fixed per-call work that a block of 256 frames shares among 256 frames (the 2.5 ns of the acyclic chain at `--block 256`). A 3-input `std.select` in the loop costs no more than a 2-input `std.add`: the port count is not the driver at this size.

**2. Sampling profile.** `sample <pid> 3` (macOS, 1 ms interval) of `signal render` of the 1000 × `std.add` loop (`--threads 1`, 20 s), release build: 2 545 samples, self time by function on the one-thread path (`whole_unit` → `whole_direct`, `crates/signal-engine/src/engine.rs`):

| Function (self time) | Samples | Share |
|---|---:|---:|
| `commit` (the fault guard over the call's outputs and events, then each output written to the plan's rows with the fade handling) | 889 | 34.9 % |
| `whole_direct` (inlined: the instance's checks, `load_call` copying inputs and parameters into the call buffers, `start_call`, building `Inputs`/`Outputs`) | 644 | 25.3 % |
| `gather` (summing the input's sources, events, parameters: base advance, staged events, effective value) | 481 | 18.9 % |
| `caught` (the `catch_unwind` closure around `process`, with the `dyn` call) | 185 | 7.3 % |
| `Inputs::signal`, `Outputs::signal` (not inlined across crates) | 193 | 7.6 % |
| `keep_events` | 78 | 3.1 % |
| `std.add`'s `process` (the arithmetic) | 51 | 2.0 % |
| `whole_unit` (the cycle's per-frame loop over its members) | 22 | 0.9 % |

**3. Instrumented timing.** `docs/measurements/m2.10-4-cycle-timing.patch`, applied to a throwaway copy only (never to the delivered engine), times sections of `whole_direct` and of the cycle loop with `std::time::Instant`, one section per run so the others are not perturbed, for one-frame calls; an empty section measures the timer itself (14.4 ns per pair on this host, subtracted). On the 1000 × `std.add` loop (5 s at 48 kHz; per node and frame, timer subtracted): checks and view 0.8 ns, `gather` 2.8 ns, `load_call` and `start_call` 1.4 ns, `guarded` with `process` 3.6 ns, of which `process` (the `dyn` call and the primitive) 3.5 ns (so the panic guard costs about 0.1 ns), `commit` and `keep_events` 4.6 ns: 13.2 ns of the 24.0 ns a member costs in the instrumented cycle frame. The remaining 10.8 ns is outside the timed sections: the call of `whole_direct` itself and the code between the sections, and, an **assumption**, the cache misses of touching each instance's slot, parameters and rows once per frame, which the sections only partly contain. The `std.select` loop, in the same order (timer 16.0 ns there): −0.3, 1.8, 0.6, 6.3 (`process` 2.9), 5.9 ns, of 27.4 ns in its cycle frame; the negative figure shows that the timer's own cost varies by about ±1–2 ns between runs (14.4 against 16.0 ns), which is the uncertainty of each section figure. The timer's coarseness (24 MHz ticks, 41.7 ns) is averaged over 240 million calls per section; the residual is the least certain figure.

**Findings** (measurement; no change was made, AC7):

- The ~24 ns of a loop node is the engine's fixed per-call work at *k* = 1: by both the profile and the timings, the fault guard and output commit (about a third), the gather (about a fifth), the call setup and the accessors; the panic guard's share is within the timings' uncertainty (0.1 ns in the `std.add` run, 3.4 ns in the `std.select` run, against ±1–2 ns) and 7 % of the profile with the `dyn` call inside it; the primitive's own arithmetic is a few percent. Dispatch (the `dyn` call) is inside `process` and is small next to the rest.
- Nothing is specific to cycles: `whole_unit`'s per-frame loop is under 1 %, and an acyclic node at `--block 1` costs the same.
- **Options**, each for a later issue to design and prove bit-identical against the oracle and the determinism matrix: a one-frame specialisation of `commit` and `gather` for cycle members (their general per-frame loops, row-indexed writes, fade and event handling serve one frame here); inlinable `Inputs`/`Outputs` accessors across crates; skipping the gather's staged-event and ramp work for a parameter with none pending. None was implemented in M2.10-4: each changes the call path every render takes, and the bit-identity of all goldens and the oracle would have to be shown for it; the measurements above say how much each could save at most (a few ns of the 24). Beyond the per-call model, ADR 0027's fused core-composite compiler (about 2× native, measured there) remains the larger lever.

The operator confirms these figures on the device: `scripts/validation-cost.sh` for the graph timings, `sample` on a release `signal render` of a loop project for the profile, and the patch's header for the instrumented timing.

### Idle cost (M2.10-5)

**Superseded rule, kept as measured.** Everything in this section was measured on the M2.10-5 build, whose budget was "the fewest threads within 2 % (`BUDGET_SLACK`) of the shortest simulated schedule", without hand-off or wake costs. That rule never shipped. The engine's rule is the cost-aware one with the real-time share rule ("Idle cost, part 2 (M2.10-6)" below, `docs/realtime-rules.md` "The worker budget"). So the budgets here (studio 11 of 12, kick and pulse 3, physical-av 4, 4 voices 9) are that build's, not the engine's; the process-CPU method, the variance findings and the elapsed-time counters still apply.

Where the CPU of a played project goes when its plan has little parallel work, and what the worker budget changes (`docs/realtime-rules.md` "Worker threads"). The starting point is the operator's measurement of 2026-10-17. On the M5 Max, release build, `signal play projects/studio --driver null --seconds 20` read with `ps %cpu` after 8 s gave:

| `--threads` | CPU |
|---|---|
| 1 | 26 % |
| 2 | 50 % |
| 4 | 75 % |
| default (12) | 116 % |

The studio's DSP is about 8 % of a core ("The studio project" above).

**Every figure below is a measurement on one host, for the operator to confirm on the device. None is a threshold, and none is a real-time claim.** The null driver runs on ordinary threads, without the audio workgroup or time-constraint policy.

**Method.**
- Machine: the devpanel seat on the operator's Mac Studio (Mac17,14, Apple M5 Max, 18 cores, 64 GB, macOS 27.0.1; `sysctl` is refused in the seat, so the machine fields come from the rows above).
- Builds: release builds of the base (a4deadc, from `git archive`) and of the candidate (a4deadc plus M2.10-5, uncommitted), both `cargo build --release --locked --offline -p signal-cli`.
- Project: a copy of `projects/studio` (48 kHz). Runs are `signal play <copy> --driver null --seconds S --threads T [--rt-policy P] [--buffer B]`, with stdin held open by `sleep … |`.
- CPU: user + sys from `/usr/bin/time -p` at S = 5 and S = 25. The figure is their difference over the 20 s between them, in % of one core per second of audio, which leaves start-up out. `ps %cpu` is a decaying average that includes start-up, so it reads higher.
- Order: base and candidate alternate cell by cell. Each matrix ran twice (runs 1 and 2, 07:38–08:22 UTC).
- Conditions: shared with other sessions, load average 3–25 (each cell's is in the run log), not pinned. A niced (`nice -n 19`, `-j 4`) test build ran beside part of run 2.
- No script is added: each cell is the command above, timed twice, and the operator repeats them with `ps` or `/usr/bin/time`.

**Why `--threads 1` costs 26 % for an 8 % workload.** Candidate, studio, one thread, three repeats, 09:08–09:20 UTC, **on an idle host** (load average 1.3–1.8). Each row is CPU per second of audio, between the 5 s and 25 s runs:

| Run | Repeats | What it contains |
|---|---|---|
| `signal render --threads 1 --block 128` | 7.3, 7.2, 7.4 % | the DSP, the engine's per-block work, the WAV and the log |
| `signal render --threads 1 --block 1` | 31.6, 32.3, 32.4 % | the same, one frame per call (the per-call cost, "Block sizes") |
| the empty project (`std.audio_out` alone), `--block 128` | 0.1, 0.1, 0.1 % | the base of the render rows |
| `signal play --driver null --fast` | 7.3, 7.3, 7.3 % | the null driver's seeded callback sizes (cut to 128-frame chunks), the runner and a control step per chunk, unpaced |
| `signal play --driver null` (paced) | 26.3, 26.2, 27.1 % | the same, with a `thread::sleep` after each chunk until it is due (375 per second at 128 frames and 48 kHz) |

- **Measurement.** The callback sizes, the runner and the control step cost nothing measurable over the render: 7.3 % both.
- **Measurement.** The paced run does the same work and costs 3.6 times the CPU time. The pacing loop itself is one `Instant::now` and one `thread::sleep` per chunk; the sleeping is not counted as CPU.
- **Assumption, not measured.** After each sleep the thread resumes on an efficiency core, or on a performance core at a low clock, because on an otherwise idle Mac the system sees a thread busy for about 7 % of the time. The same instructions then take about 3.6 times longer. The seat cannot read core placement (`powermetrics` needs root); the operator can, with `sudo powermetrics --samplers cpu_power` beside the run.
- On the busy host of the matrix below, the same one-thread run read mostly 8–11 % and sometimes 27–29 %, which fits the same reading: with the performance cores busy, the scheduling differs.

**The variance is the second finding.** At one thread the base and the candidate run the same code (no worker, no budget), yet the busy host read 27.0 % in one run and 9.8 % in the next. "Repeats" below gives five more alternations of the deciding cells.

CPU per second of audio, studio, % of one core, base → candidate (run 1 / run 2):

| Cell | Base | Candidate |
|---|---:|---:|
| `--threads 1`, park50, `--buffer` 128 | 27.0 / 9.8 | 6.5 / 8.2 |
| `--threads 1`, `--fast` (unpaced) | 9.8 / 8.3 | 7.7 / 9.6 |
| `--threads 2` | 34.0 / 15.8 | 22.3 / 17.9 |
| `--threads 4` | 57.4 / 38.0 | 60.4 / 33.7 |
| default (12 threads), park50 | 93.2 / 73.5 | 90.0 / 62.3 |
| default, budget-spin | 136.0 / 86.8 | 108.6 / 87.8 |
| default, semaphore (parks: no Mach waiter on the null driver) | 94.7 / 65.5 | 83.3 / 61.5 |
| default, fanout | 73.6 / 65.4 | 63.1 / 67.1 |
| `--threads 1`, `--buffer` 32 | 16.8 / 10.7 | 13.7 / 9.6 |
| default, `--buffer` 32 | 147.9 / 136.4 | 122.0 / 108.6 |
| `--threads 1`, `--buffer` 512 | 9.4 / 8.0 | 10.0 / 9.0 |
| default, `--buffer` 512 | 51.9 / 56.1 | 51.4 / 49.8 |
| kick, `--threads 1` | 1.9 / 3.4 | 2.1 / 2.1 |
| kick, default | 1.6 / 1.7 | 1.9 / 2.8 |

**Repeats** (08:23–08:42 UTC, five more alternations of the deciding cells, load average 4–29):

| Cell | Base | Candidate |
|---|---|---|
| `--threads 1`, paced, `--buffer` 128 | 10.2, 11.3, 9.0, 8.5, 8.2 | 29.1, 9.4, 8.3, 8.5, 8.3 |
| `--threads 1`, `--fast` | 14.4, 7.8, 10.4, 8.5, 8.2 | 11.6, 7.8, 8.1, 8.4, 7.9 |
| default (12 threads), paced | 66.2, 64.6, 71.8, 71.5, 67.3 | 67.0, 66.0, 71.9, 70.1, 68.0 |

- At one thread the two builds are the same code, and the paced figure is bimodal: mostly 8–11 %, sometimes 27–29 % (3 of the 14 one-thread paced readings).
- At the default the candidate is within 1.4 points of the base in every alternation. **On this host, giving the studio 11 threads instead of 12 saved no measurable CPU.** The two-run matrix above, whose cells were further apart in time, read the candidate 3–11 points lower; the repeats do not confirm that.

**On the idle host** (09:22–09:26 UTC, load average 1.7–2.5, two alternations), where the operator measured:

| Cell | Base | Candidate |
|---|---|---|
| default (12 threads), paced | 93.4, 96.9 | 92.7, 91.5 |
| `--threads 1`, paced | 27.3 | 27.9 |

This is the operator's picture: 26 % at one thread, and about 116 % with `ps` after 8 s, which counts start-up too. The candidate is 0.7 and 5.4 points below the base here, within the spread of the busy host's repeats.

**The worker budget of each project** (`signal play … --rt-trace --json`, `wake.worker_budget`; deterministic, so one run each):

| Project | `--threads` 2 | 4 | 12 (default) | 18 |
|---|---:|---:|---:|---:|
| kick | 2 | 3 | 3 | 3 |
| pulse | 2 | 3 | 3 | 3 |
| physical-av | 2 | 4 | 4 | 4 |
| studio | 2 | 4 | 11 | 15 |
| 80 kick voices | 2 | — | 12 | 18 (and 8: `crates/signal-cli/tests/blocks.rs`) |

**Why the studio keeps 11 of 12.** Its estimated schedule is wide: 151 tasks, about 2 055 ns of estimated work per frame. The budget rule's simulated segment time per frame, on 1 to 12 threads, is:

| Threads | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| ns | 2 055 | 1 169 | 881 | 732 | 644 | 587 | 553 | 520 | 500 | 479 | 470 | 461 |

This curve comes from a scratch build that printed it, never delivered. The 11 is within 2 % of the 12; 8 threads are 13 % slower, 6 are 27 % and 4 are 59 %. The estimate has no per-task cost, so the simulated gain from the eighth thread on is a fraction of a microsecond per 128-frame segment. The real segments are short, so wake hops and claims are not negligible there.

**Where a worker's time goes: elapsed time, not CPU.** The trace's per-worker counters (`--rt-trace`; `wake.job_ms`, `spin_ms`, `parks`, `worker_task_ms` in `signal --json play`) are **elapsed (wall-clock) durations**, read with `Instant`, not CPU time:
- **job:** from joining a job to leaving it;
- **task:** the task bodies the workers ran;
- **job − task:** the rest of the job. It includes waiting on the task board for a task to become ready (a joined worker spins there until every task is claimed: `board.rs`, `participate`), waking two more workers, claiming tasks, and any time the system deschedules the worker;
- **idle spin:** from the end of a job or a resume to the next job or the block (the 50 µs of park50, or `budget-spin`'s budget);
- **parks:** how often it blocked;
- **wake:** from a wake to the worker running.

An elapsed interval in which the worker was descheduled consumed no CPU. So these figures say where a worker's time goes, not how much CPU each part costs. The process total is the only CPU measurement here (`/usr/bin/time`, above), and the seat has no per-thread CPU reading.

Candidate, studio, default threads (budget 11: 10 eligible workers), 20 s (10 195 callbacks at `--buffer` 128), one run per row, 10:22–10:25 UTC, load average 2.7–3.3. One host, null driver, **not real time**. Totals are over the 10 eligible workers; "per callback" divides by the callbacks and by the 10 workers.

| Policy, `--buffer` | job | task | job − task | idle spin | parks | job per callback per worker | idle spin per callback per worker | wake median |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| park50, 128 | 13.30 s | 5.31 s | 7.99 s | 2.02 s | 29 732 | 130 µs | 20 µs | 25 µs |
| budget-spin, 128 | 12.96 s | 5.24 s | 7.72 s | 9.70 s | 33 228 | 127 µs | 95 µs | 164 µs |
| semaphore (parks: no Mach waiter on the null driver), 128 | 13.29 s | 5.51 s | 7.78 s | 2.22 s | 30 825 | 130 µs | 22 µs | 31 µs |
| fanout, 128 | 13.24 s | 5.49 s | 7.75 s | 2.94 s | 45 689 | 130 µs | 29 µs | 57 µs |
| park50, 32 (36 895 callbacks) | 16.26 s | 5.87 s | 10.40 s | 6.27 s | 96 301 | 44 µs | 17 µs | 25 µs |
| park50, 512 (3 016 callbacks) | 20.38 s | 6.77 s | 13.61 s | 0.87 s | 10 575 | 676 µs | 29 µs | 115 µs |

In every row the ineligible eleventh worker joined no job and parked once.
- **Measurement (elapsed).** The eligible workers spend most of their elapsed time inside jobs but outside task bodies: 7.7–8.0 s of 20 s at every policy at 128 frames, 10.4 s at 32 frames and 13.6 s at 512.
- **Measurement (elapsed).** The idle spin is the smaller term: about 2–3 s, except under `budget-spin`, whose longer spin (0.9 × the computation budget) gives 9.7 s.
- **Not measured:** how much of "job − task" and of the idle spin was spent running rather than descheduled, and so how much CPU each costs. The process total of the same configuration is 62–97 % of a core (the tables above).
- **Assumption, from the code.** "Job − task" is mostly the spin on the task board, because a joined worker spins until the job's last task is claimed. The idle spin is all spinning while it runs.

**A narrow plan, base against candidate (AC2).** 4 kick voices (`signal bench --write-voices 4`, 96 kHz) at 12 threads. The candidate gives it a budget of 9: 8 eligible workers.
- **Process CPU** (`/usr/bin/time`, 25 s minus 5 s, `play --driver null --threads 12 --buffer 128`, park50; base a4deadc and candidate alternating three times; 10:26–10:32 UTC, load average 1.6–2.7): base 61.3, 63.5, 62.5 %; candidate 55.8, 56.8, 57.4 % of a core, about 6 points (10 %) less. Auditor-2's independent run of the same A/B at load average 4.6–7.7 read base 59.0, 59.8, 52.8 % against candidate 37.1, 40.2, 38.1 %.
- **Trace** (candidate, `--rt-trace`, 20 s): the 3 ineligible workers joined no job and parked once each. Over 20 329 callbacks, the 8 eligible workers spent 7.00 s in jobs (3.67 s in task bodies) and 2.65 s in the idle spin, all elapsed.
- **In one binary** (`signal --json bench --realtime --rt-voices 4 --rt-schedule base --rt-schedule default --seconds 2`; `base` = every worker, `default` = the budget; paced 128-frame blocks; not real time). At 12 threads, ordinary variant:
  - `base`: every one of the 11 workers spins 42–47 ms and parks 800–845 times, the last six with 8–22 ms of job time;
  - `default`: the budget is 9, and workers 9–11 show 0 ms of job, 0 ms of spin and 1 park each, while the others are as before.
  - The median block was 113.1 against 114.3 µs. The `rt-all` variant shows the same pattern (median 130.0 against 122.7 µs).

**What it means** (measurement and reading, no verdict):
- **The per-thread overhead.**
  - For the studio, most of a worker's elapsed time outside its tasks is inside jobs ("job − task"), not in the spin before parking. By the code that is mostly the spin on the task board, but the seat measured elapsed time, not CPU. A worker that joins a segment spins until the segment's last task is claimed. With 10 workers on a plan whose real segments are tens of µs, most of them wait most of the segment.
  - The 50 µs spin is the smaller term in elapsed time, about a fifth of it, except under `budget-spin`.
  - Wake latency costs no CPU in itself; it delays the join.
- **`--threads 1`.**
  - The work is 7.3 % of a core: render and unpaced play agree.
  - The 26 % is the same work run after a sleep, 375 times a second: how fast it runs then, not more work (assumption above).
  - On the busy host, `--buffer` 512 (a quarter of the sleeps) read 8–10 % in both runs.
- **The budget.** It removes the workers a plan's estimate cannot use:
  - for the kick, pulse and physical-av that is 8 or 9 of the 11, though those plans hardly go parallel at 128 frames anyway;
  - for 4 kick voices, 3 of 11, which stay parked; process CPU fell about 10 % on the idle host and about a third in auditor-2's run (above);
  - for the studio, 1 of 11, which the repeats could not tell from no change.
  - It keeps every worker for 80 voices.
  - **AC2 reading:** met for narrow plans that go parallel (fewer workers woken, surplus workers parked, less process CPU, the same output); not measurable for the wide studio, where only a looser rule (options below) would remove more workers.
- **The 2× target (AC7).** The studio at default settings is not near 2× its single-thread DSP on this host. The DSP is 7.3 %, so 2× is about 15 % of a core; the candidate read 62–90 % at the default on the busy host and 92–93 % on the idle one, against 64–93 % and 93–97 % for the base. **Target not met; the measured result is about 12× the single-thread DSP, not 2×.** The largest remaining term in the eligible workers' elapsed time is "job − task", by the code mostly the spin waiting inside a job; its CPU share is not measured here. Reducing it is a scheduling decision for the operator, not made here, because each option changes how the device's real-time workers behave. The options:
  - a larger `BUDGET_SLACK`, for example 10 %, which gives the studio 9 threads, or 25 %, which gives 7;
  - a per-task hand-off cost in the budget's simulation;
  - workers that park inside a job while no task is ready.
  The 50 µs spin and the wake policies are unchanged, as decided at the M2.6-5 and M2.7-1 checkpoints.

**Heavy plans keep their capacity (AC5)**, by construction and as measured.
- **By construction.** A plan whose estimated schedule needs every thread gets every worker, and then runs exactly the scheduling of before M2.10-5: the same workers are woken and join. The additions are one atomic store per job and per callback, and one atomic load per join. 80 kick voices get the budget of every thread on 2, 8, 12 and 18 threads (`crates/signal-cli/tests/blocks.rs`, `eighty_kick_voices_get_every_thread`; the rule's synthetic counterpart, with the reason checked, is `tasks.rs`, `a_heavy_plan_gets_every_thread`).
- **Measured.** `signal --json bench`, "Scaling" (kick voices within 25 % and 50 % of the budget at k = 128, default schedule, so with the budget in the candidate), base then candidate, 08:48–08:58 UTC, load average 3.3–6.5, `cores_source` `fallback` (18):

| Threads | Base, within 25 % / 50 % | Candidate, within 25 % / 50 % |
|---:|---:|---:|
| 1 | 25 / 50 | 25 / 51 |
| 2 | 33 / 65 | 32 / 65 |
| 4 | 61 / 119 | 60 / 121 |
| 8 | 83 / 171 | 95 / 193 |
| 18 | 130 / 265 | 161 / 336 |

The candidate is not below the base beyond one voice at any thread count. Its higher 8- and 18-thread figures are one run each, and are not attributed to the change.

**The capacity results of M2.6/M2.7 (AC6): what the seat could and could not measure.**
- **Not measurable here.** The 0 xruns of 80 kick voices and the V12 worst callback (< 0.25 of the period for the single-core kick) are device results: they need the CoreAudio driver, the audio workgroup and the device's clock, and the seat has none of them. They are left for the operator's soaks:
  - `scripts/soak.sh --silent --voices 80 30 128` at auto (12 threads, park50), expecting 0 xruns and a worst below 0.75 of the period (the capacity gate);
  - the V12 soak of `projects/kick`, worst below 0.25.
- **What the source shows.** Neither path changes for those loads:
  - 80 voices get every worker (above);
  - the kick, at 96 kHz and 128 frames, never runs a parallel segment, before or after: its estimate times 128 frames is below `parallel_min_ns`, and the trace of `signal play projects/kick --driver null --rt-trace` saw no join in 20 329 callbacks.
- **What the seat measured, not real time.** `signal bench --realtime --rt-voices 80 --rt-schedule base --rt-schedule default` (paced 128-frame blocks at 96 kHz; ordinary sandbox threads, time-constraint policy offline without a workgroup). In the candidate's own run, the pinned `base` schedule (every worker) and `default` (with the budget) agree: rt-all, 8 threads, median 648.2 against 647.5 µs, p99 2 087.5 against 2 086.5; 12 threads, 649.5 against 650.5, p99 2 159.3 against 2 157.4. Between separate sessions the same rows moved by 2× (the base build's 12-thread rt-all median read 317.1 µs), so only rows of one session compare. At 24 voices, rt-all, the p99 was 423–431 µs for both schedules at 8, 12 and 18 threads.

**Tests that cannot run here (AC9).** The seat refuses binding a TCP port (`Operation not permitted`, os error 1). So these fail here, and fail the same on the base a4deadc (117 failing tests on each, the same names):
- every `::tcp` variant of `crates/signal-app/tests/server.rs` and of the `crates/signal-ui` suites `app_mode`, `backpressure`, `http`, `panel_write`, `panels`, `patch`, `pattern`, `protocol`, `schematic`, `setup_protocol` and `unsaved`;
- `a_page_of_another_origin_cannot_open_the_websocket` (`http.rs`) and `a_project_without_a_main_panel_is_served_the_generated_one` (`panels.rs`), which serve over TCP only;
- `crates/signal-cli/tests/ui.rs` (`signal ui` never prints its URL).

Their `::in_process` twins pass. No test of `cargo test` ran headless Chrome, and the browser layout checks (`scripts/ui-strip.mjs`) were not run: the seat runs no npm or UI build. No test plays audio, and no real-time behaviour is claimed from the seat.

**For the operator at the checkpoint**, on the Mac outside the seat, release build:
1. The CPU cells above, with `ps %cpu` or `/usr/bin/time` as above: studio at 1 and 12 threads, base and candidate, several alternations; and the same with `signal ui` on CoreAudio (the owner's 120 %). If `powermetrics` is to hand, which cores the one-thread run lands on.
2. The capacity soak and the V12 soak above.
3. A decision on the remaining in-job wait: one of the three options under "What it means", or none.

### Idle cost, part 2 (M2.10-6)

What the engine's worker budget is (`docs/realtime-rules.md` "The worker budget": the cost-aware schedule rule, and in real time the share rule), the cost-table corrections it rests on, and what it does to process CPU.

**The target, restated by the operator:** at default threads, the studio's total CPU ≤ 2× the same host's `--threads 1` run (≈ 62 % on the quiet Mac, where `--threads 1` reads 31 %), while 80 kick voices still use every thread with no loss of capacity (late 0, hangs 0, worst callback not worse than base). The operator's starting figures, Mac Studio M5 Max, quiet host, release, null driver, (user + sys)/20 s of `signal play projects/studio --driver null --seconds 20`: `--threads 1` 31 %, `2` 50 %, `4` 73 %, default (12) 111–117 %. With the M2.10-5 candidate's cost-aware rule (studio 7 threads) the operator read 112.9 → 100.0 %.

**Every figure below from the seat is a measurement on one host, for the operator to confirm on the device; none is a threshold and none is a real-time claim.** The machine is the devpanel seat on the operator's Mac Studio (Mac17,14, Apple M5 Max, 18 cores, 64 GB, macOS 27.0.1; `sysctl` is refused in the seat). Builds: release, `cargo build --release --locked --offline -p signal-cli`, of the base f8f8633 (`git archive`) and of the candidate (f8f8633 plus M2.10-6, uncommitted). The host was shared with other sessions and not pinned; load averages are given per run.

**The simulated schedule** (deterministic; the engine's `makespan` at *k* = 128, read from a scratch build that printed it, never delivered). Segment time in µs on 1, 2, 3, 4, 6, 8, 12 and 18 threads:

| Plan | 1 | 2 | 3 | 4 | 6 | 8 | 12 | 18 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| studio (151 tasks) | 270.0 | 278.8 | 211.0 | 176.1 | 144.4 | 127.9 | 116.6 | 112.7 |
| physical-av (14 tasks) | 71.3 | 72.1 | 71.3 | 71.3 | 71.3 | 71.3 | 71.3 | 71.3 |
| kick (9 tasks) | 23.2 | 32.8 | 32.8 | 32.8 | 32.8 | 32.8 | 32.8 | 32.8 |
| 4 kick voices (25 tasks) | 75.2 | 63.4 | 50.9 | 42.1 | 42.1 | 42.1 | 42.1 | 42.1 |
| 80 kick voices (432 tasks) | 1 397.1 | 1 029.8 | 694.6 | 527.1 | 362.4 | 279.8 | 199.3 | 146.4 |

- On more than one thread every task costs its 1.5 µs hand-off, so the studio's 151 small tasks take longer on 2 threads than on 1 in the simulation.
- The studio's 4th thread saves 34.9 µs, while the 80 voices' 18th saves 6.9 µs. No gain per thread keeps the studio narrow and the 80 voices wide, which is why the real-time share rule exists. Their serial estimates are 10.1 % and 104.8 % of their periods (2 667 µs at 48 kHz, 1 333 µs at 96 kHz, 128-frame blocks).
- Budgets: in real time 1 for every plan above but the 80 voices, which get every thread; offline (renders) kick, pulse and physical-av 1, 4 voices 4, studio 8 of 12, 80 voices every thread (`docs/realtime-rules.md`, the table).

**The schedule rule's constants** (`crates/signal-engine/src/tasks.rs`). These are measurements recorded with the M2.10-5 candidate patch on the M5 Max, 2026-10-18, release build, and not re-measured in M2.10-6. The engine never times them at run time.
- **`HANDOFF_NS` = 1 500 ns per task per segment** (claiming, completing, releasing successors, inputs moving between cores). Method: kick-voice plans of 4, 16 and 80 voices at *k* = 128 on 1 to 4 threads with spinning workers, load average 2.2–2.8. It is (T · t_T − t_1) / tasks, with t_T the time per segment on T threads, which gave 1.5–2.0 µs per task. That is an upper bound, because it also counts any imbalance between the threads. The constant takes the low end.
- **`WAKE_HOP_NS` = 6 000 ns per hop of the wake tree.** Method: the median first worker's join (`first_join_median_us`) of `signal bench --realtime`, 24 and 80 kick voices, time-constraint workers. It read 4.6–7.2 µs.
- **`THREAD_GAIN_NS` = 6 000 ns is a choice, not a measurement**: one wake hop, the latency a worker costs before it helps. With it, the 80 voices' 18th thread (which saves 6.9 µs in the simulation) is still used, and the studio's offline budget is 8 of 12 (the table above).

**The cost table** (`COST_NS`, `crates/signal-std/src/lib.rs`). `signal --json bench` of the candidate before the change (`--seconds 1`, 2026-10-18 23:32–23:37 UTC, load average 7.4–7.9, loom running beside it). The run's per-primitive medians sat above the table of "Primitives" above by a host offset: the median difference over the 30 registered types was 12.2 ns. `signal-std`'s `every_cost_names_a_registered_type` checks that every name in the table is a registered type, so a misspelt entry cannot silently leave its type at the default. Every type without a registered cost whose median less that offset was at least 40 ns, twice the engine's default of 20, now has that figure, rounded up: 39 types. Among them `std.mesh2d_audio` 674, `std.modal` 113, `std.ladder` 91 (the M2.11-2 run read 99.0), `std.count` 71, `std.buffer*` 55–60, `std.step_seq8` 48, `std.mass_spring` 44, and the vector types from width 8 or 16 up (to `std.vadd1024` 5 344; the bench feeds them from one shared `std.pack<N>`, included in the figure). Left out on purpose:
- the visual-rate types `std.mesh2d` (497), `std.particles` (654) and `std.body2d` (111): the estimate does not divide by an instance's rate divisor, so a per-tick figure would overstate them by up to 800× at divisor 800;
- the registered entries, which stay as measured on the quieter host.

**The estimate against measurement.** One thread, `signal render <project> --threads 1 --block 128`, user + sys from `/usr/bin/time -p` at 25 s minus 5 s of audio, over 20 s, two repeats, candidate, 00:29–00:32 UTC, load average 7.8–11.6. CPU time per second of audio, times the period, is the measured time per 128-frame block:

| Plan | CPU per second of audio | Measured per block | Estimate (after the cost fix) | Estimate ÷ measured |
|---|---:|---:|---:|---:|
| studio (48 kHz) | 7.8, 7.6 % | 205 µs | 270 µs | 1.32 (too high) |
| physical-av (48 kHz) | 16.4, 16.3 % | 436 µs | 71.3 µs (19.1 before) | 0.16 (6.1× too low; 22.8× before) |
| kick (96 kHz) | 1.4, 1.4 % | 18.7 µs | 23.2 µs | 1.24 (too high) |
| 80 kick voices (96 kHz) | 81.8, 81.0 % | 1 085 µs | 1 397 µs | 1.29 (too high) |

- **Measurement.** The issue's "studio ~1.5× too high" reads 1.32× here; the error is not in the table (`std.const`, 19 instances in the studio, measures like `std.abs`). **Assumption, not measured:** it comes from the per-call term of cycle members (`PER_CALL_NS`) or from `BLOCK_FACTOR`, both measured on the kick; they are not changed here.
- **Measurement.** physical-av's 22× is 6.1× after the `std.mesh2d_audio` entry. **Assumption, not measured:** the rest is the membrane at the project's parameters against the bench's defaults, or the visual-rate types' ticks. Its budget is 1 under either rule, because its time is in one unit: its simulated schedule gains nothing from a second thread. On one thread it uses 16.4 % of the period.
- **The share rule's margin.** `SERIAL_SHARE` (0.15) sits under the workers' computation fraction (0.25) by 1.67×. The three over-estimated plans are on the safe side. A plan with physical-av's 6.1× error estimated just under 15 % would run on the audio thread at about 90 % of the period; none is known (`docs/realtime-rules.md`).

**Process CPU** (AC5, assumption on this host). `signal play <project> --driver null --seconds S [--threads 1]`, stdin held open, user + sys from `/usr/bin/time -p` at S = 25 minus S = 5, over 20 s, in % of one core per second of audio, so start-up is left out (the M2.10-5 method). Base and candidate alternate cell by cell, 00:33–00:48 UTC; the 1-minute load average at each cell is in brackets. These are process CPU; nothing here is elapsed time.

| Studio | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| `--threads 1`, base / candidate | 23.5 / 24.6 (3.0) | 23.5 / 23.3 (3.4) | 23.6 / 23.7 (2.6) |
| default (12), base / candidate | 94.8 / 23.4 (2.7) | 104.2 / 24.5 (2.6) | 97.6 / 23.6 (2.5) |

| 4 kick voices, default | Run 1 | Run 2 |
|---|---|---|
| base / candidate | 66.7 / 14.4 | 71.8 / 15.0 |

- **Measurement (one host).** At default threads the candidate's studio uses 23.4–24.5 % of a core, the same as its own `--threads 1` (23.3–24.6 %): 1.0×, against the target of ≤ 2×. The base uses 94.8–104.2 %, about 4×.
- **Measurement (one host).** 4 kick voices fall from 67–72 % to 14–15 %.
- **Why.** Both plans' serial estimates fit in 15 % of the period, so in real time their budget is 1, no worker is woken, and the 11 workers stay parked. This is the mechanism; the size of the gain on the device is the operator's measurement.
- The seat's `--threads 1` reads 23–25 % here against 7.6–7.8 % for the unpaced render: the paced-run effect of M2.10-5 (after each `thread::sleep` the work runs slower; an assumption about core placement, not measured).

**80 kick voices** (AC6, sandbox only). `signal --json bench --realtime --rt-voices 80 --seconds 2` (paced 128-frame blocks at 96 kHz, 1 501 blocks per row; ordinary sandbox threads; the time-constraint policy offline without a device workgroup; park50). Candidate with `--rt-schedule base --rt-schedule default`, then the base build's `default`, 00:44–00:48 UTC, load average about 4–7. The bench's real-time mode now sets `EngineConfig::realtime`, so its `default` runs the share rule as `signal play` does; for 80 voices that changes nothing (104.8 % of the period). `rt-all` rows (the calling thread also in the time-constraint policy), median / p99 / blocks over the period:

| Threads | Base build, default | Candidate, `base` (every worker) | Candidate, `default` (budget) | Budget |
|---:|---|---|---|---:|
| 1 | 1 092.6 / 1 215.9 µs / 0 | 1 089.3 / 1 230.6 / 0 | 1 120.9 / 1 245.8 / 1 | 1 |
| 8 | 506.6 / 1 786.3 / 143 | 580.4 / 2 090.7 / 139 | 640.9 / 2 090.6 / 155 | 8 |
| 12 | 627.6 / 2 156.5 / 277 | 529.5 / 2 159.2 / 217 | 468.5 / 2 152.0 / 163 | 12 |
| 18 | 425.3 / 3 544.2 / 114 | 490.8 / 3 498.3 / 196 | 482.6 / 3 470.2 / 169 | 18 |

- **Measurement.** The budget is every thread on 8, 12 and 18, as the tests assert (`crates/signal-cli/tests/blocks.rs`). The candidate's `base` and `default` schedules run the same workers.
- Their rows differ within this session's spread: at 12 threads `default` reads lower than `base`, at 8 higher.
- The base build's rows differ from both by as much again, so only rows of one session compare (as in M2.10-5).
- The blocks over the period (hundreds of 1 501) are the sandbox's: no workgroup and a shared host. They are not the device's late count. The 0 xruns, 0 hangs and the worst callback are the operator's soak (`scripts/soak.sh --silent --voices 80 30 128`).

**Per-worker skew and task-body inflation** (elapsed time, from the same rows' trace; `wake.job_ms`, `worker_task_ms`, `slowest_task_*`). These are `Instant` durations, not CPU: a descheduled interval costs no CPU.
- **Skew**, the longest worker's job time ÷ the shortest's, over the joined workers. `rt-all`: 1.58–1.74 at 8 and 12 threads, 3.66–3.84 at 18. `ordinary`: 2.9–6.3. `rt-workers`: 1.1–1.5. With ordinary threads the work goes to the workers that wake first, and the late ones join little.
- **Inflation of a task body.** The median of each block's longest task body is 10.2 µs on one thread (`rt-all`) and 28.7, 30.7–41.0 and 41.0 µs on 8, 12 and 18 threads. So the same longest task takes about 3–4× longer when the workers run beside it. The workers' task bodies alone add up to 2.9–3.8 ms per block at 8–18 threads (`worker_task_ms` ÷ 1 509 callbacks), against 1.09 ms for the whole block on one thread: at least 2.7–3.5×, a lower bound because the calling thread's tasks are not counted.
- **Not measured:** how much of this is cache and memory contention, core class (the 6 performance-level-0 cores of 18) or descheduling on the shared host. It is why the simulation's hand-off and wake constants underpredict real parallel time: the 80 voices' simulated 199 µs at 12 threads against a measured median of 468–530 µs.

**Tests that cannot run here** (AC12). The seat refuses binding a TCP port (`Operation not permitted`), and `signal ui` never prints its URL there. `cargo test --workspace --offline` on the candidate: 2 154 passed, 110 failed, 1 ignored. The same crates (`signal-app`, `signal-ui`, `signal-cli`) on the base f8f8633 (`git archive`) fail the same 110 tests, by name: 106 `::tcp` variants (their `::in_process` twins pass), and `a_page_of_another_origin_cannot_open_the_websocket`, `a_project_without_a_main_panel_is_served_the_generated_one`, `attach_to_a_running_signal_ui`, `signal_ui_serves_the_protocol_while_it_plays`. The `::tcp` variants are of these tests: `a_bad_token_a_missing_hello_and_another_version_are_refused`, `a_canvas_binds_vector_outputs`, `a_changed_file_is_a_conflict`, `a_chosen_device_is_found_again_by_identity`, `a_client_connecting_while_off_is_told_and_gets_no_frames`, `a_client_needs_the_token`, `a_client_that_stops_reading_is_dropped_after_the_write_deadline`, `a_commit_while_stopped_writes_once`, `a_failed_commit_stays_unsaved_until_retried`, `a_failed_stopped_commit_is_still_unsaved_when_playing`, `a_file_without_one_panel_is_not_editable`, `a_folder_named_like_a_signal_file_opens_as_a_folder`, `a_knob_of_the_kick_panel_changes_one_value_of_the_toml`, `a_large_asset_arrives_whole`, `a_later_client_describes_every_parameter_while_playing`, `a_library_composite_is_added_by_a_patch`, `a_link_at_the_temporary_path_is_not_followed`, `a_newer_live_edit_is_what_retry_writes`, `a_panel_reached_through_a_link_is_not_written`, `a_panel_that_binds_what_the_plan_lacks_is_told_where`, `a_panel_with_pattern_controls`, `a_panel_write_marks_a_signal_project_edited`, `a_patch_is_heard_after_the_reload`, `a_patch_marks_a_signal_project_edited`, `a_pattern_that_does_not_load_is_not_written`, `a_pattern_write_marks_a_signal_project_edited`, `a_plan_swap_updates_describe`, `a_playing_graph_is_built_once_per_plan_version`, `a_project_that_does_not_load_shows_its_diagnostics`, `a_project_that_stops_loading_pushes_graph_error`, `a_recording_run_saves_the_setup_but_is_not_restarted`, `a_refused_archive_opens_nothing`, `a_refused_binding_writes_nothing`, `a_refused_edit_back_to_the_file_value_stays_unsaved`, `a_refused_element_or_attribute_writes_nothing`, `a_release_masked_by_a_claim_still_restarts_the_release_age`, `a_reload_that_writes_the_parameter_clears_it`, `a_scope_on_the_sinks_input_shows_the_sum_of_its_sources`, `a_signal_file_opens_in_a_working_folder`, `a_stalled_client_blocks_neither_another_request_nor_shutdown`, `a_stalled_client_is_dropped_from_and_evicted_without_blocking_the_others`, `a_stopped_commit_updates_the_inner_bases`, `a_stopped_move_on_4096_voices_writes_once_and_loads_nothing`, `a_stopped_patch_writes_and_pushes_graph`, `a_structural_reload_pushes_graph`, `a_vector_stream_follows_a_swap_that_changes_its_width`, `a_write_is_applied_live`, `a_write_is_pushed_and_applied_live`, `a_write_over_the_message_limit_closes_the_connection`, `a_write_replaces_the_panel_and_is_pushed`, `an_edit_of_the_positions_file_pushes_graph`, `an_edit_the_watch_cannot_see_is_applied_by_the_next_commit`, `an_undo_too_large_is_refused_and_a_commit_pushes_files`, `apply_takes_the_wake_policy_and_plays_again`, `closing_keeps_an_edited_working_folder`, `commit_writes_the_file_once_and_the_watch_does_not_reload_it`, `connections_before_hello_end_at_shutdown`, `connections_past_the_limit_are_refused_with_a_reason`, `connections_that_do_not_say_hello_in_time_give_their_place_back`, `describe_gives_the_plan_its_controls_sinks_and_meters`, `dropping_it_saves_the_setup`, `every_op_round_trips_on_kick_pulse_physical_av_and_a_composite`, `exit_stops_saves_the_window_and_closes_clients`, `graph_is_pushed_when_switched_on_again`, `graph_while_stopped`, `in_use_holds_when_the_project_does_not_load`, `in_use_names_exports_and_panel_bindings`, `it_starts_stopped_on_the_last_project`, `it_starts_stopped_plays_and_stops`, `kick_pulse_and_physical_av_are_described_as_written`, `meters_and_a_scope_stream_arrive`, `modules_is_deterministic`, `names_links_and_a_first_write`, `only_the_served_project_s_panels_are_written`, `opening_a_project_stops_records_it_and_serves_its_panels`, `opening_another_project_can_save_first`, `positions_are_never_written_through_a_link`, `power_off_stops_and_on_restores`, `refusals_write_nothing`, `reopening_the_same_file_with_save_keeps_the_edit`, `revert_restores_the_file_value_live`, `same_named_copies_have_different_project_ids`, `save_as_writes_the_file_and_leaves_the_folder`, `schematic_move_writes_a_group_at_once`, `schematic_move_writes_positions_and_pushes_graph`, `set_ramp_and_trigger_are_acknowledged_with_their_frame`, `shutdown_with_stalled_clients_at_the_limit_is_prompt`, `signal_ui_refuses_the_app_s_messages`, `signal_ui_reopens_only_the_project_it_serves`, `signal_ui_stops_and_plays`, `static_files_have_their_types_and_stay_in_their_folders`, `steps_never_wait_for_a_document_comparison`, `studio_is_described_and_laid_out`, `the_4096_voice_graph_reaches_the_client_and_it_stays_open`, `the_dialog_functions_and_apply_play_again_with_the_setup`, `the_errors_write_nothing`, `the_free_scope_count_is_described_and_pushed`, `the_generated_panel_s_first_write_creates_main_html`, `the_stopped_graph_is_cached_until_the_folder_changes`, `the_text_outside_the_panel_is_kept_byte_for_byte`, `the_url_carries_a_new_token_in_its_fragment`, `the_window_s_requests_reach_the_project`, `unknown_messages_are_answered_with_an_error`, `vector_streams_arrive`, `vendor_copies_a_library_composite`. No test plays audio; the browser layout checks (`scripts/ui-strip.mjs`) were not run (no npm or UI build in the seat).

**Render hashes** (AC9). `signal --json render projects/<p> --seconds 120 --threads T`, base and candidate builds, T = 1, 12 and 18. `audio_fnv1a64` / `log_fnv1a64` equal for every pair: kick 9eec9e756e2d891d / 134f1d3095c5bdd2, pulse 0e2925144758dd17 / d5a4b62fc761cd25, physical a3381da11f117c70 / 877adbebc03a52d9, physical-av 6e949ac556b7e1b5 / 870ae406ab039878, studio ae83d794a40b6941 / c4920d6570c2a66b, library-demo c625006298c22e3b / c690e983602216d4.

**Not re-measured, reused from M2.10-5:** the wake-policy and buffer-size matrix and the idle host's one-thread reading, because nothing in M2.10-6 changes the wake policies, the spin or the paced loop. Also not re-measured: the bench's "Scaling" capacity rows (offline, so the share rule does not apply to them, and the schedule rule is M2.10-5's). **Cut for time:** an idle-host repeat of the CPU table, and the studio and 4 voices with `signal ui` on CoreAudio.

**For the operator at the checkpoint**, on the Mac outside the seat, release build:
1. The studio CPU cells above (`--threads 1` and default, base and candidate) with `ps %cpu` or `/usr/bin/time`, and the same under `signal ui` on CoreAudio.
2. The capacity soak `scripts/soak.sh --silent --voices 80 30 128` at auto (12 threads, park50), expecting 0 xruns and the worst callback not worse than base, and the V12 soak of `projects/kick`.
3. A judgement on `SERIAL_SHARE` = 0.15. It is the one constant behind the real-time result: the studio plays on one thread, at about 6–8 % of its period on this host.

## What this means for D-EXEC-3

These are interpretations of a single seat measurement. They are not a decision; D-EXEC-3 stays deferred (roadmap M4).

- **Fact (measured):** the per-frame engine runs the kick project in 2.5 % of the 96 kHz budget. Fourteen kick voices (about 100 primitive instances) fit within 25 %, and 28 voices within 50 %. Today's test projects do not need block processing to run in real time.
- **Measurement:** the fixed per-frame cost of the engine is negligible (about 2 ns). The cost is per instance. The cheapest primitives (`std.abs`, `std.add`, `std.mul`, `std.noise`) cost 9–12 ns, so roughly 10 ns per instance per frame goes to dispatch, input gathering and parameter handling rather than to the DSP. Heavier primitives cost 20–57 ns.
- **Assumption:** block execution (*k* > 1) would mostly reduce that per-instance floor. It would matter for graphs of several hundred small instances (at about 10 ns each, about 260 trivial instances use 25 % of the budget), not for the kick. Whether such graphs are expected is a roadmap question for M4.
- **Caveat:** the p99 columns and the voice counts depend on the machine's load. Re-run on an idle machine before the decision, and record each run as its own column.

## The soak (V12)

`scripts/soak.sh [--driver coreaudio|null] [--signal PATH] [--target-dir DIR] [--voices N] [--threads n] [--silent] [minutes] [buffer]` copies `projects/kick` into a new temporary folder (`$TMPDIR/signal-soak.*`); the committed project is never edited. With `--voices N` (M2.6-3) the folder holds instead the kick with its `drums` composite duplicated *N* times into a tree of 4-input `std.mix`, written by `signal bench --write-voices N <folder>` (the "Kick voices" graph, its `out` gain divided by *N* so that it does not clip; the `accent` line is the same). With `--silent` (M2.6-5) the copy's `out` gain becomes 0.001 (−60 dB, `sed` and `mv` as for the edits); every module still runs, so the timing is the same. It runs `signal --json play <copy> --watch --driver <driver> --buffer <buffer> --seconds <minutes × 60>` at the project's 96 kHz, with `--threads n` when given (otherwise `signal play`'s auto, the performance cores minus one). Every 10 s it rewrites `accent` `decay_ms` in the copy, alternating 300 and 200, with `sed` into a hidden file and `mv` (no Python). At the end it prints the frames, the voices, the threads and workers with their scheduling path (`workgroup`, `time-constraint`, `none` or `mixed`, from the play JSON), the edits written and applied, the xruns, the worst callback as a fraction of the period, and the verdict:

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

### The native DSP primitives (M2.10, the owner's list)

**Measurement on one host, not a threshold.** `signal --json bench --seconds 0.5`, release build of 6d42e6c plus the uncommitted M2.10 DSP primitives after the review fixes (bounded comb and all-pass reads, the comb's convex damping one-pole, the wavetable's 4096-sample Hermite tables), 2026-10-19 22:16–22:19 UTC, devpanel-shared Mac (load average 9–18). Per-instance medians (16 instances fed by the bench's saw), less the run's host offset of 13.1 ns (the median difference to the first 30 `COST_NS` entries; `std.svf` read 22.3 against its 22, `std.ladder` 94.7 against 91):

| Type | Median | Less offset | `COST_NS` |
|---|---:|---:|---:|
| `std.biquad` | 42.1 | 29.0 | 29 |
| `std.comb` | 45.1 | 32.0 | 32 |
| `std.allpass_delay` | 44.0 | 30.9 | 31 |
| `std.fdn_reverb` | 84.8 | 71.7 | 72 |
| `std.fm_op` | 55.1 | 42.0 | 42 |
| `std.wavetable` | 44.8 | 31.7 | 32 |

The first run (21:02 UTC, before the fixes, load 5–27) read 27.6, 39.4, 37.9, 74.2, 42.5 and 26.1: the wavetable's 16 Hermite reads cost about 6 ns more than 8 linear ones; the comb and all-pass did not get dearer (the differences are within this host's run-to-run spread). The bench keeps every parameter at its default, so these are the costs with cached coefficients; a parameter modulated at audio rate adds its transcendental functions per frame (`std.biquad` a `tan`, a `pow` and a square root; `std.comb` an `exp` when `damping_hz` moves; `std.fdn_reverb` eight `pow` when `size` or `decay_s` moves).
