// Channel routing of a layered board (ADR 0047 §6.2; the note "Layered
// placement and channel routing", 2026-10-31): every wire of a level laid
// out in flow columns (a die, a board without a plate) is planned in the
// columns' terms before any x is known, so that each channel can be made as
// wide as its tracks need and every vertical gets a track of its own:
// - a forward wire between neighbouring columns is H-V-H, its vertical on a
//   track of the channel between them (at most two bends);
// - a wire skipping columns runs along one free horizontal corridor across
//   the columns between (its source's pin height, its target's, else the
//   height its dummies were given), two bends, at most four;
// - feedback (a back edge) goes round the outside of the drawing, above or
//   below, each on an outer track of its own, never through the middle;
// - a die's parameter drops from its strip along a band under the strip,
//   straight into a top pin when nothing stands above it in its column,
//   else down the channel before its column and into its pin from a gap
//   kept above its chip;
// - the wires of one net share their vertical in a channel (a trunk), so a
//   fan-out is a tree.
// The tracks of a channel are ordered against crossings (each pair's
// crossings counted from where their horizontals attach, the better of the
// two orders of the groups going up and down, then neighbouring swaps).
// The planner only proposes: each route is checked against every box and
// every wire taken before it, and a wire whose plan fails is routed as
// before (ui/lib/board-route.js, the maze). Deterministic: wires in a fixed
// order, ties by their index, fixed pass counts. No DOM.

import { STUB, TRACK, MIN_CHANNEL } from './board-layout.js';

/** A route keeps this far from a box (ui/lib/board-route.js CLEAR) and a little more. */
const CORRIDOR_CLEAR = 6;
/** Far above or below everything: where an outer track's height stands in for ordering. */
const FAR = 1e7;
/** Neighbouring swaps per channel: at most this many passes. */
const SWAP_PASSES = 12;

/** The slot of channel `c` (between columns c and c + 1; -1 the left margin) among the columns' slots (column c is 2c + 1). */
const chSlot = (c) => 2 * c + 2;
const colSlot = (c) => 2 * c + 1;

/**
 * The plan of every wire. `count` columns; `colOf(id)` a node's column, or
 * null when it stands outside them (stored, moved); `spans[c]` the boxes of
 * column c as `[{y0, y1}]`; `wires` `[{id, net, from, to, ya, yb, feedback,
 * chain}]` (`ya` its output pin's height, `yb` its input's, `chain` its
 * dummies' `[{layer, y}]`); `params` `[{id, net, to, xIn, top, gap}]` a
 * die's parameter wires (`top` true when nothing stands above its chip in
 * its column, `gap` the height its horizontal takes above its chip when
 * not, `xIn` how far right of its column its pin lies, for ordering);
 * `side(w)` 'above' or 'below' for a feedback wire; `stripX` an estimate
 * of the strip's x in column units (for the band's attachment sides).
 * Returns `{plans, channels, outer}`: `plans` a Map id → `{corners}`
 * (symbolic corners, `resolve` makes them points), `channels` a Map
 * channel → `{groups, n}` (`groups` in track order), `outer` the number of
 * outer tracks above and below.
 */
