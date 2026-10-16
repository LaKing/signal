import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { pageHandlesShortcuts, panelReloads, panelSource, shortcut } from './menu.js';

const page = readFileSync(new URL('../index.html', import.meta.url), 'utf8');
const app = readFileSync(new URL('../app.js', import.meta.url), 'utf8');

test('Cmd-O, Cmd-S and Cmd-comma are the menu’s shortcuts', () => {
  assert.equal(shortcut({ metaKey: true, key: 'o' }), 'open');
  assert.equal(shortcut({ metaKey: true, key: 'O' }), 'open');
  assert.equal(shortcut({ metaKey: true, key: 's' }), 'save');
  assert.equal(shortcut({ metaKey: true, key: ',' }), 'setup');
  assert.equal(shortcut({ key: 's' }), null, 'without Cmd');
  assert.equal(shortcut({ metaKey: true, shiftKey: true, key: 's' }), null, 'Save As (⇧⌘S) is native only');
  assert.equal(shortcut({ metaKey: true, shiftKey: true, key: 'S' }), null);
  assert.equal(shortcut({ metaKey: true, key: 'v' }), null, 'paste is the Edit menu’s');
  assert.equal(shortcut({ metaKey: true, key: 'q' }), null, 'Quit is the native menu’s');
});

// AC16: inside Signal.app the native menu takes the shortcuts; in a browser the page does.
test('the page takes the shortcuts only without a native menu', () => {
  assert.equal(pageHandlesShortcuts({}), true, 'a browser');
  assert.equal(pageHandlesShortcuts({ signalApp: { menu: true } }), false, 'Signal.app');
  assert.equal(pageHandlesShortcuts(null), true);
});

test('the panel frame loads again only for another project', () => {
  assert.equal(panelReloads(undefined, '/p/kick'), false, 'the first project: the frame is loading it');
  assert.equal(panelReloads('/p/kick', '/p/kick'), false, 'Play, Stop, a written parameter');
  assert.equal(panelReloads('/p/kick', null), false, 'a moment without a project');
  assert.equal(panelReloads('/p/kick', '/p/pulse'), true, 'another project');
  assert.equal(panelReloads(null, '/p/kick'), true, 'a project after none');
  assert.equal(panelReloads(null, null), false);
});

test('the Panel tab shows the project’s panel with the page’s fragment', () => {
  assert.equal(panelSource('#ws=5&token=ab'), '/project/main.html#ws=5&token=ab');
  assert.equal(panelSource(''), '/project/main.html');
});

// AC3, AC4, AC16: one page; the toolbar first; no in-page menu bar, no other page.
test('one page: the toolbar, then the four tabs', () => {
  assert.doesNotMatch(page, /signal-app-menu/, 'the in-page menu bar is gone');
  const toolbar = page.indexOf('<signal-app-toolbar>');
  const main = page.indexOf('<main>');
  assert.ok(toolbar > 0 && toolbar < main, 'the toolbar is at the top');
  const tabs = [...page.matchAll(/data-tab="(\w+)" role="tabpanel"/g)].map((m) => m[1]);
  assert.deepEqual(tabs, ['panel', 'schematic', 'parameters', 'setup']);
  assert.doesNotMatch(app, /location\.(assign|href)|href:/, 'switching tabs never navigates');
  for (const gone of ['panel.html', 'setup.html']) {
    assert.throws(() => readFileSync(new URL(`../${gone}`, import.meta.url)), { code: 'ENOENT' }, `${gone} is gone`);
  }
});

test('the toolbar holds the power switch, the tabs, Play and Stop, then the status, in that order', () => {
  const at = (needle) => {
    const i = app.indexOf(needle);
    assert.ok(i >= 0, needle);
    return i;
  };
  const order = [
    at('this.switch,\n'),
    at("el('div', { className: 'tabs', role: 'tablist' }"),
    at("el('div', { className: 'transport' }, this.play, this.stop)"),
    at("el('output', { className: 'compact-status' }"),
  ];
  assert.deepEqual([...order].sort((a, b) => a - b), order);
});

// AC7: the former Transport page's content is in the Parameters tab, as cards.
test('the Parameters tab holds the status, meters, messages and parameter list as cards', () => {
  const parameters = page.slice(page.indexOf('data-tab="parameters"'), page.indexOf('data-tab="setup"'));
  for (const card of ['signal-app-status', 'signal-app-meters', 'signal-app-messages']) {
    assert.match(parameters, new RegExp(`<section class="signal-card"><h2>\\w+</h2><${card}>`), card);
  }
  assert.match(parameters, /<signal-app-parameters>/);
  assert.match(app, /el\('section', \{ className: 'signal-card wide' \}, el\('h2'/, 'a card per instance');
  const setup = page.slice(page.indexOf('data-tab="setup"'));
  assert.match(setup, /<signal-app-project>/);
  assert.match(setup, /<signal-app-setup>/);
});
