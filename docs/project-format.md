# Project format

**Status: normative** (ADR 0009). This document defines what a Signal project folder contains and how it is read. `signal-project` implements it. `signal schema` prints the matching JSON Schema.

Terms are those of `00-vision.md` §3a: Project, Module, Primitive, Composite, Instance, Port, Connection, Parameter.

## 1. The project folder

```text
<project>/
  project.toml            the project: settings, instances, wiring
  composites/<name>.toml  one composite per file (optional folder)
```

These are the **project files**. Signal loads every one of them, validates every one of them, and hashes every one of them into a render's manifest (§7). A recorded session keeps an exact copy of them for every plan version (`session-format.md`). Nothing else in the folder is read. Machine settings (Setup: devices, channel mapping) never live here.

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
| `[project] sample_rate` | integer ≥ 1 | The master rate in frames per second. Default **96000**. `signal render --sample-rate` overrides it. |
| `[project] seed` | integer, 0 to 2⁶³−1 | The master seed of every random stream (ADR 0008). Default 0. `signal render --seed` overrides it. |
| `[instances]` | table | The project's instances (§4). |
| `[wiring] connections` | array of tables | The project's connections (§5). |

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
| `[instances]`, `[wiring]` | | As in `project.toml` (§4, §5). |
| `[exports] inputs` | table: name → port or list of ports | Each exported input maps to **one inner input port or a list of them** (fan-out). Whatever is connected to the export feeds every port listed. |
| `[exports] outputs` | table: name → port | Each exported output maps to **exactly one** inner output port. A list is an error. |
| `[exports] parameters` | table: name → parameter or list of parameters | Each exported parameter maps to **one inner parameter or a list of them** (fan-out). A value set on the composite's instance, and anything connected to the export, reaches every parameter listed. |

Ports are written `instance.port`, with the inner instance's local name. Export names share one namespace: a name may appear in only one of `inputs`, `outputs` and `parameters`. A list must not be empty.

## 4. Instances and module references

```toml
[instances]
<name> = { module = "<ref>", <parameter> = <number>, … }
```

- **`<name>`** is the instance's local name, `[a-z][a-z0-9_]*`. Instance addresses follow from names: `/voice/low` is instance `low` inside the instance `voice` of the project (ADR 0016 §1).
- **`module`** is the module reference:
  - a reference containing a dot, such as **`std.sine_osc`**, resolves to a **primitive type in the registry** (`signal modules` lists them);
  - a **bare name**, such as `voice`, resolves to the **composite in `composites/voice.toml`**.
- **Parameters** are every other key. The value is a number (integer or float). For a primitive, the key must be one of its declared parameters, and the value must lie in the parameter's range. For a composite, the key must be one of its exported parameters. When parameters are set at several levels, the outermost instance's value wins.

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
  - an out-of-range value set through an exported parameter is reported at the outer instance's value.
- **Every composite file is validated**, whether or not the project uses it:
  - a composite the project uses is checked where it is instantiated;
  - one that nothing in `project.toml` reaches is checked as if instantiated once under its own name, with no parameters set and nothing connected. Its errors are reported; its warnings are not.
- **Warnings** do not stop loading: a kind mismatch on a connection, or an instance from which no sink is reachable.

A delay-free feedback cycle is an error. A loop through a state-output module such as `std.unit_delay` is fine (ADR 0016 §2).

## 7. Render provenance

`signal render` writes `manifest.json` (`semantics.md` §9). The caller-supplied fields are filled in as follows. A source that cannot be read gives `"unknown"`; it never fails the render.

| Field | Source | When unavailable |
|---|---|---|
| `signal_revision` | `git rev-parse HEAD` in the Signal workspace the binary was built from | `"unknown"` if git fails, exits non-zero or prints nothing |
| `signal_dirty` | `git status --porcelain` there: `false` for empty output (a clean tree), `true` otherwise | `"unknown"` if git fails or exits non-zero |
| `rustc_version` | `rustc -V`, run in that workspace (so the pinned toolchain answers) | `"unknown"` if it fails or prints nothing |
| `project_file_hashes` | FNV-1a 64 of each project file (§1) as loaded, by relative path | always available |
| `cargo_lock_hash` | FNV-1a 64 of the workspace's `Cargo.lock` | `"unknown"` if it cannot be read |

Hashes are 16 lowercase hex digits. Provenance is read when the render runs, not when the binary is built.

## 8. JSON Schema

`signal schema` prints a JSON Schema (draft 2020-12) that accepts exactly one of the two file kinds. Module references are restricted to the registry's type names or a bare composite name. The schema is generated from the same model the loader uses. It cannot check what needs the registry's descriptors or other files: parameter names and ranges, ports, and whether a composite exists.

## 9. Setup (machine settings)

The **Setup** holds how this machine plays projects. It is never part of a project (ADR 0009). Signal.app (`signal-app`) reads and writes it at:

1. `--setup <file>`, when given;
2. else `$SIGNAL_SETUP` (used by tests);
3. else `~/Library/Application Support/Signal/setup.toml`.

```toml
version = 1
buffer = 128                      # frames requested from the device
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
| `buffer` | the buffer to request, in frames, at least 1 | `128` |
| `output_channels` | sink channel *i* plays on device channel `output_channels[i]` (from 1). The device is opened in a configuration with at least as many channels as the highest of them. A map whose length is not the project's sink width, or that names a channel the device does not have or one channel twice, is reported and the identity map is used | the identity map |
| `last_project` | the last project folder opened; opened at start when `--project` is not given | none |
| `[output]`, `[input]` | the device: `name`, optional `id`, `signature` | the system default output; no input |
| `[window]` | the window's size in points when last closed | the toolkit's default |

**Finding a saved device.** Several devices can share a name (one display can list three audio endpoints of the same name), so the device picker shows such devices as `name (#k · signature)`. A saved device with an `id` is that device only. If no present device has that id, the Setup reports that the saved device is not present and asks for another to be chosen. It never takes another device of the same name for it, nor the default. Without an `id`, a unique name finds the device. If several devices share the name, only the one whose `signature` matches does. When none or more than one is left, that is reported and nothing is chosen. Playing is refused while the output cannot be found. Playing goes on without input meters while the input cannot be found. The interface dialog shows such a device as not found. Applying without choosing another keeps it saved as it is. Only an explicit choice replaces it, and that choice may be the system default. A device chosen in the dialog is kept by its identity, so refreshing the list (which may reorder it) never turns the choice into another device.

**Reading and writing.** A file that is missing is the default Setup. Unknown keys and invalid values are reported with the reason and replaced by their defaults; the rest of the file is kept. A file that is not valid TOML is reported and replaced by the default Setup. Nothing in the file can stop the app from starting. The app writes the whole file again when the interface settings are applied, a project is opened, and at exit. It writes to `setup.toml.tmp` beside it, syncs that file, then renames it over `setup.toml`, so a reader sees either the old file or the new one.

**Interface changes and meters.** Applying interface settings while playing stops the run and starts it again from frame 0. Device lists are refreshed only while stopped, because listing devices pauses the control thread. Meters show, per output and input channel, the peak in dBFS held for 1.5 s, the RMS over 300 ms, and a clip indicator that latches until clicked (a block peak of 1.0 or more: an output sample clamped to full scale reads exactly 1.0). While a live edit is planned and its swap prepared, the meter blocks of that time are not shown (the reload drains them into the session recorder). A peak or clip inside that gap can be missed by the meters and the clip indicator. The status line's clipped-sample count comes from the runner and counts every clamped sample.