export function planChannels({ count, colOf, spans, wires, params = [], side = () => 'below', yMin = -Infinity }) {
  const plans = new Map();
  const verts = [];
  const claims = new Map();
  // Horizontal runs taken, by slot: a run of another net within half a track is in the way (but the runs into one pin).
  let into = null;
  const claim = (s0, s1, y, net) => {
    for (let s = Math.min(s0, s1); s <= Math.max(s0, s1); s++) {
      if (!claims.has(s)) claims.set(s, []);
      claims.get(s).push({ y, net, into });
    }
  };
  const claimFree = (s0, s1, y, net) => {
    for (let s = Math.min(s0, s1); s <= Math.max(s0, s1); s++) for (const o of claims.get(s) ?? []) if (o.net !== net && !(into && o.into === into) && Math.abs(o.y - y) < TRACK / 2) return false;
    return true;
  };
  const columnFree = (c, y) => (spans[c] ?? []).every((b) => y < b.y0 - CORRIDOR_CLEAR || y > b.y1 + CORRIDOR_CLEAR);
  // A corridor at height y across columns c0..c1 (inclusive; none when c1 < c0), free of boxes and other nets.
  const corridor = (c0, c1, y, net, s0, s1) => {
    if (y < yMin) return false;
    for (let c = c0; c <= c1; c++) if (!columnFree(c, y)) return false;
    return claimFree(s0, s1, y, net);
  };
  // The verticals of different nets into one input pin share a track (`key`): they may run on one another into it (a fan-in,
  // which ADR 0054 refuses since phase 2).
  const nets = new Map();
  for (const w of wires) if (w.into) (nets.get(w.into) ?? nets.set(w.into, new Set()).get(w.into)).add(w.net);
  const vert = (ch, y0, y1, net, ends, last = false) => {
    const v = { ch, y0: Math.min(y0, y1), y1: Math.max(y0, y1), net, ends };
    if (last && into && nets.get(into)?.size > 1) v.key = `into:${into}`;
    verts.push(v);
    return v;
  };
  // A net's corridors: its other wires take the same first.
  const netYs = new Map();
  const took = (net, y) => (netYs.get(net) ?? netYs.set(net, []).get(net)).push(y);
  const A = { pin: 'a' };
  const B = { pin: 'b' };
  const order = wires.map((w, i) => ({ w, i, l: colOf(w.from), m: colOf(w.to) }));
  const span = (o) => (o.w.feedback ? 1e6 : 0) + Math.abs((o.m ?? 0) - (o.l ?? 0)) * 1e3 + Math.abs(o.w.ya - o.w.yb);
  order.sort((p, q) => span(p) - span(q) || p.i - q.i);
  // Every wire's ends first: the stub out of its source to its channel, and into its target from its channel.
  for (const { w, l, m } of order) {
    if (l === null || m === null || l === undefined || m === undefined) continue;
    into = w.into ?? null;
    claim(colSlot(l), chSlot(l), w.ya, w.net);
    claim(chSlot(m - 1), colSlot(m), w.yb, w.net);
  }
  for (const { w, l, m } of order) {
    if (l === null || m === null || l === undefined || m === undefined) continue;
    const { net, ya, yb } = w;
    into = w.into ?? null;
    if (w.feedback || m <= l) {
      // Round the outside: out to the channel after its source, along an outer track, down the channel before its target.
      const where = side(w);
      const far = where === 'above' ? -FAR : FAR;
      const out = { side: where, s0: chSlot(m - 1), s1: chSlot(l), track: null };
      const v1 = vert(l, ya, far, net, [{ y: ya, dir: 'L' }, { y: far, dir: 'L' }]);
      const v2 = vert(m - 1, far, yb, net, [{ y: far, dir: 'R' }, { y: yb, dir: 'R' }], true);
      claim(colSlot(l), chSlot(l), ya, net);
      claim(chSlot(m - 1), colSlot(m), yb, net);
      plans.set(w.id, { outer: out, corners: [A, { x: { v: v1 }, y: ya }, { x: { v: v1 }, y: { outer: out } }, { x: { v: v2 }, y: { outer: out } }, { x: { v: v2 }, y: yb }, B] });
      continue;
    }
    if (m === l + 1) {
      if (Math.abs(ya - yb) < 0.5 && claimFree(colSlot(l), colSlot(m), ya, net)) {
        claim(colSlot(l), colSlot(m), ya, net);
        plans.set(w.id, { corners: [A, B] });
        continue;
      }
      const v = vert(l, ya, yb, net, [{ y: ya, dir: 'L' }, { y: yb, dir: 'R' }], true);
      claim(colSlot(l), chSlot(l), ya, net);
      claim(chSlot(l), colSlot(m), yb, net);
      plans.set(w.id, { corners: [A, { x: { v }, y: ya }, { x: { v }, y: yb }, B] });
      continue;
    }
    // A long wire: one corridor across the columns between.
    if (Math.abs(ya - yb) < 0.5 && corridor(l + 1, m - 1, ya, net, colSlot(l), colSlot(m))) {
      claim(colSlot(l), colSlot(m), ya, net);
      plans.set(w.id, { corners: [A, B] });
      continue;
    }
    if (corridor(l + 1, m - 1, ya, net, colSlot(l), chSlot(m - 1))) {
      const v = vert(m - 1, ya, yb, net, [{ y: ya, dir: 'L' }, { y: yb, dir: 'R' }], true);
      claim(colSlot(l), chSlot(m - 1), ya, net);
      claim(chSlot(m - 1), colSlot(m), yb, net);
      plans.set(w.id, { corners: [A, { x: { v }, y: ya }, { x: { v }, y: yb }, B] });
      continue;
    }
    if (corridor(l + 1, m - 1, yb, net, chSlot(l), colSlot(m))) {
      const v = vert(l, ya, yb, net, [{ y: ya, dir: 'L' }, { y: yb, dir: 'R' }], true);
      claim(colSlot(l), chSlot(l), ya, net);
      claim(chSlot(l), colSlot(m), yb, net);
      plans.set(w.id, { corners: [A, { x: { v }, y: ya }, { x: { v }, y: yb }, B] });
      continue;
    }
    // Its dummies' heights, the most common first, then the nearest its ends' middle.
    const counts = new Map();
    for (const d of w.chain ?? []) counts.set(d.y, (counts.get(d.y) ?? 0) + 1);
    const mid = (ya + yb) / 2;
    const ys = [...counts.keys()].sort((p, q) => counts.get(q) - counts.get(p) || Math.abs(p - mid) - Math.abs(q - mid) || p - q);
    // Past its dummies: beside the boxes of the columns between, the least way up and down first.
    const more = [];
    for (let c = l + 1; c <= m - 1; c++) {
      for (const b of spans[c] ?? []) for (let k = 1; k <= 3; k++) more.push(Math.round(b.y0 - CORRIDOR_CLEAR - k * TRACK), Math.round(b.y1 + CORRIDOR_CLEAR + k * TRACK));
    }
    const cost = (y) => Math.abs(y - ya) + Math.abs(y - yb);
    more.sort((p, q) => cost(p) - cost(q) || p - q);
    const y = [...(netYs.get(net) ?? []), ...ys, ...more].find((c) => corridor(l + 1, m - 1, c, net, chSlot(l), chSlot(m - 1)));
    if (y !== undefined) {
      took(net, y);
      const v1 = vert(l, ya, y, net, [{ y: ya, dir: 'L' }, { y, dir: 'R' }]);
      const v2 = vert(m - 1, y, yb, net, [{ y, dir: 'L' }, { y: yb, dir: 'R' }], true);
      claim(colSlot(l), chSlot(l), ya, net);
      claim(chSlot(l), chSlot(m - 1), y, net);
      claim(chSlot(m - 1), colSlot(m), yb, net);
      plans.set(w.id, { corners: [A, { x: { v: v1 }, y: ya }, { x: { v: v1 }, y }, { x: { v: v2 }, y }, { x: { v: v2 }, y: yb }, B] });
      continue;
    }
    // Station by station along its dummies (each free in its column by the layout).
    const chain = [...(w.chain ?? [])].sort((p, q) => p.layer - q.layer);
    if (chain.length !== m - l - 1) continue;
    const corners = [A];
    let at = ya;
    for (let k = 0; k <= chain.length; k++) {
      const c = l + k;
      const next = k < chain.length ? chain[k].y : yb;
      if (Math.abs(next - at) >= 0.5) {
        const v = vert(c, at, next, net, [{ y: at, dir: 'L' }, { y: next, dir: 'R' }], k === chain.length);
        corners.push({ x: { v }, y: at }, { x: { v }, y: next });
      }
      at = next;
    }
    for (let k = 0; k < chain.length; k++) claim(colSlot(l + k + 1), colSlot(l + k + 1), chain[k].y, net);
    corners.push(B);
    plans.set(w.id, { corners });
  }
  // A die's parameters: down from the strip to the band, then straight into a free top pin, or down the channel before its column.
  // Into a chip not at the top of its column: down the channel before or after its column, whichever its drops cross
  // fewer wires in (the ends attached there above its gap), every drop into one chip on one side; its top pins' horizontals
  // in the gap above it, the one entering farthest lowest (none crosses another's drop).
  // A drop down channel ch to height y crosses, on the best track, the lesser of the wires attached there from the left and
  // from the right above y, and every wire passing through the channel above y.
  const ends = new Map();
  const add = (ch, key, y) => {
    if (!ends.has(ch)) ends.set(ch, { L: [], R: [], through: [] });
    ends.get(ch)[key].push(y);
  };
  for (const v of verts) for (const e of v.ends) if (Math.abs(e.y) < FAR / 4) add(v.ch, e.dir, e.y);
  for (const [s, list] of claims) {
    if (s % 2) continue;
    const ch = s / 2 - 1;
    for (const o of list) if (!verts.some((v) => v.ch === ch && v.net === o.net)) add(ch, 'through', o.y);
  }
  const crossed = (ch, y) => {
    const e = ends.get(ch);
    if (!e) return 0;
    const above = (list) => list.filter((v) => v < y).length;
    return Math.min(above(e.L), above(e.R)) + above(e.through);
  };
  const sideOf = new Map();
  for (const p of params) {
    const m = colOf(p.to);
    if (m === null || m === undefined || (p.top && p.side !== 'left')) continue;
    if (!sideOf.has(p.to)) sideOf.set(p.to, { m, left: 0, right: 0, top: [] });
    const c = sideOf.get(p.to);
    const y = p.side === 'left' ? p.yIn : p.chipY;
    c.left += crossed(m - 1, y);
    c.right += p.side === 'left' ? 1e6 : crossed(m, y);
    if (p.side !== 'left') c.top.push(p);
  }
  for (const c of sideOf.values()) {
    c.ch = c.right < c.left ? c.m : c.m - 1;
    const fromLeft = c.ch < c.m;
    const nets = [...new Set(c.top.map((p) => p.net))];
    const xOfNet = (net) => Math.min(...c.top.filter((p) => p.net === net).map((p) => p.pinDx));
    nets.sort((a, b) => (fromLeft ? xOfNet(a) - xOfNet(b) : xOfNet(b) - xOfNet(a)) || (a < b ? -1 : 1));
    c.gap = new Map(nets.map((net, k) => [net, c.top[0].chipY - (k + 1) * TRACK]));
  }
  for (const p of params) {
    const m = colOf(p.to);
    if (m === null || m === undefined) continue;
    const band = { band: p.net };
    const S = { pin: 'a' };
    const P = { pin: 'b' };
    if (p.top && p.side !== 'left') {
      plans.set(p.id, { param: true, corners: [S, { x: { pin: 'a' }, y: band }, { x: { pin: 'b' }, y: band }, P] });
      continue;
    }
    const c = sideOf.get(p.to);
    const ch = c.ch;
    const gap = p.side === 'left' ? p.yIn : c.gap.get(p.net);
    const dir = p.fromLeft === undefined ? 'L' : p.fromLeft(ch) ? 'L' : 'R';
    const v = vert(ch, -FAR / 2, gap, `${p.net}`, [{ y: -FAR / 2, dir }, { y: gap, dir: ch < m ? 'R' : 'L' }]);
    v.param = true;
    // Into a side pin: along its height from the channel; into a top pin: from the gap above its chip, then down.
    const tail = p.side === 'left' ? [P] : [{ x: { pin: 'b' }, y: gap }, P];
    plans.set(p.id, { param: true, corners: [S, { x: { pin: 'a' }, y: band }, { x: { v }, y: band }, { x: { v }, y: gap }, ...tail] });
  }
  // Each channel's tracks: a net's verticals one group (a trunk), the groups ordered against crossings.
  const channels = new Map();
  const byCh = new Map();
  for (const v of verts) {
    if (!byCh.has(v.ch)) byCh.set(v.ch, []);
    byCh.get(v.ch).push(v);
  }
  for (const ch of [...byCh.keys()].sort((a, b) => a - b)) {
    const groups = [];
    const byNet = new Map();
    for (const v of byCh.get(ch)) {
      const key = v.key ?? v.net;
      let g = byNet.get(key);
      if (!g) {
        g = { net: key, y0: v.y0, y1: v.y1, ends: [], verts: [], first: groups.length };
        byNet.set(key, g);
        groups.push(g);
      }
      g.y0 = Math.min(g.y0, v.y0);
      g.y1 = Math.max(g.y1, v.y1);
      g.ends.push(...v.ends);
      g.verts.push(v);
    }
    const ordered = orderTracks(groups);
    ordered.forEach((g, k) => {
      for (const v of g.verts) {
        v.track = k;
        v.group = g;
      }
    });
    channels.set(ch, { groups: ordered, n: ordered.length });
  }
  // Outer tracks: the shortest spans innermost (left edge over their slots), so a nested loop never crosses the loop round it.
  const outer = { above: 0, below: 0 };
  for (const where of ['above', 'below']) {
    const loops = [...plans.values()].map((p) => p.outer).filter((o) => o && o.side === where);
    loops.sort((p, q) => p.s1 - p.s0 - (q.s1 - q.s0) || p.s0 - q.s0);
    const tracks = [];
    for (const o of loops) {
      let k = 0;
      while (tracks[k]?.some((t) => t.s0 <= o.s1 && o.s0 <= t.s1)) k++;
      (tracks[k] ??= []).push(o);
      o.track = k;
    }
    outer[where] = tracks.length;
  }
  return { plans, channels, outer };
}

