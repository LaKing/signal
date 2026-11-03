// The engine package of an instrument (ADR 0051; the owner's decision of
// 2026-11-03, "one object at one place"): the instances an instrument holds
// drawn as one chip, "the cube", **at the same place with the same pins on
// the Wiring, Control and Engine layers**. One function lays it out
// (`packageOf`), and every layer draws what it returns:
//   - its place: the free spot of the instrument's front panel nearest the
//     controls it serves, never over a control, a heading or the band the
//     Control layer's binding legs run in (`placeCube`), as large as the free
//     room allows (PACKAGE_WIDTHS, largest first), so that its pins are
//     comfortable at the Stack's fit and its inside reads when zoomed;
//   - its **signal pins**: one per port of the instrument a connection joins
//     to another instrument or the Stack's edge (the Wiring layer's ports,
//     `wiringModel`), inputs on its left edge, outputs on its right, in
//     connection order. The Wiring's cables end on them, the Engine's inside
//     enters and leaves at them, the Control layer draws them;
//   - the Control layer's own pins: the bound parameters on its connector
//     (its top edge when it lies below the legs' band, else its bottom), and
//     the automation sources on its right edge, under the outputs;
//   - its `body`: where the Engine lays the inside out (ui/lib/plane-engine.js).
// Derived, never stored, never dragged (rule 4). Deterministic: controls in
// panel order, ties by index, a fixed search order. Pure; no DOM.

import { PU } from './plate.js';
import { instanceOf } from './plane-instruments.js';

/** The package: its least width, its title band, its inner pad, a signal pin's pitch, a connector pin's pitch, its least height, in pu. */
export const PACKAGE_PU = Object.freeze({ w: 18, title: 4, pad: 1.5, pin: 3, param: 1, h: 10 });
/** The widths a package tries, largest first (pu), its height then at least PACKAGE_ASPECT of its width: the largest that finds a free spot. */
export const PACKAGE_WIDTHS = Object.freeze([48, 40, 32, 26, 22]);
export const PACKAGE_ASPECT = 0.5;
/** Where the least size finds no room: the shares of it tried, largest first, never below PACKAGE_MIN plane units a side. */
export const PACKAGE_SHRINK = Object.freeze([0.85, 0.7, 0.55, 0.45, 0.35]);
export const PACKAGE_MIN = 40;
/** The room kept free right of a package with automation sources, for their wires (plane units). */
export const AUTO_ROOM = 24;
/** A leg's lane pitch in the band under the controls, the band's pad, in plane units. */
export const LANE = 4;
export const BAND_PAD = 6;
/** The inner value's readout under a control, in plane units. */
export const READOUT_H = 18;
/** The package's search grid and its clearance from the controls, in plane units. */
export const SEARCH_STEP = 16;
export const CLEARANCE = 8;

/** Whether two rectangles overlap (with `gap`). */
export function overlaps(a, b, gap = 0) {
  return a.x < b.x + b.w + gap && b.x < a.x + a.w + gap && a.y < b.y + b.h + gap && b.y < a.y + a.h + gap;
}

/**
 * The free spot of `area` for a rectangle of `size` nearest `towards`, on a
 * SEARCH_STEP grid, clear of every obstacle by CLEARANCE; ties by y, then x.
 * Null when there is none. A function of its arguments only.
 */
export function freeSpot(size, area, obstacles, towards, step = SEARCH_STEP) {
  let best = null;
  for (let y = area.y; y + size.h <= area.y + area.h + 1e-9; y += step) {
    for (let x = area.x; x + size.w <= area.x + area.w + 1e-9; x += step) {
      const r = { x, y, w: size.w, h: size.h };
      if (obstacles.some((o) => overlaps(r, o, CLEARANCE))) continue;
      const d = Math.hypot(x + size.w / 2 - towards.x, y + size.h / 2 - towards.y);
      if (!best || d < best.d - 1e-9) best = { d, r };
    }
  }
  return best?.r ?? null;
}

/** The package's place (plane units): `freeSpot`'s, else the area's bottom right. */
export function placeCube(size, area, obstacles, towards) {
  return freeSpot(size, area, obstacles, towards) ?? { x: area.x + Math.max(0, area.w - size.w), y: area.y + Math.max(0, area.h - size.h), w: size.w, h: size.h };
}

/**
 * Instrument `entry`'s engine package laid out (plane units, Stack-relative):
 * `rect` (its rectangle), `controls` (`[{id, binds, writes, rect}]`, its front
 * panel's controls measured, in panel order), `blocked` (other rectangles it
 * keeps clear of: a heading), `engine` (the instances it holds, `engineOf`'s),
 * `autos` (`automations`'), `inner` (`Map(bind → inner address)`), `ins` and
 * `outs` (the Wiring layer's ports of the instrument, `wiringModel`'s, `{id,
 * address, label, kind}`), `title`. Returns `{x, y, w, h, title, names, head,
 * body, band, connector, pitch, ins, outs, params, autos}`: the pins `{id, address,
 * x, y, ...}`, `ins` and `outs` with their port's fields; `band` the Control
 * layer's legs' band (an obstacle here, drawn there); `connector` the edge
 * the parameters' pins are on (`top` or `bottom`). The same arguments give
 * the same package, whichever layer asks.
 */
