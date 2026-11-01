// <signal-plane>: the Signal Project, one infinite plane (ADR 0051 phase
// 1). The zoom and the focused Stack decide the level (ui/lib/plane-zoom.js):
//   - **Setup**: every Stack as its column of live front panels, in
//     miniature; this machine's audio peripherals from the Setup and the
//     device inventory (`setup.get`, `setup.devices`), in our signal
//     direction; the role endpoints on the channels their role is bound to,
//     and the cable from the primary Stack's edge pin to them
//     (ui/lib/plane-peripherals.js);
//   - **Stack**: a click on a Stack's header, Enter, or zooming until it
//     fills the stage fits its width; the wheel then scrolls it vertically,
//     and the Setup's cables, peripherals and other Stacks fade to SURROUND;
//   - past the Stack, in the wiring band (1.5× to 2× its fit), a cross-fade
//     in place to ADR 0047's board (<signal-schematic>, its layer switch
//     hidden), laid on the Stack's panels; past the band the board has the
//     input, and its deep zoom into dies works as before. Zooming back out
//     of the board's top level, or Escape there, returns to the Stack.
// The Stacks are `ui/plane.toml` (served as /project/plane.toml), or one
// primary Stack derived from the panel's cards (ui/lib/plane-file.js); an
// instrument is a card of the panel (the migration form) drawn at the
// Stack's width (ui/lib/plane-layout.js), its controls live as on the
// panel: each card is the panel's own code (`showWritten`) in a shadow
// root with the panel's style sheets. Every level is its own layer (the
// Stacks' HTML, the Setup's SVG, the board), one transform for the view;
// nothing is laid out per zoom frame. View state only: the plane writes no
// file (rule 4); its text is the project's.

import { onDefaultConnection } from '../signal-ui.js';
import './schematic.js';
import { loadWhitelist, showWritten } from './panel-editor.js';
import { generatedTree, parseContent, serialize } from '../lib/panel-tree.js';
import { defaultPanel } from '../lib/panel.js';
import { faceAttribute, rootRules, stylesheets } from '../lib/schematic-face.js';
import { GUTTER, PU, parsePlace, parsePlate } from '../lib/plate.js';
import { CARD_PREFIX, PLANE_FILE, RAIL_PU, panelCards, planeOf } from '../lib/plane-file.js';
import { assignInstruments, edgePins, hasPart, instrumentTitle, roleEndpoints, stackMembers } from '../lib/plane-instruments.js';
import { endpointLabel, peripherals, playbackPortOf, setupCables } from '../lib/plane-peripherals.js';
import { boxPorts, cablePath, pinPlaces, planeLayout, stackLayout } from '../lib/plane-layout.js';
import { MARGIN_PX, WIRING_BAND, boardView, fitAll, fitScale, fitWidth, fromBoardView, levelOf, panBy, planeFades, reachesFit, scrollStack, zoomAbout } from '../lib/plane-zoom.js';
import { ZOOM_MS, ease, tween } from '../lib/board-zoom.js';
import { reroot } from '../lib/schematic-scene.js';
import { GESTURE_GAP_MS, WheelKinds } from '../lib/wheel.js';
import { wheelFactor, wheelPixels } from '../lib/schematic-view.js';
import { registerWheel, takesWheel } from './keyboard.js';

const SVG = 'http://www.w3.org/2000/svg';
/** The theme every panel links. */
const THEME = new URL('./theme.css', import.meta.url).href;
/** Keys that zoom and scroll, about the centre. */
const KEY_ZOOM = 1.25;

