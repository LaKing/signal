# ADR 0038: Devices as modules

- **Status:** Accepted (2026-10-23). The owner's words of 2026-10-23 (translated):
  - the request: "The next feature would be that the physical devices also get a module representation each, according to their capabilities. Best would be if, with several sound cards, each one appeared…";
  - answers to the draft's questions: (2) one clocked device in v1, several cards through an aggregate device, Signal's own drift adapters later: "enough, of course"; (3) a card that plays and records is **one box** with inputs and outputs;
  - on (1), where the routing lives, first "I can't decide yet; we can talk about it more", then, in the discussion with the operator: "It's not that simple. A project may be finished and then taken to another machine, to other hardware. We may publish it, and someone else may open it on entirely different hardware. … There are also audio grouping things: mono, stereo, quad, 5.1, 7.1 — someone with only a stereo system must be able to open a 5.1 project and hear it immediately." Then: "No Atmos etc. needed for now. LFE downmix … there could be some kind of adapters for that — which may be machine-specific."
  - The operator's model (the project describes layout and role, the machine describes its sound system, layout adapters between them, local overrides in `devices.local.toml`) was accepted by those answers. It replaces the draft's §4 (a `devices.toml` carried with the project, keyed by device identity), now an alternative (rejected). The rest is the operator's design, delegated.
