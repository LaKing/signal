// The Engine layer (ADR 0051 §8; the owner's decision of 2026-11-03): every
// instrument of the studio's Stack laid out in its own rectangle. Every
// instance of each instrument is placed inside the rectangle, no two cards
// overlap, no wire runs over a card, a parameter has a pin only where a
// signal connection drives it (no parameter legs, no bundle), the
// connections between the instances are routed, those crossing the edge
// enter and leave at the Wiring layer's port heights, and the layout is
// deterministic.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { parseContent } from './panel-tree.js';
import { panelCards, parsePlane } from './plane-file.js';
import { assignInstruments, edgePins, stackMembers } from './plane-instruments.js';
import { pinPlaces, stackLayout } from './plane-layout.js';
import { wiringLayout, wiringModel } from './plane-wiring.js';
import { engineOf } from './plane-control.js';
import { REVEAL, chipPath, engineLayout, insideOf, layLevel, orderedTracks, revealShare } from './plane-engine.js';
import { PU } from './plate.js';

const root = new URL('../../', import.meta.url);
const read = (path) => readFileSync(new URL(path, root), 'utf8');
const whitelist = JSON.parse(read('ui/lib/panel-whitelist.json'));
const STUDIO = JSON.parse(read('ui/lib/fixtures/studio-graph.json'));

function studio(graph = STUDIO) {
  const text = read('projects/studio/ui/main.html');
  const start = text.indexOf('>', text.indexOf('<signal-panel')) + 1;
  const cards = panelCards(parseContent(text.slice(start, text.indexOf('</signal-panel>')), whitelist, { mode: 'read' }).tree);
  const { instances, connections } = graph.top;
  const owner = assignInstruments({ instances, connections, cards });
  const stack = parsePlane(read('projects/studio/ui/plane.toml')).plane.stacks[0];
  const { stackOf } = stackMembers({ stacks: [stack] }, owner, instances);
  const pins = edgePins(stack, owner, stackOf, connections);
  const heights = { 'card:transport': 120, 'card:mono-synth': 420, 'card:rhythm-composer': 400, 'card:tape-echo': 140, 'card:master': 200 };
  const layout = stackLayout(stack, stack.instruments.map((entry) => ({ entry, w: stack.width * PU, h: heights[entry], x: null })));
  const model = wiringModel(stack, owner, stackOf, instances, connections, pins);
  const pinYs = new Map(pinPlaces(pins, layout.rects).map((p) => [`${p.side} ${p.address}`, p.y]));
  const wiring = wiringLayout(model, layout, pinYs);
  const engines = layout.rects.map((r) => {
    const ports = wiring.ports.filter((p) => p.entry === r.entry);
    const edgeIns = new Map(ports.filter((p) => p.side === 'in').map((p) => [p.address, p.y]));
    const edgeOuts = new Map(ports.filter((p) => p.side === 'out').map((p) => [p.address, p.y]));
    return engineLayout({ entry: r.entry, rect: r, names: engineOf(r.entry, owner), graph, edgeIns, edgeOuts, title: r.entry });
  });
  return { owner, layout, wiring, engines };
}

const within = (inner, outer) => inner.x >= outer.x - 1e-6 && inner.y >= outer.y - 1e-6 && inner.x + inner.w <= outer.x + outer.w + 1e-6 && inner.y + inner.h <= outer.y + outer.h + 1e-6;
const overlap = (a, b) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;

/** Whether the axis-aligned segment p–q passes through the inside of `r` (its edges excluded). */
function crosses(p, q, r, e = 0.5) {
  if (Math.abs(p.y - q.y) < 1e-9) {
    const [x0, x1] = [Math.min(p.x, q.x), Math.max(p.x, q.x)];
    return p.y > r.y + e && p.y < r.y + r.h - e && x1 > r.x + e && x0 < r.x + r.w - e;
  }
  const [y0, y1] = [Math.min(p.y, q.y), Math.max(p.y, q.y)];
  return p.x > r.x + e && p.x < r.x + r.w - e && y1 > r.y + e && y0 < r.y + r.h - e;
}

