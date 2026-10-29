// ADR 0037 phase 3 (docs/ui-controls.md "Slots"): what a slot box and the
// slot card say, the interface and map editors' rows and the `slot.*` ops
// they write, how a content matches an interface, the Load dialog's
// projects, a template's text and a dropped load.

import test from 'node:test';
import assert from 'node:assert/strict';
import {
  addSlotOp,
  cardTitle,
  contentMatch,
  dropLoad,
  freeName,
  instancePorts,
  interfaceOp,
  interfaceRows,
  isProjectPath,
  loadOps,
  mapOp,
  mapRows,
  newRow,
  portJson,
  portNameError,
  projectEntries,
  slotCard,
  slotLabel,
  templateChoices,
  templateNameError,
  templateText,
  topSlots,
  unmatchedReason,
} from './slot.js';

/** A slot as `graph` gives it: `fx`, its own interface, std.gain loaded through a map. */
const FX = {
  interface: {
    ports: [
      { name: 'side', role: 'input', shape: 'signal', rate: 1, kind: 'audio' },
      { name: 'x', role: 'input', shape: 'signal', rate: 1, kind: 'audio' },
      { name: 'y', role: 'output', shape: 'signal', rate: 1, kind: 'audio' },
      { name: 'volume', role: 'parameter', shape: 'signal', rate: 1, kind: 'generic', unit: 'dB' },
    ],
  },
  from: null,
  load: 'std.gain',
  project: false,
  map: { x: 'in', y: 'out', volume: 'db' },
  matches: [
    { port: 'side', role: 'input', reason: '`std.gain` has no `side`' },
    { port: 'x', role: 'input', content: 'in' },
    { port: 'y', role: 'output', content: 'out' },
    { port: 'volume', role: 'parameter', content: 'db' },
  ],
  unmatched: [{ port: 'side', reason: '`std.gain` has no `side`' }],
  unheard: [],
  dropped: ['osc.out -> fx.side'],
};

const SPARE = { interface: { from: 'lib.voice', ports: [{ name: 'gate', role: 'input', shape: 'signal', rate: 1, kind: 'gate' }] }, from: 'lib.voice', load: null, project: false, map: null, matches: [], unmatched: [], unheard: [], dropped: [] };

/** `std.gain`'s `modules` entry. */
const GAIN = {
  module: 'std.gain',
  ports: [
    { name: 'in', direction: 'in', shape: 'signal', rate: 1, kind: 'audio' },
    { name: 'out', direction: 'out', shape: 'signal', rate: 1, kind: 'audio' },
  ],
  parameters: [{ name: 'db', kind: 'control', rate: 1 }],
};

test('a slot box says what it holds: empty (with its template), a module, a project with ↗', () => {
  assert.deepEqual(slotLabel(SPARE), { empty: true, subtitle: 'empty: lib.voice', template: 'lib.voice', project: false });
  assert.equal(slotLabel({ interface: { ports: [] }, load: null }).subtitle, 'empty');
  assert.equal(slotLabel(FX).subtitle, 'std.gain');
  assert.deepEqual(slotLabel({ from: 'lib.deck', load: '../studio', project: true }), { empty: false, subtitle: '../studio ↗', template: 'lib.deck', project: true });
  assert.ok(isProjectPath('./bass') && isProjectPath('x.signal') && !isProjectPath('lib.voice'));
});

test('an unmatched port has its reason (a hollow pin’s tooltip); an empty slot’s ports have none', () => {
  assert.equal(unmatchedReason(FX, 'side'), '`std.gain` has no `side`');
  assert.equal(unmatchedReason(FX, 'x'), null);
  assert.equal(unmatchedReason(SPARE, 'gate'), null);
});

test('Add slot: the templates to start from (none first), and its op', () => {
  const graph = { interfaces: [{ name: 'lib.deck', library: true, description: 'A deck' }, { name: 'bus', library: false, description: null }] };
  const choices = templateChoices(graph);
  assert.deepEqual(choices.map((c) => c.value), ['', 'lib.deck', 'bus']);
  assert.equal(choices[2].label, 'bus (interfaces/bus.toml)');
  // An older server's graph: the library's names.
  assert.deepEqual(templateChoices({}).map((c) => c.value), ['', 'lib.voice', 'lib.instrument', 'lib.deck']);
  assert.deepEqual(addSlotOp('spare', 'lib.voice', { x: 10.4, y: -3.6 }), { op: 'slot.add', name: 'spare', slot: 'lib.voice', x: 10, y: -4 });
  assert.deepEqual(addSlotOp('blank'), { op: 'slot.add', name: 'blank' });
});

test('the interface editor: rows from the slot, unchanged rows write nothing', () => {
  const rows = interfaceRows(FX);
  assert.deepEqual(rows.map((r) => [r.original, r.role, r.kind]), [['side', 'input', 'audio'], ['x', 'input', 'audio'], ['y', 'output', 'audio'], ['volume', 'parameter', 'generic']]);
  assert.equal(rows[3].unit, 'dB');
  assert.deepEqual(interfaceOp('fx', rows, rows.map((r) => ({ ...r }))), { op: null, problems: [] });
});

