# ADR 0052: Every device at once

- **Status:** Proposed (2026-11-01). The owner's question of 2026-11-01: "Could every audio device be usable at the same time?" The design is the operator's, delegated; the open questions at the end are the owner's.
- **Builds on:** ADR 0003 and vision §4.7 (other devices attach through clock-domain adapters), 0007 (pure-Rust math), 0008 (recorded inputs), 0019 (real-time threads), 0022 (output policy in adapters), 0036 (the device's clock is the run's; no rate switching; read-only `device_props.rs`; fades), 0038 (roles, layouts, the input ring, hot-plug), 0051 (the Setup level).
- **Supersedes, when accepted, part of ADR 0038:** §8's v1 rule (a role only on the clock device or an aggregate) and its "Later (M6): clock-domain adapters" item, and the "other clock" state of §10 and of ADR 0051 §3. Clock-domain adapters for **audio** devices move from M6 to **M4.1b**; MIDI, OSC and external sync stay in M6. The rest of ADR 0038 stands, including "several clocked devices without adapters" as rejected.
- **Date:** 2026-11-01

## Context
- ADR 0038 §8 hears a role only on the clock device or on an aggregate that is the clock device; elsewhere it is "another clock", not heard.
- Two crystals at a nominal 48 kHz differ by up to about ±100 ppm (assumption): 4.8 frames per second; a fixed buffer breaks within about 30 s.
- Aggregates work, but the person must build one in Audio MIDI Setup; Signal may not (it writes system state).

## Decision

### 1. One clock device runs the engine (unchanged)
The clock device's callback runs the engine (ADR 0036 §1); the engine, its workers (in the clock device's workgroup only) and its output are untouched. **Every other device** a role is routed to (`device` in `[play.<role>]` or `[capture.<role>]`, ADR 0038 §4) is a **secondary device**: it gets its own stream and real-time callback, and one **clock adapter** per direction.

### 2. The clock adapter
A lock-free `rtrb` ring between the engine and the device, and an asynchronous sample-rate converter whose ratio a drift loop steers.

- **The converter runs on the side whose frame count is fixed by demand:**
  - **output:** the audio adapter pushes the role's channels (after the layout adapter, `gain`, clamp and f32 conversion) at the engine's rate; the secondary callback converts exactly the frames it must deliver;
  - **input:** the secondary callback pushes raw device frames; the clock callback converts exactly the frames the engine's block needs, before the source fill.
