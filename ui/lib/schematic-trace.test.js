// The schematic's path tracing (ADR 0030 §1, docs/ui-controls.md
// "Schematic"): across composite boundaries both ways and nested, fan-out,
// fan-in, cycles, converters, library composites as opaque ends, both
// directions, every kind of start; the badges, the tree, and the time.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { BADGE_NAMES, COLLAPSE_PINS, badgeText, behind, pinInfo, startOf, trace, traceTree, tracedWire, wireKey, within } from './schematic-trace.js';
import { layout } from './schematic-layout.js';
import { exportFacts } from './schematic-groups.js';
import { openedLayout } from './schematic-open.js';

const port = (name, direction, shape = 'signal') => ({ name, direction, shape, kind: 'generic', rate: 1 });
/** An instance entry as `graph` gives it. */
function inst(name, module, { ins = [], outs = [], params = [], composite = false } = {}) {
  return { name, module, composite, ports: [...ins.map((p) => port(p, 'in')), ...outs.map((p) => port(p, 'out'))], parameters: params.map((p) => ({ name: p, kind: 'control' })) };
}
const conn = (from, to, converters = []) => ({ from, to, scale: null, converters });
const conv = (name) => ({ name, module: `core.decimate.${name}`, conversion: 'decimate', fromRate: 1, toRate: 800 });

// seq → voice (composite: hz fans out to three oscillators through a nested
// `pitch` composite; its `out` from `vca`) → mix ← noise; mix → fx (a
// library composite, opaque) → out; a feedback loop lfo ⇄ fb.
export const GRAPH = {
  project: 't',
  top: {
    instances: [
      inst('seq', 'std.note_seq', { outs: ['hz', 'gate'] }),
      inst('voice', 'voice', { ins: ['hz', 'gate', 'spare'], outs: ['out'], params: ['cutoff'], composite: true }),
      inst('noise', 'std.noise', { outs: ['out'] }),
      inst('mix', 'std.mix', { ins: ['a', 'b'], outs: ['out'] }),
      inst('fx', 'lib.tape_echo', { ins: ['in'], outs: ['out'], params: ['mix'], composite: true }),
      inst('out', 'std.audio_out', { ins: ['in'] }),
      inst('lfo', 'std.osc', { ins: ['freq'], outs: ['out'] }),
      inst('fb', 'std.scale', { ins: ['in'], outs: ['out'] }),
    ],
    connections: [
      conn('/seq.hz', '/voice.hz'),
      conn('/seq.gate', '/voice.gate'),
      conn('/voice.out', '/mix.a'),
      conn('/noise.out', '/mix.b'),
      conn('/mix.out', '/fx.in'),
      conn('/fx.out', '/out.in'),
      conn('/lfo.out', '/fb.in'),
      conn('/fb.out', '/lfo.freq'),
      conn('/lfo.out', '/voice.cutoff', [conv('out_d800')]),
    ],
  },
  composites: {
    voice: {
      inputs: [
        { name: 'hz', direction: 'in', shape: 'signal', to: ['pitch.hz'] },
        { name: 'gate', direction: 'in', shape: 'signal', to: ['env.gate'] },
        { name: 'spare', direction: 'in', shape: 'signal', to: [] },
      ],
      outputs: [{ name: 'out', direction: 'out', shape: 'signal', from: 'vca.out' }],
      parameters: [{ name: 'cutoff', to: ['filt.cutoff'] }],
      instances: [
        inst('pitch', 'pitch', { ins: ['hz'], outs: ['o1', 'o2', 'o3'], composite: true }),
        inst('osc1', 'std.osc', { ins: ['freq'], outs: ['out'] }),
        inst('osc2', 'std.osc', { ins: ['freq'], outs: ['out'] }),
        inst('osc3', 'std.osc', { ins: ['freq'], outs: ['out'] }),
        inst('sum', 'std.mix', { ins: ['a', 'b', 'c'], outs: ['out'] }),
        inst('filt', 'std.ladder', { ins: ['in'], outs: ['out'], params: ['cutoff'] }),
        inst('env', 'std.adsr', { ins: ['gate'], outs: ['out'] }),
        inst('vca', 'std.mul', { ins: ['a', 'b'], outs: ['out'] }),
      ],
      connections: [
        conn('pitch.o1', 'osc1.freq'),
        conn('pitch.o2', 'osc2.freq'),
        conn('pitch.o3', 'osc3.freq'),
        conn('osc1.out', 'sum.a'),
        conn('osc2.out', 'sum.b'),
        conn('osc3.out', 'sum.c'),
        conn('sum.out', 'filt.in'),
        conn('filt.out', 'vca.a'),
        conn('env.out', 'vca.b'),
      ],
    },
    // Nested: hz fans out to three ratios, each a primitive.
    pitch: {
      inputs: [{ name: 'hz', direction: 'in', shape: 'signal', to: ['r1.in', 'r2.in', 'r3.in'] }],
      outputs: [
        { name: 'o1', direction: 'out', shape: 'signal', from: 'r1.out' },
        { name: 'o2', direction: 'out', shape: 'signal', from: 'r2.out' },
        { name: 'o3', direction: 'out', shape: 'signal', from: 'r3.out' },
      ],
      parameters: [],
      instances: [inst('r1', 'std.scale', { ins: ['in'], outs: ['out'] }), inst('r2', 'std.scale', { ins: ['in'], outs: ['out'] }), inst('r3', 'std.scale', { ins: ['in'], outs: ['out'] })],
      connections: [],
    },
  },
};

