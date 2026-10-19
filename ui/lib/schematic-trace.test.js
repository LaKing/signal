// The schematic's path tracing (ADR 0030 §1, docs/ui-controls.md
// "Schematic"): across composite boundaries both ways and nested, fan-out,
// fan-in, cycles, converters, library composites as opaque ends, both
// directions, every kind of start; the badges, the tree, and the time.

import test from 'node:test';
import assert from 'node:assert/strict';
import { BADGE_NAMES, badgeText, behind, pinInfo, startOf, trace, traceTree, tracedWire, wireKey, within } from './schematic-trace.js';

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
