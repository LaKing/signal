// The Control plane of an instrument (ADR 0051 §6, ADR 0050 §L2 and §11;
// phase 2): the sheet under its front panel. Its controls stand where the
// panel has them (the panel's live elements, measured); its engine (the
// instances the instrument holds) is one package, "the cube", placed in the
// panel's free area nearest the controls it serves, derived and never
// stored, never dragged: the one engine package of ui/lib/plane-package.js
// (`packageOf`), at the same place with the same signal pins as the Wiring
// and Engine layers draw it; and on it:
//   - **binding legs** (the "factory routing"): each bound control to its
//     parameter's pin on the cube's connector, the legs run as a ribbon in
//     a band under the controls, nested so that no two cross (ADR 0047
//     §8b); a reader's leg (a meter, a scope) comes out of the cube; a bind
//     outside the engine (a role endpoint's `gain`) leaves the instrument
//     at its right edge;
//   - **automation**: any connection that drives a bound parameter lands
//     on that control, from the cube's output when its source is in the
//     engine, including a connection inside a composite into the export's
//     target (the mono synth's filter contour into `ladder.cutoff_hz`, which
//     `cutoff` exports), or from the instrument's edge when it comes from
//     outside (an **external control cable**: another instrument, a
//     peripheral's cable through the Stack's edge, ADR 0051 §7). Its port
//     is the Wiring level's port of the same connection, so a cable
//     continues across the level without a break. Addressing (a MIDI CC on
//     the cable) comes with M6;
//   - the **inner value** of a control whose bind reaches inside a
//     composite: the export's first target (ADR 0050 §11), read with the
//     existing `parameters` stream.
// Presentation only (rule 4). Deterministic: controls in panel order, ties
// by index, fixed search order. Pure; no DOM.

import { PU } from './plate.js';
import { instanceOf } from './plane-instruments.js';
import { portIndex, portKind, simplify } from './plane-wiring.js';
import { BAND_PAD, LANE, PACKAGE_PU, READOUT_H, packageOf } from './plane-package.js';

// The package's placement, its legs' band and the readout's room are ui/lib/plane-package.js's.
export { LANE, BAND_PAD, READOUT_H, SEARCH_STEP, CLEARANCE, overlaps, placeCube } from './plane-package.js';
/** Nested composites followed at most, finding an export's targets. */
export const MAX_DEPTH = 8;

/** The top-level instances of instrument `entry` (`owner`: `assignInstruments`'), in source order. */
export function engineOf(entry, owner) {
  return [...owner].filter(([, key]) => key === entry).map(([name]) => name);
}

