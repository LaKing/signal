// Ribbon cables on the board (ADR 0047 phase 4, the owner's idea: "szalagkábel",
// replacing the net labels): where several wires of different nets run side
// by side over a long common stretch (the drum triggers from the sequencer
// to the voices, a channel's tracks, a fan-out's trunks), they are drawn as
// one ribbon: a band, its members packed RIBBON_PITCH apart inside it,
// each member leaving it at 45° back to its own track at each end (the
// breakout), so every wire stays one continuous line from pin to pin and
// can be traced through the ribbon; the ribbon is labelled once
// ("trig ×7"). The routes are not changed: a ribbon is a way of drawing
// the members' common stretch (`display`, each member's drawn corners),
// so the measures, the snapshots and a later rip-up all see the routes.
// A stretch is a ribbon only where nothing else lies in its band: no box,
// no other wire along it, no branch of a member's net leaving it midway.
// Deterministic: segments sorted by coordinate, then wire id; windows
// taken first to last. No DOM.

/** At least this many members (different nets) make a ribbon. */
export const RIBBON_MIN = 3;
/** Neighbouring members' tracks are at most this far apart (the maze's grid step). */
export const RIBBON_GAP = 16;
/** Neighbouring tracks closer than this are lanes of one way, not tracks of their own (ADR 0047 phase 5). */
export const RIBBON_NEAR = 6;
/** A ribbon's band (between its breakouts) is at least this long. */
export const RIBBON_RUN = 48;
/** Its members are packed this far apart inside it. */
export const RIBBON_PITCH = 3;
/** A ribbon's stretch (its band and breakouts) keeps this far from every box (ADR 0047 phase 4: no band at a box's pins). */
export const RIBBON_CLEAR = 32;
/** Its band reaches this far past its outer members. */
export const RIBBON_PAD = 2;

/** `p` in a frame where the axis `axis` ('h': along x, 'v': along y) is the first coordinate. */
const along = (axis, p) => (axis === 'h' ? { u: p.x, v: p.y } : { u: p.y, v: p.x });
const back = (axis, u, v) => (axis === 'h' ? { x: u, y: v } : { x: v, y: u });

/** The common prefix of `names`, without trailing digits or separators; '' when under two characters. */
export function ribbonName(names) {
  if (!names.length) return '';
  let prefix = names[0];
  for (const n of names) {
    let i = 0;
    while (i < prefix.length && i < n.length && prefix[i] === n[i]) i++;
    prefix = prefix.slice(0, i);
  }
  prefix = prefix.replace(/[\d_.\-/ ]+$/, '');
  // A single letter only where every name is it and a number (t1, t2, …).
  if (prefix.length === 1 && names.every((n) => new RegExp(`^${prefix}[\\d_.]+$`).test(n))) return prefix;
  return prefix.length >= 2 ? prefix : '';
}

/**
 * The ribbons of a routed board level: `wires` (`[{id, corners, net}]`,
 * `net` the identity a crossing counts by), `boxes` (`[{x, y, w, h}]`,
 * everything a ribbon's band may not cover), `nameOf(wire)` a member's
 * name for the label. Returns `{ribbons, display}`: each ribbon `{id,
 * axis, band: {x, y, w, h}, members: [{wire, net, track, slot}], wires:
 * [ids], label, at}` (`at` the label's point, at the band's middle), and
 * `display`, a Map of wire id → its drawn corners (only for members).
 */
