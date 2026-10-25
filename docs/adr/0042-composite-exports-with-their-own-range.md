# ADR 0042: Composite exports with their own range

- **Status:** Accepted (operator, 2026-10-25, from the owner's choice). The owner chose, among the follow-ups of the muted-kick report of 2026-10-25, "Saját tartomány" (translated: "their own range"): a composite's exported parameter can declare its own minimum and maximum, for example `attack` 0 to 2, so that its control cannot reach ±10⁶. The design is the operator's, delegated.
- **Builds on:** ADR 0009 (the project format; text is the truth), ADR 0010 (composites hold no logic), ADR 0027 §4 (the built-in library clamps every exported value that scales a signal inside it), ADR 0029 (`signal mcp` and its gain guard), ADR 0041 (snapshots, restored as parameter edits). It supersedes nothing.
- **Date:** 2026-10-25

## Context
An exported parameter took its range from what it reaches: the range its targets share. Many targets are generic: `std.scale`'s `mul`, `std.const`'s `value` and `std.clamp`'s bounds range over ±10⁶, the engine's magnitude bound (`semantics.md` §7). In the studio's `kick909`, `attack` goes straight to `click_level.mul`. Its control in the panel was a slider from −10⁶ to 10⁶. The owner dragged it to its left end. The click's output went beyond ±10⁶, the fault guard muted the instance for the rest of the run, and setting the value back brought no sound (`semantics.md` §7; the host now says so, `ui-protocol.md` `app.muted`).

The library's composites avoid this inside: each exported value that scales a signal goes through a `std.clamp` (`lib.analog_kick`'s `attack` to [0, 2]). That keeps the output bounded, but the control still shows the target's range, and a clamp costs an instance and a connection. What the person should see, and what a value may be, is a property of the export.

## Decision

