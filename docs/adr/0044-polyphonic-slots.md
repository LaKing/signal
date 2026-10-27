# ADR 0044: Polyphonic slots

- **Status:** Accepted (2026-10-27). The owner's answers of 2026-10-27 and the operator's decisions on the rest are recorded under "Decisions on the questions" at the end, and the owner's follow-up on dynamic polyphony in §11. Drafted the same day and revised after an independent review. The owner's words (translated): "at slots we could perhaps introduce the concept of polyphony." The owner approved the operator's outline for an ADR: a slot gets a `voices = N` property; when the slot is resolved (before planning, as every slot is, ADR 0037 §5) its content is instantiated N times; a voice allocator, a library primitive and not a core concept, distributes the notes to the voices; the voices' outputs are summed; parameters are shared. The design below is the operator's (technical choices delegated). The taste and scope choices were put to the owner as questions, each with a recommended answer.
- **Builds on:** ADR 0001 and hard rule 1 (no domain concepts in the core), ADR 0008 and 0016 (determinism, canonical order), ADR 0027 §4 (the library, pinning), ADR 0028 (patterns, `std.note_seq`), ADR 0029 (the MCP gain guard), ADR 0035 (projects as content), **ADR 0037 (slots)**, ADR 0039 (the `audition` note encoding), ADR 0041 (snapshots), ADR 0042 and 0043 (ranges and value sets), `semantics.md` §3, §4, §11, §12, §14 (sums, events, determinism, swaps and the added-instance fade, the panic exception).
- **Amends** (explicitly, applied to the docs with the phase that builds each part):
  - ADR 0037: §2 (reserved keys; `load`: no project content in a poly slot), §3 (port fields, templates), §5 (what the plan is), §7 (the slot card);
  - ADR 0028 §6 (`std.note_seq` gains the outputs `notes` and `dropped`).
  - `semantics.md` (a new §17 "Sleep groups"), `realtime-rules.md` and `module-contract.md`, proposed amendments for phase 5 (§11).
  - ADR 0039 is **not** changed: its encoding is reused as it is, and audition notes keep their rules (§3).
  - **Supersedes nothing.**
- **Date:** 2026-10-27

## Context
- **The use case:** chords and overlapping release tails. Today a slot with `lib.voice` loaded with `moog_voice` plays one note at a time: `std.note_seq` reduces its pattern to one voice (last-note priority, ADR 0028 §6), and the voice's release is cut by the next note's attack.
- **What already exists:**
  - **Pattern files already hold chords.** `kind = "notes"` allows overlapping notes on different keys; only two overlapping notes on one key are an error (`project-format.md` §12). Only the player is monophonic.
  - **A note as one event:** ADR 0039's `audition` value, `Set(128·key + v)` (`v` the velocity in 127ths, 1 to 127 a note on, 0 a note off of that key; a `Trigger` ends the held note whatever its key). It is defined in `signal-std` (`audition_value`), on an ordinary event port (`event_port`: kind `trigger`, no unit), not in the core.
  - **Slots resolve to ordinary instances** in `signal-project` before `build_plan`, so the core, the graph and the engine never learn the word "slot" (ADR 0037 §5).
  - **Vectors** `{ vector = N }` and `std.unpack<N>` at the registered widths 2, 4, 8, 16, 64, 256 and 1024 (`semantics.md` §16, `crates/signal-std/src/vector.rs`).
  - **Composite exports fan out:** an exported input or parameter may name a list of inner ports (`project-format.md` §3). An exported output names exactly one inner port.
  - **`std.adsr` attacks from its current level** and `std.osc` keeps its phase across a frequency change (their rustdoc; `adsr.rs` test AC4b). `moog_voice` multiplies its loudness contour by `velocity` (`velamp`).
- **The constraint:** polyphony is a music concept. Like the transport (ADR 0033), it must live in the library and the project format, never in `signal-core`, `signal-graph` or `signal-engine` (hard rule 1). Its output must stay identical for every *k*-partition and thread count (hard rule 3), with the one exception the engine already has: a panic in a `process` call of several frames acts from that call's first frame (`semantics.md` §14). Every partition claim below carries that exception.

## Decision

### 1. `voices = N` on a slot; nothing changes without it
```toml
[instances]
melody = { module = "std.note_seq", pattern = "chords" }
synth  = { slot = "lib.poly_voice", load = "moog_voice", voices = 8, set = { volume = -9 } }
lead   = { slot = "lib.poly_voice", load = "moog_voice",
           voices = { count = 1, legato = true } }        # the mono legato mode

[wiring]
connections = [
  { from = "melody.notes", to = "synth.notes" },
  { from = "synth.out", to = "echo.in" },
]
```
- **`voices`** (optional) is an integer from 1 to **64** (`MAX_VOICES`), or a table:
  - `count`: the integer;
  - `assign`: `"rotate"` (the only value in phase 1; `"lowest"` is deferred, §10);
  - `steal`: `"oldest"` (the only value in phase 1; `"none"` is deferred);
  - `legato`: `false` (default) or `true` (§4);
  - `sleep`: `true` (default) or `false`, from phase 5 (§11). Before phase 5 the key is an error ("not yet").
  Any other key or value, or a count outside 1 to 64, is an error at the value.
- **A slot without `voices` is exactly ADR 0037's slot.** Every `voice` field of its interface (§2) is ignored, its plan is byte-identical to today's, and every golden is unchanged.
- **`voices = 1` is a polyphonic slot of one voice.** It is not the mono slot: it has an allocator and its voice is at `/synth/v0` (§5). With `legato = true` it plays the way `std.note_seq`'s mono outputs do, within the limits test 3 states.
- **Where:** wherever a slot may appear (ADR 0037 §2): `project.toml` and project composites. A poly slot inside a composite has that file's N, the same for every instance of the composite.
- **Content:** a module (`std.x`, a composite, `lib.x`). **A project is refused as the content of a poly slot** in v1, at `load`: a project's instance is a scope root whose streams do not depend on its address (ADR 0035 §4), so N copies of it would draw identical noise. No use case needs it (hard rule 8). This amends ADR 0037 §2's `load`.
- **`voices` is a reserved key**, added to ADR 0037 §2's list. It is an error on a plain instance (`module =`, `project =`). Extending it to plain instances later is the same resolution, if someone asks for it (Alternatives).

### 2. The interface declares each port's role: `voice = "shared" | "each" | "sum" | "alloc"`
A new optional **port field `voice`**, in an inline interface and in a template alike (amends ADR 0037 §3 and `project-format.md` §14). It is read only when the slot has `voices`.

| Role | On | Default for | Meaning in a slot with N voices |
|---|---|---|---|
| `shared` | inputs (signal, vector, events) | every input | The host's wire reaches the matched port of **every** voice (broadcast). Events: each voice gets its own copy of every event. |
| `each` | signal inputs | — | **One value per voice**, fed by the slot's allocator output **of the same name** (phase 1: `hz`, `gate`, `velocity`; `key` and `spread` are deferred, §10), element *e* to voice *e*. The host cannot wire it (an error at the wire: "`synth.hz` is fed by the slot's allocator"). |
| `sum` | signal outputs | every output | The voices' outputs are **summed** into the slot's output (§5). |
| `alloc` | one events input | — | The **allocator's input**: the note events (§3) the slot distributes among its voices. It is matched to no content port. |

