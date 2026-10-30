// The board's maze router (ADR 0047 §6, after the owner's verdict of
// 2026-10-30): the plate's controls' legs, routed through the gutters
// between the plate's controls and the channels between the chips, never
// under a control or a chip. A grid of lines (every STEP units, and the
// lines every pin, box edge and wire already drawn needs) over the board;
// an A* search per wire over (node, heading) with a cost for its length,
// each turn and each crossing of another net. A wire never runs along
// another net's segment nor turns where another net passes; the legs to
// one chip are one bus and may share their way (at a lower cost), so they
// run together and fan out at the chip. Deterministic: lines sorted, ties
// in the queue by insertion. No DOM.

/** The grid's spacing (layout units). */
export const STEP = 16;
/** A route turning costs as much as running this far. */
export const BEND = 24;
/** Crossing another net costs as much as running this far. */
export const CROSS = 48;
/** A bus's own way costs this much of its length again. */
export const SHARED = 0.4;
/** A route keeps this far from a box (its nodes do). */
export const KEEP = 2;
/** The search keeps to the box around its ends grown by this (layout units), then to the whole grid. */
export const REACH = 480;
/** The search leans towards its goal by this much (a weighted A*: much faster, a route at most this much costlier). */
export const GREED = 1.2;

/** A pin's approach is this far out of it: a box's edge line (KEEP + 2). */
const APPROACH = KEEP + 2;
const DX = [1, -1, 0, 0];
const DY = [0, 0, 1, -1];

/** Sorted, distinct `values`, each `required` one kept and the others dropped within `near` of a kept one. */
function lines(required, uniform, near) {
  const req = [...new Set(required.map((v) => Math.round(v * 2) / 2))].sort((a, b) => a - b);
  const out = [...req];
  for (const v of uniform) {
    let lo = 0;
    let hi = req.length;
    while (lo < hi) {
      const m = (lo + hi) >> 1;
      if (req[m] < v) lo = m + 1;
      else hi = m;
    }
    const d = Math.min(lo < req.length ? Math.abs(req[lo] - v) : Infinity, lo > 0 ? Math.abs(req[lo - 1] - v) : Infinity);
    if (d >= near) out.push(v);
  }
  return out.sort((a, b) => a - b);
}

/** A binary heap of `[key, seq, value]` by key then seq. */
class Heap {
  constructor() {
    this.k = [];
    this.s = [];
    this.v = [];
  }
  get size() {
    return this.v.length;
  }
  less(i, j) {
    return this.k[i] < this.k[j] || (this.k[i] === this.k[j] && this.s[i] < this.s[j]);
  }
  swap(i, j) {
    [this.k[i], this.k[j]] = [this.k[j], this.k[i]];
    [this.s[i], this.s[j]] = [this.s[j], this.s[i]];
    [this.v[i], this.v[j]] = [this.v[j], this.v[i]];
  }
  push(key, seq, value) {
    this.k.push(key);
    this.s.push(seq);
    this.v.push(value);
    let i = this.v.length - 1;
    while (i > 0) {
      const p = (i - 1) >> 1;
      if (!this.less(i, p)) break;
      this.swap(i, p);
      i = p;
    }
  }
  pop() {
    const top = this.v[0];
    const last = this.v.length - 1;
    this.swap(0, last);
    this.k.pop();
    this.s.pop();
    this.v.pop();
    let i = 0;
    for (;;) {
      const l = 2 * i + 1;
      const r = l + 1;
      let m = i;
      if (l < this.v.length && this.less(l, m)) m = l;
      if (r < this.v.length && this.less(r, m)) m = r;
      if (m === i) break;
      this.swap(i, m);
      i = m;
    }
    return top;
  }
}

/**
 * The maze of one board: `boxes` (`[{id, x, y, w, h}]`, chips and controls
 * a route keeps clear of), `points` (`[{x, y}]`, every point a route starts
 * or ends at), `segments` (`[[p, q]]`, the axis-parallel segments already
 * drawn, each with its `net`), within `bounds` (`{x0, y0, x1, y1}`).
 */
