import test from 'node:test';
import assert from 'node:assert/strict';
import { INITIAL, controls, powerRequest, reduce } from './power.js';

test('the switch starts on, and a click asks for the other state once', () => {
  assert.deepEqual(INITIAL, { power: 'on', pending: false });
  assert.deepEqual(powerRequest(INITIAL), { type: 'app.power', on: false });
  const pending = reduce(INITIAL, { type: 'toggle' });
  assert.equal(pending.pending, true);
  assert.equal(reduce(pending, { type: 'toggle' }), pending, 'a second click while pending does nothing');
  assert.equal(controls(pending, true).switchDisabled, true);
});

test('the reply and the server’s app state set it; a refusal leaves it', () => {
  const pending = reduce(INITIAL, { type: 'toggle' });
  const off = reduce(pending, { type: 'reply', reply: { type: 'power', on: false, playing: false } });
  assert.deepEqual(off, { power: 'off', pending: false });
  assert.deepEqual(powerRequest(off), { type: 'app.power', on: true });
  assert.deepEqual(reduce(pending, { type: 'error' }), { power: 'on', pending: false });
  assert.deepEqual(reduce(INITIAL, { type: 'app', app: { power: 'off' } }), { power: 'off', pending: false }, 'another window switched it');
  assert.equal(reduce(off, { type: 'app', app: {} }), off, 'signal ui: no power, nothing changes');
  assert.equal(reduce(off, { type: 'unknown' }), off);
});

test('off, neither Play nor Stop; on, the one that applies', () => {
  const off = { power: 'off', pending: false };
  assert.deepEqual(controls(off, false), { checked: false, label: 'off', switchDisabled: false, playDisabled: true, stopDisabled: true });
  assert.deepEqual(controls(INITIAL, false), { checked: true, label: 'on', switchDisabled: false, playDisabled: false, stopDisabled: true });
  assert.deepEqual(controls(INITIAL, true), { checked: true, label: 'on', switchDisabled: false, playDisabled: true, stopDisabled: false });
});
