// ADR 0037, editable insides (2026-10-27; the owner: "step into it with a
// double-click and edit its inside", every slot's inside its own): a slot
// opens like a composite, into its own inside or, until it has one, a
// read-only one (`graph.insides`) whose frame is the slot's interface; a
// note says which, and that the first edit makes the slot's own copy; that
// first edit writes `slot.inside` at the slot's level, then the edit at the
// new composite's level, each an undo entry, undone from inside; the
// frame's pins are wired by export ops; the menus' items. Driven through
// <signal-schematic>'s real pointer and render code (the drawing left out)
// and the editor's real write path over a fake connection.

import test from 'node:test';
import assert from 'node:assert/strict';
import { layout, insideKey } from '../lib/schematic-layout.js';
import { fit, toScreen } from '../lib/schematic-view.js';
import { editLevel } from '../lib/schematic-open.js';
import { DIE_MAX_K, FULL_K } from '../lib/board-zoom.js';
import { PatchSession } from '../lib/schematic-edit.js';
import { EDIT_OFF, schematicMenu } from '../lib/schematic-menu.js';
import { hitTest } from '../lib/schematic-hit.js';
import { sidebar } from '../lib/schematic-sidebar.js';
import { exportOp, frameOutputs, frameTargets, insideBanner, insideOps, insideReason, isPreviewLevel, levelData, previewSlot, slotContent, slotViews } from '../lib/slot.js';

globalThis.HTMLElement ??= class {};
globalThis.customElements ??= { get: () => undefined, define: () => {} };

const { SignalSchematic } = await import('./schematic.js');
const { SchematicEditor } = await import('./schematic-editor.js');

const port = (name, direction, address = name) => ({ name, address, direction, shape: 'signal', rate: 1, kind: 'audio', unit: null, range: null });
const slotOf = (load, inside, ports) => ({ interface: { ports }, from: null, load, project: Boolean(load && load.includes('/')), map: null, matches: [], unmatched: [], unheard: [], dropped: [], inside });
const io = [
  { name: 'in', role: 'input', shape: 'signal', rate: 1, kind: 'audio' },
  { name: 'out', role: 'output', shape: 'signal', rate: 1, kind: 'audio' },
];

/** A `graph` as the server gives it (`crates/signal-project/src/patch/inside_tests.rs` checks its shape): an empty slot, one holding a shared composite, one holding a project. */
export const GRAPH = {
  type: 'graph',
  project: 'insides',
  projectId: 'insides-1',
  top: {
    instances: [
      { name: 'osc', module: 'std.sine_osc', composite: false, ports: [port('out', 'out', '/osc.out')], parameters: [] },
      { name: 'fx', module: '', composite: false, ports: [port('in', 'in', '/fx.in'), port('out', 'out', '/fx.out')], parameters: [], slot: slotOf(null, { level: '/fx', own: false, make: 'empty', name: 'fx_inside' }, io) },
      { name: 'synth', module: 'voice', composite: true, ports: [port('out', 'out', '/synth.out')], parameters: [], slot: slotOf('voice', { level: '/synth', own: false, make: 'copy', name: 'synth_inside' }, [io[1]]) },
      { name: 'deck', module: '../studio', composite: false, ports: [port('out', 'out', '/deck.out')], parameters: [], slot: slotOf('../studio', { level: null, own: false, reason: 'a project in a slot is edited in its own folder: a slot’s own copy of a project is not made' }, [io[1]]) },
      { name: 'out', module: 'std.audio_out', composite: false, ports: [port('l', 'in', '/out.l'), port('r', 'in', '/out.r')], parameters: [] },
    ],
    connections: [
      { from: '/osc.out', to: '/fx.in', converters: [] },
      { from: '/fx.out', to: '/out.l', converters: [] },
      { from: '/synth.out', to: '/out.r', converters: [] },
    ],
  },
  composites: {
    voice: {
      instances: [{ name: 'amp', module: 'std.gain', composite: false, ports: [port('in', 'in', 'amp.in'), port('out', 'out', 'amp.out')], parameters: [{ name: 'db', address: 'amp.db', rate: 1 }] }],
      connections: [],
      inputs: [],
      outputs: [{ ...port('out', 'out'), from: 'amp.out', order: 0 }],
      parameters: [],
    },
  },
  insides: {
    '/fx': {
      instances: [],
      connections: [],
      inputs: [{ ...port('in', 'in'), to: [], unmatched: true, order: 0 }],
      outputs: [{ ...port('out', 'out'), from: null, unmatched: true, order: 0 }],
      parameters: [],
      slot: '/fx',
    },
    '/synth': {
      instances: [{ name: 'amp', module: 'std.gain', composite: false, ports: [port('in', 'in', 'amp.in'), port('out', 'out', 'amp.out')], parameters: [{ name: 'db', address: 'amp.db', rate: 1 }] }],
      connections: [],
      inputs: [],
      outputs: [{ ...port('out', 'out'), from: 'amp.out', order: 0 }],
      parameters: [],
      slot: '/synth',
    },
  },
  layout: { positions: {} },
  files: { '': 'a'.repeat(64), voice: 'b'.repeat(64) },
};

