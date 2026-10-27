// The groups of module types (ADR 0027 §6, docs/ui-controls.md "Editing the
// patch"): a type's group, the groups' order, and what a type is for the
// sidebar. The Add module browser's tree, search and keys are
// ./module-browser.test.js.

import test from 'node:test';
import assert from 'node:assert/strict';
import { paletteEntries } from './schematic-patch.js';
import { GROUPS, OTHER, REFERENCE, groupOf, groupOrder, moduleInfo, projectKey, words } from './schematic-palette.js';

const MODULES = {
  primitives: [
    { module: 'std.add', composite: false, category: 'core', description: 'Adds its two signal inputs: out = a + b.' },
    { module: 'std.adsr', composite: false, category: 'envelope', description: 'Gated ADSR envelope.' },
    { module: 'std.audio_out', composite: false, category: 'io', description: 'Stereo audio sink.' },
    { module: 'std.mul', composite: false, category: 'core', description: 'Multiplies its two signal inputs.' },
    { module: 'std.sine_osc', composite: false, category: 'generator', description: 'Sine oscillator.' },
    { module: 'std.svf', composite: false, category: 'filter', description: 'State-variable filter.' },
    { module: 'lab.thing', composite: false, description: 'No category.' },
    { module: 'std.future', composite: false, category: 'spectral', description: 'A category not known here.' },
  ],
  composites: [
    { module: 'drums', composite: true, description: null },
    { module: 'kick', composite: true, description: null },
  ],
};
const ENTRIES = paletteEntries(MODULES);

test('a type’s group: project for a composite, library for lib.*, else its category, else other', () => {
  assert.equal(groupOf({ module: 'kick', composite: true }), 'project');
  assert.equal(groupOf({ module: 'kick', composite: true, category: 'delay' }), 'project', 'a project composite’s category does not move it');
  assert.equal(groupOf({ module: 'lib.kick808', composite: true }), 'library', 'reserved for the built-in library (ADR 0027 §4)');
  assert.equal(groupOf({ module: 'x', composite: true, library: true }), 'library');
  assert.equal(groupOf({ module: 'std.svf', category: 'filter' }), 'filter');
  assert.equal(groupOf({ module: 'lab.thing' }), OTHER);
  assert.deepEqual(ENTRIES.map((e) => e.group), ['core', 'envelope', 'io', 'core', 'generator', 'filter', OTHER, 'spectral', 'project', 'project']);
  assert.equal(ENTRIES[0].description, 'Adds its two signal inputs: out = a + b.');
});

test('the groups’ fixed order: project, library, the std categories; an unknown one after them, other last', () => {
  assert.deepEqual(GROUPS, ['project', 'library', 'core', 'generator', 'filter', 'envelope', 'delay', 'physical', 'event', 'visual', 'io']);
  assert.deepEqual(['io', OTHER, 'zeta', 'core', 'spectral', 'project', 'library'].sort(groupOrder), ['project', 'library', 'core', 'io', 'spectral', 'zeta', OTHER]);
  assert.deepEqual(words('  Core  A '), ['core', 'a']);
  assert.deepEqual(words(null), []);
});

test('what a type is, for the sidebar: its description (a primitive’s formula) and group', () => {
  const info = moduleInfo(MODULES);
  assert.deepEqual(info.get('std.add'), { module: 'std.add', description: 'Adds its two signal inputs: out = a + b.', group: 'core', composite: false, referenceFor: null });
  assert.deepEqual(info.get('kick'), { module: 'kick', description: null, group: 'project', composite: true, referenceFor: null });
  assert.equal(moduleInfo(null).size, 0);
  assert.equal(projectKey({ projectId: 'p1', folder: '/a', project: 'kick' }), 'p1');
  assert.equal(projectKey({ folder: '/a', project: 'kick' }), '/a\nkick');
  assert.equal(projectKey(null), null);
});

test('validation references (reference_for) form their own group, last', () => {
  const modules = {
    primitives: MODULES.primitives,
    composites: [
      ...MODULES.composites,
      { module: 'lib.tape_echo', composite: true, library: true, category: 'delay', description: 'A tape echo.' },
      { module: 'lib.svf_core', composite: true, library: true, category: 'filter', description: 'std.svf rebuilt.', reference_for: 'std.svf' },
    ],
  };
  assert.equal(REFERENCE, 'reference');
  assert.equal(groupOf({ module: 'lib.svf_core', library: true, reference_for: 'std.svf' }), REFERENCE);
  assert.equal(groupOf({ module: 'lib.svf_core', library: true, reference_for: '' }), 'library', 'an empty name is no reference');
  assert.deepEqual([REFERENCE, OTHER, 'library', 'zeta', 'project'].sort(groupOrder), ['project', 'library', 'zeta', OTHER, REFERENCE]);
  const info = moduleInfo(modules);
  assert.equal(info.get('lib.svf_core').referenceFor, 'std.svf');
  assert.equal(info.get('lib.svf_core').group, REFERENCE);
  assert.equal(info.get('lib.tape_echo').referenceFor, null);
});
