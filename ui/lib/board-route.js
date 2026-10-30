// The board's wiring between chips (ADR 0047 §6; the owner's verdict of
// 2026-10-30): orthogonal routes that never pass under a box, and the
// measures (crossings, bends, length) shown beside the drawing. A wire
// leaves its output to the right and enters its input from the left, both
// straight for at least STUB; between them it turns only on vertical
// segments, each on a track of the channel it is in, ordered against
// crossings (`trackPreferences`): a vertical of one net never comes within
// a TRACK of another net's over a common height, and a net's wires from
// one pin share their first vertical (a trunk). Long edges follow their
// dummies (ui/lib/board-layout.js) through the columns they cross;
// feedback runs in its own lane below everything. Only a net with
// NET_DESTS destinations or more, a global net (the transport's clock) and
// a wire that finds no way are drawn as labels: a stub at its source
// `▸ name` and at each destination `◂ name`; traced, it is drawn whole.
// The plate's legs are the maze's (ui/lib/board-maze.js). Corners are
// rounded (`rounded`, ui/lib/schematic-wire.js). Deterministic: wires are
// routed shortest first, ties by their order; candidates in a fixed order.
// No DOM.

import { curveBox, flatten, rounded } from './schematic-wire.js';
import { STUB, TRACK } from './board-layout.js';

/** A net with this many destinations or more is drawn as labels. */
export const NET_DESTS = 4;
/** A label's stub, out of its pin. */
export const LABEL_STUB = 28;
/** A route keeps this far from a box it passes. */
export const CLEAR = 4;
/** Turns are rounded to this radius. */
export const CORNER = 6;
/** Candidates tried per leg: tracks out from the preferred one. */
const TRIES = 48;
/** Lanes tried by a detour. */
const LANES = 12;

/** The geometry of an orthogonal polyline `corners`, its turns rounded to `corner`: `{curve, points, box}` (as `wireGeometry` gives). */
export function orthGeometry(corners, corner = CORNER) {
  const curve = rounded(corners, corner);
  return { curve, points: flatten(curve), box: curveBox(curve) };
}

/**
 * The plain route of a wire from `a` to `b` with no obstacles known (a box
 * dragged, a bundle drawn): over `x` (else halfway), or a feedback loop
 * through lane `under`.
 */
export function zCorners(a, b, { feedback = false, under = 0, x = null } = {}) {
  if (feedback) {
    const right = a.x + STUB;
    const left = b.x - STUB;
    return [a, { x: right, y: a.y }, { x: right, y: under }, { x: left, y: under }, { x: left, y: b.y }, b];
  }
  if (Math.abs(a.y - b.y) < 0.5) return [a, b];
  const lo = a.x + Math.min(STUB, (b.x - a.x) / 2);
  const hi = b.x - Math.min(STUB, (b.x - a.x) / 2);
  const mx = x === null ? (a.x + b.x) / 2 : Math.min(hi, Math.max(lo, x));
  return [a, { x: mx, y: a.y }, { x: mx, y: b.y }, b];
}

function inside(seg, box) {
  // A segment (axis-parallel) through the open inside of `box` grown by CLEAR.
  const x0 = box.x - CLEAR;
  const x1 = box.x + box.w + CLEAR;
  const y0 = box.y - CLEAR;
  const y1 = box.y + box.h + CLEAR;
  const sx0 = Math.min(seg.x0, seg.x1);
  const sx1 = Math.max(seg.x0, seg.x1);
  const sy0 = Math.min(seg.y0, seg.y1);
  const sy1 = Math.max(seg.y0, seg.y1);
  return sx0 < x1 && sx1 > x0 && sy0 < y1 && sy1 > y0;
}

/**
 * The router of one level: `boxes` (`[{id, x, y, w, h, via}]`, every box a
 * wire must not pass; a control's leg, `overVias`, may pass under a via). Kept occupancy makes the
 * tracks.
 */
class Router {
  constructor(boxes) {
    this.boxes = boxes;
    this.vertical = [];
    this.horizontal = [];
    /** Each net's trunk x per upper station: `${net}\n${x}\n${y}` → x. */
    this.trunks = new Map();
  }

  clearOfBoxes(seg, skip, also) {
    for (const box of this.boxes) {
      if (skip.has(box.id) || also.has(box.id)) continue;
      // A control's leg runs on the board under the plate's other controls (they are drawn over it), never under a chip.
      if (box.via && this.overVias) continue;
      if (inside(seg, box)) return false;
    }
    return true;
  }

