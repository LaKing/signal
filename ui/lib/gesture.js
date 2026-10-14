// Gestures to positions (docs/ui-controls.md "Gestures"): drags, the
// knob's circular drag, the wheel and the keys. Everything works on
// positions (0..1, see taper.js), so it is the same at every taper.

import { clamp } from './taper.js';

/** Fine adjust (the Shift key) scales every step by this. */
export const FINE = 0.1;
/** Pixels of drag for the full travel, unless the control says otherwise. */
export const DRAG_PIXELS = 200;
/** An arrow key's step. */
export const KEY_STEP = 0.01;
/** A Page key's step. */
export const PAGE_STEP = 0.1;

/** The position change for `pixels` of drag (up or right positive). */
export function dragDelta(pixels, { fine = false, travel = DRAG_PIXELS } = {}) {
  return (pixels / (travel > 0 ? travel : DRAG_PIXELS)) * (fine ? FINE : 1);
}

/**
 * The position under a pointer at (`dx`, `dy`) from a knob's centre (screen
 * coordinates, y down), for a `sweep` in degrees centred on 12 o'clock.
 * Angles beyond the sweep clamp to its ends.
 */
export function circularPosition(dx, dy, sweep = 270) {
  const degrees = (Math.atan2(dx, -dy) * 180) / Math.PI;
  return clamp((degrees + sweep / 2) / sweep, 0, 1);
}

/**
 * A knob's position after the pointer of a circular drag moved from `from`
 * to `to` (both `circularPosition`s). Normally the knob follows the pointer;
 * with `fine` it moves a tenth of the pointer's movement, so the result
 * depends only on where the pointer went, not on how many events reported
 * it, and a pointer that did not move changes nothing.
 */
export function circularDrag(position, from, to, { fine = false } = {}) {
  return fine ? clamp(position + (to - from) * FINE, 0, 1) : to;
}

/** The angle in degrees (0 at 12 o'clock, clockwise) of a knob's position. */
export function knobAngle(position, sweep = 270) {
  return -sweep / 2 + clamp(position, 0, 1) * sweep;
}

/**
 * The position change for a wheel event: one notch (100 pixels, or 3
 * lines) is one arrow step; up is positive.
 */
export function wheelDelta(deltaY, { fine = false, deltaMode = 0 } = {}) {
  const notches = deltaMode === 1 ? deltaY / 3 : deltaMode === 2 ? deltaY * 10 : deltaY / 100;
  return -notches * KEY_STEP * (fine ? FINE : 1);
}

/**
 * A key's new position, or null when the key does nothing: arrows ±1 %,
 * Page Up/Down ±10 % (both ×0.1 with `fine`), Home 0, End 1.
 */
export function keyAction(key, position, { fine = false } = {}) {
  const scale = fine ? FINE : 1;
  const step = {
    ArrowUp: KEY_STEP,
    ArrowRight: KEY_STEP,
    ArrowDown: -KEY_STEP,
    ArrowLeft: -KEY_STEP,
    PageUp: PAGE_STEP,
    PageDown: -PAGE_STEP,
  }[key];
  if (step !== undefined) return clamp(position + step * scale, 0, 1);
  if (key === 'Home') return 0;
  if (key === 'End') return 1;
  return null;
}

/** The value a reset (double-click, Delete) goes to: the default, else min. */
export function resetValue({ min, max, default: initial }) {
  return clamp(Number.isFinite(initial) ? initial : min, min, max);
}
