import test from 'node:test';
import assert from 'node:assert/strict';
import { GAP_Y, INPUTS, OUTPUTS, dragPosition, laneY, layout, levelBottom, movedWires, routed, shapeName } from './schematic-layout.js';
import { wireGeometry } from './schematic-wire.js';

const port = (name, direction, shape = 'signal') => ({ name, direction, shape, kind: 'audio', rate: 1 });
const box = (name, ins = ['in'], outs = ['out'], extra = {}) => ({
  name,
  module: `std.${name}`,
  composite: false,
  ports: [...ins.map((p) => port(p, 'in')), ...outs.map((p) => port(p, 'out'))],
  parameters: [],
  ...extra,
});
const wire = (from, to, converters = []) => ({ from, to, scale: null, converters });

const nodeOf = (result, id) => result.nodes.find((n) => n.id === id);

test('layers run left to right by signal flow, by the longest path', () => {
  // a → b → c → out, and a → out directly: out is on the longest path's layer.
  const level = {
    instances: [box('out', ['l'], []), box('c'), box('b'), box('a', [])],
    connections: [wire('/a.out', '/b.in'), wire('/b.out', '/c.in'), wire('/c.out', '/out.l'), wire('/a.out', '/out.l')],
  };
  const result = layout(level);
  const layer = (id) => nodeOf(result, id).layer;
  assert.deepEqual(['a', 'b', 'c', 'out'].map(layer), [0, 1, 2, 3]);
  assert.ok(nodeOf(result, 'a').x < nodeOf(result, 'b').x && nodeOf(result, 'c').x < nodeOf(result, 'out').x);
});

test('each layer is ordered by the barycentre of its neighbours', () => {
  // Sources p, q; their targets listed crossed (y before x): x is fed by p,
  // y by q, so the sweeps put x above y and nothing crosses.
  const level = {
    instances: [box('p', []), box('q', []), box('y'), box('x')],
    connections: [wire('/q.out', '/y.in'), wire('/p.out', '/x.in')],
  };
  const result = layout(level);
  assert.ok(nodeOf(result, 'p').y < nodeOf(result, 'q').y, 'sources in source order');
  assert.ok(nodeOf(result, 'x').y < nodeOf(result, 'y').y, 'x follows p, above y');
});

test('a feedback connection is reversed for layering and drawn back around below', () => {
  // pulse's loop: delay → feedback → damping → delay.
  const level = {
    instances: [box('src', []), box('delay'), box('feedback'), box('damping', ['in'], ['lp'])],
    connections: [
      wire('/src.out', '/delay.in'),
      wire('/delay.out', '/feedback.in'),
      wire('/feedback.out', '/damping.in'),
      wire('/damping.lp', '/delay.in'),
    ],
  };
  const result = layout(level);
  assert.deepEqual(['src', 'delay', 'feedback', 'damping'].map((id) => nodeOf(result, id).layer), [0, 1, 2, 3]);
  const back = result.wires.find((w) => w.from.node === 'damping');
  assert.equal(back.reversed, true);
  assert.equal(back.feedback, true);
  const lowest = Math.max(...result.nodes.map((n) => n.y + n.h));
  assert.ok(back.points.some((p) => p.y > lowest), 'routed below every node');
  assert.ok(back.points.at(-1).x === nodeOf(result, 'delay').x, 'into delay.in from its left');
  assert.equal(result.wires.filter((w) => w.feedback).length, 1, 'the forward wires stay forward');
});

test('the same input gives the same output, whatever the objects’ key order', () => {
  const level = {
    instances: [box('a', []), box('b'), box('c')],
    connections: [wire('/a.out', '/b.in'), wire('/b.out', '/c.in'), wire('/c.out', '/b.in')],
  };
  const reordered = JSON.parse(JSON.stringify(level), (key, value) => {
    if (value && typeof value === 'object' && !Array.isArray(value)) {
      return Object.fromEntries(Object.entries(value).reverse());
    }
    return value;
  });
  const positions = { c: { x: 400, y: 10 }, a: { x: 0, y: 0 } };
  const shuffled = { a: { y: 0, x: 0 }, c: { y: 10, x: 400 } };
  assert.deepEqual(layout(level, positions), layout(reordered, shuffled));
  assert.deepEqual(layout(level), layout(level), 'and twice the same');
});

