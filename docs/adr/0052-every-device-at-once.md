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
  - **Staged for high source rates** (review 2026-11-06, which measured 46.9 dB of rejection at 192 → 44.1 kHz, 86.3 dB at 96 → 44.1 and 84.2 dB at 192 → 48 against the 100 dB target: 256 taps cannot reach it there). When the converter's filter would need more than 256 taps, the source rate is halved first, at most three times, by a Kaiser-windowed low-pass of odd length computed at every second frame (`signal-output/src/adapter/decimator.rs`), designed for 110 dB from `R/2 − L/2` (what would fold into the band below the lower rate's Nyquist frequency `L/2`). It runs in the push, on the producing side; the ring and everything after it count frames at the halved rate, and the reported latency adds the halvings' delay. 96 → 44.1 kHz converts 48 → 44.1 after one halving (117 taps), 192 → 44.1 after two (27 and 117), 176.4 → 44.1 after two (29 and 307); 96 → 48 and every upward conversion need none. Rates no halving brings within 256 taps (88 → 44.1 kHz, halved below 44.1) are refused (`AdapterError::Ratio`). Measured: every target met at 96, 88.2, 176.4 and 192 kHz into 44.1 and 48, and 44.1 or 48 into 96 and 192 (least rejection 107.4 dB); the halvings cost 1.4–6.1 % of a 512-frame push's period at 64 channels (`docs/benchmarks.md`).
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
- **Status, shown and not wired** (ADR 0038's rejected status outputs stand). `devices` and `devices.list` gain per device: `clock` (`engine` | `adapted`), `state` (`locking`, `locked`, `resync`, `suspect`, `absent`), `drift_ppm`, `latency_ms`, `slips`, `resyncs`, and the nominal ratio when the rates differ; `signal play`'s status line prints them. The output ceiling (`signal mcp --max-gain`) and the device's ±1 range hold on a secondary device after its conversion, the last step of its callback: interpolation and a slip's crossfade can overshoot what the Runner held. The samples held there are counted per device (`held`); the Runner's meters and counts are before the conversion.
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
| 1. The adapter, offline | `signal-output`: converter, loop, splices, fake-clock harness. **Built** (2026-11-01, `crates/signal-output/src/adapter/`), **no caller until phase 2**: kept as M4.1b's first phase (the operator, against hard rule 8) | Tests 1–7; figures in `docs/benchmarks.md`; goldens unchanged; the operator's checkpoint |
| 2. Secondary outputs live | `signal-audio` secondary streams; any device in `[play.<role>]`; failures; status in `signal play` and `devices`. **Built** (2026-11-06): routing adapts a role on another present device (`Heard::Adapted`, channels per device, `Routing::secondaries`); `SecondaryFeed` in the clock callback's preallocated slots and `SecondaryCallback` on the device's own stream (`signal-audio/src/secondary.rs`), opened at the device's rate with its own buffer, the rate re-read after cpal builds; a device gone (`StreamInvalidated`, `DeviceNotAvailable`) is stopped and rebuilt at its rate of the moment when it is back (`poll_secondaries`, every 0.5 s in `signal play` and the host); status in `signal play` (`--route ROLE@DEVICE=…`, its lines and JSON `secondaries`) and `setup.devices`' `secondaries`; the device's output latency and safety offset read in `device_props.rs`. A secondary is never the clock device (refused by id at each open), its missing channels are reported, a failing open backs off from 1 s to 30 s, and a stop lets each secondary play out its adapter's latency (at most 250 ms). Each pull is timed from one clock for both callbacks (`Instant::now` against the driver's epoch), not CoreAudio's output stamps, which run ahead of now by each device's buffer and safety offset: with the clock at 128 frames and the Mac Studio Speakers at a larger buffer the offset exceeded the stop detection's reach and the owner heard "scraps of sound" (2026-11-06). **Resync and clock suspect** (§6) in the adapter: a pull more than two periods late (a time skip), a tracking error beyond F*/2 for 0.5 s, the correction at its clamp for 1 s, or a slip while tracking (phase 2's addition: the track loop's 20 ppm/s slew does not reach a clock that runs away, and every splice moves its error back) splice to F* and return the loop to acquire with its integral kept; at the clamp for 10 s in all since the loop last locked, **or three slips within 10 s** (phase 2's addition: a clock beyond reach touches the clamp only now and then, since each splice resets the error), the adapter is `suspect`, faded out until the loop locks again (`signal-output/tests/adapter_faults.rs`: a 50 ms stall, a 2000 ppm step and back, a steady 100 ppm device never resyncing). A failed open is retried at once when the system's device list changes, else after its wait (1 s doubling to 30 s): an absent device costs the control thread one property read per poll. cpal's physical-format set while it builds (ADR 0036's residual risk) applies to a secondary device as to the clock device, outside Signal's code. Not yet: the `kAudioAggregateDevicePropertyActiveSubDeviceList` report (§9) | Fault tests on the device; two devices for 30 min without a slip (operator). **Device run 2026-11-06** (operator, silent: both sides at gain 0): the SSL 2+ Mk II as the clock device (48 kHz, its own 512-frame buffer, in use by another app) and the AT2020USB+'s output as the secondary (48 kHz, 512 frames), `signal play --route cue@AT2020USB+=1,2` for 1800 s: `locked`, +69.1 ppm (a 60 s run read +68.8), 20.1 ms reported latency, 0 slips, 0 xruns. Unplug and return on the device, and a 44.1 kHz secondary, still to run |
| 3. Secondary inputs | The mirror path; recording after the converter | A mic on a secondary device recorded and replayed bit-exactly. **In progress** (2026-11-06): the mirror path is built (`signal-audio/src/capture.rs`: `CaptureFeed` pushes a device's input into an adapter per capture side, `AdaptedSource` converts exactly a block's frames in the clock callback, each pull timed from its callback's start plus its offset in it: timed when the block runs, a callback's several blocks fell at one instant and the loop locked to a wrong slope); device run (silent, 2026-11-06): the AT2020USB+'s input against the SSL 2+, 120 s, `locked` at −74.7 ppm while the same device as a secondary output read +71.1 ppm, 0 frames missing. Recording after the converter and replay built (2026-11-07): the record is the words the engine read, after the converter (`signal-output/src/inputs.rs`), written as `inputs/<slot>.f32` and replayed bit-exactly (ADR 0038 §9, `session-format.md` §4b); device recording (silent, 2026-11-07): the AT2020USB+'s input against the SSL 2+ as clock, `signal play --capture voice@AT2020USB+=1 --record-session`, 15 s: the record complete (720 896 frames, its FNV-1a 64 checked on load), the microphone in it (99.9 % of samples nonzero after the adapter's 15.6 ms fill, peak −24.3 dBFS); `signal replay` of it identical on 1 and 4 threads, and refusing a frame past the recorded end. What the device played is not compared here: the run's output was at gain 0 (nothing audible), and a replay renders `main` only; that the replay feeds the record back bit for bit is `crates/signal-project/tests/session_inputs.rs`'s. Input hot-plug built (2026-11-07): a capture side from another device keeps its run source (`SwapSource`); the control thread hands it a fresh adapter when the device opens again and none when it went away (the run reads 0, counted missing, and records the 0s it read), polled with the secondary outputs (gone: stopped; back: reopened at once when the system's device list changes, else after 1 s doubling to 30 s). A device absent at the start gets its sources too. The clock device's own input goes with the run. Tested without a device (`capture_hotplug.rs`, no heap call in the audio thread's reads); silent device run: AT2020USB+ into the SSL 2+, 40 s, locked at −165 ppm, 0 missing, 0 dropped; an absent input reads silence and the run plays on. Not yet tested: unplugging and plugging the device back (needs the owner) |
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

## Phase 1 measurements (2026-11-01)
Phase 1 (`signal-output/src/adapter/`, offline) measured the design against the fake-clock harness and the converter tests; the figures are in `docs/benchmarks.md` "Clock adapter". Five changes, for the operator's checkpoint (proposed amendments, not yet accepted):
1. **The fill is linearised from host time stamps** (§2): `pushed at the last mark + rate × (pull time − mark time) − push / 2 − read position`, the raw fill only when a stamp is missing. The raw fill as §2 wrote it never entered track with the harness's jitter in 300 s (ratio jitter up to 453 ppm rms, slips with long device periods); the linearised one locks in every case of the harness. The stamps are the callbacks' own (cpal's `StreamInstant`); the loop still closes on fill.
2. **The slew limit and τ are per mode** (§2): 20 ppm/s and τ = 0.5 s hold in track only; acquire slews at 400 ppm/s with τ = 0.1 s. With 20 ppm/s in acquire a 100 ppm device locked after about 15 s and a 300 ppm one after 24 s to 3 minutes; τ = 0.5 s in acquire leaves the loop damped at about 0.24.
3. **Lock also needs a steady fill**: |e| ≤ 2 frames for 2 s *and* the fill moving at most 0.05 frames/s (about 1 ppm) over those 2 s, else a loop whose error never left the window entered track mid-transient.
4. **The reported latency** is F* − source period / 2 from a frame's own time (F* is the mean fill, which includes the converter's half length, its group delay; it is not added twice).
5. **Splices start while J of frames remain and are aligned** (§3): a repeat or a stop starts when a pull would leave fewer than J of frames beyond it, not when they run out (where the crossfade had no frames left), and every jump moves up to 5 ms further to where the material best correlates (WSOLA's rule), with gains summing to 1 for correlated material and equal power otherwise; a stopped producer gets an aligned jump back and a 5 ms fade-out from the history. The crossfade is 5 ms when the old head has the frames, else at least J.
   **Its cost is bounded** (review 2026-11-06, which measured a forced repair at 192 kHz, 128-frame callbacks and 64 channels at 19.2 ms against a 0.667 ms deadline): the alignment correlates a mix of at most 8 of the channels (`ALIGN_CHANNELS`), every `s`-th lag on every `s`-th frame within 8 192 products (`ALIGN_COARSE_BUDGET`), then every lag within `s` of the best; about 111 000 multiply-adds at 192 kHz whatever the channels, against 118 million for the full search (`AdapterOutput::align_multiply_adds`). The history is written a channel at a time (a skip moves up to 13 periods into it). Measured: the slowest pull around a repeat, a skip or a restart at 192 kHz and 64 channels 0.30–0.39 ms, an ordinary one 0.15 ms (`docs/benchmarks.md`). Not done: admitting a route against the callback budget (it needs the host's speed, an assumption).

Measured against §4 and the verification plan: every quality target met; the phase interpolation's error (−115.5 dB rms, −104.4 dB worst sum, at 512 phases) is below the 160-tap stopbands but about 5 dB above the 96-tap prototype's (−121.0 dB), so "below the stopband" holds for 160 taps only. Splice splatter (a sine): repeat −72 dB, skip −93 dB, stop −61 dB against a hard cut (target −40). J holds while callbacks run at most J plus half a source period later than their stamps; callbacks running before their stamps, or a jittered long-period producer, slip (the stress table). Not covered by phase 1: the rest of test 5 (unplug, time skip, ε step, sleep), the resync and clock-suspect states (§6), test 8.

## Answered questions (operator, 2026-11-01, under the owner's delegation of technical decisions)
1. **Jitter margin:** 1 ms to start; the adapter widens it by 1 ms (up to 3 ms) after a slip and records it in the device's status. Measured in phase 2.
2. **Alignment:** reported latencies only for now; a per-role alignment delay is a later option, if the owner asks.
3. **A clock-suspect device:** faded out.
4. **"Make this the clock device":** offered at the Setup level in phase 4 (it restarts the run on that device).
