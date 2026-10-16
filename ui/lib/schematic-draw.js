// How the schematic hands a level to SVG (M2.9-1b, ADR 0026 §2). Each drawn
// level is one group whose `matrix` is all a pan or a zoom changes; its
// elements are built once and kept, by key, until what is visible changes
// (culling, levels opened or closed, a new layout, the text sizes, or the
// bake below). Browsers keep SVG transforms in float32, so a level's
// geometry is baked against a content origin `q` (the point of the level
// under the screen's centre, as near as its coordinates can say) and a
// power of ten `m`: elements carry `(p − q) · m + shift`, where `shift` is
// how far `q` lands from the centre (zero but where the level's coordinates
// are coarser than a pixel there), and the matrix is `k / m` about the
// centre, so every matrix has a scale within [1e-6, 1e6] and a translation
// within ±1e7 at any zoom. Geometry more than CLIP_PX from the centre on
// screen at the bake is clipped (never what is visible); a pin wider than
// HUGE_PIN_PX on screen is drawn as the part of its disc near the screen.
// Text is drawn whenever it is at least 4 px on screen, as it always was. A
// level whose coordinates cannot place anything within a pixel of the
// screen (`representable` false: past the camera's precision) is not
// drawn. No DOM.

import { toLocal, toScreen } from './schematic-view.js';

/** The bounds of every matrix handed to SVG. */
export const SVG_MIN_SCALE = 1e-6;
export const SVG_MAX_SCALE = 1e6;
export const SVG_MAX_TRANSLATE = 1e7;
/** A level is baked again when `q`'s screen point would leave this (px). */
export const REBAKE_PX = 1e6;
/** Geometry farther than this from `q` on screen (px) is clipped. */
export const CLIP_PX = 1e7;
/** A pin wider than this on screen (radius, px) is drawn as a clipped polygon. */
export const HUGE_PIN_PX = 1e7;
/** Its arc's chords bow in by at most this (px), half the 0.01 px tolerance. */
const SAGITTA_PX = 0.005;
/** Strokes reach this far (px) past a clipped edge, which stays off screen. */
const STROKE_PX = 16;

/** The power of ten `m` that brings scale `k` within the SVG bounds as `k / m`. */
export function bakeScale(k) {
  let m = 1;
  if (k > SVG_MAX_SCALE) m = 10 ** Math.ceil(Math.log10(k / SVG_MAX_SCALE));
  else if (k < SVG_MIN_SCALE) m = 10 ** (Math.floor(Math.log10(k)) + 6);
  while (k / m > SVG_MAX_SCALE) m *= 10;
  while (k / m < SVG_MIN_SCALE) m /= 10;
  return m;
}

/** Past this (px) from the centre, a level's nearest point is not placed within a pixel (float64). */
const PLACEABLE_PX = 1e15;

/**
 * A level's bake for `transform` (a scene entry's) in `viewport`: `{q, m,
 * shift, k0, clip, representable}` (`clip` in baked coordinates).
 */
export function bake(transform, viewport) {
  const centre = { x: viewport.w / 2, y: viewport.h / 2 };
  const q = toLocal(transform, centre);
  const m = bakeScale(transform.k);
  // Where `q` really lands: off the centre when the level's coordinates are
  // coarser than a pixel there; the difference goes into the coordinates.
  const at = toScreen(transform, q);
  const scale = transform.k / m;
  const shift = { x: (at.x - centre.x) / scale, y: (at.y - centre.y) / scale };
  const half = (CLIP_PX * m) / transform.k;
  const representable = Math.abs(at.x - centre.x) <= PLACEABLE_PX && Math.abs(at.y - centre.y) <= PLACEABLE_PX;
  return { q, m, shift, k0: transform.k, clip: { x0: -half, y0: -half, x1: half, y1: half }, clipped: false, cover: null, representable };
}

/** The SVG matrix of a level baked as `baked`, drawn with `transform`: `{k, x, y}`. */
export function bakedMatrix(baked, transform) {
  const at = toScreen(transform, baked.q);
  const k = transform.k / baked.m;
  return { k, x: at.x - baked.shift.x * k, y: at.y - baked.shift.y * k };
}

/**
 * Whether `baked` still serves `transform` in `viewport`: the same `m`, its
 * translation within REBAKE_PX, what it clipped still off screen, and every
 * huge pin's polygon still covering the screen within its tolerance.
 */
