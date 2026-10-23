import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

import {
  DEFAULTS, applyKeys, buildKeymap, describeConflict, eventChord, formatChord, keyCommand, keymap, lookup, onKeys,
  parseChord, splitChord, wheelChord, wheelCommand, wheelModifierHeld, withoutWheelModifiers,
} from './keys.js';

const cases = JSON.parse(readFileSync(new URL('./fixtures/keys-cases.json', import.meta.url), 'utf8'));

test('chords parse to their canonical text, or are refused (shared with the server)', () => {
  for (const c of cases.chords) {
    const parsed = parseChord(c.text);
    if (c.error) assert.ok(parsed.error, `${c.text} should be refused, got ${parsed.chord}`);
    else assert.equal(parsed.chord, c.chord, c.text);
  }
});

test('overrides give the conflicts and problems the fixture says (shared with the server)', () => {
  for (const c of cases.overrides) {
    const map = buildKeymap(DEFAULTS, c.overrides);
    assert.equal(map.problems.length, c.problems, `${c.about}: ${map.problems.join(' | ')}`);
    const got = map.conflicts.map(({ kind, chord, command, with: w }) => ({ kind, chord, command, with: w }));
    assert.deepEqual(got, c.conflicts, c.about);
  }
});

test('the built-in keymap is whole: every chord parses, scopes exist, no clash within a scope', () => {
  const ids = new Set();
  const scopes = new Set(DEFAULTS.scopes.map((s) => s.id));
  for (const s of DEFAULTS.scopes) if (s.parent) assert.ok(scopes.has(s.parent), s.id);
  for (const c of DEFAULTS.commands) {
    assert.ok(!ids.has(c.id), `${c.id} twice`);
    ids.add(c.id);
    assert.match(c.id, /^[a-z][a-z_]*\.[a-z][a-z_]*$/, c.id);
    assert.ok(scopes.has(c.scope), `${c.id}: scope ${c.scope}`);
    for (const k of c.keys) {
      const parsed = parseChord(k);
      assert.equal(parsed.chord, k, `${c.id}: '${k}' is written canonically`);
      assert.equal(splitChord(k).key === 'Wheel', Boolean(c.wheel), `${c.id}: ${k}`);
    }
  }
  // Within a scope no two commands share a chord, and no wheel chord holds the wheel modifier.
  const map = buildKeymap(DEFAULTS, {});
  const all = buildKeymap(DEFAULTS, { 'view.page_down': ['PageDown', 'Space', 'F20'] });
  assert.deepEqual(all.conflicts, []);
  for (const [scope, table] of map.bindings) {
    const counted = DEFAULTS.commands.filter((c) => c.scope === scope).reduce((n, c) => n + c.keys.length, 0);
    assert.equal(table.size, counted, `${scope}: a chord twice`);
  }
});

test('the menu commands are the native menu items (crates/signal-app/src/window.rs)', () => {
  const menu = DEFAULTS.commands.filter((c) => c.menu).map((c) => c.id).sort();
  assert.deepEqual(menu, [
    'app.quit', 'app.settings', 'file.interface_setup', 'file.open', 'file.save', 'file.save_as',
    'help.keyboard_shortcuts', 'transport.restart', 'window.new_main', 'window.new_panel', 'window.new_schematic', 'window.reset_layout',
  ]);
});

test('formatChord shows macOS glyphs, or words elsewhere', () => {
  assert.equal(formatChord('Shift+Cmd+Z'), '⇧⌘Z');
  assert.equal(formatChord('Alt+Cmd+,'), '⌥⌘,');
  assert.equal(formatChord('PageDown'), 'Page Down');
  assert.equal(formatChord('Alt+Wheel'), '⌥ Wheel');
  assert.equal(formatChord('Cmd+ArrowUp'), '⌘↑');
  assert.equal(formatChord('Shift+Cmd+Z', { mac: false }), 'Shift+Ctrl+Z');
});

const key = (k, mods = {}, code = '') => ({ key: k, code, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...mods });

test('eventChord: the character typed, Shift only with letters and named keys, ⌥ by code', () => {
  assert.equal(eventChord(key('s', { metaKey: true }), { mac: true }), 'Cmd+S');
  assert.equal(eventChord(key('S', { metaKey: true, shiftKey: true }), { mac: true }), 'Shift+Cmd+S');
  assert.equal(eventChord(key('+', { shiftKey: true }, 'Equal'), { mac: true }), '+');
  assert.equal(eventChord(key(' '), { mac: true }), 'Space');
  assert.equal(eventChord(key(' ', { shiftKey: true }), { mac: true }), 'Shift+Space');
  assert.equal(eventChord(key('PageDown'), { mac: true }), 'PageDown');
  assert.equal(eventChord(key('≤', { altKey: true, metaKey: true }, 'Comma'), { mac: true }), 'Alt+Cmd+,');
  assert.equal(eventChord(key('Dead', { altKey: true }, 'KeyE'), { mac: true }), 'Alt+E');
  assert.equal(eventChord(key('Shift', { shiftKey: true })), null);
  // Elsewhere Ctrl is Cmd.
  assert.equal(eventChord(key('z', { ctrlKey: true }), { mac: false }), 'Cmd+Z');
  assert.equal(eventChord(key('z', { ctrlKey: true }), { mac: true }), 'Ctrl+Z');
});

