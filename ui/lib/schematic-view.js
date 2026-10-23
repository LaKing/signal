// The schematic's view transforms (M2.9-1, ADR 0026 §2): one uniform scale
// `k` from a level's coordinates to the screen's pixels about an origin `o`
// in those coordinates, which is drawn at the screen point `(x, y)`:
// `screen = (world − o) · k + (x, y)` (`ox`, `oy`; 0 when absent). The view
// is kept in the coordinates of the deepest level drawn under the screen's
// centre (its `path`, `ui/lib/schematic-scene.js`), and the origin moves to
// the pointer on every zoom, so the point under it stays where it is to
// within a pixel up to about 1e13 px per layout unit inside one level
// (float64; measured against exact arithmetic), and the zoom goes on through
// nested composites without limit. Levels above and below are derived from
// it by error-free links (`compose`, `uncompose`). Numbers handed to SVG are
// bounded separately (`ui/lib/schematic-draw.js`). The scale is clamped only
// to stay finite. No DOM.

/** The scale's limits: only there to keep numbers finite. */
export const MIN_SCALE = 1e-300;
export const MAX_SCALE = 1e300;

export const IDENTITY = Object.freeze({ k: 1, x: 0, y: 0 });

const ox = (view) => view.ox ?? 0;
const oy = (view) => view.oy ?? 0;

function finite(view) {
  return [view.k, view.x, view.y, ox(view), oy(view)].every(Number.isFinite) && view.k > 0;
}

/** `k` within the limits (a non-finite one is 1). */
export function clampScale(k) {
  if (!Number.isFinite(k) || k <= 0) return 1;
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, k));
}

/** A point of the transform's own coordinates on the screen. */
export function toScreen(view, point) {
  return { x: (point.x - ox(view)) * view.k + view.x, y: (point.y - oy(view)) * view.k + view.y };
}

/** A screen point in the transform's own coordinates. */
export function toLocal(view, point) {
  return { x: ox(view) + (point.x - view.x) / view.k, y: oy(view) + (point.y - view.y) / view.k };
}

/** Zoomed by `factor` about the screen point `at`, which stays where it is. */
export function zoomAt(view, factor, at) {
  const k = clampScale(view.k * factor);
  const applied = k / view.k;
  // The origin moves to the point under the pointer; `o' − o` is exact, so
  // the screen point of the new origin carries what rounding left over.
  const px = ox(view) + (at.x - view.x) / view.k;
  const py = oy(view) + (at.y - view.y) / view.k;
  const sx = view.x + (px - ox(view)) * view.k;
  const sy = view.y + (py - oy(view)) * view.k;
  const next = { ...view, k, ox: px, oy: py, x: at.x - (at.x - sx) * applied, y: at.y - (at.y - sy) * applied };
  return finite(next) ? next : view;
}

/** Moved by `dx`, `dy` screen pixels. */
export function pan(view, dx, dy) {
  const next = { ...view, x: view.x + dx, y: view.y + dy };
  return finite(next) ? next : view;
}

/**
 * The view that shows `rect` (`{x, y, w, h}` in the transform's own
 * coordinates) whole, centred, in a `viewport` (`{w, h}`) less `margin`
 * (and `top` at its top edge instead, where a bar lies over it; `side` at
 * its left and right edges, where labels go).
 */
export function fit(rect, viewport, margin = 24, top = margin, side = margin) {
  const w = Math.max(1, viewport.w - 2 * side);
  const h = Math.max(1, viewport.h - margin - top);
  const k = clampScale(Math.min(w / Math.max(rect.w, 1e-9), h / Math.max(rect.h, 1e-9)));
  return { k, ox: rect.x + rect.w / 2, oy: rect.y + rect.h / 2, x: viewport.w / 2, y: top + h / 2 };
}

/**
 * `view` zoomed and moved so that the screen rectangle `rect` (a part as
 * it is drawn now) fills the viewport (`top` as `fit`'s): a breadcrumb,
 * Escape, a double-click.
 */
export function focus(view, rect, viewport, margin = 24, top = margin, side = margin) {
  const target = fit(rect, viewport, margin, top, side);
  // `rect` is in screen pixels: compose `target` (screen → screen) with view.
  const next = {
    ...view,
    k: clampScale(view.k * target.k),
    x: (view.x - target.ox) * target.k + target.x,
    y: (view.y - target.oy) * target.k + target.y,
  };
  return finite(next) ? next : view;
}

