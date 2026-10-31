# ADR 0050: One surface, zoom levels: Panel, Wiring, Control, Engine

- **Status:** Proposed (2026-11-01). The owner's vision is quoted in the Context. The level structure, the phases and what happens to ADR 0047 were agreed between the coordinator and the operator. The technical details below are the operator's (delegated). The owner named the levels on 2026-11-01. He left the other questions of taste open, so the operator's defaults are recorded as decided in §13; each stays open to change at the phase 1 verdict.
- **Builds on:** ADR 0009 and 0013 (text is the truth), 0023 §3–4 (no UI dependency; panels are HTML in `ui/`), 0026 (the schematic, `graph`, `ui/schematic.toml`, the panel editor), 0030 (tracing, wires that scale, precision at depth), 0032 (windows, cards across them, `ui/windows.toml`), 0034 (the keymap), 0037 and 0045 (slots, their interfaces), 0046 (a slot's part, Group into slot), **0047** (the plate, packages, routing, dies, semantic zoom).
- **Supersedes, when accepted, part of ADR 0047:** §1's "three layers" of one view and §5's layer switch (Plate, X-ray, Board), and the single top-level board that draws every wire kind of the level under the whole plate at once (§5, and phase 2's *Where the device view lives*). The rest of ADR 0047 stays (§8 below says what). **Amends:** ADR 0026 §2 (one view instead of a Panel page and a Schematic page), ADR 0032 (a schematic window becomes a surface window), ADR 0046 §6 (a slot with a part is a card, §2), and vision §3a (*Plate* / *Board* become *Surface*, *Level* and *Card*, as kinds of view, not structural terms).
- **Date:** 2026-11-01

## Context
**The owner's vision (2026-11-01, translated from Hungarian):**
> I imagined that details come forward as you zoom in. Zoomable layering: the top is the plate layer; zooming in cross-fades to a layer where the cables and strands are. Here we follow reality. The cables connecting instruments are physical cables in reality: visible and essential. The cables wiring the controls are usually inside the instrument's box, invisible to the user. In our case they are relevant: if we want an automated fader or knob, we want to see and wire what automates it. The third layer is the audio engine inside each instrument (one card = one instrument), and inside the audio engine there is a practically infinitely deep recursive structure. Reality is not infinitely deep, but a panel is measured in centimetres and a chip's conductors in nanometres, and there is distance between the two.
> Every instrument could have a small logo on the panel, under which its audio engine runs. There is no Panel and Schematic, no Plate / X-ray / Board: there is one surface, a zoom layer level, and visual fades between levels. For the SVG it would also be better if these layers were separate renderings; only the transitions need to be well designed.
> Example: the mono synth's top layer is the current plate. Its next layer shows the audio and control routing among the other cards. The synth has a well-defined interface: a 'MIDI2' or 'Control' in and a 'Mono audio' out. Zooming further, the UI elements layer shows, besides the 'factory default' routing, the automation control too. If something controls the mono synth's cutoff, the UI element shows it as a moving knob. If its internal value changes, it must have a separate display. The audio engine is for now just a small cube somewhere, but we already see which controls come in and which audio comes in and goes out. You can zoom into it; then the outer layer fades out and its inner life, its inner wiring, comes forward. If it is made of composites, you can zoom into those too.

**What exists (fact, main 5c2f3de):**
- **Two pages.** Signal.app's Panel tab shows the plate (ADR 0047 §1–4: fixed geometry, scaled whole). Its Schematic tab, a schematic window and `/schematic` show `<signal-schematic>` with the device view: the plate drawn as a face (`ui/controls/schematic-faces.js`, face `#plate`) over the top-level board (`ui/controls/schematic-board.js`). The layer switch steps through Plate, X-ray and Board (`ui/lib/board-view.js` `LAYERS`, key `schematic.layer`).
- **One board for everything.** The top level is laid out as one board under the whole plate. Every wire of the level (transport ticks, drum triggers, audio, every knob's leg) is routed on it at once. Each card's chips stay in its card's section (ADR 0047, the note on 7a62595). The routing is readable now, but it is the densest picture the project has, and it is the first thing shown below the plate.
- **The microscope works.** A package keeps its rectangle, its die is drawn inside it, and the die fades in with zoom (`shownOpacity`, `ui/lib/board-zoom.js`). The chip is opened about the zoom point (`risen`, `ui/lib/schematic-scene.js`) and its surroundings fade (`SURROUND`). Zoom never changes the selection. Insides are laid out once per `graph`. All of this is what the owner calls "zooming into the cube".
- **A knob already knows modulation.** A control shows its base as its pointer and, when a connection drives its parameter, the effective value as a dot on the arc and `→ value` on its readout's second line (`ui/controls/knob.js`, `Binding.modulated` in `ui/lib/binding.js`). The schematic already draws a wire into a bound parameter as the control's input (`ui/lib/schematic-controls.js`). The `parameters` stream accepts plan parameter ports, not only controls (`ui-protocol.md`), so a value inside a composite can be read without a protocol change.
- **The studio's plate** (`projects/studio/ui/main.html`) has five cards: the title, MONO SYNTH, RHYTHM COMPOSER, TAPE ECHO and master. A card binds several instances: RHYTHM COMPOSER binds `/transport.bpm`, `/drums.*` and the seven drum voices. Its panel has no `place` yet (implicitly frozen at 1280 px). No example project has a `ui/schematic.toml`, so stored positions need no migration.
- **"Instrument"** is in vision §3a only as a proposed role tag, not a structural term. This ADR does not adopt it: the unit below is the **card**, a kind of view, and "instrument" stays the owner's word for it.

## Decision

### 1. One surface, four levels
- A project is drawn as **one surface** in one element, `<signal-surface>` (the plate's and the schematic's code joined, §7). It replaces the Panel and Schematic pages. Its zoom is one continuous number, and the **level** follows from it. Nothing else is switched.
- **The levels:**

  | Level | Shows | Wires drawn | Unit of the drawing |
  |---|---|---|---|
  | **L0 Panel** | the plate (ADR 0047 §1–4) with a logo on each card (§5) | none | the whole surface |
  | **L1 Wiring** | each card as a box with its derived interface (§3) | the cables between cards only, of every kind (audio, control, events) | the whole surface |
  | **L2 Control** | one card: its controls where the plate has them, its engine as one package (the "cube"), the card's interface pins on its edge | binding legs (control → parameter), automation (any connection that drives a bound parameter, into the control's input), the engine's inputs and outputs to the edge | one card, inside its L1 box |
  | **L3 Engine** | the engine's die: the card's instances and their connections | the inner wiring | one package, inside it, recursively |

- **Each level is a separate rendering:** its own SVG group (L0 is the plate's HTML face layer), its own layout function and its own cache. No graph combines two levels. Each card is a group of its own in every level, so a card can be transformed, culled and faded alone.
- **Below L3 is ADR 0047 §8 unchanged,** and those depths have no names of their own: they are only depth. Composites and slots inside the engine are packages whose dies open by zoom, recursively, with the 1e40 reach of ADR 0030.

### 2. What a card is
A card is derived from the files. Nothing new has to be written for one. `cardsOf(graph, panel)` is a pure function in `ui/lib/surface-cards.js`. Its input is the top level's instances and connections, and the panel's cards and their binds (the generated default panel when there is no `ui/main.html`). It assigns every top-level instance to exactly one card, in this order:
1. **`owns`** (phase 3, §5): a card that names the instance in its `owns` attribute takes it.
2. **A top-level slot with a part, or with a slot card on the plate,** is a card of its own: the slot, its part and its content (ADR 0046's self-contained unit). Its plate rectangle is its slot card's.
3. **By controls:** each other plate card (`.signal-card` not inside another, with its card id, `ui-controls.md` "Card ids") takes the instances that most of their bound controls sit in. Readers count as binds (a scope on `/limiter.out`, a piano roll's `position-bind`). A tie goes to the earlier card in the panel's order.
4. **By flow** (unbound instances): ADR 0047's existing section rule. A chain of single links goes to its end's card. Any other instance goes to the card most of its flow neighbours belong to, its sinks weighing a hair more. Instances are taken in source order, repeated until nothing changes, with a fixed iteration bound.
5. **Infrastructure:** an unbound instance still unassigned, or one whose outputs reach two or more cards with no majority, becomes a **card of its own**, named after the instance. Such a card has no place on the plate (§4 places it).
- **Infrastructure is a card like any other.** A transport, a mixer bus and a master chain are cards whenever the rules above make them one: the studio's master card holds `bus`, `dc`, `limiter` and `out`. A card with no instances (the studio's title) exists only on L0.
- **Expected for the studio** (an assumption until phase 1's test fixes it): MONO SYNTH = {melody, synth}, RHYTHM COMPOSER = {transport, drums, the seven voices, drum_a, drum_b}, TAPE ECHO = {echo}, master = {bus, dc, limiter, out}. Its Wiring level then shows four cables: tick RHYTHM → MONO SYNTH, audio MONO SYNTH → TAPE ECHO, audio ×2 RHYTHM → master, audio TAPE ECHO → master. It also shows master's device output.
- **A control on another card's instance** (tempo on RHYTHM COMPOSER, if an `owns` moved the transport away): at L2 its leg leaves its card through an edge pin. At L1 it is a binding cable in the leg's tone (ADR 0047 §8a), so a remote control stays visible.
- **The engine** of a card is its instances and the connections among them, a presentation grouping and not a composite. With one instance, the engine *is* that instance: its package is the cube, and its die is that module's die (a primitive's cube stays closed). With several, the cube's die is the card's own level (§4, L3).

### 3. The derived interface
- A card's interface is **derived from its instances' ports, never hand-written**. It has one pin per port that a connection joins to an instance of another card (inputs on the box's left, outputs on its right, as everywhere). A **slot card** has its slot's whole interface (ADR 0037, 0045), connected or not: this is the owner's "well-defined interface". A card with a **sink** gets an output pin to its device role (ADR 0038). A card with a device source gets the input to match.
- A pin is named by its port (`out`; `synth.out` where two instances of the card share a name) and coloured by its kind (ADR 0047 §8a's tone).
- **Phase 3 groups the pins:** pins of one kind between the same two cards form one cable with a count (`control ×3`, the owner's "Control in": hz, gate and velocity from a sequencer on another card). The group expands on hover and on selection, as ADR 0047's buses and ribbons do. A slot card keeps its interface's own names and order.
- The interface is recomputed with `graph` and the panel. Nothing is stored.

### 4. Layouts, one per level
- **L0** is the plate exactly as ADR 0047 §1–4 has it. It is drawn as **one face per card** (instead of one face for the whole plate), so that each card can follow its own transform in the fade to L1.
- **L1, Wiring,** has two arrangements, chosen per window as view state:
  - **By plate** (the default): the cards keep the plate's arrangement (rows by vertical overlap, the order within a row by x) and each box keeps its plate card's aspect. Each gap between rows and between columns is widened to the tracks its cables need. Cards with no place on the plate stand in a column left of the Wiring level (sources) or right of it (sinks), at the height of the cards they connect to.
  - **By flow:** the boxes are laid out by ADR 0047 §6's layered layout (the 2026-10-31 placement and channel routing). This is ADR 0047's *Arrange by flow*, now for cards.
  - Cables are routed in the channels with ADR 0047's tracks, lanes and ribbons (§6, §8a, §8b). The whole Wiring level is drawn at a **level scale** (§6) so that it fills the stage at the zoom where L1 begins.
  - L1 positions are derived and not stored. A box is not dragged: the plate's arrangement is edited on L0.
- **L2, one card,** is drawn **inside its L1 box**, scaled uniformly to fit, as a die fits its package (ADR 0047's microscope, the owner's verdict on 9e81dc8). Because the box has the plate card's aspect, the card's controls stand where they stand on the plate:
  - the controls appear whole and live, without the card's decoration, as ADR 0047's X-ray drew them;
  - the cube is placed in the card's free area near its controls (ADR 0047 §5's anchored placement, the spiral search restricted to the card, the piano roll's and step grid's free areas allowed);
  - its parameters' pins form one connector (§8b.4), and the legs arrive as ribbons (`legPlan`);
  - the card's interface pins stand on the box's edge, where the L1 cables end, so a cable continues into the card without a break;
  - the cube's position is derived. It is not draggable and not stored (§13).
- **L3, the engine,** is the cube's die. For a single instance, it is that module's die with its layout (`[composite.<name>]`, or a library's shipped layout), as today. For several, it is the card's instances laid out by flow, with the card's interface pins as the die's frame pins and bond wires to the cube's pins. Their positions are `[top]` in `ui/schematic.toml`, unchanged in format and in `schematic.move`'s `level = ""`: since this ADR, a top-level instance's stored position is its place in its card's engine die.
- **Every layout is deterministic** in ADR 0047 §6's sense: arrays in source order, fixed constants, no time and no randomness. The same files give the same picture on every window, monitor and machine. A steady layout (ADR 0047 §8a.5, `board-memo.js`) applies at each level.

### 5. Logos and the panel's two new attributes (phase 3)
- **Every card that has an engine shows a logo** at the right end of its header on L0, and in its box's title on L1. The logo is the way into its engine: a click travels (ADR 0047's 300 ms camera, `zoomInView`) until its cube fills the stage at L2. A second click, or the zoom, goes on into L3. Hovering the logo names the engine (`moog_voice`; "4 instances").
- **The default logo** is the card's monogram (the initials of its heading, at most two letters) in a rounded square three plate units high, with a corner mark in the tone of its main output kind. It is generated, so nothing is written.
- **Overrides, in the panel file,** are whitelist additions (`ui/lib/panel-whitelist.json`, `panel_write.rs`) on a card element (`section.signal-card`, `<signal-slot>` as a card):
  - `logo="svg:assets/moog.svg"` (the `svg:` scheme of skins, resolved from `ui/`), `logo="text:MS"`, or `logo="none"`;
  - `owns="/transport /melody"`: top-level instances the card takes (§2 rule 1).
- **`panel.check` judges them.** An `owns` naming an instance that does not exist is a warning. An instance owned by two cards is an error. A logo whose file is missing is a warning, and the monogram is drawn instead.

### 6. Zoom, fades and anchoring
- **L0 ↔ L1 is one band over the whole surface.** Let *k₀* be the scale at which the plate's width fits the stage (ADR 0047 §2). L0 holds up to 1.5 *k₀*. The band runs from 1.5 *k₀* to 2 *k₀*, and L1 holds from 2 *k₀*. The Wiring level's level scale makes it fit the stage at 2 *k₀*, as the plate does at *k₀*. Both factors are tunable constants (`ui/lib/surface-zoom.js`).
  - Across the band, with *t* from 0 to 1 a function of the scale alone, eased:
    - each card's rectangle moves from its plate rectangle to its Wiring box. This is the **morph**: both levels draw each card in that interpolated rectangle;
    - the plate's opacity goes 1 − *t*;
    - the boxes' opacity goes *t*;
    - the cables appear over the band's second half (max(0, 2*t* − 1)), once the boxes are near their places.
  - **In and out are one curve**: no hysteresis and no timer, as in ADR 0047's semantic-zoom note.
- **The anchor is card-relative in the band.** The point under the pointer (or the view's centre for a key zoom) is held as *(card, u, v)*: the card under it and the relative position within that card's rectangle. The camera is solved every frame so that this point stays under the pointer. So the card being zoomed into never slides, and the other cards move continuously around it. Outside the band the anchor is the ordinary world point.
- **L1 → L2 → L3 are nested footprints**, with ADR 0047's semantic zoom unchanged:
  - a card's L2 fades in with `shownOpacity` (its box's share of the stage from `LEAVE_SHARE` to `SHARE_FULL`, its controls' labels from 4 to 7 px), and its Wiring content fades out inversely;
  - one card per level opens, the one under the zoom point (`risen`);
  - the rest of the Wiring level fades to `SURROUND` (the owner's "the outer layer fades out");
  - the cube's die (L3) opens the same way, then the dies inside it.
  - Nothing moves here: a box, a cube and a package keep their rectangles at every zoom.
- **Hits:** a level, or a card's L2, drawn under `HIT_OPACITY` is not hit. The more opaque one takes the press. In the band, the plate's controls are hit only while *t* < 0.5.
- **Zoom never changes the selection** (kept, tested). The selection is by address, and each level draws its nearest representative:
  - an instance: its card's box on L1, the cube on L2, its chip on L3;
  - a connection: its cable on L1 if it joins two cards, else its wire on L2 or L3;
  - a parameter: its control on L0 and L2.
  - Tracing (ADR 0030 §1) is highlighted on every level that shows a part of the net.
- **Keys:**
  - ⌘0 shows the whole plate (L0 fit);
  - Escape travels out one level;
  - Enter, a double-click or a breadcrumb travels into the selection's next level;
  - the breadcrumbs name the levels (`studio ▸ MONO SYNTH ▸ engine ▸ synth ▸ vcf`).
  - The key `schematic.layer` (L) and the Plate, X-ray and Board buttons are removed.

### 7. Pages and windows
- **Signal.app's Panel and Schematic tabs become one tab**, the Surface (§13). The Parameters and Setup tabs stay.
- **Edit layout** (ADR 0026 §4, 0047 §2) runs on the surface at L0. Turning it on travels to L0 and keeps the zoom inside L0 while it is on (⌘+ and ⌘− scale the plate, up to the band's start).
- **A schematic window** (ADR 0032) becomes a surface window. It opens at L1 the first time, and later at the zoom it was left at.
- **A panel window** (cards across windows) stays an L0 view of the cards it holds, cropped and fitted (ADR 0047 §3), and **never zooms past L0**: a performance surface cannot be zoomed into the Wiring level by accident.
- `/schematic` and `/project/main.html` in a browser (`signal ui`) both show the surface. `/` (the raw protocol page) is unchanged.
- **View state per window** (`ui/windows.toml`, presentation): the zoom (scale, centre, the anchored card), the Wiring level's arrangement and the hidden kinds. ADR 0047's `layer` is read and ignored, and a written state drops it.

### 8. What of ADR 0047 survives
| ADR 0047 | Under this ADR |
|---|---|
| §1–4 the plate: fixed geometry, `plate`/`place`, Freeze, scaling, parts as small plates, cropped panel windows | **L0, unchanged.** Drawn as one face per card (§4) |
| §1, §5 the three layers and the layer switch; X-ray at a quarter opacity; the plate lifting | **Superseded** by the zoom levels. X-ray's drawing (controls whole, decoration hidden) is how L2 draws a card |
| §5 one top-level board under the whole plate, every wire kind of the level together | **Superseded.** Cables between cards are on L1, legs and automation on L2 per card, the inner wiring on L3 |
| §5 chips near their vias; cards as board sections; Arrange by controls / by flow | Anchored placement survives **inside a card on L2** (the cube). By flow survives as the Wiring level's second arrangement |
| §6 layered layout, channel routing, buses, kind toggles, measures | **Kept**, used on L1, L2 and L3 |
| §7 packages, `shapePackage` | **Kept.** The cube is a package |
| §8 dies, bond wires, library insides in `graph`, editing at the focus level; the microscope verdict; the semantic-zoom note | **Kept as L3 and below**, and the model for L1 → L2 |
| §8a ribbons, one colour per net, every element hit, screen-constant corners, a steady board | **Kept** at every level. Ribbons are a drawing of shared runs, not a layout, so they stay where runs are shared: the Wiring level's channels, legs into the cube, a die's connectors |
| §8b no labels, lanes, legs as ribbons, connectors | **Kept**: on L2 (the card's legs into the cube's connector) and in dies |
| §9 the budget | **Kept and applied per level** (§10) |

`schematic-faces.js`, `schematic-board.js`, the `board-*.js` modules and `knob.js` are reused. Only the layer switch's code is retired (`LAYERS`, `XRAY_OPACITY`, `LIFT_MS`, `LAYER_OPACITY`, `fadeSteps` in `board-view.js`, and the bar's buttons), along with the top-level board layout (`layout`'s `board` option over the whole level, replaced by L2 per card).

### 9. Editing, and what is text
| Where | What is edited | Written by (existing paths only) |
|---|---|---|
| L0 | cards, controls, binds; logos, `owns` (phase 3) | `panel.write`: the panel's HTML |
| L1 | **patch cables** between cards: drag from an output pin to an input pin. Dragging from a card's edge offers its unconnected ports (and a slot's interface). Removing a grouped cable removes its connections in one patch, after naming them | `patch` (`connection.add`, `connection.remove`): `project.toml` |
| L2 | **automation**: drag from any output (an edge pin, the cube's output, a neighbour card's pin) to a control's input, which connects to the bound parameter. Change a control's bind; Group into slot (ADR 0046) | `patch`; `panel.write` |
| L3 and below | **inner wiring**, instances, a die's positions, at the focus level (ADR 0047 §8) | `patch` at the level (`""` for a card's engine, a composite's name, a slot's own inside); `schematic.move` |

- **An instance added on a card's L3** is placed in that card. If §2 would derive it elsewhere, the same edit writes it into the card's `owns` (phase 3; before that, it simply lands where §2 puts it).
- **Rule 4:** levels, fades, the morph, the Wiring level's layout and the cards themselves are presentation, derived from `project.toml`, the composites, the panel and `ui/schematic.toml`. The new presentation data is only `owns` and `logo` in the panel file and the per-window zoom in `ui/windows.toml`. No hidden state.
- **The UI protocol and `signal mcp` are unchanged.** Every edit goes through the existing paths (`patch`, `panel.write`, `schematic.move`, `commit`), and the inner value (§11) is read with the existing `parameters` stream. An agent writes `owns` and `logo` with `write_panel`, as any panel text. `ui-protocol.md` changes only in wording ("a schematic window" becomes "a surface window"), and `agent-interface.md` gains the two attributes.

### 10. Performance
- **Lazy, per level:**
  - L1 is laid out once per `graph` version and panel text: the studio's Wiring level in a few ms, and under 50 ms on any example project;
  - a card's L2 is laid out when it first opens and kept per `graph`, under 50 ms for the studio's largest card (RHYTHM COMPOSER: 26 knobs, the step grid, one cube);
  - L3 dies, as today, once per `graph`, shared by every instance;
  - a zoom frame lays nothing out (a test, as ADR 0047's).
- **Culling:**
  - a level at opacity 0 is not in the drawing (L0's faces get `visibility: hidden`, so the plate's controls keep their elements and streams);
  - a card off screen is not drawn;
  - a card's L2 is drawn only while it opens;
  - a die follows ADR 0047's level-of-detail rule.
  - Still at most 5 000 drawn SVG elements at any view, and 50 fps pan and zoom on the studio and `library-demo`, through the morph too, **measured on the Mac** (the sandbox misleads).
- **The controls are not duplicated:** the plate's live elements are the L2 controls, shown through under their card's transform. So L2 adds no `parameters` subscriptions but the inner values (§11), at most 64, only for controls on screen.

### 11. The Control level: moving knobs and the inner value (phase 2)
- **Three values per writing control**, all from existing streams:
  - the **base** (the person's setting, what a drag writes);
  - the **effective** value at the bound address (the base combined with the connections that drive it, `semantics.md`);
  - for a bind that reaches inside a composite, the **inner** value at the export's first target (where the engine's own modulation also acts), as ADR 0026's level bars meter a composite's port at its first target.
- **A moving knob:** a driven control shows its effective value as a moving arc from the base to the effective value, in `--signal-effective`. Today's dot stays at the arc's end. The pointer stays the base, because the pointer is what the hand holds and what is saved. A fader draws the same as a band beside its thumb. This is the same element on L0 and L2, so the plate gains it too (§13).
- **The separate display:** on L2 only, each control whose bind reaches inside a composite gets a small readout beside it. It shows the inner value (`cutoff 1.2 kHz`) with a bar on the control's range. Its space is reserved (`ui-controls.md`'s fixed readouts), and it is empty while the inner value equals the effective value. It is part of the L2 drawing, not of the plate's HTML, so L0 is unchanged.
- **Automation wires:** a connection into a bound parameter is drawn from where it comes from to the control's input (`schematic-controls.js`'s rule). It comes from an L1 cable's edge pin when the source is on another card, and from the cube's output when it is inside the engine. A connection into a parameter that no control binds stays inside the engine (L3). The binding legs are the "factory routing".

### 12. The hard rules
- **Rule 1:** the core learns nothing. Cards, levels, layouts and zoom live in `ui/`. The two attributes are `signal-ui`'s whitelist and check. `domain_scope.rs` is unchanged.
- **Rule 3:** no plan, render or log changes. No golden moves (no golden hashes a `ui/` file).
- **Rule 4:** §9.
- **Rule 6:** the layouts' measures (crossings, bends, length, per level) are shown, and never choose. Each phase ends on the owner's verdict.
- **Rule 7:** no dependency, no build step.

### 13. Names and defaults (decided 2026-11-01)
- **The levels are Panel, Wiring, Control and Engine** (the owner). The depths below Engine (composites and slots inside, recursively) have no names: they are only depth, and the breadcrumbs name the instances on the way down.
- **The tab and the window are called Surface** (the operator's call): it names the one place, not a level, so "the Surface at Panel level" reads plainly. The element is `<signal-surface>`.
- The owner did not answer the remaining questions, so these operator defaults are decided. The owner may change any of them at the phase 1 verdict:
  1. **Moving knob:** the pointer stays the setting, and a moving arc shows the effective value, on Panel as well as on Control. On Control, a separate readout shows the inner value (§11).
  2. **Wiring arrangement:** by plate by default, so the zoom from Panel stays continuous. By flow is one click away (§4).
  3. **Transport:** it follows its controls, so in the studio it belongs to RHYTHM COMPOSER and Wiring shows a clock cable to MONO SYNTH. `owns="/transport"` on a card of its own overrides that (§2, §5).
  4. **Plate lock:** the main window has none for now. Panel windows stay plate-only, as the performance surface (§7).
  5. **Logo:** a two-letter monogram in a rounded square with a colour mark (§5). The engine cube on Control is placed by its controls, and it is not draggable or stored (§4).

## Alternatives considered
- **Keep the layer switch and add zoom beside it:** two ways to the same pictures, and the owner said there is no Plate / X-ray / Board.
- **One combined graph, filtered by zoom (level of detail):** one layout must then serve the Wiring level and the card at once, which is what made the top-level board dense. The owner asked for separate renderings.
- **Levels by absolute scale only, with every level drawn at the plate's scale:** the Wiring level could then never be seen whole, since zooming out would return to the plate. Hence the Wiring level's level scale (§6).
- **A plain cross-fade between L0 and L1 without the morph:** the cards would be drawn twice in different places during the band. Anchoring the card under the pointer keeps one of them steady, but the rest would ghost.
- **A card = a top-level instance:** the studio would have 18 boxes on L1 (seven drum voices, three mixers), not instruments, and the plate's grouping would be lost.
- **A card = a composite or slot only:** most example projects wire primitives at the top level and would have no cards.
- **The card grouping written as a composite in `project.toml`:** it would change the project's structure, its goldens and its hash for a presentation choice. `owns` in the panel stays presentation.
- **The pointer following the effective value (a motorised knob):** honest about the sound, but the knob would move under the hand and hide the saved value. §13 keeps the pointer as the setting.

## Consequences
- **`ui/`:**
  - new: `<signal-surface>` (joining `panel.js` and `schematic.js`), and `ui/lib/surface-cards.js`, `surface-wiring.js` and `surface-zoom.js` (the bands, the morph, the card-relative anchor), each pure with `node:test` tests;
  - changed: the plate as one face per card (`schematic-faces.js`), and L2 from `schematic-board.js` restricted to a card with one cube;
  - phase 2: the knob's and fader's moving arc (`knob.js`, `fader.js`), the inner readouts, and automation editing on L2;
  - phase 3: logos (`panel-whitelist.json`);
  - removed: the layer switch and the whole-level board.
- **`signal-ui`:** `owns` and `logo` in the whitelist and `panel.check` (phase 3). No protocol message changes.
- **`signal-app`:** one tab instead of Panel and Schematic; the schematic window's page becomes the surface; panel windows locked to L0; `layer` dropped from the view state.
- **Docs:**
  - `ui-controls.md`: "Panels", "Schematic" and "Cards across windows" become "The surface";
  - `project-format.md`: §11 on `[top]`'s meaning, and the panel's two attributes;
  - `ui-protocol.md` and `agent-interface.md`: wording and the attributes;
  - vision §3a: *Surface*, *Level* and *Card* replace *Plate* / *Board*;
  - `roadmap.md`.
- **Lost (a cost):** no single picture shows every wire of the top level any more. Tracing, which crosses levels, and L1's "every cable between cards" replace it. The owner judges whether that is enough.
- **The morph is new motion.** It is continuous with the zoom and never happens on its own, but it is the one place where things move while zooming, so it is the first thing the owner's phase 1 verdict judges.

## Tests
1. **Cards:** `cardsOf` on every example project's files, snapshotted. Studio as §2 expects. Each instance in exactly one card. The same result with every object's key order shuffled. Slots, `owns` and infrastructure each have a fixture.
2. **Interface:** one pin per port that crosses cards. A slot card's pins are its interface. Device pins. Phase 3's groups and their counts.
3. **Wiring:** deterministic; no route crosses a box; the plate's order is kept; plateless cards in their columns; a layout snapshot per example project, updated only with a recorded reason.
4. **Zoom:** opacities monotonic across each band, 0 and 1 exactly at its ends, one curve in and out. The anchored card's relative point within 1 px of the pointer at every step of the band. No rectangle changes outside the band's morph. The selection unchanged in and out. No layout per zoom frame. Hits per opacity.
5. **L2:** the controls at their plate positions within its box (within a unit). The cube inside the card, covering no control. Every L1 cable ends at the edge pin where L2's wire starts. Automation drawn for every connection into a bound parameter, and only those.
6. **Edits:** L1, L2 and L3 send the existing `patch` and `panel.write` messages with the expected ops. Undo restores the files byte for byte.
7. **Moving knob and inner value** (`FakeConnection`): the arc follows the effective value, the pointer the base. The readout reads the first target's address and is empty when equal.
8. **Performance (measurement, the Mac):** §10's budgets recorded.
9. **Every example project** passes `signal ui check`, and every golden is unchanged.
10. **The owner's verdicts:** one per phase.

## Phases
| Phase | Delivers | Exit |
|---|---|---|
| 1. One surface | §1, §2 (rules 2–5), §3's plain pins, §4 (L0 per card, Wiring by plate and by flow, L2 as ADR 0047's board restricted to the card's chips, L3 as today's dies), §6, §7, §8's retirements, §10 | Tests 1–4, 8, 9. Measured: the Wiring level's crossings and length for the studio and `library-demo`. **The owner zooms the studio from Panel to Wiring, into MONO SYNTH and into `moog_voice`, and back, and gives his verdict** on the fades and the morph, confirming or changing §13's defaults |
| 2. The Control level | §11; one cube per card with its connector and legs, and the card's engine die (§4, L3; `[top]` takes its new meaning); editing on L1 and L2 (§9) | Tests 5–7, 9. **The owner's verdict** on a project where an LFO drives the synth's cutoff: the moving knob, the inner readout, the automation wire, and patching it on L2 |
| 3. Logos, interfaces, depth | §5 (logos, `owns`, `logo`, the check); §3's grouped interfaces, slot interface names and device pins; recursion polish (an inner slot with a part repeats L0 → L2 → L3 in its footprint; breadcrumbs by level) | Tests 1–2 (phase 3 parts), 9. **The owner's verdict** on the studio's and `two-decks`' Wiring levels and logos |

## Open technical points (the operator's, to settle in phase 1)
- The band's factors (1.5 *k₀*, 2 *k₀*), the morph's easing and the cables' late fade are tuned at the operator's visual check, against the owner's verdict.
- A card with many instances and many controls (RHYTHM COMPOSER) gets one cube with one connector of about 26 pins. If phase 2's measures show the legs unreadable, the connector splits per group of the card (the plate's `.signal-group`), still one cube.
- Rule 5's "no majority" is judged after rules 3–4 have settled. A cycle between two infrastructure candidates is broken by source order.
