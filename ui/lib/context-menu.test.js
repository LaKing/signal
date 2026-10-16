// A context menu's keys (docs/ui-controls.md "Schematic", the context menu):
// arrows wrap past separators and stop on disabled items, Enter activates
// only an enabled one, Escape and Tab close; the menu stays in the window.

import test from 'node:test';
import assert from 'node:assert/strict';
import { firstIndex, menuKey, placeMenu } from './context-menu.js';

const ITEMS = [
  { id: 'add', label: 'Add', disabled: true, title: 'off' },
  { id: 'separator' },
  { id: 'fit', label: 'Fit' },
  { id: 'reset', label: 'Reset' },
  { id: 'separator' },
  { id: 'edit', label: 'Edit patch' },
];

test('the focus starts on the first enabled item', () => {
  assert.equal(firstIndex(ITEMS), 2);
  assert.equal(firstIndex([{ id: 'a', disabled: true }]), 0, 'all disabled: the first, so its title can be read');
  assert.equal(firstIndex([]), -1);
});

test('arrows move past separators and wrap; Home and End', () => {
  assert.deepEqual(menuKey('ArrowDown', 2, ITEMS), { focus: 3 });
  assert.deepEqual(menuKey('ArrowDown', 3, ITEMS), { focus: 5 });
  assert.deepEqual(menuKey('ArrowDown', 5, ITEMS), { focus: 0 }, 'wraps, onto a disabled item too');
  assert.deepEqual(menuKey('ArrowUp', 0, ITEMS), { focus: 5 });
  assert.deepEqual(menuKey('ArrowUp', 2, ITEMS), { focus: 0 });
  assert.deepEqual(menuKey('ArrowDown', -1, ITEMS), { focus: 0 });
  assert.deepEqual(menuKey('ArrowUp', -1, ITEMS), { focus: 5 });
  assert.deepEqual(menuKey('Home', 5, ITEMS), { focus: 0 });
  assert.deepEqual(menuKey('End', 0, ITEMS), { focus: 5 });
  assert.deepEqual(menuKey('ArrowDown', 0, [{ id: 'separator' }]), { focus: -1 });
});

test('Enter and Space activate an enabled item only; Escape and Tab close; other keys are not the menu’s', () => {
  assert.deepEqual(menuKey('Enter', 2, ITEMS), { activate: 'fit' });
  assert.deepEqual(menuKey(' ', 5, ITEMS), { activate: 'edit' });
  assert.deepEqual(menuKey('Enter', 0, ITEMS), { none: true }, 'disabled: kept open');
  assert.deepEqual(menuKey('Enter', -1, ITEMS), { none: true });
  assert.deepEqual(menuKey('Escape', 2, ITEMS), { close: true });
  assert.deepEqual(menuKey('Tab', 2, ITEMS), { close: true });
  assert.equal(menuKey('a', 2, ITEMS), null);
});

test('the menu stays inside the window', () => {
  assert.deepEqual(placeMenu(10, 20, 200, 100, 1000, 800), { left: 10, top: 20 });
  assert.deepEqual(placeMenu(950, 780, 200, 100, 1000, 800), { left: 800, top: 700 });
  assert.deepEqual(placeMenu(-5, -5, 200, 100, 1000, 800), { left: 0, top: 0 });
});
