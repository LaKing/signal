# Benchmarks (V13) and the soak (V12)

What the per-frame engine (*k* = 1, ADR 0016) costs at 96 kHz, as input to D-EXEC-3 (block processing, deferred to benchmarks; `docs/00-vision.md`). The numbers are **measurements**, never thresholds: no test compares a timing to a limit, and the decision is the owner's (AGENTS.md rule 6).

## Method

`signal bench [--json] [--seconds <s>]` (`crates/signal-cli/src/bench.rs`) builds each case as an execution plan and runs it in the offline `Engine` one frame at a time, as `signal render` does. Every case runs at 96 kHz with seed 42. After 1024 untimed frames, it times repeats of 1024 frames for `--seconds` (default 1 s, at least 5 repeats) and reports nanoseconds per frame: the median and the 99th percentile (nearest rank) over the repeats. The real-time budget is 10⁹ / 96 000 = **10 417 ns per frame**. The only clock reads are around each repeat, in the bench command. The engine is unchanged.

| Case | Graph | Reported |
|---|---|---|
| Engine, empty plan | No instances | The fixed per-frame overhead |
| Stimulus | A 440 Hz band-limited saw (`std.osc`), a 2 Hz `std.lfo` used as a gate, and a 132 bpm `std.transport` with ghost notes (probability 0.15) | Subtracted from each primitive's graph |
| Each registered `std.*` primitive | The stimulus and 16 instances at default parameters. Signal inputs come from the saw (phase inputs scaled to 0.05 cycles), gate inputs from the LFO, the first event input from the beats and ghost notes, and any other event input from the accents. Outputs are left open; every instance still runs. | (graph − stimulus median) / 16, per instance; the p99 column uses the graph's p99 |
| Kick project | `projects/kick` as committed (compiled into the binary) | Per frame, and as a fraction of the budget |
| Kick voices | The kick project with its `drums` instance duplicated *N* times (every connection to and from it repeated) and the copies summed by a tree of 4-input `std.mix` into `out` | The largest *N* whose **median** stays within 25 % and within 50 % of the budget (doubling, then bisection; every *N* measured is listed in `kick_voices.measured`) |

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

## What this means for D-EXEC-3

These are interpretations of a single seat measurement. They are not a decision; D-EXEC-3 stays deferred (roadmap M4).

- **Fact (measured):** the per-frame engine runs the kick project in 2.5 % of the 96 kHz budget. Fourteen kick voices (about 100 primitive instances) fit within 25 %, and 28 voices within 50 %. Today's test projects do not need block processing to run in real time.
- **Measurement:** the fixed per-frame cost of the engine is negligible (about 2 ns). The cost is per instance. The cheapest primitives (`std.abs`, `std.add`, `std.mul`, `std.noise`) cost 9–12 ns, so roughly 10 ns per instance per frame goes to dispatch, input gathering and parameter handling rather than to the DSP. Heavier primitives cost 20–57 ns.
- **Assumption:** block execution (*k* > 1) would mostly reduce that per-instance floor. It would matter for graphs of several hundred small instances (at about 10 ns each, about 260 trivial instances use 25 % of the budget), not for the kick. Whether such graphs are expected is a roadmap question for M4.
- **Caveat:** the p99 columns and the voice counts depend on the machine's load. Re-run on an idle machine before the decision, and record each run as its own column.

## The soak (V12)

`scripts/soak.sh [--driver coreaudio|null] [--signal PATH] [--target-dir DIR] [minutes] [buffer]` copies `projects/kick` into a new temporary folder (`$TMPDIR/signal-soak.*`); the committed project is never edited. It runs `signal --json play <copy> --watch --driver <driver> --buffer <buffer> --seconds <minutes × 60>` at the project's 96 kHz. Every 10 s it rewrites `accent` `decay_ms` in the copy, alternating 300 and 200, with `sed` into a hidden file and `mv` (no Python). At the end it prints the frames, the edits written and applied, the xruns, the worst callback as a fraction of the period, and the verdict:

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
