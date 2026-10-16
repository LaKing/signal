// <signal-schematic>: the project's signal path as an infinite-zoom SVG
// schematic with clickable parts (M2.9-1, ADR 0026 §2–3,
// docs/ui-controls.md "Schematic"). It draws the `graph` message: the top
// level's instances and connections, laid out (ui/lib/schematic-layout.js)
// with the positions stored in ui/schematic.toml; a composite opens in place
// when zoomed in or double-clicked (ui/lib/schematic-scene.js). One view
// transform, wheel and pinch zoom about the pointer, the background drags to
// pan, keys + − 0 and the arrows. Clicking selects an instance, a port or a
// wire and shows it on a card; dragging a box moves it (`schematic.move`).
// M2.9-3: the bar's Edit patch switch edits the patch
// (ui/controls/schematic-editor.js); off, nothing of it takes an input.
// Signal outputs on screen carry a level bar from scope streams
// (ui/lib/schematic-levels.js). Nothing is drawn or streamed while hidden or
// switched off, and nothing runs between events: no animation loop.
// M2.9-1b: each drawn level is one group, kept with its elements; a pan or a
// zoom changes only the groups' matrices, and elements are built again, by
// key, only when what is visible changes (ui/lib/schematic-draw.js). A
// project that stops loading (`graph.error`) leaves the last drawing on
// screen, marked stale, under its diagnostics.

import { onDefaultConnection } from '../signal-ui.js';
import { ENTRY_CSS, ValueEntry } from './entry-field.js';
import { EDITOR_CSS, SchematicEditor } from './schematic-editor.js';
import { paletteLevel } from '../lib/schematic-patch.js';
import { formatFixed } from '../lib/format.js';
import { layout } from '../lib/schematic-layout.js';
import { breadcrumbs, levelRect, nodeRect, rebase, scene } from '../lib/schematic-scene.js';
import { bake, bakeHolds, bakedMatrix, bakedPath, bakedPin, bakedPoint, bakedRect, inBounds, reconcile, sameItems } from '../lib/schematic-draw.js';
import { hitTest, reselect } from '../lib/schematic-hit.js';
import { card, cardStream, converterText, dragMessage, releaseMessage, resetMessage, rowValues, staleNotice } from '../lib/schematic-inspector.js';
import { LEVEL_RATE, SLOTS, candidates, levelFraction, levelPlan } from '../lib/schematic-levels.js';
import { IDENTITY, fit, focus, keyView, matrix, pan, wheelFactor, zoomAt } from '../lib/schematic-view.js';

const SVG = 'http://www.w3.org/2000/svg';
/** A press that moves less than this (px) is a click. */
const DRAG_PX = 3;
/** Text smaller than this on screen (px) is not drawn. */
const MIN_TEXT_PX = 4;
/** After the view settles (ms), the level streams follow it. */
const SETTLE_MS = 150;

