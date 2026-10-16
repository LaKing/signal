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
  const { scope } = refusedScope(0);
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
