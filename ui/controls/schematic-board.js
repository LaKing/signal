// The device view in <signal-schematic> (ADR 0047 §5–7, phase 2): the
// plate over the board. The project's panel (`ui/main.html`) is drawn as
// the device's plate at the board's origin, one layout unit per CSS px of
// the plate, by the faces' code (ui/controls/schematic-faces.js, face
// `#plate`): live controls, the panel's CSS. Its controls are measured
// there (whole plate units, ui/lib/board-view.js) and are the board's
// vias; the top level is laid out as a board (ui/lib/schematic-layout.js
// `board`). Three layers: Plate (the plate whole, the board hidden), X-ray
// (the board, the plate's cards as ghosts at a quarter of its opacity, its
// controls whole) and Board (the board and the plate's controls only).
// The bar switches the layer (and the key `schematic.layer` steps through
// them) and the signal kinds whose
// wires are drawn, and shows the board's measures. All of it is view
// state: told to the page's host (`signal-view-change`) to be kept per
// window in `ui/windows.toml`, never in the project's files.

import { LAYERS, LAYER_NAMES, KINDS, LIFT_MS, XRAY_OPACITY, metricsText, nextLayer, plateSize, plateVias, shownWires, toggleKind, viewState } from '../lib/board-view.js';
import { NOT_CONTROLS, bindList } from '../lib/schematic-controls.js';

/** The plate's face id (`SchematicFaces`). */
export const PLATE_FACE = '#plate';
/** The panel file the plate is. */
export const PLATE_FILE = 'main.html';

export const BOARD_CSS = `
/* ADR 0047 §5: the layer switch and the kinds on the bar. */
.layers { display: inline-flex; flex: none; }
.layers > button { border-radius: 0; margin-left: -1px; }
.layers > button:first-child { border-radius: 4px 0 0 4px; margin-left: 0; }
.layers > button:last-child { border-radius: 0 4px 4px 0; }
.bar button[aria-pressed="true"] { background: var(--signal-accent, #4fc3f7); color: var(--signal-bg, #16181d); }
.kinds { display: inline-flex; gap: 2px; flex: none; }
.kinds > button[aria-pressed="false"] { opacity: 0.45; text-decoration: line-through; }
.measures { flex: none; color: var(--signal-muted, #8a93a3); font-variant-numeric: tabular-nums; white-space: nowrap; }
/* The plate's cards as ghosts over the board (X-ray: the plate at a quarter of its opacity), and the plate's area under it. */
.plate-ghosts { transition: opacity ${LIFT_MS}ms ease-out; pointer-events: none; }
.plate-card { fill: color-mix(in srgb, var(--signal-fg, #d6dbe4) 30%, var(--signal-panel, #1f232b)); stroke: var(--signal-fg, #d6dbe4); stroke-width: 2; vector-effect: non-scaling-stroke; }
.plate-title { fill: var(--signal-fg, #d6dbe4); font-weight: 600; }
svg.layer-xray .plate-ghosts { opacity: ${XRAY_OPACITY}; }
.plate-layer .face-back { display: none; }
svg.layer-board .plate-ghosts, svg.layer-plate .plate-ghosts { opacity: 0; }
/* The board's ground and its sections (cards): tokens, so a later colour coding sits on top. */
svg { transition: background-color ${LIFT_MS}ms ease-out; }
svg.layer-xray, svg.layer-board { background: var(--signal-board-bg, #0e1014); }
.board-section { fill: var(--signal-board-section, #151820); stroke: none; transition: opacity ${LIFT_MS}ms ease-out; }
.board-section-edge { fill: none; stroke: var(--signal-board-section-edge, #343b47); stroke-width: 1.5; vector-effect: non-scaling-stroke; transition: opacity ${LIFT_MS}ms ease-out; }
svg.layer-plate .board-section, svg.layer-plate .board-section-edge { opacity: 0; }
/* The plate: the board hidden under it. */
svg > g > * { transition: opacity ${LIFT_MS}ms ease-out; }
svg.layer-plate > g > :not(.plate-layer) { opacity: 0; pointer-events: none; }
@media (prefers-reduced-motion: reduce) { svg, .plate-ghosts, svg > g > *, .board-section, .board-section-edge { transition: none; } }
/* A via: the plate's control on the board, a small pad at its place (the control itself is the plate's). */
.control.via > .box { fill: color-mix(in srgb, var(--signal-effective, #ffb74d) 35%, var(--signal-bg, #16181d)); stroke-width: 1; }
.control.via.reader > .box { fill: color-mix(in srgb, var(--signal-meter-rms, #3ecf8e) 35%, var(--signal-bg, #16181d)); }
.via-label { fill: var(--signal-muted, #8a93a3); }
/* ADR 0047 phase 4: a ribbon cable: a band under its members, in their colour when they share one, its label once. */
.ribbon { fill: color-mix(in srgb, var(--ribbon-tone, var(--signal-muted, #8a93a3)) 16%, var(--signal-board-bg, #0e1014)); stroke: color-mix(in srgb, var(--ribbon-tone, var(--signal-muted, #8a93a3)) 55%, transparent); stroke-width: 1; vector-effect: non-scaling-stroke; pointer-events: none; }
.ribbon.signal, .ribbon-label.signal { --ribbon-tone: var(--signal-accent, #4fc3f7); }
.ribbon.events, .ribbon-label.events { --ribbon-tone: var(--signal-meter-rms, #3ecf8e); }
.ribbon.vector, .ribbon.binding, .ribbon-label.vector, .ribbon-label.binding { --ribbon-tone: var(--signal-effective, #ffb74d); }
.ribbon-label { fill: var(--ribbon-tone, var(--signal-muted, #8a93a3)); font-weight: 600; paint-order: stroke; stroke: var(--signal-board-bg, #0e1014); stroke-width: 3px; stroke-linejoin: round; pointer-events: none; }
/* §6.5: a pin whose kind's wires are hidden. */
.pin.kind-hidden { stroke-dasharray: 1 2; fill: none; opacity: 0.6; }
/* §7: chips as packages: a dark body, pins as short legs (phase 4: no notch, nothing on a chip looks like a pin unless it is one). */
.node:not(.pins):not(.control) > .box { fill: color-mix(in srgb, var(--signal-panel, #1f232b) 55%, #000); }
.leg { fill: var(--signal-muted, #8a93a3); opacity: 0.8; }
`;

