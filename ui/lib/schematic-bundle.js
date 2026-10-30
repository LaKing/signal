// Wires that scale (M2.13-2, ADR 0030 §3; the owner's "let there be a
// down-scaling wiring"): at small scale the wires between the same two
// boxes (into one composite from one box is the same pair) are drawn as one
// bundle with a count, whatever their routing (a pair's feedback wires as
// one loop, on the lane of its first). Zooming in fans the bundle out: from
// BUNDLE_PX to FAN_PX on screen per wire (a pin row's height), in quarter
// steps, each wire's ends (and lane) move from the bundle's to its own; at
// FAN_PX the wires are drawn on their own. A traced or selected wire
// (`solo`) is always drawn on its own as well, after (above) its bundle,
// which still counts it. Pure; no DOM.

import { ROW, laneY } from './schematic-layout.js';
import { curveDistance, flatten, wireGeometry } from './schematic-wire.js';
import { orthGeometry, zCorners } from './board-route.js';

/**
 * A wire's geometry between `a` and `b`: a board's wires (ADR 0047 §6.4,
 * `orth`) as orthogonal routes over `x` (their trunk), a bus's members
 * fanning out from it; every other as `wireGeometry`.
 */
function geometry(a, b, options, orth, x = null) {
  return orth ? orthGeometry(zCorners(a, b, { feedback: options.feedback, under: options.under, x })) : wireGeometry(a, b, options);
}

/** At most this many px per wire on screen, a pair's wires are one bundle… */
export const BUNDLE_PX = 4;
/** …and from this many on, each is drawn on its own. */
export const FAN_PX = 8;
/** The fan-out moves in this many steps between them. */
export const FAN_STEPS = 4;
/** Below this many px a row on screen a level is too small to read: its bundles' counts are not shown (they would only pile up). */
export const COUNT_PX = 3.2;

/** Whether a level drawn at scale `k` shows its bundles' counts. */
export function countsShown(k) {
  return ROW * k >= COUNT_PX;
}

/** How far a bundle is fanned out at scale `k`: 0 (one bundle) … 1 (every wire on its own), in quarter steps. */
export function fanOf(k) {
  const t = (ROW * k - BUNDLE_PX) / (FAN_PX - BUNDLE_PX);
  return Math.max(0, Math.min(1, Math.floor(t * FAN_STEPS) / FAN_STEPS));
}

const end = (wire) => wire.ends?.b ?? wire.curve.segs.at(-1).to;
const start = (wire) => wire.ends?.a ?? wire.curve.start;
const lerp = (a, b, t) => ({ x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t });
const mean = (ps) => ({ x: ps.reduce((s, p) => s + p.x, 0) / ps.length, y: ps.reduce((s, p) => s + p.y, 0) / ps.length });

/** The key of a wire's pair of boxes. */
export function pairKey(wire) {
  return `${wire.from.node}\n${wire.to.node}`;
}

/**
 * A bundle of `wires` (a pair's, in the layout's order) at fan `t`: its
 * `curve` (from the mean of their outputs to the mean of their inputs; a
 * loop on the first's lane when one is feedback), each member's curve with
 * its ends `t` of the way from the bundle's to its own, the count, a `mid`
 * point for it, and the `box` of everything drawn.
 */
function bundleOf(key, wires, t, bottom, startOf = start, endOf = end, avoid = null) {
  const a = mean(wires.map(startOf));
  const b = mean(wires.map(endOf));
  const feedback = wires.some((w) => w.feedback);
  const lanes = wires.filter((w) => w.feedback && w.under !== null && w.under !== undefined).map((w) => w.under);
  const under = lanes.length ? Math.min(...lanes) : laneY(bottom, 1);
  // Around an open composite's frame, as its wires go (none of them crossing into it).
  const around = feedback || wires.some((w) => w.crossing) ? null : avoid;
  // ADR 0047 §6.4: a board's pair is a bus along its first wire's trunk.
  const orth = wires.every((w) => w.orth);
  const trunk = orth ? (wires[0].trunkX ?? null) : null;
  const main = geometry(a, b, { feedback, under, avoid: around }, orth, trunk);
  const members =
    t > 0
      ? wires.map((w) => {
          const own = w.feedback ? w.under : under;
          const x = orth && trunk !== null && w.trunkX !== null ? trunk + (w.trunkX - trunk) * t : null;
          return { wire: w, ...geometry(lerp(a, startOf(w), t), lerp(b, endOf(w), t), { feedback, under: under + ((own ?? under) - under) * t, avoid: around }, orth, x) };
        })
      : [];
  const boxes = [main.box, ...members.map((m) => m.box)];
  const box = { x0: Math.min(...boxes.map((x) => x.x0)), y0: Math.min(...boxes.map((x) => x.y0)), x1: Math.max(...boxes.map((x) => x.x1)), y1: Math.max(...boxes.map((x) => x.y1)) };
  const points = flatten(main.curve);
  return { kind: 'bundle', key, from: wires[0].from.node, to: wires[0].to.node, wires, count: wires.length, t, feedback, curve: main.curve, points, members, box, mid: points[Math.floor(points.length / 2)] };
}

