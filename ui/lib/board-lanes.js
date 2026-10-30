// Lanes for wires that share a way (ADR 0047 phase 5; the owner: "bundle
// the synth's knobs' legs into ribbon cables: now they fall on top of each
// other unreadably"). The maze lets a bus (a chip's legs from its plate's
// vias, a die's parameters from its strip) share its way, and a wire that
// found no other way may run along another net: their routes then lie on
// one line. Drawn, each net on a shared stretch gets its own lane, LANE
// apart across it, so no two nets are ever drawn on top of each other: a
// shared stretch of three nets or more is a ribbon (a band under its lanes,
// labelled once, `params ×24` for legs), and a net joining or leaving it
// steps across at 45°. The lanes are ordered so the members cross as
// little as the ways allow (the classic order of lines sharing a track:
// the one that turns off to the right, or came in from the right, runs on
// the right). The routes are unchanged; this is how they are drawn
// (`display`). Deterministic: nets ordered by a fixed comparison, ties by
// name. No DOM.

/** Lanes on a shared stretch are this far apart (layout units). */
export const LANE = 3;
/** A shared stretch of this many nets or more is drawn as a ribbon. */
export const LANE_RIBBON = 3;
/** A ribbon's band is drawn only along a stretch this long at least, and reaches this far past its outer lanes. */
const BAND_MIN = 24;
const BAND_PAD = 2;

const keyOf = (p, q) => {
  const [a, b] = p.x < q.x || (p.x === q.x && p.y < q.y) ? [p, q] : [q, p];
  return `${a.x},${a.y},${b.x},${b.y}`;
};
const unit = (p, q) => ({ x: Math.sign(q.x - p.x), y: Math.sign(q.y - p.y) });
/** The right of travel `d` (y down): east → south, south → west. */
const right = (d) => ({ x: -d.y, y: d.x });
const dot = (a, b) => a.x * b.x + a.y * b.y;

/**
 * The lanes of a level's wires (`[{id, net, corners}]`, `net` the identity
 * two wires share a line by: one net's wires are one line). Returns
 * `{display, ribbons, laned}`: `display` a Map of wire id → its drawn
 * corners (only wires a lane moves), `ribbons` the bands (`{id, axis,
 * band, members: [{net}], wires, label, lanes: true}`), `laned` the ids of
 * the wires on a shared stretch. `nameOf(wire)` names a member for a
 * ribbon's label; `isLeg(net)` says a plate control's leg; `groupOf(net)`
 * the group whose bands share one label (a chip's legs), or null.
 */
