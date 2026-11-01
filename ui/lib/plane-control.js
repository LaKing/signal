// The Control plane of an instrument (ADR 0051 §6, ADR 0050 §L2 and §11;
// phase 2): the sheet under its front panel. Its controls stand where the
// panel has them (the panel's live elements, measured); its engine (the
// instances the instrument holds) is one package, "the cube", placed in the
// panel's free area nearest the controls it serves, derived and never
// stored, never dragged; and on it:
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

/** The cube: its least width, its title band, a connector pin's pitch, an automation pin's pitch, in pu. */
export const CUBE_PU = Object.freeze({ w: 18, title: 4, pin: 1, auto: 3, pad: 1.5 });
/** A leg's lane pitch in the band under the controls, the band's pad, in plane units. */
export const LANE = 4;
export const BAND_PAD = 6;
/** The inner value's readout under a control, in plane units. */
export const READOUT_H = 18;
/** The cube's search grid and its clearance from the controls, in plane units. */
export const SEARCH_STEP = 16;
export const CLEARANCE = 8;
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

/** Whether two rectangles overlap (with `gap`). */
export function overlaps(a, b, gap = 0) {
  return a.x < b.x + b.w + gap && b.x < a.x + a.w + gap && a.y < b.y + b.h + gap && b.y < a.y + a.h + gap;
}

/**
 * The cube's place (plane units): the free spot of `area` (the instrument's
 * rectangle less a margin) nearest `towards`, on a SEARCH_STEP grid, never
 * over an obstacle; ties by y, then x. Without one, the area's bottom
 * right. A function of its arguments only.
 */
export function placeCube(size, area, obstacles, towards) {
  let best = null;
  for (let y = area.y; y + size.h <= area.y + area.h + 1e-9; y += SEARCH_STEP) {
    for (let x = area.x; x + size.w <= area.x + area.w + 1e-9; x += SEARCH_STEP) {
      const r = { x, y, w: size.w, h: size.h };
      if (obstacles.some((o) => overlaps(r, o, CLEARANCE))) continue;
      const d = Math.hypot(x + size.w / 2 - towards.x, y + size.h / 2 - towards.y);
      if (!best || d < best.d - 1e-9) best = { d, r };
    }
  }
  return best?.r ?? { x: area.x + Math.max(0, area.w - size.w), y: area.y + Math.max(0, area.h - size.h), w: size.w, h: size.h };
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
 * `inner` (`Map(bind → inner address)`). Returns `{rect, cube, pins, autoPins,
 * edgePins, legs, wires, readouts, band, bounds}`.
 */
export function controlLayout({ rect, controls = [], blocked = [], engine = [], autos = [], edgeYs = new Map(), inner = new Map(), title = '' }) {
  const names = new Set(engine);
  const bound = controls.filter((c) => c.binds.length);
  // The parameters on the cube's connector: each bound address in the engine, in the controls' x order.
  const inEngine = (address) => names.has(instanceOf(address));
  const params = [];
  for (const c of [...bound].sort((a, b) => a.rect.x - b.rect.x || a.rect.y - b.rect.y)) {
    for (const address of c.binds) if (inEngine(address) && !params.includes(address)) params.push(address);
  }
  const engineAutos = autos.filter((a) => a.source === 'engine');
  const pinPitch = CUBE_PU.pin * PU;
  const size = {
    w: Math.max(CUBE_PU.w * PU, (params.length + 1) * pinPitch),
    h: Math.max(10 * PU, (CUBE_PU.title + CUBE_PU.pad * 2) * PU + new Set(engineAutos.map((a) => a.from)).size * CUBE_PU.auto * PU),
  };
  const controlsBottom = bound.length ? Math.max(...bound.map((c) => c.rect.y + c.rect.h)) : rect.y + rect.h / 2;
  // The inner value is shown where the engine's own modulation acts on a bound parameter (elsewhere it equals the effective value, ADR 0050 §11).
  const automated = new Set(autos.map((a) => a.bind));
  const readable = (b) => inner.has(b) && automated.has(b);
  const readoutRoom = bound.some((c) => c.binds.some(readable)) ? READOUT_H : 0;
  const legCount = bound.length;
  const bandH = BAND_PAD * 2 + Math.max(1, legCount) * LANE;
  const bandTop = Math.max(rect.y, Math.min(controlsBottom + readoutRoom, rect.y + rect.h - bandH));
  const band = { x: rect.x, y: bandTop, w: rect.w, h: bandH };
  const obstacles = [...controls.map((c) => c.rect), ...blocked, band];
  const towards = bound.length
    ? { x: bound.reduce((s, c) => s + c.rect.x + c.rect.w / 2, 0) / bound.length, y: controlsBottom + bandH + size.h / 2 }
    : { x: rect.x + rect.w / 2, y: rect.y + rect.h / 2 };
  const area = { x: rect.x + 2 * PU, y: rect.y + 2 * PU, w: rect.w - 4 * PU, h: rect.h - 4 * PU };
  const cube = { ...placeCube(size, area, obstacles, towards), title, names: [...engine] };
  const below = cube.y >= band.y + band.h - 1e-6;
  const connectorY = below ? cube.y : cube.y + cube.h;
  const pins = params.map((address, i) => ({ id: `pin ${address}`, kind: 'param', address, x: cube.x + (i + 1) * (cube.w / (params.length + 1)), y: connectorY, side: below ? 'top' : 'bottom' }));
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
        pin = { id: `auto-pin ${a.from}`, kind: 'auto', auto: a.id, address: a.from, x: cube.x + cube.w, y: cube.y + (CUBE_PU.title + CUBE_PU.pad + (autoPins.length + 0.5) * CUBE_PU.auto) * PU };
        autoPins.push(pin);
      }
      const gaps = corridors(walls, rect, lane, pin.y).filter((g) => g.x1 - g.x0 >= 2 * LANE_GAP);
      const right = gaps.filter((g) => g.x1 > pin.x);
      const choice = (right.length ? right : gaps).sort((g, h) => Math.abs((g.x0 + g.x1) / 2 - land.x) - Math.abs((h.x0 + h.x1) / 2 - land.x) || g.x0 - h.x0)[0];
      let x;
      if (choice) {
        const key = `${Math.round(choice.x0)}`;
        const used = corridorUse.get(key) ?? 0;
        corridorUse.set(key, used + 1);
        x = Math.min(choice.x1 - LANE_GAP, Math.max(choice.x0, pin.x) + LANE_GAP * (used + 1));
      } else x = rect.x + rect.w - LANE_GAP * (autoPins.length + 1);
      start = pin;
      // Out of the pin to the corridor, below everything else; a corridor left of the pin is reached under the cube.
      const under = cube.y + cube.h + LANE_GAP * (corridorUse.size + 1);
      pts = x >= pin.x ? [pin, { x, y: pin.y }, { x, y: lane }, { x: land.x, y: lane }, land] : [pin, { x: pin.x + LANE_GAP, y: pin.y }, { x: pin.x + LANE_GAP, y: under }, { x, y: under }, { x, y: lane }, { x: land.x, y: lane }, land];
    } else {
      const y = edgeYs.get(a.to) ?? edgeYs.get(a.bind) ?? rect.y + (CUBE_PU.title + externalIndex * 3) * PU;
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
