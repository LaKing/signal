# Session format

**Status: normative** (ADR 0006, 0008, 0009). A **session** is the text record of a live run: the project it started from, every plan version swapped in, every command applied, each with its **effective frame**, (M2.7-4) the outcome of every latent module's due block, and (M2.7-8) the hash of every delivered result and the machine it ran on. `signal replay` renders it offline exactly as it was played, bit-identically on the same build (V10), and checks that it did (§6); `signal replay --ideal` renders it with every latent result awaited (`semantics.md` §15). `signal-project`'s `session` module implements it, so that `signal` and `Signal.app` share it: `SessionRecorder` writes a session, `LiveRecorder` records a real-time run into one, `Session::load` reads and validates one, `replay` renders one. `signal-cli` holds only the `signal replay` command.

Edit commands come from project changes through `signal_project::diff` (§5).

## 1. The session directory

```text
<session>/
  session.toml                       format version, run settings, provenance, machine, plan versions
  projects/v0/project.toml           version 0: the project the engine was built from
  projects/v0/composites/<name>.toml
  projects/v1/…                      one folder per plan version swapped in
  edits.jsonl                        every command, with its effective frame
  boundaries.jsonl                   latent modules only: each due block's outcome (§4a)
  boundary-data.f32                  with --record-boundary-data: the delivered results (§4a)
```

All files are UTF-8, except `boundary-data.f32`. Nothing else in the directory is read. A session holds no wall-clock time: recording the same commands twice gives the same bytes, except for the boundary records of a project with latent modules (§4a), which say when results arrived in that run and so may differ between two runs of the same commands.

## 2. `session.toml`

```toml
format_version = 1
sample_rate = 8000
seed = 3
frames = 40000

[provenance]
signal_revision = "cf613bd1fc85a9a8dc6de643e68080ea99ea1265"
signal_dirty = false
rustc_version = "rustc 1.99.0 (b940084d7 2026-09-28)"
cargo_lock_hash = "…"

[provenance.project_file_hashes]
"project.toml" = "cda28b8f6591f20c"

[machine]
cpu_arch = "aarch64"

[[machine.devices]]
backend = "Metal"
name = "Apple M5 Max"
vendor = 0
device_id = 0
driver = ""
driver_info = ""

[[versions]]
version = 0

[versions.files]
"project.toml" = "cda28b8f6591f20c"

[[versions]]
version = 1

[versions.files]
"project.toml" = "4674961305edf113"
```

| Key | Meaning |
|---|---|
| `format_version` | This format: 1. Another value is an error. |
| `sample_rate` | The engine's master rate (≥ 1). The session's value is used, not a project file's. |
| `seed` | The engine's master seed, 0 to 2⁶³−1. |
| `frames` | Optional. The frames the recorded run played (M2-4): `signal play` records it when the run ends, after the last `frontier`. Every command's frame is below it. Without it, replay renders the last command's frame plus one second (§6). Sessions written before it existed have no `frames` and replay as before; the format version stays 1. |
| `[provenance]` | The recording build, field for field as in a render manifest (`semantics.md` §9, `project-format.md` §7). `project_file_hashes` are version 0's. |
| `[machine]` | M2.7-8: the machine that recorded the run. `cpu_arch` is the CPU architecture (`aarch64`); `[[machine.devices]]` lists every compute device the run's latent modules used, each a GPU adapter as its backend describes it: `backend`, `name`, `vendor` and `device_id` (PCI ids, 0 where the backend has none), `driver` and `driver_info`. No device: no `devices` key. `signal play` records every device of the GPU pool its run opened (M2.7-2, sorted by id; `--features gpu` or `lab`); without them nothing links a GPU. With `[provenance]`'s build it is what an exact replay that must recompute results compares first (§6). Sessions written before it existed have none, and replay as before (§6); the format version stays 1 (a build without it refuses a session with one, as for any unknown key). |
| `[boundaries]` | Optional (M2.7-4), present when a version has a latent module (§4a): `complete` (bool), `data` (bool: delivered results recorded), `delivered` and `missed` (the counts of the lines of `boundaries.jsonl`), and when incomplete `lost` (outcomes the run had that the recorder did not get) and `reason`; with data, `data_fnv1a64`, the FNV-1a 64 of `boundary-data.f32`. Sessions written before it existed, or of projects without latent modules, have none; the format version stays 1. |
| `[[versions]]` | Every plan version in order: `version` is 0, 1, 2, …; `files` maps each project file of `projects/v<N>/` (path relative to that folder) to its FNV-1a 64, 16 lowercase hex digits. |