export function laneWires(wires, { nameOf = () => '', isLeg = () => false, ribbonName = () => '', groupOf = () => null } = {}) {
  // Every corner splits every segment it lies on: a shared stretch is then a run of identical subsegments.
  const byY = new Map();
  const byX = new Map();
  for (const w of wires) {
    for (const p of w.corners) {
      if (!byY.has(p.y)) byY.set(p.y, new Set());
      byY.get(p.y).add(p.x);
      if (!byX.has(p.x)) byX.set(p.x, new Set());
      byX.get(p.x).add(p.y);
    }
  }
  const sortedY = new Map([...byY].map(([y, s]) => [y, [...s].sort((a, b) => a - b)]));
  const sortedX = new Map([...byX].map(([x, s]) => [x, [...s].sort((a, b) => a - b)]));
  const subsOf = new Map();
  const onKey = new Map();
  for (const w of wires) {
    const subs = [];
    const c = w.corners;
    for (let k = 1; k < c.length; k++) {
      const p = c[k - 1];
      const q = c[k];
      if (p.x === q.x && p.y === q.y) continue;
      const horizontal = p.y === q.y;
      if (!horizontal && p.x !== q.x) {
        subs.push({ p, q, key: null, d: null });
        continue;
      }
      const cuts = horizontal ? (sortedY.get(p.y) ?? []).filter((x) => x > Math.min(p.x, q.x) && x < Math.max(p.x, q.x)) : (sortedX.get(p.x) ?? []).filter((y) => y > Math.min(p.y, q.y) && y < Math.max(p.y, q.y));
      const forward = horizontal ? q.x > p.x : q.y > p.y;
      const points = [p, ...(forward ? cuts : [...cuts].reverse()).map((v) => (horizontal ? { x: v, y: p.y } : { x: p.x, y: v })), q];
      for (let i = 1; i < points.length; i++) {
        const s = { p: points[i - 1], q: points[i], key: keyOf(points[i - 1], points[i]), d: unit(points[i - 1], points[i]), horizontal };
        subs.push(s);
        if (!onKey.has(s.key)) onKey.set(s.key, new Map());
        const nets = onKey.get(s.key);
        if (!nets.has(w.net)) nets.set(w.net, { w, i: subs.length - 1 });
      }
    }
    subsOf.set(w.id, subs);
  }
  // The order of the nets on each shared subsegment, right of its first traveller's way to left.
  const orderOf = new Map();
  const compare = (key, A, B) => {
    const a = A;
    const b = B;
    const sa = subsOf.get(a.w.id);
    const sb = subsOf.get(b.w.id);
    const d = sa[a.i].d;
    // B walked the same way as A along the stretch.
    const sbDir = dot(sb[b.i].d, d) > 0 ? 1 : -1;
    const step = (subs, i, dir, k) => {
      const s = subs[i + dir * k];
      if (!s) return null;
      return dir > 0 ? s : { ...s, d: { x: -s.d?.x || 0, y: -s.d?.y || 0 } };
    };
    for (const dir of [1, -1]) {
      let last = d;
      for (let k = 1; ; k++) {
        const x = step(sa, a.i, dir, k);
        const y = step(sb, b.i, dir * sbDir, k);
        if (x && y && x.key && x.key === y.key) {
          last = x.d;
          continue;
        }
        // Forward: the side it turns off to; backward: the side it came in from (walked backwards, the same test).
        const side = (s) => (s?.d ? dot(right(dir > 0 ? last : { x: -last.x, y: -last.y }), s.d) * (dir > 0 ? 1 : -1) : 0);
        const ka = side(x);
        const kb = side(y);
        if (ka !== kb) return kb - ka;
        break;
      }
    }
    return a.w.net < b.w.net ? -1 : a.w.net > b.w.net ? 1 : 0;
  };
  for (const [key, nets] of onKey) {
    if (nets.size < 2) continue;
    const list = [...nets.values()].sort((A, B) => (A.w.net < B.w.net ? -1 : A.w.net > B.w.net ? 1 : 0));
    // An insertion sort by the comparison (not always transitive: the order of insertion decides, deterministically).
    const out = [];
    for (const m of list) {
      let i = out.length;
      while (i > 0 && compare(key, out[i - 1], m) > 0) i--;
      out.splice(i, 0, m);
    }
    const first = out[0];
    const d = subsOf.get(first.w.id)[first.i].d;
    orderOf.set(key, { nets: out.map((m) => m.w.net), d });
  }
  // Runs: the subsegments contiguous on one line, one at least shared. Along a run each net keeps one lane (a ribbon's
  // conductors keep their place; one leaving it leaves a gap, nothing shifts), in the order of the comparison, a lane
  // reused by nets that never meet on it.
  const lines = new Map();
  for (const [key] of onKey) {
    const [x0, y0, x1, y1] = key.split(',').map(Number);
    const horizontal = y0 === y1;
    const line = `${horizontal ? 'h' : 'v'}${horizontal ? y0 : x0}`;
    if (!lines.has(line)) lines.set(line, { horizontal, at: horizontal ? y0 : x0, spans: [] });
    lines.get(line).spans.push({ key, lo: horizontal ? x0 : y0, hi: horizontal ? x1 : y1 });
  }
  const runOf = new Map();
  const runs = [];
  for (const line of [...lines.values()].sort((a, b) => Number(a.horizontal) - Number(b.horizontal) || a.at - b.at)) {
    line.spans.sort((a, b) => a.lo - b.lo);
    let group = [];
    const close = () => {
      if (group.some((sp) => orderOf.has(sp.key))) {
        const canon = line.horizontal ? { x: 1, y: 0 } : { x: 0, y: 1 };
        const members = new Map();
        for (const sp of group) {
          for (const [net] of onKey.get(sp.key)) {
            const m = members.get(net) ?? { net, lo: sp.lo, hi: sp.hi };
            m.lo = Math.min(m.lo, sp.lo);
            m.hi = Math.max(m.hi, sp.hi);
            members.set(net, m);
          }
        }
        // Right of the run's own way (+x or +y) to left: the comparison where two nets first meet on it, else by where they lie, then by name.
        const cmp = (A, B) => {
          for (const sp of group) {
            const on = onKey.get(sp.key);
            const a = on.get(A.net);
            const b = on.get(B.net);
            if (!a || !b) continue;
            const r = compare(sp.key, a, b);
            return dot(subsOf.get(a.w.id)[a.i].d, canon) < 0 ? -r : r;
          }
          return A.lo - B.lo || (A.net < B.net ? -1 : A.net > B.net ? 1 : 0);
        };
        const list = [...members.values()].sort((A, B) => (A.net < B.net ? -1 : A.net > B.net ? 1 : 0));
        const order = [];
        for (const m of list) {
          let i = order.length;
          while (i > 0 && cmp(order[i - 1], m) > 0) i--;
          order.splice(i, 0, m);
        }
        const lanes = new Map();
        let count = 0;
        order.forEach((m, i) => {
          let lane = 0;
          for (let j = 0; j < i; j++) {
            const e = order[j];
            if (e.lo < m.hi && m.lo < e.hi) lane = Math.max(lane, lanes.get(e.net) + 1);
          }
          lanes.set(m.net, lane);
          count = Math.max(count, lane + 1);
        });
        const run = { horizontal: line.horizontal, at: line.at, lo: group[0].lo, hi: group.at(-1).hi, count, lanes, order: order.map((m) => m.net), spans: group, pitch: LANE };
        runs.push(run);
        for (const sp of group) runOf.set(sp.key, run);
      }
      group = [];
    };
    for (const sp of line.spans) {
      if (group.length && group.at(-1).hi !== sp.lo) close();
      group.push(sp);
    }
    close();
  }
  // Each run's pitch: LANE, or less where another line runs near it, so that the two never meet (their lanes' half-widths
  // together stay under the distance between them).
  const byAxis = { h: [], v: [] };
  for (const l of lines.values()) byAxis[l.horizontal ? 'h' : 'v'].push(l);
  for (const list of Object.values(byAxis)) list.sort((a, b) => a.at - b.at);
  const lanesOf = (key) => runOf.get(key)?.count ?? 1;
  for (const list of Object.values(byAxis)) {
    for (let i = 0; i < list.length; i++) {
      for (const sp of list[i].spans) {
        const run = runOf.get(sp.key);
        if (!run) continue;
        for (const j of [i - 1, i + 1]) {
          for (let m = j; m >= 0 && m < list.length && Math.abs(list[m].at - list[i].at) <= 64; m += j < i ? -1 : 1) {
            const d = Math.abs(list[m].at - list[i].at);
            for (const o of list[m].spans) if (o.lo < sp.hi && o.hi > sp.lo) run.pitch = Math.min(run.pitch, d / ((run.count - 1) / 2 + (lanesOf(o.key) - 1) / 2 + 1));
          }
        }
      }
    }
  }
  const offset = (s, net) => {
    const run = s.key ? runOf.get(s.key) : null;
    if (!run) return { x: 0, y: 0 };
    const v = ((run.count - 1) / 2 - run.lanes.get(net)) * run.pitch;
    // Right of +x is +y; right of +y is −x.
    return run.horizontal ? { x: 0, y: v } : { x: -v, y: 0 };
  };
  const display = new Map();
  const laned = new Set();
  for (const w of wires) {
    const subs = subsOf.get(w.id);
    if (!subs.some((s) => s.key && orderOf.has(s.key))) continue;
    laned.add(w.id);
    const os = subs.map((s) => offset(s, w.net));
    const len = (s) => Math.abs(s.q.x - s.p.x) + Math.abs(s.q.y - s.p.y);
    const add = (p, o) => ({ x: p.x + o.x, y: p.y + o.y });
    const pts = [subs[0].p];
    const mag = (o) => Math.abs(o.x) + Math.abs(o.y);
    // Out of its pin into its lane: at 45° where the stretch allows, else steeper within it.
    if (mag(os[0]) > 0 && subs[0].d) {
      const j = Math.min(mag(os[0]), len(subs[0]) / 2);
      pts.push(add(add(subs[0].p, os[0]), { x: subs[0].d.x * j, y: subs[0].d.y * j }));
    }
    for (let k = 0; k + 1 < subs.length; k++) {
      const s = subs[k];
      const t = subs[k + 1];
      const c = s.q;
      if (!s.d || !t.d) {
        pts.push(add(c, mag(os[k]) ? os[k] : os[k + 1]));
        continue;
      }
      if (s.horizontal !== t.horizontal) {
        pts.push(add(add(c, os[k]), os[k + 1]));
        continue;
      }
      if (os[k].x === os[k + 1].x && os[k].y === os[k + 1].y) continue;
      // A step across at 45°, centred on the junction, where its two stretches allow; else steeper (a step straight across
      // would lie on another lane's step at the same junction).
      const h = Math.min(mag({ x: os[k + 1].x - os[k].x, y: os[k + 1].y - os[k].y }) / 2, len(s) / 2, len(t) / 2);
      pts.push(add(add(c, os[k]), { x: -s.d.x * h, y: -s.d.y * h }), add(add(c, os[k + 1]), { x: t.d.x * h, y: t.d.y * h }));
    }
    const last = subs.at(-1);
    const ol = os.at(-1);
    if (mag(ol) > 0 && last.d) {
      const j = Math.min(mag(ol), len(last) / 2);
      pts.push(add(add(last.q, ol), { x: -last.d.x * j, y: -last.d.y * j }));
    }
    pts.push(last.q);
    const clean = [];
    for (const p of pts) {
      const z = clean.at(-1);
      if (z && Math.abs(z.x - p.x) < 1e-9 && Math.abs(z.y - p.y) < 1e-9) continue;
      // Collinear axis-parallel points in a row: the middle one goes.
      if (clean.length >= 2) {
        const a = clean.at(-2);
        if ((a.x === z.x && z.x === p.x) || (a.y === z.y && z.y === p.y)) clean.pop();
      }
      clean.push({ x: p.x, y: p.y });
    }
    display.set(w.id, clean);
  }
  // The ribbons: a run where three nets or more lie side by side, its band over its shared stretch, long enough.
  const ribbons = [];
  for (const run of runs) {
    if (run.count < LANE_RIBBON) continue;
    const shared = run.spans.filter((sp) => (onKey.get(sp.key)?.size ?? 0) >= 2);
    const lo = Math.min(...shared.map((sp) => sp.lo));
    const hi = Math.max(...shared.map((sp) => sp.hi));
    if (hi - lo < BAND_MIN) continue;
    const half = ((run.count - 1) / 2) * run.pitch + BAND_PAD + 0.5;
    const band = run.horizontal ? { x: lo, y: run.at - half, w: hi - lo, h: 2 * half } : { x: run.at - half, y: lo, w: 2 * half, h: hi - lo };
    const nets = run.order;
    const ids = wires.filter((x) => nets.includes(x.net)).map((x) => x.id);
    const legs = nets.every((n) => isLeg(n));
    const name = legs ? 'params' : ribbonName(nets.map((n) => nameOf(wires.find((x) => x.net === n))).filter(Boolean));
    // Its group (a chip's legs): one label for all its bands.
    const groups = new Set(nets.map((n) => groupOf(n)));
    const group = groups.size === 1 ? [...groups][0] : null;
    ribbons.push({ id: '', axis: run.horizontal ? 'h' : 'v', band, members: nets.map((net) => ({ net })), wires: ids, label: `${name ? `${name} ` : ''}×${nets.length}`, lanes: true, length: hi - lo, group });
  }
  ribbons.sort((a, b) => b.members.length - a.members.length || b.length - a.length || (a.band.x - b.band.x) || (a.band.y - b.band.y));
  ribbons.forEach((r, i) => {
    r.id = `lane:${i}`;
  });
  return { display, ribbons, laned };
}