/**
 * What to draw of a level's `wires` (a layout's, or those on screen) at
 * scale `k`: `[wire | bundle]`, single wires and bundles in the layout's
 * order (a bundle where its first wire is), then the `solo` wires (a Set of
 * `wireKey`s: traced, selected) of the bundles, on their own. At fan 1
 * every wire is single. `bottom` is the layout's (feedback lanes). A
 * `cache` (a Map, kept by the caller per layout) gives back the same bundle
 * object for the same wires at the same fan, so a drawing keyed by it is
 * not built again. `ends(wire)` (M2.13-2, a collapsed port group's pin,
 * ui/lib/schematic-groups.js) may give a wire other ends, `{a, b, key}`:
 * the wires to one collapsed group (`key`) bundle at its pin, and a single
 * wire is drawn to it. `avoid` (an opened layout's, ui/lib/schematic-open.js):
 * the frame a bundle goes around, as its wires do.
 */
export function bundleWires(wires, k, { solo = new Set(), bottom = 0, cache = null, fanned = false, ends = null, avoid = null, written = (w) => `${w.written.from}\n${w.written.to}` } = {}) {
  // `fanned`: every wire on its own whatever the scale (a box of the level dragged).
  const t = fanned ? 1 : fanOf(k);
  if (t >= 1 && !ends) return [...wires];
  const moved = new Map();
  if (ends) for (const w of wires) {
    const e = ends(w);
    if (e) moved.set(w, e);
  }
  // Fanned out, a collapsed group's wires still end at its pin: the others are drawn as they are.
  if (t >= 1 && !moved.size) return [...wires];
  const startOf = (w) => moved.get(w)?.a ?? start(w);
  const endOf = (w) => moved.get(w)?.b ?? end(w);
  const keyOf = (w) => (moved.get(w)?.key ? `${pairKey(w)}\n${moved.get(w).key}` : pairKey(w));
  // A wire alone, its ends moved to a group's pin: drawn there (its id kept).
  const single = (w) => (moved.has(w) ? { ...w, ...geometry(startOf(w), endOf(w), { feedback: w.feedback, under: w.under ?? laneY(bottom, 1), avoid: w.feedback || w.crossing ? null : avoid }, Boolean(w.orth), w.trunkX ?? null) } : w);
  const pairs = new Map();
  for (const w of wires) {
    if (w.labelled || w.whole) continue;
    const key = keyOf(w);
    if (!pairs.has(key)) pairs.set(key, []);
    pairs.get(key).push(w);
  }
  const items = [];
  const above = [];
  for (const w of wires) {
    const pair = pairs.get(keyOf(w));
    // A net drawn as labels (ADR 0047 §6.3) is never in a bus: its stubs are drawn as they are.
    if (!pair || pair.length < 2 || (t >= 1 && !moved.has(w)) || w.labelled || w.whole) {
      items.push(single(w));
      continue;
    }
    if (pair[0] !== w) continue;
    const key = keyOf(w);
    let bundle = cache?.get(key);
    const same = (b) => b.wires.every((m) => startOf(m) === b.ends.get(m)?.a && endOf(m) === b.ends.get(m)?.b);
    if (!bundle || bundle.t !== t || bundle.wires.length !== pair.length || bundle.wires.some((m, i) => m !== pair[i]) || !same(bundle)) {
      bundle = bundleOf(key, pair, t, bottom, startOf, endOf, avoid);
      bundle.ends = new Map(pair.map((m) => [m, { a: startOf(m), b: endOf(m) }]));
      cache?.set(key, bundle);
    }
    items.push(bundle);
    for (const m of pair) if (solo.has(written(m))) above.push(single(m));
  }
  return [...items, ...above];
}

/** The wire of `bundle` whose own curve passes nearest `p` (level units). */
export function nearestWire(bundle, p) {
  let best = null;
  let bestD = Infinity;
  for (const w of bundle.wires) {
    const d = curveDistance(w.curve, p, 1e9);
    if (d < bestD) {
      bestD = d;
      best = w;
    }
  }
  return best;
}

/** Whether `bundle` as drawn passes within `limit` (level units) of `p`. */
export function nearBundle(bundle, p, limit) {
  if (curveDistance(bundle.curve, p, limit) <= limit) return true;
  return bundle.members.some((m) => curveDistance(m.curve, p, limit) <= limit);
}

/**
 * What a plain click on `hit` (a bundle's) selects of `selected`: its
 * wires; when exactly they are already selected (a second click), the one
 * `nearest()` gives (the wire under the pointer), alone.
 */
export function bundleClick(selected, hit, nearest) {
  const key = (h) => `${h.path}\n${h.wire.written.from}\n${h.wire.written.to}`;
  const mine = new Set(hit.wires.map(key));
  const again = selected.length === mine.size && selected.every((s) => s.kind === 'wire' && mine.has(key(s)));
  return again ? [nearest()] : hit.wires;
}

/** Where a bundle's count may go, in level units, best first: its middle, then a third and a quarter of the way from either end. */
export function countPlaces(bundle) {
  const ps = bundle.points;
  const at = (f) => ps[Math.min(ps.length - 1, Math.max(0, Math.round(f * (ps.length - 1))))];
  return [bundle.mid, at(1 / 3), at(2 / 3), at(1 / 4), at(3 / 4)];
}

/** A bundle's hover text: its wires, one per line. */
export function bundleText(bundle) {
  return [`${bundle.count} wires`, ...bundle.wires.map((w) => w.label)].join('\n');
}
