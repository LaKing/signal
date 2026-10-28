# Project format

**Status: normative** (ADR 0009). This document defines what a Signal project folder contains and how it is read. `signal-project` implements it. `signal schema` prints the matching JSON Schema.

Terms are those of `00-vision.md` §3a: Project, Module, Primitive, Composite, Instance, Port, Connection, Parameter.

## 1. The project folder

```text
<project>/
  project.toml            the project: settings, instances, wiring
  composites/<name>.toml  one composite per file (optional folder)
  patterns/<name>.toml    note and step data, one pattern per file (optional folder; §12)
  interfaces/<name>.toml  slot interface templates, one per file (optional folder; §14)
  ui/*.html               panels (optional; ui/main.html is the main one)
  ui/assets/              their SVG and PNG files
  ui/schematic.toml       the schematic's positions (optional, presentation only; §11)
  ui/windows.toml         Signal.app's windows per screen configuration (optional, presentation only; §13)
  snapshots.toml          parameter values stored under names (optional, never played; §16)
```

These are the **project files**. Signal loads every one of them, validates every one of them, and hashes every one of them into a render's manifest (§7). A recorded session keeps an exact copy of them for every plan version (`session-format.md`). Nothing else in the folder is read, except by `signal ui`, which serves the folder's `ui/` (panels and their `assets/`) to web pages as files and never plans them (`ui-protocol.md`). A panel is an HTML page whose controls bind to ports by address; `signal ui check <project>` checks every binding against the plan, and moving a control writes `project.toml`, never the panel (`ui-controls.md` "Panels"). Machine settings (Setup: devices, channel mapping) never live here.

