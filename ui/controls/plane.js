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
//     and the Setup's cables, peripherals and other Stacks fade to SURROUND;
//   - **Wiring** (phase 2): past the Stack, in the wiring band (1.5× to 2×
//     its fit), a cross-fade in place to the back of the rack: each
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
//   - **Engine**: zooming into the cube cross-fades to ADR 0047's board
//     (<signal-schematic>, its layer switch hidden) laid on the cube, the
//     instrument's chips filling it; past the band the board has the input,
//     and its deep zoom into dies works as before. Zooming back out of the
//     board's top level, or Escape there, returns to Control.
// The levels are nested fades of one zoom; no rectangle moves, and zoom
// never changes the selection.
// The Stacks are `ui/plane.toml` (served as /project/plane.toml), or one
// primary Stack derived from the panel's cards (ui/lib/plane-file.js); an
// instrument is a card of the panel (the migration form) drawn at the
// Stack's width (ui/lib/plane-layout.js), its controls live as on the
// panel: each card is the panel's own code (`showWritten`) in a shadow
// root with the panel's style sheets. Every level is its own layer (the
// Stacks' HTML, the Setup's SVG, each Stack's Wiring and Control SVG, the
// board), one transform for the view; each is laid out lazily, when it
// first shows, and cached; nothing is laid out per zoom frame. View state
// only: the plane writes no file of its own (rule 4); its edits go through
// the existing `patch` and `panel.write`.

import { onDefaultConnection } from '../signal-ui.js';
import './schematic.js';
import { loadWhitelist, showWritten } from './panel-editor.js';
import { generatedTree, parseContent, serialize } from '../lib/panel-tree.js';
import { setAttributes } from '../lib/panel-edit.js';
import { defaultPanel } from '../lib/panel.js';
import { faceAttribute, rootRules, stylesheets } from '../lib/schematic-face.js';
import { GUTTER, PU, parsePlace, parsePlate } from '../lib/plate.js';
import { CARD_PREFIX, PLANE_FILE, panelCards, planeOf } from '../lib/plane-file.js';
import { assignInstruments, edgePins, hasPart, instrumentTitle, roleEndpoints, stackMembers } from '../lib/plane-instruments.js';
import { cableDrop, channelSummary, endpointLabel, peripherals, playbackPortOf, setupCables, splitBoxes } from '../lib/plane-peripherals.js';
import { BOX_PU, OTHERS_PU, TEXT, TRAY_LINE_PU, capChars, boxPorts, cablePath, othersBoxes, pinPlaces, planeLayout, stackLayout, textSize, textSizes } from '../lib/plane-layout.js';
import { NOT_CONTROLS, READERS, bindList, fitText } from '../lib/schematic-controls.js';
import { CONTROL_BAND, ENGINE_BAND, FOCUS_BAND, LIFT, MARGIN_PX, WIRING_BAND, depthOf, engineBand, fitAll, fitScale, fitWidth, frameOnto, instrumentAt, levelOf, panBy, planeFades, planeFromFrame, reachesFit, screenRect, scrollStack, splitDepth, toPlane, zoomAbout, zoomStep } from '../lib/plane-zoom.js';
import { WIRING_TEXT, polylinePath, portIndex, wiringLayout, wiringModel } from '../lib/plane-wiring.js';
import { automations, controlLayout, engineOf, innerAddress } from '../lib/plane-control.js';
import { hitItem, visibleIds } from '../lib/plane-hit.js';
import { PatchSession } from '../lib/schematic-edit.js';
import { addConnection, connectTargets, removeConnection } from '../lib/schematic-patch.js';
import { formatValue } from '../lib/format.js';
import { toPosition } from '../lib/taper.js';
import { HIT_OPACITY, ZOOM_MS, ease, tween } from '../lib/board-zoom.js';
import { reroot } from '../lib/schematic-scene.js';
import { GESTURE_GAP_MS, WheelKinds } from '../lib/wheel.js';
import { wheelFactor, wheelPixels } from '../lib/schematic-view.js';
import { registerWheel, takesWheel } from './keyboard.js';
import { bindAddresses } from './base.js';

const SVG = 'http://www.w3.org/2000/svg';
/** The theme every panel links. */
const THEME = new URL('./theme.css', import.meta.url).href;
/** Keys that zoom and scroll, about the centre. */
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

