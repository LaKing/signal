# Project format

**Status: normative** (ADR 0009). This document defines what a Signal project folder contains and how it is read. `signal-project` implements it. `signal schema` prints the matching JSON Schema.

Terms are those of `00-vision.md` §3a: Project, Module, Primitive, Composite, Instance, Port, Connection, Parameter.

## 1. The project folder

```text
<project>/
  project.toml            the project: settings, instances, wiring
  composites/<name>.toml  one composite per file (optional folder)
```

These are the **project files**. Signal loads every one of them, validates every one of them, and hashes every one of them into a render's manifest (§7). Nothing else in the folder is read. Machine settings (Setup: devices, channel mapping) never live here.

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
