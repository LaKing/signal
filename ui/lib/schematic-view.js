// The schematic's view transform (M2.9-1, ADR 0026 §2): one uniform scale
// `k` and a translation (`x`, `y`) from the top level's coordinates to the
// screen's pixels, `screen = world · k + (x, y)`. Each level drawn inside a
// composite's box gets its own matrix composed from it in float64, so the
// numbers handed to SVG stay small however deep the view goes. The zoom is
// clamped only to keep every number finite. No DOM.

/** The scale's limits: only there to keep numbers finite. */
export const MIN_SCALE = 1e-150;
export const MAX_SCALE = 1e150;

export const IDENTITY = Object.freeze({ k: 1, x: 0, y: 0 });

function finite(view) {
  return Number.isFinite(view.k) && Number.isFinite(view.x) && Number.isFinite(view.y) && view.k > 0;
}

/** `k` within the limits (a non-finite one is 1). */
export function clampScale(k) {
  if (!Number.isFinite(k) || k <= 0) return 1;
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, k));
}

/** A point of the transform's own coordinates on the screen. */
export function toScreen(view, point) {
  return { x: point.x * view.k + view.x, y: point.y * view.k + view.y };
}

/** A screen point in the transform's own coordinates. */
export function toLocal(view, point) {
  return { x: (point.x - view.x) / view.k, y: (point.y - view.y) / view.k };
}

/** Zoomed by `factor` about the screen point `at`, which stays where it is. */
export function zoomAt(view, factor, at) {
  const k = clampScale(view.k * factor);
  const applied = k / view.k;
  const next = { k, x: at.x - (at.x - view.x) * applied, y: at.y - (at.y - view.y) * applied };
  return finite(next) ? next : view;
}

/** Moved by `dx`, `dy` screen pixels. */
export function pan(view, dx, dy) {
  const next = { k: view.k, x: view.x + dx, y: view.y + dy };
  return finite(next) ? next : view;
}

/**
 * The view that shows `rect` (`{x, y, w, h}` in the transform's own
 * coordinates) whole, centred, in a `viewport` (`{w, h}`) less `margin`.
 */
export function fit(rect, viewport, margin = 24) {
  const w = Math.max(1, viewport.w - 2 * margin);
  const h = Math.max(1, viewport.h - 2 * margin);
  const k = clampScale(Math.min(w / Math.max(rect.w, 1e-9), h / Math.max(rect.h, 1e-9)));
  return {
    k,
    x: viewport.w / 2 - (rect.x + rect.w / 2) * k,
    y: viewport.h / 2 - (rect.y + rect.h / 2) * k,
  };
}

/**
 * `view` zoomed and moved so that the screen rectangle `rect` (a part as
 * it is drawn now) fills the viewport: a breadcrumb, Escape, a double-click.
 */
export function focus(view, rect, viewport, margin = 24) {
  const target = fit(rect, viewport, margin);
  // `rect` is in screen pixels: compose `target` (screen → screen) with view.
  const next = { k: clampScale(view.k * target.k), x: view.x * target.k + target.x, y: view.y * target.k + target.y };
  return finite(next) ? next : view;
}

/**
 * The transform of a level drawn inside a box of its parent: `inner`
 * (child coordinates → parent coordinates) followed by `view` (parent →
 * screen).
 */
export function compose(view, inner) {
  return { k: view.k * inner.k, x: view.x + view.k * inner.x, y: view.y + view.k * inner.y };
}

/** A screen rectangle of `rect` in the transform's own coordinates. */
export function screenRect(view, rect) {
  return { x: rect.x * view.k + view.x, y: rect.y * view.k + view.y, w: rect.w * view.k, h: rect.h * view.k };
}

/** The zoom factor of a wheel step (`deltaMode` 1: lines, 2: pages); a pinch (ctrl+wheel) is finer. */
export function wheelFactor(deltaY, { deltaMode = 0, pinch = false } = {}) {
  const pixels = deltaY * (deltaMode === 1 ? 16 : deltaMode === 2 ? 400 : 1);
  return Math.exp(-pixels * (pinch ? 0.01 : 0.002));
}

/** The keys' step: + and − zoom by this about the centre, arrows pan a tenth of the viewport. */
export const KEY_ZOOM = 1.25;

/**
 * The view after key `key` (`+`, `=`, `-`, `−`, `0`, the arrows) in a
 * `viewport`; `bounds` is what `0` fits. Null for another key.
 */
export function keyView(view, key, viewport, bounds) {
  const centre = { x: viewport.w / 2, y: viewport.h / 2 };
  switch (key) {
    case '+':
    case '=':
      return zoomAt(view, KEY_ZOOM, centre);
    case '-':
    case '−':
    case '_':
      return zoomAt(view, 1 / KEY_ZOOM, centre);
    case '0':
      return fit(bounds, viewport);
    case 'ArrowLeft':
      return pan(view, viewport.w / 10, 0);
    case 'ArrowRight':
      return pan(view, -viewport.w / 10, 0);
    case 'ArrowUp':
      return pan(view, 0, viewport.h / 10);
    case 'ArrowDown':
      return pan(view, 0, -viewport.h / 10);
    default:
      return null;
  }
}

/** The SVG `transform` of a view. */
export function matrix(view) {
  return `matrix(${view.k} 0 0 ${view.k} ${view.x} ${view.y})`;
}
