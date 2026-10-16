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
  assert.deepEqual(ids(items), ['inspect', 'open', 'enter', 'separator', 'copy-address', 'separator', 'delete']);
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

test('an open composite: Close (not while the zoom holds it open) and Add module here', () => {
  const open = schematicMenu(COMPOSITE, { editing: true, open: true, wide: false, addable: true });
  assert.deepEqual(ids(open), ['inspect', 'close', 'enter', 'separator', 'copy-address', 'separator', 'add', 'delete']);
  assert.equal(item(open, 'close').disabled, false);
  assert.equal(item(open, 'add').disabled, false);
  const wide = schematicMenu(COMPOSITE, { editing: false, open: true, wide: true, addable: true });
  assert.equal(item(wide, 'close').disabled, true);
  assert.match(item(wide, 'close').title, /zoom out/);
  assert.equal(item(wide, 'add').title, EDIT_OFF);
});

test('a wire: Inspect, Edit scale, Go to source and target, Copy addresses, Delete; an export is not edited', () => {
  const items = schematicMenu(WIRE, { editing: true });
  assert.deepEqual(ids(items), ['inspect', 'scale', 'separator', 'source', 'target', 'copy-address', 'separator', 'delete']);
  assert.ok(items.filter((i) => i.id !== 'separator').every((i) => !i.disabled));
  const off = schematicMenu(WIRE, { editing: false });
  assert.deepEqual(['scale', 'delete'].map((id) => item(off, id).title), [EDIT_OFF, EDIT_OFF]);
  assert.equal(item(off, 'source').disabled, false, 'navigation is not an edit');
  const exported = schematicMenu(EXPORT, { editing: true });
  assert.equal(item(exported, 'delete').disabled, true);
  assert.equal(item(exported, 'scale').title, 'An export is not edited here');
});

test('a pin: Inspect, Copy address, Disconnect all while something is connected', () => {
  assert.deepEqual(ids(schematicMenu(PIN, {})), ['inspect', 'copy-address', 'separator', 'disconnect']);
  assert.equal(item(schematicMenu(PIN, { editing: true, connected: 2 }), 'disconnect').disabled, false);
  assert.equal(item(schematicMenu(PIN, { editing: true, connected: 0 }), 'disconnect').title, 'Nothing is connected here');
  assert.equal(item(schematicMenu(PIN, { editing: false, connected: 2 }), 'disconnect').title, EDIT_OFF);
  assert.match(item(schematicMenu(PIN, { editing: true, connected: 0, exported: 1 }), 'disconnect').title, /exports are not edited here/, 'only an export reaches it');
});

test('the background: Add module here, Fit, Reset layout of the level shown, the Edit patch switch', () => {
  const items = schematicMenu({ kind: 'background' }, { editing: false, level: '', addable: true });
  assert.deepEqual(ids(items), ['add', 'separator', 'fit', 'reset-layout', 'separator', 'edit']);
  assert.equal(item(items, 'add').disabled, true);
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