const CSS = `
:host { display: block; position: relative; overflow: hidden; height: 100%; outline: none;
  color: var(--signal-fg, #d6dbe4); background: var(--signal-bg, #16181d);
  font: var(--signal-font, 11px/1.3 -apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif); }
svg { position: absolute; inset: 0; width: 100%; height: 100%; display: block; touch-action: none; user-select: none; -webkit-user-select: none; }
.box { fill: var(--signal-panel, #1f232b); stroke: var(--signal-knob-edge, #3a414d); vector-effect: non-scaling-stroke; }
.composite > .box { stroke: var(--signal-muted, #8a93a3); stroke-width: 2; }
.composite > .frame { fill: none; stroke: var(--signal-muted, #8a93a3); stroke-dasharray: 4 3; vector-effect: non-scaling-stroke; }
.open > .box { fill: var(--signal-bg, #16181d); }
.pins > .box { fill: none; stroke: var(--signal-track, #2a2f38); stroke-dasharray: 2 4; }
.selected > .box, .wire.selected { stroke: var(--signal-focus, #5aa9ff); }
.name { fill: var(--signal-fg, #d6dbe4); font-weight: 600; }
.module, .pin-label { fill: var(--signal-muted, #8a93a3); }
.pin { fill: var(--signal-track, #2a2f38); stroke: var(--signal-muted, #8a93a3); vector-effect: non-scaling-stroke; }
.wire { fill: none; stroke-width: 1.5; vector-effect: non-scaling-stroke; }
.wire.signal { stroke: var(--signal-accent, #4fc3f7); }
.wire.vector { stroke: var(--signal-effective, #ffb74d); }
.wire.events { stroke: var(--signal-meter-rms, #3ecf8e); }
.wire.dashed { stroke-dasharray: 6 4; }
.level { fill: var(--signal-track, #2a2f38); }
.level-fill { fill: var(--signal-meter-rms, #3ecf8e); }
.level-mark { fill: var(--signal-muted, #8a93a3); visibility: hidden; }
[data-state="no-slot"] > .level-mark { visibility: visible; }
[data-state="no-slot"] > .level-fill, [data-state="idle"] > .level-fill, [data-state="pending"] > .level-fill { visibility: hidden; }
.bar { position: absolute; left: 8px; top: 8px; right: 8px; display: flex; gap: 6px; align-items: center; pointer-events: none; }
.bar > * { pointer-events: auto; }
.crumbs { display: flex; gap: 2px; flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; }
button { font: inherit; color: inherit; background: var(--signal-knob, #262b34); border: 1px solid var(--signal-knob-edge, #3a414d); border-radius: 4px; padding: 2px 8px; }
.message { position: absolute; left: 8px; bottom: 8px; color: var(--signal-muted, #8a93a3); pointer-events: none; }
svg.stale { opacity: 0.4; filter: grayscale(1); }
.diagnostics { position: absolute; left: 8px; right: 8px; bottom: 28px; max-height: 40%; overflow: hidden; visibility: hidden; padding: 6px 8px;
  white-space: pre-wrap; background: var(--signal-panel, #1f232b); border: 1px solid var(--signal-effective, #ffb74d); border-radius: var(--signal-radius, 6px); }
.diagnostics h3 { margin: 0 0 4px; font-size: 12px; color: var(--signal-effective, #ffb74d); }
.tip { position: absolute; left: 0; top: 0; pointer-events: none; visibility: hidden; white-space: nowrap; padding: 2px 6px;
  background: var(--signal-knob, #262b34); border: 1px solid var(--signal-knob-edge, #3a414d); border-radius: 4px; }
.card { position: absolute; right: 8px; top: 40px; width: 20rem; max-height: calc(100% - 56px); overflow: hidden; box-sizing: border-box;
  padding: 8px; visibility: hidden; background: var(--signal-panel, #1f232b); border: 1px solid var(--signal-knob-edge, #3a414d); border-radius: var(--signal-radius, 6px); }
.card h3 { margin: 0; font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.card .subtitle { color: var(--signal-muted, #8a93a3); margin-bottom: 6px; }
.card dl { display: grid; grid-template-columns: 5rem 1fr; gap: 2px 8px; margin: 0; }
.card dt { color: var(--signal-muted, #8a93a3); }
.card dd { margin: 0; overflow-wrap: anywhere; }
.row { display: grid; grid-template-columns: 1fr auto auto 1ch; gap: 2px 6px; align-items: center; margin-top: 4px; }
.row .readout { position: relative; font-variant-numeric: tabular-nums; white-space: pre; color: var(--signal-muted, #8a93a3); }
.row input { grid-column: 1 / 4; width: 100%; margin: 0; }
.row .mark { visibility: hidden; color: var(--signal-effective, #ffb74d); }
.row .mark.on { visibility: visible; }
${ENTRY_CSS}
${EDITOR_CSS}`;

function el(name, attributes = {}, text = null) {
  const node = document.createElementNS(SVG, name);
  for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, String(value));
  if (text !== null) node.textContent = text;
  return node;
}

function html(name, properties = {}) {
  return Object.assign(document.createElement(name), properties);
}

export class SignalSchematic extends HTMLElement {
  constructor() {
    super();
    this.view = IDENTITY;
    this.viewport = { w: 0, h: 0 };
    this.graph = null;
    this.layouts = new Map();
    /** Positions moved here and not yet in a pushed `graph`: level → instance → {x, y}. */
    this.moved = new Map();
    this.open = new Set();
    this.pinned = new Set();
    this.drawn = null;
    /** Each drawn level's group and its elements, by path. */
    this.groups = new Map();
    /** The paths of the groups in the SVG, in order. */
    this.order = [];
    this.crumbKey = null;
    this.selectedElement = null;
    /** The host's last word on its scope slots (`scopes`, else `describe`). */
    this.scopes = null;
    this.scopesPushed = false;
    this.fitted = false;
    this.selection = null;
    this.gesture = null;
    this.pending = false;
    this.barsPending = false;
    this.stops = [];
    this.levelSubs = new Map();
    this.levels = new Map();
    this.refused = new Map();
    this.bars = new Map();
    /** A node's element → its level bars, `[[address, {bar, fill, width}]]`. */
    this.barsOf = new WeakMap();
    this.levelTimer = null;
    this.retryTimer = null;
    this.cardValues = new Map();
    this.cardStop = null;
    this.cardRows = [];
  }

  connectedCallback() {
    if (!this.shadowRoot) this.build();
    this.resize = new ResizeObserver(() => this.measure());
    this.resize.observe(this);
    this.onVisibility = () => this.activity();
    document.addEventListener('visibilitychange', this.onVisibility);
    this.stopDefault = onDefaultConnection((connection) => this.attach(connection));
  }

  disconnectedCallback() {
    this.resize?.disconnect();
    document.removeEventListener('visibilitychange', this.onVisibility);
    this.stopDefault?.();
    this.detach();
  }