/** How many crossings group `a` on a track left of group `b` makes with it in one channel. */
function pairCost(a, b) {
  let c = 0;
  for (const e of a.ends) if (e.dir === 'R' && e.y > b.y0 && e.y < b.y1) c++;
  for (const e of b.ends) if (e.dir === 'L' && e.y > a.y0 && e.y < a.y1) c++;
  return c;
}

/**
 * Groups of a channel in track order (left to right) against crossings:
 * those going up (attached on the left below where they leave right) left
 * to right as their start goes down, those going down right to left as
 * theirs does, the better of the two orders of these sets, then
 * neighbouring swaps while one makes fewer crossings, then sifting.
 */
export function orderTracks(groups) {
  const startOf = (g) => {
    const l = g.ends.filter((e) => e.dir === 'L');
    return l.length ? l.reduce((s, e) => s + e.y, 0) / l.length : (g.y0 + g.y1) / 2;
  };
  const endOf = (g) => {
    const r = g.ends.filter((e) => e.dir === 'R');
    return r.length ? r.reduce((s, e) => s + e.y, 0) / r.length : (g.y0 + g.y1) / 2;
  };
  const down = groups.filter((g) => endOf(g) > startOf(g)).sort((p, q) => startOf(q) - startOf(p) || p.first - q.first);
  const up = groups.filter((g) => !(endOf(g) > startOf(g))).sort((p, q) => startOf(p) - startOf(q) || p.first - q.first);
  const total = (list) => {
    let c = 0;
    for (let i = 0; i < list.length; i++) for (let j = i + 1; j < list.length; j++) c += pairCost(list[i], list[j]);
    return c;
  };
  const a = [...up, ...down];
  const b = [...down, ...up];
  const list = total(b) < total(a) ? b : a;
  for (let pass = 0; pass < SWAP_PASSES; pass++) {
    let changed = false;
    for (let i = 0; i + 1 < list.length; i++) {
      if (pairCost(list[i + 1], list[i]) < pairCost(list[i], list[i + 1])) {
        [list[i], list[i + 1]] = [list[i + 1], list[i]];
        changed = true;
      }
    }
    if (!changed) break;
  }
  // Then each group to its best track (`siftBand`, as a die's band; 2026-11-02): neighbouring swaps stop in local orders.
  return siftBand(list, pairCost);
}

