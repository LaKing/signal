// The Wiring level of the Signal Project plane (ADR 0051 §1, §4, §7, §8;
// phase 2): one Stack from behind, the back of the rack. Each instrument
// keeps its rectangle (no rectangle moves between Stack and Wiring) and is
// drawn as a faint box inset in it, its audio engine as one package in it
// (ui/lib/plane-package.js: the same package, at the same place with the
// same pins, as the Control and Engine layers draw); its ports are
// **derived, never written**: one per instrument port that a connection
// joins to another instrument of the Stack or to the Stack's edge, the
// package's signal pins, inputs on its left edge, outputs on its right, in
// connection order; a cable runs from a pin level across the box's edge
// into the channels (the owner's decision of 2026-11-03). Only the cables between
// instruments are drawn, of every kind (audio, control, events, vector),
// and each cable to the Stack's edge continues the Setup level's edge pin
// (`edgePins`, `pinPlaces`): the pins derive from the one connection, at
// every level (§7). The cables run in channels on both sides of the boxes,
// inside the Stack's rectangle, and cross the Stack in the rails between
// instruments (ADR 0047 §6's tracks: one vertical track per net in a
// channel, a rail's crossings spread over its height), so a cable never
// passes over a box. Presentation only (rule 4): nothing here is stored.
// Deterministic: connections in source order, ties by index, a fixed
// track assignment. Pure; no DOM.

import { PU } from './plate.js';
import { RAIL_PU } from './plane-file.js';
import { instanceOf, portOf } from './plane-instruments.js';
import { pinOf } from './plane-package.js';

/** A channel's track pitch, its inner pad, and its least and largest width (a share of the Stack's), in pu / plane units. */
export const TRACK_PU = 1;
export const CHANNEL_PAD_PU = 1;
export const CHANNEL_MIN_PU = 3;
export const CHANNEL_MAX_SHARE = 0.2;
/** A box's title band, and its ports' pitch, in pu. */
export const TITLE_PU = 5;
export const PORT_PITCH_PU = 3;
/**
 * The Wiring level's text, under the Setup level's rule (`textSize`,
 * ui/lib/plane-layout.js): a size in plane units, never below `min` px on
 * screen, never past `cap` (the room the box gives it).
 */
export const WIRING_TEXT = Object.freeze({
  title: Object.freeze({ base: 16, min: 12, cap: 30 }),
  port: Object.freeze({ base: 12, min: 10, cap: 20 }),
});
/** The kinds of cable, in the order a legend lists them. */
export const CABLE_KINDS = Object.freeze(['audio', 'control', 'events', 'vector']);

/** The kind of a port as `graph` gives it, from its shape and kind: `events`, `vector`, `control` or `audio`. */
export function portKind(port) {
  const shape = port?.shape;
  if (shape === 'events') return 'events';
  if (shape && typeof shape === 'object' && 'vector' in shape) return 'vector';
  if (shape === 'vector') return 'vector';
  // A signal at the frame rate whose kind says nothing else (`generic`) carries audio; control, gate and the rest are control.
  return (port?.kind === 'audio' || port?.kind === 'generic' || port?.kind == null) && (port?.rate ?? 1) === 1 ? 'audio' : 'control';
}

/** Every top-level port and parameter of `instances` by address: `Map(address → {port, parameter})`. */
export function portIndex(instances = []) {
  const index = new Map();
  for (const i of instances) {
    for (const p of i.ports ?? []) index.set(p.address, { port: p, parameter: false, instance: i.name });
    for (const p of i.parameters ?? []) index.set(p.address, { port: { ...p, direction: 'in', shape: 'signal' }, parameter: true, instance: i.name });
  }
  return index;
}

/**
 * A cable's kind: the source's (`events`, `vector`), else `control` for a
 * signal into a parameter or from a port that is no audio, else `audio`.
 */
export function cableKind(connection, index) {
  const from = index.get(connection.from)?.port ?? null;
  const to = index.get(connection.to) ?? null;
  const kind = portKind(from ?? to?.port);
  if (kind === 'audio' && to?.parameter) return 'control';
  return kind;
}