Unknown keys are errors.

## 3. `projects/v<N>/`

An exact copy of the project files (`project-format.md` §1) of plan version *N*. Version 0 is the project the engine was built from; version *N* ≥ 1 is the one the *N*-th swap put in. Every file must be listed in `[versions.files]` with its hash, and every listed file must exist; a changed copy is an error. The run settings of the copies are not read: the session's `sample_rate` and `seed` apply.

Every version must have exactly one audio sink (`semantics.md` §8), at the same address and with as many signal inputs as version 0's.

## 4. `edits.jsonl`

One JSON object per line, with a newline after every line:

```json
{"frame":100,"seq":1,"kind":"set","target":"/osc.freq","value":550.0}
{"frame":200,"seq":2,"kind":"ramp","target":"/out.gain","to":0.25,"frames":400,"curve":"linear"}
{"frame":250,"seq":3,"kind":"trigger","target":"/env.trigger"}
{"frame":1000,"seq":5,"kind":"swap","version":1}
{"frame":1500,"seq":8,"kind":"suspend","address":"/osc2"}
{"frame":1540,"seq":9,"kind":"swap","version":2}
{"frame":2600,"seq":11,"kind":"resume","address":"/env"}
```

| `kind` | Fields | Engine call |
|---|---|---|
| `set` | `target` (port address), `value` | `Engine::push_external`, payload `set` |
| `ramp` | `target`, `to`, `frames`, `curve` (`"linear"`) | `push_external`, payload `ramp` |
| `trigger` | `target` | `push_external`, payload `trigger` |
| `suspend` | `address` (instance address) | `Engine::push_suspend` |
| `resume` | `address` | `Engine::push_resume` |
| `swap` | `version` | `Engine::prepare_swap` and `commit_swap` with `projects/v<version>/` |

