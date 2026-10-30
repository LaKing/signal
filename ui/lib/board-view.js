// The device view's state and the plate's measures (ADR 0047 §5, phase
// 2): the layer shown (Plate, X-ray, Board), the signal kinds whose wires
// are hidden, and the vias, the plate's controls as rectangles in whole
// plate units. View state only: it never changes the layout's input, nor
// what a project means (rule 4). The board has one arrangement (the owner's
// verdict of 2026-10-30). Pure; no DOM.

import { PU } from './board-layout.js';
import { CORNER } from './board-route.js';

/** A board wire's turns are rounded to this radius on screen (px) at every zoom (ADR 0047 phase 4: no fat arcs zoomed in). */
export const CORNER_PX = 6;

/**
 * The corner radius (layout units) of a board level drawn at scale `k`:
 * CORNER_PX on screen, in half-octave steps of the scale (so a zoom
 * rebuilds the paths only at a step), never more than the layout's CORNER
 * (zoomed out, a turn is never rounder than the route).
 */
export function cornerAt(k) {
  if (!(k > 0)) return CORNER;
  const step = 2 ** (Math.round(Math.log2(k) * 2) / 2);
  return Math.min(CORNER, CORNER_PX / step);
}

/** The layers, in the order one key steps through them. */
export const LAYERS = Object.freeze(['plate', 'xray', 'board']);
export const LAYER_NAMES = Object.freeze({ plate: 'Plate', xray: 'X-ray', board: 'Board' });
/** The signal kinds a wire is drawn as (`shapeName`), each a layer of its own. */
export const KINDS = Object.freeze(['signal', 'events', 'vector']);
/** The plate's opacity in X-ray (about a quarter). */
export const XRAY_OPACITY = 0.25;
/** The plate lifts and fades in this long (none with reduced motion). */
export const LIFT_MS = 200;

/**
 * What each layer shows, as the opacities the CSS (ui/controls/schematic-
 * board.js) fades to: the plate's live controls (`controls`), the plate's
 * cards themselves (`cards`: shown only on the Plate; X-ray and Board both
 * hide them, so a step between those never shows them), the cards'
 * ghosts over the board (`ghosts`), the board itself (`board`). A step
 * between layers is a transition from the opacity shown to the target,
 * never through another value (`fadeSteps`).
 */
export const LAYER_OPACITY = Object.freeze({
  plate: Object.freeze({ controls: 1, cards: 1, ghosts: 0, board: 0 }),
  xray: Object.freeze({ controls: 1, cards: 0, ghosts: XRAY_OPACITY, board: 1 }),
  board: Object.freeze({ controls: 0, cards: 0, ghosts: 0, board: 1 }),
});

/** The opacities a step from layer `from` to `to` passes through, sampled at `n` + 1 instants (linear, as the transition is). */
export function fadeSteps(from, to, part, n = 4) {
  const a = LAYER_OPACITY[from][part];
  const b = LAYER_OPACITY[to][part];
  return Array.from({ length: n + 1 }, (_, i) => a + ((b - a) * i) / n);
}

/** The state a view opens with: the board, every kind shown. */
export const DEFAULT_VIEW = Object.freeze({ layer: 'board', hidden: Object.freeze([]) });

/** `raw` (a page's, or `ui/windows.toml`'s) made a view state: anything unknown is the default's. */
export function viewState(raw) {
  const layer = LAYERS.includes(raw?.layer) ? raw.layer : DEFAULT_VIEW.layer;
  const hidden = KINDS.filter((k) => Array.isArray(raw?.hidden) && raw.hidden.includes(k));
  return { layer, hidden };
}

/** The layer after `layer` (the key that steps through them). */
export function nextLayer(layer) {
  const i = LAYERS.indexOf(layer);
  return LAYERS[(i + 1) % LAYERS.length];
}

/** `hidden` with `kind` switched. */
export function toggleKind(hidden, kind) {
  return KINDS.filter((k) => (k === kind ? !hidden.includes(k) : hidden.includes(k)));
}

/**
 * A control's rectangle as measured on the plate (CSS px at the plate's own
 * size) in whole plate units (ADR 0047 §5, determinism of vias): its
 * corner and its size each rounded to the nearest unit, at least one, so
 * engines that differ by a fraction of a pixel give the same board.
 */
export function viaRect(rect) {
  // First to 1/64 px (ADR 0047 phase 4): a fraction an engine's transform adds or takes (a control at 4.0001 or 3.9999 px,
  // a plate unit's half) never flips the unit it snaps to, so a plate measured again gives the same vias and the board stays.
  const snap = (v) => Math.round(Math.round(v * 64) / 64 / PU) * PU;
  return { x: snap(rect.x), y: snap(rect.y), w: Math.max(PU, snap(rect.w)), h: Math.max(PU, snap(rect.h)) };
}

/**
 * The vias of a plate measured: `controls` (`panelControls`') and
 * `measured(control)`, its rectangle (CSS px, plate origin) or null when
 * it is not drawn; `others`, the rectangles of the plate's other controls
 * (unbound ones). `{vias, obstacles, key}`: `vias` a Map of a control's
 * index → its rectangle in whole units; `obstacles` every control's;
 * `key` the same text for the same vias.
 */
export function plateVias(controls, measured, others = []) {
  const vias = new Map();
  const obstacles = [];
  for (const control of controls) {
    const r = measured(control);
    if (!r || !(r.w > 0) || !(r.h > 0)) continue;
    const via = viaRect(r);
    vias.set(control.index, via);
    obstacles.push(via);
  }
  for (const r of others) if (r?.w > 0 && r?.h > 0) obstacles.push(viaRect(r));
  const rect = (v) => `${v.x},${v.y},${v.w},${v.h}`;
  const key = `${[...vias].map(([i, v]) => `${i}:${rect(v)}`).join(' ')}|${obstacles.map(rect).join(' ')}`;
  return { vias, obstacles, key };
}

/** A plate's measured size (CSS px) in whole units, rounded up: `{w, h}` in layout units. */
export function plateSize(w, h) {
  const up = (v) => Math.max(PU, Math.ceil(v / PU - 1e-9) * PU);
  return { w: up(w), h: up(h) };
}

/** The measures of a board (`routeWires`' `metrics`) as the bar says them; never a judgement (hard rule 6). */
export function metricsText(metrics, ribbons = null) {
  if (!metrics) return '';
  const n = (v) => (v >= 10000 ? `${(v / 1000).toFixed(1)}k` : String(v));
  // ADR 0047 phase 4: the ribbons drawn, when there are any.
  const more = ribbons?.length ? ` · ${n(ribbons.length)} ribbons` : '';
  // ADR 0047 phase 5: no net is a label any more; the bar says the wiring.
  return `${n(metrics.crossings)} crossings · ${n(metrics.bends)} bends · length ${n(metrics.length)}${more}`;
}

/**
 * The wires of a board level to draw: those of a hidden kind left out;
 * a labelled wire in `solo` (traced, selected) drawn whole.
 */
export function shownWires(wires, hidden, solo, keyOf) {
  const out = [];
  for (const w of wires) {
    if (hidden.includes(w.shape)) continue;
    out.push(w.labelled && w.whole && solo.has(keyOf(w)) ? w.whole : w);
  }
  return out;
}
