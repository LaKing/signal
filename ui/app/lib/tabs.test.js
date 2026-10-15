import test from 'node:test';
import assert from 'node:assert/strict';
import { INITIAL_TAB, TABS, select, tabForAction } from './tabs.js';

test('the tabs are Panel, Parameters and Setup, opening on the panel', () => {
  assert.deepEqual(TABS.map((t) => t.label), ['Panel', 'Parameters', 'Setup']);
  assert.equal(INITIAL_TAB, 'panel');
});

test('a tab is shown only if there is one', () => {
  assert.equal(select('panel', 'setup'), 'setup');
  assert.equal(select('setup', 'parameters'), 'parameters');
  assert.equal(select('parameters', 'transport'), 'parameters', 'the old page is no tab');
  assert.equal(select('panel', undefined), 'panel');
});

// AC13: no native folder chooser; Open Project… focuses the Setup tab's field.
test('Interface Setup… and Open Project… show the Setup tab', () => {
  assert.deepEqual(tabForAction('setup'), { tab: 'setup', focus: null });
  assert.deepEqual(tabForAction('open'), { tab: 'setup', focus: 'project' });
  assert.equal(tabForAction('save'), null, 'Save shows no tab');
});
