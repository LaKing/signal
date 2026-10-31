// Deep zoom into a chip (ADR 0047 §8, phase 3; the owner's verdict on
// 9e81dc8: a microscope): what a die shows at a scale and how the view
// travels to it. A composite's inside (its die) is laid out at its natural
// size and drawn scaled into its package, which has its shape and its pins
// (`shapePackage`, `dieLink`); nothing grows, nothing moves. Its text fades in from 4 px on
// screen and is whole from 7 px (ADR 0030's level of detail), and
// everything around the package fades as the die comes in, to SURROUND. A
// click, a double-click or Enter zooms the view in ZOOM_MS until the package
// fills the stage; Escape and a breadcrumb zoom back out;
// zooming out until the die is small or unreadable leaves it, and
// (2026-10-31, semantic zoom) zooming in until it shows opens it, along the
// same fade (`shownOpacity`, `risen`). All of it is view state (rule 4).
// Pure; no DOM.

import { zoomAt, pan } from './schematic-view.js';

/** A die's text (a chip's title, layout units) decides how much of it is drawn. */
export const DIE_TEXT = 12;
/** It starts to fade in when that text is this large on screen (px), */
export const FADE_FROM_PX = 4;
/** and is drawn whole from this size. */
export const FADE_FULL_PX = 7;
/** Around a die drawn whole, everything else is drawn at this opacity (the owner: about 15–25 %). */
export const SURROUND = 0.2;
/** The view travels to a chip (or back out) in this long (ms); at once with reduced motion. */
export const ZOOM_MS = 300;

/** How much of a die drawn at scale `k` (its level's, on screen) is shown: 0 below FADE_FROM_PX, 1 from FADE_FULL_PX, linear between. */
export function dieOpacity(k) {
  const px = DIE_TEXT * k;
  if (!(px > FADE_FROM_PX)) return 0;
  if (px >= FADE_FULL_PX) return 1;
  return (px - FADE_FROM_PX) / (FADE_FULL_PX - FADE_FROM_PX);
}

/** The opacity of what lies around a die shown at `opacity` (`dieOpacity`): 1 with none, SURROUND with one drawn whole. */
export function surroundOpacity(opacity) {
  return 1 - (1 - SURROUND) * Math.min(1, Math.max(0, opacity));
}

/** Whether a die drawn at scale `k` is gone (zoomed out past its threshold): its level is left. */
export function belowThreshold(k) {
  return dieOpacity(k) === 0;
}

/** A die zoomed out until its rectangle is smaller than this share of the stage both ways is left (it opens at its natural size, so its text alone would keep it until the whole board's is unreadable). */
export const LEAVE_SHARE = 0.25;
/** A die's rectangle this large a share of the stage (its larger way) or more no longer holds its fade back (the size's own band, LEAVE_SHARE to this). */
export const SHARE_FULL = 0.4;

/** How much the size on the stage of a die's rectangle `rect` lets it show: 0 under LEAVE_SHARE of the stage both ways, 1 from SHARE_FULL either way, linear between. */
export function shareOpacity(rect, viewport) {
  if (!(viewport?.w > 0) || !(viewport?.h > 0)) return 1;
  const share = Math.max(rect.w / viewport.w, rect.h / viewport.h);
  if (!(share > LEAVE_SHARE)) return 0;
  if (share >= SHARE_FULL) return 1;
  return (share - LEAVE_SHARE) / (SHARE_FULL - LEAVE_SHARE);
}

/**
 * How much of a die is shown (2026-10-31, semantic zoom): drawn at scale
 * `k` on screen, its rectangle `rect` on screen in `viewport`, the lesser
 * of its text's fade (`dieOpacity`) and its size's (`shareOpacity`). One
 * function of the view alone, so a die fades in as it is zoomed into and
 * out as it is zoomed out of along the same curve; 0 is where it is left
 * (`zoomedOutOf`) and where the zoom opens it.
 */
export function shownOpacity(k, rect, viewport) {
  return Math.min(dieOpacity(k), shareOpacity(rect, viewport));
}

/** Whether a die drawn at scale `k`, its rectangle `rect` on screen, is zoomed out of: under its text's threshold, or small on the stage (`shownOpacity` 0). */
export function zoomedOutOf(k, rect, viewport) {
  return shownOpacity(k, rect, viewport) === 0;
}

/** A die shown less than this (`shownOpacity`) is not hit: its package is, as when it is not drawn. */
export const HIT_OPACITY = 0.5;

/** The scale (on screen) at which a die is drawn whole. */
export const FULL_K = FADE_FULL_PX / DIE_TEXT;
/** A zoom into a chip draws its die at most this large (a small or empty inside does not blow up: 2026-10-29's empty slot). */
export const DIE_MAX_K = 2;

/**
 * The view (in `view`'s coordinates) that zooms into a chip (ADR 0047 §8,
 * the owner's verdict on 9e81dc8: a microscope): its package's screen
 * rectangle `rect` (as drawn in `view` now) fills `viewport` less `margin`,
 * centred; its die, drawn at screen scale `dieK` now, never larger than
 * DIE_MAX_K (a small or empty inside does not blow up). The person zooms
 * on from there to read details.
 */
export function zoomInView(view, rect, viewport, dieK, margin = 24) {
  const w = Math.max(1, viewport.w - 2 * margin);
  const h = Math.max(1, viewport.h - 2 * margin);
  let factor = Math.min(w / Math.max(rect.w, 1e-9), h / Math.max(rect.h, 1e-9));
  if (dieK > 0 && dieK * factor > DIE_MAX_K) factor = DIE_MAX_K / dieK;
  const centre = { x: rect.x + rect.w / 2, y: rect.y + rect.h / 2 };
  // Zoomed about the package's centre, then its centre moved to the viewport's.
  const zoomed = zoomAt(view, factor, centre);
  return pan(zoomed, viewport.w / 2 - centre.x, viewport.h / 2 - centre.y);
}

/** The ease of a zoom's travel: slow, fast, slow (cubic). */
export function ease(t) {
  const u = Math.min(1, Math.max(0, t));
  return u < 0.5 ? 4 * u * u * u : 1 - (-2 * u + 2) ** 3 / 2;
}

/**
 * The view at `t` (0 to 1, eased by the caller) of the travel from `from`
 * to `to` (two views of one level's coordinates): a zoom about the screen
 * point both views keep in place, the scale changing geometrically, so the
 * chip grows where it is on its way to the screen's middle; a pan alone
 * when the scale stays. `to` itself at 1.
 */
export function tween(from, to, t) {
  if (t >= 1) return to;
  if (t <= 0) return from;
  const f = to.k / from.k;
  // Where `to` puts the point `from` shows at its own (x, y): `to` = `from` scaled by f about c, then nothing moved.
  const s1 = { x: to.x + (ox(from) - ox(to)) * to.k, y: to.y + (oy(from) - oy(to)) * to.k };
  const tx = s1.x - f * from.x;
  const ty = s1.y - f * from.y;
  if (Math.abs(Math.log(f)) < 1e-6) return pan(from, tx * t, ty * t);
  const c = { x: tx / (1 - f), y: ty / (1 - f) };
  return zoomAt(from, f ** t, c);
}

const ox = (view) => view.ox ?? 0;
const oy = (view) => view.oy ?? 0;
