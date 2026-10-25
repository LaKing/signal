import test from 'node:test';
import assert from 'node:assert/strict';
import {
  MAX_NAME,
  countText,
  deleteMessage,
  dropIndex,
  dropsAfter,
  keyMove,
  moveMessage,
  transitionMessage,
  transitionOf,
  transitionText,
  transitionValue,
  deletePrompt,
  deletedText,
  entries,
  exists,
  excludeMessage,
  hasUndo,
  listMessage,
  listProblem,
  undoMessage,
  undoneText,
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
  for (const invisible of ['a\u200bb', '\ufeffVerse', 'x\u2028y', 'x\u2029y', '\u200ertl', '\u3164', '\u2800\u2800', '\u3000']) {
    assert.ok(snapshotName(invisible).error, JSON.stringify(invisible));
  }
  assert.notEqual(snapshotName('e\u0301').name, snapshotName('\u00e9').name, 'not normalised, as on the server');
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

// Review of 2026-10-24, 5: a list the server refuses is said, not taken for "no project".
test('a refused list says why', () => {
  assert.equal(listProblem(null), 'Snapshots appear once a project is open.');
  assert.equal(listProblem({ code: 'no-project', message: 'open a project folder first' }), 'Snapshots appear once a project is open.');
  assert.equal(
    listProblem({ code: 'does-not-load', message: 'snapshots.toml: "/out.gain" must be a finite number' }),
    'Snapshots cannot be read: snapshots.toml: "/out.gain" must be a finite number',
  );
});

// The owner's decisions of 2026-10-24: a control can be left out of snapshots; an undo snapshot.
test('undo restore and the in-snapshots property', () => {
  assert.equal(hasUndo({ snapshots: [], undo: false }), false);
  assert.equal(hasUndo({ snapshots: [], undo: true }), true);
  assert.equal(hasUndo(null), false);
  assert.deepEqual(undoMessage('ab12'), { type: 'snapshot.undo', base: 'ab12' });
  assert.deepEqual(undoMessage(), { type: 'snapshot.undo' });
  assert.deepEqual(excludeMessage('/transport.run', true), { type: 'snapshot.exclude', address: '/transport.run', exclude: true });
  assert.deepEqual(excludeMessage('/transport.run', false), { type: 'snapshot.exclude', address: '/transport.run', exclude: false });
  const undone = undoneText({ name: 'undo', restored: 2, written: ['/a.freq'], sent: true, skipped: [], clamped: [] });
  assert.equal(undone.text, 'Undid the last restore: 2 values, 1 written to project.toml. Undo again to redo it.');
  const kept = restoredText({ name: 'A', restored: 1, written: [], sent: true, skipped: [{ address: '/transport.run', reason: 'excluded' }], clamped: [] });
  assert.equal(kept.warn, false, 'an excluded control is no problem');
  assert.match(kept.text, /left \/transport\.run alone \(not in snapshots\)/);
});

// The owner's request of 2026-10-25 (ADR 0041 §9): the list's order by drag and drop, or Alt+Arrow.
test('moving a snapshot in the list', () => {
  assert.deepEqual(moveMessage('Bridge', 0, 'ab12'), { type: 'snapshot.move', name: 'Bridge', to: 0, base: 'ab12' });
  assert.deepEqual(moveMessage('Bridge', 2), { type: 'snapshot.move', name: 'Bridge', to: 2 });
  // The keys: one place, never past an edge.
  assert.equal(keyMove('snapshot_list.move_up', 1, 3), 0);
  assert.equal(keyMove('snapshot_list.move_up', 0, 3), null);
  assert.equal(keyMove('snapshot_list.move_down', 1, 3), 2);
  assert.equal(keyMove('snapshot_list.move_down', 2, 3), null);
  assert.equal(keyMove('view.page_down', 1, 3), null);
  // Drag and drop: before or after the row under the pointer.
  assert.equal(dropsAfter(4, 30), false);
  assert.equal(dropsAfter(15, 30), true);
  assert.equal(dropIndex(2, 0, false), 0, 'the last before the first');
  assert.equal(dropIndex(0, 2, true), 2, 'the first after the last');
  assert.equal(dropIndex(0, 1, true), 1);
  assert.equal(dropIndex(0, 2, false), 1, 'before the third: second place');
  assert.equal(dropIndex(1, 1, false), null, 'onto itself');
  assert.equal(dropIndex(1, 0, true), null, 'just after the row above: where it is');
  assert.equal(dropIndex(1, 2, false), null, 'just before the row below: where it is');
});

// The owner's request of 2026-10-25 (ADR 0041 §10): a snapshot transition time.
test('the Transition field', () => {
  assert.deepEqual(transitionValue('2000'), { ms: 2000 });
  assert.deepEqual(transitionValue(' 0 '), { ms: 0 });
  assert.deepEqual(transitionValue(''), { ms: 0 }, 'empty: none');
  assert.deepEqual(transitionValue('60000'), { ms: 60000 });
  assert.match(transitionValue('60001').error, /at most 60000 ms/);
  assert.ok(transitionValue('1.5').error, 'whole milliseconds');
  assert.ok(transitionValue('-5').error);
  assert.ok(transitionValue('fast').error);
  assert.deepEqual(transitionMessage(2000, 'ab12'), { type: 'snapshot.transition', ms: 2000, base: 'ab12' });
  assert.deepEqual(transitionMessage(0), { type: 'snapshot.transition', ms: 0 });
  assert.equal(transitionOf({ transitionMs: 1500 }), 1500);
  assert.equal(transitionOf({}), 0, 'an older server');
  assert.equal(transitionOf(null), 0);
  assert.equal(transitionText(0), 'Restores switch at once (no transition).');
  assert.match(transitionText(2000), /over 2000 ms/);
});
