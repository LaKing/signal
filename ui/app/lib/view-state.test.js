// ADR 0047 §3, §5: a window's view state, said to and told by Signal.app (ui/app/lib/view-state.js).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { connectViewState, withZoom, zoomOf, zoomState } from './view-state.js';

test('the plate’s zoom as the state says it, and back', () => {
  assert.deepEqual(zoomState({ kind: 'width' }), {});
  assert.deepEqual(zoomState({ kind: 'whole' }), { zoom: 'whole' });
  assert.deepEqual(zoomState({ kind: 'manual', scale: 1.25 }), { scale: 1.25 });
  assert.deepEqual(zoomOf({}), { kind: 'width' });
  assert.deepEqual(zoomOf({ zoom: 'whole' }), { kind: 'whole' });
  assert.deepEqual(zoomOf({ scale: 2 }), { kind: 'manual', scale: 2 });
  assert.deepEqual(withZoom({ layer: 'xray', scale: 2 }, { kind: 'whole' }), { layer: 'xray', zoom: 'whole' });
});

/** A page's window with `ipc` and events. */
function page() {
  const listeners = new Map();
  const posted = [];
  return {
    posted,
    ipc: { postMessage: (text) => posted.push(JSON.parse(text)) },
    addEventListener: (name, fn) => listeners.set(name, fn),
    removeEventListener: (name) => listeners.delete(name),
    fire: (name, detail) => listeners.get(name)?.({ detail }),
    has: (name) => listeners.has(name),
  };
}

test('in Signal.app: a change is said whole, the kept state shown; outside it, nothing', () => {
  const target = page();
  const schematic = page();
  const restored = [];
  schematic.board = { restore: (s) => restored.push(s) };
  const stop = connectViewState({ target, schematic, ready: true });
  assert.deepEqual(target.posted, [{ type: 'window.ready' }]);
  target.fire('signal-view', { layer: 'plate', scale: 1.5 });
  assert.deepEqual(restored, [{ layer: 'plate', scale: 1.5 }]);
  schematic.fire('signal-view-change', { layer: 'xray', arrange: 'flow', hidden: [] });
  assert.deepEqual(target.posted.at(-1), { type: 'view.state', state: { layer: 'xray', arrange: 'flow', hidden: [], scale: 1.5 } });
  stop();
  assert.ok(!target.has('signal-view') && !schematic.has('signal-view-change'));
  const browser = { addEventListener() {} };
  assert.equal(typeof connectViewState({ target: browser, schematic }), 'function');
});
