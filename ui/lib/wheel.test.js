import test from 'node:test';
import assert from 'node:assert/strict';

import { GESTURE_GAP_MS, KIND_GAP_MS, NOTCH_MIN_PX, WheelKinds, WheelLatch, fillsWindow, ownsWheel, wheelHint, wheelKind } from './wheel.js';

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

test('wheelKind: a pinch is ctrl+wheel; a mouse notches in lines or large steps; a trackpad moves sideways or in small steps', () => {
  assert.equal(wheelKind({ deltaY: 3.5, ctrlKey: true }), 'pinch');
  assert.equal(wheelKind({ deltaY: -120, ctrlKey: true }), 'pinch');
  // Firefox's notch (lines), Chromium's (100 px), WebKit's (40 px × macOS's acceleration).
  assert.equal(wheelKind({ deltaY: 3, deltaMode: 1 }), 'mouse');
  assert.equal(wheelKind({ deltaY: 100, deltaMode: 0 }), 'mouse');
  assert.equal(wheelKind({ deltaY: -40, deltaMode: 0 }), 'mouse');
  assert.equal(wheelKind({ deltaY: 54.8, deltaX: 0, deltaMode: 0 }), 'mouse');
  assert.equal(wheelKind({ deltaY: NOTCH_MIN_PX }), 'mouse');
  assert.equal(wheelKind({ deltaY: 4.000244140625 }), 'mouse', "WebKit's slow notch");
  // A trackpad: sideways travel, small or fractional steps.
  assert.equal(wheelKind({ deltaY: 120, deltaX: 1, deltaMode: 0 }), 'trackpad');
  assert.equal(wheelKind({ deltaY: 0, deltaX: -6 }), 'trackpad');
  assert.equal(wheelKind({ deltaY: 2 }), 'trackpad');
  assert.equal(wheelKind({ deltaY: -0.5 }), 'trackpad');
  assert.equal(wheelKind({ deltaY: NOTCH_MIN_PX - 1 }), 'trackpad');
  // A mouse's Shift+wheel arrives sideways on a Mac: its size decides, not its direction.
  assert.equal(wheelKind({ deltaY: 0, deltaX: 40, shiftKey: true }), 'mouse');
  assert.equal(wheelKind({ deltaY: 0, deltaX: 3, shiftKey: true }), 'trackpad');
  assert.equal(wheelKind({ deltaY: 0, deltaX: 4.000244140625, shiftKey: true }), 'mouse');
});

test('WheelKinds: the first event decides the gesture; a pause of KIND_GAP_MS starts the next one', () => {
  const kinds = new WheelKinds();
  // A trackpad scroll whose momentum grows past a notch's size stays a trackpad's.
  assert.equal(kinds.kind({ deltaY: 2 }, 0), 'trackpad');
  assert.equal(kinds.kind({ deltaY: 60 }, 16), 'trackpad');
  assert.equal(kinds.kind({ deltaY: 120 }, 16 + KIND_GAP_MS), 'trackpad');
  // A pause: the mouse's notch is a new gesture.
  assert.equal(kinds.kind({ deltaY: 100 }, 400), 'mouse');
  assert.equal(kinds.kind({ deltaY: 1 }, 450), 'mouse', 'kept within the gesture');
  // A pinch is a pinch whenever ctrl is held, and what follows it is decided afresh.
  assert.equal(kinds.kind({ deltaY: 1, ctrlKey: true }, 460), 'pinch');
  assert.equal(kinds.kind({ deltaY: 1 }, 470), 'trackpad');
  assert.equal(kinds.kind({ deltaY: 100 }, 480), 'trackpad', 'kept');
  assert.equal(new WheelKinds().kind({ deltaY: -3, deltaMode: 1 }, 0), 'mouse');
});
