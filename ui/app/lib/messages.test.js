import test from 'node:test';
import assert from 'node:assert/strict';
import { appNotes, messageLines, mutedNotice } from './messages.js';

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