export function bakeHolds(baked, transform, viewport) {
  if (bakeScale(transform.k) !== baked.m) return false;
  const matrix = bakedMatrix(baked, transform);
  const off = Math.max(Math.abs(matrix.x), Math.abs(matrix.y));
  if (off > REBAKE_PX) return false;
  // Zoomed out far enough, a clipped edge would come on screen.
  const reach = (CLIP_PX * transform.k) / baked.k0;
  if (baked.clipped && reach < off + Math.max(viewport.w, viewport.h) + STROKE_PX) return false;
  if (!baked.cover) return true;
  if (transform.k > 2 * baked.cover.k) return false;
  const a = unbaked(matrix, { x: -STROKE_PX, y: -STROKE_PX });
  const b = unbaked(matrix, { x: viewport.w + STROKE_PX, y: viewport.h + STROKE_PX });
  const r = baked.cover.rect;
  return a.x >= r.x0 && a.y >= r.y0 && b.x <= r.x1 && b.y <= r.y1;
}

/** Whether `matrix` is within the bounds SVG is handed (scale and translation). */
export function inBounds(matrix) {
  return (
    matrix.k >= SVG_MIN_SCALE &&
    matrix.k <= SVG_MAX_SCALE &&
    Math.abs(matrix.x) <= SVG_MAX_TRANSLATE &&
    Math.abs(matrix.y) <= SVG_MAX_TRANSLATE
  );
}

/** A screen point in the coordinates of a level drawn with `matrix`. */
function unbaked(matrix, p) {
  return { x: (p.x - matrix.x) / matrix.k, y: (p.y - matrix.y) / matrix.k };
}

/** A level point as baked. */
export function bakedPoint(baked, p) {
  return { x: (p.x - baked.q.x) * baked.m + baked.shift.x, y: (p.y - baked.q.y) * baked.m + baked.shift.y };
}

/**
 * A rectangle's `{x, y, width, height}` as baked and clipped, or null if
 * nothing of it is left.
 */
export function bakedRect(baked, r) {
  const { clip } = baked;
  const a = bakedPoint(baked, r);
  const b = bakedPoint(baked, { x: r.x + r.w, y: r.y + r.h });
  const x0 = Math.max(a.x, clip.x0);
  const y0 = Math.max(a.y, clip.y0);
  const x1 = Math.min(b.x, clip.x1);
  const y1 = Math.min(b.y, clip.y1);
  if (x1 <= x0 || y1 <= y0) return null;
  if (x0 !== a.x || y0 !== a.y || x1 !== b.x || y1 !== b.y) baked.clipped = true;
  return { x: x0, y: y0, width: x1 - x0, height: y1 - y0 };
}

/** A segment `a`–`b` clipped to the box (Liang–Barsky), or null. */
function clipSegment(a, b, box) {
  let t0 = 0;
  let t1 = 1;
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  for (const [p, q] of [[-dx, a.x - box.x0], [dx, box.x1 - a.x], [-dy, a.y - box.y0], [dy, box.y1 - a.y]]) {
    if (p === 0) {
      if (q < 0) return null;
    } else {
      const t = q / p;
      if (p < 0) t0 = Math.max(t0, t);
      else t1 = Math.min(t1, t);
    }
  }
  if (t0 > t1) return null;
  const at = (t) => (t === 0 ? a : t === 1 ? b : { x: a.x + t * dx, y: a.y + t * dy });
  return [at(t0), at(t1)];
}

/** A wire's polyline as baked and clipped: a path's `d` (empty if none is left). */
export function bakedPath(baked, points) {
  let d = '';
  let last = null;
  const at = points.map((p) => bakedPoint(baked, p));
  for (let i = 1; i < at.length; i++) {
    const segment = clipSegment(at[i - 1], at[i], baked.clip);
    if (segment?.[0] !== at[i - 1] || segment[1] !== at[i]) baked.clipped = true;
    if (!segment) {
      last = null;
      continue;
    }
    const [a, b] = segment;
    if (last !== a) d += `${d ? ' ' : ''}M${a.x},${a.y}`;
    d += ` L${b.x},${b.y}`;
    last = b;
  }
  return d;
}