- **Builds on:** ADR 0003 and vision §4.7 (the logical master clock follows the primary audio device; other devices attach through clock-domain adapters), 0008 (reproducibility from recorded external inputs; vision A7), 0009 (text is the truth), 0011 (cpal behind a driver), 0019 (workers in the device's workgroup), 0022 (output adapters; the engine knows sinks generically), 0025 (`.signal` files), 0035 §3 (sinks and taps in includes), 0036 (the device's clock is the run's; joining as it is; fades; read-only `device_props.rs`), 0037 (slots: declared ports, matching, empty slot).
- **Amends:** ADR 0022 (a run may have several audio sinks, and the audio adapter converts layouts; "a swap must keep the run's sinks" becomes "the adapter accepts or refuses the sinks a swap adds or removes"); `docs/project-format.md` §1 (one machine file, `devices.local.toml`, may lie in the folder; it is not a project file) and §9 (Setup: `clock`, `[play.<role>]`, `[capture.<role>]`); M4's "audio input (recorded and replayed)" is delivered through this ADR (M4.1).
- **Supersedes nothing accepted.**
- **Date:** 2026-10-23

## Context
- **Today:**
  - A project has exactly one device sink, `std.audio_out` (`l`, `r`, `gain`). The audio adapter in `signal-output` picks it, applies `gain`, and maps sink channel *i* to device channel `output_channels[i]` of the Setup's one `[output]` device.
  - The Setup's `[input]` is metered only (`signal-audio/src/input.rs`). Input never reaches the graph.
  - `signal devices` and `setup.devices` list every CoreAudio device with its channels, rates and identity (`DeviceRef`: name, id, signature; `select.rs` finds it again).
- **What the owner asks for:**
  - every physical device gets a module of its own, shaped by its capabilities, and with several sound cards each one appears;
  - a finished project moves to other machines and is published: whoever opens it, on whatever hardware, hears it at once;
  - channel groupings (mono, stereo, quad, 5.1, 7.1) are part of the music: a 5.1 project opened on a stereo system plays at once;
  - the fold-down (LFE included) is done by adapters, which may be machine-specific.
- **The tension:**
  - Devices belong to the machine. A project must stay portable and its text must be the truth (hard rule 4).
  - A project naming "SSL 2+ MKII ch 3–4" would not play on a laptop, its meaning would depend on the hardware, and publishing it would publish the author's studio.
  - What *does* belong to the music is its channel layout (5.1 is a composition decision) and the purpose of each output (the main mix, a click for the drummer, stems for a desk). Those travel; the hardware does not.
  - ADR 0037 already solved a similar problem: a slot is an instance with declared ports whose content comes from somewhere else. Here the "somewhere else" is the machine.
- **Physics:** every device has its own crystal. Two devices at a nominal 48 kHz differ by tens of ppm (assumption: typical crystal tolerance is ±20 to 50 ppm).
  - At 100 ppm relative, that is 4.8 samples per second, about 17,000 samples (0.36 s) per hour.
  - A fixed buffer between two devices with 128 frames of slack runs dry or overflows within about 30 s.
  - So two devices cannot be fed from one engine clock without adapting the rate.

## Decision

### 1. Terms
- **Layout:** the channel grouping of a device slot's play or capture side, by name. v1: `mono`, `stereo`, `quad`, `5.1`, `7.1`, and **discrete** (*N* named channels with no spatial meaning, e.g. stems). A layout is a name in `signal-project` and `signal-output`, never in the core or the engine (hard rule 1).
- **Role:** what an output or input is *for*, a free name `[a-z][a-z0-9_]*`: `main`, `monitor`, `click`, `stems`, `voice`… `main` is the default.
- **Device:** a physical (or virtual: BlackHole, an aggregate) endpoint of this machine, identified as the Setup identifies it (`DeviceRef`: id, else name and signature).
- **Device slot:** an instance in the project whose ports are declared by its layout (or its discrete ports) and whose content is **supplied by the machine**, not by `load`.
  - Its **inputs** are what the project plays (its `play` side, to the device's output channels).
  - Its **outputs** are what it captures (its `capture` side, from the device's input channels).
- **Sound system:** the machine's description, in the Setup, of where each role plays or captures: a device, channels, and the layout those channels form.
- **Layout adapter:** the conversion, in the audio output adapter (ADR 0022), from a project's layout to the sound system's layout for a role.
- **Clock device:** the one device whose callback runs the engine (ADR 0036 §1). It is the Setup's output device, the system default without one.
- Added to vision §3a as kinds of existing terms (Slot, Setup), not as new structural concepts.

### 2. Layouts: canonical channel names and orders
Ports are named after the channels, in this order. The order is the order of the channels in a render's WAV file, and it is the order of `WAVE_FORMAT_EXTENSIBLE`'s speaker bits, so the files are standard multichannel WAVs.

| Layout | Channels (port names, in order) | WAV channel mask |
|---|---|---|
| `mono` | `m` | `0x4` (front centre) |
| `stereo` | `l`, `r` | `0x3` |
| `quad` | `l`, `r`, `lb`, `rb` | `0x33` (front L/R, back L/R) |
| `5.1` | `l`, `r`, `c`, `lfe`, `ls`, `rs` | `0x60F` (front L/R/C, LFE, side L/R) |
| `7.1` | `l`, `r`, `c`, `lfe`, `lb`, `rb`, `ls`, `rs` | `0x63F` (front L/R/C, LFE, back L/R, side L/R) |
| discrete | the declared names, in declaration order | `0` (no speaker positions) |

- The UI shows the names in capitals (L R C LFE Ls Rs). `ls`/`rs` are always the side surrounds and `lb`/`rb` the back pair, so 5.1's surrounds keep their names in 7.1.
- `stereo`'s `l`, `r` are `std.audio_out`'s inputs, so the main output of every existing project is already a stereo layout.
- Atmos, height channels (5.1.2, 7.1.4…) and ambisonics are **later** (the owner: "No Atmos etc. needed for now"). The table is the extension point: a new layout is a row here and its downmix rows in §5.

### 3. A device slot in the project text: layout and role, never hardware

    [instances]
    surround = { device = { play = "5.1" } }                                 # role "main"
    fold     = { device = { play = "stereo", role = "main" } }               # the author's stereo version of main (§5.4)
    click    = { device = { play = "mono", role = "click" } }
    stems    = { device = { play = { kick = "audio", snare = "audio", pad_l = "audio", pad_r = "audio" }, role = "stems" } }
    mic      = { device = { capture = "mono", role = "voice" }, render = "fixtures/voice.wav" }
    card     = { device = { play = "stereo", capture = "stereo" } }          # one box: plays and records (main)

- **The `device` key** is an inline table (or a sub-table) with:
  - `play` (optional): a layout name, or a table of ports for a discrete layout (declaration order is channel order; every port is `"audio"`);
  - `capture` (optional): the same, for the captured side;
  - `role` (optional, default `"main"`);
  - `kind` (optional, default `"audio"`): which machine family can supply it. v1 accepts only `"audio"`; the other kinds are reserved for §11.
  - At least one of `play` and `capture` is required. One slot with both is **one box** (the owner's answer 3).
- **Ports** are audio signals at rate 1, named per §2. A port of another shape or rate is an error ("an audio device carries audio signals at the device rate"). A discrete port named like a layout channel is allowed; it has no spatial meaning.
- **Parameters:** the play side has `gain` (as `std.audio_out`: gain, [0, 4], default 1, combine multiply), applied by the adapter before the layout conversion. Nothing else in v1.
- **`render`** (optional, capture side only): a WAV project file whose channels feed the capture outputs in offline renders (§9). It is a project file (hashed, packed). Its rate must equal the project's `sample_rate` and its channel count the capture side's, else it is an error. Past its end the outputs read 0.
- **No templates are needed:** layouts replace the draft's `lib.stereo_out`/`lib.mono_in`/`lib.stereo_in` (hard rule 8). The interface is not editable like ADR 0037's: changing a slot's layout is an edit of its `play` or `capture` value, a live edit with a plan swap, and wires naming a port the new layout lacks are refused as ADR 0037 §2 refuses removing a wired port.
- **Reserved keys:** `device` and `render` join ADR 0037's list. `load`, `map` or `slot` on a device slot is an error.
- **Wires are checked against the ports,** whatever the machine, so the machine can never make a project invalid.
- **`std.audio_out` stays** as it is and **is** the device slot `{ play = "stereo", role = "main" }`: every existing project, golden and session is unchanged.
- **The project names no device, no channel number and no machine.** Nothing in the project files can.

### 4. The machine's sound system, in the Setup
The machine describes its sound system **once**, in the Setup (`project-format.md` §9), for every project:

    [output]                                   # the clock device, as today (`clock` is an alias)
    device = { name = "SSL 2+ MKII", id = "coreaudio:…", signature = "4 ch, 44100-192000 Hz" }

    [play.main]                                # what "main" means on this machine
    channels = [1, 2]                          # device: the clock device unless named
    layout   = "stereo"                        # default: from the channel count (below)
    adapter  = { lfe = -10 }                   # §5.2: LFE into the stereo fold-down at -10 dB
    prefer   = "author"                        # §5.4: "author" (default) or "adapter"

    [play.monitor]                             # headphones
    channels = [3, 4]

    [capture.main]
    channels = [1, 2]

    # another machine:
    # [play.main]
    # device   = { name = "Fireface UFX+", id = "coreaudio:…", signature = "…" }
    # channels = [1, 2, 5, 6, 3, 4]            # this studio's wiring, in 5.1's order L R C LFE Ls Rs
    # layout   = "5.1"

- **Per role and side:** `device` (default: the clock device), `channels` (device channels from 1, listed in the layout's canonical order, so any wiring is expressed by the list), `layout` (default from the number of `channels`: 1 `mono`, 2 `stereo`, 4 `quad`, 6 `5.1`, 8 `7.1`, else discrete), `adapter` and `prefer` (§5).
- **`output_channels`** stays as the short form of `[play.main] channels` and keeps its meaning exactly (sink channel *i* on device channel `output_channels[i]`), so today's Setups and `output_identity.rs` are unchanged.
- **With no configuration** a fresh machine plays at once:
  - `play.main` = the clock device (the system default output), channels 1–2 as `stereo` (channel 1 as `mono` on a one-channel device), unless the device's macOS speaker configuration (Audio MIDI Setup ▸ Configure Speakers, `kAudioDevicePropertyPreferredChannelLayout` and `…PreferredChannelsForStereo`, read in `device_props.rs` under ADR 0036 §5's read-only rule) names one of §2's layouts or another stereo pair, which is then used;
  - `capture.main` = the clock device's input channels 1–2 (`stereo`, or `mono` with one), else the system default input (an "other clock" in v1, §8, so it is reported and not captured).
  - Why not from the channel count alone (operator's decision): many interfaces have more outputs than speakers (the SSL 2+ has 4: speakers on 1–2, phones on 3–4), and "4 channels → quad" would send a quad project's surrounds to the phones. The count is used only where the person lists the channels.
- **Unknown roles are not heard; they never fall back to `main`** (operator's decision). A role the machine does not describe (`click`, `stems`) is computed and heard nowhere, and a notice says so with a one-click "play on …" that writes the role into the Setup or `devices.local.toml`. Why: two sinks on one channel are never summed (the owner ruled out host-level mixing, ADR 0035 §3), so a fallback would have to mix or to replace the main mix, and stems added to the main mix double it, and a click in the main mix leaks into a performance. One exception keeps "open and hear it at once": a project with no `main` output plays its **first** play role in canonical order as main, with a notice.
- **Unknown capture roles** fall back to `capture.main`, with a notice: capturing the same channels twice mixes nothing, and a published project's `voice` input then works on any machine with a microphone.
- **Two roles on one device channel:** never summed. The later in canonical order (role name, then slot address) is refused for that channel, reported at its entry, and stays silent there.

### 5. Layout adapters (output policy, ADR 0022)
Where the project's layout for a role differs from the machine's, the audio adapter in `signal-output` converts. Never in the engine, and never in renders (§9).

1. **Default downmix (`itu`):** the coefficients of ITU-R BS.775 (Table 2, the operator's reading of BS.775-3; phase 2 checks them against the published text before the tests pin them), with *k* = 1/√2 (`std::f64::consts::FRAC_1_SQRT_2`, 0.70710678…):

   | From → to | Matrix |
   |---|---|
   | `5.1` → `stereo` | L = l + *k*·c + *k*·ls; R = r + *k*·c + *k*·rs |
   | `5.1` → `quad` | L = l + *k*·c; R = r + *k*·c; Lb = ls; Rb = rs |
   | `5.1` → `mono` | M = *k*·l + *k*·r + c + 0.5·ls + 0.5·rs |
   | `quad` → `stereo` | L = l + *k*·lb; R = r + *k*·rb |
   | `stereo` → `mono` | M = *k*·l + *k*·r |
   | `7.1` → `5.1` | L, R, C, LFE as they are; Ls = *k*·ls + *k*·lb; Rs = *k*·rs + *k*·rb |

   - Every other downmix is the composition along 7.1 → 5.1 → stereo → mono (and quad → stereo → mono), computed once on the control thread: e.g. `7.1` → `stereo` is L = l + *k*·c + 0.5·ls + 0.5·lb. The composition 5.1 → stereo → mono equals the direct 5.1 → mono row, which a test checks.
   - BS.775 has no 3/4 (7.1) row: the `7.1` → `5.1` row is the operator's choice in BS.775's style (side and back pair folded at −3 dB each, power-preserving for uncorrelated content), labelled so in the UI and docs.
   - **No normalisation:** a downmix may exceed full scale (L + *k*·c + *k*·ls can reach 2.41). The output is clamped and the clips counted as today (`device_sample`), the status line says "clipping in the 5.1 → stereo fold-down", and the machine sets `trim` (below). The default is the exact matrix, so its output is testable to the bit.
2. **LFE:** not in the downmix by default (BS.775). `adapter = { lfe = <dB> }` adds 10^(dB/20)·lfe to L and R (to M for mono): `lfe = -10` is 0.31623. `lfe = "off"` is the default. A target layout with an LFE channel always receives it unchanged.
3. **Upmix: no invented surround.** Channels of the same name are copied; the target's other channels are exact 0. `stereo` on `5.1` plays on L and R; `5.1` on `7.1` leaves Lb, Rb silent; `quad`'s lb, rb go to Ls, Rs on `5.1` (which has no back pair); `mono` goes to C where the target has one, else to L and R at *k* each (so stereo → mono → stereo keeps the level of a centred source).
4. **The author's own fold-down.** A project may give a role several versions, one per layout (in §3, `surround` and `fold` are both `main`). The adapter then plays:
   - with `prefer = "author"` (default): the version whose layout equals the machine's; if none does, the largest version, converted by the machine's adapter;
   - with `prefer = "adapter"`: always the largest version, converted;
   - the other versions are computed and heard nowhere (as an unrouted role). Two versions of one role with the same layout are an error at plan time.
   - Why the author first: the author heard that mix; the machine's adapter is a generic matrix. Why the machine may override: it is the listener's room (a system with a good 5.1 → stereo fold-down of its own).
5. **Discrete layouts** are never mixed: channel *k* plays on the role's channel *k*. Channels beyond the role's are not heard, with a notice ("stems: 4 channels, this machine gives 2: pad_l, pad_r not heard"); a role with more channels leaves the rest at 0. A named layout meeting a discrete role (or the reverse) is mapped by position, with a notice.
6. **Machine-specific adapters:** in the Setup's `adapter` (per role) or `devices.local.toml` (per role, this project):
   - `adapter = { kind = "itu", lfe = "off" | <dB>, trim = <dB> }`: the default (`trim`, default 0 dB, after the matrix);
   - `adapter = { kind = "direct" }`: copy by name only, no fold-down (a channel the target lacks is not heard), for checking a mix;
   - `adapter = { kind = "matrix", from = "5.1", rows = { l = { l = 1, c = 0.7071, ls = 0.7071, lfe = 0.3162 }, r = { r = 1, c = 0.7071, rs = 0.7071, lfe = 0.3162 } } }`: a custom matrix keyed by channel names, for one source layout (several `matrix` adapters may be listed, one per `from`); unknown names, a missing `from` or a non-finite coefficient are reported and the `itu` adapter is used.
   - **Later:** `bass_manage` (a crossover, default 80 Hz, of every main channel and the LFE into a sub channel; the operator's choice not to build it before a machine with a sub needs it, hard rule 8), and a **machine-level composite or slot as an adapter** (a fold-down written as a patch). The latter runs a second graph outside the project's plan, with its own determinism and swap questions, and needs its own ADR.
7. **Real-time:** the control thread resolves each role's conversion into a fixed matrix (at most 8 × 8 for §2's layouts; discrete is a channel map) and hands it to the callback through the existing lock-free routing path. The callback multiplies and adds into preallocated buffers: no allocation, lock or I/O (`no_allocation.rs` covers it). The order per role is: `gain` → matrix → `trim` → clamp and ceiling → fade (ADR 0036 §4).
8. **The UI always says when a conversion happens**, on the device slot box, its card and the status line: "5.1 → stereo (ITU, LFE off)", "5.1 → stereo (ITU, LFE −10 dB)", "5.1 → stereo (author's stereo version)", "5.1 → stereo (matrix, this machine)", "stereo → 5.1 (L/R only)", "stems: 2 of 4 channels". No conversion shows nothing. `signal play` prints the same lines, and `signal play --json` and the UI protocol report them per role.

### 6. Per-machine, per-project overrides: `devices.local.toml`
- **What:** `devices.local.toml` in the project folder overrides the Setup's sound system for this project on this machine ("on this machine this project's stems go to 3–10"):

      [play.stems]
      device   = { name = "Fireface UFX+", id = "coreaudio:…", signature = "…" }
      channels = [3, 4, 5, 6, 7, 8, 9, 10]

      [play.main]
      adapter  = { kind = "itu", lfe = -10 }

  The same tables as the Setup's `[play.<role>]` and `[capture.<role>]`, merged over them key by key. Precedence: `devices.local.toml`, then the Setup, then the defaults of §4.
- **It is not a project file** (`project-format.md` §1): never planned, hashed, read by a render or by replay, and it never changes the plan. Deleting it loses only this machine's routing for the project.
- **Never packed, never published:** `signal pack` leaves it out (its whitelist already does: it is a root file outside §10's list; a test pins it), `signal unpack` drops one found in an archive made by another tool, with a notice, and `pack --embed`, `vendor` and sessions do not copy it (a session records the resolved routing as provenance, §9).
- **Git-ignored by default (operator's decision):** when Signal first writes `devices.local.toml` and the project folder is inside a git work tree (a `.git` found upward), it appends the line `devices.local.toml` to the project folder's `.gitignore` (creating it), unless that exact line is already there, and says so in the notice. The file format docs say the same for people who write it by hand. Why written and not only documented: the owner's concern is a project published by someone who never read the docs. The repository's own `.gitignore` gains the pattern, for `projects/`.
- **Device identity is safe here:** the file is local, so naming the author's devices leaks nothing. A copy synced to another machine (a shared folder) names devices that are absent there; those entries are ignored with a notice, never matched to another device (`select.rs`'s rules).
- **Written by Signal** with `toml_edit` (comments kept, atomic), read with diagnostics that never stop a project from opening.
- **CLI:** `signal play --route stems=<device>:3,4,5,6` and `--layout main=stereo` override for one run and write nothing.

### 7. What the plan is (hard rule 1)
Resolution happens in `signal-project` at plan time, like ADR 0037 §5:
- A play side with *N* channels becomes an instance of **`std.device_out<N>`**:
  - a sink of kind `SinkKind::Audio`, with signal inputs `c1…cN` and `gain`;
  - registered per width like the vector types (`vector.rs`; widths 1 to 64).
- A capture side with *M* channels becomes **`std.device_in<M>`**, a **source** (below) with outputs `c1…cM`.
- A slot with both becomes a synthesized composite at the slot's address containing `play` and `capture`, with exports named after the slot's ports. Host wires are renamed to `cK` through the exports, as ADR 0037 renames them.
- **Layouts and roles are not in the plan's types:** the plan knows widths. `signal-project` hands the layout and role of each sink and source (by address) to the output adapter beside the plan, as it hands the manifest. The same project text gives a byte-identical plan on every machine, with any Setup and any `devices.local.toml`.
- **The one addition to the core, domain-free: sources.** `ModuleDescriptor` gains `source: Option<SourceKind>`, the mirror of `sink` (ADR 0022).
  - The engine lists sources (`Engine::sources()`: instance, kind, address, signal outputs) and lets the driver write a source's outputs for a block through a `SourceInput` handed to `run_offline` and `Runner::process`.
  - The write happens in frame phase 1, "external input" (`architecture.md`), before emit and process.
  - The engine attaches no meaning to the kind. The oracle implements the same.
  - Accepted with this ADR (operator, delegated technical decision): it is domain-free and mirrors sinks.
  - `domain_scope.rs` gains `device`, `binding`, `clock`, `layout`, `role`, `lfe`, `downmix`, `5.1`; `signal-engine` still names no device library.
- **Includes (ADR 0035 §3):** a device slot of an included project is planned as its **tap**.
  - Play side: `std.device_out_tap<N>`, inputs passed through as outputs, exactly as `std.audio_tap`.
  - Capture side: `std.device_in<M>` becomes a plain pass-through with inputs `c1…cM`, which the host wires or leaves at 0.
  - So only the host's roles reach the machine, and an include's devices become ports the host wires (its layout shown on the ports).

### 8. Clocks: one clocked device in v1 (the owner's answer 2)
- **v1 rule:** a role can be played or captured only on the **clock device**, or on an **aggregate device** that *is* the clock device.
  - A role on another device is accepted as text but reported and not heard at run time: "Fireface: another clock; make an aggregate device in Audio MIDI Setup with drift correction, or wait for clock adapters (M6)".
  - The Devices view shows such devices as "other clock".
- **Input on the clock device** shares its crystal, so it does not drift. The input callback and the output callback are still separate. The adapter moves captured frames through an `rtrb` ring pre-filled by one period:
  - the engine reads a fixed, constant-latency stream;
  - a ring underrun gives zeros for the missing frames, counted as an input xrun, and is recorded as read.
- **Aggregate devices** are the user's documented path to several cards in v1: macOS resamples the non-master members with its own drift correction, and the aggregate appears to Signal as one device with all channels. Signal does **not** create aggregates in v1: that writes system state through `AudioHardwareCreateAggregateDevice`, which ADR 0036 §5's read-only `unsafe` module forbids.
- **Later (M6): clock-domain adapters** in `signal-audio`, outside the engine, as vision §4.7 and ADR 0003 foresee ("enough, of course"):
  - per secondary device, an asynchronous resampler steered by a delay-locked loop on the two devices' callback timestamps;
  - on outputs it sits after the layout adapter (output policy); on inputs it sits before the source fill, and the recording is taken after it;
  - so the graph's determinism is untouched, and only the secondary device's analog signal is adapted;
  - the cost is a few milliseconds of latency per secondary device and an ADR of its own.

### 9. Determinism: renders, sessions, replay
- **Offline there are no devices and no machine:** renders write each output in the project's own layout, and the Setup and `devices.local.toml` are never read. A render is the same on every machine.
  - **Outputs:** `audio.wav` stays `std.audio_out`'s (unchanged). Each other play side writes `devices/<slot>.wav` with its *N* channels in §2's order and the layout's channel mask (a 5.1 slot gives a 6-channel WAV), converted as `audio.wav` is. Every version of a role is written. The manifest gains `device_audio_fnv1a64 = { surround = "…" }` and each slot's layout and role, absent when there are none.
  - **Inputs:** the slot's `render` file if it has one (or `signal render --input mic=<wav>`, which the manifest records by hash), else exact 0.
- **Live input is non-deterministic, so it is recorded** (vision A7, ADR 0008), following the boundary-data precedent (`session-format.md` §4a):
  - `inputs/<slot>.f32` holds exactly what the engine read, f32 little-endian, interleaved, from frame 0 of the run, fades and underrun zeros included;
  - `session.toml` gains `[[inputs]]`: `slot`, `address`, `channels`, `frames`, `fnv1a64`, `complete`, and as provenance the device (`name`, `id`) and channels it captured from;
  - the recorder thread drains a ring the adapter fills (no I/O on the audio thread). If it falls behind, `complete = false` with a reason, and playing is never affected;
  - `signal replay` feeds the recorded words to the sources and is bit-exact. An incomplete input makes the replay refuse unless given `--allow-incomplete`, which plays zeros where the record is missing and says so;
  - the format version stays 1: the addition is optional, as M2.7-4's was.
- **Routing in sessions:** the resolved sound system (per role: device, channels, layout, adapter, the version played) is written into `[machine]` as provenance. It never affects replay: a replay renders the sinks' signals, which do not depend on routing or conversion.
- **Size:** 2 channels at 48 kHz in f32 is 384 KB/s, about 1.4 GB per hour (fact by arithmetic). The session card shows the size.

### 10. Capabilities, hot-plug and the Devices view
- **The inventory** (`signal-audio`): every device's channel counts in both directions, the rate it runs at, whether it is running somewhere (ADR 0036 §2), its macOS speaker configuration (§4), and **channel names** where CoreAudio reports them (`kAudioObjectPropertyElementName`, read in `device_props.rs`), else "1", "2", …. `signal devices --json` and `setup.devices` gain them. A new pushed message `devices` carries the inventory and, per device: present, clock, the roles on its channels, running, xruns, period.
- **The schematic gains a Devices level** (a sidebar list and a band at the schematic's edge):
  - every present device of the machine appears, so several sound cards each appear;
  - each shows its channels (capture on the left edge, playback on the right), marked free, used by which role (and layout), or the clock device's;
  - devices named by the Setup or `devices.local.toml` but absent are listed greyed as "not present".
- **Adding a device to the project:** dragging a device or a channel range onto the schematic writes a device slot whose `play`/`capture` is the layout those channels form (§4's count rule) or a discrete layout named from the channel names (sanitized: `in_1`, `analog_3`), with a new role, **and** that role's routing in `devices.local.toml`. Two text writes, one undo. Dragging a device onto an existing slot writes only the routing. The person chooses "this project" (`devices.local.toml`) or "all projects" (the Setup).
- **A device slot box** is a slot box with a device badge, its layout and role under the title, the device and channels it plays on ("main · 5.1 · SSL 2+ MKII 1–2") and the conversion line of §5.8, or "not heard on this machine" drawn hollow.
- **Panel:** a device slot gets a card like the slot card (ADR 0037 §7): role, layout, routing, conversion, per-channel meters, `gain`, and status (running, xruns, the shared/period note of ADR 0036). Status is shown, not wired.
- **Setup dialog:** the output device is labelled "Clock / main output"; a Sound system section lists the roles (device, channels, layout, adapter, prefer). A row links to the Devices view.
- **MCP:** `devices.list`; `device.route {role, side, device, channels, layout?, adapter?, scope: "project" | "machine"}` and `device.unroute {role, side, scope}`, which write `devices.local.toml` or the Setup. Creating a device slot is ADR 0037's instance write with the `device` key.
- **Hot-plug (ADR 0036, per device):** a device's arrival or departure never changes the plan (the ports are the project's); only the adapter's routing table changes, prepared on the control thread. Routing a role in fades its channels in over 10 ms (ADR 0036 §4's raised cosine), unrouting fades them out. A departing input reads 0 from the next block with a 64-frame fade. The clock device departing stops the run as today, and Signal.app's device wait (`signal-ui/src/host.rs`) plays again when it returns, re-resolving `main` (a fresh default when the configured device is gone). Detection in v1: the control thread polls the device list about once a second (`DEVICE_POLL`); a property listener is later, with its own `unsafe` review.
- **Joining:** each device stream decides its buffer with `plan_join` on that device's own state (ADR 0036 §2). Signal never switches any device's rate. Workers join the clock device's workgroup only (ADR 0019).

### 11. Other device kinds: the shape now, the code at M6 (hard rule 8)
Each kind is the same pattern: a `device = { kind = …, … }` slot with a role, a domain-free sink or source of a library type, an adapter in `signal-output` or `signal-audio`, the machine's routing per role in the Setup (and `devices.local.toml`), and live input recorded in the session.

| Kind | Ports | Machine endpoint | Recording | When |
|---|---|---|---|---|
| `audio` | audio signals in a layout | CoreAudio device + channels | `inputs/<slot>.f32` | v1 (this ADR) |
| `midi` | event ports (notes, CC) | CoreMIDI endpoint unique id | event lines stamped with effective frames, like `edits.jsonl` | M6 |
| `osc` | event ports by address pattern | host, port | as MIDI | M6 |
| `dmx` (Art-Net) | `{ vector = 512 }` at a rate divisor | IP, universe | outputs only, none | M6 |
| `screen` | visual sink | ADR 0032's monitor key | none | "visual output", later |

## Alternatives considered
- **Device names in the project file** (`out = { device = "SSL 2+ MKII", channels = [3,4] }`): simplest to read, but the project's meaning depends on the machine, it fails on any other Mac, and the plan would differ per machine (hard rules 3 and 4). Rejected.
- **A project-carried `devices.toml` keyed by device identity** (the draft's §4: ordered binding entries per slot, tried until a present device matches, packed in `.signal` files): the routing travelled with the project without changing its meaning, but it **leaks the author's machine** (device names, ids, wiring) into every published or shared project, it is useless on anyone else's hardware, and it has no answer for a 5.1 project on a stereo system. Rejected (the owner: "someone else may open it on entirely different hardware"). Its useful part, per-project routing on one's own machine, is `devices.local.toml`, which never leaves the machine.
- **A machine patchbay with role names only** (the project names roles, the Setup maps roles to channels, no layouts): portable and simple, but the channel count and meaning are then the machine's guess: a 6-channel `main` cannot be told from six stems, and nothing knows how to fold 5.1 to stereo. Rejected as incomplete; this ADR is that model plus layouts.
- **Unknown roles falling back to `main`:** "everything is heard", but it sums sinks on one channel (ruled out, ADR 0035 §3) or replaces the main mix, doubles a mix with its stems and puts the click in the performance. Rejected (§4); a notice with a one-click route instead, and a project without `main` plays its first role.
- **Layout inferred from the device's channel count when nothing is configured:** the owner's first sketch; kept where the person lists channels, but not for an unconfigured device, whose extra outputs are usually phones or line outs (§4).
- **Normalising downmixes** (scaling each matrix so that no output can exceed full scale): safe against clipping, but quieter than BS.775 by up to 7.6 dB for content that never clips, and no longer the standard matrix. Rejected as the default; `trim` per machine.
- **Folding down in renders** (a render of a 5.1 project also writes stereo): renders would depend on a fold-down policy, which is the machine's. Rejected; a fold-down the author wants rendered is an author's version (§5.4), written as `devices/<slot>.wav` like any output.
- **Device ports from capabilities** (the module grows ports when the device has them, with a plan swap on hot-plug): matches the owner's wording literally, but ties the project's ports to the hardware. Capabilities shape the **creation** of a device slot (§10) and the routing instead.
- **Status outputs in the graph** (`present`, `xruns`): machine facts that change the graph's behaviour; allowed only once recorded as inputs. Deferred until a project needs them.
- **Several clocked devices in v1 without adapters:** buffers drift until they break within about 30 s (§Context). Rejected.
- **Signal creating aggregate devices:** convenient, but it writes system state visible to other apps and needs a writing `unsafe` exception. Later, if the owner wants it, with its own ADR.
- **Feeding input through external events:** events are per target and sequence, not per-frame signals. Rejected for a domain-free source.

## Consequences
- **`signal-core`:** `SourceKind`, `ModuleDescriptor::source`. **`signal-engine`, `signal-oracle`:** `sources()`, the `SourceInput` fill in phase 1; `domain_scope.rs` words.
- **`signal-std`:** `std.device_out<N>`, `std.device_in<M>`, their taps; width registration; contract tests.
- **`signal-project`:** the `device` and `render` keys, layouts and their port names, roles and versions, resolution to sinks, sources and synthesized composites, the layout/role table handed beside the plan; `devices.local.toml` read and write and the `.gitignore` line; `pack`/`unpack` exclusion; sessions (`inputs/`, `[[inputs]]`, routing in `[machine]`), replay, manifest.
- **`signal-output`:** the layout table and BS.775 matrices, their composition, the `itu`/`direct`/`matrix` adapters, `lfe` and `trim`, multi-sink routing, per-role fade, ceiling and meters, the source fill from rings, `devices/<slot>.wav` with channel masks, the conversion lines.
- **`signal-audio`:** the inventory with channel names and speaker configuration, the fresh-machine default, several streams on the clock device, the input ring, polling for hot-plug.
- **`signal-app`, `signal-ui` and `ui/`:** the Setup's `[play.*]`/`[capture.*]` (read, written, the Sound system section), the `devices` message, the Devices view, device slot boxes and cards with the conversion line, the route dialog, MCP tools.
- **Docs:** `project-format.md` (§1 on `devices.local.toml`, §4 keys, a new section for device slots and `devices.local.toml`, §9 roles and `clock`, §10 the exclusion), `session-format.md`, `semantics.md` (sources in phase 1), `module-contract.md`, `ui-protocol.md`, `agent-interface.md`, `roadmap.md` (M4.1 "Devices and layouts", before M4).
- **The repository's `.gitignore`** gains `devices.local.toml`.
- **Goldens:** none change.

## Tests
1. **Portability:** the same project gives byte-identical plans with no Setup, with Setups of other layouts, with `devices.local.toml` absent, present and naming absent devices, and on a machine with other devices.
2. **Renders unchanged by machine:** a render of a project with 5.1, stereo and discrete slots is byte-identical with any Setup and any `devices.local.toml` (and neither file is opened: checked by a render with both unreadable). A 5.1 slot writes a 6-channel WAV with mask `0x60F`; `devices/<slot>.wav` hashes pinned. Determinism for *k* = 1, 64, 256 and threads 1, 2, 4, 8.
3. **Equivalence:** a project with only `std.audio_out` renders and plays byte-identically to before (`output_identity.rs`). A `{ play = "stereo" }` slot on a stereo `main` plays the same device samples as `std.audio_out` with the same sources.
4. **A 5.1 project on a stereo system** (null driver, scripted machine): with impulses on each of l, r, c, lfe, ls, rs in turn, the two device channels carry exactly the `itu` row's coefficients (L: 1, 0, *k*, 0, *k*, 0); the LFE impulse is silent by default and 0.31623 with `lfe = -10`; `trim` and clipping counted; the composition 5.1 → stereo → mono equals the direct row; every pair of §2's layouts converts as §5 says (upmix zeros included); a custom `matrix` applies exactly; a bad one falls back to `itu` with a diagnostic.
5. **Versions:** with an author's stereo version, a stereo machine plays it with `prefer = "author"` and the converted 5.1 with `prefer = "adapter"`; a 5.1 machine plays the 5.1 version; two versions of one layout are a plan error.
6. **Fresh machine:** with no Setup, `main` plays on the clock device's channels 1–2 as stereo (1 as mono on a one-channel device), or on the speaker configuration's layout when it reports one (scripted); an unknown play role is silent with its notice; a project without `main` plays its first role; an unknown capture role captures `capture.main`.
7. **`devices.local.toml`:** overrides merge over the Setup key by key; `signal pack` leaves it out and `signal unpack` drops it from a foreign archive; the `.gitignore` line is written once inside a git work tree and never outside one; it never changes a plan or a render (tests 1 and 2).
8. **Sources:** the engine and the oracle agree. Source fill and the layout matrices allocate nothing (counting allocator). Phase-1 ordering is covered by `semantics.md` tests.
9. **Sessions:** a null-driver run with a seeded input stream records `inputs/<slot>.f32`, and `signal replay` is bit-exact. An injected underrun and a device removal are recorded as zeros and replay exactly. An incomplete record is refused without the flag. The resolved routing is in `[machine]`.
10. **Routing resolution:** precedence (local, Setup, default), id-only device matching, channels out of range, an overlapping channel refused (never summed), "other clock" refused at run time.
11. **Hot-plug on the null driver with a scripted device list:** no plan swap, per-role fades exact, an input fading to 0.
12. **Device (SSL 2+ MKII and BlackHole 64ch):** 10 minutes with `std.audio_out`, a stereo `monitor` role on channels 3–4 and the mic input wired to a meter, while Chrome plays on the same device: 0 xruns, nothing heard to jump at the joins; the session replays exactly.
13. **UI:** the conversion line shown for every conversion and absent without one; dragging a device writes the slot and its routing; routing by menu; the hollow unrouted box; `signal ui check` passes.
14. **The owner's verdict:** a 5.1 test project opened on the stereo system is heard at once with the conversion shown; two sound cards both visible (one played through an aggregate device); a mic input recorded and replayed.

## Phases
| Phase | Delivers | Exit |
|---|---|---|
| 1. Text and offline | `device` slots with `play`/`capture` layouts and roles, versions, resolution, sources in core/engine/oracle, `std.device_out/in`, taps, `render` fixtures, `devices/*.wav` with channel masks, manifest; a 5.1 test project | Tests 1–3, 5 (plan side), 8; goldens unchanged |
| 2. The machine's sound system | Setup roles, the fresh-machine default, layout adapters (`itu`, `direct`, `matrix`, `lfe`, `trim`), the author's versions and `prefer`, `devices.local.toml` (merge, `.gitignore`, pack exclusion), multi-sink routing on the clock device, conversion lines in `signal play` and the status line | Tests 4–7, 10 |
| 3. Input and hot-plug | Inventory with channel names and speaker configuration, capture into the graph via the ring, session recording and replay, hot-plug polling | Tests 9, 11, 12 |
| 4. Devices view | Schematic Devices level, slot boxes and cards with the conversion line, drag-to-create and route, the Setup's Sound system section, `devices` message, MCP tools | Test 13; the owner's verdict (test 14) |
| Later | Bass management; a machine-level composite or slot as adapter (own ADR); Atmos, height layouts and ambisonics; clock-domain adapters for secondary devices (M6); MIDI, OSC, Art-Net kinds (M6); graph-visible status; maybe Signal-made aggregates | Own ADRs |

## Risks
- **The BS.775 coefficients** are written from the operator's reading; phase 2 checks them against the Recommendation's text before the tests pin them. The 7.1 → 5.1 row is Signal's own.
- **hound's channel mask** (assumption: hound 3 writes `WAVE_FORMAT_EXTENSIBLE` for more than two channels with a mask of its own choosing, which for 6 channels may be `0x3F`, back rather than side surrounds). If the mask cannot be set, phase 1 writes the header itself or records the deviation; the sample data are unaffected.
- **macOS speaker configuration** may be unset or wrong on many devices; the stereo default then applies, which is the safe failure.
- **Downmix clipping** with the exact BS.775 matrix on loud surround material. Mitigation: clip count, the status line, `trim`.
- **Session size** for long input recordings (about 1.4 GB/h for stereo). Mitigation: shown on the card; compression later.
- **Device identity** may change when a USB device moves port (assumption: the CoreAudio UID of some USB devices contains a location). `select.rs`'s name and signature fallback applies; the routing then shows "not found", never a wrong device.
- **cpal** opening input and output streams on one device: two callbacks with an unknown relative phase. The constant ring latency is assumed, not measured, and must be measured in phase 3.
- **Aggregate devices** misconfigured by the user (drift correction off) will glitch. Out of Signal's control, and said in the notice.
- **Writing `.gitignore`** touches a file Signal does not own. It only appends one exact line, once, inside a git work tree, and says so.
- **The core gains a concept (sources).** Domain-free, mirroring sinks; accepted with this ADR (§7).
