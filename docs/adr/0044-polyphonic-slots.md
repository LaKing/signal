# ADR 0044: Polyphonic slots

- **Status:** Proposed (operator, 2026-10-27). The owner's words (translated): "at slots we could perhaps introduce the concept of polyphony." The owner approved the operator's outline for an ADR: a slot gets a `voices = N` property; when the slot is resolved (before planning, as every slot is, ADR 0037 §5) its content is instantiated N times; a voice allocator, a library primitive and not a core concept, distributes the notes to the voices; the voices' outputs are summed; parameters are shared. The design below is the operator's (technical choices delegated). The taste and scope choices are left as **Questions for the owner**, each with a recommended answer.
- **Builds on:** ADR 0001 and hard rule 1 (no domain concepts in the core), ADR 0008 and 0016 (determinism, canonical order), ADR 0027 §4 (the library, pinning), ADR 0028 (patterns, `std.note_seq`), ADR 0035 (projects as content), **ADR 0037 (slots)**, ADR 0039 (the `audition` note encoding), ADR 0041 (snapshots), ADR 0042 and 0043 (ranges and value sets), `semantics.md` §3, §4, §11, §12 (sums, events, determinism, swaps and the added-instance fade).
- **Amends** (on acceptance, explicitly): ADR 0037 §2 (reserved keys), §3 (port fields, templates), §5 (what the plan is) and §7 (the slot card); ADR 0028 and 0039 (`std.note_seq` gains an output); ADR 0035 §1 (where project content may be loaded). **Supersedes nothing.**
- **Date:** 2026-10-27

## Context
- **The use case:** chords and overlapping release tails. Today a slot with `lib.voice` loaded with `moog_voice` plays one note at a time: `std.note_seq` reduces its pattern to one voice (last-note priority, ADR 0028 §6), and the voice's release is cut by the next note's attack.
- **What already exists:**
  - **Pattern files already hold chords.** `kind = "notes"` allows overlapping notes on different keys; only two overlapping notes on one key are an error (`project-format.md` §12). Only the player is monophonic.
  - **A note as one event:** ADR 0039's `audition` value, `Set(128·key + v)` (`v` the velocity in 127ths, 1 to 127 a note on, 0 a note off of that key; a `Trigger` ends every note), defined in `signal-std` (`audition_value`), not in the core.
  - **Slots resolve to ordinary instances** in `signal-project` before `build_plan`, so the core, the graph and the engine never learn the word "slot" (ADR 0037 §5).
  - **Vectors** `{ vector = N }` and `std.unpack<N>` at the registered widths 2, 4, 8, 16, 64, 256 and 1024 (`semantics.md` §16, `crates/signal-std/src/vector.rs`).
  - **Composite exports fan out:** an exported input or parameter may name a list of inner ports (`project-format.md` §3). An exported output names exactly one inner port.
  - **`std.adsr` attacks from its current level** and `std.osc` keeps its phase across a frequency change, so a voice given a new note while it still sounds stays continuous (their rustdoc; `adsr.rs` test AC4b).
- **The constraint:** polyphony is a music concept. Like the transport (ADR 0033), it must live in the library and the project format, never in `signal-core`, `signal-graph` or `signal-engine` (hard rule 1). Its output must stay identical for every *k*-partition and thread count (hard rule 3).

## Decision

### 1. `voices = N` on a slot; nothing changes without it
```toml
[instances]
melody = { module = "std.note_seq", pattern = "chords" }
synth  = { slot = "lib.poly_voice", load = "moog_voice", voices = 8, set = { cutoff = 400 } }
pad    = { slot = "lib.poly_instrument", load = "moog_voice", map = { l = "out", r = "out" },
           voices = { count = 6, assign = "rotate", steal = "oldest", legato = false } }

[wiring]
connections = [
  { from = "melody.notes", to = "synth.notes" },
  { from = "synth.out", to = "echo.in" },
]
```
- **`voices`** (optional) is an integer from 1 to **64** (`MAX_VOICES`), or a table:
  - `count`: the integer;
  - `assign`: `"rotate"` (default) or `"lowest"` (§4);
  - `steal`: `"oldest"` (default) or `"none"` (§4);
  - `legato`: `false` (default) or `true` (§4).
  Any other key, or a count outside 1 to 64, is an error at the value.
