// The Deck's sidebar (ui/lib/plane-sidebar.js): what is selected and its
// net, where the signal comes from, where it goes and the chips' pins it
// passes through.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { planeSidebar } from './plane-sidebar.js';
import { selectionNet } from './plane-net.js';

const STUDIO = JSON.parse(readFileSync(new URL('./fixtures/studio-graph.json', import.meta.url), 'utf8'));

test('nothing selected: the project in numbers', () => {
  const side = planeSidebar(null, { graph: STUDIO });
  assert.equal(side.title, 'Nothing selected');
  assert.deepEqual(side.rows.find((r) => r.label === 'Instances').value, String(STUDIO.top.instances.length));
  assert.deepEqual(side.sections, []);
});

test('a cable: its ends, and its net by part: the source, the targets inside the chip, the chip’s pin it passes', () => {
  const selection = { kind: 'cable', from: '/melody.gate', to: '/synth.gate' };
  const side = planeSidebar(selection, { graph: STUDIO, net: selectionNet(STUDIO, selection) });
  assert.equal(side.title, 'Cable');
  assert.equal(side.subtitle, '/melody.gate → /synth.gate');
  const section = (title) => side.sections.find((s) => s.title === title)?.items.map((i) => i.address);
  assert.deepEqual(section('Comes from'), ['/melody.gate']);
  assert.deepEqual(section('Through the chips’ pins'), ['/synth.gate']);
  const targets = STUDIO.composites.moog_voice.inputs.find((p) => p.name === 'gate').to.map((t) => `/synth/${t}`).sort();
  assert.deepEqual(section('Goes to'), targets);
  assert.equal(side.copy.length, 2 + targets.length);
});

test('a control’s leg: the parameter, what else drives its target, the target inside', () => {
  const selection = { kind: 'leg', address: '/synth.cutoff' };
  const side = planeSidebar(selection, { graph: STUDIO, net: selectionNet(STUDIO, selection) });
  assert.equal(side.title, '/synth.cutoff');
  assert.ok(side.sections.find((s) => s.title === 'Goes to').items.some((i) => i.address === '/synth/ladder.cutoff_hz'));
  assert.ok(side.rows.some((r) => r.label === 'Net'));
});
