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
