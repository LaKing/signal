# ADR 0028: Patterns as project text: note and step data, sequencers, the tick clock

- **Status:** Accepted (owner's request of 2026-10-17: "make a working basic high-level project: a sophisticated piano-roll-like view driving a Moog emulator, a tape delay, a drum-machine-like sequencer, and a 909 kick made by synthesis, not a sampler, with a beautiful interface that resembles the originals"; design by the operator, delegated). Extends ADR 0009 (project format) and ADR 0025 (project files); builds on ADR 0027 (core set) and ADR 0026 (the editor's write path). Supersedes nothing.
- **Date:** 2026-10-17

## Context
A piano roll and a drum machine edit data that is neither a parameter nor wiring: lists of notes (key, start, length, velocity) and grids of steps. Today an instance key is a number (a parameter) or `gpu` (`project-format.md` §4), and the loader reads only `project.toml` and `composites/` (§1): M2.10-2 left the "table from a project asset" route unbuilt for exactly this reason (ADR 0027, M2.10-2 note). `std.step_seq` holds 8 values as parameters, which does not scale to a 16-step, 8-lane drum pattern or a melody of hundreds of notes. Text stays the source of truth (ADR 0009, 0013): what the piano roll shows must be a file a person can read and diff.

Timing: `std.transport` emits beats, accents and ghost 16ths on an exact integer frame grid (`round_half_up` of the tempo's frame positions), and a tempo change re-anchors the grid on a beat. A sequencer must place notes finer than a beat and stay sample-accurate and deterministic under every k-partition and thread count (ADR 0002, 0008).

## Decision
1. **Pattern files are project files.** `patterns/<name>.toml` (directly in `patterns/`, `<name>` as an instance name) joins the project files of `project-format.md` §1: loaded, validated (unknown keys are errors), hashed into the render manifest (§7), kept by recorded sessions per plan version, packed into `.signal` files (§10). A pattern is data, not logic (ADR 0010 holds): it never contains expressions.
2. **Two kinds, readable as text:**
   - `kind = "notes"`: `length` (beats), `[[notes]]` or an inline array of `{ key, start, length, velocity }`; `key` a number (60 = middle C by the library's convention, a convention of `signal-std`, not of the core), `start` and `length` in beats (finite, `start` ≥ 0, `length` > 0), `velocity` in [0, 1]. Notes are sorted by (start, key) when read; overlapping notes on one key are an error.
   - `kind = "steps"`: `steps` (count), `step` (beats per step, e.g. 0.25 for 16ths), and `[lanes]` as ordered `name = "x...x...X..x...."` strings (`.` rest, `x` hit, `X` accent), optionally a per-lane `level`. Lane order is the sequencer's output order.
   Both carry `schema_version = 1`. The exact schema lives in `project-format.md` (a new §12) and the JSON schema.

   **Note (2026-10-17, operator decision, M2.11-1): the points §2 left open, fixed with the pattern controls.** A lane's level lives in a separate optional `[levels]` table (`kick = 0.9`), not inside `[lanes]`: lanes are plain strings of exactly `steps` cells, every key of `[levels]` must name a lane, a level is in [0, 1] and defaults to 1. `key` is a TOML integer from 0 to 127 (`60.0` is an error), as are `steps` and `schema_version`. `notes` is required in `kind = "notes"` (`notes = []` for none); `[lanes]` is required with at least one lane. Overlap on one key is exact: `a.start + a.length > b.start`, touching allowed. `project-format.md` §12 is the exact schema.
3. **An instance names its pattern** with a reserved key `pattern = "<name>"` (a string, like `gpu`; no composite may export a parameter named `pattern`). A type declares whether it takes a pattern and of which kind; a pattern on a type that takes none, a missing file, or a wrong kind is a load error naming the instance and the file.
4. **The data reaches the instance without breaking the real-time rules.** The type's library (`signal-std`) compiles the parsed pattern into a flat, immutable table of numbers when the plan is built (off the audio thread); the core only carries that table to the instance (domain-free: rows of `f64`, never notes). Changing a pattern file is a **live edit**: the kept instance receives the new table at an edit frame, as a parameter `set` arrives, without allocation on the audio thread and without resetting its position, so editing a playing loop never stops or restarts it. A session records the table change like any edit, and replays it bit-identically.
5. **The tick clock.** `std.transport` gains a `tick` event output at `ppq` ticks per beat (a parameter, default 96, integer), on the same exact frame grid as its beats (tick j at `a + round_half_up((j − ppq·b)·60000·sr / (bpm_milli·ppq))`, re-anchored with the beat on a tempo change), and a `bar`/`reset` relation documented with its accents. Sequencers count ticks; a note at `start` beats fires on tick `round_half_up(start·ppq)`, so positions are exact, deterministic, and follow tempo changes without drift.
6. **Sequencers (native, `std`, category `event`):**
   - `std.note_seq`: a `notes` pattern; inputs `tick`, `reset`; outputs `gate` (signal), `key` (signal, held), `hz` (signal, 440·2^((key−69)/12)), `velocity`, `on` and `off` (events), `position` (beats, for a playhead); monophonic with last-note priority (a later issue may add voices); loops over `length`.
   - `std.step_seq8` (a drum machine): a `steps` pattern of up to 8 lanes; inputs `tick`, `reset`; outputs per lane `t1…t8` (triggers, value = level, accented ×`accent` parameter) and `accent` (trigger), `step` (the current step, for a running light); `swing` parameter (0–75 %, delays even steps by a whole number of ticks).

   **Note (2026-10-17, operator decision, erratum to §6): the accent factor parameter is `accent_level`**, not `accent`: a parameter's name must be unique among the type's ports and parameters (`signal-core`'s `descriptor.rs`), and `accent` is the trigger output. The factor is "×`accent_level`".
7. **Editing from the UI.** `pattern.read` and `pattern.write` in the UI protocol (`ui-protocol.md`), with the same rules as `panel.write` and `patch`: a `base` sha (`conflict` otherwise), the server parses and validates before writing, atomic write, the folder's own files only (no links), a `.signal` project becomes edited; the watch then applies it as a live edit (4). The piano roll and step grid controls (`<signal-piano-roll>`, `<signal-step-grid>`, ADR 0023 controls) edit exactly this text; a playhead comes from the sequencer's `position`/`step` output through an existing stream.

## Alternatives considered
- **Patterns as parameters** (`s1…s128` numbers, as `std.step_seq`): unreadable, unbounded parameter lists, and a note is not a parameter.
- **MIDI files:** binary, not diffable, and MIDI is a later milestone's domain (M6); an import can convert into this text later.
- **Sequencers with their own tempo:** two clocks drift apart; one transport, counted in ticks, keeps every sequencer in phase.
- **A pattern as an event list played by the engine:** puts musical data into the core; the core stays domain-free (AGENTS.md rule 1).

## Consequences
- A new project folder, a reserved instance key, a new project-format section, schema and manifest changes; `.signal` and sessions carry patterns.
- A generic, domain-free "instance table" path in the plan and the live-edit machinery, tested for no allocation, bit-identity across k and threads, and replay.
- `std.transport` gains `tick` (unconnected, it changes no render; goldens stay).
- Two native sequencers, two protocol messages and two controls; the studio demo project (M2.11) is built on them.