Writers put the fields in the order shown; readers accept any order. Numbers use the shortest form that reads back to the same value, and a reader must parse them correctly rounded, so that every value is the recorded f64 bit for bit (`signal-project` enables serde_json's `float_roundtrip`; its default parser can be off by one ulp). Unknown fields are errors.

### Rules

- **`frame` is the effective frame**: the frame from which the command applied, as the log records it (`semantics.md` §12).
- **Order.** Lines are strictly increasing by (`frame`, `seq`). `seq` is unique in the session; gaps are allowed (a command the engine refused is not recorded, and its `seq` stays unused).
- **Sequence numbers.** For `set`, `ramp` and `trigger`, `seq` is the external event's sequence number, which `log.jsonl` shows as `source.external.sequence`.
- **Swaps.** The *k*-th swap names version *k*, which `session.toml` lists, and every listed version after 0 is swapped in. At most one swap per frame. A frame's swap applies before the frame's other commands, whatever their `seq`, as the engine commits it before the frame runs. The plan of version *N* is given `ExecutionPlan::version` *N*, so the log's `plan_version` names it.
- **Every command takes effect.** Each command is judged against the version running at its frame, with suspension state carried by survivors across swaps (`semantics.md` §12). A session whose command the engine would discard is malformed:
  - a `set`, `ramp` or `trigger` target must be an event input or a parameter of an instance of that version (a `trigger` to a parameter is accepted: the engine logs it as a `discard`);
  - a `suspend` names a running instance, a `resume` a suspended one;
  - an instance takes at most one `suspend` or `resume` per frame.
- **Removal** is recorded as it was applied (§5): a `suspend` at *s*, then a `swap` at *s* + *F*.

### Effective frames, for drivers

A driver records the frame the engine applies a command at, not the frame it asked for. With *n* the engine's next frame when the command is pushed:

- an external event applies at `max(stamp, n, the last event's frame)`;
- a suspend or resume applies at `max(stamp, n, the last suspend or resume's frame)`;
- a swap committed directly with `Engine::commit_swap` (the offline driver, `signal replay`) applies at *n*, the next frame; a swap sent through the real-time `Controller` applies at `max(stamp, n, the last swap's frame + 1)`, and is reported at the frame it committed.

`SessionRecorder::event` and `SessionRecorder::edit` compute these, reserve a `seq`, and check the command against the version running now; the driver pushes what they return and records it with `applied_event`/`applied_edit` only once the engine has accepted it. Both `applied_event` and `applied_edit` check the command again against the version running at its effective frame, and refuse one the engine would discard. `applied_swap` records a swap (`applied_swap_as` with a `seq` the caller chose); record it before input meant for the instances it adds. `SessionRecorder::write` validates the whole session as `signal replay` would and writes nothing if it is invalid.

### Recording from the real-time runner

The real-time `Controller` (`signal-engine`) knows nothing of sessions. `LiveRecorder` is the `ControlSink` its `poll` feeds, and records into a `SessionRecorder`:

- **`seq` is the Controller's command id** (an event's id is also its sequence number), so `log.jsonl` and `edits.jsonl` name events alike.
- **Frames come from the runner's reports**: an event, suspend or resume is reported with its effective frame when the runner takes it; a swap only when it commits, so a swap the engine refused is never recorded.
- **Records follow the frontier.** Reports arrive in the order the runner made them, not the order commands apply: a swap sent first but stamped later is reported after an edit stamped before it. A report waits until the runner has run past its frame (the `next_frame` of the status `poll` reads first), then is recorded in the order the engine applied it: by frame, then swap, suspends and resumes, events, then id. So each command is checked against the version running at its own frame. After the driver stops, call `frontier` once more with the Runner's own next frame: the last status may have been dropped. `LiveRecorder::finish(next_frame)` does that and records the session's end (`frames`, `SessionRecorder::set_end`), so replay renders exactly what was played.
- **Plan versions.** `LiveRecorder::swap_plan` gives the plan to swap to, its `version` set to the session version it will be recorded as.
- **Boundary outcomes** (§4a) arrive through `ControlSink::boundary` and are recorded at `finish`, with their completeness judged from the runner statuses `poll` passes to `ControlSink::status`. `LiveRecorder::with_boundary_data()` records delivered results too (run the engine with `EngineConfig::record_boundary_data` and a `RealtimeConfig::boundary_data` queue). **An embedder must pass the stopped runner's own status (`ControlSink::status(&mut recorder, &runner.status())`) before `finish`**, as `signal play` does: the runner's last status may not reach the control thread, and without one of the final frame the records are incomplete.

A command the recorder refuses is one the engine discarded; it is listed in `LiveRecorder::refused` and not recorded.

## 4a. Boundary records

M2.7-4 (audit finding 2). A latent module (`module-contract.md` "Declared latency") may miss a due result in real time and play silence for that block, where an offline render would wait for it. So that `signal replay` reproduces what was played, a session of a project with a latent module records each due block's outcome. `boundaries.jsonl` has one line per due block that ran, ordered by (`frame`, `instance`):

```json
{"frame":512,"instance":"/bank","outcome":"delivered"}
{"frame":768,"instance":"/bank","outcome":"missed"}
{"frame":1024,"instance":"/bank","outcome":"delivered","words":256}
```

