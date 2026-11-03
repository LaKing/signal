// The Engine layer of the Signal Project plane (ADR 0051 §1, §8; the
// owner's decision of 2026-11-03): every instrument of the focused Stack
// shows its inside **in its own rectangle**, in place, in the same
// Stack-relative plane units as the Wiring and Control layers, so the view
// never moves when the layer changes. Inside the rectangle:
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
//     the height of the Wiring layer's port of the same connection.
// Composites are one card each here. Step 2 opens them: `layLevel` lays out
// any level (`{instances, connections}`) in any area, so a composite's
// inside (`graph.composites[card.module]`) is laid out in its card's `body`
// by the same function, with its frame's inputs and outputs as the edges.
// Presentation only (rule 4): nothing is stored. Deterministic: instances
// and connections in source order, fixed tie-breaks. Pure; no DOM.

import { PU } from './plate.js';
import { instanceOf, portOf } from './plane-instruments.js';
import { assignTracks, portKind, simplify } from './plane-wiring.js';
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
function nodeOf(address) {
  return instanceOf(address);
}

/**
 * One level laid out in `area` (plane units): `level` (`{instances,
 * connections}`, as `graph` gives a level: ports with `direction`,
 * `shape`, `kind`, `rate`; connections `{from, to}`), `names` (the
 * instances drawn, in source order; default all), `edgeIns` and `edgeOuts`
 * (`Map(address → y)`: where a connection from outside enters at the
 * area's left edge, keyed by the inner input it reaches, and where one
 * leaves at the right edge, keyed by the inner output), `frame` (the
 * rectangle whose edges they are; default `area`). Returns `{scale, cards,
 * pins, wires, edges, lanes}`, plane units:
 *   - a card `{id, name, module, composite, x, y, w, h, head, body,
 *     column, ins, outs}`; `body` the area below its header, where step 2
 *     lays out a composite's inside;
 *   - a pin `{id, card, name, address, side, kind, parameter, connected,
 *     x, y}`;
 *   - a wire `{id, index, from, to, kind, route, points, junctions}`:
 *     `index` the connection's place in `level.connections`, `route` one
 *     of `adjacent`, `row`, `over`, `feedback`, `edge`;
 *   - an edge pin `{id, side, address, x, y}`, where a wire crosses the
 *     frame.
 */
export function layLevel({ level, names = null, area, edgeIns = new Map(), edgeOuts = new Map(), frame = area }) {
  const all = level?.instances ?? [];
  const wanted = names ? new Set(names) : null;
  const instances = all.filter((i) => !wanted || wanted.has(i.name));
  const inside = new Set(instances.map((i) => i.name));
  const byName = new Map(instances.map((i) => [i.name, i]));
  const connections = (level?.connections ?? []).map((c, index) => ({ ...c, index }));
  const portAt = (address) => {
    const inst = byName.get(nodeOf(address));
    const name = portOf(address);
    const port = inst?.ports?.find((p) => p.name === name);
    if (port) return { port, parameter: false };
    const param = inst?.parameters?.find((p) => p.name === name);
    return param ? { port: { ...param, direction: 'in', shape: 'signal' }, parameter: true } : null;
  };
  // The connections drawn: between two drawn instances, or crossing the edge (one per inner end: the Wiring's port).
  const internal = [];
  const entering = new Map();
  const leaving = new Map();
  for (const c of connections) {
    const a = inside.has(nodeOf(c.from)) && portAt(c.from);
    const b = inside.has(nodeOf(c.to)) && portAt(c.to);
    if (a && b) internal.push(c);
    else if (b && !inside.has(nodeOf(c.from)) && !entering.has(c.to)) entering.set(c.to, c);
    else if (a && !inside.has(nodeOf(c.to)) && !leaving.has(c.from)) leaving.set(c.from, c);
  }
  // A parameter has a pin only where a signal connection drives it.
  const driven = new Map();
  for (const c of [...internal, ...entering.values()]) {
    const at = portAt(c.to);
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
    w.srcKey = `s ${w.c.from}`;
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
  const naturalH = bandH + (overs + backs) * LANE + (overs ? LANE : 0) + (backs ? LANE : 0);
  const scale = n ? Math.min(MAX_SCALE, area.w / Math.max(1, naturalW), area.h / Math.max(1, naturalH)) : 1;
  // The width left over spreads the columns: each channel takes a share.
  const spare = Math.max(0, area.w - naturalW * scale) / (n + 1);
  // From the top of the area (under the title, beside where the Wiring's ports enter), the lanes above first.
  const top = area.y + (overs ? (overs + 1) * LANE * scale : 0);
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
  const clampY = (y) => Math.min(frame.y + frame.h - 4, Math.max(frame.y + 4, y));
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
  for (const [w, t] of assignTracks(overIv, gap)) w.yh = cardsTop - (t + 1) * LANE * scale;
  for (const [w, t] of assignTracks(backIv, gap)) w.yh = cardsBottom + (t + 1) * LANE * scale;
  // The channels' tracks: one per net, over the heights it spans there; the lowest-starting nearest its column (as the Wiring's).
  const spans = new Map();
  // `y0` where the net comes into the channel from the left, `y1` where it leaves to the right.
  const span = (ch, key, y0, y1) => {
    if (!spans.has(ch)) spans.set(ch, new Map());
    const m = spans.get(ch);
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
    const xa = trackX.get(`${w.a} ${w.srcKey}`);
    let points;
    if (w.b === w.a + 1) points = [w.P, { x: xa, y: w.P.y }, { x: xa, y: w.Q.y }, w.Q];
    else {
      const xb = trackX.get(`${w.b - 1} ${w.dstKey}`);
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
  return { scale, cards, pins, wires: out, edges, lanes: { over: overIv.length, feedback: backIv.length } };
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
  const from = portAt(c.from);
  const to = portAt(c.to);
  const kind = portKind(from?.port ?? to?.port);
  if (kind === 'audio' && to?.parameter) return 'control';
  return kind;
}

/**
 * Instrument `entry`'s Engine laid out in its rectangle (plane units,
 * Stack-relative): `rect` (`s.layout.rects`' entry), `names` (its engine,
 * `engineOf`'s), `graph`, `edgeIns` / `edgeOuts` (`Map(address → y)`, the
 * Wiring layer's input and output ports of this instrument), `title`.
 * Returns `layLevel`'s with `{entry, rect, area, title}`.
 */
export function engineLayout({ entry, rect, names = [], graph, edgeIns = new Map(), edgeOuts = new Map(), title = '' }) {
  const area = { x: rect.x + INSET, y: rect.y + TITLE, w: Math.max(1, rect.w - 2 * INSET), h: Math.max(1, rect.h - TITLE - INSET) };
  const level = graph?.top ?? { instances: [], connections: [] };
  const laid = names.length ? layLevel({ level, names, area, edgeIns, edgeOuts, frame: rect }) : { scale: 1, cards: [], pins: [], wires: [], edges: [], lanes: { over: 0, feedback: 0 } };
  return { entry, rect, area, title, ...laid };
}
