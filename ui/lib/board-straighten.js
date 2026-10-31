// Fewer bends where the search left a staircase (ADR 0047 §6; the note
// "Layered placement and channel routing", 2026-10-31): after a board is
// routed, each wire with more than two bends (a chip's wire, never a leg of
// the plate's ribbons nor a die's parameter, whose shapes are designed) is
// offered the simplest orthogonal shapes between its two ends, leaving and
// arriving as it does: straight, one bend, a Z over one vertical (or
// horizontal), three bends, a U over one run. The first, by fewest bends
// then shortest, that keeps clear of every box (but its own ends' at its
// ends), runs along no other wire, turns where no other wire passes, crosses
// no more wires than its route did and is not much longer, replaces the
// route. Wires are looked at by most bends first, ties by id; each change is
// seen by the next. Deterministic. No DOM.

import { STUB, TRACK } from './board-layout.js';

/** A route keeps this far from a box it passes (ui/lib/board-route.js CLEAR). */
const CLEAR = 4;
/** Coordinates tried per free axis of a shape. */
const TRIES = 16;
/** A shape may be at most this much longer than the route it replaces (and STUB × 4 more). */
const LONGER = 1.25;

const bendsOf = (c) => Math.max(0, c.length - 2);
const lengthOf = (c) => {
  let l = 0;
  for (let i = 1; i < c.length; i++) l += Math.abs(c[i].x - c[i - 1].x) + Math.abs(c[i].y - c[i - 1].y);
  return l;
};
const sign = (v) => (v > 0 ? 1 : v < 0 ? -1 : 0);
/** The heading of the segment from p to q: `{x, y}` unit. */
const heading = (p, q) => ({ x: sign(q.x - p.x), y: sign(q.y - p.y) });

/**
 * `wires` (`[{id, corners, net}]`, `net` the net crossings are counted
 * between) among `boxes` (`[{id, x, y, w, h}]`); `eligible(w)` whether a
 * wire may be straightened; `ends(w)` the ids of the boxes at its two ends.
 * Returns a Map id → new corners of the wires changed.
 */
export function straighten(wires, boxes, { eligible = () => true, ends = () => [null, null] } = {}) {
  const segs = [];
  const add = (w, c) => {
    for (let i = 1; i < c.length; i++) {
      const p = c[i - 1];
      const q = c[i];
      if (p.x === q.x && p.y === q.y) continue;
      segs.push({ id: w.id, net: w.net, h: p.y === q.y, x0: Math.min(p.x, q.x), x1: Math.max(p.x, q.x), y0: Math.min(p.y, q.y), y1: Math.max(p.y, q.y) });
    }
  };
  for (const w of wires) add(w, w.corners);
  const others = (w) => segs.filter((s) => s.id !== w.id && s.net !== w.net);
  // Crossings of corners `c` with segments `list` (each point once).
  const crossings = (c, list) => {
    const seen = new Set();
    for (let i = 1; i < c.length; i++) {
      const p = c[i - 1];
      const q = c[i];
      const h = p.y === q.y;
      const lo = h ? Math.min(p.x, q.x) : Math.min(p.y, q.y);
      const hi = h ? Math.max(p.x, q.x) : Math.max(p.y, q.y);
      for (const s of list) {
        if (s.h === h) continue;
        if (h ? s.x0 > lo && s.x0 < hi && p.y > s.y0 && p.y < s.y1 : s.y0 > lo && s.y0 < hi && p.x > s.x0 && p.x < s.x1) seen.add(h ? `${s.x0},${p.y}` : `${p.x},${s.y0}`);
      }
    }
    return seen.size;
  };
  const inBox = (p, q, box) => {
    const x0 = Math.min(p.x, q.x);
    const x1 = Math.max(p.x, q.x);
    const y0 = Math.min(p.y, q.y);
    const y1 = Math.max(p.y, q.y);
    return x0 < box.x + box.w + CLEAR && x1 > box.x - CLEAR && y0 < box.y + box.h + CLEAR && y1 > box.y - CLEAR;
  };
  // Whether corners `c` keep clear of boxes (its first and last segments of their own ends' boxes) and of `list`'s runs and points.
  const clear = (c, list, [ea, eb]) => {
    for (let i = 1; i < c.length; i++) {
      const p = c[i - 1];
      const q = c[i];
      for (const box of boxes) {
        if ((i === 1 && box.id === ea) || (i === c.length - 1 && box.id === eb)) continue;
        if (inBox(p, q, box)) return false;
      }
      const h = p.y === q.y;
      const lo = h ? Math.min(p.x, q.x) : Math.min(p.y, q.y);
      const hi = h ? Math.max(p.x, q.x) : Math.max(p.y, q.y);
      for (const s of list) {
        if (s.h !== h) continue;
        if (h ? Math.abs(s.y0 - p.y) < TRACK / 2 && s.x0 < hi - 0.5 && s.x1 > lo + 0.5 : Math.abs(s.x0 - p.x) < TRACK - 0.01 && s.y0 < hi + TRACK / 2 && s.y1 > lo - TRACK / 2) return false;
      }
    }
    // No turn on another wire (a false junction).
    for (let i = 1; i + 1 < c.length; i++) {
      const p = c[i];
      for (const s of list) if (p.x >= s.x0 - 1 && p.x <= s.x1 + 1 && p.y >= s.y0 - 1 && p.y <= s.y1 + 1) return false;
    }
    return true;
  };
  const changed = new Map();
  const order = wires.filter((w) => eligible(w) && bendsOf(w.corners) > 2).map((w) => ({ w, b: bendsOf(w.corners) }));
  order.sort((p, q) => q.b - p.b || p.w.id - q.w.id);
  for (const { w } of order) {
    const c = w.corners;
    const a = c[0];
    const b = c.at(-1);
    const dA = heading(a, c[1]);
    const dB = heading(c.at(-2), b);
    const list = others(w);
    const now = crossings(c, list);
    const most = lengthOf(c) * LONGER + 4 * STUB;
    const xs = values(c, 'x', a, b);
    const ys = values(c, 'y', a, b);
    const all = shapes(a, b, dA, dB, xs, ys, bendsOf(c))
      .map((s) => ({ s, b: bendsOf(s), l: lengthOf(s) }))
      .filter((o) => o.l <= most)
      .sort((p, q) => p.b - q.b || p.l - q.l);
    const best = all.find((o) => clear(o.s, list, ends(w)) && crossings(o.s, list) <= now);
    if (!best) continue;
    for (let i = segs.length - 1; i >= 0; i--) if (segs[i].id === w.id) segs.splice(i, 1);
    add(w, best.s);
    w.corners = best.s;
    changed.set(w.id, best.s);
  }
  return changed;
}