export function formRibbons(wires, boxes = [], { nameOf = () => '', endsOf = null } = {}) {
  // What a member's wires join (`endsOf`): a ribbon's members all share one of them (between the same two regions, at least one end in common).
  const endsMember = (m) => new Set(m.segs.flatMap((s) => (endsOf ? endsOf(s.w) : [])));
  const ribbons = [];
  const replace = new Map();
  const used = new Set();
  for (const axis of ['h', 'v']) {
    // The segments along this axis (a wire's segment k from corner k-1 to k), long enough to hold a band.
    const segs = [];
    const across = [];
    for (const w of wires) {
      const c = w.corners ?? [];
      for (let k = 1; k < c.length; k++) {
        const p = along(axis, c[k - 1]);
        const q = along(axis, c[k]);
        if (p.v === q.v && p.u !== q.u) segs.push({ w, k, net: w.net, at: p.v, lo: Math.min(p.u, q.u), hi: Math.max(p.u, q.u), dir: q.u > p.u ? 1 : -1 });
        else if (p.u === q.u && p.v !== q.v) across.push({ net: w.net, u: p.u, lo: Math.min(p.v, q.v), hi: Math.max(p.v, q.v), ends: [p.v, q.v] });
      }
    }
    segs.sort((a, b) => a.at - b.at || a.lo - b.lo || a.w.id - b.w.id || a.k - b.k);
    // Members: one per net and track, its stretch the longest of its segments there (a net's wires share a trunk).
    const tracks = new Map();
    for (const s of segs) {
      if (s.hi - s.lo < RIBBON_RUN) continue;
      const key = `${s.net}\n${s.at}`;
      if (!tracks.has(key)) tracks.set(key, []);
      tracks.get(key).push(s);
    }
    // Overlapping stretches of one track are one member; disjoint ones are members of their own.
    const members = [];
    for (const list of tracks.values()) {
      list.sort((a, b) => a.lo - b.lo || a.w.id - b.w.id || a.k - b.k);
      let cur = null;
      for (const s of list) {
        if (cur && s.lo < cur.hi) {
          cur.segs.push(s);
          cur.lo = Math.max(cur.lo, s.lo);
          cur.hi = Math.min(cur.hi, s.hi);
        } else {
          cur = { net: s.net, at: s.at, lo: s.lo, hi: s.hi, segs: [s] };
          members.push(cur);
        }
      }
    }
    members.sort((a, b) => a.at - b.at || a.lo - b.lo || (a.segs[0].w.id - b.segs[0].w.id));
    // Windows: neighbouring tracks (at most RIBBON_GAP apart, different nets) whose common stretch holds a band and its breakouts.
    const taken = new Set();
    for (let i = 0; i < members.length; i++) {
      if (taken.has(i)) continue;
      const window = [i];
      let common = endsOf ? endsMember(members[i]) : null;
      let lo = members[i].lo;
      let hi = members[i].hi;
      let last = members[i].at;
      for (let j = i + 1; j < members.length; j++) {
        const m = members[j];
        if (m.at - last > RIBBON_GAP) break;
        // Closer than RIBBON_NEAR: lanes already (ui/lib/board-lanes.js), never packed again.
        if (m.at - last < RIBBON_NEAR) continue;
        if (taken.has(j) || m.at === last || window.some((x) => members[x].net === m.net)) continue;
        const nlo = Math.max(lo, m.lo);
        const nhi = Math.min(hi, m.hi);
        const n = window.length + 1;
        if (nhi - nlo < RIBBON_RUN + 2 * shiftOf([...window.map((x) => members[x].at), m.at])) continue;
        if (common) {
          const mine = endsMember(m);
          const both = new Set([...common].filter((e) => mine.has(e)));
          if (!both.size) continue;
          common = both;
        }
        window.push(j);
        lo = nlo;
        hi = nhi;
        last = m.at;
        if (n >= 64) break;
      }
      if (window.length < RIBBON_MIN) continue;
      const ms = window.map((x) => members[x]);
      const ats = ms.map((m) => m.at);
      const v0 = Math.min(...ats);
      const v1 = Math.max(...ats);
      const nets = new Set(ms.map((m) => m.net));
      // Nothing else in its band: no box, no other net along it, no branch of a member's net leaving it midway, no segment already a ribbon's.
      const clear =
        !ms.some((m) => m.segs.some((s) => used.has(`${s.w.id}:${s.k}`))) &&
        !boxes.some((b) => {
          // Its whole stretch, breakouts included, keeps RIBBON_CLEAR from every box: a band starts only once its members have left a box's edge.
          const r = axis === 'h' ? { u0: b.x, u1: b.x + b.w, v0: b.y, v1: b.y + b.h } : { u0: b.y, u1: b.y + b.h, v0: b.x, v1: b.x + b.w };
          return r.u0 - RIBBON_CLEAR < hi && r.u1 + RIBBON_CLEAR > lo && r.v0 - RIBBON_CLEAR <= v1 && r.v1 + RIBBON_CLEAR >= v0;
        }) &&
        !segs.some((s) => !ms.some((m) => m.segs.includes(s)) && s.at >= v0 && s.at <= v1 && s.lo < hi && s.hi > lo) &&
        !across.some((a) => nets.has(a.net) && a.u > lo && a.u < hi && a.ends.some((e) => e >= v0 && e <= v1));
      if (!clear) continue;
      for (const x of window) taken.add(x);
      const n = ms.length;
      const centre = Math.round((v0 + v1) / 2);
      // Tracks already this close (a connector's pins, ADR 0047 phase 5) keep their places: a band over them, no breakout.
      const close = ms.every((m, k) => k === 0 || m.at - ms[k - 1].at <= 2 * RIBBON_PITCH);
      const slots = close ? ms.map((m) => m.at) : ms.map((_, k) => centre + (k - (n - 1) / 2) * RIBBON_PITCH);
      const shift = Math.max(...ms.map((m, k) => Math.abs(slots[k] - m.at)));
      const id = ribbons.length;
      for (const [k, m] of ms.entries()) {
        const d = Math.abs(slots[k] - m.at);
        for (const s of m.segs) {
          used.add(`${s.w.id}:${s.k}`);
          // Along its way: its track to the breakout, 45° into its slot, the band, 45° back out.
          const pts = s.dir > 0 ? [[lo, m.at], [lo + d, slots[k]], [hi - d, slots[k]], [hi, m.at]] : [[hi, m.at], [hi - d, slots[k]], [lo + d, slots[k]], [lo, m.at]];
          if (!replace.has(s.w.id)) replace.set(s.w.id, new Map());
          replace.get(s.w.id).set(s.k, pts.map(([u, v]) => back(axis, u, v)));
        }
      }
      const s0 = slots[0] - RIBBON_PAD - 0.5;
      const s1 = slots[n - 1] + RIBBON_PAD + 0.5;
      const band = axis === 'h' ? { x: lo + shift, y: s0, w: hi - lo - 2 * shift, h: s1 - s0 } : { x: s0, y: lo + shift, w: s1 - s0, h: hi - lo - 2 * shift };
      const ids = [...new Set(ms.flatMap((m) => m.segs.map((s) => s.w.id)))];
      const name = ribbonName(ms.map((m) => nameOf(m.segs[0].w)).filter(Boolean));
      ribbons.push({
        id,
        axis,
        band,
        members: ms.map((m, k) => ({ wire: m.segs[0].w.id, wires: m.segs.map((s) => s.w.id), net: m.net, track: m.at, slot: slots[k] })),
        wires: ids,
        label: `${name ? `${name} ` : ''}×${n}`,
        at: back(axis, (lo + hi) / 2, s0),
      });
    }
  }
  const display = new Map();
  for (const w of wires) {
    const r = replace.get(w.id);
    if (!r) continue;
    const c = w.corners;
    const out = [c[0]];
    for (let k = 1; k < c.length; k++) {
      for (const p of r.get(k) ?? []) out.push(p);
      out.push(c[k]);
    }
    display.set(w.id, dedupe(out));
  }
  return { ribbons, display };
}

