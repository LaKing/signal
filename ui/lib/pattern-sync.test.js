// The pattern controls' adapter (ADR 0028 §7) against a fake connection
// with `readPattern`/`writePattern`: load on connect, debounced writes on
// the last base, one write at a time, conflicts (the edit kept only on the
// same base or meaning), refusals; and the controls' hook through their
// real `streams()`.

import test from 'node:test';
import assert from 'node:assert/strict';
import { PatternSync, WRITE_DELAY_MS, canSyncPatterns } from './pattern-sync.js';
import { parsePattern, serializePattern } from './pattern-text.js';

const settle = () => new Promise((resolve) => setImmediate(resolve));

const NOTES = (velocity = 1) => serializePattern({ schema_version: 1, kind: 'notes', length: 4, notes: [{ key: 60, start: 0, length: 1, velocity }] });

/** A server holding one file, its sha a counter; `writes` records each write. */
class FakePatternConnection {
  constructor(text) {
    this.text = text;
    this.version = 1;
    this.writes = [];
    this.reads = 0;
    this.refuse = null;
    this.gate = null;
  }

  get base() {
    return `sha-${this.version}`;
  }

  /** Another writer changes the file. */
  external(text) {
    this.text = text;
    this.version++;
  }

  async readPattern(name) {
    this.reads++;
    if (name !== 'melody' && name !== 'drums') throw Object.assign(new Error(`unknown pattern ${name}`), { code: 'unknown-pattern' });
    return { text: this.text, base: this.base };
  }

  async writePattern(name, text, base) {
    this.writes.push({ name, text, base });
    if (this.gate) await this.gate;
    if (this.refuse) throw Object.assign(new Error(this.refuse), { code: 'write-failed' });
    if (base !== this.base) throw Object.assign(new Error('the file changed'), { code: 'conflict', reply: { base: this.base, text: this.text } });
    parsePattern(text); // the server validates
    this.text = text;
    this.version++;
    return { base: this.base };
  }
}

function sync(connection, extra = {}) {
  const shown = [];
  const notices = [];
  const s = new PatternSync({
    name: 'melody',
    connection,
    kind: 'notes',
    onLoad: (pattern, text) => shown.push(text),
    onNotice: (text) => notices.push(text),
    ...extra,
  });
  return { s, shown, notices };
}

test('a connection with both methods can sync; others cannot', () => {
  assert.ok(canSyncPatterns(new FakePatternConnection('')));
  assert.ok(!canSyncPatterns({ readPattern() {} }));
  assert.ok(!canSyncPatterns(null));
});

test('load reads the file and shows it; a missing or wrong file is a notice', async () => {
  const c = new FakePatternConnection(NOTES());
  const { s, shown, notices } = sync(c);
  const pattern = await s.load();
  assert.equal(pattern.notes.length, 1);
  assert.deepEqual(shown, [NOTES()]);
  assert.equal(s.base, 'sha-1');
  const other = sync(c, { name: 'nope' });
  assert.equal(await other.s.load(), null);
  assert.match(other.notices.at(-1), /unknown pattern/);
  const wrong = sync(new FakePatternConnection('schema_version = 1\nkind = "steps"\nsteps = 1\nstep = 1\n[lanes]\na = "x"\n'));
  assert.equal(await wrong.s.load(), null);
  assert.match(wrong.notices.at(-1), /a steps pattern, not notes/);
  void notices;
});

test('edits are written once, after the delay, on the last base', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const c = new FakePatternConnection(NOTES());
  const { s } = sync(c);
  await s.load();
  s.edited(NOTES(0.5));
  t.mock.timers.tick(WRITE_DELAY_MS - 1);
  s.edited(NOTES(0.25));
  t.mock.timers.tick(WRITE_DELAY_MS - 1);
  assert.equal(c.writes.length, 0, 'debounced');
  t.mock.timers.tick(1);
  await settle();
  assert.deepEqual(c.writes, [{ name: 'melody', text: NOTES(0.25), base: 'sha-1' }]);
  assert.equal(s.base, 'sha-2');
  s.edited(NOTES(0.75));
  t.mock.timers.tick(WRITE_DELAY_MS);
  await settle();
  assert.equal(c.writes[1].base, 'sha-2', 'the next write names the written base');
  assert.equal(c.text, NOTES(0.75));
});