/**
 * The Wiring level's model of Stack `stack` (ADR 0051 §1, §4): `owner`
 * (`assignInstruments`'), `stackOf` (`stackMembers`'), `instances` and
 * `connections` (`graph.top`'s), `pins` (the Stack's `edgePins`). Returns
 * `{ports: Map(entry → {ins, outs}), cables}`: a port `{id, entry, side,
 * address, label, kind}` per instrument port a connection joins to another
 * instrument of the Stack or to its edge, in connection order; a cable
 * `{id, index, from, to, kind, a: {entry, address} | {edge, pin}, b}` per
 * such connection: `index` its place in `connections`, `edge` set on the
 * end at the Stack's edge (`'in'` or `'out'`, its pin's address).
 */
export function wiringModel(stack, owner, stackOf, instances = [], connections = [], pins = []) {
  const index = portIndex(instances);
  const order = new Map(stack.instruments.map((e, i) => [e, i]));
  const entryOf = (address) => {
    const name = instanceOf(address);
    if (stackOf.get(name) !== stack.name) return null;
    const key = owner.get(name) ?? null;
    return key !== null && order.has(key) ? key : null;
  };
  const ports = new Map(stack.instruments.map((e) => [e, { ins: [], outs: [] }]));
  const portOfAddress = new Map();
  const port = (entry, side, address, kind) => {
    const key = `${side} ${address}`;
    if (!portOfAddress.has(key)) {
      const p = { id: `port ${key}`, entry, side, address, label: portOf(address), kind };
      portOfAddress.set(key, p);
      ports.get(entry)[side === 'in' ? 'ins' : 'outs'].push(p);
    }
    return portOfAddress.get(key);
  };
  const edgeIn = new Set(pins.filter((p) => p.side === 'in').map((p) => p.address));
  const edgeOut = new Set(pins.filter((p) => p.side === 'out').map((p) => p.address));
  const cables = [];
  connections.forEach((c, i) => {
    const a = entryOf(c.from);
    const b = entryOf(c.to);
    const kind = cableKind(c, index);
    if (a && b && a !== b) {
      port(a, 'out', c.from, kind);
      port(b, 'in', c.to, kind);
      cables.push({ id: `cable ${i}`, index: i, from: c.from, to: c.to, kind, a: { entry: a, address: c.from }, b: { entry: b, address: c.to } });
    } else if (a && !b && edgeOut.has(c.from)) {
      port(a, 'out', c.from, kind);
      // One cable per edge pin: the Setup draws the pin's fan-out outside.
      if (!cables.some((x) => x.b.edge === 'out' && x.b.pin === c.from)) cables.push({ id: `cable edge out ${c.from}`, index: i, from: c.from, to: c.to, kind, a: { entry: a, address: c.from }, b: { edge: 'out', pin: c.from } });
    } else if (b && !a && edgeIn.has(c.to)) {
      port(b, 'in', c.to, kind);
      if (!cables.some((x) => x.a.edge === 'in' && x.a.pin === c.to)) cables.push({ id: `cable edge in ${c.to}`, index: i, from: c.from, to: c.to, kind, a: { edge: 'in', pin: c.to }, b: { entry: b, address: c.to } });
    }
  });
  // Two ports of one instrument and side sharing a name: both say their instance.
  for (const { ins, outs } of ports.values()) {
    for (const list of [ins, outs]) {
      const shared = list.filter((p) => list.some((q) => q !== p && q.label === p.label));
      for (const p of shared) p.label = String(p.address).replace(/^\//, '');
    }
  }
  return { ports, cables };
}

/**
 * Tracks for intervals (`[{key, lo, hi}]`), as a channel's verticals take
 * them: each interval the lowest track no interval overlapping it (with
 * `gap`) has, intervals taken in the given order. `Map(key → track)`.
 */
export function assignTracks(intervals, gap = 0) {
  const tracks = [];
  const out = new Map();
  for (const iv of intervals) {
    let t = 0;
    for (; t < tracks.length; t++) if (!tracks[t].some((o) => iv.lo <= o.hi + gap && o.lo <= iv.hi + gap)) break;
    (tracks[t] ??= []).push(iv);
    out.set(iv.key, t);
  }
  return out;
}

/** A port's y on its box (plane units, Stack-relative): the title band, then a row per port, compressed to fit the box. */
function portYs(rect, n) {
  if (!n) return [];
  const top = rect.y + TITLE_PU * PU;
  const room = Math.max(PU, rect.h - TITLE_PU * PU - PU);
  const pitch = Math.min(PORT_PITCH_PU * PU, room / n);
  return Array.from({ length: n }, (_, i) => top + pitch * (i + 0.5));
}

/**
 * The Wiring level laid out (plane units, Stack-relative): `model`
 * (`wiringModel`'s), `layout` (`stackLayout`'s: `{w, rects}`), `pinYs`
 * (`Map(side + ' ' + address → y)` of the Stack's edge pins, `pinPlaces`'),
 * `packages` (`Map(entry → package)`, ui/lib/plane-package.js `packageOf`'s
 * with these ports: the audio engine each instrument holds, the one the
 * Control and Engine layers draw at the same place). Returns `{channel,
 * boxes: [{entry, x, y, w, h}], ports: [{...port, x, y, edge}], cables:
 * [{...cable, points: [{x, y}], junctions: [{x, y}]}], bounds}`. A port is
 * its package's signal pin (`edge` where its cable crosses the box's edge,
 * level with it); without a package, a place on the box's edge. Each output
 * leaves its pin to the right, across the box's edge into the right
 * channel, its net's track there, then along the rail next to its target
 * (above it for a target below, below it for one above) across to the left
 * channel, its target's track there, and into the input's pin from the left.
 */
export function wiringLayout(model, layout, pinYs = new Map(), packages = new Map()) {
  const w = layout.w;
  const rects = layout.rects;
  const rectOf = new Map(rects.map((r, i) => [r.entry, { ...r, i }]));
  const rail = RAIL_PU * PU;
  // The ports' places first (they decide the channels' intervals): the package's pins, else the box's edge (x set once the channel's width is known).
  const placed = new Map();
  for (const r of rects) {
    const p = model.ports.get(r.entry) ?? { ins: [], outs: [] };
    const pkg = packages.get(r.entry) ?? null;
    const yi = portYs(r, p.ins.length);
    const yo = portYs(r, p.outs.length);
    const at = (q, side, y) => {
      const pin = pinOf(pkg, side, q.address);
      return pin ? { ...q, y: pin.y, pinX: pin.x } : { ...q, y };
    };
    p.ins.forEach((q, i) => placed.set(q.id, at(q, 'in', yi[i])));
    p.outs.forEach((q, i) => placed.set(q.id, at(q, 'out', yo[i])));
  }
  const portAt = (side, address) => placed.get(`port ${side} ${address}`);
  // Each crossing cable's rail: next to its target, on the side its source is.
  const railOf = (cable) => {
    if (cable.a.edge || cable.b.edge) return null;
    const ra = rectOf.get(cable.a.entry);
    const rb = rectOf.get(cable.b.entry);
    if (!ra || !rb) return null;
    return rb.i > ra.i ? rb.i : rb.i + 1;
  };
  const railY = (i) => {
    if (i <= 0) return (rects[0]?.y ?? rail) - rail / 2;
    const above = rects[i - 1];
    return above ? above.y + above.h + rail / 2 : rail / 2;
  };
  // A rail's crossings spread over its height, in cable order.
  const byRail = new Map();
  for (const c of model.cables.filter((x) => (x.a.edge || placed.has(`port out ${x.a.address}`)) && (x.b.edge || placed.has(`port in ${x.b.address}`)))) {
    const r = railOf(c);
    if (r === null) continue;
    if (!byRail.has(r)) byRail.set(r, []);
    byRail.get(r).push(c.id);
  }
  const crossY = new Map();
  for (const [r, ids] of byRail) {
    const pitch = Math.min(TRACK_PU * PU * 0.5, rail / (ids.length + 1));
    ids.forEach((id, k) => crossY.set(id, railY(r) + (k - (ids.length - 1) / 2) * pitch));
  }
  // The right channel: one track per net (an output's cables share their vertical, a fan-out a tree).
  const right = new Map();
  const left = new Map();
  for (const c of model.cables) {
    if ((!c.a.edge && !portAt('out', c.a.address)) || (!c.b.edge && !portAt('in', c.b.address))) continue;
    if (!c.a.edge) {
      const from = portAt('out', c.a.address);
      const to = c.b.edge ? pinYs.get(`out ${c.b.pin}`) ?? from.y : crossY.get(c.id);
      const key = c.a.address;
      const iv = right.get(key) ?? { key, lo: from.y, hi: from.y };
      iv.lo = Math.min(iv.lo, to ?? from.y);
      iv.hi = Math.max(iv.hi, to ?? from.y);
      right.set(key, iv);
    }
    if (!c.b.edge) {
      const into = portAt('in', c.b.address);
      const from = c.a.edge ? pinYs.get(`in ${c.a.pin}`) ?? into.y : crossY.get(c.id);
      const key = c.b.address;
      const iv = left.get(key) ?? { key, lo: into.y, hi: into.y };
      iv.lo = Math.min(iv.lo, from ?? into.y);
      iv.hi = Math.max(iv.hi, from ?? into.y);
      left.set(key, iv);
    }
  }
  // Lowest-starting nets nearest the box: an output's stub then crosses no trunk from above.
  const sorted = (m) => [...m.values()].sort((p, q) => q.lo - p.lo || (p.key < q.key ? -1 : p.key > q.key ? 1 : 0));
  const gap = PU / 2;
  const rt = assignTracks(sorted(right), gap);
  const lt = assignTracks(sorted(left), gap);
  const nTracks = Math.max(1, ...[...rt.values(), ...lt.values()].map((t) => t + 1));
  const channel = Math.min(w * CHANNEL_MAX_SHARE, Math.max(CHANNEL_MIN_PU * PU, (2 * CHANNEL_PAD_PU + nTracks * TRACK_PU) * PU));
  const pitch = Math.min(TRACK_PU * PU, (channel - 2 * CHANNEL_PAD_PU * PU) / nTracks);
  const xRight = (t) => w - channel + CHANNEL_PAD_PU * PU + (t + 0.5) * pitch;
  const xLeft = (t) => channel - CHANNEL_PAD_PU * PU - (t + 0.5) * pitch;
  const boxes = rects.map((r) => ({ entry: r.entry, x: channel, y: r.y, w: w - 2 * channel, h: r.h }));
  const ports = [...placed.values()].map(({ pinX, ...p }) => {
    const edge = { x: p.side === 'in' ? channel : w - channel, y: p.y };
    return { ...p, x: pinX ?? edge.x, edge };
  });
  const portXY = new Map(ports.map((p) => [p.id, p]));
  // A cable whose instrument has no rectangle (not measured yet) is not drawn.
  const placedCable = (c) => (c.a.edge || placed.has(`port out ${c.a.address}`)) && (c.b.edge || placed.has(`port in ${c.b.address}`));
  const cables = model.cables.filter(placedCable).map((c) => {
    const pts = [];
    if (c.a.edge) {
      const y = pinYs.get(`in ${c.a.pin}`) ?? portXY.get(`port in ${c.b.address}`).y;
      pts.push({ x: 0, y });
      const x = xLeft(lt.get(c.b.address));
      const into = portXY.get(`port in ${c.b.address}`);
      pts.push({ x, y }, { x, y: into.y }, { x: into.x, y: into.y });
    } else {
      const from = portXY.get(`port out ${c.a.address}`);
      const x = xRight(rt.get(c.a.address));
      pts.push({ x: from.x, y: from.y }, { x, y: from.y });
      if (c.b.edge) {
        const y = pinYs.get(`out ${c.b.pin}`) ?? from.y;
        pts.push({ x, y }, { x: w, y });
      } else {
        const y = crossY.get(c.id);
        const into = portXY.get(`port in ${c.b.address}`);
        const xl = xLeft(lt.get(c.b.address));
        pts.push({ x, y }, { x: xl, y }, { x: xl, y: into.y }, { x: into.x, y: into.y });
      }
    }
    return { ...c, points: simplify(pts), junctions: [] };
  });
  // A net's branches: a dot where a cable of a fan-out leaves its trunk anywhere but at the trunk's ends.
  const fan = new Map();
  for (const c of cables) if (!c.a.edge) fan.set(c.a.address, (fan.get(c.a.address) ?? 0) + 1);
  for (const c of cables) {
    if (c.a.edge || fan.get(c.a.address) < 2) continue;
    const iv = right.get(c.a.address);
    const x = xRight(rt.get(c.a.address));
    const turn = c.points.find((p, i) => i > 0 && Math.abs(p.x - x) < 1e-6 && Math.abs(p.y - c.points[0].y) > 1e-6);
    if (turn && turn.y > iv.lo + 1e-6 && turn.y < iv.hi - 1e-6) c.junctions.push({ x: turn.x, y: turn.y });
  }
  const top = rects.length ? rects[0].y - rail : 0;
  const bottom = rects.length ? rects.at(-1).y + rects.at(-1).h + rail : 0;
  return { channel, boxes, ports, cables, bounds: { x: 0, y: top, w, h: bottom - top } };
}

/** `points` without repeated points or collinear middles. */
export function simplify(points) {
  const out = [];
  for (const p of points) {
    const last = out.at(-1);
    if (last && Math.abs(last.x - p.x) < 1e-9 && Math.abs(last.y - p.y) < 1e-9) continue;
    if (out.length >= 2) {
      const a = out.at(-2);
      const b = last;
      if ((Math.abs(a.x - b.x) < 1e-9 && Math.abs(b.x - p.x) < 1e-9) || (Math.abs(a.y - b.y) < 1e-9 && Math.abs(b.y - p.y) < 1e-9)) out.pop();
    }
    out.push(p);
  }
  return out;
}

/**
 * An orthogonal polyline's SVG path, its corners rounded by up to `r` (plane units), or with `corner` 'chamfer' cut at
 * 45° as a circuit board's traces (the owner's question, 2026-11-03), each cut up to `2r` and at most half of either side.
 */
export function polylinePath(points, r = 0, corner = 'round') {
  const f = (n) => Math.round(n * 100) / 100;
  if (!points.length) return '';
  let d = `M${f(points[0].x)} ${f(points[0].y)}`;
  for (let i = 1; i < points.length; i++) {
    const p = points[i];
    const next = points[i + 1];
    if (!next || r <= 0) {
      d += ` L${f(p.x)} ${f(p.y)}`;
      continue;
    }
    const prev = points[i - 1];
    const a = Math.min(corner === 'chamfer' ? 2 * r : r, Math.hypot(p.x - prev.x, p.y - prev.y) / 2, Math.hypot(next.x - p.x, next.y - p.y) / 2);
    const ux = Math.sign(p.x - prev.x);
    const uy = Math.sign(p.y - prev.y);
    const vx = Math.sign(next.x - p.x);
    const vy = Math.sign(next.y - p.y);
    if (corner === 'chamfer') d += ` L${f(p.x - ux * a)} ${f(p.y - uy * a)} L${f(p.x + vx * a)} ${f(p.y + vy * a)}`;
    else d += ` L${f(p.x - ux * a)} ${f(p.y - uy * a)} Q${f(p.x)} ${f(p.y)} ${f(p.x + vx * a)} ${f(p.y + vy * a)}`;
  }
  return d;
}
