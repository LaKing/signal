# Testing and evaluation

How Signal is tested, what the acceptance IDs (V1…) map to, and how golden renders work. The layers follow `01-inception-proposal.md` §9; the acceptance IDs are §8.4.

**Principle: metrics detect defects; humans judge quality.** A passing suite says that nothing measurable is broken: no NaN, no clipping, no DC, triggers on their frames, the same bytes as before. It does not say that something sounds good. Every milestone that changes sound ends with a recorded human verdict (V14 for M1), and no metric replaces it.

## Test layers

| Layer | What it checks | Where |
|---|---|---|
| 1. Unit | DSP formulas against analytic values, time → frame rounding | `#[cfg(test)]` modules in each crate |
| 2. Contract | Every primitive: *k*-partition invariance, reset determinism, finite output under fuzzed input, no allocation, rate agnosticism, denormals (`module-contract.md`) | `crates/signal-contract`, run on every std type by `crates/signal-std/tests/contract.rs` |
| 3. Graph | Flattening, addresses, validation, cycles, schedule | `crates/signal-graph/tests/` |
| 4. Engine | Frame phases, events, parameters, suspension, faults, no allocation (`semantics.md`) | `crates/signal-engine/tests/` |
| 5. Project and CLI | Loading, diagnostics, schema, `signal render` artifacts and exit codes | `crates/signal-project/tests/`, `crates/signal-cli/tests/cli.rs` |
| 6. Acceptance | V1–V8 on the test project `projects/kick`; the M2 test project `projects/pulse` (M2-7) | `crates/signal-cli/tests/kick_acceptance.rs`, `crates/signal-cli/tests/pulse.rs` |
| 7. Golden renders | A fixed render's manifest, audio and log hashes | `tests/golden/<project>/`, checked by the acceptance suite |
| 8. Metrics and analysis | Peak, RMS, mean, non-finite and subnormal counts (`metrics.json`); 4× peak, aliasing ratio, zero-crossing pitch | `crates/signal-eval` |

Real time against offline (M2-3): `crates/signal-cli/tests/realtime.rs` checks V9 (60 s of `projects/kick` through the null driver, three callback-size seeds, equal to the offline render as f32) and replays a session issued through the `Controller` with `signal replay`; `crates/signal-engine/tests/realtime_no_allocation.rs` checks V11 (no heap call inside `Runner::process` over 12 000 callbacks); `crates/signal-engine/tests/realtime.rs` covers queue overflow, the clamp, channel mapping, stamping and late commands.

Later: benchmarks (M2, V13), comparison and listening packages (M3), soak runs (V12).

Run everything with `cargo test --workspace --locked --offline`. The acceptance suite renders about 18 million frames in the debug profile; its longest test, V5, renders 64 bars at 96 kHz. V9 adds about 23 million (one offline render and three null-driver runs of 60 s at 96 kHz, the runs in parallel).

## Acceptance IDs (M1)

| ID | Test in `kick_acceptance.rs` | What it shows |
|---|---|---|
| V1 | `v1_same_seed_same_audio_hash` | Two renders of the same project and seed have the same audio and log hashes and identical `audio.wav` bytes. |
| V2 | `v2_every_primitive_used_has_contract_tests` | Every primitive the project uses is a `std.*` type with the contract suite run on it in `signal-std/tests/contract.rs`. The contract tests themselves run there, not again here. |
| V3 | `v3_triggers_on_the_integer_grid_and_first_onset` | Every beat and ghost trigger in `log.jsonl` is on the transport's integer formula (ghosts recomputed from the seed's stream), accents fall on every third beat, nothing is discarded, and the first kick's first non-zero sample is frame 1 (the derivation is in the test). The owner accepted this as satisfying V3 for M1 (sq-22037848): every beat or ghost trigger frame matches the transport formula, and the first kick's audio onset is checked; later onsets in the audio are not. |
| V4 | `v4_seed_changes_only_ghosts` | Seeds 42 and 43 give the same beats and accents and different ghosts; the audio is bit-identical up to the first frame where the ghosts differ. |
| V5 | `v5_floors_over_64_bars` | 64 bars: no NaN or infinity, **4× oversampled** peak ≤ −0.3 dBFS, \|mean\| < 1e-4 per channel. |
| V6 | `v6_shaper_aliasing_at_least_60_db` | `std.tanh_shaper` at drive 20 on 1009, 2003, 4001 and 8009 Hz sines at 96 kHz: inharmonic energy ≥ 60 dB below the fundamental. **Ignored** until the shaper fix; see below. |
| V7 | `v7_delay_free_feedback_is_rejected` | The kick with its feedback loop closed without the unit delay: `signal render` exits 1 and names the cycle. |
| V8 | `v8_pitch_at_every_rate` | Renders at 44.1, 48, 96 and 192 kHz. The first kick's early sweep (trigger to 105 ms) follows f(t) = 48 + 132·e^(−t/35 ms), 180 Hz at the trigger, period by period within a tolerance derived from the kick; its tail (from 250 ms after the trigger to the next trigger) is 48 Hz within 1 % by zero crossings. See below. |
| Golden | `golden_kick_16_bars` | The seed-42, 16-bar render matches `tests/golden/kick/manifest.json`. |
| V14 | (human) | The PO listens to a render of `projects/kick` and records a verdict. Recorded for M1 in `experiments/m1-kick-v14/verdict.md` (the 60 s seed-42 render). |