test('one write at a time: an edit during a write follows it on its base', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const c = new FakePatternConnection(NOTES());
  const { s } = sync(c);
  await s.load();
  let open;
  c.gate = new Promise((resolve) => (open = resolve));
  s.edited(NOTES(0.5));
  t.mock.timers.tick(WRITE_DELAY_MS);
  await settle();
  s.edited(NOTES(0.25));
  t.mock.timers.tick(WRITE_DELAY_MS);
  await settle();
  assert.equal(c.writes.length, 1, 'waits for the first');
  c.gate = null;
  open();
  await settle();
  t.mock.timers.tick(WRITE_DELAY_MS);
  await settle();
  assert.equal(c.writes.length, 2);
  assert.equal(c.writes[1].base, 'sha-2');
  assert.equal(c.text, NOTES(0.25));
});

test('flush writes at once (blur, leaving)', async () => {
  const c = new FakePatternConnection(NOTES());
  const { s } = sync(c);
  await s.load();
  s.edited(NOTES(0.5));
  await s.flush();
  assert.equal(c.text, NOTES(0.5));
  s.close();
});

test('a conflict with a changed meaning: the file wins, the edit is dropped and said so', async () => {
  const c = new FakePatternConnection(NOTES());
  const { s, shown, notices } = sync(c);
  await s.load();
  c.external(NOTES(0.1));
  s.edited(NOTES(0.5));
  await s.flush();
  await settle();
  assert.equal(c.text, NOTES(0.1), 'nothing overwritten');
  assert.deepEqual(shown.at(-1), NOTES(0.1), 'the file is shown');
  assert.match(notices.at(-1), /changed elsewhere/);
  assert.equal(s.base, 'sha-2');
  assert.equal(s.pending, null);
  // The next edit goes on the new base.
  s.edited(NOTES(0.2));
  await s.flush();
  assert.equal(c.text, NOTES(0.2));
});

test('a conflict where the file means the same (only its text changed): the edit is kept and written', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const c = new FakePatternConnection(NOTES());
  const { s, shown } = sync(c);
  await s.load();
  c.external(`# a comment someone added\n${NOTES()}`);
  s.edited(NOTES(0.5));
  await s.flush();
  await settle();
  assert.equal(shown.length, 1, 'not reloaded into the control');
  t.mock.timers.tick(WRITE_DELAY_MS);
  await settle();
  assert.equal(c.writes.at(-1).base, 'sha-2');
  assert.equal(c.text, NOTES(0.5));
});

test('a conflict whose re-read finds the base the edit started from keeps the edit', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const c = new FakePatternConnection(NOTES());
  const { s } = sync(c);
  await s.load();
  // A stale reply: the server says conflict, but the file is still the edit's base.
  const write = c.writePattern.bind(c);
  let once = true;
  c.writePattern = async (...args) => {
    if (once) {
      once = false;
      throw Object.assign(new Error('stale'), { code: 'conflict' });
    }
    return write(...args);
  };
  s.edited(NOTES(0.5));
  await s.flush();
  await settle();
  t.mock.timers.tick(WRITE_DELAY_MS);
  await settle();
  assert.equal(c.text, NOTES(0.5));
});

test('another refusal keeps the edit pending for a retry', async () => {
  const c = new FakePatternConnection(NOTES());
  const { s, notices } = sync(c);
  await s.load();
  c.refuse = 'disk full';
  s.edited(NOTES(0.5));
  await s.flush();
  assert.match(notices.at(-1), /not written: disk full/);
  assert.equal(s.pending, NOTES(0.5));
  c.refuse = null;
  await s.retry();
  assert.equal(c.text, NOTES(0.5));
  assert.equal(notices.at(-1), null, 'cleared');
  s.close();
});

