# The built-in library: changelog

The library composites (`lib.<name>`, ADR 0027 §4) shipped with Signal. A
project that uses one sounds the same in every build of one library version;
this file says what changed between versions.

## Versioning

- **The library's version is the app's MAJOR.MINOR** (`signal_lib::VERSION`,
  the workspace version in the root `Cargo.toml`). A `.signal` archive and a
  session record it; a render manifest lists every library composite it used
  with its content hash (`library_composite_hashes`).
- **A change of a composite's output is a new name, or a MINOR change with an
  entry here.** The output is pinned: `tests/pinned.rs` renders every
  composite on a fixed stimulus and compares the hash with
  `tests/pinned.toml`. A changed hash fails the test until its row is updated
  **and** an entry under the current version's section (`## MAJOR.MINOR`)
  names the composite (as `` `lib.<name>` ``) with the new hash; an unchanged
  hash may stay named by the section that introduced it. A change that keeps
  the output (comments, the description) needs neither.
- **Earlier sections are history.** When a new MINOR starts, the section of
  the version it ends is recorded in `tests/released.toml` with its hash, and
  the test fails if it is edited or removed afterwards.
- **Names are never reused.** A removed or renamed composite's name is never
  given to a different composite.
- **A removal warns for one MINOR first.** A composite to be removed stays,
  marked deprecated in its description and here, for one MINOR version
  before it goes.

A project that wants a composite never to change copies it into its own
`composites/` with `signal vendor <project> lib.<name>`; the copy is the
project's text.

## 0.2

The library of Signal 0.2.0, the first public release. No composite changed
its output or was removed since 0.1: every entry under `## 0.1` still holds.
Added after the v0.2.0 tag:

- `lib.transport` (event), ADR 0033: the transport as a signal, a `std.playhead` with `bpm`, `run`, `rate` and `seek` exported and `position`, `running` and `tempo` out, for sequencers to follow; not in the library demo (`NOT_IN_DEMO`): `7f20f2bbed23372b`
- `lib.voice` (interface template, ADR 0037 §3): a mono voice, inputs `hz` (frequency, Hz), `gate` (gate), `velocity` (control, 0 to 1), output `out` (audio); text hash `2dc66859dd2f5871`
- `lib.instrument` (interface template, ADR 0037 §3): `lib.voice`'s inputs, outputs `l` and `r` (audio); text hash `07b3870f5381b455`
- `lib.deck` (interface template, ADR 0037 §3): inputs `position` (control, beats), `jump` (events, trigger), `running` (gate), `tempo` (control, bpm), outputs `l` and `r` (audio), parameter `release`; text hash `c8fa72b11d0b70d4`