const viewport = { w: 1000, h: 600 };

/** A schematic showing GRAPH fitted, every redraw its real `render` without the drawing. */
function schematic(graph = GRAPH) {
  const s = Object.create(SignalSchematic.prototype);
  Object.assign(s, {
    graph,
    viewport,
    fitted: true,
    open: new Set(),
    pinned: new Set(),
    opening: new Map(),
    layouts: new Map(),
    moved: new Map(),
    selected: [],
    gesture: null,
    tracing: null,
    groups: new Map(),
    order: [],
    editor: { editing: false, placing: false, starts: () => false, key: () => false },
    menu: { list: {} },
    said: [],
    svg: { getBoundingClientRect: () => ({ left: 0, top: 0 }), setPointerCapture() {}, classList: { toggle() {} }, replaceChildren() {} },
  });
  s.view = fit(s.layoutOf('').bounds, viewport);
  s.select = (parts) => (s.selected = parts);
  s.endTrace = () => (s.tracing = null);
  s.say = (text) => s.said.push(text);
  for (const name of ['focus', 'hover', 'closeMenu', 'collectBars', 'applyMarks', 'applyTrace', 'applyDrag', 'drawCrumbs', 'showSide', 'applyBars', 'updateLevels']) s[name] = () => {};
  s.drawLevel = (level) => {
    if (!s.groups.has(level.path)) s.groups.set(level.path, { g: {}, nodeCache: new Map() });
    return false;
  };
  s.sideStateKey = () => '';
  s.schedule = () => s.render();
  s.render();
  return s;
}

function middleOf(s, path, name) {
  const level = s.drawn.levels.find((l) => l.path === path);
  const node = level.stored.nodes.find((n) => n.name === name);
  return toScreen(level.transform, { x: node.x + node.w / 2, y: node.y + node.h / 2 });
}

let time = 0;
function doubleClick(s, at) {
  time += 1000;
  s.doubleClick({ clientX: at.x, clientY: at.y, timeStamp: time });
}

test('a slot with an inside lays out as a box that opens into it; a project in a slot does not', () => {
  const top = layout(GRAPH.top);
  const node = (name) => top.nodes.find((n) => n.name === name);
  assert.equal(node('fx').composite, true);
  assert.equal(insideKey(node('fx')), '/fx');
  assert.equal(insideKey(node('synth')), '/synth', 'a shared composite opens into the slot’s read-only inside, not its own level');
  assert.equal(node('deck').composite, false);
  assert.equal(insideKey(node('osc')), 'std.sine_osc');
  // The read-only inside's frame is the slot's interface, its pins carrying their role and whether they are wired inside.
  const inside = layout(GRAPH.insides['/fx']);
  const frameIn = inside.nodes.find((n) => n.kind === 'input');
  assert.deepEqual(frameIn.outs.map((p) => [p.name, p.role, p.unmatched]), [['in', 'input', true]]);
  const synth = layout(GRAPH.insides['/synth']);
  const wire = synth.wires.find((w) => w.kind === 'export');
  assert.deepEqual(wire.written, { from: 'amp.out', to: 'out', role: 'output', export: 'out' });
  // A composite's own level (not a slot's inside) keeps its pins and wires as they were.
  const voice = layout(GRAPH.composites.voice);
  assert.deepEqual(voice.wires.find((w) => w.kind === 'export').written, { from: 'amp.out', to: 'out' });
});

