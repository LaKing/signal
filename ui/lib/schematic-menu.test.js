// The schematic's context menu (docs/ui-controls.md "Schematic"): the items
// per target and state, edits disabled with a reason outside Edit patch, and
// Disconnect all as one patch of several ops.

import test from 'node:test';
import assert from 'node:assert/strict';
import { EDIT_OFF, disconnectOps, schematicMenu } from './schematic-menu.js';
import { patchMessage } from './schematic-patch.js';

const ids = (items) => items.map((i) => i.id);
const item = (items, id) => items.find((i) => i.id === id);

const PRIMITIVE = { kind: 'instance', path: '', level: '', address: '/lfo', node: { kind: 'instance', name: 'lfo', composite: false } };
const COMPOSITE = { kind: 'instance', path: '', level: '', address: '/drums', node: { kind: 'instance', name: 'drums', composite: true } };
const WIRE = { kind: 'wire', path: '', level: '', wire: { kind: 'connection', written: { from: '/lfo.out', to: '/drums.kick_decay' } }, from: '/lfo.out', to: '/drums.kick_decay' };
const EXPORT = { kind: 'wire', path: '/drums', level: 'drums', wire: { kind: 'export', written: { from: 'trigger', to: 'kick.trigger' } } };
const PIN = { kind: 'port', path: '', level: '', address: '/lfo.out', node: PRIMITIVE.node, port: { name: 'out', direction: 'out' } };

test('an instance: Inspect, Open, Enter, Copy address, Delete; a primitive has no inside', () => {
  const items = schematicMenu(PRIMITIVE, { editing: true });
  assert.deepEqual(ids(items), ['inspect', 'trace', 'open', 'enter', 'separator', 'copy-address', 'separator', 'group-slot', 'make-interface', 'delete']);
  assert.equal(item(items, 'open').disabled, true);
  assert.equal(item(items, 'open').title, 'A primitive has no inside');
  assert.equal(item(items, 'enter').disabled, true);
  assert.equal(item(items, 'delete').disabled, false);
  const composite = schematicMenu(COMPOSITE, { editing: false });
  assert.equal(item(composite, 'open').disabled, false);
  assert.equal(item(composite, 'enter').disabled, false);
  assert.equal(item(composite, 'delete').disabled, true);
  assert.equal(item(composite, 'delete').title, EDIT_OFF);
});

test('an open composite: Close (always: the zoom holds nothing open, 2026-10-22) and Add module here', () => {
  const open = schematicMenu(COMPOSITE, { editing: true, open: true, addable: true });
  assert.deepEqual(ids(open), ['inspect', 'trace', 'close', 'enter', 'separator', 'copy-address', 'separator', 'add', 'add-slot', 'group-slot', 'make-interface', 'delete']);
  assert.equal(item(open, 'close').disabled, false);
  assert.equal(item(open, 'add').disabled, false);
  const viewing = schematicMenu(COMPOSITE, { editing: false, open: true, addable: true });
  assert.equal(item(viewing, 'close').disabled, false);
  assert.equal(item(viewing, 'add').title, EDIT_OFF);
});

test('a wire: Inspect, Edit scale, Go to source and target, Copy addresses, Delete; an export is not edited', () => {
  const items = schematicMenu(WIRE, { editing: true });
  assert.deepEqual(ids(items), ['inspect', 'trace', 'scale', 'separator', 'source', 'target', 'copy-address', 'separator', 'delete']);
  assert.ok(items.filter((i) => i.id !== 'separator').every((i) => !i.disabled));
  const off = schematicMenu(WIRE, { editing: false });
  assert.deepEqual(['scale', 'delete'].map((id) => item(off, id).title), [EDIT_OFF, EDIT_OFF]);
  assert.equal(item(off, 'source').disabled, false, 'navigation is not an edit');
  const exported = schematicMenu(EXPORT, { editing: true });
  assert.equal(item(exported, 'delete').disabled, true);
  assert.equal(item(exported, 'scale').title, 'An export is not edited here');
});

test('a pin: Inspect, Copy address, Disconnect all while something is connected', () => {
  assert.deepEqual(ids(schematicMenu(PIN, {})), ['inspect', 'trace', 'copy-address', 'separator', 'disconnect']);
  assert.equal(item(schematicMenu(PIN, { editing: true, connected: 2 }), 'disconnect').disabled, false);
  assert.equal(item(schematicMenu(PIN, { editing: true, connected: 0 }), 'disconnect').title, 'Nothing is connected here');
  assert.equal(item(schematicMenu(PIN, { editing: false, connected: 2 }), 'disconnect').title, EDIT_OFF);
  assert.match(item(schematicMenu(PIN, { editing: true, connected: 0, exported: 1 }), 'disconnect').title, /exports are not edited here/, 'only an export reaches it');
});

