# Session format

**Status: normative** (ADR 0006, 0008, 0009). A **session** is the text record of a live run: the project it started from, every plan version swapped in, and every command applied, each with its **effective frame**. `signal replay` renders it offline, bit-identically on the same build (V10). `signal-project`'s `session` module implements it, so that `signal` and `Signal.app` share it: `SessionRecorder` writes a session, `LiveRecorder` records a real-time run into one, `Session::load` reads and validates one, `replay` renders one. `signal-cli` holds only the `signal replay` command.

Edit commands come from project changes through `signal_project::diff` (§5).

## 1. The session directory

```text
<session>/
  session.toml                       format version, run settings, provenance, plan versions
  projects/v0/project.toml           version 0: the project the engine was built from
  projects/v0/composites/<name>.toml
  projects/v1/…                      one folder per plan version swapped in
  edits.jsonl                        every command, with its effective frame
```

All files are UTF-8. Nothing else in the directory is read. A session holds no wall-clock time: recording the same commands twice gives the same bytes.

## 2. `session.toml`

```toml
format_version = 1
sample_rate = 8000
seed = 3

[provenance]
signal_revision = "cf613bd1fc85a9a8dc6de643e68080ea99ea1265"
signal_dirty = false
rustc_version = "rustc 1.99.0 (b940084d7 2026-09-28)"
cargo_lock_hash = "…"

[provenance.project_file_hashes]
"project.toml" = "cda28b8f6591f20c"

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
| `[provenance]` | The recording build, field for field as in a render manifest (`semantics.md` §9, `project-format.md` §7). `project_file_hashes` are version 0's. |
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
- **Records follow the frontier.** Reports arrive in the order the runner made them, not the order commands apply: a swap sent first but stamped later is reported after an edit stamped before it. A report waits until the runner has run past its frame (the `next_frame` of the status `poll` reads first), then is recorded in the order the engine applied it: by frame, then swap, suspends and resumes, events, then id. So each command is checked against the version running at its own frame. After the driver stops, call `frontier` once more with the Runner's own next frame: the last status may have been dropped.
- **Plan versions.** `LiveRecorder::swap_plan` gives the plan to swap to, its `version` set to the session version it will be recorded as.

A command the recorder refuses is one the engine discarded; it is listed in `LiveRecorder::refused` and not recorded.

## 5. From project changes to commands

`signal_project::diff(old, new, registry)` turns a change of a project's files into an edit:

- **parameters only** (same instances, types and connections in the flattened plans): one `set` per parameter whose flattened file value changed (`diff_with` and `DiffOptions::ramp_frames` make them `ramp`s over *N* frames instead);
- **anything else**: `ProjectEdit::Structural` with the new plan, the instances `removed` and `added` (a retyped instance is in both), and `set`s for survivors whose file value changed, since survivors keep their bases across a swap;
- **identical, or only whitespace and comments**: the empty edit;
- **a changed `sample_rate` or `seed`**: an error at that key: the project must be restarted.

A driver removes instances by suspending them at *s* and swapping at `Engine::removal_swap_frame(s)`, so that they fade out.

## 6. `signal replay`

```sh
signal replay <session> [--out <dir>] [--frames N] [--json]
```

- Loads and validates the session, builds the engine from version 0 with the session's sample rate and seed, and before each frame commits that frame's swap and pushes its other commands at their effective frames. The input queues are sized for the session's busiest frame (`EngineConfig::queue_capacity` is not part of the semantics, `semantics.md` §11).
- Renders `--frames` frames, by default the last command's frame plus one second (`last + 1 + sample_rate`; one second with no commands). Commands after the end are not applied, and the output says how many.
- With `--out`, writes the artifacts of `signal render`: `audio.wav`, `log.jsonl`, `manifest.json` (the replaying build's provenance, version 0's file hashes) and `metrics.json`. Prints the audio and log hashes.
- Warns when the session was recorded by another build (revision, compiler or `Cargo.lock`): bit-exactness holds on the same build (ADR 0008).
- **Exit codes**: 1 for a malformed session (each error as `file:line:column: error: message`, with the file relative to the session directory, line 1 column 1 where no line applies) or an unreadable file; 2 if the engine discards or refuses input of a validated session (a bug).
