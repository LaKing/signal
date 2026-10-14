import test from 'node:test';
import assert from 'node:assert/strict';

import { FINE, KEY_STEP, PAGE_STEP, circularDrag, circularPosition, dragDelta, keyAction, knobAngle, resetValue, wheelDelta } from './gesture.js';

const close = (a, b, what) => assert.ok(Math.abs(a - b) < 1e-12, `${what}: ${a} vs ${b}`);

test('fine adjust scales drags, wheel and keys by exactly a tenth', () => {
  assert.equal(FINE, 0.1);
  for (const px of [1, 37, -120]) {
    close(dragDelta(px, { fine: true }), dragDelta(px) * FINE, `drag ${px}`);
  }
  close(dragDelta(200), 1, 'the full travel');
  close(dragDelta(50, { travel: 100 }), 0.5, 'a control’s own travel');
  close(wheelDelta(-100, { fine: true }), wheelDelta(-100) * FINE, 'wheel');
  close(keyAction('ArrowUp', 0.5, { fine: true }) - 0.5, (keyAction('ArrowUp', 0.5) - 0.5) * FINE, 'arrow');
  close(keyAction('PageDown', 0.5, { fine: true }) - 0.5, (keyAction('PageDown', 0.5) - 0.5) * FINE, 'page');
});

test('the keys: arrows, page, home, end', () => {
  close(keyAction('ArrowUp', 0.5), 0.5 + KEY_STEP, 'up');
  close(keyAction('ArrowRight', 0.5), 0.5 + KEY_STEP, 'right');
  close(keyAction('ArrowDown', 0.5), 0.5 - KEY_STEP, 'down');
  close(keyAction('ArrowLeft', 0.5), 0.5 - KEY_STEP, 'left');
  close(keyAction('PageUp', 0.5), 0.5 + PAGE_STEP, 'page up');
  close(keyAction('PageDown', 0.5), 0.5 - PAGE_STEP, 'page down');
  assert.equal(keyAction('Home', 0.5), 0);
  assert.equal(keyAction('End', 0.5), 1);
  assert.equal(keyAction('PageUp', 0.95), 1);
  assert.equal(keyAction('ArrowDown', 0), 0);
  assert.equal(keyAction('a', 0.5), null);
});

test('the wheel: a notch up is one arrow step up', () => {
  close(wheelDelta(-100), KEY_STEP, 'pixels');
  close(wheelDelta(3, { deltaMode: 1 }), -KEY_STEP, 'lines');
});

test('circular drag follows the angle within the sweep', () => {
  close(circularPosition(0, -1), 0.5, 'straight up');
  close(circularPosition(-1, 1), 0, 'lower left, the start of 270°');
  close(circularPosition(1, 1), 1, 'lower right, the end of 270°');
  close(circularPosition(1, 0), 0.5 + 90 / 270, 'right');
  assert.equal(circularPosition(-0.01, 1), 0, 'below the sweep clamps');
  close(circularPosition(1, 0, 180), 1, 'a 180° sweep ends at 3 o’clock');
  close(knobAngle(0), -135, 'start');
  close(knobAngle(1, 300), 150, 'end');
});

test('circular fine adjust does not depend on how many events report the drag', () => {
  // A 90° Shift-drag clockwise from 12 o'clock, starting at position 0.5.
  const at = (degrees) => circularPosition(Math.sin((degrees * Math.PI) / 180), -Math.cos((degrees * Math.PI) / 180));
  const drag = (events) => {
    let position = 0.5;
    let last = at(0);
    for (let i = 1; i <= events; i++) {
      const now = at((90 * i) / events);
      position = circularDrag(position, last, now, { fine: true });
      last = now;
    }
    return position;
  };
  close(drag(1), 0.5 + (90 / 270) * FINE, 'one event');
  close(drag(100), drag(1), 'a hundred events');
  // The pointer staying where it is changes nothing.
  const still = at(45);
  assert.equal(circularDrag(0.7, still, still, { fine: true }), 0.7);
  // Without fine adjust the knob follows the pointer.
  assert.equal(circularDrag(0.2, at(0), at(90)), at(90));
});

test('reset goes to the default, else to min', () => {
  assert.equal(resetValue({ min: 0, max: 4, default: 1 }), 1);
  assert.equal(resetValue({ min: 20, max: 200, default: undefined }), 20);
  assert.equal(resetValue({ min: 0, max: 1, default: 3 }), 1);
});