test('close drains what is pending at once; later edits are ignored', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const c = new FakePatternConnection(NOTES());
  const { s } = sync(c);
  await s.load();
  s.edited(NOTES(0.5));
  await s.close();
  assert.equal(c.text, NOTES(0.5), 'written without waiting for the delay');
  s.edited(NOTES(0.25));
  t.mock.timers.tick(WRITE_DELAY_MS * 2);
  await settle();
  assert.equal(c.writes.length, 1);
});

test('an edit made while a write is in flight is still written when the control disconnects', async () => {
  const c = new FakePatternConnection(NOTES());
  let open;
  c.gate = new Promise((resolve) => (open = resolve));
  const { s } = sync(c, { timers: { setTimeout: () => 0, clearTimeout: () => {} } });
  await s.load();
  s.edited(NOTES(0.5));
  const first = s.flush();
  await settle();
  s.edited(NOTES(0.25)); // while the first write is in flight
  const closed = s.close();
  c.gate = null;
  open();
  await first;
  await closed;
  await settle();
  assert.deepEqual(c.writes.map((w) => w.base), ['sha-1', 'sha-2']);
  assert.equal(c.text, NOTES(0.25));
  assert.equal(s.pending, null);
});

test('a drain stops at a refusal', async () => {
  const c = new FakePatternConnection(NOTES());
  const { s, notices } = sync(c);
  await s.load();
  c.refuse = 'read-only';
  s.edited(NOTES(0.5));
  await s.close();
  assert.equal(c.writes.length, 1);
  assert.match(notices.at(-1), /read-only/);
});

test('edits before the first read are not taken (nothing is written over an unseen file)', async () => {
  const c = new FakePatternConnection(NOTES());
  const { s } = sync(c);
  s.edited(NOTES(0.5));
  assert.equal(s.pending, null);
  await s.load();
  await s.flush();
  assert.equal(c.writes.length, 0);
  assert.equal(c.text, NOTES());
});

test('text equal to the file is not written (an edit taken back)', async () => {
  const c = new FakePatternConnection(NOTES());
  const { s } = sync(c);
  await s.load();
  s.edited(NOTES(0.5));
  s.edited(NOTES());
  await s.flush();
  assert.equal(c.writes.length, 0);
});

test('a conflict whose file is now another kind is a notice, never loaded into the control', async () => {
  const c = new FakePatternConnection(NOTES());
  const { s, shown, notices } = sync(c);
  await s.load();
  c.external('schema_version = 1\nkind = "steps"\nsteps = 1\nstep = 1\n[lanes]\na = "x"\n');
  s.edited(NOTES(0.5));
  await s.flush();
  assert.equal(shown.length, 1, 'only the first load');
  assert.match(notices.at(-1), /a steps pattern, not notes/);
  assert.equal(c.writes.length, 1);
});

test('after 3 conflicts in a row it stops until the next edit', async () => {
  const c = new FakePatternConnection(NOTES());
  const queue = [];
  const timers = { setTimeout: (f) => queue.push(f), clearTimeout: () => {} };
  const { s, notices } = sync(c, { timers });
  await s.load();
  // A server that always answers conflict, its file unchanged (the same base).
  c.writePattern = async (name, text, base) => {
    c.writes.push({ name, text, base });
    throw Object.assign(new Error('conflict'), { code: 'conflict' });
  };
  s.edited(NOTES(0.5));
  queue.length = 0;
  await s.flush();
  for (let i = 0; i < 6 && queue.length; i++) {
    await queue.shift()();
    await settle();
  }
  assert.equal(c.writes.length, 3);
  assert.match(notices.at(-1), /3 times/);
  assert.equal(queue.length, 0, 'nothing scheduled');
  s.edited(NOTES(0.25));
  await s.flush();
  assert.equal(c.writes.length, 4, 'the next edit tries again');
});

