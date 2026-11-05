// <signal-plane>: the Signal Project, one infinite plane (ADR 0051 phase
// 1). The zoom and the focused Stack decide the level (ui/lib/plane-zoom.js):
//   - **Setup**: every Stack as its column of live front panels, in
//     miniature; this machine's audio peripherals from the Setup and the
//     device inventory (`setup.get`, `setup.devices`), in our signal
//     direction; the role endpoints (`main`, and every device slot's role,
//     ADR 0038) on the channels their role is bound to, and the cables from
//     the Stacks' edge pins to them (ui/lib/plane-peripherals.js). A cable
//     is dragged (ADR 0051 §3, §9): from a Stack's out pin onto a device
//     port it creates the connection (and, on free channels, a new device
//     slot and its role's binding); its device end onto another port moves
//     only the binding (`device.route`; Shift: this project only,
//     devices.local.toml); dropped on the empty plane, its connection goes;
//   - **Stack**: a click on a Stack's header, Enter, or zooming until it
//     fills the stage fits its width; the wheel then scrolls it vertically,
//     and the Setup's cables, peripherals and other Stacks fade to SURROUND.
//     Here the bar's Edit layout (and a front panel's context menu) opens
//     the project's panel page in Edit layout over the plane (ADR 0051 §6,
//     §9), and in Signal.app a front panel's card menu moves its card to
//     another window or a new panel window (ADR 0032; ui/app/lib/cards.js
//     `connectPlaneCards`), a card another window holds drawn as a
//     placeholder;
//   - **Wiring** (phase 2): zooming on at the Stack's fit, in the wiring
//     band (WIRING_BAND, in depth: the view deepens by LIFT at the fit's
//     scale, so the whole width stays on stage), a cross-fade in place to
//     the back of the rack: each
//     instrument a box in its own rectangle, its ports derived from the
//     connections that cross it, the cables between instruments in channels
//     and rails (ui/lib/plane-wiring.js). Patch cables are edited here
//     (`patch`). ADR 0047's single top-level board under the Stack is
//     retired (ADR 0051 §12);
//   - **Control**: zooming on, the instrument under the zoom point opens
//     into its Control plane, the sheet under its front panel: its live
//     controls where the panel has them (x-ray), its engine as one package
//     (the cube), binding legs into it, automation and external control
//     cables landing on the controls, the inner values beside them
//     (ui/lib/plane-control.js); the other instruments fade to SURROUND.
//     Automation (`patch`) and binds (`panel.write`) are edited here;
//   - **Engine** (the owner's decision of 2026-11-03): every instrument's
//     inside drawn in its own rectangle, in place, as a layer of the plane
//     like the others (ui/lib/plane-engine.js): its instances as cards, their
//     ports as pins, the signal connections between them and through the
//     instrument's edge; no parameter legs or controls (the Control
//     layer's). The front panels, Wiring and Control drawings fade out and
//     each instrument's rectangle stays as a faint frame. The view never
//     changes when the layer does. A composite is one card (a chip); zoomed
//     in, its inside fades in within the card's body, in place and scaled
//     into it, recursively (`renderInsides`), and the bar names the chips
//     the view is inside (Layer 1, Layer 2, …). Step 3: a click selects a
//     card, a pin or a wire (inside a chip revealed enough too, by full
//     address, ui/lib/plane-engine-edit.js); with the bar's Edit on, a drag
//     from a pin connects, Delete removes, the menu adds a module (the
//     schematic's browser) or removes, each a `patch` at its level: the
//     top level for an instrument, the composite's for a chip's inside
//     (its definition, every instance of it); Escape steps out one chip.
// The levels are nested fades of one zoom; no rectangle moves, and zoom
// never changes the selection.
// The Stacks are `ui/plane.toml` (served as /project/plane.toml), or one
// primary Stack derived from the panel's cards (ui/lib/plane-file.js); an
// instrument is a card of the panel (the migration form) drawn at the
// Stack's width (ui/lib/plane-layout.js), its controls live as on the
// panel: each card is the panel's own code (`showWritten`) in a shadow
// root with the panel's style sheets. Every level is its own layer (the
// Stacks' HTML, the Setup's SVG, each Stack's Wiring, Control and Engine
// SVG), one transform for the view; each is laid out lazily, when it
// first shows, and cached; nothing is laid out per zoom frame. View state
// only: the plane writes no file of its own (rule 4); its edits go through
// the existing `patch` and `panel.write`.

import { onDefaultConnection } from '../signal-ui.js';
// Every control a front panel may hold is defined on the plane's page too (the open slot card among them).
import './index.js';
import { loadWhitelist, showWritten } from './panel-editor.js';
import { generatedTree, parseContent, serialize } from '../lib/panel-tree.js';
import { setAttributes } from '../lib/panel-edit.js';
import { graphGeneration, named } from '../lib/opening.js';
import { defaultPanel } from '../lib/panel.js';
import { faceAttribute, rootRules, stylesheets } from '../lib/schematic-face.js';
import { GUTTER, PU, parsePlace, parsePlate } from '../lib/plate.js';
import { PLANE_FILE, cardOfEntry, entryOf, panelCards, planeOf, withEntries, writePlane } from '../lib/plane-file.js';
import { assignInstruments, edgePins, hasPart, instanceOf, instrumentTitle, roleEndpoints, stackMembers } from '../lib/plane-instruments.js';
import { cableDrop, channelSummary, endpointLabel, peripherals, playbackPortOf, setupCables, splitBoxes } from '../lib/plane-peripherals.js';
import { BOX_PU, OTHERS_PU, TEXT, TRAY_LINE_PU, capChars, boxPorts, cablePath, othersBoxes, pinPlaces, planeLayout, stackLayout, textSize, textSizes } from '../lib/plane-layout.js';
import { NOT_CONTROLS, READERS, bindList, fitText } from '../lib/schematic-controls.js';
import { CONTROL_BAND, ENGINE_BAND, FOCUS_BAND, LIFT, MARGIN_PX, WIRING_BAND, depthOf, engineBand, fitAll, fitScale, fitWidth, instrumentAt, levelOf, panBy, planeFades, reachesFit, scrollStack, splitDepth, toPlane, toScreen, zoomAbout, zoomStep } from '../lib/plane-zoom.js';
import { WIRING_TEXT, edgeCableMoves, polylinePath, portIndex, wiringLayout, wiringModel } from '../lib/plane-wiring.js';
import { automations, controlLayout, engineOf, innerAddress } from '../lib/plane-control.js';
import { packageOf } from '../lib/plane-package.js';
import { CARD_RX, REVEAL, chipPath, engineLayout, insideOf, locateAddress, revealShare } from '../lib/plane-engine.js';
import { ENGINE_SELECTABLE, addOps, bindKey, bindMoveOps, cardKey, connectOps, rewireOps, dropTargets, engineItems, engineSelection, engineSubject, engineTarget, fullAddress, norm, outStep, pinKey, placedOwners, removeOps, selectionKeys, wireKey } from '../lib/plane-engine-edit.js';
import { selectionNet } from '../lib/plane-net.js';
import { planeSidebar } from '../lib/plane-sidebar.js';
import { headPath } from '../lib/schematic-draw.js';
import { hitItem, visibleIds } from '../lib/plane-hit.js';
import { PatchSession } from '../lib/schematic-edit.js';
import { addConnection, connectTargets, defaultName, instanceCount, nameError, namesOf, removeConnection } from '../lib/schematic-patch.js';
import { EXPANDED, browserEntries } from '../lib/module-browser.js';
import { projectKey } from '../lib/schematic-palette.js';
import { levelData, loadable } from '../lib/slot.js';
import { BROWSER_CSS, ModuleBrowser } from './module-browser.js';
import { formatValue } from '../lib/format.js';
import { toPosition } from '../lib/taper.js';
import { HIT_OPACITY, ZOOM_MS, ease, tween } from '../lib/board-zoom.js';
import { WheelKinds } from '../lib/wheel.js';
import { wheelFactor, wheelPixels } from '../lib/schematic-view.js';
import { isPressable, isTextField, registerWheel, takesWheel } from './keyboard.js';
import { ButtonPan, closePageMenu } from './button-pan.js';
import { keyCommand } from '../lib/keys.js';
import { bindAddresses } from './base.js';
import { cardMenu, cardsOf, menuMessage } from '../app/lib/cards.js';

const SVG = 'http://www.w3.org/2000/svg';
/** The theme every panel links. */
const THEME = new URL('./theme.css', import.meta.url).href;
/**
 * Whether the zoom decides the layer (ADR 0051 §8, the levels as nested
 * fades of one zoom) or the layer is chosen alone and the zoom only scales
 * (the owner's trial, 2026-11-01; the default). localStorage
 * `signal.plane.coupled` = "1" brings the coupled zoom back.
 */
function zoomPicksLayer() {
  try {
    return globalThis.localStorage?.getItem('signal.plane.coupled') === '1';
  } catch {
    return false;
  }
}

/** The automation wires' arrowhead colours, by kind (the k-* classes' colours). */
/** A combine mark's sign by the parameter's `combine`. */
const COMBINE_SIGN = { add: '+', multiply: '×', replace: '=' };
const ARROW_COLOURS = { audio: '#4fc3f7', control: '#b39ddb', gate: '#ff6b6b', frequency: '#c6e05a', events: '#3ecf8e', vector: '#f48fb1', generic: '#9aa4b2' };

/** The zoom lock (the bar's switch; per viewer, localStorage): the wheel scrolls up and down, the pointer zooms nothing. */
const ZOOM_LOCK_KEY = 'signal.plane.zoomLock';
function readZoomLock() {
  try {
    return globalThis.localStorage?.getItem(ZOOM_LOCK_KEY) === '1';
  } catch {
    return false;
  }
}
function writeZoomLock(on) {
  try {
    globalThis.localStorage?.setItem(ZOOM_LOCK_KEY, on ? '1' : '0');
  } catch {
    // Storage blocked: the switch holds for this page only.
  }
}

/** The layers in the bar, top to bottom (ADR 0051; the owner's names, 2026-11-01). */
const LAYERS = [['setup', 'Project'], ['stack', 'Stack'], ['wiring', 'Wiring'], ['control', 'Control'], ['engine', 'Engine']];
const LAYER_NAME = Object.fromEntries(LAYERS);
/** The bar's layers: the Project is the Stack layer's, not a button of its own (the owner's, 2026-11-02). */
/** The lines' corners on the Deck: 'chamfer' (45°, a circuit board's traces) or 'round'; localStorage `signal.plane.corners` overrides. */
const CORNERS = (() => {
  try {
    return globalThis.localStorage?.getItem('signal.plane.corners') === 'round' ? 'round' : 'chamfer';
  } catch {
    return 'chamfer';
  }
})();
const BAR_LAYERS = LAYERS.filter(([id]) => id !== 'setup');
/** The chosen layer's zoom limit out, relative to the focused Stack's fit. */
const MIN_SCALE = 0.02;
/** The chosen layer's zoom limit in, relative to the focused Stack's fit, but at the Engine. */
const MAX_LAYER_SCALE = 64;
/** The Engine's zoom limit in, relative to the focused Stack's fit: deep enough for chips inside chips, still bounded. */
const DEEP_SCALE = 4096;
/** At most this many chips' insides kept built while off the stage (detached, out of the document); more are dropped and built again when needed. */
const INSIDES_KEPT = 48;

/** The factor of one zoom key (`plane.zoom_in`, `plane.zoom_out`), about the stage's centre. */
const KEY_ZOOM = 1.25;
/** Where the list of other devices' expanded state is kept, per viewer (localStorage; view state, never the project's). */
const OTHERS_KEY = 'signal.plane.otherDevices';

function othersExpanded() {
  try {
    return globalThis.localStorage?.getItem(OTHERS_KEY) === 'open';
  } catch {
    return false;
  }
}

/** The level bar's styles: in the plane, or docked in another element (`dockBar`). */
const BAR_CSS = `
.bar { position: absolute; left: 8px; top: 8px; display: flex; gap: 6px; align-items: center; padding: 4px 8px; border-radius: 6px;
  background: color-mix(in srgb, var(--signal-panel, #1f232b) 88%, transparent); border: 1px solid #343b47; user-select: none; }
.bar[hidden] { display: none; }
.bar .layers { display: flex; border: 1px solid #4a5260; border-radius: 4px; overflow: hidden; }
.bar .layer { background: none; border: none; border-left: 1px solid #343b47; color: var(--signal-fg, #d6dbe4); padding: 1px 9px; font: inherit; cursor: pointer; }
.bar .layer:first-child { border-left: none; }
.bar .layer:hover:not(:disabled), .bar .layer:focus-visible { color: var(--signal-accent, #4fc3f7); outline: none; }
.bar .layer[aria-current="true"] { background: var(--signal-accent, #4fc3f7); color: #10141a; cursor: default; }
.bar .layer:disabled { color: var(--signal-muted, #8a93a3); opacity: 0.5; cursor: default; }
.bar .zoom { display: flex; align-items: center; gap: 2px; margin-left: 6px; }
.bar .zoom button { background: none; border: 1px solid #4a5260; border-radius: 4px; color: var(--signal-fg, #d6dbe4); width: 22px; padding: 0; font: inherit; line-height: 18px; cursor: pointer; }
.bar .zoom button:hover, .bar .zoom button:focus-visible { border-color: var(--signal-accent, #4fc3f7); color: var(--signal-accent, #4fc3f7); outline: none; }
.bar .level { color: var(--signal-muted, #8a93a3); min-width: 5ch; text-align: center; font-variant-numeric: tabular-nums; }
.bar .zoom-lock { margin-left: 6px; display: inline-flex; align-items: center; justify-content: center; width: 26px; height: 22px; padding: 0; background: none; border: 1px solid #4a5260; border-radius: 4px; color: var(--signal-fg, #d6dbe4); cursor: pointer; }
.bar .zoom-lock svg { width: 15px; height: 15px; fill: none; stroke: currentColor; stroke-width: 1.6; stroke-linecap: round; }
.bar .zoom-lock:hover, .bar .zoom-lock:focus-visible { border-color: var(--signal-accent, #4fc3f7); color: var(--signal-accent, #4fc3f7); outline: none; }
.bar .zoom-lock[aria-pressed="true"] { background: var(--signal-accent, #4fc3f7); border-color: var(--signal-accent, #4fc3f7); color: #10141a; }
.bar .edit-layout { margin-left: 6px; background: none; border: 1px solid #4a5260; border-radius: 4px; color: var(--signal-fg, #d6dbe4); padding: 1px 9px; font: inherit; cursor: pointer; }
.bar .edit-layout:hover:not(:disabled), .bar .edit-layout:focus-visible { border-color: var(--signal-accent, #4fc3f7); color: var(--signal-accent, #4fc3f7); outline: none; }
.bar .edit-layout[aria-pressed="true"] { background: var(--signal-accent, #4fc3f7); border-color: var(--signal-accent, #4fc3f7); color: #10141a; }
.bar .edit-layout:disabled { color: var(--signal-muted, #8a93a3); opacity: 0.5; cursor: default; }
.bar.docked { position: static; padding: 2px 8px; }
`;

const CSS = `
${BAR_CSS}
:host { display: block; position: relative; overflow: hidden; height: 100%; outline: none; overscroll-behavior: none;
  color: var(--signal-fg, #d6dbe4); background: var(--signal-board-bg, #0e1014); font: var(--signal-font, 11px/1.3 -apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif); }
.stage { position: absolute; inset: 0; overflow: hidden; touch-action: none; }
.world { position: absolute; left: 0; top: 0; transform-origin: 0 0; }
:host([data-panning]) .stage { cursor: move; }
.stack { position: absolute; box-sizing: border-box; background: #20242c; border: 2px solid #3a414d; border-radius: 6px; }
.stack.other { transition: none; }
.fit-icon { display: inline-flex; align-items: center; justify-content: center; width: 20px; height: 20px; padding: 0; margin: 0; flex: none;
  background: none; border: none; color: var(--signal-muted, #8a93a3); opacity: 0.7; cursor: pointer; }
.fit-icon:hover, .fit-icon:focus-visible { color: var(--signal-accent, #4fc3f7); opacity: 1; outline: none; }
.fit-icon svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 1.6; stroke-linecap: round; }
.stack-head { position: absolute; left: 0; right: 0; top: 0; display: flex; align-items: center; gap: 0.6em; padding: 0 16px; box-sizing: border-box; white-space: nowrap; overflow: hidden;
  font-size: var(--t-head, 18px); line-height: 1; letter-spacing: 0.18em; text-transform: uppercase; color: var(--signal-muted, #8a93a3); cursor: zoom-in; user-select: none; border-bottom: 1px solid #3a414d; }
.stack-head b { color: var(--signal-fg, #d6dbe4); font-weight: 600; }
.stack-head .count { margin-left: auto; font-size: var(--t-count, 13px); letter-spacing: 0.08em; }
.rack { position: absolute; left: 0; top: 0; pointer-events: none; }
.plate-blank { position: absolute; box-sizing: border-box; background: repeating-linear-gradient(135deg, #262a32 0 6px, #22262d 6px 12px); border-top: 1px solid #343a45; border-bottom: 1px solid #14161b; }
.plate-blank.wide { outline: 2px dashed var(--signal-effective, #ffb74d); outline-offset: -2px; }
.panels { position: absolute; left: 0; top: 0; }
svg.setup { position: absolute; overflow: visible; pointer-events: none; }
svg.setup text { font-family: -apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif; }
.cable { fill: none; stroke: var(--signal-accent, #4fc3f7); stroke-width: 2.5; vector-effect: non-scaling-stroke; stroke-linecap: round; }
.cable.dashed { stroke-dasharray: 6 5; }
.cable.ghost { stroke: var(--signal-effective, #ffb74d); stroke-dasharray: 3 4; }
.port.target { stroke: var(--signal-effective, #ffb74d); stroke-width: 2.5; }
.pin { fill: var(--signal-accent, #4fc3f7); stroke: var(--signal-board-bg, #0e1014); stroke-width: 2; vector-effect: non-scaling-stroke; }
.pin.in { fill: var(--signal-meter-rms, #3ecf8e); }
.pin-label { fill: var(--signal-fg, #d6dbe4); font-size: var(--t-port, 28px); }
.box { fill: #1b1f26; stroke: #4a5260; stroke-width: 2; vector-effect: non-scaling-stroke; }
.box.absent { fill: none; stroke-opacity: 0.5; }
.box-title { fill: var(--signal-fg, #d6dbe4); font-size: var(--t-title, 34px); font-weight: 600; }
.box-note { fill: var(--signal-clip, #ff5252); font-size: var(--t-kind, 20px); }
.box-kind { fill: var(--signal-muted, #8a93a3); font-size: var(--t-kind, 20px); letter-spacing: 0.12em; }
.port { fill: #232832; stroke: #39404c; stroke-width: 1; vector-effect: non-scaling-stroke; }
.port.bound { stroke: var(--signal-accent, #4fc3f7); stroke-width: 1.5; }
.port-name { fill: var(--signal-fg, #d6dbe4); font-size: var(--t-port, 28px); }
.port-role { fill: var(--signal-accent, #4fc3f7); font-size: var(--t-role, 24px); font-weight: 600; }
.jack { fill: #0e1014; stroke: var(--signal-muted, #8a93a3); stroke-width: 1.5; vector-effect: non-scaling-stroke; }
.jack.bound { stroke: var(--signal-accent, #4fc3f7); fill: var(--signal-accent, #4fc3f7); }
.endpoint { fill: none; stroke: var(--signal-muted, #8a93a3); stroke-width: 1.5; stroke-dasharray: 5 4; vector-effect: non-scaling-stroke; }
.endpoint-text { fill: var(--signal-muted, #8a93a3); font-size: var(--t-port, 28px); }
.tray { fill: #181b21; stroke: #4a5260; stroke-dasharray: 6 4; stroke-width: 1.5; vector-effect: non-scaling-stroke; }
.tray-text { fill: var(--signal-muted, #8a93a3); font-size: var(--t-port, 28px); }
.others { fill: #181b21; stroke: #4a5260; stroke-width: 1.5; vector-effect: non-scaling-stroke; }
.others-head { pointer-events: all; cursor: pointer; outline: none; }
.others-head rect { fill: transparent; }
.others-head:hover .box-title, .others-head:focus-visible .box-title { fill: var(--signal-accent, #4fc3f7); }
.others-head:focus-visible rect { stroke: var(--signal-accent, #4fc3f7); stroke-width: 1.5; vector-effect: non-scaling-stroke; }
.small { fill: #1b1f26; stroke: #39404c; stroke-width: 1; vector-effect: non-scaling-stroke; }
.small-name { fill: var(--signal-fg, #d6dbe4); font-size: var(--t-small, 26px); }
.small-kind { fill: var(--signal-muted, #8a93a3); font-size: var(--t-kind, 20px); }
/* Edit layout (ADR 0051 §6): the project's panel page in its editor, over the plane. */
.editor { position: absolute; inset: 0; z-index: 6; display: flex; flex-direction: column; background: var(--signal-bg, #15181d); }
.editor-body { flex: 1; display: block; min-height: 0; overflow: auto; background: var(--signal-bg, #15181d); }
.notes { position: absolute; left: 8px; bottom: 8px; max-width: 60%; display: grid; gap: 2px; pointer-events: none; }
.notes p { margin: 0; padding: 3px 6px; border-radius: 4px; background: color-mix(in srgb, var(--signal-panel, #1f232b) 90%, transparent); font: 11px/1.4 ui-monospace, Menlo, monospace; }
.notes p.error { color: var(--signal-clip, #ff5252); }
.notes p.warning { color: var(--signal-effective, #ffb74d); }
/* ADR 0051 phase 2: the Wiring level (the back of the rack) and the Control plane (the sheet under a front panel), each Stack's, in its coordinates. */
svg.wiring, svg.control, svg.engine, svg.drag { position: absolute; left: 0; top: 0; width: 1px; height: 1px; overflow: visible; pointer-events: none; }
svg.wiring text, svg.control text, svg.engine text { font-family: -apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif; }
.w-box { fill: #13161b; stroke: #2e343e; stroke-width: 1.5; vector-effect: non-scaling-stroke; }
/* The engine package (2026-11-03): one drawing at the Wiring, Control and Engine layers, the same place and pins. */
.pkg-body { fill: #232832; stroke: #7a8496; stroke-width: 1.5; vector-effect: non-scaling-stroke; }
.pkg-top { fill: #2d3440; pointer-events: none; }
.pkg-title { fill: var(--signal-fg, #d6dbe4); font-weight: 600; letter-spacing: 0.08em; pointer-events: none; }
.pkg-pin { fill: var(--signal-board-bg, #0e1014); stroke-width: 2; vector-effect: non-scaling-stroke; }
/* Every chip's pin at every layer (the owner's, 2026-11-04): a connected one filled in its kind's colour, a free one a ring round a black hole. */
.pkg-pin.connected, .e-pin.connected { fill: var(--k, #9aa4b2); }
/* Hover titles where nothing lies under it: at the Engine its body and pins, at the Wiring its pins (at Control a front panel's piano roll may lie under it). */
.pkg-engine .pkg-body, .pkg-engine .pkg-pin, .pkg-wiring .pkg-pin, .pkg-param, .pkg-auto { pointer-events: all; }
.pkg-param { fill: var(--signal-effective, #ffb74d); stroke: var(--signal-board-bg, #0e1014); stroke-width: 1.5; vector-effect: non-scaling-stroke; }
.pkg-auto { fill: #b39ddb; stroke: var(--signal-board-bg, #0e1014); stroke-width: 1.5; vector-effect: non-scaling-stroke; }
.pkg-pin.selected { stroke: var(--signal-focus, #5aa9ff); stroke-width: 4; }
.pkg-pin-label { fill: var(--signal-fg, #d6dbe4); font-size: var(--w-port, 12px); pointer-events: none; }
.pkg-control .pkg-pin-label { font-size: 9px; fill: var(--signal-muted, #8a93a3); }
.w-box.selected { stroke: var(--signal-focus, #5aa9ff); stroke-width: 2.5; }
.w-title { fill: var(--signal-muted, #8a93a3); font-size: var(--w-title, 16px); letter-spacing: 0.16em; font-weight: 600; }
.w-port { fill: var(--signal-board-bg, #0e1014); stroke-width: 2; vector-effect: non-scaling-stroke; }
.w-port.selected { stroke-width: 4; }
.w-port-label { fill: var(--signal-fg, #d6dbe4); font-size: var(--w-port, 12px); }
.w-cable { fill: none; stroke-width: 2.5; vector-effect: non-scaling-stroke; stroke-linecap: round; stroke-linejoin: round; }
.w-cable.selected, .c-auto.selected, .c-leg.selected { stroke-width: 5; filter: drop-shadow(0 0 3px currentColor); }
.w-junction { stroke: none; }
.w-edge { fill: var(--signal-accent, #4fc3f7); stroke: var(--signal-board-bg, #0e1014); stroke-width: 2; vector-effect: non-scaling-stroke; }
.w-edge.in { fill: var(--signal-meter-rms, #3ecf8e); }
/* The colour standard (the owner's decision of 2026-11-04): one colour per kind of cable (ui/lib/plane-wiring.js CABLE_KINDS); amber stays a control's (a knob's) own. */
.k-audio { --k: var(--signal-accent, #4fc3f7); } .k-control { --k: #b39ddb; } .k-gate { --k: #ff6b6b; } .k-frequency { --k: #c6e05a; }
.k-events { --k: var(--signal-meter-rms, #3ecf8e); } .k-vector { --k: #f48fb1; } .k-generic { --k: #9aa4b2; }
:is(.k-audio, .k-control, .k-gate, .k-frequency, .k-events, .k-vector, .k-generic) { stroke: var(--k); color: var(--k); }
.w-junction, .e-junction { fill: var(--k, #9aa4b2); }
.c-sheet { fill: #0f1d17; stroke: #24412f; stroke-width: 1; vector-effect: non-scaling-stroke; }
.c-footprint { fill: color-mix(in srgb, var(--signal-effective, #ffb74d) 7%, #13241c); stroke: none; }
.c-leg { fill: none; stroke: var(--signal-effective, #ffb74d); stroke-width: 1; vector-effect: non-scaling-stroke; opacity: 0.45; color: var(--signal-effective, #ffb74d); }
.c-leg.reads { stroke: var(--signal-meter-rms, #3ecf8e); }
.c-via { fill: var(--signal-effective, #ffb74d); opacity: 0.6; }
.c-via.selected, .c-pin.selected { stroke: var(--signal-focus, #5aa9ff); stroke-width: 2; vector-effect: non-scaling-stroke; }
.c-auto { fill: none; stroke-width: 2.5; vector-effect: non-scaling-stroke; stroke-linejoin: round; }
.c-auto.inner { stroke-width: 1.75; opacity: 0.85; }
.c-pin { fill: var(--signal-effective, #ffb74d); stroke: var(--signal-board-bg, #0e1014); stroke-width: 1.5; vector-effect: non-scaling-stroke; }
.c-auto-pin { fill: #b39ddb; stroke: var(--signal-board-bg, #0e1014); stroke-width: 1.5; vector-effect: non-scaling-stroke; }
.c-auto-label { fill: var(--signal-muted, #8a93a3); font-size: 9px; }
.c-edge { fill: #b39ddb; stroke: var(--signal-board-bg, #0e1014); stroke-width: 2; vector-effect: non-scaling-stroke; }
.c-edge.edge-out { fill: var(--signal-effective, #ffb74d); }
.c-readout-text { fill: var(--signal-effective, #ffb74d); font-size: 9px; font-variant-numeric: tabular-nums; }
.c-readout-track { fill: #2a2f38; }
.c-readout-fill { fill: var(--signal-effective, #ffb74d); }
/* The Engine layer (2026-11-03): each instrument's inside in its rectangle, the cards in the Deck's look; colours written out (WebKit). */
.e-frame { fill: none; stroke: #3a414d; stroke-width: 1; vector-effect: non-scaling-stroke; }
.e-title { fill: var(--signal-muted, #8a93a3); font-size: var(--w-title, 16px); letter-spacing: 0.16em; font-weight: 600; opacity: 0.7; }
.e-card { fill: #161a20; stroke: #3d4552; stroke-width: 1.5; vector-effect: non-scaling-stroke; }
.e-card.composite { stroke: var(--signal-muted, #8a93a3); stroke-width: 2; }
.e-head { fill: #212733; pointer-events: none; }
/* Hover titles: the cards, pins and wires take the pointer (it still reaches the stage: pan, wheel and menu are the Deck's). */
.e-card, .e-pin { pointer-events: all; }
.e-wire { pointer-events: stroke; }
.e-name { fill: var(--signal-fg, #d6dbe4); font-weight: 600; }
.e-module, .e-pin-label { fill: var(--signal-muted, #8a93a3); }
.e-pin { fill: var(--signal-board-bg, #0e1014); stroke-width: 1.5; vector-effect: non-scaling-stroke; }
.e-wire { fill: none; stroke-width: 2; vector-effect: non-scaling-stroke; stroke-linecap: round; stroke-linejoin: round; }
/* The control signal into the engine, in the Control layer's amber: a bound parameter's leg from the package's connector. */
.e-bind { stroke: var(--signal-effective, #ffb74d); } .e-combine circle { fill: var(--signal-board-bg, #0e1014); stroke: var(--signal-effective, #ffb74d); } .e-combine text { fill: var(--signal-effective, #ffb74d); font-weight: 700; pointer-events: none; } .e-junction.e-bind-dot { fill: var(--signal-effective, #ffb74d); }
.e-pin.e-bind-pin, .e-pin.connected.e-bind-pin { fill: var(--signal-effective, #ffb74d); stroke: var(--signal-board-bg, #0e1014); }
.e-junction { stroke: none; }
.e-plain { fill: #1d232c; pointer-events: none; }
.e-pin-label.halo { stroke: #1d232c; stroke-linejoin: round; paint-order: stroke; }
.e-inside .e-card { fill: #1b2028; }
.e-inside .e-wire, .e-inside .e-card, .e-inside .e-pin { vector-effect: none; }
.e-inside[data-faint] .e-card, .e-inside[data-faint] .e-pin, .e-inside[data-faint] .e-wire { pointer-events: none; }
/* The Engine's selection (step 3), as Wiring's: a glow, the card's edge and a pin in the focus colour (inside a chip a line keeps its width at scale). */
.e-wire.selected { stroke-width: 4; filter: drop-shadow(0 0 3px currentColor); }
/* A selected net followed (the owner's, 2026-11-04): everything else steps back, at every layer and depth. */
svg[data-net] :is(.e-wire, .w-cable, .c-leg, .c-auto):not(.selected) { opacity: 0.18; }
svg[data-net] :is(.e-pin, .pkg-pin, .pkg-param, .pkg-auto, .c-pin, .c-auto-pin, .c-via, .w-junction, .e-junction):not(.selected) { opacity: 0.35; }
.e-card.selected { stroke: var(--signal-focus, #5aa9ff); stroke-width: 3; }
.e-pin.selected { stroke: var(--signal-focus, #5aa9ff); stroke-width: 3; }
.stage[data-engine-edit] .e-pin { cursor: crosshair; }
/* No dashed lines (the owner's rule): the cable being drawn is a plain line. */
.rubber { fill: none; stroke: var(--signal-focus, #5aa9ff); }
.drop-hint { fill: none; stroke: var(--signal-focus, #5aa9ff); }
.chooser { position: absolute; z-index: 5; width: min(640px, calc(100% - 16px)); overflow: hidden; display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); padding: 4px; border-radius: 6px; background: var(--signal-panel, #1f232b); border: 1px solid #3a414d; box-shadow: 0 6px 20px #0008; }
.chooser-title { grid-column: 1 / -1; padding: 2px 6px 4px; color: var(--signal-muted, #8a93a3); }
.chooser button { text-align: left; background: none; border: none; color: var(--signal-fg, #d6dbe4); padding: 3px 6px; font: 11px/1.4 ui-monospace, Menlo, monospace; border-radius: 4px; cursor: pointer; }
.chooser button:hover, .chooser button:focus-visible { background: #2c3340; outline: none; }
.notes p.said { color: var(--signal-fg, #d6dbe4); }
/* The sidebar (the owner's, 2026-11-04): a fixed column at the plane's left, the stage beside it, its content scrolling inside it. */
.stage { left: 280px; }
.side { position: absolute; left: 0; top: 0; bottom: 0; width: 280px; box-sizing: border-box; overflow: auto; padding: 10px 12px 16px;
  background: var(--signal-board-bg, #0e1014); border-right: 1px solid #3a414d; overscroll-behavior: contain; z-index: 3; }
.side header { display: flex; align-items: flex-start; gap: 8px; }
.side h2 { flex: 1; margin: 0; font-size: 13px; font-weight: 600; overflow-wrap: anywhere; }
.side h3 { margin: 14px 0 4px; font-size: 10px; font-weight: 600; letter-spacing: 0.1em; text-transform: uppercase; color: var(--signal-muted, #8a93a3); }
.side-sub { margin: 4px 0 0; color: var(--signal-muted, #8a93a3); overflow-wrap: anywhere; }
.side dl { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: 2px 10px; margin: 10px 0 0; }
.side dt { color: var(--signal-muted, #8a93a3); } .side dd { margin: 0; overflow-wrap: anywhere; }
.side-close { background: none; border: none; color: var(--signal-muted, #8a93a3); font-size: 16px; line-height: 1; cursor: pointer; padding: 0 2px; }
.side-close:hover { color: var(--signal-fg, #d6dbe4); }
.side-link { display: block; width: 100%; text-align: left; background: none; border: none; border-radius: 3px; padding: 2px 4px; color: var(--signal-accent, #4fc3f7);
  font: inherit; cursor: pointer; overflow-wrap: anywhere; }
.side-link:hover, .side-link:focus-visible { background: #1c2129; outline: none; }
.side-action { margin-top: 14px; background: var(--signal-knob, #262b34); border: 1px solid var(--signal-knob-edge, #3a414d); border-radius: 4px; color: var(--signal-fg, #d6dbe4);
  font: inherit; padding: 4px 10px; cursor: pointer; }
@media (prefers-reduced-motion: reduce) { .world { transition: none; } }
/* The schematic's Add module browser, for Add module… at the Engine (its preview frames plain, no dashed line). */
${BROWSER_CSS}
.browser .preview .frame-box { border-style: solid; }
/* Its schematic preview in the schematic's look, kept apart from the Setup's own .box, .pin and .pin-label. */
.browser svg.preview-svg .box { fill: #161a20; stroke: #3d4552; stroke-width: 1.5; stroke-dasharray: none; vector-effect: non-scaling-stroke; }
.browser svg.preview-svg .composite > .box { stroke: var(--signal-muted, #8a93a3); stroke-width: 2; }
.browser svg.preview-svg .frame { fill: none; stroke: #5b6474; vector-effect: non-scaling-stroke; }
.browser svg.preview-svg .pins > .box { fill: none; stroke: #2a2f38; }
.browser svg.preview-svg .name { fill: var(--signal-fg, #d6dbe4); font-weight: 600; font-size: 12px; }
.browser svg.preview-svg .module { fill: var(--signal-muted, #8a93a3); font-size: 10px; }
.browser svg.preview-svg .pin-label { fill: var(--signal-muted, #8a93a3); font-size: 9px; }
.browser svg.preview-svg .pin { fill: #2a2f38; stroke: var(--signal-muted, #8a93a3); stroke-width: 1; vector-effect: non-scaling-stroke; }
.browser svg.preview-svg .pin.signal { stroke: var(--signal-accent, #4fc3f7); } .browser svg.preview-svg .pin.events { stroke: var(--signal-meter-rms, #3ecf8e); } .browser svg.preview-svg .pin.vector { stroke: var(--signal-effective, #ffb74d); }
.browser svg.preview-svg .pin.parameter:not(.connected) { stroke: var(--signal-muted, #8a93a3); }
.browser svg.preview-svg .wire { fill: none; stroke: var(--signal-accent, #4fc3f7); stroke-width: 1.5; vector-effect: non-scaling-stroke; }
.browser svg.preview-svg .wire.events { stroke: var(--signal-meter-rms, #3ecf8e); } .browser svg.preview-svg .wire.vector { stroke: var(--signal-effective, #ffb74d); }
`;

