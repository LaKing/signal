# M3 experiment: `std.tanh_shaper` aliasing, ADAA versus oversampling

The first M3 lab experiment (`docs/roadmap.md`, M3 Evaluation loop), run by an agent for follow-up issue `2026-10-11-follow-up-bring-std-tanh-shaper-aliasing-to-60-db-on-off-div`. V6 asks that `std.tanh_shaper` at drive 20 and 96 kHz keep its inharmonic (aliased) energy at least 60 dB below the fundamental at 1009, 2003, 4001 and 8009 Hz. The current shaper (first-order ADAA) measures 49.0 / 33.5 / 26.1 / 22.0 dB. The threshold stays where it is.

**Status: awaiting the owner's verdict** (`verdict.md`, empty). Nothing in `std` has changed: `std.tanh_shaper`, V6's `#[ignore]` and every golden are as they were. ADR 0015 lets a candidate be promoted only after the contract suite, benchmarks and a recorded human verdict, and promotion changes renders (see "What promotion would change").

## The candidates

All five are `lab.*` primitives in `lab/src/tanh_shapers.rs`. They compile only with the `lab` feature. Each has `std.tanh_shaper`'s ports and parameter (`in`, `out`, `drive` 0.1–20, default 1, combine `add`), so a project switches to one by changing the type name and nothing else.

| Type | Method | Delay (frames) | CPU, ns per frame (median of 3 runs) |
|---|---|---:|---:|
| `std.tanh_shaper` (current) | first-order ADAA at the frame rate | 0.5 | 55 |
| `lab.tanh_adaa2` | second-order ADAA at the frame rate (Bilbao et al. 2017), F₂ through Li₂; its inner mean integrated by 3-point Gauss–Legendre for steps below 0.01, output clamped to [−1, 1] | 1 | 97 |
| `lab.tanh_os2_adaa1` | ×2 oversampling, first-order ADAA at 2·sr | 65.25 | 139 |
| `lab.tanh_os4_adaa1` | ×4 oversampling, first-order ADAA at 4·sr | 71.625 | 180 |
| `lab.tanh_os8_adaa1` | ×8 oversampling, first-order ADAA at 8·sr | 73.8125 | 249 |
| `lab.tanh_os8` | ×8 oversampling, plain `tanh` at 8·sr | 73.75 | 184 |

**Oversampling.** `u = drive·in` is raised by cascaded ×2 stages, shaped at the high rate, and brought back through the same stages. Every stage is a linear-phase FIR (Kaiser-windowed sinc, β = 9, about 90 dB stopband), used polyphase both ways:
- Stage 1 (sr ↔ 2·sr), 131 taps: passband to 0.40·sr, stopband from 0.50·sr. Nothing above the frame rate's Nyquist frequency folds back below it, which V6 requires because it counts aliases all the way up to Nyquist. This stage sets the passband edge (19.2 kHz at 48 kHz, 38.4 kHz at 96 kHz) and accounts for 65 of the ≈ 74 frames of delay.
- Stages 2 and 3 (to 4·sr and 8·sr): half-band filters of 27 and 19 taps.

The coefficients are computed in `prepare` in normalized frequency, so they are identical at every rate. The work per frame is fixed.

**Delay and the contract.** `docs/module-contract.md` has no way for a direct type to declare a latency. `BlockLatency` belongs to latent state-output types, whose results arrive asynchronously, and the engine compensates no path. So each candidate documents its delay in its rustdoc and its `doc` string, as `std.tanh_shaper` documents its half frame. A parallel dry path that must stay aligned needs a `std.delay` of the same length. Because the FIRs are linear phase, the delay is the same at every passband frequency (measured below).