  freeV(x, y0, y1, net) {
    const lo = Math.min(y0, y1) - TRACK / 2;
    const hi = Math.max(y0, y1) + TRACK / 2;
    for (const o of this.vertical) {
      if (o.net === net) continue;
      if (Math.abs(o.x - x) < TRACK - 0.01 && o.lo < hi && o.hi > lo) return false;
    }
    return true;
  }

  freeH(y, x0, x1, net) {
    const lo = Math.min(x0, x1);
    const hi = Math.max(x0, x1);
    for (const o of this.horizontal) {
      if (o.net === net) continue;
      if (Math.abs(o.y - y) < TRACK / 2 - 0.01 && o.lo < hi - 0.01 && o.hi > lo + 0.01) return false;
    }
    return true;
  }

  /** Whether polyline `corners` is clear (boxes, other nets): the first segment skips `skipA`, the last `skipB`. */
  clear(corners, net, skipA, skipB) {
    const none = new Set();
    for (let i = 1; i < corners.length; i++) {
      const p = corners[i - 1];
      const q = corners[i];
      if (p.x === q.x && p.y === q.y) continue;
      const seg = { x0: p.x, y0: p.y, x1: q.x, y1: q.y };
      if (i === 1 && i === corners.length - 1) {
        if (!this.clearOfBoxes(seg, skipA, skipB)) return false;
      } else if (!this.clearOfBoxes(seg, i === 1 ? skipA : i === corners.length - 1 ? skipB : none, none)) return false;
      if (p.x === q.x ? !this.freeV(p.x, p.y, q.y, net) : !this.freeH(p.y, p.x, q.x, net)) return false;
    }
    return true;
  }

  take(corners, net) {
    for (let i = 1; i < corners.length; i++) {
      const p = corners[i - 1];
      const q = corners[i];
      if (p.x === q.x && p.y !== q.y) this.vertical.push({ x: p.x, lo: Math.min(p.y, q.y), hi: Math.max(p.y, q.y), net });
      else if (p.y === q.y && p.x !== q.x) this.horizontal.push({ y: p.y, lo: Math.min(p.x, q.x), hi: Math.max(p.x, q.x), net });
    }
  }

  /** Candidate x for a vertical in [lo, hi]: from `pref` outward by TRACK, at most TRIES. */
  static xs(lo, hi, pref) {
    const out = [];
    if (hi < lo) return out;
    const start = Math.min(hi, Math.max(lo, pref));
    out.push(start);
    for (let k = 1; out.length < TRIES && (start - k * TRACK >= lo || start + k * TRACK <= hi); k++) {
      if (start + k * TRACK <= hi) out.push(start + k * TRACK);
      if (start - k * TRACK >= lo && out.length < TRIES) out.push(start - k * TRACK);
    }
    return out;
  }

  /**
   * One leg from `p` (leaving right) to `q` (entering from the left): a
   * straight run, a Z over a free track, or a detour along a free lane.
   * The corners, or null.
   */
  leg(p, q, net, skipA, skipB) {
    if (Math.abs(p.y - q.y) < 0.5 && q.x > p.x) {
      const c = [p, { x: q.x, y: p.y }];
      if (this.clear(c, net, skipA, skipB)) return c;
    }
    const lo = p.x + STUB;
    const hi = q.x - STUB;
    if (hi >= lo) {
      const trunk = this.trunks.get(`${net}\n${p.x}\n${p.y}`);
      const pref = trunk ?? this.prefs?.get(legKey(p, q)) ?? Math.round((lo + hi) / 2 / TRACK) * TRACK;
      for (const x of trunk !== undefined ? [trunk, ...Router.xs(lo, hi, pref)] : Router.xs(lo, hi, pref)) {
        if (x < lo || x > hi) continue;
        const c = [p, { x, y: p.y }, { x, y: q.y }, q];
        if (this.clear(c, net, skipA, skipB)) {
          this.trunks.set(`${net}\n${p.x}\n${p.y}`, x);
          return c;
        }
      }
    }
    return this.detour(p, q, net, skipA, skipB);
  }

