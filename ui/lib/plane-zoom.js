// The Signal Project plane's zoom (ADR 0051 §1, §8): the levels by zoom
// and focus, and the fades between them. The plane's view is one uniform
// scale `k` from plane units (device units: 1 CSS px of a front panel at
// its own size; a pu is 8) to screen px, and a translation:
// `screen = plane · k + (x, y)`. A Stack's **fit** is the scale at which its
// width fills the stage; the zoom's ratio to it, `r = k / fit`, decides the
// level:
//   - Setup, below the fit: every Stack and peripheral in view;
//   - the focus band (FOCUS_BAND, below the fit): Setup cables, peripherals
//     and other Stacks fade to SURROUND as the Stack comes to fill the stage;
//   - Stack, from the fit: the Stack fills the stage's width and the wheel
//     scrolls it vertically;
//   - the wiring band (WIRING_BAND, in depth `z`, tunable): at the fit,
//     zooming on first deepens the view by LIFT without changing its scale
//     (`depthOf`), a cross-fade in place from the front panels to the
//     Wiring level, the back of the rack (ui/lib/plane-wiring.js), with the
//     Stack's whole width on stage;
//   - the control band (CONTROL_BAND, in `z`, past the lift, where the
//     scale grows again): the instrument under the zoom point opens into
//     its Control plane (ui/lib/plane-control.js), the others fade to
//     SURROUND;
//   - the engine band (`engineBand`, from the cube's size): the Engine
//     layer (ui/lib/plane-engine.js), every instrument's inside in its
//     rectangle, fades in.
// The rectangles never move: the levels are nested footprints.
// One curve in and out, no timer: the opacities are functions of the depth
// `z` only (`r` below the fit), monotonic, exactly 0 and 1 at a band's ends. View state only (rule 4).
// Pure; no DOM.

import { SURROUND } from './board-zoom.js';

export { SURROUND };

/** The focus band, in `r`: Setup things fade from its start to SURROUND at its end (the fit). */
export const FOCUS_BAND = Object.freeze([0.6, 1]);
/**
 * The wiring band, in depth `z` (ADR 0051 §8, tunable; tuned 2026-11-01 by
 * the coordinator's review of phase 2: at 2× only half the Stack was on
 * stage). It lies at the Stack's width fit: zooming on past the fit first
 * deepens the view by LIFT without changing its scale (`depthOf`), so the
 * front panels cross-fade to the back of the rack with the whole Stack's
 * width on stage; only past the band does the scale grow again.
 */
export const WIRING_BAND = Object.freeze([1.05, 1.3]);
/** The control band, in `z` (tunable): one instrument dominates the stage; it opens into its Control plane, the others fade to SURROUND. */
export const CONTROL_BAND = Object.freeze([1.6, 2]);
/** How far the depth goes at the fit, its scale unchanged: through the whole wiring band. */
export const LIFT = WIRING_BAND[1] - 1;
/** The engine band starts at this share of the ratio at which the cube fills the stage, and never before CONTROL_BAND's end plus ENGINE_GAP. */
export const ENGINE_FROM = 0.6;
export const ENGINE_GAP = 0.5;
/** The stage's margin around a fitted Stack or the whole plane (px). */
export const MARGIN_PX = 24;
/** The view's scale limits relative to the fit: far enough out for any Setup, in to the engine band's end. */
export const MIN_R = 0.01;
export const MAX_R = CONTROL_BAND[1] + 2 * ENGINE_GAP;
/** A ratio within this of 1 is the fit (rounding of a fitted view). */
const FIT_EPSILON = 1e-6;

/**
 * The depth `z` of a view at scale ratio `r` (its scale over the Stack's
 * fit) with `lift` (0 to LIFT): `r` below the fit; at and above it, `r`
 * plus the lift. Every band and level is a function of `z`, so the levels
 * stay one continuous, monotonic curve of the zoom.
 */
export function depthOf(r, lift = 0) {
  if (!(r >= 1 - FIT_EPSILON)) return r;
  return r + Math.min(LIFT, Math.max(0, lift));
}

/** The scale ratio and the lift of depth `z` (`depthOf`'s inverse): below the fit no lift; at the fit the lift first; then the scale. */
export function splitDepth(z) {
  if (z < 1) return { r: z, lift: 0 };
  if (z <= 1 + LIFT) return { r: 1, lift: z - 1 };
  return { r: z - LIFT, lift: LIFT };
}

/** The identity view. */
export const IDENTITY = Object.freeze({ k: 1, x: 0, y: 0 });

/** A plane point on the screen. */
export function toScreen(view, p) {
  return { x: p.x * view.k + view.x, y: p.y * view.k + view.y };
}

/** A screen point on the plane. */
export function toPlane(view, p) {
  return { x: (p.x - view.x) / view.k, y: (p.y - view.y) / view.k };
}