test('a double-click on an empty slot opens its read-only inside, and the note says the first edit makes it its own', () => {
  const s = schematic();
  doubleClick(s, middleOf(s, '', 'fx'));
  assert.deepEqual([...s.drawn.open], ['/fx']);
  assert.deepEqual(editLevel(s.drawn), { path: '/fx', level: '/fx' });
  const note = insideBanner(GRAPH, s.currentLevel());
  assert.equal(note.own, false);
  assert.match(note.text, /fx is empty\. Editing makes its own inside \(composites\/fx_inside\.toml\)/);
  // The note itself, over the stage.
  const classes = new Set();
  s.insideNote = { textContent: '', classList: { toggle: (c, on) => (on ? classes.add(c) : classes.delete(c)) } };
  s.drawInsideNote();
  assert.equal(s.insideNote.textContent, note.text);
  assert.deepEqual([...classes].sort(), ['read-only', 'shown']);
});

test('a loaded slot opens read-only, its content named as shared; a project in a slot says why it does not open', () => {
  const s = schematic();
  doubleClick(s, middleOf(s, '', 'synth'));
  assert.deepEqual([...s.drawn.open], ['/synth']);
  assert.match(insideBanner(GRAPH, '/synth').text, /^Read-only: synth holds voice, shared\. Editing makes this slot’s own copy \(composites\/synth_inside\.toml\); voice stays as it is\./);
  const t = schematic();
  doubleClick(t, middleOf(t, '', 'deck'));
  assert.deepEqual([...t.drawn.open], []);
  assert.match(t.said.at(-1), /^deck: a project in a slot is edited in its own folder/);
  assert.equal(insideBanner(GRAPH, ''), null, 'the top level has no note');
});

test('its own inside: the note says so; an edit changes this slot only', () => {
  const graph = structuredClone(GRAPH);
  graph.top.instances[1].slot.inside = { level: 'fx_inside', own: true, name: 'fx_inside' };
  graph.composites.fx_inside = { ...graph.insides['/fx'] };
  delete graph.insides['/fx'];
  assert.deepEqual(insideBanner(graph, 'fx_inside'), { own: true, text: 'fx’s own inside (composites/fx_inside.toml): edits here change this slot only.' });
  const s = schematic(graph);
  doubleClick(s, middleOf(s, '', 'fx'));
  assert.deepEqual(editLevel(s.drawn), { path: '/fx', level: 'fx_inside' });
});

test('the menus: Edit inside on a slot (off for a project, with why); inside, the slot’s interface; a frame wire deleted, not scaled', () => {
  const s = schematic();
  const hit = (name) => ({ kind: 'instance', path: '', level: '', address: `/${name}`, node: s.drawn.levels[0].stored.nodes.find((n) => n.name === name) });
  const item = (items, id) => items.find((i) => i.id === id);
  const fx = schematicMenu(hit('fx'), { editing: true });
  assert.equal(item(fx, 'slot-inside').disabled, false);
  assert.equal(item(fx, 'open').disabled, false);
  const deck = schematicMenu(hit('deck'), { editing: true });
  assert.equal(item(deck, 'slot-inside').disabled, true);
  assert.match(item(deck, 'slot-inside').title, /its own folder/);
  assert.equal(item(deck, 'open').title, item(deck, 'slot-inside').title);
  const background = schematicMenu({ kind: 'background', path: '/fx', level: '/fx' }, { editing: true, slotInside: true, addable: true });
  assert.equal(item(background, 'slot-interface-inside').disabled, false);
  assert.ok(!schematicMenu({ kind: 'background', path: '', level: '' }, { editing: true }).some((i) => i.id === 'slot-interface-inside'));
  const wire = layout(GRAPH.insides['/synth']).wires.find((w) => w.kind === 'export');
  const items = schematicMenu({ kind: 'wire', path: '/synth', level: '/synth', wire }, { editing: true });
  assert.equal(item(items, 'delete').disabled, false);
  assert.equal(item(items, 'scale').disabled, true);
});

/** An editor over `graph` with the innermost level `shown`, its writes answered by `replies` in order. */
function editor(graph, shown, replies) {
  const sent = [];
  const e = Object.create(SchematicEditor.prototype);
  const s = {
    graph,
    drawn: { levels: [{ path: '', level: '' }, ...(shown ? [shown] : [])] },
    connection: {
      request: (message) => {
        sent.push(message);
        const reply = replies.shift();
        return reply?.code ? Promise.reject(Object.assign(new Error(reply.message), { code: reply.code, reply })) : Promise.resolve(reply);
      },
    },
    say: (text) => (e.said = text),
  };
  Object.assign(e, { s, session: new PatchSession(), notices: [], updateButtons() {}, hideNotice() {} });
  e.showNotice = (title, text) => e.notices.push([title, text]);
  e.session.setEditing(true);
  e.session.graphSeen(graph);
  return { e, sent };
}