- `frame` is the due frame (the first of the instance's block), below `frames`; `instance` a latent instance of the version running at that frame.
- `outcome` is `delivered` (the result arrived in time) or `missed` (it did not: a `late_result` fault in the log, and silence for the block).
- A `missed` line may have `cause` (M2.7-5): the `cause` of its `late_result` fault (`semantics.md` §9), when the result was lost and the module said why, e.g. `{"frame":768,"instance":"/bank","outcome":"missed","cause":"mapping"}`. An exact replay logs it again. An unknown cause, or a cause on a `delivered` line, is an error. Sessions written before it existed have none; the format version stays 1 (a build without it refuses a session with a cause, as for any unknown field).
- With `--record-boundary-data` (`[boundaries] data = true`), a delivered line also has `words`, the number of `f32` words of the delivered result (`StateOutputPrimitive::delivered_result`); the words are in `boundary-data.f32`, little-endian, in line order. With them, replay shows the recorded result instead of asking the device, so it does not need the same GPU.
- M2.7-8: a delivered line has `fnv1a64`, the FNV-1a 64 of its result's words as `boundary-data.f32` would store them (`f32`, little-endian; `signal_core::fnv1a64_words`), 16 lowercase hex digits, after `words`: `{"frame":1024,"instance":"/bank","outcome":"delivered","words":256,"fnv1a64":"8c2b0d4e1f3a5b67"}`. The audio thread computes it for every delivered result, with or without data, allocating nothing; a missed line has none (it shows no result). With data, the line's words must have that hash, or the session is malformed. Every latent module exposes its result (`module-contract.md` "Declared latency"), so every delivered line of a session recorded since has one. Sessions written before it have none and replay as before, unverified (§6); the format version stays 1.
- Unknown fields are errors; the counts must match `[boundaries]`, and the data its length and hash.

**Completeness.** The outcomes travel from the audio thread through the runner's log queue (data through a boundary-data queue), which are lossy. `LiveRecorder::finish(next_frame)` marks the records complete only if the last runner status it received is of `next_frame` and its counts (`RunnerStatus::delivered_results`, `late_results`) equal the outcomes received, with no log record or data dropped (`log_overflows`, `engine_log_overflows`, `boundary_data_overflows` 0). A loss is therefore detected, never silent: `complete = false`, with `lost` and `reason`. A session with incomplete records does not replay exactly (§6).

## 5. From project changes to commands

`signal_project::diff(old, new, registry)` turns a change of a project's files into an edit:

- **parameters only** (same instances, types and connections in the flattened plans): one `set` per parameter whose flattened file value changed (`diff_with` and `DiffOptions::ramp_frames` make them `ramp`s over *N* frames instead);
- **anything else**: `ProjectEdit::Structural` with the new plan, the instances `removed` and `added` (a retyped instance is in both), and `set`s for survivors whose file value changed, since survivors keep their bases across a swap;
- **identical, or only whitespace and comments**: the empty edit;
- **a changed `sample_rate` or `seed`**: an error at that key: the project must be restarted.

A driver removes instances by suspending them at *s* and swapping at `Engine::removal_swap_frame(s)`, so that they fade out. `signal_project::live::send_edit` sends an edit's commands only once its swap is prepared (`Controller::prepare_swap`, then `send_swap`) and the `Controller` can take all of them, suspends, swap and sets, plus the resumes that would undo the suspends (`Controller::available`); otherwise it sends none, so a removal never stops half-way with instances left suspended. Because each removal also reserves its resume, one edit can remove at most about half the command queue (511 instances at the default 1 024). After preparing the swap, which can take long, `send_edit` polls again, so the suspend and the swap are stamped from a fresh status and keep the removal's fade.

## 6. `signal replay`

```sh
signal replay <session> [--out <dir>] [--frames N] [--threads N] [--ideal] [--trust-provenance] [--json]
```

- **Exact replay** (the default, `semantics.md` §15). With boundary records, each recorded outcome is applied at its due frame: a miss without waiting, a delivery with its recorded data when there is some, else by waiting for the same device's result. `audio.wav` and `audio_fnv1a64` store the samples as the device received them (clamped to [−1, 1], non-finite as 0), so the replay equals the live recording (`signal play --record-audio`) bit for bit, and the manifest says `"device_conversion": true`. Incomplete records fail (exit 1) with a message that names `--ideal`; a recorded delivery this machine cannot reproduce (no device, another one, data the module refuses) fails (exit 1); records that do not match the replay's due blocks fail (exit 2).
- **Exact is checked** (M2.7-8). Every delivered result the replay shows, recorded data or recomputed, is hashed and compared with its line's `fnv1a64`. The first block whose result differs, by frame and then instance order, fails the replay with `ReplayMismatch { module, block }` (exit 2, kind `replay-mismatch`), whose message names `--ideal` and `--record-boundary-data`. It takes precedence over every other failure of the same replay, which it may have caused.
- **Provenance, before it starts** (M2.7-8). Recomputed results (delivered lines without data) are the performance's only on the build and machine that recorded them (`same_machine`, ADR 0019 §3). So when a session with `[machine]` has such a line in the frames to render, the replay first compares the recording's build (`signal_revision`, `signal_dirty`, `rustc_version`, `cargo_lock_hash`) and machine (`cpu_arch`, every device) with its own, and refuses (exit 1) on any difference, listing each and naming `--ideal`, `--record-boundary-data` and `--trust-provenance`. It is a pre-check, not the proof: two dirty builds at the same revision compare equal, and the hashes still catch a result that differs. With data for every delivery nothing is recomputed and nothing is compared (another build only warns, as before).
- **`--trust-provenance`** replays exactly despite such a difference: every recomputed result is still checked against its hash, and the manifest says `"trust_provenance": true` — only when the flag overrode a difference, so a manifest with the label is one whose machine or build was not the recording's. It cannot be combined with `--ideal`.
- **Sessions before hashes.** A session with boundary records but no `fnv1a64` on its lines (and no `[machine]`) replays exactly as before, with a warning that its exactness is unverified: how many results could not be checked, and that `--ideal` or a session recorded with `--record-boundary-data` are the alternatives.
- **`--ideal`**: an ideal render. Every latent result is awaited, whatever the session recorded: a fresh offline render of the same edits, which the manifest labels `"ideal": true`.
- A session of a project with latent modules but **no boundary records** (written before them) replays as an ideal render, labelled `ideal`, with a warning that misses were not recorded. A session without latent modules replays exactly as before: its plain replay's manifest has neither key, and `--ideal` renders the same audio and log, its manifest saying `"ideal": true` because that was asked for.
- An exact replay renders at most the recorded `frames`: `--frames` beyond them fails (exit 1), since those frames were never played; `--ideal` renders past the end.
- The JSON output adds `ideal`, `boundaries` (`exact`, `ideal`, `not-recorded`, `not-latent`), `unverified` (results replayed without a hash to check) and `trust_provenance`.

- Loads and validates the session, builds the engine from version 0 with the session's sample rate and seed, and before each frame commits that frame's swap and pushes its other commands at their effective frames. The input queues are sized for the session's busiest frame (`EngineConfig::queue_capacity` is not part of the semantics, `semantics.md` §11).
- Renders `--frames` frames, by default the session's recorded `frames`, else the last command's frame plus one second (`last + 1 + sample_rate`; one second with no commands). Commands after the end are not applied, and the output says how many.
- With `--out`, writes the artifacts of `signal render`: `audio.wav`, `log.jsonl`, `manifest.json` (the replaying build's provenance, version 0's file hashes) and `metrics.json`. Prints the audio and log hashes.
- Warns when the session was recorded by another build (revision, compiler or `Cargo.lock`): bit-exactness holds on the same build (ADR 0008).
- **Exit codes**: 1 for a malformed session (each error as `file:line:column: error: message`, with the file relative to the session directory, line 1 column 1 where no line applies) or an unreadable file, including a `words` count beyond the data in `boundary-data.f32` and recorded data that does not match its line's hash; 1 also for an exact replay that cannot be made (incomplete records, an unreproducible result, frames past the end, another build or machine without `--trust-provenance`); 2 for an exact replay that is not the performance (`ReplayMismatch`), and if the engine discards or refuses input of a validated session (a bug).
