// Wires that scale (M2.13-2, ADR 0030 §3, docs/ui-controls.md "Schematic"):
// a pair's wires as one bundle at small scale, whatever their routing; the
// fan-out by pixels per wire, in quarter steps; traced or selected wires on
// their own above; the hit on a bundle and its nearest wire; the studio's.

import test from 'node:test';
import assert from 'node:assert/strict';
import { BUNDLE_PX, FAN_PX, bundleClick, bundleText, bundleWires, countPlaces, countsShown, fanOf, nearBundle, nearestWire } from './schematic-bundle.js';
import { hitTest, wireHit } from './schematic-hit.js';
import { DragFollow } from './schematic-drag.js';
import { bake, bakedPath } from './schematic-draw.js';
import { ROW, layout } from './schematic-layout.js';
import { readFileSync } from 'node:fs';

/** The studio's `graph` structure (crates/signal-project/tests/structure.rs keeps it equal to `Project::structure`). */
const STUDIO = JSON.parse(readFileSync(new URL('./fixtures/studio-graph.json', import.meta.url), 'utf8'));

const port = (name, direction) => ({ name, direction, shape: 'signal', kind: 'generic', rate: 1 });
const inst = (name, ins, outs) => ({ name, module: 'std.x', composite: false, ports: [...ins.map((p) => port(p, 'in')), ...outs.map((p) => port(p, 'out'))], parameters: [] });
const conn = (from, to) => ({ from, to, converters: [] });
const keyOf = (w) => `${w.written.from}\n${w.written.to}`;

/** seq → voice by three wires, voice → out by one. */
const LEVEL = {
  instances: [inst('seq', [], ['hz', 'gate', 'vel']), inst('voice', ['hz', 'gate', 'vel'], ['out']), inst('out', ['l'], [])],
  connections: [conn('/seq.hz', '/voice.hz'), conn('/seq.gate', '/voice.gate'), conn('/seq.vel', '/voice.vel'), conn('/voice.out', '/out.l')],
};
const at = (px) => px / ROW;

test('the fan: one bundle up to BUNDLE_PX per wire, every wire from FAN_PX, quarter steps between', () => {
  assert.equal(fanOf(at(1)), 0);
  assert.equal(fanOf(at(BUNDLE_PX)), 0);
  assert.equal(fanOf(at(5)), 0.25);
  assert.equal(fanOf(at(6.5)), 0.5);
  assert.equal(fanOf(at(7.9)), 0.75);
  assert.equal(fanOf(at(FAN_PX)), 1);
  assert.equal(fanOf(at(40)), 1);
});

test('a pair of boxes with several wires is one bundle with a count; a lone wire stays one', () => {
  const drawn = layout(LEVEL);
  const items = bundleWires(drawn.wires, at(2), { bottom: drawn.bottom });
  assert.equal(items.length, 2);
  const [bundle, single] = items;
  assert.equal(bundle.kind, 'bundle');
  assert.equal(bundle.count, 3);
  assert.deepEqual(bundle.wires.map((w) => w.written.from), ['/seq.hz', '/seq.gate', '/seq.vel']);
  assert.equal(bundle.members.length, 0);
  assert.equal(single.written.from, '/voice.out');
  assert.match(bundleText(bundle), /^3 wires\n\/seq\.hz → \/voice\.hz\n/);
  // Fanned out by FAN_PX: every wire on its own.
  assert.deepEqual(bundleWires(drawn.wires, at(FAN_PX), { bottom: drawn.bottom }), drawn.wires);
});

test('fanning out moves each wire from the bundle to its own pins', () => {
  const drawn = layout(LEVEL);
  const half = bundleWires(drawn.wires, at(6.5), { bottom: drawn.bottom })[0];
  assert.equal(half.t, 0.5);
  assert.equal(half.members.length, 3);
  const folded = bundleWires(drawn.wires, at(1), { bottom: drawn.bottom })[0];
  for (const m of half.members) {
    const own = m.wire.curve.segs.at(-1).to;
    const end = m.curve.segs.at(-1).to;
    const middle = folded.curve.segs.at(-1).to;
    assert.ok(Math.abs(end.y - (middle.y + own.y) / 2) < 1e-9, `${m.wire.label} half way`);
  }
});

test('a traced or selected wire is drawn on its own after its bundle, which still counts it', () => {
  const drawn = layout(LEVEL);
  const gate = drawn.wires.find((w) => w.written.from === '/seq.gate');
  const items = bundleWires(drawn.wires, at(2), { bottom: drawn.bottom, solo: new Set([keyOf(gate)]) });
  assert.equal(items.at(-1), gate);
  assert.equal(items[0].count, 3);
});

