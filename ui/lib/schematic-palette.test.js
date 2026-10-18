// The add palette grouped (ADR 0027 §6, docs/ui-controls.md "Editing the
// patch"): the groups' order, the search across them, the window of rows,
// the cursor and its keys, and what a type is for the sidebar.

import test from 'node:test';
import assert from 'node:assert/strict';
import { paletteEntries } from './schematic-patch.js';
import {
  EXPANDED,
  GROUPS,
  OTHER,
  REFERENCE,
  cursorRows,
  groupOf,
  groupOrder,
  moduleInfo,
  moveCursor,
  paletteKey,
  paletteRows,
  placeCursor,
  projectKey,
  rowWindow,
  windowRows,
  toggled,
} from './schematic-palette.js';

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
const keys = (rows) => rows.map((r) => r.key);
const press = (key, extra = {}) => ({ key, metaKey: false, ctrlKey: false, ...extra });

test('a type’s group: project for a composite, library for lib.*, else its category, else other', () => {
  assert.equal(groupOf({ module: 'kick', composite: true }), 'project');
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
});

test('without a search: every group, counted, its entries shown when expanded', () => {
  const view = paletteRows(ENTRIES, '', new Set(EXPANDED));
  assert.equal(view.searching, false);
  assert.equal(view.matched, ENTRIES.length);
  assert.deepEqual(keys(view.rows), ['g:project', 'm:drums', 'm:kick', 'g:core', 'g:generator', 'g:filter', 'g:envelope', 'g:io', 'g:spectral', `g:${OTHER}`]);
  assert.deepEqual(view.rows.filter((r) => r.kind === 'group').map((r) => [r.group, r.count, r.expanded]), [
    ['project', 2, true],
    ['core', 2, false],
    ['generator', 1, false],
    ['filter', 1, false],
    ['envelope', 1, false],
    ['io', 1, false],
    ['spectral', 1, false],
    [OTHER, 1, false],
  ]);
  const core = paletteRows(ENTRIES, '', new Set(['core']));
  assert.deepEqual(keys(core.rows).slice(0, 4), ['g:project', 'g:core', 'm:std.add', 'm:std.mul'], 'entries keep their order in a group');
  assert.deepEqual(keys(paletteRows([], '', new Set()).rows), [], 'nothing loaded: no rows');
});

test('a search: only the groups with a match, all expanded, counting the matches', () => {
  const view = paletteRows(ENTRIES, 'out', new Set());
  assert.equal(view.searching, true);
  assert.deepEqual(keys(view.rows), ['g:core', 'm:std.add', 'g:io', 'm:std.audio_out'], 'name or description, collapsed groups opened');
  assert.deepEqual(view.rows.filter((r) => r.kind === 'group').map((r) => [r.group, r.count, r.expanded]), [['core', 1, true], ['io', 1, true]]);
  assert.equal(view.matched, 2);
  assert.deepEqual(keys(paletteRows(ENTRIES, 'FILTER', new Set()).rows), ['g:filter', 'm:std.svf'], 'any case; the group’s name matches too');
  assert.deepEqual(keys(paletteRows(ENTRIES, 'core a', new Set()).rows), ['g:core', 'm:std.add', 'm:std.mul'], 'every word');
  assert.deepEqual(keys(paletteRows(ENTRIES, 'project', new Set()).rows), ['g:project', 'm:drums', 'm:kick']);
  assert.deepEqual(keys(paletteRows(ENTRIES, 'composite', new Set()).rows), ['g:project', 'm:drums', 'm:kick'], 'a composite’s label');
  const none = paletteRows(ENTRIES, 'nothing like it', new Set());
  assert.deepEqual([none.rows, none.matched], [[], 0]);
});