/** A channel holding `n` tracks is this wide (at least MIN_CHANNEL; a margin, `edge`, only as wide as its tracks need). */
export function channelWidth(n, edge = false) {
  if (edge) return n ? STUB + (n + 1) * TRACK : 0;
  return Math.max(MIN_CHANNEL, 2 * STUB + (n + 1) * TRACK);
}

/**
 * The x of track `k` of channel `ch` holding `n` tracks, the columns
 * `columns` (`[{x, w}]`, absolute) and the channel's width `width`: centred
 * in the channel, TRACK apart, on even units; a margin's packed against its
 * column.
 */
export function trackX(columns, ch, k, n, width) {
  const last = columns.length - 1;
  if (ch < 0) return columns[0].x - STUB - (n - k) * TRACK;
  if (ch >= last) return columns[last].x + columns[last].w + STUB + (k + 1) * TRACK;
  const left = columns[ch].x + columns[ch].w;
  const slack = width - (2 * STUB + (n + 1) * TRACK);
  return Math.round((left + STUB + slack / 2 + (k + 1) * TRACK) / 2) * 2;
}

/**
 * Symbolic corners made points: `{pin: 'a'|'b'}` the wire's ends, `{v}` a
 * vertical's track x (`xOf(v)`), `{outer}` an outer track's height
 * (`outerY(o)`), `{band: net}` a parameter's band height (`bandY(net)`).
 */