/** `/a/b.port` as `{path: ['a', 'b'], port}`, or null. */
function split(address) {
  const text = String(address ?? '').replace(/^\//, '');
  const dot = text.lastIndexOf('.');
  if (dot <= 0) return null;
  return { path: text.slice(0, dot).split('/'), port: text.slice(dot + 1) };
}

/** The level an instance path lives in: `graph.top` for one name, else the composite of its parent. */
function levelOfPath(graph, path) {
  let level = graph?.top;
  let name = '';
  for (let i = 0; i < path.length - 1; i++) {
    const inst = (level?.instances ?? []).find((x) => x.name === path[i]);
    if (!inst?.composite) return null;
    name = inst.module;
    level = graph?.composites?.[name];
    if (!level) return null;
  }
  return { level, name };
}

/**
 * Where a bound parameter `address` acts (`graph`'s structure): the
 * parameter itself (`{address, level: '', local}`), then, for a composite's
 * export, each target inside it, followed down through nested composites
 * (`{address: '/synth/ladder.cutoff_hz', level: 'moog_voice', local:
 * 'ladder.cutoff_hz'}`). The first inner one is the **inner value**'s
 * address. `[]` for an address that names no parameter.
 */
export function bindTargets(address, graph) {
  const s = split(address);
  if (!s) return [];
  const found = levelOfPath(graph, s.path);
  if (!found) return [];
  const out = [{ address: `/${s.path.join('/')}.${s.port}`, level: found.name, local: `${s.path.at(-1)}.${s.port}`, prefix: s.path.slice(0, -1) }];
  const walk = (prefix, instName, levelBody, port, depth) => {
    if (depth > MAX_DEPTH) return;
    const inst = (levelBody?.instances ?? []).find((x) => x.name === instName);
    if (!inst?.composite) return;
    const body = graph?.composites?.[inst.module];
    const exported = (body?.parameters ?? []).find((p) => p.name === port);
    for (const to of exported?.to ?? []) {
      const t = split(to);
      if (!t) continue;
      const path = [...prefix, instName, ...t.path];
      out.push({ address: `/${path.join('/')}.${t.port}`, level: inst.module, local: to, prefix: [...prefix, instName] });
      walk([...prefix, instName], t.path.at(-1), body, t.port, depth + 1);
    }
  };
  walk(s.path.slice(0, -1), s.path.at(-1), found.level, s.port, 0);
  return out;
}

/** The inner value's address of bound `address`: the first target inside a composite, or null. */
export function innerAddress(address, graph) {
  return bindTargets(address, graph)[1]?.address ?? null;
}

/**
 * The automation of instrument `entry` (ADR 0050 §11, "Automation
 * wires"): for each address in `binds` (the controls' that write), every
 * connection that drives it or one of its targets inside the engine
 * (`bindTargets`), other than an export's own forwarding. `[{id, bind,
 * from, to, level, inner, source: 'engine' | 'external', kind}]`: `from`
 * and `to` absolute addresses, `level` the file's level (`''` the top
 * level, else the composite whose file holds it), `inner` when it is
 * inside a composite (edited in the Engine, never here), `source` whether
 * it comes from the instrument's engine or from outside it (an external
 * control cable). In bind order, then connection order.
 */
export function automations({ entry, binds = [], owner, graph }) {
  const out = [];
  const seen = new Set();
  const top = graph?.top ?? { instances: [], connections: [] };
  const index = portIndex(top.instances);
  for (const bind of binds) {
    for (const target of bindTargets(bind, graph)) {
      const body = target.level === '' ? top : graph?.composites?.[target.level];
      for (const c of body?.connections ?? []) {
        const to = target.level === '' ? c.to : c.to;
        if (target.level === '' ? to !== target.address : to !== target.local) continue;
        const prefix = target.prefix.length ? `/${target.prefix.join('/')}/` : '/';
        const from = target.level === '' ? c.from : `${prefix}${c.from}`;
        const toAbs = target.level === '' ? c.to : `${prefix}${c.to}`;
        const key = `${bind}\n${from}\n${toAbs}`;
        if (seen.has(key)) continue;
        seen.add(key);
        const inner = target.level !== '';
        const source = inner || owner.get(instanceOf(from)) === entry ? 'engine' : 'external';
        const fromPort = inner ? null : index.get(from)?.port;
        out.push({ id: `auto ${out.length} ${from} ${toAbs}`, bind, from, to: toAbs, written: { from: c.from, to: c.to }, level: target.level, inner, source, kind: fromPort && portKind(fromPort) !== 'audio' ? portKind(fromPort) : 'control' });
      }
    }
  }
  return out;
}

/** An automation lane's and track's pitch, in plane units. */
export const LANE_GAP = 4;

/**
 * The free vertical corridors of `rect` between heights `y0` and `y1`:
 * the x-ranges no obstacle spans there (`[{x0, x1}]`, left to right), the
 * ways a cable climbs between the control columns without crossing one.
 */
export function corridors(obstacles, rect, y0, y1) {
  const lo = Math.min(y0, y1);
  const hi = Math.max(y0, y1);
  const spans = obstacles.filter((o) => o.y < hi && o.y + o.h > lo).map((o) => [o.x - 2, o.x + o.w + 2]).sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  const out = [];
  let x = rect.x;
  for (const [a, b] of spans) {
    if (a > x) out.push({ x0: x, x1: a });
    x = Math.max(x, b);
  }
  if (x < rect.x + rect.w) out.push({ x0: x, x1: rect.x + rect.w });
  return out;
}

/**
 * Lanes for legs from controls at `c` to pins at `p` (x each) in a band:
 * the legs going right nest with the leftmost deepest, those going left
 * with the rightmost deepest, so no two legs cross; a right and a left one
 * never meet. Returns each leg's lane (0 nearest the controls).
 */
export function legLanes(legs) {
  const lanes = new Array(legs.length).fill(0);
  const rightward = legs.map((l, i) => ({ ...l, i })).filter((l) => l.p >= l.c).sort((a, b) => a.c - b.c || a.i - b.i);
  const leftward = legs.map((l, i) => ({ ...l, i })).filter((l) => l.p < l.c).sort((a, b) => b.c - a.c || a.i - b.i);
  rightward.forEach((l, k) => (lanes[l.i] = rightward.length - 1 - k));
  leftward.forEach((l, k) => (lanes[l.i] = leftward.length - 1 - k));
  return lanes;
}

/**
 * Instrument `entry`'s Control plane laid out (plane units, Stack-relative):
 * `rect` (its rectangle), `controls` (`[{id, tag, binds: [addresses],
 * writes, label, rect}]`, measured in panel order), `blocked` (other
 * rectangles the cube keeps clear of: a heading; a piano roll or a step
 * grid is no obstacle, the cube may lie over it), `engine`
 * (`engineOf`'s names), `autos` (`automations`'), `edgeYs` (`Map(address →
 * y)`: the Wiring level's input ports, where a cable from outside enters),
 * `inner` (`Map(bind → inner address)`), `pkg` (the instrument's engine
 * package, `packageOf`'s with the Wiring's ports, as every layer draws it;
 * without one it is laid out here, with no signal pins). Returns `{rect,
 * cube, pins, autoPins, edgePins, legs, wires, readouts, band, bounds}`:
 * `cube` the package.
 */
export function controlLayout({ rect, controls = [], blocked = [], engine = [], autos = [], edgeYs = new Map(), inner = new Map(), title = '', pkg = null }) {
  const names = new Set(engine);
  const bound = controls.filter((c) => c.binds.length);
  const inEngine = (address) => names.has(instanceOf(address));
  // The package: its place, its connector's pins (each bound parameter in the engine) and the legs' band are the one package's.
  const cube = pkg ?? packageOf({ rect, controls, blocked, engine, autos, inner, title });
  const band = cube.band;
  // The inner value is shown where the engine's own modulation acts on a bound parameter (elsewhere it equals the effective value, ADR 0050 §11).
  const automated = new Set(autos.map((a) => a.bind));
  const readable = (b) => inner.has(b) && automated.has(b);
  const legCount = bound.length;
  const pins = cube.params.map((p) => ({ ...p, kind: 'param' }));
  const pinOf = new Map(pins.map((p) => [p.address, p]));
  // A bind outside the engine leaves the instrument at its right edge, one pin per address.
  const outside = [];
  for (const c of bound) for (const a of c.binds) if (!inEngine(a) && !outside.includes(a)) outside.push(a);
  const edgePins = outside.map((address, i) => ({ id: `edge ${address}`, kind: 'edge-out', address, x: rect.x + rect.w, y: band.y + BAND_PAD + (legCount + 1 + i) * LANE }));
  const edgeOf = new Map(edgePins.map((p) => [p.address, p]));
  // The legs: down from a control's via into the band, across, into the pin.
  const legs = [];
  for (const c of bound) {
    const address = c.binds.find((a) => pinOf.has(a)) ?? c.binds.find((a) => edgeOf.has(a));
    if (!address) continue;
    const to = pinOf.get(address) ?? edgeOf.get(address);
    legs.push({ id: `leg ${c.id}`, control: c.id, address, reads: !c.writes, via: { x: c.rect.x + c.rect.w / 2, y: c.rect.y + c.rect.h }, to });
  }
  const lanes = legLanes(legs.map((l) => ({ c: l.via.x, p: l.to.x })));
  const wires = [];
  legs.forEach((l, i) => {
    const y = band.y + BAND_PAD + (lanes[i] + 0.5) * LANE;
    const points = l.to.kind === 'edge-out' ? [l.via, { x: l.via.x, y }, { x: l.to.x - PU, y }, { x: l.to.x - PU, y: l.to.y }, { x: l.to.x, y: l.to.y }] : [l.via, { x: l.via.x, y }, { x: l.to.x, y }, { x: l.to.x, y: l.to.y }];
    l.points = simplify(points);
    l.lane = lanes[i];
  });
  // Automation and external cables never run over a control (the coordinator's review of phase 2): each lands on its
  // control's top from a lane in the gap just above its row, reached by a free vertical corridor between the control
  // columns (from the cube) or along the instrument's left edge (from outside), so it crosses no control.
  const controlOf = (bind) => bound.find((c) => c.binds.includes(bind));
  const autoPins = [];
  const externalPins = [];
  const landings = new Map();
  const rowLanes = new Map();
  const corridorUse = new Map();
  let externalIndex = 0;
  const walls = [...controls.map((c) => c.rect), ...blocked, cube];
  autos.forEach((a) => {
    const c = controlOf(a.bind);
    if (!c) return;
    const n = landings.get(c.id) ?? 0;
    landings.set(c.id, n + 1);
    const land = { x: c.rect.x + c.rect.w / 2 + (n * PU) / 2, y: c.rect.y };
    const row = Math.round(c.rect.y);
    const j = rowLanes.get(row) ?? 0;
    rowLanes.set(row, j + 1);
    const lane = Math.max(rect.y + 2, c.rect.y - LANE_GAP * (j + 1));
    let start;
    let pts;
    if (a.source === 'engine') {
      // One pin per source on the cube's edge; each of its connections its own wire.
      let pin = autoPins.find((q) => q.address === a.from);
      if (!pin) {
        // The package's pin of this source (its right edge, under the signal outputs).
        const at = cube.autos.find((q) => q.address === a.from);
        pin = { id: `auto-pin ${a.from}`, kind: 'auto', auto: a.id, address: a.from, x: at?.x ?? cube.x + cube.w, y: at?.y ?? cube.y + cube.h / 2 };
        autoPins.push(pin);
      }
      const gaps = corridors(walls, rect, lane, pin.y).filter((g) => g.x1 - g.x0 >= 2 * LANE_GAP);
      // Reached from the pin along its row without crossing a control: up to the first one standing right of it there.
      const reach = Math.min(Infinity, ...walls.filter((o) => o !== cube && o.x >= pin.x && o.y - 2 < pin.y && o.y + o.h + 2 > pin.y).map((o) => o.x - 2));
      const right = gaps.filter((g) => g.x1 > pin.x && g.x0 + LANE_GAP < reach);
      const choice = (right.length ? right : gaps).sort((g, h) => Math.abs((g.x0 + g.x1) / 2 - land.x) - Math.abs((h.x0 + h.x1) / 2 - land.x) || g.x0 - h.x0)[0];
      let x;
      if (choice) {
        const key = `${Math.round(choice.x0)}`;
        const used = corridorUse.get(key) ?? 0;
        corridorUse.set(key, used + 1);
        x = Math.min(choice.x1 - LANE_GAP, Math.max(choice.x0, pin.x) + LANE_GAP * (used + 1));
        if (right.includes(choice)) x = Math.min(x, reach - 1);
      } else x = rect.x + rect.w - LANE_GAP * (autoPins.length + 1);
      start = pin;
      // Out of the pin to the corridor, below everything else; a corridor left of the pin is reached under the cube.
      const under = cube.y + cube.h + LANE_GAP * (corridorUse.size + 1);
      pts = x >= pin.x ? [pin, { x, y: pin.y }, { x, y: lane }, { x: land.x, y: lane }, land] : [pin, { x: pin.x + LANE_GAP, y: pin.y }, { x: pin.x + LANE_GAP, y: under }, { x, y: under }, { x, y: lane }, { x: land.x, y: lane }, land];
    } else {
      const y = edgeYs.get(a.to) ?? edgeYs.get(a.bind) ?? rect.y + (PACKAGE_PU.title + externalIndex * 3) * PU;
      const pin = { id: `ext-pin ${a.id}`, kind: 'external', auto: a.id, address: a.to, from: a.from, x: rect.x, y };
      externalPins.push(pin);
      // Along the instrument's left edge, in its margin, then along the row's lane.
      const x = rect.x + LANE_GAP * (externalIndex + 1);
      externalIndex++;
      start = pin;
      pts = [pin, { x, y }, { x, y: lane }, { x: land.x, y: lane }, land];
    }
    wires.push({ id: a.id, kind: 'auto', auto: a, control: c.id, from: start, points: simplify(pts) });
  });
  const readouts = [];
  for (const c of bound) {
    const address = c.binds.find(readable);
    if (!address) continue;
    readouts.push({ id: `readout ${c.id}`, control: c.id, bind: address, address: inner.get(address), x: c.rect.x, y: c.rect.y + c.rect.h + 2, w: c.rect.w, h: READOUT_H - 4 });
  }
  return { rect, cube, band, pins, autoPins, externalPins, edgePins, legs, wires, readouts, bounds: rect };
}
