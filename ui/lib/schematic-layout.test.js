import test from 'node:test';
import assert from 'node:assert/strict';
import { GAP_Y, INPUTS, OUTPUTS, layout, shapeName } from './schematic-layout.js';

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