/**
 * Wires drawn on top of each other (ADR 0047 phase 5, measured, never
 * deciding): pairs of axis-parallel segments of different nets on one line
 * overlapping by more than half a unit, of `wires` (`[{net, points}]`, the
 * drawn corners).
 */
export function overlaps(wires) {
  const h = new Map();
  const v = new Map();
  for (const w of wires) {
    const c = w.points;
    for (let k = 1; k < c.length; k++) {
      const p = c[k - 1];
      const q = c[k];
      if (p.y === q.y && p.x !== q.x) {
        if (!h.has(p.y)) h.set(p.y, []);
        h.get(p.y).push({ lo: Math.min(p.x, q.x), hi: Math.max(p.x, q.x), net: w.net });
      } else if (p.x === q.x && p.y !== q.y) {
        if (!v.has(p.x)) v.set(p.x, []);
        v.get(p.x).push({ lo: Math.min(p.y, q.y), hi: Math.max(p.y, q.y), net: w.net });
      }
    }
  }
  let count = 0;
  for (const lines of [h, v]) {
    for (const list of lines.values()) {
      list.sort((a, b) => a.lo - b.lo);
      for (let i = 0; i < list.length; i++) {
        for (let j = i + 1; j < list.length && list[j].lo < list[i].hi - 0.5; j++) {
          if (list[i].net !== list[j].net && Math.min(list[i].hi, list[j].hi) - list[j].lo > 0.5) count++;
        }
      }
    }
  }
  return count;
}
