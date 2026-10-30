// The board's placement (ADR 0047 §5–6, phase 2): an in-house layered
// (Sugiyama) layout and the placement of chips near the plate's controls.
// - `sugiyama`: layers as given (longest path), sources pulled right (the
//   balancing pass), dummy nodes on long edges, crossing minimisation by
//   layer sweeps (median, port order aware: a pin's row on its box counts)
//   with a transpose step, the best order kept; then coordinates: columns
//   left to right, each sized for the vertical tracks its channel needs,
//   and within a column each element as near its neighbours' pins as the
//   order allows (isotonic regression, a simplified Brandes–Köpf: dummies
//   weigh more, so long edges run straight).
// - `placeAnchored`: an anchored chip (one the plate's controls bind) at
//   the nearest free spot to its vias' centroid, searched in a fixed
//   spiral of whole plate units.
// Deterministic: the result is a function of the arrays' order (source
// order), the numbers given and fixed constants. Fixed iteration counts,
// ties broken by source order, never by an object's key order. No DOM.

/** One plate unit (ui/lib/plate.js `PU`): the board's grid. */
export const PU = 8;
/** Between two parallel vertical wires of a channel (layout units). */
export const TRACK = 8;
/** A wire runs at least this far straight out of a pin before it turns. */
export const STUB = 16;
/** The narrowest channel between two columns. */
export const MIN_CHANNEL = 72;
/** Between two boxes of a column; a dummy keeps half of it, and TRACK from another dummy. */
export const GAP_Y = 24;
/** Crossing minimisation: this many down-and-up sweeps, each with its transpose. */
export const SWEEPS = 8;
/** At most this many transpose passes per sweep. */
const TRANSPOSE_PASSES = 4;
/** Coordinate assignment: this many down-and-up passes. */
const PASSES = 8;
/** A dummy's pull, against a box's 1: long edges straight. */
const DUMMY_WEIGHT = 4;
/** An element with no neighbour on the side looked at stays where it is, this strongly. */
const STAY_WEIGHT = 0.25;
/** The spiral search for an anchored chip goes this many rings (plate units) out at most. */
export const SPIRAL_RINGS = 120;
/** A chip keeps this far (layout units) from a via, a card's control and another chip. */
export const CLEARANCE = PU;

const DUMMY = '@dummy:';

/** Whether `id` is a dummy node's. */
export const isDummy = (id) => String(id).startsWith(DUMMY);

/**
 * The layers after the balancing pass: a node with no predecessor (in
 * `edges`, `[{from, to}]` already pointing down the layers) moves as far
 * right as its successors allow, latest layer first; `keep(id)` ones stay.
 */
export function balance(ids, edges, layer, keep = () => false) {
  const succ = new Map(ids.map((id) => [id, []]));
  const hasPred = new Set();
  for (const e of edges) {
    if (e.from === e.to) continue;
    succ.get(e.from)?.push(e.to);
    hasPred.add(e.to);
  }
  const out = new Map(layer);
  const order = [...ids].sort((a, b) => out.get(b) - out.get(a) || ids.indexOf(a) - ids.indexOf(b));
  for (const id of order) {
    if (hasPred.has(id) || keep(id)) continue;
    const next = succ.get(id) ?? [];
    if (!next.length) continue;
    let min = Infinity;
    for (const s of next) min = Math.min(min, out.get(s));
    if (min - 1 > out.get(id)) out.set(id, min - 1);
  }
  return out;
}

/** The number of inversions of `values` (pairs i < j with values[i] > values[j]), by merge sort. */
export function inversions(values) {
  const a = [...values];
  const tmp = new Array(a.length);
  let count = 0;
  for (let width = 1; width < a.length; width *= 2) {
    for (let lo = 0; lo < a.length; lo += 2 * width) {
      const mid = Math.min(lo + width, a.length);
      const hi = Math.min(lo + 2 * width, a.length);
      let i = lo;
      let j = mid;
      let k = lo;
      while (i < mid && j < hi) {
        if (a[j] < a[i]) {
          count += mid - i;
          tmp[k++] = a[j++];
        } else tmp[k++] = a[i++];
      }
      while (i < mid) tmp[k++] = a[i++];
      while (j < hi) tmp[k++] = a[j++];
      for (let m = lo; m < hi; m++) a[m] = tmp[m];
    }
  }
  return count;
}