test('a failing re-read after a conflict counts too', async () => {
  const c = new FakePatternConnection(NOTES());
  const queue = [];
  const { s, notices } = sync(c, { timers: { setTimeout: (f) => queue.push(f), clearTimeout: () => {} } });
  await s.load();
  c.external(NOTES(0.1));
  c.readPattern = async () => {
    throw new Error('offline');
  };
  s.edited(NOTES(0.5));
  queue.length = 0;
  await s.flush();
  for (let i = 0; i < 6 && queue.length; i++) {
    await queue.shift()();
    await settle();
  }
  assert.equal(c.writes.length, 3);
  assert.match(notices.at(-1), /3 times/);
});

test('a written edit stays written when showing the notice fails', async () => {
  const c = new FakePatternConnection(NOTES());
  const notices = [];
  const s = new PatternSync({
    name: 'melody',
    connection: c,
    onNotice: (text) => {
      notices.push(text);
      if (text === null && notices.length > 1) throw new Error('the control is gone');
    },
  });
  await s.load();
  const error = console.error;
  console.error = () => {};
  try {
    s.edited(NOTES(0.5));
    await s.flush();
  } finally {
    console.error = error;
  }
  assert.equal(s.base, 'sha-2');
  assert.equal(s.pending, null);
  assert.ok(!notices.some((n) => /not written/.test(n ?? '')));
});

// ---------------------------------------------------------------- the controls' hook

globalThis.HTMLElement ??= class {};
globalThis.customElements ??= { get: () => undefined, define: () => {} };
globalThis.CustomEvent ??= class extends Event {
  constructor(type, init) {
    super(type, init);
    this.detail = init?.detail;
  }
};
const { SignalPianoRoll } = await import('../controls/piano-roll.js');
const { SignalStepGrid } = await import('../controls/step-grid.js');

/** A control object without a DOM (its model set by the test): attributes, events. */
function bare(Class, attributes) {
  const control = Object.create(Class.prototype);
  control.getAttribute = (name) => attributes[name] ?? null;
  control.events = [];
  control.dispatchEvent = (event) => control.events.push(event);
  control.redraw = () => {};
  control.update = () => {};
  return control;
}

test('<signal-piano-roll pattern> loads through the connection and writes its edits', async () => {
  const { PianoRollModel } = await import('./piano-roll-model.js');
  const c = new FakePatternConnection(NOTES());
  const roll = bare(SignalPianoRoll, { pattern: 'melody' });
  roll.model = new PianoRollModel();
  const stops = roll.streams(c);
  await settle();
  assert.equal(roll.value.notes.length, 1, 'loaded on connect');
  assert.equal(roll.text, NOTES());
  roll.model.add(2, 64, { grid: 1 });
  roll.changed();
  assert.equal(roll.events.length, 1);
  assert.equal(roll.events[0].type, 'change');
  assert.equal(roll.events[0].detail.notes.length, 2);
  for (const stop of stops) stop(); // flushes the pending write, then closes
  await settle();
  assert.equal(c.writes.length, 1);
  assert.equal(c.writes[0].base, 'sha-1');
  assert.equal(parsePattern(c.text).notes.length, 2);
});

test('<signal-step-grid pattern> loads a steps pattern and writes its edits; without the methods it does nothing', async () => {
  const { StepGridModel } = await import('./step-grid-model.js');
  const drums = 'schema_version = 1\nkind = "steps"\nsteps = 4\nstep = 0.25\n\n[lanes]\nkick = "x..."\n';
  const c = new FakePatternConnection(drums);
  const grid = bare(SignalStepGrid, { pattern: 'drums' });
  grid.model = new StepGridModel();
  const stops = grid.streams(c);
  await settle();
  assert.equal(grid.value.lanes[0].hits, 'x...');
  grid.model.press(0, 2);
  grid.model.release();
  grid.changed();
  await grid.sync.flush();
  assert.match(c.text, /kick = "x\.x\."/);
  for (const stop of stops) stop();
  const plain = bare(SignalStepGrid, { pattern: 'drums' });
  plain.model = new StepGridModel();
  assert.deepEqual(plain.streams({ subscribe() {} }), []);
  assert.deepEqual(bare(SignalStepGrid, {}).streams(c), [], 'no pattern attribute');
});

