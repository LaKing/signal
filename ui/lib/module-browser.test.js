// The Add module browser (docs/ui-controls.md "Editing the patch"): the tree
// by source and category, the search that filters it, the rows and the
// cursor's keys, and the detail pane's texts (ranges, value sets, defaults).

import test from 'node:test';
import assert from 'node:assert/strict';
import {
  EXPANDED,
  SOURCES,
  browserEntries,
  browserKey,
  browserTree,
  categoryOf,
  cursorRows,
  defaultText,
  detail,
  kindText,
  moveCursor,
  numberText,
  parameterRow,
  pathTo,
  placeCursor,
  portRow,
  rangeText,
  rateText,
  revealed,
  shapeText,
  sourceLabel,
  sourceOf,
  sourceOrder,
  summary,
  toggled,
  treeRows,
  valueSetText,
  whereText,
} from './module-browser.js';

const WAVES = [
  { value: 0, label: 'sine' },
  { value: 1, label: 'saw' },
  { value: 2, label: 'square' },
];

const MODULES = {
  primitives: [
    { module: 'std.add', composite: false, category: 'core', description: 'Adds its two signal inputs: out = a + b.' },
    { module: 'std.mul', composite: false, category: 'core', description: 'Multiplies its two signal inputs.' },
    {
      module: 'std.osc',
      composite: false,
      category: 'generator',
      description: 'An oscillator of four shapes. Its phase restarts on reset.',
      ports: [
        { name: 'reset', direction: 'in', shape: 'events', rate: 1, kind: 'trigger', unit: null, range: null },
        { name: 'out', direction: 'out', shape: 'signal', rate: 1, kind: 'audio', unit: null, range: { min: -1, max: 1 } },
      ],
      parameters: [
        { name: 'freq', kind: 'frequency', rate: 1, unit: 'Hz', range: { min: 0, max: 24000 }, default: 440, combine: 'add' },
        { name: 'wave', kind: 'control', rate: 1, unit: null, range: { min: 0, max: 2 }, default: 1, combine: 'replace', values: WAVES },
      ],
    },
    { module: 'std.svf', composite: false, category: 'filter', description: 'State-variable filter.' },
    { module: 'std.future', composite: false, category: 'spectral', description: 'A category not known here.' },
    { module: 'std.plain', composite: false, description: 'No category.' },
    { module: 'lab.thing', composite: false, description: 'An experiment.' },
    { module: 'zz.other', composite: false, description: 'Another namespace.' },
  ],
  composites: [
    { module: 'drums', composite: true, description: null, inside: { instances: [], connections: [] } },
    { module: 'kick', composite: true, description: 'A techno kick.' },
    { module: 'lib.tape_echo', composite: true, library: true, category: 'delay', description: 'A tape echo with feedback.' },
    { module: 'lib.svf_core', composite: true, library: true, category: 'filter', description: 'std.svf rebuilt from core primitives.', reference_for: 'std.svf' },
  ],
};
const ENTRIES = browserEntries(MODULES);
const keys = (rows) => rows.map((r) => r.key);
const press = (key, extra = {}) => ({ key, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...extra });

test('a type’s source: the project’s composites, the library’s, else its namespace', () => {
  assert.equal(sourceOf({ module: 'kick', composite: true }), 'project');
  assert.equal(sourceOf({ module: 'lib.tape_echo', composite: true }), 'library');
  assert.equal(sourceOf({ module: 'x', composite: true, library: true }), 'library');
  assert.equal(sourceOf({ module: 'std.svf' }), 'std');
  assert.equal(sourceOf({ module: 'lab.thing' }), 'lab');
  assert.equal(sourceOf({ module: 'plain' }), 'other');
  assert.deepEqual(SOURCES.map((s) => s.id), ['project', 'library', 'std', 'lab'], 'the sources named, in their order (data, not code)');
  assert.deepEqual(['other', 'zz', 'std', 'lab', 'aa', 'project', 'library'].sort(sourceOrder), ['project', 'library', 'std', 'lab', 'aa', 'zz', 'other']);
  assert.equal(sourceLabel('std'), 'Standard primitives');
  assert.equal(sourceLabel('zz'), 'zz.* primitives');
  assert.equal(sourceLabel('other'), 'Other');
  assert.equal(categoryOf({ category: 'filter', reference_for: 'std.svf' }), 'reference');
  assert.equal(categoryOf({}), null);
});

