// The protocol client against an in-process fake WebSocket: no sockets.

import test from 'node:test';
import assert from 'node:assert/strict';

import { HEADER, KIND, PROTOCOL } from './lib/protocol.js';
import { Connection, connectFromPage, findMeter, findParameter } from './signal-ui.js';

class FakeSocket {
  static all = [];
  constructor(url) {
    this.url = url;
    this.sent = [];
    this.closed = false;
    FakeSocket.all.push(this);
  }
  send(text) {
    this.sent.push(JSON.parse(text));
  }
  close() {
    this.closed = true;
  }
  /** The server side. */
  open() {
    this.onopen();
  }
  receive(message) {
    this.onmessage({ data: JSON.stringify(message) });
  }
  drop(code = 1006) {
    this.onclose({ code });
  }
  last(type) {
    return this.sent.filter((m) => m.type === type).at(-1);
  }
}

function fakeTimers() {
  const waiting = [];
  return {
    waiting,
    setTimeout: (fn, ms) => waiting.push({ fn, ms }) - 1,
    clearTimeout: (handle) => {
      waiting[handle] = null;
    },
    runAll() {
      for (const entry of waiting.splice(0)) entry?.fn();
    },
  };
}

function connected() {
  FakeSocket.all = [];
  const timers = fakeTimers();
  const connection = new Connection({ url: 'ws://127.0.0.1:9/', token: 'tok', WebSocketImpl: FakeSocket, timers });
  const socket = FakeSocket.all[0];
  socket.open();
  socket.receive({ type: 'welcome', protocol: 1, client: 1, project: 'p', sampleRate: 48000, frame: 0, planVersion: 0 });
  return { connection, socket, timers };
}

function meterFrame(sub, peak) {
  const buffer = new ArrayBuffer(HEADER + 8);
  const view = new DataView(buffer);
  view.setUint8(0, KIND.METERS);
  view.setUint8(1, PROTOCOL);
  view.setUint32(4, sub, true);
  view.setUint32(16, 1, true);
  view.setUint32(20, 480, true);
  view.setFloat32(HEADER, peak, true);
  view.setFloat32(HEADER + 4, peak / 2, true);
  return buffer;
}

test('hello first, then describe once welcomed', () => {
  const { connection, socket } = connected();
  assert.deepEqual(socket.sent[0], { type: 'hello', protocol: 1, token: 'tok' });
  assert.equal(connection.status, 'open');
  assert.equal(socket.sent[1].type, 'describe');
  const seen = [];
  connection.on('describe', (d) => seen.push(d.project));
  socket.receive({ type: 'describe', id: socket.sent[1].id, project: 'p', controls: [] });
  socket.receive({ type: 'describe', project: 'pushed', controls: [] });
  assert.deepEqual(seen, ['p', 'pushed']);
  assert.equal(connection.description.project, 'pushed');
});

test('requests carry ids, and replies and errors settle them', async () => {
  const { connection, socket } = connected();
  const set = connection.set('/out.gain', 0.25);
  const ramp = connection.ramp('/a.freq', 300, { ms: 5 });
  const commit = connection.commit('/out.gain', 0.25);
  const trigger = connection.trigger('/accent.trigger');
  const [s, r, c, t] = socket.sent.slice(-4);
  assert.deepEqual([s.type, s.address, s.value], ['set', '/out.gain', 0.25]);
  assert.deepEqual([r.type, r.to, r.ms], ['ramp', 300, 5]);
  assert.deepEqual([c.type, c.value], ['commit', 0.25]);
  assert.deepEqual([t.type, t.address], ['trigger', '/accent.trigger']);
  assert.equal(new Set([s.id, r.id, c.id, t.id]).size, 4);
  socket.receive({ type: 'ack', id: s.id, frame: 10, requested: 10, late: false, outcome: 'scheduled' });
  socket.receive({ type: 'error', id: r.id, code: 'unknown-address', message: 'no /a.freq' });
  socket.receive({ type: 'committed', id: c.id, address: '/out.gain', value: 0.25, writes: 1 });
  socket.receive({ type: 'error', id: t.id, code: 'not-playing', message: 'stopped' });
  assert.equal((await set).outcome, 'scheduled');
  await assert.rejects(ramp, { code: 'unknown-address' });
  assert.equal((await commit).writes, 1);
  await assert.rejects(trigger, { code: 'not-playing' });
});