export class Maze {
  constructor(boxes0, points, segments, bounds, pins = []) {
    // A rectangle given twice (a control as an obstacle and as a via's) is one.
    const seenBox = new Set();
    const boxes = boxes0.filter((b) => {
      const key = `${b.x},${b.y},${b.w},${b.h}`;
      if (seenBox.has(key)) return false;
      seenBox.add(key);
      return true;
    });
    const xs = [];
    const ys = [];
    for (const b of boxes) {
      xs.push(b.x - KEEP - 2, b.x + b.w + KEEP + 2);
      ys.push(b.y - KEEP - 2, b.y + b.h + KEEP + 2);
    }
    for (const p of [...points, ...pins]) {
      xs.push(p.x);
      ys.push(p.y);
    }
    for (const [p, q] of segments) {
      if (p.x === q.x) xs.push(p.x);
      if (p.y === q.y) ys.push(p.y);
    }
    const uniform = (a, b) => {
      const out = [];
      for (let v = Math.floor(a / STEP) * STEP; v <= b; v += STEP) out.push(v);
      return out;
    };
    this.xs = lines(xs.filter((v) => v >= bounds.x0 && v <= bounds.x1), uniform(bounds.x0, bounds.x1), STEP / 2);
    this.ys = lines(ys.filter((v) => v >= bounds.y0 && v <= bounds.y1), uniform(bounds.y0, bounds.y1), STEP / 2);
    this.nx = this.xs.length;
    this.ny = this.ys.length;
    const n = this.nx * this.ny;
    this.blocked = new Int32Array(n);
    this.boxes = boxes;
    for (const b of boxes) this.mark(b, 1);
    // Each pin's approach (the node just outside it) is its own: no other route runs through it.
    this.approach = (p) => this.node({ x: p.x + (p.out ? APPROACH : -APPROACH), y: p.y });
    for (const p of pins) {
      const a = this.approach(p);
      if (a >= 0) this.blocked[a] += 1;
    }
    // Per node: the net (index + 1) running through it across (h) and along (v), and one turning there; per edge, the net on it.
    this.passH = new Int32Array(n);
    this.passV = new Int32Array(n);
    this.turn = new Int32Array(n);
    this.edgeH = new Int32Array(n);
    this.edgeV = new Int32Array(n);
    this.nets = new Map();
    this.buses = [];
    for (const [p, q, net] of segments.map((s) => [s[0], s[1], s.net])) this.takeSegment(p, q, this.netId(net), false);
    const states = n * 4;
    this.g = new Float64Array(states);
    this.seen = new Int32Array(states);
    this.from = new Int32Array(states);
    this.done = new Int32Array(states);
    this.stamp = 0;
  }

  netId(net, bus = null) {
    if (!this.nets.has(net)) {
      this.nets.set(net, this.nets.size + 1);
      this.buses[this.nets.size] = bus;
    }
    return this.nets.get(net);
  }

  /** The index of line value `v` in `list`, or -1. */
  static at(list, v) {
    let lo = 0;
    let hi = list.length - 1;
    const want = Math.round(v * 2) / 2;
    while (lo <= hi) {
      const m = (lo + hi) >> 1;
      if (list[m] === want) return m;
      if (list[m] < want) lo = m + 1;
      else hi = m - 1;
    }
    return -1;
  }

  node(p) {
    const i = Maze.at(this.xs, p.x);
    const j = Maze.at(this.ys, p.y);
    return i < 0 || j < 0 ? -1 : j * this.nx + i;
  }

  /** Box `b`'s nodes blocked (`d` 1) or freed again (`d` −1): those within KEEP of it. */
  mark(b, d) {
    for (let i = 0; i < this.nx; i++) {
      const x = this.xs[i];
      if (x < b.x - KEEP || x > b.x + b.w + KEEP) continue;
      for (let j = 0; j < this.ny; j++) {
        const y = this.ys[j];
        if (y < b.y - KEEP || y > b.y + b.h + KEEP) continue;
        this.blocked[j * this.nx + i] += d;
      }
    }
  }

  /** Segment p–q (on lines) taken by net `id`. */
  takeSegment(p, q, id, turns = true) {
    const a = this.node(p);
    const b = this.node(q);
    if (a < 0 || b < 0) return;
    const [ai, aj] = [a % this.nx, Math.floor(a / this.nx)];
    const [bi, bj] = [b % this.nx, Math.floor(b / this.nx)];
    if (aj === bj) {
      for (let i = Math.min(ai, bi); i < Math.max(ai, bi); i++) this.edgeH[aj * this.nx + i] = id;
      for (let i = Math.min(ai, bi); i <= Math.max(ai, bi); i++) this.passH[aj * this.nx + i] = id;
    } else if (ai === bi) {
      for (let j = Math.min(aj, bj); j < Math.max(aj, bj); j++) this.edgeV[j * this.nx + ai] = id;
      for (let j = Math.min(aj, bj); j <= Math.max(aj, bj); j++) this.passV[j * this.nx + ai] = id;
    }
    if (turns) {
      this.turn[a] = this.turn[a] || id;
      this.turn[b] = this.turn[b] || id;
    }
  }

  /** Whether net `id` may use what net `other` holds (nothing, itself, or its bus). */
  shares(id, other) {
    if (!other || other === id || this.alsoShare?.has(other)) return true;
    const bus = this.buses[id];
    return bus !== null && bus !== undefined && bus === this.buses[other];
  }