/** The plate face's own CSS (in its shadow root, beside the faces'): its layers. */
export const PLATE_FACE_CSS = `
/* ADR 0047 §5: X-ray keeps only the plate's controls (whole); the cards are the drawing's ghosts. Board shows none of the plate: its vias are the drawing's pads. */
/* Board keeps X-ray's visibility (only the controls), so a step from X-ray to Board only fades what X-ray showed: the cards never
   come back for the length of the fade (the owner, 2026-10-30: "from X-ray to Board a jump"). */
:host(.plate.layer-xray) .face-panel, :host(.plate.layer-board) .face-panel { visibility: hidden; }
:host(.plate.layer-xray) .face-panel .signal-cards > * { pointer-events: none; }
:host(.plate.layer-xray) .face-panel [bind], :host(.plate.layer-board) .face-panel [bind] { visibility: visible; }
:host(.plate.layer-xray) .face-panel [bind] { pointer-events: auto; }
:host(.plate.layer-xray) .face-panel signal-slot[bind], :host(.plate.layer-board) .face-panel signal-slot[bind] { visibility: hidden; pointer-events: none; }
:host(.plate.layer-board) { opacity: 0; pointer-events: none; }
:host(.plate.layer-board) .face-panel * { pointer-events: none !important; }
/* A layer step fades from where it is to where it goes, never through another value (the owner's "it flashes"): a transition, no animation. */
:host(.plate) { transition: opacity ${LIFT_MS}ms ease-out; }
@media (prefers-reduced-motion: reduce) { :host(.plate) { transition: none; } }
`;

const SVG = 'http://www.w3.org/2000/svg';
function el(name, attributes = {}, text = null) {
  const e = document.createElementNS(SVG, name);
  for (const [k, v] of Object.entries(attributes)) e.setAttribute(k, v);
  if (text !== null) e.textContent = text;
  return e;
}

/** The plate's controls as `panelControls` counts them, in document order: the bound `signal-*` elements of `panel`. */
function controlElements(panel) {
  const out = [];
  for (const element of panel.querySelectorAll('[bind]')) {
    const name = element.localName;
    if (!name.startsWith('signal-') || NOT_CONTROLS.has(name.slice(7))) continue;
    if (!bindList(element.getAttribute('bind')).length) continue;
    out.push(element);
  }
  return out;
}