// Error-free sums and products (Knuth, Dekker): `a + b` and `a · b` as a
// float and its exact rounding error.
function twoSum(a, b) {
  const s = a + b;
  const v = s - a;
  return [s, a - (s - v) + (b - v)];
}

function split(a) {
  const c = 134217729 * a;
  const hi = c - (c - a);
  return [hi, a - hi];
}

function twoProduct(a, b) {
  const p = a * b;
  const [ah, al] = split(a);
  const [bh, bl] = split(b);
  return [p, ah * bh - p + ah * bl + al * bh + al * bl];
}

/** `a · b + c − d`, with no rounding but the last. */
function residual(a, b, c, d) {
  const [p, pe] = twoProduct(a, b);
  const [s, se] = twoSum(p, c);
  const [r, re] = twoSum(s, -d);
  return r + (re + se + pe);
}

/**
 * The transform of a level drawn inside a box of its parent: `inner`
 * (child coordinates → parent coordinates, `{k, x, y}`) followed by `view`
 * (parent → screen). The child's origin is the parent's, and the link is
 * computed without rounding, so levels on either side agree at any scale.
 */
export function compose(view, inner) {
  const cx = (ox(view) - inner.x) / inner.k;
  const cy = (oy(view) - inner.y) / inner.k;
  return {
    k: view.k * inner.k,
    ox: cx,
    oy: cy,
    x: view.x + residual(inner.k, cx, inner.x, ox(view)) * view.k,
    y: view.y + residual(inner.k, cy, inner.y, oy(view)) * view.k,
  };
}

/** The parent's transform from a child's (`compose` undone). */
export function uncompose(child, inner) {
  const k = child.k / inner.k;
  const px = inner.k * ox(child) + inner.x;
  const py = inner.k * oy(child) + inner.y;
  return {
    k,
    ox: px,
    oy: py,
    x: child.x - residual(inner.k, ox(child), inner.x, px) * k,
    y: child.y - residual(inner.k, oy(child), inner.y, py) * k,
  };
}

/** A screen rectangle of `rect` in the transform's own coordinates. */
export function screenRect(view, rect) {
  const at = toScreen(view, rect);
  return { x: at.x, y: at.y, w: rect.w * view.k, h: rect.h * view.k };
}

/** The zoom factor of a wheel step (`deltaMode` 1: lines, 2: pages); a pinch (ctrl+wheel) is finer. */
export function wheelFactor(deltaY, { deltaMode = 0, pinch = false } = {}) {
  const pixels = deltaY * (deltaMode === 1 ? 16 : deltaMode === 2 ? 400 : 1);
  return Math.exp(-pixels * (pinch ? 0.01 : 0.002));
}

/** The keys' step: zooming in and out from the keyboard is by this about the centre; the arrows pan a tenth of the viewport. */
export const KEY_ZOOM = 1.25;

/** A page of panning (Page Up, Page Down): this share of the viewport's height. */
export const PAGE_SHARE = 0.875;

/**
 * The view after the schematic's view command `id` (ui/lib/keys.default.js,
 * scope `schematic`: `schematic.zoom_in`, `zoom_out`, `fit`, `pan_*`,
 * `page_up`, `page_down`) in a `viewport`; `bounds` is what `fit` fits.
 * Null for another command.
 */
export function commandView(view, id, viewport, bounds) {
  const centre = { x: viewport.w / 2, y: viewport.h / 2 };
  switch (id) {
    case 'schematic.zoom_in':
      return zoomAt(view, KEY_ZOOM, centre);
    case 'schematic.zoom_out':
      return zoomAt(view, 1 / KEY_ZOOM, centre);
    case 'schematic.fit':
      return fit(bounds, viewport);
    case 'schematic.pan_left':
      return pan(view, viewport.w / 10, 0);
    case 'schematic.pan_right':
      return pan(view, -viewport.w / 10, 0);
    case 'schematic.pan_up':
      return pan(view, 0, viewport.h / 10);
    case 'schematic.pan_down':
      return pan(view, 0, -viewport.h / 10);
    case 'schematic.page_up':
      return pan(view, 0, viewport.h * PAGE_SHARE);
    case 'schematic.page_down':
      return pan(view, 0, -viewport.h * PAGE_SHARE);
    default:
      return null;
  }
}

/** The SVG `transform` of a matrix `{k, x, y}` (no origin). */
export function matrix(view) {
  return `matrix(${view.k} 0 0 ${view.k} ${view.x} ${view.y})`;
}