- **A slot without `voices` is exactly ADR 0037's slot.** Every `voice` field of its interface (§2) is ignored, its plan is byte-identical to today's, and every golden is unchanged.
- **`voices = 1` is a polyphonic slot of one voice.** It is not the mono slot: it has an allocator and its voice is at `/synth/v0` (§5). With `legato = true` it plays the way `std.note_seq`'s mono outputs do (test 3).
- **Where:** wherever a slot may appear (ADR 0037 §2): `project.toml` and project composites. A poly slot inside a composite has that file's N, the same for every instance of the composite.
- **Content:** a module (`std.x`, a composite, `lib.x`). **A project is refused as the content of a poly slot** in v1, at `load`: a project's instance is a scope root whose streams do not depend on its address (ADR 0035 §4), so N copies of it would draw identical noise. No use case needs it (hard rule 8). This amends ADR 0035 §1.
- **`voices` is a reserved key**, added to ADR 0037 §2's list. It is an error on a plain instance (`module =`, `project =`). Extending it to plain instances later is the same resolution, if someone asks for it (Alternatives).

### 2. The interface declares each port's role: `voice = "shared" | "each" | "sum" | "alloc"`
A new optional **port field `voice`**, in an inline interface and in a template alike (amends ADR 0037 §3 and `project-format.md` §14). It is read only when the slot has `voices`.

| Role | On | Default for | Meaning in a slot with N voices |
|---|---|---|---|
| `shared` | inputs (signal, vector, events) | every input | The host's wire reaches the matched port of **every** voice (broadcast). Events: each voice gets its own copy of every event. |
| `each` | signal inputs | — | **One value per voice.** Phase 1: the input is fed by the slot's allocator output **of the same name** (`hz`, `gate`, `velocity`, `key` or `spread`, §4), element *e* to voice *e*, and the host cannot wire it (an error at the wire: "`synth.hz` is fed by the slot's allocator"). |
| `sum` | signal outputs | every output | The voices' outputs are **summed** into the slot's output (§5). |
| `alloc` | one events input | — | The **allocator's input**: the note events (§3) the slot distributes among its voices. It is matched to no content port. |

- **Parameters are always shared:** a parameter's value, its `set` and `ramp` events and its modulation reach every voice. A `voice` field on a parameter is an error. Variation from voice to voice comes from `each` inputs inside the content (§6), never from per-voice parameter values.
- **Errors**, located at the field or the wire: `each` on an output or an events input; `each` with a name the allocator does not output; `alloc` on anything other than an events input; two `alloc` ports; `sum` on an input.
- **Phase 1 refuses, as "not yet",** events and vector outputs in a poly slot (a merge or a vector sum, when a project needs one), and a host-fed `each` input (a vector `{ vector = N }` from the host, distributed element by element; it needs `std.unpack<N>` at every N, see Alternatives). Hard rule 8: no project needs these yet.
- **Warnings:**
  - a slot with `voices` whose interface has no `each` and no `alloc` port: "every voice of `synth` gets the same inputs";
  - an `alloc` port in a slot without `voices`: "`synth.notes` feeds nothing: the slot has no voices";
  - a slot with `voices` and the template `lib.voice` or `lib.instrument`: "did you mean `lib.poly_voice`?".
- **`map`** works as in ADR 0037 §4, the same for every voice: `map = { hz = "freq" }` renames toward the content, and `map = { velocity = false }` leaves every voice's `velocity` unconnected (it reads 0) and the allocator's `velocity` unused. A `map` key naming the `alloc` port is an error ("`notes` feeds the slot's allocator; it has no content port").
- **Matching** is ADR 0037 §4's, done once against the content, since every voice is the same content. The `alloc` port takes no part in it.
- **Mono content is the normal case:** each voice is one copy of a mono module (`moog_voice`). Stereo content (`l`, `r`, or a mono `out` mapped to both) sums each channel separately.