/**
 * The layered layout of `nodes` (`[{id, w, h}]`, in source order) and
 * `edges` (`[{key, from, to, fromFrac, toFrac, fromOff, toOff, net}]`,
 * pointing down the layers, back edges already reversed; `*Frac` a pin's
 * row on its box in [0, 1), `*Off` its height from the box's top) on
 * layers `layer` (id → layer). Returns `{rows, x, y, columns, chains,
 * crossings}`: `rows` each layer's ids (dummies included) in order, `x`
 * and `y` each real node's corner, `columns` each layer's `{x, w}`,
 * `chains` each edge key's dummies `[{layer, y}]` (the y its wire runs
 * at through that column), `crossings` of the order kept (between layers,
 * dummies counted).
 */
export function sugiyama(nodes, edges, layer) {
  const real = new Map(nodes.map((n) => [n.id, n]));
  const count = nodes.length ? Math.max(...nodes.map((n) => layer.get(n.id) ?? 0)) + 1 : 0;
  const rows = Array.from({ length: count }, () => []);
  for (const n of nodes) rows[layer.get(n.id) ?? 0].push(n.id);
  const lay = new Map(layer);
  // Segments between consecutive layers: gap g joins layer g and g + 1.
  const gaps = Array.from({ length: Math.max(0, count - 1) }, () => []);
  const chains = new Map();
  let dummies = 0;
  for (const e of edges) {
    const a = lay.get(e.from);
    const b = lay.get(e.to);
    if (a === undefined || b === undefined || b <= a) continue;
    const chain = [];
    let prev = e.from;
    let prevFrac = e.fromFrac ?? 0.5;
    let prevOff = e.fromOff ?? 0;
    for (let l = a + 1; l < b; l++) {
      const id = `${DUMMY}${dummies++}`;
      rows[l].push(id);
      lay.set(id, l);
      chain.push(id);
      gaps[l - 1].push({ up: prev, upFrac: prevFrac, upOff: prevOff, lo: id, loFrac: 0.5, loOff: 0, net: e.net });
      prev = id;
      prevFrac = 0.5;
      prevOff = 0;
    }
    gaps[b - 1].push({ up: prev, upFrac: prevFrac, upOff: prevOff, lo: e.to, loFrac: e.toFrac ?? 0.5, loOff: e.toOff ?? 0, net: e.net, key: e.key });
    chains.set(e.key, chain);
  }
  // Neighbours per element, above and below, with the port fractions.
  const above = new Map();
  const below = new Map();
  for (const row of rows) for (const id of row) {
    above.set(id, []);
    below.set(id, []);
  }
  for (const segs of gaps) {
    for (const s of segs) {
      below.get(s.up).push({ other: s.lo, own: s.upFrac, frac: s.loFrac });
      above.get(s.lo).push({ other: s.up, own: s.loFrac, frac: s.upFrac });
    }
  }
  const index = new Map();
  const number = () => rows.forEach((row) => row.forEach((id, i) => index.set(id, i)));
  number();
  const at = (n) => index.get(n.other) + n.frac;
  const gapCrossings = (g) => {
    const segs = gaps[g].map((s) => [index.get(s.up) + s.upFrac, index.get(s.lo) + s.loFrac]);
    segs.sort((p, q) => p[0] - q[0] || p[1] - q[1]);
    return inversions(segs.map((s) => s[1]));
  };
  const total = () => {
    let c = 0;
    for (let g = 0; g < gaps.length; g++) c += gapCrossings(g);
    return c;
  };
  const median = (list) => {
    const v = list.map(at).sort((p, q) => p - q);
    const m = Math.floor(v.length / 2);
    return v.length % 2 ? v[m] : (v[m - 1] + v[m]) / 2;
  };
  const sweep = (r, fromAbove) => {
    const row = rows[r];
    const near = fromAbove ? above : below;
    // Elements without neighbours on that side keep their slots; the others are sorted by median into the rest.
    const moving = [];
    const slots = [];
    row.forEach((id, i) => {
      const list = near.get(id);
      if (list.length) {
        moving.push({ id, key: median(list), i });
        slots.push(i);
      }
    });
    moving.sort((p, q) => p.key - q.key || p.i - q.i);
    const next = [...row];
    slots.forEach((slot, k) => {
      next[slot] = moving[k].id;
    });
    rows[r] = next;
    next.forEach((id, i) => index.set(id, i));
  };
  // Crossings between u's and w's segments (to both neighbouring layers), u before w.
  const pairCrossings = (u, w) => {
    let c = 0;
    for (const near of [above, below]) {
      const pu = near.get(u).map(at);
      const pw = near.get(w).map(at);
      for (const a of pu) for (const b of pw) if (a > b) c++;
    }
    return c;
  };
  const transpose = () => {
    for (let pass = 0; pass < TRANSPOSE_PASSES; pass++) {
      let improved = false;
      for (const row of rows) {
        for (let j = 0; j + 1 < row.length; j++) {
          const u = row[j];
          const w = row[j + 1];
          if (pairCrossings(w, u) < pairCrossings(u, w)) {
            row[j] = w;
            row[j + 1] = u;
            index.set(w, j);
            index.set(u, j + 1);
            improved = true;
          }
        }
      }
      if (!improved) break;
    }
  };
  let best = { crossings: total(), rows: rows.map((r) => [...r]) };
  for (let s = 0; s < SWEEPS && best.crossings > 0; s++) {
    if (s % 2 === 0) for (let r = 1; r < count; r++) sweep(r, true);
    else for (let r = count - 2; r >= 0; r--) sweep(r, false);
    transpose();
    const c = total();
    if (c < best.crossings) best = { crossings: c, rows: rows.map((r) => [...r]) };
  }
  for (let r = 0; r < count; r++) rows[r] = best.rows[r];
  number();

  // Coordinates: y within each column (isotonic regression of the wanted
  // tops under the order and the gaps), then the columns' x.
  const height = (id) => real.get(id)?.h ?? 0;
  const separation = (p, q) => (isDummy(p) && isDummy(q) ? TRACK : isDummy(p) || isDummy(q) ? GAP_Y / 2 : GAP_Y);
  const y = new Map();
  for (const row of rows) {
    let top = 0;
    row.forEach((id, i) => {
      if (i > 0) top += height(row[i - 1]) + separation(row[i - 1], id);
      y.set(id, top);
    });
  }
  // Pin offsets per element and neighbour, gathered from the edges once.
  const offUp = new Map();
  const offLo = new Map();
  for (const e of edges) {
    const chain = chains.get(e.key);
    if (!chain) continue;
    const ids = [e.from, ...chain, e.to];
    for (let k = 0; k + 1 < ids.length; k++) {
      const u = ids[k];
      const v = ids[k + 1];
      const uo = k === 0 ? (e.fromOff ?? height(u) / 2) : 0;
      const vo = k + 1 === ids.length - 1 ? (e.toOff ?? height(v) / 2) : 0;
      if (!offLo.has(v)) offLo.set(v, []);
      if (!offUp.has(u)) offUp.set(u, []);
      // v wants its top at y(u) + uo - vo; u wants y(v) + vo - uo.
      offLo.get(v).push({ other: u, delta: uo - vo });
      offUp.get(u).push({ other: v, delta: vo - uo });
    }
  }
  const place = (r, wanted) => {
    const row = rows[r];
    if (!row.length) return;
    // z_k = y_k − O_k, nondecreasing; targets t_k = wanted_k − O_k.
    const O = [];
    let o = 0;
    row.forEach((id, i) => {
      if (i > 0) o += height(row[i - 1]) + separation(row[i - 1], id);
      O.push(o);
    });
    const blocks = [];
    row.forEach((id, i) => {
      const { value, weight } = wanted(id);
      blocks.push({ sum: (value - O[i]) * weight, weight, n: 1 });
      while (blocks.length > 1) {
        const b = blocks[blocks.length - 1];
        const a = blocks[blocks.length - 2];
        if (a.sum / a.weight <= b.sum / b.weight) break;
        blocks.splice(blocks.length - 2, 2, { sum: a.sum + b.sum, weight: a.weight + b.weight, n: a.n + b.n });
      }
    });
    let i = 0;
    for (const b of blocks) {
      const z = b.sum / b.weight;
      for (let k = 0; k < b.n; k++, i++) y.set(row[i], z + O[i]);
    }
  };
  const want = (lists) => (id) => {
    const list = lists.get(id) ?? [];
    if (!list.length) return { value: y.get(id), weight: STAY_WEIGHT };
    const v = list.map((n) => y.get(n.other) + n.delta).sort((p, q) => p - q);
    const m = Math.floor(v.length / 2);
    const value = v.length % 2 ? v[m] : (v[m - 1] + v[m]) / 2;
    return { value, weight: isDummy(id) ? DUMMY_WEIGHT : 1 };
  };
  for (let p = 0; p < PASSES; p++) {
    for (let r = 1; r < count; r++) place(r, want(offLo));
    for (let r = count - 2; r >= 0; r--) place(r, want(offUp));
  }
  // Whole units, the topmost at 0.
  let top = Infinity;
  for (const v of y.values()) top = Math.min(top, v);
  if (!Number.isFinite(top)) top = 0;
  for (const [id, v] of y) y.set(id, Math.round(v - top));

  // Columns: each as wide as its widest box; each channel as wide as the
  // vertical tracks crossing it need (one per net and upper end).
  const columns = [];
  let x = 0;
  for (let r = 0; r < count; r++) {
    let w = 0;
    for (const id of rows[r]) w = Math.max(w, real.get(id)?.w ?? 0);
    columns.push({ x, w });
    if (r + 1 < count) {
      const tracks = new Set();
      for (const s of gaps[r]) {
        if (y.get(s.up) + s.upOff !== y.get(s.lo) + s.loOff) tracks.add(`${s.net}\n${s.up}`);
      }
      x += w + Math.max(MIN_CHANNEL, 2 * STUB + (tracks.size + 1) * TRACK);
    }
  }
  const xs = new Map();
  const ys = new Map();
  for (const n of nodes) {
    xs.set(n.id, columns[layer.get(n.id) ?? 0].x);
    ys.set(n.id, y.get(n.id));
  }
  const dummyAt = new Map();
  for (const [key, chain] of chains) dummyAt.set(key, chain.map((id) => ({ layer: lay.get(id), y: y.get(id) })));
  return { rows, x: xs, y: ys, columns, chains: dummyAt, crossings: best.crossings };
}