test('stored positions are kept, the other nodes placed around them without overlap', () => {
  const level = {
    instances: [box('a', []), box('b'), box('c'), box('d', ['in'], [])],
    connections: [wire('/a.out', '/b.in'), wire('/b.out', '/c.in'), wire('/c.out', '/d.in')],
  };
  const free = layout(level);
  // b is stored exactly where the automatic layout puts a, and c too.
  const a = nodeOf(free, 'a');
  const result = layout(level, { b: { x: a.x, y: a.y }, c: { x: a.x + 10, y: a.y + 5 } });
  assert.deepEqual([nodeOf(result, 'b').x, nodeOf(result, 'b').y], [a.x, a.y]);
  assert.equal(nodeOf(result, 'b').pinned, true);
  assert.equal(nodeOf(result, 'a').pinned, false);
  for (const one of result.nodes) {
    for (const other of result.nodes) {
      if (one === other || (one.pinned && other.pinned)) continue;
      const apart = one.x + one.w <= other.x || other.x + other.w <= one.x || one.y + one.h <= other.y || other.y + other.h <= one.y;
      assert.ok(apart, `${one.id} and ${other.id} overlap`);
    }
  }
  const b = nodeOf(result, 'b');
  assert.equal(nodeOf(result, 'a').y, b.y + b.h + GAP_Y, 'a moved down, just past b');
});

test('boxes carry inputs left and outputs right; wires carry shape and converters', () => {
  const level = {
    instances: [
      box('env', [], ['out']),
      { name: 'sparks', module: 'std.particles', composite: false, ports: [port('impulse', 'in', { vector: 4 }), port('pos', 'out', { vector: 8 })], parameters: [{ name: 'rate', kind: 'control', rate: 800 }] },
      box('hub', [], ['out'], { ports: [port('out', 'out', 'events')] }),
    ],
    connections: [
      wire('/env.out', '/sparks.rate', [{ name: 'out_d800', module: 'core.decimate.d1.d800.signal.control', conversion: 'decimate', fromRate: 1, toRate: 800 }]),
      wire('/hub.out', '/sparks.impulse'),
    ],
  };
  const result = layout(level);
  const sparks = nodeOf(result, 'sparks');
  assert.deepEqual(sparks.ins.map((p) => p.name), ['impulse', 'rate'], 'ports, then parameters');
  assert.ok(sparks.ins.every((p) => p.x === sparks.x));
  assert.ok(sparks.outs.every((p) => p.x === sparks.x + sparks.w));
  const [converted, events] = result.wires;
  assert.deepEqual([converted.shape, converted.dashed], ['signal', true]);
  assert.deepEqual([events.shape, events.dashed], ['events', false]);
  assert.equal(shapeName({ vector: 4 }), 'vector');
  assert.ok(result.bounds.w > 0 && result.bounds.h > 0);
});

test('a composite level has its exports as pins on the frame', () => {
  const level = {
    instances: [box('env', ['trigger']), box('vca', ['a'])],
    connections: [wire('env.out', 'vca.a')],
    inputs: [{ name: 'trigger', to: ['env.trigger'], shape: 'events' }],
    outputs: [{ name: 'out', from: 'vca.out', shape: 'signal' }],
    parameters: [{ name: 'decay', to: ['env.decay'] }],
  };
  const result = layout(level);
  assert.equal(result.nodes[0].id, INPUTS);
  assert.deepEqual(result.nodes[0].outs.map((p) => p.name), ['trigger', 'decay']);
  assert.equal(result.nodes.at(-1).id, OUTPUTS);
  assert.ok(nodeOf(result, OUTPUTS).layer > nodeOf(result, 'vca').layer);
  assert.equal(result.wires.filter((w) => w.kind === 'export').length, 2, 'decay reaches no drawn port');
});

test('wires are smooth curves from their output pin to their input pin', () => {
  const level = {
    instances: [box('a', []), box('b', ['in', 'side']), box('c'), box('d', ['in'], [])],
    connections: [wire('/a.out', '/b.side'), wire('/b.out', '/c.in'), wire('/a.out', '/d.in'), wire('/c.out', '/b.in')],
  };
  const result = layout(level);
  for (const w of result.wires) {
    const a = nodeOf(result, w.from.node).outs.find((p) => p.name === w.from.port);
    const b = nodeOf(result, w.to.node).ins.find((p) => p.name === w.to.port);
    assert.deepEqual([w.curve.start, w.curve.segs.at(-1).to], [{ x: a.x, y: a.y }, { x: b.x, y: b.y }], w.label);
    assert.deepEqual([w.points[0], w.points.at(-1)], [w.curve.start, w.curve.segs.at(-1).to]);
    const { curve, points } = wireGeometry(a, b, { feedback: w.feedback, under: laneY(result.bottom, w.lane) });
    assert.deepEqual([w.curve, w.points], [curve, points], `${w.label}: the one curve function's`);
    if (!w.feedback) assert.equal(w.curve.segs.length, 1, `${w.label}: one cubic`);
  }
});

