// The schematic's wire geometry (M2.9-1, ADR 0026 §3; the owner's "smooth
// cable flow", 2026-10-16): patch cords, not right angles. A forward wire is
// one cubic Bézier that leaves its output to the right and enters its input
// from the left, both horizontally; a feedback wire loops out to the right,
// down to its lane under the boxes, along it and back up into its input
// from the left, its turns rounded. A wire is its `curve` (`{start, segs}`,
// each segment a line `{to}` or a cubic `{c1, c2, to}`, in the level's
// units), drawn exactly (`C`, ui/lib/schematic-draw.js `bakedPath`) and hit
// exactly (`curveDistance`) at any zoom, its `points`, the curve flattened
// (FLAT), and its `box`, the curve's exact bounding box (a short forward
// wire swings out past its pins, so the knots do not bound it), for what
// only needs its extent (bounds, culling). Pure and deterministic; no DOM.

/** A forward wire's handles reach at least this far out (level units)… */
export const REACH_MIN = 40;
/** …and up to this far for its height alone… */
export const REACH_TALL = 120;
/** …and a feedback wire goes this far out past its pins before it turns. */
export const LOOP_OUT = 20;
/** A feedback wire's turns, as circles of at most this radius. */
export const LOOP_RADIUS = 16;
/**
 * `points` stay within this of the curve (level units), in at most
 * MAX_PIECES pieces per cubic: a longer one's are coarser, which nothing
 * drawn or hit depends on.
 */
export const FLAT = 1;
/** A cubic is flattened into at most this many segments. */
const MAX_PIECES = 16;

const at = (p) => ({ x: p.x, y: p.y });

/**
 * The curve of a wire from output `a` to input `b`: forward, one cubic;
 * `feedback`, the loop around below through lane `under` (a y below both
 * pins). A forward wire with `avoid` (an open composite's frame, M2.13-3:
 * `{rect: {x, y, w, h}, lanes: {left, right, top, bottom}}`) that would
 * pass under it goes around it instead (`detour`).
 */
export function wireCurve(a, b, { feedback = false, under = 0, avoid = null } = {}) {
  if (!feedback) {
    // Half the run, half the height (to REACH_TALL), never under REACH_MIN:
    // two boxes close together are joined by an S that swings out past
    // them, a patch cord's, not a line down their edge.
    const reach = Math.max(REACH_MIN, (b.x - a.x) / 2, Math.min(Math.abs(b.y - a.y) / 2, REACH_TALL));
    const direct = { start: at(a), segs: [{ c1: { x: a.x + reach, y: a.y }, c2: { x: b.x - reach, y: b.y }, to: at(b) }] };
    return avoid && crosses(direct, avoid.rect) ? (detour(a, b, avoid) ?? direct) : direct;
  }
  let right = a.x + LOOP_OUT;
  let left = b.x - LOOP_OUT;
  // Its way down and its way up at least a turn's width apart, the run
  // along the lane keeping its direction: never a U-turn on the spot.
  if (Math.abs(left - right) < 2 * LOOP_RADIUS) {
    const mid = (left + right) / 2;
    const side = left > right ? 1 : -1;
    left = mid + side * LOOP_RADIUS;
    right = mid - side * LOOP_RADIUS;
  }
  return rounded([at(a), { x: right, y: a.y }, { x: right, y: under }, { x: left, y: under }, { x: left, y: b.y }, at(b)], LOOP_RADIUS);
}

/** Whether segment `p`–`q` passes through the inside of `rect` (`{x, y, w, h}`; its edge is not inside). Liang–Barsky. */
function segmentCrosses(p, q, rect) {
  let t0 = 0;
  let t1 = 1;
  const dx = q.x - p.x;
  const dy = q.y - p.y;
  const edges = [
    [-dx, p.x - rect.x],
    [dx, rect.x + rect.w - p.x],
    [-dy, p.y - rect.y],
    [dy, rect.y + rect.h - p.y],
  ];
  for (const [d, room] of edges) {
    if (d === 0) {
      if (room <= 0) return false;
      continue;
    }
    const t = room / d;
    if (d < 0) t0 = Math.max(t0, t);
    else t1 = Math.min(t1, t);
    if (t0 >= t1) return false;
  }
  return true;
}

/** Whether `curve` (flattened) passes through the inside of `rect`. */
export function crosses(curve, rect) {
  const ps = flatten(curve);
  for (let i = 1; i < ps.length; i++) if (segmentCrosses(ps[i - 1], ps[i], rect)) return true;
  return false;
}

/** The length of polyline `ps`. */
function lengthOf(ps) {
  let l = 0;
  for (let i = 1; i < ps.length; i++) l += Math.hypot(ps[i].x - ps[i - 1].x, ps[i].y - ps[i - 1].y);
  return l;
}

/**
 * A forward wire from `a` to `b` around `avoid.rect` (2026-10-22, the
 * operator's review: the drums' wires ran under an open voice's frame
 * corner): along one of its lanes (`avoid.lanes`, outside it: a column
 * left or right of it, a row above or below), entered from `a` and left
 * for `b` by cubics that leave an output and enter an input horizontally
 * as every wire does. Of the ways that stay out of the rect, the shortest
 * (ties: left, right, top, bottom); null when none does.
 */