### 1. The format
The table form of an exported parameter (M2.13-2's `{ to = …, group = … }`) takes `min` and `max`, both or neither:

```toml
[exports.parameters]
tune = "osc.freq"                                         # the range its target has
attack = { to = "click_level.mul", min = 0, max = 2 }     # its own range
glide = { to = ["glide.rise_ms", "glide.fall_ms"], min = 0, max = 2000, group = "pitch" }
```

- Both are finite numbers with `min ≤ max`. One without the other, a non-finite one or `min > max` is a load error at the export.
- Only an exported **parameter** takes a range. An exported input with `min` or `max` is a load error: what reaches an input is a signal, not a value a person sets.
- The plain forms (a string, a list) stay valid and mean what they meant.

### 2. Inside every target's range
The export's range must lie inside the range of each of its targets: a primitive parameter's declared range, or an inner composite export's range (its own, else the one its targets share). With fan-out it must fit every target. Otherwise the plan is refused with `ExportRangeOutside`, at the export ("composite `kick909` export `attack`: its range [0, 2] is not inside the range [0.1, 20] of its target `shaper.drive`"). So an export's range can only narrow, never widen, what a primitive accepts. The engine never needs to know about it.

An export without its own range that reaches an inner export with one takes that range (the narrowest of those it reaches, with fan-out), for display and for the file rule below.

### 3. A value in a file follows the primitive rule
A value outside an export's own range follows the rule of a primitive's out-of-range value. A composite instance's value inside another composite is checked against the inner export's range the same way.

As first written, that rule was a load error at the value: it came from a time when ranges were the engine's ±10⁶ bound and practically never met. **Amended, the operator's decision of 2026-10-25** (the owner's open project stopped opening once its composite's exports took ranges and value sets): a value in a file outside its range is **clamped** to it, the project loads and plays the clamped value, and a **warning** at the value says what it was and what it became (`project.toml:37:398: warning: /synth.key_track: value 132642.66 is outside [0, 2]: 2 plays (the file keeps 132642.66 until the parameter is written)`). This holds for a primitive's parameter, an export's, and a value set inside a composite file; `GraphWarning::ParameterCorrected` carries it, and the plan holds the corrected value, so a render is deterministic. An export without a range of its own is clamped to the intersection of its targets' ranges, as its control shows it, so a fanned-out value is corrected once, at the export, and every target gets the same value (what loads is what Save writes). Structural problems stay errors: an unknown key, a wrong type, a non-finite value, an export's range outside its target's.

What happens to the file (the operator's decision after the review of c89253a): the file keeps its text. A value in **`project.toml`** is "corrected on load": it does **not** make the project edited or unsaved (opening a `.signal` file with corrections sets neither `edited` nor revertible, prompts on neither open nor quit, and Revert to Saved does not loop on it); Signal.app lists the warnings beside its diagnostics and adds how many there are ("2 values will be written corrected on the next Save"; for a folder project, "when each is next written"); the next **Save** writes them along with the rest, and a commit or restore of that parameter writes it as usual. Snapshots and the undo snapshot take the corrected value, the one that plays. A value in a **composite's file** (`composites/<name>.toml`) is never written by Signal, so its warning says so (an illustration; the line and column are the value's): `composites/delay_core.toml:9:30: warning: /echo/core/top.value: value 999999 is outside [-100000, 100000]: 100000 plays (composites/delay_core.toml keeps 999999: edit it, or vendor the composite again)`.

### 4. Live edits clamp
The control's range is the export's: `describe.controls` (`min`, `max`), the Parameters tab and panel sliders, the schematic's sidebar and `graph`'s export ports (`range`), `signal modules` and the MCP tools all show it. Every live edit already clamps to the control's range (`set`, `ramp`, `commit`, a snapshot's restore, which reports `clamped`), so they now clamp to the export's. `signal mcp`'s gain guard judges a value as the server will send it, clamped to the control's range, so an export with a narrower range is judged by what it can reach.

### 5. The studio composites get ranges
The project composites of `projects/studio`, `projects/studio-transport` and `projects/two-studios` (and the frozen fixture `crates/signal-cli/tests/fixtures/studio-transport`, kept equal to them) declare ranges for their exports, from what each does and from the matching `lib.*` composite: velocities and `attack`/`snappy` [0, 2] as the library clamps them, levels in dB [−120, 12] (−120 dB is std.gain's exact silence), tunes and tones in a musical span of Hz, decays in ms, the echo's `feedback` [0, 0.75], `tone` [0, 4999] and `mix` [0, 1] as its own clamps. Every value those projects' files hold lies inside, so they load and sound as before: the seven golden audio hashes are unchanged; the manifests' `project_file_hashes` (and an included project's `files_fnv1a64`) change with the composite files, which the owner accepted.

`moog_voice`'s `key_track` was first left without a range: `projects/studio-transport/project.toml` (the owner's sound) held `key_track = 132642.66`, which `lib.ladder_voice`'s [0, 2] refuses; at that value `/synth/key_amt` exceeds ±10⁶ and was muted at frame 1 of every run, so the key tracking was silent there. The owner's decision of 2026-10-25: "Igen, 1-re, tartomány 0–2" (yes, to 1, range 0–2): `key_track` takes [0, 2] in every copy and studio-transport's value is 1, which changes how `projects/studio-transport` sounds (its key tracking plays again); no golden renders that file (the studio-transport golden is the frozen fixture, which never set `key_track`).

The built-in library was first left as it was (its composites already clamp inside, and ranges change each composite's content hash in the render manifests, `library_composite_hashes`). The owner's decision of 2026-10-25, "Igen" (yes): every library composite's exports declare ranges too, in the same way: the drums', the voice's and the echo's as the studio's, `lib.transport`'s and `lib.modal_voice`'s their targets', the validation references their native type's (up to 10⁶ where a `std.const` carries the value). Pinned outputs and every golden's audio and log are unchanged; `library_composite_hashes` change, recorded under `## 0.2` of `crates/signal-lib/CHANGELOG.md`. Every value the repository's projects and fixtures set on a library composite lies inside.

## Consequences
- A slider of an export with a range spans that range; dragging to its end cannot reach the fault guard through a generic target.
- An author can still leave a generic export unranged; its control spans ±10⁶ as before.
- The range is presentation and validation only: no instance is added, nothing is computed per frame, and renders do not change.
- Tests: `crates/signal-project/tests/export_range.rs` (both forms, refusals, fan-out, nested exports, the file rule, the studio projects), `crates/signal-ui/tests/` (describe, clamping on `set`, `commit` and a snapshot's restore), `crates/signal-cli/tests/mcp_*.rs` (the range in MCP).