- **Parameters are always shared:** a parameter's value, its `set` and `ramp` events and its modulation reach every voice. A `voice` field on a parameter is an error. Variation from voice to voice comes from `each` inputs inside the content (§6), never from per-voice parameter values.
- **Errors**, located at the field or the wire: `each` on an output or an events input; `each` with a name the allocator does not output; `alloc` on anything other than an events input; two `alloc` ports; `sum` on an input.
- **Phase 1 refuses, as "not yet",** events and vector outputs in a poly slot (a merge or a vector sum, when a project needs one), and a host-fed `each` input (a vector `{ vector = N }` from the host, distributed element by element; it needs `std.unpack<N>` at every N, see Alternatives). Hard rule 8: no project needs these yet.
- **Warnings:**
  - a slot with `voices` whose interface has no `each` and no `alloc` port: "every voice of `synth` gets the same inputs";
  - an `alloc` port in a slot without `voices`: "`synth.notes` feeds nothing: the slot has no voices";
  - a slot with `voices` and the template `lib.voice` or `lib.instrument`: "did you mean `lib.poly_voice`?".
- **`map`** works as in ADR 0037 §4, the same for every voice. `map = { hz = "freq" }` renames toward the content. `map = { velocity = false }` leaves every voice's `velocity` input unconnected, so it reads 0, and the allocator's `velocity` goes unused; that suits only a content that treats a 0 there as its fallback (`moog_voice` would be silent, since it multiplies by velocity). A `map` key naming the `alloc` port is an error ("`notes` feeds the slot's allocator; it has no content port").
- **Matching** is ADR 0037 §4's, done once against the content, since every voice is the same content. The `alloc` port takes no part in it.
- **Mono content is the normal case:** each voice is one copy of a mono module (`moog_voice`). Stereo content sums each channel separately (§10: `lib.poly_instrument` is deferred, but an inline interface with `l` and `r` works in phase 1).

