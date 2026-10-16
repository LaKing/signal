// The schematic's sidebar (docs/ui-controls.md "Schematic"): what it shows
// for nothing selected, an instance, a wire, a pin and several parts; its
// links select what they name; its actions are the context menu's.

import test from 'node:test';
import assert from 'node:assert/strict';
import { layout } from './schematic-layout.js';
import { instanceHit, portHit, wireHit } from './schematic-hit.js';
import { hitKey } from './schematic-select.js';
import { EDIT_OFF } from './schematic-menu.js';
import { LEGEND, MEMBERS_SHOWN, addressesText, connectionsOf, sidebar, tooLargeText } from './schematic-sidebar.js';
import { KICK } from './schematic-scene.test.js';

const layouts = new Map();
const layoutOf = (level) => {
  if (!layouts.has(level)) layouts.set(level, level === '' ? layout(KICK.top) : KICK.composites[level] ? layout(KICK.composites[level]) : null);
  return layouts.get(level);
};
const TOP = { path: '', level: '' };
const nodeOf = (level, name) => layoutOf(level).nodes.find((n) => n.name === name);
const box = (name) => instanceHit(TOP, nodeOf('', name));
const wireOf = (from) => wireHit(TOP, layoutOf('').wires.find((w) => w.written.from === from));
const field = (model, label) => model.fields.find(([l]) => l === label)?.[1];
const ids = (model) => model.actions.map((a) => a.id);
const action = (model, id) => model.actions.find((a) => a.id === id);
const DESCRIBE = {
  playing: true,
  controls: [{ address: '/out.gain', instance: 'out', parameter: 'gain', module: 'std.audio_out', unit: null, min: 0, max: 4, default: 1, base: 0.75, inFile: true }],
  instances: [],
};
const context = (extra = {}) => ({ graph: KICK, description: DESCRIBE, layoutOf, ...extra });

test('nothing selected: the project in numbers, the level shown and the legend', () => {
  const model = sidebar([], context());
  assert.equal(model.kind, 'project');
  assert.equal(field(model, 'instances'), '3');
  assert.equal(field(model, 'connections'), '2');
  assert.equal(field(model, 'composites'), '2');
  assert.equal(field(model, 'level shown'), 'top level');
  assert.equal(model.legend, LEGEND);
  assert.deepEqual(LEGEND.map((e) => e.label).slice(0, 4), ['signal', 'events', 'vector', 'converter (rate changes)']);
  const inside = sidebar([], context({ shown: { path: '/drums/kick', level: 'kick' } }));
  assert.equal(field(inside, 'level shown'), '/drums/kick (kick)');
  assert.equal(field(inside, 'its instances'), '3');
  assert.equal(field(inside, 'its connections'), '1');
  assert.equal(field(inside, 'instances of it'), '1');
  assert.equal(sidebar([{ kind: 'background' }], context()).kind, 'project', 'the background is nothing');
  assert.equal(sidebar([], {}).title, 'No schematic');
});

test('an instance: what it is, its pins with their connections as links, its parameters, the menu’s actions', () => {
  const model = sidebar([box('out')], context({ editing: true }));
  assert.equal(model.kind, 'instance');
  assert.equal(field(model, 'address'), '/out');
  assert.equal(field(model, 'module'), 'std.audio_out');
  assert.equal(field(model, 'composite'), 'no');
  assert.equal(field(model, 'level'), 'top level');
  assert.deepEqual(model.pins.map((p) => [p.name, p.role, p.connected, p.connections.length]), [['l', 'input', true, 1], ['r', 'input', false, 0], ['gain', 'parameter', false, 0]]);
  const [l] = model.pins;
  assert.equal(hitKey(l.hit), 'port /out.l in');
  assert.equal(l.connections[0].wire.label, '← /drums.out');
  assert.equal(hitKey(l.connections[0].wire.hit), 'wire  /drums.out /out.l', 'the link selects that wire');
  assert.equal(hitKey(l.connections[0].end.hit), 'port /drums.out out', 'and the other end');
  assert.equal(l.connections[0].removable, true);
  assert.deepEqual(model.parameters.map((p) => [p.address, p.editable]), [['/out.gain', true]], 'the card’s rows, editable at the top level');
  assert.deepEqual(ids(model), ['copy-address', 'delete'], 'a primitive has no inside: no Enter, no Open');
  assert.equal(action(model, 'delete').disabled, false);
  const off = sidebar([box('out')], context());
  assert.equal(action(off, 'delete').title, EDIT_OFF, 'the same reason as the menu’s');

  const drums = sidebar([box('drums')], context({ menu: () => ({ open: true, wide: false }) }));
  assert.equal(field(drums, 'composite'), 'yes, 1 instance inside');
  assert.deepEqual(ids(drums), ['enter', 'close', 'copy-address', 'delete'], 'an open composite closes');
  const kick = instanceHit({ path: '/drums', level: 'drums' }, nodeOf('drums', 'kick'));
  const inner = sidebar([kick], context());
  assert.equal(field(inner, 'level'), 'composite drums, 1 instance');
  assert.ok(inner.parameters.every((p) => !p.editable), 'read-only inside a composite');
});