/** A front panel's shadow root CSS: the panel's cards placed by the plane, the panel's own look otherwise. */
const PANEL_CSS = `
:host { display: block; position: absolute; left: 0; top: 0; contain: layout style; }
.face-panel { position: relative; font: var(--signal-font, 11px/1.3 -apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif); color: var(--signal-fg, #d6dbe4); }
.face-panel > .signal-cards { display: block !important; position: relative; margin: 0; padding: 0; }
.face-panel > .signal-cards > * { position: absolute !important; left: 0; top: 0; margin: 0 !important; box-sizing: border-box; transform-origin: 0 0; }
.face-note { margin: 0; padding: 4px 6px; font: 11px/1.4 ui-monospace, Menlo, monospace; color: var(--signal-clip, #ff5252); }
.face-note:empty { display: none; }
/* ADR 0051 phase 2: at Control the opened instrument's panel is x-rayed: its controls live and whole, its decoration gone, the sheet under it showing. */
.face-panel > .signal-cards > [data-xray] { background: transparent !important; border-color: transparent !important; border-image: none !important; box-shadow: none !important; }
.face-panel > .signal-cards > [data-xray]::before, .face-panel > .signal-cards > [data-xray]::after { display: none !important; }
[data-xray] .signal-group { background: transparent !important; border-color: transparent !important; box-shadow: none !important; }
[data-xray] signal-piano-roll, [data-xray] signal-step-grid, [data-xray] p { opacity: 0.08; pointer-events: none; }
/* ADR 0046 §5: the open slot card is one box here (its own CSS makes it display: contents in a grid): its header above its part, as theme.css places it on a plate. */
.face-panel > .signal-cards > signal-slot[data-card] { display: flex !important; flex-direction: column; gap: 0.6rem; }
.face-panel > .signal-cards > signal-slot[data-card] > .signal-cards { display: grid; }
/* ADR 0032: a card another window holds keeps its place, nothing of it drawn but its placeholder. */
[data-held] { position: relative; background: transparent !important; border-color: transparent !important; box-shadow: none !important; }
[data-held]::before, [data-held]::after { visibility: hidden !important; }
[data-held] :not(.held-note, .held-note *) { visibility: hidden !important; }
.held-note { position: absolute; inset: 0; z-index: 1; display: flex; gap: .6rem; align-items: center; padding: 0 .75rem; box-sizing: border-box; border: 1px dashed var(--signal-track, #2a2f38); border-radius: 8px; background: var(--signal-bg, #15181d); color: var(--signal-muted, #8a93a3); font: 12px/1.3 -apple-system, BlinkMacSystemFont, sans-serif; overflow: hidden; white-space: nowrap; visibility: visible !important; }
.held-note span { flex: 1; overflow: hidden; text-overflow: ellipsis; }
.face-panel > .signal-cards > [data-faded] .held-note { visibility: hidden !important; pointer-events: none; }
.held-note button { font: inherit; color: var(--signal-fg, #d6dbe4); background: var(--signal-track, #2a2f38); border: 1px solid var(--signal-muted, #8a93a3); border-radius: 3px; padding: .1rem .6rem; cursor: pointer; }
`;

function svg(name, attributes = {}, text = null) {
  const node = document.createElementNS(SVG, name);
  for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, String(value));
  if (text !== null) node.textContent = text;
  return node;
}

/** A small screen: the button that puts its Stack or instrument on the screen (`fitOnScreen`). */
function fitIcon(title, act) {
  const b = html('button', { type: 'button', className: 'fit-icon', title });
  b.innerHTML = '<svg viewBox="0 0 14 14" aria-hidden="true"><path d="M1 5V1h4M9 1h4v4M13 9v4H9M5 13H1V9"/></svg>';
  b.addEventListener('pointerdown', (event) => event.stopPropagation());
  b.addEventListener('click', (event) => {
    event.stopPropagation();
    act();
  });
  return b;
}

function html(name, properties = {}) {
  return Object.assign(document.createElement(name), properties);
}

/** A view as `{k, x, y}` (an `ox`, `oy` view of ui/lib/schematic-view.js made plain). */
function plain(view) {
  return { k: view.k, x: view.x - (view.ox ?? 0) * view.k, y: view.y - (view.oy ?? 0) * view.k };
}

/** The project's panel page, which the panel's relative URLs resolve from. */
function pageBase() {
  try {
    return new URL('/project/main.html', globalThis.location?.href).href;
  } catch {
    return null;
  }
}

export class SignalPlane extends HTMLElement {
  constructor() {
    super();
    this.view = { k: 0.25, x: 0, y: 0 };
    this.viewport = { w: 0, h: 0 };
    this.stops = [];
    /** The panel as read: `{html, plate, text, generated}`, or null. */
    this.panel = null;
    /** `ui/plane.toml`'s text, or null (no file). */
    this.planeText = null;
    this.graph = null;
    this.setup = null;
    this.devices = null;
    /** What is drawn, computed from the above (`compute`). */
    this.model = null;
    /** Each Stack's element: name → `{element, head, host, shadow, container, cards, sizes, key}`. */
    this.stacks = new Map();
    this.focused = null;
    this.fitted = false;
    this.wheelKinds = new WheelKinds();
    this.base = pageBase();
    this.sheets = [THEME];
    this.rootCss = '';
    this.generation = 0;
    /** Whether the list of other devices is expanded: per viewer (`OTHERS_KEY`), never in the project's text (rule 4). */
    this.othersOpen = othersExpanded();
    this.textKey = '';
    /** The instrument that opens into its Control plane (view state: follows the zoom point below the control band). */
    this.opened = null;
    /** The layer chosen in the bar when the zoom does not pick it (`zoomPicksLayer`). */
    this.layer = zoomPicksLayer() ? null : 'setup';
    this.zoomLock = readZoomLock();
    /** The depth past the Stack's fit at its scale (`depthOf`: the wiring band, view state). */
    this.lift = 0;
    /** What is selected at Wiring and Control (view state, by connection or address; zoom never changes it). */
    this.selection = null;
    /** The patches written from Wiring and Control, for undo and redo (the schematic's session model). */
    this.session = new PatchSession();
    this.graphKey = '';
    /** Signal.app's channel for the card menu (ui/app/lib/cards.js `connectPlaneCards`), or null (a browser). */
    this.cardChannel = null;
    /** What the app told this window (`signal-window`: `{view: 'main', held, targets}`), or null. */
    this.windowTold = null;
    /** The Edit layout over the plane (`editLayout`), or null. */
    this.editing = null;
    /** The bar's Edit at the Engine (view state; off when the layer changes). */
    this.engineEdit = false;
    /** Instances added from an instrument's Engine, kept in it until a connection or a control joins them to one (`placedOwners`; view state of this page). */
    this.placed = new Map();
  }

  connectedCallback() {
    if (!this.shadowRoot) this.build();
    this.resize = new ResizeObserver(() => this.measure());
    this.resize.observe(this);
    this.cardResize = new ResizeObserver(() => this.relayoutSoon());
    this.stopDefault = onDefaultConnection((connection) => this.attach(connection));
  }

  disconnectedCallback() {
    this.resize?.disconnect();
    this.cardResize?.disconnect();
    this.stopDefault?.();
    this.detach();
  }

  build() {
    const root = this.attachShadow({ mode: 'open' });
    this.tabIndex = 0;
    this.stage = html('div', { className: 'stage' });
    this.world = html('div', { className: 'world' });
    this.setupSvg = svg('svg', { class: 'setup' });
    this.world.append(this.setupSvg);
    this.bar = html('nav', { className: 'bar' });
    this.notes = html('div', { className: 'notes', role: 'status' });
    // The sidebar: a fixed column left of the stage, what is selected and its net (`renderSide`); the stage, and the view, beside it.
    this.side = html('aside', { className: 'side' });
    this.stage.append(this.world, this.bar, this.notes);
    if (this.barDock) this.dockBar(this.barDock);
    root.append(html('style', { textContent: CSS }), this.side, this.stage);
    this.renderSide();
    // The wheel: the plane's own on every layer (captured before a knob or a fader sees it when the layer is chosen alone).
    this.stage.addEventListener('wheel', (e) => this.wheelCapture(e), { passive: false, capture: true });
    this.stage.addEventListener('wheel', (e) => this.wheel(e), { passive: false });
    registerWheel(this);
    this.stage.addEventListener('pointerdown', (e) => this.pointerDownCapture(e), true);
    this.stage.addEventListener('pointerdown', (e) => this.pointerDown(e));
    // The right or the middle button dragged pans (a right click that does not move keeps its menu).
    // A right click that does not move opens its menu on the release; a double right click toggles the zoom lock.
    this.buttonPan = new ButtonPan(this, {
      capture: this.stage,
      pan: (dx, dy) => {
        this.settleTravel();
        this.setView(panBy(this.view, dx, dy));
      },
      doubleRight: () => this.toggleZoomLock(),
      closeMenu: () => closePageMenu(),
      skip: (event) => this.overEditor(event),
    });
    this.stage.addEventListener('pointerdown', (e) => this.buttonPan.down(e), true);
    this.stage.addEventListener('pointermove', (e) => this.buttonPan.move(e), true);
    this.stage.addEventListener('pointerup', (e) => this.buttonPan.up(e), true);
    this.stage.addEventListener('pointercancel', (e) => this.buttonPan.up(e), true);
    this.stage.addEventListener('lostpointercapture', (e) => this.buttonPan.lost(e));
    this.stage.addEventListener('pointermove', (e) => this.pointerMove(e));
    this.stage.addEventListener('pointerup', (e) => this.pointerUp(e));
    this.stage.addEventListener('pointercancel', () => this.cancelPointer());
    this.stage.addEventListener('lostpointercapture', (e) => {
      if (this.gesture?.id === e.pointerId) this.cancelPointer();
    });
    this.stage.addEventListener('dblclick', (e) => this.doubleClick(e));
    this.addEventListener('keydown', (e) => this.key(e), true);
  }

  // The connection.

  attach(connection) {
    this.detach();
    this.connection = connection;
    if (!connection) return;
    const on = (event, fn) => {
      const stop = connection.on(event, fn);
      if (typeof stop === 'function') this.stops.push(stop);
    };
    on('graph', (graph) => this.graphSeen(graph));
    on('welcome', () => this.fetchAll());
    // A saved Setup change, from any window or `signal mcp --attach` (`setup.apply`, a cable's `device.route` or
    // `device.unroute`): the host pushes `setup` with all the Setup level needs, so nothing is read again.
    on('setup', (push) => this.setupPushed(push));
    on('reload', () => this.readPlane());
    on('describe', (description) => this.described(description));
    on('panel.written', (message) => {
      if ((message?.file ?? 'main.html') === 'main.html') this.readPanel();
    });
    on('app', (app) => {
      const generation = app?.generation ?? null;
      if (generation !== null && generation !== this.appGeneration) {
        const changed = this.appGeneration !== undefined;
        this.appGeneration = generation;
        if (changed) this.reset();
      }
    });
    if (connection.status === undefined || connection.status === 'open') this.fetchAll();
  }

  detach() {
    this.innerStream?.stop?.();
    this.innerStream = null;
    for (const stop of this.stops.splice(0)) stop();
    this.connection = null;
  }

  request(message) {
    return this.connection?.request?.(message) ?? Promise.reject(new Error('not connected'));
  }

  /** Another project: nothing of the last one's drawing is kept. */
  reset() {
    this.generation++;
    this.panel = null;
    this.planeText = null;
    this.graph = null;
    this.model = null;
    this.fitted = false;
    this.touched = false;
    this.focused = null;
    this.stopTravel();
    this.sheets = [THEME];
    this.rootCss = '';
    this.panelCards = [];
    this.cards = [];
    this.layout = null;
    this.panelPending = false;
    this.opened = null;
    this.lift = 0;
    this.selection = null;
    this.graphKey = '';
    this.closeChooser();
    this.closeEditLayout();
    this.updateInner([]);
    for (const s of this.stacks.values()) s.element.remove();
    this.stacks.clear();
    this.fetchAll();
  }

  fetchAll() {
    this.readPanel();
    this.readSetup();
    const gen = this.generation;
    this.request({ type: 'graph' }).then(
      (graph) => gen === this.generation && this.graphSeen(graph),
      () => {},
    );
  }

  /** The panel (`panel.read` of main.html; without one, the generated panel of `describe`). */
  readPanel() {
    const gen = this.generation;
    this.request({ type: 'panel.read', file: 'main.html' }).then(
      async (read) => {
        if (gen !== this.generation) return;
        if (read?.exists && typeof read.html === 'string') {
          this.pageSeen(read.text ?? '');
          this.panelSeen({ html: read.html, plate: read.plate ?? null, generated: false });
        } else {
          this.pageSeen('');
          this.generatedFrom(this.connection?.description ?? null);
        }
      },
      () => {},
    );
  }

  /** A generated panel follows `describe` (ADR 0047 §2: `generatedTree`, as the panel's first write writes it). */
  async generatedFrom(description) {
    if (!description) {
      this.panelPending = true;
      return;
    }
    this.panelPending = false;
    const rules = await loadWhitelist();
    const tree = generatedTree(defaultPanel(description));
    this.panelSeen({ html: serialize(tree, rules), plate: tree.plate ?? null, generated: true });
  }

  described(description) {
    if (this.panelPending || this.panel?.generated) this.generatedFrom(description);
  }

  /** `ui/plane.toml`, read again with every graph (a hand edit of it shows): the text, or null without one. */
  readPlane() {
    const gen = this.generation;
    if (typeof fetch !== 'function' || !this.base) return Promise.resolve();
    return fetch(new URL(PLANE_FILE, this.base).href, { cache: 'no-store' })
      .then((r) => (r.ok ? r.text() : null))
      .catch(() => null)
      .then((text) => {
        if (gen !== this.generation || text === this.planeText) return;
        this.planeText = text;
        this.recompute();
      });
  }

  /**
   * The Setup and the device inventory as listed: read when the project
   * opens (and after a refused cable). Never refreshed here (`setup.devices`
   * without `refresh`: the list the host holds, no enumeration of the
   * hardware, which pauses the control thread). Only the latest read is
   * drawn, and a `setup` push that came while it ran wins over it.
   */
  readSetup() {
    const gen = this.generation;
    const read = (this.setupReads = (this.setupReads ?? 0) + 1);
    const pushes = this.setupPushes;
    return Promise.all([this.request({ type: 'setup.get' }).catch(() => null), this.request({ type: 'setup.devices' }).catch(() => null)]).then(([setup, devices]) => {
      if (gen !== this.generation || read !== this.setupReads) return;
      // A push that came meanwhile holds a Setup and a list at least as new.
      if (pushes !== this.setupPushes) return;
      this.setup = setup;
      this.devices = devices;
      this.recompute();
    });
  }

  /** The host's `setup` push after a saved Setup change: the Setup, the run, and the device list held with where the Setup's devices are in it. */
  setupPushed(push) {
    this.setupPushes = (this.setupPushes ?? 0) + 1;
    this.setup = push;
    this.devices = push?.devices ?? this.devices;
    this.recompute();
  }

  graphSeen(graph) {
    this.graph = graph;
    // What the Wiring and Control layouts depend on: the structure, never a value.
    this.graphKey = JSON.stringify([graph?.top?.instances?.map((i) => [i.name, i.module, i.ports?.length, i.parameters?.length]), graph?.top?.connections?.map((c) => [c.from, c.to]), Object.entries(graph?.composites ?? {}).map(([k, c]) => [k, c.connections?.map((x) => [x.from, x.to]), c.parameters?.map((x) => [x.name, x.to]), c.instances?.map((x) => [x.name, x.module, x.composite])])]);
    const cleared = this.session.graphSeen(graph);
    if (cleared?.cleared) this.say(cleared.cleared);
    // The instances kept where they were added, and an Engine selection, are this opening's (another project's names are not).
    const project = `${graph?.projectId ?? `${graph?.folder ?? ''}\n${graph?.project ?? ''}`}\n${graphGeneration(graph) ?? ''}`;
    if (project !== this.placedFor) {
      this.placed.clear();
      this.placedFor = project;
      if (this.selection?.kind === 'engine') this.select(null);
    }
    // The selection's net and the sidebar of the new structure (a value alone changes neither: its key).
    if (this.graphKey !== this.sideKey) {
      this.sideKey = this.graphKey;
      this.markSelection();
    }
    this.readPlane().then(() => this.recompute());
  }

  /** The panel page's style sheets, which style the front panels too, and their page-wide rules. */
  pageSeen(text) {
    const sheets = [THEME, ...stylesheets(text, this.base).filter((s) => s !== THEME)];
    if (sheets.join('\n') === this.sheets.join('\n')) return;
    this.sheets = sheets;
    const own = sheets.filter((s) => s !== THEME);
    if (typeof fetch !== 'function' || !own.length) {
      this.rootCss = '';
      return;
    }
    const gen = this.generation;
    Promise.all(own.map((href) => fetch(href, { cache: 'no-store' }).then((r) => (r.ok ? r.text() : ''), () => ''))).then((texts) => {
      if (gen !== this.generation) return;
      this.rootCss = texts.map(rootRules).filter(Boolean).join('\n');
      for (const s of this.stacks.values()) if (s.roots) s.roots.textContent = this.rootCss;
    });
  }

  async panelSeen(panel) {
    const key = `${panel.plate}\n${panel.html}`;
    if (this.panel && `${this.panel.plate}\n${this.panel.html}` === key) return;
    const gen = this.generation;
    this.rules ??= await loadWhitelist();
    if (gen !== this.generation) return;
    this.panel = panel;
    const parsed = parseContent(panel.html, this.rules, { mode: 'read' });
    /** The panel's cards in its order (`panelCards`); `this.cards` are they with their instrument entries (`withEntries`, once the slots are known). */
    this.panelCards = parsed.tree ? panelCards(parsed.tree) : [];
    this.cards = withEntries(this.panelCards, this.slotsWithParts());
    for (const s of this.stacks.values()) s.key = null;
    this.recompute();
  }

  // The model: what is drawn, from the files.

  recompute() {
    if (!this.panel || !this.rules) return;
    const instances = this.graph?.top?.instances ?? [];
    const connections = this.graph?.top?.connections ?? [];
    const slots = this.slotsWithParts();
    const plateWidth = parsePlate(this.panel.plate)?.w ?? null;
    // The one identity rule (ui/lib/plane-file.js): a card showing a slot with a part is that slot's instrument.
    const cards = withEntries(this.panelCards ?? [], slots);
    this.cards = cards;
    const assigned = assignInstruments({ instances, connections, cards });
    // An instance added from an instrument's Engine stays in it until it is joined to one (view state, `placedOwners`).
    const { owner, forget, seen } = placedOwners(assigned, this.placed, instances, new Set([...cards.map((c) => entryOf(c)), ...slots]));
    for (const name of forget) this.placed.delete(name);
    for (const name of seen) this.placed.get(name).seen = true;
    // The unstacked warning needs the plane: computed on the plane read (or derived), then checked with it.
    let shown = planeOf(this.planeText, cards, plateWidth, { slots });
    const members = stackMembers(shown.plane, owner, instances);
    if (!shown.derived) shown = planeOf(this.planeText, cards, plateWidth, { slots, unstacked: members.unstacked });
    const { stackOf, unstacked } = stackMembers(shown.plane, owner, instances);
    const pins = new Map(shown.plane.stacks.map((s) => [s.name, edgePins(s, owner, stackOf, connections)]));
    const endpoints = roleEndpoints(instances);
    const { boxes: all, bindings, main } = peripherals({ setup: this.setup, devices: this.devices, endpoints });
    const cables = setupCables(pins, bindings);
    // A whole box for every device (the owner's, 2026-11-01): the ones the project and the Setup use first, then the rest.
    const split = splitBoxes(all, { bindings, cables, main });
    const boxes = [...split.full, ...split.others];
    const others = [];
    this.model = { plane: shown.plane, derived: shown.derived, problems: shown.problems, cards, owner, stackOf, pins, endpoints, boxes, others, bindings, cables, unstacked, main };
    if (!this.focused || !shown.plane.stacks.some((s) => s.name === this.focused)) this.focused = shown.plane.stacks[0]?.name ?? null;
    const focusedStack = shown.plane.stacks.find((s) => s.name === this.focused);
    if (!focusedStack?.instruments.includes(this.opened)) this.opened = focusedStack?.instruments[0] ?? null;
    this.renderStacks();
    this.relayout();
    this.showNotes();
  }