const CSS = `
:host { display: block; position: relative; overflow: hidden; height: 100%; outline: none; overscroll-behavior: none;
  color: var(--signal-fg, #d6dbe4); background: var(--signal-board-bg, #0e1014); font: var(--signal-font, 11px/1.3 -apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif); }
.stage { position: absolute; inset: 0; overflow: hidden; touch-action: none; }
.world { position: absolute; left: 0; top: 0; transform-origin: 0 0; will-change: transform; }
.stack { position: absolute; box-sizing: border-box; background: #20242c; border: 2px solid #3a414d; border-radius: 6px; }
.stack.other { transition: none; }
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
.box.absent { fill: none; stroke-dasharray: 8 6; }
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
.deep { position: absolute; inset: 0; }
.deep[hidden] { display: none; }
.deep signal-schematic { height: 100%; }
.bar { position: absolute; left: 8px; top: 8px; display: flex; gap: 6px; align-items: center; padding: 4px 8px; border-radius: 6px;
  background: color-mix(in srgb, var(--signal-panel, #1f232b) 88%, transparent); border: 1px solid #343b47; user-select: none; }
.bar[hidden] { display: none; }
.bar .crumb { background: none; border: none; color: var(--signal-accent, #4fc3f7); padding: 0 2px; font: inherit; cursor: pointer; }
.bar .crumb[aria-current="true"] { color: var(--signal-fg, #d6dbe4); cursor: default; }
.bar .sep { color: var(--signal-muted, #8a93a3); }
.bar .level { color: var(--signal-muted, #8a93a3); margin-left: 6px; font-variant-numeric: tabular-nums; }
.notes { position: absolute; left: 8px; bottom: 8px; max-width: 60%; display: grid; gap: 2px; pointer-events: none; }
.notes p { margin: 0; padding: 3px 6px; border-radius: 4px; background: color-mix(in srgb, var(--signal-panel, #1f232b) 90%, transparent); font: 11px/1.4 ui-monospace, Menlo, monospace; }
.notes p.error { color: var(--signal-clip, #ff5252); }
.notes p.warning { color: var(--signal-effective, #ffb74d); }
/* ADR 0051 phase 2: the Wiring level (the back of the rack) and the Control plane (the sheet under a front panel), each Stack's, in its coordinates. */
svg.wiring, svg.control, svg.drag { position: absolute; left: 0; top: 0; width: 1px; height: 1px; overflow: visible; pointer-events: none; }
svg.wiring text, svg.control text { font-family: -apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif; }
.w-box { fill: #161a20; stroke: #3d4552; stroke-width: 1.5; vector-effect: non-scaling-stroke; }
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
.k-audio { stroke: var(--signal-accent, #4fc3f7); color: var(--signal-accent, #4fc3f7); }
.k-control { stroke: #b39ddb; color: #b39ddb; }
.k-events { stroke: var(--signal-meter-rms, #3ecf8e); color: var(--signal-meter-rms, #3ecf8e); }
.k-vector { stroke: var(--signal-effective, #ffb74d); color: var(--signal-effective, #ffb74d); }
.w-junction.k-audio { fill: var(--signal-accent, #4fc3f7); } .w-junction.k-control { fill: #b39ddb; } .w-junction.k-events { fill: var(--signal-meter-rms, #3ecf8e); } .w-junction.k-vector { fill: var(--signal-effective, #ffb74d); }
.c-sheet { fill: #0f1d17; stroke: #24412f; stroke-width: 1; vector-effect: non-scaling-stroke; }
.c-ribbon { fill: color-mix(in srgb, var(--signal-effective, #ffb74d) 12%, transparent); stroke: color-mix(in srgb, var(--signal-effective, #ffb74d) 45%, transparent); stroke-width: 1; vector-effect: non-scaling-stroke; }
.c-ribbon-label { fill: var(--signal-effective, #ffb74d); font-size: 9px; font-weight: 600; }
.c-leg { fill: none; stroke: var(--signal-effective, #ffb74d); stroke-width: 1; vector-effect: non-scaling-stroke; opacity: 0.45; color: var(--signal-effective, #ffb74d); }
.c-leg.reads { stroke: var(--signal-meter-rms, #3ecf8e); stroke-dasharray: 3 3; }
.c-via { fill: var(--signal-effective, #ffb74d); opacity: 0.6; }
.c-via.selected, .c-pin.selected { stroke: var(--signal-focus, #5aa9ff); stroke-width: 2; vector-effect: non-scaling-stroke; }
.c-auto { fill: none; stroke-width: 2.5; vector-effect: non-scaling-stroke; stroke-linejoin: round; }
.c-auto.inner { stroke-dasharray: 7 4; }
.c-arrow { fill: context-stroke; }
.c-cube-body { fill: #232832; stroke: #7a8496; stroke-width: 1.5; vector-effect: non-scaling-stroke; }
.c-cube-top { fill: #2d3440; }
.c-cube-title { fill: var(--signal-fg, #d6dbe4); font-size: 11px; font-weight: 600; letter-spacing: 0.08em; }
.c-pin { fill: var(--signal-effective, #ffb74d); }
.c-auto-pin { fill: #b39ddb; stroke: var(--signal-board-bg, #0e1014); stroke-width: 1.5; vector-effect: non-scaling-stroke; }
.c-auto-label { fill: var(--signal-muted, #8a93a3); font-size: 9px; }
.c-edge { fill: #b39ddb; stroke: var(--signal-board-bg, #0e1014); stroke-width: 2; vector-effect: non-scaling-stroke; }
.c-edge.edge-out { fill: var(--signal-effective, #ffb74d); }
.c-readout-text { fill: var(--signal-effective, #ffb74d); font-size: 9px; font-variant-numeric: tabular-nums; }
.c-readout-track { fill: #2a2f38; }
.c-readout-fill { fill: var(--signal-effective, #ffb74d); }
.rubber { fill: none; stroke: var(--signal-focus, #5aa9ff); stroke-width: 2; stroke-dasharray: 6 4; vector-effect: non-scaling-stroke; }
.chooser { position: absolute; z-index: 5; width: min(640px, calc(100% - 16px)); overflow: hidden; display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); padding: 4px; border-radius: 6px; background: var(--signal-panel, #1f232b); border: 1px solid #3a414d; box-shadow: 0 6px 20px #0008; }
.chooser-title { grid-column: 1 / -1; padding: 2px 6px 4px; color: var(--signal-muted, #8a93a3); }
.chooser button { text-align: left; background: none; border: none; color: var(--signal-fg, #d6dbe4); padding: 3px 6px; font: 11px/1.4 ui-monospace, Menlo, monospace; border-radius: 4px; cursor: pointer; }
.chooser button:hover, .chooser button:focus-visible { background: #2c3340; outline: none; }
.notes p.said { color: var(--signal-fg, #d6dbe4); }
@media (prefers-reduced-motion: reduce) { .world, .deep { transition: none; } }
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
`;