const ends = (side) => side.endpoints.map((e) => e.address).sort();

test('downstream from an exported input: into the composite, through a nested one, to the primitives it reaches', () => {
  const t = trace(GRAPH, { kind: 'pin', address: '/voice.hz' }, 'down');
  assert.equal(t.up, null);
  assert.deepEqual(ends(t.down), ['/voice/pitch/r1.in', '/voice/pitch/r2.in', '/voice/pitch/r3.in']);
  for (const e of t.down.endpoints) assert.equal(e.kind, 'primitive');
  assert.ok(tracedWire(t, '/voice', { from: 'hz', to: 'pitch.hz' }));
  assert.ok(tracedWire(t, '/voice/pitch', { from: 'hz', to: 'r2.in' }));
  assert.ok(t.pins.has('/voice/pitch.hz'));
  assert.deepEqual(t.levels, ['', '/voice', '/voice/pitch']);
  // The hops: voice.hz → pitch.hz → r1.in.
  assert.equal(t.down.endpoints[0].hops, 2);
});

test('upstream from an inner pin: out through the boundaries to the source at the top level', () => {
  const t = trace(GRAPH, { kind: 'pin', address: '/voice/pitch/r3.in' }, 'up');
  assert.deepEqual(ends(t.up), ['/seq.hz']);
  assert.ok(tracedWire(t, '', { from: '/seq.hz', to: '/voice.hz' }));
  assert.ok(tracedWire(t, '/voice', { from: 'hz', to: 'pitch.hz' }));
  assert.ok(tracedWire(t, '/voice/pitch', { from: 'hz', to: 'r3.in' }));
  assert.ok(!tracedWire(t, '/voice/pitch', { from: 'hz', to: 'r1.in' }), 'a sibling of the fan-out is not upstream');
  assert.equal(t.down, null);
});

test('an exported output, out of a nested composite and up to the top', () => {
  const t = trace(GRAPH, { kind: 'pin', address: '/voice/pitch.o2' }, 'both');
  assert.deepEqual(ends(t.up), ['/voice/pitch/r2.out']);
  assert.deepEqual(ends(t.down), ['/voice/osc2.freq']);
  assert.ok(tracedWire(t, '/voice/pitch', { from: 'r2.out', to: 'o2' }));
});

test('fan-out: every branch, each endpoint once', () => {
  const t = trace(GRAPH, { kind: 'pin', address: '/lfo.out' }, 'down');
  // lfo.out → fb.in, and → voice.cutoff → filt.cutoff (inside).
  assert.deepEqual(ends(t.down), ['/fb.in', '/voice/filt.cutoff']);
});

test('fan-in: upstream of a mixer reaches all its sources', () => {
  const t = trace(GRAPH, { kind: 'instance', address: '/mix' }, 'up');
  assert.deepEqual(ends(t.up), ['/noise.out', '/voice/vca.out']);
  const inner = trace(GRAPH, { kind: 'pin', address: '/voice/sum.out' }, 'up');
  assert.deepEqual(ends(inner.up), []);
  const sum = trace(GRAPH, { kind: 'instance', address: '/voice/sum' }, 'up');
  assert.deepEqual(ends(sum.up), ['/voice/osc1.out', '/voice/osc2.out', '/voice/osc3.out']);
});

test('converters: the connection is followed and named on the way', () => {
  const t = trace(GRAPH, { kind: 'pin', address: '/voice/filt.cutoff' }, 'up');
  assert.deepEqual(ends(t.up), ['/lfo.out']);
  assert.deepEqual(t.up.endpoints[0].via, ['out_d800']);
  const down = trace(GRAPH, { kind: 'pin', address: '/lfo.out' }, 'down');
  assert.deepEqual(down.down.endpoints.find((e) => e.address === '/voice/filt.cutoff').via, ['out_d800']);
});