**ADAA2's numerics.** The first version's inner quotient was a difference of F₂ values, which left a cancellation floor of about 3·10⁻⁷ absolute near u = 0 and for steps just above its 10⁻⁵ switch. The quotient is now integrated directly by three-point Gauss–Legendre for steps below 0.01. On slow sines with steps from 2·10⁻⁶ to 10⁻³ the output stays within its smoothing term of `tanh` one frame late, and the error falls as Δu²: 6.9·10⁻⁸ at Δu ≈ 9·10⁻⁴, 4.4·10⁻¹⁴ at 2·10⁻⁶ (`adaa2_has_no_cancellation_floor_on_slow_inputs`). Its output is clamped to [−1, 1], because rounding could exceed the declared range by about 10⁻¹⁰.

**Contract and real-time rules.** All five pass the contract suite: partition invariance, reset determinism, finite output, no allocation, state-output honesty, rate agnosticism and no subnormals, at 44.1, 48, 96 and 192 kHz (`lab/tests/contract.rs`). A render with every candidate in one project gives the same audio hash on 1 thread and on 4 workers (`the_candidates_render_the_same_on_any_thread_count`). They use no `unsafe` and no new crate: `libm` only, already a dependency of `lab`. Each frame does fixed work: no data-dependent loops, and no allocation outside `prepare`. They are bounded synchronous calls, like `std.tanh_shaper`, so they need no latent boundary.

## Measured results

Everything below is a measurement (rule 6: metrics inform, the owner decides). V6's metric is `signal_eval::aliasing`: a 1 s Hann-windowed FFT after 1000 frames of start-up, giving the inharmonic energy in dB below the fundamental. It runs on V6's own project with only the type and drive changed. The test checks that it reproduces V6's published values for `std.tanh_shaper` (49.0 and 26.1 dB) and for naive `tanh` (43.8 and 18.5 dB). `naive` is `tanh(drive·x)` with no antialiasing. Each cell in the aliasing tables is the inharmonic ratio in dB; **bold** means at or above 60.

### V6 conditions: 96 kHz, drive 20

| Shaper | 1009 Hz | 2003 Hz | 4001 Hz | 8009 Hz |
|---|---:|---:|---:|---:|
| naive `tanh` | 43.8 | 27.3 | 18.5 | 13.1 |
| `std.tanh_shaper` | 49.0 | 33.5 | 26.1 | 22.0 |
| `lab.tanh_adaa2` | 53.9 | 38.9 | 32.0 | 29.1 |
| `lab.tanh_os2_adaa1` | **125.0** | **76.0** | 53.3 | 39.6 |
| `lab.tanh_os4_adaa1` | **141.4** | **131.7** | **92.4** | **62.7** |
| `lab.tanh_os8_adaa1` | **141.2** | **130.9** | **119.0** | **101.6** |
| `lab.tanh_os8` | **141.1** | **130.9** | **119.0** | **73.2** |

### 48 kHz, drive 20 (the studio projects' rate)

| Shaper | 1009 Hz | 2003 Hz | 4001 Hz | 8009 Hz |
|---|---:|---:|---:|---:|
| naive `tanh` | 27.3 | 18.5 | 13.1 | 7.1 |
| `std.tanh_shaper` | 33.6 | 26.1 | 22.0 | 13.3 |
| `lab.tanh_adaa2` | 39.1 | 32.0 | 29.1 | 19.6 |
| `lab.tanh_os2_adaa1` | **75.8** | 53.2 | 39.5 | 41.2 |
| `lab.tanh_os4_adaa1` | **127.0** | **92.2** | **62.7** | 56.9 |
| `lab.tanh_os8_adaa1` | **126.6** | **119.4** | **101.5** | **79.5** |
| `lab.tanh_os8` | **126.6** | **119.5** | **73.1** | 45.7 |

### Lower drives