**A new library template** (amends ADR 0037 §3's table, pinned by text hash with a `CHANGELOG.md` entry under the current MAJOR.MINOR, as every template is):

| Template | Inputs | Outputs | Matches |
|---|---|---|---|
| `lib.poly_voice` | `notes` (events, kind `trigger`, `alloc`), `hz` (frequency, Hz, `each`), `gate` (gate, `each`), `velocity` (control, 0 to 1, `each`) | `out` (audio, `sum`) | `moog_voice` with no `map` |

`lib.voice` and `lib.instrument` keep their text and their hashes: a template is never changed incompatibly (ADR 0037 §3), and new roles are a new name. `lib.poly_instrument` (the stereo one) waits until a project needs it (§10).

### 3. How notes reach a poly slot: note events, with a poly output on `std.note_seq`
- **The note event convention** (library, `module-contract.md`): ADR 0039's `audition` value, unchanged, named as the convention for every note stream. `Set(128·key + v)` with `key` an integer from 0 to 127 and `v` an integer: from 1 to 127 a note on at velocity `v/127`, 0 a note off of that key. A `Trigger` ends every note the receiver plays from that port. Anything else is ignored.
  - The port is an ordinary event port, exactly as `audition` is: `event_port`, kind `trigger`, no unit. **No new `PortKind` and no new unit**, since a kind is a core type (hard rule 1). `signal_std::audition_value` is the one encoder.
  - Velocity on this path is **quantised to 127ths**, as with MIDI. One event per note keeps a note atomic (ADR 0039, Alternatives). A pattern velocity of 0.8 arrives as 102/127.
- **`std.note_seq` gains an event output `notes`** (amends ADR 0028 §6; additive):
  - It carries **every** pattern note, chords included: each note's on at its start and its off at its end, by the same tick or follow rules that place the mono notes. There is no last-note priority on this output.
  - **Within a frame**, the offs come first, then the ons, each group in key order. So a voice freed on a frame can be given a note on that frame (§4).
  - A note that starts and ends within one frame is not emitted. It would not sound at frame resolution, and the mono gate is low at the end of that frame too.
  - **A seek and a stop** (the cases where the mono outputs end their note, ADR 0033) emit an off for **each pattern key sounding on `notes`**, never a `Trigger`. So a held audition key keeps sounding through a seek, as ADR 0039 §2 says for the mono outputs.
  - **Audition notes** (ADR 0039) are added to this output as notes of their own: the pattern goes on, so keys can be played over it as chords. A key that sounds from both the pattern and an audition gets one on. Its off is sent when both have ended.
  - **A live pattern edit** (a new table at a swap frame, ADR 0028 §4): every key sounding on `notes` keeps its own end (its remaining ticks, or its end beat when following), independent of the new table, as the mono sounding note does. Notes start from the new table from the swap frame on.
  - **At most 16 events per frame** (`semantics.md` §4): `notes` never overflows. A frame whose events would exceed 16 (more than 8 keys changing at a loop boundary, or a fast forward scrub) is reduced to its end state: one `Trigger`, then an on for each key still sounding at the frame's end, audition keys included, at most 15, latest start first and then highest key.
    - The reduction **re-attacks the notes that were sustained across that frame** (their voices take a new on). This is accepted: it is rare, it happens only on a frame that would otherwise fault the sequencer, and it keeps every surviving note sounding.
    - Every note left out is counted on a new signal output **`dropped`**: a count since `prepare` that **saturates at 2¹⁹ − 1** (524 287), well below the fault guard's 10⁶, as `std.count`'s `lost` does.
  - The mono outputs (`gate`, `key`, `hz`, `velocity`, `on`, `off`) are unchanged. An unconnected `notes` changes nothing, so every golden is unchanged.
- **How an existing mono sequencer drives a poly slot:** it is the same `std.note_seq`. Wire `melody.notes → synth.notes` in place of `hz`, `gate` and `velocity`. A pattern without overlaps then plays the same melody, with each note's release left to ring on its own voice.
- **Other sources:** anything that emits note events, such as another sequencer, or a future MIDI adapter (M6). Several sources wired into one `alloc` port merge in canonical order (`semantics.md` §4).
  - A gate-and-key signal source (an arpeggiator built from primitives) needs a converter `std.gate_notes`. It waits until a project has such a source (hard rule 8).
- **Pattern format:** unchanged. The poly sequencer is the existing format played without priority.
  - `std.step_seq8` stays as it is: drum lanes are separate voices already.

### 4. The allocator: `std.voice_alloc`
A direct primitive in `signal-std`, category `event`, at divisor 1. It is **one type at the fixed width `MAX_VOICES` = 64**, so it survives a live change of N (§8).

- **Input:** `notes` (an event port, §3).
- **Parameters**, set from the slot's `voices` entry when the plan is built, each with a range or a value set (ADR 0043):
  - `voices`: 1 to 64, read rounded;
  - `legato`: 0 off, 1 on;
  - (deferred, §10: `assign` and `steal`).
- **Outputs**, each a vector of 64, where element *e* is voice *e*:
  - `hz`: `440·2^((key − 69)/12)`, through the same function as `std.note_seq`'s `hz`, so the values are bit-identical. It is 0 before the voice's first note, so `moog_voice`'s first-note glide rule works per voice;
  - `gate`: 0 or 1;
  - `velocity`: `v/127`.
  - `hz` and `velocity` hold after a note ends, so a release rings at its own pitch and level.
- **Voice states.** Each voice *e* < N is in one of three states:
  - **free**: its output gate is 0, and it plays no note;
  - **sounding**: its output gate is 1, and it plays a key;
  - **claimed**: it has been given a new note on this frame while it was sounding (a steal or a retrigger, below). Its output gate is 0 on this frame. Internally it already holds the new note: its key, velocity and `order`. On the next frame it becomes sounding with that note.
  - `order` is the value of a counter that goes up by one for each event the allocator applies, recorded at the voice's last on or off. A voice never used has `order` 0.
  - **Only a free voice counts as free.** A claimed voice is never given another note as a free voice, so a chord's later notes on the same frame cannot overwrite it.
- **Each frame**, claimed voices first become sounding (their pending start). Then the frame's events apply in their canonical order on the port (`semantics.md` §4), one after another:
  1. **Note on, key *k*, already sounding or claimed** on voice *e*: a **retrigger** of *e* (below), with the new velocity. With `legato` on, the velocity changes and the gate stays high.
  2. **Note on, a free voice exists:** `rotate` takes the free voice with the **smallest `order`**, the one released longest ago, so recent release tails ring on. Ties go to the lower index. The voice becomes sounding on this frame.
  3. **Note on, no free voice:** `oldest` **steals** the sounding or claimed voice with the smallest `order` (the oldest note on).
  4. **Note off, key *k*:** the voice playing *k* takes gate 0, becomes free and records its `order`. On a claimed voice this cancels the pending start, so its gate stays 0. An off for a key that no voice plays (a stolen note) is ignored. A `Trigger` releases every voice in the same way.
- **The off of a stolen note, precisely.** Voice *e* plays key *a* and is stolen for key *b* on frame *n*:
  - from that moment the voice plays *b*. An off of *a* on frame *n*, later in that frame's order, or on any later frame is ignored: *a* is gone and never comes back, as in ADR 0028;
  - an off of *b* on frame *n* cancels the pending start, so *e* is free at the end of *n* and *b* never sounds;
  - an off of *b* on frame *n* + 1 applies after the pending start. The gate is 0 at the end of *n* + 1, so *b* sounds for no frame.
  - The rule is the same for every partition (the state moves frame by frame), so the result is too.
- **Retrigger and steal: one frame with the gate low.** A voice whose gate is high and that is given a new note is claimed. Its gate is 0 on that frame. From the next frame on its gate is 1 with the new note's `hz` and `velocity`.
  - The envelope therefore sees a new rising edge. It attacks **from its current level**, and the oscillators keep their phase (fact for `std.adsr` and `std.osc`).
  - **This is not fully continuous.** `moog_voice` multiplies its loudness contour by `velocity`, so a steal (or a legato change) between notes of **different velocities steps the amplitude** by the ratio of the two velocities at the envelope's current level. With equal velocities the output is continuous. Test 9 measures the step, so the owner can listen (decision 1).
  - The cost is one frame of delay (10.4 µs at 96 kHz), on steals and retriggers only. A note given a free voice starts on its own frame.
  - With **`legato` on**, a voice whose gate is high takes the new note on that frame without the gap and without being claimed: gate stays high, and `hz` and `velocity` change, as `std.note_seq`'s last-note priority does. `legato` with `voices = 1` is the mono legato mode.
- **When N falls** (the `voices` parameter, §8), the voices at or above the new N take gate 0 and are no longer assigned. Their `hz` and `velocity` **hold** (they are not reset), so a voice releases at its own pitch whichever frame its suspend lands on.
- **When N rises** (from M to N), elements M to N − 1 are **reset** on the frame the new value is read: gate 0, `hz` 0, `velocity` 0, `order` 0, and no key. So a voice added again starts as a new voice does (the first-note rule of `moog_voice`'s glide), and `rotate` takes these voices first.
- **Determinism:** the result depends only on the events in canonical order and on the parameters. There is no time, no randomness, and no dependence on arrival or schedule. Ties are broken by index and by the event counter. The state moves one frame at a time inside `process`, so a block of *k* frames gives exactly the frames one by one (the contract suite checks *k*-partition invariance; the panic exception of `semantics.md` §14 applies as to any primitive). It is one instance, so the thread count cannot matter.
- **Real time:** fixed arrays (64 voices, a 128-entry key-to-voice table), no allocation, no locks. Each event does a bounded scan of at most 64 voices.

### 5. What the plan is: an ordinary instance of a generated composite (hard rule 1)
- **Resolution** stays in `signal-project`, before `build_plan` (amends ADR 0037 §5). A loaded poly slot `synth` with N voices becomes an ordinary `InstanceDef { name: "synth", module: <generated definition> }`. The **generated composite** is built in memory from the slot entry and the content, and no file holds it:

  | Inner instance | Type | What |
  |---|---|---|
  | `alloc` | `std.voice_alloc` | its parameters from `voices` |
  | `each_<port>` | `std.unpack64` | one per `each` port: `alloc.<port>` → `in`; `e<e>` → voice *e*'s matched port |
  | `v0` … `v<N−1>` | the content | the voices |
  | `sum_<port>` | `std.add` | one per `sum` output: every voice's matched output → `a`; `b` unconnected |

  - **Exports:**
    - a `shared` input exports the list of every voice's matched port (fan-out);
    - the `alloc` port exports `alloc.notes`;
    - a `sum` output exports `sum_<port>.out`;
    - every interface parameter exports the list of every voice's matched parameter;
    - every content parameter outside the interface does the same under its content name, so `set` keys and live edits reach them.
    - `alloc`'s parameters are **not exported**.
  - **Values:** interface parameter values and `set` values are values on the instance `synth`. They reach every voice through the exports, as for any composite.
  - **The host's wires** on the slot's ports become wires on these exports by name.
- **The definition's name** is deterministic and cannot collide with any written module. It is `poly:<host definition>/<slot>`, for example `poly:project/synth`, or `poly:pad_rack/synth` for a slot inside the project composite `pad_rack`.
  - `:` and `/` are outside the module-name grammar (`[a-z][a-z0-9_]*`, with the prefixes `std.`, `lib.`, `core.`), so no file can name it.
  - N is not part of the name. Survival across a change of N is decided per primitive instance, by address and type (`semantics.md` §12), so the name does not affect it.
  - Wherever a plan dump or `describe` records a composite definition's name, this is the name it records, and test 2 compares the dumps with that one name replaced by the hand-written composite's.
- **The sum uses `std.add`**, with every voice wired into `a` and `b` left unconnected, which reads +0:
  - a signal input is the canonical sum of its connections and starts from +0 (`semantics.md` §3), so `a` is never −0 and `a + 0` is exact;
  - its kinds are `generic`. Like every core-set type, its output is clamped to ±10⁶, which a sum of 64 bounded voices never reaches;
  - it gives the summed output **one port**, for meters, panel bindings (`/synth.out`), restricted inputs (ADR 0024), `is_connected`, and the host wire's `scale`, which then applies once to the sum;
  - it is needed because an exported output names exactly one inner port (`project-format.md` §3). That rule is not changed, and no new type is added (Alternatives).
- **Addresses** follow the address grammar (`[a-z][a-z0-9_]*` segments, so not `/synth#3`): `/synth/v3/osc1`, `/synth/alloc`, `/synth/each_hz`, `/synth/sum_out`.
  - Content names never collide with these, because the content lives one level down under `v<e>`.
  - The byte-wise canonical order of a receiver's sources (`/synth/v0`, `/synth/v1`, `/synth/v10`, …, `/synth/v2`) is fixed by the text, which is all determinism needs. Displays sort voices numerically.
  - Every source inside the slot shares the prefix `/synth/`, so the slot's place among the host's other sources is the same as for a mono slot.
- **Random streams:** each voice's primitives draw `derive(seed, /synth/v<e>/…)` (`semantics.md` §11), so every voice has its own noise. Voices are not scope roots. Adding voices does not change the streams of the voices that stay.
- **The plan is byte-identical to the explicit form:** a project composite holding the same instances, wires and exports, instantiated as `synth = { module = … }`, up to the definition's name. So canonical order, fades, faults, converters, `is_connected` and the oracle need nothing new.
  - **In phases 1 to 4, `signal-core`, `signal-graph` and `signal-engine` learn nothing.** No "voice", no "poly". Phase 5 adds the domain-free sleep group (§11).
  - `domain_scope.rs` gains the spellings `voice_alloc`, `VoiceAlloc`, the `poly:` definition prefix, and the resolution's type names phase 1 chooses (for example `SlotVoices`). It cannot check the plain word `voices`, which the engine already uses generically (`tasks.rs`, `fn voices` in its tests), just as ADR 0037's note could not check `slot`.
- **An empty poly slot** contributes no instance and no connection, the allocator included (ADR 0037 §5).
- **The side table** `SlotResolution` gains `voices` (count, `legato`, and the voice addresses) for `describe`, the schematic and the panels. The plan has no record of it.

### 6. Parameters, modulation, snapshots and ranges
- **Shared by default, and only shared:** a panel control `/synth.cutoff`, a `set`, a ramp and a modulation wire into `synth.cutoff` reach every voice equally, through the export fan-out.
- **Per-voice variation** comes from `each` inputs inside the content. The designed mechanism, deferred to when a sound needs it (§10), is an allocator output `spread`: −1 for voice 0 to 1 for voice N − 1, `(2e − (N − 1)) / (N − 1)` (0 for N = 1). A voice with a `spread` input could multiply it by a shared `detune_spread` parameter into its oscillators' cents, or pan by it. Per-voice velocity and key tracking already work inside the content, from `velocity` and `hz`.
- **Glide is per voice.** Each voice glides from its own previous note (polyphonic portamento), and a voice's first note starts on pitch. With `legato` on and one voice it is the mono glide of today (decision 5: the owner chose per-voice glide).
- **Not controls:** `voices` and `alloc`'s parameters. They are structure written in the slot entry, and `alloc`'s parameters are not exported (§5). So `/synth/alloc.voices` is not in `describe.controls`, cannot be set from the panel or by MCP's `set_parameter`, and is in no snapshot. Writing `voices` is the one way to change it.
- **Snapshots (ADR 0041)** are unchanged: a poly slot's controls are `/synth.<name>`, one value each. A restore is still one parameter edit, never a swap, and it does not change N.
- **Ranges and value sets (ADR 0042, 0043):** a shared export's targets are N copies of one content parameter, so its range or value set is the content's, unchanged. The allocator's parameters declare theirs (§4). `voices` outside 1 to 64 is an error in the file, not a correction, because it changes structure.

### 7. Schematic, panel, `describe`, MCP
- **Schematic:**
  - a poly slot is one box with its interface's ports and a **×N badge** ("synth ×8 ◂ moog_voice");
  - the `alloc` port is marked, and `each` ports show as fed inside;
  - semantic zoom opens the generated level, **read-only** (it is not text): the allocator, the unpacks, the sums and the N voice boxes;
  - a voice box opens into the content. That is the one shared definition, so an edit there (where the content is editable at all, ADR 0037 §7) is an edit of every voice;
  - ADR 0030's trace runs from `notes` through `alloc` and the element *e* of an unpack to voice *e*, and from the voices through `sum_<port>` out;
  - the `graph` message's `slot` gains `voices: { count, legato }` (additive, `ui-protocol.md`).
- **Panel:** **one slot card**, as ADR 0037 §7, plus:
  - "×N" in its header;
  - a voices stepper (1 to 64) and the `legato` switch, which write the slot's `voices` entry with `toml_edit` (text first). A change is a live edit (§8);
  - a **voice row**: N dots, each lit while its voice's gate is high, and a mark on a faulted voice (below). The dots observe `/synth/alloc.gate` through the vector adapter, so the row takes **one of the run's 8 vector-stream slots** (`ui-protocol.md`). When none is free, the row shows "no slot" and the rest of the card works;
  - the `out` row's meter is `/synth.out`, the sum. Per-voice meters are `/synth/v<e>.out`, shown when the voice row is opened, and they use scope slots as other meters do.
  - The content's own cards fold under the slot card **once**: their controls are the shared `/synth.<name>`.
- **Faults:** the guard mutes the **instance** that faulted, as always (`semantics.md` §7, §14). That is one primitive of one voice, never the slot.
  - The faulted voice goes on being assigned notes, which it plays partly or not at all. The allocator cannot know, since a fault is not a signal (decision 4).
  - The slot card marks the voice, and `describe` lists the fault under its voice's address.
  - A swap that replaces the voice clears it: a Replace, an unload and load, or an N that drops that voice.
- **`describe`** lists, per poly slot, `voices` with its voice addresses, the roles of its ports, and its match table (once, since all voices share it).
- **MCP:**
  - A new patch op and tool, `slot.voices {slot, count?, legato?}` (`agent-interface.md`). It writes the host's files only, as `slot.load` does, and the change is a live edit there too.
  - **ADR 0029's gain guard judges it** like any patch, by its rules R1 and R2, against `--max-gain`. A change of N changes the fan-in into every gain the slot's output reaches. Under R2 (a modulated gain) it is therefore refused unless `--max-gain` is 4. Under R1 (a static gain) it is judged by the gain's base, which N does not change.
  - The output ceiling holds whatever N is, as it holds for every project.

### 8. Live edits: changing N, Replace, the fade
A change of `voices` is a project edit like any other, applied by `diff.rs` and `live.rs` as one or two swaps (`semantics.md` §12):
- **N grows (M → N):**
  - the swap adds `v<M>` … `v<N−1>`, and their `audio` outputs fade in over F = 5 ms (the added-instance fade; they are silent, gate 0, until a note comes);
  - `live.rs` sends `set(voices = N)` to the surviving `alloc` at the swap frame, which resets elements M to N − 1 (§4);
  - `alloc`, `each_*`, `sum_*` and `v0` … `v<M−1>` survive, with their state, so held notes and release tails play on bit-continuously;
  - **shared parameters of the added voices** are brought to the survivors' live state. An added instance takes the new plan's values, which are the text's, while the survivors keep their live bases (`semantics.md` §12, "What survives"). So `live.rs` sends the added voices, at the swap frame, a `set` of each shared parameter's current live base wherever it differs from the text, and for a ramp in progress a `ramp` to its target over its remaining frames. The added voices then match the survivors' values, exactly at the ramp's end but not bit for bit during it: the restarted ramp interpolates from the value at the swap frame. These sends are recorded in the session like every edit, so a replay reproduces them.
- **N shrinks (M → N):** the removal protocol of `semantics.md` §12.
  - `set(voices = N)` to `alloc` and a suspend of `v<N>` … `v<M−1>` are requested for the same frame *s*. Their effective frames may differ if one of them is late, and that does not matter: the allocator holds `hz` and `velocity` for the released elements (§4), so nothing steps, whichever lands first.
  - At *s* + F the swap removes the voices; their audio is +0.0 from *s* + F − 1.
  - Their notes are not moved to the remaining voices: moving a note mid-way would restart its contours. A release tail is cut by the 5 ms fade, which is glitch-free.
- **`voices` added to or removed from a slot** (mono ↔ poly) moves the content to other addresses (`/synth/…` ↔ `/synth/v0/…`), so it is a **Replace**, unload then load (ADR 0037 §6).
- **Replace or a changed `load` in a poly slot** is ADR 0037 §6's unload then load. The slot is empty in between, so the allocator leaves too and nothing carries over.
- **`legato`** is a parameter edit of `alloc`, with no swap.
- **The plan cost of a change** is the voice instances added or removed. Real-time rules hold as for any swap: everything is prepared off the audio thread, and committing allocates nothing.

### 9. CPU budget and limits
- **Cost:** N times the content, plus one allocator, one unpack per `each` port (3 in phase 1) and one `std.add` per summed output, all small.
- **Estimate** (an assumption, not a measurement): `moog_voice` is 39 of the studio's 177 instances, which together are about 8 % of a core at 48 kHz (`docs/benchmarks.md`). So a voice is about **1.7 % of a core at 48 kHz** (8 % × 39/177), and about twice that at 96 kHz.
- **Parallel work:** voices share no state, so they are independent units that ADR 0019's scheduler can spread over the workers. Polyphony is the best case for the multi-core engine. Phase 1 measures it (`signal bench`, `docs/benchmarks.md`), and phase 4 on the device.
- **`MAX_VOICES` = 64:** the allocator's width, and a registered `std.unpack64`. 64 voices of `moog_voice` are about 2 500 instances, about 1 % of `MAX_PLAN_INSTANCES` (`semantics.md` §16, "Limits"), and nested poly slots (N × M) stay under the same plan limits. A project that needs more than 64 voices needs a new allocator width, which is a change of library, not of the core.
- **The fixture uses 8 voices.** No default N exists: `voices` is always written.

### 10. Phase 1 is minimal; designed but deferred
Phase 1 builds what `projects/studio-poly` needs (hard rule 8): `voices` with `count` and `legato`, the roles, `lib.poly_voice`, `std.voice_alloc` with `rotate`, `oldest` and the outputs `hz`, `gate` and `velocity`, and `note_seq.notes`. The following are designed in this ADR so that adding them changes nothing above, and wait until a project needs them:
- `assign = "lowest"` (the lowest free index);
- `steal = "none"` (drop the new note) and the allocator's `dropped` count (saturating at 2¹⁹ − 1, as `note_seq.dropped`);
- the allocator outputs `key` and `spread` (§6), and their names as `each` inputs;
- `lib.poly_instrument` (the stereo template: `lib.poly_voice`'s inputs, `l` and `r` summed);
- host-fed `each` vectors, events and vector outputs in a poly slot (§2);
- `std.gate_notes` (§3);
- `voices` on plain instances (Alternatives).

### 11. Sleeping voices: N is a maximum, and only sounding voices cost (phase 5)
The owner asked (2026-10-27): "lehet a polifóniát dinamikusan kezelni?" ("can polyphony be handled dynamically?"). Of the options offered, the owner chose: "Felső korlát van, de csak a szóló hangok fogyasztanak. – ez jól hangzik" ("There is an upper limit, but only the sounding voices consume. That sounds good."). So `voices = N` stays the maximum, fixed per plan, and a voice that is not sounding is not computed. Growing N automatically was rejected (Alternatives). This section is the design of a later phase. Phases 1 to 4 compute every voice.

**A generic mechanism: the sleep group** (domain-free, in `signal-graph` and `signal-engine`; hard rule 1).
- **What it is:** a plan may declare **sleep groups**. Each has:
  - **members:** every instance under one address prefix (an instance subtree);
  - **a driver:** one signal output of an instance outside the group;
  - **a rest length** *R* in frames.
- It knows frames, ports, instances and exact zero, nothing else. The words are "group", "sleep", "wake", "driver" and "rest". There is no voice, no note and no gate.
- **The poly slot resolution** declares one group per voice: members `/synth/v<e>`, driver element *e* of a new allocator output `active` (a vector of 64), and *R* = 1024 frames (10.7 ms at 96 kHz, 21.3 ms at 48 kHz).
  - The driver needs a scalar port, so `active` goes through an `each_active` unpack like the other per-voice outputs.
  - `active` is 1 while a voice is sounding or claimed (§4), and 0 once it is released or free.
- **Validation** when the plan is built: the driver's instance is outside the group, so it is computed before the group in every frame. Groups do not overlap. Any other plan is refused with a named reason.
- **The slot's text:** `voices = { count = 8, sleep = true }`. `sleep` defaults to **true** from the phase that delivers it, so the owner's choice is the default. `sleep = false` computes every voice, as phases 1 to 4 do.

**Semantics, per frame** (proposed for `semantics.md`, as a new §17 "Sleep groups"):
- A group is **awake** or **asleep**. It starts awake, and a group added by a swap starts awake.
- **Waking.** An asleep group wakes on frame *n* when either:
  - the driver's value at *n* is ≥ 0.5, or
  - an event is delivered at *n* to an **event input** of a member. Events to parameters do not wake it (below).
  It is awake from frame *n* on, and its members are called for *n*. The driver is computed before the group, so this is known in the same frame.
- **Falling asleep.** An awake group falls asleep at a frame *g* that is a multiple of *R* (counted from frame 0 of the run, like the ticks of `semantics.md` §16) when, for every frame of [*g* − *R*, *g*):
  - the driver's value was < 0.5;
  - no event reached a member's event input;
  - every **boundary output** was exactly zero (+0.0 or −0.0). A boundary output is a member's signal or vector output read by an instance outside the group.
  It stays asleep from *g* until it wakes.
- **While asleep, the members' time stops**, as under suspension (`semantics.md` §6):
  - they are not called, and their primitive state is frozen;
  - their boundary outputs read **exactly +0.0**. Readers cannot tell this from the ±0.0 the group was writing, because every input is a sum that starts from +0 (`semantics.md` §3);
  - their inner outputs hold their values, and only frozen members read those;
  - they emit no events;
  - the fault guard has nothing to check.
- **Parameters keep time while asleep**, unlike under suspension. Parameter events (`set`, `ramp`) apply to the members' bases without waking the group, and ramps advance with the frames, since a ramp's value is a closed-form function of its frame count (`semantics.md` §5). So a voice that wakes has exactly the shared parameter bases the awake voices have. Signal modulation is read when the voice is called, as always.
- **Suspension and sleep compose.** An instance is called only if it is neither suspended nor asleep. A suspend, a resume or a removal (§8) acts on a sleeping member as on any instance. Its audio fade starts from its output as readers see it, which is +0.0.
- **Swaps.** A group that is in both plans, with the same members prefix, driver and *R*, keeps its state (awake or asleep, and how long its outputs have been zero) at the swap frame. So an unrelated live edit does not wake the voices or change when they fall asleep. Other groups start awake.

**Determinism.**
- Whether a group sleeps on a frame depends only on signal values, events and frame numbers. There is no wall clock, no load measure and no thread timing.
- The rule is stated frame by frame. The engine **splits a group's calls** at the frames where the state can change: at wake frames, and at multiples of *R* while the group is awake with its driver below 0.5. A split never changes an output (the module contract, *k*-partition invariance). So the output is identical for every *k*-partition and thread count, with the panic exception of `semantics.md` §14.
- The semantic oracle implements the per-frame rule independently, and the determinism matrix covers it.
- Renders are reproducible: `sleep` is in the text and the groups are in the plan, so the same files give the same audio, offline and in real time, and a session replays it.

**Bit-identical to never sleeping? No, and that is the deliberate choice.**
- While a voice sleeps its oscillators' phases do not advance. A voice that wakes therefore starts its next note from another phase than a voice that never slept. So `sleep = true` and `sleep = false` give different, equally reproducible renders.
- **Bit identity was the preferred goal, and it is not achievable with a saving.** It would need every member to be at an exact fixed point (calling it would change neither its state nor its outputs). `moog_voice`'s oscillators run whenever `hz` is non-zero, so they are never at rest, and its ladder sits before the VCA and filters them all the time. A fixed-point rule (a primitive-level `at_rest` contract, Alternatives) would let such a voice sleep never, and would save nothing.
- **Clicks are impossible from sleeping itself:**
  - a group falls asleep only after its boundary outputs have been exactly zero for *R* frames, and it then outputs exactly zero;
  - waking resumes the whole group from its frozen state at once, as if no time had passed for it, and that state is one whose outputs were exactly zero. So the first woken frame is exactly what an awake voice would compute from that state with the new inputs. For `moog_voice` that is an attack from the envelope's exact 0 (`std.adsr` flushes below 1e-9 to exactly 0).
  - A content whose output never reaches exact zero (a DC offset, a never-ending feedback tail) simply never sleeps. It costs what it costs today, and the slot card shows its voices as never asleep.
- **What a project author sees:** a sleeping voice sounds like a voice whose free-running oscillators were paused while it was silent. The voices' phases on a new note follow from the text and the notes alone, as any other state does.

**Real-time rules** (proposed for `realtime-rules.md`):
- **No allocation:** each group's state (awake flag, frames of zero output, the next check frame) is created in `prepare`.
- **Bounded cost:** waking reads one driver value per frame. Falling asleep compares each boundary output with zero, per frame and only while the group is awake with its driver below 0.5, which is O(boundary outputs × frames). For a voice that is one output.
- **No other change:** no locks, no I/O, and nothing on the audio thread depends on load.
- **The worker budget** (`realtime-rules.md` "The worker budget", `docs/benchmarks.md`) sees a load that changes with the number of sounding voices. The budget's estimates are per unit, so a sleeping unit costs only its check. The schedule may change between blocks, and the output does not, as for any schedule (`semantics.md` §13).

**The module contract** (proposed for `module-contract.md`): primitives need **nothing new**.
- A sleeping member is simply not called for some frames, as a suspended one is already. A primitive may never assume that its calls cover every frame of the run.
- The existing contract suite already checks that suspension frames are not seen. A new check gives a resting state and a gap of frames, and confirms the output after the gap equals the output with no gap.

**Measurement and checks** (tests 13 to 15): 16 voices of `moog_voice` with 2 sounding cost about **2 voices plus the overhead** of 14 sleeping groups. The overhead target is under 10 % of one voice, measured with `signal bench` and recorded in `docs/benchmarks.md`. A device run confirms it.

## Alternatives considered
- **A voice concept in the engine** (the engine clones an instance N times, or keeps voice-indexed state): a domain concept in the core, against hard rule 1, and a second runtime model next to the plan. Rejected. Resolution gives the same behaviour from ordinary instances.
- **A dedicated poly container instance** (`poly = { module = …, voices = 8 }`): a second container concept beside slots. A slot already has what polyphony needs: an interface where each port can declare its role, and resolution before planning. Rejected.
- **Polyphony inside composites** (writing N voices and an allocator by hand): it works today and remains possible, and it is exactly the explicit form a poly slot plans as (§5). But N and the wiring are then repeated by hand and cannot change live without rewriting the composite. Kept as the reference that test 2 compares against.
- **`voices` on plain instances** (`module = …, voices = 8`): the same resolution with an interface derived from the module. Deferred, not rejected. The slot is the boundary where instruments are swapped, so it is where polyphony is first needed.
- **The allocator in the host, wired by hand** (`alloc = { module = "std.voice_alloc8" }` into vector inputs of the slot): visible, but N is then written twice, and a width per N means a change of N replaces the allocator and loses its held notes. The generated allocator writes N once, keeps one type at width 64 and survives N changes (§8).
- **Vectors of hz, gate and velocity from the host** (`{ vector = N }` slot inputs, element *e* to voice *e*): generic and domain-free, and the role `each` already names it. Deferred to the first project that computes its own per-voice signals. It needs `std.unpack<N>` at every N, made by name on demand as converters are (`signal_core::converter`), or the vector widths extended.
- **A poly sequencer that allocates** (vectors of voices out of `std.note_seq`): it would tie a pattern to a voice count and put stealing where the voices are unknown. Allocation belongs to one place, the slot, which knows N.
- **Summing by a fan-in export** (an exported output naming several inner outputs, summed at each receiver): it changes the composite format and `signal-graph`, and leaves no single port for meters, bindings, restricted inputs and the wire's scale. Rejected.
- **A new `std.sum` identity type** (the first draft): `std.add` with everything into `a` is exact for a canonical sum (§5) and adds no type to the core set. Rejected.
- **Retriggering through `std.adsr`'s `trigger` event input, with no gate gap:** `std.adsr` already restarts its attack on a `Trigger` while its gate is high, in any stage, which would avoid the one-frame gap. Not used, for three reasons:
  - the allocator's per-voice outputs are vectors of signals, and a vector cannot carry events, so a per-voice trigger would need 64 event outputs on the allocator and an event route per voice;
  - `lib.voice` and `moog_voice` have no trigger input, so every voice would need a new port and wiring, and a content without one would not retrigger at all;
  - the gap costs one frame (10.4 µs) on steals and retriggers only.
  If the owner hears a problem with the gap, a voice interface with an optional `retrigger` event input can be added, fed by such an event route.
- **Exact velocities in two events** (key, then velocity): ADR 0039 chose one event per note so that a note is atomic. 127ths are MIDI's resolution and inaudible as a difference. Rejected.
- **A `Trigger` on seek and stop** (the first draft): it would end held audition notes, against ADR 0039 §2. Offs of the pattern's sounding keys only (§3).
- **Voices made on demand at run time:** allocation on the audio thread, and a graph that changes without a plan swap. Both are forbidden. N is fixed per plan.
- **Stealing with a short fade before the new note** (2 to 5 ms late): a classic choice, but it moves the note late. It was offered to the owner. The operator chose the immediate retrigger, for now (decision 1).
- **Per-voice parameter values** (a parameter role `each`): parameters are scalar, and a per-voice base would be N saved values under one control, which snapshots, panels and ranges would all have to learn. Per-voice variation through `each` inputs (`spread`) needs none of that.
- **Loudness scaled by N** (1/N or 1/√N inside the slot): a single note would get quieter as N grows, and changing N live would change the level of what plays. Rejected; the owner chose the raw sum (decision 6).
- **Growing N automatically when every voice is held** (one of the options for the owner's "dynamic" question): rejected, recorded with the owner's choice.
  - A new voice is a plan rebuild and a swap, prepared on the control thread. That takes tens of milliseconds, so the note that needed the voice would sound late.
  - And it would sound at a frame that depends on how fast the machine built the plan, which is wall-clock time in the semantics, against hard rule 3: the same session would not render the same way twice.
  - A fixed maximum with sleeping voices (§11) gives the CPU saving without either problem.
- **Bit-identical sleep through a primitive-level `at_rest` contract** (a primitive declares when calling it would change neither its state nor its outputs, the engine skips it then, and the contract suite checks that a resting `process` is a no-op): exact, but free-running oscillators are never at rest. A `moog_voice` would therefore never sleep, and nothing would be saved (§11). Kept as a possible later refinement for contents that do come to an exact rest, such as an effect whose input is silent. It composes with sleep groups.
- **Sleep decided by load or by a timer:** it would depend on the machine and on wall-clock time. Rejected by hard rule 3.
- **Addresses `/synth#3/…`:** `#` is outside the address grammar, whose byte order canonical sums rely on (`signal-core` `address.rs`). `/synth/v3/…` needs no change.

## Consequences
- **`signal-core`, `signal-graph`, `signal-engine`, `signal-oracle`:** no change in phases 1 to 4; `domain_scope.rs` gains the new spellings (§5). Phase 5: sleep groups in the plan (`signal-graph`: declaration and validation), their per-frame rule and call splitting in the engine, and the same rule in the oracle. All of it is domain-free.
- **`signal-std`:**
  - `voice_alloc.rs` (`std.voice_alloc`);
  - `note_seq.rs`: the `notes` and `dropped` outputs;
  - the note event convention documented beside `audition_value`;
  - contract suite and unit tests.
- **`signal-lib`:** `interfaces/poly_voice.toml`, with a pinned text hash, a `CHANGELOG.md` entry and `tests/pinned.rs`.
- **`signal-project`:**
  - `model.rs`: `voices`, the `voice` port field, reserved keys;
  - `slot.rs`: roles, the generated composite and its name, diagnostics, `SlotResolution::voices`;
  - `diff.rs` and `live.rs`: N changes as in §8, with the parameter sends to added voices;
  - `structure.rs`, `patch.rs` (`slot.voices`), `schema.rs`.
- **`signal-ui` and `ui/`:** the ×N badge and the read-only generated level, the slot card's voices controls and voice row, the MCP tool and its gain-guard check.
- **Docs, on acceptance:**
  - `project-format.md` §14 (`voices`, `voice`, the generated composite);
  - `module-contract.md` (the note event convention, `std.voice_alloc`, `note_seq.notes`, `lib.poly_voice` under "Interfaces");
  - `00-vision.md` §3a (the Slot row: "may be polyphonic, `voices = N`");
  - `ui-protocol.md`, `ui-controls.md`, `agent-interface.md`, `benchmarks.md`, `roadmap.md`.
  - `semantics.md` needs no change in phases 1 to 4: every behaviour is an existing rule. Phase 5 adds §17 "Sleep groups", and also amends `realtime-rules.md` and `module-contract.md` (§11).
- **Goldens:** none change. New: `projects/studio-poly`, a copy of studio-transport with `synth` as a `lib.poly_voice` slot of 8 voices playing a chord pattern from `melody.notes`, and its golden.

## Tests
1. **Nothing changes without `voices`:** every golden, every plan dump and every ADR 0037 slot test is byte-identical; an unconnected `note_seq.notes` changes no render.
2. **Equivalence with the explicit form:**
   - a poly slot's plan dump is byte-identical to a hand-written project composite's with the same allocator, unpacks, sums, voices and exports, as `synth = { module = … }`, once the one definition name (`poly:…`) is replaced by the hand-written one's (§5);
   - both render bit-identically for *k* = 1, 64, 256 and threads 1, 2, 3, 4, 8, through the offline driver and the real-time runner (null driver).
3. **One voice, legato,** with `voices = { count = 1, legato = true }` fed by `melody.notes`, on fixtures with **no note that starts and ends within one frame and no audition notes**: the studio melody and edge cases (overlaps, touching notes, wraps, a seek, a stop, a live pattern edit).
   - The voice's `hz` and `gate` equal `std.note_seq`'s mono outputs bit for bit, and `velocity` equals them rounded to 127ths.
   - With a pattern whose velocities are multiples of 1/127, `moog_voice`'s audio equals the mono slot's bit for bit.
   - **Why the claim is narrowed:** a note within one frame takes the mono voice (last-note priority) and ends it, while `notes` does not emit it (§3). An audition note takes the mono voice and keeps the pattern's notes from sounding while held (ADR 0039 §2), while on `notes` it is a note of its own, which a one-voice allocator lets the next pattern note steal. Both differences are by design, and test 5 checks them separately.
4. **Allocator against a brute-force reference** (a per-frame model in the test, on seeded random note streams):
   - `rotate`, `oldest` and `legato`;
   - a same-key retrigger, `Trigger`, ties at one frame;
   - **claimed voices**: a chord on the frame of a steal never overwrites the claimed voice, and the off of the old key and of the new one on frames *n* and *n* + 1 behave as in §4;
   - N falling (held `hz` and `velocity`) and rising (reset elements);
   - the contract suite on `std.voice_alloc`: *k*-partition invariance (with the panic exception), no allocation in `process`, bounded outputs.
5. **`note_seq.notes`:**
   - chords from the pattern, offs before ons, a note within one frame not emitted;
   - seek and stop as offs of the pattern's sounding keys only, with a held audition key still sounding;
   - audition notes added, a shared key's off only when both have ended;
   - **a live pattern edit while chords are held**: each sounding key keeps its remaining ticks (and its end beat when following), whatever the new table holds;
   - the 16-event reduction (re-attacked sustained notes, audition keys kept) and `dropped`, saturating at 2¹⁹ − 1;
   - unconnected changes nothing.
6. **Diagnostics,** located by line and column:
   - errors: a count outside 1 to 64, an unknown `voices` key or value (deferred values included in phase 1), a project `load`, the role errors of §2, a host wire into an `each` port, events and vector outputs and host-fed `each` as "not yet", a `map` on the `alloc` port;
   - warnings: no `each` and no `alloc` port, an `alloc` port without `voices`, the "did you mean `lib.poly_voice`";
   - `voices` on a plain instance is an error.
7. **Live,** on the null driver and the runner, with seeded callback sizes and threads 1 and N, at every phase of a held note and of a release:
   - N grows and shrinks: survivors bit-continuous, added voices fade in as in §12, removed voices +0.0 from *s* + F − 1, held notes on surviving voices play on;
   - a shrink where `set(voices)` and the suspend land on different frames gives no step;
   - **added voices after live edits**: a shared parameter edited live (not saved) and a ramp in progress reach the added voices with the survivors' values (exact at the ramp's end);
   - mono ↔ poly and a changed `load` are Replaces; a `legato` edit needs no swap;
   - no allocation on the audio thread (counting allocator); a recorded session replays bit-identically.
8. **Determinism matrix** on `projects/studio-poly`: *k* = 1, 64, 256 × threads 1, 2, 3, 4, 8, offline and real time; the golden.
9. **Steals, measured** (a measurement, not a gate): the largest frame-to-frame step of a stolen `moog_voice` against the same note played on a free voice, **with equal and with differing velocities** (for example 0.3 stolen by 1.0, and the reverse), and the same for a legato change, reported for the owner's listening.
10. **UI:**
    - the `graph` voices field and the ×N badge;
    - the read-only generated level;
    - the slot card's stepper and switch write the expected text and survive a reload;
    - the voice row lights as the gates do, and shows "no slot" when the 8 vector slots are taken;
    - `/synth/alloc.voices` absent from `describe.controls` and refused by `set_parameter`;
    - `slot.voices` over MCP, and its refusal under R2;
    - `signal ui check` passes on `studio-poly`.
11. **Device:** 10 minutes of `studio-poly` at 16 voices with N changed every 30 s (8 ↔ 16) and a Replace every 2 minutes: 0 xruns. The CPU per voice is recorded in `docs/benchmarks.md`.
12. **The owner's listening verdict** (hard rule 6).
13. **Sleep semantics** (phase 5), on fixtures and against the oracle:
    - a group falls asleep only at multiples of *R*, after *R* frames of exact zero with its driver low;
    - it wakes on its driver and on an event to an event input, but not on parameter events, whose ramps keep advancing;
    - boundary outputs are +0.0 while it sleeps;
    - suspension, removal and swaps compose as in §11;
    - the determinism matrix (*k* = 1, 64, 256 × threads 1, 2, 3, 4, 8) on `studio-poly` with `sleep = true`, and its own golden;
    - the contract suite's new gap check on every `std` primitive;
    - no allocation on the audio thread.
14. **Sleep CPU** (measurement): 16 voices with 2 sounding cost about 2 voices plus less than 10 % of one voice, recorded in `docs/benchmarks.md`.
15. **Sleep on the device:** 10 minutes of `studio-poly` at 16 voices with `sleep = true`, chords coming and going: 0 xruns, and the measured CPU following the sounding voices.

## Phases
| Phase | Delivers | Exit criteria |
|---|---|---|
| 1. Offline | `voices` (`count`, `legato`), the `voice` field, roles and diagnostics; the generated composite; `std.voice_alloc` (`rotate`, `oldest`; `hz`, `gate`, `velocity`); `note_seq.notes` and `dropped`; `lib.poly_voice`; `describe`; `projects/studio-poly` and its golden; a CPU measurement | Tests 1–6 and 8 (offline); every golden unchanged |
| 2. Live | N changes with the parameter sends to added voices, mono ↔ poly and Replace in a poly slot, sessions and replay | Tests 7 and 8 (real time) |
| 3. UI | The ×N box and the generated level, the slot card's voices controls and voice row, `slot.voices` over MCP with the gain guard, `ui-protocol.md` fields | Test 10; the operator's visual check |
| 4. Device and verdict | Device run and measurement | Tests 9 and 11; the owner's verdict (test 12) |
| 5. Sleeping voices | §11: sleep groups (graph, engine, oracle), the allocator's `active` output, `sleep` in `voices` (default true), the amendments to `semantics.md`, `realtime-rules.md` and `module-contract.md`, the slot card's asleep dots | Tests 13–15; the owner's listening check of `sleep = true` |
| Later | The deferred items of §10, each when a project needs it | Its own tests, named here |

## Decisions on the questions (2026-10-27)
The owner's answers are quoted in Hungarian, with translations. The rest are the operator's decisions, delegated, on the recommendations offered.
1. **What a steal sounds like:** the owner did not choose. **Operator decision:** the immediate retrigger (§4) is the default. Test 9's renders, with equal and differing velocities, are kept so the owner can listen later. A fade-before-steal can then be added as another `steal` value.
2. **Which free voice a note takes:** **operator decision:** `rotate` is the default and the only value in phase 1; `lowest` is deferred (§10).
3. **The demo:** the owner: "Új projects/studio-poly" ("a new projects/studio-poly"). Studio and studio-transport and their goldens stay as they are.
4. **A voice that faults:** **operator decision:** in v1 it stays in rotation and the slot card marks it (§7).
5. **Glide:** the owner: "Igen, hangonként" ("yes, per voice"). Each voice glides from its own last note (§6). `spread` stays deferred (§10).
6. **Loudness:** the owner: "Nyers összeg" ("the raw sum"). No scaling by N (§5). `studio-poly` sets its voice's `volume` for chords.
7. **A chord tool in the piano roll:** **operator decision:** not in this ADR.
8. **Dynamic polyphony** (the owner's follow-up question): "Felső korlát van, de csak a szóló hangok fogyasztanak. – ez jól hangzik" ("there is an upper limit, but only the sounding voices consume. That sounds good."). `voices = N` is a maximum, and sleeping voices are designed in §11 as phase 5. Growing N automatically is rejected (Alternatives).