Thresholds are fixed by the PO and are never lowered to make a test pass. If a measurement misses one, the test is marked `#[ignore = "…"]` with the measured values, and the values go to the PO.

Printed values (V4, V5, V8) are shown with `cargo test -p signal-cli --test kick_acceptance -- --nocapture`; V6's with `cargo test -p signal-cli --test kick_acceptance -- --ignored v6 --nocapture`.

### The V6 measurement

`signal_eval::aliasing` windows the signal with a periodic Hann window, takes a real FFT, and compares the energy within ±2 bins of the fundamental with the energy further than ±2 bins from every harmonic below Nyquist. The test analyses 96 000 samples (1 s, 1 Hz bins) after 1000 frames of start-up: a whole number of periods of every tone, so the window adds no leakage.

**Tones.** The tones are fixed at 1009, 2003, 4001 and 8009 Hz (owner decision sq-4aece389, option A). They do not divide 96 kHz, so the shaped tone is not periodic in whole frames and its aliases fall between the harmonics, where they are counted. The original tones, 1, 2, 4 and 8 kHz, divide 96 kHz: the shaped tone repeated every 96, 48, 24 or 12 frames, every alias landed exactly on a harmonic, and the test passed for naive `tanh` as well (ADAA 253–306 dB, naive 244–294 dB), so it could not tell antialiasing from none. That check is removed, not kept as a sanity check, because it measures nothing.

**Status: ignored pending the shaper fix.** The threshold stays at 60 dB. The current first-order ADAA `std.tanh_shaper` misses it, so the test is marked `#[ignore]` with the measured values, and it prints the shaper's ratio and that of naive `tanh(20·x)` on the same sine:

| Tone | ADAA (`std.tanh_shaper`) | Naive `tanh` |
|---|---|---|
| 1009 Hz | 49.0 dB | 43.8 dB |
| 2003 Hz | 33.5 dB | 27.3 dB |
| 4001 Hz | 26.1 dB | 18.5 dB |
| 8009 Hz | 22.0 dB | 13.1 dB |

Bringing the shaper to ≥ 60 dB is follow-up issue `2026-10-11-follow-up-bring-std-tanh-shaper-aliasing-to-60-db-on-off-div`, tracked for the M3 ADAA-versus-oversampling experiment. The `#[ignore]` is removed when that lands. The owner waived V6 for M1 on these values (sq-22037848; `roadmap.md`).

### The V8 measurement

