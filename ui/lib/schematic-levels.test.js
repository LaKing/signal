import test from 'node:test';
import assert from 'node:assert/strict';
import { CLAIM_MS, RETRY_MS, candidates, levelFraction, levelPlan, levelSpan, mutedOf, parameterSpan } from './schematic-levels.js';
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
// Fitted as Fit does: every composite closed.
const fitted = () => scene({ layoutOf, view: fit(layoutOf('').bounds, viewport), viewport });

test('the candidates: only the selected instance’s signal ports, any rate divisor', () => {
  assert.deepEqual(candidates(fitted(), GRAPH), [], 'nothing selected, no meter');
  assert.deepEqual(candidates(fitted(), GRAPH, '/env').map((c) => c.address), ['/env.out']);
  const sparks = candidates(fitted(), GRAPH, '/sparks');
  assert.deepEqual(sparks.map((c) => c.address), ['/sparks.count'], 'a vector output has no meter');
  assert.deepEqual(candidates(fitted(), GRAPH, '/hub'), [], 'an event output has none');
  const drums = candidates(fitted(), GRAPH, '/drums');
  assert.deepEqual(drums.map((c) => [c.address, c.port, c.source]), [['/drums.out', '/drums/dc.out', 'scope']], 'a composite’s output is its primitive’s');
  assert.deepEqual(candidates(scene({ layoutOf, view: { k: 1, x: 1e6, y: 0 }, viewport }), GRAPH, '/env'), [], 'off screen, none');
});

// 2026-10-25, the owner's "csak a kijelölt elem kapjon mérőket, de minden
// bemeneten és kimeneten - a tartományának megfelelően".
const range = (min, max) => ({ min, max });
const MIXED = {
  top: {
    instances: [
      {
        name: 'mix',
        module: 'std.mix',
        composite: false,
        ports: [{ ...p('a', 'in'), range: null }, { ...p('b', 'in'), range: null }, { ...p('out', 'out'), range: range(-1, 1) }, p('ev', 'in', 'events')],
        parameters: [{ name: 'gain_a', kind: 'gain', rate: 1, range: range(0, 4) }],
      },
      {
        name: 'voice',
        module: 'voice',
        composite: true,
        ports: [p('in', 'in'), p('out', 'out')],
        parameters: [{ name: 'attack', kind: 'generic', rate: 1, range: range(0, 2) }],
      },
      { name: 'other', module: 'std.lag', composite: false, ports: [p('out', 'out')], parameters: [] },
    ],
    connections: [],
  },
  composites: {
    voice: {
      instances: [
        { name: 'amp', module: 'std.gain', composite: false, ports: [p('in', 'in'), p('out', 'out')], parameters: [] },
        { name: 'click', module: 'std.scale', composite: false, ports: [p('in', 'in'), p('out', 'out')], parameters: [] },
      ],
      connections: [],
      inputs: [{ name: 'in', to: ['amp.in', 'click.in'], shape: 'signal' }],
      outputs: [{ name: 'out', from: 'amp.out', shape: 'signal' }],
      parameters: [{ name: 'attack', to: ['click.mul'], range: range(0, 2) }],
    },
  },
};
const mixedLayouts = new Map();
const mixedLayoutOf = (level) => {
  if (!mixedLayouts.has(level)) mixedLayouts.set(level, layout(level === '' ? MIXED.top : MIXED.composites[level]));
  return mixedLayouts.get(level);
};
const mixedScene = () => scene({ layoutOf: mixedLayoutOf, view: fit(mixedLayoutOf('').bounds, viewport), viewport });

test('every input, parameter and output of the selected instance, inputs first, each with its range', () => {
  const list = candidates(mixedScene(), MIXED, '/mix', viewport);
  assert.deepEqual(
    list.map((c) => [c.address, c.direction, c.source, c.range]),
    [
      ['/mix.a', 'in', 'scope', null],
      ['/mix.b', 'in', 'scope', null],
      ['/mix.gain_a', 'in', 'parameter', range(0, 4)],
      ['/mix.out', 'out', 'scope', range(-1, 1)],
    ],
    'an event input has no meter; a parameter is metered by its value',
  );
  assert.ok(!list.some((c) => c.instance !== '/mix'), 'no other instance is metered');
});

