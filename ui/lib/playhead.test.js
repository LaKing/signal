// The pattern controls' playhead binding (ui/lib/playhead.js): a scope
// stream of a signal output, its newest point shown once per display frame,
// a refused slot asked again, nothing written.

import test from 'node:test';
import assert from 'node:assert/strict';
import { PLAYHEAD_RATE, playheadAddress, playheadStream } from './playhead.js';

/** A connection with `subscribe` only: records each subscription; `frame(values)` feeds the latest. */
class FakeStreams {
  constructor() {
    this.subscriptions = [];
    this.sent = []; // anything else asked of it (there must be nothing)
  }

  subscribe(stream, onFrame, callbacks) {
    const s = { stream, onFrame, callbacks, active: true };
    this.subscriptions.push(s);
    return () => {
      s.active = false;
    };
  }

  get live() {
    return this.subscriptions.filter((s) => s.active);
  }

  frame(values) {
    for (const s of this.live) s.onFrame({ values: Float32Array.from(values) });
  }
}

/** A manual frame clock and timers. */
function clock() {
  const frames = [];
  const timeouts = new Map();
  let next = 1;
  return {
    schedule: (fn) => frames.push(fn),
    tick() {
      for (const fn of frames.splice(0)) fn();
    },
    timers: {
      setTimeout: (fn, ms) => {
        const id = next++;
        timeouts.set(id, { fn, ms });
        return id;
      },
      clearTimeout: (id) => timeouts.delete(id),
    },
    fire() {
      const all = [...timeouts.values()];
      timeouts.clear();
      for (const { fn } of all) fn();
    },
    get pendingTimeouts() {
      return timeouts.size;
    },
  };
}

test('playheadAddress: an address, or null when blank', () => {
  assert.equal(playheadAddress(' /melody.position '), '/melody.position');
  assert.equal(playheadAddress(''), null);
  assert.equal(playheadAddress(null), null);
});

test('subscribes a scope stream of the port and shows the newest point once per frame', () => {
  const c = new FakeStreams();
  const k = clock();
  const shown = [];
  const stop = playheadStream(c, '/melody.position', (v) => shown.push(v), { schedule: k.schedule, timers: k.timers });
  assert.equal(c.subscriptions.length, 1);
  assert.deepEqual(c.subscriptions[0].stream, { stream: 'scope', port: '/melody.position', rate: PLAYHEAD_RATE });
  c.frame([0.1, 0.2, 0.25]);
  c.frame([0.3, 0.5]);
  assert.deepEqual(shown, [], 'nothing until the display frame');
  k.tick();
  assert.deepEqual(shown, [0.5], 'one value per frame: the newest');
  k.tick();
  assert.deepEqual(shown, [0.5], 'no new block, nothing shown');
  c.frame([]);
  k.tick();
  assert.deepEqual(shown, [0.5], 'an empty block is ignored');
  stop();
  c.frame([1]);
  k.tick();
  assert.deepEqual(shown, [0.5], 'stopped');
  assert.equal(c.live.length, 0, 'unsubscribed');
});

test('a refused slot is asked for again; another refusal is a problem, not retried', () => {
  const c = new FakeStreams();
  const k = clock();
  const problems = [];
  const stop = playheadStream(c, '/drums.step', () => {}, { schedule: k.schedule, timers: k.timers, onProblem: (p) => problems.push(p) });
  c.subscriptions[0].callbacks.onError({ code: 'no-scope-slot' });
  assert.deepEqual(problems, ['/drums.step: no-scope-slot']);
  assert.equal(c.live.length, 0);
  k.fire();
  assert.equal(c.subscriptions.length, 2, 'asked again');
  c.subscriptions[1].callbacks.onSubscribed({});
  assert.equal(problems.at(-1), null, 'subscribed: no problem');
  c.subscriptions[1].callbacks.onError({ code: 'not-a-signal-output' });
  assert.equal(problems.at(-1), '/drums.step: not-a-signal-output');
  assert.equal(k.pendingTimeouts, 0, 'not retried');
  stop();
});

test('stopping cancels a retry in wait', () => {
  const c = new FakeStreams();
  const k = clock();
  const stop = playheadStream(c, '/drums.step', () => {}, { schedule: k.schedule, timers: k.timers });
  c.subscriptions[0].callbacks.onError({ code: 'busy' });
  stop();
  assert.equal(k.pendingTimeouts, 0);
  assert.equal(c.subscriptions.length, 1);
});

test('no port or no subscribe: nothing', () => {
  assert.equal(typeof playheadStream(new FakeStreams(), null, () => {}), 'function');
  assert.equal(typeof playheadStream({}, '/a.b', () => {}), 'function');
});

globalThis.HTMLElement ??= class {};
globalThis.customElements ??= { get: () => undefined, define: () => {} };
const { SignalPianoRoll } = await import('../controls/piano-roll.js');
const { SignalStepGrid } = await import('../controls/step-grid.js');

