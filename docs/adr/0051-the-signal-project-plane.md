# ADR 0051: The Signal Project plane: Setup, Stack, Wiring, Control, Engine

- **Status:** Accepted (2026-11-01; the owner accepted it and answered the open questions the same day). Proposed 2026-11-01. The owner's vision is quoted in the Context. Agreed with the owner: the levels, the peripherals, the Stack and the studio example. Marked *(operator)*: the operator's decisions, not objected to by the owner, and accepted with the rest. **Replaces ADR 0050** (proposed, never accepted, now withdrawn).
- **Builds on:** ADR 0009 (text is the truth), 0022 (output adapters), 0025 (`.signal` files), 0032 (windows), 0035 (a project as content, `lib.mixer`), 0037 and 0045 (slots, interfaces), **0038** (devices: roles and layouts, never hardware), **0046** (a slot's part), **0047** (plate, routing, packages, dies, deep zoom).
- **Supersedes part of ADR 0047:** §1's three layers, §5's Plate / X-ray / Board switch and the single top-level board. **Amends:** ADR 0026 §2 (one view replaces the Panel and Schematic pages), 0032 (window kinds), 0038 §10 (its Devices level becomes the Setup level), 0046 §8 and its decided question 5 (grouping several instances), and vision §3a.
- **Date:** 2026-11-01

## Context
**The owner's vision (2026-11-01, translated from Hungarian):**
> One surface: the Signal Project — an infinite virtual plane. Within it the zoom levels: Setup, Stack, Wiring, Control, Engine, Layer 1, Layer 2 … Setup is the top view, and it includes the peripherals too: sound cards, MIDI controllers, Art-Net nodes, HDMI displays, DMX universes. Inside the Setup is the primary Stack; in this project it holds the instrument cards Transport, Mono Synth, Rhythm Composer, Tape Echo, Master. The SSL 2+ could appear in the Setup view, with its four audio outputs, which are inputs for us in its virtual representation. Hardware peripherals must be connectable too. So a Stack also has a connection interface, and the primary Stack's stereo output can go to the "SSL 2 stereo audio out". The Stack view can be brought to the foreground; it then roughly fills the screen and can be scrolled. An instrument is an instrument (a "műszer"). Instruments are in a Stack, so they have equal width. A Signal Project can have several Stacks. The Signal Project's plane is the whole setup, which can contain the virtual representation of the physical peripherals.
>
> The Instrument has a front panel with UI elements on it (Knob, Fader, Button …). The wiring of every such UI element is the Control plane, the sheet under the panel. As in the physical world, the controls sit on a PCB and are wired there. The Control view is this. For example, to connect a MIDI controller to the Mono Synth's cutoff knob, you connect the MIDI controller's MIDI cable to the cutoff (and, as in reality, address it). So the Control view is essentially the cabling of control.
>
> I thought there could be a mixer, because there is none in this stack now. Deck, Stack, Rack are roughly the same. Not "Rack", because physical racks exist and this needs a unique term. "Deck" was a variant, but "Stack" fits better. A Stack is a collection of equal-width instruments. It is primarily a visual grouping, and also a logical one, since it has a boundary. It should be possible to import and export Stacks, and Instruments too.

**Why ADR 0050 is withdrawn.** It started at the panel (one project, one plate, derived cards). The vision puts the machine and several Stacks above the panel and makes instruments exportable units. Its Control level and per-level renderings carry over; its morph, `owns` and logos do not.

**What exists (fact, main dca6e2a):** a Panel page (ADR 0047's plate) and a Schematic page (plate over one board, a layer switch). The studio's plate is five full-width cards stacked vertically, already a rack (title, MONO SYNTH, RHYTHM COMPOSER with the tempo knob, TAPE ECHO, master). ADR 0046 phase 1 is built. ADR 0038 is accepted, not built (M4.1): one `std.audio_out`, and the Setup's `[output]` and `output_channels` (its short form of `[play.main]`). On 0.3 the owner said: no release now; "not bad but not good enough, a bit of a dead end; we step back to step forward".

## Decision

### 1. One plane, levels by zoom
A project is drawn as one infinite **plane**, `<signal-plane>`. The zoom and the focused Stack decide the level:

| Level | Shows | Cables |
|---|---|---|
| **Setup** | every Stack as its column of front panels, in miniature; this machine's peripherals; role endpoints | Stack ↔ peripheral, Stack ↔ Stack |
| **Stack** | one Stack, its width filling the stage, scrolled vertically: live front panels | none: a driven control shows its moving arc |
| **Wiring** | the same Stack from behind: each instrument a box in its own rectangle, with its interface pins and the Stack's edge pins | between this Stack's instruments and to its edge |
| **Control** | one instrument's control plane: its controls where the panel has them, its engine as one package | binding legs, automation, external control cables |
| **Engine** | the package's die: the instrument's DSP graph | inner wiring |
| **Layer 1, 2 …** | composites and slots inside, recursively (ADR 0047 §8) | inner wiring |

Each level is a separate rendering (own SVG group, layout and cache), as in ADR 0050. Below Engine is unnamed depth; breadcrumbs name the instances.

### 2. Peripherals
- **One box per device of this machine** (one box even if it plays and records). Phase 1: audio devices from the inventory (ADR 0038 §10).
- **Port labels use the device's own names, and the drawing follows our signal direction** (agreed). The SSL 2+'s "Out 1–2" and "Out 3–4" are **inputs** on its box (on its left edge), because we send to them. Its "In 1" and "In 2" are outputs. The names come from CoreAudio where it reports them.
- **Role endpoints** are the project's device slots (ADR 0038 §3; `std.audio_out` is `main`, stereo). Each is drawn on the channels that its role is bound to ("main · stereo" on SSL 2+ Out 1–2).
- **Later kinds** (ADR 0038 §11), same rule: a MIDI controller's outs as event outputs, an Art-Net node or DMX universe as vector inputs (M6), an HDMI display as a visual input (visual output).

### 3. Portability: what the project stores, what the machine stores *(operator)*
ADR 0038's portability is kept: **a project names roles and layouts ("main stereo out"), never hardware.** The Setup level is where the project meets this machine, showing the devices its Setup binds the roles to; on another machine, that machine's devices stand in the same place.

| Where | What | Travels |
|---|---|---|
| `project.toml` | role endpoints (device slots), and the connections into them | yes |
| `ui/plane.toml` (§4) | the Stacks, their instruments in order, their places | yes |
| the Setup | role → device and channels, for every project (`[play.<role>]`, `[capture.<role>]`; today `[output]`, `output_channels`) | never |
| `devices.local.toml` | this project's routing overrides on this machine (ADR 0038 §6), and where its peripherals stand on the plane (`[plane."<device id>"] at`) | never |

- **A Setup-level cable is two texts drawn as one:** the project's connection `master.l → out.l` and the binding `[play.main] channels = [1, 2]`. Dragging its device end to Out 3–4 rewrites only the binding, for "this project" or "all projects" (ADR 0038 §10).
- **Peripheral places** are derived, deterministically: devices that receive from Stacks stand to their right, devices that only send stand to their left, in inventory order. Dragging a peripheral writes its place into `devices.local.toml`.
- **No device, or an unknown one:**
  - **bound but absent:** a hollow box with the Setup's name, "not present", dashed cables; what is heard follows ADR 0038 §4, §10;
  - **a role not bound here** (`click`): a hollow endpoint beside the Stack, "not heard on this machine", with "Play on …";
  - **no audio device, or headless:** role endpoints only (renders never read the Setup);
  - **unusable** ("other clock", ADR 0038 §8): greyed with the reason. Kinds not built yet are not drawn.

### 4. The Stack
- **A Stack is a vertical column of instruments of equal width,** like a 19-inch rack. A project has one or more Stacks. The first is the **primary** Stack.
- **It is a grouping in project text, not a graph level** *(operator)*: its instruments are top-level instances, so no address contains the Stack's name, and moving an instrument between Stacks changes no plan and no golden.
- **`ui/plane.toml`:**

      schema_version = 1
      [[stack]]
      name = "primary"             # [a-z][a-z0-9_]*, unique
      at = [0, 0]                  # top-left on the plane, in pu; absent: laid out
      width = 160                  # pu (ADR 0047); every front panel is this wide
      instruments = ["card:transport", "card:mono-synth", "card:rhythm-composer", "card:tape-echo", "card:master"]

  - An entry names a top-level slot with a part (`"mono_synth"`, the full form, §5) or a card of `ui/main.html` (`"card:<id>"`, the migration form).
  - `plane.check`: an unknown name or an instrument listed twice is an error; a top-level instance in no instrument is a warning, drawn in an "unstacked" tray at the Setup level.
- **Without the file,** one primary Stack is derived from the panel's cards in plate order, at the plate's width.
- **Equal width:** each front panel is laid out at the Stack's width; a narrower one is centred between blank side plates, a wider one scaled down whole with a warning. Heights vary, like rack units. A fixed 2 pu rail separates instruments at every level, so nothing moves between Stack and Wiring.
- **The boundary:** a Stack's interface is **derived, never written**. It has one edge pin per instrument port that a connection joins to something outside the Stack (another Stack, a role endpoint, an unstacked instance). Inputs are on the left, outputs on the right, and the pins are grouped per instrument (`master l r`).

### 5. The Instrument *(operator)*
An instrument is **a top-level slot with a part**. ADR 0037's and 0046's model fits import and export, and it has a boundary of its own:

| The owner's word | Existing concept |
|---|---|
| Instrument | a top-level slot (ADR 0037) |
| its connection interface | the slot's interface (ADR 0037, 0045) |
| Front panel | the slot's part, a small plate (ADR 0046, 0047 §4) |
| Engine | the slot's content (`load`, or its own inside) |
| Export / import | "Save slot as project…" (ADR 0046 §9); `vendor --slot` or `load` (ADR 0035 §11) |

**Built and to be built** (amended 2026-11-01 after the review: the table and the list below cite concepts as if they all existed; they do not):

| Concept cited | State | Milestone |
|---|---|---|
| A top-level slot, its content (`load`, its own inside) | built (ADR 0037 phases 1–3) | M3.2 |
| The slot's interface | built (ADR 0037; ADR 0045 phase 1, the format and checks; its editing is phases 2–4) | M3.2; editing M3.4 |
| The slot's part, a small plate | built (ADR 0046 phase 1; ADR 0047 §4) | M3.2 |
| The migration form (a card as an instrument, ADR 0050 §2's assignment, blank panels) | built (`ui/lib/plane-instruments.js`) | M0.4 phase 1 |
| `slot.group` of **one** instance (Group into slot) | built (ADR 0046 phase 1) | M3.2 |
| `slot.group` of **several** instances (Make instrument) | to be built; needs a written, round-trippable form of the grouping and of the inherited seed first | M0.4 phase 3 |
| A scope root inheriting the run's seed (ADR 0035 §4, extended) | to be built: `ScopeDef.seed` (`crates/signal-graph/src/def.rs`) is a mandatory `u64` today, and an inherited seed has no text form yet | M0.4 phase 3 |
| "Save slot as project…" (ADR 0046 §9) | to be built (ADR 0046 phase 3) | M3.4 |
| `vendor --slot` (ADR 0037 phase 4) | to be built; `signal vendor` copies library composites only | M3.3 |
| The role endpoint's card for `out.gain` and the output meters (ADR 0038 §10) | to be built; the plane draws the endpoint as a pill on its device's channels | M0.4 phase 3 |

So phase 3's order is: the inherited seed's text form and several-instance grouping (Make instrument), then export through M3.4's "Save slot as project…" and import through M3.3's `vendor --slot`; `load` (Link) is there now.

- **The migration form:** a panel card listed in a Stack is drawn as an instrument. The card is its front panel. Its engine is the instances its controls bind, assigned by ADR 0050 §2's rules (a leftover becomes an instrument with a generated panel). Its interface is derived from the connections crossing it. A card with no instances (the studio's title) is a **blank panel**. It cannot be exported until it is made an instrument.
- **Make instrument** turns a card into a slot instrument. It is ADR 0046's `slot.group` **extended to several instances**, which answers ADR 0046's question 5:
  - the instances move into the slot's own inside (`/kick` becomes `/rhythm/kick`), and the card becomes the part;
  - **seeds kept:** the inside is a scope root inheriting the run's seed (ADR 0035 §4, extended, domain-free), so `/rhythm/kick/noise` draws `/kick/noise`'s stream;
  - **not byte-identical:** addresses and the log change, and a plain fan-in across instruments may reorder; the audio is compared;
  - controls binding outside the instrument stay out of the part: the studio's `out.gain` and output meters move to the role endpoint's card (ADR 0038 §10).

### 6. Front panel and Control plane
- **The front panel** is today's panel card or a slot's part, laid out as ADR 0047 §2 lays out the plate. Edit layout works on it at the Stack level.
- **The Control plane** is ADR 0050's L2 Control, inside the instrument's rectangle: the controls at their panel positions; the engine as one package near them (ADR 0047 §5); binding legs as ribbons into its connector (§8b); **automation** (connections into bound parameters, into the control's input); and **external control cables** landing on a control from outside. Landing one on a parameter outside the interface adds it to the interface in the same patch.
- **ADR 0050 §11 is carried:** the pointer is the setting, a moving arc the effective value (on the front panel too), and a readout the inner value.
- **Addressing** (a MIDI CC on the cable, the owner's "address it") comes with MIDI in M6, as a badge where the cable lands.

### 7. Cables that cross levels
A cable is drawn **as a port on each boundary it crosses**, as ADR 0047's bond wires join a package to its die. A MIDI cable from a peripheral to the Mono Synth's cutoff is drawn like this:
- **Setup:** from the peripheral to a pin on the primary Stack's edge;
- **Wiring:** from that pin to a pin on MONO SYNTH's edge;
- **Control:** from that pin to the cutoff knob;
- **Stack:** only the knob's moving arc.

The pins derive from the one connection in `project.toml`; nothing is stored. Tracing (ADR 0030) highlights every level's part.

### 8. Zoom, focus and fades
- **Setup → Stack is focus.** A click on a Stack's header, Enter, or zooming until it fills the stage fits its width (ADR 0047 §2); the wheel then scrolls it vertically, and Setup cables and other Stacks fade to `SURROUND`.
- **Stack → Wiring is a cross-fade in place.** It happens in a zoom band past the Stack's fit (1.5× to 2×, tunable).
  - Each instrument keeps its rectangle, so no morph is needed.
  - Cables run in channels on both sides of the Stack and cross it in the rails (ADR 0047 §6's tracks).
- **Wiring → Control → Engine → Layers are nested footprints** under ADR 0047's semantic zoom. The instrument under the zoom point opens, and the others fade to `SURROUND`.
- One curve in and out, no timer. Zoom never changes the selection. Escape goes out a level, Enter goes in. The Plate / X-ray / Board switch is removed.

### 9. Editing, and what is text
| Level | Edits | Written by |
|---|---|---|
| Setup | Stack places, new Stacks, cables between Stacks or to role endpoints, peripheral places | `plane.write`; `patch`; the role binding (§3) |
| Stack | operating controls, Edit layout, instrument order and moves, Make instrument, import | `commit`; `panel.write`; `plane.write`; `patch` |
| Wiring, Control | patch cables, binds, automation, external control cables | `patch`; `panel.write` |
| Engine and below | inner wiring, positions | `patch` at the level; `schematic.move` |

- **Protocol additions:** `plane.read` and `plane.write` (checked as `panel.write` is), the MCP tool `write_plane`, and ADR 0038's `devices` message. *Amended 2026-11-01: none of the four is built.* Phases 1 and 2 need no plane writer: the plane reads `ui/plane.toml` as a served file (`/project/plane.toml`), its edits are `patch`, `panel.write`, `device.route` and `device.unroute`, and only `slot.group` (and its undo, `panel.restore`) writes `ui/plane.toml`. Stack places, new Stacks and instrument order (the Setup and Stack rows' `plane.write`) wait for phase 3, with the writer; the Setup's changes reach every window by the `setup` push (`ui-protocol.md`), not by a `devices` message.
- **View state** (focused Stack, zoom, scroll) is per window in `ui/windows.toml`; all else derives from §3's files. *Amended 2026-11-01:* **built:** the focused Stack, the opened instrument and the zoom are not kept (a window opens on the Setup level, the whole plane fitted); the one piece of view state kept is the expansion of "Other devices", per viewer in the browser's `localStorage` (`signal.plane.otherDevices`, as the schematic's sidebar collapse), never in the project's files. **Planned:** focused Stack, zoom and scroll per window in `ui/windows.toml` (ADR 0032), with phase 3 (M0.4); `localStorage` stays for the per-viewer conveniences.

### 10. Import and export
**No new format: an exported Instrument or Stack is a project** (ADR 0025, 0035).
- **Export an Instrument:** ADR 0046 §9's "Save slot as project…". The content becomes its composites, the interface becomes `[exports]`, and the part becomes its `ui/main.html`.
- **Import an Instrument** into a Stack: a slot whose own inside is a copy of the project (`vendor --slot`) and whose part is its panel; "Link" writes `load = "<path>"`, read-only.
- **Export a Stack:** a project with its instruments as they are, the connections among them, its edge pins as `[exports]` and a one-Stack `ui/plane.toml`; role endpoints are left out.
- **Import a Stack:** its instruments are copied in, and a name that collides gets a suffix (`mono_synth_2`) with its connections rewritten. The edge pins stay unconnected, to be cabled at the Setup level.
- Each import is one patch and one undo.

### 11. Pages and windows
The Panel and Schematic tabs become one plane tab (Parameters and Setup stay); a schematic window (ADR 0032) becomes a plane window, as do `/schematic` and `/project/main.html`. A panel window shows chosen front panels and **never zooms past Stack**, so a performance surface cannot fall into Wiring.

### 12. Migration
| Today | Under this ADR |
|---|---|
| ADR 0047's plate (`plate`, `place`, Freeze, scaling) | **Kept as front panels.** The plate's cards become the primary Stack's instruments. The Stack sets each card's vertical position; `place` keeps its height and its x |
| ADR 0047's layer switch and its one board under the plate | **Retired** (`LAYERS` and the fades in `board-view.js`, the bar's buttons); the board splits into Wiring and Control |
| ADR 0047 §6–8b (routing, ribbons, lanes, connectors, packages, dies) | **Kept** at Wiring, Control, Engine and below |
| ADR 0046's parts, open slot card, faces, Group into slot | Parts are front panels; the open slot card is the instrument in its Stack; Group into slot becomes Make instrument |
| ADR 0038 §10's Devices level | the Setup level's peripherals |
| ADR 0050 | withdrawn; §1, §6 and §8 above say what is carried |
| the studio | Phase 1, `ui/` only: a TRANSPORT card takes the tempo knob (the owner's list); `ui/plane.toml` lists the cards. Phase 3: Make instrument per card moves the golden's log and manifest, **the owner's decision** |

Other projects keep a derived primary Stack; none stops opening.

### 13. The mixer: a planned content change
The owner suggested a **Mixer** instrument for the studio Stack, which has only `bus` (`std.mix`). It builds on **`lib.mixer`** (ADR 0035 §9, its phase 3, not built: four stereo channels with level, mute, balance, and a master level). **It changes `projects/studio`'s golden render** unless exactly transparent, which dB levels through `std.gain` make unlikely for the studio's 0.45 and 0.55 gains. **The owner decides when it is built.**

### 14. The hard rules
- **Rule 1:** Stacks, instruments, the plane and peripherals are project and UI concepts (`ui/`, `signal-ui`, and `signal-project` for Make instrument and import/export), never engine ones. The one graph addition is a scope root's inherited seed, which is domain-free. `domain_scope.rs` is unchanged.
- **Rules 2 and 3:** the plane changes no plan, render or log; rebinding a role uses ADR 0038 §5.7's path (a fixed matrix prepared on the control thread); every layout is deterministic in ADR 0047 §6's sense.
- **Rule 4:** §3, §9. **Rule 6:** measures are shown, never choose; every phase ends with the owner's verdict. **Rule 7:** no dependency.

### 15. Terminology
Vision §3a gains Setup level, Stack, Instrument (replacing the proposed role tag), Front panel, Control plane and Peripheral, marked accepted (2026-11-01). Plate and Board become drawings: the front panels, and the Wiring and Control drawings.

## Alternatives considered
- **ADR 0050:** no place for the machine, several Stacks or exportable units.
- **Device names in the project:** the literal reading, but it fails on other machines and leaks the author's studio (ADR 0038).
- **A Stack as an include or composite:** an include is read-only in its host; a composite moves every address and golden.
- **Instruments as presentation only** (ADR 0050's cards): not exportable; kept as the migration form.
- **Peripheral places in the Setup:** projects arrange planes differently.

## Consequences
- **`ui/`:** `<signal-plane>` joins `panel.js` and `schematic.js`, with pure, `node:test`-tested modules for the plane, Stacks, peripherals, derived interfaces and zoom. The Control plane is ADR 0047's board restricted to one instrument. The layer switch and the whole-level board go.
- **`signal-ui`:** `ui/plane.toml`, `devices`, `write_plane`. **`signal-project`:** Make instrument, import and export, peripheral places. **`signal-graph`:** the inherited seed. **`signal-app`:** one plane tab, plane windows.
- **Docs:** `ui-controls.md`, `project-format.md`, `ui-protocol.md`, `agent-interface.md`.
- **M4.1 only in part:** phase 1 needs the inventory and today's `[output]` (role `main`). Other roles and capture wait for ADR 0038's phases 1–2.
- **Lost:** one drawing of every top-level wire; tracing crosses levels instead.

## Tests
1. `ui/plane.toml`: parse, check, write; the derived default of every example project snapshotted; key order irrelevant.
2. Portability: §3's cases with a scripted SSL 2+, a laptop output, no device and an absent device; no project file written; plans and renders byte-identical.
3. Peripherals and Stacks: device names, our direction, equal widths, edge pins exactly where connections cross, deterministic layouts.
4. Zoom: band opacities monotonic, exactly 0 and 1 at its ends; no rectangle moves; selection unchanged; no layout per zoom frame.
5. Boundaries: one pin per boundary a connection crosses, meeting.
6. Edits send the expected messages; undo restores files byte for byte.
7. Make instrument: streams equal (oracle); audio compared and reported.
8. Import and export: an export imported back plans as the original, under its new address.
9. Performance, measured on the Mac: at most 5 000 drawn elements, 50 fps on the studio and `library-demo`.
10. Every project passes `signal ui check`; no golden changes through phase 2.

## Phases (toward 0.4)
| Phase | Delivers | Exit |
|---|---|---|
| 1. The plane | `<signal-plane>`; `ui/plane.toml`; Setup and Stack levels; focus and scroll; audio peripherals from the Setup; role endpoints and Setup cables; front panels in the Stack; zoom fades | Tests 1–4, 9, 10. **The owner opens the studio,** sees the SSL 2+ with the primary Stack cabled to Out 1–2, focuses and scrolls the Stack, and gives his verdict on the fades |
| 2. Wiring and Control | the Wiring level and its derived interfaces; the Control plane with ADR 0050 §11; external control cables (addressing comes later, with MIDI); editing | Tests 5, 6. **The owner's verdict** on the studio's Wiring and on an LFO and an external cable driving the cutoff |
| 3. Engine, import and export | Engine and Layers; Make instrument; import and export of Instrument and Stack; the old tabs retired | Tests 7, 8. **The owner** moves the Mono Synth and a Stack between projects and **decides** on the studio's golden |
| 4. The mixer | `lib.mixer` and a Mixer instrument in the studio | **The owner's listening verdict and golden decision** |

A new `MAJOR.MINOR` (0.4) is the owner's decision, proposed when phases 1–3 pass.

## Answered questions (2026-11-01)
1. **The tab's name** (owner): **Signal Project**.
2. **Stack width** (operator): 160 pu; no half-width instruments for now.
3. **Front panels at the Setup level** (operator): live.
4. **Wiring** (operator): the back of the rack only; a flow arrangement may come later.
5. **Logos** (operator): an identifier only, not a way into the engine.
6. **The mixer** (owner): it replaces `bus`, with four channels: synth, drums, echo return and a spare. The golden decision is still taken when it is built (phase 4).
7. **Make instrument on the studio:** still open; decided at phase 3.

## Amendment (2026-11-01): machine-local state, and test 9 measured *(operator)*
Operator decisions on the owner's delegation, after the review of 2026-11-01 (`adr-0051-prerequisites-and-local-state`, local-state half; `plane-performance-budget`). The rest of this ADR stands.
- **Machine-local state of a `.signal` project** (§3's `devices.local.toml` row): a `.signal` project's working folder is removed on close and the file is never packed, so its routing and peripheral places would be lost. They live outside the project, in `Local/<id>/devices.local.toml` beside the Setup, keyed by a project id that Signal.app's saves write in the archive's comment (never an entry, so no plan, render or golden changes); reopen, move and Save As keep or copy them, copies share one state until Save As. Folder projects keep the file in their folder. The decision and its rules are ADR 0038 §6's amendment of 2026-11-01. §3's table reads: `devices.local.toml` (a folder project's, in its folder; a `.signal` project's, beside the Setup), travels never.
- **Test 9, measured** (`docs/benchmarks.md` "The Signal Project plane"; headless Chrome on the M-series Mac, not WKWebView): the studio, `library-demo` and a three-Stack studio run at 58.5–60 fps through Setup, Stack, Wiring, Control and Engine, with at most 2 524 drawn elements. Two costs were found and changed: every front-panel control's 30 Hz `parameters` stream repeated its last frame about 93 % of the time (now sent on change, with a 1 s keep-alive), and controls off the stage (cards scrolled away, the hidden Engine board's plate) kept streaming (they now pause off the stage). `<signal-plane>` gained a dev-only `metrics()` counter. The owner's verdict in Signal.app is still the exit condition: phases 1 and 2 stay "built, owner verdict pending".

## Amendment (2026-11-02): the owner's interactive session *(owner)*
The owner's decisions while trying the plane in Signal.app (`./start.sh --dev`). They change §8's navigation and §11's names; the levels themselves stand.
- **Names.** The window's tabs are **Deck** (the plane), Parameters and **Config** (the Setup tab); the File menu's Interface Setup… is **Interfaces Config…**. The plane's top level is called **Project** in the bar (the "Setup level" of this ADR).
- **The bar is the navigation.** The level bar sits at the toolbar's right end (docked, `dockBar`), visible on the Deck tab. Every layer has a button, always shown: Project · Stack · Wiring · Control · Engine; then the zoom (− ratio +); then **one Edit switch** that edits the shown layer's arrangement (built at the Stack: Edit layout of `ui/main.html`, in place; the other layers' Edit is still to be built). No second toolbar and no Done.
- **Layer and zoom are independent** (trial, the default): a layer button alone chooses what is drawn, each drawing wholly in or out; the zoom only scales, opening and focusing nothing. Control and the Engine open the instrument in the middle of the stage (else the one opened); the Engine lays that instrument's board to fill the board's stage. Enter and Escape step through the layers. §8's coupled zoom (the levels as fades of one zoom) is kept behind localStorage `signal.plane.coupled` = "1".
- **The pointer, the same on every layer.** A mouse's wheel zooms about the pointer, over a knob or a fader too; Shift with it pans up and down; a trackpad's two fingers pan and a pinch zooms; the right or the middle button held pans at once, and a right click that does not move opens the context menu on its release. The left button is always the element's own action, never a pan. The Engine's board and the Edit layout editor keep their own.
- **Knobs and faders.** A click without a move grabs the control: it follows the pointer with no button held until the next click or Escape (`stickyDrag`); the double-click and the held drag are as before.
- **Setup level.** Every device has a box of its own (the "Other devices" list is gone), the ones in use first. The main window opens maximized.
