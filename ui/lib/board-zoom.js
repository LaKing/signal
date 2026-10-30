// Deep zoom into a chip (ADR 0047 §8, phase 3): what a die shows at a
// scale and how the view travels to it. A composite's inside (its die) is
// drawn in its package's body (`dieArea`, ui/lib/schematic-layout.js) at
// the scale that fits it there; nothing grows, nothing moves. Its text
// fades in from 4 px on screen and is whole from 7 px (ADR 0030's level of
// detail), and everything around the package fades as the die comes in,
// to SURROUND at most. A click, a double-click or Enter zooms the view in
// ZOOM_MS until the chip fills it and its die is readable; Escape and a
// breadcrumb zoom back out; zooming out past the threshold leaves the die.
// All of it is view state (rule 4). Pure; no DOM.

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

/** The scale (on screen) at which a die is drawn whole. */
export const FULL_K = FADE_FULL_PX / DIE_TEXT;
/** A zoom into a chip draws its die at most this large (a small or empty inside does not blow up: 2026-10-29's empty slot). */
export const DIE_MAX_K = 2;

/**
 * The view (in `view`'s coordinates) that zooms into a chip: its package's
 * screen rectangle `rect` (as drawn in `view` now) fills `viewport` less
 * `margin` (`focus`'s fit), and its die, drawn at screen scale `dieK` now,
 * is drawn whole (FULL_K) when the fit leaves it smaller: then zoomed on
 * about the package's centre, which overflows the screen (a large inside,
 * such as moog_voice, is read by panning); never larger than DIE_MAX_K
 * (a small inside then lies in the middle, its package smaller than the
 * stage).
 */
export function zoomInView(view, rect, viewport, dieK, margin = 24) {
  const w = Math.max(1, viewport.w - 2 * margin);
  const h = Math.max(1, viewport.h - 2 * margin);
  const f = Math.min(w / Math.max(rect.w, 1e-9), h / Math.max(rect.h, 1e-9));
  let factor = f;
  if (dieK > 0 && dieK * factor < FULL_K) factor = FULL_K / dieK;
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
