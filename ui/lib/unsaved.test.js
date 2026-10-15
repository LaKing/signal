// The project-level indicator's text and the unsaved messages.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

import { retryMessage, revertMessage, unsavedLine, unsavedOf, unsavedSummary } from './unsaved.js';

const state = {
  count: 2,
  parameters: [
    { address: '/out.gain', file: 1, inFile: true, control: true, applied: true, error: 'not written: Permission denied' },
    { address: '/drums/kick/amp.decay_ms', file: 300, inFile: false, control: false, applied: true, error: null },
  ],
  lastError: 'not written: Permission denied',
};

test('N unsaved parameters, with the last error', () => {
  assert.deepEqual(unsavedSummary(state), { text: '2 unsaved parameters', error: 'not written: Permission denied' });
  assert.equal(unsavedLine(state), '2 unsaved parameters — not written: Permission denied');
  const one = { count: 1, parameters: [state.parameters[1]], lastError: null };
  assert.equal(unsavedLine(one), '1 unsaved parameter');
  assert.equal(unsavedSummary({ count: 0, parameters: [], lastError: null }), null);
  assert.equal(unsavedLine(null), '');
});

test('an address entry, and the messages', () => {
  assert.equal(unsavedOf(state, '/out.gain').error, 'not written: Permission denied');
  assert.equal(unsavedOf(state, '/osc.freq'), null);
  assert.equal(unsavedOf(null, '/out.gain'), null);
  assert.deepEqual(retryMessage('/out.gain'), { type: 'retry', address: '/out.gain' });
  assert.deepEqual(revertMessage('/out.gain'), { type: 'revert', address: '/out.gain' });
});

// M2.8-9 AC5 (M2.8-8 note 021, "ui/index.html has no unsaved mark"): every
// page that edits parameters shows the indicator.
const source = (path) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');

test('every page shows the unsaved indicator', () => {
  for (const page of ['index.html', 'app/app.js', 'controls/panel.js']) {
    assert.match(source(page), /unsavedLine\(/, `${page} shows no unsaved line`);
  }
  const raw = source('index.html');
  assert.match(raw, /<p id="unsaved" class="unsaved" role="status" hidden>/);
  assert.match(raw, /import \{[^}]*unsavedLine[^}]*\} from '\/ui\/lib\/unsaved\.js'/);
  assert.match(raw, /m\.type === 'unsaved'/, 'the pushed unsaved message');
  assert.match(raw, /d\.unsaved/, "describe's unsaved");
  assert.match(raw, /retryMessage\(/);
  assert.match(raw, /revertMessage\(/);
});