test('a composite’s input and parameter are metered at their export’s first target', () => {
  const list = candidates(mixedScene(), MIXED, '/voice', viewport);
  assert.deepEqual(
    list.map((c) => [c.address, c.port, c.source, c.range]),
    [
      ['/voice.in', '/voice/amp.in', 'scope', null],
      ['/voice.attack', '/voice/click.mul', 'parameter', range(0, 2)],
      ['/voice.out', '/voice/amp.out', 'scope', null],
    ],
  );
  // Open, its inside is drawn and metered by what is selected there.
  const open = { ...mixedScene(), open: new Set(['/voice']) };
  assert.deepEqual(candidates(open, MIXED, '/voice', viewport), []);
});

test('a meter’s span: the dBFS peak without a range, the block’s lowest to highest value in one', () => {
  assert.deepEqual(levelSpan([0, -1, 0.5]), { lo: 0, hi: 1 });
  assert.deepEqual(levelSpan([0.5, -0.5], range(-1, 1)), { lo: 0.25, hi: 0.75 });
  assert.deepEqual(levelSpan([3, 4], range(0, 2)), { lo: 1, hi: 1 }, 'clamped to the range');
  assert.deepEqual(levelSpan([NaN], range(0, 1)), { lo: 0, hi: 0 });
  assert.deepEqual(parameterSpan(0.5, range(0, 2)), { lo: 0, hi: 0.25 });
  assert.deepEqual(parameterSpan(-1e6, range(0, 2)), { lo: 0, hi: 0 });
  assert.deepEqual(parameterSpan(1, null), { lo: 0, hi: 0 });
});

test('a muted instance is marked, and a composite that holds one', () => {
  const muted = new Set(['/kick/click_level']);
  assert.equal(mutedOf(muted, '/kick/click_level'), 'self');
  assert.equal(mutedOf(muted, '/kick'), 'inside');
  assert.equal(mutedOf(muted, '/kick/click'), null, 'a name that only starts the same is not');
  assert.equal(mutedOf(new Set(), '/kick'), null);
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
  assert.equal(plan.retry, 1000 + RETRY_MS - 1500, 'asked again when its cooldown ends');
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
  assert.equal(plan.retry, null);
});

// M2.9-1b: the host's free count (`scopes`) and panel scopes first.
const scopes = (free, sinceFreedMs, at, held = 0) => ({ free, sinceFreedMs, at, held });

test('only the free slots are asked, and no timer runs while none is', () => {
  const ports = wanted('/a.out', '/b.out', '/c.out');
  let plan = levelPlan({ wanted: ports, active: true, now: 9000, scopes: scopes(1, null, 9000) });
  assert.deepEqual(plan.subscribe, ['/a.out'], 'one free, one asked');
  assert.deepEqual([...plan.states.values()], ['pending', 'no-slot', 'no-slot']);
  assert.equal(plan.retry, null, 'nothing more could succeed: no timer');
  plan = levelPlan({ wanted: ports, granted: new Set(['/a.out']), active: true, now: 9100, scopes: scopes(0, null, 9100, 1) });
  assert.deepEqual(plan.subscribe, []);
  assert.equal(plan.retry, null, 'every port beyond the free count waits for the host, not a timer');
  // A subscription asked since the last `scopes` counts against its free count.
  plan = levelPlan({ wanted: ports, granted: new Set(['/a.out', '/b.out']), active: true, now: 9200, scopes: scopes(2, null, 9000, 0) });
  assert.deepEqual(plan.subscribe, [], 'two asked against two free: no third');
});

test('a slot freed is claimed CLAIM_MS after the free count rose, whoever freed it, at attach too', () => {
  // A panel scope refused at t = 0 retries at t = 2000; a slot is freed at
  // t = 100; a schematic attaches at t = 200 and hears it was freed 100 ms ago.
  const ports = wanted('/a.out');
  let plan = levelPlan({ wanted: ports, active: true, now: 200, scopes: scopes(1, 100, 200) });
  assert.deepEqual(plan.subscribe, [], 'not at attach');
  assert.equal(plan.retry, CLAIM_MS - 100 - 0);
  assert.equal(200 + plan.retry, 2600, 'after the panel scope’s retry at 2000');
  plan = levelPlan({ wanted: ports, active: true, now: 2000, scopes: scopes(1, 100, 200) });
  assert.deepEqual(plan.subscribe, [], 'not at the panel scope’s retry either');
  // The panel scope took it.
  plan = levelPlan({ wanted: ports, active: true, now: 2010, scopes: scopes(0, 1910, 2010) });
  assert.equal(plan.retry, null);
  // A rise older than CLAIM_MS is taken at once.
  plan = levelPlan({ wanted: ports, active: true, now: 9000, scopes: scopes(1, 5000, 9000) });
  assert.deepEqual(plan.subscribe, ['/a.out']);
});