test('the first edit of a read-only inside makes the slot’s own copy, then writes the edit there; both undone from inside', async () => {
  const replies = [
    { type: 'patched', level: '', file: 'project.toml', base: 'c'.repeat(64), inverse: [{ op: 'slot.restore', slot: 'fx', changes: [] }, { op: 'composite.file', name: 'fx_inside', was: 'x' }], files: [{ file: 'composites/fx_inside.toml', level: 'fx_inside', base: 'd'.repeat(64) }], notes: ['fx’s own inside: composites/fx_inside.toml (empty)'] },
    { type: 'patched', level: 'fx_inside', file: 'composites/fx_inside.toml', base: 'e'.repeat(64), inverse: [{ op: 'instance.remove', name: 'amp' }] },
    { type: 'patched', level: 'fx_inside', base: 'd'.repeat(64), inverse: [{ op: 'instance.add', name: 'amp', module: 'std.gain' }] },
    { type: 'patched', level: '', base: 'a'.repeat(64), inverse: [{ op: 'slot.inside', slot: 'fx' }], files: [{ file: 'composites/fx_inside.toml', level: 'fx_inside', base: null }] },
  ];
  const copyInverse = replies[0].inverse;
  const { e, sent } = editor(GRAPH, { path: '/fx', level: '/fx' }, replies);
  const add = { op: 'instance.add', name: 'amp', module: 'std.gain', x: 10, y: 20 };
  assert.equal(await e.write(0, '/fx', [add]), true);
  assert.deepEqual(sent[0], { type: 'patch', level: '', base: 'a'.repeat(64), ops: [{ op: 'slot.inside', slot: 'fx' }] });
  assert.deepEqual(sent[1], { type: 'patch', level: 'fx_inside', base: 'd'.repeat(64), ops: [add] });
  assert.equal(e.notices.at(-1)[0], 'Own inside made');
  assert.match(e.notices.at(-1)[1], /composites\/fx_inside\.toml/);
  const entries = e.session.history.entries;
  assert.deepEqual(entries.map((x) => x.level), ['', 'fx_inside']);
  assert.deepEqual(entries[0].views, ['/fx', 'fx_inside']);
  // Shown now as its own (the graph that follows): both undone from inside.
  e.s.drawn = { levels: [{ path: '', level: '' }, { path: '/fx', level: 'fx_inside' }] };
  assert.equal(await e.write(-1), true);
  assert.equal(sent[2].level, 'fx_inside');
  assert.equal(await e.write(-1), true, 'the copy is undone from inside the slot too');
  assert.deepEqual(sent[3], { type: 'patch', level: '', base: 'c'.repeat(64), ops: copyInverse });
  assert.equal(e.session.bases.has('fx_inside'), false, 'its file is gone');
});

test('a refused copy writes nothing more; edits elsewhere are not let through from inside', async () => {
  const { e, sent } = editor(GRAPH, { path: '/fx', level: '/fx' }, [{ code: 'does-not-load', message: 'no' }]);
  assert.equal(await e.write(0, '/fx', [{ op: 'instance.add', name: 'amp', module: 'std.gain' }]), false);
  assert.equal(sent.length, 1);
  assert.equal(e.session.history.entries.length, 0);
  assert.equal(await e.write(0, '', [{ op: 'instance.remove', name: 'osc' }]), false, 'the top level is around the open slot');
  assert.equal(sent.length, 1);
});

test('the frame is wired by export ops: a frame input to an inner input, an inner output to a frame output, a frame wire removed', () => {
  const graph = structuredClone(GRAPH);
  graph.insides['/fx'].instances = graph.insides['/synth'].instances;
  const written = [];
  const { e } = editor(graph, { path: '/fx', level: '/fx' }, []);
  e.write = (step, level, ops) => written.push([level, ops]);
  e.cord = { setAttribute() {} };
  const inside = layout(graph.insides['/fx']);
  const frameIn = inside.nodes.find((n) => n.kind === 'input');
  const frameOut = inside.nodes.find((n) => n.kind === 'output');
  const amp = inside.nodes.find((n) => n.name === 'amp');
  const portHit = (node, port, address) => ({ kind: 'port', path: '/fx', level: '/fx', node, port, address });
  assert.equal(e.starts(portHit(frameIn, frameIn.outs[0], '/fx.in')), true, 'a frame input starts a wire');
  e.connecting = { hit: portHit(frameIn, frameIn.outs[0], '/fx.in'), keys: new Set(['/fx/amp.in']), pins: [] };
  e.connectEnd(portHit(amp, amp.ins[0], '/fx/amp.in'));
  e.connecting = { hit: portHit(amp, amp.outs[0], '/fx/amp.out'), keys: new Set(['/fx.out']), pins: [] };
  e.connectEnd(portHit(frameOut, frameOut.ins[0], '/fx.out'));
  assert.deepEqual(written, [
    ['/fx', [exportOp(true, 'input', 'in', 'amp.in')]],
    ['/fx', [exportOp(true, 'output', 'out', 'amp.out')]],
  ]);
  const synth = layout(GRAPH.insides['/synth']);
  e.remove({ kind: 'wire', level: '/synth', wire: synth.wires.find((w) => w.kind === 'export') });
  assert.deepEqual(written.at(-1), ['/synth', [exportOp(false, 'output', 'out', 'amp.out')]]);
});

