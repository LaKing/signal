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
//   - the wiring band (WIRING_BAND, 1.5× to 2× the fit, tunable): a
//     cross-fade in place to the drawing below the front panels (in phase 1
//     the board of ADR 0047, with its deep zoom into dies; ADR 0051 phase 2
//     makes it the Wiring level), past which that drawing has the input.
// One curve in and out, no timer: the opacities are functions of `r` only,
// monotonic, exactly 0 and 1 at a band's ends. View state only (rule 4).
// Pure; no DOM.

import { SURROUND } from './board-zoom.js';

export { SURROUND };

/** The focus band, in `r`: Setup things fade from its start to SURROUND at its end (the fit). */
export const FOCUS_BAND = Object.freeze([0.6, 1]);
/** The wiring band, in `r` (ADR 0051 §8: 1.5× to 2×, tunable). */
export const WIRING_BAND = Object.freeze([1.5, 2]);
/** The stage's margin around a fitted Stack or the whole plane (px). */
export const MARGIN_PX = 24;
/** The view's scale limits relative to the fit: far enough out for any Setup, far enough in for the board's dies (the board zooms on by itself past the band). */
export const MIN_R = 0.01;
export const MAX_R = WIRING_BAND[1];
/** A ratio within this of 1 is the fit (rounding of a fitted view). */
const FIT_EPSILON = 1e-6;

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
 * The opacities at ratio `r`: `focus` and `wiring` (how far into each
 * band), `setup` (Setup cables, peripherals, other Stacks: 1 down to
 * SURROUND over the focus band), `panels` (the focused Stack's front
 * panels: 1 down to 0 over the wiring band) and `deep` (the drawing below
 * them: 0 up to 1 over it).
 */
export function planeFades(r) {
  const focus = bandShare(r, FOCUS_BAND);
  const wiring = bandShare(r, WIRING_BAND);
  return { focus, wiring, setup: (1 - focus) + SURROUND * focus, panels: 1 - wiring, deep: wiring };
}

/** The level at ratio `r`: `setup` below the fit, `stack` from it, `wiring` from the wiring band's end. */
export function levelOf(r) {
  if (r >= WIRING_BAND[1]) return 'wiring';
  if (r >= 1 - FIT_EPSILON) return 'stack';
  return 'setup';
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

/** `view` zoomed by `factor` about screen point `at` (which stays where it is), its ratio to `fit` kept within MIN_R and MAX_R. */
export function zoomAbout(view, factor, at, fit = null) {
  let k = view.k * factor;
  if (fit > 0) k = Math.min(fit * MAX_R, Math.max(fit * MIN_R, k));
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
 * The board's root view (ADR 0047's coordinates, `ui/lib/schematic-view.js`:
 * `screen = (world − o) · k + (x, y)`, o = 0) that lays the board's plate
 * origin on the plane point `origin` under `view`, on a stage whose top-left
 * is `offset` (screen px) from the plane's.
 */
export function boardView(view, origin, offset = { x: 0, y: 0 }) {
  return { k: view.k, ox: 0, oy: 0, x: origin.x * view.k + view.x - offset.x, y: origin.y * view.k + view.y - offset.y };
}

/**
 * Where the board's plate origin lies on the plane (its y; plane units)
 * when the view's centre is at plane y `y`. The board lays the panel's
 * cards as the plate flows them (a title card, its gutters), the Stack at
 * its own pitch (its header, its rails), so one origin lines up one card
 * only. `anchors` (`[{y, origin}]`, by `y`): for each card in both, the
 * plane y of its middle and the origin that lays the board's card on the
 * Stack's. The origin is the anchor's at its middle and linear between
 * anchors (held past the ends): the card in view lines up, and scrolling
 * moves the board continuously, never by a jump. No anchors: `fallback`.
 */
export function boardOriginY(anchors, y, fallback) {
  if (!anchors?.length) return fallback;
  if (!(y > anchors[0].y)) return anchors[0].origin;
  for (let i = 1; i < anchors.length; i++) {
    const a = anchors[i - 1];
    const b = anchors[i];
    if (y <= b.y) return b.y > a.y ? a.origin + ((b.origin - a.origin) * (y - a.y)) / (b.y - a.y) : b.origin;
  }
  return anchors[anchors.length - 1].origin;
}

/** The plane view that `boardView` came from: the board's root view `board` mapped back (its scale is the plane's). */
export function fromBoardView(board, origin, offset = { x: 0, y: 0 }) {
  const k = board.k;
  const sx = (0 - (board.ox ?? 0)) * k + board.x + offset.x;
  const sy = (0 - (board.oy ?? 0)) * k + board.y + offset.y;
  return { k, x: sx - origin.x * k, y: sy - origin.y * k };
}