  build() {
    const root = this.attachShadow({ mode: 'open' });
    this.tabIndex = 0;
    const style = html('style', { textContent: CSS });
    this.svg = el('svg', { role: 'img', 'aria-label': 'Signal schematic' });
    this.bar = html('div', { className: 'bar' });
    this.crumbs = html('nav', { className: 'crumbs' });
    const fitButton = html('button', { type: 'button', textContent: 'Fit', title: 'Fit (0)' });
    fitButton.addEventListener('click', () => this.setView(fit(this.rootBounds(), this.viewport)));
    const reset = html('button', { type: 'button', textContent: 'Reset layout', title: 'Forget the moved positions of the level shown' });
    reset.addEventListener('click', () => this.resetLayout(this.currentLevel()));
    const resetAll = html('button', { type: 'button', textContent: 'Reset all', title: 'Forget every moved position (removes ui/schematic.toml)' });
    resetAll.addEventListener('click', () => this.resetLayout(null));
    this.bar.append(this.crumbs, fitButton, reset, resetAll);
    this.message = html('div', { className: 'message' });
    this.staleBox = html('section', { className: 'diagnostics' });
    this.tip = html('div', { className: 'tip' });
    this.cardBox = html('section', { className: 'card' });
    root.append(style, this.svg, this.bar, this.message, this.staleBox, this.tip, this.cardBox);
    this.editor = new SchematicEditor(this);
    this.editor.build(this.bar, root);
    this.svg.addEventListener('pointerdown', (e) => this.pointerDown(e));
    this.svg.addEventListener('pointermove', (e) => this.pointerMove(e));
    this.svg.addEventListener('pointerup', (e) => this.pointerUp(e));
    this.svg.addEventListener('pointercancel', () => this.cancelGesture());
    this.svg.addEventListener('pointerleave', () => this.hideTip());
    this.svg.addEventListener('dblclick', (e) => this.doubleClick(e));
    this.svg.addEventListener('wheel', (e) => this.wheel(e), { passive: false });
    // Safari's pinch.
    this.svg.addEventListener('gesturestart', (e) => this.pinch(e, true));
    this.svg.addEventListener('gesturechange', (e) => this.pinch(e, false));
    this.addEventListener('keydown', (e) => this.key(e));
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
    on('graph', (graph) => this.setGraph(graph));
    on('graph.error', (error) => this.stale(error));
    on('scopes', (scopes) => this.scopesSeen(scopes, true));
    on('welcome', () => this.fetch());
    on('describe', () => this.described());
    on('unsaved', () => this.refreshCard());
    on('app', () => this.activity());
    if (connection.status === undefined || connection.status === 'open') this.fetch();
  }

  detach() {
    for (const stop of this.stops.splice(0)) stop();
    this.releaseLevels();
    this.stopCardStream();
    this.connection = null;
  }

  fetch() {
    this.connection?.request?.({ type: 'graph' }).then(
      (graph) => this.setGraph(graph),
      (error) => {
        if (error.code === 'does-not-load') this.stale(error.reply ?? error);
        else this.say(error.code === 'closed' ? '' : `No schematic: ${error.message}`);
      },
    );
  }

  /** `graph.error`: the last drawing stays, marked stale, under the diagnostics. */
  stale(error) {
    // Another project's error: its drawing is not this one's, so it goes.
    if (this.graph && 'folder' in error && error.folder !== this.graph.folder) {
      this.graph = null;
      this.layouts.clear();
      this.selection = null;
      this.fitted = false;
      this.svg.replaceChildren();
      this.crumbs.replaceChildren();
      this.showCard();
    }
    const notice = staleNotice(error, Boolean(this.graph));
    const title = html('h3', { textContent: notice.title });
    this.staleBox.replaceChildren(title, document.createTextNode(notice.text));
    this.staleBox.style.visibility = 'visible';
    this.svg.classList.toggle('stale', Boolean(this.graph));
  }

  setGraph(graph) {
    this.staleBox.style.visibility = 'hidden';
    this.svg.classList.remove('stale');
    const old = new Map(this.layouts);
    // Another project: its own view, fitted again, nothing open or selected.
    if (this.graph && graph.project !== this.graph.project) {
      this.open = new Set();
      this.pinned = new Set();
      this.fitted = false;
      this.selection = null;
      this.view = IDENTITY;
      old.clear();
    }
    this.graph = graph;
    this.editor.graphSeen(graph);
    this.layouts.clear();
    this.moved.clear();
    // What did not move stays where it is on screen.
    if (old.size) this.view = rebase(this.view, (level) => old.get(level) ?? null, (level) => this.layoutOf(level));
    this.say(graph.layout?.error ? `Positions ignored: ${graph.layout.error}` : '');
    // The selection is found again in the new graph, or let go.
    this.selection = reselect(this.selection, (level) => this.layoutOf(level));
    this.showCard();
    this.schedule();
  }

  /**
   * The host's free scope slots: a `scopes` push (`pushed`), or `describe`
   * until the first push. Kept with when it came and the slots held then.
   */
  scopesSeen(scopes, pushed) {
    if (!pushed && this.scopesPushed) return;
    if (typeof scopes?.free !== 'number') return;
    this.scopesPushed ||= pushed;
    this.scopes = { free: scopes.free, sinceFreedMs: scopes.sinceFreedMs ?? null, at: performance.now(), held: this.levelSubs.size };
    if (pushed) this.updateLevels();
  }

  described() {
    this.scopesSeen(this.connection?.description?.scopes, false);
    this.activity();
    // The card is built again only when its rows change (a `describe` that
    // arrives after the selection, or another structure); values in place.
    const model = card(this.selection, { graph: this.graph, description: this.connection?.description });
    const rows = (m) => (m?.parameters ?? []).map((r) => `${r.address} ${r.editable}`).join('\n');
    if (rows(model) !== rows(this.cardModel)) this.showCard();
    else this.refreshCard();
  }

