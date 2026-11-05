// The Engine layer of the Signal Project plane (ADR 0051 §1, §8; the
// owner's decisions of 2026-11-03): every instrument of the focused Stack
// shows its audio engine as **its package** (ui/lib/plane-package.js), at
// the place and with the pins the Wiring and Control layers draw it, in the
// same Stack-relative plane units, so the view never moves when the layer
// changes. The package is the top-level chip: zooming in reveals its inside
// in its body (`engineLayout`, `packageReveal`), as a composite's inside is
// revealed in its card (below). Inside the package:
//   - the instrument's instances (its engine, `engineOf`) as cards in the
//     Deck's look, in columns by signal flow (the schematic's layering,
//     ui/lib/schematic-layout.js `layout`), scaled as one to fit;
//   - their pins, every input on the left (its ports, then each of its
//     parameters) and every output on the right, at every level (the
//     owner's, 2026-11-04); the control signal from the package's connector
//     wired into each bound parameter's pin, inside a chip from each of the
//     chip's parameter pins on its left as a wire entering is (the owner's,
//     2026-11-04: as the Wiring wires, every way its own track);
//   - the signal connections between them, orthogonal, in channels between
//     the columns (one track per net, ordered so that nets that meet do not
//     run along one another, `orderedTracks`), a connection that skips
//     columns along a free row or a lane above the cards, a feedback one
//     along a lane below them (lanes as ui/lib/plane-wiring.js
//     `assignTracks` gives tracks); and the
//     connections that cross the instrument's edge, entering and leaving at
//     the package's pin of the same connection (the Wiring layer's port).
// Composites are one card each; zooming in reveals a composite's inside in
// place (`insideOf`): `layLevel` lays out any level (`{instances,
// connections}`) in any area, so the inside (`graph.composites[module]`) is
// laid out in its card's `body` by the same function, scaled into it, with
// the composite's inputs and outputs entering and leaving at its card's
// pins' heights; recursively, a composite inside one reveals the same way.
// `chipPath` names the composites the view is inside (the bar's Layer N).
// Presentation only (rule 4): nothing is stored. Deterministic: instances
// and connections in source order, fixed tie-breaks. Pure; no DOM.

import { PU } from './plate.js';
import { instanceOf, portOf } from './plane-instruments.js';
import { assignTracks, connectionKind, portKind, simplify } from './plane-wiring.js';
import { HEADER, ROW, PAD, GAP_Y, layout as schematicLayout } from './schematic-layout.js';

/** A card's corner radius and its header band's height, in the cards' own units (the schematic's). */
export const CARD_RX = 6;
export const HEAD_H = HEADER - 4;
/** A channel's track pitch, its pad on each side, its least width; an edge channel's least width; a lane's pitch (cards' units). */
export const TRACK = 8;
export const CHANNEL_PAD = 12;
export const CHANNEL_MIN = 32;
export const EDGE_MIN = 20;
export const LANE = 8;
/** A chip's card's least width for its height (its inside is laid out wide, by signal flow). */
export const CHIP_ASPECT = 1.5;
/** The cards' largest scale on the plane (a small engine is not blown up past it). */
export const MAX_SCALE = 2;
/** The instrument's frame: its inset and its title row, in plane units. */
export const INSET = PU;
export const TITLE = 3 * PU;

/** Where an address's instance ends: `/a.b` → `a`, `a.b` → `a`. */
const nodeOf = instanceOf;
/** An instance's own port (`/a.b`, `a.b`), never one inside it (`/a/inner.b`). */
const OWN_PORT = /^\/?[^/.]+\.[^/.]+$/;

/**
 * One level laid out in `area` (plane units): `level` (`{instances,
 * connections}`, as `graph` gives a level: ports with `direction`,
 * `shape`, `kind`, `rate`; connections `{from, to}`), `names` (the
 * instances drawn, in source order; default all), `edgeIns` and `edgeOuts`
 * (`Map(address → y)`: where a connection from outside enters at the
 * area's left edge, keyed by the inner input it reaches, and where one
 * leaves at the right edge, keyed by the inner output), `frame` (the
 * rectangle whose edges they are; default `area`), `joinEdges` (wires
 * entering from one source are one net, a fan-out; default each its own),
 * `edgeKinds` (`Map(address → kind)`: the kind a connection entering has
 * outside, keyed by the inner input it reaches), `binds` (the control signal:
 * `[{address, x, y, export}]`, each into its target's pin row; `bindFrom`
 * 'top' or 'bottom', entering at `x` on the edge at `bindY`, or 'left', from
 * a chip's pin at `y` on the frame's left edge, wired as a connection
 * entering there is, the legs from one pin one net).
 * Returns `{scale, cards,
 * pins, wires, edges, lanes, binds}`, plane units:
 *   - a card `{id, name, module, composite, x, y, w, h, head, body,
 *     column, ins, outs}`; `body` the area below its header, where a
 *     composite's inside is laid out (`insideOf`);
 *   - a pin `{id, card, name, address, side, kind, parameter, connected,
 *     bind, x, y}`, `bind` where the control signal enters it;
 *   - a wire `{id, index, from, to, kind, route, points, junctions}`:
 *     `index` the connection's place in `level.connections`, `route` one
 *     of `adjacent`, `row`, `over`, `feedback`, `edge`;
 *   - an edge pin `{id, side, address, x, y}`, where a wire crosses the
 *     frame;
 *   - a bind `{id, address, export, kind: 'control', points, junctions}`:
 *     its whole way, from the edge into its pin.
 */
export function layLevel(args) {
  // Laid out with room for every lane a wire might need, then again with the lanes the wires took (when they still fit).
  const first = layPass(args, null);
  const { over, feedback, binds } = first.reserved;
  if (over === first.lanes.over && feedback === first.lanes.feedback && binds === first.lanes.binds) return strip(first);
  const second = layPass(args, first.lanes);
  return strip(fits(second.lanes, first.lanes) ? second : first);
}

function strip({ reserved, ...laid }) {
  return laid;
}

/** The lanes a pass took against those it kept: no more than were kept, of each kind. */
function fits(lanes, reserved) {
  return lanes.over <= reserved.over && lanes.feedback <= reserved.feedback && lanes.binds <= reserved.binds;
}

