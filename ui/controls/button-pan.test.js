// The right or the middle button held pans (ui/controls/button-pan.js),
// shared by the Deck and its Engine board: a pan at once, a right click
// without a move replays its menu on the release, a double right click is
// the zoom lock's toggle and opens no menu, a press that is not this pan's
// is left alone.

import test from 'node:test';
import assert from 'node:assert/strict';
import { ButtonPan, isDoubleRight } from './button-pan.js';

globalThis.MouseEvent ??= class {
  constructor(type, init) {
    Object.assign(this, init, { type });
  }
};

function rig(options = {}) {
  const listeners = [];
  const host = { dataset: {}, ownerDocument: { addEventListener: (t, f) => listeners.push(f), removeEventListener: (t, f) => listeners.splice(listeners.indexOf(f), 1) } };
  const sent = [];
  const target = { dispatchEvent: (e) => sent.push(e) };
  const pans = [];
  let doubles = 0;
  const pan = new ButtonPan(host, { capture: null, pan: (dx, dy) => pans.push([dx, dy]), doubleRight: () => doubles++, ...options });
  const ev = (type, fields) => ({ type, pointerId: 1, timeStamp: 0, preventDefault() {}, stopPropagation() {}, composedPath: () => [target], ...fields });
  return { host, pan, pans, sent, ev, listeners, doubles: () => doubles };
}

test('the right button held pans at once; a release after a move opens no menu', () => {
  const r = rig();
  assert.ok(r.pan.down(r.ev('pointerdown', { button: 2, clientX: 10, clientY: 10 })));
  assert.equal(r.host.dataset.panning, '');
  r.pan.move(r.ev('pointermove', { clientX: 12, clientY: 15 }));
  r.pan.move(r.ev('pointermove', { clientX: 30, clientY: 15 }));
  assert.deepEqual(r.pans, [[2, 5], [18, 0]]);
  r.pan.up(r.ev('pointerup', { clientX: 30, clientY: 15, timeStamp: 100 }));
  assert.equal(r.host.dataset.panning, undefined);
  assert.equal(r.sent.length, 0);
});

test('a right click without a move replays its menu where it was pressed; the system one is held back meanwhile', () => {
  const r = rig();
  r.pan.down(r.ev('pointerdown', { button: 2, clientX: 40, clientY: 50 }));
  assert.equal(r.listeners.length, 1, 'the system right click is swallowed while the button is down');
  let stopped = false;
  r.listeners[0]({ preventDefault() {}, stopPropagation: () => (stopped = true) });
  assert.ok(stopped);
  r.pan.up(r.ev('pointerup', { clientX: 41, clientY: 50, timeStamp: 100 }));
  assert.equal(r.sent.length, 1);
  assert.deepEqual([r.sent[0].type, r.sent[0].clientX, r.sent[0].clientY, r.sent[0].signalReplayed], ['contextmenu', 40, 50, true]);
});

test('a double right click toggles (once) and opens no second menu; the middle button only pans', () => {
  const r = rig();
  r.pan.down(r.ev('pointerdown', { button: 2, clientX: 40, clientY: 50 }));
  r.pan.up(r.ev('pointerup', { clientX: 40, clientY: 50, timeStamp: 100 }));
  r.pan.down(r.ev('pointerdown', { button: 2, clientX: 42, clientY: 51 }));
  r.pan.up(r.ev('pointerup', { clientX: 42, clientY: 51, timeStamp: 300 }));
  assert.equal(r.doubles(), 1);
  assert.equal(r.sent.length, 1, 'only the first click replayed its menu');
  r.pan.down(r.ev('pointerdown', { button: 1, clientX: 0, clientY: 0 }));
  r.pan.up(r.ev('pointerup', { clientX: 0, clientY: 0, timeStamp: 900 }));
  assert.equal(r.sent.length, 1);
});

test('the left button and a skipped press are not a pan', () => {
  const r = rig({ skip: () => true });
  assert.equal(r.pan.down(r.ev('pointerdown', { button: 2, clientX: 0, clientY: 0 })), false);
  const s = rig();
  assert.equal(s.pan.down(s.ev('pointerdown', { button: 0, clientX: 0, clientY: 0 })), false);
  assert.equal(s.pan.move(s.ev('pointermove', { clientX: 5, clientY: 5 })), false);
});

test('isDoubleRight: soon and near', () => {
  assert.ok(isDoubleRight({ at: 0, x: 0, y: 0 }, 399, { x: 3, y: 3 }));
  assert.ok(!isDoubleRight({ at: 0, x: 0, y: 0 }, 400, { x: 0, y: 0 }));
  assert.ok(!isDoubleRight({ at: 0, x: 0, y: 0 }, 10, { x: 8, y: 0 }));
  assert.ok(!isDoubleRight(null, 10, { x: 0, y: 0 }));
});