  say(text) {
    this.message.textContent = text;
  }

  // What is shown.

  /** Streams flow only while shown, switched on and playing. */
  get active() {
    const c = this.connection;
    return Boolean(this.viewport.w > 0 && document.visibilityState !== 'hidden' && c && c.streaming !== false && c.description?.playing);
  }

  activity() {
    this.updateLevels();
    this.updateCardStream();
    if (this.viewport.w > 0) this.schedule();
  }

  measure() {
    const rect = this.getBoundingClientRect();
    this.viewport = { w: rect.width, h: rect.height };
    // A hidden tab is 0×0: nothing to draw, nothing to stream.
    this.activity();
  }

  layoutOf(level) {
    if (!this.graph) return null;
    if (!this.layouts.has(level)) {
      const source = level === '' ? this.graph.top : this.graph.composites?.[level];
      if (!source) return null;
      const stored = { ...(this.graph.layout?.positions?.[level] ?? {}), ...Object.fromEntries(this.moved.get(level) ?? []) };
      this.layouts.set(level, layout(source, stored));
    }
    return this.layouts.get(level);
  }

  rootBounds() {
    return this.layoutOf('')?.bounds ?? { x: 0, y: 0, w: 1, h: 1 };
  }

  setView(view) {
    this.view = view;
    this.schedule();
  }

  /** One redraw at the next frame, only when something changed. */
  schedule() {
    if (this.pending || this.viewport.w === 0) return;
    this.pending = true;
    requestAnimationFrame(() => {
      this.pending = false;
      this.render();
    });
  }

  render() {
    if (!this.graph || this.viewport.w === 0) return;
    if (!this.fitted) {
      this.view = fit(this.rootBounds(), this.viewport);
      this.fitted = true;
    }
    const layoutOf = (level) => this.layoutOf(level);
    this.drawn = scene({ layoutOf, view: this.view, viewport: this.viewport, open: this.open, pinned: this.pinned });
    // Kept in the coordinates of the deepest level under the centre.
    this.view = this.drawn.view;
    this.open = this.drawn.open;
    // A level with nothing on screen is not drawn, nor a parent of which
    // only open boxes' insides show (the background's colour).
    const shown = this.drawn.levels.filter((level) => (level.nodes.length || level.wires.length) && !level.behind);
    let changed = false;
    const paths = [];
    for (const level of shown) {
      const drew = this.drawLevel(level);
      if (drew === null) continue;
      paths.push(level.path);
      changed = drew || changed;
    }
    for (const path of this.groups.keys()) {
      if (!paths.includes(path)) {
        this.groups.delete(path);
        changed = true;
      }
    }
    // Only when levels open or close: a pan or a zoom changes matrices.
    if (!sameItems(paths, this.order)) {
      this.svg.replaceChildren(...paths.map((path) => this.groups.get(path).g));
      this.order = paths;
    }
    if (changed) this.collectBars();
    this.applySelection();
    this.applyDrag();
    this.drawCrumbs();
    this.applyBars();
    clearTimeout(this.levelTimer);
    this.levelTimer = setTimeout(() => this.updateLevels(), SETTLE_MS);
  }

  /**
   * Level `level` (a scene entry) in its group: its matrix always, its
   * elements only when what it shows changed. Returns whether they did, or
   * null for a level its coordinates cannot place on screen (past the
   * camera's precision), which is not drawn: never a matrix out of bounds.
   */
  drawLevel(level) {
    let drawn = this.groups.get(level.path);
    if (!drawn) {
      const wires = el('g');
      const nodes = el('g');
      drawn = { g: el('g'), wires, nodes, baked: null, wireCache: new Map(), nodeCache: new Map(), shown: null };
      drawn.g.append(wires, nodes);
      this.groups.set(level.path, drawn);
    }
    const transform = level.transform;
    if (!drawn.baked || drawn.layout !== level.layout || !bakeHolds(drawn.baked, transform, this.viewport)) {
      drawn.baked = bake(transform, this.viewport);
      drawn.layout = level.layout;
      drawn.wireCache = new Map();
      drawn.nodeCache = new Map();
      drawn.shown = null;
    }
    const placed = bakedMatrix(drawn.baked, transform);
    if (!drawn.baked.representable || !inBounds(placed)) return null;
    const text = (px) => px * transform.k >= MIN_TEXT_PX;
    const sizes = `${text(12)} ${text(9)}`;
    const open = level.nodes.filter((n) => n.composite && this.drawn.open.has(`${level.path}/${n.name}`)).map((n) => n.name).join('/');
    const shown = drawn.shown;
    const same = shown && shown.sizes === sizes && shown.open === open && sameItems(shown.nodes, level.nodes) && sameItems(shown.wires, level.wires);
    if (!same) {
      const baked = drawn.baked;
      const wires = reconcile(drawn.wireCache, level.wires, (w) => w.id, () => '', (w) => this.wireElement(level, w, baked));
      const nodes = reconcile(
        drawn.nodeCache,
        level.nodes,
        (n) => n.id,
        (n) => `${this.drawn.open.has(`${level.path}/${n.name}`)} ${sizes}`,
        (n) => this.nodeElement(level, n, baked, text),
      );
      if (!sameItems([...drawn.wires.children], wires.elements)) drawn.wires.replaceChildren(...wires.elements);
      if (!sameItems([...drawn.nodes.children], nodes.elements)) drawn.nodes.replaceChildren(...nodes.elements);
      drawn.wireCache = wires.cache;
      drawn.nodeCache = nodes.cache;
      drawn.shown = { sizes, open, nodes: level.nodes, wires: level.wires };
    }
    drawn.g.setAttribute('transform', matrix(placed));
    return !same;
  }

