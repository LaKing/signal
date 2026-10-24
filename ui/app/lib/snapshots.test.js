import test from 'node:test';
import assert from 'node:assert/strict';
import {
  MAX_NAME,
  countText,
  deleteMessage,
  deletePrompt,
  deletedText,
  entries,
  exists,
  listMessage,
  overwritePrompt,
  refusalText,
  restoreMessage,
  restoredText,
  saveMessage,
  savedText,
  snapshotName,
} from './snapshots.js';

const list = {
  type: 'snapshots',
  base: 'ab12',
  snapshots: [
    { name: 'Chorus', count: 2, values: { '/a.freq': 440, '/out.gain': 0.5 } },
    { name: 'Első verze', values: { '/out.gain': 0.25 } },
  ],
};

// ADR 0041: a name is trimmed, 1 to 64 characters, any printable text.
test('names are trimmed and bounded as the server takes them', () => {
  assert.deepEqual(snapshotName('  Verse A  '), { name: 'Verse A' });
  assert.deepEqual(snapshotName('Árvíztűrő tükörfúrógép'), { name: 'Árvíztűrő tükörfúrógép' });
  assert.ok(snapshotName('   ').error);
  assert.ok(snapshotName(null).error);
  assert.ok(snapshotName('a\tb').error, 'a tab');
  const long = 'é'.repeat(MAX_NAME);
  assert.deepEqual(snapshotName(long), { name: long }, '64 characters, though 128 bytes');
  assert.match(snapshotName(`${long}x`).error, /at most 64 characters \(this one has 65\)/);
  assert.deepEqual(snapshotName('🎛'.repeat(MAX_NAME)), { name: '🎛'.repeat(MAX_NAME) }, 'code points, not UTF-16 units');
});

test('the list shows each name with its count, in the server order', () => {
  assert.deepEqual(entries(list), [
    { name: 'Chorus', count: 2 },
    { name: 'Első verze', count: 1 },
  ]);
  assert.deepEqual(entries(null), []);
  assert.equal(exists(list, 'Chorus'), true);
  assert.equal(exists(list, 'chorus'), false, 'names are exact');
  assert.equal(countText(1), '1 value');
  assert.equal(countText(0), '0 values');
});

// The messages carry the list's base, so a change by another writer is a conflict.
test('the messages', () => {
  assert.deepEqual(listMessage(), { type: 'snapshot.list' });
  assert.deepEqual(saveMessage('Chorus'), { type: 'snapshot.save', name: 'Chorus' });
  assert.deepEqual(saveMessage('Chorus', { overwrite: true, base: 'ab12' }), { type: 'snapshot.save', name: 'Chorus', overwrite: true, base: 'ab12' });
  assert.deepEqual(saveMessage('New', { base: null }), { type: 'snapshot.save', name: 'New', base: null }, 'no file yet');
  assert.deepEqual(restoreMessage('Chorus', 'ab12'), { type: 'snapshot.restore', name: 'Chorus', base: 'ab12' });
  assert.deepEqual(deleteMessage('Chorus', 'ab12'), { type: 'snapshot.delete', name: 'Chorus', base: 'ab12' });
  assert.deepEqual(deleteMessage('Chorus'), { type: 'snapshot.delete', name: 'Chorus' });
});

// The UI confirms an overwrite and a delete; Cancel is first, so it is focused.
test('overwriting and deleting ask first, Cancel the default', () => {
  const replace = overwritePrompt('Chorus');
  assert.match(replace.text, /Replace the snapshot “Chorus”/);
  assert.deepEqual(replace.buttons.map((b) => b.id), ['cancel', 'confirm']);
  assert.equal(replace.buttons[1].label, 'Replace');
  const remove = deletePrompt('Chorus');
  assert.match(remove.text, /Delete the snapshot “Chorus”\? This cannot be undone\./);
  assert.deepEqual(remove.buttons.map((b) => b.label), ['Cancel', 'Delete']);
});

test('what the card says after each action', () => {
  assert.equal(savedText({ name: 'Chorus', count: 5, replaced: false }), 'Saved “Chorus” (5 values).');
  assert.equal(savedText({ name: 'Chorus', count: 1, replaced: true }), 'Replaced “Chorus” (1 value).');
  assert.deepEqual(restoredText({ name: 'Chorus', restored: 5, written: ['/a.freq'], file: 'project.toml', sent: true, skipped: [], clamped: [] }), {
    text: 'Restored “Chorus”: 5 values, 1 written to project.toml.',
    warn: false,
  });
  const partial = restoredText({
    name: 'Old',
    restored: 2,
    written: ['/a.freq', '/out.gain'],
    sent: true,
    skipped: [{ address: '/gone.freq', reason: 'the project has no instance `gone`' }],
    clamped: [{ address: '/a.freq', value: 1e12, to: 20000 }],
  });
  assert.equal(partial.warn, true);
  assert.match(partial.text, /skipped \/gone\.freq \(no longer in the project\); clamped \/a\.freq to 20000\.$/);
  assert.equal(restoredText({ name: 'X', restored: 1, written: [], sent: false }).warn, true);
  assert.equal(deletedText({ name: 'Chorus' }), 'Deleted “Chorus”.');
  assert.match(refusalText({ code: 'conflict', message: 'snapshots.toml changed since it was listed (base); nothing was written' }), /try once more\.$/);
  assert.equal(refusalText({ code: 'exists', message: 'a snapshot is named "Chorus" already' }), 'a snapshot is named "Chorus" already');
});