test('lookup: a scope only; fine commands take Shift as finer', () => {
  const map = buildKeymap(DEFAULTS, {});
  assert.deepEqual(lookup(map, 'slider', 'ArrowUp'), { id: 'slider.increase', fine: false });
  assert.deepEqual(lookup(map, 'slider', 'Shift+ArrowUp'), { id: 'slider.increase', fine: true });
  assert.deepEqual(lookup(map, 'slider', 'Shift+Home'), null);
  assert.deepEqual(lookup(map, 'slider', 'Space'), null);
  assert.deepEqual(lookup(map, 'view', 'Space'), { id: 'view.page_down', fine: false });
  assert.deepEqual(lookup(map, 'piano_roll', 'Shift+ArrowUp'), { id: 'piano_roll.octave_up', fine: false });
});

test('the wheel modifier only takes the wheel: it is left out of a control wheel chord', () => {
  const map = buildKeymap(DEFAULTS, {});
  assert.equal(withoutWheelModifiers(map, 'Alt+Cmd+Wheel'), 'Cmd+Wheel');
  assert.equal(wheelChord({ altKey: true, metaKey: true }, { mac: true }), 'Alt+Cmd+Wheel');
  applyKeys({ overrides: {} });
  assert.deepEqual(wheelCommand('piano_roll', { altKey: true, metaKey: true }, { mac: true }), { id: 'piano_roll.zoom_time', fine: false });
  assert.deepEqual(wheelCommand('piano_roll', { altKey: true }, { mac: true }), { id: 'piano_roll.scroll', fine: false });
  assert.deepEqual(wheelCommand('slider', { shiftKey: true }, { mac: true }), { id: 'slider.wheel', fine: true });
  assert.equal(wheelModifierHeld({ altKey: true }, { mac: true }), true);
  assert.equal(wheelModifierHeld({ metaKey: true }, { mac: true }), false);
  // Another modifier, from the file.
  applyKeys({ overrides: { 'view.wheel_inside': ['Cmd+Wheel'], 'piano_roll.zoom_time': ['Ctrl+Wheel'], 'piano_roll.zoom_keys': ['Shift+Ctrl+Wheel'] } });
  assert.equal(wheelModifierHeld({ metaKey: true }, { mac: true }), true);
  assert.deepEqual(keymap().conflicts, []);
  applyKeys({ overrides: {} });
});

test('applyKeys replaces the page keymap and tells the listeners', () => {
  const seen = [];
  const stop = onKeys((map, info) => seen.push([map.commands.get('view.page_down').keys, info.problems]));
  applyKeys({ overrides: { 'view.page_down': ['PageDown'] }, path: '/x/keys.toml', problems: ['p'] });
  assert.deepEqual(seen, [[['PageDown'], ['p']]]);
  assert.equal(keyCommand('view', key(' '), { mac: true }), null);
  stop();
  applyKeys({ overrides: {} });
  assert.deepEqual(keyCommand('view', key(' '), { mac: true }), { id: 'view.page_down', fine: false });
  assert.equal(seen.length, 1);
});

test('describeConflict says which command keeps the chord', () => {
  const map = buildKeymap(DEFAULTS, { 'window.new_schematic': ['Shift+Cmd+N'] });
  assert.equal(describeConflict(map, map.conflicts[0]), '⇧⌘N is both Signal.app ▸ New Main Window and New Schematic Window: New Main Window keeps it');
});

test('the built-in keymap module holds one JSON object, as the server reads it', () => {
  const text = readFileSync(new URL('./keys.default.js', import.meta.url), 'utf8');
  const start = text.indexOf('{', text.indexOf('\nexport default'));
  const json = JSON.parse(text.slice(start, text.lastIndexOf('}') + 1));
  assert.deepEqual(json, DEFAULTS);
});

test('a wheel modifier of two keys takes the wheel only with both, and hides only chords holding both', () => {
  const map = buildKeymap(DEFAULTS, { 'view.wheel_inside': ['Ctrl+Alt+Wheel'] });
  assert.deepEqual(map.conflicts, []);
  assert.equal(withoutWheelModifiers(map, 'Ctrl+Wheel'), 'Ctrl+Wheel', 'a pinch stays a pinch');
  assert.equal(withoutWheelModifiers(map, 'Ctrl+Alt+Cmd+Wheel'), 'Cmd+Wheel');
  applyKeys({ overrides: { 'view.wheel_inside': ['Ctrl+Alt+Wheel'] } });
  assert.equal(wheelModifierHeld({ ctrlKey: true }, { mac: true }), false);
  assert.equal(wheelModifierHeld({ ctrlKey: true, altKey: true }, { mac: true }), true);
  applyKeys({ overrides: {} });
});