const CSS = `
:host { display: block; position: relative; overflow: hidden; height: 100%; outline: none; overscroll-behavior: none;
  color: var(--signal-fg, #d6dbe4); background: var(--signal-board-bg, #0e1014); font: var(--signal-font, 11px/1.3 -apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif); }
.stage { position: absolute; inset: 0; overflow: hidden; touch-action: none; }
.world { position: absolute; left: 0; top: 0; transform-origin: 0 0; will-change: transform; }
.stack { position: absolute; box-sizing: border-box; background: #20242c; border: 2px solid #3a414d; border-radius: 6px; }
.stack.other { transition: none; }
.stack-head { position: absolute; left: 0; right: 0; top: 0; display: flex; align-items: center; gap: 12px; padding: 0 16px; box-sizing: border-box;
  font-size: 18px; letter-spacing: 0.18em; text-transform: uppercase; color: var(--signal-muted, #8a93a3); cursor: zoom-in; user-select: none; border-bottom: 1px solid #3a414d; }
.stack-head b { color: var(--signal-fg, #d6dbe4); font-weight: 600; }
.stack-head .count { margin-left: auto; font-size: 13px; letter-spacing: 0.08em; }
.rack { position: absolute; left: 0; top: 0; pointer-events: none; }
.plate-blank { position: absolute; box-sizing: border-box; background: repeating-linear-gradient(135deg, #262a32 0 6px, #22262d 6px 12px); border-top: 1px solid #343a45; border-bottom: 1px solid #14161b; }
.plate-blank.wide { outline: 2px dashed var(--signal-effective, #ffb74d); outline-offset: -2px; }
.panels { position: absolute; left: 0; top: 0; }
svg.setup { position: absolute; overflow: visible; pointer-events: none; }
svg.setup text { font-family: -apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif; }
.cable { fill: none; stroke: var(--signal-accent, #4fc3f7); stroke-width: 2.5; vector-effect: non-scaling-stroke; stroke-linecap: round; }
.cable.dashed { stroke-dasharray: 6 5; }
.pin { fill: var(--signal-accent, #4fc3f7); stroke: var(--signal-board-bg, #0e1014); stroke-width: 2; vector-effect: non-scaling-stroke; }
.pin.in { fill: var(--signal-meter-rms, #3ecf8e); }
.pin-label { fill: var(--signal-fg, #d6dbe4); font-size: 28px; }
.box { fill: #1b1f26; stroke: #4a5260; stroke-width: 2; vector-effect: non-scaling-stroke; }
.box.absent { fill: none; stroke-dasharray: 8 6; }
.box-title { fill: var(--signal-fg, #d6dbe4); font-size: 34px; font-weight: 600; }
.box-note { fill: var(--signal-clip, #ff5252); font-size: 24px; }
.box-kind { fill: var(--signal-muted, #8a93a3); font-size: 20px; letter-spacing: 0.12em; }
.port { fill: #232832; stroke: #39404c; stroke-width: 1; vector-effect: non-scaling-stroke; }
.port.bound { stroke: var(--signal-accent, #4fc3f7); stroke-width: 1.5; }
.port-name { fill: var(--signal-fg, #d6dbe4); font-size: 28px; }
.port-role { fill: var(--signal-accent, #4fc3f7); font-size: 24px; font-weight: 600; }
.jack { fill: #0e1014; stroke: var(--signal-muted, #8a93a3); stroke-width: 1.5; vector-effect: non-scaling-stroke; }
.jack.bound { stroke: var(--signal-accent, #4fc3f7); fill: var(--signal-accent, #4fc3f7); }
.endpoint { fill: none; stroke: var(--signal-muted, #8a93a3); stroke-width: 1.5; stroke-dasharray: 5 4; vector-effect: non-scaling-stroke; }
.endpoint-text { fill: var(--signal-muted, #8a93a3); font-size: 26px; }
.tray { fill: #181b21; stroke: #4a5260; stroke-dasharray: 6 4; stroke-width: 1.5; vector-effect: non-scaling-stroke; }
.tray-text { fill: var(--signal-muted, #8a93a3); font-size: 26px; }
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
    // ADR 0051 §8: no Plate / X-ray / Board switch; the board alone lies under the plane's front panels.
    this.schematic.board?.restore?.({ layer: 'board' });
    this.deep.append(this.schematic);
    this.bar = html('nav', { className: 'bar' });
    this.notes = html('div', { className: 'notes', role: 'status' });
    this.stage.append(this.world, this.deep, this.bar, this.notes);
    root.append(html('style', { textContent: CSS }), this.stage);
    // The wheel: the plane's own, but the board's past the wiring band (it zooms into dies there); zooming back
    // out of the board's top level comes back to the plane (captured before the board sees it).
    this.stage.addEventListener('wheel', (e) => this.wheelCapture(e), { passive: false, capture: true });
    this.stage.addEventListener('wheel', (e) => this.wheel(e), { passive: false });
    registerWheel(this, (event) => !event.composedPath().includes(this.deep) || this.deep.hidden);
    this.stage.addEventListener('pointerdown', (e) => this.pointerDown(e));
    this.stage.addEventListener('pointermove', (e) => this.pointerMove(e));
    this.stage.addEventListener('pointerup', (e) => this.pointerUp(e));
    this.stage.addEventListener('pointercancel', () => (this.drag = null));
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
    for (const s of this.stacks.values()) s.element.remove();
    this.stacks.clear();
    this.fetchAll();
  }

  fetchAll() {
    this.readPanel();
    this.readSetup();
    this.request({ type: 'graph' }).then(
      (graph) => this.graphSeen(graph),
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
    Promise.all(own.map((href) => fetch(href, { cache: 'no-store' }).then((r) => (r.ok ? r.text() : ''), () => ''))).then((texts) => {
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
    const { boxes, bindings } = peripherals({ setup: this.setup, devices: this.devices, endpoints });
    const cables = setupCables(pins, bindings);
    this.model = { plane: shown.plane, derived: shown.derived, problems: shown.problems, cards, owner, pins, endpoints, boxes, bindings, cables, unstacked };
    if (!this.focused || !shown.plane.stacks.some((s) => s.name === this.focused)) this.focused = shown.plane.stacks[0]?.name ?? null;
    this.renderStacks();
    this.relayout();
    this.showNotes();
  }

  showNotes() {
    const problems = this.model?.problems ?? [];
    this.notes.replaceChildren(...problems.map((p) => html('p', { className: p.severity, textContent: p.message })));
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
    element.append(rack, host, head);
    this.world.insertBefore(element, this.setupSvg);
    return { element, head, title, count, rack, host, shadow, links, roots, note, container: null, cards: new Map(), sizes: new Map(), key: null };
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
    this.layout = planeLayout({ stacks, boxes: this.model.boxes, tray: this.model.unstacked.length });
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
    for (const stack of this.model.plane.stacks) {
      const s = this.stacks.get(stack.name);
      const rect = this.layout.stacks.get(stack.name);
      if (!s?.layout || !rect) continue;
      for (const pin of pinPlaces(this.model.pins.get(stack.name) ?? [], s.layout.rects)) {
        const x = rect.x + (pin.side === 'out' ? rect.w : 0);
        const y = rect.y + pin.y;
        pinAt.set(`${stack.name} ${pin.side} ${pin.address}`, { x, y });
        const g = svg('g', { 'data-pin': pin.address });
        g.append(svg('circle', { class: `pin ${pin.side}`, cx: x, cy: y, r: 10 }));
        const label = `${instrumentTitle(pin.instrument, this.cards)} · ${pin.label}`;
        g.append(svg('text', { class: 'pin-label', x: pin.side === 'out' ? x + 18 : x - 18, y: y + 40, 'text-anchor': pin.side === 'out' ? 'start' : 'end' }, label));
        groups.pins.append(g);
      }
    }
    const portAt = new Map();
    for (const b of this.model.boxes) {
      const r = this.layout.boxes.get(b.key);
      if (!r) continue;
      const g = svg('g', { 'data-device': b.key });
      g.append(svg('rect', { class: `box${b.present ? '' : ' absent'}`, x: r.x, y: r.y, width: r.w, height: r.h, rx: 10 }));
      g.append(svg('text', { class: 'box-kind', x: r.x + 20, y: r.y + 32 }, 'AUDIO DEVICE'));
      g.append(svg('text', { class: 'box-title', x: r.x + 20, y: r.y + 72 }, b.name));
      if (b.note) g.append(svg('text', { class: 'box-note', x: r.x + r.w - 20, y: r.y + 32, 'text-anchor': 'end' }, b.note));
      const ports = boxPorts(b);
      // Each edge's ports in its own half: playback (our inputs) on the left, capture (our outputs) on the right.
      const half = (ports.inputs.length && ports.outputs.length ? r.w / 2 : r.w) - 9;
      const bound = this.model.bindings.filter((x) => x.device === b.key);
      for (const p of ports.inputs) {
        const roles = bound.filter((x) => x.channels.some((c) => p.channels.includes(c)));
        g.append(svg('rect', { class: `port${roles.length ? ' bound' : ''}`, x: r.x + 6, y: r.y + p.y + 3, width: half, height: p.h - 6, rx: 4 }));
        g.append(svg('text', { class: 'port-name', x: r.x + 28, y: r.y + p.y + 36 }, p.name));
        if (roles.length) g.append(svg('text', { class: 'port-role', x: r.x + 28, y: r.y + p.y + 66 }, roles.map(endpointLabel).join(', ')));
        for (const pin of p.pins) {
          portAt.set(`${b.key} ${pin.channel}`, { x: r.x, y: r.y + pin.y });
          const on = roles.some((x) => x.channels.includes(pin.channel));
          // A row of many channels draws only the jacks a role uses.
          if (!p.rest || on) g.append(svg('circle', { class: `jack${on ? ' bound' : ''}`, cx: r.x, cy: r.y + pin.y, r: 8 }));
        }
      }
      for (const p of ports.outputs) {
        g.append(svg('rect', { class: 'port', x: r.x + r.w - 6 - half, y: r.y + p.y + 3, width: half, height: p.h - 6, rx: 4 }));
        g.append(svg('text', { class: 'port-name', x: r.x + r.w - 28, y: r.y + p.y + 36, 'text-anchor': 'end' }, p.name));
        if (!p.rest) for (const pin of p.pins) g.append(svg('circle', { class: 'jack', cx: r.x + r.w, cy: r.y + pin.y, r: 8 }));
      }
      groups.boxes.append(g);
    }
    // A role not heard on this machine: a hollow endpoint beside the Stack (ADR 0051 §3).
    const primary = this.model.plane.stacks[0];
    const primaryRect = primary ? this.layout.stacks.get(primary.name) : null;
    let unheard = 0;
    for (const b of this.model.bindings) {
      if (b.device || !primaryRect) continue;
      const x = primaryRect.x + primaryRect.w + 120;
      const y = primaryRect.y + primaryRect.h - 240 + unheard++ * 120;
      groups.boxes.append(svg('rect', { class: 'endpoint', x, y, width: 560, height: 96, rx: 48 }));
      groups.boxes.append(svg('text', { class: 'endpoint-text', x: x + 36, y: y + 40 }, `${endpointLabel(b)} (${b.endpoint})`));
      groups.boxes.append(svg('text', { class: 'endpoint-text', x: x + 36, y: y + 76 }, b.note));
      for (const stack of this.model.plane.stacks) {
        for (const pin of this.model.pins.get(stack.name) ?? []) {
          if (pin.side !== 'out' || !b.ports.some((p) => pin.outside.includes(p))) continue;
          const a = pinAt.get(`${stack.name} out ${pin.address}`);
          if (a) groups.cables.append(svg('path', { class: 'cable dashed', d: cablePath(a, { x, y: y + 48 }) }));
        }
      }
    }
    this.model.cables.forEach((c, i) => {
      const a = pinAt.get(`${c.stack} out ${c.pin}`);
      const b = portAt.get(`${c.device} ${c.channel}`);
      if (!a || !b) return;
      const path = svg('path', { class: `cable${c.dashed ? ' dashed' : ''}`, d: cablePath(a, b, i) });
      path.append(svg('title', {}, `${c.pin} → ${c.port} (${c.endpoint}) → channel ${c.channel}`));
      groups.cables.append(path);
    });
    if (this.layout.tray) {
      const t = this.layout.tray;
      groups.boxes.append(svg('rect', { class: 'tray', x: t.x, y: t.y, width: t.w, height: t.h, rx: 8 }));
      groups.boxes.append(svg('text', { class: 'box-kind', x: t.x + 20, y: t.y + 32 }, 'UNSTACKED'));
      this.model.unstacked.forEach((name, i) => groups.boxes.append(svg('text', { class: 'tray-text', x: t.x + 20, y: t.y + 100 + i * 32 }, name)));
    }
    this.setupSvg.replaceChildren(groups.cables, groups.boxes, groups.pins);
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
    } else if (this.level() !== 'setup') {
      const rect = this.focusRect();
      if (rect) this.view = fitWidth(rect, this.viewport, this.view);
    }
    this.render();
  }

  /** The focused Stack's rectangle (plane units), or null. */
  focusRect() {
    return (this.focused && this.layout?.stacks.get(this.focused)) || null;
  }

  /** The focused Stack's fit scale (the zoom's reference), or the whole plane's. */
  fitK() {
    const rect = this.focusRect();
    return rect && this.viewport.w ? fitScale(rect, this.viewport) : null;
  }

  ratio(view = this.view) {
    const fit = this.fitK();
    return fit ? view.k / fit : 0;
  }

  level(view = this.view) {
    return levelOf(this.ratio(view));
  }

  /** The person (or a command) moved the view: from now on it is theirs, never fitted again by itself. */
  setView(view) {
    this.stopTravel();
    this.touched = true;
    this.view = view;
    this.render();
  }

  /** The view drawn: one transform, the fades of its ratio, the board laid on the Stack in the wiring band. */
  render() {
    if (!this.world || !this.viewport.w) return;
    const v = this.view;
    this.world.style.transform = `translate(${v.x}px, ${v.y}px) scale(${v.k})`;
    const r = this.ratio();
    const fades = planeFades(r);
    const level = levelOf(r);
    this.dataset.level = level;
    this.setupSvg.style.opacity = String(fades.setup);
    for (const [name, s] of this.stacks) {
      const focused = name === this.focused;
      s.element.style.opacity = String(focused ? 1 : fades.setup);
      s.host.style.opacity = String(focused ? fades.panels : 1);
      // Past the wiring band the board has the pointer; the panels under it take none.
      s.element.style.pointerEvents = focused && level === 'wiring' ? 'none' : '';
    }
    this.renderDeep(fades, level);
    this.renderBar(level, r);
  }

  /** The board (ADR 0047) in the wiring band: faded in over the panels, its plate origin on the Stack's; past it, its own. */
  renderDeep(fades, level) {
    const shown = fades.deep > 0;
    if (this.deep.hidden === shown) this.deep.hidden = !shown;
    if (!shown) return;
    this.deep.style.opacity = String(fades.deep);
    this.deep.style.pointerEvents = level === 'wiring' ? '' : 'none';
    if (level === 'wiring' && this.deepOwned) return;
    this.deepOwned = level === 'wiring';
    const map = this.boardMap();
    if (!map) return;
    this.schematic.fitted = true;
    this.schematic.setView?.(boardView(this.view, map.origin, map.offset));
  }

  /** Where the board's plate origin lies on the plane (the focused Stack's first panel) and its stage on the plane's. */
  boardMap() {
    const rect = this.focusRect();
    const s = this.stacks.get(this.focused);
    if (!rect || !s?.layout) return null;
    const origin = { x: rect.x, y: rect.y + s.layout.top + RAIL_PU * PU - GUTTER };
    const stage = this.schematic.stage?.getBoundingClientRect?.();
    const own = this.stage.getBoundingClientRect();
    const offset = stage ? { x: stage.left - own.left, y: stage.top - own.top } : { x: 0, y: 0 };
    return { origin, offset };
  }

  renderBar(level, r) {
    const key = `${level}|${this.focused}`;
    if (key !== this.barKey) {
      this.barKey = key;
      const crumb = (text, current, act) => {
        const b = html('button', { type: 'button', className: 'crumb', textContent: text });
        b.setAttribute('aria-current', String(current));
        if (!current) b.addEventListener('click', act);
        return b;
      };
      const items = [crumb('Signal Project', level === 'setup', () => this.fitSetup())];
      if (level !== 'setup' && this.focused) items.push(html('span', { className: 'sep', textContent: '›' }), crumb(`Stack ${this.focused}`, level === 'stack', () => this.focusStack(this.focused)));
      if (level === 'wiring') items.push(html('span', { className: 'sep', textContent: '›' }), crumb('Board', true, () => {}));
      this.levelText = html('span', { className: 'level' });
      this.bar.replaceChildren(...items, this.levelText);
    }
    // The board has its own bar past the band.
    this.bar.hidden = level === 'wiring';
    const name = { setup: 'Setup', stack: 'Stack', wiring: 'Board' }[level];
    const text = `${name} · ${r ? r.toFixed(2) : '0'}× fit`;
    if (this.levelText.textContent !== text) this.levelText.textContent = text;
  }

  // Travel: a focus, Fit, Escape and Enter animate the view (ADR 0047 §8's curve), at once with reduced motion.

  travel(to) {
    this.stopTravel();
    this.touched = true;
    const from = this.view;
    const reduced = globalThis.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
    if (reduced) {
      this.view = to;
      this.render();
      return;
    }
    const start = performance.now();
    const step = (now) => {
      const t = Math.min(1, (now - start) / ZOOM_MS);
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

  /** Setup → Stack (ADR 0051 §8): the Stack's width fills the stage, its top in view. */
  focusStack(name) {
    if (!this.layout?.stacks.has(name)) return;
    this.focused = name;
    this.barKey = null;
    const rect = this.layout.stacks.get(name);
    const keep = this.level() !== 'setup' ? this.view : null;
    this.travel(fitWidth(rect, this.viewport, keep));
  }

  /** The whole plane in view: the Setup level. */
  fitSetup() {
    if (this.layout) this.travel(fitAll(this.layout.bounds, this.viewport));
  }

  /** Stack → the board: the view zoomed about the centre to the wiring band's end. */
  enterBoard() {
    const fit = this.fitK();
    if (!fit) return;
    const centre = { x: this.viewport.w / 2, y: this.viewport.h / 2 };
    this.travel(zoomAbout(this.view, (fit * WIRING_BAND[1]) / this.view.k, centre, fit));
  }

  /** The board's top-level view mapped back to the plane, the board given up (zooming out of it, Escape at its top). */
  leaveBoard() {
    const map = this.boardMap();
    const view = this.schematic.view;
    if (map && view) {
      const root = reroot(view, (l) => this.schematic.layoutOf(l), '');
      this.view = fromBoardView(root, map.origin, map.offset);
    }
    this.deepOwned = false;
  }

  /** Whether the board is at its top level (nothing zoomed into), so the plane may take it back. */
  boardAtTop() {
    return !(this.schematic.pinned?.size > 0) && (this.schematic.view?.path ?? '') === '';
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

  pointerDown(event) {
    if (event.button !== 0 || event.composedPath().includes(this.deep) || event.composedPath().includes(this.bar) || this.onControl(event)) return;
    this.stopTravel();
    this.drag = { start: this.point(event), last: this.point(event), id: event.pointerId, moved: false };
  }

  pointerMove(event) {
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
    if (!this.drag || event.pointerId !== this.drag.id) return;
    this.drag = null;
    this.stage.releasePointerCapture?.(event.pointerId);
  }

  doubleClick(event) {
    if (event.composedPath().includes(this.deep) || this.onControl(event)) return;
    const name = this.stackAt(event);
    if (name && this.level() === 'setup') this.focusStack(name);
  }

  /** Past the wiring band the board zooms by itself; a zoom out of its top level comes back to the plane. */
  wheelCapture(event) {
    if (this.deep.hidden || this.level() !== 'wiring' || !event.composedPath().includes(this.deep)) return;
    const out = (event.ctrlKey || this.wheelKinds.kind(event, event.timeStamp) !== 'trackpad') && event.deltaY > 0;
    if (!out || !this.boardAtTop()) return;
    const fit = this.fitK();
    const root = reroot(this.schematic.view, (l) => this.schematic.layoutOf(l), '');
    if (!fit || root.k / fit > WIRING_BAND[1] * 1.0001) return;
    event.stopPropagation();
    event.preventDefault();
    this.leaveBoard();
    // The rest of this gesture is the plane's, zooming on out: one curve in and out.
    this.handoff = event.timeStamp;
    this.wheelZoom(event, true);
  }

  /**
   * The wheel: at the Setup level a mouse's wheel and a pinch zoom about the
   * pointer, a trackpad's two-finger scroll pans; at the Stack level the
   * wheel scrolls the Stack and a pinch (or Ctrl+wheel) zooms. Zooming in
   * until the Stack fills the stage snaps to its fit (focus).
   */
  wheel(event) {
    if (event.composedPath().includes(this.deep) && !this.deep.hidden && this.level() === 'wiring') return;
    // A gesture handed back by the board (`wheelCapture`) goes on zooming here until it pauses.
    const handed = this.handoff !== undefined && event.timeStamp - this.handoff < GESTURE_GAP_MS;
    if (!handed && !takesWheel(this, event)) return;
    event.preventDefault();
    if (handed) this.handoff = event.timeStamp;
    this.wheelZoom(event, handed);
  }

  wheelZoom(event, forced) {
    this.stopTravel();
    const kind = event.ctrlKey ? 'pinch' : this.wheelKinds.kind(event, event.timeStamp);
    const level = this.level();
    const rect = this.focusRect();
    const mode = event.deltaMode ?? 0;
    if (level !== 'setup' && rect && kind !== 'pinch' && !forced) {
      // The Stack: the wheel scrolls (a trackpad's sideways travel pans, when zoomed past the width).
      const dy = wheelPixels(event.deltaY || 0, mode);
      const dx = kind === 'trackpad' ? wheelPixels(event.deltaX || 0, mode) : 0;
      this.setView(scrollStack(panBy(this.view, level === 'stack' && this.ratio() > 1.0001 ? -dx : 0, 0), dy, rect, this.viewport));
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

  /** A zoom by `factor` about screen point `at`: coming to a Stack's fit from below focuses it (its width fitted). */
  zoomBy(factor, at) {
    const fit = this.fitK();
    const r0 = this.ratio();
    let next = zoomAbout(this.view, factor, at, fit);
    if (fit && reachesFit(r0, next.k / fit)) {
      const rect = this.focusRect();
      next = fitWidth(rect, this.viewport, next);
    } else if (fit && next.k / fit >= 1) {
      // Zoomed past the fit: the Stack stays in view vertically.
      next = { ...next, ...clampTop(next, this.focusRect(), this.viewport) };
    }
    this.setView(next);
  }

  /** The keys: Enter goes in a level, Escape out; 0 fits the whole plane; + and − zoom about the centre; the arrows and the page keys scroll. */
  key(event) {
    if (event.defaultPrevented || event.metaKey || event.altKey) return;
    const level = this.level();
    const inBoard = level === 'wiring' && event.composedPath().includes(this.deep);
    if (event.key === 'Escape') {
      if (inBoard) {
        if (!this.boardAtTop() || this.schematic.selected?.length || this.schematic.tracing) return;
        event.preventDefault();
        event.stopPropagation();
        this.leaveBoard();
        this.focusStack(this.focused);
        return;
      }
      if (level === 'stack' || level === 'wiring') {
        event.preventDefault();
        this.fitSetup();
      }
      return;
    }
    if (inBoard || this.onControl(event)) return;
    const centre = { x: this.viewport.w / 2, y: this.viewport.h / 2 };
    const rect = this.focusRect();
    if (event.key === 'Enter') {
      event.preventDefault();
      if (level === 'setup' && this.focused) this.focusStack(this.focused);
      else if (level === 'stack') this.enterBoard();
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
    for (const s of this.stacks.values()) for (const card of s.cards.values()) for (const e of card.querySelectorAll('[bind]')) out.push(...String(e.getAttribute('bind')).split(/\s+/));
    return out;
  }

  /** Focuses the Stack holding a control bound to one of `addresses` and scrolls it into view; returns the controls found. */
  reveal(addresses, whole = false) {
    const wanted = new Set(addresses);
    for (const [name, s] of this.stacks) {
      const controls = [...s.cards.values()].flatMap((card) => [...card.querySelectorAll('[bind]')]).filter((e) => String(e.getAttribute('bind')).split(/\s+/).some((a) => wanted.has(a)));
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
        this.travel(view);
      }
      return whole ? controls.map((c) => c.closest('.signal-card') ?? c) : controls;
    }
    return [];
  }
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