test('the background: Add module here, Fit, Reset layout of the level shown, the Edit patch switch', () => {
  const items = schematicMenu({ kind: 'background' }, { editing: false, level: '', addable: true });
  assert.deepEqual(ids(items), ['add', 'add-slot', 'separator', 'fit', 'reset-layout', 'separator', 'edit']);
  assert.equal(item(items, 'add').disabled, true);
  assert.equal(item(items, 'add-slot').title, EDIT_OFF);
  assert.equal(item(items, 'reset-layout').label, 'Reset layout (top level)');
  assert.equal(item(items, 'edit').label, 'Edit patch');
  const editing = schematicMenu(null, { editing: true, level: 'kick', addable: true });
  assert.equal(item(editing, 'add').disabled, false);
  assert.equal(item(editing, 'reset-layout').label, 'Reset layout (kick)');
  assert.equal(item(editing, 'edit').label, 'Leave Edit patch');
});

test('Disconnect all removes every connection of the pin at its level, as one patch', () => {
  const level = {
    connections: [
      { from: '/lfo.out', to: '/drums.kick_decay', scale: 120 },
      { from: '/transport.beat', to: '/drums.trigger' },
      { from: '/lfo.out', to: '/out.gain' },
      { from: '/osc.out', to: '/lfo.freq_hz' },
    ],
  };
  const out = disconnectOps(level, 'lfo', { name: 'out', direction: 'out' });
  assert.deepEqual(out, [
    { op: 'connection.remove', from: 'lfo.out', to: 'drums.kick_decay' },
    { op: 'connection.remove', from: 'lfo.out', to: 'out.gain' },
  ]);
  assert.deepEqual(patchMessage('', 'abc', out).ops.length, 2, 'one patch, two ops');
  assert.deepEqual(disconnectOps(level, 'lfo', { name: 'freq_hz', direction: 'in' }), [{ op: 'connection.remove', from: 'osc.out', to: 'lfo.freq_hz' }]);
  assert.deepEqual(disconnectOps(level, 'drums', { name: 'out', direction: 'out' }), []);
  assert.deepEqual(disconnectOps({ connections: [{ from: 'osc.out', to: 'vca.a' }] }, 'vca', { name: 'a', direction: 'in' }), [{ op: 'connection.remove', from: 'osc.out', to: 'vca.a' }], 'inside a composite');
  assert.deepEqual(disconnectOps(null, 'lfo', { name: 'out', direction: 'out' }), []);
});

test('a part of several selected ends with Delete selected (Edit patch); the background never has it', () => {
  for (const hit of [PRIMITIVE, WIRE, PIN]) {
    const items = schematicMenu(hit, { editing: true, group: 3 });
    assert.deepEqual(ids(items).slice(-2), ['separator', 'delete-selected']);
    assert.equal(item(items, 'delete-selected').label, 'Delete selected (3)');
    assert.equal(item(items, 'delete-selected').disabled, false);
    assert.equal(item(schematicMenu(hit, { editing: false, group: 3 }), 'delete-selected').title, EDIT_OFF);
    assert.equal(item(schematicMenu(hit, { editing: true, group: 1 }), 'delete-selected'), undefined, 'alone: its own Delete');
  }
  assert.equal(item(schematicMenu({ kind: 'background' }, { editing: true, group: 3 }), 'delete-selected'), undefined);
});

test('Delete selected counts what it would delete; off when nothing would be, or when it is too large', () => {
  const items = schematicMenu(PRIMITIVE, { editing: true, group: 5, deleted: 3 });
  assert.equal(item(items, 'delete-selected').label, 'Delete selected (3)');
  const pins = schematicMenu(PIN, { editing: true, group: 2, deleted: 0 });
  assert.equal(item(pins, 'delete-selected').disabled, true);
  assert.match(item(pins, 'delete-selected').title, /pins and exports stay/);
  const large = schematicMenu(PRIMITIVE, { editing: true, group: 4000, deleted: 4000, tooLarge: 'Too much to delete at once' });
  assert.equal(item(large, 'delete-selected').disabled, true);
  assert.equal(item(large, 'delete-selected').title, 'Too much to delete at once');
});

test('Trace signal (ADR 0030 §1): on an instance, a wire, an export, a pin, always on; never on the background', () => {
  for (const hit of [PRIMITIVE, COMPOSITE, WIRE, EXPORT, PIN]) {
    for (const editing of [false, true]) {
      const trace = item(schematicMenu(hit, { editing }), 'trace');
      assert.deepEqual([trace.label, trace.disabled], ['Trace signal', false]);
    }
  }
  assert.equal(item(schematicMenu({ kind: 'background' }, {}), 'trace'), undefined);
});

