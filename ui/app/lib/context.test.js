import test from 'node:test';
import assert from 'node:assert/strict';
import { contextItems, keepsSystemMenu, placeMenu } from './context.js';

test('text fields keep the system menu; controls and the page do not', () => {
  assert.equal(keepsSystemMenu({ tagName: 'INPUT', type: 'text' }), true);
  assert.equal(keepsSystemMenu({ tagName: 'INPUT', type: 'number' }), true);
  assert.equal(keepsSystemMenu({ tagName: 'TEXTAREA' }), true);
  assert.equal(keepsSystemMenu({ tagName: 'DIV', isContentEditable: true }), true);
  assert.equal(keepsSystemMenu({ tagName: 'INPUT', type: 'range' }), false);
  assert.equal(keepsSystemMenu({ tagName: 'SIGNAL-KNOB' }), false);
  assert.equal(keepsSystemMenu(null), false);
});

test('a control gets Reset and Copy address; then the transport and the other tabs', () => {
  const items = contextItems({ control: { address: '/drums/kick_decay', label: 'decay', editable: true }, playing: true, tab: 'panel' });
  assert.deepEqual(items.map((i) => i.id), ['reset', 'copy-address', 'separator', 'stop', 'separator', 'tab:parameters', 'tab:setup']);
  assert.equal(items[0].label, 'Reset decay to default');
  assert.equal(items[0].disabled, false);
});

test('without a control: Play (disabled while the power is off) and the tabs', () => {
  const items = contextItems({ playing: false, power: false, tab: 'setup' });
  assert.deepEqual(items.map((i) => i.id), ['play', 'separator', 'tab:panel', 'tab:parameters']);
  assert.equal(items[0].disabled, true);
  assert.equal(contextItems({ control: { address: '/a/b', editable: false } })[0].disabled, true, 'read-only');
});

test('the menu stays inside the window', () => {
  assert.deepEqual(placeMenu(10, 20, 200, 100, 1000, 800), { left: 10, top: 20 });
  assert.deepEqual(placeMenu(950, 780, 200, 100, 1000, 800), { left: 800, top: 700 });
  assert.deepEqual(placeMenu(-5, -5, 200, 100, 1000, 800), { left: 0, top: 0 });
});