/** A `bind` as compared: its addresses, one space apart (the faces write it so). */
const bindKey = (bind) => bindList(bind).join(' ');

export class SchematicBoard {
  /** `host`: the <signal-schematic>. */
  constructor(host) {
    this.host = host;
    this.state = viewState(null);
    /** The plate as measured: `{size, vias, obstacles, cards, key}`. */
    this.plate = { size: null, vias: new Map(), obstacles: [], cards: [], key: '' };
  }

  /**
   * No plate measured (another project, or a project without a panel file):
   * no size, vias, obstacles or cards, and nothing of the last plate
   * observed (review 2026-10-31).
   */
  resetPlate() {
    this.plate = { size: null, vias: new Map(), obstacles: [], cards: [], key: '' };
    this.sizes?.disconnect();
    this.observed = new Set();
  }

  /** The bar's switches, after the breadcrumbs. */
  build(bar, before) {
    this.layerButtons = new Map();
    const layers = document.createElement('span');
    layers.className = 'layers';
    layers.setAttribute('role', 'group');
    layers.setAttribute('aria-label', 'Layer');
    for (const layer of LAYERS) {
      const b = document.createElement('button');
      b.type = 'button';
      b.textContent = LAYER_NAMES[layer];
      b.title = { plate: 'The plate: the panel whole, the board under it', xray: 'X-ray: the board under the plate, its controls whole', board: 'The board: the chips and their wiring, the plate’s controls as vias' }[layer];
      b.addEventListener('click', () => this.set({ layer }));
      layers.append(b);
      this.layerButtons.set(layer, b);
    }
    const kinds = document.createElement('span');
    kinds.className = 'kinds';
    kinds.setAttribute('role', 'group');
    kinds.setAttribute('aria-label', 'Signal kinds drawn');
    this.kindButtons = new Map();
    for (const kind of KINDS) {
      const b = document.createElement('button');
      b.type = 'button';
      b.textContent = kind;
      b.title = `Draw the ${kind} wires (the layout does not move)`;
      b.addEventListener('click', () => this.set({ hidden: toggleKind(this.state.hidden, kind) }));
      kinds.append(b);
      this.kindButtons.set(kind, b);
    }
    this.measures = document.createElement('span');
    this.measures.className = 'measures';
    this.measures.title = 'The board’s wiring as measured (ADR 0047 §6): shown, never deciding';
    bar.insertBefore(layers, before);
    bar.insertBefore(kinds, before);
    bar.insertBefore(this.measures, before);
    this.showState();
  }

  showState() {
    for (const [layer, b] of this.layerButtons ?? []) b.setAttribute('aria-pressed', String(layer === this.state.layer));
    for (const [kind, b] of this.kindButtons ?? []) b.setAttribute('aria-pressed', String(!this.state.hidden.includes(kind)));
    const svg = this.host.svg;
    if (svg) for (const layer of LAYERS) svg.classList.toggle(`layer-${layer}`, layer === this.state.layer);
    this.host.faces?.setPlateClass?.(this.plateClass);
  }

  /** The state changed by `change` (a click, the key, the window's kept state): drawn, and told to the page's host. */
  set(change, { tell = true } = {}) {
    const next = viewState({ ...this.state, ...change });
    const was = this.state;
    if (next.layer === was.layer && next.hidden.join() === was.hidden.join()) return;
    this.state = next;
    this.showState();
    this.host.redrawAll?.();
    if (tell) this.host.dispatchEvent(new CustomEvent('signal-view-change', { bubbles: true, composed: true, detail: { ...this.state } }));
  }

  /** The key's step to the next layer. */
  stepLayer() {
    this.set({ layer: nextLayer(this.state.layer) });
  }

  get plateClass() {
    return ` plate layer-${this.state.layer}`;
  }

  /** The Plate layer covers the board: a press there is the background's. */
  get covered() {
    return this.state.layer === 'plate';
  }

  /** What the top level's layout is asked with: `layout`'s `board`. */
  option() {
    return { vias: this.plate.vias, obstacles: this.plate.obstacles, plate: this.plate.size, cards: this.plate.cards };
  }

  /** What every level's drawing depends on of this state (its cache key). */
  get key() {
    return `${this.state.hidden.join(',')}`;
  }