// The operator's review of M2.13-3 (2026-10-20): around the innermost open
// composite the edits are off, with why; a library composite does not open in place.
test('around the open composite: Delete, Edit scale, Disconnect all, Add and Delete selected off with the reason; inspecting stays', () => {
  const around = 'the top level is around the open /synth: close it (×) to edit it';
  const off = (items, id) => assert.deepEqual([item(items, id).disabled, item(items, id).title], [true, around], id);
  const on = (items, id) => assert.equal(item(items, id).disabled, false, id);
  const instance = schematicMenu(PRIMITIVE, { editing: true, around });
  off(instance, 'delete');
  for (const id of ['inspect', 'trace', 'copy-address']) on(instance, id);
  off(schematicMenu(WIRE, { editing: true, around }), 'delete');
  off(schematicMenu(WIRE, { editing: true, around }), 'scale');
  off(schematicMenu(PIN, { editing: true, around, connected: 1 }), 'disconnect');
  off(schematicMenu({ kind: 'background', path: '' }, { editing: true, around, addable: true }), 'add');
  off(schematicMenu(PRIMITIVE, { editing: true, around, group: 2, deleted: 2 }), 'delete-selected');
  // The open composite's own Add module here… adds inside it, at the level edited: on.
  on(schematicMenu(COMPOSITE, { editing: true, around, open: true, addable: true }), 'add');
  // Outside Edit patch the reason is Edit patch's.
  assert.equal(item(schematicMenu(PRIMITIVE, { editing: false, around }), 'delete').title, EDIT_OFF);
});

test('a library composite is not opened in place: Open in place off, Enter on', () => {
  const items = schematicMenu(COMPOSITE, { editing: false, opaque: true });
  assert.deepEqual([item(items, 'open').disabled, item(items, 'open').title], [true, 'A library composite: its inside is not in the graph']);
  assert.equal(item(items, 'enter').disabled, false);
  assert.equal(item(schematicMenu(COMPOSITE, {}), 'open').disabled, false);
});

const EMPTY_SLOT = { kind: 'instance', path: '', level: '', address: '/spare', node: { kind: 'instance', name: 'spare', module: '', composite: false, slot: { from: 'lib.voice', load: null, interface: { from: 'lib.voice', ports: [] } } } };
const LOADED_SLOT = { kind: 'instance', path: '', level: '', address: '/fx', node: { kind: 'instance', name: 'fx', module: 'std.gain', composite: false, slot: { from: null, load: 'std.gain', interface: { ports: [] } } } };

test('ADR 0037 §7: a slot box: Load… when empty, Replace… and Unload when loaded, the map, the interface, templates', () => {
  const empty = schematicMenu(EMPTY_SLOT, { editing: true });
  assert.deepEqual(ids(empty), ['inspect', 'trace', 'open', 'enter', 'separator', 'copy-address', 'separator', 'slot-inside', 'slot-load', 'slot-unload', 'slot-map', 'slot-interface', 'slot-save-template', 'slot-detach', 'separator', 'make-interface', 'delete']);
  assert.equal(item(empty, 'slot-inside').title, 'An empty slot has no inside', 'an older server gives no inside');
  assert.equal(item(empty, 'slot-unload').title, 'The slot is empty');
  assert.equal(item(empty, 'slot-detach').disabled, false, 'it has a template');
  assert.equal(item(empty, 'open').title, 'An empty slot has no inside');
  assert.equal(item(empty, 'delete').label, 'Delete slot');
  const loaded = schematicMenu(LOADED_SLOT, { editing: true });
  assert.ok(ids(loaded).includes('slot-replace') && !ids(loaded).includes('slot-load'));
  assert.equal(item(loaded, 'slot-unload').disabled, false);
  assert.equal(item(loaded, 'slot-detach').title, 'Its interface has no template');
  const viewing = schematicMenu(LOADED_SLOT, { editing: false });
  for (const id of ['slot-replace', 'slot-unload', 'slot-map', 'slot-interface', 'slot-save-template', 'make-interface']) assert.equal(item(viewing, id).title, EDIT_OFF, id);
});

test('ADR 0037 §7: a slot pin: rename or retype, remove (off while wired), remove with its wires', () => {
  const pin = { kind: 'port', path: '', level: '', address: '/fx.x', node: LOADED_SLOT.node, port: { name: 'x', direction: 'in' } };
  const wired = schematicMenu(pin, { editing: true, connected: 1 });
  assert.deepEqual(ids(wired).slice(-4), ['separator', 'port-edit', 'port-remove', 'port-remove-wires']);
  assert.equal(item(wired, 'port-remove').disabled, true);
  assert.equal(item(wired, 'port-remove-wires').disabled, false);
  const free = schematicMenu(pin, { editing: true, connected: 0 });
  assert.equal(item(free, 'port-remove').disabled, false);
  assert.equal(item(free, 'port-remove-wires').title, 'No wire names it');
  assert.ok(!ids(schematicMenu(PIN, { editing: true })).includes('port-edit'), 'a primitive’s pin is not edited');
});
