// A signal's whole way (ui/lib/plane-net.js): the net of a port across the
// levels, through a composite's exports both ways, never through an instance.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { netOf, selectionNet, selectionSeeds } from './plane-net.js';
import { wireKey } from './plane-engine-edit.js';

const STUDIO = JSON.parse(readFileSync(new URL('./fixtures/studio-graph.json', import.meta.url), 'utf8'));

test('a parameter’s net: from the chip’s pin to its target inside, and the automation that drives the target', () => {
  const net = netOf(STUDIO, ['/synth.cutoff']);
  assert.ok(net.addresses.has('/synth/ladder.cutoff_hz'), 'its target inside the chip');
  assert.ok(net.wires.has(wireKey('/synth.cutoff', '/synth/ladder.cutoff_hz')), 'the export’s wire as the Engine keys it');
  assert.ok(net.addresses.has('/synth/env_amt.out') && net.addresses.has('/synth/key_amt.out'), 'what else drives it');
  assert.ok(!net.addresses.has('/synth/ladder.out') && !net.addresses.has('/synth/env_amt.in'), 'never through an instance');
});

test('a cable’s net goes on into the chip: /melody.gate → /synth.gate → its targets inside', () => {
  const net = netOf(STUDIO, ['/melody.gate', '/synth.gate']);
  const targets = STUDIO.composites.moog_voice.inputs.find((p) => p.name === 'gate').to;
  for (const t of targets) assert.ok(net.addresses.has(`/synth/${t}`), t);
  assert.ok(net.wires.has(wireKey('/melody.gate', '/synth.gate')));
  // And from the inside out: a target inside reaches the cable.
  assert.ok(netOf(STUDIO, [`/synth/${targets[0]}`]).addresses.has('/melody.gate'));
});

test('a selection’s seeds: a wire’s two ends, a pin’s or a leg’s address, none for a card', () => {
  assert.deepEqual(selectionSeeds({ kind: 'cable', from: '/a.x', to: '/b.y' }), ['/a.x', '/b.y']);
  assert.deepEqual(selectionSeeds({ kind: 'leg', address: '/synth.cutoff' }), ['/synth.cutoff']);
  assert.deepEqual(selectionSeeds({ kind: 'engine', item: { kind: 'e-bind', address: '/synth/ladder.cutoff_hz' } }), ['/synth/ladder.cutoff_hz']);
  assert.deepEqual(selectionSeeds({ kind: 'engine', item: { kind: 'e-card', address: '/synth' } }), []);
  assert.equal(selectionNet(STUDIO, { kind: 'instrument', entry: 'card:master' }), null);
  assert.equal(selectionNet(null, { kind: 'leg', address: '/synth.cutoff' }), null);
});