// auditor-1-stale-own-slot-credit: an own release taken by another client,
// then a foreign release, gives no immediate claim.
test('a slot freed after an own release taken by another client waits too', () => {
  const ports = wanted('/a.out', '/b.out');
  // t = 0: the schematic let /x go (it left the screen); the host says free 1.
  let plan = levelPlan({ wanted: ports, active: true, now: 0, scopes: scopes(1, 0, 0) });
  assert.deepEqual(plan.subscribe, []);
  // t = 50: another client took it; a panel scope is refused (retry at 2050).
  plan = levelPlan({ wanted: ports, active: true, now: 50, scopes: scopes(0, 50, 50) });
  assert.equal(plan.retry, null);
  // t = 300: another client frees a slot.
  plan = levelPlan({ wanted: ports, active: true, now: 300, scopes: scopes(1, 0, 300) });
  assert.deepEqual(plan.subscribe, [], 'no immediate claim');
  assert.equal(300 + plan.retry, 2800, 'after the panel scope’s retry at 2050');
  plan = levelPlan({ wanted: ports, active: true, now: 2050, scopes: scopes(1, 0, 300) });
  assert.deepEqual(plan.subscribe, []);
});

test('after the schematic’s own release a newly visible port waits; a swap in one plan does not', () => {
  // t = 0: /a.out left the screen and was let go; the host says free 1.
  let plan = levelPlan({ wanted: wanted('/b.out'), active: true, now: 1000, scopes: scopes(1, 0, 0) });
  assert.deepEqual(plan.subscribe, [], 'entered at t = 1000');
  assert.equal(plan.retry, 1500);
  plan = levelPlan({ wanted: wanted('/b.out'), active: true, now: 2500, scopes: scopes(1, 0, 0) });
  assert.deepEqual(plan.subscribe, ['/b.out']);
  // One port leaves and one enters in the same plan, no slot free: swapped at once.
  plan = levelPlan({ wanted: wanted('/b.out'), granted: new Set(['/a.out']), active: true, now: 10, scopes: scopes(0, null, 0, 1) });
  assert.deepEqual([plan.unsubscribe, plan.subscribe], [['/a.out'], ['/b.out']]);
});

// auditor-2-no-slot-refusal-cooldown: a lost race leaves free > 0 until the
// next `scopes`, and the refused port is not asked again at once.
test('a refused port is not asked again within RETRY_MS, whatever the free count says', () => {
  for (const code of ['no-scope-slot', 'busy']) {
    const ports = wanted('/p.out');
    let plan = levelPlan({ wanted: ports, active: true, now: 0, scopes: scopes(1, 9000, 0) });
    assert.deepEqual(plan.subscribe, ['/p.out'], code);
    // Refused at t = 5 (`code`); no `scopes` since, so free still reads 1.
    const refused = new Map([['/p.out', 5]]);
    plan = levelPlan({ wanted: ports, refused, active: true, now: 10, scopes: scopes(1, 9000, 0) });
    assert.deepEqual(plan.subscribe, [], code);
    assert.equal(plan.retry, 1995, code);
    plan = levelPlan({ wanted: ports, refused, active: true, now: 20, scopes: scopes(0, 9000, 20) });
    assert.equal(plan.retry, null, `${code}: no free slot, no timer`);
  }
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
  assert.deepEqual(candidates(shown, { top: level, composites: {} }, '/wide', small).map((c) => c.address), ['/wide.in'], 'its input is on screen');
  const plan = levelPlan({ wanted: [], granted: new Set(['/wide.out']), active: true });
  assert.deepEqual([plan.subscribe, plan.unsubscribe], [[], ['/wide.out']], 'a held slot is let go');
  // Panned so the output is on screen (and the input off), it is a candidate again.
  const back = scene({ layoutOf: () => drawn, view: { k: 1, x: 50 - wide.x - wide.w, y: 10 - wide.y }, viewport: small });
  assert.deepEqual(candidates(back, { top: level, composites: {} }, '/wide', small).map((c) => c.address), ['/wide.out']);
});