test('a wire: its ends as links, shape, rate, scale, converters; Edit scale and Delete', () => {
  const model = sidebar([wireOf('/lfo.out')], context({ editing: true }));
  assert.equal(model.kind, 'wire');
  assert.equal(field(model, 'shape'), 'signal');
  assert.equal(field(model, 'rate'), 'divisor 1');
  assert.equal(field(model, 'scale'), '120');
  assert.equal(field(model, 'converter'), 'none');
  assert.deepEqual(model.links.map((l) => [l.name, l.label, hitKey(l.hit)]), [
    ['from', '/lfo.out', 'port /lfo.out out'],
    ['to', '/drums.kick_decay', 'port /drums.kick_decay in'],
  ]);
  assert.deepEqual(ids(model), ['scale', 'copy-address', 'delete']);
  assert.ok(model.actions.every((a) => !a.disabled));
  // An export: its end on the frame has no pin to select, and it is not edited.
  const exported = wireHit({ path: '/drums', level: 'drums' }, layoutOf('drums').wires.find((w) => w.kind === 'export' && w.from.node === '@inputs'));
  const frame = sidebar([exported], context({ editing: true }));
  assert.equal(frame.links[0].hit, null);
  assert.equal(frame.links[0].label, '/drums.trigger');
  assert.equal(action(frame, 'delete').disabled, true);
});

test('a pin: what it is, its connections (removable in Edit patch), a level for a signal output, Disconnect all', () => {
  const lfo = nodeOf('', 'lfo');
  const out = sidebar([portHit(TOP, lfo, lfo.outs[0])], context({ editing: true, menu: () => ({ connected: 1 }) }));
  assert.equal(out.kind, 'port');
  assert.equal(field(out, 'direction'), 'output');
  assert.equal(field(out, 'connected'), '1 wire');
  assert.equal(out.meter, true);
  assert.deepEqual(out.connections.map((c) => [c.wire.label, hitKey(c.end.hit), c.removable]), [['→ /drums.kick_decay', 'port /drums.kick_decay in', true]]);
  assert.deepEqual(ids(out), ['copy-address', 'disconnect']);
  assert.equal(action(out, 'disconnect').disabled, false);
  const outNode = nodeOf('', 'out');
  const gain = sidebar([portHit(TOP, outNode, outNode.ins.find((p) => p.name === 'gain'))], context());
  assert.equal(field(gain, 'direction'), 'parameter (in)');
  assert.equal(field(gain, 'unit'), '—');
  assert.equal(field(gain, 'connected'), 'no');
  assert.equal(gain.meter, false);
  assert.equal(action(gain, 'disconnect').disabled, true);
});

test('connections of a pin at a composite’s level: an export’s end on the frame selects nothing', () => {
  const entry = { path: '/drums/kick', level: 'kick', layout: layoutOf('kick') };
  const env = nodeOf('kick', 'env');
  const [trigger] = connectionsOf(entry, env, env.ins.find((p) => p.name === 'trigger'));
  assert.equal(trigger.end.hit, null);
  assert.equal(trigger.removable, false);
  const osc = nodeOf('kick', 'osc');
  const [toDc] = connectionsOf(entry, osc, osc.outs[0]);
  assert.equal(hitKey(toDc.end.hit), 'port /drums/kick/dc.in in');
});

test('several parts: counts, the members as links, Delete all per level, Copy addresses', () => {
  const osc = instanceHit({ path: '/drums/kick', level: 'kick' }, nodeOf('kick', 'osc'));
  const lfo = nodeOf('', 'lfo');
  const selected = [box('lfo'), wireOf('/drums.out'), osc, portHit(TOP, lfo, lfo.outs[0])];
  const model = sidebar(selected, context({ editing: true }));
  assert.equal(model.kind, 'group');
  assert.equal(model.title, '4 selected');
  assert.equal(model.subtitle, '2 instances, 1 wire, 1 pin');
  assert.deepEqual(model.members.map((m) => m.label), ['/lfo', '/drums.out → /out.l', '/drums/kick/osc', '/lfo.out']);
  assert.equal(model.members[2].hit, osc);
  assert.deepEqual(model.plan.patches.map((p) => p.level), ['', 'kick']);
  assert.match(model.notes[0], /2 patches, one per level \(top level, kick\)/);
  assert.match(model.notes[1], /1 pin or export not deleted/);
  assert.deepEqual(ids(model), ['delete-all', 'copy-addresses']);
  assert.equal(action(model, 'delete-all').disabled, false);
  assert.equal(action(sidebar(selected, context()), 'delete-all').title, EDIT_OFF);
  const pins = sidebar([portHit(TOP, lfo, lfo.outs[0]), portHit(TOP, nodeOf('', 'out'), nodeOf('', 'out').ins[0])], context({ editing: true }));
  assert.equal(action(pins, 'delete-all').disabled, true, 'pins alone: nothing to delete');
  assert.equal(addressesText(selected), '/lfo\n/drums.out → /out.l\n/drums/kick/osc\n/lfo.out');
});

test('a large group: the first 200 members listed and the rest counted; a Delete over a message refused with its reason', () => {
  const many = Array.from({ length: 3000 }, (_, i) => ({ kind: 'instance', path: '', level: '', address: `/box_${i}`, node: { kind: 'instance', name: `box_${i}`, composite: false } }));
  const model = sidebar(many, context({ editing: true }));
  assert.equal(model.members.length, MEMBERS_SHOWN);
  assert.equal(model.more, 3000 - MEMBERS_SHOWN);
  assert.equal(action(model, 'delete-all').label, 'Delete 3000');
  assert.equal(action(model, 'delete-all').disabled, true);
  assert.match(action(model, 'delete-all').title, /over the 64 KiB a message \(and one undo\) may hold/);
  assert.equal(model.notes[0], tooLargeText(model.plan));
  const few = sidebar(many.slice(0, 5), context({ editing: true }));
  assert.equal(few.more, 0);
  assert.equal(action(few, 'delete-all').disabled, false);
  assert.equal(tooLargeText(few.plan), null);
});