test('slot.js: keys, levels, targets, the removals an inside needs, why a slot does not open', () => {
  assert.equal(isPreviewLevel('/fx'), true);
  assert.equal(isPreviewLevel('rack/fx'), true);
  assert.equal(isPreviewLevel('fx_inside'), false);
  assert.equal(isPreviewLevel(''), false);
  assert.deepEqual(previewSlot('/fx'), { parent: '', slot: 'fx' });
  assert.deepEqual(previewSlot('rack/fx'), { parent: 'rack', slot: 'fx' });
  assert.equal(levelData(GRAPH, ''), GRAPH.top);
  assert.equal(levelData(GRAPH, 'voice'), GRAPH.composites.voice);
  assert.equal(levelData(GRAPH, '/fx'), GRAPH.insides['/fx']);
  assert.equal(levelData(GRAPH, 'nothing'), null);
  const synth = GRAPH.insides['/synth'];
  assert.deepEqual(frameTargets(synth, { name: 'x', role: 'input', shape: 'signal' }), [{ instance: 'amp', port: 'in', parameter: false }]);
  assert.deepEqual(frameTargets(synth, { name: 'x', role: 'parameter', shape: 'signal' }), [{ instance: 'amp', port: 'db', parameter: true }]);
  assert.deepEqual(frameOutputs(synth, 'signal'), [], 'out is wired already');
  assert.deepEqual(frameOutputs(GRAPH.insides['/fx'], 'signal'), ['out']);
  // Removing a module the frame reaches disconnects it first (once), elsewhere nothing is added.
  const remove = { op: 'instance.remove', name: 'amp' };
  const twice = { ...synth, outputs: [...synth.outputs, { ...synth.outputs[0], name: 'r', export: 'out' }] };
  assert.deepEqual(insideOps(twice, [remove]), [exportOp(false, 'output', 'out', 'amp.out'), remove]);
  assert.deepEqual(insideOps(GRAPH.composites.voice, [remove]), [remove]);
  assert.equal(insideReason(GRAPH.top.instances[3].slot), 'a project in a slot is edited in its own folder: a slot’s own copy of a project is not made');
  assert.equal(insideReason(GRAPH.top.instances[1].slot), null);
  assert.equal(insideReason({ load: 'x' }), null, 'an older server: no reason given');
});

test('Delete on several parts: a frame wire is an export op, left to the module’s removal when that goes too', async () => {
  const { deletePlan } = await import('../lib/schematic-select.js');
  const synth = layout(GRAPH.insides['/synth']);
  const wire = { kind: 'wire', path: '/synth', level: '/synth', wire: synth.wires.find((w) => w.kind === 'export') };
  const amp = { kind: 'instance', path: '/synth', level: '/synth', address: '/synth/amp', node: synth.nodes.find((n) => n.name === 'amp') };
  assert.deepEqual(
    deletePlan([wire]).patches.map((p) => [p.level, p.ops]),
    [['/synth', [exportOp(false, 'output', 'out', 'amp.out')]]],
  );
  assert.deepEqual(
    deletePlan([wire, amp]).patches.map((p) => [p.level, p.ops]),
    [['/synth', [{ op: 'instance.remove', name: 'amp' }]]],
  );
});