test('the entries keep what the detail pane and the previews need', () => {
  const osc = ENTRIES.find((e) => e.module === 'std.osc');
  assert.equal(osc.source, 'std');
  assert.equal(osc.category, 'generator');
  assert.equal(osc.ports.length, 2);
  assert.equal(osc.parameters[1].values.length, 3);
  assert.equal(osc.inside, null);
  const drums = ENTRIES.find((e) => e.module === 'drums');
  assert.deepEqual(drums.inside, { instances: [], connections: [] });
  assert.deepEqual([drums.ports, drums.parameters, drums.description], [[], [], null]);
  const reference = ENTRIES.find((e) => e.module === 'lib.svf_core');
  assert.deepEqual([reference.source, reference.category, reference.referenceFor, reference.library], ['library', 'reference', 'std.svf', true]);
});

test('the tree: sources in order, then categories in the palette’s order, the types in the reply’s', () => {
  const tree = browserTree(ENTRIES);
  assert.equal(tree.searching, false);
  assert.equal(tree.matched, ENTRIES.length);
  assert.deepEqual(tree.sources.map((s) => [s.key, s.label, s.count]), [
    ['s:project', 'This project', 2],
    ['s:library', 'Library', 2],
    ['s:std', 'Standard primitives', 6],
    ['s:lab', 'Lab primitives', 1],
    ['s:zz', 'zz.* primitives', 1],
  ]);
  // The project's composites name no category: they are its rows themselves.
  assert.deepEqual(keys(tree.sources[0].children), ['m:drums', 'm:kick']);
  // The library's: by category, the validation references last.
  assert.deepEqual(keys(tree.sources[1].children), ['c:library/delay', 'c:library/reference']);
  // std: GROUPS' order, an unknown category after them, a type without one under `other`.
  const std = tree.sources[2].children;
  assert.deepEqual(keys(std), ['c:std/core', 'c:std/generator', 'c:std/filter', 'c:std/spectral', 'c:std/other']);
  assert.deepEqual(keys(std[0].children), ['m:std.add', 'm:std.mul']);
  assert.deepEqual([std[0].count, std[0].label, std[0].source], [2, 'core', 'std']);
  assert.deepEqual(keys(tree.sources[3].children), ['m:lab.thing']);
  assert.deepEqual(browserTree([]).sources, [], 'nothing loaded: no tree');
});

test('a search filters the tree by name and description, any case, every word', () => {
  const byName = browserTree(ENTRIES, 'SVF');
  assert.equal(byName.searching, true);
  assert.deepEqual(byName.sources.map((s) => s.key), ['s:library', 's:std']);
  assert.deepEqual(pathTo(byName, 'm:lib.svf_core'), ['s:library', 'c:library/reference']);
  assert.equal(byName.matched, 2);
  const byDescription = browserTree(ENTRIES, 'feedback');
  assert.deepEqual(byDescription.sources.map((s) => keys(s.children)), [['c:library/delay']]);
  assert.deepEqual(keys(browserTree(ENTRIES, 'signal inputs multiplies').sources[0].children[0].children), ['m:std.mul'], 'every word, anywhere');
  assert.equal(browserTree(ENTRIES, 'nothing like it').matched, 0);
  assert.deepEqual(browserTree(ENTRIES, 'nothing like it').sources, []);
  assert.equal(browserTree(ENTRIES, 'generator').matched, 1, 'a category’s name matches its types');
});

