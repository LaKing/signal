// <signal-scope>'s slot retries (M2.9-1b, AC12), driven through its real
// `streams()` with the least of a DOM it needs: a refused slot is asked
// again every 2 s only while the page and the scope itself are shown.

import test from 'node:test';
import assert from 'node:assert/strict';

globalThis.HTMLElement ??= class {};
globalThis.customElements ??= { get: () => undefined, define: () => {} };
globalThis.document ??= {};
document.addEventListener ??= () => {};
document.removeEventListener ??= () => {};
const observers = [];
globalThis.ResizeObserver = class {
  constructor(callback) {
    observers.push(callback);
  }
  observe() {}
  disconnect() {}
};

const { SignalScope } = await import('./scope.js');

/** A scope bound to `/a.out`, `size` px wide and high, whose subscription is refused. */
function refusedScope(size) {
  const scope = Object.create(SignalScope.prototype);
  scope.box = { width: size, height: size };
  scope.getAttribute = (name) => (name === 'bind' ? '/a.out' : null);
  scope.getBoundingClientRect = () => scope.box;
  scope.problem = () => {};
  scope.attached = 0;
  scope.attach = () => scope.attached++;
  const errors = [];
  const connection = {
    streaming: true,
    subscribe: (stream, onFrame, { onError }) => {
      errors.push(onError);
      return () => {};
    },
  };
  const stops = scope.streams(connection);
  errors[0]({ code: 'no-scope-slot', message: 'every scope slot is in use' });
  return { scope, stops };
}

// auditor-1-hidden-scope-retries: a scope in a tab not shown (0×0) in a
// visible page schedules nothing, and asks as soon as it is shown again.
test('a hidden scope does not ask again until it is shown', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  observers.length = 0;
  const { scope } = refusedScope(160);
  scope.box = { width: 0, height: 0 };
  for (const observer of observers) observer();
  t.mock.timers.tick(10000);
  assert.equal(scope.attached, 0, 'no retry while hidden');
  scope.box = { width: 300, height: 160 };
  for (const observer of observers) observer();
  assert.equal(scope.attached, 1, 'shown again: asked at once');
});

test('a shown scope asks again every 2 s', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const { scope } = refusedScope(160);
  t.mock.timers.tick(1999);
  assert.equal(scope.attached, 0);
  t.mock.timers.tick(1);
  assert.equal(scope.attached, 1);
});

test('a scope hidden while it waits does not ask when the 2 s are up', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const { scope } = refusedScope(160);
  scope.box = { width: 0, height: 0 };
  t.mock.timers.tick(2000);
  assert.equal(scope.attached, 0, 'parked instead');
});

// Review 2026-10-31 (P2): a hidden scope kept its capture slot. The engine has 8 and every subscription claims one.

/** A connection whose engine has `slots` scope slots, answering as the host does (a microtask later). */
function slotConnection(slots = 8) {
  const held = new Set();
  let next = 0;
  const c = {
    streaming: true,
    held,
    subscribes: 0,
    frames: new Map(),
    subscribe(stream, onFrame, { onSubscribed, onError } = {}) {
      c.subscribes++;
      const id = next++;
      let stopped = false;
      c.frames.set(stream.port, onFrame);
      queueMicrotask(() => {
        if (stopped) return;
        if (held.size < slots) {
          held.add(id);
          onSubscribed?.({ sub: id });
        } else onError?.({ code: 'no-scope-slot', message: 'every scope slot is in use' });
      });
      return () => {
        stopped = true;
        held.delete(id);
      };
    },
  };
  return c;
}

/** A scope bound to `port`, `size` px wide and high, streaming from `connection`. */
function scopeOn(connection, port, size = 160) {
  const scope = Object.create(SignalScope.prototype);
  scope.box = { width: size, height: size };
  scope.blocks = [];
  scope.latest = {};
  scope.getAttribute = (name) => (name === 'bind' ? port : null);
  scope.getBoundingClientRect = () => scope.box;
  scope.problems = [];
  scope.problem = (text) => scope.problems.push(text);
  scope.scheduled = 0;
  scope.schedule = () => scope.scheduled++;
  scope.attach = () => {};
  const first = observers.length;
  scope.stops = scope.streams(connection);
  scope.observe = () => {
    for (const observer of observers.slice(first)) observer();
  };
  return scope;
}

const settle = () => new Promise((r) => setTimeout(r, 0));

test('8 hidden scopes cannot starve a newly shown one: a hidden scope lets its capture slot go, and takes it again when shown', async () => {
  observers.length = 0;
  const connection = slotConnection(8);
  const scopes = Array.from({ length: 8 }, (_, i) => scopeOn(connection, `/s${i}.out`));
  await settle();
  assert.equal(connection.held.size, 8, 'eight shown scopes hold the eight slots');
  for (const scope of scopes) {
    scope.box = { width: 0, height: 0 };
    scope.observe();
  }
  assert.equal(connection.held.size, 0, 'hidden (0×0): every slot let go');
  const late = scopeOn(connection, '/late.out');
  await settle();
  assert.equal(connection.held.size, 1);
  assert.ok(!late.problems.some(Boolean), 'the newly shown scope has its slot');
  // One shown again takes a slot again.
  scopes[0].box = { width: 300, height: 160 };
  scopes[0].observe();
  await settle();
  assert.equal(connection.held.size, 2);
});

test('a frame while hidden draws nothing', async () => {
  observers.length = 0;
  const connection = slotConnection(8);
  const scope = scopeOn(connection, '/a.out');
  await settle();
  const onFrame = connection.frames.get('/a.out');
  onFrame({ firstFrame: 0, extra: 1, values: [0, 1] });
  assert.equal(scope.scheduled, 1, 'shown: drawn');
  scope.box = { width: 0, height: 0 };
  scope.observe();
  onFrame({ firstFrame: 2, extra: 1, values: [0, 1] });
  assert.equal(scope.scheduled, 1, 'hidden: a late frame draws nothing');
});

test('two views of one source share one capture: one subscription, both drawn, let go with the last', async () => {
  observers.length = 0;
  const connection = slotConnection(8);
  const a = scopeOn(connection, '/a.out');
  const b = scopeOn(connection, '/a.out');
  await settle();
  assert.equal(connection.subscribes, 1, 'one subscription for the source');
  assert.equal(connection.held.size, 1);
  connection.frames.get('/a.out')({ firstFrame: 0, extra: 1, values: [0, 1] });
  assert.equal(a.scheduled, 1);
  assert.equal(b.scheduled, 1);
  for (const stop of a.stops) stop();
  assert.equal(connection.held.size, 1, 'still shown in the other view');
  for (const stop of b.stops) stop();
  assert.equal(connection.held.size, 0, 'let go with the last view');
});