/** A plane rectangle on the screen. */
export function screenRect(view, r) {
  return { x: r.x * view.k + view.x, y: r.y * view.k + view.y, w: r.w * view.k, h: r.h * view.k };
}

/**
 * How far `r` is into `band` (`[a, b]`): 0 at and below `a`, 1 at and above
 * `b`, a smoothstep between (monotonic, flat at both ends, so a fade starts
 * and ends gently).
 */
export function bandShare(r, band) {
  const [a, b] = band;
  if (!(r > a)) return 0;
  if (r >= b) return 1;
  const t = (r - a) / (b - a);
  return t * t * (3 - 2 * t);
}

/**
 * The engine band (in `r`) of a cube that fills the stage at ratio
 * `cubeR` (its fit scale over the Stack's): from ENGINE_FROM of it to it,
 * never starting before CONTROL_BAND's end plus ENGINE_GAP, nor shorter
 * than ENGINE_GAP. Without a cube: the default band.
 */
export function engineBand(cubeR = 0) {
  const b = Math.max(CONTROL_BAND[1] + 2 * ENGINE_GAP, Number.isFinite(cubeR) ? cubeR : 0);
  return Object.freeze([Math.max(CONTROL_BAND[1] + ENGINE_GAP, Math.min(b - ENGINE_GAP, ENGINE_FROM * b)), b]);
}

/** The engine band without a cube measured. */
export const ENGINE_BAND = engineBand();

/**
 * The opacities at ratio `r` (`engine`: the opened instrument's engine
 * band, `engineBand`'s). How far into each band: `focus`, `wiring`,
 * `control`, `engine`. Then what each drawing shows:
 *   - `setup`: Setup cables, peripherals, other Stacks, 1 down to SURROUND
 *     over the focus band;
 *   - `panels`: the focused Stack's front panels, 1 down to 0 over the
 *     wiring band;
 *   - `board`: the Wiring drawing of the instruments not opened, 0 up to 1
 *     over the wiring band, then down to SURROUND over the control band;
 *   - `openWiring`: the opened instrument's Wiring box, up over the wiring
 *     band and out over the control band, as its Control plane comes in;
 *   - `sheet`: the opened instrument's Control plane, 0 up to 1 over the
 *     control band, then down to SURROUND over the engine band;
 *   - `controls`: its live controls (its front panel's), out over the
 *     wiring band and back over the control band, never below `panels`;
 *   - `deep`: the Engine layer, 0 up to 1 over the engine band.
 * Functions of `r` alone, so in and out are one curve (no timer).
 */
export function planeFades(r, engine = ENGINE_BAND) {
  const focus = bandShare(r, FOCUS_BAND);
  const wiring = bandShare(r, WIRING_BAND);
  const control = bandShare(r, CONTROL_BAND);
  const deep = bandShare(r, engine);
  const sheet = control * (1 - (1 - SURROUND) * deep);
  return {
    focus,
    wiring,
    control,
    engine: deep,
    setup: 1 - focus + SURROUND * focus,
    panels: 1 - wiring,
    board: wiring * (1 - (1 - SURROUND) * control),
    openWiring: wiring * (1 - control),
    sheet,
    controls: Math.max(1 - wiring, sheet),
    deep,
  };
}

/** The level at ratio `r`: `setup` below the fit, `stack` from it, `wiring` from the wiring band's end, `control` from the control band's, `engine` from the engine band's. */
export function levelOf(r, engine = ENGINE_BAND) {
  if (r >= engine[1]) return 'engine';
  if (r >= CONTROL_BAND[1]) return 'control';
  if (r >= WIRING_BAND[1]) return 'wiring';
  if (r >= 1 - FIT_EPSILON) return 'stack';
  return 'setup';
}

/** Whether the instrument that opens still follows the zoom point at ratio `r`: below the control band (inside it, another one would jump the fades). */
export function opensAt(r) {
  return r < CONTROL_BAND[0];
}

/**
 * The instrument of `rects` (`[{entry, x, y, w, h}]`, plane units) at plane
 * point `p`, else the nearest one by vertical distance; null without any.
 */
export function instrumentAt(rects, p) {
  let best = null;
  for (const r of rects) {
    const dy = Math.max(r.y - p.y, 0, p.y - (r.y + r.h));
    const dx = Math.max(r.x - p.x, 0, p.x - (r.x + r.w));
    const d = Math.hypot(dx, dy);
    if (!best || d < best.d) best = { entry: r.entry, d };
  }
  return best?.entry ?? null;
}

/** The scale at which a Stack `rect` (plane units) fills a `viewport`'s width less the margins. */
export function fitScale(rect, viewport, margin = MARGIN_PX) {
  return Math.max(1e-6, viewport.w - 2 * margin) / Math.max(1e-9, rect.w);
}

