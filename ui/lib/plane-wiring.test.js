// ADR 0051 phase 2, test 5 (Wiring's part): the studio's primary Stack from
// behind. Each instrument's ports are derived from the connections that
// cross it, exactly: one per instrument port a connection joins to another
// instrument of the Stack or to its edge, inputs left, outputs right; one
// cable per such connection, of every kind, and the edge cable meets the
// Setup level's edge pin. The cables run in channels and rails, never over
// a box; deterministic; within the layout budget.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { parseContent } from './panel-tree.js';
import { panelCards, parsePlane } from './plane-file.js';
import { assignInstruments, edgePins, stackMembers } from './plane-instruments.js';
import { pinPlaces, stackLayout } from './plane-layout.js';
import { assignTracks, cableKind, portIndex, polylinePath, simplify, wiringLayout, wiringModel } from './plane-wiring.js';
import { PU } from './plate.js';

const root = new URL('../../', import.meta.url);
const read = (path) => readFileSync(new URL(path, root), 'utf8');
const whitelist = JSON.parse(read('ui/lib/panel-whitelist.json'));
const STUDIO = JSON.parse(read('ui/lib/fixtures/studio-graph.json'));

function studioCards() {
  const text = read('projects/studio/ui/main.html');
  const start = text.indexOf('>', text.indexOf('<signal-panel')) + 1;
  return panelCards(parseContent(text.slice(start, text.indexOf('</signal-panel>')), whitelist, { mode: 'read' }).tree);
}

function studio() {
  const { instances, connections } = STUDIO.top;
  const owner = assignInstruments({ instances, connections, cards: studioCards() });
  const stack = parsePlane(read('projects/studio/ui/plane.toml')).plane.stacks[0];
  const { stackOf } = stackMembers({ stacks: [stack] }, owner, instances);
  const pins = edgePins(stack, owner, stackOf, connections);
  return { instances, connections, owner, stack, stackOf, pins };
}

/** The studio's instruments at plausible measured heights. */
function studioLayout(stack) {
  const heights = { 'card:transport': 120, 'card:mono-synth': 420, 'card:rhythm-composer': 400, 'card:tape-echo': 140, 'card:master': 200 };
  return stackLayout(stack, stack.instruments.map((entry) => ({ entry, w: stack.width * PU, h: heights[entry], x: null })));
}

const portsText = (model) => Object.fromEntries([...model.ports].map(([e, p]) => [e, { ins: p.ins.map((q) => `${q.address} ${q.kind}`), outs: p.outs.map((q) => `${q.address} ${q.kind}`) }]));

test('the studio’s Wiring: each instrument’s ports derived from the connections crossing it, exactly', () => {
  const s = studio();
  const model = wiringModel(s.stack, s.owner, s.stackOf, s.instances, s.connections, s.pins);
  assert.deepEqual(portsText(model), {
    'card:transport': { ins: [], outs: ['/transport.tick events'] },
    'card:mono-synth': { ins: ['/melody.tick events'], outs: ['/synth.out audio'] },
    'card:rhythm-composer': { ins: ['/drums.tick events'], outs: ['/drum_a.out audio', '/drum_b.out audio'] },
    'card:tape-echo': { ins: ['/echo.in audio'], outs: ['/echo.out audio'] },
    'card:master': { ins: ['/bus.c audio', '/bus.a audio', '/bus.b audio'], outs: ['/limiter.out audio'] },
  });
  // One cable per connection between instruments, of every kind; the edge cable once for the Setup's pin (its fan-out to l and r is the Setup's).
  assert.deepEqual(
    model.cables.map((c) => `${c.from} → ${c.b.edge ? `edge ${c.b.pin}` : c.to} ${c.kind}`),
    [
      '/transport.tick → /melody.tick events',
      '/transport.tick → /drums.tick events',
      '/synth.out → /echo.in audio',
      '/echo.out → /bus.c audio',
      '/drum_a.out → /bus.a audio',
      '/drum_b.out → /bus.b audio',
      '/limiter.out → edge /limiter.out audio',
    ],
  );
  // Nothing inside an instrument is drawn here (the drum lanes, the master chain): those are its Engine's.
  assert.ok(!model.cables.some((c) => c.from.startsWith('/drums.t') || c.from === '/bus.out'));
});

test('a cable’s kind: events, vector, control (into a parameter, or a signal that is no audio), audio', () => {
  const instances = [
    { name: 'lfo', ports: [{ address: '/lfo.out', direction: 'out', shape: 'signal', kind: 'control' }], parameters: [] },
    { name: 'osc', ports: [{ address: '/osc.out', direction: 'out', shape: 'signal', kind: 'audio' }, { address: '/osc.in', direction: 'in', shape: 'signal', kind: 'audio' }], parameters: [{ address: '/osc.freq', kind: 'frequency' }] },
    { name: 'seq', ports: [{ address: '/seq.t', direction: 'out', shape: 'events' }, { address: '/seq.v', direction: 'out', shape: { vector: 8 } }], parameters: [] },
  ];
  const index = portIndex(instances);
  assert.equal(cableKind({ from: '/lfo.out', to: '/osc.freq' }, index), 'control');
  assert.equal(cableKind({ from: '/osc.out', to: '/osc.freq' }, index), 'control');
  assert.equal(cableKind({ from: '/osc.out', to: '/osc.in' }, index), 'audio');
  assert.equal(cableKind({ from: '/seq.t', to: '/osc.in' }, index), 'events');
  assert.equal(cableKind({ from: '/seq.v', to: '/osc.in' }, index), 'vector');
});