test('feedback-routed wires of one pair bundle too: one loop on the first lane, fanning out to their own lanes', () => {
  // `back` placed left of `front`: both of its wires from front loop around below.
  const level = { instances: [inst('back', ['a', 'b'], []), inst('front', [], ['x', 'y'])], connections: [conn('/front.x', '/back.a'), conn('/front.y', '/back.b')] };
  const drawn = layout(level, { back: { x: 0, y: 0 }, front: { x: 400, y: 0 } });
  assert.ok(drawn.wires.every((w) => w.feedback));
  const [bundle] = bundleWires(drawn.wires, at(2), { bottom: drawn.bottom });
  assert.equal(bundle.kind, 'bundle');
  assert.equal(bundle.count, 2);
  assert.ok(bundle.feedback);
  const lowest = Math.max(...bundle.curve.segs.map((s) => s.to.y));
  assert.equal(lowest, Math.min(...drawn.wires.map((w) => w.under)));
  const fanned = bundleWires(drawn.wires, at(7.9), { bottom: drawn.bottom })[0];
  for (const m of fanned.members) {
    const deepest = Math.max(...m.curve.segs.map((s) => s.to.y));
    assert.ok(deepest > lowest - 1e-9 && deepest <= m.wire.under + 1e-9, `${m.wire.label} towards its own lane`);
  }
  // A loop of the depth-first search (back edges) as well.
  const cycle = layout({ instances: [inst('a', ['in1', 'in2'], ['o1', 'o2']), inst('b', ['i1', 'i2'], ['x1', 'x2'])], connections: [conn('/a.o1', '/b.i1'), conn('/a.o2', '/b.i2'), conn('/b.x1', '/a.in1'), conn('/b.x2', '/a.in2')] });
  const back = bundleWires(cycle.wires, at(2), { bottom: cycle.bottom }).filter((i) => i.kind === 'bundle');
  assert.equal(back.length, 2);
  assert.ok(back.some((b) => b.feedback && b.count === 2));
});

test('a bundle is hit as such, and its nearest wire is found', () => {
  const drawn = layout(LEVEL);
  const k = at(2);
  const items = bundleWires(drawn.wires, k, { bottom: drawn.bottom });
  const bundle = items[0];
  const entry = { path: '', level: '', layout: drawn, transform: { k, x: 0, y: 0 }, nodes: [], wires: drawn.wires, bundles: items };
  const mid = bundle.mid;
  const hit = hitTest({ levels: [entry] }, { x: mid.x * k, y: mid.y * k });
  assert.equal(hit.kind, 'bundle');
  assert.equal(hit.wires.length, 3);
  assert.ok(hit.wires.every((w) => w.kind === 'wire'));
  assert.ok(nearBundle(bundle, mid, 0.5));
  const vel = drawn.wires.find((w) => w.written.from === '/seq.vel');
  const onVel = vel.points[Math.floor(vel.points.length / 2)];
  assert.equal(nearestWire(bundle, onVel), vel);
});

test('the same wires at the same fan give back the same bundle (a drawing keyed by it is kept)', () => {
  const drawn = layout(LEVEL);
  const cache = new Map();
  const a = bundleWires(drawn.wires, at(2), { cache })[0];
  const b = bundleWires(drawn.wires, at(2.5), { cache })[0];
  assert.equal(a, b);
  assert.notEqual(bundleWires(drawn.wires, at(5), { cache })[0], a);
});

test('the studio: melody → synth is one bundle of 3, drums → kick one of 2, at the top level zoomed out', () => {
  const drawn = layout(STUDIO.top);
  const bundles = bundleWires(drawn.wires, 0.1, { bottom: drawn.bottom }).filter((i) => i.kind === 'bundle');
  const pair = (from, to) => bundles.find((b) => b.from === from && b.to === to);
  assert.equal(pair('melody', 'synth')?.count, 3);
  assert.equal(pair('drums', 'kick')?.count, 2);
  assert.equal(pair('limiter', 'out')?.count, 2);
});

/** Two 20-port boxes far apart, wired p0 → p0 and p19 → p19: a bundle whose wires part far from its folded curve. */
function farPair() {
  const names = Array.from({ length: 20 }, (_, i) => `p${i}`);
  const level = { instances: [inst('a', [], names), inst('b', names, [])], connections: [conn('/a.p0', '/b.p0'), conn('/a.p19', '/b.p19')] };
  return layout(level, { a: { x: 0, y: 0 }, b: { x: 1000, y: 0 } });
}