  /** Five segments: out of `p` to a track, along a free lane, to a track, into `q`. */
  detour(p, q, net, skipA, skipB) {
    const x0 = Math.min(p.x, q.x);
    const x1 = Math.max(p.x, q.x);
    const lanes = [];
    for (const box of this.boxes) {
      if (box.x > x1 + STUB || box.x + box.w < x0 - STUB) continue;
      lanes.push(box.y - CLEAR - TRACK, box.y + box.h + CLEAR + TRACK);
    }
    const mid = (p.y + q.y) / 2;
    const unique = [...new Set(lanes.map((y) => Math.round(y)))].sort((a, b) => Math.abs(a - mid) - Math.abs(b - mid) || a - b).slice(0, LANES);
    const none = new Set();
    for (const y of unique) {
      // Its two ends are independent of each other; the lane between them is checked last.
      const xa = Router.xs(p.x + STUB, p.x + STUB + 6 * TRACK, p.x + STUB).find((x) => this.clear([p, { x, y: p.y }, { x, y }], net, skipA, none));
      if (xa === undefined) continue;
      const xb = Router.xs(q.x - STUB - 6 * TRACK, q.x - STUB, q.x - STUB).find((x) => this.clear([{ x, y }, { x, y: q.y }, q], net, none, skipB));
      if (xb === undefined) continue;
      if (this.clear([{ x: xa, y }, { x: xb, y }], net, none, none)) return [p, { x: xa, y: p.y }, { x: xa, y }, { x: xb, y }, { x: xb, y: q.y }, q];
    }
    return null;
  }

  /** A feedback wire's loop through lane y `under`. */
  loop(a, b, under, net, skipA, skipB) {
    // Its own lane: the way down and the way up are independent.
    const none = new Set();
    const xa = Router.xs(a.x + STUB, a.x + STUB + TRIES * TRACK, a.x + STUB).slice(0, 24).find((x) => this.clear([a, { x, y: a.y }, { x, y: under }], net, skipA, none));
    if (xa === undefined) return null;
    const xb = Router.xs(b.x - STUB - TRIES * TRACK, b.x - STUB, b.x - STUB).slice(0, 24).find((x) => this.clear([{ x, y: under }, { x, y: b.y }, b], net, none, skipB));
    if (xb === undefined) return null;
    return [a, { x: xa, y: a.y }, { x: xa, y: under }, { x: xb, y: under }, { x: xb, y: b.y }, b];
  }
}

/** Corners without repeated points or straight-through ones. */
export function simplify(corners) {
  const out = [];
  for (const p of corners) {
    const last = out.at(-1);
    if (last && last.x === p.x && last.y === p.y) continue;
    if (out.length >= 2) {
      const a = out.at(-2);
      if ((a.x === last.x && last.x === p.x) || (a.y === last.y && last.y === p.y)) out.pop();
    }
    out.push({ x: p.x, y: p.y });
  }
  return out;
}

/**
 * The routes of one level's `wires` (`[{id, net, from, to, a, b,
 * feedback, under, stations, span, crossesPlate}]`, `a` the output pin,
 * `b` the input, `from`/`to` their boxes' ids, `stations` the runs
 * `[{x0, x1, y}]` through the columns a long edge crosses, `span` the
 * layers it crosses) among `boxes`. Returns per wire (same order)
 * `{corners, labelled, whole}` (`whole` the full route of a labelled
 * wire, drawn when traced) and the level's `labels` (`[{net, text, side,
 * at, corners, wire}]`, one at the source of each labelled net, one per
 * labelled destination) and `metrics` (`{crossings, bends, length,
 * labels}`). `fanout` false (a die, ADR 0047 phase 3) labels no net for
 * its fan-out or as global; `fallback(taken)`, called at the first wire
 * that finds no way with the segments drawn so far, gives `{route(w),
 * take(corners, net)}` (the maze): a second search before a label.
 */