  /** The top-level slots with a part (ADR 0046), whose cards are their instruments. */
  slotsWithParts() {
    return (this.graph?.top?.instances ?? []).filter(hasPart).map((i) => i.name);
  }

  showNotes() {
    const problems = this.model?.problems ?? [];
    this.notes.replaceChildren(...problems.map((p) => html('p', { className: p.severity, textContent: p.message })), ...(this.said ? [this.said] : []));
  }

  // The Stacks' front panels.

  /** Each Stack's element, its front panels rendered once per panel text and Stack (never per frame). */
  renderStacks() {
    const names = new Set(this.model.plane.stacks.map((s) => s.name));
    for (const [name, s] of this.stacks) {
      if (names.has(name)) continue;
      s.element.remove();
      this.stacks.delete(name);
    }
    for (const stack of this.model.plane.stacks) {
      let s = this.stacks.get(stack.name);
      if (!s) {
        s = this.createStack(stack.name);
        this.stacks.set(stack.name, s);
      }
      const key = `${this.panel.plate}\n${this.panel.html}\n${stack.width}\n${stack.instruments.join(' ')}`;
      if (s.key === key) continue;
      s.key = key;
      s.title.textContent = stack.name;
      s.count.textContent = `${stack.instruments.length} instrument${stack.instruments.length === 1 ? '' : 's'}`;
      this.renderPanels(s, stack);
    }
  }

  createStack(name) {
    const element = html('div', { className: 'stack' });
    element.dataset.stack = name;
    const head = html('div', { className: 'stack-head', title: 'Focus this Stack (Enter)' });
    const title = html('b');
    const count = html('span', { className: 'count' });
    head.append(html('span', { textContent: 'Stack' }), title, count, fitIcon(`Put Stack ${name} on the screen`, () => this.fitOnScreen(name, null)));
    head.addEventListener('click', () => this.focusStack(name));
    const rack = html('div', { className: 'rack' });
    const host = html('div', { className: 'panels' });
    const shadow = host.attachShadow({ mode: 'open' });
    const links = html('div', { hidden: true });
    const roots = html('style', { textContent: this.rootCss });
    const note = html('p', { className: 'face-note' });
    shadow.append(links, html('style', { textContent: PANEL_CSS }), roots, note);
    // ADR 0051 phase 2: the Wiring level behind the panels, the Control plane under them, the cable being drawn over all;
    // the Engine layer (2026-11-03) with them, in the same coordinates.
    const wiringSvg = svg('svg', { class: 'wiring' });
    const controlSvg = svg('svg', { class: 'control' });
    const engineSvg = svg('svg', { class: 'engine' });
    const rubberSvg = svg('svg', { class: 'drag' });
    wiringSvg.style.display = 'none';
    controlSvg.style.display = 'none';
    engineSvg.style.display = 'none';
    element.append(rack, wiringSvg, controlSvg, engineSvg, host, rubberSvg, head);
    this.world.insertBefore(element, this.setupSvg);
    return { element, head, title, count, rack, host, shadow, links, roots, note, wiringSvg, controlSvg, engineSvg, rubberSvg, container: null, cards: new Map(), sizes: new Map(), key: null };
  }

  /** The style sheets of the panel, linked in a Stack's shadow root. */
  linkSheets(s) {
    const have = [...s.shadow.querySelectorAll('link')].map((l) => l.href);
    if (have.join('\n') === this.sheets.join('\n')) return;
    for (const link of s.shadow.querySelectorAll('link')) link.remove();
    s.links.after(...this.sheets.map((href) => Object.assign(document.createElement('link'), { rel: 'stylesheet', href })));
  }

  /** A Stack's instruments: the panel rendered by its own code, only this Stack's cards kept, in its order. */
  async renderPanels(s, stack) {
    this.linkSheets(s);
    for (const card of s.cards.values()) this.cardResize.unobserve(card);
    const container = html('div', { className: 'face-panel' });
    s.container?.remove();
    s.container = container;
    s.shadow.append(container);
    s.cards = new Map();
    s.sizes = new Map();
    const key = s.key;
    const error = await this.showPanel(container, this.panel.html);
    if (s.key !== key) return;
    s.note.textContent = error ? `ui/main.html: ${error}` : '';
    const grid = container.querySelector(':scope > .signal-cards');
    const elements = [...(grid?.children ?? [])];
    const byId = new Map();
    (this.cards ?? []).forEach((card, i) => elements[card.index ?? i] && byId.set(entryOf(card), { element: elements[card.index ?? i], card }));
    // The cards as the app names them across windows (ADR 0032, `cardsOf`: in the whole panel's order, so before the Stack keeps its own).
    s.named = new Map(cardsOf(container).map((c) => [c.raw, c]));
    for (const element of elements) element.remove();
    for (const entry of stack.instruments) {
      const found = byId.get(entry);
      if (!found) continue;
      grid.append(found.element);
      s.cards.set(entry, found.element);
      this.cardResize.observe(found.element);
    }
    this.applyHeld(s);
    this.measureCards(s, stack);
    this.relayout();
  }

  /** The panel's text drawn into `container` by the panel's own code (`showWritten`); its error, or null. */
  showPanel(container, text) {
    return showWritten(container, text, (name, value) => faceAttribute(name, value, '', this.base));
  }

  /** Each card's natural size at its width (a placed card: its place's; else the Stack's), measured once per text and when it changes. */
  measureCards(s, stack) {
    const width = stack.width * PU;
    let changed = false;
    for (const [entry, element] of s.cards) {
      const card = cardOfEntry(entry, this.cards ?? []);
      const place = parsePlace(card?.place ?? null);
      const w = place ? place.w * PU - 2 * GUTTER : width;
      element.style.width = `${w}px`;
      if (place) element.style.height = `${place.h * PU - 2 * GUTTER}px`;
      const h = place ? place.h * PU - 2 * GUTTER : element.offsetHeight;
      // Not shown (a hidden tab): measured again when it is.
      if (!(h > 0)) continue;
      const size = { entry, w, h: Math.ceil(h / PU) * PU, x: place ? place.x * PU + GUTTER : null };
      const old = s.sizes.get(entry);
      if (old?.w !== size.w || old?.h !== size.h || old?.x !== size.x) {
        s.sizes.set(entry, size);
        changed = true;
      }
    }
    return changed;
  }

  relayoutSoon() {
    if (this.relayoutPending) return;
    this.relayoutPending = true;
    requestAnimationFrame(() => {
      this.relayoutPending = false;
      let changed = false;
      for (const stack of this.model?.plane.stacks ?? []) {
        const s = this.stacks.get(stack.name);
        if (s && this.measureCards(s, stack)) changed = true;
        // A part's cards are drawn after the panel: one another window holds gets its placeholder then.
        if (s && this.heldCards().size) this.applyHeld(s);
      }
      if (changed) this.relayout();
    });
  }

  // The layout: rectangles in plane units, a function of the files and the measured panels only.

  relayout() {
    if (!this.model) return;
    const stacks = [];
    for (const stack of this.model.plane.stacks) {
      const s = this.stacks.get(stack.name);
      if (!s) continue;
      const sizes = stack.instruments.filter((e) => s.sizes.has(e)).map((e) => s.sizes.get(e));
      s.layout = stackLayout(stack, sizes);
      stacks.push({ name: stack.name, at: stack.at, size: { w: s.layout.w, h: s.layout.h } });
    }
    this.layout = planeLayout({ stacks, boxes: this.model.boxes, tray: this.model.unstacked.length, others: { count: this.model.others.length, expanded: this.othersOpen } });
    for (const stack of this.model.plane.stacks) {
      const s = this.stacks.get(stack.name);
      const rect = this.layout.stacks.get(stack.name);
      if (!s || !rect) continue;
      Object.assign(s.element.style, { left: `${rect.x}px`, top: `${rect.y}px`, width: `${rect.w}px`, height: `${rect.h}px` });
      s.head.style.height = `${s.layout.top}px`;
      s.rack.replaceChildren(
        ...s.layout.rects.map((r) => {
          const blank = html('div', { className: `plate-blank${r.wide ? ' wide' : ''}`, title: r.wide ? `${instrumentTitle(r.entry, this.cards)} is wider than the Stack: scaled down whole` : '' });
          Object.assign(blank.style, { left: `${r.x - 2}px`, top: `${r.y - 2}px`, width: `${r.w}px`, height: `${r.h}px` });
          return blank;
        }),
      );
      for (const r of s.layout.rects) {
        const element = s.cards.get(r.entry);
        if (!element) continue;
        element.style.left = `${r.panel.x - 2}px`;
        element.style.top = `${r.panel.y - 2}px`;
        element.style.transform = r.panel.scale < 1 ? `scale(${r.panel.scale})` : '';
      }
    }
    this.drawSetup();
    // Until the person moves the view, it is the whole Setup, fitted again as the panels are measured and the peripherals read.
    if (!this.touched && this.viewport.w > 0 && this.layout.stacks.size) {
      this.fitted = true;
      this.view = fitAll(this.layout.bounds, this.viewport);
    }
    this.render();
  }

  /** The Setup level's drawing: peripherals, edge pins, role endpoints, cables, the unstacked tray (plane units). */
  drawSetup() {
    const { bounds } = this.layout;
    const pad = 400;
    const box = { x: bounds.x - pad, y: bounds.y - pad, w: bounds.w + 2 * pad, h: bounds.h + 2 * pad };
    Object.assign(this.setupSvg.style, { left: `${box.x}px`, top: `${box.y}px`, width: `${box.w}px`, height: `${box.h}px` });
    this.setupSvg.setAttribute('viewBox', `${box.x} ${box.y} ${box.w} ${box.h}`);
    const groups = { cables: svg('g'), boxes: svg('g'), pins: svg('g') };
    const pinAt = new Map();
    // What a cable drag can start from or land on (plane units).
    this.hits = { pins: [], ports: [], ends: [] };
    for (const stack of this.model.plane.stacks) {
      const s = this.stacks.get(stack.name);
      const rect = this.layout.stacks.get(stack.name);
      if (!s?.layout || !rect) continue;
      for (const pin of pinPlaces(this.model.pins.get(stack.name) ?? [], s.layout.rects)) {
        const x = rect.x + (pin.side === 'out' ? rect.w : 0);
        const y = rect.y + pin.y;
        pinAt.set(`${stack.name} ${pin.side} ${pin.address}`, { x, y });
        if (pin.side === 'out') this.hits.pins.push({ stack: stack.name, pin, x, y });
        const g = svg('g', { 'data-pin': pin.address });
        g.append(svg('circle', { class: `pin ${pin.side}`, cx: x, cy: y, r: 10 }));
        const label = `${instrumentTitle(pin.instrument, this.cards)} · ${pin.label}`;
        g.append(svg('text', { class: 'pin-label', x: pin.side === 'out' ? x + 18 : x - 18, y: y + 48, 'text-anchor': pin.side === 'out' ? 'start' : 'end' }, label));
        groups.pins.append(g);
      }
    }
    const portAt = new Map();
    for (const b of this.model.boxes) {
      const r = this.layout.boxes.get(b.key);
      if (!r) continue;
      const g = svg('g', { 'data-device': b.key });
      g.append(svg('rect', { class: `box${b.present ? '' : ' absent'}`, x: r.x, y: r.y, width: r.w, height: r.h, rx: 10 }));
      // The texts stand where their caps (TEXT) fit: a note takes the kind's line.
      const inner = r.w - 48;
      if (b.note) g.append(svg('text', { class: 'box-note', x: r.x + 24, y: r.y + 42 }, fitText(b.note, capChars(TEXT.kind, inner))));
      else g.append(svg('text', { class: 'box-kind', x: r.x + 24, y: r.y + 42 }, 'AUDIO DEVICE'));
      const title = svg('text', { class: 'box-title', x: r.x + 24, y: r.y + 96 }, fitText(b.name, capChars(TEXT.title, inner)));
      title.append(svg('title', {}, b.name));
      g.append(title);
      const ports = boxPorts(b);
      // Each edge's ports in its own half: playback (our inputs) on the left, capture (our outputs) on the right.
      const half = (ports.inputs.length && ports.outputs.length ? r.w / 2 : r.w) - 9;
      const bound = this.model.bindings.filter((x) => x.device === b.key);
      for (const p of ports.inputs) {
        const roles = bound.filter((x) => x.channels.some((c) => p.channels.includes(c)));
        const rect = svg('rect', { class: `port${roles.length ? ' bound' : ''}`, x: r.x + 6, y: r.y + p.y + 3, width: half, height: p.h - 6, rx: 4, 'data-port': `${b.key} ${p.key}` });
        g.append(rect);
        if (b.present) this.hits.ports.push({ box: b.key, port: p, rect, x: r.x - 24, y: r.y + p.y, w: half + 30, h: p.h, jacks: p.pins.map((pin) => ({ channel: pin.channel, y: r.y + pin.y })) });
        g.append(svg('text', { class: 'port-name', x: r.x + 24, y: r.y + p.y + 44 }, p.name));
        if (roles.length) g.append(svg('text', { class: 'port-role', x: r.x + 24, y: r.y + p.y + 84 }, fitText(roles.map(endpointLabel).join(', '), capChars(TEXT.role, half - 24))));
        for (const pin of p.pins) {
          portAt.set(`${b.key} ${pin.channel}`, { x: r.x, y: r.y + pin.y });
          const on = roles.some((x) => x.channels.includes(pin.channel));
          // A row of many channels draws only the jacks a role uses.
          if (!p.rest || on) g.append(svg('circle', { class: `jack${on ? ' bound' : ''}`, cx: r.x, cy: r.y + pin.y, r: 8 }));
        }
      }
      for (const p of ports.outputs) {
        g.append(svg('rect', { class: 'port', x: r.x + r.w - 6 - half, y: r.y + p.y + 3, width: half, height: p.h - 6, rx: 4 }));
        g.append(svg('text', { class: 'port-name', x: r.x + r.w - 24, y: r.y + p.y + 44, 'text-anchor': 'end' }, p.name));
        if (!p.rest) for (const pin of p.pins) g.append(svg('circle', { class: 'jack', cx: r.x + r.w, cy: r.y + pin.y, r: 8 }));
      }
      groups.boxes.append(g);
    }
    if (this.layout.others) groups.boxes.append(this.drawOthers(this.layout.others));
    // A role not heard on this machine: a hollow endpoint beside the Stack (ADR 0051 §3).
    const primary = this.model.plane.stacks[0];
    const primaryRect = primary ? this.layout.stacks.get(primary.name) : null;
    let unheard = 0;
    for (const b of this.model.bindings) {
      if (b.device || !primaryRect) continue;
      const x = primaryRect.x + primaryRect.w + 120;
      const y = primaryRect.y + primaryRect.h - 240 + unheard++ * 120;
      groups.boxes.append(svg('rect', { class: 'endpoint', x, y, width: 640, height: 112, rx: 56 }));
      groups.boxes.append(svg('text', { class: 'endpoint-text', x: x + 40, y: y + 48 }, fitText(`${endpointLabel(b)} (${b.endpoint})`, capChars(TEXT.port, 560))));
      groups.boxes.append(svg('text', { class: 'endpoint-text', x: x + 40, y: y + 92 }, b.note));
      for (const stack of this.model.plane.stacks) {
        for (const pin of this.model.pins.get(stack.name) ?? []) {
          if (pin.side !== 'out' || !b.ports.some((p) => pin.outside.includes(p))) continue;
          const a = pinAt.get(`${stack.name} out ${pin.address}`);
          if (!a) continue;
          groups.cables.append(svg('path', { class: 'cable dashed', d: cablePath(a, { x, y: y + 56 }) }));
          // Its end dragged onto a device port binds the role there ("Play on …").
          const port = b.ports.find((q) => pin.outside.includes(q));
          this.hits.ends.push({ cable: { stack: stack.name, pin: pin.address, endpoint: b.endpoint, port, device: null, channel: null }, x, y: y + 56, from: a });
        }
      }
    }
    this.model.cables.forEach((c, i) => {
      const a = pinAt.get(`${c.stack} out ${c.pin}`);
      const b = portAt.get(`${c.device} ${c.channel}`);
      if (!a || !b) return;
      this.hits.ends.push({ cable: c, x: b.x, y: b.y, from: a });
      const path = svg('path', { class: `cable${c.dashed ? ' dashed' : ''}`, d: cablePath(a, b, i) });
      path.append(svg('title', {}, `${c.pin} → ${c.port} (${c.endpoint}) → channel ${c.channel}`));
      groups.cables.append(path);
    });
    if (this.layout.tray) {
      const t = this.layout.tray;
      groups.boxes.append(svg('rect', { class: 'tray', x: t.x, y: t.y, width: t.w, height: t.h, rx: 8 }));
      groups.boxes.append(svg('text', { class: 'box-kind', x: t.x + 24, y: t.y + 42 }, 'UNSTACKED'));
      this.model.unstacked.forEach((name, i) => groups.boxes.append(svg('text', { class: 'tray-text', x: t.x + 24, y: t.y + BOX_PU.title * PU + (i + 0.75) * TRAY_LINE_PU * PU }, fitText(name, capChars(TEXT.port, t.w - 48)))));
    }
    this.ghost = svg('path', { class: 'cable ghost', d: '' });
    this.setupSvg.replaceChildren(groups.cables, groups.boxes, groups.pins, this.ghost);
  }

  // Setup-level cables, dragged (ADR 0051 §3, §9).

  /** What a press at plane point `p` grabs: a cable's device end (move), or a Stack's out pin (create); null for neither. */
  setupCableAt(p) {
    const reach = 18 / this.view.k;
    const near = (h) => Math.hypot(h.x - p.x, h.y - p.y) <= reach;
    const end = this.hits?.ends.find(near);
    if (end) return { kind: 'move', cable: end.cable, from: end.from };
    const hit = this.hits?.pins.find(near);
    if (!hit) return null;
    const siblings = (this.model.pins.get(hit.stack) ?? []).filter((q) => q.side === 'out' && q !== hit.pin && q.instrument === hit.pin.instrument);
    return { kind: 'create', pin: hit.pin, siblings, from: { x: hit.x, y: hit.y } };
  }

  /** The playback port under plane point `p` and the jack nearest it: `{box, port, channel, rect}`, or null. */
  setupPortAt(p) {
    const hit = this.hits?.ports.find((h) => p.x >= h.x && p.x <= h.x + h.w && p.y >= h.y && p.y <= h.y + h.h);
    if (!hit) return null;
    const jack = hit.jacks.reduce((best, j) => (!best || Math.abs(j.y - p.y) < Math.abs(best.y - p.y) ? j : best), null);
    return { box: hit.box, port: hit.port, channel: jack?.channel ?? hit.port.channels[0], rect: hit.rect };
  }

  /** The dragged cable drawn to plane point `p`, the port under it marked. */
  dragSetupCable(p) {
    const d = this.cableDrag;
    d.to = p;
    this.ghost?.setAttribute('d', cablePath(d.from, p));
    const target = this.setupPortAt(p);
    if (d.target?.rect !== target?.rect) {
      d.target?.rect?.classList.remove('target');
      target?.rect?.classList.add('target');
    }
    d.target = target;
  }

  /** The drag let go: what it writes (`cableDrop`), then written: the project's text by `patch`, the binding by `device.route`. */
  async dropSetupCable(event) {
    const d = this.cableDrag;
    this.cableDrag = null;
    d.target?.rect?.classList.remove('target');
    this.ghost?.setAttribute('d', '');
    if (!d.moved) return;
    const drop = d.target ? { box: d.target.box, port: d.target.port, channel: d.target.channel } : null;
    const instances = (this.graph?.top?.instances ?? []).map((i) => i.name);
    const result = cableDrop(d, drop, { bindings: this.model.bindings, instances, main: this.model.main, devices: this.devices, scope: event.shiftKey ? 'project' : 'machine' });
    if (!result.ops.length && !result.route) {
      if (result.note) this.say(result.note);
      return;
    }
    try {
      await this.writeSetupCable(result);
      if (result.note) this.say(result.note);
    } catch (error) {
      this.say(error?.message ?? String(error));
    }
  }

  /**
   * Writes a cable: its project text (one patch of project.toml), then its
   * binding. A new output needs the run to start again (ADR 0038 v1: a run
   * keeps the outputs it started with): refused while playing, it is
   * written stopped and played again.
   */
  async writeSetupCable({ ops, route }) {
    // ADR 0049: of the opening the graph shown is of.
    const patch = () => this.request(named({ type: 'patch', level: '', base: this.graph?.files?.[''] ?? null, ops }, graphGeneration(this.graph)));
    let restart = false;
    try {
      if (ops.length) {
        try {
          await patch();
        } catch (error) {
          if (!/outputs must stay/.test(error?.message ?? '')) throw error;
          await this.request({ type: 'transport.stop' });
          restart = true;
          await patch();
        }
      }
      if (route) {
        await this.request(route).catch((error) => {
          // Refused: what the Setup holds now is read again (a saved one comes as the `setup` push).
          this.readSetup();
          throw error;
        });
      }
    } finally {
      if (restart) await this.request({ type: 'transport.play' }).catch((error) => this.say(error?.message ?? String(error)));
    }
  }

  /** The list of other devices (plane units): its head ("Other devices (N)", a click or Enter toggles it), and, expanded, a small box per device. */
  drawOthers(r) {
    const others = this.model.others;
    const g = svg('g', { 'data-others': '' });
    g.append(svg('rect', { class: 'others', x: r.x, y: r.y, width: r.w, height: r.h, rx: 10 }));
    const head = svg('g', { class: 'others-head', tabindex: 0, role: 'button', 'aria-expanded': String(this.othersOpen), 'data-others-toggle': '' });
    head.append(svg('rect', { x: r.x, y: r.y, width: r.w, height: OTHERS_PU.head * PU, rx: 10 }));
    head.append(svg('text', { class: 'box-title', x: r.x + 24, y: r.y + 52 }, `${this.othersOpen ? '▾' : '▸'} Other devices (${others.length})`));
    head.append(svg('title', {}, `${this.othersOpen ? 'Collapse' : 'Expand'} the devices nothing in this project uses: ${others.map((b) => b.name).join(', ')}`));
    head.addEventListener('click', () => this.toggleOthers());
    g.append(head);
    othersBoxes({ count: others.length, expanded: this.othersOpen }).forEach((s, i) => {
      const b = others[i];
      const x = r.x + s.x;
      const y = r.y + s.y;
      const box = svg('g', { 'data-device': b.key });
      box.append(svg('rect', { class: 'small', x, y, width: s.w, height: s.h, rx: 6 }));
      const name = svg('text', { class: 'small-name', x: x + 20, y: y + 46 }, fitText(b.name, capChars(TEXT.small, s.w - 40)));
      name.append(svg('title', {}, b.name));
      box.append(name, svg('text', { class: 'small-kind', x: x + 20, y: y + 82 }, channelSummary(b)));
      g.append(box);
    });
    return g;
  }

  /** The list of other devices expanded or collapsed, kept per viewer (try/catch: storage may be refused). */
  toggleOthers() {
    this.othersOpen = !this.othersOpen;
    try {
      globalThis.localStorage?.setItem(OTHERS_KEY, this.othersOpen ? 'open' : 'closed');
    } catch {
      // Not kept: this page's view only.
    }
    this.relayout();
    this.shadowRoot?.querySelector?.('[data-others-toggle]')?.focus?.();
  }

  // The view.

  measure() {
    const r = this.stage?.getBoundingClientRect?.();
    if (!r) return;
    const w = Math.round(r.width);
    const h = Math.round(r.height);
    if (w === this.viewport.w && h === this.viewport.h) return;
    const was = this.viewport;
    this.viewport = { w, h };
    if (!(w > 0)) return;
    // Shown again (a tab): the panels are measured now.
    if (!was.w) this.relayoutSoon();
    if (!this.layout) return;
    if (!this.fitted || !this.touched) {
      this.fitted = true;
      this.view = fitAll(this.layout.bounds, this.viewport);
    } else if (Math.abs(this.scaleRatio() - 1) < 1e-6 || this.level() === 'stack') {
      const rect = this.focusRect();
      if (rect) this.view = fitWidth(rect, this.viewport, this.view);
    }
    this.render();
  }

  /** At the Setup level the focus follows the pointer: the Stack under screen point `at`, else the nearest. */
  focusUnder(at) {
    if (!this.layout?.stacks.size) return;
    const p = toPlane(this.view, at);
    let best = null;
    for (const [name, r] of this.layout.stacks) {
      const dx = Math.max(r.x - p.x, 0, p.x - (r.x + r.w));
      const dy = Math.max(r.y - p.y, 0, p.y - (r.y + r.h));
      const d = Math.hypot(dx, dy);
      if (!best || d < best.d) best = { name, d };
    }
    if (best && best.name !== this.focused) {
      this.focused = best.name;
      this.barKey = null;
    }
  }

  /** The focused Stack's rectangle (plane units), or null. */
  focusRect() {
    return (this.focused && this.layout?.stacks.get(this.focused)) || null;
  }

  /** The focused Stack's instrument rectangles on the plane (plane units), for the instrument that opens. */
  instrumentRects() {
    const rect = this.focusRect();
    const s = this.stacks.get(this.focused);
    if (!rect || !s?.layout) return [];
    return s.layout.rects.map((r) => ({ entry: r.entry, x: rect.x + r.x, y: rect.y + r.y, w: r.w, h: r.h }));
  }

  /** The focused Stack's fit scale (the zoom's reference), or the whole plane's. */
  fitK() {
    const rect = this.focusRect();
    return rect && this.viewport.w ? fitScale(rect, this.viewport) : null;
  }

  /** The view's depth (`depthOf`): its scale over the focused Stack's fit, plus the lift at the fit. Every level and fade is a function of it. */
  ratio(view = this.view, lift = this.lift) {
    const fit = this.fitK();
    return fit ? depthOf(view.k / fit, lift) : 0;
  }

  /** The view's scale over the focused Stack's fit alone. */
  scaleRatio(view = this.view) {
    const fit = this.fitK();
    return fit ? view.k / fit : 0;
  }

  /** The opened instrument's engine band (`engineBand`, from its cube as laid out; the default until it is). */
  engineBandNow() {
    // Below the control band the default band does: no Control plane is laid out before it first shows.
    // A chosen layer draws each drawing wholly in or out: no cube decides a band there.
    if (this.layer || this.ratio() < CONTROL_BAND[0]) return ENGINE_BAND;
    const cube = this.controlNow()?.layout?.cube;
    const fit = this.fitK();
    if (!cube || !fit || !this.viewport.w) return ENGINE_BAND;
    const cubeFit = Math.min((this.viewport.w - 2 * MARGIN_PX) / cube.w, (this.viewport.h - 2 * MARGIN_PX) / cube.h);
    return engineBand(depthOf(cubeFit / fit, LIFT));
  }

  level(view = this.view) {
    if (this.layer) return this.layer;
    return levelOf(this.ratio(view), this.engineBandNow());
  }

  /** The depth the fades are drawn at: the zoom's, or the chosen layer's own (each drawing wholly in or out). */
  fadeDepth() {
    if (!this.layer) return this.ratio();
    if (this.layer === 'setup') return 0;
    if (this.layer === 'stack') return 1;
    if (this.layer === 'wiring') return WIRING_BAND[1];
    if (this.layer === 'control') return CONTROL_BAND[1];
    return this.engineBandNow()[1];
  }

