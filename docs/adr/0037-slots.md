# ADR 0037: Slots

- **Status:** Accepted (2026-10-23). The owner's words of 2026-10-23 (translated): "There could be a composite-like module, I don't know what to call it: an empty shell-like module whose outputs and inputs are defined but has nothing inside. When we load something into it, it automatically connects with the matching connections…" The owner's answers of the same day to the draft's questions:
  1. the name **slot** is accepted;
  2. "every slot should have a configurable, editable, arbitrary number of inputs and outputs": a slot's interface is its own, declared on it and editable; interface files and the library interfaces are templates it may start from (§2, §3);
  3. "think in terms of a control-signal-like system, i.e. a function of the wiring": what a playhead does when following stops is a parameter that can be wired, not a fixed policy (§6).
  Read with the owner's earlier answers of that day: a whole project can be brought in as a composite (ADR 0035); mixing is done by a mixer module or composite; a live join only has to be glitch-free; an include's panel edits are written into the host (`set`); the audio interface is the one clock (ADR 0036 §1); everything is bound to signal sources; and automatic connection is preferred to a wireless broadcast, because it handles inputs and outputs alike. The rest is the operator's design, delegated: the template syntax (`from`, `remove`), matching by exact name, Replace as unload-then-load, and `release`'s default and threshold.
- **Builds on:** ADR 0005 (ports), 0006 and `semantics.md` §12 (swaps, survivors, fades), 0009 (text is the truth), 0016 (connections), 0026 and 0030 (schematic), 0027 §4 (library composites, pinning, `signal vendor`), 0033 (`is_connected`, exclusive inputs, `lib.transport`, `follow`/`reset`), 0035 (a project as content), 0036 (the device's clock).
- **Replaces** the drafts "Broadcast signals" and "Automatic connections" as the main model. Both are now alternatives (below). Their sound parts are kept: `Inputs::sources_changed`, the rate edits to ADR 0035, and the determinism and ambiguity reasoning.
- **Supersedes nothing accepted.**
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

