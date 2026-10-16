// M2.9-3 (docs/ui-controls.md "Editing the patch"): edit mode's state, its
// keys, and the history of written patches: undo and redo send the stored
// inverse with the newest base; a conflict, or an undo the server can no
// longer apply, empties the history with a notice.

import test from 'node:test';
import assert from 'node:assert/strict';
import { PatchHistory, PatchSession, patchKey } from './schematic-edit.js';
import { addInstance, removeInstance } from './schematic-patch.js';

const key = (key, extra = {}, target = { localName: 'signal-schematic' }) => ({ key, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, composedPath: () => [target], ...extra });

test('edit mode is off at first; the switch toggles it', () => {
  const session = new PatchSession();
  assert.equal(session.editing, false);
  assert.equal(session.setEditing(), true);
  assert.equal(session.editing, true);
  assert.equal(session.setEditing(true), false, 'no change');
  session.setEditing(false);
  assert.equal(session.editing, false);
});

test('keys: Escape leaves, ⌘Z and ⇧⌘Z, Delete and Backspace; a field keeps its own', () => {
  assert.equal(patchKey(key('Escape')), 'leave');
  assert.equal(patchKey(key('Escape', {}, { localName: 'input' })), 'leave');
  assert.equal(patchKey(key('z', { metaKey: true })), 'undo');
  assert.equal(patchKey(key('z', { metaKey: true, shiftKey: true })), 'redo');
  assert.equal(patchKey(key('Delete')), 'delete');
  assert.equal(patchKey(key('Backspace')), 'delete');
  assert.equal(patchKey(key('Backspace', {}, { localName: 'input' })), null);
  assert.equal(patchKey(key('z', { metaKey: true }, { localName: 'input' })), null);
  assert.equal(patchKey(key('a')), null);
});

test('Escape with the add palette open closes it only; the next one leaves', () => {
  const field = { localName: 'input' };
  assert.equal(patchKey(key('Escape', {}, field), { palette: true }), 'close', 'the search or name field');
  assert.equal(patchKey(key('Escape'), { palette: true }), 'close');
  assert.equal(patchKey(key('Escape', {}, field), { palette: false }), 'leave', 'closed: Escape leaves');
  assert.equal(patchKey(key('Escape')), 'leave');
  assert.equal(patchKey(key('z', { metaKey: true }), { palette: true }), 'undo');
});

test('a patch, its undo and its redo: the stored inverse, with the newest base', () => {
  const session = new PatchSession();
  session.graphSeen({ files: { '': 'b0', kick: 'k0' } });
  const ops = [addInstance('svf', 'std.svf', { x: 1, y: 2 })];
  const sent = session.request('', ops);
  assert.deepEqual(sent.message, { type: 'patch', level: '', base: 'b0', ops });
  assert.ok(session.request('', ops).refused, 'one write at a time');
  const inverse = [{ op: 'instance.remove', name: 'svf', edits: [{ edit: 'cut' }] }];
  session.written({ type: 'patched', level: '', base: 'b1', inverse });
  assert.equal(session.history.canUndo, true);
  assert.equal(session.history.canRedo, false);
  // A commit elsewhere moved the file on: the graph push gives the newest base.
  session.graphSeen({ files: { '': 'b2', kick: 'k0' } });
  const undo = session.undo();
  assert.deepEqual(undo.message, { type: 'patch', level: '', base: 'b2', ops: inverse }, 'the inverse verbatim');
  const redoOps = [{ op: 'instance.add', name: 'svf', module: 'std.svf', edits: [{ edit: 'insert' }] }];
  session.written({ type: 'patched', level: '', base: 'b3', inverse: redoOps });
  assert.equal(session.history.canUndo, false);
  assert.equal(session.history.canRedo, true);
  const redo = session.redo();
  assert.deepEqual(redo.message.ops, redoOps, 'redo sends the undo’s inverse');
  assert.equal(redo.message.base, 'b3');
  session.written({ type: 'patched', level: '', base: 'b4', inverse: [{ op: 'instance.remove', name: 'svf', edits: [] }] });
  assert.equal(session.history.at, 1);
  // A new patch drops nothing to redo and records a new entry.
  session.undo();
  session.written({ type: 'patched', level: '', base: 'b5', inverse: redoOps });
  session.request('kick', [removeInstance('dc')]);
  session.written({ type: 'patched', level: 'kick', base: 'k1', inverse: [] });
  assert.equal(session.history.entries.length, 1, 'redo dropped by a new patch');
  assert.equal(session.history.entries[0].level, 'kick');
  assert.equal(session.bases.get('kick'), 'k1');
});

