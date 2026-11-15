# ADR 0057: External I/O: OSC, MIDI, Art-Net and external sync (M6)

- **Status:** Accepted (operator, 2026-11-15, under the owner's delegation of technical decisions: "Minden eddigi megfogalmazott mérföldkő készüljön el")
- **Milestone:** M6, external I/O
- **Related:** ADR 0001 and 0004 (MIDI, OSC and DMX exist only as adapters; never in the core), ADR 0022 (output adapters), ADR 0038 §7, §9, §11 (device slots of other kinds, capture recorded after its converter), ADR 0016 (external input is the first source class), ADR 0033 (the playhead follows a position), `01-inception-proposal.md` §8 (M6)

## Context
M6: "MIDI/OSC input modules (logged); Art-Net output module; clock-domain adapter ADR and first adapter [moved to M4.1b, ADR 0052]; test project: audio-reactive DMX", accepted when "logged-input replay identical; Art-Net packet timing within spec", exited when "the PO runs a live session with external control and replays it".

ADR 0038 §11 fixed the shape: each kind is a device slot (`device = { kind = …, … }`) with a role, a domain-free source or sink of a library type, an adapter outside the engine, the machine's endpoint per role in the Setup, and live input recorded in the session. M4.1 built that path for audio: a capture side is a source whose outputs the driver writes block by block (`SourceInput`), recorded after its converter as `inputs/<slot>.f32` and replayed word for word (ADR 0038 §9).

## Decision

### 1. Control input is a capture side whose words are recorded (OSC, MIDI)
A control capture slot is a **source of signals** (not events), written by the driver as an audio capture side is, so its whole path, recording and replay included, is the one M4.1 built and tested:
- `device = { kind = "osc", capture = ["cutoff", "x", "y"], role = "control" }`: one output per written name, in order (the discrete layout's array form, ADR 0038 step 6), each the **last value received** for its address (`/<role>/<name>`, one float argument), 0 before the first. Planned as `std.control_in<N>` (outputs `c1…cN`) inside a composite exporting the names, as a discrete play side is.
- `device = { kind = "midi", capture = […], role = "keys" }` with names from a fixed vocabulary: `note_gate`, `note_key`, `note_velocity` (the last note held, last-note priority), `cc0` … `cc127` (0–1), `bend` (−1–1), `clock_position`, `clock_running`, `clock_tempo` (§4). Same planning.
- **Timing:** messages arriving on the control thread go through a lock-free ring into the source; the audio thread applies them at the start of the block it is computing (no frame inside a callback is promised: a network or USB message has no sample time). What the engine read is recorded, so a replay is bit for bit what was played, whatever the network did.
- **The machine's endpoint per role**, in the Setup (`[capture.<role>] osc = "0.0.0.0:9000"`, `midi = "<endpoint name>"`), as audio roles are bound; unbound, the outputs hold 0 with a notice.
- Offline (`signal render`), a control capture side reads 0, or a recorded file through `--input` as an audio capture side does.

### 2. Art-Net output is a sink of kind `Data`
- **Core, domain-free:** `SinkKind` gains `Data`: a sink whose inputs the driver hands to a non-audio output adapter. The engine attaches no meaning to it (as with `Audio`); `domain_scope.rs` still finds no DMX in the core.
- `device = { kind = "dmx", play = ["red", "green", "blue", "dimmer"], role = "lights" }`: one input per written name, in DMX channel order from 1, each 0–1 (clamped) → `round(255·v)`. Planned as `std.data_out<N>` (inputs `c1…cN`, at most 64 per slot, the discrete layouts' widths; a fuller universe is several slots) inside a composite exporting the names; inside an included project it is `std.device_out_tap<N>`.
- **Adapter** (`signal-output`, the "data adapter"): at every frame that is a multiple of `ceil(sample_rate / 44)` (1091 frames at 48 kHz: 43.996 Hz, under Art-Net's 44 Hz DMX refresh), the audio thread copies the sink's inputs into a preallocated ring; the control thread sends each as one **ArtDmx** packet (protocol 14, sequence 1–255, the universe from the binding) by UDP to the bound address (`[play.<role>] artnet = "2.0.0.1:6454", universe = 0`). Unbound, nothing is sent, with a notice.
- **Offline:** `signal render` writes the same frames to `devices/<slot>.dmx` (a header line, then one line per DMX frame: the frame number and the channel bytes in hex), so a test reads exactly what a run would send.
- **Timing within spec:** packets at most 44 per second per universe, each 18 + N bytes (N even, at least 2), sequence numbers in order; checked on the frames offline and on a run's send times (the control thread's, against the frames they carry).

### 3. MIDI through CoreMIDI, in an audited module
CoreMIDI is a C API with callbacks; Signal adds no dependency (rule 7), so it is reached by `extern "C"` declarations in **one audited module**, `signal-audio/src/midi_in.rs`, the fifth exception to rule 9 (AGENTS.md: an ADR and a single audited module; this ADR is the one). It only creates a client and an input port, connects sources, and copies each received packet's bytes into a lock-free ring from CoreMIDI's thread; parsing (running status, notes, CCs, bend, clock 0xF8, start 0xFA, continue 0xFB, stop 0xFC) is safe Rust on the control thread.

### 4. External sync: MIDI clock as a position
A MIDI clock (24 ticks per quarter note, start, stop, continue) is turned into `clock_position` (beats since start), `clock_running` (0/1) and `clock_tempo` (bpm from the tick interval, smoothed over a beat) on the MIDI capture slot. Wired to `std.playhead`'s `follow`, `follow_running`, `follow_tempo` (ADR 0033), the project follows the external clock; recorded with the rest of the slot, a replay follows it exactly. Sending clock, and Ableton Link, wait for a project that needs them.

### 5. The test project: `projects/light-show` ("blue on bass")
The studio's drums and bass into `main`; an envelope follower on the bass (`std.lag` of the rectified signal) drives `blue`, the kick's envelope drives `dimmer`, a slow LFO `red` and `green`, on a DMX slot of role `lights`; an OSC control slot (`brightness`, `hue`) scales them live. Offline its `devices/lights.dmx` is checked against the bass envelope; live, the person points a fixture (or an Art-Net monitor) at it.

## Phases
| Phase | Delivers | Exit |
|---|---|---|
| 1 | §2: `SinkKind::Data`, `std.data_out<N>`, `kind = "dmx"` slots, the data adapter, `devices/<slot>.dmx` offline, ArtDmx over UDP live | Offline frames and packet format tested; a live run sends to a local UDP socket and the packets are checked |
| 2 | §1 for OSC: `std.control_in<N>`, `kind = "osc"` slots, the UDP listener, recorded and replayed | A session with OSC messages replays bit for bit |
| 3 | §3, §4: CoreMIDI input, notes, CCs, bend, clock | A session with MIDI replays bit for bit; the playhead follows a MIDI clock |
| 4 | §5 and the owner's live session | The owner runs a live session with external control and replays it |

## Consequences
- Core: `SinkKind::Data` only (domain-free). `signal-std`: `std.data_out<N>`, `std.control_in<N>`. `signal-output`: the data adapter. `signal-audio`: the OSC listener and the CoreMIDI module. `signal-project`: the slot kinds. `signal-ui`/`signal-cli`: bindings, notices, `--input` for control files.
- AGENTS.md rule 9 lists `signal-audio/src/midi_in.rs` when phase 3 lands.

## Alternatives considered
- **Control input as events** (`Set` events on event outputs, ADR 0038 §11's sketch): exact frame stamps, but sources write signals today, and events from a source need a new engine path; a held value per block is what a control surface gives anyway. Rejected for now; events wait for a project that needs sample-exact external triggers.
- **Mapping external messages onto parameter `set` commands** (as the UI does): recorded as edits for free, but it changes the project's controls' base values (a file write on commit) and ties a surface to parameter names. Rejected: a role-bound slot keeps the project portable, as audio roles do.
- **A MIDI or OSC crate** (`midir`, `rosc`): a new dependency (rule 7). OSC is small enough to parse by hand; CoreMIDI needs FFI either way.

## Notes

**2026-11-15, phase 1 built: Art-Net output** (operator; delegated). §2 as written, with these readings:
- `SinkKind::Data` in `signal-core` (domain-free; the audio adapter still selects `Audio` sinks only, the engine hands every sink to the driver as before); `std.data_out1` … `std.data_out64` in `signal-std/src/data_out.rs`, their taps the discrete layouts' `std.device_out_tap<N>`.
- `kind = "dmx"` device slots: `play` must be an array of names (a named layout or a `capture` side is refused with the reason); the slot plans as the discrete slots' composite (`device:<slot>`) around `std.data_out<N>`, without `gain`. `signal_project::device::data_sides` hands each data sink with its slot, role and names beside the plan.
- The data adapter (`signal-output/src/data.rs`): offline, `signal render` writes `devices/<slot>.dmx` (a header, then a line per DMX frame, its frame number and the bytes in hex); live, `Runner::send_data(hop)` samples the data sinks into a preallocated ring (fixed 64-byte frames: nothing allocated on the audio thread), and an `ArtNetSender` on the control thread sends each as ArtDmx (Art-Net 4, OpCode 0x5000, protocol 14, sequence 1–255 per sink).
- **Binding:** this phase binds a role with `signal play --artnet ROLE=IP[:PORT][/UNIVERSE]`; the Setup's `[play.<role>] artnet` and Signal.app's runs come with phase 4's live session. A `dmx` role without a binding is not sent, with its notice.
- **Tests:** `crates/signal-cli/tests/dmx.rs` (the frames offline every 1091 frames, the levels as bytes, the refusals; a real-time null-driver run sending 44 packets in its second to a local UDP node, each laid out as the specification says, the universe and the sequence checked), `data.rs`'s unit tests (the hop, the bytes, the packet, the target).