/** The polygon `poly` (`[{x, y}]`) clipped to the box (Sutherland–Hodgman). */
function clipPolygon(poly, box) {
  const edges = [
    [(p) => p.x >= box.x0, (a, b) => ({ x: box.x0, y: a.y + ((box.x0 - a.x) * (b.y - a.y)) / (b.x - a.x) })],
    [(p) => p.x <= box.x1, (a, b) => ({ x: box.x1, y: a.y + ((box.x1 - a.x) * (b.y - a.y)) / (b.x - a.x) })],
    [(p) => p.y >= box.y0, (a, b) => ({ x: a.x + ((box.y0 - a.y) * (b.x - a.x)) / (b.y - a.y), y: box.y0 })],
    [(p) => p.y <= box.y1, (a, b) => ({ x: a.x + ((box.y1 - a.y) * (b.x - a.x)) / (b.y - a.y), y: box.y1 })],
  ];
  let out = poly;
  for (const [inside, cross] of edges) {
    const input = out;
    out = [];
    input.forEach((b, i) => {
      const a = input[(i + input.length - 1) % input.length];
      if (inside(b)) {
        if (!inside(a)) out.push(cross(a, b));
        out.push(b);
      } else if (inside(a)) {
        out.push(cross(a, b));
      }
    });
    if (!out.length) break;
  }
  return out;
}

/**
 * The part of the disc at `c` with radius `radius` (screen px) inside
 * `box`, as a polygon whose arc's chords bow in by at most SAGITTA_PX; [] if
 * none. Only the arc that faces the box is sampled.
 */
export function discInBox(c, radius, box) {
  const corners = [{ x: box.x0, y: box.y0 }, { x: box.x1, y: box.y0 }, { x: box.x1, y: box.y1 }, { x: box.x0, y: box.y1 }];
  if (corners.every((p) => Math.hypot(p.x - c.x, p.y - c.y) <= radius)) return corners;
  const within = c.x >= box.x0 && c.x <= box.x1 && c.y >= box.y0 && c.y <= box.y1;
  const base = Math.atan2((box.y0 + box.y1) / 2 - c.y, (box.x0 + box.x1) / 2 - c.x);
  let lo = -Math.PI;
  let hi = Math.PI;
  if (!within) {
    const angles = corners.map((p) => {
      let a = Math.atan2(p.y - c.y, p.x - c.x) - base;
      if (a > Math.PI) a -= 2 * Math.PI;
      if (a < -Math.PI) a += 2 * Math.PI;
      return a;
    });
    lo = Math.min(...angles);
    hi = Math.max(...angles);
  }
  const chord = Math.sqrt(8 * SAGITTA_PX * radius);
  const n = Math.max(1, Math.ceil((radius * (hi - lo)) / chord));
  const arc = [];
  for (let i = 0; i <= n; i++) {
    const a = base + lo + ((hi - lo) * i) / n;
    arc.push({ x: c.x + radius * Math.cos(a), y: c.y + radius * Math.sin(a) });
  }
  return clipPolygon(within ? arc : [c, ...arc], box);
}

/**
 * A pin (`{x, y}`, radius `r` in level units) as baked: `{circle: {cx, cy,
 * r}}`, or past HUGE_PIN_PX on screen `{polygon}` (points as baked), the
 * disc near the screen; `baked.cover` then records what it covers.
 */
export function bakedPin(baked, transform, viewport, pin, r = 3) {
  const radius = r * transform.k;
  if (radius <= HUGE_PIN_PX) {
    const at = bakedPoint(baked, pin);
    return { circle: { cx: at.x, cy: at.y, r: r * baked.m } };
  }
  // The viewport and two viewports around it: panning that far needs no new bake.
  const margin = 2 * Math.max(viewport.w, viewport.h);
  const box = { x0: -margin, y0: -margin, x1: viewport.w + margin, y1: viewport.h + margin };
  const polygon = discInBox(toScreen(transform, pin), radius, box);
  const matrix = bakedMatrix(baked, transform);
  const a = unbaked(matrix, { x: box.x0, y: box.y0 });
  const b = unbaked(matrix, { x: box.x1, y: box.y1 });
  baked.cover = { k: transform.k, rect: { x0: a.x, y0: a.y, x1: b.x, y1: b.y } };
  return { polygon: polygon.map((p) => unbaked(matrix, p)) };
}

/** Whether two lists hold the same objects in the same order. */
export function sameItems(a, b) {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

/**
 * Elements for `items`, by key: an item whose object and `flags` are what
 * the cached element was built for keeps it; the others are built. Returns
 * `{cache, elements, created, removed}` (`cache` for the next call).
 */
export function reconcile(cache, items, keyOf, flagsOf, build) {
  const next = new Map();
  const elements = [];
  let created = 0;
  for (const item of items) {
    const key = keyOf(item);
    const flags = flagsOf(item);
    let entry = cache.get(key);
    if (!entry || entry.item !== item || entry.flags !== flags) {
      entry = { item, flags, el: build(item) };
      created++;
    }
    next.set(key, entry);
    elements.push(entry.el);
  }
  let removed = 0;
  for (const key of cache.keys()) if (!next.has(key)) removed++;
  return { cache: next, elements, created, removed };
}
