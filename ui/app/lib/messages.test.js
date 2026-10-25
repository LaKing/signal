import test from 'node:test';
import assert from 'node:assert/strict';
import { appNotes, messageLines, mutedNotice, warningsNotice } from './messages.js';

const MUTED = '/kick/click_level was muted at frame 96 (an output or event value beyond ±10⁶): …';

test('messageLines: the muted note stays first however many events come', () => {
  const notes = appNotes({ muted: MUTED, message: 'saved', notes: ['a note'] });
  const events = Array.from({ length: 10 }, (_, i) => ({ text: `event ${i}`, className: '' }));
  const lines = messageLines(events, notes, 6);
  assert.equal(lines.length, 6);
  assert.equal(lines[0].text, MUTED);
  assert.equal(lines[0].className, 'error');
  assert.equal(lines[1].text, 'event 0');
});

test('messageLines: without a muted note, events then notes as before', () => {
  const notes = appNotes({ message: 'refused', project: { diagnostics: ['bad'] } });
  const lines = messageLines([{ text: 'e', className: '' }], notes, 6);
  assert.deepEqual(lines.map((l) => l.text), ['e', 'refused', 'bad']);
});

test('mutedNotice: once when it appears or changes, never when it stays or goes', () => {
  assert.equal(mutedNotice(null, MUTED), 'Muted: /kick/click_level is silent until the project plays again (see Parameters)');
  assert.equal(mutedNotice(MUTED, MUTED), null);
  assert.equal(mutedNotice(MUTED, null), null);
  assert.equal(mutedNotice(null, null), null);
  assert.ok(mutedNotice(MUTED, `${MUTED} (also muted: /kick/level)`));
});

test('corrected values: a warn line each, and the toolbar told once when they appear or change', () => {
  const w = ['project.toml:3:40: warning: /o.shape: value 1.5 is not one of 0 sine, 1 saw: 1 plays (the file keeps 1.5 until the parameter is written)'];
  const notes = appNotes({ project: { diagnostics: [], warnings: w } });
  assert.deepEqual(notes.map((n) => [n.className, n.text]), [['warn', w[0]]]);
  assert.equal(warningsNotice([], w), 'A value in the files was corrected on load (see Parameters)');
  assert.equal(warningsNotice(w, w), null);
  assert.equal(warningsNotice(w, []), null);
  assert.equal(warningsNotice([], [...w, w[0] + ' again']), '2 values in the files were corrected on load (see Parameters)');
});