  wireElement(level, wire, baked) {
    const from = `${level.path}/${String(wire.written.from).replace(/^\//, '')}`;
    return el('path', {
      class: ['wire', wire.shape, wire.dashed ? 'dashed' : ''].join(' ').trim(),
      d: bakedPath(baked, wire.points),
      'data-address': from,
      'data-label': wire.label,
      'data-part': 'schematic',
    });
  }

  nodeElement(level, node, baked, text) {
    const m = baked.m;
    const address = `${level.path}/${node.name}`;
    const isOpen = this.drawn.open.has(address);
    const classes = ['node', node.kind === 'instance' ? '' : 'pins', node.composite ? 'composite' : '', isOpen ? 'open' : ''];
    const group = el('g', { class: classes.join(' ').replace(/\s+/g, ' ').trim() });
    if (node.kind === 'instance') {
      group.dataset.address = address;
      group.dataset.label = node.name;
      group.dataset.part = 'schematic';
    }
    const rect = (r, attributes) => {
      const at = bakedRect(baked, r);
      if (at) group.append(el('rect', { ...attributes, ...at }));
    };
    const label = (p, size, attributes, content) => {
      const at = bakedPoint(baked, p);
      group.append(el('text', { ...attributes, x: at.x, y: at.y, 'font-size': size * m }, content));
    };
    rect(node, { class: 'box', rx: 4 * m });
    if (node.composite) rect({ x: node.x + 3, y: node.y + 3, w: node.w - 6, h: node.h - 6 }, { class: 'frame', rx: 3 * m });
    if (text(12) && node.kind === 'instance') {
      label({ x: node.x + 8, y: node.y + 14 }, 12, { class: 'name' }, node.name);
      label({ x: node.x + 8, y: node.y + 27 }, 10, { class: 'module' }, node.module);
    }
    const bars = [];
    for (const pin of [...node.ins, ...node.outs]) {
      const out = pin.direction === 'out';
      const portAddress = node.kind === 'instance' ? `${address}.${pin.name}` : `${level.path}.${pin.name}`;
      const data = { class: 'pin', 'data-address': portAddress, 'data-label': pin.name, 'data-part': 'schematic' };
      const shape = bakedPin(baked, level.transform, this.viewport, pin);
      if (shape.circle) group.append(el('circle', { ...data, ...shape.circle }));
      else if (shape.polygon.length) group.append(el('polygon', { ...data, points: shape.polygon.map((p) => `${p.x},${p.y}`).join(' ') }));
      if (text(9) && !isOpen) {
        label({ x: pin.x + (out ? -6 : 6), y: pin.y + 3 }, 9, { class: 'pin-label', 'text-anchor': out ? 'end' : 'start' }, pin.name);
      }
      if (out && node.kind === 'instance' && pin.shape === 'signal' && !isOpen) {
        // Fixed size whatever its state: only the fill's width changes.
        const bar = el('g', { class: 'meter', 'data-state': 'idle' });
        const x = pin.x - 34;
        const y = pin.y + 5;
        const track = bakedRect(baked, { x, y, w: 28, h: 3 });
        if (track) bar.append(el('rect', { class: 'level', ...track }));
        const at = bakedPoint(baked, { x, y });
        const fill = el('rect', { class: 'level-fill', x: at.x, y: at.y, width: 0, height: 3 * m });
        bar.append(fill);
        const mark = bakedRect(baked, { x, y: y + 1, w: 28, h: 1 });
        if (mark) bar.append(el('rect', { class: 'level-mark', ...mark }));
        group.append(bar);
        bars.push([portAddress, { bar, fill, width: 28 * m }]);
      }
    }
    this.barsOf.set(group, bars);
    return group;
  }

  /** The level bars of every node drawn. */
  collectBars() {
    this.bars.clear();
    for (const drawn of this.groups.values()) {
      for (const { el: group } of drawn.nodeCache.values()) for (const [address, bar] of this.barsOf.get(group) ?? []) this.bars.set(address, bar);
    }
  }

  /** The selected instance or wire, marked on its element. */
  applySelection() {
    const s = this.selection;
    let element = null;
    if (s?.kind === 'instance') element = this.nodeGroup(s.address);
    else if (s?.kind === 'wire') element = this.groups.get(s.path)?.wireCache.get(s.wire.id)?.el ?? null;
    if (element === this.selectedElement) return;
    this.selectedElement?.classList.remove('selected');
    element?.classList.add('selected');
    this.selectedElement = element;
  }