  /** The wires of `level` (a scene entry) to draw, given its `solo` set and `keyOf`. */
  wires(level, solo, keyOf) {
    if (!level.layout.board && !this.state.hidden.length) return level.wires;
    return shownWires(level.wires, this.state.hidden, solo, keyOf);
  }

  /** Whether a pin of `shape` is of a hidden kind. */
  hiddenKind(shape) {
    return this.state.hidden.includes(shape);
  }

  /** The bar's measures of the top level's board. */
  showMeasures(layout) {
    if (this.measures) this.measures.textContent = metricsText(layout?.board?.metrics ?? null, layout?.board?.ribbons ?? null);
  }

  // The plate.

  /**
   * The plate face `face` measured (after it is drawn or resized): its size,
   * each control's rectangle (the vias), the cards' (the X-ray's ghosts).
   * The top level is laid out again when they changed.
   */
  measurePlate(face) {
    const box = face.panel.getBoundingClientRect?.();
    const scale = box?.width > 0 ? box.width / (face.layoutWidth || 1280) : 0;
    if (!scale) return;
    const rel = (element) => {
      const r = element.getBoundingClientRect();
      return { x: (r.left - box.left) / scale, y: (r.top - box.top) / scale, w: r.width / scale, h: r.height / scale };
    };
    const controls = this.host.panelControls ?? [];
    // Counted on both sides alike: by tag and normalised bind, in document order (a control's `occurrence` counts its raw bind).
    const byKey = new Map();
    const counted = new Map();
    for (const c of controls) {
      const key = `${c.tag}\n${bindKey(c.attributes.bind)}`;
      const n = counted.get(key) ?? 0;
      counted.set(key, n + 1);
      byKey.set(`${key}\n${n}`, c);
    }
    const elements = controlElements(face.panel);
    this.watchPlate(face, elements);
    // Each control of the text finds its element: the `occurrence`-th of its tag and bind.
    const seen = new Map();
    const found = new Map();
    const others = [];
    for (const element of elements) {
      const bind = bindKey(element.getAttribute('bind'));
      const key = `${element.localName}\n${bind}`;
      const n = seen.get(key) ?? 0;
      seen.set(key, n + 1);
      const control = byKey.get(`${key}\n${n}`);
      if (control) found.set(control.index, rel(element));
      else others.push(rel(element));
    }
    const { vias, obstacles, key } = plateVias(controls, (c) => found.get(c.index) ?? null, others);
    const size = plateSize(face.layoutWidth || box.width / scale, face.panel.offsetHeight || box.height / scale);
    const cards = [];
    for (const card of face.panel.querySelectorAll?.('.signal-cards > *') ?? []) {
      const r = rel(card);
      if (!(r.w > 0 && r.h > 0)) continue;
      const title = card.querySelector?.('h1, h2, h3')?.textContent?.trim() ?? '';
      cards.push({ ...r, title });
    }
    // The cards' ghosts are drawn with the board: a card moved or renamed draws it again.
    const ghosts = cards.map((c) => `${Math.round(c.x)},${Math.round(c.y)},${Math.round(c.w)},${Math.round(c.h)},${c.title}`).join(';');
    const whole = `${key}|${size.w}x${size.h}|${ghosts}`;
    if (whole === this.plate.key) return;
    this.plate = { size, vias, obstacles, cards, key: whole };
    this.host.relayoutTop?.();
  }

  /**
   * The plate measured again whenever one of its controls or cards changes
   * size or a style sheet of its face loads (a piano roll laid out late, the
   * project's CSS): the vias are the controls as the page draws them now.
   * The observer measures the plate face shown when it fires (review
   * 2026-10-31: a replaced face, never the one it was made for), and
   * observes exactly the elements measured, by identity (a control replaced
   * by another, their count the same, is let go of).
   */
  watchPlate(face, elements) {
    const View = face.panel.ownerDocument?.defaultView ?? globalThis;
    if (!this.sizes && View.ResizeObserver) this.sizes = new View.ResizeObserver(() => this.scheduleMeasure());
    if (!this.sizes) return;
    this.linked ??= new WeakSet();
    for (const link of face.shadow?.querySelectorAll?.('link') ?? []) {
      if (this.linked.has(link)) continue;
      this.linked.add(link);
      link.addEventListener('load', () => this.scheduleMeasure());
    }
    const all = [...elements, ...(face.panel.querySelectorAll?.('.signal-cards > *') ?? [])];
    const observed = this.observed ?? new Set();
    if (observed.size === all.length && all.every((element) => observed.has(element))) return;
    this.sizes.disconnect();
    this.observed = new Set(all);
    for (const element of all) this.sizes.observe(element);
  }