test('cycles terminate: loops of primitives, a composite fed back into itself, a loop of exports alone', () => {
  // A wire start in a loop of primitives: the trace ends at the primitives' ports, never passing through one.
  const t = trace(GRAPH, { kind: 'wire', path: '', written: { from: '/fb.out', to: '/lfo.freq' } }, 'both');
  assert.ok(tracedWire(t, '', { from: '/fb.out', to: '/lfo.freq' }));
  assert.ok(!tracedWire(t, '', { from: '/lfo.out', to: '/fb.in' }));
  // A loop across a composite: its output fed back into its own input.
  const loop = structuredClone(GRAPH);
  loop.composites.inner = {
    inputs: [{ name: 'in', direction: 'in', shape: 'signal', to: ['pass.in'] }],
    outputs: [{ name: 'out', direction: 'out', shape: 'signal', from: 'pass.out' }],
    parameters: [],
    instances: [inst('pass', 'loopback', { ins: ['in'], outs: ['out'], composite: true })],
    connections: [],
  };
  // A composite whose inside wires its input straight to its output, used inside itself's chain.
  loop.composites.loopback = {
    inputs: [{ name: 'in', direction: 'in', shape: 'signal', to: ['x.in'] }],
    outputs: [{ name: 'out', direction: 'out', shape: 'signal', from: 'x.out' }],
    parameters: [],
    instances: [inst('x', 'std.scale', { ins: ['in'], outs: ['out'] })],
    connections: [],
  };
  loop.top.instances.push(inst('ring', 'inner', { ins: ['in'], outs: ['out'], composite: true }));
  loop.top.connections.push(conn('/ring.out', '/ring.in'));
  const r = trace(loop, { kind: 'pin', address: '/ring.out' }, 'both');
  assert.deepEqual(ends(r.up), ['/ring/pass/x.out']);
  assert.deepEqual(ends(r.down), ['/ring/pass/x.in']);
  // A pure export loop (no primitive on it) ends too: in → out inside, out → in outside.
  const pure = structuredClone(loop);
  pure.composites.wire = { inputs: [{ name: 'in', direction: 'in', shape: 'signal', to: [] }], outputs: [], parameters: [], instances: [], connections: [] };
  pure.composites.inner.connections = [];
  pure.composites.inner.inputs[0].to = [];
  pure.composites.inner.outputs[0].from = null;
  const p = trace(pure, { kind: 'pin', address: '/ring.in' }, 'both');
  assert.deepEqual(ends(p.up), ['/ring.out']);
  assert.equal(p.up.endpoints[0].kind, 'unconnected');
});

test('a loop ends at a primitive, and a loop of exports between composites reaches each pin once', () => {
  const g = structuredClone(GRAPH);
  // fb.out also reaches lfo.freq, which lfo.out reaches through fb: walking up from fb.in goes round.
  const t = trace(g, { kind: 'pin', address: '/fb.in' }, 'up');
  assert.deepEqual(ends(t.up), ['/lfo.out']);
  // Pins are not passed through: the loop ends at the primitive, so no cycle there.
  assert.equal(t.cycles, 0);
  // A loop of exports between two composites: each pin once.
  const c = structuredClone(GRAPH);
  c.composites.a = { inputs: [{ name: 'i', direction: 'in', shape: 'signal', to: ['b.i'] }], outputs: [{ name: 'o', direction: 'out', shape: 'signal', from: 'b.o' }], parameters: [], instances: [inst('b', 'b', { ins: ['i'], outs: ['o'], composite: true })], connections: [] };
  c.composites.b = { inputs: [{ name: 'i', direction: 'in', shape: 'signal', to: [] }], outputs: [{ name: 'o', direction: 'out', shape: 'signal', from: null }], parameters: [], instances: [], connections: [] };
  c.composites.b.connections = [];
  c.top.instances.push(inst('a1', 'a', { ins: ['i'], outs: ['o'], composite: true }), inst('a2', 'a', { ins: ['i'], outs: ['o'], composite: true }));
  c.top.connections.push(conn('/a1.o', '/a2.i'), conn('/a2.o', '/a1.i'));
  const both = trace(c, { kind: 'pin', address: '/a1.i' }, 'both');
  assert.ok(both.pins.has('/a2.o'));
  assert.deepEqual(ends(both.down), ['/a1/b.i']);
});

test('a library composite is an opaque end: its inside is not in graph', () => {
  const t = trace(GRAPH, { kind: 'pin', address: '/mix.out' }, 'down');
  assert.deepEqual(ends(t.down), ['/fx.in']);
  assert.equal(t.down.endpoints[0].kind, 'library');
  assert.deepEqual(t.opaque, ['/fx.in']);
  const up = trace(GRAPH, { kind: 'pin', address: '/out.in' }, 'up');
  assert.deepEqual(ends(up.up), ['/fx.out']);
  assert.equal(up.up.endpoints[0].kind, 'library');
  assert.equal(pinInfo(GRAPH, '/fx.in').opaque, true);
  assert.equal(pinInfo(GRAPH, '/voice.hz').opaque, false);
});