export function resolve(corners, { a, b, xOf, outerY = () => 0, bandY = () => 0 }) {
  const X = (x) => (typeof x === 'number' ? x : x.pin === 'a' ? a.x : x.pin === 'b' ? b.x : xOf(x.v));
  const Y = (y) => (typeof y === 'number' ? y : y.outer ? outerY(y.outer) : y.band !== undefined ? bandY(y.band) : 0);
  return corners.map((c) => (c.pin === 'a' ? { x: a.x, y: a.y } : c.pin === 'b' ? { x: b.x, y: b.y } : { x: X(c.x), y: Y(c.y) }));
}

/**
 * The order of a die's parameter nets on its band (top to bottom) against
 * crossings among themselves: `items` `[{net, xs, xe: [x]}]` (`xs` its
 * strip pin's x, `xe` where it leaves the band downwards). A pair's
 * crossings: the lower one's drop from the strip through the upper one's
 * run, the upper one's drops through the lower one's run. Neighbouring
 * swaps from the order of the runs' lengths. Returns the nets in order.
 */
export function bandOrder(items) {
  const range = (it) => [Math.min(it.xs, ...it.xe), Math.max(it.xs, ...it.xe)];
  const inside = (x, r) => x > r[0] && x < r[1];
  // i above j.
  const cost = (i, j) => {
    let c = 0;
    if (inside(j.xs, range(i))) c++;
    for (const x of i.xe) if (inside(x, range(j))) c++;
    // The lower one's drop from the strip would run along the upper one's drop: never.
    for (const x of i.xe) if (Math.abs(x - j.xs) < TRACK) c += 100;
    return c;
  };
  const list = [...items].sort((p, q) => range(p)[1] - range(p)[0] - (range(q)[1] - range(q)[0]) || p.xs - q.xs);
  for (let pass = 0; pass < 4 * SWAP_PASSES; pass++) {
    let changed = false;
    for (let i = 0; i + 1 < list.length; i++) {
      if (cost(list[i + 1], list[i]) < cost(list[i], list[i + 1])) {
        [list[i], list[i + 1]] = [list[i + 1], list[i]];
        changed = true;
      }
    }
    if (!changed) break;
  }
  return siftBand(list, cost).map((it) => it.net);
}