test('every instance of each instrument is a card inside its rectangle, the cards apart; composites one card each', () => {
  const { owner, engines } = studio();
  for (const e of engines) {
    const names = engineOf(e.entry, owner);
    assert.deepEqual(e.cards.map((c) => c.name).sort(), [...names].sort(), `${e.entry}: every instance placed`);
    for (const c of e.cards) {
      assert.ok(within(c, e.rect), `${e.entry}: ${c.name} inside the rectangle`);
      assert.ok(within(c.body, c), `${c.name}: its body inside it`);
    }
    for (let i = 0; i < e.cards.length; i++) for (let j = i + 1; j < e.cards.length; j++) assert.ok(!overlap(e.cards[i], e.cards[j]), `${e.cards[i].name} and ${e.cards[j].name} apart`);
  }
  const synth = engines.find((e) => e.entry === 'card:mono-synth');
  assert.deepEqual(synth.cards.map((c) => [c.name, c.module, c.composite, c.column]), [['melody', 'std.note_seq', false, 0], ['synth', 'moog_voice', true, 1]]);
});

test('no parameter legs: a parameter has a pin only where a signal connection drives it; ports are pins', () => {
  const { engines } = studio();
  const synth = engines.find((e) => e.entry === 'card:mono-synth');
  assert.equal(synth.pins.filter((p) => p.parameter).length, 0, 'no parameter of the synth is driven by a connection');
  assert.deepEqual(synth.pins.filter((p) => p.card === 'card synth').map((p) => `${p.side} ${p.name}`), ['in gate', 'in hz', 'in velocity', 'out out']);
  const drums = engines.find((e) => e.entry === 'card:rhythm-composer');
  // drums.t1 → kick.velocity: a modulation inside the engine, so kick's velocity has its pin; its other parameters none.
  assert.deepEqual(drums.pins.filter((p) => p.card === 'card kick').map((p) => `${p.side} ${p.name}${p.parameter ? ' (parameter)' : ''}`), ['in trigger', 'in velocity (parameter)', 'out out']);
  for (const e of engines) for (const key of ['legs', 'band', 'cube', 'readouts']) assert.equal(e[key], undefined, `no ${key} at the Engine`);
});

test('the connections are routed: orthogonal, between their pins, never over a card; the edge at the Wiring’s heights', () => {
  const { engines, wiring } = studio();
  const synth = engines.find((e) => e.entry === 'card:mono-synth');
  assert.deepEqual(synth.wires.map((w) => `${w.from} → ${w.to} ${w.kind} ${w.route}`), [
    '/transport.tick → /melody.tick events edge',
    '/melody.hz → /synth.hz control adjacent',
    '/melody.gate → /synth.gate control adjacent',
    '/melody.velocity → /synth.velocity control adjacent',
    '/synth.out → /echo.in audio edge',
  ]);
  const port = (side, address) => wiring.ports.find((p) => p.side === side && p.address === address).y;
  assert.equal(synth.edges.find((p) => p.side === 'in').y, port('in', '/melody.tick'), 'enters where the Wiring’s port is');
  assert.equal(synth.edges.find((p) => p.side === 'out').y, port('out', '/synth.out'), 'leaves where the Wiring’s port is');
  for (const e of engines) {
    const pinAt = new Map(e.pins.map((p) => [`${p.side} ${p.address}`, p]));
    for (const w of e.wires) {
      for (let i = 1; i < w.points.length; i++) {
        const [p, q] = [w.points[i - 1], w.points[i]];
        assert.ok(Math.abs(p.x - q.x) < 1e-9 || Math.abs(p.y - q.y) < 1e-9, `${w.id} orthogonal`);
        for (const c of e.cards) assert.ok(!crosses(p, q, c), `${e.entry}: ${w.from} → ${w.to} passes over ${c.name}`);
        assert.ok(within({ x: Math.min(p.x, q.x), y: Math.min(p.y, q.y), w: Math.abs(p.x - q.x), h: Math.abs(p.y - q.y) }, e.rect), `${w.id} inside the rectangle`);
      }
      const start = w.edge === 'in' ? e.edges.find((x) => x.id === `edge in ${w.to}`) : pinAt.get(`out ${w.from}`);
      const end = w.edge === 'out' ? e.edges.find((x) => x.id === `edge out ${w.from}`) : pinAt.get(`in ${w.to}`);
      assert.deepEqual([w.points[0].x, w.points[0].y], [start.x, start.y], `${w.id} starts at its pin`);
      assert.deepEqual([w.points.at(-1).x, w.points.at(-1).y], [end.x, end.y], `${w.id} ends at its pin`);
    }
  }
  // The rhythm composer: its sequencer's eight triggers to the seven voices, their velocities, the voices to the two mixers, those out.
  const drums = engines.find((e) => e.entry === 'card:rhythm-composer');
  assert.equal(drums.wires.length, 16 + 7 + 1 + 2, 'every internal connection and the edge ones');
  assert.equal(drums.wires.filter((w) => w.edge === 'out').length, 2);
});