function svg(name, attributes = {}, text = null) {
  const node = document.createElementNS(SVG, name);
  for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, String(value));
  if (text !== null) node.textContent = text;
  return node;
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
    /** The depth past the Stack's fit at its scale (`depthOf`: the wiring band, view state). */
    this.lift = 0;
    /** What is selected at Wiring and Control (view state, by connection or address; zoom never changes it). */
    this.selection = null;
    /** The patches written from Wiring and Control, for undo and redo (the schematic's session model). */
    this.session = new PatchSession();
    this.graphKey = '';
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
    this.deep = html('div', { className: 'deep', hidden: true });
    this.schematic = document.createElement('signal-schematic');
    this.schematic.setAttribute('plane', '');
    // ADR 0051 §8: no Plate / X-ray / Board switch; the board is the Engine, laid on an instrument's cube.
    this.schematic.board?.restore?.({ layer: 'board' });
    this.deep.append(this.schematic);
    this.bar = html('nav', { className: 'bar' });
    this.notes = html('div', { className: 'notes', role: 'status' });
    this.stage.append(this.world, this.deep, this.bar, this.notes);
    root.append(html('style', { textContent: CSS }), this.stage);
    // The wheel: the plane's own, but the board's in the Engine (it zooms into dies there); zooming back
    // out of the board's top level comes back to the plane (captured before the board sees it).
    this.stage.addEventListener('wheel', (e) => this.wheelCapture(e), { passive: false, capture: true });
    this.stage.addEventListener('wheel', (e) => this.wheel(e), { passive: false });
    registerWheel(this, (event) => !event.composedPath().includes(this.deep) || this.deep.hidden);
    this.stage.addEventListener('pointerdown', (e) => this.pointerDownCapture(e), true);
    this.stage.addEventListener('pointerdown', (e) => this.pointerDown(e));
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
    this.cards = [];
    this.layout = null;
    this.panelPending = false;
    this.deepOwned = false;
    this.handoff = undefined;
    this.opened = null;
    this.lift = 0;
    this.selection = null;
    this.graphKey = '';
    this.closeChooser();
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

  /** The Setup and the device inventory as listed (never refreshed here: listing devices pauses the control thread). */
  readSetup() {
    const gen = this.generation;
    Promise.all([this.request({ type: 'setup.get' }).catch(() => null), this.request({ type: 'setup.devices' }).catch(() => null)]).then(([setup, devices]) => {
      if (gen !== this.generation) return;
      this.setup = setup;
      this.devices = devices;
      this.recompute();
    });
  }

  graphSeen(graph) {
    this.graph = graph;
    // What the Wiring and Control layouts depend on: the structure, never a value.
    this.graphKey = JSON.stringify([graph?.top?.instances?.map((i) => [i.name, i.module, i.ports?.length, i.parameters?.length]), graph?.top?.connections?.map((c) => [c.from, c.to]), Object.entries(graph?.composites ?? {}).map(([k, c]) => [k, c.connections?.map((x) => [x.from, x.to]), c.parameters?.map((x) => [x.name, x.to]), c.instances?.map((x) => [x.name, x.module, x.composite])])]);
    const cleared = this.session.graphSeen(graph);
    if (cleared?.cleared) this.say(cleared.cleared);
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
    this.cards = parsed.tree ? panelCards(parsed.tree) : [];
    for (const s of this.stacks.values()) s.key = null;
    this.recompute();
  }

  // The model: what is drawn, from the files.

  recompute() {
    if (!this.panel || !this.rules) return;
    const instances = this.graph?.top?.instances ?? [];
    const connections = this.graph?.top?.connections ?? [];
    const slots = instances.filter(hasPart).map((i) => i.name);
    const plateWidth = parsePlate(this.panel.plate)?.w ?? null;
    const cards = this.cards ?? [];
    const owner = assignInstruments({ instances, connections, cards });
    // The unstacked warning needs the plane: computed on the plane read (or derived), then checked with it.
    let shown = planeOf(this.planeText, cards, plateWidth, { slots });
    const members = stackMembers(shown.plane, owner, instances);
    if (!shown.derived) shown = planeOf(this.planeText, cards, plateWidth, { slots, unstacked: members.unstacked });
    const { stackOf, unstacked } = stackMembers(shown.plane, owner, instances);
    const pins = new Map(shown.plane.stacks.map((s) => [s.name, edgePins(s, owner, stackOf, connections)]));
    const endpoints = roleEndpoints(instances);
    const { boxes: all, bindings, main } = peripherals({ setup: this.setup, devices: this.devices, endpoints });
    const cables = setupCables(pins, bindings);
    // Whole boxes for the devices the project and the Setup use; the rest in one list (the operator's review of phase 1).
    const { full: boxes, others } = splitBoxes(all, { bindings, cables, main });
    this.model = { plane: shown.plane, derived: shown.derived, problems: shown.problems, cards, owner, stackOf, pins, endpoints, boxes, others, bindings, cables, unstacked, main };
    if (!this.focused || !shown.plane.stacks.some((s) => s.name === this.focused)) this.focused = shown.plane.stacks[0]?.name ?? null;
    const focusedStack = shown.plane.stacks.find((s) => s.name === this.focused);
    if (!focusedStack?.instruments.includes(this.opened)) this.opened = focusedStack?.instruments[0] ?? null;
    this.renderStacks();
    this.relayout();
    this.showNotes();
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
    head.append(html('span', { textContent: 'Stack' }), title, count);
    head.addEventListener('click', () => this.focusStack(name));
    const rack = html('div', { className: 'rack' });
    const host = html('div', { className: 'panels' });
    const shadow = host.attachShadow({ mode: 'open' });
    const links = html('div', { hidden: true });
    const roots = html('style', { textContent: this.rootCss });
    const note = html('p', { className: 'face-note' });
    shadow.append(links, html('style', { textContent: PANEL_CSS }), roots, note);
    // ADR 0051 phase 2: the Wiring level behind the panels, the Control plane under them, the cable being drawn over all.
    const wiringSvg = svg('svg', { class: 'wiring' });
    const controlSvg = svg('svg', { class: 'control' });
    const rubberSvg = svg('svg', { class: 'drag' });
    wiringSvg.style.display = 'none';
    controlSvg.style.display = 'none';
    element.append(rack, wiringSvg, controlSvg, host, rubberSvg, head);
    this.world.insertBefore(element, this.setupSvg);
    return { element, head, title, count, rack, host, shadow, links, roots, note, wiringSvg, controlSvg, rubberSvg, container: null, cards: new Map(), sizes: new Map(), key: null };
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
    const error = await showWritten(container, this.panel.html, (name, value) => faceAttribute(name, value, '', this.base));
    if (s.key !== key) return;
    s.note.textContent = error ? `ui/main.html: ${error}` : '';
    const grid = container.querySelector(':scope > .signal-cards');
    const elements = [...(grid?.children ?? [])];
    const byId = new Map();
    (this.cards ?? []).forEach((card, i) => elements[i] && byId.set(`${CARD_PREFIX}${card.id}`, { element: elements[i], card }));
    for (const element of elements) element.remove();
    for (const entry of stack.instruments) {
      const found = byId.get(entry);
      if (!found) continue;
      grid.append(found.element);
      s.cards.set(entry, found.element);
      this.cardResize.observe(found.element);
    }
    this.measureCards(s, stack);
    this.relayout();
  }

  /** Each card's natural size at its width (a placed card: its place's; else the Stack's), measured once per text and when it changes. */
  measureCards(s, stack) {
    const width = stack.width * PU;
    let changed = false;
    for (const [entry, element] of s.cards) {
      const card = (this.cards ?? []).find((c) => `${CARD_PREFIX}${c.id}` === entry);
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
    const patch = () => this.request({ type: 'patch', level: '', base: this.graph?.files?.[''] ?? null, ops });
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
      if (route) await this.request(route);
    } finally {
      if (route) this.readSetup();
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
    if (this.ratio() < CONTROL_BAND[0]) return ENGINE_BAND;
    const cube = this.controlNow()?.layout?.cube;
    const fit = this.fitK();
    if (!cube || !fit || !this.viewport.w) return ENGINE_BAND;
    const cubeFit = Math.min((this.viewport.w - 2 * MARGIN_PX) / cube.w, (this.viewport.h - 2 * MARGIN_PX) / cube.h);
    return engineBand(depthOf(cubeFit / fit, LIFT));
  }

  level(view = this.view) {
    return levelOf(this.ratio(view), this.engineBandNow());
  }

  /** The person (or a command) moved the view: from now on it is theirs, never fitted again by itself. */
  setView(view) {
    this.stopTravel();
    this.touched = true;
    this.view = view;
    this.render();
  }

  /** The view drawn: one transform, the fades of its ratio; each level's drawing laid out once, when it first shows. */
  render() {
    if (!this.world || !this.viewport.w) return;
    const v = this.view;
    this.world.style.transform = `translate(${v.x}px, ${v.y}px) scale(${v.k})`;
    this.renderText(v.k);
    const r = this.ratio();
    const engine = this.engineBandNow();
    const fades = planeFades(r, engine);
    const level = levelOf(r, engine);
    this.dataset.level = level;
    opacity(this.setupSvg, fades.setup);
    for (const [name, s] of this.stacks) {
      const focused = name === this.focused;
      opacity(s.element, focused ? 1 : fades.setup);
      // The focused Stack's front panels, card by card: the opened instrument's live controls come back at Control (x-ray).
      for (const [entry, card] of s.cards) {
        const opened = focused && entry === this.opened;
        const o = !focused ? 1 : opened ? fades.controls : fades.panels;
        opacity(card, o);
        const hidden = o < 0.002 ? 'hidden' : '';
        if (card.style.visibility !== hidden) card.style.visibility = hidden;
        const xray = opened && fades.control > 0;
        if (card.hasAttribute('data-xray') !== xray) card.toggleAttribute('data-xray', xray);
      }
      if (!focused) {
        hide(s.wiringSvg);
        hide(s.controlSvg);
      }
    }
    this.renderWiring(fades);
    this.renderControl(fades);
    this.renderDeep(fades, level);
    this.renderBar(level, r, fades);
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

  /** The focused Stack's Wiring drawing, or null (laid out and drawn once per graph and Stack layout). */
  wiringNow() {
    const stack = this.model?.plane.stacks.find((x) => x.name === this.focused);
    const s = this.stacks.get(this.focused);
    if (!stack || !s?.layout || !this.graph) return null;
    const pins = this.model.pins.get(stack.name) ?? [];
    // What the Wiring depends on: the structure, the Stack's layout, and who holds what (the panel's binds, the Stacks).
    const key = `${this.graphKey}\n${layoutKey(s.layout)}\n${s.key}\n${JSON.stringify([[...this.model.owner], [...this.model.stackOf], pins])}`;
    if (s.wiring?.key === key) return s.wiring;
    const t0 = performance.now();
    const instances = this.graph?.top?.instances ?? [];
    const connections = this.graph?.top?.connections ?? [];
    // Only the instruments laid out (measured): an entry without a rectangle has no ports to draw.
    const laid = { ...stack, instruments: s.layout.rects.map((r) => r.entry) };
    const model = wiringModel(laid, this.model.owner, this.model.stackOf, instances, connections, pins);
    const pinYs = new Map(pinPlaces(pins, s.layout.rects).map((p) => [`${p.side} ${p.address}`, p.y]));
    const layout = wiringLayout(model, s.layout, pinYs);
    s.wiring = { key, layout, pins: pinPlaces(pins, s.layout.rects), ms: performance.now() - t0 };
    this.drawWiring(s, s.wiring);
    return s.wiring;
  }

  drawWiring(s, wiring) {
    const { layout } = wiring;
    const root = svg('g', { transform: 'translate(-2 -2)' });
    const cables = svg('g', { class: 'w-cables' });
    const elements = new Map();
    const items = [];
    s.wiringGroups = new Map();
    for (const b of layout.boxes) {
      const g = svg('g', { 'data-entry': b.entry });
      const rect = svg('rect', { class: 'w-box', x: b.x, y: b.y, width: b.w, height: b.h, rx: 6 });
      g.append(rect, svg('text', { class: 'w-title', x: b.x + 16, y: b.y + 26 }, instrumentTitle(b.entry, this.cards).toUpperCase()));
      s.wiringGroups.set(b.entry, g);
      root.append(g);
      const id = `box ${b.entry}`;
      elements.set(id, rect);
      items.push({ kind: 'box', id, entry: b.entry, x: b.x, y: b.y, w: b.w, h: b.h });
    }
    for (const c of layout.cables) {
      const path = svg('path', { class: `w-cable k-${c.kind}`, d: polylinePath(c.points, 6), 'data-id': c.id });
      path.append(svg('title', {}, `${c.from} → ${c.to} (${c.kind})${c.b.edge || c.a.edge ? ', through the Stack’s edge' : ''}`));
      cables.append(path);
      for (const j of c.junctions) cables.append(svg('circle', { class: `w-junction k-${c.kind}`, cx: j.x, cy: j.y, r: 3.5 }));
      elements.set(c.id, path);
      items.push({ kind: 'cable', id: c.id, cable: c, points: c.points });
    }
    root.prepend(cables);
    for (const p of layout.ports) {
      const g = s.wiringGroups.get(p.entry) ?? root;
      const dot = svg('circle', { class: `w-port k-${p.kind}`, cx: p.x, cy: p.y, r: 5, 'data-id': p.id });
      dot.append(svg('title', {}, `${p.address} (${p.kind} ${p.side === 'in' ? 'input' : 'output'})`));
      const label = svg('text', { class: 'w-port-label', x: p.side === 'in' ? p.x + 10 : p.x - 10, y: p.y + 4, 'text-anchor': p.side === 'in' ? 'start' : 'end' }, p.label);
      g.append(dot, label);
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
    for (const [entry, g] of s.wiringGroups) opacity(g, entry === this.opened ? fades.openWiring : fades.board);
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

  /** The opened instrument's Control plane, or null (laid out once per graph, panel, Stack layout and instrument). */
  controlNow() {
    const entry = this.opened;
    const s = this.stacks.get(this.focused);
    const card = s?.cards.get(entry);
    const r = s?.layout?.rects.find((x) => x.entry === entry);
    if (!entry || !card || !r || !this.graph) return null;
    const key = `${this.graphKey}\n${layoutKey(s.layout)}\n${entry}\n${this.panel?.html?.length ?? 0}:${s.key}`;
    if (s.control?.key === key) return s.control;
    const measured = this.measureControls(card, r);
    if (!measured) return s.control?.entry === entry ? s.control : null;
    const t0 = performance.now();
    const wiring = this.wiringNow();
    const engine = engineOf(entry, this.model.owner);
    const binds = measured.controls.filter((c) => c.writes).flatMap((c) => c.binds);
    const autos = automations({ entry, binds, owner: this.model.owner, graph: this.graph });
    const inner = new Map();
    for (const b of binds) {
      const a = innerAddress(b, this.graph);
      if (a) inner.set(b, a);
    }
    // A cable from outside enters where the Wiring level's port of the same connection is.
    const edgeYs = new Map((wiring?.layout.ports ?? []).filter((p) => p.entry === entry && p.side === 'in').map((p) => [p.address, p.y]));
    const layout = controlLayout({ rect: { x: r.x, y: r.y, w: r.w, h: r.h }, controls: measured.controls, blocked: measured.blocked, engine, autos, edgeYs, inner, title: instrumentTitle(entry, this.cards) });
    s.control = { key, entry, layout, controls: measured.controls, autos, ms: performance.now() - t0 };
    this.drawControl(s, s.control);
    return s.control;
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
    const blocked = [...card.querySelectorAll(':scope > h1, :scope > h2')].map(at);
    return { controls, blocked };
  }

  drawControl(s, control) {
    const { layout } = control;
    const root = svg('g', { transform: 'translate(-2 -2)' });
    const r = layout.rect;
    const elements = new Map();
    const items = [];
    root.append(svg('rect', { class: 'c-sheet', x: r.x, y: r.y, width: r.w, height: r.h, rx: 4 }));
    // The legs' band: a ribbon under the lanes, labelled once.
    if (layout.legs.length >= 3) {
      const band = layout.band;
      const xs = layout.legs.flatMap((l) => l.points.map((p) => p.x));
      const x0 = Math.min(...xs);
      const x1 = Math.max(...xs);
      root.append(svg('rect', { class: 'c-ribbon', x: x0 - 4, y: band.y + 2, width: x1 - x0 + 8, height: band.h - 4, rx: 3 }));
      root.append(svg('text', { class: 'c-ribbon-label', x: x0, y: band.y - 2 }, `params ×${layout.legs.length}`));
    }
    for (const l of layout.legs) {
      const path = svg('path', { class: `c-leg${l.reads ? ' reads' : ''}`, d: polylinePath(l.points, 3) });
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
      const path = svg('path', { class: `c-auto k-${w.auto.kind}${w.auto.inner ? ' inner' : ''}${w.auto.source === 'external' ? ' external' : ''}`, d: polylinePath(w.points, 4), 'marker-end': 'url(#plane-arrow)' });
      path.append(svg('title', {}, `${w.auto.source === 'external' ? 'External control cable' : 'Automation'}: ${w.auto.from} → ${w.auto.to}${w.auto.inner ? ` (inside ${w.auto.level}: edited in its Engine)` : ''}`));
      root.append(path);
      elements.set(w.id, path);
      items.push({ kind: 'auto', id: w.id, auto: w.auto, points: w.points });
    }
    const c = layout.cube;
    const cube = svg('g', { class: 'c-cube' });
    cube.append(svg('rect', { class: 'c-cube-body', x: c.x, y: c.y, width: c.w, height: c.h, rx: 6 }));
    cube.append(svg('rect', { class: 'c-cube-top', x: c.x + 4, y: c.y + 4, width: c.w - 8, height: 24, rx: 3 }));
    cube.append(svg('text', { class: 'c-cube-title', x: c.x + 12, y: c.y + 21 }, fitText(`ENGINE · ${c.names.join(', ')}`, Math.max(4, Math.floor((c.w - 24) / 7)))));
    cube.append(svg('title', {}, `${layout.cube.title}'s engine: ${c.names.join(', ') || 'no instances'}. Zoom in to open it.`));
    root.append(cube);
    elements.set('cube', cube);
    items.push({ kind: 'cube', id: 'cube', x: c.x, y: c.y, w: c.w, h: c.h });
    for (const p of layout.pins) {
      const dot = svg('rect', { class: 'c-pin', x: p.x - 3, y: p.y - 3, width: 6, height: 6 });
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
    s.readouts = new Map();
    for (const ro of layout.readouts) {
      const g = svg('g', { class: 'c-readout' });
      const text = svg('text', { class: 'c-readout-text', x: ro.x + ro.w / 2, y: ro.y + 9, 'text-anchor': 'middle' }, 'inner —');
      const track = svg('rect', { class: 'c-readout-track', x: ro.x + 4, y: ro.y + 11, width: ro.w - 8, height: 2.5 });
      const fill = svg('rect', { class: 'c-readout-fill', x: ro.x + 4, y: ro.y + 11, width: 0, height: 2.5 });
      g.append(text, track, fill);
      g.append(svg('title', {}, `The inner value at ${ro.address}: what the engine plays, its own modulation included`));
      root.append(g);
      const element = control.controls.find((x) => x.id === ro.control)?.element ?? null;
      s.readouts.set(ro.id, { ...ro, text, fill, element, shown: '' });
    }
    const defs = svg('defs');
    const marker = svg('marker', { id: 'plane-arrow', viewBox: '0 0 8 8', refX: 7, refY: 4, markerWidth: 6, markerHeight: 6, orient: 'auto-start-reverse' });
    marker.append(svg('path', { d: 'M0 0 L8 4 L0 8 z', class: 'c-arrow' }));
    defs.append(marker);
    s.controlSvg.replaceChildren(defs, root);
    s.controlItems = items;
    s.controlElements = elements;
    s.controlShown = null;
    this.markSelection();
  }

  renderControl(fades) {
    const s = this.stacks.get(this.focused);
    if (!s) return;
    if (!(fades.sheet > 0) || !this.opened) {
      hide(s.controlSvg);
      this.updateInner([]);
      return;
    }
    const control = this.controlNow();
    if (!control) {
      hide(s.controlSvg);
      this.updateInner([]);
      return;
    }
    show(s.controlSvg);
    opacity(s.controlSvg, fades.sheet);
    this.cull(s, s.controlItems, s.controlElements, 'controlShown');
    this.updateInner(control.layout.readouts);
  }

  /** The inner values' stream (`parameters`, ADR 0050 §11): subscribed while a Control plane shows them, at most 64. */
  updateInner(readouts) {
    const addresses = [...new Set(readouts.map((r) => r.address))].slice(0, 64);
    const key = addresses.join('\t');
    if (this.innerStream?.key === key) return;
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

  // The Engine (ADR 0051 §8: Control → Engine): ADR 0047's board and its die zoom, laid on the cube.

  /** The opened instrument's chips on the board's top level (its coordinates), their union, or null. */
  engineFrame() {
    const names = new Set(this.controlNow()?.layout.cube.names ?? []);
    const layout = this.schematic.layoutOf?.('');
    const nodes = (layout?.nodes ?? []).filter((n) => n.kind === 'instance' && names.has(n.name));
    if (!nodes.length) return null;
    const x0 = Math.min(...nodes.map((n) => n.x));
    const y0 = Math.min(...nodes.map((n) => n.y));
    const x1 = Math.max(...nodes.map((n) => n.x + n.w));
    const y1 = Math.max(...nodes.map((n) => n.y + n.h));
    return { x: x0, y: y0, w: Math.max(1, x1 - x0), h: Math.max(1, y1 - y0) };
  }

  /** Where the board's frame is laid: the cube on the plane and on the screen, and the board's stage's offset. */
  engineMap(view = this.view) {
    const rect = this.focusRect();
    const c = this.controlNow()?.layout.cube;
    const frame = this.engineFrame();
    if (!rect || !c || !frame) return null;
    const cube = { x: rect.x + c.x, y: rect.y + c.y, w: c.w, h: c.h };
    const stage = this.schematic.stage?.getBoundingClientRect?.();
    const own = this.stage.getBoundingClientRect();
    const offset = stage ? { x: stage.left - own.left, y: stage.top - own.top } : { x: 0, y: 0 };
    return { frame, cube, onto: screenRect(view, cube), offset };
  }

  renderDeep(fades, level) {
    const shown = fades.deep > 0 && Boolean(this.opened);
    if (this.deep.hidden === shown) this.deep.hidden = !shown;
    if (!shown) {
      this.deepOwned = false;
      return;
    }
    opacity(this.deep, fades.deep);
    this.deep.style.pointerEvents = level === 'engine' ? '' : 'none';
    if (level === 'engine' && this.deepOwned) return;
    this.deepOwned = level === 'engine';
    const map = this.engineMap();
    if (map) {
      this.schematic.fitted = true;
      this.schematic.setView?.(frameOnto(map.frame, map.onto, map.offset));
    }
    // Just un-hidden, the board has no stage or layout yet: laid on the cube again once it has.
    if ((!this.schematic.viewport?.w || !map) && !this.deepPending) {
      this.deepPending = true;
      requestAnimationFrame(() => {
        this.deepPending = false;
        if (!this.deep.hidden) {
          this.deepOwned = false;
          this.render();
        }
      });
    }
  }

  /** The board's top-level view mapped back to the plane, the board given up (zooming out of it, Escape at its top). */
  leaveEngine() {
    const map = this.engineMap();
    const view = this.schematic.view;
    if (map && view) {
      const root = reroot(view, (l) => this.schematic.layoutOf(l), '');
      this.view = planeFromFrame(root, map.frame, map.cube, map.offset);
    }
    this.deepOwned = false;
  }

  /** Whether the board is at its top level (nothing zoomed into), so the plane may take it back. */
  boardAtTop() {
    return !(this.schematic.pinned?.size > 0) && (this.schematic.view?.path ?? '') === '';
  }

  renderBar(level, r, fades = planeFades(r)) {
    const key = `${level}|${this.focused}|${this.opened}`;
    if (key !== this.barKey) {
      this.barKey = key;
      const crumb = (text, current, act) => {
        const b = html('button', { type: 'button', className: 'crumb', textContent: text });
        b.setAttribute('aria-current', String(current));
        if (!current) b.addEventListener('click', act);
        return b;
      };
      const sep = () => html('span', { className: 'sep', textContent: '›' });
      const deep = ['wiring', 'control', 'engine'];
      const items = [crumb('Signal Project', level === 'setup', () => this.fitSetup())];
      if (level !== 'setup' && this.focused) items.push(sep(), crumb(`Stack ${this.focused}`, level === 'stack', () => this.focusStack(this.focused)));
      if (deep.includes(level)) items.push(sep(), crumb('Wiring', level === 'wiring', () => this.travelTo(WIRING_BAND[1])));
      if ((level === 'control' || level === 'engine') && this.opened) items.push(sep(), crumb(instrumentTitle(this.opened, this.cards), level === 'control', () => this.travelTo(CONTROL_BAND[1])));
      if (level === 'engine') items.push(sep(), crumb('Engine', true, () => {}));
      this.levelText = html('span', { className: 'level' });
      this.bar.replaceChildren(...items, this.levelText);
    }
    // The board has its own bar in the Engine: the plane's fades out as the board's comes in, so neither pops.
    this.bar.hidden = level === 'engine';
    opacity(this.bar, 1 - fades.deep);
    const name = { setup: 'Setup', stack: 'Stack', wiring: 'Wiring', control: 'Control', engine: 'Engine' }[level];
    const text = `${name} · ${r ? r.toFixed(2) : '0'}× fit`;
    if (this.levelText.textContent !== text) this.levelText.textContent = text;
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
    this.travel(fitWidth(rect, this.viewport, keep), 0);
  }

  /** The whole plane in view: the Setup level. */
  fitSetup() {
    if (this.layout) this.travel(fitAll(this.layout.bounds, this.viewport), 0);
  }

  /** The view zoomed about `at` (the stage's centre) to depth `z` (`depthOf`); the selection is untouched. */
  travelTo(z, at = { x: this.viewport.w / 2, y: this.viewport.h / 2 }) {
    const fit = this.fitK();
    if (!fit) return;
    if (this.level() === 'engine') this.leaveEngine();
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

  /** Control → Engine: the view zoomed until the cube fills the stage, centred on it. */
  enterEngine() {
    const map = this.engineMap();
    const fit = this.fitK();
    if (!map || !fit) return;
    const engine = this.engineBandNow();
    const c = { x: map.onto.x + map.onto.w / 2, y: map.onto.y + map.onto.h / 2 };
    const zoomed = zoomAbout(this.view, (fit * splitDepth(engine[1]).r) / this.view.k, c, fit, engine[1]);
    this.travel(panBy(zoomed, this.viewport.w / 2 - c.x, this.viewport.h / 2 - c.y), LIFT);
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

  stackAt(event) {
    for (const node of event.composedPath?.() ?? []) if (node?.dataset?.stack) return node.dataset.stack;
    return null;
  }

  /** The drawing the pointer edits at this zoom: `wiring`, `control`, or null (the more opaque one takes the press, ADR 0050 §6). */
  editedLevel() {
    const fades = planeFades(this.ratio(), this.engineBandNow());
    if (fades.deep >= HIT_OPACITY) return null;
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
    const items = which === 'control' ? s.controlItems : s.wiringItems;
    if (!items) return null;
    const p = toPlane(this.view, at);
    const hit = hitItem(items, { x: p.x - rect.x, y: p.y - rect.y }, this.view.k);
    return hit ? { level: which, ...hit.item } : null;
  }

  /** The control of the opened instrument at screen point `at` (its measured rectangle), or null. */
  controlAt(at) {
    const control = this.stacks.get(this.focused)?.control;
    const rect = this.focusRect();
    if (!control || !rect) return null;
    const p = toPlane(this.view, at);
    const q = { x: p.x - rect.x, y: p.y - rect.y };
    return control.controls.find((c) => q.x >= c.rect.x && q.x <= c.rect.x + c.rect.w && q.y >= c.rect.y && q.y <= c.rect.y + c.rect.h) ?? null;
  }

  /** Before a control sees a press: a pin, a via or a port under it starts a cable (ADR 0051 §9: patch cables at Wiring, automation and binds at Control). */
  pointerDownCapture(event) {
    if (event.button !== 0 || event.composedPath().includes(this.deep) || event.composedPath().includes(this.bar) || event.composedPath().includes(this.chooser)) return;
    this.closeChooser();
    const at = this.point(event);
    const hit = this.hitAt(at);
    if (!hit || !DRAG_FROM.has(hit.kind)) return;
    event.stopPropagation();
    event.preventDefault();
    this.settleTravel();
    this.gesture = { hit, id: event.pointerId, start: at, moved: false };
    this.stage.setPointerCapture?.(event.pointerId);
  }

  pointerDown(event) {
    if (this.gesture || event.button !== 0 || event.composedPath().includes(this.deep) || event.composedPath().includes(this.bar) || event.composedPath().includes(this.chooser) || this.onControl(event)) return;
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
    if (hit?.kind === 'cube' && this.editedLevel() === 'control') {
      this.gesture = { hit, id: event.pointerId, start: at, moved: false };
      return;
    }
    if (this.editedLevel()) this.select(hit && SELECTABLE.has(hit.kind) ? selectionOf(hit) : null);
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
      this.drawRubber(this.gesture.hit, at);
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
      if (gesture.moved) this.dropCable(gesture.hit, at);
      else this.select(SELECTABLE.has(gesture.hit.kind) ? selectionOf(gesture.hit) : null);
      return;
    }
    if (!this.drag || event.pointerId !== this.drag.id) return;
    this.drag = null;
    this.stage.releasePointerCapture?.(event.pointerId);
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
    if (!s.rubber) {
      s.rubber = line;
      s.rubberSvg.append(line);
    }
  }

  clearRubber() {
    for (const s of this.stacks.values()) {
      s.rubber?.remove();
      s.rubber = null;
    }
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
      const control = this.stacks.get(this.focused)?.control?.controls.find((c) => c.id === from.control);
      if (!control) return;
      if (to?.kind === 'param') return this.rebind(control, to.address);
      if (to?.kind === 'cube') return this.chooseParameter(control, at);
      return this.say('Not bound: drop the leg on a pin of the engine’s connector, or on the engine');
    }
    const target = this.controlAt(at);
    if (!target) return this.say('No automation: drop the cable on a control');
    const bind = target.writes ? target.binds[0] : null;
    if (!bind) return this.say(`Not connected: ${target.label || target.tag} reads, it drives nothing`);
    if (from.kind === 'cube') return this.chooseOutput(this.opened, at, (a) => this.writePatch([addConnection(a, bind)], `${a} → ${bind}`), bind);
    const source = from.kind === 'auto-pin' ? from.pin.address : from.kind === 'external' ? from.pin.from : null;
    if (!source) return;
    if (from.kind === 'auto-pin' && from.pin.address.split('/').length > 2) return this.say(`${source} is inside the engine: connect it there, in its Engine`);
    this.writePatch([addConnection(source, bind)], `${source} → ${bind}`);
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

  writePatch(ops, what, step = 0) {
    const asked = step === 0 ? this.session.request('', ops) : step < 0 ? this.session.undo() : this.session.redo();
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
        this.say(step < 0 ? 'Undone' : step > 0 ? 'Redone' : `Written: ${what} (project.toml)`);
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
      await this.request({ type: 'panel.write', file: 'main.html', html: serialize(changed.tree, rules), plate: read.plate ?? null, base: read.base });
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

  markSelection() {
    for (const s of this.stacks.values()) {
      for (const elements of [s.wiringElements, s.controlElements]) {
        if (!elements) continue;
        for (const [id, element] of elements) element.classList?.toggle('selected', selectedId(this.selection, id, s));
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
    if (event.composedPath().includes(this.deep) || this.onControl(event)) return;
    const name = this.stackAt(event);
    if (name && this.level() === 'setup') this.focusStack(name);
  }

  /** In the Engine the board zooms by itself; a zoom out of its top level comes back to the plane's Control. */
  wheelCapture(event) {
    if (this.deep.hidden || this.level() !== 'engine' || !event.composedPath().includes(this.deep)) return;
    const out = (event.ctrlKey || this.kindOf(event) !== 'trackpad') && event.deltaY > 0;
    if (!out || !this.boardAtTop()) return;
    const fit = this.fitK();
    const map = this.engineMap();
    if (!fit || !map) return;
    const root = reroot(this.schematic.view, (l) => this.schematic.layoutOf(l), '');
    const back = planeFromFrame(root, map.frame, map.cube, map.offset);
    if (depthOf(back.k / fit, LIFT) > this.engineBandNow()[1] * 1.0001) return;
    event.stopPropagation();
    event.preventDefault();
    this.leaveEngine();
    // The rest of this gesture is the plane's, zooming on out: one curve in and out.
    this.handoff = event.timeStamp;
    this.wheelZoom(event, true);
  }

  /**
   * The wheel: at the Setup level a mouse's wheel and a pinch zoom about the
   * pointer, a trackpad's two-finger scroll pans; at the Stack level and
   * below the wheel scrolls the Stack and a pinch (or Ctrl+wheel) zooms.
   * Zooming in until the Stack fills the stage snaps to its fit (focus).
   */
  wheel(event) {
    if (event.composedPath().includes(this.deep) && !this.deep.hidden && this.level() === 'engine') return;
    // A gesture handed back by the board (`wheelCapture`) goes on zooming here until it pauses.
    const handed = this.handoff !== undefined && event.timeStamp - this.handoff < GESTURE_GAP_MS;
    if (!handed && !takesWheel(this, event)) return;
    event.preventDefault();
    if (handed) this.handoff = event.timeStamp;
    this.wheelZoom(event, handed);
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
    if (level !== 'setup' && rect && kind !== 'pinch' && !forced) {
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
    // The focus follows the pointer only before the focus band: inside it the other Stacks are fading, and another focus would jump their opacity and the ratio.
    if (this.ratio() < FOCUS_BAND[0]) this.focusUnder(at);
    const fit = this.fitK();
    const r0 = this.scaleRatio();
    const step = zoomStep({ view: this.view, opened: this.opened, lift: this.lift }, factor, at, { fit, rects: this.instrumentRects(), maxR: this.engineBandNow()[1] });
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

  /** The keys: Enter goes in a level, Escape out; 0 fits the whole plane; + and − zoom about the centre; the arrows and the page keys scroll; at Wiring and Control, Delete removes the selected cable and ⌘Z, ⇧⌘Z undo and redo the patches written here. */
  key(event) {
    if (event.defaultPrevented || event.altKey) return;
    if (event.key === 'Escape' && this.chooser) {
      event.preventDefault();
      this.closeChooser();
      return;
    }
    if (event.metaKey) {
      const level = this.level();
      if ((level === 'wiring' || level === 'control') && (event.key === 'z' || event.key === 'Z') && !event.composedPath().includes(this.deep)) {
        event.preventDefault();
        this.writePatch([], '', event.shiftKey ? +1 : -1);
      }
      return;
    }
    if ((event.key === 'Enter' || event.key === ' ') && event.composedPath().some((n) => n?.dataset?.othersToggle !== undefined)) {
      event.preventDefault();
      this.toggleOthers();
      return;
    }
    const level = this.level();
    const inBoard = level === 'engine' && event.composedPath().includes(this.deep);
    if (event.key === 'Escape') {
      if (inBoard) {
        if (!this.boardAtTop() || this.schematic.selected?.length || this.schematic.tracing) return;
        event.preventDefault();
        event.stopPropagation();
        this.leaveEngine();
        this.travelTo(CONTROL_BAND[1]);
        return;
      }
      if (this.selection && (level === 'wiring' || level === 'control')) {
        event.preventDefault();
        this.select(null);
        return;
      }
      event.preventDefault();
      if (level === 'engine') {
        this.leaveEngine();
        this.travelTo(CONTROL_BAND[1]);
      } else if (level === 'control') this.travelTo(WIRING_BAND[1]);
      else if (level === 'wiring') this.focusStack(this.focused);
      else if (level === 'stack') this.fitSetup();
      return;
    }
    if (inBoard || level === 'engine' || this.onControl(event)) return;
    if ((event.key === 'Delete' || event.key === 'Backspace') && (level === 'wiring' || level === 'control')) {
      if (this.deleteSelection()) event.preventDefault();
      return;
    }
    const centre = { x: this.viewport.w / 2, y: this.viewport.h / 2 };
    const rect = this.focusRect();
    if (event.key === 'Enter') {
      event.preventDefault();
      if (level === 'setup' && this.focused) this.focusStack(this.focused);
      else if (level === 'stack') this.travelTo(WIRING_BAND[1]);
      else if (level === 'wiring') this.openAtCentre();
      else if (level === 'control') this.enterEngine();
    } else if (event.key === '0' && !event.ctrlKey) {
      event.preventDefault();
      this.fitSetup();
    } else if ((event.key === '+' || event.key === '=') && !event.ctrlKey) {
      event.preventDefault();
      this.zoomBy(KEY_ZOOM, centre);
    } else if ((event.key === '-' || event.key === '_') && !event.ctrlKey) {
      event.preventDefault();
      this.zoomBy(1 / KEY_ZOOM, centre);
    } else if (rect && level !== 'setup' && ['ArrowDown', 'ArrowUp', 'PageDown', 'PageUp', ' '].includes(event.key)) {
      event.preventDefault();
      const page = this.viewport.h * 0.875;
      const dy = { ArrowDown: 80, ArrowUp: -80, PageDown: page, PageUp: -page, ' ': event.shiftKey ? -page : page }[event.key];
      this.setView(scrollStack(this.view, dy, rect, this.viewport));
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
const SELECTABLE = new Set(['cable', 'auto', 'leg', 'port', 'param', 'via', 'auto-pin', 'external', 'edge', 'box']);

/** A hit as the selection keeps it: by connection, address or instrument, so every level draws its own representative. */
function selectionOf(hit) {
  if (hit.kind === 'cable') return { kind: 'cable', from: hit.cable.from, to: hit.cable.to, edge: Boolean(hit.cable.a.edge || hit.cable.b.edge) };
  if (hit.kind === 'auto') return { kind: 'auto', from: hit.auto.from, to: hit.auto.to, inner: hit.auto.inner, level: hit.auto.level };
  if (hit.kind === 'leg') return { kind: 'leg', address: hit.leg.address };
  if (hit.kind === 'port') return { kind: 'port', address: hit.port.address };
  if (hit.kind === 'param') return { kind: 'leg', address: hit.address };
  if (hit.kind === 'box') return { kind: 'instrument', entry: hit.entry };
  if (hit.kind === 'auto-pin' || hit.kind === 'external') return { kind: 'auto', from: hit.pin.address === hit.pin.from ? hit.pin.from : (hit.pin.from ?? hit.pin.address), to: null };
  return { kind: hit.kind, id: hit.id };
}

/** Whether drawn item `id` of Stack `s` draws `selection`. */
function selectedId(selection, id, s) {
  if (!selection) return false;
  const item = s.wiringItems?.find((i) => i.id === id) ?? s.controlItems?.find((i) => i.id === id);
  if (!item) return false;
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
  const card = cards.find((c) => `${CARD_PREFIX}${c.id}` === entry);
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
