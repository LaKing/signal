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
//   - their ports as pins (inputs left, outputs right), and a parameter's
//     pin only where a signal connection drives it (a modulation inside the
//     engine); never the parameter bundle, legs or controls, which are the
//     Control layer's;
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
import { assignTracks, portKind, simplify } from './plane-wiring.js';
import { legLanes } from './plane-control.js';
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
 * entering from one source are one net, a fan-out; default each its own).
 * Returns `{scale, cards,
 * pins, wires, edges, lanes}`, plane units:
 *   - a card `{id, name, module, composite, x, y, w, h, head, body,
 *     column, ins, outs}`; `body` the area below its header, where a
 *     composite's inside is laid out (`insideOf`);
 *   - a pin `{id, card, name, address, side, kind, parameter, connected,
 *     x, y}`;
 *   - a wire `{id, index, from, to, kind, route, points, junctions}`:
 *     `index` the connection's place in `level.connections`, `route` one
 *     of `adjacent`, `row`, `over`, `feedback`, `edge`;
 *   - an edge pin `{id, side, address, x, y}`, where a wire crosses the
 *     frame.
 */
export function layLevel(args) {
  // Laid out with room for every lane a wire might need, then again with the lanes the wires took (when they still fit).
  const first = layPass(args, null);
  if (first.reserved.over === first.lanes.over && first.reserved.feedback === first.lanes.feedback) return strip(first);
  const second = layPass(args, first.lanes);
  return strip(second.lanes.over <= first.lanes.over && second.lanes.feedback <= first.lanes.feedback ? second : first);
}

function strip({ reserved, ...laid }) {
  return laid;
}