| Shaper | 96 kHz, drive 5 | 96 kHz, drive 2 | 48 kHz, drive 5 | 48 kHz, drive 2 |
|---|---|---|---|---|
| naive `tanh` | 134.4 / 69.9 / 37.7 / 21.5 | 263.5 / 153.4 / 78.2 / 40.6 | 69.9 / 37.7 / 21.5 / 9.5 | 153.4 / 78.2 / 40.6 / 15.2 |
| `std.tanh_shaper` | 138.8 / 74.7 / 43.2 / 28.5 | 263.5 / 158.0 / 83.5 / 47.4 | 74.8 / 43.2 / 28.5 / 14.8 | 158.1 / 83.5 / 47.4 / 20.5 |
| `lab.tanh_adaa2` | 143.2 / 79.5 / 48.8 / 35.8 | 251.9 / 162.8 / 89.3 / 55.7 | 79.6 / 48.8 / 35.8 / 22.1 | 163.0 / 89.3 / 55.7 / 29.6 |
| `lab.tanh_os2_adaa1` | 229.9 / 173.8 / 120.3 / 67.8 | 263.5 / 256.3 / 178.8 / 129.5 | 167.4 / 120.0 / 67.8 / 55.7 | 250.0 / 180.0 / 129.4 / 93.7 |
| `lab.tanh_os4_adaa1` | 229.1 / 173.3 / 139.4 / 127.7 | 263.5 / 255.8 / 180.0 / 146.1 | 166.7 / 140.7 / 126.6 / 93.9 | 249.1 / 181.5 / 144.9 / 116.5 |
| `lab.tanh_os8_adaa1` | 228.8 / 171.8 / 136.3 / 127.6 | 263.5 / 255.1 / 176.1 / 145.6 | 166.2 / 137.1 / 126.7 / 110.0 | 249.0 / 176.9 / 144.7 / 116.0 |
| `lab.tanh_os8` | 228.7 / 172.0 / 137.0 / 127.5 | 263.5 / 255.2 / 176.9 / 145.6 | 166.2 / 137.8 / 126.6 / 110.0 | 248.8 / 177.8 / 144.6 / 116.0 |

Cells are dB at 1009 / 2003 / 4001 / 8009 Hz. The metric's own floor in f64 is about 260 dB: naive `tanh` reads 263.5 dB at drive 2. The shared ceiling of the ×4 and ×8 candidates at 1009 Hz, about 141 dB at 96 kHz and 126.6 dB at 48 kHz (plain `tanh` included), comes from **stage 1's stopband**. This is measured: a throwaway build with stage 1 at β = 12 and 155 taps (about 117 dB of stopband, not kept) raised it to 169.6–170.0 dB at 96 kHz and 145.3–147.5 dB at 48 kHz. The same build left every candidate's worst tone unchanged (62.7 / 101.7 / 73.2 dB at 8009 Hz and 96 kHz). So stage 1 sets a ceiling on the oversampled candidates, about 126–141 dB at drive 20, far above V6's 60 dB. It is not what limits them at the high tones.

### Passband: gain (dB) and delay (frames)

Drive 0.1 on a sine of amplitude 0.1, which keeps `tanh` linear to within 3·10⁻⁵, compared with `tanh(drive·x)` of the same sine. The delay is read from the phase.

| Shaper | 96 kHz: 1 kHz | 10 kHz | 20 kHz | 48 kHz: 1 kHz | 10 kHz | 18 kHz | 20 kHz |
|---|---|---|---|---|---|---|---|
| `std.tanh_shaper` | −0.005 / 0.5 | −0.47 / 0.5 | −2.01 / 0.5 | −0.019 / 0.5 | −2.01 / 0.5 | −8.34 / 0.5 | −11.74 / 0.5 |
| `lab.tanh_adaa2` | −0.012 / 1 | −1.29 / 1 | −5.92 / 1 | −0.049 / 1 | −5.92 / 1 | −17.2 (sign flipped) | −12.3 (sign flipped) |
| `lab.tanh_os2_adaa1` | −0.001 / 65.25 | −0.12 / 65.25 | −0.47 / 65.25 | −0.005 / 65.25 | −0.47 / 65.25 | −1.60 / 65.25 | −2.13 / 65.25 |
| `lab.tanh_os4_adaa1` | −0.000 / 71.625 | −0.03 / 71.625 | −0.12 / 71.625 | −0.001 / 71.625 | −0.12 / 71.625 | −0.38 / 71.625 | −0.60 / 71.625 |
| `lab.tanh_os8_adaa1` | −0.000 / 73.81 | −0.007 / 73.81 | −0.03 / 73.81 | −0.000 / 73.81 | −0.03 / 73.81 | −0.10 / 73.81 | −0.24 / 73.81 |
| `lab.tanh_os8` | −0.000 / 73.75 | −0.000 / 73.75 | −0.000 / 73.75 | −0.000 / 73.75 | −0.000 / 73.75 | −0.000 / 73.75 | −0.12 / 73.75 |