test('the rows: a branch’s rows only while it is open; a search opens every branch', () => {
  const tree = browserTree(ENTRIES);
  const start = treeRows(tree, new Set(EXPANDED));
  assert.deepEqual(EXPANDED, ['s:project', 's:std'], 'the project and the standard primitives open at first, their categories closed');
  assert.deepEqual(keys(start.rows), ['s:project', 'm:drums', 'm:kick', 's:library', 's:std', 'c:std/core', 'c:std/generator', 'c:std/filter', 'c:std/spectral', 'c:std/other', 's:lab', 's:zz']);
  const kick = start.rows.find((r) => r.key === 'm:kick');
  assert.deepEqual([kick.depth, kick.parent], [1, 's:project']);
  const core = start.rows.find((r) => r.key === 'c:std/core');
  assert.deepEqual([core.depth, core.parent, core.expanded], [1, 's:std', false]);
  const opened = treeRows(tree, new Set([...EXPANDED, 'c:std/core']));
  const add = opened.rows.find((r) => r.key === 'm:std.add');
  assert.deepEqual([add.depth, add.parent], [2, 'c:std/core']);
  const searched = treeRows(browserTree(ENTRIES, 'svf'), new Set());
  assert.deepEqual(keys(searched.rows), ['s:library', 'c:library/reference', 'm:lib.svf_core', 's:std', 'c:std/filter', 'm:std.svf']);
  assert.ok(searched.rows.filter((r) => r.kind !== 'entry').every((r) => r.expanded));
  assert.deepEqual(keys(cursorRows(searched)), ['m:lib.svf_core', 'm:std.svf'], 'a search’s cursor moves over the types alone');
});

test('the cursor: the first type shown, kept while it shows; the arrows across the tree, held at the ends', () => {
  const view = treeRows(browserTree(ENTRIES), new Set(EXPANDED));
  assert.equal(placeCursor(view, null), 'm:drums');
  assert.equal(placeCursor(view, 'c:std/core'), 'c:std/core');
  assert.equal(placeCursor(view, 'm:std.add'), 'm:drums', 'in a closed branch: not shown');
  assert.equal(moveCursor(view, 'm:kick', 1), 's:library');
  assert.equal(moveCursor(view, 's:project', -1), 's:project', 'held at the top');
  assert.equal(moveCursor(view, 's:zz', 1), 's:zz', 'held at the end');
  assert.equal(moveCursor(view, 'm:drums', Infinity), 's:zz');
  assert.equal(moveCursor(view, 's:zz', -Infinity), 's:project');
  assert.equal(moveCursor(view, 'gone', 1), 'm:drums', 'a cursor not shown: placed again');
  assert.equal(moveCursor(treeRows(browserTree([]), new Set()), null, 1), null);
  const closed = treeRows(browserTree(ENTRIES), new Set());
  assert.equal(placeCursor(closed, null), 's:project', 'no type shown: the first row');
});