export function detour(a, b, avoid) {
  const { rect, lanes } = avoid;
  const clamp = (v, lo, hi) => Math.min(Math.max(v, lo), hi);
  const ways = [
    ...[lanes.left, lanes.right].map((x) => [
      { x, y: clamp(a.y, lanes.top, lanes.bottom) },
      { x, y: clamp(b.y, lanes.top, lanes.bottom) },
    ]),
    ...[lanes.top, lanes.bottom].map((y) => [
      { x: clamp(a.x, lanes.left, lanes.right), y },
      { x: clamp(b.x, lanes.left, lanes.right), y },
    ]),
  ];
  let best = null;
  for (const [p, q] of ways) {
    const [d, l] = unit(p, q);
    // Both ends on the lane at one point: no way along it.
    if (!(l > 0)) continue;
    const segs = [];
    const ra = Math.min(REACH_TALL, Math.hypot(p.x - a.x, p.y - a.y) / 2);
    if (ra > 0) segs.push({ c1: { x: a.x + ra, y: a.y }, c2: { x: p.x - d.x * ra, y: p.y - d.y * ra }, to: at(p) });
    segs.push({ to: at(q) });
    const rb = Math.min(REACH_TALL, Math.hypot(b.x - q.x, b.y - q.y) / 2);
    if (rb > 0) segs.push({ c1: { x: q.x + d.x * rb, y: q.y + d.y * rb }, c2: { x: b.x - rb, y: b.y }, to: at(b) });
    const curve = { start: at(a), segs };
    if (crosses(curve, rect)) continue;
    const length = lengthOf(flatten(curve));
    if (!best || length < best.length) best = { curve, length };
  }
  return best?.curve ?? null;
}

/**
 * Polyline `corners` with each turn rounded: a circular arc (as a cubic) of
 * at most `radius`, within its two legs (an end leg whole, a middle leg
 * shared half and half with the next turn).
 */
export function rounded(corners, radius) {
  const p = corners.filter((c, i) => i === 0 || c.x !== corners[i - 1].x || c.y !== corners[i - 1].y);
  const segs = [];
  let from = p[0];
  for (let i = 1; i < p.length - 1; i++) {
    const [u, lu] = unit(p[i - 1], p[i]);
    const [w, lw] = unit(p[i], p[i + 1]);
    const cos = u.x * w.x + u.y * w.y;
    const r = Math.min(radius, lu * (i === 1 ? 1 : 0.5), lw * (i === p.length - 2 ? 1 : 0.5));
    // Straight on, or straight back: no arc fits.
    if (cos > 1 - 1e-12 || cos < -1 + 1e-12 || !(r > 0)) {
      segs.push({ to: p[i] });
      from = p[i];
      continue;
    }
    const s = { x: p[i].x - u.x * r, y: p[i].y - u.y * r };
    const e = { x: p[i].x + w.x * r, y: p[i].y + w.y * r };
    if (s.x !== from.x || s.y !== from.y) segs.push({ to: s });
    // A turn by θ: an arc of radius r / tan(θ/2), its handles (4/3)·tan(θ/4) of it.
    const turn = Math.acos(cos);
    const handle = ((4 / 3) * Math.tan(turn / 4) * r) / Math.tan(turn / 2);
    segs.push({ c1: { x: s.x + u.x * handle, y: s.y + u.y * handle }, c2: { x: e.x - w.x * handle, y: e.y - w.y * handle }, to: e });
    from = e;
  }
  const end = p.at(-1);
  if (p.length > 1 && (from.x !== end.x || from.y !== end.y)) segs.push({ to: end });
  return { start: p[0], segs };
}

function unit(a, b) {
  const l = Math.hypot(b.x - a.x, b.y - a.y);
  return [{ x: (b.x - a.x) / l, y: (b.y - a.y) / l }, l];
}

/** The point at `t` of the cubic `p0 c1 c2 p3`. */
function cubicAt(p0, c1, c2, p3, t) {
  const s = 1 - t;
  const a = s * s * s;
  const b = 3 * s * s * t;
  const c = 3 * s * t * t;
  const d = t * t * t;
  return { x: a * p0.x + b * c1.x + c * c2.x + d * p3.x, y: a * p0.y + b * c1.y + c * c2.y + d * p3.y };
}

/** `curve` as a polyline within `tolerance` of it (Wang's bound), its knots exact. */
export function flatten(curve, tolerance = FLAT) {
  const points = [curve.start];
  let from = curve.start;
  for (const seg of curve.segs) {
    if (seg.c1) {
      const dd = Math.max(
        Math.hypot(from.x - 2 * seg.c1.x + seg.c2.x, from.y - 2 * seg.c1.y + seg.c2.y),
        Math.hypot(seg.c1.x - 2 * seg.c2.x + seg.to.x, seg.c1.y - 2 * seg.c2.y + seg.to.y),
      );
      const n = Math.min(MAX_PIECES, Math.max(1, Math.ceil(Math.sqrt((0.75 * dd) / tolerance))));
      for (let i = 1; i < n; i++) points.push(cubicAt(from, seg.c1, seg.c2, seg.to, i / n));
    }
    points.push(seg.to);
    from = seg.to;
  }
  return points;
}