Every delay is constant across the band: there is no phase distortion beyond the pure delay. ADAA's averaging is also a low-pass in the linear region. First order is a two-tap mean (cos(πf/sr)), and second order is (x[n] + x[n−1] + x[n−2])/3, which crosses zero at sr/3, so its phase reading flips at 18 and 20 kHz at 48 kHz. **The current shaper is already 2 dB down at 10 kHz and 8.3 dB down at 18 kHz at 48 kHz, even at low drive.** In the oversampled candidates ADAA runs at the high rate, where this droop is small. At 20 kHz and 48 kHz, stage 1's transition band starts (19.2 kHz).

### Peak of a clipped sine (drive 20, 48 kHz)

| Shaper | 100 Hz | 1009 Hz | 4001 Hz | 8009 Hz |
|---|---:|---:|---:|---:|
| `std.tanh_shaper`, `lab.tanh_adaa2` | 1.0000 | 1.0000 | 1.0000 | 1.0000 |
| `lab.tanh_os2_adaa1` | 1.0000 | 1.0892 | 1.1429 | 1.2440 |
| `lab.tanh_os4_adaa1` | 1.0000 | 1.0974 | 1.1733 | 1.2659 |
| `lab.tanh_os8_adaa1` | 1.0000 | 1.0996 | 1.1784 | 1.2708 |
| `lab.tanh_os8` | 1.0000 | 1.1003 | 1.1796 | 1.2712 |

A band-limited square overshoots. At 8 kHz and 48 kHz only its fundamental lies below the passband edge, and a lone fundamental peaks at 4/π ≈ 1.27. So the oversampled candidates' output is not bounded to [−1, 1], and their `out` port declares no range. `std.tanh_shaper` declares [−1, 1].

### CPU

From `signal bench --only <prefix> --param drive=20` (release build, 96 kHz). The figure is per instance: 16 instances fed by the bench's 440 Hz saw, minus the stimulus. Three runs of 2 s each, 2026-10-23, on the operator's Mac Studio (M5 Max). **The Mac was not idle (load average 8.5–8.8)**, so treat these as upper estimates.

| Shaper | Runs (median ns) | p99 (worst run) | Share of the 96 kHz budget (10 417 ns) |
|---|---|---:|---:|
| `std.tanh_shaper` | 55.4, 56.5, 53.3 | 80.4 | 0.5 % |
| `lab.tanh_adaa2` | 97.0, 96.8, 92.4 | 117.4 | 0.9 % |
| `lab.tanh_os2_adaa1` | 147.7, 139.0, 139.2 | 219.7 | 1.3 % |
| `lab.tanh_os4_adaa1` | 191.4, 179.0, 179.6 | 270.6 | 1.7 % |
| `lab.tanh_os8_adaa1` | 270.8, 247.0, 248.8 | 379.9 | 2.4 % |
| `lab.tanh_os8` | 197.2, 183.5, 183.7 | 306.2 | 1.8 % |

## The listening package

Renders of the same input through the current shaper and every candidate. Only the shaper's type name differs, plus the output gain, which is the same for every shaper of an input and keeps the peak at or below 0.25. The audio is not committed (`*.wav` is ignored). Make it with:

```sh
cargo build --release --locked --offline -p signal-cli --features lab
experiments/m3-tanh-aliasing/render.sh          # about 10 s
```

This writes `renders/m3-tanh-aliasing/<input>/<shaper>/audio.wav` (32-bit float WAV, stereo), each render's `manifest.json` and `metrics.json`, the project copies that were rendered (`renders/m3-tanh-aliasing/projects/`), and `index.tsv`. It reads `projects/kick` and `projects/studio` but never writes to them.