test('the layout: ports on the box edges, cables in the channels and rails, never over a box; the edge cable meets the Setup’s pin', () => {
  const s = studio();
  const layout = studioLayout(s.stack);
  const model = wiringModel(s.stack, s.owner, s.stackOf, s.instances, s.connections, s.pins);
  const pinYs = new Map(pinPlaces(s.pins, layout.rects).map((p) => [`${p.side} ${p.address}`, p.y]));
  const t0 = performance.now();
  const w = wiringLayout(model, layout, pinYs);
  const ms = performance.now() - t0;
  assert.ok(ms < 50, `laid out in ${ms.toFixed(1)} ms (budget 50)`);
  // No rectangle moves: each box is its instrument's rectangle, inset by the channel.
  for (const [i, b] of w.boxes.entries()) {
    const r = layout.rects[i];
    assert.equal(b.y, r.y);
    assert.equal(b.h, r.h);
    assert.equal(b.x, w.channel);
    assert.equal(b.x + b.w, layout.w - w.channel);
  }
  for (const p of w.ports) assert.equal(p.x, p.side === 'in' ? w.channel : layout.w - w.channel);
  for (const c of w.cables) {
    // Orthogonal, and no segment passes through a box's inside.
    for (let i = 1; i < c.points.length; i++) {
      const a = c.points[i - 1];
      const b = c.points[i];
      assert.ok(Math.abs(a.x - b.x) < 1e-9 || Math.abs(a.y - b.y) < 1e-9, `${c.id} is orthogonal`);
      for (const box of w.boxes) {
        const lo = { x: Math.min(a.x, b.x), y: Math.min(a.y, b.y) };
        const hi = { x: Math.max(a.x, b.x), y: Math.max(a.y, b.y) };
        const inside = lo.x < box.x + box.w - 1e-6 && hi.x > box.x + 1e-6 && lo.y < box.y + box.h - 1e-6 && hi.y > box.y + 1e-6;
        assert.ok(!inside, `${c.id} passes over ${box.entry}`);
      }
    }
  }
  const edge = w.cables.find((c) => c.b.edge === 'out');
  assert.deepEqual(edge.points.at(-1), { x: layout.w, y: pinYs.get('out /limiter.out') });
  // The fan-out from the transport shares its trunk: one junction where the second branch leaves it.
  const fan = w.cables.filter((c) => c.from === '/transport.tick');
  assert.equal(fan[0].points[1].x, fan[1].points[1].x);
  assert.equal(fan.flatMap((c) => c.junctions).length, 1);
  // Deterministic.
  assert.deepEqual(wiringLayout(model, layout, pinYs), w);
});

test('tracks: overlapping intervals never share one; lowest free first', () => {
  const t = assignTracks([
    { key: 'a', lo: 0, hi: 10 },
    { key: 'b', lo: 5, hi: 15 },
    { key: 'c', lo: 11, hi: 20 },
    { key: 'd', lo: 30, hi: 40 },
  ]);
  assert.deepEqual([...t], [
    ['a', 0],
    ['b', 1],
    ['c', 0],
    ['d', 0],
  ]);
  assert.equal(assignTracks([{ key: 'a', lo: 0, hi: 10 }, { key: 'c', lo: 11, hi: 20 }], 2).get('c'), 1);
});

test('polylines: collinear points dropped, corners rounded', () => {
  assert.deepEqual(simplify([{ x: 0, y: 0 }, { x: 5, y: 0 }, { x: 10, y: 0 }, { x: 10, y: 0 }, { x: 10, y: 5 }]), [{ x: 0, y: 0 }, { x: 10, y: 0 }, { x: 10, y: 5 }]);
  assert.equal(polylinePath([{ x: 0, y: 0 }, { x: 10, y: 0 }, { x: 10, y: 10 }]), 'M0 0 L10 0 L10 10');
  assert.equal(polylinePath([{ x: 0, y: 0 }, { x: 10, y: 0 }, { x: 10, y: 10 }], 4), 'M0 0 L6 0 Q10 0 10 4 L10 10');
});

test('every port and cable of the studio’s Wiring is hit at its own place, at the Wiring level’s scale', async () => {
  const { hitItem } = await import('./plane-hit.js');
  const s = studio();
  const layout = studioLayout(s.stack);
  const model = wiringModel(s.stack, s.owner, s.stackOf, s.instances, s.connections, s.pins);
  const w = wiringLayout(model, layout, new Map(pinPlaces(s.pins, layout.rects).map((p) => [`${p.side} ${p.address}`, p.y])));
  const items = [...w.boxes.map((b) => ({ kind: 'box', id: `box ${b.entry}`, ...b })), ...w.cables.map((c) => ({ kind: 'cable', id: c.id, points: c.points })), ...w.ports.map((p) => ({ kind: 'port', id: p.id, x: p.x, y: p.y }))];
  const k = 2.2;
  for (const p of w.ports) assert.equal(hitItem(items, { x: p.x + 1, y: p.y - 1 }, k)?.item.id, p.id);
  for (const c of w.cables) {
    // The middle of its longest segment: no pin is near there.
    let best = null;
    for (let i = 1; i < c.points.length; i++) {
      const a = c.points[i - 1];
      const b = c.points[i];
      const len = Math.hypot(b.x - a.x, b.y - a.y);
      if (!best || len > best.len) best = { len, at: { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 } };
    }
    const hit = hitItem(items, best.at, k)?.item.id;
    // Cables of one net share their trunk: either of them is the right answer there.
    assert.ok(hit === c.id || w.cables.find((x) => x.id === hit)?.from === c.from, `${c.id} hit (got ${hit})`);
  }
});