/** One pass of `layLevel`: `reserve` the lanes kept above and below the cards (`{over, feedback}`), or null for as many as might be needed. */
function layPass({ level, names = null, area, edgeIns = new Map(), edgeOuts = new Map(), frame = area, joinEdges = false }, reserve) {
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
  // A parameter has a pin only where a signal connection drives it.
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
  const connectedIns = new Set([...internal.map((c) => c.to), ...entering.keys()]);
  const connectedOuts = new Set([...internal.map((c) => c.from), ...leaving.keys()]);
  const natural = new Map();
  for (const i of instances) {
    const ins = [...(i.ports ?? []).filter((p) => p.direction === 'in'), ...(driven.get(i.name) ?? [])];
    const outs = (i.ports ?? []).filter((p) => p.direction === 'out');
    const w = nodeBy.get(i.name)?.w ?? 96;
    const h = HEADER + ROW * Math.max(1, ins.length, outs.length) + PAD;
    natural.set(i.name, { ins, outs, w, h });
  }
  const colW = columns.map((col) => Math.max(0, ...col.map((i) => natural.get(i.name).w)));
  const colH = columns.map((col) => col.reduce((s, i) => s + natural.get(i.name).h, 0) + Math.max(0, col.length - 1) * GAP_Y);
  // The wires, in connection order: internal ones, then those entering, then those leaving.
  const wires = [
    ...internal.map((c) => ({ c, a: columnOf.get(nodeOf(c.from)), b: columnOf.get(nodeOf(c.to)), edge: null })),
    ...[...entering.values()].map((c) => ({ c, a: -1, b: columnOf.get(nodeOf(c.to)), edge: 'in' })),
    ...[...leaving.values()].map((c) => ({ c, a: columnOf.get(nodeOf(c.from)), b: n, edge: 'out' })),
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
    w.srcKey = w.edge === 'in' && !joinEdges ? `e ${w.c.to}` : `s ${w.c.from}`;
    w.dstKey = `d ${w.c.to}`;
    if (w.b === w.a + 1) net(w.a, w.srcKey);
    else {
      net(w.a, w.srcKey);
      net(w.b - 1, w.dstKey);
      if (w.b > w.a) overs++;
      else backs++;
    }
  }
  const chW = (ch) => {
    const tracks = nets.get(ch)?.size ?? 0;
    const least = ch === -1 || ch === n - 1 ? EDGE_MIN : CHANNEL_MIN;
    return Math.max(least, 2 * CHANNEL_PAD + tracks * TRACK);
  };
  const channelsW = Array.from({ length: n + 1 }, (_, k) => chW(k - 1));
  const naturalW = colW.reduce((s, x) => s + x, 0) + channelsW.reduce((s, x) => s + x, 0);
  const bandH = Math.max(HEADER, ...colH);
  const reserved = reserve ?? { over: overs, feedback: backs };
  const naturalH = bandH + (reserved.over + reserved.feedback) * LANE + (reserved.over ? LANE : 0) + (reserved.feedback ? LANE : 0);
  const scale = n ? Math.min(MAX_SCALE, area.w / Math.max(1, naturalW), area.h / Math.max(1, naturalH)) : 1;
  // The width left over spreads the columns: each channel takes a share.
  const spare = Math.max(0, area.w - naturalW * scale) / (n + 1);
  // From the top of the area (under the title, beside where the Wiring's ports enter), the lanes above first.
  const top = area.y + (reserved.over ? (reserved.over + 1) * LANE * scale : 0);
  const cardsTop = top;
  const cardsBottom = top + bandH * scale;
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
          kind: parameter ? 'control' : portKind(p),
          parameter,
          connected: side === 'in' ? connectedIns.has(address) : connectedOuts.has(address),
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
  // Ends: an inner pin, or the frame's edge at the Wiring's height (else spread down the frame, in order).
  // Kept off the frame's corners: 4 plane units, less in a small frame (a chip's card, deep inside).
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
    w.kind = kindOf(w.c, portAt);
    if (w.edge === 'in') {
      const y = clampY(edgeIns.get(w.c.to) ?? nextIn());
      w.P = { x: frame.x, y };
      edges.push({ id: `edge in ${w.c.to}`, side: 'in', address: w.c.to, from: w.c.from, x: frame.x, y });
    } else w.P = pinOf.get(`out ${w.c.from}`) ?? pinOf.get(`out ${addressOf(w.c.from, byName)}`);
    if (w.edge === 'out') {
      const y = clampY(edgeOuts.get(w.c.from) ?? nextOut());
      w.Q = { x: frame.x + frame.w, y };
      edges.push({ id: `edge out ${w.c.from}`, side: 'out', address: w.c.from, to: w.c.to, x: frame.x + frame.w, y });
    } else w.Q = pinOf.get(`in ${w.c.to}`) ?? pinOf.get(`in ${addressOf(w.c.to, byName)}`);
  }
  const drawn = wires.filter((w) => w.P && w.Q);
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
  const overIv = [];
  const backIv = [];
  for (const w of drawn) {
    if (w.b === w.a + 1) w.route = w.edge ? 'edge' : 'adjacent';
    else if (w.b > w.a) {
      const y = [w.P.y, w.Q.y].find((yy) => clear(yy, w.a, w.b));
      if (y !== undefined) {
        w.route = w.edge ? 'edge' : 'row';
        w.yh = y;
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
  const lanes = { over: 0, feedback: 0 };
  for (const [w, t] of assignTracks(overIv, gap)) {
    w.yh = cardsTop - (t + 1) * LANE * scale;
    lanes.over = Math.max(lanes.over, t + 1);
  }
  for (const [w, t] of assignTracks(backIv, gap)) {
    w.yh = cardsBottom + (t + 1) * LANE * scale;
    lanes.feedback = Math.max(lanes.feedback, t + 1);
  }
  // The channels' tracks: one per net, over the heights it spans there; the lowest-starting nearest its column (as the Wiring's).
  const spans = new Map();
  // `y0` where the net comes into the channel from the left, `y1` where it leaves to the right.
  const span = (ch, key, y0, y1) => {
    if (!spans.has(ch)) spans.set(ch, new Map());
    const m = spans.get(ch);
    // A wire straight across the channel (along a row) takes no track of its own.
    if (Math.abs(y0 - y1) < 1e-9 && !m.has(key)) return;
    const iv = m.get(key) ?? { key, lo: Math.min(y0, y1), hi: Math.max(y0, y1), yl: y0, yr: y1 };
    iv.lo = Math.min(iv.lo, y0, y1);
    iv.hi = Math.max(iv.hi, y0, y1);
    m.set(key, iv);
  };
  for (const w of drawn) {
    if (w.b === w.a + 1) span(w.a, w.srcKey, w.P.y, w.Q.y);
    else {
      span(w.a, w.srcKey, w.P.y, w.yh);
      span(w.b - 1, w.dstKey, w.yh, w.Q.y);
    }
  }
  const trackX = new Map();
  for (const [ch, m] of spans) {
    const tracks = orderedTracks([...m.values()], (PAD * scale) / 2);
    const count = Math.max(1, ...[...tracks.values()].map((t) => t + 1));
    const { x0, x1 } = chX.get(ch);
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
      srcKey: w.srcKey,
      track: xa,
      points: simplify(points.map((p) => ({ x: p.x, y: p.y }))),
      junctions: [],
    };
  });
  // A fan-out's branches: a dot where a branch leaves its trunk anywhere but at the trunk's ends.
  const fan = new Map();
  for (const w of out) fan.set(w.srcKey, (fan.get(w.srcKey) ?? 0) + 1);
  for (const w of out) {
    if (fan.get(w.srcKey) < 2) continue;
    const ys = out.filter((o) => o.srcKey === w.srcKey).flatMap((o) => o.points.filter((p) => Math.abs(p.x - w.track) < 1e-6).map((p) => p.y));
    const lo = Math.min(...ys);
    const hi = Math.max(...ys);
    const turn = w.points.find((p, i) => i > 0 && Math.abs(p.x - w.track) < 1e-6 && Math.abs(p.y - w.points[0].y) > 1e-6);
    if (turn && turn.y > lo + 1e-6 && turn.y < hi - 1e-6) w.junctions.push({ x: turn.x, y: turn.y });
  }
  for (const w of out) {
    delete w.srcKey;
    delete w.track;
  }
  return { scale, cards, pins, wires: out, edges, lanes, reserved };
}