test('both directions, and an instance start: inputs upstream, outputs downstream', () => {
  const t = trace(GRAPH, { kind: 'instance', address: '/voice' }, 'both');
  assert.deepEqual(ends(t.up), ['/lfo.out', '/seq.gate', '/seq.hz']);
  assert.deepEqual(ends(t.down), ['/mix.a']);
  // Its own pins are in the trace, even one nothing reaches.
  assert.ok(t.pins.has('/voice.hz') && t.pins.has('/voice.out') && t.pins.has('/voice.cutoff'));
  assert.ok(t.instances.has('/voice'));
  const up = trace(GRAPH, { kind: 'instance', address: '/voice' }, 'up');
  assert.equal(up.down, null);
  assert.ok(!tracedWire(up, '', { from: '/voice.out', to: '/mix.a' }));
  const down = trace(GRAPH, { kind: 'instance', address: '/voice' }, 'down');
  assert.equal(down.up, null);
  assert.ok(tracedWire(down, '', { from: '/voice.out', to: '/mix.a' }));
});

test('a wire start: its source upstream, its target downstream, the wire itself', () => {
  const t = trace(GRAPH, { kind: 'wire', path: '/voice', written: { from: 'filt.out', to: 'vca.a' } }, 'both');
  assert.equal(t.label, '/voice/filt.out → /voice/vca.a');
  assert.ok(tracedWire(t, '/voice', { from: 'filt.out', to: 'vca.a' }));
  // Upstream of filt.out: nothing (a primitive's output is an end).
  assert.deepEqual(ends(t.up), []);
  assert.deepEqual(ends(t.down), []);
  // An export wire, named as the layout writes it.
  const e = trace(GRAPH, { kind: 'wire', path: '/voice', written: { from: 'vca.out', to: 'out' } }, 'down');
  assert.deepEqual(ends(e.down), ['/mix.a']);
  assert.equal(trace(GRAPH, { kind: 'wire', path: '/voice', written: { from: 'nope', to: 'x' } }, 'both'), null);
});

test('a start not in the graph, a missing graph, an unknown pin', () => {
  assert.equal(trace(null, { kind: 'pin', address: '/a.b' }), null);
  assert.equal(trace(GRAPH, { kind: 'pin', address: '/nothing.here' }), null);
  assert.equal(trace(GRAPH, { kind: 'instance', address: '/voice/nothing' }), null);
  assert.equal(trace(GRAPH, null), null);
});

test('an exported input leading nowhere is an open end, and an unconnected one has no ends', () => {
  const t = trace(GRAPH, { kind: 'pin', address: '/voice.spare' }, 'both');
  assert.deepEqual(ends(t.up), []);
  assert.deepEqual(ends(t.down), []);
  const g = structuredClone(GRAPH);
  g.top.connections.push(conn('/seq.gate', '/voice.spare'));
  const u = trace(g, { kind: 'pin', address: '/seq.gate' }, 'down');
  assert.ok(u.down.endpoints.some((e) => e.address === '/voice.spare' && e.kind === 'unconnected'));
});

test('startOf: hits of a pin, a frame pin, an instance, a wire', () => {
  assert.deepEqual(startOf({ kind: 'port', address: '/voice.hz' }), { kind: 'pin', address: '/voice.hz' });
  assert.deepEqual(startOf({ kind: 'instance', address: '/voice' }), { kind: 'instance', address: '/voice' });
  assert.deepEqual(startOf({ kind: 'wire', path: '/voice', wire: { written: { from: 'a.out', to: 'b.in' } } }), { kind: 'wire', path: '/voice', written: { from: 'a.out', to: 'b.in' } });
  assert.equal(startOf({ kind: 'background' }), null);
  assert.equal(startOf(null), null);
});

test('behind: what a boundary pin hides, inside and outside, and the badge text', () => {
  const inside = behind(GRAPH, '/voice.hz', 'inside');
  assert.equal(inside.direction, 'down');
  assert.deepEqual(ends(inside), ['/voice/pitch/r1.in', '/voice/pitch/r2.in', '/voice/pitch/r3.in']);
  assert.equal(badgeText(inside, '/voice.hz', 'inside'), '→ 3 inside: pitch/r1.in, pitch/r2.in, pitch/r3.in');
  const outside = behind(GRAPH, '/voice.hz', 'outside');
  assert.equal(badgeText(outside, '/voice.hz', 'outside'), '← from /seq.hz');
  assert.equal(badgeText(behind(GRAPH, '/voice.out', 'outside'), '/voice.out', 'outside'), '→ to /mix.a');
  assert.equal(badgeText(behind(GRAPH, '/voice.out', 'inside'), '/voice.out', 'inside'), '← inside from vca.out');
  assert.equal(badgeText(behind(GRAPH, '/voice.spare', 'inside'), '/voice.spare', 'inside'), '→ nothing inside');
  assert.equal(badgeText(behind(GRAPH, '/voice.spare', 'outside'), '/voice.spare', 'outside'), '← nothing outside');
  // Outside goes through other composites to their primitives, never into this one.
  assert.equal(badgeText(behind(GRAPH, '/voice/pitch.hz', 'outside'), '/voice/pitch.hz', 'outside'), '← from /seq.hz');
  assert.equal(badgeText(behind(GRAPH, '/fx.in', 'inside'), '/fx.in', 'inside'), '→ inside a library composite (not in the graph)');
  assert.equal(badgeText(behind(GRAPH, '/fx.in', 'outside'), '/fx.in', 'outside'), '← from /mix.out');
  assert.equal(behind(GRAPH, '/mix.a', 'inside'), null, 'a primitive has no inside');
  // Long lists are cut.
  const g = structuredClone(GRAPH);
  g.composites.pitch.inputs[0].to = ['r1.in', 'r2.in', 'r3.in', 'r1.in2', 'r2.in2'];
  for (const r of g.composites.pitch.instances) r.ports.push(port('in2', 'in'));
  const text = badgeText(behind(g, '/voice/pitch.hz', 'inside'), '/voice/pitch.hz', 'inside');
  assert.match(text, new RegExp(`^→ 5 inside: (\\S+, ){${BADGE_NAMES - 1}}\\S+, … 2 more$`));
});