/**
 * Sifting (the Deck's nesting, ui/lib/plane-control.js `legLanes`, for
 * nets with several drops; the owner, 2026-11-02: fewer crossings among
 * the legs): each net in turn taken out of `list` (top to bottom) and put
 * back where its crossings with all the others (`cost(above, below)`) are
 * fewest, the first such place on a tie, until a pass moves none (at most
 * SWAP_PASSES). Neighbouring swaps alone stop in a local order where a
 * net's best place is several lanes away. Deterministic.
 */
export function siftBand(list, cost) {
  const order = [...list];
  const n = order.length;
  for (let pass = 0; pass < SWAP_PASSES; pass++) {
    let moved = false;
    for (const item of [...order]) {
      const at = order.indexOf(item);
      order.splice(at, 1);
      // Placed at p: every one above it (order[0 .. p-1]) over it, every one below under it.
      let below = 0;
      for (const o of order) below += cost(item, o);
      let best = below;
      let bestAt = 0;
      let here = below;
      let current = below;
      for (let p = 1; p <= order.length; p++) {
        const o = order[p - 1];
        here += cost(o, item) - cost(item, o);
        if (p === at) current = here;
        if (here < best) {
          best = here;
          bestAt = p;
        }
      }
      // Kept where it was unless the new place is strictly better.
      const to = current <= best ? at : bestAt;
      order.splice(to, 0, item);
      if (to !== at) moved = true;
    }
    if (!moved) break;
  }
  return order;
}