Export ranges (ADR 0042, the owner's decision of 2026-10-25): every library
composite's exported parameters declare their own range (`min` and `max` in
its `[exports.parameters]`), inside each target's range: the drums' levels
−120…12 dB and velocities 0…2 as their clamps, the voices' and the echo's
controls as `projects/studio`'s, the validation references their native
type's range (the filters' cutoff and the oscillator's frequency up to
100 000 Hz, ADR 0043), `lib.modal_voice` its targets'. `signal modules`, the
panel and `signal mcp` show them; a value outside one is clamped, in a file
with a warning at the value (the operator's decision of 2026-10-25), and
live. No composite's output changed (every pinned hash above holds); each
composite's content hash in a render manifest (`library_composite_hashes`)
did.

Ranges and value sets (ADR 0043, the owner's "MINDEN kapjon értelmezési
tartományt vagy értékkészletet", 2026-10-25): the discrete exports declare a
value set instead of a range (`lib.ladder_voice`'s oscillator waves and
octaves, `lib.lfo_shapes`' shape, `lib.modal_voice`'s exciter kind,
`lib.transport`'s run). Every constant is within the generic ±10⁵ now:
`lib.adsr_core` and `lib.delay_core` make 999 999 as 27 027 × 37 (exact),
`lib.onepole_core` and `lib.svf_core` cap the cutoff at 100 000 Hz, their
range's end and the native's (before, 318 309 Hz, which acted only above
649 610 Hz), `lib.osc_core`'s frequency ends at 100 000 Hz and
`lib.modal_voice`'s pitch clamp at 100 000 Hz. No composite's output
changed (every pinned hash above holds); content hashes did. A project's
vendored copy of the old `adsr_core` or `delay_core` (`top = std.const
999999`, before the 27 027 × 37 replacement) is not replaced: its constant
is clamped to 10⁵ with a warning at that file's value (edit it, or vendor the
composite again), so a 5 s delay there arrives at 100 000 frames (2.08 s at
48 kHz), and an envelope's long stages likewise.

## 0.1

The first library (M2.10-3). Each line: the composite, its category, and its
pinned render hash (`tests/pinned.rs`: its stimulus long enough for its
whole signal path; `lfo_shapes` once per shape, with every output).

- `lib.analog_clap` (generator), from the M2.11 studio's clap: `998f4706c835f181`
- `lib.analog_cymbal` (generator), from the M2.11 studio's cymbal: `8aa09a97621232e1`
- `lib.analog_hat` (generator), from the M2.11 studio's hi-hat: `7e038d7e1126fb35`
- `lib.analog_kick` (generator), from the M2.11 studio's kick, its click level clamped and its default level −3 dB: `339b880fcac1f285`
- `lib.analog_rim` (generator), from the M2.11 studio's rim shot, its default level 1 dB: `4d858832caea2739`
- `lib.analog_snare` (generator), from the M2.11 studio's snare, its snappy level clamped and its default level −4 dB: `07993fd454049105`
- `lib.analog_tom` (generator), from the M2.11 studio's tom, its default level −1 dB: `be2e4e676af0efa1`
- `lib.ar_env` (envelope), an attack-release envelope from a lag: `9fccd4e2baaad1fd`
- `lib.crossfade` (core), a linear crossfade, each input weighted before the sum (no intermediate beyond the larger input): `85b54d5bd8141c4d`
- `lib.ladder_voice` (generator), from the M2.11 studio's three-oscillator voice, its key tracking and velocity clamped: `b10bcbec2ba1d575`
- `lib.lfo_shapes` (generator), five LFO shapes from one phasor, the random drawn at every cycle's start (frame 0 and every reset included): `3cdbb0eb89ca6716`
- `lib.stereo_width` (core), mid/side width, computed as the change from the input from halves taken first (no intermediate beyond the inputs or outputs; width 1 exact): `4398c461e58ecc48`
- `lib.tape_echo` (delay), from the M2.11 studio's tape delay: `2229c4b8e2b245b9`
- `lib.modal_voice` (generator), added in M2.10-8: `std.exciter` into a fully modulatable `std.modal` with a loudness contour and velocity to level and brightness, every control and the bank's per-mode vector inputs exported; not in the library demo (`NOT_IN_DEMO`): `9ce7810f4cfa9c81`

The validation references (M2.10-4, ADR 0027 §4 "Validation"): std types
rebuilt from core primitives, each rendered against its native counterpart
by `tests/validation.rs` (`signal_lib::VALIDATION`; the library demo leaves
them out, and `signal modules` names the native type as `reference_for`). Each line: the composite, its category, how it compares, and its
pinned render hash.

- `lib.adsr_core` (envelope), `std.adsr` as a stage machine of signals, within 1e-9: `79558ea641c77ef5`
- `lib.delay_core` (delay), `std.delay` on a `std.buffer1m`, bit-exact at whole-frame delays, within 1e-9 of the input's peak at fractional ones; direct, so a loop through it needs a `std.unit_delay`: `07fd4ab98b0d06a1`
- `lib.lag_core` (filter), `std.lag`, within 1e-9: `d29413a0768c6695`
- `lib.onepole_core` (filter), `std.onepole`, bit-exact up to 649 610 Hz: `4e558aa8ced1a6e1`
- `lib.osc_core` (generator), `std.osc`'s sine, bit-exact, and a naive saw (no polyBLEP): `5543b2e26d5c32f5`
- `lib.svf_core` (filter), `std.svf`, bit-exact up to 649 610 Hz: `940cf89f0188e264`
- `lib.tanh_shaper_core` (filter), `std.tanh_shaper` without ADAA (the midpoint form), within 1e-9 + 0.035·Δu² per frame, a meaningful comparison at low drive only: `73ac862501564041`

Every drum's velocity is clamped to [0, 2]. The defaults keep each composite
within ±1 on its stimulus (`tests/library.rs`).