test('the tree by level: outermost first, counts, roles, the graph order', () => {
  const t = trace(GRAPH, { kind: 'pin', address: '/voice.hz' }, 'both');
  const tree = traceTree(GRAPH, t);
  assert.deepEqual(
    tree.map((l) => [l.path, l.label, l.wires, l.pins]),
    [
      ['', 'top level', 1, 2],
      ['/voice', '/voice (voice)', 1, 1],
      ['/voice/pitch', '/voice/pitch (pitch)', 3, 3],
    ],
  );
  assert.deepEqual(
    tree[0].children.map((c) => [c.name, c.children.map((p) => `${p.port}:${p.role}`)]),
    [
      ['seq', ['hz:endpoint']],
      ['voice', ['hz:start']],
    ],
  );
  assert.equal(tree[1].children[0].composite, true);
  assert.deepEqual(
    tree[2].children.map((c) => c.name),
    ['r1', 'r2', 'r3'],
  );
  // An instance start marks all its pins as the start; a library composite is opaque.
  const v = traceTree(GRAPH, trace(GRAPH, { kind: 'instance', address: '/mix' }, 'down'));
  const fx = v[0].children.find((c) => c.name === 'fx');
  assert.equal(fx.opaque, true);
  assert.equal(fx.children[0].role, 'endpoint');
  assert.equal(v[0].children.find((c) => c.name === 'mix').children.every((p) => p.role === 'start'), true);
  assert.deepEqual(traceTree(GRAPH, null), []);
});

test('helpers: within, wireKey', () => {
  assert.ok(within('/a', '/a') && within('/a/b', '/a') && !within('/ab', '/a') && !within('', '/a'));
  assert.equal(wireKey({ from: 'a.out', to: 'b.in' }), 'a.out\nb.in');
});

/**
 * A project the studio's size: ~180 instances, voices of 20-port composites
 * (a nested one each), every voice fed by a sequencer and summed.
 */
function bigGraph(voices = 9) {
  const top = { instances: [inst('seq', 'std.note_seq', { outs: ['hz', 'gate'] }), inst('bus', 'std.mix', { ins: Array.from({ length: voices }, (_, i) => `i${i}`), outs: ['out'] })], connections: [] };
  const params = Array.from({ length: 17 }, (_, i) => `p${i}`);
  const inner = [];
  const innerConns = [];
  for (let i = 0; i < 18; i++) inner.push(inst(`n${i}`, 'std.scale', { ins: ['in'], outs: ['out'], params: ['mul'] }));
  for (let i = 1; i < 18; i++) innerConns.push(conn(`n${i - 1}.out`, `n${i}.in`));
  inner.push(inst('sub', 'sub', { ins: ['in'], outs: ['out'], composite: true }));
  innerConns.push(conn('n17.out', 'sub.in'));
  const composites = {
    v: {
      inputs: [
        { name: 'hz', direction: 'in', shape: 'signal', to: ['n0.in', 'n5.in', 'n9.in'] },
        { name: 'gate', direction: 'in', shape: 'signal', to: ['n3.in'] },
      ],
      outputs: [{ name: 'out', direction: 'out', shape: 'signal', from: 'sub.out' }],
      parameters: params.map((p, i) => ({ name: p, to: [`n${i}.mul`] })),
      instances: inner,
      connections: innerConns,
    },
    sub: {
      inputs: [{ name: 'in', direction: 'in', shape: 'signal', to: ['a.in'] }],
      outputs: [{ name: 'out', direction: 'out', shape: 'signal', from: 'a.out' }],
      parameters: [],
      instances: [inst('a', 'std.scale', { ins: ['in'], outs: ['out'] })],
      connections: [],
    },
  };
  for (let i = 0; i < voices; i++) {
    top.instances.push(inst(`v${i}`, 'v', { ins: ['hz', 'gate'], outs: ['out'], params, composite: true }));
    top.connections.push(conn('/seq.hz', `/v${i}.hz`), conn('/seq.gate', `/v${i}.gate`), conn(`/v${i}.out`, `/bus.i${i}`));
  }
  return { project: 'big', top, composites };
}