test('the keys: ↑↓ move, Enter adds a type or opens and closes a branch, → and ← open, close and step in and out', () => {
  const view = treeRows(browserTree(ENTRIES), new Set(EXPANDED));
  assert.deepEqual(browserKey(press('ArrowDown'), view, 'm:drums'), { move: 1 });
  assert.deepEqual(browserKey(press('ArrowUp'), view, 'm:drums'), { move: -1 });
  assert.deepEqual(browserKey(press('PageDown'), view, 'm:drums'), { move: 8 });
  assert.deepEqual(browserKey(press('End', { metaKey: true }), view, 'm:drums'), { move: Infinity });
  assert.equal(browserKey(press('Home'), view, 'm:drums'), null, 'plain Home and End are the field’s');
  assert.equal(browserKey(press('Enter'), view, 'm:drums'), 'add');
  assert.deepEqual(browserKey(press('Enter'), view, 'c:std/core'), { toggle: 'c:std/core' });
  assert.deepEqual(browserKey(press('ArrowRight'), view, 'c:std/core'), { toggle: 'c:std/core' }, 'closed: opens');
  assert.deepEqual(browserKey(press('ArrowRight'), view, 's:std'), { cursor: 'c:std/core' }, 'open: into its first row');
  assert.deepEqual(browserKey(press('ArrowLeft'), view, 's:std'), { toggle: 's:std' }, 'open: closes');
  assert.deepEqual(browserKey(press('ArrowLeft'), view, 'c:std/core'), { cursor: 's:std' }, 'closed: out to its branch');
  assert.equal(browserKey(press('ArrowLeft'), view, 's:library'), null, 'a closed source: nothing above it');
  assert.deepEqual(browserKey(press('ArrowLeft'), view, 'm:kick'), { cursor: 's:project' }, 'a type: out to its branch, the field empty');
  assert.equal(browserKey(press('ArrowLeft'), view, 'm:kick', { fieldEmpty: false }), null, 'a typed search: the field’s caret');
  assert.equal(browserKey(press('ArrowRight'), view, 'm:kick'), null, 'a type: the field’s');
  assert.equal(browserKey(press('a'), view, 'm:drums'), null);
  assert.equal(browserKey(press('Enter'), view, null), 'add', 'nothing chosen: add does nothing');
  const searched = treeRows(browserTree(ENTRIES, 'svf'), new Set());
  assert.equal(browserKey(press('ArrowLeft'), searched, 'm:std.svf'), null, 'searching: the arrows sideways are the field’s');
  assert.equal(browserKey(press('Enter'), searched, 'm:std.svf'), 'add');
});

test('the open branches: toggled into a new set; a search’s choice revealed when it is cleared', () => {
  const start = new Set(EXPANDED);
  const next = toggled(start, 'c:std/core');
  assert.deepEqual([...next].sort(), ['c:std/core', 's:project', 's:std']);
  assert.deepEqual([...start], EXPANDED, 'the old set untouched');
  assert.deepEqual([...toggled(next, 's:project')].sort(), ['c:std/core', 's:std']);
  const tree = browserTree(ENTRIES);
  const shown = revealed(new Set(), tree, 'm:lib.svf_core');
  assert.deepEqual([...shown].sort(), ['c:library/reference', 's:library']);
  assert.ok(treeRows(tree, shown).rows.some((r) => r.key === 'm:lib.svf_core'));
  assert.equal(pathTo(tree, 'm:nothing'), null);
  assert.deepEqual([...revealed(new Set(['s:std']), tree, 'm:nothing')], ['s:std']);
});

test('numbers, ranges, kinds, shapes and rates as the detail pane writes them', () => {
  assert.equal(numberText(440), '440');
  assert.equal(numberText(-1), '−1', 'a true minus');
  assert.equal(numberText(0.1 + 0.2), '0.3', 'six significant digits');
  assert.equal(numberText(24000), '24000');
  assert.equal(numberText(Infinity), '∞');
  assert.equal(numberText(-Infinity), '−∞');
  assert.equal(numberText(NaN), '—');
  assert.equal(rangeText({ min: 0, max: 24000 }), '[0, 24000]');
  assert.equal(rangeText({ min: -1, max: 1 }), '[−1, 1]');
  assert.equal(rangeText({ min: null, max: null }), '[−∞, ∞]', 'JSON writes an unbounded end as null');
  assert.equal(rangeText({ min: 0, max: null }), '[0, ∞]');
  assert.equal(rangeText(null), null);
  assert.equal(rangeText({}), null);
  assert.equal(kindText('audio'), 'Audio');
  assert.equal(kindText('frequency'), 'Frequency');
  assert.equal(kindText('note_number'), 'Note number');
  assert.equal(kindText(null), '—');
  assert.equal(shapeText('signal'), 'signal');
  assert.equal(shapeText('events'), 'events');
  assert.equal(shapeText({ vector: 256 }), 'vector ×256');
  assert.equal(rateText(1), 'full rate');
  assert.equal(rateText(800), '÷800');
  assert.equal(rateText(undefined), 'full rate');
});