**New library templates** (amends ADR 0037 §3's table, pinned by text hash with a `CHANGELOG.md` entry under the current MAJOR.MINOR, as every template is):

| Template | Inputs | Outputs | Matches |
|---|---|---|---|
| `lib.poly_voice` | `notes` (events, `alloc`), `hz` (frequency, Hz, `each`), `gate` (gate, `each`), `velocity` (control, 0 to 1, `each`) | `out` (audio, `sum`) | `moog_voice` with no `map` |
| `lib.poly_instrument` | as `lib.poly_voice` | `l`, `r` (audio, `sum`) | a mono voice through `map = { l = "out", r = "out" }` |

`lib.voice` and `lib.instrument` keep their text and their hashes: a template is never changed incompatibly (ADR 0037 §3), and new roles are a new name.

### 3. How notes reach a poly slot: note events, with a poly output on `std.note_seq`
- **The note event convention** (library, `module-contract.md`): ADR 0039's `audition` value, unchanged, named as the convention for every note stream. `Set(128·key + v)` with `key` an integer from 0 to 127 and `v` an integer: from 1 to 127 a note on at velocity `v/127`, 0 a note off of that key. A `Trigger` ends every note. Anything else is ignored. The port is an ordinary events port of kind `generic` with the unit `note`. **No new `PortKind`**, since a kind is a core type (hard rule 1). `signal_std::audition_value` is the one encoder.
  - Velocity on this path is **quantised to 127ths**, as with MIDI. One event per note keeps a note atomic (ADR 0039, Alternatives). A pattern velocity of 0.8 arrives as 102/127.
- **`std.note_seq` gains an event output `notes`** (amends ADR 0028 §6 and ADR 0039; additive):
  - It carries **every** pattern note, chords included: each note's on at its start and its off at its end, by the same tick or follow rules that place the mono notes. There is no last-note priority on this output.
  - Within a frame, a `Trigger` (if any) comes first, then the offs, then the ons, so a voice freed on a frame can be given a note on that frame (§4). Each group is in key order.
  - A note that starts and ends within one frame is not emitted. It would not sound at frame resolution, and the mono gate is low at the end of that frame too.
  - A **seek** and a **stop** (the cases where the mono outputs end their note, ADR 0033) emit one `Trigger`.
  - **Audition notes** (ADR 0039) are added to this output as notes of their own: the pattern goes on, so keys can be played over it as chords. A key that sounds from both the pattern and an audition gets one on. Its off is sent when both have ended.
  - **At most 16 events per frame** (`semantics.md` §4): `notes` never overflows. A frame whose events would exceed 16 (a chord of more than 8 at a loop boundary, or a fast forward scrub) is reduced to its end state: one `Trigger`, then the ons of the notes still sounding at the frame's end, at most 15, latest start first and then highest key. Every note left out is counted on a new signal output **`dropped`** (a count since `prepare`), for a scope or the slot card.
  - The mono outputs (`gate`, `key`, `hz`, `velocity`, `on`, `off`) are unchanged. An unconnected `notes` changes nothing, so every golden is unchanged.
- **How an existing mono sequencer drives a poly slot:** it is the same `std.note_seq`. Wire `melody.notes → synth.notes` in place of `hz`, `gate` and `velocity`. A pattern without overlaps then plays the same melody, with each note's release left to ring on its own voice.
- **Other sources:** anything that emits note events, such as another sequencer, or a future MIDI adapter (M6). Several sources wired into one `alloc` port merge in canonical order (`semantics.md` §4).
  - A gate-and-key signal source (an arpeggiator built from primitives) needs a converter `std.gate_notes`. It waits until a project has such a source (hard rule 8).
- **Pattern format:** unchanged. The poly sequencer is the existing format played without priority.
  - `std.step_seq8` stays as it is: drum lanes are separate voices already.

### 4. The allocator: `std.voice_alloc`
A direct primitive in `signal-std`, category `event`, at divisor 1. It is **one type at the fixed width `MAX_VOICES` = 64**, so it survives a live change of N (§8).

- **Input:** `notes` (events, §3).
- **Parameters**, set from the slot's `voices` entry when the plan is built, each with a range or a value set (ADR 0043):
  - `voices`: 1 to 64, read rounded;
  - `assign`: 0 `rotate`, 1 `lowest`;
  - `steal`: 0 `oldest`, 1 `none`;
  - `legato`: 0 off, 1 on.
- **Outputs**, each a vector of 64, where element *e* is voice *e* and elements at or above `voices` stay 0:
  - `hz`: `440·2^((key − 69)/12)`, through the same function as `std.note_seq`'s `hz`, so the values are bit-identical. It is 0 before the voice's first note, so `moog_voice`'s first-note glide rule works per voice;
  - `gate`: 0 or 1;
  - `velocity`: `v/127`;
  - `key`;
  - `spread`: `(2e − (N − 1)) / (N − 1)` for N ≥ 2, from −1 for voice 0 to 1 for voice N − 1; 0 for N = 1;
  - and the signal **`dropped`**: the notes not played since `prepare`.
  `hz`, `velocity` and `key` hold after a note ends, so a release rings at its own pitch.
- **Per voice state:** the key it plays (or none), its gate, and `order`: the value of a counter that goes up by one for each event the allocator applies, recorded at the voice's last on or off. A voice never used has `order` 0.
- **Each frame**, the frame's events apply in their canonical order on the port (`semantics.md` §4), one after another:
  1. **Note on, key *k*, already sounding** on voice *e* (its gate high): a **retrigger** of *e* (below), with the new velocity. With `legato` on, the velocity changes and the gate stays high.
  2. **Note on, a free voice exists** (gate low, *e* < N): `rotate` takes the free voice with the **smallest `order`**, the one released longest ago, so recent release tails ring on. `lowest` takes the lowest free index. Ties go to the lower index.
  3. **Note on, no free voice:** `oldest` **steals** the sounding voice with the smallest `order` (the oldest note on). `none` drops the note (`dropped` + 1).
  4. **Note off, key *k*:** the voice playing *k* takes gate 0 and records its `order`. An off for a key that no voice plays (stolen or dropped) is ignored. A `Trigger` releases every voice.
- **Retrigger and steal: the gate is low for one frame.** A voice whose gate is high and that is given a new note (a steal, or a retrigger of its own key) has gate 0 on that frame. From the next frame on its gate is 1 with the new note's `hz`, `key` and `velocity`.
  - The envelope therefore sees a new rising edge. It attacks **from its current level**, and the oscillators keep their phase, so the output stays continuous (fact for `std.adsr` and `std.osc`; a content made of other modules needs the same properties).
  - The cost is one frame of delay (10.4 µs at 96 kHz), on steals and retriggers only. A note given a free voice starts on its own frame.
  - With **`legato` on**, a stolen voice whose gate is high takes the new note on that frame without the gap: gate stays high, and `hz`, `key` and `velocity` change, as `std.note_seq`'s last-note priority does. `legato` with `voices = 1` and `steal = "oldest"` is the mono legato mode. A stolen note never comes back, as in ADR 0028.
  - A frame whose events end a pending start before it begins (an off on the next frame) applies the start first, then the off. The order is fixed, so the result is too.
- **When N changes** (the `voices` parameter, §8), the voices at or above the new N are released (gate 0) and are no longer assigned. Voices added later are free, with `order` 0, so `rotate` takes them first.
- **Determinism:** the result depends only on the events in canonical order and on the parameters. There is no time, no randomness, and no dependence on arrival or schedule. Ties are broken by index and by the event counter. Its state moves one frame at a time inside `process`, so a block of *k* frames gives exactly the frames one by one (the contract suite checks *k*-partition invariance). It is one instance, so the thread count cannot matter.
- **Real time:** fixed arrays (64 voices, a 128-entry key-to-voice table), no allocation, no locks. Each event does a bounded scan of at most 64 voices.

### 5. What the plan is: an ordinary instance of a generated composite (hard rule 1)
- **Resolution** stays in `signal-project`, before `build_plan` (amends ADR 0037 §5). A loaded poly slot `synth` with N voices becomes an ordinary `InstanceDef { name: "synth", module: <generated definition> }`. The **generated composite** is built in memory from the slot entry and the content, and no file holds it:

  | Inner instance | Type | What |
  |---|---|---|
  | `alloc` | `std.voice_alloc` | its parameters from `voices` |
  | `each_<port>` | `std.unpack64` | one per `each` port: `alloc.<port>` → `in`; `e<e>` → voice *e*'s matched port |
  | `v0` … `v<N−1>` | the content | the voices |
  | `sum_<port>` | `std.sum` (new, below) | one per `sum` output: every voice's matched output → `in` |

  - **Exports:**
    - a `shared` input exports the list of every voice's matched port (fan-out);
    - the `alloc` port exports `alloc.notes`;
    - a `sum` output exports `sum_<port>.out`;
    - every interface parameter exports the list of every voice's matched parameter;
    - every content parameter outside the interface does the same under its content name, so `set` keys and live edits reach them.
  - **Values:** interface parameter values and `set` values are values on the instance `synth`. They reach every voice through the exports, as for any composite.
  - **The host's wires** on the slot's ports become wires on these exports by name.
- **`std.sum`** (new, `core` category, direct, divisor 1): one signal input `in` and an output `out = in`. Since a signal input is the canonical sum of its connections (`semantics.md` §3), `out` is the voices' sum. Its kinds are `generic`.
  - A sum starts from +0, so `in` is never −0, and the copy is exact.
  - It gives the summed output **one port**, for meters, panel bindings (`/synth.out`), restricted inputs (ADR 0024), `is_connected`, and the host wire's `scale`, which then applies once to the sum.
  - It is needed because an exported output names exactly one inner port (`project-format.md` §3). That rule is not changed (Alternatives).
- **Addresses** follow the address grammar (`[a-z][a-z0-9_]*` segments, so not `/synth#3`): `/synth/v3/osc1`, `/synth/alloc`, `/synth/each_hz`, `/synth/sum_out`.
  - Content names never collide with these, because the content lives one level down under `v<e>`.
  - The byte-wise canonical order of a receiver's sources (`/synth/v0`, `/synth/v1`, `/synth/v10`, …, `/synth/v2`) is fixed by the text, which is all determinism needs. Displays sort voices numerically.
  - Every source inside the slot shares the prefix `/synth/`, so the slot's place among the host's other sources is the same as for a mono slot.
- **Random streams:** each voice's primitives draw `derive(seed, /synth/v<e>/…)` (`semantics.md` §11), so every voice has its own noise. Voices are not scope roots. Adding voices does not change the streams of the voices that stay.
- **The plan is byte-identical to the explicit form:** a project composite holding the same instances, wires and exports, instantiated as `synth = { module = … }`. So canonical order, fades, faults, converters, `is_connected` and the oracle need nothing new.
  - **`signal-core`, `signal-graph` and `signal-engine` learn nothing.** No "voice", no "poly". `domain_scope.rs` gains the project-format spellings phase 1 chooses (`voice_alloc`, `voices`, the resolution's type names), as ADR 0037's note did for slots.
- **An empty poly slot** contributes no instance and no connection, the allocator included (ADR 0037 §5).
- **The side table** `SlotResolution` gains `voices` (count, `assign`, `steal`, `legato`, and the voice addresses) for `describe`, the schematic and the panels. The plan has no record of it.

### 6. Parameters, modulation, snapshots and ranges
- **Shared by default, and only shared:** a panel control `/synth.cutoff`, a `set`, a ramp and a modulation wire into `synth.cutoff` reach every voice equally, through the export fan-out.
- **Per-voice variation** comes from `each` inputs inside the content. Example: a voice with a `spread` input (−1 to 1 across the voices, §4) that multiplies a shared `detune_spread` parameter into its oscillators' cents, or pans by it. The slot declares `spread = { voice = "each" }` and the allocator feeds it. Per-voice velocity and key tracking already work inside the content from `velocity`, `key` and `hz`.
- **Snapshots (ADR 0041)** are unchanged: a poly slot's controls are `/synth.<name>`, one value each. `voices` and the allocator settings are structure written in the slot entry, not controls, so a snapshot neither saves nor restores them. A restore is still one parameter edit, never a swap.
- **Ranges and value sets (ADR 0042, 0043):** a shared export's targets are N copies of one content parameter, so its range or value set is the content's, unchanged. The allocator's parameters declare theirs (§4). `voices` outside 1 to 64 is an error in the file, not a correction, because it changes structure.

### 7. Schematic, panel, `describe`, MCP
- **Schematic:**
  - a poly slot is one box with its interface's ports and a **×N badge** ("synth ×8 ◂ moog_voice");
  - the `alloc` port is marked, and `each` ports show as fed inside;
  - semantic zoom opens the generated level, **read-only** (it is not text): the allocator, the unpacks, the sums and the N voice boxes;
  - a voice box opens into the content. That is the one shared definition, so an edit there (where the content is editable at all, ADR 0037 §7) is an edit of every voice;
  - ADR 0030's trace runs from `notes` through `alloc` and the element *e* of an unpack to voice *e*, and from the voices through `sum_<port>` out;
  - the `graph` message's `slot` gains `voices: { count, assign, steal, legato }` (additive, `ui-protocol.md`).
- **Panel:** **one slot card**, as ADR 0037 §7, plus:
  - "×N" in its header;
  - a voices stepper (1 to 64) and the `assign`, `steal` and `legato` selectors, which write the slot's `voices` entry with `toml_edit` (text first). A change is a live edit (§8);
  - a **voice row**: N dots, each lit while its voice's gate is high (observation of `/synth/alloc.gate`, through the vector adapter), with a mark on a faulted voice (below);
  - the `dropped` count when it is not 0.
  - The `out` row's meter is `/synth.out`, the sum. Per-voice meters are `/synth/v<e>.out`, shown when the voice row is opened.
  - The content's own cards fold under the slot card **once**: their controls are the shared `/synth.<name>`.
- **Faults:** the guard mutes the **instance** that faulted, as always (`semantics.md` §7, §14). That is one primitive of one voice, never the slot.
  - The faulted voice goes on being assigned notes, which it plays partly or not at all. The allocator cannot know, since a fault is not a signal (Question 4).
  - The slot card marks the voice, and `describe` lists the fault under its voice's address.
  - A swap that replaces the voice clears it: a Replace, an unload and load, or an N that drops that voice.
- **`describe`** lists, per poly slot, `voices` with its voice addresses, the roles of its ports, and its match table (once, since all voices share it).
- **MCP:** a new patch op and tool, `slot.voices {slot, count?, assign?, steal?, legato?}` (`agent-interface.md`). It writes the host's files only, as `slot.load` does, and `voices` changes are live edits there too. ADR 0029's gain guard sees a poly slot's output as one fan-in source.

### 8. Live edits: changing N, Replace, the fade
A change of `voices` is a project edit like any other, applied by `diff.rs` and `live.rs` as one or two swaps (`semantics.md` §12):
- **N grows (M → N):**
  - the swap adds `v<M>` … `v<N−1>`, and their `audio` outputs fade in over F = 5 ms (the added-instance fade; they are silent, gate 0, until a note comes);
  - `live.rs` sends `set(voices = N)` to the surviving `alloc` at the swap frame;
  - `alloc`, `each_*`, `sum_*` and `v0` … `v<M−1>` survive, with their state, so held notes and release tails play on bit-continuously.
- **N shrinks (M → N):** the removal protocol of `semantics.md` §12. At frame *s*, `set(voices = N)` to `alloc`, which releases the voices at or above N, and a suspend of `v<N>` … `v<M−1>`. At *s* + F the swap removes them; their audio is +0.0 from *s* + F − 1. Their notes are not moved to the remaining voices: moving a note mid-way would restart its contours. A release tail is cut by the 5 ms fade, which is glitch-free.
- **`voices` added to or removed from a slot** (mono ↔ poly) moves the content to other addresses (`/synth/…` ↔ `/synth/v0/…`), so it is a **Replace**, unload then load (ADR 0037 §6).
- **Replace or a changed `load` in a poly slot** is ADR 0037 §6's unload then load. The slot is empty in between, so the allocator leaves too and nothing carries over.
- **`assign`, `steal` and `legato`** are parameter edits of `alloc`, with no swap.
- **The plan cost of a change** is the voice instances added or removed. Real-time rules hold as for any swap: everything is prepared off the audio thread, and committing allocates nothing.

### 9. CPU budget and limits
- **Cost:** N times the content, plus one allocator, ≤ 5 unpacks and one `std.sum` per summed output, all small.
- **Estimate** (an assumption, not a measurement): `moog_voice` is 39 primitives, and the studio's 177 instances are about 8 % of a core at 48 kHz (`docs/benchmarks.md`). So a voice is roughly 2 to 3 % of a core at 48 kHz and twice that at 96 kHz.
- **Parallel work:** voices share no state, so they are independent units that ADR 0019's scheduler can spread over the workers. Polyphony is the best case for the multi-core engine. Phase 1 measures it (`signal bench`, `docs/benchmarks.md`), and phase 4 on the device.
- **`MAX_VOICES` = 64:** the allocator's width, and a registered `std.unpack64`. 64 voices of `moog_voice` are about 2 500 instances, about 1 % of `MAX_PLAN_INSTANCES` (`semantics.md` §16, "Limits"), and nested poly slots (N × M) stay under the same plan limits. A project that needs more than 64 voices needs a new allocator width, which is a change of library, not of the core.
- **The fixture uses 8 voices.** No default N exists: `voices` is always written.

## Alternatives considered
- **A voice concept in the engine** (the engine clones an instance N times, or keeps voice-indexed state): a domain concept in the core, against hard rule 1, and a second runtime model next to the plan. Rejected. Resolution gives the same behaviour from ordinary instances.
- **A dedicated poly container instance** (`poly = { module = …, voices = 8 }`): a second container concept beside slots. A slot already has what polyphony needs: an interface where each port can declare its role, and resolution before planning. Rejected.
- **Polyphony inside composites** (writing N voices and an allocator by hand): it works today and remains possible, and it is exactly the explicit form a poly slot plans as (§5). But N and the wiring are then repeated by hand and cannot change live without rewriting the composite. Kept as the reference that test 2 compares against.
- **`voices` on plain instances** (`module = …, voices = 8`): the same resolution with an interface derived from the module. Deferred, not rejected. The slot is the boundary where instruments are swapped, so it is where polyphony is first needed.
- **The allocator in the host, wired by hand** (`alloc = { module = "std.voice_alloc8" }` into vector inputs of the slot): visible, but N is then written twice, and a width per N means a change of N replaces the allocator and loses its held notes. The generated allocator writes N once, keeps one type at width 64 and survives N changes (§8).
- **Vectors of hz, gate and velocity from the host** (`{ vector = N }` slot inputs, element *e* to voice *e*): generic and domain-free, and the role `each` already names it. Deferred to the first project that computes its own per-voice signals. It needs `std.unpack<N>` at every N, made by name on demand as converters are (`signal_core::converter`), or the vector widths extended.
- **A poly sequencer that allocates** (vectors of voices out of `std.note_seq`): it would tie a pattern to a voice count and put stealing where the voices are unknown. Allocation belongs to one place, the slot, which knows N.
- **Summing by a fan-in export** (an exported output naming several inner outputs, summed at each receiver): it needs no `std.sum`, but it changes the composite format and `signal-graph`, and leaves no single port for meters, bindings, restricted inputs and the wire's scale. Rejected for one tiny primitive.
- **Exact velocities in two events** (key, then velocity): ADR 0039 chose one event per note so that a note is atomic. 127ths are MIDI's resolution and inaudible as a difference. Rejected.
- **Voices made on demand at run time:** allocation on the audio thread, and a graph that changes without a plan swap. Both are forbidden. N is fixed per plan.
- **Stealing with a short fade before the new note** (2 to 5 ms late): a classic choice, but it moves the note late. It stays open as a choice for the owner (Question 1).
- **Per-voice parameter values** (a parameter role `each`): parameters are scalar, and a per-voice base would be N saved values under one control, which snapshots, panels and ranges would all have to learn. Per-voice variation through `each` inputs (`spread`) needs none of that.
- **Addresses `/synth#3/…`:** `#` is outside the address grammar, whose byte order canonical sums rely on (`signal-core` `address.rs`). `/synth/v3/…` needs no change.

## Consequences
- **`signal-core`, `signal-graph`, `signal-engine`, `signal-oracle`:** no change. `domain_scope.rs` gains the new spellings.
- **`signal-std`:**
  - `voice_alloc.rs` (`std.voice_alloc`) and `std.sum`;
  - `note_seq.rs`: the `notes` and `dropped` outputs;
  - the note event convention documented beside `audition_value`;
  - contract suite and unit tests.
- **`signal-lib`:** `interfaces/poly_voice.toml` and `poly_instrument.toml`, with pinned text hashes, a `CHANGELOG.md` entry and `tests/pinned.rs`.
- **`signal-project`:**
  - `model.rs`: `voices`, the `voice` port field, reserved keys;
  - `slot.rs`: roles, the generated composite, diagnostics, `SlotResolution::voices`;
  - `diff.rs` and `live.rs`: N changes as in §8;
  - `structure.rs`, `patch.rs` (`slot.voices`), `schema.rs`.
- **`signal-ui` and `ui/`:** the ×N badge and the read-only generated level, the slot card's voices controls and voice row, the MCP tool.
- **Docs, on acceptance:**
  - `project-format.md` §14 (`voices`, `voice`, the generated composite);
  - `module-contract.md` (the note event convention, `std.voice_alloc`, `std.sum`, `note_seq.notes`, the poly templates under "Interfaces");
  - `00-vision.md` §3a (the Slot row: "may be polyphonic, `voices = N`");
  - `ui-protocol.md`, `ui-controls.md`, `agent-interface.md`, `benchmarks.md`, `roadmap.md`.
  - `semantics.md` needs no change: every behaviour is an existing rule.
- **Goldens:** none change. New: `projects/studio-poly`, a copy of studio-transport with `synth` as a `lib.poly_voice` slot of 8 voices playing a chord pattern from `melody.notes`, and its golden.

## Tests
1. **Nothing changes without `voices`:** every golden, every plan dump and every ADR 0037 slot test is byte-identical; an unconnected `note_seq.notes` changes no render.
2. **Equivalence with the explicit form:**
   - a poly slot's plan is byte-identical to a hand-written project composite with the same allocator, unpacks, sums, voices and exports, as `synth = { module = … }`;
   - both render bit-identically for *k* = 1, 64, 256 and threads 1, 2, 3, 4, 8, through the offline driver and the real-time runner (null driver).
3. **One voice, legato:** with `voices = { count = 1, legato = true }` fed by `melody.notes`:
   - the voice's `hz`, `gate` and `key` equal `std.note_seq`'s mono outputs bit for bit, and `velocity` equals them rounded to 127ths. The fixtures are the studio melody and edge cases (overlaps, touching notes, notes within one frame, wraps, a seek, a stop);
   - with a pattern whose velocities are multiples of 1/127, `moog_voice`'s audio equals the mono slot's bit for bit.
4. **Allocator against a brute-force reference** (a per-frame model in the test, on seeded random note streams): `rotate` and `lowest`, `oldest` and `none`, `legato`, a same-key retrigger, `Trigger`, ties at one frame, changes of `voices`, `dropped`, and `spread` for each N.
   - The contract suite on `std.voice_alloc` and `std.sum`: *k*-partition invariance, no allocation in `process`, bounded outputs.
5. **`note_seq.notes`:**
   - chords from the pattern, offs before ons, a note within one frame not emitted;
   - seek and stop as one `Trigger`;
   - audition notes added, a shared key's off only when both have ended;
   - the 16-event reduction and `dropped`;
   - unconnected changes nothing.
6. **Diagnostics,** located by line and column:
   - errors: a count outside 1 to 64, an unknown `voices` key, a project `load`, the role errors of §2, a host wire into an `each` port, events and vector outputs and host-fed `each` as "not yet", a `map` on the `alloc` port;
   - warnings: no `each` and no `alloc` port, an `alloc` port without `voices`, the "did you mean `lib.poly_voice`";
   - `voices` on a plain instance is an error.
7. **Live,** on the null driver and the runner, with seeded callback sizes and threads 1 and N, at every phase of a held note and of a release:
   - N grows and shrinks: survivors bit-continuous, added voices fade in as in §12, removed voices +0.0 from *s* + F − 1, held notes on surviving voices play on;
   - mono ↔ poly and a changed `load` are Replaces;
   - `assign`, `steal` and `legato` edits need no swap;
   - no allocation on the audio thread (counting allocator); a recorded session replays bit-identically.
8. **Determinism matrix** on `projects/studio-poly`: *k* = 1, 64, 256 × threads 1, 2, 3, 4, 8, offline and real time; the golden.
9. **Continuity of a steal** (a measurement, not a gate): the largest frame-to-frame step of a stolen `moog_voice` against the same note played on a free voice, reported for the owner's listening.
10. **UI:**
    - the `graph` voices field and the ×N badge;
    - the read-only generated level;
    - the slot card's stepper and selectors write the expected text and survive a reload;
    - the voice row lights as the gates do;
    - `slot.voices` over MCP;
    - `signal ui check` passes on `studio-poly`.
11. **Device:** 10 minutes of `studio-poly` at 16 voices with N changed every 30 s (8 ↔ 16) and a Replace every 2 minutes: 0 xruns. The CPU per voice is recorded in `docs/benchmarks.md`.
12. **The owner's listening verdict** (hard rule 6).

## Phases
| Phase | Delivers | Exit criteria |
|---|---|---|
| 1. Offline | `voices`, the `voice` field, roles and diagnostics; the generated composite; `std.voice_alloc`, `std.sum`; `note_seq.notes`; `lib.poly_voice`, `lib.poly_instrument`; `describe`; `projects/studio-poly` and its golden; a CPU measurement | Tests 1–6 and 8 (offline); every golden unchanged |
| 2. Live | N changes, mono ↔ poly and Replace in a poly slot, sessions and replay | Tests 7 and 8 (real time) |
| 3. UI | The ×N box and the generated level, the slot card's voices controls and voice row, `slot.voices` over MCP, `ui-protocol.md` fields | Test 10; the operator's visual check |
| 4. Device and verdict | Device run and measurement | Test 11; the owner's verdict (test 12) |

## Questions for the owner
Each has the operator's recommended answer. With no answer, the recommendation is what phase 1 builds.
1. **What a steal sounds like.** When every voice is held and a new note comes, the oldest note's voice is taken: (a) at once, its envelope retriggered from where it is (one frame late, continuous, as many hardware polysynths do), or (b) after a short fade of 2 to 5 ms (smoother on slow attacks, but the new note is that much late). **Recommended: (a)**, with (b) added later as a third `steal` value if you hear a problem.
2. **Which free voice a note takes.** (a) `rotate`: the voice released longest ago, so the last notes' tails ring on; (b) `lowest`: always the lowest free voice, so a tail can be cut by the next note. **Recommended: (a) as the default**, with (b) selectable.
3. **The demo.** Polyphony in a new project `projects/studio-poly` (a copy of studio-transport with a chord pattern and an 8-voice slot), or studio-transport itself made polyphonic, which changes its golden and its sound? **Recommended: the new project**; studio and studio-transport stay as they are.
4. **A voice that faults.** In v1 a faulted voice stays in rotation and its notes are silent until the content is replaced or N changes. Skipping it would need the engine to report a fault as a signal into the graph, which is a new mechanism. **Recommended: accept v1's behaviour**; the slot card marks the voice.
5. **Per-voice character.** The mechanism (`spread`, −1 to 1 across the voices) is in phase 1. Should the demo's voice also get a "spread" control (detune and pan across the voices), which means a poly variant of `moog_voice`? **Recommended: not yet.** The demo plays `moog_voice` unchanged, `spread` is tested on a fixture, and a spread control is added when you want that sound.
6. **Chords in the piano roll.** The pattern format and the roll already hold overlapping notes on different keys, so chords can be drawn note by note. Is a chord tool wanted (several keys at once, chord shapes)? **Recommended: not in this ADR**; ask for it when drawing chords note by note proves slow.