| Input | What it is | Rate, length | Shaper settings |
|---|---|---|---|
| `sweep` | A sine swept exponentially from 100 Hz to 10 kHz over 8 s (`inputs/sweep/`) | 48 kHz, 8 s | drive 20, output gain 0.18 |
| `bass` | An eighth-note saw bass line (A1 A1 C2 A1 E2 D2 C2 G1 at 120 bpm), low-passed at 900 Hz, enveloped (`inputs/bass/`) | 48 kHz, 8 s | drive 8, gain 0.16 |
| `kick` | `projects/kick` as committed: its shaper at drive 2.5, up to 4 on an accent | 96 kHz, 8 s | gain 0.18 (committed: 0.75) |
| `studio` | `projects/studio` as committed: limiter (drive 1), tape-delay saturation in the feedback loop (drive 2), 909 kick (drive 0.6) | 48 kHz, 16 s | gain 0.2 (committed: 0.25) |

**How to listen.** Use headphones or monitors at a comfortable level, in a player that can switch between files: Audacity with the tracks stacked and soloed in turn, or Finder's Quick Look one file after another. Every file of an input has the same output gain, so levels match within a fraction of a dB. Do not compare levels across inputs. Suggested order:

1. **`sweep`, `tanh_shaper` then `tanh_os8_adaa1`.** At drive 20 the sine becomes nearly a square. The aliases are tones that do not follow the sweep: they whistle downward or wander while the sweep rises, from about 1 kHz upward and loudest in the last third. The current shaper and `tanh_adaa2` should have them clearly. `tanh_os2_adaa1`, `tanh_os4_adaa1` and `tanh_os8` should have less, mainly near the end. `tanh_os8_adaa1` should have the least. The oversampled files also keep more top end, because ADAA's droop is gone, and they peak higher (about 1.27× near the end; see "Peak").
2. **`bass`.** Here aliasing shows up as fizz or roughness on top of the bass, not as tones. Listen whether the current shaper's top sounds grainy next to the oversampled ones, or whether the difference cannot be heard.
3. **`kick`.** The kick's tone sits below 200 Hz at drive 2.5–4, so little aliases. Expect the files to sound nearly or exactly alike. That is the point of this input: on the kick, promotion would cost a re-recorded golden for little audible change. The oversampled files start 74 frames (0.77 ms) later.
4. **`studio`.** This is what promotion would do to the owner's demo. The differences, if any, are in the tape echoes' saturated repeats and on the master limiter. The oversampled shapers add ≈ 74 frames (1.5 ms at 48 kHz) to every pass through the tape delay's feedback loop, so each repeat lands about 1.5 ms later than the one before (the n-th about n × 1.5 ms later). The whole mix is also 1.5 ms late, through the limiter.

**What differs, as facts.** The aliasing tables, the passband table and the peak table above. The delay is 0.5 frame (current), 1 (ADAA2), or 65–74 frames (oversampled). CPU is 55 ns per frame (current) against 97–249 ns.

### The renders (revision `77db2f0` plus this experiment's working tree, release build)

| Input | `std.tanh_shaper` | `lab.tanh_adaa2` | `lab.tanh_os2_adaa1` | `lab.tanh_os4_adaa1` | `lab.tanh_os8_adaa1` | `lab.tanh_os8` |
|---|---|---|---|---|---|---|
| sweep (384 000 frames) | `faf96eb4c86ffe19` | `0ad036309c87d1d1` | `2abce91498018539` | `32084ed3870ece09` | `e522c7c85a8f521d` | `6d2883476672b725` |
| bass (384 000) | `7b6e0b3ece7df6e1` | `b1fd9f3a2c5e3d3d` | `075da89b7cd25f19` | `66aba898d6e29c25` | `107abe154c397789` | `534eb339b35f7569` |
| kick (768 000) | `9e2a88ce8cc94d31` | `c0c7c9c237c11fb5` | `379916bab2843c99` | `b8e28e917e5e6865` | `31a4fb253a69e7f5` | `b683ee10d63d703d` |
| studio (768 000) | `8b099c09c82794f1` | `bb4e91a4a19e96bd` | `33258a717ad3f6e5` | `7646ba4e4617b48d` | `ded5a420abf8f8a9` | `3ecc0293ad626855` |