/**
 * A channel's tracks (0 leftmost) for nets `[{key, lo, hi, yl, yr}]`
 * (`yl` where a net comes in from the left, `yr` where it leaves to the
 * right), so that two nets whose spans meet never run along one another
 * or cross where they need not: of two going down, the lower one runs left
 * of the other; of two going up, the upper one. Each net takes the track
 * after the last one of the nets placed before it that it meets (with
 * `gap`); those going down are placed first, lowest first, then those
 * going up, highest first. Ties by key. `Map(key → track)`.
 */
export function orderedTracks(nets, gap = 0) {
  const byKey = (p, q) => (p.key < q.key ? -1 : p.key > q.key ? 1 : 0);
  const down = nets.filter((n) => n.yr >= n.yl).sort((p, q) => q.yl - p.yl || byKey(p, q));
  const up = nets.filter((n) => n.yr < n.yl).sort((p, q) => p.yl - q.yl || byKey(p, q));
  const placed = [];
  const out = new Map();
  for (const n of [...down, ...up]) {
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

/** A connection's kind as the Wiring names it: the source's (`events`, `vector`), `control` into a parameter or from a port that is no audio, else `audio`. */
function kindOf(c, portAt) {
  const from = portAt(c.from, 'out');
  const to = portAt(c.to, 'in');
  const kind = portKind(from?.port ?? to?.port);
  if (kind === 'audio' && to?.parameter) return 'control';
  return kind;
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
  // The control signal's band by the connector, kept clear of the cards (`bindsOf`).
  const band = bindBand(pkg);
  const area = body ? { x: body.x, y: band && pkg.connector === 'top' ? body.y + band : body.y, w: body.w, h: Math.max(1, body.h - band) } : { x: rect.x + INSET, y: rect.y + TITLE, w: Math.max(1, rect.w - 2 * INSET), h: Math.max(1, rect.h - TITLE - INSET) };
  const edgeIns = new Map((pkg?.ins ?? []).map((p) => [p.address, p.y]));
  const edgeOuts = new Map((pkg?.outs ?? []).map((p) => [p.address, p.y]));
  const level = graph?.top ?? { instances: [], connections: [] };
  const laid = names.length ? layLevel({ level, names, area, edgeIns, edgeOuts, frame }) : { scale: 1, cards: [], pins: [], wires: [], edges: [], lanes: { over: 0, feedback: 0 } };
  const card = { id: `package ${entry}`, package: true, composite: true, name: title, module: null, ...frame, body: area, reveal: packageReveal(area, laid.scale) };
  return { entry, rect, pkg, frame, area, title, card, ...laid, ...bindsOf(pkg, laid, band) };
}

/**
 * The control signal into the engine (the owner's, 2026-11-04): each bound parameter on the package's connector (`pkg.params`)
 * to a pin on its instance's card edge facing the connector, in the connector's order along each card, so that the legs into
 * one card never cross. Returns `{binds, bindPins}`: a bind `{id, address, kind: 'control', points}` (connector pin, a turn
 * by the connector, a turn by the card, the card's pin), a pin `{id, card, name, address, side, kind, parameter, connected,
 * bind, x, y}`.
 */
export function bindsOf(pkg, laid, band = bindBand(pkg)) {
  const binds = [];
  const bindPins = [];
  if (!pkg?.params?.length || !laid.cards.length) return { binds, bindPins };
  const top = pkg.connector === 'top';
  const k = laid.scale;
  // Out of the connector below the title band (it passes under it), the lanes in the band between it and the cards.
  const start = top ? pkg.body.y : pkg.y + pkg.h;
  const gap = band / (pkg.params.length + 1);
  const byCard = new Map();
  for (const p of pkg.params) {
    const c = laid.cards.find((card) => card.name === instanceOf(p.address));
    if (!c) continue;
    if (!byCard.has(c)) byCard.set(c, []);
    byCard.get(c).push(p);
  }
  for (const [c, list] of byCard) {
    const ordered = [...list].sort((a, b) => a.x - b.x);
    const y = top ? c.y : c.y + c.h;
    ordered.forEach((p, i) => {
      const x = c.x + CARD_RX * k + ((i + 0.5) * (c.w - 2 * CARD_RX * k)) / ordered.length;
      bindPins.push({ id: `bind ${p.address}`, card: c.id, name: portOf(p.address), address: p.address, side: 'in', kind: 'control', parameter: true, connected: true, bind: true, x, y });
    });
  }
  // Nested so that no two cross (the Control layer's legs' rule): straight out, along its lane, straight into the card.
  const lanes = legLanes(bindPins.map((q) => ({ c: pkg.params.find((p) => p.address === q.address).x, p: q.x })));
  bindPins.forEach((q, i) => {
    const from = pkg.params.find((p) => p.address === q.address);
    const laneY = top ? start + (lanes[i] + 1) * gap : start - (lanes[i] + 1) * gap;
    binds.push({ id: q.id, address: q.address, kind: 'control', points: simplify([{ x: from.x, y: start }, { x: from.x, y: laneY }, { x: q.x, y: laneY }, { x: q.x, y: q.y }]) });
  });
  return { binds, bindPins };
}

/** The height of the control signal's band in a package (0 without bound parameters): a lane per parameter, at most a third of its body. */
export function bindBand(pkg) {
  if (!pkg?.params?.length) return 0;
  return Math.min(pkg.body.h / 3, (pkg.params.length + 1) * PU * 0.5);
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
 * (or a driven parameter) enters at its pin's height on the card's left
 * edge, a wire to an output leaves at its pin's height on the right edge,
 * and an input feeding several inner ports is one net. Returns `layLevel`'s
 * with `{module, area, frame}`, or null when the graph has no such
 * composite. Pure; the same arguments give the same inside.
 */
export function insideOf({ graph, card, pins = [], scale = 1 }) {
  const def = graph?.composites?.[card?.module];
  if (!def || !card.body) return null;
  const mine = pins.filter((p) => p.card === card.id);
  const connections = [...(def.connections ?? [])];
  const edgeIns = new Map();
  const edgeOuts = new Map();
  for (const p of mine) {
    if (p.side === 'in') {
      const from = (p.parameter ? def.parameters : def.inputs)?.find((x) => x.name === p.name);
      for (const to of from?.to ?? []) {
        connections.push({ from: `@inputs.${p.name}`, to });
        if (!edgeIns.has(to)) edgeIns.set(to, p.y);
      }
    } else {
      const out = def.outputs?.find((x) => x.name === p.name);
      if (!out?.from) continue;
      connections.push({ from: out.from, to: `@outputs.${p.name}` });
      if (!edgeOuts.has(out.from)) edgeOuts.set(out.from, p.y);
    }
  }
  // The body less the pin labels' room on each side (at most 30% of it each).
  const labels = (side) => Math.max(0, ...mine.filter((p) => p.side === side).map((p) => (LABEL_PAD + LABEL_CHAR * String(p.name).length) * scale));
  const b = card.body;
  const left = Math.min(0.3 * b.w, Math.max(0, labels('in') - (b.x - card.x)));
  const right = Math.min(0.3 * b.w, Math.max(0, labels('out') - (card.x + card.w - (b.x + b.w))));
  const area = { x: b.x + left, y: b.y, w: Math.max(1e-6, b.w - left - right), h: Math.max(1e-6, b.h) };
  const frame = { x: card.x, y: card.y, w: card.w, h: card.h };
  const laid = layLevel({ level: { instances: def.instances ?? [], connections }, area, edgeIns, edgeOuts, frame, joinEdges: true });
  return { module: card.module, area, frame, ...laid };
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
