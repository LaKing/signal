# ADR 0037: Slots

- **Status:** Accepted (2026-10-23). The owner's words of 2026-10-23 (translated): "There could be a composite-like module, I don't know what to call it: an empty shell-like module whose outputs and inputs are defined but has nothing inside. When we load something into it, it automatically connects with the matching connections…" The owner's answers of the same day to the draft's questions:
  1. the name **slot** is accepted;
  2. "every slot should have a configurable, editable, arbitrary number of inputs and outputs": a slot's interface is its own, declared on it and editable; interface files and the library interfaces are templates it may start from (§2, §3);
  3. "think in terms of a control-signal-like system, i.e. a function of the wiring": what a playhead does when following stops is a parameter that can be wired, not a fixed policy (§6).
  Read with the owner's earlier answers of that day: a whole project can be brought in as a composite (ADR 0035); mixing is done by a mixer module or composite; a live join only has to be glitch-free; an include's panel edits are written into the host (`set`); the audio interface is the one clock (ADR 0036 §1); everything is bound to signal sources; and automatic connection is preferred to a wireless broadcast, because it handles inputs and outputs alike. The rest is the operator's design, delegated: the template syntax (`from`, `remove`), matching by exact name, Replace as unload-then-load, and `release`'s default and threshold.
- **Builds on:** ADR 0005 (ports), 0006 and `semantics.md` §12 (swaps, survivors, fades), 0009 (text is the truth), 0016 (connections), 0026 and 0030 (schematic), 0027 §4 (library composites, pinning, `signal vendor`), 0033 (`is_connected`, exclusive inputs, `lib.transport`, `follow`/`reset`), 0035 (a project as content), 0036 (the device's clock).
- **Replaces** the drafts "Broadcast signals" and "Automatic connections" as the main model. Both are now alternatives (below). Their sound parts are kept: `Inputs::sources_changed`, the rate edits to ADR 0035, and the determinism and ambiguity reasoning.
- **Supersedes nothing accepted.** Amended on 2026-10-27 by "Editable insides" (the owner's request and answer, at the end).
- **Date:** 2026-10-23

## Context
- **Use cases:**
  - two studios mixed in one host;
  - swapping the instrument a sequencer plays (today's `moog_voice`, a future CA-72 voice with the same interface);
  - an included project whose sinks become taps (ADR 0035 §3) and must reach the host's mixer;
  - in every case, the host's transport optionally driving the content.
- **What exists today:**
  - a composite instance's ports are its exports, and they only exist once the composite is chosen;
  - wiring therefore names a definition's export names, so swapping the definition means rewriting the wires;
  - a definition's exported ports have no declared kind, shape or rate: they are derived from inner ports (`def.rs`).
- **What the owner asks for:** an instance whose ports exist before, and independently of, what is loaded into it, in any number the person chooses. Loading connects by matching, and the host's wiring stays written and visible.

## Decision

### 1. Name: **slot**
- "Slot" is the word musicians already use (plugin slot, insert slot), short in TOML (`slot =`), and free in Signal's vocabulary. Accepted by the owner.
- Rejected names:
  - "socket": a patching word for a port's jack, and a network word;
  - "shell": a Unix word.
- **Terms**, added to vision §3a as kinds of existing terms rather than new structural concepts:
  - **Interface:** a set of typed input and output ports and parameters, with nothing inside. Every slot has its own; a named one in `interfaces/` or the library is a template.
  - **Slot:** an instance whose ports and parameters are its own interface, editable, and whose module is whatever its `load` names (or nothing).

### 2. A slot is an instance with its own interface and optional content
The slot's interface is **its own**: declared on the slot, with any number of inputs, outputs and parameters, and edited there, in the text or in the schematic. A named interface (§3) is only a template it may start from.

```toml
[instances]
fx     = { slot = { inputs = { in = "audio" }, outputs = { out = "audio" } } }
deck_a = { slot = "lib.deck", load = "../studio-transport" }
deck_b = { slot = { from = "lib.deck", inputs = { cue = "gate" }, outputs = { cue_l = "audio", cue_r = "audio" } },
           load = "../studio", seed = 3 }
synth  = { slot = "lib.voice", load = "moog_voice", set = { cutoff = 400 } }
spare  = { slot = "lib.voice" }                                    # empty
pad    = { slot = "lib.instrument", load = "moog_voice", map = { l = "out", r = "out" } }
blank  = { slot = {} }                                             # no ports yet

[instances.bus_in]                     # the same, as a table, for a long interface
load = "parts/bus"
[instances.bus_in.slot]
from       = "deck"                    # interfaces/deck.toml
remove     = ["tempo"]
inputs     = { position = "control", sync = "events" }
parameters = { volume = { unit = "dB", group = "out" } }
```

**The `slot` key** (required):
- **an inline table** declares the interface: `inputs`, `outputs` and `parameters` with the port and parameter fields of §3. `slot = {}` is a slot with no ports, valid, to which ports are added later;
- **`from`** (optional, in the table) names a template (§3). The slot's interface is the template's, then:
  - `remove = [names]` drops template ports or parameters;
  - each entry in `inputs`, `outputs` and `parameters` adds a port, or replaces the template's port of that name with the new declaration (a retype);
  - a name both in `remove` and in a table is an error, and so is a `remove` name the template does not have;
- **a string** is short for `{ from = "<name>" }`: `slot = "lib.deck"`.
- **The template is a reference, not a copy.** A project template edited in `interfaces/` reaches every slot that names it, as a live edit; a library template never changes incompatibly (§3). "Detach from template" in the editor writes the resolved interface inline and removes `from`.

**Editing the interface** writes the slot's entry with `toml_edit`, keeping comments, as the parameter panel writes `project.toml`. In the schematic (slot box context menu, Edit interface…) and through MCP (`slot.interface`, §7):
- **add** a port or parameter: an entry in `inputs`, `outputs` or `parameters`;
- **remove**: the entry is deleted, or, for a template port, its name is added to `remove`;
- **retype** (shape, rate, kind, unit, range, group): the entry is written or replaced;
- **rename**: remove the old name and add the new one, and rewrite in the same write every host wire, `map` key and panel binding in the host's files that names the old port, so nothing dangles; the result is reported;
- removing a port that wires still name is refused, naming the wires, unless the person asks to remove them too;
- every edit is a live edit: the slot's interface is re-resolved, the content re-matched (§4), and the plan swapped (§6), with survivors keeping their state.

**The other keys:**
- **`load`** (optional): the content, one of:
  - a module reference: `std.x`, a bare composite, or `lib.x`;
  - a project: a folder or a `.signal` file, per ADR 0035 §1.
  - How the two are told apart: a value containing `/` or ending in `.signal` is a project path. Every other value is a module reference, since module references never contain `/`. A project in a subfolder is written `./bass`, which is what the GUI writes.
- **`map`** (optional): `{ <slot port or parameter> = "<content name>" | false }`, overriding the match (§4).
- **`seed`** and **`set`** (optional): as ADR 0035 §1.
  - `set` keys are the content's parameters outside the interface: `"<inner>.<param>"` for a project, an exported parameter name for a composite.
  - The panel writes them there (ADR 0035 §10).
- **Interface parameters** are set by name (`volume = -6`). They reach the matched content parameter, with the outermost value winning as usual.
- **Reserved keys:** `slot`, `load` and `map` join `gpu`, `pattern`, `project`, `seed` and `set`; `from` and `remove` are reserved inside the `slot` table, so they cannot name a port. `gpu` and `pattern` on a slot are errors, as on a composite instance.
- **Where slots may appear:** in `[instances]` of `project.toml` and of project composites.
  - A slot inside a composite has that composite file's interface and `load`, the same for every instance of the composite.
  - Library composites contain no slots in v1, so a library update can never change content.
- **Wiring is explicit, as for any instance:** `{ from = "transport.position", to = "deck_a.position" }` and `{ from = "deck_a.l", to = "mix.a_l" }`.
  - The host's text and schematic show every wire, whether or not the slot is loaded.
  - These wires are checked against the slot's **interface**, always, also when the slot is empty:
    - an undeclared port is an error;
    - a shape or rate mismatch is an error;
    - a kind mismatch is a warning.
  - So loading content can never make the host's own wiring invalid. It can only fail to match (§4).

### 3. Ports, parameters and templates
- **Port fields** (on a slot and in a template alike):
  - `shape`: `"signal"` (default), `"events"`, or `{ vector = N }`;
  - `rate`: a divisor, default 1;
  - `kind`: as `PortKind`, default `generic`;
  - optional `unit`, `range`, `group` (presentation only, as for exports);
  - short form `name = "<kind>"` means a signal of that kind at rate 1; `"events"` means an event port.
- **Parameters** declare a name (optional `unit`, `group`). Range and default come from the content. A value set on an empty slot is kept in the text and applied to nothing.
- **Template files:** `interfaces/<name>.toml`, a new project-file folder (`project-format.md` §1). It is loaded, validated, hashed, packed, watched and copied into sessions like `composites/`. It is "a composite with only exports", except that ports declare their types, because nothing inside them could tell.
  ```toml
  schema_version = 1
  [interface]
  name = "deck"
  description = "A playing song: follows a position if given one; stereo out."
  [inputs]
  position = { kind = "control", unit = "beats", group = "transport" }
  jump     = { shape = "events", kind = "trigger", group = "transport" }
  running  = { kind = "gate", group = "transport" }
  tempo    = { kind = "control", unit = "bpm", group = "transport" }
  [outputs]
  l = { kind = "audio" }
  r = { kind = "audio" }
  [parameters]          # optional; names, optional unit and group
  release = { group = "transport" }
  ```
- **References:**
  - `from = "deck"` resolves to `interfaces/deck.toml`;
  - `from = "lib.deck"` resolves to a library template;
  - the namespace is the `slot` key's own and never collides with module names.
- **"Save interface as template…"** in the editor writes a slot's resolved interface to `interfaces/<name>.toml` and sets its `from`; "Make interface from this instance" writes one from any instance's resolved ports.
- **Library templates:** `crates/signal-lib/interfaces/<name>.toml`, read-only, listed by `signal modules --interfaces`.
  - **Pinning:** by text hash in `tests/pinned.toml`, with a `CHANGELOG.md` entry under the current MAJOR.MINOR. A template has no render, so its text is what is pinned.
  - **Never changed incompatibly:** a changed port set or type is a new name, as for library composites (ADR 0027 §4).
  - `signal vendor <project> lib.deck` copies one into `interfaces/`.
  - The manifest gains `library_interface_hashes`, absent when there are none.
- **Standard templates, phase 1 only, because the use cases need them:**

  | Template | Inputs | Outputs | Parameters | Matches today |
  |---|---|---|---|---|
  | `lib.voice` | `hz` (frequency, Hz), `gate` (gate), `velocity` (control, 0 to 1) | `out` (audio) | — | `moog_voice`, exactly, with no `map` |
  | `lib.instrument` | as `lib.voice` | `l`, `r` (audio) | — | a mono voice through `map = { l = "out", r = "out" }` |
  | `lib.deck` | `position` (control, beats), `jump` (events, trigger), `running` (gate), `tempo` (control, bpm) | `l`, `r` (audio) | `release` | any project through its default sink exports `l`, `r` (ADR 0035 §2) |

  - Both a mono and a stereo instrument template exist; since a slot's interface is editable, either is only a starting point.
  - Others (`lib.effect`, `lib.effect_stereo`) wait until a project needs them (hard rule 8).
  - The conventions are documented in `module-contract.md` "The library", under a new heading "Interfaces".

### 4. The matching rule (in `signal-project`, at plan time)
The rule is the same whatever the slot's interface came from: it matches the slot's **resolved** interface (template, then `remove`, then the inline entries).
- **Candidate names:** for each slot input, output and parameter *p*, the candidate content name is `map[p]` if present, `false` if mapped to `false` (deliberately unmatched), else *p* itself.
- **Content ports:**
  - a composite's or project's exports;
  - a primitive's descriptor ports and parameters;
  - for a project without `[exports] outputs`: its taps' channels, named `l` and `r` (ADR 0035 §2–3).
  - The resolved types of an export come from the graph. `signal-graph` gains a pure, domain-free query, `signal_graph::ports_of(definition, defs, registry)`, which returns each exported port's direction, shape, rate and kind (the derivation the flattener already does in `interface()`).
- **The checks:**

  | Case | Result |
  |---|---|
  | Name found, same direction, same shape, same rate | matched |
  | Kinds differ, neither `generic` | matched, kind warning |
  | Direction differs, or a port matched to a parameter | error |
  | Shape or rate differs | error |
  | `map` key that is not a slot port or parameter | error |
  | `map` value naming no content port | error |
  | Two slot inputs mapped to one content input | error (one export, two meanings) |
  | Several slot outputs mapped to one content output | allowed: outputs fan out (mono to `l` and `r`) |

  - Errors are located at the `load` value or the `map` entry, naming both sides: "project.toml:14:28: error: /deck_a: `../x` output `l` is a vector of 2; the slot's interface (from `lib.deck`) declares `l` a signal".
  - Matching is by exact name only. No library roles and no fuzzy matching, so what connects is decided by the text alone (Alternatives).
- **Unmatched cases:**
  - **Slot input or parameter not matched:** the host's wires into it are left out of the plan. The content does not get them ("not passed in").
  - **Content input not matched:** unconnected inside. `is_connected` is false and it reads 0, so the content uses its own fallback, for example its own transport (§6).
  - **Slot output not matched:** the host's wires from it are left out of the plan. The downstream inputs read 0 with `is_connected` false. For a sum, that is identical to a source of exact 0, and fallback idioms (`std.prefer`, sequencer `tick`) see "nothing there".
  - **Content output not in the interface:** still computed (ADR 0016), heard nowhere. The schematic marks it ("2 outputs not in this slot: unheard") and offers "Add to interface", which adds the port to the slot (§2).
  - **Diagnostics:**
    - unmatched ports are not warnings by themselves (a deck that keeps its own transport is normal);
    - `describe` and the inspector show the full match table;
    - a **warning** is given only when the slot is loaded and nothing at all matches ("`../x` matches none of `deck_a`'s ports"), or when a wired slot port is unmatched *and* a content port of the same kind and direction is unmatched too ("did you mean `map = { position = "clock" }`?");
    - a content that does not load is an error at `load`, located in its own file (ADR 0035 §1).

### 5. What the plan is: an ordinary instance (hard rule 1)
- **Resolution** happens entirely in `signal-project`, before `build_plan`. A loaded slot `deck_a` becomes an ordinary `InstanceDef { name: "deck_a", module: <content definition> }`:
  - a qualified definition name for a project (ADR 0035 §1);
  - the plain reference for a module.
- **Wires are renamed:** each host wire on a matched slot port becomes a wire on the content's matched name (`deck_a.position` becomes `deck_a.follow`, say). Unmatched ones are dropped.
- **An empty slot contributes no instance and no connection.** It exists in the text, the model, the schematic and `describe`, and not in the plan. It is valid and silent.
- **Consequences for the plan:**
  - Addresses inside are `/deck_a/...`, exactly as for `deck_a = { module = ... }`; there is no extra `/content` level.
  - The plan is **byte-identical** to the explicit form: the same content instantiated under the slot's name with the renamed wires. So canonical order, converters (`semantics.md` §16), exclusive and restricted inputs, cycles and `is_connected` all behave as for wires. The engine, the oracle and determinism are untouched.
  - The slot's interface itself never reaches the plan: an interface edit that changes no match and no wire gives a byte-identical plan.
  - Scope roots and seeds (ADR 0035 §4) apply to project content as to a plain include. Composite content draws `derive(host seed, address)`, as the explicit instance would.
- **No new runtime concept:** `signal-core`, `signal-graph` and `signal-engine` learn no word "slot". The only additions there are the domain-free `Inputs::sources_changed` (§6) and the domain-free `ports_of` query (§4). `domain_scope.rs` gains `slot`, `interface` and `load`.
- **The UI side table:** the build returns `SlotResolutions` (per slot: resolved interface and its template, content, the match table, the dropped wires) for `structure.rs`, the panels and `describe`. The plan carries no record of it.

### 6. Live: load, unload and replace are live edits; following is wiring
- **Load** (empty to X) is one swap at the edit frame (`semantics.md` §12):
  - X's instances are added, and their `audio` outputs fade in over F = 5 ms;
  - the host's wires to the slot connect at that frame;
  - surviving receivers see `is_connected` turn true;
  - X's sequencers start with the first-frame seek.
- **Unload** (X to empty): suspend X's instances at *s* and swap at *s* + F (`live.rs`'s existing protocol):
  - X's audio reaches exact +0.0 before its terms leave the host's sums;
  - non-audio outputs step at the swap, as for any removal.
- **Replace** (X to Y, a changed `load`) is **unload, then load**:
  - suspend X at *s*, swap to the plan with the slot empty at *s* + F, then swap to the plan with Y at *s* + F + 1;
  - Y fades in over F, so there is a 5 ms fade-out followed by a 5 ms fade-in, and no click: glitch-free, which is the owner's requirement (ADR 0035 §6);
  - no state carries over, even where X and Y share an address and type;
  - what is "the same content" is decided by the canonical `load` reference.
  - An edit *inside* the loaded content (its files) is an ordinary live edit with survivors (ADR 0035 §7).
  - A musical crossfade between two contents is two slots and a mixer (`lib.mixer`, ADR 0035 §9), the DJ's A/B decks.
- **A source that changes is marked: `Inputs::sources_changed(port)`** (`signal-core`; set by the engine and the oracle when the plan's slot is built, allocating nothing):
  - It is true in the first call of a swapped-in plan for a surviving instance's input whose set of (source port, scale) differs from the previous plan's. It is false otherwise, and false for added instances, which have their own first-frame rule.
  - Blocks split at swaps, so it is partition-independent.
  - Unchanged plans set nothing, so every golden stays the same.
  - `std.note_seq` and `std.step_seq8` treat it on `follow` as a seek, generalising ADR 0033 §3's "a swap that connected `follow`".
  - This fixes the bug both earlier drafts found: rewiring `deck_a.position` from one transport to another fired every skipped step at once.
- **The in-content idiom for "follow if given a position": `std.playhead` follows.** This is better than `std.prefer` before the sequencers.
  - `std.playhead` gains optional inputs `follow` (signal, beats), `follow_jump` (events), `follow_running` (gate) and `follow_tempo` (control).
  - While `follow` is connected, `position` is an exact copy of it, and `running` and `tempo` are copies of their follow inputs when those are connected (else derived from the copied position's motion: `running` = moved since the last frame).
  - Its own clock keeps running underneath, unheard, from its own `bpm`, `rate`, `run` and `seek`, exactly as when it is not following.
  - `jump` forwards `follow_jump`, and adds one `Trigger` on the frame the playhead starts or stops following, or `sources_changed(follow)`.
  - The project's sequencers stay wired to `transport.position` and `transport.jump` as today. Every switch reaches them as a seek on `reset`: nothing fires retroactively, and the next note is on time.
- **What happens when following stops is a function of the wiring** (the owner: "think in terms of a control-signal-like system"): `std.playhead` gains the parameter **`release`** (gate, 0 or 1, default **0**, combine replace), set, modulated by a wire, or bound to a panel control like any parameter. It is read on the frame following stops:
  - **below 0.5 (default 0): continue.** The playhead goes on from the last followed position, at its own tempo, `rate` and `run`; its own clock is re-anchored there (a segment start, ADR 0033), so there is no jump in position, only the switch `jump` the sequencers take as a seek;
  - **0.5 or above (1): return.** The position steps to its own clock's position, which kept running underneath, with a `jump` on that frame.
  - Because `release` is an ordinary parameter, the choice can come from anywhere in the graph: a panel switch, a pattern, a comparator on the host's position, or another project's output. No policy is built in.
  - The threshold at 0.5 (the operator's) gives a gate any control can drive; values between are not interpolated, because a position cannot be half-returned without a glide no one asked for. A glide back, if wanted, is a module (a `std.line` into `follow`).
- Unconnected, with `release` at its default, the playhead is bit-identical to today: every golden is unchanged, and `lib.transport`'s pinned render hash `7f20f2bbed23372b` holds.
- `lib.transport` exports the four inputs and `release`, with a changelog note: ports and a parameter added, outputs unchanged.
- **A deck project** then exports `[exports] inputs = { position = "transport.follow", jump = "transport.follow_jump", running = "transport.follow_running", tempo = "transport.follow_tempo" }` and `parameters = { release = "transport.release" }`. In a `lib.deck` slot it follows the host when the host wires those inputs, keeps its own transport when they are not wired or `map` sets them to `false`, and does what the slot's `release` says when the host unwires them.
- `std.prefer` (ADR 0035 §5) stays as the generic fallback for other exported inputs, with an event output `switched` (one `Trigger` on a selection change or `sources_changed` of the chosen input). It is not the transport idiom.

### 7. Text, schematic, panels, describe, MCP
- **Text is the truth:**
  - `slot`, `load` and `map` are text;
  - "Load into slot…", "Replace…", "Unload" and Edit interface… write and remove them with `toml_edit`, as the parameter panel writes;
  - Replace keeps the `set` and `map` entries the new content accepts and removes the rest, saying so;
  - a `set` key the content does not have is an error, like any unknown key; a hand edit that breaks this keeps the last good plan;
  - no slot remembers anything outside its entry.
- **Schematic:**
  - a slot is a box with its interface's ports, grouped as declared, whether loaded or not;
  - an empty slot is drawn hollow (dashed outline, "empty", with its template's name if it has one: "empty: lib.voice");
  - a loaded one shows the content's name under the title and a link marker if it is a project;
  - unmatched slot ports have hollow pins with the reason in the tooltip;
  - semantic zoom opens the content inside the slot box like a composite (read-only for projects and `lib.*`, ADR 0035 §10 and ADR 0027 §6);
  - **editing the interface:** Edit interface… opens the slot's port list (add, remove, rename, retype, reorder groups); a "+" pin at the end of each side adds an input or an output; a pin's context menu renames, retypes or removes it; the writes are §2's;
  - the context menu has Load…, Replace…, Unload, Edit map…, Edit interface…, Save interface as template…, Detach from template, and "Make interface from this instance";
  - the Load dialog lists composites, library modules and projects in sibling folders, sorted by how many interface ports match, and offers to add the content's unmatched ports to the slot;
  - ADR 0030's trace follows renamed wires through the slot boundary;
  - the `graph` message gains `slot: { interface, from, load, matches, unmatched, unheard }` per slot box (additive, `ui-protocol.md`).
- **Panels.** Every slot has a representation on the panel (the owner, 2026-10-23: "a slot should have some representation on the panel surface too"), loaded or not:
  - **the slot card**, a `<signal-slot bind="/deck_a">` control that the default panel shows for every slot and a hand-written panel places like any control. Its header has the slot's name, its interface (template name, or "slot"), the loaded content's name with a link marker for a project, or "empty", and Load…, Replace… and Unload, which write `load` as the schematic's menu does (§7, text first). Its body has the interface's ports as small rows, each with a level or activity indicator (signals: a meter; events: a blink; unmatched: hollow, with the reason as its tooltip), and the interface parameters as ordinary controls, which write the slot entry. It accepts a dropped project folder, `.signal` file or composite name from the Load list as a Load. A Load, Replace or Unload in progress (the fades of §6) shows as a brief state on the card;
  - **the content's own cards** follow the slot card, in a group that folds open and closed under it (fold state is view state): a project's `ui/main.html` rooted at `/deck_a` (ADR 0035 §10), or, for a composite, the default card of its exported parameters, titled "DECK_A: lib.deck ◂ studio-transport". They come and go with the content; the slot card stays;
  - content parameters outside the interface write `set`;
  - a host panel binding to a slot port (`/deck_a.l`) is translated through the match;
  - when the slot is empty or the port unmatched, the binding is inactive, not a `signal ui check` error.
- **`describe` and MCP:**
  - `describe` lists slots: resolved interface and template, load, match table, dropped wires, unheard outputs;
  - new patch ops `slot.load {slot, load, map?}`, `slot.unload {slot}`, `slot.map {slot, map}` and `slot.interface {slot, add?, remove?, rename?, retype?}` are MCP tools (`agent-interface.md` "Edit the graph");
  - they write only the host's files;
  - a `load` path outside the host's folder follows ADR 0035 §8, and an agent may name one only inside roots its configuration allows (Safety section).

### 8. Rules carried from ADR 0035
- **Rate:** content runs at the host's rate (live, the device's rate per ADR 0036 §1; offline, the host's `sample_rate`). A project's own `sample_rate` is noted, not checked (ADR 0035 §8).
- **Cycles and depth:** a slot loading its own host, directly or indirectly, is refused at the closing `load` (ADR 0035 §8). Project loads count toward depth 8.
- **Packing:** `pack` packs `interfaces/` and follows ADR 0035 §13 for project content (`--embed`). `signal vendor <host> --slot <key> [--as <name>]` turns project content into the host's own composite (ADR 0035 §11) and rewrites `load` to it.

### 9. Remaining open
- *To verify in phase 2 (technical, the operator's):* that the Controller can queue Replace's second swap prepared against the next plan version. If it cannot, the minimal change is preparing a swap against a predicted version, an API detail and not a new concept.
- A quantised load (at the next bar of a chosen position) is later, as for joins (ADR 0035 §6).

## Alternatives considered
- **Broadcast signals (an earlier draft): dropped,** not kept for later either.
  - It declared named wireless connections in text at both ends (`[broadcasts]`, `from = "@transport.position"`), resolved while flattening to the nearest enclosing transmitter, with rebinding at an instance boundary.
  - Slots solve the include and transport cases with wires the host draws, and they handle outputs as well, which a one-to-many broadcast does not (an include's audio cannot push to the host).
  - A second spelling of a connection, with its own namespace, scoping, shadowing, rebinding and rename tooling, costs more than it saves; every receiving composite would still have to write the name.
  - The owner preferred automatic connection, which handles inputs and outputs alike.
  - Revisit only if a measured case appears: a value needed by many receivers deep inside many nested composites.
- **Automatic connections by library roles (an earlier draft, B1 with `[auto]`, plus B2 editor wiring and B3 "materialize"): rejected as the main model.**
  - The library would declare roles on module ports, a project would switch role groups on in `[auto]`, the nearest scope would win, and two candidates at one level would be an error.
  - Its plan-time resolution and ambiguity-is-an-error reasoning are sound. But what connects comes from module descriptors in code, not from the project's text, and a library release could change it; B3 would give two spellings of one meaning.
  - Slots keep the automatic part to one boundary (name matching between a slot's interface and its content), decided by the two texts alone.
  - Roles could later *suggest* `map` entries in the Load dialog (presentation only, never read by the planner).
  - Kept from it: `sources_changed`, `switched`, and the rate edits.
- **Interfaces only as named files** (the first draft of this ADR: `slot = "lib.deck"` required a template): rejected by the owner's answer 2; every slot's interface is its own and editable, and templates are optional.
- **A fixed release policy for a following playhead** (always continue, or always return): rejected by the owner's answer 3; `release` is a parameter the wiring decides.
- **Matching by role, kind or position instead of by name:** silent and fragile (two audio outputs `l` and `r` are indistinguishable by kind). Exact names plus explicit `map` are predictable and greppable.
- **A slot as a runtime object** (the engine routes to whichever content is loaded): a new runtime model. Rejected by hard rule 1; resolution at plan time gives the same behaviour.
- **Interface derived from an existing module** (`slot = { like = "moog_voice" }`): the interface would change whenever that module changes. It is offered instead as the editor action "Make interface from this instance", which writes the ports.
- **Keeping survivors across Replace:** fewer fades, but the result would depend on internal names coinciding between unrelated contents, and their states would mix. Rejected for unload-then-load.
- **An overlapping crossfade on Replace:** both contents would have to exist at once under one address, i.e. hidden state. Use two slots and a mixer.
- **Plain composite instances only** (ADR 0035 alone): still valid, and it remains (`project =`, `module =`). A slot is what makes content swappable without rewriting the host's wires.

## Consequences
- **`signal-core`:** `Inputs::sources_changed` (and its setter for the engine and the oracle). Nothing else.
- **`signal-graph`:** `ports_of` (a pure query over definitions). No slot notion.
- **`signal-engine`, `signal-oracle`:** set `sources_changed` when building a swapped-in plan (tests in `connected.rs`); `domain_scope.rs` words.
- **`signal-std`:**
  - `follow.rs`: seek on `sources_changed(follow)`;
  - `playhead.rs`: the follow inputs, the switch `jump`, the underlying own clock and `release`;
  - `std.prefer.switched` (ADR 0035);
  - contract tests.
- **`signal-lib`:**
  - `interfaces/{voice,instrument,deck}.toml` with pinned text hashes;
  - `transport.toml` exports the follow inputs and `release`;
  - `CHANGELOG.md`, `tests/pinned.rs`.
- **`signal-project`:**
  - `model.rs`: `slot` (inline, `from`, `remove`), `load`, `map`, reserved keys, `interfaces/`;
  - a new `slot.rs`: interface resolution (template, `remove`, entries), matching, renaming, `SlotResolutions`;
  - `lib.rs` (plan building), `library.rs` (library templates), `locate.rs`, `schema.rs`;
  - `diff.rs` (a changed `load` is a Replace; an interface edit is an ordinary edit) and `live.rs` (load, unload, replace sequencing);
  - `structure.rs` (`graph` fields), `patch.rs` (slot ops, interface edits with the rename rewrite), `vendor.rs` (`--slot`, vendoring templates);
  - `archive.rs`, `provenance.rs` (`library_interface_hashes`), `session.rs`.
- **`signal-ui`:** bindings through matches, slot cards, empty card, MCP tools.
- **`ui/`:** slot box (hollow when empty), "+" pins and the interface editor, menus, Load dialog, map editor, trace across slots (`ui/lib/`, `node:test`).
- **Docs:**
  - `project-format.md`: §1 `interfaces/`, §4 slot keys, a new §14 Interfaces, §6 diagnostics;
  - `semantics.md` §12: `sources_changed`, Replace as two swaps;
  - `module-contract.md`: the flag, follow and `release` on the playhead, the interface conventions;
  - `00-vision.md` §3a: Slot and Interface (added with this ADR's acceptance);
  - `ui-controls.md`, `ui-protocol.md`, `agent-interface.md`, `roadmap.md` (M3.2).
- **Goldens:** none change. New: `projects/two-decks`, two `lib.deck` slots (`../studio`, `../studio-transport`) into `lib.mixer`, with the host transport wired to one deck. ADR 0035's `projects/two-studios` (plain includes) stays beside it.

## Tests
1. **Equivalence:**
   - a slot loaded with X (composite, primitive, project) gives a plan **byte-identical** to `name = { module/project = X }` with the renamed wires;
   - both render bit-identically for *k* = 1, 64, 256 and threads 1, 2, 4, 8;
   - `moog_voice` in a `lib.voice` slot inside a copy of studio-transport equals the studio-transport golden byte for byte;
   - the same slot with its interface inline, from a project template, and from `lib.voice` gives byte-identical plans.
2. **Empty slot:**
   - the host renders identically to the host without the slot entry and its wires;
   - receivers have `is_connected` false;
   - the wires are still checked (an undeclared port and a shape mismatch are errors at the wire).
3. **Interfaces and matching diagnostics:**
   - `from` with `remove` and retyping entries resolves as specified; a `remove` of an unknown name and a name both removed and declared are errors;
   - exact match, `map` rename, `map = false`, mono to stereo fan-out;
   - errors: direction, shape, rate, unknown `map` key or value, two inputs onto one;
   - warnings: kind mismatch, nothing matched, the "did you mean";
   - all located by line and column;
   - unheard outputs and dropped wires listed by `describe`.
4. **Interface edits:** add, remove, rename and retype through `slot.interface` write the expected text with comments kept; a rename rewrites the host's wires, `map` keys and bindings and renders identically; removing a wired port is refused with the wires named; an edit that changes no match gives a byte-identical plan.
5. **Live, on the null driver and the real-time runner, seeded callback sizes, threads 1 and N:**
   - load, unload, Replace and an interface edit;
   - survivors bit-continuous;
   - the newcomer's audio follows the F-frame fade exactly, and a leaver reaches +0.0 at *s* + F − 1;
   - Replace keeps no state across contents that share addresses;
   - no allocation on the audio thread (counting allocator).
6. **Follow:**
   - a deck following the host fires every hit where a local follower would;
   - wiring, unwiring and rewiring `position` (another transport) at every phase of a beat fires nothing retroactively, nothing twice, and the next note on time (`sources_changed`, the playhead switch `jump`);
   - on release with `release` 0 the position continues from the last followed value with no step; with `release` 1 it steps to the own clock's position, which equals an unfollowed playhead's at that frame; `release` driven by a wire switches between the two;
   - unfollowed, the deck equals itself alone.
7. **Library:** template text hashes pinned; `lib.transport`'s render hash unchanged; studio and studio-transport goldens unchanged.
8. **UI:**
   - `graph` slot fields;
   - Load, Replace, Unload and the interface editor write the expected text and survive a reload;
   - bindings through a slot, and inactive when empty;
   - `signal ui check` passes on `two-decks`.
9. **Device:** 10 minutes of `two-decks`, with Replace in the voice slot every 30 s: 0 xruns.
10. **The owner's listening and usability verdict.**

## Phases
| Phase | Delivers | Exit criteria |
|---|---|---|
| 1. Offline | Slot interfaces (inline, `from` templates with `remove` and overrides, `lib.voice`/`instrument`/`deck`), slot keys, matching, `ports_of`, resolution to ordinary instances, diagnostics, `describe` | Tests 1–3, 7; every golden unchanged |
| 2. Live and follow | Load, unload, Replace sequencing; `sources_changed`; playhead follow and `release`; `std.prefer.switched`; `projects/two-decks` | Tests 5–6; the Controller question of §9 answered |
| 3. UI | Slot boxes, the interface editor (text writes, "+" pins, rename rewrite), menus, Load dialog, map editor, slot cards, bindings, MCP tools | Tests 4 and 8; the operator's visual check |
| 4. Files and verdict | Pack and vendor `--slot`, sessions; device run | Test 9; the owner's verdict (test 10), with ADR 0035 phase 5 |

## Notes

**2026-10-23, phase 1 (offline) implemented** (operator; decisions delegated). Where this ADR left a choice open, phase 1 decided as follows; the text above stands otherwise.
- **`projects/two-decks` is phase 2's**, as the Phases table says (the Goldens paragraph lists it without a phase); phase 1's slot tests use fixtures and the copy of studio-transport inside `projects/two-studios` (`moog_voice` in a `lib.voice` slot renders the studio-transport golden's audio).
- **Order of an interface's ports**: by role (inputs, outputs, parameters), each in name order, the template's first, then the slot's own additions; TOML tables keep no order the loader can read. Presentation (groups, ADR 0030) is unaffected.
- **A value set on a slot** must name a parameter of its interface (an error at the key otherwise), loaded or not; on an empty slot it is kept and applied to nothing (§3).
- **Shape and rate checks apply to ports, not parameters**: a parameter is a signal of the content's rate; a slot input matched to a content parameter (or the reverse) stays an error (§4's table).
- **`domain_scope.rs`**: `slot`, `interface` and `load` are already generic words of the crates it checks (the engine's per-instance state slots, the flattener's resolved exports, atomic loads), so it checks the project format's spellings instead (`interfaces`, `SlotResolution`, `SlotResolutions`, `InterfaceFile`, with ADR 0035's words); the core, the graph and the engine know only `ports_of` and the scope root from these ADRs.
- **`describe`** lists the build's `SlotResolutions` as `content.slots` (per slot: its resolved interface, `load`, the match table, the wires left out and the unheard outputs); `ui-protocol.md` gains the field with phase 3's slot boxes.
- **`signal_graph::ports_of(module, definitions, registry)`** returns `ModulePort`s: a primitive's ports and parameters in descriptor order, a composite's exports in name order, each with its direction, whether it is a parameter, shape, rate and kind (an exported input's from its first inner port in address order).
- **2026-10-23, after the review:** a composite's `[exports]` naming its slot's ports are checked against the interface and renamed through the match like wires; through an empty slot or an unmatched port they are left out, and so are the wires on them in the definitions that use the composite.


**2026-10-27, phase 3 (UI) implemented before phase 2** (the owner's approval of 2026-10-27, after "I haven't found yet how to create a slot and configure its inputs and outputs"; operator; decisions delegated). Phase 2 (live sequencing, `sources_changed`, playhead follow and `release`, `std.prefer.switched`, `projects/two-decks`) is still to come; phase 3's UI and tools are built on what phase 1 plans. The text above stands; where it left a choice open, or where phase 2 is missing, phase 3 does as follows.
- **How Load, Replace and Unload behaved until phase 2** (superseded on 2026-10-27 by phase 2, below: a Replace is now §6's unload-then-load, with no state carried; this paragraph is history). Each was one ordinary structural edit, written as text by `patch` (`slot.load`, `slot.unload`) exactly as the schematic's other structural edits are, then applied by the normal reload: the diff of the two plans, one swap, `semantics.md` §12 as for any instance added or removed. So a **Load** adds the content's instances, whose `audio` outputs fade in over F as any added instance's; an **Unload** removes them as any removed instance is (suspended, its `audio` reaching +0.0 before the swap); a **Replace** is *one* edit and *one* swap, not §6's unload-then-load: the old content's instances leave and the new one's arrive in the same swap, each with the fade of its kind, and an inner instance whose address and type the two contents share **survives with its state** (the diff's survivor rule), which §6 rules out. §6's sequencing (suspend X at *s*, swap to the empty slot at *s* + F, swap to Y at *s* + F + 1, no state carried, the Controller question of §9) is phase 2's, as are `sources_changed` and following; until then a slot holding a deck follows nothing but its own transport, and a Replace under a running sequencer may fire skipped steps as any rewiring does today. The slot card says "Loading…" or "Replacing with …" only while the write is under way, not during a fade.
- **The ops** (`docs/ui-protocol.md` "patch"): `slot.add` {name, slot?, load?, x?, y?} (the entry `name = { slot = …, load = "…" }`, appended as `instance.add` appends; its undo is `instance.remove` with exact edits, and a slot is deleted by `instance.remove` like any instance), `slot.load` {slot, load, map?, set?}, `slot.unload` {slot}, `slot.map` {slot, map}, `slot.interface` {slot, add?, remove?, rename?, retype?, removeWires?, detach?, template?}. They write only the slot's own keys (`slot`, `load`, `map`, `set`, `seed`, its interface parameters' values) and, for a rename or `removeWires`, the level's wires and `[exports]` on the slot's ports, each key with `toml_edit` in place, so every other byte of the file is kept, comments included. Their inverse is a new op, **`slot.restore`** {slot, changes: [{path, value, at}], wires}: each key changed, its old value as written (TOML text) and its old place in its table, and the wires to rename back; an undo gives back every value byte for byte and every key in its place, a key of a section (`[instances.x]`, `[instances.x.slot]`) with the comment lines above it and its trailing comment (they go with the key it removes, and `decor` carries them back), and removes the tables it had to make (tested on inline entries and on sections); a key that was itself a section (`[instances.x.slot]` as a whole, which only Detach and a template replace) comes back inline. An untrusted `slot.restore` may touch nothing but those keys of a slot's entry (and the wires on its ports): its `decor` must be spacing and comments only (the prefix whole blank or `#` lines, the key's suffix and the value's prefix spaces and tabs, the value's suffix spaces and tabs then at most a `#` comment), and every slot op checks after its edit that the file with the slot's entry left out is exactly the file before it (with the wires it renames), so nothing outside the entry can be written; the reload's load then checks what it writes, as for every patch.
- **Renames** keep each renamed key in its old place (the interface's entry, the `map` key, the parameter's value), so renaming back gives the file back byte for byte. One op renames each port once: a rename whose new name another rename of the same op takes or gives (`{x: z, z: w}`, `{x: y, y: x}`) is refused, `bad-value`; chained renames are separate ops (in one patch or several), each reading the file as the one before left it, and the panels' bindings follow the renames in their order. A renamed interface parameter's values in `snapshots.toml` (every snapshot's and the undo snapshot's, `"/fx.volume"` → `"/fx.amount"`, ADR 0041) move with it, and a removed one's go, in the order the patch's ops did them (an op's removals before its renames, so a rename onto a removed name keeps the renamed parameter's values), written in the file's canonical text under the patch's writer lock, taken as seen and pushed to every client as the snapshot list (the reply's `snapshots`); a patch that moves no value leaves the file byte for byte; an undo of a removal does not bring those values back. Its `snapshot_exclude` entry is renamed or removed in the same write, and `snapshot_exclude` on a slot names its interface's parameters (checked against them, loaded or not), so `snapshot.exclude` works on a slot's control.
- **Interface edits** (§2). `remove` of a template's port names it in `remove` (and deletes its own entry if it retyped one); adding a name that `remove` holds takes it out of `remove`; `retype` writes the slot's own entry (the template's port of that name is replaced); a retype that changes a wired port's direction is refused (`in-use`). `rename` = remove the old name and add the new one with the old declaration, and rewrites in the same write every wire and `[exports]` target of the level that names the old port, its `map` key and an interface parameter's value set on the slot (a port of a loaded slot that matched by name, with no `map` entry, gets `map.<new> = "<old>"`, so it keeps its match and the plan does not change; the reply's `notes` say so); the host then rewrites the **panel bindings** on it (`bind` attributes equal to `/slot.old`, through every instance path of a composite level) in the project's `ui/*.html`, each compared once more before its rename (ADR 0040), and says which it rewrote (`panels`), each taken as seen and pushed to every client as `panel.written`, as `panel.write` does; a panel it cannot rewrite (changed meanwhile, reached through a link, unreadable) is a `notes` line, the rename stands. The undo renames back the same way. Removing a port that a wire names is refused (`in-use`) naming the wires unless `removeWires`, which removes them by `connection.remove`'s exact edits (undone the same way); a port the level's `[exports]` name is refused (`in-use`) naming them, `removeWires` or not (exports are not edited by slot ops). Panel bindings on a removed port are not checked first: the binding check then reports them as unknown ports. `detach` writes the resolved interface inline (no `from`, no `remove`); `template` sets `slot = "<template>"` alone (Save interface as template… writes `interfaces/<name>.toml` first by `file.write`, which now takes that path, then this op; the file write is not in the schematic's undo history).
- **Replace keeps what the new content accepts** (§7): a `slot.load` without `map` keeps the `map` entries whose value names a port or parameter of the new content (or is `false`) and removes the others, and keeps the `set` keys the new content takes (a module's parameters, a project's `<inner>.<parameter>`), removing the others; a module loaded in place of a project drops `seed`. The reply's `notes` say what was removed. What the content offers is `signal_project::content_names` (the Load dialog's match counts read the same). With `map` or `set` given, they replace the slot's. **Unload** removes `load` (and a project's `seed`), and keeps `map` and `set`: they apply again to what is loaded next, by the Replace rule.
- **The schematic.** A slot's box shows its interface's pins whether loaded or not (`graph`'s instance gives them, with `slot`: `{interface, from, load, project, map, matches, unmatched, unheard, dropped}`, additive; `graph` also lists the templates, `interfaces`); an empty slot is not in the plan but is in `graph`, in its file's place. The box is outlined in the accent colour, an empty one hollow and dashed, with "empty: lib.voice" (or the content, a project's with ↗) under its name; an unmatched port's pin is hollow and dashed, its reason in the tooltip. Every slot box keeps one row below its pins for its "+" pins (drawn only in Edit patch), which open the interface editor with a new input or output. Edit patch's menus: on the background (and an open composite) **Add slot here…** (a name and the interface to start from: none, `lib.voice`, `lib.instrument`, `lib.deck` or the project's `interfaces/*.toml`); on a slot box Load… (empty) or Replace… (loaded), Unload, Edit map…, Edit interface…, Save interface as template…, Detach from template; on any instance Make interface from this instance…; on a slot's pin Rename or retype port…, Remove port, Remove port and its wires. Semantic zoom opens a loaded **project composite** in place as any composite (its frame's pins are the content's names); `lib.*` and project content are not opened, as for plain instances of them. ADR 0030's trace follows the written wires to the slot's pins and, inside an opened content, the content's own; a trace across the match itself (slot port to content port under another name) is not done yet.
- **The Load dialog** is the Add module browser in a load mode (`ui/controls/module-browser.js`): the module types and, under Projects, the project folders beside this one (its subfolders `./x` and siblings `../x` holding a `project.toml`, at most 64 of each, from the new message `slot.projects`), each row saying how many of the slot's ports and parameters it matches by name (in the schematic through the slot's `map` too; the slot card's dialog by name alone, `describe` carrying no `map`), the list sorted by that count; its detail adds the match and names the content's ports the slot lacks, and "Add its unmatched ports to the slot" adds them in the same patch, before the `slot.load`. It shows no previews (it adds no instance). A `.signal` file is not listed (ADR 0035 phase 4).
- **The panel.** `<signal-slot bind="/deck_a">` (`ui/controls/slot.js`) and the default panel's card for every top-level slot, loaded or not: the header (name, template or "slot", content with ↗, or "empty"), Load… (or Replace…, the same Load dialog) and Unload, which `patch` `project.toml` against its current `file.read` base; the interface's ports, filled when matched, hollow with the reason when not; a **Levels** switch that shows the matched signal ports' levels (scope streams through the slot's match, at most 4 per card, each one of the run's 8 scope slots, so off by default); events show their matched state only (the protocol has no event stream); the matched interface parameters as knobs. A module name, a project path or a dropped folder (`file://`, written as an absolute path, which the load warns about, ADR 0035) dropped on the card loads it. **The content's own cards** (a project's `ui/main.html` rooted at the slot, a composite's card of its exported parameters) are not shown yet: rooting an included project's panel and writing its edits into `set` is ADR 0035 §10's, not yet built, so a content's parameters outside the interface are not panel controls; to make one editable, add it to the interface (Edit interface…, or the Load dialog's offer). A slot's **controls** (`describe.controls`, the parameter panel, `commit`) are its interface's matched parameters, by the interface's names, written on the slot's entry; phase 1's controls under the content's names (which `commit` would have written as keys the slot refuses) are gone.
- **Bindings** (§7): a binding on a top-level slot's input or output (`/fx.y`) is checked, and streamed (`scope`, `vector`, `trigger`), through the match to the plan port behind the content's port (a composite's export followed to its primitive); on an empty slot or an unmatched port it is **inactive**: `signal ui check` and `panel.check` give a warning ("inactive: /spare.out: /spare is empty"), never an error, and a stream on it is refused with the new error code `inactive`. `<signal-slot>` binds a top-level slot; another `bind` is an error.
- **MCP** (`docs/agent-interface.md`): the tools `slot_add`, `slot_load`, `slot_unload`, `slot_map`, `slot_interface` send the same ops through `patch`, and so are judged by the gain guard as any patch (tested: loading a module into an empty slot that feeds a gain makes it modulated, refused below `--max-gain 4`). A project that a slot op loads must lie inside the project's own folder or a folder named by the new option `--load-root` (repeatable; the operator's choice: nothing outside the project by default); `patch` with slot ops is checked the same way, and so are `write_file` and every `patch`, whatever its ops (an `instance.add` with edits included): a project the edited file's text includes (`project = …`, any form: `..`, a bare name, a link, each resolved from the project's folder as an include is) or loads into a slot that the file did not already name is refused outside those folders. `write_file` and `read_file` take `interfaces/<name>.toml`.
- **Test 8's** `signal ui check` on `two-decks` waited for phase 2's project (done with it, below); phase 3's UI tests run on scratch projects (`crates/signal-ui/tests/slots.rs`). Test 4's rename "renders identically" is checked as byte-identical plans (`patch/slot_tests.rs`), which implies it.


**2026-10-27, phase 2 (live and follow) implemented** (operator; decisions delegated). Where this ADR left a choice open, phase 2 decided as follows; the text above stands otherwise.
- **The Controller question of §9: answered, with the minimal change §9 names.** The Controller could already *send* a second swap after the first: `send_swap` advances its target version and keeps the sent plan's structural mirror, and `prepare_swap` prepares against the newest mirror. It could not *prepare* the second before sending the first, which `send_edit`'s all-or-nothing rule needs (nothing is sent unless every command of the edit can go: a Replace stopped after its unload would leave the slot empty while the files say it is loaded). The change is one method, `Controller::prepare_swap_after(previous, plan)` (and `signal_output::Controller`'s), which prepares against `previous`'s mirror and plan version + 1: a swap "prepared against a predicted version", as §9 says. It is refused for another Controller's `previous`, and, sent without `previous` before it, refused as stale by `send_swap`'s existing check. No engine or core change; `crates/signal-output/tests/chained_swaps.rs` tests it (and that the Runner's callbacks allocate nothing across the two swaps).
- **Replace** (`live.rs`): a save in which a slot of the project's own files (its root and its composites; a slot inside loaded content is the content's own edit, ADR 0035 §7) changes what it loads (both loaded, another canonical content: a project's canonical folder, else the module reference its `load` resolves to) is sent as two swaps: the old content's instances suspended at *s*, then the project with those slots unloaded at *s* + F (**as `slot.unload` writes it**: its files have `load` and `seed` removed, so the middle version is a project its own files say, and a session records and replays it as one), then the new project at *s* + F + 1. The rest of the save's edit goes with the first swap; the second only adds (a second swap that would remove is refused, nothing sent). Both swaps are prepared and every command counted before anything is sent. **What is "the same content" is decided by what it plays, not by the `load` string**, refining §6's "the canonical `load` reference" (2026-10-28, for the own copies of a slot's insides that `slot.inside` writes, `composites/<slot>_inside.toml`): another `load` whose content, instantiated alone under the slot's name with the values set on it, plans byte-identically (the same flattened instances, types, parameters and wiring at the same addresses; patterns aside, whose change an ordinary edit swaps in under the survivors) is no Replace, and the edit is an ordinary one, which then changes nothing that plays: making an own copy is inaudible (`slot_live.rs`, `loading_an_identical_copy_is_no_replace`). **A Replace is one even when the two contents plan alike but for their values** (the same instances with other parameters, which the plain diff would send as `set`s): no state carries over. A Load is one swap and an Unload the suspend-then-remove protocol, as before. Once a swap has gone out, a later refusal (a set, the second swap; only a desynchronised Controller, the queue having been counted) stops the edit there: the swaps not sent are withdrawn from the session recorder, the edit is `Applied::unfinished`, and the project returned as running is the last one swapped in (for a Replace whose load did not go out: the slot plays empty). Tested with a stand-in sender that refuses each command in turn (`live.rs`, `sending`). **A suspend as late as its swap** (a control thread stalled for more than the latency between stamping and the runner taking the commands) lands on the swap's frame after the swap removed its instance and is discarded, so that removal has no fade; `send_edit` stamps from a status polled after preparing, so it takes a stall, as for any removal (`semantics.md` §12). Two late swaps still commit on consecutive frames, and the session records and replays them (`slot_live.rs`). An instance whose type changes at the same address in an ordinary edit (not a Replace) stays, as before and as §12 says, a removal plus an addition in one swap: the old one fades out before it, the new one in from it. `Applied` gains `replaced` and `load_swap`; `signal ui`'s `reload` message gains `replaced` and `loadSwap` (additive).
- **An interface edit** is an ordinary edit (§2): one that changes no match plans byte-identically and sends nothing.
- **`Inputs::sources_changed(port)`**: per port of the descriptor, inputs only (a parameter's sources are not flagged), true when a swap since the instance's **last call** gave the port another list of (source port, scale) than the plan before, in canonical order (so another source, another scale, one more or one fewer). Several connections from one source to one input are allowed; the canonical order puts them by scale in IEEE total order (`signal-graph`, the engine, the oracle, `semantics.md` §3), so writing the same set in another order is no change. Tested on a rate-divided instance, a state-output instance (delivered to `absorb`) and one suspended across two swaps, against the oracle (`crates/signal-oracle/tests/sources_changed.rs`). It is handed to the first call after the swap, then cleared (allocating nothing); a change the instance has not seen yet (suspended across the swap, or between its ticks) carries into the next swap's flags until it is called. The engine computes it when it builds the swapped-in plan against the old one (or the Controller's mirror), the oracle the same way; an added instance and an unchanged input read false. Tested in `crates/signal-engine/tests/sources_changed.rs` (blocks of 1, 3, 64 on 1 and 4 threads; a resume; no allocation) and against the oracle (`crates/signal-oracle/tests/follow.rs`).
- **`std.note_seq` and `std.step_seq8`** take `sources_changed(follow)` on a call's first frame as a seek (as a `reset` trigger).
- **`std.playhead` following**: the four inputs are appended after its ports (`follow`, `follow_jump`, `follow_running`, `follow_tempo`), so the existing ports keep their indices. While following, `running` and `tempo` without their follow inputs come from the motion since the frame before (`running` 1 when it moved, `tempo` that motion per minute), 0 on the first followed frame, which has none; a non-finite `follow` reads as the last followed value. `follow_jump`'s events are forwarded only while following; the own clock's `seek` is unheard then (it moves the clock underneath). `jump` carries at most one `Trigger` a frame (a seek heard, a forwarded `Trigger`, starting to follow, `sources_changed(follow)`, a stop with `return`), so the sequencers see one. `release` is a value set (`continue` 0, `return` 1) snapped as a choice (0.5 returns, ADR 0043), read at the frame following stops. **`continue` emits no `jump`** (the operator's decision of 2026-10-28, after the review: the position does not step, and a `jump` on `reset` would end a sounding note): the own clock is re-anchored at the last followed position one frame of its own motion on (a segment that started there the frame before), so the stop frame neither steps nor holds; the own clock then keeps that place, so a later `return` returns to it. `return` steps to the own clock with a `jump`. A seek of the own clock landing on the stop frame wins over either: the position is its target, with one `jump`. So §6's "adds one `Trigger` on the frame the playhead starts or stops following" holds for starting, rewiring and `return`, not for `continue`.
- **`std.prefer`** (category `core`): `switched` carries one `Trigger` on the first frame of a call after a swap that changed a choice or gave a chosen input other sources; none on an instance's first frame (nothing was chosen before). `events` forwards the chosen input's events as they arrived.
- **`projects/two-decks`** loads **`./deck`, a copy of studio-transport as committed in M3.1, inside two-decks' folder**, and `../studio`, mixed by `std.mix`, not the Goldens paragraph's `../studio-transport` into `lib.mixer`: as `projects/two-studios` keeps its copy, so that the golden does not move when the owner tunes `projects/studio-transport` (theirs since 2026-10-23); and `lib.mixer` is ADR 0035 phase 3's. **`lib.transport` does not export the follow inputs yet** (§6's last paragraph): its text hash is recorded in the studio-transport and two-studios golden manifests (`library_composite_hashes`), so changing it moves those two goldens' manifests (not their audio); that is the owner's to approve. The deck instead vendors it (`deck/composites/transport.toml`, lib.transport's text plus the follow inputs and `release`) and exports them as §6's deck project does. Its golden, `tests/golden/two-decks/manifest.json`, is new. `crates/signal-cli/tests/two_decks.rs`: the golden; deck A following a host at 126 bpm plays the deck alone at 126 bpm bit for bit (blocks of 1, 64, 256 on 1, 2, 4 threads); deck B plays the studio's audio and the deck alone the copy's (so the vendored transport with its follow inputs unconnected is bit-identical to lib.transport); the slot plans as the explicit include (ADR 0035 test 10); `signal ui check` passes (test 8, with a panel of the two slot cards, the transport and `release`); a live Replace of deck B on the null driver.
- **Test 5** is `crates/signal-project/tests/slot_live.rs` (Load, Unload, Replace and an interface edit through `send_edit` on the null driver, seeded callback sizes, 1 and 4 threads; the host's sound bit-continuous; the newcomer's fade exact; the leaver at +0.0 from *s* + F − 1; a Replace of two contents that share `/deck/osc` starting the new one fresh; each session replaying to what was played) with `chained_swaps.rs` for the allocation count. **Test 6** is `crates/signal-project/tests/follow_switch.rs` (wiring, rewiring to a transport 0.1 beats ahead, and letting go with `release` 0 and 1, at phases across a beat and blocks of 1, 64, 256: every hit where the follow rule puts it for the position played, none on a switch frame, none twice), the playhead's and sequencers' unit tests, and two_decks.rs above.
- **The slot card** shows no state during the fades: a Replace takes 2·F + 1 frames (about 10 ms), less than one frame of the page; the card's "Replacing with …" covers the write and the reload, as in phase 3.
- **Not in phase 2** (ADR 0035's phase 2, which this one does not need): watching an include's own files for live edits (the host's folder is watched), includes in projects read from memory, and the edit-cost measurement in `benchmarks.md`. Test 9 (the device run) and the owner's verdict are phase 4's.



## Editable insides (2026-10-27)

The owner, 2026-10-27 (translated): "I think the slot should behave like a composite. One should be able to assemble something inside the slot, not just load a ready-made thing into it — though that is also a valid path — but it would be better if you could also step into it with a double-click and edit its inside." Asked what an edit inside does to content other slots or instances share, the owner chose **"Saját példány"** (own copy): every slot's editable inside is its own, kept by the project in its own file; a double-click on an empty slot gives an empty inside whose frame's pins are the slot's interface ports, to assemble anything in; a slot that holds content shows it, and the first edit makes an own copy (copy-on-write), so other users of that composite, and `lib.*` or `std.*` content, are unchanged. The technical choices below are the operator's (delegated). This amends §2 (what `load` may hold is unchanged; a composite may now be a slot's own), §7 (the schematic, the protocol, the tools) and the phase 3 notes; nothing else above changes.

**1. The file: the slot loads a project composite that names it.** A slot's **own inside** is a project composite `composites/<name>.toml` that the slot `load`s and whose `[composite] inside_of` names the slot: `"<slot>"` at the top level, `"<composite>/<slot>"` in a composite's file (`project-format.md` §3, §14).
- Chosen over a dedicated slot key (`inside = …`, or an inline inside in the slot's entry) because `load` already resolves, matches, plans and validates a composite: no second resolution path, nothing new in `signal-core`, `signal-graph` or `signal-engine` (hard rule 1), and the file is an ordinary composite a person can read and edit. Text stays the truth (ADR 0009): whose inside a composite is, is written in it, so the editor reads ownership from the files alone, and nothing outside the files remembers it.
- The marker is presentation and ownership only: the plan never reads it. Its form is checked (`<slot>` or `<composite>/<slot>`; an error at the value otherwise, and on a library composite), and **only the slot it names may load the composite**: another slot's `load` of it, or a plain instance's `module`, is an error at the reference, whose hint names the fix (set `inside_of` to the new slot; for a plain instance, remove it; a copied file needs its name and marker changed) (review follow-up, 2026-10-27), so "edits here change this slot only" holds, and keeping the inside's exports in step can break no one else. The Add module browser does not list a slot's own inside, and the Load dialog lists it for its slot only (`modules` gives it `inside_of`). The marker is not checked against the slots otherwise: an Unload keeps the file and its marker (an orphan, loaded by nothing; a new own inside for that slot is then `<slot>_inside_2`), and a Load of it back makes it the slot's own again. A slot holds its own inside when it loads a composite whose marker names it; any other composite it loads is shared content.
- **Its exports are named after the slot's ports**, so the slot matches them by name with no `map` (§4 unchanged). A port with no export is unmatched until something inside is wired to it: its host wires are left out of the plan, as for any unmatched port (§4), which is what an empty inside means.
- **Its name** is deterministic and free: `<slot>_inside` (`<composite>_<slot>_inside` for a slot in a composite), then `_2`, `_3`, … while a file of the project or a library composite (whose bare name a project composite must not take beside a `lib.<name>` reference) has it. `slot.inside` takes another name if given one.

**2. What "own copy" means for each kind of content** (`slot.inside`, `crates/signal-project/src/inside.rs`):

| Content | Own inside |
|---|---|
| none (empty slot) | an empty composite: `[composite] name`, `inside_of`, an empty `[instances]` |
| a project composite (`load = "voice"`) | its file's text copied into the new file, comments, ranges, groups and values kept (but in an export table whose keys are renamed through a `map`, which is written afresh); `[composite] name` set, `inside_of` added; the composite it came from and its other users unchanged |
| a library composite (`load = "lib.tape_echo"`) | the same, from the library's text (its `category`, `description` and its `lib.*` references kept) |
| a primitive (`load = "std.gain"`) | wrapped: one instance of it named after its type's last segment (`gain`), every port and parameter exported (reserved names left out) |
| a project (`./x`, `../x`, `.signal`) | **not made in v1**: a project in a slot is edited in its own folder (ADR 0035); the slot does not open, and says so. Turning one into the host's own composite is `signal vendor --slot` (§8, phase 4) |
| its own inside already | refused (`bad-value`) |

- The exports are renamed through the slot's match: each slot port matched to a content name exports that name's targets under the port's name (one content output under two slot outputs, `map = { l = "out", r = "out" }`, becomes the two exports `l` and `r`); a content name no slot port matches keeps its name, unless the interface has that name (a port mapped elsewhere or to `false`), when it is left out, so the port stays unmatched. The slot's `map` is then removed; `set`, `seed`, its interface parameters' values and every other key stay.
- **Nested composites stay shared**: a copy names the composites inside it as the original did. Opening one inside the slot's inside and editing it edits that composite, as for any composite instance; only the slot's own level is copied.
- **The plan does not change.** A copied composite plans byte-identically to the slot before the copy (a composite's exports are flattened away, §5); tested on the studio-transport voice slot of test 1, which then renders the same audio (`crates/signal-cli/tests/slot_insides.rs`). A wrapped primitive plans one level down (`/fx` → `/fx/gain`): it plays the same, bit for bit (tested), but a primitive that draws a random stream draws it from its new address, and a live edit sees it as a new instance (no survivor).

**3. Copy on write, in the schematic.** A slot opens like a composite (a click or a double-click on its box, Enter, Open in place, and its menu's new **Edit inside**). Its frame's pins are **the slot's interface**, whatever it holds, so what is wired outside is what reaches the frame inside, under the same names.
- `graph` gives every slot box `slot.inside` (`{level, own, make, name, reason}`), and gives the inside a slot without an own one would get, read-only, in a new field `insides`, keyed by the slot's address (`/fx`, `rack/fx`, keys no composite can have). It is made by the same function `slot.inside` uses, for a project composite too (read beside the project's other files, in the project's folder so a composite that includes a project reads; one that cannot be read so, because it includes a project, is shown as its own level framed through the match), so what is shown is what the copy will be: the first edit, derived from the preview's names, applies to the copy (review follow-up: a composite loaded through a `map` has its exports renamed by the copy). For a wrapped primitive the note adds that editing restarts its sound once (it moves one level down). A slot's own inside is its composite's level in `composites`, with the slot's interface on its frame and `slot` naming it.
- A note over the stage says which: read-only and what the first edit does (the file it will write), or the slot's own inside and that an edit changes this slot only.
- **The first edit** in a read-only inside (in Edit patch: Add module, a wire, a delete, Add slot here…) is written as two patches: `slot.inside` at the slot's level, then the edit at the new composite's level, against the base the first reply gives (`files`). They are two undo entries; both can be undone from inside the slot. Boxes moved in a read-only inside are not written. The notice names the file.
- **Wiring the frame** is new: `export.connect` and `export.disconnect` write a composite level's `[exports]` key by key (a frame input to inner inputs, an interface parameter to inner parameters, an inner output to a frame output, which names one port), undone by `export.restore` byte for byte. The schematic offers them only inside slots: a plain composite's frame stays as it was (its exports are edited as text), to keep this change to what the owner asked for; the ops themselves are general and the agent interface may use them on any composite. A module removed inside takes the frame wires on it with it (the removal of an instance an export names is otherwise refused, `in-use`; that check now judges the edited project when the patch edits the exports).

**4. The interface and the inside move together.** A `slot.interface` edit of a slot holding its own inside keeps the inside in step **in the same patch**: a removed port's export is removed, a renamed port's export renamed in its place (and no `map` entry is written, as it would be for shared content: the plan does not change); a rename onto a name the inside already exports (one no port reaches) is refused, never overwriting it; an export another port still matches through its `map` (`map = { l = "r" }`, `l` removed) is kept, and a renamed export's `map` values follow it (`r` → `rr` makes `l = "rr"`; a port renamed in the same op that maps to it is refused: rename it apart); an added port is a new frame pin, unmatched until wired; a retype changes nothing inside (the load check judges a type the inside no longer fits). From inside, the frame's background menu has **Edit the slot's interface…**, the same editor, its patch written at the slot's level.
- A patch now has **other files** besides its level's (`Patched::files`): a slot's own inside made, changed or removed. The host writes them under the same writer lock (ADR 0040), each only while it still holds what was read (`conflict` otherwise, nothing written), never through a link; a new or changed one before the level's file, a removed one after it (one changed meanwhile is kept, and said); a failure puts back what was written, only while it still holds the patch (as the positions file's failure does for the level's file), and a file that could not be put back is named in the error (never "nothing changed"). Between the two renames a reader may see one file new and the other old (accepted, as for `vendor`).
- An untrusted `composite.file` (an inverse a client sends back) may write only the own inside of a slot at the patch's level, keeping it that slot's (`inside_of` the same before and after), and may not create a file under a library composite's name.
- The inside's exports that no port of the slot reaches (a wrapped primitive's parameters, a copied composite's other exports) are shown on the frame after the interface's pins (`extra`), so they can be seen, unwired and removed with what they name. The inverse holds the inside's whole text (`composite.file`, which may write only a composite with `inside_of`, before and after, and only while it holds `was`), so an undo gives back both files byte for byte. As with `vendor`, a reader between the two renames may see one file new and the other old for an instant; the watch's reload sees both.
- The reply names the other files and their new bases (`files`); `graph.files` has them from then on.

**5. What stays as it was.** Addresses under the slot do not change (a copied composite plans byte-identically), so panel bindings, snapshots and `snapshot_exclude` on the slot's interface (`/fx.volume`) and `set` keys keep working; a wrapped primitive keeps every parameter exported under its own name (but one the slot reached through `map`, now exported under the slot's port's name), so its `set` keys too, which name parameters outside the interface. **Unload keeps the own inside's file** (the slot just no longer loads it; Load lists it under This project and loading it back makes it the slot's own again), and so do Replace and deleting the slot: the project's files are never removed behind the person's back. Undoing a `slot.inside` removes the file it made.

**6. The agent interface.** `patch` takes `slot.inside`, `export.connect` and `export.disconnect` (and their inverses), and the new tool **`slot_inside`** sends `slot.inside`. The same safety applies: the MCP server simulates the patch with the inside's file, judges the edited files (the new inside included) with the gain guard, and refuses a project the inside's text would newly include outside the allowed folders (`--load-root`), as for `write_file`.

**7. Phase 2.** Nothing here touches the engine or the sequencing. Phase 2 (merged 2026-10-27) decides a Replace by what the slot's content plans (`Project::slot_content_plan`): a `load` change whose content plans alike is no Replace. So a `slot.inside` that copies a composite (the same plan) sends nothing and is inaudible; one that wraps a primitive (the content one level down: another plan) is a proper Replace, unload then load; an empty slot's first inside is a Load of an empty composite (nothing plays); and an edit inside an own inside is an ordinary live edit of a composite (survivors keep their state, a parameter is a set). Tested in `crates/signal-project/tests/slot_live.rs` (`an_own_inside_is_no_replace_and_its_edits_are_ordinary`, through `slot.inside` itself).

**Not yet.** The MCP server judges the inside it simulates from its own read of the files; the host makes it again from its own, and only the level's file is checked against `base`, so a copied composite changed in that instant is judged as it was (the load check still judges what is written). An inverse `composite.file` holds the inside's whole text twice, so a slot's own inside over about 30 KB makes its interface edits fail the 64 KiB undo-size check (a hash for `was` would lift it). A plain composite's frame wiring in the schematic (the ops exist); copying a composite's stored schematic positions to its copy (the copy is laid out afresh); a copy of a project in a slot (phase 4's `vendor --slot`).
