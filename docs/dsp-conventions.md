# DSP conventions

**Normative.** These conventions apply to every primitive in `crates/` and `lab/`. They implement ADR 0007 (numerics) and complement `module-contract.md` (how a primitive is written) and `realtime-rules.md` (what may run on the audio thread). `crates/signal-std` follows them and is the reference.

## Units

- **Signals** carry plain numbers. Audio is full scale at ±1.
- **Frequencies** are in Hz (`unit: "Hz"`).
- **Durations** are in milliseconds (`unit: "ms"`).
- **Phases** are in cycles (`unit: "cycles"`): 1 is a whole period. They are not in radians or degrees.
- **Gains** are linear factors, not dB.
- **Tempo** is in beats per minute (`unit: "bpm"`). Musical time exists only in library primitives such as `std.transport`, never in the core (ADR 0003).
- A parameter states its unit in its descriptor and in its rustdoc. A parameter without a unit says so.

## Ranges

- Every parameter declares a range and a default inside it. The executor clamps the effective value to the range (ADR 0016 §4), so a primitive never sees a value outside it.
- A descriptor is static, so a range cannot depend on the sample rate. A rate-dependent limit, such as `sr/2` for an oscillator frequency, is declared as the widest static bound and applied in `process`. The rustdoc states both.
- Declare the range of an output when it is known (`[−1, 1]` for a sine). Declare no range for a signal input that accepts any value.
- A choice with one value in M1 (such as `std.lfo`'s `shape`) is a parameter with a one-point range, so that later values do not change the port layout.

## Numbers: f64

- Every signal value, parameter value and state variable is `f64` (ADR 0007). `f32` appears only at device and file boundaries.
- Phase is an f64 accumulator in cycles, wrapped with `fract(x) = x − ⌊x⌋` after every frame. Never let a phase grow without bound, and never keep it in radians.
- No fast-math, and no reassociation that the formula does not state. Write the formula in the order the rustdoc gives it, so that a test can compare exact values.

## Transcendental functions: `libm`

- Every transcendental function (`sin`, `cos`, `tan`, `asin`, `exp`, `log`, `log1p`, `pow`, `tanh`, `cosh`, …) comes from the pure-Rust `libm` crate (ADR 0007), never from `f64`'s methods. Those call the platform's libm, which can differ between platforms, and Signal's output must be bit-identical everywhere.
- Prefer the numerically stable form: `log1p(x)` over `log(1 + x)`, and `|u| + log1p(e^(−2|u|)) − ln 2` over `log(cosh(u))`, which overflows.

## Time and rounding

- Time is frames (ADR 0003). A duration in milliseconds becomes `ms·sr/1000` frames.
- When a duration must be a whole number of frames, use the one rounding rule: `round_half_up(x) = ⌊x + ½⌋`, with ties going toward +∞. For example, `std.exp_env`'s attack takes `round_half_up(attack_ms·sr/1000)` frames.
- A continuous time constant may stay fractional, such as a decay's τ in frames.
- **Positions on a grid** (beats, 16ths) are computed from their index in exact integer arithmetic (`u128`), as `round_half_up(p/q) = ⌊(2p + q)/2q⌋`. They are never accumulated in floating point, so long polymeters do not drift (ADR 0003).

## Denormals

- Subnormal numbers are slow on most CPUs, and they appear when a value decays geometrically toward 0. Rust cannot set flush-to-zero flags safely (ADR 0007), so primitives flush explicitly.
- **Threshold: 1e-9.** A decaying value is set to exactly 0 when its magnitude falls below 1e-9. This applies to envelopes and to the state of a recursive filter's feedback path. 1e-9 is −180 dBFS, about 40 dB below the smallest step of 24-bit audio (2⁻²³ ≈ 1.2e-7), so the flush is inaudible.
  - `std.exp_env` flushes its output and goes idle.
  - `std.dc_block` flushes `y[n]`, which is its output and its feedback state.
- A primitive whose output can only be subnormal when its input is subnormal (a gain, a delay) need not flush.
- The contract check `no_subnormals` drives every primitive with an impulse and 65 536 frames of silence. It fails on any subnormal output.

## Antialiasing

- A nonlinearity that creates harmonics aliases at the sample rate. **ADAA comes first**: antiderivative antialiasing. First order replaces `f(u[n])` with the mean of `f` over the step from `u[n−1]` to `u[n]`:

  ```text
  y[n] = (F(u[n]) − F(u[n−1])) / (u[n] − u[n−1]),   F' = f
  ```

  - When `|u[n] − u[n−1]|` is below a small threshold, the quotient is ill-conditioned. Use its limit `f((u[n] + u[n−1])/2)` instead. `std.tanh_shaper` uses 1e-6.
  - Sum the difference `F(u[n]) − F(u[n−1])` in a form with as little cancellation as possible.
  - ADAA delays the signal by half a frame. Every ADAA primitive documents that delay.
- **Oversampling is not used in M1.** The M3 lab experiment compares it with ADAA (ADR 0015 governs promotion).
- Oscillators produce only band-limited waveforms. M1 has only the sine. Other waveforms need their own antialiasing and are out of scope in M1.

## The rate-agnostic rule

- Everything that depends on the sample rate is derived from `ctx.sample_rate` in `prepare`, or computed per frame from the rate `prepare` stored. Examples are phase increments, frame counts of durations, filter coefficients and grid positions. No constant assumes a rate.
- `prepare` fully reinitialises the instance. An instance prepared again at another rate renders exactly what a fresh instance at that rate renders. The contract check `rate_agnostic` enforces this at 44.1, 48, 96 and 192 kHz.
- Parameters are in physical units (Hz, ms, bpm), so a project sounds the same at every rate, up to the rate's own limits (`sr/2`).

## When parameters are read

- Normally a primitive reads every parameter's effective value on every frame.
- When a primitive reads a parameter only at certain frames, the rustdoc says when. Examples:
  - `std.exp_env` reads `attack_ms` on the trigger frame.
  - `std.transport` reads `bpm` and `ghost_offset_ms` once, on the first frame.

## Randomness

- All randomness comes from the instance's `RngStream` (`ctx.rng`, ADR 0008). `reset` restarts it from the copy kept in `prepare`.
- The rustdoc states exactly how many draws are made, and when. Examples:
  - `std.noise` makes one draw per frame.
  - `std.transport` makes one draw per ghost candidate, whatever the probability.

  A fixed draw schedule is what makes a seed change only what it is meant to change.

## Documenting a primitive

The rustdoc of every primitive states:
- its ports (direction, shape, kind, unit, range);
- its parameters (unit, range, default, `combine`);
- its feedthrough class;
- its formula, including the initial state;
- anything read only at certain frames, and its random draws;
- its worst-case work per frame.

`crates/signal-std` is the template.
