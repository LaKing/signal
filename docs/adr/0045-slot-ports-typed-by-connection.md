# ADR 0045: Slot ports typed by connection

- **Status:** Accepted (2026-10-29: the owner accepted it, "igen"; proposed the same day). The owner asked on 2026-10-28 (translated from Hungarian): "Are all 8 port kinds justified? What is Rate, Unit, Group? The range comes from its connection, right? Maybe a slot only needs an 'undefined' io, and when something is connected to it, that defines it." The operator answered the questions (Context) and proposed an outline: a slot port that declares only its name and direction, typed by what it is connected to, with explicit fields still overriding, templates keeping their types, inference a pure function of the files, and an interface dialog that shows the name and direction and keeps the rest under "Advanced". **The owner accepted that outline.** The design below is the operator's (technical choices delegated).
- **Builds on:** ADR 0005 (ports, rates, converters, `generic`), ADR 0009 (text is the truth), ADR 0030 §3 (port groups), ADR 0035 (a project as content), **ADR 0037 (slots)** with its phase notes and "Editable insides", ADR 0042 and 0043 (ranges and value sets of exports), ADR 0044 (polyphonic slots, its `voice` port field).
- **Amends ADR 0037 §3** (the port fields: a port may be **auto**; and §3's sentence that a template's "ports declare their types, because nothing inside them could tell" no longer holds for a project template, §5), and, for auto ports only and as a consequence of §3, the checks of §2 ("Wiring is explicit": a wire on an auto port is not checked against a declared type) and of §4 (the table's shape and rate rows do not apply to an inferred field). Typed ports keep every rule of ADR 0037 unchanged. Also amends `project-format.md` §14 and ADR 0044 §2 (a port with a `voice` field must be typed, §6 below). It is applied to those documents when phase 1 is built. **Supersedes nothing.**
- **Date:** 2026-10-29

## Context

### What the fields are, and what reads them for a slot port
A slot's interface never reaches the plan (ADR 0037 §5). A loaded slot becomes an ordinary instance of its content, with the host's wires renamed to the content's ports, so what plays is decided by the content's own ports and the wires. The interface's types decide only which wires and loads are **refused**, which **warnings** are given, and what the editor **shows**. Verified in the code:

| Field | What it is | What reads it on a slot port today | Structural? |
|---|---|---|---|
| `shape` | `signal`, `events` or `{ vector = N }` | `resolve_def` in `crates/signal-project/src/slot.rs`: another shape at a host wire's other end is an error at the wire; at the matched content port, an error at `load` or the `map` entry | **yes**: a mismatch is an error |
| `rate` | A **divisor** *d* of the master rate: a port at rate *d* carries one value every *d* frames, and its type is called on those frames only (`module-contract.md` "Rates and vectors") | the same checks, by exact equality. Between two ordinary ports, the graph inserts a converter where one rate divides the other (`core.hold…`/`core.decimate…`); **across a slot port no converter is ever considered**: any other rate is an error | **yes**: a mismatch is an error |
| `kind` | One of 8 `PortKind`s (`crates/signal-core/src/descriptor.rs`): `audio`, `control`, `gate`, `trigger`, `frequency`, `gain`, `phase`, `generic` | a kind mismatch, neither side `generic`, is a **warning** (ADR 0005, note of 2026-10-18); the slot card's tooltip | no: advisory |
| `unit` | A label (`Hz`, `beats`, `dB`) | the editor and the slot card only | no: presentation |
| `range` | `[min, max]` | the editor only; nothing clamps a signal at a port | no: presentation |
| `group` | The name of the group a pin is drawn in on the box (ADR 0030 §3, port groups) | the schematic's pin order only | no: presentation |

### The owner's questions, answered
- **Only `shape` and `rate` are structural**, and for a slot port only in the sense that a mismatch is refused: neither reaches the engine through the slot.
- **`kind` is advisory.** For a primitive's own ports it has two effects:
  - **`audio`:** the engine fades the `audio` outputs of a suspended or removed instance to 0 (`crates/signal-engine/src/engine.rs`, `output_audio`, built from each primitive descriptor port's kind);
  - **every kind** names the converter the graph inserts (the type name ends in the source port's kind, ADR 0005).
  - `gate` (on while non-zero) and `trigger` (momentary impulses) are meanings that modules and people read. No engine code branches on them (verified: outside `signal-std`'s own types, `PortKind::Gate` and `PortKind::Trigger` appear only where kind names are mapped to and from kinds, as in `signal-core`'s converter names and `slot.rs`'s `kind_of`, and in one benchmark's choice of a stimulus, `crates/signal-cli/src/bench.rs`).
  - **For a slot port, kind has neither effect:** the fade follows the content primitive's own `audio` outputs whatever the slot declares, since the interface never reaches the plan. So on a slot port, kind gives warnings and labels only.
- **`unit`, `group` and `range` are presentation only** for slot ports.
- **"The range comes from its connection, right?"** For parameters it already does:
  - a slot's interface parameters declare only a name, an optional `unit` and an optional `group`. Their range and default come from the matched content parameter (ADR 0037 §3; `ParamSpec` has no range, and `slot.rs`'s `parameter()` sets none);
  - a composite's exported parameter takes the range its targets share unless it declares its own (ADR 0042 §2), or a value set (ADR 0043).
  - For slot **ports**, the range was written by hand and only shown. This ADR infers it from the connection, as the owner expected.
- **Are all 8 kinds justified?** The kinds stay in the core as they are. Changing the set touches every primitive's descriptor, the converter names and the kind-mismatch rule, so it would need a revision of ADR 0005. It is out of scope here.
  - *A consideration for that revision:* `frequency`, `gain` and `phase` largely say what a unit says (`Hz`, a factor or `dB`, cycles). `audio`, `control`, `gate`, `trigger` and `generic` say how the values behave, which a unit does not.
- **"Maybe a slot only needs an undefined io."** Yes, for the slot's own interface. Since the interface's types decide only refusals, warnings and labels, a port whose type is taken from what it is wired to loses nothing. It saves the person from retyping by hand what the wiring already says. Where a type is a contract (a template that different contents must fit), it stays declared.

## Decision

### 1. The text: an auto port
A slot port may be declared with its name and direction only:

```toml
[instances]
fx = { slot = { inputs = { in = {} }, outputs = { out = {} } } }       # both auto
mix_in = { slot = { inputs = { a = {}, b = { auto = true, group = "bus" } },  # auto, with a group
                    outputs = { l = "audio", r = "audio" } } }          # typed, as today
ev = { slot = { inputs = { notes = { auto = true, shape = "events" } } } } # shape pinned, the rest inferred
```

- **An auto port** is a port declared as the **empty table `{}`**, or as a table with **`auto = true`**.
  - In an auto port, each of `shape`, `rate`, `kind`, `unit` and `range` is **pinned** when written, and **inferred** (§2) when not.
  - `group` is never inferred: it is the slot's own layout. It is written, or absent.
- **A typed port** is every other declaration, and it means exactly what it means today:
  - the short forms (`"audio"`, `"events"`, …);
  - any table without `auto` that writes at least one field.
  - Its unwritten `shape`, `rate` and `kind` are `signal`, 1 and `generic`, and its unwritten `unit` and `range` are none.
- **Why `{}` and `auto = true`, and not "every unwritten field is inferred":**
  - the tools write tables such as `{ kind = "audio", group = "x" }` and `{ group = "x" }` (`port_text` in `crates/signal-project/src/patch/slot.rs`), and the library templates hold `{ kind = "control", unit = "beats", … }`;
  - reading their unwritten fields as inferred would change what every such declaration means. The empty table is the one exception, discussed in §10.
- **`auto` takes only `true`.** `auto = false`, `auto` on a parameter, and `auto` in a library template (§5) are errors at the key.
- Unit, range and kind may be pinned on an auto port for presentation or a contract (`{ auto = true, kind = "gate" }`).
- **Parameters are unchanged:** a name, optional `unit` and `group`, range and default from the content. `{}` under `parameters` keeps its meaning, a parameter with no unit and no group: auto ports are inputs and outputs only.
- **`{}` means an auto port wherever ports are declared:** a slot's inline interface and a project template alike, with no special case. The library's templates may hold none (§5).
- **No unit, no range, pinned:** in an auto port, `unit = ""` pins "no unit" and `range = []` pins "no range". The writer needs them when it unpins one field of a typed port (§9). No existing text changes meaning: `range = []` is a parse error today (`model.rs` reads two numbers), and `unit = ""` is already an empty unit, shown as none.

### 2. Inference: the defining end
Each inferred field of an auto port is copied from **one end**, its **defining end**. The defining end is a port whose type is **known**: a primitive's port, a composite's or project's export (`signal_graph::ports_of`), a typed slot port, or an auto port already resolved **from an end** at an earlier stage of §3. All of the port's inferred fields come from that one end, which `describe` and the editor name (§8, §9).

**Not known**, so never a defining end and never part of an agreement check (§4):
- an auto port not yet resolved, even one with some fields pinned. Its pinned fields are checked against every end (§4), but they do not make it an end;
- an auto port that fell to the default: the default is the absence of an end, not a type that another port may copy;
- a **parameter** as a consumer (of any instance, slot or not): a parameter takes signals and events alike (`crates/signal-graph/src/validate.rs` lets events into a parameter), so it says nothing about the shape of what feeds it.

In priority order:

- **An input:**
  1. the **host-side sources**: the `from` ends of the wires into it at its level. The first among those whose type is known defines it, in the order of the `from` strings **as written** (`instance.port`, compared with Rust's `str::cmp`). This is a textual order of its own, not `semantics.md` §3's canonical source order, which orders flattened addresses. Several wires from one source (with different scales) count as one end;
  2. else, if the slot is loaded and the port matched (ADR 0037 §4), the **content's target port**: the matched name's port as `ports_of` gives it. For an export with several inner targets, that is its first inner port in address order (ADR 0037 phase 1 notes);
  3. else the **default**.
- **An output:**
  1. if the slot is loaded and the port matched, the **content's source port**;
  2. else the **host-side consumers**: the `to` ends of the wires from it, the first among those whose type is known, in the order of the `to` strings as written (`str::cmp`), several wires to one consumer counting as one;
  3. else the **default**.
- **The default**, when nothing defines it: `signal`, rate 1, `generic`, no unit, no range. This is today's `{}`.
- **What counts as an end:** only wires at the slot's own level. A composite's `[exports]` naming the slot's port is not an end: a composite file is the same for every instance of it (ADR 0037 §2), so what is outside it cannot type its inside.
- **Unit and range** are copied from the defining end where it declares them:
  - a primitive port's descriptor `unit` and `range`;
  - an export's: its own range where ADR 0042 gives it one, else the unit and range of the inner port or parameter behind it, the first in address order (`ports_of` gains both fields, §11);
  - a project's tap channels `l` and `r` (ADR 0035 §2–3) carry neither.
- **A pinned field never steers the choice.** The defining end is chosen by the order above whatever the port pins. An end that contradicts a pinned field is not skipped: it is an error at that wire, as for any pinned field (§4). So the choice stays a function of the wiring alone.
- **`map`:**
  - the content end is the port that `map[p]` names, or `p` itself when the map has no entry for it;
  - `map = { p = false }` leaves no content end, so the port is typed by the host side or by the default;
  - several slot outputs mapped to one content output (mono to `l` and `r`) each infer that output's type;
  - two slot inputs onto one content input stay an error (ADR 0037 §4), whether typed or auto.
- **Fan-out and fan-in:**
  - an input fed by several host sources, or an empty slot's output feeding several consumers, is defined by the first end of known type;
  - the other known ends must agree with it in shape (§4);
  - their rates may differ, because each wire is judged as an ordinary wire once the slot is loaded (§4). A signal or vector gets a converter where one rate divides the other. **Events have no converter** (`crates/signal-graph/src/convert.rs`), so events at another rate are `RateMismatch` at the wire.
  - This is the brief's "the source side wins": the side that sends defines the type, and converters are inserted as usual where a signal's or vector's rate differs.
- **Own insides** ("Editable insides"): the exports of a slot's own inside are named after its ports, so they are the content end, as for any composite.
  - An auto input of an empty own inside is typed by the host's wire. So the frame's pin inside shows what arrives from outside.
  - An auto output becomes the inner source's type as soon as something inside is wired to the frame.
- **Nested content** (a slot inside loaded content) is resolved at its own level first, as `resolve_slots` already does until the definitions stop changing. The content's exports are then known types. A project cannot load its own host (ADR 0035 §8), so levels never depend on each other in a circle.

### 3. Order of resolution: five stages, no cycles
The ends of one level are resolved in a fixed sequence. An end that is an auto port counts as **known** only once an earlier stage has resolved it from an end, so no stage reads its own result:

| Stage | Resolves | From |
|---|---|---|
| 1 | outputs of loaded slots whose port matched | the content's source port (always known: a content port is never auto at this level) |
| 2 | inputs with host sources | the first known source: a module port, a typed slot port, or an output resolved in stage 1 |
| 3 | inputs still open, of loaded slots whose port matched | the content's target port |
| 4 | outputs still open | the first known consumer: a module port, a typed slot port, or an input resolved in stage 2 or 3 |
| 5 | inputs still open | the first source resolved in stage 4 |
| — | everything still open | the default (not a known end, §2) |

The stages are the definition; §2's priority list reads them in order. Where the two could differ (an input whose host source is known only from stage 4, while its content defined it in stage 3), the earlier stage wins.

- **Why there is no cycle:**
  - stages 1 and 3 read only content ports;
  - stage 2 reads outputs resolved in stage 1;
  - stage 4 reads inputs resolved in stages 2 and 3;
  - stage 5 reads outputs resolved in stage 4;
  - nothing reads stage 5's results.
  - Each stage reads only earlier stages, so the dependencies follow the stage order, a chain. Each port is resolved once, so the result does not depend on the order of the entries in the file or on the order in which ports are visited. It is a pure function of the level's text and the content's types.
- **Stage 5** completes an input whose only sources are the outputs of empty slots that stage 4 typed from another consumer, so that the two ends of a valid wire never disagree only because one of them defaulted. Example: two empty auto slots `a` and `b` and a typed events input `seq.reset`, with `a.out → b.in` and `a.out → seq.reset`:
  - stages 2 and 3 leave `b.in` open (its only source is not known, and `b` is empty);
  - stage 4 makes `a.out` events, from `seq.reset`, its only known consumer;
  - stage 5 makes `b.in` events. Every end agrees, and the wiring is valid, as it should be.
- **Two slots wired to each other, both auto, both empty** (`a.out → b.in`): `b.in`'s only source, `a.out`, is not resolved in stage 1 (empty); `b.in` has no content (stage 3); `a.out`'s only consumer, `b.in`, is not known at stage 4; at stage 5, `a.out` is still unresolved. **Both get the default**, and neither is a known end for the other.
- A chain through an empty auto slot (`osc.out → a.in`, `a.out → b.in`, `b` loaded with an events input):
  - `a.in` is `osc.out`'s type (stage 2);
  - `b.in` is the content's events (stage 3);
  - `a.out` takes `b.in`'s type (stage 4).
- **Text stays the truth (ADR 0009):**
  - inference reads the files and the registry, nothing else;
  - nothing inferred is written back by any automatic process, and no inferred type is remembered between builds;
  - a value reaches the file only through an explicit pin (§8, §9).

### 4. Checks: pinned fields as today, inferred fields judged at the wires
- **A pinned field** (every field of a typed port, a written field of an auto port) is checked exactly as ADR 0037 §2 and §4 say:
  - against every host wire, an error at the wire, or for kind a warning;
  - against the matched content port, an error at `load` or the `map` entry.
- **An inferred field is not checked by the slot against the ends it connects.**
  - A loaded slot's wires on a matched port become ordinary wires from the real source to the real target (ADR 0037 §5). The graph judges them as it judges every wire:
    - for signals and vectors (and a signal into a parameter), a rate that divides or is a multiple of the other gets a converter, which `GraphWarning::ConverterInserted` reports; events need equal rates;
    - any other rate, or another shape, is `GraphError::RateMismatch` or `ShapeMismatch`;
    - another kind gives `GraphWarning::KindMismatch`.
  - **Located at the wire as written:** `resolve_slots` already keeps the spans of renamed wires, and `locate.rs` places `ShapeMismatch` and `RateMismatch` by their flattened addresses (`connection_or_instance`).
  - **The message:** the graph's own text names flattened addresses (`/fx/osc.freq`, `crates/signal-graph/src/error.rs`). Phase 1 adds a mapping in `locate.rs` from a flattened end behind a slot back to the slot's port as written and the content's port name (Consequences), so the message reads `project.toml:21:8: error: /fx.in (auto, from /seq.notes) is events; it reaches moog_voice's hz, a signal`.
  - So an auto port is **transparent**: a slot with auto ports plans **byte-identically** to the explicit instance with the renamed wires, converters included, exactly as ADR 0037 §5 requires.
- **Ends that no renamed wire carries** (an empty slot, or an unmatched port) are checked among themselves:
  - every **known** host end of an auto port must have the shape of its defining end; open and defaulted ports take no part (§2). Otherwise it is an error at each wire that differs, naming the defining end ("`/fx.out` (auto) is a signal, from `/scope.in`; `/seq.tick` takes events");
  - rates are not compared there, because no wire of them plans.
- **Conflicts** between the two sides (the host sends events, the content takes a signal) are therefore **errors at the wire**, as mismatches are today. A save that has one is refused, and the last good plan keeps playing.
- **A Replace's middle version** (the slot empty, ADR 0037 §6) is checked by the same rules. A defaulted port is no known end, so emptying a slot only removes known ends: it never adds one that disagrees. Every end the middle version types comes from a known port that the full versions have too. So a Replace whose old and new versions both build is not refused by the agreement check of its middle version (tested, test 3).
- **One guarantee of ADR 0037 §2 is narrowed:** "loading content can never make the host's own wiring invalid" now holds for typed ports and pinned fields only.
  - An auto output takes the content's type, so loading content whose `out` is a signal can make a host wire into an events input an error.
  - That is what typing by connection means. A person who wants the guarantee pins the field.
- **Warnings:** an inferred kind gives no kind warning of its own (the graph warns on the renamed wire, once). The "did you mean `map = …`" warning of ADR 0037 §4 still compares kinds: an auto port's kind is the inferred one.

### 5. Templates
- **Library templates** (`lib.voice`, `lib.instrument`, `lib.deck`, and ADR 0044's `lib.poly_voice`) **have no auto ports.** Their types are the contract a content is made to fit, and they are pinned by text hash and never changed incompatibly (ADR 0037 §3). `crates/signal-lib/tests/library.rs` checks that no library template declares `{}` or `auto`. The three existing ones declare every port with a kind, so they are typed and unchanged.
- **A slot `from` a template keeps the template's types.** Its own entries replace a template port as any retype does (ADR 0037 §2). So a slot may write `in = {}` to make one template port auto: a deliberate choice, written on the slot and shown.
- **Project templates** (`interfaces/<name>.toml`) **may have auto ports.** The operator's decision, with these reasons:
  - **The owner's answer 2 to ADR 0037:** a slot's interface is its own, and a project template is only a starting point the project owns. A project template of names and directions ("a bus with `in` and `out`") is a natural use. Each slot from it infers its own types, which is what such a template means.
  - **"Save interface as template…" round-trips:** it writes the slot's ports as declared, auto ones as `{}` (or `auto = true` with what is pinned). Pinning them all would turn a person's auto ports into a contract they did not choose.
  - **`{}` means one thing in every file a project writes.** Only the library's templates, which are not the project's, differ.
  - A project that wants a contract writes typed ports in its template, as today.
- **A risk this brings, accepted (the coordinator, 2026-10-29):**
  - the UI's `declaration()` (`ui/lib/slot.js`) writes `{}` for a plain generic signal port, and `ui/lib/slot.test.js` expects it (`'l = {}'`);
  - "Save interface as template…" and "Make interface from this instance…" write templates with it (`templateText`, then `file.write`, `ui/controls/schematic-editor.js`);
  - so a project template written by those tools before this change, holding `{}`, becomes auto.
  - No `interfaces/` folder exists in the repository, nor, by the reviewer's search, under `~/projects`, `~/Documents` or `~/Music`.
  - Phase 1 changes `declaration()` to write `"generic"` for such a port, so the tools never again write `{}` for a typed port, and updates its test (test 6).

### 6. Polyphonic slots (ADR 0044)
- `voice` is a role, not a type, and it is never inferred.
- **For now, every port with a `voice` field must be typed** (an error at the field otherwise), whatever its role:
  - `each` and `alloc` ports have types fixed by the allocator (`each`: signal at rate 1; `alloc`: events), and ADR 0044 §2 refuses host wires on `each` always;
  - a `sum` output's source in the plan is a generated `std.add` (ADR 0044 §5), not the content's port, so §2's content end would name the wrong port;
  - ADR 0044 is not built yet (hard rule 8).
- **Auto `shared` inputs and `sum` outputs are deferred to ADR 0044's phase 1**, which can decide them with the generated composite in hand.
- This amends ADR 0044 §2, which is not built yet.

### 7. Live edits
- **An inferred type never reaches the plan**, so its change sends nothing of its own. What is sent is the edit that caused it:
  - a **rewire** is an ordinary edit: one swap, survivors keep their state, and `sources_changed` marks the input whose sources changed (ADR 0037 §6);
  - a **load change** is a Load, an Unload or a Replace, by phase 2's rule.
  - This is exactly how a retype of a typed port already behaves: an interface edit that changes no match plans byte-identically and sends nothing (ADR 0037, phase 2 notes). An auto port's match depends on its name and `map` only, so inference never changes a match.
- **Phase 2's Replace rule** (`Project::slot_content_plan`) plans the slot's content alone under the slot's name with the values set on it, without the interface or the host's wires (verified in `slot.rs`). So inference never turns a load change into a Replace, or a Replace into an ordinary edit.
  - The middle version of a Replace (the slot empty) is checked as any project. Both of its swaps are prepared before anything is sent, so a middle version that does not build refuses the save, and nothing is sent.
- **The risk: a distant rewire retypes a slot.** Moving `/fx.in`'s source from an oscillator to a pattern's events output makes `/fx.in` events. Replacing the content of `b` retypes an empty `a.out` that feeds `b.in`. What happens then:
  - a **shape** change that the other side cannot take is an error at the wire, and the save is refused, so it is never silent;
  - a **rate** change of a signal or vector is not an error: a converter appears where the wire crosses rates, and `GraphWarning::ConverterInserted` reports it. It plays the rewire as written, as for any wire, but the person may not have expected it. Events at another rate are refused (`RateMismatch`);
  - kind, unit and range change what is shown.
- **The mitigations:**
  - **pinning:** any field, in the dialog's Advanced part (§8), through MCP (§9), or by hand;
  - **the editor shows inferred values and where they come from**, greyed (§8);
  - **the change is reported:** `reload` and the `patch` reply gain `retyped`, which lists the auto ports whose inferred shape or rate changed with the edit (`[{slot, port, was, now, from}]`, additive). It compares the version before the edit with the final one, never with a Replace's middle version. The UI shows it as a notice ("`/fx.in` is now events (auto, from `/seq.notes`)");
  - converters, as today, are visible in the plan and the schematic (ADR 0005).

### 8. UI: the interface dialog and the slot box
- **Edit interface…** (`ui/controls/slot-dialogs.js`, rows from `ui/lib/slot.js`) shows each port's **name** and **direction** (input, output, parameter). Beside them, a greyed summary of the effective type:
  - **auto:** "auto: signal · rate 1 · from `/osc.out`", or "auto: events · from the content's `notes`", or "auto: nothing connected (signal · rate 1)";
  - **typed:** its fields, not greyed ("audio · signal · rate 1").
- **"Advanced"**, collapsed by default (its fold state is view state), holds `shape`, `rate`, `kind`, `unit`, `range` and `group`, each with a **pin**:
  - pinned, the field is editable;
  - unpinned, it shows the inferred value, greyed;
  - pinning starts from the inferred value; unpinning a field of a typed port makes the port auto with **every other field pinned at the value it had** (§9), so only the named field becomes inferred;
  - "Pin all" pins every field at its inferred value; "Unpin all" leaves the port `{}`, or `{ auto = true, group = … }` if it has a group.
- **New ports are auto:**
  - the "+" pins and "+ input"/"+ output" add `{}` (today they add an `audio` signal, `newRow`);
  - the Load dialog's "Add its unmatched ports to the slot" adds them as `{}` as well: they infer the content's types and match by name, and a later Replace retypes them with the new content.
  - "Make interface from this instance…" keeps writing typed ports: it is an explicit copy of a concrete instance's types. From phase 1 its writer, `declaration()`, writes a plain generic signal port as `"generic"`, not `{}` (§5).
- **The schematic and the slot card:**
  - a pin's tooltip says "auto" and where the type comes from (`in: auto, signal, from /osc.out`);
  - the `<signal-slot>` card's port rows show the effective kind, with "(auto)";
  - a box draws an auto pin as it draws a typed one: the type it has now.

### 9. Protocol, `describe` and MCP
- **`describe` (`content.slots`) and `graph` (`slot.interface.ports`)** keep giving each port its effective `shape`, `rate`, `kind`, `unit` and `range`, so every client reads them as before. An auto port adds (additive):
  - `"auto": {"inferred": ["shape", "rate", …], "from": {"end": "host" | "content" | "default", "port": "osc.out" | null}}`. For a slot inside a composite, `port` is qualified by the level (`rack/osc.out`), as `SlotResolution.slot` is;
  - `inferred` lists only the fields not pinned.
- **`slot.interface`** (`ui-protocol.md` "patch") and the MCP tool **`slot_interface`** (`agent-interface.md`):
  - `add` entries take `auto: true`: an auto port, whose given fields are pinned. Without `auto`, an `add` means what it means today, a typed port with the defaults, so existing clients are unaffected;
  - `retype` entries take **`unpin: ["shape" | "rate" | "kind" | "unit" | "range", …]`**: only those fields become inferred. On a typed port, the port becomes auto and **every other field is written pinned at its previously effective value**, unit and range included (as `unit = ""` and `range = []` where it had none, §1). Unpinning `kind` of `"audio"` writes `{ auto = true, shape = "signal", rate = 1, unit = "", range = [] }`. A field given a value in the same entry is pinned; a field both given and unpinned is `bad-value`;
  - the inverse (`slot.restore`) gives back the entry's text byte for byte, as today.
- **The canonical text the writer produces:**
  - a port with no inferred field is written typed, as `port_text` writes it today;
  - an auto port with nothing pinned and no group is written `{}`;
  - any other auto port is written `{ auto = true, <pinned fields>, group = … }`, with a pinned `shape = "signal"` or `rate = 1` written out.
- **`signal schema`** (`project-format.md` §8) accepts `auto` in a slot port's table.
- **"Detach from template"** writes the resolved interface inline as declared: auto ports stay auto.

### 10. Backwards compatibility
- **Every typed declaration means exactly what it did:** the short forms, and every table without `auto` that writes a field. This covers everything the server-side writer has written (`port_text` never writes `{}` for a port) and everything in the repository (verified with `git grep`: the slots of `projects/two-decks` name `lib.deck`, and the three library templates write a kind on every port).
- **The one change of meaning is a port written as the empty table `{}`.** It was a generic signal at rate 1 and is now auto, in a slot's interface and in a project template alike. This is the form the owner accepted, and the change was accepted with it (the coordinator, 2026-10-29). No file in the repository has one.
  - **The UI's tools have written it, though:** `declaration()` writes `{}` for a plain generic signal port in the templates that "Save interface as template…" and "Make interface from this instance…" write (§5). Such a template written before this change becomes auto. None was found in the folders searched (§5); *not verified* beyond them. Phase 1 makes `declaration()` write `"generic"`.
  - For **every project that loads today**, the inferred shape and rate of such a port are a signal at rate 1. Today a loading project's wires and matched content on it are all signals at rate 1 (exact checks), and the stages of §3 copy one of them, or fall to the default. So **every plan is byte-identical**.
  - What can differ: the kind, unit and range shown (now the defining end's), the kind warnings (now the graph's, on the renamed wire), and what a later rewire does (it retypes instead of being refused).
- **Goldens and hashes:**
  - no file in the repository changes, so no golden audio, log or manifest changes;
  - `library_interface_hashes` are unchanged, because the library templates are not touched.
- **ADR 0037's wording** "a set of typed input and output ports" (vision §3a, "Interface") becomes "a set of input and output ports, each typed or typed by its connections, and parameters" when this ADR is accepted.

### 11. Where it lives (hard rule 1)
- **All of it is in `signal-project`**, before `build_plan`, as slot resolution is.
- **`signal-graph`'s `ports_of` gains `unit` and `range` on `ModulePort`:**
  - copied from the primitive's `PortDescriptor`, which already has them;
  - for an export, from the port behind it.
  - These are domain-free fields that the core already carries, so no new word reaches the core, the graph or the engine.
- The engine and the oracle are untouched.

## Alternatives considered
- **Every unwritten field of a table is inferred** (so `{ kind = "audio" }` pins the kind alone): the most regular rule. Rejected: every table the tools and the templates already hold would change meaning (§1).
- **Only an explicit marker** (`in = "auto"`, or `{ auto = true }` with `{}` keeping its old meaning): fully compatible. But it is not the form the owner accepted, and `{}`'s old meaning ("a generic signal") is better written `"generic"`, which is what the tools write. `in = "auto"` as a further short form can be added later if wanted. It is a third spelling of one thing, so it is not added now.
- **Unification** (the most specific type that agrees with every end, as a type checker infers it): more general. But a conflict then belongs to no single wire, and "where does this type come from" has no one answer to show. The priority rule names one defining end, which the UI shows and a person can reason about.
- **Inputs defined by the content first:** the content would retype the host's inputs whenever it is replaced, while the host's wiring stays the same. The host is what sends, and the content is what is swapped, so the host side wins for inputs and the content for outputs.
- **Writing inferred types into the file** (materialising them): the text would change on edits elsewhere, and a person's file would churn on every rewire. Pinning is the explicit form.
- **A slot port of no type at run time** (the engine adapts): a new runtime concept, against hard rule 1 and ADR 0037 §5. Resolution at plan time gives the same result.
- **Fewer kinds, or kinds folded into units:** out of scope (ADR 0005). Recorded as a consideration in the Context.
- **No auto ports in project templates:** simpler to state, and a template would always be a contract. Rejected for the reasons in §5. It can be revisited if a project shows a template's auto port confusing its slots.

## Consequences
- **The format:** `auto` in a slot port's table; `{}` an auto port (`project-format.md` §14, §8 schema).
- **`signal-project`:**
  - `model.rs`: `PortSpec` records which fields were written, and whether the port is auto;
  - `slot.rs`: the five stages of §3 in `resolve_def` (per level, inside `resolve_slots`' existing loop); the per-field checks of §4; the agreement check among unplanned known ends; the error text naming the slot port and the defining end; `InterfacePort` gains `auto`; an error for a `voice` field on an auto port;
  - `locate.rs`: a mapping from a `ShapeMismatch` or `RateMismatch` end behind a slot (a flattened address) back to the slot's port as written and the content's port name, for §4's message;
  - `patch/slot.rs`: `add.auto`, `retype.unpin`, the canonical writer of §9, `retyped` in the reply;
  - `schema.rs`; `live.rs` (`retyped` on `reload`, computed by comparing the `SlotResolutions` before and after).
- **`signal-graph`:** `ModulePort.unit`, `ModulePort.range`.
- **`signal-lib`:** a test that no library template has an auto port.
- **The plan:** unchanged in kind. A slot with auto ports plans as the explicit instance does; converters appear where an auto port joins two rates.
- **`signal-ui` and `signal-cli` (`signal mcp`):** `describe`, `graph` and `slot_interface` carry the new fields.
- **`ui/`:**
  - `ui/lib/slot.js`: `interfaceRows` with pinned and inferred fields, `newRow` auto, `portJson` and `interfaceOp` with `auto` and `unpin`, `declaration` and `templateText` for auto ports, the summary text. **In phase 1 already:** `declaration()` writes `"generic"`, not `{}`, for a plain generic signal port, and `ui/lib/slot.test.js` changes with it (§5);
  - `ui/controls/slot-dialogs.js`: the summary column and the Advanced part with pins;
  - `ui/controls/slot.js` and the schematic's pin tooltips.
- **Docs:**
  - `project-format.md` §14 (auto ports, inference, the checks);
  - `agent-interface.md` (`slot_interface`);
  - `ui-protocol.md` (`auto` on interface ports, `add.auto`, `retype.unpin`, `retyped` on `reload` and in the `patch` reply);
  - `module-contract.md` "Interfaces" (library templates are typed);
  - `00-vision.md` §3a "Interface";
  - `roadmap.md`, the M3.2 row (slots), naming this ADR and its phases;
  - ADR 0037's Notes (§3's sentence on templates' declared types amended) and ADR 0044 §2, pointers.
- **Goldens:** none change (§10).

## Tests
1. **Resolution** (`crates/signal-project`, a new `tests/slot_auto.rs` and unit tests in `slot.rs`):
   - each stage of §3;
   - an input from its host source, with the content port as fallback; an output from the content, with host consumers as fallback; the default;
   - fan-in at several rates (the first `from` string by `str::cmp` defines it); two wires from one source count as one end;
   - an empty output's fan-out;
   - `map` (rename, `false`, mono to `l` and `r`);
   - an own inside, before and after a frame wire;
   - nested content;
   - unit and range copied from a primitive, through an export, and from an export's own range (ADR 0042); none from tap channels;
   - a parameter as a consumer is no known end; a partly pinned open auto port is no known end;
   - `{}` under `parameters` keeps its meaning (a parameter with no unit and no group).
2. **Cycles and order:**
   - two empty auto slots wired to each other give the default;
   - the triangle of §3 (`a.out → b.in`, `a.out → seq.reset`, events): stage 5 types `b.in` events, and the project builds with no agreement error;
   - the chain of §3;
   - a slot wired to itself;
   - the same project with its TOML entries permuted resolves identically.
3. **Conflicts and checks:**
   - host events into an auto input whose content takes a signal: an error at the wire, naming both;
   - a content output's shape against a host consumer;
   - disagreeing host ends on an empty slot: an error at the differing wire;
   - a rate that divides plans with a converter, byte-identical to the explicit form;
   - a rate that does not divide: `RateMismatch` at the wire;
   - events at different rates through an auto port: `RateMismatch` at the wire (no converter);
   - an end that contradicts a pinned field is an error at that wire, not skipped;
   - the error text names the slot's port and the content's port (the `locate.rs` mapping);
   - **a Replace's middle version:** a Replace whose old and new versions both build, where the slot's emptying leaves another slot's auto port defaulted, is not refused by the agreement check (`slot_live.rs`);
   - pinned fields of an auto port and every typed port are checked exactly as before (ADR 0037's tests unchanged);
   - `auto = false`, `auto` on a parameter, in a library template (`crates/signal-lib/tests/library.rs`), and a `voice` field on an auto port: errors at the key.
4. **Equivalence and compatibility:**
   - a slot with auto ports plans byte-identically to the same slot with the inferred types pinned, and to the explicit instance;
   - every repository project and fixture plans byte-identically before and after;
   - a `{}` port under every valid wiring of today plans as before;
   - a project template holding `{}` makes an auto port in every slot `from` it, each inferred from its own wiring.
   - every golden unchanged.
5. **Live** (`slot_live.rs`, null driver, seeded callback sizes, 1 and 4 threads):
   - a rewire that changes an auto input's inferred rate sends one ordinary swap with the converter, survivors bit-continuous, `sources_changed` set (phase 1); `retyped` reported, comparing the version before with the final one (phase 2);
   - a Replace that retypes another slot's empty auto output is still unload-then-load, and the retype adds no swap;
   - a rewire that makes a shape conflict is refused and nothing is sent.
6. **Edits:**
   - `slot.interface` `add` with `auto`, `retype` with `unpin`, pinning and unpinning each field, written in the canonical form of §9 with comments kept;
   - the inverse gives the text back byte for byte;
   - "Save interface as template…" and "Detach" keep auto ports auto;
   - unpinning one field of a typed port pins every other field at its previous value (`unit = ""`, `range = []` where it had none);
   - MCP `slot_interface` the same (`crates/signal-cli/tests/mcp_*.rs`).
7. **UI** (`node:test`):
   - `interfaceRows`, `portJson` and `interfaceOp` with auto, pinned and unpinned fields;
   - `templateText` / `declaration()`: a plain generic signal port is written `"generic"`, never `{}`, and an auto port `{}` (phase 1 for the first);
   - the summary text;
   - new rows auto;
   - the Advanced part folds and pins;
   - `graph`'s `auto` field read by the box and the card.
8. **The operator's visual check** of the dialog, and **the owner's usability verdict.**

## Phases
| Phase | Delivers | Exit criteria |
|---|---|---|
| 1. Format and resolution | `auto` and `{}`, the stages of §3, the checks of §4 with the `locate.rs` mapping, `ports_of` unit and range, `describe`'s and `graph`'s `auto`, library-template test, `declaration()` writing `"generic"`, `project-format.md`, the roadmap row | Tests 1–4, 5 without `retyped` (no new live code: the inferred type never reaches the plan), 7's `declaration()` case; every golden unchanged |
| 2. Editing | `slot.interface` `add.auto` and `retype.unpin`, the canonical writer, `retyped` on `reload` and in the `patch` reply, MCP, `ui-protocol.md`, `agent-interface.md` | Test 6; test 5's `retyped` |
| 3. UI | The summary, Advanced with pins, auto new ports, tooltips, the card | Test 7; the operator's visual check |
| 4. Verdict | — | The owner's verdict (test 8) |

## Notes

**2026-10-29, phase 1 built** (operator; decisions delegated). As the Phases table says, with these readings where the text left a choice:
- **`from.port` of a host end** is qualified by the level as `SlotResolution.slot` is: `/osc.out` at the root, `rack/osc.out` in a composite (§9's example `"osc.out"` read as that rule's root case, which §8's `from /osc.out` also writes). A content end is the content's port name (`notes`); the default `null`.
- **The agreement check** (§4) runs for an auto port whose shape is inferred, defined by a host end, and not carried by a renamed wire; each other known host end of another shape is an error at its wire, at the slot's endpoint as written. A pinned shape is checked as every pinned field is. **One error per wire:** when both ends of a wire are slot ports and both sides find it wrong (a pinned or typed check from each side, or an agreement check and a pinned one), only the input's side reports it; an output's side reports only where the input's side found nothing.
- **A wire whose other end is an auto port no end typed** (open or defaulted) is not compared by a typed port's or a pinned field's check either: that end is no known end (§2). For every project that loads today nothing changes, since a `{}` other end was a generic signal at rate 1 and only such wires loaded.
- **The `locate.rs` mapping** follows a renamed wire's ends through the content's exports to the flattened ports of `ShapeMismatch` and `RateMismatch`, so an error behind a composite or project content is located at the host's wire as written (it fell back to the inner instance's key before), and its message names the slot's port, how it is typed and what it reaches: for an input, the content as `load` writes it and its port (`thru's x`, `../deck's x`, as the other slot messages name the content); for an output, the consumer qualified by the level. A slot in a composite is named by its level (`rack/inner.x (auto, from rack/e.rise)`) and located at the composite's wire. Graph warnings on such wires (`ConverterInserted`, `KindMismatch`) are located there too; their text is the graph's.
- **`voice` (§6):** ADR 0044 is not built, so no port reads a `voice` field; one is an unknown field, an error at the key, on an auto port as on any.
- **Phase 2's writer, the minimum phase 1 needs:** today's interface edits must not turn an auto port typed, so `port_text` writes an auto port as §9's canonical text (`{}`, or `{ auto = true, <pinned>, group }`; one with nothing left inferred is written typed), a rename and "Detach" keep it auto, and a `retype` of an auto port keeps it auto with the fields it gives pinned. `add.auto`, `retype.unpin` and `retyped` are phase 2's.
- **The UI pins nothing it did not change** (hard rule 4): the interface editor's rows keep a port's `auto`, and a retype of an auto row sends only the fields the person changed against the row's initial values, never an inferred one (`ui/lib/slot.js` `autoRetype`). `declaration()` writes an auto port as `{}` or `{ auto = true, <its pinned fields>, group }`, so "Save interface as template…" keeps it auto (§5); a typed plain generic signal is `"generic"`. The rest of the editor (summary, Advanced, pins, new ports auto) is phase 3's.
- **`range = []` on a typed port** is accepted and means no range, as an unwritten `range` does (§1 defines it for auto ports; a typed port has the same reading, so the writer's text and `signal schema` need no special case). `unit = ""` on a typed port stays an empty unit, as before.
- **The library-template refusal** (§5) is located at the port's key in the template (`hz = {}`), the entry that is auto, with the message naming the port.
- **Test 5's `sources_changed`** is observed by a content primitive (`test.watch` in `slot_live.rs`) that outputs 1 on the first frame of its first call after the rewire's swap.
- **Test 3's Replace middle version** (`slot_live.rs`): the empty slot's auto output feeds both the replaced slot's events input and an events input of a module, so the middle version types it from the module (the emptied input is open, no known end). Counting an open end as a known default makes that middle version fail the agreement check (checked by breaking the rule).

## Decided after the review (2026-10-29)
- **New ports are auto** (§8): the owner's idea, confirmed by the coordinator. Today a "+" pin adds an `audio` port; after this ADR it adds `{}`, which shows "auto: nothing connected" until it is wired.
- **`{}` changes meaning everywhere, with no special case for `interfaces/`** (§1, §5, §10), the coordinator's decision; `declaration()` stops writing it for typed ports.
- **Unpinning one field of a typed port pins the others at their previous values** (§9), the coordinator's decision.
- **A defaulted port is no known end, and stage 5 exists** (§2, §3), after the review found that a defaulted end gave false conflicts.
- **Ports with a `voice` field are typed until ADR 0044's phase 1** (§6).