/** The length two axis-aligned segments share along one line (0 when they only cross or touch). */
function along(a, b, c, d) {
  if (Math.abs(a.y - b.y) < 1e-6 && Math.abs(c.y - d.y) < 1e-6 && Math.abs(a.y - c.y) < 1e-6) return Math.min(Math.max(a.x, b.x), Math.max(c.x, d.x)) - Math.max(Math.min(a.x, b.x), Math.min(c.x, d.x));
  if (Math.abs(a.x - b.x) < 1e-6 && Math.abs(c.x - d.x) < 1e-6 && Math.abs(a.x - c.x) < 1e-6) return Math.min(Math.max(a.y, b.y), Math.max(c.y, d.y)) - Math.max(Math.min(a.y, b.y), Math.min(c.y, d.y));
  return 0;
}

test('two connections of different nets never run along one another (the mono synth’s gate, hz and velocity a step down each)', () => {
  for (const e of studio().engines) {
    for (const v of e.wires) for (const w of e.wires) {
      if (v === w || v.from === w.from || v.to === w.to) continue;
      for (let i = 1; i < v.points.length; i++) for (let j = 1; j < w.points.length; j++) {
        assert.ok(along(v.points[i - 1], v.points[i], w.points[j - 1], w.points[j]) <= 1e-6, `${e.entry}: ${v.from} → ${v.to} runs along ${w.from} → ${w.to}`);
      }
    }
  }
});

test('ordered tracks: of nets going down the lower runs left, of nets going up the upper; apart nets share a track', () => {
  const t = orderedTracks([
    { key: 'gate', lo: 10, hi: 20, yl: 10, yr: 20 },
    { key: 'hz', lo: 20, hi: 30, yl: 20, yr: 30 },
    { key: 'vel', lo: 30, hi: 40, yl: 30, yr: 40 },
    { key: 'far', lo: 100, hi: 110, yl: 100, yr: 110 },
  ], 1);
  assert.ok(t.get('vel') < t.get('hz') && t.get('hz') < t.get('gate'), 'a staircase down: the lowest leftmost');
  assert.equal(t.get('far'), 0, 'a net apart from the others shares the first track');
  const u = orderedTracks([
    { key: 'a', lo: 10, hi: 20, yl: 20, yr: 10 },
    { key: 'b', lo: 20, hi: 30, yl: 30, yr: 20 },
  ], 1);
  assert.ok(u.get('a') < u.get('b'), 'a staircase up: the highest leftmost');
});

