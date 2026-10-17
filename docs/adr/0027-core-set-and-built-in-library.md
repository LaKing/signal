# ADR 0027: The core set, the built-in library, and where algorithms stay native

- **Status:** Accepted (owner's request of 2026-10-17: "before we start, we should produce all the completely fundamental building elements from which these and more complex structures can be built… NI Reaktor's pattern: in Primary you can already build a lot, but still there came the core cell… a foundation, a built-in set in the application, from which everything can be built: hardware emulations, physical-world simulations, signal conversions, signal processing"; and the same day: "these core-cell-like things can be in Rust; I don't insist that they be visually editable; it's enough if a core cell module is a Rust source text", then "ok, let it be so"; design by the operator, delegated). Refines ADR 0001 (primitives and composites) and ADR 0005 (ports). Consistent with ADR 0010; supersedes nothing.
- **Date:** 2026-10-17

## Context
The owner's list (LFO shapes, envelopes, wavetable and FM oscillators, filters, effects, spectrum displays, new controls) needs a foundation first: a complete set of fundamental elements, built into the application, from which every module can be built. Reaktor's model is Primary modules over core cells, whose insides are atoms compiled into one piece of code. Signal's equivalent of an atom is a Rust primitive (ADR 0010); the owner does not require editing core cells visually.

Measured (operator, 2026-10-17, one Apple Silicon performance core, release build, `signal render` and `signal bench`; scripts and raw output in the M2.10 issue record):
- A primitive instance costs about **2.5 ns per frame** in block execution (chains of 100 to 4000 `std.add`, 48 and 96 kHz), and about **22 ns per frame** inside a one-sample feedback loop, whose members run at k = 1 (`docs/semantics.md` §13, `crates/signal-engine/src/units.rs`). One 1000-instance cycle on all cores measured 41 ns, slower than on one thread (unexplained).
- A fixed-coefficient TPT state-variable filter built from today's std primitives (6 instances, 5 in one loop) is **bit-exact** with `std.svf` over 10 s of noise, and costs 119 ns per frame against 12 ns (9.8×). A modulated one would need about 24 instances (about 28×, estimated).
- A fused kernel (the same filter as a 12-operation register-file interpreter, outside the engine) costs about 20 ns per frame, bit-identical.
- Determinism held in every case: the same hash across k = 1, 64, 256 and 1 or 18 threads.

Two traps found: canonical fan-in order is by source address, so a sum inside a composite can associate differently depending on addresses outside it; and constants computed outside the engine's `libm` (by hand, in TOML) can differ by one ulp.

Missing from std today: division and the transcendental functions, comparison and selection, a sample-rate source, memory, value events from signals, triggered randomness, vector utilities.

## Decision
1. **The core set is a deliberately complete, minimal set of Rust primitives in `std.*`**, marked by a new descriptor field `category` (`core`, `generator`, `filter`, `envelope`, `delay`, `physical`, `event`, `visual`, `io`), which groups the palette and `signal modules --category`. Not `core.*`: that prefix is the converters' (`docs/module-contract.md`), and "core" already names the domain-free crates. The existing std arithmetic is part of the set and keeps its names. Every core primitive:
   - passes the contract suite and states its formula in its rustdoc;
   - is total: finite in, finite out (`div` by 0 gives 0, `sqrt` of x < 0 gives 0, `log` of x ≤ 0 gives the log of the smallest normal, `pow` defined everywhere), each case documented and tested;
   - takes **ordered operands** (`a`, `b`, …) wherever association matters, so a result never depends on fan-in address order;
   - computes transcendental functions with `libm` (ADR 0007).

   | Group | Primitives (✓ exists) |
   |---|---|
   | Arithmetic | add✓ sub✓ mul✓ div neg abs✓ min✓ max✓ clamp✓ scale✓ mod (floored) fract floor round (half-even) sign |
   | Functions | sqrt exp log pow sin cos tan tanh atan2 (radians), sin_cycles (cycles, the oscillator convention) |
   | Constants and rate | const, sample_rate (sr and 1/sr), frames (frames run, exact to 2^53) |
   | Compare and select | compare (lt, le, eq, ge, gt, ne → gate), select (gate > 0 ? a : b), and, or, not |
   | State (hides the loop) | unit_delay✓ delay✓ phasor (wrap, reset), counter (triggers → count, modulo, reset, wrap trigger), integrator (TPT step, reset), lag✓ onepole✓ |
   | Memory | buffer (N frames, N fixed per instance, allocated in prepare; write index, value, gate; R read taps; interpolation none, linear, Hermite; read-after-write within a frame defined), table (read-only, from parameters or a project asset; interpolated) |
   | Events | sample_hold✓ latch (a Set value held as a signal) to_events (a signal → Set on change or trigger) edge✓ route (events to a or b by a gate) counter clock_div✓; merge is event fan-in (ADR 0016 §3) |
   | Randomness | noise✓, random (trigger → a uniform value held; the instance's seeded stream, ADR 0008) |
   | Vectors | pack, unpack, index (fractional), reduce (sum, min, max, mean, in element order), elementwise add and mul; widths registered on demand |

2. **Granularity follows the measurement.** An acyclic node costs about 2.5 ns per frame, so a core primitive is one formula term. A node in a loop costs about 22 ns, so every common one-sample recurrence is one stateful primitive (phasor, counter, integrator, smoother, delay), and library composites are written acyclic where they can be. A composite meant for polyphonic use that closes a loop over more than about 8 nodes is a sign that its algorithm belongs native (3).

3. **What stays native Rust** ("a core cell is Rust source text"): algorithms whose per-sample feedback crosses many terms are std primitives: filters with modulated coefficients (state-variable, ladder, biquad, allpass, comb), band-limited oscillators (polyBLEP, wavetables with mip levels, FM operators with feedback), interpolated delays and reverb cores (FDN), physical models, envelopes with exact timing, FFT and spectra, nonlinear zero-delay-feedback solvers. Same contract, same suite.

4. **The built-in library: composites shipped with Signal.** A module reference `lib.<name>` resolves to a composite shipped in a new crate `signal-lib` (TOML embedded with `include_str!`), a third form beside `std.*` primitives and bare project composites (`project-format.md` §4). The registry refuses `lib.` names; a project composite may not shadow a library one. They are ordinary composites (ADR 0010: no logic in composites), listed by `signal modules`, read-only, and `signal vendor lib.<name>` copies one into the project's `composites/`, where it is the project's own text (ADR 0009).
   - **Versioning:** the library is the app's MAJOR.MINOR. A change of a composite's output is a new name or a MINOR change with a changelog entry; names are never reused; a removed one warns for one MINOR first. The render manifest lists every library composite used with its hash (ADR 0008); a `.signal` file records the library version (ADR 0025).
   - **Validation:** every library composite with a native counterpart is rendered against it on the same stimulus: bit-exact, or within a stated, measured tolerance; the cost ratio is recorded in `docs/benchmarks.md`. Native primitives remain as references and as the fast path.

5. **Rates for core math** (decided in the issue that builds it): core arithmetic, function, compare and select types become rate-generic, an instance key `rate = <divisor>` choosing the divisor, so a coefficient graph can run at control rate with the converters inserted as today, Signal's substitute for Reaktor's compute-on-change. If that contract change is refused there, coefficient graphs run at divisor 1 at the measured cost.

6. **UI:** the palette groups by `category` (and library, project); a library composite opens by semantic zoom (ADR 0026), read-only, with "vendor to edit"; a native primitive shows its formula when selected.

## Alternatives considered
- **A fused core-composite compiler** (a composite compiled at plan time into one kernel: SSA over a register file, interpreted, later JIT). It would bring a loop composite from about 10× native to about 2× (measured, micro-benchmark). It is not a DSL in ADR 0010's sense (the text is the same TOML), but it is a second execution engine whose bit-identity with the per-frame reference, faults, state carried across swaps (ADR 0006) and trust boundary would all need proving; it would supersede parts of ADR 0002 and 0016. **Revisit** when a project from the owner's list misses its budget because of loop-heavy library composites with no native counterpart, or a library composite in polyphonic use exceeds 10× its native reference.
- **A cheaper path for cycle units in the engine** (the same semantics, less per-call cost at k = 1): compatible with this decision; measured first (M2.10).
- **A `core.*` namespace:** rejected (the converters').
- **Visual editing inside core cells:** not required by the owner.
- **Per-project Rust modules compiled by the app:** a Rust toolchain at run time, `unsafe` dylib loading without a stable ABI, and native code that can hang or cause undefined behaviour inside the process. WASM (wasmtime) would sandbox it and bound its time, at about 1.5–3× native and a large dependency. Either needs its own ADR, superseding ADR 0010's "no scripting yet". Not now.

## Consequences
- About 30 new std primitives with contract tests, and a `category` on every std type.
- A new crate `signal-lib`, a third module-reference form, manifest provenance for library composites, `signal vendor`.
- A validation suite pairing library composites with native references, and costs predictable from the measured overheads (about 2.5 ns per acyclic node, 22 ns per loop node).
- New algorithms with dense per-sample feedback still need a Rust primitive and a rebuild (ADR 0010 unchanged).
