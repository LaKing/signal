import test from 'node:test';
import assert from 'node:assert/strict';
import { RETRY_MS, candidates, levelFraction, levelPlan, retryDue } from './schematic-levels.js';
import { layout } from './schematic-layout.js';
import { scene } from './schematic-scene.js';
import { fit } from './schematic-view.js';

const p = (name, direction, shape = 'signal', rate = 1) => ({ name, direction, shape, kind: 'audio', rate });
const GRAPH = {
  top: {
    instances: [
      { name: 'env', module: 'std.lag', composite: false, ports: [p('out', 'out')], parameters: [] },
      // A divided-rate signal output (rate divisor 800) has a level too.
      { name: 'sparks', module: 'std.particles', composite: false, ports: [p('pos', 'out', { vector: 8 }), p('count', 'out', 'signal', 800)], parameters: [] },
      { name: 'hub', module: 'std.clock_div', composite: false, ports: [p('out', 'out', 'events')], parameters: [] },
      { name: 'drums', module: 'drums', composite: true, ports: [p('out', 'out')], parameters: [] },
    ],
    connections: [],
  },
  composites: {
    drums: { instances: [{ name: 'dc', module: 'std.dc_block', composite: false, ports: [p('out', 'out')], parameters: [] }], connections: [], inputs: [], outputs: [{ name: 'out', from: 'dc.out', shape: 'signal' }], parameters: [] },
  },
};
const viewport = { w: 1000, h: 600 };
const layouts = new Map();
const layoutOf = (level) => {
  if (!layouts.has(level)) layouts.set(level, layout(level === '' ? GRAPH.top : GRAPH.composites[level]));
  return layouts.get(level);
};
const fitted = () => scene({ layoutOf, view: fit(layoutOf('').bounds, viewport), viewport });

test('the candidates: visible signal outputs, any rate divisor, in screen order, the selected instance first', () => {
  const list = candidates(fitted(), GRAPH);
  assert.deepEqual(list.map((c) => c.address), ['/env.out', '/sparks.count', '/drums.out']);
  assert.equal(list.find((c) => c.address === '/drums.out').port, '/drums/dc.out', 'a composite’s output is its primitive’s');
  assert.ok(!list.some((c) => c.address === '/sparks.pos' || c.address === '/hub.out'), 'vectors and events have no level');
  assert.deepEqual(candidates(fitted(), GRAPH, '/drums').map((c) => c.address)[0], '/drums.out');
  assert.deepEqual(candidates(scene({ layoutOf, view: { k: 1, x: 1e6, y: 0 }, viewport }), GRAPH), [], 'off screen, none');
});

const wanted = (...ports) => ports.map((port) => ({ address: port, port }));

test('live up to the slots, the rest no slot; off screen unsubscribed', () => {
  const ports = wanted('/a.out', '/b.out', '/c.out');
  let plan = levelPlan({ wanted: ports, active: true, slots: 2 });
  assert.deepEqual(plan.subscribe, ['/a.out', '/b.out']);
  assert.deepEqual([...plan.states.values()], ['pending', 'pending', 'no-slot']);
  plan = levelPlan({ wanted: ports, granted: new Set(['/a.out', '/b.out']), active: true, slots: 2 });
  assert.deepEqual([plan.subscribe, plan.unsubscribe], [[], []]);
  assert.deepEqual([...plan.states.values()], ['live', 'live', 'no-slot']);
  // Panned: a and b left the screen, so their slots go and c takes one.
  plan = levelPlan({ wanted: wanted('/c.out'), granted: new Set(['/a.out', '/b.out']), active: true, slots: 2 });
  assert.deepEqual(plan.unsubscribe, ['/a.out', '/b.out']);
  assert.deepEqual(plan.subscribe, ['/c.out']);
});

test('a refusal (no free slot) shows no slot and is asked again after a while', () => {
  const ports = wanted('/a.out', '/b.out');
  const refused = new Map([['/b.out', 1000]]);
  let plan = levelPlan({ wanted: ports, granted: new Set(['/a.out']), refused, active: true, now: 1500 });
  assert.deepEqual(plan.subscribe, []);
  assert.equal(plan.states.get('/b.out'), 'no-slot');
  assert.ok(retryDue(plan.states));
  plan = levelPlan({ wanted: ports, granted: new Set(['/a.out']), refused, active: true, now: 1000 + RETRY_MS });
  assert.deepEqual(plan.subscribe, ['/b.out'], 'a slot freed elsewhere is taken on the retry');
  // The selected instance's outputs come first, and take a slot from the last.
  plan = levelPlan({ wanted: wanted('/c.out', '/a.out'), granted: new Set(['/a.out', '/b.out']), active: true, slots: 2 });
  assert.deepEqual(plan.unsubscribe, ['/b.out']);
  assert.deepEqual(plan.subscribe, ['/c.out']);
});

test('hidden, switched off or stopped: everything is let go, every bar idle', () => {
  const plan = levelPlan({ wanted: wanted('/a.out', '/b.out'), granted: new Set(['/a.out', '/b.out']), active: false });
  assert.deepEqual(plan.subscribe, []);
  assert.deepEqual(plan.unsubscribe, ['/a.out', '/b.out']);
  assert.deepEqual([...plan.states.values()], ['idle', 'idle']);
  assert.equal(retryDue(plan.states), false);
});

test('a bar is the block’s peak on a −60…0 dBFS scale', () => {
  assert.equal(levelFraction(new Float32Array([0, -1, 0.5])), 1);
  assert.equal(levelFraction([0.001]), 0);
  assert.ok(Math.abs(levelFraction([0.1]) - 2 / 3) < 1e-6);
  assert.equal(levelFraction([]), 0);
});

// auditor-1-003: a box partly on screen keeps only its visible outputs; one
// whose point is off screen is neither subscribed nor holding a slot, and a
// pan that moves it off lets its slot go.
test('only ports whose point is on screen are candidates, whatever their box', () => {
  const level = {
    instances: [{ name: 'wide', module: 'std.wide', composite: false, ports: [p('in', 'in'), p('out', 'out')], parameters: [] }],
    connections: [],
  };
  const drawn = layout(level);
  const wide = drawn.nodes[0];
  const small = { w: 100, h: 100 };
  // The box starts on screen; its output (on its right edge) is past the viewport.
  const view = { k: 1, x: 20 - wide.x, y: 10 - wide.y };
  const shown = scene({ layoutOf: () => drawn, view, viewport: small });
  assert.deepEqual(shown.levels[0].nodes.map((n) => n.name), ['wide'], 'the box is drawn');
  assert.ok(20 + wide.w > small.w, 'its output is off screen');
  assert.deepEqual(candidates(shown, { top: level, composites: {} }, null, small), []);
  const plan = levelPlan({ wanted: [], granted: new Set(['/wide.out']), active: true });
  assert.deepEqual([plan.subscribe, plan.unsubscribe], [[], ['/wide.out']], 'a held slot is let go');
  // Panned so the output is on screen, it is a candidate again.
  const back = scene({ layoutOf: () => drawn, view: { k: 1, x: 50 - wide.x - wide.w, y: 10 - wide.y }, viewport: small });
  assert.deepEqual(candidates(back, { top: level, composites: {} }, null, small).map((c) => c.address), ['/wide.out']);
});
