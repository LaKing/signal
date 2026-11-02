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

test('a control gets Reset and Copy address; no Restart from Files and no tab switching (the owner’s, 2026-11-02)', () => {
  const items = contextItems({ control: { address: '/drums/kick_decay', label: 'decay', editable: true }, tab: 'plane' });
  assert.deepEqual(items.map((i) => i.id), ['reset', 'copy-address']);
  assert.equal(items[0].label, 'Reset decay to default');
  assert.equal(items[0].disabled, false);
});

// ADR 0033: no Play or Stop; the project runs while the power is on.
test('without a control: nothing (Restart from Files is in the Project menu)', () => {
  assert.deepEqual(contextItems({ power: false, tab: 'setup' }), []);
  assert.ok(!contextItems({}).some((i) => i.id === 'play' || i.id === 'stop'));
  assert.equal(contextItems({ control: { address: '/a/b', editable: false } })[0].disabled, true, 'read-only');
});

// M2.9-1: a part of the schematic (an instance, a port, a wire) has its address to copy.
test('a schematic part gets Copy address, and no Reset', () => {
  const items = contextItems({ control: { address: '/drums/kick.out', label: 'out', part: true }, tab: 'plane' });
  assert.deepEqual(items.map((i) => i.id), ['copy-address']);
});

// 2026-10-17: the schematic's own items come first, in place of its Copy address.
test('a right click on the schematic shows its items first, and nothing else', () => {
  const part = [
    { id: 'inspect', label: 'Inspect', disabled: false },
    { id: 'separator' },
    { id: 'delete', label: 'Delete instance', disabled: true, title: 'Turn on Edit patch to change the patch' },
  ];
  const items = contextItems({ part, control: { address: '/drums', part: true }, tab: 'plane' });
  assert.deepEqual(items.map((i) => i.id), ['part:inspect', 'separator', 'part:delete']);
  assert.equal(items[2].title, 'Turn on Edit patch to change the patch');
  assert.equal(items[2].disabled, true);
  assert.deepEqual(contextItems({ part: [], control: { address: '/a.out', part: true } }).map((i) => i.id).slice(0, 1), ['copy-address'], 'no items: Copy address as before');
});

test('the menu stays inside the window', () => {
  assert.deepEqual(placeMenu(10, 20, 200, 100, 1000, 800), { left: 10, top: 20 });
});

// 2026-11-02: over a knob the plane adds Show Instrument and Show Stack after the control's own items.
test('an element’s extra items come after a control’s own', () => {
  const items = contextItems({ control: { address: '/synth.cutoff', label: 'cutoff', editable: true }, extra: [{ id: 'show-instrument', label: 'Show Instrument' }, { id: 'show-stack', label: 'Show Stack' }] });
  assert.deepEqual(items.map((i) => i.id), ['reset', 'copy-address', 'separator', 'part:show-instrument', 'part:show-stack']);
});