test('setting .value checks the pattern', async () => {
  const { PianoRollModel } = await import('./piano-roll-model.js');
  const roll = bare(SignalPianoRoll, {});
  roll.model = new PianoRollModel();
  roll.view = { width: 0 };
  assert.throws(() => {
    roll.value = { schema_version: 1, kind: 'steps', steps: 1, step: 1, lanes: [] };
  }, /expected a "notes" pattern/);
  assert.throws(() => {
    roll.value = { schema_version: 1, kind: 'notes', length: 4, notes: [{ key: 60, start: 0, length: 2, velocity: 1 }, { key: 60, start: 1, length: 1, velocity: 1 }] };
  }, /overlaps/);
  roll.text = NOTES(0.5);
  assert.equal(roll.value.notes[0].velocity, 0.5);
});

test('<signal-piano-roll> takes no edits before its pattern is read; a reload ends a drag', async () => {
  const { PianoRollModel } = await import('./piano-roll-model.js');
  const c = new FakePatternConnection(NOTES());
  let release;
  const read = c.readPattern.bind(c);
  c.readPattern = async (name) => {
    await new Promise((resolve) => (release = resolve));
    return read(name);
  };
  const roll = bare(SignalPianoRoll, { pattern: 'melody' });
  roll.model = new PianoRollModel();
  roll.streams(c);
  assert.ok(roll.locked);
  assert.equal(roll.onKey({ key: 'Enter', composedPath: () => [] }), false, 'no keyboard add while loading');
  release();
  await settle();
  assert.ok(!roll.locked);
  // A drag in progress, then the file changes elsewhere and a write conflicts.
  const id = roll.model.notes[0].id;
  roll.model.beginDrag('move', id, 0, 60);
  roll.model.dragTo(2, 64, { grid: 1 });
  roll.gesture = { kind: 'drag' };
  c.readPattern = read;
  c.external(NOTES(0.1));
  roll.sync.edited(NOTES(0.9));
  await roll.sync.flush();
  assert.equal(roll.model.drag, null, 'the drag ended');
  assert.equal(roll.gesture, null);
  assert.equal(roll.value.notes[0].velocity, 0.1, 'the file is shown');
  assert.equal(roll.model.endDrag(), false, 'its release changes nothing');
});

test('an edit that cannot be written is a notice, the change event still fires, nothing is thrown', async () => {
  const { PianoRollModel } = await import('./piano-roll-model.js');
  const c = new FakePatternConnection(NOTES());
  const roll = bare(SignalPianoRoll, { pattern: 'melody' });
  roll.model = new PianoRollModel();
  roll.streams(c);
  await settle();
  // An overlap the model would never make, forced in.
  roll.model.notes.push({ id: 99, key: 60, start: 0.5, length: 1, velocity: 1 });
  assert.doesNotThrow(() => roll.changed());
  assert.match(roll.notice, /not written: .*overlaps/);
  assert.equal(roll.events.length, 1);
  await roll.sync.flush();
  assert.equal(c.writes.length, 0);
});

test('<signal-step-grid> takes no edits before its pattern is read', async () => {
  const { StepGridModel } = await import('./step-grid-model.js');
  const c = new FakePatternConnection('schema_version = 1\nkind = "steps"\nsteps = 4\nstep = 0.25\n\n[lanes]\nkick = "x..."\n');
  let release;
  const read = c.readPattern.bind(c);
  c.readPattern = async (name) => {
    await new Promise((resolve) => (release = resolve));
    return read(name);
  };
  const grid = bare(SignalStepGrid, { pattern: 'drums' });
  grid.model = new StepGridModel();
  grid.cursor = { lane: 0, step: 0 };
  grid.streams(c);
  assert.equal(grid.edit(() => grid.model.setSteps(8)), false);
  assert.equal(grid.onKey({ key: ' ', composedPath: () => [] }), false);
  release();
  await settle();
  assert.equal(grid.onKey({ key: ' ', composedPath: () => [] }), true);
  assert.equal(grid.value.lanes[0].hits, 'X...');
  await grid.sync.close();
  assert.match(c.text, /kick = "X\.\.\."/);
});

