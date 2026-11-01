// ADR 0051 §4–5, tests 3 and 5 (phase 1's part): the studio's cards as
// instruments (by controls, then by flow), its device slot a role endpoint
// and never an instrument's, and the primary Stack's edge pins exactly
// where connections cross it; deterministic.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { parseContent } from './panel-tree.js';
import { panelCards, parsePlane } from './plane-file.js';
import { assignInstruments, edgePins, instanceOf, instrumentTitle, portOf, roleEndpoints, stackMembers } from './plane-instruments.js';

const root = new URL('../../', import.meta.url);
const read = (path) => readFileSync(new URL(path, root), 'utf8');
const whitelist = JSON.parse(read('ui/lib/panel-whitelist.json'));
const STUDIO = JSON.parse(read('ui/lib/fixtures/studio-graph.json'));

function studioCards() {
  const text = read('projects/studio/ui/main.html');
  const start = text.indexOf('>', text.indexOf('<signal-panel')) + 1;
  return panelCards(parseContent(text.slice(start, text.indexOf('</signal-panel>')), whitelist, { mode: 'read' }).tree);
}

const groups = (owner) => {
  const out = {};
  for (const [name, key] of owner) (out[key ?? '-'] ??= []).push(name);
  return out;
};

test('addresses: the top-level instance and the port', () => {
  assert.equal(instanceOf('/synth.cutoff'), 'synth');
  assert.equal(instanceOf('/rack/fx.out'), 'rack');
  assert.equal(portOf('/limiter.out'), 'out');
});

test('the studio’s instruments: each card takes what its controls bind, the rest follows the flow; its audio_out is a role endpoint', () => {
  const owner = assignInstruments({ instances: STUDIO.top.instances, connections: STUDIO.top.connections, cards: studioCards() });
  assert.deepEqual(groups(owner), {
    'card:transport': ['transport'],
    'card:mono-synth': ['melody', 'synth'],
    'card:rhythm-composer': ['drums', 'kick', 'snare', 'clap', 'hats', 'tom', 'rim', 'cymbal', 'drum_a', 'drum_b'],
    'card:tape-echo': ['echo'],
    'card:master': ['bus', 'dc', 'limiter'],
    '-': ['out'],
  });
  assert.deepEqual(roleEndpoints(STUDIO.top.instances), [{ name: 'out', role: 'main', layout: 'stereo', ports: ['/out.l', '/out.r'] }]);
});

test('the primary Stack’s boundary: one edge pin, master’s out, joined to the role endpoint’s two channels', () => {
  const instances = STUDIO.top.instances;
  const owner = assignInstruments({ instances, connections: STUDIO.top.connections, cards: studioCards() });
  const plane = parsePlane(read('projects/studio/ui/plane.toml')).plane;
  const { stackOf, unstacked } = stackMembers(plane, owner, instances);
  assert.deepEqual(unstacked, []);
  assert.equal(stackOf.get('out'), null);
  assert.deepEqual(edgePins(plane.stacks[0], owner, stackOf, STUDIO.top.connections), [{ side: 'out', instrument: 'card:master', address: '/limiter.out', label: 'out', outside: ['/out.l', '/out.r'] }]);
});

test('edge pins appear exactly where connections cross: inputs first, grouped per instrument in Stack order, names told apart', () => {
  const instances = ['a', 'b', 'c', 'd', 'e'].map((name) => ({ name, module: 'm' }));
  const owner = new Map([
    ['a', 'card:one'],
    ['b', 'card:one'],
    ['c', 'card:two'],
    ['d', null],
    ['e', 'card:three'],
  ]);
  const plane = { stacks: [{ name: 's', instruments: ['card:two', 'card:one'] }, { name: 't', instruments: ['card:three'] }] };
  const { stackOf, unstacked } = stackMembers(plane, owner, instances);
  assert.deepEqual(unstacked, ['d']);
  const connections = [
    { from: '/a.out', to: '/b.in' }, // inside
    { from: '/a.out', to: '/e.in' }, // to another Stack
    { from: '/b.out', to: '/d.in' }, // to an unstacked instance
    { from: '/d.out', to: '/c.in' }, // from it
    { from: '/a.out', to: '/c.x' }, // inside
    { from: '/e.out', to: '/a.in' }, // from another Stack
  ];
  const pins = edgePins(plane.stacks[0], owner, stackOf, connections);
  assert.deepEqual(
    pins.map((p) => `${p.side} ${p.instrument} ${p.label} ← ${p.outside.join(',')}`),
    ['in card:two in ← /d.out', 'in card:one in ← /e.out', 'out card:one a.out ← /e.in', 'out card:one b.out ← /d.in'],
  );
  // The other Stack's side of the same connections: they meet.
  assert.deepEqual(
    edgePins(plane.stacks[1], owner, stackOf, connections).map((p) => `${p.side} ${p.address} ${p.outside.join(',')}`),
    ['in /e.in /a.out', 'out /e.out /a.in'],
  );
});

test('a slot with a part is its own instrument; ties of controls go to the earlier card; the result never depends on map order', () => {
  const instances = [
    { name: 'deck', module: 'deck', slot: { part: { file: 'ui/parts/deck.html' } } },
    { name: 'osc', module: 'std.sine' },
    { name: 'amp', module: 'std.gain' },
  ];
  const cards = [
    { id: 'first', binds: ['/osc.hz', '/deck.level'] },
    { id: 'second', binds: ['/osc.phase'] },
  ];
  const connections = [{ from: '/osc.out', to: '/amp.in' }];
  const a = assignInstruments({ instances, connections, cards });
  assert.deepEqual([...a], [
    ['deck', 'deck'],
    ['osc', 'card:first'],
    ['amp', 'card:first'],
  ]);
  assert.deepEqual([...assignInstruments({ instances, connections, cards })], [...a]);
  assert.equal(instrumentTitle('card:first', [{ id: 'first', heading: 'FIRST' }]), 'FIRST');
  assert.equal(instrumentTitle('deck'), 'deck');
});