test('a studio-sized project traces in under 10 ms, every top-level instance both ways', () => {
  const g = bigGraph(9);
  const count = g.top.instances.length + 9 * 20;
  assert.ok(count >= 180);
  // Warm the caches once, as the page does on the first trace.
  trace(g, { kind: 'instance', address: '/v0' }, 'both');
  const t0 = performance.now();
  const t = trace(g, { kind: 'pin', address: '/seq.hz' }, 'both');
  const one = performance.now() - t0;
  assert.equal(t.down.endpoints.length, 27);
  assert.ok(one < 10, `one trace took ${one} ms`);
  const t1 = performance.now();
  for (const n of g.top.instances) trace(g, { kind: 'instance', address: `/${n.name}` }, 'both');
  const all = performance.now() - t1;
  assert.ok(all < 10 * g.top.instances.length, `all took ${all} ms`);
});

test('a badge naming one inner port says so without a count', () => {
  assert.equal(badgeText(behind(GRAPH, '/voice.gate', 'inside'), '/voice.gate', 'inside'), '→ inside: env.gate');
  assert.equal(badgeText(behind(GRAPH, '/voice.cutoff', 'inside'), '/voice.cutoff', 'inside'), '→ inside: filt.cutoff');
});

// Through modules (M2.13-2): a primitive crossed from its inputs and
// parameters to all its outputs (downstream) and back (upstream), to sinks
// and sources; the chain's order and the collapse; the studio's synth.

const through = { depth: 'through' };
const has = (t, ...addresses) => addresses.every((a) => t.pins.has(a));

test('through modules: downstream from a pin crosses every module to the ends', () => {
  const next = trace(GRAPH, { kind: 'pin', address: '/seq.hz' }, 'down');
  assert.equal(next.depth, 'next');
  assert.ok(!next.pins.has('/voice/osc1.out'), 'to the next module stops at its inputs');
  const t = trace(GRAPH, { kind: 'pin', address: '/seq.hz' }, 'down', through);
  assert.equal(t.depth, 'through');
  assert.ok(has(t, '/voice/osc1.out', '/voice/osc2.out', '/voice/osc3.out', '/voice/sum.out', '/voice/filt.out', '/voice/vca.out', '/voice.out', '/mix.out', '/fx.in'));
  // A library composite still ends it (its inside is not in the graph).
  assert.deepEqual(
    t.down.endpoints.map((e) => [e.address, e.kind]),
    [['/fx.in', 'library']],
  );
  // The pins crossed within a module are not wires, and no loop.
  assert.equal(t.cycles, 0);
  assert.equal(t.hops.get('/voice/osc1.freq') + 1, t.hops.get('/voice/osc1.out'));
});

test('through modules: a parameter crosses too; upstream reaches the sources', () => {
  const p = trace(GRAPH, { kind: 'pin', address: '/voice.cutoff' }, 'down', through);
  assert.ok(has(p, '/voice/filt.cutoff', '/voice/filt.out', '/voice/vca.out', '/mix.out'));
  const up = trace(GRAPH, { kind: 'pin', address: '/mix.a' }, 'up', through);
  const ends = new Map(up.up.endpoints.map((e) => [e.address, e.kind]));
  assert.ok(ends.size > 0);
  assert.ok([...ends.values()].every((k) => k === 'source'), JSON.stringify([...ends]));
  assert.ok([...ends.keys()].some((a) => a.startsWith('/seq.')), JSON.stringify([...ends]));
});

test('through modules: a loop of modules ends, each pin once', () => {
  const t = trace(GRAPH, { kind: 'pin', address: '/lfo.freq' }, 'down', through);
  assert.ok(has(t, '/lfo.out', '/fb.in', '/fb.out', '/voice/filt.out'));
  assert.ok(t.cycles >= 1);
});

test('through modules: the tree is the chain in order; a short trace stays open', () => {
  const t = trace(GRAPH, { kind: 'pin', address: '/seq.hz' }, 'down', through);
  const tree = traceTree(GRAPH, t);
  const top = tree.find((l) => l.path === '');
  assert.deepEqual(
    top.children.map((c) => c.name),
    ['seq', 'voice', 'mix', 'fx'],
  );
  const voice = tree.find((l) => l.path === '/voice');
  assert.deepEqual(
    voice.children.map((c) => c.name).slice(0, 3),
    ['pitch', 'osc1', 'osc2'],
  );
  assert.ok(
    tree.every((l) => !l.collapsed),
    'a short trace stays open',
  );
});