test('skips and feedback: a wire past a column runs along a free row or a lane above; one back along a lane below', () => {
  const ports = (name, ins, outs) => ({ name, module: 'std.x', ports: [...ins.map((p) => ({ name: p, address: `/${name}.${p}`, direction: 'in', shape: 'signal', kind: 'audio', rate: 1 })), ...outs.map((p) => ({ name: p, address: `/${name}.${p}`, direction: 'out', shape: 'signal', kind: 'audio', rate: 1 }))], parameters: [{ name: 'gain', address: `/${name}.gain` }] });
  const level = {
    instances: [ports('a', ['in'], ['out']), ports('b', ['in', 'x', 'y', 'z'], ['out']), ports('c', ['in', 'back'], ['out'])],
    connections: [
      { from: '/a.out', to: '/b.in' },
      { from: '/b.out', to: '/c.in' },
      { from: '/a.out', to: '/c.back' },
      { from: '/c.out', to: '/a.in' },
      { from: '/c.out', to: '/b.gain' },
    ],
  };
  const area = { x: 0, y: 0, w: 1000, h: 400 };
  const laid = layLevel({ level, area });
  assert.deepEqual(laid.cards.map((c) => [c.name, c.column]), [['a', 0], ['b', 1], ['c', 2]]);
  const route = Object.fromEntries(laid.wires.map((w) => [`${w.from}>${w.to}`, w.route]));
  assert.equal(route['/c.out>/a.in'], 'feedback');
  assert.equal(route['/c.out>/b.gain'], 'feedback');
  assert.ok(['row', 'over'].includes(route['/a.out>/c.back']));
  const bottom = Math.max(...laid.cards.map((c) => c.y + c.h));
  for (const w of laid.wires.filter((x) => x.route === 'feedback')) assert.ok(w.points.some((p) => p.y > bottom), 'a feedback lane below the cards');
  assert.deepEqual(laid.pins.filter((p) => p.parameter).map((p) => p.address), ['/b.gain'], 'the driven parameter alone has a pin');
  for (const w of laid.wires) for (let i = 1; i < w.points.length; i++) for (const c of laid.cards) assert.ok(!crosses(w.points[i - 1], w.points[i], c), `${w.id} over ${c.name}`);
  // A fan-out's branch leaving its trunk midway has a dot.
  assert.ok(laid.wires.filter((w) => w.from === '/a.out').length === 2);
});

test('deterministic: the same graph and rectangles give the same layout; an instrument without an engine draws nothing', () => {
  const a = studio().engines;
  const b = studio(structuredClone(STUDIO)).engines;
  assert.deepEqual(a, b);
  const empty = engineLayout({ entry: 'x', rect: { x: 0, y: 0, w: 1280, h: 200 }, names: [], graph: STUDIO });
  assert.deepEqual([empty.cards, empty.wires, empty.pins], [[], [], []]);
});

test('odd graphs: a port inside an instance is not its own, two wires entering from one source keep apart, a malformed end leaves no edge pin', () => {
  const inst = (name, ins, outs) => ({ name, module: 'std.x', ports: [...ins.map((p) => ({ name: p, address: `/${name}.${p}`, direction: 'in', shape: 'signal', kind: 'audio', rate: 1 })), ...outs.map((p) => ({ name: p, address: `/${name}.${p}`, direction: 'out', shape: 'signal', kind: 'audio', rate: 1 }))], parameters: [] });
  const level = {
    instances: [inst('a', ['in'], ['out']), inst('b', ['in'], ['out']), inst('c', ['in'], ['out']), inst('x', [], ['out'])],
    connections: [
      { from: '/a.out', to: '/b.in' },
      { from: '/a/inner.out', to: '/b.in' },
      { from: '/x.out', to: '/a.in' },
      { from: '/x.out', to: '/c.in' },
      { from: '/x.out', to: '/b.out' },
      { from: '/b.out', to: '/b.in' },
    ],
  };
  const laid = layLevel({ level, names: ['a', 'b', 'c'], area: { x: 0, y: 0, w: 800, h: 300 }, edgeIns: new Map([['/a.in', 40], ['/c.in', 200]]) });
  assert.deepEqual(laid.wires.map((w) => `${w.from}>${w.to}`), ['/a.out>/b.in', '/x.out>/a.in', '/x.out>/c.in', '/b.out>/b.in'], 'the nested address and the malformed end are not drawn');
  assert.deepEqual(laid.edges.map((e) => e.id), ['edge in /a.in', 'edge in /c.in'], 'no orphan edge pin');
  const [ea, ec] = laid.wires.filter((w) => w.edge === 'in');
  // Each entering wire its own net: on one track only where their spans are apart, never joined into one.
  const span = (w) => [Math.min(...w.points.slice(1, -1).map((p) => p.y)), Math.max(...w.points.slice(1, -1).map((p) => p.y))];
  const [sa, sc] = [span(ea), span(ec)];
  assert.ok(ea.points[1].x !== ec.points[1].x || sa[1] < sc[0] || sc[1] < sa[0], 'never joined on one vertical');
  assert.deepEqual([ea.junctions, ec.junctions], [[], []]);
  for (const w of laid.wires) for (const p of w.points) assert.ok(Number.isFinite(p.x) && Number.isFinite(p.y));
});