test('feedback loops have lanes of their own, below every box', () => {
  const level = {
    instances: [box('a', []), box('b', ['in', 'back']), box('c', ['in', 'back']), box('d')],
    connections: [wire('/a.out', '/b.in'), wire('/b.out', '/c.in'), wire('/c.out', '/d.in'), wire('/d.out', '/c.back'), wire('/c.out', '/b.back')],
  };
  const result = layout(level);
  const loops = result.wires.filter((w) => w.feedback);
  assert.deepEqual(loops.map((w) => w.lane), [1, 2]);
  const lowest = Math.max(...result.nodes.map((n) => n.y + n.h));
  assert.equal(result.bottom, lowest);
  const depth = loops.map((w) => Math.max(...w.points.map((p) => p.y)));
  assert.ok(depth.every((y) => y > lowest), 'below every box');
  assert.notEqual(depth[0], depth[1], 'not on top of each other');
});

/**
 * Dragging `id` of `level` to `to`: the wires `movedWires` gives, put in
 * place of the layout's, are the wires of the layout with `id` stored at
 * `to` (when no other box has to move out of its way).
 */
function followsDrag(level, id, to) {
  const before = layout(level);
  const after = layout(level, { [id]: to });
  for (const n of after.nodes) if (n.id !== id) assert.deepEqual([n.x, n.y], [nodeOf(before, n.id).x, nodeOf(before, n.id).y], `${n.id} stayed`);
  const moved = new Map(movedWires(before, id, to).map((m) => [m.wire.id, m]));
  const shape = (w) => ({ curve: w.curve, points: w.points, box: w.box, feedback: w.feedback, under: w.under });
  const route = (m) => ({ ...routed(m), feedback: m.feedback, under: m.under });
  for (const w of after.wires) assert.deepEqual(moved.has(w.id) ? route(moved.get(w.id)) : shape(before.wires[w.id]), shape(w), `${w.label} follows`);
  return moved;
}

test('a dragged box’s wires follow it as the next layout will draw them', () => {
  const level = {
    instances: [box('a', []), box('b', ['in', 'back']), box('c'), box('d', ['in'], [])],
    connections: [wire('/a.out', '/b.in'), wire('/b.out', '/c.in'), wire('/c.out', '/d.in'), wire('/c.out', '/b.back')],
  };
  const free = layout(level);
  const c = nodeOf(free, 'c');
  // A little up and right: the wires in and out of c, and the loop from it.
  const near = followsDrag(level, 'c', { x: c.x + 17, y: c.y - 9 });
  assert.deepEqual([...near.values()].map((m) => m.wire.label).sort(), ['/b.out → /c.in', '/c.out → /b.back', '/c.out → /d.in']);
  // Far below every box: the loop's lane goes down with it.
  followsDrag(level, 'c', { x: c.x, y: c.y + 500 });
  // Left of b: b → c turns into a loop and takes a lane; the loop c → b is
  // forward now, and gives its lane up.
  const back = followsDrag(level, 'c', { x: nodeOf(free, 'a').x, y: c.y + 300 });
  assert.equal([...back.values()].find((m) => m.wire.label === '/b.out → /c.in').feedback, true);
  assert.equal([...back.values()].find((m) => m.wire.label === '/c.out → /b.back').feedback, true, 'a back edge stays one');
  // A loop not attached to the dragged box moves only when its lane does.
  const loop = {
    instances: [box('a', []), box('b', ['in', 'back']), box('c'), box('e', [])],
    connections: [wire('/a.out', '/b.in'), wire('/b.out', '/c.in'), wire('/c.out', '/b.back')],
  };
  const e = nodeOf(layout(loop), 'e');
  assert.deepEqual([...followsDrag(loop, 'e', { x: e.x + 5, y: e.y }).keys()], [], 'nothing attached, the lanes stay');
  assert.equal(followsDrag(loop, 'e', { x: e.x, y: e.y + 900 }).size, 1, 'the loop goes below it');
  assert.deepEqual(movedWires(free, 'nope', { x: 0, y: 0 }), []);
});

test('a drag goes in whole layout units, as it is stored', () => {
  assert.deepEqual(dragPosition({ x: 10, y: 20 }, { x: 13, y: -7 }, 2), { x: 17, y: 17 });
  assert.deepEqual(dragPosition({ x: 10, y: 20 }, { x: 0.4, y: 0.6 }, 1), { x: 10, y: 21 });
});