/** A control object without a DOM, as in pattern-sync.test.js. */
function bare(Class, attributes) {
  const control = Object.create(Class.prototype);
  control.getAttribute = (name) => attributes[name] ?? null;
  control.redraw = () => {};
  control.update = () => {};
  control.requestDraw = () => {};
  control.updateRun = () => {};
  return control;
}

test('<signal-piano-roll position-bind> and <signal-step-grid step-bind> follow their outputs, writing nothing', async () => {
  const frames = [];
  const raf = globalThis.requestAnimationFrame;
  globalThis.requestAnimationFrame = (fn) => frames.push(fn);
  try {
    const c = new FakeStreams();
    // Neither control has a pattern here: the playhead works on its own.
    const roll = bare(SignalPianoRoll, { 'position-bind': '/melody.position' });
    const grid = bare(SignalStepGrid, { 'step-bind': '/drums.step' });
    const stops = [...roll.streams(c), ...grid.streams(c)];
    assert.deepEqual(c.subscriptions.map((s) => s.stream.port), ['/melody.position', '/drums.step']);
    for (const s of c.subscriptions) s.onFrame({ values: Float32Array.from([s.stream.port === '/drums.step' ? 5 : 2.5]) });
    for (const fn of frames.splice(0)) fn();
    assert.equal(roll.position, 2.5);
    assert.equal(grid.step, 5);
    for (const stop of stops) stop();
    assert.equal(roll.position, null, 'no playhead once stopped');
    assert.equal(grid.step, null);
    assert.equal(c.live.length, 0);
    // Without the attribute: no subscription.
    const plain = bare(SignalStepGrid, {});
    assert.deepEqual(plain.streams(c), []);
    assert.equal(c.subscriptions.length, 2);
  } finally {
    globalThis.requestAnimationFrame = raf;
  }
});

// Review 2026-10-31: a playhead claims one of the engine's 8 scope slots; it holds it only while its control is shown.
test('a playhead holds its slot only while its control is shown: the plate on Board (inert) lets it go, back on Plate it subscribes again; two views of one output share one', () => {
  const c = new FakeStreams();
  const k = clock();
  const listeners = new Map();
  const doc = {
    visibilityState: 'visible',
    addEventListener: (type, fn) => listeners.set(type, [...(listeners.get(type) ?? []), fn]),
    removeEventListener: (type, fn) => listeners.set(type, (listeners.get(type) ?? []).filter((f) => f !== fn)),
    dispatch: (type) => (listeners.get(type) ?? []).forEach((fn) => fn()),
  };
  const face = { inert: false, parentNode: null };
  const shadow = { parentNode: null, host: face };
  const control = (name) => ({ name, ownerDocument: doc, parentNode: shadow, getBoundingClientRect: () => ({ width: 300, height: 120 }) });
  const shown = [];
  const roll = control('roll');
  const stop = playheadStream(c, '/melody.position', (v) => shown.push(v), { schedule: k.schedule, timers: k.timers, element: roll });
  assert.equal(c.live.length, 1, 'shown: subscribed');
  // The plate goes to Board: its face inert, the faces say so.
  face.inert = true;
  doc.dispatch('signal-visibility');
  assert.equal(c.live.length, 0, 'Board: the slot let go');
  c.frame([1]);
  k.tick();
  assert.deepEqual(shown, [], 'nothing shown while hidden');
  face.inert = false;
  doc.dispatch('signal-visibility');
  assert.equal(c.live.length, 1, 'Plate again: subscribed again');
  assert.equal(c.subscriptions.length, 2);
  // The page hidden.
  doc.visibilityState = 'hidden';
  doc.dispatch('visibilitychange');
  assert.equal(c.live.length, 0, 'page hidden: let go');
  doc.visibilityState = 'visible';
  doc.dispatch('visibilitychange');
  assert.equal(c.live.length, 1);
  // A second view of the same output (the panel's and the plate's): one subscription.
  const other = playheadStream(c, '/melody.position', () => {}, { schedule: k.schedule, timers: k.timers, element: control('panel') });
  assert.equal(c.live.length, 1, 'shared');
  stop();
  assert.equal(c.live.length, 1, 'the other view keeps it');
  other();
  assert.equal(c.live.length, 0);
  assert.equal((listeners.get('signal-visibility') ?? []).length, 0, 'stopped: no listener left');
});

test('a playhead hidden while its refused slot waits does not ask until it is shown', () => {
  const c = new FakeStreams();
  const k = clock();
  const listeners = new Map();
  const doc = { addEventListener: (t, fn) => listeners.set(t, fn), removeEventListener: () => {} };
  const element = { ownerDocument: doc, parentNode: null, inert: false, getBoundingClientRect: () => ({ width: 10, height: 10 }) };
  const stop = playheadStream(c, '/drums.step', () => {}, { schedule: k.schedule, timers: k.timers, element });
  c.subscriptions[0].callbacks.onError({ code: 'no-scope-slot' });
  element.inert = true;
  listeners.get('signal-visibility')();
  k.fire();
  assert.equal(c.subscriptions.length, 1, 'hidden: not asked');
  element.inert = false;
  listeners.get('signal-visibility')();
  assert.equal(c.subscriptions.length, 2, 'shown: asked at once');
  stop();
});