// Step 2: a composite's inside, revealed by zoom in its card's body (the owner's, 2026-11-03).

test('a chip’s inside: every inner instance inside the card’s body, apart; the edges at the card’s pins’ heights; an input fanning out one net', () => {
  const { engines } = studio();
  let chips = 0;
  for (const e of engines) for (const card of e.cards.filter((c) => c.composite)) {
    chips++;
    const inside = insideOf({ graph: STUDIO, card, pins: e.pins, scale: e.scale });
    const def = STUDIO.composites[card.module];
    assert.deepEqual(inside.cards.map((c) => c.name).sort(), def.instances.map((i) => i.name).sort(), `${card.name}: every inner instance placed`);
    assert.ok(inside.scale < e.scale, `${card.name}: scaled into the body`);
    for (const c of inside.cards) assert.ok(within(c, card.body), `${card.name}: ${c.name} inside the body`);
    for (let i = 0; i < inside.cards.length; i++) for (let j = i + 1; j < inside.cards.length; j++) assert.ok(!overlap(inside.cards[i], inside.cards[j]), `${card.name}: ${inside.cards[i].name} and ${inside.cards[j].name} apart`);
    const pins = e.pins.filter((p) => p.card === card.id);
    for (const edge of inside.edges) {
      const name = edge.side === 'in' ? edge.from.slice('@inputs.'.length) : edge.to.slice('@outputs.'.length);
      const pin = pins.find((p) => p.side === edge.side && p.name === name);
      assert.ok(pin, `${card.name}: ${edge.id} has its pin`);
      assert.equal(edge.y, pin.y, `${card.name}: ${edge.id} at its pin’s height`);
      assert.equal(edge.x, edge.side === 'in' ? card.x : card.x + card.w, `${card.name}: ${edge.id} on the card’s edge`);
    }
    for (const w of inside.wires) for (let i = 1; i < w.points.length; i++) {
      const [p, q] = [w.points[i - 1], w.points[i]];
      assert.ok(within({ x: Math.min(p.x, q.x), y: Math.min(p.y, q.y), w: Math.abs(p.x - q.x), h: Math.abs(p.y - q.y) }, card), `${card.name}: ${w.id} inside the card`);
      for (const c of inside.cards) assert.ok(!crosses(p, q, c, 0.5 * inside.scale), `${card.name}: ${w.from} → ${w.to} passes over ${c.name}`);
    }
  }
  assert.equal(chips, 9, 'the synth, the seven drum voices and the echo');
  const synth = engines.find((e) => e.entry === 'card:mono-synth');
  const voice = synth.cards.find((c) => c.name === 'synth');
  const inside = insideOf({ graph: STUDIO, card: voice, pins: synth.pins, scale: synth.scale });
  assert.deepEqual(inside.edges.filter((x) => x.side === 'in').map((x) => `${x.from} ${x.address}`), ['@inputs.gate fenv.gate', '@inputs.gate aenv.gate', '@inputs.hz glide.in', '@inputs.hz hz_prev.in', '@inputs.velocity velamp.b']);
  assert.deepEqual(inside.edges.filter((x) => x.side === 'out').map((x) => x.address), ['volume.out']);
  // The gate's two branches: one net, so one track in the edge channel (no two wires along one another).
  const gate = inside.wires.filter((w) => w.from === '@inputs.gate');
  assert.equal(gate.length, 2);
  assert.equal(gate[0].points[1].x, gate[1].points[1].x, 'the gate’s branches share their track');
  // The kick's velocity is a parameter its sequencer drives: its inside's edge enters at that pin, to every inner port it sets.
  const drums = engines.find((e) => e.entry === 'card:rhythm-composer');
  const kick = drums.cards.find((c) => c.name === 'kick');
  const kickInside = insideOf({ graph: STUDIO, card: kick, pins: drums.pins, scale: drums.scale });
  const velocity = drums.pins.find((p) => p.card === kick.id && p.name === 'velocity');
  const targets = STUDIO.composites.kick909.parameters.find((p) => p.name === 'velocity').to;
  assert.deepEqual(kickInside.edges.filter((x) => x.from === '@inputs.velocity').map((x) => x.address), targets);
  for (const x of kickInside.edges.filter((y) => y.from === '@inputs.velocity')) assert.equal(x.y, velocity.y);
  // No such composite: no inside.
  assert.equal(insideOf({ graph: STUDIO, card: { ...kick, module: 'nope' }, pins: drums.pins }), null);
});