test('a value set: each value with its name (ADR 0043), the default named by it', () => {
  assert.deepEqual(valueSetText(WAVES), ['0 → sine', '1 → saw', '2 → square']);
  assert.deepEqual(valueSetText(null), []);
  assert.equal(defaultText({ default: 1, values: WAVES }), '1 (saw)');
  assert.equal(defaultText({ default: 1.4, values: WAVES }), '1.4 (→ saw)', 'a default off the set: the entry it snaps to');
  assert.equal(defaultText({ default: 440 }), '440');
  assert.equal(defaultText({ default: -6 }), '−6');
  assert.equal(defaultText({}), '—');
  assert.deepEqual(parameterRow({ name: 'wave', kind: 'control', unit: null, range: { min: 0, max: 2 }, default: 1, combine: 'replace', values: WAVES }), {
    name: 'wave',
    kind: 'Control',
    range: '[0, 2]',
    values: ['0 → sine', '1 → saw', '2 → square'],
    default: '1 (saw)',
    unit: '—',
    combine: 'replace',
  });
  assert.deepEqual(parameterRow({ name: 'freq', kind: 'frequency', unit: 'Hz', range: { min: 0, max: 24000 }, default: 440, combine: 'add' }), {
    name: 'freq',
    kind: 'Frequency',
    range: '[0, 24000]',
    values: null,
    default: '440',
    unit: 'Hz',
    combine: 'add',
  });
  assert.equal(parameterRow({ name: 'x', values: [] }).values, null, 'an empty set is a continuous parameter');
});

test('the detail of a type: where it is from, its full description, its inputs, outputs and parameters', () => {
  const osc = detail(ENTRIES.find((e) => e.module === 'std.osc'));
  assert.equal(osc.module, 'std.osc');
  assert.equal(osc.where, 'Standard primitives · generator');
  assert.equal(osc.description, 'An oscillator of four shapes. Its phase restarts on reset.', 'the whole text, not its first sentence');
  assert.equal(osc.counts, '1 input, 1 output, 2 parameters');
  assert.deepEqual(osc.inputs, [{ name: 'reset', kind: 'Trigger', shape: 'events', rate: 'full rate', range: '—', unit: '—' }]);
  assert.deepEqual(osc.outputs, [{ name: 'out', kind: 'Audio', shape: 'signal', rate: 'full rate', range: '[−1, 1]', unit: '—' }]);
  assert.deepEqual(osc.parameters.map((p) => [p.name, p.range, p.values, p.default, p.unit]), [
    ['freq', '[0, 24000]', null, '440', 'Hz'],
    ['wave', '[0, 2]', ['0 → sine', '1 → saw', '2 → square'], '1 (saw)', '—'],
  ]);
  const reference = detail(ENTRIES.find((e) => e.module === 'lib.svf_core'));
  assert.deepEqual([reference.where, reference.referenceFor, reference.counts], ['Library · reference', 'std.svf', '0 inputs, 0 outputs, 0 parameters']);
  const drums = detail(ENTRIES.find((e) => e.module === 'drums'));
  assert.deepEqual([drums.where, drums.description], ['This project', null]);
  assert.equal(whereText({ source: 'lab', category: null }), 'Lab primitives');
  assert.equal(detail(null), null);
  assert.deepEqual(portRow({}), { name: '', kind: '—', shape: 'signal', rate: 'full rate', range: '—', unit: '—' });
});

test('a row’s short description: its first sentence', () => {
  assert.equal(summary('An oscillator of four shapes. Its phase restarts on reset.'), 'An oscillator of four shapes.');
  assert.equal(summary('Adds its two signal inputs: out = a + b.'), 'Adds its two signal inputs: out = a + b.');
  assert.equal(summary('std.svf rebuilt from core primitives.'), 'std.svf rebuilt from core primitives.', 'a dot inside a name does not end it');
  assert.equal(summary('No full stop'), 'No full stop');
  assert.equal(summary('Two\nlines. More.'), 'Two lines.');
  assert.equal(summary(null), '');
});