/**
 * The view that fits Stack `rect`'s width to the viewport (ADR 0051 §8:
 * focus). Its top at the margin, or, given `keep` (a view), the plane point
 * at the viewport's vertical centre kept there, clamped to the Stack.
 */
export function fitWidth(rect, viewport, keep = null, margin = MARGIN_PX) {
  const k = fitScale(rect, viewport, margin);
  const x = margin - rect.x * k;
  let y = margin - rect.y * k;
  if (keep) {
    const centre = toPlane(keep, { x: viewport.w / 2, y: viewport.h / 2 }).y;
    y = viewport.h / 2 - centre * k;
  }
  return clampScroll({ k, x, y }, rect, viewport, margin);
}

/**
 * `view` with its vertical position kept inside Stack `rect`: its top no
 * lower than the margin, its bottom no higher than the viewport's bottom
 * less the margin (a Stack shorter than the stage: its top at the margin).
 */
export function clampScroll(view, rect, viewport, margin = MARGIN_PX) {
  const top = rect.y * view.k + view.y;
  const height = rect.h * view.k;
  let y = view.y;
  if (height <= viewport.h - 2 * margin) y += margin - top;
  else if (top > margin) y += margin - top;
  else if (top + height < viewport.h - margin) y += viewport.h - margin - (top + height);
  return { ...view, y };
}

/** The Stack level's wheel: `view` scrolled by `dy` screen px, kept inside Stack `rect`. */
export function scrollStack(view, dy, rect, viewport, margin = MARGIN_PX) {
  return clampScroll({ ...view, y: view.y - dy }, rect, viewport, margin);
}

/** The view that shows `bounds` (plane units) whole, centred, less the margins. */
export function fitAll(bounds, viewport, margin = MARGIN_PX) {
  const w = Math.max(1, viewport.w - 2 * margin);
  const h = Math.max(1, viewport.h - 2 * margin);
  const k = Math.min(w / Math.max(1e-9, bounds.w), h / Math.max(1e-9, bounds.h));
  return { k, x: viewport.w / 2 - (bounds.x + bounds.w / 2) * k, y: viewport.h / 2 - (bounds.y + bounds.h / 2) * k };
}

/** `view` zoomed by `factor` about screen point `at` (which stays where it is), its ratio to `fit` kept within MIN_R and the scale of depth `maxR` (MAX_R by default). */
export function zoomAbout(view, factor, at, fit = null, maxR = MAX_R) {
  let k = view.k * factor;
  if (fit > 0) k = Math.min(fit * splitDepth(maxR).r, Math.max(fit * MIN_R, k));
  if (!Number.isFinite(k) || k <= 0) return view;
  const f = k / view.k;
  return { k, x: at.x - (at.x - view.x) * f, y: at.y - (at.y - view.y) * f };
}

/** `view` moved by `dx`, `dy` screen px. */
export function panBy(view, dx, dy) {
  return { ...view, x: view.x + dx, y: view.y + dy };
}

/**
 * Whether a zoom from ratio `r0` to `r1` comes to the fit from below (ADR
 * 0051 §8: zooming until a Stack fills the stage focuses it): the view
 * then snaps to the fit, and the next wheel scrolls.
 */
export function reachesFit(r0, r1) {
  return r0 < 1 - FIT_EPSILON && r1 >= 1 - FIT_EPSILON;
}

/**
 * One zoom step of the plane's navigation (ADR 0051 §8): `state`
 * (`{view, opened, …}`) zoomed by `factor` about screen point `at`. Below
 * the control band the instrument that opens follows the zoom point
 * (`rects`, plane units); inside it and past it, it stays. Only `view` and
 * `opened` change: zoom never changes the selection, nor anything else the
 * state holds. `fit`: the focused Stack's fit scale; `maxR`: the zoom's
 * limit (the engine band's end).
 */
export function zoomStep(state, factor, at, { fit = null, rects = [], maxR = MAX_R } = {}) {
  if (!fit) return { ...state, view: zoomAbout(state.view, factor, at) };
  const r0 = state.view.k / fit;
  const z0 = depthOf(r0, state.lift ?? 0);
  const opened = opensAt(z0) && rects.length ? (instrumentAt(rects, toPlane(state.view, at)) ?? state.opened) : state.opened;
  const z1 = Math.min(maxR, Math.max(MIN_R, z0 * factor));
  const { r, lift } = splitDepth(z1);
  // In the lift the scale stays: nothing on stage moves while the levels cross-fade.
  const view = Math.abs(r - r0) < 1e-12 ? state.view : zoomAbout(state.view, (r * fit) / state.view.k, at, fit, maxR);
  return { ...state, view, opened, lift };
}
