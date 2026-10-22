// The setup page's draft, with the M2.5 dialog's cases (crates/signal-app/
// tests/dialog.rs): a chosen device keeps its identity when the list is
// reordered, one that disappears becomes unresolved, and Apply keeps an
// unresolved saved device.

import test from 'node:test';
import assert from 'node:assert/strict';
import { NOT_FOUND, applyMessage, bufferFrames, choiceOf, choicesOf, choose, cpuWorkers, draftOf, refreshed, selection, withFields } from './draft.js';

const dell = (id, signature = '2 ch, 48000 Hz') => ({ name: 'DELL U3219Q', id, signature, direction: 'output' });
const ssl = { name: 'SSL 2+', id: 'coreaudio:ssl', signature: '2 ch, 44100-96000 Hz', direction: 'output' };

test('a chosen device keeps its identity when the list is reordered', () => {
  const before = [dell('a'), dell('b'), ssl];
  let draft = draftOf({ setup: { buffer: 128 } }, { devices: before, output: null, input: null });
  draft = { ...draft, output: choose(1, before) };
  assert.deepEqual(choicesOf(draft).output, { name: 'DELL U3219Q', id: 'b', signature: '2 ch, 48000 Hz' });
  // The server finds DELL b at index 0 now.
  const after = [dell('b'), ssl, dell('a')];
  draft = refreshed(draft, { devices: after, choices: { output: { found: 0 } } });
  assert.equal(draft.output.kind, 'device');
  assert.equal(draft.output.index, 0);
  assert.equal(draft.output.device.id, 'b');
});

test('a chosen device that disappears becomes unresolved, and Apply keeps the saved one', () => {
  const devices = [dell('a'), ssl];
  let draft = { ...draftOf({}, { devices }), output: choose(0, devices) };
  const reason = "the saved output 'DELL U3219Q' (id a) is not present; choose a device";
  draft = refreshed(draft, { devices: [ssl], choices: { output: { unresolved: reason } } });
  assert.deepEqual(draft.output, { kind: 'unresolved', reason });
  assert.equal(selection(draft.output), 'keep');
  assert.equal(NOT_FOUND, '— not found: choose a device —');
});

// auditor-1-5: edits [256, 4, '3, 4', 'semaphore'] typed but not applied
// survive choosing a device and a refresh, and Apply sends them.
test('unsaved edits survive choosing a device and a refresh', () => {
  const devices = [dell('a'), ssl, { name: 'SSL 2+', id: 'coreaudio:ssl-in', signature: '2 ch, 44100-96000 Hz', direction: 'input' }];
  const saved = draftOf({ setup: { buffer: 128, cpuWorkers: 1, outputChannels: [1, 2], rtPolicy: 'park50' } }, { devices, output: null, input: null });
  const typed = { buffer: '256', cpuWorkers: '4', channels: '3, 4', rtPolicy: 'semaphore' };
  const edited = (draft) => [draft.buffer, draft.cpuWorkers, draft.channels, draft.rtPolicy];

  let draft = { ...withFields(saved, typed), input: choose(2, devices) };
  assert.deepEqual(edited(draft), ['256', '4', '3, 4', 'semaphore'], 'after choosing the input');
  assert.equal(draft.input.device.id, 'coreaudio:ssl-in');

  draft = refreshed(withFields(draft, typed), { devices, choices: { input: { found: 2 } } });
  assert.deepEqual(edited(draft), ['256', '4', '3, 4', 'semaphore'], 'after a refresh');

  const message = applyMessage(draft);
  assert.deepEqual([message.buffer, message.cpuWorkers, message.outputChannels, message.rtPolicy], [256, 4, '3, 4', 'semaphore']);
  assert.deepEqual(edited(withFields(saved, {})), [128, 1, '1, 2', 'park50'], 'nothing typed: the Setup’s');
});

test('the saved devices as the server resolves them', () => {
  const devices = [ssl];
  assert.equal(choiceOf({ found: 0 }, devices).device.name, 'SSL 2+');
  assert.equal(choiceOf(null, devices).kind, 'unset');
  assert.equal(choiceOf({ unresolved: 'gone' }, devices).reason, 'gone');
});

test('Apply sends the draft as setup.apply takes it', () => {
  const devices = [ssl];
  const draft = {
    output: choose(0, devices),
    input: { kind: 'unset' },
    buffer: '256',
    cpuWorkers: '2',
    channels: '1, 2',
    rtPolicy: 'semaphore',
  };
  assert.deepEqual(applyMessage(draft), {
    type: 'setup.apply',
    output: { name: 'SSL 2+', id: 'coreaudio:ssl', signature: '2 ch, 44100-96000 Hz' },
    input: null,
    buffer: 256,
    cpuWorkers: 2,
    outputChannels: '1, 2',
    rtPolicy: 'semaphore',
  });
  assert.equal(applyMessage({ ...draft, channels: '' }).outputChannels, null, 'the identity map');
  assert.equal(cpuWorkers(''), 'auto');
  assert.throws(() => cpuWorkers('65'), /1 to 64/);
});

test('the buffer and CPU workers take a decimal comma or dot as every typed number', () => {
  assert.equal(bufferFrames('256'), 256);
  assert.equal(bufferFrames('256,0'), 256);
  assert.equal(bufferFrames('127.6'), 128);
  assert.equal(bufferFrames(64), 64);
  assert.equal(bufferFrames('0'), 1);
  assert.throws(() => bufferFrames('abc'), /frames/);
  assert.throws(() => bufferFrames(''), /frames/);
  assert.equal(cpuWorkers('4,0'), 4);
  assert.throws(() => cpuWorkers('4,5'), /1 to 64/);
});
