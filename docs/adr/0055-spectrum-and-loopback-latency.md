# ADR 0055: Spectrum and loopback latency (M4)

- **Status:** Accepted (operator, 2026-11-15, under the owner's delegation of technical decisions: "Minden eddigi megfogalmazott mérföldkő készüljön el")
- **Milestone:** M4, the second domain: analysis
- **Related:** ADR 0001, 0003 (domain concepts live in library modules), ADR 0027 (the core set), ADR 0038 (devices, capture, renders), `01-inception-proposal.md` §8 (M4: "generator → device out → device in → spectrum and latency report")

## Context
M4's deliverables were "audio input (recorded and replayed), rate divisors, vector ports, first auto-converters, FFT analyser, test project `tone-analyser`; D-EXEC-3 decided from benchmarks", and its exit "the PO judges the measurements credible". By 2026-11-15 most of it exists under other milestones:
- audio input, recorded after its converter and replayed bit for bit: ADR 0038 §7, §9 (M4.1), ADR 0052 phase 3;
- rate divisors, vector ports and the converters inserted where unambiguous (ADR 0005): M2.8 (`module-contract.md` "Rates and vectors", `signal_core::converter`);
- block execution, measured and bit-exact for every partition and thread count: M2.6 (`docs/benchmarks.md` "Block sizes", "Threads").

What is missing is the analysis itself: a spectrum a person can read and test against analytic signals, a loopback latency measurement, and the test project that strings them together.

## Decision

### 1. `std.spectrum512` and `std.spectrum2048`
A windowed magnitude spectrum of its input, as a vector, with its strongest partial.
- **Ports:** input `in` (audio). Outputs `mag`, a vector of *N*/2 bins (256 or 1024: widths the vector types have), `peak_hz` (Hz) and `peak` (linear). No parameters.
- **At divisor 1** (`module-contract.md`: a type at a rate *d* above 1 sees only its ticks, and a spectrum needs every sample). Every *N*/4 frames counted from its reset (75 % overlap) it analyses the last *N* inputs, zeros before the first; between analyses its outputs hold.
- **Normalisation:** a periodic Hann window; bin *b* (0 ≤ *b* < *N*/2, centre *b*·*f*s/*N*) is `2·|X[b]| / Σw`, so a sine of amplitude *A* centred on a bin reads *A* there (bin 0 is `|X[0]| / Σw`, a constant's value).
- **Peak:** the largest bin among 1 … *N*/2 − 2, refined by Grandke's interpolation for a Hann window (offset δ = (2γ − β)/(β + γ) toward the larger neighbour γ of the peak β, the height β·(1 − δ²)/sinc δ; exact for one sine, unlike a parabola through the logarithms, which missed an off-bin sine by 1.6 % of a bin in the first build): `peak_hz` its centre, `peak` its height; 0 and 0 when every bin is below 1e−9.
- **Arithmetic:** a radix-2 complex FFT of our own (about 60 lines, `signal-std/src/spectrum.rs`), f64, its twiddles and window from `libm` in `prepare`. Not `realfft`: it chooses SIMD paths at run time, which would make the type same-machine only; this one is **bit-exact on any machine**, as the core set is. `process` allocates nothing (buffers sized in `prepare`).
- **Cost:** one FFT of *N* per *N*/4 frames plus the held vector copied each frame (*N*/2 values): for 2048 at 48 kHz about 1 µs per frame on an M-series core (to be measured, `benchmarks.md`).

### 2. `std.latency_probe`
A click out, the same click found in what comes back: a round trip in frames.
- **Ports:** input `in` (audio). Outputs `click` (audio: `level` on one frame every `period` seconds, the first at frame `period`/2), `frames` (the last measured delay, held; −1 until one is found) and `found` (1 while the last click was found within its period, else 0).
- **Parameters:** `period` (s, 0.25–10, default 1), `level` (0–1, default 0.5), `threshold` (0.001–1, default 0.05).
- **Detection:** after each click, the first frame whose |`in`| reaches `threshold` within the period is the delay from the click's frame; frames are counted from reset, so the result does not depend on the callbacks. A delay of 0 is the click itself (a wire); the probe is a measurement, not a correlator: noise above the threshold reads as an early return, which `found` cannot tell. A correlation probe waits for a project that needs one.

### 3. `projects/tone-analyser`
The test project of the inception proposal: a 1 kHz tone at 0.05 (−26 dBFS) and the probe's clicks at 0.15 summed into `main` (at most 0.2 per channel); a capture slot of role `loop` (mono) feeding a `std.spectrum2048` and the probe; the measurements on a discrete report slot `report = { device = { play = ["latency", "found", "peak_hz", "peak"], role = "report" } }` (ADR 0038's discrete layouts), so `signal render` writes them to `devices/report.wav` frame by frame. The probe's threshold (0.05) sits above the tone after a halving loop and below the click, so the tone alone never reads as a return.
- **Offline:** `signal render --input loop=<wav>` feeds the loop (ADR 0038 §9). The project ships no loop file: the test makes one from the render's own output, delayed by 480 frames and halved.
- **Live:** the person patches the device's output back into an input (or chooses a loopback device) and binds `loop` to it (`[capture.loop]`); the panel shows the measurements with `<signal-scope mode="value">`, a readout of a signal output's newest point (added to the scope for this), and the loop on an ordinary scope. A role the machine does not bind (`report`) is not heard, with its notice.

### 4. D-EXEC-3 is decided
The vision deferred D-EXEC-3 ("block execution only if measurements show a need") to benchmarks. M2.6 measured the need (`benchmarks.md` "Block sizes", 2026-10-12: at *k* = 256 the kick costs 141 ns per frame and 9 kick voices 1 028 ns, 2.7× and 2.5× less than at *k* = 1 in the same run; the roadmap's M2.6 row: 175 kick voices within 25 % of the budget on 18 threads, against 14 before) and built block execution bit-identical for every partition and thread count (ADR 0019); the vision's table already reads "Decided 2026-10-12 by ADR 0019". **M4's "D-EXEC-3 decided from benchmarks" is therefore met**; nothing changes here.

## Consequences
- `signal-std` gains three types (`std.spectrum512`, `std.spectrum2048`, `std.latency_probe`), category `analysis` (new in `category::ALL` and the palette's groups). Their measurement outputs (`frames`, `found`, `peak_hz`, `peak`) are of kind `generic`, so they wire into a discrete slot's audio ports without a kind warning.
- `<signal-scope>` gains `mode="value"` and `unit`.
- The schema snapshot and the std type count move; no golden changes (no existing project uses them).
- `projects/tone-analyser` with its panel, fixture and tests: the spectrum against analytic signals (a bin-centred sine, an off-bin sine, a sum of two, a constant, silence), the probe against a known delay, the project's render measuring the fixture's loop.

## Verification
1. A bin-centred sine of amplitude *A* reads *A* in its bin (±1e−9) and `peak_hz` its frequency (±1e−6 Hz); an off-bin sine (`f = (b + 0.3)·fs/N`) reads `peak_hz` within 0.5 % of a bin and `peak` within 0.5 % of *A*; two sines, each in its bin; silence, all 0.
2. The output does not depend on how the frames are split into calls (the contract suite).
3. The probe finds a delay of *d* frames exactly for *d* = 0, 1, 480, 4799 at 48 kHz with `period` 1 s, and −1 / 0 without a return.
4. `signal render projects/tone-analyser --input loop=<the output delayed 480 frames, halved>` writes `devices/report.wav` whose `latency` channel reads 480 from the click's return on, `found` 1, `peak_hz` 1000 Hz (±5 Hz) and `peak` 0.025 (±1 %), on 1 and 4 threads alike (`crates/signal-cli/tests/tone_analyser.rs`).
5. The exit: the owner reads the measurements live on the panel with a cable loop (or a loopback device) and judges them credible.