test('the cursor: the first entry shown, else the first heading; kept while it shows', () => {
  const open = paletteRows(ENTRIES, '', new Set(EXPANDED));
  assert.equal(placeCursor(open, null), 'm:drums');
  assert.equal(placeCursor(open, 'g:core'), 'g:core');
  assert.equal(placeCursor(open, 'm:std.add'), 'm:drums', 'in a closed group: not shown');
  const closed = paletteRows(ENTRIES, '', new Set());
  assert.equal(placeCursor(closed, null), 'g:project');
  const search = paletteRows(ENTRIES, 'out', new Set());
  assert.equal(placeCursor(search, 'g:core'), 'm:std.add', 'a search’s headings take no cursor');
  assert.deepEqual(keys(cursorRows(search)), ['m:std.add', 'm:std.audio_out']);
  assert.equal(placeCursor(paletteRows(ENTRIES, 'nothing like it', new Set()), 'm:std.add'), null);
});

test('the arrows move across the groups, held at the ends; Home and End with a modifier', () => {
  const view = paletteRows(ENTRIES, '', new Set(['project', 'core']));
  assert.deepEqual(keys(view.rows).slice(0, 6), ['g:project', 'm:drums', 'm:kick', 'g:core', 'm:std.add', 'm:std.mul']);
  assert.equal(moveCursor(view, 'm:kick', 1), 'g:core', 'down out of a group onto the next heading');
  assert.equal(moveCursor(view, 'g:core', 1), 'm:std.add', 'and into it');
  assert.equal(moveCursor(view, 'm:std.add', -2), 'm:kick', 'up across the heading');
  assert.equal(moveCursor(view, 'g:project', -1), 'g:project', 'held at the top');
  assert.equal(moveCursor(view, `g:${OTHER}`, 1), `g:${OTHER}`, 'held at the end');
  assert.equal(moveCursor(view, 'm:drums', Infinity), `g:${OTHER}`);
  assert.equal(moveCursor(view, `g:${OTHER}`, -Infinity), 'g:project');
  assert.equal(moveCursor(view, 'gone', 1), 'm:drums', 'a cursor not shown: placed again');
  const search = paletteRows(ENTRIES, 'out', new Set());
  assert.equal(moveCursor(search, 'm:std.add', 1), 'm:std.audio_out', 'a search: from entry to entry, over the heading');
  assert.equal(moveCursor(paletteRows([], '', new Set()), null, 1), null);
});

test('the keys: arrows move, Enter adds an entry or opens and closes a heading, → and ← open and close', () => {
  const view = paletteRows(ENTRIES, '', new Set(['project']));
  assert.deepEqual(paletteKey(press('ArrowDown'), view, 'm:drums'), { move: 1 });
  assert.deepEqual(paletteKey(press('ArrowUp'), view, 'm:drums'), { move: -1 });
  assert.deepEqual(paletteKey(press('PageDown'), view, 'm:drums'), { move: 8 });
  assert.equal(paletteKey(press('Enter'), view, 'm:drums'), 'add');
  assert.deepEqual(paletteKey(press('Enter'), view, 'g:core'), { toggle: 'core' });
  assert.deepEqual(paletteKey(press('ArrowRight'), view, 'g:core'), { toggle: 'core' });
  assert.equal(paletteKey(press('ArrowLeft'), view, 'g:core'), null, 'closed already: the field’s own caret key');
  assert.deepEqual(paletteKey(press('ArrowLeft'), view, 'g:project'), { toggle: 'project' });
  assert.equal(paletteKey(press('ArrowRight'), view, 'm:drums'), null, 'on an entry: the field’s');
  assert.equal(paletteKey(press('Home'), view, 'm:drums'), null, 'plain Home and End are the field’s');
  assert.deepEqual(paletteKey(press('End', { metaKey: true }), view, 'm:drums'), { move: Infinity });
  assert.equal(paletteKey(press('a'), view, 'm:drums'), null);
  assert.equal(paletteKey(press('Enter'), view, null), 'add', 'nothing chosen: add does nothing');
});

test('the expanded groups: toggled into a new set', () => {
  const start = new Set(EXPANDED);
  const next = toggled(start, 'core');
  assert.deepEqual([...next].sort(), ['core', 'project']);
  assert.deepEqual([...start], ['project'], 'the old set untouched');
  assert.deepEqual([...toggled(next, 'project')], ['core']);
});

