import test from 'node:test';
import assert from 'node:assert/strict';

import { BLACK_SHARE, MIN_VELOCITY, auditionTarget, keyAt } from './audition.js';
import { MAX_KEY, keyToY, makeView } from './piano-roll-model.js';

const view = makeView({ rowPx: 10, scrollY: (MAX_KEY - 72) * 10, height: 200, width: 400 });
const W = 64;
const rowY = (key) => keyToY(key, view);

test('a press on the keys: its row; right of a black key, the white key above or below', () => {
  // C5 (72) is the top row here, then B4, A#4 (black), A4, …
  assert.equal(rowY(72), 0);
  assert.equal(keyAt(10, rowY(71) + 5, view, W).key, 71, 'B4, white');
  assert.equal(keyAt(10, rowY(70) + 5, view, W).key, 70, 'A#4 on the black key');
  assert.equal(keyAt(50, rowY(70) + 2, view, W).key, 71, 'right of it, upper half: B4');
  assert.equal(keyAt(50, rowY(70) + 7, view, W).key, 69, 'lower half: A4');
  assert.equal(keyAt(Math.round(W * BLACK_SHARE), rowY(70) + 7, view, W).key, 69, 'from the black key\'s end');
  assert.equal(keyAt(-1, 5, view, W), null);
  assert.equal(keyAt(W + 1, 5, view, W), null);
  assert.equal(keyAt(10, 5, view, 0), null);
  // Past the highest key.
  const top = makeView({ rowPx: 10, scrollY: -20 });
  assert.equal(keyAt(10, 5, top, W), null);
  assert.equal(keyAt(10, 25, top, W).key, MAX_KEY);
});

test('the velocity grows towards the key\'s tip, from the softest to 1', () => {
  const at = (x, key) => keyAt(x, rowY(key) + 5, view, W).velocity;
  assert.equal(at(0, 71), MIN_VELOCITY);
  assert.equal(at(W, 71), 1);
  assert.ok(Math.abs(at(W / 2, 71) - (MIN_VELOCITY + (1 - MIN_VELOCITY) / 2)) < 1e-12);
  // A black key is shorter: its tip is louder sooner.
  const blackW = Math.round(W * BLACK_SHARE);
  assert.equal(at(blackW - 0.001, 70) > 0.99, true);
  assert.ok(at(20, 70) > at(20, 71));
});

test('the live notes go to the attribute, else the followed sequencer, else the only one', () => {
  const seq = (address, audition = true) => ({
    address,
    module: 'std.note_seq',
    ports: [
      { name: 'position', direction: 'out', shape: 'signal' },
      ...(audition ? [{ name: 'audition', direction: 'in', shape: 'events' }] : []),
    ],
  });
  const one = { instances: [seq('/melody'), { address: '/out', module: 'std.audio_out', ports: [] }] };
  assert.equal(auditionTarget({ positionBind: '/melody.position' }, one), '/melody.audition');
  assert.equal(auditionTarget({}, one), '/melody.audition', 'the only sequencer');
  assert.equal(auditionTarget({ audition: '/lead.audition' }, one), '/lead.audition', 'as given');
  assert.equal(auditionTarget({ audition: 'none', positionBind: '/melody.position' }, one), null);
  const two = { instances: [seq('/a'), seq('/b')] };
  assert.equal(auditionTarget({}, two), null, 'which one: not guessed');
  assert.equal(auditionTarget({ positionBind: '/b.position' }, two), '/b.audition');
  assert.equal(auditionTarget({ positionBind: '/old.position' }, { instances: [seq('/old', false)] }), null, 'no audition input');
  // A playhead that follows something without `audition` (a step grid's, a transport): the only sequencer.
  const other = { address: '/transport', module: 'lib.transport', ports: [{ name: 'position', direction: 'out', shape: 'signal' }] };
  assert.equal(auditionTarget({ positionBind: '/transport.position' }, { instances: [other, seq('/melody')] }), '/melody.audition');
  assert.equal(auditionTarget({ positionBind: '/transport.position' }, { instances: [other, seq('/a'), seq('/b')] }), null);
  assert.equal(auditionTarget({ positionBind: '/melody.position' }, null), null, 'not described yet');
  // Inside an include or a composite: the followed sequencer's address.
  assert.equal(auditionTarget({ positionBind: '/studio/melody.position' }, { instances: [seq('/studio/melody'), seq('/x')] }), '/studio/melody.audition');
});