The values are `audio_fnv1a64`. Peaks run from 0.166 to 0.231, every one at or below 0.25 (`index.tsv`). The kick, studio and bass renders print the projects' existing `kind mismatch` warnings (for example a `Control` envelope wired to a `Frequency` input). These are the same for every shaper.

## Recommendation (the agent's opinion, not a verdict)

`lab.tanh_os8_adaa1` is the only candidate that clears 60 dB with margin under V6 (worst tone 101.6 dB at 96 kHz) and also at 48 kHz (worst 79.5 dB at 8009 Hz). Its passband is flatter than the current shaper's: −0.03 dB against −2.0 dB at 10 kHz at 48 kHz. It costs about 250 ns per frame (2.4 % of a 96 kHz frame, about 4.5× the current shaper) and 73.8 frames of delay. The others:
- `lab.tanh_os4_adaa1` (180 ns) passes V6 at 96 kHz, but only by 2.7 dB at 8009 Hz, and misses at 48 kHz (56.9 dB).
- `lab.tanh_os8` without ADAA misses at 48 kHz (45.7 dB). ADAA at the high rate is what carries the high tones.
- Second-order ADAA alone gains only 5–7 dB over first order, which is far from enough, and its passband droop is worse.

Before promoting, the owner should judge two costs that the numbers cannot settle:
- **The delay.** It is harmless on a kick or a master limiter. Inside the tape delay's feedback loop it compounds, and it shifts every project that uses the shaper by ≈ 74 frames.
- **The overshoot above 1.** It changes the port's declared range and the peak of every project that clips hard.

If the delay matters, the next step I would propose is a **minimum-phase stage 1**. It has the same magnitude response, so in principle the same aliasing figures, with a delay of a few frames instead of 65, at the cost of phase dispersion near the passband edge. It is not built or measured here. Another option is to keep `std.tanh_shaper` as it is and add the oversampled shaper as a separate `std` type, chosen per use. V6 would then apply to that type, which changes what the issue asks for.

## What promotion would change

If a candidate replaced `std.tanh_shaper` (the same type name), these would change and need the golden-update procedure (`docs/testing-and-evaluation.md`, "Updating a golden": a note and a listening verdict per golden):

