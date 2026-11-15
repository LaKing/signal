# ADR 0056: Stateful and generative (M5)

- **Status:** Accepted (operator, 2026-11-15, under the owner's delegation of technical decisions: "Minden eddigi megfogalmazott mérföldkő készüljön el")
- **Milestone:** M5, stateful and generative
- **Related:** ADR 0003 (musical time is a library's), ADR 0008 (seeded randomness), ADR 0028 (patterns as tables), ADR 0033 (the playhead), ADR 0016 §latent and `module-contract.md` "Trust boundary" (latent modules), `01-inception-proposal.md` §8 (M5)

## Context
The inception proposal's M5: "async helpers with deterministic apply frames; sampler primitive (via an async loader); stateful sequencing, hierarchical musical time in `transport`; test project with a ≥ 20 min generative performance", accepted when "20-min render deterministic; async results applied at declared frames", exited by "long-form listening review: development, recurrence, contrast audible".

What exists: latent modules (a declared latency, results due at a frame fixed by counting blocks, `due`/`miss`, used by the GPU types since M2.7); pattern files compiled into an instance's table when the plan is built (ADR 0028), swapped in at a plan swap's frame; `std.playhead`, a position in beats (ADR 0033); seeded per-instance random streams (ADR 0008).

## Decision

### 1. The async loader is the plan: data files compiled into tables
Loading a file is I/O, which the audio thread never does (`realtime-rules.md`). The control thread already does it for patterns: it reads `patterns/<name>.toml` when it builds a plan, compiles it into the instance's `Table` (`Registry::set_table_rule`), and the plan swap that carries it applies on the swap's frame, the frame every live edit is applied on (`semantics.md` §12, "Pushed input and effective frames"). That is an async loader with a deterministic apply frame: offline, the table is in the plan from frame 0; live, at the swap's frame, recorded in the session with the edit, so a replay applies it on the same frame.
- **A sample is a pattern of kind `"sample"`** (`patterns/<name>.toml` with `file = "<path>.wav"`), named by the instance's `pattern` as any pattern is: its name, kind check, table compile and swap are the pattern machinery's. The WAV (mono or stereo, at the project's rate, at most 2²⁰ values: about 21 s mono, 10 s stereo at 48 kHz; `MAX_TABLE_VALUES`) is read when the project is read from its folder; the project's files are text, so it is read beside them, not among them. It is a **project file**: packed (`signal pack`), hashed into a render's `project_file_hashes` under its path, and copied byte for byte into every version of a session (`session-format.md` §3), so a replay needs nothing outside the session. A file of another rate is refused with the reason (resampling is a later step, rule 8). An edit of the WAV alone is read at the project's next reload; it is not watched on its own.
- **Latent modules stay the path for unbounded computation** (a model, a device): they already give "results applied at declared frames" (`module-contract.md`, block-counting invariant), which their tests check. M5 adds no second mechanism.

### 2. `std.sampler`
Plays its table on a trigger.
- **Ports:** `trigger` (events: each starts the sample from its first frame, cutting the one playing, with a 64-frame raised-cosine fade of the old voice to avoid a click), `rate` (signal, playback rate in `[0, 4]`, 1 while unconnected: linear interpolation between frames), outputs `l`, `r` (a mono sample on both). **Parameters:** `gain` (0–4, default 1), `start` (0–1 of the length), `loop` (0/1).
- No table (no `sample`), silence. Bit-exact; allocates nothing in `process`.

### 3. Hierarchical musical time: `std.meter`
Bars, sections and phrases from a position in beats (`std.playhead.position`), so every level is exact from the playhead and survives tempo changes and jumps (ADR 0033).
- **Input:** `position` (beats). **Parameters:** `beats_per_bar` (1–32, default 4), `bars_per_section` (1–256, default 8), `sections_per_phrase` (1–64, default 4).
- **Outputs (signals):** `bar` (whole bars since 0), `beat_in_bar` (0 ≤ x < beats_per_bar, fractional), `section`, `bar_in_section`, `phrase`, `section_in_phrase`, `bar_phase` (0–1). **Events:** `bar_start`, `section_start`, `phrase_start` on the frame the position crosses each boundary forward (a jump backward emits none; a jump forward emits one).
- `std.transport` itself is unchanged (its golden renders stay): the hierarchy is the meter's, fed by the playhead, as the follow inputs are (ADR 0033).

### 4. Stateful sequencing: `std.markov`
A seeded state machine that moves on each trigger.
- **Ports:** `step` (events), `reset` (events); outputs `state` (0 … N−1) and `changed` (events on each move).
- **Its table** (a pattern of kind `"chain"`, `patterns/<name>.toml`, ADR 0028's file form): `states = ["a", "b", …]` and under `[weights]` one row per state, `a = [w₀, …]`, compiled into cumulative rows; a move draws the next state from the current row with the instance's random stream (ADR 0008), so a render is the same for every seed it is given and differs between seeds. A row of zeros stays put.
- With `std.meter`'s `section_start` as `step`, the sections of a piece follow the chain: recurrence (a state returns), contrast (rows that leave), development (rows that move forward).

### 5. The test project: `projects/generative`
At least 20 minutes, never the same bar twice in a row but recognisably one piece: a `std.playhead` at 100 bpm and a `std.meter` (4/4, 8-bar sections, 4-section phrases); a `std.markov` over the form (`start`, intro, groove, build, peak, breakdown: `start` is where the chain stands before the first section, so the meter's first `section_start` makes it the intro) stepped on `section_start`, and a second over four chords (Am, F, C, G) stepped on `bar_start`; the studio's 909 drums from one step pattern, a bass and a pad (`moog_voice`) transposed by the chord, a pentatonic lead, and a texture sample rendered from the project's own `moog_voice` (`samples/texture.wav`, 1.5 s, no third-party audio) played on each bar at the chord's pitch; each layer's level per form state from a `std.table8` (no interpolation) eased by a `std.lag`; `lib.mixer`, a reverb on the pad and the echo on the lead; dc block and tanh on the bus, the output at 0.25.
- **Safe gain:** the 20 minutes peak at −13.8 dBFS.

## Consequences
- `signal-project`: the pattern kinds `"chain"` and `"sample"` (the WAV read with the project, packed, hashed, copied into sessions); `signal-std`: `std.sampler`, `std.meter`, `std.markov` and their table rules.
- `project-format.md` (§`sample`, the chain pattern), `module-contract.md` (the three types), `semantics.md` §12 (a sample swapped in at the swap's frame).
- No golden changes.

## Verification
1. A sample's table is its WAV's words; a missing, out-of-folder, wrong-rate or too-long file is refused with its reason; `signal pack` packs it; a render's manifest hashes it.
2. `std.sampler` plays the table from the trigger's frame, sample for sample at rate 1, with the fade on a retrigger; the split never matters.
3. `std.meter` against hand-computed bars and boundaries, a tempo change and a jump each way.
4. `std.markov`: the same seed gives the same states, another seed others; the empirical transition frequencies over 10⁵ steps within 1 % of the weights.
5. A session copies its samples; `signal replay` of it, with the project's WAV deleted, renders bit for bit what the render did (`crates/signal-cli/tests/sampler.rs`). A sample swapped in by a live edit is a pattern edit, applied at the swap's frame as every pattern edit is (`semantics.md` §12).
6. `projects/generative`: its 20-minute render is the same on 1 and 4 threads and in blocks of 64 and 256 (a release-build run, its hash recorded in `docs/benchmarks.md`; the test suite checks 2 minutes); every one of the five sections occurs within the first 20 minutes, and no section follows itself more than its row allows.
7. The exit: the owner listens to the 20 minutes and records whether development, recurrence and contrast are audible.