**Early sweep.** The kick's pitch is f(t) = 48 + 132·e^(−t/τ) Hz, τ = 35 ms, t from the trigger frame (`projects/kick/composites/kick.toml`). From the first kick's trigger to 3τ (105 ms, before any ghost retrigger), the test locates every upward and every downward zero crossing of the left channel by linear interpolation, takes each crossing half a frame earlier (the shaper's delay), and reads each period between two like crossings as 1/(period). That reading is the mean of the instantaneous frequency over the period, so it is compared with the mean of f(t) over the same period, not with one point of the curve. The first period starts at the first downward crossing (≈ 2.8 ms); the output starts from 0 on the trigger frame, which is not a crossing. The onset is checked through the curve: f(0) = 180 Hz, and the test prints the onset the first period implies.

**Tolerance.** Per period, the sum of what the kick's own stages add to the reading, derived from its constants, and the measurement's resolution at the rate (`sweep_tolerance` in the test, with the derivation):

- the DC blocker's phase lead atan(10 Hz / f), which grows as f falls (≈ 0.23 Hz on the first periods, rate-independent);
- the oscillator's frame-by-frame phase sum and the feedback phase modulation, both offsets proportional to f/sr (≈ 0.06 Hz on the first period at 44.1 kHz, 0.01 Hz at 192 kHz);
- linear interpolation of a crossing of the shaped sine (below 0.001 Hz).

The resulting tolerance is 0.14–0.27 Hz. The test also requires it to reject τ counted in frames of another of the four rates (about 4 Hz off or more); a 1 % error in τ reads about 0.5 Hz off near t = τ and fails.

**Tail.** From 250 ms after the trigger to the next trigger, the frequency by zero crossings (`signal_eval::zero_crossing_frequency`) is 48 Hz within 1 %.

## Human verdicts

A milestone's listening verdict (V14 for M1) is a file `experiments/<id>/verdict.md`, the location `01-inception-proposal.md` §9 plans for verdicts, chosen so that the M3 `experiments/` workflow need not move it. Its format is provisional until Q11 is decided. It holds the verdict verbatim, who gave it and when, the owner decision that relayed it, and the identifiers of the render it judges: project, seed, length, sample rate, frames, `audio_fnv1a64` and `log_fnv1a64`.

| Verdict | File |
|---|---|
| M1, V14 | `experiments/m1-kick-v14/verdict.md` |

## The M2 test project: `projects/pulse`

`crates/signal-cli/tests/pulse.rs` checks `projects/pulse` (M2-7): the project is at 96 kHz with seed 7 and uses the documented patch, every primitive in it has the contract suite; 16 bars render at 44.1, 48, 96 and 192 kHz with every sample finite (`is_finite()` per sample, since a peak taken with `f64::max` ignores NaN), a 4× oversampled peak ≤ −0.3 dBFS and |mean| < 1e-4 per channel; and `golden_pulse_16_bars` matches `tests/golden/pulse/manifest.json`. The same file checks that a `std.delay` closes a feedback loop without a `std.unit_delay` (accepted, finite, bounded), and that the same loop through a direct primitive is rejected. Printed values: `cargo test -p signal-cli --test pulse -- --nocapture`. The 60 s seed-7 render for the owner's listening verdict is `signal render projects/pulse --seconds 60 --out renders/pulse-seed7/`.

## Golden renders

`tests/golden/kick/manifest.json` is the `manifest.json` (`semantics.md` §9) of `signal render projects/kick --frames 2792727` (16 bars at 132 bpm and 96 kHz, seed 42), with the fields that depend on the build rather than the sound removed:

| Removed field | Why |
|---|---|
| `provenance.signal_revision` | Changes with every commit. |
| `provenance.signal_dirty` | Changes with the working tree. |
| `provenance.rustc_version` | Changes with the toolchain. |
| `provenance.cargo_lock_hash` | Changes with any dependency update, sound or not. |
| `debug_assertions` | Changes with the build profile (`cargo test` vs `--release`). |

What remains is `seed`, `sample_rate`, `frames`, `provenance.project_file_hashes`, `log_fnv1a64` and `audio_fnv1a64`. Any change to the sound changes the audio hash; any change to an event changes the log hash; any edit of a project file changes its hash. The test also asserts that exactly these fields remain, so a field added to the manifest later must be added to the golden or listed above.

`tests/golden/pulse/manifest.json` is made the same way from `signal render projects/pulse --frames 3072000` (16 bars at 120 bpm and 96 kHz, seed 7), and is updated by the same rules.

### Updating a golden

A golden changes only for an **intentional** change of sound or of the project files:

1. Write a note saying what changed and why the sound is meant to change, and get it reviewed with the change.
2. Get a listening verdict on the new render from the PO, recorded in the repository like the milestone verdicts (V14).
3. Replace the golden with the fresh manifest that `golden_kick_16_bars` (or `golden_pulse_16_bars`) prints when it fails.

A golden that changes without both the note and the verdict is a regression, whatever the metrics say.