/** A graph with a composite inside a composite: `fx` (an `outer`: a filter `inner` between two gains) at the top. */
function nested() {
  const inst = (name, module, ins, outs, composite = false) => ({
    name,
    address: name,
    module,
    composite,
    parameters: [{ name: 'gain', address: `${name}.gain` }],
    ports: [...ins.map((p) => ({ name: p, address: `${name}.${p}`, direction: 'in', shape: 'signal', kind: 'audio', rate: 1 })), ...outs.map((p) => ({ name: p, address: `${name}.${p}`, direction: 'out', shape: 'signal', kind: 'audio', rate: 1 }))],
  });
  const top = (i) => ({ ...i, address: `/${i.name}`, ports: i.ports.map((p) => ({ ...p, address: `/${p.address}` })) });
  return {
    top: {
      instances: [top(inst('src', 'std.osc', [], ['out'])), top(inst('fx', 'outer', ['in'], ['out'], true))],
      connections: [{ from: '/src.out', to: '/fx.in' }],
    },
    composites: {
      outer: {
        inputs: [{ name: 'in', to: ['pre.in'] }],
        outputs: [{ name: 'out', from: 'post.out' }],
        parameters: [],
        instances: [inst('pre', 'std.gain', ['in'], ['out']), inst('inner', 'inner', ['in'], ['out'], true), inst('post', 'std.gain', ['in'], ['out'])],
        connections: [{ from: 'pre.out', to: 'inner.in' }, { from: 'inner.out', to: 'post.in' }],
      },
      inner: {
        inputs: [{ name: 'in', to: ['lp.in', 'hp.in'] }],
        outputs: [{ name: 'out', from: 'mix.out' }],
        parameters: [],
        instances: [inst('lp', 'std.lp', ['in'], ['out']), inst('hp', 'std.hp', ['in'], ['out']), inst('mix', 'std.mix', ['a', 'b'], ['out'])],
        connections: [{ from: 'lp.out', to: 'mix.a' }, { from: 'hp.out', to: 'mix.b' }],
      },
    },
  };
}

