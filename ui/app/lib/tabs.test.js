import test from 'node:test';
import assert from 'node:assert/strict';
import { INITIAL_TAB, TABS, select, tabForAction } from './tabs.js';

// ADR 0051 §11: one plane, the Signal Project tab, in place of the Panel and Schematic tabs.
test('the tabs are Deck, Parameters and Config, opening on the Deck', () => {
  assert.deepEqual(TABS.map((t) => t.label), ['Deck', 'Parameters', 'Config']);
  assert.equal(INITIAL_TAB, 'plane');
});

test('a tab is shown only if there is one', () => {
  assert.equal(select('plane', 'setup'), 'setup');
  assert.equal(select('setup', 'parameters'), 'parameters');
  assert.equal(select('parameters', 'transport'), 'parameters', 'the old page is no tab');
  assert.equal(select('plane', undefined), 'plane');
  assert.equal(select('plane', 'schematic'), 'plane', 'the Schematic tab is gone');
});

// AC13: no native folder chooser; Open Project… focuses the Setup tab's field.
test('Interface Setup… and Open Project… show the Setup tab', () => {
  assert.deepEqual(tabForAction('setup'), { tab: 'setup', focus: null });
  assert.deepEqual(tabForAction('open'), { tab: 'setup', focus: 'project' });
  assert.equal(tabForAction('save'), null, 'Save shows no tab');
});
