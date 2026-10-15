// M2.8-11, read from source (the look itself is checked by a person in the
// window): one scroll container (AC6), and boxes that keep their size while
// values change (AC18, docs/ui-controls.md "Stable layout").

import test from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';

const ui = new URL('../../', import.meta.url);
const read = (path) => readFileSync(new URL(path, ui), 'utf8');
const noComments = (css) => css.replace(/\/\*[\s\S]*?\*\//g, '');
const SCROLLS = /overflow(-x|-y)?\s*:\s*(auto|scroll)/g;

const controls = readdirSync(new URL('controls/', ui)).filter((f) => f.endsWith('.js'));
const panels = ['default-panel.html', '../projects/kick/ui/main.html', '../projects/pulse/ui/main.html', '../projects/physical-av/ui/main.html'];

test('the window has one scroll container: the shown tab', () => {
  const css = noComments(read('app/app.css'));
  assert.match(css, /html, body \{[^}]*overflow: hidden/, 'the page never scrolls');
  const scrolls = [...css.matchAll(/([^{}]+)\{[^}]*overflow(-x|-y)?\s*:\s*(auto|scroll)/g)].map((m) => m[1].trim());
  assert.deepEqual(scrolls, ['.tab-scroll'], 'only the Parameters and Setup tabs scroll');
  assert.match(css, /\.tab-panel \{[^}]*overflow: hidden/, 'the Panel tab does not: its frame scrolls its own document');
  assert.match(css, /\.project-panel \{[^}]*height: 100%/, 'the frame fills the tab');
  assert.doesNotMatch(css, /100vh|100vw/);
  const page = read('app/index.html');
  assert.equal([...page.matchAll(/class="tab tab-scroll"/g)].length, 2);
  assert.equal([...page.matchAll(/class="tab tab-panel"/g)].length, 1);
});

test('nothing inside a tab or a panel scrolls on its own', () => {
  assert.deepEqual([...noComments(read('controls/theme.css')).matchAll(SCROLLS)], []);
  for (const file of controls) assert.deepEqual([...read(`controls/${file}`).matchAll(SCROLLS)], [], file);
  for (const file of ['app/app.js', 'app/setup.js']) assert.deepEqual([...read(file).matchAll(SCROLLS)], [], file);
  for (const panel of panels) {
    const text = read(panel);
    assert.deepEqual([...text.matchAll(SCROLLS)], [], panel);
    assert.doesNotMatch(text, /100vh|100vw/, panel);
  }
});

test('readouts, marks, columns and the status keep their size', () => {
  const base = read('controls/base.js');
  assert.match(base, /\.readout \{[^}]*width: var\(--signal-readout-width, 11ch\);[^}]*height: 1\.3em;[^}]*overflow: hidden;/, 'a readout is a fixed box');
  assert.match(base, /\.unsaved-mark \{[^}]*position: absolute;/, 'the unsaved mark overlays, out of the flow');
  for (const file of controls) {
    assert.doesNotMatch(read(`controls/${file}`), /\.readout \{[^}]*min-width/, `${file}: a readout's width is fixed, not a minimum`);
  }
  assert.match(read('controls/knob.js'), /\.readout \{ height: 2\.6em; \}/, 'the effective value has its line, shown or not');
  assert.match(read('controls/fader.js'), /\.readout \{ height: 2\.6em; \}/);
  const css = noComments(read('app/app.css'));
  assert.match(css, /\.parameters \{ table-layout: fixed;/, 'fixed column widths');
  assert.match(css, /\.compact-status \.line \{ width: 25ch;/, 'the status is as wide as compactStatus');
  assert.match(css, /signal-app-meters \{ display: block; height: 250px;/, 'the meters box keeps its height');
  assert.match(css, /\.messages \{[^}]*height: [\d.]+em; overflow: hidden;/, 'the messages keep their height');
  const app = read('app/app.js');
  assert.match(app, /mark\.style\.visibility = 'hidden'/, 'a row’s unsaved mark keeps its space');
  assert.match(app, /this\.unsaved\.style\.visibility = line \? 'visible' : 'hidden'/);
  assert.match(read('controls/panel.js'), /this\.\$\('\.unsaved'\)\.style\.visibility = line \? 'visible' : 'hidden'/);
});

// auditor-1-unsaved-width: the unsaved line's text changes length (one
// parameter, two, an error added), and nothing beside it may move.
test('the unsaved lines are sized by their row, not by their text', async () => {
  const { unsavedLine } = await import('../../lib/unsaved.js');
  const one = { count: 1, parameters: [{ address: '/out.gain' }], lastError: null };
  const lengths = new Set([one, { ...one, count: 2 }, { ...one, lastError: 'Permission denied (os error 13)' }].map((s) => unsavedLine(s).length));
  assert.ok(lengths.size > 1, 'the text does change length');
  const css = noComments(read('app/app.css'));
  assert.match(css, /\.unsaved \{[^}]*display: flex;/);
  assert.match(css, /\.unsaved \.line \{ flex: 1 1 0; min-width: 0; \}/, 'the messages card’s line takes the row’s width, so Retry all and Revert all stay put');
  assert.match(css, /\.unsaved \.line \{[^}]*overflow: hidden; text-overflow: ellipsis;/);
  const panel = noComments(read('controls/panel.js'));
  assert.match(panel, /\.unsaved \{ flex: none; width: 46ch; margin-left: auto;/, 'the bar’s indicator is a box of fixed width at its end');
  assert.match(panel, /\.unsaved-text \{ flex: none; width: 28ch; overflow: hidden; text-overflow: ellipsis;/, 'its text is cut to a fixed width');
});