test('review follow-up: a slot’s own inside is listed only for its slot; a wrapped module’s note says it restarts once', async () => {
  const { loadable, insideOwner } = await import('../lib/slot.js');
  const { browserEntries } = await import('../lib/module-browser.js');
  const entries = browserEntries({ primitives: [], composites: [{ module: 'voice', composite: true }, { module: 'fx_inside', composite: true, inside_of: 'fx' }, { module: 'rack_fx_inside', composite: true, inside_of: 'rack/fx' }] });
  assert.equal(entries[1].insideOf, 'fx');
  assert.deepEqual(loadable(entries).map((e) => e.module), ['voice'], 'Add module lists no slot’s inside');
  assert.deepEqual(loadable(entries, insideOwner('', 'fx')).map((e) => e.module), ['voice', 'fx_inside']);
  assert.deepEqual(loadable(entries, insideOwner('rack', 'fx')).map((e) => e.module), ['voice', 'rack_fx_inside']);
  const graph = structuredClone(GRAPH);
  graph.top.instances[1].slot = { ...graph.top.instances[1].slot, load: 'std.gain', inside: { level: '/fx', own: false, make: 'wrap', name: 'fx_inside' } };
  graph.insides['/fx'].instances = [{ name: 'gain', module: 'std.gain', composite: false, ports: [], parameters: [] }];
  assert.match(insideBanner(graph, '/fx').text, /Editing restarts this module’s sound once \(it moves into its own inside, as \/fx\/gain\)\.$/);
  assert.doesNotMatch(insideBanner(GRAPH, '/synth').text, /restarts/, 'a copied composite plans the same: no restart');
  // A slot in a composite: the instance address shown.
  assert.match(insideBanner(graph, '/fx', '/rack1/fx').text, /as \/rack1\/fx\/gain\)\.$/);
});

test('a double-click on an empty slot zooms into it: its whole package on the stage, its die at a sane scale; one small module inside too (2026-10-29, ADR 0047 §8)', () => {
  const big = { w: 1600, h: 1000 };
  const withSine = structuredClone(GRAPH);
  withSine.insides['/fx'].instances = [{ name: 'osc', module: 'std.sine_osc', composite: false, ports: [port('out', 'out', 'osc.out')], parameters: [] }];
  for (const graph of [GRAPH, withSine]) {
    const s = schematic(graph);
    s.viewport = big;
    s.view = fit(s.layoutOf('').bounds, big);
    s.render();
    doubleClick(s, middleOf(s, '', 'fx'));
    assert.deepEqual(editLevel(s.drawn), { path: '/fx', level: '/fx' });
    const [top, fx] = s.drawn.levels;
    assert.ok(fx.transform.k <= DIE_MAX_K + 1e-9 && fx.transform.k >= FULL_K - 1e-9, `its die at ${fx.transform.k}`);
    const frame = top.layout.nodes.find((n) => n.name === 'fx');
    assert.equal(frame, top.stored.nodes.find((n) => n.name === 'fx'), 'the package as laid out: nothing grows');
    const at = toScreen(top.transform, frame);
    assert.ok(at.x >= 0 && at.y >= 0 && at.x + frame.w * top.transform.k <= big.w && at.y + frame.h * top.transform.k <= big.h, 'the whole package on the stage, its title included');
  }
});

test('the sidebar says what a slot is in each state: category slot, its module what it holds (2026-10-29)', () => {
  const graph = structuredClone(GRAPH);
  // A fourth state: its own inside (`slot.inside` made it), one module in it.
  const own = structuredClone(graph.top.instances[1]);
  Object.assign(own, { name: 'own', module: 'own_inside', composite: true });
  own.slot = { ...own.slot, load: 'own_inside', inside: { level: 'own_inside', own: true, name: 'own_inside' } };
  graph.top.instances.push(own);
  graph.composites.own_inside = { ...structuredClone(graph.insides['/synth']), slot: '/own' };
  const s = schematic(graph);
  const model = (name) => {
    const node = s.drawn.levels[0].stored.nodes.find((n) => n.name === name);
    return sidebar([{ kind: 'instance', path: '', level: '', address: `/${name}`, node }], { graph, layoutOf: (level) => s.layoutOf(level), menu: (hit) => s.menuContext(hit, { x: 0, y: 0 }) });
  };
  const fields = (name) => Object.fromEntries(model(name).fields);
  assert.deepEqual([fields('fx').category, fields('fx').module, fields('fx').inside], ['slot', 'empty', 'read-only, 0 instances (the first edit makes its own copy)']);
  assert.equal(model('fx').subtitle, 'slot: empty');
  assert.deepEqual([fields('synth').category, fields('synth').module, fields('synth').inside], ['slot', 'voice', 'read-only, 1 instance (the first edit makes its own copy)']);
  assert.deepEqual([fields('own').category, fields('own').module, fields('own').inside], ['slot', 'own_inside, its own inside', 'its own, 1 instance']);
  assert.deepEqual([fields('deck').category, fields('deck').module], ['slot', '../studio ↗, a project']);
  assert.match(fields('deck').inside, /edited in its own folder/);
  // Not a slot: as before.
  assert.deepEqual([fields('osc').module, fields('osc').composite], ['std.sine_osc', 'no']);
  assert.equal(slotContent({ from: 'lib.voice', interface: {} }), 'empty: lib.voice');
});