export function routeWires(boxes, wires, { nameOf = (w) => w.net, fanout = true, fallback = null } = {}) {
  const router = new Router(boxes);
  router.prefs = trackPreferences(wires);
  // Nets: the wires from one pin.
  const nets = new Map();
  for (const w of wires) {
    if (!nets.has(w.net)) nets.set(w.net, []);
    nets.get(w.net).push(w);
  }
  // Labels only for a true fan-out and a global net (the owner's verdict: fewer labels, more real wires).
  // A die (ADR 0047 phase 3, `fanout` false) draws every net as wires: only a wire with no way is a label.
  const labelledNet = (list) => fanout && (list.length >= NET_DESTS || list.some((w) => w.global));
  const result = wires.map(() => null);
  const order = wires.map((w, i) => ({ w, i, d: Math.abs(w.b.x - w.a.x) + Math.abs(w.b.y - w.a.y) + (w.feedback ? 1e6 : 0) }));
  order.sort((p, q) => p.d - q.d || p.i - q.i);
  const skip = (id) => new Set(id === undefined || id === null ? [] : [id]);
  // The segments drawn so far (with their nets), for a fallback made at the first wire with no way.
  const taken = [];
  let maze = null;
  for (const { w, i } of order) {
    router.overVias = Boolean(w.overVias);
    if (w.never) {
      result[i] = { corners: zCorners(w.a, w.b, { feedback: w.feedback, under: w.under }), labelled: false };
      continue;
    }
    if (labelledNet(nets.get(w.net))) {
      result[i] = { labelled: true };
      continue;
    }
    let corners = null;
    if (w.feedback) corners = router.loop(w.a, w.b, w.under, w.net, skip(w.from), skip(w.to));
    else {
      if (w.stations?.length) corners = through(router, w);
      corners ??= router.leg(w.a, w.b, w.net, skip(w.from), skip(w.to));
    }
    // No way in the channels: the maze's search around everything drawn so far (`fallback`, a die's), else a label.
    if (!corners && fallback) {
      if (!maze) {
        maze = fallback(taken);
      }
      corners = maze?.route(w) ?? null;
    }
    if (corners) {
      corners = simplify(corners);
      router.take(corners, w.net);
      for (let k = 1; k < corners.length; k++) taken.push(Object.assign([corners[k - 1], corners[k]], { net: w.net }));
      if (maze) maze.take(corners, w.net);
      result[i] = { corners, labelled: false };
    } else result[i] = { labelled: true, failed: true };
  }
  // Labelled wires: their whole route for a trace (best effort, taking no track), and the stubs.
  const labels = [];
  const sourced = new Set();
  wires.forEach((w, i) => {
    const r = result[i];
    if (!r.labelled) return;
    // Its whole route, drawn only while traced: the plain one, taking no track.
    r.whole = simplify(zCorners(w.a, w.b, { feedback: w.feedback, under: w.under }));
    r.corners = [{ x: w.b.x - LABEL_STUB, y: w.b.y }, w.b];
    const text = nameOf(w);
    const at = w.labelAt ?? 'both';
    if (at !== 'out') labels.push({ net: w.net, text: `◂ ${text}`, side: 'in', at: { x: w.b.x - LABEL_STUB, y: w.b.y }, corners: r.corners, wire: w.id });
    else r.corners = [w.a, { x: w.a.x + LABEL_STUB, y: w.a.y }];
    if (at !== 'in' && !sourced.has(w.net)) {
      sourced.add(w.net);
      const end = { x: w.a.x + LABEL_STUB, y: w.a.y };
      labels.push({ net: w.net, text: `▸ ${text}`, side: 'out', at: end, corners: [w.a, end], wire: w.id });
    }
  });
  return { routes: result, labels, metrics: measure(wires, result, labels) };
}

/** The key of a leg from `p` to `q`. */
function legKey(p, q) {
  return `${p.x},${p.y}>${q.x},${q.y}`;
}

/** The legs of a forward wire: from its output through its stations to its input, `[{p, q, net}]`. */
function legsOf(w) {
  const legs = [];
  let p = w.a;
  for (const s of w.stations ?? []) {
    legs.push({ p, q: { x: s.x0, y: s.y }, net: w.net });
    p = { x: s.x1, y: s.y };
  }
  legs.push({ p, q: w.b, net: w.net });
  return legs;
}

/**
 * How many crossings `legs` (each with its track `x`) make among each
 * other in one channel: a leg's vertical with another's horizontals.
 */
export function legCrossings(legs) {
  let c = 0;
  const between = (v, a, b) => v > Math.min(a, b) && v < Math.max(a, b);
  for (const A of legs) {
    for (const B of legs) {
      if (A === B || A.net === B.net) continue;
      if (between(A.x, B.p.x, B.x) && between(B.p.y, A.p.y, A.q.y)) c++;
      if (between(A.x, B.x, B.q.x) && between(B.q.y, A.p.y, A.q.y)) c++;
    }
  }
  return c;
}