/** A wire's geometry: `{curve, points, box}` (see `wireCurve`). */
export function wireGeometry(a, b, options) {
  const curve = wireCurve(a, b, options);
  return { curve, points: flatten(curve), box: curveBox(curve) };
}

/** The extremes in (0, 1) of one coordinate of a cubic, `p0 … p3`. */
function extremes(p0, p1, p2, p3) {
  // The derivative over 3: a t² + b t + c.
  const a = -p0 + 3 * p1 - 3 * p2 + p3;
  const b = 2 * (p0 - 2 * p1 + p2);
  const c = p1 - p0;
  if (Math.abs(a) < 1e-12) return Math.abs(b) < 1e-12 ? [] : [-c / b];
  const disc = b * b - 4 * a * c;
  if (disc < 0) return [];
  const r = Math.sqrt(disc);
  return [(-b + r) / (2 * a), (-b - r) / (2 * a)];
}

/** The exact bounding box of `curve`: `{x0, y0, x1, y1}`. */
export function curveBox(curve) {
  const ps = [curve.start];
  let from = curve.start;
  for (const seg of curve.segs) {
    if (seg.c1) {
      const ts = [...extremes(from.x, seg.c1.x, seg.c2.x, seg.to.x), ...extremes(from.y, seg.c1.y, seg.c2.y, seg.to.y)];
      for (const t of ts) if (t > 0 && t < 1) ps.push(cubicAt(from, seg.c1, seg.c2, seg.to, t));
    }
    ps.push(seg.to);
    from = seg.to;
  }
  return hull(ps);
}

/** The flattened curve alone: what consumers of a polyline take. */
export function wirePoints(a, b, options) {
  return flatten(wireCurve(a, b, options));
}

/** The halves of cubic `[p0, c1, c2, p3]` at t = ½ (de Casteljau), sharing their middle point. */
export function split([p0, p1, p2, p3]) {
  const mid = (a, b) => ({ x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 });
  const a = mid(p0, p1);
  const b = mid(p1, p2);
  const c = mid(p2, p3);
  const d = mid(a, b);
  const e = mid(b, c);
  const m = mid(d, e);
  return [[p0, a, d, m], [m, e, c, p3]];
}

/** The bounding box of points `ps`: `{x0, y0, x1, y1}`. */
export function hull(ps) {
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  for (const p of ps) {
    if (p.x < x0) x0 = p.x;
    if (p.x > x1) x1 = p.x;
    if (p.y < y0) y0 = p.y;
    if (p.y > y1) y1 = p.y;
  }
  return { x0, y0, x1, y1 };
}

export function distanceToSegment(p, a, b) {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const length = dx * dx + dy * dy;
  const t = length === 0 ? 0 : Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / length));
  return Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy));
}

/** How far a cubic's handles are from its chord: its chord is within this of it. */
export function bow([p0, c1, c2, p3]) {
  return Math.max(distanceToSegment(c1, p0, p3), distanceToSegment(c2, p0, p3));
}

/**
 * The distance from `p` to `curve`, exact within `limit` / 16; Infinity when
 * it is farther than `limit` (pieces farther than that are never split, so
 * this is cheap at any zoom: `limit` is the hit tolerance in level units).
 */
export function curveDistance(curve, p, limit) {
  let best = Infinity;
  const eps = limit / 16;
  const visit = (c, depth) => {
    const box = hull(c);
    const dx = Math.max(box.x0 - p.x, 0, p.x - box.x1);
    const dy = Math.max(box.y0 - p.y, 0, p.y - box.y1);
    if (Math.hypot(dx, dy) > Math.min(best, limit)) return;
    if (depth >= 48 || bow(c) <= eps) {
      best = Math.min(best, distanceToSegment(p, c[0], c[3]));
      return;
    }
    const [l, r] = split(c);
    visit(l, depth + 1);
    visit(r, depth + 1);
  };
  let from = curve.start;
  for (const seg of curve.segs) {
    if (seg.c1) visit([from, seg.c1, seg.c2, seg.to], 0);
    else best = Math.min(best, distanceToSegment(p, from, seg.to));
    from = seg.to;
  }
  return best <= limit ? best : Infinity;
}

/**
 * The cord dragged from output `a` to the pointer at `b` (screen pixels): a
 * forward wire's curve, or left of `a` a loop below both.
 */
export function cordCurve(a, b) {
  const feedback = b.x <= a.x;
  return wireCurve(a, b, { feedback, under: Math.max(a.y, b.y) + 2 * LOOP_RADIUS });
}

/** `curve` as SVG path data, unbaked (the rubber band's, in screen pixels). */
export function curveData(curve) {
  let d = `M${curve.start.x},${curve.start.y}`;
  for (const s of curve.segs) d += s.c1 ? ` C${s.c1.x},${s.c1.y} ${s.c2.x},${s.c2.y} ${s.to.x},${s.to.y}` : ` L${s.to.x},${s.to.y}`;
  return d;
}
