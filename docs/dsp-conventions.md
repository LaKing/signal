# DSP conventions

**Normative.** These conventions apply to every primitive in `crates/` and `lab/`. They implement ADR 0007 (numerics) and complement `module-contract.md` (how a primitive is written) and `realtime-rules.md` (what may run on the audio thread). `crates/signal-std` follows them and is the reference.

## Units

- **Signals** carry plain numbers. Audio is full scale at ±1.
- **Frequencies** are in Hz (`unit: "Hz"`).
- **Durations** are in milliseconds (`unit: "ms"`). One exception: the decay of a physical model (`std.string`, `std.modal`, `std.mass_spring`) is its T60, the time to fall by 60 dB, in seconds (`decay_s`, `unit: "s"`), as resonances are usually given.
- **Phases** are in cycles (`unit: "cycles"`): 1 is a whole period. They are not in radians or degrees.
- **Gains** are linear factors, not dB.
- **Tempo** is in beats per minute (`unit: "bpm"`). Musical time exists only in library primitives such as `std.transport`, never in the core (ADR 0003).
- A parameter states its unit in its descriptor and in its rustdoc. A parameter without a unit says so.

## Ranges

- Every parameter declares a range and a default inside it. The executor clamps the effective value to the range (ADR 0016 §4), so a primitive never sees a value outside it.
- A descriptor is static, so a range cannot depend on the sample rate. A rate-dependent limit, such as `sr/2` for an oscillator frequency, is declared as the widest static bound and applied in `process`. The rustdoc states both.
- Declare the range of an output when it is known (`[−1, 1]` for a sine). Declare no range for a signal input that accepts any value.
- A choice with one value in M1 (such as `std.lfo`'s `shape`) is a parameter with a one-point range, so that later values do not change the port layout.
- A choice with *N* values is an integer-valued parameter with the static range `[0, N−1]`. `process` rounds it with `round_half_up` on every frame, and the rustdoc lists what each integer selects. Example: `std.osc`'s `shape` (0 sine, 1 saw, 2 square, 3 triangle).

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
- A fractional delay is read with 4-point, 3rd-order Hermite interpolation (`std.delay`). A state-output delay of `d` frames cannot read the current input, so exact Hermite holds from `d ≥ 2`; for `1 < d < 2` the unknown newest point is replaced by the previous sample, which is exact at `d = 1` and continuous at `d = 1` and `d = 2` (operator decision, M2-7).
- **Positions on a grid** (beats, 16ths) are computed from their index in exact integer arithmetic (`u128`), as `round_half_up(p/q) = ⌊(2p + q)/2q⌋`. They are never accumulated in floating point, so long polymeters do not drift (ADR 0003).

## Denormals

- Subnormal numbers are slow on most CPUs, and they appear when a value decays geometrically toward 0. Rust cannot set flush-to-zero flags safely (ADR 0007), so primitives flush explicitly.
- **Threshold: 1e-9.** A decaying value is set to exactly 0 when its magnitude falls below 1e-9. This applies to envelopes and to the state of a recursive filter's feedback path. 1e-9 is −180 dBFS, about 40 dB below the smallest step of 24-bit audio (2⁻²³ ≈ 1.2e-7), so the flush is inaudible.
  - `std.exp_env` flushes its output and goes idle.
  - `std.dc_block` flushes `y[n]`, which is its output and its feedback state.
  - `std.svf` and `std.onepole` flush each integrator state (`s₁`, `s₂`; `s`), and `std.ladder` each stage's (`s₁`…`s₄`).
  - `std.adsr` flushes its release to exactly 0 and goes idle. Its decay ends at exactly the sustain level when the distance to it falls below 1e-9.
  - `std.lag` flushes the difference `x[n] − y[n]`: below 1e-9 the output is exactly the input.
  - `std.biquad` flushes its two integrator states; `std.comb`, `std.allpass_delay` and `std.fdn_reverb` flush every value written into a recirculating line and their damping states, and `std.allpass_delay` its output.
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
- **`std.ladder`'s `tanh` is not antialiased.** It sits inside the zero-delay feedback loop, where ADAA's half frame of delay would detune the cutoff and the self-oscillation.
- **Oversampling is used by one std type, `std.tanh_shaper_os8`** (since M3). It runs `std.tanh_shaper`'s first-order ADAA at 8× the rate, raised and brought back by three ×2 linear-phase FIR stages (`signal_std::oversample`: Kaiser-windowed sinc, β = 9, designed in `prepare` in normalized frequency so the coefficients are the same at every rate; polyphase, fixed work per frame). Oversampling costs a delay, 73.8125 frames, which the type documents as an ADAA primitive documents its half frame, and a band-limited clipped output overshoots 1. So it is a separate type, chosen where aliasing matters and the delay does not, and `std.tanh_shaper` stays the default shaper. The comparison and the verdict are `experiments/m3-tanh-aliasing/` (ADR 0015); its other candidates stay in the lab (`lab.tanh_*`).
- Oscillators produce only band-limited waveforms. A waveform with discontinuities needs its own antialiasing: see the next section.

## Band-limited oscillators: polyBLEP and polyBLAMP

A naive saw, square or triangle computed from the phase has infinitely many harmonics, and every one above `sr/2` aliases. `std.osc` corrects the naive waveform near each discontinuity with a short polynomial residual, the difference between a band-limited and an ideal discontinuity. The residual covers the frame before and the frame after the discontinuity (2 points).

- **polyBLEP** (band-limited step) corrects a jump in the value: the saw's wrap and the square's two edges. With the phase step `Δ = f/sr` and `t` the phase since the jump, the residual of an upward jump of 2 is

  ```text
  blep(t) = 2x − x² − 1,  x = t/Δ          if t < Δ        (the frame after)
          = x² + 2x + 1,  x = (t − 1)/Δ    if t > 1 − Δ    (the frame before)
          = 0                              otherwise
  ```

  It is added for an upward jump and subtracted for a downward one: `saw = 2p − 1 − blep(p)`.
- **polyBLAMP** (band-limited ramp) corrects a jump in the slope: the triangle's corners. It is the integral of the polyBLEP residual. For a slope change of 1 per frame it is `(1 − t/Δ)³/6` after the corner and `((t − 1)/Δ + 1)³/6` before it. A slope change of *c* per cycle is *c*·Δ per frame, so the residual is scaled by *c*·Δ.
- The polynomials assume `Δ ≤ ½`, which the clamp of the frequency to `sr/2` guarantees. With `Δ = 0` there is nothing to correct.
- A 2-point residual is an approximation. It lowers aliasing but does not remove it, and less is removed as the fundamental rises. Each oscillator documents its measured residual aliasing: `signal_eval::aliasing` on an off-divisor tone, as in `testing-and-evaluation.md`, "The V6 measurement". `std.wavetable` avoids the problem instead: its tables are additive, with one mip level per octave holding only harmonics below sr/2 at a constant pitch and `pm` = 0. What remains is the table interpolation's images, largest in the bass: measured on a saw at 48 kHz, −98.4 dB relative to the fundamental at 20 Hz, −102.1 dB at 30 Hz, −116.0 dB at 60 Hz, −129.1 dB at 120 Hz, below −170 dB from 1.2 kHz up (its rustdoc has the table). Hard sync and oversampling are out of scope.

## TPT filters

Filters are written in the topology-preserving transform (TPT, Zavalishin) form: the analog structure is kept, and each integrator is discretised with the trapezoidal rule. The result is the bilinear transform of the analog prototype, prewarped so the cutoff is exact. Unlike a direct-form biquad, the state stays meaningful when the coefficients change, so the filter is stable under audio-rate modulation of its cutoff.

- The integrator gain is `g = tan(π·fc/sr)` (`libm::tan`). The cutoff is clamped in `process` to `[1, 0.49·sr]`, which keeps `g` finite. The declared range is the static bound `[1, 0.49·(2³² − 1)]`.
- A trapezoidal integrator keeps one state `s`. Each frame computes `v = g·u`, outputs `y = v + s`, and stores `s ← y + v`.
- **One-pole** (`std.onepole`): `G = g/(1 + g)`, `v = (x − s)·G`, `lp = v + s`, `s ← lp + v`, `hp = x − lp`.
- **State-variable** (`std.svf`): `k = 1/q`, then `hp = (x − (k + g)·s₁ − s₂)/(1 + k·g + g²)`, `bp` and `lp` from two integrators in series. `lp + k·bp + hp = x`, and `bp` has gain `q` at the cutoff.
- **Ladder** (`std.ladder`, Zavalishin rev. 2.1.x §5.2–5.3, §6.3–6.5): four one-pole stages in a feedback loop of gain `k = 4.2·resonance`, with one `tanh` at the loop's input, `u = tanh((1 + drive)·x − k·y₄)`. The zero-delay feedback equation is solved by at most 8 bracketed Newton steps per frame, and the passband is compensated by `1 + k`. Its cutoff is clamped to `[1, 0.45·sr]`, not `0.49·sr`. The rustdoc has the formula and the output bound.
- **Biquad** (`std.biquad`): the RBJ cookbook responses as an SVF with output mixing (Simper): `out = m₀·x + m₁·bp + m₂·lp`, with `g` and `k` adjusted for the peak and the shelves. The cookbook's TDF-II gives the same transfer function for fixed coefficients, but its state belongs to the old coefficients when they move, so it is not used.
- Coefficients that need a transcendental function are recomputed only when their inputs change, which is never more than once per frame. The result does not depend on how the frames are split into calls.
- The integrator states are flushed below 1e-9 (see "Denormals").

## The rate-agnostic rule

- Everything that depends on the sample rate is derived from `ctx.sample_rate` in `prepare`, or computed per frame from the rate `prepare` stored. Examples are phase increments, frame counts of durations, filter coefficients and grid positions. No constant assumes a rate.
- `prepare` fully reinitialises the instance. An instance prepared again at another rate renders exactly what a fresh instance at that rate renders. The contract check `rate_agnostic` enforces this at 44.1, 48, 96 and 192 kHz.
- Parameters are in physical units (Hz, ms, bpm), so a project sounds the same at every rate, up to the rate's own limits (`sr/2`).

## When parameters are read

- Normally a primitive reads every parameter's effective value on every frame.
- When a primitive reads a parameter only at certain frames, the rustdoc says when. Examples:
  - `std.exp_env` reads `attack_ms` on the trigger frame.
  - `std.adsr` reads `attack_ms` on the frame its attack starts.
  - `std.transport` reads `bpm` on the first frame and on each beat's frame (a new tempo starts on a beat), and `ghost_offset_ms` once, on the first frame.

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