test('retry and revert, and the host unsaved state kept from describe and pushes', async () => {
  const { connection, socket } = connected();
  const retry = connection.retry('/out.gain');
  const revert = connection.revert('/out.gain');
  const [r, v] = socket.sent.slice(-2);
  assert.deepEqual([r.type, r.address, v.type, v.address], ['retry', '/out.gain', 'revert', '/out.gain']);
  socket.receive({ type: 'error', id: r.id, code: 'write-failed', message: 'read-only' });
  socket.receive({ type: 'reverted', id: v.id, address: '/out.gain', value: 1, sent: true });
  await assert.rejects(retry, { code: 'write-failed', message: 'read-only' });
  assert.equal((await revert).type, 'reverted');

  const unsaved = { count: 1, parameters: [{ address: '/out.gain', error: 'read-only' }], lastError: 'read-only' };
  socket.receive({ type: 'describe', planVersion: 0, controls: [], instances: [], unsaved });
  assert.deepEqual(connection.unsaved, unsaved);
  const pushed = [];
  connection.on('unsaved', (u) => pushed.push(u));
  socket.receive({ type: 'unsaved', count: 0, parameters: [], lastError: null });
  assert.equal(pushed.length, 1);
  assert.equal(connection.unsaved.count, 0);
});

test('frames go to their subscription by sub', () => {
  const { connection, socket } = connected();
  const meters = [];
  const replies = [];
  connection.subscribe({ stream: 'meters', rate: 30 }, (f) => meters.push(f.values[0].peak), { onSubscribed: (r) => replies.push(r.sub) });
  const request = socket.last('subscribe');
  assert.deepEqual([request.stream, request.rate], ['meters', 30]);
  socket.receive({ type: 'subscribed', id: request.id, sub: 5, stream: 'meters' });
  socket.onmessage({ data: meterFrame(5, 0.5) });
  socket.onmessage({ data: meterFrame(6, 0.25) });
  assert.deepEqual(meters, [0.5]);
  assert.deepEqual(replies, [5]);
});

test('a dropped connection rejects what is outstanding, reconnects and subscribes again', async () => {
  const { connection, socket, timers } = connected();
  const meters = [];
  const statuses = [];
  connection.on('status', (s) => statuses.push(s));
  connection.subscribe({ stream: 'meters' }, (f) => meters.push(f.values[0].peak));
  socket.receive({ type: 'subscribed', id: socket.last('subscribe').id, sub: 5, stream: 'meters' });
  const describe = connection.describe();
  const commit = connection.commit('/out.gain', 1);
  socket.drop(1006);
  await assert.rejects(describe, { code: 'closed' });
  await assert.rejects(commit, { code: 'closed' });
  await assert.rejects(connection.set('/out.gain', 1), { code: 'closed' }, 'made while down');
  assert.equal(connection.status, 'reconnecting');
  assert.equal(timers.waiting.length, 1);
  timers.runAll();
  const again = FakeSocket.all[1];
  assert.ok(again && again !== socket);
  again.open();
  assert.deepEqual(again.sent[0], { type: 'hello', protocol: 1, token: 'tok' }, 'the same token');
  again.receive({ type: 'welcome', protocol: 1 });
  assert.deepEqual(again.sent.slice(1).map((m) => m.type), ['describe', 'subscribe'], 'nothing replayed but the subscription');
  again.receive({ type: 'subscribed', id: again.last('subscribe').id, sub: 9, stream: 'meters' });
  again.onmessage({ data: meterFrame(5, 0.75) });
  again.onmessage({ data: meterFrame(9, 0.5) });
  assert.deepEqual(meters, [0.5], 'the old sub is gone, the new one delivers');
  assert.deepEqual(statuses, ['reconnecting', 'open']);
});

test('unauthorized or protocol closes are final', async () => {
  for (const code of [1008, 1002]) {
    const { connection, socket, timers } = connected();
    const pending = connection.describe();
    socket.drop(code);
    await assert.rejects(pending, { code: 'closed' });
    assert.equal(connection.status, 'failed');
    assert.equal(timers.waiting.length, 0);
  }
});

test('unsubscribing and closing', () => {
  const { connection, socket, timers } = connected();
  const stop = connection.subscribe({ stream: 'scope', port: '/a.out' }, () => {});
  socket.receive({ type: 'subscribed', id: socket.last('subscribe').id, sub: 3, stream: 'scope' });
  stop();
  assert.deepEqual([socket.last('unsubscribe').sub], [3]);
  connection.close();
  assert.equal(connection.status, 'closed');
  assert.ok(socket.closed);
  assert.equal(timers.waiting.length, 0, 'no reconnect after close()');
});

test('the page URL names the connection', () => {
  FakeSocket.all = [];
  assert.equal(connectFromPage({ hash: '', hostname: '127.0.0.1' }), null);
  const connection = connectFromPage({ hash: '#ws=50123&token=ab', hostname: '127.0.0.1' }, { WebSocketImpl: FakeSocket, timers: fakeTimers() });
  assert.equal(connection.url, 'ws://127.0.0.1:50123/');
  assert.equal(connection.token, 'ab');
});

