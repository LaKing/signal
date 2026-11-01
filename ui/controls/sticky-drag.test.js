// A knob or a fader grabbed by a click follows the pointer with no button
// held, until the next press or Escape (`stickyDrag`, the owner's, 2026-11-02;
// ADR 0051 "Amendment (2026-11-02)").

import test from 'node:test';
import assert from 'node:assert/strict';

globalThis.HTMLElement ??= class {};
globalThis.customElements ??= { get: () => undefined, define: () => {} };

const { SignalValueElement } = await import('./base.js');

const listeners = new Map();
const stubDocument = {
  addEventListener: (type, fn) => listeners.set(type, fn),
  removeEventListener: (type, fn) => listeners.get(type) === fn && listeners.delete(type),
};

/** Runs `body` with a document that records its listeners (the page's own is put back after). */
function withDocument(body) {
  const had = Object.prototype.hasOwnProperty.call(globalThis, 'document');
  const before = globalThis.document;
  globalThis.document = stubDocument;
  listeners.clear();
  try {
    body();
  } finally {
    if (had) globalThis.document = before;
    else delete globalThis.document;
  }
}

/** A control enough for `grab`: its drag moves the position by the pointer's rise, in 100 px. */
function control() {
  const attributes = new Map();
  const moves = [];
  const c = {
    attributes,
    moves,
    committed: 0,
    setAttribute: (n, v) => attributes.set(n, v),
    removeAttribute: (n) => attributes.delete(n),
    releasePointerCapture() {},
    clicks: { reset() {} },
    binding: { position: 0.5, moveTo: (p) => moves.push(p) },
    dragStart(e) {
      this.last = e.clientY;
    },
    dragMove(e) {
      const p = this.binding.position + (this.last - e.clientY) / 100;
      this.last = e.clientY;
      this.binding.position = p;
      return p;
    },
    dragEnd() {
      if (this.moved) this.committed++;
    },
    update() {},
  };
  c.grab = SignalValueElement.prototype.grab;
  c.letGo = SignalValueElement.prototype.letGo;
  return c;
}

test('a click grabs: the pointer moves the value with no button held; a press elsewhere lets go and does nothing else', () => withDocument(() => {
  const c = control();
  c.grab({ pointerId: 1, clientY: 100 });
  assert.ok(c.attributes.has('grabbed'));
  listeners.get('pointermove')({ clientY: 90 });
  listeners.get('pointermove')({ clientY: 80 });
  assert.deepEqual(c.moves.map((p) => Math.round(p * 100) / 100), [0.6, 0.7]);
  let stopped = false;
  listeners.get('pointerdown')({ composedPath: () => [], stopPropagation: () => (stopped = true), preventDefault() {} });
  assert.equal(stopped, true, 'the press elsewhere only lets go');
  assert.equal(c.attributes.has('grabbed'), false);
  assert.equal(c.committed, 1, 'what it set is committed');
  assert.equal(listeners.size, 0, 'no listener left');
}));

test('Escape lets go; a press on the control itself goes on (a double-click still resets)', () => withDocument(() => {
  const c = control();
  c.grab({ pointerId: 1, clientY: 100 });
  listeners.get('keydown')({ key: 'Escape', stopPropagation() {}, preventDefault() {} });
  assert.equal(c.attributes.has('grabbed'), false);
  c.grab({ pointerId: 1, clientY: 100 });
  let stopped = false;
  listeners.get('pointerdown')({ composedPath: () => [c], stopPropagation: () => (stopped = true), preventDefault() {} });
  assert.equal(stopped, false);
  assert.equal(c.released, true, 'its release grabs nothing again');
}));
