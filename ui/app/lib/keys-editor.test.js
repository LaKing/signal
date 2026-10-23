import test from 'node:test';
import assert from 'node:assert/strict';

import { DEFAULTS, buildKeymap } from '../../lib/keys.js';
import { addWrites, changedCount, collisions, describeCollision, groups, recorded, removeWrite, resetMessage, setMessage, typed } from './keys-editor.js';

test('groups: every scope with its commands, searched by name, id, chord or scope', () => {
  const map = buildKeymap(DEFAULTS, {});
  const all = groups(map);
  assert.equal(all[0].scope, 'app');
  assert.equal(all.reduce((n, g) => n + g.commands.length, 0), DEFAULTS.commands.length);
  const page = groups(map, { search: 'page down' });
  assert.ok(page.some((g) => g.commands.some((c) => c.id === 'view.page_down')));
  assert.ok(page.some((g) => g.commands.some((c) => c.id === 'slider.decrease_page')), 'by its chord, Page Down');
  const byChord = groups(map, { search: '⇧⌘N' });
  assert.deepEqual(byChord.flatMap((g) => g.commands.map((c) => c.id)), ['window.new_main']);
  assert.deepEqual(groups(map, { search: 'piano undo' }).flatMap((g) => g.commands.map((c) => c.id)), ['piano_roll.undo']);
  assert.deepEqual(groups(map, { search: 'nothing like it' }), []);
});

test('what differs from the built-in keymap is marked and counted, conflicts said on both commands', () => {
  const map = buildKeymap(DEFAULTS, { 'window.new_schematic': ['Shift+Cmd+N'], 'view.page_down': ['PageDown'] });
  assert.equal(changedCount(map), 2);
  const rows = groups(map).flatMap((g) => g.commands);
  const schematic = rows.find((c) => c.id === 'window.new_schematic');
  assert.equal(schematic.overridden, true);
  assert.equal(schematic.conflicts.length, 1);
  assert.match(schematic.conflicts[0], /New Main Window keeps it/);
  assert.equal(rows.find((c) => c.id === 'window.new_main').conflicts.length, 1);
  assert.equal(rows.find((c) => c.id === 'view.page_up').overridden, false);
});

test('collisions of a chord about to be added: same scope, a parent, a scope under it', () => {
  const map = buildKeymap(DEFAULTS, {});
  assert.deepEqual(collisions(map, 'window.new_schematic', 'Shift+Cmd+N').map((c) => [c.command, c.kind]), [['window.new_main', 'same']]);
  assert.deepEqual(collisions(map, 'schematic.fit', 'Space').map((c) => [c.command, c.kind]), [['view.page_down', 'hides']]);
  const under = collisions(map, 'view.top', 'T').map((c) => [c.command, c.kind]);
  assert.deepEqual(under, [['schematic.trace', 'hidden']]);
  assert.deepEqual(collisions(map, 'view.top', 'F19'), []);
  assert.equal(describeCollision(collisions(map, 'window.new_schematic', 'Shift+Cmd+N')[0], 'Shift+Cmd+N'), '⇧⌘N is Signal.app ▸ New Main Window');
});

test('the writes: added, replaced, removed, back to the built-in keys', () => {
  const map = buildKeymap(DEFAULTS, {});
  assert.deepEqual(addWrites(map, 'window.new_schematic', 'Shift+Cmd+N'), [setMessage('window.new_schematic', ['Shift+Cmd+N'])]);
  assert.deepEqual(addWrites(map, 'window.new_schematic', 'Shift+Cmd+N', { replace: true }), [
    { type: 'keys.set', command: 'window.new_main', keys: [] },
    { type: 'keys.set', command: 'window.new_schematic', keys: ['Shift+Cmd+N'] },
  ]);
  assert.deepEqual(addWrites(map, 'view.page_down', 'PageDown'), [], 'already there');
  assert.deepEqual(removeWrite(map, 'view.page_down', 'Space'), { type: 'keys.set', command: 'view.page_down', keys: ['PageDown'] });
  assert.equal(removeWrite(map, 'view.page_down', 'F1'), null);
  const changed = buildKeymap(DEFAULTS, { 'view.page_down': ['PageDown'] });
  assert.deepEqual(addWrites(changed, 'view.page_down', 'Space'), [{ type: 'keys.set', command: 'view.page_down', keys: null }], 'the built-in keys again');
  assert.deepEqual(resetMessage('view.top'), { type: 'keys.reset', command: 'view.top' });
  assert.deepEqual(resetMessage(), { type: 'keys.reset' });
});

test('recording: a key press for a key command, a wheel for a wheel command; typed text checked', () => {
  const map = buildKeymap(DEFAULTS, {});
  const save = map.commands.get('file.save');
  const wheel = map.commands.get('view.wheel_inside');
  assert.equal(recorded({ type: 'keydown', key: 'k', metaKey: true, shiftKey: false, altKey: false, ctrlKey: false }, save), 'Cmd+K');
  assert.equal(recorded({ type: 'keydown', key: 'Meta', metaKey: true }, save), null, 'a modifier alone');
  assert.equal(recorded({ type: 'wheel', metaKey: true }, save), null);
  assert.equal(recorded({ type: 'wheel', altKey: false, metaKey: true, ctrlKey: false, shiftKey: false }, wheel), 'Cmd+Wheel');
  assert.deepEqual(typed('cmd+k', save), { chord: 'Cmd+K' });
  assert.ok(typed('Wheel', save).error);
  assert.ok(typed('Shift+=', save).error);
  assert.deepEqual(typed('ctrl+wheel', wheel), { chord: 'Ctrl+Wheel' });
});