  /**
   * The opacities drawn now (`planeFades` at `fadeDepth`). At the Engine
   * layer chosen alone only the Engine drawing shows in the focused Stack:
   * the front panels, the Wiring and the Control drawings are out.
   */
  fadesNow() {
    const fades = planeFades(this.fadeDepth(), this.engineBandNow());
    return this.layer === 'engine' ? { ...fades, panels: 0, controls: 0, board: 0, openWiring: 0, sheet: 0, deep: 1 } : fades;
  }

  /** The person (or a command) moved the view: from now on it is theirs, never fitted again by itself. */
  setView(view) {
    this.stopTravel();
    this.touched = true;
    this.view = view;
    this.render();
  }

  /**
   * Dev-only, for measuring (ADR 0051 test 9: at most 5 000 drawn elements; docs/benchmarks.md):
   * what the plane holds and draws now. `elements`: every element under it, the front panels'
   * and controls' shadow roots included; `drawn`: those shown (not `display: none`, `[hidden]` or
   * `visibility: hidden`, under no zero opacity); both per layer (`setup`, each Stack by name,
   * `chrome` the bar and notes; the Engine drawing is its Stack's). `renders` and `renderMs`: the
   * view's renders since the plane was made, and their script time. Reads computed styles,
   * so it is called by a measurement, never per frame. Never writes anything. An order of
   * magnitude, not an exact count: light-DOM children of a shadow host count whether slotted
   * or not, and overlays on the stage (the chooser, Edit layout) are in no layer.
   */
  metrics() {
    const walk = (node, shown, out) => {
      for (const child of node.children ?? []) {
        let on = shown;
        if (on) {
          const style = getComputedStyle(child);
          on = style.display !== 'none' && !child.hidden && Number(style.opacity) > 0;
          if (on && style.visibility === 'hidden') {
            out.elements++;
            if (child.shadowRoot) walk(child.shadowRoot, on, out);
            walk(child, on, out);
            continue;
          }
        }
        out.elements++;
        if (on) out.drawn++;
        if (child.shadowRoot) walk(child.shadowRoot, on, out);
        walk(child, on, out);
      }
      return out;
    };
    const layer = (element) => {
      if (!element) return { elements: 0, drawn: 0 };
      const style = getComputedStyle(element);
      const on = style.display !== 'none' && !element.hidden && Number(style.opacity) > 0 && style.visibility !== 'hidden';
      return walk(element, on, { elements: 1, drawn: on ? 1 : 0 });
    };
    const layers = { setup: layer(this.setupSvg), chrome: { elements: 0, drawn: 0 } };
    for (const part of [this.bar, this.notes]) {
      const counted = layer(part);
      layers.chrome.elements += counted.elements;
      layers.chrome.drawn += counted.drawn;
    }
    const stacks = {};
    for (const [name, s] of this.stacks) stacks[name] = layer(s.element);
    const all = [layers.setup, layers.chrome, ...Object.values(stacks)];
    return {
      level: this.level(),
      view: { ...this.view },
      focused: this.focused,
      opened: this.opened,
      elements: all.reduce((n, c) => n + c.elements, 0),
      drawn: all.reduce((n, c) => n + c.drawn, 0),
      layers: { ...layers, stacks },
      renders: this.renders ?? 0,
      renderMs: Math.round((this.renderMs ?? 0) * 1000) / 1000,
    };
  }

  /** The view drawn: one transform, the fades of its ratio; each level's drawing laid out once, when it first shows. */
  render() {
    const t0 = performance.now();
    this.drawView();
    this.renders = (this.renders ?? 0) + 1;
    this.renderMs = (this.renderMs ?? 0) + performance.now() - t0;
  }

  /** The zoom came to rest: canvases draw again at the scale now seen. */
  settleLater() {
    // No compositor layer, not even while zooming (the owner's, 2026-11-02): every frame is drawn at the scale it is
    // seen at, vectors and text sharp. Canvases (the piano roll) draw again once the zoom rests (`signal-rescale`).
    clearTimeout(this.settleTimer);
    this.settleTimer = setTimeout(() => {
      if (typeof Event === 'function') document.dispatchEvent?.(new Event('signal-rescale'));
    }, 150);
  }

  drawView() {
    if (!this.world || !this.viewport.w) return;
    const v = this.view;
    const transform = `translate(${v.x}px, ${v.y}px) scale(${v.k})`;
    if (this.world.style.transform !== transform) {
      this.world.style.transform = transform;
      // A layer only while the scale changes: a pan alone moves the drawing as drawn, sharp.
      if (v.k !== this.drawnK) this.settleLater();
      this.drawnK = v.k;
    }
    this.renderText(v.k);
    const r = this.fadeDepth();
    const fades = this.fadesNow();
    const level = this.layer ?? levelOf(r, this.engineBandNow());
    this.dataset.level = level;
    opacity(this.setupSvg, fades.setup);
    for (const [name, s] of this.stacks) {
      const focused = name === this.focused;
      opacity(s.element, focused ? 1 : fades.setup);
      // The focused Stack's front panels, card by card: the opened instrument's live controls come back at Control (x-ray).
      for (const [entry, card] of s.cards) {
        // At Control every instrument's live controls show over its sheet; deeper, the opened one's.
        const opened = focused && (entry === this.opened || fades.sheet > 0);
        const o = !focused ? 1 : opened ? fades.controls : fades.panels;
        opacity(card, o);
        const hidden = o < 0.002 ? 'hidden' : '';
        if (card.style.visibility !== hidden) {
          card.style.visibility = hidden;
          card.toggleAttribute('data-faded', Boolean(hidden));
        }
        // Faded to the surround (the Engine under it), it takes no press: a click goes to the wire under it.
        const inert = o < HIT_OPACITY;
        if (card.inert !== inert) {
          card.inert = inert;
          card.style.pointerEvents = inert ? 'none' : '';
        }
        const xray = opened && fades.control > 0;
        if (card.hasAttribute('data-xray') !== xray) card.toggleAttribute('data-xray', xray);
      }
      if (!focused) {
        hide(s.wiringSvg);
        hide(s.controlSvg);
        hide(s.engineSvg);
      }
    }
    this.renderWiring(fades);
    this.renderControl(fades);
    this.renderEngine(fades);
    this.renderBar(level, this.layer ? this.scaleRatio() : r, level === 'engine' ? this.chipsNow() : []);
  }

  /** The Setup level's text sizes at scale `k` (`textSizes`: readable at the Setup's fit, never past the room the layout gives them), as custom properties every level's text reads. */
  renderText(k) {
    const sizes = textSizes(k);
    const key = Object.values(sizes).map((n) => n.toFixed(2)).join(' ');
    if (key === this.textKey) return;
    this.textKey = key;
    for (const [name, size] of Object.entries(sizes)) this.world.style.setProperty(`--t-${name}`, `${size.toFixed(2)}px`);
    // The Wiring level's text under the same rule: never below its least size on screen.
    for (const [name, rule] of Object.entries(WIRING_TEXT)) this.world.style.setProperty(`--w-${name}`, `${textSize(rule, k).toFixed(2)}px`);
  }

  // The Wiring level: the focused Stack from behind (ui/lib/plane-wiring.js), laid out when it first shows, kept per graph and layout.

  /** The focused Stack's Wiring model (`wiringModel`: each instrument's ports and the cables between them), once per graph, Stack layout and ownership. */
  wiringModelNow(s, stack) {
    const pins = this.model.pins.get(stack.name) ?? [];
    // What it depends on: the structure, the Stack's layout, and who holds what (the panel's binds, the Stacks).
    const key = `${this.graphKey}\n${layoutKey(s.layout)}\n${s.key}\n${JSON.stringify([[...this.model.owner], [...this.model.stackOf], pins])}`;
    if (s.wiringModel?.key === key) return s.wiringModel;
    // Only the instruments laid out (measured): an entry without a rectangle has no ports to draw.
    const laid = { ...stack, instruments: s.layout.rects.map((r) => r.entry) };
    const model = wiringModel(laid, this.model.owner, this.model.stackOf, this.graph?.top?.instances ?? [], this.graph?.top?.connections ?? [], pins);
    s.wiringModel = { key, model, pins };
    return s.wiringModel;
  }

  /**
   * The focused Stack's engine packages (ui/lib/plane-package.js, the owner's decision of 2026-11-03): each instrument's
   * audio engine as one package, at the same place with the same pins on the Wiring, Control and Engine layers, which all
   * draw these. Laid out once per graph, Stack layout, ownership, panel and titles (the controls measured then, once:
   * a layout read only when the key changes, never per frame); `{key, map: Map(entry → package), fronts: Map(entry →
   * {controls, blocked, autos, inner, measured})}`, or null.
   */
  packagesNow() {
    const stack = this.model?.plane.stacks.find((x) => x.name === this.focused);
    const s = this.stacks.get(this.focused);
    if (!stack || !s?.layout || !this.graph) return null;
    // Once per frame (every layer and hit test asks): the key is built again only when the frame or what it reads changed.
    const memo = { frame: this.renders ?? 0, model: this.model, graph: this.graph, layout: s.layout, stackKey: s.key, cards: this.cards };
    if (s.packages && s.packagesMemo && Object.keys(memo).every((name) => s.packagesMemo[name] === memo[name])) return s.packages;
    s.packagesMemo = memo;
    const wiring = this.wiringModelNow(s, stack);
    const titles = s.layout.rects.map((r) => instrumentTitle(r.entry, this.cards));
    const key = `${wiring.key}\n${titles.join('\n')}`;
    if (s.packages?.key === key) return s.packages;
    const map = new Map();
    const fronts = new Map();
    s.layout.rects.forEach((r, i) => {
      const front = this.measureFront(s.cards.get(r.entry), r, r.entry);
      const ports = wiring.model.ports.get(r.entry) ?? { ins: [], outs: [] };
      fronts.set(r.entry, front);
      map.set(r.entry, packageOf({ rect: { x: r.x, y: r.y, w: r.w, h: r.h }, controls: front.controls, blocked: front.blocked, engine: engineOf(r.entry, this.model.owner), autos: front.autos, inner: front.inner, ins: ports.ins, outs: ports.outs, title: titles[i] }));
    });
    // A card not laid out yet (measured null) is measured again on the next frame: its key says so, so nothing is kept for it.
    const unmeasured = [...fronts].filter(([, f]) => !f.measured).map(([entry]) => entry);
    s.packages = { key: unmeasured.length ? `${key}\nunmeasured ${unmeasured.join(' ')}` : key, map, fronts };
    return s.packages;
  }

  /** An instrument's front panel as its package and its Control plane need it: its controls measured, their automation and inner values. */
  measureFront(card, r, entry) {
    const measured = card ? this.measureControls(card, r) : null;
    const controls = measured?.controls ?? [];
    const binds = controls.filter((c) => c.writes).flatMap((c) => c.binds);
    const inner = new Map();
    for (const b of binds) {
      const a = innerAddress(b, this.graph);
      if (a) inner.set(b, a);
    }
    return { measured: Boolean(measured), controls, blocked: measured?.blocked ?? [], autos: automations({ entry, binds, owner: this.model.owner, graph: this.graph }), inner };
  }

  /** The focused Stack's Wiring drawing, or null (laid out and drawn once per graph, Stack layout and packages). */
  wiringNow() {
    const s = this.stacks.get(this.focused);
    const packages = this.packagesNow();
    if (!packages || !s?.wiringModel) return null;
    if (s.wiring?.key === packages.key) return s.wiring;
    const t0 = performance.now();
    const { model, pins } = s.wiringModel;
    const places = pinPlaces(pins, s.layout.rects);
    const pinYs = new Map(places.map((p) => [`${p.side} ${p.address}`, p.y]));
    const layout = wiringLayout(model, s.layout, pinYs, packages.map);
    s.wiring = { key: packages.key, layout, pins: places, packages: packages.map, ms: performance.now() - t0 };
    this.drawWiring(s, s.wiring);
    return s.wiring;
  }

  /**
   * An engine package drawn (ui/lib/plane-package.js `packageOf`'s, plane units), the same at every layer (`layer` its
   * class): its body, its title band, its signal pins and their labels, each with a hover title (`note` the body's).
   * `{g, pins: Map(id → element), labels, pinsGroup}`: an inside drawn in it goes before `labels` (under the pins).
   */
  drawPackage(pkg, layer, note) {
    const g = svg('g', { class: `pkg pkg-${layer}` });
    const body = svg('rect', { class: 'pkg-body', x: pkg.x, y: pkg.y, width: pkg.w, height: pkg.h, rx: 6 });
    body.append(svg('title', {}, note));
    g.append(body);
    // The title band and its text in proportion to a shrunk package's (11 plane units in a full one).
    const inset = Math.min(4, pkg.head.h / 6);
    const size = Math.min(11, pkg.head.h * 0.45);
    g.append(svg('rect', { class: 'pkg-top', x: pkg.x + inset, y: pkg.y + inset, width: Math.max(0, pkg.w - 2 * inset), height: Math.max(0, pkg.head.h - 2 * inset), rx: 3 }));
    g.append(svg('text', { class: 'pkg-title', x: pkg.x + 3 * inset, y: pkg.y + pkg.head.h / 2 + size * 0.36, 'font-size': size.toFixed(2) }, fitText(`ENGINE · ${pkg.names.join(', ') || 'no instances'}`, Math.max(4, Math.floor((pkg.w - 6 * inset) / (size * 0.64))))));
    const labels = svg('g', { class: 'pkg-labels' });
    const pinsGroup = svg('g', { class: 'pkg-pins' });
    const pins = new Map();
    // Every pin by the port it stands for: a selection's net marks it (`markSelection`).
    const marks = [];
    // A shrunk package's pins closer: their dots and labels in proportion (a full one's pitch is 24).
    const radius = Math.min(5, pkg.pitch * 0.38);
    const small = pkg.pitch < 20 ? { style: `font-size: ${(pkg.pitch * 0.7).toFixed(2)}px` } : {};
    for (const p of [...pkg.ins, ...pkg.outs]) {
      const dot = svg('circle', { class: `pkg-pin k-${p.kind}${p.connected === false ? '' : ' connected'}`, cx: p.x, cy: p.y, r: +radius.toFixed(2), 'data-id': p.id });
      dot.append(svg('title', {}, `${p.address} (${p.kind} ${p.side === 'in' ? 'input' : 'output'}): a pin of ${pkg.title}’s engine`));
      pinsGroup.append(dot);
      pins.set(p.id, dot);
      dot.netAddress = p.address;
      marks.push(dot);
      labels.append(svg('text', { class: 'pkg-pin-label', x: p.side === 'in' ? p.x + 2 * radius : p.x - 2 * radius, y: p.y + 4, 'text-anchor': p.side === 'in' ? 'start' : 'end', ...small }, p.label));
    }
    // Its control pins at every layer (the owner's, 2026-11-04): the bound parameters on its connector, the automation
    // sources under its outputs. The Control layer draws its own over them, with its legs and wires and their hit tests.
    if (layer !== 'control') {
      const small = Math.min(3.5, radius * 0.7);
      for (const p of pkg.params) {
        const dot = svg('circle', { class: 'pkg-param', cx: p.x, cy: p.y, r: +small.toFixed(2) });
        dot.append(svg('title', {}, `${p.address}: a bound parameter of ${pkg.title}’s engine (its control at the Control layer)`));
        pinsGroup.append(dot);
        Object.assign(dot, { netAddress: p.address, netOnly: true });
        marks.push(dot);
      }
      for (const p of pkg.autos) {
        const dot = svg('circle', { class: 'pkg-auto', cx: p.x, cy: p.y, r: +(radius * 0.8).toFixed(2) });
        dot.append(svg('title', {}, `${p.address}: automation out of ${pkg.title}’s engine into a control`));
        pinsGroup.append(dot);
        Object.assign(dot, { netAddress: p.address, netOnly: true });
        marks.push(dot);
      }
    }
    g.append(labels, pinsGroup);
    return { g, pins, labels, pinsGroup, marks };
  }

  drawWiring(s, wiring) {
    const { layout } = wiring;
    const root = svg('g', { transform: 'translate(-2 -2)' });
    const cables = svg('g', { class: 'w-cables' });
    const elements = new Map();
    const items = [];
    s.wiringGroups = new Map();
    s.netMarks = { ...s.netMarks, wiring: [] };
    // Bottom up: the instruments' faint boxes, the cables (a cable runs into its box to its pin), the packages over them.
    const boxes = svg('g', { class: 'w-boxes' });
    const packages = svg('g', { class: 'w-packages' });
    // The packages' pins by port id: a port is its package's pin (the same at Control and the Engine).
    const pinElements = new Map();
    for (const b of layout.boxes) {
      const g = svg('g', { 'data-entry': b.entry });
      const front = svg('g', { 'data-entry': b.entry });
      const rect = svg('rect', { class: 'w-box', x: b.x, y: b.y, width: b.w, height: b.h, rx: 6 });
      g.append(rect, svg('text', { class: 'w-title', x: b.x + 16, y: b.y + 26 }, instrumentTitle(b.entry, this.cards).toUpperCase()));
      const pkg = wiring.packages.get(b.entry);
      if (pkg) {
        const drawn = this.drawPackage(pkg, 'wiring', `${pkg.title}’s audio engine: its cables end on its pins (the same at Control and the Engine; zoom in at the Engine to see its inside)`);
        front.append(drawn.g);
        for (const [id, element] of drawn.pins) pinElements.set(id, element);
        s.netMarks.wiring.push(...drawn.marks);
      }
      s.wiringGroups.set(b.entry, [g, front]);
      boxes.append(g);
      packages.append(front);
      const id = `box ${b.entry}`;
      elements.set(id, rect);
      items.push({ kind: 'box', id, entry: b.entry, x: b.x, y: b.y, w: b.w, h: b.h });
    }
    for (const c of layout.cables) {
      const path = svg('path', { class: `w-cable k-${c.kind}`, d: polylinePath(c.points, 6, CORNERS), 'data-id': c.id });
      path.append(svg('title', {}, `${c.from} → ${c.to} (${c.kind})${c.b.edge || c.a.edge ? ', through the Stack’s edge' : ''}`));
      cables.append(path);
      for (const j of c.junctions) cables.append(svg('circle', { class: `w-junction k-${c.kind}`, cx: j.x, cy: j.y, r: 3.5 }));
      elements.set(c.id, path);
      items.push({ kind: 'cable', id: c.id, cable: c, points: c.points });
    }
    root.append(boxes, cables, packages);
    for (const p of layout.ports) {
      let dot = pinElements.get(p.id);
      if (!dot) {
        // No package pin for it (an instrument whose package is not laid out): a port on the box's edge.
        const g = s.wiringGroups.get(p.entry)?.[1] ?? root;
        dot = svg('circle', { class: `w-port k-${p.kind}`, cx: p.x, cy: p.y, r: 5, 'data-id': p.id });
        dot.append(svg('title', {}, `${p.address} (${p.kind} ${p.side === 'in' ? 'input' : 'output'})`));
        g.append(dot, svg('text', { class: 'w-port-label', x: p.side === 'in' ? p.x + 10 : p.x - 10, y: p.y + 4, 'text-anchor': p.side === 'in' ? 'start' : 'end' }, p.label));
      }
      elements.set(p.id, dot);
      items.push({ kind: 'port', id: p.id, port: p, x: p.x, y: p.y });
    }
    // The Stack's edge pins: where the Setup level's cables leave (ADR 0051 §7), at the rectangle's edge.
    for (const pin of wiring.pins) {
      const x = pin.side === 'out' ? layout.bounds.w : 0;
      const id = `edge ${pin.side} ${pin.address}`;
      const dot = svg('circle', { class: `w-edge ${pin.side}`, cx: x, cy: pin.y, r: 6 });
      dot.append(svg('title', {}, `the Stack’s edge: ${pin.address} ${pin.side === 'out' ? '→' : '←'} ${pin.outside.join(', ')}`));
      root.append(dot);
      elements.set(id, dot);
      items.push({ kind: 'edge', id, pin, x, y: pin.y });
    }
    s.wiringSvg.replaceChildren(root);
    s.wiringItems = items;
    s.wiringElements = elements;
    s.wiringShown = null;
    this.markSelection();
  }

  renderWiring(fades) {
    const s = this.stacks.get(this.focused);
    if (!s) return;
    if (!(fades.board > 0 || fades.openWiring > 0)) return hide(s.wiringSvg);
    const wiring = this.wiringNow();
    if (!wiring) return hide(s.wiringSvg);
    show(s.wiringSvg);
    for (const [entry, groups] of s.wiringGroups) for (const g of groups) opacity(g, entry === this.opened ? fades.openWiring : fades.board);
    opacity(s.wiringSvg.querySelector('.w-cables'), fades.board);
    this.cull(s, s.wiringItems, s.wiringElements, 'wiringShown');
  }

  /** Culling: the items of a drawing off the stage are hidden (their elements kept), recomputed only when what is on it changes. */
  cull(s, items, elements, key) {
    const rect = this.focusRect();
    if (!rect || !items) return;
    const a = toPlane(this.view, { x: 0, y: 0 });
    const b = toPlane(this.view, { x: this.viewport.w, y: this.viewport.h });
    const shown = visibleIds(items, { x: a.x - rect.x, y: a.y - rect.y, w: b.x - a.x, h: b.y - a.y }, 24 / this.view.k);
    const before = s[key];
    for (const item of items) {
      const on = shown.has(item.id);
      if (before && before.has(item.id) === on) continue;
      const element = elements.get(item.id);
      if (element) element.style.display = on ? '' : 'none';
    }
    s[key] = shown;
  }

  // The Control level: the opened instrument's control plane (ui/lib/plane-control.js), laid out when it first opens.

  /** The opened instrument's Control plane, or null. */
  controlNow() {
    return this.controlFor(this.opened);
  }

  /** Every instrument's Control plane in the focused Stack (the owner's, 2026-11-02: Control shows them all), drawn again when one changed. */
  controlsNow() {
    const s = this.stacks.get(this.focused);
    if (!s?.layout) return [];
    const all = s.layout.rects.map((r) => this.controlFor(r.entry)).filter(Boolean);
    const key = all.map((c) => c.key).join('\n\n');
    if (s.controlDrawn !== key) {
      s.controlDrawn = key;
      this.drawControls(s, all);
    }
    return all;
  }

  /** Instrument `entry`'s Control plane, or null (laid out once per graph, panel, Stack layout and instrument). */
  controlFor(entry) {
    const s = this.stacks.get(this.focused);
    const r = s?.layout?.rects.find((x) => x.entry === entry);
    if (!entry || !r || !s.cards.get(entry)) return null;
    const packages = this.packagesNow();
    const pkg = packages?.map.get(entry);
    const front = packages?.fronts.get(entry);
    if (!pkg || !front?.measured) return null;
    s.controls ??= new Map();
    const known = s.controls.get(entry);
    const key = `${packages.key}\n${entry}`;
    if (known?.key === key) return known;
    const t0 = performance.now();
    const wiring = this.wiringNow();
    // A cable from outside enters level with the Wiring level's port of the same connection (its package's pin).
    const edgeYs = new Map((wiring?.layout.ports ?? []).filter((p) => p.entry === entry && p.side === 'in').map((p) => [p.address, p.y]));
    // The package is the one the Wiring and the Engine draw: the same place, the same pins.
    const layout = controlLayout({ rect: { x: r.x, y: r.y, w: r.w, h: r.h }, controls: front.controls, blocked: front.blocked, engine: pkg.names, autos: front.autos, edgeYs, inner: front.inner, title: pkg.title, pkg });
    const control = { key, entry, layout, controls: front.controls, autos: front.autos, ms: performance.now() - t0 };
    s.controls.set(entry, control);
    return control;
  }

  /**
   * The opened card's controls measured on the plane (plane units,
   * Stack-relative), from its live elements: `{controls, blocked}`, or null
   * while it is not laid out. A function of the panel's layout, never of
   * the zoom (the screen rectangles are divided by the card's own scale).
   */
  measureControls(card, r) {
    const cr = card.getBoundingClientRect();
    const k = cr.width / Math.max(1e-9, r.panel.w);
    if (!(k > 0) || !(cr.width > 0)) return null;
    const at = (e) => {
      const er = e.getBoundingClientRect();
      return { x: r.panel.x + (er.left - cr.left) / k, y: r.panel.y + (er.top - cr.top) / k, w: er.width / k, h: er.height / k };
    };
    const controls = [];
    for (const e of card.querySelectorAll('[bind]')) {
      const type = e.localName.replace(/^signal-/, '');
      if (!e.localName.startsWith('signal-') || NOT_CONTROLS.has(type)) continue;
      const binds = bindList(e.getAttribute('bind'));
      if (!binds.length) continue;
      controls.push({ id: `control ${controls.length}`, element: e, tag: e.localName, type, binds, writes: !READERS.has(type), label: e.getAttribute('label') ?? '', rect: at(e) });
    }
    // A heading blocks its text, not the whole width its block takes (the engine package may stand beside it), and room
    // after it for the Wiring's and the Engine's instrument titles, drawn there a little further right.
    const heading = (h) => {
      const range = document.createRange?.();
      range?.selectNodeContents(h);
      const tr = range?.getBoundingClientRect?.();
      if (!(tr?.width > 0)) return at(h);
      const box = at(h);
      return { x: box.x, y: box.y, w: Math.min(box.w, (tr.right - cr.left) / k + r.panel.x - box.x + 8 * PU), h: box.h };
    };
    const blocked = [...card.querySelectorAll(':scope > h1, :scope > h2')].map(heading);
    return { controls, blocked };
  }

  /** The Stack's Control drawing: each instrument's plane, its items' ids led by its entry. */
  drawControls(s, controls) {
    const root = svg('g', { transform: 'translate(-2 -2)' });
    s.controlItems = [];
    s.controlElements = new Map();
    s.readouts = new Map();
    s.netMarks = { ...s.netMarks, control: [] };
    for (const control of controls) {
      const one = this.drawControl(control);
      s.netMarks.control.push(...one.marks);
      root.append(one.root);
      for (const item of one.items) s.controlItems.push({ ...item, id: `${control.entry}|${item.id}`, entry: control.entry });
      for (const [id, element] of one.elements) s.controlElements.set(`${control.entry}|${id}`, element);
      for (const [id, ro] of one.readouts) s.readouts.set(`${control.entry}|${id}`, ro);
    }
    // An arrowhead per kind, its colour written out: WebKit draws a marker's \`context-stroke\` black.
    const defs = svg('defs');
    for (const [kind, colour] of Object.entries(ARROW_COLOURS)) {
      const marker = svg('marker', { id: `plane-arrow-${kind}`, viewBox: '0 0 8 8', refX: 7, refY: 4, markerWidth: 6, markerHeight: 6, orient: 'auto-start-reverse' });
      marker.append(svg('path', { d: 'M0 0 L8 4 L0 8 z', fill: colour }));
      defs.append(marker);
    }
    s.controlSvg.replaceChildren(defs, root);
    s.controlShown = null;
    this.markSelection();
  }