/** The studio's `graph` structure (crates/signal-project/tests/structure.rs keeps it equal to `Project::structure`). */
const STUDIO = JSON.parse(readFileSync(new URL('./fixtures/studio-graph.json', import.meta.url), 'utf8'));

test('the studio: through modules from the synth’s hz reaches the oscillators, the filter, the VCA and the audio output', () => {
  const t = trace(STUDIO, { kind: 'pin', address: '/synth.hz' }, 'down', through);
  assert.ok(has(t, '/synth/osc1.out', '/synth/osc2.out', '/synth/osc3.out', '/synth/ladder.lp', '/synth/vca.out', '/synth.out', '/echo.in', '/out.l', '/out.r'));
  const sinks = t.down.endpoints.filter((e) => e.kind === 'sink').map((e) => e.address);
  assert.ok(sinks.includes('/out.l') && sinks.includes('/out.r'), sinks.join(', '));
  // To the next module, it stops where the voice's export leads.
  const next = trace(STUDIO, { kind: 'pin', address: '/synth.hz' }, 'down');
  assert.deepEqual(next.down.endpoints.map((e) => e.address).sort(), ['/synth/glide.in', '/synth/hz_prev.in']);
  // A parameter of the voice crosses its filter to the output as well.
  assert.ok(has(trace(STUDIO, { kind: 'pin', address: '/synth.cutoff' }, 'down', through), '/synth/ladder.cutoff_hz', '/synth/ladder.lp', '/out.l'));
  // The chain in order; long, so every level but the start's is collapsed.
  const tree = traceTree(STUDIO, t);
  assert.ok(t.pins.size > COLLAPSE_PINS);
  assert.deepEqual(
    tree.filter((l) => !l.collapsed).map((l) => l.path),
    [''],
  );
  const order = tree.find((l) => l.path === '/synth').children.map((c) => c.name);
  assert.equal(order[0], 'glide');
  // (Key tracking reaches the ladder's cutoff before the mixer reaches its input: the chain is by fewest hops.)
  for (const [a, b] of [
    ['pitch', 'osc1'],
    ['osc1', 'mixer'],
    ['pitch', 'ladder'],
    ['mixer', 'trim'],
    ['ladder', 'trim'],
    ['trim', 'vca'],
    ['vca', 'volume'],
  ]) {
    assert.ok(order.indexOf(a) < order.indexOf(b), `${a} before ${b}: ${order.join(' ')}`);
  }
});

test('the studio: a deep trace from the synth’s hz takes under 10 ms', () => {
  const start = { kind: 'pin', address: '/synth.hz' };
  trace(STUDIO, start, 'both', through);
  let best = Infinity;
  for (let i = 0; i < 5; i++) {
    const t0 = performance.now();
    trace(STUDIO, start, 'both', through);
    best = Math.min(best, performance.now() - t0);
  }
  assert.ok(best < 10, `deep trace took ${best.toFixed(2)} ms`);
});

test('through modules upstream: a constant (parameters only) is a source', () => {
  const g = {
    top: {
      instances: [inst('k', 'std.const', { outs: ['out'], params: ['value'] }), inst('m', 'std.mul', { ins: ['a'], outs: ['out'], params: ['gain'] })],
      connections: [conn('/k.out', '/m.a')],
    },
    composites: {},
  };
  const t = trace(g, { kind: 'pin', address: '/m.out' }, 'up', through);
  const ends = new Map(t.up.endpoints.map((e) => [e.address, e.kind]));
  assert.equal(ends.get('/k.value'), 'source');
  // A parameter of a module with input ports, set by its value, is an end but not a source.
  assert.equal(ends.get('/m.gain'), 'primitive');
});

test('a collapsed group start (pins): the tree builds, its members are the start', () => {
  const t = trace(GRAPH, { kind: 'pins', addresses: ['/voice.hz'], label: '/voice group' }, 'both');
  const tree = traceTree(GRAPH, t);
  assert.ok(Array.isArray(tree));
  const roles = tree.flatMap((l) => l.children.flatMap((c) => c.children.filter((p) => p.address === '/voice.hz').map((p) => p.role)));
  assert.ok(roles.every((r) => r === 'start'));
});

