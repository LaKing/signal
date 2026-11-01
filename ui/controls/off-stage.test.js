// A control whose streams only show values (a knob's parameter, a meter)
// stops them while it is off the stage and starts them again when it comes
// back (ADR 0051 test 9, docs/benchmarks.md: the plane's Stacks scrolled
// away and the hidden Engine board kept every front panel's stream).
// Driven through `SignalElement.attach` with a stub IntersectionObserver.

import test from 'node:test';
import assert from 'node:assert/strict';

globalThis.HTMLElement ??= class {};
globalThis.customElements ??= { get: () => undefined, define: () => {} };

const observed = new Set();
let notify = null;
/** The browser's observer, stubbed while `fn` runs (every test file shares one process: ui/index.js). */
function stubbed(fn) {
  const before = globalThis.IntersectionObserver;
  globalThis.IntersectionObserver = class {
    constructor(callback) {
      notify = (target, shown) => callback([{ target, isIntersecting: shown }]);
    }
    observe(element) {
      observed.add(element);
    }
    unobserve(element) {
      observed.delete(element);
    }
  };
  try {
    fn();
  } finally {
    globalThis.IntersectionObserver = before;
  }
}

const { SignalElement, SignalValueElement } = await import('./base.js');

/** A connection counting its parameter subscriptions and their stops. */
function connection() {
  const c = { subscribed: 0, stopped: 0, listeners: 0 };
  c.on = () => {
    c.listeners++;
    return () => c.listeners--;
  };
  c.subscribe = () => {
    c.subscribed++;
    return () => c.stopped++;
  };
  return c;
}

/** A control of class `Class` bound to `/a.freq`, without a DOM. */
function control(Class) {
  const element = Object.create(Class.prototype);
  Object.assign(element, { stops: [], streamStops: null, offStage: false, connection: null });
  element.binding = { address: '/a.freq', frame() {}, fromHost() {} };
  element.update = () => {};
  element.described = () => {};
  return element;
}

test('a knob subscribes once on the stage, stops off it, starts again on it, stops when detached', () => stubbed(() => {
  const knob = control(SignalValueElement);
  const c = connection();
  knob.attach(c);
  assert.ok(observed.has(knob), 'watched');
  assert.equal(c.subscribed, 0, 'not before the observer says where it is');
  notify(knob, true);
  assert.equal(c.subscribed, 1);
  notify(knob, true);
  assert.equal(c.subscribed, 1, 'still on: no second subscription');
  notify(knob, false);
  assert.equal(c.stopped, 1, 'off the stage: unsubscribed');
  notify(knob, true);
  assert.equal(c.subscribed, 2, 'back: subscribed again');
  knob.detach();
  assert.equal(c.stopped, 2);
  assert.ok(!observed.has(knob), 'no longer watched');
  notify(knob, true);
  assert.equal(c.subscribed, 2, 'a late word after detaching starts nothing');
}));

test('attached again while known on the stage: subscribed at once', () => stubbed(() => {
  const knob = control(SignalValueElement);
  const c = connection();
  knob.attach(c);
  notify(knob, true);
  knob.attach(c);
  assert.equal(c.subscribed, 2);
  assert.equal(c.stopped, 1);
}));

test('a control whose streams keep state never pauses', () => stubbed(() => {
  class Keeper extends SignalElement {
    streams(connection) {
      return [connection.subscribe({ stream: 'parameters', addresses: ['/a.freq'] }, () => {})];
    }
  }
  const keeper = control(Keeper);
  const c = connection();
  keeper.attach(c);
  assert.equal(c.subscribed, 1, 'subscribed at once');
  assert.ok(!observed.has(keeper), 'not watched');
  keeper.detach();
  assert.equal(c.stopped, 1);
}));

test('without IntersectionObserver every control is on the stage', () => {
  assert.equal(typeof globalThis.IntersectionObserver, 'undefined', 'node has none');
  const knob = control(SignalValueElement);
  const c = connection();
  knob.attach(c);
  assert.equal(c.subscribed, 1);
});