All files are UTF-8 [TOML 1.1](https://toml.io). **Unknown keys are errors** in every table, never silently ignored.

## 2. `project.toml`

```toml
schema_version = 1

[project]
name = "example"
sample_rate = 96000   # optional, default 96000
seed = 7              # optional, default 0

[instances]
voice = { module = "voice", freq = 220, drive = 2.0 }
out = { module = "std.audio_out", gain = 0.5 }

[wiring]
connections = [
  { from = "voice.out", to = "out.l" },
  { from = "voice.out", to = "out.r", scale = 0.5 },
]
```

| Key | Type | Meaning |
|---|---|---|
| `schema_version` | integer, required | The format version. This version of Signal reads `1` and refuses any other value. |
| `[project] name` | string, required | The project's name. |
| `[project] sample_rate` | integer ≥ 1 | The master rate in frames per second of renders and replays, and the rate the project is written for. Default **96000**. `signal render --sample-rate` overrides it. A live run plays at the rate the output device runs at instead, never switching it (ADR 0036, the owner's decision of 2026-10-23: the device's clock is the run's), says so when they differ, and records the rate played in its session. |
| `[project] seed` | integer, 0 to 2⁶³−1 | The master seed of every random stream (ADR 0008). Default 0. `signal render --seed` overrides it. |
| `[instances]` | table | The project's instances (§4). |
| `[wiring] connections` | array of tables | The project's connections (§5). |
| `[exports]` | tables | ADR 0035 §2: what the project exposes when another project includes it or a slot loads it (§15), with the syntax and meaning of a composite's (§3: inputs fan out, one source per output, parameters fan out, groups). Rendered alone, a project's exports mean nothing: they are checked (each names a port that exists) and change no render. Without `outputs`, an included project exports its audio sink's channels (§15). |

## 3. `composites/<name>.toml`

A composite is a module type defined in text: instances, the connections between them, and what it exposes.

```toml
schema_version = 1

[composite]
name = "voice"

[instances]
low = { module = "std.sine_osc" }
high = { module = "std.sine_osc", freq = 441 }
shaper = { module = "std.tanh_shaper" }

[wiring]
connections = [
  { from = "low.out", to = "shaper.in" },
  { from = "high.out", to = "shaper.in", scale = 0.5 },
]

[exports]
inputs = { pm = ["low.pm", "high.pm"] }
outputs = { out = "shaper.out" }
parameters = { freq = ["low.freq", "high.freq"], drive = "shaper.drive" }
```

| Key | Type | Meaning |
|---|---|---|
| `schema_version` | integer, required | As in `project.toml`. |
| `[composite] name` | string, required | The composite's name, `[a-z][a-z0-9_]*`. It **must equal the file name** without `.toml`. |
| `[composite] category` | string, optional | The palette group (ADR 0027 §1): one of the categories the registry's primitive types carry (`core`, `generator`, `filter`, `envelope`, `delay`, `physical`, `event`, `visual`, `io`); another value is an error when the plan is built. Required in a library composite (§4). |
| `[composite] description` | string, optional | What the composite does and what each export means. Required in a library composite. |
| `[composite] inside_of` | string, optional | ADR 0037, editable insides: the slot whose **own inside** this composite is, `"<slot>"` at the top level or `"<composite>/<slot>"` in a composite's file (§14). Checked for its form only (another form is an error at the value; a library composite may not have it); it may name a slot that no longer loads it. Presentation and ownership only: the plan never reads it. |
| `[instances]`, `[wiring]` | | As in `project.toml` (§4, §5). |
| `[exports] inputs` | table: name → port or list of ports | Each exported input maps to **one inner input port or a list of them** (fan-out). Whatever is connected to the export feeds every port listed. |
| `[exports] outputs` | table: name → port | Each exported output maps to **exactly one** inner output port. A list is an error. |
| `[exports] parameters` | table: name → parameter or list of parameters | Each exported parameter maps to **one inner parameter or a list of them** (fan-out). A value set on the composite's instance, and anything connected to the export, reaches every parameter listed. Its range is its own when the table form gives one (**Ranges**, below), else the range its targets share. |

Ports are written `instance.port`, with the inner instance's local name. Export names share one namespace: a name may appear in only one of `inputs`, `outputs` and `parameters`. A list must not be empty.

**Groups** (M2.13-2, ADR 0030 §3; optional, presentation only). Any export may instead be written as a table naming the group the schematic shows it in: `{ to = <port or list>, group = "<name>" }` for an input or a parameter, `{ from = "instance.port", group = "<name>" }` for an output. The ports mean exactly what the plain form's do; no other key is allowed but an exported parameter's `min` and `max` (**Ranges**, below), and `to` (or `from`) is required. A box with many ports orders them by group, each group by the order the exports are written in the file; an export without `group` is grouped by its name's prefix (`osc1_octave` with `osc1_detune`, up to the first `_`), else stands alone (`ui-controls.md` "Schematic"). Nothing else reads it: the plan, the audio and the hashes are the same with or without it.

```toml
[exports]
inputs = { hz = { to = ["glide.in", "hz_prev.in"], group = "pitch" } }
outputs = { out = { from = "volume.out", group = "audio" } }
parameters = { glide = { to = ["glide.rise_ms", "glide.fall_ms"], group = "pitch" }, volume = "volume.db" }
```

**Ranges** (ADR 0042; optional). An exported parameter's table may give its own range, `min` and `max`, both or neither (finite, `min ≤ max`): `attack = { to = "click_level.mul", min = 0, max = 2 }`, also with `group`. It must lie inside the range of every target (a primitive parameter's declared range, or an inner composite export's range), else the plan is refused at the export; so it only narrows what the primitives accept. An export without one that reaches an inner export with one takes that range. The range is the control's (`describe.controls`, the panel, the schematic, `signal mcp`): live edits and snapshot restores clamp to it, and a value in a file outside it is clamped with a warning at the value, as for a primitive's (§6). An exported input takes no range (an error). Nothing is computed with it: the plan, the audio and the hashes are the same with or without it, as long as every value lies inside.

**Value sets** (ADR 0043). An exported parameter may instead name its allowed values: `wave = { to = "osc.shape", values = [{ value = 0, label = "sine" }, { value = 1, label = "saw" }] }` (finite, distinct, each with a label; not with `min` and `max`: its range is their span). Each value must be allowed by every target that has a value set (a primitive's, such as `std.osc`'s `shape`, or an inner export's), else the plan is refused at the export. An export without its own takes the value set its targets share. A value in a file that is not one of the set is snapped to it as its modules read it (the nearest, of two as near the greater; or, for an on/off read as `> 0`, anything positive on: ADR 0043 §2), with a warning at the value (§6); a live edit is snapped likewise. An export with its own `min`/`max` and its targets' value set keeps the values inside its range (none kept is refused at the export). The repository's and the library's composites declare a range or a value set on every export.

## 4. Instances and module references

```toml
[instances]
<name> = { module = "<ref>", gpu = "<device>", pattern = "<name>", <parameter> = <number>, … }
```

- **`<name>`** is the instance's local name, `[a-z][a-z0-9_]*`. Instance addresses follow from names: `/voice/low` is instance `low` inside the instance `voice` of the project (ADR 0016 §1).
- **`module`** is the module reference:
  - a reference containing a dot, such as **`std.sine_osc`**, resolves to a **primitive type in the registry** (`signal modules` lists them), except one starting `lib.` (below);
  - a **bare name**, such as `voice`, resolves to the **composite in `composites/voice.toml`**;
  - **`lib.<name>`**, such as `lib.tape_echo`, resolves to the **library composite** `<name>` shipped with Signal (ADR 0027 §4; `signal modules` lists them, `docs/module-contract.md` "The library"). A library composite is an ordinary composite file, read and checked by the rules of §3 and this section (its diagnostics name it `signal-lib:lib/<name>.toml`, a file of the program, not of the project folder), read-only, and not a project file: it is in no project folder or archive, and the render manifest lists the ones used with their hashes (§7). It uses `std.*` and other `lib.*` modules only, never a project composite, and takes no `gpu` pin or `pattern`; a cycle among library composites is an error when the plan is built, like any recursive composite. No primitive type may be registered under `lib.`. **A project composite does not shadow a library one:** a project composite named `lib.<name>` (in `composites/lib.<name>.toml` or by its `name`) is an error naming the library composite, and so is a direct reference to `lib.<name>` in a project that also has a composite `<name>`: the error, at the reference, names both. `<name>` and `lib.<name>` are otherwise different references (a project may own a composite `<name>` and use other library composites, including ones that use `lib.<name>` inside). `signal vendor <project> lib.<name> [--as <copy>]` copies a library composite, and the library composites it uses, into `composites/` as the project's own text: the first as `<copy>` (default `<name>`), the others under their names; in the copies and in `project.toml` and `composites/*.toml`, every reference to a copied composite is rewritten from `lib.<x>` to its new name, keeping everything else in the text. It never overwrites a file: a dependency whose file exists with exactly the copy's text (vendored before) is kept; if any other copy's file exists (the copy itself, or a dependency whose name the project uses for other text), nothing is written. Its writes are all or nothing as far as the file system allows: each file goes to a temporary name first, new files are linked into place (never replacing one), the project's rewritten files are renamed over theirs last, the project is loaded again, and on a failure the new files are removed and the originals written back. Nor does it leave a project that loaded unloadable: a copy named like another library composite the project still references directly (`--as x` beside a remaining `lib.x`) is refused with the diagnostics, and nothing is written. Instance names do not change, so the project renders bit-identically after vendoring.
- **`gpu`** (M2.7-2, optional) pins a **GPU-backed primitive instance** to a GPU device: a device's stable id or its index in the machine's pool, as a string (`gpu = "metal-apple-m5-max"`, `gpu = "1"`; `signal devices --compute` lists both; an index counts the devices the Setup's `gpu_devices` allows, sorted by id). Without it the instance is placed by the rule: round-robin over the devices sorted by id, by instance address (`docs/realtime-rules.md` "GPU"). A pin naming no device of the machine is reported and the instance is placed by the rule; a pin on an instance that uses no GPU is reported and ignored. It is an error on a composite instance (pin the instances inside it) and when empty. `gpu` is **reserved**: no composite may export a parameter named `gpu`. A running project keeps its placement: a live edit that changes a pin, or adds a pinned instance, is refused (restart the project). The render manifest records the placement.
- **`snapshot_exclude`** (ADR 0041 §8, optional, `project.toml` only) lists the instance's controls (its parameters, or a composite's or included project's exported ones) that are **left out of snapshots** (§16): `transport = { module = "lib.transport", bpm = 120, snapshot_exclude = ["run"] }`. Absent, every control goes in. Each name must be a parameter of the instance, once; in a composite's file it is an error (it would name parameters of every instance of the composite). It is not a parameter value: nothing plays differently and no render changes. Signal writes it (the Parameters tab's "snapshots" box, `snapshot.exclude`, `signal snapshot exclude`) with `toml_edit`, names in byte order, the key removed when the list empties. `snapshot_exclude` is **reserved**: no composite may export a parameter of that name.
- **`pattern`** (M2.11-1, ADR 0028 §3, optional) names the pattern file `patterns/<name>.toml` (§12) whose data the instance takes, as a string (`pattern = "melody"`). A module type declares whether it takes a pattern, and of which kind (`std.note_seq` a `notes` pattern, `std.step_seq8` a `steps` pattern); its library compiles the pattern into the instance's table when the plan is built. It is an error, at the value and naming the instance and the file, when the file does not exist, when its kind is not the one the type takes, when the type takes no pattern, on a composite instance (name the pattern inside the composite), and when the library refuses the data (a key `std.note_seq` cannot play). A type that takes a pattern may be given none: it then plays nothing. `pattern` is **reserved**: no composite may export a parameter named `pattern`. A changed pattern file is a live edit: a swap at an edit frame in which every instance survives, keeping its state (`semantics.md` §12).
- **Parameters** are every other key. The value is a number (integer or float). For a primitive, the key must be one of its declared parameters, and the value should lie in the parameter's range (and be one of its value set, if it has one): a value outside is clamped (or snapped), with a warning (§6). For a composite, the key must be one of its exported parameters, and the value should lie in the export's own range (or value set) when it declares one (§3), else it is corrected likewise with a warning. When parameters are set at several levels, the outermost instance's value wins.

An instance names **exactly one** of `module`, `project` (ADR 0035) and `slot` (ADR 0037):

```toml
[instances]
drums  = { project = "../studio-transport" }                     # an included project (§15)
bass   = { project = "parts/bass", seed = 3, bpm = 128, set = { "synth.cutoff" = 400 } }
synth  = { slot = "lib.voice", load = "moog_voice" }              # a slot (§14)
deck_b = { slot = { from = "lib.deck", inputs = { cue = "gate" } }, load = "../studio", seed = 3 }
spare  = { slot = "lib.voice" }                                   # an empty slot
```

- **`project`** is a project folder (holding `project.toml`), as a path relative to the folder of the project whose file names it (for a composite's file too: the project's folder, not `composites/`). It is read and checked by every rule of this document in its own folder, and planned as a composite instance of the whole project (§15). An absolute path is allowed, with a warning (not portable). A `.signal` file comes with ADR 0035's phase 4 and is refused for now, as is an include in a project read from files in memory (it has no folder).
- **`slot`** declares the slot's interface (§14): a template name, short for `{ from = "<name>" }`, or a table. **`load`** (optional) is its content: a value containing `/` or ending in `.signal` is a project path (as `project`; a project in a subfolder is written `./bass`), every other value a module reference as `module`'s. **`map`** (optional) overrides the match: `{ <slot port or parameter> = "<content name>" | false }`.
- **`seed`** (optional, an included project or a slot loading one): the include's seed scope (§15); the default is the included project's own `[project] seed`. On a slot that loads a module it is an error.
- **`set`** (optional): overrides of the content's own parameters outside its exports or interface: for a project, its top-level instances' parameters, keyed `"<instance>.<parameter>"`, with the rules of that instance's entry in the included `project.toml` (a key naming no top-level instance is an error at the key); for a module in a slot, that module's parameters outside the slot's interface (an unknown one is an error). The host writes there what its panels change in an included project; it never writes the included project's files.
- **Reserved keys:** `module`, `gpu`, `pattern`, `project`, `seed`, `set`, `slot`, `load` and `map`; no composite or project may export a parameter of any of these names (an error at the export). `load` and `map` are a slot's only; `seed` and `set` a project's or a slot's; `gpu` and `pattern` a module instance's (an error on a project or a slot).

## 5. Connections

```toml
[wiring]
connections = [ { from = "a.out", to = "b.in", scale = 1.0 } ]
```

| Key | Meaning |
|---|---|
| `from` | An output port, `instance.port` (for a composite instance, an exported output). |
| `to` | An input port or a parameter, `instance.port` (for a composite instance, an exported input or parameter). |
| `scale` | Optional, default 1, must be finite. It multiplies a signal source. Events are delivered unscaled (`semantics.md` §10.1). |

Several connections into one input are summed (signals) or merged (events). One output may feed many inputs. Connections into a parameter modulate it (signals) or set its base (events) (ADR 0016 §3, §4).

## 6. Validation

Loading reports every problem as a **diagnostic** with the file path and a 1-based line and column (counted in characters):

```text
project.toml:8:18: error: /osc: unknown module `std.sine` (neither a registered primitive type nor a composite)
composites/voice.toml:7:34: error: /voice/osc.typo: unknown parameter
```

- **Syntax errors, unknown keys and wrong value types** come from parsing. They point at the offending text.
- **Graph errors** come from building the plan (`signal-graph`). They are mapped back to the file:
  - unknown modules, ports and parameters, wrong-direction or mismatched connections, out-of-range values, and delay-free feedback cycles are reported at the instance or connection concerned;
  - an out-of-range value set through an exported parameter is reported at the outer instance's value; outside the export's own range (§3, "Ranges"), it is reported there too, naming the export;
  - an export's own range not inside one of its targets' ranges is reported at the export.
- **Every composite file is validated**, whether or not the project uses it:
  - a composite the project uses is checked where it is instantiated;
  - one that nothing in `project.toml` reaches is checked as if instantiated once under its own name, with no parameters set and nothing connected. Its errors are reported; its warnings are not.
- **Corrected values** (ADR 0042 §3, ADR 0043 §3, the operator's decisions of 2026-10-25): a parameter value outside its range is clamped to it (an export without its own range: to its targets' intersection), and one not in its value set is snapped to it as its modules read it (a primitive's, an export's, one set inside a composite file); the project loads and plays the corrected value, and a warning at the value names the value and what it became (`project.toml:9:42: warning: /out.gain: value 5 is outside [0, 4]: 4 plays (the file keeps 5 until the parameter is written)`). The file keeps its text. A value in `project.toml` is corrected on load: it neither edits the project nor counts as unsaved, and the next Save of a `.signal` project (where the file still holds that value) or a write of that parameter writes it; a folder project's files are not written by a Save. A value in a composite's file is never written for it: its warning ends `(composites/<name>.toml keeps 5: edit it, or vendor the composite again)`. A render records nothing more: its plan holds the corrected value.
- **A constant beyond a range** (ADR 0043 §1): a parameter whose connections are all `std.const`s nothing modulates, whose sum (scales and base included) lies outside its range, is warned of at the connection; the range clamps it, so a larger value is built on the signal path.
- **Warnings** do not stop loading: a kind mismatch on a connection (between two different kinds, neither of them `generic`: a `generic` port matches any kind, ADR 0005, note of 2026-10-18), or an instance from which no sink is reachable.
- **Included projects** (§15) are checked alone first, by every rule, in their own folders. Their diagnostics are located in their files, named from the including project's folder (`../studio-transport/composites/kick909.toml:7:3: error: …`), with an error at the including entry (`` drums: `../studio-transport` does not load (1 problem in its files) ``, or `does not plan`). Their composites read as written in messages (`kick909`, never their qualified names).
- **Slots** (§14): an interface that does not resolve, a wire on a slot port its interface does not declare, or of another shape or rate, and a content that does not match are errors located at the `slot`, wire endpoint, `load` or `map` value, naming both sides; a kind mismatch is a warning, so is a loaded slot that matches nothing and a wired slot port left unmatched beside an unmatched content port of its kind and direction ("did you mean `map = { … }`?").

A delay-free feedback cycle is an error. A loop through a state-output module such as `std.unit_delay` is fine (ADR 0016 §2).

## 7. Render provenance

`signal render` writes `manifest.json` (`semantics.md` §9). The caller-supplied fields are filled in as follows. A source that cannot be read gives `"unknown"`; it never fails the render.

| Field | Source | When unavailable |
|---|---|---|
| `signal_revision` | `git rev-parse HEAD` in the Signal workspace the binary was built from | `"unknown"` if git fails, exits non-zero or prints nothing |
| `signal_dirty` | `git status --porcelain` there: `false` for empty output (a clean tree), `true` otherwise | `"unknown"` if git fails or exits non-zero |
| `rustc_version` | `rustc -V`, run in that workspace (so the pinned toolchain answers) | `"unknown"` if it fails or prints nothing |
| `project_file_hashes` | FNV-1a 64 of each project file (§1) as loaded, by relative path | always available |
| `library_composite_hashes` | FNV-1a 64 of each library composite (§4) the plan instantiates, directly or inside another composite, by reference (`lib.<name>`), included projects' too. One that only an unused project composite references is validated but not listed | absent when there are none |
| `project_file_hashes` of includes | ADR 0035 §13: each included project's files under `include:<key>/<path>`, `<key>` the including instance (`drums`, or `<composite>.<instance>`), an include's own includes nested the same way | none without includes |
| `included_projects` | ADR 0035 §13: per including instance its `key`, the `path` as written, the project's `name`, its `seed` scope, and `files_fnv1a64`, the FNV-1a 64 of its file hashes (`<path> <hash>` lines in path order) | absent when there are none |
| `library_interface_hashes` | ADR 0037 §3: FNV-1a 64 of the text of each library interface template a slot starts from, by reference (`lib.deck`) | absent when there are none |
| `cargo_lock_hash` | FNV-1a 64 of the workspace's `Cargo.lock` | `"unknown"` if it cannot be read |

Hashes are 16 lowercase hex digits. Provenance is read when the render runs, not when the binary is built.

## 8. JSON Schema

`signal schema` prints a JSON Schema (draft 2020-12) that accepts exactly one of the five file kinds: `project.toml`, a composite, a pattern file of `kind = "notes"` or `kind = "steps"` (§12), and an interface template (§14). Module references are restricted to the registry's type names, the library composites' `lib.<name>` (§4) or a bare composite name. The schema is generated from the same model the loader uses. It cannot check what needs the registry's descriptors or other files: parameter names and ranges, ports, and whether a composite exists.

## 9. Setup (machine settings)

The **Setup** holds how this machine plays projects. It is never part of a project (ADR 0009). Signal.app (`signal-app`) reads and writes it at:

1. `--setup <file>`, when given;
2. else `$SIGNAL_SETUP` (used by tests);
3. else `~/Library/Application Support/Signal/setup.toml`.

```toml
version = 1
buffer = 128                      # frames requested from the device, or "device"
cpu_workers = "auto"              # threads per block: "auto" or a number
output_channels = [1, 2]          # device channel (from 1) of each sink channel
last_project = "/Users/me/projects/kick"

[output]                          # absent: the system's default output
name = "SSL 2+ MKII"
id = "coreaudio:AppleUSBAudioEngine:…"   # the device's stable id, when it has one
signature = "4 ch, 44100-192000 Hz"      # channels and rates when it was chosen

[input]                           # absent: no input; input is metered only
name = "SSL 2+ MKII"
id = "coreaudio:AppleUSBAudioEngine:…"
signature = "2 ch, 44100-192000 Hz"

[window]
width = 900.0
height = 640.0
```

| Key | Meaning | Default |
|---|---|---|
| `version` | the format version; another version makes the whole file default | `1` |
| `buffer` | the buffer to request, in frames, at least 1, when Signal is the output's only client; a device that runs for another app keeps its own buffer, whatever is asked (ADR 0036). `"device"`: never request one, always follow the device's current buffer (not yet offered in the interface dialog, which shows the default for it; choosing another number there replaces it) | `128` |
| `cpu_workers` | "CPU workers" in the interface dialog: the threads a block runs on, the audio thread included (`EngineConfig::threads`; 1 runs no worker threads), or `"auto"`, one per performance core (ADR 0019 §2). It changes the timing, never the output | `"auto"` |
| `output_channels` | sink channel *i* plays on device channel `output_channels[i]` (from 1). The device is opened in a configuration with at least as many channels as the highest of them. A map whose length is not the project's sink width, or that names a channel the device does not have or one channel twice, is reported and the identity map is used | the identity map |
| `last_project` | the last project opened: a folder, or a `.signal` file (§10), never its working folder; opened at start when `--project` is not given | none |
| `[output]`, `[input]` | the device: `name`, optional `id`, `signature` | the system default output; no input |
| `[window]` | the window's size in points when last closed | the toolkit's default |
| `rt_policy` | the interface setup's **wake policy** (M2.7-1; in the setup page since M2.8-4): how idle worker threads wait and are woken in real time: `"park50"`, `"budget-spin"`, `"semaphore"` or `"fanout"` (`docs/realtime-rules.md` "Wake policies"). It changes the timing, never the output. Written only when not the default | `"park50"` |
| `rt_computation` | **advanced, not in the dialog**: the worker threads' time-constraint computation, a fraction of the device period above 0 and at most 1 (`docs/realtime-rules.md` "The policy"). Written only when not the default | `0.25` |
| `gpu_devices` | **advanced, shown read-only in the dialog** (M2.7-2): the GPU adapters GPU-backed modules may use: `"all"`, or a list of adapter ids or inventory indices (`signal devices --compute`), e.g. `["metal-apple-m5-max"]`; `[]` uses none. An entry naming no adapter is reported. `signal render`, `play`, `replay` and `bench --gpu` of a `--features gpu` build read it from the same file (`--setup`, else `$SIGNAL_SETUP`, else the default path) and say when it limited the devices. Written only when not `"all"` | `"all"` |

**Finding a saved device.** Several devices can share a name (one display can list three audio endpoints of the same name), so the device picker shows such devices as `name (#k · signature)`. A saved device with an `id` is that device only. If no present device has that id, the Setup reports that the saved device is not present and asks for another to be chosen. It never takes another device of the same name for it, nor the default. Without an `id`, a unique name finds the device. If several devices share the name, only the one whose `signature` matches does. When none or more than one is left, that is reported and nothing is chosen. Playing is refused while the output cannot be found. Playing goes on without input meters while the input cannot be found. The interface dialog shows such a device as not found. Applying without choosing another keeps it saved as it is. Only an explicit choice replaces it, and that choice may be the system default. A device chosen in the dialog is kept by its identity, so refreshing the list (which may reorder it) never turns the choice into another device.

**Reading and writing.** A file that is missing is the default Setup. Unknown keys and invalid values are reported with the reason and replaced by their defaults; the rest of the file is kept. A file that is not valid TOML is reported and replaced by the default Setup. Nothing in the file can stop the app from starting. The app writes the whole file again when the interface settings are applied, a project is opened, and at exit. It writes to a hidden staging file of its own beside it (`.setup.toml.<pid>.<n>.tmp`, created new; ADR 0040), syncs that file, then renames it over `setup.toml`, so a reader sees either the old file or the new one.

**The parameter panel.** Signal.app lists the project's top-level instances with their parameters: name, unit, range, default, the base and, while playing, the effective value the runner reports (base and modulation, clamped), so a modulated parameter shows both. A composite instance shows the parameters its composite exports. Dragging a slider sends the value to the running engine as a 5 ms linear `ramp` (against zipper noise). Releasing it writes the value once into that instance's entry in `[instances]` of `project.toml`, with `toml_edit`: an existing value's text is replaced and nothing else changes (comments, ordering, spacing); a key not in the file is inserted after `module` and the parameters declared before it, so its position does not depend on the order of edits. A value is written in the shortest form that reads back to the same number, as an integer when it is one. **Reset to default** removes the key and ramps each parameter it sets back to that parameter's own value (an exported parameter that fans out may have targets with different values in the composite's files). The file is written through a hidden temporary file renamed over it. When the engine took the value, the running project takes the same edit and, when the file it read is exactly the text the running project came from, the file watch takes the written file as seen, so nothing is reloaded or applied twice (an edit to the file by another writer, before or right after the write, is still reloaded). When the Controller refused it for any port (a full command queue), the file is still written, and the running project takes, for that parameter, a value in range that no port has in the file, so the watch's reload of the written file then sends every port the file's value and playback ends at the file's value. The same holds for a refused reset. A reload that is refused itself (the queue still full) or cannot read the folder is tried again every watch interval until it succeeds, without waiting for another change to the files. Parameters of the primitives inside a composite are shown read-only: their values are in the composite's file, and writing one there would change every instance of that composite.

**Interface changes and meters.** Applying interface settings while playing stops the run and starts it again from frame 0. Device lists are refreshed only while stopped, because listing devices pauses the control thread. Meters show, per output and input channel, the peak in dBFS held for 1.5 s, the RMS over 300 ms, and a clip indicator that latches until clicked (a block peak of 1.0 or more: an output sample clamped to full scale reads exactly 1.0). While a live edit is planned and its swap prepared, the meter blocks of that time are not shown (the reload drains them into the session recorder). A peak or clip inside that gap can be missed by the meters and the clip indicator. The status line's clipped-sample count comes from the runner and counts every clamped sample.

**Keyboard shortcuts (`keys.toml`, ADR 0034).** Beside the Setup file, in the same folder (`~/Library/Application Support/Signal/keys.toml`; with `--setup` or `$SIGNAL_SETUP`, beside that file), a machine's preference and never part of a project. It holds only overrides of the built-in keymap (`ui/lib/keys.default.js`), one table per command id's first part, each command a list of chords (`[]` unbinds it):

```toml
version = 1

[view]
page_down = ["PageDown", "Space", "F7"]   # the view's Page Down: three chords
wheel_inside = ["Ctrl+Alt+Wheel"]         # the wheel modifier (built in: Alt+Wheel, ⌥)

[schematic]
fit = ["Home", "Cmd+0"]
```

A chord is `Ctrl`, `Alt`, `Shift`, `Cmd` (any order) and a key: a letter, a digit, the character typed (`+`, not `Shift+=`), a named key (`PageDown`, `Space`, `Escape`, `ArrowUp`, `F1`…) or `Wheel` for a wheel command. Signal ▸ Settings… writes it (each change at once, with `toml_edit`: comments and other entries kept; atomically, as the Setup is, and where a link points when it is a link; an override equal to the built-in chords is removed; never a file that does not read or is of another `version`); an edit made elsewhere applies within a quarter second. A missing file is the built-in keymap. An unknown command, a fixed one (the Edit menu's, text fields', menus'), a chord that does not parse or is of the wrong kind is reported with its line and column and left out; conflicts (two commands of one scope on a chord; a control's chord hiding a parent's where the built-in keymap does not; a wheel chord holding the wheel modifier) are reported at the override that made them; a file that is not TOML, or of another `version`, is reported and the built-in keys are used. Nothing in it can stop Signal from starting.

## 10. Project files (`.signal`)

A **project file** is a project folder as one file (ADR 0025): a zip archive of the project files of §1, with `project.toml` at the archive's root. It is only a container. The text files in it are the project, read and written exactly as in a folder, and nothing in the archive means anything of its own (ADR 0009, 0013). `signal pack <folder> <file.signal>` writes one, `signal unpack <file.signal> <folder>` unpacks one, and `signal render`, `signal play` and `signal ui` take a `.signal` file wherever they take a project folder: they unpack it into a temporary folder for the run and remove it at the end. A render of a `.signal` file is the render of its folder, hashes and provenance included. In `signal play --watch` and `signal ui` the edits go to that temporary copy and are not written back. Signal.app opens, saves and saves as `.signal` files (§9 and `ui-protocol.md` "Signal.app").

**What goes in.** The files a project reads, and nothing else:

- `project.toml`;
- `composites/*.toml` (directly in `composites/`);
- `patterns/*.toml` (directly in `patterns/`, §12);
- `interfaces/*.toml` (directly in `interfaces/`, §14);
- every file under `ui/` (panels and their `assets/`, at any depth);
- `snapshots.toml` (§16, ADR 0041).

**What stays out.** Everything else, in particular:

- hidden files and folders, at any depth: any name that starts with `.` (`.DS_Store`, `.git/`, an editor's temporary file);
- `renders/` and `sessions/`, and any other folder beside `composites/`, `patterns/` and `ui/`;
- any other file at the root (notes, a `.signal` file) or in `composites/` or `patterns/` (a file that is not `.toml`, a sub-folder).

A symbolic link where a project file would be is refused, not followed, and a folder without `project.toml` is not packed.

**The library version.** The archive's comment records the built-in library's version (the app's MAJOR.MINOR, ADR 0027 §4) of the build that packed it, as `signal-library <MAJOR.MINOR>`; `signal unpack --json` reports it as `library_version` (`null` for an archive without one). It is provenance only: it is no file, and an archive is never refused for it.

**Deterministic.** The same files always give the same bytes: entries are files only (no folder entries), in sorted order of their `/`-separated paths, each deflated (level 6), dated 1980-01-01 00:00:00 (the zip format's earliest time) and marked as a regular Unix file with mode `0644`. The folder's place, the files' times and modes, and the order the file system lists them in do not count.

**Reading.** Every entry is checked before anything is written, and a refused archive writes nothing (a folder made for it is removed) and is opened nowhere; the refusal names the reason and the entry:

- an entry's path must be relative and inside the project: no leading `/`, no `..`, `.` or empty component, no backslash, no NUL or other control character;
- an entry must be a regular file or a folder: no symbolic links, no device files, FIFOs or sockets (from its Unix mode), nothing encrypted;
- no two entries may name the same file, case ignored (APFS does not tell `A.toml` from `a.toml`);
- at most **10 000 entries** and **1 GiB** unpacked: the declared sizes are added up first, and the bytes actually written are counted too, so a size that lies is caught;
- `project.toml` must be a file at the archive's root;
- the folder to unpack into must not exist, or be empty. A `.signal` file larger than 1 GiB and 64 MiB is refused before it is read.

An archive made by another tool is read by the same rules. Files in it that a project never reads are unpacked, and are left out when it is saved again.

## 11. Schematic positions (`ui/schematic.toml`)

The schematic (`ui-controls.md` "Schematic", ADR 0026 §3) is laid out automatically; a box moved by hand keeps its place through this optional file. It is **presentation only**: it never changes what the project means, renders, plays or hashes (it is not a file `signal render` loads), and deleting it loses nothing but positions. Like every file under `ui/` it is packed in a `.signal` file (§10), so moving a box edits a `.signal` project.

```toml
[top]                            # the project's top level
drums = { x = 120, y = -40 }

[composite.kick]                 # the inside of composite `kick` (every instance of it)
osc = { x = 0, y = 30 }
```

One table per graph level: `[top]` for the top level, `[composite.<name>]` for a composite's inside; each key is an instance of that level, its value `{ x, y }`, integers in the schematic's units. `schematic.move` (`ui-protocol.md`) writes it with `toml_edit`, atomically, as `commit` writes `project.toml`, keeping comments and everything else; "Reset layout" removes a level's table, or the whole file. Read leniently, since it is never the project: an entry that is not `{ x = integer, y = integer }` and any other table are ignored; an instance or level that does not exist (any more) is kept in the file and in `graph`'s `layout.positions` as read, and ignored by the drawing; a file that does not parse is reported with `graph` (`layout.error`) and taken as empty.

## 12. Pattern files (`patterns/<name>.toml`)

A **pattern** is note or step data as text (ADR 0028): what a piano roll or a step grid edits. It is data, not logic, and never holds an expression (ADR 0010). Pattern files are project files (§1): every `.toml` file directly in `patterns/` is loaded and validated whether an instance names it or not, hashed into a render's manifest (§7), copied into a session for every plan version (`session-format.md` §3), packed into a `.signal` file (§10) and watched: a changed pattern is applied to a running project as a live edit. `<name>` is the file name without `.toml`, `[a-z][a-z0-9_]*`; an instance names it with `pattern = "<name>"` (§4). **Unknown keys are errors**, as everywhere.

### Notes (`kind = "notes"`)

```toml
schema_version = 1
kind = "notes"
length = 4                    # beats: the loop

[[notes]]
key = 60                      # 60 = middle C (the library's convention)
start = 0                     # beats from the pattern's start
length = 0.5                  # beats
velocity = 0.8

[[notes]]
key = 64
start = 1
length = 1
velocity = 1
```

The same pattern with the notes written inline (one array: a file uses one form or the other):

```toml
schema_version = 1
kind = "notes"
length = 4
notes = [
  { key = 60, start = 0, length = 0.5, velocity = 0.8 },
  { key = 64, start = 1, length = 1, velocity = 1 },
]
```

| Key | Type | Meaning |
|---|---|---|
| `schema_version` | integer, required | `1`. |
| `kind` | string, required | `"notes"`. |
| `length` | number, required | The loop length in beats, above 0 and at most 65536. |
| `notes` | array of tables, required | At most 16384 notes, in any order; `notes = []` is a pattern of no notes. Written inline (`notes = [ { … }, … ]`, what the UI writes, one table per line) or as `[[notes]]` tables. Each has exactly `key`, `start`, `length` and `velocity`. |
| `notes[].key` | integer, required | A TOML integer from 0 to 127 (`60.0` and `60.5` are errors). |
| `notes[].start` | number, required | Beats, finite, at least 0. A note starting at or after `length` never plays. |
| `notes[].length` | number, required | Beats, finite, above 0. A note may cross the loop end. |
| `notes[].velocity` | number, required | 0 to 1. |

`start`, `length` and `velocity` take integers or floats (`start = 1` is `1.0`). Notes are sorted by start, then key, when read. Two notes *a*, *b* on one key (*a*.start ≤ *b*.start) overlap iff `a.start + a.length > b.start`, compared exactly, with no tolerance; that is an error at the later one. Touching notes (equal) are allowed.

A player counts the loop in ticks, `T = max(1, round_half_up(length · ppq))`, and starts a note on tick `round_half_up(start · ppq)` (`std.note_seq`). A note starting just before the loop end can round to tick *T*, the loop's end: it then **wraps** to tick 0 of the next loop, where it falls in time (from the second loop on, as a wrapped step), so no note that starts before `length` is lost.

### Steps (`kind = "steps"`)

```toml
schema_version = 1
kind = "steps"
steps = 16                    # steps in the loop
step = 0.25                   # beats per step: 16ths

[lanes]                       # in output order: the first lane is t1
kick = "X...x...x...x..x"
snare = "....X.......X..."
hat = "..x...x...x...x."

[levels]                      # optional: a lane not listed has level 1
kick = 0.9
hat = 0.6
```

| Key | Type | Meaning |
|---|---|---|
| `schema_version` | integer, required | `1`. |
| `kind` | string, required | `"steps"`. |
| `steps` | integer, required | A TOML integer, 1 to 256. |
| `step` | number, required | Beats per step, above 0 and at most 64. |
| `[lanes]` | table, required | 1 to 64 lanes (`std.step_seq8` plays up to 8), in the order written, which is the output order. A lane name is `[a-z][a-z0-9_]*`, unique (TOML refuses a repeated key). A lane is a string of exactly `steps` cells, one character per step: `.` rest, `x` hit, `X` accent; any other length or character is an error. |
| `[levels]` | table, optional | `<lane> = <number>`: a lane's level, its hit value, 0 to 1 (integer or float). Every key must name a lane of `[lanes]` (an error naming the key otherwise). A lane not listed has level 1. |

The loop is `steps · step` beats. A player places step *i* on its tick, `round_half_up(i · step · ppq)` (`std.step_seq8`, with swing delaying the even steps by whole ticks). When `step · ppq` is not a whole number of ticks, a step can land on the loop's end or past it (steps that round up to the loop end, or the last step delayed by swing); it then **wraps** and plays on the next loop's first ticks, where it falls in time (from the second loop on). No step of a pattern is ever lost. A whole number of ticks per step never wraps. A player's playhead (`std.step_seq8`'s `step`) shows the loop position: on each tick, the latest step whose tick (without swing) is at or before it. A wrapped step fires without moving the playhead back, so when wrapped hits share a tick with the next loop's steps, the playhead shows the next loop's.

### The source the library compiles

The loader reads a pattern into its kind's canonical **source**, a header and rows of numbers, which the library of the type naming it compiles into that instance's table (ADR 0028 §4; `module-contract.md` "Tables"):

- `notes`: header `[length]`; one row `[key, start, length, velocity]` per note, sorted by start, then key;
- `steps`: header `[steps, step]`; one row `[level, c₀ … c₍steps−1₎]` per lane in file order (`level` from `[levels]`, 1 when not listed), each cell 0 (rest), 1 (hit) or 2 (accent).

### Editing over the UI protocol

`pattern.read` and `pattern.write` (`ui-protocol.md`) read and write these files whole, with the checks of a reload. A client message is at most 64 KiB, so a pattern editable over the protocol is one whose text, JSON-escaped, fits that with its envelope: about 63 KiB of TOML, some 1200 notes written one per line, or any steps pattern up to 64 lanes of 256 steps. Larger files load from disk and are edited as text.

## 13. Window layout (`ui/windows.toml`)

Signal.app's windows on each screen configuration (ADR 0032): which windows are open, where, and which panel cards each holds. It is **presentation only**, like `ui/schematic.toml` (§11): it never changes what the project means, renders, plays or hashes, and deleting it loses nothing but the layout. Like every file under `ui/` it is packed in a `.signal` file (§10), so moving a window edits a `.signal` project.

```toml
[[layout."Monitor #41234 3024x1964@2 +0+0 | Monitor #16711 2560x1440@1 +3024+0".windows]]
view = "main"                    # "main", "schematic" or "panel"
screen = 0                       # the screen, an index in the key's order
x = 156                          # the frame's corner from the screen's, in points
y = 97
width = 1200                     # the content's size, in points
height = 800
cards = []                       # a main window: the order of the cards it shows first

[[layout."Monitor #41234 3024x1964@2 +0+0 | Monitor #16711 2560x1440@1 +3024+0".windows]]
view = "panel"
screen = 1
x = 40
y = 80
width = 480
height = 360
cards = ["kick", "output"]       # the cards this window holds, in order
fullscreen = true                # optional: it opens full screen (x, y, width, height: its normal frame)
```

One entry per **screen configuration**, under its key: for each connected monitor, left to right then top to bottom, its name (its type), size in pixels, scale factor and top-left corner in pixels, joined by ` | `. Signal.app applies the entry of the screens connected now, at start, when the project opens and when the screens change; without one it opens the default state (one main window on the main screen, every card in it). It writes only that entry, with `toml_edit`, atomically, a second after the last change, keeping the other entries and comments; Window ▸ Reset Layout removes it. A card id is a card's `id`, else its first heading's slug (`ui-controls.md` "Cards across windows"). A panel window's cards are hidden from the main windows' Panel tab. An entry that does not read is reported and the default state is used; unknown keys in a window are errors there.

## 14. Slots and interfaces (ADR 0037)

A **slot** is an instance whose ports and parameters are **its own interface**, declared on it and editable, and whose module is whatever its `load` names, or nothing. Wiring names the slot's ports, so changing the content never rewrites the host's wires.

```toml
[instances]
fx     = { slot = { inputs = { in = "audio" }, outputs = { out = "audio" } } }
synth  = { slot = "lib.voice", load = "moog_voice", set = { cutoff = 400 } }
pad    = { slot = "lib.instrument", load = "moog_voice", map = { l = "out", r = "out" } }
blank  = { slot = {} }                                            # no ports yet

[instances.bus_in]                     # the same, as a table, for a long interface
load = "./bus"
[instances.bus_in.slot]
from       = "deck"                    # interfaces/deck.toml
remove     = ["tempo"]
inputs     = { position = "control", sync = "events" }
parameters = { volume = { unit = "dB", group = "out" } }
```

**The interface.** A table with `inputs`, `outputs` and `parameters` (one namespace; `slot = {}` is valid). With `from`, it starts from a template: the template's ports, then `remove = [names]` drops some, then each entry adds a port or replaces the template's port of that name (a retype). A name both removed and declared, a `remove` of a name the template does not have (or with no `from`), and `from` or `remove` as a port name are errors. The template is a reference: an edit of `interfaces/<name>.toml` reaches every slot naming it.

**Ports**: `shape` (`"signal"`, the default, `"events"` or `{ vector = N }`), `rate` (a divisor, default 1), `kind` (a `PortKind`: `audio`, `control`, `gate`, `trigger`, `frequency`, `gain`, `phase`, `generic`, the default), and `unit`, `range = [min, max]` and `group`, presentation only. The short form `name = "<kind>"` is a signal of that kind at rate 1, and `"events"` an event port. **Parameters** declare a name with an optional `unit` and `group`; their range and default come from the content. A value set on the slot (`volume = -6`) must name an interface parameter; it reaches the matched content parameter, and on an empty or unmatched slot it is kept and applied to nothing. An interface lists its ports by role (inputs, outputs, parameters), each in name order.

**Templates**: `interfaces/<name>.toml` in the project (project files: loaded, validated, hashed and packed like `composites/`), or the library's `lib.voice`, `lib.instrument` and `lib.deck` (`module-contract.md` "Interfaces"):

```toml
schema_version = 1
[interface]
name = "deck"                          # must be the file's name
description = "A playing song: follows a position if given one; stereo out."
[inputs]
position = { kind = "control", unit = "beats", group = "transport" }
jump     = { shape = "events", kind = "trigger", group = "transport" }
[outputs]
l = { kind = "audio" }
r = { kind = "audio" }
[parameters]
release = { group = "transport" }
```

**Wires on a slot** are checked against its interface, loaded or not: an undeclared port, a wrong direction, or another shape or rate than the other end's is an error at the wire; a kind mismatch a warning. So loading content never makes the host's wiring invalid.

**Matching** (when the plan is built; by exact name only). For each slot port or parameter *p*, the content name is `map[p]`, nothing for `false`, else *p*. The content's ports are a primitive's declared ports and parameters, a composite's or a project's exports (with the types of the inner ports behind them, `signal_graph::ports_of`), and, for a project without `[exports] outputs`, its sink's channels `l`, `r` (§15). A name found with the same direction, shape and rate is matched (another kind, neither `generic`: a warning); another direction, a port matched to a parameter, another shape or rate, a `map` key that is no slot port, a `map` value naming no content port, and two slot inputs (or parameters) onto one content input are errors. Several slot outputs may match one content output (mono to `l` and `r`).

**What the plan is.** A loaded slot is planned exactly as an ordinary instance of its content under the slot's name: each host wire on a matched slot port becomes a wire on the content's matched name, and a wire on an unmatched port is left out (the receivers read 0, `is_connected` false). The plan is byte-identical to that explicit form, so canonical order, converters, exclusive and restricted inputs, cycles and `is_connected` behave as for written wires. An **empty slot** contributes no instance and no connection: it is valid and silent. A content output not in the interface is still computed and heard nowhere. The build's side table, `Planned::slots` (per slot: its resolved interface, load, match table, the wires left out and the unheard outputs), is what `describe` lists (`content.slots`); the plan carries no record of it. Slots may appear in `project.toml` and in project composites (the same for every instance of the composite), not in library composites.

**Editable insides** (ADR 0037, 2026-10-27). A slot's **own inside** is a project composite it loads whose `[composite] inside_of` names it: `fx = { slot = { inputs = { in = "audio" }, outputs = { out = "audio" } }, load = "fx_inside" }` with `composites/fx_inside.toml` holding `name = "fx_inside"`, `inside_of = "fx"`. It is an ordinary composite (§3), planned, matched and checked as any; only the editor reads the marker. Its exports are named after the slot's ports, so the slot matches them by name with no `map`; a port with no export is unmatched until something inside is wired to it, and the wires on it are left out of the plan, as for any unmatched port. The editor makes it (`slot.inside`, `docs/ui-protocol.md` "patch") on the first edit of a slot's inside, from what the slot holds: nothing (an empty composite); a project composite or a library composite (its text copied into the new file, comments, ranges and groups kept, its exports renamed after the slot's ports through the match, the slot's `map` removed; the composite it was copied from is not changed, nor any other user of it); or a primitive (wrapped: one instance of it, named after its type's last segment, `std.gain` → `gain`, with every port and parameter exported, the matched ones under the slot's names). The name is `<slot>_inside` (`<composite>_<slot>_inside` for a slot in a composite), then `_2`, `_3`, … while a file of the project or a library composite has it. The plan does not change: byte-identical for a copied composite; a wrapped primitive plans one level down (`/fx` → `/fx/gain`: its random stream, if it draws one, is derived from that address, and live it is a new instance) and plays the same. A project as content (`./x`, `../x`) is not copied: it is edited in its own folder (ADR 0035; `signal vendor --slot`, ADR 0037 §8, is phase 4's). Nested composites inside a copy stay shared: opening one and editing it edits that composite, as for any composite instance. An interface edit of a slot holding its own inside keeps it in step in the same patch (a removed port's export removed, a renamed one's renamed in its place); Unload, Replace and deleting the slot keep the file (Load can pick it again).

**Editing** (ADR 0037 phase 3). The schematic (Add slot here…, Load…, Replace…, Unload, Edit map…, Edit interface…, the "+" pins, Save interface as template…, Detach from template, Make interface from this instance…), the slot card and `signal mcp`'s slot tools write these keys through the UI protocol's `patch` (`slot.*` ops, `docs/ui-protocol.md`), each key in place with `toml_edit` (inline or as sections), comments and every other byte kept; a renamed port's wires, `[exports]` targets, `map` key and value, and the panels' bindings on it, are rewritten with it. A template is written whole into `interfaces/<name>.toml` (`file.write`).

## 15. Projects as content (ADR 0035)

A project included as an instance (`drums = { project = "../studio-transport" }`, §4) or loaded into a slot (§14) is an ordinary composite instance of the whole project: its insides are `/drums/transport/playhead`, `/drums/kick/env` and so on.

- **Read in its own folder.** Its composites resolve in its own `composites/`, its patterns in its own `patterns/`, its interfaces in its own `interfaces/`; `lib.*` in the same library. Its definitions are planned under qualified names (`<its canonical folder>/<name>`, which no bare or `lib.` name can be), so they never collide with the host's, and two instances of one project share them.
- **Its exports are its ports** (§2). Without `[exports] outputs`, it exports its audio sink's channels, named after the sink's inputs (`l`, `r`), so any project can be included without editing it.
- **Its sinks are taps.** Inside an include, every instance of a sink type is planned as the tap its library registers for it (`module-contract.md` "Sink taps"): `std.audio_out` as `std.audio_tap`, at the same address with the same inputs, parameters and connections, whose outputs `l_out`, `r_out` are what the project records alone. The host's own `std.audio_out` stays the plan's one audio sink. A sink type without a tap makes the project refused as an include, at the instance in its file. Nothing mixes implicitly: the host wires the outputs where it wants them (a `std.mix`, an effect, its `audio_out`); an output it does not wire is computed and heard nowhere.
- **Its seed.** The include is a scope root (`semantics.md` §11) with the entry's `seed`, else the included project's own `[project] seed`: inside, every random stream is the one the project draws alone. The host's seed and `--seed` reach the host's own instances only. Two instances of one project with one seed draw the same streams.
- **Its rate.** It runs at its host's rate (offline the host's `sample_rate` or `--sample-rate`, live the device's), never resampled. Its own `sample_rate` is noted, never an error: `signal render --json` gives `notes` ("studio-transport is written for 48000 Hz and plays at 96000 Hz here"), and `describe` the same as each include's `note` (`content.includes`).
- **Cycles and depth.** A project that includes itself, directly or through other includes or slot loads, is refused at the entry that closes the cycle, naming the chain by the projects' canonical paths (symbolic links resolved: `/…/two-studios → /…/studio-transport → /…/two-studios`). The same project included twice, not in a cycle, is fine. Includes and project loads nest 8 deep at most (a guard, not a semantic limit).
- **What is bit-identical** (ADR 0035 §4): included, run at its `sample_rate` with no `set` and none of its exports driven, every output, event and parameter value inside is the project's alone, on any partition and thread count; its tap's outputs, converted to f32, are its own `audio.wav` byte for byte (`crates/signal-cli/tests/two_studios.rs`).
- **Exports of a tap output.** `[exports] outputs` may name a sink's tap output (`l = "out.l_out"`): alone it is checked against the tap and means nothing, included it is the tap's output.
- **Live edits.** A host with includes takes live edits of its own files; an edit that changes an include's `seed` (so the random streams of instances that would survive) is refused with "restart the project", as a changed `[project] seed` is.
- **Packing and sessions, until phase 4.** `signal pack` refuses a project with includes (naming them) rather than writing an archive that does not load; a session recorded from one records the includes in its provenance (`included_projects`, the include files' hashes) and is refused by `signal replay` with the reason.
- Not yet (ADR 0035's later phases): watching an include's files live and joining or leaving live (phase 2), rooted include panels and the `set` overrides written from them (phase 3), `.signal` includes, `pack --embed`, sessions with includes and `vendor --include` (phase 4). A project with includes plays and renders from its folder; edits made in memory (Signal.app's live edits) refuse an include until phase 2.

## 16. Snapshots (`snapshots.toml`, ADR 0041)

A **snapshot** (`00-vision.md` §3a: a Preset or Snapshot) is the value of every control of the project stored under a name a person gives, so that it can be restored later. The optional file `snapshots.toml` at the project's root holds them; a missing file means no snapshots. It is **never played**: `signal render`, `signal play` and the plan do not read it, so it changes no render and no hash, but it is the project's own text and a `.signal` file packs it (§10).

```toml
# Snapshots: parameter values under a name (docs/project-format.md §16).
# Signal writes this file whole; comments are not kept.
schema_version = 1
transition_ms = 500
order = ["chorus", "Verse A"]

[undo]
"/drums.kick_decay" = 420
"/out.gain" = 0.5

[snapshots."Verse A"]
"/drums.kick_decay" = 300
"/out.gain" = 0.25

[snapshots.chorus]
"/drums.kick_decay" = 450
"/out.gain" = 0.4
```

| Key | Meaning |
|---|---|
| `schema_version` | `1`; required once the file has a snapshot |
| `snapshots."<name>"` | One snapshot. The name is any text, 1 to 64 characters (Unicode scalar values) with no space around it, no control or format character (category Cf), no U+2028 or U+2029, and not only blank characters; it is a TOML key, quoted when it needs it. Names are exact and not normalised (`Chorus` ≠ `chorus`; a composed and a decomposed `é` are two names) |
| `transition_ms` | The snapshot transition time (optional, ADR 0041 §10): a whole number of milliseconds from 0 to 60000. A restore or undo in Signal moves each continuous control to its value over it (a stepped one switches at the start); 0 or absent switches at once. Written only when not 0 |
| `order` | The list's order (optional, ADR 0041 §9): an array of snapshot names. The names it lists come first, in its order, then every other snapshot in byte order. Signal writes it only when the order is not byte order, with every name; a name without a snapshot is ignored and dropped on the next write; a name twice is an error |
| `undo` | The **undo snapshot** (optional): the values the last restore (or undo) replaced, a table of values as a snapshot's. It is apart from the named snapshots, so no name is reserved and none can collide with it |
| `"/<instance>.<parameter>" = <number>` | One value: the parameter's full address, as the UI protocol and `signal mcp` name a control, and a finite number (an integer or a float) |

Unknown keys, a bad name, an address that is not `/<instance>.<parameter>` and a value that is not a finite number are errors; a file with an error is refused by every snapshot write and never overwritten. A `snapshots.toml` that is a symbolic link is neither read nor written.

- **What a snapshot holds.** Every **control** of the project: each top-level instance's own parameters (for a composite or an included project, its exported ones, §3, §15), the parameters an edit writes into `project.toml` (§9). A parameter inside a composite is in the composite's file, shared by its instances, and is in no snapshot.
- **Save** records each control's value as it plays (in Signal.app and `signal ui`: its live base, an unsaved edit included; with `signal snapshot save`, the files' value), defaults included, and a modulated control's base, not its modulated signal. A name in use is replaced only when asked.
- **Restore** writes each value that differs from the file into the control's entry in `project.toml` exactly where a parameter edit writes it (§9, "The parameter panel"; a value out of the control's range clamped, as an edit clamps), all of them in one write, and applies them live as a parameter edit is applied. An entry whose instance or parameter no longer exists is skipped and reported; controls the snapshot does not name are left alone. In a project that includes others only the host's `project.toml` is written, never an included project's files.
- **Canonical text.** Signal writes the whole file: the header, `schema_version`, then `transition_ms` (only when not 0), then `order` (only when the list's order is not byte order, with every name), then `[undo]` when there is one, then the snapshots in byte order of their names, each one's values in byte order of their addresses, each value in the shortest form that reads back to the same `f64` (an integer when it is one). The same snapshots give the same bytes. No time or date is written. Deleting the last snapshot leaves `schema_version` alone in the file.
- **Left out of snapshots.** A control named in its instance's `snapshot_exclude` (§4) is neither saved nor restored: a snapshot that still holds its value (saved before it was left out) leaves it as it is, and the restore reports it skipped (`excluded`). The undo snapshot holds no value of it either.
- **Undo.** Before every restore Signal writes the values playing now of the controls the restore can change (the snapshot's entries that exist and go into snapshots) as `[undo]`, in the write that reads the snapshot, under the same writer lock as the `project.toml` write that follows; a restore that cannot write it does not happen, a restore that changes nothing leaves it as it was, and if the `project.toml` write fails the new undo snapshot is kept when a value of the restore reached the engine (it holds what played before) and put back to the one from before when none did (ADR 0041 §7). Undo restores `[undo]` the same way, after the values playing now take its place, so a second undo redoes. One level: each restore replaces it.
- **Order.** A new snapshot goes to the end of the list, an overwrite keeps its place, a delete takes it out, and a move (`snapshot.move`, a drag in the Snapshots card, Alt+ArrowUp/Down on a row) puts it at a place (past the end: the last). The undo snapshot is in no order.
- **Writers.** Every write is under the project's writer lock and compared just before its rename (ADR 0040); a change by another writer meanwhile is refused (`conflict`), never lost.