test('through modules, an output leading nowhere is an open end, never a sink (the studio synth.hz)', () => {
  const studio = JSON.parse(readFileSync(new URL('./fixtures/studio-graph.json', import.meta.url), 'utf8'));
  const t = trace(studio, { kind: 'pin', address: '/synth.hz' }, 'both', { depth: 'through' });
  const kinds = new Map([...t.down.endpoints, ...t.up.endpoints].map((e) => [e.address, e.kind]));
  assert.equal(kinds.get('/synth/ladder.lp12'), 'unconnected');
  assert.equal(kinds.get('/echo/tone.hp'), 'unconnected');
  assert.equal(kinds.get('/out.l'), 'sink');
  // Upstream, an input with nothing connected is an open end too; a parameter left to its value is not.
  assert.equal(kinds.get('/melody.reset'), 'unconnected');
  assert.equal(kinds.get('/transport.bpm'), 'source');
  const roles = new Map(traceTree(studio, t).flatMap((l) => l.children.flatMap((i) => i.children.map((p) => [p.address, p.role]))));
  assert.equal(roles.get('/synth/ladder.lp12'), 'open');
  assert.equal(roles.get('/out.l'), 'endpoint');
  // Behind a composite's output, inside, its source is a module's output: a real end, not an open one.
  assert.deepEqual(
    behind(studio, '/synth.out', 'inside').endpoints.map((e) => `${e.address} ${e.kind}`),
    ['/synth/volume.out primitive'],
  );
});

test('a composite instance start goes through its inside, both ways, its ends staying outside (2026-10-20)', () => {
  const t = trace(GRAPH, { kind: 'instance', address: '/voice' }, 'both');
  // Inward from its inputs: the export wires and the inner pins they reach, nested composite included.
  assert.ok(tracedWire(t, '/voice', { from: 'hz', to: 'pitch.hz' }), 'hz → pitch.hz');
  assert.ok(tracedWire(t, '/voice/pitch', { from: 'hz', to: 'r1.in' }), 'into the nested composite');
  assert.ok(t.pins.has('/voice/pitch.hz') && t.pins.has('/voice/pitch/r1.in') && t.pins.has('/voice/env.gate') && t.pins.has('/voice/filt.cutoff'));
  // Inward from its output, upstream: vca.out → out.
  assert.ok(tracedWire(t, '/voice', { from: 'vca.out', to: 'out' }));
  assert.ok(t.pins.has('/voice/vca.out'));
  assert.ok(t.levels.includes('/voice') && t.levels.includes('/voice/pitch'));
  // The ends are still the outside ones.
  assert.deepEqual(ends(t.up), ['/lfo.out', '/seq.gate', '/seq.hz']);
  assert.deepEqual(ends(t.down), ['/mix.a']);
  // Through modules, the inside is crossed from the inputs to the output.
  const deep = trace(GRAPH, { kind: 'instance', address: '/voice' }, 'down', { depth: 'through' });
  for (const a of ['/voice/osc1.out', '/voice/sum.out', '/voice/filt.out', '/voice/vca.a']) assert.ok(deep.pins.has(a), a);
  assert.ok(tracedWire(deep, '/voice', { from: 'filt.out', to: 'vca.a' }));
  // Down only: the inputs go inward, the output is not walked inward upstream.
  const down = trace(GRAPH, { kind: 'instance', address: '/voice' }, 'down');
  assert.ok(tracedWire(down, '/voice', { from: 'hz', to: 'pitch.hz' }));
  assert.ok(!tracedWire(down, '/voice', { from: 'vca.out', to: 'out' }));
  // A library composite's inside is not in graph: nothing more.
  const fx = trace(GRAPH, { kind: 'instance', address: '/fx' }, 'both');
  assert.ok(![...fx.levels].some((p) => p.startsWith('/fx')));
});

// M2.13-3 (AC7, AC8): the synth open in place. Its box grown into a frame, the
// top level's wires to it are routed again to the frame's edge; a trace still
// names them (they keep what they connect) and goes on inside the frame.
test('a trace across an open synth: its re-routed wires are traced, and the trace goes on inside', () => {
  const factsOf = (m) => exportFacts(STUDIO.composites[m] ?? null);
  const top = layout(STUDIO.top, {}, { factsOf });
  const inner = layout(STUDIO.composites.moog_voice, {}, { factsOf });
  const opened = openedLayout(top, 'synth', inner.bounds);
  const frame = opened.nodes.find((n) => n.name === 'synth');
  const t = trace(STUDIO, { kind: 'pin', address: '/melody.hz' }, 'down');
  const crossing = opened.wires.filter((w) => w.crossing && tracedWire(t, '', w.written));
  assert.ok(crossing.length >= 1, 'the wire into the frame is traced');
  for (const w of crossing) {
    const stored = top.wires.find((s) => s.id === w.id);
    assert.equal(wireKey(w.written), wireKey(stored.written), 'the same key as the stored wire');
    assert.notDeepEqual(w.curve.segs.at(-1).to, stored.curve.segs.at(-1).to, 'routed again');
    const pin = frame.ins.find((p) => p.name === w.to.port);
    assert.deepEqual(w.curve.segs.at(-1).to, { x: pin.x, y: pin.y }, `${w.label} ends at the frame's pin`);
  }
  assert.ok(t.wires.get('/synth')?.size > 0, 'and the trace continues inside the frame');
});