test('an open slot’s own items act on it from inside: the menu on its frame, the slot ops written at its level from there, Delete closing it (2026-10-29)', async () => {
  const s = schematic();
  doubleClick(s, middleOf(s, '', 'fx'));
  s.editor.editing = true;
  const item = (items, id) => items.find((i) => i.id === id);
  // A right click on the empty inside hits the frame: the slot whose inside is shown.
  const centre = { x: viewport.w / 2, y: viewport.h / 2 };
  const frame = hitTest(s.drawn, centre);
  assert.deepEqual([frame.kind, frame.address], ['instance', '/fx']);
  const items = schematicMenu(frame, s.menuContext(frame, centre));
  for (const id of ['slot-load', 'slot-map', 'slot-interface', 'slot-save-template', 'make-interface', 'delete']) assert.equal(item(items, id).disabled, false, id);
  assert.equal(item(items, 'slot-unload').title, 'The slot is empty');
  assert.equal(item(items, 'slot-detach').title, 'Its interface has no template');
  // Another box around it stays as it was: around the open slot.
  const osc = { kind: 'instance', path: '', level: '', address: '/osc', node: s.drawn.levels[0].layout.nodes.find((n) => n.name === 'osc') };
  assert.match(item(schematicMenu(osc, s.menuContext(osc, centre)), 'delete').title, /^the top level is around the open \/fx/);
  // Out of Edit patch, off as anywhere.
  s.editor.editing = false;
  assert.equal(item(schematicMenu(frame, s.menuContext(frame, centre)), 'slot-load').title, EDIT_OFF);
  // The editor: Unload of the open synth written at the top level from its inside, undone from there; a sibling's not.
  const { e, sent } = editor(GRAPH, { path: '/synth', level: '/synth' }, [{ type: 'patched', level: '', base: 'c'.repeat(64), inverse: [{ op: 'slot.load', slot: 'synth', load: 'voice' }] }]);
  const top = layout(GRAPH.top);
  const slotHit = (name) => ({ kind: 'instance', path: '', level: '', address: `/${name}`, node: top.nodes.find((n) => n.name === name) });
  e.unload(slotHit('synth'));
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(sent[0], { type: 'patch', level: '', base: 'a'.repeat(64), ops: [{ op: 'slot.unload', slot: 'synth' }] });
  assert.deepEqual(e.session.history.entries[0]?.views, ['/synth', 'synth_inside'], 'undone from inside too, whichever inside it shows');
  e.detach(slotHit('fx'));
  assert.equal(sent.length, 1, 'a sibling slot is around the open synth');
  assert.match(e.said, /^Not edited: the top level is around the open \/synth/);
});

/** The schematic `s` with fx open (double-clicked) and a real editor over a fake connection answering `replies`: `{s, e, sent}`. */
function openWithEditor(graph, replies) {
  const s = schematic(graph);
  doubleClick(s, middleOf(s, '', 'fx'));
  const { e, sent } = editor(graph, null, replies);
  s.connection = e.s.connection;
  e.s = s;
  s.editor = e;
  return { s, e, sent };
}

