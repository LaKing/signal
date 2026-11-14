// ADR 0035 §10: the host's panel shows its top-level includes' panels.
import test from 'node:test';
import assert from 'node:assert/strict';
import { includePanelFile, includePattern, includeTitle, panelIncludes } from './include.js';

test('an include’s panel: its file, its title card, and only top-level includes', () => {
  assert.equal(includePanelFile('drums'), 'include/drums/main.html');
  assert.equal(includeTitle('drums', { name: 'studio-transport' }), 'DRUMS — studio-transport (linked)');
  assert.equal(includeTitle('drums', null), 'DRUMS (linked)');
  const description = { content: { includes: [{ key: 'studio', name: 'studio' }, { key: 'deck_b', slot: true }, { key: 'rack.inner', name: 'x' }] } };
  assert.deepEqual(panelIncludes(description).map((i) => i.key), ['studio']);
  assert.deepEqual(panelIncludes({}), []);
});

test('the generated panel leaves an include’s and an own inside’s instances to their own panels', async () => {
  const { defaultPanel } = await import('./panel.js');
  const control = (instance, parameter) => ({ address: `/${instance}.${parameter}`, instance, parameter, module: 'm', min: 0, max: 1, base: 0, default: 0 });
  const spec = defaultPanel({ controls: [control('mix', 'gain_a'), control('studio/clap', 'decay'), control('studio', 'level')] });
  assert.deepEqual(spec.groups.map((g) => g.instance), ['mix', 'studio']);
});

test('an included panel’s pattern controls name the include’s pattern', () => {
  assert.equal(includePattern('studio', 'melody'), 'include/studio/melody');
  assert.equal(includePattern('studio', undefined), undefined);
});