test('parameters are found among controls and plan ports', () => {
  const description = {
    controls: [{ address: '/out.gain', min: 0, max: 4, default: 1, base: 0.5, unit: null }],
    instances: [{ parameters: [{ address: '/a.freq', unit: 'Hz', range: { min: 0, max: 24000 }, default: 440, base: 440 }] }],
  };
  assert.deepEqual(findParameter(description, '/out.gain'), { min: 0, max: 4, default: 1, unit: null, base: 0.5, control: true, kind: null });
  assert.deepEqual(findParameter(description, '/a.freq'), { min: 0, max: 24000, default: 440, unit: 'Hz', base: 440, control: false, kind: null });
  assert.equal(findParameter(description, '/nope'), null);
});

test('a meter is bound by the audio sink input it shows', () => {
  const description = {
    sinks: [{ index: 0, address: '/out', kind: 'audio', inputs: 2 }],
    instances: [
      { address: '/a', ports: [{ address: '/a.out', direction: 'out', shape: 'signal' }] },
      {
        address: '/out',
        ports: [
          { address: '/out.gain', direction: 'in', shape: 'value' },
          { address: '/out.l', direction: 'in', shape: 'signal' },
          { address: '/out.r', direction: 'in', shape: 'signal' },
        ],
      },
    ],
  };
  assert.deepEqual(findMeter(description, '/out.l'), { source: 'output', channel: 0 });
  assert.deepEqual(findMeter(description, '/out.r'), { source: 'output', channel: 1 });
  assert.equal(findMeter(description, '/a.out'), null);
  assert.equal(findMeter(description, '/out.gain'), null);
  assert.equal(findMeter(null, '/out.l'), null);
});

// M2.8-11 AC1: Signal.app's meters threw in their `describe` listener while
// playing, and the parameter list registered after them was never filled.
test('a throwing listener does not keep the later ones from the event', () => {
  const { connection, socket } = connected();
  const seen = [];
  const reported = [];
  const report = globalThis.reportError;
  globalThis.reportError = (error) => reported.push(error.message);
  try {
    connection.on('describe', () => {
      seen.push('meters');
      throw new TypeError('Cannot set property floor');
    });
    connection.on('describe', (d) => seen.push(`parameters ${d.controls.length}`));
    socket.receive({ type: 'describe', id: socket.last('describe').id, playing: true, controls: [{ address: '/osc.freq' }] });
  } finally {
    globalThis.reportError = report;
  }
  assert.deepEqual(seen, ['meters', 'parameters 1']);
  assert.deepEqual(reported, ['Cannot set property floor']);
});

// auditor-1-power-initial-state: a connection opened while Signal.app is off
// (the panel's frame after another project is opened) learns it from
// `welcome`, before it sends any subscription.
test('a connection welcomed while power is off sends no subscribe', () => {
  FakeSocket.all = [];
  const connection = new Connection({ url: 'ws://127.0.0.1:9/', token: 'tok', WebSocketImpl: FakeSocket, timers: fakeTimers() });
  connection.subscribe({ stream: 'parameters', addresses: ['/out.gain'] }, () => {});
  const socket = FakeSocket.all[0];
  socket.open();
  socket.receive({ type: 'welcome', protocol: 1, power: 'off' });
  assert.equal(connection.streaming, false);
  assert.equal(socket.last('subscribe'), undefined, 'nothing subscribed while off');
  connection.subscribe({ stream: 'meters' }, () => {});
  assert.equal(socket.last('subscribe'), undefined, 'nor a later subscription');
  socket.receive({ type: 'app', power: 'on' });
  assert.deepEqual(socket.sent.filter((m) => m.type === 'subscribe').map((m) => m.stream), ['parameters', 'meters']);
});

test('power off stops the streams and power on subscribes again', () => {
  const { connection, socket } = connected();
  const frames = [];
  const errors = [];
  connection.subscribe({ stream: 'meters' }, (f) => frames.push(f.values[0].peak), { onError: (e) => errors.push(e.code) });
  socket.receive({ type: 'subscribed', id: socket.last('subscribe').id, sub: 5, stream: 'meters' });
  socket.receive({ type: 'app', power: 'off' });
  assert.equal(socket.last('unsubscribe').sub, 5);
  socket.onmessage({ data: meterFrame(5, 0.5) });
  assert.deepEqual(frames, [], 'no frame reaches a stream let go');
  socket.receive({ type: 'app', power: 'off', playing: false });
  assert.equal(socket.sent.filter((m) => m.type === 'subscribe').length, 1, 'an unchanged power sends nothing');
  socket.receive({ type: 'app', power: 'on' });
  const again = socket.last('subscribe');
  assert.equal(socket.sent.filter((m) => m.type === 'subscribe').length, 2);
  // Refused because the power went off again before it arrived: kept, not an error.
  socket.receive({ type: 'error', id: again.id, code: 'powered-off', message: 'the power is off' });
  assert.deepEqual(errors, []);
  socket.receive({ type: 'app' });
  assert.equal(connection.streaming, true, 'signal ui has no power: it streams');
});
