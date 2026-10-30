# ADR 0047: The front plate and the board: one device in layers

- **Status:** Proposed (2026-10-30). The owner's idea (translated, condensed): "The panel is the front plate; the schematic is the circuit board. Make them two layers of one device that share the controls as common anchors, as in real hardware: knobs and faders on the front plate, below it a PCB, on it chips, inside the chips microelectronics. Switching from panel to schematic is like lifting the front plate: the controls stay where they are and the circuit is revealed. The schematic is a microscope: zoom in to see ever smaller structures." Asked three questions, the owner answered: (1) the front panel's geometry must be **fixed like a physical object**: today a narrow window wraps the MONO SYNTH's knobs into three rows and a wide one into one, and changing monitors must never move anything; (2) modules sit **near** their controls, not strictly under them, and a module such as `moog_voice` is a chip whose inner structure is revealed by **zooming into it, not by opening it**; (3) stay inspired by physical reality, in 2D (plate, board, chips with their own inner schematics), and find a way to generate clear, readable wiring automatically: today there is a lot of wire chaos. The approach below is the operator's, agreed with the coordinator; the questions of taste are at the end.
- **Builds on:** ADR 0009 and 0013 (text is the truth), 0023 §3–4 (no UI dependency or build step; panels are HTML in `ui/`), **0026** (the schematic, its layered layout, `ui/schematic.toml`, the panel editor), **0030** (tracing, in-place opening, bundles, port groups, text that scales with zoom), **0032** (windows and cards across them, `ui/windows.toml`), 0034 (the keymap), **0037** (slots, editable insides), **0046** (a slot's part and its face in the schematic).
- **Amends, when accepted:** ADR 0026 §4 (the panel's responsive card grid that "reflows with the window" becomes a fixed plate, §1) and §3 (the layout algorithm, §6; positions stay where they are); ADR 0030 §2 and its refinements of 2026-10-20 to 2026-10-22 (opening in place is replaced by deep zoom in phase 3, §8); ADR 0032 §7 (what a panel window shows, §3); ADR 0046's implementation note "the slot's face" (a face is its part's plate, §4). **Supersedes nothing accepted as a whole**; ADR 0030 §1 (tracing) and §3 (bundles, port groups) stay and are extended.
- **Date:** 2026-10-30

## Context
- **The panel reflows (fact).** `<signal-panel>` is a CSS grid of `minmax(var(--signal-card-min), 1fr)` columns (`ui/controls/theme.css`), `wide` cards span a row, and rows and groups are flex boxes that wrap. The studio's `studio.css` lets the MONO SYNTH's groups wrap "so the panel never scrolls sideways". So the same panel has a different geometry on every window width, and therefore on every monitor (ADR 0032 restores window sizes per screen key, so another desk gives another panel).
- **The schematic knows nothing of the panel's geometry (fact).** Its layout (`ui/lib/schematic-layout.js`) is longest-path layering with four barycentre sweeps and smooth curves (`schematic-wire.js`); boxes that a control binds get control nodes beside them (`schematic-controls.js`), and a slot with a part shows its face, laid out at 1280 CSS px and scaled into its box (ADR 0046, `schematic-face.js`). Nothing relates a knob's place on the panel to its module's place in the schematic.
- **Wire chaos (fact, from the owner; measured only by eye so far).** Long edges are curves that cross boxes and each other; high-fanout nets (the transport's position and gate into every sequencer and voice) draw a fan across the level; feedback runs in lanes below everything. Bundles (ADR 0030 §3) help only between the same two boxes.
- **Opening in place moves things (fact).** An opened composite grows into a frame and pushes its neighbours aside (`makeRoom`, ADR 0030's review of 2026-10-20). The owner first had it open on zoom, then asked for a click and an × (2026-10-22). Now the owner asks for zoom again, but without opening: what disturbed was things jumping, and here nothing moves (§8).
- **Library composites are closed (fact).** `graph` carries no library composite's inside (ADR 0030 "Not done"), so `lib.ladder_voice` cannot be looked into at all.
- **What is presentation (fact, ADR 0046 Context):** a render never reads `ui/`, and the golden manifests hash no `ui/` file. No example project has a `ui/schematic.toml` today (checked on f910a12), so stored schematic positions need no migration.
- **Licences (fact):** ELK is EPL-2.0, outside the set ADR 0031 allows; dagre is MIT but a dependency, which ADR 0023 §3 and 0026 exclude for the UI. The layout stays in-house.

## Decision

### 1. One device, three layers, one coordinate space
- A project's interface is drawn as **one device**: the **plate** (the front panel: cards and controls), the **board** (the top level's instances and connections), and **chips** (modules on the board, whose insides are reached by zooming, §8). Panel and schematic become two layers of one view.
- **One coordinate space, the device's:** 1 device unit = 1 CSS px of the plate at its reference size. The plate's top-left corner is the origin. The board's positions (`ui/schematic.toml` `[top]`) are in the same unit, as the schematic's units are today, so stored positions keep their numbers.
- The **plate unit** (`pu`) is 8 device units: the grid that cards and parts snap to (a Eurorack HP in spirit). 1280 px, the reference width, is 160 pu.

### 2. The plate: fixed geometry, scaled whole (phase 1)
- **Every card has a fixed place on the plate, written in the panel file**:
  - `<signal-panel plate="160 90">`: the plate's width and height in pu;
  - `<section class="signal-card" place="0 12 160 38">`: the card's x, y, width and height in pu; the same attribute on `<signal-slot>` as a card (the open slot card, ADR 0046 §5).
  - Both are whitelist additions (`ui/lib/panel-whitelist.json`, `panel_write.rs`), integers, checked by `panel.check`: a card outside the plate is an error; two cards overlapping, or content taller than its card (measured by the page and reported back with the check, as diagnostics are shown today), a warning.
- **The plate is laid out once, at its own size, and scaled uniformly** to the window by one CSS transform. A card's content is laid out at its card's fixed width, so rows and groups wrap the same way on every window, monitor and scale factor. The window changes only the scale, never the arrangement. `wide`, the column grid and `--signal-card-min` stop shaping a placed panel (they stay for a panel without places, below).
- **Scaling:** fit the plate's width to the window, scroll vertically; never below a minimum scale (0.5, tunable), under which the plate pans sideways as well. ⌘+, ⌘− and ⌘0 (fit the whole plate) are keymap entries (ADR 0034). The scale is view state, not text.
- **A panel without places is still a fixed plate.** It is laid out once at the reference width (1280 px, ADR 0046's face width) and that arrangement is used at every window size (an *implicit freeze*). So phase 1 removes reflow from every panel at once, before any file is written.
- **Freeze (migration, one time):** *Freeze layout* in the panel editor measures the implicit arrangement, rounds each card's box to whole pu (outward, so nothing gets smaller), and writes `plate` and `place` through `panel.write`. No new protocol message: the page measures, the server checks and writes as it does any edit; an agent writes the same attributes with `write_file`, as any panel text. The operator freezes every example project's panel in one commit and checks each against a screenshot of today's 1280 px layout.
- **The generated default panel** (`ui/default-panel.html`, `ui/lib/panel.js`) gets fixed geometry too: a pure packing function places its cards in their order on shelves 160 pu wide, each card's size computed from its controls' fixed cells (a knob, fader and meter each a fixed number of pu). The same project gives the same plate on any window. Its first editor write writes the places with it.
- **The panel editor** (ADR 0026 §4) moves cards on the grid (snapped to pu) and resizes them by their corners; dragging a card over another is refused with an outline, not pushed. Controls inside a card are still ordered and grouped, not placed (rows and groups stay the card's inner structure). A card whose content changes is refitted in height by the editor, never shrunk under its content.

### 3. Several windows
- The **main window's panel is the plate.** A **panel window** (ADR 0032) is another view of the same plate: it shows the cards it holds **at their plate positions**, cropped to their bounding box and scaled to fit, so their arrangement is the plate's. The main window keeps a placeholder at a moved card's place, as today.
- `card.order` and Move up / Move down are removed: order on the plate is place, edited in the panel editor. `ui/windows.toml` keeps `cards` as membership only. The scale and the layer (§5) of each window are view state, kept per window in `ui/windows.toml` beside ADR 0046's fold.

### 4. Parts are small plates
- A slot's part (ADR 0046) is a plate of its own: `<signal-panel part="synth" plate="160 40">`, its cards placed in its pu. The open slot card in the host is placed by its `place`, its size the part's plate plus the header.
- **The slot's face in the schematic is that plate**, at its own size, at one device unit per CSS px, scaled with its chip: no more "laid out at 1280 px and scaled to at most 720 units" (ADR 0046's face note). A part without places is frozen implicitly at 1280 px, as a panel is.

### 5. The board under the plate (phase 2)
- **Layers of the device view:** **Plate** (opaque: the panel as today), **X-ray** (the plate at about a quarter of its opacity, the board visible beneath, the controls drawn whole), **Board** (the plate gone but for its controls). Switching animates the plate lifting and fading (about 200 ms; instant with reduced motion). Signal.app's Panel and Schematic tabs stay, as the Plate and Board layers of the same view; the layer is per window (§3). Controls work in every layer; the schematic's selection, tracing and patching work in X-ray and Board.
- **Controls are vias.** Each plate control that binds the level is a via at its plate position: the control drawn where it is on the plate, with a short leg to the parameter or port pin it binds (the binding wires of `schematic-controls.js`, now from a fixed place). A host-panel control is no longer drawn beside its module: its place is the plate's.
- **Placement of chips:**
  - an **anchored** chip (one that plate controls bind) is placed **near its vias**: its preferred position is the vias' centroid, then it is moved to the nearest free spot that covers no via, no card's controls and no other chip, searched in a fixed spiral order of whole pu. Near, not under (the owner's answer 2);
  - **free** chips (no control binds them) are laid out by the layered layout (§6) in the board's free space, to the right of and below the plate, their layers ordered relative to the anchored chips they connect to (an anchored chip's layer is fixed by its x);
  - the board may extend beyond the plate in every direction.
- **Arrangement toggle:** *Arrange by controls* (above, the default) or *Arrange by flow* (every chip by the layered layout alone, today's schematic made readable), as view state. A position in `ui/schematic.toml` pins its chip in both.
- **Positions are text:** `[top]` and `[composite.<name>]` of `ui/schematic.toml`, unchanged in format (`project-format.md` §11); a dragged chip writes its position as today, and the layout fills every unpinned one. Via positions are not stored: they follow from the plate's places.
- **Determinism of vias:** a control's position is measured from the fixed plate and rounded to whole pu before the layout reads it, so engines that differ by a fraction of a pixel (WebKit in Signal.app, Chrome headless) give the same board.

### 6. Readable wiring (phase 2)
A new pure layout and router in `ui/lib/` (replacing `layout`'s inner steps; same inputs, same `node:test` style), deterministic: the result is a function of the level as written (arrays in source order), the stored positions, the vias and fixed constants. No randomness, no time, fixed iteration counts, ties broken by source order, never by an object's key order.
1. **Layered (Sugiyama) layout, left to right:** feedback edges reversed (the existing depth-first search), layers by longest path then shortened by a balancing pass, **dummy nodes** on long edges, **crossing minimisation** by layer sweeps (median and barycentre, port order aware: a pin's row on its box counts) with a transpose step, then **coordinate assignment** that keeps long edges straight (Brandes–Köpf, simplified). Pinned and anchored nodes are fixed constraints.
2. **Orthogonal routing in channels:** wires run horizontally along their dummy nodes and turn vertically only in the channels between layers, each vertical segment on a **track** of its channel (left-edge track assignment), so parallel wires never overlap and never pass under a box. Corners are rounded (`rounded` in `schematic-wire.js`). Feedback runs in the lanes below, as today.
3. **Net labels:** a net with 4 or more destinations, or one that would span more than 3 layers, or one between the plate's area and free space, is drawn as a stub at each end with the net's name (`▸ transport.position` at the source, `◂ transport.position` at each destination), not as a long wire. A click on a label traces the net (ADR 0030 §1), drawing it whole while traced.
4. **Buses:** wires that share a channel between the same groups of boxes merge into one bus with a count (`8`), fanning out at their ends; ADR 0030's pair bundles become the simplest bus. The fan-out by zoom (4 to 8 px per wire on screen) stays.
5. **Signal-kind layers:** signal, events and vector wires (the existing colours of `shapeName`) are three toggleable layers. A hidden layer's wires are not drawn and its pins are marked; **the layout never depends on the toggles**, so the picture does not move when one is switched. Toggles are view state.
6. **Measured, not judged:** the layout reports its crossings, bends and total wire length per level. They are shown to the operator and owner beside the drawing; they never choose between layouts (hard rule 6). Readability is the owner's verdict.

### 7. Chips and their packages
- A module on the board is drawn as a **package**: its body (the box today), its pins as legs on its edges, its title and, for a slot with a part, its face (§4) on top. Primitives are packages without an inside.
- A package's size is computed from its pins as today, widened (up to a fixed maximum aspect) towards its inside's aspect, so the inside is not drawn as a sliver (§8). The size depends on the layouts only, never on the view.

### 8. Deep zoom into chips (phase 3)
- **Semantic zoom, nothing opens.** A composite's or slot's inside (its **die**) is drawn **inside its package's footprint**, at the scale that fits the die's layout in it. It fades in once its text would be drawn at 4 px or more on screen (ADR 0030's level-of-detail rule), and is fully drawn from 7 px. Zooming further reveals the dies inside its chips, recursively. **No box grows, no neighbour moves, there is no frame and no ×.**
- **Bond wires:** each package pin is joined to its die's boundary pin (`@inputs` / `@outputs`, the frame pins of today) by a short bond wire drawn inside the package, so a traced path is one continuous drawing from the board into the die and back (ADR 0030 §1's "links" made permanent).
- **A slot with a part:** its face is the chip's own front plate. Zooming into it lifts it as the device's plate lifts (§5), revealing the slot's inside below it with vias from the part's inner controls (`/vcf.cutoff`, ADR 0046 §6): plate and board recur at every slot level.
- **The inside's layout belongs to the module, like a datasheet:**
  - a project composite's: `[composite.<name>]` in the project's `ui/schematic.toml`, as today; "Save slot as project…" (ADR 0046 §9) copies it with the composite;
  - a slot's own inside: its composite's table, as today;
  - a library composite's: a presentation file shipped with the library, `crates/signal-lib/schematic/<name>.toml` (one `[composite.<name>]` table, same format), embedded beside the composite's text but **not** in its pinned text hash or a render's manifest (rendering never reads it); a test checks that each names only existing instances. Without one, the auto-layout draws it. Library insides are read-only, as today; `signal vendor` copies a shipped layout into the project's `[composite.<copy>]` with the composite.
  - `graph` gains library composites' insides and their shipped layouts, additively (`ui-protocol.md`), built by `signal-ui` from the library text it already loads.
- **Editing at depth:** edits (move, patch, connect, the inspector) act at the **focus level**: the deepest level under the pointer whose text is drawn at 7 px or more. The breadcrumbs show the focus path. A shared composite's inside says so on its die ("shared by 3") and an edit there edits its file, as today; a slot's own inside is copy on write (ADR 0037 "Editable insides"); a library die is read-only.
- **What becomes of opening in place:** a click selects; a double-click, Enter, a breadcrumb and the menu's *Zoom into* (formerly *Open in place*) animate the view until that chip's die is the focus level; Escape zooms out one level; Fit shows the whole device. `ui/lib/schematic-open.js`'s frames, `makeRoom` and the × are retired.
- **Precision:** the view is kept relative to the deepest level that fills the screen (re-rooted as the zoom passes each level), so coordinates stay small at any depth; ADR 0030's ten-level reach (a total scale of 1e40) is kept.

### 9. Performance budget
- **Layout:** the studio's top level with its vias in under 50 ms; each composite's inside laid out once per `graph` version and shared by every instance; levels laid out lazily, when first drawn.
- **Rendering:** a die is drawn only while its package is on screen and its text at 4 px or more; below, the package is drawn closed. At most 5 000 drawn SVG elements at any view (tunable); 50 fps pan and zoom on the studio and `library-demo`, as ADR 0030 §5, measured on the Mac (the sandbox misleads).
- **The plate:** built once per panel text; a window resize changes one transform only.

### 10. The hard rules
- **Rule 1:** `signal-core`, `signal-graph` and `signal-engine` learn nothing. The plate's geometry is panel text (`signal-ui`'s whitelist and check), the layout and routing live in `ui/`, library insides in `graph` come from `signal-ui`, the shipped layouts from `signal-lib` as presentation files. `domain_scope.rs` is unchanged.
- **Rule 3:** no plan, render or log changes. The layout is deterministic in the sense of §6: the same files give the same picture on every window, monitor and machine.
- **Rule 4:** the plate's geometry is in the panel file, chip positions in `ui/schematic.toml`, library layouts in the library's files. The layer, scale, arrangement and kind toggles are view state per window in `ui/windows.toml`. Nothing is remembered elsewhere.
- **Rule 6:** crossings and lengths are shown, never decide.
- **Rule 7:** no dependency, no build step.
- **Goldens:** none moves. No golden hashes a `ui/` file, and the library layouts are outside the pinned hashes.

## Alternatives considered
- **Keep the responsive grid, make it stable by breakpoints:** fewer jumps, but still a different object on another monitor; the owner asked for a physical object.
- **Free pixel positions without a grid:** exact but untidy, and a knob a pixel off its neighbour would show; a pu grid keeps alignment by construction.
- **The plate's geometry in a TOML file beside the panel, or in `ui/windows.toml`:** a second file to keep in step with the HTML, and windows.toml is per screen key, which is exactly what the plate must not depend on.
- **Modules strictly under their controls:** the owner chose near; strictly under would cover the plate's controls in X-ray and break the flow.
- **ELK or dagre for the layout:** ELK's EPL-2.0 is not allowed (ADR 0031); dagre is a dependency and gives no channel routing or net labels.
- **Keep opening in place, only fix its jumps:** the owner asked for zoom, and a frame that grows over its neighbours cannot avoid moving them.
- **A chip's inside layout in its composite file:** it would move the composite's hash, so every render's provenance and the library's pins, for a change of presentation.
- **Canvas or WebGL for deep zoom:** faster for very many elements, but text, hit testing and the live faces are HTML and SVG today; the level-of-detail budget keeps SVG within limits. Kept as the fallback if phase 3's measurement misses §9.

## Consequences
- **`ui/`:** `panel.js` and `theme.css` (the placed plate, its scale), the panel editor (grid moves, resize, Freeze), `ui/lib/panel.js` (the default panel's packing), `panel-whitelist.json`, `ui/app/lib/cards.js` (cropped panel windows, no `card.order`), a device view joining `<signal-panel>` and `<signal-schematic>` with its layers, new `ui/lib/` modules for the layered layout, the router, net labels and buses, `schematic-face.js` (faces as plates), `schematic-open.js` retired in phase 3.
- **`signal-ui`:** the whitelist and check for `plate` and `place`; `graph`'s library insides and shipped layouts (phase 3).
- **`signal-lib`:** `schematic/<name>.toml` presentation files, embedded, with their own check (phase 3).
- **`signal-app`:** the per-window layer and scale in `ui/windows.toml`; no `card.order`.
- **Docs:** `ui-controls.md` ("Panels", "Cards across windows", "Schematic", "Slot faces"), `project-format.md` (§11 and the panel's attributes), `ui-protocol.md`, `agent-interface.md`, vision §3a (Plate and Board, as kinds of view, not structural terms), `roadmap.md`.
- **The example projects' panels** are rewritten once by Freeze (phase 1); their goldens are untouched.

## Tests
1. **Fixed plate:** the placed and the implicitly frozen studio panel give identical card and control rectangles in pu at window widths 600, 1280 and 2560 and scale factors 1 and 2 (headless Chrome); the scale is the only difference.
2. **Freeze:** freezing each example panel writes `plate` and `place` whose boxes contain the 1280 px layout's boxes; the frozen panel's arrangement equals the implicit one; undo restores the file byte for byte.
3. **Check:** `place` outside the plate is an error, overlaps and overflow are warnings; the whitelist refuses other forms.
4. **Default panel:** the packing is a pure function: same `describe`, same plate; no overlap.
5. **Layout determinism:** the same level twice, and with every object's key order shuffled, gives the same nodes, routes and labels; layout snapshots (JSON) of the kick, studio, studio-transport, physical-av and library-demo levels, updated only with a recorded reason.
6. **Routing:** no route crosses a box; no two vertical segments share a track; net labels at the thresholds; buses' counts; toggling a kind changes no position.
7. **Anchoring:** an anchored chip covers no via and lies within a bounded distance of its vias' centroid; a pinned position always wins.
8. **Deep zoom:** the fade-in thresholds; bond wires join every package pin to its die pin; the focus level; nothing moves when a die is drawn; library dies read-only; a shipped layout naming a missing instance fails `signal-lib`'s test.
9. **Performance (measurement):** §9's budgets on the Mac, recorded.
10. **Every example project** passes `signal ui check`, and every golden is unchanged.
11. **The owner's verdicts:** one per phase.

## Phases
| Phase | Delivers | Exit |
|---|---|---|
| 1. A fixed plate | §1–4: `plate` and `place`, the implicit freeze, Freeze and the frozen example panels, uniform scaling, the default panel's packing, grid editing, parts as plates and faces as their plates, cropped panel windows | Tests 1–4, 10; the owner sees the studio's MONO SYNTH identical on a narrow and a wide window and on another monitor |
| 2. The board | §5–7: the device view and its layers, vias, anchored and free placement, the arrangement toggle, the layered layout, channel routing, net labels, buses, kind layers, packages | Tests 5–7, 9 (layout, frame rate), 10; crossings and lengths reported against today's layout for the studio and library-demo; the owner's verdict on the studio's wiring |
| 3. Deep zoom | §8: dies in packages, bond wires, level of detail, library insides and shipped layouts in `graph`, editing at the focus level, slot faces lifting, opening in place retired | Tests 8, 9 (deep zoom from the board into `moog_voice` to a primitive at 50 fps), 10; the owner's verdict |

## Open questions of taste (for the owner)
1. **The plate unit:** 8 px at the reference size (160 pu across 1280 px). *Proposed: yes.*
2. **A window narrower than the plate:** fit the width and scroll down (to a scale of 0.5, then pan sideways), or always show the whole plate? *Proposed: fit the width; ⌘0 shows the whole plate.*
3. **The layer switch:** three positions, Plate, X-ray and Board, on the toolbar and one key that steps through them, with X-ray at a quarter of the plate's opacity. *Proposed: yes; the Panel and Schematic tabs stay as shortcuts to Plate and Board.*
4. **The board's default arrangement:** chips near their controls, or by signal flow? *Proposed: near their controls by default, Arrange by flow one click away.*
5. **When a net becomes labels:** 4 or more destinations, or a span of more than 3 layers. *Proposed: yes, tuned at the operator's review.*
6. **The chip look:** a restrained package (dark body, pins as short legs, its title on it), or today's boxes? *Proposed: the package look, in the theme's colours.*