  /**
   * The cheapest route of `net` (in bus `bus`, or null) from `start`
   * leaving along +x to `goal` arriving along +x, the boxes `own` (its own
   * control and pad) passable; its corners, taken, or null.
   */
  route(start, goal, net, { bus = null, own = [], share = [] } = {}) {
    const s = this.node(start);
    const t = this.node(goal);
    if (s < 0 || t < 0) return null;
    const id = this.netId(net, bus);
    // The nets at its own pins (a wire into the parameter it sets as well): it may join them there.
    this.alsoShare = new Set(share.filter((n) => this.nets.has(n)).map((n) => this.nets.get(n)));
    for (const b of own) this.mark(b, -1);
    const ways = [this.approach({ ...start, out: true }), this.approach({ ...goal, out: false })].filter((a) => a >= 0);
    for (const a of ways) this.blocked[a] -= 1;
    const reach = { x0: Math.min(start.x, goal.x) - REACH, x1: Math.max(start.x, goal.x) + REACH, y0: Math.min(start.y, goal.y) - REACH, y1: Math.max(start.y, goal.y) + REACH };
    let found = this.search(s, t, id, reach);
    if (found === null) found = this.search(s, t, id, null);
    for (const b of own) this.mark(b, 1);
    for (const a of ways) this.blocked[a] += 1;
    this.alsoShare = null;
    if (found === null) return null;
    const corners = found.map(({ n }) => ({ x: this.xs[n % this.nx], y: this.ys[Math.floor(n / this.nx)] }));
    const clean = [];
    for (const p of corners) {
      const last = clean.at(-1);
      if (last && last.x === p.x && last.y === p.y) continue;
      if (clean.length >= 2) {
        const a = clean.at(-2);
        if ((a.x === last.x && last.x === p.x) || (a.y === last.y && last.y === p.y)) clean.pop();
      }
      clean.push(p);
    }
    for (let k = 1; k < clean.length; k++) this.takeSegment(clean[k - 1], clean[k], id);
    return clean;
  }

  search(s, t, id, reach) {
    const { nx, xs, ys } = this;
    this.stamp++;
    const stamp = this.stamp;
    const tx = xs[t % nx];
    const ty = ys[Math.floor(t / nx)];
    const heap = new Heap();
    let seq = 0;
    const start = s * 4;
    this.g[start] = 0;
    this.seen[start] = stamp;
    this.from[start] = -1;
    const h = (n) => Math.abs(xs[n % nx] - tx) + Math.abs(ys[Math.floor(n / nx)] - ty);
    heap.push(GREED * h(s), seq++, start);
    while (heap.size) {
      const state = heap.pop();
      if (this.done[state] === stamp) continue;
      this.done[state] = stamp;
      const n = state >> 2;
      const d = state & 3;
      if (n === t && d === 0) {
        const path = [];
        for (let st = state; st !== -1; st = this.from[st]) path.push({ n: st >> 2, d: st & 3 });
        return path.reverse();
      }
      const i = n % nx;
      const j = Math.floor(n / nx);
      const turning = (nd) => nd !== d;
      for (let nd = 0; nd < 4; nd++) {
        if ((d ^ 1) === nd && d < 4) continue;
        // It leaves its start along +x.
        if (n === s && nd !== 0) continue;
        const ni = i + DX[nd];
        const nj = j + DY[nd];
        if (ni < 0 || nj < 0 || ni >= nx || nj >= this.ny) continue;
        const m = nj * nx + ni;
        if (reach && (xs[ni] < reach.x0 || xs[ni] > reach.x1 || ys[nj] < reach.y0 || ys[nj] > reach.y1)) continue;
        if (this.blocked[m] > 0 && m !== t) continue;
        // Along another net's segment: never (its bus's: cheaper).
        const edge = nd < 2 ? this.edgeH[j * nx + Math.min(i, ni)] : this.edgeV[Math.min(j, nj) * nx + i];
        if (!this.shares(id, edge)) continue;
        // Turning where another net passes or turns: never.
        if (turning(nd) && n !== s && (!this.shares(id, this.passH[n]) || !this.shares(id, this.passV[n]) || !this.shares(id, this.turn[n]))) continue;
        let cost = Math.abs(xs[ni] - xs[i]) + Math.abs(ys[nj] - ys[j]);
        if (edge && edge !== id) cost *= SHARED;
        if (turning(nd) && n !== s) cost += BEND;
        // Crossing another net that runs across.
        const across = nd < 2 ? this.passV[m] : this.passH[m];
        if (across && !this.shares(id, across)) cost += CROSS;
        const next = m * 4 + nd;
        const g = this.g[state] + cost;
        if (this.seen[next] === stamp && this.g[next] <= g) continue;
        this.seen[next] = stamp;
        this.g[next] = g;
        this.from[next] = state;
        heap.push(g + GREED * h(m), seq++, next);
      }
    }
    return null;
  }
}
