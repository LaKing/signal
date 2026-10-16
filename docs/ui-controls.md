# The control library

The web-component controls of ADR 0023 §3 (M2.8-2). They are in the repository's `ui/` folder as plain ES modules, with no dependencies and no build step. `signal ui` serves them at `/ui/` (`docs/ui-protocol.md`).

| File | Holds |
|---|---|
| `ui/signal-ui.js` | The protocol client: `connect`, `connectFromPage`, `Connection` (describe, subscribe, set, ramp, trigger, commit, retry, revert, reconnect; the host's unsaved state as `connection.unsaved` and the `unsaved` event), the default connection the controls use |
| `ui/controls/index.js` | Defines every element below and `<signal-panel>` (import it once) |
| `ui/controls/panel.js` | `<signal-panel>`, the loader of a project panel ("Panels") |
| `ui/controls/schematic.js` | `<signal-schematic>`, the project's signal path as an infinite-zoom SVG schematic ("Schematic") |
| `ui/schematic.html` | The schematic's page in `signal ui` (`/schematic`) |
| `ui/default-panel.html` | The generated panel of a project without `ui/main.html` |
| `ui/controls/theme.css` | The theme's custom properties and their dark defaults |
| `ui/lib/` | Pure logic with `node --test` tests: `taper.js`, `gesture.js`, `ballistics.js`, `trigger.js`, `skin.js`, `format.js`, `panel.js` (the default panel, diagnostics); `entry.js` (typed values, M2.9-4; its field is `ui/controls/entry-field.js`); `protocol.js` (M2.8-1); the schematic's `schematic-view.js`, `schematic-layout.js`, `schematic-scene.js`, `schematic-hit.js`, `schematic-inspector.js`, `schematic-levels.js` (M2.9-1) |
| `ui/fake-connection.js` | A connection without an engine, for the gallery |
| `ui/gallery.html` | Every control in every mode |
| `ui/assets/` | The gallery's skins; `scripts/ui-strip.mjs` draws its PNGs (`pulse`: the pulse panel's strip) |

## Using them

```html
<link rel="stylesheet" href="/ui/controls/theme.css">
<signal-knob bind="/drums.kick_decay" taper="log" label="decay"></signal-knob>
<script type="module">
  import '/ui/controls/index.js';
  import { connectFromPage, setDefaultConnection } from '/ui/signal-ui.js';
  setDefaultConnection(connectFromPage());
</script>
```

A control uses the default connection (`setDefaultConnection`), or the one given to `element.useConnection(connection)`. From the connection's `describe` it takes the parameter's range, default, unit and base value; the attributes `min`, `max`, `default`, `unit` override them, and without a connection or a `bind` the control works on those attributes alone.

**Edits.** A drag sends a `ramp` (5 ms) per move and a `commit` on release. Keys and the wheel send a live `set` per step and one `commit` 500 ms after the last one; a pending commit is also sent when the control loses focus, leaves the page, or the page is hidden or closed (`pagehide`, `visibilitychange`). A reset (double-click, Delete) commits at once. A typed value ("Value entry") is sent as a release is: a 5 ms `ramp`, then a `commit`. Live edits (`set`, `ramp`) the server refuses are not reported in the console; the next parameter frame shows the value the runner has. **A commit is never dropped silently** (`Binding` in `ui/lib/binding.js`): its refusal is shown on the control ("Unsaved", below). Base and effective values come from a `parameters` subscription (30 per second).