  /** A drag in progress keeps its offset on its box, built again or not. */
  applyDrag() {
    const gesture = this.gesture;
    if (!gesture?.drag || !gesture.moved) return;
    const group = this.nodeGroup(gesture.hit.address) ?? gesture.drag;
    const scale = gesture.k / (this.groups.get(gesture.hit.path)?.baked.m ?? 1);
    group.setAttribute('transform', `translate(${(gesture.last.x - gesture.start.x) / scale} ${(gesture.last.y - gesture.start.y) / scale})`);
    gesture.drag = group;
  }

  drawCrumbs() {
    const crumbs = breadcrumbs(this.drawn, this.viewport);
    const key = `${this.graph.project}\n${crumbs.map((c) => c.path).join('\n')}`;
    if (key === this.crumbKey) return;
    this.crumbKey = key;
    this.crumbs.replaceChildren(
      ...crumbs.map((crumb, i) => {
        const button = html('button', { type: 'button', textContent: i === 0 ? this.graph.project ?? 'project' : crumb.name });
        button.addEventListener('click', () => this.goTo(crumb.path));
        return button;
      }),
    );
  }

  /** The level shown: the deepest breadcrumb's (`""` the top level, else its composite). */
  currentLevel() {
    return this.drawn ? breadcrumbs(this.drawn, this.viewport).at(-1).level : '';
  }

  /** Back out to the level at `path` (a breadcrumb), its inside filling the view. */
  goTo(path) {
    for (const open of [...this.pinned]) if (open !== path && open.startsWith(`${path}/`)) this.pinned.delete(open);
    if (path === '') {
      this.setView(fit(this.rootBounds(), this.viewport));
      return;
    }
    const rect = this.drawn && levelRect(this.drawn, path);
    if (rect) this.setView(focus(this.view, rect, this.viewport));
  }

  // Input.

  point(event) {
    const rect = this.svg.getBoundingClientRect();
    return { x: event.clientX - rect.left, y: event.clientY - rect.top };
  }

  pointerDown(event) {
    if (event.button !== 0 || !this.drawn) return;
    this.focus({ preventScroll: true });
    const at = this.point(event);
    const hit = hitTest(this.drawn, at);
    this.svg.setPointerCapture?.(event.pointerId);
    // Edit patch: a press on an output starts a connection.
    if (this.editor.starts(hit)) {
      this.gesture = { start: at, last: at, hit, moved: false, connect: true };
      this.editor.connectStart(hit, at);
      return;
    }
    // The level's scale is kept from the start: a render during the drag
    // (a push, a zoom) may replace the group or close the level.
    const k = hit.kind === 'instance' ? this.levelOf(hit.path)?.transform.k : undefined;
    this.gesture = { start: at, last: at, hit, moved: false, k, drag: k ? this.nodeGroup(hit.address) : null };
  }

  nodeGroup(address) {
    const cut = address.lastIndexOf('/');
    return this.groups.get(address.slice(0, cut))?.nodeCache.get(address.slice(cut + 1))?.el ?? null;
  }

  pointerMove(event) {
    const at = this.point(event);
    const gesture = this.gesture;
    if (!gesture) {
      this.hover(at);
      return;
    }
    if (Math.hypot(at.x - gesture.start.x, at.y - gesture.start.y) > DRAG_PX) gesture.moved = true;
    if (gesture.connect) {
      this.editor.connectMove(at);
      return;
    }
    if (!gesture.moved) return;
    const last = gesture.last;
    gesture.last = at;
    if (gesture.drag) this.applyDrag();
    else this.setView(pan(this.view, at.x - last.x, at.y - last.y));
  }

  levelOf(path) {
    return this.drawn.levels.find((l) => l.path === path);
  }

  pointerUp(event) {
    const gesture = this.gesture;
    this.gesture = null;
    if (!gesture) return;
    const at = this.point(event);
    if (gesture.connect && gesture.moved) {
      this.editor.connectEnd(hitTest(this.drawn, at));
      return;
    }
    if (gesture.connect) this.editor.cancelConnect();
    if (!gesture.moved) {
      this.select(gesture.hit.kind === 'background' ? null : gesture.hit);
      return;
    }
    if (gesture.drag) {
      const { node, level } = gesture.hit;
      const k = gesture.k;
      const x = Math.round(node.x + (at.x - gesture.start.x) / k);
      const y = Math.round(node.y + (at.y - gesture.start.y) / k);
      // Kept here until the pushed `graph` holds it.
      if (!this.moved.has(level)) this.moved.set(level, new Map());
      this.moved.get(level).set(node.name, { x, y });
      const old = new Map(this.layouts);
      this.layouts.delete(level);
      this.view = rebase(this.view, (l) => old.get(l) ?? null, (l) => this.layoutOf(l));
      this.schedule();
      this.connection?.request?.({ type: 'schematic.move', level, instance: node.name, x, y }).catch((error) => this.say(`Not moved: ${error.message}`));
    }
  }

  cancelGesture() {
    if (this.gesture?.drag) this.gesture.drag.removeAttribute('transform');
    if (this.gesture?.connect) this.editor.cancelConnect();
    this.gesture = null;
  }