/**
 * The real connection's behaviour: `status` 'connecting' until the server's
 * `welcome`, every request refused at once (`closed`) until then and while
 * it is down, a `status` event on each change.
 */
class StatusConnection extends FakePatternConnection {
  constructor(text) {
    super(text);
    this.status = 'connecting';
    this.listeners = new Map();
  }

  on(event, fn) {
    if (!this.listeners.has(event)) this.listeners.set(event, new Set());
    this.listeners.get(event).add(fn);
    return () => this.listeners.get(event).delete(fn);
  }

  emit(event, value) {
    for (const fn of [...(this.listeners.get(event) ?? [])]) fn(value);
  }

  /** How many listeners are registered, all events. */
  get listening() {
    return [...this.listeners.values()].reduce((n, set) => n + set.size, 0);
  }

  set(status) {
    this.status = status;
    this.emit('status', status);
  }

  /** Another client writes the file through the server, which pushes `pattern.written`. */
  otherClientWrites(text, { withText = true } = {}) {
    this.external(text);
    this.emit('pattern.written', { type: 'pattern.written', name: 'melody', base: this.base, ...(withText ? { text } : {}) });
  }

  refuseUnlessOpen() {
    if (this.status !== 'open') throw Object.assign(new Error('connection closed'), { code: 'closed' });
  }

  async readPattern(name) {
    this.refuseUnlessOpen();
    return super.readPattern(name);
  }

  async writePattern(name, text, base) {
    this.refuseUnlessOpen();
    return super.writePattern(name, text, base);
  }
}

test('start() reads the file once the connection opens, not before', async () => {
  const c = new StatusConnection(NOTES());
  const { s, shown, notices } = sync(c);
  const stop = s.start();
  await settle();
  assert.equal(c.reads, 0, 'nothing sent while connecting');
  assert.equal(s.loaded, false);
  assert.deepEqual(notices, []);
  c.set('open');
  await settle();
  assert.equal(s.loaded, true);
  assert.deepEqual(shown, [NOTES()]);
  stop();
  c.set('reconnecting');
  c.set('open');
  await settle();
  assert.equal(c.reads, 1, 'stopped: no more reads');
});

test('start() on an open connection (or one without a status) reads at once', async () => {
  const open = new StatusConnection(NOTES());
  open.status = 'open';
  const a = sync(open);
  a.s.start();
  await settle();
  assert.equal(a.s.loaded, true);
  const plain = new FakePatternConnection(NOTES());
  const b = sync(plain);
  b.s.start();
  await settle();
  assert.equal(b.s.loaded, true);
});

test('after a reconnect the file is read again: shown only when it changed', async () => {
  const c = new StatusConnection(NOTES());
  c.status = 'open';
  const { s, shown } = sync(c);
  s.start();
  await settle();
  c.set('reconnecting');
  c.set('open');
  await settle();
  assert.equal(c.reads, 2);
  assert.equal(shown.length, 1, 'unchanged: not shown again');
  c.set('reconnecting');
  c.external(NOTES(0.3));
  c.set('open');
  await settle();
  assert.deepEqual(shown, [NOTES(), NOTES(0.3)]);
  assert.equal(s.base, c.base);
});

test('a write refused by a dropped connection is sent again when it opens', async () => {
  const c = new StatusConnection(NOTES());
  c.status = 'open';
  const { s, notices } = sync(c, { delay: 0 });
  s.start();
  await settle();
  c.set('reconnecting');
  s.edited(NOTES(0.5));
  await s.flush();
  assert.equal(c.text, NOTES());
  assert.match(notices.at(-1), /not written: connection closed/);
  c.set('open');
  await settle();
  await s.writing;
  assert.equal(c.text, NOTES(0.5), 'written on reconnect');
  assert.equal(notices.at(-1), null);
  assert.equal(c.reads, 1, 'the held edit is not replaced by a read');
});

