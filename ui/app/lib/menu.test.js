import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { MENU, PANEL_PAGE, panelSource, quit, shortcut, withFragment } from './menu.js';

test('the menu has Open project, Interface setup and Quit', () => {
  assert.deepEqual(MENU.map((m) => m.label), ['Open project…', 'Interface setup', 'Quit']);
});

test('Cmd-O, Cmd-comma and Cmd-Q are the menu’s shortcuts', () => {
  assert.equal(shortcut({ metaKey: true, key: 'o' }), 'open');
  assert.equal(shortcut({ metaKey: true, key: 'O' }), 'open');
  assert.equal(shortcut({ metaKey: true, key: ',' }), 'setup');
  assert.equal(shortcut({ metaKey: true, key: 'q' }), 'quit');
  assert.equal(shortcut({ key: 'q' }), null, 'without Cmd');
  assert.equal(shortcut({ metaKey: true, shiftKey: true, key: 'q' }), null);
  assert.equal(shortcut({ metaKey: true, key: 'v' }), null, 'paste is not the menu’s');
});

test('Quit asks the app through its IPC, and nothing in a browser', () => {
  const sent = [];
  assert.equal(quit({ ipc: { postMessage: (m) => sent.push(m) } }), true);
  assert.deepEqual(sent, ['quit']);
  assert.equal(quit({}), false);
});

// auditor-1-6: the project's panel opens inside the app's shell, which
// keeps the menu (and so Open project, Interface setup and Quit).
test('the project panel is shown inside the shell with the menu', () => {
  assert.equal(PANEL_PAGE, 'panel.html');
  assert.equal(panelSource('#ws=5&token=ab'), '/project/main.html#ws=5&token=ab');
  const page = readFileSync(new URL(`../${PANEL_PAGE}`, import.meta.url), 'utf8');
  assert.match(page, /<signal-app-menu><\/signal-app-menu>/);
  assert.match(page, /<signal-app-panel><\/signal-app-panel>/);
  const app = readFileSync(new URL('../app.js', import.meta.url), 'utf8');
  assert.match(app, /href: withFragment\(PANEL_PAGE, location\.hash\)/, 'the menu’s Panel opens the shell page');
  assert.doesNotMatch(app, /href: withFragment\('\/project\//, 'never the bare panel');
});

test('another page keeps the fragment', () => {
  assert.equal(withFragment('setup.html', '#ws=5&token=ab'), 'setup.html#ws=5&token=ab');
  assert.equal(withFragment('setup.html', ''), 'setup.html');
});