  /** The plate measured at the next frame: the plate face shown then, if any. */
  scheduleMeasure() {
    if (this.measuring) return;
    this.measuring = true;
    requestAnimationFrame(() => {
      this.measuring = false;
      this.remeasure();
    });
  }

  /** The panel's controls read again (`panelSeen`): the plate drawn now is measured for them. */
  remeasure() {
    const face = this.host.faces?.faces?.get(PLATE_FACE);
    if (face) this.measurePlate(face);
  }

  /** The plate's area and its cards' ghosts for the top level's group `group` (in the drawing's baked units). */
  drawPlate(group, ghosts, baked, bakedRect, bakedPoint, faceArea, text, layout = null) {
    const size = this.plate.size;
    const parts = [];
    // The board's sections (the owner's verdict on 7a62595): each card a darker base with its card's outline, what lies in it within.
    for (const section of layout?.board?.sections ?? []) {
      const r = bakedRect(baked, section);
      if (r) parts.push(el('rect', { class: 'board-section', rx: 10 * baked.m, ...r }));
      const c = bakedRect(baked, section.card);
      if (c) parts.push(el('rect', { class: 'board-section-edge', rx: 8 * baked.m, ...c }));
    }
    ghosts.replaceChildren();
    // A project without a panel file has no plate (nothing read, nothing said).
    if (!this.host.panelText) {
      group.replaceChildren();
      return;
    }
    for (const card of this.plate.cards) {
      const r = bakedRect(baked, card);
      if (r) ghosts.append(el('rect', { class: 'plate-card', rx: 8 * baked.m, ...r }));
      if (card.title && text(12)) {
        const at = bakedPoint(baked, { x: card.x + 12, y: card.y + 20 });
        ghosts.append(el('text', { class: 'plate-title', x: at.x, y: at.y, 'font-size': 13 * baked.m }, card.title));
      }
    }
    parts.push(faceArea(baked, { x: 0, y: 0, w: size?.w ?? 1280, h: size?.h ?? 800, file: PLATE_FILE, natural: { w: size?.w ?? 1280 } }, PLATE_FACE, { path: '', root: '' }));
    group.replaceChildren(...parts);
  }

  /**
   * The ribbons of a board level (ADR 0047 phase 4, `layout.board.ribbons`):
   * each a band under its members (in their tone when they share one), and
   * its label once ("t ×7"), in `bands` and `labels` (appended after the
   * net labels). A ribbon whose members are all of a hidden kind is not drawn.
   */
  drawRibbons(bands, labels, level, baked, bakedRect, bakedPoint, text) {
    const layout = level.layout;
    const parts = [];
    const byId = new Map(layout.wires.map((w) => [w.id, w]));
    for (const r of layout.board?.ribbons ?? []) {
      const members = r.wires.map((id) => byId.get(id)).filter(Boolean);
      if (!members.length || members.every((w) => this.state.hidden.includes(w.shape))) continue;
      const tones = new Set(members.map((w) => level.tones?.get(w.id) ?? (w.kind === 'binding' ? 'binding' : w.shape)));
      const tone = tones.size === 1 ? [...tones][0] : 'mixed';
      const rect = bakedRect(baked, r.band);
      if (rect) parts.push(el('rect', { class: `ribbon ${tone}`, rx: 2 * baked.m, 'data-ribbon': String(r.id), ...rect }));
      // Its label where the layout placed it, off every box and label (`placeRibbonLabels`); none where no place was free.
      if (r.text && text(8)) {
        const at = bakedPoint(baked, r.text);
        const t = el('text', { class: `ribbon-label ${tone}`, x: at.x, y: at.y, 'font-size': 8 * baked.m, 'text-anchor': r.text.anchor, 'data-ribbon': String(r.id) }, r.label);
        t.append(el('title', {}, `A ribbon of ${r.members.length} wires: click one where it leaves the ribbon, or in it zoomed in, to trace it through`));
        labels.append(t);
      }
    }
    bands.replaceChildren(...parts);
  }

  /** The window's kept view state (`ui/windows.toml`, from the page's host): shown, not told back. */
  restore(raw) {
    this.set(viewState(raw), { tell: false });
  }
}

export { XRAY_OPACITY };