**Unsaved.** The host knows which parameters' live values the file does not hold (`docs/ui-protocol.md` "Unsaved"), and every client shows what it says, so a second window or a reloaded page shows the same. A control whose write failed — on the host (`write-failed` from a read-only folder, for example), or a commit lost with its connection — gets the **unsaved mark**: the `unsaved` attribute (a dashed amber outline, `--signal-unsaved`), the error text (`role="alert"`) and two buttons, **Retry** (`retry`: write the live value again) and **Revert** (`revert`: the live value back to the file's). The mark stays until a write or revert is confirmed or the host no longer lists the parameter. An edit that is only in progress (a drag, a key step before its commit) is listed by the host but gets no mark: only a refused write does. A failed reset of a key the file does not set leaves nothing unsaved (the ports and the file both use the default), so its error mark clears on the next host push. `<signal-value>` writes only a typed value, and shows the mark like the others. The project-level indicator, **"N unsaved parameters — last error"** with **Retry all** and **Revert all**, is in `<signal-panel>`'s bar (so in the default panel and in every project panel), in Signal.app's window (above its messages) and on the raw protocol test page at `/` (`ui/index.html`, below its status line; M2.8-9); it counts every listed parameter, edits in progress included, and is not shown when there are none (its space is kept: "Stable layout"). Signal.app's parameter list shows each row's error with its own Retry and Revert, and Save (Signal.app's File menu, or Cmd-S on its page in a browser) retries every unsaved parameter and says "saved" or what was not, then writes a `.signal` project back to its file (M2.8-12).

**The client.** `Connection` says `hello`, asks `describe` when welcomed, and keeps its subscriptions across reconnects (backoff 0.25, 0.5, 1, 2, then 5 s; the new `sub` ids are mapped). A request that is outstanding when the connection drops, or made while it is down, is rejected with `code: 'closed'` and never sent again: a repeated `commit` or `trigger` would act twice. After close code 1008 (unauthorized, for example a server restarted with a new token) or 1002 (protocol) it stops, with status `failed`. A listener that throws is reported (`reportError`) and the others still get the event (M2.8-11: a throwing meter once kept Signal.app's parameter list from every `describe` while playing). While Signal.app is switched off (`power: "off"` in `welcome` or `app`, `docs/ui-protocol.md` "Signal.app") it sends no subscription and lets go of those it has; switched on, it sends them again.

## Elements

Common to all:
- `bind`: the port address (a control or plan parameter port; an event input for `<signal-button>`; signal outputs or inputs for `<signal-scope>`). In a project panel, the controls that edit (fader, knob, toggle, 2D pad) bind controls, which a release commits ("Panels").
- `skin` (below) and `label`.
- Focusable (`tabindex="0"`), a focus ring (`--signal-focus`) on keyboard focus, an ARIA role, pointer events with pointer capture (mouse, pen and touch; `touch-action: none`).

| Element | Attributes | Pointer | ARIA |
|---|---|---|---|
| `<signal-fader>` | `orientation` (`vertical`, `horizontal`), `taper` (`linear`, `log`, `db`), `floor` (dB taper, default −60), `min`, `max`, `default`, `unit` | drag (Shift: fine, ×0.1); double-click resets; wheel | `slider`, `aria-valuemin/max/now/text`, `aria-orientation` |
| `<signal-knob>` | `sweep` (degrees, 10–350, default 270), `bipolar` (the arc starts at 0, or the middle), `drag` (`vertical` default, `circular`), `taper` … | vertical or circular drag (Shift: fine; in a circular drag the knob then moves a tenth of the pointer's angular movement, however many events report it); wheel; double-click resets | `slider` |
| `<signal-xy>` | `bind="/x.address /y.address"`, `spring` | press and drag place the puck (Shift: fine, relative); release springs back to the centre with `spring`; double-click resets both | `group`; two inner `slider`s (X, Y, `tabindex="-1"`) carry the values |
| `<signal-button>` | `bind` (an event input) | press sends one `trigger` (momentary: lit while held) | `button`, `aria-pressed` |
| `<signal-toggle>` | `min`, `max` (off and on) | click flips and commits | `switch`, `aria-checked` |
| `<signal-meter>` | `bind`: a signal input of the audio sink (`/out.l`), which is the output meter of the same channel; `source` (`output`, `input`) and `channel` override it (the Setup's input has no port: `source="input" channel="0"`); `floor` (scale bottom, −96 to −24 dB, default −60) | click clears the clip | `meter`; `aria-valuenow` is the held peak in dB, the floor for silence; `aria-valuetext` says peak, RMS and clip |
| `<signal-scope>` | `bind` (one port; two for X–Y), `mode` (`time`, `xy`; two ports default to `xy`), `rate` (points per second, default 12000), `trigger-level` (default 0), `trigger-edge` (`rising`, `falling`) | — | `group` with `aria-label`, `aria-description`, `aria-keyshortcuts` |
| `<signal-value>` | `bind`, `unit`, `digits` | double-click types a value | `status`; shows the effective value, else the base |
| `<signal-canvas>` | `bind` (1 to 3 vector outputs, in the renderer's order), `renderer` (`points`, `mesh`, `bodies`), `rate` (snapshots per second, default 60), `grid` (`mesh`: nodes per side, default the square root), `tilt`, `height` (`mesh` view, default 0.5), `skin`, `backend` (`auto`, `2d`, `webgl`) | — | `img` with `aria-label`, `aria-description`; `data-backend` and `.backend` say what draws |

The fader and knob show the effective value when it differs from the base (a modulated parameter): an amber mark on the fader, a dot on the knob's arc, and `→ value` on the readout's second line (there, empty, when not modulated) and in `aria-valuetext`.

The meter uses the M2.5-1 ballistics (`crates/signal-app/src/meter.rs`) ported line for line to `ui/lib/ballistics.js`: a 1.5 s peak hold, a 300 ms RMS, and a clip indicator latched at a peak of 1.0 until cleared. A protocol meter frame carries each channel's peak and RMS over `extra` frames, so its block's sum of squares is `rms² · frames`.

The canvas is described under "Canvas" below.

The scope shows the subscribed points from the first crossing of the trigger level on the trigger edge that leaves a full display after it (half of the last two consecutive blocks, at most 256 points), else from the start (free running). X–Y pairs the points of the latest block of each port that fall on the same frames.

### Keys

Shift makes every step fine (×0.1).

| Element | Arrows | Page Up / Page Down | Home / End | Other |
|---|---|---|---|---|
| Fader, knob | Up, Right +1 %; Down, Left −1 % of the travel | ±10 % | min / max | Delete, Backspace: default; Enter: type a value |
| 2D pad | Left/Right move X, Up/Down move Y, 1 % | Y ±10 %; with Alt, X | both to min / both to max | Delete: defaults; Escape: centre (with `spring`); Enter: type X (Tab: then Y) |
| Button | — | — | — | Space, Enter: trigger (held; repeats ignored) |
| Toggle | Right, Up on; Left, Down off | on / off | off / on | Space, Enter flip |
| Meter | the scale's floor ±6 dB | ±24 dB | floor −96 dB / −60 dB | Enter, Space, Delete clear the clip |
| Scope | trigger level ±0.01 (Shift ±0.001) | ±0.1 | level 0 / the last block's middle | E: edge; Space: freeze |
| Value | one decimal more or less | — | fewest / most decimals | Enter: type a value |

The meter, scope and value send nothing to the engine with these keys; they change what they show (and the meter's clip latch). The travel is the control's taper ("Tapers"): a step of 1 % is 1 % of the position, so on a `log` taper it is a constant ratio and on a `db` taper a constant number of dB. Each step is a live `set`; the file is written once, as one release, 500 ms after the last key.

## Value entry

A value can be typed (M2.9-4) on `<signal-fader>`, `<signal-knob>`, `<signal-value>` and `<signal-xy>` (x and y are two fields, side by side in its readout; a press on the readout does not move the puck; Tab from x commits it and opens y), in the base cell of every editable row of Signal.app's Parameters tab, and on the base readout of the editable rows of the schematic's inspector card. `<signal-button>` and `<signal-toggle>` take nothing to type. The pure part is `ui/lib/entry.js` (`parseValue`, `entryText`, `clampEntry`, the field's states `EntryField`), the field `ui/controls/entry-field.js`.

- **Opening.** Double-click the readout (on a knob or fader, its value text; elsewhere on the control a double-click still resets), or press Enter while the control (in the Parameters tab and the inspector: the slider) has the keyboard focus. A text field covers exactly the readout's box (`position: absolute` over it, out of the flow): nothing else moves or resizes ("Stable layout"). It holds the base value in full precision, the shortest text that reads back to the same number, with the parameter's unit (`440 Hz`, `0.3333333333333333`), selected.
- **Committing.** Enter or Tab commits: the value is sent live as a drag's release is (a 5 ms `ramp`), then written to the file with `commit`, so the unsaved mark, Retry, Revert and Reset work as for a drag. Enter or Tab on an unchanged text cancels (nothing is sent or written, so opening a field only to read the full value never writes the file; a held Enter's repeats do nothing). Escape cancels; a click elsewhere cancels when the text was not changed and commits when it was. Cmd or Ctrl shortcuts (Save, Open, …) reach the page: a changed text is committed first.
- **Out of range.** The value is clamped to the range and sent; however it was committed (Enter, Tab or a click elsewhere), the field shows the clamped value, read-only, for 2 s (Enter or Escape on it closes it sooner), and the reason (`Clamped to the maximum, 20.00 kHz`) is shown for 2 s in the control's message place: on a panel control the box of the unsaved mark (over what is below it, without Retry and Revert), in the Parameters tab the row's `not in file` cell (cut, the whole reason as its tooltip), in the schematic its message line.
- **Invalid text** keeps the field open while it has the focus, its border red (`aria-invalid`), with the reason as its tooltip; nothing is sent. Editing the text clears the mark; a click elsewhere lets it go (the field never stays open without the focus).
- **Off.** A control inside an element with the `no-entry` attribute (or with it itself) takes no typed value: a double-click on its readout resets, as elsewhere, and Enter does nothing. The panel editor (M2.9-2) sets it in edit mode. A `<signal-value>` bound to a plan parameter that is not a control takes none either (`commit` would refuse it).

**Accepted text** (`parseValue(text, {unit, range, taper})`, returning `{value}` or `{error, reason}`):
- A number: an optional `+` or `-` (or the minus sign `−`, U+2212, as Signal prints it), digits with a decimal dot or comma (`0,5`; there is no thousands separator, so `1,000` is 1), an optional exponent (`1e3`, `2.5E-2`).
- After it, case-insensitively, a form of the parameter's unit: `Hz` also as `kHz`; `ms` also as `s` (×1000), `s` also as `ms`; any other unit (`dB`, `st`, `ct`, `bpm`, …) as itself only, so `st` and `ct` only where the unit is that. `dB` on a linear gain, a parameter without a unit whose plan `kind` in `describe` is `gain` (a control's: its first plan port's), or one shown on a `db` taper, whatever the control: `-6 dB` is 10^(−6/20). `%` where the range is within 0..1 (`50%` is 0.5).
- Or an SI prefix, case-sensitive, `k` (×1000), `m` (×0.001) or `M` (×10⁶), alone (in the parameter's unit: `1.2k`, `500m`) or before a unit form (`1 MHz`). A unit form is read first, so `1ms` and `1MS` are a millisecond, `1M` a million, `1m` a thousandth; `K` is no prefix, and no prefix goes before `%` or `dB`.
- `-inf` or `-∞` (with or without `dB`) is the minimum of a dB taper, of a parameter in dB, and of a linear gain whose range starts at 0.
- Refused, each with its reason: nothing typed (`empty`), no number (`not-a-number`), a unit or anything else after the number that does not fit the parameter (`unit`: `"ms" does not fit a parameter in Hz`), and a value that is not finite (`not-finite`: `1e400`, `inf`, `-inf` where it is none of those).

## Tapers

A position is 0..1 along the travel (`ui/lib/taper.js`):
- `linear`: proportional.
- `log`: geometric from `min` to `max`; needs `0 < min < max`, otherwise linear.
- `db`: positions above 0 are linear in dB from the floor (`floor`, default −60 dB, or `min` in dB if higher) to `max` in dB; position 0 is `min`. Values at or below the floor's value map to position 0 and back to `min`, so value → position → value round-trips only above the floor.

## Skins

`skin="svg:<url>"` uses an SVG whose named parts (`data-part="…"` or `id`) the control moves:

| Part | Moved by | How |
|---|---|---|
| `pointer` | knob | rotated by the knob's angle (0 at 12 o'clock) about `data-cx`, `data-cy` (default: the view box's centre); draw it pointing up |
| `thumb` | fader | moved `data-travel` units: up from the bottom (`data-axis="y"`, default; draw it at the top) or right (`data-axis="x"`; draw it at the left) |
| `puck` | 2D pad | moved `data-travel-x` right by X and `data-travel-y` up from the bottom by Y |
| `on`, `off` | button, toggle | shown when on, off |
| `value` | fader, knob | its text is the formatted value |

Before it is used the SVG is sanitized (`ui/lib/skin.js`, conservatively: what it cannot read as local is dropped):
- `script`, `foreignObject`, `iframe`, `object`, `embed`, `audio`, `video` and the animation elements are removed;
- every `on…` attribute and `xml:base` are removed;
- `href`, `xlink:href` and `src` stay only when they are a fragment (`#id`) or a path on the same host: a reference with a control character (a URL parser drops tabs and newlines, so `java⇥script:` is `javascript:`), a backslash, a scheme or a host is removed;
- `<style>`, `style` and any other attribute holding `url(` (`fill`, `filter`, `mask`, `clip-path` …): comments go, CSS with an escape or control character (`u\72l(` spells `url(`) or with `image-set(`/`src(` is dropped whole, `@import` rules go, and `url()`s other than same-host ones become `none`.

So a skin cannot run code or load anything from another host.

`skin="strip:<url>;frames=N"` uses a PNG filmstrip or sprite sheet, one frame per position (rounded):
- `orientation=vertical|horizontal`: frames top to bottom or left to right; by default vertical when the image is at least as tall as it is wide.
- `columns=C`: a sprite sheet, frames row by row.
- Retina: a name ending in `@2x` is a double-resolution image (shown at half its pixel size; `scale=S` for another scale), and `2x=<url>` names a double-resolution copy of a 1x image (CSS `image-set`). URLs are used as written: the server refuses `%`, so names are never percent-encoded (`knob@2x.png` stays `knob@2x.png`).

URLs are relative to the page. A skin that does not load falls back to the built-in drawing with a `console.info` line.

## Canvas

`<signal-canvas>` (M2.8-7, ADR 0023 §5) draws the state of a visual-rate model, streamed as vector frames (`ui-protocol.md`, the `vector` stream, frame kind 4): one subscription per bound port, the latest snapshot drawn at the next animation frame. It simulates nothing: the engine's modules (`std.particles`, `std.mesh2d`, `std.body2d`) are the model, so a replay of a recorded session draws the same.

| `renderer` | Binds | Draws |
|---|---|---|
| `points` | a positions vector (x, y pairs), optionally a life vector (0 dead) | each live particle: a dot sized and faded by its life, or a skin's frame |
| `mesh` | a displacement vector of `grid` × `grid` nodes | a slanted wire grid (depth foreshortened by `tilt`, displacement times `height`) |
| `bodies` | a bodies vector (x, y, size, kind per body) | the box [−1, 1]² and each body: a circle (size its radius) or a box (size its half-width), or a skin stretched over it |

The world is the box [−1, 1]², y up, fitted to the canvas with its aspect kept (`fitBox`). **Backends.** With `backend="auto"` (the default) an unskinned `points` or `mesh` canvas draws with WebGL when the browser gives a context (`ui/canvas/gl.js`: round point sprites, line pairs), and everything else with Canvas 2D; `2d` forces Canvas 2D. **Skins** are the controls' (`strip:…` and `svg:…`, "Skins"), loaded as images and drawn with `drawImage`, never inserted: for `points`, the frame of a sprite sheet is chosen by the particle's age (`spriteFrame`: the frames spread evenly over its life); for `bodies`, frame 0 for circles and the last for boxes, or the SVG over every body. The spark sheet (`ui/assets/spark-strip.png`, `projects/physical-av/ui/assets/spark.png`: 8 frames of 32 × 32 in a row, a glow cooling and shrinking with age) is drawn by `node scripts/ui-strip.mjs sparks`; `ui/assets/body.svg` is a shaded disc.

The pure parts (`ui/lib/canvas.js`: projection, scaling, sprite frame choice, the draw lists, the backend choice, colours) are tested by `node --test ui/`; the renderers are `ui/canvas/points.js`, `mesh.js` and `bodies.js`.

## Theming

Every color and size is a CSS custom property with a dark default (`ui/controls/theme.css` lists them): `--signal-bg`, `--signal-panel`, `--signal-fg`, `--signal-muted`, `--signal-track`, `--signal-accent`, `--signal-effective`, `--signal-thumb`, `--signal-knob`, `--signal-focus`, `--signal-clip`, the meter colours, `--signal-font`, `--signal-radius`, and the sizes (`--signal-fader-length`, `--signal-knob-size`, …, `--signal-readout-width`, `--signal-value-width`, `--signal-card-min`). Set them on the page, a container or one control.

## Stable layout

**The rule (M2.8-11, the owner's: "things keep vibrating, jumping and repositioning").** A value changing, a stream frame, a hover, a drag or an unsaved mark appearing never changes the size or the position of any other element. Only the dragged control's own thumb moves. Everything in `ui/` follows it:

- **Readouts have a fixed width, in tabular figures, with their space reserved.** A control's `.readout` is a box of `--signal-readout-width` (11ch by default; the meter's 9ch, the 2D pad's as wide as its pad, the scope's and canvas's their own width) and fixed height, `white-space: pre`, cut with an ellipsis rather than grown. The fader's and knob's effective value has its own second line, there (empty) when not modulated. `<signal-value>` is `--signal-value-width` wide. Where text is built for a column, `formatFixed` (`ui/lib/format.js`) gives every value of a unit the same number of characters (`fixedWidth(unit)`), in exponent form when it would not fit.
- **Table columns have fixed widths** (`table-layout: fixed` with a `<colgroup>`): Signal.app's parameter list.
- **Unsaved and error marks reserve their space or overlay.** A control's unsaved mark is placed over what is below it (`position: absolute`), never in its flow; a row's mark, the indicator in `<signal-panel>`'s bar and in Signal.app's messages keep their space and are only made visible (`visibility`), never inserted; their changing text ("1 unsaved parameter", "2 …", "… — error") sits in a box sized by its row or of fixed width, cut with an ellipsis, so Retry and Revert beside it never move.
- **A `describe` with the same structure updates the values in place.** Signal.app's parameter list rebuilds its DOM only when the instances or parameters change (`structureKey`, `planUpdate` in `ui/app/lib/parameters.js`), and never under the slider being dragged: that waits for its release. Meters are rebuilt only when the meters change; the generated panel only when what it shows does (its controls, meters and scope, `defaultPanel`), not on a new plan version with the same structure.
- **Meters, scopes and status lines have fixed sizes.** The meter's art is fixed; the scope and canvas are as wide as their card and of a fixed height; Signal.app's toolbar status is `compactStatus` (`ui/app/lib/status.js`), always 25 characters, its status card fixed-width fields (`statusFields`), its meters box and messages fixed heights (the newest messages, each cut to one line, the whole text in its tooltip).
- **No layout depends on text that changes while running.** Labels, titles, the bar's status and the toolbar's notice are cut with an ellipsis in boxes of fixed width; hover and focus change colours and outlines only, never a border width, padding or size.

`node --test ui/` checks these from the source (`ui/app/lib/layout.test.js`, the fixed widths in `ui/lib/format.test.js` and `ui/app/lib/status.test.js`). Whether nothing moves on the screen is checked by a person: the bounding boxes of every element, recorded over 5 s in headless Chrome and in Signal.app's window while playing with streams and during a drag.

## Gallery

`ui/gallery.html` shows every control in every mode. Without a fragment in the URL it runs against `FakeConnection` (a demo project with deterministic signals, a modulated control and a meter that clips every few seconds). With the `#ws=…&token=…` fragment that `signal ui` prints it connects to the running project and adds a section built from its `describe`: a fader and a value per control, the output meters and a scope of the first signal output. `window.signalConnection` is the connection, for the console.

Load it:
- from the server: `signal ui <project>`, then open `http://127.0.0.1:<port>/ui/gallery.html` (add the printed fragment to connect);
- from a file, in Safari (`file:///…/ui/gallery.html`). Chromium refuses ES modules from `file://` (CORS, origin `null`); use the server there. Under `file://`, a browser may also refuse to fetch the SVG skins; those controls then draw their default.

### Checks in a browser

What `node --test` cannot see, checked by a person (the M2.8-2 checkpoint):
1. From the server: `signal ui projects/kick --driver null`, open `/ui/gallery.html` in Safari and in Chrome with their inspectors: no console errors, only 127.0.0.1 requests.
2. From a file: open `ui/gallery.html` in Safari: it runs on the fake connection, with at most `console.info` lines.
3. Interaction: fader and knob drags (with Shift), double-click reset, the wheel, the knob's circular drag; the pad's spring-back; the button lit while held; the toggle; the meter's hold falling after about 1.5 s and the clip cleared by a click and by Enter; the scope steady at each edge and level, E flipping the edge, X–Y drawing; Tab to every control (focus ring) and the keys above; touch (an iPad with `signal ui --remote`, or Safari's responsive mode); the strips sharp on a retina display; `--signal-accent` set on `body` restyling the controls.
4. Live: `signal ui projects/kick --open`, then the printed `#ws=…&token=…` on `/ui/gallery.html`: a fader drag changes one key in `project.toml` on release; a burst of wheel steps writes it once, about 500 ms after the last; a value typed on a fader (double-click its value) changes the key on Enter; `signalConnection.socket.close()` reconnects and the meters resume, while a `describe()` outstanding at the close rejects with `closed`; after a server restart (a new token) the status is `failed`, with no retries.
5. `cargo test --workspace` on the Mac, for the socket tests a sandbox cannot run.

## Panels

A project carries its panels as text (ADR 0023 §4): HTML files in its `ui/` folder, `ui/main.html` the main one, with their SVG and PNG files in `ui/assets/`. `signal ui` serves them under `/project/` and prints the main panel's URL (`docs/ui-protocol.md`). A panel is a page with the controls inside a `<signal-panel>`, laid out as a grid of cards (M2.8-11): `.signal-cards` (as many columns of at least `--signal-card-min` as fit the width) holding `.signal-card`s (a `wide` one spans the grid), both from `theme.css`. Every text of a panel is inside a card: its title and description too (`ui/lib/cards.test.js` checks the example panels and the default one). Inside a card, a `.row` lays controls out in a line that wraps; controls that belong together, such as a stereo meter pair, go in a `.signal-group`, which is never broken across lines and wraps as a whole.

```html
<link rel="stylesheet" href="/ui/controls/theme.css">
<signal-panel>
  <div class="signal-cards">
    <section class="signal-card">
      <h2>kick</h2>
      <div class="row">
        <signal-knob bind="/drums.kick_decay" taper="log" min="100" max="1500" label="decay" skin="svg:assets/knob.svg"></signal-knob>
        <signal-meter bind="/out.l" label="L"></signal-meter>
      </div>
    </section>
  </div>
</signal-panel>
<script type="module">import '/ui/controls/index.js';</script>
```

A panel has no scroll container of its own (no `overflow: auto`, no `100vh`): its page scrolls, which in Signal.app is the Panel tab's frame.

Panels import the library by its served path (`/ui/…`); their own files are relative to the page (`assets/knob.svg` is the project's `ui/assets/knob.svg`).

**`<signal-panel>`** connects with the page's `#ws=…&token=…`, makes that the controls' connection (`setDefaultConnection`), shows the connection's status and the project's name in its bar (a card of one line), and once described asks the server's binding check for its file (`panel.check`; `file` overrides the file the page's path names), once per plan version (`BindingCheck` in `ui/lib/panel.js`); a check the connection dropped is asked again with the `describe` of the reconnect. Every diagnostic is listed at the top of the panel as `ui/main.html:36:28: error: …`, and the control it is about gets an outline (red for an error, amber for a warning), the diagnostic as its tooltip and, for an error, `aria-invalid`. It checks again when the plan version changes. With `generated` it builds the default panel instead.

**The binding check** (`crates/signal-ui/src/bindings.rs`) is the same for `signal ui check <project>` (every `ui/*.html`, against the project's plan; exit 1 on an error) and for `panel.check` (one file, against the running plan). It reads every `signal-*` start tag (comments and the content of `script`, `style`, `textarea`, `title` skipped) and checks each `bind` as the browser gives it to the control, with character references decoded as the HTML tokenizer decodes an attribute value (numeric ones, `&#46;` or `&#x2E;`; every name of the WHATWG table, `crates/signal-ui/src/entities.rs`, the longest that matches; a legacy name without `;`, such as `&amp`, is left as written before `=` or an ASCII letter or digit):

| Control | Binds | Addresses |
|---|---|---|
| `<signal-fader>`, `<signal-knob>`, `<signal-toggle>` | a control: a top-level parameter (`/<instance>.<parameter>`, a composite's exports included), which `commit` writes to `project.toml` | 1 |
| `<signal-xy>` | controls, x then y | 2 |
| `<signal-value>` | a parameter: a control or a plan parameter port (it only shows the value) | 1 |
| `<signal-button>` | an event input of the plan | 1 |
| `<signal-meter>` | a signal input of the audio sink (none with `source="input"`) | 1 |
| `<signal-scope>` | signal outputs or inputs of the plan (a primitive's: `/drums/kick/dc.out`, not the composite's `/drums.out`; an input shows the sum of its sources) | 1 or 2 |
| `<signal-canvas>` | vector outputs of the plan (`/membrane_v.z`) | 1 to 3 |

Errors: an address that is not one, an unknown port (the message lists what the instance has, or says that it is a composite), a port of the wrong kind or shape (`wrong kind: /out.gain is a parameter; <signal-scope> binds a signal output or input of the plan`), an editing control on a parameter inside a composite, which `commit` refuses (`not editable: /drums/kick/amp_env.decay_ms is a parameter inside the composite /drums, …`: export it, or show it with `<signal-value>`), the wrong number of addresses, a `bind` missing where one is needed, and an unknown `signal-*` element. Warnings: an editing control without `bind` (it edits only its own value), `taper="log"` where the range does not start above 0 (the control is linear), and a file without `<signal-panel>`. Positions are `file:line:column` (1-based, columns in characters) at the address in the source (where a character reference starts, if it does), or at the tag's `<`, as the project loader's.

**Edits** are the controls' own: a drag sends ramps and commits on release, which writes the value into `project.toml` (`commit`); the panel file is never written. The running project already holds the written value, so the file watch does not reload it (no `reload`, nothing in the log). A write that fails leaves the edit unsaved, with the control's mark and the bar's indicator ("Unsaved" above).

**The default panel.** For a project without `ui/main.html`, `/project/main.html` is `ui/default-panel.html`, whose `<signal-panel generated>` builds from `describe` (`defaultPanel` in `ui/lib/panel.js`): the top-level parameters (`controls`) grouped by instance, a fader for a level (unit `dB`, or a gain without unit; a dB taper for a gain) and a knob for anything else (a log taper for Hz and times whose range starts above 0); a meter per signal input of the audio sink; and a scope on the sink's first input (`/out.l`: the sum of every source feeding it, each scaled, as the sink receives it). Each instance is a card, the meters and the scope a wide `output` card. It is rebuilt only when that structure changes; a new plan version with the same structure keeps its elements, which take the new values.

**The example panels.** `projects/kick/ui/main.html`: decay and drive knobs (an SVG skin, `projects/kick/ui/assets/knob.svg`), the gain fader, the output meters, a scope of the kick. `projects/pulse/ui/main.html`: cutoff and resonance on a 2D pad, the delay's time, feedback and level on knobs (a PNG filmstrip, `projects/pulse/ui/assets/knob-strip.png` and `@2x`, drawn by `node scripts/ui-strip.mjs pulse`), the eight sequence steps as faders (55–440 Hz), the gain, meters and a scope of the filter. `projects/physical-av/ui/main.html` (M2.8-7): three canvases (the membrane seen, as a mesh; the sparks with the PNG sprite sheet; the bodies with the SVG skin), a strike button, knobs, meters and a scope of the membrane heard.

### Checks of the panels by the operator

What the sandbox cannot run (a browser, a device, loopback), checked by a person (the M2.8-3 checkpoint):
1. `signal ui check projects/kick` and `signal ui check projects/pulse`: each prints `…/ui/main.html: N controls, M bindings: ok` and exits 0.
2. The AC2 twin over TCP: `cargo test -p signal-ui --test panels` (the `::tcp` tests and `a_project_without_a_main_panel_is_served_the_generated_one` bind 127.0.0.1); all pass.
3. Kick: `signal ui projects/kick --open` (CoreAudio, the default output) opens the printed panel URL. The status reads `open`, no diagnostics are listed, and the summary reads `ui/main.html: 6 controls, 6 bindings`. The kick plays. Drag the decay knob (SVG skin): the tail changes while dragging; on release `project.toml` gets `kick_decay = …` on the `drums` line and nothing else changes (`git diff projects/kick`); the terminal prints no `watch:` line; `ui/main.html` is unchanged. Drag drive and the gain fader the same way. The meters move with the kick and hold their peaks; the scope shows a steady kick waveform. Undo with `git checkout projects/kick/project.toml`.
4. Pulse: `signal ui projects/pulse --open`. Move the pad: cutoff (x) and resonance (y) change the sound and their values; its release commits both axes, so `filter`'s `cutoff_hz` and `q` are written (an axis that did not move is written as it was). Move the delay knobs (PNG strip, sharp on a retina display): each release writes one key, `delay`'s `time_ms`, `feedback`'s `db` or `mix`'s `gain_b`. Move a few step faders: the sequence's pitches change, and each release writes that step's key, `seq`'s `s1` … `s8`; the gain fader writes `out`'s `gain`. Nothing else in `project.toml` changes (`git diff projects/pulse`), and `ui/main.html` never does. Meters and scope as above.
5. In-page errors: copy `projects/kick` to a scratch folder, change the decay knob's bind to `/drums.kick_decya`, the drive knob's to `/drums/kick/amp_env.decay_ms` and the scope's to `/out.gain`, run `signal ui <copy> --driver null --open`: the panel lists the three diagnostics (unknown port, not editable, wrong kind) with `ui/main.html:line:column`, outlines the three controls in red with the message as tooltip, and the other controls still work. `signal ui check <copy>` prints the same three lines and exits 1. Reload the page with the server stopped and started again (a new token, so open the new URL): the diagnostics are shown again.
6. Default panel: `signal ui crates/signal-cli/tests/fixtures/sine --driver null --open`: the generated panel shows the instances' parameters (knobs, the gain as a fader), two meters and a scope on the sink's left input (`/out.l`).
7. Safari and Chrome, inspectors open: no console errors, only 127.0.0.1 requests.
8. Unsaved (M2.8-8): with a copy of a project open (`signal ui <copy> --open`, and in Signal.app), `chmod a-w <copy>` and move the gain fader. On release the fader gets the dashed amber outline and the error text with Retry and Revert; the bar reads "1 unsaved parameter — … Permission denied …"; Signal.app's window shows the same line above its messages and the row's error in its parameter list. A second browser on the same URL, and a reload of the page, show the same. Move the fader again, `chmod u+w <copy>`, click Retry: the newest value is in `project.toml`, and the mark and the indicator go. Make it read-only again, move the fader, click Revert: the sound and the fader return to the file's value, and the mark goes. While a fader is being dragged it shows no mark (the bar may count it).
9. The physical-av demo (M2.8-7): `signal ui check projects/physical-av` prints `…/ui/main.html: 13 controls, 14 bindings: ok`. `signal ui projects/physical-av --open`: the membrane is heard on every beat (90 bpm) and on the strike button; the membrane canvas wobbles a slanted grid from the strike point, the sparks burst with each strike (a sprite per spark, cooling as it ages) and stream with the sound, the bodies jump on every bar and fall back. In the inspector's console `[...document.querySelectorAll('signal-canvas')].map((c) => c.backend)` reads `webgl` for the membrane (or `2d` without WebGL) and `2d` for the skinned two; set `backend="2d"` on the membrane and it still draws. In the Network panel the WebSocket carries binary frames whose first byte is 4, about 60 a second per canvas; `stats` (from the raw page `/`) shows `vectorOverflows: 0` and no dropped frames. In Signal.app: Open… → `projects/physical-av` → Play, the same.
10. Signal.app's window (M2.8-11): the menu bar has Signal (About, Hide, Quit), File (Open… ⌘O, Save ⌘S, Save As… ⇧⌘S, Interface Setup… ⌘,, Quit ⌘Q; M2.8-12) and Edit; ⌘C, ⌘V and ⌘Z work in the Setup tab's text fields. Play, then switch Panel → Parameters → Panel → Parameters: the parameter list and the meters are there each time, nothing reloads. At every window width, narrow included, at most one scrollbar shows. Switch the power off: the device is released (another app can take it exclusively), Activity Monitor shows Signal below 1 % CPU averaged over 10 s with the window open; switch it on: the same project plays again. Over 5 s while playing (and during a drag) the bounding boxes of every element, in headless Chrome and in the window, do not move or resize, except the dragged control's thumb ("Stable layout").

## Schematic

`<signal-schematic>` (M2.9-1, ADR 0026 §2–3) draws the `graph` message (`ui-protocol.md`): the project's signal path as written, not flattened. It fills its container (it is `display: block; height: 100%`), takes the default connection, and has no attributes. Signal.app shows it in its Schematic tab; `signal ui` serves it alone at `/schematic` (`ui/schematic.html`), a page of its own rather than a tab of the generated panel, so that it fills the window and the panel stays a grid of cards.

- **Drawing.** Every instance is a box with its name and module, its inputs (ports, then parameters) on the left and its outputs on the right; a composite has a second, dashed frame. Wires are orthogonal, coloured by the source's shape (signal: accent, vector: the effective colour, events: green), dashed where a converter changes the rate; a feedback connection is drawn back around below. Hovering a wire names its ends and its converter. Strokes do not scale (`vector-effect: non-scaling-stroke`), text below 4 px on screen is not drawn. The layout (`ui/lib/schematic-layout.js`) is layered left to right by signal flow: longest-path layers, barycentre ordering, feedback edges reversed; a node with a position in `ui/schematic.toml` keeps it and the others move down past it.
- **Zoom and pan.** One view transform (`ui/lib/schematic-view.js`): the wheel zooms about the pointer, a pinch (ctrl+wheel, Safari's gesture events) too; dragging the background pans; `+` `−` zoom about the centre, `0` fits, the arrows pan. The zoom is clamped only to keep numbers finite: each level drawn gets its own matrix composed in float64, so the numbers SVG sees stay small.
- **Semantic zoom.** A composite opens in place when its box is wider than 360 px on screen (and closes below 300 px), or when double-clicked: its own layout is drawn scaled into its box, recursively. The breadcrumbs (the open boxes under the view's centre) show where the view is; a breadcrumb or Escape goes back out. Only what is on screen is in the DOM (`ui/lib/schematic-scene.js`).
- **Clickable.** A click selects an instance, a port or a wire (`ui/lib/schematic-hit.js`) and shows it on a card at the top right (`ui/lib/schematic-inspector.js`): an instance's parameters with their base and effective values from the `parameters` stream (`describe`'s only until it reports), editable for a top-level instance exactly as on the Parameters tab (a 5 ms ramp while dragged, one commit on release, double-click resets, the same unsaved mark), read-only inside a composite; a port's address, shape, kind and rate; a connection's ends, scale and converter. In Signal.app a right click on a part opens the context menu with "Copy address". Dragging a box moves it: `schematic.move` writes its position to `ui/schematic.toml` (`project-format.md` §11); "Reset layout" forgets the shown level's positions, "Reset all" removes the file.
- **Level bars.** Each signal output (any rate divisor; vectors and events have none) of an instance on screen has a small bar, from a `scope` stream at 7680 points per second (a 256-point block about every 33 ms; the bar is its peak, −60 to 0 dBFS). Scopes have 8 slots per run, shared with every other scope of every client (`ui-protocol.md` "Schematic level bars"), so the outputs on screen are taken in order, the selected instance's first, then top to bottom: those past the free slots show the same bar with a mark across it, **"no slot"**, asked again at most every 2 s; while stopped a bar is **idle** (empty). Live, no slot and idle bars have the same size, and nothing moves when a slot frees. A port that leaves the screen lets its slot go; the tab or page hidden (the element 0×0), the power off or nothing playing lets every slot go. Nothing runs between events: a redraw is asked for (one animation frame) only when the view, the graph or a stream frame changed, and nothing is asked for while hidden or off.

## Tests

`node --test ui/` (run by `cargo test` through `crates/signal-ui/tests/js.rs`, skipped with a message when `node` is not on PATH) covers the tapers' round trips, fine adjust, keys and reset, the ballistics against the M2.5-1 reference numbers, the scope trigger on a known sine and X–Y pairing, skin parsing, frame selection and sanitizing rules, the client against an in-process fake WebSocket (requests, frames, reconnect, rejection on close), the fake connection, a control's refused commit kept as unsaved with its error until one is confirmed and no commit passed through `quietly` (`ui/lib/binding.test.js`), the unsaved indicator's text (`ui/lib/unsaved.test.js`), typed values: the parser, the round trip of the field's starting text for every unit, the key steps per taper and the field's states (`ui/lib/entry.test.js`), and the field in its owners on a stub DOM: its lifetime, a Parameters row, a toggle without a readout, the 2D pad's readout (`ui/controls/entry-field.test.js`), that `ui/` loads nothing remote and imports only relative paths, that the project panels load nothing remote, and the default panel and diagnostics (`ui/lib/panel.test.js`). In Rust: `crates/signal-cli/tests/ui_check.rs` (`signal ui check` on the example panels and on a broken copy) and `crates/signal-ui/tests/panels.rs` (the kick's panel through the protocol, `panel.check`, the generated panel). Look and interaction are checked by a person in a browser.

**Signal.app's window** (M2.8-4, one page since M2.8-11) is `ui/app/index.html`: a toolbar, then four tabs switched without a reload or a new connection. The toolbar holds, in order, the power switch (`app.power`: off, nothing runs and nothing streams; on, what played plays again), the tabs Panel | Schematic | Parameters | Setup, Play and Stop, the project's name (with "Edited" while a `.signal` project differs from its file, M2.8-12), and the compact status with the last notice. **Panel** is the project's own panel (`/project/main.html`, as in a browser) in a same-origin frame filling the tab and kept while the others show; **Schematic** is `<signal-schematic>` filling the tab ("Schematic"; it pans and zooms, never scrolls); **Parameters** holds the status, the meters, the messages (live reload, diagnostics, notes, the unsaved line) and the parameter list, as cards; **Setup** holds the project (a folder, its `project.toml` or a `.signal` file, typed) and the interface setup. Only the shown tab scrolls, as one container: the page never does, and the Panel tab's frame scrolls its own document (`ui/app/app.css`). The menu is native (ADR 0023): File's Open… shows the system's chooser (a folder, its `project.toml` or a `.signal` file; a drop on the window or a Finder double-click opens the same), Save writes every unsaved parameter again and then a `.signal` project to its file, Save As… writes the project to a new `.signal` file (ADR 0025), Interface Setup… shows the Setup tab; in a browser, without that menu, the page keeps the toolbar and takes Cmd-O (the Setup tab's field), Cmd-S and Cmd-comma itself, and has no Save As. Web components in `ui/app/app.js` and `ui/app/setup.js` work over the protocol's app messages (`ui-protocol.md`, "Signal.app"), with their logic in `ui/app/lib/` (the shortcuts, tabs, power switch, status, Save, the setup draft, the parameter list) tested by `node --test ui/`.