test('Delete from an open frame: the menu’s goes through the write gate and closes it once written; the Delete key deletes nothing (2026-10-29)', async () => {
  const frameOf = (s) => hitTest(s.drawn, { x: viewport.w / 2, y: viewport.h / 2 });
  // Backspace with the frame selected from inside (a click on its empty inside selects it): refused, nothing sent, still open.
  const keyed = openWithEditor(GRAPH, []);
  keyed.s.selected = [frameOf(keyed.s)];
  assert.equal(keyed.s.selected[0].address, '/fx');
  await keyed.s.deleteSelection();
  assert.equal(keyed.sent.length, 0);
  assert.match(keyed.s.said.at(-1), /^Not edited: the top level is around the open \/fx/);
  assert.deepEqual([...keyed.s.drawn.open], ['/fx']);
  // The menu's Delete: written at the top level from the inside shown, then closed.
  const { s, e, sent } = openWithEditor(GRAPH, [{ type: 'patched', level: '', base: 'c'.repeat(64), inverse: [{ op: 'instance.add', name: 'fx' }] }]);
  const frame = frameOf(s);
  const written = s.removePart(frame);
  assert.deepEqual([...s.drawn.open], ['/fx'], 'open until written');
  assert.equal(await written, true);
  assert.deepEqual(sent, [{ type: 'patch', level: '', base: 'a'.repeat(64), ops: [{ op: 'instance.remove', name: 'fx' }] }]);
  assert.deepEqual([...s.drawn.open], [], 'closed once written');
  assert.ok(e.session.history.entries[0].views.includes('/fx'));
  // A refused one leaves it open.
  const refused = openWithEditor(GRAPH, [{ code: 'conflict', message: 'changed' }]);
  assert.equal(await refused.s.removePart(frameOf(refused.s)), false);
  assert.deepEqual([...refused.s.drawn.open], ['/fx']);
});

test('an own inside unloaded from inside is undone from the read-only inside then shown; an own inside loaded from the read-only one, likewise (review 2026-10-29)', async () => {
  // fx holds its own inside, fx_inside, shown.
  const own = structuredClone(GRAPH);
  own.top.instances[1].module = 'fx_inside';
  own.top.instances[1].slot = { ...own.top.instances[1].slot, load: 'fx_inside', inside: { level: 'fx_inside', own: true, name: 'fx_inside' } };
  own.composites.fx_inside = { ...own.insides['/fx'] };
  delete own.insides['/fx'];
  own.files.fx_inside = 'd'.repeat(64);
  const unloaded = openWithEditor(own, [
    { type: 'patched', level: '', base: 'c'.repeat(64), inverse: [{ op: 'slot.load', slot: 'fx', load: 'fx_inside' }] },
    { type: 'patched', level: '', base: 'a'.repeat(64), inverse: [{ op: 'slot.unload', slot: 'fx' }] },
  ]);
  assert.deepEqual(editLevel(unloaded.s.drawn), { path: '/fx', level: 'fx_inside' });
  const fx = hitTest(unloaded.s.drawn, { x: viewport.w / 2, y: viewport.h / 2 });
  unloaded.e.unload(fx);
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(unloaded.sent[0].ops, [{ op: 'slot.unload', slot: 'fx' }]);
  // The graph that follows shows the read-only inside, /fx: ⌘Z from there.
  unloaded.s.drawn = { levels: [{ path: '', level: '' }, { path: '/fx', level: '/fx' }] };
  assert.equal(await unloaded.e.write(-1), true, 'undone from the inside shown now');
  assert.deepEqual(unloaded.sent[1], { type: 'patch', level: '', base: 'c'.repeat(64), ops: [{ op: 'slot.load', slot: 'fx', load: 'fx_inside' }] });

  // fx empty, its read-only inside shown; Load of an own inside named otherwise than the one `slot.inside` would write.
  const loaded = openWithEditor(GRAPH, [
    { type: 'patched', level: '', base: 'c'.repeat(64), inverse: [{ op: 'slot.unload', slot: 'fx' }] },
    { type: 'patched', level: '', base: 'a'.repeat(64), inverse: [{ op: 'slot.load', slot: 'fx', load: 'fx_inside_2' }] },
  ]);
  loaded.e.loader = { close() {} };
  loaded.e.dialogs = { close() {} };
  loaded.e.loading = loaded.e.slotTarget(hitTest(loaded.s.drawn, { x: viewport.w / 2, y: viewport.h / 2 }));
  loaded.e.loadChosen('fx_inside_2', { entry: { module: 'fx_inside_2', insideOf: 'fx' } });
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(loaded.sent[0].ops.at(-1), { op: 'slot.load', slot: 'fx', load: 'fx_inside_2' });
  loaded.s.drawn = { levels: [{ path: '', level: '' }, { path: '/fx', level: 'fx_inside_2' }] };
  assert.equal(await loaded.e.write(-1), true, 'undone from the own inside shown now');
  assert.equal(loaded.sent[1].level, '');
  assert.deepEqual(slotViews('/fx', '', 'fx', { inside: { level: '/fx', own: false, name: 'fx_inside' } }, 'x', '/fx'), ['/fx', 'fx_inside', 'x']);
});