/** Whether rectangles `a` and `b` (`{x, y, w, h}`) come closer than `gap`. */
export function near(a, b, gap = 0) {
  return a.x < b.x + b.w + gap && b.x < a.x + a.w + gap && a.y < b.y + b.h + gap && b.y < a.y + a.h + gap;
}

/** The cells of spiral ring `r` (whole steps), nearest first, then by y, then by x: a fixed order. */
export function ring(r) {
  if (r === 0) return [[0, 0]];
  const cells = [];
  for (let d = -r; d <= r; d++) {
    cells.push([d, -r], [d, r]);
    if (d !== -r && d !== r) cells.push([-r, d], [r, d]);
  }
  return cells.sort((p, q) => p[0] * p[0] + p[1] * p[1] - (q[0] * q[0] + q[1] * q[1]) || p[1] - q[1] || p[0] - q[0]);
}

/**
 * Chip `chip` (`{w, h}`) placed near `vias` (its controls' rectangles):
 * its centre at their centroid, snapped to whole plate units, then moved
 * to the first spot of a fixed spiral (whole PU steps) where it comes
 * within CLEARANCE of none of `obstacles` (`[{x, y, w, h}]`). `{x, y}`;
 * null when no spot within SPIRAL_RINGS is free.
 */
export function placeAnchored(chip, vias, obstacles) {
  if (!vias.length) return null;
  let cx = 0;
  let cy = 0;
  for (const v of vias) {
    cx += v.x + v.w / 2;
    cy += v.y + v.h / 2;
  }
  cx /= vias.length;
  cy /= vias.length;
  return placeNear(chip, { x: cx - chip.w / 2, y: cy - chip.h / 2 }, obstacles);
}

/**
 * Chip `chip` (`{w, h}`) at the first spot of the fixed spiral from `at`
 * (its top-left, snapped to whole plate units) that comes within
 * CLEARANCE of none of `obstacles`: `{x, y}`, or null within SPIRAL_RINGS.
 */
export function placeNear(chip, at0, obstacles) {
  const x0 = Math.round(at0.x / PU) * PU;
  const y0 = Math.round(at0.y / PU) * PU;
  for (let r = 0; r <= SPIRAL_RINGS; r++) {
    for (const [dx, dy] of ring(r)) {
      const at = { x: x0 + dx * PU, y: y0 + dy * PU, w: chip.w, h: chip.h };
      if (!obstacles.some((o) => near(at, o, CLEARANCE))) return { x: at.x, y: at.y };
    }
  }
  return null;
}

/** The centre of `vias`' centroid: what an anchored chip is measured from (test 7). */
export function centroid(vias) {
  let x = 0;
  let y = 0;
  for (const v of vias) {
    x += v.x + v.w / 2;
    y += v.y + v.h / 2;
  }
  return { x: x / vias.length, y: y / vias.length };
}