| Uses `std.tanh_shaper` | Where | Golden or pinned render affected |
|---|---|---|
| `projects/kick` | `composites/kick.toml` (`shaper`, drive 2.5) | `tests/golden/kick`. V3's onset derivation and V8's "half a frame earlier" crossing correction assume a delay of 0.5 frame (`kick_acceptance.rs`). The M1 V14 verdict judged the old sound. |
| `projects/studio` | `project.toml` (`limiter`), `composites/kick909.toml` (`shaper`), `composites/tape_delay.toml` (`sat`, in the feedback loop) | `tests/golden/studio`. The M2.11 verdict judged the old sound. |
| `projects/studio-transport` (the owner's) and its fixture `crates/signal-cli/tests/fixtures/studio-transport` | the same three instances | `tests/golden/studio-transport` |
| `projects/two-studios` (includes a copy of studio-transport) | the same three instances | `tests/golden/two-studios` |
| `projects/library-demo` | `limiter_l`, `limiter_r` | `tests/golden/library-demo` |
| `lib.analog_kick`, `lib.tape_echo` (`crates/signal-lib/lib/`) | `shaper`, `sat` | `crates/signal-lib/tests/pinned.toml` (`analog_kick`, `tape_echo`), and library-demo through them |
| `lib.tanh_shaper_core` | validation reference for `std.tanh_shaper` | `crates/signal-lib/tests/validation.rs` (`tanh_shaper_core_is_within_the_midpoint_bound`) would no longer hold: the delay and the different shape need a new bound or reference |
| `crates/signal-project/tests/fixtures/example` | `composites/voice.toml` | loading tests only, no audio |
| `ui/lib/fixtures/studio-graph.json` | three `std.tanh_shaper` descriptors of the studio's graph | changes if the `out` range or `doc` changes (UI fixture) |
| `scripts/validation-cost.sh` (line 153), `crates/signal-cli/tests/cli.rs` (line 503), `crates/signal-lib/src/lib.rs` (line 164) | `lib.tanh_shaper_core` paired with `std.tanh_shaper` | the validation pairing and its cost script |
| `crates/signal-lib/lib/tanh_shaper_core.toml` | its description ("within [-1, 1]", the 1e-9 + 0.035·Δu² bound against `std.tanh_shaper`) | the text no longer holds |
| `crates/signal-project/tests/diff.rs`, `errors.rs`, `load.rs` | name the type | load-only, no audio; unchanged unless the type is renamed |
| `crates/signal-cli/tests/studio.rs` (lines 270, 282) | list the studio's types | only under a rename |
| `CHANGELOG.md`, `crates/signal-lib/CHANGELOG.md` | | a promotion is a change of sound, recorded there |

Also affected: the `doc` string in `signal modules` and the CLI schema snapshot (`crates/signal-cli/tests/snapshots/cli__schema.snap`); the `out` range [−1, 1]; the scheduler's cost estimate `("std.tanh_shaper", 30)` in `crates/signal-std/src/lib.rs` `COST_NS`; the `std.tanh_shaper` row of `docs/benchmarks.md`; `docs/dsp-conventions.md` ("Oversampling is not used in M1", ADAA's half-frame rule); and the un-ignoring of V6 with its docs. The goldens of `pulse`, `physical` and `ladder` do not use the shaper.

## Tools built for this experiment, and what is left for M3

Built, only what this experiment needed:
- `signal bench --only <PREFIX> [--param NAME=VALUE …]` (`crates/signal-cli/src/bench.rs`). It measures only the primitives whose type name starts with PREFIX, in any namespace, so `lab.*` in a lab build. GPU types are excluded. It can set a parameter on every measured instance, and prints the empty-plan, stimulus and per-instance rows in the same JSON shape as the full bench. Tested for shape in `crates/signal-cli/tests/bench.rs`.
- `crates/signal-cli/tests/tanh_aliasing.rs` (lab builds). It reruns V6's measurement on any shaper, at any rate and drive, together with the passband, delay and peak measurements. Its quick tests check that it reproduces V6's published values, that each candidate's delay is the documented one, and that renders are the same on 1 and 4 threads. The full table is printed by the ignored test:
  `cargo test --release -p signal-cli --features lab --test tanh_aliasing -- --ignored --nocapture`
- `render.sh`: copy, swap the type, render, check the peak, index.

Left for the M3 row: a general `signal compare` (any two or more variants of a project, level-matched, with an aligned A/B player and per-render metrics side by side), a listening-package format with a manifest of its own, and a recorded way for the owner to say whether a package was decision-useful (the M3 exit condition). The `index.tsv` and README here are a hand-made first instance of such a package.

## Open questions for the owner

1. Is ≈ 74 frames of delay acceptable for the shaper? At 48 kHz that is 1.5 ms, and it compounds in the tape delay's feedback loop. Or should a minimum-phase or shorter-latency variant be measured first?
2. Should the oversampled shaper replace `std.tanh_shaper` under its name, which changes six goldens and two pinned library renders, or arrive as a new `std` type next to it? In the second case, V6 would need to be retargeted.
3. Is an output that overshoots 1 (up to 1.27 on hard-clipped high tones) acceptable for a "shaper", or should the output be bounded? Bounding it again would bring back some aliasing.
4. Does the listening package (sweep, bass, kick, studio) help you decide (the M3 exit condition), and what should `signal compare` add?