/** One pass of `layLevel`: `reserve` the lanes kept above and below the cards (`{over, feedback, binds}`), or null for as many as might be needed. */
function layPass({ level, names = null, area, edgeIns = new Map(), edgeOuts = new Map(), edgeKinds = new Map(), frame = area, joinEdges = false, taps = [], binds = [], bindFrom = 'top', bindY = area.y, bindWired = false }, reserve) {
  const all = level?.instances ?? [];
  const wanted = names ? new Set(names) : null;
  const instances = all.filter((i) => !wanted || wanted.has(i.name));
  const inside = new Set(instances.map((i) => i.name));
  const byName = new Map(instances.map((i) => [i.name, i]));
  const connections = (level?.connections ?? []).map((c, index) => ({ ...c, index }));
  // A port of a drawn instance on the side a connection's end needs (`out` for its source, `in` for its target).
  const portAt = (address, direction) => {
    if (!OWN_PORT.test(String(address ?? ''))) return null;
    const inst = byName.get(nodeOf(address));
    const name = portOf(address);
    const port = inst?.ports?.find((p) => p.name === name && p.direction === direction);
    if (port) return { port, parameter: false };
    const param = direction === 'in' ? inst?.parameters?.find((p) => p.name === name) : null;
    return param ? { port: { ...param, direction: 'in', shape: 'signal' }, parameter: true } : null;
  };
  // The connections drawn: between two drawn instances, or crossing the edge (one per inner end: the Wiring's port).
  const internal = [];
  const entering = new Map();
  const leaving = new Map();
  for (const c of connections) {
    const a = inside.has(nodeOf(c.from)) && portAt(c.from, 'out');
    const b = inside.has(nodeOf(c.to)) && portAt(c.to, 'in');
    if (a && b) internal.push(c);
    else if (b && !inside.has(nodeOf(c.from)) && !entering.has(c.to)) entering.set(c.to, c);
    else if (a && !inside.has(nodeOf(c.to)) && !leaving.has(c.from)) leaving.set(c.from, c);
  }
  // The control signal (`binds`): each leg into a parameter (or a port) of a drawn instance.
  const legs = binds.filter((b) => inside.has(nodeOf(b.address)) && portAt(b.address, 'in')).map((b, i) => ({ ...b, x: bindFrom === 'left' ? frame.x : b.x, key: `b ${i} ${b.address}` }));
  // The parameters a signal connection drives (the layering sees these alone).
  const driven = new Map();
  for (const c of [...internal, ...entering.values()]) {
    const at = portAt(c.to, 'in');
    if (!at?.parameter) continue;
    const name = nodeOf(c.to);
    if (!driven.has(name)) driven.set(name, []);
    if (!driven.get(name).some((p) => p.name === at.port.name)) driven.get(name).push(at.port);
  }
  // Columns and their order from the schematic's layering of these instances alone (parameters left out, but the driven ones).
  const stripped = instances.map((i) => ({ ...i, parameters: driven.get(i.name) ?? [] }));
  const placed = schematicLayout({ instances: stripped, connections: internal.map((c) => ({ from: c.from, to: c.to })) });
  const nodeBy = new Map(placed.nodes.filter((n) => n.kind === 'instance').map((n) => [n.name, n]));
  const layerOf = [...new Set(instances.map((i) => nodeBy.get(i.name)?.layer ?? 0))].sort((p, q) => p - q);
  const columnOf = new Map(instances.map((i) => [i.name, layerOf.indexOf(nodeBy.get(i.name)?.layer ?? 0)]));
  const n = layerOf.length;
  const columns = Array.from({ length: n }, () => []);
  for (const i of instances) columns[columnOf.get(i.name)].push(i);
  for (const col of columns) col.sort((p, q) => (nodeBy.get(p.name)?.y ?? 0) - (nodeBy.get(q.name)?.y ?? 0) || all.indexOf(p) - all.indexOf(q));
  // Each card's pins and natural size (the schematic's units).
  // An address in either spelling (`/a.b` as the graph writes a top-level port, `a.b` as a connection may).
  const spell = (a) => String(a ?? '').replace(/^\//, '');
  const connectedIns = new Set([...internal.map((c) => c.to), ...entering.keys(), ...legs.map((l) => l.address)].map(spell));
  const bound = new Set(legs.map((l) => spell(l.address)));
  // The inputs a connection already takes (ADR 0054: one each; a control's leg is none).
  const wiredIns = new Set([...internal.map((c) => c.to), ...entering.keys()].map(spell));
  // Automation sources inside a chip (`taps`, the package's right-edge pins): a pin of their own on the chip's right, named by
  // the way into it (`env_amt.out`), wired out to the frame's right edge at the tap's height.
  const tapsOf = new Map();
  for (const t of taps) {
    const text = spell(t.address);
    const cut = text.indexOf('/');
    if (cut < 0 || !inside.has(text.slice(0, cut))) continue;
    const name = text.slice(0, cut);
    if (!tapsOf.has(name)) tapsOf.set(name, []);
    tapsOf.get(name).push({ name: text.slice(cut + 1), address: t.address, y: t.y, direction: 'out', shape: 'signal', tap: true });
  }
  const connectedOuts = new Set([...internal.map((c) => c.from), ...leaving.keys(), ...[...tapsOf.values()].flat().map((t) => t.address)].map(spell));
  // Wired legs into one card in the order that keeps them apart (the leftmost source the lowest row from above, the highest from below).
  const rank = new Map();
  // From the left (a chip's pins), the upper pin the upper row.
  const fromLeft = bindFrom === 'left';
  if (fromLeft) [...legs].sort((a, b) => a.y - b.y).forEach((l, i) => rank.set(spell(l.address), i));
  else if (bindWired) [...legs].sort((a, b) => (bindFrom === 'bottom' ? a.x - b.x : b.x - a.x)).forEach((l, i) => rank.set(spell(l.address), i));
  const natural = new Map();
  for (const i of instances) {
    // Every input on the left (the owner's, 2026-11-04): its ports, then each parameter (one no port shares a name with), a bound one first where its leg is wired.
    const ports = (i.ports ?? []).filter((p) => p.direction === 'in');
    const params = (i.parameters ?? []).filter((q) => !ports.some((p) => p.name === q.name)).map((q, j) => ({ q, j, r: rank.get(spell(q.address ?? `${i.name}.${q.name}`)) ?? Infinity }));
    params.sort((a, b) => a.r - b.r || a.j - b.j);
    const ins = [...ports, ...params.map(({ q }) => ({ ...q, direction: 'in', shape: 'signal' }))];
    const outs = [...(i.ports ?? []).filter((p) => p.direction === 'out'), ...(tapsOf.get(i.name) ?? [])];
    const h = HEADER + ROW * Math.max(1, ins.length, outs.length) + PAD;
    // A chip as wide as its rows are tall and then some (`CHIP_ASPECT`): its inside, laid out wide by signal flow, has room in its body.
    const w = Math.max(nodeBy.get(i.name)?.w ?? 96, i.composite ? CHIP_ASPECT * h : 0);
    natural.set(i.name, { ins, outs, w, h });
  }
  const colW = columns.map((col) => Math.max(0, ...col.map((i) => natural.get(i.name).w)));
  const colH = columns.map((col) => col.reduce((s, i) => s + natural.get(i.name).h, 0) + Math.max(0, col.length - 1) * GAP_Y);
  // The wires, in connection order: internal ones, then those entering, then those leaving.
  const wires = [
    ...internal.map((c) => ({ c, a: columnOf.get(nodeOf(c.from)), b: columnOf.get(nodeOf(c.to)), edge: null })),
    ...[...entering.values()].map((c) => ({ c, a: -1, b: columnOf.get(nodeOf(c.to)), edge: 'in' })),
    ...[...leaving.values()].map((c) => ({ c, a: columnOf.get(nodeOf(c.from)), b: n, edge: 'out' })),
    // From a chip's pin on its left edge (the owner's, 2026-11-04: as the Wiring wires): a wire entering, each pin one net.
    ...(fromLeft ? legs : []).map((l, i) => ({ c: { index: connections.length + i, from: `@${l.export ?? ''}`, to: l.address }, a: -1, b: columnOf.get(nodeOf(l.address)), edge: 'in', leg: l })),
    ...[...tapsOf].flatMap(([name, ts]) => ts.map((t, i) => ({ c: { index: connections.length + legs.length + i, from: t.address, to: '@automation' }, a: columnOf.get(name), b: n, edge: 'out', tap: t }))),
  ].sort((p, q) => p.c.index - q.c.index || (p.edge ?? '').localeCompare(q.edge ?? ''));
  // Channel i lies right of column i (-1: the left edge's, n - 1: the right edge's). Its nets, an upper bound on its tracks.
  const nets = new Map();
  const net = (ch, key) => {
    if (!nets.has(ch)) nets.set(ch, new Set());
    nets.get(ch).add(key);
  };
  let overs = 0;
  let backs = 0;
  for (const w of wires) {
    // Wires entering from one outside source keep apart: each its own edge pin, its own net (`joinEdges`: one net per source, a composite's input fanning out).
    w.srcKey = w.leg ? `p ${w.leg.y} ${w.leg.export}` : w.edge === 'in' && !joinEdges ? `e ${w.c.to}` : `s ${w.c.from}`;
    // Into a column across others: one track per source there (its branches into the column one trunk).
    w.dstKey = `d ${w.srcKey} ${w.b}`;
    if (w.b === w.a + 1) net(w.a, w.srcKey);
    else {
      net(w.a, w.srcKey);
      net(w.b - 1, w.dstKey);
      if (w.b > w.a) overs++;
      else backs++;
    }
  }
  // A wired leg from above (or below) comes down (or up) the channel left of its card's column, a track of its own.
  if (bindWired && !fromLeft) for (const l of legs) net(columnOf.get(nodeOf(l.address)) - 1, l.key);
  const chW = (ch) => {
    const tracks = nets.get(ch)?.size ?? 0;
    const least = ch === -1 || ch === n - 1 ? EDGE_MIN : CHANNEL_MIN;
    return Math.max(least, 2 * CHANNEL_PAD + tracks * TRACK);
  };
  const channelsW = Array.from({ length: n + 1 }, (_, k) => chW(k - 1));
  const naturalW = colW.reduce((s, x) => s + x, 0) + channelsW.reduce((s, x) => s + x, 0);
  const bandH = Math.max(HEADER, ...colH);
  const reserved = reserve ?? { over: overs, feedback: backs, binds: bindWired && !fromLeft ? legs.length : 0 };
  const naturalH = bandH + (reserved.binds + reserved.over + reserved.feedback) * LANE + (reserved.binds ? LANE : 0) + (reserved.over ? LANE : 0) + (reserved.feedback ? LANE : 0);
  const scale = n ? Math.min(MAX_SCALE, area.w / Math.max(1, naturalW), area.h / Math.max(1, naturalH)) : 1;
  // The width left over spreads the columns: each channel takes a share.
  const spare = Math.max(0, area.w - naturalW * scale) / (n + 1);
  // From the top of the area (under the title, beside where the Wiring's ports enter), the lanes above first: the control signal's (when it comes from above), then the wires'.
  const bindsH = reserved.binds ? (reserved.binds + 1) * LANE * scale : 0;
  // The height left over (a wide level in a tall area) above and below in equal parts.
  const slack = Math.max(0, area.h - naturalH * scale) / 2;
  const top = area.y + slack + (bindFrom === 'bottom' ? 0 : bindsH) + (reserved.over ? (reserved.over + 1) * LANE * scale : 0);
  const cardsTop = top;
  const cardsBottom = top + bandH * scale;
  // The control signal's lanes, lane 0 nearest the edge it comes from: above the wires' lanes, or below those under the cards.
  const bindBottom = cardsBottom + (reserved.feedback ? (reserved.feedback + 1) * LANE * scale : 0) + bindsH;
  const bindLane = (t) => (bindFrom === 'bottom' ? bindBottom - (t + 1) * LANE * scale : area.y + slack + (t + 1) * LANE * scale);
  const chX = new Map();
  const colX = [];
  let x = area.x;
  for (let k = 0; k <= n; k++) {
    const w = channelsW[k] * scale + spare;
    chX.set(k - 1, { x0: x, x1: x + w });
    x += w;
    if (k < n) {
      colX.push(x);
      x += colW[k] * scale;
    }
  }
  // The cards: each column's stack centred in the band, each card centred in its column.
  const cards = [];
  const pins = [];
  const pinOf = new Map();
  const cardOf = new Map();
  columns.forEach((col, k) => {
    let y = cardsTop + ((bandH - colH[k]) * scale) / 2;
    for (const i of col) {
      const nat = natural.get(i.name);
      const w = nat.w * scale;
      const h = nat.h * scale;
      const cx = colX[k] + (colW[k] * scale - w) / 2;
      const card = {
        id: `card ${i.name}`,
        name: i.name,
        module: i.module,
        composite: Boolean(i.composite),
        column: k,
        x: cx,
        y,
        w,
        h,
        head: { x: cx, y, w, h: HEAD_H * scale },
        body: { x: cx + PAD * scale, y: y + HEADER * scale, w: Math.max(0, w - 2 * PAD * scale), h: Math.max(0, h - (HEADER + PAD) * scale) },
        ins: [],
        outs: [],
      };
      const pin = (p, side, j) => {
        const address = p.address ?? `${i.name}.${p.name}`;
        const parameter = side === 'in' && !(i.ports ?? []).includes(p);
        const q = {
          id: `pin ${side} ${address}`,
          card: card.id,
          name: p.name,
          address,
          side,
          kind: parameter || p.tap ? 'control' : portKind(p),
          parameter,
          tap: Boolean(p.tap),
          connected: side === 'in' ? connectedIns.has(spell(address)) : connectedOuts.has(spell(address)),
          bind: side === 'in' && bound.has(spell(address)),
          wired: side === 'in' && wiredIns.has(spell(address)),
          x: side === 'in' ? cx : cx + w,
          y: y + (HEADER + ROW * (j + 0.5)) * scale,
        };
        pins.push(q);
        pinOf.set(`${side} ${address}`, q);
        card[side === 'in' ? 'ins' : 'outs'].push(q.id);
      };
      nat.ins.forEach((p, j) => pin(p, 'in', j));
      nat.outs.forEach((p, j) => pin(p, 'out', j));
      cards.push(card);
      cardOf.set(i.name, card);
      y += h + GAP_Y * scale;
    }
  });
  // Ends: an inner pin, or the frame's edge at the Wiring's height (else spread down the frame, in order, kept off the frame's
  // corners: 4 plane units, less in a small frame, a chip's card deep inside).
  const inset = Math.min(4, frame.h / 8);
  const clampY = (y) => Math.min(frame.y + frame.h - inset, Math.max(frame.y + inset, y));
  const spread = () => {
    let k = 0;
    return () => frame.y + TITLE + (k++ + 0.5) * 2 * PU;
  };
  const nextIn = spread();
  const nextOut = spread();
  const edges = [];
  for (const w of wires) {
    // A wire entering keeps the kind it has outside (one net across the edge, one colour), unless that says nothing.
    const outside = w.edge === 'in' && !w.leg ? edgeKinds.get(w.c.to) : null;
    w.kind = w.leg || w.tap ? 'control' : outside && outside !== 'generic' ? outside : kindOf(w.c, portAt);
    if (w.leg) w.P = { x: frame.x, y: w.leg.y };
    else if (w.edge === 'in') {
      const y = edgeIns.has(w.c.to) ? edgeIns.get(w.c.to) : clampY(nextIn());
      w.P = { x: frame.x, y };
      edges.push({ id: `edge in ${w.c.to}`, side: 'in', address: w.c.to, from: w.c.from, x: frame.x, y });
    } else w.P = pinOf.get(`out ${w.c.from}`) ?? pinOf.get(`out ${addressOf(w.c.from, byName)}`);
    if (w.edge === 'out') {
      const y = w.tap ? w.tap.y : edgeOuts.has(w.c.from) ? edgeOuts.get(w.c.from) : clampY(nextOut());
      w.Q = { x: frame.x + frame.w, y };
      edges.push({ id: `edge out ${w.c.from}`, side: 'out', address: w.c.from, to: w.c.to, x: frame.x + frame.w, y });
    } else w.Q = pinOf.get(`in ${w.c.to}`) ?? pinOf.get(`in ${addressOf(w.c.to, byName)}`);
  }
  const drawn = wires.filter((w) => w.P && w.Q);
  // One net, one colour (the owner's, 2026-11-05: a branch's dot never on a wire of another colour): a tap is its net's
  // colour, and a net whose branches would differ takes its source's kind.
  const netKinds = new Map();
  for (const w of drawn) if (!w.tap) netKinds.set(w.srcKey, (netKinds.get(w.srcKey) ?? new Set()).add(w.kind));
  const netKind = new Map([...netKinds].map(([key, kinds]) => [key, kinds.size === 1 ? [...kinds][0] : null]));
  for (const w of drawn) {
    if (!netKinds.has(w.srcKey)) continue;
    const k = netKind.get(w.srcKey);
    w.kind = k ?? (w.P.kind && w.P.kind !== 'generic' ? w.P.kind : 'generic');
  }
  // A generic pin (the core arithmetic's) takes its wire's colour.
  for (const w of drawn) for (const q of [w.P, w.Q]) if (q.kind === 'generic' && q.card) q.kind = w.kind;
  // A wire that skips columns: along its source's or its target's row where no card of the columns between is, else a lane above.
  const clear = (y, a, b) => {
    const gap = (GAP_Y * scale) / 3;
    for (let k = a + 1; k < b; k++) for (const i of columns[k]) {
      const c = cardOf.get(i.name);
      if (y > c.y - gap && y < c.y + c.h + gap) return false;
    }
    return true;
  };
  const mid = (ch) => (chX.get(ch).x0 + chX.get(ch).x1) / 2;
  // The rows taken (a wire along a height across columns): another net's row may cross this one, never run along it (the
  // owner's, 2026-11-05: a chip's left and right pins share their heights, so a pin's way in and a tap's way out would). The
  // chip's pins first (their way is at their height), then the wires in order.
  const rows = [];
  const rowFree = (y, lo, hi, key) => !rows.some((r) => r.key !== key && Math.abs(r.y - y) < 1e-9 && Math.min(hi, r.hi) - Math.max(lo, r.lo) > 1e-9);
  const overIv = [];
  const backIv = [];
  for (const w of [...drawn.filter((d) => d.leg), ...drawn.filter((d) => !d.leg)]) {
    if (w.b === w.a + 1) w.route = w.edge ? 'edge' : 'adjacent';
    else if (w.b > w.a) {
      const lo = chX.get(w.a).x0;
      const hi = chX.get(w.b - 1).x1;
      const y = [w.P.y, w.Q.y].find((yy) => clear(yy, w.a, w.b) && rowFree(yy, lo, hi, w.srcKey));
      if (y !== undefined) {
        w.route = w.edge ? 'edge' : 'row';
        w.yh = y;
        rows.push({ y, lo, hi, key: w.srcKey });
      } else {
        w.route = 'over';
        overIv.push({ key: w, lo: Math.min(mid(w.a), mid(w.b - 1)), hi: Math.max(mid(w.a), mid(w.b - 1)) });
      }
    } else {
      w.route = 'feedback';
      backIv.push({ key: w, lo: Math.min(mid(w.a), mid(w.b - 1)), hi: Math.max(mid(w.a), mid(w.b - 1)) });
    }
  }
  const gap = TRACK * scale;
  const lanes = { over: 0, feedback: 0, binds: 0 };
  // The branches of one net share a lane (a tree, never parallel lines a hair apart): one interval per source, over all its branches.
  const bySource = (ivs) => {
    const groups = new Map();
    for (const iv of ivs) {
      const g = groups.get(iv.key.srcKey);
      if (g) {
        g.lo = Math.min(g.lo, iv.lo);
        g.hi = Math.max(g.hi, iv.hi);
        g.wires.push(iv.key);
      } else groups.set(iv.key.srcKey, { key: iv.key.srcKey, lo: iv.lo, hi: iv.hi, wires: [iv.key] });
    }
    return [...groups.values()];
  };
  // Of the chip's pins going over, the lower one nearer the cards (its rise then never crosses an upper one's run).
  const legY = (g) => g.wires[0].leg?.y;
  const order = (groups) => groups.sort((g, h) => (legY(g) === undefined ? 0 : 1) - (legY(h) === undefined ? 0 : 1) || (legY(h) ?? 0) - (legY(g) ?? 0));
  for (const [groups, below] of [[order(bySource(overIv)), false], [bySource(backIv), true]]) {
    const tracks = assignTracks(groups, gap);
    for (const g of groups) {
      const t = tracks.get(g.key);
      for (const w of g.wires) w.yh = below ? cardsBottom + (t + 1) * LANE * scale : cardsTop - (t + 1) * LANE * scale;
      if (below) lanes.feedback = Math.max(lanes.feedback, t + 1);
      else lanes.over = Math.max(lanes.over, t + 1);
    }
  }
  // The channels' tracks: one per net, over the heights it spans there; the lowest-starting nearest its column (as the Wiring's).
  const spans = new Map();
  // `y0` where the net comes into the channel from the left, `y1` where it leaves to the right.
  const span = (ch, key, y0, y1) => {
    if (!spans.has(ch)) spans.set(ch, new Map());
    const m = spans.get(ch);
    // A wire straight across the channel (along a row) takes no track of its own.
    if (Math.abs(y0 - y1) < 1e-9 && !m.has(key)) return;
    const iv = m.get(key) ?? { key, lo: Math.min(y0, y1), hi: Math.max(y0, y1), yl: y0, yr: y1, ins: [], outs: [] };
    iv.lo = Math.min(iv.lo, y0, y1);
    iv.hi = Math.max(iv.hi, y0, y1);
    // Every height it comes in at and leaves at (a fan-out leaves at several).
    iv.ins.push(y0);
    iv.outs.push(y1);
    m.set(key, iv);
  };
  for (const w of drawn) {
    if (w.b === w.a + 1) span(w.a, w.srcKey, w.P.y, w.Q.y);
    else {
      span(w.a, w.srcKey, w.P.y, w.yh);
      span(w.b - 1, w.dstKey, w.yh, w.Q.y);
    }
  }
  // The control signal's legs from above (or below): each along a lane of its own (nested, `bindLanes`) and a track of its own
  // in the channel left of its card (those from the left are wires, above).
  const drawnLegs = fromLeft ? [] : legs.map((l) => ({ ...l, ch: columnOf.get(nodeOf(l.address)) - 1, Q: pinOf.get(`in ${l.address}`) ?? pinOf.get(`in ${addressOf(l.address, byName)}`) })).filter((l) => l.Q);
  if (bindWired) {
    const laneOf = bindLanes(drawnLegs.map((l) => ({ c: l.x, p: mid(l.ch) })), gap);
    drawnLegs.forEach((l, i) => {
      l.yh = bindLane(laneOf[i]);
      lanes.binds = Math.max(lanes.binds, laneOf[i] + 1);
      span(l.ch, l.key, l.yh, l.Q.y);
    });
  }
  // A wired leg's track: in a block of their own by the card (its run into its pin then never along a wire's run from a source
  // at the same height), ordered so that they need not cross: of those coming from the right the one on the lane nearest the
  // edge leftmost, of those from the left the farthest.
  const legInfo = new Map(drawnLegs.filter((l) => l.yh !== undefined).map((l) => [l.key, { d: Math.abs(l.yh - bindY), right: l.x > mid(l.ch) }]));
  const trackX = new Map();
  for (const [ch, m] of spans) {
    const legNets = [...m.values()].filter((iv) => legInfo.has(iv.key));
    const order = (iv) => (legInfo.get(iv.key).right ? legInfo.get(iv.key).d : -legInfo.get(iv.key).d);
    legNets.sort((a, b) => order(a) - order(b) || (a.key < b.key ? -1 : 1));
    // Two nets share a track only a row and a half apart: one ending a pin's row above where the other starts would read as one
    // line through the other's branch dot (the owner's, 2026-11-05: @inputs.hz down onto glide.in over glide_sum's branch).
    const tracks = orderedTracks([...m.values()].filter((iv) => !legInfo.has(iv.key)), 1.5 * ROW * scale);
    const first = Math.max(0, ...[...tracks.values()].map((t) => t + 1));
    legNets.forEach((iv, t) => tracks.set(iv.key, first + t));
    const count = Math.max(1, ...[...tracks.values()].map((t) => t + 1));
    const { x0 } = chX.get(ch);
    const x1 = chX.get(ch).x1;
    const pitch = Math.min(TRACK * scale, (x1 - x0) / (count + 1));
    const centre = (x0 + x1) / 2;
    for (const [key, t] of tracks) trackX.set(`${ch} ${key}`, centre + (t - (count - 1) / 2) * pitch);
  }
  const out = drawn.map((w) => {
    const xa = trackX.get(`${w.a} ${w.srcKey}`) ?? mid(w.a);
    let points;
    if (w.b === w.a + 1) points = [w.P, { x: xa, y: w.P.y }, { x: xa, y: w.Q.y }, w.Q];
    else {
      const xb = trackX.get(`${w.b - 1} ${w.dstKey}`) ?? mid(w.b - 1);
      points = [w.P, { x: xa, y: w.P.y }, { x: xa, y: w.yh }, { x: xb, y: w.yh }, { x: xb, y: w.Q.y }, w.Q];
    }
    return {
      id: `wire ${w.c.index}${w.edge ? ` ${w.edge}` : ''}`,
      index: w.c.index,
      from: w.c.from,
      to: w.c.to,
      kind: w.kind,
      edge: w.edge,
      route: w.route,
      leg: w.leg,
      srcKey: w.srcKey,
      track: xa,
      points: simplify(points.map((p) => ({ x: p.x, y: p.y }))),
      junctions: [],
    };
  });
  // A fan-out's branches: a dot where a branch turns off another branch of its net in the middle of one of its straight runs (a T), never at a corner or an end.
  const fan = new Map();
  for (const w of out) fan.set(w.srcKey, (fan.get(w.srcKey) ?? 0) + 1);
  const dots = new Set();
  const within = (p, a, b) => {
    const eps = 1e-6;
    if (Math.abs(a.x - b.x) < eps && Math.abs(p.x - a.x) < eps) return p.y > Math.min(a.y, b.y) + eps && p.y < Math.max(a.y, b.y) - eps;
    if (Math.abs(a.y - b.y) < eps && Math.abs(p.y - a.y) < eps) return p.x > Math.min(a.x, b.x) + eps && p.x < Math.max(a.x, b.x) - eps;
    return false;
  };
  for (const w of out) {
    if (fan.get(w.srcKey) < 2) continue;
    const others = out.filter((o) => o !== w && o.srcKey === w.srcKey);
    for (const p of w.points.slice(1, -1)) {
      const id = `${p.x.toFixed(6)} ${p.y.toFixed(6)}`;
      if (dots.has(id) || !others.some((o) => o.points.slice(1).some((q, i) => within(p, o.points[i], q)))) continue;
      dots.add(id);
      w.junctions.push({ x: p.x, y: p.y });
    }
  }
  for (const w of out) {
    delete w.srcKey;
    delete w.track;
  }
  // The chip's pins' legs: the control signal's (amber), apart from the wires.
  const fromPins = out.filter((w) => w.leg).map((w) => ({ id: `bind ${w.leg.key}`, address: w.leg.address, export: w.leg.export ?? null, kind: 'control', points: w.points, junctions: w.junctions }));
  const wiresOut = out.filter((w) => !w.leg);
  for (const w of wiresOut) delete w.leg;
  const legsOut = drawnLegs.map((l) => {
    const Q = { x: l.Q.x, y: l.Q.y };
    const base = { id: `bind ${l.key}`, address: l.address, export: l.export ?? null, kind: 'control', junctions: [] };
    const xb = trackX.get(`${l.ch} ${l.key}`) ?? mid(l.ch);
    return { ...base, points: simplify([{ x: l.x, y: bindY }, { x: l.x, y: l.yh }, { x: xb, y: l.yh }, { x: xb, y: Q.y }, Q]) };
  });
  // A wired parameter on to several targets: a dot where the leg nearer the edge turns off the farther one's way.
  for (const b of legsOut) {
    const turn = b.points[1];
    if (!turn) continue;
    const farther = legsOut.find((o) => o !== b && o.points[0].x === b.points[0].x && o.points[1] && Math.abs(o.points[1].y - bindY) > Math.abs(turn.y - bindY) + 1e-9);
    if (farther && !farther.junctions.some((j) => j.y === turn.y)) farther.junctions.push({ x: turn.x, y: turn.y });
  }
  const ways = [...legsOut, ...fromPins];
  return { scale, cards, pins, wires: wiresOut, edges, lanes, reserved, binds: ways };
}

/**
 * The lanes of legs `[{c, p}]` (each from `c` on a band's edge, along its lane, on at `p`; lane 0 nearest the edge), nested
 * so that legs need not cross: of two going right whose spans meet, the one from further right runs nearer the edge (of two
 * from one point, the one going further), mirrored for those going left; a leg takes the lane after the last of those, and
 * no lane holds two legs whose spans meet (with `gap`), so the lanes are as few as that allows. `[lane]`, in the legs' order.
 */
export function bindLanes(legs, gap = 0) {
  const lanes = new Array(legs.length).fill(0);
  const all = legs.map((l, i) => ({ i, lo: Math.min(l.c, l.p), hi: Math.max(l.c, l.p), right: l.p >= l.c, c: l.c, p: l.p }));
  const right = all.filter((l) => l.right).sort((a, b) => b.c - a.c || b.p - a.p || a.i - b.i);
  const left = all.filter((l) => !l.right).sort((a, b) => a.c - b.c || a.p - b.p || a.i - b.i);
  const meet = (a, b) => a.lo <= b.hi + gap && b.lo <= a.hi + gap;
  const placed = [];
  for (const list of [right, left]) {
    const before = [];
    for (const l of list) {
      let lane = 0;
      for (const o of before) if (meet(l, o)) lane = Math.max(lane, lanes[o.i] + 1);
      while (placed.some((o) => lanes[o.i] === lane && meet(l, o))) lane++;
      lanes[l.i] = lane;
      before.push(l);
      placed.push(l);
    }
  }
  return lanes;
}

/**
 * A channel's tracks (0 leftmost) for nets `[{key, lo, hi, yl, yr}]`
 * (`yl` where a net comes in from the left, `yr` where it leaves to the
 * right), so that two nets whose spans meet never run along one another
 * or cross where they need not: of two going down, the lower one runs left
 * of the other; of two going up, the upper one. Each net takes the track
 * after the last one of the nets placed before it that it meets (with
 * `gap`); those going down are placed first, lowest first, then those
 * going up, highest first, but a net that comes in at the height another
 * leaves at before it (so left of it). Ties by key. `Map(key → track)`.
 */
export function orderedTracks(nets, gap = 0) {
  const byKey = (p, q) => (p.key < q.key ? -1 : p.key > q.key ? 1 : 0);
  const down = nets.filter((n) => n.yr >= n.yl).sort((p, q) => q.yl - p.yl || byKey(p, q));
  const up = nets.filter((n) => n.yr < n.yl).sort((p, q) => p.yl - q.yl || byKey(p, q));
  // A net that comes in at the height another leaves at runs left of it: else the one's run in and the other's run out lie on
  // one line between their tracks (the owner's, 2026-11-05: ways may cross, never run along one another).
  const order = [...down, ...up];
  for (let moved = true, guard = 0; moved && guard < order.length * order.length; guard++) {
    moved = false;
    for (let i = 0; i < order.length && !moved; i++) {
      for (let j = 0; j < i; j++) {
        const a = order[i];
        const b = order[j];
        if ((a.ins ?? [a.yl]).some((y) => (b.outs ?? [b.yr]).some((z) => Math.abs(y - z) < 1e-9))) {
          order.splice(i, 1);
          order.splice(j, 0, a);
          moved = true;
          break;
        }
      }
    }
  }
  const placed = [];
  const out = new Map();
  for (const n of order) {
    let t = 0;
    for (const o of placed) if (n.lo <= o.hi + gap && o.lo <= n.hi + gap) t = Math.max(t, out.get(o.key) + 1);
    placed.push(n);
    out.set(n.key, t);
  }
  return out;
}

/** An address as `graph` writes a top-level port (`/a.b`) for one written without the slash, or the reverse. */
function addressOf(address, byName) {
  const text = String(address ?? '');
  const name = nodeOf(text);
  const port = portOf(text);
  const inst = byName.get(name);
  const found = [...(inst?.ports ?? []), ...(inst?.parameters ?? [])].find((p) => p.name === port);
  return found?.address ?? text;
}

/** A connection's kind as the Wiring names it (ui/lib/plane-wiring.js `connectionKind`). */
function kindOf(c, portAt) {
  const from = portAt(c.from, 'out');
  const to = portAt(c.to, 'in');
  return connectionKind(from?.port ?? null, to?.port ?? null, !!to?.parameter);
}

/**
 * Instrument `entry`'s Engine laid out in its engine package (plane units,
 * Stack-relative; the owner's decision of 2026-11-03): `rect`
 * (`s.layout.rects`' entry), `pkg` (its package, ui/lib/plane-package.js
 * `packageOf`'s: the same rectangle and signal pins as the Wiring and Control
 * layers draw), `names` (its engine, `engineOf`'s), `graph`, `title`. The
 * inside is `layLevel`'s in the package's body, its connections across the
 * instrument's edge entering and leaving at the package's pins (a connection
 * the package has no pin for, spread down its edge). Without a package, the
 * instrument's rectangle is the frame. Returns `layLevel`'s with `{entry,
 * rect, pkg, frame, area, title, card}`: `card` the package as the top-level
 * chip (`chipPath`, `revealShare`): `{id: 'package ' + entry, package: true,
 * composite: true, name, x, y, w, h, body, reveal}`.
 */
export function engineLayout({ entry, rect, pkg = null, names = [], graph, title = '' }) {
  const frame = pkg ? { x: pkg.x, y: pkg.y, w: pkg.w, h: pkg.h } : { x: rect.x, y: rect.y, w: rect.w, h: rect.h };
  const body = pkg ? pkg.body : null;
  const area = body ? { ...body } : { x: rect.x + INSET, y: rect.y + TITLE, w: Math.max(1, rect.w - 2 * INSET), h: Math.max(1, rect.h - TITLE - INSET) };
  const edgeIns = new Map((pkg?.ins ?? []).map((p) => [p.address, p.y]));
  const edgeOuts = new Map((pkg?.outs ?? []).map((p) => [p.address, p.y]));
  const edgeKinds = new Map((pkg?.ins ?? []).filter((p) => p.kind).map((p) => [p.address, p.kind]));
  const level = graph?.top ?? { instances: [], connections: [] };
  // The control signal into the engine (the owner's, 2026-11-03/04): each bound parameter on the connector wired into its pin on its card's left.
  const top = pkg?.connector !== 'bottom';
  const binds = (pkg?.params ?? []).map((p) => ({ address: p.address, x: p.x }));
  const bindTo = pkg ? { binds, bindFrom: top ? 'top' : 'bottom', bindY: top ? pkg.body.y : pkg.y + pkg.h, bindWired: true } : {};
  const laid = names.length ? layLevel({ level, names, area, edgeIns, edgeOuts, edgeKinds, frame, taps: (pkg?.autos ?? []).map((a) => ({ address: a.address, y: a.y })), ...bindTo }) : { scale: 1, cards: [], pins: [], wires: [], edges: [], lanes: { over: 0, feedback: 0, binds: 0 }, binds: [] };
  const card = { id: `package ${entry}`, package: true, composite: true, name: title, module: null, ...frame, body: area, reveal: packageReveal(area, laid.scale) };
  return { entry, rect, pkg, frame, area, title, card, ...laid };
}

/** A composite's inside fades in as its card's body grows on screen: from the first width to the second (screen px). */
export const REVEAL = Object.freeze([240, 480]);
/**
 * A package's inside fades in by the scale its cards are seen at (their
 * units per screen px, from the first to the second), and never before its
 * body is twice a composite's widths on screen: closed at the Stack's fit,
 * the detail comes by zoom.
 */
export const PACKAGE_REVEAL = Object.freeze([0.5, 1]);

/** A package's reveal widths (screen px of its `body`) for its inside laid out at `scale`. */
export function packageReveal(body, scale) {
  const natural = body.w / Math.max(1e-9, scale);
  return [Math.max(2 * REVEAL[0], PACKAGE_REVEAL[0] * natural), Math.max(2 * REVEAL[1], PACKAGE_REVEAL[1] * natural)];
}
/** How deep `chipPath` looks (a guard: a graph never nests itself). */
const CHIP_DEPTH = 16;
/** A pin label's width by its name's length, in the cards' units (the 9 px label at the pin ± 6). */
const LABEL_CHAR = 5.2;
const LABEL_PAD = 8;
/** The most of a chip's body its pin labels keep clear on each side. */
const LABEL_SHARE = 0.125;

/** How far a composite's inside is revealed when its card's body is `px` wide on screen: 0 to 1 over `range` (REVEAL, a package's `reveal`), a smoothstep. */
export function revealShare(px, range = REVEAL) {
  const [a, b] = range;
  if (!(px > a)) return 0;
  if (px >= b) return 1;
  const t = (px - a) / (b - a);
  return t * t * (3 - 2 * t);
}

/**
 * A composite card's inside laid out in its body (plane units): `graph`
 * (its `composites[card.module]`: instances, connections, and the inputs,
 * parameters and outputs that lead in and out), `card` and `pins` (the
 * level's where the card was laid out, `layLevel`'s), `scale` (that
 * level's). The inside is `layLevel`'s, in the body less the room the
 * card's pin labels take, so nothing of the card moves; a wire from an input
 * enters at its pin's height on the card's left edge, a wire to an output
 * leaves at its pin's height on the right edge, an input feeding several
 * inner ports is one net, and a parameter goes on to its targets as the
 * control signal (`layLevel`'s `binds`, from the left). Returns `layLevel`'s with
 * `{module, area, frame, names}` (`names` the card's pins as the inside
 * shows them: `{side, text, address, x, y}`), or null when the graph has no
 * such composite. Pure; the same arguments give the same inside.
 */
export function insideOf({ graph, card, pins = [], scale = 1 }) {
  const def = graph?.composites?.[card?.module];
  if (!def || !card.body) return null;
  const mine = pins.filter((p) => p.card === card.id);
  const connections = [...(def.connections ?? [])];
  const edgeIns = new Map();
  const edgeOuts = new Map();
  const edgeKinds = new Map();
  // A parameter entering at its pin on the card's left edge goes on to its targets inside as the control signal (`layLevel`'s
  // `binds`): a pin row of its own on each target's card, wired from the pin; one a signal connection drives (and no control) is
  // wired on as an input is.
  const sources = [];
  const taps = [];
  for (const p of mine) {
    if (p.side === 'in' && p.parameter && (p.bind || !p.connected)) {
      for (const to of def.parameters?.find((x) => x.name === p.name)?.to ?? []) sources.push({ address: to, y: p.y, export: p.name });
    } else if (p.side === 'in') {
      for (const to of (p.parameter ? def.parameters : def.inputs)?.find((x) => x.name === p.name)?.to ?? []) {
        connections.push({ from: `@inputs.${p.name}`, to });
        if (!edgeIns.has(to)) edgeIns.set(to, p.y);
        if (p.kind && !edgeKinds.has(to)) edgeKinds.set(to, p.kind);
      }
    } else if (p.tap) {
      // An automation source's tap: deeper on, or from its inner output out to the edge at the tap's height.
      if (String(p.name).includes('/')) taps.push({ address: p.name, y: p.y });
      else {
        connections.push({ from: p.name, to: `@automation.${p.name}` });
        if (!edgeOuts.has(p.name)) edgeOuts.set(p.name, p.y);
      }
    } else {
      const out = def.outputs?.find((x) => x.name === p.name);
      if (!out?.from) continue;
      connections.push({ from: out.from, to: `@outputs.${p.name}` });
      if (!edgeOuts.has(out.from)) edgeOuts.set(out.from, p.y);
    }
  }
  // The body less the pin labels' room on each side (at most an eighth of it each: the labels fade out as the inside comes in).
  const labels = (side) => Math.max(0, ...mine.filter((p) => p.side === side).map((p) => (LABEL_PAD + LABEL_CHAR * String(p.name).length) * scale));
  const b = card.body;
  const left = Math.min(LABEL_SHARE * b.w, Math.max(0, labels('in') - (b.x - card.x)));
  const right = Math.min(LABEL_SHARE * b.w, Math.max(0, labels('out') - (card.x + card.w - (b.x + b.w))));
  const area = { x: b.x + left, y: b.y, w: Math.max(1e-6, b.w - left - right), h: Math.max(1e-6, b.h) };
  const frame = { x: card.x, y: card.y, w: card.w, h: card.h };
  const laid = layLevel({ level: { instances: def.instances ?? [], connections }, area, edgeIns, edgeOuts, edgeKinds, frame, joinEdges: true, taps, binds: sources, bindFrom: 'left' });
  // The chip's pins named inside too, at the inside's scale (the owner's, 2026-11-04: the chip's pins hand their names on).
  const names = mine.map((p) => ({ side: p.side, text: String(p.name), address: p.address, x: p.side === 'in' ? frame.x : frame.x + frame.w, y: p.y }));
  return { module: card.module, area, frame, ...laid, names };
}

/**
 * The chips the view is inside (the bar's Layer N; derived from the view,
 * no state of its own): of `cards` (a level's, plane units; at the top the
 * instruments' packages, `engineLayout`'s `card`) the chip whose body holds
 * plane point `p` and is fully revealed at scale `k` (at least its
 * `reveal[1]`, else `full`, screen px wide), then of its inside
 * (`insideFor(card, id)`: the laid inside, or null) the same, deeper.
 * `[{id, card, laid}]`, outermost first; `id` the cards' ids joined by `/`,
 * a package's own leaving its inside's as they are (the instrument's
 * instances are the project's top level).
 */
export function chipPath(cards, p, k, insideFor, full = REVEAL[1]) {
  const path = [];
  let level = cards;
  while (level && path.length < CHIP_DEPTH) {
    const card = level.find((c) => c.composite && c.body && c.body.w * k >= (c.reveal?.[1] ?? full) && p.x >= c.body.x && p.x <= c.body.x + c.body.w && p.y >= c.body.y && p.y <= c.body.y + c.body.h);
    if (!card) break;
    const id = path.length && !path.at(-1).card.package ? `${path.at(-1).id}/${card.id}` : card.id;
    const laid = insideFor(card, id);
    if (!laid) break;
    path.push({ id, card, laid });
    level = laid.cards;
  }
  return path;
}

/**
 * Where an address lies at the Engine (the sidebar's links zoom to it):
 * `layouts` (`engineLayout`'s, one per instrument), `graph`, `address` (an
 * instance `/synth`, a port `/synth.cutoff`, one inside a chip
 * `/synth/ladder.cutoff_hz`). Returns `{entry, rect, depth}`: the card that
 * holds it (Stack-relative plane units), laid inside its chips as
 * `insideOf` lays them, `depth` the chips it is in; null when no
 * instrument's engine holds it.
 */
export function locateAddress(layouts, graph, address) {
  const text = String(address ?? '').replace(/^\//, '');
  const dot = text.lastIndexOf('.');
  const path = (dot > text.lastIndexOf('/') ? text.slice(0, dot) : text).split('/').filter(Boolean);
  if (!path.length) return null;
  const e = (layouts ?? []).find((l) => l.cards.some((c) => c.name === path[0]));
  if (!e) return null;
  let laid = e;
  let card = e.cards.find((c) => c.name === path[0]);
  let depth = 0;
  for (const name of path.slice(1)) {
    const inner = card.composite ? insideOf({ graph, card, pins: laid.pins, scale: laid.scale }) : null;
    const next = inner?.cards.find((c) => c.name === name);
    if (!next) break;
    laid = inner;
    card = next;
    depth += 1;
  }
  return { entry: e.entry, rect: { x: card.x, y: card.y, w: card.w, h: card.h }, depth };
}