test('a level above y = 0 keeps its loops’ lanes while a box moves (the same floor in the drag as in the layout)', () => {
  const level = {
    instances: [box('a', []), box('b', ['in', 'back']), box('c'), box('d', ['in'], [])],
    connections: [wire('/a.out', '/b.in'), wire('/b.out', '/c.in'), wire('/c.out', '/d.in'), wire('/c.out', '/b.back')],
  };
  const positions = { a: { x: 0, y: -900 }, b: { x: 200, y: -900 }, c: { x: 400, y: -900 }, d: { x: 600, y: -900 } };
  const stored = layout(level, positions);
  assert.equal(stored.bottom, 0, 'lanes below 0');
  assert.equal(levelBottom(stored.nodes), 0);
  const moved = movedWires(stored, 'd', { x: 650, y: -880 });
  assert.deepEqual(moved.map((m) => m.wire.label), ['/c.out → /d.in'], 'only the attached wire');
  const after = layout(level, { ...positions, d: { x: 650, y: -880 } });
  assert.deepEqual(routed(moved[0]).curve, after.wires[moved[0].wire.id].curve);
});

test('a pin knows its shape and whether a wire reaches it', () => {
  const level = {
    instances: [box('a', [], ['out', 'spare']), { ...box('b', ['in', 'free'], []), parameters: [{ name: 'gain' }, { name: 'pan' }] }],
    connections: [wire('/a.out', '/b.in'), wire('/a.out', '/b.gain')],
  };
  const result = layout(level);
  const pins = (id) => Object.fromEntries([...nodeOf(result, id).ins, ...nodeOf(result, id).outs].map((p) => [p.name, [p.shape, p.connected, Boolean(p.parameter)]]));
  assert.deepEqual(pins('a'), { out: ['signal', true, false], spare: ['signal', false, false] });
  assert.deepEqual(pins('b'), { in: ['signal', true, false], free: ['signal', false, false], gain: ['signal', true, true], pan: ['signal', false, true] });
});

// 2026-10-17: a group of selected boxes dragged together.
test('a group dragged together: its wires follow it as the next layout, with all of them stored there, draws them', () => {
  const level = {
    instances: [box('a', []), box('b', ['in', 'back']), box('c'), box('d', ['in'], [])],
    connections: [wire('/a.out', '/b.in'), wire('/b.out', '/c.in'), wire('/c.out', '/d.in'), wire('/c.out', '/b.back')],
  };
  const before = layout(level);
  const check = (offset, ids) => {
    const moves = new Map(ids.map((id) => [id, { x: nodeOf(before, id).x + offset.x, y: nodeOf(before, id).y + offset.y }]));
    const after = layout(level, Object.fromEntries(moves));
    for (const n of after.nodes) {
      const want = moves.get(n.id) ?? { x: nodeOf(before, n.id).x, y: nodeOf(before, n.id).y };
      assert.deepEqual([n.x, n.y], [want.x, want.y], `${n.id} where it was put`);
    }
    const moved = new Map(movedWires(before, moves).map((m) => [m.wire.id, m]));
    const shape = (w) => ({ curve: w.curve, points: w.points, box: w.box, feedback: w.feedback, under: w.under });
    const route = (m) => ({ ...routed(m), feedback: m.feedback, under: m.under });
    for (const w of after.wires) assert.deepEqual(moved.has(w.id) ? route(moved.get(w.id)) : shape(before.wires[w.id]), shape(w), `${w.label} follows`);
    return moved;
  };
  // b and c together: the wire between them keeps its shape but moves, and
  // those in and out of the pair change.
  const pair = check({ x: 13, y: 7 }, ['b', 'c']);
  assert.deepEqual([...pair.values()].map((m) => m.wire.label).sort(), ['/a.out → /b.in', '/b.out → /c.in', '/c.out → /b.back', '/c.out → /d.in']);
  // Far below: the loop's lane goes down with the lowest of them.
  check({ x: 0, y: 600 }, ['c', 'd']);
  // Every box: every wire moves.
  assert.equal(check({ x: -40, y: 25 }, ['a', 'b', 'c', 'd']).size, 4);
  // One id in a Map is the one-box drag.
  const c = nodeOf(before, 'c');
  const to = { x: c.x + 17, y: c.y - 9 };
  assert.deepEqual(movedWires(before, new Map([['c', to]])), movedWires(before, 'c', to));
  assert.deepEqual(movedWires(before, new Map([['nope', to]])), []);
  assert.equal(levelBottom(before.nodes, new Map([['d', { x: 0, y: 1000 }]])), 1000 + nodeOf(before, 'd').h);
});
