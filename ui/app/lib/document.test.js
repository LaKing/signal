import test from 'node:test';
import assert from 'node:assert/strict';
import { PROMPT, fieldValue, openRefusal, openRetry, projectLabel, windowTitle } from './document.js';

const folder = { dir: '/p/pulse', file: null, name: 'pulse', edited: false };
const file = { dir: '/open/kick-1-1', file: '/p/kick.signal', name: 'kick', edited: false };
const edited = { ...file, edited: true };

// M2.8-12 AC17: the toolbar's name, and "— Edited" while the working folder differs from the file.
test('the project is shown by its file, edited or not', () => {
  assert.deepEqual(projectLabel(file), { name: 'kick.signal', edited: false });
  assert.deepEqual(projectLabel(edited), { name: 'kick.signal', edited: true });
  assert.deepEqual(projectLabel(folder), { name: 'pulse', edited: false });
  assert.deepEqual(projectLabel({ ...folder, edited: true }), { name: 'pulse', edited: false }, 'a folder is never edited');
  assert.deepEqual(projectLabel({ dir: '/p/old/' }), { name: 'old', edited: false }, 'an older server without name');
  assert.deepEqual(projectLabel(null), { name: 'No project', edited: false });
  assert.equal(windowTitle(edited), 'kick.signal — Edited');
  assert.equal(windowTitle(file), 'kick.signal');
  assert.equal(windowTitle(folder), 'pulse');
  assert.equal(windowTitle(null), 'Signal');
});

// AC16, AC22: the Setup tab's field shows the .signal file, not its working folder.
test('the field shows the file, else the folder', () => {
  assert.equal(fieldValue(file), '/p/kick.signal');
  assert.equal(fieldValue(folder), '/p/pulse');
  assert.equal(fieldValue(null), '');
});

// AC20: in the page (a browser, or the field), an edited project is asked about first.
test('opening another project while edited asks Save, Cancel or Don’t Save', () => {
  assert.deepEqual(PROMPT.map((b) => b.label), ['Save', 'Cancel', "Don't Save"]);
  const refusal = { code: 'edited', message: 'Save changes to kick?' };
  assert.deepEqual(openRetry('/p/pulse', refusal, 'save'), { type: 'project.open', path: '/p/pulse', unsaved: 'save' });
  assert.deepEqual(openRetry('/p/pulse', refusal, 'discard'), { type: 'project.open', path: '/p/pulse', unsaved: 'discard' });
  assert.equal(openRetry('/p/pulse', refusal, 'cancel'), null, 'Cancel opens nothing');
  assert.equal(openRetry('/p/pulse', { code: 'archive', message: 'x' }, 'save'), null, 'another refusal is not asked about');
});

// AC7, AC22: a refusal is shown as the server names it; signal ui refuses what it does not serve.
test('a refusal is shown verbatim', () => {
  const notAllowed = { code: 'not-allowed', message: 'only Signal.app’s server opens projects and plays or stops them' };
  assert.equal(openRefusal(notAllowed), notAllowed.message);
  assert.equal(openRefusal({ code: 'archive', message: "kick.signal was not opened: 'ui/link' is a symbolic link" }), "kick.signal was not opened: 'ui/link' is a symbolic link");
  assert.equal(openRefusal('gone'), 'gone');
});