test('recursive: a chip inside a chip reveals in its own card’s body, its edges at that card’s pins, however small; deterministic', () => {
  const graph = nested();
  // A roomy level, and one so small that the inner chip's card is a few plane units tall.
  for (const area of [{ x: 0, y: 0, w: 1000, h: 400 }, { x: 0, y: 0, w: 60, h: 24 }]) {
    const top = layLevel({ level: graph.top, area });
    const fx = top.cards.find((c) => c.name === 'fx');
    const outer = insideOf({ graph, card: fx, pins: top.pins, scale: top.scale });
    const inner = outer.cards.find((c) => c.name === 'inner');
    assert.equal(inner.composite, true);
    const deep = insideOf({ graph, card: inner, pins: outer.pins, scale: outer.scale });
    assert.deepEqual(deep.cards.map((c) => c.name).sort(), ['hp', 'lp', 'mix']);
    for (const c of deep.cards) assert.ok(within(c, inner.body) && within(c, fx.body), `${c.name} inside its card’s body, inside the outer one’s`);
    assert.ok(deep.scale < outer.scale && outer.scale < top.scale, 'each level smaller');
    const pinY = (side) => outer.pins.find((p) => p.card === inner.id && p.side === side).y;
    assert.deepEqual(deep.edges.map((x) => [x.side, x.address, x.y]), [['in', 'lp.in', pinY('in')], ['in', 'hp.in', pinY('in')], ['out', 'mix.out', pinY('out')]]);
    for (const w of deep.wires) for (const p of w.points) assert.ok(within({ x: p.x, y: p.y, w: 0, h: 0 }, inner), `${w.id} inside the inner card (${inner.h.toFixed(2)} tall)`);
    for (const w of deep.wires) for (let i = 1; i < w.points.length; i++) for (const c of deep.cards) assert.ok(!crosses(w.points[i - 1], w.points[i], c, 0.5 * deep.scale), `${w.from} → ${w.to} passes over ${c.name}`);
    assert.deepEqual(insideOf({ graph: structuredClone(graph), card: inner, pins: outer.pins, scale: outer.scale }), deep, 'the same inside every time');
  }
});

test('reveal: none below the first width, whole from the second, monotonic between', () => {
  assert.equal(revealShare(0), 0);
  assert.equal(revealShare(REVEAL[0]), 0);
  assert.equal(revealShare(REVEAL[1]), 1);
  assert.equal(revealShare(1e9), 1);
  let last = 0;
  for (let px = REVEAL[0]; px <= REVEAL[1]; px += 5) {
    const s = revealShare(px);
    assert.ok(s >= last && s <= 1);
    last = s;
  }
});

test('Layer N: the chips whose fully revealed body holds the view’s centre, outermost first; none when too small or outside', () => {
  const graph = nested();
  const top = layLevel({ level: graph.top, area: { x: 0, y: 0, w: 1000, h: 400 } });
  const fx = top.cards.find((c) => c.name === 'fx');
  const insides = new Map();
  const insideFor = (card, id) => {
    if (!insides.has(id)) {
      const parent = id.includes('/') ? insides.get(id.slice(0, id.lastIndexOf('/'))) : top;
      insides.set(id, insideOf({ graph, card, pins: parent.pins, scale: parent.scale }));
    }
    return insides.get(id);
  };
  const outer = insideFor(fx, fx.id);
  const inner = outer.cards.find((c) => c.name === 'inner');
  const centre = (r) => ({ x: r.x + r.w / 2, y: r.y + r.h / 2 });
  const k1 = REVEAL[1] / fx.body.w;
  assert.deepEqual(chipPath(top.cards, centre(inner.body), k1 * 0.99, insideFor), [], 'not yet fully revealed');
  assert.deepEqual(chipPath(top.cards, centre(inner.body), k1, insideFor).map((s) => s.id), ['card fx'], 'Layer 1: inside fx; inner too small yet');
  const k2 = REVEAL[1] / inner.body.w;
  const two = chipPath(top.cards, centre(inner.body), k2, insideFor);
  assert.deepEqual(two.map((s) => s.id), ['card fx', 'card fx/card inner'], 'Layer 2: inside inner, inside fx');
  assert.deepEqual(two.map((s) => s.card.name), ['fx', 'inner']);
  assert.equal(two[1].laid, insides.get('card fx/card inner'), 'from the cached insides');
  assert.deepEqual(chipPath(top.cards, { x: fx.body.x - 1, y: fx.body.y }, k2, insideFor), [], 'the centre outside every chip');
  const pre = outer.cards.find((c) => c.name === 'pre');
  assert.deepEqual(chipPath(top.cards, centre(pre.body), k2, insideFor).map((s) => s.id), ['card fx'], 'on a plain card inside: Layer 1 only');
});