/**
 * Where each leg's vertical would best run (ADR 0047 §6.2, the tracks of a
 * channel): the legs entering the same column (the same `q.x`) are one
 * channel; a net's legs from one point share a track; legs going down
 * take tracks right to left as their start goes down, legs going up left
 * to right; of the two orders of the groups (up first, down first) the one
 * with fewer crossings, ties up first. Tracks TRACK apart, centred in the
 * room all of them have. A Map of `legKey` → x, which the router tries
 * first.
 */
export function trackPreferences(wires) {
  const channels = new Map();
  for (const w of wires) {
    if (w.feedback || w.never) continue;
    for (const leg of legsOf(w)) {
      if (Math.abs(leg.p.y - leg.q.y) < 0.5 || leg.q.x - leg.p.x < 2 * STUB) continue;
      const key = Math.round(leg.q.x);
      if (!channels.has(key)) channels.set(key, []);
      channels.get(key).push(leg);
    }
  }
  const prefs = new Map();
  for (const [, legs] of [...channels].sort((a, b) => a[0] - b[0])) {
    // One track per net and starting point.
    const tracks = new Map();
    for (const leg of legs) {
      const key = `${leg.net}
${leg.p.x}
${leg.p.y}`;
      if (!tracks.has(key)) tracks.set(key, { legs: [], p: leg.p, net: leg.net, down: 0 });
      const t = tracks.get(key);
      t.legs.push(leg);
      t.down += leg.q.y > leg.p.y ? 1 : -1;
    }
    const list = [...tracks.values()];
    const down = list.filter((t) => t.down > 0).sort((a, b) => b.p.y - a.p.y || a.p.x - b.p.x);
    const up = list.filter((t) => t.down <= 0).sort((a, b) => a.p.y - b.p.y || a.p.x - b.p.x);
    const lo = Math.max(...legs.map((l) => l.p.x + STUB));
    const hi = Math.min(...legs.map((l) => l.q.x - STUB));
    const place = (order) => {
      const n = order.length;
      const mid = (lo + hi) / 2;
      const out = new Map();
      order.forEach((t, i) => out.set(t, Math.round((mid + (i - (n - 1) / 2) * TRACK) / 2) * 2));
      return out;
    };
    const cost = (xs) => legCrossings(list.flatMap((t) => t.legs.map((l) => ({ ...l, x: xs.get(t) }))));
    const a = place([...up, ...down]);
    const b = place([...down, ...up]);
    const xs = cost(b) < cost(a) ? b : a;
    for (const t of list) for (const leg of t.legs) prefs.set(legKey(leg.p, leg.q), xs.get(t));
  }
  return prefs;
}

/** A long edge along its stations, leg by leg; null when one leg finds no way. */
function through(router, w) {
  const corners = [w.a];
  let p = w.a;
  let skipA = new Set([w.from]);
  for (const s of w.stations) {
    const leg = router.leg(p, { x: s.x0, y: s.y }, w.net, skipA, new Set());
    if (!leg) return null;
    const run = [{ x: s.x0, y: s.y }, { x: s.x1, y: s.y }];
    if (!router.clear(run, w.net, new Set(), new Set())) return null;
    corners.push(...leg.slice(1), run[1]);
    p = run[1];
    skipA = new Set();
  }
  const last = router.leg(p, w.b, w.net, skipA, new Set([w.to]));
  if (!last) return null;
  corners.push(...last.slice(1));
  return corners;
}

/** The segments of corners `c`. */
function segments(c) {
  const out = [];
  for (let i = 1; i < c.length; i++) out.push([c[i - 1], c[i]]);
  return out;
}

/**
 * The measures of a routed level (ADR 0047 §6.6; shown, never deciding):
 * crossings between drawn wires of different nets, bends (corners), the
 * total drawn length, and the labelled nets.
 */