  hover(at) {
    if (!this.drawn) return;
    const hit = hitTest(this.drawn, at);
    if (hit.kind !== 'wire') {
      this.hideTip();
      return;
    }
    const converters = hit.wire.converters ?? [];
    this.tip.textContent = converters.length ? `${hit.from} → ${hit.to}: ${converters.map(converterText).join('; ')}` : `${hit.from} → ${hit.to}`;
    this.tip.style.transform = `translate(${Math.round(at.x + 12)}px, ${Math.round(at.y + 12)}px)`;
    this.tip.style.visibility = 'visible';
  }

  hideTip() {
    this.tip.style.visibility = 'hidden';
  }

  doubleClick(event) {
    if (!this.drawn) return;
    const hit = hitTest(this.drawn, this.point(event));
    // Edit patch: the add palette, where the background or an open
    // composite's inside was double-clicked.
    const level = this.editor.editing ? paletteLevel(hit, this.drawn, this.point(event)) : null;
    if (level) {
      this.editor.openPalette(this.point(event), level);
      return;
    }
    if (hit.kind !== 'instance' || !hit.node.composite) return;
    this.pinned.add(hit.address);
    const rect = nodeRect(this.drawn, hit.address);
    if (rect) this.setView(focus(this.view, rect, this.viewport));
  }

  wheel(event) {
    event.preventDefault();
    const factor = wheelFactor(event.deltaY, { deltaMode: event.deltaMode, pinch: event.ctrlKey });
    this.setView(zoomAt(this.view, factor, this.point(event)));
  }

  pinch(event, start) {
    event.preventDefault();
    if (start) this.pinchScale = 1;
    const factor = event.scale / (this.pinchScale || 1);
    this.pinchScale = event.scale;
    if (Number.isFinite(factor) && factor > 0) this.setView(zoomAt(this.view, factor, this.point(event)));
  }

  key(event) {
    // Edit patch takes its keys first: Escape leaves edit mode before it
    // goes back out of a level.
    if (this.editor.key(event)) return;
    // Keys of the inspector's slider and the bar's buttons are theirs (an
    // event from the shadow tree reaches this listener retargeted to the host).
    if (event.composedPath()[0] !== this) return;
    if (event.key === 'Escape') {
      const crumbs = this.drawn ? breadcrumbs(this.drawn, this.viewport) : [];
      if (crumbs.length > 1) {
        this.pinned.delete(crumbs.at(-1).path);
        this.goTo(crumbs.at(-2).path);
      } else {
        this.select(null);
      }
      event.preventDefault();
      return;
    }
    const view = keyView(this.view, event.key, this.viewport, this.rootBounds());
    if (view) {
      event.preventDefault();
      this.setView(view);
    }
  }

  resetLayout(level) {
    const message = level === null ? { type: 'schematic.reset' } : { type: 'schematic.reset', level };
    this.connection?.request?.(message).catch((error) => this.say(`Not reset: ${error.message}`));
  }

  // The selection and its card.

  select(hit) {
    this.selection = hit;
    this.showCard();
    this.schedule();
  }

  showCard() {
    this.stopCardStream();
    const model = card(this.selection, { graph: this.graph, description: this.connection?.description });
    this.cardModel = model;
    this.cardRows = [];
    if (!model) {
      this.cardBox.style.visibility = 'hidden';
      this.cardBox.replaceChildren();
      return;
    }
    const parts = [html('h3', { textContent: model.title }), html('div', { className: 'subtitle', textContent: model.subtitle })];
    if (model.fields.length) {
      const list = html('dl');
      for (const [label, value] of model.fields) list.append(html('dt', { textContent: label }), html('dd', { textContent: value }));
      parts.push(list);
    }
    for (const row of model.parameters) parts.push(this.parameterRow(row));
    parts.push(...this.editor.cardParts(this.selection));
    this.cardBox.replaceChildren(...parts);
    this.cardBox.style.visibility = 'visible';
    this.refreshCard();
    this.updateCardStream();
  }

  parameterRow(row) {
    const box = html('div', { className: 'row' });
    const name = html('span', { textContent: row.name, title: row.address });
    const base = html('span', { className: 'readout', title: 'base' });
    const effective = html('span', { className: 'readout', title: 'effective' });
    const mark = html('span', { className: 'mark', textContent: '•', title: 'unsaved' });
    box.append(name, base, effective, mark);
    const shown = { row, base, effective, mark, input: null };
    if (row.editable) {
      const input = html('input', { type: 'range', min: row.min, max: row.max, step: (row.max - row.min) / 1000 || 'any' });
      input.dataset.address = row.address;
      input.dataset.label = row.name;
      input.addEventListener('input', () => this.send(dragMessage(row.address, Number(input.value))));
      input.addEventListener('change', () => this.send(releaseMessage(row.address, Number(input.value))));
      input.addEventListener('dblclick', () => this.send(resetMessage(row.address)));
      // A typed value (docs/ui-controls.md "Value entry"): a double-click on
      // the base, or Enter on the slider; sent as a release is, the clamp's
      // reason on the message line.
      shown.entry = new ValueEntry(base, {
        spec: () => ({ unit: row.unit, range: { min: row.min, max: row.max }, taper: row.taper, kind: row.kind }),
        value: () => shown.value,
        apply: (value) => {
          shown.value = value;
          input.value = String(value);
          this.send(dragMessage(row.address, value));
          this.send(releaseMessage(row.address, value));
        },
        note: (text) => {
          if (text) this.say(text);
          else if (this.message.textContent === shown.notice) this.say('');
          shown.notice = text;
        },
        closed: () => this.refreshCard(),
        focus: () => input.focus({ preventScroll: true }),
      });
      input.addEventListener('keydown', (event) => {
        if (event.key === 'Enter' && shown.entry.open()) event.preventDefault();
      });
      box.append(input);
      shown.input = input;
    }
    this.cardRows.push(shown);
    return box;
  }

