# ADR 0046: A slot with its own panel part

- **Status:** Accepted (2026-10-29; the owner: "mehet, nézzük meg így", taking every proposed answer below). Proposed 2026-10-29. The owner, looking at the schematic's panel controls drawn as nodes beside their instances (commit f9d2718, `ui/lib/schematic-controls.js`), said (translated): "the direction is good. I see these blocks move together — we could put each of them into a slot." Asked whether that means a visual frame or a real slot, the owner chose **a real slot in the project**: the module and its panel controls actually go into one slot, a self-contained, loadable unit with its own panel part. The design below is the operator's; the questions of taste are at the end.
- **Builds on:** ADR 0009 (text is the truth), 0023 §4 (panels are HTML in `ui/`, bound by address), 0026 §3–5 (schematic positions, the panel editor and its whitelist), 0032 (cards across windows), 0035 §10 (an included project's panel rooted at its instance: `<signal-panel root>`, not built yet), **0037** (slots, the slot card, bindings through the match, "Editable insides"), 0040 (writer lock), 0041 (snapshots), 0044 (polyphonic slots), **0045** (auto ports; project templates may hold them, library templates may not).
- **Amends, when accepted:** ADR 0037 §7 "Panels" (the content's own cards: a slot shows its part instead, §5), ADR 0035 §10 (the root mechanism is shared and defined here, §3), ADR 0026 §5 (the whitelist: `<signal-slot>` also as a card, §5). **Supersedes nothing.**
- **Date:** 2026-10-29

## Context
- **Today a panel is one level of addresses.** `ui/main.html` binds absolute addresses of the project (`/synth.cutoff`, `/drums/kick/dc.out`). Editing controls bind only top-level parameters (a composite's exports included); `commit` writes them into `project.toml`, and a parameter inside a composite is "not editable" (`crates/signal-ui/src/bindings.rs`).
- **A slot has a card but no controls of its own.** The `<signal-slot>` card shows the interface's ports and matched parameters (ADR 0037 phase 3). The "content's own cards" of ADR 0037 §7 and the rooted include panels of ADR 0035 §10 are not built.
- **The schematic (f9d2718)** draws each control of `ui/main.html` that binds a top-level instance as a node beside that instance. It has no position of its own and moves with its host. That is what the owner saw "move together".
- **What is presentation, measured:** the golden manifests' `project_file_hashes` list `project.toml`, `composites/` and `patterns/` files and no `ui/` file (`tests/golden/studio/manifest.json`), and a render never reads `ui/`. A panel change therefore moves no golden.

## Decision

### 1. The unit: interface, content and part
A slot is already an interface (in its entry) and a content (its `load`, possibly its own inside, ADR 0037 "Editable insides"). It gains a third piece of its own:
- **Part** (term added to vision §3a as a kind of panel, not a structural concept): the slot's own panel fragment, made of cards and controls whose bindings are **relative to the slot**.
- The part belongs to **the slot**, not to the content. So it stays when the content is replaced, and it works for every kind of content, a primitive included, which has no file that could carry a panel. Reuse outside the project goes through a project (§9).

### 2. The file: `ui/parts/<key>.html`
- **One file per slot:** `ui/parts/<slot>.html` for a top-level slot, `ui/parts/<composite>.<slot>.html` for a slot in `composites/<composite>.toml` (one part for every instance of that composite, rooted at each one).
- **Its format is a panel** (ADR 0023 §4, `ui-controls.md` "Panels"): one `<signal-panel part="<slot>">` (or `"<composite>/<slot>"`, the form `inside_of` uses) holding `.signal-cards` with cards, rows, groups and controls, within the same whitelist. The `part` attribute names the owner, as `inside_of` does for an inside, and is checked against the file's name. The panel editor reads and writes a part as it does any panel.
- **Relative URLs** in a part (a `skin="svg:assets/knob.svg"`) resolve from `ui/`, as the host page's do, because a part is rendered inside the host page, never as a page of its own. A control moved into a part therefore keeps its `skin` text.
- **Presentation only, as all of `ui/`:** never planned, never in a render's provenance, packed in a `.signal` file, watched like the panels. A part that does not parse is reported by the binding check and shown as its error. It never stops the project.
- **Why the slot names no file:** adding or editing a part then never touches `project.toml`, so a slot of an existing project (the two `two-decks` slots) can gain a part without moving its golden manifest. The `part` marker inside the file makes the link explicit and greppable.

### 3. Relative addresses: a part binds only inside its slot
A part's `bind` addresses are relative to the slot's address, its **root** (`/synth` at the top level; `/rack1/fx` for the slot `fx` of the composite instance `rack1`). The same rule renders ADR 0035 §10's included panels (`<signal-panel root="/drums">`).

| Written in the part | Means | Example (root `/synth`) |
|---|---|---|
| `.<name>` | a port or parameter of the **slot's interface** | `.cutoff` → `/synth.cutoff` |
| `/<path>.<port>` | a port or parameter **inside the content** | `/vcf.cutoff` → `/synth/vcf.cutoff` |

- `.<name>` must name a port or parameter of the slot's resolved interface. Otherwise it is an error ("not in synth's interface: add it with Edit interface…"). A content parameter outside the interface is reached by adding it to the interface (ADR 0037 phase 3's rule), so a part never writes `set`.
- `/<path>` is checked against the plan under the root. With no such address in the current content (the slot is empty, or another content is loaded), the control is **inactive**: a warning, not an error, as for host bindings through an unmatched port (ADR 0037 phase 3). It becomes active again when content with that address is loaded.
- **Self-contained:** an address outside the slot cannot be written in a part. It is an error with the hint "bind it from the host's panel". A slot's part therefore means the same wherever the slot is.
- The resolved address is the ordinary absolute one. Once rooted, every existing mechanism applies unchanged: the match translation of ADR 0037 phase 3, streams, `describe.controls`, snapshots (`/synth.cutoff` stays `/synth.cutoff`) and `snapshot_exclude`.
- In `ui/main.html` a relative `bind` stays an error, as today.

### 4. What a part's control writes, and automation across the boundary
| Bound to | A writing control commits to | Status |
|---|---|---|
| `.p`, an interface parameter of a top-level slot | the slot's entry, `p = v` in `project.toml`: exactly what `commit` writes for `/synth.p` today. On an empty slot the value is kept and applied to nothing (ADR 0037 §3), so the knob still works | phase 1 |
| `/inner.p` in the slot's **own inside** | the value on that instance in the inside's composite file (`composites/<slot>_inside.toml`). The file is owned by this slot alone (ADR 0037 "Editable insides" §1), so the write changes nothing else. It is an ordinary live edit: a `set` on a survivor | phase 2 |
| `/inner.p` in **shared** content (a project composite, `lib.*`, a project) | not editable, as any parameter inside a composite today; the hint offers **Edit inside** (copy on write) or Add to interface | as today |
| anything in a slot **inside a composite** | not editable (the entry is shared by every instance of the composite), as today; readers work | as today |

- **Automation:** a wire into a slot's interface parameter (`lfo.out → synth.cutoff`) is renamed through the match to the content's parameter (ADR 0037 §5). So the value is the control's base combined with the wire, by the parameter's combine, exactly as for a plain instance. A part's control on `.cutoff` is that parameter's control. In the schematic, the wire is drawn into the control's input (f9d2718's rule), at the frame when the slot is open (§6).
- Automation from inside the content (a wire inside the own inside into `vcf.cutoff`) is drawn into a control on `/vcf.cutoff` the same way.
- Nothing about how values combine changes: the part only chooses which address a control names.

### 5. The host's panel: an open slot card
- **Placement is the host's.** `<signal-slot bind="/synth">` is allowed in two places (a whitelist addition):
  - **as a card**, a direct child of `div.signal-cards`: the **open slot card**. The slot card's header (name, interface, content, Load… / Replace… / Unload, as today) is followed by the part's cards, laid out in the host's grid as cards of their own and framed together in the accent colour;
  - **inside a row**, as today: the compact card, whose header has a ▸ that opens the part's cards beneath it.
- **Folding is view state:** open by default, and the fold is kept per window in `ui/windows.toml` (presentation, ADR 0032).
- **Card ids** of a part are `<slot>:<id>` (ADR 0035 §10's `drums:<id>`), so a part's card can be moved to another window like any card.
- **Fallbacks, in order:** the slot's own part; else its template's part (§9); else, for project content with a `ui/main.html`, that panel rooted at the slot (ADR 0035 §10, read-only); else the slot card as today (the interface's matched parameters as knobs). The default panel gives every top-level slot an open slot card.
- **Edit layout** inside an open slot card edits the part (`panel.write` with `file = "parts/synth.html"`, its own `base`, undone in the same history). Dragging a control from a host card into the slot card moves it into the part with its bind made relative. Dragging it out makes the bind absolute. A control that binds outside the slot cannot be dropped into it, and the reason is given.

### 6. The schematic: the controls inside the slot's frame
- **A closed slot** is one box that holds its part's controls on the interface (`.p`): the box grows to make room, the pins stay on its edges, and the control nodes sit between them with short binding wires to their pins. Box and controls are one block: a drag moves the slot, and the controls come along because they have no position of their own (unchanged since f9d2718).
- **Opened in place:** controls on the interface stand just inside the frame, beside the frame pin they bind. Controls on inner addresses (`/vcf.cutoff`) stand beside their inner instance, placed by the inside's layout (`[composite.<inside>]` positions in `ui/schematic.toml`) and moving with that instance.
- **Host-panel controls** that still bind the slot from `ui/main.html` are drawn outside the box, as today, so it stays visible which controls are the slot's own and which are the host's.
- `graph` gains, per slot box, `slot.part: {file, controls: [...], error}`, the controls already rooted. `ui/lib/schematic-controls.js` reads parts as it reads the main panel.

### 7. Moving, loading, unloading, retyping: the part comes along
| Edit | What happens to the part |
|---|---|
| Drag in the schematic | nothing to write: the controls follow the box |
| Load, Replace | the part stays. `.p` controls keep working, since the interface is the same. `/inner` controls go active or inactive with the new content |
| Unload | the part stays. Interface knobs still write the entry. `/inner` controls go inactive |
| Interface **rename** of `p` | in the same patch, `.p` becomes `.q` in the part (as host bindings are rewritten today, ADR 0037 phase 3). The part is one more of the patch's files (`Patched::files`), under the writer lock |
| Interface **remove** of `p` | refused while the part binds `.p`, naming the controls, unless "remove its controls too" is chosen (as for `removeWires`) |
| **Retype** | nothing: binds name ports, not types. The binding check judges the new type (a meter on an events port is an error, as today) |
| Slot **renamed** | the file is renamed with its `part` marker, in the same patch. The part's relative binds are unchanged. Host binds `/old…` are rewritten as a rename rewrites them |
| Slot **deleted** | its part is removed in the same patch, and the undo restores it byte for byte. Unlike an own inside's composite (kept, ADR 0037 "Editable insides" §5), a part has no meaning without its slot |
| Slot moved **into or out of a composite** (when such an edit exists) | the file is renamed to the new key. The binds are unchanged |
| **Own inside made** (`slot.inside`) | nothing: addresses under the slot do not change (ADR 0037 "Editable insides" §5) |

### 8. Group into slot, and its undo
A new patch op, **`slot.group {instance, controls?}`**: Edit patch's menu on any instance ("Group into slot"), the panel's card menu, and MCP `slot_group`.
- **The entry:** `synth = { module = "moog_voice", cutoff = 400 }` becomes `synth = { slot = { inputs = {…}, outputs = {…}, parameters = {…} }, load = "moog_voice", cutoff = 400 }`. `project = "…"` becomes `load = "…"` with `seed` and `set` kept (ADR 0037 §2). Every other key stays in its place (`toml_edit`).
  - **Ports:** every port that a wire, an `[exports]` target or a host binding at the level names, declared `{}` (auto, ADR 0045), so each takes the content's type.
  - **Parameters:** every parameter the entry sets, a wire reaches, an export names or a moved control binds, by name. Their values stay the same keys on the same entry, now interface parameters.
  - **Nothing is renamed:** ports and parameters keep their names, so no `map` is needed and no wire is rewritten.
- **The plan is byte-identical** (ADR 0037 §5 and its test 1: a loaded slot plans as the explicit instance). So the audio and log are the same, on every partition and thread count. Only `project.toml`'s text, and so its hash in a new render's provenance, changes.
- **Refused in v1, with the reason:** an instance with `pattern` or `gpu` (neither is a slot key; wrapping it into an own inside would move its addresses a level down); an instance that is already a slot.
- **The panel:** the controls of `ui/main.html` whose every bind lies in the instance (`/synth.p`, `/synth/...`) move into a new `ui/parts/synth.html`, with binds made relative and labels, skins, rows and groups kept. `controls` narrows the choice; the menu shows the list with every control ticked.
  - A card left holding nothing but moved controls is replaced, in its place, by the open slot card, and the part keeps that card's heading and id.
  - A card that also holds other controls keeps them, and the open slot card is inserted after it.
  - `ui/windows.toml` entries naming a moved card's id are rewritten to `synth:<id>` in the same write.
  - Without `ui/main.html`, no part is written: the default panel already gives the slot a card.
  - Other panel files keep their controls, which go on working because the addresses are unchanged.
- **Undo** restores every file byte for byte, as one entry:
  - the entry through `slot.restore`, extended to the `module` or `project` key it replaced;
  - `ui/main.html` and `ui/windows.toml` by their whole text (a new inverse `panel.restore {file, text, was}`, a member of `Patched::files` like `composite.file`);
  - `ui/parts/synth.html` removed.
  - It fits the 64 KiB undo limit: the largest example panel, studio-transport's, is 11.9 KB (measured). A larger one is refused before anything is written, as any patch is.
- **Ungroup** (`slot.ungroup`, phase 2) is the reverse as an edit of its own: the entry back to `module` or `project`, and the part's controls back into the host panel in the slot card's place, with absolute binds. It is refused, with the reason, for an own inside, a `map`, a template, or an interface port the content lacks, because each would change the plan or leave a wire dangling.

### 9. Templates, composites, projects, library, voices
- **Project templates** may have a part: `ui/interfaces/<name>.html`, marker `part="interface:<name>"`. It binds only `.p` names of that template (auto ports allowed, ADR 0045). A slot from the template shows it unless it has its own part. Edit layout on it asks: edit the template's part for every slot, or make this slot's own (copy on write, as insides).
- **Library templates** (`lib.voice`, `lib.deck`, …) get parts only when a project needs one (hard rule 8). They would be read-only and pinned by text hash like the templates. A library template's ports stay typed (ADR 0045 §5), so its part can be checked once.
- **Composites:** a part is not a composite's. A plain composite instance shows no part. Making one is "Group into slot". Its own inside is the editable content, and the part sits beside it.
- **Taking a unit elsewhere:** "Save slot as project…" (phase 3) writes a folder: the content as its composites, the interface as `[exports]`, the part as its `ui/main.html` (the `.p` binds become its exports, rooted on load). Loading that project into a slot elsewhere shows it through §5's fallback. "Make part own" copies it into the slot's part. This reuses ADR 0035 and adds no second format.
- **Polyphonic slots (ADR 0044):** parameters are shared by every voice, so `.p` controls are unchanged. A writing control on `/inner.p` of an own inside edits the definition every voice copies. A reader on an inner address is inactive until a project needs per-voice display (which voice it would be is the owner's).

### 10. Migration and existing projects
- **Nothing is converted.** kick, pulse, studio, studio-transport, physical-av, two-decks and two-studios load, plan, render and show their panels unchanged. A panel without `ui/parts/` and without relative binds is read exactly as today, and every golden holds (none reads `ui/`).
- New projects and edits opt in through Group into slot, Add slot here… with a part, or a hand-written `ui/parts/` file.

### 11. Protocol, `describe`, MCP
- `panel.read`, `panel.write` and `panel.check` take `parts/<key>.html` and `interfaces/<name>.html` under `ui/`.
- `describe` lists each slot's part, its file and its rooted controls.
- `graph` lists the same under `slot.part` (§6).
- The patch ops `slot.group` and `slot.ungroup`, and the inverse `panel.restore`.
- MCP tools `slot_group` and `slot_ungroup` go through `patch`, under the gain guard and the `--load-root` rules (ADR 0029, ADR 0037 phase 3).
- A rename's reply names the parts it rewrote (`panels`).
- All additions are additive (`ui-protocol.md`, `agent-interface.md`).

### 12. The hard rules
- **Rule 1:** `signal-core`, `signal-graph` and `signal-engine` learn nothing. Parts, roots and relative addresses live in `signal-ui` (bindings, panel writes) and `ui/` (rendering, the schematic). `signal-project` gains only the `slot.group` and `slot.ungroup` text edits. `domain_scope.rs` is unchanged.
- **Rule 3, determinism:** no part reaches a plan. Group into slot plans byte-identically. Rooting is a pure function of the files, and the schematic's placement stays a deterministic layout.
- **Rule 4, text is the truth:** a part is a file. Its owner is written in it. What it binds is written in it. Folds and windows are in `ui/windows.toml`. Nothing is remembered outside the files.

## Alternatives considered
- **A visual frame only** (a group rectangle in `ui/schematic.toml` around an instance and its controls): cheap, but the owner chose a real slot. It makes nothing loadable, and the panel would not know the group.
- **A composite with a panel** (`ui/parts/<composite>.html`, shown for every instance of the composite): the part would follow a shared definition, so laying it out for one use would change every use. A primitive cannot carry one. A plain composite instance has no interface of its own, so its controls would name exports that change with the definition. The slot already is the unit with an interface. It is kept as a possible later extension (open question 4).
- **Controls with schematic positions of their own** (`@control:` keys in `ui/schematic.toml`): a second position namespace, and controls would drift away from their hosts. Grouping into a slot gives "move together" by structure instead.
- **The part inline in the host's `ui/main.html`** (children of `<signal-slot>`): one file fewer, but the part would live in the host. A rename or delete of the slot would edit the host's panel, a template could not have a part, and nothing could carry it elsewhere.
- **The part as TOML in the slot's entry:** every layout edit would move `project.toml`'s hash, and the panel format would be duplicated.
- **Absolute binds in parts, rewritten on every move or rename:** brittle, and a part could not travel.

## Consequences
- **`signal-ui`:**
  - `bindings.rs`: roots, relative binds, part checks, inactive inner binds;
  - `panel_write.rs`: parts, `panel.restore`, `<signal-slot>` as a card;
  - `host.rs`: the watch of `ui/parts/`;
  - `describe`.
- **`signal-project`:** `patch` (`slot.group`, `slot.ungroup`, part files in `Patched::files` for renames and deletes), `windows.toml` rewrite of card ids.
- **`ui/`:**
  - `panel.js`: a rooted `<signal-panel part>`;
  - `controls/slot.js`: the open slot card and the fold;
  - the panel editor across the boundary;
  - `ui/lib/panel-whitelist.json`;
  - `schematic-controls.js`: part controls inside the slot's box and frame;
  - menus.
- **Docs:**
  - `project-format.md` §1 and a §14 subsection;
  - `ui-controls.md` "Panels" and "Slots";
  - `ui-protocol.md`, `agent-interface.md`;
  - vision §3a (Part);
  - `roadmap.md`.

## Tests
1. **Equivalence:** Group into slot on the studio's `synth`, a primitive and a project include gives plans byte-identical to before. Studio's golden audio and log are reproduced from the grouped copy (k = 1, 64, 256; threads 1, 4).
2. **Undo:** grouping then undoing gives `project.toml`, `ui/main.html` and `ui/windows.toml` back byte for byte, and no `ui/parts/` file is left.
3. **Binding check:** `.p` on an interface port or parameter; `/inner` active and inactive (empty, other content); an address outside the slot refused; the `part` marker against the file name; a part of a slot in a composite rooted per instance; relative binds in `ui/main.html` refused.
4. **Bookkeeping:** interface rename rewrites the part; a remove is refused while bound; a slot rename renames the file; a delete removes the part and the undo restores it; Load, Replace and Unload leave the part's bytes alone.
5. **Writes:** a part's `.p` knob commits the same `project.toml` bytes as a host knob on `/slot.p`. A wire into the parameter combines as for a plain instance (oracle). Phase 2: an own inside's `/inner.p` writes the inside's file, and the shared content's is refused.
6. **Every example project** passes `signal ui check` unchanged, and every golden is unchanged.
7. **UI** (`node --test ui/`): the open and compact slot cards; card ids; drag in and out; the schematic's closed box with controls, the opened frame, and a drag moving box and controls together.
8. **The owner's verdict** on the studio with `synth` grouped.

## Phases
| Phase | Delivers | Exit |
|---|---|---|
| 1. Parts and grouping | `ui/parts/`, roots and relative binds, the check, the open slot card, Edit layout of a part, `slot.group` and its undo, the bookkeeping of §7, part controls in the schematic's slot box and frame, `graph`, `describe`, MCP `slot_group` | Tests 1–4, 6, 7; every golden unchanged; the operator's check in Signal.app |
| 2. Editing across the boundary | dragging controls in and out of a slot card; own-inside inner parameters editable (§4); `slot.ungroup`; project-template parts | Test 5 and the rest of 7 |
| 3. Units that travel | rooted project panels (with ADR 0035 §10), Make part own, Save slot as project…; library template parts and poly readers when a project needs them | The owner's verdict (test 8) |

## Decided questions of taste (owner, 2026-10-29)
1. **The closed slot in the schematic:** its controls inside one bigger box.
2. **The open slot card in the panel:** the part's cards open by default.
3. **What Group into slot loads:** the module as it is, shared, with the plan unchanged; the first edit inside makes an own copy.
4. **A composite with a panel:** only slots have a part.
5. **Grouping several instances into one slot:** not in these phases; a later ADR when wanted (addresses would move a level down).
6. **The name:** "part".

## Implementation notes

**2026-10-29, phase 1 implemented** (operator; decisions delegated). The text above stands; where it left a choice open, or could not be followed yet, phase 1 does as follows.
- **Where it lives:** `crates/signal-project/src/patch/group.rs` (`slot.group`, `panel.restore`, a slot's part removed with it; `slot.restore` may write back the `module` or `project` key, only to make a slot the instance it held or back); `crates/signal-ui/src/parts.rs` (keys and markers, rooting, the panel tree read and written exactly as `ui/lib/panel-tree.js` writes it, checked against the page's canonical text of the studio's panel, `ui/lib/fixtures/studio-canonical.html`, Group into slot's move, card ids, `ui/windows.toml`); the binding check's `Scope` (`bindings.rs`); the host's `part_edits` (the panels' side of a patch); `ui/lib/parts.js`, `ui/controls/slot.js`, `ui/app/lib/cards.js`, `ui/lib/schematic-controls.js` (`partNodes`).
- **The panels' side of a patch is the host's.** `signal-project` writes the entry and carries the files; the host computes the main panel's and the part's new text (they are panels, `signal-ui`'s) after `patch::apply` and adds them as more of `Patched::files` with `panel.restore` inverses, before the undo is tried and the size checked. What a panel binds on an instance reaches `slot.group` through a new `Context::bound` (`signal_ui::parts::bound_on`; the MCP server's simulation reads it the same way).
- **The interface takes every parameter a host binding names**, moved or not (§8 says "a moved control binds"): a host control left in `ui/main.html` on `/synth.p` keeps working only if `p` is in the interface. It also takes the names in the instance's `snapshot_exclude` (else the entry would not load). A bare project name (`project = "studio-transport"`) becomes `load = "./studio-transport"`: a bare `load` names a module. The interface's ports and parameters are listed in the content's order (a composite's exports: name order).
- **`controls`** are indexes among `ui/main.html`'s controls as `panel.check` counts them. The menu's dialog listing them ("every control ticked") is not built: the schematic's Group into slot (an instance's menu in Edit patch) moves every control, which is the dialog's default; the panel card menu's entry is not built either. Left open for phase 2.
- **Cards.** A card left with nothing but moved controls goes whole into the part (its heading, so its slug id), and `ui/windows.toml`'s `cards` naming its old id are rewritten to `<slot>:<id>`; a card that keeps other controls keeps them, and the part gets a card with its heading and classes holding only the moved ones (rows and groups kept, emptied ones dropped). With nothing to move, the open slot card goes at the end of the first grid. The open slot card's own id is `slot.<name>` (the review, 2026-10-30): no heading's slug has a `.` and no part card's id is one, so it shifts no other card's id; a card left whose `-N` id the move shifts anyway (a card of the same heading removed before it) is renamed in `ui/windows.toml` too, all renames at once; and a window given a card of the part is given the open slot card before it, so the header stays with its cards. A main panel the editor cannot read is left as it is (a `notes` line); a part file that already exists refuses the group. The server's slugs fold the common Latin letters with marks as the page's NFKD does; a heading of other letters may get another id on the server, and its window entry is then not rewritten (layout only). Signal.app keeps its layout in memory: a `ui/windows.toml` rewritten by a patch while the app runs may be overwritten by its next layout write (to be seen in the operator's check).
- **A group in a composite's level** writes the entry and moves no control (a part there would be every instance's); the notes say so.
- **§7's rows that have no edit yet.** No edit renames an instance or moves a slot into or out of a composite yet (there is no `instance.rename`), so "Slot renamed" and "moved into or out of a composite" wait for those edits (`patch::part_key` and `part_path` name the files). An interface **rename** rewrites the part's `.old` binds as the host panels' bindings are rewritten (after the level's file, under the writer lock, pushed as `panel.written`, named in the reply's `panels`), not as a member of `Patched::files`: its undo is the rename back, which rewrites them back byte for byte; a rename onto a name the part binds already (`.q` for a name not in the interface) is refused, `bad-value`, as the two binds could not be parted again. Every interface edit that drops a name the part binds (a `remove`, a template set or detached, an undo's `slot.restore`) is `in-use` naming the controls (the names before and after the patch compared, a renamed one not dropped); `removeControls: true` (a new field of `slot.interface`) removes them from the part as one more file of the patch, undone byte for byte. **Deleting** a slot removes its part; only the forward `instance.remove` does it (an inverse that removes, the undo of a `slot.add`, carries its own `panel.restore` or leaves a part a slot added later as an orphan, which the check reports as naming no slot).
- **The check.** `.p` must be of the slot's resolved interface (an error otherwise); an interface port or parameter that reaches nothing now (empty, unmatched) is inactive, a warning; `/inner` with nothing there now is inactive. "Outside the slot" is any other form (`echo.in`, `../echo.in`): an error naming the fix. A part of a composite's slot is checked at each instance's root, the findings merged; with no instance only its errors of form are reported. A file under `ui/parts/` whose name is no part's, or that names no slot, is an error. An empty slot's interface knob writes nothing yet (the panel has no control row for an unmatched interface parameter, so `commit` has no range for it): §4's "the knob still works" on an empty slot is left open.
- **`panel.restore` from a client** (an undo sent back) is judged by the host. A text a patch of this host wrote or replaced (its SHA-256 kept for the session) is taken whole: the undo and redo of what the host did, a hand-written part removed with its slot included, which so always comes back. Any other text holds only what the panel editor writes: an HTML file's `<signal-panel>` content in its canonical form (`serialize(parse(content))` equal to it: no comment, so no comment trick hides markup), outside that element what the file holds now, and a part that does not exist exactly as a new part is written (its comment and its `<signal-panel part>`, no other attribute); `ui/windows.toml` must read as TOML. So an inverse cannot put a script into a panel. A part whose removal was kept (another writer changed it meanwhile) is left out of the reply's inverse: it is still there, and the undo leaves it as it is.
- **The open slot card** keeps the part it read as its own state (`partState`), never in the element's `part`, which is the DOM's own (the operator's visual check, 2026-10-30, found `part="[object Object]"` and no part drawn: fixed, `ui/controls/slot.test.js`). It reads its part once its connection is open (the page attaches its controls while connecting, and a request then is refused `closed`: the second check found the read failing so), again on every reconnect; until it knows, and when the read fails, it shows no interface knobs, the failure said in the card (`crates/signal-ui/tests/parts.rs`, `the_page_reads_its_part_from_a_real_server`, drives the page's own modules against a real server). It is `display: contents` (its header a grid item, its part's cards grid items of the host's grid); the app's card code lays it out by its own box (`cardBox`). The part is drawn in the slot's light DOM, rooted, so the page's CSS (the studio's `studio.css`) styles the moved cards as before. The **fold** is view state of the page, open by default; keeping it per window in `ui/windows.toml` (Signal.app's writer) is left open. **Edit layout** of a part starts with the host's Edit layout: a panel editor of its own in each open slot card (its tools in the card, its palette the slot's addresses made relative, its first write creating the file), one undo order across the files (⌘Z, and every Undo button, undo whichever file was written last). The open slot card itself is a control of the host's tree under the cards: the host's editor can move it into a row (it becomes the compact card) but offers no drop back among the cards (phase 2's dragging).
- **`describe.parts`** is a top-level list (each slot's part with `slot`, `roots`, its controls rooted at the first root) rather than a field of `content.slots`; `graph`'s `slot.part` adds `base` and `root` to `{file, controls, error}`. Both additive.
- **Tests:** test 1 `crates/signal-cli/tests/slot_group.rs` (the grouped studio plans byte-identically and reproduces the golden's audio and log over 120 s, blocks of 1, 64, 256 on 1 and 4 threads; a primitive and two project includes); tests 2 and 4 `crates/signal-ui/tests/parts.rs` over both transports and `crates/signal-project/src/patch/group_tests.rs`; test 3 `crates/signal-ui/tests/parts.rs` and `crates/signal-ui/src/parts/tests.rs`; test 6 the existing `signal ui check` and golden tests, unchanged; test 7 `ui/lib/parts.test.js`, `ui/lib/schematic-parts.test.js`, `ui/app/lib/cards.test.js`, `ui/controls/panel.test.js` (the open and compact card's choice, card ids, the closed box with its controls, the opened frame, a drag; no browser is driven: the operator's check in Signal.app is phase 1's last exit condition).

**2026-10-30, the slot's face** (operator; the owner, trying phase 1: "not much change in the schematic; grouping works but it's clumsy, and the many knobs laid out vertically make huge columns. Could a slot have a panel header with the same layout as on the panel itself?"). §6 is drawn differently, its decided question 1 kept (the controls inside one bigger box): a closed slot box with a part shows the part itself, its **face**, under its title, rendered as the panel renders it (the same cards, CSS and skins, live and operable), instead of a column of control nodes; its pins stay on its edges, and a wire into a parameter a face control binds ends at its pin, the control outlined in the face when the wire or the pin is hovered or selected. Opened in place, the frame shows the face at its top and the inside below it, and the part's interface controls get no frame nodes either; a part's controls on inner addresses still stand beside their inner instance. Presentation only: no format, protocol or plan change (the face reads the part with `panel.read`, as the open slot card does). Also: a box's many host-panel controls stand in a grid, the marquee takes control nodes and starts with ⌘-drag too, and Group into slot moves the controls selected with the box. `docs/ui-controls.md` "Slot faces"; tests `ui/lib/schematic-face.test.js`, `ui/lib/schematic-parts.test.js`, `ui/lib/schematic-marquee.test.js`, `ui/controls/schematic-faces.test.js`.