export function measure(wires, routes, labels = []) {
  // A segment drawn twice by one net (a bus's shared way) counts once; a crossing point of two nets once.
  const h = [];
  const v = [];
  const seen = new Set();
  let bends = 0;
  let length = 0;
  routes.forEach((r, i) => {
    const c = r?.corners ?? [];
    bends += Math.max(0, c.length - 2);
    const net = wires[i].net;
    for (const [p, q] of segments(c)) {
      const key = `${net}\n${Math.min(p.x, q.x)},${Math.min(p.y, q.y)},${Math.max(p.x, q.x)},${Math.max(p.y, q.y)}`;
      if (seen.has(key)) continue;
      seen.add(key);
      length += Math.abs(q.x - p.x) + Math.abs(q.y - p.y);
      if (p.y === q.y) h.push({ y: p.y, x0: Math.min(p.x, q.x), x1: Math.max(p.x, q.x), net });
      else if (p.x === q.x) v.push({ x: p.x, y0: Math.min(p.y, q.y), y1: Math.max(p.y, q.y), net });
    }
  });
  const points = new Set();
  for (const a of h) for (const b of v) if (a.net !== b.net && b.x > a.x0 && b.x < a.x1 && a.y > b.y0 && a.y < b.y1) points.add(`${b.x},${a.y}\n${a.net < b.net ? `${a.net}\n${b.net}` : `${b.net}\n${a.net}`}`);
  return { crossings: points.size, bends, length: Math.round(length), labels: new Set(labels.map((l) => l.net)).size };
}

/**
 * The same measures of any drawn wires (`[{points, net}]`, polylines, as
 * the curved layout gives them): crossings between wires of different
 * nets (segment intersections), the length; bends are not counted for
 * curves. What the board's measures are compared against.
 */
export function polylineMetrics(wires) {
  const segs = [];
  let length = 0;
  for (const w of wires) {
    for (let i = 1; i < w.points.length; i++) {
      const p = w.points[i - 1];
      const q = w.points[i];
      length += Math.hypot(q.x - p.x, q.y - p.y);
      segs.push({ p, q, net: w.net, x0: Math.min(p.x, q.x), x1: Math.max(p.x, q.x), y0: Math.min(p.y, q.y), y1: Math.max(p.y, q.y) });
    }
  }
  const cross = (a, b) => {
    const d = (p, q, r) => (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x);
    const d1 = d(a.p, a.q, b.p);
    const d2 = d(a.p, a.q, b.q);
    const d3 = d(b.p, b.q, a.p);
    const d4 = d(b.p, b.q, a.q);
    return d1 * d2 < 0 && d3 * d4 < 0;
  };
  // Pairs of wires crossing (a wire's several crossings with another each count).
  let crossings = 0;
  segs.sort((a, b) => a.x0 - b.x0);
  for (let i = 0; i < segs.length; i++) {
    const a = segs[i];
    for (let j = i + 1; j < segs.length && segs[j].x0 <= a.x1; j++) {
      const b = segs[j];
      if (a.net === b.net || b.y0 > a.y1 || b.y1 < a.y0) continue;
      if (cross(a, b)) crossings++;
    }
  }
  return { crossings, length: Math.round(length) };
}

/** A net label's text: its font size (layout units), */
export const LABEL_TEXT = 9;
/** about this wide per character, */
export const LABEL_CHAR = 5.4;
/** a line this tall; a label moved off another goes this many lines up or down at most. */
export const LABEL_LINE = 11;
const LABEL_TRIES = 4;

/** The rectangle of `label`'s text with its baseline moved by `dy`. */
function labelRect(label, dy) {
  const w = label.text.length * LABEL_CHAR;
  const x = label.side === 'out' ? label.at.x + 3 : label.at.x - 3 - w;
  return { x, y: label.at.y + dy - LABEL_TEXT + 3, w, h: LABEL_LINE - 1 };
}

/**
 * The labels' texts kept off one another (ADR 0047 phase 3, the owner's
 * "labels overlap"): in order, each label's text stays at its stub when it
 * overlaps no text placed before it, else moves the fewest lines up or down
 * (up first) to where it overlaps none, at most LABEL_TRIES lines; its stub
 * stays at its pin. Sets each label's `text` point (`{x, y}`, the
 * baseline's start or end by its side). Deterministic: the labels' order.
 */
export function spreadLabels(labels) {
  const placed = [];
  const hit = (r) => placed.some((o) => r.x < o.x + o.w && o.x < r.x + r.w && r.y < o.y + o.h && o.y < r.y + r.h);
  for (const label of labels) {
    let dy = 0;
    for (let k = 1; k <= LABEL_TRIES && hit(labelRect(label, dy)); k++) {
      dy = -k * LABEL_LINE;
      if (!hit(labelRect(label, dy))) break;
      dy = k * LABEL_LINE;
    }
    if (hit(labelRect(label, dy))) dy = 0;
    placed.push(labelRect(label, dy));
    label.textAt = { x: label.at.x + (label.side === 'out' ? 3 : -3), y: label.at.y + 3 + dy };
  }
  return labels;
}

