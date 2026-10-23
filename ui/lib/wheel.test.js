import test from 'node:test';
import assert from 'node:assert/strict';

import { GESTURE_GAP_MS, WheelLatch, fillsWindow, ownsWheel, wheelHint } from './wheel.js';

test('a control owns the wheel when it fills the window, has the focus or the modifier is held', () => {
  assert.equal(ownsWheel({}), false);
  assert.equal(ownsWheel({ fills: true }), true);
  assert.equal(ownsWheel({ focused: true }), true);
  assert.equal(ownsWheel({ focused: true, released: true }), false);
  assert.equal(ownsWheel({ focused: true, released: true, modifier: true }), true);
  assert.equal(ownsWheel({ fills: true, never: true }), false);
});

test('fillsWindow: most of the window', () => {
  assert.equal(fillsWindow({ width: 1200, height: 760 }, { width: 1200, height: 800 }), true);
  assert.equal(fillsWindow({ width: 600, height: 300 }, { width: 1200, height: 800 }), false);
  assert.equal(fillsWindow(null, { width: 1, height: 1 }), false);
});

test('a gesture is decided by its first event and keeps going where it started', () => {
  const latch = new WheelLatch();
  const roll = {};
  // The page scrolls; the pointer passes over a focused piano roll: still the page.
  latch.seen(0);
  latch.settle();
  latch.seen(16);
  assert.equal(latch.claim(roll, true), false);
  latch.settle();
  assert.equal(latch.owner, null);
  // A pause: a new gesture, which the roll takes.
  latch.seen(16 + GESTURE_GAP_MS + 1);
  assert.equal(latch.claim(roll, true), true);
  // Momentum carries the pointer out of it: the page does not take over.
  latch.seen(16 + GESTURE_GAP_MS + 20);
  assert.equal(latch.holds(false), true);
  assert.equal(latch.claim(roll, false), true);
});

test('the hint shows once per gesture, only over the control the gesture started on', () => {
  const latch = new WheelLatch();
  const knob = {};
  latch.seen(0);
  latch.claim(knob, false);
  assert.equal(latch.hint(knob), true);
  latch.seen(10);
  assert.equal(latch.hint(knob), false, 'once');
  // A page scroll passing over the knob: no hint.
  latch.seen(1000);
  latch.settle();
  latch.seen(1010);
  latch.claim(knob, false);
  assert.equal(latch.hint(knob), false);
  // A gesture the knob took: no hint either.
  latch.seen(2000);
  latch.claim(knob, true);
  assert.equal(latch.hint(knob), false);
  // Slow notches scrolling the page: the page's gesture just before, so no hint.
  latch.seen(5000);
  latch.settle();
  latch.seen(5600);
  latch.claim(knob, false);
  assert.equal(latch.hint(knob), false, 'the page was being scrolled');
  latch.seen(9000);
  latch.claim(knob, false);
  assert.equal(latch.hint(knob), true, 'after a quiet moment');
  assert.equal(wheelHint('⌥'), 'Click to use the wheel here, or hold ⌥');
});