/** The largest distance from the members' tracks `ats` (sorted) to their slots in a band centred between them. */
function shiftOf(ats) {
  const n = ats.length;
  const centre = Math.round((Math.min(...ats) + Math.max(...ats)) / 2);
  let most = 0;
  ats.forEach((at, k) => {
    most = Math.max(most, Math.abs(centre + (k - (n - 1) / 2) * RIBBON_PITCH - at));
  });
  return most;
}

function dedupe(points) {
  const out = [];
  for (const p of points) {
    const last = out.at(-1);
    if (last && last.x === p.x && last.y === p.y) continue;
    out.push({ x: p.x, y: p.y });
  }
  return out;
}

/** A ribbon's label: its font size (layout units), about this wide per character, this tall. */
export const RIBBON_TEXT = 8;
const RIBBON_CHAR = 4.8;
const RIBBON_LINE = 9;

/** The rectangle of a label `text` at `at` (`{x, y, anchor}`, its baseline's start, middle or end). */
export function ribbonLabelRect(text, at) {
  const w = text.length * RIBBON_CHAR;
  const x = at.anchor === 'end' ? at.x - w : at.anchor === 'middle' ? at.x - w / 2 : at.x;
  return { x, y: at.y - RIBBON_LINE + 2, w, h: RIBBON_LINE };
}

/**
 * The ribbons' labels placed (ADR 0047 phase 4; the coordinator's check of
 * 3f32ab2: "t ×7" over a pin's name): one per set of members (a ribbon
 * whose members an earlier labelled one holds has none), at the first of
 * a few places along its band (above or beside its middle, then its
 * quarters) whose text overlaps no rectangle of `texts` (boxes, a via's
 * name, net labels) nor a label placed before; else none. Sets each
 * ribbon's `text` (`{x, y, anchor}` or null). Deterministic: the ribbons'
 * order.
 */
export function placeRibbonLabels(ribbons, texts = []) {
  const placed = [];
  const hit = (r) => [...texts, ...placed].some((o) => r.x < o.x + o.w && o.x < r.x + r.w && r.y < o.y + o.h && o.y < r.y + r.h);
  const labelled = [];
  const groups = new Set();
  for (const r of ribbons) {
    r.text = null;
    const members = new Set(r.members.map((m) => m.net));
    if (labelled.some((set) => [...members].every((n) => set.has(n)))) continue;
    // One label per group (ADR 0047 phase 5: a chip's legs, however many bands they gather in), on its widest band (the first).
    if (r.group && groups.has(r.group)) continue;
    const b = r.band;
    const spots = [];
    if (r.axis === 'h') {
      for (const f of [0.5, 0.25, 0.75]) {
        const x = b.x + b.w * f;
        spots.push({ x, y: b.y - 3, anchor: 'middle' }, { x, y: b.y + b.h + RIBBON_LINE - 1, anchor: 'middle' });
      }
    } else {
      for (const f of [0.5, 0.25, 0.75]) {
        const y = b.y + b.h * f + 3;
        spots.push({ x: b.x + b.w + 3, y, anchor: 'start' }, { x: b.x - 3, y, anchor: 'end' });
      }
    }
    const at = spots.find((s) => !hit(ribbonLabelRect(r.label, s)));
    if (!at) continue;
    r.text = at;
    placed.push(ribbonLabelRect(r.label, at));
    labelled.push(members);
    if (r.group) groups.add(r.group);
  }
  return ribbons;
}