- **The drift loop is a PI controller on the ring's fill level**, chosen over ADR 0038's timestamp loop because fill is the quantity that must stay bounded; cpal's callback timestamps are host times with scheduling jitter, and reading the device's sample clock (`AudioDeviceGetCurrentTime`) would widen `device_props.rs` beyond property reads; and fill needs nothing from the device, so fake callbacks test the loop exactly. Timestamps (cpal's safe `StreamInstant`) are still read for the displayed drift and to detect discontinuities (§6).
- **Loop:**
  - the fill (ring plus unread converter history, in source frames) is sampled once per callback on the converting side and low-passed (one pole, τ = 0.5 s) to remove the two periods' sawtooth;
  - with *e* the filtered fill minus the target, ratio = nominal × (1 + Kp·*e* + Ki·∫*e* d*t*), Kp = 2ζω/*f*ₛ, Ki = ω²/*f*ₛ, ζ = 0.7 (the fill integrates the rate difference, so both poles sit at ω);
  - **acquire** (ω = 2π × 0.2 Hz) until |*e*| ≤ 2 frames for 2 s, then **track** (ω = 2π × 0.02 Hz) with the integrator kept (bumpless);
  - the correction is clamped to ±1000 ppm and slewed at most 20 ppm/s (100 ppm is 0.17 cent);
  - the integrator removes steady-state error for a constant drift; the acquire excursion at 100 ppm is about 4 frames (estimate).
- **Position:** 64-bit fixed point (32.32). The ratio's rounding (≤ 2⁻³² per frame, 0.23 ppb) cannot accumulate: the loop closes on fill, not on a computed position.

### 3. Latency, startup, underrun and overrun
- **Target fill** F* = *P*ₑ + *P*_d + *J* + *H* (engine period, device period, jitter margin *J* = 1 ms, converter half length). At 48 kHz, 128-frame periods and 96 taps: 352 frames, **about 7.3 ms**. Ring capacity 4 × F*, rounded up to a power of two.
- **Reported latency** of a secondary role: F* + the converter's group delay + the device's own latency (`kAudioDevicePropertyLatency`, `…SafetyOffset`, stream latency; read only).
- **Startup:** the device plays silence until the ring holds F*, then fades in (ADR 0036's 10 ms raised cosine), the ratio at nominal, the loop in acquire.
- **No clicks: hold and repair.** The converter keeps 1024 source frames of history.
  - **Underrun** (the fill would fall below this callback's need): a **repeat splice**, the read position crossfading back by F* − fill into the history (equal power, 5 ms); a few milliseconds are heard twice instead of a gap.
  - **Overrun** (fill > 3 × F*): a **skip splice** forward to F*, same crossfade.
  - **Empty ring** (the engine stopped): 5 ms fade out from the history, silence, then the startup fill again.
  - Each splice is counted (`slips`) and keeps the integrator. In the steady state none occur.
- **The engine side never blocks:** a push into a full ring drops and counts the frames. That happens only when the device stopped consuming, and §6 then removes the route.

### 4. Real-time safety (hard rules 1, 2, 7, 9)
- **No allocation or lock in either callback.** Rings, tables, history and loop state are allocated on the control thread when a route is made and reach the audio adapter as a prepared table through its command queue (ADR 0038's hot-plug); the replaced table returns to the control thread to be dropped.
- **The converter is bounded:** polyphase Kaiser-windowed sinc, a tap count fixed when the route is made, 512 phases with linear interpolation between neighbours, f32 tables and accumulator.
  - Equal nominal rates (drift only): 96 taps, passband 0–20 kHz at 48 kHz.
  - 44.1 ↔ 48 kHz: 160 taps, transition 20–22.05 kHz (below the lower Nyquist).
  - At most 256 taps. Work per callback = taps × channels × frames multiply-adds (about 41,000 for 2 channels, 128 frames, 160 taps), bounded by the device's maximum buffer, checked when the route is made.
- **Quality targets:** passband ripple ≤ ±0.01 dB to 20 kHz; alias and image rejection ≥ 100 dB beyond the lower Nyquist; THD+N of a 1 kHz sine at −1 dBFS ≤ −100 dB; phase-interpolation error below the stopband (assumption at 512 phases, to be measured).
- **Placement:** the adapter (ring ends, converter, loop, splices) is output policy, plain code in `signal-output` (ADR 0022). `signal-audio` opens the secondary streams through cpal and calls it; they run on CoreAudio's own I/O threads. The engine gains nothing; `domain_scope.rs` adds `resampl`, `drift`, `ppm`, `clock_adapter`.
- **No new `unsafe`** (cpal gives streams and timestamps; latencies are property reads in `device_props.rs`) and **no new dependency**: the converter is ours (ADR 0007), measured with the locked `realfft`.

### 5. Device rates
- **Signal never changes a device's rate** (the owner's rule); it reads it. A secondary device at another nominal rate uses the same converter at the nominal ratio (48000/44100), steered by the same loop.
- A secondary device's rate changed by another app is seen by the control thread's poll; the route is rebuilt at the new ratio and swapped in with a fade. The clock device changing rate stops the run, as today (`rate_moved`).

### 6. Failure and status
- **Unplugged mid-run:** the stream's error callback or the device poll reports it; the control thread removes its routes and the box shows "not present". On return the routes are rebuilt with a fresh ring and the startup fill. The clock device departing stops the run, as today.
- **Sleep and wake:** the clock device resumes through Signal.app's device wait (unchanged); secondary streams restart with the startup fill.
- **A clock that jumps** (a callback's host time more than two periods past the frames delivered, |*e*| > F*/2 for 0.5 s, or the correction at its clamp): the adapter splices to F*, returns to acquire and counts a **resync**. At the clamp for 10 s, the device is **clock suspect** and faded out until it recovers or is re-routed.
- **Status, shown and not wired** (ADR 0038's rejected status outputs stand). `devices` and `devices.list` gain per device: `clock` (`engine` | `adapted`), `state` (`locking`, `locked`, `resync`, `suspect`, `absent`), `drift_ppm`, `latency_ms`, `slips`, `resyncs`, and the nominal ratio when the rates differ; `signal play`'s status line prints them.
- **At the Setup level** (ADR 0051 §2–3): the clock device's box carries a **clock** badge; a secondary box reads "adapted · +37 ppm · 7.3 ms" (and "44.1 → 48 kHz"), pulses while locking, shows `suspect` in the warning colour with its reason, and `absent` hollow as ADR 0051 draws it. "Unusable (other clock)" goes.

### 7. Determinism (hard rule 3)
- **The engine's output stays bit-identical.** Adapters sit after the sinks and before the source fill, outside the engine. Offline renders, goldens, `devices/<slot>.wav` and replay have no adapters.
- **Only the device-side resampling is non-deterministic**: its ratio follows real device clocks, as the device's timing and DAC already lie outside the engine.
- **Sessions record the engine side:** the sinks' signals, and each secondary input as `inputs/<slot>.f32` *after* its converter, exactly what the engine read (ADR 0038 §9), so `signal replay` stays bit-exact. Adapter state goes into `[machine]` as provenance and never affects a replay.

### 8. Inputs from secondary devices
The mirror path: raw frames into the ring from the device callback, conversion in the clock callback with the same loop, splices and target, the source reading the result. Reported latency: F* + group delay + the device's input latency. Input on the clock device keeps ADR 0038 §8's ring without a converter.

### 9. Aggregate devices stay supported
An aggregate is one device to Signal, clock or secondary, its own drift correction untouched. A device both inside the aggregate in use and routed on its own (read only, `kAudioAggregateDevicePropertyActiveSubDeviceList`) is reported, not refused. Signal still does not create aggregates.

## Verification
1. **Fake clocks** (`signal-output`, deterministic): a virtual-time harness, an engine producer at *f*ₛ and a fake device whose callbacks come at ratio 1 ± ε with seeded jitter (±0.5 period) and sizes 64–1024; ε ∈ {0, ±20, ±100, ±300} ppm; period pairs 128/128, 128/512, 64/441, 512/64.
2. **Converter quality** (FFT): sweeps 20 Hz–20 kHz at ratios 1, 1 ± 100 ppm, 48/44.1, 44.1/48 against §4's targets; impulse delay equal to the reported group delay ±1 frame. Figures in `docs/benchmarks.md`.
3. **Convergence:** lock (|*e*| ≤ 2 frames) within 5 s for |ε| ≤ 100 ppm; in track mean |*e*| ≤ 0.5 frame, ratio jitter ≤ 0.5 ppm rms, zero slips; a 0 → 50 ppm ramp over 10 minutes without a slip.
4. **One hour simulated** (172.8 M frames, ε = +100 ppm, jitter on): zero slips, and a marker impulse every minute arrives at a constant delay (±1 frame): no drift accumulates.
5. **Faults:** underrun, overrun, empty ring (splice splatter ≥ 40 dB below a hard cut's); unplug and return, a 50 ms time skip, an ε step to 2000 ppm (resync, then suspect), a 10 s sleep, each with the right state and counters.
6. **Real time:** `signal-contract`'s allocation counter around both callback bodies, through route changes; worst-case multiply-adds asserted from the route; converter cost benchmarked.
7. **Determinism:** goldens, `output_identity.rs` and replay unchanged; under the null driver, the clock output with a fake secondary routed is bit-identical to the output without it.
8. **The owner's device check:** the SSL 2+ as the clock device and the Mac's speakers as a secondary device (then swapped), quietly (`gain` ≤ 0.1), a click and a tone to each for 30 minutes: `locked`, a plausible ppm, zero slips, no click heard. Unplugging a secondary SSL 2+ leaves the speakers playing; replugging returns it to `locked`. RT figures are measured on the Mac, not in the sandbox.

## Phases
| Phase | Delivers | Exit |
|---|---|---|
| 1. The adapter, offline | `signal-output`: converter, loop, splices, fake-clock harness | Tests 1–7; figures in `docs/benchmarks.md`; goldens unchanged; the operator's checkpoint |
| 2. Secondary outputs live | `signal-audio` secondary streams; any device in `[play.<role>]`; failures; status in `signal play` and `devices` | Fault tests on the device; two devices for 30 min without a slip (operator) |
| 3. Secondary inputs | The mirror path; recording after the converter | A mic on a secondary device recorded and replayed bit-exactly |
| 4. Setup level | ADR 0051's badges and states; `devices.list` fields | **The owner's verdict** on test 8 |

M4.1b follows ADR 0038's phases 2 and 3 (roles, the input ring, hot-plug polling).

## Alternatives considered
- **Keep the v1 rule** (aggregates only): answers the owner with "build an aggregate". Rejected; aggregates stay.
- **A timestamp loop** (ADR 0038's sketch): rejected as the controller (§2); kept for display and discontinuities.
- **Slip-only correction** (drop or repeat frames): a splice every few seconds at 100 ppm. Rejected.
- **Linear or cubic interpolation:** images at about −40 to −60 dB. Rejected.
- **A resampler crate** (rubato, say): a new dependency and an allocation audit, for about 300 lines of our own. Rejected for now.
- **An engine per device:** breaks the single graph and determinism. Rejected.

## Consequences
- ADR 0038 §8, §10 and ADR 0051 §3 read "adapted" for "other clock"; the M6 notice goes.
- `signal-output` gains the clock adapter, `signal-audio` secondary streams, `signal-ui` and `ui/` the status. Core and engine unchanged.
- Docs: `realtime-rules.md`, `session-format.md`, `ui-protocol.md`, `agent-interface.md`, `benchmarks.md`.
- A secondary role costs about 7–10 ms of latency and tens of microseconds per callback (estimate).

## Risks
- Secondary devices are **not sample-aligned** with the clock device; their latencies are reported, not compensated (question 2).
- cpal on several devices at once is assumed to give independent callbacks; phase 2 measures it.
- The loop constants are a design, not measurements; phase 1 fixes them from tests 3–4.

## Open questions (for the owner)
1. **Latency against safety:** a 1 ms jitter margin (about 7 ms per secondary role at 128 frames), or 3 ms for fewer slips on a busy machine?
2. **Alignment:** delay the clock device (and faster secondaries) so every device sounds at the same moment, at a latency cost everywhere, or are reported latencies enough for now?
3. **A clock-suspect device:** faded out (proposed), or left playing out of tune?
4. **"Make this the clock device"** on a secondary box at the Setup level, restarting the run on it?