test('a conflict, or an undo the server refuses as bad-value, empties the history with a notice', () => {
  const session = new PatchSession();
  session.graphSeen({ files: { '': 'b0' } });
  session.request('', [removeInstance('a')]);
  session.written({ level: '', base: 'b1', inverse: [{ op: 'instance.add' }] });
  session.undo();
  const error = Object.assign(new Error('changed'), { code: 'conflict', reply: { code: 'conflict', level: '', base: 'b9' } });
  const result = session.refused(error);
  assert.match(result.cleared, /history was emptied/);
  assert.equal(session.history.canUndo, false);
  assert.equal(session.bases.get(''), 'b9', 'the current base taken');
  session.request('', [removeInstance('b')]);
  session.written({ level: '', base: 'b10', inverse: [{ op: 'instance.add' }] });
  session.undo();
  assert.ok(session.refused(Object.assign(new Error('no'), { code: 'bad-value' })).cleared);
  assert.equal(session.history.entries.length, 0);
  // A new patch refused otherwise keeps the history and says why, with the references.
  session.request('', [removeInstance('c')]);
  session.written({ level: '', base: 'b11', inverse: [] });
  session.request('', [removeInstance('drums')]);
  const refused = session.refused(Object.assign(new Error('in use'), { code: 'in-use', reply: { code: 'in-use', references: [{ file: 'ui/main.html', line: 30, text: '/drums.kick_decay' }] } }));
  assert.equal(refused.reason, 'in use');
  assert.equal(refused.references[0].text, '/drums.kick_decay');
  assert.equal(session.history.entries.length, 1);
});

test('two projects with the same folder and project name are told apart by projectId', () => {
  // auditor-1-008: /a/kick and /b/kick, both folder "kick", project "kick".
  const session = new PatchSession();
  session.graphSeen({ folder: 'kick', project: 'kick', projectId: 'id-a', files: { '': 'a0' } });
  const sent = session.request('', [addInstance('svf', 'std.svf')]);
  session.written({ level: '', base: 'a1', inverse: [{ op: 'instance.remove', name: 'svf' }] }, sent.pending);
  assert.equal(session.history.canUndo, true);
  const result = session.graphSeen({ folder: 'kick', project: 'kick', projectId: 'id-b', files: { '': 'b0' } });
  assert.match(result.cleared, /Another project/);
  assert.equal(session.history.canUndo, false);
  assert.equal(session.undo(), null, 'A’s inverse is never sent to B');
  assert.deepEqual([...session.bases], [['', 'b0']]);
  // The same project seen again keeps a history written in it.
  const again = session.request('', [removeInstance('x')]);
  session.written({ level: '', base: 'b1', inverse: [{ op: 'instance.add' }] }, again.pending);
  assert.equal(session.graphSeen({ folder: 'kick', project: 'kick', projectId: 'id-b', files: { '': 'b1' } }), null);
  assert.equal(session.history.canUndo, true);
});

test('another project empties the history: no inverse is sent to a project it was not written in', () => {
  const session = new PatchSession();
  session.graphSeen({ folder: 'a', project: 'a', files: { '': 'a0' } });
  const sent = session.request('', [addInstance('svf', 'std.svf')]);
  session.written({ level: '', base: 'a1', inverse: [{ op: 'instance.remove', name: 'svf' }] }, sent.pending);
  assert.equal(session.history.canUndo, true);
  // The same project again keeps it.
  assert.equal(session.graphSeen({ folder: 'a', project: 'a', files: { '': 'a1' } }), null);
  assert.equal(session.history.canUndo, true);
  // Another project: emptied, with a notice, and its bases only.
  const result = session.graphSeen({ folder: 'b', project: 'b', files: { '': 'b0' } });
  assert.match(result.cleared, /Another project/);
  assert.equal(session.history.canUndo, false);
  assert.equal(session.undo(), null);
  assert.deepEqual([...session.bases], [['', 'b0']]);
  // A write in flight when the project changed: its reply changes nothing.
  const pending = session.request('', [removeInstance('x')]).pending;
  session.graphSeen({ folder: 'c', project: 'c', files: { '': 'c0' } });
  session.written({ level: '', base: 'b1', inverse: [{ op: 'instance.add' }] }, pending);
  assert.equal(session.history.canUndo, false);
  assert.equal(session.bases.get(''), 'c0');
  assert.ok(session.request('', [removeInstance('y')]).message, 'a new write can start');
});

test('the history alone: undo and redo positions', () => {
  const history = new PatchHistory();
  assert.equal(history.undo(), null);
  history.push({ level: '', forward: ['f1'], backward: ['b1'] });
  history.push({ level: '', forward: ['f2'], backward: ['b2'] });
  assert.deepEqual(history.undo().backward, ['b2']);
  history.moved(-1, ['f2*']);
  assert.deepEqual(history.redo().forward, ['f2*']);
  history.moved(+1, ['b2*']);
  assert.deepEqual(history.undo().backward, ['b2*']);
  history.clear();
  assert.equal(history.canUndo || history.canRedo, false);
});