test('the interface editor: add, remove, rename and retype make one slot.interface op', () => {
  const before = interfaceRows(FX);
  const after = before.map((r) => ({ ...r }));
  after.splice(0, 1); // side removed
  after[0].name = 'input'; // x renamed
  after[1].kind = 'control'; // y retyped
  after.push(newRow('output', freeName(after, 'out')));
  const { op, problems } = interfaceOp('fx', before, after, { removeWires: true });
  assert.deepEqual(problems, []);
  assert.deepEqual(op, {
    op: 'slot.interface',
    slot: 'fx',
    remove: ['side'],
    rename: { x: 'input' },
    add: [{ name: 'out', role: 'output', kind: 'audio' }],
    retype: [{ name: 'y', role: 'output', kind: 'control' }],
    removeWires: true,
  });
  // A rename alone retypes nothing.
  const renamed = before.map((r) => ({ ...r }));
  renamed[1].name = 'input';
  assert.deepEqual(interfaceOp('fx', before, renamed).op, { op: 'slot.interface', slot: 'fx', rename: { x: 'input' } });
});

test('the interface editor refuses a bad name, a name twice, a vector without a width', () => {
  const before = interfaceRows(FX);
  const bad = [...before, { ...newRow('input', 'Bad') }];
  assert.match(interfaceOp('fx', before, bad).problems[0], /lower-case/);
  const twice = [...before, newRow('input', 'x')];
  assert.match(interfaceOp('fx', before, twice).problems[0], /named twice/);
  const vector = [...before, { ...newRow('input', 'v'), shape: 'vector', width: 0 }];
  assert.match(interfaceOp('fx', before, vector).problems[0], /width/);
  assert.equal(portNameError('remove'), 'remove is reserved in a slot’s interface');
  assert.equal(portNameError('ok_1'), null);
});

test('a port as slot.interface names it: only what differs from the defaults', () => {
  assert.deepEqual(portJson({ name: 'a', role: 'input', shape: 'signal', rate: 1, kind: 'generic', unit: '', range: null, group: '' }), { name: 'a', role: 'input' });
  assert.deepEqual(portJson({ name: 'v', role: 'output', shape: 'vector', width: 3, rate: 2, kind: 'audio', unit: 'Hz', range: [0, 1], group: 'g' }), { name: 'v', role: 'output', shape: { vector: 3 }, rate: 2, kind: 'audio', range: [0, 1], unit: 'Hz', group: 'g' });
  assert.deepEqual(portJson({ name: 'p', role: 'parameter', shape: 'events', rate: 4, kind: 'gate', unit: 'dB', range: [0, 1], group: '' }), { name: 'p', role: 'parameter', unit: 'dB' });
});

test('a content’s match by name and role, its unmatched ports, and the Load ops that add them first', () => {
  const match = contentMatch(GAIN, FX);
  assert.equal(match.matched, 3, 'x → in, y → out, volume → db, through the map');
  assert.equal(match.total, 4);
  assert.deepEqual(match.unmatched, [], 'every content port is mapped');
  const plain = contentMatch(GAIN, SPARE);
  assert.equal(plain.matched, 0);
  assert.deepEqual(plain.unmatched.map((c) => c.name), ['in', 'out', 'db']);
  assert.deepEqual(loadOps('spare', 'std.gain', { add: plain.unmatched.slice(0, 1) }), [
    { op: 'slot.interface', slot: 'spare', add: [{ name: 'in', role: 'input', kind: 'audio' }] },
    { op: 'slot.load', slot: 'spare', load: 'std.gain' },
  ]);
  assert.deepEqual(loadOps('fx', '../studio'), [{ op: 'slot.load', slot: 'fx', load: '../studio' }]);
});

test('the map editor: by name, false or a content name per port; by-name rows are left out of the op', () => {
  const rows = mapRows(FX, GAIN);
  assert.deepEqual(rows.map((r) => [r.port, r.value]), [['side', ''], ['x', 'in'], ['y', 'out'], ['volume', 'db']]);
  assert.deepEqual(rows[1].options, ['in']);
  assert.deepEqual(rows[3].options, ['db']);
  rows[0].value = false;
  rows[2].value = '';
  assert.deepEqual(mapOp('fx', rows), { op: 'slot.map', slot: 'fx', map: { side: false, x: 'in', volume: 'db' } });
  assert.deepEqual(mapOp('fx', mapRows(SPARE, null)), { op: 'slot.map', slot: 'fx', map: {} });
});