  send(message) {
    this.connection?.request?.(message).catch((error) => this.say(error.message));
  }

  /** The card's values, in place: same boxes, same widths. */
  refreshCard() {
    for (const shown of this.cardRows) {
      const values = rowValues(shown.row, this.cardValues, this.connection?.unsaved, this.connection?.description);
      shown.value = values.base;
      if (!shown.entry?.isOpen) shown.base.textContent = formatFixed(values.base, shown.row.unit);
      shown.effective.textContent = formatFixed(values.effective, shown.row.unit);
      shown.mark.classList.toggle('on', Boolean(values.unsaved));
      if (shown.input && document.activeElement !== shown.input && this.shadowRoot.activeElement !== shown.input && Number.isFinite(values.base)) {
        shown.input.value = String(values.base);
      }
    }
  }

  updateCardStream() {
    const stream = this.active ? cardStream(this.cardModel) : null;
    const key = stream ? JSON.stringify(stream) : null;
    if (key === this.cardKey) return;
    this.stopCardStream();
    if (!stream || !this.connection) return;
    this.cardKey = key;
    this.cardStop = this.connection.subscribe(stream, (frame) => {
      stream.addresses.forEach((address, i) => {
        const v = frame.values?.[i];
        if (v) this.cardValues.set(address, v);
      });
      this.refreshCard();
    });
  }

  stopCardStream() {
    this.cardStop?.();
    this.cardStop = null;
    this.cardKey = null;
    this.cardValues.clear();
  }

  // Level bars.

  updateLevels() {
    clearTimeout(this.retryTimer);
    this.retryTimer = null;
    if (!this.connection) return;
    const selected = this.selection?.kind === 'instance' ? this.selection.address : null;
    const wanted = this.drawn && this.graph ? candidates(this.drawn, this.graph, selected, this.viewport) : [];
    const plan = levelPlan({
      wanted,
      granted: new Set(this.levelSubs.keys()),
      refused: this.refused,
      active: this.active,
      now: performance.now(),
      slots: this.connection.description?.scopes?.slots ?? SLOTS,
      scopes: this.scopes,
    });
    for (const port of plan.unsubscribe) this.dropLevel(port);
    for (const port of plan.subscribe) this.takeLevel(port);
    this.states = new Map(wanted.map((c) => [c.address, { state: plan.states.get(c.address), port: c.port }]));
    this.applyBars();
    // Only while a request could succeed; otherwise the next `scopes` says.
    if (this.active && plan.retry !== null) this.retryTimer = setTimeout(() => this.updateLevels(), plan.retry);
  }

  takeLevel(port) {
    this.refused.delete(port);
    const stop = this.connection.subscribe(
      { stream: 'scope', port, rate: LEVEL_RATE },
      (frame) => {
        this.levels.set(port, levelFraction(frame.values));
        this.scheduleBars();
      },
      {
        onSubscribed: () => this.markLive(port),
        onError: (error) => {
          this.dropLevel(port);
          this.refused.set(port, performance.now());
          if (error.code === 'no-scope-slot' || error.code === 'busy') this.updateLevels();
        },
      },
    );
    this.levelSubs.set(port, stop);
  }

  markLive(port) {
    for (const entry of this.states?.values() ?? []) if (entry.port === port) entry.state = 'live';
    this.applyBars();
  }

  dropLevel(port) {
    this.levelSubs.get(port)?.();
    this.levelSubs.delete(port);
    this.levels.delete(port);
  }

  releaseLevels() {
    clearTimeout(this.retryTimer);
    clearTimeout(this.levelTimer);
    for (const port of [...this.levelSubs.keys()]) this.dropLevel(port);
  }

  scheduleBars() {
    if (this.barsPending || !this.active) return;
    this.barsPending = true;
    requestAnimationFrame(() => {
      this.barsPending = false;
      this.applyBars();
    });
  }

  applyBars() {
    for (const [address, shown] of this.bars) {
      const entry = this.states?.get(address);
      const state = entry?.state ?? 'idle';
      const level = state === 'live' ? this.levels.get(entry.port) ?? 0 : 0;
      // Written only when changed: a pan frame touches no bar.
      if (shown.state !== state) shown.bar.setAttribute('data-state', state);
      if (shown.level !== level) shown.fill.setAttribute('width', String(shown.width * level));
      shown.state = state;
      shown.level = level;
    }
  }
}

customElements.get('signal-schematic') ?? customElements.define('signal-schematic', SignalSchematic);
