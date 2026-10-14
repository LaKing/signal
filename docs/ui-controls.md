# The control library

The web-component controls of ADR 0023 §3 (M2.8-2). They are in the repository's `ui/` folder as plain ES modules, with no dependencies and no build step. `signal ui` serves them at `/ui/` (`docs/ui-protocol.md`).

| File | Holds |
|---|---|
| `ui/signal-ui.js` | The protocol client: `connect`, `connectFromPage`, `Connection` (describe, subscribe, set, ramp, trigger, commit, reconnect), the default connection the controls use |
| `ui/controls/index.js` | Defines every element below (import it once) |
| `ui/controls/theme.css` | The theme's custom properties and their dark defaults |
| `ui/lib/` | Pure logic with `node --test` tests: `taper.js`, `gesture.js`, `ballistics.js`, `trigger.js`, `skin.js`, `format.js`; `protocol.js` (M2.8-1) |
| `ui/fake-connection.js` | A connection without an engine, for the gallery |
| `ui/gallery.html` | Every control in every mode |
| `ui/assets/` | The gallery's skins; `scripts/ui-strip.mjs` draws its PNGs |

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

**Edits.** A drag sends a `ramp` (5 ms) per move and a `commit` on release. Keys and the wheel send a live `set` per step and one `commit` 400 ms after the last one; a pending commit is also sent when the control loses focus, leaves the page, or the page is hidden or closed (`pagehide`, `visibilitychange`). A reset (double-click, Delete) commits at once. Edits the server refuses are not reported in the console; the next parameter frame shows the value the runner has. Base and effective values come from a `parameters` subscription (30 per second).

**The client.** `Connection` says `hello`, asks `describe` when welcomed, and keeps its subscriptions across reconnects (backoff 0.25, 0.5, 1, 2, then 5 s; the new `sub` ids are mapped). A request that is outstanding when the connection drops, or made while it is down, is rejected with `code: 'closed'` and never sent again: a repeated `commit` or `trigger` would act twice. After close code 1008 (unauthorized, for example a server restarted with a new token) or 1002 (protocol) it stops, with status `failed`.

## Elements

Common to all:
- `bind`: the port address (a control or plan parameter port; an event input for `<signal-button>`; signal outputs for `<signal-scope>`).
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
| `<signal-value>` | `bind`, `unit`, `digits` | — | `status`; shows the effective value, else the base |

The fader and knob show the effective value when it differs from the base (a modulated parameter): an amber mark on the fader, a dot on the knob's arc, and `→ value` in the readout and `aria-valuetext`.

The meter uses the M2.5-1 ballistics (`crates/signal-app/src/meter.rs`) ported line for line to `ui/lib/ballistics.js`: a 1.5 s peak hold, a 300 ms RMS, and a clip indicator latched at a peak of 1.0 until cleared. A protocol meter frame carries each channel's peak and RMS over `extra` frames, so its block's sum of squares is `rms² · frames`.

The scope shows the subscribed points from the first crossing of the trigger level on the trigger edge that leaves a full display after it (half of the last two consecutive blocks, at most 256 points), else from the start (free running). X–Y pairs the points of the latest block of each port that fall on the same frames.

### Keys

Shift makes every step fine (×0.1).

| Element | Arrows | Page Up / Page Down | Home / End | Other |
|---|---|---|---|---|
| Fader, knob | Up, Right +1 %; Down, Left −1 % of the travel | ±10 % | min / max | Delete, Backspace: default |
| 2D pad | Left/Right move X, Up/Down move Y, 1 % | Y ±10 %; with Alt, X | both to min / both to max | Delete: defaults; Escape: centre (with `spring`) |
| Button | — | — | — | Space, Enter: trigger (held; repeats ignored) |
| Toggle | Right, Up on; Left, Down off | on / off | off / on | Space, Enter flip |
| Meter | the scale's floor ±6 dB | ±24 dB | floor −96 dB / −60 dB | Enter, Space, Delete clear the clip |
| Scope | trigger level ±0.01 (Shift ±0.001) | ±0.1 | level 0 / the last block's middle | E: edge; Space: freeze |
| Value | one decimal more or less | — | fewest / most decimals | — |

The meter, scope and value send nothing to the engine; their keys change what they show (and the meter's clip latch).

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

## Theming

Every color and size is a CSS custom property with a dark default (`ui/controls/theme.css` lists them): `--signal-bg`, `--signal-panel`, `--signal-fg`, `--signal-muted`, `--signal-track`, `--signal-accent`, `--signal-effective`, `--signal-thumb`, `--signal-knob`, `--signal-focus`, `--signal-clip`, the meter colours, `--signal-font`, `--signal-radius`, and the sizes (`--signal-fader-length`, `--signal-knob-size`, …). Set them on the page, a container or one control.

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
4. Live: `signal ui projects/kick --open`, then the printed `#ws=…&token=…` on `/ui/gallery.html`: a fader drag changes one key in `project.toml` on release; a burst of wheel steps writes it once, about 400 ms after the last; `signalConnection.socket.close()` reconnects and the meters resume, while a `describe()` outstanding at the close rejects with `closed`; after a server restart (a new token) the status is `failed`, with no retries.
5. `cargo test --workspace` on the Mac, for the socket tests a sandbox cannot run.

## Tests

`node --test ui/` (run by `cargo test` through `crates/signal-ui/tests/js.rs`, skipped with a message when `node` is not on PATH) covers the tapers' round trips, fine adjust, keys and reset, the ballistics against the M2.5-1 reference numbers, the scope trigger on a known sine and X–Y pairing, skin parsing, frame selection and sanitizing rules, the client against an in-process fake WebSocket (requests, frames, reconnect, rejection on close), the fake connection, and that `ui/` loads nothing remote and imports only relative paths. Look and interaction are checked by a person in a browser.