test('a control attached while its connection connects loads once it opens', async () => {
  const { StepGridModel } = await import('./step-grid-model.js');
  const c = new StatusConnection('schema_version = 1\nkind = "steps"\nsteps = 4\nstep = 0.25\n\n[lanes]\nkick = "x..."\n');
  const grid = bare(SignalStepGrid, { pattern: 'drums' });
  grid.model = new StepGridModel();
  const stops = grid.streams(c);
  await settle();
  assert.ok(grid.locked);
  assert.equal(grid.notice ?? null, null, 'no "connection closed" notice');
  c.set('open');
  await settle();
  assert.ok(!grid.locked);
  assert.equal(grid.value.lanes[0].hits, 'x...');
  for (const stop of stops) stop();
  assert.equal(c.listening, 0, 'the listeners are removed');
});

test('<signal-piano-roll pattern> places its view for the file it loads, once', async () => {
  const { PianoRollModel, makeView, keyToY } = await import('./piano-roll-model.js');
  const wide = serializePattern({ schema_version: 1, kind: 'notes', length: 4, notes: [{ key: 60, start: 0, length: 1, velocity: 1 }, { key: 84, start: 2, length: 1, velocity: 1 }] });
  const c = new FakePatternConnection(wide);
  const roll = bare(SignalPianoRoll, { pattern: 'melody' });
  roll.model = new PianoRollModel();
  roll.canvases = {};
  // The view placed for the empty default pattern shown while reading (16 beats, around C4).
  roll.view = makeView({ width: 808, height: 400 });
  roll.place();
  assert.equal(roll.view.beatPx, 50);
  roll.streams(c);
  await settle();
  assert.equal(roll.view.beatPx, 200, 'the 4-beat loop fits the width');
  for (const key of [60, 84]) {
    const y = keyToY(key, roll.view);
    assert.ok(y >= 0 && y + roll.view.rowPx <= roll.view.height, `key ${key} in view`);
  }
  // A later reload (another writer, a conflict) keeps the user's view.
  roll.view = { ...roll.view, beatPx: 120 };
  c.external(NOTES(0.2));
  roll.sync.edited(NOTES(0.9));
  await roll.sync.flush();
  assert.equal(roll.value.notes[0].velocity, 0.2);
  assert.equal(roll.view.beatPx, 120);
});

test('a pattern.written push from another client is shown at once when idle', async () => {
  const c = new StatusConnection(NOTES());
  c.status = 'open';
  const { s, shown, notices } = sync(c);
  const stop = s.start();
  await settle();
  c.otherClientWrites(NOTES(0.4));
  assert.deepEqual(shown, [NOTES(), NOTES(0.4)], 'shown from the pushed text');
  assert.equal(s.base, c.base);
  assert.equal(c.reads, 1, 'no read needed');
  // Without text: read again.
  c.otherClientWrites(NOTES(0.5), { withText: false });
  await settle();
  assert.equal(c.reads, 2);
  assert.deepEqual(shown.at(-1), NOTES(0.5));
  // Another pattern's push, or our own base: nothing.
  c.emit('pattern.written', { type: 'pattern.written', name: 'drums', base: 'other', text: NOTES(0.6) });
  c.emit('pattern.written', { type: 'pattern.written', name: 'melody', base: s.base, text: NOTES(0.7) });
  assert.equal(shown.length, 3);
  assert.ok(!notices.some((n) => n && /changed elsewhere/.test(n)));
  stop();
  c.otherClientWrites(NOTES(0.8));
  assert.equal(shown.length, 3, 'stopped: not listening');
});