test('a template’s text: [interface], then inputs, outputs, parameters in their order; a plain generic signal port is "generic", never {} (ADR 0045 §5)', () => {
  const text = templateText('bus', [
    { name: 'pos', role: 'input', shape: 'signal', rate: 1, kind: 'control', unit: 'beats', group: 'transport' },
    { name: 'jump', role: 'input', shape: 'events', rate: 1, kind: 'trigger' },
    { name: 'v', role: 'output', shape: { vector: 2 }, rate: 4, kind: 'generic', range: { min: 0, max: 1 } },
    { name: 'l', role: 'output', shape: 'signal', rate: 1, kind: 'generic' },
    { name: 'release', role: 'parameter', group: 'transport' },
    { name: 'mix', role: 'parameter' },
  ], 'A bus "in"');
  assert.equal(
    text,
    [
      'schema_version = 1',
      '',
      '[interface]',
      'name = "bus"',
      'description = "A bus \\"in\\""',
      '',
      '[inputs]',
      'pos = { kind = "control", unit = "beats", group = "transport" }',
      'jump = { shape = "events", kind = "trigger" }',
      '',
      '[outputs]',
      'v = { shape = { vector = 2 }, rate = 4, range = [0, 1] }',
      'l = "generic"',
      '',
      '[parameters]',
      'release = { group = "transport" }',
      'mix = {}',
      '',
    ].join('\n'),
  );
  assert.equal(templateNameError('bus'), null);
  assert.match(templateNameError('Bus'), /lower-case/);
});

test('Make interface from this instance: its ports and parameters as interface entries', () => {
  const instance = { ports: [{ name: 'in', direction: 'in', shape: 'signal', rate: 1, kind: 'audio', unit: null }, { name: 'out', direction: 'out', shape: 'events', rate: 1, kind: 'trigger' }], parameters: [{ name: 'db', unit: 'dB' }] };
  assert.deepEqual(instancePorts(instance).map((p) => [p.name, p.role, p.shape, p.kind, p.unit]), [['in', 'input', 'signal', 'audio', null], ['out', 'output', 'events', 'trigger', null], ['db', 'parameter', 'signal', 'generic', 'dB']]);
});

test('the Load dialog’s projects: those that load, by the load they write, with their ports', () => {
  const entries = projectEntries({ projects: [{ load: '../studio', name: 'studio', ports: [{ name: 'l', role: 'output', shape: 'signal', rate: 1, kind: 'audio' }, { name: 'tempo', role: 'parameter', shape: 'signal', rate: 1, kind: 'generic' }] }, { load: './broken', name: null, error: 'does not load' }] });
  assert.equal(entries.length, 1);
  assert.equal(entries[0].module, '../studio');
  assert.equal(entries[0].source, 'folders');
  assert.deepEqual(entries[0].ports.map((p) => [p.name, p.direction]), [['l', 'out']]);
  assert.deepEqual(entries[0].parameters.map((p) => p.name), ['tempo']);
  assert.equal(contentMatch(entries[0], { interface: { ports: [{ name: 'l', role: 'output' }, { name: 'r', role: 'output' }] } }).matched, 1);
});

test('the slot card: from describe’s content.slots and controls, top-level slots only', () => {
  const description = {
    content: { includes: [], slots: [{ slot: '/fx', interface: FX.interface, load: 'std.gain', matches: FX.matches, dropped: [], unheard: ['extra'] }, { slot: 'comp/inner', interface: { ports: [] }, matches: [] }, { slot: '/spare', interface: SPARE.interface, matches: [] }] },
    controls: [{ address: '/fx.volume', instance: 'fx', parameter: 'volume' }],
  };
  assert.deepEqual(Object.keys(topSlots(description)), ['fx', 'spare']);
  const card = slotCard(description, 'fx');
  assert.equal(card.interface, 'slot');
  assert.equal(card.content, 'std.gain');
  assert.deepEqual(card.ports.map((p) => [p.name, p.matched]), [['side', false], ['x', true], ['y', true]]);
  assert.equal(card.ports[0].reason, '`std.gain` has no `side`');
  assert.equal(card.parameters[0].control.address, '/fx.volume');
  assert.deepEqual(card.unheard, ['extra']);
  assert.equal(cardTitle(card), 'FX: slot ◂ std.gain');
  const spare = slotCard(description, 'spare');
  assert.equal(spare.empty, true);
  assert.equal(spare.ports[0].reason, 'the slot is empty');
  assert.equal(cardTitle(spare), 'SPARE: lib.voice ◂ empty');
  assert.equal(cardTitle({ ...card, content: '../studio', project: true }), 'FX: slot ◂ ../studio ↗');
  assert.equal(slotCard(description, 'nope'), null);
});

test('a drop loads a module name or a project path, nothing else', () => {
  assert.equal(dropLoad(' moog_voice \n'), 'moog_voice');
  assert.equal(dropLoad('lib.tape_echo'), 'lib.tape_echo');
  assert.equal(dropLoad('../studio'), '../studio');
  assert.equal(dropLoad('/Users/me/Music/song.signal'), '/Users/me/Music/song.signal');
  assert.equal(dropLoad('Hello world'), null);
  assert.equal(dropLoad(''), null);
});