test('a click selects the bundle; a second click on it, where only the folded curve is, selects its nearest wire', () => {
  const drawn = farPair();
  const k = 0.1;
  const entry = (solo) => ({ path: '', level: '', layout: drawn, transform: { k, x: 0, y: 0 }, nodes: [], wires: drawn.wires, bundles: bundleWires(drawn.wires, k, { bottom: drawn.bottom, solo }) });
  const bundle = entry(new Set()).bundles[0];
  const point = { x: bundle.mid.x * k, y: bundle.mid.y * k };
  const first = hitTest({ levels: [entry(new Set())] }, point);
  assert.equal(first.kind, 'bundle');
  const nearest = () => first.wires.find((w) => w.wire === nearestWire(first.bundle, bundle.mid));
  const once = bundleClick([], first, nearest);
  assert.equal(once.length, 2);
  // Selected, its wires are drawn on their own, yet the folded curve is still what is under the pointer.
  const second = hitTest({ levels: [entry(new Set(drawn.wires.map(keyOf)))] }, point);
  assert.equal(second.kind, 'bundle');
  const twice = bundleClick(once, second, nearest);
  assert.equal(twice.length, 1);
  assert.equal(twice[0].kind, 'wire');
  assert.ok(drawn.wires.includes(twice[0].wire));
  // A third click on the bundle selects it whole again.
  assert.equal(bundleClick(twice, second, nearest).length, 2);
});

test('a dragged box’s level is drawn unbundled, so its wires follow it (DragFollow)', () => {
  const drawn = layout(LEVEL);
  const transform = { k: at(2), x: 0, y: 0 };
  const baked = bake(transform, { w: 1000, h: 600 });
  const element = (d) => {
    const attributes = { d };
    return { getAttribute: (n) => attributes[n] ?? null, setAttribute: (n, v) => (attributes[n] = String(v)), removeAttribute: (n) => delete attributes[n] };
  };
  // As drawLevel keys them: a bundle by its pair, a wire by its id.
  const groupOf = (items) => ({
    baked,
    wireCache: new Map(items.map((w) => [w.kind === 'bundle' ? `bundle ${w.key}` : w.id, { el: element(bakedPath(baked, w.points, w.curve)) }])),
    nodeCache: new Map(drawn.nodes.map((n) => [n.id, { el: element() }])),
  });
  const move = (items) => {
    const group = groupOf(items);
    const voice = drawn.nodes.find((n) => n.name === 'voice');
    new DragFollow('', 'voice').apply({ layout: drawn, levels: [{ path: '', level: '', transform }], groups: new Map([['', group]]), to: { x: voice.x + 100, y: voice.y + 40 } });
    return drawn.wires.filter((w) => group.wireCache.get(w.id) && group.wireCache.get(w.id).el.getAttribute('d') !== bakedPath(baked, w.points, w.curve)).length;
  };
  // Bundled, the three seq → voice wires have no element of their own to follow the box: only voice → out moves.
  assert.equal(move(bundleWires(drawn.wires, transform.k, { bottom: drawn.bottom })), 1);
  // Dragging (fanned): all four wires of voice follow it.
  assert.equal(move(bundleWires(drawn.wires, transform.k, { bottom: drawn.bottom, fanned: true })), 4);
});

test('a bundle’s count may go at its middle, then a third and a quarter of the way along it', () => {
  const drawn = layout(LEVEL);
  const [bundle] = bundleWires(drawn.wires, at(2), { bottom: drawn.bottom });
  const places = countPlaces(bundle);
  assert.equal(places.length, 5);
  assert.equal(places[0], bundle.mid);
  assert.ok(places.every((p) => bundle.points.includes(p)));
});

test('a bundle’s wire hits are the wires’ own hits', () => {
  const drawn = layout(LEVEL);
  const entry = { path: '', level: '' };
  assert.deepEqual(wireHit(entry, drawn.wires[0]).wire, drawn.wires[0]);
});

test('a level too small to read shows no bundle counts (they would pile up); readable, it does', () => {
  assert.ok(!countsShown(0.08), 'the synth inside a tall box, k 0.08');
  assert.ok(!countsShown(0.15));
  assert.ok(countsShown(0.25) && countsShown(0.4));
});