export function packageOf({ rect, controls = [], blocked = [], engine = [], autos = [], inner = new Map(), ins = [], outs = [], title = '' }) {
  const names = new Set(engine);
  const inEngine = (address) => names.has(instanceOf(address));
  const bound = controls.filter((c) => c.binds.length);
  // The parameters on the connector: each bound address in the engine, in the controls' x order.
  const params = [];
  for (const c of [...bound].sort((a, b) => a.rect.x - b.rect.x || a.rect.y - b.rect.y)) {
    for (const address of c.binds) if (inEngine(address) && !params.includes(address)) params.push(address);
  }
  // The automation sources in the engine, one pin each, in the automations' order.
  const sources = [];
  for (const a of autos) if (a.source === 'engine' && !sources.includes(a.from)) sources.push(a.from);
  const pitch = PACKAGE_PU.pin * PU;
  const rows = Math.max(ins.length, outs.length + sources.length);
  const least = {
    w: Math.max(PACKAGE_PU.w * PU, (params.length + 1) * PACKAGE_PU.param * PU),
    h: Math.max(PACKAGE_PU.h * PU, (PACKAGE_PU.title + 2 * PACKAGE_PU.pad) * PU + rows * pitch),
  };
  // The legs' band under the controls (and their inner values' readouts): the package keeps clear of it.
  const controlsBottom = bound.length ? Math.max(...bound.map((c) => c.rect.y + c.rect.h)) : rect.y + rect.h / 2;
  const automated = new Set(autos.map((a) => a.bind));
  const readoutRoom = bound.some((c) => c.binds.some((b) => inner.has(b) && automated.has(b))) ? READOUT_H : 0;
  const bandH = BAND_PAD * 2 + Math.max(1, bound.length) * LANE;
  const bandTop = Math.max(rect.y, Math.min(controlsBottom + readoutRoom, rect.y + rect.h - bandH));
  const band = { x: rect.x, y: bandTop, w: rect.w, h: bandH };
  const obstacles = [...controls.map((c) => c.rect), ...blocked, band];
  const area = { x: rect.x + 2 * PU, y: rect.y + 2 * PU, w: rect.w - 4 * PU, h: rect.h - 4 * PU };
  const towards = (size) =>
    bound.length ? { x: bound.reduce((s, c) => s + c.rect.x + c.rect.w / 2, 0) / bound.length, y: controlsBottom + bandH + size.h / 2 } : { x: rect.x + rect.w / 2, y: rect.y + rect.h / 2 };
  // As large as the free room allows: the widths tried largest first, then the least size; where even that finds no
  // room, smaller and smaller (its pins closer), on a finer grid, then over the legs' band; never over a control while
  // any of these fits (only an instrument with no room at all for the smallest takes its bottom right corner).
  const sizes = [];
  for (const wpu of PACKAGE_WIDTHS) {
    const w = Math.max(least.w, wpu * PU);
    if (!sizes.some((s) => s.w === w)) sizes.push({ w, h: Math.max(least.h, Math.round((w * PACKAGE_ASPECT) / PU) * PU) });
  }
  sizes.push(least);
  const shrunk = PACKAGE_SHRINK.map((f) => ({ w: Math.max(PACKAGE_MIN, Math.round(least.w * f)), h: Math.max(PACKAGE_MIN, Math.round(least.h * f)) }));
  // With automation sources, room on its right for their wires to turn up between the controls (the Control layer's corridors).
  const room = sources.length ? AUTO_ROOM : 0;
  const spot = (size, among, step) => {
    const at = freeSpot({ w: size.w + room, h: size.h }, area, among, towards(size), step);
    return at && { ...at, w: size.w };
  };
  let r = null;
  for (const size of sizes) if ((r = spot(size, obstacles))) break;
  for (const size of r ? [] : shrunk) if ((r = spot(size, obstacles, SEARCH_STEP / 2))) break;
  const walls = obstacles.filter((o) => o !== band);
  for (const size of r ? [] : shrunk) if ((r = spot(size, walls, SEARCH_STEP / 2))) break;
  r ??= placeCube(least, area, obstacles, towards(least));
  const below = r.y >= band.y + band.h - 1e-6;
  // A shrunk package keeps its parts in proportion: its title band, its pad and its pins' pitch at most their sizes.
  const head = Math.min(PACKAGE_PU.title * PU, r.h / 4);
  const pad = Math.min(PACKAGE_PU.pad * PU, r.w / 10, r.h / 10);
  const rowPitch = Math.min(pitch, (r.h - head - 2 * pad) / Math.max(1, rows));
  const top = r.y + head + pad;
  const rowY = (i) => top + (i + 0.5) * rowPitch;
  const pin = (p, side, i) => ({ ...p, id: p.id ?? `port ${side} ${p.address}`, side, x: side === 'in' ? r.x : r.x + r.w, y: rowY(i) });
  return {
    x: r.x,
    y: r.y,
    w: r.w,
    h: r.h,
    title,
    names: [...engine],
    head: { x: r.x, y: r.y, w: r.w, h: head },
    body: { x: r.x + pad, y: r.y + head, w: Math.max(1, r.w - 2 * pad), h: Math.max(1, r.h - head - pad) },
    band,
    connector: below ? 'top' : 'bottom',
    pitch: rowPitch,
    ins: ins.map((p, i) => pin(p, 'in', i)),
    outs: outs.map((p, i) => pin(p, 'out', i)),
    params: params.map((address, i) => ({ id: `pin ${address}`, address, x: r.x + (i + 1) * (r.w / (params.length + 1)), y: below ? r.y : r.y + r.h, side: below ? 'top' : 'bottom' })),
    autos: sources.map((address, i) => ({ id: `auto-pin ${address}`, address, x: r.x + r.w, y: rowY(outs.length + i) })),
  };
}

/** A package's signal pin of a port (`side`, `address`), or null. */
export function pinOf(pkg, side, address) {
  return (side === 'in' ? pkg?.ins : pkg?.outs)?.find((p) => p.address === address) ?? null;
}
