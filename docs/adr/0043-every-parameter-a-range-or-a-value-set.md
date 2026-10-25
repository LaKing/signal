# ADR 0043: Every parameter a range or a value set

- **Status:** Accepted (operator, 2026-10-25, from the owner's direction). The owner's words of 2026-10-25: "MINDEN kapjon értelmezési tartományt vagy értékkészletet" (translated: "EVERYTHING gets a range (domain) or a value set"). The design is the operator's, delegated.
- **Builds on:** ADR 0042 (composite exports with their own range), ADR 0027 §1 (the core set), ADR 0029 (`signal mcp`), ADR 0041 (snapshots). It extends ADR 0042 to every primitive parameter and adds value sets. It supersedes nothing; it replaces the stepped heuristic of ADR 0041 §10: `panel::stepped` is true exactly for a parameter with a value set, so a snapshot transition switches those and ramps every other.
- **Date:** 2026-10-25

## Context
ADR 0042 gave composite exports their own range. Underneath, many primitive parameters still declared the engine's magnitude bound as their range: `std.const`'s `value`, `std.scale`'s `mul` and `add`, `std.clamp`'s bounds, `std.random`'s, a table's elements, a step's value, a counter's step, `std.edge`'s threshold (±10⁶), and the frequencies and cutoffs declared as the static bound for any 32-bit sample rate (`[0, (2³² − 1)/2]`, `[1, 0.49·(2³² − 1)]`, about 2·10⁹). A slider over such a range is useless, and its ends meet the fault guard. Discrete parameters (a filter's mode, a waveform, an interpolation, a gate's on/off) were numbers with a range, rounded in `process`, which a panel could only guess were steps.

## Decision

### 1. Every primitive parameter has a meaningful range
- **Generic values** take ±10⁵ (`signal_std::GENERIC`): `std.const` `value`, `std.scale` `mul`/`add`, `std.clamp` `lo`/`hi`, `std.random` `lo`/`hi`, `std.edge` `threshold` (and `hysteresis` 0…10⁵), `std.to_events` `threshold` 0…10⁵, `std.counter` `step`, `std.step_seq` `s1…s8`, `std.table<N>` `v0…`, `std.integrator` `gain` 0…10⁵. That is wide enough for every use in the repository (Hz, ms, gains, frame counts up to 10⁵) and well inside the ±10⁶ guard. A larger value is made with a connection's scale, which no range limits (the bound tests do so).
- **Frequencies** of the oscillators (`std.osc`, `std.sine_osc`, `std.fm_op`, `std.modal`, `std.string`, `std.wavetable`, `std.phasor`) end at 100 kHz (`sr/2` at 200 kHz), the **cutoffs** (`std.svf`, `std.onepole`, `std.biquad`, `std.ladder`, `std.comb`'s and `std.fdn_reverb`'s damping) and `std.mass_spring`/`std.mesh2d`'s `spring_hz` at 100 kHz. `process` still limits each to its rate-dependent bound (`sr/2`, `0.49·sr`, the stability bound), so nothing changes at the rates projects use; above 200 kHz the declared end is lower than that bound. `dsp-conventions.md` "Ranges" says so.
- `std.buffer1m`'s `size` ends at 10⁶ frames (its capacity is 2²⁰), within the bound; its default follows.
- The rest kept their ranges: they were already meaningful. `std.counter`'s `modulo` stays [1, 10⁶] (the count's own bound, not a generic value).
- A test over the whole registry (`crates/signal-std/tests/ranges.rs`): no parameter has the ±10⁶ range, every range is finite and within ±10⁶, every default inside.

### 2. Value sets for discrete parameters
A parameter descriptor may carry a **value set**: a list of `(value, label)` (`signal_core::ValueLabel`), generic, no domain concept. A parameter with one is **stepped**; every other is continuous. Its range spans the values. The std types:

| Parameter | Values |
|---|---|
| `std.biquad` `mode` | 0 low-pass, 1 high-pass, 2 band-pass, 3 notch, 4 peak, 5 low shelf, 6 high shelf, 7 all-pass |
| `std.osc` `shape` | 0 sine, 1 saw, 2 square, 3 triangle |
| `std.exciter` `kind` | 0 pluck, 1 strike, 2 noise burst, 3 bow |
| `std.fm_op` `mode` | 0 ratio, 1 fixed |
| `std.compare` `mode` | 0 lt, 1 le, 2 eq, 3 ge, 4 gt, 5 ne |
| `std.mass_spring` `boundary` | 0 fixed–fixed, 1 fixed–free |
| `std.integrator` `mode` | 0 cutoff, 1 gain |
| `std.lfo` `shape` | 0 sine |
| `std.buffer*` and `std.table<N>` `interp` | 0 none, 1 linear, 2 Hermite |
| `std.table<N>` `wrap` | 0 clamp, 1 wrap |
| `std.to_events` `on_change` | 0 off, 1 on |
| `std.playhead` `run` | 0 stopped, 1 running |

Integer counts (`std.clock_div` `divide`, `std.modal` `modes`, `std.mass_spring` `masses` and nodes, `ppq`, a sequence's `length`, `accent_every`, a buffer's `size`) keep a range: they are counts, not choices, and their ranges run to 64, 256 or 960 values. `std.modal` `material` and `law` and `std.wavetable` `position` morph continuously and keep a range.

`describe` gives a control's `values` (`[{value, label}]`), `graph` and `signal modules` a parameter's; `signal mcp` passes `describe` on. A value-set parameter is a **selector** in the Parameters tab and the schematic's sidebar, not a slider.

### 3. Values outside the set
The rule of out-of-range values, as amended by the operator's decision of 2026-10-25 (ADR 0042 §3; the owner's open project, whose composite's waves held 1.0023 and 2.0102, stopped opening under the first rule, a load error): a value in a file that is not one of the set takes the **nearest** of them (the lower of two as near), the project loads and plays it, and a **warning** at the value says so (`project.toml:37:398: warning: /synth.osc1_wave: value 1.002265625 is not one of 0 sine, 1 saw, 2 square, 3 triangle: 1 plays (the file keeps 1.002265625 until the parameter is written)`); an out-of-range value is clamped likewise. The file keeps its text until that parameter is written (a commit, a restore, Save: Signal.app counts it as unsaved and shows the warnings with its diagnostics and in the toolbar). Structural problems stay errors (an export's value set not allowed by its target, a range outside its target's, an unknown key, a bad type, a non-finite value). A live edit (`set`, `ramp`, `commit`, a snapshot's restore, an MCP tool) takes the nearest value (the lower of two as near), and a restore reports it in `clamped`. A connected signal still modulates a value-set parameter; `process` rounds the effective value as before.

### 4. Composite exports
An export may declare a value set instead of a range: `wave = { to = "osc.shape", values = [{ value = 0, label = "sine" }, …] }` (not with `min`/`max`; its range is the values' span). Each value must be allowed by every target that has a value set (a primitive's, or an inner export's), else `ExportValueOutside` at the export. An export without its own takes the value set its targets share. Every export of the repository's project composites, the frozen studio-transport fixture and the built-in library declares a range or a value set itself (`crates/signal-project/tests/export_range.rs`); a user's composite may leave it to its target, which is always meaningful now.

### 5. What changed in files
The library's validation references keep their bit-exactness: `adsr_core` and `delay_core` make 999 999 as 27 027 × 37 (two constants within ±10⁵, an exact product), `onepole_core` and `svf_core` cap the cutoff at 100 000 Hz (their range's end, the native's), `osc_core`'s frequency and `modal_voice`'s pitch clamp end at 100 000 Hz. The voices' oscillator waves and octaves, `lfo_shapes`' shape, `modal_voice`'s exciter kind and `lib.transport`'s run declare value sets; the kick project's composites declare ranges. No golden's audio changed; manifests' file and library hashes did, which the owner accepted.

## Consequences
- No slider spans ±10⁶ or 2·10⁹; a discrete parameter reads as names.
- A project written with a generic value beyond ±10⁵ (a constant of 10⁶) or a value between a set's values still loads, with that value clamped or snapped and a warning; to keep a larger constant, it goes through a connection's scale. None in the repository did (the tests that did now scale).
- A frequency or cutoff above 100 kHz is no longer declarable; at rates above 200 kHz that is below `sr/2`.
- Tests: `crates/signal-std/tests/ranges.rs`, `crates/signal-project/tests/export_range.rs`, `crates/signal-ui/tests/export_range.rs`, `crates/signal-cli/tests/mcp.rs`, `ui/lib/value-set.test.js`.