/** Coordinates to try on axis `k` for a wire from `a` to `b` with route `c`: its own runs', its ends', between, a few tracks off them. */
function values(c, k, a, b) {
  const out = new Set();
  for (const p of c) out.add(p[k]);
  const lo = Math.min(a[k], b[k]);
  const hi = Math.max(a[k], b[k]);
  for (let i = 0; i <= 4; i++) out.add(Math.round(lo + ((hi - lo) * i) / 4));
  for (let i = 1; i <= 4; i++) {
    out.add(a[k] + i * TRACK);
    out.add(a[k] - i * TRACK);
    out.add(b[k] + i * TRACK);
    out.add(b[k] - i * TRACK);
  }
  const mid = (a[k] + b[k]) / 2;
  return [...out].sort((p, q) => Math.abs(p - mid) - Math.abs(q - mid) || p - q);
}

/**
 * The shapes from `a` (leaving along `dA`) to `b` (arriving along `dB`)
 * with fewer than `under` bends: straight, L, Z, three bends, U; each run out
 * of a pin and into one at least STUB / 2 long, in its heading.
 */
function shapes(a, b, dA, dB, xs, ys, under) {
  const out = [];
  const stub = STUB / 2;
  const ok = (p, q, d) => (q.x - p.x) * d.x + (q.y - p.y) * d.y >= stub && (d.x ? p.y === q.y : p.x === q.x);
  const hA = dA.x !== 0;
  const hB = dB.x !== 0;
  if (hA === hB && (hA ? a.y === b.y : a.x === b.x) && ok(a, b, dA) && dA.x === dB.x && dA.y === dB.y) out.push([a, b]);
  if (under > 1 && hA !== hB) {
    const p = hA ? { x: b.x, y: a.y } : { x: a.x, y: b.y };
    if (ok(a, p, dA) && ok(p, b, dB)) out.push([a, p, b]);
  }
  // Values on an axis whose run from `from` (along `d`, on that axis) is at least a stub, nearest `from` first.
  const ahead = (list, from, d) => list.filter((v) => (v - from) * d >= stub).sort((p, q) => Math.abs(p - from) - Math.abs(q - from) || p - q);
  const along = (d) => (d.x ? d.x : d.y);
  const ax = (p) => (hA ? p.x : p.y);
  if (under > 2 && hA === hB && dA.x === dB.x && dA.y === dB.y) {
    const lane = hA ? xs : ys;
    for (const v of lane.filter((v) => (v - ax(a)) * along(dA) >= stub && (ax(b) - v) * along(dB) >= stub).slice(0, TRIES)) {
      const p = hA ? { x: v, y: a.y } : { x: a.x, y: v };
      const q = hA ? { x: v, y: b.y } : { x: b.x, y: v };
      out.push([a, p, q, b]);
    }
  }
  if (under > 3 && hA !== hB) {
    // Out along dA to u, across to v, along dA's axis to b's line, into b.
    const us = ahead(hA ? xs : ys, hA ? a.x : a.y, along(dA)).slice(0, TRIES / 2);
    const vs = (hA ? ys : xs).filter((v) => ((hA ? b.y : b.x) - v) * along(dB) >= stub).slice(0, TRIES / 2);
    for (const u of us) {
      for (const v of vs) {
        const p = hA ? { x: u, y: a.y } : { x: a.x, y: u };
        const q = hA ? { x: u, y: v } : { x: v, y: u };
        const r = hA ? { x: b.x, y: v } : { x: v, y: b.y };
        if ((hA ? p.y !== q.y : p.x !== q.x) && (hA ? q.x !== r.x : q.y !== r.y)) out.push([a, p, q, r, b]);
      }
    }
  }
  if (under > 4 && hA === hB && dA.x === dB.x && dA.y === dB.y) {
    const us = ahead(hA ? xs : ys, hA ? a.x : a.y, along(dA)).slice(0, TRIES / 4);
    const ts = ahead(hA ? xs : ys, hA ? b.x : b.y, -along(dB)).slice(0, TRIES / 4);
    const vs = (hA ? ys : xs).slice(0, TRIES);
    for (const u of us) {
      for (const t of ts) {
        if (u === t) continue;
        for (const v of vs) {
          if (hA ? v === a.y || v === b.y : v === a.x || v === b.x) continue;
          const p = hA ? { x: u, y: a.y } : { x: a.x, y: u };
          const q = hA ? { x: u, y: v } : { x: v, y: u };
          const r = hA ? { x: t, y: v } : { x: v, y: t };
          const s2 = hA ? { x: t, y: b.y } : { x: b.x, y: t };
          out.push([a, p, q, r, s2, b]);
        }
      }
    }
  }
  return out;
}