  /** One instrument's Control plane drawn: its group, hit items, elements by id and readouts. */
  drawControl(control) {
    const { layout } = control;
    const root = svg('g');
    const r = layout.rect;
    const elements = new Map();
    const items = [];
    root.append(svg('rect', { class: 'c-sheet', x: r.x, y: r.y, width: r.w, height: r.h, rx: 4 }));
    // Each bound control's footprint on the sheet (the owner's, 2026-11-02): what its leg leaves from, as the grabbed outline.
    const legged = new Set(layout.legs.map((l) => l.control));
    for (const c of control.controls) {
      if (!legged.has(c.id) || !c.rect) continue;
      root.append(svg('rect', { class: 'c-footprint', x: c.rect.x - 3, y: c.rect.y - 3, width: c.rect.w + 6, height: c.rect.h + 6, rx: 7 }));
    }
    // The legs run bare in their band, no ribbon box over them (the owner's, 2026-11-04).
    for (const l of layout.legs) {
      const path = svg('path', { class: `c-leg${l.reads ? ' reads' : ''}`, d: polylinePath(l.points, 3, CORNERS) });
      path.append(svg('title', {}, `${control.controls.find((c) => c.id === l.control)?.label || l.address} ⇢ ${l.address}`));
      root.append(path);
      elements.set(l.id, path);
      items.push({ kind: 'leg', id: l.id, leg: l, points: l.points });
      const via = svg('circle', { class: 'c-via', cx: l.via.x, cy: l.via.y, r: 2.5 });
      root.append(via);
      const viaId = `via ${l.control}`;
      elements.set(viaId, via);
      items.push({ kind: 'via', id: viaId, control: l.control, x: l.via.x, y: l.via.y });
    }
    for (const w of layout.wires) {
      const path = svg('path', { class: `c-auto k-${w.auto.kind}${w.auto.inner ? ' inner' : ''}${w.auto.source === 'external' ? ' external' : ''}`, d: polylinePath(w.points, 4, CORNERS), 'marker-end': `url(#plane-arrow-${w.auto.kind})` });
      path.append(svg('title', {}, `${w.auto.source === 'external' ? 'External control cable' : 'Automation'}: ${w.auto.from} → ${w.auto.to}${w.auto.inner ? ` (inside ${w.auto.level}: edited in its Engine)` : ''}`));
      root.append(path);
      elements.set(w.id, path);
      items.push({ kind: 'auto', id: w.id, auto: w.auto, points: w.points });
    }
    // The engine package: the one the Wiring and the Engine draw, at the same place with the same signal pins.
    const c = layout.cube;
    const package_ = this.drawPackage(c, 'control', `${c.title}’s engine: ${c.names.join(', ') || 'no instances'}. Its inside is the Engine layer’s.`);
    const cube = package_.g;
    root.append(cube);
    elements.set('cube', cube);
    items.push({ kind: 'cube', id: 'cube', x: c.x, y: c.y, w: c.w, h: c.h });
    for (const p of layout.pins) {
      const dot = svg('circle', { class: 'c-pin', cx: p.x, cy: p.y, r: 3.5 });
      dot.append(svg('title', {}, p.address));
      root.append(dot);
      elements.set(p.id, dot);
      items.push({ kind: 'param', id: p.id, address: p.address, x: p.x, y: p.y });
    }
    for (const p of layout.autoPins) {
      const dot = svg('circle', { class: 'c-auto-pin', cx: p.x, cy: p.y, r: 4 });
      root.append(dot, svg('text', { class: 'c-auto-label', x: p.x - 8, y: p.y + 4, 'text-anchor': 'end' }, String(p.address).replace(/^\/[^/]+\//, '')));
      elements.set(p.id, dot);
      items.push({ kind: 'auto-pin', id: p.id, pin: p, x: p.x, y: p.y });
    }
    for (const p of [...layout.externalPins, ...layout.edgePins]) {
      const dot = svg('circle', { class: `c-edge ${p.kind}`, cx: p.x, cy: p.y, r: 5 });
      dot.append(svg('title', {}, p.kind === 'external' ? `${p.from} → ${p.address}: an external control cable through the instrument’s edge` : `${p.address}: bound outside the instrument`));
      root.append(dot);
      elements.set(p.id, dot);
      items.push({ kind: p.kind, id: p.id, pin: p, x: p.x, y: p.y });
    }
    // The inner value's readouts (ADR 0050 §11): text and a bar on the control's range, updated from the `parameters` stream.
    const readouts = new Map();
    for (const ro of layout.readouts) {
      const g = svg('g', { class: 'c-readout' });
      const text = svg('text', { class: 'c-readout-text', x: ro.x + ro.w / 2, y: ro.y + 9, 'text-anchor': 'middle' }, 'inner —');
      const track = svg('rect', { class: 'c-readout-track', x: ro.x + 4, y: ro.y + 11, width: ro.w - 8, height: 2.5 });
      const fill = svg('rect', { class: 'c-readout-fill', x: ro.x + 4, y: ro.y + 11, width: 0, height: 2.5 });
      g.append(text, track, fill);
      g.append(svg('title', {}, `The inner value at ${ro.address}: what the engine plays, its own modulation included`));
      root.append(g);
      const element = control.controls.find((x) => x.id === ro.control)?.element ?? null;
      readouts.set(ro.id, { ...ro, text, fill, element, shown: '' });
    }
    return { root, items, elements, readouts, marks: package_.marks };
  }

  renderControl(fades) {
    const s = this.stacks.get(this.focused);
    if (!s) return;
    const controls = fades.sheet > 0 ? this.controlsNow() : [];
    if (!controls.length) {
      hide(s.controlSvg);
      this.updateInner([]);
      return;
    }
    show(s.controlSvg);
    opacity(s.controlSvg, fades.sheet);
    this.cull(s, s.controlItems, s.controlElements, 'controlShown');
    this.updateInner(controls.flatMap((c) => c.layout.readouts));
  }

  /** The inner values' stream (`parameters`, ADR 0050 §11): subscribed while a Control plane shows them, at most 64. */
  updateInner(readouts) {
    const addresses = [...new Set(readouts.map((r) => r.address))].slice(0, 64);
    const key = addresses.join('\t');
    // The same addresses: readouts drawn anew show the values already in (frames come only on change).
    if (this.innerStream?.key === key) return this.scheduleInner();
    this.innerStream?.stop?.();
    this.innerStream = null;
    if (!addresses.length || !this.connection?.subscribe) return;
    const stream = { key, addresses, values: new Map(), stop: null };
    stream.stop = this.connection.subscribe({ stream: 'parameters', addresses, rate: 30 }, (frame) => {
      addresses.forEach((a, i) => {
        const v = frame?.values?.[i];
        if (v) stream.values.set(a, v.effective);
      });
      this.scheduleInner();
    });
    this.innerStream = stream;
  }

  scheduleInner() {
    if (this.innerPending) return;
    this.innerPending = true;
    requestAnimationFrame(() => {
      this.innerPending = false;
      const s = this.stacks.get(this.focused);
      const values = this.innerStream?.values;
      if (!s?.readouts || !values) return;
      for (const ro of s.readouts.values()) {
        const value = values.get(ro.address);
        if (!Number.isFinite(value)) continue;
        const range = ro.element?.binding?.range ?? null;
        const text = `inner ${formatValue(value, range?.unit ?? null)}`;
        if (text !== ro.shown) {
          ro.text.textContent = text;
          ro.shown = text;
        }
        const position = range ? toPosition(value, range) : 0;
        ro.fill.setAttribute('width', String(Math.max(0, Math.min(1, position)) * (ro.w - 8)));
      }
    });
  }

  // The Engine layer (the owner's decision of 2026-11-03): every instrument's inside in its own rectangle (ui/lib/plane-engine.js),
  // a drawing of the focused Stack like the Wiring and Control ones, in the same transform: the view never changes with the layer.

  /** Every instrument's Engine in the focused Stack (laid out and drawn once per graph, Stack layout and ownership), or null. */
  enginesNow() {
    const s = this.stacks.get(this.focused);
    if (!s?.layout || !this.graph || !this.model) return null;
    // What the Engine depends on: the packages' key (the structure, the Stack's layout, who holds what, the panel, the titles).
    const packages = this.packagesNow();
    if (!packages) return null;
    if (s.engines?.key === packages.key) return s.engines;
    const t0 = performance.now();
    const layouts = s.layout.rects.map((r) => {
      const pkg = packages.map.get(r.entry) ?? null;
      return engineLayout({ entry: r.entry, rect: { x: r.x, y: r.y, w: r.w, h: r.h }, pkg, names: pkg?.names ?? engineOf(r.entry, this.model.owner), graph: this.graph, title: instrumentTitle(r.entry, this.cards) });
    });
    s.engines = { key: packages.key, layouts, ms: performance.now() - t0 };
    this.drawEngines(s, s.engines);
    return s.engines;
  }

  /**
   * The Stack's Engine drawing: per instrument its faint frame and title and its engine package, closed (the package the
   * Wiring and Control draw, at the same place with the same pins), each with a hover title. A package is the top-level
   * chip (`s.engineChips`, `package: true`): its inside, the instrument's instances and their connections, is drawn when
   * first revealed by zoom (`renderInsides`), in its body, and fades in over it; the composites in it open the same way.
   */
  drawEngines(s, engines) {
    const root = svg('g', { transform: 'translate(-2 -2)' });
    const items = [];
    const elements = new Map();
    // The chips (the packages, then the composite cards in them) by id, the packages in order, and the insides built (`renderInsides`).
    s.engineChips = new Map();
    s.engineTopChips = [];
    s.insidesBuilt = new Set();
    // The elements a selection marks (`keyed`): the packages' pins here, each built inside's on its chip.
    s.engineKeyed = [];
    s.netMarks = { ...s.netMarks, engine: [] };
    for (const e of engines.layouts) {
      const g = svg('g', { 'data-entry': e.entry });
      const r = e.rect;
      g.append(svg('rect', { class: 'e-frame', x: r.x + 2, y: r.y + 2, width: r.w - 4, height: r.h - 4, rx: 6 }));
      g.append(svg('text', { class: 'e-title', x: r.x + 16, y: r.y + 18 }, `${String(e.title).toUpperCase()}${e.cards.length ? '' : ' · NO ENGINE'}`));
      if (e.pkg) {
        const drawn = this.drawPackage(e.pkg, 'engine', `${e.title}’s audio engine${e.cards.length ? `: ${e.cards.length} instance${e.cards.length === 1 ? '' : 's'}; zoom in to see its inside` : ': no instances'}`);
        // The plain body, where the inside will show, fading out as it comes in.
        const plain = svg('rect', { class: 'e-plain', x: e.area.x, y: e.area.y, width: e.area.w, height: e.area.h, rx: 3 });
        drawn.g.insertBefore(plain, drawn.labels);
        s.netMarks.engine.push(...drawn.marks);
        // A pin marks the connection crossing the instrument's edge there (the inside's edge, `engineItems`' e-edge).
        for (const p of e.edges) {
          const pin = drawn.pins.get(`port ${p.side} ${p.address}`);
          if (pin) this.keyed(pin, p.side === 'in' ? wireKey(p.from, p.address) : wireKey(p.address, p.to), s.engineKeyed);
        }
        const chip = { id: e.card.id, entry: e.entry, card: e.card, pins: [], scale: e.scale, address: '', element: drawn.g, before: drawn.labels, plain, labels: drawn.labels, parent: null, children: [], laid: e, group: null, package: true };
        s.engineChips.set(chip.id, chip);
        s.engineTopChips.push(chip);
        g.append(drawn.g);
        elements.set(`${e.entry}|${e.card.id}`, drawn.g);
        items.push({ id: `${e.entry}|${e.card.id}`, ...e.frame });
      }
      root.append(g);
    }
    s.engineSvg.replaceChildren(root);
    s.engineItems = items;
    s.engineElements = elements;
    s.engineShown = null;
  }

  /**
   * One level of an Engine drawn into `g` (`layLevel`'s, plane units): its wires, its cards and their pins, each with a hover
   * title. `parent`: the chip it is the inside of (an instrument's package: the project's top level, its addresses as they
   * are; a composite card: its composite's level). Each composite card is registered as a chip (`s.engineChips`): its plain
   * body shows until its inside, laid out and drawn when first revealed (`renderInsides`), fades in over it. The lines are
   * drawn at the level's scale (no `non-scaling-stroke` inside a chip: the same width in every engine, whatever it makes of
   * the stage's CSS transform).
   */
  drawLevel(s, g, laid, { entry, parent }) {
    const k = laid.scale;
    // Inside a package: the instrument's instances, top-level instances; inside a composite card: its composite's.
    const top = Boolean(parent.package);
    const prefix = top ? '' : parent.address;
    // A line's width at this level: at its scale.
    const line = (width) => ({ style: `stroke-width: ${+(width * k).toPrecision(4)}` });
    // An address as the person knows it: inside the chip /synth, `fenv.gate` is /synth/fenv.gate and its input `gate` /synth.gate.
    const full = (a) => fullAddress(prefix, a);
    const keyed = (parent.keyed = []);
    const edgeNote = top ? ', through the instrument’s edge' : `, through ${parent.card.name}’s edge`;
    const wires = svg('g', { class: 'e-wires' });
    for (const w of laid.wires) {
      const path = this.keyed(svg('path', { class: `e-wire k-${w.kind}`, d: polylinePath(w.points, 6 * k, CORNERS), ...line(2) }), wireKey(full(w.from), full(w.to)), keyed);
      path.append(svg('title', {}, `${full(w.from)} → ${full(w.to)} (${w.kind})${w.edge ? edgeNote : ''}`));
      wires.append(path);
      for (const j of w.junctions) wires.append(svg('circle', { class: `e-junction k-${w.kind}`, cx: j.x, cy: j.y, r: 3 * k }));
    }
    // The control signal: in a package each bound parameter wired from the connector into its pin, inside a chip from the chip's pin.
    for (const b of laid.binds ?? []) {
      const key = bindKey(full(b.address));
      const title = top ? `${b.address}: its control at the Control layer, through the package’s connector` : `${full(b.address)}: ${parent.card.name}’s parameter ${b.export ?? ''} (${parent.address}.${b.export ?? ''}), set by its control`;
      const path = this.keyed(svg('path', { class: 'e-wire e-bind', d: polylinePath(b.points, 6 * k, CORNERS), ...line(1.5) }), key, keyed);
      path.append(svg('title', {}, title));
      wires.append(path);
      for (const j of b.junctions ?? []) wires.append(svg('circle', { class: 'e-junction e-bind-dot', cx: j.x, cy: j.y, r: 3 * k }));
    }
    // Where a parameter's control and its connection combine (its declared `combine`): a ring with the operation, not a dot.
    for (const c of laid.combines ?? []) {
      const mark = svg('g', { class: 'e-combine' });
      mark.append(svg('circle', { cx: c.x, cy: c.y, r: 3.5 * k, 'stroke-width': 1.2 * k }));
      mark.append(svg('text', { x: c.x, y: c.y + 1.9 * k, 'font-size': 6 * k, 'text-anchor': 'middle' }, COMBINE_SIGN[c.mode] ?? '+'));
      mark.append(svg('title', {}, `${full(c.address)}: its control and its connection combined (${c.mode})`));
      wires.append(mark);
    }
    g.append(wires);
    const composites = new Map();
    for (const c of laid.cards) {
      const card = svg('g', { class: `e-node${c.composite ? ' composite' : ''}`, 'data-name': c.name });
      card.append(this.keyed(svg('rect', { class: `e-card${c.composite ? ' composite' : ''}`, x: c.x, y: c.y, width: c.w, height: c.h, rx: CARD_RX * k, ...line(c.composite ? 2 : 1.5) }), cardKey(`${prefix}/${c.name}`), keyed));
      // The header band inset by half the card's edge, as the schematic's cards.
      card.append(svg('path', { class: 'e-head', d: headPath({ x: c.head.x + 0.75 * k, y: c.head.y + 0.75 * k, width: c.head.w - 1.5 * k, height: c.head.h - 0.75 * k }, Math.max(0, (CARD_RX - 0.75) * k)) }));
      card.append(svg('text', { class: 'e-name', x: c.x + 8 * k, y: c.y + 14 * k, 'font-size': 12 * k }, c.name));
      card.append(svg('text', { class: 'e-module', x: c.x + 8 * k, y: c.y + 26 * k, 'font-size': 9.5 * k }, c.module));
      const address = `${prefix}/${c.name}`;
      const count = c.composite ? (this.graph?.composites?.[c.module]?.instances?.length ?? 0) : 0;
      card.append(svg('title', {}, `${address}: ${c.module}${c.composite ? ` (a chip of ${count} instances: zoom in to see its inside; its context menu shows it whole)` : ''}`));
      if (c.composite) {
        // The plain body: a faint panel where the inside will show, fading out as it comes in.
        const plain = svg('rect', { class: 'e-plain', x: c.body.x, y: c.body.y, width: c.body.w, height: c.body.h, rx: 3 * k });
        card.append(plain);
        const id = top ? c.id : `${parent.id}/${c.id}`;
        card.dataset.chip = id;
        const chip = { id, entry, card: c, pins: laid.pins, scale: k, address, element: card, plain, labels: svg('g', { class: 'e-chip-labels' }), parent, children: [], laid: undefined, group: null };
        composites.set(c.id, chip);
        s.engineChips.set(id, chip);
        parent.children.push(chip);
      }
      g.append(card);
    }
    for (const p of laid.pins) {
      const dot = this.keyed(svg('circle', { class: `e-pin k-${p.kind}${p.connected ? ' connected' : ''}${p.bind ? ' e-bind-pin' : ''}`, cx: p.x, cy: p.y, r: 3 * k, ...line(1.5) }), pinKey(p.side, full(p.address)), keyed);
      dot.append(svg('title', {}, `${full(p.address)} (${p.parameter ? `a parameter${p.bind ? ' its control sets' : p.connected ? ' a connection drives' : ''}` : `${p.kind} ${p.side === 'in' ? 'input' : 'output'}`})`));
      // A chip's pin labels fade out as its inside comes in (the owner's look, 2026-11-03: they grew over it), kept legible by a halo till then.
      const chip = composites.get(p.card);
      const label = svg('text', { class: `e-pin-label${chip ? ' halo' : ''}`, x: p.side === 'in' ? p.x + 6 * k : p.x - 6 * k, y: p.y + 3 * k, 'font-size': 9 * k, 'text-anchor': p.side === 'in' ? 'start' : 'end', ...(chip ? { 'stroke-width': 2.5 * k } : {}) }, p.name);
      g.append(dot);
      if (chip) chip.labels.append(label);
      else g.append(label);
    }
    // Inside a chip, its pins' names at the inside's scale by its edges (the chip's own labels fade out as its inside comes in).
    for (const n of laid.names ?? []) {
      const left = n.side === 'in';
      g.append(svg('text', { class: 'e-pin-label halo e-inherited', x: left ? n.x + 4 * k : n.x - 4 * k, y: n.y - 2.5 * k, 'font-size': 8 * k, 'text-anchor': left ? 'start' : 'end', 'stroke-width': 2.5 * k }, n.text));
    }
    for (const chip of composites.values()) g.append(chip.labels);
  }

  /** An Engine element marked with `key` (what a selection of it marks, `selectionKeys`), kept in `list`, selected now if it is. */
  keyed(element, key, list) {
    // One element may stand for several connections (a package's pin of a fan-out): each of its keys marks it.
    if (element.planeKeys) element.planeKeys.push(key);
    else {
      element.planeKeys = [key];
      element.setAttribute('data-key', key);
      list.push(element);
    }
    if (this.selKeys?.has(key)) element.classList.add('selected');
    return element;
  }

  /**
   * The Engine's hit items now (`engineItems`'): each revealed chip's inside (revealed enough to take the pointer, its chips
   * around it too), outer first (a package's, the instrument's instances at the project's top level, then the composites in
   * it): a deeper card lies over the chip's. A closed package has none: the press is its instrument's empty space.
   */
  engineHitItems(s) {
    const items = [];
    const revealed = new Set();
    const visit = (chip) => {
      if (!chip.hit || !chip.laid || !chip.group?.parentNode) return;
      revealed.add(chip.id);
      chip.items ??= chip.package ? engineItems(chip.laid, { entry: chip.entry }) : engineItems(chip.laid, { entry: chip.entry, level: chip.card.module, prefix: chip.address, chip: chip.id });
      items.push(...chip.items);
      for (const child of chip.children) visit(child);
    };
    for (const chip of s.engineTopChips ?? []) visit(chip);
    return { items, revealed };
  }

  renderEngine(fades) {
    const s = this.stacks.get(this.focused);
    if (!s) return;
    if (!(fades.deep > 0)) return hide(s.engineSvg);
    const engines = this.enginesNow();
    if (!engines) return hide(s.engineSvg);
    show(s.engineSvg);
    opacity(s.engineSvg, fades.deep);
    this.cull(s, s.engineItems, s.engineElements, 'engineShown');
    this.renderInsides(s);
  }

  /** A chip's inside laid out (`insideOf`), once: kept on the chip, which lives as long as the Engine's layout (its key: the graph, the Stack's layout, who holds what). */
  insideFor(chip) {
    if (chip.laid === undefined) chip.laid = insideOf({ graph: this.graph, card: chip.card, pins: chip.pins, scale: chip.scale });
    return chip.laid;
  }

  /**
   * The chips' insides by zoom (no layout read, no layout per frame): a chip on the stage whose body is wide enough on screen
   * (`revealShare`) has its inside laid out and drawn once, kept, and put in its card at that opacity while its plain body fades out;
   * recursively, the chips inside it. The others' insides are taken out of the document (a few kept built, INSIDES_KEPT), so
   * what is drawn stays bounded by what is on the stage.
   */
  renderInsides(s) {
    const rect = this.focusRect();
    if (!rect || !s.engineTopChips) return;
    const v = this.view;
    const margin = 24;
    const wanted = new Set();
    const visit = (chip) => {
      const c = chip.card;
      const share = revealShare(c.body.w * v.k, c.reveal);
      const x = (rect.x + c.x) * v.k + v.x;
      const y = (rect.y + c.y) * v.k + v.y;
      const on = share > 0 && x < this.viewport.w + margin && x + c.w * v.k > -margin && y < this.viewport.h + margin && y + c.h * v.k > -margin;
      opacity(chip.plain, on ? 1 - share : 1);
      opacity(chip.labels, on ? 1 - share : 1);
      chip.hit = false;
      if (!on || !this.insideFor(chip)) return;
      if (!chip.group) {
        chip.group = svg('g', { class: 'e-inside' });
        this.drawLevel(s, chip.group, chip.laid, { entry: chip.entry, parent: chip });
      }
      // Kept in order of use: the least recently drawn are dropped first.
      s.insidesBuilt.delete(chip);
      s.insidesBuilt.add(chip);
      wanted.add(chip);
      // Under the card's own pins and labels (drawn after the cards; a package's in its group, `before`), over its body.
      if (chip.group.parentNode !== chip.element) chip.element.insertBefore(chip.group, chip.before ?? null);
      opacity(chip.group, share);
      // A faint inside takes no pointer: the chip's card under it keeps its hover title and its menu.
      const faint = share < 0.5;
      if (chip.group.hasAttribute('data-faint') !== faint) chip.group.toggleAttribute('data-faint', faint);
      // Hit-tested (selection, editing) as it takes the pointer: only revealed enough (`engineHitItems`).
      chip.hit = !faint;
      for (const child of chip.children) visit(child);
    };
    for (const chip of s.engineTopChips) visit(chip);
    let spare = s.insidesBuilt.size - wanted.size;
    for (const chip of [...s.insidesBuilt]) {
      if (wanted.has(chip)) continue;
      if (chip.group?.parentNode) chip.group.remove();
      if (spare > INSIDES_KEPT && s.insidesBuilt.has(chip)) spare -= this.dropInside(s, chip);
    }
  }

  /** A chip's inside dropped (built again when next revealed), with the chips inside it; how many built insides that dropped. */
  dropInside(s, chip) {
    let dropped = s.insidesBuilt.delete(chip) ? 1 : 0;
    chip.keyed = null;
    chip.items = null;
    for (const child of chip.children) {
      dropped += this.dropInside(s, child);
      s.engineChips.delete(child.id);
    }
    chip.children = [];
    chip.group = null;
    return dropped;
  }

  /** The chips the view is inside (`chipPath`, from the laid-out packages and cards, no DOM): the stage's centre in a fully revealed chip's body, deeper and deeper; a package first. */
  chipsNow() {
    const s = this.stacks.get(this.focused);
    const rect = this.focusRect();
    if (this.level() !== 'engine' || !s?.engines || !s.engineChips || !rect || !this.viewport.w) return [];
    const p = toPlane(this.view, { x: this.viewport.w / 2, y: this.viewport.h / 2 });
    const cards = s.engines.layouts.map((e) => e.card);
    const path = chipPath(cards, { x: p.x - rect.x, y: p.y - rect.y }, this.view.k, (card, id) => {
      const chip = s.engineChips.get(id);
      return chip ? this.insideFor(chip) : null;
    });
    return path.map((step) => s.engineChips.get(step.id));
  }

  /** The view travels to show a chip (`part`: its `body`, the Layer N buttons; else the whole card, Show Chip). */
  showChip(chip, part = null, maxK = Infinity) {
    const rect = this.focusRect();
    const fit = this.fitK();
    if (!chip || !rect || !fit) return;
    const r = part === 'body' ? chip.card.body : chip.card;
    const target = { x: rect.x + r.x, y: rect.y + r.y, w: r.w, h: r.h };
    let k = fitAll(target, this.viewport).k;
    // A package's inside is whole only past its reveal width: inside it (Layer 1), never short of it.
    if (part === 'body' && chip.card.reveal) k = Math.max(k, (chip.card.reveal[1] + 1) / Math.max(1e-9, chip.card.body.w));
    if (!this.layer) {
      // The coupled zoom: the chip's instrument is the opened one, and the view stays at the Engine (never above its band's end).
      if (this.opened !== chip.entry) {
        this.opened = chip.entry;
        this.barKey = null;
      }
      k = Math.max(k, fit * splitDepth(this.engineBandNow()[1]).r);
    }
    this.travel(this.viewAround(target, Math.min(k, fit * DEEP_SCALE, maxK)), this.layer ? 0 : LIFT);
  }

  /** Zoomed to an address at the Engine (a sidebar's link): its card a part of the stage, inside its chips revealed. */
  showAddress(address) {
    const found = locateAddress(this.enginesNow()?.layouts, this.graph, address);
    if (!found) return this.say(`${address}: not in this Stack’s engines`);
    // At a chosen layer, the Engine chosen; with the coupled zoom, `showChip` zooms into the Engine's band.
    if (this.layer && this.layer !== 'engine') this.chooseLayer('engine');
    // The card a third of the stage, so that its pins read and its neighbours show.
    const r = found.rect;
    const pad = { x: r.x - r.w, y: r.y - r.h, w: r.w * 3, h: r.h * 3 };
    this.showChip({ card: pad, entry: found.entry });
  }

  /** The view at scale `k` with plane rectangle `r`'s centre at the stage's centre. */
  viewAround(r, k) {
    return { k, x: this.viewport.w / 2 - (r.x + r.w / 2) * k, y: this.viewport.h / 2 - (r.y + r.h / 2) * k };
  }

  /** A chip by its id now (the bar's buttons look it up when clicked: a layout since may have replaced the chip). */
  chipById(id) {
    return this.stacks.get(this.focused)?.engineChips?.get(id) ?? null;
  }

  /** Docks the level bar (the breadcrumb, the zoom, Edit layout) in `host`, such as the app's toolbar, with its styles. */
  dockBar(host) {
    this.barDock = host;
    if (!this.bar) return;
    const root = host.shadowRoot ?? host.attachShadow({ mode: 'open' });
    root.replaceChildren(html('style', { textContent: BAR_CSS }), this.bar);
    this.bar.classList.add('docked');
  }

  /** The bar: the layers (with, inside chips at the Engine, a Layer N button per chip, `chips` outermost first), the zoom, the lock, Edit. */
  renderBar(level, r, chips = []) {
    const instruments = this.instrumentRects().length > 0;
    // The Engine's Edit is the Engine's: another layer leaves it.
    if (this.engineEdit && level !== 'engine') this.leaveEngineEdit();
    const key = `${level}|${this.focused}|${this.opened}|${instruments}|${Boolean(this.editing)}|${this.engineEdit}|${chips.map((c) => c.id).join(' ')}`;
    if (key !== this.barKey) {
      this.barKey = key;
      // Every layer is a button, the shown one marked: a click goes there (the owner's, 2026-11-01).
      const layers = html('div', { className: 'layers', role: 'group', ariaLabel: 'Layers' });
      for (const [id, label] of BAR_LAYERS) {
        const b = html('button', { type: 'button', className: 'layer', textContent: label, title: this.layerTitle(id) });
        const deeper = id === 'engine' && chips.length > 0;
        const here = (id === level && !deeper) || (id === 'stack' && level === 'setup');
        b.setAttribute('aria-current', String(here));
        b.disabled = id !== 'stack' && (!this.focused || ((id === 'control' || id === 'engine') && !instruments));
        // Inside a package, Engine travels back out to the instrument whole.
        if (deeper) {
          const outer = chips[0].id;
          b.addEventListener('click', () => this.outOfChips(this.chipById(outer)));
        }
        else if (!here) b.addEventListener('click', () => this.goTo(id === 'stack' && !this.focused ? 'setup' : id));
        layers.append(b);
      }
      // Inside chips (derived from the view): Layer 1 inside an instrument's engine package, Layer 2 inside a composite in it, …;
      // the deepest marked; a click fits that chip's body on the stage.
      chips.forEach((chip, i) => {
        const deepest = i === chips.length - 1;
        const inside = chip.package ? `${chip.card.name}’s engine package` : `${chip.card.name} (${chip.address})`;
        const b = html('button', { type: 'button', className: 'layer', textContent: `Layer ${i + 1}`, title: `Layer ${i + 1}: inside ${inside}` });
        b.setAttribute('aria-current', String(deepest));
        const id = chip.id;
        b.addEventListener('click', () => this.showChip(this.chipById(id), 'body'));
        layers.append(b);
      });
      this.levelText = html('span', { className: 'level' });
      const zoom = html('span', { className: 'zoom' });
      const step = (text, factor, title) => {
        const b = html('button', { type: 'button', textContent: text, title });
        b.addEventListener('click', () => this.zoomBy(factor, { x: this.viewport.w / 2, y: this.viewport.h / 2 }));
        return b;
      };
      zoom.append(step('−', 1 / KEY_ZOOM, 'Zoom out'), this.levelText, step('+', KEY_ZOOM, 'Zoom in'));
      // One Edit switch: it edits the shown layer's arrangement (built for the Stack: its front panels' layout).
      const edit = html('button', { type: 'button', className: 'edit-layout', textContent: 'Edit' });
      edit.disabled = level !== 'stack' && level !== 'engine';
      edit.title =
        level === 'stack'
          ? 'Edit: arrange the front panels, written to ui/main.html (again to leave it)'
          : level === 'engine'
            ? 'Edit the Engine: drag from a pin to connect, Delete removes, the menu adds a module; each edit a patch of the project’s files, undoable (again to leave it)'
            : `Edit at the ${LAYER_NAME[level]} layer: not built yet`;
      edit.setAttribute('aria-pressed', String(level === 'engine' ? Boolean(this.engineEdit) : Boolean(this.editing)));
      edit.addEventListener('click', () => {
        if (level === 'engine') this.toggleEngineEdit();
        else if (this.editing) this.closeEditLayout();
        else this.editLayout();
      });
      this.editButton = edit;
      // The zoom lock: the wheel scrolls up and down; the − and + buttons still zoom.
      const lock = html('button', { type: 'button', className: 'zoom-lock', title: 'Zoom lock: the wheel scrolls up and down, the mouse zooms nothing (− and + still do); a double right click toggles it' });
      lock.innerHTML = '<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="6.5" cy="6.5" r="4.5"/><path d="M10 10l4.5 4.5"/><path d="M4.5 6.5h4"/></svg>';
      lock.setAttribute('aria-pressed', String(this.zoomLock));
      lock.addEventListener('click', () => this.toggleZoomLock());
      this.zoomLockButton = lock;
      this.bar.replaceChildren(layers, zoom, lock, edit);
    }
    const text = `${r ? r.toFixed(2) : '0'}×`;
    if (this.levelText.textContent !== text) this.levelText.textContent = text;
  }

  /** From inside a chip back out to the Engine of its instrument, whole on the stage (chosen layer), or at the engine band's end (coupled zoom). */
  outOfChips(chip) {
    if (!chip) return;
    if (this.layer) return this.fitOnScreen(this.focused, chip.entry);
    // The coupled zoom: at the engine band's end, the instrument's centre at the stage's (computed outright: deep inside a
    // chip the instrument's centre is far off the stage, and a zoom about it would keep it there).
    const r = this.instrumentRects().find((x) => x.entry === chip.entry);
    const fit = this.fitK();
    if (!r || !fit) return;
    this.opened = chip.entry;
    this.barKey = null;
    const to = this.viewAround(r, fit * splitDepth(this.engineBandNow()[1]).r);
    this.travel({ ...to, ...clampTop(to, this.focusRect(), this.viewport) }, LIFT);
  }

  /** Escape inside chips: out one chip level (the parent chip's body fitted, or from the outermost its instrument whole); whether it was inside one. */
  outOneChip() {
    const chips = this.chipsNow();
    const step = outStep(chips);
    if (!step) return false;
    // Never so close that the chip left is still entered (its body under the full reveal's width), or Escape would repeat itself.
    const left = chips.at(-1).card;
    const cap = ((left.reveal?.[1] ?? REVEAL[1]) - 1) / Math.max(1e-9, left.body.w);
    if (step.chip) return this.showChip(step.chip, step.part, cap), true;
    const r = this.instrumentRects().find((x) => x.entry === step.instrument.entry);
    if (this.layer && r && fitAll(r, this.viewport).k > cap) {
      this.opened = step.instrument.entry;
      this.barKey = null;
      this.travel(this.viewAround(r, cap), 0);
    } else this.outOfChips(step.instrument);
    return true;
  }

  /** A layer button's title: what it shows. */
  layerTitle(id) {
    if (id === 'setup') return 'Project: the whole project, its Stacks and devices';
    if (id === 'stack') return this.focused ? `Stack ${this.focused}: its instruments' front panels` : 'Stack: the whole project, its Stacks and devices';
    if (id === 'wiring') return 'Wiring: the cables between the instruments';
    const name = this.opened ? instrumentTitle(this.opened, this.cards) : 'the instrument in the middle';
    return id === 'control' ? `Control: ${name}'s bindings and automation` : 'Engine: every instrument’s engine package, in its place; zoom into one to see its inside (Layer 1), into a composite in it (Layer 2), …';
  }

  /** Goes to a layer from wherever the view is (a layer button). */
  goTo(level) {
    if (this.layer) return this.chooseLayer(level);
    const rect = this.focusRect();
    const here = this.level();
    if (level === 'setup') return this.fitSetup();
    if (!rect) return;
    if (level === 'stack') return this.focusStack(this.focused);
    // Wiring, Control and Engine are depths of the focused Stack: from its fit (keeping its scroll when already in it).
    this.settleTravel();
    const base = here === 'setup' ? fitWidth(rect, this.viewport, null) : fitWidth(rect, this.viewport, this.view);
    const centre = { x: this.viewport.w / 2, y: this.viewport.h / 2 };
    if (level === 'wiring') {
      const { to, lift } = this.depthView(WIRING_BAND[1], base, centre);
      return this.travel(to, lift);
    }
    const rects = this.instrumentRects();
    const atCentre = instrumentAt(rects, toPlane(base, centre));
    const opened = rects.find((r) => r.entry === this.opened) ?? rects.find((r) => r.entry === atCentre) ?? rects[0];
    if (!opened) return;
    this.opened = opened.entry;
    this.barKey = null;
    const at = { x: base.x + (opened.x + opened.w / 2) * base.k, y: base.y + (opened.y + opened.h / 2) * base.k };
    const control = this.depthView(CONTROL_BAND[1], base, at);
    if (level === 'control') return this.travel(control.to, control.lift);
    // The Engine: from the instrument's Control plane, on into its rectangle.
    this.stopTravel();
    this.view = control.to;
    this.lift = control.lift;
    this.render();
    this.enterEngine();
  }

  /** The layer chosen alone (`zoomPicksLayer` false): the view stays where it is; Control and the Engine open the instrument in the middle (or the last opened). */
  chooseLayer(level) {
    if (level === this.layer) return;
    // Into Control or the Engine from above: the instrument in the middle of the stage opens (else the one opened, else the first).
    const above = this.layer !== 'control' && this.layer !== 'engine';
    if ((level === 'control' || level === 'engine') && (above || !this.instrumentRects().some((r) => r.entry === this.opened))) {
      const rects = this.instrumentRects();
      const centre = toPlane(this.view, { x: this.viewport.w / 2, y: this.viewport.h / 2 });
      const known = rects.some((r) => r.entry === this.opened) ? this.opened : null;
      this.opened = instrumentAt(rects, centre) ?? known ?? rects[0]?.entry ?? null;
    }
    if (this.editing && level !== 'stack') this.closeEditLayout();
    this.layer = level;
    this.barKey = null;
    this.render();
  }

  /** The view at depth `z` (`depthOf`) zoomed from `base` about `at` (stage pixels), its top kept in the Stack. */
  depthView(z, base, at) {
    const fit = this.fitK();
    const { r, lift } = splitDepth(z);
    let to = zoomAbout(base, (fit * r) / base.k, at, fit, this.engineBandNow()[1]);
    to = { ...to, ...clampTop(to, this.focusRect(), this.viewport) };
    return { to, lift };
  }

  // Travel: a focus, Fit, Escape and Enter animate the view (ADR 0047 §8's curve), at once with reduced motion.

  travel(to, lift = this.lift) {
    this.stopTravel();
    this.touched = true;
    const from = this.view;
    const fromLift = this.lift;
    const reduced = globalThis.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
    if (reduced) {
      this.view = to;
      this.lift = lift;
      this.render();
      return;
    }
    const start = performance.now();
    this.travelTarget = to;
    this.travelLift = lift;
    const step = (now) => {
      const t = Math.min(1, (now - start) / ZOOM_MS);
      this.lift = t >= 1 ? lift : fromLift + (lift - fromLift) * ease(t);
      this.view = t >= 1 ? to : plain(tween(from, to, ease(t)));
      this.render();
      this.traveling = t < 1 ? requestAnimationFrame(step) : null;
    };
    this.traveling = requestAnimationFrame(step);
  }

  stopTravel() {
    if (this.traveling) cancelAnimationFrame(this.traveling);
    this.traveling = null;
  }

  /** A travel under way ends where it was going (a scroll during a focus scrolls the fitted Stack, never one half-way there). */
  settleTravel() {
    if (!this.traveling) return;
    this.stopTravel();
    this.view = this.travelTarget;
    this.lift = this.travelLift ?? this.lift;
  }

  /** Setup → Stack (ADR 0051 §8): the Stack's width fills the stage, its top in view. */
  focusStack(name) {
    if (!this.layout?.stacks.has(name)) return;
    this.focused = name;
    this.barKey = null;
    const rect = this.layout.stacks.get(name);
    const keep = this.level() !== 'setup' ? this.view : null;
    if (this.layer === 'setup') this.layer = 'stack';
    this.travel(fitWidth(rect, this.viewport, keep), 0);
  }

  /**
   * A new, empty Stack standing at `at` (plane px), written to ui/plane.toml
   * in one patch (\`panel.restore\` of the file, so the plane's Undo takes it
   * back). Without a file the derived Stack is written first, as it stands.
   */
  addStack(at) {
    const plane = this.model?.plane;
    if (!plane) return;
    const names = new Set(plane.stacks.map((s) => s.name));
    let n = plane.stacks.length + 1;
    while (names.has(`stack_${n}`)) n++;
    const name = `stack_${n}`;
    const width = plane.stacks[0]?.width ?? 160;
    const stacks = plane.stacks.map((s) => ({ name: s.name, at: s.at ?? null, width: s.width, instruments: [...s.instruments] }));
    stacks.push({ name, at: [Math.round(at.x / PU), Math.round(at.y / PU)], width, instruments: [] });
    const op = { op: 'panel.restore', file: PLANE_FILE, text: writePlane({ schema_version: plane.schema_version ?? 1, stacks }) };
    if (this.planeText !== null && this.planeText !== undefined) op.was = this.planeText;
    this.writePatch([op], `Stack ${name}`).then((written) => written && this.say(`Stack ${name} created (ui/plane.toml)`));
  }

  /** The zoom lock on or off (its button, a double right click). */
  toggleZoomLock() {
    this.zoomLock = !this.zoomLock;
    writeZoomLock(this.zoomLock);
    this.zoomLockButton?.setAttribute('aria-pressed', String(this.zoomLock));
    this.say(this.zoomLock ? 'Zoom lock on: the wheel scrolls' : 'Zoom lock off: the wheel zooms');
  }

  /**
   * A screen icon's click (the owner's, 2026-11-02): the Stack, or one of its
   * instruments, glides onto the screen whole (the view's travel; the layer
   * is kept). The Stack becomes the focused one; an instrument the opened one.
   */
  fitOnScreen(name, entry) {
    const rect = this.layout?.stacks.get(name);
    if (!rect) return;
    if (this.focused !== name) {
      this.focused = name;
      this.barKey = null;
    }
    let target = rect;
    if (entry) {
      const r = this.stacks.get(name)?.layout?.rects.find((x) => x.entry === entry);
      if (!r) return;
      target = { x: rect.x + r.x, y: rect.y + r.y, w: r.w, h: r.h };
      this.opened = entry;
      this.barKey = null;
    }
    this.touched = true;
    this.travel(fitAll(target, this.viewport), 0);
  }

  /** The whole plane in view: the Setup level. */
  fitSetup() {
    if (this.layer && this.layer !== 'setup') this.chooseLayer('setup');
    if (this.layout) this.travel(fitAll(this.layout.bounds, this.viewport), 0);
  }

  /** The view zoomed about `at` (the stage's centre) to depth `z` (`depthOf`); the selection is untouched. */
  travelTo(z, at = { x: this.viewport.w / 2, y: this.viewport.h / 2 }) {
    const fit = this.fitK();
    if (!fit) return;
    const { r, lift } = splitDepth(z);
    let to = zoomAbout(this.view, (fit * r) / this.view.k, at, fit, this.engineBandNow()[1]);
    to = { ...to, ...clampTop(to, this.focusRect(), this.viewport) };
    this.travel(to, lift);
  }

  /** Wiring → Control: the instrument at the stage's centre opens, the view zoomed about its centre to the control band's end. */
  openAtCentre() {
    const centre = { x: this.viewport.w / 2, y: this.viewport.h / 2 };
    const entry = instrumentAt(this.instrumentRects(), toPlane(this.view, centre));
    if (entry) this.opened = entry;
    this.barKey = null;
    this.travelTo(CONTROL_BAND[1], centre);
  }

  /** Control → Engine (the coupled zoom): the view zoomed on into the engine band, about the opened instrument's centre. */
  enterEngine() {
    const r = this.instrumentRects().find((x) => x.entry === this.opened);
    if (!r) return;
    this.travelTo(this.engineBandNow()[1], toScreen(this.view, { x: r.x + r.w / 2, y: r.y + r.h / 2 }));
  }

  // The pointer, the wheel, the keys.

  point(event) {
    const r = this.stage.getBoundingClientRect();
    return { x: event.clientX - r.left, y: event.clientY - r.top };
  }

  /** On a front panel's control (a signal-* element of a panel's shadow root): the control's, not the plane's. */
  onControl(event) {
    for (const node of event.composedPath?.() ?? []) {
      if (node === this.stage) return false;
      if (node?.classList?.contains?.('face-panel')) return false;
      if (typeof node?.localName === 'string' && node.localName.startsWith('signal-') && node.localName !== 'signal-plane') return true;
    }
    return false;
  }

  // Edit layout (ADR 0051 §6, §9: the front panels' layout is edited at the Stack level).

  /**
   * Opens Edit layout over the plane: the panel editor of `ui/main.html`
   * (a `<signal-panel file="main.html">` entered) filling the stage, in a
   * shadow root with the panel's style sheets. ADR 0053: this page holds
   * the session token, so the panel is never loaded here as a page or a
   * frame; the editor draws its written content within the whitelist
   * (`panel.read`, `parseContent`), as the front panels are drawn, and the
   * panel's scripts never run. Everything it writes is written at once
   * (`panel.write`) and comes back to the front panels as `panel.written`.
   * Done, the editor's own switch or Escape closes it.
   */
  editLayout() {
    if (this.editing || !this.stage) return;
    const overlay = html('div', { className: 'editor' });
    const body = html('div', { className: 'editor-body' });
    const shadow = body.attachShadow({ mode: 'open' });
    const links = this.sheets.map((href) => Object.assign(document.createElement('link'), { rel: 'stylesheet', href }));
    const panel = document.createElement('signal-panel');
    panel.setAttribute('file', 'main.html');
    // Its relative URLs (skins) from the panel's page, as the front panels resolve them.
    panel.attributeValue = (name, value) => faceAttribute(name, value, '', this.base);
    // The bar's Edit layout button is the one switch: the panel's own bar shows only while a write failed (its Retry all, Revert all).
    const css = `signal-panel { display: block; min-height: 100%; }\nsignal-panel::part(bar) { display: none; }\n:host(.unsaved) signal-panel::part(bar) { display: flex; }\n${this.rootCss ?? ''}`;
    shadow.append(...links, html('style', { textContent: css }), panel);
    overlay.append(body);
    this.stage.append(overlay);
    const editing = { overlay, body, panel, entered: false, stop: null };
    this.editing = editing;
    this.editButton?.setAttribute('aria-pressed', 'true');
    this.barKey = null;
    this.enterEditLayout(editing);
  }

  /** The editor entered as soon as it has a plan to check against; its leaving closes the overlay. */
  enterEditLayout(editing) {
    const { panel } = editing;
    const started = Date.now();
    const attempt = () => {
      if (this.editing !== editing) return;
      const edit = panel.shadowRoot?.querySelector?.('.edit');
      if (edit?.getAttribute('aria-disabled') !== 'false') {
        if (Date.now() - started < 10000) requestAnimationFrame(attempt);
        else this.say('Edit layout: the panel could not be edited (no plan yet?)');
        return;
      }
      if (typeof MutationObserver === 'function') {
        const observer = new MutationObserver(() => {
          if (panel.hasAttribute('editing')) editing.entered = true;
          else if (editing.entered) this.closeEditLayout();
        });
        observer.observe(panel, { attributes: true, attributeFilter: ['editing'] });
        // A failed write shows the panel's bar (its unsaved note and Retry all) until it is settled.
        const unsaved = panel.shadowRoot?.querySelector?.('.unsaved');
        const shown = () => editing.body.classList.toggle('unsaved', unsaved?.style.visibility === 'visible');
        const watch = new MutationObserver(shown);
        if (unsaved) watch.observe(unsaved, { attributes: true, attributeFilter: ['style'] });
        shown();
        editing.stop = () => {
          observer.disconnect();
          watch.disconnect();
        };
      }
      panel.setEditing?.(true);
    };
    attempt();
  }

  /** Whether `event` is on the Edit layout or the sidebar over the plane (the plane underneath takes nothing of it). */
  overEditor(event) {
    const path = event.composedPath?.() ?? [];
    return Boolean((this.editing && path.includes(this.editing.overlay)) || (this.side && path.includes(this.side)));
  }

  /** Closes Edit layout (its editor left first; what it wrote is written). */
  closeEditLayout() {
    const editing = this.editing;
    if (!editing) return;
    this.editing = null;
    editing.stop?.();
    editing.panel.setEditing?.(false);
    editing.overlay.remove();
    this.editButton?.setAttribute('aria-pressed', 'false');
    this.barKey = null;
    this.focus?.();
  }

  // Cards across windows (ADR 0032), and the front panels' context menu.

  /** What the app told this window (`signal-window`): a card another window holds drawn as a placeholder, the card menu's targets. */
  windowState(state) {
    this.windowTold = state;
    for (const s of this.stacks.values()) this.applyHeld(s);
  }

  /** The cards another window holds, by the app's card id: where. */
  heldCards() {
    const told = this.windowTold;
    if (told?.view !== 'main') return new Map();
    return new Map((told.held ?? []).map((h) => (typeof h === 'string' ? [h, ''] : [h.card, h.where ?? ''])));
  }

  /** Stack `s`'s cards as the app names them: its front panels (named when drawn) and the cards of its open slot cards' parts (named now: a part draws its cards later). */
  namedCards(s) {
    const out = [...(s.named?.values() ?? [])].filter((c) => [...s.cards.values()].some((e) => e === c.raw || e.contains?.(c.raw)));
    for (const element of s.cards.values()) {
      if (element.localName !== 'signal-slot') continue;
      for (const c of cardsOf(element)) if (!out.some((o) => o.raw === c.raw)) out.push(c);
    }
    return out;
  }

  /** Stack `s`'s held cards marked (`data-held`, PANEL_CSS), each with its placeholder: where it is, Show and Bring back. */
  applyHeld(s) {
    for (const note of s.shadow?.querySelectorAll?.('.held-note') ?? []) note.remove();
    const held = this.heldCards();
    for (const card of this.namedCards(s)) {
      const where = held.get(card.id);
      if (where === undefined) {
        card.raw.removeAttribute?.('data-held');
        continue;
      }
      card.raw.setAttribute('data-held', '');
      const note = html('div', { className: 'held-note' });
      // Its buttons are pressed, never a drag of the Stack.
      note.addEventListener('pointerdown', (event) => event.stopPropagation());
      const show = html('button', { type: 'button', textContent: 'Show' });
      const back = html('button', { type: 'button', textContent: 'Bring back' });
      show.addEventListener('click', () => this.cardChannel?.({ type: 'window.show', card: card.id }));
      back.addEventListener('click', () => this.cardChannel?.({ type: 'card.move', card: card.id, to: 'here' }));
      note.append(html('span', { textContent: `${card.title} — in ${where || 'another window'}` }), show, back);
      card.raw.append(note);
    }
  }

  /** The card a right click at `event` is on (`{raw, id, title}`), or null: not on a control bound to a parameter (it keeps its own menu). */
  cardAt(event) {
    const path = event.composedPath?.() ?? [];
    const name = this.stackAt(event);
    const s = name ? this.stacks.get(name) : null;
    if (!s) return null;
    const cards = this.namedCards(s);
    for (const node of path) {
      if (node?.binding?.address || node?.dataset?.address) return null;
      const card = cards.find((c) => c.raw === node);
      if (card) return card;
    }
    return null;
  }

  /**
   * The plane's items in the app's context menu (`contextMenu`, as
   * ui/app/app.js asks every element on the click's path): on a front
   * panel's card at the Stack level, Edit layout, and in Signal.app the
   * card's Move to items; at the Engine, Show Chip on a chip's card; Show Instrument and Show Stack on every layer; else none.
   */
  contextMenu(event) {
    const path = event.composedPath?.() ?? [];
    if (this.editing && path.includes(this.editing.overlay)) return null;
    const level = this.level();
    const name = this.stackAt(event);
    // The Deck's empty background: a new Stack there (the owner's, 2026-11-02).
    if (!name) {
      if (level !== 'setup' && level !== 'stack') return null;
      if (!this.model || path.some((node) => node?.classList?.contains?.('box') || node?.classList?.contains?.('tray'))) return null;
      const at = toPlane(this.view, this.point(event));
      return { items: [{ id: 'create-stack', label: 'Create new Stack' }], act: (id) => id === 'create-stack' && this.addStack(at) };
    }
    // The instrument whose front panel holds the click (on a control of it too).
    const s = this.stacks.get(name);
    // At the Engine: what the click is on (a card, a pin, a wire, inside a revealed chip too; else an instrument's or a chip's empty space).
    const hit = level === 'engine' && name === this.focused ? this.hitAt(this.point(event)) : null;
    const entry = [...(s?.cards ?? [])].find(([, element]) => path.includes(element))?.[0] ?? hit?.entry ?? null;
    const shows = [];
    // On a chip's card or anything inside it (the innermost chip under the click): Show Chip first.
    const chipId = hit?.kind === 'e-card' && hit.chipId ? hit.chipId : (hit?.chip ?? null);
    let chip = chipId ? s?.engineChips?.get(chipId) : null;
    // Else on an instrument's engine package (closed, or anything of its inside): that package, the top-level chip.
    if (!chip && hit?.entry) {
      const pkg = s?.engineChips?.get(`package ${hit.entry}`);
      const rect = this.focusRect();
      const at = rect ? toPlane(this.view, this.point(event)) : null;
      const c = pkg?.card;
      if (c && at && at.x - rect.x >= c.x && at.x - rect.x <= c.x + c.w && at.y - rect.y >= c.y && at.y - rect.y <= c.y + c.h) chip = pkg;
    }
    if (chip) shows.push({ id: 'show-chip', label: 'Show Chip' }, { id: 'separator' });
    // The Engine's own items: Add module… on empty space, Remove and Copy address on what is under the click (the edits with Edit on).
    const editing = this.engineEditing();
    const engine = [];
    if (editing && hit?.kind === 'e-space') engine.push({ id: 'add-module', label: 'Add module…' });
    if (editing && ['e-card', 'e-wire', 'e-edge'].includes(hit?.kind)) {
      const removal = removeOps(hit);
      engine.push({ id: 'remove', label: hit.kind === 'e-card' ? 'Remove' : 'Remove connection', disabled: Boolean(removal.error), ...(removal.error ? { title: removal.error } : {}) });
    }
    if (hit?.kind === 'e-card' || hit?.kind === 'e-pin') engine.push({ id: 'copy-address', label: 'Copy address' });
    if (engine.length) shows.push(...engine, { id: 'separator' });
    if (entry) shows.push({ id: 'show-instrument', label: 'Show Instrument' });
    shows.push({ id: 'show-stack', label: 'Show Stack' });
    const show = (id) => {
      if (id === 'show-chip') return this.showChip(chip), true;
      if (id === 'add-module') return this.openAddModule(hit), true;
      if (id === 'remove') return this.writeEngine(removeOps(hit)).then((ok) => ok && this.selection?.kind === 'engine' && this.selection.item.key === hit.key && this.select(null)), true;
      if (id === 'copy-address') return globalThis.navigator?.clipboard?.writeText(hit.address).catch(() => this.say('Not copied: the clipboard was refused')), true;
      if (id === 'show-instrument') return this.fitOnScreen(name, entry), true;
      if (id === 'show-stack') return this.fitOnScreen(name, null), true;
      return false;
    };
    // The Show in items' subject (ui/app/lib/show-in.js `subjectOf`): the instance, the pin or the wire under the click.
    const subject = engineSubject(hit);
    // A control bound to a parameter, or anything with an address (a slot card's port): the app's items first (Reset to default, Copy address, Show in), then these.
    if (path.some((node) => node?.binding?.address || node?.dataset?.address)) return { withControl: true, items: shows, act: show };
    const items = [...shows];
    const card = level === 'stack' || level === 'setup' ? this.cardAt(event) : null;
    if (card && this.cardChannel) items.push({ id: 'separator' }, ...cardMenu(this.windowTold, null, card.id));
    if (level === 'stack') items.push({ id: 'separator' }, { id: 'edit-layout', label: 'Edit layout…' });
    return {
      items,
      ...(subject ? { hit: subject } : {}),
      act: (id) => {
        if (show(id)) return;
        if (id === 'edit-layout') return this.editLayout();
        const element = card?.element ?? card?.raw;
        // The window a card needs: its own size (plate px, unscaled) with the panel page's bar and margins.
        const size = { width: Math.ceil((element?.offsetWidth || 320) + 24), height: Math.ceil((element?.offsetHeight || 200) + 72) };
        const message = card ? menuMessage(id, card.id, null, size) : null;
        if (message) this.cardChannel?.(message);
      },
    };
  }

  stackAt(event) {
    for (const node of event.composedPath?.() ?? []) if (node?.dataset?.stack) return node.dataset.stack;
    return null;
  }

  /** The drawing the pointer edits at this zoom: `wiring`, `control`, `engine`, or null (the more opaque one takes the press, ADR 0050 §6). */
  editedLevel() {
    const fades = this.fadesNow();
    if (fades.deep >= HIT_OPACITY) return this.stacks.get(this.focused)?.engines ? 'engine' : null;
    if (fades.sheet >= HIT_OPACITY && this.opened) return 'control';
    if (fades.board >= HIT_OPACITY) return 'wiring';
    return null;
  }

  /** The item of the edited drawing at screen point `at` (`hitItem`'s), with the drawing's name, or null. */
  hitAt(at) {
    const which = this.editedLevel();
    const s = this.stacks.get(this.focused);
    const rect = this.focusRect();
    if (!which || !s || !rect) return null;
    if (which === 'engine') {
      // Laid out for the graph now (a push since the last render draws it first), then: a card, a pin, a wire (inside a revealed chip too), else the empty space of a chip's body or an instrument.
      if (!this.enginesNow()) return null;
      const { items, revealed } = this.engineHitItems(s);
      const p = toPlane(this.view, at);
      const rects = s.engines.layouts.map((e) => ({ entry: e.entry, ...e.rect }));
      const hit = engineTarget(items, { x: p.x - rect.x, y: p.y - rect.y }, this.view.k, { revealed, rects });
      return hit ? { level: 'engine', ...hit } : null;
    }
    const items = which === 'control' ? s.controlItems : s.wiringItems;
    if (!items) return null;
    const p = toPlane(this.view, at);
    const hit = hitItem(items, { x: p.x - rect.x, y: p.y - rect.y }, this.view.k);
    return hit ? { level: which, ...hit.item } : null;
  }

  /** A press on an instrument's Control plane opens that instrument (its engine is the one the Engine layer shows). */
  adopt(hit) {
    if (!hit?.entry || hit.level !== 'control' || hit.entry === this.opened) return;
    this.opened = hit.entry;
    this.barKey = null;
  }

  /** The control at screen point `at` (its measured rectangle), or null. */
  controlAt(at) {
    const controls = [...(this.stacks.get(this.focused)?.controls?.values() ?? [])];
    const rect = this.focusRect();
    if (!controls.length || !rect) return null;
    const p = toPlane(this.view, at);
    const q = { x: p.x - rect.x, y: p.y - rect.y };
    return controls.flatMap((c) => c.controls).find((c) => q.x >= c.rect.x && q.x <= c.rect.x + c.rect.w && q.y >= c.rect.y && q.y <= c.rect.y + c.rect.h) ?? null;
  }

  /** Before a control sees a press: a pin, a via or a port under it starts a cable (ADR 0051 §9: patch cables at Wiring, automation and binds at Control). */
  pointerDownCapture(event) {
    if (this.overEditor(event)) return;
    if (event.button !== 0 || event.composedPath().includes(this.bar) || event.composedPath().includes(this.chooser)) return;
    this.closeChooser();
    const at = this.point(event);
    const hit = this.hitAt(at);
    const end = this.grabbedEnd(hit, at);
    if (!hit || !(DRAG_FROM.has(hit.kind) || (hit.kind === 'e-pin' && this.engineEditing()) || end)) return;
    this.adopt(hit);
    event.stopPropagation();
    event.preventDefault();
    this.settleTravel();
    this.gesture = { hit, id: event.pointerId, start: at, moved: false, end };
    this.stage.setPointerCapture?.(event.pointerId);
  }

  pointerDown(event) {
    if (this.overEditor(event)) return;
    if (this.gesture || event.button !== 0 || event.composedPath().includes(this.bar) || event.composedPath().includes(this.chooser) || this.onControl(event)) return;
    // At the Setup level a press on a cable's device end or a Stack's out pin drags a Setup cable (ADR 0038, 0051 §3).
    if (event.button === 0 && this.level() === 'setup' && this.model && !this.onControl(event)) {
      const grabbed = this.setupCableAt(toPlane(this.view, this.point(event)));
      if (grabbed) {
        this.settleTravel();
        this.cableDrag = { ...grabbed, id: event.pointerId, start: this.point(event), moved: false, target: null };
        this.stage.setPointerCapture?.(event.pointerId);
        event.preventDefault();
        return;
      }
    }
    this.settleTravel();
    const at = this.point(event);
    // A wire, a cable or a box of the edited drawing: selected (by its connection or port, kept at every zoom); the empty sheet clears it.
    const hit = this.hitAt(at);
    this.adopt(hit);
    if (hit?.kind === 'cube' && this.editedLevel() === 'control') {
      this.gesture = { hit, id: event.pointerId, start: at, moved: false };
      return;
    }
    if (this.editedLevel()) this.select(selectionOf(hit));
    this.drag = { start: at, last: at, id: event.pointerId, moved: false };
  }

  pointerMove(event) {
    if (this.cableDrag && event.pointerId === this.cableDrag.id) {
      const at = this.point(event);
      if (!this.cableDrag.moved && Math.hypot(at.x - this.cableDrag.start.x, at.y - this.cableDrag.start.y) < 3) return;
      this.cableDrag.moved = true;
      this.dragSetupCable(toPlane(this.view, at));
      return;
    }
    if (this.gesture && event.pointerId === this.gesture.id) {
      const at = this.point(event);
      if (!this.gesture.moved && Math.hypot(at.x - this.gesture.start.x, at.y - this.gesture.start.y) < 3) return;
      if (!this.gesture.moved) this.stage.setPointerCapture?.(event.pointerId);
      this.gesture.moved = true;
      // A wire's end grabbed: the rubber from its other end, which stays.
      const g = this.gesture;
      this.drawRubber(g.end ? (g.end === 'from' ? g.hit.points.at(-1) : g.hit.points[0]) : g.hit, at);
      this.drawHint(this.dropHint(g, this.hitAt(at), at));
      return;
    }
    if (!this.drag || event.pointerId !== this.drag.id) return;
    const at = this.point(event);
    const dx = at.x - this.drag.last.x;
    const dy = at.y - this.drag.last.y;
    this.drag.last = at;
    if (!this.drag.moved && Math.hypot(at.x - this.drag.start.x, at.y - this.drag.start.y) < 3) return;
    // Captured only once it drags: a press that does not move stays a click (a Stack's header focuses it).
    if (!this.drag.moved) this.stage.setPointerCapture?.(event.pointerId);
    this.drag.moved = true;
    // The layer chosen alone: the left button is each element's own action, never a pan (the owner's, 2026-11-02; the right or the middle button pans).
    if (this.layer) return;
    const level = this.level();
    const rect = this.focusRect();
    if (level === 'stack' && rect) this.setView(scrollStack(this.view, -dy, rect, this.viewport));
    else this.setView(panBy(this.view, dx, dy));
  }

  pointerUp(event) {
    if (this.cableDrag && event.pointerId === this.cableDrag.id) {
      this.stage.releasePointerCapture?.(event.pointerId);
      this.dropSetupCable(event);
      return;
    }
    if (this.gesture && event.pointerId === this.gesture.id) {
      const gesture = this.gesture;
      this.gesture = null;
      this.stage.releasePointerCapture?.(event.pointerId);
      this.clearRubber();
      const at = this.point(event);
      if (gesture.moved && gesture.end) this.dropWireEnd(gesture.hit, gesture.end, at);
      else if (gesture.moved) this.dropCable(gesture.hit, at);
      else this.select(selectionOf(gesture.hit));
      return;
    }
    if (!this.drag || event.pointerId !== this.drag.id) return;
    this.drag = null;
    this.stage.releasePointerCapture?.(event.pointerId);
  }

  /**
   * Which end of a wire a press grabs (the owner's, 2026-11-04: every wire rewired by hand, at every layer): a Wiring
   * cable between two instruments, a Control automation of the top level, an Engine wire with Edit on; the end nearer the
   * press (`'from'` or `'to'`), or null for anything else.
   */
  grabbedEnd(hit, at) {
    // A control's leg into the engine (the package's level): its card's end moves, the connector's stays.
    if (hit?.kind === 'e-bind' && this.engineEditing() && !hit.prefix) return 'to';
    // A cable through the Stack's edge: its end inside the Stack moves, the edge's stays (the Setup moves that one).
    if (hit?.kind === 'cable' && hit.cable.a.edge && hit.cable.b.edge) return null;
    if (hit?.kind === 'cable' && hit.cable.a.edge) return 'to';
    if (hit?.kind === 'cable' && hit.cable.b.edge) return 'from';
    // Inside a chip, an export's wire and its parameter's leg move either end: the inner one, or the chip's own pin's.
    const movable = hit?.kind === 'cable' || (hit?.kind === 'auto' && hit.auto?.to && !hit.auto.inner) || ((hit?.kind === 'e-wire' || hit?.kind === 'e-bind') && this.engineEditing());
    if (!movable || !hit.points?.length) return null;
    const rect = this.focusRect();
    if (!rect) return null;
    const p = toPlane(this.view, at);
    const q = { x: p.x - rect.x, y: p.y - rect.y };
    const d = (r) => Math.hypot(q.x - r.x, q.y - r.y);
    return d(hit.points[0]) < d(hit.points.at(-1)) ? 'from' : 'to';
  }

  /** A wire's end dropped (`grabbedEnd`): onto a pin that fits, the connection moved there in one patch (one undo). */
  dropWireEnd(hit, end, at) {
    const to = this.hitAt(at);
    if (hit.kind === 'e-wire') return this.writeEngine(rewireOps(hit, end, to, this.graph?.composites?.[hit.patchLevel]));
    if (hit.kind === 'e-bind' && hit.prefix) return this.dropInnerBind(hit, to, at, end);
    if (hit.kind === 'e-bind') return this.dropBindLeg(hit, to, at);
    if (hit.kind === 'cable' && (hit.cable.a.edge || hit.cable.b.edge)) return this.dropEdgeCable(hit.cable, to);
    const old = hit.kind === 'cable' ? hit.cable : hit.auto;
    let from = old.from;
    let into = old.to;
    if (hit.kind === 'cable') {
      const want = end === 'from' ? 'out' : 'in';
      if (to?.kind !== 'port' || to.port.side !== want) return this.say(`Not moved: drop the cable’s ${end === 'from' ? 'source on an output' : 'end on an input'} of an instrument`);
      if (end === 'from') from = to.port.address;
      else into = to.port.address;
    } else if (end === 'to') {
      const target = this.controlAt(at);
      const bind = target?.writes ? target.binds[0] : null;
      if (!bind) return this.say('Not moved: drop the automation’s end on a control that drives a parameter');
      into = bind;
    } else {
      const source = to?.kind === 'auto-pin' ? to.pin.address : to?.kind === 'external' ? to.pin.from : null;
      if (!source) return this.say('Not moved: drop the automation’s source on an automation pin of the engine or an external cable');
      from = source;
    }
    if (from === old.from && into === old.to) return this.say('Not moved: it is already there');
    return this.writePatch([removeConnection(old.from, old.to), addConnection(from, into)], `${old.from} → ${old.to} moved: ${from} → ${into}`);
  }

  /**
   * A control's leg at the Engine dropped (`grabbedEnd`): onto a parameter's pin the control binds it, onto a card of the
   * package's level a chooser of the card's parameters (`panel.write`, as the Control layer's via dropped on the connector).
   */
  dropBindLeg(hit, to, at) {
    const control = this.controlFor(hit.entry)?.controls.find((c) => c.binds.some((b) => norm(b) === norm(hit.address)));
    if (!control) return this.say(`Not bound: no control of this instrument binds ${hit.address}`);
    const bind = (address) => {
      this.opened = hit.entry;
      return this.rebind(control, address);
    };
    if (to?.kind === 'e-pin' && to.parameter && !to.prefix) return bind(to.address);
    if (to?.kind === 'e-card' && !to.prefix) {
      const inst = this.graph?.top?.instances?.find((i) => i.name === to.name);
      const list = (inst?.parameters ?? []).map((p) => p.address ?? `/${to.name}.${p.name}`);
      return this.openChooser(`Bind ${control.label || control.tag} to:`, list.map((a) => ({ text: a.replace(/^\//, ''), act: () => bind(a) })), at);
    }
    return this.say('Not bound: drop the leg on a card of the engine, or on a parameter’s pin');
  }

  /**
   * A cable through the Stack's edge with its inner end dropped (`grabbedEnd`): onto a port of the side it needs, every
   * connection it stands for moved there in one patch (one undo), their ends outside the Stack kept.
   */
  dropEdgeCable(cable, to) {
    const want = cable.b.edge ? 'out' : 'in';
    if (to?.kind !== 'port' || to.port.side !== want) return this.say(`Not moved: drop the cable’s end on ${want === 'out' ? 'an output' : 'an input'} of an instrument`);
    const inStack = (address) => this.model?.stackOf?.get(instanceOf(address)) === this.focused;
    const moves = edgeCableMoves(cable, this.graph?.top?.connections ?? [], inStack, to.port.address);
    if (!moves.length) return this.say('Not moved: it is already there');
    const ops = [...moves.map((m) => removeConnection(m.was.from, m.was.to)), ...moves.map((m) => addConnection(m.now.from, m.now.to))];
    const what = moves.length === 1 ? `${moves[0].was.from} → ${moves[0].was.to} moved: ${moves[0].now.from} → ${moves[0].now.to}` : `${moves.length} connections through the Stack’s edge moved to ${to.port.address}`;
    return this.writePatch(ops, what);
  }

  /**
   * A parameter export's leg inside a chip dropped: its inner end onto an inner parameter's pin, or a card of the chip's
   * level through a chooser of its parameters; its end on the chip's own pin (`end` `'from'`) onto another of its pins.
   */
  dropInnerBind(hit, to, at, end = 'to') {
    if (end === 'from') return to?.kind === 'e-pin' ? this.writeEngine(bindMoveOps(hit, to)) : this.say('Not moved: drop the chip’s end of the leg on another of the chip’s own pins');
    if (to?.kind === 'e-pin' && to.parameter && to.chip === hit.chip) return this.writeEngine(bindMoveOps(hit, to.local));
    if (to?.kind === 'e-card' && to.chip === hit.chip) {
      const inst = this.graph?.composites?.[hit.patchLevel]?.instances?.find((i) => i.name === to.name);
      const names = (inst?.parameters ?? []).map((p) => p.name);
      return this.openChooser(`${hit.prefix}.${hit.export} reaches:`, names.map((n) => ({ text: `${to.name}.${n}`, act: () => this.writeEngine(bindMoveOps(hit, `${to.name}.${n}`)) })), at);
    }
    return this.say('Not moved: drop the leg on a card inside the same chip, or on a parameter’s pin there');
  }

  /** The cable being drawn: from the pressed pin to the pointer (plane units, in the focused Stack). */
  drawRubber(hit, at) {
    const s = this.stacks.get(this.focused);
    const rect = this.focusRect();
    if (!s || !rect) return;
    const p = toPlane(this.view, at);
    const from = hit.kind === 'cube' ? { x: hit.x + hit.w, y: hit.y + hit.h / 2 } : { x: hit.x, y: hit.y };
    const line = s.rubber ?? svg('path', { class: 'rubber' });
    line.setAttribute('d', `M${from.x - 2} ${from.y - 2} L${p.x - rect.x - 2} ${p.y - rect.y - 2}`);
    // 2 px on screen at any zoom (the stage's CSS scale is no SVG transform: a non-scaling stroke would scale with it).
    line.style.strokeWidth = String(2 / this.view.k);
    if (!s.rubber) {
      s.rubber = line;
      s.rubberSvg.append(line);
    }
  }

  clearRubber() {
    for (const s of this.stacks.values()) {
      s.rubber?.remove();
      s.rubber = null;
      s.hint?.remove();
      s.hint = null;
    }
  }

  /**
   * Where the cable or the wire's end being dragged (`gesture`) would land if dropped now on `to` (`hitAt`'s at `at`): a
   * pin or a port `{x, y}`, a card, a box or a control `{x, y, w, h}` (plane units, in the focused Stack), or null where
   * it fits nothing. A hint only: the drop decides (and says why it refuses).
   */
  dropHint(g, to, at) {
    const point = (p) => ({ x: p.x, y: p.y });
    const box = (r) => ({ x: r.x, y: r.y, w: r.w, h: r.h });
    const control = () => {
      const c = this.controlAt(at);
      return c?.writes ? box(c.rect) : null;
    };
    const hit = g.hit;
    if (g.end) {
      const want = g.end === 'from' ? 'out' : 'in';
      if (hit.kind === 'cable') return to?.kind === 'port' && to.port.side === want ? point(to) : null;
      if (hit.kind === 'auto') return g.end === 'to' ? control() : to?.kind === 'auto-pin' || to?.kind === 'external' ? point(to) : null;
      // ADR 0054: an input that takes a connection already is no target (but the wire's own end).
      const taken = (pin) => pin?.side === 'in' && pin.wired && !(g.end === 'to' && pin.address === hit.to);
      if (hit.kind === 'e-wire') return to?.kind === 'e-pin' && to.side === want && !taken(to) && (to.chip === hit.chip || (hit.frame && to.cardChip === hit.chip)) ? point(to) : null;
      if (hit.kind === 'e-bind' && !hit.prefix) return to?.kind === 'e-pin' && to.parameter && !to.prefix ? point(to) : to?.kind === 'e-card' && !to.prefix ? box(to) : null;
      if (hit.kind === 'e-bind' && g.end === 'from') return to?.kind === 'e-pin' && to.side === 'in' && to.cardChip === hit.chip ? point(to) : null;
      if (hit.kind === 'e-bind') return to?.kind === 'e-pin' && to.parameter && to.chip === hit.chip ? point(to) : to?.kind === 'e-card' && to.chip === hit.chip ? box(to) : null;
      return null;
    }
    if (hit.kind === 'port') {
      if (to?.kind === 'port' && to.port.side !== hit.port.side && to.port.entry !== hit.port.entry) return point(to);
      return to?.kind === 'box' && to.entry !== hit.port.entry ? box(to) : null;
    }
    if (hit.kind === 'via') return to?.kind === 'param' ? point(to) : to?.kind === 'cube' ? box(to) : null;
    if (hit.kind === 'auto-pin' || hit.kind === 'external' || hit.kind === 'cube') return control();
    if (hit.kind === 'e-pin') {
      if (to?.kind === 'e-pin' && to !== hit && !(to.side === 'in' && to.wired && hit.side === 'out') && (to.side !== hit.side || to.cardChip === hit.chip || hit.cardChip === to.chip)) return point(to);
      return to?.kind === 'e-card' && to.chip === hit.chip && to.name !== hit.card ? box(to) : null;
    }
    return null;
  }

  /** The drop's hint (`dropHint`'s) drawn over the focused Stack: a ring round a pin, an outline round a card; none for null. */
  drawHint(shape) {
    const s = this.stacks.get(this.focused);
    if (!s) return;
    if (!shape) {
      s.hint?.remove();
      s.hint = null;
      return;
    }
    // In screen px at the zoom now, set off as the rubber is (2 units).
    const k = this.view.k;
    const style = `stroke-width: ${3 / k}`;
    const ring = 'w' in shape ? svg('rect', { class: 'drop-hint', x: shape.x - 2 - 4 / k, y: shape.y - 2 - 4 / k, width: shape.w + 8 / k, height: shape.h + 8 / k, rx: 6 / k, style }) : svg('circle', { class: 'drop-hint', cx: shape.x - 2, cy: shape.y - 2, r: 10 / k, style });
    s.hint?.remove();
    s.hint = ring;
    s.rubberSvg.append(ring);
  }

  /**
   * A cable dropped (ADR 0051 §9): at Wiring, an output onto an input of
   * another instrument connects them (`patch`), onto an instrument a
   * chooser offers its inputs; at Control, a source (the cube's automation
   * pin, an external cable's pin, the cube itself through a chooser of the
   * engine's outputs) onto a control connects it to the control's bound
   * parameter (automation), and a control's via onto a connector pin or the
   * cube binds the control to that parameter (`panel.write`).
   */
  dropCable(from, at) {
    const to = this.hitAt(at);
    if (from.level === 'engine') return this.dropEngineWire(from, to, at);
    if (from.level === 'wiring') {
      const source = from.kind === 'port' ? from.port : null;
      if (!source) return;
      if (to?.kind === 'port' && to.port.side !== source.side && to.port.entry !== source.entry) {
        const [a, b] = source.side === 'out' ? [source.address, to.port.address] : [to.port.address, source.address];
        return this.writePatch([addConnection(a, b)], `${a} → ${b}`);
      }
      const entry = to?.kind === 'box' ? to.entry : to?.kind === 'port' ? to.port.entry : null;
      if (!entry || entry === source.entry) return this.say('Not connected: drop the cable on another instrument’s input or box');
      if (source.side === 'out') return this.chooseInput(source.address, entry, at);
      return this.chooseOutput(entry, at, (a) => this.writePatch([addConnection(a, source.address)], `${a} → ${source.address}`), source.address);
    }
    // Control.
    if (from.kind === 'via') {
      const control = this.controlFor(from.entry)?.controls.find((c) => c.id === from.control);
      if (!control) return;
      if (to?.kind === 'param') return this.rebind(control, to.address);
      if (to?.kind === 'cube' && to.entry === from.entry) return this.chooseParameter(control, at);
      return this.say('Not bound: drop the leg on a pin of the engine’s connector, or on the engine');
    }
    const target = this.controlAt(at);
    if (!target) return this.say('No automation: drop the cable on a control');
    const bind = target.writes ? target.binds[0] : null;
    if (!bind) return this.say(`Not connected: ${target.label || target.tag} reads, it drives nothing`);
    if (from.kind === 'cube') return this.chooseOutput(from.entry ?? this.opened, at, (a) => this.writePatch([addConnection(a, bind)], `${a} → ${bind}`), bind);
    const source = from.kind === 'auto-pin' ? from.pin.address : from.kind === 'external' ? from.pin.from : null;
    if (!source) return;
    if (from.kind === 'auto-pin' && from.pin.address.split('/').length > 2) return this.say(`${source} is inside the engine: connect it there, in its Engine`);
    this.writePatch([addConnection(source, bind)], `${source} → ${bind}`);
  }

  // The Engine's edits (step 3, the owner's request of 2026-11-03), with the bar's Edit on: every one a `patch` at its level,
  // as the schematic's editor writes them (ui/lib/plane-engine-edit.js): the instrument's level is the project's top level,
  // a chip's inside its composite's level (the project's composite definition: every instance of it).

  /** Whether the Engine is edited now: the bar's Edit on, at the Engine. */
  engineEditing() {
    return Boolean(this.engineEdit) && this.level() === 'engine';
  }

  /** The bar's Edit at the Engine, on or off. */
  toggleEngineEdit(on = !this.engineEdit) {
    if (!on) this.leaveEngineEdit();
    else {
      this.engineEdit = true;
      this.barKey = null;
      this.stage?.toggleAttribute?.('data-engine-edit', true);
    }
    this.say(on ? 'Edit at the Engine: drag from a pin to a pin or a card to connect, Delete removes the selection, the menu adds a module' : '');
    this.render();
  }

  /** Edit at the Engine left (the switch, or another layer shown): no chooser or browser of it stays open. */
  leaveEngineEdit() {
    this.engineEdit = false;
    this.barKey = null;
    this.stage?.toggleAttribute?.('data-engine-edit', false);
    this.closeChooser();
    this.addBrowser?.close();
  }

  /** Writes an Engine edit (`{level, ops, what}` or `{error}`), saying what it reaches inside a chip. */
  writeEngine(edit) {
    if (!edit || edit.error) {
      if (edit?.error) this.say(edit.error);
      return Promise.resolve(false);
    }
    return this.writePatch(edit.ops, edit.what, 0, edit.level);
  }

  /** The file a level's patch writes, as the person reads it. */
  levelFile(level) {
    if (!level) return 'project.toml';
    const count = instanceCount(this.graph, level);
    return `composites/${level}.toml, ${count === 1 ? 'its one instance' : `every one of its ${count} instances`}`;
  }

  /** A wire from Engine pin `from` dropped on `to` (a pin: connected; a card of its level: a chooser of what fits there). */
  dropEngineWire(from, to, at) {
    if (to?.kind === 'e-pin') return this.writeEngine(connectOps(from, to));
    if (to?.kind === 'e-card' && to.name !== from.card) {
      if (to.chip !== from.chip) return this.say(`Not connected: ${to.address} is at another level than ${from.address}`);
      const options = dropTargets(levelData(this.graph, from.patchLevel), from, to);
      const title = from.side === 'out' ? `Connect ${from.address} to ${to.address}:` : `Into ${from.address} from ${to.address}:`;
      return this.openChooser(title, options.map((o) => ({ text: o.text, act: () => this.writeEngine({ level: from.patchLevel, ops: [addConnection(o.from, o.to)], what: `${fullAddress(from.prefix, o.from)} → ${fullAddress(from.prefix, o.to)}` }) })), at);
    }
    this.say('Not connected: drop the wire on a pin, or on a card of the same level');
  }

  /** Add module… at an Engine's empty space (`target`: `engineTarget`'s `e-space`): the schematic's Add module browser, then one patch. */
  openAddModule(target) {
    if (!this.engineEditing() || target?.kind !== 'e-space') return;
    if (!this.addBrowser) {
      this.addBrowser = new ModuleBrowser({
        add: (module, name) => this.addModule(module, name),
        closed: () => {
          this.placing = null;
          this.focus?.({ preventScroll: true });
        },
        graph: () => this.graph,
        defaultName: (module) => (this.placing ? defaultName(module, this.placing.names) : ''),
      });
      this.shadowRoot.append(this.addBrowser.build(EXPANDED));
    }
    this.placing = { target, names: namesOf(levelData(this.graph, target.patchLevel)) };
    const where = target.chip ? `inside ${target.prefix} (${this.levelFile(target.patchLevel)})` : `in ${instrumentTitle(target.entry, this.cards)}`;
    // The types asked once per state of the project's files (`graph.files`), as the schematic's editor asks them.
    const key = this.graph?.files ? `${projectKey(this.graph)}\n${JSON.stringify(this.graph.files)}` : null;
    const known = key !== null && this.modulesKnown?.key === key ? loadable(this.modulesKnown.entries) : null;
    this.addBrowser.open({ where, entries: known });
    if (known) return;
    const placing = this.placing;
    this.connection?.request?.({ type: 'modules', inside: true }).then(
      (modules) => {
        const entries = browserEntries(modules);
        this.modulesKnown = { key, entries };
        if (this.placing === placing) this.addBrowser.setEntries(loadable(entries));
      },
      (error) => {
        if (this.placing === placing) this.addBrowser.failed(error.message);
      },
    );
  }

  /** The browser's Add: an error text when the name is refused, else null (written; at an instrument's level kept in it until it is connected). */
  addModule(module, name) {
    const placing = this.placing;
    if (!placing || !module) return null;
    const error = nameError(name, placing.names);
    if (error) return error;
    this.placing = null;
    this.addBrowser.close();
    const { target } = placing;
    // An instance with no control and no connection is in no instrument (`assignInstruments`): this page keeps it in the one it was added to.
    if (!target.chip) this.placed.set(name, { entry: target.entry, seen: false });
    this.writeEngine(addOps(target, module, name)).then((ok) => {
      if (!ok && !target.chip && this.placed.get(name)?.entry === target.entry) this.placed.delete(name);
    });
    return null;
  }

  // Choosers: a small list at the pointer, one patch on a pick.

  chooseInput(from, entry, at) {
    const level = this.graph?.top;
    const names = new Set(engineOf(entry, this.model.owner));
    const shape = portIndex(level?.instances ?? []).get(from)?.port?.shape ?? 'signal';
    const [instance, port] = String(from).replace(/^\//, '').split('.');
    const targets = connectTargets(level, { instance, port, shape }).filter((t) => names.has(t.instance));
    this.openChooser(`Connect ${from} to ${instrumentTitle(entry, this.cards)}:`, targets.map((t) => ({ text: `${t.instance}.${t.port}${t.parameter ? ' (parameter)' : ''}`, act: () => this.writePatch([addConnection(from, `/${t.instance}.${t.port}`)], `${from} → /${t.instance}.${t.port}`) })), at);
  }

  chooseOutput(entry, at, pick, into = '') {
    const names = new Set(engineOf(entry, this.model.owner));
    const outs = [];
    // Only outputs of the target's shape (a parameter takes a signal).
    const wanted = portIndex(this.graph?.top?.instances ?? []).get(into)?.port?.shape ?? 'signal';
    const shapeOf = (shape) => (shape && typeof shape === 'object' ? 'vector' : shape);
    for (const i of this.graph?.top?.instances ?? []) {
      if (!names.has(i.name)) continue;
      for (const p of i.ports ?? []) if (p.direction === 'out' && p.address !== into && (!into || shapeOf(p.shape) === shapeOf(wanted))) outs.push(p.address);
    }
    this.openChooser(`From ${instrumentTitle(entry, this.cards)}’s outputs${into ? ` into ${into}` : ''}:`, outs.map((a) => ({ text: a.replace(/^\//, ''), act: () => pick(a) })), at);
  }

  chooseParameter(control, at) {
    const names = new Set(engineOf(this.opened, this.model.owner));
    const list = [];
    for (const i of this.graph?.top?.instances ?? []) if (names.has(i.name)) for (const p of i.parameters ?? []) list.push(p.address);
    this.openChooser(`Bind ${control.label || control.tag} to:`, list.map((a) => ({ text: a.replace(/^\//, ''), act: () => this.rebind(control, a) })), at);
  }

  openChooser(title, options, at) {
    this.closeChooser();
    if (!options.length) return this.say(`${title} nothing fits`);
    const list = html('div', { className: 'chooser', role: 'menu' });
    list.append(html('div', { className: 'chooser-title', textContent: title }));
    for (const o of options.slice(0, 200)) {
      const b = html('button', { type: 'button', textContent: o.text, role: 'menuitem' });
      b.addEventListener('click', () => {
        this.closeChooser();
        o.act();
      });
      list.append(b);
    }
    this.chooser = list;
    this.stage.append(list);
    // Inside the stage: it never scrolls, it stands where it fits.
    const w = list.offsetWidth;
    const h = list.offsetHeight;
    list.style.left = `${Math.max(8, Math.min(at.x, this.viewport.w - w - 8))}px`;
    list.style.top = `${Math.max(8, Math.min(at.y, this.viewport.h - h - 8))}px`;
    list.querySelector('button')?.focus();
  }

  closeChooser() {
    this.chooser?.remove();
    this.chooser = null;
  }

  // Edits: `patch` (connections) and `panel.write` (binds), through the existing paths; undo and redo for the patches.

  writePatch(ops, what, step = 0, level = '') {
    const asked = step === 0 ? this.session.request(level, ops) : step < 0 ? this.session.undo() : this.session.redo();
    if (!asked) return Promise.resolve(false);
    if (asked.refused) {
      this.say(asked.refused);
      return Promise.resolve(false);
    }
    const request = this.connection?.request?.(asked.message);
    if (!request) {
      this.session.refused({ message: 'Not connected' }, asked.pending);
      this.say('Not written: not connected');
      return Promise.resolve(false);
    }
    return request.then(
      (reply) => {
        this.session.written(reply, asked.pending);
        this.say(step < 0 ? 'Undone' : step > 0 ? 'Redone' : `Written: ${what} (${this.levelFile(level)})`);
        return true;
      },
      (error) => {
        const result = this.session.refused(error, asked.pending);
        this.say(result.cleared ?? `Not written: ${result.reason}`);
        return false;
      },
    );
  }

  /** Control `control` bound to `address` instead (`panel.write` of the panel with that one attribute changed). */
  async rebind(control, address) {
    if (control.binds.length === 1 && control.binds[0] === address) return;
    try {
      const generation = graphGeneration(this.graph);
      const rules = this.rules ?? (await loadWhitelist());
      const read = await this.request({ type: 'panel.read', file: 'main.html' });
      if (!read.exists) return this.say('Not bound: the project has no panel file');
      if (!read.editable) return this.say(`Not bound: ${read.reason ?? 'the panel is not editable'}`);
      const parsed = parseContent(read.html, rules, { mode: 'read' });
      if (parsed.error) return this.say(`Not bound: the panel is not one the editor writes (${parsed.error})`);
      const node = controlNode(parsed.tree, this.cards, this.opened, control);
      if (!node) return this.say('Not bound: the control is no longer in the panel');
      const changed = setAttributes(parsed.tree, node.id, { bind: address });
      if (changed.error) return this.say(`Not bound: ${changed.error}`);
      await this.request(named({ type: 'panel.write', file: 'main.html', html: serialize(changed.tree, rules), plate: read.plate ?? null, base: read.base }, generation));
      this.say(`${control.label || control.tag} now binds ${address} (ui/main.html)`);
      this.readPanel();
    } catch (error) {
      this.say(`Not bound: ${error.message}`);
    }
  }

  /** The selected cable or automation removed (`patch`); a leg, a port or an edge pin is not removed here. */
  deleteSelection() {
    const sel = this.selection;
    if (!sel) return false;
    if (sel.kind === 'engine') {
      if (!this.engineEditing()) return false;
      this.writeEngine(removeOps(sel.item, this.graph?.composites?.[sel.item?.patchLevel])).then((ok) => ok && this.select(null));
      return true;
    }
    if (sel.kind === 'cable') {
      if (sel.edge) {
        this.say('A cable through the Stack’s edge is the Setup level’s: remove it there');
        return true;
      }
      this.writePatch([removeConnection(sel.from, sel.to)], `${sel.from} → ${sel.to} removed`).then((ok) => ok && this.select(null));
      return true;
    }
    if (sel.kind === 'auto') {
      if (!sel.to) {
        this.say('Select an automation wire to remove it');
        return true;
      }
      if (sel.inner) {
        this.say(`${sel.from} → ${sel.to} is inside ${sel.level}: edit it in its Engine`);
        return true;
      }
      this.writePatch([removeConnection(sel.from, sel.to)], `${sel.from} → ${sel.to} removed`).then((ok) => ok && this.select(null));
      return true;
    }
    return false;
  }

  /** The selection (view state, by connection or address): zoom never changes it; each level marks its own drawing of it. */
  select(selection) {
    this.selection = selection;
    this.markSelection();
  }

  /** The sidebar drawn for the selection (ui/lib/plane-sidebar.js): hidden while nothing is selected. */
  renderSide() {
    if (!this.side) return;
    const model = planeSidebar(this.selection, { graph: this.graph, net: this.selNet });
    const close = html('button', { className: 'side-close', type: 'button', title: 'Close (Escape)', textContent: '×' });
    close.addEventListener('click', () => this.select(null));
    const head = html('header');
    head.append(html('h2', { textContent: model.title }));
    if (this.selection) head.append(close);
    const parts = [head];
    if (model.subtitle) parts.push(html('p', { className: 'side-sub', textContent: model.subtitle }));
    if (model.rows.length) {
      const dl = html('dl');
      for (const r of model.rows) dl.append(html('dt', { textContent: r.label }), html('dd', { textContent: r.value }));
      parts.push(dl);
    }
    for (const section of model.sections) {
      const box = html('section');
      box.append(html('h3', { textContent: `${section.title} · ${section.items.length + section.more}` }));
      for (const item of section.items) {
        const link = html('button', { className: 'side-link', type: 'button', textContent: item.label, title: `Select ${item.address}` });
        link.addEventListener('click', () => {
          this.select({ kind: 'address', address: item.address });
          this.showAddress(item.address);
        });
        box.append(link);
      }
      if (section.more) box.append(html('p', { className: 'side-sub', textContent: `and ${section.more} more` }));
      parts.push(box);
    }
    if (model.copy.length) {
      const copy = html('button', { className: 'side-action', type: 'button', textContent: model.copy.length === 1 ? 'Copy address' : `Copy ${model.copy.length} addresses` });
      copy.addEventListener('click', () => {
        navigator.clipboard?.writeText(model.copy.join('\n')).then(() => this.say('Copied'), () => this.say('The clipboard took nothing'));
      });
      parts.push(copy);
    }
    this.side.replaceChildren(...parts);
  }

  markSelection() {
    // The Engine's elements by key (an inside built later marks itself as it is drawn, `keyed`).
    // The selection's whole net (ui/lib/plane-net.js): every level and layer marks its part of it.
    this.selNet = selectionNet(this.graph, this.selection);
    this.selKeys = selectionKeys(this.selection, this.selNet);
    this.renderSide();
    for (const s of this.stacks.values()) {
      for (const layer of [s.wiringSvg, s.controlSvg, s.engineSvg]) {
        if (this.selNet) layer?.setAttribute('data-net', '');
        else layer?.removeAttribute('data-net');
      }
      for (const list of [s.engineKeyed, ...[...(s.insidesBuilt ?? [])].map((chip) => chip.keyed)]) {
        for (const element of list ?? []) element.classList?.toggle('selected', (element.planeKeys ?? [element.getAttribute('data-key')]).some((key) => this.selKeys.has(key)));
      }
    }
    for (const s of this.stacks.values()) {
      for (const elements of [s.wiringElements, s.controlElements]) {
        if (!elements) continue;
        for (const [id, element] of elements) element.classList?.toggle('selected', selectedId(this.selection, id, s, this.selNet));
      }
      // The packages' pins at every layer: a pin of the net marked; one only the net marks is let go otherwise.
      for (const list of Object.values(s.netMarks ?? {})) {
        for (const element of list) {
          const on = Boolean(this.selNet?.addresses.has(norm(element.netAddress)));
          if (on) element.classList?.add('selected');
          else if (element.netOnly) element.classList?.remove('selected');
        }
      }
    }
  }

  /** A pointer cancelled or its capture lost: no cable is left half drawn, no pan left running. */
  cancelPointer() {
    this.gesture = null;
    this.drag = null;
    this.clearRubber();
    if (this.cableDrag) {
      this.cableDrag.target?.rect?.classList.remove('target');
      this.ghost?.setAttribute('d', '');
      this.cableDrag = null;
    }
  }

  say(text) {
    clearTimeout(this.sayTimer);
    this.said?.remove();
    this.said = null;
    if (!text) return;
    this.said = html('p', { className: 'said', textContent: text });
    this.notes.append(this.said);
    this.sayTimer = setTimeout(() => {
      this.said?.remove();
      this.said = null;
    }, 6000);
  }

  doubleClick(event) {
    if (this.overEditor(event)) return;
    if (this.onControl(event)) return;
    const name = this.stackAt(event);
    if (name && this.level() === 'setup') this.focusStack(name);
  }

  /** The layer chosen alone: the wheel is the plane's everywhere on it, over a knob or a fader too (the owner's, 2026-11-02). */
  wheelCapture(event) {
    if (!this.layer || this.overEditor(event)) return;
    event.stopPropagation();
    event.preventDefault();
    this.wheelZoom(event, false);
  }

  /**
   * The wheel: at the Setup level a mouse's wheel and a pinch zoom about the
   * pointer, a trackpad's two-finger scroll pans; at the Stack level and
   * below the wheel scrolls the Stack and a pinch (or Ctrl+wheel) zooms.
   * Zooming in until the Stack fills the stage snaps to its fit (focus).
   */
  wheel(event) {
    if (this.overEditor(event)) return;
    if (!takesWheel(this, event)) return;
    event.preventDefault();
    this.wheelZoom(event, false);
  }

  /** The wheel gesture's kind (`WheelKinds`), classified once per event. */
  kindOf(event) {
    if (event.signalPlaneKind === undefined) event.signalPlaneKind = this.wheelKinds.kind(event, event.timeStamp);
    return event.signalPlaneKind;
  }

  wheelZoom(event, forced) {
    const kind = event.ctrlKey ? 'pinch' : this.kindOf(event);
    const level = this.level();
    const rect = this.focusRect();
    const mode = event.deltaMode ?? 0;
    if (this.layer && !forced) {
      // The layer chosen alone: the wheel does the same on every layer (the owner's, 2026-11-02). A mouse's wheel and a
      // pinch zoom about the pointer; a trackpad's two fingers pan; Shift with a mouse's wheel pans up and down.
      if (kind === 'trackpad') return this.setView(panBy(this.view, -wheelPixels(event.deltaX || 0, mode), -wheelPixels(event.deltaY || 0, mode)));
      if (kind === 'mouse' && (event.shiftKey || this.zoomLock)) return this.setView(panBy(this.view, 0, -wheelPixels(event.deltaY || event.deltaX || 0, mode)));
      // Locked, a pinch (or Ctrl with the wheel) zooms nothing either.
      if (this.zoomLock) return;
    } else if (level !== 'setup' && rect && kind !== 'pinch' && !forced) {
      // The Stack and below: the wheel scrolls (a trackpad's sideways travel pans, when zoomed past the width).
      const dy = wheelPixels(event.deltaY || 0, mode);
      const dx = kind === 'trackpad' ? wheelPixels(event.deltaX || 0, mode) : 0;
      this.settleTravel();
      this.setView(scrollStack(panBy(this.view, this.scaleRatio() > 1.0001 ? -dx : 0, 0), dy, rect, this.viewport));
      return;
    }
    if (kind === 'trackpad' && !forced) {
      const dx = wheelPixels(event.deltaX || 0, mode);
      const dy = wheelPixels(event.deltaY || 0, mode);
      this.setView(panBy(this.view, -dx, -dy));
      return;
    }
    const d = event.deltaY || (event.shiftKey ? event.deltaX : 0);
    if (!d) return;
    this.zoomBy(wheelFactor(d, { deltaMode: mode, pinch: kind === 'pinch' }), this.point(event));
  }

  /** A zoom by `factor` about screen point `at`: coming to a Stack's fit from below focuses it (its width fitted); below the control band the instrument under `at` is the one that opens. Never the selection. */
  zoomBy(factor, at) {
    if (this.layer) {
      // The layer is chosen alone: the zoom only scales, about `at` (no focus, no instrument opens, no fit stop).
      const fit = this.fitK() ?? fitScale(this.layout?.bounds ?? { w: 1, h: 1 }, this.viewport);
      // Only the Engine goes deep (into the chips); the other layers keep the earlier limit.
      const k = Math.min(fit * (this.layer === 'engine' ? DEEP_SCALE : MAX_LAYER_SCALE), Math.max(fit * MIN_SCALE, this.view.k * factor));
      this.lift = 0;
      this.setView(zoomAbout(this.view, k / this.view.k, at));
      return;
    }
    // The focus follows the pointer only before the focus band: inside it the other Stacks are fading, and another focus would jump their opacity and the ratio.
    if (this.ratio() < FOCUS_BAND[0]) this.focusUnder(at);
    const fit = this.fitK();
    const r0 = this.scaleRatio();
    // Past the engine band the zoom goes on, into the chips (still the Engine level), as far as DEEP_SCALE.
    const step = zoomStep({ view: this.view, opened: this.opened, lift: this.lift }, factor, at, { fit, rects: this.instrumentRects(), maxR: Math.max(this.engineBandNow()[1], DEEP_SCALE + LIFT) });
    if (step.opened !== this.opened) {
      this.opened = step.opened;
      this.barKey = null;
    }
    let next = step.view;
    if (fit && reachesFit(r0, next.k / fit)) {
      this.lift = 0;
      // The zoom stops at the fit about the pointer (no jump in scale), and the Stack glides to fill the width.
      const rect = this.focusRect();
      const stop = zoomAbout(this.view, fit / this.view.k, at, fit);
      this.view = stop;
      this.touched = true;
      this.travel(fitWidth(rect, this.viewport, stop));
      return;
    } else if (fit && next.k / fit >= 1) {
      // Zoomed past the fit: the Stack stays in view vertically.
      next = { ...next, ...clampTop(next, this.focusRect(), this.viewport) };
    }
    this.lift = step.lift ?? this.lift;
    this.setView(next);
  }

  /**
   * The keys, commands of the shared keymap (scope `plane`, ui/lib/keys.default.js;
   * rebindable in keys.toml): `plane.in` goes in a level, `plane.out` out;
   * `plane.whole` shows the whole plane; `plane.zoom_in` and `plane.zoom_out`
   * zoom about the centre; the line and page commands scroll the focused
   * Stack; at Wiring and Control `plane.delete` removes the selected cable
   * and `plane.undo`, `plane.redo` undo and redo the patches written there,
   * at the Engine with Edit on the same for its connections and instances;
   * inside a chip `plane.out` steps out one chip level.
   * Captured before the plane's parts see them, but nothing else is taken:
   * Space reaches the page's keys (ui/controls/keyboard.js), where it plays
   * and stops the transport (ADR 0039); a field being typed in keeps every
   * key, a button or link its Space and Enter.
   */
  key(event) {
    if (event.defaultPrevented) return;
    // Edit layout over the plane: its Escape closes it; the plane underneath takes no key.
    if (this.editing) {
      if (event.key === 'Escape') {
        event.preventDefault();
        this.closeEditLayout();
      }
      return;
    }
    const path = event.composedPath?.() ?? [];
    // The Add module browser keeps its keys (it closes on its own Escape).
    if (this.addBrowser?.holds(event)) return;
    const target = path[0] ?? event.target;
    if (isTextField(target)) return;
    if (event.key === 'Escape' && this.chooser) {
      event.preventDefault();
      this.closeChooser();
      return;
    }
    // The list of other devices' head is a button of the drawing.
    if ((event.key === 'Enter' || event.key === ' ') && path.some((n) => n?.dataset?.othersToggle !== undefined)) {
      event.preventDefault();
      this.toggleOthers();
      return;
    }
    if (isPressable(target) && (event.key === 'Enter' || event.key === ' ')) return;
    const command = keyCommand('plane', event)?.id ?? null;
    if (!command) return;
    const level = this.level();
    if (command === 'plane.undo' || command === 'plane.redo') {
      if (level === 'wiring' || level === 'control' || this.engineEditing()) {
        event.preventDefault();
        this.writePatch([], '', command === 'plane.redo' ? +1 : -1);
      }
      return;
    }
    if (command === 'plane.out' && this.selection && (level === 'wiring' || level === 'control' || level === 'engine')) {
      // A selection is let go first.
      event.preventDefault();
      this.select(null);
      return;
    }
    if (command === 'plane.out' && level === 'engine' && this.outOneChip()) {
      event.preventDefault();
      event.stopPropagation?.();
      return;
    }
    if (this.layer && (command === 'plane.out' || command === 'plane.in')) {
      const ids = LAYERS.map(([id]) => id);
      const next = ids[ids.indexOf(level) + (command === 'plane.in' ? 1 : -1)];
      event.preventDefault();
      event.stopPropagation?.();
      if (next && (next === 'setup' || this.focused)) this.chooseLayer(next);
      return;
    }
    if (command === 'plane.out') {
      event.preventDefault();
      if (level === 'engine') this.travelTo(CONTROL_BAND[1]);
      else if (level === 'control') this.travelTo(WIRING_BAND[1]);
      else if (level === 'wiring') this.focusStack(this.focused);
      else if (level === 'stack') this.fitSetup();
      return;
    }
    if (this.onControl(event)) return;
    if (command === 'plane.delete') {
      // At the Engine only its own selection, with Edit on (a Wiring cable marked there is removed at Wiring).
      const mine = level === 'wiring' || level === 'control' || (level === 'engine' && this.selection?.kind === 'engine');
      if (mine && this.deleteSelection()) event.preventDefault();
      return;
    }
    const centre = { x: this.viewport.w / 2, y: this.viewport.h / 2 };
    const rect = this.focusRect();
    const page = this.viewport.h * 0.875;
    const scroll = { 'plane.line_down': 80, 'plane.line_up': -80, 'plane.page_down': page, 'plane.page_up': -page }[command];
    if (command === 'plane.in') {
      event.preventDefault();
      if (level === 'setup' && this.focused) this.focusStack(this.focused);
      else if (level === 'stack') this.travelTo(WIRING_BAND[1]);
      else if (level === 'wiring') this.openAtCentre();
      else if (level === 'control') this.enterEngine();
    } else if (command === 'plane.whole') {
      event.preventDefault();
      this.fitSetup();
    } else if (command === 'plane.zoom_in' || command === 'plane.zoom_out') {
      event.preventDefault();
      this.zoomBy(command === 'plane.zoom_in' ? KEY_ZOOM : 1 / KEY_ZOOM, centre);
    } else if (scroll !== undefined && rect && level !== 'setup') {
      event.preventDefault();
      this.setView(scrollStack(this.view, scroll, rect, this.viewport));
    }
  }

  // Show in (the app's context menu): a bound address revealed on its front panel.

  /** The addresses the front panels bind or read. */
  shownAddresses() {
    const out = [];
    for (const s of this.stacks.values()) for (const card of s.cards.values()) for (const e of card.querySelectorAll('[bind]')) out.push(...bindAddresses(e));
    return out;
  }

  /** Focuses the Stack holding a control bound to one of `addresses` and scrolls it into view; returns the controls found. */
  reveal(addresses, whole = false) {
    // Shown just now (another tab): measured first, so the view is not computed for an empty stage.
    this.measure();
    const wanted = new Set(addresses);
    for (const [name, s] of this.stacks) {
      const controls = [...s.cards.values()].flatMap((card) => [...card.querySelectorAll('[bind]')]).filter((e) => bindAddresses(e).some((a) => wanted.has(a)));
      if (!controls.length) continue;
      const element = whole ? (controls[0].closest('.signal-card') ?? controls[0]) : controls[0];
      const rect = this.layout?.stacks.get(name);
      if (rect) {
        this.focused = name;
        this.barKey = null;
        const fitted = fitWidth(rect, this.viewport);
        const top = element.getBoundingClientRect();
        const host = s.host.getBoundingClientRect();
        // The control's y on the plane: its offset in the Stack's panels at the panels' own scale.
        const scale = host.width > 0 ? s.host.offsetWidth / host.width || 1 / this.view.k : 1 / this.view.k;
        const y = rect.y + (top.top - host.top) * scale;
        const view = scrollStack(fitted, y * fitted.k + fitted.y - this.viewport.h / 3, rect, this.viewport);
        this.travel(view, 0);
      }
      return whole ? controls.map((c) => c.closest('.signal-card') ?? c) : controls;
    }
    return [];
  }
}


/** The kinds of item a press drags a cable from, and those a click selects. */
const DRAG_FROM = new Set(['port', 'via', 'auto-pin', 'external']);
const SELECTABLE = new Set(['cable', 'auto', 'leg', 'port', 'param', 'via', 'auto-pin', 'external', 'edge', 'box', ...ENGINE_SELECTABLE]);

/** A hit as the selection keeps it: by connection, address or instrument, so every level draws its own representative. */
function selectionOf(hit) {
  if (!hit || !SELECTABLE.has(hit.kind)) return null;
  if (ENGINE_SELECTABLE.has(hit.kind)) return engineSelection(hit);
  if (hit.kind === 'cable') return { kind: 'cable', from: hit.cable.from, to: hit.cable.to, edge: Boolean(hit.cable.a.edge || hit.cable.b.edge) };
  if (hit.kind === 'auto') return { kind: 'auto', from: hit.auto.from, to: hit.auto.to, inner: hit.auto.inner, level: hit.auto.level };
  if (hit.kind === 'leg') return { kind: 'leg', address: hit.leg.address };
  if (hit.kind === 'port') return { kind: 'port', address: hit.port.address };
  if (hit.kind === 'param') return { kind: 'leg', address: hit.address };
  if (hit.kind === 'box') return { kind: 'instrument', entry: hit.entry };
  if (hit.kind === 'auto-pin' || hit.kind === 'external') return { kind: 'auto', from: hit.pin.address === hit.pin.from ? hit.pin.from : (hit.pin.from ?? hit.pin.address), to: null };
  return { kind: hit.kind, id: hit.id };
}

/** Whether drawn item `id` of Stack `s` draws `selection`, or a part of its net (`net`, ui/lib/plane-net.js `netOf`'s). */
function selectedId(selection, id, s, net = null) {
  if (!selection) return false;
  const item = s.wiringItems?.find((i) => i.id === id) ?? s.controlItems?.find((i) => i.id === id);
  if (!item) return false;
  if (net && inNet(item, net, s)) return true;
  switch (selection.kind) {
    case 'cable':
      return (item.kind === 'cable' && item.cable.from === selection.from && item.cable.to === selection.to) || (item.kind === 'auto' && item.auto.from === selection.from && item.auto.to === selection.to);
    case 'auto':
      return item.kind === 'auto' && item.auto.from === selection.from && (selection.to === null || item.auto.to === selection.to);
    case 'leg':
      return (item.kind === 'leg' && item.leg.address === selection.address) || (item.kind === 'param' && item.address === selection.address);
    case 'port':
      return item.kind === 'port' && item.port.address === selection.address;
    case 'instrument':
      return item.kind === 'box' && item.entry === selection.entry;
    default:
      return item.id === selection.id;
  }
}

/** Whether a Wiring or Control item is a part of `net`: a cable or an automation wire of it, a port, a leg, its via or its pin on it. */
function inNet(item, net, s) {
  const has = (address) => Boolean(address) && net.addresses.has(norm(address));
  switch (item.kind) {
    case 'cable':
      return net.wires.has(wireKey(item.cable.from, item.cable.to)) || (has(item.cable.from) && has(item.cable.to));
    case 'auto':
      return has(item.auto.from) || has(item.auto.to);
    case 'port':
      return has(item.port.address);
    case 'leg':
      return has(item.leg.address);
    case 'param':
      return has(item.address);
    case 'via':
      return Boolean(s.controlItems?.some((i) => i.kind === 'leg' && i.entry === item.entry && i.leg.control === item.control && has(i.leg.address)));
    case 'auto-pin':
      return has(item.pin.address);
    default:
      return false;
  }
}

/** A Stack layout's key: what the Wiring and Control layouts depend on. */
function layoutKey(layout) {
  return `${layout.w} ${layout.rects.map((r) => `${r.entry}@${r.y}:${r.h}:${r.panel.x}:${r.panel.w}`).join(' ')}`;
}

function opacity(element, value) {
  if (!element) return;
  const text = String(Math.round(value * 1000) / 1000);
  if (element.style.opacity !== text) element.style.opacity = text;
}

function hide(element) {
  if (element && element.style.display !== 'none') element.style.display = 'none';
}

function show(element) {
  if (element && element.style.display === 'none') element.style.display = '';
}

/**
 * The panel tree's node of `control` (measured on the opened card `entry`):
 * the card's node (by its index among the panel's cards), then its
 * controls of the same tag and `bind` in order, the one at the control's
 * place among the card's elements of that tag and bind.
 */
function controlNode(tree, cards, entry, control) {
  const card = cardOfEntry(entry, cards);
  const grid = (tree?.children ?? []).find((c) => c.kind === 'cards');
  const node = grid?.children?.[card?.index ?? -1];
  if (!node) return null;
  // Counted over the whole card, as the walk below counts its node.
  const cardElement = control.element.closest('.face-panel > .signal-cards > *') ?? control.element.closest('.signal-card, section, signal-slot');
  const same = [...(cardElement?.querySelectorAll(control.tag) ?? [])].filter((e) => e.getAttribute('bind') === control.element.getAttribute('bind'));
  const nth = Math.max(0, same.indexOf(control.element));
  let n = 0;
  const walk = (x) => {
    for (const child of x.children ?? []) {
      if (child.kind === 'control') {
        if (child.tag === control.tag && (child.attributes?.bind ?? '') === (control.element.getAttribute('bind') ?? '')) {
          if (n === nth) return child;
          n++;
        }
      }
      const found = walk(child);
      if (found) return found;
    }
    return null;
  };
  return walk(node);
}

/** A view zoomed past the fit keeps Stack `rect` covering the stage vertically where it can. */
function clampTop(view, rect, viewport) {
  if (!rect) return {};
  const top = rect.y * view.k + view.y;
  const height = rect.h * view.k;
  if (height < viewport.h - 2 * MARGIN_PX) return {};
  if (top > MARGIN_PX) return { y: view.y + MARGIN_PX - top };
  if (top + height < viewport.h - MARGIN_PX) return { y: view.y + viewport.h - MARGIN_PX - (top + height) };
  return {};
}

customElements.get('signal-plane') ?? customElements.define('signal-plane', SignalPlane);