test('the window of rows the list shows follows the cursor and never runs past the end', () => {
  assert.equal(rowWindow(20, 0, 0, 12), 0);
  assert.equal(rowWindow(20, 12, 0, 12), 1, 'one past the bottom: moved by one');
  assert.equal(rowWindow(20, 19, 0, 12), 8);
  assert.equal(rowWindow(20, 5, 8, 12), 5, 'above the window: it starts there');
  assert.equal(rowWindow(20, 10, 3, 12), 3, 'in sight: unmoved');
  assert.equal(rowWindow(20, -1, 15, 12), 8, 'no cursor (the wheel): held at the end');
  assert.equal(rowWindow(5, 4, 3, 12), 0, 'fewer rows than the window');
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

test('a window starting inside a group shows its heading first, in place of one row', () => {
  const view = paletteRows(ENTRIES, '', new Set(['project', 'core']));
  const all = keys(view.rows); // project, drums, kick, core, add, mul, generator, filter, envelope, io, spectral, other
  assert.equal(all.length, 12);
  const atTop = windowRows(view.rows, 0, 0, 5);
  assert.deepEqual([keys(atTop.shown), atTop.above, atTop.below], [['g:project', 'm:drums', 'm:kick', 'g:core', 'm:std.add'], 0, 7]);
  const inside = windowRows(view.rows, all.indexOf('m:std.mul'), 0, 4);
  assert.deepEqual([keys(inside.shown), inside.above, inside.below], [['g:core', 'm:std.add', 'm:std.mul', 'g:generator'], 3, 5], 'moved to the cursor; starting on a heading, all rows its own');
  const mid = windowRows(view.rows, all.indexOf('m:std.mul'), 5, 4);
  assert.deepEqual(keys(mid.shown), ['g:core', 'm:std.mul', 'g:generator', 'g:filter'], 'the heading named although scrolled away');
  assert.deepEqual([mid.above, mid.below], [4, 4]);
  assert.equal(windowRows([], -1, 0, 4).shown.length, 0);
});

test('validation references (reference_for) form their own group, last and collapsed', () => {
  const modules = {
    primitives: MODULES.primitives,
    composites: [
      ...MODULES.composites,
      { module: 'lib.tape_echo', composite: true, library: true, category: 'delay', description: 'A tape echo.' },
      { module: 'lib.svf_core', composite: true, library: true, category: 'filter', description: 'std.svf rebuilt.', reference_for: 'std.svf' },
      { module: 'lib.adsr_core', composite: true, library: true, category: 'envelope', description: 'std.adsr rebuilt.', reference_for: 'std.adsr' },
    ],
  };
  assert.equal(REFERENCE, 'reference');
  assert.equal(groupOf({ module: 'lib.svf_core', library: true, reference_for: 'std.svf' }), REFERENCE);
  assert.equal(groupOf({ module: 'lib.svf_core', library: true, reference_for: '' }), 'library', 'an empty name is no reference');
  assert.deepEqual([REFERENCE, OTHER, 'library', 'zeta', 'project'].sort(groupOrder), ['project', 'library', 'zeta', OTHER, REFERENCE]);
  assert.ok(!EXPANDED.includes(REFERENCE), 'collapsed until the viewer opens it');
  const entries = paletteEntries(modules);
  const view = paletteRows(entries, '', new Set(EXPANDED));
  const headings = view.rows.filter((r) => r.kind === 'group');
  assert.deepEqual(headings.at(-1), { kind: 'group', key: 'g:reference', group: REFERENCE, count: 2, expanded: false });
  assert.equal(headings.find((r) => r.group === 'library').count, 1, 'not hidden: only moved out of the library');
  assert.ok(!view.rows.some((r) => r.key === 'm:lib.svf_core'), 'collapsed: its entries do not show');
  const searched = paletteRows(entries, 'svf', new Set());
  assert.deepEqual(keys(searched.rows), ['g:filter', 'm:std.svf', 'g:reference', 'm:lib.svf_core'], 'a search finds them, last');
  const info = moduleInfo(modules);
  assert.equal(info.get('lib.svf_core').referenceFor, 'std.svf');
  assert.equal(info.get('lib.svf_core').group, REFERENCE);
  assert.equal(info.get('lib.tape_echo').referenceFor, null);
});