// External review, C: both changed (an edit pending here, the file changed
// elsewhere and pushed): the draft is kept, not written, and the conflict
// is said; the next edit writes it over the other version knowingly.
test('a pattern.written push while an edit is pending keeps the draft and says both changed', async () => {
  const { BOTH_CHANGED } = await import('./pattern-sync.js');
  const c = new StatusConnection(NOTES());
  c.status = 'open';
  const { s, shown, notices } = sync(c, { delay: 0 });
  s.start();
  await settle();
  s.edited(NOTES(0.9));
  c.otherClientWrites(NOTES(0.2));
  assert.equal(shown.length, 1, 'the draft is not replaced by the push');
  assert.equal(notices.at(-1), BOTH_CHANGED);
  await s.flush();
  assert.equal(c.text, NOTES(0.2), 'nothing written over the other version unasked');
  assert.equal(c.writes.length, 0);
  assert.equal(s.pending, NOTES(0.9), 'the draft is kept');
  // The user edits again: the draft is written over the other version.
  s.edited(NOTES(0.95));
  await s.flush();
  assert.equal(c.text, NOTES(0.95));
  assert.equal(notices.at(-1), null);
});

// External review, C: an external edit of the file (an editor, the server's
// reload) is pushed as any write; an idle control shows it at once.
test('a file changed on disk is shown at once by an idle control', async () => {
  const c = new StatusConnection(NOTES(1));
  c.status = 'open';
  const { s, shown } = sync(c);
  s.start();
  await settle();
  c.external(NOTES(0.25));
  c.emit('pattern.written', { type: 'pattern.written', name: 'melody', base: c.base, text: c.text, origin: 'file' });
  assert.deepEqual(shown, [NOTES(1), NOTES(0.25)]);
  assert.equal(s.base, c.base, 'the next write names the file now');
});

test('the control’s own write and its push: the push changes nothing', async () => {
  const c = new StatusConnection(NOTES());
  c.status = 'open';
  // The server pushes to every client, the writer included, before the reply reaches it.
  const write = c.writePattern.bind(c);
  c.writePattern = async (...args) => {
    const reply = await write(...args);
    c.emit('pattern.written', { type: 'pattern.written', name: 'melody', base: reply.base, text: c.text });
    return reply;
  };
  const { s, shown } = sync(c, { delay: 0 });
  s.start();
  await settle();
  s.edited(NOTES(0.3));
  await s.flush();
  assert.equal(c.text, NOTES(0.3));
  assert.equal(shown.length, 1, 'not reloaded by its own push');
  assert.equal(s.base, c.base);
});

test('shortNotice: the conflict notices fit the toolbar; others are kept', async () => {
  const { shortNotice, CHANGED_ELSEWHERE, MAX_CONFLICTS } = await import('./pattern-sync.js');
  assert.equal(shortNotice(CHANGED_ELSEWHERE), 'changed elsewhere — reloaded');
  assert.equal(shortNotice(`not written: the server answered conflict ${MAX_CONFLICTS} times; your next edit tries again`), 'not written — edit again to retry');
  assert.equal(shortNotice('not written: disk full'), 'not written: disk full');
  assert.equal(shortNotice(null), '');
});

// Second review of batch 2, item 6: a change pushed while the control's
// own write is in flight (another writer, after ours) is not lost: looked
// at once the write settles, it is shown (idle), as any push.
test('a push that comes while a write is in flight is shown once it settles', async () => {
  const c = new StatusConnection(NOTES());
  c.status = 'open';
  let release;
  const { s, shown } = sync(c, { delay: 0 });
  s.start();
  await settle();
  c.gate = new Promise((resolve) => {
    release = resolve;
  });
  s.edited(NOTES(0.3));
  const writing = s.flush();
  await settle();
  // Another writer's change lands after ours; its push comes while our
  // write's reply is still on the way.
  c.emit('pattern.written', { type: 'pattern.written', name: 'melody', base: 'sha-external', text: NOTES(0.8) });
  release();
  c.gate = null;
  await writing;
  await settle();
  assert.equal(shown.at(-1), NOTES(0.8), 'the push held during the write is shown after it');
  assert.equal(s.base, 'sha-external');
});
